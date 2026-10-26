//! Game reference values on a synthetic world: walk 4.317 m/s, sprint
//! 5.612 m/s, first jump tick 0.3332, step-up, caution at the edge, flight.

use super::*;

const AIR: u32 = 0;
const STONE: u32 = 1;
const SLAB: u32 = 2;
const ICE: u32 = 3;
const SLIME: u32 = 4;
const WEB: u32 = 5;
/// Climbable without collision.
const LADDER: u32 = 6;
/// Full slowing ground (factor 0.4).
const SOUL: u32 = 7;

fn tables() -> Tables {
    let full = vec![Aabb { min: [0.0; 3], max: [1.0; 3] }];
    let slab = vec![Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] }];
    Tables {
        shapes: vec![vec![], full.clone(), slab, full.clone(), full.clone(), vec![], vec![], full],
        top: Vec::new(),
        oversized: vec![false; 8],
        friction: vec![0.6, 0.6, 0.6, 0.98, 0.8, 0.6, 0.6, 0.6],
        speed_factor: vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.4],
        jump_factor: vec![1.0; 8],
        fluids: FluidTables::from_parts(vec![Fluid::None; 8], vec![0.0; 8]),
        bounce: vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        inside: vec![0, 0, 0, 0, inside::SLIME, inside::WEB, 0, 0],
        block: vec![0, 1, 2, 3, 4, 5, 6, 7],
        climbable: vec![false, false, false, false, false, false, true, false],
        support_keep: vec![0; 8],
        no_bounce: vec![false; 8],
        lily_pad: u16::MAX,
        // Stone suffocates (push out of the blocks).
        suff_rule: vec![SUFF_NEVER, SUFF_TAG, SUFF_NEVER, SUFF_NEVER, SUFF_NEVER, SUFF_NEVER, SUFF_NEVER, SUFF_NEVER],
        suffocating: vec![false, true, false, false, false, false, false, false],
        air: Vec::new(),
        special: Special::default(),
        piston: Default::default(),
    }
}

/// Ground of `floor` under y = 0, plus blocks placed by hand.
struct Flat<'a> {
    t: &'a Tables,
    floor: u32,
    extra: Vec<([i32; 3], u32)>,
}

impl World for Flat<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if let Some((_, s)) = self.extra.iter().find(|(p, _)| *p == [x, y, z]) {
            return Some(*s);
        }
        Some(if y < 0 { self.floor } else { AIR })
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        let s = self.state(x, y, z).unwrap_or(AIR);
        out.extend_from_slice(&self.t.shapes[s as usize]);
        self.t.oversized[s as usize]
    }
}

fn settle(p: &mut Player, w: &Flat, t: &Tables) {
    for _ in 0..5 {
        p.tick(Input::default(), w, t);
    }
    assert!(p.on_ground);
}

fn speed_after(input: Input, ticks: usize, floor: u32) -> f64 {
    let t = tables();
    let w = Flat { t: &t, floor, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    let mut last = p.pos;
    let mut v = 0.0;
    for _ in 0..ticks {
        p.tick(input, &w, &t);
        v = ((p.pos[0] - last[0]).powi(2) + (p.pos[2] - last[2]).powi(2)).sqrt();
        last = p.pos;
    }
    v * 20.0
}

#[test]
fn walking_and_sprinting_terminal_speeds() {
    let walk = Input { forward: true, ..Default::default() };
    let v = speed_after(walk, 200, STONE);
    assert!((v - 4.317).abs() < 0.002, "walk {v} m/s");
    let sprint = Input { forward: true, sprint: true, ..Default::default() };
    let v = speed_after(sprint, 200, STONE);
    assert!((v - 5.612).abs() < 0.002, "sprint {v} m/s");
}

#[test]
fn diagonal_is_as_fast_as_straight() {
    let straight = speed_after(Input { forward: true, ..Default::default() }, 200, STONE);
    let diag = speed_after(Input { forward: true, left: true, ..Default::default() }, 200, STONE);
    // Diagonal: input 1.0 instead of 0.98.
    assert!((diag / straight - 1.0 / 0.98).abs() < 1.0e-3, "{diag} / {straight}");
}

#[test]
fn ice_walks_slightly_slower_but_slides_much_farther() {
    // Same formula: acceleration ×0.216/f³, friction f×0.91; ice (0.98) gives 4.157 m/s.
    let ice = speed_after(Input { forward: true, ..Default::default() }, 400, ICE);
    assert!((ice - 4.157).abs() < 0.002, "ice {ice}");
    let slide = |floor| {
        let t = tables();
        let w = Flat { t: &t, floor, extra: vec![] };
        let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
        settle(&mut p, &w, &t);
        for _ in 0..100 {
            p.tick(Input { forward: true, ..Default::default() }, &w, &t);
        }
        let z0 = p.pos[2];
        for _ in 0..200 {
            p.tick(Input::default(), &w, &t);
        }
        p.pos[2] - z0
    };
    let (on_ice, on_stone) = (slide(ICE), slide(STONE));
    assert!(on_ice > 5.0 * on_stone, "ice slide {on_ice}, stone {on_stone}");
}

#[test]
fn first_jump_tick_and_landing() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    p.tick(Input { jump: true, ..Default::default() }, &w, &t);
    // Jump force in f32 (0.42F), widened to f64: exactly like the game.
    assert_eq!(p.pos[1], f64::from(0.42f32), "y {}", p.pos[1]);
    assert!((p.vel[1] - 0.3332).abs() < 1.0e-6, "vy {}", p.vel[1]);
    // Falls back to the ground; maximum jump ≈ 1.2522.
    let mut top: f64 = 0.0;
    for _ in 0..30 {
        p.tick(Input::default(), &w, &t);
        top = top.max(p.pos[1]);
    }
    assert!(p.on_ground && p.pos[1] == 0.0);
    assert!((top - 1.2522).abs() < 1.0e-3, "top {top}");
}

