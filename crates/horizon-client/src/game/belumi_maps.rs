use super::{Dim, Game, native_hooks::NativeMinimap};
use horizon_client::{
    hud::{HudBatch, WORLDMAP_SIZE, WORLDMAP_SPRITE, rgba},
    mapengine::{Overlay, Painter, WORLD_WINDOW, paint_sized},
    regionmap::{REGION, RegionMap},
    waypoints::{self, Waypoint, Waypoints},
};
use std::{path::PathBuf, time::Instant};

const SAVE_EVERY: f32 = 30.0;
const MAP_ZOOMS: [f32; 8] = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
const MAX_CAVE_MAPS: usize = 8;
const DEFAULT_ZOOM: usize = 2;

/// A map layer: the surface, or the cave floor at a height band (`y >> 4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layer {
    Surface,
    Cave(i32),
}

pub(super) struct MapState {
    pub(super) server: String,
    waypoints: Option<Waypoints>,
    regions: Option<RegionMap>,
    /// The cave maps by 16-block height band (a cave floor or the Nether), opened as they are scanned or viewed.
    caves: std::collections::HashMap<i32, RegionMap>,
    /// The layer the open map shows when the player picked one; none follows where the player is.
    layer: Option<Layer>,
    /// What the painter last drew, so a change repaints.
    painted: Option<Layer>,
    painter: Painter,
    /// Where the open map is centred; none follows the player.
    centre: Option<(f64, f64)>,
    last_draw: Option<Instant>,
    loaded_for: Option<(String, &'static str)>,
    since_save: f32,
    pub(super) open: bool,
    zoom: usize,
    died: bool,
}
impl Default for MapState {
    fn default() -> Self {
        Self {
            server: String::new(),
            waypoints: None,
            regions: None,
            caves: Default::default(),
            layer: None,
            painted: None,
            painter: Painter::new(),
            centre: None,
            last_draw: None,
            loaded_for: None,
            since_save: 0.0,
            open: false,
            zoom: DEFAULT_ZOOM,
            died: false,
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
    Import(String),
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
        Some("import") => name.map_or(Command::Bad, Command::Import),
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

    /// The chunk borders and slime chunks the maps draw over the terrain.
    pub(super) fn belumi_map_overlay(&self) -> Overlay {
        let cfg = &self.settings.belumi;
        Overlay {
            grid: cfg.map_grid,
            slime_seed: (cfg.map_slime && self.dim == Dim::Overworld).then(|| self.belumi_world_seed()).flatten(),
        }
    }

    fn belumi_world_seed(&self) -> Option<i64> {
        let scope = self.belumi_scope();
        self.settings
            .belumi
            .map_seeds
            .get(&scope)
            .copied()
            .or_else(|| scope.starts_with("world:").then_some(self.level.seed))
    }

    /// `/map seed N | grid | slime | export`: the map options and a PNG of everything explored.
    pub(super) fn belumi_map_command(&mut self, command: &str, ran: &mut bool) {
        if *ran {
            return;
        }
        let mut words = command.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("map")) {
            return;
        }
        *ran = true;
        let sub = words.next().map(str::to_ascii_lowercase);
        let reply = match sub.as_deref() {
            Some("seed") => match words.next().and_then(|n| n.parse::<i64>().ok()) {
                Some(seed) => {
                    let scope = self.belumi_scope();
                    self.settings.belumi.map_seeds.insert(scope, seed);
                    self.settings.save(&Self::options_path());
                    self.belumi_maps.painter.reset();
                    format!("Seed {seed} saved for slime chunks.")
                }
                None => match self.belumi_world_seed() {
                    Some(seed) => format!("Seed {seed}. Set another with /map seed NUMBER."),
                    None => "No seed known here: type /map seed NUMBER.".to_owned(),
                },
            },
            Some("grid") => {
                self.settings.belumi.map_grid = !self.settings.belumi.map_grid;
                self.settings.save(&Self::options_path());
                format!("Chunk borders {}.", if self.settings.belumi.map_grid { "on" } else { "off" })
            }
            Some("slime") => {
                self.settings.belumi.map_slime = !self.settings.belumi.map_slime;
                self.settings.save(&Self::options_path());
                match (self.settings.belumi.map_slime, self.belumi_world_seed()) {
                    (true, Some(_)) => "Slime chunks on.".to_owned(),
                    (true, None) => "Slime chunks on, but the seed is unknown: type /map seed NUMBER.".to_owned(),
                    (false, _) => "Slime chunks off.".to_owned(),
                }
            }
            Some("export") => self.belumi_export_map(),
            _ => "/map seed NUMBER | grid | slime | export".to_owned(),
        };
        self.native_chat(&reply);
    }

    fn belumi_export_map(&mut self) -> String {
        let Some(regions) = self.belumi_maps.regions.as_mut() else {
            return "Nothing explored yet.".to_owned();
        };
        regions.load_all();
        let Some((x0, z0, x1, z1)) = regions.bounds() else {
            return "Nothing explored yet.".to_owned();
        };
        let tiles = regions.snapshot(x0, z0, x1, z1);
        let (w, h) = (f64::from((x1 - x0 + 1) * 16), f64::from((z1 - z0 + 1) * 16));
        let span = w.max(h);
        let bpp = (span / 4096.0).ceil().max(1.0);
        let size = ((span / bpp).ceil() as usize).clamp(16, 4096);
        let (cx, cz) = (f64::from(x0 * 16) + w / 2.0, f64::from(z0 * 16) + h / 2.0);
        let mut image = Vec::new();
        paint_sized(&mut image, size, cx, cz, bpp, &tiles, self.belumi_map_overlay());
        let dir = Self::belumi_dir().join("map-exports");
        let _ = std::fs::create_dir_all(&dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let path = dir.join(format!("{}-{}-{stamp}.png", safe_name(&self.belumi_scope()), self.dim.id()));
        let written = std::fs::File::create(&path).ok().and_then(|f| {
            let mut enc = png::Encoder::new(std::io::BufWriter::new(f), size as u32, size as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().ok()?.write_image_data(&image).ok()
        });
        match written {
            Some(()) => format!("Saved {} ({size} x {size}, {bpp} block(s) per pixel).", path.display()),
            None => "Could not write the map image.".to_owned(),
        }
    }

    /// A waypoint where the player died, replacing the last one.
    pub(super) fn belumi_death_waypoint(&mut self) {
        let dead = self.screen == super::Screen::Dead;
        if !dead {
            self.belumi_maps.died = false;
            return;
        }
        if self.belumi_maps.died || !self.settings.belumi.waypoints {
            return;
        }
        self.belumi_maps.died = true;
        let Some(p) = self.native_player() else {
            return;
        };
        let scope = self.belumi_scope();
        let at = [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32];
        let store = self.belumi_waypoint_store();
        store.remove(&scope, "Death");
        let _ = store.add(&scope, p.dimension, "Death", at);
        self.belumi_save_waypoints();
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
        self.belumi_maps.regions = None;
        self.belumi_maps.caves.clear();
        self.belumi_maps.layer = None;
        self.belumi_maps.painted = None;
        self.belumi_maps.painter.reset();
        self.belumi_maps.centre = None;
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
            .join(safe_name(dim))
    }

    fn belumi_save_map(&mut self, now: bool) {
        let maps = self.belumi_maps.regions.iter_mut().chain(self.belumi_maps.caves.values_mut());
        for regions in maps {
            if now {
                regions.save_now();
            } else {
                regions.save_in_background();
            }
        }
    }

    /// The height band (`y >> 4`) of the player's head.
    fn belumi_head_band(&self) -> Option<i32> {
        let p = self.player.as_ref()?;
        Some(((p.position.y + f64::from(p.dimensions.eye_height)).floor() as i32) >> 4)
    }

    /// The layer the world map shows: the one picked, else where the player is.
    fn belumi_view_layer(&self) -> Layer {
        self.belumi_maps.layer.unwrap_or_else(|| match self.belumi_head_band() {
            Some(band) if self.map_in_cave() => Layer::Cave(band),
            _ => Layer::Surface,
        })
    }

    pub(super) fn belumi_explore(&mut self, dt: f32) {
        if !self.settings.belumi.worldmap || self.player.is_none() || self.belumi_replay_active() {
            return;
        }
        let key = (self.belumi_scope(), self.dim.id());
        if self.belumi_maps.loaded_for.as_ref() != Some(&key) {
            self.belumi_save_map(true);
            self.belumi_maps.caves.clear();
            self.belumi_maps.layer = None;
            self.belumi_maps.painted = None;
            self.belumi_maps.regions = Some(RegionMap::open(Self::belumi_map_path(&key.0, key.1)));
            self.belumi_maps.painter.reset();
            self.belumi_maps.centre = None;
            self.belumi_maps.loaded_for = Some(key.clone());
        }
        let scanned = self.extras.engine.take_scanned();
        let open = self.belumi_maps.open;
        let at = self
            .player
            .as_ref()
            .map(|p| ((p.position.x.floor() as i32) >> 4, (p.position.z.floor() as i32) >> 4));
        let caves = self.extras.engine.take_scanned_cave();
        let base = Self::belumi_map_path(&key.0, key.1);
        for (chunk, tile, head) in caves {
            let band = head >> 4;
            self.belumi_maps
                .caves
                .entry(band)
                .or_insert_with(|| RegionMap::open(base.join(format!("cave-{band}"))))
                .absorb(vec![(chunk, tile)]);
        }
        if let Some(regions) = self.belumi_maps.regions.as_mut() {
            regions.absorb(scanned);
        }
        if let Some(at) = at
            && !open
        {
            for regions in self.belumi_maps.regions.iter_mut().chain(self.belumi_maps.caves.values_mut()) {
                regions.trim(at);
            }
        }
        if self.belumi_maps.caves.len() > MAX_CAVE_MAPS
            && let Some(here) = self.belumi_head_band()
        {
            let far = self
                .belumi_maps
                .caves
                .keys()
                .copied()
                .filter(|b| (b - here).abs() > 2)
                .max_by_key(|b| (b - here).abs());
            if let Some(far) = far
                && let Some(mut gone) = self.belumi_maps.caves.remove(&far)
            {
                gone.save_now();
            }
        }
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
        self.belumi_add_waypoint_at(name, None)
    }

    /// A waypoint at the player, or at (x, z) on the map with the height the map knows.
    fn belumi_add_waypoint_at(&mut self, name: Option<String>, spot: Option<(f64, f64)>) -> String {
        let Some(p) = self.native_player() else {
            return "No player to place a waypoint for.".into();
        };
        let scope = self.belumi_scope();
        let dim = p.dimension;
        let at = match spot {
            Some((x, z)) => [x.floor() as i32, self.belumi_map_height(x, z).unwrap_or(p.y.floor() as i32), z.floor() as i32],
            None => [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32],
        };
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
            Command::Import(path) => {
                let result = self.belumi_waypoint_store().import_xaero(std::path::Path::new(&path), &scope, dim.unwrap_or("minecraft:overworld"));
                self.belumi_save_waypoints();
                match result {
                    Ok((added, 0)) => format!("Imported {added} waypoints from Xaero."),
                    Ok((added, skipped)) => format!("Imported {added} waypoints from Xaero, {skipped} were already here."),
                    Err(e) => e,
                }
            }
            Command::Bad => "Usage: /waypoint add [NAME] | remove NAME | list | clear | import XAERO_FOLDER_OR_FILE".into(),
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

    pub(super) fn belumi_visible_waypoints(&mut self) -> Vec<Waypoint> {
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
            let at = self.belumi_maps.open.then_some(self.belumi_maps.centre).flatten();
            let reply = self.belumi_add_waypoint_at(None, at);
            self.native_chat(&reply);
        }
        let cfg = &self.settings.belumi;
        if cfg.worldmap && key == cfg.worldmap_key && self.native_playing() {
            self.belumi_maps.open = !self.belumi_maps.open;
            self.belumi_maps.layer = None;
            self.belumi_maps.centre = None;
            self.belumi_maps.last_draw = None;
        } else if key == "key.keyboard.escape" {
            self.belumi_maps.open = false;
        } else if self.belumi_maps.open && matches!(key, "key.keyboard.page.up" | "key.keyboard.page.down" | "key.keyboard.home" | "key.keyboard.end") {
            let now = match self.belumi_view_layer() {
                Layer::Cave(b) => b,
                Layer::Surface => self.belumi_head_band().unwrap_or(4),
            };
            self.belumi_maps.layer = match key {
                "key.keyboard.page.up" => Some(Layer::Cave(now + 1)),
                "key.keyboard.page.down" => Some(Layer::Cave(now - 1)),
                "key.keyboard.home" => Some(Layer::Surface),
                _ => None,
            };
        } else if self.belumi_maps.open && key == "key.keyboard.c" {
            self.belumi_maps.centre = None;
        } else if self.belumi_maps.open && key == "key.keyboard.enter" {
            self.belumi_map_teleport();
        }
    }

    pub(super) fn belumi_map_wheel(&mut self, steps: f32, claimed: &mut bool) {
        if !self.belumi_maps.open || *claimed {
            return;
        }
        self.belumi_maps.zoom = zoom_step(self.belumi_maps.zoom, -steps);
        *claimed = true;
    }

    /// The height of the surface the map knows at (x, z), if it has seen it.
    fn belumi_map_height(&mut self, x: f64, z: f64) -> Option<i32> {
        let (bx, bz) = (x.floor() as i32, z.floor() as i32);
        let chunk = (bx >> 4, bz >> 4);
        let regions = self.belumi_maps.regions.as_mut()?;
        let tiles = regions.snapshot(chunk.0, chunk.1, chunk.0, chunk.1);
        let t = tiles.get(&chunk)?;
        let i = ((bz & 15) * 16 + (bx & 15)) as usize;
        (t.color[i] & 0x7F != 0).then(|| i32::from(t.height[i]) + 1)
    }

    fn belumi_map_teleport(&mut self) {
        let Some((x, z)) = self.belumi_maps.centre else {
            self.native_chat("Move the map to a place first (hold the left button and drag).");
            return;
        };
        let Some(y) = self.belumi_map_height(x, z) else {
            self.native_chat("The map has not seen that place yet.");
            return;
        };
        let command = format!("/tp @s {:.1} {} {:.1}", x.floor() + 0.5, y, z.floor() + 0.5);
        self.belumi_maps.open = false;
        self.handle_chat_input(&command);
    }

    pub(super) fn belumi_maps_open(&self) -> bool {
        self.belumi_maps.open
    }

    fn extras_left_held(&self) -> bool {
        self.left_button_held()
    }

    /// Mouse movement while the map is open and the left button is held pans it.
    pub(super) fn belumi_map_drag(&mut self, xo: f64, yo: f64) {
        if !self.belumi_maps.open || !self.extras_left_held() {
            return;
        }
        let bpp = f64::from(MAP_ZOOMS[self.belumi_maps.zoom]);
        let Some(p) = self.native_player() else {
            return;
        };
        let (cx, cz) = self.belumi_maps.centre.unwrap_or((p.x, p.z));
        self.belumi_maps.centre = Some((cx - xo * bpp, cz - yo * bpp));
    }

    pub(super) fn belumi_draw_worldmap(&mut self, batch: &mut HudBatch) {
        if !self.belumi_maps.open || !self.settings.belumi.worldmap {
            return;
        }
        if self.screen != super::Screen::Playing {
            self.belumi_maps.open = false;
            return;
        }
        let Some(p) = self.native_player() else {
            return;
        };
        let icon_rows = self.map_icons_wanted().then(|| self.map_icon_rows());
        let atlas = &self.gui.atlas;
        let Some(sprite) = atlas.sprites.get(WORLDMAP_SPRITE).copied() else {
            return;
        };
        let bpp = MAP_ZOOMS[self.belumi_maps.zoom];
        let now = Instant::now();
        let dt = self
            .belumi_maps
            .last_draw
            .map_or(0.0, |t| now.duration_since(t).as_secs_f64().min(0.1));
        self.belumi_maps.last_draw = Some(now);
        let pan = self.belumi_pan_keys();
        if pan != (0.0, 0.0) {
            let speed = 600.0 * f64::from(bpp) * dt;
            let (cx, cz) = self.belumi_maps.centre.unwrap_or((p.x, p.z));
            self.belumi_maps.centre = Some((cx + pan.0 * speed, cz + pan.1 * speed));
        }
        let (mx, mz) = self.belumi_maps.centre.unwrap_or((p.x, p.z));
        let view = self.belumi_view_layer();
        let overlay = self.belumi_map_overlay();
        let tiles_here;
        {
            let MapState { regions, caves, painter, loaded_for, painted, .. } = &mut self.belumi_maps;
            if *painted != Some(view) {
                painter.reset();
                *painted = Some(view);
            }
            let map = match view {
                Layer::Surface => regions.as_mut(),
                Layer::Cave(band) => loaded_for.as_ref().map(|(scope, dim)| {
                    caves
                        .entry(band)
                        .or_insert_with(|| RegionMap::open(Self::belumi_map_path(scope, dim).join(format!("cave-{band}"))))
                }),
            };
            tiles_here = map.as_ref().map_or(0, |r| r.tile_count());
            if let Some(regions) = map {
                let changed = regions.take_changed();
                painter.want(mx, mz, bpp, changed, overlay, |(x0, z0, x1, z1)| {
                    regions.snapshot(
                        (x0.floor() as i32) >> 4,
                        (z0.floor() as i32) >> 4,
                        (x1.floor() as i32) >> 4,
                        (z1.floor() as i32) >> 4,
                    )
                });
            }
        }
        if let Some(done) = self.belumi_maps.painter.take() {
            let (w, h) = (atlas.width as f32, atlas.height as f32);
            self.map_patches.push(horizon_client::font::Patch {
                x: (sprite.u0 * w).round() as u32,
                y: (sprite.v0 * h).round() as u32,
                width: WORLDMAP_SIZE as u32,
                height: WORLDMAP_SIZE as u32,
                rgba: done.image,
            });
        }
        let side = (batch.height - 24.0).min(batch.width - 24.0).max(64.0);
        let (x, y) = ((batch.width - side) / 2.0, (batch.height - side) / 2.0);
        batch.fill(&self.gui.atlas, 0.0, 0.0, batch.width, batch.height, rgba(0, 0, 0, 150));
        let scale = side / WORLD_WINDOW as f32;
        if let Some((cx, cz, cb)) = self.belumi_maps.painter.shown() {
            let canvas = WORLDMAP_SIZE as f64;
            let texel = |wx: f64, wz: f64| {
                (
                    ((wx - cx) / f64::from(cb) + canvas / 2.0) / canvas,
                    ((wz - cz) / f64::from(cb) + canvas / 2.0) / canvas,
                )
            };
            let half = WORLD_WINDOW as f64 / 2.0 * f64::from(bpp);
            let (u0, v0) = texel(mx - half, mz - half);
            let (u1, v1) = texel(mx + half, mz + half);
            let span = (sprite.u1 - sprite.u0, sprite.v1 - sprite.v0);
            let r = horizon_client::hud::Rect {
                u0: sprite.u0 + span.0 * u0 as f32,
                u1: sprite.u0 + span.0 * u1 as f32,
                v0: sprite.v0 + span.1 * v0 as f32,
                v1: sprite.v0 + span.1 * v1 as f32,
                ..sprite
            };
            batch.rect_uv(x, y, side, side, r, u32::MAX);
        }
        let to_screen = |wx: f64, wz: f64| {
            (
                x + side / 2.0 + ((wx - mx) as f32 / bpp) * scale,
                y + side / 2.0 + ((wz - mz) as f32 / bpp) * scale,
            )
        };
        let (pwx, pwz) = to_screen(p.x, p.z);
        let (px, py) = (pwx, pwz);
        if let Some(r) = self.gui.atlas.sprites.get("map/decorations/player").copied() {
            let (s, c) = (p.yaw + 180.0 + 180.0).to_radians().sin_cos();
            let q = |dx: f32, dy: f32| [px + dx * c - dy * s, py + dx * s + dy * c];
            batch.quad(
                [q(-4.0, 4.0), q(4.0, 4.0), q(4.0, -4.0), q(-4.0, -4.0)],
                [[r.u0, r.v0], [r.u1, r.v0], [r.u1, r.v1], [r.u0, r.v1]],
                u32::MAX,
            );
        }
        if let Some(rows) = &icon_rows {
            for row in rows {
                let (sx, sy) = to_screen(row.pos[0], row.pos[2]);
                if sx < x + 6.0 || sy < y + 6.0 || sx > x + side - 6.0 || sy > y + side - 6.0 {
                    continue;
                }
                self.draw_map_icon(batch, row, sx, sy, 10.0);
            }
        } else if self.settings.belumi.map_entities
            && let Some(remote) = self.remote.as_ref()
        {
            for (is_player, pos, kind) in remote.map_entities() {
                let Some(color) = super::map_entities::color_of(kind, is_player) else {
                    continue;
                };
                let (sx, sy) = to_screen(pos[0], pos[2]);
                if sx < x + 2.0 || sy < y + 2.0 || sx > x + side - 2.0 || sy > y + side - 2.0 {
                    continue;
                }
                batch.fill(&self.gui.atlas, sx - 2.5, sy - 2.5, 5.0, 5.0, 0xFF00_0000);
                batch.fill(&self.gui.atlas, sx - 2.0, sy - 2.0, 4.0, 4.0, color);
            }
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
        let (cx, cy) = (x + side / 2.0, y + side / 2.0);
        batch.fill(&self.gui.atlas, cx - 5.0, cy - 0.5, 4.0, 1.0, 0xFFFF_FFFF);
        batch.fill(&self.gui.atlas, cx + 1.0, cy - 0.5, 4.0, 1.0, 0xFFFF_FFFF);
        batch.fill(&self.gui.atlas, cx - 0.5, cy - 5.0, 1.0, 4.0, 0xFFFF_FFFF);
        batch.fill(&self.gui.atlas, cx - 0.5, cy + 1.0, 1.0, 4.0, 0xFFFF_FFFF);
        let layer_name = match view {
            Layer::Surface => "surface".to_owned(),
            Layer::Cave(b) => format!("below, y {} to {}", b * 16, b * 16 + 15),
        };
        let title = format!(
            "World map  {:.0} {:.0}  {} blocks/px  ({layer_name})",
            mx, mz, bpp
        );
        self.native_text(batch, &title, [x + 4.0, y + 4.0], 0x00FF_FFFF, true);
        let help = "Drag: left button  |  Arrows/WASD: move  |  Wheel: zoom  |  C: centre  |  PgUp/PgDn: layer  |  Home: surface  |  End: follow  |  B: waypoint  |  Enter: teleport  |  M: close";
        self.native_text(batch, help, [x + 4.0, y + side - 12.0], 0x00CC_CCCC, true);
        if tiles_here == 0 {
            self.native_text(batch, "Explore to fill the map.", [cx - 50.0, cy + 14.0], 0x00AA_AAAA, true);
        }
        let _ = REGION;
    }

    /// The map's pan direction from the movement keys and arrows.
    fn belumi_pan_keys(&self) -> (f64, f64) {
        use winit::keyboard::KeyCode as K;
        let held = |a: K, b: K| self.keys.contains(&a) || self.keys.contains(&b);
        let axis = |neg: bool, pos: bool| f64::from(i8::from(pos) - i8::from(neg));
        (
            axis(held(K::KeyA, K::ArrowLeft), held(K::KeyD, K::ArrowRight)),
            axis(held(K::KeyW, K::ArrowUp), held(K::KeyS, K::ArrowDown)),
        )
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
        assert_eq!(zoom_step(7, 1.0), 7);
        assert_eq!(safe_name("server:play.example.org:25565"), "server_play.example.org_25565");
        assert!(!safe_name("world:../../x").contains('/'));
    }
}
