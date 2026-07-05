use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};

use crate::Gpu;

pub const HEADLESS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Offscreen render target (color + depth) for windowless rendering:
/// screenshots, golden-image tests, tooling.
pub struct HeadlessTarget {
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl HeadlessTarget {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("headless color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HEADLESS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = crate::create_depth_view(device, width, height);
        Self {
            color,
            color_view,
            depth_view,
            width,
            height,
        }
    }

    /// Read back the color target and write it as a PNG.
    pub fn save_png(&self, gpu: &Gpu, path: &Path) -> Result<()> {
        let bytes_per_row = self.width * 4;
        let padded_bytes_per_row =
            bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("device poll failed")?;
        rx.recv()
            .context("map_async callback dropped")?
            .context("readback buffer map failed")?;

        let data = slice.get_mapped_range().context("get mapped range")?;
        let mut pixels = Vec::with_capacity((bytes_per_row * self.height) as usize);
        for row in data.chunks(padded_bytes_per_row as usize) {
            pixels.extend_from_slice(&row[..bytes_per_row as usize]);
        }
        drop(data);
        readback.unmap();

        let file =
            File::create(path).with_context(|| format!("create {}", path.display()))?;
        let mut png_encoder = png::Encoder::new(BufWriter::new(file), self.width, self.height);
        png_encoder.set_color(png::ColorType::Rgba);
        png_encoder.set_depth(png::BitDepth::Eight);
        let mut writer = png_encoder.write_header().context("write png header")?;
        writer
            .write_image_data(&pixels)
            .context("write png data")?;
        Ok(())
    }
}
