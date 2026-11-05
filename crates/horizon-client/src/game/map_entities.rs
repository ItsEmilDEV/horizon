use super::Game;
use horizon_client::extras_hud::Dot;
use horizon_client_render::minimap::{SIZE, View};

const PLAYER: u32 = 0xFFFF_FFFF;
const HOSTILE: u32 = 0xFFFF_3030;
const NEUTRAL: u32 = 0xFFFF_D030;
const PASSIVE: u32 = 0xFF40_E040;

const HOSTILE_KINDS: &[&str] = &[
    "zombie", "zombie_villager", "husk", "drowned", "skeleton", "stray", "bogged", "wither_skeleton",
    "creeper", "spider", "cave_spider", "witch", "slime", "magma_cube", "ghast", "blaze", "piglin_brute",
    "hoglin", "zoglin", "phantom", "pillager", "vindicator", "evoker", "ravager", "vex", "guardian",
    "elder_guardian", "shulker", "silverfish", "endermite", "warden", "breeze", "wither", "ender_dragon",
    "creaking",
];
const NEUTRAL_KINDS: &[&str] = &[
    "enderman", "zombified_piglin", "piglin", "wolf", "bee", "polar_bear", "iron_golem", "goat", "llama",
    "trader_llama", "panda", "spider_jockey", "dolphin",
];
const PASSIVE_KINDS: &[&str] = &[
    "cow", "pig", "sheep", "chicken", "horse", "donkey", "mule", "rabbit", "cat", "ocelot", "fox", "parrot",
    "bat", "cod", "salmon", "tropical_fish", "pufferfish", "squid", "glow_squid", "turtle", "axolotl", "frog",
    "tadpole", "villager", "wandering_trader", "snow_golem", "mooshroom", "camel", "strider", "sniffer",
    "allay", "armadillo", "happy_ghast", "skeleton_horse", "zombie_horse",
];

pub(super) fn color_of(kind: &str, is_player: bool) -> Option<u32> {
    if is_player {
        return Some(PLAYER);
    }
    let name = kind.strip_prefix("minecraft:")?;
    if HOSTILE_KINDS.contains(&name) {
        Some(HOSTILE)
    } else if NEUTRAL_KINDS.contains(&name) {
        Some(NEUTRAL)
    } else if PASSIVE_KINDS.contains(&name) {
        Some(PASSIVE)
    } else {
        None
    }
}

impl Game<'_> {
    /// Markers for the entities around the minimap's centre, as offsets in GUI pixels.
    pub(super) fn map_entity_dots(&self, view: &View, side: f32) -> Vec<Dot> {
        if !self.settings.belumi.map_entities {
            return Vec::new();
        }
        let Some(remote) = self.remote.as_ref() else {
            return Vec::new();
        };
        let (right, forward) = view.axes();
        let per_block = f64::from(side) / (SIZE as f64 * f64::from(view.blocks_per_pixel));
        let half = f64::from(side) / 2.0 - 3.0;
        remote
            .map_entities()
            .into_iter()
            .filter_map(|(is_player, pos, kind)| {
                let color = color_of(kind, is_player)?;
                let (dx, dz) = (pos[0] - view.x, pos[2] - view.z);
                let x = (dx * right[0] + dz * right[1]) * per_block;
                let y = -(dx * forward[0] + dz * forward[1]) * per_block;
                let inside = if view.round { x.hypot(y) <= half } else { x.abs() <= half && y.abs() <= half };
                (inside && (dx != 0.0 || dz != 0.0)).then_some(Dot { x: x as f32, y: y as f32, color })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobs_get_their_threat_colour_and_props_none() {
        assert_eq!(color_of("minecraft:zombie", false), Some(HOSTILE));
        assert_eq!(color_of("minecraft:cow", false), Some(PASSIVE));
        assert_eq!(color_of("minecraft:wolf", false), Some(NEUTRAL));
        assert_eq!(color_of("minecraft:player", true), Some(PLAYER));
        assert_eq!(color_of("minecraft:item_display", false), None);
        assert_eq!(color_of("minecraft:armor_stand", false), None);
    }
}