#[test]
fn steps_up_a_slab_but_not_a_full_block() {
    let t = tables();
    let walk = Input { forward: true, ..Default::default() };
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 1], SLAB)] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    for _ in 0..10 {
        p.tick(walk, &w, &t);
    }
    assert_eq!(p.pos[1], 0.5, "stepped up onto the slab");
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 1], STONE)] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    for _ in 0..10 {
        p.tick(walk, &w, &t);
    }
    assert_eq!(p.pos[1], 0.0);
    assert!(p.horizontal_collision);
    // Half-width in `f32` like the game: 0.30000001192…
    assert!((p.pos[2] - (1.0 - f64::from(0.3f32))).abs() < 1.0e-12, "blocked against the wall: {}", p.pos[2]);
}

#[test]
fn sneaking_stops_at_edge() {
    let t = tables();
    // A single row of ground: z ∈ [0, 1), empty beyond.
    struct Ledge<'a>(&'a Tables);
    impl World for Ledge<'_> {
        fn state(&self, _x: i32, y: i32, z: i32) -> Option<u32> {
            Some(if y == -1 && z == 0 { STONE } else { AIR })
        }
        fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
            out.extend_from_slice(&self.0.shapes[self.state(x, y, z).unwrap() as usize]);
            false
        }
    }
    let w = Ledge(&t);
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    for _ in 0..5 {
        p.tick(Input { shift: true, ..Default::default() }, &w, &t);
    }
    for _ in 0..60 {
        p.tick(Input { forward: true, shift: true, ..Default::default() }, &w, &t);
    }
    assert!(p.on_ground, "still on the ground");
    assert!(p.pos[2] <= 1.3 + 1.0e-9 && p.pos[2] > 1.0, "stops at the edge: {}", p.pos[2]);
}

#[test]
fn double_jump_toggles_flight_and_ground_stops_it() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    p.may_fly = true;
    settle(&mut p, &w, &t);
    let jump = Input { jump: true, ..Default::default() };
    p.tick(jump, &w, &t);
    p.tick(Input::default(), &w, &t);
    let s = p.tick(jump, &w, &t);
    assert_eq!(s.flying, Some(true));
    assert!(p.flying);
    // Go up then stay in the air without gravity.
    for _ in 0..10 {
        p.tick(jump, &w, &t);
    }
    let y = p.pos[1];
    for _ in 0..40 {
        p.tick(Input::default(), &w, &t);
    }
    assert!(p.pos[1] >= y && p.flying, "hovers in flight: {} → {}", y, p.pos[1]);
    // Going down to the ground stops the flight.
    for _ in 0..200 {
        p.tick(Input { shift: true, ..Default::default() }, &w, &t);
    }
    assert!(!p.flying && p.on_ground);
}

#[test]
fn sine_table_matches_game_convention() {
    assert_eq!(sin_t(0.0), 0.0);
    assert_eq!(cos_t(0.0), 1.0);
    assert!((sin_t(std::f32::consts::FRAC_PI_2) - 1.0).abs() < 1.0e-6);
}

#[test]
fn standing_still_far_from_a_cobweb_is_not_slowed() {
    // The zone swept without movement starts from the position at the start of the tick, not from the spawn.
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![([5, 0, 5], WEB)] };
    let mut p = Player::new([5.5, 0.0, 5.5], 0.0, 0.0);
    p.pos = [0.5, 0.0, 0.5];
    settle(&mut p, &w, &t);
    p.tick(Input { forward: true, ..Default::default() }, &w, &t);
    assert!(p.vel[2] > 0.05, "slowed without cobweb: {:?}", p.vel);
}

/// Height reached after the first bounce, dropping the player from `drop` blocks onto slime.
fn slime_rebound(drop: f64, input: Input) -> f64 {
    let t = tables();
    let w = Flat { t: &t, floor: SLIME, extra: vec![] };
    let mut p = Player::new([0.5, drop, 0.5], 0.0, 0.0);
    let mut landed = false;
    let mut top: f64 = 0.0;
    for _ in 0..200 {
        p.tick(input, &w, &t);
        if p.on_ground {
            if landed && top > 0.0 {
                break;
            }
            landed = true;
        } else if landed {
            top = top.max(p.pos[1]);
        }
    }
    top
}

#[test]
fn slime_bounces_back_almost_to_drop_height_unless_sneaking() {
    let h = slime_rebound(5.0, Input::default());
    // Restitution 1 with gravity compensation: only the air friction (×0.98 per
    // tick, going down then up) loses height: 5 → 3.62.
    assert!((h - 3.6228).abs() < 1.0e-3, "bounce {h}");
    let h = slime_rebound(5.0, Input { shift: true, ..Default::default() });
    assert_eq!(h, 0.0, "sneaking: no bounce");
}

#[test]
fn cobweb_nearly_stops_walking_and_falling() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], WEB), ([0, 1, 0], WEB)] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    let z0 = p.pos[2];
    for _ in 0..20 {
        p.tick(Input { forward: true, ..Default::default() }, &w, &t);
    }
    let walked = p.pos[2] - z0;
    // Without cobweb: 20 ticks ≈ 4 blocks; in the cobweb, each movement is ×0.25 and the speed restarts from 0.
    assert!(walked > 0.0 && walked < 0.6, "cobweb {walked}");
}

/// One-block wall in front: auto-jump climbs onto it, otherwise one bumps into it.
#[test]
fn auto_jump_climbs_one_block() {
    let t = tables();
    let extra = (-3..4).flat_map(|x| (3..20).map(move |z| ([x, 0, z], STONE))).collect();
    let w = Flat { t: &t, floor: STONE, extra };
    let fwd = Input { forward: true, ..Default::default() };
    for (auto, climbs) in [(true, true), (false, false)] {
        let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
        p.auto_jump = auto;
        settle(&mut p, &w, &t);
        let mut forced = false;
        for _ in 0..40 {
            forced |= p.tick(fwd, &w, &t).auto_jump;
        }
        assert_eq!(p.pos[1] >= 1.0, climbs, "auto {auto}: y {}", p.pos[1]);
        assert_eq!(forced, auto);
    }
}

