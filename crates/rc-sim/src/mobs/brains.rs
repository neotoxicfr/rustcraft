//! Species-specific behaviors (`server-mobs.md` § 4): goals registered in
//! the game's order, creeper fuse, enderman teleports, laying, growth, love,
//! breeding, player interactions (feeding, taming, shearing, milking, sitting).

use super::goals::surface_goals::{Mtb, SGoal};
use super::goals::{Food, Goal, Melee, TBase, Who};
use super::illagers::goals::IGoal;
use super::mob::{Ent, Mob};
use super::mth::dist2;
use super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

fn stroll(speed: f64) -> Goal {
    Goal::Stroll { speed, interval: 120, prob: Some(0.001), wanted: [0.0; 3] }
}

fn look_player(dist: f32) -> Goal {
    Goal::LookAtPlayer { dist, prob: 0.02, at: None, time: 0 }
}

fn look_around() -> Goal {
    Goal::LookAround { rel: [0.0; 2], time: 0 }
}

fn panic(speed: f64) -> Goal {
    Goal::Panic { speed, pos: [0.0; 3], tag: "minecraft:panic_causes" }
}

fn breed(speed: f64) -> Goal {
    Goal::Breed { speed, partner: None, love_time: 0 }
}

fn tempt(speed: f64, food: Food, scare: bool) -> Goal {
    Goal::Tempt { speed, food, scare, stop: 2.5, calm: 0, p: [0.0; 3], prot: [0.0; 2], on: false }
}

fn follow_parent(speed: f64) -> Goal {
    Goal::FollowParent { speed, parent: None, recalc: 0 }
}

fn melee(speed: f64, unseen: bool, kind: Melee) -> Goal {
    Goal::Melee { speed, unseen, kind, path: None, pathed: [0.0; 3], recalc: 0, cd: 0, last: i64::MIN / 2 }
}

fn hurt_by(alert: bool) -> Goal {
    Goal::HurtBy { base: TBase::new(true, false), alert, stamp: 0 }
}

/// Targeting of prey while the beast is not tamed (no sight required).
fn non_tame_target(who: Who) -> Goal {
    Goal::Nearest {
        base: TBase::new(false, false),
        who,
        interval: super::mth::reduced(10),
        target: None,
        angry_only: false,
        non_tame: true,
    }
}

fn nearest(who: Who, must_see: bool) -> Goal {
    Goal::Nearest {
        base: TBase::new(must_see, false),
        who,
        interval: super::mth::reduced(10),
        target: None,
        angry_only: false,
        non_tame: false,
    }
}

/// Ability to break down doors of the zombie family: on a
/// ground navigation, it also sets the opening of doors by navigation; the breaking-down
/// goal (priority 1, appended at the end of the list) only acts while it is true.
pub fn set_can_break_doors(m: &mut Mob, on: bool) {
    let ground = !(m.nav.fly || m.nav.swim);
    if ground {
        if m.can_break_doors != on {
            m.can_break_doors = on;
            m.nav.can_open_doors = on;
        }
    } else {
        m.can_break_doors = false;
    }
}

/// Goals of each species, in the game's order of addition (goal and target selectors);
/// the zombie family keeps the door-breaking goal last (active only with
/// the ability, like the goal added then removed by the game).
pub fn register_goals(s: &Server, m: &mut Mob) {
    register_species_goals(s, m);
    if m.kind.zombie_like() {
        m.goals.add(1, Goal::Ill(IGoal::BreakDoor { door: None, time: 0, last: -1, zombie: true }));
    }
}

