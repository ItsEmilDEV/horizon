//! Accurate block placement (the functions of the Fabric mod "Accurate Block Placement Reborn"): with the right button held
//! blocks are placed as fast as the crosshair moves to a new spot, instead of one every fourth tick, and the pause between two
//! blocks broken in creative is gone. The vanilla wait stays where a faster repeat would do harm: a block that opens
//! something (a chest, a shulker box, a door), and scaffolding, which a held button would otherwise stack on itself.
//! Off by default: some servers count it as cheating.
use super::Game;

/// What the crosshair points at, for the repeat of a held right button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// Nothing, or a plain block: the new block goes next to it.
    Plain,
    Scaffolding,
    /// A block that does something when used (opens a menu, toggles).
    Interactive,
}

const VANILLA_USE_DELAY: u32 = 4;
const VANILLA_BREAK_DELAY: u32 = 5;

const INTERACTIVE: [&str; 33] = [
    "chest", "barrel", "shulker_box", "furnace", "smoker", "hopper", "dispenser", "dropper", "crafting_table", "anvil",
    "enchanting_table", "brewing_stand", "loom", "stonecutter", "cartography_table", "smithing_table", "grindstone", "beacon",
    "lectern", "door", "trapdoor", "button", "lever", "fence_gate", "bed", "bell", "note_block", "cauldron", "crafter",
    "jukebox", "repeater", "comparator", "daylight_detector",
];

/// How a block name counts for the repeat.
pub(super) fn classify(block: &str) -> Target {
    let short = block.strip_prefix("minecraft:").unwrap_or(block);
    if short == "scaffolding" {
        Target::Scaffolding
    } else if INTERACTIVE.iter().any(|k| short.contains(k)) {
        Target::Interactive
    } else {
        Target::Plain
    }
}

/// The wait (ticks) before the held right button uses again.
pub(super) fn use_delay(on: bool, block_in_hand: bool, target: Target, holding_scaffolding: bool) -> u32 {
    let harmless = match target {
        Target::Plain => true,
        Target::Scaffolding => !holding_scaffolding,
        Target::Interactive => false,
    };
    if on && block_in_hand && harmless { 0 } else { VANILLA_USE_DELAY }
}

/// The wait (ticks) after a block was broken.
pub(super) fn break_delay(on: bool, breaking_too: bool, creative: bool) -> u32 {
    if on && breaking_too && creative { 0 } else { VANILLA_BREAK_DELAY }
}

impl Game<'_> {
    fn placement_on(&self) -> bool {
        self.modules_ref().is_some_and(|m| m.misc.accurate_placement)
    }

    /// The wait for the next use of the held right button, in ticks.
    pub(super) fn belumi_use_delay(&self) -> u32 {
        if !self.placement_on() {
            return VANILLA_USE_DELAY;
        }
        let held = self.held().map(|s| s.item.to_string()).unwrap_or_default();
        let block_in_hand = !held.is_empty() && self.pack.assets.registry.default_state(&held).is_some();
        let target = self.pick().map_or(Target::Plain, |h| classify(&self.res.facts().get(self.world.block(h.pos[0], h.pos[1], h.pos[2])).name));
        use_delay(true, block_in_hand, target, held == "minecraft:scaffolding")
    }

    /// The wait after a block was broken, in ticks.
    pub(super) fn belumi_destroy_delay(&self) -> u32 {
        let breaking = self.modules_ref().is_some_and(|m| m.misc.accurate_break);
        break_delay(self.placement_on(), breaking, self.creative)
    }

    pub(super) fn belumi_placement_key(&mut self, key: &str, pressed: bool) {
        let want = self.modules_ref().map(|m| m.misc.accurate_key.clone()).unwrap_or_default();
        if !pressed || want.is_empty() || key != want || !self.native_playing() {
            return;
        }
        self.belumi_placement_toggle();
    }

    pub(super) fn belumi_placement_toggle(&mut self) {
        let on = {
            let m = self.modules();
            m.misc.accurate_placement = !m.misc.accurate_placement;
            m.misc.accurate_placement
        };
        self.modules_changed();
        self.toast(if on {
            "Accurate placement: ON. Some servers count this as cheating."
        } else {
            "Accurate placement: OFF"
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_placed_without_waiting_only_when_the_mode_is_on_and_a_block_is_in_hand() {
        assert_eq!(use_delay(true, true, Target::Plain, false), 0);
        assert_eq!(use_delay(false, true, Target::Plain, false), 4, "off: vanilla");
        assert_eq!(use_delay(true, false, Target::Plain, false), 4, "an item that is not a block keeps the wait (food, a bow)");
    }

    #[test]
    fn a_block_that_opens_something_keeps_the_vanilla_wait() {
        assert_eq!(use_delay(true, true, Target::Interactive, false), 4);
        assert_eq!(classify("minecraft:chest"), Target::Interactive);
        assert_eq!(classify("minecraft:purple_shulker_box"), Target::Interactive);
        assert_eq!(classify("minecraft:oak_door"), Target::Interactive);
        assert_eq!(classify("minecraft:stone"), Target::Plain);
        assert_eq!(classify("minecraft:grass_block"), Target::Plain);
    }

    #[test]
    fn scaffolding_only_matters_when_scaffolding_is_what_is_held() {
        assert_eq!(classify("minecraft:scaffolding"), Target::Scaffolding);
        assert_eq!(use_delay(true, true, Target::Scaffolding, true), 4, "scaffolding on scaffolding stays reachable");
        assert_eq!(use_delay(true, true, Target::Scaffolding, false), 0, "another block next to scaffolding is fast");
    }

    #[test]
    fn creative_breaking_has_no_pause_in_the_mode_and_survival_keeps_its_own() {
        assert_eq!(break_delay(true, true, true), 0);
        assert_eq!(break_delay(true, false, true), 5, "the break option is off");
        assert_eq!(break_delay(true, true, false), 5, "survival");
        assert_eq!(break_delay(false, true, true), 5);
    }
}
