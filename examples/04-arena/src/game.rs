//! Arena wave-fight rules: enemies, waves, gunplay, enemy fire, particles.
//! Pure simulation — no rendering, no GPU types — so everything here is
//! unit-testable. `main.rs` turns this state into segments.
//!
//! The world is the same [`TriangleSoup`] the player collides with:
//! enemies capsule-slide around pillars, bolts splash on geometry, the
//! pistol's hitscan stops at walls, and enemies hold fire without line of
//! sight. Cover works the same for both sides.

use glam::{Vec3, Vec4, vec3, vec4};
use vex_engine::TriangleSoup;
use vex_engine::collide::slide_capsule;

pub const PLAYER_MAX_HP: f32 = 100.0;
const PLAYER_HIT_RADIUS: f32 = 0.55;
/// Grace period after a melee (contact) hit — projectiles ignore it, so
/// standing in the open under fire still kills you.
const IFRAME_SECONDS: f32 = 0.7;

// --- the pistol (hitscan) ---
const FIRE_COOLDOWN: f32 = 0.24;
pub const RECOIL_SECONDS: f32 = 0.22;
const GUN_DAMAGE: f32 = 24.0;
const GUN_RANGE: f32 = 70.0;
/// Ray-to-center slack, so aiming near an enemy connects (crosshair feel).
const AIM_ASSIST: f32 = 0.4;
const GUN_KNOCKBACK: f32 = 0.6;

// --- enemy bolts ---
const BOLT_HIT_RADIUS: f32 = 0.5;
const BOLT_RANGE: f32 = 48.0;
/// Enemies hold fire closer than this (rushers should melee, not shoot).
const ENEMY_FIRE_MIN_RANGE: f32 = 4.5;

const INTERMISSION_SECONDS: f32 = 3.0;
const SPAWN_INTERVAL: f32 = 0.7;
/// Seconds an enemy spends materializing (no collision, ghost render).
pub const SPAWN_RAMP: f32 = 0.8;
const SPAWN_RADIUS: f32 = 24.0;
const GATE_ANGLES_DEG: [f32; 4] = [0.0, 90.0, 180.0, 270.0];

const SEPARATION_PUSH: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyKind {
    Shard,
    Sentinel,
}

impl EnemyKind {
    pub fn max_hp(self) -> f32 {
        match self {
            Self::Shard => 30.0,
            Self::Sentinel => 100.0,
        }
    }

    pub fn speed(self) -> f32 {
        match self {
            Self::Shard => 3.4,
            Self::Sentinel => 1.5,
        }
    }

    pub fn radius(self) -> f32 {
        match self {
            Self::Shard => 0.55,
            Self::Sentinel => 0.9,
        }
    }

    pub fn contact_damage(self) -> f32 {
        match self {
            Self::Shard => 8.0,
            Self::Sentinel => 16.0,
        }
    }

    /// Height of the body center above the floor.
    pub fn hover_height(self) -> f32 {
        match self {
            Self::Shard => 1.1,
            Self::Sentinel => 1.0,
        }
    }

    /// Collision capsule height (feet at the ground, top of the body).
    fn capsule_height(self) -> f32 {
        self.hover_height() + self.radius()
    }

    pub fn color(self) -> Vec4 {
        match self {
            Self::Shard => vec4(1.0, 0.15, 0.9, 1.0),
            Self::Sentinel => vec4(1.0, 0.55, 0.05, 1.0),
        }
    }

    fn score(self) -> u32 {
        match self {
            Self::Shard => 10,
            Self::Sentinel => 30,
        }
    }

    /// Seconds between shots at a given wave, or `None` if this enemy does
    /// not shoot yet. Sentinels are the primary shooters from wave 1;
    /// shards start taking pot-shots from wave 3. Both fire faster on
    /// later waves — the core "each wave gets harder" lever.
    pub fn fire_interval(self, wave: u32) -> Option<f32> {
        let w = wave as f32;
        match self {
            Self::Sentinel => Some((2.8 - 0.18 * w).max(0.9)),
            Self::Shard if wave >= 3 => Some((4.0 - 0.15 * w).max(1.8)),
            Self::Shard => None,
        }
    }
}

