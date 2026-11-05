use super::{Game, Screen};
use std::time::Instant;

const FIRST: f32 = 9.0;
const STEP: f32 = 6.0;
const SETTLE: f32 = 2.0;
const SHOTS: u8 = 4;

#[derive(Default)]
pub(super) struct DiagnoseState {
    since: Option<Instant>,
    taken: u8,
}

impl Game<'_> {
    /// `--diagnose-shots DIR`: once the world is up, turn a quarter every few seconds and
    /// save a screenshot each time, then quit; for the launcher's diagnose run.
    pub(super) fn belumi_diagnose_frame(&mut self) {
        let Some(dir) = self.options.diagnose_shots.clone() else {
            return;
        };
        // A run nobody watches must not stop when the window has no focus.
        if self.screen == Screen::Paused {
            self.screen = Screen::Playing;
        }
        let lit = self.player.as_ref().is_some_and(|p| {
            self.streaming.is_lit(
                (p.position.x.floor() as i32) >> 4,
                (p.position.z.floor() as i32) >> 4,
            )
        });
        if self.screen != Screen::Playing || self.renderer.is_none() || !lit {
            self.belumi_diag.since = None;
            return;
        }
        let t = self.belumi_diag.since.get_or_insert_with(Instant::now).elapsed().as_secs_f32();
        let k = self.belumi_diag.taken;
        if k >= SHOTS {
            if t > FIRST + STEP * f32::from(SHOTS) + 1.0 {
                self.options.world_frames = Some(self.world_frames + 1);
            }
            return;
        }
        let at = FIRST + STEP * f32::from(k);
        if t >= at && t < at + SETTLE {
            self.yaw = f32::from(k) * 90.0;
            self.pitch = -4.0;
        } else if t >= at + SETTLE
            && let Some(renderer) = self.renderer.as_mut()
            && renderer.screenshot.is_none()
        {
            let _ = std::fs::create_dir_all(&dir);
            renderer.screenshot = Some(horizon_client::gfx::ScreenshotRequest {
                path: dir.join(format!("shot-{k}.png")),
                size: None,
            });
            self.belumi_diag.taken += 1;
        }
    }
}
