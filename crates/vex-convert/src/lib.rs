//! glTF → `.vec` conversion: the vector3d content pipeline.
//!
//! Usable as a library (the viewer converts glTF in-process for hot reload)
//! or via the `vex-convert` CLI.

pub mod classify;
pub mod gltf_load;

use std::path::Path;

use anyhow::Result;
use vex_core::VecModel;

pub use classify::{ConvertOptions, ConvertStats, SourceGeometry, build_model};

/// Load a `.gltf`/`.glb` file and classify it into a vector model.
pub fn convert_gltf(path: &Path, options: &ConvertOptions) -> Result<(VecModel, ConvertStats)> {
    let source = gltf_load::load_gltf(path)?;
    Ok(build_model(&source, options))
}
