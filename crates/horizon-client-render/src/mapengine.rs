//! The map engine behind the minimap: worker threads scan chunk snapshots
//! into tiles and paint a north-up canvas around the player; the game thread
//! only schedules, and draws the canvas turned and shifted by the GPU, so the
//! map scrolls and rotates smoothly without recomposing every frame.
//!
//! A chunk is rescanned when its sections are no longer the ones the tile was
//! scanned from (an edit copies the section it touches), not on a global
//! revision.
use crate::{
    blocks::BlockFacts,
    fast_hash::{FxHashMap, FxHashSet},
    minimap::{
        Layer, Palette, Snapshot, Tile, UNKNOWN, WALL, brightness, packed_rgba, scan_tile,
    },
    world::World,
};
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The canvas side in texels.
pub const CANVAS: usize = 256;
/// The side of the window the map shows of it, in texels.
pub const WINDOW: usize = 128;
/// The canvas is painted again when the player is this far (texels) from its centre.
const RECENTER: f64 = 20.0;
/// Painting again for changed tiles waits at least this long.
const REPAINT: Duration = Duration::from_millis(120);
const MAX_PENDING: usize = 24;
const PER_UPDATE: usize = 8;
const WORKERS: usize = 2;
/// A cave tile whose head height moved this far is scanned again.
const HEAD_STEP: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Surface,
    Cave,
}

enum Job {
    Scan {
        generation: u64,
        key: (i32, i32),
        kind: Kind,
        snap: Snapshot,
        layer: Layer,
        head: i32,
        air: u32,
    },
    Paint {
        generation: u64,
        cx: f64,
        cz: f64,
        bpp: f32,
        cave: bool,
        tiles: FxHashMap<(i32, i32), Arc<Tile>>,
    },
}
enum Done {
    Scan {
        generation: u64,
        key: (i32, i32),
        kind: Kind,
        tile: Tile,
        snap: Snapshot,
        head: i32,
    },
    Painted(Canvas),
}

/// A painted canvas: its centre in the world, the blocks one texel covers, and RGBA.
pub struct Canvas {
    pub image: Vec<u8>,
    pub cx: f64,
    pub cz: f64,
    pub bpp: f32,
    pub cave: bool,
    generation: u64,
}

struct Slot {
    tile: Arc<Tile>,
    snap: Snapshot,
    head: i32,
}

/// What the minimap wants painted: the player's place, the zoom and whether it shows caves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Want {
    pub x: f64,
    pub z: f64,
    pub bpp: f32,
    pub cave: bool,
}

pub struct MapEngine {
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    workers: Vec<JoinHandle<()>>,
    palette: Arc<Mutex<Option<Arc<Palette>>>>,
    surface: FxHashMap<(i32, i32), Slot>,
    cave: FxHashMap<(i32, i32), Slot>,
    pending: FxHashSet<(Kind, (i32, i32))>,
    dirty: bool,
    scanned: Vec<((i32, i32), Tile)>,
    painting: bool,
    generation: u64,
    requested: Option<Want>,
    last_paint: Instant,
    ready: Option<Canvas>,
    shown: Option<(f64, f64, f32, bool)>,
    /// Keep the surface tiles scanned for `take_scanned` (the world map wants them).
    pub record: bool,
}

impl Default for MapEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MapEngine {
    pub fn new() -> Self {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel::<Done>();
        let rx = Arc::new(Mutex::new(rx));
        let palette: Arc<Mutex<Option<Arc<Palette>>>> = Arc::new(Mutex::new(None));
        let workers = (0..WORKERS)
            .map(|i| {
                let (rx, tx, palette) = (rx.clone(), tx.clone(), palette.clone());
                std::thread::Builder::new()
                    .name(format!("belumi-map-{i}"))
                    .spawn(move || worker(&rx, &tx, &palette))
                    .expect("map worker")
            })
            .collect();
        Self {
            jobs: Some(jobs),
            done,
            workers,
            palette,
            surface: FxHashMap::default(),
            cave: FxHashMap::default(),
            pending: FxHashSet::default(),
            dirty: false,
            scanned: Vec::new(),
            painting: false,
            generation: 0,
            requested: None,
            last_paint: Instant::now(),
            ready: None,
            shown: None,
            record: false,
        }
    }

    /// Forget everything (a new world or dimension, or the map turned off).
    pub fn clear(&mut self) {
        self.surface.clear();
        self.cave.clear();
        self.pending.clear();
        self.dirty = false;
        self.scanned.clear();
        self.requested = None;
        self.ready = None;
        self.shown = None;
        self.generation += 1;
        self.painting = false;
    }

