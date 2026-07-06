//! Arena: the playable demo. Waves of enemies pour out of the gates of a
//! neon fight pit; cut them down with the sword. Everything on screen is
//! the engine's stroke pipeline — enemies, particles, banners included.
//!
//! Windowed:    cargo run -p arena
//! Headless:    cargo run -p arena -- --screenshot out.png [--size WxH]
//!                  [--demo SECONDS] [--recoil 0..1] [--pos x,y,z]
//!                  [--yaw DEG] [--pitch DEG]
//!
//! Controls: click captures · WASD + Space · left click fires the pistol ·
//!           [R] restarts after game over · [C] CRT · Esc releases.

mod game;

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec2, Vec3, Vec4, vec2, vec3};
use game::{EnemyKind, Game, GameEvent, Phase};
use vex_audio::{AudioEngine, Sfx};
use vex_core::{EdgeKind, Frustum, Segment, VecModel, font, phosphor};
use vex_engine::{App, BakedScene, FpsController, Input, KeyCode, MouseButton, TriangleSoup};
use vex_render::{
    CameraBinding, CameraUniform, Gpu, HDR_FORMAT, LineRenderer, OccluderRenderer,
    PostProcessor, PostSettings,
};
#[cfg(not(target_arch = "wasm32"))]
use {std::path::Path, vex_render::HeadlessTarget};

#[cfg(not(target_arch = "wasm32"))]
const SCENE_PATH: &str = "assets/arena/scene.ron";
#[cfg(not(target_arch = "wasm32"))]
const SHARD_PATH: &str = "assets/arena/shard.vec";
#[cfg(not(target_arch = "wasm32"))]
const SENTINEL_PATH: &str = "assets/arena/sentinel.vec";
#[cfg(not(target_arch = "wasm32"))]
const HEALTHPACK_PATH: &str = "assets/arena/healthpack.vec";

/// Asset root: next to the executable for shipped builds (the packaged
/// tarball puts `assets/` beside the binary), falling back to the working
/// directory for `cargo run` from the repo.
#[cfg(not(target_arch = "wasm32"))]
fn asset_root() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && dir.join(SCENE_PATH).exists()
    {
        return dir.to_path_buf();
    }
    std::path::PathBuf::from(".")
}

/// The browser has no filesystem: every asset is baked into the wasm.
#[cfg(target_arch = "wasm32")]
mod embedded {
    pub const SCENE: &str = include_str!("../../../assets/arena/scene.ron");
    pub const ARENA: &[u8] = include_bytes!("../../../assets/arena/arena.vec");
    pub const SHARD: &[u8] = include_bytes!("../../../assets/arena/shard.vec");
    pub const SENTINEL: &[u8] = include_bytes!("../../../assets/arena/sentinel.vec");
    pub const HEALTHPACK: &[u8] = include_bytes!("../../../assets/arena/healthpack.vec");
    pub const PISTOL: &[u8] = include_bytes!("../../../assets/pistol.vec");
}
const LINE_WIDTH_PX: f32 = 1.6;
const HUD_LINE_WIDTH_PX: f32 = 2.0;
const COLLISION_CELL: f32 = 2.0;
const WEAPON_FOV_DEG: f32 = 55.0;
/// Longest-axis length of the pistol viewmodel, in view units.
const WEAPON_LENGTH: f32 = 0.52;
/// The pistol's materials aren't emissive, so lift its stroke intensity
/// into HDR slightly — a faint phosphor halo that keeps the viewmodel
/// readable against busy scenes. Runs through the scene glow dial like
/// everything else.
const WEAPON_GLOW: f32 = 1.35;

// Reorient the lying-flat pistol (model axes: Z = barrel, X = gun-vertical,
// Y = thickness) into a viewmodel frame — barrel forward (−Z), grip down
// (−Y). Signs picked empirically from the axis renders.
const GUN_STAND: f32 = -std::f32::consts::FRAC_PI_2; // roll upright about barrel
const GUN_AIM: f32 = std::f32::consts::PI; // aim the barrel INTO the screen
/// Idle adjustments once the barrel already points forward.
const GUN_BASE_PITCH: f32 = -0.04;
const GUN_BASE_YAW: f32 = 0.06;

