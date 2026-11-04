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
    #[cfg(not(feature = "belumi-voice"))]
    warned: bool,
}

#[cfg_attr(not(feature = "belumi-voice"), allow(dead_code))]
pub(super) fn speaking_line(names: &[String]) -> Option<String> {
    match names.len() {
        0 => None,
        1..=3 => Some(format!("Speaking: {}", names.join(", "))),
        n => Some(format!("Speaking: {} and {} more", names[..3].join(", "), n - 3)),
    }
}

#[cfg(feature = "belumi-voice")]
mod real {
    use super::*;
    use belumi_voice::{
        AudioMode, COMPATIBILITY_VERSION, Listener, Mode, REQUEST_SECRET_CHANNEL, SECRET_CHANNEL,
        State, VoiceClient, VoiceSettings, parse_secret, request_secret_payload,
    };
    use horizon_protocol::packets::{configuration::CustomPayload, play::Serverbound};

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

        pub(in crate::game) fn belumi_voice_joined(&mut self) {
            if !self.settings.belumi.voice || self.belumi_maps.server.is_empty() {
                return;
            }
            if let Some(r) = self.remote.as_ref() {
                r.session.send(Serverbound::CustomPayload(CustomPayload {
                    channel: REQUEST_SECRET_CHANNEL.to_owned(),
                    data: request_secret_payload(COMPATIBILITY_VERSION),
                }));
            }
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
            let host = self.belumi_maps.server.clone();
            let settings = self.belumi_voice_settings();
            match VoiceClient::start(init, &host, AudioMode::Devices, settings) {
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
            let Some(client) = self.belumi_voice.client.as_ref() else {
                return;
            };
            client.set_settings(settings);
            let notes = client.take_notes();
            let state = client.state();
            for n in notes {
                self.native_chat(&format!("Voice chat: {n}"));
            }
            if self.belumi_voice.last_state.as_ref() != Some(&state) {
                match &state {
                    State::Connected => self.native_chat("Voice chat connected."),
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

        pub(in crate::game) fn belumi_voice_lines(&self) -> Vec<String> {
            let Some(c) = self.belumi_voice.client.as_ref() else {
                return Vec::new();
            };
            if !matches!(c.state(), State::Connected) {
                return Vec::new();
            }
            let mut lines = Vec::new();
            let tx = self.belumi_voice.held
                || (self.settings.belumi.voice_mode == 1 && c.mic_level_db() >= self.settings.belumi.voice_threshold_db());
            lines.push(if self.belumi_voice.muted {
                "Mic muted".to_owned()
            } else if tx {
                "Mic on".to_owned()
            } else {
                "Mic ready".to_owned()
            });
            let names: Vec<String> = c
                .speaking()
                .into_iter()
                .filter_map(|u| {
                    self.remote
                        .as_ref()
                        .and_then(|r| r.tab_players.get(&u).map(|p| p.name.clone()))
                })
                .collect();
            lines.extend(super::speaking_line(&names));
            lines
        }
    }
}

#[cfg(not(feature = "belumi-voice"))]
mod stub {
    use super::*;

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
        pub(in crate::game) fn belumi_voice_lines(&self) -> Vec<String> {
            Vec::new()
        }
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
