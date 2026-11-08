//! Pings and shared waypoints between Belumi players on the same server. A ping (Z, Shift+Z, Ctrl+Z) marks the block in
//! the crosshair for everyone for 20 seconds; `/sharewp [NAME]` offers a waypoint, `/acceptwp` keeps the one offered.
//! The messages travel as a `belumi:ping` payload, which a Horizon server (dedicated, LAN, single player) hands on to the
//! other players; other kinds of servers drop it, and then nothing happens.
use super::Game;
use horizon_client::hud::HudBatch;
use horizon_protocol::packets::play::Serverbound as S;
use std::time::{Duration, Instant};

pub(super) const CHANNEL: &str = "belumi:ping";
const SHOWN: Duration = Duration::from_secs(20);
const SEND_EVERY: Duration = Duration::from_millis(500);
const MAX_PINGS: usize = 24;
const MAX_PAYLOAD: usize = 400;
const KINDS: [(&str, u32); 3] = [("Look here", 0x0055_FF55), ("Danger", 0x00FF_5555), ("Come here", 0x0055_AAFF)];

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Msg {
    Ping { kind: usize, pos: [f64; 3] },
    Waypoint { name: String, pos: [i32; 3], dimension: String },
}

#[derive(Clone, Debug)]
struct Ping {
    from: String,
    kind: usize,
    pos: [f64; 3],
    at: Instant,
}

#[derive(Default)]
pub(super) struct Pings {
    list: Vec<Ping>,
    last_sent: Option<Instant>,
    /// A waypoint another player offered: who, and what.
    offered: Option<(String, Msg, Instant)>,
}

/// The payload for a message.
pub(super) fn encode(msg: &Msg) -> Vec<u8> {
    match msg {
        Msg::Ping { kind, pos } => serde_json::json!({"k": "ping", "t": kind, "x": pos[0], "y": pos[1], "z": pos[2]}),
        Msg::Waypoint { name, pos, dimension } => serde_json::json!({"k": "wp", "n": name, "x": pos[0], "y": pos[1], "z": pos[2], "d": dimension}),
    }
    .to_string()
    .into_bytes()
}

fn clean_name(s: &str, max: usize) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.chars().count() <= max && s.chars().all(|c| !c.is_control())).then(|| s.to_owned())
}

