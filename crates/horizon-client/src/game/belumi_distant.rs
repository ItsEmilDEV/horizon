//! Distant chunks (what the Bobby mod does): the columns a server sent are kept on disk and stay drawn after the
//! server forgot them, and the next visit shows the cached columns out to your own render distance before the server
//! sends anything. Nothing here changes what the server knows: a column the server sends again replaces the cached one.
use super::Game;
use horizon_client::fast_hash::FxHashSet;
use horizon_protocol::{
    buf::{Codec, Reader, Writer},
    packets::play,
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant, SystemTime},
};

/// Everything the cache holds past this size is pruned, oldest files first, when a game starts.
const CACHE_LIMIT: u64 = 1 << 30;
/// Columns taken from disk into the world per frame.
const PER_FRAME: usize = 6;
const EVICT_EVERY: Duration = Duration::from_secs(2);

enum Job {
    Write(PathBuf, Vec<u8>),
}

#[derive(Default)]
pub(super) struct Distant {
    /// Columns the server no longer holds that are still drawn.
    ghosts: FxHashSet<(i32, i32)>,
    /// The world and dimension the loader was started for.
    loaded_for: Option<(String, String)>,
    incoming: Option<Receiver<((i32, i32), Vec<u8>)>>,
    writer: Option<Sender<Job>>,
    last_evict: Option<Instant>,
}

fn root() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join("belumi-chunks")
}

fn file(dir: &Path, x: i32, z: i32) -> PathBuf {
    dir.join(format!("{x}.{z}.bin"))
}

fn pack(bytes: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    if enc.write_all(bytes).is_err() {
        return Vec::new();
    }
    enc.finish().unwrap_or_default()
}

fn unpack(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(bytes).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Deletes the oldest cached columns until the cache fits `limit` bytes (with a tenth to spare).
fn prune(root: &Path, limit: u64) {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut total = 0u64;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
                files.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), entry.path()));
            }
        }
    }
    if total <= limit {
        return;
    }
    files.sort();
    let target = limit - limit / 10;
    for (_, len, path) in files {
        if total <= target {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
}

/// The columns saved in `dir` within `radius` of `centre`, nearest first.
fn saved_columns(dir: &Path, centre: (i32, i32), radius: i32) -> Vec<(i32, i32)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(i32, (i32, i32))> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".bin")?;
            let (x, z) = stem.split_once('.')?;
            let (x, z) = (x.parse::<i32>().ok()?, z.parse::<i32>().ok()?);
            let d = (x - centre.0).abs().max((z - centre.1).abs());
            (d <= radius).then_some((d, (x, z)))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, k)| k).collect()
}

impl Distant {
    fn write(&mut self, path: PathBuf, bytes: Vec<u8>) {
        let tx = self.writer.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let spawned = std::thread::Builder::new().name("Belumi chunk cache".into()).spawn(move || {
                prune(&root(), CACHE_LIMIT);
                for Job::Write(path, bytes) in rx {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let tmp = path.with_extension("tmp");
                    if std::fs::write(&tmp, &bytes).is_ok() {
                        let _ = std::fs::rename(&tmp, &path);
                    }
                }
            });
            if let Err(e) = spawned {
                log::warn!("Belumi: no chunk cache thread ({e})");
            }
            tx
        });
        let _ = tx.send(Job::Write(path, bytes));
    }
}

