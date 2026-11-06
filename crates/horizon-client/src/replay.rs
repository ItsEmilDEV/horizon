//! Replays: a session's packets on disk, and a server that plays them back to the client.
//!
//! The recorder taps the connection thread (`net.rs`): every clientbound packet of the login, the
//! configuration and the play phase goes into a file with its time. Playing a file back is a small
//! server on an in-memory pipe, as single player's is: it answers the handshake, then sends the packets at
//! their times, faster, slower or paused. The client cannot tell it from a real server.
//!
//! File: `BRPL1`, a length-prefixed JSON header, then chunks of zlib-compressed records
//! (`state u8, time ms u32, id i32, length u32, bytes`), each chunk led by its sizes and its first and last time.

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use horizon_protocol::{
    buf::write_varint,
    connection::{Connection, Pipe},
    packets::{configuration as cfg, play},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering},
        mpsc::{Receiver, RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

pub const MAGIC: &[u8; 5] = b"BRPL1";
/// The custom payload that carries the recorded player's own pose (position, view, state).
pub const POSE_CHANNEL: &str = "belumi:pose";
/// The custom payload that carries a bookmark or a clip mark.
pub const MARK_CHANNEL: &str = "belumi:mark";
const CHUNK_RAW: usize = 256 * 1024;
const FLUSH_EVERY: Duration = Duration::from_secs(5);
const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(5);

pub const LOGIN: u8 = 0;
pub const CONFIG: u8 = 1;
pub const PLAY: u8 = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub protocol: i32,
    pub server: String,
    pub player: String,
    pub uuid: String,
    pub started_unix: u64,
    pub game: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub state: u8,
    pub t: u32,
    pub id: i32,
    pub data: Vec<u8>,
}

impl Record {
    fn write(&self, out: &mut Vec<u8>) {
        out.push(self.state);
        out.extend_from_slice(&self.t.to_le_bytes());
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.data);
    }

    fn read_all(mut raw: &[u8]) -> Vec<Record> {
        let mut out = Vec::new();
        while raw.len() >= 13 {
            let state = raw[0];
            let t = u32::from_le_bytes([raw[1], raw[2], raw[3], raw[4]]);
            let id = i32::from_le_bytes([raw[5], raw[6], raw[7], raw[8]]);
            let len = u32::from_le_bytes([raw[9], raw[10], raw[11], raw[12]]) as usize;
            if raw.len() < 13 + len {
                break;
            }
            out.push(Record { state, t, id, data: raw[13..13 + len].to_vec() });
            raw = &raw[13 + len..];
        }
        out
    }
}

// ---- recording -------------------------------------------------------------------------

enum Msg {
    Record(Record),
    Stop,
}

pub struct Recorder {
    tx: Mutex<Option<Sender<Msg>>>,
    start: Instant,
    pub path: PathBuf,
    done: Arc<AtomicBool>,
}

static ACTIVE: Mutex<Option<Arc<Recorder>>> = Mutex::new(None);
static RECORDING: AtomicBool = AtomicBool::new(false);

impl Recorder {
    fn push(&self, state: u8, id: i32, data: &[u8]) {
        let t = self.start.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(Msg::Record(Record { state, t, id, data: data.to_vec() }));
        }
    }

    /// Milliseconds since the recording began.
    pub fn elapsed_ms(&self) -> u32 {
        self.start.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
    }

    fn stop(&self) {
        if let Some(tx) = self.tx.lock().unwrap().take() {
            let _ = tx.send(Msg::Stop);
        }
    }

    /// Whether the writer has finished its file.
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

