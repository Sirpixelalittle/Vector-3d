//! Hand-authored test shapes. Real content comes from the M2 converter;
//! these exist so the renderer has honest geometry to chew on before then.

use glam::{Mat4, Vec3, Vec4, vec3};

use crate::mesh::{Mesh, edges_from_triangles};
use crate::Segment;

/// A drawable solid: the occluder mesh plus the edges worth stroking.
/// Edges index into `mesh.vertices`.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub mesh: Mesh,
    pub edges: Vec<(u32, u32)>,
}

impl Shape {
    pub fn transformed(&self, transform: Mat4) -> Shape {
        Shape {
            mesh: self.mesh.transformed(transform),
            edges: self.edges.clone(),
        }
    }

    /// Materialize the edge list as colored world-space segments.
    pub fn segments(&self, color: Vec4) -> Vec<Segment> {
        self.edges
            .iter()
            .map(|&(a, b)| {
                Segment::new(
                    self.mesh.vertices[a as usize],
                    self.mesh.vertices[b as usize],
                    color,
                )
            })
            .collect()
    }
}

/// Axis-aligned cube of the given half-extent. Edges are hand-listed (12):
/// deriving them from the triangulation would include face diagonals.
pub fn cube(half_extent: f32) -> Shape {
    let h = half_extent;
    let vertices = vec![
        vec3(-h, -h, -h), // 0
        vec3(h, -h, -h),  // 1
        vec3(h, -h, h),   // 2
        vec3(-h, -h, h),  // 3
        vec3(-h, h, -h),  // 4
        vec3(h, h, -h),   // 5
        vec3(h, h, h),    // 6
        vec3(-h, h, h),   // 7
    ];
    #[rustfmt::skip]
    let indices = vec![
        0, 1, 2,  0, 2, 3, // bottom
        4, 6, 5,  4, 7, 6, // top
        0, 4, 5,  0, 5, 1, // back  (-z)
        3, 2, 6,  3, 6, 7, // front (+z)
        0, 3, 7,  0, 7, 4, // left  (-x)
        1, 5, 6,  1, 6, 2, // right (+x)
    ];
    #[rustfmt::skip]
    let edges = vec![
        (0, 1), (1, 2), (2, 3), (3, 0), // bottom ring
        (4, 5), (5, 6), (6, 7), (7, 4), // top ring
        (0, 4), (1, 5), (2, 6), (3, 7), // pillars
    ];
    Shape {
        mesh: Mesh { vertices, indices },
        edges,
    }
}

/// Regular icosahedron scaled to the given circumradius. All 30 edges are
/// creases (dihedral ≈ 138°), so deriving them from the faces is correct.
pub fn icosahedron(radius: f32) -> Shape {
    let phi = (1.0 + 5.0f32.sqrt()) / 2.0;
    #[rustfmt::skip]
    let raw = [
        vec3(-1.0,  phi,  0.0), vec3( 1.0,  phi,  0.0),
        vec3(-1.0, -phi,  0.0), vec3( 1.0, -phi,  0.0),
        vec3( 0.0, -1.0,  phi), vec3( 0.0,  1.0,  phi),
        vec3( 0.0, -1.0, -phi), vec3( 0.0,  1.0, -phi),
        vec3( phi,  0.0, -1.0), vec3( phi,  0.0,  1.0),
        vec3(-phi,  0.0, -1.0), vec3(-phi,  0.0,  1.0),
    ];
    let vertices: Vec<Vec3> = raw.iter().map(|v| v.normalize() * radius).collect();
    #[rustfmt::skip]
    let indices = vec![
        0, 11, 5,   0, 5, 1,    0, 1, 7,    0, 7, 10,   0, 10, 11,
        1, 5, 9,    5, 11, 4,   11, 10, 2,  10, 7, 6,   7, 1, 8,
        3, 9, 4,    3, 4, 2,    3, 2, 6,    3, 6, 8,    3, 8, 9,
        4, 9, 5,    2, 4, 11,   6, 2, 10,   8, 6, 7,    9, 8, 1,
    ];
    let edges = edges_from_triangles(&indices);
    Shape {
        mesh: Mesh { vertices, indices },
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_has_12_edges_and_12_triangles() {
        let shape = cube(1.0);
        assert_eq!(shape.edges.len(), 12);
        assert_eq!(shape.mesh.indices.len() / 3, 12);
        assert_eq!(shape.mesh.vertices.len(), 8);
    }

    #[test]
    fn icosahedron_is_regular() {
        let shape = icosahedron(1.0);
        assert_eq!(shape.mesh.vertices.len(), 12);
        assert_eq!(shape.mesh.indices.len() / 3, 20);
        assert_eq!(shape.edges.len(), 30, "V - E + F = 2 (Euler)");
        for v in &shape.mesh.vertices {
            assert!((v.length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn shape_segments_use_mesh_positions() {
        let shape = cube(2.0);
        let segments = shape.segments(crate::phosphor::GREEN);
        assert_eq!(segments.len(), 12);
        for segment in &segments {
            // Every cube edge has length 2 × half_extent × 1 axis.
            assert!(((segment.a - segment.b).length() - 4.0).abs() < 1e-5);
        }
    }
}