/// Adjustable double-tap window: at 0, the double tap no longer sprints.
#[test]
fn sprint_window_option() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let fwd = Input { forward: true, ..Default::default() };
    for (window, sprints) in [(7, true), (0, false)] {
        let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
        p.sprint_window = window;
        settle(&mut p, &w, &t);
        p.tick(fwd, &w, &t);
        p.tick(Input::default(), &w, &t);
        p.tick(fwd, &w, &t);
        assert_eq!(p.sprinting, sprints, "window {window}");
    }
}

/// Supporting block with a tall shape (fence post of 1.5 blocks): standing on it, the feet are
/// in the cell above; the support is the post, found in the row under the zone (one-block
/// margin of the collisions). Without the "overflows" flag, this edge cell is ignored.
#[test]
fn support_under_tall_shape() {
    const FENCE: u32 = 8;
    let post = vec![Aabb { min: [0.375, 0.0, 0.375], max: [0.625, 1.5, 0.625] }];
    let b = Aabb { min: [0.2, 1.5, 0.2], max: [0.8, 3.3, 0.8] };
    let pos = [0.5, 1.5, 0.5];
    for (oversized, want) in [(true, Some([0, 0, 0])), (false, None)] {
        let mut t = tables();
        t.shapes.push(post.clone());
        t.oversized.push(oversized);
        let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], FENCE)] };
        assert_eq!(support_of(&w, &b, pos), want, "overflowing shape: {oversized}");
    }
    // On ordinary ground, nothing changes: the block under the feet.
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let b = Aabb { min: [0.2, 0.0, 0.2], max: [0.8, 1.8, 0.8] };
    assert_eq!(support_of(&w, &b, [0.5, 0.0, 0.5]), Some([0, -1, 0]));
}

/// A collision box touches the underside of `b` (zone of 10⁻⁶ under the feet, like the game's
/// supporting-block search).
fn supported(w: &Flat, b: &Aabb) -> bool {
    let (y0, y1) = (b.min[1] - 1.0e-6, b.min[1]);
    let mut boxes = Vec::new();
    for x in b.min[0].floor() as i32..=b.max[0].floor() as i32 {
        for z in b.min[2].floor() as i32..=b.max[2].floor() as i32 {
            for y in y0.floor() as i32 - 1..=y1.floor() as i32 {
                boxes.clear();
                w.collision(x, y, z, &mut boxes);
                let o = [f64::from(x), f64::from(y), f64::from(z)];
                if boxes.iter().any(|s| {
                    s.min[0] + o[0] < b.max[0]
                        && s.max[0] + o[0] > b.min[0]
                        && s.min[2] + o[2] < b.max[2]
                        && s.max[2] + o[2] > b.min[2]
                        && s.max[1] + o[1] >= y0
                        && s.min[1] + o[1] < y1
                }) {
                    return true;
                }
            }
        }
    }
    false
}

/// "On the ground" only above a collision box (the server sends back the blocks under
/// a player who says they are on the ground without support): random walk, sprint, jump and sneak on a
/// relief of steps, slabs and holes, block edges and corners included.
#[test]
fn on_ground_only_above_a_collision_box() {
    let t = tables();
    let mut seed = 0x9e37_79b9_u64;
    let mut rnd = |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let mut extra = Vec::new();
    for x in -8..8 {
        for z in -8..8 {
            let h = rnd(3) as i32;
            for y in 0..h {
                extra.push(([x, y, z], STONE));
            }
            if rnd(4) == 0 {
                extra.push(([x, h, z], SLAB));
            } else if h == 0 && rnd(6) == 0 {
                extra.push(([x, -1, z], AIR));
            }
        }
    }
    let w = Flat { t: &t, floor: STONE, extra };
    let mut p = Player::new([0.5, 3.0, 0.5], 0.0, 0.0);
    p.auto_jump = true;
    let mut input = Input::default();
    let mut grounded = 0;
    for tick in 0..6000 {
        if tick % 15 == 0 {
            let r = rnd(64);
            input = Input {
                forward: r & 1 != 0,
                left: r & 2 != 0,
                jump: r & 4 != 0 && r & 8 != 0,
                shift: r & 48 == 48,
                sprint: r & 16 != 0,
                ..Default::default()
            };
            p.yaw = rnd(360) as f32;
        }
        let before = p.pos;
        p.tick(input, &w, &t);
        if p.pos[0].abs() > 7.0 || p.pos[2].abs() > 7.0 {
            p.pos = [0.5, 4.0, 0.5];
            p.vel = [0.0; 3];
            continue;
        }
        if p.on_ground {
            grounded += 1;
            // Like the game, the height is resolved before x and z: the support can be under the
            // position before the horizontal step (step that leaves an edge), otherwise under the new one.
            let mut held = p.bounding_box();
            for a in [0, 2] {
                held.min[a] -= p.pos[a] - before[a];
                held.max[a] -= p.pos[a] - before[a];
            }
            assert!(
                supported(&w, &p.bounding_box()) || supported(&w, &held),
                "on the ground without support at tick {tick} at {:?} (before {before:?})",
                p.pos
            );
        }
    }
    assert!(grounded > 1000, "too few ticks on the ground ({grounded})");
}

