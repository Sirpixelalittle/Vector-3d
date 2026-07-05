use glam::{Mat4, Vec3, Vec4, Vec4Swizzles};

/// View frustum as six inward-facing planes, extracted from a
/// view-projection matrix (Gribb–Hartmann), for WebGPU depth range 0..1.
/// A point is inside when `plane · (p, 1) ≥ 0` for every plane.
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    pub fn from_view_proj(view_proj: Mat4) -> Self {
        let r0 = view_proj.row(0);
        let r1 = view_proj.row(1);
        let r2 = view_proj.row(2);
        let r3 = view_proj.row(3);
        Self {
            planes: [
                r3 + r0, // left
                r3 - r0, // right
                r3 + r1, // bottom
                r3 - r1, // top
                r2,      // near (z ≥ 0 in 0..1 clip)
                r3 - r2, // far
            ],
        }
    }

    /// Conservative AABB test: false only if the box is fully outside some
    /// plane (the standard p-vertex test; may keep rare corner cases).
    pub fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        self.planes.iter().all(|plane| {
            let normal = plane.xyz();
            // The box corner farthest along the plane normal.
            let p_vertex = Vec3::select(normal.cmpge(Vec3::ZERO), max, min);
            normal.dot(p_vertex) + plane.w >= 0.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    fn test_frustum() -> Frustum {
        // Camera at origin looking down -Z, 90° fov, square aspect.
        let proj = glam::camera::rh::proj::directx::perspective(
            90f32.to_radians(),
            1.0,
            0.1,
            100.0,
        );
        Frustum::from_view_proj(proj)
    }

    #[test]
    fn keeps_box_in_front() {
        let f = test_frustum();
        assert!(f.intersects_aabb(vec3(-1.0, -1.0, -6.0), vec3(1.0, 1.0, -4.0)));
    }

    #[test]
    fn culls_box_behind_camera() {
        let f = test_frustum();
        assert!(!f.intersects_aabb(vec3(-1.0, -1.0, 4.0), vec3(1.0, 1.0, 6.0)));
    }

    #[test]
    fn culls_box_far_to_the_side_but_keeps_straddlers() {
        let f = test_frustum();
        // At z=-10 the frustum half-width is 10; x∈[30,32] is well outside.
        assert!(!f.intersects_aabb(vec3(30.0, -1.0, -11.0), vec3(32.0, 1.0, -10.0)));
        // A box straddling the left plane must be kept.
        assert!(f.intersects_aabb(vec3(-12.0, -1.0, -11.0), vec3(-8.0, 1.0, -10.0)));
    }

    #[test]
    fn culls_beyond_far_plane() {
        let f = test_frustum();
        assert!(!f.intersects_aabb(vec3(-1.0, -1.0, -300.0), vec3(1.0, 1.0, -200.0)));
    }
}
