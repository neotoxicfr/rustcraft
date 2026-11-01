//! G5-F02d: thorns worn by the player, an after-hit effect of the victim on the entity
//! that caused the hit (the melee attacker, the shooter of a projectile).
//!
//! Called only by the attacks that have after-hit effects, like the game: melee, charges,
//! dragon bite and wing, arrows, trident, fireballs, wither skull, shulker bullet, llama spit,
//! evoker fangs, wind charge. Never by an explosion, the guardian beam, the sonic boom, the
//! pufferfish or `/damage`.

use crate::mobs::mob::{Ent, Mob};
use crate::server::Server;

impl Server {
    /// Thorns of the player hit by the mob `attacker`. Equipment walked main hand, off hand,
    /// feet, legs, chest, head; each enchanted item tries its luck (0.15 × level, world
    /// random); on success: 1 to 5 `thorns` damage (draw from the attacker's random) caused by
    /// the player, then 2 points of wear on the item (unbreaking, creative mode).
    pub(crate) fn player_thorns(&mut self, attacker: &mut Mob) {
        for slot in [36 + self.held as usize, 45, 8, 7, 6, 5] {
            let Some(s) = &self.slots[slot] else { continue };
            let lvl = self.life.items.enchantment(s, "minecraft:thorns");
            if lvl <= 0 || self.mobs.rng.next_float() >= 0.15 * lvl as f32 {
                continue;
            }
            let dmg = attacker.rng.next_float() * 4.0 + 1.0;
            let pos = self.pos;
            self.mob_hurt(attacker, "minecraft:thorns", dmg, Some(Ent::Player), Some(pos));
            self.hurt_item(slot, 2);
        }
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

    /// `player_thorns` for the causing entity `by` of a projectile: a mob still in the list
    /// (shooter gone or the player itself: nothing).
    pub(crate) fn player_thorns_by(&mut self, by: Option<Ent>) {
        let Some(Ent::Mob(id)) = by else { return };
        let Some(i) = self.mob_index(id) else { return };
        let Some(mut m) = self.mobs.list[i].take() else { return };
        self.player_thorns(&mut m);
        self.mobs.list[i] = Some(m);
    }
}
