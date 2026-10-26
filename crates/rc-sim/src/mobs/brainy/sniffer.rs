//! Sniffer: brain (core, idle, sniffing, digging), synchronized states and sounds, dig
//! position (5 tries, blocks of the `sniffer_diggable_block` tag never dug), seed dropped
//! 120 ticks after digging starts, sniffer egg on breeding.

use super::super::jrand::JRand;
use super::super::mob::Mob;
use super::super::villager::brain::{DimKey, GPos};
use super::brain::*;
use crate::player::Stack;
use crate::server::Server;

pub const IDLING: u8 = 0;
pub const FEELING_HAPPY: u8 = 1;
pub const SCENTING: u8 = 2;
pub const SNIFFING: u8 = 3;
pub const SEARCHING: u8 = 4;
pub const DIGGING: u8 = 5;
pub const RISING: u8 = 6;

pub fn brain(rng: &mut JRand) -> Brain {
    use K::*;
    use St::*;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::Swim { chance: 0.8 },
            Bk::Panic { speed: 2.0, armadillo: false, sniffer: true },
            Bk::MoveSink { lo: 500, hi: 700, cd: 0, path: None, last: None, speed: 0.0, armadillo: false },
            Bk::CountDown(TemptCd),
        ],
    );
    let idle = vec![
        at(0, Act::Idle, Bk::MakeLove { speed: 1.0, close: 2, at: 0, sniffer: true }),
        at(1, Act::Idle, Bk::Tempt { speed: Spd::Fixed(1.25), close: Close::Age(2.5, 3.5), sniffer: true }),
        at(2, Act::Idle, Bk::LookSink { lo: 45, hi: 90 }),
        at(3, Act::Idle, Bk::Happy),
        at(
            4,
            Act::Idle,
            run_one(
                &[],
                0x534e_4946,
                vec![
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 2),
                    (Bk::Scent, 1),
                    (Bk::Sniffing, 1),
                    (Bk::SetLook { range: 6.0 }, 1),
                    (Bk::Stroll { speed: 1.0, from_water: true }, 1),
                    (Bk::DoNothing { lo: 5, hi: 20 }, 2),
                ],
            ),
        ),
    ];
    let sniff = vec![at(0, Act::Sniff, Bk::Search)];
    let dig = vec![at(0, Act::Dig, Bk::Dig), at(0, Act::Dig, Bk::FinishDig)];
    Brain::new(
        rng,
        &[Sn::Living, Sn::HurtBy, Sn::Players, Sn::FoodTempt],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[(Digging, Absent)], &[]), idle),
            (act(Act::Sniff, &[(Panicking, Absent), (SniffTarget, Present), (Walk, Present)], &[]), sniff),
            (act(Act::Dig, &[(Panicking, Absent), (Walk, Absent), (Digging, Present)], &[]), dig),
        ],
    )
}

/// Own sounds, synchronized state.
pub fn transition(s: &mut Server, m: &mut Mob, st: u8) {
    match st {
        FEELING_HAPPY => {
            s.mob_sound(m, "minecraft:entity.sniffer.happy", 1.0, 1.0);
            m.bm.sniffer = st;
        }
        SCENTING => {
            m.bm.sniffer = st;
            let p = if m.is_baby() { 1.3 } else { 1.0 };
            s.mob_sound(m, "minecraft:entity.sniffer.scenting", 1.0, p);
        }
        SNIFFING => {
            s.mob_sound(m, "minecraft:entity.sniffer.sniffing", 1.0, 1.0);
            m.bm.sniffer = st;
        }
        DIGGING => {
            m.bm.sniffer = st;
            m.bm.drop_seed_at = m.tick_count + 120;
            s.mob_event(m, 63);
        }
        RISING => {
            s.mob_sound(m, "minecraft:entity.sniffer.digging_stop", 1.0, 1.0);
            m.bm.sniffer = st;
        }
        _ => m.bm.sniffer = st,
    }
}

/// Dig and target forgotten, back to idle.
pub fn reset_sniffing(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    b.mem.digging.erase();
    b.mem.sniff_target.erase();
    transition(s, m, IDLING);
}

/// The sniffing test.
pub fn can_sniff(m: &Mob, b: &Brain) -> bool {
    !b.mem.tempting.has()
        && !b.mem.panicking.has()
        && !m.in_water
        && !m.is_in_love()
        && m.on_ground
        && m.mount.vehicle.is_none()
}

/// Head block: 2.25 blocks ahead (full gaze), 0.2 above the feet.
fn head_block(m: &Mob) -> [i32; 3] {
    let f = direction(m.pitch, m.yaw);
    [
        (m.pos[0] + f[0] * 2.25).floor() as i32,
        (m.pos[1] + f64::from(0.2f32)).floor() as i32,
        (m.pos[2] + f[2] * 2.25).floor() as i32,
    ]
}

