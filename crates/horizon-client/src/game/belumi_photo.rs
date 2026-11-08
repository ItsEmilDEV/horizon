//! Photo mode, on top of the free camera (F9): G circles the camera around what you look at, H takes a picture every few
//! seconds (a time-lapse), J takes a 360 degree panorama. F1 hides the HUD as always. Everything lands in `screenshots/`.
use super::Game;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const ORBIT_DEGREES_PER_SECOND: f64 = 12.0;
const TIMELAPSE_EVERY: Duration = Duration::from_secs(5);
/// Frames to wait after the camera moved before a picture is taken (the world needs to catch up).
const SETTLE_FRAMES: u32 = 4;
const PANORAMA_SIZE: [u32; 2] = [4096, 2048];

#[derive(Clone, Copy, Debug, PartialEq)]
struct Orbit {
    center: [f64; 3],
    radius: f64,
    angle: f64,
    height: f64,
}

struct Panorama {
    dir: PathBuf,
    shots: Vec<(f32, f32)>,
    next: usize,
    wait: u32,
    asked: bool,
    fov: f32,
    /// The HUD and camera type to give back: a panorama has no HUD, no hand and no body in it.
    saved: (bool, super::perspective::CameraType),
}

#[derive(Default)]
pub(super) struct Photo {
    orbit: Option<Orbit>,
    timelapse: Option<(PathBuf, Instant, u32)>,
    panorama: Option<Panorama>,
}

/// The yaw and pitch (degrees) that face `to` from `from`.
pub(super) fn look_at(from: [f64; 3], to: [f64; 3]) -> (f32, f32) {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    ((-dx.atan2(dz)).to_degrees() as f32, (-dy.atan2(dx.hypot(dz))).to_degrees() as f32)
}

/// Where the orbiting camera is and where it looks.
fn orbit_pose(o: &Orbit) -> ([f64; 3], f32, f32) {
    let a = o.angle.to_radians();
    let pos = [o.center[0] + a.cos() * o.radius, o.center[1] + o.height, o.center[2] + a.sin() * o.radius];
    let (yaw, pitch) = look_at(pos, o.center);
    (pos, yaw, pitch)
}

/// The camera directions of a panorama: rings of overlapping pictures round the horizon and above and below it, then the
/// poles. `hfov` is the horizontal field of view of one picture in degrees.
fn panorama_shots(hfov: f32) -> Vec<(f32, f32)> {
    let step = (hfov * 0.65).clamp(20.0, 80.0);
    let per_ring = (360.0 / step).ceil() as usize;
    let mut shots = Vec::new();
    for pitch in [0.0_f32, 45.0, -45.0] {
        let n = if pitch == 0.0 { per_ring } else { (per_ring * 3 / 4).max(4) };
        for i in 0..n {
            shots.push((i as f32 * 360.0 / n as f32, pitch));
        }
    }
    shots.push((0.0, 90.0));
    shots.push((0.0, -90.0));
    shots
}

/// One picture of a panorama: where the camera looked and what it saw (RGBA).
pub(super) struct Shot {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

fn basis(yaw: f32, pitch: f32) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let (sy, cy) = f64::from(yaw).to_radians().sin_cos();
    let (sp, cp) = f64::from(pitch).to_radians().sin_cos();
    let f = [-sy * cp, -sp, cy * cp];
    let r = [-cy, 0.0, -sy];
    let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
    (f, r, u)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Puts the pictures together as an equirectangular image (RGBA): each pixel takes the colour from the picture whose
/// centre is nearest to its direction, among those that see it. The centre of the image faces yaw 0.
pub(super) fn equirect(shots: &[Shot], vertical_fov: f32, out: [u32; 2]) -> Vec<u8> {
    let bases: Vec<_> = shots.iter().map(|s| basis(s.yaw, s.pitch)).collect();
    let tv = (f64::from(vertical_fov).to_radians() / 2.0).tan();
    let mut image = vec![0u8; (out[0] * out[1] * 4) as usize];
    for y in 0..out[1] {
        let lat = (0.5 - (f64::from(y) + 0.5) / f64::from(out[1])) * std::f64::consts::PI;
        for x in 0..out[0] {
            let lon = ((f64::from(x) + 0.5) / f64::from(out[0]) - 0.5) * std::f64::consts::TAU;
            let d = [-lon.sin() * lat.cos(), lat.sin(), lon.cos() * lat.cos()];
            let mut best: Option<(f64, usize, f64, f64)> = None;
            for (i, (s, (f, r, u))) in shots.iter().zip(&bases).enumerate() {
                let z = dot(d, *f);
                if z <= 0.01 {
                    continue;
                }
                let th = tv * f64::from(s.width) / f64::from(s.height);
                let (nx, ny) = (dot(d, *r) / (z * th), dot(d, *u) / (z * tv));
                if nx.abs() > 1.0 || ny.abs() > 1.0 {
                    continue;
                }
                if best.is_none_or(|b| z > b.0) {
                    best = Some((z, i, nx, ny));
                }
            }
            if let Some((_, i, nx, ny)) = best {
                let s = &shots[i];
                let px = (((nx * 0.5 + 0.5) * f64::from(s.width)) as u32).min(s.width - 1);
                let py = (((0.5 - ny * 0.5) * f64::from(s.height)) as u32).min(s.height - 1);
                let from = ((py * s.width + px) * 4) as usize;
                let to = ((y * out[0] + x) * 4) as usize;
                image[to..to + 4].copy_from_slice(&s.rgba[from..from + 4]);
            }
        }
    }
    image
}

fn stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{secs}")
}

