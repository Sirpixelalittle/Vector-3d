//! Model viewer: drop in a `.vec` — or a `.gltf`/`.glb`, converted
//! in-process — orbit around it, and hot-reload on file change (save from
//! Blender, watch it refresh).
//!
//! Windowed:    cargo run -p viewer -- assets/suzanne/Suzanne.gltf
//! Headless:    cargo run -p viewer -- model.vec --screenshot out.png
//!                  [--size WxH] [--smooth] [--yaw R] [--pitch R] [--zoom F]
//!
//! Keys: left-click + mouse orbits (Esc releases), scroll zooms,
//!       [S] toggles smooth silhouette-candidate edges, [R] reframes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use glam::{Vec2, Vec4, vec3};
use vex_convert::ConvertOptions;
use vex_core::{EdgeKind, Segment, VecModel, phosphor};
use vex_engine::{App, Input, KeyCode, OrbitCamera};
use vex_render::{
    CameraBinding, CameraUniform, Gpu, HDR_FORMAT, HeadlessTarget, LineRenderer,
    OccluderRenderer, PostProcessor, PostSettings,
};

const LINE_WIDTH_PX: f32 = 1.6;
const FOG_DENSITY: f32 = 0.02;
/// Smooth silhouette candidates, shown dim blue by the [S] debug view.
const SMOOTH_DEBUG_INTENSITY: f32 = 0.16;
const RELOAD_POLL_SECONDS: f32 = 0.5;

fn load_model(path: &Path) -> Result<VecModel> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("vec") => VecModel::load(path).with_context(|| format!("load {}", path.display())),
        Some("gltf" | "glb") => {
            let (model, stats) = vex_convert::convert_gltf(path, &ConvertOptions::default())?;
            println!(
                "converted {}: {} drawn edges ({} boundary · {} crease · {} material · {} decor), {} smooth candidates",
                path.display(),
                stats.always_edges(),
                stats.boundary,
                stats.crease,
                stats.material,
                stats.decor,
                stats.smooth,
            );
            Ok(model)
        }
        _ => bail!("expected a .vec, .gltf or .glb file"),
    }
}

/// World-space segments for the model + a context grid under it.
fn build_segments(model: &VecModel, show_smooth: bool) -> Vec<Segment> {
    let mut segments = grid_under(model);
    segments.extend(model.edge_segments(EdgeKind::Always, 1.0));
    if show_smooth {
        let blue = Vec4::new(
            phosphor::BLUE.x,
            phosphor::BLUE.y,
            phosphor::BLUE.z,
            SMOOTH_DEBUG_INTENSITY,
        );
        segments.extend(
            model
                .edge_segments(EdgeKind::Smooth, 1.0)
                .into_iter()
                .map(|s| Segment::new(s.a, s.b, blue)),
        );
    }
    segments
}

fn grid_under(model: &VecModel) -> Vec<Segment> {
    let size = model.aabb_max - model.aabb_min;
    let center = (model.aabb_min + model.aabb_max) * 0.5;
    let extent = (size.x.max(size.z) * 1.2).max(0.5);
    let y = model.aabb_min.y - size.y * 0.02;
    let color = phosphor::dim(phosphor::GREEN, 0.10);
    const CELLS: i32 = 12;
    (-CELLS / 2..=CELLS / 2)
        .flat_map(|i| {
            let t = i as f32 / (CELLS / 2) as f32 * extent;
            [
                Segment::new(
                    vec3(center.x - extent, y, center.z + t),
                    vec3(center.x + extent, y, center.z + t),
                    color,
                ),
                Segment::new(
                    vec3(center.x + t, y, center.z - extent),
                    vec3(center.x + t, y, center.z + extent),
                    color,
                ),
            ]
        })
        .collect()
}

struct Renderers {
    camera_binding: CameraBinding,
    lines: LineRenderer,
    /// View-dependent silhouettes, re-uploaded every frame.
    silhouette_lines: LineRenderer,
    occluders: OccluderRenderer,
    post: PostProcessor,
}

