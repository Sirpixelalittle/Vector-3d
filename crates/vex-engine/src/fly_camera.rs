use glam::{Mat4, Quat, Vec3};
use winit::keyboard::KeyCode;

use crate::Input;

/// Just shy of straight up/down, in radians.
const PITCH_LIMIT: f32 = 1.55;
const SPRINT_MULTIPLIER: f32 = 4.0;
pub const NEAR_PLANE: f32 = 0.05;
pub const FAR_PLANE: f32 = 500.0;

/// Free-flying camera: yaw around world Y, pitch around local X.
/// WASD to move, Space/LCtrl for up/down, LShift to sprint.
pub struct FlyCamera {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub speed: f32,
    pub sensitivity: f32,
}

impl FlyCamera {
    pub fn new(pos: Vec3, yaw: f32, pitch: f32) -> Self {
        Self {
            pos,
            yaw,
            pitch,
            fov_y: 70f32.to_radians(),
            speed: 4.0,
            sensitivity: 0.0022,
        }
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation() * Vec3::X
    }

    pub fn update(&mut self, dt: f32, input: &Input) {
        if input.is_captured() {
            let look = input.mouse_delta() * self.sensitivity;
            self.yaw -= look.x;
            self.pitch = (self.pitch - look.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }

        let mut wish = Vec3::ZERO;
        if input.is_down(KeyCode::KeyW) {
            wish += self.forward();
        }
        if input.is_down(KeyCode::KeyS) {
            wish -= self.forward();
        }
        if input.is_down(KeyCode::KeyD) {
            wish += self.right();
        }
        if input.is_down(KeyCode::KeyA) {
            wish -= self.right();
        }
        if input.is_down(KeyCode::Space) {
            wish += Vec3::Y;
        }
        if input.is_down(KeyCode::ControlLeft) {
            wish -= Vec3::Y;
        }
        let boost = if input.is_down(KeyCode::ShiftLeft) {
            SPRINT_MULTIPLIER
        } else {
            1.0
        };
        self.pos += wish.normalize_or_zero() * self.speed * boost * dt;
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_to_mat4(self.pos, self.forward(), Vec3::Y)
    }

    /// WebGPU-convention projection (Y-up NDC, depth 0..1) times view.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, NEAR_PLANE, FAR_PLANE)
            * self.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec4, vec3};

    #[test]
    fn zero_yaw_looks_down_negative_z() {
        let camera = FlyCamera::new(Vec3::ZERO, 0.0, 0.0);
        assert!(camera.forward().abs_diff_eq(Vec3::NEG_Z, 1e-6));
    }

    #[test]
    fn positive_pitch_looks_up() {
        let camera = FlyCamera::new(Vec3::ZERO, 0.0, 0.5);
        assert!(camera.forward().y > 0.0);
    }

    #[test]
    fn point_ahead_projects_to_screen_center() {
        let camera = FlyCamera::new(Vec3::ZERO, 0.0, 0.0);
        let clip = camera.view_proj(16.0 / 9.0) * Vec4::new(0.0, 0.0, -5.0, 1.0);
        let ndc = clip / clip.w;
        assert!(clip.w > 0.0);
        assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5);
        assert!(ndc.z > 0.0 && ndc.z < 1.0, "wgpu depth must be 0..1");
    }

    #[test]
    fn w_key_moves_along_forward() {
        let mut camera = FlyCamera::new(Vec3::ZERO, 0.3, -0.2);
        let mut input = Input::default();
        input.set_key(KeyCode::KeyW, true);
        camera.update(0.5, &input);
        let expected = camera.forward() * camera.speed * 0.5;
        assert!(camera.pos.abs_diff_eq(expected, 1e-5));
    }

    #[test]
    fn movement_is_frame_rate_independent() {
        let mut slow = FlyCamera::new(vec3(1.0, 2.0, 3.0), 1.0, 0.0);
        let mut fast = FlyCamera::new(vec3(1.0, 2.0, 3.0), 1.0, 0.0);
        let mut input = Input::default();
        input.set_key(KeyCode::KeyD, true);
        slow.update(0.2, &input);
        for _ in 0..10 {
            fast.update(0.02, &input);
        }
        assert!(slow.pos.abs_diff_eq(fast.pos, 1e-4));
    }
}
