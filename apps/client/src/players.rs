//! Player list of the server (`docs/specs/players.md`): profile, game mode, presence
//! in the list, latency, display name, order, hat.

use hashbrown::HashMap;

use rc_proto_v777::players::{InfoUpdate, Property};

#[derive(Debug, Clone, Default)]
pub struct PlayerEntry {
    pub name: String,
    /// Profile properties (appearance: `textures`).
    pub properties: Vec<Property>,
    pub game_mode: i32,
    pub listed: bool,
    pub latency: i32,
    pub display_name: Option<rc_proto::nbt::Nbt>,
    pub list_order: i32,
    pub show_hat: bool,
    pub chat_session: bool,
}

#[derive(Default)]
pub struct PlayerList {
    pub map: HashMap<u128, PlayerEntry>,
}

impl PlayerList {
    /// Update: an entry without an add for an unknown player is ignored (like the game).
    pub fn apply(&mut self, u: InfoUpdate) {
        for e in u.entries {
            if let Some((name, properties)) = e.profile {
                self.map.insert(e.uuid, PlayerEntry { name, properties, show_hat: true, ..Default::default() });
            }
            let Some(p) = self.map.get_mut(&e.uuid) else { continue };
            if let Some(v) = e.chat_session {
                p.chat_session = v;
            }
            if let Some(v) = e.game_mode {
                p.game_mode = v;
            }
            if let Some(v) = e.listed {
                p.listed = v;
            }
            if let Some(v) = e.latency {
                p.latency = v;
            }
            if let Some(v) = e.display_name {
                p.display_name = v;
            }
            if let Some(v) = e.list_order {
                p.list_order = v;
            }
            if let Some(v) = e.show_hat {
                p.show_hat = v;
            }
        }
    }

    pub fn remove(&mut self, uuids: &[u128]) {
        for u in uuids {
            self.map.remove(u);
        }
    }

    pub fn name(&self, uuid: u128) -> Option<&str> {
        self.map.get(&uuid).map(|p| p.name.as_str())
    }
}
