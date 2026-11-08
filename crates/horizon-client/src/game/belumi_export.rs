//! Replay as a video: while a replay plays, F7 (or the Video button) saves the picture of the game at the chosen rate, with
//! the time of the replay each one belongs to; at the end (or on F7 again) `ffmpeg`, when it is installed, makes an MP4 of
//! them at the right speed, whatever frames the machine managed to take. Without ffmpeg the pictures stay in their folder
//! with the command that makes the video. The HUD is left out of the pictures.
use super::Game;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) const FPS: u32 = 30;

struct Job {
    dir: PathBuf,
    /// The replay time (ms) of each picture and its file.
    frames: Vec<(u32, PathBuf)>,
    next_due: u32,
    hid_gui: bool,
    out: PathBuf,
}

#[derive(Default)]
pub(super) struct Export {
    job: Option<Job>,
    encoding: Option<mpsc::Receiver<Result<PathBuf, String>>>,
}

/// The ffmpeg concat list: each picture lasts until the next one's time, the last for one frame.
pub(super) fn concat_list(frames: &[(u32, String)], fps: u32) -> String {
    let mut out = String::new();
    for (i, (t, file)) in frames.iter().enumerate() {
        let secs = match frames.get(i + 1) {
            Some((next, _)) => f64::from(next.saturating_sub(*t)) / 1000.0,
            None => 1.0 / f64::from(fps.max(1)),
        };
        out.push_str(&format!("file '{file}'\nduration {:.4}\n", secs.max(0.001)));
    }
    // The concat demuxer ignores the last duration unless the file is named again.
    if let Some((_, last)) = frames.last() {
        out.push_str(&format!("file '{last}'\n"));
    }
    out
}

/// The arguments that turn the list into an MP4 of constant `fps`.
pub(super) fn ffmpeg_args(list: &str, fps: u32, out: &str) -> Vec<String> {
    ["-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", list, "-vf"]
        .into_iter()
        .map(str::to_owned)
        .chain([format!("fps={fps},format=yuv420p"), "-c:v".into(), "libx264".into(), "-crf".into(), "18".into(), out.to_owned()])
        .collect()
}

/// The next picture is due this many replay milliseconds after the last.
pub(super) fn interval_ms(fps: u32) -> u32 {
    (1000 / fps.max(1)).max(1)
}

fn stamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).to_string()
}

