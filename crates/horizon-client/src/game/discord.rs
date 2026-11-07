//! What the player is doing, for Discord Rich Presence (`horizon_client::discord_presence`).
use super::*;
use horizon_client::discord_presence::{self as presence, Dimension, Play, Show, Status, World};

impl Game<'_> {
    /// Each frame: only a change reaches the presence thread.
    pub(super) fn update_discord_presence(&self) {
        presence::set(self.discord_status());
    }
    fn discord_status(&self) -> Status {
        if !self.settings.extras.discord_presence {
            return Status::Off;
        }
        if self.title.is_some() {
            return Status::Menus;
        }
        let options = self.modules_ref().map(|m| m.discord.clone()).unwrap_or_default();
        let mut players = None;
        let play = if let Some(host) = &self.lan {
            players = Some(1 + host.hub.guests.len());
            Play::Hosting { players: 1 + host.hub.guests.len() }
        } else if let Some(server) = &self.integrated {
            match (server.published(), &self.remote) {
                (Some(_), Some(remote)) => {
                    players = Some(remote.player_count());
                    Play::Hosting { players: remote.player_count() }
                }
                _ => Play::Singleplayer,
            }
        } else if let Some(remote) = &self.remote {
            players = Some(remote.player_count());
            Play::Multiplayer
        } else {
            Play::Singleplayer
        };
        let place = match play {
            Play::Multiplayer if options.show_server && !self.belumi_client_server().is_empty() => {
                Some(horizon_client::net::shown_address(self.belumi_client_server()))
            }
            Play::Singleplayer | Play::Hosting { .. } if options.show_world => {
                self.integrated.as_ref().map(|i| i.world_name.clone())
            }
            _ => None,
        };
        Status::World(World {
            play,
            dimension: match self.dim {
                Dim::Overworld => Dimension::Overworld,
                Dim::Nether => Dimension::Nether,
                Dim::End => Dimension::End,
            },
            mode: self.mode,
            place,
            players: players.filter(|_| options.show_players),
            show: Show { dimension: options.show_dimension, mode: options.show_mode, time: options.show_time },
        })
    }
}