/// Begins recording into `dir` (made if missing). Replaces a recording in progress.
pub fn start(dir: &Path, header: &Header) -> std::io::Result<Arc<Recorder>> {
    stop();
    std::fs::create_dir_all(dir)?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H.%M.%S").to_string();
    let server: String = header.server.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    let path = dir.join(format!("{stamp}_{server}.brpl"));
    let mut file = File::create(&path)?;
    file.write_all(MAGIC)?;
    let json = serde_json::to_vec(header).unwrap_or_default();
    file.write_all(&(json.len() as u32).to_le_bytes())?;
    file.write_all(&json)?;
    let (tx, rx) = channel();
    let done = Arc::new(AtomicBool::new(false));
    let recorder = Arc::new(Recorder { tx: Mutex::new(Some(tx)), start: Instant::now(), path, done: done.clone() });
    std::thread::Builder::new()
        .name("horizon-replay-writer".into())
        .spawn(move || {
            writer_loop(file, rx);
            done.store(true, Ordering::Release);
        })?;
    *ACTIVE.lock().unwrap() = Some(recorder.clone());
    RECORDING.store(true, Ordering::Release);
    Ok(recorder)
}

/// Ends the recording in progress and returns it (its file closes in the background).
pub fn stop() -> Option<Arc<Recorder>> {
    RECORDING.store(false, Ordering::Release);
    let recorder = ACTIVE.lock().unwrap().take()?;
    recorder.stop();
    Some(recorder)
}

pub fn active() -> Option<Arc<Recorder>> {
    ACTIVE.lock().unwrap().clone()
}

pub fn recording() -> bool {
    RECORDING.load(Ordering::Acquire)
}

/// A packet the client received (`state`: [`LOGIN`], [`CONFIG`] or [`PLAY`]); a no-op unless recording.
pub fn record(state: u8, id: i32, data: &[u8]) {
    if !recording() {
        return;
    }
    if let Some(r) = ACTIVE.lock().unwrap().as_ref() {
        r.push(state, id, data);
    }
}

fn flush_chunk(file: &mut File, raw: &mut Vec<u8>, first: u32, last: u32) {
    if raw.is_empty() {
        return;
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
    let packed = z.write_all(raw).and_then(|()| z.finish());
    if let Ok(packed) = packed {
        let mut head = Vec::with_capacity(16);
        for v in [packed.len() as u32, raw.len() as u32, first, last] {
            head.extend_from_slice(&v.to_le_bytes());
        }
        let _ = file.write_all(&head);
        let _ = file.write_all(&packed);
        let _ = file.flush();
    }
    raw.clear();
}

fn writer_loop(mut file: File, rx: Receiver<Msg>) {
    let mut raw = Vec::with_capacity(CHUNK_RAW + 4096);
    let (mut first, mut last) = (0u32, 0u32);
    let mut last_flush = Instant::now();
    loop {
        match rx.recv_timeout(FLUSH_EVERY) {
            Ok(Msg::Record(r)) => {
                if raw.is_empty() {
                    first = r.t;
                }
                last = r.t;
                r.write(&mut raw);
                if raw.len() >= CHUNK_RAW {
                    flush_chunk(&mut file, &mut raw, first, last);
                    last_flush = Instant::now();
                }
            }
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if last_flush.elapsed() >= FLUSH_EVERY {
            flush_chunk(&mut file, &mut raw, first, last);
            last_flush = Instant::now();
        }
    }
    flush_chunk(&mut file, &mut raw, first, last);
    let _ = file.sync_all();
}

// ---- reading ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct ChunkInfo {
    offset: u64,
    packed: u32,
    raw: u32,
    #[allow(dead_code)]
    first: u32,
    last: u32,
}

pub struct Replay {
    pub path: PathBuf,
    pub header: Header,
    chunks: Vec<ChunkInfo>,
    pub duration_ms: u32,
    pub bytes: u64,
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl Replay {
    /// Reads the header and the chunk table (not the packets).
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let bytes = file.metadata()?.len();
        let mut magic = [0u8; 5];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a replay"));
        }
        let mut len = [0u8; 4];
        file.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        if len > 1 << 20 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad header"));
        }
        let mut json = vec![0u8; len];
        file.read_exact(&mut json)?;
        let header: Header = serde_json::from_slice(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut chunks = Vec::new();
        let mut at = 9 + len as u64;
        loop {
            let mut head = [0u8; 16];
            if file.read_exact(&mut head).is_err() {
                break;
            }
            let info = ChunkInfo { offset: at + 16, packed: u32_at(&head, 0), raw: u32_at(&head, 4), first: u32_at(&head, 8), last: u32_at(&head, 12) };
            if at + 16 + u64::from(info.packed) > bytes {
                break;
            }
            chunks.push(info);
            at += 16 + u64::from(info.packed);
            if file.seek(SeekFrom::Start(at)).is_err() {
                break;
            }
        }
        let duration_ms = chunks.last().map_or(0, |c| c.last);
        Ok(Self { path: path.to_path_buf(), header, chunks, duration_ms, bytes })
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// The records of chunk `i`.
    pub fn chunk(&self, i: usize) -> Vec<Record> {
        let Some(info) = self.chunks.get(i) else {
            return Vec::new();
        };
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        if file.seek(SeekFrom::Start(info.offset)).is_err() {
            return Vec::new();
        }
        let mut packed = vec![0u8; info.packed as usize];
        if file.read_exact(&mut packed).is_err() {
            return Vec::new();
        }
        let mut raw = Vec::with_capacity(info.raw as usize);
        if ZlibDecoder::new(&packed[..]).read_to_end(&mut raw).is_err() {
            return Vec::new();
        }
        Record::read_all(&raw)
    }

    /// The marks recorded in the file (bookmarks), as (time, label).
    pub fn marks(&self) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        for i in 0..self.chunks.len() {
            for r in self.chunk(i) {
                if r.state == PLAY
                    && let Some(label) = parse_mark(&r.data)
                {
                    out.push((r.t, label));
                }
            }
        }
        out
    }
}

