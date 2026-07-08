//! Arena wave-fight rules: enemies, waves, gunplay, enemy fire, particles.
//! Pure simulation — no rendering, no GPU types — so everything here is
//! unit-testable. `main.rs` turns this state into segments.
//!
//! The world is the same [`TriangleSoup`] the player collides with:
//! enemies capsule-slide around pillars, bolts splash on geometry, the
//! pistol's slugs stop at walls, and enemies hold fire without line of
//! sight. Cover works the same for both sides.

use glam::{Vec3, Vec4, vec3, vec4};
use vex_engine::TriangleSoup;
use vex_engine::collide::slide_capsule;

pub const PLAYER_MAX_HP: f32 = 100.0;
const PLAYER_HIT_RADIUS: f32 = 0.55;
/// Grace period after a melee (contact) hit — projectiles ignore it, so
/// standing in the open under fire still kills you.
const IFRAME_SECONDS: f32 = 0.7;

// --- the pistol ---
const FIRE_COOLDOWN: f32 = 0.24;
pub const RECOIL_SECONDS: f32 = 0.22;
const GUN_DAMAGE: f32 = 24.0;
const GUN_RANGE: f32 = 70.0;
/// Slug speed — ~5× the fastest enemy bolt. Nothing alive can strafe out
/// of a shot at arena ranges, so the old hitscan feel survives the switch
/// to a real projectile (which is what lets the boss shell deflect it).
const BULLET_SPEED: f32 = 95.0;
/// Speed kept after glancing off the boss's sealed entrance shell.
const RICOCHET_KEEP: f32 = 0.35;
/// Path-to-center slack, so aiming near an enemy connects (crosshair feel).
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
/// Spawn ring radius. The arena's octagon wall faces sit at ~24.0 from
/// the center — keep a full body-width of clearance so nobody is born
/// embedded in a wall.
const SPAWN_RADIUS: f32 = 22.5;
const GATE_ANGLES_DEG: [f32; 4] = [0.0, 90.0, 180.0, 270.0];

const SEPARATION_PUSH: f32 = 3.0;

// --- health pack ---
/// Every N kills, a pack spawns at the arena center. Kills come slower on
/// later waves, so healing scales down naturally with difficulty.
const KILLS_PER_PACK: u32 = 10;
const HEAL_AMOUNT: f32 = 35.0;
const PACK_PICKUP_RADIUS: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyKind {
    Shard,
    Sentinel,
    /// Wave-10 mini-boss: a split icosahedron whose crown rises and spins
    /// while bolt rings radiate from the exposed core.
    Boss,
}

impl EnemyKind {
    pub fn max_hp(self) -> f32 {
        match self {
            Self::Shard => 30.0,
            Self::Sentinel => 100.0,
            Self::Boss => 900.0,
        }
    }

    pub fn speed(self) -> f32 {
        match self {
            Self::Shard => 3.4,
            Self::Sentinel => 1.5,
            Self::Boss => 1.1,
        }
    }

    pub fn radius(self) -> f32 {
        match self {
            Self::Shard => 0.55,
            Self::Sentinel => 0.9,
            Self::Boss => 1.15,
        }
    }

    pub fn contact_damage(self) -> f32 {
        match self {
            Self::Shard => 8.0,
            Self::Sentinel => 16.0,
            Self::Boss => 26.0,
        }
    }

    /// Height of the body center above the floor.
    pub fn hover_height(self) -> f32 {
        match self {
            Self::Shard => 1.1,
            Self::Sentinel => 1.0,
            Self::Boss => 1.35,
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
            Self::Boss => vec4(1.0, 0.22, 0.12, 1.0),
        }
    }

