//! Soundlab: sketch a sound in RON, hear it on every save, then print
//! the paste-ready Rust for a game's sound bank. The tuning loop the
//! synth toolkit was missing — no game rebuild between tweaks.
//!
//!     cargo run -p soundlab -- sketch.sound.ron      # creates it if new
//!     cargo run -p soundlab -- sketch.ron --code     # print Rust, no audio
//!     cargo run -p soundlab -- sketch.ron --name pyew
//!
//! While running: edit + save the file to replay · Enter replays ·
//! `c` prints the Rust recipe · `q` quits.

mod recipe;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use recipe::{Sound, peak};
use vex_audio::AudioEngine;

/// Written when the sketch file doesn't exist yet: a working sound that
/// documents the whole format in its comments.
const TEMPLATE: &str = r#"// soundlab sketch — edit and save; the tool replays on every save.
//
// A Sound is layers (mixed together); each layer is parts (in series).
//   Sweep(   dur:, from:, to:, decay:, amp:, shape: )  linear pitch slide
//   SweepExp(dur:, from:, to:, decay:, amp:, shape: )  geometric slide, soft attack
//   Burst(   dur:, decay:, amp: )                      white noise
//   Silence( dur: )                                    gap between parts
// shape: Square | Saw | Sine · decay: exponential falloff (higher = snappier)
// Keep the printed peak under 1.0 — the pack step hard-clamps.
Sound(
    layers: [
        [SweepExp(dur: 0.24, from: 2800.0, to: 220.0, decay: 8.5, amp: 0.34, shape: Sine)],
        [SweepExp(dur: 0.24, from: 2905.0, to: 236.0, decay: 8.5, amp: 0.20, shape: Sine)],
        [Burst(dur: 0.018, decay: 40.0, amp: 0.18)],
    ],
)
"#;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.iter().find(|a| !a.starts_with("--")).cloned() else {
        println!("usage: soundlab <sketch.ron> [--code] [--name NAME]");
        return Ok(());
    };
    let path = PathBuf::from(path);
    let name = args
        .iter()
        .position(|a| a == "--name")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| recipe_name(&path));

    if !path.exists() {
        std::fs::write(&path, TEMPLATE).context("write starter sketch")?;
        println!("new sketch: {} (a laser 'pyew' to start from)", path.display());
    }

    let mut sound = report(&path, &name)?;
    if args.iter().any(|a| a == "--code") {
        println!("\n{}", sound.rust_code(&name));
        return Ok(());
    }

    let mut audio = AudioEngine::new().context("open audio device")?;
    audio.play(&sound.sound());
    println!("save the file to replay · Enter replays · c prints Rust · q quits");

    // Terminal input on its own thread; mtime polled here.
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut last_seen = mtime(&path);
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(cmd) => match cmd.trim() {
                "q" => break,
                "c" => println!("\n{}\n", sound.rust_code(&name)),
                _ => audio.play(&sound.sound()),
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let seen = mtime(&path);
                if seen != last_seen {
                    last_seen = seen;
                    // Mid-edit saves may not parse; keep the last good
                    // sound and keep watching.
                    match report(&path, &name) {
                        Ok(fresh) => {
                            sound = fresh;
                            audio.play(&sound.sound());
                        }
                        Err(err) => println!("  (not replaying: {err:#})"),
                    }
                }
            }
        }
    }
    Ok(())
}

/// Load, summarize to the terminal, and hand back the sketch.
fn report(path: &Path, name: &str) -> Result<Sound> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let sound = Sound::from_str(&text)?;
    let samples = sound.samples();
    let secs = samples.len() as f32 / vex_audio::synth::SAMPLE_RATE as f32;
    let parts: usize = sound.layers.iter().map(Vec::len).sum();
    let p = peak(&samples);
    let clip = if p > 1.0 { "  ⚠ CLIPS (>1.0)" } else { "" };
    println!(
        "{name}: {:.2}s · {} layer(s), {parts} part(s) · peak {p:.2}{clip}",
        secs,
        sound.layers.len(),
    );
    Ok(sound)
}

/// Default recipe name from the file stem: `boss-roar.sound.ron` → `boss_roar`.
fn recipe_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sound")
        .trim_end_matches(".sound");
    let cleaned: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    if cleaned.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("s_{cleaned}")
    } else {
        cleaned
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_and_names_are_rustified() {
        let sound = Sound::from_str(TEMPLATE).expect("starter sketch is valid");
        assert_eq!(sound.layers.len(), 3);
        assert_eq!(recipe_name(Path::new("boss-roar.sound.ron")), "boss_roar");
        assert_eq!(recipe_name(Path::new("8bit.ron")), "s_8bit");
    }
}
