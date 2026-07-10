use glam::{Mat4, Quat, Vec3};
use winit::keyboard::KeyCode;

use crate::collide::{TriangleSoup, slide_capsule};
use crate::Input;

const PITCH_LIMIT: f32 = 1.5;
const GRAVITY: f32 = -18.0;
const JUMP_SPEED: f32 = 6.0;
const SPRINT_MULTIPLIER: f32 = 2.4;
/// Weapon-bob cycles per meter walked.
const BOB_FREQUENCY: f32 = 1.8;
pub const NEAR_PLANE: f32 = 0.05;
pub const FAR_PLANE: f32 = 300.0;

/// First-person walking controller: yaw/pitch look, ground-plane WASD,
/// capsule collision against a [`TriangleSoup`]. Jump and sprint are
/// classic defaults; games can disable them and enable the dash — a
/// burst of decaying horizontal velocity on a long cooldown, triggered
/// by Space or Shift, in the direction being walked (facing if still).
pub struct FpsController {
    /// Feet position (bottom of the capsule).
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub radius: f32,
    pub height: f32,
    pub eye_height: f32,
    pub speed: f32,
    pub sensitivity: f32,
    pub fov_y: f32,
    pub jump_enabled: bool,
    pub sprint_enabled: bool,
    pub dash_enabled: bool,
    pub dash_cooldown: f32,
    /// Initial dash burst speed; travel distance ≈ speed / decay.
    pub dash_speed: f32,
    pub dash_decay: f32,
    /// A banked bonus dash (powerups): the next dash consumes this
    /// instead of starting the cooldown. Grant via [`grant_dash`].
    ///
    /// [`grant_dash`]: Self::grant_dash
    pub extra_dash: bool,
    velocity_y: f32,
    grounded: bool,
    bob_phase: f32,
    dash_velocity: Vec3,
    dash_timer: f32,
    dashed: bool,
}

impl FpsController {
    pub fn new(pos: Vec3, yaw: f32) -> Self {
        Self {
            pos,
            yaw,
            pitch: 0.0,
            radius: 0.35,
            height: 1.7,
            eye_height: 1.55,
            speed: 3.2,
            sensitivity: 0.0022,
            fov_y: 70f32.to_radians(),
            jump_enabled: true,
            sprint_enabled: true,
            dash_enabled: false,
            dash_cooldown: 10.0,
            dash_speed: 26.0,
            dash_decay: 6.5,
            extra_dash: false,
            velocity_y: 0.0,
            grounded: false,
            bob_phase: 0.0,
            dash_velocity: Vec3::ZERO,
            dash_timer: 0.0,
            dashed: false,
        }
    }

