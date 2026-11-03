//! How blocks react to the player hitting and breaking them: hit at the start of mining
//! (dragon egg teleported, note block note, redstone ore lit), just before the
//! break (unstable TNT primed, cracked decorated pot) and just after (ice turned into water).

use super::{Kind, P, below, flag};
use crate::player::Stack;
use crate::server::Server;

impl Server {
    /// Just before the player breaks it: returns the broken state (pot
    /// cracked by a tool that breaks pots, without Silk Touch: loot is sherds).
    pub(crate) fn player_will_destroy(&mut self, p: P, s: u32, tool: Option<&Stack>) -> u32 {
        let name = self.data.block_of(s).map_or("", |b| b.name.as_str());
        let (tnt, pot) = (name == "minecraft:tnt", name == "minecraft:decorated_pot");
        if tnt && self.game_mode != 1 && self.view().get_bool(s, "unstable") {
            self.tnt_prime_block(p, None);
        }
        if pot
            && tool.is_some_and(|t| {
                self.life.items.has_tag("minecraft:breaks_decorated_pots", &t.id)
                    && self.life.items.enchantment(t, "minecraft:silk_touch") == 0
            })
        {
            let cracked = self.view().set_bool(s, "cracked", true);
            self.set_block(p, cracked, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
            return cracked;
        }
        // Wire cut with shears (main hand): disarmed before the break, so the hooks
        // re-triggered by the removal (which reads the state placed here) do not receive a pulse.
        // The returned state stays the one from before, as in the game.
        if self.view().kind(s) == Kind::Tripwire && tool.is_some_and(|t| t.count > 0 && t.id == "minecraft:shears") {
            let disarmed = self.view().set_bool(s, "disarmed", true);
            self.set_block(p, disarmed, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
            self.game_event_block("minecraft:shear", p, Some(crate::mobs::mob::Ent::Player), None);
        }
        s
    }

    /// Just after the player breaks it, loot included (not in creative): turtle eggs put back
    /// with one egg fewer; ice without Silk Touch above a block that blocks movement or a liquid
    /// → water (the Nether evaporates it).
    pub(crate) fn player_destroyed(&mut self, p: P, s: u32, tool: Option<&Stack>) {
        // G6-F08: turtle eggs broken after their loot: one egg fewer put back.
        if self.view().block(s) == self.bs.t.id.turtle_egg {
            self.turtle_egg_decrease(p, s);
            return;
        }
        let ice =
            self.data.block_of(s).is_some_and(|b| matches!(b.name.as_str(), "minecraft:ice" | "minecraft:frosted_ice"));
        if !ice || tool.is_some_and(|t| self.life.items.enchantment(t, "minecraft:silk_touch") > 0) {
            return;
        }
        if self.dim.name == "minecraft:the_nether" {
            self.remove_block(p, false);
            return;
        }
        let under = self.get(below(p));
        let ub = self.view().block(under);
        let melts = self
            .block_tags
            .get("minecraft:ice_melts_when_destroyed_above")
            .is_some_and(|t| t.binary_search(&u32::from(ub)).is_ok());
        let liquid = self.data.block_of(under).is_some_and(|b| {
            matches!(b.name.as_str(), "minecraft:water" | "minecraft:lava" | "minecraft:bubble_column")
        });
        if melts || liquid {
            let water = self.view().default_state(self.bs.t.id.water);
            self.set_block(p, water, flag::ALL);
        }
    }
    /// Reaction of block `s` at `p` hit by the player (before the mining progress is computed).
    pub(crate) fn block_attack(&mut self, p: P, s: u32) {
        match self.view().kind(s) {
            Kind::DragonEgg => self.dragon_egg_teleport(p, s),
            Kind::NoteBlock => {
                self.note_play(p, s, Some(crate::mobs::mob::Ent::Player));
                self.award_stat("minecraft:custom", "minecraft:play_noteblock", 1);
            }
            _ => self.redstone_ore_touch(p, s),
        }
    }

    /// Redstone ore touched (hit, walked on, clicked): lit if it is not (flags 3). Its dust
    /// particles are drawn by the server too, which sends nothing but still draws, on the world
    /// random, two floats per face whose neighbour is not opaque (the two coordinates across
    /// that face).
    pub(crate) fn redstone_ore_touch(&mut self, p: P, s: u32) {
        let b = self.view().block(s);
        if b != self.bs.t.id.redstone_ore && b != self.bs.t.id.deepslate_redstone_ore {
            return;
        }
        for d in super::Dir::ALL {
            if !self.view().info(self.get(super::rel(p, d))).has(rc_data::flags::SOLID_RENDER) {
                let _ = (self.bs.rng.next_float(), self.bs.rng.next_float());
            }
        }
        if !self.view().get_bool(s, "lit") {
            let lit = self.view().set_bool(s, "lit", true);
            self.set_block(p, lit, flag::ALL);
        }
    }
}
