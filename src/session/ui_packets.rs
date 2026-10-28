//! Target 26.3 UI wire layouts; semantic admission remains in the UI/inventory owners.
use crate::protocol::packet_ids::play::serverbound as sb;
use crate::{
    inventory::PacketCommand,
    protocol::{self, nbt::Nbt, Result, Writer},
    ui::{CommandEdit, JigsawEdit, LastSeenUpdate, StructureEdit},
};

fn utf(writer: &mut Writer, value: &str, limit: usize) -> Result<()> {
    if value.encode_utf16().count() > limit || value.len() > limit * 3 {
        return Err("Source outbound UTF length bound".into());
    }
    writer.string(value);
    Ok(())
}

pub(super) fn chat(
    message: &str,
    timestamp_millis: i64,
    salt: i64,
    last_seen: LastSeenUpdate,
) -> Result<PacketCommand> {
    let mut writer = Writer::default();
    utf(&mut writer, message, 256)?;
    writer.i64(timestamp_millis);
    writer.i64(salt);
    // The explicitly offline profile has no authenticated message-signing private key.
    writer.byte(0);
    writer.var(last_seen.offset);
    writer.0.extend_from_slice(&last_seen.acknowledged);
    writer.byte(last_seen.checksum);
    Ok(PacketCommand::new(sb::CHAT, writer))
}

pub(super) fn unsigned_command(command: &str) -> Result<PacketCommand> {
    let mut writer = Writer::default();
    utf(&mut writer, command, 32767)?;
    Ok(PacketCommand::new(sb::CHAT_COMMAND, writer))
}

/// ServerboundCommandSuggestionPacket: VarInt transaction id and UTF command.
pub(super) fn command_suggestion(id: i32, command: &str) -> Result<PacketCommand> {
    let mut writer = Writer::default();
    writer.var(id);
    utf(&mut writer, command, 32500)?;
    Ok(PacketCommand::new(sb::COMMAND_SUGGESTION, writer))
}

pub(super) fn chat_ack(offset: i32) -> PacketCommand {
    let mut writer = Writer::default();
    writer.var(offset);
    PacketCommand::new(sb::CHAT_ACK, writer)
}

pub(super) fn sign(position: [i32; 3], front: bool, lines: &[String; 4]) -> Result<PacketCommand> {
    let mut writer = Writer::default();
    writer.i64(protocol::pack_pos(position));
    for line in lines {
        utf(&mut writer, line, 384)?;
    }
    writer.var(i32::from(front));
    Ok(PacketCommand::new(sb::SIGN_UPDATE, writer))
}

pub(super) fn book(slot: i32, pages: &[String], title: Option<&str>) -> Result<PacketCommand> {
    if pages.len() > 100 {
        return Err("Source outbound book page count bound".into());
    }
    let mut writer = Writer::default();
    writer.var(slot);
    writer.var(pages.len() as i32);
    for page in pages {
        utf(&mut writer, page, 1024)?;
    }
    writer.byte(u8::from(title.is_some()));
    if let Some(title) = title {
        utf(&mut writer, title, 32)?;
    }
    Ok(PacketCommand::new(sb::EDIT_BOOK, writer))
}

pub(super) fn command_block(edit: &CommandEdit) -> Result<PacketCommand> {
    let mut writer = Writer::default();
    let id = if let Some(entity_id) = edit.entity_id {
        writer.var(entity_id);
        sb::SET_COMMAND_MINECART
    } else {
        writer.i64(protocol::pack_pos(edit.pos));
        sb::SET_COMMAND_BLOCK
    };
    utf(&mut writer, &edit.command, 32767)?;
    if edit.entity_id.is_some() {
        writer.byte(u8::from(edit.track_output));
    } else {
        writer.var(edit.mode);
        writer.byte(
            u8::from(edit.track_output)
                | (u8::from(edit.conditional) << 1)
                | (u8::from(edit.automatic) << 2),
        );
    }
    Ok(PacketCommand::new(id, writer))
}

pub(super) fn structure(edit: &StructureEdit) -> Result<PacketCommand> {
    let mut writer = Writer::default();
    writer.i64(protocol::pack_pos(edit.pos));
    writer.var(edit.update);
    writer.var(edit.mode);
    utf(&mut writer, &edit.name, 32767)?;
    // Source writes the low byte without clamping. Its receiving decoder clamps the
    // signed byte to the structure bounds; doing that here changes transmitted values.
    for coordinate in edit.offset.into_iter().chain(edit.size) {
        writer.byte(coordinate as u8);
    }
    writer.var(edit.mirror);
    writer.var(edit.rotation);
    utf(&mut writer, &edit.data, 32767)?;
    writer.f32(edit.integrity);
    let mut seed = edit.seed as u64;
    loop {
        if seed & !127 == 0 {
            writer.byte(seed as u8);
            break;
        }
        writer.byte((seed as u8 & 127) | 128);
        seed >>= 7;
    }
    writer.byte(
        u8::from(edit.ignore_entities)
            | (u8::from(edit.show_air) << 1)
            | (u8::from(edit.show_box) << 2)
            | (u8::from(edit.strict) << 3),
    );
    Ok(PacketCommand::new(sb::SET_STRUCTURE_BLOCK, writer))
}