    pub fn update(&mut self, dt: f32, input: &Input, soup: &TriangleSoup) {
        if input.is_captured() {
            let look = input.mouse_delta() * self.sensitivity;
            self.yaw -= look.x;
            self.pitch = (self.pitch - look.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }

        // Ground-plane movement basis from yaw only.
        let forward = Quat::from_rotation_y(self.yaw) * Vec3::NEG_Z;
        let right = Quat::from_rotation_y(self.yaw) * Vec3::X;
        let mut wish = Vec3::ZERO;
        if input.is_down(KeyCode::KeyW) {
            wish += forward;
        }
        if input.is_down(KeyCode::KeyS) {
            wish -= forward;
        }
        if input.is_down(KeyCode::KeyD) {
            wish += right;
        }
        if input.is_down(KeyCode::KeyA) {
            wish -= right;
        }
        let sprint = if self.sprint_enabled && input.is_down(KeyCode::ShiftLeft) {
            SPRINT_MULTIPLIER
        } else {
            1.0
        };
        let horizontal = wish.normalize_or_zero() * self.speed * sprint;

        self.dash_timer = (self.dash_timer - dt).max(0.0);
        if self.dash_enabled
            && self.dash_timer <= 0.0
            && (input.is_just_pressed(KeyCode::Space)
                || input.is_just_pressed(KeyCode::ShiftLeft))
        {
            let dir = if wish.length_squared() > 1e-6 {
                wish.normalize_or_zero()
            } else {
                forward
            };
            self.dash_velocity = dir * self.dash_speed;
            if self.extra_dash {
                // A banked bonus dash spends itself instead of the
                // cooldown — the meter never moves, and the next dash is
                // available immediately.
                self.extra_dash = false;
            } else {
                self.dash_timer = self.dash_cooldown;
            }
            self.dashed = true;
        }
        // The burst decays exponentially; it rides through the same
        // collision slide as walking, so walls stop it (nothing teleports).
        self.dash_velocity *= (-self.dash_decay * dt).exp();
        if self.dash_velocity.length_squared() < 1e-4 {
            self.dash_velocity = Vec3::ZERO;
        }

        self.velocity_y += GRAVITY * dt;
        if self.grounded {
            self.velocity_y = self.velocity_y.max(0.0);
            if self.jump_enabled && input.is_down(KeyCode::Space) {
                self.velocity_y = JUMP_SPEED;
            }
        }

        let motion = (horizontal + self.dash_velocity + Vec3::Y * self.velocity_y) * dt;
        let result = slide_capsule(soup, self.pos, self.radius, self.height, motion);
        self.pos = result.position;
        self.grounded = result.grounded;
        if self.grounded {
            self.velocity_y = self.velocity_y.max(0.0);
            self.bob_phase += horizontal.length() * dt * BOB_FREQUENCY;
        }
    }

    pub fn is_grounded(&self) -> bool {
        self.grounded
    }

    /// Zero every piece of transient motion state — velocities, the dash
    /// burst/cooldown/bank, weapon bob, the just-dashed latch — back to
    /// the values [`new`] starts with. Tuning (sensitivity, speeds, the
    /// dash configuration) and the pose (`pos`/`yaw`/`pitch`) are left
    /// alone. For game restarts: a fresh run must not inherit the last
    /// run's momentum, half-spent cooldown, or banked powerup.
    ///
    /// [`new`]: Self::new
    pub fn reset_motion(&mut self) {
        self.extra_dash = false;
        self.velocity_y = 0.0;
        self.grounded = false;
        self.bob_phase = 0.0;
        self.dash_velocity = Vec3::ZERO;
        self.dash_timer = 0.0;
        self.dashed = false;
    }

    /// 0 → 1 dash recharge (1 = ready). Always 1 when the dash is off.
    /// Bank a bonus dash (powerup pickup): if the dash is ready it's held
    /// as an extra charge — the next dash spends the charge, not the
    /// cooldown — otherwise it just finishes the recharge instantly.
    pub fn grant_dash(&mut self) {
        if self.dash_timer <= 0.0 {
            self.extra_dash = true;
        } else {
            self.dash_timer = 0.0;
        }
    }

    pub fn dash_ready_fraction(&self) -> f32 {
        if self.dash_cooldown <= 0.0 {
            return 1.0;
        }
        1.0 - (self.dash_timer / self.dash_cooldown).clamp(0.0, 1.0)
    }

    /// True once per dash — take-and-clear, for sound/FX triggers.
    pub fn just_dashed(&mut self) -> bool {
        std::mem::take(&mut self.dashed)
    }

    /// Walk-cycle phase in radians — drives weapon bob.
    pub fn bob_phase(&self) -> f32 {
        self.bob_phase * std::f32::consts::TAU
    }

    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::Y * self.eye_height
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = glam::camera::rh::view::look_to_mat4(
            self.eye(),
            self.rotation() * Vec3::NEG_Z,
            Vec3::Y,
        );
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, NEAR_PLANE, FAR_PLANE)
            * view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    fn floor() -> TriangleSoup {
        let vertices = [
            vec3(-20.0, 0.0, -20.0),
            vec3(20.0, 0.0, -20.0),
            vec3(20.0, 0.0, 20.0),
            vec3(-20.0, 0.0, 20.0),
        ];
        TriangleSoup::new(&vertices, &[0, 1, 2, 0, 2, 3], 2.0)
    }

    #[test]
    fn falls_to_the_floor_and_stays() {
        let soup = floor();
        let mut player = FpsController::new(vec3(0.0, 2.0, 0.0), 0.0);
        let input = Input::default();
        for _ in 0..120 {
            player.update(1.0 / 60.0, &input, &soup);
        }
        assert!(player.is_grounded());
        assert!(player.pos.y.abs() < 0.01, "feet at y={}", player.pos.y);
    }

    #[test]
    fn dash_bursts_cools_down_and_recharges() {
        let soup = floor();
        let mut player = FpsController::new(vec3(0.0, 0.0, 0.0), 0.0);
        player.jump_enabled = false;
        player.sprint_enabled = false;
        player.dash_enabled = true;
        let mut input = Input::default();
        input.set_key(KeyCode::KeyW, true);
        input.set_key(KeyCode::Space, true);
        for _ in 0..30 {
            player.update(1.0 / 60.0, &input, &soup);
        }
        assert!(player.just_dashed());
        let dashed_z = player.pos.z;
        assert!(dashed_z < -3.0, "dash covered ground: z={dashed_z}");
        assert!(player.pos.y.abs() < 0.01, "no liftoff with jump disabled");
        assert!(player.dash_ready_fraction() < 0.2, "on cooldown");
        // Walking alone over the same time covers far less.
        assert!(dashed_z < -(3.2 * 0.5 + 1.0));
        // Recharges after the cooldown elapses. (end_frame clears the
        // just-pressed edge, as the shell does every frame — otherwise
        // the held key would re-trigger the instant the cooldown ends.)
        input.end_frame();
        for _ in 0..(10 * 60 + 5) {
            player.update(1.0 / 60.0, &input, &soup);
        }
        assert!(player.dash_ready_fraction() >= 1.0);
    }

