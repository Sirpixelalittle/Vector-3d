//! The editor's document: a list of colored primitive shapes plus post
//! settings, saved as hand-editable RON. Baking runs the shapes through
//! the same weld/classify pipeline as Blender content (`vex-convert`),
//! so the export is a first-class `.vec`: occluders double as collision,
//! cylinders get runtime silhouettes, and overlapping composites weld
//! into clean outlines.

use std::path::Path;

use anyhow::{Context, Result};
use glam::{Mat4, Vec3, vec3};
use serde::{Deserialize, Serialize};
use vex_convert::{ConvertOptions, ConvertStats, SourceGeometry, build_model};
use vex_core::VecModel;

/// `.vec` palettes index with a u8.
pub const PALETTE_CAP: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Box,
    Cylinder,
    Wedge,
    Frame,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 4] = [
        ShapeKind::Box,
        ShapeKind::Cylinder,
        ShapeKind::Wedge,
        ShapeKind::Frame,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Box => "BOX",
            ShapeKind::Cylinder => "CYLINDER",
            ShapeKind::Wedge => "WEDGE",
            ShapeKind::Frame => "FRAME",
        }
    }
}

/// One placed primitive. `pos` is the center of the shape's *base* (its
/// bottom sits at `pos.y`), `size` its bounding box, `yaw` in degrees.
/// Color is authored as hue/saturation so the editor can step them; the
/// glow multiplier is premultiplied into HDR palette RGB at bake time —
/// exactly how Blender emissive strength ships through the converter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    pub pos: (f32, f32, f32),
    pub size: (f32, f32, f32),
    #[serde(default)]
    pub yaw: f32,
    #[serde(default = "default_hue")]
    pub hue: f32,
    #[serde(default = "default_sat")]
    pub sat: f32,
    #[serde(default = "default_glow")]
    pub glow: f32,
}

fn default_hue() -> f32 {
    120.0
}
fn default_sat() -> f32 {
    0.85
}
fn default_glow() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PostDef {
    pub exposure: f32,
    pub bloom_strength: f32,
    pub crt: f32,
    pub glow: f32,
}

