//! glTF 2.0 → `SourceGeometry`. Loads buffers only (never images —
//! textures are meaningless to a line renderer). Node transforms are baked
//! into world-space positions, so face normals computed later handle
//! non-uniform scale correctly for free.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use glam::{Mat4, Vec3};

use crate::classify::SourceGeometry;

pub fn load_gltf(path: &Path) -> Result<SourceGeometry> {
    let mut gltf =
        gltf::Gltf::open(path).with_context(|| format!("open {}", path.display()))?;
    let blob = gltf.blob.take();
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let buffers = load_buffers(&gltf.document, blob, base)?;

    let mut builder = Builder::default();

    // Default scene if present, else every scene in the file.
    let scenes: Vec<_> = match gltf.document.default_scene() {
        Some(scene) => vec![scene],
        None => gltf.document.scenes().collect(),
    };
    for scene in scenes {
        for node in scene.nodes() {
            builder.visit_node(&node, Mat4::IDENTITY, &buffers)?;
        }
    }

    if builder.source.tri_positions.is_empty() && builder.source.line_positions.is_empty() {
        bail!("no triangle or line geometry found in {}", path.display());
    }
    Ok(builder.source)
}

#[derive(Default)]
struct Builder {
    source: SourceGeometry,
    palette_lookup: HashMap<[u16; 3], u8>,
}

impl Builder {
    fn visit_node(
        &mut self,
        node: &gltf::Node,
        parent: Mat4,
        buffers: &[Vec<u8>],
    ) -> Result<()> {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                self.add_primitive(&primitive, world, buffers)?;
            }
        }
        for child in node.children() {
            self.visit_node(&child, world, buffers)?;
        }
        Ok(())
    }

    fn add_primitive(
        &mut self,
        primitive: &gltf::Primitive,
        world: Mat4,
        buffers: &[Vec<u8>],
    ) -> Result<()> {
        use gltf::mesh::Mode;

        let material = primitive.material();
        let palette = self.intern_color(material_color(&material))?;
        let style = material_style(&material);
        let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
        let Some(positions) = reader.read_positions() else {
            log::warn!("primitive without positions skipped");
            return Ok(());
        };
        let positions: Vec<Vec3> = positions
            .map(|p| world.transform_point3(Vec3::from(p)))
            .collect();
        let indices: Vec<u32> = match reader.read_indices() {
            Some(indices) => indices.into_u32().collect(),
            None => (0..positions.len() as u32).collect(),
        };
        let at = |i: u32| positions[i as usize];

        match primitive.mode() {
            Mode::Triangles => {
                for tri in indices.chunks_exact(3) {
                    self.source
                        .tri_positions
                        .extend([at(tri[0]), at(tri[1]), at(tri[2])]);
                    self.source.tri_palette.push(palette);
                    self.source.tri_style.push(style);
                }
            }
            Mode::Lines => {
                for pair in indices.chunks_exact(2) {
                    self.push_line(at(pair[0]), at(pair[1]), palette, style);
                }
            }
            Mode::LineStrip => {
                for pair in indices.windows(2) {
                    self.push_line(at(pair[0]), at(pair[1]), palette, style);
                }
            }
            Mode::LineLoop => {
                for pair in indices.windows(2) {
                    self.push_line(at(pair[0]), at(pair[1]), palette, style);
                }
                if indices.len() > 2 {
                    self.push_line(
                        at(indices[indices.len() - 1]),
                        at(indices[0]),
                        palette,
                        style,
                    );
                }
            }
            other => log::warn!("skipping unsupported primitive mode {other:?}"),
        }
        Ok(())
    }

    fn push_line(&mut self, a: Vec3, b: Vec3, palette: u8, style: u8) {
        self.source.line_positions.push((a, b));
        self.source.line_palette.push(palette);
        self.source.line_style.push(style);
    }

    /// Register a color in the palette (quantized for dedup).
    fn intern_color(&mut self, color: Vec3) -> Result<u8> {
        let key = color.to_array().map(|c| (c.clamp(0.0, 64.0) * 1023.0) as u16);
        if let Some(&index) = self.palette_lookup.get(&key) {
            return Ok(index);
        }
        if self.source.palette.len() >= u8::MAX as usize {
            bail!("more than 255 distinct material colors — palette byte overflow");
        }
        let index = self.source.palette.len() as u8;
        self.source.palette.push(color);
        self.palette_lookup.insert(key, index);
        Ok(index)
    }
}

/// Styling by material-name convention: a material named e.g. `trim-dash`
/// or `sign_flicker` marks its edges dashed / flickering. Cheap to author
/// in Blender, no custom exporter plumbing.
fn material_style(material: &gltf::Material) -> u8 {
    let Some(name) = material.name() else {
        return 0;
    };
    let name = name.to_ascii_lowercase();
    let mut style = 0;
    if name.contains("dash") {
        style |= vex_core::STYLE_DASH;
    }
    if name.contains("flicker") {
        style |= vex_core::STYLE_FLICKER;
    }
    style
}

/// Emissive color wins (neon assets author glow there); base color
/// otherwise. Blender's emissive-strength slider
/// (KHR_materials_emissive_strength) scales into HDR — values above 1.0
/// bloom in the M5 post chain.
fn material_color(material: &gltf::Material) -> Vec3 {
    let emissive = Vec3::from(material.emissive_factor());
    if emissive != Vec3::ZERO {
        return emissive * material.emissive_strength().unwrap_or(1.0);
    }
    let base = material.pbr_metallic_roughness().base_color_factor();
    Vec3::new(base[0], base[1], base[2])
}

fn load_buffers(
    document: &gltf::Document,
    mut blob: Option<Vec<u8>>,
    base: &Path,
) -> Result<Vec<Vec<u8>>> {
    document
        .buffers()
        .map(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => blob
                .take()
                .context("glTF declares a BIN buffer but the file has no binary chunk"),
            gltf::buffer::Source::Uri(uri) => {
                if let Some(encoded) = uri
                    .strip_prefix("data:")
                    .and_then(|rest| rest.split_once("base64,"))
                    .map(|(_, data)| data)
                {
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .context("decode base64 buffer")
                } else {
                    std::fs::read(base.join(uri))
                        .with_context(|| format!("read buffer file {uri}"))
                }
            }
        })
        .collect()
}
