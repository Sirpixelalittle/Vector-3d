//! M1 demo: hidden-line rendering. A solid spinning cube and an orbiting
//! icosahedron occlude each other (and the grid) while staying pure line art.
//!
//! Windowed:    cargo run -p cube
//! Headless:    cargo run -p cube -- --screenshot out.png [--size 1920x1080]
//!                  [--time 2.0] [--eye 3.4,2.4,4.8] [--look 0,0,0]

use std::path::Path;

use anyhow::{Context, Result};
use glam::{Mat4, Vec2, Vec3, Vec3Swizzles, Vec4, vec3};
use vex_core::{Segment, Shape, phosphor, shapes};
use vex_engine::{App, FlyCamera, Input};
use vex_render::{CameraBinding, CameraUniform, Gpu, HeadlessTarget, LineRenderer, OccluderRenderer};

const LINE_WIDTH_PX: f32 = 1.6;
const GRID_HALF_EXTENT: i32 = 10;
const GRID_Y: f32 = -1.0;
const GRID_BRIGHTNESS: f32 = 0.18;
/// Depth-cue coefficient: brightness × exp(-k·distance). At 6 m ≈ 0.74,
/// at 20 m ≈ 0.37 — far grid fades, hero shapes stay hot.
const FOG_DENSITY: f32 = 0.05;
const DEFAULT_EYE: Vec3 = vec3(3.4, 2.4, 4.8);

/// One frame's worth of world-space geometry, built on the CPU.
struct SceneGeometry {
    segments: Vec<Segment>,
    occluder_vertices: Vec<Vec3>,
    occluder_indices: Vec<u32>,
}

fn build_scene(time: f32) -> SceneGeometry {
    let mut scene = SceneGeometry {
        segments: grid(),
        occluder_vertices: Vec::new(),
        occluder_indices: Vec::new(),
    };

    let cube = shapes::cube(1.0).transformed(cube_transform(time));
    scene.segments.extend(cube_segments(&cube));
    cube.mesh
        .append_into(&mut scene.occluder_vertices, &mut scene.occluder_indices);

    let icosa = shapes::icosahedron(1.1).transformed(icosa_transform(time));
    scene.segments.extend(icosa.segments(phosphor::LIME));
    icosa
        .mesh
        .append_into(&mut scene.occluder_vertices, &mut scene.occluder_indices);

    scene
}

/// Gentle tumble in place.
fn cube_transform(time: f32) -> Mat4 {
    Mat4::from_rotation_y(time * 0.5) * Mat4::from_rotation_x(time * 0.21)
}

/// Orbit around the cube (radius 3.2) while tumbling — periodically passes
/// in front of and behind the cube, exercising occlusion both ways.
fn icosa_transform(time: f32) -> Mat4 {
    Mat4::from_rotation_y(time * 0.45)
        * Mat4::from_translation(vec3(3.2, 0.4, 0.0))
        * Mat4::from_rotation_y(time * -1.1)
        * Mat4::from_rotation_z(time * 0.7)
}

/// Cube styling relies on the edge order in `shapes::cube`:
/// bottom ring, top ring, pillars — four edges each.
fn cube_segments(cube: &Shape) -> Vec<Segment> {
    const RING_COLORS: [Vec4; 3] = [phosphor::GREEN, phosphor::CYAN, phosphor::MAGENTA];
    cube.edges
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| {
            Segment::new(
                cube.mesh.vertices[a as usize],
                cube.mesh.vertices[b as usize],
                RING_COLORS[i / 4],
            )
        })
        .collect()
}

/// Ground grid, dimmed so the shapes stay the heroes. Pure line art:
/// deliberately no occluder, so shapes sink below it visibly at y < -1.
fn grid() -> Vec<Segment> {
    let color = phosphor::dim(phosphor::GREEN, GRID_BRIGHTNESS);
    let extent = GRID_HALF_EXTENT as f32;
    (-GRID_HALF_EXTENT..=GRID_HALF_EXTENT)
        .flat_map(|i| {
            let t = i as f32;
            [
                Segment::new(vec3(-extent, GRID_Y, t), vec3(extent, GRID_Y, t), color),
                Segment::new(vec3(t, GRID_Y, -extent), vec3(t, GRID_Y, extent), color),
            ]
        })
        .collect()
}

struct Renderers {
    camera_binding: CameraBinding,
    lines: LineRenderer,
    occluders: OccluderRenderer,
}

impl Renderers {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let camera_binding = CameraBinding::new(device);
        let lines = LineRenderer::new(device, target_format, &camera_binding);
        let occluders = OccluderRenderer::new(device, &camera_binding);
        Self {
            camera_binding,
            lines,
            occluders,
        }
    }

    /// Upload this frame's state and record both passes.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: &SceneGeometry,
        camera: &CameraUniform,
    ) {
        self.camera_binding.update(&gpu.queue, camera);
        self.lines
            .set_segments(&gpu.device, &gpu.queue, &scene.segments);
        self.occluders.set_geometry(
            &gpu.device,
            &gpu.queue,
            &scene.occluder_vertices,
            &scene.occluder_indices,
        );
        self.occluders
            .render(encoder, depth, &self.camera_binding, true);
        self.lines
            .render(encoder, color, depth, &self.camera_binding, true, false);
    }
}

