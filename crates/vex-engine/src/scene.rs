//! RON scene format + baking. A scene references `.vec` models and places
//! instances; baking flattens everything into world space once (levels are
//! static), recording per-instance buffer ranges so rendering can
//! frustum-cull by drawing only visible slices.
//!
//! ```ron
//! (
//!     models: { "corridor": "corridor.vec", "plant": "plant.vec" },
//!     instances: [
//!         (model: "corridor"),
//!         (model: "plant", position: (2.6, 0.0, 7.0), yaw_deg: 40.0,
//!          tint: (1.0, 0.2, 0.9), intensity: 0.8),
//!     ],
//!     player: (position: (0.0, 0.0, 7.0), yaw_deg: 0.0),
//!     weapon: Some((model: "sword")),
//!     fog_density: 0.06,
//! )
//! ```

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Mat4, Vec3, Vec4};
use serde::Deserialize;
use vex_core::{EdgeKind, Segment, VecModel};
use vex_render::PostSettings;

#[derive(Debug, Deserialize)]
pub struct SceneFile {
    /// Model name → path, relative to the scene file.
    pub models: BTreeMap<String, String>,
    pub instances: Vec<InstanceDef>,
    #[serde(default)]
    pub player: PlayerDef,
    #[serde(default)]
    pub weapon: Option<WeaponDef>,
    #[serde(default = "default_fog")]
    pub fog_density: f32,
    /// Engine-level look controls (glow/bloom/exposure) — the scene, not
    /// the assets, decides how hot the phosphor runs.
    #[serde(default)]
    pub post: PostDef,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PostDef {
    pub exposure: f32,
    pub bloom_strength: f32,
    pub glow: f32,
    pub crt: f32,
}

impl Default for PostDef {
    fn default() -> Self {
        let defaults = PostSettings::default();
        Self {
            exposure: defaults.exposure,
            bloom_strength: defaults.bloom_strength,
            glow: defaults.glow,
            crt: defaults.crt,
        }
    }
}

impl PostDef {
    fn resolve(&self) -> PostSettings {
        PostSettings {
            exposure: self.exposure,
            bloom_strength: self.bloom_strength,
            glow: self.glow,
            crt: self.crt,
        }
    }
}

fn default_fog() -> f32 {
    0.05
}

fn one() -> f32 {
    1.0
}

fn white() -> (f32, f32, f32) {
    (1.0, 1.0, 1.0)
}

#[derive(Debug, Deserialize)]
pub struct InstanceDef {
    pub model: String,
    #[serde(default)]
    pub position: (f32, f32, f32),
    #[serde(default)]
    pub yaw_deg: f32,
    #[serde(default = "one")]
    pub scale: f32,
    /// Multiplies every edge's intensity.
    #[serde(default = "one")]
    pub intensity: f32,
    /// Multiplies every edge's RGB — palette recolor per instance.
    #[serde(default = "white")]
    pub tint: (f32, f32, f32),
}

#[derive(Debug, Deserialize)]
pub struct PlayerDef {
    pub position: (f32, f32, f32),
    #[serde(default)]
    pub yaw_deg: f32,
}

impl Default for PlayerDef {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0, 0.0),
            yaw_deg: 0.0,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WeaponDef {
    pub model: String,
}

/// One placed instance after baking: world AABB + its slices of the shared
/// segment/index buffers (for culling), plus what runtime silhouette
/// extraction needs (model reference, transform, styling).
#[derive(Debug, Clone)]
pub struct BakedInstance {
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
    pub segments: Range<u32>,
    pub occluder_indices: Range<u32>,
    pub model: usize,
    pub transform: Mat4,
    pub tint: Vec3,
    pub intensity: f32,
}

/// A scene flattened into world space, ready for upload.
pub struct BakedScene {
    /// Loaded models, indexed by `BakedInstance::model`.
    pub models: Vec<VecModel>,
    pub segments: Vec<Segment>,
    pub occluder_vertices: Vec<Vec3>,
    pub occluder_indices: Vec<u32>,
    pub instances: Vec<BakedInstance>,
    pub player_spawn: Vec3,
    pub player_yaw: f32,
    pub weapon: Option<VecModel>,
    pub fog_density: f32,
    pub post: PostSettings,
}

impl BakedScene {
    /// View-dependent silhouette segments for one instance (world space,
    /// instance tint/intensity applied). Call for visible instances only.
    pub fn instance_silhouettes(&self, instance: &BakedInstance, eye: Vec3) -> Vec<Segment> {
        let mut segments = Vec::new();
        self.instance_silhouettes_into(instance, eye, &mut segments);
        segments
    }

