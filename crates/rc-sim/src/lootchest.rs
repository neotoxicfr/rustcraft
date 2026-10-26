//! Loot containers (S7, `mechanics-s7.md` § 5): chests, trapped chests, barrels,
//! dispensers, hoppers, shulker boxes carrying `LootTable` (+ `LootTableSeed`) are
//! filled on first opening or on break. Non-zero seed: linear congruential generator
//! seeded by it; zero: random sequence named by the table. The game's fill: free slots
//! shuffled, stacks split while room remains, list shuffled, each stack put
//! in the last free slot of the list.

use rc_proto::nbt::Nbt;

use crate::containers::{read_items, write_items};
use crate::player::Stack;
use crate::randsrc::{Rand, shuffle_dyn};
use crate::server::Server;

/// Size of a loot container according to its block entity.
pub fn loot_size(id: &str) -> Option<usize> {
    let short = id.trim_start_matches("minecraft:");
    Some(match short {
        "chest" | "trapped_chest" | "barrel" | "shulker_box" => 27,
        "dispenser" | "dropper" | "crafter" => 9,
        "hopper" => 5,
        _ => return None,
    })
}

/// Splitting of stacks then shuffle.
pub fn shuffle_and_split(result: &mut Vec<Stack>, available: usize, rng: &mut dyn Rand) {
    let mut split: Vec<Stack> = Vec::new();
    let mut kept = Vec::with_capacity(result.len());
    for s in result.drain(..) {
        if s.count <= 0 {
            continue;
        }
        if s.count > 1 {
            split.push(s);
        } else {
            kept.push(s);
        }
    }
    *result = kept;
    while available as i64 - result.len() as i64 - split.len() as i64 > 0 && !split.is_empty() {
        let k = rng.between(0, split.len() as i32 - 1) as usize;
        let mut s = split.remove(k);
        let remove = rng.between(1, s.count / 2);
        let copy = Stack { count: remove, ..s.clone() };
        s.count -= remove;
        if s.count > 1 && rng.next_bool() {
            split.push(s);
        } else {
            result.push(s);
        }
        if copy.count > 1 && rng.next_bool() {
            split.push(copy);
        } else {
            result.push(copy);
        }
    }
    result.append(&mut split);
    shuffle_dyn(rng, result);
}

/// Filling (`fill`): generated items put into free slots drawn at random.
pub fn fill(items: &mut [Option<Stack>], mut stacks: Vec<Stack>, rng: &mut dyn Rand) {
    let mut slots: Vec<usize> = (0..items.len()).filter(|&i| items[i].is_none()).collect();
    shuffle_dyn(rng, &mut slots);
    shuffle_and_split(&mut stacks, slots.len(), rng);
    for s in stacks {
        let Some(i) = slots.pop() else {
            eprintln!("singleplayer: container overfilled by its loot");
            return;
        };
        items[i] = (s.count > 0).then_some(s);
    }
}

impl Server {
    /// Loot container opened (`by_player`: player's luck) or broken: table drawn, contents
    /// written, table forgotten. Returns false if there was no table.
    pub(crate) fn unpack_loot(&mut self, p: [i32; 3], by_player: bool) -> bool {
        let Some(be) = self.block_entity(p) else { return false };
        // Identifier read like the game (`chests/x` → `minecraft:chests/x`).
        let Some(table) = be.get("LootTable").and_then(Nbt::as_str).map(crate::commands::args::id) else {
            return false;
        };
        let seed = be.get("LootTableSeed").and_then(Nbt::as_i64).unwrap_or(0);
        let id = be.get("id").and_then(Nbt::as_str).unwrap_or("").to_owned();
        // Decorated pot: a single slot, kept under the key `item` (no `Items` list).
        let pot = id == "minecraft:decorated_pot";
        let size = if pot { 1 } else { loot_size(&id).unwrap_or(27) };
        let mut items = if pot { vec![be.get("item").and_then(Stack::from_nbt)] } else { read_items(be, size) };
        self.fill_loot(&table, seed, by_player, &mut items, p);
        if let Some(be) = self.block_entity_mut(p, &id) {
            crate::nbtio::remove(be, "LootTable");
            crate::nbtio::remove(be, "LootTableSeed");
            if pot {
                match items.first().cloned().flatten() {
                    Some(s) => crate::nbtio::set(be, "item", s.to_nbt(None)),
                    None => crate::nbtio::remove(be, "item"),
                }
            } else {
                write_items(be, &items);
            }
        }
        if crate::life::s2log() {
            eprintln!("singleplayer: loot {table} (seed {seed}) at {p:?}");
        }
        if by_player {
            self.s9_container_loot(&table); // S9
        }
        true
    }

    /// Draw of a loot table into `items`: fixed seed, otherwise the table's sequence,
    /// otherwise the level's random; exploration maps resolved from `origin`.
    pub(crate) fn fill_loot(
        &mut self,
        table: &str,
        seed: i64,
        by_player: bool,
        items: &mut Vec<Option<Stack>>,
        origin: [i32; 3],
    ) {
        let p = origin;
        let luck = if by_player { self.luck() } else { 0.0 };
        let generated = {
            let mut items = std::mem::take(items);
            let tags = &self.block_tags;
            let data = &self.data;
            let tag_fn = |tag: &str, name: &str| {
                let bid = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
                bid.is_some_and(|b| tags.get(tag).is_some_and(|v| v.binary_search(&b).is_ok()))
            };
            let life = &mut *self.life;
            let loot = crate::loot::Loot { pack: &life.pack, items: &life.items, block_tag: &tag_fn };
            let ctx = crate::loot::Ctx { luck, ..Default::default() };
            // Fixed seed, otherwise sequence declared by the table (`random_sequence`), otherwise the level's
            // random (table without sequence or nonexistent: nothing is created in the sequences).
            let seq =
                life.pack.get("loot_table", table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
            if seed != 0 {
                let mut rng = crate::mobs::jrand::JRand::new(seed);
                let stacks = loot.drops(table, &ctx, &mut rng);
                fill(&mut items, stacks, &mut rng);
            } else if let Some(q) = seq {
                let rng = life.seqs.get(&q);
                let stacks = loot.drops(table, &ctx, rng);
                fill(&mut items, stacks, rng);
            } else {
                let rng = &mut life.rng;
                let stacks = loot.drops(table, &ctx, rng);
                fill(&mut items, stacks, rng);
            }
            items
        };
        // S8: exploration maps in the loot (structure searched from the container).
        let mut generated = generated;
        for it in generated.iter_mut() {
            if let Some(s) = it {
                self.resolve_exploration(s, p);
                if s.count <= 0 {
                    *it = None;
                }
            }
        }
        *items = generated;
    }

    /// Before opening: loot drawn; a spectator cannot open a still
    /// sealed container (true: opening refused).
    pub(crate) fn loot_before_open(&mut self, positions: &[[i32; 3]]) -> bool {
        let sealed = positions.iter().any(|p| self.block_entity(*p).is_some_and(|b| b.get("LootTable").is_some()));
        if sealed && self.game_mode == 3 {
            // Red message above the hotbar.
            let mut msg = rc_text::Component::translate("container.spectatorCantOpen", Vec::new());
            msg.style.color = Some(0xFF5555);
            let _ = self.send(rc_client::Event::ActionBar(msg));
            return true;
        }
        for p in positions {
            self.unpack_loot(*p, true);
        }
        false
    }
}