    pub fn tile_count(&self) -> usize {
        self.surface.len() + self.cave.len()
    }

    /// The surface tiles scanned since the last call (what the world map keeps).
    pub fn take_scanned(&mut self) -> Vec<((i32, i32), Tile)> {
        std::mem::take(&mut self.scanned)
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.jobs {
            let _ = tx.send(job);
        }
    }

    fn collect(&mut self) {
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Scan {
                    generation,
                    key,
                    kind,
                    tile,
                    snap,
                    head,
                } => {
                    if generation != self.generation {
                        continue;
                    }
                    self.pending.remove(&(kind, key));
                    let map = match kind {
                        Kind::Surface => &mut self.surface,
                        Kind::Cave => &mut self.cave,
                    };
                    let same = map.get(&key).is_some_and(|s| {
                        s.tile.color == tile.color
                            && s.tile.height == tile.height
                            && s.tile.depth == tile.depth
                    });
                    if !same {
                        self.dirty = true;
                    }
                    if self.record && kind == Kind::Surface && !same {
                        self.scanned.push((key, tile.clone()));
                    }
                    map.insert(
                        key,
                        Slot {
                            tile: Arc::new(tile),
                            snap,
                            head,
                        },
                    );
                }
                Done::Painted(canvas) => {
                    if canvas.generation == self.generation {
                        self.ready = Some(canvas);
                    }
                    self.painting = false;
                }
            }
        }
    }

    /// Take in finished work and send the chunks around `center` that are new or
    /// changed to the workers, nearest first. `surface_radius` is in chunks;
    /// `cave` is the cave radius and the head height when the map shows below.
    pub fn update(
        &mut self,
        world: &World,
        facts: &BlockFacts,
        center: (i32, i32),
        surface_radius: i32,
        cave: Option<(i32, i32)>,
    ) {
        self.collect();
        if let Ok(mut p) = self.palette.lock() {
            p.get_or_insert_with(|| Arc::new(Palette::new(facts)));
        }
        let air = world.air.0;
        let reach = |k: &(i32, i32), r: i32| {
            (k.0 - center.0).abs() <= r + 2 && (k.1 - center.1).abs() <= r + 2
        };
        self.surface
            .retain(|k, _| reach(k, surface_radius) && world.chunks.contains_key(k));
        match cave {
            Some((radius, _)) => self
                .cave
                .retain(|k, _| reach(k, radius) && world.chunks.contains_key(k)),
            None => self.cave.clear(),
        }
        let mut sent = 0;
        let mut plan = vec![(Kind::Surface, surface_radius, 0)];
        if let Some((radius, head)) = cave {
            plan.insert(0, (Kind::Cave, radius, head));
        }
        for (kind, radius, head) in plan {
            for ring in 0..=radius {
                for (dx, dz) in ring_offsets(ring) {
                    if sent >= PER_UPDATE || self.pending.len() >= MAX_PENDING {
                        return;
                    }
                    let key = (center.0 + dx, center.1 + dz);
                    let Some(chunk) = world.chunks.get(&key) else {
                        continue;
                    };
                    if self.pending.contains(&(kind, key)) {
                        continue;
                    }
                    let map = match kind {
                        Kind::Surface => &self.surface,
                        Kind::Cave => &self.cave,
                    };
                    let stale = match map.get(&key) {
                        None => true,
                        Some(slot) => {
                            !slot.snap.matches(chunk)
                                || (kind == Kind::Cave && (slot.head - head).abs() >= HEAD_STEP)
                        }
                    };
                    if !stale {
                        continue;
                    }
                    self.pending.insert((kind, key));
                    sent += 1;
                    let layer = match kind {
                        Kind::Surface => Layer::Surface,
                        Kind::Cave => Layer::Below(head),
                    };
                    self.send(Job::Scan {
                        generation: self.generation,
                        key,
                        kind,
                        snap: Snapshot::of(chunk),
                        layer,
                        head,
                        air,
                    });
                }
            }
        }
    }

    /// Whether a canvas painted for `shown` no longer serves `want`.
    pub fn canvas_stale(shown: &Want, want: &Want) -> bool {
        want.bpp != shown.bpp
            || want.cave != shown.cave
            || (want.x - shown.x).abs() > RECENTER * f64::from(want.bpp)
            || (want.z - shown.z).abs() > RECENTER * f64::from(want.bpp)
    }

    /// Ask for a canvas for `want` if the last one no longer fits or tiles changed.
    pub fn want_canvas(&mut self, want: Want) {
        self.collect();
        if self.painting {
            return;
        }
        let stale = match &self.requested {
            None => true,
            Some(r) => {
                Self::canvas_stale(r, &want)
                    || (self.dirty && self.last_paint.elapsed() >= REPAINT)
            }
        };
        if !stale {
            return;
        }
        let map = if want.cave { &self.cave } else { &self.surface };
        let half = (CANVAS as f64 / 2.0) * f64::from(want.bpp);
        let snap8 = |v: f64| (v / 8.0).round() * 8.0;
        let (cx, cz) = (snap8(want.x), snap8(want.z));
        let tiles: FxHashMap<(i32, i32), Arc<Tile>> = map
            .iter()
            .filter(|(k, _)| {
                let (bx, bz) = (f64::from(k.0 * 16) + 8.0, f64::from(k.1 * 16) + 8.0);
                (bx - cx).abs() <= half + 16.0 && (bz - cz).abs() <= half + 16.0
            })
            .map(|(k, s)| (*k, s.tile.clone()))
            .collect();
        self.painting = true;
        self.dirty = false;
        self.last_paint = Instant::now();
        self.requested = Some(Want {
            x: cx,
            z: cz,
            ..want
        });
        self.send(Job::Paint {
            generation: self.generation,
            cx,
            cz,
            bpp: want.bpp,
            cave: want.cave,
            tiles,
        });
    }

    /// The newest painted canvas, to upload; from now on `shown` is its place.
    pub fn take_canvas(&mut self) -> Option<Canvas> {
        self.collect();
        let canvas = self.ready.take()?;
        self.shown = Some((canvas.cx, canvas.cz, canvas.bpp, canvas.cave));
        Some(canvas)
    }

    /// The centre, texel size and cave flag of the canvas that was uploaded last.
    pub fn shown(&self) -> Option<(f64, f64, f32, bool)> {
        self.shown
    }
}

