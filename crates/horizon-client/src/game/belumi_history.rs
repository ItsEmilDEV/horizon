//! The world's history (single player): `SaveDir` keeps older versions of every chunk it overwrites (see `save.rs`), and
//! `/rollback RADIUS MINUTES` asks for the chunks around you to go back to how they were. The chunks of an open world live
//! in memory, so the rollback is applied when the dimension loads next: save and quit, open the world again.
use super::Game;

const MAX_RADIUS: i32 = 16;
const MAX_MINUTES: u64 = 7 * 24 * 60;

/// The chunks within `radius` of the chunk `centre`.
pub(super) fn square(centre: (i32, i32), radius: i32) -> Vec<(i32, i32)> {
    (-radius..=radius).flat_map(|dx| (-radius..=radius).map(move |dz| (centre.0 + dx, centre.1 + dz))).collect()
}

/// `/rollback list` or `/rollback RADIUS MINUTES`.
pub(super) fn parse(args: &str) -> Option<Option<(i32, u64)>> {
    let mut it = args.split_whitespace();
    let first = it.next()?;
    if first.eq_ignore_ascii_case("list") && it.next().is_none() {
        return Some(None);
    }
    let radius: i32 = first.parse().ok().filter(|r| (0..=MAX_RADIUS).contains(r))?;
    let minutes: u64 = it.next()?.parse().ok().filter(|m| (1..=MAX_MINUTES).contains(m))?;
    it.next().is_none().then_some(Some((radius, minutes)))
}

fn age(secs: u64) -> String {
    match secs {
        0..=119 => format!("{secs} s"),
        120..=7199 => format!("{} min", secs / 60),
        7200..=172_799 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86_400),
    }
}

impl Game<'_> {
    pub(super) fn belumi_history_command(&mut self, command: &str, ran: &mut bool) {
        let (head, rest) = match command.trim().split_once(char::is_whitespace) {
            Some((h, r)) => (h, r),
            None => (command.trim(), ""),
        };
        if *ran || !head.eq_ignore_ascii_case("rollback") {
            return;
        }
        *ran = true;
        let Some(server) = self.integrated.as_ref() else {
            self.native_chat("Rollback works in single player worlds (the world's history is kept on your disk).");
            return;
        };
        let Some(what) = parse(rest) else {
            self.native_chat("Usage: /rollback list   or   /rollback RADIUS MINUTES (radius in chunks, up to 16).");
            return;
        };
        match what {
            None => {
                let Some((chunks, versions, oldest)) = server.call(|g| g.save.history_summary()) else {
                    self.native_chat("The world did not answer.");
                    return;
                };
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                match oldest {
                    Some(t) => self.native_chat(&format!("History of this dimension: {versions} versions of {chunks} chunks, back {}.", age(now.saturating_sub(t)))),
                    None => self.native_chat("No history yet: a chunk gets one when it is saved again after a change."),
                }
            }
            Some((radius, minutes)) => {
                let Some(p) = self.player.as_ref() else { return };
                let centre = ((p.position.x.floor() as i32) >> 4, (p.position.z.floor() as i32) >> 4);
                let chunks = square(centre, radius);
                let total = chunks.len();
                match server.call(move |g| g.save.history_request(&chunks, minutes).map_err(|e| e.to_string())) {
                    Some(Ok(0)) => self.native_chat("None of those chunks has an older version from that time."),
                    Some(Ok(n)) => self.native_chat(&format!(
                        "{n} of {total} chunks go back to {minutes} min ago. Save and quit, then open the world again to get them back; what they are now stays in the history."
                    )),
                    _ => self.native_chat("The rollback could not be prepared."),
                }
            }
        }
    }

    /// Turns the world history on or off from the modules' switch (a flag the saving code reads).
    pub(super) fn belumi_history_switch(&mut self) {
        let on = self.modules_ref().is_none_or(|m| m.misc.world_history);
        horizon_client::save::set_history_enabled(on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_read() {
        assert_eq!(parse("list"), Some(None));
        assert_eq!(parse("4 30"), Some(Some((4, 30))));
        assert_eq!(parse("0 1"), Some(Some((0, 1))));
        for bad in ["", "list now", "17 30", "-1 30", "4", "4 0", "4 30 5", "a b", "4 99999"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_square_has_every_chunk_once() {
        let s = square((10, -3), 2);
        assert_eq!(s.len(), 25);
        assert!(s.contains(&(8, -5)) && s.contains(&(12, -1)) && s.contains(&(10, -3)));
        assert_eq!(square((0, 0), 0), vec![(0, 0)]);
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age(30), "30 s");
        assert_eq!(age(600), "10 min");
        assert_eq!(age(7200), "2 h");
        assert_eq!(age(3 * 86_400), "3 d");
    }
}
