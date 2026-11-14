//! The player a mob deals with in a game open to the local network.
//!
//! A mob's code names "the player" (`Ent::Player`): the player installed while it ticks, the one
//! the entity walk installed (the host in its own world). A mob that picked another player of its
//! world (a target search, a hit from them) is bound to that player by entity id and ticks with
//! that player installed, so everything it does with "the player" (chase, attack, stare, anger,
//! the credit of its death) concerns them, like the game's mobs whose target is any player.
//! Unbound (always in singleplayer), it keeps the walk's player.

use super::mob::{Ent, Mob};
use crate::server::Server;

/// A living player standing in the world the server works in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlayerHere {
    /// Seat for `switch_installed_player` (`None`: the host).
    pub seat: Option<usize>,
    pub id: i32,
    pub pos: [f64; 3],
    pub game_mode: i32,
}

/// The players standing in a world: the host and the seven guest seats at most.
pub(crate) type PlayersHere = smallvec::SmallVec<[PlayerHere; 8]>;

impl Mob {
    /// Entity id of the player this mob deals with (`None`: the walk's player).
    pub(crate) fn bound_player(&self) -> Option<i32> {
        self.bound_player_id.map(std::num::NonZeroI32::get)
    }

    /// The mob now deals with the player of entity `id`.
    pub(crate) fn bind_to_player(&mut self, id: i32) {
        self.bound_player_id = std::num::NonZeroI32::new(id);
    }

    /// The bound player is not in the mob's world (gone to another world, out of the game, or
    /// dead): the mob stops chasing them, instead of turning on the player installed instead.
    /// It stays bound to them while that matters: a neutral mob's anger at them lasts its time,
    /// as in the game, where it names that player wherever they are (back within it, they are
    /// attacked again); a running target objective stops at its next test instead of going on
    /// with the player installed. Then it forgets them.
    pub(crate) fn bound_player_absent(&mut self, now: i64) {
        self.stop_chasing_the_player();
        let angry_at_them = self.anger_target == Some(Ent::Player) && self.anger_end > 0 && self.anger_end > now;
        if !angry_at_them && !self.targets.running(|_| true) {
            self.bound_player_id = None;
            if self.anger_target == Some(Ent::Player) {
                self.anger_target = None;
            }
        }
    }

    /// The target, attacker and attack memory naming "the player" are dropped.
    fn stop_chasing_the_player(&mut self) {
        if self.target == Some(Ent::Player) {
            self.target = None;
        }
        if matches!(self.last_hurt_by_mob, Some((Ent::Player, _))) {
            self.last_hurt_by_mob = None;
        }
        if let Some(b) = self.bm.brain.as_mut()
            && b.mem.attack.v == Some(Ent::Player)
        {
            b.mem.attack.erase();
        }
    }

    /// The mob takes `target` from `caller` (a call for help, a reinforcement): a player target
    /// is the caller's player, so the mob is bound like the caller.
    pub(crate) fn take_target_from(&mut self, caller: &Mob, target: Ent) {
        self.target = Some(target);
        if target == Ent::Player {
            self.bound_player_id = caller.bound_player_id;
        }
    }

    /// Whether "the player" of this mob is the player of entity `id` (an unbound mob means the
    /// walk's player, whoever that is).
    pub(crate) fn may_mean_player(&self, id: i32) -> bool {
        self.bound_player().is_none_or(|bound| bound == id)
    }
}

impl Server {
    /// The living players standing in the world the server works in, the installed one first
    /// (when it stands here), then the others in seat order. An open world holds eight players
    /// at most: the list stays on the stack.
    pub(crate) fn players_standing_here(&self) -> PlayersHere {
        let installed = (self.player_stands_here() && !self.life.vit.dead).then(|| PlayerHere {
            seat: self.installed_seat(),
            id: self.player_id,
            pos: self.pos,
            game_mode: self.game_mode,
        });
        let others = self.kept_players().filter(|(_, c)| !c.vit.dead && self.stands_here(&c.client_world));
        let others = others.map(|(seat, c)| PlayerHere { seat, id: c.player_id, pos: c.pos, game_mode: c.game_mode });
        installed.into_iter().chain(others).collect()
    }

