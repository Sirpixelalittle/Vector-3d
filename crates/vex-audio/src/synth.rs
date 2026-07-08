//! Procedural retro-SFX toolkit: oscillator sweeps, noise bursts and
//! mixing helpers for building sounds from square waves, saws, sines and
//! noise at startup — no audio files, era-correct for a vector CRT, and
//! deterministic (same output every run, native or web).
//!
//! The engine ships only these primitives. Each game composes them into
//! its own sound bank (see the arena's `sounds.rs`) and hands the results
//! to [`AudioEngine`](crate::AudioEngine) to play.

use std::sync::Arc;

use kira::Frame;
use kira::sound::static_sound::StaticSoundData;

pub const SAMPLE_RATE: u32 = 22_050;

/// Deterministic noise (xorshift*), so sounds are identical every run.
struct Noise(u64);

impl Noise {
    fn new() -> Self {
        Self(0x1234_5678_9ABC_DEF1)
    }

    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let bits = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32;
        bits / (1u64 << 23) as f32 - 1.0
    }
}

fn seconds(duration: f32) -> usize {
    (duration * SAMPLE_RATE as f32) as usize
}

/// Sweep an oscillator from `f0` to `f1` Hz with an exponential amplitude
/// decay; `shape(phase)` maps 0..1 phase to a waveform sample.
pub fn sweep(
    duration: f32,
    f0: f32,
    f1: f32,
    decay: f32,
    amp: f32,
    shape: impl Fn(f32) -> f32,
) -> Vec<f32> {
    let n = seconds(duration);
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let freq = f0 + (f1 - f0) * t;
            phase = (phase + freq / SAMPLE_RATE as f32).fract();
            shape(phase) * amp * (-decay * t).exp()
        })
        .collect()
}

/// Geometric frequency sweep with a click-free attack: pitch dives fast
/// then tails off, like a discharge — the movie-laser envelope. (The
/// linear [`sweep`] reads as chiptune; this reads as sci-fi.)
pub fn sweep_exp(
    duration: f32,
    f0: f32,
    f1: f32,
    decay: f32,
    amp: f32,
    shape: impl Fn(f32) -> f32,
) -> Vec<f32> {
    const ATTACK_SECONDS: f32 = 0.005;
    let n = seconds(duration);
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let freq = f0 * (f1 / f0).powf(t);
            phase = (phase + freq / SAMPLE_RATE as f32).fract();
            let attack = (i as f32 / (ATTACK_SECONDS * SAMPLE_RATE as f32)).min(1.0);
            shape(phase) * amp * attack * (-decay * t).exp()
        })
        .collect()
}

pub fn square(phase: f32) -> f32 {
    if phase < 0.5 { 1.0 } else { -1.0 }
}

pub fn saw(phase: f32) -> f32 {
    phase * 2.0 - 1.0
}

pub fn sine(phase: f32) -> f32 {
    (phase * std::f32::consts::TAU).sin()
}

/// White-noise burst with exponential decay.
pub fn burst(duration: f32, decay: f32, amp: f32) -> Vec<f32> {
    let mut noise = Noise::new();
    let n = seconds(duration);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            noise.next() * amp * (-decay * t).exp()
        })
        .collect()
}

/// Sum `b` into `a` sample-wise (growing `a` if needed).
pub fn mix(mut a: Vec<f32>, b: &[f32]) -> Vec<f32> {
    if b.len() > a.len() {
        a.resize(b.len(), 0.0);
    }
    for (dst, src) in a.iter_mut().zip(b) {
        *dst += src;
    }
    a
}

/// Concatenate `b` after `a`.
pub fn append(mut a: Vec<f32>, b: Vec<f32>) -> Vec<f32> {
    a.extend(b);
    a
}

pub fn silence(duration: f32) -> Vec<f32> {
    vec![0.0; seconds(duration)]
}

/// Pack samples into playable sound data, hard-clamping to ±1.
pub fn to_sound(samples: Vec<f32>) -> StaticSoundData {
    let frames: Arc<[Frame]> = samples
        .into_iter()
        .map(|s| Frame::from_mono(s.clamp(-1.0, 1.0)))
        .collect();
    StaticSoundData {
        sample_rate: SAMPLE_RATE,
        frames,
        settings: Default::default(),
        slice: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeps_and_bursts_have_the_requested_length() {
        assert_eq!(sweep(0.5, 440.0, 220.0, 4.0, 0.5, sine).len(), seconds(0.5));
        assert_eq!(sweep_exp(0.25, 880.0, 110.0, 6.0, 0.4, saw).len(), seconds(0.25));
        assert_eq!(burst(0.1, 20.0, 0.5).len(), seconds(0.1));
        assert_eq!(silence(0.2).len(), seconds(0.2));
    }

    #[test]
    fn synthesis_is_deterministic() {
        let (a, b) = (burst(0.1, 20.0, 0.5), burst(0.1, 20.0, 0.5));
        assert_eq!(a, b, "noise reseeds identically");
        let (a, b) = (
            sweep_exp(0.2, 2800.0, 220.0, 8.5, 0.34, sine),
            sweep_exp(0.2, 2800.0, 220.0, 8.5, 0.34, sine),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn to_sound_clamps_hot_samples() {
        let sound = to_sound(vec![2.0, -3.0, 0.5]);
        assert_eq!(sound.frames[0].left, 1.0);
        assert_eq!(sound.frames[1].left, -1.0);
        assert_eq!(sound.frames[2].left, 0.5);
    }

    #[test]
    fn mix_grows_to_the_longer_input() {
        let out = mix(vec![0.1, 0.2], &[0.0, 0.0, 0.3]);
        assert_eq!(out.len(), 3);
        assert!((out[2] - 0.3).abs() < 1e-6);
    }
}
