use super::{Dim, Game, native_hooks::NativeMinimap};
use horizon_client::{
    extras::Corner,
    hud::{HudBatch, WORLDMAP_SIZE, WORLDMAP_SPRITE, rgba},
    minimap::{Layer, Minimap, View},
    waypoints::{self, Waypoint, Waypoints},
    worldmap::Worldmap,
};
use std::{path::PathBuf, time::Duration};

const EXPLORE_BUDGET: Duration = Duration::from_micros(150);
const SAVE_EVERY: f32 = 30.0;
const MAP_ZOOMS: [f32; 6] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

pub(super) struct MapState {
    pub(super) server: String,
    waypoints: Option<Waypoints>,
    explorer: Minimap,
    worldmap: Worldmap,
    loaded_for: Option<(String, &'static str)>,
    since_save: f32,
    pub(super) open: bool,
    zoom: usize,
    image: Vec<u8>,
    composed: Option<(View, usize)>,
    pub(super) compose_skip: u8,
}
impl Default for MapState {
    fn default() -> Self {
        Self {
            server: String::new(),
            waypoints: None,
            explorer: Minimap::default(),
            worldmap: Worldmap::default(),
            loaded_for: None,
            since_save: 0.0,
            open: false,
            zoom: 2,
            image: Vec::new(),
            composed: None,
            compose_skip: 0,
        }
    }
}

pub(super) fn safe_name(scope: &str) -> String {
    scope
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
        .collect()
}

pub(super) fn zoom_step(current: usize, steps: f32) -> usize {
    let next = current as i64 + steps.signum() as i64 * steps.abs().ceil().max(1.0) as i64;
    next.clamp(0, MAP_ZOOMS.len() as i64 - 1) as usize
}

pub(super) struct Camera {
    pub(super) eye: [f64; 3],
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) vertical_fov: f32,
    pub(super) aspect: f32,
}

pub(super) fn project(cam: &Camera, target: [f64; 3]) -> Option<(f32, f32)> {
    let d = [target[0] - cam.eye[0], target[1] - cam.eye[1], target[2] - cam.eye[2]];
    let (sy, cy) = f64::from(cam.yaw).to_radians().sin_cos();
    let (sp, cp) = f64::from(cam.pitch).to_radians().sin_cos();
    let forward = [-sy * cp, -sp, cy * cp];
    let right = [-cy, 0.0, -sy];
    let up = [-sy * sp, cp, cy * sp];
    let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
    let z = dot(forward);
    if z < 0.5 {
        return None;
    }
    let t = (f64::from(cam.vertical_fov).to_radians() / 2.0).tan();
    let nx = dot(right) / (z * t * f64::from(cam.aspect));
    let ny = dot(up) / (z * t);
    if nx.abs() > 1.2 || ny.abs() > 1.2 {
        return None;
    }
    Some((((nx + 1.0) / 2.0) as f32, ((1.0 - ny) / 2.0) as f32))
}

#[derive(Debug, PartialEq)]
pub(super) enum Command {
    Add(Option<String>),
    Remove(String),
    List,
    Clear,
    Bad,
}

pub(super) fn parse_command(text: &str) -> Option<Command> {
    let mut words = text.split_whitespace();
    let head = words.next()?;
    if !head.eq_ignore_ascii_case("waypoint") && !head.eq_ignore_ascii_case("wp") {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    let name = rest.get(1..).map(|r| r.join(" ")).filter(|n| !n.is_empty());
    Some(match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("add") => Command::Add(name),
        Some("remove" | "delete") => name.map_or(Command::Bad, Command::Remove),
        Some("list") | None => Command::List,
        Some("clear") => Command::Clear,
        _ => Command::Bad,
    })
}

