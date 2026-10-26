//! Goat: brain (core, idle, long jump, ram), long jump shared with the frog, ram preparation
//! and hit (knockback, horns broken on blocks of the `snaps_goat_horn` tag), screaming goat,
//! milking, babies.

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::{block_of, dist2};
use super::super::species::Kind;
use super::brain::*;
use crate::player::Stack;
use crate::server::Server;

/// Interval between two long jumps (600 to 1,200 ticks).
pub const JUMP_LO: i32 = 600;
pub const JUMP_HI: i32 = 1200;

/// Goat brain (sensors, activities in the game's insertion order).
pub fn brain(rng: &mut JRand) -> Brain {
    use K::*;
    use St::*;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::Swim { chance: 0.8 },
            Bk::Panic { speed: 2.0, armadillo: false, sniffer: false },
            Bk::LookSink { lo: 45, hi: 90 },
            move_sink(),
            Bk::CountDown(TemptCd),
            Bk::CountDown(LjCd),
            Bk::CountDown(RamCd),
        ],
    );
    let idle = vec![
        at(0, Act::Idle, look_sometimes(6.0, true)),
        at(0, Act::Idle, Bk::MakeLove { speed: 1.0, close: 2, at: 0, sniffer: false }),
        at(1, Act::Idle, Bk::Tempt { speed: Spd::Fixed(1.25), close: Close::Fixed(2.5), sniffer: false }),
        at(2, Act::Idle, Bk::BabyFollow { lo: 5, hi: 16, speed: Spd::Fixed(1.25) }),
        at(
            3,
            Act::Idle,
            run_one(
                &[],
                0x474f_4154,
                vec![
                    (Bk::Stroll { speed: 1.0, from_water: true }, 2),
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 2),
                    (Bk::DoNothing { lo: 30, hi: 60 }, 1),
                ],
            ),
        ),
    ];
    let jump = vec![
        at(0, Act::LongJump, Bk::LongJumpMid { lo: JUMP_LO, hi: JUMP_HI, sound: "minecraft:entity.goat.step" }),
        at(1, Act::LongJump, Bk::LongJump(Box::new(long_jump(JUMP_LO, JUMP_HI, 5, 5, false)))),
    ];
    let ram = vec![at(0, Act::Ram, Bk::Ram { dir: [0.0; 3] }), at(1, Act::Ram, Bk::PrepareRam(Box::default()))];
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players, Sn::Items, Sn::Adult, Sn::HurtBy, Sn::FoodTempt],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[(RamCd, Present), (LjCd, Present)], &[]), idle),
            (act(Act::LongJump, &[(Tempting, Absent), (Breed, Absent), (Walk, Absent), (LjCd, Absent)], &[]), jump),
            (act(Act::Ram, &[(Tempting, Absent), (Breed, Absent), (RamCd, Absent)], &[]), ram),
        ],
    )
}

/// Fresh long-jump state.
pub fn long_jump(lo: i32, hi: i32, height: i32, width: i32, frog: bool) -> LongJump {
    LongJump {
        lo,
        hi,
        height,
        width,
        mult: 3.571_428_8,
        frog,
        candidates: Vec::new(),
        not_preferred: Vec::new(),
        wants_preferred: false,
        initial: None,
        chosen: None,
        tries: 0,
        prepare_start: 0,
        angles: JRand::from_time(0x4c4a),
    }
}

/// Initialization of the goat's memories (world random): jump 600..1,200, ram 600..6,000.
pub fn init_memories(s: &mut Server, m: &mut Mob) {
    let lj = JUMP_LO + s.mobs.rng.next_int_bound(JUMP_HI - JUMP_LO + 1);
    let ram = 600 + s.mobs.rng.next_int_bound(6000 - 600 + 1);
    if let Some(b) = m.bm.brain.as_mut() {
        b.mem.lj_cd.set(lj);
        b.mem.ram_cd.set(ram);
    }
}

