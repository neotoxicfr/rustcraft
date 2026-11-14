//! The players registered in a boss bar that belongs to a world: the dragon fight's and each
//! raid's. Like the game, a bar keeps its own set of players, refreshed from the players of its
//! world; a change of the bar goes to each registered player alone, and a player who joins or
//! leaves the set alone gets the bar added or removed.

use rc_client::Event;
use rc_proto_v777::play::{BossEvent, BossOp};
use smallvec::SmallVec;

use crate::server::Server;

/// Profiles (UUIDs) of the registered players: an open world holds eight players at most, so the
/// set stays inline.
pub type Profiles = SmallVec<[u128; 8]>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BarViewers {
    profiles: Profiles,
}

impl BarViewers {
    pub(crate) fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    pub(crate) fn contains(&self, uuid: u128) -> bool {
        self.profiles.contains(&uuid)
    }

    pub(crate) fn profiles(&self) -> &[u128] {
        &self.profiles
    }

    /// Registers exactly the players of `wanted`; returns those it added and those it removed.
    pub(crate) fn replace_with(&mut self, wanted: &[u128]) -> (Profiles, Profiles) {
        let removed: Profiles = self.profiles.iter().copied().filter(|u| !wanted.contains(u)).collect();
        let added: Profiles = wanted.iter().copied().filter(|u| !self.profiles.contains(u)).collect();
        self.profiles.retain(|u| wanted.contains(u));
        self.profiles.extend(added.iter().copied());
        (added, removed)
    }

    /// Unregisters `uuid`; whether it was registered.
    pub(crate) fn remove(&mut self, uuid: u128) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|u| *u != uuid);
        self.profiles.len() != before
    }

    /// Unregisters everyone; returns who was registered.
    pub(crate) fn take_all(&mut self) -> Profiles {
        std::mem::take(&mut self.profiles)
    }

    /// Test helper: a set holding `uuid`.
    #[cfg(test)]
    pub(crate) fn of(uuid: u128) -> BarViewers {
        BarViewers { profiles: smallvec::smallvec![uuid] }
    }
}

impl Server {
    /// Change `op` of bar `bar` sent to each player of `profiles` still in game, to that player
    /// alone. From a world nobody stands in (muted), nothing leaves, like the other world events.
    pub(crate) fn send_bar_to(&self, profiles: &[u128], bar: u128, op: &BossOp) {
        if self.muted {
            return;
        }
        for &uuid in profiles {
            if let Some(seat) = self.seat_of_uuid(uuid) {
                self.send_to(seat, Event::Boss(BossEvent { id: bar, op: op.clone() }));
            }
        }
    }

    /// Profiles of the living players standing in the world the server works in that `keep`
    /// accepts (given their feet position), the installed one first.
    pub(crate) fn profiles_of_players_here(&self, keep: impl Fn([f64; 3]) -> bool) -> Profiles {
        self.players_standing_here().into_iter().filter(|p| keep(p.pos)).map(|p| p.uuid).collect()
    }

    /// Seats (`None`: the host) of the living players of `profiles` standing in the world the
    /// server works in, for `as_player`.
    pub(crate) fn seats_here_of(&self, profiles: &[u128]) -> SmallVec<[Option<usize>; 8]> {
        self.players_standing_here().into_iter().filter(|p| profiles.contains(&p.uuid)).map(|p| p.seat).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The set follows the wanted players: newcomers and leavers reported once each, the others
    /// kept. Sabotage verified (failure): the removed list taken from the wanted players.
    #[test]
    fn viewers_report_who_joins_and_who_leaves() {
        let mut v = BarViewers::default();
        assert_eq!(v.replace_with(&[1, 2]), (smallvec::smallvec![1, 2], Profiles::new()));
        assert_eq!(v.replace_with(&[2, 3]), (smallvec::smallvec![3], smallvec::smallvec![1]));
        assert_eq!(v.profiles(), &[2, 3]);
        assert!(v.remove(2) && !v.remove(2));
        assert_eq!(v.take_all().as_slice(), &[3]);
        assert!(v.is_empty());
    }
}