// ---------------------------------------------------------------- weapon --

struct Weapon {
    model: VecModel,
    fit: Mat4,
}

impl Weapon {
    fn new(model: VecModel) -> Self {
        let extent = (model.aabb_max - model.aabb_min).max_element().max(1e-4);
        let center = (model.aabb_min + model.aabb_max) * 0.5;
        let reorient = Quat::from_rotation_y(GUN_AIM) * Quat::from_rotation_z(GUN_STAND);
        let fit = Mat4::from_scale(Vec3::splat(WEAPON_LENGTH / extent))
            * Mat4::from_quat(reorient)
            * Mat4::from_translation(-center);
        Self { model, fit }
    }

    /// Idle hold plus recoil. The gun rests low-center, barrel forward;
    /// firing kicks it back toward the camera and flips the muzzle up,
    /// easing back to rest as `recoil` decays 1 → 0.
    fn placement(&self, bob_phase: f32, recoil: f32) -> Mat4 {
        let bob = vec3(
            (bob_phase * 0.5).cos() * 0.010,
            (bob_phase).sin() * 0.012 - 0.015,
            0.0,
        );
        let kick = vec3(0.0, 0.02 * recoil, 0.10 * recoil);
        Mat4::from_translation(vec3(0.16, -0.20, -0.52) + bob + kick)
            * Mat4::from_quat(
                Quat::from_rotation_x(GUN_BASE_PITCH + 0.38 * recoil)
                    * Quat::from_rotation_y(GUN_BASE_YAW),
            )
            * self.fit
    }