/// Finalization: memories, screaming (2%), damage by age, single horn (10%).
pub fn finalize(s: &mut Server, m: &mut Mob) {
    init_memories(s, m);
    m.bm.screaming = s.mobs.rng.next_double() < 0.02;
    age_boundary(m);
    if !m.is_baby() && s.mobs.rng.next_float() < 0.1 {
        if s.mobs.rng.next_bool() {
            m.bm.left_horn = false;
        } else {
            m.bm.right_horn = false;
        }
    }
}

/// Damage 1 baby, 2 adult.
pub fn age_boundary(m: &mut Mob) {
    m.bm.boundary_baby = m.is_baby();
    m.attrs.attack.base = if m.is_baby() { 1.0 } else { 2.0 };
}

/// Baby goat: memories, screaming inherited from a randomly chosen parent
/// or 2%.
pub fn offspring(s: &mut Server, m: &mut Mob, partner: i32) -> Mob {
    let mut baby = s.new_mob(Kind::Goat, m.pos, 0.0);
    init_memories(s, &mut baby);
    let pick_me = s.mobs.rng.next_bool();
    let parent_screams = if pick_me { m.bm.screaming } else { s.mob_ref(partner).is_some_and(|p| p.bm.screaming) };
    baby.bm.screaming = parent_screams || s.mobs.rng.next_double() < 0.02;
    baby.age = -24000;
    age_boundary(&mut baby);
    baby
}

// ---------- Long jump (goat and frog) ----------

pub fn long_jump_check(s: &mut Server, m: &mut Mob, b: &mut Brain, j: &mut LongJump) -> bool {
    let ok = m.on_ground && !m.in_water && m.lava_height <= 0.0 && s.bname(m.block_pos()) != "minecraft:honey_block";
    if !ok {
        let v = j.lo + s.mobs.rng.next_int_bound(j.hi - j.lo + 1);
        b.mem.lj_cd.set(v / 2);
    }
    ok
}

pub fn long_jump_start(_s: &mut Server, m: &mut Mob, _b: &mut Brain, j: &mut LongJump) {
    j.chosen = None;
    j.tries = 20;
    j.initial = Some(m.pos);
    let o = m.block_pos();
    j.candidates.clear();
    // Fastest X first, then y, then z; weight = ⌈distance²⌉.
    for z in o[2] - j.width..=o[2] + j.width {
        for y in o[1] - j.height..=o[1] + j.height {
            for x in o[0] - j.width..=o[0] + j.width {
                let p = [x, y, z];
                if p == o {
                    continue;
                }
                let d = [x - o[0], y - o[1], z - o[2]];
                j.candidates.push((p, d[0] * d[0] + d[1] * d[1] + d[2] * d[2]));
            }
        }
    }
    if j.frog {
        j.not_preferred.clear();
        j.wants_preferred = m.rng.next_float() < 0.5;
    }
}

pub fn long_jump_continue(s: &mut Server, m: &mut Mob, b: &mut Brain, j: &mut LongJump) -> bool {
    let valid =
        j.initial == Some(m.pos) && j.tries > 0 && !m.in_water && (j.chosen.is_some() || !j.candidates.is_empty());
    if !valid && !b.mem.lj_mid.has() {
        let v = j.lo + s.mobs.rng.next_int_bound(j.hi - j.lo + 1);
        b.mem.lj_cd.set(v / 2);
        b.mem.look.erase();
    }
    valid
}

pub fn long_jump_tick(s: &mut Server, m: &mut Mob, b: &mut Brain, j: &mut LongJump, t: i64) {
    if let Some(v) = j.chosen {
        if t - j.prepare_start >= 40 {
            m.yaw = m.body_yaw;
            super::set_discard_friction(m, true);
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let boost = len + f64::from(jump_boost(m));
            m.vel = v.map(|x| x * (boost / len));
            m.needs_sync = true;
            b.mem.lj_mid.set(true);
            let snd = jump_sound(m);
            s.bz_mob_sound_src(m, snd, 6, 1.0, 1.0);
        }
    } else {
        j.tries -= 1;
        pick_candidate(s, m, b, j, t);
    }
}

