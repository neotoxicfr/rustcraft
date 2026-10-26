//! Game reference values (computed by hand from the tick rules) on a flat ground.

use super::*;
use crate::{FluidTables, SUFF_NEVER};

const AIR: u32 = 0;
const STONE: u32 = 1;
const ICE: u32 = 2;
const SLIME: u32 = 3;
const WEB: u32 = 4;
const SOUL: u32 = 5;

fn tables() -> Tables {
    let full = vec![Aabb { min: [0.0; 3], max: [1.0; 3] }];
    Tables {
        shapes: vec![vec![], full.clone(), full.clone(), full.clone(), vec![], full],
        top: Vec::new(),
        oversized: vec![false; 6],
        friction: vec![0.6, 0.6, 0.98, 0.8, 0.6, 0.6],
        speed_factor: vec![1.0, 1.0, 1.0, 1.0, 1.0, 0.4],
        jump_factor: vec![1.0; 6],
        fluids: FluidTables::from_parts(vec![Fluid::None; 6], vec![0.0; 6]),
        bounce: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        inside: vec![0, 0, 0, inside::SLIME, inside::WEB, 0],
        block: vec![0, 1, 2, 3, 4, 5],
        climbable: vec![false; 6],
        support_keep: vec![0; 6],
        no_bounce: vec![false; 6],
        lily_pad: u16::MAX,
        suff_rule: vec![SUFF_NEVER; 6],
        suffocating: vec![false; 6],
        air: Vec::new(),
        special: crate::Special::default(),
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

fn item(pos: [f64; 3], vel: [f64; 3]) -> Body {
    Body::new(pos, vel, 0.25, 0.25, 0.2125)
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1.0e-12, "{a} != {b}");
}

#[test]
fn item_falls_with_game_constants() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut b = item([0.5, 2.0, 0.5], [0.0; 3]);
    b.tick_item(&w, &t, 1, 3);
    // Gravity 0.04 then air friction 0.98 (widened float).
    close(b.pos[1], 2.0 - 0.04);
    close(b.vel[1], -0.04 * f64::from(0.98f32));
    b.tick_item(&w, &t, 2, 3);
    let v1 = -0.04 * f64::from(0.98f32);
    close(b.pos[1], 2.0 - 0.04 + (v1 - 0.04));
    assert!(!b.on_ground);
}

#[test]
fn item_lands_and_rests() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut b = item([0.5, 1.0, 0.5], [0.0; 3]);
    let mut n = 1;
    while !b.on_ground && n < 100 {
        b.tick_item(&w, &t, n, 0);
        n += 1;
    }
    assert!(b.on_ground);
    close(b.pos[1], 0.0);
    // Vertical collision: vertical velocity cancelled (no bounce on stone).
    assert!(b.vel[1].abs() < 1.0e-12);
    for k in n..n + 40 {
        b.tick_item(&w, &t, k, 0);
    }
    close(b.pos[1], 0.0);
    assert!(b.on_ground);
}

#[test]
fn item_slides_and_stops_with_block_friction() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut stone = item([0.5, 0.0, 0.5], [0.2, 0.0, 0.0]);
    stone.on_ground = true;
    let wi = Flat { t: &t, floor: ICE, extra: vec![] };
    let mut ice = item([0.5, 0.0, 0.5], [0.2, 0.0, 0.0]);
    ice.on_ground = true;
    stone.tick_item(&w, &t, 1, 0);
    ice.tick_item(&wi, &t, 1, 0);
    // Ground friction: 0.98 × block friction, computed in float.
    close(stone.vel[0], 0.2 * f64::from(0.98f32 * 0.6));
    close(ice.vel[0], 0.2 * f64::from(0.98f32 * 0.98));
    for k in 2..200 {
        stone.tick_item(&w, &t, k, 0);
        ice.tick_item(&wi, &t, k, 0);
    }
    assert!(ice.pos[0] > stone.pos[0] + 1.0);
    assert!(stone.vel[0].abs() < 1.0e-6);
}

#[test]
fn item_bounces_on_slime() {
    let t = tables();
    let w = Flat { t: &t, floor: SLIME, extra: vec![] };
    let mut b = item([0.5, 3.0, 0.5], [0.0; 3]);
    let mut up = false;
    for n in 1..60 {
        b.tick_item(&w, &t, n, 0);
        if b.vel[1] > 0.05 {
            up = true;
            break;
        }
    }
    assert!(up, "the item must bounce on the slime");
}