    fn frame_geometry(
        &self,
        bob_phase: f32,
        recoil: f32,
    ) -> (Vec<Segment>, Vec<Vec3>, Vec<u32>) {
        let placement = self.placement(bob_phase, recoil);
        let mut segments: Vec<Segment> = self
            .model
            .edge_segments(EdgeKind::Always, WEAPON_GLOW)
            .into_iter()
            .map(|s| Segment {
                a: placement.transform_point3(s.a),
                b: placement.transform_point3(s.b),
                ..s
            })
            .collect();
        segments.extend(
            self.model
                .silhouette_segments(placement, Vec3::ZERO, WEAPON_GLOW),
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

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Muzzle tip in weapon-view space (barrel points −Z; hand-tuned to the
/// viewmodel). The flash and any barrel effects hang off this point.
const MUZZLE: Vec3 = vec3(0.16, -0.10, -1.02);

/// A brief radial star at the barrel tip on the frames right after firing.
/// Drawn with the weapon camera, so it stays glued to the gun.
fn muzzle_flash(recoil: f32) -> Vec<Segment> {
    // Only the first ~third of the recoil decay (a couple of frames).
    let k = ((recoil - 0.65) / 0.35).clamp(0.0, 1.0);
    if k <= 0.0 {
        return Vec::new();
    }
    let len = 0.04 + 0.06 * k;
    let color = Vec4::new(1.0, 0.85, 0.4, 1.4 + k);
    // Ride the same recoil kick the viewmodel uses, so the flash stays
    // glued to the barrel as the gun jumps.
    let origin = MUZZLE + vec3(0.0, 0.02 * recoil, 0.10 * recoil);
    const SPOKES: usize = 6;
    (0..SPOKES)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / SPOKES as f32;
            // Forward-biased so it reads as blowing out of the barrel.
            let dir = vec3(a.cos() * len, a.sin() * len, -len * 0.5);
            Segment::new(origin, origin + dir, color)
        })
        .collect()
}

// --------------------------------------------------------------- enemies --

struct GameModels {
    shard: VecModel,
    sentinel: VecModel,
    healthpack: VecModel,
}

impl GameModels {
    fn get(&self, kind: EnemyKind) -> &VecModel {
        match kind {
            EnemyKind::Shard => &self.shard,
            EnemyKind::Sentinel => &self.sentinel,
        }
    }
}

/// Enemies, spawn telegraphs, pickups and particles as this frame's
/// dynamic geometry (world-space segments + occluder soup).
fn build_dynamic(
    models: &GameModels,
    game: &Game,
    time: f32,
) -> (Vec<Segment>, Vec<Vec3>, Vec<u32>) {
    let mut segments = Vec::new();
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // The medkit: spinning cross hovering over a pulsing claim ring.
    if let Some(pack) = &game.health_pack {
        let pop = smoothstep((pack.age / 0.35).min(1.0));
        let transform = Mat4::from_translation(pack.center())
            * Mat4::from_rotation_y(pack.age * 0.9)
            * Mat4::from_scale(Vec3::splat(pop));
        segments.extend(
            models
                .healthpack
                .edge_segments(EdgeKind::Always, 0.7 + 0.3 * (pack.age * 2.2).sin())
                .into_iter()
                .map(|s| Segment {
                    a: transform.transform_point3(s.a),
                    b: transform.transform_point3(s.b),
                    ..s
                }),
        );
        let base = vertices.len() as u32;
        vertices.extend(
            models
                .healthpack
                .vertices
                .iter()
                .map(|&v| transform.transform_point3(v)),
        );
        indices.extend(models.healthpack.occluder_indices.iter().map(|&i| i + base));
        let ring_radius = 0.9 + (pack.age * 2.0).sin() * 0.12;
        segments.extend(ring_segments(
            pack.pos + Vec3::Y * 0.02,
            ring_radius,
            Vec4::new(1.0, 0.10, 0.08, 0.5 * pop),
        ));
    }

    for enemy in &game.enemies {
        let progress = enemy.spawn_progress();
        let scale = 0.2 + 0.8 * smoothstep(progress);
        // Shards tumble freely; sentinels aim their eye at the player.
        let yaw = match enemy.kind {
            EnemyKind::Shard => enemy.age * 1.4 + enemy.wobble,
            EnemyKind::Sentinel => enemy.yaw,
        };
        let transform = Mat4::from_translation(enemy.center())
            * Mat4::from_rotation_y(yaw)
            * Mat4::from_scale(Vec3::splat(scale));
        let intensity = (0.25 + 0.75 * progress) * (1.0 + enemy.hit_flash * 2.0);
        let model = models.get(enemy.kind);
        segments.extend(model.edge_segments(EdgeKind::Always, intensity).into_iter().map(
            |s| Segment {
                a: transform.transform_point3(s.a),
                b: transform.transform_point3(s.b),
                ..s
            },
        ));
        if progress >= 1.0 {
            let base = vertices.len() as u32;
            vertices.extend(
                model
                    .vertices
                    .iter()
                    .map(|&v| transform.transform_point3(v)),
            );
            indices.extend(model.occluder_indices.iter().map(|&i| i + base));
        } else {
            // Spawn telegraph: a ring blooming on the floor.
            segments.extend(telegraph_ring(enemy.pos, enemy.kind, progress));
        }
    }

    // Enemy bolts: hot glowing darts in their shooter's color.
    for bolt in &game.bolts {
        let half = bolt.segment_half();
        segments.push(Segment::new(bolt.pos - half, bolt.pos + half, bolt.color));
    }

    for particle in &game.particles {
        let fade = (particle.life / particle.max_life).clamp(0.0, 1.0);
        let color = Vec4::new(
            particle.color.x,
            particle.color.y,
            particle.color.z,
            particle.color.w * fade,
        );
        segments.push(Segment::new(
            particle.pos - particle.axis,
            particle.pos + particle.axis,
            color,
        ));
    }

    // A faint pulse ring around the player during game over, for drama.
    if matches!(game.phase, Phase::GameOver) {
        let pulse = 2.0 + (time * 1.5).sin() * 0.3;
        segments.extend(ring_segments(
            Vec3::ZERO + vec3(0.0, 0.02, 0.0),
            pulse,
            Vec4::new(1.0, 0.08, 0.05, 0.5),
        ));
    }

    (segments, vertices, indices)
}

fn telegraph_ring(at: Vec3, kind: EnemyKind, progress: f32) -> Vec<Segment> {
    let radius = 0.3 + kind.radius() * 2.2 * progress;
    let color = kind.color() * Vec4::new(1.0, 1.0, 1.0, 0.4 + 0.6 * progress);
    ring_segments(at + Vec3::Y * 0.02, radius, color)
}

fn ring_segments(center: Vec3, radius: f32, color: Vec4) -> Vec<Segment> {
    const SIDES: usize = 14;
    (0..SIDES)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / SIDES as f32;
            let b = std::f32::consts::TAU * (i + 1) as f32 / SIDES as f32;
            Segment::new(
                center + vec3(a.cos() * radius, 0.0, a.sin() * radius),
                center + vec3(b.cos() * radius, 0.0, b.sin() * radius),
                color,
            )
        })
        .collect()
}