/// A custom payload packet (clientbound, play) with `channel` and `data`, as a record's id and bytes.
pub fn custom_payload(channel: &str, data: &[u8]) -> (i32, Vec<u8>) {
    let body = play::Clientbound::CustomPayload(cfg::CustomPayload { channel: channel.to_owned(), data: data.to_vec() }).encode();
    split_body(&body)
}

fn split_body(body: &[u8]) -> (i32, Vec<u8>) {
    let mut r = horizon_protocol::buf::Reader::new(body);
    let id = r.varint().unwrap_or(0);
    let used = body.len() - r.remaining();
    (id, body[used..].to_vec())
}

fn parse_mark(payload: &[u8]) -> Option<String> {
    let mut r = horizon_protocol::buf::Reader::new(payload);
    let channel = r.string(256).ok()?;
    if channel != MARK_CHANNEL {
        return None;
    }
    let rest = r.rest();
    Some(String::from_utf8_lossy(rest).into_owned())
}

/// The recorded player's pose at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: u8,
}

pub const POSE_SNEAK: u8 = 1;
pub const POSE_SPRINT: u8 = 2;
pub const POSE_GROUND: u8 = 4;

impl Pose {
    pub fn to_bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(33);
        for v in [self.x, self.y, self.z] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [self.yaw, self.pitch] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(self.flags);
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 33 {
            return None;
        }
        let f64_at = |i: usize| f64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]));
        let f32_at = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap_or([0; 4]));
        Some(Self { x: f64_at(0), y: f64_at(8), z: f64_at(16), yaw: f32_at(24), pitch: f32_at(28), flags: b[32] })
    }
}

// ---- clips -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    pub start_ms: u32,
    pub end_ms: u32,
    pub created_unix: u64,
}

pub fn clips_path(replay: &Path) -> PathBuf {
    replay.with_extension("clips.json")
}

