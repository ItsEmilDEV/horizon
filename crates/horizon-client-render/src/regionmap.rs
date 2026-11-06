//! The world map's memory of explored terrain: tiles kept per region of
//! 32x32 chunks, one file per region, loaded when the map needs them and
//! saved in the background when they changed. Far regions are dropped from
//! memory (the files keep them).
use crate::{
    fast_hash::{FxHashMap, FxHashSet},
    minimap::Tile,
};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Chunks per region side.
pub const REGION: i32 = 32;
const MAGIC: &[u8; 4] = b"BRM2";
const RECORD: usize = 2 + 256 + 512 + 256 + 1024;
const MAX_RAW: u64 = 64 * 1024 * 1024;
/// Regions kept in memory, farthest first to go.
const KEEP: usize = 96;
/// Region numbers beyond this are not worlds, whatever a file is called.
const MAX_REGION: i32 = 1 << 20;

struct Region {
    tiles: FxHashMap<(i32, i32), Arc<Tile>>,
    dirty: bool,
}

pub struct RegionMap {
    dir: PathBuf,
    regions: FxHashMap<(i32, i32), Region>,
    absent: FxHashSet<(i32, i32)>,
    changed: bool,
}

pub fn region_of(chunk: (i32, i32)) -> (i32, i32) {
    (chunk.0.div_euclid(REGION), chunk.1.div_euclid(REGION))
}

fn file(dir: &Path, region: (i32, i32)) -> PathBuf {
    dir.join(format!("r.{}.{}.bin", region.0, region.1))
}

pub fn encode_region(tiles: &[((i32, i32), Arc<Tile>)]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(tiles.len() * RECORD);
    for ((cx, cz), t) in tiles {
        let index = (cz.rem_euclid(REGION) * REGION + cx.rem_euclid(REGION)) as u16;
        raw.extend_from_slice(&index.to_le_bytes());
        raw.extend_from_slice(&t.color);
        for h in t.height {
            raw.extend_from_slice(&h.to_le_bytes());
        }
        raw.extend_from_slice(&t.depth);
        for c in t.rgb {
            raw.extend_from_slice(&c.to_le_bytes());
        }
    }
    let mut out = MAGIC.to_vec();
    let mut z = ZlibEncoder::new(&mut out, Compression::fast());
    let _ = z.write_all(&raw);
    let _ = z.finish();
    out
}

pub fn decode_region(bytes: &[u8], region: (i32, i32)) -> Vec<((i32, i32), Tile)> {
    let Some(body) = bytes.strip_prefix(MAGIC.as_slice()) else {
        return Vec::new();
    };
    let mut raw = Vec::new();
    if ZlibDecoder::new(body).take(MAX_RAW).read_to_end(&mut raw).is_err() {
        return Vec::new();
    }
    raw.as_chunks::<RECORD>()
        .0
        .iter()
        .filter_map(|rec| {
            let index = i32::from(u16::from_le_bytes([rec[0], rec[1]]));
            if index >= REGION * REGION {
                return None;
            }
            let key = (
                region.0 * REGION + index % REGION,
                region.1 * REGION + index / REGION,
            );
            let mut color = [0u8; 256];
            color.copy_from_slice(&rec[2..258]);
            let mut height = [0i16; 256];
            for (i, h) in height.iter_mut().enumerate() {
                let o = 258 + i * 2;
                *h = i16::from_le_bytes([rec[o], rec[o + 1]]);
            }
            let mut depth = [0u8; 256];
            depth.copy_from_slice(&rec[770..1026]);
            let mut tile = Tile::from_columns(color, height, depth);
            for (i, c) in tile.rgb.iter_mut().enumerate() {
                let o = 1026 + i * 4;
                *c = u32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
            }
            Some((key, tile))
        })
        .collect()
}

static WRITE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static WRITTEN: std::sync::Mutex<Option<FxHashMap<PathBuf, u64>>> = std::sync::Mutex::new(None);

