//! Frog and tadpole: brains, tongue that swallows small cubes (the froglight of a magma cube
//! comes from the loot table), spawn laying, long jumps toward preferred blocks, croaking;
//! tadpole that grows (conversion to frog), fed, locked, bucketed.

use rc_proto::nbt::Nbt;

use super::super::env::Fluid;
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::dist2;
use super::super::species::Kind;
use super::brain::*;
use crate::nbtio;
use crate::player::Stack;
use crate::server::Server;

/// Frog brain.
pub fn brain(rng: &mut JRand) -> Brain {
    use K::*;
    use St::*;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::Panic { speed: 2.0, armadillo: false, sniffer: false },
            Bk::LookSink { lo: 45, hi: 90 },
            move_sink(),
            Bk::CountDown(TemptCd),
            Bk::CountDown(LjCd),
        ],
    );
    let tempt = || Bk::Tempt { speed: Spd::Fixed(1.25), close: Close::Fixed(2.5), sniffer: false };
    let idle = vec![
        at(0, Act::Idle, look_sometimes(6.0, true)),
        at(0, Act::Idle, Bk::MakeLove { speed: 1.0, close: 2, at: 0, sniffer: false }),
        at(1, Act::Idle, tempt()),
        at(2, Act::Idle, Bk::StartAttacking(Finder::Attackable)),
        at(3, Act::Idle, Bk::FindLand { range: 6, speed: 1.0, next: 0 }),
        at(
            4,
            Act::Idle,
            run_one(
                &[(Walk, Absent)],
                0x4652_4f47,
                vec![
                    (Bk::Stroll { speed: 1.0, from_water: true }, 1),
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 1),
                    (Bk::Croak { counter: 0 }, 3),
                    (Bk::Trigger(Cond::OnGround), 2),
                ],
            ),
        ),
    ];
    let swim = vec![
        at(0, Act::Swim, look_sometimes(6.0, true)),
        at(1, Act::Swim, tempt()),
        at(2, Act::Swim, Bk::StartAttacking(Finder::Attackable)),
        at(3, Act::Swim, Bk::FindLand { range: 8, speed: 1.5, next: 0 }),
        at(
            5,
            Act::Swim,
            try_all(
                &[(Walk, Absent)],
                vec![
                    (Bk::StrollSwim { speed: 0.75 }, 1),
                    (Bk::Stroll { speed: 1.0, from_water: true }, 1),
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 1),
                    (Bk::Trigger(Cond::InWater), 5),
                ],
            ),
        ),
    ];
    let lay = vec![
        at(0, Act::LaySpawn, look_sometimes(6.0, true)),
        at(1, Act::LaySpawn, Bk::StartAttacking(Finder::Attackable)),
        at(2, Act::LaySpawn, Bk::FindLandNearLiquid { range: 8, speed: 1.0, next: 0 }),
        at(3, Act::LaySpawn, Bk::LaySpawn),
        at(
            4,
            Act::LaySpawn,
            run_one(
                &[],
                0x4652_4f48,
                vec![
                    (Bk::Stroll { speed: 1.0, from_water: true }, 2),
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 1),
                    (Bk::Croak { counter: 0 }, 2),
                    (Bk::Trigger(Cond::OnGround), 1),
                ],
            ),
        ),
    ];
    let tongue = seq(0, Act::Tongue, vec![Bk::StopAttacking { axolotl: false }, Bk::Tongue(Tongue::default())]);
    let jump = vec![
        at(0, Act::LongJump, Bk::LongJumpMid { lo: 100, hi: 140, sound: "minecraft:entity.frog.step" }),
        at(1, Act::LongJump, Bk::LongJump(Box::new(super::goat::long_jump(100, 140, 2, 4, true)))),
    ];
    Brain::new(
        rng,
        &[Sn::Living, Sn::HurtBy, Sn::FrogAttackables, Sn::FrogTempt, Sn::InWater],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[(LjMid, Absent), (InWater, Absent)], &[]), idle),
            (act(Act::Swim, &[(LjMid, Absent), (InWater, Present)], &[]), swim),
            (act(Act::LaySpawn, &[(LjMid, Absent), (Pregnant, Present)], &[]), lay),
            (act(Act::Tongue, &[(Attack, Present)], &[Attack]), tongue),
            (act(Act::LongJump, &[(Tempting, Absent), (Breed, Absent), (LjCd, Absent), (InWater, Absent)], &[]), jump),
        ],
    )
}

