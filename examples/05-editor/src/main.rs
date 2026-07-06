//! Level editor: block out playable spaces from glowing primitives, with
//! live hue / saturation / glow controls, then export a first-class `.vec`
//! (same weld/classify pipeline as Blender content — occluders double as
//! collision). Blender remains the tool for hero assets; this is for
//! spaces.
//!
//! Windowed:    cargo run -p editor -- [level.ron]
//! Headless:    cargo run -p editor -- --screenshot out.png
//!                  [--level L.ron] [--size WxH] [--pos x,y,z]
//!                  [--yaw DEG] [--pitch DEG]
//!
//! Click captures the mouse. WASD + Space/Ctrl fly (Shift = fast).
//! LMB place · RMB delete aimed · Z undo · TAB shape · R rotate ·
//! T/G Y/H U/J size · ,/. hue · N/M saturation · K/L glow ·
//! scroll distance · Q/E base height · F5 save · F6 export ·
//! F1 help · F2 grid · [ ] - = 9 0 C post dials · Esc releases.

mod level;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use glam::{Mat4, Vec2, Vec3, Vec4, vec2, vec3};
use level::{Level, PostDef, Shape, ShapeKind, bake, shape_aabb};
use vex_core::{EdgeKind, Segment, VecModel, font};
use vex_engine::{App, FlyCamera, Input, KeyCode, MouseButton};
use vex_render::{
    CameraBinding, CameraUniform, Gpu, HDR_FORMAT, LineRenderer, OccluderRenderer, PostProcessor,
    PostSettings,
};

const LINE_WIDTH_PX: f32 = 1.6;
const HUD_LINE_WIDTH_PX: f32 = 2.0;
const FOG_DENSITY: f32 = 0.004;
const GRID_EXTENT: i32 = 40;
const SNAP: f32 = 0.5;
const YAW_SNAP: f32 = 15.0;
const HUE_STEP: f32 = 15.0;
const SAT_STEP: f32 = 0.25;
const GLOW_STEP: f32 = 0.25;
const SIZE_STEP: f32 = 0.25;
const STATUS_SECONDS: f32 = 3.0;

