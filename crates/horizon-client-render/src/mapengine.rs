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
        Colorize, Layer, Palette, Snapshot, Tile, UNKNOWN, WALL, brightness, packed_rgba,
        scan_tile_with,
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
        overlay: Overlay,
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
    Failed {
        generation: u64,
        key: (i32, i32),
        kind: Kind,
    },
}

/// What is drawn over the terrain: chunk borders and the chunks slimes spawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overlay {
    pub grid: bool,
    pub slime_seed: Option<i64>,
}

/// Java's `Random.next(bits)` on a 48-bit seed.
fn java_next(seed: &mut i64, bits: u32) -> i32 {
    *seed = (seed.wrapping_mul(0x5_DEEC_E66D).wrapping_add(0xB)) & ((1i64 << 48) - 1);
    (*seed >> (48 - bits)) as i32
}

/// Whether slimes spawn in chunk (`cx`, `cz`) of the world with `seed` (the game's own rule).
pub fn is_slime_chunk(seed: i64, cx: i32, cz: i32) -> bool {
    let mixed = seed
        .wrapping_add(i64::from(cx.wrapping_mul(cx).wrapping_mul(0x4c1906)))
        .wrapping_add(i64::from(cx.wrapping_mul(0x5ac0db)))
        .wrapping_add(i64::from(cz.wrapping_mul(cz)).wrapping_mul(0x4307a7))
        .wrapping_add(i64::from(cz.wrapping_mul(0x5f24f)))
        ^ 0x3ad8_025f;
    let mut state = (mixed ^ 0x5_DEEC_E66D) & ((1i64 << 48) - 1);
    loop {
        let bits = java_next(&mut state, 31);
        let value = bits % 10;
        if bits.wrapping_sub(value).wrapping_add(9) >= 0 {
            return value == 0;
        }
    }
}

fn blend(px: &mut [u8], color: [u8; 3], alpha: f32) {
    for (c, t) in px.iter_mut().zip(color) {
        *c = (f32::from(*c) * (1.0 - alpha) + f32::from(t) * alpha) as u8;
    }
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
    pub overlay: Overlay,
}

type Shared<T> = Arc<Mutex<T>>;

pub struct MapEngine {
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    workers: Vec<JoinHandle<()>>,
    palette: Arc<Mutex<Option<Arc<Palette>>>>,
    colors: Shared<Option<Arc<dyn Colorize + Send + Sync>>>,
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
    last_scan: Instant,
    scan_centre: (i32, i32),
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
        let colors: Shared<Option<Arc<dyn Colorize + Send + Sync>>> = Arc::new(Mutex::new(None));
        let workers = (0..WORKERS)
            .map(|i| {
                let (rx, tx) = (rx.clone(), tx.clone());
                let (palette, colors) = (palette.clone(), colors.clone());
                std::thread::Builder::new()
                    .name(format!("belumi-map-{i}"))
                    .spawn(move || worker(&rx, &tx, &palette, &colors))
                    .expect("map worker")
            })
            .collect();
        Self {
            jobs: Some(jobs),
            done,
            workers,
            palette,
            colors,
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
            last_scan: Instant::now(),
            scan_centre: (i32::MAX, i32::MAX),
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

    /// Start or stop keeping the scanned surface tiles for the world map; starting scans them again.
    pub fn set_record(&mut self, on: bool) {
        if on && !self.record {
            self.surface.clear();
            self.pending.retain(|(kind, _)| *kind != Kind::Surface);
        }
        self.record = on;
    }

    /// Colour the maps from the blocks' textures and the biomes (tiles scanned before keep the vanilla colours).
    pub fn set_colors(&mut self, colors: Arc<dyn Colorize + Send + Sync>) {
        if let Ok(mut c) = self.colors.lock() {
            *c = Some(colors);
        }
        self.surface.clear();
        self.cave.clear();
        self.pending.clear();
        self.generation += 1;
        self.ready = None;
        self.painting = false;
        self.requested = None;
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
                        self.painting = false;
                    }
                }
                Done::Failed { generation, key, kind } => {
                    if generation == self.generation {
                        self.pending.remove(&(kind, key));
                    }
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
        if self.scan_centre == center && self.last_scan.elapsed() < Duration::from_millis(60) {
            return;
        }
        self.scan_centre = center;
        self.last_scan = Instant::now();
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
            || want.overlay != shown.overlay
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
            overlay: want.overlay,
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

fn worker(
    rx: &Mutex<Receiver<Job>>,
    tx: &Sender<Done>,
    shared: &Mutex<Option<Arc<Palette>>>,
    colors: &Mutex<Option<Arc<dyn Colorize + Send + Sync>>>,
) {
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
                    if tx.send(Done::Failed { generation, key, kind }).is_err() {
                        return;
                    }
                    continue;
                };
                let colorize = colors.lock().ok().and_then(|c| c.clone());
                let scanned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    scan_tile_with(
                        &snap,
                        &palette,
                        air,
                        layer,
                        0,
                        colorize.as_deref().map(|c| (c as &dyn Colorize, key.0, key.1)),
                    )
                }));
                match scanned {
                    Ok(tile) => Done::Scan {
                        generation,
                        key,
                        kind,
                        tile,
                        snap,
                        head,
                    },
                    Err(_) => Done::Failed { generation, key, kind },
                }
            }
            Job::Paint {
                generation,
                cx,
                cz,
                bpp,
                cave,
                overlay,
                tiles,
            } => {
                let mut image = Vec::new();
                paint_sized(&mut image, CANVAS, cx, cz, f64::from(bpp), &tiles, overlay);
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
    fn get(&mut self, x: i32, z: i32) -> Option<(u8, i16, u8, u32)> {
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
        Some((tile.color[i], tile.height[i], tile.depth[i], tile.rgb[i]))
    }
}

