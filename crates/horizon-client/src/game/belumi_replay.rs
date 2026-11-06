//! Replays in the game: recording a session, clips and bookmarks with their notifications, starting a
//! recording as a replay, its controls, and the replay pages of the client screen.

use super::{Game, Screen, belumi_client::{Act, Page}};
use horizon_client::{
    hud::HudBatch,
    replay::{self, Control, Entry, Header, Pose, Replay},
};
use horizon_core::geometry::Vec3;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

const TOAST_SECONDS: f32 = 3.5;
const POSE_EVERY: Duration = Duration::from_millis(50);
const SPEEDS: [u32; 7] = [250, 500, 1000, 2000, 4000, 8000, 16000];

#[derive(Clone, Debug, PartialEq)]
pub(super) enum RAct {
    Play(usize, Option<usize>),
    Delete(usize, Option<usize>),
    Record,
    Clip(i32),
    Pause,
    Speed(i32),
    Seek,
    Back(u32),
    Stop,
    Cam,
}

pub(super) struct Playing {
    ctl: Arc<Control>,
    path: PathBuf,
    header: Header,
    clip: Option<(u32, u32, String)>,
    pose: Option<Pose>,
    announced_end: bool,
    free_cam: bool,
    entered: bool,
}

#[derive(Default)]
pub(super) struct ReplayState {
    recorder: Option<Arc<replay::Recorder>>,
    last_pose: Option<Instant>,
    last_pose_sent: Option<Pose>,
    playing: Option<Playing>,
    /// A replay to start once the session in the way is closed: path, start, end, clip name.
    pending: Option<(PathBuf, u32, u32, String)>,
    pub(super) toasts: Vec<(String, Instant)>,
    entries: Vec<Entry>,
    scroll: usize,
    timeline: [f32; 4],
}

fn clock(ms: u32) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / f64::from(1 << 20)) } else { format!("{} KB", bytes / 1024) }
}

