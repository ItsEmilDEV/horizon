//! The blueprint: a `.schem` or `.litematic` plan from the game folder's `schematics/` shown in the world as outlines where
//! blocks are missing (cyan) or wrong (red), with a material list. `/blueprint list|load NAME|place|here|move X Y Z|layer N|off`.
use super::{Game, belumi_client::{Act, Page}};
use horizon_client::{
    blueprint::{Blueprint, Status, Turn, status},
    gizmos::Line,
    hud::HudBatch,
};
use std::{path::PathBuf, sync::Arc, time::{Duration, Instant}};

const SCAN_RADIUS: i32 = 28;
const MAX_BOXES: usize = 900;
const EVERY: Duration = Duration::from_millis(500);
const FULL_SCAN_LIMIT: usize = 500_000;
const MAX_FILE: u64 = 64 << 20;
const MISSING: u32 = 0xFF55_D8FF;
const WRONG: u32 = 0xFFFF_4040;

struct Loaded {
    name: String,
    plan: Arc<Blueprint>,
    /// The world position of the plan's cell (0, 0, 0), once placed.
    origin: Option<[i32; 3]>,
    /// Only this level (counted from the plan's bottom) is shown, when set.
    layer: Option<i32>,
}

/// What a scan found: outlines to draw and the counts in the scanned part.
#[derive(Default)]
struct Scan {
    boxes: Vec<(Status, [i32; 3])>,
    done: u32,
    missing: u32,
    wrong: u32,
}

#[derive(Default)]
pub(super) struct Plans {
    loaded: Option<Loaded>,
    scan: Scan,
    scanned: Option<Instant>,
    /// A full pass over the plan, for the page: placed, missing, wrong, or none when the plan is too big for one.
    totals: Option<(u32, u32, u32)>,
    totals_at: Option<Instant>,
}

fn dir() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join("schematics")
}

/// The plans in a folder: file names with a `.schem` or `.litematic` ending.
pub(super) fn list_plans(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".schem") || n.ends_with(".litematic"))
        .collect();
    names.sort();
    names
}

/// The file a name means: exactly, or with one of the two endings.
pub(super) fn find_plan(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return None;
    }
    [name.to_owned(), format!("{name}.schem"), format!("{name}.litematic")].into_iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// `/blueprint` arguments.
#[derive(Debug, PartialEq)]
pub(super) enum Cmd {
    Page,
    List,
    Load(String),
    Place,
    Here,
    Move([i32; 3]),
    Layer(Option<i32>),
    Turn(horizon_client::blueprint::Turn),
    Off,
}

pub(super) fn parse(args: &str) -> Option<Cmd> {
    let mut it = args.split_whitespace();
    let Some(head) = it.next() else { return Some(Cmd::Page) };
    let rest: Vec<&str> = it.collect();
    match (head.to_ascii_lowercase().as_str(), rest.as_slice()) {
        ("list", []) => Some(Cmd::List),
        ("load", [name @ ..]) if !name.is_empty() => Some(Cmd::Load(name.join(" "))),
        ("place", []) => Some(Cmd::Place),
        ("here", []) => Some(Cmd::Here),
        ("off", []) => Some(Cmd::Off),
        ("rotate", []) => Some(Cmd::Turn(Turn::Rotate)),
        ("mirror", ["x"]) => Some(Cmd::Turn(Turn::MirrorX)),
        ("mirror", ["z"]) => Some(Cmd::Turn(Turn::MirrorZ)),
        ("move", [x, y, z]) => Some(Cmd::Move([x.parse().ok()?, y.parse().ok()?, z.parse().ok()?])),
        ("layer", ["all"]) => Some(Cmd::Layer(None)),
        ("layer", [n]) => Some(Cmd::Layer(Some(n.parse().ok().filter(|n| *n >= 0)?))),
        _ => None,
    }
}

