//! The arena's sound bank: every effect synthesized at startup from the
//! engine's [`vex_audio::synth`] toolkit. Sounds are game content and
//! live here with the game — the engine just plays what it's handed, so
//! adding an effect never touches engine code.

use vex_audio::StaticSoundData;
use vex_audio::synth::{append, burst, mix, saw, silence, sine, square, sweep, sweep_exp, to_sound};

pub struct Sounds {
    pub shot: StaticSoundData,
    pub bolt_fire: StaticSoundData,
    pub bolt_impact: StaticSoundData,
    pub enemy_death: StaticSoundData,
    pub player_hit: StaticSoundData,
    pub wave_start: StaticSoundData,
    pub game_over: StaticSoundData,
    pub health_spawn: StaticSoundData,
    pub health_pickup: StaticSoundData,
    pub boss_ring: StaticSoundData,
    pub dash: StaticSoundData,
    pub ricochet: StaticSoundData,
    pub power_up: StaticSoundData,
}

impl Sounds {
    /// Synthesize the whole bank — deterministic, a few hundred KB, and
    /// fast enough to run at startup on native and web alike.
    pub fn synth() -> Self {
        Self {
            shot: shot(),
            bolt_fire: bolt_fire(),
            bolt_impact: bolt_impact(),
            enemy_death: enemy_death(),
            player_hit: player_hit(),
            wave_start: wave_start(),
            game_over: game_over(),
            health_spawn: health_spawn(),
            health_pickup: health_pickup(),
            boss_ring: boss_ring(),
            dash: dash(),
            ricochet: ricochet(),
            power_up: power_up(),
        }
    }