/// Spectator placed on the ground without flying (flight cut on the ground then sent back by the server): the flight
/// is restored and signalled, it no longer stops on the ground, and the player passes through blocks.
/// Before: flight cut at the first ground contact, endless fall into the End's void.
#[test]
fn spectator_always_flies_and_passes_through_blocks() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    p.may_fly = true;
    p.spectator = true;
    let sig = p.tick(Input::default(), &w, &t);
    assert!(p.flying, "flight imposed on the spectator");
    assert_eq!(sig.flying, Some(true), "the client signals the flight to the server");
    assert!(!p.on_ground);
    // The residual ground speed (−0.0784) damps in flight (×0.6): slight drift, then stop.
    let mut y = 0.0;
    for i in 0..40 {
        let sig = p.tick(Input::default(), &w, &t);
        assert_ne!(sig.flying, Some(false), "never a flight end as a spectator");
        if i == 19 {
            y = p.pos[1];
        }
    }
    assert!(p.flying && y > -0.25 && (p.pos[1] - y).abs() < 1.0e-6, "motionless in flight: y = {}", p.pos[1]);
    // Descent (Shift) through the stone ground.
    for _ in 0..40 {
        p.tick(Input { shift: true, ..Default::default() }, &w, &t);
    }
    assert!(p.pos[1] < -2.0, "the spectator passes through the ground: y = {}", p.pos[1]);
    assert!(!p.vertical_collision && !p.horizontal_collision && !p.on_ground);
    // Outside spectator mode, the flight always stops on contact with the ground.
    let mut c = Player::new([0.5, 1.0, 0.5], 0.0, 0.0);
    (c.may_fly, c.flying) = (true, true);
    let mut stopped = false;
    for _ in 0..40 {
        stopped |= c.tick(Input { shift: true, ..Default::default() }, &w, &t).flying == Some(false);
    }
    assert!(
        stopped && !c.flying && c.on_ground && c.pos[1].abs() < 1.0e-6,
        "stop {stopped}, flight {}, ground {}, y {}",
        c.flying,
        c.on_ground,
        c.pos[1]
    );
}

/// Horizontal speed per tick after `ticks` ticks of forward sprint flight.
fn fly_speed_over(w: &dyn World, t: &Tables, y: f64, ticks: usize) -> (Player, f64) {
    let mut p = Player::new([0.5, y, 0.5], 0.0, 0.0);
    (p.may_fly, p.flying) = (true, true);
    let fwd = Input { forward: true, sprint: true, ..Default::default() };
    let mut last = p.pos;
    let mut v = 0.0;
    for _ in 0..ticks {
        p.tick(fwd, w, t);
        v = ((p.pos[0] - last[0]).powi(2) + (p.pos[2] - last[2]).powi(2)).sqrt();
        last = p.pos;
    }
    (p, v)
}

/// In flight, a ladder caps neither the speed nor the fall, and does not reset the fall to zero.
#[test]
fn flying_ignores_climbables() {
    let t = tables();
    let ladders: Vec<([i32; 3], u32)> = (0..80).flat_map(|z| (4..8).map(move |y| ([0, y, z], LADDER))).collect();
    let w = Flat { t: &t, floor: STONE, extra: ladders };
    let (p, v) = fly_speed_over(&w, &t, 5.0, 20);
    assert!(p.flying && p.pos[2] < 80.0);
    assert!(v > 0.3, "speed in flight in ladders: {v}");
    // Climb in flight: no vy imposed at 0.2 by the ladder.
    let mut q = Player::new([0.5, 5.0, 0.5], 0.0, 0.0);
    (q.may_fly, q.flying) = (true, true);
    q.tick(Input { jump: true, ..Default::default() }, &w, &t);
    assert!((q.vel[1] - f64::from(0.05f32 * 3.0) * 0.6).abs() < 1.0e-6, "vy {}", q.vel[1]);
}

/// In flight just above soul sand, no slowdown from the block.
#[test]
fn flying_ignores_block_speed_factor() {
    let t = tables();
    let soul = Flat { t: &t, floor: SOUL, extra: vec![] };
    let stone = Flat { t: &t, floor: STONE, extra: vec![] };
    let (a, va) = fly_speed_over(&soul, &t, 0.3, 20);
    let (_, vb) = fly_speed_over(&stone, &t, 0.3, 20);
    assert!(a.flying && !a.on_ground);
    assert!((va - vb).abs() < 1.0e-9, "soul sand {va}, stone {vb}");
    // On foot, the factor still applies.
    assert!(speed_after(Input { forward: true, ..Default::default() }, 100, SOUL) < 3.0);
}

#[test]
fn jump_boost_adds_to_jump_power() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    p.effects.set("minecraft:jump_boost", Some(1));
    settle(&mut p, &w, &t);
    p.tick(Input { jump: true, ..Default::default() }, &w, &t);
    assert_eq!(p.pos[1], f64::from(0.42f32 + 0.1 * 2.0), "y {}", p.pos[1]);
    p.effects.set("minecraft:jump_boost", None);
    assert_eq!(p.effects, MoveEffects::default());
}

#[test]
fn levitation_lifts_and_slow_falling_caps_gravity() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 10.0, 0.5], 0.0, 0.0);
    p.effects.set("minecraft:levitation", Some(0));
    for _ in 0..60 {
        p.tick(Input::default(), &w, &t);
    }
    // Fixed point: v = 0.98 × (0.8 v + 0.01).
    assert!(p.pos[1] > 11.0 && (p.vel[1] - 0.0098 / 0.216).abs() < 1.0e-3, "y {} vy {}", p.pos[1], p.vel[1]);
    assert_eq!(p.fall_distance, 0.0);

    let mut q = Player::new([0.5, 400.0, 0.5], 0.0, 0.0);
    q.effects.set("minecraft:slow_falling", Some(0));
    for _ in 0..400 {
        q.tick(Input::default(), &w, &t);
    }
    // Fixed point: v = (v − 0.01) × 0.98 → −0.49; the fall restarts from zero at each tick.
    assert!((q.vel[1] + 0.49).abs() < 1.0e-3, "vy {}", q.vel[1]);
    assert!(q.fall_distance < 1.0, "fall {}", q.fall_distance);
}