    /// The players of this world other than spectators closer than `range` to `pos`, nearest
    /// first, written into `out` (a brain's nearby players sensor, its list refilled in place).
    pub(crate) fn players_within(&self, pos: [f64; 3], range: f64, out: &mut Vec<i32>) {
        let mut near: smallvec::SmallVec<[(f64, i32); 8]> = self
            .players_standing_here()
            .into_iter()
            .filter(|p| p.game_mode != 3)
            .map(|p| (super::mth::dist2(p.pos, pos), p.id))
            .filter(|&(d, _)| d < range * range)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.clear();
        out.extend(near.into_iter().map(|(_, id)| id));
    }

    /// Seat of the living player of entity `id` standing in the world the server works in (the
    /// installed one included).
    pub(crate) fn seat_of_player_here(&self, id: i32) -> Option<Option<usize>> {
        if id == self.player_id {
            return (self.player_stands_here() && !self.life.vit.dead).then(|| self.installed_seat());
        }
        let mut here = self.kept_players().filter(|(_, c)| !c.vit.dead && self.stands_here(&c.client_world));
        here.find(|(_, c)| c.player_id == id).map(|(seat, _)| seat)
    }

    /// Installs, for mob `m`, the player of seat `seat`: the lines of sight the mob already traced
    /// to "the player" this tick were to the previous one.
    pub(crate) fn install_player_for_mob(&mut self, m: &mut Mob, seat: Option<usize>) {
        if self.installed_seat() != seat {
            self.switch_installed_player(seat);
            m.sight.retain(|(e, _)| *e != Ent::Player);
        }
    }

    /// Ticks mob `m` with the player it is bound to installed (`tick`), then puts the walk's
    /// player back, also when the tick bound it to another one. A mob whose bound player is not
    /// in its world, the walk's player included, stops chasing them
    /// ([`Mob::bound_player_absent`]); a mob carrying the installed player keeps it (its riding
    /// state).
    pub(crate) fn with_mob_player(&mut self, m: &mut Mob, tick: impl FnOnce(&mut Server, &mut Mob)) {
        if self.lan.is_none() {
            return tick(self, m);
        }
        let walk_player = self.installed_seat();
        if let Some(id) = m.bound_player() {
            match self.seat_of_player_here(id) {
                Some(seat) if !self.carries_player(m) => self.install_player_for_mob(m, seat),
                Some(_) => {}
                None => m.bound_player_absent(self.time),
            }
        }
        tick(self, m);
        self.switch_installed_player(walk_player);
    }

    /// Whether target `t` of mob `m` is the mob's: any mob, or "the player" when the installed
    /// one is the player the mob deals with (bound to a player away from its world, the mob does
    /// not go on with whoever is installed instead).
    pub(crate) fn is_mobs_player(&self, m: &Mob, t: Ent) -> bool {
        t != Ent::Player || m.may_mean_player(self.player_id)
    }

    /// The installed player becomes the one mob `m` deals with (it hit the mob, or the mob
    /// noticed them): only on the local network, where several players may be meant.
    pub(crate) fn bind_mob_to_installed_player(&self, m: &mut Mob) {
        if self.lan.is_some() {
            m.bind_to_player(self.player_id);
        }
    }

    /// The player of this world a search of mob `m` picks: the candidates ordered by `rank`
    /// (lower first; at equal rank the installed player, then by seat), the first one passing
    /// `test`, run with that player installed. The mob is bound to the one picked, who stays
    /// installed for the rest of its tick; nobody passing, the player installed before stays.
    /// In singleplayer only the player is tried, as before.
    pub(crate) fn pick_player(
        &mut self,
        m: &mut Mob,
        rank: impl Fn([f64; 3]) -> f64,
        mut test: impl FnMut(&mut Server, &mut Mob) -> bool,
    ) -> bool {
        if self.lan.is_none() {
            return test(self, m);
        }
        let before = self.installed_seat();
        let mut candidates = self.players_standing_here();
        // Stable: equal ranks keep the installed player first, then the seat order.
        candidates.sort_by(|a, b| rank(a.pos).total_cmp(&rank(b.pos)));
        for c in candidates {
            self.install_player_for_mob(m, c.seat);
            if test(self, m) {
                m.bind_to_player(c.id);
                return true;
            }
        }
        self.install_player_for_mob(m, before);
        false
    }

    /// [`Self::pick_player`] by distance to `from`: the nearest player passing `test`.
    pub(crate) fn pick_nearest_player(
        &mut self,
        m: &mut Mob,
        from: [f64; 3],
        test: impl FnMut(&mut Server, &mut Mob) -> bool,
    ) -> bool {
        self.pick_player(m, |p| super::mth::dist2(p, from), test)
    }
}
