use crate::extras::Corner;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub const FILE: &str = "belumi.toml";
pub const ZOOM_KEY: &str = "belumi.zoom";
pub const INSPECTOR_KEY: &str = "belumi.inspector";
pub const WAYPOINT_KEY: &str = "belumi.waypoint";
pub const WORLDMAP_KEY: &str = "belumi.worldmap";
pub const PLUGINS_DIR: &str = "belumi-plugins";
pub const FREECAM_KEY: &str = "belumi.freecam";
pub const FREECAM_SPEEDS: [f32; 4] = [5.0, 12.0, 30.0, 60.0];

pub const ZOOM_LEVELS: [f32; 5] = [1.5, 2.0, 3.0, 4.0, 8.0];
pub const MINIMAP_SIZES: [f32; 3] = [64.0, 96.0, 128.0];
pub const MINIMAP_ZOOMS: [f32; 4] = [0.5, 1.0, 2.0, 4.0];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Belumi {
    pub hud: bool,
    pub hud_corner: String,
    pub hud_coords: bool,
    pub hud_fps: bool,
    pub hud_ping: bool,
    pub hud_direction: bool,
    pub hud_speed: bool,
    pub zoom: bool,
    pub zoom_key: String,
    pub zoom_level: u8,
    pub zoom_smooth: bool,
    pub zoom_scroll: bool,
    pub inspector: bool,
    pub inspector_key: String,
    pub inspector_corner: String,
    pub minimap: bool,
    pub minimap_corner: String,
    pub minimap_size: u8,
    pub minimap_zoom: u8,
    pub minimap_rotate: bool,
    pub minimap_round: bool,
    pub waypoints: bool,
    pub waypoint_key: String,
    pub waypoint_list: bool,
    pub waypoint_labels: bool,
    pub worldmap: bool,
    pub worldmap_key: String,
    pub plugins: bool,
    pub plugin_corner: String,
    pub freecam: bool,
    pub freecam_key: String,
    pub freecam_speed: u8,
}
impl Default for Belumi {
    fn default() -> Self {
        Self {
            hud: false,
            hud_corner: Corner::TopLeft.name().into(),
            hud_coords: true,
            hud_fps: true,
            hud_ping: true,
            hud_direction: true,
            hud_speed: false,
            zoom: true,
            zoom_key: "key.keyboard.c".into(),
            zoom_level: 2,
            zoom_smooth: true,
            zoom_scroll: true,
            inspector: false,
            inspector_key: "key.keyboard.f8".into(),
            inspector_corner: Corner::BottomLeft.name().into(),
            minimap: false,
            minimap_corner: Corner::TopRight.name().into(),
            minimap_size: 1,
            minimap_zoom: 1,
            minimap_rotate: true,
            minimap_round: false,
            waypoints: true,
            waypoint_key: "key.keyboard.b".into(),
            waypoint_list: true,
            waypoint_labels: true,
            worldmap: false,
            worldmap_key: "key.keyboard.m".into(),
            plugins: false,
            plugin_corner: Corner::BottomRight.name().into(),
            freecam: false,
            freecam_key: "key.keyboard.f9".into(),
            freecam_speed: 1,
        }
    }
}

fn corner_of(name: &str, default: Corner) -> Corner {
    Corner::ALL
        .into_iter()
        .find(|c| c.name() == name)
        .unwrap_or(default)
}

