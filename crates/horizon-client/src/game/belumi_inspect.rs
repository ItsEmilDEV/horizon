//! The Inspect HUD module (what the mod Jade does): a few lines about what the crosshair points at. A block: its name, how
//! hard it is and whether it needs a tool, the state that matters (crop age, signal strength, light level) and what a chest
//! you opened held. A creature: its name and health.
use super::Game;
use std::collections::BTreeMap;

const WHITE: u32 = 0x00FF_FFFF;
const GREY: u32 = 0x00AA_B3C6;
const GREEN: u32 = 0x004D_FF6A;
const YELLOW: u32 = 0x00FF_E14D;
const RED: u32 = 0x00FF_6B6B;

/// The lines for a block: `name` as the player reads it, `hardness` (negative: unbreakable), whether it `needs_tool` to give
/// drops, the interesting `properties`, and what a chest of it was last seen to hold.
pub(super) fn block_lines(name: &str, hardness: f32, needs_tool: bool, properties: &BTreeMap<String, String>, held: &[(String, u32)]) -> Vec<(String, u32)> {
    let mut lines = vec![(name.to_owned(), WHITE)];
    if hardness < 0.0 {
        lines.push(("Unbreakable".to_owned(), RED));
    } else {
        let tool = if needs_tool { ", needs a tool" } else { "" };
        lines.push((format!("Hardness {hardness:.1}{tool}"), GREY));
    }
    let prop = |k: &str| properties.get(k).map(String::as_str);
    if let Some(age) = prop("age") {
        lines.push((format!("Growth {age}"), GREEN));
    }
    if let Some(power) = prop("power") {
        lines.push((format!("Signal {power}"), YELLOW));
    }
    if let Some(level) = prop("level").filter(|_| properties.contains_key("level")) {
        lines.push((format!("Level {level}"), GREY));
    }
    if let Some(l) = prop("lit").filter(|v| *v == "true") {
        let _ = l;
        lines.push(("Lit".to_owned(), YELLOW));
    }
    if !held.is_empty() {
        let total: u32 = held.iter().map(|(_, n)| n).sum();
        lines.push((format!("Inside: {total} items"), GREY));
        for (item, n) in held.iter().take(3) {
            lines.push((format!("{n} x {}", item.strip_prefix("minecraft:").unwrap_or(item).replace('_', " ")), WHITE));
        }
    }
    lines
}

/// The lines for a creature.
pub(super) fn mob_lines(name: &str, health: f32, max: f32) -> Vec<(String, u32)> {
    let frac = if max > 0.0 { (health / max).clamp(0.0, 1.0) } else { 0.0 };
    let color = if frac > 0.6 { GREEN } else if frac > 0.3 { YELLOW } else { RED };
    vec![(name.to_owned(), WHITE), (format!("Health {health:.1} / {max:.0}"), color)]
}

/// The most of an item first, then by name, merged.
pub(super) fn top_items(items: &[(String, u32)]) -> Vec<(String, u32)> {
    let mut merged: BTreeMap<&str, u32> = BTreeMap::new();
    for (item, n) in items {
        *merged.entry(item).or_default() += n;
    }
    let mut list: Vec<(String, u32)> = merged.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list
}

impl Game<'_> {
    /// The lines of the Inspect module: the creature or the block in the crosshair.
    pub(super) fn belumi_inspect_lines(&mut self) -> Option<Vec<(String, u32)>> {
        if let Some((i, d)) = self.mob_hit()
            && self.before_block(d)
            && let Some(m) = self.mobs.get(i)
        {
            let id = m.kind.id.clone();
            let name = m.custom_name.clone().unwrap_or_else(|| {
                let key = format!("entity.{}", id.replacen(':', ".", 1));
                horizon_client::lang::get_or(&key, id.strip_prefix("minecraft:").unwrap_or(&id))
            });
            return Some(mob_lines(&name, m.health, m.kind.max_health));
        }
        let hit = self.pick()?;
        let state = self.world.block(hit.pos[0], hit.pos[1], hit.pos[2]);
        if self.res.facts().is_air(state) {
            return None;
        }
        let (block, hardness, needs_tool) = {
            let f = self.res.facts().get(state);
            (f.name.to_string(), f.hardness, f.requires_tool)
        };
        let key = format!("block.{}", block.replacen(':', ".", 1));
        let name = horizon_client::lang::get_or(&key, block.strip_prefix("minecraft:").unwrap_or(&block));
        let properties = self.pack.assets.registry.state(state).map(|s| s.properties.clone()).unwrap_or_default();
        let held = self.belumi_chest_contents(hit.pos);
        Some(block_lines(&name, hardness, needs_tool, &properties, &top_items(&held)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(list: &[(&str, &str)]) -> BTreeMap<String, String> {
        list.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn a_block_shows_its_name_hardness_and_the_state_that_matters() {
        let l = block_lines("Wheat Crops", 0.0, false, &props(&[("age", "5")]), &[]);
        assert_eq!(l[0].0, "Wheat Crops");
        assert_eq!(l[1].0, "Hardness 0.0");
        assert!(l.iter().any(|(t, _)| t == "Growth 5"));
        let stone = block_lines("Iron Ore", 3.0, true, &props(&[]), &[]);
        assert_eq!(stone[1].0, "Hardness 3.0, needs a tool");
        let wire = block_lines("Redstone Dust", 0.0, false, &props(&[("power", "12")]), &[]);
        assert!(wire.iter().any(|(t, _)| t == "Signal 12"));
        assert_eq!(block_lines("Bedrock", -1.0, false, &props(&[]), &[])[1].0, "Unbreakable");
    }

    #[test]
    fn a_remembered_chest_lists_its_biggest_stacks() {
        let items = top_items(&[("minecraft:apple".into(), 3), ("minecraft:diamond".into(), 9), ("minecraft:apple".into(), 4), ("minecraft:stick".into(), 1)]);
        assert_eq!(items[0], ("minecraft:diamond".to_owned(), 9));
        assert_eq!(items[1], ("minecraft:apple".to_owned(), 7));
        let l = block_lines("Chest", 2.5, false, &props(&[]), &items);
        assert!(l.iter().any(|(t, _)| t == "Inside: 17 items"));
        assert!(l.iter().any(|(t, _)| t == "9 x diamond"));
        assert_eq!(l.iter().filter(|(t, _)| t.contains(" x ")).count(), 3, "three at most");
    }

    #[test]
    fn a_creature_shows_its_health_in_a_colour_that_follows_it() {
        let full = mob_lines("Zombie", 20.0, 20.0);
        assert_eq!(full[1].0, "Health 20.0 / 20");
        assert_eq!(full[1].1, GREEN);
        assert_eq!(mob_lines("Zombie", 4.0, 20.0)[1].1, RED);
        assert_eq!(mob_lines("Zombie", 10.0, 20.0)[1].1, YELLOW);
        assert_eq!(mob_lines("Odd", 5.0, 0.0)[1].1, RED, "no maximum known");
    }
}
