//! The `.vec` model format: the engine's native "vector model" — welded
//! vertices, classified edges, and the invisible occluder mesh, in one file.
//!
//! Binary layout (little-endian):
//! ```text
//! "VEC1" magic, u32 version
//! chunks: [u8;4] tag, u32 byte_len, payload   (unknown tags are skipped)
//!   PALT  u32 count, count × 3×f32 linear RGB
//!   VERT  u32 count, count × 3×f32 positions
//!   EDGE  u32 count, count × { u32 a, u32 b, u8 palette, u8 kind, u8 style,
//!                              u8 pad, f32 intensity, 3×f32 n1, 3×f32 n2 }
//!         style bitflags: 1 = dashed, 2 = flicker (0 in pre-style files)
//!   OCCL  u32 count, count × u32 triangle indices into VERT
//!   AABB  6 × f32 (min, max)
//! ```

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

use glam::{Mat4, Vec3, Vec4};

use crate::Segment;

pub const VEC_MAGIC: [u8; 4] = *b"VEC1";
pub const VEC_VERSION: u32 = 1;

/// Refuse to allocate for counts beyond this when reading (corrupt files).
const MAX_COUNT: u32 = 64_000_000;

const EDGE_BYTES: usize = 4 + 4 + 1 + 1 + 2 + 4 + 12 + 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EdgeKind {
    /// Boundary, crease, material-boundary, or authored decor line —
    /// always drawn.
    Always = 0,
    /// Smooth-surface edge: drawn only while it is a silhouette (M4).
    /// Carries both adjacent face normals for the runtime test.
    Smooth = 1,
}

impl EdgeKind {
    fn from_u8(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Self::Always),
            1 => Ok(Self::Smooth),
            other => Err(invalid(format!("unknown edge kind {other}"))),
        }
    }
}

/// Style bit: dashed stroke (world-unit dash period chosen at draw time).
pub const STYLE_DASH: u8 = 1;
/// Style bit: intensity flicker/pulse animation.
pub const STYLE_FLICKER: u8 = 2;

/// Dash period (world units) applied to `STYLE_DASH` edges.
const DASH_PERIOD: f32 = 0.22;
/// Flicker amount applied to `STYLE_FLICKER` edges.
const FLICKER_AMOUNT: f32 = 0.45;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VecEdge {
    pub a: u32,
    pub b: u32,
    pub palette: u8,
    pub kind: EdgeKind,
    /// Bitflags: `STYLE_DASH` | `STYLE_FLICKER`.
    pub style: u8,
    pub intensity: f32,
    /// Adjacent face normals (equal for boundary edges, zero for decor).
    pub n1: Vec3,
    pub n2: Vec3,
}

/// A loaded vector model. Vertices are shared by edges and the occluder.
#[derive(Debug, Clone, PartialEq)]
pub struct VecModel {
    pub palette: Vec<Vec3>,
    pub vertices: Vec<Vec3>,
    pub edges: Vec<VecEdge>,
    /// Occluder triangles, 3 indices each, into `vertices`.
    pub occluder_indices: Vec<u32>,
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
}

impl VecModel {
    /// Materialize edges of one kind as colored segments (styles applied).
    /// `intensity_scale` multiplies each edge's stored intensity.
    pub fn edge_segments(&self, kind: EdgeKind, intensity_scale: f32) -> Vec<Segment> {
        self.edges
            .iter()
            .filter(|edge| edge.kind == kind)
            .map(|edge| self.materialize(edge, intensity_scale))
            .collect()
    }

    fn materialize(&self, edge: &VecEdge, intensity_scale: f32) -> Segment {
        let rgb = self
            .palette
            .get(edge.palette as usize)
            .copied()
            .unwrap_or(Vec3::ONE);
        let mut segment = Segment::new(
            self.vertices[edge.a as usize],
            self.vertices[edge.b as usize],
            Vec4::new(rgb.x, rgb.y, rgb.z, edge.intensity * intensity_scale),
        );
        if edge.style & STYLE_DASH != 0 {
            segment.dash_period = DASH_PERIOD;
        }
        if edge.style & STYLE_FLICKER != 0 {
            segment.flicker = FLICKER_AMOUNT;
        }
        segment
    }