/// Per-wave bolt strength — faster, harder-hitting shots as waves climb.
fn bolt_speed(wave: u32) -> f32 {
    (11.0 + 0.8 * wave as f32).min(24.0)
}

fn bolt_damage(wave: u32) -> f32 {
    (7.0 + 1.3 * wave as f32).min(22.0)
}

#[derive(Debug)]
pub struct Enemy {
    pub kind: EnemyKind,
    /// Ground position (y = 0 plane); the body hovers above it.
    pub pos: Vec3,
    pub yaw: f32,
    pub hp: f32,
    pub age: f32,
    pub hit_flash: f32,
    pub wobble: f32,
    fire_cooldown: f32,
    /// Steering hysteresis: which side (±1) this enemy last swerved to
    /// avoid an obstacle; 0 when the way ahead is clear.
    avoid: f32,
}

impl Enemy {
    /// 0 → 1 while materializing at a spawn gate.
    pub fn spawn_progress(&self) -> f32 {
        (self.age / SPAWN_RAMP).min(1.0)
    }

    pub fn center(&self) -> Vec3 {
        let bob = if self.kind == EnemyKind::Shard {
            (self.age * 3.0 + self.wobble).sin() * 0.15
        } else {
            0.0
        };
        self.pos + Vec3::Y * (self.kind.hover_height() + bob)
    }
}

/// An enemy projectile travelling toward where the player was when fired —
/// no homing, so strafing dodges it.
#[derive(Debug, Clone, Copy)]
pub struct Bolt {
    pub pos: Vec3,
    pub vel: Vec3,
    pub life: f32,
    /// Shooter's color (hot intensity) — shard bolts magenta, sentinel amber.
    pub color: Vec4,
    damage: f32,
}

impl Bolt {
    /// Short segment along travel, for rendering.
    pub fn segment_half(&self) -> Vec3 {
        self.vel.normalize_or_zero() * 0.35
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    /// Segment half-vector: the particle renders as `pos ± axis`.
    pub axis: Vec3,
    pub color: Vec4,
    pub life: f32,
    pub max_life: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// Breather before `wave` begins.
    Intermission { timer: f32 },
    Fighting,
    GameOver,
}

/// Things that just happened, for presentation layers (audio, and later
/// maybe rumble/score popups). Drained by the caller each frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GameEvent {
    Shot,
    BoltFired(Vec3),
    BoltImpact(Vec3),
    EnemyDied(Vec3),
    PlayerHit,
    WaveStarted(u32),
    GameOver,
}

pub struct Game {
    pub wave: u32,
    pub score: u32,
    pub hp: f32,
    pub phase: Phase,
    pub enemies: Vec<Enemy>,
    pub bolts: Vec<Bolt>,
    pub particles: Vec<Particle>,
    /// Set on the frame the player takes damage (drives shake/flash).
    pub damage_flash: f32,
    pub iframes: f32,
    /// Drain with `std::mem::take` each frame.
    pub events: Vec<GameEvent>,
    recoil: f32,
    fire_cooldown: f32,
    spawn_queue: Vec<EnemyKind>,
    spawn_timer: f32,
    rng: Lcg,
}

impl Game {
    pub fn new() -> Self {
        Self {
            wave: 1,
            score: 0,
            hp: PLAYER_MAX_HP,
            phase: Phase::Intermission {
                timer: INTERMISSION_SECONDS,
            },
            enemies: Vec::new(),
            bolts: Vec::new(),
            particles: Vec::new(),
            damage_flash: 0.0,
            iframes: 0.0,
            events: Vec::new(),
            recoil: 0.0,
            fire_cooldown: 0.0,
            spawn_queue: Vec::new(),
            spawn_timer: 0.0,
            rng: Lcg(0x9E37_79B9_7F4A_7C15),
        }
    }

    pub fn restart(&mut self) {
        *self = Self::new();
    }

    /// Recoil amount, 1 → 0 over `RECOIL_SECONDS` after each shot.
    pub fn recoil(&self) -> f32 {
        self.recoil.max(0.0)
    }

