//! Discord Rich Presence: "Playing Belumi" on the player's Discord profile, with what they are
//! doing (docs/discord-presence.md). The connection to the Discord app is the crate
//! `belumi-presence`; here is what the game says through it. The game only calls [`set`] each
//! frame: a change is turned into an activity and handed over. Nothing is sent with the setting
//! off, or with `HORIZON_NO_DISCORD=1` / `BELUMI_NO_DISCORD=1`. Without a Belumi application ID
//! (`BELUMI_DISCORD_APP_ID` below or in the environment) there is no presence at all.
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use belumi_presence::Presence;
use serde_json::{Value, json};

use crate::command::args::GameMode;

use belumi_presence::app_id;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Play {
    Singleplayer,
    /// A server or someone else's LAN world.
    Multiplayer,
    /// This world is open to LAN or friends; `players` counts the host.
    Hosting {
        players: usize,
    },
}

/// What the player chose to show (`modules::DiscordOptions`), minus what the game fills in itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Show {
    pub dimension: bool,
    pub mode: bool,
    pub time: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    pub play: Play,
    pub dimension: Dimension,
    pub mode: GameMode,
    /// The server's address or the world's name, when the player allows it.
    pub place: Option<String>,
    /// Players on the server, when the player allows it.
    pub players: Option<usize>,
    pub show: Show,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The player turned Rich Presence off: the activity is cleared.
    Off,
    Menus,
    World(World),
}

/// What the game wants shown, and since when it has been in a world.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wanted {
    status: Status,
    /// Unix seconds: when the player entered the world (dimension changes keep it).
    since: Option<u64>,
}
impl Wanted {
    /// A new status at `now`; false if it is the one already wanted.
    fn update(&mut self, status: Status, now: u64) -> bool {
        if self.status == status {
            return false;
        }
        let in_world = |s: &Status| matches!(s, Status::World { .. });
        if !in_world(&status) {
            self.since = None;
        } else if !in_world(&self.status) {
            self.since = Some(now);
        }
        self.status = status;
        true
    }
}

static WANTED: Mutex<Wanted> = Mutex::new(Wanted {
    status: Status::Menus,
    since: None,
});
static PRESENCE: OnceLock<Presence> = OnceLock::new();

fn lock() -> std::sync::MutexGuard<'static, Wanted> {
    WANTED.lock().unwrap_or_else(|e| e.into_inner())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Starts the presence, once: when the game starts, never for a dedicated server.
/// `automated` runs (offscreen, benchmarks) only talk to a test's fake Discord.
pub fn start(enabled: bool, automated: bool) {
    let fake_discord = std::env::var_os("BELUMI_DISCORD_IPC_DIR").is_some()
        || std::env::var_os("HORIZON_DISCORD_IPC_DIR").is_some();
    if PRESENCE.get().is_some()
        || belumi_presence::disabled_by_env()
        || (app_id().is_none() && !fake_discord)
        || (automated && !fake_discord)
    {
        return;
    }
    if !enabled {
        lock().status = Status::Off;
    }
    let id = app_id().unwrap_or_else(|| "0".into());
    if let Some(presence) = Presence::start(&id) {
        let wanted = lock().clone();
        presence.set(activity(&wanted.status, wanted.since));
        let _ = PRESENCE.set(presence);
    }
}

/// What the player does now. Free without [`start`].
pub fn set(status: Status) {
    let Some(presence) = PRESENCE.get() else {
        return;
    };
    let mut wanted = lock();
    if wanted.update(status, unix_now()) {
        presence.set(activity(&wanted.status, wanted.since));
    }
}

/// On quit: clears the activity, waiting at most a moment for Discord.
pub fn stop() {
    if let Some(presence) = PRESENCE.get() {
        presence.stop();
    }
}

