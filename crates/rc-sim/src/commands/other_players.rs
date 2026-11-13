//! The other players of a world open to the local network, for commands. The game's commands see
//! every player of its list; ours run with one player installed as the server's own (`E::Player`).
//! The others are named `E::OtherPlayer(entity id)`: the selectors find them, their kept state
//! answers the readers in place, and a command acts on one of them with that player installed
//! for the time of the effect (`Server::on_player`), so the singleplayer code acts on it unchanged.

use super::ents::E;
use crate::lan::ctx::PlayerCtx;
use crate::server::Server;

/// A player of the list, as the selectors walk it.
#[derive(Clone, Copy)]
pub(crate) enum ListedPlayer<'a> {
    /// The installed player (`E::Player`).
    Current,
    /// Another player, read in its kept state.
    Other(&'a PlayerCtx),
}

impl ListedPlayer<'_> {
    pub(crate) fn entity(&self) -> E {
        match self {
            ListedPlayer::Current => E::Player,
            ListedPlayer::Other(c) => E::OtherPlayer(c.player_id),
        }
    }
}

impl Server {
    /// Kept state of the other player of entity `id` (the host's while a guest is installed);
    /// `None` when no other player in game has it.
    pub(crate) fn other_player(&self, id: i32) -> Option<&PlayerCtx> {
        self.other_players().find(|c| c.player_id == id)
    }

    /// Seat of the other player of entity `id`, in the form `as_player` expects (`None`: the
    /// host).
    fn seat_of_other_player(&self, id: i32) -> Option<Option<usize>> {
        self.kept_players().find(|(_, c)| c.player_id == id).map(|(seat, _)| seat)
    }

    /// Seat of player `e` in the form `as_player` expects (`None`: the host); `None` when `e` is
    /// not a player in game.
    pub(crate) fn seat_of_player(&self, e: E) -> Option<Option<usize>> {
        match e {
            E::Player => Some(self.lan.as_ref().and_then(|l| l.active)),
            E::OtherPlayer(id) => self.seat_of_other_player(id),
            _ => None,
        }
    }

    /// Every player in game, in the order of the game's player list: the host, then the guests
    /// by arrival. Walked in place: a selector search walks it without building a list.
    pub(crate) fn listed_players(&self) -> impl Iterator<Item = ListedPlayer<'_>> + '_ {
        // While a guest is installed, its seat holds the host's state (seat `None`).
        let host = match self.kept_players().find(|(seat, _)| seat.is_none()) {
            Some((_, c)) => ListedPlayer::Other(c),
            None => ListedPlayer::Current,
        };
        // The installed guest keeps its place among the guests by arrival.
        let guests = self.kept_players().map(|(seat, c)| match seat {
            None => ListedPlayer::Current,
            Some(_) => ListedPlayer::Other(c),
        });
        std::iter::once(host).chain(guests)
    }

    /// [`Server::listed_players`] as entity references.
    pub(crate) fn players_in_list_order(&self) -> Vec<E> {
        self.listed_players().map(|p| p.entity()).collect()
    }

    /// The other players in game, the host first while a guest is installed, without building a
    /// list (empty in singleplayer).
    pub(crate) fn other_players(&self) -> impl Iterator<Item = &PlayerCtx> + '_ {
        self.listed_players().filter_map(|p| match p {
            ListedPlayer::Other(c) => Some(c),
            ListedPlayer::Current => None,
        })
    }

    /// Whether `c` is the host's kept state (while a guest is installed).
    pub(crate) fn is_host(&self, c: &PlayerCtx) -> bool {
        self.lan.as_ref().is_some_and(|l| l.shared.host.1 == c.uuid)
    }

    /// Whether another player is in game (an open world with a guest): without one, the
    /// selectors keep their singleplayer path.
    pub(crate) fn has_other_players(&self) -> bool {
        self.other_players().next().is_some()
    }

    /// The player of the list named `name` (case ignored, the first in list order), like the
    /// game's lookup by name.
    pub(crate) fn player_named(&self, name: &str) -> Option<E> {
        let named = |n: &str| super::select::java_eq_ignore_case(name, n);
        self.listed_players().find_map(|p| match p {
            ListedPlayer::Current => named(&self.name).then_some(E::Player),
            ListedPlayer::Other(c) => named(&c.name).then_some(E::OtherPlayer(c.player_id)),
        })
    }

    /// The player of the list whose profile is `uuid`.
    pub(crate) fn player_with_uuid(&self, uuid: u128) -> Option<E> {
        if self.uuid == uuid {
            return Some(E::Player);
        }
        self.other_players().find(|c| c.uuid == uuid).map(|c| E::OtherPlayer(c.player_id))
    }

    /// Game mode of a player of the list (`None`: not a player).
    pub(crate) fn player_game_mode(&self, e: E) -> Option<i32> {
        match e {
            E::Player => Some(self.game_mode),
            E::OtherPlayer(id) => self.other_player(id).map(|c| c.game_mode),
            _ => None,
        }
    }

    /// Experience level of a player of the list (`None`: not a player).
    pub(crate) fn player_xp_level(&self, e: E) -> Option<i32> {
        match e {
            E::Player => Some(self.life.vit.xp_level),
            E::OtherPlayer(id) => self.other_player(id).map(|c| c.vit.xp_level),
            _ => None,
        }
    }

    /// Reference to entity `x` once player `target` is installed in place of the current one:
    /// the current player becomes another player, the target becomes `E::Player`.
    pub(crate) fn seen_from_player(&self, x: E, target: E) -> E {
        match x {
            E::Player if target != E::Player => E::OtherPlayer(self.player_id),
            E::OtherPlayer(_) if x == target => E::Player,
            _ => x,
        }
    }

    /// Runs `f` with player `e` installed as the current player (`E::Player` names it inside
    /// `f`), in its own world, the server's state restored afterwards; `None` when `e` is not a
    /// player in game. Entity references taken outside `f` other than players stay valid inside
    /// only when that player stands in the same world.
    pub(crate) fn on_player<T>(&mut self, e: E, f: impl FnOnce(&mut Server) -> T) -> Option<T> {
        match e {
            E::Player => Some(f(self)),
            E::OtherPlayer(id) => {
                let seat = self.seat_of_other_player(id)?;
                Some(self.as_player(seat, f))
            }
            _ => None,
        }
    }

    /// `f` on each player of `targets` installed in turn (other entities skipped), in order.
    pub(crate) fn on_each_player<T>(&mut self, targets: &[E], mut f: impl FnMut(&mut Server) -> T) -> Vec<T> {
        targets.iter().filter_map(|&e| self.on_player(e, &mut f)).collect()
    }
}