/// A ticket for one save; a write older than one already done to the same file is dropped.
fn write_ticket() -> u64 {
    WRITE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

fn write_region(dir: &Path, region: (i32, i32), tiles: &[((i32, i32), Arc<Tile>)], ticket: u64) {
    let _ = fs::create_dir_all(dir);
    let path = file(dir, region);
    let bytes = encode_region(tiles);
    let Ok(mut written) = WRITTEN.lock() else {
        return;
    };
    let last = written.get_or_insert_with(FxHashMap::default);
    if last.get(&path).is_some_and(|&t| t > ticket) {
        return;
    }
    let tmp = path.with_extension(format!("{ticket}.tmp"));
    if fs::write(&tmp, bytes).is_ok() && fs::rename(&tmp, &path).is_ok() {
        last.insert(path, ticket);
    } else {
        let _ = fs::remove_file(&tmp);
    }
}

impl RegionMap {
    pub fn open(dir: PathBuf) -> Self {
        Self {
            dir,
            regions: FxHashMap::default(),
            absent: FxHashSet::default(),
            changed: false,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether tiles changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn is_empty(&self) -> bool {
        self.regions.values().all(|r| r.tiles.is_empty()) && self.absent.is_empty()
            && !self.dir.is_dir()
    }

    pub fn tile_count(&self) -> usize {
        self.regions.values().map(|r| r.tiles.len()).sum()
    }

    fn load(&mut self, region: (i32, i32)) {
        if self.regions.contains_key(&region) || self.absent.contains(&region) {
            return;
        }
        match fs::read(file(&self.dir, region)) {
            Ok(bytes) => {
                let tiles: FxHashMap<_, _> = decode_region(&bytes, region)
                    .into_iter()
                    .map(|(k, t)| (k, Arc::new(t)))
                    .collect();
                self.regions.insert(region, Region { tiles, dirty: false });
                self.changed = true;
            }
            Err(_) => {
                self.absent.insert(region);
            }
        }
    }

    /// Read every region file of this map into memory (an export wants all of it).
    pub fn load_all(&mut self) {
        let names: Vec<(i32, i32)> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let rest = name.strip_prefix("r.")?.strip_suffix(".bin")?;
                let (x, z) = rest.split_once('.')?;
                let (x, z): (i32, i32) = (x.parse().ok()?, z.parse().ok()?);
                (x.abs() <= MAX_REGION && z.abs() <= MAX_REGION).then_some((x, z))
            })
            .collect();
        for region in names {
            self.load(region);
        }
    }

    /// The chunk range `(x0, z0, x1, z1)` of everything held, if anything is.
    pub fn bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut it = self.regions.values().flat_map(|r| r.tiles.keys());
        let first = it.next()?;
        Some(it.fold((first.0, first.1, first.0, first.1), |b, k| {
            (b.0.min(k.0), b.1.min(k.1), b.2.max(k.0), b.3.max(k.1))
        }))
    }

    /// Take in scanned tiles (the surface, or a cave floor for a cave map); only a changed tile marks its region.
    pub fn absorb(&mut self, scanned: Vec<((i32, i32), Tile)>) {
        for (key, tile) in scanned {
            let tile = tile.into_stored();
            let region = region_of(key);
            self.load(region);
            self.absent.remove(&region);
            let r = self.regions.entry(region).or_insert_with(|| Region {
                tiles: FxHashMap::default(),
                dirty: false,
            });
            let same = r.tiles.get(&key).is_some_and(|t| {
                t.color == tile.color
                    && t.height == tile.height
                    && t.depth == tile.depth
                    && t.rgb == tile.rgb
            });
            if !same {
                r.tiles.insert(key, Arc::new(tile));
                r.dirty = true;
                self.changed = true;
            }
        }
    }

    /// The tiles of every chunk in `[x0, x1] x [z0, z1]` (chunk coordinates), loading regions as needed.
    pub fn snapshot(&mut self, x0: i32, z0: i32, x1: i32, z1: i32) -> FxHashMap<(i32, i32), Arc<Tile>> {
        let (a, b) = (region_of((x0, z0)), region_of((x1, z1)));
        let mut out = FxHashMap::default();
        for rz in a.1..=b.1 {
            for rx in a.0..=b.0 {
                self.load((rx, rz));
                if let Some(r) = self.regions.get(&(rx, rz)) {
                    for (k, t) in &r.tiles {
                        if (x0..=x1).contains(&k.0) && (z0..=z1).contains(&k.1) {
                            out.insert(*k, t.clone());
                        }
                    }
                }
            }
        }
        out
    }

    /// Write the changed regions in the background.
    pub fn save_in_background(&mut self) {
        let jobs = self.take_dirty();
        if jobs.is_empty() {
            return;
        }
        let dir = self.dir.clone();
        let ticket = write_ticket();
        std::thread::spawn(move || {
            for (region, tiles) in jobs {
                write_region(&dir, region, &tiles, ticket);
            }
        });
    }

    /// Write the changed regions now (leaving the world).
    pub fn save_now(&mut self) {
        let ticket = write_ticket();
        for (region, tiles) in self.take_dirty() {
            write_region(&self.dir, region, &tiles, ticket);
        }
    }

    fn take_dirty(&mut self) -> Vec<((i32, i32), Vec<((i32, i32), Arc<Tile>)>)> {
        self.regions
            .iter_mut()
            .filter(|(_, r)| r.dirty)
            .map(|(k, r)| {
                r.dirty = false;
                (*k, r.tiles.iter().map(|(c, t)| (*c, t.clone())).collect())
            })
            .collect()
    }

    /// Drop the regions farthest from `centre` (a chunk) once more than `KEEP` are loaded.
    pub fn trim(&mut self, centre: (i32, i32)) {
        if self.regions.len() <= KEEP {
            return;
        }
        let here = region_of(centre);
        let mut far: Vec<_> = self
            .regions
            .iter()
            .filter(|(_, r)| !r.dirty)
            .map(|(k, _)| (((k.0 - here.0).pow(2) + (k.1 - here.1).pow(2)), *k))
            .collect();
        far.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
        for (_, k) in far.into_iter().take(self.regions.len() - KEEP) {
            self.regions.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("belumi-regions-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tile(color: u8, rgb: u32) -> Tile {
        let mut t = Tile::from_columns([color; 256], [70; 256], [2; 256]);
        t.rgb = [rgb; 256];
        t
    }

    #[test]
    fn a_region_round_trips_through_its_file_format() {
        let a = Arc::new(tile(3, 0x112233));
        let b = Arc::new(tile(9, 0));
        let bytes = encode_region(&[((-1, 0), a), ((31, 31), b)]);
        let mut tiles = decode_region(&bytes, (-1, 0));
        tiles.sort_by_key(|(k, _)| *k);
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].0, (-1, 0));
        assert_eq!(tiles[0].1.rgb[5], 0x112233);
        assert_eq!(tiles[0].1.color[0], 3);
        assert_eq!(tiles[1].0, (-1 * 32 + 31, 31));
        assert!(decode_region(b"nope", (0, 0)).is_empty());
    }

    #[test]
    fn bounds_and_load_all_cover_everything_saved() {
        let dir = Temp::new("bounds");
        let mut map = RegionMap::open(dir.path().to_path_buf());
        assert_eq!(map.bounds(), None);
        map.absorb(vec![((-3, 5), tile(3, 1)), ((70, -40), tile(4, 2))]);
        map.save_now();
        let mut again = RegionMap::open(dir.path().to_path_buf());
        assert_eq!(again.bounds(), None);
        again.load_all();
        assert_eq!(again.bounds(), Some((-3, -40, 70, 5)));
    }

    #[test]
    fn regions_follow_negative_chunks() {
        assert_eq!(region_of((0, 0)), (0, 0));
        assert_eq!(region_of((31, 32)), (0, 1));
        assert_eq!(region_of((-1, -32)), (-1, -1));
        assert_eq!(region_of((-33, 0)), (-2, 0));
    }

    #[test]
    fn explored_tiles_are_saved_per_region_and_loaded_again() {
        let dir = Temp::new("saved");
        let mut map = RegionMap::open(dir.path().to_path_buf());
        map.absorb(vec![((1, 1), tile(3, 0xAABBCC)), ((40, 1), tile(4, 0x010203))]);
        assert!(map.take_changed());
        assert_eq!(map.tile_count(), 2);
        map.save_now();
        assert!(dir.path().join("r.0.0.bin").is_file());
        assert!(dir.path().join("r.1.0.bin").is_file());
        let mut again = RegionMap::open(dir.path().to_path_buf());
        let snap = again.snapshot(0, 0, 63, 63);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[&(1, 1)].rgb[0], 0xAABBCC);
        let near = again.snapshot(0, 0, 10, 10);
        assert_eq!(near.len(), 1);
    }

    #[test]
    fn an_unchanged_tile_does_not_dirty_its_region() {
        let dir = Temp::new("unchanged");
        let mut map = RegionMap::open(dir.path().to_path_buf());
        map.absorb(vec![((0, 0), tile(3, 1))]);
        map.save_now();
        map.take_changed();
        map.absorb(vec![((0, 0), tile(3, 1))]);
        assert!(!map.take_changed());
        map.absorb(vec![((0, 0), tile(3, 2))]);
        assert!(map.take_changed());
    }

    #[test]
    fn far_clean_regions_are_dropped_from_memory() {
        let dir = Temp::new("trim");
        let mut map = RegionMap::open(dir.path().to_path_buf());
        for i in 0..(KEEP as i32 + 10) {
            map.absorb(vec![((i * REGION, 0), tile(3, 1))]);
        }
        map.save_now();
        map.trim((0, 0));
        assert_eq!(map.regions.len(), KEEP);
        assert!(map.regions.contains_key(&(0, 0)));
        assert!(!map.regions.contains_key(&(KEEP as i32 + 9, 0)));
    }
}
