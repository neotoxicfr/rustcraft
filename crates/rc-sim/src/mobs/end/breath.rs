//! Dragon breath (F15): the dragon's fireball and breath clouds, on the common projectile engine
//! (S8). The fireball is an accelerated projectile (`PKind::DragonFireball`, no fire) which, on
//! impact (except on its launcher), places an area-effect cloud (`projectiles/cloud.rs`): radius
//! 3 growing to 7 over 600 ticks, instant damage II at 0.25 duration scale, `dragon_breath`
//! particle. The flame of the sitting dragon is a fixed cloud of radius 5 for 200 ticks (instant
//! damage I).

use rc_proto::nbt::Nbt;

use super::super::mob::Mob;
use super::super::mth::dist2;
use crate::player::Stack;
use crate::projectiles::cloud::Cloud;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

/// Stack carrying the effects of a breath cloud (`potion_contents`: one custom effect).
fn breath_potion(amp: i32, duration: i32) -> Stack {
    let effect = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:instant_damage".into())),
        ("amplifier".into(), Nbt::Byte(amp as i8)),
        ("duration".into(), Nbt::Int(duration)),
    ]);
    let contents = Nbt::Compound(vec![("custom_effects".into(), Nbt::List(vec![effect]))]);
    let mut s = Stack { id: "minecraft:potion".into(), count: 1, components: None };
    crate::items::set_component(&mut s, "minecraft:potion_contents", contents);
    s
}

impl Server {
    /// Dragon breath cloud: radius, duration, radius change, instant damage
    /// (amplifier; duration 1 for the fireball, 0 for the flame), duration scale 0.25;
    /// returns its id.
    pub(crate) fn spawn_breath_cloud(
        &mut self,
        owner: Option<u128>,
        pos: [f64; 3],
        radius: f32,
        duration: i32,
        per_tick: f32,
        amp: i32,
    ) -> i32 {
        let id = self.proj_new_id();
        let mut c = Cloud::new(id, pos);
        c.owner = owner;
        c.custom_particle = Some(("minecraft:dragon_breath".into(), 1.0));
        c.radius = radius.clamp(0.0, 32.0);
        c.duration = duration;
        c.radius_per_tick = per_tick;
        c.duration_scale = 0.25;
        c.potion = Some(breath_potion(amp, if amp == 0 { 0 } else { 1 }));
        self.cloud_add(c);
        id
    }

    /// Removes a cloud (`discard`).
    pub(crate) fn discard_cloud(&mut self, id: i32) {
        self.cloud_discard(id);
    }

    /// Launches a dragon fireball (`DragonFireball(dragon, direction)` then the exact placement:
    /// zero rotation), speed = direction × 0.1.
    pub(crate) fn shoot_dragon_fireball(&mut self, owner: &Mob, pos: [f64; 3], dir: [f64; 3]) {
        let id = self.proj_new_id();
        let mut p =
            Proj::new(PKind::DragonFireball, id, pos, Stack { id: "minecraft:air".into(), count: 1, components: None });
        let n = super::phases::norm(dir);
        p.vel = n.map(|c| c * p.accel);
        p.owner = Some(owner.uuid);
        p.needs_sync = true;
        self.proj_add(p);
    }

    /// Impact: nothing on its launcher; otherwise a cloud of radius 3 that
    /// grows to 7 over 600 ticks, centered on the first living entity (box inflated by
    /// 4/2/4) within 4 blocks; event 2006; then removed.
    pub(crate) fn dragon_fireball_impact(&mut self, p: &mut Proj, hit_owner: bool) {
        if hit_owner {
            return;
        }
        let bb = p.bbox();
        let area = rc_physics::Aabb {
            min: [bb.min[0] - 4.0, bb.min[1] - 2.0, bb.min[2] - 4.0],
            max: [bb.max[0] + 4.0, bb.max[1] + 2.0, bb.max[2] + 4.0],
        };
        let at =
            self.living_in(&area).into_iter().map(|(_, pos)| pos).find(|q| dist2(*q, p.pos) < 16.0).unwrap_or(p.pos);
        // Launcher kept if alive (simulated mob or player who deflected it).
        let owner = self.proj_owner(p).map(|e| match e {
            super::super::mob::Ent::Player => self.uuid,
            super::super::mob::Ent::Mob(id) => self.mob_ref(id).map_or(0, |m| m.uuid),
        });
        self.level_event(2006, p.block_pos(), if p.silent { -1 } else { 1 });
        let per_tick = (7.0f32 - 3.0) / 600.0;
        self.spawn_breath_cloud(owner, at, 3.0, 600, per_tick, 1);
        p.removed = true;
    }
}
