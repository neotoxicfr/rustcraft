//! Synced values of a player's entity as the clients hold them, and the changes the other
//! players have not been sent yet.
//!
//! Every value the server writes for the installed player's own entity passes here
//! (`Server::send_to_players`): the player's own client still receives it at once; the other
//! players receive the changes of a tick together, one packet per player at the tracking pass
//! (`lan_broadcast_players`), the way the game's tracker packs every changed value of an entity
//! into a single packet per tick. An observer paired later is told every value that differs from
//! the entity defaults, like the game's pairing, whatever was sent before it arrived.

use rc_proto_v777::entity::Meta;
use rc_proto_v777::play::Attribute;

/// Value a client gives synced value `index` of a new player entity (the defaults the game
/// defines for an entity, a living entity, an avatar and a player). `None`: an index the player
/// entity does not have.
fn player_default(index: u8) -> Option<Meta> {
    Some(match index {
        0 | 8 | 16 => Meta::Byte(0),
        1 => Meta::Int(300),
        2 => Meta::Text(None),
        3..=5 | 11 => Meta::Bool(false),
        6 => Meta::Pose(0),
        7 | 12 | 13 | 18 => Meta::Int(0),
        9 => Meta::Float(1.0),
        10 => Meta::Particles(Vec::new()),
        14 => Meta::BlockPos(None),
        // Main arm: right.
        15 => Meta::Variant(1, 42),
        17 => Meta::Float(0.0),
        19 | 20 => Meta::OptionalInt(None),
        _ => return None,
    })
}

/// Writes `value` at `index` in a list kept in index order, one entry per index.
fn put_in_index_order(list: &mut Vec<(u8, Meta)>, index: u8, value: Meta) {
    match list.binary_search_by_key(&index, |(i, _)| *i) {
        Ok(at) => list[at].1 = value,
        Err(at) => list.insert(at, (index, value)),
    }
}

/// Synced values of one player's entity: what the clients hold, what the observers still miss.
#[derive(Debug, Default)]
pub(crate) struct PlayerEntityData {
    /// Values that differ from the entity defaults, in index order: what a client that knows the
    /// entity holds, and what an observer paired now is told.
    held: Vec<(u8, Meta)>,
    /// Values written since the last tracking pass, in index order, the last write of an index
    /// only; kept while the world is open to other players.
    unsent: Vec<(u8, Meta)>,
    /// The entity was replaced since the last tracking pass (respawn): the observers drop it and
    /// are paired with the new one.
    replaced: bool,
    /// Synced attributes sent to the player's own client since the last tracking pass, in id
    /// order, the last send of an attribute only; kept while the world is open to other players.
    unsent_attributes: Vec<Attribute>,
}

impl PlayerEntityData {
    /// Values written for the entity; `for_observers`: those that change what the clients hold are
    /// kept for the next tracking pass too (the game only marks a value that differs).
    pub(crate) fn record(&mut self, values: &[(u8, Meta)], for_observers: bool) {
        for (index, value) in values {
            let is_default = player_default(*index).as_ref() == Some(value);
            let changed = match self.held.binary_search_by_key(index, |(i, _)| *i) {
                Ok(at) if self.held[at].1 == *value => false,
                Ok(at) if is_default => {
                    self.held.remove(at);
                    true
                }
                Ok(at) => {
                    self.held[at].1 = value.clone();
                    true
                }
                Err(_) if is_default => false,
                Err(at) => {
                    self.held.insert(at, (*index, value.clone()));
                    true
                }
            };
            if for_observers && changed {
                put_in_index_order(&mut self.unsent, *index, value.clone());
            }
        }
    }

    /// Do the clients already hold `value` at `index`?
    pub(crate) fn holds(&self, index: u8, value: &Meta) -> bool {
        match self.held.binary_search_by_key(&index, |(i, _)| *i) {
            Ok(at) => self.held[at].1 == *value,
            Err(_) => player_default(index).as_ref() == Some(value),
        }
    }

    /// What an observer paired now is told: the values that differ from the defaults, in index
    /// order.
    pub(crate) fn pairing_values(&self) -> &[(u8, Meta)] {
        &self.held
    }

    /// The values written since the last tracking pass, for the observers already paired.
    pub(crate) fn take_unsent(&mut self) -> Vec<(u8, Meta)> {
        std::mem::take(&mut self.unsent)
    }