/// What the server handed on: `SENDER\n{json}`. Anything out of shape is refused.
pub(super) fn decode(data: &[u8]) -> Option<(String, Msg)> {
    if data.len() > MAX_PAYLOAD + 40 {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let (from, json) = text.split_once('\n')?;
    let from = clean_name(from, 32)?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let num = |k: &str| v.get(k)?.as_f64().filter(|n| n.is_finite() && n.abs() < 3.0e7);
    let msg = match v.get("k")?.as_str()? {
        "ping" => {
            let kind = usize::try_from(v.get("t")?.as_u64()?).ok().filter(|k| *k < KINDS.len())?;
            Msg::Ping { kind, pos: [num("x")?, num("y")?, num("z")?] }
        }
        "wp" => Msg::Waypoint {
            name: clean_name(v.get("n")?.as_str()?, 28)?,
            pos: [num("x")?.round() as i32, num("y")?.round() as i32, num("z")?.round() as i32],
            dimension: clean_name(v.get("d")?.as_str()?, 40)?,
        },
        _ => return None,
    };
    Some((from, msg))
}

/// The server's side: the payload a guest sent with the guest's name in front, or nothing when it is not a message.
pub(super) fn relay(name: &str, data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > MAX_PAYLOAD {
        return None;
    }
    let mut out = format!("{name}\n").into_bytes();
    out.extend_from_slice(data);
    // The shape is checked the way a client will: a message that would be refused there is not handed on.
    decode(&out)?;
    Some(out)
}

impl Game<'_> {
    fn pings_on(&self) -> bool {
        self.remote.is_some() && self.belumi_replay.playing_none() && self.modules_ref().is_none_or(|m| m.misc.pings)
    }

    fn belumi_ping_send(&mut self, msg: &Msg) -> bool {
        if self.belumi_ping.last_sent.is_some_and(|t| t.elapsed() < SEND_EVERY) {
            return false;
        }
        self.belumi_ping.last_sent = Some(Instant::now());
        self.send(S::CustomPayload(horizon_protocol::packets::configuration::CustomPayload { channel: CHANNEL.into(), data: encode(msg) }));
        true
    }

    /// The ping key: Z marks "look here", Shift+Z "danger", Ctrl+Z "come here", at the block in the crosshair.
    pub(super) fn belumi_ping_key(&mut self, key: &str, pressed: bool) {
        let want = self.modules_ref().map(|m| m.misc.ping_key.clone()).unwrap_or_default();
        if !pressed || key != want || !self.native_playing() || !self.pings_on() {
            return;
        }
        let kind = if self.has_shift_down() {
            1
        } else if self.has_control_down() {
            2
        } else {
            0
        };
        let Some(hit) = self.pick() else {
            self.toast("Aim at a block to ping it");
            return;
        };
        let pos = [f64::from(hit.pos[0]) + 0.5, f64::from(hit.pos[1]) + 1.0, f64::from(hit.pos[2]) + 0.5];
        if self.belumi_ping_send(&Msg::Ping { kind, pos }) {
            self.belumi_ping_add("You".to_owned(), kind, pos);
        }
    }

    fn belumi_ping_add(&mut self, from: String, kind: usize, pos: [f64; 3]) {
        let list = &mut self.belumi_ping.list;
        // A player has one ping of each kind at a time.
        list.retain(|p| !(p.from == from && p.kind == kind));
        list.push(Ping { from, kind, pos, at: Instant::now() });
        if list.len() > MAX_PINGS {
            list.remove(0);
        }
    }

    /// A `belumi:ping` payload from the server.
    pub(super) fn belumi_ping_receive(&mut self, data: &[u8]) {
        if !self.pings_on() {
            return;
        }
        let Some((from, msg)) = decode(data) else { return };
        match msg {
            Msg::Ping { kind, pos } => {
                self.belumi_ping_add(from, kind, pos);
                self.audio.play_ping();
            }
            Msg::Waypoint { ref name, pos, .. } => {
                self.native_chat(&format!("{from} shared the waypoint {name} ({} {} {}). Type /acceptwp to keep it.", pos[0], pos[1], pos[2]));
                self.belumi_ping.offered = Some((from, msg, Instant::now()));
            }
        }
    }

    /// `/sharewp [NAME]` (the nearest waypoint without a name) and `/acceptwp`.
    pub(super) fn belumi_ping_command(&mut self, command: &str, ran: &mut bool) {
        let (head, rest) = match command.trim().split_once(char::is_whitespace) {
            Some((h, r)) => (h.to_ascii_lowercase(), r.trim().to_owned()),
            None => (command.trim().to_ascii_lowercase(), String::new()),
        };
        match head.as_str() {
            "sharewp" if !*ran => {
                *ran = true;
                if !self.pings_on() {
                    self.native_chat("Pings and shared waypoints are for servers (and need the option on).");
                    return;
                }
                let Some(p) = self.native_player() else { return };
                let scope = self.belumi_scope();
                let store = self.belumi_waypoint_store();
                let chosen = if rest.is_empty() {
                    store.nearest(&scope, p.dimension, [p.x, p.y, p.z], 1).first().map(|(w, _)| (*w).clone())
                } else {
                    store.here(&scope, p.dimension).find(|w| w.name.eq_ignore_ascii_case(&rest)).cloned()
                };
                let Some(w) = chosen else {
                    self.native_chat("No such waypoint here. /sharewp NAME shares one of yours.");
                    return;
                };
                let msg = Msg::Waypoint { name: w.name.clone(), pos: [w.x, w.y, w.z], dimension: w.dimension.clone() };
                if self.belumi_ping_send(&msg) {
                    self.native_chat(&format!("Shared {} with the other players.", w.name));
                } else {
                    self.native_chat("Wait a moment before sharing again.");
                }
            }
            "acceptwp" if !*ran => {
                *ran = true;
                let Some((from, Msg::Waypoint { name, pos, dimension }, at)) = self.belumi_ping.offered.take() else {
                    self.native_chat("Nobody offered you a waypoint.");
                    return;
                };
                if at.elapsed() > Duration::from_secs(120) {
                    self.native_chat("That offer is too old.");
                    return;
                }
                let scope = self.belumi_scope();
                let full: String = format!("{from}: {name}").chars().take(40).collect();
                let store = self.belumi_waypoint_store();
                match store.add(&scope, &dimension, &full, pos) {
                    Ok(_) => {
                        self.belumi_save_waypoints();
                        self.native_chat(&format!("Kept the waypoint {full}."));
                    }
                    Err(e) => self.native_chat(&e),
                }
            }
            _ => {}
        }
    }

    /// The pings, in the world: a mark, the kind, who and how far.
    pub(super) fn belumi_draw_pings(&mut self, batch: &mut HudBatch) {
        self.belumi_ping.list.retain(|p| p.at.elapsed() < SHOWN);
        if self.belumi_ping.list.is_empty() || !self.camera_type.first_person() {
            return;
        }
        let Some(p) = self.player.as_ref() else { return };
        let feet = [p.position.x, p.position.y, p.position.z];
        let Some((cam, bob)) = self.belumi_frame_camera(batch.width / batch.height.max(1.0)) else { return };
        let list = self.belumi_ping.list.clone();
        for ping in list {
            let Some((u, v)) = super::belumi_maps::project_bobbed(&cam, bob, ping.pos) else { continue };
            let (label, color) = KINDS[ping.kind];
            let d = ((ping.pos[0] - feet[0]).powi(2) + (ping.pos[1] - feet[1]).powi(2) + (ping.pos[2] - feet[2]).powi(2)).sqrt();
            let text = format!("{label} - {} {}", ping.from, horizon_client::waypoints::distance_label(d));
            let width = self.native_text_width(&text);
            let (x, y) = (u * batch.width, v * batch.height);
            self.native_fill(batch, [x - 3.0, y - 3.0, 6.0, 6.0], 0xFF00_0000 | color);
            self.native_fill(batch, [x - width / 2.0 - 2.0, y - 14.0, width + 4.0, 10.0], 0x6000_0000);
            self.native_text(batch, &text, [x - width / 2.0, y - 13.0], color, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ping_goes_through_the_relay_and_reads_back() {
        let sent = encode(&Msg::Ping { kind: 1, pos: [10.5, 64.0, -3.5] });
        let handed = relay("Alex", &sent).unwrap();
        assert_eq!(decode(&handed), Some(("Alex".to_owned(), Msg::Ping { kind: 1, pos: [10.5, 64.0, -3.5] })));
    }

    #[test]
    fn a_waypoint_goes_through_too() {
        let msg = Msg::Waypoint { name: "Base".into(), pos: [100, 64, -20], dimension: "minecraft:overworld".into() };
        assert_eq!(decode(&relay("Steve", &encode(&msg)).unwrap()), Some(("Steve".to_owned(), msg)));
    }

    #[test]
    fn nonsense_is_not_handed_on_and_not_believed() {
        for bad in [
            &b"not json"[..],
            br#"{"k":"ping","t":9,"x":1,"y":1,"z":1}"#,
            br#"{"k":"ping","t":0,"x":1e99,"y":1,"z":1}"#,
            br#"{"k":"ping","t":0,"x":1,"y":1}"#,
            br#"{"k":"unknown"}"#,
            br#"{"k":"wp","n":"","x":1,"y":1,"z":1,"d":"x"}"#,
        ] {
            assert_eq!(relay("Alex", bad), None, "{}", String::from_utf8_lossy(bad));
        }
        assert_eq!(relay("Alex", &vec![b' '; 500]), None, "too big");
        assert_eq!(decode(b"no newline"), None);
        assert_eq!(decode("\u{7}bell\n{}".as_bytes()), None, "a sender with control characters");
    }

    #[test]
    fn the_sender_cannot_smuggle_a_second_line() {
        // The server writes the name it knows; a client's own payload with a newline in the json part stays json.
        let sent = br#"{"k":"ping","t":0,"x":1,"y":2,"z":3}"#;
        assert!(relay("Alex", sent).is_some());
        let split = b"{\"k\":\"ping\",\"t\":0,\nx\":1}";
        assert_eq!(relay("Alex", split), None);
    }
}