struct CubeDemo {
    camera: FlyCamera,
    time: f32,
    scene: SceneGeometry,
    renderers: Option<Renderers>,
}

impl CubeDemo {
    fn new() -> Self {
        // Face the origin: yaw from the horizontal offset, pitch from height.
        let yaw = DEFAULT_EYE.x.atan2(DEFAULT_EYE.z);
        let pitch = -DEFAULT_EYE.y.atan2(DEFAULT_EYE.xz().length());
        Self {
            camera: FlyCamera::new(DEFAULT_EYE, yaw, pitch),
            time: 0.0,
            scene: build_scene(0.0),
            renderers: None,
        }
    }
}

impl App for CubeDemo {
    fn init(&mut self, gpu: &Gpu, target_format: wgpu::TextureFormat) {
        self.renderers = Some(Renderers::new(&gpu.device, target_format));
    }

    fn update(&mut self, dt: f32, input: &Input) {
        self.time += dt;
        self.camera.update(dt, input);
        self.scene = build_scene(self.time);
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
        let camera = CameraUniform::new(
            self.camera.view_proj(viewport.x / viewport.y),
            viewport,
            LINE_WIDTH_PX,
            self.camera.pos,
            FOG_DENSITY,
            self.time,
            1.0,
        );
        renderers.draw(gpu, encoder, color, depth, &self.scene, &camera);
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(path) = flag_value(&args, "--screenshot") {
        let options = ScreenshotOptions::parse(&args)?;
        return screenshot(Path::new(&path), &options);
    }

    println!(
        "controls: left-click captures the mouse · WASD move · Space/LCtrl up/down · \
         LShift sprint · Esc releases · close window to quit"
    );
    vex_engine::run("vector3d — 01 cube", CubeDemo::new())
}

struct ScreenshotOptions {
    size: (u32, u32),
    time: f32,
    eye: Vec3,
    look: Vec3,
}

impl ScreenshotOptions {
    fn parse(args: &[String]) -> Result<Self> {
        Ok(Self {
            size: flag_value(args, "--size")
                .map(parse_size)
                .transpose()?
                .unwrap_or((1280, 720)),
            time: flag_value(args, "--time")
                .map(|t| t.parse().context("--time expects seconds"))
                .transpose()?
                .unwrap_or(2.0),
            eye: flag_value(args, "--eye")
                .map(parse_vec3)
                .transpose()?
                .unwrap_or(DEFAULT_EYE),
            look: flag_value(args, "--look")
                .map(parse_vec3)
                .transpose()?
                .unwrap_or(Vec3::ZERO),
        })
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn parse_size(value: String) -> Result<(u32, u32)> {
    let (w, h) = value
        .split_once('x')
        .context("--size expects WxH, e.g. 1920x1080")?;
    Ok((w.parse()?, h.parse()?))
}

fn parse_vec3(value: String) -> Result<Vec3> {
    let parts: Vec<f32> = value
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .context("expected three comma-separated numbers, e.g. 3.4,2.4,4.8")?;
    anyhow::ensure!(parts.len() == 3, "expected exactly three components");
    Ok(vec3(parts[0], parts[1], parts[2]))
}

/// Render one frame offscreen at a fixed animation time and camera pose —
/// the headless verification path (works without a display).
fn screenshot(path: &Path, options: &ScreenshotOptions) -> Result<()> {
    let (width, height) = options.size;
    let gpu = Gpu::headless()?;
    let target = HeadlessTarget::new(&gpu.device, width, height);
    let mut renderers = Renderers::new(&gpu.device, vex_render::HEADLESS_FORMAT);

    let scene = build_scene(options.time);
    let view = glam::camera::rh::view::look_at_mat4(options.eye, options.look, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(
        60f32.to_radians(),
        width as f32 / height as f32,
        0.05,
        500.0,
    );
    let viewport = Vec2::new(width as f32, height as f32);
    let camera = CameraUniform::new(
        proj * view,
        viewport,
        LINE_WIDTH_PX,
        options.eye,
        FOG_DENSITY,
        options.time,
        1.0,
    );

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    renderers.draw(
        &gpu,
        &mut encoder,
        &target.color_view,
        &target.depth_view,
        &scene,
        &camera,
    );
    gpu.queue.submit([encoder.finish()]);

    target.save_png(&gpu, path)?;
    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vec3_accepts_commas_and_spaces() {
        assert_eq!(
            parse_vec3("1, -2.5,3".into()).unwrap(),
            vec3(1.0, -2.5, 3.0)
        );
        assert!(parse_vec3("1,2".into()).is_err());
        assert!(parse_vec3("a,b,c".into()).is_err());
    }

    #[test]
    fn scene_pairs_every_occluder_triangle_with_edges() {
        let scene = build_scene(1.0);
        // Cube (12 tris) + icosahedron (20 tris).
        assert_eq!(scene.occluder_indices.len() / 3, 32);
        // Grid (2×21) + cube edges (12) + icosa edges (30).
        assert_eq!(scene.segments.len(), 42 + 12 + 30);
    }
}