fn register_species_goals(_s: &Server, m: &mut Mob) {
    if super::variants::register_goals(m) {
        return;
    }
    if super::surface::handles(m.kind) {
        super::surface::register_goals(_s, m);
        return;
    }
    if super::mounts::handles(m.kind) {
        super::mounts::goals::register(m);
        return;
    }
    if m.kind.is_cube() {
        super::cubes::register_goals(m);
        return;
    }
    // F10: animals with a brain (the tadpole keeps the fish's goals).
    if super::brainy::handles(m.kind) {
        super::brainy::register_goals(m);
        return;
    }
    // F09: aquatic fauna (nautiluses only have a brain).
    if super::sealife::handles(m.kind) {
        super::sealife::register_goals(m);
        return;
    }
    if super::golems::handles(m.kind) {
        super::golems::register_goals(m);
        return;
    }
    if super::illagers::handles(m.kind) {
        super::illagers::register_goals(m);
        return;
    }
    if super::aquatic::handles(m.kind) {
        super::aquatic::register_goals(m);
        return;
    }
    // F11: bee, fox, panda.
    if super::wild::handles(m.kind) {
        super::wild::register_goals(m);
        return;
    }
    // F12: blaze, ghast, strider (the wither skeleton takes the skeleton's goals).
    // F15: shulker (the dragon has no goal).
    if super::end::handles(m.kind) {
        super::end::register_goals(m);
        return;
    }
    if super::nether::handles(m.kind) && m.kind != Kind::WitherSkeleton {
        super::nether::register_goals(m);
        return;
    }
    // F13: piglin, brute, hoglin and zoglin only have a brain.
    if super::piglins::has_brain(m.kind) {
        return;
    }
    // F14: brains (only the adult happy ghast has goals).
    if super::recent::handles(m.kind) {
        super::recent::register_goals(m);
        return;
    }
    // F16: wither.
    if m.kind == Kind::Wither {
        super::wither::register_goals(m);
        return;
    }
    let g = &mut m.goals;
    let t = &mut m.targets;
    // Variants: goals of the base species (husk and zombie villager, stray / bogged / parched,
    // cave spider).
    match m.kind.base() {
        Kind::Zombie => {
            g.add(4, Goal::Surf(SGoal::move_to_block(Mtb::RemoveEgg, 1.0, 24, 3, 0)));
            g.add(8, look_player(8.0));
            g.add(8, look_around());
            // Spear (zombie, husk, zombie villager armed with a spear), then melee, then
            // nighttime village crossing (doors according to the door-breaking ability).
            g.add(2, Goal::Spear(super::piglins::spear::SpearGoal::new()));
            g.add(3, melee(1.0, false, Melee::Zombie { raise: 0 }));
            g.add(6, Goal::Ill(IGoal::village_walk()));
            g.add(7, stroll(1.0));
            t.add(1, hurt_by(true));
            t.add(2, nearest(Who::Player, true));
            t.add(3, nearest(Who::Villager, false));
            // F06: iron golem (same priority, sight required).
            t.add(3, nearest(Who::IronGolem, true));
            t.add(5, nearest(Who::BabyTurtleOnLand, true));
        }
        // F13: zombified piglin (from the zombie, then its own goals).
        Kind::ZombifiedPiglin => {
            g.add(4, Goal::Surf(SGoal::move_to_block(Mtb::RemoveEgg, 1.0, 24, 3, 0)));
            g.add(8, look_player(8.0));
            g.add(8, look_around());
            g.add(1, Goal::Spear(super::piglins::spear::SpearGoal::new()));
            g.add(2, melee(1.0, false, Melee::Zombie { raise: 0 }));
            g.add(7, stroll(1.0));
            t.add(1, hurt_by(true));
            t.add(
                2,
                Goal::Nearest {
                    base: TBase::new(true, false),
                    who: Who::Player,
                    interval: super::mth::reduced(10),
                    target: None,
                    angry_only: true,
                    non_tame: false,
                },
            );
            t.add(3, Goal::ResetAnger { stamp: 0, alert: true });
        }
        Kind::Skeleton => {
            // F12: the wither skeleton targets piglins first (added before the base).
            if m.kind == Kind::WitherSkeleton {
                t.add(3, super::surface::nearest(Who::Piglin, true, 10));
            }
            g.add(2, Goal::RestrictSun);
            g.add(3, Goal::FleeSun { speed: 1.0, wanted: [0.0; 3] });
            g.add(3, Goal::Avoid { who: Who::Wolf, max: 6.0, walk: 1.0, sprint: 1.2, from: None, path: None });
            g.add(5, stroll(1.0));
            g.add(6, look_player(8.0));
            g.add(6, look_around());
            t.add(1, hurt_by(false));
            t.add(2, nearest(Who::Player, true));
            t.add(3, nearest(Who::IronGolem, true));
            t.add(3, nearest(Who::BabyTurtleOnLand, true));
        }
        Kind::Creeper => {
            g.add(1, Goal::Float);
            g.add(2, Goal::Swell { target: None });
            g.add(
                3,
                Goal::Avoid { who: Who::Exact(Kind::Ocelot), max: 6.0, walk: 1.0, sprint: 1.2, from: None, path: None },
            );
            g.add(3, Goal::Avoid { who: Who::Cat, max: 6.0, walk: 1.0, sprint: 1.2, from: None, path: None });
            g.add(4, melee(1.0, false, Melee::Plain));
            g.add(5, stroll(0.8));
            g.add(6, look_player(8.0));
            g.add(6, look_around());
            t.add(1, nearest(Who::Player, true));
            t.add(2, hurt_by(false));
        }
        Kind::Spider => {
            g.add(1, Goal::Float);
            // Flee from an armadillo that is not rolled into a ball.
            g.add(
                2,
                Goal::Avoid {
                    who: Who::Exact(Kind::Armadillo),
                    max: 6.0,
                    walk: 1.0,
                    sprint: 1.2,
                    from: None,
                    path: None,
                },
            );
            g.add(3, Goal::Leap { yd: 0.4, target: None });
            g.add(4, melee(1.0, true, Melee::Spider));
            g.add(5, stroll(0.8));
            g.add(6, look_player(8.0));
            g.add(6, look_around());
            t.add(1, hurt_by(false));
            t.add(2, nearest(Who::Player, true));
            t.add(3, nearest(Who::IronGolem, true));
        }
        Kind::Enderman => {
            g.add(0, Goal::Float);
            g.add(1, Goal::EndermanFreeze { target: None });
            g.add(2, melee(1.0, false, Melee::Plain));
            g.add(7, Goal::Stroll { speed: 1.0, interval: 120, prob: Some(0.0), wanted: [0.0; 3] });
            g.add(8, look_player(8.0));
            g.add(8, look_around());
            g.add(10, Goal::EndermanLeave);
            g.add(11, Goal::EndermanTake);
            t.add(1, Goal::EndermanLook { base: TBase::new(false, false), pending: None, aggro: 0, teleport: 0 });
            t.add(2, hurt_by(false));
            t.add(3, nearest(Who::Endermite, true));
            t.add(4, Goal::ResetAnger { stamp: 0, alert: false });
        }
        Kind::Pig => {
            g.add(0, Goal::Float);
            g.add(1, panic(1.25));
            g.add(3, breed(1.0));
            g.add(4, tempt(1.2, Food::Item("minecraft:carrot_on_a_stick"), false));
            g.add(4, tempt(1.2, Food::Tag("minecraft:pig_food"), false));
            g.add(5, follow_parent(1.1));
            g.add(6, stroll(1.0));
            g.add(7, look_player(6.0));
            g.add(8, look_around());
        }
        Kind::Cow => {
            g.add(0, Goal::Float);
            g.add(1, panic(2.0));
            g.add(2, breed(1.0));
            g.add(3, tempt(1.25, Food::Tag("minecraft:cow_food"), false));
            g.add(4, follow_parent(1.25));
            g.add(5, stroll(1.0));
            g.add(6, look_player(6.0));
            g.add(7, look_around());
        }
        Kind::Sheep => {
            g.add(0, Goal::Float);
            g.add(1, panic(1.25));
            g.add(2, breed(1.0));
            g.add(3, tempt(1.1, Food::Tag("minecraft:sheep_food"), false));
            g.add(4, follow_parent(1.1));
            g.add(5, Goal::EatBlock { tick: 0 });
            g.add(6, stroll(1.0));
            g.add(7, look_player(6.0));
            g.add(8, look_around());
        }
        Kind::Chicken => {
            g.add(0, Goal::Float);
            g.add(1, panic(1.4));
            g.add(2, breed(1.0));
            g.add(3, tempt(1.0, Food::Tag("minecraft:chicken_food"), false));
            g.add(4, follow_parent(1.1));
            g.add(5, stroll(1.0));
            g.add(6, look_player(6.0));
            g.add(7, look_around());
        }
        Kind::Wolf => {
            g.add(1, Goal::Float);
            g.add(1, Goal::Panic { speed: 1.5, pos: [0.0; 3], tag: "minecraft:panic_environmental_causes" });
            g.add(2, Goal::SitOrdered);
            g.add(
                3,
                Goal::Avoid { who: Who::Exact(Kind::Llama), max: 24.0, walk: 1.5, sprint: 1.5, from: None, path: None },
            );
            g.add(4, Goal::Leap { yd: 0.4, target: None });
            g.add(5, melee(1.0, true, Melee::Plain));
            g.add(6, Goal::FollowOwner { speed: 1.0, start: 10.0, stop: 2.0, recalc: 0, old_water: 0.0 });
            g.add(7, breed(1.0));
            g.add(8, stroll(1.0));
            g.add(9, Goal::Beg { dist: 8.0, time: 0 });
            g.add(10, look_player(8.0));
            g.add(10, look_around());
            t.add(1, Goal::OwnerHurtBy { base: TBase::new(false, false), stamp: 0, by: None });
            t.add(2, Goal::OwnerHurt { base: TBase::new(false, false), stamp: 0, hit: None });
            t.add(3, hurt_by(true));
            t.add(
                4,
                Goal::Nearest {
                    base: TBase::new(true, false),
                    who: Who::Player,
                    interval: super::mth::reduced(10),
                    target: None,
                    angry_only: true,
                    non_tame: false,
                },
            );
            t.add(
                5,
                Goal::Nearest {
                    base: TBase::new(false, false),
                    who: Who::WolfPrey,
                    interval: super::mth::reduced(10),
                    target: None,
                    angry_only: false,
                    non_tame: true,
                },
            );
            t.add(6, non_tame_target(Who::BabyTurtleOnLand));
            t.add(7, nearest(Who::Skeleton, false));
            t.add(8, Goal::ResetAnger { stamp: 0, alert: true });
        }
        Kind::Cat => {
            g.add(1, Goal::Float);
            g.add(1, Goal::Panic { speed: 1.5, pos: [0.0; 3], tag: "minecraft:panic_causes" });
            g.add(2, Goal::SitOrdered);
            g.add(3, Goal::CatRelax { goal_pos: None, on_bed: 0 });
            g.add(4, tempt(0.6, Food::Tag("minecraft:cat_food"), true));
            g.add(5, Goal::Surf(SGoal::move_to_block(Mtb::CatLie, 1.1, 8, 6, -2)));
            g.add(6, Goal::FollowOwner { speed: 1.0, start: 10.0, stop: 5.0, recalc: 0, old_water: 0.0 });
            g.add(7, Goal::Surf(SGoal::move_to_block(Mtb::CatSit, 0.8, 8, 1, 0)));
            g.add(8, Goal::Leap { yd: 0.3, target: None });
            g.add(9, Goal::OcelotAttack { target: None, time: 0 });
            g.add(10, breed(0.8));
            g.add(11, Goal::Stroll { speed: 0.8, interval: 120, prob: Some(1.000_000_1e-5), wanted: [0.0; 3] });
            g.add(12, look_player(10.0));
            // Flee from players (not tamed), added afterwards as in the game.
            g.add(4, Goal::Avoid { who: Who::Player, max: 16.0, walk: 0.8, sprint: 1.33, from: None, path: None });
            t.add(1, non_tame_target(Who::Rabbit));
            t.add(1, non_tame_target(Who::BabyTurtleOnLand));
        }
        Kind::Rabbit | Kind::Mooshroom | Kind::Bat | Kind::Ocelot | Kind::PolarBear | Kind::Turtle | Kind::Parrot => {}
        // F05: villager brain (`villager/brain.rs`), wandering trader goals.
        Kind::Villager => {}
        Kind::WanderingTrader => super::villager::trader::register_goals(g),
        // Species without a common base: goals set by `variants::register_goals`, `cubes`, `surface`…
        _ => {}
    }
}

