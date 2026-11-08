//! Sound controller: single sound events turned down or muted (the mod of that name). The page lists the sounds played last;
//! each gets Mute, 25, 50 and 100 buttons. The levels are kept in `belumi-modules.toml` (`sound_levels`, in percent).
//! Above 100 the sound is boosted after the game's own volume (up to 200 %); a very loud one is held in check by the mixer's limiter.
use super::{Game, belumi_client::{Act, Page}};
use horizon_client::hud::HudBatch;
use std::collections::BTreeMap;

const ROWS: usize = 9;

/// The rows of the page: sounds that have a level first (so a muted one can be brought back), then the ones played last.
pub(super) fn rows(levels: &BTreeMap<String, u8>, recent: &[String], max: usize) -> Vec<String> {
    let mut out: Vec<String> = levels.keys().cloned().collect();
    for e in recent {
        if out.len() >= max {
            break;
        }
        if !out.contains(e) {
            out.push(e.clone());
        }
    }
    out.truncate(max);
    out
}

/// A level of 100 is no level: the entry goes.
pub(super) fn set_level(levels: &mut BTreeMap<String, u8>, event: &str, percent: u8) {
    if percent == 100 {
        levels.remove(event);
    } else {
        levels.insert(event.to_owned(), percent);
    }
}

/// The name as the player reads it: `block.stone.break` from `minecraft:block.stone.break`.
pub(super) fn short(event: &str) -> &str {
    event.strip_prefix("minecraft:").unwrap_or(event)
}

impl Game<'_> {
    /// Once a frame: the audio gets the levels when they changed.
    pub(super) fn belumi_sounds_frame(&mut self) {
        let Some(levels) = self.modules_ref().map(|m| m.misc.sound_levels.clone()) else { return };
        if levels == self.belumi_sound_levels {
            return;
        }
        self.audio.set_event_levels(levels.iter().map(|(e, p)| (e.clone(), f32::from(*p) / 100.0)));
        self.belumi_sound_levels = levels;
    }

    pub(super) fn belumi_sounds_act(&mut self, act: &Act) -> bool {
        match act {
            Act::SoundLevel(event, percent) => {
                set_level(&mut self.modules().misc.sound_levels, event, *percent);
                self.modules_changed();
                true
            }
            Act::SoundsReset => {
                self.modules().misc.sound_levels.clear();
                self.modules_changed();
                true
            }
            _ => false,
        }
    }

    pub(super) fn draw_sounds_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let levels = self.modules_ref().map(|m| m.misc.sound_levels.clone()).unwrap_or_default();
        let listed = rows(&levels, &self.audio.recent_events(), ROWS);
        let pw = 480.0_f32.min(w - 12.0);
        let ph = (56.0 + listed.len().max(1) as f32 * 20.0 + 22.0).min(h - 16.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Sound controller");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        self.client_button(batch, [x + pw - 118.0, y + 3.0, 58.0, 14.0], "Reset", None, Act::SoundsReset);
        let mut ry = y + 28.0;
        if listed.is_empty() {
            self.native_text(batch, "Play the game for a moment: the sounds you hear show up here.", [x + 10.0, ry], 0x008A_93A8, false);
            return;
        }
        for event in &listed {
            let level = levels.get(event).copied();
            let color = match level {
                Some(0) => 0x00FF_6B6B,
                Some(_) => 0x00FF_E14D,
                None => 0x00FF_FFFF,
            };
            self.native_text(batch, short(event), [x + 10.0, ry + 4.0], color, false);
            let bx = x + pw - 250.0;
            for (i, (label, percent)) in [("Mute", 0u8), ("25", 25), ("50", 50), ("100", 100), ("150", 150), ("200", 200)].into_iter().enumerate() {
                let active = match (level, percent) {
                    (None, 100) => true,
                    (Some(l), p) => l == p,
                    _ => false,
                };
                self.client_button(batch, [bx + i as f32 * 46.0, ry, 42.0, 16.0], label, Some(active), Act::SoundLevel(event.clone(), percent));
            }
            ry += 20.0;
        }
        self.native_text(batch, "Above 100 a sound is louder than the game plays it.", [x + 10.0, ry + 4.0], 0x008A_93A8, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_list_the_adjusted_sounds_first_then_the_recent_ones_without_repeats() {
        let levels: BTreeMap<String, u8> = [("minecraft:entity.cow.ambient".to_owned(), 0)].into();
        let recent = vec!["minecraft:block.stone.break".to_owned(), "minecraft:entity.cow.ambient".to_owned(), "minecraft:block.grass.step".to_owned()];
        assert_eq!(rows(&levels, &recent, 9), vec!["minecraft:entity.cow.ambient", "minecraft:block.stone.break", "minecraft:block.grass.step"]);
        assert_eq!(rows(&levels, &recent, 2).len(), 2);
        assert!(rows(&BTreeMap::new(), &[], 9).is_empty());
    }

    #[test]
    fn a_level_of_one_hundred_removes_the_entry() {
        let mut levels = BTreeMap::new();
        set_level(&mut levels, "minecraft:a", 0);
        set_level(&mut levels, "minecraft:b", 50);
        assert_eq!(levels.len(), 2);
        set_level(&mut levels, "minecraft:a", 100);
        assert_eq!(levels.keys().collect::<Vec<_>>(), vec!["minecraft:b"]);
        set_level(&mut levels, "minecraft:c", 150);
        assert_eq!(levels.get("minecraft:c"), Some(&150), "a boost is kept");
    }

    #[test]
    fn names_read_without_the_namespace() {
        assert_eq!(short("minecraft:block.stone.break"), "block.stone.break");
        assert_eq!(short("mod:x"), "mod:x");
    }
}
