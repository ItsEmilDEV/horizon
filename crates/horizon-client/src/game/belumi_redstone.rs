//! The redstone workbench: the signal strength written over redstone dust and its relatives, buttons for `/tick` (freeze,
//! step, rate) and a profiler module showing what the server's tick spends its time on (single player).
use super::{Game, belumi_client::{Act, Page}};
use horizon_client::hud::HudBatch;
use std::time::{Duration, Instant};

const RADIUS: i32 = 12;
const HEIGHT: i32 = 6;
const EVERY: Duration = Duration::from_millis(500);
const MAX_SPOTS: usize = 160;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Spot {
    pub(super) pos: [i32; 3],
    pub(super) text: String,
    pub(super) color: u32,
}

#[derive(Default)]
pub(super) struct Redstone {
    spots: Vec<Spot>,
    scanned: Option<Instant>,
}

/// The colour of a signal: dark red for 1, bright yellow-white for 15, grey for none.
pub(super) fn power_color(power: u8) -> u32 {
    if power == 0 {
        return 0x0080_8080;
    }
    let p = u32::from(power.min(15));
    (0xA0 + p * 5) << 16 | (p * 14) << 8 | 0x30
}

/// What a block shows: the signal strength of dust and the like, the delay of a repeater; none for anything else.
/// `name` is the block's id, `property` reads one of its state properties.
pub(super) fn label(name: &str, property: &dyn Fn(&str) -> Option<String>) -> Option<(String, u32)> {
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    if short == "repeater" {
        let delay = property("delay")?;
        return Some((format!("{delay}t"), 0x0090_C0FF));
    }
    if short == "redstone_wire" || short.ends_with("weighted_pressure_plate") || short == "daylight_detector" || short == "target" {
        let power: u8 = property("power")?.parse().ok()?;
        return Some((power.to_string(), power_color(power)));
    }
    None
}

