//! Panda (§ 4.F11): main and hidden genes (dominant: lazy, worried, playful, aggressive;
//! recessive: brown, weak), baby sneezing (slime ball), rolling, on its back, sitting
//! to eat, unhappy without bamboo, fear of thunderstorms, bite.

use super::super::goals::{Goal, TBase};
use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, cos, dist2, sin};
use super::super::species::Kind;
use super::goals::{Gate, WGoal};
use crate::player::Stack;
use crate::server::Server;

pub const SNEEZE: u8 = 2;
pub const ROLL: u8 = 4;
pub const SIT: u8 = 8;
pub const ON_BACK: u8 = 16;

/// Genes in the order of the game's ids.
pub const GENES: [&str; 7] = ["normal", "lazy", "worried", "playful", "brown", "weak", "aggressive"];
pub const NORMAL: u8 = 0;
pub const LAZY: u8 = 1;
pub const WORRIED: u8 = 2;
pub const PLAYFUL: u8 = 3;
pub const BROWN: u8 = 4;
pub const WEAK: u8 = 5;
pub const AGGRESSIVE: u8 = 6;

/// Recessive gene (brown, weak).
pub fn recessive(g: u8) -> bool {
    g == BROWN || g == WEAK
}

/// Random draw of a gene (sixteen values).
pub fn random_gene(r: &mut super::super::jrand::JRand) -> u8 {
    match r.next_int_bound(16) {
        0 => LAZY,
        1 => WORRIED,
        2 => PLAYFUL,
        4 => AGGRESSIVE,
        n if n < 9 => WEAK,
        n if n < 11 => BROWN,
        _ => NORMAL,
    }
}

/// Expressed gene.
pub fn variant(main: u8, hidden: u8) -> u8 {
    if recessive(main) { if main == hidden { main } else { NORMAL } } else { main }
}

pub fn gene(m: &Mob) -> u8 {
    variant(m.wild.main_gene, m.wild.hidden_gene)
}

pub fn is_lazy(m: &Mob) -> bool {
    gene(m) == LAZY
}

pub fn is_worried(m: &Mob) -> bool {
    gene(m) == WORRIED
}

pub fn is_playful(m: &Mob) -> bool {
    gene(m) == PLAYFUL
}

pub fn is_weak(m: &Mob) -> bool {
    gene(m) == WEAK
}

pub fn is_aggressive(m: &Mob) -> bool {
    gene(m) == AGGRESSIVE
}

fn thundering(s: &Server) -> bool {
    s.life.weather.thunder_eff() > 0.9
}

/// Worried during the thunderstorm.
pub fn is_scared(s: &Server, m: &Mob) -> bool {
    is_worried(m) && thundering(s)
}

pub fn is_eating(m: &Mob) -> bool {
    m.wild.eat > 0
}

/// Neither on its back, nor scared, nor eating, nor rolling, nor sitting.
pub fn can_perform_action(s: &Server, m: &Mob) -> bool {
    let w = &m.wild;
    !w.flag(ON_BACK) && !is_scared(s, m) && !is_eating(m) && !w.flag(ROLL) && !w.flag(SIT)
}

/// Weak → 10 HP; lazy → speed 0.07.
pub fn set_attributes(m: &mut Mob) {
    if is_weak(m) {
        m.attrs.max_health.base = 10.0;
        // Lowering the maximum brings the health down to that maximum.
        m.health = m.health.min(m.max_health());
    }
    if is_lazy(m) {
        m.attrs.speed.base = f64::from(0.07f32);
    }
}

/// The main gene choice / the hidden gene choice: an out-of-bounds id is redrawn at random.
fn set_gene(m: &mut Mob, main: bool, g: u8) {
    let g = if g > 6 { random_gene(&mut m.rng) } else { g };
    if main {
        m.wild.main_gene = g;
    } else {
        m.wild.hidden_gene = g;
    }
}

