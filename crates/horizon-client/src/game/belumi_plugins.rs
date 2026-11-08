use super::Game;
use horizon_client::extras::Corner;

#[derive(Default)]
pub(super) struct PluginState {
    #[cfg(feature = "belumi-plugins")]
    host: Option<belumi_client_plugins::Host>,
    ticks: u64,
    warned: bool,
}

/// A plugin's screen as the client screen draws it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ScreenView {
    pub(super) plugin: String,
    pub(super) title: String,
    pub(super) rows: Vec<RowView>,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(feature = "belumi-plugins"), allow(dead_code))]
pub(super) enum RowView {
    Label(String),
    Button { id: String, text: String },
    Toggle { id: String, text: String, on: bool },
}

#[cfg_attr(not(feature = "belumi-plugins"), allow(dead_code))]
pub(super) fn player_json(
    at: [f64; 3],
    yaw: f32,
    pitch: f32,
    dimension: &str,
    fps: u32,
) -> String {
    format!(
        "{{\"x\":{:.3},\"y\":{:.3},\"z\":{:.3},\"yaw\":{:.2},\"pitch\":{:.2},\"dimension\":{},\"fps\":{fps}}}",
        at[0],
        at[1],
        at[2],
        yaw,
        pitch,
        serde_json::to_string(dimension).unwrap_or_else(|_| "\"\"".into()),
    )
}

#[cfg_attr(not(feature = "belumi-plugins"), allow(dead_code))]
pub(super) fn split_command(text: &str) -> (&str, &str) {
    let text = text.trim();
    match text.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (text, ""),
    }
}

#[cfg(feature = "belumi-plugins")]
mod real {
    use super::*;
    use belumi_client_plugins::Host;

    fn dir() -> std::path::PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join(horizon_client::belumi::PLUGINS_DIR)
    }

    impl Game<'_> {
        pub(in crate::game) fn belumi_plugins_active(&self) -> bool {
            self.belumi_plugins.host.is_some()
        }

        fn belumi_plugins_load(&mut self) {
            let mut host = match Host::new() {
                Ok(h) => h,
                Err(e) => {
                    self.native_chat(&format!("Plugins unavailable: {e}"));
                    self.belumi_plugins.warned = true;
                    return;
                }
            };
            host.load_dir(&dir());
            for e in host.take_errors() {
                self.native_chat(&format!("Plugin error: {e}"));
            }
            self.belumi_plugins.host = Some(host);
        }

        pub(in crate::game) fn belumi_plugins_frame(&mut self) {
            let wanted = self.settings.belumi.plugins && self.native_window();
            if wanted && self.belumi_plugins.host.is_none() && !self.belumi_plugins.warned {
                self.belumi_plugins_load();
            }
            if !wanted && let Some(mut host) = self.belumi_plugins.host.take() {
                host.shutdown();
                self.belumi_plugins.warned = false;
            }
            let Some(host) = self.belumi_plugins.host.as_mut() else {
                return;
            };
            let lines = host.shared().take_chat();
            let errors = host.take_errors();
            let (open, close) = host.shared().take_open().map_or((None, host.shared().take_close()), |o| (Some(o), false));
            for l in lines {
                self.native_chat(&l);
            }
            for e in errors {
                self.native_chat(&format!("Plugin error: {e}"));
            }
            if let Some(plugin) = open {
                self.client_open_plugin(Some(plugin));
            }
            if close {
                self.client_close_plugin();
            }
        }

        pub(in crate::game) fn belumi_plugin_screens(&self) -> Vec<ScreenView> {
            use belumi_client_plugins::ScreenRow;
            let Some(host) = self.belumi_plugins.host.as_ref() else { return Vec::new() };
            host.shared()
                .screens()
                .into_iter()
                .map(|(plugin, spec)| ScreenView {
                    plugin,
                    title: spec.title,
                    rows: spec
                        .rows
                        .into_iter()
                        .map(|r| match r {
                            ScreenRow::Label { text } => RowView::Label(text),
                            ScreenRow::Button { id, text } => RowView::Button { id, text },
                            ScreenRow::Toggle { id, text, on } => RowView::Toggle { id, text, on },
                        })
                        .collect(),
                })
                .collect()
        }

        pub(in crate::game) fn belumi_plugin_press(&mut self, plugin: &str, id: &str, value: bool) {
            if let Some(host) = self.belumi_plugins.host.as_mut() {
                host.screen_event(plugin, id, value);
            }
        }

        pub(in crate::game) fn belumi_plugins_tick(&mut self) {
            if self.belumi_plugins.host.is_none() {
                return;
            }
            self.belumi_plugins.ticks += 1;
            let ticks = self.belumi_plugins.ticks;
            let fps = self.native_frame_rate().0;
            let json = self.native_player().map(|p| player_json([p.x, p.y, p.z], p.yaw, p.pitch, p.dimension, fps));
            if let Some(host) = self.belumi_plugins.host.as_mut() {
                if let Some(json) = json {
                    host.shared().set_player(json);
                }
                host.dispatch(&belumi_client_plugins::Event::Tick { tick: ticks });
            }
        }

        pub(in crate::game) fn belumi_plugins_event(&mut self, event: belumi_client_plugins::Event) {
            if let Some(host) = self.belumi_plugins.host.as_mut() {
                host.dispatch(&event);
            }
        }

        pub(in crate::game) fn belumi_plugins_key(&mut self, key: &str, pressed: bool) {
            if self.belumi_plugins.host.is_some() {
                self.belumi_plugins_event(belumi_client_plugins::Event::Key {
                    key: key.to_owned(),
                    pressed,
                });
            }
        }

        pub(in crate::game) fn belumi_plugins_chat(&mut self, text: &str) {
            if self.belumi_plugins.host.is_some() {
                self.belumi_plugins_event(belumi_client_plugins::Event::Chat {
                    text: text.to_owned(),
                });
            }
        }

        pub(in crate::game) fn belumi_plugins_join(&mut self, address: &str) {
            self.belumi_plugins_event(belumi_client_plugins::Event::Join {
                address: address.to_owned(),
            });
        }

        pub(in crate::game) fn belumi_plugins_leave(&mut self) {
            self.belumi_plugins_event(belumi_client_plugins::Event::Leave);
        }

        pub(in crate::game) fn belumi_plugins_command(&mut self, command: &str, ran: &mut bool) {
            if *ran || !self.settings.belumi.plugins {
                return;
            }
            let (name, args) = split_command(command);
            if name == "plugins" {
                *ran = true;
                let lines: Vec<String> = match self.belumi_plugins.host.as_ref() {
                    None => vec!["Plugins are not running. Turn them on in Belumi Settings.".into()],
                    Some(h) if h.info().is_empty() => {
                        vec![format!("No plugins in {}/.", horizon_client::belumi::PLUGINS_DIR)]
                    }
                    Some(h) => h
                        .info()
                        .iter()
                        .map(|p| {
                            let state = if p.disabled { " (disabled after an error)" } else { "" };
                            format!("{} {}{state}", p.name, p.version)
                        })
                        .collect(),
                };
                for l in lines {
                    self.native_chat(&l);
                }
                return;
            }
            let handled = self
                .belumi_plugins
                .host
                .as_mut()
                .is_some_and(|h| h.run_command(name, args));
            if handled {
                *ran = true;
            }
        }

        pub(in crate::game) fn belumi_plugin_lines(&self) -> Vec<String> {
            self.belumi_plugins
                .host
                .as_ref()
                .map(|h| h.shared().hud_lines())
                .unwrap_or_default()
        }
    }
}

