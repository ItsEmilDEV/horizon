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
    menu_scroll: usize,
    #[cfg(feature = "belumi-voice")]
    boxes: Option<(horizon_client::ui::EditBox, horizon_client::ui::EditBox)>,
    #[cfg(feature = "belumi-voice")]
    notice: String,
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
            if pressed && key == cfg.voice_menu_key && self.native_playing() && self.remote.is_some() {
                self.voice_open_screen();
                return;
            }
            if self.screen == crate::game::Screen::BelumiVoice {
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
                // While the client asks which protocol the server speaks, nothing shows: most servers have no
                // voice chat, and "connecting" would sit on the screen for them.
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
            if self.player.is_none() || self.remote.is_none() {
                return;
            }
            let Some((cam, bob)) = self.belumi_frame_camera(batch.width / batch.height.max(1.0)) else {
                return;
            };
            let me = self.voice_me();
            let mut marks: Vec<(f32, f32, &'static str, u32)> = Vec::new();
            for (uuid, pos) in self.belumi_player_spots() {
                if uuid == me {
                    continue;
                }
                let Some(badge) = self.belumi_voice.roster.players.get(&uuid).and_then(|s| s.badge()) else {
                    continue;
                };
                let Some((u, v)) = crate::game::belumi_maps::project_bobbed(&cam, bob, [pos[0], pos[1] + 2.75, pos[2]]) else {
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
            if self.player.is_none() || self.remote.is_none() {
                return;
            }
            let Some((cam, bob)) = self.belumi_frame_camera(batch.width / batch.height.max(1.0)) else {
                return;
            };
            let spots: Vec<(f32, f32)> = self
                .belumi_player_spots()
                .into_iter()
                .filter(|(u, _)| speaking.contains(u))
                .filter_map(|(_, pos)| crate::game::belumi_maps::project_bobbed(&cam, bob, [pos[0], pos[1] + 2.4, pos[2]]))
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

        // ---- the voice screen -------------------------------------------------------------

        const PANEL_W: f32 = 340.0;
        const ROW_H: f32 = 16.0;
        const PAD: f32 = 9.0;
        const NAME_LEN: usize = 24;

        const BG: u32 = 0xF012_1620;
        const EDGE: u32 = 0xFF2C_3447;
        const HEAD: u32 = 0xFF1A_2030;
        const ROW: u32 = 0x3000_0000;
        const HOVER: u32 = 0x40FF_FFFF;
        const ACCENT: u32 = 0xFF7B_4DFF;
        const ACCENT_HOVER: u32 = 0xFF95_6DFF;
        const BUTTON: u32 = 0xFF2A_3144;
        const BUTTON_HOVER: u32 = 0xFF3A_4360;
        const OK: u32 = 0xFF55_E07A;
        const WARN: u32 = 0xFFF2_C14E;
        const BAD: u32 = 0xFFF2_6B6B;

        #[derive(Clone)]
        enum Act {
            Mute,
            Deafen,
            Join(u128),
            AskPassword(String),
            Leave,
            Less(u128),
            More(u128),
            Reset(u128),
            Create,
            JoinByName,
        }

        enum Entry {
            Header(&'static str),
            Note(&'static str),
            Group { id: u128, name: String, members: usize, locked: bool, mine: bool },
            Player { uuid: u128, name: String, badge: &'static str, percent: u16 },
        }

        struct Frame {
            panel: [f32; 4],
            toggles: [[f32; 4]; 2],
            rows: Vec<(f32, Entry)>,
            list: [f32; 4],
            name: [f32; 4],
            pass: [f32; 4],
            create: [f32; 4],
            join: [f32; 4],
            total: usize,
            shown: usize,
            first: usize,
        }

        impl Game<'_> {
            fn voice_entries(&self) -> Vec<Entry> {
                let r = &self.belumi_voice.roster;
                let mut out = vec![Entry::Header("Groups")];
                let groups = r.listed_groups();
                if groups.is_empty() {
                    out.push(Entry::Note("No groups yet. Make one below."));
                }
                for g in groups {
                    out.push(Entry::Group {
                        id: g.id,
                        name: g.name.clone(),
                        members: r.members(g.id),
                        locked: g.has_password,
                        mine: r.my_group == Some(g.id),
                    });
                }
                out.push(Entry::Header("Players"));
                let me = self.voice_me();
                let mut players: Vec<(String, u128)> = self
                    .remote
                    .as_ref()
                    .map(|r| r.tab_players.iter().map(|(u, p)| (p.name.clone(), *u)).filter(|(_, u)| *u != me).collect())
                    .unwrap_or_default();
                players.sort_by_key(|(n, _)| n.to_lowercase());
                if players.is_empty() {
                    out.push(Entry::Note("Nobody else is here."));
                }
                for (name, uuid) in players {
                    let badge = match r.players.get(&uuid).and_then(|s| s.badge()) {
                        Some(Badge::Disconnected) => "no voice chat",
                        Some(Badge::FullMuted) => "deafened",
                        Some(Badge::Muted) => "muted",
                        None => "",
                    };
                    out.push(Entry::Player { uuid, name, badge, percent: self.voice_percent(uuid) });
                }
                out
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

            fn voice_frame_layout(&self, w: f32, h: f32) -> Frame {
                let panel_h = (h - 12.0).clamp(150.0, 250.0);
                let (x, y) = ((w - PANEL_W) / 2.0, ((h - panel_h) / 2.0).floor());
                let inner = PANEL_W - 2.0 * PAD;
                let half = (inner - 4.0) / 2.0;
                let toggles = [[x + PAD, y + 26.0, half, 16.0], [x + PAD + half + 4.0, y + 26.0, half, 16.0]];
                let list_top = y + 48.0;
                let form_top = y + panel_h - 46.0;
                let list = [x + 4.0, list_top, PANEL_W - 8.0, form_top - list_top - 2.0];
                let entries = self.voice_entries();
                let shown = ((list[3] / ROW_H).floor() as usize).max(1);
                let first = self.belumi_voice.menu_scroll.min(entries.len().saturating_sub(shown));
                let rows = entries
                    .into_iter()
                    .enumerate()
                    .skip(first)
                    .take(shown)
                    .map(|(i, e)| (list_top + (i - first) as f32 * ROW_H, e))
                    .collect::<Vec<_>>();
                let total = self.voice_entries().len();
                let fy = form_top + 14.0;
                Frame {
                    panel: [x, y, PANEL_W, panel_h],
                    toggles,
                    rows,
                    list,
                    name: [x + PAD, fy, 128.0, 16.0],
                    pass: [x + PAD + 132.0, fy, 84.0, 16.0],
                    create: [x + PAD + 220.0, fy, 54.0, 16.0],
                    join: [x + PAD + 278.0, fy, 44.0, 16.0],
                    total,
                    shown,
                    first,
                }
            }

            fn voice_targets(&self, f: &Frame) -> Vec<([f32; 4], Act)> {
                let mut out = vec![(f.toggles[0], Act::Mute), (f.toggles[1], Act::Deafen), (f.create, Act::Create), (f.join, Act::JoinByName)];
                let right = f.panel[0] + PANEL_W - PAD;
                for (y, e) in &f.rows {
                    match e {
                        Entry::Group { id, name, locked, mine, .. } => {
                            let act = if *mine {
                                Act::Leave
                            } else if *locked {
                                Act::AskPassword(name.clone())
                            } else {
                                Act::Join(*id)
                            };
                            out.push(([right - 44.0, y + 1.0, 44.0, ROW_H - 2.0], act));
                        }
                        Entry::Player { uuid, .. } => {
                            out.push(([right - 72.0, y + 1.0, 14.0, ROW_H - 2.0], Act::Less(*uuid)));
                            out.push(([right - 56.0, y + 1.0, 28.0, ROW_H - 2.0], Act::Reset(*uuid)));
                            out.push(([right - 26.0, y + 1.0, 14.0, ROW_H - 2.0], Act::More(*uuid)));
                        }
                        _ => {}
                    }
                }
                out
            }

            fn voice_open_screen(&mut self) {
                if !self.belumi_voice.boxes.is_some() {
                    let make = |max: usize| {
                        let mut b = horizon_client::ui::EditBox::new(0.0, 0.0, 0.0, 0.0);
                        b.max_length = max;
                        b.focused = false;
                        b
                    };
                    self.belumi_voice.boxes = Some((make(NAME_LEN), make(NAME_LEN)));
                }
                self.belumi_voice.menu_scroll = 0;
                self.belumi_voice.notice.clear();
                self.screen = crate::game::Screen::BelumiVoice;
                self.keys.clear();
                self.left = false;
                self.right = false;
                self.capture(false);
            }

            pub(in crate::game) fn belumi_voice_close_screen(&mut self) {
                self.voice_close_screen();
            }

            fn voice_close_screen(&mut self) {
                if self.screen == crate::game::Screen::BelumiVoice {
                    self.screen = crate::game::Screen::Playing;
                    self.capture(true);
                }
            }

            fn voice_submit(&mut self, create: bool) {
                let Some((name, pass)) = self.belumi_voice.boxes.as_ref().map(|(n, p)| (n.value.trim().to_owned(), p.value.clone())) else {
                    return;
                };
                let password = (!pass.is_empty()).then_some(pass);
                if name.is_empty() {
                    self.belumi_voice.notice = "Type a group name first.".into();
                    return;
                }
                if create {
                    self.voice_send(groups::CREATE_GROUP, groups::create_group(&name, password.as_deref(), 0));
                    self.belumi_voice.notice = format!("Created {name}.");
                } else {
                    match self.belumi_voice.roster.find_group(&name).map(|g| (g.id, g.name.clone(), g.has_password)) {
                        Some((id, found, locked)) => {
                            if locked && password.is_none() {
                                self.belumi_voice.notice = format!("{found} has a password: type it next to the name.");
                                if let Some((n, p)) = self.belumi_voice.boxes.as_mut() {
                                    n.focused = false;
                                    p.focused = true;
                                }
                                return;
                            }
                            self.voice_send(groups::SET_GROUP, groups::join_group(id, password.as_deref()));
                            self.belumi_voice.notice = format!("Joining {found}.");
                        }
                        None => {
                            self.belumi_voice.notice = format!("No group matches {name}.");
                            return;
                        }
                    }
                }
                if let Some((n, p)) = self.belumi_voice.boxes.as_mut() {
                    n.set_value("");
                    p.set_value("");
                    n.focused = false;
                    p.focused = false;
                }
            }

            fn voice_do(&mut self, act: Act) {
                match act {
                    Act::Mute => self.belumi_voice.muted = !self.belumi_voice.muted,
                    Act::Deafen => self.voice_toggle_deafen_quiet(),
                    Act::Join(id) => self.voice_send(groups::SET_GROUP, groups::join_group(id, None)),
                    Act::AskPassword(name) => {
                        if let Some((n, p)) = self.belumi_voice.boxes.as_mut() {
                            n.set_value(&name);
                            n.focused = false;
                            p.focused = true;
                        }
                        self.belumi_voice.notice = format!("Type the password for {name}, then press Join.");
                    }
                    Act::Leave => self.voice_send(groups::LEAVE_GROUP, groups::leave_group()),
                    Act::Less(uuid) => {
                        let next = self.voice_percent(uuid).saturating_sub(10);
                        self.voice_set_percent(uuid, next);
                    }
                    Act::More(uuid) => {
                        let next = (self.voice_percent(uuid) + 10).min(300);
                        self.voice_set_percent(uuid, next);
                    }
                    Act::Reset(uuid) => self.voice_set_percent(uuid, 100),
                    Act::Create => self.voice_submit(true),
                    Act::JoinByName => self.voice_submit(false),
                }
            }

            fn voice_toggle_deafen_quiet(&mut self) {
                self.belumi_voice.deafened = !self.belumi_voice.deafened;
            }

            pub(in crate::game) fn belumi_voice_click(&mut self) {
                let (w, h) = self.gui_size();
                let frame = self.voice_frame_layout(w, h);
                let [px, py, pw, ph] = frame.panel;
                let cursor = self.cursor;
                if !horizon_client::ui::hovered(cursor, px, py, pw, ph) {
                    self.voice_close_screen();
                    return;
                }
                if let Some((_, act)) = self
                    .voice_targets(&frame)
                    .into_iter()
                    .find(|(r, _)| horizon_client::ui::hovered(cursor, r[0], r[1], r[2], r[3]))
                {
                    self.audio.play_button_click();
                    self.voice_do(act);
                    return;
                }
                let shift = self.has_shift_down();
                if let Some((n, p)) = self.belumi_voice.boxes.take() {
                    let (mut n, mut p) = (n, p);
                    let over = |r: [f32; 4]| horizon_client::ui::hovered(cursor, r[0], r[1], r[2], r[3]);
                    n.focused = over(frame.name);
                    p.focused = over(frame.pass);
                    if n.focused {
                        n.click(&self.gui.atlas, cursor.0, shift, false);
                    }
                    if p.focused {
                        p.click(&self.gui.atlas, cursor.0, shift, false);
                    }
                    self.belumi_voice.boxes = Some((n, p));
                }
            }

            pub(in crate::game) fn belumi_voice_scroll(&mut self, steps: f64) {
                let total = self.voice_entries().len();
                let (w, h) = self.gui_size();
                let shown = self.voice_frame_layout(w, h).shown;
                let max = total.saturating_sub(shown);
                let now = self.belumi_voice.menu_scroll as i64 - steps.signum() as i64;
                self.belumi_voice.menu_scroll = now.clamp(0, max as i64) as usize;
            }

            pub(in crate::game) fn belumi_voice_screen_key(&mut self, event: &winit::event::KeyEvent) {
                use winit::{event::ElementState, keyboard::{KeyCode, PhysicalKey}};
                if event.state != ElementState::Pressed {
                    return;
                }
                let PhysicalKey::Code(key) = event.physical_key else {
                    return;
                };
                let focused = self.belumi_voice.boxes.as_ref().is_some_and(|(n, p)| n.focused || p.focused);
                match key {
                    KeyCode::Escape => {
                        self.voice_close_screen();
                        return;
                    }
                    KeyCode::Enter | KeyCode::NumpadEnter => {
                        let create = self.belumi_voice.boxes.as_ref().is_some_and(|(n, _)| {
                            self.belumi_voice.roster.find_group(n.value.trim()).is_none()
                        });
                        if focused {
                            self.voice_submit(create);
                        }
                        return;
                    }
                    KeyCode::Tab => {
                        if let Some((n, p)) = self.belumi_voice.boxes.as_mut() {
                            let to_pass = n.focused;
                            n.focused = !to_pass;
                            p.focused = to_pass;
                        }
                        return;
                    }
                    _ => {}
                }
                if !focused {
                    if horizon_client::keybinds::key_name(key).is_some_and(|k| k == self.settings.belumi.voice_menu_key) {
                        self.voice_close_screen();
                    }
                    return;
                }
                let shortcut = self.edit_shortcut(event);
                let (ctrl, shift) = (self.has_control_down(), self.has_shift_down());
                let typed = event.text.as_deref().map(str::to_owned);
                let Some((mut n, mut p)) = self.belumi_voice.boxes.take() else {
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
                self.belumi_voice.boxes = Some((n, p));
            }

            fn voice_text_button(&self, batch: &mut horizon_client::hud::HudBatch, r: [f32; 4], label: &str, color: u32, hover_color: u32) {
                let over = horizon_client::ui::hovered(self.cursor, r[0], r[1], r[2], r[3]);
                self.native_fill(batch, r, if over { hover_color } else { color });
                let tw = self.native_text_width(label);
                self.native_text(batch, label, [r[0] + (r[2] - tw) / 2.0, r[1] + ((r[3] - 8.0) / 2.0).floor()], 0x00FF_FFFF, true);
            }

            pub(in crate::game) fn belumi_voice_draw_screen(&mut self, batch: &mut horizon_client::hud::HudBatch) {
                if !self.settings.belumi.voice || self.remote.is_none() {
                    self.voice_close_screen();
                    return;
                }
                let frame = self.voice_frame_layout(batch.width, batch.height);
                let [x, y, w, h] = frame.panel;
                self.native_fill(batch, [0.0, 0.0, batch.width, batch.height], 0x9000_0000);
                self.native_fill(batch, [x - 1.0, y - 1.0, w + 2.0, h + 2.0], EDGE);
                self.native_fill(batch, [x, y, w, h], BG);
                self.native_fill(batch, [x, y, w, 22.0], HEAD);
                self.native_fill(batch, [x, y + 22.0, w, 1.0], EDGE);
                self.native_fill(batch, [x, y, 3.0, 22.0], ACCENT);
                self.native_text(batch, "Voice chat", [x + 10.0, y + 7.0], 0x00FF_FFFF, true);
                let (dot, status) = match self.belumi_voice.client.as_ref().map(|c| c.state()) {
                    Some(State::Connected) => (OK, "Connected"),
                    Some(State::Failed(_)) => (BAD, "Connection lost"),
                    _ => (WARN, "Connecting"),
                };
                let sw = self.native_text_width(status);
                self.native_fill(batch, [x + w - sw - 22.0, y + 8.0, 6.0, 6.0], dot);
                self.native_text(batch, status, [x + w - sw - PAD, y + 7.0], 0x00C8_D0E0, true);

                let muted = self.belumi_voice.muted;
                let deaf = self.belumi_voice.deafened;
                self.voice_text_button(batch, frame.toggles[0], if muted { "Microphone: muted" } else { "Microphone: on" }, if muted { 0xFF8A_3030 } else { BUTTON }, if muted { 0xFFA8_4040 } else { BUTTON_HOVER });
                self.voice_text_button(batch, frame.toggles[1], if deaf { "Sound: deafened" } else { "Sound: on" }, if deaf { 0xFF8A_3030 } else { BUTTON }, if deaf { 0xFFA8_4040 } else { BUTTON_HOVER });

                let right = x + w - PAD;
                for (ry, entry) in &frame.rows {
                    let ry = *ry;
                    let over = horizon_client::ui::hovered(self.cursor, frame.list[0], ry, frame.list[2], ROW_H);
                    match entry {
                        Entry::Header(text) => {
                            self.native_text(batch, text, [x + PAD, ry + 4.0], 0x00F2_C14E, true);
                            self.native_fill(batch, [x + PAD + self.native_text_width(text) + 5.0, ry + 8.0, w - 2.0 * PAD - self.native_text_width(text) - 5.0, 1.0], EDGE);
                        }
                        Entry::Note(text) => self.native_text(batch, text, [x + PAD + 4.0, ry + 4.0], 0x008A_93A8, false),
                        Entry::Group { name, members, locked, mine, .. } => {
                            self.native_fill(batch, [frame.list[0], ry + 1.0, frame.list[2], ROW_H - 2.0], if over { HOVER } else { ROW });
                            let label = format!("{name}  ({members})");
                            self.native_text(batch, &label, [x + PAD + 2.0, ry + 4.0], if *mine { 0x0055_E07A } else { 0x00FF_FFFF }, true);
                            if *locked {
                                let lw = self.native_text_width(&label);
                                self.native_text(batch, "[password]", [x + PAD + 6.0 + lw, ry + 4.0], 0x008A_93A8, false);
                            }
                            let (text, color, hover) = if *mine { ("Leave", 0xFF8A_3030, 0xFFA8_4040) } else { ("Join", ACCENT, ACCENT_HOVER) };
                            self.voice_text_button(batch, [right - 44.0, ry + 1.0, 44.0, ROW_H - 2.0], text, color, hover);
                        }
                        Entry::Player { name, badge, percent, .. } => {
                            self.native_fill(batch, [frame.list[0], ry + 1.0, frame.list[2], ROW_H - 2.0], if over { HOVER } else { ROW });
                            self.native_text(batch, name, [x + PAD + 2.0, ry + 4.0], 0x00FF_FFFF, true);
                            if !badge.is_empty() {
                                let nw = self.native_text_width(name);
                                self.native_text(batch, badge, [x + PAD + 8.0 + nw, ry + 4.0], 0x00F2_6B6B, false);
                            }
                            self.voice_text_button(batch, [right - 72.0, ry + 1.0, 14.0, ROW_H - 2.0], "-", BUTTON, BUTTON_HOVER);
                            self.voice_text_button(batch, [right - 56.0, ry + 1.0, 28.0, ROW_H - 2.0], &format!("{percent}%"), if *percent == 100 { BUTTON } else { 0xFF3A_2F66 }, BUTTON_HOVER);
                            self.voice_text_button(batch, [right - 26.0, ry + 1.0, 14.0, ROW_H - 2.0], "+", BUTTON, BUTTON_HOVER);
                        }
                    }
                }
                if frame.first > 0 {
                    self.native_text(batch, "^", [x + w - 12.0, frame.list[1] - 8.0], 0x008A_93A8, false);
                }
                if frame.first + frame.shown < frame.total {
                    self.native_text(batch, "v", [x + w - 12.0, frame.list[1] + frame.list[3] - 6.0], 0x008A_93A8, false);
                }

                let form_y = frame.name[1] - 14.0;
                self.native_fill(batch, [x + PAD, form_y - 2.0, w - 2.0 * PAD, 1.0], EDGE);
                self.native_text(batch, "Create or join a group", [x + PAD, form_y + 1.0], 0x00F2_C14E, true);
                let atlas_boxes = self.belumi_voice.boxes.take();
                if let Some((mut n, mut p)) = atlas_boxes {
                    [n.x, n.y, n.width, n.height] = frame.name;
                    [p.x, p.y, p.width, p.height] = frame.pass;
                    n.draw(batch, &self.gui.atlas, None);
                    p.draw(batch, &self.gui.atlas, None);
                    if n.value.is_empty() && !n.focused {
                        self.native_text(batch, "Group name", [n.text_x(), n.text_y()], 0x0070_7888, false);
                    }
                    if p.value.is_empty() && !p.focused {
                        self.native_text(batch, "Password", [p.text_x(), p.text_y()], 0x0070_7888, false);
                    }
                    self.belumi_voice.boxes = Some((n, p));
                }
                self.voice_text_button(batch, frame.create, "Create", ACCENT, ACCENT_HOVER);
                self.voice_text_button(batch, frame.join, "Join", BUTTON, BUTTON_HOVER);
                let hint = if self.belumi_voice.notice.is_empty() {
                    "Wheel: scroll   Esc: close".to_owned()
                } else {
                    self.belumi_voice.notice.clone()
                };
                self.native_text(batch, &hint, [x + PAD, y + h - 11.0], 0x008A_93A8, false);
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
        pub(in crate::game) fn belumi_voice_screen_key(&mut self, _event: &winit::event::KeyEvent) {}
        pub(in crate::game) fn belumi_voice_click(&mut self) {}
        pub(in crate::game) fn belumi_voice_close_screen(&mut self) {}
        pub(in crate::game) fn belumi_voice_scroll(&mut self, _steps: f64) {}
        pub(in crate::game) fn belumi_voice_draw_screen(&mut self, _batch: &mut horizon_client::hud::HudBatch) {}
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
