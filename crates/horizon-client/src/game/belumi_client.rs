//! The client screen (Right Shift): the HUD builder, where the HUD modules are dragged into place, and
//! the module pages for everything that has no spot on the HUD. Also the drawing of the HUD modules.

use super::{Game, Screen, perspective::CameraType};
use winit::keyboard::KeyCode;
use horizon_client::{
    hud::HudBatch,
    hud_layout::{self, GuiDraw, PoseOp},
    items::ItemStack,
    text::Text,
};
use horizon_client_render::modules::{self, Modules, Placed};

const BG: u32 = 0xF012_1620;
const HEAD: u32 = 0xFF1A_2030;
const EDGE: u32 = 0xFF2C_3447;
const ACCENT: u32 = 0xFF7B_4DFF;
const ACCENT_HOVER: u32 = 0xFF95_6DFF;
const BUTTON: u32 = 0xFF2A_3144;
const BUTTON_HOVER: u32 = 0xFF3A_4360;
const ON: u32 = 0xFF2F_7D4A;
const ON_HOVER: u32 = 0xFF3C_9B5C;
const OFF_TEXT: u32 = 0x00AA_B3C6;
const GOLD: u32 = 0x00F2_C14E;
const SNAP_REACH: f32 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Page {
    #[default]
    Builder,
    Modules,
    Crosshair,
    Chat,
    Discord,
    Accounts,
    Changes,
    Chests,
    Combat,
    Redstone,
    Blueprint,
    Sounds,
    Plugin,
    Replays,
    Replay,
}

