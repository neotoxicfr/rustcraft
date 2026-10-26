//! Items that aim with their own ray, fluids included (`interactions.md` § 3.5), on the
//! integrated server side: empty bucket (sources, waterlogged blocks, powder snow), water and
//! lava buckets, empty bottle, lily pad and frogspawn. The "except the author" sounds are not
//! sent: the player's client plays them itself. Boats and rafts: entities not simulated
//! in singleplayer (`docs/gaps-client.md`).

use rc_data::flags;
use rc_physics::ray::{FACES, Fluids, RayHit, RayWorld, item_clip};

use crate::blocks::Dir;
use crate::blocks::level::Fluid;
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

#[cfg(test)]
mod tests;

type P = [i32; 3];

/// Server world seen by a ray: block outlines (item ray), or collision shapes
/// if the second field is true.
pub(crate) struct SimRay<'a>(pub &'a Server, pub bool);

impl RayWorld for SimRay<'_> {
    fn outline(&self, p: P) -> &[rc_data::Aabb] {
        let info = self.0.v().info(self.0.get(p));
        self.0.data.shape(if self.1 { info.collision } else { info.outline })
    }

    fn interaction(&self, p: P) -> &[rc_data::Aabb] {
        rc_physics::ray::interaction_shape(&self.0.data, self.0.get(p))
    }

    fn fluid(&self, p: P) -> Option<(bool, f64)> {
        let f = self.0.fluid_at(p);
        if f.is_empty() {
            return None;
        }
        // Same fluid above: full shape, otherwise amount / 9.
        let above = self.0.fluid_at([p[0], p[1] + 1, p[2]]).kind == f.kind;
        Some((f.source, if above { 1.0 } else { f64::from(f32::from(f.amount) / 9.0) }))
    }
}

fn stack(id: &str) -> Stack {
    Stack { id: id.into(), count: 1, components: None }
}

