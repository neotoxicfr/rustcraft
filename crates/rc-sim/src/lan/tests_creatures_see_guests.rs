//! Mobs deal with every player of the world on the local network: they pick a guest as their
//! target (the nearest, or the highest for a phantom), freeze under a guest's gaze, retaliate
//! against the guest who hit them, and hurt the guest they fight or touch.

use super::tests_g3f05::{FLOOR, G, T, feet, guest_ready, host_ready, join, open, publish, server_tick};
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;

/// A world open to one guest, at night: the host at `host`, the guest at `guest_at`, both ready.
fn host_and_guest(tag: &str, host: [f64; 3], guest_at: [f64; 3]) -> (T, G) {
    let mut t = open(tag);
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    host_ready(&mut t, host);
    guest_ready(&mut t, guest_at);
    match t.s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
        Some(c) => c.1 = 18000,
        None => t.s.clocks.push(("minecraft:overworld".into(), 18000)),
    }
    (t, g)
}

fn guest(t: &T) -> &super::ctx::PlayerCtx {
    t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap()
}

fn guest_mut(t: &mut T) -> &mut super::ctx::PlayerCtx {
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap()
}

fn spawn(t: &mut T, kind: Kind, pos: [f64; 3]) -> i32 {
    let m = t.s.new_mob(kind, pos, 0.0);
    let id = m.id;
    t.s.add_mob(m);
    id
}

fn mob(t: &T, id: i32) -> &Mob {
    t.s.mob_ref(id).expect("mob")
}