    fn score(self) -> u32 {
        match self {
            Self::Shard => 10,
            Self::Sentinel => 30,
            Self::Boss => 400,
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
            // The boss fires ring volleys through its own attack cycle,
            // not the aimed-shot path.
            Self::Shard | Self::Boss => None,
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

// --- boss attack cycle -----------------------------------------------------
// Closed: stalks the player. Open: plants, the crown rises and spins, and
// bolt rings radiate from the exposed core. All state derives from `age`,
// so simulation and rendering share one source of truth.
const BOSS_CLOSED_SECONDS: f32 = 3.2;
const BOSS_OPEN_SECONDS: f32 = 3.6;
pub const BOSS_CYCLE: f32 = BOSS_CLOSED_SECONDS + BOSS_OPEN_SECONDS;
/// Crown rise/settle time within the open window.
const BOSS_OPEN_RAMP: f32 = 0.6;
/// How high the crown lifts off the base (world units).
pub const BOSS_RISE: f32 = 1.15;
/// Whole crown turns per open window. Integral turns matter: the split
/// icosahedron's halves only mesh at multiples of its 72-degree symmetry,
/// so the crown must seat back exactly where it lifted off.
const BOSS_SPIN_TURNS_PER_OPEN: f32 = 3.0;
const BOSS_RING_INTERVAL: f32 = 0.7;
const BOSS_RING_BOLTS: usize = 12;
/// Ring bolts fly slower than aimed shots — dodgeable walls, not snipes.
const BOSS_RING_SPEED_SCALE: f32 = 0.62;
/// Entrance march: gate → arena center, brisk, sealed, not firing.
const BOSS_ENTRANCE_SPEED: f32 = 2.8;
/// Close enough to center to take the stage and start the cycle.
const BOSS_ARRIVE_RADIUS: f32 = 1.5;
/// Sealed shell during the entrance deflects most damage — otherwise the
/// scripted march would be a free execution window.
const BOSS_ENTRANCE_ARMOR: f32 = 0.15;

fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Crown state at `age`: `(openness 0..1, spin angle in radians)`. Spin
/// only advances while open, and carries across cycles so the crown never
/// snaps.
pub fn boss_crown(age: f32) -> (f32, f32) {
    let t = age.rem_euclid(BOSS_CYCLE);
    let open_t = (t - BOSS_CLOSED_SECONDS).clamp(0.0, BOSS_OPEN_SECONDS);
    let openness = if t < BOSS_CLOSED_SECONDS {
        0.0
    } else {
        smooth01(open_t / BOSS_OPEN_RAMP).min(smooth01((BOSS_OPEN_SECONDS - open_t) / BOSS_OPEN_RAMP))
    };
    let cycles = (age / BOSS_CYCLE).floor();
    let spin = (cycles + open_t / BOSS_OPEN_SECONDS)
        * BOSS_SPIN_TURNS_PER_OPEN
        * std::f32::consts::TAU;
    (openness, spin)
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
    /// Boss only: age at which it reached the arena center and its attack
    /// cycle began. `None` while marching in from the gate.
    boss_cycle_start: Option<f32>,
}

impl Enemy {
    /// 0 → 1 while materializing at a spawn gate.
    pub fn spawn_progress(&self) -> f32 {
        (self.age / SPAWN_RAMP).min(1.0)
    }

    /// Boss crown state: `(openness, spin)`. Zero until it has taken the
    /// stage at the arena center — the entrance march stays sealed.
    pub fn crown(&self) -> (f32, f32) {
        match self.boss_cycle_start {
            Some(start) => boss_crown(self.age - start),
            None => (0.0, 0.0),
        }
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

/// The player's slug: tiny, white-hot, and fast enough to feel hitscan —
/// but a real body in the world rather than a ray, so the boss's sealed
/// entrance shell can visibly deflect it instead of silently eating 85%
/// of the damage.
#[derive(Debug, Clone, Copy)]
pub struct Bullet {
    pub pos: Vec3,
    pub vel: Vec3,
    life: f32,
    /// Set once the boss shell has deflected this slug: spent metal. It
    /// still flies and splashes, but wounds nothing and never bounces
    /// again — otherwise a slow ricochet caught by the advancing shell
    /// re-hits every frame and grinds the boss down.
    spent: bool,
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
    HealthSpawned(Vec3),
    HealthPicked,
    /// The boss loosed a bolt ring from its open core.
    BossRing(Vec3),
    /// A slug glanced off the boss's sealed entrance shell.
    Ricochet(Vec3),
}

/// The medkit hovering at the arena center, waiting to be claimed.
#[derive(Debug, Clone, Copy)]
pub struct HealthPack {
    /// Ground position (the cross hovers above it).
    pub pos: Vec3,
    pub age: f32,
}

impl HealthPack {
    /// Hover center for rendering and pickup chimes.
    pub fn center(&self) -> Vec3 {
        self.pos + Vec3::Y * (1.05 + (self.age * 2.2).sin() * 0.08)
    }
}

pub struct Game {
    pub wave: u32,
    pub score: u32,
    pub hp: f32,
    pub phase: Phase,
    pub enemies: Vec<Enemy>,
    pub bolts: Vec<Bolt>,
    /// The player's slugs in flight (enemy shots are `bolts`).
    pub bullets: Vec<Bullet>,
    pub particles: Vec<Particle>,
    /// Set on the frame the player takes damage (drives shake/flash).
    pub damage_flash: f32,
    pub iframes: f32,
    /// Drain with `std::mem::take` each frame.
    pub events: Vec<GameEvent>,
    pub health_pack: Option<HealthPack>,
    kills_toward_pack: u32,
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
            bullets: Vec::new(),
            particles: Vec::new(),
            damage_flash: 0.0,
            iframes: 0.0,
            events: Vec::new(),
            health_pack: None,
            kills_toward_pack: 0,
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

    /// Skip ahead for testing (`--wave N`): clears the field and starts
    /// the given wave after a short intermission (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn jump_to_wave(&mut self, wave: u32) {
        self.wave = wave.max(1);
        self.phase = Phase::Intermission { timer: 1.5 };
        self.enemies.clear();
        self.bolts.clear();
        self.bullets.clear();
        self.spawn_queue.clear();
    }

    /// Inject a bolt flying at `eye` from `pos`, colored and tuned like
    /// `kind`'s real fire (staged threat-band screenshots; native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn force_bolt(&mut self, kind: EnemyKind, pos: Vec3, eye: Vec3) {
        let tint = kind.color();
        let speed = bolt_speed(self.wave);
        self.bolts.push(Bolt {
            pos,
            vel: (eye - pos).normalize_or_zero() * speed,
            life: BOLT_RANGE / speed,
            color: vec4(tint.x, tint.y, tint.z, 1.7),
            damage: bolt_damage(self.wave),
        });
    }

    /// Spawn the medkit immediately (headless screenshots; native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn force_health_pack(&mut self, age: f32) {
        self.health_pack = Some(HealthPack {
            pos: Vec3::ZERO,
            age,
        });
    }

    /// `muzzle` is the world-space point where the viewmodel's barrel tip
    /// appears on screen (computed by the renderer) — slugs spawn there
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
        self.update_bullets(dt, soup);

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
            self.try_fire(eye, aim, muzzle);
        }
        self.update_enemies(dt, eye, soup);
        self.update_health_pack(dt, eye);

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
        // Boss waves past 10 field a tougher boss (bolts already scale).
        let hp = if kind == EnemyKind::Boss {
            kind.max_hp() * (1.0 + 0.05 * self.wave.saturating_sub(10) as f32)
        } else {
            kind.max_hp()
        };
        let gate = GATE_ANGLES_DEG[(self.rng.next_f32() * 4.0) as usize % 4];
        let angle = (gate + (self.rng.next_f32() - 0.5) * 24.0).to_radians();
        let pos = vec3(SPAWN_RADIUS * angle.cos(), 0.0, SPAWN_RADIUS * angle.sin());
        // Stagger initial fire so a fresh wave doesn't volley in unison.
        // The boss fires through its own cycle: ready from the start.
        let fire_cooldown = if kind == EnemyKind::Boss {
            0.0
        } else {
            kind.fire_interval(self.wave)
                .map_or(f32::INFINITY, |interval| interval * (0.5 + self.rng.next_f32()))
        };
        self.enemies.push(Enemy {
            kind,
            pos,
            yaw: 0.0,
            hp,
            age: 0.0,
            hit_flash: 0.0,
            wobble: self.rng.next_f32() * std::f32::consts::TAU,
            fire_cooldown,
            avoid: 0.0,
            boss_cycle_start: None,
        });
    }

