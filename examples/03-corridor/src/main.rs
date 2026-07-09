//! M3 demo: walk the corridor. RON scene, frustum-culled hidden-line world,
//! capsule collision (the occluder mesh *is* the collision mesh), and a
//! first-person weapon layer with the converted Blender sword.
//!
//! Windowed:    cargo run -p corridor
//! Headless:    cargo run -p corridor -- --screenshot out.png [--size WxH]
//!                  [--pos x,y,z] [--yaw DEG] [--pitch DEG]
//!
//! Controls: click to capture · WASD walk · LShift sprint · Space jump.

use std::path::Path;

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec2, Vec3, Vec4, vec3};
use vex_core::{EdgeKind, Frustum, Segment, VecModel, font, phosphor};
use vex_engine::{App, BakedScene, FpsController, Input, KeyCode, TriangleSoup};
use vex_render::{
    CameraBinding, CameraUniform, Gpu, HDR_FORMAT, HeadlessTarget, LineRenderer,
    OccluderRenderer, PostProcessor, PostSettings,
};

const SCENE_PATH: &str = "assets/corridor/scene.ron";
const LINE_WIDTH_PX: f32 = 1.6;
const COLLISION_CELL: f32 = 1.5;
const WEAPON_FOV_DEG: f32 = 55.0;
/// Weapon length in view units after auto-fit.
const WEAPON_LENGTH: f32 = 0.78;
const HUD_LINE_WIDTH_PX: f32 = 2.0;

/// The sword in view space: auto-fit transform + resting pose.
struct Weapon {
    model: VecModel,
    fit: Mat4,
}

impl Weapon {
    fn new(model: VecModel) -> Self {
        // Normalize whatever scale/centering the artist used: longest axis
        // becomes WEAPON_LENGTH, centered on the origin.
        let extent = (model.aabb_max - model.aabb_min).max_element().max(1e-4);
        let center = (model.aabb_min + model.aabb_max) * 0.5;
        let fit = Mat4::from_scale(Vec3::splat(WEAPON_LENGTH / extent))
            * Mat4::from_translation(-center);
        Self { model, fit }
    }

    /// View-space placement: gripped low right, blade sweeping up-forward
    /// into the scene (the model's −Z end is the tip), plus walk bob.
    fn placement(&self, bob_phase: f32) -> Mat4 {
        let bob = vec3(
            (bob_phase * 0.5).cos() * 0.010,
            (bob_phase).sin() * 0.016 - 0.02,
            0.0,
        );
        Mat4::from_translation(vec3(0.31, -0.08, -0.66) + bob)
            * Mat4::from_quat(
                Quat::from_rotation_z(-0.22)
                    * Quat::from_rotation_y(0.10)
                    * Quat::from_rotation_x(0.78),
            )
            * self.fit
    }

    /// Weapon-layer geometry for this frame: always-drawn edges plus the
    /// blade's view-dependent silhouettes (the weapon camera sits at the
    /// view-space origin).
    fn frame_geometry(&self, bob_phase: f32) -> (Vec<Segment>, Vec<Vec3>, Vec<u32>) {
        let placement = self.placement(bob_phase);
        let mut segments: Vec<Segment> = self
            .model
            .edge_segments(EdgeKind::Always, 1.0)
            .into_iter()
            .map(|s| Segment {
                a: placement.transform_point3(s.a),
                b: placement.transform_point3(s.b),
                ..s
            })
            .collect();
        segments.extend(
            self.model
                .silhouette_segments(placement, Vec3::ZERO, 1.0),
        );
        let vertices: Vec<Vec3> = self
            .model
            .vertices
            .iter()
            .map(|&v| placement.transform_point3(v))
            .collect();
        (segments, vertices, self.model.occluder_indices.clone())
    }
}

/// HUD strokes in pixel space: health readout plus a crosshair, drawn by
/// the same line renderer as everything else.
fn hud_segments(viewport: Vec2) -> Vec<Segment> {
    let red = Vec4::new(phosphor::RED.x, phosphor::RED.y, phosphor::RED.z, 0.95);
    let mut out = font::text_segments("HEALTH 100", glam::Vec2::new(28.0, 26.0), 20.0, red);
    let center = viewport * 0.5;
    let (gap, len) = (5.0, 9.0);
    let dim_red = Vec4::new(phosphor::RED.x, phosphor::RED.y, phosphor::RED.z, 0.8);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        out.push(Segment::new(
            vec3(center.x + dx * gap, center.y + dy * gap, 0.0),
            vec3(
                center.x + dx * (gap + len),
                center.y + dy * (gap + len),
                0.0,
            ),
            dim_red,
        ));
    }
    out
}

