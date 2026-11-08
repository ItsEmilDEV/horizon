//! The way back: a trail of breadcrumbs behind the player, waypoints the game sets by itself (the portal you came through,
//! the spawn point) and the compass HUD module that points at a waypoint, at the nearest one or at where the trail began.
use super::Game;
use horizon_client::{hud::HudBatch, waypoints};
use std::collections::VecDeque;

const CAP: usize = 200;
/// A breadcrumb is dropped when the player is this far from the last one.
const STEP: f64 = 4.0;
/// Breadcrumbs further away than this are not drawn.
const SHOWN: f64 = 96.0;

#[derive(Default)]
pub(super) struct Trail {
    crumbs: VecDeque<[f64; 3]>,
    dim: &'static str,
    /// Where the player was last frame and in which dimension, for the portal waypoint.
    last: Option<(&'static str, [f64; 3])>,
    spawn: Option<[i32; 3]>,
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Trail {
    /// Adds a breadcrumb when the player moved far enough; a new dimension starts a new trail.
    pub(super) fn push(&mut self, dim: &'static str, at: [f64; 3]) -> bool {
        if self.dim != dim {
            self.crumbs.clear();
            self.dim = dim;
        }
        if self.crumbs.back().is_some_and(|last| dist(*last, at) < STEP) {
            return false;
        }
        if self.crumbs.len() == CAP {
            self.crumbs.pop_front();
        }
        self.crumbs.push_back(at);
        true
    }

    /// Where this trail began (the oldest breadcrumb).
    pub(super) fn start(&self) -> Option<[f64; 3]> {
        self.crumbs.front().copied()
    }
}

/// How far to turn from `yaw` to face `to` (degrees, -180..180, positive to the left as on the maps) and the distance.
pub(super) fn guide(from: [f64; 3], yaw: f32, to: [f64; 3]) -> (f32, f64) {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    let target = (-dx.atan2(dz)).to_degrees() as f32;
    let mut d = (target - yaw).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    (d, dist(from, to))
}

fn dimension_name(id: &str) -> &'static str {
    match id {
        "minecraft:the_nether" => "Nether",
        "minecraft:the_end" => "End",
        _ => "Overworld",
    }
}

impl Game<'_> {
    /// Once a frame: the trail, and the waypoints the game sets itself.
    pub(super) fn belumi_trail_frame(&mut self) {
        let Some(p) = self.native_player() else {
            self.belumi_trail.last = None;
            return;
        };
        let here = [p.x, p.y, p.z];
        if self.modules_ref().is_some_and(|m| m.misc.trail) {
            self.belumi_trail.push(p.dimension, here);
        }
        if self.settings.belumi.waypoints {
            // The portal: the spot you left a dimension from. Not after a death, which sends you home.
            if let Some((was, at)) = self.belumi_trail.last
                && was != p.dimension
                && !self.belumi_death_pending()
            {
                let name = format!("Portal {}", dimension_name(was));
                let scope = self.belumi_scope();
                let at = [at[0].floor() as i32, at[1].floor() as i32, at[2].floor() as i32];
                let store = self.belumi_waypoint_store();
                store.remove(&scope, &name);
                let _ = store.add(&scope, was, &name, at);
                self.belumi_save_waypoints();
            }
            // The spawn point the server sent.
            let spawn = self.spawn.map(|s| [s.x.floor() as i32, s.y.floor() as i32, s.z.floor() as i32]);
            if spawn != self.belumi_trail.spawn {
                self.belumi_trail.spawn = spawn;
                if let Some(at) = spawn {
                    let scope = self.belumi_scope();
                    let store = self.belumi_waypoint_store();
                    store.remove(&scope, "Spawn");
                    let _ = store.add(&scope, "minecraft:overworld", "Spawn", at);
                    self.belumi_save_waypoints();
                }
            }
        }
        self.belumi_trail.last = Some((p.dimension, here));
    }

    /// What the compass points at: its name and place.
    fn belumi_compass_target(&mut self) -> Option<(String, [f64; 3])> {
        let p = self.native_player()?;
        let from = [p.x, p.y, p.z];
        let want = self.modules_ref().map(|m| m.misc.compass_target.clone()).unwrap_or_default();
        if want.eq_ignore_ascii_case("back") {
            return self.belumi_trail.start().map(|at| ("Start".to_owned(), at));
        }
        let scope = self.belumi_scope();
        let store = self.belumi_waypoint_store();
        let found = if want.is_empty() || want.eq_ignore_ascii_case("nearest") {
            None
        } else {
            store.here(&scope, p.dimension).find(|w| w.name.eq_ignore_ascii_case(&want)).cloned()
        };
        let w = match found {
            Some(w) => w,
            None => store.nearest(&scope, p.dimension, from, 1).first().map(|(w, _)| (*w).clone())?,
        };
        Some((w.name.clone(), [f64::from(w.x) + 0.5, f64::from(w.y), f64::from(w.z) + 0.5]))
    }

    /// The compass module's text and colour.
    pub(super) fn belumi_compass_text(&mut self) -> (String, u32) {
        let (Some(p), Some((name, at))) = (self.native_player(), self.belumi_compass_target()) else {
            return ("No waypoint".to_owned(), 0x00AA_B3C6);
        };
        let (rel, d) = guide([p.x, p.y, p.z], p.yaw, at);
        (format!("{} {} {}", waypoints::arrow(rel), name, waypoints::distance_label(d)), 0x00FF_FFFF)
    }