    /// Fire the pistol: loose a slug from the barrel tip, converging onto
    /// the eye ray so it lands where the crosshair points. Recoil and the
    /// muzzle flash kick immediately; damage arrives when the slug does —
    /// a few frames later at most, since it outruns everything alive.
    fn try_fire(&mut self, eye: Vec3, aim: Vec3, muzzle: Vec3) {
        if self.fire_cooldown > 0.0 || matches!(self.phase, Phase::GameOver) {
            return;
        }
        self.fire_cooldown = FIRE_COOLDOWN;
        self.recoil = 1.0;
        self.events.push(GameEvent::Shot);
        let target = eye + aim * GUN_RANGE;
        self.bullets.push(Bullet {
            pos: muzzle,
            vel: (target - muzzle).normalize_or_zero() * BULLET_SPEED,
            life: GUN_RANGE / BULLET_SPEED,
            spent: false,
        });
    }

    /// Advance the player's slugs — swept like enemy bolts, so the speed
    /// can't tunnel. A slug stops on the first enemy or wall in its path
    /// (pillars are cover for both sides); the boss's sealed entrance
    /// shell instead deflects it with a visible ricochet and only a
    /// sliver of the damage.
    fn update_bullets(&mut self, dt: f32, soup: &TriangleSoup) {
        if matches!(self.phase, Phase::GameOver) {
            return;
        }
        const BULLET_SKIN: f32 = 0.02;
        let mut i = 0;
        while i < self.bullets.len() {
            let mut bullet = self.bullets[i];
            // Life burns on every path — a ricochet must never grant a
            // slug immortality.
            bullet.life -= dt;
            if bullet.life <= 0.0 {
                self.bullets.swap_remove(i);
                continue;
            }
            let dir = bullet.vel.normalize_or_zero();
            let step_len = bullet.vel.length() * dt;
            let sweep = step_len + BULLET_SKIN;
            let reach = soup.raycast(bullet.pos, dir, sweep).unwrap_or(sweep);

            // Nearest materialized enemy whose assist-padded sphere the
            // sweep enters before it reaches world geometry. Spent slugs
            // are inert to enemies — splash on walls only.
            let mut best: Option<(usize, f32)> = None;
            if !bullet.spent {
                for (idx, enemy) in self.enemies.iter().enumerate() {
                    if enemy.spawn_progress() < 1.0 {
                        continue;
                    }
                    let radius = enemy.kind.radius() + AIM_ASSIST;
                    if let Some(t) = sphere_entry(bullet.pos, dir, reach, enemy.center(), radius)
                        && best.is_none_or(|(_, d)| t < d)
                    {
                        best = Some((idx, t));
                    }
                }
            }

            if let Some((idx, t)) = best {
                let hit = bullet.pos + dir * t;
                let enemy = &mut self.enemies[idx];
                enemy.hit_flash = 1.0;
                // The boss's sealed shell deflects slugs during its
                // entrance march, and it shrugs off knockback entirely —
                // so it can't be shoved back into a wall and re-pinned.
                let entering =
                    enemy.kind == EnemyKind::Boss && enemy.boss_cycle_start.is_none();
                if entering {
                    enemy.hp -= GUN_DAMAGE * BOSS_ENTRANCE_ARMOR;
                    let center = enemy.center();
                    let normal = (hit - center).normalize_or_zero();
                    bullet.pos = hit + normal * BULLET_SKIN;
                    let approach = bullet.vel.dot(normal);
                    if approach < 0.0 {
                        bullet.vel = (bullet.vel - 2.0 * approach * normal) * RICOCHET_KEEP;
                    }
                    bullet.spent = true;
                    spark(&mut self.particles, &mut self.rng, hit, 3);
                    self.events.push(GameEvent::Ricochet(hit));
                    if self.enemies[idx].hp <= 0.0 {
                        self.kill(idx);
                    }
                    self.bullets[i] = bullet;
                    i += 1;
                } else {
                    enemy.hp -= GUN_DAMAGE;
                    if enemy.kind != EnemyKind::Boss {
                        enemy.pos += dir * GUN_KNOCKBACK;
                    }
                    let dead = enemy.hp <= 0.0;
                    spark(&mut self.particles, &mut self.rng, hit, 5);
                    if dead {
                        self.kill(idx);
                    }
                    self.bullets.swap_remove(i);
                }
                continue;
            }

            if reach < sweep {
                // World geometry: splash and die.
                spark(&mut self.particles, &mut self.rng, bullet.pos + dir * reach, 4);
                self.bullets.swap_remove(i);
                continue;
            }

            bullet.pos += dir * step_len;
            self.bullets[i] = bullet;
            i += 1;
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

        // Earn a medkit every KILLS_PER_PACK kills. Overkill banks toward
        // the next one, so leaving a pack on the floor as a reserve is a
        // real strategy — but only one exists at a time.
        self.kills_toward_pack += 1;
        if self.kills_toward_pack >= KILLS_PER_PACK && self.health_pack.is_none() {
            self.kills_toward_pack -= KILLS_PER_PACK;
            let pack = HealthPack {
                pos: Vec3::ZERO,
                age: 0.0,
            };
            self.events.push(GameEvent::HealthSpawned(pack.center()));
            self.health_pack = Some(pack);
        }
    }

    /// Age the pack and hand it to the player on contact — unless they are
    /// already at full health, in which case it waits to be needed.
    fn update_health_pack(&mut self, dt: f32, eye: Vec3) {
        let Some(pack) = &mut self.health_pack else {
            return;
        };
        pack.age += dt;
        if self.hp >= PLAYER_MAX_HP {
            return;
        }
        let player_ground = vec3(eye.x, 0.0, eye.z);
        if (player_ground - pack.pos).length() < PACK_PICKUP_RADIUS {
            self.hp = (self.hp + HEAL_AMOUNT).min(PLAYER_MAX_HP);
            self.health_pack = None;
            self.events.push(GameEvent::HealthPicked);
        }
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
            // The boss enters by marching to the arena center — fighting
            // it at its spawn gate (pinned on the wall) is not a thing.
            let mut target = player_ground;
            let mut speed = enemy.kind.speed();
            if enemy.kind == EnemyKind::Boss && enemy.boss_cycle_start.is_none() {
                target = Vec3::ZERO;
                speed = BOSS_ENTRANCE_SPEED;
                if enemy.pos.length() < BOSS_ARRIVE_RADIUS {
                    enemy.boss_cycle_start = Some(enemy.age);
                }
            }
            let dir = (target - enemy.pos).normalize_or_zero();
            // The boss plants itself while its core is open — the attack
            // is a stationary tell; repositioning is the player's window.
            if enemy.kind == EnemyKind::Boss && enemy.boss_cycle_start.is_some() {
                let (openness, spin) = enemy.crown();
                if openness > 0.05 {
                    speed = 0.0;
                }
                enemy.fire_cooldown -= dt;
                if openness > 0.85 && enemy.fire_cooldown <= 0.0 {
                    enemy.fire_cooldown = BOSS_RING_INTERVAL;
                    // A ring of bolts thrown off the spinning crown: the
                    // lattice rotates with the spin, launched from the rim
                    // of the exposed core.
                    let core = enemy.pos + Vec3::Y * enemy.kind.hover_height();
                    let tint = enemy.kind.color();
                    for k in 0..BOSS_RING_BOLTS {
                        let a = spin + std::f32::consts::TAU * k as f32 / BOSS_RING_BOLTS as f32;
                        let out = vec3(a.cos(), 0.0, a.sin());
                        let speed = bolt_speed(wave) * BOSS_RING_SPEED_SCALE;
                        new_bolts.push(Bolt {
                            pos: core + out * (enemy.kind.radius() * 0.8),
                            vel: out * speed,
                            life: BOLT_RANGE / speed,
                            color: vec4(tint.x, tint.y, tint.z, 1.7),
                            damage: bolt_damage(wave),
                        });
                    }
                    self.events.push(GameEvent::BossRing(core));
                }
            }
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
                heading * speed * dt,
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
                let slid = slide_capsule(
                    soup,
                    enemy.pos,
                    enemy.kind.radius(),
                    enemy.kind.capsule_height(),
                    away * 1.4,
                );
                enemy.pos = vec3(slid.position.x, 0.0, slid.position.z);
            }
        }
        self.bolts.extend(new_bolts);

