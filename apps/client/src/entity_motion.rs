//! Motion of small entities simulated on the client between two server resyncs
//! (`docs/specs/entities-objects.md`, "client-side motion"): dropped items, experience
//! orbs, falling blocks, primed TNT. The game moves them itself on every tick
//! with the same physics as the server (`rc_physics::body`); the server only sends their
//! position every 10 to 20 ticks and their velocity when it changes. Without this, a dropped
//! item would stay frozen at the spawn point then jump to the arrival point.

use rc_physics::Aabb;
use rc_physics::body::{Body, Follow, Heft};

use crate::entities::Entities;

/// Kinds simulated here.
fn simulated(kind: &str) -> bool {
    matches!(kind, "item" | "experience_orb" | "falling_block" | "tnt")
}

/// Gravity and slowing block of a simulated kind moved by something else (piston, shulker).
pub(crate) fn heft(kind: &str) -> Heft {
    match kind {
        "item" => Heft::ITEM,
        "experience_orb" => Heft::ORB,
        _ => Heft::BLOCK,
    }
}

/// Opening of the shulkers (their collision box depends on it), and for each one that opens
/// further this tick, the zone its opening sweeps and the movement of what is found there.
pub(crate) fn shulker_peeks(ents: &mut Entities) -> Vec<(Aabb, [f64; 3])> {
    let mut out = Vec::new();
    for e in ents.map.values_mut().filter(|e| e.kind == "shulker") {
        let target = f32::from(e.meta_byte(17)) * 0.01;
        let old = e.peek;
        if e.peek > target {
            e.peek = (e.peek - 0.05).clamp(target, 1.0);
        } else if e.peek < target {
            e.peek = (e.peek + 0.05).clamp(0.0, target);
        } else {
            continue;
        }
        let (from, to) = (crate::entities::shulker_physical(old), crate::entities::shulker_physical(e.peek));
        let push = (to - from) * e.attribute_scale();
        if push > 0.0 {
            let d = e.shulker_step().map(|s| f64::from(s as f32 * push));
            out.push((e.shulker_zone(from, to), d));
        }
    }
    out
}

/// Pushes of the opening shulkers (`zones` from [`shulker_peeks`]) applied to the local
/// player (`me`, if they can be pushed) and to the bodies simulated here, with collisions.
pub(crate) fn shulker_push(
    ents: &mut Entities,
    zones: &[(Aabb, [f64; 3])],
    me: Option<(&mut rc_physics::Player, &dyn rc_physics::World)>,
    w: &dyn rc_physics::World,
    t: &rc_physics::Tables,
) {
    if let Some((p, pw)) = me {
        for (area, d) in zones {
            if p.bounding_box().intersects(area) {
                p.shove(*d, pw, t);
            }
        }
    }
    for e in ents.map.values_mut().filter(|e| simulated(&e.kind) && e.vehicle.is_none()) {
        let k = heft(&e.kind);
        let Some(b) = e.motion.as_deref_mut() else { continue };
        (b.pos, b.vel, b.on_ground) = (e.pos, e.velocity, e.on_ground);
        for (area, d) in zones {
            if b.bounding_box().intersects(area) {
                b.shove(*d, w, t, k);
            }
        }
        (e.pos, e.velocity, e.on_ground) = (b.pos, b.vel, b.on_ground);
    }
}

/// Pseudo-random draw in [0, 1) (orbs in lava: the exact value is specific to
/// each client in the game too).
fn rand01(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 40) as f32 / (1u64 << 24) as f32
}