/// The draw of a gene (parent's random).
fn one_of(p: &mut Mob) -> u8 {
    if p.rng.next_bool() { p.wild.main_gene } else { p.wild.hidden_gene }
}

/// One gene from each parent (random which), 1 in 32 mutation
/// for each; draws from the baby's and parents' random.
pub fn genes_from_parents(baby: &mut Mob, p1: &mut Mob, p2: Option<&Mob>) {
    match p2 {
        None => {
            if baby.rng.next_bool() {
                let g = one_of(p1);
                set_gene(baby, true, g);
                let r = random_gene(&mut baby.rng);
                set_gene(baby, false, r);
            } else {
                let r = random_gene(&mut baby.rng);
                set_gene(baby, true, r);
                let g = one_of(p1);
                set_gene(baby, false, g);
            }
        }
        Some(p2) => {
            let mut p2 = p2.clone();
            if baby.rng.next_bool() {
                let a = one_of(p1);
                set_gene(baby, true, a);
                let b = one_of(&mut p2);
                set_gene(baby, false, b);
            } else {
                let a = one_of(&mut p2);
                set_gene(baby, true, a);
                let b = one_of(p1);
                set_gene(baby, false, b);
            }
        }
    }
    if baby.rng.next_int_bound(32) == 0 {
        let r = random_gene(&mut baby.rng);
        set_gene(baby, true, r);
    }
    if baby.rng.next_int_bound(32) == 0 {
        let r = random_gene(&mut baby.rng);
        set_gene(baby, false, r);
    }
}

/// Goals in the game's order.
pub fn register(m: &mut Mob) {
    let wrap = |gate: Gate, inner: Goal| Goal::Wild(WGoal::Gate { gate, inner: Box::new(inner) });
    let g = &mut m.goals;
    g.add(0, Goal::Float);
    g.add(
        2,
        wrap(Gate::PandaPanic, Goal::Panic { speed: 2.0, pos: [0.0; 3], tag: "minecraft:panic_environmental_causes" }),
    );
    g.add(
        2,
        Goal::Wild(WGoal::PandaBreed {
            inner: Box::new(Goal::Breed { speed: 1.0, partner: None, love_time: 0 }),
            cd: 0,
        }),
    );
    let melee = Goal::Melee {
        speed: f64::from(1.2f32),
        unseen: true,
        kind: super::super::goals::Melee::Plain,
        path: None,
        pathed: [0.0; 3],
        recalc: 0,
        cd: 0,
        last: i64::MIN / 2,
    };
    g.add(3, wrap(Gate::PandaAction, melee));
    g.add(
        4,
        Goal::Tempt {
            speed: 1.0,
            food: super::super::goals::Food::Tag("minecraft:panda_food"),
            scare: false,
            stop: 2.5,
            calm: 0,
            p: [0.0; 3],
            prot: [0.0; 2],
            on: false,
        },
    );
    let avoid = |who, max: f32| Goal::Avoid { who, max, walk: 2.0, sprint: 2.0, from: None, path: None };
    g.add(6, wrap(Gate::PandaAvoid, avoid(super::super::goals::Who::Player, 8.0)));
    g.add(6, wrap(Gate::PandaAvoid, avoid(super::super::goals::Who::Monster, 4.0)));
    g.add(7, Goal::Wild(WGoal::PandaSit { cd: 0 }));
    g.add(8, Goal::Wild(WGoal::LieOnBack { cd: 0 }));
    g.add(8, Goal::Wild(WGoal::Sneeze));
    g.add(9, Goal::Wild(WGoal::PandaLook { at: None, time: 0 }));
    g.add(10, Goal::LookAround { rel: [0.0; 2], time: 0 });
    g.add(12, Goal::Wild(WGoal::Roll));
    g.add(13, Goal::FollowParent { speed: 1.25, parent: None, recalc: 0 });
    g.add(14, Goal::Stroll { speed: 1.0, interval: 120, prob: Some(0.001), wanted: [0.0; 3] });
    m.targets.add(1, Goal::Wild(WGoal::PandaHurtBy { base: TBase::new(true, false), stamp: 0 }));
}