/// Skeleton: bow (interval according to difficulty) or melee, appended at the end of the list.
pub fn reassess_weapon(s: &Server, m: &mut Mob) {
    m.goals.goals.retain(|w| !matches!(w.goal, Goal::Bow { .. } | Goal::Melee { .. }));
    // Bow in either hand (main hand first).
    let bow = m.equipment[..2].iter().flatten().any(|i| i.id == "minecraft:bow");
    if bow {
        // Bogged and parched: 50 (Hard) / 70.
        let (hard, normal) = if matches!(m.kind, Kind::Bogged | Kind::Parched) { (50, 70) } else { (20, 40) };
        let interval = if s.level.difficulty_id() == 3 { hard } else { normal };
        m.goals.add(
            4,
            Goal::Bow {
                speed: 1.0,
                min_interval: interval,
                radius2: 225.0,
                attack_time: -1,
                see: 0,
                cw: false,
                back: false,
                strafe: -1,
            },
        );
    } else {
        m.goals.add(4, melee(1.2, false, Melee::Plain));
    }
}

impl Server {
    /// `ate`: the sheep grazes (wool grows back, baby ages by 60 s).
    pub(crate) fn mob_ate(&mut self, m: &mut Mob) {
        self.mob_game_event(m, "minecraft:eat");
        if m.kind == Kind::Sheep {
            m.sheared = false;
            // Lamb with locked age: does not grow.
            if m.is_baby() && !m.mount.age_locked {
                super::mounts::age_up(m, 60, false);
            }
        }
    }

    /// Birth.
    pub(crate) fn breed(&mut self, m: &mut Mob, partner: i32) {
        if super::surface::breed(self, m, partner) || self.mount_breed(m, partner) || super::wild::breed(self, m, partner) {
            return;
        }
        let Some(pi) = self.mob_index(partner) else { return };
        let Some(p) = self.mobs.list[pi].as_ref().map(|p| (p.variant, p.wool, p.collar)) else { return };
        let baby = self.breed_offspring(m, Some(p));
        self.s9_bred(m, partner, Some(&baby)); // S9 (before love is forgotten, which erases its cause)
        m.age = 6000;
        m.reset_love();
        if let Some(p) = self.mobs.list[pi].as_mut() {
            p.age = 6000;
            p.reset_love();
        }
        self.mob_event(m, 18);
        if self.rule("minecraft:mob_drops", true) {
            let xp = m.rng.next_int_bound(7) + 1;
            self.spawn_orbs(m.pos, xp);
        }
        self.add_mob(baby);
    }

    // ---------- Enderman ----------

    pub(crate) fn enderman_teleport_random(&mut self, m: &mut Mob) -> bool {
        if !m.alive() {
            return false;
        }
        let x = m.pos[0] + (m.rng.next_double() - 0.5) * 64.0;
        let y = m.pos[1] + f64::from(m.rng.next_int_bound(64) - 32);
        let z = m.pos[2] + (m.rng.next_double() - 0.5) * 64.0;
        self.enderman_teleport(m, [x, y, z])
    }

    pub(crate) fn enderman_teleport_towards(&mut self, m: &mut Mob, target: [f64; 3]) -> bool {
        let eye = self.player_eye_y();
        let mut d = [m.pos[0] - target[0], m.pos[1] + f64::from(m.height()) * 0.5 - eye, m.pos[2] - target[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        d = if l < 1.0e-4 { [0.0; 3] } else { d.map(|v| v / l) };
        let x = m.pos[0] + (m.rng.next_double() - 0.5) * 8.0 - d[0] * 16.0;
        let y = m.pos[1] + f64::from(m.rng.next_int_bound(16) - 8) - d[1] * 16.0;
        let z = m.pos[2] + (m.rng.next_double() - 0.5) * 8.0 - d[2] * 16.0;
        self.enderman_teleport(m, [x, y, z])
    }

    /// Refused as a passenger, after the destination draw by the caller.
    pub(crate) fn enderman_teleport(&mut self, m: &mut Mob, to: [f64; 3]) -> bool {
        if m.mount.vehicle.is_some() {
            return false;
        }
        let old = m.pos;
        // Destination brought back inside the world border.
        let k = self.border_bounds();
        let e = f64::from(1.0e-5f32);
        let clamp = |v: f64, lo: f64, hi: f64| if v < lo { lo } else { v.min(hi) };
        let to = [clamp(to[0], k[0], k[1] - e), to[1], clamp(to[2], k[2], k[3] - e)];
        let mut y = to[1];
        let mut p = super::mth::block_of(to);
        if !self.chunks.get(&(p[0] >> 4, p[2] >> 4)).is_some_and(Option::is_some) {
            return false;
        }
        // Descent to the first block of `#entities_can_teleport_to`.
        let min = self.dim.height.min_y;
        let mut found = false;
        while p[1] > min {
            p[1] -= 1;
            if self.btag_id(p, crate::tag_bits::TagId::EntitiesCanTeleportTo) {
                found = true;
                break;
            }
            y -= 1.0;
        }
        if !found || self.btag_id(p, crate::tag_bits::TagId::EndermanDoesNotTeleportTo) {
            return false;
        }
        let b = Mob::bbox_at([to[0], y, to[2]], m.width(), m.height());
        if !rc_physics::is_free(&self.world(), &b) {
            return false;
        }
        if self.box_has_liquid(&b) {
            return false;
        }
        // No forbidden block in the whole arrival box (bounds included).
        let (lo, hi) = (super::mth::block_of(b.min), super::mth::block_of(b.max));
        for x in lo[0]..=hi[0] {
            for yy in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if self.btag([x, yy, z], "minecraft:enderman_does_not_teleport_to") {
                        return false;
                    }
                }
            }
        }
        let below = [to[0].floor() as i32, y.floor() as i32 - 1, to[2].floor() as i32];
        if self.mob_fluid(below) == super::env::Fluid::Water {
            return false;
        }
        m.pos = [to[0], y, to[2]];
        m.needs_sync = true;
        self.mob_event(m, 46);
        m.nav.stop();
        let src = super::recent::vibration::Src::Living(Ent::Mob(m.id));
        self.game_event_src("minecraft:teleport", old, Some(src), None);
        let ob = super::mth::block_of(old);
        let nb = m.block_pos();
        let pack = |d: i32| d.clamp(-127, 127) + 127;
        let diff = pack(nb[0] - ob[0]) << 16 | pack(nb[1] - ob[1]) << 8 | pack(nb[2] - ob[2]);
        self.level_event(2018, ob, diff);
        self.world_sound("minecraft:entity.enderman.teleport", 5, old, 1.0, 1.0);
        self.mob_sound(m, "minecraft:entity.enderman.teleport", 1.0, 1.0);
        true
    }