#[test]
fn item_slowed_by_web_and_soul_sand() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 3, 0], WEB)] };
    let mut free = item([0.5, 3.5, 0.5], [0.0, -0.3, 0.0]);
    let wf = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut webbed = free.clone();
    for n in 1..4 {
        free.tick_item(&wf, &t, n, 0);
        webbed.tick_item(&w, &t, n, 0);
    }
    assert!(webbed.pos[1] > free.pos[1] + 0.3);
    let ws = Flat { t: &t, floor: SOUL, extra: vec![] };
    let mut soul = item([0.5, 0.0, 0.5], [0.2, 0.0, 0.0]);
    soul.on_ground = true;
    soul.tick_item(&ws, &t, 1, 0);
    close(soul.vel[0], 0.2 * f64::from(0.4f32) * f64::from(0.98f32 * 0.6));
}

#[test]
fn item_reads_slowing_block_almost_one_block_down() {
    // Real soul sand (14/16 high) placed on ice: the item is INSIDE the sand's
    // cell (factor 0.4 read here), but it reads the 0.999999 friction under its feet, so
    // that of the ice (0.98), not that of the sand (0.6).
    let mut t = tables();
    t.shapes[SOUL as usize] = vec![Aabb { min: [0.0; 3], max: [1.0, 0.875, 1.0] }];
    let w = Flat { t: &t, floor: ICE, extra: vec![([0, -1, 0], SOUL)] };
    let mut b = item([0.5, -0.125, 0.5], [0.2, 0.0, 0.0]);
    b.on_ground = true;
    b.tick_item(&w, &t, 1, 0);
    close(b.vel[0], 0.2 * f64::from(0.4f32) * f64::from(0.98f32 * 0.98));
    // 7-layer snow (cell with factor 1) on soul sand: the speed factor
    // comes from the sand, almost one block lower.
    t.shapes[WEB as usize] = vec![Aabb { min: [0.0; 3], max: [1.0, 0.875, 1.0] }];
    t.inside[WEB as usize] = 0;
    let w = Flat { t: &t, floor: SOUL, extra: vec![([0, -1, 0], WEB)] };
    let mut b = item([0.5, -0.125, 0.5], [0.2, 0.0, 0.0]);
    b.on_ground = true;
    b.tick_item(&w, &t, 1, 0);
    close(b.vel[0], 0.2 * f64::from(0.4f32) * f64::from(0.98f32 * 0.6));
}

#[test]
fn resting_item_moves_one_tick_in_four() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut b = item([0.5, 0.0, 0.5], [0.0; 3]);
    b.on_ground = true;
    // (tick + id) % 4 ≠ 0 and zero horizontal velocity: no movement, gravity accumulates.
    b.tick_item(&w, &t, 1, 0);
    close(b.vel[1], -0.04);
    close(b.pos[1], 0.0);
    b.tick_item(&w, &t, 2, 2);
    close(b.pos[1], 0.0);
    assert!(b.on_ground);
}

#[test]
fn falling_block_falls_without_ground_bounce() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut b = Body::new([0.5, 5.0, 0.5], [0.0; 3], 0.98, 0.98, 0.833);
    b.tick_falling_block(&w, &t);
    close(b.pos[1], 4.96);
    close(b.vel[1], -0.04 * f64::from(0.98f32));
    for _ in 0..60 {
        b.tick_falling_block(&w, &t);
    }
    close(b.pos[1], 0.0);
    assert!(b.on_ground);
}

#[test]
fn tnt_hops_and_fuse_runs_out() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut b = Body::new([0.5, 0.0, 0.5], [0.02, 0.2, 0.0], 0.98, 0.98, 0.15);
    let mut fuse = 80;
    assert!(b.tick_tnt(&w, &t, &mut fuse));
    close(b.pos[1], 0.2 - 0.04);
    assert_eq!(fuse, 79);
    let mut alive = true;
    for _ in 0..79 {
        alive = b.tick_tnt(&w, &t, &mut fuse);
    }
    assert!(!alive);
    assert!(b.on_ground);
    close(b.pos[1], 0.0);
}

