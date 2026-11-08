//! Pat (a clone of the mod PatPat, and more): Shift + the right button on a creature pets it. While you hold, the creature is
//! squashed and stretched in time, hearts rise, your arm swings and a soft sound plays; letting go ends it softly. No use is
//! sent to the server meanwhile, so a pet does not sit or stand up under your hand. A normal click without Shift stays vanilla.
use super::Game;
use horizon_client::particles::{ParticleOptions, ParticleType};
use horizon_core::random::RandomSource;
use std::{collections::HashMap, time::{Duration, Instant}};

/// How far you can reach a creature to pat it (as for hitting it).
const REACH: f64 = 3.0;
/// One squash and stretch.
pub(super) const PERIOD: f32 = 1.2;
/// How long a pat takes to reach full strength and to fade after it ended.
const RAMP_IN: f32 = 0.25;
const RAMP_OUT: f32 = 0.35;
const HEARTS_EVERY: Duration = Duration::from_millis(550);
const SWING_EVERY: Duration = Duration::from_millis(600);
const SOUND_EVERY: Duration = Duration::from_millis(2600);

/// The squash of a pat `seconds` after it started, 0 to 1: a slow wave, eased in; `faded` seconds after it ended it fades out.
pub(super) fn amplitude(seconds: f32, ended: Option<f32>) -> f32 {
    let wave = 0.5 - 0.5 * (std::f32::consts::TAU * seconds / PERIOD).cos();
    let rise = (seconds / RAMP_IN).clamp(0.0, 1.0);
    let fade = ended.map_or(1.0, |e| (1.0 - e / RAMP_OUT).clamp(0.0, 1.0));
    wave * rise * fade
}

/// Whether a click is a pat: the mode is on, Shift is down, and the hand is empty (or any hand may pat).
pub(super) fn is_pat(enabled: bool, shift: bool, empty_hand: bool, any_hand: bool) -> bool {
    enabled && shift && (empty_hand || any_hand)
}

#[derive(Clone, Debug)]
struct Pat {
    started: Instant,
    ended: Option<Instant>,
    hearts: Instant,
    swing: Instant,
    sound: Instant,
}

impl Pat {
    fn new(now: Instant) -> Self {
        let past = now.checked_sub(Duration::from_secs(10)).unwrap_or(now);
        Self { started: now, ended: None, hearts: past, swing: past, sound: now }
    }
    fn seconds(&self, now: Instant) -> (f32, Option<f32>) {
        let t = now.duration_since(self.started).as_secs_f32();
        (t, self.ended.map(|e| now.duration_since(e).as_secs_f32()))
    }
    fn over(&self, now: Instant) -> bool {
        self.ended.is_some_and(|e| now.duration_since(e).as_secs_f32() >= RAMP_OUT)
    }
}

#[derive(Default)]
pub(super) struct Pats {
    /// Creatures being patted (by entity id): by you and, from the server, by others.
    active: HashMap<u32, Pat>,
    /// The creature you pat.
    mine: Option<u32>,
}