        // Pairwise separation so enemies don't merge into one blob. The
        // pushes are applied THROUGH the collision slide, never as raw
        // teleports — a raw shove could put an enemy on the far side of a
        // wall, and the slide's came-from tie-break would then dutifully
        // keep it outside the arena forever (a real soft-lock we shipped).
        let mut pushes = vec![Vec3::ZERO; self.enemies.len()];
        for a in 0..self.enemies.len() {
            for b in (a + 1)..self.enemies.len() {
                let (ea, eb) = (&self.enemies[a], &self.enemies[b]);
                let min_dist = ea.kind.radius() + eb.kind.radius();
                let delta = eb.pos - ea.pos;
                let dist = delta.length();
                if dist > 1e-4 && dist < min_dist {
                    let push = delta / dist * (min_dist - dist) * 0.5 * SEPARATION_PUSH * dt;
                    pushes[a] -= push;
                    pushes[b] += push;
                }
            }
        }
        for (enemy, push) in self.enemies.iter_mut().zip(pushes) {
            if push != Vec3::ZERO {
                let slid = slide_capsule(
                    soup,
                    enemy.pos,
                    enemy.kind.radius(),
                    enemy.kind.capsule_height(),
                    push,
                );
                enemy.pos = vec3(slid.position.x, 0.0, slid.position.z);
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
    // Every 10th wave the arena empties for a lone mini-boss.
    if wave >= 10 && wave.is_multiple_of(10) {
        return vec![EnemyKind::Boss];
    }
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

// --- screen-edge threat warning ---------------------------------------

/// Bolts closer than this start warming the screen edge they approach
/// from; the band brightens as they close.
const THREAT_RANGE: f32 = 12.0;

/// How hot each screen edge should burn this frame: `xyz` is the blended
/// color of the bolts bearing down from that side, `w` their combined
/// urgency 0..1. `[left, right, rear]` — bolts ahead get no entry, the
/// dart itself is the indicator there.
#[derive(Debug, Clone, Copy, Default)]
pub struct ThreatEdges(pub [Vec4; 3]);

/// Which edges to warm for the bolts currently in flight. Pure: eye and
/// flattened forward in, per-edge color+urgency out. Only *closing* bolts
/// within [`THREAT_RANGE`] count, weighted by proximity squared so the
/// band surges just before impact.
pub fn threat_edges(eye: Vec3, forward: Vec3, bolts: &[Bolt]) -> ThreatEdges {
    let fwd = vec3(forward.x, 0.0, forward.z).normalize_or_zero();
    let right = fwd.cross(Vec3::Y).normalize_or_zero();
    let mut acc = [Vec4::ZERO; 3];
    for bolt in bolts {
        let offset = eye - bolt.pos;
        let dist = offset.length();
        if dist > THREAT_RANGE || bolt.vel.dot(offset) <= 0.0 {
            continue;
        }
        let toward = vec3(-offset.x, 0.0, -offset.z).normalize_or_zero();
        let (f, r) = (toward.dot(fwd), toward.dot(right));
        let edge = if f >= r.abs() {
            continue; // ahead: the dart is on screen already
        } else if -f >= r.abs() {
            2 // behind
        } else if r > 0.0 {
            1 // right
        } else {
            0 // left
        };
        let closeness = 1.0 - dist / THREAT_RANGE;
        let urgency = closeness * closeness;
        acc[edge] += Vec4::new(
            bolt.color.x * urgency,
            bolt.color.y * urgency,
            bolt.color.z * urgency,
            urgency,
        );
    }
    // Normalize color by total weight; clamp urgency so stacked volleys
    // saturate instead of blooming out.
    for slot in &mut acc {
        if slot.w > 1e-4 {
            let w = slot.w;
            *slot = Vec4::new(slot.x / w, slot.y / w, slot.z / w, w.min(1.0));
        }
    }
    ThreatEdges(acc)
}

/// Earliest distance along unit `dir` from `origin`, within `max`, where
/// the ray enters the sphere — 0.0 if it starts inside. None on a miss.
fn sphere_entry(origin: Vec3, dir: Vec3, max: f32, center: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - center;
    let c = oc.length_squared() - radius * radius;
    if c <= 0.0 {
        return Some(0.0);
    }
    let b = oc.dot(dir);
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    (t >= 0.0 && t <= max).then_some(t)
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
            fire_cooldown: if kind == EnemyKind::Boss {
                0.0
            } else {
                kind.fire_interval(wave).unwrap_or(f32::INFINITY)
            },
            avoid: 0.0,
            boss_cycle_start: None,
        }
    }

    /// Step (without firing) until every slug in flight has landed or
    /// expired — they outrun everything, so this is a handful of frames.
    fn settle_bullets(game: &mut Game, soup: &TriangleSoup) {
        for _ in 0..90 {
            if game.bullets.is_empty() {
                break;
            }
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, soup);
        }
    }

