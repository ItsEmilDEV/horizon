//! "What changed since my last visit?": when the server sends a column that the cache held (a ghost), the old and the new
//! blocks are compared. Changes you watched yourself (block updates while you were there) are remembered per column in a
//! `X.Z.seen` file next to the cached column and do not count. What is left is shown in chat once, on the Changes page, and
//! can be turned into a waypoint.
use super::{Game, belumi_client::{Act, Page}};
use horizon_client::{
    chunk::Chunk,
    fast_hash::{FxHashMap, FxHashSet},
    hud::HudBatch,
};
use std::time::{Duration, Instant};

/// A column with fewer changed blocks than this is noise (a flowing fluid, a growing crop).
const MIN_BLOCKS: usize = 4;
const SAMPLES: usize = 3;
const SEEN_CAP: usize = 4096;
const LISTED: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChunkChange {
    pub(super) chunk: (i32, i32),
    pub(super) blocks: usize,
    pub(super) samples: Vec<[i32; 3]>,
}

#[derive(Default)]
pub(super) struct Changes {
    /// Block positions the server changed while the player watched, per column.
    seen: FxHashMap<(i32, i32), FxHashSet<(i32, i32, i32)>>,
    dirty: FxHashSet<(i32, i32)>,
    found: Vec<ChunkChange>,
    compared: usize,
    announced: usize,
    last_found: Option<Instant>,
    last_flush: Option<Instant>,
}

/// Names of blocks that change by themselves all the time.
fn noisy_name(name: &str) -> bool {
    ["water", "lava", "fire", "leaves", "snow", "kelp", "seagrass", "vine", "bubble", "torch", "sapling"].iter().any(|n| name.contains(n))
}

/// The blocks that differ between two versions of a column: how many (not counting `skip` positions and pairs that
/// `noisy` calls noise) and the first few.
pub(super) fn diff_chunks(
    old: &Chunk,
    new: &Chunk,
    min_y: i32,
    skip: &dyn Fn((i32, i32, i32)) -> bool,
    noisy: &dyn Fn(u32, u32) -> bool,
) -> (usize, Vec<[i32; 3]>) {
    let (mut count, mut samples) = (0, Vec::new());
    let (mut a, mut b) = ([0u32; 4096], [0u32; 4096]);
    for (i, (so, sn)) in old.sections.iter().zip(&new.sections).enumerate() {
        if std::sync::Arc::ptr_eq(so, sn) {
            continue;
        }
        so.unpack(&mut a);
        sn.unpack(&mut b);
        for k in 0..4096 {
            if a[k] == b[k] || noisy(a[k], b[k]) {
                continue;
            }
            let pos = (new.x * 16 + (k & 15) as i32, min_y + i as i32 * 16 + (k >> 8) as i32, new.z * 16 + ((k >> 4) & 15) as i32);
            if skip(pos) {
                continue;
            }
            count += 1;
            if samples.len() < SAMPLES {
                samples.push([pos.0, pos.1, pos.2]);
            }
        }
    }
    (count, samples)
}

pub(super) fn seen_text(set: &FxHashSet<(i32, i32, i32)>) -> String {
    let mut lines: Vec<String> = set.iter().map(|p| format!("{} {} {}", p.0, p.1, p.2)).collect();
    lines.sort();
    lines.join("\n")
}

pub(super) fn parse_seen(text: &str) -> FxHashSet<(i32, i32, i32)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace().map(str::parse::<i32>);
            Some((it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?))
        })
        .take(SEEN_CAP)
        .collect()
}