fn jump_sound(m: &Mob) -> &'static str {
    match m.kind {
        Kind::Frog => "minecraft:entity.frog.long_jump",
        _ if m.bm.screaming => "minecraft:entity.goat.screaming.long_jump",
        _ => "minecraft:entity.goat.long_jump",
    }
}

/// 0.1 × (level + 1) under jump boost.
fn jump_boost(m: &Mob) -> f32 {
    m.x.effects.iter().find(|e| e.id == "minecraft:jump_boost").map_or(0.0, |e| 0.1 * (e.amp as f32 + 1.0))
}

fn pick_candidate(s: &mut Server, m: &mut Mob, b: &mut Brain, j: &mut LongJump, t: i64) {
    while !j.candidates.is_empty() {
        let Some((target, _)) = jump_candidate(s, j) else { continue };
        let o = m.block_pos();
        if o[0] == target[0] && o[2] == target[2] {
            continue;
        }
        let ok = if j.frog { super::frog::acceptable_landing(s, m, target) } else { default_landing(s, m, target) };
        if !ok {
            continue;
        }
        let center = [f64::from(target[0]) + 0.5, f64::from(target[1]) + 0.5, f64::from(target[2]) + 0.5];
        let Some(v) = optimal_jump(s, m, j, center) else { continue };
        b.mem.look.set(Track::block(target));
        let path = super::path_begin(s, m, |s, m| s.create_path_len(m, target, 0, 8.0));
        if path.as_ref().is_none_or(|p| !p.reached) {
            j.chosen = Some(v);
            j.prepare_start = t;
            return;
        }
    }
}

/// Weighted draw (world random), removed from the list; the frog prefers (one time in two) the
/// blocks of the `frog_prefer_jump_to` tag, the others are set aside then returned in the order
/// they were drawn.
fn jump_candidate(s: &mut Server, j: &mut LongJump) -> Option<([i32; 3], i32)> {
    if !j.frog || !j.wants_preferred {
        return weighted_take(s, &mut j.candidates);
    }
    while !j.candidates.is_empty() {
        if let Some(c) = weighted_take(s, &mut j.candidates) {
            let below = [c.0[0], c.0[1] - 1, c.0[2]];
            if s.btag_id(below, crate::tag_bits::TagId::FrogPreferJumpTo) {
                return Some(c);
            }
            j.not_preferred.push(c);
        }
    }
    (!j.not_preferred.is_empty()).then(|| j.not_preferred.remove(0))
}

fn weighted_take(s: &mut Server, list: &mut Vec<([i32; 3], i32)>) -> Option<([i32; 3], i32)> {
    let total: i64 = list.iter().map(|c| i64::from(c.1)).sum();
    if total <= 0 {
        // Zero weight: nothing is drawn, the list does not move (the game then loops forever;
        // impossible here, all positions are at distance ≥ 1).
        list.clear();
        return None;
    }
    let mut sel = s.mobs.rng.next_int_bound(total as i32);
    let i = list.iter().position(|c| {
        sel -= c.1;
        sel < 0
    })?;
    Some(list.remove(i))
}

/// Default landing: block below full as rendered, cell without malus.
pub fn default_landing(s: &Server, m: &Mob, p: [i32; 3]) -> bool {
    let view = s.mob_view(m);
    let t = super::super::nav::static_path_type(s, &view, p);
    s.solid_render([p[0], p[1] - 1, p[2]]) && super::super::nav::mob_malus(m, t) == 0.0
}

/// Angles 65/70/75/80 shuffled (own random), speed clamped.
fn optimal_jump(s: &Server, m: &Mob, j: &mut LongJump, target: [f64; 3]) -> Option<[f64; 3]> {
    let mut angles = [65, 70, 75, 80];
    j.angles.shuffle(&mut angles);
    let max_v = (m.attrs.jump.value() * f64::from(j.mult)) as f32;
    angles.into_iter().find_map(|a| jump_vector(s, m, target, max_v, a))
}