impl Belumi {
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(FILE))
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }
    pub fn parse(text: &str) -> Self {
        let mut b: Self = toml::from_str(text).unwrap_or_default();
        b.zoom_level = b.zoom_level.min(ZOOM_LEVELS.len() as u8 - 1);
        b.minimap_size = b.minimap_size.min(MINIMAP_SIZES.len() as u8 - 1);
        b.minimap_zoom = b.minimap_zoom.min(MINIMAP_ZOOMS.len() as u8 - 1);
        b.freecam_speed = b.freecam_speed.min(FREECAM_SPEEDS.len() as u8 - 1);
        b
    }
    pub fn serialize(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
    pub fn save(&self, dir: &Path) {
        let path = dir.join(FILE);
        if *self == Self::default() && !path.exists() {
            return;
        }
        let _ = fs::write(path, self.serialize());
    }
    pub fn corner(&self) -> Corner {
        corner_of(&self.hud_corner, Corner::TopLeft)
    }
    pub fn inspector_corner(&self) -> Corner {
        corner_of(&self.inspector_corner, Corner::BottomLeft)
    }
    pub fn minimap_corner(&self) -> Corner {
        corner_of(&self.minimap_corner, Corner::TopRight)
    }
    pub fn plugin_corner(&self) -> Corner {
        corner_of(&self.plugin_corner, Corner::BottomRight)
    }
    pub fn zoom_factor(&self) -> f32 {
        ZOOM_LEVELS[usize::from(self.zoom_level).min(ZOOM_LEVELS.len() - 1)]
    }
    pub fn minimap_pixels(&self) -> f32 {
        MINIMAP_SIZES[usize::from(self.minimap_size).min(MINIMAP_SIZES.len() - 1)]
    }
    pub fn minimap_blocks_per_pixel(&self) -> f32 {
        MINIMAP_ZOOMS[usize::from(self.minimap_zoom).min(MINIMAP_ZOOMS.len() - 1)]
    }
    pub fn freecam_blocks_per_second(&self) -> f32 {
        FREECAM_SPEEDS[usize::from(self.freecam_speed).min(FREECAM_SPEEDS.len() - 1)]
    }
    pub fn set_key(&mut self, name: &str, value: String) -> bool {
        match SPECS.iter().find(|s| s.key_name() == Some(name)) {
            Some(Spec {
                kind: Kind::Key { set, .. },
                ..
            }) => {
                set(self, value);
                true
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Hud,
    HudCorner,
    HudCoords,
    HudFps,
    HudPing,
    HudDirection,
    HudSpeed,
    Zoom,
    ZoomKey,
    ZoomLevel,
    ZoomSmooth,
    ZoomScroll,
    Minimap,
    MinimapCorner,
    MinimapSize,
    MinimapZoom,
    MinimapRotate,
    MinimapRound,
    Waypoints,
    WaypointKey,
    WaypointList,
    WaypointLabels,
    Worldmap,
    WorldmapKey,
    Plugins,
    PluginCorner,
    Freecam,
    FreecamKey,
    FreecamSpeed,
    Inspector,
    InspectorKey,
    InspectorCorner,
}

enum Kind {
    Switch {
        get: fn(&Belumi) -> bool,
        set: fn(&mut Belumi, bool),
    },
    Key {
        name: &'static str,
        get: fn(&Belumi) -> &str,
        set: fn(&mut Belumi, String),
    },
    Choice {
        labels: &'static [&'static str],
        get: fn(&Belumi) -> usize,
        set: fn(&mut Belumi, usize),
    },
}

struct Spec {
    item: Item,
    caption: &'static str,
    description: &'static str,
    parent: Option<Item>,
    kind: Kind,
}
impl Spec {
    fn key_name(&self) -> Option<&'static str> {
        match self.kind {
            Kind::Key { name, .. } => Some(name),
            _ => None,
        }
    }
}

const CORNERS: &[&str] = &["Top left", "Top right", "Bottom left", "Bottom right"];
const SIZES: &[&str] = &["Small", "Medium", "Large"];
const MAP_ZOOMS: &[&str] = &["0.5 blocks/px", "1 block/px", "2 blocks/px", "4 blocks/px"];
const FREECAM_LABELS: &[&str] = &["Slow", "Normal", "Fast", "Very fast"];
const LEVELS: &[&str] = &["1.5x", "2x", "3x", "4x", "8x"];

fn corner_index(name: &str, default: Corner) -> usize {
    let c = corner_of(name, default);
    Corner::ALL.iter().position(|x| *x == c).unwrap_or(0)
}

macro_rules! switch {
    ($item:ident, $cap:expr, $desc:expr, $parent:expr, $field:ident) => {
        Spec {
            item: Item::$item,
            caption: $cap,
            description: $desc,
            parent: $parent,
            kind: Kind::Switch {
                get: |b| b.$field,
                set: |b, v| b.$field = v,
            },
        }
    };
}
macro_rules! key {
    ($item:ident, $cap:expr, $parent:expr, $name:expr, $field:ident) => {
        Spec {
            item: Item::$item,
            caption: $cap,
            description: "Click, then press a key; Escape unbinds it.",
            parent: $parent,
            kind: Kind::Key {
                name: $name,
                get: |b| &b.$field,
                set: |b, v| b.$field = v,
            },
        }
    };
}
macro_rules! corner {
    ($item:ident, $cap:expr, $desc:expr, $parent:expr, $field:ident, $default:expr) => {
        Spec {
            item: Item::$item,
            caption: $cap,
            description: $desc,
            parent: $parent,
            kind: Kind::Choice {
                labels: CORNERS,
                get: |b| corner_index(&b.$field, $default),
                set: |b, v| b.$field = Corner::ALL[v.min(3)].name().into(),
            },
        }
    };
}
macro_rules! choice {
    ($item:ident, $cap:expr, $desc:expr, $parent:expr, $labels:expr, $field:ident) => {
        Spec {
            item: Item::$item,
            caption: $cap,
            description: $desc,
            parent: $parent,
            kind: Kind::Choice {
                labels: $labels,
                get: |b| usize::from(b.$field),
                set: |b, v| b.$field = v as u8,
            },
        }
    };
}

const SPECS: &[Spec] = &[
    switch!(Hud, "HUD", "Shows a small overlay with the lines below.", None, hud),
    corner!(HudCorner, "HUD corner", "The screen corner the overlay sits in.", Some(Item::Hud), hud_corner, Corner::TopLeft),
    switch!(HudCoords, "Coordinates", "Your block position.", Some(Item::Hud), hud_coords),
    switch!(HudFps, "FPS", "Frames per second.", Some(Item::Hud), hud_fps),
    switch!(HudPing, "Ping", "Round-trip time to the server in multiplayer.", Some(Item::Hud), hud_ping),
    switch!(HudDirection, "Direction", "The way you face and the compass heading.", Some(Item::Hud), hud_direction),
    switch!(HudSpeed, "Speed", "Your speed in blocks per second.", Some(Item::Hud), hud_speed),
    switch!(Zoom, "Zoom", "Hold the zoom key to narrow the field of view.", None, zoom),
    key!(ZoomKey, "Zoom key", Some(Item::Zoom), ZOOM_KEY, zoom_key),
    choice!(ZoomLevel, "Zoom level", "How far the zoom key magnifies.", Some(Item::Zoom), LEVELS, zoom_level),
    switch!(ZoomSmooth, "Smooth zoom", "Eases the field of view in and out.", Some(Item::Zoom), zoom_smooth),
    switch!(ZoomScroll, "Scroll adjusts zoom", "The mouse wheel changes the zoom while the key is held.", Some(Item::Zoom), zoom_scroll),
    switch!(Minimap, "Minimap", "A small map of the surroundings in a screen corner.", None, minimap),
    corner!(MinimapCorner, "Minimap corner", "The screen corner the minimap sits in.", Some(Item::Minimap), minimap_corner, Corner::TopRight),
    choice!(MinimapSize, "Minimap size", "The minimap's side on screen.", Some(Item::Minimap), SIZES, minimap_size),
    choice!(MinimapZoom, "Minimap zoom", "How many blocks one map pixel covers.", Some(Item::Minimap), MAP_ZOOMS, minimap_zoom),
    switch!(MinimapRotate, "Rotate with view", "Forward is up; otherwise north is up.", Some(Item::Minimap), minimap_rotate),
    switch!(MinimapRound, "Round minimap", "A round map instead of a square one.", Some(Item::Minimap), minimap_round),
    switch!(Waypoints, "Waypoints", "Save places with the key or /waypoint add NAME; remove with /waypoint remove NAME.", None, waypoints),
    key!(WaypointKey, "Waypoint key", Some(Item::Waypoints), WAYPOINT_KEY, waypoint_key),
    switch!(WaypointList, "Nearest list", "Lists the nearest waypoints with distance and direction.", Some(Item::Waypoints), waypoint_list),
    switch!(WaypointLabels, "Map markers", "Marks waypoints on the minimap and the world map.", Some(Item::Waypoints), waypoint_labels),
    switch!(Worldmap, "World map", "Remembers explored terrain and shows it in a full-screen overlay.", None, worldmap),
    key!(WorldmapKey, "World map key", Some(Item::Worldmap), WORLDMAP_KEY, worldmap_key),
    switch!(Plugins, "Client plugins", "Runs the WebAssembly plugins in belumi-plugins/ (needs a build with plugin support). Type /plugins to list them.", None, plugins),
    corner!(PluginCorner, "Plugin HUD corner", "The screen corner plugin HUD lines appear in.", Some(Item::Plugins), plugin_corner, Corner::BottomRight),
    switch!(Freecam, "Freecam", "Detaches the camera from your body: fly around while you stay put. The wheel changes the speed.", None, freecam),
    key!(FreecamKey, "Freecam key", Some(Item::Freecam), FREECAM_KEY, freecam_key),
    choice!(FreecamSpeed, "Freecam speed", "How fast the camera flies; sprint triples it.", Some(Item::Freecam), FREECAM_LABELS, freecam_speed),
    switch!(Inspector, "Packet inspector", "A key shows live packet rates, sizes and the latest packets.", None, inspector),
    key!(InspectorKey, "Inspector key", Some(Item::Inspector), INSPECTOR_KEY, inspector_key),
    corner!(InspectorCorner, "Inspector corner", "The screen corner the inspector sits in.", Some(Item::Inspector), inspector_corner, Corner::BottomLeft),
];

pub fn rows() -> Vec<Result<Item, &'static str>> {
    use Item::*;
    let sections: &[(&'static str, &[Item])] = &[
        ("Heads-up display", &[Hud, HudCorner, HudCoords, HudFps, HudPing, HudDirection, HudSpeed]),
        ("Zoom", &[Zoom, ZoomKey, ZoomLevel, ZoomSmooth, ZoomScroll]),
        ("Maps", &[Minimap, MinimapCorner, MinimapSize, MinimapZoom, MinimapRotate, MinimapRound, Worldmap, WorldmapKey]),
        ("Waypoints", &[Waypoints, WaypointKey, WaypointList, WaypointLabels]),
        ("Plugins", &[Plugins, PluginCorner]),
        ("Camera", &[Freecam, FreecamKey, FreecamSpeed]),
        ("Debug", &[Inspector, InspectorKey, InspectorCorner]),
    ];
    sections
        .iter()
        .flat_map(|(head, items)| {
            std::iter::once(Err(*head)).chain(items.iter().map(|i| Ok(*i)))
        })
        .collect()
}

impl Item {
    fn spec(self) -> &'static Spec {
        SPECS
            .iter()
            .find(|s| s.item == self)
            .expect("every item has a spec")
    }
    pub fn caption(self) -> &'static str {
        self.spec().caption
    }
    pub fn description(self) -> &'static str {
        self.spec().description
    }
    pub fn parent(self) -> Option<Item> {
        self.spec().parent
    }
    pub fn switch(self, b: &Belumi) -> Option<bool> {
        match self.spec().kind {
            Kind::Switch { get, .. } => Some(get(b)),
            _ => None,
        }
    }
    pub fn value(self, b: &Belumi) -> String {
        match self.spec().kind {
            Kind::Switch { get, .. } => if get(b) { "On" } else { "Off" }.into(),
            Kind::Key { get, .. } => crate::keybinds::display_name(get(b)).plain(),
            Kind::Choice { labels, get, .. } => labels[get(b).min(labels.len() - 1)].into(),
        }
    }
    pub fn cycle(self, b: &mut Belumi, back: bool) {
        match self.spec().kind {
            Kind::Switch { get, set } => set(b, !get(b)),
            Kind::Choice { labels, get, set } => {
                let n = labels.len();
                let i = get(b);
                set(b, if back { (i + n - 1) % n } else { (i + 1) % n });
            }
            Kind::Key { .. } => {}
        }
    }
    pub fn key_name(self) -> Option<&'static str> {
        self.spec().key_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let d = Belumi::default();
        assert_eq!(Belumi::parse(&d.serialize()), d);
        assert_eq!(Belumi::parse(""), d);
    }

    #[test]
    fn broken_input_falls_back_and_levels_clamp() {
        assert_eq!(Belumi::parse("hud = [1"), Belumi::default());
        let b = Belumi::parse("zoom_level = 99\nminimap_size = 77\nminimap_zoom = 9");
        assert_eq!(b.zoom_level, ZOOM_LEVELS.len() as u8 - 1);
        assert_eq!(b.minimap_size, MINIMAP_SIZES.len() as u8 - 1);
        assert_eq!(b.minimap_zoom, MINIMAP_ZOOMS.len() as u8 - 1);
    }

    #[test]
    fn every_item_has_exactly_one_spec_and_text() {
        for item in rows().into_iter().flatten() {
            assert_eq!(SPECS.iter().filter(|s| s.item == item).count(), 1);
            assert!(!item.caption().is_empty() && !item.description().is_empty());
        }
        assert_eq!(rows().into_iter().flatten().count(), SPECS.len());
    }

    #[test]
    fn items_cycle_and_dependents_follow_their_switch() {
        let mut b = Belumi::default();
        Item::Hud.cycle(&mut b, false);
        assert_eq!(Item::Hud.switch(&b), Some(true));
        Item::HudCorner.cycle(&mut b, false);
        assert_eq!(b.corner(), Corner::TopRight);
        Item::HudCorner.cycle(&mut b, true);
        assert_eq!(b.corner(), Corner::TopLeft);
        Item::ZoomLevel.cycle(&mut b, true);
        assert_eq!(b.zoom_level, 1);
        Item::MinimapZoom.cycle(&mut b, false);
        assert_eq!(b.minimap_blocks_per_pixel(), 2.0);
        assert_eq!(Item::HudPing.parent(), Some(Item::Hud));
        assert_eq!(Item::HudCorner.value(&b), "Top left");
    }

    #[test]
    fn keys_bind_by_name_and_other_names_are_refused() {
        let mut b = Belumi::default();
        assert!(b.set_key(WORLDMAP_KEY, "key.keyboard.n".into()));
        assert_eq!(b.worldmap_key, "key.keyboard.n");
        assert!(b.set_key(WAYPOINT_KEY, "key.keyboard.v".into()));
        assert!(b.set_key(FREECAM_KEY, "key.keyboard.g".into()));
        assert!(!b.set_key("key.jump", "key.keyboard.x".into()));
        assert_eq!(Item::ZoomKey.key_name(), Some(ZOOM_KEY));
        assert_eq!(Item::Hud.key_name(), None);
    }
}
