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
pub const VOICE_KEY: &str = "belumi.voice";
pub const VOICE_MUTE_KEY: &str = "belumi.voice_mute";
pub const VOICE_DEAFEN_KEY: &str = "belumi.voice_deafen";
pub const VOICE_THRESHOLDS: [f32; 4] = [-60.0, -50.0, -40.0, -30.0];
pub const VOICE_MIC_GAINS: [f32; 4] = [0.5, 1.0, 1.5, 2.0];
pub const VOICE_VOLUMES: [f32; 4] = [0.5, 1.0, 1.5, 2.0];
pub const FX_VIGNETTE: [f32; 4] = [0.0, 0.25, 0.45, 0.7];
pub const FX_SATURATION: [f32; 5] = [0.7, 0.85, 1.0, 1.2, 1.4];
pub const FX_CONTRAST: [f32; 5] = [0.9, 1.0, 1.1, 1.2, 1.3];
pub const FX_WARMTH: [f32; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];
pub const FX_SHARPEN: [f32; 4] = [0.0, 0.25, 0.5, 0.9];
pub const FX_GRAIN: [f32; 4] = [0.0, 0.03, 0.06, 0.1];
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
    pub map_entities: bool,
    pub worldmap: bool,
    pub worldmap_key: String,
    pub effects: bool,
    pub fx_vignette: u8,
    pub fx_saturation: u8,
    pub fx_contrast: u8,
    pub fx_warmth: u8,
    pub fx_sharpen: u8,
    pub fx_grain: u8,
    pub voice: bool,
    pub voice_mode: u8,
    pub voice_key: String,
    pub voice_mute_key: String,
    pub voice_deafen_key: String,
    pub voice_threshold: u8,
    pub voice_mic: u8,
    pub voice_volume: u8,
    pub voice_device: String,
    pub quality: u8,
    pub plugins: bool,
    pub plugin_corner: String,
    pub freecam: bool,
    pub freecam_key: String,
    pub freecam_speed: u8,
    pub voice_player_volumes: std::collections::BTreeMap<String, u16>,
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
            map_entities: true,
            worldmap: false,
            worldmap_key: "key.keyboard.m".into(),
            effects: false,
            fx_vignette: 0,
            fx_saturation: 2,
            fx_contrast: 1,
            fx_warmth: 2,
            fx_sharpen: 0,
            fx_grain: 0,
            voice: false,
            voice_mode: 0,
            voice_key: "key.keyboard.v".into(),
            voice_mute_key: "key.keyboard.k".into(),
            voice_deafen_key: "key.keyboard.j".into(),
            voice_threshold: 1,
            voice_mic: 1,
            voice_volume: 1,
            voice_device: String::new(),
            quality: 0,
            plugins: false,
            plugin_corner: Corner::BottomRight.name().into(),
            freecam: false,
            freecam_key: "key.keyboard.f9".into(),
            freecam_speed: 1,
            voice_player_volumes: Default::default(),
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
        b.fx_vignette = b.fx_vignette.min(FX_VIGNETTE.len() as u8 - 1);
        b.fx_saturation = b.fx_saturation.min(FX_SATURATION.len() as u8 - 1);
        b.fx_contrast = b.fx_contrast.min(FX_CONTRAST.len() as u8 - 1);
        b.fx_warmth = b.fx_warmth.min(FX_WARMTH.len() as u8 - 1);
        b.fx_sharpen = b.fx_sharpen.min(FX_SHARPEN.len() as u8 - 1);
        b.fx_grain = b.fx_grain.min(FX_GRAIN.len() as u8 - 1);
        b.voice_mode = b.voice_mode.min(1);
        b.quality = b.quality.min(QUALITY_LABELS.len() as u8 - 1);
        b.voice_threshold = b.voice_threshold.min(VOICE_THRESHOLDS.len() as u8 - 1);
        b.voice_mic = b.voice_mic.min(VOICE_MIC_GAINS.len() as u8 - 1);
        b.voice_volume = b.voice_volume.min(VOICE_VOLUMES.len() as u8 - 1);
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
    pub fn fx_values(&self) -> [f32; 6] {
        let at = |t: &[f32], i: u8| t[usize::from(i).min(t.len() - 1)];
        [
            at(&FX_CONTRAST, self.fx_contrast),
            at(&FX_SATURATION, self.fx_saturation),
            at(&FX_VIGNETTE, self.fx_vignette),
            at(&FX_SHARPEN, self.fx_sharpen),
            at(&FX_GRAIN, self.fx_grain),
            at(&FX_WARMTH, self.fx_warmth),
        ]
    }
    pub fn voice_threshold_db(&self) -> f32 {
        VOICE_THRESHOLDS[usize::from(self.voice_threshold).min(VOICE_THRESHOLDS.len() - 1)]
    }
    pub fn voice_mic_gain(&self) -> f32 {
        VOICE_MIC_GAINS[usize::from(self.voice_mic).min(VOICE_MIC_GAINS.len() - 1)]
    }
    pub fn player_gain(&self, uuid: &str) -> f32 {
        self.voice_player_volumes
            .get(uuid)
            .map_or(1.0, |p| f32::from(*p) / 100.0)
    }
    pub fn voice_volume_gain(&self) -> f32 {
        VOICE_VOLUMES[usize::from(self.voice_volume).min(VOICE_VOLUMES.len() - 1)]
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
    MapEntities,
    Worldmap,
    WorldmapKey,
    Effects,
    FxVignette,
    FxSaturation,
    FxContrast,
    FxWarmth,
    FxSharpen,
    FxGrain,
    Voice,
    VoiceMode,
    VoiceKey,
    VoiceMuteKey,
    VoiceDeafenKey,
    VoiceThreshold,
    VoiceMic,
    VoiceVolume,
    VoiceDevice,
    Quality,
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
    Device {
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
const FX_VIGNETTE_LABELS: &[&str] = &["Off", "Light", "Medium", "Strong"];
const FX_SATURATION_LABELS: &[&str] = &["70%", "85%", "100%", "120%", "140%"];
const FX_CONTRAST_LABELS: &[&str] = &["90%", "100%", "110%", "120%", "130%"];
const FX_WARMTH_LABELS: &[&str] = &["Cold", "Cool", "Neutral", "Warm", "Hot"];
const FX_SHARPEN_LABELS: &[&str] = &["Off", "Low", "Medium", "High"];
const FX_GRAIN_LABELS: &[&str] = &["Off", "Low", "Medium", "High"];
pub const QUALITY_LABELS: &[&str] = &["Default", "High", "Medium", "Low", "Minimal", "Shaders off"];
const VOICE_MODES: &[&str] = &["Push to talk", "Voice activation"];
const VOICE_THRESHOLD_LABELS: &[&str] = &["-60 dB (sensitive)", "-50 dB", "-40 dB", "-30 dB (loud only)"];
const VOICE_GAIN_LABELS: &[&str] = &["50%", "100%", "150%", "200%"];
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

static INPUT_DEVICES: std::sync::OnceLock<fn() -> Vec<String>> = std::sync::OnceLock::new();

/// Where the list of microphones comes from (set once by the game when it has voice chat).
pub fn set_input_device_lister(lister: fn() -> Vec<String>) {
    let _ = INPUT_DEVICES.set(lister);
}

fn input_devices() -> Vec<String> {
    INPUT_DEVICES.get().map_or_else(Vec::new, |f| f())
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
    switch!(MapEntities, "Entities on maps", "Shows players, hostile mobs and animals as coloured dots on the minimap and the world map.", Some(Item::Minimap), map_entities),
    switch!(Worldmap, "World map", "Remembers explored terrain and shows it in a full-screen overlay.", None, worldmap),
    key!(WorldmapKey, "World map key", Some(Item::Worldmap), WORLDMAP_KEY, worldmap_key),
    switch!(Effects, "Screen effects", "Colour grading, sharpening, vignette and film grain, applied in the pass that already draws the screen (about 0.1 ms).", None, effects),
    choice!(FxVignette, "Vignette", "Darkens the screen edges.", Some(Item::Effects), FX_VIGNETTE_LABELS, fx_vignette),
    choice!(FxSaturation, "Saturation", "How vivid colours are.", Some(Item::Effects), FX_SATURATION_LABELS, fx_saturation),
    choice!(FxContrast, "Contrast", "Separates light from dark.", Some(Item::Effects), FX_CONTRAST_LABELS, fx_contrast),
    choice!(FxWarmth, "Colour temperature", "Shifts the image toward blue or orange.", Some(Item::Effects), FX_WARMTH_LABELS, fx_warmth),
    choice!(FxSharpen, "Sharpen", "Brings out texture detail.", Some(Item::Effects), FX_SHARPEN_LABELS, fx_sharpen),
    choice!(FxGrain, "Film grain", "Fine moving noise.", Some(Item::Effects), FX_GRAIN_LABELS, fx_grain),
    switch!(Voice, "Voice chat", "Talks to other players on servers with the Simple Voice Chat mod (needs a build with voice support).", None, voice),
    choice!(Quality, "Graphics quality", "Steps the shader pack down through its own quality profiles (High, Medium, Low), then Minimal, then turns shaders off. Default leaves the pack as it is.", None, QUALITY_LABELS, quality),
    choice!(VoiceMode, "Microphone mode", "Push to talk sends only while the key is held; voice activation opens on sound.", Some(Item::Voice), VOICE_MODES, voice_mode),
    key!(VoiceKey, "Push-to-talk key", Some(Item::Voice), VOICE_KEY, voice_key),
    key!(VoiceMuteKey, "Mute key", Some(Item::Voice), VOICE_MUTE_KEY, voice_mute_key),
    key!(VoiceDeafenKey, "Deafen key", Some(Item::Voice), VOICE_DEAFEN_KEY, voice_deafen_key),
    choice!(VoiceThreshold, "Activation level", "How loud you must be to open the microphone in voice activation mode.", Some(Item::Voice), VOICE_THRESHOLD_LABELS, voice_threshold),
    choice!(VoiceMic, "Microphone volume", "Boosts or lowers what you send.", Some(Item::Voice), VOICE_GAIN_LABELS, voice_mic),
    choice!(VoiceVolume, "Voice volume", "How loud other players sound.", Some(Item::Voice), VOICE_GAIN_LABELS, voice_volume),
    Spec {
        item: Item::VoiceDevice,
        caption: "Microphone",
        description: "Click to switch to the next microphone; the system default follows your desktop's choice.",
        parent: Some(Item::Voice),
        kind: Kind::Device {
            get: |b| &b.voice_device,
            set: |b, v| b.voice_device = v,
        },
    },
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
        ("Graphics quality", &[Quality]),
        ("Heads-up display", &[Hud, HudCorner, HudCoords, HudFps, HudPing, HudDirection, HudSpeed]),
        ("Zoom", &[Zoom, ZoomKey, ZoomLevel, ZoomSmooth, ZoomScroll]),
        ("Maps", &[Minimap, MinimapCorner, MinimapSize, MinimapZoom, MinimapRotate, MinimapRound, MapEntities, Worldmap, WorldmapKey]),
        ("Waypoints", &[Waypoints, WaypointKey, WaypointList, WaypointLabels]),
        ("Screen effects", &[Effects, FxVignette, FxSaturation, FxContrast, FxWarmth, FxSharpen, FxGrain]),
        ("Voice chat", &[Voice, VoiceMode, VoiceKey, VoiceMuteKey, VoiceDeafenKey, VoiceThreshold, VoiceMic, VoiceVolume, VoiceDevice]),
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
            Kind::Device { get, .. } => {
                let name = get(b);
                if name.is_empty() {
                    "System default".into()
                } else if name.chars().count() > 28 {
                    format!("{}...", name.chars().take(26).collect::<String>())
                } else {
                    name.into()
                }
            }
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
            Kind::Device { get, set } => {
                let mut options = vec![String::new()];
                options.extend(input_devices());
                let n = options.len();
                let at = options.iter().position(|o| o == get(b)).unwrap_or(0);
                set(b, options[if back { (at + n - 1) % n } else { (at + 1) % n }].clone());
            }
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
    fn the_microphone_item_steps_through_the_default_and_the_listed_devices() {
        fn two() -> Vec<String> {
            vec!["Headset".into(), "Webcam".into()]
        }
        set_input_device_lister(two);
        let mut b = Belumi::default();
        assert_eq!(Item::VoiceDevice.value(&b), "System default");
        Item::VoiceDevice.cycle(&mut b, false);
        assert_eq!(b.voice_device, "Headset");
        Item::VoiceDevice.cycle(&mut b, false);
        assert_eq!(b.voice_device, "Webcam");
        Item::VoiceDevice.cycle(&mut b, false);
        assert_eq!(b.voice_device, "");
        Item::VoiceDevice.cycle(&mut b, true);
        assert_eq!(b.voice_device, "Webcam");
        assert_eq!(Belumi::parse(&b.serialize()).voice_device, "Webcam");
    }

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
        assert!(b.set_key(VOICE_KEY, "key.keyboard.b".into()));
        assert!(b.set_key(VOICE_MUTE_KEY, "key.keyboard.j".into()));
        assert!(!b.set_key("key.jump", "key.keyboard.x".into()));
        assert_eq!(Item::ZoomKey.key_name(), Some(ZOOM_KEY));
        assert_eq!(Item::Hud.key_name(), None);
    }
}