    /// A bolt at `pos` flying straight at the player's eye.
    fn incoming(pos: Vec3, color: Vec4) -> Bolt {
        Bolt {
            pos,
            vel: (EYE - pos).normalize() * 14.0,
            life: 3.0,
            color,
            damage: 10.0,
        }
    }

    #[test]
    fn threat_edges_map_direction_and_skip_the_visible_front() {
        // Facing -Z: a bolt from +X is on the right, -X left, +Z behind,
        // -Z dead ahead (no warning — you can see that one).
        let magenta = vec4(1.0, 0.2, 0.9, 2.0);
        let from_right = [incoming(EYE + vec3(8.0, 0.0, 0.0), magenta)];
        let edges = threat_edges(EYE, AIM, &from_right);
        assert_eq!(edges.0[0].w, 0.0, "left dark");
        assert!(edges.0[1].w > 0.0, "right lit");
        assert!((edges.0[1].x - 1.0).abs() < 1e-4, "carries the bolt hue");

        let from_left = [incoming(EYE + vec3(-8.0, 0.0, 0.0), magenta)];
        assert!(threat_edges(EYE, AIM, &from_left).0[0].w > 0.0);

        let from_behind = [incoming(EYE + vec3(0.0, 0.0, 8.0), magenta)];
        assert!(threat_edges(EYE, AIM, &from_behind).0[2].w > 0.0);

        let from_ahead = [incoming(EYE + vec3(0.0, 0.0, -8.0), magenta)];
        let ahead = threat_edges(EYE, AIM, &from_ahead);
        assert!(ahead.0.iter().all(|e| e.w == 0.0), "front stays clean");
    }

    #[test]
    fn threat_edges_ignore_receding_and_distant_bolts() {
        let color = vec4(1.0, 0.6, 0.2, 2.0);
        // Flying away from the player: no warning even though it's close.
        let receding = [Bolt {
            pos: EYE + vec3(4.0, 0.0, 0.0),
            vel: vec3(10.0, 0.0, 0.0),
            life: 3.0,
            color,
            damage: 10.0,
        }];
        assert!(threat_edges(EYE, AIM, &receding).0.iter().all(|e| e.w == 0.0));

        // Closing but beyond the warning range.
        let far = [incoming(EYE + vec3(THREAT_RANGE + 5.0, 0.0, 0.0), color)];
        assert!(threat_edges(EYE, AIM, &far).0.iter().all(|e| e.w == 0.0));
    }

    #[test]
    fn threat_urgency_rises_as_the_bolt_closes() {
        let color = vec4(1.0, 0.6, 0.2, 2.0);
        let near = threat_edges(EYE, AIM, &[incoming(EYE + vec3(3.0, 0.0, 0.0), color)]);
        let far = threat_edges(EYE, AIM, &[incoming(EYE + vec3(9.0, 0.0, 0.0), color)]);
        assert!(near.0[1].w > far.0[1].w, "closer burns hotter");
        assert!(near.0[1].w <= 1.0, "clamped");
    }

