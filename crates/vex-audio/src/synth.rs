//! Procedural retro SFX: every sound is synthesized from square waves,
//! saws, sines and noise at startup — no audio files, era-correct for a
//! vector CRT, and deterministic (same bank every run, native or web).

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
fn sweep(
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
fn sweep_exp(
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

fn square(phase: f32) -> f32 {
    if phase < 0.5 { 1.0 } else { -1.0 }
}

fn saw(phase: f32) -> f32 {
    phase * 2.0 - 1.0
}

fn sine(phase: f32) -> f32 {
    (phase * std::f32::consts::TAU).sin()
}

/// White-noise burst with exponential decay.
fn burst(duration: f32, decay: f32, amp: f32) -> Vec<f32> {
    let mut noise = Noise::new();
    let n = seconds(duration);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            noise.next() * amp * (-decay * t).exp()
        })
        .collect()
}

fn mix(mut a: Vec<f32>, b: &[f32]) -> Vec<f32> {
    if b.len() > a.len() {
        a.resize(b.len(), 0.0);
    }
    for (dst, src) in a.iter_mut().zip(b) {
        *dst += src;
    }
    a
}

fn append(mut a: Vec<f32>, b: Vec<f32>) -> Vec<f32> {
    a.extend(b);
    a
}

fn silence(duration: f32) -> Vec<f32> {
    vec![0.0; seconds(duration)]
}

fn to_sound(samples: Vec<f32>) -> StaticSoundData {
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

pub fn shot() -> StaticSoundData {
    // Crack of noise over a low thump.
    let crack = burst(0.12, 26.0, 0.75);
    let thump = sweep(0.14, 170.0, 60.0, 14.0, 0.65, sine);
    to_sound(mix(crack, &thump))
}

pub fn bolt_fire() -> StaticSoundData {
    // Movie-laser "pyew": a fast exponential pitch dive on two slightly
    // detuned sines — their beating is the metallic shimmer — plus a tiny
    // trigger snap of noise at the front.
    let body = sweep_exp(0.24, 2800.0, 220.0, 8.5, 0.34, sine);
    let shimmer = sweep_exp(0.24, 2905.0, 236.0, 8.5, 0.20, sine);
    let snap = burst(0.018, 40.0, 0.18);
    to_sound(mix(mix(body, &shimmer), &snap))
}

pub fn bolt_impact() -> StaticSoundData {
    let sizzle = burst(0.09, 30.0, 0.5);
    let ping = sweep(0.07, 1300.0, 700.0, 22.0, 0.2, sine);
    to_sound(mix(sizzle, &ping))
}

pub fn enemy_death() -> StaticSoundData {
    // Falling saw plus a noise tail: a shape bursting into line particles.
    let fall = sweep(0.38, 240.0, 36.0, 6.0, 0.6, saw);
    let debris = burst(0.30, 9.0, 0.35);
    to_sound(mix(fall, &debris))
}

pub fn player_hit() -> StaticSoundData {
    // Harsh dual square growl.
    let low = sweep(0.22, 95.0, 70.0, 8.0, 0.55, square);
    let lower = sweep(0.22, 62.0, 48.0, 8.0, 0.45, square);
    to_sound(mix(low, &lower))
}

pub fn wave_start() -> StaticSoundData {
    // Two rising blips.
    let a = sweep(0.10, 660.0, 660.0, 4.0, 0.4, square);
    let b = sweep(0.16, 990.0, 990.0, 5.0, 0.4, square);
    to_sound(append(a, append(silence(0.05), b)))
}

pub fn health_spawn() -> StaticSoundData {
    // Gentle two-note bell: "a medkit is up."
    let a = sweep_exp(0.14, 660.0, 655.0, 5.0, 0.32, sine);
    let b = sweep_exp(0.22, 990.0, 982.0, 5.0, 0.30, sine);
    to_sound(append(a, append(silence(0.02), b)))
}

pub fn health_pickup() -> StaticSoundData {
    // Classic ascending power-up triad with a sparkle on top.
    let mut out = Vec::new();
    for freq in [523.0, 659.0, 784.0] {
        out = append(out, sweep(0.06, freq, freq, 3.0, 0.42, square));
    }
    let sparkle = sweep_exp(0.16, 1046.0, 1570.0, 6.0, 0.28, sine);
    to_sound(append(out, sparkle))
}

pub fn boss_ring() -> StaticSoundData {
    // A heavy rotating discharge: paired detuned saws diving an octave
    // with a grinding noise bed — fatter and angrier than a single bolt.
    let a = sweep_exp(0.34, 420.0, 88.0, 6.5, 0.34, saw);
    let b = sweep_exp(0.34, 436.0, 96.0, 6.5, 0.26, saw);
    let grind = burst(0.22, 11.0, 0.22);
    to_sound(mix(mix(a, &b), &grind))
}

pub fn dash() -> StaticSoundData {
    // A quick airy whoosh: rising sine under a fast-decaying noise gust.
    let gust = burst(0.20, 14.0, 0.34);
    let rise = sweep_exp(0.18, 240.0, 780.0, 4.0, 0.22, sine);
    to_sound(mix(gust, &rise))
}

pub fn game_over() -> StaticSoundData {
    // Three falling notes into the void.
    let mut out = Vec::new();
    for (freq, dur) in [(440.0, 0.16), (330.0, 0.16), (220.0, 0.34)] {
        out = append(out, sweep(dur, freq, freq * 0.97, 5.0, 0.5, square));
        out = append(out, silence(0.03));
    }
    to_sound(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(sound: &StaticSoundData) -> f32 {
        sound
            .frames
            .iter()
            .map(|f| f.left.abs().max(f.right.abs()))
            .fold(0.0, f32::max)
    }

    #[test]
    fn every_sound_is_audible_and_in_range() {
        for (name, sound) in [
            ("shot", shot()),
            ("bolt_fire", bolt_fire()),
            ("bolt_impact", bolt_impact()),
            ("enemy_death", enemy_death()),
            ("player_hit", player_hit()),
            ("wave_start", wave_start()),
            ("game_over", game_over()),
            ("health_spawn", health_spawn()),
            ("health_pickup", health_pickup()),
            ("boss_ring", boss_ring()),
            ("dash", dash()),
        ] {
            assert!(!sound.frames.is_empty(), "{name} is empty");
            let p = peak(&sound);
            assert!(p > 0.05, "{name} is near-silent (peak {p})");
            assert!(p <= 1.0, "{name} clips (peak {p})");
            assert!(
                sound.frames.iter().all(|f| f.left.is_finite()),
                "{name} contains NaN/inf"
            );
        }
    }

    #[test]
    fn synthesis_is_deterministic() {
        let (a, b) = (shot(), shot());
        assert_eq!(a.frames.len(), b.frames.len());
        assert!(
            a.frames
                .iter()
                .zip(b.frames.iter())
                .all(|(x, y)| x.left == y.left)
        );
    }
}
