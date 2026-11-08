//! Gamma utils: the brightness beyond the options slider, and keys for it. The light map brightens dark places by a factor
//! (the slider's 0 to 1); this adds up to 15 x 0.6 on top, so a cave is readable without a torch. A key cycles the levels and
//! another switches fullbright. Both are off by default; some servers count a bright cave as an unfair help.
use super::Game;

const STEP: f32 = 0.6;
const LEVELS: [u8; 5] = [0, 4, 8, 12, 15];

/// How much gamma a level adds to the slider's.
pub(super) fn boost(level: u8) -> f32 {
    f32::from(level.min(15)) * STEP
}

/// The next level of the cycle (0, 4, 8, 12, 15, back to 0); a level between them goes to the next one up.
pub(super) fn next_level(level: u8) -> u8 {
    LEVELS.iter().copied().find(|l| *l > level).unwrap_or(0)
}

impl Game<'_> {
    /// Once a frame: the renderer gets the slider's gamma plus the boost, and fullbright as set.
    pub(super) fn belumi_gamma_frame(&mut self) {
        let level = self.modules_ref().map_or(0, |m| m.misc.gamma_level);
        let want = self.settings.gamma + boost(level);
        let bright = self.settings.extras.fullbright;
        if let Some(r) = self.renderer.as_mut() {
            if (r.gamma - want).abs() > f32::EPSILON {
                r.gamma = want;
            }
            if r.fullbright != bright {
                r.fullbright = bright;
            }
        }
    }

    pub(super) fn belumi_gamma_cycle(&mut self) {
        let level = {
            let m = self.modules();
            m.misc.gamma_level = next_level(m.misc.gamma_level);
            m.misc.gamma_level
        };
        self.modules_changed();
        self.toast(if level == 0 { "Gamma boost off".to_owned() } else { format!("Gamma boost {level} of 15") });
    }

    pub(super) fn belumi_fullbright_toggle(&mut self) {
        self.settings.extras.fullbright = !self.settings.extras.fullbright;
        self.settings.save(&Game::options_path());
        let on = self.settings.extras.fullbright;
        self.toast(if on { "Fullbright ON" } else { "Fullbright OFF" });
    }

    pub(super) fn belumi_gamma_key(&mut self, key: &str, pressed: bool) {
        if !pressed || !self.native_playing() {
            return;
        }
        let (gamma_key, bright_key) = match self.modules_ref() {
            Some(m) => (m.misc.gamma_key.clone(), m.misc.fullbright_key.clone()),
            None => return,
        };
        if !gamma_key.is_empty() && key == gamma_key {
            self.belumi_gamma_cycle();
        } else if !bright_key.is_empty() && key == bright_key {
            self.belumi_fullbright_toggle();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_levels_cycle_and_a_stray_level_goes_to_the_next_step() {
        let mut level = 0;
        let mut seen = Vec::new();
        for _ in 0..6 {
            level = next_level(level);
            seen.push(level);
        }
        assert_eq!(seen, vec![4, 8, 12, 15, 0, 4]);
        assert_eq!(next_level(5), 8);
        assert_eq!(next_level(200), 0);
    }

    #[test]
    fn the_boost_grows_with_the_level_and_stops_at_fifteen() {
        assert_eq!(boost(0), 0.0);
        assert!((boost(15) - 9.0).abs() < 1e-6);
        assert_eq!(boost(15), boost(99));
        assert!(boost(8) > boost(4));
    }
}
