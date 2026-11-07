use super::{Game, perspective::CameraType};

const SPEED_MIN: f32 = 1.0;
const SPEED_MAX: f32 = 120.0;

#[derive(Default)]
pub(super) struct FreecamState {
    active: bool,
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    speed: Option<f32>,
    saved: Option<CameraType>,
    anchor: [f64; 3],
    /// A camera path moves the camera: the keys and the mouse do not.
    driven: bool,
}

/// One point of a replay's camera path: where the camera is at `t` ms of the replay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PathKey {
    pub t: u32,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// The camera on the path at `t`: a smooth curve through the points (Catmull-Rom for the position, the
/// short way round for the yaw), standing still before the first point and after the last. `keys` is sorted by time.
pub(super) fn path_pose(keys: &[PathKey], t: u32) -> Option<([f64; 3], f32, f32)> {
    let (first, last) = (keys.first()?, keys.last()?);
    if t <= first.t || keys.len() == 1 {
        return Some((first.pos, first.yaw, first.pitch));
    }
    if t >= last.t {
        return Some((last.pos, last.yaw, last.pitch));
    }
    let i = keys.iter().rposition(|k| k.t <= t)?;
    let (a, b) = (keys[i], keys[i + 1]);
    let before = if i > 0 { keys[i - 1] } else { a };
    let after = keys.get(i + 2).copied().unwrap_or(b);
    let u = f64::from(t - a.t) / f64::from((b.t - a.t).max(1));
    let (u2, u3) = (u * u, u * u * u);
    let mut pos = [0.0; 3];
    for axis in 0..3 {
        let (p0, p1, p2, p3) = (before.pos[axis], a.pos[axis], b.pos[axis], after.pos[axis]);
        pos[axis] = 0.5
            * (2.0 * p1
                + (p2 - p0) * u
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u2
                + (3.0 * p1 - p0 - 3.0 * p2 + p3) * u3);
    }
    let turn = (b.yaw - a.yaw + 180.0).rem_euclid(360.0) - 180.0;
    let eased = (u * u * (3.0 - 2.0 * u)) as f32;
    Some((pos, a.yaw + turn * eased, a.pitch + (b.pitch - a.pitch) * eased))
}

pub(super) fn wheel_speed(current: f32, steps: f32) -> f32 {
    (current * 1.2_f32.powf(steps)).clamp(SPEED_MIN, SPEED_MAX)
}

pub(super) fn step(
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    input: [f32; 3],
    speed: f32,
    dt: f32,
) -> [f64; 3] {
    let (sy, cy) = f64::from(yaw).to_radians().sin_cos();
    let (sp, cp) = f64::from(pitch).to_radians().sin_cos();
    let forward = [-sy * cp, -sp, cy * cp];
    let right = [-cy, 0.0, -sy];
    let d = f64::from(speed * dt);
    let (f, r, u) = (f64::from(input[0]), f64::from(input[1]), f64::from(input[2]));
    [
        pos[0] + (forward[0] * f + right[0] * r) * d,
        pos[1] + (forward[1] * f + u) * d,
        pos[2] + (forward[2] * f + right[2] * r) * d,
    ]
}

pub(super) fn limit(pos: [f64; 3], anchor: [f64; 3], range: f64) -> [f64; 3] {
    let (dx, dz) = (pos[0] - anchor[0], pos[2] - anchor[2]);
    let dist = dx.hypot(dz);
    if dist <= range {
        return pos;
    }
    let k = range / dist;
    [anchor[0] + dx * k, pos[1], anchor[2] + dz * k]
}

