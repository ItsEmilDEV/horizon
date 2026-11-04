use crate::extras::Corner;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub const FILE: &str = "belumi.toml";
pub const ZOOM_KEY: &str = "belumi.zoom";
pub const INSPECTOR_KEY: &str = "belumi.inspector";

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
        }
    }
}

pub const ZOOM_LEVELS: [f32; 5] = [1.5, 2.0, 3.0, 4.0, 8.0];

impl Belumi {
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(FILE))
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }
    pub fn parse(text: &str) -> Self {
        let mut b: Self = toml::from_str(text).unwrap_or_default();
        b.zoom_level = b.zoom_level.min(ZOOM_LEVELS.len() as u8 - 1);
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
        Corner::ALL
            .into_iter()
            .find(|c| c.name() == self.hud_corner)
            .unwrap_or(Corner::TopLeft)
    }
    pub fn inspector_corner(&self) -> Corner {
        Corner::ALL
            .into_iter()
            .find(|c| c.name() == self.inspector_corner)
            .unwrap_or(Corner::BottomLeft)
    }
    pub fn zoom_factor(&self) -> f32 {
        ZOOM_LEVELS[usize::from(self.zoom_level).min(ZOOM_LEVELS.len() - 1)]
    }
    pub fn set_key(&mut self, name: &str, value: String) -> bool {
        match name {
            ZOOM_KEY => self.zoom_key = value,
            INSPECTOR_KEY => self.inspector_key = value,
            _ => return false,
        }
        true
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
    Inspector,
    InspectorKey,
    InspectorCorner,
}

pub fn rows() -> Vec<Result<Item, &'static str>> {
    use Item::*;
    vec![
        Err("Heads-up display"),
        Ok(Hud),
        Ok(HudCorner),
        Ok(HudCoords),
        Ok(HudFps),
        Ok(HudPing),
        Ok(HudDirection),
        Ok(HudSpeed),
        Err("Zoom"),
        Ok(Zoom),
        Ok(ZoomKey),
        Ok(ZoomLevel),
        Ok(ZoomSmooth),
        Ok(ZoomScroll),
        Err("Debug"),
        Ok(Inspector),
        Ok(InspectorKey),
        Ok(InspectorCorner),
    ]
}