// ------------------------------------------------------------------- hud --

fn hud_segments(viewport: Vec2, game: &Game) -> Vec<Segment> {
    let red = Vec4::new(phosphor::RED.x, phosphor::RED.y, phosphor::RED.z, 0.95);
    let lime = Vec4::new(phosphor::LIME.x, phosphor::LIME.y, phosphor::LIME.z, 0.9);
    let cyan = Vec4::new(phosphor::CYAN.x, phosphor::CYAN.y, phosphor::CYAN.z, 1.0);

    let mut out = font::text_segments(
        &format!("HEALTH {:.0}", game.hp),
        vec2(28.0, 26.0),
        20.0,
        red,
    );
    out.extend(font::text_segments(
        &format!("WAVE {}", game.wave),
        vec2(28.0, viewport.y - 46.0),
        20.0,
        lime,
    ));
    let score_text = format!("SCORE {}", game.score);
    out.extend(font::text_segments(
        &score_text,
        vec2(
            viewport.x - font::text_width(&score_text, 20.0) - 28.0,
            viewport.y - 46.0,
        ),
        20.0,
        lime,
    ));

    // Crosshair.
    let center = viewport * 0.5;
    let (gap, len) = (5.0, 9.0);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        out.push(Segment::new(
            vec3(center.x + dx * gap, center.y + dy * gap, 0.0),
            vec3(center.x + dx * (gap + len), center.y + dy * (gap + len), 0.0),
            Vec4::new(phosphor::RED.x, phosphor::RED.y, phosphor::RED.z, 0.8),
        ));
    }

    // Damage flash: a red frame that fades out.
    if game.damage_flash > 0.0 {
        let inset = 14.0;
        let color = Vec4::new(1.0, 0.06, 0.04, game.damage_flash * 1.4);
        let corners = [
            vec3(inset, inset, 0.0),
            vec3(viewport.x - inset, inset, 0.0),
            vec3(viewport.x - inset, viewport.y - inset, 0.0),
            vec3(inset, viewport.y - inset, 0.0),
        ];
        for i in 0..4 {
            out.push(Segment::new(corners[i], corners[(i + 1) % 4], color));
        }
    }

    // Banners.
    let mut banner = |text: &str, y: f32, size: f32, color: Vec4| {
        let x = (viewport.x - font::text_width(text, size)) * 0.5;
        out.extend(font::text_segments(text, vec2(x, y), size, color));
    };
    match game.phase {
        Phase::Intermission { timer } => {
            banner(
                &format!("WAVE {}", game.wave),
                viewport.y * 0.62,
                44.0,
                cyan,
            );
            if timer % 0.8 < 0.55 {
                banner("GET READY", viewport.y * 0.62 - 40.0, 18.0, lime);
            }
        }
        Phase::GameOver => {
            banner("GAME OVER", viewport.y * 0.62, 44.0, red);
            banner(
                &format!("WAVE {} - SCORE {}", game.wave, game.score),
                viewport.y * 0.62 - 40.0,
                20.0,
                lime,
            );
            banner("PRESS R TO RESTART", viewport.y * 0.62 - 78.0, 16.0, cyan);
        }
        Phase::Fighting => {}
    }
    out
}

// ------------------------------------------------------------------- app --

