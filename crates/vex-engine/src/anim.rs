//! Rigid-transform clip animation, loaded from hand-editable RON.
//!
//! A [`Clip`] is a set of keyframed [`Track`]s over the channels of a
//! rigid pose — translation, euler rotation, uniform scale — plus
//! [`Channel::Intensity`], which games feed to the line renderer so glow
//! itself can be animated (the most vector-native channel there is).
//!
//! Clips know nothing about what they animate: sample one at a time to
//! get a [`Pose`], then compose `base_transform * pose.transform()`.
//! Layering is plain code: sample two clips and multiply the transforms.
//!
//! ```ron
//! Clip(
//!     duration: 8.0,
//!     loop_mode: Loop,
//!     tracks: [
//!         Track(channel: RotY, easing: Linear, keys: [(0.0, 0.0), (8.0, 360.0)]),
//!         Track(channel: PosY, easing: Smooth, keys: [(0.0, 0.0), (2.0, 0.3), (4.0, 0.0)]),
//!         Track(channel: Intensity, easing: Smooth, keys: [(0.0, 1.0), (4.0, 1.6), (8.0, 1.0)]),
//!     ],
//! )
//! ```

use std::path::Path;

use anyhow::{Context, Result};
use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    PosX,
    PosY,
    PosZ,
    /// Degrees.
    RotX,
    /// Degrees.
    RotY,
    /// Degrees.
    RotZ,
    /// Uniform scale (default 1).
    Scale,
    /// Line-brightness multiplier (default 1); games pass it to
    /// `edge_segments`/`silhouette_segments` as the intensity scale.
    Intensity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Easing {
    /// Hold the previous key's value until the next key.
    Step,
    #[default]
    Linear,
    /// Smoothstep between keys.
    Smooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoopMode {
    /// Clamp at the last key; [`AnimPlayer::finished`] turns true.
    Once,
    #[default]
    Loop,
    /// Forward then backward, repeating.
    PingPong,
}

/// One animated channel: `(time, value)` keys, kept sorted by time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub channel: Channel,
    #[serde(default)]
    pub easing: Easing,
    pub keys: Vec<(f32, f32)>,
}

impl Track {
    fn sample(&self, t: f32) -> Option<f32> {
        let (first, last) = (self.keys.first()?, self.keys.last()?);
        if t <= first.0 {
            return Some(first.1);
        }
        if t >= last.0 {
            return Some(last.1);
        }
        let next = self.keys.iter().position(|k| k.0 > t)?;
        let (t0, v0) = self.keys[next - 1];
        let (t1, v1) = self.keys[next];
        let span = (t1 - t0).max(1e-6);
        let x = ((t - t0) / span).clamp(0.0, 1.0);
        let x = match self.easing {
            Easing::Step => 0.0,
            Easing::Linear => x,
            Easing::Smooth => x * x * (3.0 - 2.0 * x),
        };
        Some(v0 + (v1 - v0) * x)
    }
}

/// The sampled state of a clip at one instant. Identity by default —
/// channels a clip doesn't animate stay at their neutral value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub translation: Vec3,
    /// Euler degrees, applied yaw (Y) → pitch (X) → roll (Z).
    pub rotation_deg: Vec3,
    pub scale: f32,
    pub intensity: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation_deg: Vec3::ZERO,
            scale: 1.0,
            intensity: 1.0,
        }
    }
}

impl Pose {
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(
            EulerRot::YXZ,
            self.rotation_deg.y.to_radians(),
            self.rotation_deg.x.to_radians(),
            self.rotation_deg.z.to_radians(),
        )
    }

    /// Local animation transform: translate · rotate · scale. Compose as
    /// `base * pose.transform()`.
    pub fn transform(&self) -> Mat4 {
        Mat4::from_translation(self.translation)
            * Mat4::from_quat(self.rotation())
            * Mat4::from_scale(Vec3::splat(self.scale))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub duration: f32,
    #[serde(default)]
    pub loop_mode: LoopMode,
    pub tracks: Vec<Track>,
}

impl Clip {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        Self::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Result<Self> {
        let mut clip: Clip = ron::from_str(text).context("parse animation clip")?;
        anyhow::ensure!(clip.duration > 0.0, "clip duration must be positive");
        for track in &mut clip.tracks {
            anyhow::ensure!(!track.keys.is_empty(), "track has no keys");
            track
                .keys
                .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        }
        Ok(clip)
    }

    /// Map raw time onto the clip's local timeline per its loop mode.
    pub fn wrap_time(&self, t: f32) -> f32 {
        match self.loop_mode {
            LoopMode::Once => t.clamp(0.0, self.duration),
            LoopMode::Loop => t.rem_euclid(self.duration),
            LoopMode::PingPong => {
                let phase = t.rem_euclid(self.duration * 2.0);
                if phase <= self.duration {
                    phase
                } else {
                    self.duration * 2.0 - phase
                }
            }
        }
    }

    /// Sample the pose at raw time `t` (wrapping/clamping applied).
    pub fn sample(&self, t: f32) -> Pose {
        let t = self.wrap_time(t);
        let mut pose = Pose::default();
        for track in &self.tracks {
            let Some(value) = track.sample(t) else {
                continue;
            };
            match track.channel {
                Channel::PosX => pose.translation.x = value,
                Channel::PosY => pose.translation.y = value,
                Channel::PosZ => pose.translation.z = value,
                Channel::RotX => pose.rotation_deg.x = value,
                Channel::RotY => pose.rotation_deg.y = value,
                Channel::RotZ => pose.rotation_deg.z = value,
                Channel::Scale => pose.scale = value,
                Channel::Intensity => pose.intensity = value,
            }
        }
        pose
    }
}