fn read_png(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return None,
    };
    Some((info.width, info.height, rgba))
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> bool {
    let Ok(file) = std::fs::File::create(path) else { return false };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().ok().and_then(|mut wr| wr.write_image_data(rgba).ok()).is_some()
}

impl Game<'_> {
    fn screenshots_dir() -> PathBuf {
        std::env::current_dir().unwrap_or_default().join("screenshots")
    }

    fn photo_ready(&self) -> bool {
        self.belumi_freecam_active()
    }

    pub(super) fn belumi_photo_key(&mut self, key: &str, pressed: bool) {
        if !pressed || !self.native_playing() || !self.photo_ready() {
            return;
        }
        match key {
            "key.keyboard.g" => self.photo_orbit_toggle(),
            "key.keyboard.h" => self.photo_timelapse_toggle(),
            "key.keyboard.j" => self.photo_panorama_start(),
            _ => {}
        }
    }

    fn photo_orbit_toggle(&mut self) {
        if self.belumi_photo.orbit.take().is_some() {
            self.belumi_freecam_drive(None);
            self.toast("Orbit off");
            return;
        }
        let Some((pos, yaw, pitch)) = self.belumi_freecam_pose() else { return };
        let (sy, cy) = f64::from(yaw).to_radians().sin_cos();
        let (sp, cp) = f64::from(pitch).to_radians().sin_cos();
        let forward = [-sy * cp, -sp, cy * cp];
        // Around the block in the crosshair, or ten blocks ahead when it points at the sky.
        let center = self
            .pick()
            .map(|h| [f64::from(h.pos[0]) + 0.5, f64::from(h.pos[1]) + 0.5, f64::from(h.pos[2]) + 0.5])
            .unwrap_or([pos[0] + forward[0] * 10.0, pos[1] + forward[1] * 10.0, pos[2] + forward[2] * 10.0]);
        let (dx, dz) = (pos[0] - center[0], pos[2] - center[2]);
        let radius = dx.hypot(dz).clamp(3.0, 60.0);
        self.belumi_photo.orbit = Some(Orbit { center, radius, angle: dz.atan2(dx).to_degrees(), height: (pos[1] - center[1]).clamp(-20.0, 40.0) });
        self.toast("Orbit on (G again to stop)");
    }

    fn photo_timelapse_toggle(&mut self) {
        if let Some((dir, _, n)) = self.belumi_photo.timelapse.take() {
            self.toast(format!("Time-lapse done: {n} pictures in {}", dir.display()));
            return;
        }
        let dir = Self::screenshots_dir().join(format!("timelapse-{}", stamp()));
        let _ = std::fs::create_dir_all(&dir);
        self.belumi_photo.timelapse = Some((dir, Instant::now() - TIMELAPSE_EVERY, 0));
        self.toast("Time-lapse on: a picture every 5 s (H again to stop)");
    }

    fn photo_panorama_start(&mut self) {
        if self.belumi_photo.panorama.is_some() {
            return;
        }
        let Some((_, _, _, fov)) = self.locator_camera else { return };
        let Some((w, h)) = self.renderer.as_ref().map(|r| (r.config.width, r.config.height)) else { return };
        let aspect = w as f32 / h.max(1) as f32;
        let hfov = (2.0 * ((f64::from(fov).to_radians() / 2.0).tan() * f64::from(aspect)).atan()).to_degrees() as f32;
        let dir = std::env::temp_dir().join(format!("belumi-panorama-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let shots = panorama_shots(hfov);
        self.toast(format!("Panorama: {} pictures, hold still", shots.len()));
        self.belumi_photo.orbit = None;
        let saved = (self.ui.hide_gui, self.camera_type);
        self.ui.hide_gui = true;
        self.camera_type = super::perspective::CameraType::FirstPerson;
        self.belumi_photo.panorama = Some(Panorama { dir, shots, next: 0, wait: SETTLE_FRAMES, asked: false, fov, saved });
    }

    /// Once a frame: the orbit moves the camera, the time-lapse and the panorama take their pictures.
    pub(super) fn belumi_photo_frame(&mut self, dt: f32) {
        if !self.belumi_freecam_active() {
            if let Some(p) = self.belumi_photo.panorama.take() {
                self.ui.hide_gui = p.saved.0;
            }
            self.belumi_photo = Photo::default();
            return;
        }
        if let Some(o) = self.belumi_photo.orbit.as_mut() {
            o.angle += ORBIT_DEGREES_PER_SECOND * f64::from(dt.min(0.1));
            let pose = orbit_pose(o);
            self.belumi_freecam_drive(Some(pose));
        }
        if let Some((dir, last, n)) = self.belumi_photo.timelapse.as_mut()
            && last.elapsed() >= TIMELAPSE_EVERY
            && let Some(r) = self.renderer.as_mut()
            && r.screenshot.is_none()
        {
            *last = Instant::now();
            *n += 1;
            r.screenshot = Some(horizon_client::gfx::ScreenshotRequest { path: dir.join(format!("{n:04}.png")), size: None });
        }
        self.photo_panorama_step();
    }

    fn photo_panorama_step(&mut self) {
        let pose = self.belumi_freecam_pose();
        let Some(p) = self.belumi_photo.panorama.as_mut() else { return };
        if p.next >= p.shots.len() {
            // All taken: put them together.
            let Panorama { dir, shots, fov, saved, .. } = self.belumi_photo.panorama.take().expect("checked above");
            self.ui.hide_gui = saved.0;
            self.camera_type = saved.1;
            let mut pictures = Vec::new();
            for (i, (yaw, pitch)) in shots.iter().enumerate() {
                if let Some((width, height, rgba)) = read_png(&dir.join(format!("{i:03}.png"))) {
                    pictures.push(Shot { yaw: *yaw, pitch: *pitch, width, height, rgba });
                }
            }
            let _ = std::fs::remove_dir_all(&dir);
            self.belumi_freecam_drive(None);
            if pictures.len() < shots.len() {
                self.toast("Panorama failed: a picture is missing");
                return;
            }
            let image = equirect(&pictures, fov, PANORAMA_SIZE);
            let out = Self::screenshots_dir().join(format!("panorama-{}.png", stamp()));
            let _ = std::fs::create_dir_all(Self::screenshots_dir());
            let ok = write_png(&out, PANORAMA_SIZE[0], PANORAMA_SIZE[1], &image);
            self.toast(if ok { format!("Panorama saved: {}", out.display()) } else { "Panorama: could not write the picture".to_owned() });
            return;
        }
        let Some((pos, _, _)) = pose else { return };
        let (yaw, pitch) = p.shots[p.next];
        let (path, asked, wait) = (p.dir.join(format!("{:03}.png", p.next)), p.asked, p.wait);
        self.belumi_freecam_drive(Some((pos, yaw, pitch)));
        let Some(p) = self.belumi_photo.panorama.as_mut() else { return };
        if wait > 0 {
            p.wait -= 1;
            return;
        }
        if !asked {
            let Some(r) = self.renderer.as_mut() else { return };
            if r.screenshot.is_some() {
                return;
            }
            r.screenshot = Some(horizon_client::gfx::ScreenshotRequest { path: path.clone(), size: None });
            p.asked = true;
            p.wait = 2;
            return;
        }
        if path.exists() && std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
            p.next += 1;
            p.asked = false;
            p.wait = SETTLE_FRAMES;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_at_a_point_faces_it() {
        let (yaw, pitch) = look_at([0.0, 0.0, 0.0], [0.0, 0.0, 10.0]);
        assert!(yaw.abs() < 1e-3 && pitch.abs() < 1e-3);
        let (yaw, _) = look_at([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
        assert!((yaw + 90.0).abs() < 1e-3, "east is yaw -90: {yaw}");
        let (_, pitch) = look_at([0.0, 10.0, 0.0], [0.0, 0.0, 10.0]);
        assert!(pitch > 40.0 && pitch < 50.0, "looking down is a positive pitch: {pitch}");
    }

    #[test]
    fn the_orbit_keeps_its_distance_and_always_looks_at_the_centre() {
        let mut o = Orbit { center: [10.0, 64.0, -5.0], radius: 8.0, angle: 0.0, height: 3.0 };
        for _ in 0..12 {
            let (pos, yaw, pitch) = orbit_pose(&o);
            let flat = (pos[0] - 10.0).hypot(pos[2] + 5.0);
            assert!((flat - 8.0).abs() < 1e-9 && (pos[1] - 67.0).abs() < 1e-9);
            let (f, _, _) = basis(yaw, pitch);
            let to_centre = [10.0 - pos[0], 64.0 - pos[1], -5.0 - pos[2]];
            let len = dot(to_centre, to_centre).sqrt();
            assert!((dot(f, to_centre) / len - 1.0).abs() < 1e-6, "the forward vector points at the centre");
            o.angle += 30.0;
        }
    }

    #[test]
    fn a_panorama_covers_the_horizon_and_both_poles() {
        let shots = panorama_shots(90.0);
        assert!(shots.iter().filter(|s| s.1 == 0.0).count() >= 6);
        assert!(shots.contains(&(0.0, 90.0)) && shots.contains(&(0.0, -90.0)));
        assert!(panorama_shots(30.0).len() > shots.len());
    }

    /// A made-up world: the colour depends only on the direction. Pictures of it, put together, must give the same colours.
    fn world(d: [f64; 3]) -> [u8; 4] {
        let c = |v: f64| ((v * 0.5 + 0.5) * 255.0) as u8;
        [c(d[0]), c(d[1]), c(d[2]), 255]
    }

    fn take(yaw: f32, pitch: f32, vfov: f32, w: u32, h: u32) -> Shot {
        let (f, r, u) = basis(yaw, pitch);
        let (tv, th) = ((f64::from(vfov).to_radians() / 2.0).tan(), (f64::from(vfov).to_radians() / 2.0).tan() * f64::from(w) / f64::from(h));
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = ((f64::from(x) + 0.5) / f64::from(w) * 2.0 - 1.0, 1.0 - (f64::from(y) + 0.5) / f64::from(h) * 2.0);
                let d = [f[0] + r[0] * nx * th + u[0] * ny * tv, f[1] + r[1] * nx * th + u[1] * ny * tv, f[2] + r[2] * nx * th + u[2] * ny * tv];
                let len = dot(d, d).sqrt();
                rgba.extend_from_slice(&world([d[0] / len, d[1] / len, d[2] / len]));
            }
        }
        Shot { yaw, pitch, width: w, height: h, rgba }
    }

    #[test]
    fn the_stitched_panorama_shows_the_same_world_in_every_direction() {
        let (vfov, w, h) = (70.0_f32, 160, 90);
        let hfov = (2.0 * ((f64::from(vfov).to_radians() / 2.0).tan() * f64::from(w) / f64::from(h)).atan()).to_degrees() as f32;
        let shots: Vec<Shot> = panorama_shots(hfov).into_iter().map(|(yaw, pitch)| take(yaw, pitch, vfov, w, h)).collect();
        let out = equirect(&shots, vfov, [128, 64]);
        let mut wrong = 0;
        for y in 0..64u32 {
            for x in 0..128u32 {
                let lat = (0.5 - (f64::from(y) + 0.5) / 64.0) * std::f64::consts::PI;
                let lon = ((f64::from(x) + 0.5) / 128.0 - 0.5) * std::f64::consts::TAU;
                let want = world([-lon.sin() * lat.cos(), lat.sin(), lon.cos() * lat.cos()]);
                let got = &out[((y * 128 + x) * 4) as usize..][..4];
                if got[3] != 255 || (0..3).any(|k| got[k].abs_diff(want[k]) > 12) {
                    wrong += 1;
                }
            }
        }
        assert!(wrong < 40, "{wrong} of 8192 pixels differ");
    }

    #[test]
    fn a_picture_survives_the_png_round_trip() {
        let dir = std::env::temp_dir().join(format!("belumi-photo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rgba: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 3) as u8).collect();
        let path = dir.join("a.png");
        assert!(write_png(&path, 4, 4, &rgba));
        assert_eq!(read_png(&path), Some((4, 4, rgba)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