impl Server {
    /// Item ray from the player's eyes, over their block interaction range.
    fn item_ray(&self, mode: Fluids) -> RayHit {
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(crate::commands::ents::E::Player), self.pos[2]];
        let range = self.player_attribute("minecraft:block_interaction_range");
        item_clip(&SimRay(self, false), eye, self.pitch, self.yaw, range, mode)
    }

    /// `use_item` of items with their own ray; true if the item is handled here.
    pub(crate) fn ray_item_use(&mut self, hand: i32) -> bool {
        if self.game_mode == 3 || self.life.vit.dead {
            return false;
        }
        let slot = self.hand_slot(hand);
        let Some(s) = self.slots[slot].clone() else { return false };
        match s.id.as_str() {
            "minecraft:bucket" => self.bucket_fill_use(slot),
            "minecraft:water_bucket" | "minecraft:lava_bucket" => self.bucket_empty_use(slot, &s),
            "minecraft:glass_bottle" => self.bottle_use(slot),
            "minecraft:lily_pad" | "minecraft:frogspawn" => self.on_water_use(slot, &s.id),
            id if id.ends_with("_spawn_egg") => return self.egg_fluid_use(slot, id),
            _ => return false,
        }
        true
    }

    /// Empty bucket: water or lava source, waterlogged block, powder snow.
    fn bucket_fill_use(&mut self, slot: usize) {
        let hit = self.item_ray(Fluids::SourceOnly);
        // Adventure: no block allowed (`can_place_on` component not handled in singleplayer).
        if hit.miss || self.game_mode == 2 {
            return;
        }
        let p = hit.pos;
        let Some(taken) = self.bucket_pickup_at(p) else { return };
        let taken = stack(taken);
        self.award_stat("minecraft:used", "minecraft:bucket", 1);
        self.game_event_block("minecraft:fluid_pickup", p, Some(Ent::Player), None);
        if self.trig_active("minecraft:filled_bucket") {
            self.trigger("minecraft:filled_bucket", &|sv, c| {
                c.get("item").is_none_or(|i| i.is_null() || sv.p_item(i, Some(&taken)))
            });
        }
        self.filled_result(slot, taken, true);
    }

    /// Taking a liquid with the bucket from the block at `p` (water or lava source, bubble column, powder
    /// snow, waterlogged block): block emptied, filled bucket returned; `None` if nothing to take.
    pub(crate) fn bucket_pickup_at(&mut self, p: P) -> Option<&'static str> {
        let s = self.get(p);
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let source = self.v().get_int(s, "level") == 0;
        let wet = self.v().has(s, "waterlogged") && self.v().get_bool(s, "waterlogged");
        Some(match name.as_str() {
            "minecraft:water" | "minecraft:lava" if source => {
                let air = self.air();
                self.set_block(p, air, 11);
                if name == "minecraft:water" { "minecraft:water_bucket" } else { "minecraft:lava_bucket" }
            }
            "minecraft:bubble_column" => {
                let air = self.air();
                self.set_block(p, air, 11);
                "minecraft:water_bucket"
            }
            "minecraft:powder_snow" => {
                let air = self.air();
                self.set_block(p, air, 11);
                self.level_event(2001, p, s as i32);
                "minecraft:powder_snow_bucket"
            }
            _ if wet => {
                let dry = self.v().set_bool(s, "waterlogged", false);
                self.set_update(p, dry);
                if !self.can_place_at(dry, p) {
                    self.destroy_block(p, true, 512);
                }
                "minecraft:water_bucket"
            }
            _ => return None,
        })
    }

    /// Water or lava bucket: in the targeted waterloggable block (water), otherwise in front of the face touched.
    fn bucket_empty_use(&mut self, slot: usize, held: &Stack) {
        let hit = self.item_ray(Fluids::None);
        if hit.miss || self.game_mode == 2 || !self.may_interact(hit.pos) {
            return;
        }
        let water = held.id == "minecraft:water_bucket";
        let d = FACES[usize::from(hit.face.min(5))];
        let next = [hit.pos[0] + d[0], hit.pos[1] + d[1], hit.pos[2] + d[2]];
        let container = water && self.liquid_container(hit.pos);
        let place = if container { hit.pos } else { next };
        if !self.bucket_empty_at(place, water, Some(next), Some(Ent::Player)) {
            return;
        }
        if self.trig_active("minecraft:placed_block") {
            self.trig_location("minecraft:placed_block", place, Some(held));
        }
        self.award_stat("minecraft:used", &held.id, 1);
        let result = if self.game_mode == 1 { held.clone() } else { stack("minecraft:bucket") };
        self.filled_result(slot, result, true);
    }

    /// Waterloggable block that accepts water: the double slab refuses, the
    /// barrier only accepts a player in creative (`by_creative`), not flowing water.
    pub(crate) fn accepts_water(&self, s: u32, by_creative: bool) -> bool {
        let v = self.v();
        if matches!(v.kind(s), crate::blocks::Kind::Slab) && v.get(s, "type") == Some("double") {
            return false;
        }
        if self.data.block_of(s).is_some_and(|b| b.name == "minecraft:barrier") {
            return by_creative;
        }
        true
    }

    /// The block at `p` is a liquid container (waterloggable, seagrass, kelp).
    fn liquid_container(&self, p: P) -> bool {
        self.v().sim(self.get(p)) & rc_data::model::sim_bits::LIQUID_CONTAINER != 0
    }

    /// Emptying at `p`; second attempt in `retry`, ignoring
    /// sneaking. `by`: the player (sound played by their client), none for a
    /// dispenser (sound for everyone).
    pub(crate) fn bucket_empty_at(&mut self, p: P, water: bool, retry: Option<P>, by: Option<Ent>) -> bool {
        let s = self.get(p);
        let info = self.v().info(s);
        let air = self.v().is_air(s);
        let portal = matches!(self.bname(p), "minecraft:end_portal" | "minecraft:end_gateway");
        let may_replace = !portal && (info.has(flags::REPLACEABLE) || !info.has(flags::LEGACY_SOLID));
        // Liquid container (waterloggables, seagrasses, kelps): water enters without breaking anything;
        // already waterlogged or refusal (seagrasses): success with no effect, like the game.
        let container = water && self.liquid_container(p);
        let loggable = container && self.v().has(s, "waterlogged") && self.accepts_water(s, by == Some(Ent::Player) && self.game_mode == 1);
        let inside = air || ((may_replace || loggable) && (!self.sneaking || retry.is_none()));
        if !inside {
            return retry.is_some_and(|q| self.bucket_empty_at(q, water, None, by));
        }
        if water && self.dim.name == "minecraft:the_nether" {
            self.water_evaporates(p, by.is_some());
            return true;
        }
        if container {
            if loggable && !self.v().get_bool(s, "waterlogged") {
                self.place_water_in(p, s); // G6-F01: campfire, dried ghast
            }
        } else {
            if may_replace && !info.has(flags::LIQUID) {
                self.destroy_block(p, true, 512);
            }
            let f = Fluid { kind: if water { 1 } else { 2 }, source: true, amount: 8, falling: false };
            let src = self.legacy_block(f);
            if !self.set_block(p, src, 11) && !self.fluid_of(s).source {
                return false;
            }
        }
        // Emptying sound: for everyone except the author (played by their client); without an author, for everyone.
        if by.is_none() {
            let snd = if water { "minecraft:item.bucket.empty" } else { "minecraft:item.bucket.empty_lava" };
            self.block_sound(p, snd, 1.0, 1.0);
        }
        self.game_event_block("minecraft:fluid_place", p, by, None);
        true
    }

    /// Water that evaporates (dimension that forbids it): extinguish sound (except for the author if there is
    /// one, their client plays it) and smoke for everyone.
    pub(crate) fn water_evaporates(&mut self, p: P, by_player: bool) {
        let pitch = 2.6 + (self.mobs.rng.next_float() - self.mobs.rng.next_float()) * 0.8;
        if !by_player {
            self.block_sound(p, "minecraft:block.fire.extinguish", 0.5, pitch);
        }
        let at = p.map(f64::from);
        self.particles_full("minecraft:large_smoke", 0, at, [1.0; 3], [0.0; 3], 8, 1);
    }

    /// Empty bottle on a targeted water source: water bottle.
    fn bottle_use(&mut self, slot: usize) {
        let hit = self.item_ray(Fluids::SourceOnly);
        if hit.miss || !self.may_interact(hit.pos) || self.fluid_at(hit.pos).kind != 1 {
            return;
        }
        self.game_event_block("minecraft:fluid_pickup", hit.pos, Some(Ent::Player), None);
        self.award_stat("minecraft:used", "minecraft:glass_bottle", 1);
        let water = Stack {
            id: "minecraft:potion".into(),
            count: 1,
            components: Some(rc_proto::nbt::Nbt::Compound(vec![(
                "minecraft:potion_contents".into(),
                rc_proto::nbt::Nbt::Compound(vec![(
                    "potion".into(),
                    rc_proto::nbt::Nbt::String("minecraft:water".into()),
                )]),
            )])),
        };
        self.filled_result(slot, water, true);
    }

    /// Spawn egg aiming at a source (liquid block): mob placed in that block, with no
    /// vertical offset. True if the egg is handled here (simulated species).
    fn egg_fluid_use(&mut self, slot: usize, item: &str) -> bool {
        use crate::mobs::species::Kind;
        let name = format!("minecraft:{}", item.trim_start_matches("minecraft:").trim_end_matches("_spawn_egg"));
        let Some(kind) = Kind::from_name(&name) else { return false };
        let hit = self.item_ray(Fluids::SourceOnly);
        if hit.miss || (kind.info().not_in_peaceful && self.level.difficulty_id() == 0) {
            return true;
        }
        let liquid = self
            .data
            .block_of(self.get(hit.pos))
            .is_some_and(|b| matches!(b.name.as_str(), "minecraft:water" | "minecraft:lava"));
        if !liquid || self.game_mode == 2 || !self.may_interact(hit.pos) {
            return true;
        }
        let p = hit.pos;
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let yaw = crate::mobs::mth::wrap(self.mobs.rng.next_float() * 360.0);
        let mut m = self.new_mob(kind, pos, yaw);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        let mut g = crate::mobs::spawn::Group::default();
        self.finalize_spawn(&mut m, crate::mobs::spawn::Reason::SpawnEgg, &mut g);
        if let Some(a) = kind.info().ambient {
            let a = a.to_owned();
            self.make_sound(&mut m, &a);
        }
        self.add_mob(m);
        if self.game_mode != 1
            && let Some(s) = &mut self.slots[slot]
        {
            s.count -= 1;
            if s.count <= 0 {
                self.slots[slot] = None;
            }
        }
        self.broadcast_menus();
        self.game_event_block("minecraft:entity_place", p, Some(Ent::Player), None);
        self.award_stat("minecraft:used", item, 1);
        true
    }

    /// Lily pad, frogspawn: ordinary placement above the targeted block (sources only),
    /// or above the block where the ray ends if it touches nothing. No adventure mode
    /// check on this path (neither does the game: placement allowed in adventure).
    fn on_water_use(&mut self, slot: usize, item: &str) {
        let hit = self.item_ray(Fluids::SourceOnly);
        let clicked = [hit.pos[0], hit.pos[1] + 1, hit.pos[2]];
        self.place_item_from(slot, item, clicked, Dir::from_index(usize::from(hit.face.min(5))), hit.at, true);
    }
}
