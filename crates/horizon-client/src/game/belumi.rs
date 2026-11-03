use super::{Game, native_hooks::NativeZoom};
use horizon_client::{extras_hud::Corners, hud::HudBatch};

const BOX_COLOR: u32 = 0x7800_0000;
const TEXT_COLOR: u32 = 0x00FF_FFFF;
const ZOOM_MIN: f32 = 1.1;
const ZOOM_MAX: f32 = 10.0;

#[derive(Default)]
pub(super) struct BelumiState {
    zoom_held: bool,
    zoom_factor: Option<f32>,
    last_position: Option<[f64; 3]>,
    speed: f32,
    speed_clock: f32,
    speed_since: f32,
}

pub(super) fn cardinal(yaw: f32) -> &'static str {
    let y = yaw.rem_euclid(360.0);
    match ((y + 22.5) / 45.0) as u32 % 8 {
        0 => "S",
        1 => "SW",
        2 => "W",
        3 => "NW",
        4 => "N",
        5 => "NE",
        6 => "E",
        _ => "SE",
    }
}

pub(super) fn zoom_after_wheel(current: f32, steps: f32) -> f32 {
    (current * 1.15_f32.powf(steps)).clamp(ZOOM_MIN, ZOOM_MAX)
}

impl Game<'_> {
    pub(super) fn belumi_frame(&mut self, elapsed: std::time::Duration) {
        let dt = elapsed.as_secs_f32();
        let Some(p) = self.native_player() else {
            self.belumi.last_position = None;
            return;
        };
        let now = [p.x, p.y, p.z];
        self.belumi.speed_since += dt;
        if let Some(last) = self.belumi.last_position {
            let d = ((now[0] - last[0]).powi(2) + (now[2] - last[2]).powi(2)).sqrt() as f32;
            self.belumi.speed_clock += d;
        }
        self.belumi.last_position = Some(now);
        if self.belumi.speed_since >= 0.25 {
            self.belumi.speed = self.belumi.speed_clock / self.belumi.speed_since;
            self.belumi.speed_clock = 0.0;
            self.belumi.speed_since = 0.0;
        }
    }

    pub(super) fn belumi_key(&mut self, key: &str, pressed: bool) {
        let cfg = &self.settings.belumi;
        if key != cfg.zoom_key {
            return;
        }
        if !pressed {
            self.belumi.zoom_held = false;
            self.belumi.zoom_factor = None;
        } else if cfg.zoom && self.native_playing() {
            self.belumi.zoom_held = true;
            self.belumi.zoom_factor = Some(cfg.zoom_factor());
        }
    }

    pub(super) fn belumi_wheel(&mut self, steps: f32, claimed: &mut bool) {
        if !self.belumi.zoom_held || !self.settings.belumi.zoom_scroll {
            return;
        }
        let current = self
            .belumi
            .zoom_factor
            .unwrap_or_else(|| self.settings.belumi.zoom_factor());
        self.belumi.zoom_factor = Some(zoom_after_wheel(current, steps));
        *claimed = true;
    }

    pub(super) fn belumi_zoom(&self, zoom: &mut Option<NativeZoom>) {
        if zoom.is_some() || !self.belumi.zoom_held || !self.settings.belumi.zoom {
            return;
        }
        let factor = self
            .belumi
            .zoom_factor
            .unwrap_or_else(|| self.settings.belumi.zoom_factor());
        *zoom = Some(NativeZoom {
            factor,
            smooth: self.settings.belumi.zoom_smooth,
            turn: 1.0 / factor,
        });
    }

    fn belumi_own_ping(&self) -> Option<i32> {
        let name = self.options.player_name();
        let r = self.remote.as_ref()?;
        r.tab_players
            .values()
            .find(|p| p.name == name)
            .map(|p| p.latency)
            .filter(|l| *l >= 0)
    }

    pub(super) fn belumi_hud_lines(&self) -> Vec<String> {
        let cfg = &self.settings.belumi;
        let mut lines = Vec::new();
        if !cfg.hud {
            return lines;
        }
        if cfg.hud_fps {
            lines.push(format!("{} FPS", self.native_frame_rate().0));
        }
        if let Some(p) = self.native_player() {
            if cfg.hud_coords {
                lines.push(format!("XYZ {:.0} {:.0} {:.0}", p.x, p.y, p.z));
            }
            if cfg.hud_direction {
                lines.push(format!(
                    "{} ({:.0})",
                    cardinal(p.yaw),
                    p.yaw.rem_euclid(360.0)
                ));
            }
            if cfg.hud_speed {
                lines.push(format!("{:.1} b/s", self.belumi.speed));
            }
        }
        if cfg.hud_ping
            && let Some(ms) = self.belumi_own_ping()
        {
            lines.push(format!("{ms} ms"));
        }
        lines
    }

    pub(super) fn belumi_hud(&mut self, batch: &mut HudBatch, corners: &mut Corners) {
        let corner = self.settings.belumi.corner();
        if !self.native_corner_shown(corner) {
            return;
        }
        let lines = self.belumi_hud_lines();
        if lines.is_empty() {
            return;
        }
        let width = lines
            .iter()
            .map(|l| self.native_text_width(l))
            .fold(0.0_f32, f32::max)
            + 6.0;
        let height = lines.len() as f32 * 10.0 + 2.0;
        let (x, y) = corners.place(corner, width, height);
        self.native_fill(batch, [x, y, width, height], BOX_COLOR);
        for (i, line) in lines.iter().enumerate() {
            self.native_text(
                batch,
                line,
                [x + 3.0, y + 2.0 + i as f32 * 10.0],
                TEXT_COLOR,
                true,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_maps_to_minecraft_compass() {
        assert_eq!(cardinal(0.0), "S");
        assert_eq!(cardinal(90.0), "W");
        assert_eq!(cardinal(180.0), "N");
        assert_eq!(cardinal(270.0), "E");
        assert_eq!(cardinal(-90.0), "E");
        assert_eq!(cardinal(359.0), "S");
    }

    #[test]
    fn wheel_zoom_stays_in_range() {
        assert!(zoom_after_wheel(2.0, 1.0) > 2.0);
        assert!(zoom_after_wheel(2.0, -1.0) < 2.0);
        assert_eq!(zoom_after_wheel(9.9, 50.0), ZOOM_MAX);
        assert_eq!(zoom_after_wheel(1.2, -50.0), ZOOM_MIN);
    }
}
