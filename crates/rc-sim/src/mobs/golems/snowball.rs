//! Snowballs of snow golems (`Snowball(level, mob, stack)`): launched by the common projectile
//! engine (`projectiles/`, S8: gravity 0.03, friction 0.99 / 0.8 in water, zero damage, 3 on a
//! blaze, break particles then disappearance, network tracking, save).

use rc_proto_v777::entity::Meta;

use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

impl Server {
    /// Throws a snowball (normalized direction, triangular inaccuracy
    /// × 0.0172275, power) from `start`; own random of the projectile. `owner`:
    /// UUID of the golem (taken out of the list during its own tick, it cannot be found there).
    pub(crate) fn throw_snowball(&mut self, owner: u128, start: [f64; 3], dir: [f64; 3], pow: f32, unc: f32) {
        let id = self.proj_new_id();
        let one = Stack { id: "minecraft:snowball".into(), count: 1, components: None };
        let mut p = Proj::new(PKind::Snowball, id, start, one.clone());
        p.owner = Some(owner);
        let net = self.net(Some(&one));
        p.set_meta(8, Meta::Item(net));
        p.shoot(dir, pow, unc);
        self.proj_spawn(p, &one);
    }
}
