//! The client's modules: where the HUD modules sit (the HUD builder's layout), and the settings of the
//! modules that have no place on the HUD (crosshair, chat tools, freelook, replay). One file,
//! `belumi-modules.toml`, next to `belumi.toml`.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

pub const FILE: &str = "belumi-modules.toml";

/// The HUD modules, in the order the builder lists them.
pub const HUD_IDS: [&str; 12] = ["keystrokes", "cps", "armor", "effects", "clock", "rec", "inventory", "fps", "ping", "coords", "compass", "profiler"];

pub fn hud_title(id: &str) -> &'static str {
    match id {
        "keystrokes" => "Keystrokes",
        "cps" => "CPS",
        "armor" => "Armor and items",
        "effects" => "Effects",
        "clock" => "Clock",
        "rec" => "Recording",
        "inventory" => "Inventory",
        "fps" => "FPS",
        "ping" => "Ping",
        "coords" => "Coordinates",
        "compass" => "Compass",
        "profiler" => "Profiler",
        _ => "Module",
    }
}

/// A short name for a button that has little room.
pub fn hud_short(id: &str) -> &'static str {
    match id {
        "keystrokes" => "Keys",
        "armor" => "Armor",
        "effects" => "Effects",
        "rec" => "Rec",
        "inventory" => "Inventory",
        "coords" => "Coords",
        other => hud_title(other),
    }
}

/// One HUD module on the screen. `nx` and `ny` run from 0 (the left or top edge) to 1 (the right or
/// bottom edge) over the room the module has, so a layout survives another window size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub id: String,
    pub enabled: bool,
    pub nx: f32,
    pub ny: f32,
    pub scale: f32,
    /// The module's own switches (for example `mouse` for the keystrokes' buttons).
    #[serde(default)]
    pub opt: BTreeMap<String, bool>,
    /// How solid the module's background is, in percent (0 shows the text alone).
    #[serde(default = "full")]
    pub bg: u8,
}

fn full() -> u8 {
    100
}

impl Placed {
    pub fn flag(&self, name: &str, default: bool) -> bool {
        self.opt.get(name).copied().unwrap_or(default)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crosshair {
    pub enabled: bool,
    /// 0 cross, 1 dot, 2 ring, 3 square, 4 chevron.
    pub style: u8,
    pub color: u32,
    pub opacity: u8,
    pub size: u8,
    pub thickness: u8,
    pub gap: u8,
    pub outline: bool,
    pub dot: bool,
    /// It spreads while you move, as a shooter's crosshair does.
    pub dynamic: bool,
}

impl Default for Crosshair {
    fn default() -> Self {
        Self {
            enabled: false,
            style: 0,
            color: 0x00FF_FFFF,
            opacity: 220,
            size: 5,
            thickness: 1,
            gap: 2,
            outline: true,
            dot: false,
            dynamic: false,
        }
    }
}

/// Ready-made crosshairs: name and how to make it from the player's own (on/off is kept).
pub const CROSSHAIR_PRESETS: [&str; 5] = ["Classic", "Dot", "Ring", "Tight", "Chevron"];

impl Crosshair {
    pub fn apply_preset(&mut self, index: usize) {
        let on = self.enabled;
        *self = Crosshair::default();
        self.enabled = on;
        match index {
            0 => {}
            1 => {
                self.style = 1;
                self.size = 2;
                self.color = 0x4DFF6A;
            }
            2 => {
                self.style = 2;
                self.size = 6;
                self.color = 0x35E0D0;
            }
            3 => {
                self.size = 4;
                self.gap = 0;
                self.color = 0xFFE14D;
            }
            _ => {
                self.style = 4;
                self.size = 6;
                self.thickness = 2;
                self.color = 0xFF3B3B;
            }
        }
    }
}

pub const CROSSHAIR_STYLES: [&str; 5] = ["Cross", "Dot", "Ring", "Square", "Chevron"];
pub const CROSSHAIR_COLORS: [u32; 10] = [
    0xFFFFFF, 0xFF3B3B, 0xFF9A2E, 0xFFE14D, 0x4DFF6A, 0x35E0D0, 0x4DA3FF, 0x9B5CFF, 0xFF6AD5, 0x101010,
];

/// A filled rectangle of the crosshair, relative to its centre, with the 0xAARRGGBB colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: u32,
}

impl Crosshair {
    pub fn argb(&self) -> u32 {
        (u32::from(self.opacity) << 24) | (self.color & 0x00FF_FFFF)
    }