#[test]
fn blindness_prevents_sprinting() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    p.effects.set("minecraft:blindness", Some(0));
    let s = p.tick(Input { forward: true, sprint: true, ..Default::default() }, &w, &t);
    assert!(!p.sprinting && s.sprinting.is_none());
    p.effects.set("minecraft:blindness", None);
    p.tick(Input { forward: true, sprint: true, ..Default::default() }, &w, &t);
    assert!(p.sprinting);
}

/// Under a one-block ceiling: crawling posture (0.6 × 0.6, eyes at 0.4), no sprint.
#[test]
fn crawls_under_a_one_block_ceiling() {
    let t = tables();
    let ceiling: Vec<([i32; 3], u32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| ([x, 1, z], STONE))).collect();
    let w = Flat { t: &t, floor: STONE, extra: ceiling };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    assert_eq!(p.pose, Pose::Swimming);
    assert_eq!(p.dimensions(), (f64::from(0.6f32), f64::from(0.6f32)));
    assert!(p.eye_height() == f64::from(0.4f32) && p.crawling() && !p.crouching);
    p.tick(Input { forward: true, sprint: true, ..Default::default() }, &w, &t);
    assert!(!p.sprinting, "no sprint while crawling");
    // Outside the ceiling, standing again.
    let mut q = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    let open = Flat { t: &t, floor: STONE, extra: vec![] };
    q.pose = Pose::Swimming;
    q.tick(Input::default(), &open, &t);
    assert_eq!(q.pose, Pose::Standing);
}

/// A corner in a suffocating block pushes at 0.1 towards the nearest free face.
#[test]
fn pushed_out_of_suffocating_blocks() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], STONE), ([0, 1, 0], STONE)] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    p.move_towards_closest_space(&w, &t, 0.29, 0.71);
    assert_eq!(p.vel, [-0.1, 0.0, 0.0]);
    p.move_towards_closest_space(&w, &t, 0.71, 0.71);
    assert_eq!(p.vel, [0.1, 0.0, 0.0]);
    // West and east blocked: the south, closer than the north.
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], STONE), ([-1, 0, 0], STONE), ([1, 0, 0], STONE)] };
    let mut q = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    q.move_towards_closest_space(&w, &t, 0.29, 0.71);
    assert_eq!(q.vel, [0.0, 0.0, 0.1]);
    // Free corner: nothing.
    let mut r = Player::new([5.5, 0.0, 5.5], 0.0, 0.0);
    r.move_towards_closest_space(&w, &t, 5.29, 5.71);
    assert_eq!(r.vel, [0.0; 3]);
}

// ---------- Wave W4a: player physics (audit differences) ----------

/// Stable scaffolding (shape extracted in an empty context: top slab + 4 posts).
const SCAFFOLD: u32 = 8;
/// Unstable scaffolding (`bottom=true`, distance 7).
const SCAFFOLD_BOTTOM: u32 = 9;
const POWDER: u32 = 10;
/// Ladder facing north, trapdoor open north, trapdoor open south, trapdoor closed north.
const LADDER_N: u32 = 11;
const TRAP_OPEN_N: u32 = 12;
const TRAP_OPEN_S: u32 = 13;
const TRAP_SHUT_N: u32 = 14;
/// Bubble column (rising), water block.
const BUBBLES: u32 = 15;

/// Base tables plus the states of the player's special cases.
fn tables_w4a() -> Tables {
    let mut t = tables();
    let post = |x: f64, z: f64| Aabb { min: [x, 0.0, z], max: [x + 0.125, 1.0, z + 0.125] };
    let stable = vec![
        Aabb { min: [0.0, 0.875, 0.0], max: [1.0; 3] },
        post(0.0, 0.0),
        post(0.875, 0.0),
        post(0.0, 0.875),
        post(0.875, 0.875),
    ];
    let trap = vec![Aabb { min: [0.0, 0.0, 0.8125], max: [1.0, 1.0, 1.0] }];
    // (shape, climbable, effect, special kind)
    let extra: [(Vec<Aabb>, bool, u8, u8); 8] = [
        (stable.clone(), true, 0, SP_SCAFFOLDING | SP_FLAG),
        (stable, true, 0, SP_SCAFFOLDING | SP_FLAG | SP_BOTTOM),
        (vec![], false, inside::POWDER_SNOW, SP_POWDER_SNOW),
        (vec![Aabb { min: [0.0, 0.0, 0.8125], max: [1.0, 1.0, 1.0] }], true, 0, SP_LADDER),
        (trap.clone(), false, 0, SP_TRAPDOOR | SP_FLAG),
        (trap.clone(), false, 0, SP_TRAPDOOR | SP_FLAG | (1 << 6)),
        (trap, false, 0, SP_TRAPDOOR),
        (vec![], false, inside::BUBBLE_COLUMN, 0),
    ];
    t.special.kind = vec![0; t.shapes.len()];
    for (shape, climb, ins, kind) in extra {
        let i = t.shapes.len();
        t.shapes.push(shape);
        t.oversized.push(false);
        t.friction.push(0.6);
        t.speed_factor.push(1.0);
        t.jump_factor.push(1.0);
        t.bounce.push(0.0);
        t.inside.push(ins);
        t.block.push(i as u16);
        t.climbable.push(climb);
        t.support_keep.push(0);
        t.no_bounce.push(false);
        t.suff_rule.push(SUFF_NEVER);
        t.suffocating.push(false);
        t.special.kind.push(kind);
    }
    t.special.glide_through = vec![false; t.shapes.len()];
    t.fluids = FluidTables::from_parts(vec![Fluid::None; t.shapes.len()], vec![0.0; t.shapes.len()]);
    t
}