/// Tadpole brain (idle only).
pub fn tadpole_brain(rng: &mut JRand) -> Brain {
    use K::*;
    use St::*;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::Panic { speed: 2.0, armadillo: false, sniffer: false },
            Bk::LookSink { lo: 45, hi: 90 },
            move_sink(),
            Bk::CountDown(TemptCd),
        ],
    );
    let idle = vec![
        at(0, Act::Idle, look_sometimes(6.0, true)),
        at(1, Act::Idle, Bk::Tempt { speed: Spd::Fixed(1.25), close: Close::Fixed(2.5), sniffer: false }),
        at(
            2,
            Act::Idle,
            try_all(
                &[(Walk, Absent)],
                vec![
                    (Bk::StrollSwim { speed: 0.5 }, 2),
                    (Bk::WalkFromLook { speed: Spd::Fixed(0.5), close: 3, axolotl: false }, 3),
                    (Bk::Trigger(Cond::InWater), 5),
                ],
            ),
        ),
    ];
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players, Sn::HurtBy, Sn::FrogTempt],
        vec![(core(), core_nodes), (act(Act::Idle, &[], &[]), idle)],
    )
}

// ---------- Variants ----------

/// Default variant (temperate), resolved in the registry at construction.
pub fn default_variant(m: &Mob) -> i32 {
    m.variant
}

/// Variant read back (name), otherwise temperate.
pub fn init(s: &Server, m: &mut Mob) {
    let reg = s.regs.entries("minecraft:frog_variant");
    let temperate = reg.iter().position(|x| x == "minecraft:temperate").unwrap_or(0) as i32;
    let name = m.bm.frog_variant.clone();
    m.variant = name.and_then(|n| reg.iter().position(|x| *x == n)).map_or(temperate, |i| i as i32);
    if let Some(d) = m.data.iter_mut().find(|d| d.index == 18) {
        d.default = rc_proto_v777::entity::Meta::Variant(temperate, 27);
        d.value = rc_proto_v777::entity::Meta::Variant(m.variant, 27);
        d.dirty = false;
    }
}

pub fn variant_name(m: &Mob) -> Option<String> {
    m.bm.frog_variant.clone()
}

/// Finalization: biome variant, memories (jump 100..140, world random).
pub fn finalize(s: &mut Server, m: &mut Mob) {
    if let Some(v) = s.select_variant("frog", m.block_pos()) {
        m.variant = v;
        m.bm.frog_variant = s.regs.entries("minecraft:frog_variant").get(v as usize).cloned();
    }
    let lj = 100 + s.mobs.rng.next_int_bound(41);
    if let Some(b) = m.bm.brain.as_mut() {
        b.mem.lj_cd.set(lj);
    }
}

// ---------- Prey and tongue ----------

/// Tag `frog_food`, size-1 cubes only.
pub fn can_eat(s: &Server, o: &Mob) -> bool {
    if o.kind.is_cube() && o.cube.size != 1 {
        return false;
    }
    s.bz_etag(o.kind, "minecraft:frog_food")
}

pub fn set_tongue_target(m: &mut Mob, t: Option<Ent>) {
    m.bm.tongue = match t {
        Some(Ent::Mob(id)) => Some(id),
        Some(Ent::Player) => None,
        None => None,
    };
}

fn add_unreachable(b: &mut Brain, u: u128) {
    let mut list = b.mem.unreachable.v.clone().unwrap_or_default();
    let add = !list.contains(&u);
    if list.len() == 5 && add {
        list.remove(0);
    }
    if add {
        list.push(u);
    }
    b.mem.unreachable.set_exp(list, 100);
}

/// Target reachable within 1.75 blocks, otherwise blacklisted (100 ticks) and target forgotten;
/// not while croaking; edible.
pub fn tongue_check(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    let Some(target) = b.mem.attack.get() else { return false };
    let Some(tp) = s.ent_pos(target) else { return false };
    let tb = super::super::mth::block_of(tp);
    let path = super::path_begin(s, m, |s, m| s.create_path(m, tb, 0));
    let reach = path.is_some_and(|p| p.dist_to_target < 1.75);
    if !reach {
        b.mem.attack.erase();
        if let Ent::Mob(id) = target
            && let Some(u) = s.mob_ref(id).map(|o| o.uuid)
        {
            add_unreachable(b, u);
        }
    }
    let edible = match target {
        Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| can_eat(s, o)),
        Ent::Player => false,
    };
    reach && super::pose(m) != super::POSE_CROAKING && edible
}

pub fn tongue_start(s: &mut Server, m: &mut Mob, b: &mut Brain, tg: &mut Tongue) {
    let Some(target) = b.mem.attack.get() else { return };
    b.mem.look.set(Track::ent(target, true));
    set_tongue_target(m, Some(target));
    if let Some(p) = s.ent_pos(target) {
        b.mem.walk.set(Walk::to(p, 2.0, 0));
    }
    tg.path_counter = 10;
    tg.state = 0;
}

