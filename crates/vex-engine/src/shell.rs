use std::sync::Arc;

use anyhow::{Context, Result};
// std::time::Instant on native; performance.now() in the browser.
use web_time::Instant;
use glam::Vec2;
use vex_render::Gpu;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::Input;

const WINDOW_SIZE: (u32, u32) = (1280, 720);
/// Clamp pathological frame gaps (first frame, suspend) to keep dt sane.
const MAX_DT: f32 = 0.1;
const TITLE_REFRESH_SECONDS: f32 = 0.5;

/// A vector3d application: create GPU resources in `init`, advance the
/// simulation in `update`, record draw calls in `render`.
pub trait App {
    fn init(&mut self, gpu: &Gpu, target_format: wgpu::TextureFormat);
    fn update(&mut self, dt: f32, input: &Input);
    fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        viewport: Vec2,
    );

    /// Should a left click grab the mouse for look? Menus return false so
    /// clicks reach the app as ordinary pointer input instead.
    fn wants_capture(&self) -> bool {
        true
    }

    /// Polled once per frame after `update`; return true to close the app
    /// (a menu's Quit button). No effect on the web — there's no window to
    /// close, so don't offer the button there.
    fn should_quit(&self) -> bool {
        false
    }
}

/// Open a window and run the app until close. Left click captures the mouse
/// for look; Escape releases it. The title shows a live fps readout.
///
/// On the web this appends a canvas to the document body and returns
/// immediately (the event loop lives on in the browser); GPU setup happens
/// asynchronously since wasm cannot block on the adapter request.
pub fn run(title: &str, app: impl App + 'static) -> Result<()> {
    let event_loop = EventLoop::new().context("create event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let shell = Shell {
        title: title.to_owned(),
        app,
        gfx: None,
        init_started: false,
        #[cfg(target_arch = "wasm32")]
        pending_gfx: Default::default(),
        input: Input::default(),
        last_frame: None,
        fps_accum: 0.0,
        fps_frames: 0,
        #[cfg(target_arch = "wasm32")]
        stray_cursor_moves: 0,
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut shell = shell;
        event_loop.run_app(&mut shell).context("event loop")?;
    }
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(shell);
    }
    Ok(())
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    depth_view: wgpu::TextureView,
}

/// Async-init handoff slot: the spawned future drops the result here and
/// `about_to_wait` picks it up (wasm is single-threaded; Rc is fine).
#[cfg(target_arch = "wasm32")]
type PendingGfx = std::rc::Rc<std::cell::RefCell<Option<Result<Gfx>>>>;

struct Shell<A: App> {
    title: String,
    app: A,
    gfx: Option<Gfx>,
    init_started: bool,
    #[cfg(target_arch = "wasm32")]
    pending_gfx: PendingGfx,
    input: Input,
    last_frame: Option<Instant>,
    fps_accum: f32,
    fps_frames: u32,
    /// Web only: consecutive cursor *position* changes seen while we
    /// believe the pointer is locked. A locked pointer reports a frozen
    /// position, so movement here means the browser revoked the lock
    /// behind our back (its Esc handling eats the keydown) — after a
    /// short debounce we release our side so the next click re-grabs.
    #[cfg(target_arch = "wasm32")]
    stray_cursor_moves: u8,
}

/// Everything after window creation: surface, adapter, device, swapchain.
/// Async because the browser cannot block on the adapter request.
async fn init_gfx(window: Arc<Window>) -> Result<Gfx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let surface = instance
        .create_surface(window.clone())
        .context("create surface")?;
    let gpu = Gpu::new(instance, Some(&surface)).await?;

    let size = window.inner_size();
    let mut config = surface
        .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
        .context("surface not supported by adapter")?;
    // Prefer an sRGB swapchain format so shader output stays linear.
    let caps = surface.get_capabilities(&gpu.adapter);
    if let Some(srgb) = caps.formats.iter().copied().find(wgpu::TextureFormat::is_srgb) {
        config.format = srgb;
    }
    surface.configure(&gpu.device, &config);
    let depth_view = vex_render::create_depth_view(&gpu.device, config.width, config.height);

    Ok(Gfx {
        window,
        surface,
        config,
        gpu,
        depth_view,
    })
}