impl Default for PostDef {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            bloom_strength: 0.14,
            crt: 0.0,
            glow: 0.5,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Level {
    #[serde(default)]
    pub post: PostDef,
    #[serde(default)]
    pub shapes: Vec<Shape>,
}

impl Level {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        ron::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let pretty = ron::ser::PrettyConfig::new().depth_limit(3);
        let text = ron::ser::to_string_pretty(self, pretty).context("serialize level")?;
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Distinct quantized (hue, sat, glow) combos in use.
    pub fn palette_len(&self) -> usize {
        let mut keys: Vec<(i32, i32, i32)> = self.shapes.iter().map(color_key).collect();
        keys.sort_unstable();
        keys.dedup();
        keys.len()
    }
}

pub fn hsv_to_rgb(hue_deg: f32, sat: f32, value: f32) -> Vec3 {
    let h = hue_deg.rem_euclid(360.0) / 60.0;
    let c = value * sat;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    vec3(r, g, b) + Vec3::splat(value - c)
}

fn color_key(shape: &Shape) -> (i32, i32, i32) {
    (
        (shape.hue.rem_euclid(360.0) * 10.0).round() as i32,
        (shape.sat * 100.0).round() as i32,
        (shape.glow * 100.0).round() as i32,
    )
}

fn quad(tris: &mut Vec<Vec3>, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
    tris.extend([a, b, c, a, c, d]);
}

fn boxy(tris: &mut Vec<Vec3>, min: Vec3, max: Vec3) {
    let (a, b) = (min, max);
    let p = |x: f32, y: f32, z: f32| vec3(x, y, z);
    // Outward CCW faces.
    quad(tris, p(a.x, a.y, b.z), p(b.x, a.y, b.z), p(b.x, b.y, b.z), p(a.x, b.y, b.z)); // +Z
    quad(tris, p(b.x, a.y, a.z), p(a.x, a.y, a.z), p(a.x, b.y, a.z), p(b.x, b.y, a.z)); // -Z
    quad(tris, p(b.x, a.y, b.z), p(b.x, a.y, a.z), p(b.x, b.y, a.z), p(b.x, b.y, b.z)); // +X
    quad(tris, p(a.x, a.y, a.z), p(a.x, a.y, b.z), p(a.x, b.y, b.z), p(a.x, b.y, a.z)); // -X
    quad(tris, p(a.x, b.y, b.z), p(b.x, b.y, b.z), p(b.x, b.y, a.z), p(a.x, b.y, a.z)); // +Y
    quad(tris, p(a.x, a.y, a.z), p(b.x, a.y, a.z), p(b.x, a.y, b.z), p(a.x, a.y, b.z)); // -Y
}

/// Local-space triangles for a shape (base centered at origin, bottom at
/// y = 0), 3 corners per triangle.
fn local_triangles(kind: ShapeKind, size: Vec3) -> Vec<Vec3> {
    let (sx, sy, sz) = (size.x, size.y, size.z);
    let (hx, hz) = (sx * 0.5, sz * 0.5);
    let mut tris = Vec::new();
    match kind {
        ShapeKind::Box => boxy(&mut tris, vec3(-hx, 0.0, -hz), vec3(hx, sy, hz)),
        ShapeKind::Cylinder => {
            // Elliptical prism: sx/sz are the footprint diameters. The
            // classifier turns the smooth sides into runtime silhouettes.
            const N: usize = 12;
            let ring: Vec<(f32, f32)> = (0..N)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / N as f32;
                    (hx * a.cos(), hz * a.sin())
                })
                .collect();
            for i in 0..N {
                let (x0, z0) = ring[i];
                let (x1, z1) = ring[(i + 1) % N];
                // Ring runs counter-clockwise seen from above (+Y), so the
                // outward side face is bottom0 → top0 → top1 → bottom1.
                quad(
                    &mut tris,
                    vec3(x0, 0.0, z0),
                    vec3(x0, sy, z0),
                    vec3(x1, sy, z1),
                    vec3(x1, 0.0, z1),
                );
                tris.extend([vec3(0.0, sy, 0.0), vec3(x1, sy, z1), vec3(x0, sy, z0)]);
                tris.extend([vec3(0.0, 0.0, 0.0), vec3(x0, 0.0, z0), vec3(x1, 0.0, z1)]);
            }
        }
        ShapeKind::Wedge => {
            // Ramp rising toward +Z: full-height back face at +Z.
            let (b0, b1, b2, b3) = (
                vec3(-hx, 0.0, -hz),
                vec3(hx, 0.0, -hz),
                vec3(hx, 0.0, hz),
                vec3(-hx, 0.0, hz),
            );
            let (t2, t3) = (vec3(hx, sy, hz), vec3(-hx, sy, hz));
            quad(&mut tris, b0, b1, b2, b3); // bottom (faces down: CCW from below)
            quad(&mut tris, b2, t2, t3, b3); // back wall at +Z... faces +Z
            quad(&mut tris, b0, b1, t2, t3); // slope, faces up/-Z
            tris.extend([b1, b2, t2]); // +X side
            tris.extend([b0, t3, b3]); // -X side
        }
        ShapeKind::Frame => {
            // Doorway: two posts + lintel, overlapping at the corners —
            // welding and the coplanar drop clean the seams (the
            // healthpack trick).
            let t = (0.15 * sx.min(sy)).clamp(0.12, 0.45);
            boxy(&mut tris, vec3(-hx, 0.0, -hz), vec3(-hx + t, sy, hz));
            boxy(&mut tris, vec3(hx - t, 0.0, -hz), vec3(hx, sy, hz));
            boxy(&mut tris, vec3(-hx, sy - t, -hz), vec3(hx, sy, hz));
        }
    }
    tris
}

/// World-space triangles for a placed shape.
pub fn shape_triangles(shape: &Shape) -> Vec<Vec3> {
    let size = vec3(shape.size.0.max(0.01), shape.size.1.max(0.01), shape.size.2.max(0.01));
    let transform = Mat4::from_translation(vec3(shape.pos.0, shape.pos.1, shape.pos.2))
        * Mat4::from_rotation_y(shape.yaw.to_radians());
    local_triangles(shape.kind, size)
        .into_iter()
        .map(|p| transform.transform_point3(p))
        .collect()
}

