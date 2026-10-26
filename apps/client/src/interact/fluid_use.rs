//! Items that aim with their own ray, fluids included (`interactions.md` § 3.5): boats
//! and rafts, buckets, empty bottle, lily pad and frogspawn. The game client runs the
//! same code as the server: arm swing, hand bounce, predicted blocks and sounds (the server
//! excludes the acting player from these sounds). The server remains the master of the result.

use rc_data::flags;
use rc_physics::ray::{FACES, Fluids, RayHit, RayWorld, item_clip};

use super::UseRes;
use crate::item_use::ItemView;

/// Family of an item with its own ray.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RayItem {
    Boat,
    /// Lily pad (`ice` accepted below) or frogspawn.
    OnWater {
        lily: bool,
    },
    EmptyBucket,
    /// Water bucket (true) or lava bucket.
    FullBucket {
        water: bool,
    },
    GlassBottle,
    /// Spawn egg: on the client, any block touched (sources included) is enough.
    SpawnEgg,
}

impl RayItem {
    pub(super) fn of(name: &str) -> Option<RayItem> {
        let n = name.strip_prefix("minecraft:").unwrap_or(name);
        Some(match n {
            _ if n.ends_with("_boat") || n.ends_with("_raft") => RayItem::Boat,
            "lily_pad" => RayItem::OnWater { lily: true },
            "frogspawn" => RayItem::OnWater { lily: false },
            "bucket" => RayItem::EmptyBucket,
            "water_bucket" => RayItem::FullBucket { water: true },
            "lava_bucket" => RayItem::FullBucket { water: false },
            "glass_bottle" => RayItem::GlassBottle,
            _ if n.ends_with("_spawn_egg") => RayItem::SpawnEgg,
            _ => return None,
        })
    }
}

/// Fluid of a state: 1 water, 2 lava, 0 none.
fn fluid_kind(data: &rc_data::VersionData, s: u32) -> u8 {
    let name = data.state(s).and_then(|i| data.fluids.get(usize::from(i.fluid))).map_or("", String::as_str);
    match name {
        "minecraft:water" | "minecraft:flowing_water" => 1,
        "minecraft:lava" | "minecraft:flowing_lava" => 2,
        _ => 0,
    }
}

/// Entity that blocks the placement of a boat: vehicles (boats, rafts, minecarts) and every
/// living pushable creature (hence happy ghast, shulker, remote players), except bats,
/// armor stands and mounts already ridden.
fn blocks_boat(e: &crate::entities::Entity) -> bool {
    let k = e.kind.as_str();
    if k.ends_with("boat") || k.ends_with("raft") || k.ends_with("minecart") {
        return true;
    }
    if e.class == crate::entities::Kind::Other || matches!(k, "bat" | "armor_stand") {
        return false;
    }
    let horse_like = matches!(
        k,
        "horse"
            | "donkey"
            | "mule"
            | "skeleton_horse"
            | "zombie_horse"
            | "camel"
            | "camel_husk"
            | "llama"
            | "trader_llama"
    );
    !(horse_like && !e.passengers.is_empty())
}

/// Client world as seen by the items' ray.
struct ClientRay<'a> {
    app: &'a crate::App,
}

impl RayWorld for ClientRay<'_> {
    fn outline(&self, p: [i32; 3]) -> &[rc_data::Aabb] {
        let d = &self.app.data;
        d.state(self.app.act_state(p)).map_or(&[], |i| d.shape(i.outline))
    }

    fn interaction(&self, p: [i32; 3]) -> &[rc_data::Aabb] {
        rc_physics::ray::interaction_shape(&self.app.data, self.app.act_state(p))
    }

    fn fluid(&self, p: [i32; 3]) -> Option<(bool, f64)> {
        let d = &self.app.data;
        let s = self.app.act_state(p);
        let kind = fluid_kind(d, s);
        if kind == 0 {
            return None;
        }
        let info = d.state(s)?;
        // Same fluid above: full shape; otherwise amount / 9 (see `quirks`: the game
        // keeps the first computed height in cache for each state).
        let above = fluid_kind(d, self.app.act_state([p[0], p[1] + 1, p[2]])) == kind;
        let h = if above { 1.0 } else { f64::from(f32::from(info.fluid_amount) / 9.0) };
        Some((info.has(flags::FLUID_SOURCE), h))
    }
}

impl crate::App {
    /// Item ray from the local player's eyes, over its block interaction range.
    fn item_ray(&self, mode: Fluids) -> RayHit {
        let eye = [self.player.pos[0], self.player.pos[1] + self.player.eye_height(), self.player.pos[2]];
        let range = self.player.attribute("minecraft:block_interaction_range").unwrap_or(4.5);
        item_clip(&ClientRay { app: self }, eye, self.pitch, self.yaw, range, mode)
    }