    /// Attack speed, open mouth, timestamp.
    pub(crate) fn enderman_target_changed(m: &mut Mob, had: Option<Ent>) {
        if m.kind != Kind::Enderman || m.target == had {
            return;
        }
        if m.target.is_none() {
            m.target_change_time = 0;
            m.creepy = false;
            m.stared_at = false;
            m.attrs.speed.remove_mod("minecraft:attacking");
        } else {
            m.target_change_time = m.tick_count;
            m.creepy = true;
            m.attrs.speed.set_mod("minecraft:attacking", 0.15, 0);
        }
    }

    // ---------- Own ticks ----------

    /// Creeper fuse (before the common tick).
    pub(crate) fn creeper_tick(&mut self, m: &mut Mob) -> bool {
        if !m.alive() {
            return false;
        }
        m.old_swell = m.swell;
        if m.ignited {
            m.swell_dir = 1;
        }
        if m.swell_dir > 0 && m.swell == 0 {
            self.mob_sound(m, "minecraft:entity.creeper.primed", 1.0, 0.5);
            self.mob_game_event(m, "minecraft:prime_fuse");
        }
        m.swell += m.swell_dir;
        if m.swell < 0 {
            m.swell = 0;
        }
        if m.swell >= m.fuse {
            m.swell = m.fuse;
            let r = m.explosion_radius as f32 * if m.powered { 2.0 } else { 1.0 };
            m.dead = true;
            let c = m.pos;
            // Charged: the first victim that drops something from `charged_creeper/root`
            // (its head) uses up the creeper's right.
            let skull = std::mem::replace(&mut self.mobs.charged_skull, m.powered.then_some((m.id, false)));
            self.mob_explode(m, c, r);
            self.mobs.charged_skull = skull;
            self.creeper_lingering_cloud(m);
            self.mob_removed_effects(m); // "killed" removal effects after the explosion
            m.removed = true;
            return true;
        }
        false
    }

    /// Creeper under effects that explodes: lingering cloud of its effects (radius 2.5, −0.5 per
    /// use, wait 10, 300 ticks, durations × 0.25, radius that melts down to 0).
    fn creeper_lingering_cloud(&mut self, m: &Mob) {
        if m.x.effects.is_empty() {
            return;
        }
        let mut c = crate::projectiles::cloud::Cloud::new(self.proj_new_id(), m.pos);
        c.radius = 2.5;
        c.radius_on_use = -0.5;
        c.wait_time = 10;
        c.duration = 300;
        c.duration_scale = 0.25;
        c.radius_per_tick = -c.radius / c.duration as f32;
        let effects = rc_proto::nbt::Nbt::List(m.x.effects.iter().map(super::effects::Effect::to_nbt).collect());
        let mut potion = Stack { id: "minecraft:potion".into(), count: 1, components: None };
        crate::items::set_component(
            &mut potion,
            "minecraft:potion_contents",
            rc_proto::nbt::Nbt::Compound(vec![("custom_effects".into(), effects)]),
        );
        c.potion = Some(potion);
        self.cloud_add(c);
    }

    /// Creeper ignited by an item of `#creeper_igniters`: sound (flint and steel or fire charge), fuse
    /// lit, item worn (or one unit removed if it does not wear).
    fn creeper_ignite(&mut self, m: &mut Mob, slot: usize, item: &str) -> bool {
        if !self.item_tag(item, "minecraft:creeper_igniters") {
            return false;
        }
        let snd = if item == "minecraft:fire_charge" { "minecraft:item.firecharge.use" } else { "minecraft:item.flintandsteel.use" };
        let pitch = m.rng.next_float() * 0.4 + 0.8;
        let cat = Self::mob_sound_source(m);
        self.world_sound(snd, cat, m.pos, 1.0, pitch);
        m.ignited = true;
        let damageable = self.slots[slot].as_ref().is_some_and(|s| self.life.items.damageable(s));
        if damageable {
            self.hurt_item(slot, 1);
        } else {
            self.consume_held(slot);
        }
        true
    }

    /// Sun burning (§ 1.7) of the undead.
    pub(crate) fn sun_burn(&mut self, m: &mut Mob) {
        if !m.alive() || !m.kind.info().burns || self.dim.name != "minecraft:overworld" {
            return;
        }
        let br = self.magic_light(m.block_pos());
        if br <= 0.5 {
            return;
        }
        let roll = m.rng.next_float() * 30.0;
        if roll >= (br - 0.4) * 2.0 {
            return;
        }
        let eye = super::mth::block_of([m.pos[0], m.eye_y(), m.pos[2]]);
        if self.mob_in_water_or_rain(m) || !self.can_see_sky(eye) {
            return;
        }
        // F04: the zombie horse shelters under its armor (= body).
        let slot = if m.kind == Kind::ZombieHorse || m.kind == Kind::ZombieNautilus { 6 } else { 5 };
        if let Some(h) = m.equipment[slot].clone() {
            if self.life.items.damageable(&h) {
                let n = m.rng.next_int_bound(2);
                if n > 0 {
                    let mut h = h;
                    let d = self.life.items.damage(&h) + n;
                    if d >= self.life.items.max_damage(&h) {
                        m.equipment[slot] = None;
                    } else {
                        crate::items::set_component(&mut h, "minecraft:damage", rc_proto::nbt::Nbt::Int(d));
                        m.equipment[slot] = Some(h);
                    }
                }
            }
        } else if m.fire < 160 {
            m.fire = 160;
        }
    }

    /// Chicken laying.
    pub(crate) fn chicken_tick(&mut self, m: &mut Mob) {
        if !m.alive() || m.is_baby() || m.chicken_jockey {
            return;
        }
        m.egg_time -= 1;
        if m.egg_time > 0 {
            return;
        }
        // Gift table `gameplay/chicken_lay` (egg according to the variant), in its named sequence;
        // sound and event only if an item came out.
        let variant = self.regs.entries("minecraft:chicken_variant").get(m.variant as usize).cloned();
        let ctx = crate::loot::Ctx {
            comps: variant.map(|v| ("minecraft:chicken/variant".to_owned(), v)).into_iter().collect(),
            ..Default::default()
        };
        let drops = super::surface::loot(self, "minecraft:gameplay/chicken_lay", &ctx);
        let mut any = false;
        for d in drops {
            if d.count > 0 {
                any = true;
                self.spawn_at_location(m, d, 0.0);
            }
        }
        if any {
            let p = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
            self.mob_sound(m, "minecraft:entity.chicken.egg", 1.0, p);
            self.game_event_at("minecraft:entity_place", m.pos, Some(Ent::Mob(m.id)));
        }
        m.egg_time = m.rng.next_int_bound(6000) + 6000;
    }