pub fn tongue_tick(s: &mut Server, m: &mut Mob, b: &mut Brain, tg: &mut Tongue) {
    let Some(target) = b.mem.attack.get() else { return };
    set_tongue_target(m, Some(target));
    match tg.state {
        0 => {
            let tp = s.ent_pos(target).unwrap_or(m.pos);
            if dist2(tp, m.pos).sqrt() < f64::from(1.75f32) {
                s.bz_mob_sound_src(m, "minecraft:entity.frog.tongue", 6, 2.0, 1.0);
                super::set_pose(m, super::POSE_USING_TONGUE);
                // The prey is pulled toward the frog (× 0.75).
                if let Ent::Mob(id) = target
                    && let Some(i) = s.mob_index(id)
                    && let Some(o) = s.mobs.list[i].as_mut()
                {
                    let d = [m.pos[0] - o.pos[0], m.pos[1] - o.pos[1], m.pos[2] - o.pos[2]];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let n = if l < 1.0e-4 { [0.0; 3] } else { d.map(|x| x / l) };
                    o.vel = n.map(|x| x * 0.75);
                    o.needs_sync = true;
                }
                tg.eat_timer = 0;
                tg.state = 1;
            } else if tg.path_counter <= 0 {
                b.mem.walk.set(Walk::to(tp, 2.0, 0));
                tg.path_counter = 10;
            } else {
                tg.path_counter -= 1;
            }
        }
        1 => {
            let before = tg.eat_timer;
            tg.eat_timer += 1;
            if before >= 6 {
                tg.state = 2;
                eat_entity(s, m);
            }
        }
        2 => {
            if tg.eat_timer >= 10 {
                tg.state = 3;
            } else {
                tg.eat_timer += 1;
            }
        }
        _ => {}
    }
}

/// Swallows the prey: normal hit; killed, it is removed (the loot comes from its table, which
/// gives the froglight to a magma cube killed by a frog).
pub(crate) fn eat_entity(s: &mut Server, m: &mut Mob) {
    s.bz_mob_sound_src(m, "minecraft:entity.frog.eat", 6, 2.0, 1.0);
    let Some(id) = m.bm.tongue else { return };
    if !s.mob_ref(id).is_some_and(Mob::alive) {
        return;
    }
    // Variant component of the killer, read by the loot table (`damage_source_properties`).
    let comps = variant_name(m).map(|v| vec![("minecraft:frog/variant".to_owned(), v)]).unwrap_or_default();
    s.mobs.brainy.killer = Some((m.id, comps));
    super::melee_hit(s, m, Ent::Mob(id));
    s.mobs.brainy.killer = None;
    // "Killed" removal: the removal effects (burst, cobwebs, slimes) happen as at the end of the
    // 20 death ticks.
    if let Some(i) = s.mob_index(id)
        && s.mobs.list[i].as_ref().is_some_and(|o| !o.alive())
        && let Some(mut o) = s.mobs.list[i].take()
    {
        s.mob_removed_effects(&mut o);
        o.removed = true;
        s.mobs.list[i] = Some(o);
    }
}

// ---------- Laying ----------

/// On the ground out of water, spawn placed on the neighboring water (top face with no shape,
/// fluid or block carrying the spawn, air above).
pub fn lay_spawn(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    if m.in_water || !m.on_ground {
        return false;
    }
    let bp = m.block_pos();
    let below = [bp[0], bp[1] - 1, bp[2]];
    for d in [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]] {
        let r = [below[0] + d[0], below[1], below[2] + d[2]];
        let top_empty = !top_face(s, r);
        let supports = s.mob_source_water(r) || s.btag_id(r, crate::tag_bits::TagId::SupportsFrogspawn);
        if top_empty && supports {
            let sp = [r[0], r[1] + 1, r[2]];
            if s.is_air(sp) {
                let st = s.default_state("minecraft:frogspawn");
                s.set_block(sp, st, crate::blocks::flag::ALL);
                s.game_event_block("minecraft:block_place", sp, Some(super::super::mob::Ent::Mob(m.id)), Some(st));
                s.bz_mob_sound_src(m, "minecraft:entity.frog.lay_spawn", 4, 1.0, 1.0);
                b.mem.pregnant.erase();
                return true;
            }
        }
    }
    true
}

/// Does the collision shape touch the top of the block (non-empty top face)?
fn top_face(s: &Server, p: [i32; 3]) -> bool {
    let mut boxes = Vec::new();
    s.phys.collision(s.st(p), &mut boxes);
    boxes.iter().any(|b| b.max[1] >= 1.0 - 1.0e-7)
}

// ---------- Long jump ----------

