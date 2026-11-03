use std::borrow::Cow;

use horizon_protocol::{
    State,
    buf::{Codec, Reader, Writer},
    ids::{
        configuration::clientbound as cfg_clientbound,
        play::{clientbound as play_clientbound, serverbound as play_serverbound},
    },
    packets::{
        configuration::UpdateTags,
        play::{AddEntity, AwardStats, LevelEvent, UpdateAdvancements},
        recipes::RecipeBookAdd,
    },
};

use horizon_protocol::{
    chunk::{ChunkData, Section, Strategy},
    dialect::lookup,
    dialect_26_2 as tables,
    packets::play::{BlockUpdate, LevelChunkWithLight, SectionBlocksUpdate},
};

pub const NATIVE_PROTOCOL: i32 = 777;
pub const MINECRAFT_26_2_PROTOCOL: i32 = 776;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProtocolVersion(i32);

impl ProtocolVersion {
    pub const NATIVE: Self = Self(NATIVE_PROTOCOL);
    pub const MINECRAFT_26_2: Self = Self(MINECRAFT_26_2_PROTOCOL);

    pub const fn new(raw: i32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> i32 {
        self.0
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProtocolSelection {
    #[default]
    Auto,
    Native,
    Explicit(ProtocolVersion),
}

impl ProtocolSelection {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "native" => Ok(Self::Native),
            _ => value
                .parse::<i32>()
                .map(|raw| match raw {
                    NATIVE_PROTOCOL => Self::Native,
                    _ => Self::Explicit(ProtocolVersion::new(raw)),
                })
                .map_err(|_| {
                    format!("protocol must be auto, native or a protocol number, not {value:?}")
                }),
        }
    }

    pub fn from_env() -> Result<Option<Self>, String> {
        let Some(value) = std::env::var_os("BELUMI_TARGET_PROTOCOL") else {
            return Ok(None);
        };
        let value = value
            .to_str()
            .ok_or_else(|| "BELUMI_TARGET_PROTOCOL is not valid Unicode".to_owned())?;
        if value.trim().is_empty() {
            return Ok(None);
        }
        Self::parse(value)
            .map(Some)
            .map_err(|e| format!("BELUMI_TARGET_PROTOCOL: {e}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolCompat {
    protocol: ProtocolVersion,
}

impl ProtocolCompat {
    pub const fn native() -> Self {
        Self {
            protocol: ProtocolVersion::NATIVE,
        }
    }

    pub fn for_version(protocol: ProtocolVersion) -> Result<Self, String> {
        match protocol.raw() {
            NATIVE_PROTOCOL | MINECRAFT_26_2_PROTOCOL => Ok(Self { protocol }),
            other => Err(format!(
                "unsupported Minecraft protocol {other}; this client supports \
                 {NATIVE_PROTOCOL} (26.3) and {MINECRAFT_26_2_PROTOCOL} (26.2)"
            )),
        }
    }

    pub const fn protocol(self) -> ProtocolVersion {
        self.protocol
    }

    pub fn clientbound_id(self, state: State, id: i32) -> i32 {
        if self.protocol != ProtocolVersion::MINECRAFT_26_2 {
            return id;
        }
        match state {
            State::Configuration => verified_26_2_configuration_clientbound_id(id),
            State::Play => verified_26_2_play_clientbound_id(id),
            _ => id,
        }
    }

    pub fn clientbound_payload<'a>(
        self,
        state: State,
        id: i32,
        payload: &'a [u8],
    ) -> Cow<'a, [u8]> {
        if self.protocol != ProtocolVersion::MINECRAFT_26_2 {
            return Cow::Borrowed(payload);
        }
        if state == State::Configuration {
            let rewritten = match id {
                cfg_clientbound::UPDATE_TAGS => rewrite_26_2_update_tags(payload),
                _ => None,
            };
            return rewritten.map_or(Cow::Borrowed(payload), Cow::Owned);
        }
        if state != State::Play {
            return Cow::Borrowed(payload);
        }
        let rewritten = match id {
            play_clientbound::ADD_ENTITY => rewrite_26_2_add_entity(payload),
            play_clientbound::LEVEL_PARTICLES => rewrite_26_2_level_particles(payload),
            play_clientbound::EXPLODE => Some([payload, &[1]].concat()),
            play_clientbound::BLOCK_UPDATE => rewrite_26_2_block_update(payload),
            play_clientbound::AWARD_STATS => rewrite_26_2_award_stats(payload),
            play_clientbound::SECTION_BLOCKS_UPDATE => rewrite_26_2_section_blocks_update(payload),
            play_clientbound::LEVEL_EVENT => rewrite_26_2_level_event(payload),
            play_clientbound::LEVEL_CHUNK_WITH_LIGHT => rewrite_26_2_level_chunk(payload),
            play_clientbound::LOGIN => rewrite_26_2_login(payload),
            play_clientbound::RESPAWN => rewrite_26_2_respawn(payload),
            play_clientbound::RECIPE_BOOK_ADD => rewrite_26_2_recipe_book_add(payload),
            play_clientbound::UPDATE_ADVANCEMENTS => rewrite_26_2_update_advancements(payload),
            play_clientbound::ENTITY_POSITION_SYNC => rewrite_26_2_entity_position_sync(payload),
            play_clientbound::MOVE_ENTITY_POS => rewrite_26_2_move_entity(payload, true, false),
            play_clientbound::MOVE_ENTITY_POS_ROT => rewrite_26_2_move_entity(payload, true, true),
            play_clientbound::MOVE_ENTITY_ROT => rewrite_26_2_move_entity(payload, false, true),
            _ => None,
        };
        rewritten.map_or(Cow::Borrowed(payload), Cow::Owned)
    }

    pub fn clientbound_retarget(self, state: State, id: i32, payload: Vec<u8>) -> (i32, Vec<u8>) {
        if state != State::Play
            || self.protocol != ProtocolVersion::MINECRAFT_26_2
            || id != play_clientbound::ANIMATE
        {
            return (id, payload);
        }
        retarget_26_2_animate(&payload).unwrap_or((id, payload))
    }

    pub fn serverbound_id(self, state: State, id: i32) -> i32 {
        if state != State::Play || self.protocol != ProtocolVersion::MINECRAFT_26_2 {
            return id;
        }
        match id {
            play_serverbound::PUNCH => MINECRAFT_26_2_PLAY_SWING,
            47..=63 => id - 1,
            _ => id,
        }
    }

    pub fn encode_serverbound(self, state: State, body: &[u8]) -> Vec<u8> {
        let mut reader = Reader::new(body);
        let Ok(id) = reader.varint() else {
            return body.to_vec();
        };
        let mapped = self.serverbound_id(state, id);
        let is_26_2_play = state == State::Play && self.protocol == ProtocolVersion::MINECRAFT_26_2;
        let add_swing_hand = is_26_2_play && id == play_serverbound::PUNCH;
        let strip_teleport = is_26_2_play && id == play_serverbound::ACCEPT_TELEPORTATION;
        if is_26_2_play && id == play_serverbound::PLAYER_ACTION {
            return remap_26_2_player_action(mapped, reader.rest());
        }
        if is_26_2_play && id == play_serverbound::SIGN_UPDATE {
            if let Some(out) = reorder_26_2_sign_update(mapped, reader.rest()) {
                return out;
            }
        }
        if mapped == id && !add_swing_hand && !strip_teleport {
            return body.to_vec();
        }
        let rest = reader.rest();
        let mut writer = Writer::new();
        writer.varint(mapped);
        if add_swing_hand {
            writer.varint(0);
            writer.bytes(rest);
        } else if strip_teleport {
            let mut payload = Reader::new(rest);
            if let Ok(teleport_id) = payload.varint() {
                writer.varint(teleport_id);
            } else {
                writer.bytes(rest);
            }
        } else {
            writer.bytes(rest);
        }
        writer.data
    }
}

fn reorder_26_2_sign_update(id: i32, rest: &[u8]) -> Option<Vec<u8>> {
    let mut payload = Reader::new(rest);
    let pos = payload.bytes(8).ok()?.to_vec();
    let mut lines = Vec::new();
    for _ in 0..4 {
        lines.push(payload.string(384).ok()?);
    }
    let front = payload.varint().ok()? == 1;
    let mut writer = Writer::new();
    writer.varint(id);
    writer.bytes(&pos);
    writer.u8(u8::from(front));
    for line in &lines {
        writer.string(line);
    }
    Some(writer.data)
}

fn remap_26_2_player_action(id: i32, rest: &[u8]) -> Vec<u8> {
    let mut payload = Reader::new(rest);
    let mut writer = Writer::new();
    writer.varint(id);
    match payload.varint() {
        Ok(action) => {
            writer.varint(match action {
                0 | 1 => 0,
                other => other - 1,
            });
            writer.bytes(payload.rest());
        }
        Err(_) => writer.bytes(rest),
    }
    writer.data
}

const MINECRAFT_26_2_PLAY_SWING: i32 = 63;

fn rewrite_26_2_add_entity(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let mut packet = <AddEntity as Codec<AddEntity>>::read(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    packet.kind = lookup(&tables::ENTITY_TYPE_FROM_26_2, packet.kind);
    let mut writer = Writer::new();
    <AddEntity as Codec<AddEntity>>::write(&packet, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_update_tags(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let mut packet = <UpdateTags as Codec<UpdateTags>>::read(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    for registry in &mut packet.registries {
        let table: &[i32] = match registry.registry.as_str() {
            "minecraft:entity_type" => &tables::ENTITY_TYPE_FROM_26_2,
            "minecraft:block" => &tables::BLOCK_FROM_26_2,
            "minecraft:item" => &tables::ITEM_FROM_26_2,
            _ => continue,
        };
        for tag in &mut registry.tags {
            for id in &mut tag.ids {
                *id = lookup(table, *id);
            }
        }
    }
    let mut writer = Writer::new();
    <UpdateTags as Codec<UpdateTags>>::write(&packet, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_award_stats(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let mut packet = <AwardStats as Codec<AwardStats>>::read(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    for (kind, value, _) in &mut packet.stats {
        let table: &[i32] = match *kind {
            0 => &tables::BLOCK_FROM_26_2,
            1..=5 => &tables::ITEM_FROM_26_2,
            6 | 7 => &tables::ENTITY_TYPE_FROM_26_2,
            8 => &tables::CUSTOM_STAT_FROM_26_2,
            _ => continue,
        };
        *value = lookup(table, *value);
    }
    let mut writer = Writer::new();
    <AwardStats as Codec<AwardStats>>::write(&packet, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_block_update(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let mut packet = <BlockUpdate as Codec<BlockUpdate>>::read(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    packet.state = lookup(&tables::BLOCK_STATE_FROM_26_2, packet.state);
    let mut writer = Writer::new();
    <BlockUpdate as Codec<BlockUpdate>>::write(&packet, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_section_blocks_update(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let mut packet = <SectionBlocksUpdate as Codec<SectionBlocksUpdate>>::read(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    for change in &mut packet.changes {
        let state = i32::try_from(*change >> 12).ok()?;
        let state = i64::from(lookup(&tables::BLOCK_STATE_FROM_26_2, state));
        *change = (state << 12) | (*change & 0xfff);
    }
    let mut writer = Writer::new();
    <SectionBlocksUpdate as Codec<SectionBlocksUpdate>>::write(&packet, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_level_event(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let mut packet = <LevelEvent as Codec<LevelEvent>>::read(&mut reader).ok()?;
    if reader.remaining() != 0 || packet.kind != 2001 {
        return None;
    }
    packet.data = lookup(&tables::BLOCK_STATE_FROM_26_2, packet.data);
    let mut writer = Writer::new();
    <LevelEvent as Codec<LevelEvent>>::write(&packet, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_level_chunk(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let mut packet = <LevelChunkWithLight as Codec<LevelChunkWithLight>>::read(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    let sections = rewrite_26_2_sections(&packet.chunk)?;
    packet.chunk.buffer = ChunkData::from_sections(&sections);
    let mut writer = Writer::new();
    <LevelChunkWithLight as Codec<LevelChunkWithLight>>::write(&packet, &mut writer);
    Some(writer.data)
}

const NATIVE_BLOCK_STATE_BITS: u8 = 16;

fn rewrite_26_2_sections(chunk: &ChunkData) -> Option<Vec<Section>> {
    let mut reader = Reader::new(&chunk.buffer);
    let mut sections = Vec::new();
    while reader.remaining() > 0 {
        let mut section = Section::read(&mut reader, 0, 0).ok()?;
        let values: Vec<i32> = (0..horizon_protocol::chunk::BLOCKS)
            .map(|i| lookup(&tables::BLOCK_STATE_FROM_26_2, section.states.get(i)))
            .collect();
        section.states = horizon_protocol::chunk::Paletted::from_values(
            &values,
            Strategy::Blocks,
            NATIVE_BLOCK_STATE_BITS,
        );
        sections.push(section);
    }
    Some(sections)
}

fn retarget_26_2_animate(payload: &[u8]) -> Option<(i32, Vec<u8>)> {
    let mut reader = Reader::new(payload);
    let entity = reader.varint().ok()?;
    let action = reader.u8().ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    let mut writer = Writer::new();
    writer.varint(entity);
    match action {
        0 | 3 => {
            writer.varint(i32::from(action == 3));
            writer.varint(1);
            writer.varint(6);
            Some((play_clientbound::SWING_ANIMATION, writer.data))
        }
        2 | 4 | 5 => {
            writer.u8(match action {
                2 => 0,
                4 => 1,
                _ => 2,
            });
            Some((play_clientbound::ANIMATE, writer.data))
        }
        _ => None,
    }
}

fn rewrite_26_2_level_particles(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let override_limiter = reader.bool().ok()?;
    let always_show = reader.bool().ok()?;
    let pos = [reader.f64().ok()?, reader.f64().ok()?, reader.f64().ok()?];
    let dist = [reader.f32().ok()?, reader.f32().ok()?, reader.f32().ok()?];
    let speed = reader.f32().ok()?;
    let count = reader.i32().ok()?;
    let particle = reader.rest();
    if particle.is_empty() {
        return None;
    }
    let mut writer = Writer::new();
    writer.bytes(particle);
    writer.bool(override_limiter);
    writer.bool(always_show);
    for v in pos {
        writer.f64(v);
    }
    for v in dist {
        writer.f32(v);
    }
    for _ in 0..3 {
        writer.f32(speed);
    }
    writer.varint(count);
    writer.varint(0);
    Some(writer.data)
}

fn rewrite_26_2_login(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    reader.i32().ok()?;
    reader.bool().ok()?;
    let levels = usize::try_from(reader.varint().ok()?).ok()?;
    if levels > 1024 {
        return None;
    }
    for _ in 0..levels {
        reader.string(32767).ok()?;
    }
    reader.varint().ok()?;
    reader.varint().ok()?;
    reader.varint().ok()?;
    reader.bool().ok()?;
    reader.bool().ok()?;
    reader.bool().ok()?;
    reader.varint().ok()?;
    reader.string(32767).ok()?;
    reader.i64().ok()?;
    reader.u8().ok()?;
    let previous_at = reader.position();
    let previous = reader.u8().ok()?;

    let mut writer = Writer::new();
    writer.bytes(&payload[..previous_at]);
    if previous == 0xff {
        writer.varint(0);
    } else {
        writer.varint(i32::from(previous) + 1);
    }
    writer.bytes(&payload[reader.position()..]);
    Some(writer.data)
}

fn rewrite_26_2_respawn(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    reader.varint().ok()?;
    reader.string(32767).ok()?;
    reader.i64().ok()?;
    reader.u8().ok()?;
    let previous_at = reader.position();
    let previous = reader.u8().ok()?;
    let mut writer = Writer::new();
    writer.bytes(&payload[..previous_at]);
    if previous == 0xff {
        writer.varint(0);
    } else {
        writer.varint(i32::from(previous) + 1);
    }
    writer.bytes(&payload[reader.position()..]);
    Some(writer.data)
}

fn rewrite_26_2_recipe_book_add(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let add = RecipeBookAdd::read_26_2(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    let mut writer = Writer::new();
    <RecipeBookAdd as Codec<RecipeBookAdd>>::write(&add, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_update_advancements(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let update = UpdateAdvancements::read_26_2(&mut reader).ok()?;
    if reader.remaining() != 0 {
        return None;
    }
    let mut writer = Writer::new();
    <UpdateAdvancements as Codec<UpdateAdvancements>>::write(&update, &mut writer);
    Some(writer.data)
}

fn rewrite_26_2_entity_position_sync(payload: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let id = reader.varint().ok()?;
    let mut position = [0.0; 3];
    for value in &mut position {
        *value = reader.f64().ok()?;
    }
    for _ in 0..3 {
        reader.f64().ok()?;
    }
    let yaw = reader.f32().ok()?;
    let pitch = reader.f32().ok()?;
    let on_ground = reader.bool().ok()?;
    if reader.remaining() != 0 {
        return None;
    }

    let mut writer = Writer::new();
    writer.varint(id);
    writer.varint(0);
    for value in position {
        writer.f64(value);
    }
    writer.f32(yaw);
    writer.f32(pitch);
    writer.bool(on_ground);
    Some(writer.data)
}

fn rewrite_26_2_move_entity(payload: &[u8], pos: bool, rot: bool) -> Option<Vec<u8>> {
    let mut reader = Reader::new(payload);
    let id = reader.varint().ok()?;
    let delta = if pos {
        Some([reader.i16().ok()?, reader.i16().ok()?, reader.i16().ok()?])
    } else {
        None
    };
    let rotation = if rot {
        Some((reader.i8().ok()?, reader.i8().ok()?))
    } else {
        None
    };
    let on_ground = reader.bool().ok()?;
    if reader.remaining() != 0 {
        return None;
    }

    let mut writer = Writer::new();
    writer.varint(id);
    if let Some(delta) = delta {
        writer.varint(i32::from(on_ground));
        for value in delta {
            writer.i16(value);
        }
    } else {
        writer.bool(on_ground);
    }
    if let Some((yaw, pitch)) = rotation {
        writer.i8(yaw);
        writer.i8(pitch);
    }
    Some(writer.data)
}

const PLAY_26_2_ADD_TRANSIENT_BLOCK_AT: i32 = play_clientbound::ADD_TRANSIENT_BLOCK;
const PLAY_26_2_POST_EFFECTS_AT: i32 = play_clientbound::POST_EFFECTS - 1;
const PLAY_26_2_SWING_ANIMATION_AT: i32 = play_clientbound::SWING_ANIMATION - 2;
const PLAY_26_2_LAST_ID: i32 = play_clientbound::SHOW_DIALOG - 3;

fn verified_26_2_play_clientbound_id(id: i32) -> i32 {
    if id < PLAY_26_2_ADD_TRANSIENT_BLOCK_AT {
        id
    } else if id < PLAY_26_2_POST_EFFECTS_AT {
        id + 1
    } else if id < PLAY_26_2_SWING_ANIMATION_AT {
        id + 2
    } else if id <= PLAY_26_2_LAST_ID {
        id + 3
    } else {
        id
    }
}

const CONFIGURATION_CLIENTBOUND_26_2: &[(i32, i32)] = &[
    (12, cfg_clientbound::UPDATE_ENABLED_FEATURES),
    (13, cfg_clientbound::UPDATE_TAGS),
    (14, cfg_clientbound::SELECT_KNOWN_PACKS),
];

fn verified_26_2_configuration_clientbound_id(id: i32) -> i32 {
    CONFIGURATION_CLIENTBOUND_26_2
        .iter()
        .find_map(|&(from, to)| (from == id).then_some(to))
        .unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::packets::play::Clientbound;
    use horizon_protocol::packets::{Empty, configuration as cfg, play};

    fn encode_with_id(packet: cfg::Clientbound, id: i32) -> Vec<u8> {
        let body = packet.encode();
        let mut reader = Reader::new(&body);
        reader.varint().unwrap();
        let mut writer = Writer::new();
        writer.varint(id);
        writer.bytes(reader.rest());
        writer.data
    }

    fn decode_mapped(
        packet: cfg::Clientbound,
        id: i32,
        compat: ProtocolCompat,
    ) -> cfg::Clientbound {
        let body = encode_with_id(packet, id);
        let mut reader = Reader::new(&body);
        reader.varint().unwrap();
        cfg::Clientbound::decode(
            compat.clientbound_id(State::Configuration, id),
            reader.rest(),
        )
        .unwrap()
    }

    #[test]
    fn maps_only_the_verified_26_2_configuration_ids() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        assert_eq!(compat.clientbound_id(State::Configuration, 12), 13);
        assert_eq!(compat.clientbound_id(State::Configuration, 13), 14);
        assert_eq!(compat.clientbound_id(State::Configuration, 14), 15);
    }

    #[test]
    fn leaves_unrelated_configuration_ids_alone() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        for id in [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 15, 16, 17, 18, 19, 20, 99,
        ] {
            assert_eq!(compat.clientbound_id(State::Configuration, id), id);
        }
    }

    #[test]
    fn native_26_3_configuration_ids_are_unchanged() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::NATIVE).unwrap();
        for id in 0..=20 {
            assert_eq!(compat.clientbound_id(State::Configuration, id), id);
        }
    }

    #[test]
    fn translation_is_isolated_by_state() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        for state in [State::Handshake, State::Status, State::Login] {
            for id in [12, 13, 14, 37, 44, 82, 121] {
                assert_eq!(compat.clientbound_id(state, id), id, "{state:?} id {id}");
            }
        }
        for id in [12, 13, 14] {
            assert_eq!(compat.clientbound_id(State::Configuration, id), id + 1);
        }
    }

    #[test]
    fn maps_verified_26_2_play_clientbound_ids() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        for (old, new) in [
            (0, 0),
            (36, 36),
            (37, 38),
            (48, 49),
            (49, 50),
            (81, 82),
            (82, 84),
            (120, 122),
            (121, 124),
            (130, 133),
            (140, 143),
        ] {
            assert_eq!(compat.clientbound_id(State::Play, old), new, "id {old}");
        }
    }

    #[test]
    fn native_26_3_play_ids_are_unchanged() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::NATIVE).unwrap();
        for id in 0..=143 {
            assert_eq!(compat.clientbound_id(State::Play, id), id);
        }
    }

    #[test]
    fn decodes_a_26_2_play_keep_alive_after_translation() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let payload = 0x0102_0304_0506_0708i64.to_be_bytes();
        let packet =
            play::Clientbound::decode(compat.clientbound_id(State::Play, 44), &payload).unwrap();
        assert!(matches!(packet, play::Clientbound::KeepAlive(_)));
    }

    fn login_26_2(previous_game_type: u8) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.i32(42);
        writer.bool(false);
        writer.varint(1);
        writer.string("minecraft:overworld");
        writer.varint(20);
        writer.varint(10);
        writer.varint(8);
        writer.bool(false);
        writer.bool(true);
        writer.bool(false);
        writer.varint(1);
        writer.string("minecraft:overworld");
        writer.i64(0x0102_0304_0506_0708);
        writer.u8(0);
        writer.u8(previous_game_type);
        writer.bool(false);
        writer.bool(true);
        writer.bool(false);
        writer.varint(0);
        writer.varint(63);
        writer.bool(false);
        writer.bool(false);
        writer.data
    }

    #[test]
    fn rewrites_26_2_login_previous_game_type() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        for (previous, expected) in [(0xff, None), (1, Some(1)), (3, Some(3))] {
            let payload = login_26_2(previous);
            assert!(rewrite_26_2_login(&payload).is_some(), "rewrite failed");
            let rewritten =
                compat.clientbound_payload(State::Play, play_clientbound::LOGIN, &payload);
            assert!(matches!(rewritten, Cow::Owned(_)), "not rewritten");
            let packet = play::Clientbound::decode(play_clientbound::LOGIN, &rewritten).unwrap();
            let play::Clientbound::Login(login) = packet else {
                panic!("expected Login");
            };
            assert_eq!(login.player_id, 42);
            assert_eq!(login.spawn.previous_game_type, expected);
        }
    }

    #[test]
    fn leaves_native_login_payload_untouched() {
        let payload = login_26_2(0xff);
        let native = ProtocolCompat::for_version(ProtocolVersion::NATIVE).unwrap();
        assert!(matches!(
            native.clientbound_payload(State::Play, play_clientbound::LOGIN, &payload),
            Cow::Borrowed(_)
        ));
    }

    fn entity_position_sync_26_2() -> Vec<u8> {
        let mut writer = Writer::new();
        writer.varint(7);
        for value in [1.0, 2.0, 3.0, 0.1, 0.2, 0.3] {
            writer.f64(value);
        }
        writer.f32(90.0);
        writer.f32(-45.0);
        writer.bool(true);
        writer.data
    }

    #[test]
    fn rewrites_26_2_entity_position_sync() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let payload = entity_position_sync_26_2();
        let rewritten = compat.clientbound_payload(
            State::Play,
            play_clientbound::ENTITY_POSITION_SYNC,
            &payload,
        );
        assert!(matches!(rewritten, Cow::Owned(_)));
        let packet =
            play::Clientbound::decode(play_clientbound::ENTITY_POSITION_SYNC, &rewritten).unwrap();
        let play::Clientbound::EntityPositionSync(sync) = packet else {
            panic!("expected EntityPositionSync");
        };
        assert_eq!(sync.id, 7);
        assert_eq!(sync.path.end(), [1.0, 2.0, 3.0]);
        assert_eq!((sync.yaw, sync.pitch, sync.on_ground), (90.0, -45.0, true));
    }

    fn move_entity_26_2(pos: bool, rot: bool) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.varint(9);
        if pos {
            for value in [10, -20, 30] {
                writer.i16(value);
            }
        }
        if rot {
            writer.i8(40);
            writer.i8(-50);
        }
        writer.bool(true);
        writer.data
    }

    #[test]
    fn rewrites_26_2_move_entity_payloads() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();

        let payload = move_entity_26_2(true, false);
        let rewritten =
            compat.clientbound_payload(State::Play, play_clientbound::MOVE_ENTITY_POS, &payload);
        let packet =
            play::Clientbound::decode(play_clientbound::MOVE_ENTITY_POS, &rewritten).unwrap();
        let play::Clientbound::MoveEntityPos(packet) = packet else {
            panic!("expected MoveEntityPos");
        };
        assert_eq!(packet.0.id, 9);
        assert_eq!(packet.0.delta, Some(play::VecDelta::Linear([10, -20, 30])));
        assert!(packet.0.on_ground);

        let payload = move_entity_26_2(true, true);
        let rewritten = compat.clientbound_payload(
            State::Play,
            play_clientbound::MOVE_ENTITY_POS_ROT,
            &payload,
        );
        let packet =
            play::Clientbound::decode(play_clientbound::MOVE_ENTITY_POS_ROT, &rewritten).unwrap();
        let play::Clientbound::MoveEntityPosRot(packet) = packet else {
            panic!("expected MoveEntityPosRot");
        };
        assert_eq!(packet.0.delta, Some(play::VecDelta::Linear([10, -20, 30])));
        assert_eq!(packet.0.rotation, Some((40, -50)));
        assert!(packet.0.on_ground);

        let payload = move_entity_26_2(false, true);
        let rewritten =
            compat.clientbound_payload(State::Play, play_clientbound::MOVE_ENTITY_ROT, &payload);
        let packet =
            play::Clientbound::decode(play_clientbound::MOVE_ENTITY_ROT, &rewritten).unwrap();
        let play::Clientbound::MoveEntityRot(packet) = packet else {
            panic!("expected MoveEntityRot");
        };
        assert_eq!(packet.0.rotation, Some((40, -50)));
        assert!(packet.0.on_ground);
    }

    #[test]
    fn rewrites_26_2_recipe_book_add_tag_slots() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let mut writer = Writer::new();
        writer.varint(1);
        writer.varint(318);
        writer.varint(1);
        writer.varint(2);
        writer.varint(2);
        writer.varint(4);
        for _ in 0..4 {
            writer.varint(6);
            writer.string("minecraft:planks");
        }
        writer.varint(4);
        writer.varint(12);
        writer.varint(4);
        writer.varint(13);
        writer.varint(0);
        writer.varint(0);
        writer.bool(false);
        writer.i8(1);
        writer.bool(false);
        let payload = writer.data;

        let rewritten =
            compat.clientbound_payload(State::Play, play_clientbound::RECIPE_BOOK_ADD, &payload);
        assert!(matches!(rewritten, Cow::Owned(_)));
        let packet =
            play::Clientbound::decode(play_clientbound::RECIPE_BOOK_ADD, &rewritten).unwrap();
        let play::Clientbound::RecipeBookAdd(add) = packet else {
            panic!("expected RecipeBookAdd");
        };
        assert_eq!(add.entries.len(), 1);
        assert_eq!(add.entries[0].id, 318);
    }

    #[test]
    fn rewrites_26_2_advancement_display_positions() {
        use horizon_protocol::item::{Stack, Template};
        use horizon_world::nbt::Tag;

        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let mut writer = Writer::new();
        writer.bool(false);
        writer.varint(1);
        writer.string("test:root");
        writer.bool(false);
        writer.bool(true);
        writer.nbt(&Tag::End);
        writer.nbt(&Tag::End);
        Template::write(&Stack::new(1, 1), &mut writer);
        writer.varint(0);
        writer.i32(0);
        writer.f32(1.5);
        writer.f32(-2.5);
        writer.varint(0);
        writer.bool(false);
        writer.varint(0);
        writer.varint(0);
        writer.bool(true);
        let payload = writer.data;

        let rewritten = compat.clientbound_payload(
            State::Play,
            play_clientbound::UPDATE_ADVANCEMENTS,
            &payload,
        );
        assert!(matches!(rewritten, Cow::Owned(_)));
        let packet =
            play::Clientbound::decode(play_clientbound::UPDATE_ADVANCEMENTS, &rewritten).unwrap();
        let play::Clientbound::UpdateAdvancements(update) = packet else {
            panic!("expected UpdateAdvancements");
        };
        assert_eq!(update.added.len(), 1);
        assert_eq!(update.added[0].id, "test:root");
        assert_eq!((update.added[0].x, update.added[0].y), (1.5, -2.5));
        assert!(update.added[0].display.is_some());
        assert!(update.show);
    }

    #[test]
    fn strips_26_3_teleport_fields_for_26_2() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let body = play::Serverbound::AcceptTeleportation(play::AcceptTeleportation {
            id: 7,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            yaw: 4.0,
            pitch: 5.0,
        })
        .encode();
        let encoded = compat.encode_serverbound(State::Play, &body);
        let mut reader = Reader::new(&encoded);
        assert_eq!(
            reader.varint().unwrap(),
            play_serverbound::ACCEPT_TELEPORTATION
        );
        assert_eq!(reader.varint().unwrap(), 7);
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn maps_26_2_play_serverbound_insertions_and_swing() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        assert_eq!(compat.serverbound_id(State::Play, 45), 45);
        assert_eq!(compat.serverbound_id(State::Play, 46), 63);
        assert_eq!(compat.serverbound_id(State::Play, 47), 46);
        assert_eq!(compat.serverbound_id(State::Play, 63), 62);
        assert_eq!(compat.serverbound_id(State::Play, 64), 64);

        let punch = play::Serverbound::Punch(Empty).encode();
        assert_eq!(
            compat.encode_serverbound(State::Play, &punch),
            [MINECRAFT_26_2_PLAY_SWING as u8, 0]
        );
    }

    #[test]
    fn serverbound_encoding_is_identity_for_verified_26_2_ids() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let body = cfg::Serverbound::FinishConfiguration(Empty).encode();
        assert_eq!(compat.encode_serverbound(State::Configuration, &body), body);
    }

    #[test]
    fn mapped_payloads_decode_with_native_codecs() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();

        let features = cfg::Clientbound::UpdateEnabledFeatures(cfg::UpdateEnabledFeatures {
            features: vec!["minecraft:vanilla".into()],
        });
        assert_eq!(decode_mapped(features.clone(), 12, compat), features);

        let tags = cfg::Clientbound::UpdateTags(cfg::UpdateTags {
            registries: vec![cfg::RegistryTags {
                registry: "minecraft:test".into(),
                tags: vec![cfg::TagEntry {
                    name: "minecraft:test".into(),
                    ids: vec![1, 2, 3],
                }],
            }],
        });
        assert_eq!(decode_mapped(tags.clone(), 13, compat), tags);

        let packs = cfg::Clientbound::SelectKnownPacks(cfg::SelectKnownPacks {
            packs: vec![cfg::KnownPack {
                namespace: "minecraft".into(),
                id: "core".into(),
                version: "26.2".into(),
            }],
        });
        assert_eq!(decode_mapped(packs.clone(), 14, compat), packs);
    }

    #[test]
    fn translates_26_2_entity_type_ids_in_add_entity() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let add = |kind| AddEntity {
            id: 7,
            uuid: 1,
            kind,
            position: [1.0, 2.0, 3.0],
            velocity: [0.0; 3],
            pitch: 0,
            yaw: 0,
            head_yaw: 0,
            data: 0,
        };
        for (from, to) in [(117, 120), (114, 117), (70, 71), (0, 0)] {
            let body = Clientbound::AddEntity(add(from)).encode();
            let mut r = Reader::new(&body);
            let id = r.varint().unwrap();
            let payload = compat.clientbound_payload(State::Play, id, r.rest());
            assert_eq!(
                Clientbound::decode(id, &payload).unwrap(),
                Clientbound::AddEntity(add(to))
            );
        }
        let native = ProtocolCompat::native();
        let body = Clientbound::AddEntity(add(117)).encode();
        let mut r = Reader::new(&body);
        let id = r.varint().unwrap();
        assert!(matches!(
            native.clientbound_payload(State::Play, id, r.rest()),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn translates_26_2_registry_tags_but_not_unshifted_ones() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let tags = |entity: Vec<i32>, block: Vec<i32>| {
            cfg::Clientbound::UpdateTags(cfg::UpdateTags {
                registries: vec![
                    cfg::RegistryTags {
                        registry: "minecraft:entity_type".into(),
                        tags: vec![cfg::TagEntry {
                            name: "minecraft:t".into(),
                            ids: entity,
                        }],
                    },
                    cfg::RegistryTags {
                        registry: "minecraft:fluid".into(),
                        tags: vec![cfg::TagEntry {
                            name: "minecraft:t".into(),
                            ids: block,
                        }],
                    },
                ],
            })
        };
        let body = tags(vec![117, 70], vec![117, 70]).encode();
        let mut r = Reader::new(&body);
        let id = r.varint().unwrap();
        let payload = compat.clientbound_payload(State::Configuration, id, r.rest());
        assert_eq!(
            cfg::Clientbound::decode(id, &payload).unwrap(),
            tags(vec![120, 71], vec![117, 70])
        );
    }

    #[test]
    fn rewrites_26_2_chunk_sections_with_native_block_states() {
        use horizon_protocol::chunk::{BLOCKS, Paletted};
        let section = |states: Paletted| Section {
            non_empty: 7,
            fluids: 0,
            states,
            biomes: Paletted::from_values(
                &vec![3; horizon_protocol::chunk::BIOMES],
                Strategy::Biomes,
                7,
            ),
        };
        let wide: Vec<i32> = (0..BLOCKS as i32).map(|i| i % 300).collect();
        let few: Vec<i32> = (0..BLOCKS as i32)
            .map(|i| [0, 9, 100][i as usize % 3])
            .collect();
        let sections = vec![
            section(Paletted::from_values(&wide, Strategy::Blocks, 15)),
            section(Paletted::from_values(&few, Strategy::Blocks, 15)),
            section(Paletted::from_values(&[5; BLOCKS], Strategy::Blocks, 15)),
        ];
        let chunk = ChunkData {
            heightmaps: vec![],
            buffer: ChunkData::from_sections(&sections),
            block_entities: vec![],
        };
        let out = rewrite_26_2_sections(&chunk).unwrap();
        let map = |v: i32| lookup(&tables::BLOCK_STATE_FROM_26_2, v);
        assert_eq!(out.len(), 3);
        for i in 0..BLOCKS {
            assert_eq!(out[0].states.get(i), map(wide[i]));
            assert_eq!(out[1].states.get(i), map(few[i]));
            assert_eq!(out[2].states.get(i), map(5));
        }
        assert_eq!(out[0].states.bits, 16);
        assert_eq!(out[0].biomes, sections[0].biomes);
        assert_eq!(out[0].non_empty, 7);
    }

    #[test]
    fn translates_26_2_block_updates() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let state = 1234;
        let body = Clientbound::BlockUpdate(BlockUpdate {
            pos: [1, 2, 3],
            state,
        })
        .encode();
        let mut r = Reader::new(&body);
        let id = r.varint().unwrap();
        let payload = compat.clientbound_payload(State::Play, id, r.rest());
        assert_eq!(
            Clientbound::decode(id, &payload).unwrap(),
            Clientbound::BlockUpdate(BlockUpdate {
                pos: [1, 2, 3],
                state: lookup(&tables::BLOCK_STATE_FROM_26_2, state)
            })
        );
    }

    #[test]
    fn remaps_26_2_player_actions() {
        use horizon_protocol::packets::play::{PlayerAction, Serverbound};
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let send = |action| {
            let body = Serverbound::PlayerAction(PlayerAction {
                action,
                pos: [4, 5, 6],
                direction: 1,
                sequence: 9,
            })
            .encode();
            let out = compat.encode_serverbound(State::Play, &body);
            let mut r = Reader::new(&out);
            let id = r.varint().unwrap();
            match Serverbound::decode(id, r.rest()).unwrap() {
                Serverbound::PlayerAction(a) => (id, a),
                other => panic!("{other:?}"),
            }
        };
        for (from, to) in [
            (0, 0),
            (1, 0),
            (2, 1),
            (3, 2),
            (4, 3),
            (5, 4),
            (6, 5),
            (7, 6),
            (8, 7),
        ] {
            let (id, a) = send(from);
            assert_eq!(id, play_serverbound::PLAYER_ACTION);
            assert_eq!(
                (a.action, a.pos, a.direction, a.sequence),
                (to, [4, 5, 6], 1, 9)
            );
        }
        let body = Serverbound::PlayerAction(PlayerAction {
            action: 2,
            pos: [0; 3],
            direction: 0,
            sequence: 1,
        })
        .encode();
        assert_eq!(
            ProtocolCompat::native().encode_serverbound(State::Play, &body),
            body
        );
    }

    #[test]
    fn reorders_26_2_sign_update() {
        use horizon_protocol::packets::play::{Serverbound, SignUpdate};
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        for front in [true, false] {
            let body = Serverbound::SignUpdate(SignUpdate {
                pos: [-4, 70, 9],
                lines: ["Hi".into(), String::new(), "x".into(), "ü".into()],
                front,
            })
            .encode();
            let out = compat.encode_serverbound(State::Play, &body);
            let mut r = Reader::new(&out);
            r.varint().unwrap();
            assert_eq!(
                r.bytes(8).unwrap(),
                horizon_protocol::buf::pack_block_pos([-4, 70, 9]).to_be_bytes()
            );
            assert_eq!(r.u8().unwrap(), u8::from(front));
            for line in ["Hi", "", "x", "ü"] {
                assert_eq!(r.string(384).unwrap(), line);
            }
            assert!(r.rest().is_empty());
        }
    }

    #[test]
    fn rewrites_26_2_respawn_previous_game_type() {
        let respawn = |previous: &[u8]| {
            let mut w = Writer::new();
            w.varint(3);
            w.string("minecraft:overworld");
            w.i64(42);
            w.u8(0);
            w.bytes(previous);
            w.bool(false);
            w.bool(false);
            w.bool(false);
            w.varint(0);
            w.varint(63);
            w.u8(3);
            w.data
        };
        let out = rewrite_26_2_respawn(&respawn(&[0xff])).unwrap();
        assert_eq!(out, respawn(&[0]));
        let out = rewrite_26_2_respawn(&respawn(&[1])).unwrap();
        assert_eq!(out, respawn(&[1 + 1]));
    }

    #[test]
    fn retargets_26_2_animate() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let animate = |action: u8| {
            let mut w = Writer::new();
            w.varint(300);
            w.u8(action);
            w.data
        };
        let (id, body) =
            compat.clientbound_retarget(State::Play, play_clientbound::ANIMATE, animate(4));
        assert_eq!(id, play_clientbound::ANIMATE);
        assert_eq!(body, animate(1));
        assert_eq!(
            compat
                .clientbound_retarget(State::Play, play_clientbound::ANIMATE, animate(5))
                .1,
            animate(2)
        );
        assert_eq!(
            compat
                .clientbound_retarget(State::Play, play_clientbound::ANIMATE, animate(2))
                .1,
            animate(0)
        );
        let (id, body) =
            compat.clientbound_retarget(State::Play, play_clientbound::ANIMATE, animate(0));
        assert_eq!(id, play_clientbound::SWING_ANIMATION);
        assert_eq!(
            play::Clientbound::decode(id, &body).unwrap(),
            play::Clientbound::SwingAnimation(play::SwingAnimation {
                id: 300,
                hand: 0,
                kind: 1,
                duration: 6
            })
        );
        let (_, body) =
            compat.clientbound_retarget(State::Play, play_clientbound::ANIMATE, animate(3));
        assert_eq!(body[2..], [1, 1, 6]);
        let (id, body) = ProtocolCompat::native().clientbound_retarget(
            State::Play,
            play_clientbound::ANIMATE,
            animate(4),
        );
        assert_eq!((id, body), (play_clientbound::ANIMATE, animate(4)));
    }

    #[test]
    fn rewrites_26_2_level_particles_without_translating_the_particle() {
        let mut w = Writer::new();
        w.bool(true);
        w.bool(false);
        for v in [1.5f64, 64.0, -2.0] {
            w.f64(v);
        }
        for v in [0.25f32, 0.5, 0.75] {
            w.f32(v);
        }
        w.f32(0.1);
        w.i32(7);
        let header = w.data.len();
        w.varint(45);
        w.f32(1.0);
        let particle = w.data[header..].to_vec();
        let out = rewrite_26_2_level_particles(&w.data).unwrap();
        assert!(
            out.starts_with(&particle),
            "particle bytes must pass through"
        );
        let mut r = Reader::new(&out[particle.len()..]);
        assert!(r.bool().unwrap() && !r.bool().unwrap());
        assert_eq!(
            [r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap()],
            [1.5, 64.0, -2.0]
        );
        assert_eq!(
            [r.f32().unwrap(), r.f32().unwrap(), r.f32().unwrap()],
            [0.25, 0.5, 0.75]
        );
        assert_eq!(
            [r.f32().unwrap(), r.f32().unwrap(), r.f32().unwrap()],
            [0.1; 3]
        );
        assert_eq!((r.varint().unwrap(), r.varint().unwrap()), (7, 0));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn translates_26_2_stat_value_ids_by_stat_type() {
        let compat = ProtocolCompat::for_version(ProtocolVersion::MINECRAFT_26_2).unwrap();
        let stats = |entries: Vec<(i32, i32, i32)>| {
            Clientbound::AwardStats(play::AwardStats { stats: entries })
        };
        let body = stats(vec![
            (0, 100, 1),
            (1, 100, 2),
            (6, 117, 3),
            (8, 70, 4),
            (9, 70, 5),
        ])
        .encode();
        let mut r = Reader::new(&body);
        let id = r.varint().unwrap();
        let out = compat.clientbound_payload(State::Play, id, r.rest());
        let want = |t: &[i32], v: i32| lookup(t, v);
        assert_eq!(
            Clientbound::decode(id, &out).unwrap(),
            stats(vec![
                (0, want(&tables::BLOCK_FROM_26_2, 100), 1),
                (1, want(&tables::ITEM_FROM_26_2, 100), 2),
                (6, 120, 3),
                (8, 71, 4),
                (9, 70, 5),
            ])
        );
    }

    #[test]
    fn parses_cli_and_environment_selection_values() {
        assert_eq!(
            ProtocolSelection::parse("auto").unwrap(),
            ProtocolSelection::Auto
        );
        assert_eq!(
            ProtocolSelection::parse("native").unwrap(),
            ProtocolSelection::Native
        );
        assert_eq!(
            ProtocolSelection::parse("776").unwrap(),
            ProtocolSelection::Explicit(ProtocolVersion::MINECRAFT_26_2)
        );
        assert_eq!(
            ProtocolSelection::parse("777").unwrap(),
            ProtocolSelection::Native
        );
        assert!(ProtocolSelection::parse("latest").is_err());
        assert!(ProtocolSelection::parse("").is_err());
    }

    #[test]
    fn refuses_unsupported_versions_instead_of_falling_back() {
        let error = ProtocolCompat::for_version(ProtocolVersion::new(775)).unwrap_err();
        assert!(error.contains("unsupported Minecraft protocol 775"));
        assert!(ProtocolCompat::for_version(ProtocolVersion::new(9999)).is_err());
    }
}