impl Renderers {
    fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let camera_binding = CameraBinding::new(device);
        // Scene passes render linear HDR; `post` owns the output format.
        let lines = LineRenderer::new(device, HDR_FORMAT, &camera_binding);
        let silhouette_lines = LineRenderer::new(device, HDR_FORMAT, &camera_binding);
        let occluders = OccluderRenderer::new(device, &camera_binding);
        let post = PostProcessor::new(device, output_format);
        Self {
            camera_binding,
            lines,
            silhouette_lines,
            occluders,
            post,
        }
    }

    fn upload(&mut self, gpu: &Gpu, model: &VecModel, segments: &[Segment]) {
        self.lines.set_segments(&gpu.device, &gpu.queue, segments);
        self.occluders.set_geometry(
            &gpu.device,
            &gpu.queue,
            &model.vertices,
            &model.occluder_indices,
        );
    }

    fn set_silhouettes(&mut self, gpu: &Gpu, segments: &[Segment]) {
        self.silhouette_lines
            .set_segments(&gpu.device, &gpu.queue, segments);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        gpu: &Gpu,
        camera: &CameraUniform,
        viewport: Vec2,
        settings: &PostSettings,
    ) {
        self.post
            .ensure_size(&gpu.device, viewport.x as u32, viewport.y as u32);
        self.camera_binding.update(&gpu.queue, camera);
        let hdr = self.post.hdr_view();
        self.occluders
            .render(encoder, depth, &self.camera_binding, true);
        self.lines
            .render(encoder, hdr, depth, &self.camera_binding, true, false);
        self.silhouette_lines
            .render(encoder, hdr, depth, &self.camera_binding, false, false);
        self.post.run(&gpu.queue, encoder, output, settings);
    }
}

struct ViewerApp {
    source: PathBuf,
    model: VecModel,
    camera: OrbitCamera,
    show_smooth: bool,
    renderers: Option<Renderers>,
    needs_upload: bool,
    mtime: Option<SystemTime>,
    poll_accum: f32,
    time: f32,
}

impl ViewerApp {
    fn new(source: PathBuf) -> Result<Self> {
        let model = load_model(&source)?;
        let camera = OrbitCamera::framing(model.aabb_min, model.aabb_max);
        let mtime = file_mtime(&source);
        Ok(Self {
            source,
            model,
            camera,
            show_smooth: false,
            renderers: None,
            needs_upload: true,
            mtime,
            poll_accum: 0.0,
            time: 0.0,
        })
    }

