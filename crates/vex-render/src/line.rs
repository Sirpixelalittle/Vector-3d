use vex_core::Segment;

use crate::camera::{CameraBinding, shader_with_camera};

const INITIAL_CAPACITY: usize = 1024;
const VERTICES_PER_SEGMENT: u32 = 6;

/// One line segment as GPU instance data.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SegmentInstance {
    pos_a: [f32; 3],
    pos_b: [f32; 3],
    color: [f32; 4],
    /// x = dash period in world units (0 = solid), y = flicker amount.
    style: [f32; 2],
}

impl From<&Segment> for SegmentInstance {
    fn from(segment: &Segment) -> Self {
        Self {
            pos_a: segment.a.to_array(),
            pos_b: segment.b.to_array(),
            color: segment.color.to_array(),
            style: [segment.dash_period, segment.flicker],
        }
    }
}

/// Additive blending: overlapping strokes sum toward white, and the cap
/// overlap where segments meet reads as the bright "beam dwell" dot of a
/// real vector CRT. Deliberate, not a bug.
const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Draws colored line segments as screen-space quads with round caps and
/// analytic anti-aliasing (see `shaders/line.wgsl`).
pub struct LineRenderer {
    pipeline: wgpu::RenderPipeline,
    instance_buffer: wgpu::Buffer,
    capacity: usize,
    count: u32,
    /// Reused staging area for instance conversion — dynamic callers
    /// upload every frame, and a fresh Vec per frame is avoidable churn.
    scratch: Vec<SegmentInstance>,
}

impl LineRenderer {
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        camera: &CameraBinding,
    ) -> Self {
        let shader = shader_with_camera(device, "line shader", include_str!("shaders/line.wgsl"));

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line pipeline layout"),
            bind_group_layouts: &[Some(&camera.layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("line pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SegmentInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32x4,
                        3 => Float32x2,
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                // Strokes are thin: they must not occlude each other's AA
                // fringe. They only get tested against occluders (M1).
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(ADDITIVE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let instance_buffer = Self::create_instance_buffer(device, INITIAL_CAPACITY);

        Self {
            pipeline,
            instance_buffer,
            capacity: INITIAL_CAPACITY,
            count: 0,
            scratch: Vec::new(),
        }
    }

    fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("line instances"),
            size: (capacity * std::mem::size_of::<SegmentInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Upload the segment list. Static scenes call this once at startup.
    pub fn set_segments(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        segments: &[Segment],
    ) {
        if segments.len() > self.capacity {
            self.capacity = segments.len().next_power_of_two();
            self.instance_buffer = Self::create_instance_buffer(device, self.capacity);
        }
        self.scratch.clear();
        self.scratch.extend(segments.iter().map(SegmentInstance::from));
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.scratch));
        }
        self.count = segments.len() as u32;
    }

    /// Record the line pass. `clear_color` is normally true (lines open the
    /// color target); `clear_depth` is true only when no occluder pass ran
    /// this frame — otherwise the pass loads the occluder depth and the
    /// depth test performs hidden-line removal.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        camera: &CameraBinding,
        clear_color: bool,
        clear_depth: bool,
    ) {
        let color_load = if clear_color {
            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
        } else {
            wgpu::LoadOp::Load
        };
        let depth_load = if clear_depth {
            wgpu::LoadOp::Clear(1.0)
        } else {
            wgpu::LoadOp::Load
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("line pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: color_load,
                    store: wgpu::StoreOp::Store,
                },
            })],
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

        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &camera.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        pass.draw(0..VERTICES_PER_SEGMENT, 0..self.count);
    }

    /// Like [`render`](Self::render), but draws only the given segment
    /// ranges — the frustum-culling path: one pass, one draw per visible
    /// instance's slice of the shared buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn render_ranges(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        camera: &CameraBinding,
        clear_color: bool,
        clear_depth: bool,
        ranges: &[std::ops::Range<u32>],
    ) {
        let color_load = if clear_color {
            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
        } else {
            wgpu::LoadOp::Load
        };
        let depth_load = if clear_depth {
            wgpu::LoadOp::Clear(1.0)
        } else {
            wgpu::LoadOp::Load
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("line pass (ranges)"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: color_load,
                    store: wgpu::StoreOp::Store,
                },
            })],
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
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        for range in ranges {
            if range.start < range.end && range.end <= self.count {
                pass.draw(0..VERTICES_PER_SEGMENT, range.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_layout_matches_vertex_attributes() {
        // 3 + 3 + 4 + 2 floats; stride must match the vertex_attr_array offsets.
        assert_eq!(std::mem::size_of::<SegmentInstance>(), 48);
    }
}
