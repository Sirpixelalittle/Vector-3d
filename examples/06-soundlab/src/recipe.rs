//! The sketch format: a sound described in RON, one field per synth
//! primitive argument — so a finished sketch translates mechanically
//! (via [`Sound::rust_code`]) into a recipe for a game's sound bank.
//!
//! Semantics: a [`Sound`] is layers mixed together; each layer is parts
//! played in series. That composes everything the arena's bank uses:
//! mixed one-shots, sequenced blips, silence-separated phrases.

use anyhow::{Context, Result};
use serde::Deserialize;
use vex_audio::StaticSoundData;
use vex_audio::synth::{append, burst, mix, saw, silence, sine, square, sweep, sweep_exp, to_sound};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Shape {
    Square,
    Saw,
    Sine,
}

impl Shape {
    fn wave(self) -> fn(f32) -> f32 {
        match self {
            Shape::Square => square,
            Shape::Saw => saw,
            Shape::Sine => sine,
        }
    }

    fn ident(self) -> &'static str {
        match self {
            Shape::Square => "square",
            Shape::Saw => "saw",
            Shape::Sine => "sine",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub enum Part {
    /// Linear pitch slide `from → to` Hz with exponential amp decay.
    Sweep {
        dur: f32,
        from: f32,
        to: f32,
        decay: f32,
        amp: f32,
        shape: Shape,
    },
    /// Geometric slide with a click-free attack — the laser envelope.
    SweepExp {
        dur: f32,
        from: f32,
        to: f32,
        decay: f32,
        amp: f32,
        shape: Shape,
    },
    /// White-noise burst.
    Burst { dur: f32, decay: f32, amp: f32 },
    /// A silent gap (sequencing).
    Silence { dur: f32 },
}

impl Part {
    fn samples(&self) -> Vec<f32> {
        match *self {
            Part::Sweep { dur, from, to, decay, amp, shape } => {
                sweep(dur, from, to, decay, amp, shape.wave())
            }
            Part::SweepExp { dur, from, to, decay, amp, shape } => {
                sweep_exp(dur, from, to, decay, amp, shape.wave())
            }
            Part::Burst { dur, decay, amp } => burst(dur, decay, amp),
            Part::Silence { dur } => silence(dur),
        }
    }

    fn rust_expr(&self) -> String {
        match *self {
            Part::Sweep { dur, from, to, decay, amp, shape } => format!(
                "sweep({dur:?}, {from:?}, {to:?}, {decay:?}, {amp:?}, {})",
                shape.ident()
            ),
            Part::SweepExp { dur, from, to, decay, amp, shape } => format!(
                "sweep_exp({dur:?}, {from:?}, {to:?}, {decay:?}, {amp:?}, {})",
                shape.ident()
            ),
            Part::Burst { dur, decay, amp } => {
                format!("burst({dur:?}, {decay:?}, {amp:?})")
            }
            Part::Silence { dur } => format!("silence({dur:?})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Sound {
    /// Layers are mixed; each layer's parts play in series.
    pub layers: Vec<Vec<Part>>,
}

impl Sound {
    pub fn from_str(text: &str) -> Result<Self> {
        let sound: Sound = ron::from_str(text).context("parse sound sketch")?;
        if sound.layers.iter().all(|l| l.is_empty()) {
            anyhow::bail!("the sketch has no parts");
        }
        Ok(sound)
    }

    /// Mixed mono samples, pre-clamp (peaks above 1.0 will distort once
    /// packed — [`peak`] them before shipping).
    pub fn samples(&self) -> Vec<f32> {
        let mut out = Vec::new();
        for layer in &self.layers {
            let mut series = Vec::new();
            for part in layer {
                series = append(series, part.samples());
            }
            out = mix(out, &series);
        }
        out
    }

    pub fn sound(&self) -> StaticSoundData {
        to_sound(self.samples())
    }

    /// A paste-ready recipe for a game's sound bank (the arena's
    /// `sounds.rs` pattern).
    pub fn rust_code(&self, name: &str) -> String {
        let mut lines = vec![format!("fn {name}() -> StaticSoundData {{")];
        for (i, layer) in self.layers.iter().enumerate() {
            let mut expr = layer
                .first()
                .map(Part::rust_expr)
                .unwrap_or_else(|| "Vec::new()".into());
            for part in layer.iter().skip(1) {
                expr = format!("append({expr}, {})", part.rust_expr());
            }
            lines.push(format!("    let l{i} = {expr};"));
        }
        let mut combined = "l0".to_string();
        for i in 1..self.layers.len() {
            combined = format!("mix({combined}, &l{i})");
        }
        lines.push(format!("    to_sound({combined})"));
        lines.push("}".into());
        lines.join("\n")
    }
}

/// Loudest absolute sample — keep it under 1.0 or the pack clamp clips.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vex_audio::synth::SAMPLE_RATE;

    const SKETCH: &str = r#"
        Sound(
            layers: [
                [SweepExp(dur: 0.2, from: 2800.0, to: 220.0, decay: 8.5, amp: 0.34, shape: Sine)],
                [Silence(dur: 0.1), Burst(dur: 0.1, decay: 20.0, amp: 0.3)],
            ],
        )
    "#;

    #[test]
    fn parses_builds_and_is_deterministic() {
        let sound = Sound::from_str(SKETCH).unwrap();
        let (a, b) = (sound.samples(), sound.samples());
        assert_eq!(a, b, "same sketch, same samples");
        assert!(peak(&a) > 0.05, "audible");
        assert!(a.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn layers_mix_to_the_longest_and_parts_append() {
        let sound = Sound::from_str(SKETCH).unwrap();
        let n = sound.samples().len();
        // Layer 0 is 0.2s; layer 1 is 0.1 + 0.1 = 0.2s appended.
        assert_eq!(n, (0.2 * SAMPLE_RATE as f32) as usize);

        // The burst layer is silent for its first 0.1s.
        let solo = Sound {
            layers: vec![vec![
                Part::Silence { dur: 0.1 },
                Part::Burst { dur: 0.1, decay: 20.0, amp: 0.3 },
            ]],
        };
        let samples = solo.samples();
        let split = (0.1 * SAMPLE_RATE as f32) as usize;
        assert!(samples[..split].iter().all(|&s| s == 0.0), "gap first");
        assert!(peak(&samples[split..]) > 0.05, "then noise");
    }

    #[test]
    fn rust_code_is_a_paste_ready_recipe() {
        let sound = Sound::from_str(SKETCH).unwrap();
        let code = sound.rust_code("pyew");
        assert!(code.contains("fn pyew() -> StaticSoundData {"));
        assert!(code.contains("sweep_exp(0.2, 2800.0, 220.0, 8.5, 0.34, sine)"));
        assert!(code.contains("append(silence(0.1), burst(0.1, 20.0, 0.3))"));
        assert!(code.contains("to_sound(mix(l0, &l1))"));
    }

    #[test]
    fn empty_sketches_are_rejected() {
        assert!(Sound::from_str("Sound(layers: [])").is_err());
        assert!(Sound::from_str("Sound(layers: [[], []])").is_err());
    }
}