    /// Smooth edges that are silhouettes seen from `eye_world`, transformed
    /// to world space. The test runs in model space (the eye is transformed
    /// in), so stored normals are used as-is; sign products are invariant
    /// under rotation + uniform scale, which is all scenes support.
    ///
    /// An edge is a silhouette when its two faces disagree about facing the
    /// eye: `(n1·v)(n2·v) ≤ 0` with `v` from the edge midpoint to the eye.
    pub fn silhouette_segments(
        &self,
        world_from_model: Mat4,
        eye_world: Vec3,
        intensity_scale: f32,
    ) -> Vec<Segment> {
        let eye = world_from_model.inverse().transform_point3(eye_world);
        self.edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Smooth)
            .filter(|edge| {
                let mid =
                    (self.vertices[edge.a as usize] + self.vertices[edge.b as usize]) * 0.5;
                let v = eye - mid;
                edge.n1.dot(v) * edge.n2.dot(v) <= 0.0
            })
            .map(|edge| {
                let segment = self.materialize(edge, intensity_scale);
                Segment {
                    a: world_from_model.transform_point3(segment.a),
                    b: world_from_model.transform_point3(segment.b),
                    ..segment
                }
            })
            .collect()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut writer = BufWriter::new(File::create(path)?);
        self.save_to(&mut writer)
    }

    pub fn save_to(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&VEC_MAGIC)?;
        w.write_all(&VEC_VERSION.to_le_bytes())?;

        write_chunk(w, *b"PALT", 4 + self.palette.len() * 12, |w| {
            write_u32(w, self.palette.len() as u32)?;
            self.palette.iter().try_for_each(|c| write_vec3(w, *c))
        })?;
        write_chunk(w, *b"VERT", 4 + self.vertices.len() * 12, |w| {
            write_u32(w, self.vertices.len() as u32)?;
            self.vertices.iter().try_for_each(|v| write_vec3(w, *v))
        })?;
        write_chunk(w, *b"EDGE", 4 + self.edges.len() * EDGE_BYTES, |w| {
            write_u32(w, self.edges.len() as u32)?;
            self.edges.iter().try_for_each(|e| {
                write_u32(w, e.a)?;
                write_u32(w, e.b)?;
                w.write_all(&[e.palette, e.kind as u8, e.style, 0])?;
                write_f32(w, e.intensity)?;
                write_vec3(w, e.n1)?;
                write_vec3(w, e.n2)
            })
        })?;
        write_chunk(w, *b"OCCL", 4 + self.occluder_indices.len() * 4, |w| {
            write_u32(w, self.occluder_indices.len() as u32)?;
            self.occluder_indices.iter().try_for_each(|&i| write_u32(w, i))
        })?;
        write_chunk(w, *b"AABB", 24, |w| {
            write_vec3(w, self.aabb_min)?;
            write_vec3(w, self.aabb_max)
        })
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?.read_to_end(&mut bytes)?;
        Self::load_from(&bytes)
    }

    pub fn load_from(bytes: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor { bytes, offset: 0 };
        if cursor.take(4)? != VEC_MAGIC {
            return Err(invalid("not a .vec file (bad magic)"));
        }
        let version = cursor.u32()?;
        if version != VEC_VERSION {
            return Err(invalid(format!("unsupported .vec version {version}")));
        }

        let mut model = VecModel {
            palette: Vec::new(),
            vertices: Vec::new(),
            edges: Vec::new(),
            occluder_indices: Vec::new(),
            aabb_min: Vec3::ZERO,
            aabb_max: Vec3::ZERO,
        };

        while !cursor.at_end() {
            let tag: [u8; 4] = cursor.take(4)?.try_into().unwrap();
            let len = cursor.u32()? as usize;
            let mut chunk = Cursor {
                bytes: cursor.slice(len)?,
                offset: 0,
            };
            match &tag {
                b"PALT" => {
                    model.palette = read_items(&mut chunk, 12, |c| c.vec3())?;
                }
                b"VERT" => {
                    model.vertices = read_items(&mut chunk, 12, |c| c.vec3())?;
                }
                b"EDGE" => {
                    model.edges = read_items(&mut chunk, EDGE_BYTES, |c| {
                        let a = c.u32()?;
                        let b = c.u32()?;
                        let meta = c.take(4)?;
                        Ok(VecEdge {
                            a,
                            b,
                            palette: meta[0],
                            kind: EdgeKind::from_u8(meta[1])?,
                            style: meta[2],
                            intensity: c.f32()?,
                            n1: c.vec3()?,
                            n2: c.vec3()?,
                        })
                    })?;
                }
                b"OCCL" => {
                    model.occluder_indices = read_items(&mut chunk, 4, |c| c.u32())?;
                }
                b"AABB" => {
                    model.aabb_min = chunk.vec3()?;
                    model.aabb_max = chunk.vec3()?;
                }
                _ => {} // unknown chunk: skipped (forward compatibility)
            }
        }

        model.validate()?;
        Ok(model)
    }

    fn validate(&self) -> io::Result<()> {
        let vertex_count = self.vertices.len() as u32;
        for edge in &self.edges {
            if edge.a >= vertex_count || edge.b >= vertex_count {
                return Err(invalid("edge vertex index out of range"));
            }
            if !self.palette.is_empty() && edge.palette as usize >= self.palette.len() {
                return Err(invalid("edge palette index out of range"));
            }
        }
        if !self.occluder_indices.len().is_multiple_of(3) {
            return Err(invalid("occluder indices not a multiple of 3"));
        }
        if self.occluder_indices.iter().any(|&i| i >= vertex_count) {
            return Err(invalid("occluder index out of range"));
        }
        Ok(())
    }
}