pub fn load_clips(replay: &Path) -> Vec<Clip> {
    std::fs::read(clips_path(replay)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_clips(replay: &Path, clips: &[Clip]) {
    if let Ok(json) = serde_json::to_vec_pretty(clips) {
        let _ = std::fs::write(clips_path(replay), json);
    }
}

/// Adds a clip of the last `seconds` ending at `now_ms` to the replay's clip list.
pub fn add_clip(replay: &Path, now_ms: u32, seconds: u32, name: &str) -> Clip {
    let clip = Clip {
        name: name.to_owned(),
        start_ms: now_ms.saturating_sub(seconds.saturating_mul(1000)),
        end_ms: now_ms,
        created_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    };
    let mut clips = load_clips(replay);
    clips.push(clip.clone());
    save_clips(replay, &clips);
    clip
}

/// One replay of a folder, for the list.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub header: Header,
    pub duration_ms: u32,
    pub bytes: u64,
    pub clips: Vec<Clip>,
}

/// The replays of `dir`, newest first.
pub fn list(dir: &Path) -> Vec<Entry> {
    let mut out: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "brpl"))
        .filter_map(|p| {
            let r = Replay::open(&p).ok()?;
            Some(Entry { clips: load_clips(&p), path: p, header: r.header, duration_ms: r.duration_ms, bytes: r.bytes })
        })
        .collect();
    out.sort_by(|a, b| b.header.started_unix.cmp(&a.header.started_unix).then_with(|| b.path.cmp(&a.path)));
    out
}

/// Deletes the oldest replays so that at most `keep` remain.
pub fn prune(dir: &Path, keep: usize) {
    for e in list(dir).into_iter().skip(keep) {
        let _ = std::fs::remove_file(&e.path);
        let _ = std::fs::remove_file(clips_path(&e.path));
    }
}

// ---- playback --------------------------------------------------------------------------

/// What the game and the playing server tell each other.
pub struct Control {
    pub paused: AtomicBool,
    /// 1000 is normal speed.
    pub speed_milli: AtomicU32,
    /// A requested time to jump to, or -1.
    pub seek: AtomicI64,
    /// The server stopped for a jump back: restart the session there, or -1.
    pub restart: AtomicI64,
    /// Where playback is, in ms.
    pub position: AtomicU32,
    pub duration: AtomicU32,
    /// Playback stops here (a clip's end); `u32::MAX` for the whole replay.
    pub end: AtomicU32,
    pub finished: AtomicBool,
    pub stop: AtomicBool,
    /// The login and configuration are through: the world is coming.
    pub playing: AtomicBool,
}

impl Control {
    pub fn new(duration: u32, end: u32) -> Arc<Self> {
        Arc::new(Self {
            paused: AtomicBool::new(false),
            speed_milli: AtomicU32::new(1000),
            seek: AtomicI64::new(-1),
            restart: AtomicI64::new(-1),
            position: AtomicU32::new(0),
            duration: AtomicU32::new(duration),
            end: AtomicU32::new(end),
            finished: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            playing: AtomicBool::new(false),
        })
    }
}

fn body_of(id: i32, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(data.len() + 5);
    write_varint(&mut body, id);
    body.extend_from_slice(data);
    body
}

/// Starts the server for `replay` on `pipe` (the server end). It plays from the start, sending everything before
/// `start_ms` at once, and ends at `ctl.end`. The thread ends with the connection or `ctl.stop`.
pub fn serve(pipe: Pipe, replay: Replay, ctl: Arc<Control>, start_ms: u32) {
    let _ = std::thread::Builder::new().name("horizon-replay-server".into()).spawn(move || run_server(pipe, replay, ctl, start_ms));
}