/// Yaw and pitch that turn eyes at `eye` toward `to`.
fn gaze(eye: [f64; 3], to: [f64; 3]) -> (f32, f32) {
    let (dx, dy, dz) = (to[0] - eye[0], to[1] - eye[1], to[2] - eye[2]);
    ((dz.atan2(dx).to_degrees() - 90.0) as f32, (-(dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees()) as f32)
}

/// The guest looks at mob `id`'s eyes.
fn guest_looks_at(t: &mut T, id: i32) {
    let to = [mob(t, id).pos()[0], mob(t, id).eye_y(), mob(t, id).pos()[2]];
    let p = guest(t).pos;
    let (yaw, pitch) = gaze([p[0], p[1] + 1.62, p[2]], to);
    let c = guest_mut(t);
    (c.yaw, c.pitch) = (yaw, pitch);
}

/// Ticks the server until `done` holds (at most `limit` ticks); returns the ticks it took.
fn ticks_until(t: &mut T, limit: u32, mut each: impl FnMut(&mut T), mut done: impl FnMut(&T) -> bool) -> Option<u32> {
    for tick in 1..=limit {
        each(t);
        server_tick(t);
        if done(t) {
            return Some(tick);
        }
    }
    None
}

/// A zombie next to the guest, the host far across the floor: it takes the guest (the nearest
/// player) as its target and hurts it; the host is left alone. Fails on the code before (a mob's
/// target could only be the host: the zombie walked to the host and hit it); sabotage verified
/// (failure): the search trying the installed player alone.
#[test]
fn a_zombie_fights_the_nearest_guest() {
    let (mut t, _g) = host_and_guest("zombie-guest", feet([-5, FLOOR + 1, -5]), feet([5, FLOOR + 1, 5]));
    let z = spawn(&mut t, Kind::Zombie, feet([5, FLOOR + 1, 2]));
    let guest_id = guest(&t).player_id;
    let hurt = ticks_until(&mut t, 300, |_| {}, |t| guest(t).vit.health < 20.0);
    assert!(hurt.is_some(), "the zombie never hurt the guest (target {:?})", mob(&t, z).target);
    assert_eq!(mob(&t, z).bound_player(), Some(guest_id), "the zombie deals with the guest");
    assert_eq!(t.s.life.vit.health, 20.0, "the host far away was left alone");
}

/// A phantom above both players picks the highest one, the guest on a pillar, the host on the
/// floor. Fails on the code before (the phantom only knew the host); sabotage verified
/// (failure): the candidates left in seat order instead of by height.
#[test]
fn a_phantom_picks_the_highest_player_a_guest() {
    let (mut t, _g) = host_and_guest("phantom-guest", feet([-3, FLOOR + 1, 0]), feet([3, FLOOR + 6, 0]));
    let ph = spawn(&mut t, Kind::Phantom, feet([0, FLOOR + 20, 0]));
    let guest_id = guest(&t).player_id;
    let found = ticks_until(&mut t, 200, |_| {}, |t| t.s.mob_ref(ph).is_some_and(|m| m.target.is_some()));
    assert!(found.is_some(), "the phantom never picked a target");
    assert_eq!(mob(&t, ph).target, Some(Ent::Player));
    assert_eq!(mob(&t, ph).bound_player(), Some(guest_id), "the phantom picked the guest, the higher player");
}

/// A creaking 6 blocks from the guest, the host behind it looking away: the guest's gaze
/// activates it against the guest and freezes it. Fails on the code before (only the host's gaze
/// counted: the creaking walked on); sabotage verified (failure): the nearby players' list left
/// empty.
#[test]
fn a_guest_gaze_activates_and_freezes_a_creaking() {
    let (mut t, _g) = host_and_guest("creaking-guest", feet([-5, FLOOR + 1, -5]), feet([0, FLOOR + 1, 5]));
    t.s.yaw = 180.0;
    let c = spawn(&mut t, Kind::Creaking, feet([0, FLOOR + 1, -1]));
    let guest_id = guest(&t).player_id;
    let frozen = ticks_until(
        &mut t,
        100,
        |t| guest_looks_at(t, c),
        |t| t.s.mob_ref(c).is_some_and(|m| m.rec.active && !m.rec.can_move),
    );
    assert!(frozen.is_some(), "the creaking never froze under the guest's gaze");
    assert_eq!(mob(&t, c).bound_player(), Some(guest_id), "activated against the guest");
}

/// The guest hits a zombie: the zombie retaliates against the guest, not the host who stands
/// as close. Fails on the code before (the hit recorded "the player", read back as the host when
/// the zombie ticked); sabotage verified (failure): the binding on a hurt removed.
#[test]
fn a_zombie_hit_by_a_guest_retaliates_against_the_guest() {
    let (mut t, g) = host_and_guest("zombie-hit", feet([-2, FLOOR + 1, 0]), feet([2, FLOOR + 1, 0]));
    let z = spawn(&mut t, Kind::Zombie, feet([0, FLOOR + 1, 0]));
    let _ = g;
    t.s.as_guest(0, |sv| {
        sv.mobs.attack_ticker = 100;
        sv.player_attack(z);
    });
    let guest_id = guest(&t).player_id;
    assert_eq!(mob(&t, z).bound_player(), Some(guest_id), "the hit names the guest");
    let hurt = ticks_until(&mut t, 200, |_| {}, |t| guest(t).vit.health < 20.0);
    assert!(hurt.is_some(), "the zombie never hit back at the guest");
    assert_eq!(t.s.life.vit.health, 20.0, "the host was spared");
}

/// Micro-bench (ignored, `--ignored --nocapture`): what ticking a mob with its guest installed
/// adds, one round trip of the installed player (guest in, host back), and a zombie's tick bound
/// to the host then to the guest. Figures only printed (never asserted).
#[test]
#[ignore = "micro-bench"]
fn bench_mob_ticked_with_its_guest() {
    let (mut t, _g) = host_and_guest("bench-bound", feet([-5, FLOOR + 1, -5]), feet([5, FLOOR + 1, 5]));
    const ROUNDS: u32 = 200_000;
    let start = std::time::Instant::now();
    for _ in 0..ROUNDS {
        t.s.switch_installed_player(Some(0));
        t.s.switch_installed_player(None);
    }
    let round_trip = start.elapsed().as_nanos() as f64 / f64::from(ROUNDS);
    println!("[bench] installed player round trip (guest in, host back): {round_trip:.0} ns");
    let z = spawn(&mut t, Kind::Zombie, feet([0, FLOOR + 1, 0]));
    let host_id = t.s.player_id;
    let guest_id = guest(&t).player_id;
    for (who, id) in [("host", host_id), ("guest", guest_id)] {
        const TICKS: u32 = 20_000;
        let i = t.s.mob_index(z).expect("zombie");
        let mut m = t.s.mobs.list.take(i).expect("zombie");
        let start = std::time::Instant::now();
        for _ in 0..TICKS {
            m.bind_to_player(id);
            m.set_pos_axis(0, 0.5);
            t.s.tick_mob(&mut m);
        }
        let per_tick = start.elapsed().as_nanos() as f64 / f64::from(TICKS);
        t.s.mobs.list.put(i, m);
        println!("[bench] zombie tick bound to the {who}: {per_tick:.0} ns");
    }
}

/// A slime touching the guest hurts it, the host far away untouched: the guest's own contact
/// with the cubes (already in its own tick before this mission: passes on the code before).
#[test]
fn a_slime_touching_the_guest_hurts_it() {
    let (mut t, _g) = host_and_guest("slime-guest", feet([-5, FLOOR + 1, -5]), feet([4, FLOOR + 1, 4]));
    let mut slime = t.s.new_mob(Kind::Slime, feet([4, FLOOR + 1, 4]), 0.0);
    slime.set_size(4, true);
    t.s.add_mob(slime);
    let hurt = ticks_until(&mut t, 100, |_| {}, |t| guest(t).vit.health < 20.0);
    assert!(hurt.is_some(), "the slime never hurt the guest it touches");
    assert_eq!(t.s.life.vit.health, 20.0, "the host far away was untouched");
}