/// Axis-aligned bounds of a point set (zeros when empty).
pub fn compute_aabb(points: &[Vec3]) -> (Vec3, Vec3) {
    let mut iter = points.iter();
    let Some(&first) = iter.next() else {
        return (Vec3::ZERO, Vec3::ZERO);
    };
    iter.fold((first, first), |(lo, hi), &p| (lo.min(p), hi.max(p)))
}

// --- io helpers -------------------------------------------------------------

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn write_chunk(
    w: &mut impl Write,
    tag: [u8; 4],
    len: usize,
    body: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    w.write_all(&tag)?;
    write_u32(w, len as u32)?;
    body(w)
}

fn write_u32(w: &mut (impl Write + ?Sized), value: u32) -> io::Result<()> {
    w.write_all(&value.to_le_bytes())
}

fn write_f32(w: &mut (impl Write + ?Sized), value: f32) -> io::Result<()> {
    w.write_all(&value.to_le_bytes())
}

fn write_vec3(w: &mut (impl Write + ?Sized), value: Vec3) -> io::Result<()> {
    for component in value.to_array() {
        write_f32(w, component)?;
    }
    Ok(())
}

fn read_items<T>(
    chunk: &mut Cursor,
    item_bytes: usize,
    mut read: impl FnMut(&mut Cursor) -> io::Result<T>,
) -> io::Result<Vec<T>> {
    let count = chunk.u32()?;
    if count > MAX_COUNT {
        return Err(invalid(format!("count {count} exceeds sanity limit")));
    }
    if chunk.remaining() != count as usize * item_bytes {
        return Err(invalid("chunk length does not match item count"));
    }
    (0..count).map(|_| read(chunk)).collect()
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn at_end(&self) -> bool {
        self.offset >= self.bytes.len()
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    fn slice(&mut self, len: usize) -> io::Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(invalid("unexpected end of file"));
        }
        let slice = &self.bytes[self.offset..self.offset + len];
        self.offset += len;
        Ok(slice)
    }

    fn take(&mut self, len: usize) -> io::Result<&'a [u8]> {
        self.slice(len)
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.slice(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.slice(4)?.try_into().unwrap()))
    }

    fn vec3(&mut self) -> io::Result<Vec3> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    fn sample_model() -> VecModel {
        VecModel {
            palette: vec![vec3(0.05, 1.0, 0.15), vec3(0.05, 0.75, 1.0)],
            vertices: vec![
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 0.0, 0.0),
                vec3(0.0, 1.0, 0.0),
            ],
            edges: vec![
                VecEdge {
                    a: 0,
                    b: 1,
                    palette: 0,
                    kind: EdgeKind::Always,
                    style: STYLE_DASH,
                    intensity: 1.0,
                    n1: Vec3::Z,
                    n2: Vec3::Z,
                },
                VecEdge {
                    a: 1,
                    b: 2,
                    palette: 1,
                    kind: EdgeKind::Smooth,
                    style: 0,
                    intensity: 0.5,
                    n1: Vec3::Z,
                    n2: Vec3::Y,
                },
            ],
            occluder_indices: vec![0, 1, 2],
            aabb_min: Vec3::ZERO,
            aabb_max: vec3(1.0, 1.0, 0.0),
        }
    }

    #[test]
    fn roundtrip_preserves_model() {
        let model = sample_model();
        let mut bytes = Vec::new();
        model.save_to(&mut bytes).unwrap();
        assert_eq!(VecModel::load_from(&bytes).unwrap(), model);
    }

    #[test]
    fn unknown_chunks_are_skipped() {
        let model = sample_model();
        let mut bytes = Vec::new();
        model.save_to(&mut bytes).unwrap();
        // Append a future chunk type; the loader must ignore it.
        bytes.extend_from_slice(b"CHAI");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(VecModel::load_from(&bytes).unwrap(), model);
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(VecModel::load_from(b"NOPE").is_err());
        let mut bytes = Vec::new();
        sample_model().save_to(&mut bytes).unwrap();
        assert!(VecModel::load_from(&bytes[..bytes.len() - 3]).is_err());
    }

    #[test]
    fn rejects_out_of_range_indices() {
        let mut model = sample_model();
        model.edges[0].a = 99;
        let mut bytes = Vec::new();
        model.save_to(&mut bytes).unwrap();
        assert!(VecModel::load_from(&bytes).is_err());
    }

    #[test]
    fn edge_segments_filter_by_kind_and_apply_palette() {
        let model = sample_model();
        let always = model.edge_segments(EdgeKind::Always, 1.0);
        assert_eq!(always.len(), 1);
        assert_eq!(always[0].color.truncate(), model.palette[0]);
        let smooth = model.edge_segments(EdgeKind::Smooth, 2.0);
        assert_eq!(smooth.len(), 1);
        assert!((smooth[0].color.w - 1.0).abs() < 1e-6, "0.5 × 2.0");
    }

    #[test]
    fn aabb_of_points() {
        let (lo, hi) = compute_aabb(&[vec3(1.0, -2.0, 3.0), vec3(-1.0, 5.0, 0.0)]);
        assert_eq!(lo, vec3(-1.0, -2.0, 0.0));
        assert_eq!(hi, vec3(1.0, 5.0, 3.0));
    }

    #[test]
    fn dash_style_survives_roundtrip_and_materialization() {
        let model = sample_model();
        let mut bytes = Vec::new();
        model.save_to(&mut bytes).unwrap();
        let loaded = VecModel::load_from(&bytes).unwrap();
        assert_eq!(loaded.edges[0].style, STYLE_DASH);
        let segments = loaded.edge_segments(EdgeKind::Always, 1.0);
        assert!(segments[0].dash_period > 0.0);
        assert_eq!(segments[0].flicker, 0.0);
    }

    /// Two triangles folded along the Y axis like a roof ridge, normals
    /// +Z-ish and −Z-ish: viewed from ±X the ridge is a silhouette; viewed
    /// head-on from +Z both faces agree and it is not.
    #[test]
    fn silhouette_flips_with_viewpoint() {
        let model = VecModel {
            palette: vec![Vec3::ONE],
            vertices: vec![vec3(0.0, -1.0, 0.0), vec3(0.0, 1.0, 0.0)],
            edges: vec![VecEdge {
                a: 0,
                b: 1,
                palette: 0,
                kind: EdgeKind::Smooth,
                style: 0,
                intensity: 1.0,
                n1: vec3(0.5, 0.0, 0.87),
                n2: vec3(-0.5, 0.0, 0.87),
            }],
            occluder_indices: vec![],
            aabb_min: vec3(0.0, -1.0, 0.0),
            aabb_max: vec3(0.0, 1.0, 0.0),
        };
        let from_side = model.silhouette_segments(Mat4::IDENTITY, vec3(5.0, 0.0, 0.5), 1.0);
        assert_eq!(from_side.len(), 1, "side view: faces disagree → silhouette");
        let head_on = model.silhouette_segments(Mat4::IDENTITY, vec3(0.0, 0.0, 5.0), 1.0);
        assert!(head_on.is_empty(), "head-on: both faces toward eye");
        // Rotating the instance 90° about Y turns the head-on eye into a
        // side view — the model-space test must respect the transform.
        let rotated = model.silhouette_segments(
            Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
            vec3(0.0, 0.0, 5.0),
            1.0,
        );
        assert_eq!(rotated.len(), 1);
    }
}
