//! Building plans: `.schem` (Sponge, versions 2 and 3) and `.litematic` files read into one grid of block states, and the
//! comparison of that grid with the world. Nothing here touches the game: it is the reading and the arithmetic.
use horizon_world::nbt::{self, Limits, Tag};
use std::{collections::BTreeMap, io::Read};

/// A block state a plan asks for: the block and the properties the plan names (others do not matter).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Spec {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

impl Spec {
    pub fn is_air(&self) -> bool {
        matches!(self.name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
    }

    /// `minecraft:oak_stairs[facing=north,half=bottom]` or just `minecraft:stone`.
    pub fn parse(text: &str) -> Self {
        let (name, rest) = match text.split_once('[') {
            Some((n, r)) => (n, r.trim_end_matches(']')),
            None => (text, ""),
        };
        let properties = rest
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect();
        Self { name: name.trim().to_owned(), properties }
    }

    /// Whether a world block state is this: the same block, and every property the plan names equal.
    pub fn matches(&self, name: &str, properties: &BTreeMap<String, String>) -> bool {
        self.name == name && self.properties.iter().all(|(k, v)| properties.get(k) == Some(v))
    }
}

/// A plan: its size and a palette index for every cell, `x + z * size_x + y * size_x * size_z`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Blueprint {
    pub size: [i32; 3],
    pub palette: Vec<Spec>,
    pub cells: Vec<u16>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    NotAPlan(String),
    TooBig(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAPlan(s) | Self::TooBig(s) => f.write_str(s),
        }
    }
}

/// The most cells a plan may have (a 256 x 256 x 256 build is 16.7 million).
const MAX_CELLS: usize = 1 << 24;

impl Blueprint {
    pub fn index(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let [sx, sy, sz] = self.size;
        ((0..sx).contains(&x) && (0..sy).contains(&y) && (0..sz).contains(&z)).then(|| (x + z * sx + y * sx * sz) as usize)
    }

    pub fn spec_at(&self, x: i32, y: i32, z: i32) -> Option<&Spec> {
        self.palette.get(usize::from(*self.cells.get(self.index(x, y, z)?)?))
    }

    /// How many cells of each block the plan needs (air left out), most first.
    pub fn materials(&self) -> Vec<(String, u32)> {
        let mut by_index = vec![0u32; self.palette.len()];
        for c in &self.cells {
            if let Some(n) = by_index.get_mut(usize::from(*c)) {
                *n += 1;
            }
        }
        let mut by_name: BTreeMap<&str, u32> = BTreeMap::new();
        for (spec, n) in self.palette.iter().zip(by_index) {
            if !spec.is_air() && n > 0 {
                *by_name.entry(&spec.name).or_default() += n;
            }
        }
        let mut list: Vec<(String, u32)> = by_name.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        list
    }