    /// Pose the gun mid-recoil (headless screenshots; native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn force_recoil(&mut self, amount: f32) {
        self.recoil = amount.clamp(0.0, 1.0);
    }

    /// `muzzle` is the world-space point where the viewmodel's barrel tip
    /// appears on screen (computed by the renderer) — tracers start there
    /// so they visually leave the gun, not the screen center.
    pub fn update(
        &mut self,
        dt: f32,
        eye: Vec3,
        aim: Vec3,
        muzzle: Vec3,
        attack: bool,
        soup: &TriangleSoup,
    ) {
        self.damage_flash = (self.damage_flash - dt * 2.5).max(0.0);
        self.iframes = (self.iframes - dt).max(0.0);
        self.fire_cooldown = (self.fire_cooldown - dt).max(0.0);
        self.recoil = (self.recoil - dt / RECOIL_SECONDS).max(0.0);
        self.update_particles(dt);
        self.update_bolts(dt, eye, soup);

        match self.phase {
            Phase::GameOver => return,
            Phase::Intermission { timer } => {
                let timer = timer - dt;
                if timer <= 0.0 {
                    self.spawn_queue = compose_wave(self.wave);
                    self.spawn_timer = 0.0;
                    self.phase = Phase::Fighting;
                    self.events.push(GameEvent::WaveStarted(self.wave));
                } else {
                    self.phase = Phase::Intermission { timer };
                }
            }
            Phase::Fighting => {
                self.update_spawning(dt);
            }
        }

        if attack {
            self.try_fire(eye, aim, muzzle, soup);
        }
        self.update_enemies(dt, eye, soup);

        if matches!(self.phase, Phase::Fighting)
            && self.spawn_queue.is_empty()
            && self.enemies.is_empty()
        {
            self.wave += 1;
            self.phase = Phase::Intermission {
                timer: INTERMISSION_SECONDS,
            };
        }
        if self.hp <= 0.0 {
            self.hp = 0.0;
            self.phase = Phase::GameOver;
            self.events.push(GameEvent::GameOver);
        }
    }

    fn update_spawning(&mut self, dt: f32) {
        if self.spawn_queue.is_empty() {
            return;
        }
        self.spawn_timer -= dt;
        if self.spawn_timer > 0.0 {
            return;
        }
        self.spawn_timer = SPAWN_INTERVAL;
        let kind = self.spawn_queue.remove(0);
        let gate = GATE_ANGLES_DEG[(self.rng.next_f32() * 4.0) as usize % 4];
        let angle = (gate + (self.rng.next_f32() - 0.5) * 24.0).to_radians();
        let pos = vec3(SPAWN_RADIUS * angle.cos(), 0.0, SPAWN_RADIUS * angle.sin());
        // Stagger initial fire so a fresh wave doesn't volley in unison.
        let fire_cooldown = kind
            .fire_interval(self.wave)
            .map_or(f32::INFINITY, |interval| interval * (0.5 + self.rng.next_f32()));
        self.enemies.push(Enemy {
            kind,
            pos,
            yaw: 0.0,
            hp: kind.max_hp(),
            age: 0.0,
            hit_flash: 0.0,
            wobble: self.rng.next_f32() * std::f32::consts::TAU,
            fire_cooldown,
            avoid: 0.0,
        });
    }

    /// Fire the pistol: hitscan along `aim`, damaging the nearest enemy the
    /// ray passes through (within aim assist). The ray stops at world
    /// geometry — pillars are cover for both sides. Always kicks recoil +
    /// muzzle flash so shots feel real even on a miss.
    fn try_fire(&mut self, eye: Vec3, aim: Vec3, muzzle: Vec3, soup: &TriangleSoup) {
        if self.fire_cooldown > 0.0 || matches!(self.phase, Phase::GameOver) {
            return;
        }
        self.fire_cooldown = FIRE_COOLDOWN;
        self.recoil = 1.0;
        self.events.push(GameEvent::Shot);

        let wall_dist = soup.raycast(eye, aim, GUN_RANGE).unwrap_or(GUN_RANGE);
        let mut best: Option<(usize, f32)> = None;
        for (i, enemy) in self.enemies.iter().enumerate() {
            if enemy.spawn_progress() < 1.0 {
                continue;
            }
            let to = enemy.center() - eye;
            let along = to.dot(aim);
            if along <= 0.0 || along > wall_dist {
                continue;
            }
            let miss = (to - aim * along).length();
            if miss < enemy.kind.radius() + AIM_ASSIST
                && best.is_none_or(|(_, d)| along < d)
            {
                best = Some((i, along));
            }
        }

        // The tracer starts at the renderer-supplied muzzle point (screen-
        // aligned with the viewmodel's flash). The muzzle *flash* itself is
        // drawn attached to the viewmodel in main.rs; no world-space sparks
        // at the camera, which just smear across the view.
        if let Some((i, along)) = best {
            let hit_point = eye + aim * along;
            tracer(&mut self.particles, muzzle, hit_point);
            spark(&mut self.particles, &mut self.rng, hit_point, 5);
            let enemy = &mut self.enemies[i];
            enemy.hp -= GUN_DAMAGE;
            enemy.hit_flash = 1.0;
            enemy.pos += aim * GUN_KNOCKBACK;
            if enemy.hp <= 0.0 {
                self.kill(i);
            }
        } else {
            let end = eye + aim * wall_dist;
            tracer(&mut self.particles, muzzle, end);
            if wall_dist < GUN_RANGE {
                spark(&mut self.particles, &mut self.rng, end, 4);
            }
        }
    }

    fn kill(&mut self, index: usize) {
        let enemy = self.enemies.swap_remove(index);
        self.score += enemy.kind.score();
        self.events.push(GameEvent::EnemyDied(enemy.center()));
        burst(
            &mut self.particles,
            &mut self.rng,
            enemy.center(),
            enemy.kind.color(),
            14,
        );
    }

    fn update_enemies(&mut self, dt: f32, eye: Vec3, soup: &TriangleSoup) {
        let player_ground = vec3(eye.x, 0.0, eye.z);
        let frozen = matches!(self.phase, Phase::GameOver);
        let wave = self.wave;
        let mut new_bolts = Vec::new();

        for i in 0..self.enemies.len() {
            let enemy = &mut self.enemies[i];
            enemy.age += dt;
            enemy.hit_flash = (enemy.hit_flash - dt * 4.0).max(0.0);
            if frozen || enemy.spawn_progress() < 1.0 {
                continue;
            }
            let to_player = player_ground - enemy.pos;
            let dir = to_player.normalize_or_zero();
            // Steer around obstacles instead of face-planting into them:
            // whisker raycasts pick a clear heading near the desired one.
            let lookahead = enemy.kind.radius() + enemy.kind.speed() * 0.9 + 0.6;
            let heading = steer(
                soup,
                enemy.center(),
                dir,
                enemy.kind.radius(),
                lookahead,
                &mut enemy.avoid,
            );
            // Capsule-vs-world slide as the backstop: resolves grazing
            // contacts and any penetration left by knockbacks last frame.
            let slid = slide_capsule(
                soup,
                enemy.pos,
                enemy.kind.radius(),
                enemy.kind.capsule_height(),
                heading * enemy.kind.speed() * dt,
            );
            enemy.pos = vec3(slid.position.x, 0.0, slid.position.z);
            // The face (and the sentinel's eye) keeps tracking the player
            // even while detouring — it is aiming, not sightseeing.
            enemy.yaw = f32::atan2(-dir.x, -dir.z);

            // Fire a bolt if this enemy shoots, the player is at range, and
            // there is a clear line of sight (no shooting into pillars).
            let distance = to_player.length();
            enemy.fire_cooldown -= dt;
            if enemy.fire_cooldown <= 0.0
                && let Some(interval) = enemy.kind.fire_interval(wave)
            {
                enemy.fire_cooldown = interval;
                let from = enemy.center();
                if distance > ENEMY_FIRE_MIN_RANGE && soup.line_of_sight(from, eye) {
                    let vel = (eye - from).normalize_or_zero() * bolt_speed(wave);
                    let tint = enemy.kind.color();
                    new_bolts.push(Bolt {
                        pos: from,
                        vel,
                        life: BOLT_RANGE / bolt_speed(wave),
                        color: vec4(tint.x, tint.y, tint.z, 1.7),
                        damage: bolt_damage(wave),
                    });
                    self.events.push(GameEvent::BoltFired(from));
                }
            }

            // Contact damage (melee), gated by iframes.
            let reach = enemy.kind.radius() + PLAYER_HIT_RADIUS;
            if self.iframes <= 0.0 && distance < reach {
                self.hp -= self.enemies[i].kind.contact_damage();
                self.iframes = IFRAME_SECONDS;
                self.damage_flash = 1.0;
                self.events.push(GameEvent::PlayerHit);
                let enemy = &mut self.enemies[i];
                let away = (enemy.pos - player_ground).normalize_or_zero();
                enemy.pos += away * 1.4;
            }
        }
        self.bolts.extend(new_bolts);

        // Pairwise separation so enemies don't merge into one blob.
        for a in 0..self.enemies.len() {
            for b in (a + 1)..self.enemies.len() {
                let (left, right) = self.enemies.split_at_mut(b);
                let (ea, eb) = (&mut left[a], &mut right[0]);
                let min_dist = ea.kind.radius() + eb.kind.radius();
                let delta = eb.pos - ea.pos;
                let dist = delta.length();
                if dist > 1e-4 && dist < min_dist {
                    let push = delta / dist * (min_dist - dist) * 0.5 * SEPARATION_PUSH * dt;
                    ea.pos -= push;
                    eb.pos += push;
                }
            }
        }
    }

    /// Advance bolts; they splash on world geometry and damage the player
    /// on contact. Projectiles ignore the contact iframe window, so focused
    /// fire in the open is lethal — but pillars are real cover.
    fn update_bolts(&mut self, dt: f32, eye: Vec3, soup: &TriangleSoup) {
        if matches!(self.phase, Phase::GameOver) {
            return;
        }
        // Sweep each step with a small skin so a frame that stops just shy
        // of a wall still registers next frame (the raycast's self-hit
        // epsilon would otherwise open a tunneling window).
        const BOLT_SKIN: f32 = 0.02;
        let mut damage = 0.0;
        let mut i = 0;
        while i < self.bolts.len() {
            let bolt = self.bolts[i];
            let step = bolt.vel * dt;
            let step_len = step.length();
            if step_len > 1e-6
                && let Some(t) = soup.raycast(bolt.pos, step / step_len, step_len + BOLT_SKIN)
            {
                let impact = bolt.pos + step / step_len * t.min(step_len);
                spark(&mut self.particles, &mut self.rng, impact, 4);
                self.events.push(GameEvent::BoltImpact(impact));
                self.bolts.swap_remove(i);
                continue;
            }
            let bolt = &mut self.bolts[i];
            bolt.pos += step;
            bolt.life -= dt;
            if (bolt.pos - eye).length() < BOLT_HIT_RADIUS {
                damage += bolt.damage;
                self.bolts.swap_remove(i);
                continue;
            }
            if bolt.life <= 0.0 {
                self.bolts.swap_remove(i);
                continue;
            }
            i += 1;
        }
        if damage > 0.0 {
            self.hp -= damage;
            self.damage_flash = 1.0;
            self.events.push(GameEvent::PlayerHit);
        }
    }

    fn update_particles(&mut self, dt: f32) {
        for particle in &mut self.particles {
            particle.life -= dt;
            particle.pos += particle.vel * dt;
            particle.vel *= 0.92_f32.powf(dt * 60.0);
        }
        self.particles.retain(|p| p.life > 0.0);
    }
}

