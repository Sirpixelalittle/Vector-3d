//! The heart of the converter: weld vertices, build edge→face adjacency,
//! and classify every edge of the source geometry.
//!
//! ```text
//! 1 adjacent face                      → BOUNDARY  (always drawn)
//! adjacent faces differ in palette     → MATERIAL  (always drawn)
//! dihedral angle > crease threshold    → CREASE    (always drawn)
//! dihedral angle ≈ 0 (coplanar)        → dropped   (can never silhouette)
//! otherwise                            → SMOOTH    (silhouette candidate)
//! authored line primitives             → DECOR     (always drawn)
//! ```

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use vex_core::{EdgeKind, VecEdge, VecModel, compute_aabb};

/// Weld epsilon as a fraction of the bounding-box diagonal.
const WELD_EPSILON_FACTOR: f32 = 1e-5;
/// Smooth edges flatter than this can never become silhouettes — dropped.
const COPLANAR_DROP_DEG: f32 = 1.0;
/// Cross-product length below which a triangle counts as degenerate.
const DEGENERATE_AREA_EPS: f32 = 1e-12;

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    /// Dihedral angle (degrees) above which an edge is a crease.
    pub crease_angle_deg: f32,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            crease_angle_deg: 30.0,
        }
    }
}

/// Flattened world-space source geometry, ready for classification.
/// (The glTF loader produces this; tests construct it directly.)
#[derive(Debug, Default, Clone)]
pub struct SourceGeometry {
    /// Triangle corners, 3 per triangle.
    pub tri_positions: Vec<Vec3>,
    /// Palette index per triangle.
    pub tri_palette: Vec<u8>,
    /// Style bits per triangle (STYLE_DASH/STYLE_FLICKER); empty = solid.
    pub tri_style: Vec<u8>,
    /// Authored decor lines (glTF LINES/LINE_STRIP/LINE_LOOP primitives).
    pub line_positions: Vec<(Vec3, Vec3)>,
    /// Palette index per decor line.
    pub line_palette: Vec<u8>,
    /// Style bits per decor line; empty = solid.
    pub line_style: Vec<u8>,
    pub palette: Vec<Vec3>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConvertStats {
    pub source_vertices: usize,
    pub welded_vertices: usize,
    pub triangles: usize,
    pub degenerate_triangles: usize,
    pub boundary: usize,
    pub crease: usize,
    pub material: usize,
    pub non_manifold: usize,
    pub smooth: usize,
    pub dropped_coplanar: usize,
    pub decor: usize,
}

impl ConvertStats {
    pub fn always_edges(&self) -> usize {
        self.boundary + self.crease + self.material + self.non_manifold + self.decor
    }
}

/// Classify source geometry into a `.vec` model.
pub fn build_model(source: &SourceGeometry, options: &ConvertOptions) -> (VecModel, ConvertStats) {
    let mut stats = ConvertStats {
        source_vertices: source.tri_positions.len() + source.line_positions.len() * 2,
        ..Default::default()
    };

    let welded = weld(source);
    stats.welded_vertices = welded.vertices.len();

    // Per-triangle geometric normals; degenerate triangles are excluded from
    // both classification and the occluder.
    let mut occluder_indices = Vec::new();
    let mut faces = Vec::new(); // (normal, palette, style)
    let mut adjacency: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (tri, corners) in welded.tri_indices.chunks_exact(3).enumerate() {
        let [a, b, c] = [corners[0], corners[1], corners[2]];
        let cross = (welded.vertices[b as usize] - welded.vertices[a as usize])
            .cross(welded.vertices[c as usize] - welded.vertices[a as usize]);
        if a == b || b == c || a == c || cross.length_squared() < DEGENERATE_AREA_EPS {
            stats.degenerate_triangles += 1;
            continue;
        }
        stats.triangles += 1;
        let face_id = faces.len() as u32;
        let style = source.tri_style.get(tri).copied().unwrap_or(0);
        faces.push((cross.normalize(), source.tri_palette[tri], style));
        occluder_indices.extend_from_slice(&[a, b, c]);
        for (u, v) in [(a, b), (b, c), (c, a)] {
            adjacency.entry((u.min(v), u.max(v))).or_default().push(face_id);
        }
    }

    // Deterministic order regardless of hash-map iteration.
    let mut edge_keys: Vec<_> = adjacency.keys().copied().collect();
    edge_keys.sort_unstable();

    let crease_cos = options.crease_angle_deg.to_radians().cos();
    let coplanar_cos = COPLANAR_DROP_DEG.to_radians().cos();

    let mut edges = Vec::new();
    for (a, b) in edge_keys {
        let adjacent = &adjacency[&(a, b)];
        let (n1, palette1, style1) = faces[adjacent[0] as usize];
        let edge = match adjacent.len() {
            1 => {
                stats.boundary += 1;
                make_edge(a, b, palette1, style1, EdgeKind::Always, n1, n1)
            }
            2 => {
                let (n2, palette2, _) = faces[adjacent[1] as usize];
                let alignment = n1.dot(n2).clamp(-1.0, 1.0);
                if palette1 != palette2 {
                    stats.material += 1;
                    make_edge(a, b, palette1, style1, EdgeKind::Always, n1, n2)
                } else if alignment < crease_cos {
                    stats.crease += 1;
                    make_edge(a, b, palette1, style1, EdgeKind::Always, n1, n2)
                } else if alignment >= coplanar_cos {
                    stats.dropped_coplanar += 1;
                    continue;
                } else {
                    stats.smooth += 1;
                    make_edge(a, b, palette1, style1, EdgeKind::Smooth, n1, n2)
                }
            }
            _ => {
                stats.non_manifold += 1;
                let (n2, _, _) = faces[adjacent[1] as usize];
                make_edge(a, b, palette1, style1, EdgeKind::Always, n1, n2)
            }
        };
        edges.push(edge);
    }

    // Decor lines pass straight through; duplicates of existing edges are
    // dropped so additive blending doesn't double-brighten them.
    let mut seen: HashSet<(u32, u32)> = edges.iter().map(|e| (e.a, e.b)).collect();
    for (i, &(a, b)) in welded.line_indices.iter().enumerate() {
        if a == b || !seen.insert((a.min(b), a.max(b))) {
            continue;
        }
        stats.decor += 1;
        edges.push(make_edge(
            a.min(b),
            a.max(b),
            source.line_palette[i],
            source.line_style.get(i).copied().unwrap_or(0),
            EdgeKind::Always,
            Vec3::ZERO,
            Vec3::ZERO,
        ));
    }

    edges.sort_unstable_by_key(|e| (e.a, e.b));

    let (aabb_min, aabb_max) = compute_aabb(&welded.vertices);
    let model = VecModel {
        palette: source.palette.clone(),
        vertices: welded.vertices,
        edges,
        occluder_indices,
        aabb_min,
        aabb_max,
    };
    (model, stats)
}

fn make_edge(
    a: u32,
    b: u32,
    palette: u8,
    style: u8,
    kind: EdgeKind,
    n1: Vec3,
    n2: Vec3,
) -> VecEdge {
    VecEdge {
        a,
        b,
        palette,
        kind,
        style,
        intensity: 1.0,
        n1,
        n2,
    }
}

struct Welded {
    vertices: Vec<Vec3>,
    tri_indices: Vec<u32>,
    line_indices: Vec<(u32, u32)>,
}

/// Merge positions closer than ε (a fraction of the bbox diagonal) so that
/// triangles exported with duplicated vertices (UV seams, flat shading)
/// share edges again.
fn weld(source: &SourceGeometry) -> Welded {
    let all_points = source
        .tri_positions
        .iter()
        .chain(source.line_positions.iter().flat_map(|(a, b)| [a, b]));
    let (lo, hi) = compute_aabb(&all_points.copied().collect::<Vec<_>>());
    let epsilon = ((hi - lo).length() * WELD_EPSILON_FACTOR).max(1e-8);

    let mut lookup: HashMap<[i64; 3], u32> = HashMap::new();
    let mut vertices = Vec::new();
    let mut intern = |p: Vec3| -> u32 {
        let key = [
            (p.x / epsilon).round() as i64,
            (p.y / epsilon).round() as i64,
            (p.z / epsilon).round() as i64,
        ];
        *lookup.entry(key).or_insert_with(|| {
            vertices.push(p);
            vertices.len() as u32 - 1
        })
    };

    let tri_indices = source.tri_positions.iter().map(|&p| intern(p)).collect();
    let line_indices = source
        .line_positions
        .iter()
        .map(|&(a, b)| (intern(a), intern(b)))
        .collect();
    Welded {
        vertices,
        tri_indices,
        line_indices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;
    use vex_core::shapes;

    /// Explode a shape's indexed mesh into a triangle soup with duplicated
    /// vertices — what a naive exporter produces. Welding must undo it.
    fn soup(shape: &vex_core::Shape, palette: u8) -> SourceGeometry {
        let mut source = SourceGeometry {
            palette: vec![vec3(0.0, 1.0, 0.0), vec3(0.0, 0.5, 1.0)],
            ..Default::default()
        };
        for &i in &shape.mesh.indices {
            source.tri_positions.push(shape.mesh.vertices[i as usize]);
        }
        source.tri_palette = vec![palette; shape.mesh.indices.len() / 3];
        source
    }

    #[test]
    fn cube_soup_yields_12_crease_edges_and_no_smooth() {
        let source = soup(&shapes::cube(1.0), 0);
        let (model, stats) = build_model(&source, &ConvertOptions::default());
        assert_eq!(stats.welded_vertices, 8, "soup of 36 welds back to 8");
        assert_eq!(stats.crease, 12);
        assert_eq!(stats.boundary, 0);
        // The 6 face diagonals are coplanar → dropped, not smooth.
        assert_eq!(stats.dropped_coplanar, 6);
        assert_eq!(stats.smooth, 0);
        assert_eq!(model.edges.len(), 12);
        assert_eq!(model.occluder_indices.len(), 36);
    }

    #[test]
    fn icosahedron_edges_are_all_creases_at_30_degrees() {
        // Icosahedron dihedral ≈ 138.2° → face-normal angle ≈ 41.8°.
        let source = soup(&shapes::icosahedron(1.0), 0);
        let (_, stats) = build_model(&source, &ConvertOptions::default());
        assert_eq!(stats.crease, 30);
        assert_eq!(stats.smooth, 0);

        // ... but with a 60° threshold they all become smooth candidates.
        let options = ConvertOptions {
            crease_angle_deg: 60.0,
        };
        let (_, stats) = build_model(&source, &options);
        assert_eq!(stats.crease, 0);
        assert_eq!(stats.smooth, 30);
    }

    #[test]
    fn open_plane_has_boundary_edges_only() {
        let source = SourceGeometry {
            tri_positions: vec![
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 0.0, 0.0),
                vec3(1.0, 1.0, 0.0),
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 1.0, 0.0),
                vec3(0.0, 1.0, 0.0),
            ],
            tri_palette: vec![0, 0],
            palette: vec![Vec3::ONE],
            ..Default::default()
        };
        let (_, stats) = build_model(&source, &ConvertOptions::default());
        assert_eq!(stats.boundary, 4);
        assert_eq!(stats.dropped_coplanar, 1, "the quad diagonal");
        assert_eq!(stats.smooth, 0);
    }

