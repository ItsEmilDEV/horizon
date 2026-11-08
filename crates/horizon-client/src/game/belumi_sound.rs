//! Sound physics: the world's sounds get the reverb of the room the player stands in. The room is measured with rays from the
//! head: in a small closed room the sound rings a little, in a big cave a lot, in the open not at all. Off by default.
use super::Game;
use std::time::{Duration, Instant};

const REACH: f32 = 40.0;
const STEP: f32 = 0.5;
const EVERY: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(super) struct Sound {
    last: Option<Instant>,
    wet: f32,
    decay: f32,
    /// A room was set on the audio last: it has to be taken back when the option goes off.
    set: bool,
}

/// The 14 directions of a cube: six faces and eight corners.
fn directions() -> [[f32; 3]; 14] {
    let k = 1.0 / 3.0_f32.sqrt();
    [
        [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0],
        [k, k, k], [k, k, -k], [k, -k, k], [k, -k, -k], [-k, k, k], [-k, k, -k], [-k, -k, k], [-k, -k, -k],
    ]
}

/// How far a ray from `from` goes through air (up to `REACH`), or `None` when it never hits anything.
fn cast(solid: &dyn Fn(i32, i32, i32) -> bool, from: [f32; 3], dir: [f32; 3]) -> Option<f32> {
    let mut t = STEP;
    while t <= REACH {
        let p = [from[0] + dir[0] * t, from[1] + dir[1] * t, from[2] + dir[2] * t];
        if solid(p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32) {
            return Some(t);
        }
        t += STEP;
    }
    None
}

/// The reverb a room asks for: how much of the sound goes through it (0..0.5) and how long it rings (0..1). A room is
/// closed when most rays hit a wall; the further the walls, the longer it rings.
pub(super) fn room_from_hits(hits: &[Option<f32>]) -> (f32, f32) {
    if hits.is_empty() {
        return (0.0, 0.0);
    }
    let closed: Vec<f32> = hits.iter().flatten().copied().collect();
    let enclosed = closed.len() as f32 / hits.len() as f32;
    if closed.is_empty() {
        return (0.0, 0.0);
    }
    let mean = closed.iter().sum::<f32>() / closed.len() as f32;
    let size = (mean / 20.0).clamp(0.0, 1.0);
    // The ground alone is not a room (a third of the rays hit it). A few open rays (a doorway) hardly matter, many do.
    let closedness = ((enclosed - 0.4) / 0.6).clamp(0.0, 1.0);
    let wet = (closedness * closedness * (0.12 + 0.38 * size)).clamp(0.0, 0.5);
    (wet, 0.25 + 0.6 * size)
}

/// Measures the room around `head` in a world described by `solid`.
pub(super) fn measure(solid: &dyn Fn(i32, i32, i32) -> bool, head: [f32; 3]) -> (f32, f32) {
    let hits: Vec<Option<f32>> = directions().iter().map(|d| cast(solid, head, *d)).collect();
    room_from_hits(&hits)
}

impl Game<'_> {
    pub(super) fn belumi_sound_frame(&mut self) {
        let on = self.modules_ref().is_some_and(|m| m.misc.sound_physics);
        if !on || self.player.is_none() {
            if self.belumi_sound.set {
                self.belumi_sound = Sound::default();
                self.audio.set_room(0.0, 0.0);
            }
            return;
        }
        if self.belumi_sound.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.belumi_sound.last = Some(Instant::now());
        let Some(eye) = self.eye_position() else { return };
        let facts = self.res.facts();
        let world = &self.world;
        let solid = |x: i32, y: i32, z: i32| {
            let f = facts.get(world.block(x, y, z));
            !facts.is_air(world.block(x, y, z)) && (f.full_collision || f.solid_render)
        };
        let (wet, decay) = measure(&solid, [eye[0] as f32, eye[1] as f32, eye[2] as f32]);
        // Ease toward the new room so walking through a door does not click.
        let s = &mut self.belumi_sound;
        s.wet += (wet - s.wet) * 0.35;
        s.decay += (decay - s.decay) * 0.35;
        s.set = true;
        let (w, d) = (s.wet, s.decay);
        self.audio.set_room(w, d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box room of `size` blocks each way around the origin: walls at distance `size / 2 + 1`, a floor and a ceiling.
    fn room(half: i32) -> impl Fn(i32, i32, i32) -> bool {
        move |x, y, z| x.abs() > half || y.abs() > half || z.abs() > half
    }

    #[test]
    fn the_open_air_has_no_reverb_and_a_closed_room_has() {
        let open = |_x: i32, y: i32, _z: i32| y < -2;
        let (wet_open, _) = measure(&open, [0.5, 0.5, 0.5]);
        let (wet_room, decay_room) = measure(&room(3), [0.5, 0.5, 0.5]);
        assert!(wet_open < 0.02, "{wet_open}");
        assert!(wet_room > 0.05 && decay_room > 0.2, "{wet_room} {decay_room}");
    }

    #[test]
    fn a_big_cave_rings_longer_and_wetter_than_a_closet() {
        let (wet_small, decay_small) = measure(&room(2), [0.5, 0.5, 0.5]);
        let (wet_big, decay_big) = measure(&room(14), [0.5, 0.5, 0.5]);
        assert!(wet_big > wet_small && decay_big > decay_small, "{wet_small}/{decay_small} {wet_big}/{decay_big}");
        assert!(wet_big <= 0.5 && decay_big <= 1.0);
    }

    #[test]
    fn no_rays_or_no_walls_mean_no_reverb() {
        assert_eq!(room_from_hits(&[]), (0.0, 0.0));
        assert_eq!(room_from_hits(&[None, None, None]), (0.0, 0.0));
    }

    #[test]
    fn a_doorway_hardly_changes_a_room() {
        let closed = vec![Some(5.0); 14];
        let mut door = closed.clone();
        door[0] = None;
        door[1] = None;
        let (a, _) = room_from_hits(&closed);
        let (b, _) = room_from_hits(&door);
        assert!(b > a * 0.4 && b < a, "{a} {b}");
    }
}
