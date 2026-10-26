//! G6-F23: tripwire hooks (sounds and game events, receiver then source), tripwire cut with
//! shears (disarmed before breaking: no pulse), vine growth (neighbour cells tested like the
//! game, top face of the block above for the "up" face). Fresh flat world of G6-F01: the
//! absence of the 26.3 cache or of the client jar makes it FAIL.

use rc_client::Event;

use super::g6f01_tests::{W, Y, stack, world};
use super::*;

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

/// Positions of the sounds `name`, in emission order.
fn sound_at(w: &W, evs: &[Event], name: &str) -> Vec<[f64; 3]> {
    let id = w.p.s.sound_id(name).expect("known sound");
    evs.iter()
        .filter_map(|e| match e {
            Event::Sound(s) if matches!(s.sound, rc_proto_v777::play::SoundRef::Id(i) if i == id) => s.pos,
            _ => None,
        })
        .collect()
}

/// Positions of the game events `event`, in order.
fn events_at(w: &W, event: &str) -> Vec<[f64; 3]> {
    w.p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == event).map(|e| e.1).collect()
}

/// Two hooks facing each other (source to the west, receiver to the east) linked by three tripwires.
const SRC: P = [2, Y, 4];
const RCV: P = [6, Y, 4];
const WIRES: [P; 3] = [[3, Y, 4], [4, Y, 4], [5, Y, 4]];

fn line(w: &mut W) -> Vec<Event> {
    w.set([1, Y, 4], "minecraft:stone");
    w.set([7, Y, 4], "minecraft:stone");
    w.set(SRC, "minecraft:tripwire_hook[facing=east]");
    w.set(RCV, "minecraft:tripwire_hook[facing=west]");
    let _ = w.events();
    w.p.s.mobs.rec.vib.log.clear();
    for p in WIRES {
        w.set(p, "minecraft:tripwire");
    }
    w.events()
}

fn hook(w: &W, p: P, prop: &str) -> bool {
    let s = w.p.s.get(p);
    w.p.s.v().get_bool(s, prop)
}

/// Closed line: an attach sound and a `block_attach` at the receiver then at the source; taut
/// tripwire (powered): power-on click and `block_activate`, same order; release: power-off click.
#[test]
fn hooks_play_sounds_and_events_receiver_first() {
    let mut w = world("g6f23-hook");
    let evs = line(&mut w);
    assert!(hook(&w, SRC, "attached") && hook(&w, RCV, "attached"), "line attached");
    let order = vec![center(RCV), center(SRC)];
    assert_eq!(sound_at(&w, &evs, "minecraft:block.tripwire.attach"), order, "attach sound");
    assert_eq!(events_at(&w, "minecraft:block_attach"), order, "attach event");
    w.p.s.mobs.rec.vib.log.clear();
    let mid = WIRES[1];
    let st = w.p.s.get(mid);
    let pressed = w.p.s.v().set_bool(st, "powered", true);
    w.p.s.tripwire_update_source(mid, pressed);
    let evs = w.events();
    assert!(hook(&w, SRC, "powered") && hook(&w, RCV, "powered"), "line powered");
    assert_eq!(sound_at(&w, &evs, "minecraft:block.tripwire.click_on"), order, "power-on click");
    assert_eq!(events_at(&w, "minecraft:block_activate"), order, "power-on event");
    w.p.s.mobs.rec.vib.log.clear();
    w.p.s.tripwire_update_source(mid, st);
    let evs = w.events();
    assert!(!hook(&w, SRC, "powered"), "line unpowered");
    assert_eq!(sound_at(&w, &evs, "minecraft:block.tripwire.click_off"), order, "power-off click");
    assert_eq!(events_at(&w, "minecraft:block_deactivate"), order, "power-off event");
}

/// Tripwire broken by hand: the line powers on (pulse). With shears: disarmed before breaking,
/// `shear` event, the line detaches without powering on.
#[test]
fn shears_disarm_tripwire_before_breaking() {
    for shears in [false, true] {
        let mut w = world(if shears { "g6f23-shears" } else { "g6f23-hand" });
        let _ = line(&mut w);
        w.p.s.mobs.rec.vib.log.clear();
        w.hold(if shears { stack("minecraft:shears", 1) } else { None });
        assert!(w.p.s.player_destroy_block(WIRES[1]), "tripwire broken");
        let evs = w.events();
        let on = sound_at(&w, &evs, "minecraft:block.tripwire.click_on").len();
        let shear = events_at(&w, "minecraft:shear");
        if shears {
            assert_eq!(on, 0, "no pulse with shears");
            assert!(!hook(&w, SRC, "powered") && !hook(&w, SRC, "attached"), "source detached, unpowered");
            assert_eq!(shear, vec![center(WIRES[1])], "cut event");
            assert_eq!(
                sound_at(&w, &evs, "minecraft:block.tripwire.detach"),
                vec![center(RCV), center(SRC)],
                "detached: receiver then source"
            );
        } else {
            assert_eq!(on, 2, "pulse by hand (receiver and source)");
            assert!(hook(&w, SRC, "powered"), "source powered");
            assert!(shear.is_empty(), "no cut by hand");
        }
    }
}