#[test]
fn orb_skips_first_tick_then_follows_player() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut b = Body::new([0.5, 0.0, 0.5], [0.0; 3], 0.5, 0.5, 0.425);
    b.on_ground = true;
    let p = [Follow { id: 7, pos: [4.5, 0.0, 0.5], eye: 1.62, spectator: false, dead: false }];
    b.tick_orb(&w, &t, &p, [0.0; 4]);
    assert_eq!(b.pos, [0.5, 0.0, 0.5]);
    for _ in 0..10 {
        b.tick_orb(&w, &t, &p, [0.0; 4]);
    }
    assert!(b.pos[0] > 1.0, "the orb approaches the player: {:?}", b.pos);
    // Spectator or too far: nothing attracts it.
    let mut c = Body::new([0.5, 0.0, 0.5], [0.0; 3], 0.5, 0.5, 0.425);
    let far = [Follow { id: 7, pos: [4.5, 0.0, 0.5], eye: 1.62, spectator: true, dead: false }];
    for _ in 0..10 {
        c.tick_orb(&w, &t, &far, [0.0; 4]);
    }
    close(c.pos[0], 0.5);
}

#[test]
fn ray_clip_hits_block_faces() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![([3, 0, 0], STONE)] };
    // Downwards: top face of the ground.
    let p = clip_blocks(&w, [0.5, 2.0, 0.5], [0.5, -1.0, 0.5]).unwrap();
    close(p[1], 0.0);
    // Horizontal: west face of the block at x = 3.
    let p = clip_blocks(&w, [0.5, 0.5, 0.5], [5.5, 0.5, 0.5]).unwrap();
    close(p[0], 3.0);
    // Free.
    assert!(clip_blocks(&w, [0.5, 0.5, 0.5], [2.5, 0.9, 0.5]).is_none());
}

/// Flat ground plus boxes of solid entities (boats, shulkers).
struct WithEnts<'a> {
    flat: Flat<'a>,
    ents: Vec<Aabb>,
}

impl World for WithEnts<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.flat.state(x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.flat.collision(x, y, z, out)
    }

    fn entity_colliders(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        crate::entity_boxes_in(&self.ents, area, out);
    }
}

#[test]
fn item_lands_on_solid_entity() {
    let t = tables();
    // Boat (box 1.375 × 0.5625) placed on the ground, under the item.
    let boat = Aabb { min: [-0.1875, 0.0, -0.1875], max: [1.1875, 0.5625, 1.1875] };
    let w = WithEnts { flat: Flat { t: &t, floor: STONE, extra: vec![] }, ents: vec![boat] };
    let mut b = item([0.5, 2.0, 0.5], [0.0; 3]);
    for n in 1..60 {
        b.tick_item(&w, &t, n, 0);
    }
    assert!(b.on_ground);
    close(b.pos[1], 0.5625);
    // Without the entity, it falls to the ground.
    let mut c = item([0.5, 2.0, 0.5], [0.0; 3]);
    for n in 1..60 {
        c.tick_item(&w.flat, &t, n, 0);
    }
    close(c.pos[1], 0.0);
}

#[test]
fn orb_touching_solid_entity_is_colliding() {
    let t = tables();
    let boat = Aabb { min: [-0.1875, 0.0, -0.1875], max: [1.1875, 0.5625, 1.1875] };
    let w = WithEnts { flat: Flat { t: &t, floor: STONE, extra: vec![] }, ents: vec![boat] };
    // Placed on the boat (simple contact): the orb is "stuck", without gravity.
    let mut b = Body::new([0.5, 0.5625, 0.5], [0.0; 3], 0.5, 0.5, 0.425);
    for _ in 0..5 {
        b.tick_orb(&w, &t, &[], [0.0; 4]);
    }
    close(b.pos[1], 0.5625);
    assert!(!b.on_ground);
}

#[test]
fn orb_ignores_spectators_but_keeps_dying_target() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let p = |id, x, spectator, dead| Follow { id, pos: [x, 0.0, 0.5], eye: 1.62, spectator, dead };
    // The nearest is a spectator: it is ignored, the orb goes towards the valid player further away.
    let mut a = Body::new([0.5, 0.0, 0.5], [0.0; 3], 0.5, 0.5, 0.425);
    a.on_ground = true;
    let players = [p(1, 2.5, true, false), p(2, 4.5, false, false)];
    for _ in 0..6 {
        a.tick_orb(&w, &t, &players, [0.0; 4]);
    }
    assert!(a.pos[0] > 0.5);
    // A tracked player who dies stays tracked.
    let mut b = Body::new([0.5, 0.0, 0.5], [0.0; 3], 0.5, 0.5, 0.425);
    b.on_ground = true;
    let alive = [p(2, 4.5, false, false)];
    b.tick_orb(&w, &t, &alive, [0.0; 4]);
    b.tick_orb(&w, &t, &alive, [0.0; 4]);
    let x = b.pos[0];
    let dying = [p(2, 4.5, false, true)];
    b.tick_orb(&w, &t, &dying, [0.0; 4]);
    assert!(b.pos[0] > x, "still attracted: {} -> {}", x, b.pos[0]);
}

