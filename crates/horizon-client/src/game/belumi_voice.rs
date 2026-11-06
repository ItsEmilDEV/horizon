use super::Game;

#[derive(Default)]
pub(super) struct VoiceState {
    #[cfg(feature = "belumi-voice")]
    client: Option<belumi_voice::VoiceClient>,
    #[cfg(feature = "belumi-voice")]
    muted: bool,
    #[cfg(feature = "belumi-voice")]
    last_state: Option<belumi_voice::State>,
    #[cfg(feature = "belumi-voice")]
    since_update: f32,
    #[cfg(feature = "belumi-voice")]
    held: bool,
    #[cfg(feature = "belumi-voice")]
    roster: belumi_voice::groups::Roster,
    #[cfg(feature = "belumi-voice")]
    sent_state: Option<(bool, bool)>,
    #[cfg(feature = "belumi-voice")]
    deafened: bool,
    #[cfg(feature = "belumi-voice")]
    connecting_for: f32,
    #[cfg(feature = "belumi-voice")]
    target: String,
    #[cfg(feature = "belumi-voice")]
    starting: Option<std::sync::mpsc::Receiver<Result<belumi_voice::VoiceClient, String>>>,
    #[cfg(feature = "belumi-voice")]
    menu: bool,
    #[cfg(feature = "belumi-voice")]
    menu_row: usize,
    #[cfg(feature = "belumi-voice")]
    probe: usize,
    #[cfg(feature = "belumi-voice")]
    probe_wait: f32,
    #[cfg(not(feature = "belumi-voice"))]
    warned: bool,
}

#[allow(dead_code)]
pub(super) fn speaking_line(names: &[String]) -> Option<String> {
    match names.len() {
        0 => None,
        1..=3 => Some(format!("Speaking: {}", names.join(", "))),
        n => Some(format!("Speaking: {} and {} more", names[..3].join(", "), n - 3)),
    }
}

#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub(super) enum VoiceCmd {
    Help,
    Groups,
    Create { name: String, password: Option<String> },
    Join { target: String, password: Option<String> },
    Leave,
    Players,
    Volume { player: String, percent: Option<u16> },
    VolumeReset(String),
    Mute,
    Deafen,
}

#[allow(dead_code)]
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

#[allow(dead_code)]
pub(super) fn parse_voice_command(text: &str) -> Option<VoiceCmd> {
    let w = words(text);
    let head = w.first()?.to_ascii_lowercase();
    if head != "voice" && head != "vc" {
        return None;
    }
    let sub = w.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    Some(match sub.as_str() {
        "groups" | "list" => VoiceCmd::Groups,
        "group" | "g" => match w.get(2).map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("create" | "new") => match w.get(3) {
                Some(name) => VoiceCmd::Create { name: name.clone(), password: w.get(4).cloned() },
                None => VoiceCmd::Help,
            },
            Some("join") => match w.get(3) {
                Some(target) => VoiceCmd::Join { target: target.clone(), password: w.get(4).cloned() },
                None => VoiceCmd::Help,
            },
            Some("leave") => VoiceCmd::Leave,
            Some("list") | None => VoiceCmd::Groups,
            _ => VoiceCmd::Help,
        },
        "players" => VoiceCmd::Players,
        "volume" | "vol" => {
            let Some(player) = w.get(2).cloned() else {
                return Some(VoiceCmd::Help);
            };
            match w.get(3).map(String::as_str) {
                None => VoiceCmd::Volume { player, percent: None },
                Some("reset") => VoiceCmd::VolumeReset(player),
                Some(p) => match p.trim_end_matches('%').parse::<u16>().ok().filter(|p| *p <= 300) {
                    Some(percent) => VoiceCmd::Volume { player, percent: Some(percent) },
                    None => VoiceCmd::Help,
                },
            }
        }
        "mute" => VoiceCmd::Mute,
        "deafen" | "deaf" => VoiceCmd::Deafen,
        _ => VoiceCmd::Help,
    })
}

#[cfg(feature = "belumi-voice")]
pub(super) use real::register_devices;
#[cfg(not(feature = "belumi-voice"))]
pub(super) use stub::register_devices;

#[cfg(feature = "belumi-voice")]
mod real {
    use super::*;
    use belumi_voice::groups::{self, Badge, Incoming};

    enum MenuAction {
        Mute,
        Deafen,
        Join(u128),
        NeedsPassword(String),
        Leave,
        Player(u128),
    }

    struct MenuRow {
        text: String,
        color: u32,
        action: Option<MenuAction>,
    }
    use belumi_voice::{
        AudioMode, COMPATIBILITY_VERSION, Listener, Mode, REQUEST_SECRET_CHANNEL, SECRET_CHANNEL,
        State, VoiceClient, VoiceSettings, parse_secret, request_secret_payload,
    };
    use horizon_protocol::packets::{configuration::CustomPayload, play::Serverbound};

    pub(in crate::game) fn register_devices() {
        horizon_client::belumi::set_input_device_lister(belumi_voice::input_devices);
    }

    impl Game<'_> {
        pub(in crate::game) fn belumi_voice_channel(&self, channel: &str) -> bool {
            self.settings.belumi.voice && (channel == SECRET_CHANNEL || groups::is_channel(channel))
        }