/// The jump vector computation for an angle with collision checks along the parabola (box of the
/// "long jump" pose).
fn jump_vector(s: &Server, m: &Mob, target: [f64; 3], max_v: f32, angle: i32) -> Option<[f64; 3]> {
    let p = m.pos;
    let plane = {
        let d = [target[0] - p[0], 0.0, target[2] - p[2]];
        let l = (d[0] * d[0] + d[2] * d[2]).sqrt();
        if l < 1.0e-4 { [0.0; 3] } else { [d[0] / l * 0.5, 0.0, d[2] / l * 0.5] }
    };
    let tp = [target[0] - plane[0], target[1] - plane[1], target[2] - plane[2]];
    let dir = [tp[0] - p[0], tp[1] - p[1], tp[2] - p[2]];
    let angrad = angle as f32 * std::f32::consts::PI / 180.0;
    let xz = dir[2].atan2(dir[0]);
    let r2 = dir[0] * dir[0] + dir[2] * dir[2];
    let r = r2.sqrt();
    let y = dir[1];
    let g = m.attrs.gravity.value();
    let sin2 = f64::from(2.0f32 * angrad).sin();
    let cos2 = f64::from(angrad).cos().powi(2);
    let sin_a = f64::from(angrad).sin();
    let cos_a = f64::from(angrad).cos();
    let (sxz, cxz) = (xz.sin(), xz.cos());
    let v0sqr = r2 * g / (r * sin2 - 2.0 * y * cos2);
    if v0sqr < 0.0 {
        return None;
    }
    let v0 = v0sqr.sqrt();
    if v0 > f64::from(max_v) {
        return None;
    }
    let v0r = v0 * cos_a;
    let v0y = v0 * sin_a;
    let samples = (r / v0r).ceil() as i32 * 2;
    let mut ri = 0.0;
    let mut prev: Option<[f64; 3]> = None;
    let (w, h, _) = super::pose_dims_for(m, super::POSE_LONG_JUMPING);
    for _ in 0..samples - 1 {
        ri += r / f64::from(samples);
        let yi = sin_a / cos_a * ri - ri.powi(2) * g / (2.0 * v0sqr * cos_a.powi(2));
        let sample = [p[0] + ri * cxz, p[1] + yi, p[2] + ri * sxz];
        if let Some(pr) = prev
            && !clear_transition(s, w, h, pr, sample)
        {
            return None;
        }
        prev = Some(sample);
    }
    let k = f64::from(0.95f32);
    Some([v0r * cxz * k, v0y * k, v0r * sxz * k])
}

fn clear_transition(s: &Server, w: f32, h: f32, a: [f64; 3], b: [f64; 3]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let min = f64::from(w.min(h));
    let checks = (len / min).ceil() as i32;
    let n = if len < 1.0e-4 { [0.0; 3] } else { d.map(|x| x / len) };
    let mut next = a;
    for i in 0..checks {
        next = if i == checks - 1 {
            b
        } else {
            let st = min * f64::from(0.9f32);
            [next[0] + n[0] * st, next[1] + n[1] * st, next[2] + n[2] * st]
        };
        let bb = Mob::bbox_at(next, w, h);
        if !rc_physics::is_free(&s.world(), &bb) {
            return false;
        }
    }
    true
}

// ---------- Ram ----------

/// Ram target conditions: combat with no range, line of sight required; not a goat; armor stand
/// only if `mob_griefing`.
fn ram_target_ok(s: &Server, m: &mut Mob, e: Ent) -> bool {
    if let Ent::Mob(id) = e
        && s.mob_ref(id).is_none_or(|o| o.kind == Kind::Goat)
    {
        return false;
    }
    s.targeting_ok(m, e, true, 0.0, true, true)
}

fn walkable(s: &Server, m: &Mob, p: [i32; 3]) -> bool {
    if !s.nav_stable(m, p) {
        return false;
    }
    let view = s.mob_view(m);
    let t = super::super::nav::static_path_type(s, &view, p);
    super::super::nav::mob_malus(m, t) == 0.0
}