    /// Local use of an item with its own ray (`use_item`), or `None` for another item.
    pub(super) fn ray_item_use(&mut self, hand: usize, name: &str) -> Option<UseRes> {
        let kind = RayItem::of(name)?;
        Some(match kind {
            RayItem::Boat => self.boat_use(),
            RayItem::OnWater { lily } => self.on_water_use(hand, lily),
            RayItem::EmptyBucket => self.bucket_use(hand, None),
            RayItem::FullBucket { water } => self.bucket_use(hand, Some(water)),
            RayItem::GlassBottle => self.bottle_use(),
            // The server checks the liquid block and the difficulty; the client swings the hand.
            RayItem::SpawnEgg if self.item_ray(Fluids::SourceOnly).miss => UseRes::Pass,
            RayItem::SpawnEgg => UseRes::Success { swing: true },
        })
    }

    /// Boat or raft: point touched (fluids included), nobody around the eyes, free room.
    fn boat_use(&mut self) -> UseRes {
        let hit = self.item_ray(Fluids::Any);
        if hit.miss {
            return UseRes::Pass;
        }
        let table = self.renderer.as_ref().and_then(|r| r.entities.as_ref()).map_or(&[][..], |e| e.dims.as_slice());
        let eye = [self.player.pos[0], self.player.pos[1] + self.player.eye_height(), self.player.pos[2]];
        let local = self.entities.local;
        let boxed = |e: &crate::entities::Entity| {
            let s = e.size(table);
            let hw = f64::from(s.w) / 2.0;
            ([e.pos[0] - hw, e.pos[1], e.pos[2] - hw], [e.pos[0] + hw, e.pos[1] + f64::from(s.h), e.pos[2] + hw])
        };
        let vehicle = |k: &str| k.ends_with("boat") || k.ends_with("raft") || k.ends_with("minecart");
        // A targetable entity enclosing the eyes: nothing.
        let wrapped = self.entities.map.iter().any(|(&id, e)| {
            if id == local || !(e.class != crate::entities::Kind::Other || vehicle(&e.kind)) {
                return false;
            }
            let (lo, hi) = boxed(e);
            (0..3).all(|i| eye[i] >= lo[i] && eye[i] < hi[i])
        });
        if wrapped {
            return UseRes::Pass;
        }
        // Boat box (1.375 × 0.5625) at the point touched: blocks and solid entities.
        let (hw, h) = (1.375 / 2.0, 0.5625);
        let lo = [hit.at[0] - hw, hit.at[1], hit.at[2] - hw];
        let hi = [hit.at[0] + hw, hit.at[1] + h, hit.at[2] + hw];
        let overlaps = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| a[i] < hi[i] && b[i] > lo[i]);
        let solid = self.entities.map.iter().any(|(&id, e)| {
            if id == local || !blocks_boat(e) {
                return false;
            }
            let (a, b) = boxed(e);
            overlaps(a, b)
        });
        // The local player counts too (alive, neither spectator nor on a ladder): standing in
        // the water or at the edge, aiming at water less than a block away refuses the placement.
        let me = self.player.bounding_box();
        let me_blocks = !self.hud.dead() && self.env.game_mode != 3 && {
            let colliders = [];
            let world = crate::GridWorld {
                grid: &self.world.grid,
                min_y: self.world.min_y,
                height: self.world.height,
                t: &self.phys,
                border: &self.weather.border,
                colliders: &colliders,
                movers: None,
                noclip: None,
            };
            !self.player.on_climbable(&world, &self.phys)
        };
        if me_blocks && overlaps(me.min, me.max) {
            return UseRes::Fail;
        }
        if solid || self.boxes_collide(lo, hi) {
            return UseRes::Fail;
        }
        UseRes::Success { swing: true }
    }

    /// A block collision shape overlaps the box (strict bounds).
    fn boxes_collide(&self, lo: [f64; 3], hi: [f64; 3]) -> bool {
        let (a, b) = (lo.map(|v| v.floor() as i32 - 1), hi.map(|v| v.floor() as i32 + 1));
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    let Some(info) = self.data.state(self.act_state([x, y, z])) else { continue };
                    let o = [f64::from(x), f64::from(y), f64::from(z)];
                    let hit = self
                        .data
                        .shape(info.collision)
                        .iter()
                        .any(|c| (0..3).all(|i| c.min[i] + o[i] < hi[i] && c.max[i] + o[i] > lo[i]));
                    if hit {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Lily pad, frogspawn: placement above the targeted source (or the ray's arrival block
    /// if it touches nothing), by the ordinary placement of a block item.
    fn on_water_use(&mut self, hand: usize, lily: bool) -> UseRes {
        let hit = self.item_ray(Fluids::SourceOnly);
        let clicked = [hit.pos[0], hit.pos[1] + 1, hit.pos[2]];
        let replaceable = |s: u32| self.data.state(s).is_some_and(|i| i.has(flags::REPLACEABLE));
        let target = if replaceable(self.act_state(clicked)) {
            clicked
        } else {
            let d = FACES[usize::from(hit.face.min(5))];
            [clicked[0] + d[0], clicked[1] + d[1], clicked[2] + d[2]]
        };
        let ly = target[1] - self.world.min_y;
        if !replaceable(self.act_state(target)) || !(0..self.world.height).contains(&ly) {
            return UseRes::Fail;
        }
        // Survival: source water below (or ice for the lily pad), no fluid in place.
        let below = self.act_state([target[0], target[1] - 1, target[2]]);
        let water_source = self
            .data
            .state(below)
            .and_then(|i| self.data.fluids.get(usize::from(i.fluid)))
            .is_some_and(|f| f == "minecraft:water");
        let ice = lily
            && self
                .data
                .block_of(below)
                .is_some_and(|b| matches!(b.name.as_str(), "minecraft:ice" | "minecraft:frosted_ice"));
        if !(water_source || ice) || fluid_kind(&self.data, self.act_state(target)) != 0 {
            return UseRes::Fail;
        }
        // No adventure-mode check on this path (placement allowed, like the game).
        // Predicted ordinary placement: free room (player, entities), block, sound (the server excludes
        // the author from it) and decremented stack.
        let Some(stack) = self.hand_stack(hand).cloned() else { return UseRes::Pass };
        let Some(block) = self.block_of_item(stack.item).cloned() else { return UseRes::Success { swing: true } };
        let state = block.default_state;
        if self.collides_with(target, state) || self.entity_blocks(target, state) {
            return UseRes::Fail;
        }
        self.commit_place(hand, target, state);
        UseRes::Success { swing: true }
    }

    /// Empty bucket (`full = None`, sources only) or full (water / lava, no fluid).
    fn bucket_use(&mut self, hand: usize, full: Option<bool>) -> UseRes {
        let hit = self.item_ray(if full.is_none() { Fluids::SourceOnly } else { Fluids::None });
        if hit.miss {
            return UseRes::Pass;
        }
        let pos = hit.pos;
        let clicked = self.act_state(pos);
        if self.env.game_mode == 2 {
            // Adventure: only on a block allowed by the item.
            let ok = self.hand_stack(hand).cloned().is_some_and(|s| {
                ItemView::new(&s, &self.data, &self.act.ids).can_place_on(&self.data, &self.act.tags, clicked)
            });
            if !ok {
                return UseRes::Fail;
            }
        }
        let d = FACES[usize::from(hit.face.min(5))];
        let next = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
        match full {
            None => self.bucket_pickup(pos, clicked),
            Some(water) => {
                let container = water && self.liquid_container(clicked);
                let place = if container { pos } else { next };
                if self.bucket_empty(place, water, Some(next)) { UseRes::Success { swing: true } } else { UseRes::Fail }
            }
        }
    }

    /// Picking up a source (water, lava), a waterlogged block or powder snow.
    fn bucket_pickup(&mut self, pos: [i32; 3], s: u32) -> UseRes {
        let info = self.data.state(s).copied().unwrap_or_default();
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let (new, sound) = match name.as_str() {
            "minecraft:water" | "minecraft:lava" if info.has(flags::FLUID_SOURCE) => {
                let snd = if name == "minecraft:water" {
                    "minecraft:item.bucket.fill"
                } else {
                    "minecraft:item.bucket.fill_lava"
                };
                (self.air_state(), snd)
            }
            "minecraft:powder_snow" => (self.air_state(), "minecraft:item.bucket.fill_powder_snow"),
            // Bubble column: picked up like a water source (water bucket, water sound).
            "minecraft:bubble_column" => (self.air_state(), "minecraft:item.bucket.fill"),
            _ if self.state_prop(s, "waterlogged").as_deref() == Some("true") => {
                let Some(dry) = self.with_prop(s, "waterlogged", "false") else { return UseRes::Fail };
                (dry, "minecraft:item.bucket.fill")
            }
            _ => return UseRes::Fail,
        };
        self.act_set_block(pos, new);
        let at = self.player.pos;
        if let Some(a) = self.audio.get() {
            a.play(sound, rc_audio::Category::Player, Some(at), 1.0, 1.0, None);
        }
        UseRes::Success { swing: true }
    }

    /// Emptying a bucket at `pos`; second attempt at `retry` (neighbor block of the touched face).
    fn bucket_empty(&mut self, pos: [i32; 3], water: bool, retry: Option<[i32; 3]>) -> bool {
        let s = self.act_state(pos);
        let Some(info) = self.data.state(s).copied() else { return false };
        let air = info.has(flags::AIR);
        let portal = self
            .data
            .block_of(s)
            .is_some_and(|b| matches!(b.name.as_str(), "minecraft:end_portal" | "minecraft:end_gateway"));
        let may_replace = !portal && (info.has(flags::REPLACEABLE) || !info.has(flags::LEGACY_SOLID));
        // Liquid container: water enters it; already waterlogged (or seagrass): success with no
        // effect, like the game.
        let container = water && self.liquid_container(s);
        let loggable = container && self.state_prop(s, "waterlogged").is_some();
        let sneaking = self.sneak_key();
        let inside = air || ((may_replace || loggable) && (!sneaking || retry.is_none()));
        if !inside {
            return retry.is_some_and(|p| self.bucket_empty(p, water, None));
        }
        let c = pos.map(|v| f64::from(v) + 0.5);
        if water && self.dimension.ends_with("the_nether") {
            // Evaporation: sound only (the particles come from the server).
            let pitch = 2.6 + (self.act.rand() as f32 - self.act.rand() as f32) * 0.8;
            if let Some(a) = self.audio.get() {
                a.play("minecraft:block.fire.extinguish", rc_audio::Category::Block, Some(c), 0.5, pitch, None);
            }
            return true;
        }
        if container {
            if self.state_prop(s, "waterlogged").as_deref() == Some("false") {
                let Some(wet) = self.with_prop(s, "waterlogged", "true") else { return false };
                self.act_set_block(pos, wet);
            }
        } else {
            // Source placed (already there: state unchanged, success anyway).
            let block = if water { "minecraft:water" } else { "minecraft:lava" };
            let Some(src) = self.data.blocks.iter().find(|b| b.name == block).map(|b| b.default_state) else {
                return false;
            };
            if s != src {
                self.act_set_block(pos, src);
            }
        }
        let sound = if water { "minecraft:item.bucket.empty" } else { "minecraft:item.bucket.empty_lava" };
        if let Some(a) = self.audio.get() {
            a.play(sound, rc_audio::Category::Block, Some(c), 1.0, 1.0, None);
        }
        true
    }

    /// Empty bottle on a targeted water source: filling sound.
    fn bottle_use(&mut self) -> UseRes {
        let hit = self.item_ray(Fluids::SourceOnly);
        if hit.miss || fluid_kind(&self.data, self.act_state(hit.pos)) != 1 {
            return UseRes::Pass;
        }
        let at = self.player.pos;
        if let Some(a) = self.audio.get() {
            a.play("minecraft:item.bottle.fill", rc_audio::Category::Neutral, Some(at), 1.0, 1.0, None);
        }
        UseRes::Success { swing: true }
    }

    /// The state is that of a liquid container (waterloggable, seagrass, kelp).
    fn liquid_container(&self, s: u32) -> bool {
        self.data.sim.get(s as usize).is_some_and(|b| b & rc_data::model::sim_bits::LIQUID_CONTAINER != 0)
    }

    fn air_state(&self) -> u32 {
        self.data.blocks.iter().find(|b| b.name == "minecraft:air").map_or(0, |b| b.default_state)
    }

    /// Same state with a property changed (contiguous states of a block).
    pub(super) fn with_prop(&self, s: u32, key: &str, value: &str) -> Option<u32> {
        let info = self.data.state(s)?;
        let block = self.data.blocks.get(usize::from(info.block))?;
        let want: Vec<(String, String)> = self
            .data
            .state_properties(s)
            .map(|(k, v)| (k.to_owned(), if k == key { value.to_owned() } else { v.to_owned() }))
            .collect();
        (block.first_state..block.first_state + block.state_count)
            .find(|&t| self.data.state_properties(t).zip(want.iter()).all(|((k, v), (wk, wv))| k == wk && v == wv))
    }
}

#[cfg(test)]
mod tests {
    use super::RayItem;

    #[test]
    fn ray_items() {
        assert_eq!(RayItem::of("minecraft:oak_boat"), Some(RayItem::Boat));
        assert_eq!(RayItem::of("minecraft:bamboo_chest_raft"), Some(RayItem::Boat));
        assert_eq!(RayItem::of("minecraft:lily_pad"), Some(RayItem::OnWater { lily: true }));
        assert_eq!(RayItem::of("minecraft:frogspawn"), Some(RayItem::OnWater { lily: false }));
        assert_eq!(RayItem::of("minecraft:water_bucket"), Some(RayItem::FullBucket { water: true }));
        assert_eq!(RayItem::of("minecraft:milk_bucket"), None);
        assert_eq!(RayItem::of("minecraft:boat"), None);
    }
}