impl Game<'_> {
    fn pat_settings(&self) -> (bool, bool, bool, bool, bool) {
        self.modules_ref()
            .map_or((false, false, false, false, false), |m| (m.misc.pat, m.misc.pat_hearts, m.misc.pat_sound, m.misc.pat_any_hand, m.misc.pat_players))
    }

    /// The creature a pat would go to now: in reach, in front of any block, alive, not the one you ride.
    fn pat_target(&self) -> Option<u32> {
        let (on, _, _, any_hand, players) = self.pat_settings();
        let empty = self.held().is_none();
        if !is_pat(on, self.has_shift_down(), empty, any_hand) || self.screen != super::Screen::Playing || self.player.is_none() {
            return None;
        }
        if self.belumi_freecam_active() || self.belumi_replay_active() || self.spectator() || !self.right {
            return None;
        }
        let (i, d) = self.mob_hit()?;
        if d > REACH || !self.before_block(d) {
            return None;
        }
        let m = self.mobs.get(i)?;
        (m.alive() && Some(m.id) != self.riding && (players || m.kind.id != "minecraft:player")).then_some(m.id)
    }

    /// Whether a use of the hand should do nothing because it is a pat.
    pub(super) fn belumi_pat_blocks_use(&self) -> bool {
        self.pat_target().is_some()
    }

    /// Once a frame: the pats begin and end, their effects play, and every creature gets its squash.
    pub(super) fn belumi_pat_frame(&mut self) {
        let now = Instant::now();
        let target = self.pat_target();
        if target != self.belumi_pats.mine {
            if let Some(old) = self.belumi_pats.mine.take()
                && let Some(p) = self.belumi_pats.active.get_mut(&old)
            {
                p.ended = Some(now);
                self.belumi_pat_tell(old, false);
            }
            if let Some(id) = target {
                self.belumi_pats.active.insert(id, Pat::new(now));
                self.belumi_pats.mine = Some(id);
                self.belumi_pat_tell(id, true);
            }
        }
        // The effects of each running pat.
        let (_, hearts_on, sound_on, _, _) = self.pat_settings();
        let ids: Vec<u32> = self.belumi_pats.active.keys().copied().collect();
        let mut done = Vec::new();
        for id in ids {
            let Some(m) = self.mobs.iter().find(|m| m.id == id) else {
                done.push(id);
                continue;
            };
            if !m.alive() {
                done.push(id);
                continue;
            }
            let (at, height, tamed) = ([m.position.x, m.position.y, m.position.z], m.kind.height as f32, m.extra.owner.is_some());
            let mine = self.belumi_pats.mine == Some(id);
            let Some(p) = self.belumi_pats.active.get_mut(&id) else { continue };
            if p.over(now) {
                done.push(id);
                continue;
            }
            if p.ended.is_some() {
                continue;
            }
            let (spawn_hearts, swing, sound) = (
                hearts_on && now.duration_since(p.hearts) >= HEARTS_EVERY,
                mine && now.duration_since(p.swing) >= SWING_EVERY,
                sound_on && now.duration_since(p.sound) >= SOUND_EVERY,
            );
            if spawn_hearts {
                p.hearts = now;
            }
            if swing {
                p.swing = now;
            }
            if sound {
                p.sound = now;
            }
            if spawn_hearts {
                let count = if tamed { 3 } else { 2 };
                let options = ParticleOptions::simple(ParticleType::Heart);
                for _ in 0..count {
                    let r = self.particles.level_random();
                    let (dx, dz) = ((r.next_double() - 0.5) * 0.6, (r.next_double() - 0.5) * 0.6);
                    let up = f64::from(height) + 0.1 + r.next_double() * 0.3;
                    self.particles.add(&options, [at[0] + dx, at[1] + up, at[2] + dz], [0.0, 0.03, 0.0]);
                }
            }
            if swing {
                self.swing_arm();
            }
            if sound && let Some(event) = self.pat_sound_for(id) {
                self.audio.play_unshared(&event, Some(at), 0.5, 1.15);
            }
        }
        for id in done {
            self.belumi_pats.active.remove(&id);
            if self.belumi_pats.mine == Some(id) {
                self.belumi_pats.mine = None;
            }
        }
        // Every creature's squash: patted ones by their pat, the others none.
        let active = &self.belumi_pats.active;
        for m in &mut self.mobs {
            m.extra.pat = match active.get(&m.id) {
                Some(p) => {
                    let (t, ended) = p.seconds(now);
                    amplitude(t, ended)
                }
                None => 0.0,
            };
        }
    }

    /// The soft sound of a creature being patted: a cat purrs, others use their own ambient sound.
    fn pat_sound_for(&self, id: u32) -> Option<String> {
        let kind = self.mobs.iter().find(|m| m.id == id)?.kind.id.clone();
        let short = kind.strip_prefix("minecraft:").unwrap_or(&kind);
        let names = self.audio.event_names();
        let has = |e: &str| names.iter().any(|n| n == e || n.strip_prefix("minecraft:") == Some(e));
        let purr = format!("entity.{short}.purr");
        let ambient = format!("entity.{short}.ambient");
        [purr, ambient].into_iter().find(|e| has(e)).map(|e| format!("minecraft:{e}"))
    }

    /// Tells the other players (through the server's relay) that you began or stopped patting `id`.
    fn belumi_pat_tell(&mut self, id: u32, on: bool) {
        self.belumi_ping_send_pat(id, on);
    }

    /// Another player began or stopped patting a creature (from the server).
    pub(super) fn belumi_pat_remote(&mut self, id: u32, on: bool) {
        let now = Instant::now();
        if on {
            self.belumi_pats.active.entry(id).or_insert_with(|| Pat::new(now));
        } else if let Some(p) = self.belumi_pats.active.get_mut(&id)
            && self.belumi_pats.mine != Some(id)
        {
            p.ended = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_squash_is_a_slow_wave_that_eases_in_and_fades_out() {
        assert_eq!(amplitude(0.0, None), 0.0);
        // Half a period in, the wave is at its height (and the ramp is long done).
        assert!((amplitude(PERIOD / 2.0, None) - 1.0).abs() < 1e-3);
        assert!(amplitude(0.05, None) < amplitude(0.2, None) + 1.0, "eased in");
        assert!(amplitude(0.1, None) < 0.2, "the first moments are gentle");
        assert!((amplitude(PERIOD, None)).abs() < 1e-3, "a full period is back at rest");
        // After the end it fades to nothing.
        let half = amplitude(PERIOD / 2.0, Some(RAMP_OUT / 2.0));
        assert!(half > 0.4 && half < 0.6);
        assert_eq!(amplitude(PERIOD / 2.0, Some(RAMP_OUT)), 0.0);
        for i in 0..200 {
            let a = amplitude(i as f32 * 0.03, None);
            assert!((0.0..=1.0).contains(&a));
        }
    }

    #[test]
    fn the_scale_flattens_and_widens_less_for_big_creatures_and_more_for_babies() {
        let [x, y, z] = horizon_client::mobs::render::pat_squash(1.0, 1.0, false);
        assert!(x > 1.0 && y < 1.0 && (x - z).abs() < 1e-6);
        let [_, big, _] = horizon_client::mobs::render::pat_squash(1.0, 3.0, false);
        let [_, small, _] = horizon_client::mobs::render::pat_squash(1.0, 0.5, false);
        assert!(big > y && y > small, "{big} {y} {small}");
        let [_, baby, _] = horizon_client::mobs::render::pat_squash(1.0, 1.0, true);
        assert!(baby < y);
        assert_eq!(horizon_client::mobs::render::pat_squash(0.0, 1.0, false), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn only_shift_with_an_empty_hand_pats_unless_any_hand_may() {
        assert!(is_pat(true, true, true, false));
        assert!(!is_pat(true, false, true, false), "no Shift: a normal click");
        assert!(!is_pat(true, true, false, false), "a held item uses itself");
        assert!(is_pat(true, true, false, true));
        assert!(!is_pat(false, true, true, true), "off");
    }

    #[test]
    fn a_pat_that_ended_is_over_after_the_fade() {
        let t0 = Instant::now();
        let mut p = Pat::new(t0);
        assert!(!p.over(t0));
        p.ended = Some(t0);
        assert!(!p.over(t0 + Duration::from_millis(100)));
        assert!(p.over(t0 + Duration::from_millis(400)));
    }
}
