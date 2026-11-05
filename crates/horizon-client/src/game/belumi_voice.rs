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
    connecting_for: f32,
    #[cfg(feature = "belumi-voice")]
    target: String,
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

#[cfg(feature = "belumi-voice")]
pub(super) use real::register_devices;
#[cfg(not(feature = "belumi-voice"))]
pub(super) use stub::register_devices;

#[cfg(feature = "belumi-voice")]
mod real {
    use super::*;
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
            channel == SECRET_CHANNEL && self.settings.belumi.voice
        }

        fn belumi_voice_settings(&self) -> VoiceSettings {
            let c = &self.settings.belumi;
            VoiceSettings {
                mode: if c.voice_mode == 1 { Mode::Activation } else { Mode::PushToTalk },
                threshold_db: c.voice_threshold_db(),
                mic_gain: c.voice_mic_gain(),
                volume: c.voice_volume_gain(),
                muted: self.belumi_voice.muted,
                deafened: false,
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

        pub(in crate::game) fn belumi_voice_payload(&mut self, data: &[u8]) {
            let init = match parse_secret(data) {
                Ok(i) => i,
                Err(e) => {
                    self.native_chat(&format!("Voice chat: unreadable server data ({e})."));
                    return;
                }
            };
            self.belumi_voice.client = None;
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
            match VoiceClient::start(init, &host, AudioMode::Devices, settings, microphone) {
                Ok(c) => {
                    self.belumi_voice.last_state = None;
                    self.belumi_voice.client = Some(c);
                }
                Err(e) => self.native_chat(&format!("Voice chat: {e}")),
            }
        }

        pub(in crate::game) fn belumi_voice_left(&mut self) {
            self.belumi_voice.client = None;
            self.belumi_voice.last_state = None;
            self.belumi_voice.held = false;
            self.belumi_voice.probe_wait = -1.0;
        }

        pub(in crate::game) fn belumi_voice_key(&mut self, key: &str, pressed: bool) {
            let cfg = &self.settings.belumi;
            if !cfg.voice {
                return;
            }
            if key == cfg.voice_key {
                let on = pressed && (self.native_playing() || self.belumi_voice.held);
                self.belumi_voice.held = on;
                if let Some(c) = self.belumi_voice.client.as_ref() {
                    c.set_talking(on);
                }
            } else if pressed && key == cfg.voice_mute_key && self.native_playing() {
                self.belumi_voice.muted = !self.belumi_voice.muted;
                let text = if self.belumi_voice.muted { "Microphone muted." } else { "Microphone on." };
                self.native_chat(text);
            }
        }

        pub(in crate::game) fn belumi_voice_frame(&mut self, dt: f32) {
            if !self.settings.belumi.voice {
                if self.belumi_voice.client.is_some() {
                    self.belumi_voice_left();
                }
                return;
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
                if let Some(c) = self.belumi_voice.client.as_ref() {
                    c.update_world(listener, players);
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
            let bar_width = 64.0;
            let mut width = self.native_text_width(&title) + 12.0;
            for name in &speakers {
                width = width.max(self.native_text_width(name) + 12.0);
            }
            width = width.max(bar_width + 8.0);
            let height = 12.0 + if level.is_some() { 8.0 } else { 0.0 } + speakers.len() as f32 * 10.0;
            let (x, y) = corners.place(corner, width, height);
            self.native_fill(batch, [x, y, width, height], 0x6000_0000);
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
            for name in &speakers {
                self.native_fill(batch, [x + 4.0, row + 2.0, 3.0, 3.0], CYAN);
                self.native_text(batch, name, [x + 10.0, row], CYAN & 0x00FF_FFFF, true);
                row += 10.0;
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
        pub(in crate::game) fn belumi_voice_payload(&mut self, _data: &[u8]) {}
        pub(in crate::game) fn belumi_voice_joined(&mut self) {
            if self.settings.belumi.voice && !self.belumi_voice.warned {
                self.belumi_voice.warned = true;
                self.native_chat("This build has no voice chat support (build with --features belumi-voice).");
            }
        }
        pub(in crate::game) fn belumi_voice_left(&mut self) {}
        pub(in crate::game) fn belumi_voice_key(&mut self, _key: &str, _pressed: bool) {}
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
    fn the_speaking_line_lists_up_to_three_names() {
        assert_eq!(speaking_line(&[]), None);
        assert_eq!(speaking_line(&["A".into()]).unwrap(), "Speaking: A");
        let many: Vec<String> = ["A", "B", "C", "D", "E"].iter().map(|s| (*s).into()).collect();
        assert_eq!(speaking_line(&many).unwrap(), "Speaking: A, B, C and 2 more");
    }
}