    /// The crosshair as rectangles around (0, 0); `spread` widens the gap (a moving player's).
    pub fn pieces(&self, spread: f32) -> Vec<Piece> {
        let c = self.argb();
        let t = f32::from(self.thickness.max(1));
        let len = f32::from(self.size.max(1));
        let gap = f32::from(self.gap) + spread;
        // The centre block spans [a, b) in whole pixels, so rounding never favours one side.
        let a = -(t / 2.0).floor();
        let b = a + t;
        let mut shapes: Vec<Piece> = Vec::new();
        let mut rect = |x: f32, y: f32, w: f32, h: f32| shapes.push(Piece { x, y, w, h, color: c });
        match self.style {
            0 => {
                rect(a, a - gap - len, t, len);
                rect(a, b + gap, t, len);
                rect(a - gap - len, a, len, t);
                rect(b + gap, a, len, t);
            }
            1 => {
                let d = (t * 2.0).max(2.0);
                rect(-d / 2.0, -d / 2.0, d, d);
            }
            2 => {
                let r = len + gap;
                let steps = 24;
                for i in 0..steps {
                    let angle = i as f32 / steps as f32 * std::f32::consts::TAU;
                    rect(angle.cos() * r + a, angle.sin() * r + a, t, t);
                }
            }
            3 => {
                let r = len + gap;
                rect(-r, -r, 2.0 * r, t);
                rect(-r, r - t, 2.0 * r, t);
                rect(-r, -r, t, 2.0 * r);
                rect(r - t, -r, t, 2.0 * r);
            }
            _ => {
                let r = len;
                for i in 0..=(r as i32) {
                    let i = i as f32;
                    rect(a - i, a - r + i + gap, t, t);
                    rect(a + i, a - r + i + gap, t, t);
                }
            }
        }
        if self.dot && self.style != 1 {
            rect(a, a, t, t);
        }
        if self.outline {
            let mut outlined = Vec::with_capacity(shapes.len() * 2);
            for p in &shapes {
                outlined.push(Piece { x: p.x - 1.0, y: p.y - 1.0, w: p.w + 2.0, h: p.h + 2.0, color: (u32::from(self.opacity) << 24) | 0x0010_1010 });
            }
            outlined.extend(shapes);
            return outlined;
        }
        shapes
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatTools {
    pub timestamps: bool,
    pub seconds: bool,
    pub merge_spam: bool,
    pub ping_sound: bool,
    /// Lines with any of these words are hidden.
    pub hide: Vec<String>,
    /// Lines with any of these words are coloured.
    pub highlight: Vec<String>,
    /// Chat lines kept (vanilla keeps 100).
    pub history: u32,
}

impl Default for ChatTools {
    fn default() -> Self {
        Self { timestamps: false, seconds: false, merge_spam: false, ping_sound: false, hide: Vec::new(), highlight: Vec::new(), history: 100 }
    }
}

/// What Discord Rich Presence says about the game (docs/discord-presence.md). The address and the
/// world name are off until the player turns them on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordOptions {
    /// "Playing on <address>" while on a server.
    pub show_server: bool,
    /// The singleplayer world's name.
    pub show_world: bool,
    pub show_dimension: bool,
    /// The game mode as the small icon.
    pub show_mode: bool,
    /// How long you have been in the world.
    pub show_time: bool,
    /// How many players are on the server or in your open world.
    pub show_players: bool,
}

impl Default for DiscordOptions {
    fn default() -> Self {
        Self { show_server: false, show_world: false, show_dimension: true, show_mode: true, show_time: true, show_players: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Misc {
    pub freelook: bool,
    pub freelook_key: String,
    pub tooltips: bool,
    /// A shulker box's tooltip shows its contents as a grid.
    pub container_preview: bool,
    /// Columns a server sent stay drawn after it forgot them and are cached for the next visit.
    pub distant_chunks: bool,
    /// Breadcrumbs behind the player (the way back), drawn in the world.
    pub trail: bool,
    /// Compare the columns a server sends with the cached ones and report what changed since the last visit.
    pub changes: bool,
    /// Remember what the containers you open hold (`/chests TEXT`).
    pub chest_memory: bool,
    /// Write down every fight: swings, hits, damage taken, clicks per second.
    pub combat: bool,
    /// The world's sounds ring with the room you are in (reverb from the size of the room).
    pub sound_physics: bool,
    /// Keep older versions of the chunks a world saves over (`/rollback`).
    pub world_history: bool,
    /// The signal strength written over redstone dust and its relatives.
    pub redstone_numbers: bool,
    /// Pings and shared waypoints between Belumi players on a server.
    pub pings: bool,
    /// Accurate block placement: no wait between blocks placed with the right button held, none between blocks broken in
    /// creative (`accurate_break`).
    pub accurate_placement: bool,
    pub accurate_break: bool,
    pub accurate_key: String,
    pub ping_key: String,
    /// What the compass module points at: a waypoint's name, `back` for where the trail began, empty for the nearest.
    pub compass_target: String,
    pub low_durability_warning: bool,
}

impl Default for Misc {
    fn default() -> Self {
        Self {
            freelook: true,
            freelook_key: "key.keyboard.left.alt".into(),
            tooltips: false,
            container_preview: true,
            distant_chunks: true,
            trail: false,
            changes: true,
            chest_memory: true,
            combat: true,
            sound_physics: false,
            world_history: true,
            redstone_numbers: false,
            pings: true,
            accurate_placement: false,
            accurate_break: true,
            accurate_key: String::new(),
            ping_key: "key.keyboard.z".into(),
            compass_target: String::new(),
            low_durability_warning: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Replay {
    pub record: bool,
    pub clip_seconds: u16,
    pub clip_key: String,
    pub mark_key: String,
    pub keep: u16,
    /// A death saves a clip of the last moments at once (with the killer, for the killer camera).
    pub auto_death_clip: bool,
}

impl Default for Replay {
    fn default() -> Self {
        Self { record: false, clip_seconds: 30, clip_key: "key.keyboard.f10".into(), mark_key: "key.keyboard.f6".into(), keep: 20, auto_death_clip: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Modules {
    pub hud: Vec<Placed>,
    pub crosshair: Crosshair,
    pub chat: ChatTools,
    pub misc: Misc,
    pub replay: Replay,
    pub discord: DiscordOptions,
    pub menu_key: String,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            hud: HUD_IDS.iter().map(|id| default_placed(id)).collect(),
            crosshair: Crosshair::default(),
            chat: ChatTools::default(),
            misc: Misc::default(),
            replay: Replay::default(),
            discord: DiscordOptions::default(),
            menu_key: "key.keyboard.right.shift".into(),
        }
    }
}

fn default_placed(id: &str) -> Placed {
    let (nx, ny) = match id {
        "keystrokes" => (0.02, 0.62),
        "cps" => (0.02, 0.5),
        "armor" => (0.98, 0.45),
        "effects" => (0.02, 0.2),
        "rec" => (0.5, 0.02),
        "inventory" => (0.98, 0.95),
        "fps" => (0.02, 0.02),
        "ping" => (0.02, 0.08),
        "coords" => (0.02, 0.14),
        "compass" => (0.5, 0.08),
        "profiler" => (0.75, 0.3),
        _ => (0.98, 0.02),
    };
    Placed { id: id.into(), enabled: false, nx, ny, scale: 1.0, opt: BTreeMap::new(), bg: 100 }
}

impl Modules {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut m) => {
                m.repair();
                m
            }
            Err(_) => {
                let _ = fs::rename(&path, path.with_extension("toml.bak"));
                Self::default()
            }
        }
    }

    pub fn save(&self, dir: &Path) {
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = fs::write(dir.join(FILE), text);
        }
    }

    /// Every module present once, with sane numbers, whatever the file said.
    pub fn repair(&mut self) {
        let mut seen = Vec::new();
        self.hud.retain(|m| HUD_IDS.contains(&m.id.as_str()) && !seen.contains(&m.id) && {
            seen.push(m.id.clone());
            true
        });
        for id in HUD_IDS {
            if !self.hud.iter().any(|m| m.id == id) {
                self.hud.push(default_placed(id));
            }
        }
        for m in &mut self.hud {
            m.bg = m.bg.min(100);
            m.nx = if m.nx.is_finite() { m.nx.clamp(0.0, 1.0) } else { 0.0 };
            m.ny = if m.ny.is_finite() { m.ny.clamp(0.0, 1.0) } else { 0.0 };
            m.scale = if m.scale.is_finite() { m.scale.clamp(0.5, 3.0) } else { 1.0 };
        }
        self.chat.history = self.chat.history.clamp(100, 2000);
        self.crosshair.size = self.crosshair.size.clamp(1, 30);
        self.crosshair.thickness = self.crosshair.thickness.clamp(1, 8);
        self.crosshair.gap = self.crosshair.gap.clamp(0, 20);
        self.crosshair.style = self.crosshair.style.min(CROSSHAIR_STYLES.len() as u8 - 1);
        self.replay.clip_seconds = self.replay.clip_seconds.clamp(5, 300);
        self.replay.keep = self.replay.keep.clamp(1, 200);
    }

    pub fn hud_mut(&mut self, id: &str) -> Option<&mut Placed> {
        self.hud.iter_mut().find(|m| m.id == id)
    }

    pub fn hud_ref(&self, id: &str) -> Option<&Placed> {
        self.hud.iter().find(|m| m.id == id)
    }

    /// A first layout to start from.
    pub fn starter(&mut self) {
        for m in &mut self.hud {
            *m = default_placed(&m.id);
            m.enabled = matches!(m.id.as_str(), "keystrokes" | "cps" | "armor" | "effects");
        }
    }
}

/// A module's top left corner in pixels from its `nx`, `ny`.
pub fn position(p: &Placed, screen: (f32, f32), size: (f32, f32)) -> (f32, f32) {
    (p.nx * (screen.0 - size.0).max(0.0), p.ny * (screen.1 - size.1).max(0.0))
}

/// The `nx`, `ny` a top left corner stands for.
pub fn fractions(pos: (f32, f32), screen: (f32, f32), size: (f32, f32)) -> (f32, f32) {
    let room = ((screen.0 - size.0).max(1.0), (screen.1 - size.1).max(1.0));
    ((pos.0 / room.0).clamp(0.0, 1.0), (pos.1 / room.1).clamp(0.0, 1.0))
}

/// Where a dragged module (`rect`: x, y, w, h) lands: pulled onto the screen's edges and centre lines and
/// onto the edges of the other modules within `reach` pixels. Also the guide lines it snapped to.
pub fn snap(rect: [f32; 4], screen: (f32, f32), others: &[[f32; 4]], reach: f32) -> ([f32; 2], Vec<(bool, f32)>) {
    let [x, y, w, h] = rect;
    let mut xs = vec![0.0, screen.0 / 2.0, screen.0];
    let mut ys = vec![0.0, screen.1 / 2.0, screen.1];
    for o in others {
        xs.extend([o[0], o[0] + o[2], o[0] + o[2] / 2.0]);
        ys.extend([o[1], o[1] + o[3], o[1] + o[3] / 2.0]);
    }
    let pull = |start: f32, size: f32, lines: &[f32]| -> Option<(f32, f32)> {
        let mut best: Option<(f32, f32)> = None;
        for &line in lines {
            for (edge, offset) in [(start, 0.0), (start + size, size), (start + size / 2.0, size / 2.0)] {
                let d = line - edge;
                if d.abs() <= reach && best.is_none_or(|(b, _)| d.abs() < b.abs()) {
                    best = Some((d, line));
                }
                let _ = offset;
            }
        }
        best
    };
    let mut guides = Vec::new();
    let mut at = [x, y];
    if let Some((d, line)) = pull(x, w, &xs) {
        at[0] += d;
        guides.push((true, line));
    }
    if let Some((d, line)) = pull(y, h, &ys) {
        at[1] += d;
        guides.push((false, line));
    }
    at[0] = at[0].clamp(0.0, (screen.0 - w).max(0.0));
    at[1] = at[1].clamp(0.0, (screen.1 - h).max(0.0));
    (at, guides)
}

/// Clicks per second over the last second.
#[derive(Clone, Debug, Default)]
pub struct Clicks {
    times: std::collections::VecDeque<f64>,
}

impl Clicks {
    pub fn click(&mut self, now: f64) {
        self.times.push_back(now);
        self.trim(now);
    }

    fn trim(&mut self, now: f64) {
        while self.times.front().is_some_and(|t| now - t > 1.0) {
            self.times.pop_front();
        }
    }

    pub fn per_second(&mut self, now: f64) -> usize {
        self.trim(now);
        self.times.len()
    }
}

/// The time of day as `HH:MM` or `HH:MM:SS` from seconds since midnight.
pub fn clock_text(seconds_of_day: u32, seconds: bool) -> String {
    let (h, m, s) = (seconds_of_day / 3600 % 24, seconds_of_day / 60 % 60, seconds_of_day % 60);
    if seconds { format!("{h:02}:{m:02}:{s:02}") } else { format!("{h:02}:{m:02}") }
}

/// Whether a chat line is hidden by the filter, and which colour (0xRRGGBB) a highlight gives it.
pub fn judge_line(line: &str, tools: &ChatTools) -> (bool, Option<u32>) {
    let lower = line.to_lowercase();
    let has = |words: &[String]| words.iter().any(|w| !w.trim().is_empty() && lower.contains(&w.trim().to_lowercase()));
    (has(&tools.hide), has(&tools.highlight).then_some(0x00FF_E14D))
}

/// Whether `line` names `me` as a word (for the ping sound).
pub fn mentions(line: &str, me: &str) -> bool {
    if me.is_empty() {
        return false;
    }
    let (line, me) = (line.to_lowercase(), me.to_lowercase());
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(i) = line[from..].find(&me) {
        let at = from + i;
        let end = at + me.len();
        let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let before = at == 0 || !word(bytes[at - 1]);
        let after = end >= bytes.len() || !word(bytes[end]);
        if before && after {
            return true;
        }
        from = at + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_survives_a_file_with_missing_and_broken_modules() {
        let mut m = Modules::default();
        m.hud.retain(|p| p.id != "cps");
        m.hud.push(Placed { id: "nonsense".into(), enabled: true, nx: f32::NAN, ny: 9.0, scale: 0.0, opt: BTreeMap::new(), bg: 100 });
        m.hud[0].nx = 7.0;
        m.repair();
        assert_eq!(m.hud.len(), HUD_IDS.len());
        assert!(m.hud_ref("cps").is_some() && m.hud_ref("nonsense").is_none());
        assert_eq!(m.hud[0].nx, 1.0);
    }

    #[test]
    fn the_background_defaults_to_solid_and_is_capped() {
        let old = "[[hud]]\nid = \"cps\"\nenabled = true\nnx = 0.5\nny = 0.5\nscale = 1.0\n";
        let mut m: Modules = toml::from_str(old).unwrap();
        assert_eq!(m.hud[0].bg, 100);
        m.hud[0].bg = 250;
        m.repair();
        assert_eq!(m.hud_ref("cps").unwrap().bg, 100);
    }

    #[test]
    fn the_cross_has_the_same_gap_and_length_on_every_side() {
        for thickness in 1..=4u8 {
            for gap in 0..=5u8 {
                let c = Crosshair { enabled: true, thickness, gap, outline: false, dot: false, ..Crosshair::default() };
                let p = c.pieces(0.0);
                let (top, bottom, left, right) = (p[0], p[1], p[2], p[3]);
                let t = f32::from(thickness);
                let a = -(t / 2.0).floor();
                let b = a + t;
                let g = f32::from(gap);
                assert_eq!(a - (top.y + top.h), g, "top, thickness {thickness}");
                assert_eq!(bottom.y - b, g, "bottom, thickness {thickness}");
                assert_eq!(a - (left.x + left.w), g, "left, thickness {thickness}");
                assert_eq!(right.x - b, g, "right, thickness {thickness}");
                assert!(top.h == bottom.h && left.w == right.w && top.h == left.w);
            }
        }
    }

    #[test]
    fn presets_keep_the_switch_and_stay_in_range() {
        for i in 0..CROSSHAIR_PRESETS.len() {
            let mut c = Crosshair { enabled: true, ..Crosshair::default() };
            c.apply_preset(i);
            assert!(c.enabled);
            assert!(usize::from(c.style) < CROSSHAIR_STYLES.len());
            assert!(!c.pieces(0.0).is_empty());
        }
    }

    #[test]
    fn the_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("belumi-modules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = Modules::default();
        m.starter();
        m.crosshair.enabled = true;
        m.chat.hide = vec!["spam".into()];
        m.save(&dir);
        assert_eq!(Modules::load(&dir), m);
        std::fs::write(dir.join(FILE), "not toml {{").unwrap();
        assert_eq!(Modules::load(&dir), Modules::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn positions_and_fractions_agree() {
        let p = Placed { id: "cps".into(), enabled: true, nx: 0.25, ny: 1.0, scale: 1.0, opt: BTreeMap::new(), bg: 100 };
        let at = position(&p, (400.0, 240.0), (100.0, 40.0));
        assert_eq!(at, (75.0, 200.0));
        assert_eq!(fractions(at, (400.0, 240.0), (100.0, 40.0)), (0.25, 1.0));
    }

    #[test]
    fn dragging_snaps_to_edges_centre_and_neighbours() {
        let screen = (400.0, 240.0);
        let (at, guides) = snap([3.0, 100.0, 50.0, 20.0], screen, &[], 6.0);
        assert_eq!(at[0], 0.0);
        assert!(guides.contains(&(true, 0.0)));
        let (at, _) = snap([176.0, 100.0, 50.0, 20.0], screen, &[], 6.0);
        assert_eq!(at[0], 175.0);
        let (at, _) = snap([100.0, 60.0, 50.0, 20.0], screen, &[[100.0, 82.0, 50.0, 20.0]], 6.0);
        assert!(at[1] > 60.0 && (at[1] + 20.0 - 82.0).abs() < 0.01);
        let (at, guides) = snap([100.0, 90.0, 50.0, 20.0], screen, &[], 2.0);
        assert_eq!((at, guides.len()), ([100.0, 90.0], 0));
    }

    #[test]
    fn crosshairs_have_pieces_and_an_outline_behind_them() {
        let mut c = Crosshair::default();
        c.outline = false;
        assert_eq!(c.pieces(0.0).len(), 4);
        c.dot = true;
        assert_eq!(c.pieces(0.0).len(), 5);
        c.outline = true;
        assert_eq!(c.pieces(0.0).len(), 10);
        c.style = 1;
        assert_eq!(c.pieces(0.0).len(), 2);
        assert_eq!(c.argb() >> 24, 220);
        for style in 0..CROSSHAIR_STYLES.len() as u8 {
            c.style = style;
            assert!(!c.pieces(3.0).is_empty());
        }
    }

    #[test]
    fn clicks_count_only_the_last_second() {
        let mut c = Clicks::default();
        for t in [0.0, 0.2, 0.4, 0.9] {
            c.click(t);
        }
        assert_eq!(c.per_second(0.95), 4);
        assert_eq!(c.per_second(1.3), 2);
        assert_eq!(c.per_second(5.0), 0);
    }

    #[test]
    fn chat_judging_and_mentions() {
        let tools = ChatTools { hide: vec!["spam".into(), " ".into()], highlight: vec!["Base".into()], ..ChatTools::default() };
        assert_eq!(judge_line("buy SPAM now", &tools), (true, None));
        assert_eq!(judge_line("my base is here", &tools), (false, Some(0x00FF_E14D)));
        assert_eq!(judge_line("hello", &tools), (false, None));
        assert!(mentions("<Alex> hi mcexample!", "McExample"));
        assert!(!mentions("<Alex> hi mcexampleX", "McExample"));
        assert!(!mentions("anything", ""));
        assert_eq!(clock_text(3661, true), "01:01:01");
        assert_eq!(clock_text(86399, false), "23:59");
    }
}