struct Renderers {
    world_camera: CameraBinding,
    weapon_camera: CameraBinding,
    hud_camera: CameraBinding,
    world_lines: LineRenderer,
    /// Per-frame world silhouettes (view-dependent).
    world_silhouettes: LineRenderer,
    world_occluders: OccluderRenderer,
    weapon_lines: LineRenderer,
    weapon_occluders: OccluderRenderer,
    hud_lines: LineRenderer,
    post: PostProcessor,
}

impl Renderers {
    fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let world_camera = CameraBinding::new(device);
        let weapon_camera = CameraBinding::new(device);
        let hud_camera = CameraBinding::new(device);
        // Every stroke pass renders linear HDR; the post chain owns the
        // swapchain format. The HUD draws pre-post on purpose: it glows.
        Self {
            world_lines: LineRenderer::new(device, HDR_FORMAT, &world_camera),
            world_silhouettes: LineRenderer::new(device, HDR_FORMAT, &world_camera),
            world_occluders: OccluderRenderer::new(device, &world_camera),
            weapon_lines: LineRenderer::new(device, HDR_FORMAT, &weapon_camera),
            weapon_occluders: OccluderRenderer::new(device, &weapon_camera),
            hud_lines: LineRenderer::new(device, HDR_FORMAT, &hud_camera),
            post: PostProcessor::new(device, output_format),
            world_camera,
            weapon_camera,
            hud_camera,
        }
    }
}

struct Frame<'a> {
    gpu: &'a Gpu,
    color: &'a wgpu::TextureView,
    depth: &'a wgpu::TextureView,
    viewport: Vec2,
}

struct CorridorApp {
    scene: BakedScene,
    soup: TriangleSoup,
    player: FpsController,
    weapon: Option<Weapon>,
    renderers: Option<Renderers>,
    world_uploaded: bool,
    time: f32,
    post_settings: PostSettings,
}

impl CorridorApp {
    fn new() -> Result<Self> {
        let scene = vex_engine::load_scene(Path::new(SCENE_PATH))
            .with_context(|| format!("load scene {SCENE_PATH}"))?;
        let soup = TriangleSoup::new(
            &scene.occluder_vertices,
            &scene.occluder_indices,
            COLLISION_CELL,
        );
        println!(
            "scene: {} instances · {} segments · {} collision triangles",
            scene.instances.len(),
            scene.segments.len(),
            soup.triangle_count(),
        );
        let player = FpsController::new(scene.player_spawn, scene.player_yaw);
        let weapon = scene.weapon.clone().map(Weapon::new);
        Ok(Self {
            post_settings: scene.post,
            scene,
            soup,
            player,
            weapon,
            renderers: None,
            world_uploaded: false,
            time: 0.0,
        })
    }

    /// Live look tuning: [ ] glow · - = bloom · 9 0 exposure.
    fn tune_post(&mut self, input: &Input) {
        let p = &mut self.post_settings;
        let before = (p.glow, p.bloom_strength, p.exposure);
        if input.is_just_pressed(KeyCode::BracketLeft) {
            p.glow = (p.glow - 0.1).max(0.0);
        }
        if input.is_just_pressed(KeyCode::BracketRight) {
            p.glow = (p.glow + 0.1).min(2.0);
        }
        if input.is_just_pressed(KeyCode::Minus) {
            p.bloom_strength = (p.bloom_strength - 0.03).max(0.0);
        }
        if input.is_just_pressed(KeyCode::Equal) {
            p.bloom_strength = (p.bloom_strength + 0.03).min(0.8);
        }
        if input.is_just_pressed(KeyCode::Digit9) {
            p.exposure = (p.exposure - 0.1).max(0.3);
        }
        if input.is_just_pressed(KeyCode::Digit0) {
            p.exposure = (p.exposure + 0.1).min(2.5);
        }
        if before != (p.glow, p.bloom_strength, p.exposure) {
            println!(
                "post: glow {:.2} · bloom {:.2} · exposure {:.2}",
                p.glow, p.bloom_strength, p.exposure
            );
        }
    }