impl<A: App> Shell<A> {
    fn create_window(&self, event_loop: &ActiveEventLoop) -> Result<Arc<Window>> {
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::PhysicalSize::new(WINDOW_SIZE.0, WINDOW_SIZE.1));
        #[cfg(target_arch = "wasm32")]
        {
            // Drop the canvas straight into <body>; index.html styles it.
            use winit::platform::web::WindowAttributesExtWebSys;
            attrs = attrs.with_append(true);
        }
        Ok(Arc::new(
            event_loop.create_window(attrs).context("create window")?,
        ))
    }

    fn finish_init(&mut self, gfx: Gfx) {
        self.app.init(&gfx.gpu, gfx.config.format);
        gfx.window.request_redraw();
        self.gfx = Some(gfx);
    }

    fn resize(&mut self, width: u32, height: u32) {
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        gfx.config.width = width;
        gfx.config.height = height;
        gfx.surface.configure(&gfx.gpu.device, &gfx.config);
        gfx.depth_view = vex_render::create_depth_view(&gfx.gpu.device, width, height);
    }

    fn set_capture(&mut self, captured: bool) {
        let Some(gfx) = self.gfx.as_ref() else {
            return;
        };
        let grab = if captured {
            // Wayland supports Locked; X11 typically only Confined.
            gfx.window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| gfx.window.set_cursor_grab(CursorGrabMode::Confined))
        } else {
            gfx.window.set_cursor_grab(CursorGrabMode::None)
        };
        if let Err(err) = grab {
            log::warn!("cursor grab: {err}");
            return;
        }
        gfx.window.set_cursor_visible(!captured);
        self.input.captured = captured;
        #[cfg(target_arch = "wasm32")]
        {
            self.stray_cursor_moves = 0;
        }
    }

    fn update_fps_title(&mut self, dt: f32) {
        self.fps_accum += dt;
        self.fps_frames += 1;
        if self.fps_accum < TITLE_REFRESH_SECONDS {
            return;
        }
        let fps = self.fps_frames as f32 / self.fps_accum;
        if let Some(gfx) = self.gfx.as_ref() {
            gfx.window
                .set_title(&format!("{} — {fps:.0} fps", self.title));
        }
        self.fps_accum = 0.0;
        self.fps_frames = 0;
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(0.0, |last| (now - last).as_secs_f32())
            .min(MAX_DT);
        self.last_frame = Some(now);

        self.app.update(dt, &self.input);
        self.input.end_frame();
        self.update_fps_title(dt);

        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match gfx.surface.get_current_texture() {
            Cst::Success(frame) | Cst::Suboptimal(frame) => frame,
            Cst::Outdated | Cst::Lost => {
                gfx.surface.configure(&gfx.gpu.device, &gfx.config);
                return;
            }
            Cst::Timeout | Cst::Occluded => return,
            Cst::Validation => {
                log::error!("surface validation error; skipping frame");
                return;
            }
        };

        let color = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gfx
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let viewport = Vec2::new(gfx.config.width as f32, gfx.config.height as f32);
        self.app
            .render(&gfx.gpu, &mut encoder, &color, &gfx.depth_view, viewport);
        gfx.gpu.queue.submit([encoder.finish()]);
        gfx.window.pre_present_notify();
        gfx.gpu.queue.present(frame);
    }
}

impl<A: App> ApplicationHandler for Shell<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.init_started {
            return;
        }
        self.init_started = true;
        let window = match self.create_window(event_loop) {
            Ok(window) => window,
            Err(err) => {
                log::error!("window creation failed: {err:#}");
                event_loop.exit();
                return;
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        match pollster::block_on(init_gfx(window)) {
            Ok(gfx) => self.finish_init(gfx),
            Err(err) => {
                log::error!("graphics init failed: {err:#}");
                event_loop.exit();
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let slot = self.pending_gfx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                *slot.borrow_mut() = Some(init_gfx(window).await);
            });
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::Focused(false) => self.set_capture(false),
            WindowEvent::CursorMoved { position, .. } => {
                let position = Vec2::new(position.x as f32, position.y as f32);
                // Web: the browser can revoke pointer lock behind our back
                // (its own Esc handling exits the lock and eats the
                // keydown). We'd then think we're capturing while a free,
                // invisible cursor drifts — and mouse-look dies in one
                // direction the moment it rests against the window edge.
                // A locked pointer reports a frozen position, so movement
                // while "captured" proves the lock is gone: release after
                // a short debounce (the grab request is async on the web,
                // so the first events after a capturing click are noise)
                // and let the next click re-grab. Native grabs can't be
                // silently revoked.
                #[cfg(target_arch = "wasm32")]
                if self.input.captured && position != self.input.cursor {
                    self.stray_cursor_moves += 1;
                    if self.stray_cursor_moves >= 3 {
                        log::info!("pointer lock revoked by the browser; releasing capture");
                        self.set_capture(false);
                    }
                }
                self.input.cursor = position;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // The capturing click is consumed; everything after that
                // is game input (attacks etc.). Apps that are showing a
                // menu decline the grab and get the click as input.
                if button == MouseButton::Left
                    && state == ElementState::Pressed
                    && !self.input.captured
                    && self.app.wants_capture()
                {
                    self.set_capture(true);
                } else {
                    self.input.set_mouse_button(button, state.is_pressed());
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape && event.state.is_pressed() {
                        self.set_capture(false);
                    }
                    self.input.set_key(code, event.state.is_pressed());
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 40.0,
                };
                self.input.add_scroll(lines);
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                if self.app.should_quit() {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.input.captured
        {
            self.input.mouse_delta += Vec2::new(delta.0 as f32, delta.1 as f32);
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        // Web: pick up the asynchronously-created GPU context when ready.
        // (Take in its own statement — the RefMut temporary must drop
        // before finish_init borrows self mutably.)
        #[cfg(target_arch = "wasm32")]
        if self.gfx.is_none() {
            let pending = self.pending_gfx.borrow_mut().take();
            match pending {
                Some(Ok(gfx)) => self.finish_init(gfx),
                Some(Err(err)) => log::error!(
                    "graphics init failed (browser has WebGPU?): {err:#}"
                ),
                None => {}
            }
        }
        // Continuous redraw: this is a real-time renderer, not a GUI app.
        if let Some(gfx) = self.gfx.as_ref() {
            gfx.window.request_redraw();
        }
    }
}
