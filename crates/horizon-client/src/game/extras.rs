//! Horizon's extras in the game (`docs/extras.md`); a native mod's minimap
//! (`native_minimap`: its tiles, image and HUD) and zoom (`native_zoom`) on the camera; clicks
//! and frame times for mods (`native_clicks`, `native_frame_rate`).
//! Each hook does nothing while its extra is off and no mod uses it.
use super::{Dim, Game, Screen, native_hooks::NativeMinimap};
use horizon_client::{
    extras::{Corner, Cps, FpsCounter, fov_target},
    extras_hud::{self, Corners},
    hud::{HudBatch, MINIMAP_SPRITE},
    mapengine::{self, MapEngine, Want},
    minimap::View,
    multiplayer,
    reconnect::{Retry, Tracker},
    title::Mode,
};
use std::time::Duration;
use winit::event::MouseButton;

#[derive(Default)]
pub(super) struct ExtrasState {
    fps: FpsCounter,
    /// Left and right mouse buttons: held, and their clicks.
    mouse: [bool; 2],
    cps: [Cps; 2],
    /// The map engine both maps live on.
    pub(super) engine: MapEngine,
    /// The resources the engine's colours were made from.
    colors_for: Option<std::sync::Arc<horizon_client::streaming::WorkerContext>>,
    /// The dimension the map's tiles belong to.
    minimap_dim: Option<Dim>,
    /// The map shows below (a cave or the Nether), and since when the
    /// surroundings ask for the other.
    cave: bool,
    cave_flip: Option<std::time::Instant>,
    /// The canvas rect last uploaded for the minimap, in atlas texels.
    canvas_uploaded: bool,
    /// Auto-reconnect's attempts at the current server.
    reconnect: Tracker,
}
/// Going underground is shown after this long, coming back up after the second.
const CAVE_ENTER: Duration = Duration::from_millis(500);
const CAVE_LEAVE: Duration = Duration::from_millis(1200);

