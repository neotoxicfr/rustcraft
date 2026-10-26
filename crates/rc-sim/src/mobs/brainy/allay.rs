//! Allay: brain (core, idle), wanted items (identical to the held one, potion included),
//! drop-off at the liked note block or liked player, flight (flying control, gravity-free
//! movement), healing, duplication with amethyst while dancing, heard note block.

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{DEG, block_of, cos, dist2, rotlerp, sin};
use super::super::villager::brain::{DimKey, GPos};
use super::brain::*;
use crate::player::Stack;
use crate::server::Server;

/// Pitches of the throw sound.
const THROW_PITCHES: [f32; 16] =
    [0.5625, 0.625, 0.75, 0.9375, 1.0, 1.0, 1.125, 1.25, 1.5, 1.875, 2.0, 2.25, 2.5, 3.0, 3.75, 4.0];

pub fn brain(rng: &mut JRand) -> Brain {
    use K::*;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::Swim { chance: 0.8 },
            Bk::Panic { speed: 2.5, armadillo: false, sniffer: false },
            Bk::LookSink { lo: 45, hi: 90 },
            move_sink(),
            Bk::CountDown(LikedNoteblockCd),
            Bk::CountDown(PickupCd),
        ],
    );
    let idle = seq(
        0,
        Act::Idle,
        vec![
            Bk::GoToItem,
            Bk::GiveItems,
            Bk::StayClose,
            look_sometimes(6.0, false),
            run_one(
                &[],
                0x414c_4c41,
                vec![
                    (Bk::StrollFly { speed: 1.0 }, 2),
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 2),
                    (Bk::DoNothing { lo: 30, hi: 60 }, 1),
                ],
            ),
        ],
    );
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players, Sn::HurtBy, Sn::Items],
        vec![(core(), core_nodes), (act(Act::Idle, &[], &[]), idle)],
    )
}

// ---------- Items ----------

/// Same item, same potion effects.
fn same_item(a: &Stack, b: &Stack) -> bool {
    let potion = |s: &Stack| crate::items::component(s, "minecraft:potion_contents").cloned();
    a.id == b.id && potion(a) == potion(b)
}

fn can_add(s: &Server, m: &Mob, st: &Stack) -> bool {
    match &m.bm.inventory {
        None => true,
        Some(inv) => inv.id == st.id && inv.components == st.components && inv.count < s.life.items.max_stack(inv),
    }
}

/// Item in hand, `mob_griefing` rule, room, identical item.
fn wants(s: &Server, m: &Mob, st: &Stack) -> bool {
    let Some(hand) = &m.equipment[0] else { return false };
    s.rule("minecraft:mob_griefing", true) && can_add(s, m, st) && same_item(hand, st)
}

/// Item sensor: the closest wanted one, within 32 blocks, in view (box 32/16/32).
pub fn wanted_item(s: &Server, m: &Mob) -> Option<i32> {
    if m.kind != super::super::species::Kind::Allay {
        return None;
    }
    s.nearest_wanted_item(m, |st| wants(s, m, st))
}

fn has_item_to_throw(m: &Mob) -> bool {
    m.bm.inventory.is_some()
}

/// Drop-off point: liked note block (above, same dimension, 1,024 blocks, still a note block,
/// delay running; otherwise forgotten), then liked player (survival or creative, within 64 blocks).
fn deposit_target(s: &Server, m: &Mob, b: &mut Brain) -> Option<Track> {
    if let Some(g) = b.mem.liked_noteblock.get() {
        let same_dim = g.dim == DimKey::of(&s.dim.name);
        let close = same_dim && {
            let bp = m.block_pos();
            (0..3).all(|i| (bp[i] - g.pos[i]).abs() <= 1024)
        };
        if close && s.bname(g.pos) == "minecraft:note_block" && b.mem.liked_noteblock_cd.has() {
            return Some(Track::block([g.pos[0], g.pos[1] + 1, g.pos[2]]));
        }
        b.mem.liked_noteblock.erase();
    }
    liked_player(s, m, b).map(|e| Track::ent(e, true))
}