/// Sprint: the modifier sent back by the server carries the same identifier as ours and
/// replaces it (×1.3, not ×1.69). Before: 0.169.
#[test]
fn server_sprint_modifier_replaces_local_one() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    p.tick(Input { forward: true, sprint: true, ..Default::default() }, &w, &t);
    assert!(p.sprinting);
    let sprint = ("minecraft:sprinting".to_owned(), f64::from(0.3f32), 2);
    p.set_attribute("minecraft:movement_speed", 0.1, vec![sprint]);
    let v = p.attribute("minecraft:movement_speed").unwrap();
    assert_eq!(v, 0.1 * (1.0 + f64::from(0.3f32)), "speed {v}");
    // Sprint stop: no modifier left.
    p.tick(Input::default(), &w, &t);
    assert!(!p.sprinting);
    assert_eq!(p.attribute("minecraft:movement_speed"), Some(0.1));
}

/// Knockback sent by the server: the velocity is replaced as is, the next tick takes over.
#[test]
fn server_motion_sets_velocity() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    p.set_motion([0.4, 0.36, 0.0]);
    assert_eq!(p.vel, [0.4, 0.36, 0.0]);
    p.tick(Input::default(), &w, &t);
    assert!(p.pos[0] > 0.85 && p.pos[1] > 0.3, "pos {:?}", p.pos);
}

/// Box and eyes in `f32` like the game (0.6 → 0.60000002…, 1.8 → 1.79999995…).
#[test]
fn dimensions_follow_float_math() {
    let p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    assert_eq!(p.dimensions(), (f64::from(0.6f32), f64::from(1.8f32)));
    assert_eq!(p.eye_height(), f64::from(1.62f32));
    let b = p.bounding_box();
    assert_eq!(b.max[0] - 0.5, f64::from(0.3f32) + 0.0);
}

/// Scaffolding: one stands on it; Shift goes down; one enters from the side.
#[test]
fn scaffolding_depends_on_the_player() {
    let t = tables_w4a();
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], SCAFFOLD), ([1, 0, 0], SCAFFOLD)] };
    let mut p = Player::new([0.5, 1.0, 0.5], 0.0, 0.0);
    for _ in 0..10 {
        p.tick(Input::default(), &w, &t);
    }
    assert!(p.on_ground && p.pos[1] == 1.0, "standing on it: {:?}", p.pos);
    // Sneaking: the platform disappears, one goes down by climbing (no stop in the scaffolding).
    for _ in 0..10 {
        p.tick(Input { shift: true, ..Default::default() }, &w, &t);
    }
    assert!(p.pos[1] < 0.5, "descent while sneaking: {:?}", p.pos);
    // From the side, at block height: one passes through the posts.
    let mut q = Player::new([-1.5, 0.0, 0.5], -90.0, 0.0);
    for _ in 0..30 {
        q.tick(Input { forward: true, ..Default::default() }, &w, &t);
    }
    assert!(q.pos[0] > 0.5, "entry from the side: {:?}", q.pos);
}

/// Unstable scaffolding in the air: the bottom slab carries whoever is above its base.
#[test]
fn unstable_scaffolding_bottom_slab() {
    let t = tables_w4a();
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 5, 0], SCAFFOLD_BOTTOM)] };
    let mut p = Player::new([0.5, 5.4, 0.5], 0.0, 0.0);
    let mut out = Vec::new();
    p.descending = true;
    p.ctx(&w, &t).collision(0, 5, 0, &mut out);
    assert_eq!(out, vec![SCAFFOLDING_BOTTOM]);
    p.pos[1] = 4.9;
    out.clear();
    p.ctx(&w, &t).collision(0, 5, 0, &mut out);
    assert!(out.is_empty());
}

/// Powder snow: one sinks without boots; one walks on it with leather boots; a fall of
/// more than 2.5 blocks is stopped at 0.9.
#[test]
fn powder_snow_collision_context() {
    let t = tables_w4a();
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], POWDER), ([0, 1, 0], POWDER)] };
    let mut p = Player::new([0.5, 2.0, 0.5], 0.0, 0.0);
    for _ in 0..40 {
        p.tick(Input::default(), &w, &t);
    }
    assert!(p.pos[1] < 1.0, "without boots, one sinks: {:?}", p.pos);
    let mut q = Player::new([0.5, 2.0, 0.5], 0.0, 0.0);
    q.leather_boots = true;
    for _ in 0..10 {
        q.tick(Input::default(), &w, &t);
    }
    assert!(q.on_ground && q.pos[1] == 2.0, "leather boots: {:?}", q.pos);
    // Fall from high: placed at 1 + 0.9.
    let mut r = Player::new([0.5, 8.0, 0.5], 0.0, 0.0);
    for _ in 0..40 {
        r.tick(Input::default(), &w, &t);
        if r.on_ground {
            break;
        }
    }
    assert!(r.on_ground && (r.pos[1] - (1.0 + f64::from(0.9f32))).abs() < 1.0e-9, "cushioned fall: {:?}", r.pos);
}

/// Powder snow with leather boots: against a snow wall, one climbs (0.2 per tick).
#[test]
fn leather_boots_climb_powder_snow() {
    let t = tables_w4a();
    // Powder snow two blocks thick, backed against a stone wall.
    let wall: Vec<([i32; 3], u32)> = (0..4)
        .flat_map(|y| [([1, y, 0], POWDER), ([2, y, 0], POWDER), ([3, y, 0], STONE), ([3, y + 1, 0], STONE)])
        .collect();
    let w = Flat { t: &t, floor: STONE, extra: wall };
    let mut p = Player::new([0.5, 0.0, 0.5], -90.0, 0.0);
    p.leather_boots = true;
    settle(&mut p, &w, &t);
    let mut top = 0.0f64;
    for _ in 0..40 {
        p.tick(Input { forward: true, ..Default::default() }, &w, &t);
        top = top.max(p.pos[1]);
    }
    assert!(top > 1.5, "climb: {top}");
    // Without boots: one enters the snow without climbing.
    let mut q = Player::new([0.5, 0.0, 0.5], -90.0, 0.0);
    settle(&mut q, &w, &t);
    let mut top = 0.0f64;
    for _ in 0..40 {
        q.tick(Input { forward: true, ..Default::default() }, &w, &t);
        top = top.max(q.pos[1]);
    }
    assert!(top < 0.5, "without boots: {top}");
}