impl Drop for MapEngine {
    fn drop(&mut self) {
        self.jobs = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(rx: &Mutex<Receiver<Job>>, tx: &Sender<Done>, shared: &Mutex<Option<Arc<Palette>>>) {
    loop {
        let job = match rx.lock() {
            Ok(rx) => rx.recv(),
            Err(_) => return,
        };
        let Ok(job) = job else { return };
        let done = match job {
            Job::Scan {
                generation,
                key,
                kind,
                snap,
                layer,
                head,
                air,
            } => {
                let Some(palette) = shared.lock().ok().and_then(|p| p.clone()) else {
                    continue;
                };
                let tile = scan_tile(&snap, &palette, air, layer, 0);
                Done::Scan {
                    generation,
                    key,
                    kind,
                    tile,
                    snap,
                    head,
                }
            }
            Job::Paint {
                generation,
                cx,
                cz,
                bpp,
                cave,
                tiles,
            } => {
                let mut image = Vec::new();
                paint_canvas(&mut image, cx, cz, f64::from(bpp), &tiles);
                Done::Painted(Canvas {
                    image,
                    cx,
                    cz,
                    bpp,
                    cave,
                    generation,
                })
            }
        };
        if tx.send(done).is_err() {
            return;
        }
    }
}

struct Lookup<'a> {
    tiles: &'a FxHashMap<(i32, i32), Arc<Tile>>,
    last: Option<((i32, i32), &'a Tile)>,
}
impl<'a> Lookup<'a> {
    fn get(&mut self, x: i32, z: i32) -> Option<(u8, i16, u8)> {
        let key = (x >> 4, z >> 4);
        let tile = match self.last {
            Some((k, t)) if k == key => t,
            _ => {
                let t = &**self.tiles.get(&key)?;
                self.last = Some((key, t));
                t
            }
        };
        let i = ((z & 15) * 16 + (x & 15)) as usize;
        Some((tile.color[i], tile.height[i], tile.depth[i]))
    }
}

/// North-up canvas centred on (`cx`, `cz`), each texel `bpp` blocks across.
pub fn paint_canvas(
    image: &mut Vec<u8>,
    cx: f64,
    cz: f64,
    bpp: f64,
    tiles: &FxHashMap<(i32, i32), Arc<Tile>>,
) {
    image.resize(CANVAS * CANVAS * 4, 0);
    let mut lookup = Lookup { tiles, last: None };
    let half = CANVAS as f64 / 2.0;
    for j in 0..CANVAS {
        let wz = cz + (j as f64 + 0.5 - half) * bpp;
        let z = wz.floor() as i32;
        for i in 0..CANVAS {
            let wx = cx + (i as f64 + 0.5 - half) * bpp;
            let x = wx.floor() as i32;
            let o = (j * CANVAS + i) * 4;
            let rgba = match lookup.get(x, z) {
                Some((color, height, depth)) if color & !WALL != 0 => {
                    let north = lookup.get(x, z - 1).filter(|c| c.0 & !WALL != 0).map(|c| c.1);
                    let b = brightness(color, height, north, depth, x + z);
                    packed_rgba(color & !WALL, b)
                }
                _ => UNKNOWN,
            };
            image[o..o + 4].copy_from_slice(&rgba);
        }
    }
}

/// Chunk offsets at Chebyshev distance `ring`.
fn ring_offsets(ring: i32) -> impl Iterator<Item = (i32, i32)> {
    (-ring..=ring).flat_map(move |dz| {
        (-ring..=ring)
            .filter(move |&dx| dx.abs() == ring || dz.abs() == ring)
            .map(move |dx| (dx, dz))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(color: u8, height: i16) -> Arc<Tile> {
        Arc::new(Tile::from_columns([color; 256], [height; 256], [0; 256]))
    }

    #[test]
    fn a_canvas_is_painted_from_the_tiles_under_it() {
        let mut tiles = FxHashMap::default();
        tiles.insert((0, 0), tile(1, 64));
        let mut image = Vec::new();
        paint_canvas(&mut image, 8.0, 8.0, 1.0, &tiles);
        assert_eq!(image.len(), CANVAS * CANVAS * 4);
        let centre = (CANVAS / 2 * CANVAS + CANVAS / 2) * 4;
        assert_eq!(image[centre + 3], 255);
        assert_ne!(&image[centre..centre + 4], &UNKNOWN);
        assert_eq!(&image[0..4], &UNKNOWN);
    }

    #[test]
    fn zoomed_out_texels_cover_more_blocks() {
        let mut tiles = FxHashMap::default();
        for x in -3..3 {
            for z in -3..3 {
                tiles.insert((x, z), tile(1, 64));
            }
        }
        let mut near = Vec::new();
        let mut far = Vec::new();
        paint_canvas(&mut near, 0.0, 0.0, 0.5, &tiles);
        paint_canvas(&mut far, 0.0, 0.0, 2.0, &tiles);
        let at = (CANVAS / 2 * CANVAS + CANVAS / 2 + 40) * 4;
        assert_ne!(&near[at..at + 4], &UNKNOWN);
        assert_eq!(&far[at..at + 4], &UNKNOWN);
    }

    #[test]
    fn the_canvas_stays_until_the_player_nears_its_edge_or_the_zoom_changes() {
        let shown = Want { x: 0.0, z: 0.0, bpp: 1.0, cave: false };
        let near = Want { x: 15.0, z: -10.0, ..shown };
        assert!(!MapEngine::canvas_stale(&shown, &near));
        let far = Want { x: 25.0, ..shown };
        assert!(MapEngine::canvas_stale(&shown, &far));
        assert!(MapEngine::canvas_stale(&shown, &Want { bpp: 2.0, ..shown }));
        assert!(MapEngine::canvas_stale(&shown, &Want { cave: true, ..shown }));
        let zoomed = Want { bpp: 4.0, ..shown };
        assert!(!MapEngine::canvas_stale(&zoomed, &Want { x: 70.0, ..zoomed }));
    }

    #[test]
    fn a_worker_paints_a_canvas_and_the_engine_hands_it_over_once() {
        let mut engine = MapEngine::new();
        engine.want_canvas(Want { x: 3.0, z: 5.0, bpp: 1.0, cave: false });
        let mut canvas = None;
        for _ in 0..200 {
            canvas = engine.take_canvas();
            if canvas.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let canvas = canvas.expect("a painted canvas");
        assert_eq!(canvas.image.len(), CANVAS * CANVAS * 4);
        assert_eq!((canvas.cx, canvas.cz), (0.0, 8.0));
        assert!(engine.take_canvas().is_none());
        assert_eq!(engine.shown(), Some((0.0, 8.0, 1.0, false)));
        engine.want_canvas(Want { x: 3.0, z: 5.0, bpp: 1.0, cave: false });
        assert!(engine.take_canvas().is_none());
    }
}