    /// Age and love of animals.
    pub(crate) fn ageable_tick(&mut self, m: &mut Mob) {
        // Fed baby: a growth particle every 4 ticks for 40 ticks (entity's random draw:
        // X, Y, Z), even dead.
        if m.x.forced_timer > 0 {
            if m.x.forced_timer % 4 == 0 {
                let (w, h) = (f64::from(m.width()), f64::from(m.height()));
                let x = m.pos[0] + w * (2.0 * m.rng.next_double() - 1.0);
                let y = m.pos[1] + h * m.rng.next_double() + 0.5;
                let z = m.pos[2] + w * (2.0 * m.rng.next_double() - 1.0);
                self.particles_full("minecraft:happy_villager", 0, [x, y, z], [0.0; 3], [0.0; 3], 1, 0);
            }
            m.x.forced_timer -= 1;
        }
        if !m.alive() {
            return;
        }
        // Frozen babies (false): age lock of any species; F04: skeleton
        // and zombie horses.
        let frozen = (super::mounts::handles(m.kind) && !super::mounts::can_age_up(m)) || m.mount.age_locked;
        if m.age < 0 {
            if !frozen {
                m.age += 1;
            }
        } else if m.age > 0 {
            m.age -= 1;
        }
        if m.kind.is_animal() {
            if m.age != 0 {
                m.in_love = 0;
            }
            if m.in_love > 0 {
                m.in_love -= 1;
                if m.in_love % 10 == 0 {
                    // Heart particles: draws kept.
                    let _ = (m.rng.next_gaussian(), m.rng.next_gaussian(), m.rng.next_gaussian());
                    let _ = (m.rng.next_double(), m.rng.next_double(), m.rng.next_double());
                }
            }
        }
        // Age lock particles (the brain family does them in its own step).
        if !super::brainy::handles(m.kind) {
            super::brainy::age_lock_particles(self, m);
        }
    }

    /// Golden dandelion on a baby outside `#cannot_be_age_locked`, delay
    /// of 40 ticks elapsed: lock toggled, age reset to a baby's start, particles, sound.
    pub(crate) fn golden_dandelion(&mut self, m: &mut Mob, slot: usize, item: &str) -> bool {
        if item != "minecraft:golden_dandelion"
            || !m.kind.ageable()
            || !m.is_baby()
            || m.bm.lock_timer != 0
            || m.kind.cannot_be_age_locked()
        {
            return false;
        }
        m.mount.age_locked = !m.mount.age_locked;
        m.age = -24000;
        m.bm.lock_timer = 40;
        self.consume_held(slot);
        if m.mount.age_locked {
            m.persistent = true;
        }
        let snd = if m.mount.age_locked {
            "minecraft:item.golden_dandelion.use"
        } else {
            "minecraft:item.golden_dandelion.unuse"
        };
        let p = m.block_pos().map(|v| f64::from(v) + 0.5);
        self.world_sound(snd, 7, p, 1.0, 1.0);
        true
    }

    /// Wet wolf, on the ground and without a path: start of the shake (event 8). End of the AI step.
    pub(crate) fn wolf_shake_start(&self, m: &mut Mob) {
        let x = &mut m.x;
        if x.wet && !x.shaking && m.nav.is_done() && m.on_ground {
            x.shaking = true;
            x.shake = [0.0; 2];
            self.mob_event(m, 8);
        }
    }

    /// End of the living wolf's tick: wet in water or rain (shake interrupted:
    /// event 56); otherwise the shake advances by 0.05 per tick (sound at the start, end at 2), with
    /// the droplet draws (no particle on the server side).
    pub(crate) fn wolf_shake_tick(&mut self, m: &mut Mob) {
        if !m.alive() {
            return;
        }
        if self.mob_in_water_or_rain(m) {
            m.x.wet = true;
            if m.x.shaking {
                self.mob_event(m, 56);
                m.x.shaking = false;
                m.x.shake = [0.0; 2];
            }
            return;
        }
        if !m.x.shaking {
            return;
        }
        if m.x.shake[1] == 0.0 {
            let pitch = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
            self.mob_sound(m, "minecraft:entity.wolf.shake", m.sound_volume(), pitch);
            self.mob_game_event(m, "minecraft:entity_action");
        }
        m.x.shake[0] = m.x.shake[1];
        m.x.shake[1] += 0.05;
        if m.x.shake[0] >= 2.0 {
            m.x.wet = false;
            m.x.shaking = false;
            m.x.shake = [0.0; 2];
        }
        let a = m.x.shake[1];
        if a > 0.4 {
            let n = (super::mth::sin((a - 0.4) * std::f32::consts::PI) * 7.0) as i32;
            for _ in 0..n {
                let _ = (m.rng.next_float(), m.rng.next_float());
            }
        }
    }

    /// Dead player (rule `forgive_dead_players`): neutral mobs within ±32 (±10 in height)
    /// of their block that have them as anger target drop it.
    pub(crate) fn forgive_dead_player(&mut self) {
        let b = self.pos.map(|v| v.floor());
        let area = rc_physics::Aabb {
            min: [b[0] - 32.0, b[1] - 10.0, b[2] - 32.0],
            max: [b[0] + 33.0, b[1] + 11.0, b[2] + 33.0],
        };
        let uuid = self.uuid;
        for m in self.mobs.list.iter_mut().flatten() {
            let neutral = matches!(
                m.kind,
                Kind::Bee | Kind::IronGolem | Kind::PolarBear | Kind::Wolf | Kind::Enderman | Kind::ZombifiedPiglin
            );
            if neutral && !m.removed && m.touches(&area) && m.angry_at_player(uuid) {
                m.stop_being_angry();
            }
        }
    }

    /// Persistent anger (wolf): ends at the deadline or against a non-attackable player.
    pub(crate) fn anger_tick(&self, m: &mut Mob) {
        if m.anger_end >= 0
            && (self.time >= m.anger_end || (m.anger_target == Some(Ent::Player) && self.game_mode == 1))
        {
            m.anger_end = -1;
            m.anger_target = None;
        }
    }

    // ---------- Player interactions ----------

    pub(crate) fn consume_held(&mut self, slot: usize) {
        if self.game_mode == 1 {
            return;
        }
        if let Some(s) = &mut self.slots[slot] {
            s.count -= 1;
            if s.count <= 0 {
                self.slots[slot] = None;
            }
        }
    }

    /// Right click on a mob with hand `hand` (0 main, 1 off: the client
    /// sends it when the main one did nothing); true if the action took place.
    pub(crate) fn player_interact(&mut self, id: i32, hand: i32) -> bool {
        if self.game_mode == 3 {
            return false;
        }
        let slot = match hand {
            0 => 36 + self.held as usize,
            1 => 45,
            _ => return false,
        };
        let Some(i) = self.mob_index(id) else { return false };
        let mut m = self.mobs.list[i].take().expect("mob");
        let held_s9 = self.slots[slot].clone(); // S9
        let r = self.interact_mob(&mut m, slot);
        if r {
            self.s9_entity_interact(held_s9.as_ref(), &m); // S9
        }
        self.mobs.list[i] = Some(m);
        if r {
            self.broadcast_menus();
        }
        r
    }