/// Raycast steering: keep the desired heading when clear; otherwise swing
/// left/right in widening steps and take the first open lane, preferring
/// the side chosen last time (`bias`) so the enemy doesn't flip-flop at a
/// pillar edge. Falls back to the desired heading when boxed in — the
/// collision slide takes over from there.
fn steer(
    soup: &TriangleSoup,
    origin: Vec3,
    desired: Vec3,
    radius: f32,
    lookahead: f32,
    bias: &mut f32,
) -> Vec3 {
    if path_clear(soup, origin, desired, radius, lookahead) {
        *bias = 0.0;
        return desired;
    }
    let preferred = if *bias >= 0.0 { 1.0 } else { -1.0 };
    for angle in [0.6, 1.2, 1.8] {
        for side in [preferred, -preferred] {
            let candidate = rotate_y(desired, angle * side);
            if path_clear(soup, origin, candidate, radius, lookahead) {
                *bias = side;
                return candidate;
            }
        }
    }
    desired
}

/// Two whisker rays offset by the body radius — a single center ray would
/// declare a lane clear while the shoulders clip the pillar corner.
fn path_clear(soup: &TriangleSoup, origin: Vec3, dir: Vec3, radius: f32, lookahead: f32) -> bool {
    let shoulder = vec3(-dir.z, 0.0, dir.x) * (radius * 0.7);
    soup.raycast(origin + shoulder, dir, lookahead).is_none()
        && soup.raycast(origin - shoulder, dir, lookahead).is_none()
}

