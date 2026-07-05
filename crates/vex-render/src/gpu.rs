use anyhow::{Context, Result};

/// Owned GPU handles shared by every renderer.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// Create from an existing instance, optionally requiring compatibility
    /// with a surface (windowed use).
    pub async fn new(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("no suitable GPU adapter")?;
        let info = adapter.get_info();
        log::info!("adapter: {} ({:?})", info.name, info.backend);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("vex device"),
                ..Default::default()
            })
            .await
            .context("failed to create GPU device")?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    /// Windowless context for offscreen rendering and tooling.
    pub fn headless() -> Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        pollster::block_on(Self::new(instance, None))
    }
}
