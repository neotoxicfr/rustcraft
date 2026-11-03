//! Loot of a block taken out of the world: mined by the player, by the world (a piston, a fluid,
//! a lost support…) or blown up. The three share one draw: the block's table resolved once per
//! block, its properties and its block entity lent to the context, the table's sequence (or the
//! level's random) taken out for the draw and put back after.
//!
//! `RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT=0` keeps the three former draws (below), which copied the
//! properties, cloned the block entity, wrote the table's name and looked it up twice.

use std::sync::Arc;

use serde_json::Value;

use crate::commands::ents::E;
use crate::loot::{BlockProps, Ctx, Loot};
use crate::player::Stack;
use crate::progress::pred::{Ctx as WorldCtx, Who};
use crate::rng::Rng;
use crate::server::Server;

/// How a block leaves the world, as its loot sees it.
#[derive(Clone, Copy)]
pub(crate) enum BlockRemoval<'a> {
    /// Mined with `tool` (an empty hand: `None`), by the player (the `this` entity of the
    /// conditions) or by the world.
    Mined { tool: Option<&'a Stack>, by_player: bool },
    /// Blown up; `radius` when the explosion's loot decays. No tool and no world are lent.
    Blown { radius: Option<f32> },
}

impl Server {
    /// Loot table of the block of `state`, resolved once per block.
    pub(crate) fn block_loot_table(&self, state: u32) -> Option<Arc<Value>> {
        let block = self.data.state(state)?.block;
        let name = &self.data.blocks.get(usize::from(block))?.name;
        self.life.pack.block_loot_table(u32::from(block), name)
    }

    /// Loot of block `state` at `p`, removed as `how` says. The caller checks the
    /// `block_drops` rule when the removal does (a player's mining draws even without it).
    pub(crate) fn draw_block_loot(&mut self, p: [i32; 3], state: u32, how: BlockRemoval<'_>) -> Vec<Stack> {
        let Some(table) = self.block_loot_table(state) else { return Vec::new() };
        let sequence = table["random_sequence"].as_str();
        let mut rng = match sequence {
            Some(s) => *self.life.seqs.get(s),
            None => self.life.rng,
        };
        let drops = self.draw_block_table(&table, p, state, how, &mut rng);
        match sequence {
            Some(s) => *self.life.seqs.get(s) = rng,
            None => self.life.rng = rng,
        }
        drops
    }

    /// The draw itself, the world only read: the generator was taken out by the caller.
    fn draw_block_table(&self, table: &Value, p: [i32; 3], state: u32, how: BlockRemoval<'_>, rng: &mut Rng) -> Vec<Stack> {
        let block = self.data.block_of(state).map_or("", |b| b.name.as_str());
        let block_entity = self.block_entity(p);
        let tag = |t: &str, n: &str| self.block_name_tagged(t, n);
        let loot = Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag };
        match how {
            BlockRemoval::Blown { radius } => {
                let ctx = Ctx {
                    block: Some((block, BlockProps::State(state))),
                    tool: Some(None),
                    explosion: radius,
                    block_entity,
                    ..Default::default()
                };
                loot.drops_of_table(table, &ctx, rng)
            }
            BlockRemoval::Mined { tool, by_player } => {
                let world = self.loot_world(WorldCtx {
                    this: by_player.then_some(Who::E(E::Player)),
                    origin: Some(p.map(|c| f64::from(c) + 0.5)),
                    block: Some(state),
                    tool: Some(tool),
                    ..Default::default()
                });
                let ctx = Ctx {
                    block: Some((block, BlockProps::State(state))),
                    tool: Some(tool),
                    block_entity,
                    world: Some(&world),
                    ..Default::default()
                };
                loot.drops_of_table(table, &ctx, rng)
            }
        }
    }

    /// Former draw of a block removed by the world (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT=0`).
    pub(crate) fn block_drops_with_owned_context(
        &mut self,
        p: [i32; 3],
        state: u32,
        tool: Option<&Stack>,
    ) -> Vec<Stack> {
        let Some(block) = self.data.block_of(state).map(|b| b.name.clone()) else { return Vec::new() };
        self.mined_block_loot_with_owned_context(p, state, &block, tool, false)
    }

    /// Former draw of a mined block (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT=0`): the player's and the
    /// world's were the same code, `by_player` aside.
    pub(crate) fn mined_block_loot_with_owned_context(
        &mut self,
        p: [i32; 3],
        state: u32,
        block: &str,
        tool: Option<&Stack>,
        by_player: bool,
    ) -> Vec<Stack> {
        let props: Vec<(String, String)> =
            self.data.state_properties(state).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let be = self.block_entity(p).cloned();
        let table_id = format!("minecraft:blocks/{}", block.trim_start_matches("minecraft:"));
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, name: &str| {
            let id = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let seq =
            self.life.pack.get("loot_table", &table_id).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        let mut rng = match &seq {
            Some(s) => *self.life.seqs.get(s),
            None => self.life.rng,
        };
        let drops = {
            let world = self.loot_world(WorldCtx {
                this: by_player.then_some(Who::E(E::Player)),
                origin: Some([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]),
                block: Some(state),
                tool: Some(tool),
                ..Default::default()
            });
            let loot = Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
            let ctx = Ctx {
                block: Some((block, props.into())),
                tool: Some(tool),
                block_entity: be.as_ref(),
                world: Some(&world),
                ..Default::default()
            };
            loot.drops(&table_id, &ctx, &mut rng)
        };
        match &seq {
            Some(s) => *self.life.seqs.get(s) = rng,
            None => self.life.rng = rng,
        }
        drops
    }

    /// Former draw of a blown-up block (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT=0`).
    pub(crate) fn explosion_drops_with_owned_context(
        &mut self,
        p: [i32; 3],
        state: u32,
        radius: Option<f32>,
    ) -> Vec<Stack> {
        let name = self.state_name(state).to_owned();
        let props: Vec<(String, String)> =
            self.data.state_properties(state).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let be = self.block_entity(p).cloned();
        let table = format!("minecraft:blocks/{}", name.trim_start_matches("minecraft:"));
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, n: &str| {
            let id = data.blocks.iter().position(|b| b.name == n).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let loot = Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
        let ctx = Ctx {
            block: Some((name.as_str(), props.into())),
            tool: Some(None),
            explosion: radius,
            block_entity: be.as_ref(),
            ..Default::default()
        };
        let seq =
            self.life.pack.get("loot_table", &table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        match seq {
            Some(s) => {
                let rng = self.life.seqs.get(&s);
                loot.drops(&table, &ctx, rng)
            }
            None => {
                let mut rng = self.life.rng;
                let d = loot.drops(&table, &ctx, &mut rng);
                self.life.rng = rng;
                d
            }
        }
    }
}

#[cfg(test)]
#[path = "block_loot_tests.rs"]
mod tests;