/// Frog landing: no fluid (cell, below, above); preferred blocks accepted; a trapdoor cell (or
/// air above a trapdoor) is accepted without further check (like the game, despite the malus);
/// otherwise default landing.
pub fn acceptable_landing(s: &Server, m: &Mob, p: [i32; 3]) -> bool {
    let below = [p[0], p[1] - 1, p[2]];
    let above = [p[0], p[1] + 1, p[2]];
    if s.mob_fluid(p) != Fluid::None || s.mob_fluid(below) != Fluid::None || s.mob_fluid(above) != Fluid::None {
        return false;
    }
    if s.btag_id(p, crate::tag_bits::TagId::FrogPreferJumpTo) || s.btag_id(below, crate::tag_bits::TagId::FrogPreferJumpTo) {
        return true;
    }
    let view = s.mob_view(m);
    let t = super::super::nav::static_path_type(s, &view, p);
    let tb = super::super::nav::static_path_type(s, &view, below);
    use super::super::nav::PathType::Trapdoor;
    if t != Trapdoor && (!s.is_air(p) || tb != Trapdoor) { super::goat::default_landing(s, m, p) } else { true }
}

// ---------- Tadpole ----------

/// Age to reach to become a frog.
pub const TICKS_TO_FROG: i32 = 24000;

/// End of the AI step: ages (unless locked), lock particle timer.
pub fn tadpole_ai_step(s: &mut Server, m: &mut Mob) {
    if !m.mount.age_locked {
        set_tadpole_age(s, m, m.bm.tadpole_age + 1);
    }
    super::age_lock_particles(s, m);
}

fn set_tadpole_age(s: &mut Server, m: &mut Mob, age: i32) {
    m.bm.tadpole_age = age;
    if age >= TICKS_TO_FROG && !m.removed {
        grow_up(s, m);
    }
}

/// Conversion to frog: "conversion" finalization (biome variant, memories), persistent, growth
/// sound.
fn grow_up(s: &mut Server, m: &mut Mob) {
    let mut f = s.converted(m, Kind::Frog, false, false);
    let mut g = super::super::spawn::Group::default();
    s.finalize_spawn(&mut f, super::super::spawn::Reason::Conversion, &mut g);
    f.persistent = true;
    s.mob_sound(m, "minecraft:entity.tadpole.grow_up", 0.15, 1.0);
    m.removed = true;
    s.add_mob(f);
}

/// Right click on a tadpole: food (slime ball), golden dandelion, water bucket.
pub fn tadpole_interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if s.item_tag(item, "minecraft:frog_food") && !m.mount.age_locked {
        s.consume_held(slot);
        let left = (TICKS_TO_FROG - m.bm.tadpole_age).max(0);
        let secs = (left / 20) as f32 * 0.1;
        let age = m.bm.tadpole_age + secs as i32 * 20;
        set_tadpole_age(s, m, age);
        // Particle (no server-side effect): three draws from the entity random.
        let _ = (m.rng.next_double(), m.rng.next_double(), m.rng.next_double());
        return Some(true);
    }
    if item == "minecraft:golden_dandelion" && m.bm.lock_timer == 0 {
        m.mount.age_locked = !m.mount.age_locked;
        m.bm.tadpole_age = 0;
        m.bm.lock_timer = 40;
        s.consume_held(slot);
        if m.mount.age_locked {
            m.persistent = true;
        }
        let snd = if m.mount.age_locked {
            "minecraft:item.golden_dandelion.use"
        } else {
            "minecraft:item.golden_dandelion.unuse"
        };
        let p = m.block_pos().map(|v| f64::from(v) + 0.5);
        s.world_sound(snd, 7, p, 1.0, 1.0);
        return Some(true);
    }
    super::bucket_pickup(s, m, slot, item)
}

/// Tadpole bucket data: age and lock.
pub fn bucket_data(m: &Mob, data: &mut Nbt) {
    nbtio::set(data, "Age", Nbt::Int(m.bm.tadpole_age));
    nbtio::set(data, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked)));
}

pub fn load_bucket_data(m: &mut Mob, data: &Nbt) {
    if let Some(a) = nbtio::int(data, "Age") {
        m.bm.tadpole_age = a as i32;
    }
    m.mount.age_locked = nbtio::int(data, "AgeLocked").is_some_and(|v| v != 0);
}

/// Bucket item of the family.
pub fn bucket_item(k: Kind) -> &'static str {
    if k == Kind::Tadpole { "minecraft:tadpole_bucket" } else { "minecraft:axolotl_bucket" }
}

/// Filled bucket (default bucket data, then species-specific).
pub fn bucket_of(m: &Mob) -> Stack {
    let mut b = Stack { id: bucket_item(m.kind).into(), count: 1, components: None };
    if let Some(n) = &m.custom_name {
        crate::items::set_component(&mut b, "minecraft:custom_name", n.clone());
    }
    let mut data = super::super::interact::bucket_default_data(m);
    if m.kind == Kind::Tadpole {
        bucket_data(m, &mut data);
    } else {
        super::axolotl::bucket_data(m, &mut b, &mut data);
    }
    crate::items::set_component(&mut b, "minecraft:bucket_entity_data", data);
    b
}
