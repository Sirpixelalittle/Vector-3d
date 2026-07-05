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
/// gravity and jumping, capsule collision against a [`TriangleSoup`].
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
    velocity_y: f32,
    grounded: bool,
    bob_phase: f32,
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
            velocity_y: 0.0,
            grounded: false,
            bob_phase: 0.0,
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
        let sprint = if input.is_down(KeyCode::ShiftLeft) {
            SPRINT_MULTIPLIER
        } else {
            1.0
        };
        let horizontal = wish.normalize_or_zero() * self.speed * sprint;

        self.velocity_y += GRAVITY * dt;
        if self.grounded {
            self.velocity_y = self.velocity_y.max(0.0);
            if input.is_down(KeyCode::Space) {
                self.velocity_y = JUMP_SPEED;
            }
        }

        let motion = (horizontal + Vec3::Y * self.velocity_y) * dt;
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
