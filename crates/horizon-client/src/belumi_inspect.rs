use horizon_protocol::{Direction, State};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

const RECENT: usize = 12;
const TOP: usize = 4;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static INNER: OnceLock<Mutex<Inner>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub direction: Direction,
    pub name: &'static str,
    pub bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub in_packets: u32,
    pub in_bytes: usize,
    pub out_packets: u32,
    pub out_bytes: usize,
    pub top: Vec<(&'static str, u32, usize)>,
    pub recent: Vec<Packet>,
}

#[derive(Default)]
struct Inner {
    window: VecDeque<(f64, Packet)>,
    recent: VecDeque<Packet>,
}

impl Inner {
    fn push(&mut self, now: f64, packet: Packet) {
        self.forget(now);
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(packet.clone());
        self.window.push_back((now, packet));
    }
    fn forget(&mut self, now: f64) {
        while self.window.front().is_some_and(|(t, _)| now - t >= 1.0) {
            self.window.pop_front();
        }
    }
    fn snapshot(&mut self, now: f64) -> Snapshot {
        self.forget(now);
        let mut s = Snapshot::default();
        let mut by_name: HashMap<&'static str, (u32, usize)> = HashMap::new();
        for (_, p) in &self.window {
            match p.direction {
                Direction::Clientbound => {
                    s.in_packets += 1;
                    s.in_bytes += p.bytes;
                }
                Direction::Serverbound => {
                    s.out_packets += 1;
                    s.out_bytes += p.bytes;
                }
            }
            let e = by_name.entry(p.name).or_default();
            e.0 += 1;
            e.1 += p.bytes;
        }
        let mut top: Vec<_> = by_name.into_iter().map(|(n, (c, b))| (n, c, b)).collect();
        top.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(b.0)));
        top.truncate(TOP);
        s.top = top;
        s.recent = self.recent.iter().cloned().collect();
        s
    }
}

fn now() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn inner() -> &'static Mutex<Inner> {
    INNER.get_or_init(Default::default)
}

pub fn set_active(on: bool) {
    ACTIVE.store(on, Ordering::Relaxed);
    if !on && let Ok(mut i) = inner().lock() {
        *i = Inner::default();
    }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn record(direction: Direction, id: i32, bytes: usize) {
    if !is_active() {
        return;
    }
    let name = horizon_protocol::ids::name(State::Play, direction, id).unwrap_or("?");
    let t = now();
    if let Ok(mut i) = inner().lock() {
        i.push(t, Packet { direction, name, bytes });
    }
}

pub fn record_encoded(direction: Direction, encoded: &[u8]) {
    if !is_active() {
        return;
    }
    if let Some((id, _)) = read_varint(encoded) {
        record(direction, id, encoded.len());
    }
}

pub fn snapshot() -> Snapshot {
    let t = now();
    inner().lock().map(|mut i| i.snapshot(t)).unwrap_or_default()
}

fn read_varint(bytes: &[u8]) -> Option<(i32, usize)> {
    let mut value = 0i32;
    for (i, b) in bytes.iter().take(5).enumerate() {
        value |= i32::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

pub fn format_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(direction: Direction, name: &'static str, bytes: usize) -> Packet {
        Packet { direction, name, bytes }
    }

    #[test]
    fn counts_the_last_second_and_ranks_by_bytes() {
        let mut i = Inner::default();
        i.push(0.0, p(Direction::Clientbound, "old", 999));
        i.push(1.5, p(Direction::Clientbound, "chunk", 4000));
        i.push(1.6, p(Direction::Clientbound, "move", 20));
        i.push(1.7, p(Direction::Clientbound, "move", 20));
        i.push(1.8, p(Direction::Serverbound, "pos", 30));
        let s = i.snapshot(2.0);
        assert_eq!((s.in_packets, s.in_bytes), (3, 4040));
        assert_eq!((s.out_packets, s.out_bytes), (1, 30));
        assert_eq!(s.top[0], ("chunk", 1, 4000));
        assert_eq!(s.top[1], ("move", 2, 40));
        assert_eq!(s.recent.len(), 5);
    }

    #[test]
    fn recent_is_capped_and_old_entries_expire() {
        let mut i = Inner::default();
        for n in 0..40 {
            i.push(0.0, p(Direction::Clientbound, "a", n));
        }
        assert_eq!(i.snapshot(0.1).recent.len(), RECENT);
        assert_eq!(i.snapshot(5.0).in_packets, 0);
    }

    #[test]
    fn varint_ids_read_from_encoded_packets() {
        assert_eq!(read_varint(&[0x05, 1, 2]), Some((5, 1)));
        assert_eq!(read_varint(&[0x80, 0x01]), Some((128, 2)));
        assert_eq!(read_varint(&[0x80; 6]), None);
        assert_eq!(read_varint(&[]), None);
    }

    #[test]
    fn byte_sizes_read_naturally() {
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
    }
}