pub(super) fn jigsaw(edit: &JigsawEdit) -> Result<PacketCommand> {
    let mut writer = Writer::default();
    writer.i64(protocol::pack_pos(edit.pos));
    for identifier in [&edit.name, &edit.target, &edit.pool] {
        utf(&mut writer, &protocol::parse_identifier(identifier)?, 32767)?;
    }
    utf(&mut writer, &edit.final_state, 32767)?;
    utf(&mut writer, &edit.joint, 32767)?;
    writer.var(edit.selection_priority);
    writer.var(edit.placement_priority);
    Ok(PacketCommand::new(sb::SET_JIGSAW_BLOCK, writer))
}

pub(super) fn generate_jigsaw(position: [i32; 3], levels: i32, keep: bool) -> PacketCommand {
    let mut writer = Writer::default();
    writer.i64(protocol::pack_pos(position));
    writer.var(levels);
    writer.byte(u8::from(keep));
    PacketCommand::new(sb::JIGSAW_GENERATE, writer)
}

/// `ServerboundCustomClickActionPacket` (play id 68): Identifier, then the optional NBT payload
/// behind a VarInt byte length (`UNTRUSTED_TAG_CODEC`; TAG_End when absent), per the local 26.3 decompiled client.
pub(super) fn custom_click(id: &str, payload: Option<&Nbt>) -> Result<PacketCommand> {
    let mut body = Writer::default();
    match payload {
        Some(tag) => tag.write_root(&mut body)?,
        None => body.byte(0),
    }
    if body.0.len() > 65536 {
        return Err("Source outbound custom click payload bound".into());
    }
    let mut writer = Writer::default();
    utf(&mut writer, &protocol::parse_identifier(id)?, 32767)?;
    writer.var(body.0.len() as i32);
    writer.0.extend(body.0);
    Ok(PacketCommand::new(sb::CUSTOM_CLICK_ACTION, writer))
}

pub(super) fn respawn() -> PacketCommand {
    let mut writer = Writer::default();
    writer.var(0);
    PacketCommand::new(sb::CLIENT_COMMAND, writer)
}

pub(super) fn request_statistics() -> PacketCommand {
    // GameProtocols registration ordinal 12; ClientCommand Action.REQUEST_STATS
    // ordinal 1, encoded by FriendlyByteBuf.writeEnum as a VarInt (26.1.2 javap).
    let mut writer = Writer::default();
    writer.var(1);
    PacketCommand::new(sb::CLIENT_COMMAND, writer)
}

pub(super) fn seen_advancements(tab: Option<&str>) -> PacketCommand {
    // Local 26.1.2 javap: GameProtocols.lambda$static$1 registers this at
    // ordinal 50 (bytecode offset 451). SeenAdvancementsPacket.write writes
    // Action via writeEnum (VarInt), then Identifier only for OPENED_TAB.
    // Action ordinals: OPENED_TAB=0, CLOSED_SCREEN=1; Identifier uses UTF.
    let mut writer = Writer::default();
    match tab {
        Some(tab) => {
            writer.var(0);
            writer.string(tab);
        }
        None => writer.var(1),
    }
    PacketCommand::new(sb::SEEN_ADVANCEMENTS, writer)
}

#[cfg(test)]
mod advancement_tests {
    use super::*;

    #[test]
    fn p1_advancements_seen_packet_encodes_open_and_close() {
        for root in [
            "minecraft:story/root",
            "minecraft:nether/root",
            "custom:root",
        ] {
            let packet = seen_advancements(Some(root));
            assert_eq!(packet.id, 0x33);
            let mut reader = crate::protocol::Reader::new(&packet.payload.0);
            assert_eq!(reader.var().unwrap(), 0);
            assert_eq!(reader.string(32767).unwrap(), root);
            reader.done().unwrap();
        }
        let packet = seen_advancements(None);
        assert_eq!(packet.id, 0x33);
        assert_eq!(packet.payload.0, [1]);
    }
}

#[cfg(test)]
mod statistics_tests {
    use super::*;

    #[test]
    fn p1_statistics_request_uses_client_command_action_one() {
        let packet = request_statistics();
        assert_eq!(packet.id, 0x0c);
        assert_eq!(packet.payload.0, [1]);
        let mut reader = crate::protocol::Reader::new(&packet.payload.0);
        assert_eq!(reader.var().unwrap(), 1);
        reader.done().unwrap();
        assert_eq!(respawn().payload.0, [0]);
    }
}

#[cfg(test)]
mod dialog_tests {
    use super::*;
    use crate::protocol::Reader;

    #[test]
    fn custom_click_packet_matches_the_server_wire_layout() {
        let payload = Nbt::Compound(vec![
            ("name".into(), Nbt::String("Ёлки".into())),
            ("radius".into(), Nbt::Float(12.)),
            ("walls".into(), Nbt::Byte(1)),
        ]);
        let packet = custom_click("solaris:dialog_probe/submit", Some(&payload)).unwrap();
        // Local 26.3 packets.json: play serverbound custom_click_action = 68.
        assert_eq!(packet.id, 0x44);
        let mut reader = Reader::new(&packet.payload.0);
        assert_eq!(reader.string(32767).unwrap(), "solaris:dialog_probe/submit");
        let len = reader.var().unwrap() as usize;
        let body = reader.take(len).unwrap();
        reader.done().unwrap();
        assert_eq!(Reader::new(body).nbt_typed().unwrap(), payload);
        // No payload: length 1, TAG_End.
        let none = custom_click("solaris:x", None).unwrap();
        assert_eq!(&none.payload.0[none.payload.0.len() - 2..], &[1, 0]);
    }
}
