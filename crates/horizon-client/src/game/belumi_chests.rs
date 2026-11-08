//! Chest memory: what every container you opened held, where it is, and when. `/chests diamond` finds it again (also on
//! other servers' worlds, kept apart by world), the page lists the hits and a click sets a waypoint and points the
//! compass at the chest. Only what the server showed you is remembered.
use super::{Game, belumi_client::{Act, Page}};
use horizon_client::hud::HudBatch;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

const FILE: &str = "belumi-chests.json";
const MAX_ENTRIES: usize = 2000;
const LISTED: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Slot {
    pub(super) item: String,
    pub(super) count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Entry {
    pub(super) scope: String,
    pub(super) dimension: String,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) z: i32,
    pub(super) block: String,
    pub(super) at: u64,
    pub(super) items: Vec<Slot>,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default, rename = "chest")]
    entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Hit {
    pub(super) entry: Entry,
    /// What matched the query: item and count.
    pub(super) matching: Vec<Slot>,
}

#[derive(Default)]
pub(super) struct Chests {
    store: Option<Store>,
    dirty: bool,
    last_save: Option<Instant>,
    /// The open container: its menu id, where it is and its slots by position.
    open: Option<(i32, Entry, Vec<Option<Slot>>)>,
    pub(super) query: String,
}

/// `minecraft:diamond_pickaxe` -> `diamond pickaxe`, for matching by what the player types.
pub(super) fn plain(item: &str) -> String {
    item.strip_prefix("minecraft:").unwrap_or(item).replace('_', " ").to_lowercase()
}

/// How many slots a menu's container has, or none when it is not a container worth remembering.
pub(super) fn container_slots(menu: &str) -> Option<usize> {
    match menu {
        "minecraft:shulker_box" => Some(27),
        "minecraft:hopper" => Some(5),
        "minecraft:generic_3x3" => Some(9),
        m => m.strip_prefix("minecraft:generic_9x")?.parse::<usize>().ok().filter(|r| (1..=6).contains(r)).map(|r| r * 9),
    }
}