fn explored(s: &Server, b: &Brain, p: [i32; 3]) -> bool {
    let dim = DimKey::of(&s.dim.name);
    b.mem.explored.v.as_ref().is_some_and(|l| l.iter().any(|g| g.dim == dim && g.pos == p))
}

/// Diggable block: tag, never dug in this dimension, reachable (distance 1).
fn diggable(s: &Server, m: &mut Mob, b: &Brain, p: [i32; 3]) -> bool {
    s.btag_id(p, crate::tag_bits::TagId::SnifferDiggableBlock)
        && !explored(s, b, p)
        && super::path_begin(s, m, |s, m| s.create_path(m, p, 1)).is_some_and(|p| p.reached)
}

/// Adult on the ground, out of water, neither tempted nor panicking nor mounted; block below the head.
pub fn can_dig(s: &Server, m: &mut Mob, b: &Brain) -> bool {
    if b.mem.panicking.has()
        || b.mem.tempting.has()
        || m.is_baby()
        || m.in_water
        || !m.on_ground
        || m.mount.vehicle.is_some()
    {
        return false;
    }
    let h = head_block(m);
    diggable(s, m, b, [h[0], h[1] - 1, h[2]])
}

/// Search continuation condition: otherwise idle; the walk target must remain the sniffed target.
pub fn search_continue(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    if !can_sniff(m, b) {
        transition(s, m, IDLING);
        return false;
    }
    let w = b.mem.walk.get().and_then(|w| s.bz_track_block(w.target));
    match (w, b.mem.sniff_target.get()) {
        (Some(w), Some(t)) => w == t,
        _ => false,
    }
}

/// 5 position tries on the ground (10 + 2i, height 3), block below.
pub fn dig_position(s: &mut Server, m: &mut Mob, b: &Brain) -> Option<[i32; 3]> {
    for i in 0..5 {
        let Some(p) = s.land_random_pos(m, 10 + 2 * i, 3) else { continue };
        let bp = super::super::mth::block_of(p);
        let below = [bp[0], bp[1] - 1, bp[2]];
        if diggable(s, m, b, below) {
            return Some(below);
        }
    }
    None
}

/// Successful dig: position under the feet added at the head (20 kept before the addition, so up
/// to 21 in memory, as in the game).
pub fn store_explored(s: &Server, m: &Mob, b: &mut Brain) {
    let on = [m.pos[0].floor() as i32, (m.pos[1] - f64::from(0.2f32)).floor() as i32, m.pos[2].floor() as i32];
    let on = m.support.unwrap_or(on);
    let mut list: Vec<GPos> = b.mem.explored.v.clone().unwrap_or_default().into_iter().take(20).collect();
    list.insert(0, GPos { dim: DimKey::of(&s.dim.name), pos: on });
    b.mem.explored.set(list);
}

/// Before the common tick: seed dropped at the scheduled tick during digging.
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    if m.bm.sniffer == DIGGING && m.bm.drop_seed_at == m.tick_count {
        let h = head_block(m);
        let drops = s.simple_loot("minecraft:gameplay/sniffer_digging", None);
        for d in drops {
            let r = &mut s.mobs.rng;
            let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
            s.spawn_item([f64::from(h[0]), f64::from(h[1]), f64::from(h[2])], v, d, 10);
        }
        s.mob_sound(m, "minecraft:entity.sniffer.drop_seed", 1.0, 1.0);
    }
}

/// Breeding: sniffer egg dropped (no baby).
pub fn spawn_egg(s: &mut Server, m: &mut Mob, partner: i32) {
    let r = &mut s.mobs.rng;
    let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
    super::finalize_breeding(s, m, partner, None);
    let p = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 0.5;
    s.mob_sound(m, "minecraft:block.sniffer_egg.plop", 1.0, p);
    s.spawn_item(m.pos, v, Stack { id: "minecraft:sniffer_egg".into(), count: 1, components: None }, 10);
}

/// Jump: small push forward when the mob is almost motionless.
pub fn after_jump(m: &mut Mob) {
    if m.move_ctl.speed_mod > 0.0 && m.vel[0] * m.vel[0] + m.vel[2] * m.vel[2] < 0.01 {
        let a = m.yaw * super::super::mth::DEG;
        let (sn, cs) = (super::super::mth::sin(a), super::super::mth::cos(a));
        m.vel[0] += -f64::from(sn) * f64::from(0.1f32);
        m.vel[2] += f64::from(cs) * f64::from(0.1f32);
    }
}