    /// Record the whole frame: culled world passes, then the weapon layer
    /// on a cleared depth buffer (the weapon never clips into walls).
    fn draw(&mut self, frame: &Frame) {
        let Some(renderers) = self.renderers.as_mut() else {
            return;
        };
        let aspect = frame.viewport.x / frame.viewport.y;
        renderers.post.ensure_size(
            &frame.gpu.device,
            frame.viewport.x as u32,
            frame.viewport.y as u32,
        );

        if !self.world_uploaded {
            renderers.world_lines.set_segments(
                &frame.gpu.device,
                &frame.gpu.queue,
                &self.scene.segments,
            );
            renderers.world_occluders.set_geometry(
                &frame.gpu.device,
                &frame.gpu.queue,
                &self.scene.occluder_vertices,
                &self.scene.occluder_indices,
            );
            self.world_uploaded = true;
        }

        // Frustum culling: draw only visible instances' buffer slices.
        // Visible instances also contribute this frame's silhouettes.
        let view_proj = self.player.view_proj(aspect);
        let frustum = Frustum::from_view_proj(view_proj);
        let eye = self.player.eye();
        let mut segment_ranges = Vec::new();
        let mut occluder_ranges = Vec::new();
        let mut silhouettes = Vec::new();
        for instance in &self.scene.instances {
            if frustum.intersects_aabb(instance.aabb_min, instance.aabb_max) {
                segment_ranges.push(instance.segments.clone());
                occluder_ranges.push(instance.occluder_indices.clone());
                self.scene
                    .instance_silhouettes_into(instance, eye, &mut silhouettes);
            }
        }
        renderers
            .world_silhouettes
            .set_segments(&frame.gpu.device, &frame.gpu.queue, &silhouettes);

        let world_uniform = CameraUniform::new(
            view_proj,
            frame.viewport,
            LINE_WIDTH_PX,
            eye,
            self.scene.fog_density,
            self.time,
            self.post_settings.glow,
        );
        renderers
            .world_camera
            .update(&frame.gpu.queue, &world_uniform);

        let hdr = renderers.post.hdr_view();
        let mut encoder = frame
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        renderers.world_occluders.render_ranges(
            &mut encoder,
            frame.depth,
            &renderers.world_camera,
            true,
            &occluder_ranges,
        );
        renderers.world_lines.render_ranges(
            &mut encoder,
            hdr,
            frame.depth,
            &renderers.world_camera,
            true,
            false,
            &segment_ranges,
        );
        renderers.world_silhouettes.render(
            &mut encoder,
            hdr,
            frame.depth,
            &renderers.world_camera,
            false,
            false,
        );

        if let Some(weapon) = &self.weapon {
            let (segments, vertices, indices) = weapon.frame_geometry(self.player.bob_phase());
            renderers
                .weapon_lines
                .set_segments(&frame.gpu.device, &frame.gpu.queue, &segments);
            renderers.weapon_occluders.set_geometry(
                &frame.gpu.device,
                &frame.gpu.queue,
                &vertices,
                &indices,
            );
            // Weapon camera: projection only — geometry is in view space.
            // No depth cue (fog 0), eye at origin.
            let weapon_uniform = CameraUniform::new(
                glam::camera::rh::proj::directx::perspective(
                    WEAPON_FOV_DEG.to_radians(),
                    aspect,
                    0.02,
                    10.0,
                ),
                frame.viewport,
                LINE_WIDTH_PX,
                Vec3::ZERO,
                0.0,
                self.time,
                self.post_settings.glow,
            );
            renderers
                .weapon_camera
                .update(&frame.gpu.queue, &weapon_uniform);
            // Clearing depth here is what keeps the sword out of the walls.
            renderers.weapon_occluders.render(
                &mut encoder,
                frame.depth,
                &renderers.weapon_camera,
                true,
            );
            renderers.weapon_lines.render(
                &mut encoder,
                hdr,
                frame.depth,
                &renderers.weapon_camera,
                false,
                false,
            );
        }

        // HUD: pixel-space strokes, always on top (fresh depth).
        renderers.hud_lines.set_segments(
            &frame.gpu.device,
            &frame.gpu.queue,
            &hud_segments(frame.viewport),
        );
        let hud_uniform = CameraUniform::new(
            glam::camera::rh::proj::directx::orthographic(
                0.0,
                frame.viewport.x,
                0.0,
                frame.viewport.y,
                -1.0,
                1.0,
            ),
            frame.viewport,
            HUD_LINE_WIDTH_PX,
            Vec3::ZERO,
            0.0,
            self.time,
            self.post_settings.glow,
        );
        renderers.hud_camera.update(&frame.gpu.queue, &hud_uniform);
        renderers.hud_lines.render(
            &mut encoder,
            hdr,
            frame.depth,
            &renderers.hud_camera,
            false,
            true,
        );

        // Phosphor: bloom + tonemap (+ optional CRT) to the output.
        renderers
            .post
            .run(&frame.gpu.queue, &mut encoder, frame.color, &self.post_settings);
        frame.gpu.queue.submit([encoder.finish()]);
    }
}

