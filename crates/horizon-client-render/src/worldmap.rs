use crate::{
    fast_hash::FxHashMap,
    minimap::{Tile, View, paint},
};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const MAGIC: &[u8; 4] = b"BWM1";
const RECORD: usize = 8 + 256 + 512 + 256;
const MAX_RAW: u64 = 512 * 1024 * 1024;
static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Default)]
pub struct Worldmap {
    tiles: FxHashMap<(i32, i32), Tile>,
    dirty: bool,
}

impl Worldmap {
    pub fn len(&self) -> usize {
        self.tiles.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn absorb(&mut self, scanned: Vec<((i32, i32), Tile)>) {
        for (key, tile) in scanned {
            if !tile.surface() {
                continue;
            }
            let same = self
                .tiles
                .get(&key)
                .is_some_and(|t| t.color == tile.color && t.height == tile.height && t.depth == tile.depth);
            if !same {
                self.tiles.insert(key, tile);
                self.dirty = true;
            }
        }
    }
    pub fn compose(&self, view: View, size: usize, image: &mut Vec<u8>) {
        paint(image, size, view, |x, z| {
            let t = self.tiles.get(&(x >> 4, z >> 4))?;
            let i = ((z & 15) * 16 + (x & 15)) as usize;
            Some((t.color[i], t.height[i], t.depth[i]))
        });
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(self.tiles.len() * RECORD);
        let mut keys: Vec<_> = self.tiles.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let t = &self.tiles[&key];
            raw.extend_from_slice(&key.0.to_le_bytes());
            raw.extend_from_slice(&key.1.to_le_bytes());
            raw.extend_from_slice(&t.color);
            for h in t.height {
                raw.extend_from_slice(&h.to_le_bytes());
            }
            raw.extend_from_slice(&t.depth);
        }
        let mut out = MAGIC.to_vec();
        let mut z = ZlibEncoder::new(&mut out, Compression::fast());
        let _ = z.write_all(&raw);
        let _ = z.finish();
        out
    }
    pub fn decode(bytes: &[u8]) -> Self {
        let mut map = Self::default();
        let Some(body) = bytes.strip_prefix(MAGIC.as_slice()) else {
            return map;
        };
        let mut raw = Vec::new();
        if ZlibDecoder::new(body).take(MAX_RAW).read_to_end(&mut raw).is_err() {
            return map;
        }
        for rec in raw.chunks_exact(RECORD) {
            let cx = i32::from_le_bytes(rec[0..4].try_into().unwrap());
            let cz = i32::from_le_bytes(rec[4..8].try_into().unwrap());
            let mut color = [0u8; 256];
            color.copy_from_slice(&rec[8..264]);
            let mut height = [0i16; 256];
            for (i, h) in height.iter_mut().enumerate() {
                let o = 264 + i * 2;
                *h = i16::from_le_bytes([rec[o], rec[o + 1]]);
            }
            let mut depth = [0u8; 256];
            depth.copy_from_slice(&rec[776..1032]);
            map.tiles
                .insert((cx, cz), Tile::from_columns(color, height, depth));
        }
        map
    }
    pub fn load(path: &Path) -> Self {
        fs::read(path).map(|b| Self::decode(&b)).unwrap_or_default()
    }
    pub fn save_in_background(&mut self, path: &Path) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut copy = Self {
            tiles: self.tiles.clone(),
            dirty: true,
        };
        let path = path.to_path_buf();
        std::thread::spawn(move || copy.save(&path));
    }
    pub fn save(&mut self, path: &Path) {
        if !self.dirty {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = path.with_extension(format!("{}.tmp", SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        if fs::write(&tmp, self.encode()).is_ok() && fs::rename(&tmp, path).is_ok() {
            self.dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(color: u8, height: i16) -> Tile {
        Tile::from_columns([color; 256], [height; 256], [0; 256])
    }

    #[test]
    fn tiles_round_trip_through_the_file_format() {
        let mut m = Worldmap::default();
        m.absorb(vec![((0, 0), tile(1, 64)), ((-3, 7), tile(12, 63))]);
        assert!(m.is_dirty());
        let back = Worldmap::decode(&m.encode());
        assert_eq!(back.len(), 2);
        let t = &back.tiles[&(-3, 7)];
        assert_eq!((t.color[5], t.height[200]), (12, 63));
    }

    #[test]
    fn unchanged_tiles_do_not_dirty_the_map_and_garbage_loads_empty() {
        let mut m = Worldmap::default();
        m.absorb(vec![((1, 1), tile(1, 64))]);
        m.dirty = false;
        m.absorb(vec![((1, 1), tile(1, 64))]);
        assert!(!m.is_dirty());
        m.absorb(vec![((1, 1), tile(2, 64))]);
        assert!(m.is_dirty());
        assert!(Worldmap::decode(b"nope").is_empty());
        assert!(Worldmap::decode(b"BWM1garbage").is_empty());
    }

    #[test]
    fn composing_paints_known_columns_and_leaves_the_rest_unknown() {
        let mut m = Worldmap::default();
        m.absorb(vec![((0, 0), tile(1, 64))]);
        let view = View {
            x: 8.0,
            z: 8.0,
            yaw: 0.0,
            rotate: false,
            blocks_per_pixel: 1.0,
            round: false,
        };
        let mut image = Vec::new();
        m.compose(view, 64, &mut image);
        assert_eq!(image.len(), 64 * 64 * 4);
        let center = (32 * 64 + 32) * 4;
        assert_ne!(image[center + 3], 0);
        let known = image[center..center + 4].to_vec();
        let far = (2 * 64 + 60) * 4;
        assert_ne!(&image[far..far + 4], known.as_slice());
    }
}