    /// The player entity is replaced by a new one (respawn): it starts from the defaults, and
    /// what the old one had not sent is lost with it.
    pub(crate) fn replace_entity(&mut self) {
        self.held.clear();
        self.unsent.clear();
        self.unsent_attributes.clear();
        self.replaced = true;
    }

    /// Attributes just sent to the player's own client, for the observers' next tracking pass.
    pub(crate) fn record_attributes(&mut self, sent: &[Attribute]) {
        for a in sent {
            match self.unsent_attributes.binary_search_by_key(&a.id, |b| b.id) {
                Ok(at) => self.unsent_attributes[at] = a.clone(),
                Err(at) => self.unsent_attributes.insert(at, a.clone()),
            }
        }
    }

    /// The attributes changed since the last tracking pass, for the observers already paired.
    pub(crate) fn take_unsent_attributes(&mut self) -> Vec<Attribute> {
        std::mem::take(&mut self.unsent_attributes)
    }

    /// Was the entity replaced since the last call?
    pub(crate) fn take_replaced(&mut self) -> bool {
        std::mem::take(&mut self.replaced)
    }
}

impl crate::server::Server {
    /// Synced value `index` of the installed player's own entity told to the other players only
    /// (at the tracking pass, when it changed): its own client already knows it (its pose, its
    /// shift key, its health from the health packet) and is not sent this copy.
    pub(crate) fn note_own_entity_value(&self, index: u8, value: Meta) {
        let world_open = self.lan.is_some();
        self.entity_data.borrow_mut().record(&[(index, value)], world_open);
    }

    /// Attributes of the installed player just sent to its own client: told to the other players
    /// at the next tracking pass, like the game's tracker sends a changed attribute to the
    /// players tracking the entity as well as to the player itself.
    pub(crate) fn note_own_attributes(&self, sent: &[Attribute]) {
        if self.lan.is_some() {
            self.entity_data.borrow_mut().record_attributes(sent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_value_is_not_sent_again_to_the_observers() {
        let mut d = PlayerEntityData::default();
        d.record(&[(1, Meta::Int(300)), (6, Meta::Pose(0))], true);
        assert!(d.take_unsent().is_empty(), "defaults the observers already hold");
        d.record(&[(6, Meta::Pose(5))], true);
        d.record(&[(6, Meta::Pose(5))], true);
        assert_eq!(d.take_unsent(), vec![(6, Meta::Pose(5))]);
        d.record(&[(6, Meta::Pose(5))], true);
        assert!(d.take_unsent().is_empty(), "the next tick: still crouching, nothing to tell");
    }

    #[test]
    fn values_back_to_their_default_are_no_longer_held() {
        let mut d = PlayerEntityData::default();
        d.record(&[(9, Meta::Float(20.0)), (1, Meta::Int(250))], false);
        assert_eq!(d.pairing_values(), &[(1, Meta::Int(250)), (9, Meta::Float(20.0))], "index order");
        d.record(&[(1, Meta::Int(300))], false);
        assert_eq!(d.pairing_values(), &[(9, Meta::Float(20.0))], "full air is the default");
        assert!(d.holds(1, &Meta::Int(300)) && d.holds(9, &Meta::Float(20.0)) && !d.holds(9, &Meta::Float(1.0)));
        assert!(d.take_unsent().is_empty(), "nothing kept for observers in a closed world");
    }

    #[test]
    fn the_last_write_of_a_tick_goes_to_the_observers_once() {
        let mut d = PlayerEntityData::default();
        d.record(&[(7, Meta::Int(1)), (0, Meta::Byte(2))], true);
        d.record(&[(7, Meta::Int(2))], true);
        d.record(&[(7, Meta::Int(0))], true);
        assert_eq!(d.take_unsent(), vec![(0, Meta::Byte(2)), (7, Meta::Int(0))]);
        assert!(d.take_unsent().is_empty());
        assert_eq!(d.pairing_values(), &[(0, Meta::Byte(2))]);
    }

    #[test]
    fn a_replaced_entity_starts_from_the_defaults() {
        let mut d = PlayerEntityData::default();
        d.record(&[(9, Meta::Float(0.0)), (12, Meta::Int(3))], true);
        d.replace_entity();
        assert!(d.pairing_values().is_empty() && d.take_unsent().is_empty());
        assert!(d.take_replaced());
        assert!(!d.take_replaced(), "told once");
    }
}