fn run_server(pipe: Pipe, replay: Replay, ctl: Arc<Control>, start_ms: u32) {
    let mut conn = Connection::local(pipe);
    // The handshake and the login start the client opens with.
    for _ in 0..2 {
        if conn.recv().is_err() {
            log::warn!("Horizon replay: the client closed before its handshake");
            return;
        }
    }
    log::info!("Horizon replay: serving {} ({} chunks) from {} ms", replay.path.display(), replay.chunk_count(), start_ms);
    let (mut reader, mut writer) = conn.split();
    std::thread::spawn(move || while reader.recv().is_ok() {});
    let mut position = 0.0f64;
    let mut last = Instant::now();
    let mut idle = Instant::now();
    let mut fast_until = start_ms;
    let mut keep_alive_id = 1i64;
    let mut sent_any_play = false;
    for chunk in 0..replay.chunk_count() {
        for rec in replay.chunk(chunk) {
            loop {
                if ctl.stop.load(Ordering::Relaxed) {
                    return;
                }
                let now = Instant::now();
                let dt = now.duration_since(last).as_secs_f64() * 1000.0;
                last = now;
                let seek = ctl.seek.swap(-1, Ordering::AcqRel);
                if seek >= 0 {
                    if (seek as f64) < position {
                        ctl.restart.store(seek, Ordering::Release);
                        return;
                    }
                    fast_until = seek as u32;
                }
                let target_end = ctl.end.load(Ordering::Relaxed);
                if rec.state < PLAY || rec.t <= fast_until {
                    position = position.max(f64::from(rec.t));
                    break;
                }
                if rec.t > target_end {
                    ctl.finished.store(true, Ordering::Release);
                    ctl.paused.store(true, Ordering::Release);
                }
                if !ctl.paused.load(Ordering::Relaxed) {
                    position += dt * f64::from(ctl.speed_milli.load(Ordering::Relaxed)) / 1000.0;
                }
                ctl.position.store(position as u32, Ordering::Relaxed);
                if f64::from(rec.t) <= position && !ctl.paused.load(Ordering::Relaxed) {
                    break;
                }
                if sent_any_play && idle.elapsed() >= KEEP_ALIVE_EVERY {
                    keep_alive_id += 1;
                    let body = play::Clientbound::KeepAlive(cfg::KeepAlive { id: keep_alive_id }).encode();
                    if writer.send(&body).is_err() {
                        return;
                    }
                    idle = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(4));
            }
            if rec.state == PLAY {
                ctl.playing.store(true, Ordering::Release);
                sent_any_play = true;
            }
            if writer.send(&body_of(rec.id, &rec.data)).is_err() {
                log::warn!("Horizon replay: the client went away at {} ms", rec.t);
                return;
            }
            idle = Instant::now();
            if rec.state == PLAY {
                ctl.position.store(position.max(f64::from(rec.t)) as u32, Ordering::Relaxed);
            }
        }
    }
    // The end of the recording: stay up, paused, and keep the connection alive.
    ctl.finished.store(true, Ordering::Release);
    ctl.paused.store(true, Ordering::Release);
    ctl.position.store(replay.duration_ms, Ordering::Relaxed);
    loop {
        if ctl.stop.load(Ordering::Relaxed) {
            return;
        }
        let seek = ctl.seek.swap(-1, Ordering::AcqRel);
        if seek >= 0 {
            ctl.restart.store(seek, Ordering::Release);
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
        if idle.elapsed() >= KEEP_ALIVE_EVERY {
            keep_alive_id += 1;
            let body = play::Clientbound::KeepAlive(cfg::KeepAlive { id: keep_alive_id }).encode();
            if writer.send(&body).is_err() {
                return;
            }
            idle = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("belumi-replay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn records_round_trip_through_a_file() {
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp("file");
        let header = Header { protocol: 7, server: "play.example.org:25565".into(), player: "Alex".into(), uuid: "u".into(), started_unix: 5, game: "26.3".into() };
        let rec = start(&dir, &header).unwrap();
        record(LOGIN, 2, &[1, 2, 3]);
        record(PLAY, 40, &[9; 10]);
        std::thread::sleep(Duration::from_millis(30));
        record(PLAY, 41, &[]);
        let path = rec.path.clone();
        stop();
        for _ in 0..200 {
            if rec.finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let replay = Replay::open(&path).unwrap();
        assert_eq!(replay.header, header);
        assert_eq!(replay.chunk_count(), 1);
        let records = replay.chunk(0);
        assert_eq!(records.iter().map(|r| (r.state, r.id, r.data.len())).collect::<Vec<_>>(), vec![(LOGIN, 2, 3), (PLAY, 40, 10), (PLAY, 41, 0)]);
        assert!(records[2].t >= records[1].t);
        assert!(replay.duration_ms >= 30);
        let listed = list(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].header.player, "Alex");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recording_nothing_when_not_recording() {
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        stop();
        record(PLAY, 1, &[1]);
        assert!(!recording());
    }

    #[test]
    fn poses_survive_bytes_and_clips_survive_files() {
        let pose = Pose { x: 1.5, y: -64.0, z: 3e5, yaw: 90.0, pitch: -12.5, flags: POSE_SNEAK | POSE_GROUND };
        assert_eq!(Pose::from_bytes(&pose.to_bytes()), Some(pose));
        assert_eq!(Pose::from_bytes(&[0; 5]), None);
        let dir = temp("clips");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.brpl");
        let clip = add_clip(&path, 10_000, 30, "Ace");
        assert_eq!((clip.start_ms, clip.end_ms), (0, 10_000));
        let clip = add_clip(&path, 90_000, 30, "Second");
        assert_eq!(clip.start_ms, 60_000);
        assert_eq!(load_clips(&path).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_payloads_split_into_an_id_and_a_body() {
        let (id, data) = custom_payload(POSE_CHANNEL, &[1, 2, 3]);
        let body = body_of(id, &data);
        let mut r = horizon_protocol::buf::Reader::new(&body);
        assert_eq!(r.varint().unwrap(), id);
        assert_eq!(r.string(256).unwrap(), POSE_CHANNEL);
    }

    #[test]
    fn the_server_plays_a_replay_to_a_client_on_a_pipe() {
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp("serve");
        let header = Header { protocol: 1, server: "t".into(), player: "P".into(), uuid: "u".into(), started_unix: 1, game: "g".into() };
        let rec = start(&dir, &header).unwrap();
        record(LOGIN, 2, &[7]);
        record(CONFIG, 3, &[8]);
        record(PLAY, 30, &[1, 1]);
        record(PLAY, 31, &[2, 2]);
        let path = rec.path.clone();
        stop();
        for _ in 0..200 {
            if rec.finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let replay = Replay::open(&path).unwrap();
        let duration = replay.duration_ms;
        let ctl = Control::new(duration, u32::MAX);
        let (client, server) = Pipe::pair();
        serve(server, replay, ctl.clone(), 0);
        let mut c = Connection::local(client);
        c.send(&[0]).unwrap();
        c.send(&[1]).unwrap();
        let got: Vec<(i32, Vec<u8>)> = (0..4).map(|_| c.recv().unwrap()).collect();
        assert_eq!(got, vec![(2, vec![7]), (3, vec![8]), (30, vec![1, 1]), (31, vec![2, 2])]);
        for _ in 0..100 {
            if ctl.finished.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ctl.finished.load(Ordering::Acquire));
        ctl.stop.store(true, Ordering::Release);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_jump_back_asks_for_a_restart() {
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp("back");
        let header = Header::default();
        let rec = start(&dir, &header).unwrap();
        record(PLAY, 30, &[1]);
        std::thread::sleep(Duration::from_millis(60));
        record(PLAY, 31, &[2]);
        let path = rec.path.clone();
        stop();
        for _ in 0..200 {
            if rec.finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let replay = Replay::open(&path).unwrap();
        let ctl = Control::new(replay.duration_ms, u32::MAX);
        ctl.speed_milli.store(100, Ordering::Relaxed);
        let (client, server) = Pipe::pair();
        serve(server, replay, ctl.clone(), 0);
        let mut c = Connection::local(client);
        c.send(&[0]).unwrap();
        c.send(&[1]).unwrap();
        assert_eq!(c.recv().unwrap().0, 30);
        std::thread::sleep(Duration::from_millis(50));
        ctl.seek.store(0, Ordering::Release);
        for _ in 0..200 {
            if ctl.restart.load(Ordering::Acquire) >= 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ctl.restart.load(Ordering::Acquire), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
