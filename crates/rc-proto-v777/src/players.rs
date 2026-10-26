//! Player list: `player_info_update` and `player_info_remove` (`docs/specs/players.md`).

use rc_proto::varint::read_varint;
use rc_proto::{Decode, Reader, Result};
use rc_proto::{read_count, read_len, read_str};

/// Profile property (`textures`: appearance encoded in base 64, signed by Mojang).
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// Actions of `player_info_update` (bit = identifier).
pub mod action {
    pub const ADD_PLAYER: u8 = 1;
    pub const INITIALIZE_CHAT: u8 = 1 << 1;
    pub const UPDATE_GAME_MODE: u8 = 1 << 2;
    pub const UPDATE_LISTED: u8 = 1 << 3;
    pub const UPDATE_LATENCY: u8 = 1 << 4;
    pub const UPDATE_DISPLAY_NAME: u8 = 1 << 5;
    pub const UPDATE_LIST_ORDER: u8 = 1 << 6;
    pub const UPDATE_HAT: u8 = 1 << 7;
}

/// Entry: only the fields of the packet's actions are present.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InfoEntry {
    pub uuid: u128,
    /// Add: profile name and properties.
    pub profile: Option<(String, Vec<Property>)>,
    /// Chat session: present or not.
    pub chat_session: Option<bool>,
    /// Contents of the present chat session (verification of signed messages).
    pub chat_key: Option<ChatSessionData>,
    pub game_mode: Option<i32>,
    pub listed: Option<bool>,
    pub latency: Option<i32>,
    /// Name displayed in the list (`None` inside = back to the profile name).
    pub display_name: Option<Option<rc_proto::nbt::Nbt>>,
    pub list_order: Option<i32>,
    pub show_hat: Option<bool>,
}

/// Announced chat session of a player: identifier, key expiry (ms since the
/// epoch), public key (DER X.509) and signature of the key by the services.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatSessionData {
    pub session: u128,
    pub expires_ms: i64,
    pub key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InfoUpdate {
    pub actions: u8,
    pub entries: Vec<InfoEntry>,
}

fn read_bytes<'a>(r: &mut Reader<'a>, max: usize) -> Result<&'a [u8]> {
    let n = read_len(r, max)?;
    r.take(n)
}

pub fn read_info_update(r: &mut Reader<'_>) -> Result<InfoUpdate> {
    let actions = r.u8()?;
    let n = read_count(r, 16)?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let mut e = InfoEntry { uuid: u128::decode(r)?, ..Default::default() };
        if actions & action::ADD_PLAYER != 0 {
            let name = read_str(r, 16)?.to_owned();
            let k = read_len(r, 16)?;
            let mut props = Vec::with_capacity(k);
            for _ in 0..k {
                let name = read_str(r, 64)?.to_owned();
                let value = read_str(r, 32767)?.to_owned();
                let signature = if bool::decode(r)? { Some(read_str(r, 1024)?.to_owned()) } else { None };
                props.push(Property { name, value, signature });
            }
            e.profile = Some((name, props));
        }
        if actions & action::INITIALIZE_CHAT != 0 {
            let present = bool::decode(r)?;
            if present {
                let session = u128::decode(r)?;
                let expires_ms = i64::decode(r)?;
                let key = read_bytes(r, 512)?.to_vec();
                let signature = read_bytes(r, 4096)?.to_vec();
                e.chat_key = Some(ChatSessionData { session, expires_ms, key, signature });
            }
            e.chat_session = Some(present);
        }
        if actions & action::UPDATE_GAME_MODE != 0 {
            e.game_mode = Some(read_varint(r)?);
        }
        if actions & action::UPDATE_LISTED != 0 {
            e.listed = Some(bool::decode(r)?);
        }
        if actions & action::UPDATE_LATENCY != 0 {
            e.latency = Some(read_varint(r)?);
        }
        if actions & action::UPDATE_DISPLAY_NAME != 0 {
            e.display_name = Some(if bool::decode(r)? { rc_proto::nbt::read(r)? } else { None });
        }
        if actions & action::UPDATE_LIST_ORDER != 0 {
            e.list_order = Some(read_varint(r)?);
        }
        if actions & action::UPDATE_HAT != 0 {
            e.show_hat = Some(bool::decode(r)?);
        }
        entries.push(e);
    }
    Ok(InfoUpdate { actions, entries })
}

pub fn read_info_remove(r: &mut Reader<'_>) -> Result<Vec<u128>> {
    let n = read_count(r, 16)?;
    (0..n).map(|_| u128::decode(r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_proto::varint::write_varint;

    #[test]
    fn add_player_with_textures_and_latency() {
        let mut w = Vec::new();
        w.push(action::ADD_PLAYER | action::UPDATE_LISTED | action::UPDATE_LATENCY);
        write_varint(&mut w, 1);
        w.extend_from_slice(&7u128.to_be_bytes());
        rc_proto::write_str(&mut w, "Steve");
        write_varint(&mut w, 1);
        rc_proto::write_str(&mut w, "textures");
        rc_proto::write_str(&mut w, "e30=");
        w.push(1);
        rc_proto::write_str(&mut w, "sig");
        w.push(1);
        write_varint(&mut w, 42);
        let mut r = Reader::new(&w);
        let u = read_info_update(&mut r).unwrap();
        assert!(r.is_empty());
        let e = &u.entries[0];
        assert_eq!(e.uuid, 7);
        let (name, props) = e.profile.as_ref().unwrap();
        assert_eq!(name, "Steve");
        assert_eq!(props[0], Property { name: "textures".into(), value: "e30=".into(), signature: Some("sig".into()) });
        assert_eq!((e.listed, e.latency, e.game_mode), (Some(true), Some(42), None));
    }
}