    #[test]
    fn every_tenth_wave_is_a_lone_boss() {
        assert_eq!(compose_wave(10), vec![EnemyKind::Boss]);
        assert_eq!(compose_wave(20), vec![EnemyKind::Boss]);
        assert!(!compose_wave(9).contains(&EnemyKind::Boss));
        assert!(!compose_wave(11).contains(&EnemyKind::Boss));
    }

    #[test]
    fn boss_crown_opens_spins_and_settles() {
        let (closed, _) = boss_crown(1.0);
        assert_eq!(closed, 0.0, "closed while stalking");
        let (open, spin_a) = boss_crown(BOSS_CLOSED_SECONDS + 1.5);
        assert!(open > 0.99, "fully open mid-window");
        let (_, spin_b) = boss_crown(BOSS_CLOSED_SECONDS + 2.0);
        assert!(spin_b > spin_a, "crown spins while open");
        let (reclosed, _) = boss_crown(BOSS_CYCLE + 0.5);
        assert_eq!(reclosed, 0.0, "closed again next cycle");
        // Spin carries across cycles — no snap-back.
        let (_, end_of_open) = boss_crown(BOSS_CYCLE - 1e-3);
        let (_, next_cycle) = boss_crown(BOSS_CYCLE + 0.5);
        assert!((next_cycle - end_of_open).abs() < 0.1);
        // And the seated crown is always in registration: whole turns
        // only, so the icosahedron pattern meshes when closed.
        for cycle in 1..5 {
            let (openness, spin) = boss_crown(cycle as f32 * BOSS_CYCLE + 1.0);
            assert_eq!(openness, 0.0);
            let turns = spin / std::f32::consts::TAU;
            assert!(
                (turns - turns.round()).abs() < 1e-3,
                "crown seated {turns} turns — must be whole"
            );
        }
    }

    #[test]
    fn boss_marches_to_center_sealed_then_takes_the_stage() {
        let soup = open_soup();
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.wave = 10;
        // Spawn out at a gate, like the real spawn ring.
        game.enemies.push(spawned(EnemyKind::Boss, vec3(0.0, 0.0, 22.0), 10));

        // Marching in: sealed, silent, and no cycle has started.
        for _ in 0..30 {
            game.update(0.016, EYE, AIM, MUZ, false, &soup);
        }
        assert!(game.bolts.is_empty(), "no fire during the entrance");
        assert_eq!(game.enemies[0].crown().0, 0.0, "shell sealed while entering");
        let entering = game.enemies[0].pos;
        assert!(entering.length() < 22.0, "moved toward center");
        assert!(entering.z < 22.0 && entering.z > 0.0, "heading inward");

        // Run until it reaches the center and takes the stage.
        for _ in 0..800 {
            game.update(0.016, EYE, AIM, MUZ, false, &soup);
            if game.enemies[0].pos.length() < BOSS_ARRIVE_RADIUS {
                break;
            }
        }
        assert!(
            game.enemies[0].pos.length() < BOSS_ARRIVE_RADIUS,
            "reached center (got {:?})",
            game.enemies[0].pos
        );
    }