    fn interact_mob(&mut self, m: &mut Mob, slot: usize) -> bool {
        let item = self.slots[slot].as_ref().map(|s| s.id.clone()).unwrap_or_default();
        if !m.alive() || dist2(self.pos, m.pos) > 64.0 {
            return false;
        }
        // Name tag, egg of the same species: before everything else.
        if self.important_interact(m, slot) {
            return true;
        }
        // Then the own interaction; if it does nothing, the item acts on the mob.
        self.mob_interact(m, slot, &item) || self.item_on_mob(m, slot)
    }

    fn mob_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> bool {
        let item = item.to_owned();
        if m.kind == Kind::SulfurCube {
            return self.sulfur_interact(m, slot).unwrap_or(false);
        }
        // W8c: shears on equipment (before the mob's own interaction).
        if self.shear_equipment(m, slot) {
            return true;
        }
        // F05: trading (villager, wandering trader).
        if super::villager::is_npc(m.kind)
            && let Some(r) = self.npc_interact(m, &item)
        {
            return r;
        }
        if super::mounts::handles(m.kind) {
            return self.mount_interact(m, slot); // F04
        }
        if super::golems::handles(m.kind) {
            return self.golem_interact(m, slot, &item).unwrap_or(false);
        }
        // F09: fish bucket, fish to the dolphin, nautiluses.
        if super::sealife::handles(m.kind)
            && let Some(r) = super::sealife::interact(self, m, slot, &item)
        {
            return r;
        }
        // F10: animals with a brain (food, milking, brush, buckets, allay).
        if super::brainy::handles(m.kind) {
            return self.bz_brainy_interact(m, slot, &item).unwrap_or(false);
        }
        // F11: flowers with effect (bee), bamboo (panda).
        if super::wild::handles(m.kind)
            && let Some(r) = super::wild::interact(self, m, slot, &item)
        {
            return r;
        }
        // F12: strider (mount, feed, saddle).
        if let Some(r) = self.nether_interact(m, slot, &item) {
            return r;
        }
        // F13: gold ingot to the piglin, food of the hoglin.
        if super::piglins::handles(m.kind)
            && let Some(r) = self.pig_interact(m, slot, &item)
        {
            return r;
        }
        // F14: happy ghast (harness, ride, food of the baby).
        if let Some(r) = self.recent_interact(m, slot, &item) {
            return r;
        }
        // F02: surface animals (mushroom, ocelot, parrot).
        if super::surface::handles(m.kind)
            && let Some(r) = super::surface::interact(self, m, slot, &item)
        {
            return r;
        }
        match m.kind {
            Kind::Creeper => return self.creeper_ignite(m, slot, &item),
            Kind::Cow if item == "minecraft:bucket" && !m.is_baby() => {
                self.world_sound("minecraft:entity.cow.milk", 7, self.pos, 1.0, 1.0);
                if self.game_mode != 1 {
                    self.consume_held(slot);
                    let milk = Stack { id: "minecraft:milk_bucket".into(), count: 1, components: None };
                    if self.slots[slot].is_none() {
                        self.slots[slot] = Some(milk);
                    } else {
                        self.drop_from_player(milk, false);
                    }
                }
                return true;
            }
            Kind::Sheep if item == "minecraft:shears" && !m.sheared && !m.is_baby() => {
                self.sheep_shear(m, 7);
                self.sheared_by_player(m);
                self.hurt_item(slot, 1);
                return true;
            }
            // Bogged: shearing (mushrooms dropped at box height, saved state).
            Kind::Bogged if item == "minecraft:shears" && !m.sheared => {
                self.entity_sound(m.id, "minecraft:entity.bogged.shear", 7, 1.0, 1.0);
                let drops = self.simple_loot("minecraft:shearing/bogged", None);
                for d in drops {
                    self.drop_at_height(m, d, f64::from(m.height()));
                }
                m.sheared = true;
                self.sheared_by_player(m);
                self.hurt_item(slot, 1);
                return true;
            }
            // Zombie villager: golden apple under weakness → cure.
            Kind::ZombieVillager if item == "minecraft:golden_apple" => {
                if m.x.effects.iter().any(|e| e.id == "minecraft:weakness") {
                    self.consume_held(slot);
                    let t = m.rng.next_int_bound(2401) + 3600;
                    let starter = Some(self.uuid);
                    self.start_cure(m, starter, t);
                }
                return true;
            }
            Kind::Wolf | Kind::Cat => return self.tame_interact(m, slot),
            _ => {}
        }
        self.animal_food_interact(m, slot, &item)
    }

    /// Loot table with no particular context (shearing).
    pub(crate) fn simple_loot(&mut self, table: &str, sheep: Option<(String, bool)>) -> Vec<Stack> {
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, n: &str| {
            let id = data.blocks.iter().position(|b| b.name == n).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
        let ctx = crate::loot::Ctx { sheep, ..Default::default() };
        let seq =
            self.life.pack.get("loot_table", table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        match seq {
            Some(s) => {
                let rng = self.life.seqs.get(&s);
                loot.drops(table, &ctx, rng)
            }
            None => {
                let mut rng = self.life.rng;
                let d = loot.drops(table, &ctx, &mut rng);
                self.life.rng = rng;
                d
            }
        }
    }
}

// ---------- Enderman: blocks taken and placed ----------

/// Outline shapes of blocks, without fluid ("outline" ray).
struct OutlineRay<'a>(&'a Server);

impl rc_physics::ray::RayWorld for OutlineRay<'_> {
    fn outline(&self, p: [i32; 3]) -> &[rc_data::Aabb] {
        let info = self.0.v().info(self.0.get(p));
        self.0.data.shape(info.outline)
    }

    fn fluid(&self, _p: [i32; 3]) -> Option<(bool, f64)> {
        None
    }

    fn interaction(&self, p: [i32; 3]) -> &[rc_data::Aabb] {
        rc_physics::ray::interaction_shape(&self.0.data, self.0.get(p))
    }
}

impl Server {
    /// Sheep shearing: sound in the author's category (players 7, blocks 4
    /// for a dispenser), wool dropped unit by unit, sheep sheared.
    pub(crate) fn sheep_shear(&mut self, m: &mut Mob, category: i32) {
        self.entity_sound(m.id, "minecraft:entity.sheep.shear", category, 1.0, 1.0);
        let color = super::combat::DYES[(m.wool & 15) as usize].to_owned();
        let drops = self.simple_loot("minecraft:shearing/sheep", Some((color, false)));
        // One entity per unit, at +1: starting velocity of a dropped item, then extra
        // drawn from the sheep's random draw.
        for d in drops {
            for _ in 0..d.count {
                let r = &mut self.mobs.rng;
                let base = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
                let r = &mut m.rng;
                let v = [
                    base[0] + f64::from((r.next_float() - r.next_float()) * 0.1),
                    base[1] + f64::from(r.next_float() * 0.05),
                    base[2] + f64::from((r.next_float() - r.next_float()) * 0.1),
                ];
                let one = Stack { count: 1, ..d.clone() };
                self.spawn_item([m.pos[0], m.pos[1] + 1.0, m.pos[2]], v, one, 10);
            }
        }
        m.sheared = true;
    }