fn snap(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

// ------------------------------------------------------------------ undo --

enum UndoOp {
    /// Last shape in the list was placed — undo pops it.
    Placed,
    /// A shape was deleted from `index` — undo reinserts it.
    Deleted(usize, Shape),
}

// ------------------------------------------------------------------- app --

struct Renderers {
    world_camera: CameraBinding,
    hud_camera: CameraBinding,
    level_lines: LineRenderer,
    overlay_lines: LineRenderer,
    occluders: OccluderRenderer,
    hud_lines: LineRenderer,
    post: PostProcessor,
}

impl Renderers {
    fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let world_camera = CameraBinding::new(device);
        let hud_camera = CameraBinding::new(device);
        Self {
            level_lines: LineRenderer::new(device, HDR_FORMAT, &world_camera),
            overlay_lines: LineRenderer::new(device, HDR_FORMAT, &world_camera),
            occluders: OccluderRenderer::new(device, &world_camera),
            hud_lines: LineRenderer::new(device, HDR_FORMAT, &hud_camera),
            post: PostProcessor::new(device, output_format),
            world_camera,
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

struct EditorApp {
    path: PathBuf,
    level: Level,
    model: VecModel,
    aabbs: Vec<(Vec3, Vec3)>,
    ghost: Shape,
    ghost_model: Option<VecModel>,
    place_distance: f32,
    base_height: f32,
    undo: Vec<UndoOp>,
    cam: FlyCamera,
    post_settings: PostSettings,
    renderers: Option<Renderers>,
    level_uploaded: bool,
    time: f32,
    status: String,
    status_ttl: f32,
    show_help: bool,
    show_grid: bool,
}

impl EditorApp {
    fn new(path: PathBuf) -> Result<Self> {
        let level = if path.exists() {
            Level::load(&path)?
        } else {
            Level::default()
        };
        let post_settings = PostSettings {
            exposure: level.post.exposure,
            bloom_strength: level.post.bloom_strength,
            crt: level.post.crt,
            glow: level.post.glow,
        };
        let (model, _) = bake(&level)?;
        let aabbs = level.shapes.iter().map(shape_aabb).collect();
        let shape_count = level.shapes.len();
        let mut app = Self {
            path,
            level,
            model,
            aabbs,
            ghost: Shape {
                kind: ShapeKind::Box,
                pos: (0.0, 0.0, 0.0),
                size: (3.0, 2.5, 0.5),
                yaw: 0.0,
                hue: 120.0,
                sat: 0.85,
                glow: 1.0,
            },
            ghost_model: None,
            place_distance: 8.0,
            base_height: 0.0,
            undo: Vec::new(),
            cam: {
                let mut cam = FlyCamera::new(vec3(0.0, 3.0, 12.0), 0.0, -0.2);
                cam.speed = 8.0;
                cam
            },
            post_settings,
            renderers: None,
            level_uploaded: false,
            time: 0.0,
            status: String::new(),
            status_ttl: 0.0,
        show_help: true,
            show_grid: true,
        };
        app.say(&format!("LOADED {} SHAPES", shape_count));
        Ok(app)
    }

    fn say(&mut self, message: &str) {
        self.status = message.to_string();
        self.status_ttl = STATUS_SECONDS;
    }

    /// Re-bake the level preview after any edit. On bake failure (palette
    /// full) the caller must roll its edit back first.
    fn rebake(&mut self) -> Result<()> {
        let (model, _) = bake(&self.level)?;
        self.model = model;
        self.aabbs = self.level.shapes.iter().map(shape_aabb).collect();
        self.level_uploaded = false;
        Ok(())
    }

    /// Ghost position: ahead of the camera, snapped to the grid.
    fn ghost_pos(&self) -> Vec3 {
        let raw = self.cam.pos + self.cam.forward() * self.place_distance;
        vec3(snap(raw.x, SNAP), self.base_height, snap(raw.z, SNAP))
    }

    fn rebuild_ghost(&mut self) {
        let mut shape = self.ghost;
        let pos = self.ghost_pos();
        shape.pos = (pos.x, pos.y, pos.z);
        let single = Level {
            shapes: vec![shape],
            ..Default::default()
        };
        self.ghost_model = bake(&single).ok().map(|(m, _)| m);
    }

    /// Nearest shape AABB hit by the camera ray.
    fn aimed_shape(&self) -> Option<usize> {
        let (origin, dir) = (self.cam.pos, self.cam.forward());
        let mut best: Option<(f32, usize)> = None;
        for (i, (min, max)) in self.aabbs.iter().enumerate() {
            let inv = dir.recip();
            let t0 = (*min - origin) * inv;
            let t1 = (*max - origin) * inv;
            let (lo, hi) = (t0.min(t1), t0.max(t1));
            let enter = lo.max_element().max(0.05);
            let exit = hi.min_element();
            if enter <= exit && enter < 120.0 && best.is_none_or(|(t, _)| enter < t) {
                best = Some((enter, i));
            }
        }
        best.map(|(_, i)| i)
    }

    fn place(&mut self) {
        let mut shape = self.ghost;
        let pos = self.ghost_pos();
        shape.pos = (pos.x, pos.y, pos.z);
        self.level.shapes.push(shape);
        match self.rebake() {
            Ok(()) => {
                self.undo.push(UndoOp::Placed);
                self.say(&format!(
                    "PLACED {} ({} TOTAL)",
                    shape.kind.label(),
                    self.level.shapes.len()
                ));
            }
            Err(err) => {
                self.level.shapes.pop();
                let _ = self.rebake();
                self.say(&format!("CANNOT PLACE: {err}"));
            }
        }
    }

    fn delete_aimed(&mut self) {
        let Some(index) = self.aimed_shape() else {
            self.say("NOTHING AIMED");
            return;
        };
        let shape = self.level.shapes.remove(index);
        let _ = self.rebake();
        self.undo.push(UndoOp::Deleted(index, shape));
        self.say(&format!("DELETED {}", shape.kind.label()));
    }

    fn undo_last(&mut self) {
        match self.undo.pop() {
            Some(UndoOp::Placed) => {
                self.level.shapes.pop();
                let _ = self.rebake();
                self.say("UNDO PLACE");
            }
            Some(UndoOp::Deleted(index, shape)) => {
                let index = index.min(self.level.shapes.len());
                self.level.shapes.insert(index, shape);
                let _ = self.rebake();
                self.say("UNDO DELETE");
            }
            None => self.say("NOTHING TO UNDO"),
        }
    }

    fn save(&mut self) {
        self.level.post = PostDef {
            exposure: self.post_settings.exposure,
            bloom_strength: self.post_settings.bloom_strength,
            crt: self.post_settings.crt,
            glow: self.post_settings.glow,
        };
        match self.level.save(&self.path) {
            Ok(()) => self.say(&format!("SAVED {}", self.path.display())),
            Err(err) => self.say(&format!("SAVE FAILED: {err}")),
        }
    }

    fn export(&mut self) {
        let out = self.path.with_extension("vec");
        match self.model.save(&out) {
            Ok(()) => self.say(&format!(
                "EXPORTED {} ({} EDGES)",
                out.display(),
                self.model.edges.len()
            )),
            Err(err) => self.say(&format!("EXPORT FAILED: {err}")),
        }
    }

    fn handle_edit_keys(&mut self, input: &Input) {
        let shift = input.is_down(KeyCode::ShiftLeft);
        if input.is_just_pressed(KeyCode::Tab) {
            let i = ShapeKind::ALL.iter().position(|&k| k == self.ghost.kind).unwrap_or(0);
            self.ghost.kind = ShapeKind::ALL[(i + 1) % ShapeKind::ALL.len()];
        }
        if input.is_just_pressed(KeyCode::KeyR) {
            let step = if shift { -YAW_SNAP } else { YAW_SNAP };
            self.ghost.yaw = (self.ghost.yaw + step).rem_euclid(360.0);
        }
        let size_axis = |axis: usize, delta: f32, ghost: &mut Shape| {
            let v = match axis {
                0 => &mut ghost.size.0,
                1 => &mut ghost.size.1,
                _ => &mut ghost.size.2,
            };
            *v = (*v + delta).clamp(0.25, 40.0);
        };
        if input.is_just_pressed(KeyCode::KeyT) {
            size_axis(0, SIZE_STEP, &mut self.ghost);
        }
        if input.is_just_pressed(KeyCode::KeyG) {
            size_axis(0, -SIZE_STEP, &mut self.ghost);
        }
        if input.is_just_pressed(KeyCode::KeyY) {
            size_axis(1, SIZE_STEP, &mut self.ghost);
        }
        if input.is_just_pressed(KeyCode::KeyH) {
            size_axis(1, -SIZE_STEP, &mut self.ghost);
        }
        if input.is_just_pressed(KeyCode::KeyU) {
            size_axis(2, SIZE_STEP, &mut self.ghost);
        }
        if input.is_just_pressed(KeyCode::KeyJ) {
            size_axis(2, -SIZE_STEP, &mut self.ghost);
        }
        if input.is_just_pressed(KeyCode::Comma) {
            self.ghost.hue = (self.ghost.hue - HUE_STEP).rem_euclid(360.0);
        }
        if input.is_just_pressed(KeyCode::Period) {
            self.ghost.hue = (self.ghost.hue + HUE_STEP).rem_euclid(360.0);
        }
        if input.is_just_pressed(KeyCode::KeyN) {
            self.ghost.sat = (self.ghost.sat - SAT_STEP).clamp(0.0, 1.0);
        }
        if input.is_just_pressed(KeyCode::KeyM) {
            self.ghost.sat = (self.ghost.sat + SAT_STEP).clamp(0.0, 1.0);
        }
        if input.is_just_pressed(KeyCode::KeyK) {
            self.ghost.glow = (self.ghost.glow - GLOW_STEP).clamp(0.25, 4.0);
        }
        if input.is_just_pressed(KeyCode::KeyL) {
            self.ghost.glow = (self.ghost.glow + GLOW_STEP).clamp(0.25, 4.0);
        }
        if input.is_just_pressed(KeyCode::KeyQ) {
            self.base_height = (self.base_height - SNAP).max(0.0);
        }
        if input.is_just_pressed(KeyCode::KeyE) {
            self.base_height += SNAP;
        }
        self.place_distance = (self.place_distance + input.scroll_delta()).clamp(2.0, 30.0);
    }

    fn handle_post_keys(&mut self, input: &Input) {
        let p = &mut self.post_settings;
        if input.is_just_pressed(KeyCode::BracketLeft) {
            p.glow = (p.glow - 0.05).max(0.0);
        }
        if input.is_just_pressed(KeyCode::BracketRight) {
            p.glow = (p.glow + 0.05).min(2.0);
        }
        if input.is_just_pressed(KeyCode::Minus) {
            p.bloom_strength = (p.bloom_strength - 0.02).max(0.0);
        }
        if input.is_just_pressed(KeyCode::Equal) {
            p.bloom_strength = (p.bloom_strength + 0.02).min(0.5);
        }
        if input.is_just_pressed(KeyCode::Digit9) {
            p.exposure = (p.exposure - 0.1).max(0.2);
        }
        if input.is_just_pressed(KeyCode::Digit0) {
            p.exposure = (p.exposure + 0.1).min(3.0);
        }
        if input.is_just_pressed(KeyCode::KeyC) {
            p.crt = if p.crt > 0.0 { 0.0 } else { 1.0 };
        }
    }

    /// Grid, axes, ghost preview, aim highlight — rebuilt every frame.
    fn overlay_segments(&self) -> Vec<Segment> {
        let mut out = Vec::new();
        if self.show_grid {
            let dim = Vec4::new(0.05, 0.30, 0.10, 0.30);
            let bright = Vec4::new(0.08, 0.45, 0.15, 0.45);
            let e = GRID_EXTENT as f32;
            for i in -GRID_EXTENT..=GRID_EXTENT {
                let v = i as f32;
                let color = if i % 5 == 0 { bright } else { dim };
                out.push(Segment::new(vec3(v, 0.0, -e), vec3(v, 0.0, e), color));
                out.push(Segment::new(vec3(-e, 0.0, v), vec3(e, 0.0, v), color));
            }
            out.push(Segment::new(
                Vec3::ZERO,
                vec3(2.0, 0.0, 0.0),
                Vec4::new(1.0, 0.2, 0.2, 1.0),
            ));
            out.push(Segment::new(
                Vec3::ZERO,
                vec3(0.0, 0.0, 2.0),
                Vec4::new(0.25, 0.5, 1.0, 1.0),
            ));
        }
        // Level silhouettes (view-dependent, so per frame).
        out.extend(
            self.model
                .silhouette_segments(Mat4::IDENTITY, self.cam.pos, 1.0),
        );
        // Ghost: same look it will have, at reduced intensity, plus a
        // base cross so the snapped anchor is obvious.
        if let Some(ghost) = &self.ghost_model {
            out.extend(ghost.edge_segments(EdgeKind::Always, 0.40));
            out.extend(ghost.silhouette_segments(Mat4::IDENTITY, self.cam.pos, 0.40));
        }
        let p = self.ghost_pos();
        let cross = Vec4::new(1.0, 0.9, 0.3, 0.8);
        out.push(Segment::new(p - Vec3::X * 0.4, p + Vec3::X * 0.4, cross));
        out.push(Segment::new(p - Vec3::Z * 0.4, p + Vec3::Z * 0.4, cross));
        // Aim highlight: pulsing white box around the shape RMB would
        // delete.
        if let Some(i) = self.aimed_shape() {
            let (min, max) = self.aabbs[i];
            let pulse = 0.9 + 0.4 * (self.time * 6.0).sin();
            let color = Vec4::new(1.0, 1.0, 1.0, pulse);
            let c = [
                vec3(min.x, min.y, min.z),
                vec3(max.x, min.y, min.z),
                vec3(max.x, min.y, max.z),
                vec3(min.x, min.y, max.z),
                vec3(min.x, max.y, min.z),
                vec3(max.x, max.y, min.z),
                vec3(max.x, max.y, max.z),
                vec3(min.x, max.y, max.z),
            ];
            for (a, b) in [
                (0, 1), (1, 2), (2, 3), (3, 0),
                (4, 5), (5, 6), (6, 7), (7, 4),
                (0, 4), (1, 5), (2, 6), (3, 7),
            ] {
                out.push(Segment::new(c[a], c[b], color));
            }
        }
        out
    }

    fn hud(&self, viewport: Vec2) -> Vec<Segment> {
        let green = Vec4::new(0.5, 1.0, 0.25, 1.0);
        let dim = Vec4::new(0.3, 0.7, 0.2, 0.8);
        let amber = Vec4::new(1.0, 0.75, 0.2, 1.3);
        let size = 14.0;
        let step = 22.0;
        let mut out = Vec::new();
        let mut y = viewport.y - 34.0;
        let line = |text: &str, color: Vec4, out: &mut Vec<Segment>, y: &mut f32| {
            out.extend(font::text_segments(text, vec2(24.0, *y), size, color));
            *y -= step;
        };
        let g = &self.ghost;
        line(&format!("SHAPE {}", g.kind.label()), green, &mut out, &mut y);
        line(
            &format!("SIZE {:.2} {:.2} {:.2}", g.size.0, g.size.1, g.size.2),
            green,
            &mut out,
            &mut y,
        );
        line(
            &format!("HUE {:>3.0} SAT {:.2} GLOW {:.2}", g.hue, g.sat, g.glow),
            green,
            &mut out,
            &mut y,
        );
        line(
            &format!(
                "DIST {:.1} BASE {:.1} YAW {:.0}",
                self.place_distance, self.base_height, g.yaw
            ),
            dim,
            &mut out,
            &mut y,
        );
        line(
            &format!(
                "SHAPES {} PALETTE {}/255",
                self.level.shapes.len(),
                self.level.palette_len()
            ),
            dim,
            &mut out,
            &mut y,
        );
        if self.status_ttl > 0.0 {
            line(&self.status.clone(), amber, &mut out, &mut y);
        }
        // Color swatch in the ghost's actual color, next to the HUE line.
        let swatch_color = {
            let rgb = level::hsv_to_rgb(g.hue, g.sat, 1.0);
            Vec4::new(rgb.x, rgb.y, rgb.z, g.glow.max(0.6))
        };
        let (sx, sy) = (font::text_width("HUE 000 SAT 0.00 GLOW 0.00", size) + 44.0,
                        viewport.y - 34.0 - 2.0 * step);
        for k in 0..4 {
            let o = k as f32 * 4.0;
            out.push(Segment::new(
                vec3(sx, sy + o, 0.0),
                vec3(sx + 26.0, sy + o, 0.0),
                swatch_color,
            ));
        }

        if self.show_help {
            let mut hy = 34.0 + 7.0 * step;
            let help = |text: &str, out: &mut Vec<Segment>, hy: &mut f32| {
                out.extend(font::text_segments(text, vec2(24.0, *hy), 12.0, dim));
                *hy -= step * 0.82;
            };
            help("LMB PLACE   RMB DELETE   Z UNDO", &mut out, &mut hy);
            help("TAB SHAPE   R ROTATE   T/G Y/H U/J SIZE", &mut out, &mut hy);
            help(",/. HUE   N/M SAT   K/L GLOW", &mut out, &mut hy);
            help("SCROLL DIST   Q/E BASE HEIGHT", &mut out, &mut hy);
            help("F5 SAVE   F6 EXPORT VEC   F2 GRID   F1 HELP", &mut out, &mut hy);
            help("[ ] GLOW  - = BLOOM  9 0 EXPOSURE  C CRT", &mut out, &mut hy);
            help("WASD + SPACE/CTRL FLY   SHIFT FAST", &mut out, &mut hy);
        }
        out
    }

    fn draw(&mut self, frame: &Frame) {
        // Built before renderers are mutably borrowed.
        let overlay = self.overlay_segments();
        let hud = self.hud(frame.viewport);
        let Some(renderers) = self.renderers.as_mut() else {
            return;
        };
        let aspect = frame.viewport.x / frame.viewport.y;
        renderers.post.ensure_size(
            &frame.gpu.device,
            frame.viewport.x as u32,
            frame.viewport.y as u32,
        );

        if !self.level_uploaded {
            renderers.level_lines.set_segments(
                &frame.gpu.device,
                &frame.gpu.queue,
                &self.model.edge_segments(EdgeKind::Always, 1.0),
            );
            renderers.occluders.set_geometry(
                &frame.gpu.device,
                &frame.gpu.queue,
                &self.model.vertices,
                &self.model.occluder_indices,
            );
            self.level_uploaded = true;
        }

        let view = glam::camera::rh::view::look_to_mat4(self.cam.pos, self.cam.forward(), Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            70f32.to_radians(),
            aspect,
            0.05,
            400.0,
        );
        let uniform = CameraUniform::new(
            proj * view,
            frame.viewport,
            LINE_WIDTH_PX,
            self.cam.pos,
            FOG_DENSITY,
            self.time,
            self.post_settings.glow,
        );
        renderers.world_camera.update(&frame.gpu.queue, &uniform);

        renderers
            .overlay_lines
            .set_segments(&frame.gpu.device, &frame.gpu.queue, &overlay);

        let hdr = renderers.post.hdr_view();
        let mut encoder = frame
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        renderers
            .occluders
            .render(&mut encoder, frame.depth, &renderers.world_camera, true);
        renderers.level_lines.render(
            &mut encoder,
            hdr,
            frame.depth,
            &renderers.world_camera,
            true,
            false,
        );
        renderers.overlay_lines.render(
            &mut encoder,
            hdr,
            frame.depth,
            &renderers.world_camera,
            false,
            false,
        );

        renderers
            .hud_lines
            .set_segments(&frame.gpu.device, &frame.gpu.queue, &hud);
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

        renderers
            .post
            .run(&frame.gpu.queue, &mut encoder, frame.color, &self.post_settings);
        frame.gpu.queue.submit([encoder.finish()]);
    }
}

impl App for EditorApp {
    fn init(&mut self, gpu: &Gpu, target_format: wgpu::TextureFormat) {
        self.renderers = Some(Renderers::new(&gpu.device, target_format));
    }

    fn update(&mut self, dt: f32, input: &Input) {
        self.time += dt;
        self.status_ttl = (self.status_ttl - dt).max(0.0);
        self.cam.update(dt, input);
        self.handle_edit_keys(input);
        self.handle_post_keys(input);
        if input.is_just_pressed(KeyCode::F1) {
            self.show_help = !self.show_help;
        }
        if input.is_just_pressed(KeyCode::F2) {
            self.show_grid = !self.show_grid;
        }
        if input.is_just_pressed(KeyCode::F5) {
            self.save();
        }
        if input.is_just_pressed(KeyCode::F6) {
            self.export();
        }
        if input.is_just_pressed(KeyCode::KeyZ) {
            self.undo_last();
        }
        if input.is_captured() && input.is_mouse_just_pressed(MouseButton::Left) {
            self.place();
        }
        if input.is_captured() && input.is_mouse_just_pressed(MouseButton::Right) {
            self.delete_aimed();
        }
        // Ghost preview tracks the camera every frame; it's a handful of
        // triangles, so re-baking is effectively free.
        self.rebuild_ghost();
    }

    fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        viewport: Vec2,
    ) {
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

    if let Some(out) = flag_value(&args, "--screenshot") {
        return screenshot(Path::new(&out), &args);
    }

    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "level.ron".to_string());
    println!(
        "editing {path} — click captures · F1 in-app help · F5 saves · F6 exports .vec"
    );
    vex_engine::run("vector3d — editor", EditorApp::new(PathBuf::from(path))?)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Headless render of a level (plus the editor chrome) for verification.
fn screenshot(out: &Path, args: &[String]) -> Result<()> {
    let (width, height) = match flag_value(args, "--size") {
        Some(raw) => {
            let (w, h) = raw.split_once('x').context("--size expects WxH")?;
            (w.parse()?, h.parse()?)
        }
        None => (1280, 720),
    };
    let path = flag_value(args, "--level").unwrap_or_else(|| "level.ron".to_string());
    let mut app = EditorApp::new(PathBuf::from(path))?;
    if let Some(pos) = flag_value(args, "--pos") {
        let parts: Vec<f32> = pos
            .split(',')
            .map(|p| p.trim().parse::<f32>())
            .collect::<Result<_, _>>()
            .context("--pos expects x,y,z")?;
        anyhow::ensure!(parts.len() == 3, "--pos expects x,y,z");
        app.cam.pos = vec3(parts[0], parts[1], parts[2]);
    }
    if let Some(yaw) = flag_value(args, "--yaw") {
        app.cam.yaw = yaw.parse::<f32>().context("--yaw expects degrees")?.to_radians();
    }
    if let Some(pitch) = flag_value(args, "--pitch") {
        app.cam.pitch = pitch
            .parse::<f32>()
            .context("--pitch expects degrees")?
            .to_radians();
    }
    app.rebuild_ghost();

    let gpu = Gpu::headless()?;
    let target = vex_render::HeadlessTarget::new(&gpu.device, width, height);
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