/// One client tick. `me`: local player (id, feet, eye height, alive and not
/// spectator); `fast_lava`: fast lava in this dimension; `ps`: TNT smoke.
pub(crate) fn tick(
    ents: &mut Entities,
    w: &dyn rc_physics::World,
    t: &rc_physics::Tables,
    me: Follow,
    fast_lava: bool,
    spectator: &dyn Fn(u128) -> bool,
    ps: &mut crate::particles::Particles,
) {
    static SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let mut seed = SEED.load(std::sync::atomic::Ordering::Relaxed);
    // Players that orbs can follow (the nearest-player search ignores spectators,
    // a dying follow stays followed).
    let mut players = vec![me];
    if ents.map.values().any(|e| e.kind == "experience_orb") {
        for (id, e) in &ents.map {
            if e.kind == "player" {
                let eye = f64::from(e.dims(&ents.dims)[1]);
                let (spectator, dead) = (spectator(e.uuid), e.death_time > 0);
                players.push(Follow { id: *id, pos: e.pos, eye, spectator, dead });
            }
        }
    }
    let mut gone = Vec::new();
    for (&id, e) in ents.map.iter_mut() {
        if !simulated(&e.kind) || e.vehicle.is_some() {
            continue;
        }
        let (no_gravity, fuse0) = (e.meta_bool(5), e.meta_int(8).unwrap_or(80));
        let body = e.motion.get_or_insert_with(|| {
            let d = usize::try_from(e.kind_id).ok().and_then(|i| ents.dims.get(i)).copied();
            let (wd, h, eye) = match e.kind.as_str() {
                "item" => (0.25, 0.25, 0.2125),
                "experience_orb" => (0.5, 0.5, 0.425),
                "tnt" => (0.98, 0.98, 0.15),
                _ => (0.98, 0.98, 0.833),
            };
            let (wd, h, eye) = d.map_or((wd, h, eye), |d| (f64::from(d[0]), f64::from(d[1]), f64::from(d[2])));
            Box::new(Body::new(e.pos, e.velocity, wd, h, eye))
        });
        // State received from the server (position resyncs, velocity, ground) taken back before the tick.
        body.pos = e.pos;
        body.vel = e.velocity;
        body.on_ground = e.on_ground;
        body.no_gravity = no_gravity;
        body.fast_lava = fast_lava;
        let age = i32::try_from(e.age).unwrap_or(i32::MAX);
        match e.kind.as_str() {
            "item" => body.tick_item(w, t, age, id),
            "falling_block" => body.tick_falling_block(w, t),
            "tnt" => {
                let mut fuse = fuse0;
                let alive = body.tick_tnt(w, t, &mut fuse);
                e.meta.insert(8, rc_proto_v777::entity::Meta::Int(fuse));
                if alive {
                    // Burning fuse: one smoke per tick above the center.
                    let at = [body.pos[0], body.pos[1] + 0.5, body.pos[2]];
                    ps.spawn(crate::particles::Kind::Smoke, &crate::particles::Options::default(), at, [0.0; 3]);
                } else {
                    gone.push(id);
                }
            }
            _ => {
                let r = [rand01(&mut seed), rand01(&mut seed), rand01(&mut seed), rand01(&mut seed)];
                body.tick_orb(w, t, &players, r);
            }
        }
        let (pos, vel, ground, moved) = (body.pos, body.vel, body.on_ground, body.moved);
        let (r, h) = (body.width / 2.0, body.height);
        if e.kind == "experience_orb" && moved != [0.0; 3] {
            // Interpolated orb: the target follows the simulated movement (if the room is free).
            let free = |p: [f64; 3]| {
                let b = rc_physics::Aabb { min: [p[0] - r, p[1], p[2] - r], max: [p[0] + r, p[1] + h, p[2] + r] };
                rc_physics::is_free(w, &b)
            };
            e.predict(moved, free);
        }
        e.pos = pos;
        e.velocity = vel;
        e.on_ground = ground;
    }
    SEED.store(seed, std::sync::atomic::Ordering::Relaxed);
    for id in gone {
        ents.map.remove(&id);
    }
}

