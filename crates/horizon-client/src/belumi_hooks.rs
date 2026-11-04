pub enum BelumiEvent<'a> {
    ServerStart,
    ServerStop,
    Tick(u64),
    PlayerJoin { uuid: u128, name: &'a str },
    PlayerQuit { uuid: u128, name: &'a str },
    Chat { uuid: u128, message: &'a str },
    Command { uuid: u128, command: &'a str },
    BlockBreak { uuid: u128, dimension: &'a str, pos: [i32; 3] },
    PlayerDeath { uuid: u128, dimension: &'a str, pos: [i32; 3] },
    UseBlock { uuid: u128, dimension: &'a str, pos: [i32; 3], face: u8, off_hand: bool },
}

pub enum BelumiAction {
    Broadcast(String),
    Message { uuid: u128, text: String },
    Kick { uuid: u128, reason: String },
}

#[cfg(feature = "belumi")]
mod host {
    use super::BelumiEvent;
    use horizon_protocol::crypto::uuid_string;
    use belumi_plugin_api::{Event, EventResult};
    use belumi_server::PluginManager;
    use std::{path::Path, sync::Mutex};

    static MANAGER: Mutex<Option<PluginManager>> = Mutex::new(None);

    fn parse_uuid(s: &str) -> Option<u128> {
        let hex: String = s.chars().filter(|c| *c != '-').collect();
        u128::from_str_radix(&hex, 16).ok()
    }

    pub fn set_roster(players: &[(u128, &str)]) {
        if let Some(m) = MANAGER.lock().unwrap().as_mut() {
            let list: Vec<(String, String)> = players
                .iter()
                .map(|(u, n)| (uuid_string(*u), (*n).to_owned()))
                .collect();
            m.set_roster(&list);
        }
    }

    pub fn sync() -> Vec<super::BelumiAction> {
        let mut guard = MANAGER.lock().unwrap();
        let Some(m) = guard.as_mut() else {
            return Vec::new();
        };
        m.sync()
            .into_iter()
            .filter_map(|a| {
                Some(match a {
                    belumi_server::Action::Broadcast(t) => super::BelumiAction::Broadcast(t),
                    belumi_server::Action::Message { uuid, text } => super::BelumiAction::Message { uuid: parse_uuid(&uuid)?, text },
                    belumi_server::Action::Kick { uuid, reason } => super::BelumiAction::Kick { uuid: parse_uuid(&uuid)?, reason },
                })
            })
            .collect()
    }

    pub fn init_plugins(dir: &Path) {
        if !dir.is_dir() {
            return;
        }
        match PluginManager::new() {
            Ok(mut m) => {
                match m.load_dir(dir) {
                    Ok(n) => log::info!("[belumi] {n} plugin(s) loaded from {}", dir.display()),
                    Err(e) => log::error!("[belumi] plugins: {e}"),
                }
                *MANAGER.lock().unwrap() = Some(m);
            }
            Err(e) => log::error!("[belumi] plugin host: {e}"),
        }
    }

    pub fn command_tree() -> serde_json::Value {
        let guard = MANAGER.lock().unwrap();
        let names = guard.as_ref().map(|m| m.command_names()).unwrap_or_default();
        if names.is_empty() {
            return serde_json::Value::Null;
        }
        let children: serde_json::Map<String, serde_json::Value> = names
            .into_iter()
            .map(|n| {
                (
                    n,
                    serde_json::json!({
                        "type": "literal",
                        "executable": true,
                        "children": {"args": {
                            "type": "argument",
                            "parser": "brigadier:string",
                            "properties": {"type": "greedy"},
                            "executable": true
                        }}
                    }),
                )
            })
            .collect();
        serde_json::json!({"type": "root", "children": children})
    }

    /// `permission` is the player's operator level (0-4).
    pub(super) fn run_plugin_command(uuid: u128, permission: u8, line: &str) -> bool {
        use belumi_server::CommandOutcome;
        let mut guard = MANAGER.lock().unwrap();
        let Some(m) = guard.as_mut() else {
            return false;
        };
        let id = uuid_string(uuid);
        if line.trim().trim_start_matches('/').eq_ignore_ascii_case("belumi reload") {
            if permission < 4 {
                m.notify(&id, "You do not have permission to use this command.");
            } else {
                match m.reload() {
                    Ok(n) => {
                        log::info!("[belumi] reloaded, {n} plugin(s) loaded");
                        m.notify(&id, &format!("Reloaded {n} plugin(s)."));
                    }
                    Err(e) => {
                        log::error!("[belumi] reload failed: {e}");
                        m.notify(&id, "Reload failed, see the server log.");
                    }
                }
            }
            return true;
        }
        m.try_command(&id, permission, line) != CommandOutcome::NotPlugin
    }

    pub fn emit(event: BelumiEvent<'_>) -> bool {
        let mut guard = MANAGER.lock().unwrap();
        let Some(m) = guard.as_mut() else {
            return false;
        };
        let e = match event {
            BelumiEvent::ServerStart => Event::ServerStart,
            BelumiEvent::ServerStop => Event::ServerStop,
            BelumiEvent::Tick(tick) => Event::Tick { tick },
            BelumiEvent::PlayerJoin { uuid, name } => Event::PlayerJoin {
                uuid: uuid_string(uuid),
                name: name.to_owned(),
            },
            BelumiEvent::PlayerQuit { uuid, name } => Event::PlayerQuit {
                uuid: uuid_string(uuid),
                name: name.to_owned(),
            },
            BelumiEvent::Chat { uuid, message } => Event::Chat {
                uuid: uuid_string(uuid),
                message: message.to_owned(),
            },
            BelumiEvent::Command { uuid, command } => Event::Command {
                uuid: uuid_string(uuid),
                command: command.to_owned(),
            },
            BelumiEvent::BlockBreak { uuid, dimension, pos } => Event::BlockBreak {
                uuid: uuid_string(uuid),
                dimension: dimension.to_owned(),
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            BelumiEvent::PlayerDeath { uuid, dimension, pos } => Event::PlayerDeath {
                uuid: uuid_string(uuid),
                dimension: dimension.to_owned(),
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            BelumiEvent::UseBlock { uuid, dimension, pos, face, off_hand } => Event::UseBlock {
                uuid: uuid_string(uuid),
                dimension: dimension.to_owned(),
                x: pos[0],
                y: pos[1],
                z: pos[2],
                face,
                off_hand,
            },
        };
        m.emit(&e) == EventResult::Cancel
    }
}

#[cfg(not(feature = "belumi"))]
mod host {
    use super::BelumiEvent;
    use std::path::Path;

    pub fn init_plugins(_dir: &Path) {}

    #[inline(always)]
    pub(super) fn run_plugin_command(_uuid: u128, _permission: u8, _line: &str) -> bool {
        false
    }

    pub fn command_tree() -> serde_json::Value {
        serde_json::Value::Null
    }

    #[inline(always)]
    pub fn set_roster(_players: &[(u128, &str)]) {}

    #[inline(always)]
    pub fn sync() -> Vec<super::BelumiAction> {
        Vec::new()
    }

    #[inline(always)]
    pub fn emit(_event: BelumiEvent<'_>) -> bool {
        false
    }
}

pub use host::{command_tree, emit, init_plugins, set_roster, sync};

pub fn command(uuid: u128, permission: u8, line: &str) -> bool {
    host::run_plugin_command(uuid, permission, line) || emit(BelumiEvent::Command { uuid, command: line })
}

pub fn merge_commands(base: &serde_json::Value, plugins: serde_json::Value) -> serde_json::Value {
    let Some(add) = plugins["children"].as_object() else {
        return base.clone();
    };
    let mut out = if base.is_object() { base.clone() } else { serde_json::json!({"type": "root"}) };
    let children = out
        .as_object_mut()
        .expect("object")
        .entry("children")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(c) = children.as_object_mut() {
        c.extend(add.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::merge_commands;
    use serde_json::{Value, json};

    #[test]
    fn plugin_commands_join_the_mods_tree() {
        let plugins = json!({"type": "root", "children": {"hello": {"type": "literal"}}});
        assert_eq!(merge_commands(&Value::Null, Value::Null), Value::Null);
        assert_eq!(merge_commands(&Value::Null, plugins.clone()), plugins);
        let mods = json!({"type": "root", "children": {"beacon": {"type": "literal"}, "hello": {"type": "old"}}});
        let merged = merge_commands(&mods, plugins);
        assert_eq!(merged["children"]["beacon"]["type"], "literal");
        assert_eq!(merged["children"]["hello"]["type"], "literal", "the plugin wins");
        assert_eq!(merge_commands(&mods, Value::Null), mods);
    }
}
