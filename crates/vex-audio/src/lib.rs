//! 3D spatial audio for the vector3d engine, built on kira.
//!
//! The engine is content-free: it owns the device, the listener and
//! spatial playback, and plays whatever [`StaticSoundData`] a game hands
//! it. Games define their own sound banks — typically synthesized with
//! the [`synth`] toolkit — so adding a sound never touches engine code.
//! Spatial one-shots use transient kira spatial tracks that persist until
//! their sound finishes, so playback is fire-and-forget.
//!
//! Browser note: construct [`AudioEngine`] after the first user gesture
//! (e.g. the click that captures the mouse) — autoplay policies keep the
//! AudioContext suspended until then.

pub mod synth;

use anyhow::{Context, Result, anyhow};
use glam::{Quat, Vec3};
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances};
use kira::{AudioManager, AudioManagerSettings, DefaultBackend, Easing, Tween};

/// Re-exported so games can hold sound banks without depending on kira.
pub use kira::sound::static_sound::StaticSoundData;

/// Full volume inside this range (world units)…
const MIN_DISTANCE: f32 = 2.0;
/// …fading to silence at this range.
const MAX_DISTANCE: f32 = 42.0;

pub struct AudioEngine {
    manager: AudioManager<DefaultBackend>,
    listener: kira::listener::ListenerHandle,
}

impl AudioEngine {
    /// Open the default audio device and place the listener at the origin.
    pub fn new() -> Result<Self> {
        let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| anyhow!("open audio device: {e}"))?;
        let listener = manager
            .add_listener(Vec3::ZERO, Quat::IDENTITY)
            .context("add audio listener")?;
        Ok(Self { manager, listener })
    }

    /// Track the camera every frame so spatial sounds pan and fade.
    pub fn set_listener(&mut self, position: Vec3, orientation: Quat) {
        self.listener.set_position(position, Tween::default());
        self.listener.set_orientation(orientation, Tween::default());
    }

    /// Non-spatial playback (UI, the player's own gun).
    pub fn play(&mut self, sound: &StaticSoundData) {
        if let Err(err) = self.manager.play(sound.clone()) {
            log::debug!("audio play failed: {err}");
        }
    }

    /// Positional one-shot: a transient spatial track that pans and
    /// attenuates relative to the listener, freed when the sound ends.
    pub fn play_at(&mut self, sound: &StaticSoundData, position: Vec3) {
        let builder = SpatialTrackBuilder::new()
            .distances(SpatialTrackDistances {
                min_distance: MIN_DISTANCE,
                max_distance: MAX_DISTANCE,
            })
            .attenuation_function(Easing::Linear)
            .persist_until_sounds_finish(true);
        match self
            .manager
            .add_spatial_sub_track(self.listener.id(), position, builder)
        {
            Ok(mut track) => {
                if let Err(err) = track.play(sound.clone()) {
                    log::debug!("audio play failed: {err}");
                }
                // Dropping the handle is fine: the track persists until
                // the sound finishes, then frees itself.
            }
            Err(_) => {
                // Track capacity exhausted (a wall of simultaneous sounds):
                // fall back to non-spatial rather than going silent.
                self.play(sound);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synth::{sine, sweep_exp, to_sound};

    /// Opens the real audio device and plays test tones — run explicitly
    /// with `cargo test -p vex-audio -- --ignored` on a machine with
    /// speakers. (Game sound banks have their own audibility tests.)
    #[test]
    #[ignore = "needs an audio device; audibly plays sounds"]
    fn device_smoke_test() {
        let mut audio = AudioEngine::new().expect("open audio device");
        audio.set_listener(Vec3::ZERO, Quat::IDENTITY);
        let blip = to_sound(sweep_exp(0.25, 880.0, 220.0, 5.0, 0.4, sine));
        audio.play(&blip);
        std::thread::sleep(std::time::Duration::from_millis(400));
        audio.play_at(&blip, Vec3::new(5.0, 1.0, -3.0));
        std::thread::sleep(std::time::Duration::from_millis(400));
        audio.play_at(&blip, Vec3::new(-6.0, 1.0, 0.0));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}