    /// A plan from the bytes of a `.schem` or `.litematic` file (gzip-compressed NBT).
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut raw)
            .map_err(|_| Error::NotAPlan("The file is not a compressed plan.".into()))?;
        let (root, _) = nbt::read_named(&raw, Limits::default()).map_err(|e| Error::NotAPlan(format!("The plan could not be read: {e}")))?;
        let tag = &root.tag;
        if tag.get("Regions").is_some() {
            Self::from_litematic(tag)
        } else {
            // Sponge 3 puts everything under "Schematic".
            Self::from_sponge(tag.get("Schematic").unwrap_or(tag))
        }
    }

    fn check_size(size: [i32; 3]) -> Result<usize, Error> {
        if size.iter().any(|s| *s <= 0) {
            return Err(Error::NotAPlan("The plan has no size.".into()));
        }
        let cells = size.iter().map(|s| *s as usize).try_fold(1usize, |a, s| a.checked_mul(s)).filter(|c| *c <= MAX_CELLS);
        cells.ok_or_else(|| Error::TooBig("The plan is larger than 16 million blocks.".into()))
    }

    fn from_sponge(tag: &Tag) -> Result<Self, Error> {
        let short = |k: &str| match tag.get(k) {
            Some(Tag::Short(v)) => Some(i32::from(*v)),
            Some(Tag::Int(v)) => Some(*v),
            _ => None,
        };
        let size = [short("Width"), short("Height"), short("Length")];
        let [Some(w), Some(h), Some(l)] = size else { return Err(Error::NotAPlan("It is not a Sponge plan (no Width, Height, Length).".into())) };
        let total = Self::check_size([w, h, l])?;
        // Version 3 keeps the blocks under "Blocks", version 2 at the top.
        let blocks = tag.get("Blocks").unwrap_or(tag);
        let Some(Tag::Compound(palette)) = blocks.get("Palette") else { return Err(Error::NotAPlan("The plan has no block palette.".into())) };
        let data = match blocks.get("Data").or_else(|| blocks.get("BlockData")) {
            Some(Tag::ByteArray(d)) => d,
            _ => return Err(Error::NotAPlan("The plan has no block data.".into())),
        };
        let mut specs = vec![Spec::default(); palette.len()];
        for (name, index) in palette {
            let Tag::Int(i) = index else { continue };
            if let Some(slot) = usize::try_from(*i).ok().and_then(|i| specs.get_mut(i)) {
                *slot = Spec::parse(&name.to_string_lossy());
            }
        }
        // The data is a run of variable-length integers, one per cell.
        let mut cells = Vec::with_capacity(total);
        let (mut value, mut shift) = (0u32, 0u32);
        for byte in data {
            value |= u32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                cells.push(u16::try_from(value).map_err(|_| Error::NotAPlan("A palette index is too large.".into()))?);
                (value, shift) = (0, 0);
            } else {
                shift += 7;
                if shift > 28 {
                    return Err(Error::NotAPlan("The block data is damaged.".into()));
                }
            }
        }
        if cells.len() != total {
            return Err(Error::NotAPlan(format!("The block data has {} entries, the size says {total}.", cells.len())));
        }
        Ok(Self { size: [w, h, l], palette: specs, cells })
    }

    fn from_litematic(tag: &Tag) -> Result<Self, Error> {
        let Some(Tag::Compound(regions)) = tag.get("Regions") else { return Err(Error::NotAPlan("The plan has no regions.".into())) };
        struct Part {
            origin: [i32; 3],
            size: [i32; 3],
            palette: Vec<Spec>,
            ids: Vec<u16>,
        }
        let int3 = |t: Option<&Tag>| -> Option<[i32; 3]> {
            let t = t?;
            let v = |k: &str| match t.get(k) {
                Some(Tag::Int(v)) => Some(*v),
                _ => None,
            };
            Some([v("x")?, v("y")?, v("z")?])
        };
        let mut parts = Vec::new();
        for region in regions.values() {
            let (Some(pos), Some(size)) = (int3(region.get("Position")), int3(region.get("Size"))) else { continue };
            // A negative size runs the region the other way from its position.
            let abs = [size[0].abs(), size[1].abs(), size[2].abs()];
            let origin = [pos[0] + (size[0] + 1).min(0), pos[1] + (size[1] + 1).min(0), pos[2] + (size[2] + 1).min(0)];
            let total = Self::check_size(abs)?;
            let Some(Tag::List(palette_tags)) = region.get("BlockStatePalette") else { return Err(Error::NotAPlan("A region has no palette.".into())) };
            let palette: Vec<Spec> = palette_tags
                .iter()
                .map(|t| {
                    let name = match t.get("Name") {
                        Some(Tag::String(n)) => n.to_string_lossy(),
                        _ => String::new(),
                    };
                    let properties = match t.get("Properties") {
                        Some(Tag::Compound(p)) => p
                            .iter()
                            .filter_map(|(k, v)| match v {
                                Tag::String(v) => Some((k.to_string_lossy(), v.to_string_lossy())),
                                _ => None,
                            })
                            .collect(),
                        _ => BTreeMap::new(),
                    };
                    Spec { name, properties }
                })
                .collect();
            let Some(Tag::LongArray(longs)) = region.get("BlockStates") else { return Err(Error::NotAPlan("A region has no block states.".into())) };
            let ids = unpack_bits(longs, palette.len(), total)?;
            parts.push(Part { origin, size: abs, palette, ids });
        }
        if parts.is_empty() {
            return Err(Error::NotAPlan("The plan has no regions.".into()));
        }
        // All regions go into one grid that starts at the lowest corner.
        let min = [0, 1, 2].map(|a| parts.iter().map(|p| p.origin[a]).min().unwrap_or(0));
        let max = [0, 1, 2].map(|a| parts.iter().map(|p| p.origin[a] + p.size[a]).max().unwrap_or(0));
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let total = Self::check_size(size)?;
        let mut palette = vec![Spec { name: "minecraft:air".into(), ..Spec::default() }];
        let mut cells = vec![0u16; total];
        for p in parts {
            let map: Vec<u16> = p
                .palette
                .iter()
                .map(|s| match palette.iter().position(|e| e == s) {
                    Some(i) => i as u16,
                    None => {
                        palette.push(s.clone());
                        (palette.len() - 1) as u16
                    }
                })
                .collect();
            for y in 0..p.size[1] {
                for z in 0..p.size[2] {
                    for x in 0..p.size[0] {
                        let from = (x + z * p.size[0] + y * p.size[0] * p.size[2]) as usize;
                        let (gx, gy, gz) = (x + p.origin[0] - min[0], y + p.origin[1] - min[1], z + p.origin[2] - min[2]);
                        let to = (gx + gz * size[0] + gy * size[0] * size[2]) as usize;
                        cells[to] = map.get(usize::from(p.ids[from])).copied().unwrap_or(0);
                    }
                }
            }
        }
        if palette.len() > usize::from(u16::MAX) {
            return Err(Error::TooBig("The plan has too many kinds of blocks.".into()));
        }
        Ok(Self { size, palette, cells })
    }
}