fn ffmpeg_present() -> bool {
    Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn escape(path: &Path) -> String {
    path.display().to_string().replace('\'', "'\\''")
}

impl Game<'_> {
    pub(super) fn belumi_export_active(&self) -> bool {
        self.belumi_export.job.is_some()
    }

    pub(super) fn belumi_export_toggle(&mut self) {
        if self.belumi_export.job.is_some() {
            self.belumi_export_finish();
            return;
        }
        if self.belumi_replay.playing_none() {
            self.toast("Play a replay first, then press F7");
            return;
        }
        let dir = std::env::current_dir().unwrap_or_default().join("replays").join(format!("video-{}", stamp()));
        if std::fs::create_dir_all(&dir).is_err() {
            self.toast("Could not make a folder for the pictures");
            return;
        }
        let out = dir.join("replay.mp4");
        let pos = self.belumi_replay.position().unwrap_or(0);
        let hid_gui = self.ui.hide_gui;
        self.ui.hide_gui = true;
        self.belumi_export.job = Some(Job { dir, frames: Vec::new(), next_due: pos, hid_gui, out });
        self.toast("Recording a video: close this screen. F7 stops it.");
    }

    /// Once a frame: takes the next picture when it is due, ends the video when the replay does.
    pub(super) fn belumi_export_frame(&mut self) {
        if let Some(rx) = self.belumi_export.encoding.as_ref()
            && let Ok(result) = rx.try_recv()
        {
            self.belumi_export.encoding = None;
            match result {
                Ok(path) => self.toast(format!("Video saved: {}", path.display())),
                Err(e) => self.toast(e),
            }
        }
        if self.belumi_export.job.is_none() {
            return;
        }
        let Some(pos) = self.belumi_replay.position() else {
            self.belumi_export_finish();
            return;
        };
        let finished = self.belumi_replay.finished();
        let due = self.belumi_export.job.as_ref().is_some_and(|j| pos >= j.next_due);
        if due && !finished {
            let Some(r) = self.renderer.as_mut() else { return };
            if r.screenshot.is_some() {
                return;
            }
            let Some(job) = self.belumi_export.job.as_mut() else { return };
            let path = job.dir.join(format!("{:06}.png", job.frames.len()));
            r.screenshot = Some(horizon_client::gfx::ScreenshotRequest { path: path.clone(), size: None });
            job.frames.push((pos, path));
            job.next_due = pos + interval_ms(FPS);
        }
        if finished {
            self.belumi_export_finish();
        }
    }

    fn belumi_export_finish(&mut self) {
        let Some(job) = self.belumi_export.job.take() else { return };
        self.ui.hide_gui = job.hid_gui;
        // The last picture may still be on its way to the disk.
        if job.frames.len() < 2 {
            self.toast("Too short for a video");
            let _ = std::fs::remove_dir_all(&job.dir);
            return;
        }
        let names: Vec<(u32, String)> =
            job.frames.iter().map(|(t, p)| (*t, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).collect();
        let list = job.dir.join("frames.txt");
        let _ = std::fs::write(&list, concat_list(&names, FPS));
        let args = ffmpeg_args("frames.txt", FPS, "replay.mp4");
        if !ffmpeg_present() {
            let line = format!("cd '{}' && ffmpeg {}", escape(&job.dir), args.join(" "));
            let _ = std::fs::write(job.dir.join("make-video.sh"), format!("#!/bin/sh\n{line}\n"));
            self.toast(format!("{} pictures in {}. ffmpeg is not installed; with it, run make-video.sh there.", job.frames.len(), job.dir.display()));
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.belumi_export.encoding = Some(rx);
        self.toast("Making the video...");
        let (dir, out) = (job.dir.clone(), job.out.clone());
        std::thread::spawn(move || {
            // The pictures are written by the renderer; give the last one a moment.
            std::thread::sleep(std::time::Duration::from_millis(800));
            let status = Command::new("ffmpeg").args(&args).current_dir(&dir).stdout(Stdio::null()).stderr(Stdio::piped()).output();
            let result = match status {
                Ok(o) if o.status.success() => {
                    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.ends_with(".png") || name == "frames.txt" {
                            let _ = std::fs::remove_file(e.path());
                        }
                    }
                    Ok(out)
                }
                Ok(o) => Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""))),
                Err(e) => Err(format!("ffmpeg could not start: {e}")),
            };
            let _ = tx.send(result);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_picture_lasts_until_the_next_and_the_last_names_itself_again() {
        let frames = vec![(1000, "a.png".to_owned()), (1033, "b.png".to_owned()), (1100, "c.png".to_owned())];
        let list = concat_list(&frames, 30);
        assert_eq!(list, "file 'a.png'\nduration 0.0330\nfile 'b.png'\nduration 0.0670\nfile 'c.png'\nduration 0.0333\nfile 'c.png'\n");
    }

    #[test]
    fn the_command_makes_a_constant_rate_mp4() {
        let args = ffmpeg_args("frames.txt", 30, "replay.mp4");
        assert_eq!(args.first().map(String::as_str), Some("-y"));
        assert!(args.windows(2).any(|w| w == ["-i", "frames.txt"]));
        assert!(args.contains(&"fps=30,format=yuv420p".to_owned()));
        assert_eq!(args.last().map(String::as_str), Some("replay.mp4"));
    }

    #[test]
    fn the_interval_follows_the_rate() {
        assert_eq!(interval_ms(30), 33);
        assert_eq!(interval_ms(60), 16);
        assert_eq!(interval_ms(0), 1000);
    }

    #[test]
    fn quotes_in_a_path_are_escaped_for_the_shell() {
        assert_eq!(escape(Path::new("/a/b'c")), "/a/b'\\''c");
    }
}
