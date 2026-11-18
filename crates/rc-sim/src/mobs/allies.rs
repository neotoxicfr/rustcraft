//! Allies: two entities are allies when one of them counts the other as an ally (the relation is
//! tested both ways). By default an entity counts the members of its scoreboard team; a tamed
//! animal counts its owner and whatever its owner counts; illagers count each other while
//! neither is on a team; an evoker counts the vexes it (or an ally) summoned; an allay counts the
//! player it likes. Targeting never aims at an ally, and a mob calling for help skips the kin that
//! are allies of the aggressor.

use super::mob::{Ent, Mob};
use super::species::Kind;
use crate::server::Server;

/// One side of an ally test. A mob is held by reference: the mob being ticked is out of the list,
/// so it could not be found again by its identifier.
#[derive(Clone, Copy)]
pub(crate) enum Party<'a> {
    Player,
    Mob(&'a Mob),
}

impl Party<'_> {
    fn is(self, other: Party) -> bool {
        match (self, other) {
            (Party::Player, Party::Player) => true,
            (Party::Mob(a), Party::Mob(b)) => a.id == b.id,
            _ => false,
        }
    }
}

impl Server {
    /// The entity `e` as one side of an ally test (`None`: a mob no longer here).
    pub(crate) fn party(&self, e: Ent) -> Option<Party<'_>> {
        match e {
            Ent::Player => Some(Party::Player),
            Ent::Mob(id) => self.mob_ref(id).map(Party::Mob),
        }
    }

    /// Identifiers of the mobs of `kin` that are allies of `attacker`: a call for help against it
    /// leaves them out. Read before the call itself, which changes the mobs.
    pub(crate) fn kin_allied_to<'a>(&'a self, attacker: Ent, kin: impl Iterator<Item = &'a Mob>) -> Vec<i32> {
        let Some(a) = self.party(attacker) else { return Vec::new() };
        kin.filter(|o| self.allied(Party::Mob(o), a)).map(|o| o.id).collect()
    }

    /// `a` and `b` are allies (the same entity included).
    pub(crate) fn allied(&self, a: Party, b: Party) -> bool {
        a.is(b) || self.counts_as_ally(a, b) || self.counts_as_ally(b, a)
    }

    /// Both are on the same scoreboard team.
    pub(crate) fn on_same_team(&self, a: Party, b: Party) -> bool {
        // No team at all, the usual world: nothing to look up.
        if self.cmd.scores.teams.is_empty() {
            return false;
        }
        matches!((self.team_of_party(a), self.team_of_party(b)), (Some(x), Some(y)) if x == y)
    }

    fn team_of_party(&self, p: Party) -> Option<&str> {
        let team = match p {
            Party::Player => self.cmd.scores.team_of(&self.name),
            Party::Mob(m) => self.cmd.scores.team_of_uuid(m.uuid),
        };
        team.map(|t| t.name.as_str())
    }

    fn on_a_team(&self, p: Party) -> bool {
        !self.cmd.scores.teams.is_empty() && self.team_of_party(p).is_some()
    }

    /// Whether `who` counts `other` as an ally (one way only).
    fn counts_as_ally(&self, who: Party, other: Party) -> bool {
        let Party::Mob(m) = who else { return self.on_same_team(who, other) };
        // A tamed animal whose owner is here sees through its owner's eyes, its own team aside.
        if self.owner_is_here(m) {
            return other.is(Party::Player) || self.counts_as_ally(Party::Player, other);
        }
        if self.on_same_team(who, other) {
            return true;
        }
        match m.kind {
            Kind::Evoker => self.illager_counts(who, other) || self.evoker_counts_vex(m, other),
            k if super::illagers::is_illager(k) => self.illager_counts(who, other),
            Kind::Allay => other.is(Party::Player) && super::brainy::allay::likes_player(self, m),
            _ => false,
        }
    }

    /// A tamed animal whose owner, the player, is here.
    fn owner_is_here(&self, m: &Mob) -> bool {
        super::species::tamable(m.kind) && m.tame && m.owner == Some(self.uuid)
    }

    /// An illager counts another illager (the illagers' friends) while neither is on a team.
    fn illager_counts(&self, who: Party, other: Party) -> bool {
        let other_is_illager = matches!(other, Party::Mob(o) if super::illagers::is_illager(o.kind));
        other_is_illager && !self.on_a_team(who) && !self.on_a_team(other)
    }

    /// An evoker counts a vex whose summoner is itself, or an entity it counts as an illager ally.
    fn evoker_counts_vex(&self, evoker: &Mob, other: Party) -> bool {
        let Party::Mob(vex) = other else { return false };
        if vex.kind != Kind::Vex {
            return false;
        }
        let Some(root_id) = vex.ill.owner_id else { return false };
        if root_id == evoker.id {
            return true;
        }
        let Some(root) = self.mob_ref(root_id) else { return false };
        let (me, root) = (Party::Mob(evoker), Party::Mob(root));
        self.on_same_team(me, root) || self.illager_counts(me, root)
    }

    /// The game's can-attack test of mob `m` against `target` (alive and seen as an enemy, never a
    /// ghast, never a player in Peaceful or out of reach of attacks, never its owner for a tamed
    /// animal, then the species' own rules).
    pub(crate) fn can_attack(&self, m: &Mob, target: Ent) -> bool {
        if !self.ent_alive(target) {
            return false;
        }
        match target {
            Ent::Player => {
                if !self.player_attackable() || self.owner_is_here(m) {
                    return false;
                }
            }
            Ent::Mob(id) => {
                let Some(o) = self.mob_ref(id) else { return false };
                // Never a ghast; a mob target must be seen as an enemy: not invulnerable, not an
                // axolotl playing dead.
                if o.kind == Kind::Ghast || !super::brainy::seen_as_enemy(o) {
                    return false;
                }
            }
        }
        // F06: the iron golem's own test. F07: baby villagers spared by illagers. F09: the baby
        // dolphin does not attack.
        super::golems::can_attack(self, m, target)
            && super::illagers::can_attack(self, m, target)
            && super::sealife::can_attack(m)
    }
}