impl Game<'_> {
    fn belumi_dir() -> PathBuf {
        std::env::current_dir().unwrap_or_default()
    }

    pub(super) fn belumi_scope(&self) -> String {
        if self.remote.is_some() && !self.belumi_maps.server.is_empty() {
            format!("server:{}", self.belumi_maps.server)
        } else {
            format!("world:{}", self.level.name)
        }
    }

    pub(super) fn belumi_connecting(&mut self, address: &str) {
        self.belumi_maps.server = address.to_owned();
    }

    pub(super) fn belumi_maps_clear_server(&mut self) {
        self.belumi_maps.server.clear();
    }

    pub(super) fn belumi_left(&mut self) {
        self.belumi_freecam_off();
        self.belumi_save_map(true);
        self.belumi_maps.server.clear();
        self.belumi_maps.loaded_for = None;
        self.belumi_maps.open = false;
        self.belumi_maps.explorer.clear();
        self.belumi_maps.worldmap = Worldmap::default();
    }

    fn belumi_waypoint_store(&mut self) -> &mut Waypoints {
        self.belumi_maps
            .waypoints
            .get_or_insert_with(|| Waypoints::load(&Self::belumi_dir()))
    }

    fn belumi_save_waypoints(&self) {
        if let Some(w) = &self.belumi_maps.waypoints {
            w.save(&Self::belumi_dir());
        }
    }

    fn belumi_map_path(scope: &str, dim: &str) -> PathBuf {
        Self::belumi_dir()
            .join("belumi-maps")
            .join(safe_name(scope))
            .join(format!("{}.bin", safe_name(dim)))
    }

    fn belumi_save_map(&mut self, now: bool) {
        let Some((scope, dim)) = self.belumi_maps.loaded_for.clone() else {
            return;
        };
        let path = Self::belumi_map_path(&scope, dim);
        if now {
            self.belumi_maps.worldmap.save(&path);
        } else {
            self.belumi_maps.worldmap.save_in_background(&path);
        }
    }

    pub(super) fn belumi_explore(&mut self, dt: f32) {
        if !self.settings.belumi.worldmap || self.player.is_none() || self.dim == Dim::Nether {
            return;
        }
        let key = (self.belumi_scope(), self.dim.id());
        if self.belumi_maps.loaded_for.as_ref() != Some(&key) {
            self.belumi_save_map(true);
            self.belumi_maps.worldmap = Worldmap::load(&Self::belumi_map_path(&key.0, key.1));
            self.belumi_maps.explorer.clear();
            self.belumi_maps.composed = None;
            self.belumi_maps.loaded_for = Some(key);
        }
        let Some(p) = self.player.as_ref() else {
            return;
        };
        let center = ((p.position.x.floor() as i32) >> 4, (p.position.z.floor() as i32) >> 4);
        let radius = self.settings.render_distance.clamp(2, 10);
        self.belumi_maps.explorer.record = true;
        self.belumi_maps.explorer.update(
            &self.world,
            self.res.facts(),
            center,
            radius,
            Layer::Surface,
            EXPLORE_BUDGET,
        );
        let scanned = self.belumi_maps.explorer.take_scanned();
        self.belumi_maps.worldmap.absorb(scanned);
        self.belumi_maps.since_save += dt;
        if self.belumi_maps.since_save >= SAVE_EVERY {
            self.belumi_maps.since_save = 0.0;
            self.belumi_save_map(false);
        }
    }

    pub(super) fn belumi_minimap(&self, minimap: &mut Option<NativeMinimap>) {
        let cfg = &self.settings.belumi;
        if minimap.is_some() || !cfg.minimap {
            return;
        }
        *minimap = Some(NativeMinimap {
            corner: cfg.minimap_corner(),
            size: cfg.minimap_pixels(),
            blocks_per_pixel: cfg.minimap_blocks_per_pixel(),
            rotate: cfg.minimap_rotate,
            round: cfg.minimap_round,
        });
    }

    pub(super) fn belumi_add_waypoint(&mut self, name: Option<String>) -> String {
        let Some(p) = self.native_player() else {
            return "No player to place a waypoint for.".into();
        };
        let scope = self.belumi_scope();
        let dim = p.dimension;
        let at = [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32];
        let store = self.belumi_waypoint_store();
        let name = name.unwrap_or_else(|| store.next_name(&scope));
        let reply = match store.add(&scope, dim, &name, at) {
            Ok(w) => format!("Set waypoint {} at {} {} {}.", w.name, w.x, w.y, w.z),
            Err(e) => e,
        };
        self.belumi_save_waypoints();
        reply
    }

    pub(super) fn belumi_command(&mut self, command: &str, ran: &mut bool) {
        if *ran || !self.settings.belumi.waypoints {
            return;
        }
        let Some(cmd) = parse_command(command) else {
            return;
        };
        *ran = true;
        let scope = self.belumi_scope();
        let dim = self.native_player().map(|p| p.dimension);
        let reply = match cmd {
            Command::Add(name) => self.belumi_add_waypoint(name),
            Command::Remove(name) => {
                let removed = self.belumi_waypoint_store().remove(&scope, &name);
                self.belumi_save_waypoints();
                if removed {
                    format!("Waypoint {name} removed.")
                } else {
                    format!("No waypoint called {name}.")
                }
            }
            Command::Clear => {
                self.belumi_waypoint_store().list.retain(|w| w.scope != scope);
                self.belumi_save_waypoints();
                "All waypoints of this world removed.".into()
            }
            Command::List => {
                let from = self.native_player().map_or([0.0; 3], |p| [p.x, p.y, p.z]);
                let store = self.belumi_waypoint_store();
                let found = store.nearest(&scope, dim.unwrap_or(""), from, 50);
                if found.is_empty() {
                    "No waypoints here. Use /waypoint add NAME.".into()
                } else {
                    found
                        .iter()
                        .map(|(w, d)| format!("{} {} {} {} ({})", w.name, w.x, w.y, w.z, waypoints::distance_label(*d)))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
            Command::Bad => "Usage: /waypoint add [NAME] | remove NAME | list | clear".into(),
        };
        for line in reply.lines() {
            self.native_chat(line);
        }
    }

    pub(super) fn belumi_waypoint_lines(&mut self) -> Vec<String> {
        let cfg = &self.settings.belumi;
        if !cfg.waypoints || !cfg.waypoint_list {
            return Vec::new();
        }
        let Some(p) = self.native_player() else {
            return Vec::new();
        };
        let scope = self.belumi_scope();
        let store = self.belumi_waypoint_store();
        store
            .nearest(&scope, p.dimension, [p.x, p.y, p.z], 3)
            .into_iter()
            .map(|(w, d)| {
                let a = waypoints::arrow(waypoints::bearing(w, [p.x, p.y, p.z], p.yaw));
                format!("{a} {} {}", w.name, waypoints::distance_label(d))
            })
            .collect()
    }

    fn belumi_visible_waypoints(&mut self) -> Vec<Waypoint> {
        let Some(p) = self.native_player() else {
            return Vec::new();
        };
        let scope = self.belumi_scope();
        self.belumi_waypoint_store()
            .here(&scope, p.dimension)
            .cloned()
            .collect()
    }

    pub(super) fn belumi_draw_labels(&mut self, batch: &mut HudBatch) {
        let cfg = &self.settings.belumi;
        if !cfg.waypoints || !cfg.waypoint_labels || !self.camera_type.first_person() {
            return;
        }
        let Some(p) = self.player.as_ref() else {
            return;
        };
        let feet = [p.position.x, p.position.y, p.position.z];
        let eye = [feet[0], feet[1] + f64::from(p.dimensions.eye_height), feet[2]];
        let zoom = if self.belumi_zoom_active() { self.settings.belumi.zoom_factor() } else { 1.0 };
        let cam = Camera {
            eye,
            yaw: self.yaw,
            pitch: self.pitch,
            vertical_fov: (self.settings.fov / zoom).clamp(1.0, 170.0),
            aspect: batch.width / batch.height.max(1.0),
        };
        let list = self.belumi_visible_waypoints();
        for w in list {
            let target = [f64::from(w.x) + 0.5, f64::from(w.y) + 1.0, f64::from(w.z) + 0.5];
            let Some((u, v)) = project(&cam, target) else {
                continue;
            };
            let dist = waypoints::distance(&w, feet);
            let text = format!("{} {}", w.name, waypoints::distance_label(dist));
            let width = self.native_text_width(&text);
            let (x, y) = (u * batch.width - width / 2.0, v * batch.height - 4.0);
            self.native_fill(batch, [x - 2.0, y - 1.0, width + 4.0, 10.0], 0x6000_0000);
            self.native_text(batch, &text, [x, y], w.color, true);
        }
    }

    pub(super) fn belumi_map_key(&mut self, key: &str, pressed: bool) {
        if !pressed {
            return;
        }
        let cfg = &self.settings.belumi;
        if cfg.waypoints && key == cfg.waypoint_key && self.native_playing() {
            let reply = self.belumi_add_waypoint(None);
            self.native_chat(&reply);
        }
        let cfg = &self.settings.belumi;
        if cfg.worldmap && key == cfg.worldmap_key && self.native_playing() {
            self.belumi_maps.open = !self.belumi_maps.open;
        } else if key == "key.keyboard.escape" {
            self.belumi_maps.open = false;
        }
    }

    pub(super) fn belumi_map_wheel(&mut self, steps: f32, claimed: &mut bool) {
        if !self.belumi_maps.open || *claimed {
            return;
        }
        self.belumi_maps.zoom = zoom_step(self.belumi_maps.zoom, -steps);
        *claimed = true;
    }

    pub(super) fn belumi_draw_worldmap(&mut self, batch: &mut HudBatch) {
        if !self.belumi_maps.open || !self.settings.belumi.worldmap {
            return;
        }
        let Some(p) = self.native_player() else {
            return;
        };
        let atlas = &self.gui.atlas;
        let Some(sprite) = atlas.sprites.get(WORLDMAP_SPRITE).copied() else {
            return;
        };
        let bpp = MAP_ZOOMS[self.belumi_maps.zoom];
        let view = View {
            x: p.x,
            z: p.z,
            yaw: p.yaw,
            rotate: false,
            blocks_per_pixel: bpp,
            round: false,
        };
        let moved = self.belumi_maps.composed.is_none_or(|(c, _)| {
            (c.x - view.x).abs().max((c.z - view.z).abs()) as f32 >= bpp / 2.0
                || c.blocks_per_pixel != bpp
        });
        let fresh = self.belumi_maps.worldmap.is_dirty() && self.belumi_maps.compose_skip == 0;
        if (moved || fresh) && self.belumi_maps.compose_skip == 0 {
            let mut image = std::mem::take(&mut self.belumi_maps.image);
            self.belumi_maps.worldmap.compose(view, WORLDMAP_SIZE, &mut image);
            let (w, h) = (atlas.width as f32, atlas.height as f32);
            self.map_patches.push(horizon_client::font::Patch {
                x: (sprite.u0 * w).round() as u32,
                y: (sprite.v0 * h).round() as u32,
                width: WORLDMAP_SIZE as u32,
                height: WORLDMAP_SIZE as u32,
                rgba: image.clone(),
            });
            self.belumi_maps.image = image;
            self.belumi_maps.composed = Some((view, self.belumi_maps.zoom));
            self.belumi_maps.compose_skip = 2;
        }
        self.belumi_maps.compose_skip = self.belumi_maps.compose_skip.saturating_sub(1);
        let side = (batch.height - 24.0).min(batch.width - 24.0).max(64.0);
        let (x, y) = ((batch.width - side) / 2.0, (batch.height - side) / 2.0);
        batch.fill(&self.gui.atlas, 0.0, 0.0, batch.width, batch.height, rgba(0, 0, 0, 150));
        batch.rect_uv(x, y, side, side, sprite, u32::MAX);
        let scale = side / WORLDMAP_SIZE as f32;
        let to_screen = |wx: f64, wz: f64| {
            (
                x + side / 2.0 + ((wx - view.x) as f32 / bpp) * scale,
                y + side / 2.0 + ((wz - view.z) as f32 / bpp) * scale,
            )
        };
        let (px, py) = (x + side / 2.0, y + side / 2.0);
        if let Some(r) = self.gui.atlas.sprites.get("map/decorations/player").copied() {
            let (s, c) = (p.yaw + 180.0 + 180.0).to_radians().sin_cos();
            let q = |dx: f32, dy: f32| [px + dx * c - dy * s, py + dx * s + dy * c];
            batch.quad(
                [q(-4.0, 4.0), q(4.0, 4.0), q(4.0, -4.0), q(-4.0, -4.0)],
                [[r.u0, r.v0], [r.u1, r.v0], [r.u1, r.v1], [r.u0, r.v1]],
                u32::MAX,
            );
        }
        if self.settings.belumi.waypoints && self.settings.belumi.waypoint_labels {
            for w in self.belumi_visible_waypoints() {
                let (sx, sy) = to_screen(f64::from(w.x) + 0.5, f64::from(w.z) + 0.5);
                if sx < x || sy < y || sx > x + side || sy > y + side {
                    continue;
                }
                let [_, r, g, b] = w.color.to_be_bytes();
                batch.fill(&self.gui.atlas, sx - 2.5, sy - 2.5, 5.0, 5.0, rgba(r, g, b, 255));
                self.native_text(batch, &w.name, [sx + 4.0, sy - 4.0], w.color, true);
            }
        }
        let title = format!(
            "World map  {:.0} {:.0}  {} blocks/px   [wheel: zoom]",
            p.x,
            p.z,
            bpp
        );
        self.native_text(batch, &title, [x + 4.0, y + 4.0], 0x00FF_FFFF, true);
        if self.belumi_maps.worldmap.is_empty() {
            self.native_text(batch, "Explore to fill the map.", [px - 50.0, py + 14.0], 0x00AA_AAAA, true);
        }
        let _ = Corner::TopLeft;
    }

    pub(super) fn belumi_zoom_active(&self) -> bool {
        self.belumi.zoom_held()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera { eye: [0.0, 0.0, 0.0], yaw: 0.0, pitch: 0.0, vertical_fov: 70.0, aspect: 16.0 / 9.0 }
    }

    #[test]
    fn a_point_straight_ahead_projects_to_the_screen_centre() {
        let (u, v) = project(&cam(), [0.0, 0.0, 10.0]).unwrap();
        assert!((u - 0.5).abs() < 1e-4 && (v - 0.5).abs() < 1e-4);
    }

    #[test]
    fn right_is_right_up_is_up_and_behind_is_hidden() {
        let west = project(&cam(), [-5.0, 0.0, 10.0]).unwrap();
        assert!(west.0 > 0.5, "facing south, west is on the right");
        let above = project(&cam(), [0.0, 3.0, 10.0]).unwrap();
        assert!(above.1 < 0.5);
        assert!(project(&cam(), [0.0, 0.0, -10.0]).is_none());
        assert!(project(&cam(), [300.0, 0.0, 10.0]).is_none());
    }

    #[test]
    fn labels_stay_put_when_the_view_is_pitched() {
        let level = project(&cam(), [0.0, 0.0, 10.0]).unwrap();
        let mut pitched = cam();
        pitched.pitch = 30.0;
        let (u, v) = project(&pitched, [0.0, -10.0 * 30f64.to_radians().tan(), 10.0]).unwrap();
        assert!((u - level.0).abs() < 1e-3 && (v - level.1).abs() < 1e-3, "{u} {v}");
        pitched.yaw = 40.0;
        let (sy, cy) = 40f64.to_radians().sin_cos();
        let ahead = [-sy * 10.0, -10.0 * 30f64.to_radians().tan(), cy * 10.0];
        let (u, v) = project(&pitched, ahead).unwrap();
        assert!((u - 0.5).abs() < 0.2 && v < 0.6, "{u} {v}");
    }

    #[test]
    fn commands_parse_with_multi_word_names() {
        assert_eq!(parse_command("waypoint add Big Base"), Some(Command::Add(Some("Big Base".into()))));
        assert_eq!(parse_command("wp add"), Some(Command::Add(None)));
        assert_eq!(parse_command("waypoint remove Home"), Some(Command::Remove("Home".into())));
        assert_eq!(parse_command("waypoint remove"), Some(Command::Bad));
        assert_eq!(parse_command("waypoint"), Some(Command::List));
        assert_eq!(parse_command("waypoint zap"), Some(Command::Bad));
        assert_eq!(parse_command("gamemode creative"), None);
    }

    #[test]
    fn map_zoom_steps_stay_in_range_and_scopes_become_file_names() {
        assert_eq!(zoom_step(2, 1.0), 3);
        assert_eq!(zoom_step(0, -3.0), 0);
        assert_eq!(zoom_step(5, 1.0), 5);
        assert_eq!(safe_name("server:play.example.org:25565"), "server_play.example.org_25565");
        assert!(!safe_name("world:../../x").contains('/'));
    }
}