    /// Append one instance's visible silhouettes to a shared frame buffer.
    pub fn instance_silhouettes_into(
        &self,
        instance: &BakedInstance,
        eye: Vec3,
        out: &mut Vec<Segment>,
    ) {
        let start = out.len();
        self.models[instance.model].silhouette_segments_into(
            instance.transform,
            eye,
            instance.intensity,
            out,
        );
        for segment in &mut out[start..] {
            let rgb = segment.color.truncate() * instance.tint;
            segment.color = Vec4::new(rgb.x, rgb.y, rgb.z, segment.color.w);
        }
    }
}

pub fn load_scene(path: &Path) -> Result<BakedScene> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    load_scene_from_str(&text, |rel| {
        let model_path = base.join(rel);
        VecModel::load(&model_path)
            .with_context(|| format!("load model from {}", model_path.display()))
    })
}

/// Filesystem-free variant: the caller resolves each model reference from
/// the scene's `models` map (embedded assets on wasm, files elsewhere).
pub fn load_scene_from_str(
    text: &str,
    mut load_model: impl FnMut(&str) -> Result<VecModel>,
) -> Result<BakedScene> {
    let file: SceneFile = ron::from_str(text).context("parse scene")?;

    let mut models: Vec<VecModel> = Vec::new();
    let mut model_index: BTreeMap<&str, usize> = BTreeMap::new();
    for (name, rel) in &file.models {
        let model = load_model(rel).with_context(|| format!("load model '{name}'"))?;
        model_index.insert(name, models.len());
        models.push(model);
    }

    let mut baked = BakedScene {
        models,
        segments: Vec::new(),
        occluder_vertices: Vec::new(),
        occluder_indices: Vec::new(),
        instances: Vec::new(),
        player_spawn: Vec3::from(file.player.position),
        player_yaw: file.player.yaw_deg.to_radians(),
        weapon: None,
        fog_density: file.fog_density,
        post: file.post.resolve(),
    };

    for def in &file.instances {
        let Some(&index) = model_index.get(def.model.as_str()) else {
            bail!("instance references unknown model '{}'", def.model);
        };
        bake_instance(&mut baked, index, def);
    }

    if let Some(weapon) = &file.weapon {
        let Some(&index) = model_index.get(weapon.model.as_str()) else {
            bail!("weapon references unknown model '{}'", weapon.model);
        };
        baked.weapon = Some(baked.models[index].clone());
    }

    Ok(baked)
}