#[test]
fn bobber_stops_on_ground_and_follows_hook() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    let mut b = Body::new([0.5, 1.0, 0.5], [0.3, 0.2, 0.0], 0.25, 0.25, 0.2125);
    let mut st = Bobber::default();
    let tick = |hooked| BobberTick { hooked, biting: false, seed: 1, rand: 0.5 };
    for _ in 0..40 {
        b.tick_bobber(&w, &t, &mut st, &tick(None));
    }
    // In flight: gravity 0.03 and × 0.92; placed, it stops dead.
    assert!(b.on_ground);
    close(b.pos[1], 0.0);
    assert_eq!(b.vel, [0.0; 3]);
    assert_eq!(st.state, BobberState::Flying);
    // Hooked: zero velocity, then it follows the entity.
    b.tick_bobber(&w, &t, &mut st, &tick(Some([3.0, 1.2, 3.0])));
    assert_eq!(st.state, BobberState::Hooked);
    b.tick_bobber(&w, &t, &mut st, &tick(Some([3.0, 1.2, 3.0])));
    assert_eq!(b.pos, [3.0, 1.2, 3.0]);
    // The hook disappears: in flight again.
    b.tick_bobber(&w, &t, &mut st, &tick(None));
    assert_eq!(st.state, BobberState::Flying);
}

#[test]
fn bobber_bite_uses_synchronized_random() {
    // Seed 0: first draw of the Java randomness = 0.73096776 (known value of `new Random(0)`).
    let mut st = Bobber::default();
    st.reseed(0);
    let v = st.bite_velocity();
    let r = 0.730_967_76f32;
    assert!((v - f64::from(-0.4f32 * (r * (1.0f32 - 0.6f32) + 0.6f32))).abs() < 1.0e-6, "{v}");
}

#[test]
fn server_item_stuck_in_block_moves_out() {
    let t = tables();
    // Item in a stone block at (0, 1, 0), free above.
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 1, 0], STONE)] };
    let mut b = item([0.5, 1.3, 0.5], [0.0; 3]);
    b.tick_item_server(&w, &t, 1, 0, &mut || 0.5);
    assert!(b.no_physics);
    // Nearest free face: the top (y 1.425 at the centre → 0.575 to the top; the sides
    // are free too at 0.5): north side, velocity −(0.5 × 0.2 + 0.1) in z.
    close(b.vel[2], f64::from(-(0.5f32 * 0.2 + 0.1)) * f64::from(0.98f32));
}

/// G5-F14: horizontal piston push on a dropped item. Server side (authoritative instance),
/// the ground is recomputed even without a vertical component: the item lifts off. Client
/// side (simulated without authority), the ground is only recomputed if the entity moves vertically.
#[test]
fn horizontal_piston_push_ungrounds_server_body_only() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![] };
    for (server, grounded) in [(true, false), (false, true)] {
        let mut b = item([0.5, 0.0, 0.5], [0.0; 3]);
        b.server = server;
        b.on_ground = true;
        b.piston_move([0.3, 0.0, 0.0], 7, &w, &t, 0.04);
        close(b.pos[0], 0.8);
        assert_eq!(b.on_ground, grounded, "server {server}");
    }
}

/// G5-F14: push brought back to zero by the tick's 0.51 cap: no movement, but
/// the blocks touched act all the same (cobweb: blocking reset), like the game.
#[test]
fn capped_piston_push_still_applies_block_effects() {
    let t = tables();
    let w = Flat { t: &t, floor: STONE, extra: vec![([0, 0, 0], WEB), ([1, 0, 0], WEB)] };
    let mut b = item([0.5, 0.0, 0.5], [0.0; 3]);
    b.piston_move([0.51, 0.0, 0.0], 3, &w, &t, 0.04);
    close(b.pos[0], 1.01);
    b.stuck = [0.0; 3];
    b.piston_move([0.3, 0.0, 0.0], 3, &w, &t, 0.04);
    close(b.pos[0], 1.01);
    assert_eq!(b.stuck, [0.25, f(0.05), 0.25]);
    // Player: same rule.
    let mut p = crate::Player::new([0.5, 0.0, 0.5], 0.0, 0.0);
    p.piston_move([0.51, 0.0, 0.0], 3, &w, &t);
    p.stuck = [0.0; 3];
    p.piston_move([0.3, 0.0, 0.0], 3, &w, &t);
    close(p.pos[0], 1.01);
    assert!(p.stuck != [0.0; 3], "{:?}", p.stuck);
}