/// Litematica's packed ids: `count` values of `bits` bits (at least 2, enough for the palette) one after the other across
/// the longs, a value may start in one long and end in the next.
pub fn unpack_bits(longs: &[i64], palette: usize, count: usize) -> Result<Vec<u16>, Error> {
    let bits = (usize::BITS - palette.saturating_sub(1).leading_zeros()).max(2) as usize;
    if bits > 16 || longs.len() * 64 < count * bits {
        return Err(Error::NotAPlan("The block states are shorter than the size says.".into()));
    }
    let mask = (1u64 << bits) - 1;
    Ok((0..count)
        .map(|i| {
            let start = i * bits;
            let (word, shift) = (start / 64, start % 64);
            let mut v = (longs[word] as u64) >> shift;
            if shift + bits > 64 {
                v |= (longs[word + 1] as u64) << (64 - shift);
            }
            (v & mask) as u16
        })
        .collect())
}

/// Packs ids the way Litematica does (for the tests, and for anyone who wants to write a plan).
pub fn pack_bits(ids: &[u16], palette: usize) -> Vec<i64> {
    let bits = (usize::BITS - palette.saturating_sub(1).leading_zeros()).max(2) as usize;
    let mut longs = vec![0i64; (ids.len() * bits).div_ceil(64).max(1)];
    for (i, id) in ids.iter().enumerate() {
        let start = i * bits;
        let (word, shift) = (start / 64, start % 64);
        longs[word] |= (u64::from(*id) << shift) as i64;
        if shift + bits > 64 {
            longs[word + 1] |= (u64::from(*id) >> (64 - shift)) as i64;
        }
    }
    longs
}

/// What the world has at a plan's cell, for the comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The plan wants nothing here.
    Free,
    /// The world has the block the plan wants.
    Done,
    /// Nothing is there (air, or something that gives way): a block is missing.
    Missing,
    /// A different block is there.
    Wrong,
}