// ---------- Spawning ----------

/// Genes (world random), attributes, 20% babies in a new group.
pub fn finalize_spawn(s: &mut Server, m: &mut Mob, group: &mut super::super::spawn::Group) {
    let a = random_gene(&mut s.mobs.rng);
    set_gene(m, true, a);
    let b = random_gene(&mut s.mobs.rng);
    set_gene(m, false, b);
    set_attributes(m);
    m.health = m.max_health();
    if !group.ageable {
        group.baby_chance = Some(0.2);
    }
}

// ---------- Ticks ----------

/// End of `tick`: thunderstorm fear, lost target, unhappiness, sneezing, rolling, sitting, eating.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if is_worried(m) {
        if thundering(s) && !m.in_water {
            m.wild.set_flag(SIT, true);
            m.wild.eat = 0;
        } else if !is_eating(m) {
            m.wild.set_flag(SIT, false);
        }
    }
    if m.target.is_none() {
        m.wild.got_bamboo = false;
        m.wild.did_bite = false;
    }
    if m.wild.unhappy > 0 {
        if let Some(t) = m.target
            && let (Some(p), Some(eye)) = (s.ent_pos(t), s.ent_eye(t))
        {
            super::super::goals::mob_look_at_direct(m, p, eye, 90.0, 90.0);
        }
        if m.wild.unhappy == 29 || m.wild.unhappy == 14 {
            s.mob_sound(m, "minecraft:entity.panda.cant_breed", 1.0, 1.0);
        }
        m.wild.unhappy -= 1;
    }
    if m.wild.flag(SNEEZE) {
        m.wild.sneeze += 1;
        if m.wild.sneeze > 20 {
            m.wild.set_flag(SNEEZE, false);
            m.wild.sneeze = 0;
            after_sneeze(s, m);
        } else if m.wild.sneeze == 1 {
            s.mob_sound(m, "minecraft:entity.panda.pre_sneeze", 1.0, 1.0);
        }
    }
    if m.wild.flag(ROLL) {
        handle_roll(m);
    } else {
        m.wild.roll_counter = 0;
    }
    if m.wild.flag(SIT) {
        m.pitch = 0.0;
    }
    handle_eating(s, m);
}

/// Sitting with an item, 1 in 80 to start; end after 80 ticks (1 in 20);
/// beyond 100, an edible item on the ground is swallowed.
fn handle_eating(s: &mut Server, m: &mut Mob) {
    let held = m.equipment[0].is_some();
    if !is_eating(m) && m.wild.flag(SIT) && !is_scared(s, m) && held && m.rng.next_int_bound(80) == 1 {
        m.wild.eat = 1;
    } else if !held || !m.wild.flag(SIT) {
        m.wild.eat = 0;
    }
    if is_eating(m) {
        eating_particles(s, m);
        if m.wild.eat > 80 && m.rng.next_int_bound(20) == 1 {
            let ground =
                m.equipment[0].as_ref().is_some_and(|st| s.item_tag(&st.id, "minecraft:panda_eats_from_ground"));
            if m.wild.eat > 100 && ground {
                m.equipment[0] = None;
                m.wild.set_flag(SIT, false);
            }
            m.wild.eat = 0;
            return;
        }
        m.wild.eat += 1;
    }
}

/// Eating sound every 5 ticks; item particles (six, entity random draws).
fn eating_particles(s: &mut Server, m: &mut Mob) {
    if m.wild.eat % 5 != 0 {
        return;
    }
    let v = 0.5 + 0.5 * m.rng.next_int_bound(2) as f32;
    let p = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
    s.mob_sound(m, "minecraft:entity.panda.eat", v, p);
    if m.equipment[0].is_some() {
        for _ in 0..6 {
            for _ in 0..6 {
                let _ = m.rng.next_float();
            }
        }
    }
}