/// The activity Discord shows for `status`; `None` clears it.
fn activity(status: &Status, since: Option<u64>) -> Option<Value> {
    let mut assets = json!({"large_image": "belumi-minecraft", "large_text": "Belumi - Minecraft 26.3"});
    let (details, state, party, time) = match status {
        Status::Off => return None,
        Status::Menus => ("In the menus".to_owned(), None, None, false),
        Status::World(w) => {
            if w.show.mode {
                let (key, text) = match w.mode {
                    GameMode::Survival => ("survival", "Survival"),
                    GameMode::Creative => ("creative", "Creative"),
                    GameMode::Adventure => ("adventure", "Adventure"),
                    GameMode::Spectator => ("spectator", "Spectator"),
                };
                assets["small_image"] = key.into();
                assets["small_text"] = text.into();
            }
            let place = match w.dimension {
                Dimension::Overworld => "In the Overworld",
                Dimension::Nether => "In the Nether",
                Dimension::End => "In the End",
            };
            let (mut details, party) = match w.play {
                Play::Singleplayer => ("Singleplayer".to_owned(), None),
                Play::Multiplayer => ("Multiplayer".to_owned(), None),
                Play::Hosting { players } => ("Hosting a world".to_owned(), Some(players)),
            };
            if let Some(name) = w.place.as_deref().filter(|p| !p.trim().is_empty()) {
                details = match w.play {
                    Play::Multiplayer => format!("Playing on {name}"),
                    _ => format!("{details}: {name}"),
                };
            }
            let mut parts: Vec<String> = Vec::new();
            if w.show.dimension {
                parts.push(place.to_owned());
            }
            // The party bar already counts the players of an open world.
            if let (Some(n), None) = (w.players, party) {
                parts.push(format!("{n} {}", if n == 1 { "player" } else { "players" }));
            }
            let state = (!parts.is_empty()).then(|| parts.join(" · "));
            let party = party.filter(|_| w.players.is_some());
            (details, state, party, w.show.time)
        }
    };
    let mut a = json!({"details": details, "assets": assets});
    if let Some(state) = state {
        a["state"] = state.into();
    }
    if let (true, Some(start)) = (time, since) {
        a["timestamps"] = json!({"start": start});
    }
    if let Some(players) = party {
        let max = crate::lan::MAX_PLAYERS;
        a["party"] = json!({
            "id": format!("belumi-{}", std::process::id()),
            "size": [players.clamp(1, max), max],
        });
    }
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(play: Play, dimension: Dimension) -> Status {
        Status::World(World {
            play,
            dimension,
            mode: GameMode::Survival,
            place: None,
            players: None,
            show: Show { dimension: true, mode: true, time: true },
        })
    }

    fn with(status: Status, f: impl FnOnce(&mut World)) -> Status {
        match status {
            Status::World(mut w) => {
                f(&mut w);
                Status::World(w)
            }
            other => other,
        }
    }

    #[test]
    fn activities_say_what_the_player_does_and_nothing_private() {
        assert_eq!(activity(&Status::Off, None), None);
        let menus = activity(&Status::Menus, None).unwrap();
        assert_eq!(menus["details"], "In the menus");
        assert!(menus.get("state").is_none() && menus.get("timestamps").is_none());
        assert_eq!(menus["assets"]["large_image"], "belumi-minecraft");

        let single = activity(&world(Play::Singleplayer, Dimension::Nether), Some(1000)).unwrap();
        assert_eq!(single["details"], "Singleplayer");
        assert_eq!(single["state"], "In the Nether");
        assert_eq!(single["timestamps"]["start"], 1000);
        assert!(single.get("party").is_none());
        assert_eq!(single["assets"]["small_image"], "survival");
        assert!(menus["assets"].get("small_image").is_none());

        // The address is shown only when the game fills `place`.
        let multi = activity(&world(Play::Multiplayer, Dimension::End), Some(5)).unwrap();
        assert_eq!(multi["details"], "Multiplayer");
        assert_eq!(multi["state"], "In the End");

        let host = world(Play::Hosting { players: 3 }, Dimension::Overworld);
        let host = activity(&host, Some(5)).unwrap();
        assert_eq!(host["details"], "Hosting a world");
        assert_eq!(host["state"], "In the Overworld");
        assert!(host.get("party").is_none(), "the player count is off until chosen");
    }

    #[test]
    fn the_player_chooses_what_is_shown() {
        let server = with(world(Play::Multiplayer, Dimension::Overworld), |w| {
            w.place = Some("play.example.net".into());
            w.players = Some(12);
        });
        let a = activity(&server, Some(9)).unwrap();
        assert_eq!(a["details"], "Playing on play.example.net");
        assert_eq!(a["state"], "In the Overworld · 12 players");

        let world_name = with(world(Play::Singleplayer, Dimension::Nether), |w| w.place = Some("My Base".into()));
        assert_eq!(activity(&world_name, None).unwrap()["details"], "Singleplayer: My Base");

        let bare = with(server, |w| w.show = Show { dimension: false, mode: false, time: false });
        let a = activity(&bare, Some(9)).unwrap();
        assert_eq!(a["state"], "12 players");
        assert!(a["assets"].get("small_image").is_none() && a.get("timestamps").is_none());

        let hosting = with(world(Play::Hosting { players: 3 }, Dimension::End), |w| w.players = Some(3));
        assert_eq!(activity(&hosting, None).unwrap()["party"]["size"], json!([3, 8]));
    }

    #[test]
    fn time_counts_from_entering_the_world() {
        let mut w = Wanted {
            status: Status::Menus,
            since: None,
        };
        assert!(!w.update(Status::Menus, 10));
        assert!(w.update(world(Play::Singleplayer, Dimension::Overworld), 20));
        assert_eq!(w.since, Some(20));
        // Another dimension, opening to LAN: the same visit.
        assert!(w.update(world(Play::Singleplayer, Dimension::Nether), 30));
        assert!(w.update(world(Play::Hosting { players: 1 }, Dimension::Nether), 40));
        assert_eq!(w.since, Some(20));
        assert!(w.update(Status::Menus, 50));
        assert_eq!(w.since, None);
        assert!(w.update(world(Play::Multiplayer, Dimension::Overworld), 60));
        assert_eq!(w.since, Some(60));
        assert!(w.update(Status::Off, 70));
        assert_eq!(w.since, None);
    }
}
