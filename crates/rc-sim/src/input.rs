//! Client messages received through the memory link (identifier + packet body) translated into
//! server inputs. Packets with no effect at milestone 1 are ignored.

use rc_client::ids::Ids;
use rc_proto::varint::read_varint;
use rc_proto::{BlockPos, Decode, Reader};

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    AcceptTeleport {
        id: i32,
    },
    /// Movement: optional position and rotation, on ground, horizontal collision.
    Move {
        pos: Option<[f64; 3]>,
        rot: Option<(f32, f32)>,
        on_ground: bool,
    },
    TickEnd,
    Loaded,
    /// Held keys (`player_input`): sneaking = 32.
    Keys(u8),
    /// Middle click on a block; with the block entity data (Ctrl).
    PickBlock {
        pos: [i32; 3],
        with_data: bool,
    },
    /// `client_information`: view distance, skin parts, main arm (0 left).
    ClientInfo {
        view: i32,
        skin_parts: u8,
        main_hand: i32,
    },
    /// `spectator_action`: targeted entity (camera), or none.
    SpectatorAction(Option<i32>),
    HeldSlot(i16),
    Flying(bool),
    /// `player_action`: action, block, face, sequence.
    Action {
        action: i32,
        pos: [i32; 3],
        face: u8,
        seq: i32,
    },
    /// `use_item_on`: hand, block, face, touched point (relative to the block), inside, sequence.
    UseItemOn {
        hand: i32,
        pos: [i32; 3],
        face: i32,
        at: [f32; 3],
        inside: bool,
        seq: i32,
    },
    UseItem {
        seq: i32,
    },
    /// Slot 1–45 of menu 0 set in creative (item identifier, quantity; `None` = empty).
    CreativeSlot {
        slot: i16,
        item: Option<(i32, i32)>,
    },
    Chat(String),
    Command(String),
    GameMode(i32),
    Other,
}

fn pos(r: &mut Reader<'_>) -> rc_proto::Result<[i32; 3]> {
    let p = BlockPos::decode(r)?;
    Ok([p.x, p.y, p.z])
}

fn vec3(r: &mut Reader<'_>) -> rc_proto::Result<[f64; 3]> {
    Ok([f64::decode(r)?, f64::decode(r)?, f64::decode(r)?])
}

pub fn decode(ids: &Ids, packet: &[u8]) -> rc_proto::Result<Input> {
    let mut r = Reader::new(packet);
    let id = read_varint(&mut r)?;
    let r = &mut r;
    let flags = |r: &mut Reader<'_>| r.u8().map(|f| f & 1 != 0);
    Ok(if id == ids.accept_teleportation {
        Input::AcceptTeleport { id: read_varint(r)? }
    } else if id == ids.move_pos_rot {
        let p = vec3(r)?;
        let rot = (f32::decode(r)?, f32::decode(r)?);
        Input::Move { pos: Some(p), rot: Some(rot), on_ground: flags(r)? }
    } else if id == ids.move_pos {
        let p = vec3(r)?;
        Input::Move { pos: Some(p), rot: None, on_ground: flags(r)? }
    } else if id == ids.move_rot {
        let rot = (f32::decode(r)?, f32::decode(r)?);
        Input::Move { pos: None, rot: Some(rot), on_ground: flags(r)? }
    } else if id == ids.move_status {
        Input::Move { pos: None, rot: None, on_ground: flags(r)? }
    } else if id == ids.pick_item_from_block {
        let p = pos(r)?;
        Input::PickBlock { pos: p, with_data: bool::decode(r)? }
    } else if id == ids.spectator_action {
        let e = read_varint(r)?;
        Input::SpectatorAction((e != 0).then(|| e - 1))
    } else if id == ids.player_input {
        Input::Keys(r.u8()?)
    } else if id == ids.client_tick_end {
        Input::TickEnd
    } else if id == ids.player_loaded {
        Input::Loaded
    } else if id == ids.client_information_out {
        rc_proto::read_str(r, 16)?;
        let view = i32::from(r.u8()?);
        let _chat_visibility = read_varint(r)?;
        let _chat_colors = bool::decode(r)?;
        let skin_parts = r.u8()?;
        Input::ClientInfo { view, skin_parts, main_hand: read_varint(r)? }
    } else if id == ids.set_carried_item_out {
        Input::HeldSlot(i16::decode(r)?)
    } else if id == ids.abilities_out {
        Input::Flying(r.u8()? & 2 != 0)
    } else if id == ids.player_action {
        let action = read_varint(r)?;
        let p = pos(r)?;
        let face = r.u8()?;
        Input::Action { action, pos: p, face, seq: read_varint(r)? }
    } else if id == ids.use_item_on {
        let hand = read_varint(r)?;
        let p = pos(r)?;
        let face = read_varint(r)?;
        let at = [f32::decode(r)?, f32::decode(r)?, f32::decode(r)?];
        let inside = bool::decode(r)?;
        let _border = bool::decode(r)?;
        Input::UseItemOn { hand, pos: p, face, at, inside, seq: read_varint(r)? }
    } else if id == ids.use_item {
        let _hand = read_varint(r)?;
        Input::UseItem { seq: read_varint(r)? }
    } else if id == ids.set_creative_mode_slot {
        let slot = i16::decode(r)?;
        let count = read_varint(r)?;
        let item = if count > 0 { Some((read_varint(r)?, count)) } else { None };
        Input::CreativeSlot { slot, item }
    } else if id == ids.chat_out {
        Input::Chat(rc_proto::read_str(r, 256)?.to_owned())
    } else if id == ids.chat_command_out || id == ids.chat_command_signed {
        Input::Command(rc_proto::read_str(r, 32767)?.to_owned())
    } else if id == ids.change_game_mode {
        Input::GameMode(read_varint(r)?)
    } else {
        Input::Other
    })
}
