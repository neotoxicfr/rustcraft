//! Player harvests and trims drawn from a "block interaction" loot table:
//! bush berries (`harvest/sweet_berry_bush`), glow berries of vines
//! (`harvest/cave_vine`), pumpkin carved with shears (`carve/pumpkin`). Each table is
//! drawn with the random sequence it declares, otherwise from the level's random source.

use super::{Dir, P, flag};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

impl Server {
    /// Interaction table drawn for block `p` (state `s`), tool if any.
    fn block_interact_loot(&mut self, table: &str, p: P, s: u32, tool: Option<&Stack>) -> Vec<Stack> {
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let props: Vec<(String, String)> =
            self.data.state_properties(s).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let be = self.block_entity(p).cloned();
        let ctx = crate::loot::Ctx {
            block: Some((name.as_str(), props)),
            tool: Some(tool),
            block_entity: be.as_ref(),
            ..Default::default()
        };
        crate::mobs::surface::loot(self, table, &ctx)
    }

    /// Picking with no useful effect (berry bush of age > 1, vine carrying berries); true if
    /// the action is consumed. Bone meal on a bush not yet ripe falls through to the item.
    pub(crate) fn harvest_use(&mut self, p: P, s: u32) -> bool {
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let prop = |k: &str| self.data.state_properties(s).find(|(n, _)| *n == k).map(|(_, v)| v.to_owned());
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        match name.as_str() {
            "minecraft:sweet_berry_bush" => {
                let age: i32 = prop("age").and_then(|v| v.parse().ok()).unwrap_or(0);
                let held = self.slots[36 + self.held as usize].as_ref().map(|h| h.id.as_str());
                if age < 3 && held == Some("minecraft:bone_meal") {
                    return false;
                }
                if age <= 1 {
                    return false;
                }
                for d in self.block_interact_loot("minecraft:harvest/sweet_berry_bush", p, s, None) {
                    self.pop_resource(p, d);
                }
                let pitch = 0.8 + self.mobs.rng.next_float() * 0.4;
                self.world_sound("minecraft:block.sweet_berry_bush.pick_berries", 4, at, 1.0, pitch);
                if let Some(ns) = self.with_prop(s, "age", "1") {
                    self.set_block(p, ns, flag::CLIENTS);
                    self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(ns));
                }
                true
            }
            "minecraft:cave_vines" | "minecraft:cave_vines_plant" => {
                if prop("berries").as_deref() != Some("true") {
                    return false;
                }
                for d in self.block_interact_loot("minecraft:harvest/cave_vine", p, s, None) {
                    self.pop_resource(p, d);
                }
                let pitch = self.mobs.rng.next_float() * 0.4 + 0.8;
                self.world_sound("minecraft:block.cave_vines.pick_berries", 4, at, 1.0, pitch);
                if let Some(ns) = self.with_prop(s, "berries", "false") {
                    self.set_block(p, ns, flag::CLIENTS);
                    self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(ns));
                }
                true
            }
            _ => false,
        }
    }

    /// Shears on a pumpkin: seeds dropped in front of the carved face (table
    /// `carve/pumpkin`), carved pumpkin turned toward that face (top or bottom face:
    /// toward the player), shears damaged; true if the action is consumed.
    pub(crate) fn carve_pumpkin(&mut self, p: P, s: u32, face: Dir) -> bool {
        let slot = 36 + self.held as usize;
        let Some(tool) = self.slots[slot].clone() else { return false };
        if tool.id != "minecraft:shears" || self.data.block_of(s).is_none_or(|b| b.name != "minecraft:pumpkin") {
            return false;
        }
        let dir = if face.axis() == 1 { self.player_facing().opposite() } else { face };
        let step = dir.step();
        for d in self.block_interact_loot("minecraft:carve/pumpkin", p, s, Some(&tool)) {
            let pos = [
                f64::from(p[0]) + 0.5 + f64::from(step[0]) * 0.65,
                f64::from(p[1]) + 0.1,
                f64::from(p[2]) + 0.5 + f64::from(step[2]) * 0.65,
            ];
            // Velocity pushed toward the face, drawn on the level's random (the item's starting one,
            // on its own random, is replaced).
            let r = &mut self.mobs.rng;
            let v = [
                0.05 * f64::from(step[0]) + r.next_double() * 0.02,
                0.05,
                0.05 * f64::from(step[2]) + r.next_double() * 0.02,
            ];
            self.spawn_item(pos, v, d, 0);
        }
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.world_sound("minecraft:block.pumpkin.carve", 4, at, 1.0, 1.0);
        let carved = self.default_state("minecraft:carved_pumpkin");
        let carved = self.with_prop(carved, "facing", dir.name()).unwrap_or(carved);
        self.set_block(p, carved, flag::ALL);
        self.hurt_item(slot, 1);
        self.game_event_block("minecraft:shear", p, Some(Ent::Player), None);
        self.award_stat("minecraft:used", "minecraft:shears", 1);
        true
    }
}