impl Game<'_> {
    /// Whether distant chunks are on and this is a server (single player and LAN worlds have their own saves).
    pub(super) fn belumi_distant_on(&self) -> bool {
        self.remote.is_some()
            && !self.belumi_replay_active()
            && self.modules_ref().is_none_or(|m| m.misc.distant_chunks)
    }

    fn belumi_distant_dir(&self) -> PathBuf {
        root()
            .join(super::belumi_maps::safe_name(&self.belumi_scope()))
            .join(super::belumi_maps::safe_name(self.dim.id()))
    }

    /// A column arrived from the server: it is no longer a ghost, and a copy goes to the disk.
    pub(super) fn belumi_distant_received(&mut self, p: &play::LevelChunkWithLight) {
        self.belumi_distant.ghosts.remove(&(p.x, p.z));
        if !self.belumi_distant_on() {
            return;
        }
        let mut w = Writer::new();
        <play::LevelChunkWithLight as Codec<play::LevelChunkWithLight>>::write(p, &mut w);
        let path = file(&self.belumi_distant_dir(), p.x, p.z);
        let bytes = pack(&w.data);
        if !bytes.is_empty() {
            self.belumi_distant.write(path, bytes);
        }
    }

    /// The server dropped this column (or the storage moved away from it): with distant chunks on it stays as it was.
    /// Returns whether it was kept.
    pub(super) fn belumi_distant_keep(&mut self, x: i32, z: i32) -> bool {
        if !self.belumi_distant_on() || !self.world.chunks.contains_key(&(x, z)) {
            return false;
        }
        self.belumi_distant.ghosts.insert((x, z));
        self.world.block_entities.retain(|k, _| (k.0 >> 4, k.2 >> 4) != (x, z));
        true
    }

    /// How far the client draws on a server: the render distance, not only what the server sends.
    pub(super) fn belumi_distant_radius(&self, server: i32) -> i32 {
        let own = self.settings.render_distance;
        if self.belumi_distant_on() { own.max(2) } else { own.min(server + 1).max(2) }
    }

    /// Once a frame: loads the cache when the world or dimension changed, takes a few cached columns into the world and
    /// lets go of ghosts that are further away than the render distance.
    pub(super) fn belumi_distant_tick(&mut self) {
        if !self.belumi_distant_on() {
            if !self.belumi_distant.ghosts.is_empty() || self.belumi_distant.incoming.is_some() {
                let gone: Vec<_> = self.belumi_distant.ghosts.drain().collect();
                self.belumi_distant.incoming = None;
                self.belumi_distant.loaded_for = None;
                self.belumi_distant_release(gone);
            }
            return;
        }
        let Some(at) = self.player.as_ref().map(|p| ((p.position.x.floor() as i32) >> 4, (p.position.z.floor() as i32) >> 4))
        else {
            return;
        };
        let radius = self.streaming.radius.max(2);
        let key = (self.belumi_scope(), self.dim.id().to_owned());
        if self.belumi_distant.loaded_for.as_ref() != Some(&key) {
            self.belumi_distant.loaded_for = Some(key);
            self.belumi_distant.ghosts.clear();
            let dir = self.belumi_distant_dir();
            let (tx, rx) = mpsc::channel();
            self.belumi_distant.incoming = Some(rx);
            let _ = std::thread::Builder::new().name("Belumi chunk cache load".into()).spawn(move || {
                for (x, z) in saved_columns(&dir, at, radius + 2) {
                    let Some(bytes) = std::fs::read(file(&dir, x, z)).ok().and_then(|b| unpack(&b)) else { continue };
                    if tx.send(((x, z), bytes)).is_err() {
                        return;
                    }
                }
            });
        }
        for _ in 0..PER_FRAME {
            let Some(rx) = self.belumi_distant.incoming.as_ref() else { break };
            let Ok(((x, z), bytes)) = rx.try_recv() else { break };
            self.belumi_distant_insert(x, z, &bytes);
        }
        let due = self.belumi_distant.last_evict.is_none_or(|t| t.elapsed() >= EVICT_EVERY);
        if due {
            self.belumi_distant.last_evict = Some(Instant::now());
            let far: Vec<_> =
                self.belumi_distant.ghosts.iter().copied().filter(|&(x, z)| (x - at.0).abs().max((z - at.1).abs()) > radius + 2).collect();
            for k in &far {
                self.belumi_distant.ghosts.remove(k);
            }
            self.belumi_distant_release(far);
        }
    }

    /// A cached column becomes a ghost, unless the world has that column already.
    fn belumi_distant_insert(&mut self, x: i32, z: i32, bytes: &[u8]) {
        if self.world.chunks.contains_key(&(x, z)) {
            return;
        }
        let Some(format) = self.remote.as_ref().map(|r| r.chunk_format.clone()) else { return };
        let mut r = Reader::new(bytes);
        let Ok(packet) = <play::LevelChunkWithLight as Codec<play::LevelChunkWithLight>>::read(&mut r) else { return };
        if (packet.x, packet.z) != (x, z) {
            return;
        }
        let Ok(chunk) = format.column(&packet) else { return };
        self.streaming.insert_remote(&mut self.world, chunk);
        self.belumi_distant.ghosts.insert((x, z));
    }

    /// Takes columns out of the world and the renderer.
    fn belumi_distant_release(&mut self, gone: Vec<(i32, i32)>) {
        if gone.is_empty() {
            return;
        }
        let mut freed = Vec::with_capacity(gone.len());
        for &(x, z) in &gone {
            freed.extend(self.streaming.forget(&mut self.world, x, z));
        }
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.remove_columns(gone.iter().copied());
        }
        horizon_client::chunk::free_columns(freed, gone.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("belumi-distant-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_packed_column_unpacks_to_the_same_bytes() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 7) as u8).collect();
        let packed = pack(&data);
        assert!(packed.len() < data.len());
        assert_eq!(unpack(&packed).unwrap(), data);
    }

    #[test]
    fn saved_columns_come_nearest_first_and_only_within_the_radius() {
        let dir = Dir::new("a");
        for (x, z) in [(0, 0), (5, 0), (-2, 1), (30, 30)] {
            std::fs::write(file(dir.path(), x, z), b"x").unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        assert_eq!(saved_columns(dir.path(), (0, 0), 10), vec![(0, 0), (-2, 1), (5, 0)]);
    }

    #[test]
    fn the_prune_removes_the_oldest_files_first_and_keeps_the_rest() {
        let dir = Dir::new("b");
        let old = file(dir.path(), 1, 1);
        std::fs::write(&old, vec![0u8; 600]).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let new = file(dir.path(), 2, 2);
        std::fs::write(&new, vec![0u8; 600]).unwrap();
        prune(dir.path(), 1000);
        assert!(!old.exists());
        assert!(new.exists());
        prune(dir.path(), 1 << 20);
        assert!(new.exists());
    }
}
