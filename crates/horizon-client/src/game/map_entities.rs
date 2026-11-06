use horizon_client::hud::rgba;
use super::Game;
use horizon_client::extras_hud::Dot;
use horizon_client::hud::HudBatch;
use horizon_client::items::ItemStack;
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

/// Mobs that have no spawn egg to stand for them.
const NO_EGG: &[&str] = &["iron_golem", "snow_golem", "wither", "ender_dragon"];

/// What stands for an entity while the player list is held.
#[derive(Clone)]
pub(super) enum Icon {
    /// A player's face: the skin sprite the player list draws from.
    Face(String),
    /// A mob's spawn egg.
    Egg(String),
    /// Nothing better: a dot in this colour.
    Dot(u32),
}

/// An entity for the maps' player-list view: where it is, its icon and its name.
#[derive(Clone)]
pub(super) struct Row {
    pub pos: [f64; 3],
    pub icon: Icon,
    pub label: Option<String>,
}

/// Where a waypoint sits on the minimap, pinned to the rim when it is out of range.
pub(super) fn waypoint_spot(x: f64, y: f64, half: f64, round: bool) -> (f32, f32) {
    let outside = if round { x.hypot(y) > half } else { x.abs() > half || y.abs() > half };
    if !outside {
        return (x as f32, y as f32);
    }
    let scale = if round { half / x.hypot(y) } else { half / x.abs().max(y.abs()) };
    ((x * scale) as f32, (y * scale) as f32)
}

impl Game<'_> {
    /// Markers for the waypoints of this world and dimension, on the rim when out of range.
    pub(super) fn map_waypoint_dots(&mut self, view: &View, side: f32) -> Vec<Dot> {
        let cfg = &self.settings.belumi;
        if !cfg.waypoints || !cfg.waypoint_labels {
            return Vec::new();
        }
        let (right, forward) = view.axes();
        let per_block = f64::from(side) / (SIZE as f64 * f64::from(view.blocks_per_pixel));
        let half = (f64::from(side) / 2.0 - 4.0).max(1.0);
        let all = self.belumi_visible_waypoints();
        all.into_iter()
            .map(|w| {
                let (dx, dz) = (f64::from(w.x) + 0.5 - view.x, f64::from(w.z) + 0.5 - view.z);
                let x = (dx * right[0] + dz * right[1]) * per_block;
                let y = -(dx * forward[0] + dz * forward[1]) * per_block;
                let (x, y) = waypoint_spot(x, y, half, view.round);
                let [_, r, g, b] = w.color.to_be_bytes();
                Dot { x, y, color: rgba(r, g, b, 255), waypoint: true }
            })
            .collect()
    }

    /// The player list is held: the maps show faces and names instead of dots.
    pub(super) fn map_icons_wanted(&self) -> bool {
        self.settings.belumi.map_entities
            && self.screen == super::Screen::Playing
            && self.keys.contains(&winit::keyboard::KeyCode::Tab)
    }

    /// The entities for the player-list view: players with their face and name (always), mobs with their egg
    /// and the name they were given.
    pub(super) fn map_icon_rows(&mut self) -> Vec<Row> {
        let Some(remote) = self.remote.as_ref() else {
            return Vec::new();
        };
        let me = remote.profile();
        let list = remote.map_entity_list();
        let info: std::collections::HashMap<u128, (String, String)> = remote
            .tab_players
            .iter()
            .map(|(u, p)| (*u, (p.name.clone(), p.skin.clone())))
            .collect();
        let mut rows = Vec::new();
        for (id, player, pos, kind) in list {
            if let Some(uuid) = player {
                if uuid == me {
                    continue;
                }
                let (name, skin) = info.get(&uuid).cloned().unwrap_or_default();
                let skin = self.face_skin(uuid, &skin);
                rows.push(Row { pos, icon: Icon::Face(skin), label: (!name.is_empty()).then_some(name) });
                continue;
            }
            let Some(color) = color_of(&kind, false) else { continue };
            let short = kind.strip_prefix("minecraft:").unwrap_or(&kind).to_owned();
            let icon = if NO_EGG.contains(&short.as_str()) { Icon::Dot(color) } else { Icon::Egg(short) };
            let label = self.mobs.iter().find(|m| m.id == id).and_then(|m| m.custom_name.clone()).filter(|n| !n.is_empty());
            rows.push(Row { pos, icon, label });
        }
        rows
    }

    /// One row on a map at screen position (x, y): its icon centred there and its name small below.
    pub(super) fn draw_map_icon(&self, batch: &mut HudBatch, row: &Row, x: f32, y: f32, face: f32) {
        match &row.icon {
            Icon::Face(skin) => {
                let mut draws = Vec::new();
                horizon_client::tab_list::face(&mut draws, skin, (x - face / 2.0).round() as i32, (y - face / 2.0).round() as i32, face as i32, true, false, 0xFFFF_FFFF);
                horizon_client::hud_layout::paint(batch, &self.gui.atlas, &draws);
            }
            Icon::Egg(kind) => {
                self.draw_item_model(batch, &ItemStack::new(&format!("minecraft:{kind}_spawn_egg"), 1), (x - 8.0).round(), (y - 8.0).round());
            }
            Icon::Dot(color) => {
                self.native_fill(batch, [x - 2.5, y - 2.5, 5.0, 5.0], 0xFF00_0000);
                self.native_fill(batch, [x - 2.0, y - 2.0, 4.0, 4.0], *color);
            }
        }
        if let Some(name) = &row.label {
            let below = match row.icon {
                Icon::Egg(_) => 8.0,
                _ => face / 2.0 + 1.0,
            };
            let w = self.native_text_width(name);
            self.native_fill(batch, [x - w / 2.0 - 1.0, y + below, w + 2.0, 9.0], 0x6000_0000);
            self.native_text(batch, name, [(x - w / 2.0).round(), y + below + 1.0], 0x00FF_FFFF, false);
        }
    }

    /// Markers for the entities around the minimap's centre, as offsets in GUI pixels.
    pub(super) fn map_entity_dots(&self, view: &View, side: f32) -> Vec<Dot> {
        if !self.settings.belumi.map_entities || self.map_icons_wanted() {
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
                (inside && (dx != 0.0 || dz != 0.0)).then_some(Dot { x: x as f32, y: y as f32, color, waypoint: false })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_waypoints_sit_on_the_rim_in_their_direction() {
        assert_eq!(waypoint_spot(10.0, 5.0, 30.0, true), (10.0, 5.0));
        let (x, y) = waypoint_spot(60.0, 0.0, 30.0, true);
        assert!((x - 30.0).abs() < 1e-4 && y.abs() < 1e-4);
        let (x, y) = waypoint_spot(60.0, 30.0, 30.0, false);
        assert_eq!((x, y), (30.0, 15.0));
        assert_eq!(waypoint_spot(-5.0, 100.0, 40.0, false), (-2.0, 40.0));
    }

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