    /// Does an entity other than `except` touch cell `p` (unit cube)?
    fn any_entity_in_cell(&self, except: i32, p: [i32; 3]) -> bool {
        let c = p.map(f64::from);
        let b = rc_physics::Aabb { min: c, max: [c[0] + 1.0, c[1] + 1.0, c[2] + 1.0] };
        self.entities_in(&b, false, Some(except), false) > 0
    }

    /// Placement of the carried block: cell drawn within ±1 block (height 0..2), empty, placed on a
    /// full block that is not bedrock, block that can be held at that place, no entity inside.
    pub(crate) fn enderman_leave_tick(&mut self, m: &mut Mob) {
        let x = (m.pos[0] - 1.0 + m.rng.next_double() * 2.0).floor() as i32;
        let y = (m.pos[1] + m.rng.next_double() * 2.0).floor() as i32;
        let z = (m.pos[2] - 1.0 + m.rng.next_double() * 2.0).floor() as i32;
        let p = [x, y, z];
        let below = [x, y - 1, z];
        let Some(carried) = m.carried else { return };
        let carried = self.update_from_neighbour_shapes(carried, p);
        let ok = self.is_air(p)
            && !self.is_air(below)
            && self.bname(below) != "minecraft:bedrock"
            && self.full_collision(below)
            && self.can_survive(carried, p)
            && !self.any_entity_in_cell(m.id, p);
        if ok {
            self.set_block(p, carried, crate::blocks::flag::ALL);
            self.game_event_block("minecraft:block_place", p, Some(Ent::Mob(m.id)), Some(carried));
            m.carried = None;
        }
    }

    /// Taking a block: cell drawn within ±2 blocks (height 0..3), reached by an outline ray
    /// from the center of the enderman's cell at the same height; the default state of the block
    /// is carried.
    pub(crate) fn enderman_take_tick(&mut self, m: &mut Mob) {
        let x = (m.pos[0] - 2.0 + m.rng.next_double() * 4.0).floor() as i32;
        let y = (m.pos[1] + m.rng.next_double() * 3.0).floor() as i32;
        let z = (m.pos[2] - 2.0 + m.rng.next_double() * 4.0).floor() as i32;
        let p = [x, y, z];
        let b = m.block_pos();
        let from = [f64::from(b[0]) + 0.5, f64::from(y) + 0.5, f64::from(b[2]) + 0.5];
        let to = [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5];
        let hit = rc_physics::ray::clip(&OutlineRay(self), from, to, rc_physics::ray::Fluids::None);
        // Missed: the returned position is the cell of the arrival point, hence the targeted cell.
        let reachable = hit.miss || hit.pos == p;
        if reachable && self.btag_id(p, crate::tag_bits::TagId::EndermanHoldable) {
            let state = self.get(p);
            let name = self.bname(p).to_owned();
            self.remove_block(p, false);
            self.game_event_block("minecraft:block_destroy", p, Some(Ent::Mob(m.id)), Some(state));
            m.carried = Some(self.default_state(&name));
        }
    }

    /// Carried block re-read (`carriedBlockState`, air counts as nothing).
    pub(crate) fn enderman_load_carried(&self, m: &mut Mob, e: &rc_proto::nbt::Nbt) {
        if m.kind != Kind::Enderman {
            return;
        }
        m.carried = e
            .get("carriedBlockState")
            .and_then(|s| self.states.state_of(&self.data, s))
            .filter(|&s| !self.v().is_air(s));
    }

    /// Carried block written (`carriedBlockState`, 26.3 state format: name alone for the default
    /// state, otherwise `id` and `properties`).
    pub(crate) fn enderman_save_carried(&self, m: &Mob, e: &mut rc_proto::nbt::Nbt) {
        if m.kind != Kind::Enderman {
            return;
        }
        match m.carried {
            Some(s) => crate::nbtio::set(e, "carriedBlockState", self.states.palette_entry(&self.data, s)),
            None => crate::nbtio::remove(e, "carriedBlockState"),
        }
    }
}

// ---------- Cat curled up against the sleeping owner ----------

impl Server {
    /// Is another cat lying or curled up in the box of the targeted cell widened by 2?
    fn cat_space_occupied(&self, m: &Mob, gp: [i32; 3]) -> bool {
        let c = gp.map(f64::from);
        let area = rc_physics::Aabb { min: c.map(|v| v - 2.0), max: c.map(|v| v + 3.0) };
        self.mobs
            .list
            .iter()
            .flatten()
            .any(|o| o.kind == Kind::Cat && o.id != m.id && (o.cat_lying || o.cat_relax) && o.touches(&area))
    }

    /// Is the owner (player) sleeping?
    fn cat_owner_sleeping(&self, m: &Mob) -> bool {
        m.owner == Some(self.uuid) && self.ent_alive(Ent::Player) && self.life.vit.sleeping.is_some()
    }

    /// Start: tamed, no order to sit, owner asleep at 10 blocks at most, on a
    /// block where a cat can lie down; targeted cell = bed cell opposite to its orientation.
    pub(crate) fn cat_relax_can_use(&self, m: &Mob, goal_pos: &mut Option<[i32; 3]>) -> bool {
        if !m.tame || m.sitting || !self.cat_owner_sleeping(m) {
            return false;
        }
        if dist2(m.pos, self.pos) > 100.0 {
            return false;
        }
        let op = self.pos.map(|v| v.floor() as i32);
        if !self.btag_id(op, crate::tag_bits::TagId::CatsCanLieOn) {
            return false;
        }
        let facing = self.data.state_properties(self.st(op)).find(|(k, _)| *k == "facing").map(|(_, v)| v.to_owned());
        let gp = match facing.as_deref() {
            Some("north") => [op[0], op[1], op[2] + 1],
            Some("south") => [op[0], op[1], op[2] - 1],
            Some("west") => [op[0] + 1, op[1], op[2]],
            Some("east") => [op[0] - 1, op[1], op[2]],
            _ => op,
        };
        *goal_pos = Some(gp);
        !self.cat_space_occupied(m, gp)
    }

    pub(crate) fn cat_relax_can_continue(&self, m: &Mob, goal_pos: Option<[i32; 3]>) -> bool {
        let Some(gp) = goal_pos else { return false };
        m.tame && !m.sitting && self.cat_owner_sleeping(m) && !self.cat_space_occupied(m, gp)
    }

    /// Towards the targeted cell at each evaluation; near the owner (distance² < 2.5): curled up
    /// (looking at them) then lying after 16 evaluation ticks (8 reduced).
    pub(crate) fn cat_relax_tick(&mut self, m: &mut Mob, goal_pos: Option<[i32; 3]>, on_bed: &mut i32) {
        let Some(gp) = goal_pos else { return };
        m.cat_sit_pose = false;
        self.move_to_pos(m, gp.map(f64::from), f64::from(1.1f32));
        if dist2(m.pos, self.pos) < 2.5 {
            *on_bed += 1;
            if *on_bed > super::mth::reduced(16) {
                m.cat_lying = true;
                m.cat_relax = false;
            } else {
                let eye = self.player_eye_y();
                super::goals::mob_look_at_direct(m, self.pos, eye, 45.0, 45.0);
                m.cat_relax = true;
            }
        } else {
            m.cat_lying = false;
        }
    }