/// What a click on a registered widget does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Act {
    Page(Page),
    Close,
    Starter,
    Toggle(String),
    Select(String),
    ScaleDown,
    ScaleUp,
    Preset(usize),
    DiscordFlag(&'static str),
    Channel(u8, i32),
    BgDown,
    BgUp,
    Flag(String, String),
    Sprint,
    Sneak,
    Freelook,
    CrosshairOn,
    Style(i32),
    Colour(u32),
    Step(&'static str, i32),
    Switch(&'static str),
    ChatFlag(&'static str),
    ChatHistory(i32),
    FocusBox(usize),
    Tooltips,
    ContainerPreview,
    DistantChunks,
    Trail,
    ChangesOn,
    ChestsOn,
    CopyJoin,
    CombatOn,
    SoundOn,
    HistoryOn,
    RedstoneNumbers,
    PingsOn,
    AccurateOn,
    LightOn,
    GammaCycle,
    RiptideFix,
    ConfirmCycle,
    SoundLevel(String, u8),
    SoundsReset,
    AccurateBreak,
    BlueprintLayer(i32),
    TickCmd(&'static str),
    CombatClear,
    ChestMark(usize),
    ChangeMark(usize),
    ChangesClear,
    PickAccount(usize),
    PluginOpen(String),
    PluginPress(String, String, bool),
    Reconnect,
    Warning,
    Extra(&'static str),
    Replay(super::belumi_replay::RAct),
}

#[derive(Default)]
pub(super) struct ClientState {
    pub(super) saved: Option<super::belumi_accounts::Saved>,
    /// The game ends at the next chance: an account switch was asked for.
    pub(super) switch_quit: bool,
    /// The plugin whose screen the Plugin page shows; none shows the list.
    pub(super) plugin_page: Option<String>,
    modules: Option<Modules>,
    page: Page,
    selected: Option<String>,
    /// The module being dragged and where inside it the pointer holds it.
    drag: Option<(String, [f32; 2])>,
    guides: Vec<(bool, f32)>,
    widgets: Vec<([f32; 4], Act)>,
    mouse_down: bool,
    boxes: Option<(horizon_client::ui::EditBox, horizon_client::ui::EditBox)>,
    dirty: bool,
    last_save: Option<std::time::Instant>,
    freelook: Option<Freelook>,
    /// How far each keystrokes key is lit (0 to 1), eased so a press fades in and out.
    fade: std::collections::BTreeMap<String, f32>,
    /// Seconds since the last frame, for the fades.
    fade_dt: f32,
    fade_at: Option<std::time::Instant>,
    /// First visible row of the Modules page.
    modules_row: usize,
    /// The address of the server this game is connected to.
    server: String,
    /// Earlier HUD layouts, newest last (Ctrl+Z in the builder).
    undo: Vec<Vec<Placed>>,
}

pub(super) struct Freelook {
    yaw: f32,
    pitch: f32,
    saved: CameraType,
}

/// A module's content, in its own unscaled pixels.
struct Built {
    w: f32,
    h: f32,
    draws: Vec<GuiDraw>,
    /// Items to draw on top: the stack and where.
    items: Vec<(ItemStack, f32, f32)>,
    /// What stands in for the module in the builder when it has nothing to show.
    empty: bool,
}

/// `a` to `b` by `t`, channel by channel (ARGB).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    (0..4).fold(0, |out, i| {
        let sh = i * 8;
        let (x, y) = (((a >> sh) & 0xFF) as f32, ((b >> sh) & 0xFF) as f32);
        out | (((x + (y - x) * t).round() as u32) << sh)
    })
}

fn fill(draws: &mut Vec<GuiDraw>, x: f32, y: f32, w: f32, h: f32, color: u32) {
    draws.push(GuiDraw::Fill { x0: x as i32, y0: y as i32, x1: (x + w) as i32, y1: (y + h) as i32, color });
}

fn text(draws: &mut Vec<GuiDraw>, s: &str, x: f32, y: f32, color: u32) {
    draws.push(GuiDraw::Text { text: Text::literal(s), x: x as i32, y: y as i32, color: color | 0xFF00_0000, shadow: true });
}

impl Game<'_> {
    pub(super) fn modules(&mut self) -> &mut Modules {
        self.belumi_client.modules.get_or_insert_with(|| Modules::load(&Self::belumi_dir_pub()))
    }

    pub(super) fn modules_ref(&self) -> Option<&Modules> {
        self.belumi_client.modules.as_ref()
    }

    fn belumi_dir_pub() -> std::path::PathBuf {
        std::env::current_dir().unwrap_or_default()
    }

    /// Remembers the HUD layout before a change, for Ctrl+Z.
    pub(super) fn belumi_client_server(&self) -> &str {
        &self.belumi_client.server
    }

    pub(super) fn belumi_client_set_server(&mut self, address: &str) {
        self.belumi_client.server = address.to_owned();
    }

    fn undo_push(&mut self) {
        let Some(hud) = self.modules_ref().map(|m| m.hud.clone()) else {
            return;
        };
        let undo = &mut self.belumi_client.undo;
        if undo.last() != Some(&hud) {
            undo.push(hud);
            if undo.len() > 50 {
                undo.remove(0);
            }
        }
    }

    fn undo_pop(&mut self) {
        if let Some(hud) = self.belumi_client.undo.pop() {
            self.modules().hud = hud;
            self.modules_changed();
        }
    }

    pub(super) fn modules_changed(&mut self) {
        self.belumi_client.dirty = true;
    }

    fn modules_save_now(&mut self) {
        if let Some(m) = self.belumi_client.modules.as_ref() {
            m.save(&Self::belumi_dir_pub());
        }
        self.belumi_client.dirty = false;
    }

    /// Writes the layout a moment after it changed (not on every pixel of a drag).
    pub(super) fn modules_tick(&mut self) {
        self.belumi_replay_tick();
        let tools = self.modules().chat.clone();
        if self.ui.chat.tools != tools {
            self.ui.chat.tools = tools.clone();
        }
        self.ui.chat.local_seconds = chrono::Timelike::num_seconds_from_midnight(&chrono::Local::now());
        if self.ui.chat.me.is_empty()
            && let Some(auth) = horizon_client::auth::current()
        {
            self.ui.chat.me = auth.name.clone();
        }
        let mentions = self.ui.chat.take_mentions();
        if mentions > 0 && tools.ping_sound {
            self.audio.play_ping();
        }
        if !self.belumi_client.dirty {
            return;
        }
        let due = self.belumi_client.last_save.is_none_or(|t| t.elapsed().as_secs_f32() > 1.0);
        if due && !(self.screen == Screen::BelumiClient && self.belumi_client.mouse_down) {
            self.belumi_client.last_save = Some(std::time::Instant::now());
            self.modules_save_now();
        }
    }

    /// The player's own crosshair, in place of the game's. Returns false when none is set up.
    pub(super) fn belumi_crosshair_draw(&mut self, batch: &mut HudBatch) -> bool {
        let Some(c) = self.modules_ref().map(|m| m.crosshair.clone()).filter(|c| c.enabled) else {
            return false;
        };
        let moving = c.dynamic
            && [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD].iter().any(|k| self.keys.contains(k));
        let spread = if moving { 3.0 } else { 0.0 };
        let (cx, cy) = ((batch.width / 2.0).floor(), (batch.height / 2.0).floor());
        for piece in c.pieces(spread) {
            self.native_fill(batch, [cx + piece.x.round(), cy + piece.y.round(), piece.w.round().max(1.0), piece.h.round().max(1.0)], piece.color);
        }
        true
    }

    // ---- freelook --------------------------------------------------------------------------

    pub(super) fn belumi_freelook_active(&self) -> bool {
        self.belumi_client.freelook.is_some()
    }

    /// The camera's angles while looking around, instead of the player's.
    pub(super) fn belumi_freelook_angles(&self) -> Option<(f32, f32)> {
        self.belumi_client.freelook.as_ref().map(|f| (f.yaw, f.pitch))
    }

    pub(super) fn belumi_freelook_turn(&mut self, xo: f64, yo: f64) {
        if let Some(f) = self.belumi_client.freelook.as_mut() {
            f.yaw += (xo * 0.15) as f32;
            f.pitch = (f.pitch + (yo * 0.15) as f32).clamp(-90.0, 90.0);
        }
    }

    fn freelook_key(&mut self, key: &str, pressed: bool) {
        let (on, wanted) = {
            let m = self.modules();
            (m.misc.freelook, m.misc.freelook_key.clone())
        };
        if !on || key != wanted {
            return;
        }
        if pressed && self.native_playing() && !self.belumi_freecam_active() {
            if self.belumi_client.freelook.is_none() {
                let saved = self.camera_type;
                self.belumi_client.freelook = Some(Freelook { yaw: self.yaw, pitch: self.pitch, saved });
                if saved == CameraType::FirstPerson {
                    self.camera_type = CameraType::ThirdPersonBack;
                }
            }
        } else if !pressed && let Some(f) = self.belumi_client.freelook.take() {
            self.camera_type = f.saved;
        }
    }

    // ---- the HUD modules ------------------------------------------------------------------

    fn mapping_label(&mut self, name: &str) -> (bool, String) {
        self.native_mapping(name).unwrap_or((false, String::new()))
    }

    /// How lit a key is: eases towards 1 while held and back to 0 after (about 0.12 s each way).
    fn key_fade(&mut self, name: &str, down: bool) -> f32 {
        let step = (self.belumi_client.fade_dt / 0.12).min(1.0);
        let v = self.belumi_client.fade.entry(name.to_owned()).or_insert(0.0);
        *v = if down { (*v + step).min(1.0) } else { (*v - step).max(0.0) };
        *v
    }

    fn short_label(label: &str) -> String {
        let l = label.trim();
        if l.chars().count() <= 3 { l.to_uppercase() } else { l.chars().take(3).collect() }
    }

    fn build_module(&mut self, p: &Placed) -> Built {
        let mut draws = Vec::new();
        let mut items = Vec::new();
        let (w, h, empty);
        match p.id.as_str() {
            "keystrokes" => {
                let cell = 18.0;
                let gap = 2.0;
                let mouse = p.flag("mouse", true);
                let space = p.flag("space", true);
                let total_w = cell * 3.0 + gap * 2.0;
                let mut y = 0.0;
                let key = |draws: &mut Vec<GuiDraw>, this: &mut Self, name: &str, x: f32, y: f32, kw: f32, kh: f32, label: Option<String>| {
                    let (down, shown) = this.mapping_label(name);
                    let label = label.unwrap_or_else(|| Self::short_label(&shown));
                    let lit = this.key_fade(name, down);
                    fill(draws, x, y, kw, kh, mix(0x9000_0000, 0xE0FF_FFFF, lit));
                    let lw = HudBatch::text_width(&this.gui.atlas, &label);
                    let color = mix(0xFFFF_FFFF, 0xFF18_1820, lit);
                    draws.push(GuiDraw::Text { text: Text::literal(&label), x: (x + (kw - lw) / 2.0) as i32, y: (y + (kh - 8.0) / 2.0) as i32, color, shadow: lit < 0.5 });
                };
                key(&mut draws, self, "key.forward", cell + gap, y, cell, cell, None);
                y += cell + gap;
                key(&mut draws, self, "key.left", 0.0, y, cell, cell, None);
                key(&mut draws, self, "key.back", cell + gap, y, cell, cell, None);
                key(&mut draws, self, "key.right", 2.0 * (cell + gap), y, cell, cell, None);
                y += cell + gap;
                if mouse {
                    let (l, r) = self.native_clicks();
                    let half = (total_w - gap) / 2.0;
                    for (i, (name, label, n)) in [("key.attack", "LMB", l), ("key.use", "RMB", r)].into_iter().enumerate() {
                        let x = i as f32 * (half + gap);
                        let (down, _) = self.mapping_label(name);
                        let lit = self.key_fade(name, down);
                        fill(&mut draws, x, y, half, cell + 4.0, mix(0x9000_0000, 0xE0FF_FFFF, lit));
                        let color = mix(0xFFFF_FFFF, 0xFF18_1820, lit);
                        let lw = HudBatch::text_width(&self.gui.atlas, label);
                        draws.push(GuiDraw::Text { text: Text::literal(label), x: (x + (half - lw) / 2.0) as i32, y: (y + 3.0) as i32, color, shadow: lit < 0.5 });
                        let count = format!("{n}");
                        let cw = HudBatch::text_width(&self.gui.atlas, &count);
                        draws.push(GuiDraw::Text { text: Text::literal(&count), x: (x + (half - cw) / 2.0) as i32, y: (y + 12.0) as i32, color: mix(0xFFAA_B3C6, 0xFF55_5566, lit), shadow: false });
                    }
                    y += cell + 4.0 + gap;
                }
                if space {
                    let (down, _) = self.mapping_label("key.jump");
                    let lit = self.key_fade("key.jump", down);
                    fill(&mut draws, 0.0, y, total_w, 10.0, mix(0x9000_0000, 0xE0FF_FFFF, lit));
                    fill(&mut draws, 14.0, y + 4.0, total_w - 28.0, 2.0, mix(0xFFAA_B3C6, 0xFF18_1820, lit));
                    y += 10.0 + gap;
                }
                (w, h, empty) = (total_w, y - gap, false);
            }
            "cps" => {
                let (l, r) = self.native_clicks();
                let label = if p.flag("split", true) { format!("{l} | {r} CPS") } else { format!("{} CPS", l + r) };
                let tw = HudBatch::text_width(&self.gui.atlas, &label);
                fill(&mut draws, 0.0, 0.0, tw + 8.0, 14.0, 0x9000_0000);
                let rate = l.max(r);
                let color = if rate >= 12 { 0x004D_FF6A } else if rate >= 7 { 0x00FF_E14D } else { 0x00FF_FFFF };
                text(&mut draws, &label, 4.0, 3.0, color);
                (w, h, empty) = (tw + 8.0, 14.0, false);
            }
            "effects" => {
                let icons = p.flag("icons", true);
                let ambient = p.flag("ambient", true);
                let mut shown: Vec<_> = self.potions.effects.list.iter().filter(|e| e.visible && (ambient || !e.ambient)).collect();
                // Permanent effects last, the rest by the time they have left.
                shown.sort_by_key(|e| if e.duration < 0 { i32::MAX } else { e.duration });
                let blink = (self.started.elapsed().as_millis() / 300) % 2 == 0;
                let mut lines: Vec<(String, u32, String)> = Vec::new();
                for e in shown {
                    let name = horizon_client::lang::get_or(&format!("effect.minecraft.{}", e.id), e.id);
                    let level = match e.amplifier {
                        0 => String::new(),
                        n => format!(" {}", roman(n + 1)),
                    };
                    let time = if e.duration < 0 {
                        "inf".to_owned()
                    } else {
                        let s = e.duration / 20;
                        format!("{}:{:02}", s / 60, s % 60)
                    };
                    let color = if e.duration >= 0 && e.duration < 200 {
                        if blink { 0x00FF_6B6B } else { 0x00FF_C0C0 }
                    } else if e.duration >= 0 && e.duration < 600 {
                        0x00FF_E14D
                    } else {
                        0x00FF_FFFF
                    };
                    lines.push((format!("{name}{level}  {time}"), color, e.id.to_string()));
                }
                empty = lines.is_empty();
                if empty {
                    lines.push(("No effects".into(), 0x00AA_B3C6, String::new()));
                }
                let row = if icons { 12.0 } else { 10.0 };
                let pad = if icons { 14.0 } else { 0.0 };
                let tw = lines.iter().map(|l| HudBatch::text_width(&self.gui.atlas, &l.0)).fold(0.0, f32::max);
                let (bw, bh) = (tw + 8.0 + pad, lines.len() as f32 * row + 4.0);
                fill(&mut draws, 0.0, 0.0, bw, bh, 0x9000_0000);
                for (i, (line, color, id)) in lines.iter().enumerate() {
                    let y = 2.0 + i as f32 * row;
                    if icons && !id.is_empty() {
                        draws.push(GuiDraw::Sprite {
                            sprite: std::borrow::Cow::Owned(format!("mob_effect/{id}")),
                            x: 3,
                            y: y as i32,
                            w: 10,
                            h: 10,
                            region: None,
                            color: 0xFFFF_FFFF,
                            blend: hud_layout::Blend::Textured,
                        });
                    }
                    text(&mut draws, line, 4.0 + pad, y + 1.0, *color);
                }
                (w, h) = (bw, bh);
            }
            "inspect" => {
                let found = self.belumi_inspect_lines();
                empty = found.is_none();
                let lines = found.unwrap_or_else(|| vec![("Nothing in the crosshair".to_owned(), 0x00AA_B3C6)]);
                let tw = lines.iter().map(|l| HudBatch::text_width(&self.gui.atlas, &l.0)).fold(0.0, f32::max);
                let (bw, bh) = (tw + 8.0, lines.len() as f32 * 10.0 + 4.0);
                fill(&mut draws, 0.0, 0.0, bw, bh, 0x9000_0000);
                for (i, (line, color)) in lines.iter().enumerate() {
                    text(&mut draws, line, 4.0, 3.0 + i as f32 * 10.0, *color);
                }
                (w, h) = (bw, bh);
            }
            "profiler" => {
                let lines = self.belumi_profiler_lines();
                let tw = lines.iter().map(|l| HudBatch::text_width(&self.gui.atlas, &l.0)).fold(0.0, f32::max);
                let (bw, bh) = (tw + 8.0, lines.len() as f32 * 10.0 + 4.0);
                fill(&mut draws, 0.0, 0.0, bw, bh, 0x9000_0000);
                for (i, (line, color)) in lines.iter().enumerate() {
                    text(&mut draws, line, 4.0, 3.0 + i as f32 * 10.0, *color);
                }
                (w, h, empty) = (bw, bh, false);
            }
            "armor" => {
                let slots: Vec<Option<ItemStack>> = vec![
                    self.armor[0].clone(),
                    self.armor[1].clone(),
                    self.armor[2].clone(),
                    self.armor[3].clone(),
                    self.inventory.get(self.selected).cloned().flatten(),
                    self.offhand.clone(),
                ];
                let numbers = p.flag("numbers", true);
                let warn = self.modules_ref().is_some_and(|m| m.misc.low_durability_warning);
                let blink = (self.started.elapsed().as_millis() / 400) % 2 == 0;
                let mut rows = 0;
                let mut width: f32 = 20.0;
                let mut lines: Vec<(f32, Option<String>, u32, ItemStack)> = Vec::new();
                for stack in slots.iter().flatten() {
                    let max = self.items().get(&stack.item).map_or(0, |i| i.max_damage as i32);
                    let (label, color) = if max > 0 {
                        let left = (max - stack.damage as i32).max(0);
                        let ratio = left as f32 / max as f32;
                        let color = if ratio < 0.1 && warn && blink {
                            0x00FF_3B3B
                        } else if ratio < 0.1 && warn {
                            0x00FF_9A9A
                        } else if ratio < 0.25 {
                            0x00FF_9A2E
                        } else if ratio < 0.5 {
                            0x00FF_E14D
                        } else {
                            0x004D_FF6A
                        };
                        (numbers.then(|| format!("{left}")), color)
                    } else if stack.count > 1 {
                        (Some(format!("x{}", stack.count)), 0x00FF_FFFF)
                    } else {
                        (None, 0x00FF_FFFF)
                    };
                    if let Some(l) = &label {
                        width = width.max(22.0 + HudBatch::text_width(&self.gui.atlas, l));
                    }
                    lines.push((rows as f32 * 18.0 + 2.0, label, color, stack.clone()));
                    rows += 1;
                }
                empty = rows == 0;
                let rows_shown = rows.max(1);
                fill(&mut draws, 0.0, 0.0, width + 6.0, rows_shown as f32 * 18.0 + 4.0, 0x9000_0000);
                if empty {
                    text(&mut draws, "No gear", 4.0, 6.0, 0x00AA_B3C6);
                    width = width.max(HudBatch::text_width(&self.gui.atlas, "No gear") + 2.0);
                }
                for (y, label, color, stack) in lines {
                    if let Some(l) = label {
                        text(&mut draws, &l, 22.0, y + 5.0, color);
                    }
                    items.push((stack, 3.0, y));
                }
                (w, h) = (width + 6.0, rows_shown as f32 * 18.0 + 4.0);
            }
            "inventory" => {
                let mut any = false;
                for row in 0..3 {
                    for col in 0..9 {
                        if let Some(Some(stack)) = self.inventory.get(9 + row * 9 + col) {
                            items.push((stack.clone(), 3.0 + col as f32 * 18.0, 3.0 + row as f32 * 18.0));
                            any = true;
                        }
                    }
                }
                fill(&mut draws, 0.0, 0.0, 9.0 * 18.0 + 4.0, 3.0 * 18.0 + 4.0, 0x6000_0000);
                for row in 0..3 {
                    for col in 0..9 {
                        fill(&mut draws, 2.0 + col as f32 * 18.0, 2.0 + row as f32 * 18.0, 17.0, 17.0, 0x30FF_FFFF);
                    }
                }
                (w, h, empty) = (9.0 * 18.0 + 4.0, 3.0 * 18.0 + 4.0, !any);
            }
            "rec" => {
                let (label, live) = match self.belumi_recording_text() {
                    Some(t) => (t, true),
                    None => ("REC 0:00".to_owned(), false),
                };
                let tw = HudBatch::text_width(&self.gui.atlas, &label);
                fill(&mut draws, 0.0, 0.0, tw + 16.0, 14.0, 0x9000_0000);
                let blink = (self.started.elapsed().as_millis() / 500) % 2 == 0;
                fill(&mut draws, 4.0, 4.0, 6.0, 6.0, if live && blink { 0xFFFF_3B3B } else { 0xFF70_2020 });
                text(&mut draws, &label, 13.0, 3.0, 0x00FF_FFFF);
                (w, h, empty) = (tw + 16.0, 14.0, !live);
            }
            "fps" | "ping" | "coords" | "compass" => {
                let (label, color) = match p.id.as_str() {
                    "fps" => {
                        let f = self.fps.round() as i32;
                        (format!("{f} FPS"), if f >= 60 { 0x004D_FF6A } else if f >= 30 { 0x00FF_E14D } else { 0x00FF_6B6B })
                    }
                    "ping" => match self.belumi_own_ping() {
                        Some(ms) => (format!("{ms} ms"), if ms <= 80 { 0x004D_FF6A } else if ms <= 160 { 0x00FF_E14D } else { 0x00FF_6B6B }),
                        None => ("- ms".to_owned(), 0x00AA_B3C6),
                    },
                    "compass" => self.belumi_compass_text(),
                    _ => match self.eye_position() {
                        Some([x, y, z]) => (format!("{:.0} {:.0} {:.0}", x.floor(), (y - 1.62).floor(), z.floor()), 0x00FF_FFFF),
                        None => ("- - -".to_owned(), 0x00AA_B3C6),
                    },
                };
                let tw = HudBatch::text_width(&self.gui.atlas, &label);
                fill(&mut draws, 0.0, 0.0, tw + 8.0, 14.0, 0x9000_0000);
                text(&mut draws, &label, 4.0, 3.0, color);
                (w, h, empty) = (tw + 8.0, 14.0, false);
            }
            _ => {
                let seconds = p.flag("seconds", false);
                let now = chrono::Local::now();
                let secs = {
                    use chrono::Timelike;
                    now.num_seconds_from_midnight()
                };
                let label = modules::clock_text(secs, seconds);
                let tw = HudBatch::text_width(&self.gui.atlas, &label);
                fill(&mut draws, 0.0, 0.0, tw + 8.0, 14.0, 0x9000_0000);
                text(&mut draws, &label, 4.0, 3.0, 0x00FF_FFFF);
                (w, h, empty) = (tw + 8.0, 14.0, false);
            }
        }
        Built { w, h, draws, items, empty }
    }

    fn module_scale(p: &Placed) -> f32 {
        if matches!(p.id.as_str(), "armor" | "inventory") { 1.0 } else { p.scale }
    }

    /// Draws one built module at its place. `outline`: a frame around it (the builder's).
    fn paint_module(&self, batch: &mut HudBatch, built: &Built, at: (f32, f32), scale: f32, bg: u8) {
        let mut all = vec![GuiDraw::Push(vec![PoseOp::Translate(at.0, at.1), PoseOp::Scale(scale, scale)])];
        all.extend(built.draws.iter().cloned().map(|d| match d {
            GuiDraw::Fill { x0, y0, x1, y1, color } if bg < 100 => {
                let a = ((color >> 24) as f32 * f32::from(bg) / 100.0).round() as u32;
                GuiDraw::Fill { x0, y0, x1, y1, color: (a << 24) | (color & 0x00FF_FFFF) }
            }
            other => other,
        }));
        all.push(GuiDraw::Pop);
        hud_layout::paint(batch, &self.gui.atlas, &all);
        for (stack, x, y) in &built.items {
            self.draw_item(batch, stack, (at.0 + x * scale).round(), (at.1 + y * scale).round());
        }
    }

    /// The HUD modules in play: each enabled one at its place.
    pub(super) fn belumi_modules_draw(&mut self, batch: &mut HudBatch) {
        if self.belumi_client.modules.is_none() {
            let _ = self.modules();
        }
        let now = std::time::Instant::now();
        let last = self.belumi_client.fade_at.replace(now);
        self.belumi_client.fade_dt = last.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.1));
        self.belumi_replay_hud(batch, true);
        if self.screen == Screen::BelumiClient || self.ui.hide_gui {
            return;
        }
        let list: Vec<Placed> = self.modules_ref().map(|m| m.hud.iter().filter(|p| p.enabled).cloned().collect()).unwrap_or_default();
        for p in list {
            let built = self.build_module(&p);
            if built.empty {
                continue;
            }
            let scale = Self::module_scale(&p);
            let size = (built.w * scale, built.h * scale);
            let at = modules::position(&p, (batch.width, batch.height), size);
            self.paint_module(batch, &built, (at.0.round(), at.1.round()), scale, p.bg);
        }
    }

    /// `/find TEXT` (or `/chatsearch`): the last chat lines that contain TEXT.
    pub(super) fn belumi_client_command(&mut self, command: &str, ran: &mut bool) {
        let mut parts = command.trim().splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("").to_ascii_lowercase();
        if !*ran && head == "joinlink" {
            *ran = true;
            self.belumi_copy_join_link();
            return;
        }
        if *ran || !matches!(head.as_str(), "find" | "chatsearch") {
            return;
        }
        *ran = true;
        let needle = parts.next().unwrap_or("").trim().to_owned();
        if needle.is_empty() {
            self.native_chat("Usage: /find TEXT  searches the chat history.");
            return;
        }
        let found = self.ui.chat.search(&needle, 12);
        if found.is_empty() {
            self.native_chat(&format!("Nothing in the chat history matches {needle}."));
            return;
        }
        self.native_chat(&format!("{} lines match {needle} (newest first):", found.len()));
        for line in found {
            self.native_chat(&format!("> {line}"));
        }
    }

    /// `belumi://join/ADDRESS` for the server the player is on, put on the clipboard.
    pub(super) fn belumi_copy_join_link(&mut self) {
        use horizon_client::clipboard::Clipboard;
        let address = self.belumi_client_server().trim().to_owned();
        if self.remote.is_none() || address.is_empty() {
            self.native_chat("Join a server first: the link is for the server you are on.");
            return;
        }
        let link = format!("belumi://join/{address}");
        self.clipboard.set(&link);
        self.native_chat(&format!("Copied {link}. A friend with Belumi opens it to join you."));
    }

    // ---- opening and closing ------------------------------------------------------------

    pub(super) fn belumi_client_key(&mut self, key: &str, pressed: bool) {
        self.freelook_key(key, pressed);
        self.belumi_replay_key(key, pressed);
        self.belumi_photo_key(key, pressed);
        self.belumi_ping_key(key, pressed);
        self.belumi_placement_key(key, pressed);
        self.belumi_gamma_key(key, pressed);
        let menu_key = self.modules().menu_key.clone();
        if pressed && key == menu_key && self.native_playing() {
            self.client_open();
        }
    }

    pub(super) fn client_open(&mut self) {
        let _ = self.modules();
        self.belumi_client.page = if self.belumi_replay_active() { Page::Replay } else { Page::Builder };
        self.belumi_client.selected = None;
        self.belumi_client.drag = None;
        self.belumi_client.mouse_down = false;
        self.screen = Screen::BelumiClient;
        self.keys.clear();
        self.left = false;
        self.right = false;
        self.capture(false);
    }

    /// A plugin asked for its screen (or the list of them with none): the client screen opens on the Plugin page.
    #[cfg_attr(not(feature = "belumi-plugins"), allow(dead_code))]
    /// Opens the client screen on a page.
    pub(super) fn client_open_page(&mut self, page: Page) {
        if !self.native_playing() {
            return;
        }
        if self.screen != Screen::BelumiClient {
            self.client_open();
        }
        self.belumi_client.page = page;
    }

    #[cfg_attr(not(feature = "belumi-plugins"), allow(dead_code))]
    pub(super) fn client_open_plugin(&mut self, plugin: Option<String>) {
        if !self.native_playing() {
            return;
        }
        if self.screen != Screen::BelumiClient {
            self.client_open();
        }
        self.belumi_client.page = Page::Plugin;
        self.belumi_client.plugin_page = plugin;
    }

    #[cfg_attr(not(feature = "belumi-plugins"), allow(dead_code))]
    pub(super) fn client_close_plugin(&mut self) {
        if self.belumi_client.page == Page::Plugin {
            self.belumi_client_close_screen();
        }
    }

    pub(super) fn belumi_client_close_screen(&mut self) {
        if self.screen == Screen::BelumiClient {
            self.modules_save_now();
            self.belumi_client.drag = None;
            self.screen = Screen::Playing;
            self.capture(true);
        }
    }

    // ---- input -----------------------------------------------------------------------------

    pub(super) fn belumi_client_mouse(&mut self, pressed: bool) {
        self.belumi_client.mouse_down = pressed;
        if !pressed {
            let (w, h) = self.gui_size();
            self.drag_update(w, h);
            if self.belumi_client.drag.take().is_some() {
                self.belumi_client.guides.clear();
                self.modules_changed();
            }
            return;
        }
        let cursor = self.cursor;
        let hit = self
            .belumi_client
            .widgets
            .iter()
            .rev()
            .find(|(r, _)| horizon_client::ui::hovered(cursor, r[0], r[1], r[2], r[3]))
            .map(|(_, a)| a.clone());
        if let Some(act) = hit {
            self.audio.play_button_click();
            self.client_do(act);
            return;
        }
        if self.belumi_client.page == Page::Builder {
            self.builder_press();
        } else if let Some((n, p)) = self.belumi_client.boxes.take() {
            let (mut n, mut p) = (n, p);
            n.focused = false;
            p.focused = false;
            self.belumi_client.boxes = Some((n, p));
        }
    }

    /// A press on the builder's canvas: pick the module under the pointer and start dragging it.
    fn builder_press(&mut self) {
        let (w, h) = self.gui_size();
        let cursor = self.cursor;
        let list: Vec<Placed> = self.modules_ref().map(|m| m.hud.iter().filter(|p| p.enabled).cloned().collect()).unwrap_or_default();
        let mut picked = None;
        for p in list.iter().rev() {
            let built = self.build_module(p);
            let scale = Self::module_scale(p);
            let size = (built.w * scale, built.h * scale);
            let at = modules::position(p, (w, h), size);
            if horizon_client::ui::hovered(cursor, at.0 - 2.0, at.1 - 2.0, size.0 + 4.0, size.1 + 4.0) {
                picked = Some((p.id.clone(), [cursor.0 - at.0, cursor.1 - at.1]));
                break;
            }
        }
        match picked {
            Some((id, grab)) => {
                self.undo_push();
                self.belumi_client.selected = Some(id.clone());
                self.belumi_client.drag = Some((id, grab));
            }
            None => self.belumi_client.selected = None,
        }
    }

    pub(super) fn belumi_client_scroll(&mut self, steps: f64) {
        if self.belumi_client.page == Page::Replays {
            self.replay_scroll(steps);
            return;
        }
        if self.belumi_client.page == Page::Modules {
            let row = &mut self.belumi_client.modules_row;
            *row = if steps > 0.0 { row.saturating_sub(1) } else { *row + 1 };
            return;
        }
        if self.belumi_client.page != Page::Builder {
            return;
        }
        if let Some(id) = self.belumi_client.selected.clone()
            && let Some(m) = self.modules().hud_mut(&id)
            && !matches!(id.as_str(), "armor" | "inventory")
        {
            m.scale = (m.scale + 0.1 * steps.signum() as f32).clamp(0.5, 3.0);
            m.scale = (m.scale * 10.0).round() / 10.0;
            self.modules_changed();
        }
    }

    pub(super) fn belumi_client_screen_key(&mut self, event: &winit::event::KeyEvent) {
        use winit::{event::ElementState, keyboard::{KeyCode, PhysicalKey}};
        if event.state != ElementState::Pressed {
            return;
        }
        let PhysicalKey::Code(key) = event.physical_key else {
            return;
        };
        let focused = self.belumi_client.boxes.as_ref().is_some_and(|(n, p)| n.focused || p.focused);
        match key {
            KeyCode::Escape => {
                if self.belumi_client.page == Page::Builder {
                    self.belumi_client_close_screen();
                } else {
                    self.chat_boxes_commit();
                    self.belumi_client.page = if self.belumi_client.page == Page::Builder { Page::Builder } else { Page::Modules };
                }
                return;
            }
            KeyCode::Tab if focused => {
                if let Some((n, p)) = self.belumi_client.boxes.as_mut() {
                    let to_pass = n.focused;
                    n.focused = !to_pass;
                    p.focused = to_pass;
                }
                return;
            }
            KeyCode::Enter | KeyCode::NumpadEnter if focused => {
                self.chat_boxes_commit();
                if let Some((n, p)) = self.belumi_client.boxes.as_mut() {
                    n.focused = false;
                    p.focused = false;
                }
                return;
            }
            _ => {}
        }
        if !focused {
            let menu = self.modules().menu_key.clone();
            if horizon_client::keybinds::key_name(key).is_some_and(|k| k == menu) {
                self.belumi_client_close_screen();
            } else if key == KeyCode::KeyZ && self.has_control_down() && self.belumi_client.page == Page::Builder {
                self.undo_pop();
            } else if self.belumi_client.page == Page::Builder
                && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown)
                && let Some(id) = self.belumi_client.selected.clone()
            {
                let (w, h) = self.gui_size();
                let p = self.modules_ref().and_then(|m| m.hud_ref(&id)).cloned();
                if let Some(p) = p {
                    self.undo_push();
                    let built = self.build_module(&p);
                    let scale = Self::module_scale(&p);
                    let size = (built.w * scale, built.h * scale);
                    let mut at = modules::position(&p, (w, h), size);
                    let step = if self.has_shift_down() { 10.0 } else { 1.0 };
                    match key {
                        KeyCode::ArrowLeft => at.0 -= step,
                        KeyCode::ArrowRight => at.0 += step,
                        KeyCode::ArrowUp => at.1 -= step,
                        _ => at.1 += step,
                    }
                    let (nx, ny) = modules::fractions(at, (w, h), size);
                    if let Some(m) = self.modules().hud_mut(&id) {
                        m.nx = nx;
                        m.ny = ny;
                    }
                    self.modules_changed();
                }
            }
            return;
        }
        let shortcut = self.edit_shortcut(event);
        let (ctrl, shift) = (self.has_control_down(), self.has_shift_down());
        let typed = event.text.as_deref().map(str::to_owned);
        let Some((mut n, mut p)) = self.belumi_client.boxes.take() else {
            return;
        };
        {
            let target = if n.focused { &mut n } else { &mut p };
            if let Some(shortcut) = shortcut {
                target.shortcut(shortcut, &mut self.clipboard);
            } else if !target.key(key, ctrl, shift)
                && let Some(text) = typed
            {
                target.insert_text(&text);
            }
        }
        self.belumi_client.boxes = Some((n, p));
    }

    fn chat_boxes_commit(&mut self) {
        let Some((n, p)) = self.belumi_client.boxes.as_ref() else {
            return;
        };
        let words = |s: &str| s.split(',').map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()).collect::<Vec<_>>();
        let (hide, highlight) = (words(&n.value), words(&p.value));
        let chat = &mut self.modules().chat;
        if chat.hide != hide || chat.highlight != highlight {
            chat.hide = hide;
            chat.highlight = highlight;
            self.modules_changed();
        }
    }

    fn client_do(&mut self, act: Act) {
        if matches!(act, Act::Toggle(_) | Act::Starter | Act::ScaleDown | Act::ScaleUp | Act::BgDown | Act::BgUp | Act::Flag(..)) {
            self.undo_push();
        }
        match act {
            Act::Page(page) => {
                self.chat_boxes_commit();
                self.belumi_client.page = page;
                self.belumi_client.saved = None;
                self.belumi_client.plugin_page = None;
                if page == Page::Replays {
                    self.replay_page_open();
                }
                if page == Page::Chat {
                    let mk = |text: &str| {
                        let mut b = horizon_client::ui::EditBox::new(0.0, 0.0, 0.0, 0.0);
                        b.max_length = 200;
                        b.focused = false;
                        b.set_value(text);
                        b
                    };
                    let chat = self.modules().chat.clone();
                    self.belumi_client.boxes = Some((mk(&chat.hide.join(", ")), mk(&chat.highlight.join(", "))));
                }
            }
            Act::Close => self.belumi_client_close_screen(),
            Act::Replay(a) => self.replay_do(a),
            Act::Starter => {
                self.modules().starter();
                self.modules_changed();
            }
            Act::Toggle(id) => {
                if let Some(m) = self.modules().hud_mut(&id) {
                    m.enabled = !m.enabled;
                }
                self.belumi_client.selected = Some(id);
                self.modules_changed();
            }
            Act::Select(id) => self.belumi_client.selected = Some(id),
            Act::ScaleDown | Act::ScaleUp => {
                let up = act == Act::ScaleUp;
                if let Some(id) = self.belumi_client.selected.clone()
                    && let Some(m) = self.modules().hud_mut(&id)
                {
                    m.scale = ((m.scale + if up { 0.1 } else { -0.1 }).clamp(0.5, 3.0) * 10.0).round() / 10.0;
                    self.modules_changed();
                }
            }
            Act::Channel(ch, step) => {
                let c = &mut self.modules().crosshair;
                let shift = match ch {
                    0 => 16,
                    1 => 8,
                    _ => 0,
                };
                let v = ((c.color >> shift) & 0xFF) as i32;
                let v = (v + step).clamp(0, 255) as u32;
                c.color = (c.color & !(0xFF << shift) & 0x00FF_FFFF) | (v << shift);
                self.modules_changed();
            }
            Act::DiscordFlag(name) => {
                let d = &mut self.modules().discord;
                match name {
                    "server" => d.show_server = !d.show_server,
                    "world" => d.show_world = !d.show_world,
                    "dimension" => d.show_dimension = !d.show_dimension,
                    "mode" => d.show_mode = !d.show_mode,
                    "time" => d.show_time = !d.show_time,
                    _ => d.show_players = !d.show_players,
                }
                self.modules_changed();
            }
            Act::Preset(i) => {
                self.modules().crosshair.apply_preset(i);
                self.modules_changed();
            }
            Act::BgDown | Act::BgUp => {
                let up = act == Act::BgUp;
                if let Some(id) = self.belumi_client.selected.clone()
                    && let Some(m) = self.modules().hud_mut(&id)
                {
                    m.bg = (i16::from(m.bg) + if up { 10 } else { -10 }).clamp(0, 100) as u8;
                    self.modules_changed();
                }
            }
            Act::Flag(id, name) => {
                if let Some(m) = self.modules().hud_mut(&id) {
                    let default = !matches!(name.as_str(), "seconds");
                    let now = m.flag(&name, default);
                    m.opt.insert(name, !now);
                    self.modules_changed();
                }
            }
            Act::Sprint => {
                self.settings.toggle_sprint = !self.settings.toggle_sprint;
                self.settings.save(&Game::options_path());
            }
            Act::Sneak => {
                self.settings.toggle_crouch = !self.settings.toggle_crouch;
                self.settings.save(&Game::options_path());
            }
            Act::Freelook => {
                let m = self.modules();
                m.misc.freelook = !m.misc.freelook;
                self.modules_changed();
            }
            Act::Tooltips => {
                let m = self.modules();
                m.misc.tooltips = !m.misc.tooltips;
                self.modules_changed();
            }
            Act::PluginOpen(plugin) => self.belumi_client.plugin_page = Some(plugin),
            Act::PluginPress(plugin, id, value) => self.belumi_plugin_press(&plugin, &id, value),
            Act::PickAccount(i) => self.belumi_switch_account(i),
            Act::BlueprintLayer(_) => {
                self.blueprint_act(&act);
            }
            Act::SoundLevel(..) | Act::SoundsReset => {
                self.belumi_sounds_act(&act);
            }
            Act::RiptideFix => {
                let on = {
                    let m = self.modules();
                    m.misc.riptide_fix = !m.misc.riptide_fix;
                    m.misc.riptide_fix
                };
                self.modules_changed();
                self.belumi_confirm(if on { "Riptide fix ON: a server that checks speed may notice" } else { "Riptide fix OFF" });
            }
            Act::ConfirmCycle => {
                let m = self.modules();
                m.misc.confirm = match m.misc.confirm.as_str() {
                    "hud" => "chat",
                    "chat" => "off",
                    _ => "hud",
                }
                .to_owned();
                self.modules_changed();
            }
            Act::GammaCycle => self.belumi_gamma_cycle(),
            Act::LightOn => {
                let m = self.modules();
                m.misc.light_overlay = !m.misc.light_overlay;
                self.modules_changed();
            }
            Act::AccurateOn => self.belumi_placement_toggle(),
            Act::AccurateBreak => {
                let m = self.modules();
                m.misc.accurate_break = !m.misc.accurate_break;
                self.modules_changed();
            }
            Act::PingsOn => {
                let m = self.modules();
                m.misc.pings = !m.misc.pings;
                self.modules_changed();
            }
            Act::RedstoneNumbers | Act::TickCmd(_) => {
                self.redstone_act(&act);
            }
            Act::HistoryOn => {
                let m = self.modules();
                m.misc.world_history = !m.misc.world_history;
                self.modules_changed();
            }
            Act::SoundOn => {
                let m = self.modules();
                m.misc.sound_physics = !m.misc.sound_physics;
                self.modules_changed();
            }
            Act::CombatOn => {
                let m = self.modules();
                m.misc.combat = !m.misc.combat;
                self.modules_changed();
            }
            Act::CombatClear => {
                self.combat_act(&act);
            }
            Act::CopyJoin => self.belumi_copy_join_link(),
            Act::ChestsOn => {
                let m = self.modules();
                m.misc.chest_memory = !m.misc.chest_memory;
                self.modules_changed();
            }
            Act::ChestMark(_) => {
                self.belumi_chests_act(&act);
            }
            Act::ChangesOn => {
                let m = self.modules();
                m.misc.changes = !m.misc.changes;
                self.modules_changed();
            }
            Act::ChangeMark(_) | Act::ChangesClear => {
                self.belumi_changes_act(&act);
            }
            Act::Trail => {
                let m = self.modules();
                m.misc.trail = !m.misc.trail;
                self.modules_changed();
            }
            Act::DistantChunks => {
                let m = self.modules();
                m.misc.distant_chunks = !m.misc.distant_chunks;
                self.modules_changed();
            }
            Act::ContainerPreview => {
                let m = self.modules();
                m.misc.container_preview = !m.misc.container_preview;
                self.modules_changed();
            }
            Act::Reconnect => {
                self.settings.extras.auto_reconnect = !self.settings.extras.auto_reconnect;
                self.settings.save(&Game::options_path());
            }
            Act::Extra(name) => {
                let e = &mut self.settings.extras;
                match name {
                    "fullbright" => e.fullbright = !e.fullbright,
                    "mouse_tweaks" => e.mouse_tweaks = !e.mouse_tweaks,
                    "sort" => e.sort = !e.sort,
                    "discord" => e.discord_presence = !e.discord_presence,
                    _ => e.own_name_tag = !e.own_name_tag,
                }
                self.settings.save(&Game::options_path());
            }
            Act::Warning => {
                let m = self.modules();
                m.misc.low_durability_warning = !m.misc.low_durability_warning;
                self.modules_changed();
            }
            Act::CrosshairOn => {
                let c = &mut self.modules().crosshair;
                c.enabled = !c.enabled;
                self.modules_changed();
            }
            Act::Style(step) => {
                let c = &mut self.modules().crosshair;
                let n = modules::CROSSHAIR_STYLES.len() as i32;
                c.style = (i32::from(c.style) + step).rem_euclid(n) as u8;
                self.modules_changed();
            }
            Act::Colour(rgb) => {
                self.modules().crosshair.color = rgb;
                self.modules_changed();
            }
            Act::Step(name, step) => {
                let c = &mut self.modules().crosshair;
                let bump = |v: u8, lo: i32, hi: i32, k: i32| (i32::from(v) + k).clamp(lo, hi) as u8;
                match name {
                    "opacity" => c.opacity = bump(c.opacity, 40, 255, step * 15),
                    "size" => c.size = bump(c.size, 1, 30, step),
                    "thickness" => c.thickness = bump(c.thickness, 1, 8, step),
                    _ => c.gap = bump(c.gap, 0, 20, step),
                }
                self.modules_changed();
            }
            Act::Switch(name) => {
                let c = &mut self.modules().crosshair;
                match name {
                    "outline" => c.outline = !c.outline,
                    "dot" => c.dot = !c.dot,
                    _ => c.dynamic = !c.dynamic,
                }
                self.modules_changed();
            }
            Act::ChatFlag(name) => {
                let c = &mut self.modules().chat;
                match name {
                    "timestamps" => c.timestamps = !c.timestamps,
                    "seconds" => c.seconds = !c.seconds,
                    "merge" => c.merge_spam = !c.merge_spam,
                    _ => c.ping_sound = !c.ping_sound,
                }
                self.modules_changed();
            }
            Act::ChatHistory(step) => {
                let c = &mut self.modules().chat;
                c.history = (c.history as i32 + step * 100).clamp(100, 2000) as u32;
                self.modules_changed();
            }
            Act::FocusBox(i) => {
                if let Some((n, p)) = self.belumi_client.boxes.as_mut() {
                    n.focused = i == 0;
                    p.focused = i == 1;
                }
            }
        }
    }

    // ---- drawing the screen ----------------------------------------------------------------

    pub(super) fn reg(&mut self, rect: [f32; 4], act: Act) {
        self.belumi_client.widgets.push((rect, act));
    }

    pub(super) fn hovered(&self, r: [f32; 4]) -> bool {
        horizon_client::ui::hovered(self.cursor, r[0], r[1], r[2], r[3])
    }

    /// A button: `state` is `Some(on)` for a switch. Registered for clicks.
    pub(super) fn client_button(&mut self, batch: &mut HudBatch, r: [f32; 4], label: &str, state: Option<bool>, act: Act) {
        let over = self.hovered(r);
        let (base, hover) = match state {
            Some(true) => (ON, ON_HOVER),
            _ => (BUTTON, BUTTON_HOVER),
        };
        self.native_fill(batch, r, if over { hover } else { base });
        let tw = self.native_text_width(label);
        let color = if state == Some(false) { OFF_TEXT } else { 0x00FF_FFFF };
        self.native_text(batch, label, [r[0] + (r[2] - tw) / 2.0, r[1] + ((r[3] - 8.0) / 2.0).floor()], color, true);
        self.reg(r, act);
    }

    pub(super) fn client_accent_button(&mut self, batch: &mut HudBatch, r: [f32; 4], label: &str, act: Act) {
        let over = self.hovered(r);
        self.native_fill(batch, r, if over { ACCENT_HOVER } else { ACCENT });
        let tw = self.native_text_width(label);
        self.native_text(batch, label, [r[0] + (r[2] - tw) / 2.0, r[1] + ((r[3] - 8.0) / 2.0).floor()], 0x00FF_FFFF, true);
        self.reg(r, act);
    }

    pub(super) fn panel(&self, batch: &mut HudBatch, r: [f32; 4], title: &str) {
        self.native_fill(batch, [r[0] - 1.0, r[1] - 1.0, r[2] + 2.0, r[3] + 2.0], EDGE);
        self.native_fill(batch, r, BG);
        self.native_fill(batch, [r[0], r[1], r[2], 20.0], HEAD);
        self.native_fill(batch, [r[0], r[1], 3.0, 20.0], ACCENT);
        self.native_fill(batch, [r[0], r[1] + 20.0, r[2], 1.0], EDGE);
        self.native_text(batch, title, [r[0] + 10.0, r[1] + 6.0], 0x00FF_FFFF, true);
    }

    pub(super) fn belumi_client_draw_screen(&mut self, batch: &mut HudBatch) {
        self.belumi_client.widgets.clear();
        let (w, h) = (batch.width, batch.height);
        match self.belumi_client.page {
            Page::Builder => self.draw_builder(batch, w, h),
            Page::Modules => self.draw_modules_page(batch, w, h),
            Page::Crosshair => self.draw_crosshair_page(batch, w, h),
            Page::Chat => self.draw_chat_page(batch, w, h),
            Page::Discord => self.draw_discord_page(batch, w, h),
            Page::Accounts => self.draw_accounts_page(batch, w, h),
            Page::Plugin => self.draw_plugin_page(batch, w, h),
            Page::Changes => self.draw_changes_page(batch, w, h),
            Page::Chests => self.draw_chests_page(batch, w, h),
            Page::Combat => self.draw_combat_page(batch, w, h),
            Page::Redstone => self.draw_redstone_page(batch, w, h),
            Page::Blueprint => self.draw_blueprint_page(batch, w, h),
            Page::Sounds => self.draw_sounds_page(batch, w, h),
            Page::Replays => self.draw_replays_page(batch, w, h),
            Page::Replay => self.draw_replay_controls_page(batch, w, h),
        }
        self.belumi_replay_hud(batch, false);
    }

    /// While a module is dragged: it follows the pointer, snapped to the edges and its neighbours.
    fn drag_update(&mut self, w: f32, h: f32) {
        let Some((id, grab)) = self.belumi_client.drag.clone() else {
            return;
        };
        let list: Vec<Placed> = self.modules_ref().map(|m| m.hud.clone()).unwrap_or_default();
        let Some(p) = list.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        let built = self.build_module(&p);
        let scale = Self::module_scale(&p);
        let size = (built.w * scale, built.h * scale);
        let mut others = Vec::new();
        for o in list.iter().filter(|o| o.enabled && o.id != id) {
            let b = self.build_module(o);
            let s = Self::module_scale(o);
            let sz = (b.w * s, b.h * s);
            let at = modules::position(o, (w, h), sz);
            others.push([at.0, at.1, sz.0, sz.1]);
        }
        let want = [self.cursor.0 - grab[0], self.cursor.1 - grab[1], size.0, size.1];
        let reach = if self.has_shift_down() { 0.0 } else { SNAP_REACH };
        let (at, guides) = modules::snap(want, (w, h), &others, reach);
        let (nx, ny) = modules::fractions((at[0], at[1]), (w, h), size);
        self.belumi_client.guides = guides;
        if let Some(m) = self.modules().hud_mut(&id) {
            m.nx = nx;
            m.ny = ny;
        }
        self.belumi_client.dirty = true;
    }

    fn draw_builder(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x5000_0000);
        if self.belumi_client.mouse_down {
            self.drag_update(w, h);
        } else if self.belumi_client.drag.take().is_some() {
            self.belumi_client.guides.clear();
        }
        let modules_list: Vec<Placed> = self.modules_ref().map(|m| m.hud.clone()).unwrap_or_default();
        for (vertical, at) in self.belumi_client.guides.clone() {
            if vertical {
                self.native_fill(batch, [at - 0.5, 0.0, 1.0, h], 0xB0FF_E14D);
            } else {
                self.native_fill(batch, [0.0, at - 0.5, w, 1.0], 0xB0FF_E14D);
            }
        }
        let selected = self.belumi_client.selected.clone();
        for p in modules_list.iter().filter(|p| p.enabled) {
            let built = self.build_module(p);
            let scale = Self::module_scale(p);
            let size = (built.w * scale, built.h * scale);
            let at = modules::position(p, (w, h), size);
            let (ax, ay) = (at.0.round(), at.1.round());
            self.paint_module(batch, &built, (ax, ay), scale, p.bg);
            let chosen = selected.as_deref() == Some(p.id.as_str());
            let edge = if chosen { ACCENT_HOVER } else { 0xA0FF_FFFF };
            for r in [[ax - 1.0, ay - 1.0, size.0 + 2.0, 1.0], [ax - 1.0, ay + size.1, size.0 + 2.0, 1.0], [ax - 1.0, ay, 1.0, size.1], [ax + size.0, ay, 1.0, size.1]] {
                self.native_fill(batch, r, edge);
            }
            let tag = modules::hud_title(&p.id);
            let tw = self.native_text_width(tag);
            let ty = if ay >= 11.0 { ay - 11.0 } else { ay + size.1 + 2.0 };
            let tx = (ax - 1.0).min(w - tw - 7.0).max(0.0);
            self.native_fill(batch, [tx, ty, tw + 6.0, 10.0], if chosen { ACCENT } else { 0xC01A_2030 });
            self.native_text(batch, tag, [tx + 3.0, ty + 1.0], 0x00FF_FFFF, false);
            let _ = built.empty;
        }
        // title and top buttons
        let bar = [w / 2.0 - 150.0, 6.0, 300.0, 22.0];
        self.native_fill(batch, [bar[0] - 1.0, bar[1] - 1.0, bar[2] + 2.0, bar[3] + 2.0], EDGE);
        self.native_fill(batch, bar, HEAD);
        self.native_fill(batch, [bar[0], bar[1], 3.0, bar[3]], ACCENT);
        self.native_text(batch, "HUD Builder", [bar[0] + 10.0, bar[1] + 7.0], 0x00FF_FFFF, true);
        self.client_button(batch, [bar[0] + 90.0, bar[1] + 3.0, 60.0, 16.0], "Starter", None, Act::Starter);
        self.client_accent_button(batch, [bar[0] + 154.0, bar[1] + 3.0, 78.0, 16.0], "Modules", Act::Page(Page::Modules));
        self.client_button(batch, [bar[0] + 236.0, bar[1] + 3.0, 58.0, 16.0], "Done", None, Act::Close);
        // palette
        let ids = modules::HUD_IDS;
        let chip_w = 62.0_f32.min((w - 20.0) / 2.0);
        let per_row = (((w - 12.0) / (chip_w + 4.0)).floor() as usize).clamp(1, ids.len());
        let rows = ids.len().div_ceil(per_row);
        let py = h - 30.0 - (rows - 1) as f32 * 22.0;
        let selected_label = selected.as_deref().map(modules::hud_title);
        let widest = per_row.min(ids.len()) as f32 * (chip_w + 4.0) - 4.0;
        self.native_fill(batch, [(w - widest) / 2.0 - 6.0, py - 6.0, widest + 12.0, 30.0 + (rows - 1) as f32 * 22.0], 0xE012_1620);
        for (i, id) in ids.iter().enumerate() {
            let (row, col) = (i / per_row, i % per_row);
            let in_row = (ids.len() - row * per_row).min(per_row) as f32;
            let x0 = (w - (in_row * (chip_w + 4.0) - 4.0)) / 2.0;
            let on = modules_list.iter().find(|p| p.id == *id).is_some_and(|p| p.enabled);
            self.client_button(batch, [x0 + col as f32 * (chip_w + 4.0), py + row as f32 * 22.0, chip_w, 18.0], modules::hud_short(id), Some(on), Act::Toggle((*id).to_owned()));
        }
        // settings of the chosen module
        if let Some(id) = selected
            && let Some(p) = modules_list.iter().find(|p| p.id == id).cloned()
        {
            let mut row: Vec<(String, Option<bool>, Act, f32)> = Vec::new();
            if !matches!(p.id.as_str(), "armor" | "inventory") {
                row.push(("-".into(), None, Act::ScaleDown, 16.0));
                row.push((format!("{:.0}%", p.scale * 100.0), None, Act::Select(id.clone()), 34.0));
                row.push(("+".into(), None, Act::ScaleUp, 16.0));
            }
            if !matches!(p.id.as_str(), "armor" | "inventory") {
                row.push(("Bg -".into(), None, Act::BgDown, 8.0 + self.native_text_width("Bg -")));
                row.push((format!("{}%", p.bg), None, Act::Select(id.clone()), 30.0));
                row.push(("+".into(), None, Act::BgUp, 16.0));
            }
            let flags: &[(&str, &str, bool)] = match p.id.as_str() {
                "keystrokes" => &[("mouse", "Mouse", true), ("space", "Space", true)],
                "cps" => &[("split", "Left | Right", true)],
                "armor" => &[("numbers", "Numbers", true)],
                "effects" => &[("icons", "Icons", true), ("ambient", "Beacon", true)],
                "clock" => &[("seconds", "Seconds", false)],
                _ => &[],
            };
            for (name, label, default) in flags {
                let on = p.flag(name, *default);
                row.push(((*label).to_owned(), Some(on), Act::Flag(id.clone(), (*name).to_owned()), 8.0 + self.native_text_width(label)));
            }
            row.push(("Remove".into(), None, Act::Toggle(id.clone()), 44.0));
            let total: f32 = row.iter().map(|r| r.3 + 4.0).sum::<f32>() - 4.0;
            let mut x = (w - total) / 2.0;
            let sy = py - 30.0;
            self.native_fill(batch, [x - 6.0, sy - 5.0, total + 12.0, 26.0], 0xE012_1620);
            for (label, state, act, width) in row {
                self.client_button(batch, [x, sy, width, 16.0], &label, state, act);
                x += width + 4.0;
            }
            let _ = selected_label;
        } else {
            let hint = "Drag to move. Wheel: size. Arrows: nudge. Ctrl+Z: undo.";
            let tw = self.native_text_width(hint);
            self.native_text(batch, hint, [(w - tw) / 2.0, py - 16.0], 0x00C8_D0E0, true);
        }
    }

    fn draw_modules_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let pw = 420.0_f32.min(w - 12.0);
        let ph = (h - 12.0).clamp(150.0, 320.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Modules");
        self.client_button(batch, [x + pw - 72.0, y + 3.0, 66.0, 14.0], "HUD Builder", None, Act::Page(Page::Builder));
        let m = self.modules().clone();
        let cards: Vec<(String, String, Option<bool>, Act, Option<Page>)> = vec![
            ("Toggle Sprint".into(), "Sprint without holding".into(), Some(self.settings.toggle_sprint), Act::Sprint, None),
            ("Toggle Sneak".into(), "Sneak without holding".into(), Some(self.settings.toggle_crouch), Act::Sneak, None),
            ("Freelook".into(), format!("Hold {}", horizon_client::keybinds::display_name(&m.misc.freelook_key).plain()), Some(m.misc.freelook), Act::Freelook, None),
            ("Crosshair".into(), "Your own shape and colour".into(), Some(m.crosshair.enabled), Act::CrosshairOn, Some(Page::Crosshair)),
            ("Item tooltips".into(), "Item id and durability numbers".into(), Some(m.misc.tooltips), Act::Tooltips, None),
            ("Shulker preview".into(), "Contents in the tooltip".into(), Some(m.misc.container_preview), Act::ContainerPreview, None),
            ("World history".into(), "Older chunk versions, /rollback".into(), Some(m.misc.world_history), Act::HistoryOn, None),
            ("Sound physics".into(), "Reverb of the room you are in".into(), Some(m.misc.sound_physics), Act::SoundOn, None),
            ("Sound controller".into(), "Turn single sounds down".into(), None, Act::Page(Page::Sounds), Some(Page::Sounds)),
            ("Switch notices".into(), format!("Say what a switch did: {}", m.misc.confirm), Some(m.misc.confirm != "off"), Act::ConfirmCycle, None),
            ("Riptide fix".into(), "Depth Strider keeps Riptide fast".into(), Some(m.misc.riptide_fix), Act::RiptideFix, None),
            ("Gamma boost".into(), format!("Brightness beyond the slider: {} of 15", m.misc.gamma_level), Some(m.misc.gamma_level > 0), Act::GammaCycle, None),
            ("Light overlay".into(), "X where monsters can spawn".into(), Some(m.misc.light_overlay), Act::LightOn, None),
            ("Accurate placement".into(), "Fast blocks (some servers: cheat)".into(), Some(m.misc.accurate_placement), Act::AccurateOn, None),
            ("Accurate creative break".into(), "No wait between breaks".into(), Some(m.misc.accurate_break), Act::AccurateBreak, None),
            ("Pings".into(), "Z marks a spot for friends".into(), Some(m.misc.pings), Act::PingsOn, None),
            ("Blueprint".into(), "Show a .schem / .litematic plan".into(), None, Act::Page(Page::Blueprint), Some(Page::Blueprint)),
            ("Redstone workbench".into(), "Signals, /tick controls".into(), None, Act::Page(Page::Redstone), Some(Page::Redstone)),
            ("Combat log".into(), "Swings, hits, damage, CPS".into(), Some(m.misc.combat), Act::CombatOn, Some(Page::Combat)),
            ("Chest memory".into(), "Find items with /chests".into(), Some(m.misc.chest_memory), Act::ChestsOn, Some(Page::Chests)),
            ("Changes".into(), "What changed since your last visit".into(), Some(m.misc.changes), Act::ChangesOn, Some(Page::Changes)),
            ("Breadcrumb trail".into(), "Dots behind you, for the way back".into(), Some(m.misc.trail), Act::Trail, None),
            ("Distant chunks".into(), "Keep far chunks, cache them".into(), Some(m.misc.distant_chunks), Act::DistantChunks, None),
            ("Break warning".into(), "Flash when gear is low".into(), Some(m.misc.low_durability_warning), Act::Warning, None),
            ("Discord presence".into(), "What your profile shows".into(), Some(self.settings.extras.discord_presence), Act::Extra("discord"), Some(Page::Discord)),
            ("Plugin screens".into(), "Screens made by plugins".into(), None, Act::Page(Page::Plugin), Some(Page::Plugin)),
            ("Switch account".into(), "Another launcher account".into(), None, Act::Page(Page::Accounts), Some(Page::Accounts)),
            ("Chat tools".into(), "Time, filter, sound".into(), None, Act::Page(Page::Chat), Some(Page::Chat)),
            ("Replays and clips".into(), "Record, clip, watch again".into(), Some(m.replay.record), Act::Replay(super::belumi_replay::RAct::Record), Some(Page::Replays)),
            ("Auto-reconnect".into(), "Rejoin a lost server".into(), Some(self.settings.extras.auto_reconnect), Act::Reconnect, None),
            ("Fullbright".into(), "No darkness".into(), Some(self.settings.extras.fullbright), Act::Extra("fullbright"), None),
            ("Mouse tweaks".into(), "Drag and wheel in inventories".into(), Some(self.settings.extras.mouse_tweaks), Act::Extra("mouse_tweaks"), None),
            ("Sort buttons".into(), "Sort chests and the inventory".into(), Some(self.settings.extras.sort), Act::Extra("sort"), None),
            ("Own name tag".into(), "Your name over you in 3rd person".into(), Some(self.settings.extras.own_name_tag), Act::Extra("own_name_tag"), None),
        ];
        let col_w = (pw - 30.0) / 2.0;
        let total_rows = cards.len().div_ceil(2);
        let visible = (((ph - 30.0 - 26.0) / 40.0).floor() as usize).max(1);
        let first = self.belumi_client.modules_row.min(total_rows.saturating_sub(visible));
        self.belumi_client.modules_row = first;
        if total_rows > visible {
            let note = format!("Wheel: rows {}-{} of {}", first + 1, (first + visible).min(total_rows), total_rows);
            self.native_text(batch, &note, [x + 10.0, y + ph - 18.0], 0x008A_93A8, false);
        }
        for (i, (title, sub, state, act, edit)) in cards.into_iter().enumerate() {
            let row = i / 2;
            if row < first || row >= first + visible {
                continue;
            }
            let (cx, cy) = (x + 10.0 + (i % 2) as f32 * (col_w + 10.0), y + 30.0 + (row - first) as f32 * 40.0);
            let card = [cx, cy, col_w, 34.0];
            let over = self.hovered(card);
            self.native_fill(batch, card, if over { 0xFF22_2A3B } else { 0xFF1A_2030 });
            if let Some(on) = state {
                self.native_fill(batch, [cx, cy, 2.0, 34.0], if on { 0xFF55_E07A } else { 0xFF4A_5268 });
            }
            self.native_text(batch, &title, [cx + 8.0, cy + 5.0], 0x00FF_FFFF, true);
            let mut sub = sub;
            if self.native_text_width(&sub) > col_w - 48.0 {
                while self.native_text_width(&format!("{sub}...")) > col_w - 48.0 && sub.len() > 4 {
                    sub.pop();
                }
                sub.push_str("...");
            }
            self.native_text(batch, &sub, [cx + 8.0, cy + 19.0], 0x008A_93A8, false);
            if let Some(page) = edit {
                self.client_button(batch, [cx + col_w - 32.0, cy + 4.0, 28.0, 12.0], "Edit", None, Act::Page(page));
            }
            if let Some(on) = state {
                let label = if on { "ON" } else { "OFF" };
                self.client_button(batch, [cx + col_w - 32.0, cy + 19.0, 28.0, 12.0], label, Some(on), act);
            }
        }
        self.client_accent_button(batch, [x + pw - 66.0, y + ph - 22.0, 56.0, 16.0], "Done", Act::Close);
    }

    fn stepper(&mut self, batch: &mut HudBatch, x: f32, y: f32, label: &str, value: &str, down: Act, up: Act) {
        self.native_text(batch, label, [x, y + 4.0], 0x00C8_D0E0, true);
        let vx = x + 76.0;
        self.client_button(batch, [vx, y, 14.0, 14.0], "-", None, down);
        let vw = 40.0;
        self.native_fill(batch, [vx + 16.0, y, vw, 14.0], 0xFF1A_2030);
        let tw = self.native_text_width(value);
        self.native_text(batch, value, [vx + 16.0 + (vw - tw) / 2.0, y + 3.0], 0x00FF_FFFF, true);
        self.client_button(batch, [vx + 18.0 + vw, y, 14.0, 14.0], "+", None, up);
    }

    fn draw_crosshair_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let pw = 360.0_f32.min(w - 12.0);
        let ph = (h - 16.0).clamp(216.0, 262.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Crosshair");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let c = self.modules().crosshair.clone();
        // preview
        let pv = [x + 10.0, y + 30.0, 110.0, 110.0];
        self.native_fill(batch, pv, 0xFF3A_4A3A);
        for i in 0..11 {
            for j in 0..11 {
                if (i + j) % 2 == 0 {
                    self.native_fill(batch, [pv[0] + i as f32 * 10.0, pv[1] + j as f32 * 10.0, 10.0, 10.0], 0xFF34_4334);
                }
            }
        }
        let (cx, cy) = (pv[0] + 55.0, pv[1] + 55.0);
        for piece in c.pieces(0.0) {
            self.native_fill(batch, [cx + piece.x, cy + piece.y, piece.w, piece.h], piece.color);
        }
        self.client_button(batch, [pv[0], pv[1] + 116.0, 110.0, 16.0], if c.enabled { "Crosshair: ON" } else { "Crosshair: OFF" }, Some(c.enabled), Act::CrosshairOn);
        let cx0 = x + 134.0;
        let mut ry = y + 30.0;
        let style = modules::CROSSHAIR_STYLES[usize::from(c.style)];
        self.stepper(batch, cx0, ry, "Shape", style, Act::Style(-1), Act::Style(1));
        ry += 20.0;
        self.stepper(batch, cx0, ry, "Opacity", &format!("{}%", u32::from(c.opacity) * 100 / 255), Act::Step("opacity", -1), Act::Step("opacity", 1));
        ry += 20.0;
        self.stepper(batch, cx0, ry, "Size", &c.size.to_string(), Act::Step("size", -1), Act::Step("size", 1));
        ry += 20.0;
        self.stepper(batch, cx0, ry, "Thickness", &c.thickness.to_string(), Act::Step("thickness", -1), Act::Step("thickness", 1));
        ry += 20.0;
        self.stepper(batch, cx0, ry, "Gap", &c.gap.to_string(), Act::Step("gap", -1), Act::Step("gap", 1));
        ry += 22.0;
        self.native_text(batch, "Colour", [cx0, ry + 3.0], 0x00C8_D0E0, true);
        for (i, rgb) in modules::CROSSHAIR_COLORS.iter().enumerate() {
            let sx = cx0 + 50.0 + i as f32 * 17.0;
            let r = [sx, ry, 14.0, 14.0];
            let chosen = c.color & 0x00FF_FFFF == *rgb;
            self.native_fill(batch, [sx - 1.0, ry - 1.0, 16.0, 16.0], if chosen { 0xFFFF_FFFF } else if self.hovered(r) { 0xFF8A_93A8 } else { 0xFF2C_3447 });
            self.native_fill(batch, r, 0xFF00_0000 | rgb);
            self.reg(r, Act::Colour(*rgb));
        }
        ry += 22.0;
        let mut px = cx0;
        for (i, name) in modules::CROSSHAIR_PRESETS.iter().enumerate() {
            let bw = self.native_text_width(name) + 8.0;
            self.client_button(batch, [px, ry, bw, 14.0], name, None, Act::Preset(i));
            px += bw + 3.0;
        }
        ry += 20.0;
        for (i, name) in ["R", "G", "B"].into_iter().enumerate() {
            let shift = [16, 8, 0][i];
            let v = (c.color >> shift) & 0xFF;
            let bx = cx0 + i as f32 * 76.0;
            self.client_button(batch, [bx, ry, 12.0, 14.0], "-", None, Act::Channel(i as u8, -16));
            self.native_text(batch, &format!("{name} {v}"), [bx + 16.0, ry + 3.0], 0x00C8_D0E0, true);
            self.client_button(batch, [bx + 54.0, ry, 12.0, 14.0], "+", None, Act::Channel(i as u8, 16));
        }
        ry += 20.0;
        self.client_button(batch, [cx0, ry, 62.0, 14.0], "Outline", Some(c.outline), Act::Switch("outline"));
        self.client_button(batch, [cx0 + 66.0, ry, 50.0, 14.0], "Dot", Some(c.dot), Act::Switch("dot"));
        self.client_button(batch, [cx0 + 120.0, ry, 62.0, 14.0], "Dynamic", Some(c.dynamic), Act::Switch("dynamic"));
    }

    fn draw_discord_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let pw = 360.0_f32.min(w - 12.0);
        let ph = (h - 16.0).clamp(212.0, 252.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Discord presence");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let d = self.modules().discord.clone();
        let on = self.settings.extras.discord_presence;
        let half = (pw - 30.0) / 2.0;
        let mut ry = y + 30.0;
        self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], if on { "Rich Presence: ON" } else { "Rich Presence: OFF" }, Some(on), Act::Extra("discord"));
        ry += 26.0;
        let rows: [[(&str, bool, &'static str); 2]; 3] = [
            [("Server address", d.show_server, "server"), ("World name", d.show_world, "world")],
            [("Dimension", d.show_dimension, "dimension"), ("Game mode icon", d.show_mode, "mode")],
            [("Time played", d.show_time, "time"), ("Player count", d.show_players, "players")],
        ];
        for row in rows {
            for (i, (label, state, key)) in row.into_iter().enumerate() {
                self.client_button(batch, [x + 10.0 + i as f32 * (half + 10.0), ry, half, 16.0], label, Some(state), Act::DiscordFlag(key));
            }
            ry += 22.0;
        }
        ry += 4.0;
        self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], "Copy join link (belumi://join/...)", None, Act::CopyJoin);
        ry += 22.0;
        self.native_text(batch, "Address and world name stay hidden until switched on.", [x + 10.0, ry], 0x008A_93A8, false);
        ry += 12.0;
        self.native_text(batch, "Friends see this on your Discord profile while you play.", [x + 10.0, ry], 0x008A_93A8, false);
    }

    fn draw_chat_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let pw = 360.0_f32.min(w - 12.0);
        let ph = (h - 16.0).clamp(170.0, 230.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Chat tools");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let c = self.modules().chat.clone();
        let half = (pw - 30.0) / 2.0;
        let mut ry = y + 30.0;
        self.client_button(batch, [x + 10.0, ry, half, 16.0], "Timestamps", Some(c.timestamps), Act::ChatFlag("timestamps"));
        self.client_button(batch, [x + 20.0 + half, ry, half, 16.0], "with seconds", Some(c.seconds), Act::ChatFlag("seconds"));
        ry += 22.0;
        self.client_button(batch, [x + 10.0, ry, half, 16.0], "Merge repeated lines", Some(c.merge_spam), Act::ChatFlag("merge"));
        self.client_button(batch, [x + 20.0 + half, ry, half, 16.0], "Sound on your name", Some(c.ping_sound), Act::ChatFlag("ping"));
        ry += 24.0;
        self.stepper(batch, x + 10.0, ry, "History", &format!("{}", c.history), Act::ChatHistory(-1), Act::ChatHistory(1));
        self.native_text(batch, "lines kept (vanilla: 100)", [x + 10.0 + 76.0 + 92.0, ry + 4.0], 0x008A_93A8, false);
        ry += 26.0;
        self.native_text(batch, "Hide lines with these words (comma separated)", [x + 10.0, ry], GOLD, true);
        let hide_box = [x + 10.0, ry + 11.0, pw - 20.0, 16.0];
        ry += 34.0;
        self.native_text(batch, "Highlight lines with these words", [x + 10.0, ry], GOLD, true);
        let high_box = [x + 10.0, ry + 11.0, pw - 20.0, 16.0];
        if let Some((mut n, mut p)) = self.belumi_client.boxes.take() {
            [n.x, n.y, n.width, n.height] = hide_box;
            [p.x, p.y, p.width, p.height] = high_box;
            n.draw(batch, &self.gui.atlas, None);
            p.draw(batch, &self.gui.atlas, None);
            self.belumi_client.boxes = Some((n, p));
            self.reg(hide_box, Act::FocusBox(0));
            self.reg(high_box, Act::FocusBox(1));
        }
        self.native_text(batch, "Search the chat history with the command /find TEXT.", [x + 10.0, y + ph - 14.0], 0x008A_93A8, false);
    }
}

fn roman(n: i32) -> String {
    const TABLE: [(i32, &str); 6] = [(10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"), (0, "")];
    if n <= 0 || n > 39 {
        return n.to_string();
    }
    let mut n = n;
    let mut out = String::new();
    for (value, symbol) in TABLE {
        while value > 0 && n >= value {
            out.push_str(symbol);
            n -= value;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::roman;

    #[test]
    fn effect_levels_are_roman_numerals() {
        assert_eq!(roman(2), "II");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(9), "IX");
        assert_eq!(roman(14), "XIV");
        assert_eq!(roman(0), "0");
    }
}
