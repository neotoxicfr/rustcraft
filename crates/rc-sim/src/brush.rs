//! G5-F10: brush and suspicious blocks (sand, gravel). The brush on a block starts a use
//! of 200 ticks; every 10 ticks (5th tick of each pass), the targeted block is brushed: dust
//! (draws from the level's random, particles on the client side), block sound, then for a suspicious block
//! a brushing stroke (at most one every 10 ticks). On the first stroke the archaeology
//! loot (`LootTable`, `LootTableSeed`) is drawn; the `dusted` state follows the number of
//! strokes (0, 1-2, 3-5, 6-9); on the 10th stroke the item falls on the brushed face and the block becomes
//! sand or gravel again. With no stroke for 40 ticks, the brushing recedes by 2 strokes every 4
//! ticks. Rules recorded from game 26.3, in our own words.

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::player::Stack;
use crate::server::Server;

/// Duration of a brush use.
pub(crate) const BRUSH_USE_TICKS: i32 = 200;

/// Brushing in progress of a suspicious block (not saved).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Brushing {
    pub count: i32,
    pub cooldown_end: i64,
    pub reset_at: i64,
    /// Brushed face (order down, up, north, south, west, east).
    pub dir: Option<u8>,
}

/// Visible stage of the brushing (`dusted`).
pub(crate) fn completion(count: i32) -> i32 {
    match count {
        0 => 0,
        1..3 => 1,
        3..6 => 2,
        _ => 3,
    }
}

/// Blocks (except air) without brushing dust: no terrain particles or invisible
/// rendering (recorded from game 26.3; no extracted state data carries it).
const NO_DUST: [&str; 9] = [
    "minecraft:barrier",
    "minecraft:light",
    "minecraft:structure_void",
    "minecraft:moving_piston",
    "minecraft:end_portal",
    "minecraft:end_gateway",
    "minecraft:bubble_column",
    "minecraft:water",
    "minecraft:lava",
];

/// Faces in the game's order.
const FACES: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

fn turns_into(name: &str) -> Option<&'static str> {
    match name {
        "minecraft:suspicious_sand" => Some("minecraft:sand"),
        "minecraft:suspicious_gravel" => Some("minecraft:gravel"),
        _ => None,
    }
}

impl Server {
    /// Brush used on a block (slot `slot`): the use starts if the aim touches a block.
    /// Always consumed.
    pub(crate) fn brush_use_on(&mut self, slot: usize) -> bool {
        if self.pov_hit(false).is_some() {
            let hand = if slot == 45 { 1 } else { 0 };
            self.start_using(hand, BRUSH_USE_TICKS);
        }
        true
    }

    /// Brush use tick (`left`: ticks remaining before this tick). With no targeted block,
    /// the use is released.
    pub(crate) fn brush_use_tick(&mut self, hand: i32, left: i32) {
        let Some((p, face)) = self.pov_hit(false) else {
            self.stop_using();
            return;
        };
        let elapsed = BRUSH_USE_TICKS - left + 1;
        if elapsed % 10 != 5 {
            return;
        }
        let state = self.st(p);
        let name = self.state_name(state).to_owned();
        // Dust: client particles, but draws from the level's random on the server side too
        // (number 7..11, then two draws per particle).
        if !self.v().is_air(state) && !NO_DUST.contains(&name.as_str()) {
            let r = &mut self.life.rng;
            let n = 7 + r.next_int_bound(5);
            for _ in 0..n {
                r.next_double();
                r.next_double();
            }
        }
        let sound = match name.as_str() {
            "minecraft:suspicious_sand" => "minecraft:item.brush.brushing.sand",
            "minecraft:suspicious_gravel" => "minecraft:item.brush.brushing.gravel",
            _ => "minecraft:item.brush.brushing.generic",
        };
        // The game's client plays this sound itself for the one brushing; our client does not
        // predict it: also sent to the player.
        self.play_sound(sound, 4, p.map(|v| f64::from(v) + 0.5), 1.0, 1.0);
        if turns_into(&name).is_some() && self.be_get(p).is_some() {
            let slot = self.hand_slot(hand);
            let brush = self.slots[slot].clone();
            if self.brush_block(p, face as u8, brush.as_ref()) {
                self.hurt_item(slot, 1);
            }
        }
    }

    /// A brushing stroke on suspicious block `p`; true if the brushing is complete (the
    /// brush then wears).
    pub(crate) fn brush_block(&mut self, p: [i32; 3], face: u8, brush: Option<&Stack>) -> bool {
        let now = self.time;
        let mut b = self.bs.brushing.get(&p).copied().unwrap_or_default();
        b.dir.get_or_insert(face);
        b.reset_at = now + 40;
        if now < b.cooldown_end {
            self.bs.brushing.insert(p, b);
            return false;
        }
        b.cooldown_end = now + 10;
        self.brush_unpack_loot(p, brush);
        let before = completion(b.count);
        b.count += 1;
        if b.count >= 10 {
            self.bs.brushing.remove(&p);
            self.brushing_completed(p, b.dir);
            return true;
        }
        self.bs.brushing.insert(p, b);
        let s = self.get(p);
        let block = self.v().block(s);
        self.schedule_tick(p, block, 2);
        let after = completion(b.count);
        if before != after {
            let ns = self.v().set(s, "dusted", &after.to_string());
            self.set_block(p, ns, crate::blocks::flag::ALL);
            self.brush_send_update(p);
        }
        false
    }

