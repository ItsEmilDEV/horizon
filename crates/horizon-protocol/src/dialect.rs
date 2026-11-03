use crate::dialect_26_2 as t;
use std::sync::atomic::{AtomicBool, Ordering};

static MINECRAFT_26_2: AtomicBool = AtomicBool::new(false);

pub fn set_26_2(on: bool) {
    MINECRAFT_26_2.store(on, Ordering::Relaxed);
}
pub fn is_26_2() -> bool {
    MINECRAFT_26_2.load(Ordering::Relaxed)
}

pub fn lookup(table: &[i32], id: i32) -> i32 {
    usize::try_from(id)
        .ok()
        .and_then(|i| table.get(i))
        .copied()
        .filter(|&mapped| mapped >= 0)
        .unwrap_or(id)
}

fn map(table: &[i32], id: i32) -> i32 {
    if is_26_2() { lookup(table, id) } else { id }
}

pub fn item_in(id: i32) -> i32 {
    map(&t::ITEM_FROM_26_2, id)
}
pub fn item_out(id: i32) -> i32 {
    map(&t::ITEM_TO_26_2, id)
}
pub fn component_in(id: i32) -> i32 {
    map(&t::DATA_COMPONENT_TYPE_FROM_26_2, id)
}
pub fn component_out(id: i32) -> i32 {
    map(&t::DATA_COMPONENT_TYPE_TO_26_2, id)
}
pub fn particle_in(id: i32) -> i32 {
    map(&t::PARTICLE_TYPE_FROM_26_2, id)
}
pub fn sound_event_in(id: i32) -> i32 {
    map(&t::SOUND_EVENT_FROM_26_2, id)
}
pub fn command_argument_in(id: i32) -> i32 {
    map(&t::COMMAND_ARGUMENT_TYPE_FROM_26_2, id)
}
pub fn block_state_in(id: i32) -> i32 {
    map(&t::BLOCK_STATE_FROM_26_2, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_leaves_unknown_and_unmapped_ids_alone() {
        let table = [0, 2, -1];
        assert_eq!(lookup(&table, 1), 2);
        assert_eq!(lookup(&table, 2), 2);
        assert_eq!(lookup(&table, 9), 9);
        assert_eq!(lookup(&table, -4), -4);
    }

    #[test]
    fn tables_cover_the_known_shifts() {
        assert_eq!(lookup(&t::ENTITY_TYPE_FROM_26_2, 117), 120);
        assert_ne!(lookup(&t::BLOCK_STATE_FROM_26_2, 100), 100);
        for id in 0..t::ITEM_FROM_26_2.len() as i32 {
            assert_eq!(lookup(&t::ITEM_TO_26_2, lookup(&t::ITEM_FROM_26_2, id)), id);
        }
        for id in 0..t::DATA_COMPONENT_TYPE_FROM_26_2.len() as i32 {
            let native = lookup(&t::DATA_COMPONENT_TYPE_FROM_26_2, id);
            if native != id || t::DATA_COMPONENT_TYPE_FROM_26_2[id as usize] >= 0 {
                assert_eq!(lookup(&t::DATA_COMPONENT_TYPE_TO_26_2, native), id);
            }
        }
    }
}