        fn voice_me(&self) -> u128 {
            horizon_client::auth::current().map_or(0, |a| a.uuid)
        }

        fn voice_send(&self, channel: &str, data: Vec<u8>) {
            if let Some(r) = self.remote.as_ref() {
                r.session.send(Serverbound::CustomPayload(CustomPayload {
                    channel: channel.to_owned(),
                    data,
                }));
            }
        }

        fn voice_group_event(&mut self, channel: &str, data: &[u8]) {
            let incoming = match groups::parse_detect(channel, data) {
                Ok((i, legacy)) => {
                    if legacy {
                        self.belumi_voice.roster.legacy = true;
                    }
                    i
                }
                Err(e) => {
                    let head: String = data.iter().take(48).map(|b| format!("{b:02x}")).collect();
                    log::warn!("voice chat: unreadable {channel} packet ({e}), {} bytes: {head}", data.len());
                    return;
                }
            };
            let joined = matches!(incoming, Incoming::Joined { wrong_password: false, .. });
            let me = self.voice_me();
            self.belumi_voice.roster.apply(incoming, me);
            if let Some(notice) = self.belumi_voice.roster.notice.take() {
                self.native_chat(&notice);
            }
            if joined {
                let r = &self.belumi_voice.roster;
                let text = match r.my_group.and_then(|g| r.groups.get(&g)) {
                    Some(g) => format!("Joined the group {}.", g.name),
                    None => "Left the group.".to_owned(),
                };
                self.native_chat(&text);
            }
        }

        fn belumi_voice_settings(&self) -> VoiceSettings {
            let c = &self.settings.belumi;
            VoiceSettings {
                mode: if c.voice_mode == 1 { Mode::Activation } else { Mode::PushToTalk },
                threshold_db: c.voice_threshold_db(),
                mic_gain: c.voice_mic_gain(),
                volume: c.voice_volume_gain(),
                muted: self.belumi_voice.muted,
                deafened: self.belumi_voice.deafened,
            }
        }

        fn voice_debug(&mut self, text: &str) {
            log::info!("{text}");
            if std::env::var_os("BELUMI_DEBUG_VOICE").is_some() {
                self.native_chat(text);
            }
        }

        fn belumi_voice_request(&mut self) {
            let versions = [20, COMPATIBILITY_VERSION, 19, 22, 18, 23];
            let Some(&version) = versions.get(self.belumi_voice.probe) else {
                if self.belumi_voice.probe == versions.len() {
                    self.belumi_voice.probe += 1;
                    self.belumi_voice.probe_wait = -1.0;
                    self.voice_debug("Voice chat: the server accepted none of the known protocol versions.");
                }
                return;
            };
            self.belumi_voice.probe_wait = 0.0;
            self.voice_debug(&format!("Voice chat: asking the server (protocol {version})."));
            if let Some(r) = self.remote.as_ref() {
                r.session.send(Serverbound::CustomPayload(CustomPayload {
                    channel: REQUEST_SECRET_CHANNEL.to_owned(),
                    data: request_secret_payload(version),
                }));
            }
        }

        pub(in crate::game) fn belumi_voice_joined(&mut self) {
            if !self.settings.belumi.voice || self.belumi_maps.server.is_empty() {
                return;
            }
            self.belumi_voice.probe = 0;
            self.belumi_voice_request();
        }

