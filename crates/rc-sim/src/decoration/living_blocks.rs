//! Armor stands and mannequins among the blocks, like any living entity of the game: their state
//! as the shared crossed-block pass (`inside_blocks`) reads and writes it (fire, freezing, health,
//! box), the blocks they stand on (magma, redstone ore, turtle eggs, sculk sensors), what slows
//! them down, and their count by the plates and the tripwire. Their state is their saved compound,
//! reached through their registry entry.

use rc_physics::Aabb;
use rc_proto::nbt::Nbt;

use super::{DKind, flag, living_tick::fall_distance, mannequin};
use crate::mobs::jrand::JRand;
use crate::nbtio;
use crate::server::Server;

/// Random of a decoration's own sounds: it keeps none of its own, the sound seeds' source.
pub(crate) fn decoration_sound_rng() -> JRand {
    JRand::new(crate::vitals::sound_seed())
}

/// An effect among the saved active effects of a compound.
fn has_effect(e: &Nbt, id: &str) -> bool {
    nbtio::list(e.get("active_effects")).iter().any(|x| x.get("id").and_then(Nbt::as_str) == Some(id))
}

impl Server {
    pub(crate) fn deco_id_at(&self, i: usize) -> i32 {
        self.deco.live[i].id
    }

    /// Box of living decoration `i`, and whether it falls from above 2.5 blocks (the powder snow
    /// rule of the pass).
    pub(crate) fn deco_box_and_falling_at(&self, i: usize) -> (Aabb, bool) {
        let kind = self.deco.live[i].kind;
        match self.live_nbt(i) {
            Some(e) => (self.deco_box_of(kind, e), fall_distance(e) > 2.5),
            None => (Aabb { min: [0.0; 3], max: [0.0; 3] }, false),
        }
    }

    /// Still in the world, and for a mannequin not dying.
    pub(crate) fn deco_alive_at(&self, i: usize) -> bool {
        let l = &self.deco.live[i];
        !l.removed && (l.kind == DKind::Stand || self.live_nbt(i).is_some_and(|e| mannequin::health(e) > 0.0))
    }

    pub(crate) fn deco_fire_at(&self, i: usize) -> i32 {
        self.live_nbt(i).map_or(0, mannequin::fire_ticks)
    }

    pub(crate) fn deco_set_fire_at(&mut self, i: usize, v: i32) {
        if self.deco_fire_at(i) != v {
            self.deco_edit_at(i, |n| nbtio::set(n, "Fire", Nbt::Short(v as i16)));
        }
    }

    pub(crate) fn deco_frozen_at(&self, i: usize) -> i32 {
        self.live_nbt(i).and_then(|e| nbtio::int(e, "TicksFrozen")).map_or(0, |v| v as i32)
    }

    /// Freezing written like the game saves it (only above 0), and shown to the clients (they
    /// draw a frozen one shaking).
    pub(crate) fn deco_set_frozen_at(&mut self, i: usize, v: i32) {
        if self.deco_frozen_at(i) == v {
            return;
        }
        self.deco_edit_at(i, |n| {
            if v > 0 {
                nbtio::set(n, "TicksFrozen", Nbt::Int(v));
            } else {
                nbtio::remove(n, "TicksFrozen");
            }
        });
        self.deco_data(self.deco.live[i].id, vec![(7, rc_proto_v777::entity::Meta::Int(v))]);
    }

    /// Can freeze: neither kind is freeze-immune, so only a worn `#freeze_immune_wearables` piece
    /// (feet, legs, chest, head, body) protects it.
    pub(crate) fn deco_can_freeze_at(&self, i: usize) -> bool {
        let Some(e) = self.live_nbt(i) else { return false };
        let Some(Nbt::Compound(slots)) = e.get("equipment") else { return true };
        !slots.iter().any(|(slot, s)| {
            matches!(slot.as_str(), "feet" | "legs" | "chest" | "head" | "body")
                && s.get("id").and_then(Nbt::as_str).is_some_and(|id| self.item_tag(id, "minecraft:freeze_immune_wearables"))
        })
    }

    /// Hurt by the world (`ty`, `amount`): an armor stand's rules or a mannequin's damage.
    pub(crate) fn deco_hurt_at(&mut self, i: usize, ty: &str, amount: f32) -> bool {
        let (id, stand) = (self.deco.live[i].id, self.deco.live[i].kind == DKind::Stand);
        self.deco_hurt_by_id(id, stand, ty, amount)
    }

    /// Set on fire for `ticks` (times its burning time, rounded up), kept if longer; its freezing
    /// cleared, like any living entity's.
    pub(crate) fn deco_ignite_at(&mut self, i: usize, ticks: i32) {
        let Some(e) = self.live_nbt(i) else { return };
        let ty = self.deco.live[i].kind.name();
        let k = self.living_attribute(e, ty, "minecraft:burning_time", 1.0);
        let t = (f64::from(ticks) * k).ceil() as i32;
        if self.deco_fire_at(i) < t {
            self.deco_set_fire_at(i, t);
        }
        self.deco_set_frozen_at(i, 0);
    }

    /// Sound of the decoration at its place; `unmuted`: played even when it is silent.
    pub(crate) fn deco_sound_at(&mut self, i: usize, name: &str, volume: f32, pitch: f32, unmuted: bool) {
        let Some(e) = self.live_nbt(i) else { return };
        if !unmuted && flag(e, "Silent") {
            return;
        }
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.world_sound(name, 6, pos, volume, pitch);
    }