/// Roll (32 ticks): momentum on the first tick, bounces at ticks 7, 15, 23.
fn handle_roll(m: &mut Mob) {
    m.wild.roll_counter += 1;
    if m.wild.roll_counter > 32 {
        m.wild.set_flag(ROLL, false);
        return;
    }
    let v = m.vel;
    let c = m.wild.roll_counter;
    if c == 1 {
        let a = m.yaw * DEG;
        let k = if m.is_baby() { 0.1f32 } else { 0.2 };
        m.wild.roll_delta = [v[0] + f64::from(-sin(a) * k), 0.0, v[2] + f64::from(cos(a) * k)];
        m.vel = [m.wild.roll_delta[0], 0.27, m.wild.roll_delta[2]];
    } else if c != 7 && c != 15 && c != 23 {
        m.vel = [m.wild.roll_delta[0], v[1], m.wild.roll_delta[2]];
    } else {
        m.vel = [0.0, if m.on_ground { 0.27 } else { v[1] }, 0.0];
    }
    m.needs_sync = true;
}

/// Sound, adult pandas within 10 blocks jump, sneeze loot (slime ball).
fn after_sneeze(s: &mut Server, m: &mut Mob) {
    s.mob_sound(m, "minecraft:entity.panda.sneeze", 1.0, 1.0);
    let b = m.bbox();
    let area = rc_physics::Aabb { min: b.min.map(|v| v - 10.0), max: b.max.map(|v| v + 10.0) };
    let ids: Vec<i32> =
        s.mobs.of_kind(Kind::Panda).filter(|o| o.touches(&area)).map(|o| o.id).collect();
    for id in ids {
        let Some(i) = s.mob_index(id) else { continue };
        let Some(mut o) = s.mobs.list[i].take() else { continue };
        if !o.is_baby() && o.on_ground && !o.in_water && can_perform_action(s, &o) {
            s.mob_jump_from_ground(&mut o);
        }
        s.mobs.list[i] = Some(o);
    }
    // The panda itself (out of the list during its tick) is also in the box.
    if !m.is_baby() && m.on_ground && !m.in_water && can_perform_action(s, m) {
        s.mob_jump_from_ground(m);
    }
    if s.rule("minecraft:mob_drops", true) {
        let drops = super::super::surface::loot(s, "minecraft:gameplay/panda_sneeze", &crate::loot::Ctx::default());
        for d in drops {
            s.spawn_at_location(m, d, 0.0);
        }
    }
}

/// Pickup: empty hand, edible item on the ground, guaranteed drop.
pub fn pickup(s: &mut Server, m: &mut Mob) {
    if !m.can_pickup || !m.alive() || m.dead || !s.rule("minecraft:mob_griefing", true) || m.equipment[0].is_some() {
        return;
    }
    let bb = m.bbox();
    let hit = s
        .life
        .drops
        .iter()
        .find(|d| {
            !d.is_removed()
                && d.stack.count > 0
                && d.pickup_delay <= 0
                && s.item_tag(&d.stack.id, "minecraft:panda_eats_from_ground")
                && d.pos[0] + 0.125 > bb.min[0] - 1.0
                && d.pos[0] - 0.125 < bb.max[0] + 1.0
                && d.pos[1] + 0.25 > bb.min[1]
                && d.pos[1] < bb.max[1]
                && d.pos[2] + 0.125 > bb.min[2] - 1.0
                && d.pos[2] - 0.125 < bb.max[2] + 1.0
        })
        .map(|d| (d.id, d.stack.clone()));
    let Some((id, st)) = hit else { return };
    let n = st.count;
    m.equipment[0] = Some(st);
    m.drop_chance[0] = 2.0;
    s.mob_take(id, m.id, n);
}