fn rotate_y(v: Vec3, angle: f32) -> Vec3 {
    let (sin, cos) = angle.sin_cos();
    vec3(v.x * cos + v.z * sin, v.y, -v.x * sin + v.z * cos)
}

/// Wave composition: shards scale fast, sentinels join from wave 2.
pub fn compose_wave(wave: u32) -> Vec<EnemyKind> {
    let shards = (2 + wave * 2).min(14);
    let sentinels = wave.saturating_sub(1).min(6);
    let mut queue = Vec::new();
    for i in 0..shards.max(sentinels) {
        if i < shards {
            queue.push(EnemyKind::Shard);
        }
        if i < sentinels {
            queue.push(EnemyKind::Sentinel);
        }
    }
    queue
}

fn burst(particles: &mut Vec<Particle>, rng: &mut Lcg, at: Vec3, color: Vec4, count: usize) {
    for _ in 0..count {
        let dir = rng.direction();
        particles.push(Particle {
            pos: at,
            vel: dir * (3.0 + rng.next_f32() * 4.0),
            axis: rng.direction() * (0.08 + rng.next_f32() * 0.14),
            color,
            life: 0.45 + rng.next_f32() * 0.35,
            max_life: 0.8,
        });
    }
}

fn spark(particles: &mut Vec<Particle>, rng: &mut Lcg, at: Vec3, count: usize) {
    for _ in 0..count {
        let dir = rng.direction();
        particles.push(Particle {
            pos: at,
            vel: dir * (5.0 + rng.next_f32() * 3.0),
            axis: rng.direction() * 0.09,
            color: vec4(0.9, 0.95, 1.0, 1.2),
            life: 0.18 + rng.next_f32() * 0.12,
            max_life: 0.3,
        });
    }
}