    /// The breadcrumbs as small squares in the world.
    pub(super) fn belumi_draw_trail(&mut self, batch: &mut HudBatch) {
        if !self.modules_ref().is_some_and(|m| m.misc.trail) || !self.camera_type.first_person() {
            return;
        }
        let Some(p) = self.player.as_ref() else { return };
        let feet = [p.position.x, p.position.y, p.position.z];
        let Some((cam, bob)) = self.belumi_frame_camera(batch.width / batch.height.max(1.0)) else { return };
        let spots: Vec<[f64; 3]> = self.belumi_trail.crumbs.iter().copied().filter(|c| dist(*c, feet) <= SHOWN).collect();
        for c in spots {
            let Some((u, v)) = super::belumi_maps::project_bobbed(&cam, bob, [c[0], c[1] + 0.2, c[2]]) else { continue };
            let (x, y) = (u * batch.width, v * batch.height);
            self.native_fill(batch, [x - 2.0, y - 2.0, 4.0, 4.0], 0xC0FF_FFFF);
            self.native_fill(batch, [x - 1.0, y - 1.0, 2.0, 2.0], 0xFF55_AAFF);
        }
    }

    /// `/compass [nearest|back|NAME]` and `/trail [on|off|clear]`.
    pub(super) fn belumi_trail_command(&mut self, command: &str, ran: &mut bool) {
        if *ran {
            return;
        }
        let (head, rest) = match command.trim().split_once(char::is_whitespace) {
            Some((h, r)) => (h, r.trim()),
            None => (command.trim(), ""),
        };
        let reply = match head.to_ascii_lowercase().as_str() {
            "compass" => {
                let target = if rest.is_empty() { "nearest" } else { rest }.to_owned();
                let m = self.modules();
                m.misc.compass_target = if target.eq_ignore_ascii_case("nearest") { String::new() } else { target.clone() };
                self.modules_changed();
                format!("The compass points at: {target}.")
            }
            "trail" => {
                let on = match rest.to_ascii_lowercase().as_str() {
                    "on" => Some(true),
                    "off" => Some(false),
                    "clear" => {
                        self.belumi_trail.crumbs.clear();
                        None
                    }
                    "" => Some(!self.modules_ref().is_some_and(|m| m.misc.trail)),
                    _ => return,
                };
                if let Some(on) = on {
                    self.modules().misc.trail = on;
                    self.modules_changed();
                    format!("Breadcrumb trail {}.", if on { "on" } else { "off" })
                } else {
                    "Trail cleared.".to_owned()
                }
            }
            _ => return,
        };
        *ran = true;
        self.native_chat(&reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breadcrumb_is_dropped_only_after_a_few_blocks() {
        let mut t = Trail::default();
        assert!(t.push("a", [0.0, 64.0, 0.0]));
        assert!(!t.push("a", [1.0, 64.0, 1.0]));
        assert!(t.push("a", [5.0, 64.0, 0.0]));
        assert_eq!(t.start(), Some([0.0, 64.0, 0.0]));
    }

    #[test]
    fn the_trail_keeps_the_newest_crumbs_and_a_new_dimension_starts_over() {
        let mut t = Trail::default();
        for i in 0..(CAP + 50) {
            t.push("a", [i as f64 * 5.0, 64.0, 0.0]);
        }
        assert_eq!(t.crumbs.len(), CAP);
        assert_eq!(t.start().unwrap()[0], 50.0 * 5.0);
        t.push("b", [0.0, 64.0, 0.0]);
        assert_eq!(t.crumbs.len(), 1);
    }

    #[test]
    fn the_guide_turns_toward_the_target() {
        // Facing south (yaw 0), a target to the south is straight ahead, to the west (+x is east) 90 degrees right.
        let (rel, d) = guide([0.0, 0.0, 0.0], 0.0, [0.0, 0.0, 10.0]);
        assert!(rel.abs() < 0.01 && (d - 10.0).abs() < 1e-9);
        let (east, _) = guide([0.0, 0.0, 0.0], 0.0, [10.0, 0.0, 0.0]);
        let (west, _) = guide([0.0, 0.0, 0.0], 0.0, [-10.0, 0.0, 0.0]);
        assert!((east + 90.0).abs() < 0.01 && (west - 90.0).abs() < 0.01);
        let (behind, _) = guide([0.0, 0.0, 0.0], 0.0, [0.0, 0.0, -10.0]);
        assert!((behind.abs() - 180.0).abs() < 0.01);
    }

    #[test]
    fn the_guide_agrees_with_the_waypoint_bearing() {
        let w = waypoints::Waypoint {
            scope: "s".into(),
            dimension: "d".into(),
            name: "w".into(),
            x: 20,
            y: 64,
            z: -7,
            color: 0,
        };
        let from = [3.0, 64.0, 4.0];
        let (rel, _) = guide(from, 33.0, [f64::from(w.x) + 0.5, f64::from(w.y), f64::from(w.z) + 0.5]);
        assert!((rel - waypoints::bearing(&w, from, 33.0)).abs() < 0.001);
    }
}