    fn poll_reload(&mut self, dt: f32) {
        self.poll_accum += dt;
        if self.poll_accum < RELOAD_POLL_SECONDS {
            return;
        }
        self.poll_accum = 0.0;
        let current = file_mtime(&self.source);
        if current == self.mtime {
            return;
        }
        self.mtime = current;
        // A half-written export just fails to parse; we keep the old model
        // and the next poll retries.
        match load_model(&self.source) {
            Ok(model) => {
                self.camera = OrbitCamera::framing(model.aabb_min, model.aabb_max);
                self.model = model;
                self.needs_upload = true;
                println!("reloaded {}", self.source.display());
            }
            Err(err) => println!("reload failed (will retry): {err:#}"),
        }
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl App for ViewerApp {
    fn init(&mut self, gpu: &Gpu, target_format: wgpu::TextureFormat) {
        self.renderers = Some(Renderers::new(&gpu.device, target_format));
    }

    fn update(&mut self, dt: f32, input: &Input) {
        self.time += dt;
        self.camera.update(input);
        if input.is_just_pressed(KeyCode::KeyS) {
            self.show_smooth = !self.show_smooth;
            self.needs_upload = true;
        }
        if input.is_just_pressed(KeyCode::KeyR) {
            self.camera = OrbitCamera::framing(self.model.aabb_min, self.model.aabb_max);
        }
        self.poll_reload(dt);
    }

    fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        viewport: Vec2,
    ) {
        let Some(renderers) = self.renderers.as_mut() else {
            return;
        };
        if self.needs_upload {
            let segments = build_segments(&self.model, self.show_smooth);
            renderers.upload(gpu, &self.model, &segments);
            self.needs_upload = false;
        }
        // Silhouettes depend on the eye — refreshed every frame.
        let silhouettes =
            self.model
                .silhouette_segments(glam::Mat4::IDENTITY, self.camera.eye(), 1.0);
        renderers.set_silhouettes(gpu, &silhouettes);
        let settings = PostSettings::default();
        let camera = CameraUniform::new(
            self.camera.view_proj(viewport.x / viewport.y),
            viewport,
            LINE_WIDTH_PX,
            self.camera.eye(),
            FOG_DENSITY,
            self.time,
            settings.glow,
        );
        renderers.draw(encoder, color, depth, gpu, &camera, viewport, &settings);
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(source) = args.iter().find(|a| !a.starts_with("--")).cloned() else {
        bail!("usage: viewer <model.vec|model.gltf|model.glb> [--screenshot out.png]");
    };
    let source = PathBuf::from(source);

    if let Some(shot) = flag_value(&args, "--screenshot") {
        return screenshot(&source, Path::new(&shot), &args);
    }

    println!(
        "controls: left-click orbits (Esc releases) · scroll zooms · \
         [S] smooth candidates · [R] reframe · edit+save the file to hot-reload"
    );
    let title = format!(
        "vector3d — viewer — {}",
        source.file_name().unwrap_or_default().to_string_lossy()
    );
    vex_engine::run(&title, ViewerApp::new(source)?)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn parse_flag<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match flag_value(args, flag) {
        Some(raw) => raw
            .parse()
            .with_context(|| format!("{flag} expects a number")),
        None => Ok(default),
    }
}

/// Render one frame offscreen with the auto-framing orbit camera.
fn screenshot(source: &Path, out: &Path, args: &[String]) -> Result<()> {
    let (width, height) = match flag_value(args, "--size") {
        Some(raw) => {
            let (w, h) = raw.split_once('x').context("--size expects WxH")?;
            (w.parse()?, h.parse()?)
        }
        None => (1280, 720),
    };
    let show_smooth = args.iter().any(|a| a == "--smooth");

    let model = load_model(source)?;
    let mut camera = OrbitCamera::framing(model.aabb_min, model.aabb_max);
    camera.yaw = parse_flag(args, "--yaw", camera.yaw)?;
    camera.pitch = parse_flag(args, "--pitch", camera.pitch)?;
    camera.distance *= parse_flag(args, "--zoom", 1.0f32)?;

    let gpu = Gpu::headless()?;
    let target = HeadlessTarget::new(&gpu.device, width, height);
    let mut renderers = Renderers::new(&gpu.device, vex_render::HEADLESS_FORMAT);
    renderers.upload(&gpu, &model, &build_segments(&model, show_smooth));
    let silhouettes = model.silhouette_segments(glam::Mat4::IDENTITY, camera.eye(), 1.0);
    renderers.set_silhouettes(&gpu, &silhouettes);

    let settings = PostSettings::default();
    let viewport = Vec2::new(width as f32, height as f32);
    let uniform = CameraUniform::new(
        camera.view_proj(viewport.x / viewport.y),
        viewport,
        LINE_WIDTH_PX,
        camera.eye(),
        FOG_DENSITY,
        0.0,
        settings.glow,
    );
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    renderers.draw(
        &mut encoder,
        &target.color_view,
        &target.depth_view,
        &gpu,
        &uniform,
        viewport,
        &settings,
    );
    gpu.queue.submit([encoder.finish()]);
    target.save_png(&gpu, out)?;
    println!("wrote {}", out.display());
    Ok(())
}