pub fn status(wanted: &Spec, world_air_or_replaceable: bool, world: Option<(&str, &BTreeMap<String, String>)>) -> Status {
    if wanted.is_air() {
        return Status::Free;
    }
    match world {
        Some((name, props)) if wanted.matches(name, props) => Status::Done,
        _ if world_air_or_replaceable => Status::Missing,
        _ => Status::Wrong,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_world::nbt::{Compound, JavaString, NamedTag};
    use std::io::Write;

    fn gzip(root: Tag) -> Vec<u8> {
        let raw = nbt::write_named(&NamedTag { name: JavaString::from(""), tag: root }, Limits::saved()).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap()
    }

    fn varints(values: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &v in values {
            let mut v = v;
            loop {
                let byte = (v & 0x7F) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(byte);
                    break;
                }
                out.push(byte | 0x80);
            }
        }
        out
    }

    #[test]
    fn a_block_state_text_is_read_and_compared_by_the_properties_it_names() {
        let s = Spec::parse("minecraft:oak_stairs[facing=north,half=bottom]");
        assert_eq!((s.name.as_str(), s.properties.len()), ("minecraft:oak_stairs", 2));
        let world: BTreeMap<String, String> = [("facing", "north"), ("half", "bottom"), ("shape", "straight")].map(|(k, v)| (k.to_owned(), v.to_owned())).into();
        assert!(s.matches("minecraft:oak_stairs", &world));
        assert!(!s.matches("minecraft:oak_slab", &world));
        let mut other = world.clone();
        other.insert("facing".into(), "south".into());
        assert!(!s.matches("minecraft:oak_stairs", &other));
        assert!(Spec::parse("minecraft:stone").matches("minecraft:stone", &BTreeMap::new()));
        assert!(Spec::parse("minecraft:air").is_air());
    }

    #[test]
    fn a_sponge_two_plan_reads_with_its_size_palette_and_cells() {
        // 2 x 1 x 2: stone, air / air, stairs (x + z * 2).
        let palette = Tag::compound([("minecraft:air", Tag::Int(0)), ("minecraft:stone", Tag::Int(1)), ("minecraft:oak_stairs[facing=east]", Tag::Int(2))]);
        let root = Tag::compound([
            ("Version", Tag::Int(2)),
            ("Width", Tag::Short(2)),
            ("Height", Tag::Short(1)),
            ("Length", Tag::Short(2)),
            ("Palette", palette),
            ("BlockData", Tag::ByteArray(varints(&[1, 0, 0, 2]))),
        ]);
        let plan = Blueprint::read(&gzip(root)).unwrap();
        assert_eq!(plan.size, [2, 1, 2]);
        assert_eq!(plan.spec_at(0, 0, 0).map(|s| s.name.as_str()), Some("minecraft:stone"));
        assert!(plan.spec_at(1, 0, 0).unwrap().is_air());
        assert_eq!(plan.spec_at(1, 0, 1).unwrap().properties.get("facing").map(String::as_str), Some("east"));
        assert_eq!(plan.spec_at(2, 0, 0), None);
        assert_eq!(plan.materials(), vec![("minecraft:oak_stairs".to_owned(), 1), ("minecraft:stone".to_owned(), 1)]);
    }

    #[test]
    fn a_sponge_three_plan_keeps_its_blocks_one_level_down() {
        let blocks = Tag::compound([
            ("Palette", Tag::compound([("minecraft:stone", Tag::Int(0))])),
            ("Data", Tag::ByteArray(varints(&[0, 0, 0, 0]))),
        ]);
        let schematic = Tag::compound([("Version", Tag::Int(3)), ("Width", Tag::Short(4)), ("Height", Tag::Short(1)), ("Length", Tag::Short(1)), ("Blocks", blocks)]);
        let plan = Blueprint::read(&gzip(Tag::compound([("Schematic", schematic)]))).unwrap();
        assert_eq!(plan.size, [4, 1, 1]);
        assert_eq!(plan.materials(), vec![("minecraft:stone".to_owned(), 4)]);
    }

    #[test]
    fn long_runs_use_several_bytes_per_index_and_a_wrong_count_is_refused() {
        // 200 distinct indices need two bytes each.
        let mut palette = Compound::new();
        for i in 0..200 {
            palette.insert(JavaString::from(format!("minecraft:b{i}").as_str()), Tag::Int(i));
        }
        let ids: Vec<u32> = (0..200).collect();
        let mk = |data: Vec<u8>, w: i16| {
            gzip(Tag::compound([
                ("Width", Tag::Short(w)),
                ("Height", Tag::Short(1)),
                ("Length", Tag::Short(1)),
                ("Palette", Tag::Compound(palette.clone())),
                ("BlockData", Tag::ByteArray(data)),
            ]))
        };
        let plan = Blueprint::read(&mk(varints(&ids), 200)).unwrap();
        assert_eq!(plan.spec_at(199, 0, 0).map(|s| s.name.as_str()), Some("minecraft:b199"));
        assert!(matches!(Blueprint::read(&mk(varints(&ids), 199)), Err(Error::NotAPlan(_))));
    }

    #[test]
    fn packed_ids_round_trip_across_long_boundaries() {
        for palette in [3usize, 5, 17, 300] {
            let ids: Vec<u16> = (0..333u32).map(|i| (i % palette as u32) as u16).collect();
            let longs = pack_bits(&ids, palette);
            assert_eq!(unpack_bits(&longs, palette, ids.len()).unwrap(), ids, "palette {palette}");
        }
        assert!(unpack_bits(&[0], 3, 1000).is_err());
    }

    #[test]
    fn a_litematic_plan_with_two_regions_becomes_one_grid() {
        let state = |name: &str| Tag::compound([("Name", Tag::String(JavaString::from(name)))]);
        let region = |x: i32, ids: &[u16]| {
            Tag::compound([
                ("Position", Tag::compound([("x", Tag::Int(x)), ("y", Tag::Int(0)), ("z", Tag::Int(0))])),
                ("Size", Tag::compound([("x", Tag::Int(2)), ("y", Tag::Int(1)), ("z", Tag::Int(1))])),
                ("BlockStatePalette", Tag::List(vec![state("minecraft:air"), state("minecraft:stone"), state("minecraft:glass")])),
                ("BlockStates", Tag::LongArray(pack_bits(ids, 3))),
            ])
        };
        let root = Tag::compound([("Regions", Tag::compound([("a", region(0, &[1, 0])), ("b", region(2, &[2, 2]))]))]);
        let plan = Blueprint::read(&gzip(root)).unwrap();
        assert_eq!(plan.size, [4, 1, 1]);
        let names: Vec<&str> = (0..4).map(|x| plan.spec_at(x, 0, 0).unwrap().name.as_str()).collect();
        assert_eq!(names, ["minecraft:stone", "minecraft:air", "minecraft:glass", "minecraft:glass"]);
    }

    #[test]
    fn what_is_not_a_plan_is_said_so() {
        assert!(Blueprint::read(b"nonsense").is_err());
        assert!(Blueprint::read(&gzip(Tag::compound([("Foo", Tag::Int(1))]))).is_err());
        let huge = gzip(Tag::compound([
            ("Width", Tag::Short(30000)),
            ("Height", Tag::Short(30000)),
            ("Length", Tag::Short(30000)),
            ("Palette", Tag::compound([("minecraft:stone", Tag::Int(0))])),
            ("BlockData", Tag::ByteArray(vec![0])),
        ]));
        assert!(matches!(Blueprint::read(&huge), Err(Error::TooBig(_))));
    }

    #[test]
    fn the_status_compares_the_plan_with_the_world() {
        let stone = Spec::parse("minecraft:stone");
        let none = BTreeMap::new();
        assert_eq!(status(&Spec::parse("minecraft:air"), false, Some(("minecraft:dirt", &none))), Status::Free);
        assert_eq!(status(&stone, false, Some(("minecraft:stone", &none))), Status::Done);
        assert_eq!(status(&stone, true, Some(("minecraft:air", &none))), Status::Missing);
        assert_eq!(status(&stone, false, Some(("minecraft:dirt", &none))), Status::Wrong);
        assert_eq!(status(&stone, true, None), Status::Missing);
    }
}
