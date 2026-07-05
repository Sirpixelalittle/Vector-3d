use std::collections::HashSet;

use glam::{Mat4, Vec3};

/// An indexed triangle mesh. In this engine surfaces are never shaded —
/// meshes exist to occlude lines (rendered into the depth buffer only).
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vec3>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// A copy of this mesh with every vertex transformed.
    pub fn transformed(&self, transform: Mat4) -> Mesh {
        Mesh {
            vertices: self
                .vertices
                .iter()
                .map(|&v| transform.transform_point3(v))
                .collect(),
            indices: self.indices.clone(),
        }
    }

    /// Append this mesh into a combined vertex/index soup, offsetting indices.
    pub fn append_into(&self, vertices: &mut Vec<Vec3>, indices: &mut Vec<u32>) {
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&self.vertices);
        indices.extend(self.indices.iter().map(|&i| i + base));
    }
}

/// Unique undirected edges of a triangle list, sorted for determinism.
///
/// Note: on a triangulated quad face this includes the diagonal — the M2
/// converter removes those with a dihedral-angle test. Only use this
/// directly on shapes whose faces are all true triangles (e.g. icosahedra).
pub fn edges_from_triangles(indices: &[u32]) -> Vec<(u32, u32)> {
    let mut edges = HashSet::new();
    for tri in indices.chunks_exact(3) {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            edges.insert((a.min(b), a.max(b)));
        }
    }
    let mut edges: Vec<_> = edges.into_iter().collect();
    edges.sort_unstable();
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    fn quad() -> Mesh {
        Mesh {
            vertices: vec![
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 0.0, 0.0),
                vec3(1.0, 1.0, 0.0),
                vec3(0.0, 1.0, 0.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    #[test]
    fn transformed_moves_vertices() {
        let moved = quad().transformed(Mat4::from_translation(vec3(0.0, 0.0, 5.0)));
        assert_eq!(moved.vertices[0], vec3(0.0, 0.0, 5.0));
        assert_eq!(moved.indices, quad().indices);
    }

    #[test]
    fn append_into_offsets_indices() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        quad().append_into(&mut vertices, &mut indices);
        quad().append_into(&mut vertices, &mut indices);
        assert_eq!(vertices.len(), 8);
        assert_eq!(&indices[6..9], &[4, 5, 6]);
    }

    #[test]
    fn triangulated_quad_has_diagonal_edge() {
        // 4 boundary edges + 1 diagonal: exactly why M2 needs dihedral tests.
        assert_eq!(edges_from_triangles(&quad().indices).len(), 5);
    }
}