/// A real colour lit from the north-west: higher than the north neighbour is
/// brighter, lower darker; water darkens with depth.
fn shaded(rgb: u32, color: u8, height: i16, north: Option<i16>, depth: u8) -> [u8; 4] {
    let factor = if color == crate::minimap::WATER {
        (1.0 - f32::from(depth.min(10)) * 0.045).max(0.55)
    } else {
        let diff = f32::from(height) - north.map_or(f32::from(height), f32::from);
        1.0 + diff.clamp(-3.0, 3.0) * 0.06
    };
    let ch = |s: u32| (((rgb >> s) & 255) as f32 * factor).clamp(0.0, 255.0) as u8;
    [ch(16), ch(8), ch(0), 255]
}

/// North-up canvas centred on (`cx`, `cz`), each texel `bpp` blocks across.
pub fn paint_canvas(
    image: &mut Vec<u8>,
    cx: f64,
    cz: f64,
    bpp: f64,
    tiles: &FxHashMap<(i32, i32), Arc<Tile>>,
) {
    paint_sized(image, CANVAS, cx, cz, bpp, tiles, Overlay::default());
}

/// `paint_canvas` for a canvas `size` texels across.
pub fn paint_sized(
    image: &mut Vec<u8>,
    size: usize,
    cx: f64,
    cz: f64,
    bpp: f64,
    tiles: &FxHashMap<(i32, i32), Arc<Tile>>,
    overlay: Overlay,
) {
    image.resize(size * size * 4, 0);
    let mut lookup = Lookup { tiles, last: None };
    let mut slime_at: Option<((i32, i32), bool)> = None;
    let half = size as f64 / 2.0;
    for j in 0..size {
        let wz = cz + (j as f64 + 0.5 - half) * bpp;
        let z = wz.floor() as i32;
        for i in 0..size {
            let wx = cx + (i as f64 + 0.5 - half) * bpp;
            let x = wx.floor() as i32;
            let o = (j * size + i) * 4;
            let rgba = match lookup.get(x, z) {
                Some((color, height, depth, rgb)) if color & !WALL != 0 => {
                    let north = lookup.get(x, z - 1).filter(|c| c.0 & !WALL != 0).map(|c| c.1);
                    if rgb != 0 {
                        shaded(rgb, color, height, north, depth)
                    } else {
                        let b = brightness(color, height, north, depth, x + z);
                        packed_rgba(color & !WALL, b)
                    }
                }
                _ => UNKNOWN,
            };
            image[o..o + 4].copy_from_slice(&rgba);
            if let Some(seed) = overlay.slime_seed {
                let chunk = (x >> 4, z >> 4);
                let slime = match slime_at {
                    Some((c, s)) if c == chunk => s,
                    _ => {
                        let s = is_slime_chunk(seed, chunk.0, chunk.1);
                        slime_at = Some((chunk, s));
                        s
                    }
                };
                if slime {
                    blend(&mut image[o..o + 3], [60, 255, 90], 0.32);
                }
            }
            if overlay.grid && bpp <= 2.0 && (x & 15 == 0 || z & 15 == 0) {
                blend(&mut image[o..o + 3], [255, 255, 255], 0.22);
            }
        }
    }
}


/// The world map's canvas size and the part of it the screen shows, in texels.
pub const WORLD_CANVAS: usize = 1280;
pub const WORLD_WINDOW: usize = 1024;

