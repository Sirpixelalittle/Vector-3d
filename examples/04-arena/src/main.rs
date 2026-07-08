//! Arena: the playable demo. Waves of enemies pour out of the gates of a
//! neon fight pit; cut them down with the pistol. Everything on screen
//! is the engine's stroke pipeline — enemies, particles, banners included.
//!
//! Windowed:    cargo run -p arena
//! Headless:    cargo run -p arena -- --screenshot out.png [--size WxH]
//!                  [--demo SECONDS] [--recoil 0..1] [--pos x,y,z]
//!                  [--yaw DEG] [--pitch DEG] [--wave N] [--pack AGE]
//!                  [--menu [--options]]
//!
//! Boots to a start screen (point and click: Play / Options with a mouse
//! sensitivity slider / Quit). In the arena: WASD moves · SPACE/SHIFT
//! dashes (10 s cooldown) · left click fires · \[R\] restarts · \[C\] CRT ·
//! Esc releases the mouse.

mod game;
mod menu;
mod sounds;

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec2, Vec3, Vec4, vec2, vec3};
use game::{EnemyKind, Game, GameEvent, Phase};
use sounds::Sounds;
use vex_audio::AudioEngine;
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
#[cfg(not(target_arch = "wasm32"))]
const BOSS_TOP_PATH: &str = "assets/arena/boss_top.vec";
#[cfg(not(target_arch = "wasm32"))]
const BOSS_BOTTOM_PATH: &str = "assets/arena/boss_bottom.vec";

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

/// Every asset baked into the executable: the browser has no filesystem,
/// and a bare native binary copied anywhere still runs — a real
/// `assets/` directory next to the executable (or in the working
/// directory) takes precedence, so packaged builds stay moddable.
mod embedded {
    pub const SCENE: &str = include_str!("../../../assets/arena/scene.ron");
    pub const ARENA: &[u8] = include_bytes!("../../../assets/arena/arena.vec");
    pub const SHARD: &[u8] = include_bytes!("../../../assets/arena/shard.vec");
    pub const SENTINEL: &[u8] = include_bytes!("../../../assets/arena/sentinel.vec");
    pub const HEALTHPACK: &[u8] = include_bytes!("../../../assets/arena/healthpack.vec");
    pub const BOSS_TOP: &[u8] = include_bytes!("../../../assets/arena/boss_top.vec");
    pub const BOSS_BOTTOM: &[u8] = include_bytes!("../../../assets/arena/boss_bottom.vec");
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
    let len = 0.05 + 0.08 * k;
    // White-hot and strongly overbright: it lives for a couple of frames,
    // so it can afford to bloom hard.
    let color = Vec4::new(1.0, 0.93, 0.66, 2.6 + 1.6 * k);
    // Ride the same recoil kick the viewmodel uses, so the flash stays
    // glued to the barrel as the gun jumps.
    let origin = MUZZLE + vec3(0.0, 0.02 * recoil, 0.10 * recoil);
    const SPOKES: usize = 8;
    let mut out: Vec<Segment> = (0..SPOKES)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / SPOKES as f32;
            // Forward-biased so it reads as blowing out of the barrel.
            let dir = vec3(a.cos() * len, a.sin() * len, -len * 0.5);
            Segment::new(origin, origin + dir, color)
        })
        .collect();
    // A hot jet straight out of the bore.
    out.push(Segment::new(
        origin,
        origin + vec3(0.0, 0.0, -len * 2.2),
        Vec4::new(1.0, 0.98, 0.85, 3.2 + 1.6 * k),
    ));
    out
}

// --------------------------------------------------------------- enemies --

struct GameModels {
    shard: VecModel,
    sentinel: VecModel,
    healthpack: VecModel,
    boss_top: VecModel,
    boss_bottom: VecModel,
}

impl GameModels {
    fn get(&self, kind: EnemyKind) -> &VecModel {
        match kind {
            EnemyKind::Shard => &self.shard,
            EnemyKind::Sentinel => &self.sentinel,
            // The boss renders as two composed models, not through here.
            EnemyKind::Boss => &self.boss_bottom,
        }
    }
}