    /// Every sound with its name — for the bank-wide audibility tests.
    #[cfg(test)]
    fn all(&self) -> [(&'static str, &StaticSoundData); 13] {
        [
            ("shot", &self.shot),
            ("bolt_fire", &self.bolt_fire),
            ("bolt_impact", &self.bolt_impact),
            ("enemy_death", &self.enemy_death),
            ("player_hit", &self.player_hit),
            ("wave_start", &self.wave_start),
            ("game_over", &self.game_over),
            ("health_spawn", &self.health_spawn),
            ("health_pickup", &self.health_pickup),
            ("boss_ring", &self.boss_ring),
            ("dash", &self.dash),
            ("ricochet", &self.ricochet),
            ("power_up", &self.power_up),
        ]
    }
}

fn shot() -> StaticSoundData {
    // Crack of noise over a low thump.
    let crack = burst(0.12, 26.0, 0.75);
    let thump = sweep(0.14, 170.0, 60.0, 14.0, 0.65, sine);
    to_sound(mix(crack, &thump))
}

fn bolt_fire() -> StaticSoundData {
    // Ray-gun zap. Saws, not sines: a pure sine dive reads as a bird
    // chirp (player feedback), while saw harmonics carry the electric
    // bite of an arcade laser. Two detuned saws thicken the tone, a
    // square an octave down gives it a chest, and a snap of noise is
    // the trigger transient. Short and mid-pitched on purpose — bolts
    // fire constantly at high waves, so no whistle tails.
    let body = sweep_exp(0.14, 1500.0, 240.0, 11.0, 0.30, saw);
    let edge = sweep_exp(0.14, 1512.0, 252.0, 11.0, 0.20, saw);
    let sub = sweep_exp(0.14, 750.0, 120.0, 11.0, 0.16, square);
    let snap = burst(0.02, 45.0, 0.22);
    to_sound(mix(mix(mix(body, &edge), &sub), &snap))
}

fn bolt_impact() -> StaticSoundData {
    let sizzle = burst(0.09, 30.0, 0.5);
    let ping = sweep(0.07, 1300.0, 700.0, 22.0, 0.2, sine);
    to_sound(mix(sizzle, &ping))
}

fn enemy_death() -> StaticSoundData {
    // Falling saw plus a noise tail: a shape bursting into line particles.
    let fall = sweep(0.38, 240.0, 36.0, 6.0, 0.6, saw);
    let debris = burst(0.30, 9.0, 0.35);
    to_sound(mix(fall, &debris))
}

fn player_hit() -> StaticSoundData {
    // Harsh dual square growl.
    let low = sweep(0.22, 95.0, 70.0, 8.0, 0.55, square);
    let lower = sweep(0.22, 62.0, 48.0, 8.0, 0.45, square);
    to_sound(mix(low, &lower))
}

fn wave_start() -> StaticSoundData {
    // Two rising blips.
    let a = sweep(0.10, 660.0, 660.0, 4.0, 0.4, square);
    let b = sweep(0.16, 990.0, 990.0, 5.0, 0.4, square);
    to_sound(append(a, append(silence(0.05), b)))
}

fn health_spawn() -> StaticSoundData {
    // Gentle two-note bell: "a medkit is up."
    let a = sweep_exp(0.14, 660.0, 655.0, 5.0, 0.32, sine);
    let b = sweep_exp(0.22, 990.0, 982.0, 5.0, 0.30, sine);
    to_sound(append(a, append(silence(0.02), b)))
}

fn health_pickup() -> StaticSoundData {
    // Classic ascending power-up triad with a sparkle on top.
    let mut out = Vec::new();
    for freq in [523.0, 659.0, 784.0] {
        out = append(out, sweep(0.06, freq, freq, 3.0, 0.42, square));
    }
    let sparkle = sweep_exp(0.16, 1046.0, 1570.0, 6.0, 0.28, sine);
    to_sound(append(out, sparkle))
}

fn boss_ring() -> StaticSoundData {
    // A heavy rotating discharge: paired detuned saws diving an octave
    // with a grinding noise bed — fatter and angrier than a single bolt.
    let a = sweep_exp(0.34, 420.0, 88.0, 6.5, 0.34, saw);
    let b = sweep_exp(0.34, 436.0, 96.0, 6.5, 0.26, saw);
    let grind = burst(0.22, 11.0, 0.22);
    to_sound(mix(mix(a, &b), &grind))
}

fn dash() -> StaticSoundData {
    // A quick airy whoosh: rising sine under a fast-decaying noise gust.
    let gust = burst(0.20, 14.0, 0.34);
    let rise = sweep_exp(0.18, 240.0, 780.0, 4.0, 0.22, sine);
    to_sound(mix(gust, &rise))
}

fn ricochet() -> StaticSoundData {
    // A slug glancing off the boss's sealed shell: bright metallic ping
    // whipping downward over a hard little snap — unmistakably "bounced".
    let ping = sweep_exp(0.16, 2600.0, 900.0, 9.0, 0.26, sine);
    let edge = sweep_exp(0.09, 3900.0, 1700.0, 12.0, 0.10, saw);
    let snap = burst(0.03, 30.0, 0.18);
    to_sound(mix(mix(ping, &edge), &snap))
}

fn power_up() -> StaticSoundData {
    // Boss bounty claimed: a firm step up into a bright airy rise —
    // "charged", cousin to the dash whoosh it banks.
    let step = sweep(0.07, 620.0, 620.0, 4.0, 0.34, square);
    let rise = sweep_exp(0.18, 900.0, 1900.0, 5.5, 0.30, sine);
    to_sound(append(step, rise))
}

fn game_over() -> StaticSoundData {
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
        let bank = Sounds::synth();
        for (name, sound) in bank.all() {
            assert!(!sound.frames.is_empty(), "{name} is empty");
            let p = peak(sound);
            assert!(p > 0.05, "{name} is near-silent (peak {p})");
            assert!(p <= 1.0, "{name} clips (peak {p})");
            assert!(
                sound.frames.iter().all(|f| f.left.is_finite()),
                "{name} contains NaN/inf"
            );
        }
    }
}