struct PaintJob {
    generation: u64,
    cx: f64,
    cz: f64,
    bpp: f32,
    overlay: Overlay,
    tiles: FxHashMap<(i32, i32), Arc<Tile>>,
}
pub struct Painted {
    pub image: Vec<u8>,
    pub cx: f64,
    pub cz: f64,
    pub bpp: f32,
    generation: u64,
}

/// One worker that paints the world map's canvas, so the game thread never does.
pub struct Painter {
    jobs: Option<Sender<PaintJob>>,
    done: Receiver<Painted>,
    worker: Option<JoinHandle<()>>,
    busy: bool,
    generation: u64,
    requested: Option<(f64, f64, f32)>,
    overlay: Overlay,
    dirty: bool,
    last: Instant,
    ready: Option<Painted>,
    shown: Option<(f64, f64, f32)>,
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    pub fn new() -> Self {
        let (jobs, rx) = channel::<PaintJob>();
        let (tx, done) = channel::<Painted>();
        let worker = std::thread::Builder::new()
            .name("belumi-worldmap".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let mut image = Vec::new();
                    paint_sized(&mut image, WORLD_CANVAS, job.cx, job.cz, f64::from(job.bpp), &job.tiles, job.overlay);
                    let done = Painted {
                        image,
                        cx: job.cx,
                        cz: job.cz,
                        bpp: job.bpp,
                        generation: job.generation,
                    };
                    if tx.send(done).is_err() {
                        return;
                    }
                }
            })
            .ok();
        Self {
            jobs: Some(jobs),
            done,
            worker,
            busy: false,
            generation: 0,
            requested: None,
            overlay: Overlay::default(),
            dirty: false,
            last: Instant::now(),
            ready: None,
            shown: None,
        }
    }

    pub fn reset(&mut self) {
        self.generation += 1;
        self.requested = None;
        self.ready = None;
        self.shown = None;
        self.dirty = false;
        self.busy = false;
    }

    fn drain(&mut self) {
        while let Ok(p) = self.done.try_recv() {
            if p.generation == self.generation {
                self.ready = Some(p);
                self.busy = false;
            }
        }
    }

    /// Whether a canvas centred on `shown` at `bpp` no longer serves a view centred on (`x`, `z`).
    pub fn stale(shown: (f64, f64, f32), x: f64, z: f64, bpp: f32) -> bool {
        let margin = ((WORLD_CANVAS - WORLD_WINDOW) as f64 / 2.0 - 32.0) * f64::from(bpp);
        bpp != shown.2 || (x - shown.0).abs() > margin || (z - shown.1).abs() > margin
    }

    /// Paint a canvas for the view when the last one no longer serves it or
    /// the map changed; `tiles` is asked for only then, with the block range
    /// `(x0, z0, x1, z1)` the canvas covers.
    pub fn want(
        &mut self,
        x: f64,
        z: f64,
        bpp: f32,
        changed: bool,
        overlay: Overlay,
        tiles: impl FnOnce((f64, f64, f64, f64)) -> FxHashMap<(i32, i32), Arc<Tile>>,
    ) {
        self.drain();
        self.dirty |= changed;
        if self.busy {
            return;
        }
        let stale = match self.requested {
            None => true,
            Some(r) => {
                Self::stale(r, x, z, bpp)
                    || overlay != self.overlay
                    || (self.dirty && self.last.elapsed() >= REPAINT)
            }
        };
        if !stale {
            return;
        }
        let snap = |v: f64| (v / 16.0).round() * 16.0;
        let (cx, cz) = (snap(x), snap(z));
        let half = WORLD_CANVAS as f64 / 2.0 * f64::from(bpp);
        let tiles = tiles((cx - half, cz - half, cx + half, cz + half));
        self.busy = true;
        self.dirty = false;
        self.last = Instant::now();
        self.requested = Some((cx, cz, bpp));
        self.overlay = overlay;
        if let Some(tx) = &self.jobs {
            let _ = tx.send(PaintJob {
                generation: self.generation,
                cx,
                cz,
                bpp,
                overlay,
                tiles,
            });
        }
    }

    pub fn take(&mut self) -> Option<Painted> {
        self.drain();
        let p = self.ready.take()?;
        self.shown = Some((p.cx, p.cz, p.bpp));
        Some(p)
    }

    pub fn shown(&self) -> Option<(f64, f64, f32)> {
        self.shown
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
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
    fn the_java_random_matches_its_known_first_value() {
        let mut seed = (42i64 ^ 0x5_DEEC_E66D) & ((1i64 << 48) - 1);
        assert_eq!(java_next(&mut seed, 32), -1_170_105_035);
    }

    #[test]
    fn slime_chunks_are_about_one_in_ten_and_stable() {
        let count = (-50..50)
            .flat_map(|x| (-50..50).map(move |z| (x, z)))
            .filter(|&(x, z)| is_slime_chunk(12345, x, z))
            .count();
        assert!((800..1200).contains(&count), "{count}");
        assert_eq!(is_slime_chunk(12345, 7, -3), is_slime_chunk(12345, 7, -3));
        assert_ne!(
            (0..40).map(|x| is_slime_chunk(1, x, 0)).collect::<Vec<_>>(),
            (0..40).map(|x| is_slime_chunk(2, x, 0)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_overlay_draws_chunk_borders_and_slime_tint_and_changes_what_is_stale() {
        let mut tiles = FxHashMap::default();
        for x in -4..4 {
            for z in -4..4 {
                tiles.insert((x, z), tile(1, 64));
            }
        }
        let (mut plain, mut grid) = (Vec::new(), Vec::new());
        paint_sized(&mut plain, CANVAS, 0.0, 0.0, 1.0, &tiles, Overlay::default());
        paint_sized(&mut grid, CANVAS, 0.0, 0.0, 1.0, &tiles, Overlay { grid: true, slime_seed: None });
        let border = (CANVAS / 2 * CANVAS + CANVAS / 2) * 4;
        assert_ne!(&plain[border..border + 3], &grid[border..border + 3]);
        let inside = border + (5 * CANVAS + 5) * 4;
        assert_eq!(&plain[inside..inside + 3], &grid[inside..inside + 3]);
        let shown = Want { x: 0.0, z: 0.0, bpp: 1.0, cave: false, overlay: Overlay::default() };
        assert!(MapEngine::canvas_stale(&shown, &Want { overlay: Overlay { grid: true, slime_seed: None }, ..shown }));
    }

    #[test]
    fn real_colours_shade_by_slope_and_water_depth() {
        let flat = shaded(0x808080, 1, 64, Some(64), 0);
        let up = shaded(0x808080, 1, 66, Some(64), 0);
        let down = shaded(0x808080, 1, 62, Some(64), 0);
        assert!(up[0] > flat[0] && flat[0] > down[0]);
        let shallow = shaded(0x3F76E4, crate::minimap::WATER, 62, None, 1);
        let deep = shaded(0x3F76E4, crate::minimap::WATER, 40, None, 10);
        assert!(shallow[2] > deep[2]);
        assert_eq!(flat[3], 255);
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
    fn the_world_canvas_serves_a_view_until_it_nears_the_edge() {
        let shown = (0.0, 0.0, 1.0);
        assert!(!Painter::stale(shown, 50.0, -60.0, 1.0));
        assert!(Painter::stale(shown, 100.0, 0.0, 1.0));
        assert!(Painter::stale(shown, 0.0, 0.0, 2.0));
        assert!(!Painter::stale((0.0, 0.0, 4.0), 300.0, 0.0, 4.0));
    }

    #[test]
    fn the_painter_paints_once_and_waits_for_a_reason_to_paint_again() {
        let mut painter = Painter::new();
        let mut asked = 0;
        painter.want(5.0, 5.0, 1.0, false, Overlay::default(), |_| {
            asked += 1;
            FxHashMap::default()
        });
        let mut got = None;
        for _ in 0..300 {
            got = painter.take();
            if got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let got = got.expect("a painted world canvas");
        assert_eq!(got.image.len(), WORLD_CANVAS * WORLD_CANVAS * 4);
        assert_eq!((got.cx, got.cz), (0.0, 0.0));
        painter.want(5.0, 5.0, 1.0, false, Overlay::default(), |_| {
            asked += 1;
            FxHashMap::default()
        });
        assert_eq!(asked, 1);
        assert_eq!(painter.shown(), Some((0.0, 0.0, 1.0)));
    }

    #[test]
    fn the_canvas_stays_until_the_player_nears_its_edge_or_the_zoom_changes() {
        let shown = Want { x: 0.0, z: 0.0, bpp: 1.0, cave: false, overlay: Overlay::default() };
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
        engine.want_canvas(Want { x: 3.0, z: 5.0, bpp: 1.0, cave: false, overlay: Overlay::default() });
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
        engine.want_canvas(Want { x: 3.0, z: 5.0, bpp: 1.0, cave: false, overlay: Overlay::default() });
        assert!(engine.take_canvas().is_none());
    }
}