fn liked_player(s: &Server, m: &Mob, b: &Brain) -> Option<Ent> {
    let u = b.mem.liked_player.get()?;
    let ok = u == s.uuid
        && (s.game_mode == 0 || s.game_mode == 1)
        && s.ent_alive(Ent::Player)
        && dist2(s.pos, m.pos) < 64.0 * 64.0;
    ok.then_some(Ent::Player)
}

/// Is the player the one the allay likes?
pub fn likes_player(s: &Server, m: &Mob) -> bool {
    m.bm.brain.as_ref().and_then(|b| b.mem.liked_player.get()) == Some(s.uuid)
}

/// `GoToWantedItem` (1.75; interrupts walking; 32 blocks).
pub fn go_to_item(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    let Some(id) = b.mem.wanted_item.get() else { return false };
    let Some(p) = s.life.drops.iter().find(|d| d.id == id && !d.is_removed()).map(|d| d.pos) else { return false };
    let in_border = s.may_interact(super::super::mth::block_of(p));
    if b.mem.pickup_cd.has() || dist2(p, m.pos) >= 32.0 * 32.0 || !in_border || !can_pick_up(m, b) {
        return false;
    }
    b.mem.look.set(Track::Item { id, eye: true });
    b.mem.walk.set(Walk { target: Track::Item { id, eye: false }, speed: 1.75, close: 0 });
    true
}

/// No pickup delay, item in hand.
fn can_pick_up(m: &Mob, b: &Brain) -> bool {
    !b.mem.pickup_cd.has() && m.equipment[0].is_some()
}

/// The "stay close to the target" behavior (4, 16, 2.25) when no wanted item is seen.
pub fn stay_close(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    let Some(t) = deposit_target(s, m, b) else { return false };
    if b.mem.wanted_item.has() {
        return false;
    }
    let Some(c) = s.bz_track_pos(t) else { return false };
    if dist2(m.pos, c) < 16.0 * 16.0 {
        return false;
    }
    b.mem.look.set(t);
    b.mem.walk.set(Walk { target: t, speed: 2.25, close: 4 });
    true
}

/// Items in inventory and known drop-off point.
pub fn can_throw(s: &Server, m: &Mob, b: &mut Brain) -> bool {
    has_item_to_throw(m) && deposit_target(s, m, b).is_some()
}

pub fn give_start(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if let Some(t) = deposit_target(s, m, b) {
        b.mem.look.set(t);
        b.mem.walk.set(Walk { target: t, speed: 2.25, close: 3 });
    }
}

pub fn give_tick(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    let Some(t) = deposit_target(s, m, b) else { return };
    let Some(c) = s.bz_track_pos(t) else { return };
    let eye = [m.pos[0], m.eye_y(), m.pos[2]];
    if dist2(c, eye) < 9.0 {
        let liked = liked_player(s, m, b).is_some();
        throw_item(s, m, c, liked);
        b.mem.pickup_cd.set(60);
    }
}

/// Throws an item from the inventory toward the target (+1 in height; speeds 0.2/0.3/0.2); throw
/// sound one tick in seven, at 90%.
fn throw_item(s: &mut Server, m: &mut Mob, target: [f64; 3], liked: bool) {
    let Some(mut inv) = m.bm.inventory.take() else { return };
    let one = Stack { id: inv.id.clone(), count: 1, components: inv.components.clone() };
    inv.count -= 1;
    m.bm.inventory = (inv.count > 0).then_some(inv);
    let thrown = one.clone();
    throw_stack(s, m, one, [target[0], target[1] + 1.0, target[2]], [0.2, 0.3, 0.2], 0.2);
    // W8c: `allay_drop_item_on_block` for the liked player (block under the target).
    if liked {
        let below = [target[0].floor() as i32, target[1].floor() as i32 - 1, target[2].floor() as i32];
        s.trig_location("minecraft:allay_drop_item_on_block", below, Some(&thrown));
    }
    if s.time % 7 == 0 && s.mobs.rng.next_double() < 0.9 {
        let pitch = THROW_PITCHES[s.mobs.rng.next_int_bound(16) as usize];
        s.bz_mob_sound_src(m, "minecraft:entity.allay.item_thrown", 6, 1.0, pitch);
    }
}