impl Game<'_> {
    fn replays_dir() -> PathBuf {
        std::env::current_dir().unwrap_or_default().join("replays")
    }

    pub(super) fn toast(&mut self, text: impl Into<String>) {
        self.belumi_replay.toasts.push((text.into(), Instant::now()));
        if self.belumi_replay.toasts.len() > 5 {
            self.belumi_replay.toasts.remove(0);
        }
    }

    pub(super) fn belumi_replay_active(&self) -> bool {
        self.belumi_replay.playing.is_some()
    }

    // ---- recording ------------------------------------------------------------------------

    /// A session is about to connect: record it when recording is on (never a replay).
    pub(super) fn belumi_replay_begin(&mut self, address: &str, name: &str) {
        self.belumi_replay_stop_recording();
        if self.belumi_replay.playing.is_some() || !self.modules().replay.record {
            return;
        }
        let header = Header {
            protocol: 0,
            server: address.to_owned(),
            player: name.to_owned(),
            uuid: horizon_client::auth::current().map_or_else(String::new, |a| format!("{:032x}", a.uuid)),
            started_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            game: "26.3".into(),
        };
        let keep = usize::from(self.modules().replay.keep);
        replay::prune(&Self::replays_dir(), keep.saturating_sub(1));
        match replay::start(&Self::replays_dir(), &header) {
            Ok(r) => {
                self.belumi_replay.recorder = Some(r);
                self.toast("Recording started");
            }
            Err(e) => log::warn!("Horizon: could not start a replay recording: {e}"),
        }
    }

    fn belumi_replay_stop_recording(&mut self) {
        if self.belumi_replay.recorder.take().is_some() {
            replay::stop();
            self.toast("Recording saved");
        }
    }

    /// The game is closing: finish the recording's file before the process goes.
    pub(super) fn belumi_replay_shutdown(&mut self) {
        if let Some(rec) = self.belumi_replay.recorder.take() {
            replay::stop();
            for _ in 0..200 {
                if rec.finished() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if let Some(p) = self.belumi_replay.playing.take() {
            p.ctl.stop.store(true, Ordering::Release);
        }
    }

    pub(super) fn belumi_replay_end(&mut self) {
        self.belumi_replay_stop_recording();
        if let Some(p) = self.belumi_replay.playing.take() {
            p.ctl.stop.store(true, Ordering::Release);
            if let Some(player) = self.player.as_mut() {
                player.spectator = false;
            }
            if self.belumi_freecam_active() {
                self.belumi_freecam_off();
            }
        }
    }

    /// Each frame: the recorded player's pose (20 times a second, when it changed).
    fn belumi_replay_sample(&mut self) {
        let Some(rec) = self.belumi_replay.recorder.clone() else {
            return;
        };
        if rec.finished() || self.belumi_replay.last_pose.is_some_and(|t| t.elapsed() < POSE_EVERY) {
            return;
        }
        let Some(p) = self.player.as_ref() else {
            return;
        };
        let pose = Pose {
            x: p.position.x,
            y: p.position.y,
            z: p.position.z,
            yaw: self.yaw,
            pitch: self.pitch,
            flags: u8::from(p.shift_key_down) * replay::POSE_SNEAK | u8::from(p.sprinting) * replay::POSE_SPRINT,
        };
        self.belumi_replay.last_pose = Some(Instant::now());
        if self.belumi_replay.last_pose_sent == Some(pose) {
            return;
        }
        self.belumi_replay.last_pose_sent = Some(pose);
        let (id, data) = replay::custom_payload(replay::POSE_CHANNEL, &pose.to_bytes());
        replay::record(replay::PLAY, id, &data);
    }

    fn belumi_replay_mark(&mut self, label: &str) {
        let (id, data) = replay::custom_payload(replay::MARK_CHANNEL, label.as_bytes());
        replay::record(replay::PLAY, id, &data);
    }

    /// Hotkeys: the clip key saves the last seconds, the mark key a bookmark.
    pub(super) fn belumi_replay_key(&mut self, key: &str, pressed: bool) {
        if !pressed || !self.native_playing() {
            return;
        }
        let (clip_key, mark_key, seconds) = {
            let r = &self.modules().replay;
            (r.clip_key.clone(), r.mark_key.clone(), u32::from(r.clip_seconds))
        };
        if self.belumi_replay.playing.is_some() {
            self.belumi_replay_playing_key(key);
            return;
        }
        if key != clip_key && key != mark_key {
            return;
        }
        let Some(rec) = self.belumi_replay.recorder.clone() else {
            self.toast("Recording is off: turn it on under Replays");
            return;
        };
        let now = rec.elapsed_ms();
        if key == clip_key {
            let name = format!("Clip {}", clock(now));
            replay::add_clip(&rec.path, now, seconds, &name);
            self.toast(format!("Clip saved: the last {seconds} s"));
        } else {
            self.belumi_replay_mark(&format!("Bookmark {}", clock(now)));
            self.toast(format!("Bookmark at {}", clock(now)));
        }
    }

    fn belumi_replay_playing_key(&mut self, key: &str) {
        let Some(p) = self.belumi_replay.playing.as_ref() else {
            return;
        };
        let ctl = p.ctl.clone();
        let shift = self.has_shift_down();
        match key {
            "key.keyboard.space" => {
                let now = !ctl.paused.load(Ordering::Relaxed);
                ctl.finished.store(false, Ordering::Relaxed);
                ctl.paused.store(now, Ordering::Relaxed);
            }
            "key.keyboard.left.bracket" => self.belumi_replay_speed(-1),
            "key.keyboard.right.bracket" => self.belumi_replay_speed(1),
            "key.keyboard.left" => self.belumi_replay_jump(if shift { -30_000 } else { -5_000 }),
            "key.keyboard.right" => self.belumi_replay_jump(if shift { 30_000 } else { 5_000 }),
            "key.keyboard.home" => self.belumi_replay_jump(i64::from(i32::MIN) / 2),
            "key.keyboard.v" => self.belumi_replay_cam(),
            _ => {}
        }
    }

    fn belumi_replay_speed(&mut self, step: i32) {
        let Some(p) = self.belumi_replay.playing.as_ref() else {
            return;
        };
        let now = p.ctl.speed_milli.load(Ordering::Relaxed);
        let at = SPEEDS.iter().position(|s| *s >= now).unwrap_or(2) as i32;
        let next = SPEEDS[(at + step).clamp(0, SPEEDS.len() as i32 - 1) as usize];
        p.ctl.speed_milli.store(next, Ordering::Relaxed);
        self.toast(format!("Replay speed {}x", f64::from(next) / 1000.0));
    }

    fn belumi_replay_jump(&mut self, delta_ms: i64) {
        let Some(p) = self.belumi_replay.playing.as_ref() else {
            return;
        };
        let now = i64::from(p.ctl.position.load(Ordering::Relaxed));
        let (lo, hi) = (p.clip.as_ref().map_or(0, |c| i64::from(c.0)), i64::from(p.ctl.duration.load(Ordering::Relaxed)));
        let to = (now + delta_ms).clamp(lo.min(hi), hi);
        p.ctl.finished.store(false, Ordering::Relaxed);
        p.ctl.seek.store(to, Ordering::Release);
    }

    fn belumi_replay_cam(&mut self) {
        let Some(p) = self.belumi_replay.playing.as_mut() else {
            return;
        };
        p.free_cam = !p.free_cam;
        let free = p.free_cam;
        if free != self.belumi_freecam_active() {
            self.belumi_freecam_toggle();
        }
        self.toast(if free { "Free camera" } else { "The player's view" });
    }

    // ---- playing --------------------------------------------------------------------------

    pub(super) fn belumi_replay_start(&mut self, path: PathBuf, start_ms: u32, end_ms: u32, clip: String) {
        self.belumi_replay.pending = Some((path, start_ms, end_ms, clip));
        self.belumi_client_close_screen();
        if self.remote.is_some() {
            if let Err(e) = self.leave_server(None) {
                log::warn!("Horizon: could not leave the session for a replay: {e}");
            }
        }
    }

    /// Per frame: begin a waiting replay, follow a running one.
    pub(super) fn belumi_replay_tick(&mut self) {
        self.belumi_replay_sample();
        self.belumi_replay.toasts.retain(|t| t.1.elapsed().as_secs_f32() < TOAST_SECONDS);
        if self.belumi_replay.pending.is_some() && self.remote.is_none() && self.integrated.is_none() {
            if let Some((path, start, end, clip)) = self.belumi_replay.pending.take() {
                self.belumi_replay_connect(path, start, end, clip);
            }
        }
        let Some(p) = self.belumi_replay.playing.as_mut() else {
            return;
        };
        let restart = p.ctl.restart.swap(-1, Ordering::AcqRel);
        if restart >= 0 {
            let (path, end, clip) = (p.path.clone(), p.ctl.end.load(Ordering::Relaxed), p.clip.as_ref().map(|c| c.2.clone()).unwrap_or_default());
            self.belumi_replay_end();
            self.belumi_replay_start(path, restart as u32, end, clip);
            return;
        }
        if p.ctl.finished.load(Ordering::Acquire) && !p.announced_end {
            p.announced_end = true;
            self.toast("Replay finished");
        }
        let (Some(pose), Some(player)) = (self.belumi_replay.playing.as_ref().and_then(|p| p.pose), self.player.as_mut()) else {
            return;
        };
        player.position = Vec3::new(pose.x, pose.y, pose.z);
        player.velocity = Vec3::new(0.0, 0.0, 0.0);
        player.fall_distance = 0.0;
        player.spectator = true;
        player.flying = true;
        player.may_fly = true;
        player.shift_key_down = pose.flags & replay::POSE_SNEAK != 0;
        player.sprinting = pose.flags & replay::POSE_SPRINT != 0;
        player.yaw = pose.yaw;
        player.head_yaw = pose.yaw;
        player.pitch = pose.pitch;
        self.yaw = pose.yaw;
        self.pitch = pose.pitch;
        let in_world = self.belumi_replay.playing.as_ref().is_some_and(|p| !p.entered);
        if in_world && self.screen == Screen::Playing && !self.belumi_freecam_active() {
            if let Some(p) = self.belumi_replay.playing.as_mut() {
                p.entered = true;
                p.free_cam = true;
            }
            self.belumi_freecam_toggle();
        }
    }

    fn belumi_replay_connect(&mut self, path: PathBuf, start_ms: u32, end_ms: u32, clip: String) {
        let replay = match Replay::open(&path) {
            Ok(r) => r,
            Err(e) => {
                self.toast(format!("Could not open the replay: {e}"));
                return;
            }
        };
        let header = replay.header.clone();
        let ctl = Control::new(replay.duration_ms, end_ms);
        let (client, server) = horizon_protocol::connection::Pipe::pair();
        replay::serve(server, replay, ctl.clone(), start_ms);
        self.belumi_replay.playing = Some(Playing {
            ctl,
            path,
            header: header.clone(),
            clip: (end_ms != u32::MAX || start_ms > 0).then_some((start_ms, end_ms, clip)),
            pose: None,
            announced_end: false,
            free_cam: false,
            entered: false,
        });
        if let Err(e) = self.connect_replay(client, &header.player) {
            self.toast(format!("Could not start the replay: {e}"));
            self.belumi_replay_end();
        }
    }

    pub(super) fn belumi_replay_payload(&mut self, channel: &str, data: &[u8]) {
        if channel == replay::POSE_CHANNEL {
            if let (Some(p), Some(pose)) = (self.belumi_replay.playing.as_mut(), Pose::from_bytes(data)) {
                p.pose = Some(pose);
            }
        }
    }

    pub(super) fn belumi_replay_channel(&self, channel: &str) -> bool {
        channel == replay::POSE_CHANNEL || channel == replay::MARK_CHANNEL
    }

    // ---- HUD: the recording light, the replay bar and the notifications ---------------------

    pub(super) fn belumi_recording_text(&self) -> Option<String> {
        self.belumi_replay.recorder.as_ref().map(|r| format!("REC {}", clock(r.elapsed_ms())))
    }

    pub(super) fn belumi_replay_hud(&mut self, batch: &mut HudBatch, bar: bool) {
        let toasts: Vec<String> = self.belumi_replay.toasts.iter().map(|t| t.0.clone()).collect();
        let mut y = 32.0;
        for text in toasts.iter().rev() {
            let tw = self.native_text_width(text);
            let x = ((batch.width - tw) / 2.0).round() - 6.0;
            self.native_fill(batch, [x - 1.0, y - 1.0, tw + 14.0, 16.0], 0xFF2C_3447);
            self.native_fill(batch, [x, y, tw + 12.0, 14.0], 0xF012_1620);
            self.native_fill(batch, [x, y, 2.0, 14.0], 0xFF7B_4DFF);
            self.native_text(batch, text, [x + 7.0, y + 3.0], 0x00FF_FFFF, true);
            y += 17.0;
        }
        let Some(p) = self.belumi_replay.playing.as_ref().filter(|_| bar) else {
            return;
        };
        let (pos, dur) = (p.ctl.position.load(Ordering::Relaxed), p.ctl.duration.load(Ordering::Relaxed).max(1));
        let (paused, speed) = (p.ctl.paused.load(Ordering::Relaxed), p.ctl.speed_milli.load(Ordering::Relaxed));
        let clip = p.clip.clone();
        let w = (batch.width - 40.0).clamp(160.0, 400.0);
        let (x, y) = ((batch.width - w) / 2.0, batch.height - 52.0);
        self.native_fill(batch, [x - 1.0, y - 1.0, w + 2.0, 24.0], 0xFF2C_3447);
        self.native_fill(batch, [x, y, w, 22.0], 0xE012_1620);
        let bar = [x + 8.0, y + 14.0, w - 16.0, 3.0];
        self.native_fill(batch, bar, 0xFF2A_3144);
        if let Some((s, e, _)) = &clip {
            let a = bar[0] + bar[2] * (*s as f32 / dur as f32);
            let b = bar[0] + bar[2] * ((*e).min(dur) as f32 / dur as f32);
            self.native_fill(batch, [a, bar[1], (b - a).max(1.0), 3.0], 0xFF55_3A99);
        }
        let frac = (pos as f32 / dur as f32).clamp(0.0, 1.0);
        self.native_fill(batch, [bar[0], bar[1], bar[2] * frac, 3.0], 0xFF7B_4DFF);
        self.native_fill(batch, [bar[0] + bar[2] * frac - 1.0, bar[1] - 2.0, 3.0, 7.0], 0xFFFF_FFFF);
        let label = format!("{} / {}   {}x{}", clock(pos), clock(dur), f64::from(speed) / 1000.0, if paused { "   PAUSED" } else { "" });
        self.native_text(batch, &label, [x + 8.0, y + 3.0], 0x00FF_FFFF, true);
        let hint = "Space pause  [ ] speed  Arrows seek  V camera";
        let hw = self.native_text_width(hint);
        if hw + self.native_text_width(&label) + 24.0 < w {
            self.native_text(batch, hint, [x + w - hw - 8.0, y + 3.0], 0x008A_93A8, false);
        }
    }

    // ---- the replay pages ------------------------------------------------------------------

    pub(super) fn replay_page_open(&mut self) {
        self.belumi_replay.entries = replay::list(&Self::replays_dir());
        self.belumi_replay.scroll = 0;
    }

    pub(super) fn replay_scroll(&mut self, steps: f64) {
        let rows = self.replay_rows().len();
        let max = rows.saturating_sub(6);
        let now = self.belumi_replay.scroll as i64 - steps.signum() as i64;
        self.belumi_replay.scroll = now.clamp(0, max as i64) as usize;
    }

    /// The list's rows: (replay index, clip index or none).
    fn replay_rows(&self) -> Vec<(usize, Option<usize>)> {
        let mut rows = Vec::new();
        for (i, e) in self.belumi_replay.entries.iter().enumerate() {
            rows.push((i, None));
            for c in 0..e.clips.len() {
                rows.push((i, Some(c)));
            }
        }
        rows
    }

    pub(super) fn draw_replays_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let pw = 440.0_f32.min(w - 12.0);
        let ph = (h - 12.0).clamp(180.0, 300.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Replays and clips");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let r = self.modules().replay.clone();
        let row = y + 26.0;
        self.client_button(batch, [x + 10.0, row, 120.0, 16.0], if r.record { "Recording: ON" } else { "Recording: OFF" }, Some(r.record), Act::Replay(RAct::Record));
        self.native_text(batch, "Clip length", [x + 140.0, row + 4.0], 0x00C8_D0E0, true);
        self.client_button(batch, [x + 204.0, row, 14.0, 16.0], "-", None, Act::Replay(RAct::Clip(-1)));
        self.native_fill(batch, [x + 220.0, row, 40.0, 16.0], 0xFF1A_2030);
        let secs = format!("{} s", r.clip_seconds);
        let sw = self.native_text_width(&secs);
        self.native_text(batch, &secs, [x + 220.0 + (40.0 - sw) / 2.0, row + 4.0], 0x00FF_FFFF, true);
        self.client_button(batch, [x + 262.0, row, 14.0, 16.0], "+", None, Act::Replay(RAct::Clip(1)));
        let keys = format!(
            "Clip: {}   Bookmark: {}",
            horizon_client::keybinds::display_name(&r.clip_key).plain(),
            horizon_client::keybinds::display_name(&r.mark_key).plain()
        );
        self.native_text(batch, &keys, [x + 10.0, row + 22.0], 0x008A_93A8, false);
        let list_y = row + 36.0;
        let rows = self.replay_rows();
        if rows.is_empty() {
            let note = if r.record { "Nothing recorded yet. Join a server and play." } else { "Turn recording on, then join a server: the session is saved." };
            self.native_text(batch, note, [x + 14.0, list_y + 14.0], 0x008A_93A8, false);
        }
        let shown = (((y + ph - 8.0 - list_y) / 18.0).floor() as usize).max(1);
        let start = self.belumi_replay.scroll.min(rows.len().saturating_sub(shown));
        for (n, (i, c)) in rows.iter().copied().skip(start).take(shown).enumerate() {
            let ry = list_y + n as f32 * 18.0;
            let e = self.belumi_replay.entries[i].clone();
            let rect = [x + 6.0, ry, pw - 12.0, 16.0];
            let over = self.hovered(rect);
            self.native_fill(batch, rect, if over { 0xFF22_2A3B } else if c.is_some() { 0xFF16_1C28 } else { 0xFF1A_2030 });
            let (label, info) = match c {
                None => {
                    let when = chrono::DateTime::from_timestamp(e.header.started_unix as i64, 0)
                        .map(|t| t.with_timezone(&chrono::Local).format("%d.%m. %H:%M").to_string())
                        .unwrap_or_default();
                    (format!("{when}  {}", e.header.server), format!("{}  {}", clock(e.duration_ms), size_text(e.bytes)))
                }
                Some(c) => {
                    let clip = &e.clips[c];
                    (format!("   > {}", clip.name), format!("{} - {}", clock(clip.start_ms), clock(clip.end_ms)))
                }
            };
            let mut label = label;
            while self.native_text_width(&label) > pw - 190.0 && label.len() > 4 {
                label.pop();
            }
            self.native_text(batch, &label, [x + 12.0, ry + 4.0], if c.is_some() { 0x00C8_D0E0 } else { 0x00FF_FFFF }, true);
            let iw = self.native_text_width(&info);
            self.native_text(batch, &info, [x + pw - 100.0 - iw, ry + 4.0], 0x008A_93A8, false);
            self.client_accent_button(batch, [x + pw - 92.0, ry + 1.0, 44.0, 14.0], "Play", Act::Replay(RAct::Play(i, c)));
            self.client_button(batch, [x + pw - 46.0, ry + 1.0, 36.0, 14.0], "Del", None, Act::Replay(RAct::Delete(i, c)));
        }
        if rows.len() > shown {
            self.native_text(batch, "wheel: more", [x + pw - 70.0, y + ph - 11.0], 0x008A_93A8, false);
        }
    }

    pub(super) fn draw_replay_controls_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x5000_0000);
        let Some(p) = self.belumi_replay.playing.as_ref() else {
            return;
        };
        let (pos, dur) = (p.ctl.position.load(Ordering::Relaxed), p.ctl.duration.load(Ordering::Relaxed).max(1));
        let (paused, speed) = (p.ctl.paused.load(Ordering::Relaxed), p.ctl.speed_milli.load(Ordering::Relaxed));
        let free = p.free_cam;
        let title = format!("Replay: {} on {}", p.header.player, p.header.server);
        let clip = p.clip.clone();
        let pw = 420.0_f32.min(w - 12.0);
        let (x, y) = ((w - pw) / 2.0, h - 92.0);
        self.panel(batch, [x, y, pw, 82.0], &title);
        let tl = [x + 10.0, y + 28.0, pw - 20.0, 8.0];
        self.belumi_replay.timeline = tl;
        self.native_fill(batch, tl, 0xFF2A_3144);
        if let Some((s, e, _)) = &clip {
            let a = tl[0] + tl[2] * (*s as f32 / dur as f32);
            let b = tl[0] + tl[2] * ((*e).min(dur) as f32 / dur as f32);
            self.native_fill(batch, [a, tl[1], (b - a).max(1.0), tl[3]], 0xFF55_3A99);
        }
        let frac = (pos as f32 / dur as f32).clamp(0.0, 1.0);
        self.native_fill(batch, [tl[0], tl[1], tl[2] * frac, tl[3]], 0xFF7B_4DFF);
        self.native_fill(batch, [tl[0] + tl[2] * frac - 1.0, tl[1] - 2.0, 3.0, tl[3] + 4.0], 0xFFFF_FFFF);
        self.reg([tl[0], tl[1] - 4.0, tl[2], tl[3] + 8.0], Act::Replay(RAct::Seek));
        let time = format!("{} / {}", clock(pos), clock(dur));
        self.native_text(batch, &time, [x + 10.0, y + 42.0], 0x00FF_FFFF, true);
        let by = y + 56.0;
        self.client_button(batch, [x + 10.0, by, 22.0, 16.0], "<<", None, Act::Replay(RAct::Back(5)));
        self.client_accent_button(batch, [x + 36.0, by, 56.0, 16.0], if paused { "Play" } else { "Pause" }, Act::Replay(RAct::Pause));
        self.client_button(batch, [x + 96.0, by, 22.0, 16.0], ">>", None, Act::Replay(RAct::Back(0)));
        self.client_button(batch, [x + 130.0, by, 14.0, 16.0], "-", None, Act::Replay(RAct::Speed(-1)));
        let st = format!("{}x", f64::from(speed) / 1000.0);
        let sw = self.native_text_width(&st);
        self.native_fill(batch, [x + 146.0, by, 36.0, 16.0], 0xFF1A_2030);
        self.native_text(batch, &st, [x + 146.0 + (36.0 - sw) / 2.0, by + 4.0], 0x00FF_FFFF, true);
        self.client_button(batch, [x + 184.0, by, 14.0, 16.0], "+", None, Act::Replay(RAct::Speed(1)));
        self.client_button(batch, [x + 210.0, by, 88.0, 16.0], if free { "Free camera" } else { "Player's view" }, Some(free), Act::Replay(RAct::Cam));
        self.client_button(batch, [x + pw - 112.0, by, 50.0, 16.0], "Close", None, Act::Close);
        self.client_button(batch, [x + pw - 58.0, by, 48.0, 16.0], "Stop", None, Act::Replay(RAct::Stop));
    }

    pub(super) fn replay_do(&mut self, act: RAct) {
        match act {
            RAct::Record => {
                let r = &mut self.modules().replay;
                r.record = !r.record;
                let on = r.record;
                self.modules_changed();
                self.toast(if on { "Recording on: the next session is saved" } else { "Recording off" });
            }
            RAct::Clip(step) => {
                let r = &mut self.modules().replay;
                let next = (i32::from(r.clip_seconds) + step * if r.clip_seconds >= 60 { 30 } else { 10 }).clamp(10, 300);
                r.clip_seconds = next as u16;
                self.modules_changed();
            }
            RAct::Play(i, c) => {
                let Some(e) = self.belumi_replay.entries.get(i).cloned() else {
                    return;
                };
                match c.and_then(|c| e.clips.get(c).cloned()) {
                    Some(clip) => self.belumi_replay_start(e.path, clip.start_ms, clip.end_ms, clip.name),
                    None => self.belumi_replay_start(e.path, 0, u32::MAX, String::new()),
                }
            }
            RAct::Delete(i, c) => {
                let Some(e) = self.belumi_replay.entries.get(i).cloned() else {
                    return;
                };
                match c {
                    Some(c) => {
                        let mut clips = e.clips.clone();
                        if c < clips.len() {
                            clips.remove(c);
                            replay::save_clips(&e.path, &clips);
                        }
                    }
                    None => {
                        if self.belumi_replay.recorder.as_ref().is_some_and(|r| r.path == e.path) {
                            self.toast("That recording is still running");
                            return;
                        }
                        let _ = std::fs::remove_file(&e.path);
                        let _ = std::fs::remove_file(replay::clips_path(&e.path));
                    }
                }
                self.replay_page_open();
            }
            RAct::Pause => {
                if let Some(p) = self.belumi_replay.playing.as_ref() {
                    p.ctl.finished.store(false, Ordering::Relaxed);
                    let now = p.ctl.paused.load(Ordering::Relaxed);
                    p.ctl.paused.store(!now, Ordering::Relaxed);
                }
            }
            RAct::Speed(step) => self.belumi_replay_speed(step),
            RAct::Back(secs) => self.belumi_replay_jump(if secs > 0 { -i64::from(secs) * 1000 } else { 5_000 }),
            RAct::Seek => {
                let tl = self.belumi_replay.timeline;
                let frac = ((self.cursor.0 - tl[0]) / tl[2].max(1.0)).clamp(0.0, 1.0);
                if let Some(p) = self.belumi_replay.playing.as_ref() {
                    let dur = p.ctl.duration.load(Ordering::Relaxed);
                    p.ctl.finished.store(false, Ordering::Relaxed);
                    p.ctl.seek.store((frac * dur as f32) as i64, Ordering::Release);
                }
            }
            RAct::Stop => {
                self.belumi_client_close_screen();
                if let Err(e) = self.leave_server(None) {
                    log::warn!("Horizon: could not leave the replay: {e}");
                }
            }
            RAct::Cam => self.belumi_replay_cam(),
        }
    }
}