impl App for CorridorApp {
    fn init(&mut self, gpu: &Gpu, target_format: wgpu::TextureFormat) {
        self.renderers = Some(Renderers::new(&gpu.device, target_format));
    }

    fn update(&mut self, dt: f32, input: &Input) {
        self.time += dt;
        if input.is_just_pressed(KeyCode::KeyC) {
            self.post_settings.crt = if self.post_settings.crt > 0.0 { 0.0 } else { 1.0 };
        }
        self.tune_post(input);
        self.player.update(dt, input, &self.soup);
    }

    fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        viewport: Vec2,
    ) {
        // The shell hands us an encoder, but this app submits its own
        // (multi-pass with two cameras); the shell's encoder stays empty.
        let _ = encoder;
        self.draw(&Frame {
            gpu,
            color,
            depth,
            viewport,
        });
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(path) = flag_value(&args, "--screenshot") {
        return screenshot(Path::new(&path), &args);
    }

    println!(
        "controls: click captures · WASD walk · LShift sprint · Space jump · Esc releases\n\
         look:     [C] CRT · [ ] glow · - = bloom · 9 0 exposure (printed as changed)"
    );
    let mut app = CorridorApp::new()?;
    if args.iter().any(|a| a == "--crt") {
        app.post_settings.crt = 1.0;
    }
    vex_engine::run("vector3d — 03 corridor", app)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn parse_vec3(value: &str) -> Result<Vec3> {
    let parts: Vec<f32> = value
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .context("expected x,y,z")?;
    anyhow::ensure!(parts.len() == 3, "expected exactly three components");
    Ok(vec3(parts[0], parts[1], parts[2]))
}

/// Headless: place the player, render one frame, save a PNG.
fn screenshot(out: &Path, args: &[String]) -> Result<()> {
    let (width, height) = match flag_value(args, "--size") {
        Some(raw) => {
            let (w, h) = raw.split_once('x').context("--size expects WxH")?;
            (w.parse()?, h.parse()?)
        }
        None => (1280, 720),
    };

    let mut app = CorridorApp::new()?;
    if args.iter().any(|a| a == "--crt") {
        app.post_settings.crt = 1.0;
    }
    if let Some(glow) = flag_value(args, "--glow") {
        app.post_settings.glow = glow.parse().context("--glow expects a number")?;
    }
    if let Some(bloom) = flag_value(args, "--bloom") {
        app.post_settings.bloom_strength = bloom.parse().context("--bloom expects a number")?;
    }
    if let Some(pos) = flag_value(args, "--pos") {
        app.player.pos = parse_vec3(&pos)?;
    }
    if let Some(yaw) = flag_value(args, "--yaw") {
        app.player.yaw = yaw.parse::<f32>().context("--yaw expects degrees")?.to_radians();
    }
    if let Some(pitch) = flag_value(args, "--pitch") {
        app.player.pitch = pitch
            .parse::<f32>()
            .context("--pitch expects degrees")?
            .to_radians();
    }

    let gpu = Gpu::headless()?;
    let target = HeadlessTarget::new(&gpu.device, width, height);
    app.renderers = Some(Renderers::new(&gpu.device, vex_render::HEADLESS_FORMAT));
    app.draw(&Frame {
        gpu: &gpu,
        color: &target.color_view,
        depth: &target.depth_view,
        viewport: Vec2::new(width as f32, height as f32),
    });
    target.save_png(&gpu, out)?;
    println!("wrote {}", out.display());
    Ok(())
}