/// Ladder: 0.15 bounds in `f32` (descent at −0.15000000596…).
#[test]
fn ladder_bounds_are_float() {
    let t = tables_w4a();
    let col: Vec<([i32; 3], u32)> = (0..20).map(|y| ([0, y, 0], LADDER)).collect();
    let w = Flat { t: &t, floor: STONE, extra: col };
    let mut p = Player::new([0.5, 15.0, 0.5], 0.0, 0.0);
    p.vel[1] = -1.0;
    p.tick(Input::default(), &w, &t);
    let expected = (-f64::from(0.15f32) - 0.08) * f64::from(0.98f32);
    assert_eq!(p.vel[1], expected);
}

/// Trapdoor open above a ladder of the same orientation: climbable.
#[test]
fn open_trapdoor_over_ladder_is_climbable() {
    let t = tables_w4a();
    let at = |top: u32| {
        let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], LADDER_N), ([0, 1, 0], top)] };
        let p = Player::new([0.5, 1.2, 0.5], 0.0, 0.0);
        p.on_climbable(&w, &t)
    };
    assert!(at(TRAP_OPEN_N));
    assert!(!at(TRAP_OPEN_S), "different orientation");
    assert!(!at(TRAP_SHUT_N), "closed trapdoor");
}

/// In a bubble column, only its own speed factor counts: no
/// slowdown from the soul sand below.
#[test]
fn bubble_column_ignores_block_below_speed_factor() {
    let t = tables_w4a();
    let col: Vec<([i32; 3], u32)> = (-10..10).map(|x| ([x, 0, 0], BUBBLES)).collect();
    let w = Flat { t: &t, floor: SOUL, extra: col };
    let mut p = Player::new([0.5, 0.0, 0.5], -90.0, 0.0);
    p.vel = [0.2, 0.0, 0.0];
    p.move_with_collisions(0.0, 0.0, Input::default(), &w, &t);
    assert_eq!(p.vel[0], 0.2, "factor 1 in the column (before: 0.4 from the soul sand)");
}

/// Chunk not loaded: one descends to 0.1 above the bottom of the dimension's world, not
/// below (Nether: bottom at 0).
#[test]
fn unloaded_gravity_uses_dimension_bottom() {
    struct Void;
    impl World for Void {
        fn state(&self, _: i32, _: i32, _: i32) -> Option<u32> {
            None
        }
        fn collision(&self, _: i32, _: i32, _: i32, _: &mut Vec<Aabb>) -> bool {
            false
        }
        fn min_y(&self) -> i32 {
            0
        }
    }
    let t = tables();
    let mut p = Player::new([0.5, -10.0, 0.5], 0.0, 0.0);
    p.tick(Input::default(), &Void, &t);
    assert_eq!(p.vel[1], 0.0, "below the bottom of the world");
    let mut q = Player::new([0.5, 10.0, 0.5], 0.0, 0.0);
    q.tick(Input::default(), &Void, &t);
    assert_eq!(q.vel[1], -0.1 * f64::from(0.98f32));
}

/// Elytra: a new jump press in the air starts the glide (signal, posture 0.6);
/// not without a glider or on the ground.
#[test]
fn elytra_start_and_pose() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 20.0, 0.5], 0.0, 0.0);
    p.tick(Input::default(), &w, &t);
    let s = p.tick(Input { jump: true, ..Default::default() }, &w, &t);
    assert!(!s.start_fall_flying, "without a glider");
    p.glider = true;
    p.tick(Input::default(), &w, &t);
    let s = p.tick(Input { jump: true, ..Default::default() }, &w, &t);
    assert!(s.start_fall_flying && p.fall_flying);
    assert_eq!(p.pose, Pose::FallFlying);
    assert_eq!(p.eye_height(), f64::from(0.4f32));
    // On the ground: never.
    let mut q = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    q.glider = true;
    settle(&mut q, &w, &t);
    let s = q.tick(Input { jump: true, ..Default::default() }, &w, &t);
    assert!(!s.start_fall_flying && !q.fall_flying);
}

/// Gliding: one tick follows the game's formula (cos² lift, conversion of the fall,
/// friction 0.99/0.98/0.99), look horizontal towards the south.
#[test]
fn elytra_glide_tick_matches_formula() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 50.0, 0.5], 0.0, 0.0);
    p.fall_flying = true;
    p.glider = true;
    p.vel = [0.0, -0.5, 1.0];
    p.tick(Input::default(), &w, &t);
    // Look (0, 0, 1); lift 1.
    let mut v = [0.0f64, -0.5, 1.0];
    v[1] += 0.08 * (-1.0 + 0.75);
    let c = v[1] * -0.1;
    v[1] += c;
    v[2] += c;
    let h = 1.0f64;
    v[2] += (h - v[2]) * 0.1;
    let v = [v[0] * f64::from(0.99f32), v[1] * f64::from(0.98f32), v[2] * f64::from(0.99f32)];
    assert!((p.vel[1] - v[1]).abs() < 1.0e-9 && (p.vel[2] - v[2]).abs() < 1.0e-9, "{:?} / {v:?}", p.vel);
}