/// Boxes of the solid entities known to the client (boats and rafts, living shulkers depending on
/// their opening, adult living happy ghasts that are immobilized, or under a player `viewer_y` who
/// is above their box), except entity `exclude` (the moving vehicle).
pub(crate) fn colliders(ents: &Entities, exclude: Option<i32>, viewer_y: Option<f64>) -> Vec<rc_physics::Aabb> {
    let mut out = Vec::new();
    for (&id, e) in &ents.map {
        if Some(id) == exclude {
            continue;
        }
        let k = e.kind.as_str();
        let boat = k.ends_with("boat") || k.ends_with("raft");
        if !(boat || k == "shulker" || k == "happy_ghast") {
            continue;
        }
        if k == "shulker" {
            if e.death_time > 0 {
                continue;
            }
            // Scaled cube pushed toward the face opposite the attachment (`Entity::shulker_box`).
            out.push(e.shulker_box());
            continue;
        }
        // Current box (type size, baby and scale attribute included).
        let b = e.bounding_box(&ents.dims, &|_| None, false).1;
        if k == "happy_ghast" {
            let ok = e.death_time == 0 && !e.baby() && (viewer_y.is_some_and(|y| y >= b.max[1]) || e.meta_bool(18));
            if !ok {
                continue;
            }
        }
        out.push(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::{AddEntity, Meta};

    /// Empty world (air everywhere).
    struct Air;

    impl rc_physics::World for Air {
        fn state(&self, _: i32, _: i32, _: i32) -> Option<u32> {
            Some(0)
        }
        fn collision(&self, _: i32, _: i32, _: i32, _: &mut Vec<Aabb>) -> bool {
            false
        }
    }

    /// A shulker of scale 2 on the floor opening completely (data 17 = 100) sweeps 2 blocks
    /// upward: the local player standing on it and an item lying on it rise by 2, a player
    /// beside it stays. Before: the client only moved the opening.
    #[test]
    fn opening_shulker_lifts_the_local_player_and_bodies() {
        let d = rc_data::load_or_prepare("26.3", |_| {}).expect("26.3 data missing");
        let t = rc_physics::Tables::new(&d);
        let mut ents = Entities {
            type_names: vec!["minecraft:shulker".into(), "minecraft:item".into()],
            ..Default::default()
        };
        for (id, kind, pos) in [(1, 0, [0.5, 64.0, 0.5]), (2, 1, [0.5, 66.0, 0.5])] {
            let add = AddEntity { id, uuid: id as u128, kind, pos, velocity: [0.0; 3], pitch: 0.0, yaw: 0.0, head_yaw: 0.0, data: 0 };
            ents.apply(EntityEvent::Add(Box::new(add)));
        }
        let sh = ents.map.get_mut(&1).expect("shulker");
        sh.meta.insert(17, Meta::Byte(100));
        sh.scale = 2.0;
        let item = ents.map.get_mut(&2).expect("item");
        let mut b = Body::new(item.pos, [0.0; 3], 0.25, 0.25, 0.2125);
        b.no_gravity = true;
        item.motion = Some(Box::new(b));
        let mut on = rc_physics::Player::new([0.5, 66.0, 0.5], 0.0, 0.0);
        let aside = rc_physics::Player::new([3.5, 66.0, 0.5], 0.0, 0.0);
        for _ in 0..25 {
            let zones = shulker_peeks(&mut ents);
            assert!(!zones.iter().any(|(a, _)| aside.bounding_box().intersects(a)), "player beside the shulker swept");
            shulker_push(&mut ents, &zones, Some((&mut on, &Air)), &Air, &t);
        }
        assert!((ents.map[&1].peek - 1.0).abs() < 1e-6, "shulker not open");
        assert!((on.pos[1] - 68.0).abs() < 1e-4, "player on the shulker at {}", on.pos[1]);
        assert!((ents.map[&2].pos[1] - 68.0).abs() < 1e-4, "item at {}", ents.map[&2].pos[1]);
    }

    /// Gravity and slowing block of the kinds moved by a piston or a shulker: TNT and falling
    /// blocks read the slowing block half a block down, like their own tick.
    #[test]
    fn heft_per_kind() {
        assert_eq!(heft("item"), Heft::ITEM);
        assert_eq!(heft("experience_orb"), Heft::ORB);
        assert_eq!(heft("tnt").slow_h, 0.500_001);
        assert_eq!(heft("falling_block"), Heft::BLOCK);
    }
}