struct Renderers {
    world_camera: CameraBinding,
    weapon_camera: CameraBinding,
    hud_camera: CameraBinding,
    world_lines: LineRenderer,
    dynamic_lines: LineRenderer,
    world_occluders: OccluderRenderer,
    dynamic_occluders: OccluderRenderer,
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
        Self {
            world_lines: LineRenderer::new(device, HDR_FORMAT, &world_camera),
            dynamic_lines: LineRenderer::new(device, HDR_FORMAT, &world_camera),
            world_occluders: OccluderRenderer::new(device, &world_camera),
            dynamic_occluders: OccluderRenderer::new(device, &world_camera),
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

struct ArenaApp {
    scene: BakedScene,
    soup: TriangleSoup,
    player: FpsController,
    weapon: Option<Weapon>,
    models: GameModels,
    game: Game,
    renderers: Option<Renderers>,
    world_uploaded: bool,
    time: f32,
    post_settings: PostSettings,
    /// Created on the first captured click — that user gesture is exactly
    /// what browser autoplay policies require before audio may start.
    audio: Option<AudioEngine>,
    audio_failed: bool,
}

impl ArenaApp {
    #[cfg(not(target_arch = "wasm32"))]
    fn load_content() -> Result<(BakedScene, GameModels)> {
        let root = asset_root();
        let scene = vex_engine::load_scene(&root.join(SCENE_PATH))
            .with_context(|| format!("load scene {SCENE_PATH}"))?;
        let models = GameModels {
            shard: VecModel::load(&root.join(SHARD_PATH)).context("load shard.vec")?,
            sentinel: VecModel::load(&root.join(SENTINEL_PATH)).context("load sentinel.vec")?,
            healthpack: VecModel::load(&root.join(HEALTHPACK_PATH))
                .context("load healthpack.vec")?,
        };
        Ok((scene, models))
    }

    #[cfg(target_arch = "wasm32")]
    fn load_content() -> Result<(BakedScene, GameModels)> {
        let scene = vex_engine::load_scene_from_str(embedded::SCENE, |reference| {
            Ok(match reference {
                "arena.vec" => VecModel::load_from(embedded::ARENA)?,
                "../pistol.vec" => VecModel::load_from(embedded::PISTOL)?,
                other => anyhow::bail!("no embedded model for '{other}'"),
            })
        })?;
        let models = GameModels {
            shard: VecModel::load_from(embedded::SHARD).context("shard.vec")?,
            sentinel: VecModel::load_from(embedded::SENTINEL).context("sentinel.vec")?,
            healthpack: VecModel::load_from(embedded::HEALTHPACK).context("healthpack.vec")?,
        };
        Ok((scene, models))
    }

    fn new() -> Result<Self> {
        let (scene, models) = Self::load_content()?;
        let soup = TriangleSoup::new(
            &scene.occluder_vertices,
            &scene.occluder_indices,
            COLLISION_CELL,
        );
        let player = FpsController::new(scene.player_spawn, scene.player_yaw);
        let weapon = scene.weapon.clone().map(Weapon::new);
        log::info!(
            "arena: {} static segments · {} collision triangles",
            scene.segments.len(),
            soup.triangle_count(),
        );
        Ok(Self {
            post_settings: scene.post,
            scene,
            soup,
            player,
            weapon,
            models,
            game: Game::new(),
            renderers: None,
            world_uploaded: false,
            time: 0.0,
            audio: None,
            audio_failed: false,
        })
    }

    fn ensure_audio(&mut self) {
        if self.audio.is_some() || self.audio_failed {
            return;
        }
        match AudioEngine::new() {
            Ok(audio) => self.audio = Some(audio),
            Err(err) => {
                log::warn!("audio disabled: {err:#}");
                self.audio_failed = true;
            }
        }
    }

    fn play_events(&mut self, events: Vec<GameEvent>) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        audio.set_listener(self.player.eye(), self.player.rotation());
        for event in events {
            match event {
                GameEvent::Shot => audio.play(Sfx::Shot),
                GameEvent::BoltFired(at) => audio.play_at(Sfx::BoltFire, at),
                GameEvent::BoltImpact(at) => audio.play_at(Sfx::BoltImpact, at),
                GameEvent::EnemyDied(at) => audio.play_at(Sfx::EnemyDeath, at),
                GameEvent::PlayerHit => audio.play(Sfx::PlayerHit),
                GameEvent::WaveStarted(_) => audio.play(Sfx::WaveStart),
                GameEvent::GameOver => audio.play(Sfx::GameOver),
                GameEvent::HealthSpawned(at) => audio.play_at(Sfx::HealthSpawn, at),
                GameEvent::HealthPicked => audio.play(Sfx::HealthPickup),
            }
        }
    }

    /// Full look direction (includes pitch) — the hitscan ray.
    fn aim(&self) -> Vec3 {
        self.player.rotation() * Vec3::NEG_Z
    }

    /// World-space point that projects to the same screen position as the
    /// viewmodel's barrel tip. The weapon layer renders with its own FOV,
    /// so the weapon-space MUZZLE can't be used directly: scaling x/y by
    /// the ratio of the two FOV tangents makes both cameras agree on where
    /// the point sits on screen — tracers then leave the drawn barrel.
    fn muzzle_world(&self) -> Vec3 {
        let fov_scale = (self.player.fov_y * 0.5).tan()
            / ((WEAPON_FOV_DEG.to_radians()) * 0.5).tan();
        // Match the flash at full recoil kick (the frame the shot fires).
        let m = MUZZLE + vec3(0.0, 0.02, 0.10);
        let view_point = vec3(m.x * fov_scale, m.y * fov_scale, m.z);
        self.player.eye() + self.player.rotation() * view_point
    }

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

        let amp = self.game.damage_flash * 0.05;
        let eye = self.player.eye()
            + vec3(
                (self.time * 57.0).sin() * amp,
                (self.time * 71.0).sin() * amp,
                0.0,
            );
        let view = glam::camera::rh::view::look_to_mat4(
            eye,
            self.player.rotation() * Vec3::NEG_Z,
            Vec3::Y,
        );
        let proj = glam::camera::rh::proj::directx::perspective(
            self.player.fov_y,
            aspect,
            0.05,
            300.0,
        );
        let view_proj = proj * view;

        let frustum = Frustum::from_view_proj(view_proj);
        let mut segment_ranges = Vec::new();
        let mut occluder_ranges = Vec::new();
        for instance in &self.scene.instances {
            if frustum.intersects_aabb(instance.aabb_min, instance.aabb_max) {
                segment_ranges.push(instance.segments.clone());
                occluder_ranges.push(instance.occluder_indices.clone());
            }
        }

        let (dynamic_segments, dynamic_vertices, dynamic_indices) =
            build_dynamic(&self.models, &self.game, self.time);
        renderers.dynamic_lines.set_segments(
            &frame.gpu.device,
            &frame.gpu.queue,
            &dynamic_segments,
        );
        renderers.dynamic_occluders.set_geometry(
            &frame.gpu.device,
            &frame.gpu.queue,
            &dynamic_vertices,
            &dynamic_indices,
        );

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
        renderers
            .dynamic_occluders
            .render(&mut encoder, frame.depth, &renderers.world_camera, false);
        renderers.world_lines.render_ranges(
            &mut encoder,
            hdr,
            frame.depth,
            &renderers.world_camera,
            true,
            false,
            &segment_ranges,
        );
        renderers.dynamic_lines.render(
            &mut encoder,
            hdr,
            frame.depth,
            &renderers.world_camera,
            false,
            false,
        );

        if let Some(weapon) = &self.weapon {
            let (mut segments, vertices, indices) =
                weapon.frame_geometry(self.player.bob_phase(), self.game.recoil());
            // Muzzle flash lives in the weapon layer, pinned to the barrel
            // tip, so it never smears across the world near the camera.
            segments.extend(muzzle_flash(self.game.recoil()));
            renderers
                .weapon_lines
                .set_segments(&frame.gpu.device, &frame.gpu.queue, &segments);
            renderers.weapon_occluders.set_geometry(
                &frame.gpu.device,
                &frame.gpu.queue,
                &vertices,
                &indices,
            );
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

        renderers.hud_lines.set_segments(
            &frame.gpu.device,
            &frame.gpu.queue,
            &hud_segments(frame.viewport, &self.game),
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

        renderers
            .post
            .run(&frame.gpu.queue, &mut encoder, frame.color, &self.post_settings);
        frame.gpu.queue.submit([encoder.finish()]);
    }
}

impl App for ArenaApp {
    fn init(&mut self, gpu: &Gpu, target_format: wgpu::TextureFormat) {
        self.renderers = Some(Renderers::new(&gpu.device, target_format));
    }

    fn update(&mut self, dt: f32, input: &Input) {
        self.time += dt;
        if input.is_just_pressed(KeyCode::KeyC) {
            self.post_settings.crt = if self.post_settings.crt > 0.0 { 0.0 } else { 1.0 };
        }
        if matches!(self.game.phase, Phase::GameOver)
            && input.is_just_pressed(KeyCode::KeyR)
        {
            self.game.restart();
            self.player.pos = self.scene.player_spawn;
        }
        if input.is_captured() {
            self.ensure_audio();
        }
        self.player.update(dt, input, &self.soup);
        let attack = input.is_captured() && input.is_mouse_just_pressed(MouseButton::Left);
        let (eye, aim) = (self.player.eye(), self.aim());
        self.game
            .update(dt, eye, aim, self.muzzle_world(), attack, &self.soup);
        let events = std::mem::take(&mut self.game.events);
        self.play_events(events);
    }

    fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        viewport: Vec2,
    ) {
        let _ = encoder; // this app submits its own multi-camera encoder
        self.draw(&Frame {
            gpu,
            color,
            depth,
            viewport,
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(path) = flag_value(&args, "--screenshot") {
        return screenshot(Path::new(&path), &args);
    }

    println!(
        "controls: click captures · WASD + Space · LEFT CLICK fires · \
         [R] restart · [C] CRT · Esc releases"
    );
    vex_engine::run("vector3d — arena", ArenaApp::new()?)
}

/// Web entry: wasm-bindgen invokes `main` when the module loads. The event
/// loop is spawned (not blocked on) and this returns to the browser.
#[cfg(target_arch = "wasm32")]
fn main() -> Result<()> {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Warn).ok();
    vex_engine::run("vector3d — arena", ArenaApp::new()?)
}

#[cfg(not(target_arch = "wasm32"))]
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_vec3(value: &str) -> Result<Vec3> {
    let parts: Vec<f32> = value
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .context("expected x,y,z")?;
    anyhow::ensure!(parts.len() == 3, "expected exactly three components");
    Ok(vec3(parts[0], parts[1], parts[2]))
}

/// Headless verification: optionally simulate `--demo T` seconds of the
/// fight (deterministic RNG, player standing at spawn), pose the gun
/// with `--recoil 0..1`, then render one frame.
#[cfg(not(target_arch = "wasm32"))]
fn screenshot(out: &Path, args: &[String]) -> Result<()> {
    let (width, height) = match flag_value(args, "--size") {
        Some(raw) => {
            let (w, h) = raw.split_once('x').context("--size expects WxH")?;
            (w.parse()?, h.parse()?)
        }
        None => (1280, 720),
    };

    let mut app = ArenaApp::new()?;
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
    if let Some(demo) = flag_value(args, "--demo") {
        let seconds: f32 = demo.parse().context("--demo expects seconds")?;
        let (eye, aim) = (app.player.eye(), app.aim());
        let steps = (seconds * 60.0) as usize;
        for i in 0..steps {
            app.time += 1.0 / 60.0;
            // Fire periodically so demo shots show combat.
            let attack = i % 20 == 0;
            app.game
                .update(1.0 / 60.0, eye, aim, app.muzzle_world(), attack, &app.soup);
        }
    }
    if let Some(recoil) = flag_value(args, "--recoil") {
        let t: f32 = recoil.parse().context("--recoil expects 0..1")?;
        app.game.force_recoil(t);
    }
    if let Some(pack) = flag_value(args, "--pack") {
        let age: f32 = pack.parse().context("--pack expects an age in seconds")?;
        app.game.force_health_pack(age);
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