impl Game<'_> {
    fn extras_now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
    pub(super) fn left_button_held(&self) -> bool {
        self.extras.mouse[0]
    }
    /// Mouse buttons while playing, for mods' clicks.
    pub(super) fn extras_mouse(&mut self, button: MouseButton, pressed: bool) {
        self.native_mouse_event(button, pressed);
        let i = match button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            _ => return,
        };
        self.extras.mouse[i] = pressed;
        // For mods (`input.clicks-per-second`, `native_clicks`).
        if pressed {
            let now = self.extras_now();
            self.extras.cps[i].click(now);
        }
    }
    /// Once per frame: the frame's time, and keys let go behind screens.
    pub(super) fn extras_frame(&mut self, elapsed: Duration) {
        // Under agent control the window may lack focus without pausing.
        let focused = self.focused || self.control.is_some();
        let playing = self.screen == Screen::Playing && self.title.is_none() && focused;
        if !playing {
            self.extras.mouse = [false; 2];
        }
        self.update_minimap();
        self.belumi_diagnose_frame();
        self.belumi_death_waypoint();
        // For mods (`render.frame-stats`, `native_frame_rate`).
        let now = self.extras_now();
        self.extras.fps.record(now, elapsed.as_secs_f32() * 1000.0);
        self.native_frame(elapsed);
    }
    /// The frame counter for mods: fps, min, 1% low.
    pub(super) fn frame_reading(&self) -> (u32, u32, u32) {
        let r = self.extras.fps.reading();
        (r.fps, r.min, r.low)
    }
    /// Attack and use clicks in the last second, and whether each is held.
    pub(super) fn mouse_reading(&mut self) -> ((u32, u32), [bool; 2]) {
        let now = self.extras_now();
        let [l, r] = &mut self.extras.cps;
        let cps = (l.per_second(now) as u32, r.per_second(now) as u32);
        (cps, self.extras.mouse)
    }
    /// Auto-reconnect: a connection to `address` as `name` starts.
    pub(super) fn extras_connecting(&mut self, address: &str, name: &str) {
        self.extras.reconnect.connecting(address, name);
        self.belumi_connecting(address);
        self.belumi_plugins_join(address);
    }
    /// Auto-reconnect: the server let the player in.
    pub(super) fn extras_joined(&mut self) {
        self.extras.reconnect.joined(std::time::Instant::now());
        self.belumi_voice_joined();
    }
    /// Auto-reconnect: the player left the server on purpose.
    pub(super) fn extras_left(&mut self) {
        self.extras.reconnect.reset();
        self.belumi_left();
        self.belumi_plugins_leave();
        self.belumi_voice_left();
    }
    /// Auto-reconnect's next attempt after a disconnect, when it is on and
    /// the reason allows one.
    pub(super) fn reconnect_after(&mut self, reason: &str, from_server: bool) -> Option<Retry> {
        if !self.settings.extras.auto_reconnect {
            self.extras.reconnect.reset();
            return None;
        }
        let now = std::time::Instant::now();
        self.extras.reconnect.ended(reason, from_server, now)
    }
    /// Show the attempt's countdown on the Disconnected screen.
    pub(super) fn offer_reconnect(&mut self, retry: Option<Retry>) {
        let Some(retry) = retry else {
            return;
        };
        if let Some(title) = self.title.as_mut()
            && let Mode::Multiplayer(multiplayer::Screen::Disconnected { retry: slot, .. }) =
                &mut title.mode
        {
            log::info!(
                "Horizon: reconnecting to {} in {} s",
                horizon_client::net::shown_address(&retry.address),
                retry.seconds_left(std::time::Instant::now())
            );
            *slot = Some(retry);
        }
    }
    /// Auto-reconnect: connect again once the countdown is over. An
    /// online-mode server gets the signed-in account again: `connect_server`
    /// takes it from `auth::current()`, held in memory for the whole run
    /// (the launcher's session file is gone after the first read).
    pub(super) fn extras_retry(&mut self) -> horizon_client::Result<()> {
        if let Some(retry) = self.title.as_mut().and_then(|t| t.due_retry()) {
            self.connect_server(&retry.address, &retry.name)?;
        }
        Ok(())
    }
    /// The minimap a mod shows (`native_minimap`): its corner, its side
    /// in GUI pixels and its view of the player.
    fn minimap_shown(&self) -> Option<(Corner, f32, View)> {
        let p = self.player.as_ref()?;
        if self.title.is_some() || self.ui.hide_gui {
            return None;
        }
        let m = self.minimap_wanted()?;
        let view = View {
            x: p.position.x,
            z: p.position.z,
            yaw: self.yaw,
            rotate: m.rotate,
            blocks_per_pixel: m.blocks_per_pixel,
            round: m.round,
        };
        Some((m.corner, m.size, view))
    }
    /// The minimap shown: a native mod's (`native_minimap`).
    fn minimap_wanted(&self) -> Option<NativeMinimap> {
        self.native_minimap_now()
    }
    /// The map shows below ground (a cave or the Nether).
    pub(super) fn map_in_cave(&self) -> bool {
        self.extras.cave
    }

    /// Once per frame: send new or changed chunks to the map workers, ask for a
    /// canvas when the view needs one, and upload the one a worker finished.
    fn update_minimap(&mut self) {
        let shown = self.minimap_shown();
        let world_map = self.settings.belumi.worldmap && self.player.is_some();
        if shown.is_none() && !world_map {
            if self.minimap_wanted().is_none() || self.player.is_none() {
                self.extras.engine.clear();
                self.extras.minimap_dim = None;
            }
            return;
        }
        if self.extras.minimap_dim != Some(self.dim) {
            self.extras.engine.clear();
            self.extras.minimap_dim = Some(self.dim);
            self.extras.cave = self.dim == Dim::Nether;
            self.extras.cave_flip = None;
        }
        if self
            .extras
            .colors_for
            .as_ref()
            .is_none_or(|c| !std::sync::Arc::ptr_eq(c, &self.pack.context))
        {
            let colors = horizon_client::mapcolors::BlockColors::new(
                self.pack.context.clone(),
                self.res.facts().states.len(),
            );
            self.extras.engine.set_colors(std::sync::Arc::new(colors));
            self.extras.colors_for = Some(self.pack.context.clone());
        }
        self.extras.engine.set_record(world_map);
        let Some(p) = self.player.as_ref() else {
            return;
        };
        let head = (p.position.y + f64::from(p.dimensions.eye_height)).floor() as i32;
        let (bx, bz) = (p.position.x.floor() as i32, p.position.z.floor() as i32);
        let covered = self.dim == Dim::Nether || self.world.light(bx, head, bz) >> 4 == 0;
        if covered == self.extras.cave {
            self.extras.cave_flip = None;
        } else {
            let since = *self.extras.cave_flip.get_or_insert_with(std::time::Instant::now);
            let wait = if covered { CAVE_ENTER } else { CAVE_LEAVE };
            if self.dim == Dim::Nether || since.elapsed() >= wait {
                self.extras.cave = covered;
                self.extras.cave_flip = None;
            }
        }
        let cave = self.extras.cave;
        let render = self.settings.render_distance.max(2);
        let mut surface_radius = if world_map { render.clamp(2, 10) } else { 0 };
        let mut cave_plan = None;
        if let Some((_, _, view)) = &shown {
            // The chunks the canvas can reach.
            let reach = mapengine::WINDOW as f32 / 2.0
                * view.blocks_per_pixel
                * std::f32::consts::SQRT_2;
            let radius = (((reach / 16.0).ceil().clamp(0.0, 4096.0)) as i32).saturating_add(1).min(render);
            if cave {
                cave_plan = Some((radius, head));
            } else {
                surface_radius = surface_radius.max(radius);
            }
        }
        if cave && world_map {
            let keep = render.clamp(2, 8);
            cave_plan = Some(cave_plan.map_or((keep, head), |(r, h)| (r.max(keep), h)));
        }
        if self.dim == Dim::Nether {
            surface_radius = -1;
        }
        self.extras.engine.update(
            &self.world,
            self.res.facts(),
            (bx >> 4, bz >> 4),
            surface_radius,
            cave_plan,
        );
        let Some((_, _, view)) = shown else {
            return;
        };
        self.extras.engine.want_canvas(Want {
            x: view.x,
            z: view.z,
            bpp: view.blocks_per_pixel,
            cave,
            overlay: self.belumi_map_overlay(),
        });
        let Some(canvas) = self.extras.engine.take_canvas() else {
            return;
        };
        let Some(rect) = self.gui.atlas.sprites.get(MINIMAP_SPRITE).copied() else {
            return;
        };
        let (w, h) = (self.gui.atlas.width as f32, self.gui.atlas.height as f32);
        self.map_patches.push(horizon_client::font::Patch {
            x: (rect.u0 * w).round() as u32,
            y: (rect.v0 * h).round() as u32,
            width: mapengine::CANVAS as u32,
            height: mapengine::CANVAS as u32,
            rgba: canvas.image,
        });
        self.extras.canvas_uploaded = true;
    }
    /// A mod's zoom in effect (`native_zoom`): (factor, smooth, turning
    /// scale); none is 1.
    fn zoom_now(&self) -> (f32, bool, f64) {
        self.native_zoom_now().map_or((1.0, true, 1.0), |z| {
            (z.factor, z.smooth, f64::from(z.turn))
        })
    }
    /// The field-of-view modifier target, divided by a smooth zoom.
    pub(super) fn zoomed_fov_target(&self, vanilla: f32) -> f32 {
        match self.zoom_now() {
            (factor, true, _) => fov_target(vanilla, factor),
            _ => vanilla,
        }
    }
    /// The field of view in degrees, divided by an instant zoom.
    pub(super) fn zoomed_fov_degrees(&self, fov: f32) -> f32 {
        match self.zoom_now() {
            (factor, false, _) => fov / factor,
            _ => fov,
        }
    }
    /// Mouse turning while zoomed.
    pub(super) fn zoom_turn_scale(&self) -> f64 {
        self.zoom_now().2
    }
    /// The minimap and mods' corner panels, in the HUD layer (hidden with F1
    /// by the caller). The top left corner gives way to the debug screen.
    pub(super) fn draw_extras(&mut self, batch: &mut HudBatch) {
        let map = self.minimap_shown();
        let waypoint_dots = match &map {
            Some((_, size, view)) => self.map_waypoint_dots(view, *size),
            None => Vec::new(),
        };
        let visible = |corner| !(self.debug && corner == Corner::TopLeft);
        let mut corners = Corners::new(batch.width, batch.height, self.effect_rows_height());
        let atlas = &self.gui.atlas;
        if let Some((map_corner, map_size, view)) = map
            && visible(map_corner)
            && let Some((cx, cz, bpp, _)) = self.extras.engine.shown()
            && let Some(image) = atlas.sprites.get(MINIMAP_SPRITE).copied()
        {
            let (arrow, north) = if view.rotate {
                (0.0, Some(view.north()))
            } else {
                (180.0 + view.yaw, None)
            };
            let mut dots = self.map_entity_dots(&view, map_size);
            dots.extend(waypoint_dots);
            let (right, forward) = view.axes();
            let look = extras_hud::MinimapLook {
                corner: map_corner,
                size: map_size,
                image,
                arrow,
                north,
                dots: &dots,
                edge: if self.settings.belumi.ui_style == 0 {
                    horizon_client::hud::rgba(10, 10, 10, 230)
                } else {
                    horizon_client::hud::rgba(255, 255, 255, 90)
                },
                canvas: extras_hud::CanvasLook {
                    centre: (cx, cz),
                    blocks_per_texel: f64::from(bpp),
                    player: (view.x, view.z),
                    right,
                    forward,
                    round: view.round,
                    texels: mapengine::CANVAS as f32,
                    window: mapengine::WINDOW as f32,
                },
            };
            let (mx, my) = extras_hud::draw_minimap(batch, atlas, &mut corners, &look);
            if self.map_icons_wanted() {
                let (right, forward) = view.axes();
                let per_block = f64::from(map_size) / (horizon_client_render::minimap::SIZE as f64 * f64::from(view.blocks_per_pixel));
                let half = f64::from(map_size) / 2.0 - 5.0;
                for row in self.map_icon_rows() {
                    let (dx, dz) = (row.pos[0] - view.x, row.pos[2] - view.z);
                    let x = (dx * right[0] + dz * right[1]) * per_block;
                    let y = -(dx * forward[0] + dz * forward[1]) * per_block;
                    let inside = if view.round { x.hypot(y) <= half } else { x.abs() <= half && y.abs() <= half };
                    if inside {
                        self.draw_map_icon(batch, &row, mx + x as f32, my + y as f32, 8.0);
                    }
                }
            }
        }
        self.native_hud(batch, &mut corners);
    }
    /// The height the status effect icons take at the top right
    /// (`Hud.extractEffects`: beneficial at y 1, harmful at y 26).
    fn effect_rows_height(&self) -> f32 {
        let effects = &self.potions.effects.list;
        if effects.is_empty() || self.screen != Screen::Playing {
            return 0.0;
        }
        let harmful = effects.iter().any(|e| {
            horizon_client::brewing::effect(e.id)
                .is_some_and(|d| d.category != horizon_client::brewing::Category::Beneficial)
        });
        if harmful { 50.0 } else { 25.0 }
    }
}
