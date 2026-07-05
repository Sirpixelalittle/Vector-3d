//! Shared primitives for the vector3d engine.

pub mod font;
mod frustum;
mod mesh;
mod model;
pub mod shapes;

pub use frustum::Frustum;
pub use mesh::{Mesh, edges_from_triangles};
pub use model::{
    EdgeKind, STYLE_DASH, STYLE_FLICKER, VEC_MAGIC, VEC_VERSION, VecEdge, VecModel, compute_aabb,
};
pub use shapes::Shape;

use glam::{Vec3, Vec4};

/// A colored line segment in world space — the engine's visible primitive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub a: Vec3,
    pub b: Vec3,
    /// Linear RGB in `xyz`; `w` is a brightness multiplier (dim < 1.0 < glow).
    pub color: Vec4,
    /// Dash period in world units; 0.0 draws solid.
    pub dash_period: f32,
    /// 0.0 steady … 1.0 fully pulsing (per-segment phase from instance id).
    pub flicker: f32,
}

impl Segment {
    pub fn new(a: Vec3, b: Vec3, color: Vec4) -> Self {
        Self {
            a,
            b,
            color,
            dash_period: 0.0,
            flicker: 0.0,
        }
    }

    pub fn with_dash(mut self, period: f32) -> Self {
        self.dash_period = period;
        self
    }

    pub fn with_flicker(mut self, amount: f32) -> Self {
        self.flicker = amount;
        self
    }
}

/// Connect consecutive points into segments of one color.
pub fn polyline(points: &[Vec3], color: Vec4) -> Vec<Segment> {
    points
        .windows(2)
        .map(|pair| Segment::new(pair[0], pair[1], color))
        .collect()
}

/// Phosphor-inspired palette (linear RGB, brightness 1.0).
pub mod phosphor {
    use glam::{Vec4, vec4};

    pub const GREEN: Vec4 = vec4(0.05, 1.0, 0.15, 1.0);
    pub const LIME: Vec4 = vec4(0.55, 1.0, 0.10, 1.0);
    pub const CYAN: Vec4 = vec4(0.05, 0.75, 1.0, 1.0);
    pub const BLUE: Vec4 = vec4(0.10, 0.30, 1.0, 1.0);
    pub const MAGENTA: Vec4 = vec4(1.0, 0.15, 0.90, 1.0);
    pub const RED: Vec4 = vec4(1.0, 0.08, 0.05, 1.0);
    pub const AMBER: Vec4 = vec4(1.0, 0.55, 0.05, 1.0);
    pub const WHITE: Vec4 = vec4(0.90, 0.95, 1.0, 1.0);

    /// Same hue at `k` brightness.
    pub fn dim(color: Vec4, k: f32) -> Vec4 {
        vec4(color.x, color.y, color.z, color.w * k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    #[test]
    fn polyline_connects_consecutive_points() {
        let points = [vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(1.0, 1.0, 0.0)];
        let segments = polyline(&points, phosphor::GREEN);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].b, segments[1].a);
    }

    #[test]
    fn polyline_needs_two_points() {
        assert!(polyline(&[vec3(0.0, 0.0, 0.0)], phosphor::GREEN).is_empty());
    }

    #[test]
    fn dim_scales_brightness_but_not_hue() {
        let dimmed = phosphor::dim(phosphor::CYAN, 0.25);
        assert_eq!(dimmed.truncate(), phosphor::CYAN.truncate());
        assert!((dimmed.w - 0.25).abs() < 1e-6);
    }
}
