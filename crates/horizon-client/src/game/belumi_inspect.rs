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

/// How long breaking takes, in seconds, by the vanilla rule: the block's hardness, the tool's speed, and 30 against 100 for
/// whether the tool is right for the drops. None for what cannot be broken or breaks at once.
pub(super) fn break_seconds(hardness: f32, speed: f32, correct: bool) -> Option<f32> {
    if hardness <= 0.0 || speed <= 0.0 {
        return None;
    }
    let per_tick = speed / hardness / if correct { 30.0 } else { 100.0 };
    if per_tick >= 1.0 {
        return None;
    }
    Some((1.0 / per_tick).ceil() / 20.0)
}

/// The lines about the tool in hand and the block: whether it drops anything with it and how long the break takes.
pub(super) fn tool_lines(needs_tool: bool, correct: bool, seconds: Option<f32>) -> Vec<(String, u32)> {
    let mut lines = Vec::new();
    if needs_tool {
        lines.push(if correct { ("Drops with your tool".to_owned(), GREEN) } else { ("No drops with your tool".to_owned(), RED) });
    }
    if let Some(s) = seconds {
        lines.push((format!("Breaks in {s:.1} s"), GREY));
    }
    lines
}

/// The lines for a player: the name, and that it is a player.
pub(super) fn player_lines(name: &str) -> Vec<(String, u32)> {
    vec![(name.to_owned(), WHITE), ("Player".to_owned(), GREY)]
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
    /// The lines of the Inspect module: the creature, object or block in the crosshair, as far as the module's range reaches
    /// (further than the game's own reach, which is for hitting and placing).
    pub(super) fn belumi_inspect_lines(&mut self) -> Option<Vec<(String, u32)>> {
        let range = f64::from(self.modules_ref().map_or(20, |m| m.misc.inspect_range).clamp(4, 64));
        let (eye, dir) = self.aim()?;
        // The block the ray reaches, and how far.
        let hit = self.clip_ray(eye, dir, range);
        let block_distance = hit.map(|h| (0..3).map(|a| (h.point[a] - eye[a]).powi(2)).sum::<f64>().sqrt());
        // The nearest creature on the ray, before any block.
        let mob = self
            .mobs
            .iter()
            .enumerate()
            .filter(|(_, m)| m.alive() && Some(m.id) != self.riding)
            .filter_map(|(i, m)| Some((i, super::ray_box(eye, dir, range, m.bounds())?)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, d)| block_distance.is_none_or(|b| *d <= b));
        if let Some((i, _)) = mob
            && let Some(m) = self.mobs.get(i)
        {
            let id = m.kind.id.clone();
            if id == "minecraft:player" {
                let name = self.remote.as_ref().and_then(|r| r.player_name(m.id)).or_else(|| m.custom_name.clone()).unwrap_or_else(|| "Player".to_owned());
                return Some(player_lines(&name));
            }
            let name = m.custom_name.clone().unwrap_or_else(|| {
                let key = format!("entity.{}", id.replacen(':', ".", 1));
                horizon_client::lang::get_or(&key, id.strip_prefix("minecraft:").unwrap_or(&id))
            });
            return Some(mob_lines(&name, m.health, m.kind.max_health));
        }
        // The nearest of the objects the game keeps apart from creatures: paintings, frames, armor stands, boats, carts (these
        // are found within the game's own reach).
        let objects = [
            self.painting_hit().map(|h| (h.1, "painting")),
            self.frame_hit().map(|h| (h.1, "item_frame")),
            self.armor_stand_hit().map(|h| (h.1, "armor_stand")),
            self.boat_hit().map(|h| (h.1, "boat")),
            self.cart_hit().map(|h| (h.1, "minecart")),
        ];
        if let Some((d, kind)) = objects.into_iter().flatten().min_by(|a, b| a.0.total_cmp(&b.0))
            && block_distance.is_none_or(|b| d <= b)
        {
            let name = horizon_client::lang::get_or(&format!("entity.minecraft.{kind}"), kind);
            return Some(vec![(name, WHITE)]);
        }
        let hit = hit?;
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
        let mut lines = block_lines(&name, hardness, needs_tool, &properties, &top_items(&held));
        // The tool in hand against this block.
        let stack = self.held().cloned();
        let (correct, speed) = {
            let items = self.res.items();
            (items.correct_for_drops(stack.as_ref(), &block), items.mining_speed(stack.as_ref(), &block))
        };
        let seconds = if self.creative { None } else { break_seconds(hardness, speed, correct || !needs_tool) };
        let extra = tool_lines(needs_tool, correct, seconds);
        let at = lines.len().min(2);
        lines.splice(at..at, extra);
        Some(lines)
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
    fn breaking_time_follows_the_vanilla_rule() {
        // Stone (hardness 1.5) by hand needs a tool for drops: 1.5 * 100 / 1 ticks... = 150 ticks = 7.5 s.
        assert_eq!(break_seconds(1.5, 1.0, false), Some(7.5));
        // With a wooden pickaxe (speed 2, right tool): 1.5 * 30 / 2 = 22.5 ticks, rounded up to 23 = 1.15 s.
        let s = break_seconds(1.5, 2.0, true).unwrap();
        assert!((s - 1.15).abs() < 1e-5, "{s}");
        assert_eq!(break_seconds(0.0, 1.0, true), None, "plants and torches break at once");
        assert_eq!(break_seconds(-1.0, 8.0, true), None, "bedrock");
        assert_eq!(break_seconds(0.5, 40.0, true), None, "instant");
    }

    #[test]
    fn the_tool_lines_say_whether_it_drops_and_how_long_it_takes() {
        let l = tool_lines(true, false, Some(7.5));
        assert_eq!(l[0], ("No drops with your tool".to_owned(), RED));
        assert_eq!(l[1].0, "Breaks in 7.5 s");
        assert_eq!(tool_lines(true, true, None)[0].1, GREEN);
        assert!(tool_lines(false, false, None).is_empty());
        assert_eq!(player_lines("Alex")[0].0, "Alex");
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