/// A hot tracer segment from muzzle to hit point that flashes and dies.
fn tracer(particles: &mut Vec<Particle>, from: Vec3, to: Vec3) {
    particles.push(Particle {
        pos: (from + to) * 0.5,
        vel: Vec3::ZERO,
        axis: (to - from) * 0.5,
        color: vec4(1.0, 0.95, 0.7, 1.6),
        life: 0.06,
        max_life: 0.06,
    });
}

/// Tiny deterministic xorshift* — no rand dependency, reproducible demos.
pub struct Lcg(u64);

impl Lcg {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn direction(&mut self) -> Vec3 {
        loop {
            let v = vec3(
                self.next_f32() * 2.0 - 1.0,
                self.next_f32() * 2.0 - 1.0,
                self.next_f32() * 2.0 - 1.0,
            );
            let len = v.length();
            if len > 1e-3 && len <= 1.0 {
                return v / len;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EYE: Vec3 = vec3(0.0, 1.55, 0.0);
    const AIM: Vec3 = vec3(0.0, 0.0, -1.0);
    /// Tracer origin — a plausible barrel-tip point below-right of the eye.
    const MUZ: Vec3 = vec3(0.15, 1.35, -0.6);

    /// Empty world: nothing to collide with.
    fn open_soup() -> TriangleSoup {
        TriangleSoup::new(&[], &[], 2.0)
    }

    /// A big wall filling the z = `z` plane (like a pillar face).
    fn wall_at_z(z: f32) -> TriangleSoup {
        let v = [
            vec3(-12.0, -2.0, z),
            vec3(12.0, -2.0, z),
            vec3(12.0, 8.0, z),
            vec3(-12.0, 8.0, z),
        ];
        TriangleSoup::new(&v, &[0, 1, 2, 0, 2, 3], 2.0)
    }

    fn spawned(kind: EnemyKind, pos: Vec3, wave: u32) -> Enemy {
        Enemy {
            kind,
            pos,
            yaw: 0.0,
            hp: kind.max_hp(),
            age: SPAWN_RAMP + 1.0,
            hit_flash: 0.0,
            wobble: 0.0,
            fire_cooldown: kind.fire_interval(wave).unwrap_or(f32::INFINITY),
            avoid: 0.0,
        }
    }

    #[test]
    fn wave_composition_scales() {
        assert!(compose_wave(1).iter().all(|k| *k == EnemyKind::Shard));
        assert_eq!(compose_wave(1).len(), 4);
        let w3 = compose_wave(3);
        assert_eq!(w3.iter().filter(|k| **k == EnemyKind::Sentinel).count(), 2);
        assert!(compose_wave(30).len() <= 20, "capped");
    }

    #[test]
    fn later_waves_shoot_faster_and_harder() {
        let early = EnemyKind::Sentinel.fire_interval(1).unwrap();
        let late = EnemyKind::Sentinel.fire_interval(8).unwrap();
        assert!(late < early, "fire interval shrinks");
        assert!(bolt_damage(8) > bolt_damage(1));
        assert!(bolt_speed(8) > bolt_speed(1));
        // Shards only start shooting at wave 3.
        assert!(EnemyKind::Shard.fire_interval(1).is_none());
        assert!(EnemyKind::Shard.fire_interval(3).is_some());
    }

    #[test]
    fn shooting_hits_the_nearest_enemy_along_aim() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Sentinel, vec3(0.0, 0.0, -8.0), 1));
        game.enemies.push(spawned(EnemyKind::Shard, vec3(0.0, 0.0, -3.0), 1));

        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        // The near shard (30hp) takes 24 and survives; the far sentinel is
        // shadowed by it and untouched.
        let shard = game.enemies.iter().find(|e| e.kind == EnemyKind::Shard).unwrap();
        assert!((shard.hp - 6.0).abs() < 1e-3);
        let sentinel = game.enemies.iter().find(|e| e.kind == EnemyKind::Sentinel).unwrap();
        assert_eq!(sentinel.hp, 100.0);
        assert!(game.recoil() > 0.9, "shot kicks recoil");
    }