/// North, east, south, west up to 7 walkable cells; at least
/// 4 away; the closest reachable one.
fn ram_start_pos(s: &Server, m: &mut Mob, target: Ent) -> Option<[i32; 3]> {
    let tp = block_of(s.ent_pos(target)?);
    if !walkable(s, m, tp) {
        return None;
    }
    let mut cands = Vec::new();
    for d in [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]] {
        let mut c = tp;
        for _ in 0..7 {
            c = [c[0] + d[0], c[1], c[2] + d[2]];
            if !walkable(s, m, c) {
                c = [c[0] - d[0], c[1], c[2] - d[2]];
                break;
            }
        }
        if (c[0] - tp[0]).abs() + (c[2] - tp[2]).abs() >= 4 {
            cands.push(c);
        }
    }
    let me = m.block_pos();
    let d2 = |c: &[i32; 3]| {
        let d = [i64::from(c[0] - me[0]), i64::from(c[1] - me[1]), i64::from(c[2] - me[2])];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
    };
    cands.sort_by_key(d2);
    cands.into_iter().find(|&c| super::path_begin(s, m, |s, m| s.create_path(m, c, 0)).is_some_and(|p| p.reached))
}

fn choose_ram_position(s: &Server, m: &mut Mob, p: &mut PrepRam, target: Ent) {
    p.reached_at = None;
    let tp = s.ent_pos(target).map(block_of);
    p.cand = ram_start_pos(s, m, target).zip(tp).map(|(start, tp)| (start, tp, target));
}

pub fn prepare_ram_start(s: &mut Server, m: &mut Mob, b: &mut Brain, p: &mut PrepRam) {
    let f = |_: &Server, _: Ent| true;
    // Closest seen entity that satisfies the ram conditions.
    let n = b.mem.visible.v.as_ref().map_or(0, |v| v.list.len());
    let mut chosen = None;
    for i in 0..n {
        let Some(e) = b.mem.visible.v.as_ref().map(|v| v.list[i]) else { break };
        if ram_target_ok(s, m, e) && s.bz_visible_contains(m, b, e) {
            chosen = Some(e);
            break;
        }
    }
    let _ = f;
    if let Some(e) = chosen {
        choose_ram_position(s, m, p, e);
    }
}

pub fn prepare_ram_tick(s: &mut Server, m: &mut Mob, b: &mut Brain, p: &mut PrepRam, t: i64) {
    let Some((start, tpos, target)) = p.cand else { return };
    b.mem.walk.set(Walk { target: Track::block(start), speed: 1.25, close: 0 });
    b.mem.look.set(Track::ent(target, true));
    let moved = s.ent_pos(target).map(block_of) != Some(tpos);
    if moved {
        s.mob_event(m, 59);
        m.nav.stop();
        choose_ram_position(s, m, p, target);
        return;
    }
    if m.block_pos() == start {
        s.mob_event(m, 58);
        let since = *p.reached_at.get_or_insert(t);
        if t - since >= 20 {
            let sx = f64::from((tpos[0] - start[0]).signum()) * 0.5;
            let sz = f64::from((tpos[2] - start[2]).signum()) * 0.5;
            b.mem.ram_target.set([f64::from(tpos[0]) + 0.5 + sx, f64::from(tpos[1]), f64::from(tpos[2]) + 0.5 + sz]);
            let snd = if m.bm.screaming {
                "minecraft:entity.goat.screaming.prepare_ram"
            } else {
                "minecraft:entity.goat.prepare_ram"
            };
            let pitch = Server::voice_pitch(m);
            s.bz_mob_sound_src(m, snd, 6, 1.0, pitch);
            p.cand = None;
        }
    }
}

pub fn prepare_ram_stop(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if !b.mem.ram_target.has() {
        s.mob_event(m, 59);
        b.mem.ram_cd.set(if m.bm.screaming { 100 } else { 600 });
    }
}