    /// Stop: standing; morning gift if the owner slept 100 ticks and according to the gift
    /// chance of the moment (0.7 at dawn in the day timeline, 0 otherwise).
    pub(crate) fn cat_relax_stop(&mut self, m: &mut Mob, on_bed: &mut i32) {
        m.cat_lying = false;
        if self.life.vit.sleep_timer >= 100 {
            let r = self.mobs.rng.next_float();
            if r < self.cat_gift_chance() {
                self.cat_morning_gift(m);
            }
        }
        *on_bed = 0;
        m.cat_relax = false;
        m.nav.stop();
    }

    /// Gift chance (`gameplay/cat_waking_up_gift_chance`): "day" timeline of the main
    /// world, 0.7 from 23667 to 362 (24000 loop), 0 elsewhere and outside that world.
    fn cat_gift_chance(&self) -> f32 {
        if self.dim.name != "minecraft:overworld" {
            return 0.0;
        }
        let t = self.day_time().rem_euclid(24_000);
        if !(362..23_667).contains(&t) { 0.7 } else { 0.0 }
    }

    /// Random jump within ±5/±2/±5 (no particles, outside blocks forbidden to cats) then
    /// loot from the gift table in front of them.
    fn cat_morning_gift(&mut self, m: &mut Mob) {
        let b = m.block_pos();
        let x = f64::from(b[0] + m.rng.next_int_bound(11) - 5);
        let y = f64::from(b[1] + m.rng.next_int_bound(5) - 2);
        let z = f64::from(b[2] + m.rng.next_int_bound(11) - 5);
        self.mob_random_teleport(m, [x, y, z], "minecraft:cat_does_not_teleport_to");
        let c = m.block_pos();
        let yaw = m.body_yaw.to_radians();
        let at = [f64::from(c[0]) - f64::from(yaw.sin()), f64::from(c[1]), f64::from(c[2]) + f64::from(yaw.cos())];
        let ctx = crate::loot::Ctx::default();
        let drops = super::surface::loot(self, "minecraft:gameplay/cat_morning_gift", &ctx);
        for d in drops {
            let vx = self.life.rng.next_double() * 0.2 - 0.1;
            let vz = self.life.rng.next_double() * 0.2 - 0.1;
            self.spawn_item(at, [vx, 0.2, vz], d, 0);
        }
    }

    /// Random jump: descent to a block where it can land,
    /// free box with no liquid, no forbidden block (tag `avoid`) under or in the
    /// box. True if the mob moved.
    pub(crate) fn mob_random_teleport(&mut self, m: &mut Mob, to: [f64; 3], avoid: &str) -> bool {
        let mut y = to[1];
        let mut p = to.map(|v| v.floor() as i32);
        if !self.chunks.get(&(p[0] >> 4, p[2] >> 4)).is_some_and(Option::is_some) {
            return false;
        }
        let min = self.dim.height.min_y;
        let mut found = false;
        while p[1] > min {
            p[1] -= 1;
            if self.btag_id(p, crate::tag_bits::TagId::EntitiesCanTeleportTo) {
                found = true;
                break;
            }
            y -= 1.0;
        }
        if !found || self.btag(p, avoid) {
            return false;
        }
        let bb = Mob::bbox_at([to[0], y, to[2]], m.width(), m.height());
        if !rc_physics::is_free(&self.world(), &bb) {
            return false;
        }
        let lo = bb.min.map(|v| v.floor() as i32);
        let hi = bb.max.map(|v| v.ceil() as i32 - 1);
        for xx in lo[0]..=hi[0] {
            for yy in lo[1]..=hi[1] {
                for zz in lo[2]..=hi[2] {
                    let q = [xx, yy, zz];
                    if self.mob_fluid(q) != super::env::Fluid::None || self.btag(q, avoid) {
                        return false;
                    }
                }
            }
        }
        m.pos = [to[0], y, to[2]];
        m.needs_sync = true;
        m.nav.stop();
        true
    }
}

impl Server {
    /// End of the cat's tick: begging meow (wild one tried, every 100 ticks) then
    /// purr every 5 ticks when lying down or curled up.
    pub(crate) fn cat_post_tick(&mut self, m: &mut Mob) {
        if m.kind != Kind::Cat {
            return;
        }
        if !m.tame && m.tick_count % 100 == 0 && m.goals.running(|g| matches!(g, Goal::Tempt { .. })) {
            let s = self
                .variant_sound(m, "beg_for_food_sound")
                .unwrap_or_else(|| "minecraft:entity.cat.beg_for_food".into());
            self.mob_sound(m, &s, 1.0, 1.0);
        }
        if (m.cat_lying || m.cat_relax) && m.tick_count % 5 == 0 {
            let s = self.variant_sound(m, "purr_sound").unwrap_or_else(|| "minecraft:entity.cat.purr".into());
            let v = 0.6 + 0.4 * (m.rng.next_float() - m.rng.next_float());
            self.mob_sound(m, &s, v, 1.0);
        }
    }
}

// ---------- Jockeys ----------

impl Server {
    /// Baby zombie: existing chicken within ±5/3/5 without a passenger (5%), otherwise new chicken
    /// (5%), which it rides.
    pub(crate) fn zombie_chicken_jockey(&mut self, m: &mut Mob) {
        if self.mobs.rng.next_float() < 0.05 {
            let b = m.bbox();
            let area = rc_physics::Aabb {
                min: [b.min[0] - 5.0, b.min[1] - 3.0, b.min[2] - 5.0],
                max: [b.max[0] + 5.0, b.max[1] + 3.0, b.max[2] + 5.0],
            };
            let found = self.mobs.list.iter().position(|o| {
                o.as_ref().is_some_and(|o| {
                    o.kind == Kind::Chicken && !o.removed && o.mount.passengers.is_empty() && o.touches(&area)
                })
            });
            if let Some(i) = found
                && let Some(mut c) = self.mobs.list[i].take()
            {
                c.chicken_jockey = true;
                self.mob_start_riding(m, &mut c, false);
                self.mobs.list[i] = Some(c);
            }
        } else if self.mobs.rng.next_float() < 0.05 {
            let mut c = self.new_mob(Kind::Chicken, m.pos, m.yaw);
            c.pitch = 0.0;
            let mut g = super::spawn::Group::default();
            self.finalize_spawn(&mut c, super::spawn::Reason::Jockey, &mut g);
            c.chicken_jockey = true;
            self.mob_start_riding(m, &mut c, false);
            self.add_mob(c);
        }
    }

    /// Spider: skeleton rider (1 in 100), finalized with the same spawn reason.
    pub(crate) fn spider_skeleton_jockey(&mut self, m: &mut Mob, reason: super::spawn::Reason) {
        let mut sk = self.new_mob(Kind::Skeleton, m.pos, m.yaw);
        sk.pitch = 0.0;
        let mut g = super::spawn::Group::default();
        self.finalize_spawn(&mut sk, reason, &mut g);
        if self.mob_start_riding(&mut sk, m, false) {
            self.add_mob(sk);
        }
    }
}