/// The entries holding something that matches `query`, nearest first when `from` is known.
pub(super) fn search(entries: &[Entry], scope: &str, query: &str, from: Option<[f64; 3]>) -> Vec<Hit> {
    let q = plain(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = entries
        .iter()
        .filter(|e| e.scope == scope)
        .filter_map(|e| {
            let mut matching: Vec<Slot> = Vec::new();
            for s in e.items.iter().filter(|s| plain(&s.item).contains(&q)) {
                match matching.iter_mut().find(|m| m.item == s.item) {
                    Some(m) => m.count += s.count,
                    None => matching.push(s.clone()),
                }
            }
            (!matching.is_empty()).then(|| Hit { entry: e.clone(), matching })
        })
        .collect();
    if let Some(f) = from {
        let d = |h: &Hit| ((f64::from(h.entry.x) - f[0]).powi(2) + (f64::from(h.entry.z) - f[2]).powi(2)).sqrt();
        hits.sort_by(|a, b| d(a).total_cmp(&d(b)));
    }
    hits
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn age_label(secs: u64) -> String {
    match secs {
        0..=89 => "now".to_owned(),
        90..=5399 => format!("{} min ago", secs / 60),
        5400..=129_599 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

impl Game<'_> {
    fn chests_path() -> PathBuf {
        std::env::current_dir().unwrap_or_default().join(FILE)
    }

    fn chests_store(&mut self) -> &mut Store {
        self.belumi_chests.store.get_or_insert_with(|| {
            std::fs::read_to_string(Self::chests_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
        })
    }

    fn chests_save(&mut self) {
        self.belumi_chests.dirty = false;
        self.belumi_chests.last_save = Some(Instant::now());
        if let Some(store) = &self.belumi_chests.store
            && let Ok(text) = serde_json::to_string_pretty(store)
        {
            let _ = std::fs::write(Self::chests_path(), text);
        }
    }

    fn chests_on(&self) -> bool {
        self.remote.is_some() && self.modules_ref().is_none_or(|m| m.misc.chest_memory)
    }

    /// A container's content arrived: remember it under the block that was used.
    pub(super) fn belumi_chests_content(&mut self, container: i32, items: &[Option<horizon_items::items::ItemStack>]) {
        if !self.chests_on() {
            return;
        }
        let Some(r) = self.remote.as_ref() else { return };
        let (Some((id, menu)), Some(pos)) = (r.menu.clone(), r.use_target) else { return };
        let Some(n) = (id == container).then(|| container_slots(&menu)).flatten() else { return };
        let block = self.res.facts().get(self.world.block(pos[0], pos[1], pos[2])).name.to_string();
        let slots: Vec<Option<Slot>> = (0..n)
            .map(|i| items.get(i).and_then(|s| s.as_ref()).map(|s| Slot { item: s.item.to_string(), count: s.count }))
            .collect();
        let entry = Entry {
            scope: self.belumi_scope(),
            dimension: self.native_player().map_or("minecraft:overworld", |p| p.dimension).to_owned(),
            x: pos[0],
            y: pos[1],
            z: pos[2],
            block,
            at: now(),
            items: slots.iter().flatten().cloned().collect(),
        };
        self.belumi_chests.open = Some((container, entry.clone(), slots));
        self.chests_put(entry);
    }

    /// One slot of the open container changed (you moved something in or out).
    pub(super) fn belumi_chests_slot(&mut self, container: i32, slot: i32, item: Option<horizon_items::items::ItemStack>) {
        let Some((id, entry, slots)) = self.belumi_chests.open.as_mut() else { return };
        let Some(cell) = usize::try_from(slot).ok().and_then(|i| slots.get_mut(i)) else { return };
        if *id != container {
            return;
        }
        *cell = item.map(|s| Slot { item: s.item.to_string(), count: s.count });
        entry.items = slots.iter().flatten().cloned().collect();
        entry.at = now();
        let entry = entry.clone();
        self.chests_put(entry);
    }

    fn chests_put(&mut self, entry: Entry) {
        let store = self.chests_store();
        store.entries.retain(|e| !(e.scope == entry.scope && e.dimension == entry.dimension && (e.x, e.y, e.z) == (entry.x, entry.y, entry.z)));
        store.entries.push(entry);
        if store.entries.len() > MAX_ENTRIES {
            store.entries.sort_by_key(|e| std::cmp::Reverse(e.at));
            store.entries.truncate(MAX_ENTRIES);
        }
        self.belumi_chests.dirty = true;
    }

    pub(super) fn belumi_chests_tick(&mut self) {
        let due = self.belumi_chests.last_save.is_none_or(|t| t.elapsed() >= Duration::from_secs(5));
        if self.belumi_chests.dirty && due {
            self.chests_save();
        }
    }

    pub(super) fn belumi_chests_flush(&mut self) {
        self.belumi_chests.open = None;
        if self.belumi_chests.dirty {
            self.chests_save();
        }
    }

    /// The entries that match now: chests that were broken since (the block is air again) are left out.
    fn chests_hits(&mut self) -> Vec<Hit> {
        let scope = self.belumi_scope();
        let query = self.belumi_chests.query.clone();
        let from = self.native_player().map(|p| [p.x, p.y, p.z]);
        let entries = self.chests_store().entries.clone();
        let mut hits = search(&entries, &scope, &query, from);
        hits.retain(|h| {
            let e = &h.entry;
            !(self.world.chunks.contains_key(&(e.x >> 4, e.z >> 4)) && self.res.facts().is_air(self.world.block(e.x, e.y, e.z)))
        });
        hits
    }

    pub(super) fn belumi_chests_command(&mut self, command: &str, ran: &mut bool) {
        let (head, rest) = match command.trim().split_once(char::is_whitespace) {
            Some((h, r)) => (h, r.trim()),
            None => (command.trim(), ""),
        };
        if *ran || !head.eq_ignore_ascii_case("chests") {
            return;
        }
        *ran = true;
        if rest.is_empty() {
            self.native_chat("Usage: /chests TEXT  finds the chests you have opened that hold it (for example /chests diamond).");
            return;
        }
        self.belumi_chests.query = rest.to_owned();
        let hits = self.chests_hits();
        if hits.is_empty() {
            self.native_chat(&format!("No chest you opened held {rest}."));
            return;
        }
        let total: u32 = hits.iter().flat_map(|h| h.matching.iter()).map(|s| s.count).sum();
        self.native_chat(&format!("{} chest(s) hold {total} of {rest}; Right Shift > Modules > Chests shows them.", hits.len()));
        self.client_open_page(Page::Chests);
    }

    fn belumi_chest_mark(&mut self, i: usize) {
        let Some(hit) = self.chests_hits().get(i).cloned() else { return };
        let e = hit.entry;
        let name = format!("Chest {} {}", plain(&hit.matching[0].item), i + 1);
        let name: String = name.chars().take(28).collect();
        let scope = self.belumi_scope();
        let store = self.belumi_waypoint_store();
        store.remove(&scope, &name);
        let _ = store.add(&scope, &e.dimension, &name, [e.x, e.y, e.z]);
        self.belumi_save_waypoints();
        self.modules().misc.compass_target = name.clone();
        self.modules_changed();
        self.toast(format!("Waypoint {name} set; the compass points at it."));
    }

    pub(super) fn belumi_chests_act(&mut self, act: &Act) -> bool {
        if let Act::ChestMark(i) = act {
            self.belumi_chest_mark(*i);
            return true;
        }
        false
    }

    pub(super) fn draw_chests_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let hits = self.chests_hits();
        let rows = hits.len().clamp(1, LISTED) as f32;
        let pw = 360.0_f32.min(w - 12.0);
        let ph = (66.0 + rows * 22.0).min(h - 16.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        let title = if self.belumi_chests.query.is_empty() { "Chests".to_owned() } else { format!("Chests: {}", self.belumi_chests.query) };
        self.panel(batch, [x, y, pw, ph], &title);
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let mut ry = y + 28.0;
        if hits.is_empty() {
            self.native_text(batch, "Type /chests TEXT in chat, for example /chests diamond.", [x + 10.0, ry], 0x008A_93A8, false);
            self.native_text(batch, "The game remembers every container you open.", [x + 10.0, ry + 12.0], 0x008A_93A8, false);
            return;
        }
        let me = self.native_player().map(|p| (p.x, p.z));
        let t = now();
        for (i, hit) in hits.iter().take(LISTED).enumerate() {
            let e = &hit.entry;
            let d = me.map_or(0.0, |(px, pz)| (f64::from(e.x) - px).hypot(f64::from(e.z) - pz));
            let count: u32 = hit.matching.iter().map(|s| s.count).sum();
            let label = format!("{count} x {}  {}  {}", plain(&hit.matching[0].item), horizon_client::waypoints::distance_label(d), age_label(t.saturating_sub(e.at)));
            self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], &label, None, Act::ChestMark(i));
            ry += 22.0;
        }
        ry += 4.0;
        self.native_text(batch, "Click one to set a waypoint and point the compass at it.", [x + 10.0, ry], 0x008A_93A8, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(x: i32, items: &[(&str, u32)]) -> Entry {
        Entry {
            scope: "server:a".into(),
            dimension: "minecraft:overworld".into(),
            x,
            y: 64,
            z: 0,
            block: "minecraft:chest".into(),
            at: 1,
            items: items.iter().map(|(i, c)| Slot { item: (*i).into(), count: *c }).collect(),
        }
    }

    #[test]
    fn menus_map_to_their_container_sizes() {
        assert_eq!(container_slots("minecraft:generic_9x3"), Some(27));
        assert_eq!(container_slots("minecraft:generic_9x6"), Some(54));
        assert_eq!(container_slots("minecraft:shulker_box"), Some(27));
        assert_eq!(container_slots("minecraft:hopper"), Some(5));
        assert_eq!(container_slots("minecraft:crafting"), None);
        assert_eq!(container_slots("minecraft:generic_9x9"), None);
    }

    #[test]
    fn a_search_matches_by_plain_words_sums_counts_and_sorts_by_distance() {
        let list = vec![
            entry(100, &[("minecraft:diamond", 3), ("minecraft:diamond", 4), ("minecraft:stone", 64)]),
            entry(10, &[("minecraft:diamond_pickaxe", 1)]),
            entry(5, &[("minecraft:dirt", 1)]),
        ];
        let hits = search(&list, "server:a", "Diamond", Some([0.0, 64.0, 0.0]));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].entry.x, 10);
        assert_eq!(hits[1].matching, vec![Slot { item: "minecraft:diamond".into(), count: 7 }]);
        assert_eq!(search(&list, "server:a", "diamond pick", None).len(), 1);
        assert!(search(&list, "server:other", "diamond", None).is_empty());
        assert!(search(&list, "server:a", "  ", None).is_empty());
    }

    #[test]
    fn the_store_round_trips_through_toml() {
        let store = Store { entries: vec![entry(1, &[("minecraft:gold_ingot", 12)])] };
        let text = serde_json::to_string_pretty(&store).unwrap();
        let back: Store = serde_json::from_str(&text).unwrap();
        assert_eq!(back.entries, store.entries);
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age_label(10), "now");
        assert_eq!(age_label(600), "10 min ago");
        assert_eq!(age_label(7200), "2 h ago");
        assert_eq!(age_label(3 * 86_400), "3 d ago");
    }
}