impl Game<'_> {
    pub(super) fn belumi_freecam_active(&self) -> bool {
        self.belumi_free.active
    }

    /// The freecam or the open world map has the keys and the mouse: the player stands still.
    pub(super) fn belumi_input_taken(&self) -> bool {
        self.belumi_free.active || self.belumi_maps.open
    }

    pub(super) fn belumi_freecam_toggle(&mut self) {
        if self.belumi_free.active {
            self.belumi_freecam_off();
            return;
        }
        let Some(p) = self.player.as_ref() else {
            return;
        };
        let anchor = [p.position.x, p.position.y, p.position.z];
        let start = self.view;
        self.belumi_free = FreecamState {
            active: true,
            pos: start.position,
            yaw: start.y_rot,
            pitch: start.x_rot,
            speed: None,
            saved: Some(self.camera_type),
            anchor,
            driven: false,
        };
        self.camera_type = CameraType::ThirdPersonBack;
    }

    /// Where the free camera is, for a camera path point.
    pub(super) fn belumi_freecam_pose(&self) -> Option<([f64; 3], f32, f32)> {
        self.belumi_free.active.then(|| (self.belumi_free.pos, self.belumi_free.yaw, self.belumi_free.pitch))
    }

    /// A camera path puts the camera here (`None` gives it back to the player).
    pub(super) fn belumi_freecam_drive(&mut self, pose: Option<([f64; 3], f32, f32)>) {
        let f = &mut self.belumi_free;
        f.driven = pose.is_some() && f.active;
        if let (true, Some((pos, yaw, pitch))) = (f.driven, pose) {
            f.pos = pos;
            f.yaw = yaw;
            f.pitch = pitch;
        }
    }

    pub(super) fn belumi_freecam_off(&mut self) {
        if !self.belumi_free.active {
            return;
        }
        if let Some(saved) = self.belumi_free.saved.take() {
            self.camera_type = saved;
        }
        self.belumi_free.active = false;
    }

    pub(super) fn belumi_freecam_view(&self) -> Option<(glam::DVec3, f32, f32)> {
        self.belumi_free.active.then(|| {
            (
                glam::DVec3::from_array(self.belumi_free.pos),
                self.belumi_free.yaw,
                self.belumi_free.pitch,
            )
        })
    }

    pub(super) fn belumi_freecam_turn(&mut self, xo: f64, yo: f64) {
        let f = &mut self.belumi_free;
        if f.driven {
            return;
        }
        f.yaw += (xo * 0.15) as f32;
        f.pitch = (f.pitch + (yo * 0.15) as f32).clamp(-90.0, 90.0);
    }

    pub(super) fn belumi_freecam_wheel(&mut self, steps: f32, claimed: &mut bool) {
        if !self.belumi_free.active || *claimed {
            return;
        }
        let current = self
            .belumi_free
            .speed
            .unwrap_or_else(|| self.settings.belumi.freecam_blocks_per_second());
        self.belumi_free.speed = Some(wheel_speed(current, steps));
        *claimed = true;
    }

    pub(super) fn belumi_freecam_frame(&mut self, dt: f32) {
        if !self.belumi_free.active {
            return;
        }
        if self.player.is_none() || (!self.settings.belumi.freecam && !self.belumi_replay_active()) {
            self.belumi_freecam_off();
            return;
        }
        if let Some(p) = self.player.as_ref() {
            self.belumi_free.anchor = [p.position.x, p.position.y, p.position.z];
        }
        if self.belumi_free.driven {
            return;
        }
        let held = |g: &mut Self, name: &str| {
            g.native_playing() && g.native_mapping(name).is_some_and(|(down, _)| down)
        };
        let axis = |plus: bool, minus: bool| f32::from(u8::from(plus)) - f32::from(u8::from(minus));
        let input = [
            axis(held(self, "key.forward"), held(self, "key.back")),
            axis(held(self, "key.right"), held(self, "key.left")),
            axis(held(self, "key.jump"), held(self, "key.sneak")),
        ];
        let sprint = held(self, "key.sprint");
        let base = self
            .belumi_free
            .speed
            .unwrap_or_else(|| self.settings.belumi.freecam_blocks_per_second());
        let speed = if sprint { base * 3.0 } else { base };
        let f = &self.belumi_free;
        let next = step(f.pos, f.yaw, f.pitch, input, speed, dt.min(0.1));
        let range = f64::from(
            (self.settings.render_distance.max(2) as u32 * 16)
                .saturating_sub(16)
                .clamp(32, 256),
        );
        let next = limit(next, f.anchor, range);
        self.belumi_free.pos = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_follows_the_view_and_strafe_stays_level() {
        let p = step([0.0; 3], 0.0, 0.0, [1.0, 0.0, 0.0], 10.0, 1.0);
        assert!((p[2] - 10.0).abs() < 1e-9 && p[0].abs() < 1e-9);
        let up = step([0.0; 3], 0.0, -90.0, [1.0, 0.0, 0.0], 10.0, 1.0);
        assert!(up[1] > 9.9);
        let side = step([0.0; 3], 0.0, 45.0, [0.0, 1.0, 0.0], 10.0, 1.0);
        assert!(side[1].abs() < 1e-9 && side[0] < -9.9);
        let rise = step([0.0; 3], 0.0, 0.0, [0.0, 0.0, 1.0], 10.0, 1.0);
        assert!((rise[1] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn range_pulls_the_camera_back_toward_the_body_horizontally() {
        let p = limit([100.0, 70.0, 0.0], [0.0, 64.0, 0.0], 40.0);
        assert!((p[0] - 40.0).abs() < 1e-9 && p[1] == 70.0);
        assert_eq!(limit([3.0, 1.0, 4.0], [0.0; 3], 40.0), [3.0, 1.0, 4.0]);
    }

    #[test]
    fn a_camera_path_runs_through_its_points() {
        let key = |t, x: f64, yaw| PathKey { t, pos: [x, 64.0, 0.0], yaw, pitch: 0.0 };
        assert_eq!(path_pose(&[], 5), None);
        let one = [key(1000, 3.0, 20.0)];
        assert_eq!(path_pose(&one, 0).unwrap().0[0], 3.0);
        let keys = [key(1000, 0.0, 350.0), key(3000, 10.0, 10.0), key(5000, 20.0, 10.0)];
        assert_eq!(path_pose(&keys, 500).unwrap().0[0], 0.0, "before the first point");
        assert_eq!(path_pose(&keys, 9000).unwrap().0[0], 20.0, "after the last point");
        let at = |t| path_pose(&keys, t).unwrap();
        assert!((at(1000).0[0]).abs() < 1e-9 && (at(3000).0[0] - 10.0).abs() < 1e-9);
        assert!(at(2000).0[0] > 0.0 && at(2000).0[0] < 10.0);
        // 350 to 10 degrees turns 20 degrees through north, not 340 the other way.
        let yaw = at(2000).1;
        assert!((yaw - 360.0).abs() < 1.0 || yaw.abs() < 1.0, "{yaw}");
    }

    #[test]
    fn wheel_speed_stays_in_bounds() {
        assert!(wheel_speed(12.0, 1.0) > 12.0);
        assert_eq!(wheel_speed(100.0, 50.0), SPEED_MAX);
        assert_eq!(wheel_speed(2.0, -50.0), SPEED_MIN);
    }
}