/// Resets the scene `cells` (air around `p`, then the given blocks) before each draw.
fn reset(w: &mut W, p: P, cells: &[(P, u32)], air: u32) {
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                w.p.s.set_block([p[0] + x, p[1] + y, p[2] + z], air, flag::CLIENTS);
            }
        }
    }
    for &(q, s) in cells {
        w.p.s.set_block(q, s, flag::CLIENTS);
    }
}

/// Default state of `block` with the faces `on` set (without touching the world).
fn state(w: &W, block: &str, on: &[&str]) -> u32 {
    let id = w.p.s.bs.t.block_id(block);
    assert_ne!(id, u16::MAX, "unknown block {block}");
    let v = w.p.s.v();
    on.iter().fold(v.default_state(id), |s, f| v.set_bool(s, f, true))
}

/// Vine attached to the east of a stone: when the draw picks north (or south), it goes around
/// the stone (north-east cell, south face; or south-east, north face) because the game tests the
/// stone itself. Old code: it tested the cell south (or north) of the stone, which was empty.
#[test]
fn vine_wraps_around_the_corner_of_its_support() {
    let mut w = world("g6f23-corner");
    let p = [8, Y + 1, 8];
    let e = rel(p, Dir::East);
    let air = w.p.s.air();
    let stone = state(&w, "minecraft:stone", &[]);
    let vine = state(&w, "minecraft:vine", &["east"]);
    let mut wrapped = None;
    for _ in 0..2000 {
        reset(&mut w, p, &[(e, stone), (p, vine)], air);
        w.p.s.vine_random_tick(vine, p);
        for (d, face) in [(Dir::North, "south"), (Dir::South, "north")] {
            let q = rel(e, d);
            let s = w.p.s.get(q);
            if w.p.s.v().kind(s) == Kind::Vine {
                assert!(w.p.s.v().get_bool(s, face), "face turned towards the stone");
                wrapped = Some(q);
            }
        }
        if wrapped.is_some() {
            break;
        }
    }
    assert!(wrapped.is_some(), "the vine never went around its stone");
}

/// Vine attached to the stone above it ("up"): 5% of horizontal draws place an "up"
/// vine in the targeted cell if the block ABOVE that cell accepts it. Old code:
/// it tested two cells higher.
#[test]
fn vine_spreads_under_a_ceiling() {
    let mut w = world("g6f23-ceiling");
    let p = [8, Y + 1, 8];
    let n = rel(p, Dir::North);
    w.set(above(p), "minecraft:stone");
    w.set(above(n), "minecraft:stone");
    let vine = state(&w, "minecraft:vine", &["up"]);
    let mut grew = false;
    for _ in 0..20_000 {
        w.p.s.vine_random_tick(vine, p);
        let s = w.p.s.get(n);
        if w.p.s.v().kind(s) == Kind::Vine {
            assert!(w.p.s.v().get_bool(s, "up"), "vine placed on the ceiling");
            grew = true;
            break;
        }
    }
    assert!(grew, "the vine never spread under the ceiling");
}

/// Upward growth: the face is kept when the neighbouring block of the cell above
/// accepts it. Old code: it tested two cells further, the face always fell.
#[test]
fn vine_grows_upward_along_its_wall() {
    let mut w = world("g6f23-up");
    let p = [8, Y + 1, 8];
    let ap = above(p);
    let air = w.p.s.air();
    let stone = state(&w, "minecraft:stone", &[]);
    let vine = state(&w, "minecraft:vine", &["east"]);
    let mut grew = false;
    for _ in 0..2000 {
        reset(&mut w, p, &[(rel(p, Dir::East), stone), (rel(ap, Dir::East), stone), (p, vine)], air);
        w.p.s.vine_random_tick(vine, p);
        let s = w.p.s.get(ap);
        if w.p.s.v().kind(s) == Kind::Vine {
            assert!(w.p.s.v().get_bool(s, "east"), "east face kept");
            grew = true;
            break;
        }
    }
    assert!(grew, "the vine never grew upwards");
}

/// "Up" face: the game requires the TOP face of the block above (quirk reproduced); a
/// bottom slab above makes the vine fall, a top slab keeps it.
#[test]
fn vine_up_face_reads_the_top_of_the_block_above() {
    let mut w = world("g6f23-slab");
    let p = [8, Y + 1, 8];
    for (slab, kept) in [("bottom", false), ("top", true)] {
        w.set(above(p), &format!("minecraft:smooth_stone_slab[type={slab}]"));
        let vine = state(&w, "minecraft:vine", &["up"]);
        let got = w.p.s.vine_update_shape(vine, p, Dir::Up);
        assert_eq!(got == vine, kept, "slab {slab}");
        assert_eq!(w.p.s.v().kind(got) == Kind::Vine, kept, "slab {slab}: vine kept?");
    }
}
