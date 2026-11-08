//! Combat statistics: every fight (a run of swings, hits and damage with a pause of 8 s ending it) is written down: swings,
//! hits that landed (the server's damage events), damage taken, the clicks per second second by second, and who you fought.
//! The Combat page lists the last fights.
use super::{Game, belumi_client::{Act, Page}};
use horizon_client::hud::HudBatch;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FILE: &str = "belumi-combat.json";
const KEEP: usize = 50;
/// A fight ends this long after its last swing, hit or damage.
const QUIET: Duration = Duration::from_secs(8);
const LISTED: usize = 8;
/// Seconds of the clicks-per-second graph kept per fight.
const GRAPH: usize = 30;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Fight {
    pub(super) start_unix: u64,
    pub(super) seconds: u32,
    pub(super) swings: u32,
    pub(super) hits: u32,
    /// Damage dealt to creatures whose health the client sees.
    #[serde(default)]
    pub(super) dealt: f32,
    pub(super) hurt: u32,
    pub(super) taken: f32,
    pub(super) peak_cps: u32,
    pub(super) cps: Vec<u8>,
    pub(super) opponents: Vec<String>,
    pub(super) died: bool,
}

impl Fight {
    /// The share of swings that landed, in percent.
    pub(super) fn accuracy(&self) -> Option<u32> {
        (self.swings > 0).then(|| (self.hits * 100 / self.swings).min(100))
    }

    pub(super) fn is_worth_keeping(&self) -> bool {
        self.swings >= 2 || self.hits > 0 || self.taken >= 1.0 || self.died
    }

    /// One line: `14/31 hits 45%, 8.5 damage taken, peak 9 CPS, zombie x2`.
    pub(super) fn summary(&self) -> String {
        let mut parts = Vec::new();
        match self.accuracy() {
            Some(a) => parts.push(format!("{}/{} hits {a}%", self.hits, self.swings)),
            None => parts.push(format!("{} hits", self.hits)),
        }
        if self.dealt > 0.0 {
            parts.push(format!("{:.1} dealt", self.dealt));
        }
        parts.push(format!("{:.1} taken", self.taken));
        if self.peak_cps > 0 {
            parts.push(format!("peak {} CPS", self.peak_cps));
        }
        if !self.opponents.is_empty() {
            parts.push(self.opponents.join(", "));
        }
        if self.died {
            parts.push("died".to_owned());
        }
        parts.join(", ")
    }
}

struct Live {
    fight: Fight,
    last: Instant,
    started: Instant,
    bucket_start: Instant,
    bucket: u32,
    /// The creatures fought, by kind and entity.
    opponents: BTreeMap<String, BTreeSet<i32>>,
    /// The health each swung-at creature had when last looked at.
    targets: HashMap<i32, f32>,
}

impl Live {
    fn new() -> Self {
        let now = Instant::now();
        let start_unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Self { fight: Fight { start_unix, ..Fight::default() }, last: now, started: now, bucket_start: now, bucket: 0, opponents: BTreeMap::new(), targets: HashMap::new() }
    }

    /// Closes the seconds that passed: each becomes one point of the clicks graph.
    fn roll(&mut self, now: Instant) {
        while now.duration_since(self.bucket_start) >= Duration::from_secs(1) {
            let c = self.bucket.min(255);
            self.fight.cps.push(c as u8);
            self.fight.peak_cps = self.fight.peak_cps.max(c);
            self.bucket = 0;
            self.bucket_start += Duration::from_secs(1);
        }
    }

    fn finish(mut self, died: bool) -> Fight {
        let now = Instant::now();
        self.roll(now);
        if self.bucket > 0 {
            self.fight.peak_cps = self.fight.peak_cps.max(self.bucket);
            self.fight.cps.push(self.bucket.min(255) as u8);
        }
        let skip = self.fight.cps.len().saturating_sub(GRAPH);
        self.fight.cps.drain(..skip);
        self.fight.seconds = self.last.duration_since(self.started).as_secs() as u32 + 1;
        self.fight.died = died;
        self.fight.opponents = self.opponents.into_iter().map(|(k, ids)| if ids.len() > 1 { format!("{k} x{}", ids.len()) } else { k }).collect();
        self.fight
    }
}

#[derive(Default)]
pub(super) struct Combat {
    /// Entity ids the player swung at, put in by `send` (which cannot change the game).
    swings: Mutex<Vec<i32>>,
    live: Option<Live>,
    fights: Option<Vec<Fight>>,
    health: Option<f32>,
}

impl Combat {
    pub(super) fn note_swing(&self, target: i32) {
        if let Ok(mut q) = self.swings.lock() {
            if q.len() < 256 {
                q.push(target);
            }
        }
    }
}