fn bake_instance(baked: &mut BakedScene, model_index: usize, def: &InstanceDef) {
    let transform = Mat4::from_translation(Vec3::from(def.position))
        * Mat4::from_rotation_y(def.yaw_deg.to_radians())
        * Mat4::from_scale(Vec3::splat(def.scale));
    let tint = Vec3::from(def.tint);

    // Build world-space geometry into locals first: the borrow of
    // `baked.models` must end before the pushes into `baked` below.
    let model = &baked.models[model_index];
    let mut baked_segments = Vec::new();
    for segment in model.edge_segments(EdgeKind::Always, def.intensity) {
        let rgb = segment.color.truncate() * tint;
        baked_segments.push(Segment {
            a: transform.transform_point3(segment.a),
            b: transform.transform_point3(segment.b),
            color: Vec4::new(rgb.x, rgb.y, rgb.z, segment.color.w),
            // Styles ride along; dash periods stay world-scaled by design.
            ..segment
        });
    }

    let world_vertices: Vec<Vec3> = model
        .vertices
        .iter()
        .map(|&v| transform.transform_point3(v))
        .collect();
    let model_indices = model.occluder_indices.clone();

    // World AABB from the model AABB's 8 transformed corners.
    let (lo, hi) = (model.aabb_min, model.aabb_max);
    let mut aabb_min = Vec3::splat(f32::INFINITY);
    let mut aabb_max = Vec3::splat(f32::NEG_INFINITY);
    for corner in 0..8 {
        let p = Vec3::new(
            if corner & 1 == 0 { lo.x } else { hi.x },
            if corner & 2 == 0 { lo.y } else { hi.y },
            if corner & 4 == 0 { lo.z } else { hi.z },
        );
        let world = transform.transform_point3(p);
        aabb_min = aabb_min.min(world);
        aabb_max = aabb_max.max(world);
    }

    let segment_start = baked.segments.len() as u32;
    baked.segments.extend(baked_segments);
    let vertex_base = baked.occluder_vertices.len() as u32;
    let index_start = baked.occluder_indices.len() as u32;
    baked.occluder_vertices.extend(world_vertices);
    baked
        .occluder_indices
        .extend(model_indices.iter().map(|&i| i + vertex_base));

    baked.instances.push(BakedInstance {
        aabb_min,
        aabb_max,
        segments: segment_start..baked.segments.len() as u32,
        occluder_indices: index_start..baked.occluder_indices.len() as u32,
        model: model_index,
        transform,
        tint,
        intensity: def.intensity,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;
    use vex_core::{VecEdge, compute_aabb};

    fn tiny_model() -> VecModel {
        let vertices = vec![vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)];
        let (aabb_min, aabb_max) = compute_aabb(&vertices);
        VecModel {
            palette: vec![vec3(0.0, 1.0, 0.0)],
            vertices,
            edges: vec![VecEdge {
                a: 0,
                b: 1,
                palette: 0,
                kind: EdgeKind::Always,
                style: 0,
                intensity: 1.0,
                n1: Vec3::Z,
                n2: Vec3::Z,
            }],
            occluder_indices: vec![0, 1, 2],
            aabb_min,
            aabb_max,
        }
    }

    #[test]
    fn scene_bakes_transforms_ranges_and_tint() {
        let dir = std::env::temp_dir().join("vex-scene-test");
        std::fs::create_dir_all(&dir).unwrap();
        tiny_model().save(&dir.join("tri.vec")).unwrap();
        std::fs::write(
            dir.join("scene.ron"),
            r#"(
                models: { "tri": "tri.vec" },
                instances: [
                    (model: "tri"),
                    (model: "tri", position: (10.0, 0.0, 0.0), scale: 2.0,
                     tint: (1.0, 0.5, 0.5), intensity: 0.5),
                ],
                player: (position: (0.0, 0.0, 3.0)),
            )"#,
        )
        .unwrap();

        let baked = load_scene(&dir.join("scene.ron")).unwrap();
        assert_eq!(baked.instances.len(), 2);
        assert_eq!(baked.segments.len(), 2);
        assert_eq!(baked.occluder_indices.len(), 6);
        assert_eq!(baked.instances[1].segments, 1..2);
        assert_eq!(baked.instances[1].occluder_indices, 3..6);
        // Second instance: translated ×10, scaled ×2, tinted, half intensity.
        let s = &baked.segments[1];
        assert!(s.a.abs_diff_eq(vec3(10.0, 0.0, 0.0), 1e-5));
        assert!(s.b.abs_diff_eq(vec3(12.0, 0.0, 0.0), 1e-5));
        assert!((s.color.y - 0.5).abs() < 1e-6, "green × 0.5 tint");
        assert!((s.color.w - 0.5).abs() < 1e-6, "intensity 0.5");
        assert_eq!(baked.player_spawn, vec3(0.0, 0.0, 3.0));
        // World AABB of the scaled instance reaches x = 12.
        assert!((baked.instances[1].aabb_max.x - 12.0).abs() < 1e-4);
        assert!(baked.weapon.is_none());
    }

    #[test]
    fn unknown_model_reference_fails_clearly() {
        let dir = std::env::temp_dir().join("vex-scene-test-bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("scene.ron"),
            r#"(models: {}, instances: [(model: "ghost")])"#,
        )
        .unwrap();
        let Err(err) = load_scene(&dir.join("scene.ron")) else {
            panic!("expected unknown-model error");
        };
        assert!(err.to_string().contains("ghost"));
    }
}