/// World AABB of a placed shape (for aim picking and highlights).
pub fn shape_aabb(shape: &Shape) -> (Vec3, Vec3) {
    let tris = shape_triangles(shape);
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in tris {
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

/// Flatten the level into converter input. Palette entries are HDR:
/// `hsv * glow`, deduplicated across shapes. Errors when the level needs
/// more than 255 distinct color combos (the `.vec` palette limit).
pub fn build_source(level: &Level) -> Result<SourceGeometry> {
    let mut palette: Vec<Vec3> = Vec::new();
    let mut keys: Vec<(i32, i32, i32)> = Vec::new();
    let mut tri_positions = Vec::new();
    let mut tri_palette = Vec::new();
    for shape in &level.shapes {
        let key = color_key(shape);
        let index = match keys.iter().position(|&k| k == key) {
            Some(i) => i,
            None => {
                anyhow::ensure!(
                    palette.len() < PALETTE_CAP,
                    "palette full: more than {PALETTE_CAP} distinct hue/sat/glow combos"
                );
                keys.push(key);
                palette.push(hsv_to_rgb(shape.hue, shape.sat, 1.0) * shape.glow);
                palette.len() - 1
            }
        };
        let tris = shape_triangles(shape);
        tri_palette.extend(std::iter::repeat_n(index as u8, tris.len() / 3));
        tri_positions.extend(tris);
    }
    Ok(SourceGeometry {
        tri_positions,
        tri_palette,
        tri_style: Vec::new(),
        line_positions: Vec::new(),
        line_palette: Vec::new(),
        line_style: Vec::new(),
        palette,
    })
}

/// Bake the whole level into a renderable / exportable model.
pub fn bake(level: &Level) -> Result<(VecModel, ConvertStats)> {
    let source = build_source(level)?;
    Ok(build_model(&source, &ConvertOptions::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vex_core::EdgeKind;

    fn shape(kind: ShapeKind) -> Shape {
        Shape {
            kind,
            pos: (1.0, 0.0, -2.0),
            size: (2.0, 3.0, 1.0),
            yaw: 30.0,
            hue: 200.0,
            sat: 0.8,
            glow: 1.5,
        }
    }

    #[test]
    fn every_kind_bakes_to_edges_and_occluders() {
        for kind in ShapeKind::ALL {
            let level = Level {
                shapes: vec![shape(kind)],
                ..Default::default()
            };
            let (model, stats) = bake(&level).unwrap();
            assert!(!model.edges.is_empty(), "{kind:?} produced no edges");
            assert!(
                !model.occluder_indices.is_empty(),
                "{kind:?} produced no occluders"
            );
            assert_eq!(stats.degenerate_triangles, 0, "{kind:?} degenerate tris");
        }
    }

    #[test]
    fn cylinder_sides_become_runtime_silhouettes() {
        let level = Level {
            shapes: vec![shape(ShapeKind::Cylinder)],
            ..Default::default()
        };
        let (model, _) = bake(&level).unwrap();
        assert!(
            model.edges.iter().any(|e| e.kind == EdgeKind::Smooth),
            "cylinder should have smooth (silhouette) edges"
        );
        assert!(
            model.edges.iter().any(|e| e.kind == EdgeKind::Always),
            "cylinder rims should be always-drawn creases"
        );
    }

    #[test]
    fn box_is_twelve_crease_edges() {
        let level = Level {
            shapes: vec![Shape {
                yaw: 0.0,
                ..shape(ShapeKind::Box)
            }],
            ..Default::default()
        };
        let (model, stats) = bake(&level).unwrap();
        assert_eq!(stats.welded_vertices, 8);
        assert_eq!(model.edges.len(), 12);
    }

    #[test]
    fn palette_dedups_and_premultiplies_glow() {
        let mut level = Level::default();
        level.shapes.push(shape(ShapeKind::Box));
        level.shapes.push(Shape {
            pos: (8.0, 0.0, 0.0),
            ..shape(ShapeKind::Wedge)
        });
        level.shapes.push(Shape {
            hue: 0.0,
            glow: 2.0,
            sat: 1.0,
            ..shape(ShapeKind::Box)
        });
        let source = build_source(&level).unwrap();
        assert_eq!(source.palette.len(), 2, "same combo shares an entry");
        let red = source.palette[1];
        assert!((red.x - 2.0).abs() < 1e-4, "glow premultiplied into HDR red");
        assert_eq!(level.palette_len(), 2);
    }

    #[test]
    fn save_load_roundtrip() {
        let level = Level {
            shapes: vec![shape(ShapeKind::Frame)],
            post: PostDef {
                glow: 0.7,
                ..Default::default()
            },
        };
        let dir = std::env::temp_dir().join("vex-editor-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.ron");
        level.save(&path).unwrap();
        let loaded = Level::load(&path).unwrap();
        assert_eq!(loaded.shapes.len(), 1);
        assert_eq!(loaded.shapes[0].kind, ShapeKind::Frame);
        assert!((loaded.post.glow - 0.7).abs() < 1e-6);
        assert!((loaded.shapes[0].size.1 - 3.0).abs() < 1e-6);
    }

    #[test]
    fn hsv_hits_the_primaries() {
        assert!((hsv_to_rgb(0.0, 1.0, 1.0) - vec3(1.0, 0.0, 0.0)).length() < 1e-4);
        assert!((hsv_to_rgb(120.0, 1.0, 1.0) - vec3(0.0, 1.0, 0.0)).length() < 1e-4);
        assert!((hsv_to_rgb(240.0, 1.0, 1.0) - vec3(0.0, 0.0, 1.0)).length() < 1e-4);
        assert!((hsv_to_rgb(180.0, 0.0, 1.0) - Vec3::ONE).length() < 1e-4);
    }
}