/// Enemy bolts as tiny wire darts — nose and tail spikes on a small
/// square mid-ring, oriented along the velocity, with a fading trail.
/// A streak-along-velocity vanishes to a dot exactly when a bolt flies
/// at your face; the dart's ring stays visible head-on (Tempest drew
/// its shots as little shapes for the same reason).
fn bolt_dart(bolt: &game::Bolt) -> impl Iterator<Item = Segment> {
    const HALF_LEN: f32 = 0.30;
    const GIRTH: f32 = 0.075;
    const TRAIL: f32 = 0.5;
    // Lazy roll around the flight axis (radians/sec) — the square ring
    // glints as its corners sweep past, like a thrown dart drifting.
    const SPIN_RATE: f32 = 2.5;
    let fwd = bolt.vel.normalize_or_zero();
    let reference = if fwd.y.abs() > 0.9 { Vec3::X } else { Vec3::Y };
    let base_right = fwd.cross(reference).normalize_or_zero() * GIRTH;
    let base_up = base_right.cross(fwd).normalize_or_zero() * GIRTH;
    // `life` ticks down every frame, so it doubles as the spin clock — no
    // new bolt state. Bolts from one volley share a life and roll in sync.
    let (s, c) = (bolt.life * SPIN_RATE).sin_cos();
    let right = base_right * c + base_up * s;
    let up = base_up * c - base_right * s;
    let (nose, tail) = (bolt.pos + fwd * HALF_LEN, bolt.pos - fwd * HALF_LEN);
    let ring = [
        bolt.pos + right,
        bolt.pos + up,
        bolt.pos - right,
        bolt.pos - up,
    ];
    let color = bolt.color;
    let trail_color = Vec4::new(color.x, color.y, color.z, color.w * 0.45);
    let mut out = Vec::with_capacity(13);
    for i in 0..4 {
        out.push(Segment::new(nose, ring[i], color));
        out.push(Segment::new(tail, ring[i], color));
        out.push(Segment::new(ring[i], ring[(i + 1) % 4], color));
    }
    out.push(Segment::new(tail, tail - fwd * TRAIL, trail_color));
    out.into_iter()
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

        if enemy.kind == EnemyKind::Boss {
            // Two composed models: the base, and the crown riding the
            // attack cycle — rising, spinning, and burning hotter while
            // the core is exposed. Sealed during the entrance march.
            let (openness, spin) = enemy.crown();
            let intensity =
                (0.25 + 0.75 * progress) * (1.0 + enemy.hit_flash * 2.0 + openness * 0.7);
            let base = Mat4::from_translation(enemy.center())
                * Mat4::from_rotation_y(enemy.yaw)
                * Mat4::from_scale(Vec3::splat(scale));
            let crown = base
                * Mat4::from_translation(Vec3::Y * (openness * game::BOSS_RISE))
                * Mat4::from_rotation_y(spin);
            for (model, transform) in
                [(&models.boss_bottom, base), (&models.boss_top, crown)]
            {
                segments.extend(
                    model
                        .edge_segments(EdgeKind::Always, intensity)
                        .into_iter()
                        .map(|s| Segment {
                            a: transform.transform_point3(s.a),
                            b: transform.transform_point3(s.b),
                            ..s
                        }),
                );
                if progress >= 1.0 {
                    let start = vertices.len() as u32;
                    vertices.extend(model.vertices.iter().map(|&v| transform.transform_point3(v)));
                    indices.extend(model.occluder_indices.iter().map(|&i| i + start));
                }
            }
            if progress < 1.0 {
                segments.extend(telegraph_ring(enemy.pos, enemy.kind, progress));
            }
            continue;
        }

        // Shards tumble freely; sentinels aim their eye at the player.
        let yaw = match enemy.kind {
            EnemyKind::Shard => enemy.age * 1.4 + enemy.wobble,
            EnemyKind::Sentinel | EnemyKind::Boss => enemy.yaw,
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
        segments.extend(bolt_dart(bolt));
    }

    // Player slugs: tiny white-hot streaks, a fraction of a dart's size
    // and colorless like the gun — whose shot is whose reads instantly.
    const BULLET_STREAK: f32 = 0.22;
    for bullet in &game.bullets {
        let dir = bullet.vel.normalize_or_zero();
        segments.push(Segment::new(
            bullet.pos - dir * BULLET_STREAK,
            bullet.pos,
            Vec4::new(1.0, 0.97, 0.88, 2.2),
        ));
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

/// Screen-edge threat bands: three nested border lines per warned edge,
/// dim and additive — bloom melts them into a soft glow the color of the
/// incoming fire. Left/right edges mean fire from that side; the bottom
/// band means behind you (bolts ahead are their own indicator).
fn threat_segments(viewport: Vec2, threats: &game::ThreatEdges) -> Vec<Segment> {
    // Inset per band and its share of the edge's urgency: the outermost
    // line burns hottest, the inner ones feather the glow inward.
    const BANDS: [(f32, f32); 3] = [(4.0, 0.55), (11.0, 0.33), (18.0, 0.18)];
    let mut out = Vec::new();
    let (w, h) = (viewport.x, viewport.y);
    for (edge, warn) in threats.0.iter().enumerate() {
        if warn.w < 0.02 {
            continue;
        }
        for (inset, share) in BANDS {
            let color = Vec4::new(warn.x, warn.y, warn.z, warn.w * share);
            let (a, b) = match edge {
                // Left / right: a vertical band spanning mid-screen.
                0 => (vec3(inset, h * 0.16, 0.0), vec3(inset, h * 0.84, 0.0)),
                1 => (
                    vec3(w - inset, h * 0.16, 0.0),
                    vec3(w - inset, h * 0.84, 0.0),
                ),
                // Behind: along the bottom, clear of the HUD text.
                _ => (
                    vec3(w * 0.24, inset, 0.0),
                    vec3(w * 0.76, inset, 0.0),
                ),
            };
            out.push(Segment::new(a, b, color));
        }
    }
    out
}

/// The start screen in HUD space: title block, buttons that swell and
/// brighten under the cursor, the options slider, and control hints.
/// Geometry comes from the same [`menu::layout`] the hit tests use.
fn menu_segments(state: &menu::Menu, viewport: Vec2) -> Vec<Segment> {
    let green = |w: f32| Vec4::new(0.62, 1.0, 0.68, w);
    let mut out = Vec::new();

    // Title block in the upper third.
    let title_px = (viewport.x * 0.075).clamp(40.0, 88.0);
    let title_w = font::text_width(menu::TITLE, title_px);
    out.extend(font::text_segments(
        menu::TITLE,
        vec2((viewport.x - title_w) * 0.5, viewport.y * 0.72),
        title_px,
        green(1.55),
    ));
    let sub_px = title_px * 0.36;
    let sub_w = font::text_width(menu::SUBTITLE, sub_px);
    out.extend(font::text_segments(
        menu::SUBTITLE,
        vec2((viewport.x - sub_w) * 0.5, viewport.y * 0.72 - sub_px * 2.1),
        sub_px,
        Vec4::new(1.0, 0.32, 0.22, 1.15),
    ));

    for (i, item) in menu::layout(state.page, viewport).iter().enumerate() {
        let hover = state.hover[i];
        // Hovered items grow around their center and brighten.
        let px = item.px * (1.0 + menu::HOVER_GROW * hover);
        let width = font::text_width(item.label, px);
        let origin = vec2(
            (viewport.x - width) * 0.5,
            item.origin.y - (px - item.px) * 0.5,
        );
        out.extend(font::text_segments(
            item.label,
            origin,
            px,
            green(0.7 + 1.3 * hover),
        ));

        if item.kind == menu::ItemKind::Slider {
            let (left, right) = menu::slider_track(item);
            let handle_x = left.x + (right.x - left.x) * state.sensitivity;
            let y = left.y;
            // Filled portion hot, remainder dim, diamond handle.
            out.push(Segment::new(
                vec3(left.x, y, 0.0),
                vec3(handle_x, y, 0.0),
                green(1.3),
            ));
            out.push(Segment::new(
                vec3(handle_x, y, 0.0),
                vec3(right.x, y, 0.0),
                green(0.35),
            ));
            const HANDLE: f32 = 9.0;
            let corners = [
                vec3(handle_x, y + HANDLE, 0.0),
                vec3(handle_x + HANDLE, y, 0.0),
                vec3(handle_x, y - HANDLE, 0.0),
                vec3(handle_x - HANDLE, y, 0.0),
            ];
            for k in 0..4 {
                out.push(Segment::new(corners[k], corners[(k + 1) % 4], green(1.6)));
            }
            // Live multiplier readout beside the track.
            let readout =
                format!("{:.0}%", menu::sensitivity_scale(state.sensitivity) * 100.0);
            out.extend(font::text_segments(
                &readout,
                vec2(right.x + 26.0, y - 9.0),
                18.0,
                green(0.9),
            ));
        }
    }

    // Control hints, small and dim at the bottom.
    let hint = "WASD MOVE - SPACE DASH - LMB FIRE";
    let hint_px = 13.0;
    let hint_w = font::text_width(hint, hint_px);
    out.extend(font::text_segments(
        hint,
        vec2((viewport.x - hint_w) * 0.5, 34.0),
        hint_px,
        green(0.5),
    ));
    out
}

fn hud_segments(viewport: Vec2, game: &Game, dash_ready: f32) -> Vec<Segment> {
    let red = Vec4::new(phosphor::RED.x, phosphor::RED.y, phosphor::RED.z, 0.95);
    let lime = Vec4::new(phosphor::LIME.x, phosphor::LIME.y, phosphor::LIME.z, 0.9);
    let cyan = Vec4::new(phosphor::CYAN.x, phosphor::CYAN.y, phosphor::CYAN.z, 1.0);

    let mut out = font::text_segments(
        &format!("HEALTH {:.0}", game.hp),
        vec2(28.0, 26.0),
        20.0,
        red,
    );
    // Dash meter: a 10-second resource needs to be legible at a glance.
    let ready = dash_ready >= 1.0;
    let dash_color = if ready {
        Vec4::new(phosphor::CYAN.x, phosphor::CYAN.y, phosphor::CYAN.z, 1.25)
    } else {
        Vec4::new(phosphor::CYAN.x, phosphor::CYAN.y, phosphor::CYAN.z, 0.45)
    };
    out.extend(font::text_segments("DASH", vec2(28.0, 58.0), 12.0, dash_color));
    let (bar_x, bar_y, bar_w) = (92.0, 63.0, 110.0);
    out.push(Segment::new(
        vec3(bar_x, bar_y, 0.0),
        vec3(bar_x + bar_w * dash_ready.clamp(0.0, 1.0), bar_y, 0.0),
        dash_color,
    ));

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

/// Which top-level view the app is showing. The menu owns the mouse as a
/// pointer (the shell doesn't capture clicks while it's up); gameplay owns
/// it as look input.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Menu,
    Playing,
}

/// Backdrop camera drift while the menu is up (radians/second).
const MENU_ORBIT_SPEED: f32 = 0.12;

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
    screen: Screen,
    menu: menu::Menu,
    menu_angle: f32,
    quit: bool,
    /// Authored default mouse sensitivity; the options slider scales it.
    sens_base: f32,
    /// Surface size from the last render — update() needs it for menu
    /// hit-testing before this frame's render runs.
    last_viewport: Vec2,
    /// Created on the first captured click — that user gesture is exactly
    /// what browser autoplay policies require before audio may start.
    audio: Option<AudioEngine>,
    audio_failed: bool,
    /// The arena's own sound bank, handed to the engine to play.
    sounds: Sounds,
}

impl ArenaApp {
    /// Load everything from the copies baked into the executable.
    fn load_embedded() -> Result<(BakedScene, GameModels)> {
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
            boss_top: VecModel::load_from(embedded::BOSS_TOP).context("boss_top.vec")?,
            boss_bottom: VecModel::load_from(embedded::BOSS_BOTTOM).context("boss_bottom.vec")?,
        };
        Ok((scene, models))
    }

    /// Native: an `assets/` directory on disk wins (repo runs, modded
    /// packages); otherwise fall back to the embedded copies so a bare
    /// executable runs from anywhere.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_content() -> Result<(BakedScene, GameModels)> {
        let root = asset_root();
        if !root.join(SCENE_PATH).exists() {
            log::info!("no assets on disk — running from embedded copies");
            return Self::load_embedded();
        }
        let scene = vex_engine::load_scene(&root.join(SCENE_PATH))
            .with_context(|| format!("load scene {SCENE_PATH}"))?;
        let models = GameModels {
            shard: VecModel::load(&root.join(SHARD_PATH)).context("load shard.vec")?,
            sentinel: VecModel::load(&root.join(SENTINEL_PATH)).context("load sentinel.vec")?,
            healthpack: VecModel::load(&root.join(HEALTHPACK_PATH))
                .context("load healthpack.vec")?,
            boss_top: VecModel::load(&root.join(BOSS_TOP_PATH)).context("load boss_top.vec")?,
            boss_bottom: VecModel::load(&root.join(BOSS_BOTTOM_PATH))
                .context("load boss_bottom.vec")?,
        };
        Ok((scene, models))
    }

    #[cfg(target_arch = "wasm32")]
    fn load_content() -> Result<(BakedScene, GameModels)> {
        Self::load_embedded()
    }

    fn new() -> Result<Self> {
        let (scene, models) = Self::load_content()?;
        let soup = TriangleSoup::new(
            &scene.occluder_vertices,
            &scene.occluder_indices,
            COLLISION_CELL,
        );
        let mut player = FpsController::new(scene.player_spawn, scene.player_yaw);
        // Arena movement: no jump, no sprint — one dash on a long
        // cooldown (community feedback: commitment over mobility).
        player.jump_enabled = false;
        player.sprint_enabled = false;
        player.dash_enabled = true;
        let weapon = scene.weapon.clone().map(Weapon::new);
        log::info!(
            "arena: {} static segments · {} collision triangles",
            scene.segments.len(),
            soup.triangle_count(),
        );
        Ok(Self {
            post_settings: scene.post,
            sens_base: player.sensitivity,
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
            sounds: Sounds::synth(),
            screen: Screen::Menu,
            menu: menu::Menu::new(),
            menu_angle: 0.0,
            quit: false,
            last_viewport: vec2(1280.0, 720.0),
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
                GameEvent::Shot => audio.play(&self.sounds.shot),
                GameEvent::BoltFired(at) => audio.play_at(&self.sounds.bolt_fire, at),
                GameEvent::BoltImpact(at) => audio.play_at(&self.sounds.bolt_impact, at),
                GameEvent::EnemyDied(at) => audio.play_at(&self.sounds.enemy_death, at),
                GameEvent::PlayerHit => audio.play(&self.sounds.player_hit),
                GameEvent::WaveStarted(_) => audio.play(&self.sounds.wave_start),
                GameEvent::GameOver => audio.play(&self.sounds.game_over),
                GameEvent::HealthSpawned(at) => audio.play_at(&self.sounds.health_spawn, at),
                GameEvent::HealthPicked => audio.play(&self.sounds.health_pickup),
                GameEvent::BossRing(at) => audio.play_at(&self.sounds.boss_ring, at),
                GameEvent::Ricochet(at) => audio.play_at(&self.sounds.ricochet, at),
            }
        }
    }

    /// Full look direction (includes pitch) — slugs converge onto it.
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

        match self.screen {
            Screen::Menu => self.draw_menu(frame),
            Screen::Playing => self.draw_game(frame),
        }
    }

    /// The start screen: the empty arena drifting by under big vector
    /// type. Same world passes and post chain as gameplay — the menu is
    /// just different segments in the HUD layer.
    fn draw_menu(&mut self, frame: &Frame) {
        let Some(renderers) = self.renderers.as_mut() else {
            return;
        };
        let aspect = frame.viewport.x / frame.viewport.y;
        // Tilted slightly upward so the busy center-floor pattern sinks
        // to the bottom of the frame; the labels sit over the calmer
        // mid-wall lines instead.
        let eye = vec3(
            self.menu_angle.cos() * 15.5,
            8.0,
            self.menu_angle.sin() * 15.5,
        );
        let target = vec3(0.0, 7.0, 0.0);
        let view =
            glam::camera::rh::view::look_to_mat4(eye, (target - eye).normalize(), Vec3::Y);
        let proj =
            glam::camera::rh::proj::directx::perspective(60f32.to_radians(), aspect, 0.05, 300.0);
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

        renderers.hud_lines.set_segments(
            &frame.gpu.device,
            &frame.gpu.queue,
            &menu_segments(&self.menu, frame.viewport),
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

    fn draw_game(&mut self, frame: &Frame) {
        // Computed before the renderers borrow (aim() takes &self).
        let threats = game::threat_edges(self.player.eye(), self.aim(), &self.game.bolts);
        let Some(renderers) = self.renderers.as_mut() else {
            return;
        };
        let aspect = frame.viewport.x / frame.viewport.y;
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
            &{
                let mut hud =
                    hud_segments(frame.viewport, &self.game, self.player.dash_ready_fraction());
                hud.extend(threat_segments(frame.viewport, &threats));
                hud
            },
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
        match self.screen {
            Screen::Menu => self.update_menu(dt, input),
            Screen::Playing => self.update_game(dt, input),
        }
    }

    fn wants_capture(&self) -> bool {
        matches!(self.screen, Screen::Playing)
    }

    fn should_quit(&self) -> bool {
        self.quit
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
        self.last_viewport = viewport;
        self.draw(&Frame {
            gpu,
            color,
            depth,
            viewport,
        });
    }
}

impl ArenaApp {
    fn update_game(&mut self, dt: f32, input: &Input) {
        self.time += dt;
        if input.is_just_pressed(KeyCode::KeyC) {
            self.post_settings.crt = if self.post_settings.crt > 0.0 { 0.0 } else { 1.0 };
        }
        if matches!(self.game.phase, Phase::GameOver)
            && input.is_just_pressed(KeyCode::KeyR)
        {
            self.game.restart();
            self.player.pos = self.scene.player_spawn;
            self.player.yaw = self.scene.player_yaw;
            self.player.pitch = 0.0;
        }
        if input.is_captured() {
            self.ensure_audio();
        }
        // Death freezes the player with the rest of the world — the game
        // over screen is a freeze-frame, not a ghost tour.
        if !matches!(self.game.phase, Phase::GameOver) {
            self.player.update(dt, input, &self.soup);
        }
        if self.player.just_dashed()
            && let Some(audio) = self.audio.as_mut()
        {
            audio.play(&self.sounds.dash);
        }
        let attack = input.is_captured() && input.is_mouse_just_pressed(MouseButton::Left);
        let (eye, aim) = (self.player.eye(), self.aim());
        self.game
            .update(dt, eye, aim, self.muzzle_world(), attack, &self.soup);
        let events = std::mem::take(&mut self.game.events);
        self.play_events(events);
    }

    fn update_menu(&mut self, dt: f32, input: &Input) {
        self.time += dt;
        self.menu_angle += dt * MENU_ORBIT_SPEED;
        // Window cursor is y-down; the menu lives in HUD space (y-up).
        let cursor = input.cursor_position();
        let cursor = vec2(cursor.x, self.last_viewport.y - cursor.y);
        let click = input.is_mouse_just_pressed(MouseButton::Left);
        let held = input.is_mouse_down(MouseButton::Left);
        match self.menu.update(dt, self.last_viewport, cursor, click, held) {
            menu::Action::Play => self.screen = Screen::Playing,
            menu::Action::Quit => self.quit = true,
            menu::Action::None => {}
        }
        // The slider applies live, so the new aim feel is there the moment
        // you step into the arena.
        self.player.sensitivity =
            self.sens_base * menu::sensitivity_scale(self.menu.sensitivity);
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
        "controls: click captures · WASD moves · SPACE/SHIFT dashes · \
         LEFT CLICK fires · [R] restart · [C] CRT · Esc releases"
    );
    let mut app = ArenaApp::new()?;
    if let Some(wave) = flag_value(&args, "--wave") {
        let wave: u32 = wave.parse().context("--wave expects a number")?;
        app.game.jump_to_wave(wave);
        println!("jumping to wave {wave}");
    }
    vex_engine::run("vector3d — arena", app)
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
    // Screenshots default to gameplay so existing flags keep working;
    // `--menu` (optionally with `--options`) shoots the start screen,
    // first item hovered so the grow/brighten styling is visible.
    if args.iter().any(|a| a == "--menu") {
        app.menu.hover[0] = 1.0;
        if args.iter().any(|a| a == "--options") {
            app.menu.page = menu::Page::Options;
        }
    } else {
        app.screen = Screen::Playing;
    }
    if let Some(wave) = flag_value(args, "--wave") {
        app.game.jump_to_wave(wave.parse().context("--wave expects a number")?);
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
