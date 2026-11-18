//! Projectiles fired by a dispenser: arrows (pickable),
//! snowballs, eggs, thrown potions, experience bottles, rockets (just outside the block, without
//! angle); fire and wind charges (one block from the centre, then ordinary shot). Power 1.1
//! and inaccuracy 6 by default, potions and bottles ×1.25 / ×0.5. The trident is not part of it:
//! it is thrown like an ordinary item.

use rc_proto_v777::entity::Meta;

use super::{PKind, Pickup, Proj};
use crate::player::Stack;
use crate::server::Server;

/// Is the item fired as a projectile by a dispenser?
pub fn dispensable(id: &str) -> bool {
    matches!(
        id.trim_start_matches("minecraft:"),
        "arrow"
            | "tipped_arrow"
            | "spectral_arrow"
            | "snowball"
            | "egg"
            | "blue_egg"
            | "brown_egg"
            | "splash_potion"
            | "lingering_potion"
            | "experience_bottle"
            | "firework_rocket"
            | "fire_charge"
            | "wind_charge"
    )
}

impl Server {
    /// Shot from the dispenser at `p` facing `dir` (non-unit step); returns the sound
    /// event (1002 throw, 1004 rocket, 1018 fireball, 1051 wind charge).
    pub(crate) fn dispense_projectile(&mut self, p: [i32; 3], dir: [i32; 3], item: &Stack) -> i32 {
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let d = dir.map(f64::from);
        let at = [c[0] + 0.7 * d[0], c[1] + 0.7 * d[1], c[2] + 0.7 * d[2]];
        let mut one = item.clone();
        one.count = 1;
        let short = item.id.trim_start_matches("minecraft:").to_owned();
        if short == "fire_charge" || short == "wind_charge" {
            // One block from the centre; the noisy creation direction (world random) is
            // immediately replaced by the dispenser's ordinary shot (power 1, inaccuracy
            // 6.666…, projectile's random).
            let fire = short == "fire_charge";
            let at = [c[0] + d[0], c[1] + d[1], c[2] + d[2]];
            let r = &mut self.bs.rng;
            for _ in 0..3 {
                let _ = (r.next_double(), r.next_double());
            }
            let id = self.proj_new_id();
            let kind = if fire { PKind::SmallFireball } else { PKind::WindCharge };
            let mut f = Proj::new(kind, id, at, one.clone());
            f.shoot(d, 1.0, 6.666_666_5);
            if fire {
                let net = self.net(Some(&one));
                f.set_meta(8, Meta::Item(net));
            }
            self.proj_add(f);
            return if fire { 1018 } else { 1051 };
        }
        if short == "firework_rocket" {
            let k = 0.500_009_999_999_747_4;
            let pos = [c[0] + d[0] * k, c[1] + d[1] * k, c[2] + d[2] * k];
            let mut f = self.new_firework(pos, one, None, true);
            f.shoot(d, 0.5, 1.0);
            self.proj_add(f);
            return 1004;
        }
        let (kind, pow, unc) = match short.as_str() {
            "arrow" | "tipped_arrow" => (PKind::Arrow, 1.1f32, 6.0f32),
            "spectral_arrow" => (PKind::SpectralArrow, 1.1, 6.0),
            "snowball" => (PKind::Snowball, 1.1, 6.0),
            "egg" | "blue_egg" | "brown_egg" => (PKind::Egg, 1.1, 6.0),
            "splash_potion" => (PKind::SplashPotion, 1.1 * 1.25, 6.0 * 0.5),
            "lingering_potion" => (PKind::LingeringPotion, 1.1 * 1.25, 6.0 * 0.5),
            "experience_bottle" => (PKind::XpBottle, 1.1 * 1.25, 6.0 * 0.5),
            _ => return 1001,
        };
        let id = self.proj_new_id();
        let mut pr = Proj::new(kind, id, at, one.clone());
        if kind.arrow_like() {
            pr.pickup = Pickup::Allowed;
            if kind == PKind::Arrow {
                let color = if crate::items::component(&one, "minecraft:potion_contents").is_some() {
                    super::hits::potion_color(&one)
                } else {
                    -1
                };
                pr.set_meta(11, Meta::Int(color));
            }
        } else {
            let net = self.net(Some(&one));
            pr.set_meta(8, Meta::Item(net));
        }
        // Default position: 0.7 in front of the centre, 0.1 higher; shot along the axis.
        pr.pos[1] += 0.1;
        pr.old_pos = pr.pos;
        pr.shoot(d, pow, unc);
        self.proj_add(pr);
        1002
    }
}
