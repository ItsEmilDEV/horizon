//! Switching the account from inside the game. The launcher leaves `belumi-accounts.json` (names and ids, no token)
//! in the game folder; picking another account writes `belumi-switch.json` with that account and the server, and the
//! game quits. The launcher sees it end, signs that account in and starts the game again on the same server.
use super::{Game, belumi_client::Act};
use horizon_client::hud::HudBatch;

const ACCOUNTS_FILE: &str = "belumi-accounts.json";
const REQUEST_FILE: &str = "belumi-switch.json";

/// The launcher's list: the active account's id, whether the launcher can start the game again, and (name, id).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Saved {
    pub(super) active: String,
    pub(super) can_switch: bool,
    pub(super) accounts: Vec<(String, String)>,
}

pub(super) fn parse_saved(text: &str) -> Option<Saved> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let accounts = v
        .get("accounts")?
        .as_array()?
        .iter()
        .filter_map(|a| Some((a.get("name")?.as_str()?.to_owned(), a.get("uuid")?.as_str()?.to_owned())))
        .collect();
    Some(Saved {
        active: v.get("active").and_then(|a| a.as_str()).unwrap_or("").to_owned(),
        can_switch: v.get("can_switch").and_then(|a| a.as_bool()).unwrap_or(false),
        accounts,
    })
}

pub(super) fn request_json(uuid: &str, address: &str) -> String {
    serde_json::json!({"uuid": uuid, "address": address}).to_string()
}

impl Game<'_> {
    fn belumi_saved_accounts(&mut self) -> Saved {
        if let Some(saved) = &self.belumi_client.saved {
            return saved.clone();
        }
        let saved = std::fs::read_to_string(Self::belumi_dir_for_accounts().join(ACCOUNTS_FILE))
            .ok()
            .and_then(|t| parse_saved(&t))
            .unwrap_or_default();
        self.belumi_client.saved = Some(saved.clone());
        saved
    }

    fn belumi_dir_for_accounts() -> std::path::PathBuf {
        std::env::current_dir().unwrap_or_default()
    }

    /// Asks the launcher for another account, on the server the player is on, and ends the game.
    pub(super) fn belumi_switch_account(&mut self, index: usize) {
        let saved = self.belumi_saved_accounts();
        let Some((name, uuid)) = saved.accounts.get(index).cloned() else { return };
        if !saved.can_switch || uuid == saved.active {
            return;
        }
        let address = if self.remote.is_some() { self.belumi_client_server().to_owned() } else { String::new() };
        let path = Self::belumi_dir_for_accounts().join(REQUEST_FILE);
        match std::fs::write(&path, request_json(&uuid, &address)) {
            Ok(()) => {
                self.toast(format!("Switching to {name}..."));
                self.belumi_client.switch_quit = true;
            }
            Err(e) => self.toast(format!("Could not ask the launcher: {e}")),
        }
    }

    pub(super) fn draw_accounts_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        use super::belumi_client::Page;
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let saved = self.belumi_saved_accounts();
        let rows = saved.accounts.len().clamp(1, 8) as f32;
        let pw = 300.0_f32.min(w - 12.0);
        let ph = (60.0 + rows * 22.0 + 30.0).min(h - 16.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Switch account");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        let mut ry = y + 28.0;
        if saved.accounts.is_empty() {
            self.native_text(batch, "No accounts known. Start the game from the", [x + 10.0, ry], 0x008A_93A8, false);
            self.native_text(batch, "Belumi launcher and sign in there.", [x + 10.0, ry + 12.0], 0x008A_93A8, false);
            return;
        }
        for (i, (name, uuid)) in saved.accounts.iter().take(8).enumerate() {
            let active = *uuid == saved.active;
            let label = if active { format!("{name} (playing)") } else { name.clone() };
            if active || !saved.can_switch {
                self.native_text(batch, &label, [x + 14.0, ry + 4.0], if active { 0x00FF_FFFF } else { 0x008A_93A8 }, false);
            } else {
                self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], &label, None, Act::PickAccount(i));
            }
            ry += 22.0;
        }
        ry += 4.0;
        let note = if saved.can_switch {
            "The game restarts and joins this server again."
        } else {
            "Set the launcher to stay open while playing to switch."
        };
        self.native_text(batch, note, [x + 10.0, ry], 0x008A_93A8, false);
    }

    /// The Plugin page: the screens plugins made, or one plugin's rows.
    pub(super) fn draw_plugin_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        use super::belumi_client::Page;
        use super::belumi_plugins::RowView;
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let screens = self.belumi_plugin_screens();
        let shown = self.belumi_client.plugin_page.clone();
        let current = shown.as_ref().and_then(|p| screens.iter().find(|s| &s.plugin == p)).cloned();
        let rows = current.as_ref().map_or(screens.len(), |s| s.rows.len()).clamp(1, 24) as f32;
        let pw = 320.0_f32.min(w - 12.0);
        let ph = (44.0 + rows * 22.0).min(h - 16.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        let title = current.as_ref().map_or("Plugin screens".to_owned(), |s| if s.title.is_empty() { s.plugin.clone() } else { s.title.clone() });
        self.panel(batch, [x, y, pw, ph], &title);
        let back = if current.is_some() { Act::Page(Page::Plugin) } else { Act::Page(Page::Modules) };
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, back);
        let mut ry = y + 28.0;
        let Some(screen) = current else {
            if screens.is_empty() {
                self.native_text(batch, "No plugin has a screen. Plugins with the", [x + 10.0, ry], 0x008A_93A8, false);
                self.native_text(batch, "`screen` permission can add one.", [x + 10.0, ry + 12.0], 0x008A_93A8, false);
            }
            for s in &screens {
                let label = if s.title.is_empty() { s.plugin.clone() } else { format!("{} ({})", s.title, s.plugin) };
                self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], &label, None, Act::PluginOpen(s.plugin.clone()));
                ry += 22.0;
            }
            return;
        };
        for row in &screen.rows {
            match row {
                RowView::Label(text) => self.native_text(batch, text, [x + 12.0, ry + 4.0], 0x00FF_FFFF, false),
                RowView::Button { id, text } => {
                    self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], text, None, Act::PluginPress(screen.plugin.clone(), id.clone(), false));
                }
                RowView::Toggle { id, text, on } => {
                    self.client_button(batch, [x + 10.0, ry, pw - 20.0, 16.0], text, Some(*on), Act::PluginPress(screen.plugin.clone(), id.clone(), !on));
                }
            }
            ry += 22.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launchers_list_is_read() {
        let s = parse_saved(r#"{"active":"aa","can_switch":true,"accounts":[{"name":"Alex","uuid":"aa"},{"name":"Steve","uuid":"bb"},{"bad":1}]}"#).unwrap();
        assert_eq!(s.active, "aa");
        assert!(s.can_switch);
        assert_eq!(s.accounts, vec![("Alex".into(), "aa".into()), ("Steve".into(), "bb".into())]);
        assert!(parse_saved("nonsense").is_none());
    }

    #[test]
    fn a_request_holds_the_account_and_the_server() {
        let v: serde_json::Value = serde_json::from_str(&request_json("bb", "mc.example.org")).unwrap();
        assert_eq!(v["uuid"], "bb");
        assert_eq!(v["address"], "mc.example.org");
    }
}
