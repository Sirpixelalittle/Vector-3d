//! HDR offscreen target + post chain: threshold-less mip bloom, exposure
//! soft-clip, optional CRT effects. Scene passes render into
//! [`PostProcessor::hdr_view`]; [`PostProcessor::run`] composites to the
//! swapchain (or a headless target).

use glam::Vec2;

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Bloom chain starts at half resolution and stops at this mip floor.
const BLOOM_MIN_DIM: u32 = 8;
const BLOOM_MAX_LEVELS: usize = 6;

/// The user-facing "phosphor" knob cluster. `glow` is applied in the line
/// shader (via [`crate::CameraUniform`]), the rest in the post chain —
/// together they are the engine-level brightness controls: assets author
/// *relative* emissive strengths, these decide what reaches the eye.
#[derive(Debug, Clone, Copy)]
pub struct PostSettings {
    pub exposure: f32,
    pub bloom_strength: f32,
    /// 0.0 clean … 1.0 full CRT (barrel distortion, chroma, vignette).
    pub crt: f32,
    /// HDR compression dial: 0 = no overbright at all, 1 = authored
    /// emissive strengths verbatim, >1 = hotter than authored.
    pub glow: f32,
}

impl Default for PostSettings {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            bloom_strength: 0.14,
            crt: 0.0,
            glow: 0.5,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PostParams {
    exposure: f32,
    bloom_strength: f32,
    crt: f32,
    /// 1.0 when the output format is linear (e.g. WebGPU swapchains, which
    /// have no sRGB surface formats) and the shader must encode sRGB
    /// itself; 0.0 when the target format already encodes.
    srgb_encode: f32,
    viewport: [f32; 2],
    _pad: [f32; 2],
}

pub struct PostProcessor {
    sampler: wgpu::Sampler,
    io_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    down_pipeline: wgpu::RenderPipeline,
    up_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    params_buffer: wgpu::Buffer,
    /// The composite must gamma-encode when the target is a linear format.
    srgb_encode: bool,

    size: (u32, u32),
    hdr_view: Option<wgpu::TextureView>,
    bloom_views: Vec<wgpu::TextureView>,
    /// Source bindings for downsample into mip i (HDR for i = 0).
    down_binds: Vec<wgpu::BindGroup>,
    /// Source bindings for upsample into mip i (reads mip i + 1).
    up_binds: Vec<wgpu::BindGroup>,
    composite_bind: Option<wgpu::BindGroup>,
}

impl PostProcessor {
    pub fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };

        let io_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post io layout"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post composite layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                sampler_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post params"),
            size: std::mem::size_of::<PostParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let make_pipeline = |label: &str,
                             layout: &wgpu::BindGroupLayout,
                             entry: &str,
                             format: wgpu::TextureFormat,
                             blend: Option<wgpu::BlendState>| {
            let pipeline_layout =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(label),
                    bind_group_layouts: &[Some(layout)],
                    immediate_size: 0,
                });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

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

        let down_pipeline =
            make_pipeline("bloom down", &io_layout, "fs_downsample", HDR_FORMAT, None);
        let up_pipeline = make_pipeline(
            "bloom up",
            &io_layout,
            "fs_upsample",
            HDR_FORMAT,
            Some(ADDITIVE),
        );
        let composite_pipeline = make_pipeline(
            "composite",
            &composite_layout,
            "fs_composite",
            output_format,
            None,
        );

        Self {
            sampler,
            io_layout,
            composite_layout,
            down_pipeline,
            up_pipeline,
            composite_pipeline,
            params_buffer,
            srgb_encode: !output_format.is_srgb(),
            size: (0, 0),
            hdr_view: None,
            bloom_views: Vec::new(),
            down_binds: Vec::new(),
            up_binds: Vec::new(),
            composite_bind: None,
        }
    }

    /// (Re)create the HDR target and bloom chain when the viewport changes.
    /// Call at the top of every frame.
    pub fn ensure_size(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.size == (width, height) || width == 0 || height == 0 {
            return;
        }
        self.size = (width, height);

        let make_texture = |label: &str, w: u32, h: u32| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };

        let hdr = make_texture("hdr scene", width, height);
        let hdr_view = hdr.create_view(&Default::default());

        // Bloom chain: half res, halving until the floor. Always at least
        // one level — the composite pass samples level 0 unconditionally.
        self.bloom_views.clear();
        let (mut w, mut h) = ((width / 2).max(1), (height / 2).max(1));
        loop {
            let texture = make_texture("bloom mip", w, h);
            self.bloom_views
                .push(texture.create_view(&Default::default()));
            if w / 2 < BLOOM_MIN_DIM
                || h / 2 < BLOOM_MIN_DIM
                || self.bloom_views.len() >= BLOOM_MAX_LEVELS
            {
                break;
            }
            w /= 2;
            h /= 2;
        }

        let io_bind = |src: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post io bind"),
                layout: &self.io_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
        };
        self.down_binds = std::iter::once(io_bind(&hdr_view))
            .chain(self.bloom_views.iter().map(&io_bind))
            .take(self.bloom_views.len())
            .collect();
        self.up_binds = self.bloom_views.iter().skip(1).map(&io_bind).collect();

        self.composite_bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite bind"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.bloom_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        }));
        self.hdr_view = Some(hdr_view);
    }

    /// The linear HDR target every scene pass should render into.
    pub fn hdr_view(&self) -> &wgpu::TextureView {
        self.hdr_view
            .as_ref()
            .expect("call PostProcessor::ensure_size before hdr_view")
    }

    /// Bloom + tonemap the HDR scene into `output`.
    pub fn run(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        settings: &PostSettings,
    ) {
        let params = PostParams {
            exposure: settings.exposure,
            bloom_strength: settings.bloom_strength,
            crt: settings.crt,
            srgb_encode: if self.srgb_encode { 1.0 } else { 0.0 },
            viewport: Vec2::new(self.size.0 as f32, self.size.1 as f32).to_array(),
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        for (i, view) in self.bloom_views.iter().enumerate() {
            self.fullscreen_pass(
                encoder,
                "bloom down",
                view,
                &self.down_pipeline,
                &self.down_binds[i],
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            );
        }
        for i in (0..self.bloom_views.len().saturating_sub(1)).rev() {
            self.fullscreen_pass(
                encoder,
                "bloom up",
                &self.bloom_views[i],
                &self.up_pipeline,
                &self.up_binds[i],
                wgpu::LoadOp::Load,
            );
        }
        self.fullscreen_pass(
            encoder,
            "composite",
            output,
            &self.composite_pipeline,
            self.composite_bind.as_ref().expect("sized"),
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        );
    }

    fn fullscreen_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        target: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        bind: &wgpu::BindGroup,
        load: wgpu::LoadOp<wgpu::Color>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
