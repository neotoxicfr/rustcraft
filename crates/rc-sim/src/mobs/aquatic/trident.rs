//! Trident thrown by a drowned: created at the drowned's eyes − 0.1,
//! Loyalty and glint read from the item, aimed shot (power 1.6, inaccuracy by difficulty).
//! Flight, damage (8 + Impaling), bounce, embedding, return by Loyalty, network tracking and save:
//! common projectile engine (`projectiles/`, S8), pickup forbidden (mob thrower).

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::mob::Mob;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

impl Server {
    /// Trident thrown (power 1.6) from `start`.
    pub(crate) fn spawn_trident(&mut self, m: &Mob, start: [f64; 3], dir: [f64; 3], uncertainty: f32, item: Stack) {
        let id = self.proj_new_id();
        let mut thrown = item;
        thrown.count = 1;
        let mut p = Proj::new(PKind::Trident, id, start, thrown.clone());
        p.owner = Some(m.uuid);
        let ctx = crate::projectiles::ench::Ctx { this: Some("minecraft:trident"), ..Default::default() };
        p.loyalty =
            self.ench_value(&thrown, "minecraft:trident_return_acceleration", 0.0, &ctx).clamp(0.0, 127.0) as u8;
        let foil = crate::enchanting::has_any(&self.life.items, &thrown)
            || crate::items::component(&thrown, "minecraft:enchantment_glint_override")
                .and_then(Nbt::as_i64)
                .is_some_and(|v| v != 0);
        p.set_meta(11, Meta::Byte(p.loyalty as i8));
        p.set_meta(12, Meta::Bool(foil));
        p.shoot(dir, 1.6, uncertainty);
        self.proj_spawn(p, &thrown);
    }
}
