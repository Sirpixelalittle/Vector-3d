use glam::Vec3;

use crate::camera::{CameraBinding, shader_with_camera};

const INITIAL_VERTEX_CAPACITY: usize = 4096;
const INITIAL_INDEX_CAPACITY: usize = 8192;

/// Depth bias pushing occluder surfaces *away* from the camera so edges
/// drawn exactly on a face win the line pass's LessEqual test. Constant
/// term is in units of the smallest resolvable depth delta; slope term
/// scales with the primitive's depth gradient (grazing angles).
/// The one genuinely finicky knob in the whole technique — tune against
/// grazing-angle screenshots, not in the abstract.
const DEPTH_BIAS_CONSTANT: i32 = 2;
const DEPTH_BIAS_SLOPE_SCALE: f32 = 2.0;

/// Renders invisible occluder geometry into the depth buffer only (its own
/// depth-only pass). Runs before the line pass; the line pass then loads
/// this depth and tests against it — that is the hidden-line trick.
pub struct OccluderRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    index_capacity: usize,
    index_count: u32,
}

impl OccluderRenderer {
    pub fn new(device: &wgpu::Device, camera: &CameraBinding) -> Self {
        let shader = shader_with_camera(
            device,
            "occluder shader",
            include_str!("shaders/occluder.wgsl"),
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("occluder pipeline layout"),
            bind_group_layouts: &[Some(&camera.layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("occluder pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vec3>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // No culling: occluders may be open/single-sided sheets
                // (leaves), and hand-authored winding is not trustworthy.
                // Revisit per-model once the converter (M2) owns winding.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: DEPTH_BIAS_CONSTANT,
                    slope_scale: DEPTH_BIAS_SLOPE_SCALE,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            // Depth-only: no fragment shader, no color targets.
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        let vertex_buffer = Self::create_vertex_buffer(device, INITIAL_VERTEX_CAPACITY);
        let index_buffer = Self::create_index_buffer(device, INITIAL_INDEX_CAPACITY);

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            vertex_capacity: INITIAL_VERTEX_CAPACITY,
            index_capacity: INITIAL_INDEX_CAPACITY,
            index_count: 0,
        }
    }

    fn create_vertex_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("occluder vertices"),
            size: (capacity * std::mem::size_of::<Vec3>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn create_index_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("occluder indices"),
            size: (capacity * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Upload the occluder triangle soup for this frame.
    pub fn set_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[Vec3],
        indices: &[u32],
    ) {
        debug_assert_eq!(indices.len() % 3, 0, "indices must form whole triangles");
        if vertices.len() > self.vertex_capacity {
            self.vertex_capacity = vertices.len().next_power_of_two();
            self.vertex_buffer = Self::create_vertex_buffer(device, self.vertex_capacity);
        }
        if indices.len() > self.index_capacity {
            self.index_capacity = indices.len().next_power_of_two();
            self.index_buffer = Self::create_index_buffer(device, self.index_capacity);
        }
        if !vertices.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        }
        if !indices.is_empty() {
            queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(indices));
        }
        self.index_count = indices.len() as u32;
    }

    /// Record the depth-only occluder pass. Normally `clear_depth` is true:
    /// this pass opens the frame's depth buffer.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        camera: &CameraBinding,
        clear_depth: bool,
    ) {
        let depth_load = if clear_depth {
            wgpu::LoadOp::Clear(1.0)
        } else {
            wgpu::LoadOp::Load
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("occluder pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: depth_load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &camera.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }

    /// Like [`render`](Self::render), but draws only the given index
    /// ranges (frustum culling by instance).
    pub fn render_ranges(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        camera: &CameraBinding,
        clear_depth: bool,
        ranges: &[std::ops::Range<u32>],
    ) {
        let depth_load = if clear_depth {
            wgpu::LoadOp::Clear(1.0)
        } else {
            wgpu::LoadOp::Load
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("occluder pass (ranges)"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: depth_load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &camera.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for range in ranges {
            if range.start < range.end && range.end <= self.index_count {
                pass.draw_indexed(range.clone(), 0, 0..1);
            }
        }
    }
}