impl Game<'_> {
    fn blueprint_world_status(&self, wanted: &horizon_client::blueprint::Spec, x: i32, y: i32, z: i32) -> Status {
        let id = self.world.block(x, y, z);
        let facts = self.res.facts();
        let air = facts.is_air(id) || facts.get(id).replaceable;
        let state = self.pack.assets.registry.state(id);
        status(wanted, air, state.map(|s| (s.name.as_str(), &s.properties)))
    }

    /// Once a frame: the outlines near the player, twice a second.
    pub(super) fn belumi_blueprint_frame(&mut self) {
        let Some(l) = self.belumi_plans.loaded.as_ref() else { return };
        let Some(origin) = l.origin else { return };
        if self.belumi_plans.scanned.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.belumi_plans.scanned = Some(Instant::now());
        let Some(p) = self.player.as_ref() else { return };
        let (px, py, pz) = (p.position.x.floor() as i32, p.position.y.floor() as i32, p.position.z.floor() as i32);
        let (plan, layer) = (l.plan.clone(), l.layer);
        let [sx, sy, sz] = plan.size;
        // The part of the plan within reach of the player.
        let lo = [(px - SCAN_RADIUS - origin[0]).max(0), (py - SCAN_RADIUS - origin[1]).max(0), (pz - SCAN_RADIUS - origin[2]).max(0)];
        let hi = [(px + SCAN_RADIUS - origin[0]).min(sx - 1), (py + SCAN_RADIUS - origin[1]).min(sy - 1), (pz + SCAN_RADIUS - origin[2]).min(sz - 1)];
        let mut scan = Scan::default();
        let mut found: Vec<(i64, Status, [i32; 3])> = Vec::new();
        for y in lo[1]..=hi[1] {
            if layer.is_some_and(|l| l != y) {
                continue;
            }
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    let Some(spec) = plan.spec_at(x, y, z) else { continue };
                    if spec.is_air() {
                        continue;
                    }
                    let w = [origin[0] + x, origin[1] + y, origin[2] + z];
                    if !self.world.chunks.contains_key(&(w[0] >> 4, w[2] >> 4)) {
                        continue;
                    }
                    match self.blueprint_world_status(spec, w[0], w[1], w[2]) {
                        Status::Done => scan.done += 1,
                        s @ (Status::Missing | Status::Wrong) => {
                            if s == Status::Missing {
                                scan.missing += 1;
                            } else {
                                scan.wrong += 1;
                            }
                            let d = i64::from((w[0] - px).pow(2) + (w[1] - py).pow(2) + (w[2] - pz).pow(2));
                            found.push((d, s, w));
                        }
                        Status::Free => {}
                    }
                }
            }
        }
        found.sort_by_key(|f| f.0);
        found.truncate(MAX_BOXES);
        scan.boxes = found.into_iter().map(|(_, s, w)| (s, w)).collect();
        self.belumi_plans.scan = scan;
    }

    /// The outlines as lines for the world's renderer.
    pub(super) fn belumi_blueprint_lines(&self) -> Vec<Line> {
        let mut g = horizon_client::gizmos::Gizmos::default();
        for (status, p) in &self.belumi_plans.scan.boxes {
            let (inset, color, width) = if *status == Status::Missing { (0.14, MISSING, 1.5) } else { (-0.01, WRONG, 2.5) };
            let min = [f64::from(p[0]) + inset, f64::from(p[1]) + inset, f64::from(p[2]) + inset];
            let max = [f64::from(p[0] + 1) - inset, f64::from(p[1] + 1) - inset, f64::from(p[2] + 1) - inset];
            g.cuboid(min, max, color, width);
        }
        g.lines
    }

    /// The whole plan against the world, for the page (not for a huge plan).
    fn blueprint_totals(&mut self) {
        if self.belumi_plans.totals_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.belumi_plans.totals_at = Some(Instant::now());
        let Some(l) = self.belumi_plans.loaded.as_ref() else { return };
        let (Some(origin), plan) = (l.origin, l.plan.clone()) else {
            self.belumi_plans.totals = None;
            return;
        };
        if plan.cells.len() > FULL_SCAN_LIMIT {
            self.belumi_plans.totals = None;
            return;
        }
        let [sx, sy, sz] = plan.size;
        let (mut done, mut missing, mut wrong) = (0, 0, 0);
        for y in 0..sy {
            for z in 0..sz {
                for x in 0..sx {
                    let Some(spec) = plan.spec_at(x, y, z) else { continue };
                    if spec.is_air() {
                        continue;
                    }
                    let w = [origin[0] + x, origin[1] + y, origin[2] + z];
                    if !self.world.chunks.contains_key(&(w[0] >> 4, w[2] >> 4)) {
                        continue;
                    }
                    match self.blueprint_world_status(spec, w[0], w[1], w[2]) {
                        Status::Done => done += 1,
                        Status::Missing => missing += 1,
                        Status::Wrong => wrong += 1,
                        Status::Free => {}
                    }
                }
            }
        }
        self.belumi_plans.totals = Some((done, missing, wrong));
    }

    /// How many of an item the player carries (the inventory and the off hand).
    fn blueprint_have(&self, item: &str) -> u32 {
        self.inventory.iter().flatten().chain(self.offhand.iter()).filter(|s| &*s.item == item).map(|s| s.count).sum()
    }

    pub(super) fn belumi_blueprint_command(&mut self, command: &str, ran: &mut bool) {
        let (head, rest) = match command.trim().split_once(char::is_whitespace) {
            Some((h, r)) => (h, r),
            None => (command.trim(), ""),
        };
        if *ran || !(head.eq_ignore_ascii_case("blueprint") || head.eq_ignore_ascii_case("bp")) {
            return;
        }
        *ran = true;
        let Some(cmd) = parse(rest) else {
            self.native_chat("Usage: /blueprint list | load NAME | place | here | move X Y Z | rotate | mirror x|z | layer N|all | off");
            return;
        };
        match cmd {
            Cmd::Page => self.client_open_page(Page::Blueprint),
            Cmd::List => {
                let names = list_plans(&dir());
                if names.is_empty() {
                    self.native_chat(&format!("No plans yet: put .schem or .litematic files in {}.", dir().display()));
                } else {
                    self.native_chat(&format!("Plans: {}", names.join(", ")));
                }
            }
            Cmd::Load(name) => self.blueprint_load(&name),
            Cmd::Place => {
                let Some(hit) = self.pick() else {
                    self.native_chat("Aim at the block next to which the plan's corner goes, then /blueprint place.");
                    return;
                };
                let at = [hit.pos[0] + hit.face[0], hit.pos[1] + hit.face[1], hit.pos[2] + hit.face[2]];
                self.blueprint_set_origin(at);
            }
            Cmd::Here => {
                let Some(p) = self.player.as_ref() else { return };
                let at = [p.position.x.floor() as i32, p.position.y.floor() as i32, p.position.z.floor() as i32];
                self.blueprint_set_origin(at);
            }
            Cmd::Move([dx, dy, dz]) => {
                let moved = self.belumi_plans.loaded.as_mut().and_then(|l| l.origin.as_mut()).map(|o| {
                    *o = [o[0] + dx, o[1] + dy, o[2] + dz];
                    *o
                });
                match moved {
                    Some(o) => {
                        self.belumi_plans.scanned = None;
                        self.native_chat(&format!("The plan's corner is at {} {} {}.", o[0], o[1], o[2]));
                    }
                    None => self.native_chat("Place a plan first (/blueprint place)."),
                }
            }
            Cmd::Layer(layer) => {
                if let Some(l) = self.belumi_plans.loaded.as_mut() {
                    l.layer = layer;
                    self.belumi_plans.scanned = None;
                    self.native_chat(&layer.map_or("Showing every level.".to_owned(), |n| format!("Showing level {n} (0 is the bottom).")));
                }
            }
            Cmd::Turn(turn) => match self.belumi_plans.loaded.as_mut() {
                Some(l) => {
                    l.plan = Arc::new(l.plan.turned(turn));
                    let size = l.plan.size;
                    self.belumi_plans.scanned = None;
                    self.belumi_plans.totals_at = None;
                    self.native_chat(&format!("The plan is {} x {} x {} now; its corner stays where it was.", size[0], size[1], size[2]));
                }
                None => self.native_chat("Load a plan first: /blueprint load NAME."),
            },
            Cmd::Off => {
                self.belumi_plans = Plans::default();
                self.native_chat("The plan is hidden.");
            }
        }
    }

    fn blueprint_set_origin(&mut self, at: [i32; 3]) {
        let Some(l) = self.belumi_plans.loaded.as_mut() else {
            self.native_chat("Load a plan first: /blueprint load NAME.");
            return;
        };
        l.origin = Some(at);
        self.belumi_plans.scanned = None;
        self.belumi_plans.totals_at = None;
        self.native_chat(&format!("The plan's corner is at {} {} {}. Cyan outlines are missing blocks, red ones are wrong.", at[0], at[1], at[2]));
    }

    fn blueprint_load(&mut self, name: &str) {
        let Some(path) = find_plan(&dir(), name) else {
            self.native_chat(&format!("No plan called {name} in {}. /blueprint list shows them.", dir().display()));
            return;
        };
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
            self.native_chat("That file is too big to be a plan.");
            return;
        }
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| Blueprint::read(&b).map_err(|e| e.to_string())) {
            Ok(plan) => {
                let blocks: u32 = plan.materials().iter().map(|(_, n)| n).sum();
                self.native_chat(&format!(
                    "Loaded {name}: {} x {} x {}, {blocks} blocks. Aim at a block and type /blueprint place (or /blueprint here).",
                    plan.size[0], plan.size[1], plan.size[2]
                ));
                self.belumi_plans = Plans { loaded: Some(Loaded { name: name.to_owned(), plan: Arc::new(plan), origin: None, layer: None }), ..Plans::default() };
            }
            Err(e) => self.native_chat(&format!("Could not read {name}: {e}")),
        }
    }

    pub(super) fn blueprint_act(&mut self, act: &Act) -> bool {
        let Act::BlueprintLayer(step) = act else { return false };
        if let Some(l) = self.belumi_plans.loaded.as_mut() {
            let top = l.plan.size[1] - 1;
            l.layer = match (l.layer, *step) {
                (_, 0) => None,
                (None, s) if s > 0 => Some(0),
                (None, _) => Some(top),
                (Some(n), s) => Some((n + s).clamp(0, top)),
            };
            self.belumi_plans.scanned = None;
        }
        true
    }

    pub(super) fn draw_blueprint_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        self.blueprint_totals();
        let pw = 380.0_f32.min(w - 12.0);
        let ph = (h - 16.0).clamp(200.0, 270.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Blueprint");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let mut ry = y + 28.0;
        let Some(l) = self.belumi_plans.loaded.as_ref() else {
            self.native_text(batch, "No plan loaded. Put .schem or .litematic files in schematics/ of the", [x + 10.0, ry], 0x008A_93A8, false);
            self.native_text(batch, "game folder, then /blueprint load NAME and /blueprint place.", [x + 10.0, ry + 12.0], 0x008A_93A8, false);
            return;
        };
        let (name, size, origin, layer, plan) = (l.name.clone(), l.plan.size, l.origin, l.layer, l.plan.clone());
        self.native_text(batch, &format!("{name}  {} x {} x {}", size[0], size[1], size[2]), [x + 10.0, ry], 0x00FF_FFFF, false);
        ry += 12.0;
        let place = origin.map_or("not placed: aim at a block, /blueprint place".to_owned(), |o| format!("corner at {} {} {}", o[0], o[1], o[2]));
        self.native_text(batch, &place, [x + 10.0, ry], 0x008A_93A8, false);
        ry += 12.0;
        if let Some((done, missing, wrong)) = self.belumi_plans.totals {
            self.native_text(batch, &format!("built {done}, missing {missing}, wrong {wrong}"), [x + 10.0, ry], 0x004D_FF6A, false);
        }
        ry += 14.0;
        let level = layer.map_or("all levels".to_owned(), |n| format!("level {n}"));
        self.client_button(batch, [x + 10.0, ry, 18.0, 16.0], "-", None, Act::BlueprintLayer(-1));
        self.native_text(batch, &level, [x + 34.0, ry + 4.0], 0x00FF_FFFF, true);
        self.client_button(batch, [x + 110.0, ry, 18.0, 16.0], "+", None, Act::BlueprintLayer(1));
        self.client_button(batch, [x + 134.0, ry, 40.0, 16.0], "All", None, Act::BlueprintLayer(0));
        ry += 24.0;
        self.native_text(batch, "Materials (need / you carry)", [x + 10.0, ry], 0x008A_93A8, false);
        ry += 12.0;
        for (item, need) in plan.materials().into_iter().take(((y + ph - ry - 8.0) / 11.0).max(1.0) as usize) {
            let have = self.blueprint_have(&item);
            let color = if have >= need { 0x004D_FF6A } else { 0x00FF_E14D };
            let short = item.strip_prefix("minecraft:").unwrap_or(&item).replace('_', " ");
            self.native_text(batch, &format!("{short}: {need}  ({have})"), [x + 14.0, ry], color, false);
            ry += 11.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_read() {
        assert_eq!(parse(""), Some(Cmd::Page));
        assert_eq!(parse("list"), Some(Cmd::List));
        assert_eq!(parse("load my castle"), Some(Cmd::Load("my castle".into())));
        assert_eq!(parse("place"), Some(Cmd::Place));
        assert_eq!(parse("move 1 -2 3"), Some(Cmd::Move([1, -2, 3])));
        assert_eq!(parse("layer 4"), Some(Cmd::Layer(Some(4))));
        assert_eq!(parse("layer all"), Some(Cmd::Layer(None)));
        assert_eq!(parse("rotate"), Some(Cmd::Turn(Turn::Rotate)));
        assert_eq!(parse("mirror x"), Some(Cmd::Turn(Turn::MirrorX)));
        assert_eq!(parse("mirror z"), Some(Cmd::Turn(Turn::MirrorZ)));
        assert_eq!(parse("mirror y"), None);
        for bad in ["load", "move 1 2", "move a b c", "layer -1", "layer x", "place now", "fly"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_plan_is_found_by_name_with_or_without_its_ending_and_never_outside_the_folder() {
        let dir = std::env::temp_dir().join(format!("belumi-bp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("castle.schem"), b"x").unwrap();
        std::fs::write(dir.join("tower.litematic"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(list_plans(&dir), vec!["castle.schem".to_owned(), "tower.litematic".to_owned()]);
        assert!(find_plan(&dir, "castle").is_some() && find_plan(&dir, "castle.schem").is_some() && find_plan(&dir, "tower").is_some());
        assert!(find_plan(&dir, "../castle").is_none() && find_plan(&dir, "").is_none() && find_plan(&dir, "nothing").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