#[cfg(not(feature = "belumi-plugins"))]
mod stub {
    use super::*;

    impl Game<'_> {
        pub(in crate::game) fn belumi_plugins_active(&self) -> bool {
            false
        }
        pub(in crate::game) fn belumi_plugins_frame(&mut self) {
            if self.settings.belumi.plugins && !self.belumi_plugins.warned && self.native_window() {
                self.belumi_plugins.warned = true;
                self.native_chat("This build has no plugin support (build with --features belumi-plugins).");
            }
            if !self.settings.belumi.plugins {
                self.belumi_plugins.warned = false;
            }
        }
        pub(in crate::game) fn belumi_plugins_tick(&mut self) {
            let _ = self.belumi_plugins.ticks;
        }
        pub(in crate::game) fn belumi_plugins_key(&mut self, _key: &str, _pressed: bool) {}
        pub(in crate::game) fn belumi_plugins_chat(&mut self, _text: &str) {}
        pub(in crate::game) fn belumi_plugins_join(&mut self, _address: &str) {}
        pub(in crate::game) fn belumi_plugins_leave(&mut self) {}
        pub(in crate::game) fn belumi_plugins_command(&mut self, _command: &str, _ran: &mut bool) {}
        pub(in crate::game) fn belumi_plugin_lines(&self) -> Vec<String> {
            Vec::new()
        }
        pub(in crate::game) fn belumi_plugin_screens(&self) -> Vec<ScreenView> {
            Vec::new()
        }
        pub(in crate::game) fn belumi_plugin_press(&mut self, _plugin: &str, _id: &str, _value: bool) {}
    }
}

impl Game<'_> {
    pub(super) fn belumi_plugin_corner(&self) -> Corner {
        self.settings.belumi.plugin_corner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_json_is_valid_json_with_the_documented_fields() {
        let json = player_json([1.5, 64.0, -3.25], 90.0, -10.0, "minecraft:overworld", 120);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["x"], 1.5);
        assert_eq!(v["dimension"], "minecraft:overworld");
        assert_eq!(v["fps"], 120);
    }

    #[test]
    fn commands_split_into_name_and_arguments() {
        assert_eq!(split_command("hello big world"), ("hello", "big world"));
        assert_eq!(split_command("  plugins  "), ("plugins", ""));
        assert_eq!(split_command(""), ("", ""));
    }
}