    /// Archaeology loot drawn at the first stroke: the table (fixed seed, otherwise the table's
    /// sequence, otherwise the level's random) gives at most one item, kept in the block entity.
    fn brush_unpack_loot(&mut self, p: [i32; 3], brush: Option<&Stack>) {
        let Some(be) = self.be_get(p) else { return };
        let Some(table) = be.get("LootTable").and_then(Nbt::as_str).map(str::to_owned) else { return };
        let seed = be.get("LootTableSeed").and_then(Nbt::as_i64).unwrap_or(0);
        self.s9_container_loot(&table); // `player_generates_container_loot`
        let luck = self.luck();
        let first = {
            let tags = &self.block_tags;
            let data = &self.data;
            let tag_fn = |tag: &str, name: &str| {
                let bid = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
                bid.is_some_and(|b| tags.get(tag).is_some_and(|v| v.binary_search(&b).is_ok()))
            };
            let life = &mut *self.life;
            let loot = crate::loot::Loot { pack: &life.pack, items: &life.items, block_tag: &tag_fn };
            let ctx = crate::loot::Ctx { luck, tool: Some(brush), ..Default::default() };
            let seq =
                life.pack.get("loot_table", &table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
            let stacks = if seed != 0 {
                let mut rng = crate::mobs::jrand::JRand::new(seed);
                loot.drops(&table, &ctx, &mut rng)
            } else if let Some(q) = seq {
                loot.drops(&table, &ctx, life.seqs.get(&q))
            } else {
                loot.drops(&table, &ctx, &mut life.rng)
            };
            stacks.into_iter().next()
        };
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::remove(be, "LootTable");
            crate::nbtio::remove(be, "LootTableSeed");
            crate::nbtio::remove(be, "item");
            if let Some(it) = first {
                crate::nbtio::set(be, "item", it.to_nbt(None));
            }
        }
    }

    /// Brushing complete: the item falls on the brushed face (no momentum, a portion of 10 to 30),
    /// event 3008, then the block becomes sand or gravel again.
    fn brushing_completed(&mut self, p: [i32; 3], dir: Option<u8>) {
        let item = self.be_get(p).and_then(|b| b.get("item")).and_then(Stack::from_nbt);
        if let Some(mut it) = item.filter(|s| s.count > 0) {
            let d = FACES[usize::from(dir.unwrap_or(1)).min(5)];
            let drop = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            // Width and height of the item entity: 0.25.
            let (size, height) = (f64::from(0.25f32), f64::from(0.25f32));
            let at = [
                f64::from(drop[0]) + 0.5 * (1.0 - size) + size / 2.0,
                f64::from(drop[1]) + 0.5 + height / 2.0,
                f64::from(drop[2]) + 0.5 * (1.0 - size) + size / 2.0,
            ];
            let r = &mut self.life.rng;
            let n = r.next_int_bound(21) + 10;
            // Velocity drawn by the new entity then reset to zero.
            r.next_double();
            r.next_double();
            it.count = it.count.min(n);
            self.spawn_item(at, [0.0; 3], it, 0);
        }
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::remove(be, "item");
        }
        let s = self.get(p);
        self.level_event(3008, p, s as i32);
        let into = turns_into(self.state_name(s)).unwrap_or("minecraft:air");
        let ns = self.default_state(into);
        self.set_block(p, ns, crate::blocks::flag::ALL);
    }

    /// Tick of the suspicious block: with no stroke for 40 ticks, the brushing recedes by 2 every
    /// 4 ticks (`dusted` state followed); back to 0 it is forgotten, otherwise the tick is rescheduled.
    pub(crate) fn brush_check_reset(&mut self, p: [i32; 3]) {
        let Some(mut b) = self.bs.brushing.get(&p).copied() else { return };
        let now = self.time;
        let s = self.get(p);
        if b.count != 0 && now >= b.reset_at {
            let before = completion(b.count);
            b.count = (b.count - 2).max(0);
            let after = completion(b.count);
            b.reset_at = now + 4;
            if before != after {
                let ns = self.v().set(s, "dusted", &after.to_string());
                self.set_block(p, ns, crate::blocks::flag::ALL);
                self.bs.brushing.insert(p, b);
                self.brush_send_update(p);
            }
        }
        if b.count == 0 {
            self.bs.brushing.remove(&p);
        } else {
            self.bs.brushing.insert(p, b);
            let block = self.v().block(self.get(p));
            self.schedule_tick(p, block, 2);
        }
    }

    /// Block entity data sent to the client: brushed face and item (only these two
    /// fields; the face is never saved).
    fn brush_send_update(&self, p: [i32; 3]) {
        let Some(kind) = self
            .data
            .registry("minecraft:block_entity_type")
            .and_then(|r| r.entries.iter().position(|e| e == "minecraft:brushable_block"))
        else {
            return;
        };
        let mut f = Vec::new();
        if let Some(d) = self.bs.brushing.get(&p).and_then(|b| b.dir) {
            f.push(("hit_direction".into(), Nbt::Byte(d as i8)));
        }
        if let Some(it) = self.be_get(p).and_then(|b| b.get("item")) {
            f.push(("item".into(), it.clone()));
        }
        let _ = self.send(Event::BlockEntityData { pos: p, kind: kind as i32, nbt: Some(Nbt::Compound(f)) });
    }
}

#[cfg(test)]
mod tests {
    /// Visible stages: 0; 1-2; 3-5; 6 and more.
    #[test]
    fn completion_steps() {
        let got: Vec<i32> = (0..10).map(super::completion).collect();
        assert_eq!(got, [0, 1, 1, 2, 2, 2, 3, 3, 3, 3]);
    }
}