    #[test]
    fn boss_shell_ricochets_slugs_during_entrance() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.wave = 10;
        // Directly ahead, still marching in.
        game.enemies.push(spawned(EnemyKind::Boss, vec3(0.0, 0.0, -8.0), 10));
        let full = EnemyKind::Boss.max_hp();
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        for _ in 0..30 {
            if game.enemies[0].hp < full {
                break;
            }
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, &open_soup());
        }
        let taken = full - game.enemies[0].hp;
        assert!(
            (taken - GUN_DAMAGE * BOSS_ENTRANCE_ARMOR).abs() < 1e-3,
            "entrance shell should deflect: took {taken}"
        );
        // The slug glances off instead of dying: still flying, back the
        // way it came, with the event that drives the ricochet ping.
        assert_eq!(game.bullets.len(), 1, "slug ricochets instead of dying");
        assert!(game.bullets[0].vel.z > 0.0, "bounced back out");
        assert!(
            game.events.iter().any(|e| matches!(e, GameEvent::Ricochet(_))),
            "ricochet event fired"
        );
        assert!(game.enemies[0].boss_cycle_start.is_none(), "still entering");

        // A spent slug is inert: even if the advancing shell swallows it,
        // it never wounds again and burns out on schedule. (A trapped
        // ricochet used to re-hit every frame and grind the boss down.)
        let hp_after_glance = game.enemies[0].hp;
        for _ in 0..150 {
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, &open_soup());
        }
        assert_eq!(
            game.enemies[0].hp, hp_after_glance,
            "one glance, one tick — spent metal wounds nothing"
        );
        assert!(game.bullets.is_empty(), "spent slug expired");
    }

    #[test]
    fn arrived_boss_takes_full_damage_and_eats_the_slug() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.wave = 10;
        let mut boss = spawned(EnemyKind::Boss, vec3(0.0, 0.0, -8.0), 10);
        // Materialized (past the spawn ramp — targetable), arrived, and
        // 0.1s into its cycle: the closed window, so it stalks.
        boss.boss_cycle_start = Some(boss.age - 0.1);
        game.enemies.push(boss);
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        settle_bullets(&mut game, &open_soup());
        let taken = EnemyKind::Boss.max_hp() - game.enemies[0].hp;
        assert!(
            (taken - GUN_DAMAGE).abs() < 1e-3,
            "full damage once it takes the stage: {taken}"
        );
        assert!(game.bullets.is_empty(), "slug consumed, not deflected");
        assert!(
            !game.events.iter().any(|e| matches!(e, GameEvent::Ricochet(_))),
            "no ricochet off an arrived boss"
        );
    }

    #[test]
    fn boss_ignores_knockback() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.wave = 10;
        // Arrived and closed, so it stalks (not planted) — isolating that
        // the gun's knockback doesn't shove it back toward a wall. Age
        // stays past the spawn ramp so the slug can actually connect.
        let mut boss = spawned(EnemyKind::Boss, vec3(0.0, 0.0, -8.0), 10);
        boss.boss_cycle_start = Some(boss.age - 0.1);
        game.enemies.push(boss);
        let full = EnemyKind::Boss.max_hp();
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        for _ in 0..30 {
            if game.enemies[0].hp < full {
                break;
            }
            game.update(1.0 / 60.0, EYE, AIM, MUZ, false, &open_soup());
        }
        assert!(game.enemies[0].hp < full, "slug landed");
        // It steps toward the player (−Z→ +Z a hair), never away from it:
        // knockback (which points along −Z aim) would push z more negative.
        assert!(game.enemies[0].pos.z > -8.05, "not shoved backward");
    }

    #[test]
    fn boss_fires_radial_rings_only_while_open() {
        let soup = open_soup();
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.wave = 10;
        let mut boss = spawned(EnemyKind::Boss, vec3(0.0, 0.0, -10.0), 10);
        // Simulate having taken the stage (the entrance is covered by its
        // own test); the cycle clock starts from arrival.
        boss.boss_cycle_start = Some(0.0);
        boss.age = 1.0; // closed window
        game.enemies.push(boss);
        game.update(0.016, EYE, AIM, MUZ, false, &soup);
        assert!(game.bolts.is_empty(), "no fire while closed");
        let closed_pos = game.enemies[0].pos;
        assert_ne!(closed_pos, vec3(0.0, 0.0, -10.0), "stalks while closed");

        // Age into the open window: planted and firing.
        game.enemies[0].age = BOSS_CLOSED_SECONDS + 1.0;
        game.enemies[0].fire_cooldown = 0.0;
        let before = game.enemies[0].pos;
        game.update(0.016, EYE, AIM, MUZ, false, &soup);
        assert_eq!(game.enemies[0].pos, before, "planted while open");
        assert_eq!(game.bolts.len(), 12, "a full ring");
        assert!(
            game.events
                .iter()
                .any(|e| matches!(e, GameEvent::BossRing(_)))
        );
        // Radial: horizontal, uniform speed, directions cancel out.
        let speed = game.bolts[0].vel.length();
        let mut sum = Vec3::ZERO;
        for bolt in &game.bolts {
            assert!(bolt.vel.y.abs() < 1e-4);
            assert!((bolt.vel.length() - speed).abs() < 1e-3);
            sum += bolt.vel / speed;
        }
        assert!(sum.length() < 1e-3, "even circle, no aim bias");
    }

    #[test]
    fn jump_to_wave_stages_the_requested_wave() {
        let mut game = Game::new();
        game.jump_to_wave(10);
        // Run out the short intermission.
        for _ in 0..200 {
            game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        }
        assert_eq!(game.wave, 10);
        assert!(
            game.enemies.iter().all(|e| e.kind == EnemyKind::Boss)
                && (!game.enemies.is_empty() || game.spawn_queue == vec![EnemyKind::Boss]),
            "wave 10 fields exactly the boss"
        );
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
        assert!(game.recoil() > 0.9, "shot kicks recoil");
        settle_bullets(&mut game, &open_soup());
        // The near shard (30hp) takes 24 and survives; the far sentinel is
        // shadowed by it and untouched.
        let shard = game.enemies.iter().find(|e| e.kind == EnemyKind::Shard).unwrap();
        assert!((shard.hp - 6.0).abs() < 1e-3);
        let sentinel = game.enemies.iter().find(|e| e.kind == EnemyKind::Sentinel).unwrap();
        assert_eq!(sentinel.hp, 100.0);
    }

    #[test]
    fn fire_rate_is_limited() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Sentinel, vec3(0.0, 0.0, -8.0), 1));
        // Two shots one frame apart: the cooldown blocks the second.
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        assert_eq!(game.bullets.len(), 1, "cooldown blocks the second slug");
        settle_bullets(&mut game, &open_soup());
        let sentinel = &game.enemies[0];
        assert!((sentinel.hp - (100.0 - GUN_DAMAGE)).abs() < 1e-3);
    }

    #[test]
    fn missing_still_recoils_and_the_slug_flies_past() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.enemies.push(spawned(EnemyKind::Shard, vec3(20.0, 0.0, 0.0), 1)); // off to the side
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        assert!(game.recoil() > 0.9);
        assert_eq!(game.bullets.len(), 1, "slug in flight");
        settle_bullets(&mut game, &open_soup());
        assert!(game.bullets.is_empty(), "slug expired at range");
        assert_eq!(game.enemies[0].hp, 30.0, "whiff");
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
        // Wall at z = -5, enemy behind it: the slug splashes on the wall.
        game.update(0.016, EYE, AIM, MUZ, true, &wall_at_z(-5.0));
        settle_bullets(&mut game, &wall_at_z(-5.0));
        assert!(game.bullets.is_empty(), "slug died on the wall");
        assert_eq!(game.enemies[0].hp, 100.0, "cover protects the enemy");
        assert!(!game.particles.is_empty(), "wall impact spark");
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
    fn ten_kills_spawn_a_pack_and_pickup_heals() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.kills_toward_pack = KILLS_PER_PACK - 1;
        game.hp = 40.0;
        // Fight away from the center so the spawn isn't instantly grabbed.
        // The muzzle must ride with the eye — the slug physically spawns
        // there (the old hitscan never cared).
        let away = vec3(0.0, 1.55, 8.0);
        let muz = away + vec3(0.15, -0.2, -0.6);
        let mut shard = spawned(EnemyKind::Shard, vec3(0.0, 0.0, 6.0), 1);
        shard.hp = 1.0;
        game.enemies.push(shard);
        game.update(0.016, away, AIM, muz, true, &open_soup());
        for _ in 0..30 {
            if game.enemies.is_empty() {
                break;
            }
            game.update(1.0 / 60.0, away, AIM, muz, false, &open_soup());
        }
        let pack = game.health_pack.expect("pack spawned on the 10th kill");
        assert_eq!(pack.pos, Vec3::ZERO);
        assert!(
            game.events
                .iter()
                .any(|e| matches!(e, GameEvent::HealthSpawned(_)))
        );

        // Walk to the center: heals and consumes.
        game.events.clear();
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert!(game.health_pack.is_none(), "consumed");
        assert!((game.hp - 75.0).abs() < 1e-3, "40 + 35 heal");
        assert!(game.events.contains(&GameEvent::HealthPicked));
    }

    #[test]
    fn pack_waits_when_player_is_full() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.health_pack = Some(HealthPack {
            pos: Vec3::ZERO,
            age: 0.0,
        });
        // Standing on it at full health: it stays banked.
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert!(game.health_pack.is_some());
        assert_eq!(game.hp, PLAYER_MAX_HP);
        // Take damage, step again: now it heals.
        game.hp = 60.0;
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert!(game.health_pack.is_none());
        assert!((game.hp - 95.0).abs() < 1e-3);
    }

    #[test]
    fn only_one_pack_at_a_time_and_overkill_banks() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.health_pack = Some(HealthPack {
            pos: vec3(5.0, 0.0, 5.0), // away from the player
            age: 0.0,
        });
        game.kills_toward_pack = KILLS_PER_PACK - 1;
        let mut shard = spawned(EnemyKind::Shard, vec3(0.0, 0.0, -2.0), 1);
        shard.hp = 1.0;
        game.enemies.push(shard);
        game.update(0.016, EYE, AIM, MUZ, true, &open_soup());
        settle_bullets(&mut game, &open_soup());
        // Still just the one pack; the earned spawn is banked in the counter.
        assert_eq!(game.kills_toward_pack, KILLS_PER_PACK);
        assert!(
            !game
                .events
                .iter()
                .any(|e| matches!(e, GameEvent::HealthSpawned(_)))
        );
    }

    #[test]
    fn clearing_a_wave_advances_after_intermission() {
        let mut game = Game::new();
        game.phase = Phase::Fighting;
        game.update(0.016, EYE, AIM, MUZ, false, &open_soup());
        assert_eq!(game.wave, 2);
        assert!(matches!(game.phase, Phase::Intermission { .. }));
    }

    /// Arena-shaped wall ring (matches gen_arena.py dimensions), so spawn
    /// positions interact with walls exactly like the real level.
    fn octagon_soup() -> TriangleSoup {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let ring: Vec<Vec3> = (0..8)
            .map(|i| {
                let a = (22.5f32 + 45.0 * i as f32).to_radians();
                vec3(26.0 * a.cos(), 0.0, 26.0 * a.sin())
            })
            .collect();
        for i in 0..8 {
            let (a, b) = (ring[i], ring[(i + 1) % 8]);
            let base = vertices.len() as u32;
            vertices.extend([a, b, b + Vec3::Y * 3.5, a + Vec3::Y * 3.5]);
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        TriangleSoup::new(&vertices, &indices, 2.0)
    }

    /// Soak: an immortal, auto-aiming player clears waves for ~15 sim
    /// minutes in both an open and a walled arena, with uneven frame
    /// times. Catches wave soft-locks (banner shown, nothing spawns) and
    /// stuck-enemy stalls that only appear across many wave transitions.
    #[test]
    fn waves_never_soft_lock() {
        for (name, soup) in [("open", open_soup()), ("octagon", octagon_soup())] {
            let dts = [1.0 / 60.0, 1.0 / 144.0, 1.0 / 30.0, 0.021];
            let eye = EYE;
            let mut game = Game::new();
            let mut last_wave = game.wave;
            let mut stall = 0.0f32;
            for frame in 0..55_000u32 {
                let dt = dts[(frame % 4) as usize];
                let (aim, attack) = game
                    .enemies
                    .iter()
                    .find(|e| e.spawn_progress() >= 1.0)
                    .map(|e| ((e.center() - eye).normalize_or_zero(), true))
                    .unwrap_or((AIM, false));
                game.update(dt, eye, aim, MUZ, attack, &soup);
                game.events.clear();
                game.hp = PLAYER_MAX_HP; // immortal

                assert!(
                    !(matches!(game.phase, Phase::Fighting)
                        && game.spawn_queue.is_empty()
                        && game.enemies.is_empty()),
                    "[{name}] fighting-with-nothing at frame {frame}, wave {}",
                    game.wave
                );
                if game.wave != last_wave {
                    last_wave = game.wave;
                    stall = 0.0;
                }
                stall += dt;
                assert!(
                    stall < 120.0,
                    "[{name}] no wave progress for 2 sim-minutes: frame {frame}, \
                     wave {}, phase {:?}, queue {}, enemies {} (spawned {})",
                    game.wave,
                    game.phase,
                    game.spawn_queue.len(),
                    game.enemies.len(),
                    game.enemies
                        .iter()
                        .filter(|e| e.spawn_progress() >= 1.0)
                        .count(),
                );
            }
            assert!(
                last_wave >= 5,
                "[{name}] expected to clear several waves, reached {last_wave}"
            );
        }
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