    #[test]
    fn fire_rate_is_limited() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Sentinel, vec3(0.0, 0.0, -8.0), 1));
        // Two shots one frame apart: the cooldown blocks the second.
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        let sentinel = &game.enemies[0];
        assert!((sentinel.hp - (100.0 - GUN_DAMAGE)).abs() < 1e-3);
    }

    #[test]
    fn missing_still_recoils_and_leaves_a_tracer() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Shard, vec3(20.0, 0.0, 0.0), 1)); // off to the side
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        assert_eq!(game.enemies[0].hp, 30.0, "whiff");
        assert!(game.recoil() > 0.9);
        assert!(!game.particles.is_empty(), "muzzle flash + tracer");
    }

    #[test]
    fn enemy_bolts_fly_and_damage_the_player() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        // A sentinel whose fire timer is up, at range.
        let mut sentinel = spawned(EnemyKind::Sentinel, vec3(0.0, 0.0, -12.0), 1);
        sentinel.fire_cooldown = 0.0;
        game.enemies.push(sentinel);

        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert_eq!(game.bolts.len(), 1, "one bolt fired");

        // Fast-forward: the bolt should reach and hurt the player.
        let before = game.hp;
        for _ in 0..240 {
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, &open_soup());
            if game.hp < before {
                break;
            }
        }
        assert!(game.hp < before, "bolt connected");
    }

    #[test]
    fn bolts_ignore_contact_iframes() {
        let mut game = Game::new();
        game.hp = 50.0;
        game.iframes = 0.7; // as if just meleed
        game.bolts.push(Bolt {
            pos: EYE + vec3(0.0, 0.0, -0.3),
            vel: vec3(0.0, 0.0, 10.0),
            life: 1.0,
            color: Vec4::ONE,
            damage: 15.0,
        });
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert!(game.hp < 50.0, "projectile hit lands through iframes");
    }

    #[test]
    fn walls_block_the_gun() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Sentinel, vec3(0.0, 0.0, -8.0), 1));
        // Wall at z = -5, enemy behind it: the shot splashes on the wall.
        game.update(0.016, EYE, AIM, MUZ, true, &wall_at_z(-5.0));
        assert_eq!(game.enemies[0].hp, 100.0, "cover protects the enemy");
        assert!(!game.particles.is_empty(), "wall impact spark + tracer");
    }

    #[test]
    fn walls_stop_enemies() {
        let soup = wall_at_z(-5.0);
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Shard, vec3(0.0, 0.0, -9.0), 1));
        // March at the player (behind the wall) for three seconds.
        for _ in 0..180 {
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, &soup);
        }
        let z = game.enemies[0].pos.z;
        assert!(
            z < -5.0 - 0.4,
            "enemy held on its side of the wall (z = {z})"
        );
    }

    #[test]
    fn walls_splash_bolts() {
        let soup = wall_at_z(-5.0);
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        // A bolt heading for the player from behind the wall.
        game.bolts.push(Bolt {
            pos: vec3(0.0, 1.2, -9.0),
            vel: vec3(0.0, 0.0, 12.0),
            life: 5.0,
            color: Vec4::ONE,
            damage: 15.0,
        });
        for _ in 0..120 {
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, &soup);
        }
        assert!(game.bolts.is_empty(), "bolt died on the wall");
        assert_eq!(game.hp, PLAYER_MAX_HP, "player untouched behind cover");
    }

    #[test]
    fn enemies_route_around_a_pillar() {
        // Narrow free-standing wall (a pillar face) between enemy and player.
        let v = [
            vec3(-2.0, 0.0, -5.0),
            vec3(2.0, 0.0, -5.0),
            vec3(2.0, 3.0, -5.0),
            vec3(-2.0, 3.0, -5.0),
        ];
        let soup = TriangleSoup::new(&v, &[0, 1, 2, 0, 2, 3], 2.0);
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Shard, vec3(0.0, 0.0, -9.0), 1));
        // Six seconds is ample to swing around a 4-wide obstacle at 3.4 m/s.
        for _ in 0..360 {
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, &soup);
        }
        let pos = game.enemies[0].pos;
        assert!(
            pos.z > -3.0,
            "steering carried the enemy past the pillar (pos = {pos})"
        );
    }

    #[test]
    fn enemies_hold_fire_without_line_of_sight() {
        let soup = wall_at_z(-5.0);
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        let mut sentinel = spawned(EnemyKind::Sentinel, vec3(0.0, 0.0, -12.0), 1);
        sentinel.fire_cooldown = 0.0;
        game.enemies.push(sentinel);
        game.update(0.016, EYE, AIM, MUZ, false, &soup);
        assert!(game.bolts.is_empty(), "no shot without line of sight");
    }

    #[test]
    fn contact_damage_has_iframes() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Shard, vec3(0.0, 0.0, -0.5), 1));
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        let after_first = game.hp;
        assert!(after_first < PLAYER_MAX_HP);
        game.enemies[0].pos = vec3(0.0, 0.0, -0.5);
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert_eq!(game.hp, after_first, "iframes block the second contact");
    }

    #[test]
    fn clearing_a_wave_advances_after_intermission() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert_eq!(game.wave, 2);
        assert!(matches!(game.phase, Phase::Intermission { .. }));
    }

    #[test]
    fn player_death_ends_the_game() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.hp = 5.0;
        game.enemies.push(spawned(EnemyKind::Sentinel, vec3(0.0, 0.0, -0.5), 1));
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert_eq!(game.phase, Phase::GameOver);
        assert_eq!(game.hp, 0.0);
    }
}
