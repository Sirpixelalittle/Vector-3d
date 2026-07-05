//! CLI: convert a glTF file into the engine's `.vec` format.
//!
//!     vex-convert model.gltf [-o model.vec] [--crease 30]

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use vex_convert::{ConvertOptions, ConvertStats};

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(parsed) = parse_args(&args)? else {
        print_usage();
        return Ok(());
    };

    let (model, stats) = vex_convert::convert_gltf(&parsed.input, &parsed.options)?;
    model
        .save(&parsed.output)
        .with_context(|| format!("write {}", parsed.output.display()))?;

    print_report(&parsed, &stats, &model);
    Ok(())
}

struct Args {
    input: PathBuf,
    output: PathBuf,
    options: ConvertOptions,
}

fn parse_args(args: &[String]) -> Result<Option<Args>> {
    let mut input = None;
    let mut output = None;
    let mut options = ConvertOptions::default();

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "-o" | "--output" => {
                output = Some(PathBuf::from(
                    iter.next().context("-o expects a path")?,
                ));
            }
            "--crease" => {
                options.crease_angle_deg = iter
                    .next()
                    .context("--crease expects degrees")?
                    .parse()
                    .context("--crease expects a number of degrees")?;
            }
            other if !other.starts_with('-') && input.is_none() => {
                input = Some(PathBuf::from(other));
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let Some(input) = input else { return Ok(None) };
    let output = output.unwrap_or_else(|| input.with_extension("vec"));
    Ok(Some(Args {
        input,
        output,
        options,
    }))
}

fn print_usage() {
    println!(
        "usage: vex-convert <model.gltf|model.glb> [-o out.vec] [--crease DEGREES]\n\
         \n\
         Converts glTF geometry into the vector3d .vec format:\n\
         triangle meshes become invisible occluders plus classified edges\n\
         (boundary/crease/material always drawn; smooth edges saved as\n\
         silhouette candidates), and LINES primitives (Blender loose edges)\n\
         pass through as always-drawn decoration."
    );
}

fn print_report(args: &Args, stats: &ConvertStats, model: &vex_core::VecModel) {
    let size = std::fs::metadata(&args.output).map(|m| m.len()).unwrap_or(0);
    println!(
        "{} → {}",
        args.input.display(),
        args.output.display()
    );
    println!(
        "  vertices   {} welded (from {})",
        stats.welded_vertices, stats.source_vertices
    );
    println!(
        "  triangles  {}{}",
        stats.triangles,
        if stats.degenerate_triangles > 0 {
            format!(" ({} degenerate skipped)", stats.degenerate_triangles)
        } else {
            String::new()
        }
    );
    println!(
        "  edges      {} drawn: {} boundary · {} crease · {} material · {} non-manifold · {} decor",
        stats.always_edges(),
        stats.boundary,
        stats.crease,
        stats.material,
        stats.non_manifold,
        stats.decor,
    );
    println!(
        "             {} smooth silhouette candidates ({} coplanar dropped)",
        stats.smooth, stats.dropped_coplanar
    );
    println!("  palette    {} colors", model.palette.len());
    println!(
        "  aabb       [{:.2}, {:.2}, {:.2}] .. [{:.2}, {:.2}, {:.2}]",
        model.aabb_min.x,
        model.aabb_min.y,
        model.aabb_min.z,
        model.aabb_max.x,
        model.aabb_max.y,
        model.aabb_max.z
    );
    println!("  wrote      {:.1} KB", size as f64 / 1024.0);
}
