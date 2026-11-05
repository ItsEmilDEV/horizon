//! Real map colours: the average of a block's top texture, tinted by the biome
//! the way the game tints grass, leaves and water, looked up lazily per block state.
use crate::{minimap::Colorize, streaming::WorkerContext};
use horizon_assets::model::Direction;
use horizon_core::position::BlockPos;
use horizon_world::registry::BlockStateId;
use std::sync::{Arc, RwLock};

const KNOWN: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tint {
    None,
    Grass,
    Foliage,
    Dry,
    Water,
    Constant(u32),
}
impl Tint {
    fn code(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Grass => 1,
            Self::Foliage => 2,
            Self::Dry => 3,
            Self::Water => 4,
            Self::Constant(_) => 5,
        }
    }
}

pub struct BlockColors {
    context: Arc<WorkerContext>,
    entries: RwLock<Vec<(u32, u32)>>,
}

/// The mean colour of an RGBA square, weighted by alpha, as 0xRRGGBB; none when it is all clear.
pub fn average_rgb(rgba: &[u8]) -> Option<u32> {
    let (mut r, mut g, mut b, mut weight) = (0u64, 0u64, 0u64, 0u64);
    for p in rgba.as_chunks::<4>().0 {
        let a = u64::from(p[3]);
        r += u64::from(p[0]) * a;
        g += u64::from(p[1]) * a;
        b += u64::from(p[2]) * a;
        weight += a;
    }
    if weight == 0 {
        return None;
    }
    Some(((r / weight) as u32) << 16 | ((g / weight) as u32) << 8 | (b / weight) as u32)
}

/// Textures average darker than a map should look: brighten the colour a little.
pub fn lift(rgb: u32) -> u32 {
    let ch = |s: u32| ((((rgb >> s) & 255) * 5 / 4).min(255)) << s;
    ch(16) | ch(8) | ch(0)
}

/// A colour times a tint, per channel.
pub fn multiply(base: u32, tint: u32) -> u32 {
    let ch = |s: u32| (((base >> s) & 255) * ((tint >> s) & 255) / 255) << s;
    ch(16) | ch(8) | ch(0)
}

fn tint_for(name: &str) -> Tint {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    match name {
        "spruce_leaves" => Tint::Constant(0x61_9961),
        "birch_leaves" => Tint::Constant(0x80_A755),
        "lily_pad" => Tint::Constant(0x20_8030),
        "leaf_litter" => Tint::Dry,
        "water" | "bubble_column" | "water_cauldron" => Tint::Water,
        "vine" => Tint::Foliage,
        _ if name.ends_with("_leaves") => Tint::Foliage,
        _ => Tint::Grass,
    }
}

impl BlockColors {
    pub fn new(context: Arc<WorkerContext>, states: usize) -> Self {
        Self {
            context,
            entries: RwLock::new(vec![(0, 0); states]),
        }
    }

    fn layer_rgb(&self, layer: u32) -> Option<u32> {
        let render = &self.context.render;
        let size = render.sprite_size as usize;
        let bytes = size * size * 4;
        let start = layer as usize * bytes;
        render.pixels.get(start..start + bytes).and_then(average_rgb)
    }

    fn sprite_rgb(&self, sprite: &str) -> Option<u32> {
        let layers = &self.context.render.sprite_layers;
        let layer = layers
            .get(sprite)
            .or_else(|| layers.get(&format!("minecraft:{sprite}")))?;
        self.layer_rgb(*layer)
    }

    fn compute(&self, id: u32) -> (u32, u32) {
        let state = BlockStateId(id);
        let name = self
            .context
            .assets
            .registry
            .state(state)
            .map_or("", |s| s.name.as_str());
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "water" | "bubble_column" => {
                let base = self.sprite_rgb("block/water_still").unwrap_or(0x3F76E4);
                return (base, Tint::Water.code());
            }
            "lava" => return (self.sprite_rgb("block/lava_still").unwrap_or(0xCF5A0F), 0),
            _ => {}
        }
        let catalog = &self.context.render.model_catalog;
        let models = catalog.collect(state, BlockPos::new(0, 0, 0)).ok();
        let faces = || models.iter().flatten().flat_map(|(_, m)| &m.faces);
        let face = faces()
            .find(|f| f.quad.facing == Direction::Up)
            .or_else(|| faces().next());
        let layer = face.map(|f| f.layer).or_else(|| catalog.particle(state));
        let Some(base) = layer.and_then(|l| self.layer_rgb(l)) else {
            return (0, 0);
        };
        let tint = if face.is_some_and(|f| f.quad.tint_index >= 0) {
            tint_for(name)
        } else {
            Tint::None
        };
        match tint {
            Tint::Constant(c) => (multiply(base, c), 0),
            other => (base, other.code()),
        }
    }

    fn entry(&self, id: u32) -> (u32, u32) {
        if let Some(e) = self.entries.read().ok().and_then(|v| v.get(id as usize).copied())
            && e.0 & KNOWN != 0
        {
            return e;
        }
        let (rgb, tint) = self.compute(id);
        let e = (KNOWN | rgb, tint);
        if let Ok(mut v) = self.entries.write()
            && let Some(slot) = v.get_mut(id as usize)
        {
            *slot = e;
        }
        e
    }
}

impl Colorize for BlockColors {
    fn column_rgb(&self, state: u32, biome: u16, x: i32, z: i32) -> u32 {
        let (known, tint) = self.entry(state);
        let base = known & 0x00FF_FFFF;
        if known & 0x00FF_FFFF == 0 && tint == 0 {
            return 0;
        }
        let biomes = &self.context.biomes;
        let info = biomes.get(biome);
        let tinted = match tint {
            1 => multiply(base, biomes.grass(biome, f64::from(x), f64::from(z))),
            2 => multiply(base, info.foliage),
            3 => multiply(base, info.dry_foliage),
            4 => multiply(base, info.water),
            _ => base,
        };
        lift(tinted).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_average_weighs_by_alpha_and_skips_clear_pixels() {
        let px = [255, 0, 0, 255, 0, 0, 255, 255, 9, 9, 9, 0];
        assert_eq!(average_rgb(&px), Some(0x7F_0000 | 0x7F));
        assert_eq!(average_rgb(&[1, 2, 3, 0]), None);
    }

    #[test]
    fn lifting_brightens_and_stops_at_white() {
        assert_eq!(lift(0x404040), 0x505050);
        assert_eq!(lift(0xF0F0F0), 0xFFFFFF);
        assert_eq!(lift(0), 0);
    }

    #[test]
    fn tints_multiply_per_channel() {
        assert_eq!(multiply(0xFFFFFF, 0x79C05A), 0x79C05A);
        assert_eq!(multiply(0x808080, 0xFFFFFF), 0x808080);
        assert_eq!(multiply(0xFFFFFF, 0), 0);
    }

    #[test]
    fn tint_kinds_follow_the_block() {
        assert_eq!(tint_for("minecraft:oak_leaves"), Tint::Foliage);
        assert_eq!(tint_for("minecraft:birch_leaves"), Tint::Constant(0x80_A755));
        assert_eq!(tint_for("minecraft:water"), Tint::Water);
        assert_eq!(tint_for("minecraft:grass_block"), Tint::Grass);
        assert_eq!(tint_for("minecraft:vine"), Tint::Foliage);
    }
}
