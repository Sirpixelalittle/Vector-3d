//! wgpu renderer for the vector3d engine.
//!
//! Frame recipe (M1): update the [`CameraBinding`], record the depth-only
//! [`OccluderRenderer`] pass (clears depth), then the [`LineRenderer`] pass
//! (clears color, loads depth) — the depth test does hidden-line removal.

mod camera;
mod gpu;
mod headless;
mod line;
mod occluder;
mod post;

pub use camera::{CameraBinding, CameraUniform};
pub use gpu::Gpu;
pub use headless::{HEADLESS_FORMAT, HeadlessTarget};
pub use line::LineRenderer;
pub use occluder::OccluderRenderer;
pub use post::{HDR_FORMAT, PostProcessor, PostSettings};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Create a depth attachment view sized to the render target.
pub fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vex depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
