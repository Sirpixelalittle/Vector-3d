//! 3D spatial audio for the vector3d engine, built on kira.
//!
//! All SFX are procedurally synthesized ([`synth`]) — no audio files.
//! Spatial one-shots use transient kira spatial tracks that persist until
//! their sound finishes, so playback is fire-and-forget.
//!
//! Browser note: construct [`AudioEngine`] after the first user gesture
//! (e.g. the click that captures the mouse) — autoplay policies keep the
//! AudioContext suspended until then.

mod synth;

use anyhow::{Context, Result, anyhow};
use glam::{Quat, Vec3};
use kira::sound::static_sound::StaticSoundData;
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances};
use kira::{AudioManager, AudioManagerSettings, DefaultBackend, Easing, Tween};

/// Full volume inside this range (world units)…
const MIN_DISTANCE: f32 = 2.0;
/// …fading to silence at this range.
const MAX_DISTANCE: f32 = 42.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sfx {
    Shot,
    BoltFire,
    BoltImpact,
    EnemyDeath,
    PlayerHit,
    WaveStart,
    GameOver,
    HealthSpawn,
    HealthPickup,
    BossRing,
    Dash,
    Ricochet,
}

pub struct AudioEngine {
    manager: AudioManager<DefaultBackend>,
    listener: kira::listener::ListenerHandle,
    bank: Bank,
}

struct Bank {
    shot: StaticSoundData,
    bolt_fire: StaticSoundData,
    bolt_impact: StaticSoundData,
    enemy_death: StaticSoundData,
    player_hit: StaticSoundData,
    wave_start: StaticSoundData,
    game_over: StaticSoundData,
    health_spawn: StaticSoundData,
    health_pickup: StaticSoundData,
    boss_ring: StaticSoundData,
    dash: StaticSoundData,
    ricochet: StaticSoundData,
}

impl Bank {
    fn get(&self, sfx: Sfx) -> &StaticSoundData {
        match sfx {
            Sfx::Shot => &self.shot,
            Sfx::BoltFire => &self.bolt_fire,
            Sfx::BoltImpact => &self.bolt_impact,
            Sfx::EnemyDeath => &self.enemy_death,
            Sfx::PlayerHit => &self.player_hit,
            Sfx::WaveStart => &self.wave_start,
            Sfx::GameOver => &self.game_over,
            Sfx::HealthSpawn => &self.health_spawn,
            Sfx::HealthPickup => &self.health_pickup,
            Sfx::BossRing => &self.boss_ring,
            Sfx::Dash => &self.dash,
            Sfx::Ricochet => &self.ricochet,
        }
    }
}

impl AudioEngine {
    /// Open the default audio device and synthesize the sound bank.
    pub fn new() -> Result<Self> {
        let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| anyhow!("open audio device: {e}"))?;
        let listener = manager
            .add_listener(Vec3::ZERO, Quat::IDENTITY)
            .context("add audio listener")?;
        Ok(Self {
            manager,
            listener,
            bank: Bank {
                shot: synth::shot(),
                bolt_fire: synth::bolt_fire(),
                bolt_impact: synth::bolt_impact(),
                enemy_death: synth::enemy_death(),
                player_hit: synth::player_hit(),
                wave_start: synth::wave_start(),
                game_over: synth::game_over(),
                health_spawn: synth::health_spawn(),
                health_pickup: synth::health_pickup(),
                boss_ring: synth::boss_ring(),
                dash: synth::dash(),
                ricochet: synth::ricochet(),
            },
        })
    }

    /// Track the camera every frame so spatial sounds pan and fade.
    pub fn set_listener(&mut self, position: Vec3, orientation: Quat) {
        self.listener.set_position(position, Tween::default());
        self.listener.set_orientation(orientation, Tween::default());
    }

    /// Non-spatial playback (UI, the player's own gun).
    pub fn play(&mut self, sfx: Sfx) {
        if let Err(err) = self.manager.play(self.bank.get(sfx).clone()) {
            log::debug!("audio play failed: {err}");
        }
    }

    /// Positional one-shot: a transient spatial track that pans and
    /// attenuates relative to the listener, freed when the sound ends.
    pub fn play_at(&mut self, sfx: Sfx, position: Vec3) {
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
                if let Err(err) = track.play(self.bank.get(sfx).clone()) {
                    log::debug!("audio play failed: {err}");
                }
                // Dropping the handle is fine: the track persists until
                // the sound finishes, then frees itself.
            }
            Err(_) => {
                // Track capacity exhausted (a wall of simultaneous sounds):
                // fall back to non-spatial rather than going silent.
                self.play(sfx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens the real audio device and plays sounds — run explicitly with
    /// `cargo test -p vex-audio -- --ignored` on a machine with speakers.
    #[test]
    #[ignore = "needs an audio device; audibly plays sounds"]
    fn device_smoke_test() {
        let mut audio = AudioEngine::new().expect("open audio device");
        audio.set_listener(Vec3::ZERO, Quat::IDENTITY);
        audio.play(Sfx::Shot);
        audio.play_at(Sfx::BoltFire, Vec3::new(5.0, 1.0, -3.0));
        std::thread::sleep(std::time::Duration::from_millis(600));
        audio.play_at(Sfx::EnemyDeath, Vec3::new(-6.0, 1.0, 0.0));
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
}