impl Game<'_> {
    /// Once a frame: the live profiler follows the module, the overlay is measured twice a second.
    pub(super) fn belumi_redstone_frame(&mut self) {
        let (numbers, profiler) = match self.modules_ref() {
            Some(m) => (m.misc.redstone_numbers, m.hud.iter().any(|p| p.id == "profiler" && p.enabled)),
            None => (false, false),
        };
        let was_live = horizon_client::belumi_profile::live();
        if profiler != was_live {
            horizon_client::belumi_profile::set_live(profiler);
        }
        if profiler {
            // The window thread's own spans nobody reads.
            let _ = horizon_client::spans::take();
        }
        if !numbers || self.player.is_none() {
            self.belumi_redstone.spots.clear();
            return;
        }
        if self.belumi_redstone.scanned.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.belumi_redstone.scanned = Some(Instant::now());
        let Some(p) = self.player.as_ref() else { return };
        let (cx, cy, cz) = (p.position.x.floor() as i32, p.position.y.floor() as i32, p.position.z.floor() as i32);
        let facts = self.res.facts();
        let registry = self.pack.assets.registry;
        let mut spots = Vec::new();
        'scan: for x in cx - RADIUS..=cx + RADIUS {
            for z in cz - RADIUS..=cz + RADIUS {
                if !self.world.chunks.contains_key(&(x >> 4, z >> 4)) {
                    continue;
                }
                for y in cy - HEIGHT..=cy + HEIGHT {
                    let id = self.world.block(x, y, z);
                    if facts.is_air(id) {
                        continue;
                    }
                    let name = &facts.get(id).name;
                    if !(name.contains("redstone_wire") || name.contains("repeater") || name.contains("pressure_plate") || name.contains("daylight") || name.ends_with(":target")) {
                        continue;
                    }
                    let Some(state) = registry.state(id) else { continue };
                    let prop = |k: &str| state.properties.get(k).map(|v| v.to_string());
                    if let Some((text, color)) = label(name, &prop) {
                        spots.push(Spot { pos: [x, y, z], text, color });
                        if spots.len() >= MAX_SPOTS {
                            break 'scan;
                        }
                    }
                }
            }
        }
        self.belumi_redstone.spots = spots;
    }

    /// The numbers, in the world above their blocks.
    pub(super) fn belumi_draw_redstone(&mut self, batch: &mut HudBatch) {
        if self.belumi_redstone.spots.is_empty() || !self.camera_type.first_person() {
            return;
        }
        let Some((cam, bob)) = self.belumi_frame_camera(batch.width / batch.height.max(1.0)) else { return };
        let spots = self.belumi_redstone.spots.clone();
        for s in spots {
            let Some((u, v)) = super::belumi_maps::project_bobbed(&cam, bob, [f64::from(s.pos[0]) + 0.5, f64::from(s.pos[1]) + 0.35, f64::from(s.pos[2]) + 0.5]) else { continue };
            let w = self.native_text_width(&s.text);
            let (x, y) = (u * batch.width - w / 2.0, v * batch.height - 4.0);
            self.native_text(batch, &s.text, [x, y], s.color, true);
        }
    }

    /// The Profiler module's lines: the server tick and its biggest parts.
    pub(super) fn belumi_profiler_lines(&self) -> Vec<(String, u32)> {
        if self.integrated.is_none() {
            return vec![("Server tick: single player only".into(), 0x00AA_B3C6)];
        }
        let (tick, spans) = horizon_client::belumi_profile::live_snapshot(6);
        let color = if tick < 25.0 { 0x004D_FF6A } else if tick < 45.0 { 0x00FF_E14D } else { 0x00FF_6B6B };
        let mut lines = vec![(format!("Server tick {tick:.1} ms (of 50)"), color)];
        lines.extend(spans.into_iter().map(|(n, ms)| (format!("{n} {ms:.2}"), 0x00FF_FFFF)));
        lines
    }

    fn belumi_tick_command(&mut self, command: &'static str) {
        self.send_chat(&format!("/{command}"));
    }

    pub(super) fn redstone_act(&mut self, act: &Act) -> bool {
        match act {
            Act::TickCmd(c) => {
                self.belumi_tick_command(c);
                true
            }
            Act::RedstoneNumbers => {
                let m = self.modules();
                m.misc.redstone_numbers = !m.misc.redstone_numbers;
                self.modules_changed();
                true
            }
            _ => false,
        }
    }

    pub(super) fn draw_redstone_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let pw = 360.0_f32.min(w - 12.0);
        let ph = (h - 16.0).clamp(200.0, 236.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Redstone workbench");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let numbers = self.modules_ref().is_some_and(|m| m.misc.redstone_numbers);
        let mut ry = y + 28.0;
        self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], if numbers { "Signal numbers: ON" } else { "Signal numbers: OFF" }, Some(numbers), Act::RedstoneNumbers);
        ry += 24.0;
        let (frozen, rate) = (self.clock.frozen(), self.clock.rate());
        let status = if frozen { "Time is frozen".to_owned() } else { format!("Ticking at {rate:.0} per second") };
        self.native_text(batch, &status, [x + 10.0, ry], 0x00FF_FFFF, false);
        ry += 14.0;
        let half = (pw - 30.0) / 2.0;
        self.client_button(batch, [x + 10.0, ry, half, 16.0], if frozen { "Unfreeze" } else { "Freeze" }, None, Act::TickCmd(if frozen { "tick unfreeze" } else { "tick freeze" }));
        self.client_button(batch, [x + 20.0 + half, ry, half, 16.0], "Step 1 tick", None, Act::TickCmd("tick step 1"));
        ry += 22.0;
        self.client_button(batch, [x + 10.0, ry, half, 16.0], "Step 5 ticks", None, Act::TickCmd("tick step 5"));
        self.client_button(batch, [x + 20.0 + half, ry, half, 16.0], "Step 20 ticks", None, Act::TickCmd("tick step 20"));
        ry += 22.0;
        let q = (pw - 20.0 - 18.0) / 4.0;
        for (i, (label, cmd)) in [("1/s", "tick rate 1"), ("5/s", "tick rate 5"), ("10/s", "tick rate 10"), ("20/s", "tick rate 20")].into_iter().enumerate() {
            self.client_button(batch, [x + 10.0 + i as f32 * (q + 6.0), ry, q, 16.0], label, None, Act::TickCmd(cmd));
        }
        ry += 26.0;
        self.native_text(batch, "Needs commands (a creative world). Turn the Profiler module on", [x + 10.0, ry], 0x008A_93A8, false);
        self.native_text(batch, "in the HUD builder to see what the server tick spends its time on.", [x + 10.0, ry + 12.0], 0x008A_93A8, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(list: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| list.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned())
    }

    #[test]
    fn dust_shows_its_power_and_a_repeater_its_delay() {
        assert_eq!(label("minecraft:redstone_wire", &props(&[("power", "13")])).map(|l| l.0), Some("13".into()));
        assert_eq!(label("minecraft:repeater", &props(&[("delay", "3")])).map(|l| l.0), Some("3t".into()));
        assert_eq!(label("minecraft:light_weighted_pressure_plate", &props(&[("power", "7")])).map(|l| l.0), Some("7".into()));
        assert_eq!(label("minecraft:stone", &props(&[("power", "7")])), None);
        assert_eq!(label("minecraft:redstone_wire", &props(&[])), None);
        assert_eq!(label("minecraft:redstone_wire", &props(&[("power", "x")])), None);
    }

    #[test]
    fn stronger_signals_are_brighter_and_none_is_grey() {
        assert_eq!(power_color(0), 0x0080_8080);
        let red = |c: u32| c >> 16;
        assert!(red(power_color(15)) > red(power_color(1)));
        let green = |c: u32| (c >> 8) & 0xFF;
        assert!(green(power_color(15)) > green(power_color(1)));
        assert!(power_color(15) <= 0x00FF_FFFF && power_color(200) == power_color(15));
    }
}