impl Game<'_> {
    fn belumi_changes_on(&self) -> bool {
        self.belumi_distant_on() && self.modules_ref().is_none_or(|m| m.misc.changes)
    }

    fn seen_file(&self, x: i32, z: i32) -> std::path::PathBuf {
        self.belumi_distant_dir().join(format!("{x}.{z}.seen"))
    }

    /// The server changed a block: the column's cached copy is behind now, and nobody should call that a surprise.
    pub(super) fn belumi_changes_seen(&mut self, pos: [i32; 3]) {
        if !self.belumi_changes_on() {
            return;
        }
        let key = (pos[0] >> 4, pos[2] >> 4);
        let set = self.belumi_changes.seen.entry(key).or_default();
        if set.len() < SEEN_CAP && set.insert((pos[0], pos[1], pos[2])) {
            self.belumi_changes.dirty.insert(key);
        }
    }

    pub(super) fn belumi_changes_load_seen(&mut self, x: i32, z: i32) {
        if let Ok(text) = std::fs::read_to_string(self.seen_file(x, z)) {
            self.belumi_changes.seen.entry((x, z)).or_default().extend(parse_seen(&text));
        }
    }

    pub(super) fn belumi_changes_save_seen(&mut self, x: i32, z: i32) {
        self.belumi_changes.dirty.remove(&(x, z));
        let Some(set) = self.belumi_changes.seen.get(&(x, z)).filter(|s| !s.is_empty()) else { return };
        let (path, text) = (self.seen_file(x, z), seen_text(set));
        self.belumi_distant.write(path, text.into_bytes());
    }

    /// A fresh column arrived: when the cache held an older one, compare them.
    pub(super) fn belumi_changes_compare(&mut self, new: &Chunk) {
        let key = (new.x, new.z);
        if !self.belumi_changes_on() {
            return;
        }
        // The old version: a ghost in the world, or the cached file when the server was faster than the loader.
        let old = if self.belumi_distant.is_ghost(key.0, key.1) {
            self.world.chunks.get(&key).cloned()
        } else if !self.world.chunks.contains_key(&key) {
            self.belumi_changes_load_seen(key.0, key.1);
            self.belumi_distant_read(key.0, key.1).map(std::sync::Arc::new)
        } else {
            None
        };
        if let Some(old) = old {
            let seen = self.belumi_changes.seen.get(&key).cloned().unwrap_or_default();
            let facts = self.res.facts();
            let noisy = |a: u32, b: u32| noisy_name(&facts.get(horizon_world::registry::BlockStateId(a)).name) || noisy_name(&facts.get(horizon_world::registry::BlockStateId(b)).name);
            let (blocks, samples) = diff_chunks(&old, new, self.world.layout().min_y(), &|p| seen.contains(&p), &noisy);
            self.belumi_changes.compared += 1;
            if blocks >= MIN_BLOCKS {
                self.belumi_changes.found.retain(|c| c.chunk != key);
                self.belumi_changes.found.push(ChunkChange { chunk: key, blocks, samples });
                self.belumi_changes.last_found = Some(Instant::now());
            }
        }
        // The new version is cached from now on: edits seen before it are part of it.
        if self.belumi_changes.seen.remove(&key).is_some() {
            let path = self.seen_file(key.0, key.1);
            self.belumi_distant.remove(path);
        }
        self.belumi_changes.dirty.remove(&key);
    }

    /// Once a frame: writes the lists of watched edits now and then, and tells the player what changed.
    pub(super) fn belumi_changes_tick(&mut self) {
        if !self.belumi_changes_on() {
            if !self.belumi_changes.found.is_empty() {
                self.belumi_changes = Changes::default();
            }
            return;
        }
        let due = self.belumi_changes.last_flush.is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
        if due {
            self.belumi_changes.last_flush = Some(Instant::now());
            let dirty: Vec<_> = self.belumi_changes.dirty.iter().copied().collect();
            for (x, z) in dirty {
                self.belumi_changes_save_seen(x, z);
            }
        }
        let n = self.belumi_changes.found.len();
        let quiet = self.belumi_changes.last_found.is_some_and(|t| t.elapsed() >= Duration::from_secs(6));
        if n > self.belumi_changes.announced && quiet {
            self.belumi_changes.announced = n;
            let blocks: usize = self.belumi_changes.found.iter().map(|c| c.blocks).sum();
            self.native_chat(&format!("Since your last visit {n} chunk(s) changed ({blocks} blocks). Right Shift > Modules > Changes lists them."));
        }
    }

    /// Writes every watched edit that is not on disk yet (the game is leaving).
    pub(super) fn belumi_changes_flush(&mut self) {
        let dirty: Vec<_> = self.belumi_changes.dirty.iter().copied().collect();
        for (x, z) in dirty {
            self.belumi_changes_save_seen(x, z);
        }
        self.belumi_changes = Changes::default();
    }

    fn belumi_change_mark(&mut self, i: usize) {
        let mut list = self.belumi_changes.found.clone();
        list.sort_by(|a, b| b.blocks.cmp(&a.blocks));
        let Some(c) = list.get(i) else { return };
        let Some(at) = c.samples.first().copied() else { return };
        let scope = self.belumi_scope();
        let dim = self.native_player().map_or("minecraft:overworld", |p| p.dimension);
        let name = format!("Changed {},{}", c.chunk.0, c.chunk.1);
        let store = self.belumi_waypoint_store();
        store.remove(&scope, &name);
        let _ = store.add(&scope, dim, &name, at);
        self.belumi_save_waypoints();
        self.modules().misc.compass_target = name.clone();
        self.modules_changed();
        self.toast(format!("Waypoint {name} set; the compass points at it."));
    }

    pub(super) fn belumi_changes_act(&mut self, act: &Act) -> bool {
        match act {
            Act::ChangeMark(i) => {
                self.belumi_change_mark(*i);
                true
            }
            Act::ChangesClear => {
                self.belumi_changes.found.clear();
                self.belumi_changes.announced = 0;
                true
            }
            _ => false,
        }
    }

    pub(super) fn draw_changes_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let mut list = self.belumi_changes.found.clone();
        list.sort_by(|a, b| b.blocks.cmp(&a.blocks));
        let rows = list.len().clamp(1, LISTED) as f32;
        let pw = 340.0_f32.min(w - 12.0);
        let ph = (66.0 + rows * 22.0).min(h - 16.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Changes since your last visit");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let mut ry = y + 28.0;
        if list.is_empty() {
            self.native_text(batch, "Nothing changed in the chunks you saw before,", [x + 10.0, ry], 0x008A_93A8, false);
            self.native_text(batch, "or you have not been on this server yet.", [x + 10.0, ry + 12.0], 0x008A_93A8, false);
            return;
        }
        let feet = self.player.as_ref().map(|p| (p.position.x, p.position.z));
        for (i, c) in list.iter().take(LISTED).enumerate() {
            let dist = feet.map_or(0.0, |(px, pz)| (f64::from(c.chunk.0 * 16 + 8) - px).hypot(f64::from(c.chunk.1 * 16 + 8) - pz));
            let label = format!("{} blocks  {},{}  {}", c.blocks, c.chunk.0 * 16, c.chunk.1 * 16, horizon_client::waypoints::distance_label(dist));
            self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], &label, None, Act::ChangeMark(i));
            ry += 22.0;
        }
        ry += 4.0;
        self.native_text(batch, "Click one to set a waypoint and point the compass at it.", [x + 10.0, ry], 0x008A_93A8, false);
        self.client_button(batch, [x + pw - 66.0, y + ph - 22.0, 56.0, 14.0], "Clear", None, Act::ChangesClear);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::chunk::Section;
    use std::sync::Arc;

    fn chunk(x: i32, z: i32, edits: &[(usize, usize, u32)]) -> Chunk {
        let mut c = Chunk::empty(x, z);
        for &(section, index, state) in edits {
            let mut s = (*c.sections[section]).clone();
            s.set(index, state);
            c.sections[section] = Arc::new(s);
        }
        c
    }

    #[test]
    fn identical_columns_have_no_changes_and_a_change_is_found_with_its_place() {
        let a = chunk(1, 2, &[]);
        let same = a.clone();
        let (n, _) = diff_chunks(&a, &same, -64, &|_| false, &|_, _| false);
        assert_eq!(n, 0);
        let b = chunk(1, 2, &[(5, super::super::belumi_changes::tests::idx(3, 4, 5), 9)]);
        let (n, samples) = diff_chunks(&a, &b, -64, &|_| false, &|_, _| false);
        assert_eq!(n, 1);
        assert_eq!(samples, vec![[16 + 3, -64 + 5 * 16 + 4, 32 + 5]]);
    }

    pub(super) fn idx(x: usize, y: usize, z: usize) -> usize {
        y << 8 | z << 4 | x
    }

    #[test]
    fn watched_edits_and_noisy_pairs_do_not_count() {
        let a = chunk(0, 0, &[]);
        let edits: Vec<_> = (0..6).map(|i| (0, idx(i, 0, 0), 7)).collect();
        let b = chunk(0, 0, &edits);
        let (all, _) = diff_chunks(&a, &b, 0, &|_| false, &|_, _| false);
        assert_eq!(all, 6);
        let (skipped, _) = diff_chunks(&a, &b, 0, &|p| p.0 < 3, &|_, _| false);
        assert_eq!(skipped, 3);
        let (noise, _) = diff_chunks(&a, &b, 0, &|_| false, &|_, new| new == 7);
        assert_eq!(noise, 0);
        let (_, samples) = diff_chunks(&a, &b, 0, &|_| false, &|_, _| false);
        assert_eq!(samples.len(), SAMPLES);
    }

    #[test]
    fn the_seen_list_round_trips_and_ignores_garbage() {
        let set: FxHashSet<(i32, i32, i32)> = [(1, 2, 3), (-5, 70, 9)].into_iter().collect();
        assert_eq!(parse_seen(&seen_text(&set)), set);
        assert!(parse_seen("a b c\n1 2\n").is_empty());
    }

    #[test]
    fn water_and_leaves_are_noise() {
        assert!(noisy_name("minecraft:water") && noisy_name("minecraft:oak_leaves") && noisy_name("minecraft:wall_torch"));
        assert!(!noisy_name("minecraft:stone") && !noisy_name("minecraft:chest"));
        let _ = Section::default();
    }
}