pub fn ram_start(_s: &mut Server, m: &mut Mob, b: &mut Brain, dir: &mut [f64; 3]) {
    let cur = m.block_pos();
    let Some(rt) = b.mem.ram_target.get() else { return };
    let d = [f64::from(cur[0]) - rt[0], 0.0, f64::from(cur[2]) - rt[2]];
    let l = (d[0] * d[0] + d[2] * d[2]).sqrt();
    *dir = if l < 1.0e-4 { [0.0; 3] } else { [d[0] / l, 0.0, d[2] / l] };
    b.mem.walk.set(Walk::to(rt, 3.0, 0));
}

pub fn ram_tick(s: &mut Server, m: &mut Mob, b: &mut Brain, dir: [f64; 3]) {
    let bb = m.bbox();
    // Living entities hit (player after the mobs) satisfying the conditions.
    let mut hit: Option<Ent> = None;
    let ids: Vec<i32> = s.mobs.list.iter().flatten().filter(|o| o.alive() && o.touches(&bb)).map(|o| o.id).collect();
    for id in ids {
        if id != m.id && ram_target_ok(s, m, Ent::Mob(id)) {
            hit = Some(Ent::Mob(id));
            break;
        }
    }
    if hit.is_none() && s.player_aabb().intersects(&bb) && ram_target_ok(s, m, Ent::Player) {
        hit = Some(Ent::Player);
    }
    let impact =
        if m.bm.screaming { "minecraft:entity.goat.screaming.ram_impact" } else { "minecraft:entity.goat.ram_impact" };
    if let Some(target) = hit {
        let dmg = m.attrs.attack.value() as f32;
        let ty = "minecraft:mob_attack_no_aggro";
        let lvl = |id: &str| m.x.effects.iter().find(|e| e.id == id).map_or(0, |e| e.amp + 1);
        let boost = 0.25 * (lvl("minecraft:speed") - lvl("minecraft:slowness")) as f32;
        let factor = (m.speed * 1.65).clamp(0.2, 3.0) + boost;
        let force = if m.is_baby() { 1.0 } else { 2.5 };
        match target {
            Ent::Player => {
                let _ = s.player_hurt_by_mob(m, ty, dmg, Some((m.pos[0] - s.pos[0], m.pos[2] - s.pos[2])));
                // W8c: block replayed after the hit (the shield wears down a second time).
                let blocking = s.reapply_blocking("minecraft:mob_attack", dmg, m.pos);
                let bf = if blocking { 0.5 } else { 1.0 };
                s.player_knockback(f64::from(bf * factor) * force, dir[0], dir[2]);
            }
            Ent::Mob(id) => {
                if let Some(i) = s.mob_index(id) {
                    let mut o = s.mobs.list[i].take().expect("mob");
                    s.mob_hurt_by(&mut o, m, ty, dmg);
                    // Victim's own knockback (full sulfur cube: ram aim).
                    s.ghost_push(m);
                    let by = Some(Ent::Mob(m.id));
                    s.mob_knockback(&mut o, f64::from(factor) * force, dir[0], dir[2], by, dmg, false);
                    s.ghost_pop();
                    s.mobs.list[i] = Some(o);
                }
            }
        }
        finish_ram(s, m, b);
        s.bz_mob_sound_src(m, impact, 6, 1.0, 1.0);
    } else if rammed_horn_block(s, m) {
        s.bz_mob_sound_src(m, impact, 6, 1.0, 1.0);
        if drop_horn(s, m) {
            s.bz_mob_sound_src(m, "minecraft:entity.goat.horn_break", 6, 1.0, 1.0);
        }
        finish_ram(s, m, b);
    } else {
        let lost = match (b.mem.walk.get(), b.mem.ram_target.get()) {
            (Some(w), Some(rt)) => s.bz_track_pos(w.target).is_some_and(|c| dist2(c, rt) < 0.25 * 0.25),
            _ => true,
        };
        if lost {
            finish_ram(s, m, b);
        }
    }
}