    #[test]
    fn banked_dash_spends_itself_not_the_cooldown() {
        let soup = floor();
        let mut player = FpsController::new(vec3(0.0, 0.0, 0.0), 0.0);
        player.jump_enabled = false;
        player.sprint_enabled = false;
        player.dash_enabled = true;

        // Granted while ready: banked as an extra charge.
        player.grant_dash();
        assert!(player.extra_dash);

        // end_frame after each update, as the shell does — otherwise the
        // held key's press edge would re-trigger next frame and spend the
        // now-ready normal dash too.
        let mut input = Input::default();
        input.set_key(KeyCode::KeyW, true);
        input.set_key(KeyCode::Space, true);
        for _ in 0..10 {
            player.update(1.0 / 60.0, &input, &soup);
            input.end_frame();
        }
        assert!(player.just_dashed());
        assert!(!player.extra_dash, "charge consumed");
        assert!(
            player.dash_ready_fraction() >= 1.0,
            "the meter never moved: {}",
            player.dash_ready_fraction()
        );

        // The very next press dashes again — and that one cools down.
        input.end_frame();
        input.set_key(KeyCode::Space, false);
        input.end_frame();
        input.set_key(KeyCode::Space, true);
        player.update(1.0 / 60.0, &input, &soup);
        assert!(player.just_dashed(), "second dash fired immediately");
        assert!(player.dash_ready_fraction() < 0.2, "normal cooldown now");
    }

    #[test]
    fn granting_mid_cooldown_just_refills() {
        let soup = floor();
        let mut player = FpsController::new(vec3(0.0, 0.0, 0.0), 0.0);
        player.jump_enabled = false;
        player.sprint_enabled = false;
        player.dash_enabled = true;
        let mut input = Input::default();
        input.set_key(KeyCode::KeyW, true);
        input.set_key(KeyCode::Space, true);
        for _ in 0..10 {
            player.update(1.0 / 60.0, &input, &soup);
        }
        assert!(player.dash_ready_fraction() < 1.0, "cooling down");

        player.grant_dash();
        assert!(!player.extra_dash, "no bank while recharging");
        assert!(player.dash_ready_fraction() >= 1.0, "recharge finished");
    }

    #[test]
    fn reset_motion_drops_momentum_and_bank_but_keeps_tuning() {
        let soup = floor();
        let mut player = FpsController::new(vec3(0.0, 0.0, 0.0), 0.0);
        player.jump_enabled = false;
        player.sprint_enabled = false;
        player.dash_enabled = true;
        player.sensitivity = 0.005;

        // Die mid-dash: burst velocity live, cooldown running, bob
        // advanced, the just-dashed latch still set — plus a banked
        // charge, as if a powerup was held at death.
        let mut input = Input::default();
        input.set_key(KeyCode::KeyW, true);
        input.set_key(KeyCode::Space, true);
        for _ in 0..5 {
            player.update(1.0 / 60.0, &input, &soup);
            input.end_frame();
        }
        assert!(player.dash_ready_fraction() < 1.0, "mid-cooldown");
        player.extra_dash = true;

        player.reset_motion();

        assert!(!player.extra_dash, "the bank does not survive a restart");
        assert!(player.dash_ready_fraction() >= 1.0, "meter starts full");
        assert!(!player.just_dashed(), "no stale dash latch");
        assert_eq!(player.bob_phase(), 0.0);
        assert_eq!(player.sensitivity, 0.005, "options tuning survives");

        // No inherited momentum: standing still stays still.
        let idle = Input::default();
        let start = player.pos;
        for _ in 0..30 {
            player.update(1.0 / 60.0, &idle, &soup);
        }
        assert!(
            (player.pos.x - start.x).abs() < 1e-3
                && (player.pos.z - start.z).abs() < 1e-3,
            "drifted to {:?}",
            player.pos
        );
    }

    #[test]
    fn disabled_sprint_ignores_shift() {
        let soup = floor();
        let mut fast = FpsController::new(vec3(0.0, 0.0, 0.0), 0.0);
        let mut slow = FpsController::new(vec3(0.0, 0.0, 0.0), 0.0);
        slow.sprint_enabled = false;
        let mut input = Input::default();
        input.set_key(KeyCode::KeyW, true);
        input.set_key(KeyCode::ShiftLeft, true);
        for _ in 0..60 {
            fast.update(1.0 / 60.0, &input, &soup);
            slow.update(1.0 / 60.0, &input, &soup);
        }
        assert!(fast.pos.z < slow.pos.z - 1.0, "sprint flag matters");
    }

    #[test]
    fn walks_forward_on_the_ground_plane() {
        let soup = floor();
        let mut player = FpsController::new(vec3(0.0, 0.0, 5.0), 0.0);
        let mut input = Input::default();
        input.set_key(KeyCode::KeyW, true);
        for _ in 0..60 {
            player.update(1.0 / 60.0, &input, &soup);
        }
        assert!(player.pos.z < 5.0 - 2.0, "moved toward -Z: z={}", player.pos.z);
        assert!(player.pos.x.abs() < 1e-3);
    }
}