/// Edible items on the ground within ±6 (or ±8), in list order.
pub fn ground_food(s: &Server, m: &Mob, r: f64) -> Vec<[f64; 3]> {
    let b = m.bbox();
    let area = rc_physics::Aabb { min: b.min.map(|v| v - r), max: b.max.map(|v| v + r) };
    s.life
        .drops
        .iter()
        .filter(|d| {
            let ib = rc_physics::Aabb {
                min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
            };
            !d.is_removed()
                && d.pickup_delay <= 0
                && ib.intersects(&area)
                && s.item_tag(&d.stack.id, "minecraft:panda_eats_from_ground")
        })
        .map(|d| d.pos)
        .collect()
}

/// Out of water, stops and sits.
pub fn try_to_sit(m: &mut Mob) {
    if !m.in_water {
        m.zza = 0.0;
        m.nav.stop();
        m.wild.set_flag(SIT, true);
    }
}

/// Bamboo within radius 8, three layers (the game's search order).
pub fn can_find_bamboo(s: &Server, m: &Mob) -> bool {
    let o = m.block_pos();
    for y in 0..3 {
        for r in 0..8 {
            let mut x = 0;
            while x <= r {
                let mut z = if x < r && x > -r { r } else { 0 };
                while z <= r {
                    if s.bname([o[0] + x, o[1] + y, o[2] + z]) == "minecraft:bamboo" {
                        return true;
                    }
                    z = if z > 0 { -z } else { 1 - z };
                }
                x = if x > 0 { -x } else { 1 - x };
            }
        }
    }
    false
}

// ---------- Sounds ----------

pub fn ambient_sound(m: &Mob) -> &'static str {
    if is_aggressive(m) {
        "minecraft:entity.panda.aggressive_ambient"
    } else if is_worried(m) {
        "minecraft:entity.panda.worried_ambient"
    } else {
        "minecraft:entity.panda.ambient"
    }
}

// ---------- Interactions ----------

/// Right click: nothing if scared; on its back → gets up; bamboo: baby →
/// grows, adult ready → love, otherwise sits and eats (old item dropped). Golden dandelion
/// on a baby: common rule.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if is_scared(s, m) {
        return Some(false);
    }
    if m.wild.flag(ON_BACK) {
        m.wild.set_flag(ON_BACK, false);
        return Some(true);
    }
    if !s.item_tag(item, "minecraft:panda_food") {
        if m.is_baby() && item == "minecraft:golden_dandelion" {
            return None;
        }
        return Some(false);
    }
    if m.target.is_some() {
        m.wild.got_bamboo = true;
    }
    if m.is_baby() && !m.mount.age_locked {
        s.consume_held(slot);
        let secs = super::super::mounts::feeding_secs(-m.age);
        super::super::mounts::age_up(m, secs, true);
        return Some(true);
    }
    if m.is_baby() {
        return Some(false);
    }
    if m.age == 0 && !m.is_in_love() {
        s.consume_held(slot);
        m.in_love = 600;
        m.love_cause = true;
        s.mob_event(m, 18);
        return Some(true);
    }
    if m.wild.flag(SIT) || m.in_water {
        return Some(false);
    }
    try_to_sit(m);
    m.wild.eat = 1;
    if let Some(old) = m.equipment[0].take()
        && s.game_mode != 1
    {
        s.spawn_at_location(m, old, 0.0);
    }
    m.equipment[0] = Some(Stack { id: item.to_owned(), count: 1, components: None });
    s.consume_held(slot);
    Some(true)
}

impl Server {
    /// Jump of a panda pushed by a baby's sneeze.
    pub(crate) fn mob_jump_from_ground(&mut self, m: &mut Mob) {
        let j = m.attrs.jump.value() as f32;
        m.vel[1] = f64::from(j).max(m.vel[1]);
        m.needs_sync = true;
    }

    /// Closest player within 8 blocks (of the breeding goal).
    pub(crate) fn panda_near_player(&self, m: &mut Mob) -> Option<Ent> {
        (dist2(m.pos, self.pos) <= 64.0 && self.targeting_ok(m, Ent::Player, false, 8.0, false, true))
            .then_some(Ent::Player)
    }
}