/// Owns playback time for one clip instance. For simple cases you can
/// skip this and call `clip.sample(your_time)` directly.
#[derive(Debug, Clone)]
pub struct AnimPlayer {
    pub clip: Clip,
    pub time: f32,
    pub speed: f32,
}

impl AnimPlayer {
    pub fn new(clip: Clip) -> Self {
        Self {
            clip,
            time: 0.0,
            speed: 1.0,
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt * self.speed;
    }

    pub fn pose(&self) -> Pose {
        self.clip.sample(self.time)
    }

    pub fn restart(&mut self) {
        self.time = 0.0;
    }

    /// True once a `Once` clip has played through (never for loops).
    pub fn finished(&self) -> bool {
        matches!(self.clip.loop_mode, LoopMode::Once) && self.time >= self.clip.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPIN_BOB: &str = r#"Clip(
        duration: 4.0,
        loop_mode: Loop,
        tracks: [
            Track(channel: RotY, easing: Linear, keys: [(0.0, 0.0), (4.0, 360.0)]),
            Track(channel: PosY, easing: Smooth, keys: [(2.0, 1.0), (0.0, 0.0), (4.0, 0.0)]),
            Track(channel: Intensity, keys: [(0.0, 1.0), (4.0, 2.0)]),
        ],
    )"#;

    #[test]
    fn parses_and_sorts_keys() {
        let clip = Clip::from_str(SPIN_BOB).unwrap();
        assert_eq!(clip.tracks[1].keys[0].0, 0.0, "keys sorted by time");
        assert_eq!(clip.tracks[1].keys[1].0, 2.0);
    }

    #[test]
    fn linear_and_smooth_interpolation() {
        let clip = Clip::from_str(SPIN_BOB).unwrap();
        let pose = clip.sample(1.0);
        assert!((pose.rotation_deg.y - 90.0).abs() < 1e-4, "linear quarter turn");
        // Smooth midpoint between (0,0) and (2,1) is exactly 0.5.
        assert!((pose.translation.y - 0.5).abs() < 1e-4);
        // t=1 of 0..4 linear 1..2 intensity.
        assert!((pose.intensity - 1.25).abs() < 1e-4);
    }

    #[test]
    fn step_easing_holds_previous_key() {
        let clip = Clip::from_str(
            r#"Clip(duration: 2.0, tracks: [
                Track(channel: Scale, easing: Step, keys: [(0.0, 1.0), (1.0, 3.0)]),
            ])"#,
        )
        .unwrap();
        assert_eq!(clip.sample(0.99).scale, 1.0);
        assert_eq!(clip.sample(1.0).scale, 3.0);
    }

    #[test]
    fn loop_modes_wrap_clamp_and_reflect() {
        let mut clip = Clip::from_str(SPIN_BOB).unwrap();
        assert!((clip.wrap_time(5.0) - 1.0).abs() < 1e-5, "loop wraps");
        clip.loop_mode = LoopMode::Once;
        assert_eq!(clip.wrap_time(5.0), 4.0, "once clamps");
        clip.loop_mode = LoopMode::PingPong;
        assert!((clip.wrap_time(5.0) - 3.0).abs() < 1e-5, "pingpong reflects");
        assert!((clip.wrap_time(9.0) - 1.0).abs() < 1e-5, "pingpong repeats");
    }

    #[test]
    fn unanimated_channels_stay_neutral() {
        let clip = Clip::from_str(
            r#"Clip(duration: 1.0, tracks: [
                Track(channel: PosX, keys: [(0.0, 5.0)]),
            ])"#,
        )
        .unwrap();
        let pose = clip.sample(0.5);
        assert_eq!(pose.translation.x, 5.0);
        assert_eq!(pose.scale, 1.0);
        assert_eq!(pose.intensity, 1.0);
        assert_eq!(pose.rotation_deg, Vec3::ZERO);
    }

    #[test]
    fn pose_transform_composes_translate_rotate_scale() {
        let pose = Pose {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation_deg: Vec3::new(0.0, 90.0, 0.0),
            scale: 2.0,
            intensity: 1.0,
        };
        // Local +X, scaled then yawed 90° (X → -Z), then translated.
        let p = pose.transform().transform_point3(Vec3::X);
        assert!((p - Vec3::new(1.0, 2.0, 1.0)).length() < 1e-4, "got {p}");
    }

    #[test]
    fn player_reports_finished_for_once_clips() {
        let mut clip = Clip::from_str(SPIN_BOB).unwrap();
        clip.loop_mode = LoopMode::Once;
        let mut player = AnimPlayer::new(clip);
        player.update(2.0);
        assert!(!player.finished());
        player.update(2.5);
        assert!(player.finished());
        player.restart();
        assert!(!player.finished());
    }

    #[test]
    fn rejects_bad_clips() {
        assert!(Clip::from_str("Clip(duration: 0.0, tracks: [])").is_err());
        assert!(
            Clip::from_str(
                "Clip(duration: 1.0, tracks: [Track(channel: PosX, keys: [])])"
            )
            .is_err()
        );
    }
}