    #[test]
    fn material_boundary_beats_flatness() {
        // Two coplanar quads... two coplanar triangles with different
        // palettes: the shared edge must draw even though it is flat.
        let source = SourceGeometry {
            tri_positions: vec![
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 0.0, 0.0),
                vec3(1.0, 1.0, 0.0),
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 1.0, 0.0),
                vec3(0.0, 1.0, 0.0),
            ],
            tri_palette: vec![0, 1],
            palette: vec![vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)],
            ..Default::default()
        };
        let (_, stats) = build_model(&source, &ConvertOptions::default());
        assert_eq!(stats.material, 1);
        assert_eq!(stats.boundary, 4);
    }

    #[test]
    fn decor_lines_pass_through_and_dedup_against_mesh_edges() {
        let mut source = soup(&shapes::cube(1.0), 0);
        // One brand-new decor line, one duplicating an existing cube edge.
        source.line_positions.push((
            vec3(-0.5, 1.0, -0.5),
            vec3(0.5, 1.0, 0.5),
        ));
        source.line_positions.push((
            vec3(-1.0, -1.0, -1.0),
            vec3(1.0, -1.0, -1.0),
        ));
        source.line_palette = vec![1, 1];
        let (model, stats) = build_model(&source, &ConvertOptions::default());
        assert_eq!(stats.decor, 1, "duplicate of a mesh edge is dropped");
        assert_eq!(model.edges.len(), 13);
    }

    #[test]
    fn degenerate_triangles_are_skipped_without_panicking() {
        let source = SourceGeometry {
            tri_positions: vec![
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 0.0, 0.0),
                vec3(2.0, 0.0, 0.0), // collinear
                vec3(0.0, 0.0, 0.0),
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 1.0, 1.0), // collapsed
            ],
            tri_palette: vec![0, 0],
            palette: vec![Vec3::ONE],
            ..Default::default()
        };
        let (model, stats) = build_model(&source, &ConvertOptions::default());
        assert_eq!(stats.degenerate_triangles, 2);
        assert_eq!(stats.triangles, 0);
        assert!(model.occluder_indices.is_empty());
    }
}