    /// Honey block `p` crossed: sliding down its side, it is slowed and forgets its fall (true).
    pub(crate) fn deco_honey_slide(&mut self, i: usize, p: [i32; 3]) -> bool {
        let Some(e) = self.live_nbt(i) else { return false };
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let vel = nbtio::motion(e.get("Motion")).unwrap_or_default();
        let width = (self.deco_half_width_and_height_at(i).0 * 2.0) as f32;
        let Some(v) = rc_physics::honey_slide(pos, vel, flag(e, "OnGround"), width, p) else { return false };
        self.deco_edit_at(i, |n| {
            nbtio::set(n, "Motion", super::dbl(v));
            nbtio::set(n, "fall_distance", Nbt::Double(0.0));
        });
        true
    }

    /// Bubble column cell entered: its vertical speed set by `push`, its fall forgotten when the
    /// column is closed above.
    pub(crate) fn deco_bubble_column_push(&mut self, i: usize, push: impl Fn(f64) -> f64, forget_fall: bool) {
        let Some(e) = self.live_nbt(i) else { return };
        let mut v = nbtio::motion(e.get("Motion")).unwrap_or_default();
        v[1] = push(v[1]);
        self.deco_edit_at(i, |n| {
            nbtio::set(n, "Motion", super::dbl(v));
            if forget_fall {
                nbtio::set(n, "fall_distance", Nbt::Double(0.0));
            }
        });
    }

    /// Stuck in a block (cobweb, berry bush): its next movement scaled by `k`, its fall forgotten.
    pub(crate) fn deco_stick(&mut self, i: usize, k: [f64; 3]) {
        self.deco.live[i].stuck = k;
        if self.live_nbt(i).is_some_and(|e| fall_distance(e) != 0.0) {
            self.deco_edit_at(i, |n| nbtio::set(n, "fall_distance", Nbt::Double(0.0)));
        }
    }

    /// The Weaving effect among its saved effects (cobweb rule).
    pub(crate) fn deco_weaving_at(&self, i: usize) -> bool {
        self.live_nbt(i).is_some_and(|e| has_effect(e, "minecraft:weaving"))
    }

    /// Half width and height of its box.
    pub(crate) fn deco_half_width_and_height_at(&self, i: usize) -> (f64, f64) {
        let (b, _) = self.deco_box_and_falling_at(i);
        ((b.max[0] - b.min[0]) / 2.0, b.max[1] - b.min[1])
    }

    /// On the ground, the block 0.2 under its feet (its main supporting block first): magma burns
    /// a mannequin (`hot_floor`), a slime block holds back its sideways speed, redstone ore lights
    /// up, turtle eggs may crack (one chance in 100, `mob_griefing`), an inactive sculk sensor
    /// feels a step.
    pub(super) fn deco_step_on(&mut self, i: usize) {
        let Some(e) = self.live_nbt(i) else { return };
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        if !self.living_step_may_act(e, pos) {
            return;
        }
        let support = self.deco_box(e).and_then(|b| rc_physics::support_of(&self.world(), &b, pos));
        let p = self.on_pos(pos, support, 0.2);
        let s = self.st(p);
        let b = self.view().block(s);
        let ids = &self.bs.t.id;
        if b == ids.magma {
            self.deco_hurt_at(i, "minecraft:hot_floor", 1.0);
        } else if b == ids.slime {
            self.deco_slime_step(i);
        } else if b == ids.redstone_ore || b == ids.deepslate_redstone_ore {
            self.redstone_ore_touch(p, s);
        } else if b == ids.turtle_egg {
            self.turtle_egg_crush_by_living(p, 100);
        }
        if !self.mobs.rec.vib_blocks.is_empty() {
            self.sensor_entity_on(pos, support, crate::mobs::recent::vibration::Src::Other, false);
        }
    }

    /// Walking on a slime block with a vertical speed under 0.1: its sideways speed × (0.4 +
    /// that speed × 0.2).
    fn deco_slime_step(&mut self, i: usize) {
        let Some(e) = self.live_nbt(i) else { return };
        let mut v = nbtio::motion(e.get("Motion")).unwrap_or_default();
        let vy = v[1].abs();
        // At rest sideways nothing changes (a zero keeps its sign): no edit.
        if vy >= 0.1 || (v[0] == 0.0 && v[2] == 0.0) {
            return;
        }
        let k = 0.4 + vy * 0.2;
        (v[0], v[2]) = (v[0] * k, v[2] * k);
        self.deco_edit_at(i, |n| nbtio::set(n, "Motion", super::dbl(v)));
    }

    /// Armor stands and mannequins whose box `hit` accepts, as the plates and tripwire count
    /// entities (`triggers`: not the markers, which ignore trigger blocks). The kind is read from
    /// the registry entry first, so the other decorations cost no compound lookup.
    pub(crate) fn living_decorations_touching(&self, hit: &impl Fn(&Aabb) -> bool, triggers: bool) -> usize {
        let living = |l: &super::Live| !l.removed && matches!(l.kind, DKind::Stand | DKind::Mannequin);
        (self.deco.live.iter().enumerate())
            .filter(|(_, l)| living(l))
            .filter_map(|(i, l)| Some((l.kind, self.live_nbt(i)?)))
            .filter(|(kind, e)| !(triggers && *kind == DKind::Stand && flag(e, "Marker")))
            .filter(|(kind, e)| hit(&self.deco_box_of(*kind, e)))
            .count()
    }
}