fn finish_ram(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    s.mob_event(m, 59);
    let (lo, hi) = if m.bm.screaming { (100, 300) } else { (600, 6000) };
    let v = lo + s.mobs.rng.next_int_bound(hi - lo + 1);
    b.mem.ram_cd.set(v);
    b.mem.ram_target.erase();
}

/// Block of the `snaps_goat_horn` tag ahead (direction of horizontal movement) or above.
fn rammed_horn_block(s: &Server, m: &Mob) -> bool {
    let (x, z) = (m.vel[0], m.vel[2]);
    let l = (x * x + z * z).sqrt();
    let n = if l < 1.0e-4 { [0.0, 0.0] } else { [x / l, z / l] };
    let f = block_of([m.pos[0] + n[0], m.pos[1], m.pos[2] + n[1]]);
    s.btag_id(f, crate::tag_bits::TagId::SnapsGoatHorn) || s.btag_id([f[0], f[1] + 1, f[2]], crate::tag_bits::TagId::SnapsGoatHorn)
}

/// Adult only; one horn drawn at random if both remain.
fn drop_horn(s: &mut Server, m: &mut Mob) -> bool {
    if m.is_baby() || (!m.bm.left_horn && !m.bm.right_horn) {
        return false;
    }
    let left = if !m.bm.left_horn {
        false
    } else if !m.bm.right_horn {
        true
    } else {
        m.rng.next_bool()
    };
    if left {
        m.bm.left_horn = false;
    } else {
        m.bm.right_horn = false;
    }
    let horn = create_horn(s, m);
    let dx = f64::from(m.rng.next_float() * 0.4 - 0.2);
    let dy = f64::from(m.rng.next_float() * 0.4 + 0.3);
    let dz = f64::from(m.rng.next_float() * 0.4 - 0.2);
    s.spawn_item(m.pos, [dx, dy, dz], horn, 10);
    true
}

/// Goat horn: instrument of the tag (regular or screaming) drawn by a random seeded from the hash
/// of the goat's UUID (always the same for a given goat).
pub fn create_horn(s: &Server, m: &Mob) -> Stack {
    let tag = if m.bm.screaming { "screaming_goat_horns" } else { "regular_goat_horns" };
    let list: Vec<String> = s
        .life
        .pack
        .get("tags/instrument", tag)
        .and_then(|v| v["values"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let mut st = Stack { id: "minecraft:goat_horn".into(), count: 1, components: None };
    if !list.is_empty() {
        let pick = &list[horn_pick(m.uuid, list.len())];
        crate::items::set_component(&mut st, "minecraft:instrument", rc_proto::nbt::Nbt::String(pick.clone()));
    }
    st
}

/// Rank of the instrument: random seeded from the Java hash of the UUID (`hi ^ lo`, folded halves).
pub fn horn_pick(uuid: u128, n: usize) -> usize {
    let hilo = (uuid >> 64) as u64 ^ uuid as u64;
    let hash = ((hilo >> 32) as i32) ^ (hilo as i32);
    let mut r = JRand::new(i64::from(hash));
    r.next_int_bound(n as i32) as usize
}

// ---------- Interactions ----------

/// Milking (bucket, adult), then animal food; double eating sound when the stack is not used up
/// (the game replays it after the player's interaction with an animal).
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item == "minecraft:bucket" && !m.is_baby() {
        let snd = if m.bm.screaming { "minecraft:entity.goat.screaming.milk" } else { "minecraft:entity.goat.milk" };
        s.world_sound(snd, 7, s.pos, 1.0, 1.0);
        s.bz_fill_bucket(slot, "minecraft:milk_bucket");
        return Some(true);
    }
    let r = s.bz_animal_interact(m, slot, item);
    if r == Some(true) && s.slots[slot].as_ref().is_some_and(|st| s.item_tag(&st.id, "minecraft:goat_food")) {
        let p = 0.8 + s.mobs.rng.next_float() * 0.4;
        let snd = if m.bm.screaming { "minecraft:entity.goat.screaming.eat" } else { "minecraft:entity.goat.eat" };
        s.world_sound(snd, 6, m.pos, 1.0, p);
    }
    r
}