fn plain_kind(kind: &str) -> String {
    kind.strip_prefix("minecraft:").unwrap_or(kind).replace('_', " ")
}

impl Game<'_> {
    fn combat_path() -> PathBuf {
        std::env::current_dir().unwrap_or_default().join(FILE)
    }

    fn combat_fights(&mut self) -> &mut Vec<Fight> {
        self.belumi_combat.fights.get_or_insert_with(|| {
            std::fs::read_to_string(Self::combat_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
        })
    }

    fn combat_on(&self) -> bool {
        self.remote.is_some() && self.belumi_replay.playing_none() && self.modules_ref().is_none_or(|m| m.misc.combat)
    }

    fn combat_live(&mut self) -> &mut Live {
        self.belumi_combat.live.get_or_insert_with(Live::new)
    }

    fn combat_opponent(&mut self, id: i32) {
        let kind = self.remote.as_ref().and_then(|r| r.entity_kind(id)).map_or_else(|| "creature".to_owned(), |k| plain_kind(&k));
        self.combat_live().opponents.entry(kind).or_default().insert(id);
    }

    /// A damage event: the player was hurt, or the player's hit landed.
    pub(super) fn belumi_combat_damage(&mut self, victim: i32, cause: Option<i32>) {
        if !self.combat_on() {
            return;
        }
        let me = self.remote.as_ref().map(|r| r.entity_id);
        if Some(victim) == me {
            let live = self.combat_live();
            live.fight.hurt += 1;
            live.last = Instant::now();
            if let Some(c) = cause {
                self.combat_opponent(c);
            }
        } else if cause.is_some() && cause == me && !self.mobs.iter().any(|m| m.id as i32 == victim) {
            // A creature's health is watched (below); this is for players and the like.
            let live = self.combat_live();
            live.fight.hits += 1;
            live.last = Instant::now();
            self.combat_opponent(victim);
        }
    }

    /// The health the server sent: a drop during a fight is damage taken.
    pub(super) fn belumi_combat_health(&mut self, health: f32) {
        let before = self.belumi_combat.health.replace(health);
        if let (Some(before), Some(live)) = (before, self.belumi_combat.live.as_mut())
            && health < before
        {
            live.fight.taken += before - health;
            live.last = Instant::now();
        }
    }

    /// Once a frame: the swings, the end of a fight.
    pub(super) fn belumi_combat_tick(&mut self) {
        let swings: Vec<i32> = self.belumi_combat.swings.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if !self.combat_on() {
            self.belumi_combat.live = None;
            return;
        }
        for target in swings {
            let live = self.combat_live();
            let now = Instant::now();
            live.roll(now);
            live.fight.swings += 1;
            live.bucket += 1;
            live.last = now;
            if let Some(m) = self.mobs.iter().find(|m| m.id as i32 == target) {
                let health = m.health;
                self.combat_live().targets.entry(target).or_insert(health);
            }
            self.combat_opponent(target);
        }
        // A creature that was swung at lost health: a hit landed.
        if let Some(live) = self.belumi_combat.live.as_mut() {
            for (id, last) in live.targets.iter_mut() {
                if let Some(m) = self.mobs.iter().find(|m| m.id as i32 == *id) {
                    if m.health < *last {
                        live.fight.hits += 1;
                        live.fight.dealt += *last - m.health;
                        live.last = Instant::now();
                    }
                    *last = m.health;
                }
            }
        }
        let died = self.screen == super::Screen::Dead;
        let over = self.belumi_combat.live.as_ref().is_some_and(|l| l.last.elapsed() >= QUIET) || (died && self.belumi_combat.live.is_some());
        if over && let Some(live) = self.belumi_combat.live.take() {
            let fight = live.finish(died);
            if fight.is_worth_keeping() {
                self.toast(format!("Fight: {}", fight.summary()));
                let list = self.combat_fights();
                list.insert(0, fight);
                list.truncate(KEEP);
                if let Ok(text) = serde_json::to_string_pretty(list) {
                    let _ = std::fs::write(Self::combat_path(), text);
                }
            }
        }
    }

    /// `/combat` opens the page of the last fights.
    pub(super) fn belumi_combat_command(&mut self, command: &str, ran: &mut bool) {
        if !*ran && command.trim().eq_ignore_ascii_case("combat") {
            *ran = true;
            self.client_open_page(Page::Combat);
        }
    }

    pub(super) fn combat_act(&mut self, act: &Act) -> bool {
        if matches!(act, Act::CombatClear) {
            self.combat_fights().clear();
            let _ = std::fs::remove_file(Self::combat_path());
            return true;
        }
        false
    }

    pub(super) fn draw_combat_page(&mut self, batch: &mut HudBatch, w: f32, h: f32) {
        self.native_fill(batch, [0.0, 0.0, w, h], 0x9000_0000);
        let list: Vec<Fight> = self.combat_fights().iter().take(LISTED).cloned().collect();
        let rows = list.len().clamp(1, LISTED) as f32;
        let pw = 460.0_f32.min(w - 12.0);
        let ph = (60.0 + rows * 20.0).min(h - 16.0);
        let (x, y) = ((w - pw) / 2.0, ((h - ph) / 2.0).floor());
        self.panel(batch, [x, y, pw, ph], "Combat");
        self.client_button(batch, [x + pw - 54.0, y + 3.0, 48.0, 14.0], "Back", None, Act::Page(Page::Modules));
        self.client_button(batch, [x + pw - 112.0, y + 3.0, 52.0, 14.0], "Clear", None, Act::CombatClear);
        let mut ry = y + 28.0;
        if list.is_empty() {
            self.native_text(batch, "No fights yet. Fight something on a server and it is written down here:", [x + 10.0, ry], 0x008A_93A8, false);
            self.native_text(batch, "swings, hits that landed, damage taken and your clicks per second.", [x + 10.0, ry + 12.0], 0x008A_93A8, false);
            return;
        }
        for f in &list {
            let when = chrono::DateTime::from_timestamp(f.start_unix as i64, 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_default();
            self.native_fill(batch, [x + 8.0, ry, pw - 16.0, 18.0], 0xFF1A_2030);
            self.native_text(batch, &format!("{when}  {}", f.summary()), [x + 12.0, ry + 5.0], if f.died { 0x00FF_8A8A } else { 0x00FF_FFFF }, false);
            // The clicks graph: one bar per second, the tallest is the peak.
            let top = f64::from(f.peak_cps.max(1));
            let bars = f.cps.len().min(GRAPH);
            for (i, c) in f.cps.iter().rev().take(bars).rev().enumerate() {
                let bh = (f64::from(*c) / top * 12.0).ceil() as f32;
                self.native_fill(batch, [x + pw - 12.0 - (bars - i) as f32 * 2.0, ry + 15.0 - bh, 1.0, bh], 0xFF55_AAFF);
            }
            ry += 20.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accuracy_and_summary_read_well() {
        let f = Fight { swings: 31, hits: 14, taken: 8.5, peak_cps: 9, opponents: vec!["zombie x2".into()], ..Fight::default() };
        assert_eq!(f.accuracy(), Some(45));
        assert_eq!(f.summary(), "14/31 hits 45%, 8.5 taken, peak 9 CPS, zombie x2");
        let with_dealt = Fight { dealt: 12.0, ..f.clone() };
        assert!(with_dealt.summary().contains("12.0 dealt, 8.5 taken"));
        let none = Fight::default();
        assert_eq!(none.accuracy(), None);
        assert!(!none.is_worth_keeping());
        assert!(Fight { died: true, ..Fight::default() }.is_worth_keeping());
        assert_eq!(Fight { swings: 3, hits: 9, ..Fight::default() }.accuracy(), Some(100), "never over 100");
    }

    #[test]
    fn a_fight_closes_its_seconds_and_counts_opponents() {
        let mut live = Live::new();
        live.fight.swings = 5;
        live.bucket = 5;
        live.opponents.entry("zombie".into()).or_default().extend([1, 2, 3, 3]);
        live.opponents.entry("skeleton".into()).or_default().insert(9);
        live.bucket_start -= Duration::from_secs(2);
        let fight = live.finish(false);
        assert_eq!(fight.cps, vec![5, 0], "two seconds passed: five clicks, then none");
        assert_eq!(fight.peak_cps, 5);
        assert_eq!(fight.opponents, vec!["skeleton".to_owned(), "zombie x3".to_owned()]);
        assert!(!fight.died);
    }

    #[test]
    fn the_graph_keeps_only_the_last_seconds() {
        let mut live = Live::new();
        live.fight.cps = (0..100).map(|i| i as u8).collect();
        let fight = live.finish(true);
        assert_eq!(fight.cps.len(), GRAPH);
        assert_eq!(fight.cps.last(), Some(&99), "the newest seconds stay");
        assert!(fight.died);
    }

    #[test]
    fn a_fight_round_trips_through_json() {
        let f = Fight { start_unix: 5, swings: 2, cps: vec![1, 2], opponents: vec!["player".into()], ..Fight::default() };
        let back: Vec<Fight> = serde_json::from_str(&serde_json::to_string(&[f.clone()]).unwrap()).unwrap();
        assert_eq!(back, vec![f]);
    }

    #[test]
    fn swings_queue_up_and_are_capped() {
        let c = Combat::default();
        for i in 0..400 {
            c.note_swing(i);
        }
        assert_eq!(c.swings.lock().unwrap().len(), 256);
    }
}