/// Item at hand height, speed toward the target (the two random draws of the new item still
/// happen).
fn throw_stack(s: &mut Server, m: &Mob, st: Stack, target: [f64; 3], vel: [f64; 3], hand: f32) {
    let r = &mut s.mobs.rng;
    let _ = (r.next_double(), r.next_double());
    let d = [target[0] - m.pos[0], target[1] - m.pos[1], target[2] - m.pos[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let n = if l < 1.0e-4 { [0.0; 3] } else { d.map(|x| x / l) };
    let v = [n[0] * vel[0], n[1] * vel[1], n[2] * vel[2]];
    let pos = [m.pos[0], m.eye_y() - f64::from(hand), m.pos[2]];
    s.spawn_item(pos, v, st, 10);
    s.set_last_thrower(Ent::Mob(m.id)); // W8c: thrower
}

// ---------- Flight ----------

/// The mob's aim vector: head pitch and yaw of the previous tick.
pub(crate) fn view_vector(m: &Mob) -> [f64; 3] {
    let (xr, yr) = (m.pitch_o * DEG, -m.head_yaw_o * DEG);
    let (yc, ys, xc, xs) = (cos(yr), sin(yr), cos(xr), sin(xr));
    [f64::from(ys * xc), f64::from(-xs), f64::from(yc * xc)]
}

/// Flying wander (10/7, −2, cone of π/2 around the gaze); walk value zero everywhere: the first
/// valid point of the 10 tries.
pub fn fly_target_pos(s: &Server, m: &mut Mob) -> Option<[f64; 3]> {
    fly_target_pos_with(s, m, None, |_, _| 0.0)
}

/// Same wander with a home area (shift toward the area, points outside the area discarded) and
/// walk value: the best of the 10 tries (strictly greater).
pub fn fly_target_pos_with(
    s: &Server,
    m: &mut Mob,
    home: Option<([i32; 3], i32)>,
    weight: impl Fn(&Server, [i32; 3]) -> f32,
) -> Option<[f64; 3]> {
    let restrict = home.is_some_and(|(c, r)| {
        let lim = f64::from(r) + 10.0 + 1.0;
        let cc = [f64::from(c[0]) + 0.5, f64::from(c[1]) + 0.5, f64::from(c[2]) + 0.5];
        super::super::mth::dist2(cc, m.pos) < lim * lim
    });
    let mut best_w = f32::NEG_INFINITY;
    let v = view_vector(m);
    let max_y = s.dim.height.min_y + s.dim.height.sections as i32 * 16 - 1;
    let mut best: Option<[i32; 3]> = None;
    for _ in 0..10 {
        let center = super::super::mth::atan2(v[2], v[0]) - f64::from(std::f32::consts::FRAC_PI_2);
        let yr = center + f64::from(2.0 * m.rng.next_float() - 1.0) * f64::from(std::f32::consts::FRAC_PI_2);
        let dist = m.rng.next_double().sqrt() * 10.0 * f64::from(std::f32::consts::SQRT_2);
        let xt = -dist * yr.sin();
        let zt = dist * yr.cos();
        if xt.abs() > 10.0 || zt.abs() > 10.0 {
            continue;
        }
        let yt = m.rng.next_int_bound(15) - 7 - 2;
        // Offset rounded to the block first, then added to the position and rounded again.
        let dir = block_of([xt, f64::from(yt), zt]);
        let (mut dx, mut dz) = (f64::from(dir[0]), f64::from(dir[2]));
        if let Some((c, _)) = home {
            if m.pos[0] > f64::from(c[0]) {
                dx -= m.rng.next_double() * 10.0 / 2.0;
            } else {
                dx += m.rng.next_double() * 10.0 / 2.0;
            }
            if m.pos[2] > f64::from(c[2]) {
                dz -= m.rng.next_double() * 10.0 / 2.0;
            } else {
                dz += m.rng.next_double() * 10.0 / 2.0;
            }
        }
        let mut p = block_of([dx + m.pos[0], f64::from(dir[1]) + m.pos[1], dz + m.pos[2]]);
        if p[1] < s.dim.height.min_y || p[1] > max_y {
            continue;
        }
        if restrict && let Some((c, r)) = home {
            let d = (0..3).map(|i| i64::from(p[i] - c[i]).pow(2)).sum::<i64>();
            if d >= i64::from(r) * i64::from(r) {
                continue;
            }
        }
        if s.is_solid(p) {
            p[1] += 1;
            while p[1] <= max_y && s.is_solid(p) {
                p[1] += 1;
            }
        }
        let view = s.mob_view(m);
        let t = super::super::nav::static_path_type(s, &view, p);
        if super::super::nav::mob_malus(m, t) != 0.0 {
            continue;
        }
        let w = weight(s, p);
        if w > best_w {
            best_w = w;
            best = Some(p);
        }
    }
    best.map(|p| [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5])
}

/// The flying movement control (turns of 20°, stays in the air).
pub fn fly_move_control(m: &mut Mob) {
    if m.move_ctl.op == MoveOp::MoveTo {
        m.move_ctl.op = MoveOp::Wait;
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < f64::from(2.500_000_3e-7f32) {
            m.yya = 0.0;
            m.zza = 0.0;
            return;
        }
        let yr =
            (super::super::mth::atan2(zd, xd) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 - 90.0;
        m.yaw = rotlerp(m.yaw, yr, 90.0);
        let speed = if m.on_ground {
            (m.move_ctl.speed_mod * m.movement_speed()) as f32
        } else {
            (m.move_ctl.speed_mod * f64::from(0.1f32)) as f32
        };
        m.speed = speed;
        m.zza = speed;
        let sd = (xd * xd + zd * zd).sqrt();
        if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
            let xr = -(super::super::mth::atan2(yd, sd) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32;
            m.pitch = rotlerp(m.pitch, xr, 20.0);
            m.yya = if yd > 0.0 { speed } else { -speed };
        }
    } else {
        m.yya = 0.0;
        m.zza = 0.0;
    }
}

/// Water 0.02 (× 0.8), lava 0.02 (× 0.5), air speed (× 0.91); no gravity.
pub fn travel(s: &mut Server, m: &mut Mob) {
    let (speed, drag) = if m.in_water {
        (0.02, f64::from(0.8f32))
    } else if m.lava_height > 0.0 {
        (0.02, 0.5)
    } else {
        (m.speed, f64::from(0.91f32))
    };
    s.move_relative_pub(m, speed);
    let v = m.vel;
    s.mob_move(m, v);
    m.vel = m.vel.map(|x| x * drag);
}

// ---------- Tick ----------

/// End of the AI step: pickup, healing (1 HP / 10 ticks), dance stopped, duplication delay.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    pick_up(s, m);
    if m.alive() && m.tick_count % 10 == 0 {
        m.health = (m.health + 1.0).min(m.max_health());
    }
    if m.bm.dancing && should_stop_dancing(s, m) && m.tick_count % 20 == 0 {
        set_dancing(m, false);
        m.bm.jukebox = None;
    }
    if m.bm.dup_cooldown > 0 {
        m.bm.dup_cooldown -= 1;
    }
}

/// Pickup: range +1, identical items, one-slot inventory.
fn pick_up(s: &mut Server, m: &mut Mob) {
    let Some(b) = m.bm.brain.as_ref() else { return };
    if !can_pick_up(m, b) || !m.alive() || !s.rule("minecraft:mob_griefing", true) {
        return;
    }
    let bb = m.bbox();
    let ids: Vec<i32> = s
        .life
        .drops
        .iter()
        .filter(|d| {
            !d.is_removed()
                && d.pickup_delay <= 0
                && d.pos[0] + 0.125 > bb.min[0] - 1.0
                && d.pos[0] - 0.125 < bb.max[0] + 1.0
                && d.pos[1] + 0.25 > bb.min[1] - 1.0
                && d.pos[1] < bb.max[1] + 1.0
                && d.pos[2] + 0.125 > bb.min[2] - 1.0
                && d.pos[2] - 0.125 < bb.max[2] + 1.0
        })
        .map(|d| d.id)
        .collect();
    for id in ids {
        let Some(st) = s.life.drops.iter().find(|d| d.id == id && !d.is_removed()).map(|d| d.stack.clone()) else {
            continue;
        };
        if !wants(s, m, &st) {
            continue;
        }
        let max = s.life.items.max_stack(&st);
        let room = match &m.bm.inventory {
            None => max,
            Some(inv) => max - inv.count,
        };
        let n = room.min(st.count);
        if n <= 0 {
            continue;
        }
        match &mut m.bm.inventory {
            None => m.bm.inventory = Some(Stack { id: st.id.clone(), count: n, components: st.components.clone() }),
            Some(inv) => inv.count += n,
        }
        s.mob_take(id, m.id, n);
    }
}

fn should_stop_dancing(s: &Server, m: &Mob) -> bool {
    match m.bm.jukebox {
        None => true,
        Some(j) => {
            let c = [f64::from(j[0]) + 0.5, f64::from(j[1]) + 0.5, f64::from(j[2]) + 0.5];
            dist2(c, m.pos) >= 10.0 * 10.0 || s.bname(j) != "minecraft:jukebox"
        }
    }
}

/// Never enters panic.
pub fn set_dancing(m: &mut Mob, v: bool) {
    if !m.no_ai && (!v || !super::is_panicking(m)) {
        m.bm.dancing = v;
    }
}

/// Start (`playing`) or end of a heard tune: at the start, an allay that is not dancing remembers
/// this jukebox and dances (the jukebox stays remembered even if dancing is refused); at the end,
/// only the remembered jukebox (or none) stops the dance.
pub fn set_jukebox_playing(m: &mut Mob, jukebox: [i32; 3], playing: bool) {
    if playing {
        if !m.bm.dancing {
            m.bm.jukebox = Some(jukebox);
            set_dancing(m, true);
        }
    } else if m.bm.jukebox.is_none_or(|j| j == jukebox) {
        m.bm.jukebox = None;
        set_dancing(m, false);
    }
}

impl Server {
    /// `jukebox_play` / `jukebox_stop_play` events of jukebox `j`: the allay-specific listener
    /// (radius 10, at the eyes, delivered immediately) compares the eye block with the jukebox
    /// block (integer distance² ≤ 100). Allays being updated (taken out of the list) hear nothing:
    /// the jukebox emits outside the mob tick.
    pub(crate) fn bz_allays_hear_jukebox(&mut self, j: [i32; 3], playing: bool) {
        for o in self.mobs.list.iter_mut().flatten() {
            if o.kind != super::super::species::Kind::Allay || o.removed {
                continue;
            }
            let ear = block_of([o.pos[0], o.eye_y(), o.pos[2]]);
            let d: i64 = (0..3).map(|i| (i64::from(ear[i]) - i64::from(j[i])).pow(2)).sum();
            if d <= 10 * 10 {
                set_jukebox_playing(o, j, playing);
            }
        }
    }
}

// ---------- Vibrations (note block) ----------

/// The allay accepts the vibration coming from block `origin`: not without AI; if it has a liked
/// note block (same dimension, within 1,024 blocks at most on each axis), only that one.
pub(crate) fn can_receive_vibration(s: &Server, m: &Mob, origin: [i32; 3]) -> bool {
    if m.no_ai {
        return false;
    }
    let Some(liked) = m.bm.brain.as_ref().and_then(|b| b.mem.liked_noteblock.get()) else { return true };
    let me = block_of(m.pos);
    let close = liked.dim == DimKey::of(&s.dim.name) && (0..3).all(|i| (liked.pos[i] - me[i]).abs() <= 1024);
    close && liked.pos == origin
}

/// Note arrived (after its travel): the originating note block is remembered (600-tick delay
/// refreshed by notes from the same block; other blocks ignored).
pub(crate) fn on_receive_vibration(s: &Server, m: &mut Mob, info: &super::super::recent::vibration::VibInfo) {
    if info.event != "minecraft:note_block_play" {
        return;
    }
    let p = info.pos.map(|v| v.floor() as i32);
    let dim = DimKey::of(&s.dim.name);
    let Some(b) = m.bm.brain.as_mut() else { return };
    let g = GPos { dim, pos: p };
    match b.mem.liked_noteblock.get() {
        None => {
            b.mem.liked_noteblock.set(g);
            b.mem.liked_noteblock_cd.set(600);
        }
        Some(l) if l == g => b.mem.liked_noteblock_cd.set(600),
        Some(_) => {}
    }
}

/// End of the allay tick (server): listener moved to the eyes, then vibrations tick.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    s.listener_move(super::super::recent::vibration::Who::Mob(m.id), [m.pos[0], m.eye_y(), m.pos[2]]);
    s.vib_tick_mob(m);
}

// ---------- Interactions ----------

/// Amethyst while dancing (duplication), item given, item taken back (inventory thrown).
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    let held = s.slots[slot].clone();
    if m.bm.dancing && s.item_tag(item, "minecraft:duplicates_allays") && m.bm.dup_cooldown == 0 {
        let mut a = s.new_mob(super::super::species::Kind::Allay, m.pos, 0.0);
        a.persistent = true;
        a.bm.dup_cooldown = 6000;
        m.bm.dup_cooldown = 6000;
        s.add_mob(a);
        s.mob_event(m, 18);
        s.bz_mob_sound_src(m, "minecraft:block.amethyst_block.chime", 6, 2.0, 1.0);
        s.consume_held(slot);
        return Some(true);
    }
    if m.equipment[0].is_none()
        && let Some(h) = held
    {
        m.equipment[0] = Some(Stack { id: h.id.clone(), count: 1, components: h.components.clone() });
        s.send_mob_equipment(m, &[0]);
        s.consume_held(slot);
        s.bz_mob_sound_src(m, "minecraft:entity.allay.item_given", 6, 2.0, 1.0);
        if let Some(b) = m.bm.brain.as_mut() {
            b.mem.liked_player.set(s.uuid);
        }
        return Some(true);
    }
    if m.equipment[0].is_some() && s.slots[slot].is_none() {
        let hand = m.equipment[0].take();
        s.send_mob_equipment(m, &[0]);
        s.bz_mob_sound_src(m, "minecraft:entity.allay.item_taken", 6, 2.0, 1.0);
        s.mob_swing(m);
        if let Some(inv) = m.bm.inventory.take() {
            let pos = m.pos;
            throw_stack(s, m, inv, pos, [0.3, 0.3, 0.3], 0.3);
        }
        if let Some(b) = m.bm.brain.as_mut() {
            b.mem.liked_player.erase();
        }
        if let Some(h) = hand {
            let held = s.held as usize;
            let creative = s.game_mode == 1;
            if let Some(rest) = s.pickup_into_inventory(h, creative, held) {
                s.drop_from_player(rest, false);
            }
        }
        return Some(true);
    }
    None
}

/// Death: inventory and held item dropped.
pub fn drop_inventory(s: &mut Server, m: &mut Mob) {
    if let Some(inv) = m.bm.inventory.take() {
        super::armadillo::spawn_at_location(s, m, inv);
    }
    if let Some(h) = m.equipment[0].take() {
        super::armadillo::spawn_at_location(s, m, h);
    }
}