        pub(in crate::game) fn belumi_voice_payload(&mut self, channel: &str, data: &[u8]) {
            if channel != SECRET_CHANNEL {
                self.voice_group_event(channel, data);
                return;
            }
            let init = match parse_secret(data) {
                Ok(i) => i,
                Err(e) => {
                    self.native_chat(&format!("Voice chat: unreadable server data ({e})."));
                    return;
                }
            };
            if self.belumi_voice.starting.is_some()
                || self
                    .belumi_voice
                    .client
                    .as_ref()
                    .is_some_and(|c| !matches!(c.state(), State::Failed(_)))
            {
                return;
            }
            self.belumi_voice.client = None;
            self.belumi_voice.sent_state = None;
            self.belumi_voice.probe_wait = -1.0;
            self.voice_debug(&format!(
                "Voice chat: server answered, connecting to '{}' port {}.",
                init.voice_host, init.server_port
            ));
            self.belumi_voice.connecting_for = 0.0;
            self.belumi_voice.target = format!("'{}' port {}", init.voice_host, init.server_port);
            let host = self.belumi_maps.server.clone();
            let settings = self.belumi_voice_settings();
            let microphone = Some(self.settings.belumi.voice_device.clone()).filter(|d| !d.is_empty());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let started = VoiceClient::start(init, &host, AudioMode::Devices, settings, microphone);
                let _ = tx.send(started.map_err(|e| e.to_string()));
            });
            self.belumi_voice.starting = Some(rx);
        }

        pub(in crate::game) fn belumi_voice_left(&mut self) {
            self.belumi_voice.starting = None;
            self.belumi_voice.client = None;
            self.belumi_voice.last_state = None;
            self.belumi_voice.held = false;
            self.belumi_voice.sent_state = None;
            self.belumi_voice.roster = Default::default();
            self.belumi_voice.probe_wait = -1.0;
        }

        pub(in crate::game) fn belumi_voice_key(&mut self, key: &str, pressed: bool) {
            let cfg = &self.settings.belumi;
            if !cfg.voice {
                return;
            }
            if pressed && key == cfg.voice_menu_key && (self.belumi_voice.menu || (self.native_playing() && self.remote.is_some())) {
                self.belumi_voice.menu = !self.belumi_voice.menu;
                return;
            }
            if self.belumi_voice.menu {
                if pressed {
                    self.voice_menu_key(key);
                }
                return;
            }
            if key == cfg.voice_key {
                let on = pressed && (self.native_playing() || self.belumi_voice.held);
                self.belumi_voice.held = on;
                if let Some(c) = self.belumi_voice.client.as_ref() {
                    c.set_talking(on);
                }
            } else if pressed && key == cfg.voice_deafen_key && self.native_playing() {
                self.voice_toggle_deafen();
            } else if pressed && key == cfg.voice_mute_key && self.native_playing() {
                self.belumi_voice.muted = !self.belumi_voice.muted;
                let text = if self.belumi_voice.muted { "Microphone muted." } else { "Microphone on." };
                self.native_chat(text);
            }
        }

        pub(in crate::game) fn belumi_voice_menu_open(&self) -> bool {
            self.belumi_voice.menu
        }

        /// The menu's lines: headers and hints are not selectable, the rest carry what Enter does.
        fn voice_menu_rows(&self) -> Vec<MenuRow> {
            const WHITE: u32 = 0xFFFF_FFFF;
            const GRAY: u32 = 0xFFAA_AAAA;
            const YELLOW: u32 = 0xFFFF_FF55;
            const GREEN: u32 = 0xFF55_FF55;
            const RED: u32 = 0xFFFF_5555;
            let line = |text: String, color: u32, action: Option<MenuAction>| MenuRow { text, color, action };
            let on = |b: bool| if b { "on" } else { "off" };
            let r = &self.belumi_voice.roster;
            let mut rows = vec![
                line(format!("Microphone muted: {}", on(self.belumi_voice.muted)), if self.belumi_voice.muted { RED } else { WHITE }, Some(MenuAction::Mute)),
                line(format!("Deafened: {}", on(self.belumi_voice.deafened)), if self.belumi_voice.deafened { RED } else { WHITE }, Some(MenuAction::Deafen)),
                line("Groups".into(), YELLOW, None),
            ];
            let groups = r.listed_groups();
            if groups.is_empty() {
                rows.push(line("No groups yet".into(), GRAY, None));
            }
            for g in groups {
                let mine = r.my_group == Some(g.id);
                let lock = if g.has_password { ", password" } else { "" };
                let text = format!("{} ({} in it{lock}){}", g.name, r.members(g.id), if mine { "  <- you" } else { "" });
                let action = if mine { MenuAction::Leave } else if g.has_password { MenuAction::NeedsPassword(g.name.clone()) } else { MenuAction::Join(g.id) };
                rows.push(line(text, if mine { GREEN } else { WHITE }, Some(action)));
            }
            rows.push(line("New group: /voice group create NAME".into(), GRAY, None));
            rows.push(line("Players".into(), YELLOW, None));
            let me = self.voice_me();
            let mut players: Vec<(String, u128)> = self
                .remote
                .as_ref()
                .map(|r| r.tab_players.iter().map(|(u, p)| (p.name.clone(), *u)).filter(|(_, u)| *u != me).collect())
                .unwrap_or_default();
            players.sort_by_key(|(n, _)| n.to_lowercase());
            if players.is_empty() {
                rows.push(line("Nobody else is here".into(), GRAY, None));
            }
            for (name, uuid) in players {
                let badge = match r.players.get(&uuid).and_then(|s| s.badge()) {
                    Some(Badge::Disconnected) => " [no voice chat]",
                    Some(Badge::FullMuted) => " [deafened]",
                    Some(Badge::Muted) => " [muted]",
                    None => "",
                };
                let pct = self.voice_percent(uuid);
                rows.push(line(format!("{name}{badge}   < {pct}% >"), if badge.is_empty() { WHITE } else { GRAY }, Some(MenuAction::Player(uuid))));
            }
            rows
        }

        fn voice_percent(&self, uuid: u128) -> u16 {
            self.settings.belumi.voice_player_volumes.get(&format!("{uuid:032x}")).copied().unwrap_or(100)
        }

        fn voice_set_percent(&mut self, uuid: u128, percent: u16) {
            let key = format!("{uuid:032x}");
            if percent == 100 {
                self.settings.belumi.voice_player_volumes.remove(&key);
            } else {
                self.settings.belumi.voice_player_volumes.insert(key, percent.min(300));
            }
            self.settings.save(&Game::options_path());
        }

        fn voice_menu_selectable(&self) -> Vec<usize> {
            self.voice_menu_rows().iter().enumerate().filter(|(_, r)| r.action.is_some()).map(|(i, _)| i).collect()
        }

        fn voice_menu_move(&mut self, step: i32) {
            let list = self.voice_menu_selectable();
            if list.is_empty() {
                return;
            }
            let at = list.iter().position(|i| *i == self.belumi_voice.menu_row).unwrap_or(0) as i32;
            let next = (at + step).rem_euclid(list.len() as i32) as usize;
            self.belumi_voice.menu_row = list[next];
        }

        fn voice_menu_key(&mut self, key: &str) {
            let rows = self.voice_menu_rows();
            let action = rows.get(self.belumi_voice.menu_row).and_then(|r| r.action.as_ref());
            match key {
                "key.keyboard.escape" => self.belumi_voice.menu = false,
                "key.keyboard.up" => self.voice_menu_move(-1),
                "key.keyboard.down" => self.voice_menu_move(1),
                "key.keyboard.left" | "key.keyboard.right" => {
                    if let Some(MenuAction::Player(uuid)) = action {
                        let uuid = *uuid;
                        let step: i32 = if key.ends_with("left") { -10 } else { 10 };
                        let next = (i32::from(self.voice_percent(uuid)) + step).clamp(0, 300) as u16;
                        self.voice_set_percent(uuid, next);
                    }
                }
                "key.keyboard.r" => {
                    if let Some(MenuAction::Player(uuid)) = action {
                        let uuid = *uuid;
                        self.voice_set_percent(uuid, 100);
                    }
                }
                "key.keyboard.enter" | "key.keyboard.space" => match action {
                    Some(MenuAction::Mute) => self.belumi_voice.muted = !self.belumi_voice.muted,
                    Some(MenuAction::Deafen) => self.voice_toggle_deafen(),
                    Some(MenuAction::Join(id)) => {
                        let id = *id;
                        self.voice_send(groups::SET_GROUP, groups::join_group(id, None));
                    }
                    Some(MenuAction::NeedsPassword(name)) => {
                        let line = format!("This group has a password: /voice group join \"{name}\" PASSWORD");
                        self.belumi_voice.menu = false;
                        self.native_chat(&line);
                    }
                    Some(MenuAction::Leave) => self.voice_send(groups::LEAVE_GROUP, groups::leave_group()),
                    Some(MenuAction::Player(uuid)) => {
                        let uuid = *uuid;
                        self.voice_set_percent(uuid, 100);
                    }
                    None => {}
                },
                _ => {}
            }
        }

        pub(in crate::game) fn belumi_voice_wheel(&mut self, steps: f32, claimed: &mut bool) {
            if !self.belumi_voice.menu || *claimed {
                return;
            }
            self.voice_menu_move(if steps > 0.0 { -1 } else { 1 });
            *claimed = true;
        }

        pub(in crate::game) fn belumi_voice_draw_menu(&mut self, batch: &mut horizon_client::hud::HudBatch) {
            if !self.belumi_voice.menu {
                return;
            }
            if !self.settings.belumi.voice || self.remote.is_none() || self.screen != crate::game::Screen::Playing {
                self.belumi_voice.menu = false;
                return;
            }
            let rows = self.voice_menu_rows();
            if rows.get(self.belumi_voice.menu_row).is_none_or(|r| r.action.is_none()) {
                self.belumi_voice.menu_row = rows.iter().position(|r| r.action.is_some()).unwrap_or(0);
            }
            let hint = "Up/Down: pick   Left/Right: volume   Enter: use   R: reset   Esc: close";
            let mut width = self.native_text_width(hint) + 16.0;
            for r in &rows {
                width = width.max(self.native_text_width(&r.text) + 24.0);
            }
            let height = rows.len() as f32 * 11.0 + 34.0;
            let (x, y) = ((batch.width - width) / 2.0, ((batch.height - height) / 2.0).max(4.0));
            self.native_fill(batch, [0.0, 0.0, batch.width, batch.height], 0x6000_0000);
            self.belumi_panel(batch, [x, y, width, height]);
            self.native_text(batch, "Voice chat", [x + 6.0, y + 4.0], 0x00FF_FFFF, true);
            let mut row_y = y + 18.0;
            for (i, r) in rows.iter().enumerate() {
                if i == self.belumi_voice.menu_row && r.action.is_some() {
                    self.native_fill(batch, [x + 3.0, row_y - 1.0, width - 6.0, 10.0], 0x5055_AA55);
                }
                let indent = if r.action.is_some() { 10.0 } else { 6.0 };
                self.native_text(batch, &r.text, [x + indent, row_y], r.color & 0x00FF_FFFF, true);
                row_y += 11.0;
            }
            self.native_text(batch, hint, [x + 6.0, y + height - 11.0], 0x00AA_AAAA, true);
        }

        fn voice_toggle_deafen(&mut self) {
            self.belumi_voice.deafened = !self.belumi_voice.deafened;
            let text = if self.belumi_voice.deafened {
                "Voice chat deafened: you hear nobody and nobody hears you."
            } else {
                "Voice chat on."
            };
            self.native_chat(text);
        }

        pub(in crate::game) fn belumi_voice_frame(&mut self, dt: f32) {
            if !self.settings.belumi.voice {
                if self.belumi_voice.client.is_some() {
                    self.belumi_voice_left();
                }
                return;
            }
            if let Some(rx) = &self.belumi_voice.starting
                && let Ok(started) = rx.try_recv()
            {
                self.belumi_voice.starting = None;
                match started {
                    Ok(client) => {
                        self.belumi_voice.last_state = None;
                        self.belumi_voice.client = Some(client);
                    }
                    Err(e) => self.native_chat(&format!("Voice chat: {e}")),
                }
            }
            let settings = self.belumi_voice_settings();
            if self.belumi_voice.client.is_none() {
                if self.remote.is_some() && self.belumi_voice.probe_wait >= 0.0 {
                    self.belumi_voice.probe_wait += dt;
                    if self.belumi_voice.probe_wait > 4.0 {
                        self.belumi_voice.probe += 1;
                        self.belumi_voice_request();
                    }
                }
                return;
            }
            let wanted = Some(self.settings.belumi.voice_device.clone()).filter(|d| !d.is_empty());
            if let Some(client) = self.belumi_voice.client.as_mut() {
                if client.microphone() != wanted.as_deref() {
                    client.set_microphone(wanted);
                }
            }
            let Some(client) = self.belumi_voice.client.as_ref() else {
                return;
            };
            client.set_settings(settings);
            let want = (self.belumi_voice.muted, self.belumi_voice.deafened);
            let connected = matches!(client.state(), State::Connected);
            if connected && self.belumi_voice.sent_state != Some(want) {
                self.belumi_voice.sent_state = Some(want);
                let packet = if self.belumi_voice.roster.legacy {
                    groups::update_state_legacy(want.1)
                } else {
                    groups::update_state(want.0, want.1)
                };
                self.voice_send(groups::CLIENT_STATE, packet);
            }
            let Some(client) = self.belumi_voice.client.as_ref() else {
                return;
            };
            let mut report = None;
            if matches!(client.state(), State::Connecting | State::Authenticated) {
                self.belumi_voice.connecting_for += dt;
                if (6.0..6.0 + dt + 0.001).contains(&self.belumi_voice.connecting_for) {
                    let (sent, received) = client.packets();
                    report = Some(format!(
                        "Voice chat: still connecting to {} (sent {sent}, received {received} packets).",
                        self.belumi_voice.target
                    ));
                }
            }
            let notes = client.take_notes();
            let state = client.state();
            if let Some(text) = report {
                self.voice_debug(&text);
            }
            for n in notes {
                self.native_chat(&format!("Voice chat: {n}"));
            }
            if self.belumi_voice.last_state.as_ref() != Some(&state) {
                match &state {
                    State::Connected => self.native_chat("Voice chat connected."),
                    State::Authenticated => self.voice_debug("Voice chat: authenticated, waiting for audio."),
                    State::Failed(why) => self.native_chat(&format!("Voice chat lost: {why}")),
                    _ => {}
                }
                self.belumi_voice.last_state = Some(state.clone());
            }
            self.belumi_voice.since_update += dt;
            if self.belumi_voice.since_update >= 0.1
                && let (Some(p), Some(r)) = (self.native_player(), self.remote.as_ref())
            {
                self.belumi_voice.since_update = 0.0;
                let players = r.player_positions();
                let listener = Listener { pos: [p.x, p.y, p.z], yaw: p.yaw };
                let volumes: std::collections::HashMap<u128, f32> = self
                    .settings
                    .belumi
                    .voice_player_volumes
                    .iter()
                    .filter_map(|(k, p)| {
                        Some((u128::from_str_radix(k, 16).ok()?, f32::from(*p) / 100.0))
                    })
                    .collect();
                if let Some(c) = self.belumi_voice.client.as_ref() {
                    c.update_world(listener, players);
                    c.set_player_volumes(volumes);
                }
            }
        }

        pub(in crate::game) fn belumi_voice_panel(
            &self,
            batch: &mut horizon_client::hud::HudBatch,
            corners: &mut horizon_client::extras_hud::Corners,
            corner: horizon_client::extras::Corner,
        ) {
            const GREEN: u32 = 0xFF55_FF55;
            const RED: u32 = 0xFFFF_5555;
            const YELLOW: u32 = 0xFFFF_FF55;
            const GRAY: u32 = 0xFFAA_AAAA;
            const WHITE: u32 = 0xFFFF_FFFF;
            const CYAN: u32 = 0xFF55_FFFF;
            if !self.settings.belumi.voice || self.remote.is_none() || !self.native_corner_shown(corner) {
                return;
            }
            let (dot, title, level) = match self.belumi_voice.client.as_ref() {
                None if self.belumi_voice.probe_wait >= 0.0 => (YELLOW, "Voice: connecting".to_owned(), None),
                None => return,
                Some(c) => match c.state() {
                    State::Connecting | State::Authenticated => (YELLOW, "Voice: connecting".to_owned(), None),
                    State::Failed(_) => (RED, "Voice: connection lost".to_owned(), None),
                    State::Connected => {
                        let cfg = &self.settings.belumi;
                        let level = c.mic_level_db();
                        let open = self.belumi_voice.held
                            || (cfg.voice_mode == 1 && level >= cfg.voice_threshold_db());
                        if self.belumi_voice.muted {
                            (RED, "Muted".to_owned(), Some(level))
                        } else if open {
                            (GREEN, "Talking".to_owned(), Some(level))
                        } else if cfg.voice_mode == 1 {
                            (GRAY, "Listening".to_owned(), Some(level))
                        } else {
                            let key = horizon_client::keybinds::display_name(&cfg.voice_key).plain();
                            (GRAY, format!("Hold {key} to talk"), Some(level))
                        }
                    }
                },
            };
            let mut speakers: Vec<String> = match self.belumi_voice.client.as_ref() {
                Some(c) if matches!(c.state(), State::Connected) => c
                    .speaking()
                    .into_iter()
                    .filter_map(|u| {
                        self.remote
                            .as_ref()
                            .and_then(|r| r.tab_players.get(&u).map(|p| p.name.clone()))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            speakers.truncate(6);
            let group_line = {
                let r = &self.belumi_voice.roster;
                r.my_group
                    .and_then(|g| r.groups.get(&g))
                    .map(|g| format!("Group: {}", g.name))
            };
            let bar_width = 64.0;
            let mut width = self.native_text_width(&title) + 12.0;
            for name in &speakers {
                width = width.max(self.native_text_width(name) + 12.0);
            }
            width = width.max(bar_width + 8.0);
            if let Some(g) = &group_line {
                width = width.max(self.native_text_width(g) + 12.0);
            }
            let height = 12.0
                + if level.is_some() { 8.0 } else { 0.0 }
                + if group_line.is_some() { 10.0 } else { 0.0 }
                + speakers.len() as f32 * 10.0;
            let (x, y) = corners.place(corner, width, height);
            self.belumi_panel(batch, [x, y, width, height]);
            self.native_fill(batch, [x + 3.0, y + 3.0, 5.0, 5.0], dot);
            self.native_text(batch, &title, [x + 11.0, y + 2.0], WHITE & 0x00FF_FFFF, true);
            let mut row = y + 12.0;
            if let Some(db) = level {
                let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                self.native_fill(batch, [x + 4.0, row, bar_width, 3.0], 0x8040_4040);
                self.native_fill(batch, [x + 4.0, row, bar_width * fill, 3.0], if fill > 0.85 { RED } else { GREEN });
                if self.settings.belumi.voice_mode == 1 {
                    let mark = ((self.settings.belumi.voice_threshold_db() + 60.0) / 60.0).clamp(0.0, 1.0);
                    self.native_fill(batch, [x + 4.0 + bar_width * mark, row - 1.0, 1.0, 5.0], WHITE);
                }
                row += 8.0;
            }
            if let Some(g) = &group_line {
                self.native_text(batch, g, [x + 4.0, row], YELLOW & 0x00FF_FFFF, true);
                row += 10.0;
            }
            for name in &speakers {
                self.native_fill(batch, [x + 4.0, row + 2.0, 3.0, 3.0], CYAN);
                self.native_text(batch, name, [x + 10.0, row], CYAN & 0x00FF_FFFF, true);
                row += 10.0;
            }
        }

        pub(in crate::game) fn belumi_voice_command(&mut self, command: &str, ran: &mut bool) {
            if *ran {
                return;
            }
            let Some(cmd) = super::parse_voice_command(command) else {
                return;
            };
            *ran = true;
            if !self.settings.belumi.voice {
                self.native_chat("Voice chat is off. Turn it on in the Belumi settings.");
                return;
            }
            match cmd {
                super::VoiceCmd::Help => {
                    for line in [
                        "/voice groups | group create NAME [PASSWORD] | group join NAME|NUMBER [PASSWORD] | group leave",
                        "/voice players | volume PLAYER [0-300|reset] | mute | deafen",
                        "Quote names with spaces: /voice group create \"My team\"",
                    ] {
                        self.native_chat(line);
                    }
                }
                super::VoiceCmd::Groups => {
                    let r = &self.belumi_voice.roster;
                    let lines: Vec<String> = r
                        .listed_groups()
                        .into_iter()
                        .enumerate()
                        .map(|(i, g)| {
                            format!(
                                "{}. {} ({}, {} in it{}){}",
                                i + 1,
                                g.name,
                                g.kind_name(),
                                r.members(g.id),
                                if g.has_password { ", password" } else { "" },
                                if r.my_group == Some(g.id) { " <- you" } else { "" },
                            )
                        })
                        .collect();
                    if lines.is_empty() {
                        self.native_chat("No groups yet. Make one with /voice group create NAME [PASSWORD].");
                    }
                    for l in lines {
                        self.native_chat(&l);
                    }
                }
                super::VoiceCmd::Create { name, password } => {
                    if name.is_empty() || name.chars().count() > groups::MAX_GROUP_NAME {
                        self.native_chat("A group name has 1 to 24 characters.");
                    } else if password.as_ref().is_some_and(|p| p.chars().count() > groups::MAX_GROUP_NAME) {
                        self.native_chat("A group password has at most 24 characters.");
                    } else {
                        self.voice_send(groups::CREATE_GROUP, groups::create_group(&name, password.as_deref(), 0));
                    }
                }
                super::VoiceCmd::Join { target, password } => {
                    match self.belumi_voice.roster.find_group(&target).map(|g| g.id) {
                        Some(id) => self.voice_send(groups::SET_GROUP, groups::join_group(id, password.as_deref())),
                        None => self.native_chat(&format!("No group matches {target}. See /voice groups.")),
                    }
                }
                super::VoiceCmd::Leave => {
                    if self.belumi_voice.roster.my_group.is_some() {
                        self.voice_send(groups::LEAVE_GROUP, groups::leave_group());
                    } else {
                        self.native_chat("You are not in a group.");
                    }
                }
                super::VoiceCmd::Players => {
                    let mut rows: Vec<(String, u128)> = self
                        .remote
                        .as_ref()
                        .map(|r| r.tab_players.iter().map(|(u, p)| (p.name.clone(), *u)).collect())
                        .unwrap_or_default();
                    rows.sort_by_key(|(n, _)| n.to_lowercase());
                    for (name, uuid) in rows {
                        let state = match self.belumi_voice.roster.players.get(&uuid).and_then(|s| s.badge()) {
                            Some(Badge::Disconnected) => " [no voice chat]",
                            Some(Badge::FullMuted) => " [deafened]",
                            Some(Badge::Muted) => " [muted]",
                            None => "",
                        };
                        let pct = self.settings.belumi.voice_player_volumes.get(&format!("{uuid:032x}"));
                        let volume = pct.map_or(String::new(), |p| format!(" {p}%"));
                        self.native_chat(&format!("{name}{state}{volume}"));
                    }
                }
                super::VoiceCmd::Volume { player, percent } => {
                    let Some((name, uuid)) = self.voice_find_player(&player) else {
                        self.native_chat(&format!("No player matches {player}."));
                        return;
                    };
                    let key = format!("{uuid:032x}");
                    match percent {
                        None => {
                            let p = self.settings.belumi.voice_player_volumes.get(&key).copied().unwrap_or(100);
                            self.native_chat(&format!("{name}: {p}%"));
                        }
                        Some(p) => {
                            if p == 100 {
                                self.settings.belumi.voice_player_volumes.remove(&key);
                            } else {
                                self.settings.belumi.voice_player_volumes.insert(key, p);
                            }
                            self.settings.save(&Game::options_path());
                            self.native_chat(&format!("{name}: {p}%"));
                        }
                    }
                }
                super::VoiceCmd::VolumeReset(player) => match self.voice_find_player(&player) {
                    Some((name, uuid)) => {
                        self.settings.belumi.voice_player_volumes.remove(&format!("{uuid:032x}"));
                        self.settings.save(&Game::options_path());
                        self.native_chat(&format!("{name}: 100%"));
                    }
                    None => self.native_chat(&format!("No player matches {player}.")),
                },
                super::VoiceCmd::Mute => {
                    self.belumi_voice.muted = !self.belumi_voice.muted;
                    let text = if self.belumi_voice.muted { "Microphone muted." } else { "Microphone on." };
                    self.native_chat(text);
                }
                super::VoiceCmd::Deafen => self.voice_toggle_deafen(),
            }
        }

        fn voice_find_player(&self, text: &str) -> Option<(String, u128)> {
            let wanted = text.to_lowercase();
            let r = self.remote.as_ref()?;
            let mut all: Vec<(String, u128)> = r.tab_players.iter().map(|(u, p)| (p.name.clone(), *u)).collect();
            all.sort_by_key(|(n, _)| n.to_lowercase());
            all.iter()
                .find(|(n, _)| n.to_lowercase() == wanted)
                .or_else(|| all.iter().find(|(n, _)| n.to_lowercase().starts_with(&wanted)))
                .cloned()
        }

        pub(in crate::game) fn belumi_voice_draw_badges(&mut self, batch: &mut horizon_client::hud::HudBatch) {
            if !self.settings.belumi.voice || self.belumi_voice.roster.players.is_empty() || !self.camera_type.first_person() {
                return;
            }
            let (Some(p), Some(r)) = (self.player.as_ref(), self.remote.as_ref()) else {
                return;
            };
            let eye = [p.position.x, p.position.y + f64::from(p.dimensions.eye_height), p.position.z];
            let cam = crate::game::belumi_maps::Camera {
                eye,
                yaw: self.yaw,
                pitch: self.pitch,
                vertical_fov: self.settings.fov.clamp(1.0, 170.0),
                aspect: batch.width / batch.height.max(1.0),
            };
            let me = self.voice_me();
            let mut marks: Vec<(f32, f32, &'static str, u32)> = Vec::new();
            for (uuid, pos) in r.player_positions() {
                if uuid == me {
                    continue;
                }
                let Some(badge) = self.belumi_voice.roster.players.get(&uuid).and_then(|s| s.badge()) else {
                    continue;
                };
                let Some((u, v)) = crate::game::belumi_maps::project(&cam, [pos[0], pos[1] + 2.75, pos[2]]) else {
                    continue;
                };
                let (label, color) = match badge {
                    Badge::Disconnected => ("NO VOICE", 0xFF70_7070),
                    Badge::FullMuted => ("DEAF", 0xFFCC_3333),
                    Badge::Muted => ("MUTED", 0xFFDD_8822),
                };
                marks.push((u * batch.width, v * batch.height, label, color));
            }
            for (x, y, label, color) in marks {
                let w = self.native_text_width(label) + 4.0;
                self.native_fill(batch, [x - w / 2.0, y - 5.0, w, 10.0], color);
                self.native_text(batch, label, [x - w / 2.0 + 2.0, y - 4.0], 0x00FF_FFFF, false);
            }
        }

        pub(in crate::game) fn belumi_voice_draw_speakers(&mut self, batch: &mut horizon_client::hud::HudBatch) {
            let speaking = match self.belumi_voice.client.as_ref() {
                Some(c) if matches!(c.state(), State::Connected) => c.speaking(),
                _ => return,
            };
            if speaking.is_empty() || !self.camera_type.first_person() {
                return;
            }
            let (Some(p), Some(r)) = (self.player.as_ref(), self.remote.as_ref()) else {
                return;
            };
            let eye = [p.position.x, p.position.y + f64::from(p.dimensions.eye_height), p.position.z];
            let cam = crate::game::belumi_maps::Camera {
                eye,
                yaw: self.yaw,
                pitch: self.pitch,
                vertical_fov: self.settings.fov.clamp(1.0, 170.0),
                aspect: batch.width / batch.height.max(1.0),
            };
            let spots: Vec<(f32, f32)> = r
                .player_positions()
                .into_iter()
                .filter(|(u, _)| speaking.contains(u))
                .filter_map(|(_, pos)| crate::game::belumi_maps::project(&cam, [pos[0], pos[1] + 2.4, pos[2]]))
                .collect();
            for (u, v) in spots {
                let (x, y) = (u * batch.width, v * batch.height);
                for i in 0..3 {
                    let h = 3.0 + 3.0 * i as f32;
                    self.native_fill(batch, [x - 6.0 + 5.0 * i as f32, y - h, 3.0, h], 0xFF55_FF55);
                }
            }
        }
    }
}

#[cfg(not(feature = "belumi-voice"))]
mod stub {
    use super::*;

    pub(in crate::game) fn register_devices() {}

    impl Game<'_> {
        pub(in crate::game) fn belumi_voice_channel(&self, _channel: &str) -> bool {
            false
        }
        pub(in crate::game) fn belumi_voice_payload(&mut self, _channel: &str, _data: &[u8]) {}
        pub(in crate::game) fn belumi_voice_command(&mut self, _command: &str, _ran: &mut bool) {}
        pub(in crate::game) fn belumi_voice_draw_badges(&mut self, _batch: &mut horizon_client::hud::HudBatch) {}
        pub(in crate::game) fn belumi_voice_joined(&mut self) {
            if self.settings.belumi.voice && !self.belumi_voice.warned {
                self.belumi_voice.warned = true;
                self.native_chat("This build has no voice chat support (build with --features belumi-voice).");
            }
        }
        pub(in crate::game) fn belumi_voice_left(&mut self) {}
        pub(in crate::game) fn belumi_voice_key(&mut self, _key: &str, _pressed: bool) {}
        pub(in crate::game) fn belumi_voice_menu_open(&self) -> bool {
            false
        }
        pub(in crate::game) fn belumi_voice_wheel(&mut self, _steps: f32, _claimed: &mut bool) {}
        pub(in crate::game) fn belumi_voice_draw_menu(&mut self, _batch: &mut horizon_client::hud::HudBatch) {}
        pub(in crate::game) fn belumi_voice_frame(&mut self, _dt: f32) {}
        pub(in crate::game) fn belumi_voice_panel(
            &self,
            _batch: &mut horizon_client::hud::HudBatch,
            _corners: &mut horizon_client::extras_hud::Corners,
            _corner: horizon_client::extras::Corner,
        ) {
        }
        pub(in crate::game) fn belumi_voice_draw_speakers(&mut self, _batch: &mut horizon_client::hud::HudBatch) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_commands_parse_with_quotes_and_defaults() {
        use super::VoiceCmd::*;
        assert_eq!(parse_voice_command("voice groups"), Some(Groups));
        assert_eq!(parse_voice_command("vc"), Some(Help));
        assert_eq!(parse_voice_command("waypoint add"), None);
        assert_eq!(
            parse_voice_command("voice group create \"My team\" secret"),
            Some(Create { name: "My team".into(), password: Some("secret".into()) })
        );
        assert_eq!(
            parse_voice_command("voice group join 2"),
            Some(Join { target: "2".into(), password: None })
        );
        assert_eq!(parse_voice_command("voice group leave"), Some(Leave));
        assert_eq!(parse_voice_command("voice group create"), Some(Help));
        assert_eq!(
            parse_voice_command("voice volume Alex 150%"),
            Some(Volume { player: "Alex".into(), percent: Some(150) })
        );
        assert_eq!(parse_voice_command("voice volume Alex 999"), Some(Help));
        assert_eq!(parse_voice_command("voice volume Alex reset"), Some(VolumeReset("Alex".into())));
        assert_eq!(parse_voice_command("VOICE deafen"), Some(Deafen));
    }

    #[test]
    fn the_speaking_line_lists_up_to_three_names() {
        assert_eq!(speaking_line(&[]), None);
        assert_eq!(speaking_line(&["A".into()]).unwrap(), "Speaking: A");
        let many: Vec<String> = ["A", "B", "C", "D", "E"].iter().map(|s| (*s).into()).collect();
        assert_eq!(speaking_line(&many).unwrap(), "Speaking: A, B, C and 2 more");
    }
}
