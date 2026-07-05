use glam::{Mat4, Vec2, Vec3};

/// GPU camera block shared by all passes: clip transform, viewport in
/// pixels, stroke width in pixels, depth-cue fog, eye position, animation
/// time, and the master glow dial.
/// 112 bytes; layout mirrored in `shaders/camera.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    viewport: [f32; 2],
    line_width_px: f32,
    fog_density: f32,
    eye: [f32; 3],
    time: f32,
    glow: f32,
    _pad: [f32; 3],
}

impl CameraUniform {
    /// `fog_density` is the exponential depth-cue coefficient
    /// (`brightness *= exp(-fog_density × distance)`); 0.0 disables it.
    /// `time` (seconds) drives style animation (flicker).
    /// `glow` compresses HDR palette values (hue-preserving): the engine
    /// owns final brightness, assets only author relative strengths —
    /// 0 = no overbright at all, 1 = authored strengths verbatim.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        view_proj: Mat4,
        viewport: Vec2,
        line_width_px: f32,
        eye: Vec3,
        fog_density: f32,
        time: f32,
        glow: f32,
    ) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            viewport: viewport.to_array(),
            line_width_px,
            fog_density,
            eye: eye.to_array(),
            time,
            glow,
            _pad: [0.0; 3],
        }
    }
}

/// The camera uniform buffer plus its bind group — one per frame target,
/// shared by every pipeline (all pass shaders bind it at group 0).
pub struct CameraBinding {
    buffer: wgpu::Buffer,
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) bind_group: wgpu::BindGroup,
}

impl CameraBinding {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            buffer,
            layout,
            bind_group,
        }
    }

    /// Upload this frame's camera state. Call once per frame before passes.
    pub fn update(&self, queue: &wgpu::Queue, camera: &CameraUniform) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(camera));
    }
}

/// Compile a WGSL body with the shared camera block prepended.
pub(crate) fn shader_with_camera(
    device: &wgpu::Device,
    label: &str,
    body: &str,
) -> wgpu::ShaderModule {
    let source = format!("{}\n{body}", include_str!("shaders/camera.wgsl"));
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_uniform_layout_matches_wgsl() {
        // mat4 (64) + vec2 (8) + 2×f32 (8) + vec3 (12) + time (4)
        // + glow (4) + 3×f32 pad = 112, 16-aligned.
        assert_eq!(std::mem::size_of::<CameraUniform>(), 112);
    }
}
