use glam::{Mat4, Quat, Vec3, vec3};

use crate::Input;

const PITCH_LIMIT: f32 = 1.5;
const ZOOM_SPEED: f32 = 0.15;

/// Model-inspection camera: orbits a target point; mouse look while
/// captured, scroll wheel to zoom.
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub sensitivity: f32,
    pub min_distance: f32,
}

impl OrbitCamera {
    /// Frame an axis-aligned bounding box comfortably.
    pub fn framing(aabb_min: Vec3, aabb_max: Vec3) -> Self {
        let half_diagonal = ((aabb_max - aabb_min).length() * 0.5).max(0.01);
        Self {
            target: (aabb_min + aabb_max) * 0.5,
            distance: half_diagonal * 2.8,
            yaw: 0.7,
            pitch: -0.35,
            fov_y: 55f32.to_radians(),
            sensitivity: 0.005,
            min_distance: half_diagonal * 0.15,
        }
    }

    pub fn update(&mut self, input: &Input) {
        if input.is_captured() {
            let look = input.mouse_delta() * self.sensitivity;
            self.yaw -= look.x;
            self.pitch = (self.pitch - look.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
        let scroll = input.scroll_delta();
        if scroll != 0.0 {
            self.distance = (self.distance * (-scroll * ZOOM_SPEED).exp()).max(self.min_distance);
        }
    }

    pub fn eye(&self) -> Vec3 {
        let rotation = Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch);
        self.target + rotation * vec3(0.0, 0.0, self.distance)
    }

    /// WebGPU-convention projection; near/far scale with orbit distance so
    /// tiny and huge models both render without depth trouble.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).clamp(0.001, 0.1);
        let far = self.distance * 100.0 + 10.0;
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, near, far)
            * glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_centers_the_box_at_distance() {
        let camera = OrbitCamera::framing(vec3(-1.0, 0.0, -1.0), vec3(1.0, 2.0, 1.0));
        assert_eq!(camera.target, vec3(0.0, 1.0, 0.0));
        assert!((camera.eye() - camera.target).length() - camera.distance < 1e-4);
    }

    #[test]
    fn scroll_zooms_in_but_never_through_the_model() {
        let mut camera = OrbitCamera::framing(Vec3::splat(-1.0), Vec3::splat(1.0));
        let mut input = Input::default();
        let before = camera.distance;
        input.add_scroll(3.0);
        camera.update(&input);
        assert!(camera.distance < before);
        for _ in 0..100 {
            camera.update(&input);
        }
        assert!(camera.distance >= camera.min_distance);
    }
}