impl Item {
    pub fn caption(self) -> &'static str {
        match self {
            Item::Hud => "HUD",
            Item::HudCorner => "HUD corner",
            Item::HudCoords => "Coordinates",
            Item::HudFps => "FPS",
            Item::HudPing => "Ping",
            Item::HudDirection => "Direction",
            Item::HudSpeed => "Speed",
            Item::Zoom => "Zoom",
            Item::ZoomKey => "Zoom key",
            Item::ZoomLevel => "Zoom level",
            Item::ZoomSmooth => "Smooth zoom",
            Item::ZoomScroll => "Scroll adjusts zoom",
            Item::Inspector => "Packet inspector",
            Item::InspectorKey => "Inspector key",
            Item::InspectorCorner => "Inspector corner",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Item::Hud => "Shows a small overlay with the lines below.",
            Item::HudCorner => "The screen corner the overlay sits in.",
            Item::HudCoords => "Your block position.",
            Item::HudFps => "Frames per second.",
            Item::HudPing => "Round-trip time to the server in multiplayer.",
            Item::HudDirection => "The way you face and the axis it points along.",
            Item::HudSpeed => "Your speed in blocks per second.",
            Item::Zoom => "Hold the zoom key to narrow the field of view.",
            Item::ZoomKey => "Click, then press a key; Escape unbinds it.",
            Item::ZoomLevel => "How far the zoom key magnifies.",
            Item::ZoomSmooth => "Eases the field of view in and out.",
            Item::ZoomScroll => "The mouse wheel changes the zoom while the key is held.",
            Item::Inspector => "A key shows live packet rates, sizes and the latest packets.",
            Item::InspectorKey => "Click, then press a key; Escape unbinds it.",
            Item::InspectorCorner => "The screen corner the inspector sits in.",
        }
    }
    pub fn parent(self) -> Option<Item> {
        use Item::*;
        match self {
            HudCorner | HudCoords | HudFps | HudPing | HudDirection | HudSpeed => Some(Hud),
            ZoomKey | ZoomLevel | ZoomSmooth | ZoomScroll => Some(Zoom),
            InspectorKey | InspectorCorner => Some(Inspector),
            Hud | Zoom | Inspector => None,
        }
    }
    pub fn switch(self, b: &Belumi) -> Option<bool> {
        Some(match self {
            Item::Hud => b.hud,
            Item::HudCoords => b.hud_coords,
            Item::HudFps => b.hud_fps,
            Item::HudPing => b.hud_ping,
            Item::HudDirection => b.hud_direction,
            Item::HudSpeed => b.hud_speed,
            Item::Zoom => b.zoom,
            Item::ZoomSmooth => b.zoom_smooth,
            Item::ZoomScroll => b.zoom_scroll,
            Item::Inspector => b.inspector,
            _ => return None,
        })
    }
    pub fn value(self, b: &Belumi) -> String {
        match self {
            Item::HudCorner => b.corner().label().into(),
            Item::InspectorCorner => b.inspector_corner().label().into(),
            Item::InspectorKey => crate::keybinds::display_name(&b.inspector_key).plain(),
            Item::ZoomKey => crate::keybinds::display_name(&b.zoom_key).plain(),
            Item::ZoomLevel => format!("{}x", b.zoom_factor()),
            other => match other.switch(b) {
                Some(true) => "On".into(),
                _ => "Off".into(),
            },
        }
    }
    pub fn cycle(self, b: &mut Belumi, back: bool) {
        match self {
            Item::Hud => b.hud = !b.hud,
            Item::HudCoords => b.hud_coords = !b.hud_coords,
            Item::HudFps => b.hud_fps = !b.hud_fps,
            Item::HudPing => b.hud_ping = !b.hud_ping,
            Item::HudDirection => b.hud_direction = !b.hud_direction,
            Item::HudSpeed => b.hud_speed = !b.hud_speed,
            Item::Zoom => b.zoom = !b.zoom,
            Item::ZoomSmooth => b.zoom_smooth = !b.zoom_smooth,
            Item::ZoomScroll => b.zoom_scroll = !b.zoom_scroll,
            Item::Inspector => b.inspector = !b.inspector,
            Item::HudCorner => {
                let i = Corner::ALL
                    .iter()
                    .position(|c| *c == b.corner())
                    .unwrap_or(0);
                let n = Corner::ALL.len();
                let next = if back { (i + n - 1) % n } else { (i + 1) % n };
                b.hud_corner = Corner::ALL[next].name().into();
            }
            Item::ZoomLevel => {
                let n = ZOOM_LEVELS.len() as u8;
                b.zoom_level = if back {
                    (b.zoom_level + n - 1) % n
                } else {
                    (b.zoom_level + 1) % n
                };
            }
            Item::InspectorCorner => {
                let i = Corner::ALL
                    .iter()
                    .position(|c| *c == b.inspector_corner())
                    .unwrap_or(0);
                let n = Corner::ALL.len();
                let next = if back { (i + n - 1) % n } else { (i + 1) % n };
                b.inspector_corner = Corner::ALL[next].name().into();
            }
            Item::ZoomKey | Item::InspectorKey => {}
        }
    }
    pub fn key_name(self) -> Option<&'static str> {
        match self {
            Item::ZoomKey => Some(ZOOM_KEY),
            Item::InspectorKey => Some(INSPECTOR_KEY),
            _ => None,
        }
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
        assert_eq!(
            Belumi::parse("zoom_level = 99").zoom_level,
            ZOOM_LEVELS.len() as u8 - 1
        );
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
        assert_eq!(Item::HudPing.parent(), Some(Item::Hud));
    }

    #[test]
    fn every_row_item_has_text() {
        for item in rows().into_iter().flatten() {
            assert!(!item.caption().is_empty() && !item.description().is_empty());
        }
    }
}
