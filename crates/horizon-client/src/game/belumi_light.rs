//! Light overlay: an X on the floor where a monster can spawn. Red: it is dark here at any hour (no block light and no sky
//! light). Yellow: no block light, but the sky lights it by day; monsters can spawn here at night. Anywhere with block light is
//! safe and shows nothing. Only floors with room for a monster (two blocks of air above a solid block) count.
use super::Game;
use horizon_client::gizmos::{Gizmos, Line};
use std::time::{Duration, Instant};

const RADIUS: i32 = 12;
const HEIGHT: i32 = 4;
const EVERY: Duration = Duration::from_millis(500);
const MAX_SPOTS: usize = 800;
const RED: u32 = 0xFFFF_3030;
const YELLOW: u32 = 0xFFFF_E030;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Level {
    /// Dark at any hour.
    Always,
    /// Dark only at night.
    Night,
}

/// What a light byte (sky light in the high four bits, block light in the low four) means for spawning.
pub(super) fn classify(light: u8) -> Option<Level> {
    let (sky, block) = (light >> 4, light & 15);
    if block > 0 {
        None
    } else if sky == 0 {
        Some(Level::Always)
    } else {
        Some(Level::Night)
    }
}

/// The two strokes of an X lying on the floor cell `(x, y, z)` (the cell above the floor block).
pub(super) fn cross(x: i32, y: i32, z: i32, color: u32) -> [Line; 2] {
    let (fx, fy, fz) = (f64::from(x), f64::from(y) + 0.02, f64::from(z));
    let m = 0.15;
    [
        Line { start: [fx + m, fy, fz + m], end: [fx + 1.0 - m, fy, fz + 1.0 - m], color, width: 2.0 },
        Line { start: [fx + m, fy, fz + 1.0 - m], end: [fx + 1.0 - m, fy, fz + m], color, width: 2.0 },
    ]
}

#[derive(Default)]
pub(super) struct LightOverlay {
    spots: Vec<([i32; 3], Level)>,
    scanned: Option<Instant>,
}

impl Game<'_> {
    pub(super) fn belumi_light_frame(&mut self) {
        let on = self.modules_ref().is_some_and(|m| m.misc.light_overlay);
        if !on || self.player.is_none() {
            self.belumi_light.spots.clear();
            return;
        }
        if self.belumi_light.scanned.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.belumi_light.scanned = Some(Instant::now());
        let Some(p) = self.player.as_ref() else { return };
        let (cx, cy, cz) = (p.position.x.floor() as i32, p.position.y.floor() as i32, p.position.z.floor() as i32);
        let facts = self.res.facts();
        let solid = |x: i32, y: i32, z: i32| {
            let id = self.world.block(x, y, z);
            let f = facts.get(id);
            !facts.is_air(id) && f.full_collision
        };
        let room = |x: i32, y: i32, z: i32| {
            let id = self.world.block(x, y, z);
            facts.is_air(id) || (facts.get(id).replaceable && facts.get(id).fluid == horizon_items::blocks::Fluid::None)
        };
        let mut spots = Vec::new();
        'scan: for x in cx - RADIUS..=cx + RADIUS {
            for z in cz - RADIUS..=cz + RADIUS {
                if !self.world.chunks.contains_key(&(x >> 4, z >> 4)) {
                    continue;
                }
                for y in cy - HEIGHT..=cy + HEIGHT {
                    if !solid(x, y - 1, z) || !room(x, y, z) || !room(x, y + 1, z) {
                        continue;
                    }
                    if let Some(level) = classify(self.world.light(x, y, z)) {
                        spots.push(([x, y, z], level));
                        if spots.len() >= MAX_SPOTS {
                            break 'scan;
                        }
                    }
                }
            }
        }
        self.belumi_light.spots = spots;
    }

    /// The Xs as lines for the world's renderer.
    pub(super) fn belumi_light_lines(&self) -> Vec<Line> {
        let mut g = Gizmos::default();
        for ([x, y, z], level) in &self.belumi_light.spots {
            for line in cross(*x, *y, *z, if *level == Level::Always { RED } else { YELLOW }) {
                g.lines.push(line);
            }
        }
        g.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_cells_are_red_cells_the_sun_lights_are_yellow_and_lit_cells_are_nothing() {
        assert_eq!(classify(0x00), Some(Level::Always));
        assert_eq!(classify(0xF0), Some(Level::Night), "open sky, no torch");
        assert_eq!(classify(0x30), Some(Level::Night));
        assert_eq!(classify(0x01), None, "any block light is safe");
        assert_eq!(classify(0xFE), None);
    }

    #[test]
    fn the_cross_lies_flat_on_the_cell_inside_its_edges() {
        let [a, b] = cross(10, 64, -3, RED);
        for l in [a, b] {
            assert!(l.start[1] > 64.0 && l.start[1] < 64.1 && l.end[1] == l.start[1]);
            for v in [l.start, l.end] {
                assert!((10.0..11.0).contains(&v[0]) && (-3.0..-2.0).contains(&v[2]));
            }
        }
        assert_ne!(a.start[2], b.start[2], "the two strokes cross");
    }
}