/// Gliding: the block's speed factor does not apply; sprint cannot start.
#[test]
fn gliding_ignores_block_speed_and_cannot_start_sprint() {
    let t = tables();
    let run = |floor: u32| {
        let w = Flat { t: &t, floor, extra: vec![] };
        let mut p = Player::new([0.5, 0.3, 0.5], 0.0, 0.0);
        p.fall_flying = true;
        p.glider = true;
        p.vel = [0.0, 0.0, 1.0];
        p.tick(Input::default(), &w, &t);
        p.vel[2]
    };
    let (soul, stone) = (run(SOUL), run(STONE));
    assert!((soul - stone).abs() < 1.0e-9, "soul sand {soul}, stone {stone}");
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    for glide in [true, false] {
        let mut p = Player::new([0.5, 20.0, 0.5], 0.0, 0.0);
        p.food_ok = true;
        p.glider = true;
        p.fall_flying = glide;
        let input = Input { forward: true, sprint: true, ..Default::default() };
        p.tick(input, &w, &t);
        assert_eq!(p.sprinting, !glide, "gliding {glide}");
    }
}

/// Rocket hooked while gliding: the speed tends towards 1.5 × the look.
#[test]
fn firework_boost_while_gliding() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 50.0, 0.5], 0.0, 0.0);
    p.fall_flying = true;
    p.vel = [0.0, 0.0, 0.0];
    p.rocket_boost();
    assert_eq!(p.vel, [0.0, 0.0, 0.1 + 1.5 * 0.5]);
    p.fall_flying = false;
    p.vel = [0.0; 3];
    p.rocket_boosts = 1;
    p.tick(Input::default(), &w, &t);
    assert!(p.vel[2] == 0.0, "without gliding, nothing");
}

/// Push: a mob that overlaps the player pushes them aside (entity-to-entity push rule); nothing if it does not
/// push or if climbing.
#[test]
fn neighbours_push_the_player() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mob = |x: f64, pusher| Nearby {
        bb: Aabb { min: [x - 0.3, 0.0, 0.2], max: [x + 0.3, 1.8, 0.8] },
        pos: [x, 0.0, 0.5],
        living: true,
        pusher,
    };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    settle(&mut p, &w, &t);
    p.vel = [0.0; 3];
    p.nearby = vec![mob(0.9, Pusher::Living)];
    p.pushed_by_neighbours(&w, &t);
    let xa = 0.9f64 - 0.5;
    let dd = xa.sqrt();
    let expected = -(xa / dd) * (1.0 / dd).min(1.0) * f64::from(0.05f32);
    assert!((p.vel[0] - expected).abs() < 1.0e-12 && p.vel[2] == 0.0, "{:?} / {expected}", p.vel);
    p.vel = [0.0; 3];
    p.nearby = vec![mob(0.9, Pusher::None)];
    p.pushed_by_neighbours(&w, &t);
    assert_eq!(p.vel, [0.0; 3]);
}

/// Impulse: momentum along the look axis, 20 ticks of spin attack, + 1.2 on the ground;
/// posture 0.6 × 0.6.
#[test]
fn riptide_launch() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut p = Player::new([0.5, 0.0, 0.5], 0.0, -90.0);
    settle(&mut p, &w, &t);
    let y0 = p.pos[1];
    p.vel = [0.0; 3];
    p.riptide(3.0, false, &w, &t);
    assert!((p.pos[1] - y0 - f64::from(1.199_999_9f32)).abs() < 1.0e-9, "rise {}", p.pos[1] - y0);
    assert!((p.vel[1] - 3.0).abs() < 1.0e-6, "momentum {:?}", p.vel);
    assert_eq!(p.spin_attack_ticks, 20);
    p.tick(Input::default(), &w, &t);
    assert_eq!(p.pose, Pose::SpinAttack);
    assert_eq!(p.spin_attack_ticks, 19);
}

/// Honey slide pooled: bit for bit identical to the old rule written inline
/// (copied here as the reference) on a grid of positions, velocities, widths and ground.
#[test]
fn honey_slide_matches_inline_rule() {
    let reference = |pos: [f64; 3], vel: [f64; 3], ground: bool, width: f32, c: [i32; 3]| -> Option<[f64; 3]> {
        let old = |vy: f64| vy / f64::from(0.98f32) + 0.08;
        let new = |vy: f64| (vy - 0.08) * f64::from(0.98f32);
        let reach = 0.4375 + f64::from(width / 2.0);
        let sliding = !ground
            && pos[1] <= f64::from(c[1]) + 0.9375 - 1.0e-7
            && old(vel[1]) < -0.08
            && ((f64::from(c[0]) + 0.5 - pos[0]).abs() + 1.0e-7 > reach
                || (f64::from(c[2]) + 0.5 - pos[2]).abs() + 1.0e-7 > reach);
        if !sliding {
            return None;
        }
        let mut v = vel;
        let o = old(v[1]);
        if o < -0.13 {
            let k = -0.05 / o;
            v[0] *= k;
            v[2] *= k;
        }
        v[1] = new(-0.05);
        Some(v)
    };
    let c = [3, 10, -4];
    let mut slid = 0;
    for &x in &[2.0, 2.3, 2.87, 3.13, 3.5, 3.9, 4.2] {
        for &y in &[10.0, 10.5, 10.93, 10.95, 11.2] {
            for &vy in &[0.1, -0.05, -0.1568, -0.2, -0.5, -3.0] {
                for &width in &[0.25f32, 0.6, 0.9] {
                    for ground in [false, true] {
                        let pos = [x, y, -3.5];
                        let vel = [0.37, vy, -0.21];
                        let got = honey_slide(pos, vel, ground, width, c);
                        let want = reference(pos, vel, ground, width, c);
                        assert_eq!(got.map(|v| v.map(f64::to_bits)), want.map(|v| v.map(f64::to_bits)), "{pos:?} {vel:?} {width} {ground}");
                        slid += usize::from(got.is_some());
                    }
                }
            }
        }
    }
    assert!(slid > 20, "grid too poor: {slid} slides");
}
