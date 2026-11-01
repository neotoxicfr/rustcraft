//! G5-F02d: thorns worn by the player, an after-hit effect of the victim on the entity
//! that caused the hit (the melee attacker, the shooter of a projectile).
//!
//! Called only by the attacks that have after-hit effects, like the game: melee, charges,
//! dragon bite and wing, arrows, trident, fireballs, wither skull, shulker bullet, llama spit,
//! evoker fangs, wind charge. Never by an explosion, the guardian beam, the sonic boom, the
//! pufferfish or `/damage`.
//!
//! G5-F04: the effects come from the pack (`ench_fx`): the player's equipment (main hand, off
//! hand, feet, legs, chest, head), then, for a melee hit, the attacker's weapon.

use crate::mobs::mob::{Ent, Mob};
use crate::server::Server;

impl Server {
    /// After-hit effects of a melee hit of the mob `attacker` (of type `ty`) on the player: the
    /// player's equipment (Thorns: each enchanted item tries its luck, 0.15 × level on the world
    /// random; on success 1 to 5 `thorns` damage drawn on the attacker's random, caused by the
    /// player, then 2 points of wear on the item), then the attacker's weapon.
    pub(crate) fn player_thorns(&mut self, attacker: &mut Mob, ty: &str) {
        let weapon = attacker.equipment[0].clone();
        self.melee_post_attack(attacker, Ent::Player, ty, weapon.as_ref());
    }

    /// The brain `b`, taken out during the tick of `m`, put back on the mob for the time of `f`:
    /// a hit dealt by a behaviour may hurt its author (thorns), and its reaction to the damage
    /// reads and writes this brain on the spot, like in the game where it never leaves the mob.
    pub(crate) fn with_brain<R>(
        &mut self,
        m: &mut Mob,
        b: &mut crate::mobs::brainy::brain::Brain,
        f: impl FnOnce(&mut Server, &mut Mob) -> R,
    ) -> R {
        m.bm.brain = Some(Box::new(std::mem::replace(b, crate::mobs::brainy::brain::Brain::hole())));
        let r = f(self, m);
        if let Some(back) = m.bm.brain.take() {
            *b = *back;
        }
        r
    }

    /// The player's after-hit effects for the causing entity `by` of a projectile hit of type
    /// `ty`: a mob still in the list (shooter gone or the player itself: nothing).
    pub(crate) fn player_thorns_by(&mut self, by: Option<Ent>, ty: &str) {
        if matches!(by, Some(Ent::Mob(_))) {
            self.post_attack_by(Ent::Player, by, ty);
        }
    }
}
