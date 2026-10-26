//! Projectile impacts on a new flat world: the shooter's last victim (arrow, wind
//! charge, not the trident), enderman that dodges without sending the projectile back, thrown
//! potions on the enderman, axolotl rehydration, rocket against a honey block, shulker bullet
//! that sends back a projectile deflectable according to its momentum.

use std::path::PathBuf;
use std::sync::Arc;

use super::physics::{BlockHit, EntityHit, Hit, Target};
use super::{PKind, Proj};
use crate::blocks::probe::Probe;
use crate::mobs::end::bullet::BulletState;
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::player::Stack;

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-projimpact-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    // Chunks generated over ±48: the enderman's teleports (±32) find the ground.
    let want: Vec<(i32, i32)> = (-3..=3).flat_map(|x| (-3..=3).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "flat world generation too long");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    (p, dir)
}

fn finish(p: Probe, dir: PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn stack(id: &str) -> Stack {
    Stack { id: id.into(), count: 1, components: None }
}

fn potion(id: &str, name: &str) -> Stack {
    let mut s = stack(id);
    crate::items::set_component(
        &mut s,
        "minecraft:potion_contents",
        rc_proto::nbt::Nbt::Compound(vec![("potion".into(), rc_proto::nbt::Nbt::String(name.into()))]),
    );
    s
}

fn mob(p: &mut Probe, k: Kind, pos: [f64; 3], health: f32) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    m.persistent = true;
    m.health = health;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn living(id: i32, pos: [f64; 3]) -> EntityHit {
    EntityHit { ent: Target::Living(Ent::Mob(id)), loc: pos }
}

fn ground(pos: [f64; 3]) -> Hit {
    let b = [pos[0].floor() as i32, -61, pos[2].floor() as i32];
    Hit::Block(BlockHit { loc: [pos[0], -60.0, pos[2]], pos: b, face: 1, inside: false, border: false })
}

/// Arrow and wind charge of the player: the target becomes their last victim, dated with their
/// tick counter (the tamed wolf then attacks this target); a non-living target
/// clears the memory; the trident never touches it.
#[test]
fn player_projectiles_record_last_victim() {
    let (mut p, dir) = open("victim");
    let z1 = mob(&mut p, Kind::Zombie, [3.5, -60.0, 0.5], 20.0);
    let z2 = mob(&mut p, Kind::Zombie, [6.5, -60.0, 0.5], 20.0);
    let z3 = mob(&mut p, Kind::Zombie, [9.5, -60.0, 0.5], 20.0);

    let mut arrow = Proj::new(PKind::Arrow, 900_101, [2.5, -59.0, 0.5], stack("minecraft:arrow"));
    arrow.owner = Some(p.s.uuid);
    arrow.damage = 2.0;
    arrow.vel = [1.0, 0.0, 0.0];
    p.s.mobs.player_ticks = 77;
    let pos = p.s.mob_ref(z1).expect("zombie").pos;
    p.s.proj_hit_entity(&mut arrow, living(z1, pos));
    assert_eq!(p.s.mobs.player_hit, Some((Ent::Mob(z1), 77)), "player's arrow");

    let mut wind = Proj::new(PKind::WindCharge, 900_102, [5.5, -59.0, 0.5], stack("minecraft:wind_charge"));
    wind.owner = Some(p.s.uuid);
    wind.vel = [1.0, 0.0, 0.0];
    p.s.mobs.player_ticks = 90;
    let pos = p.s.mob_ref(z2).expect("zombie").pos;
    p.s.proj_hit(&mut wind, Hit::Entity(living(z2, pos)));
    assert_eq!(p.s.mobs.player_hit, Some((Ent::Mob(z2), 90)), "player's wind charge");

    // Minecart: non-living target, memory cleared.
    let cart = crate::minecart::Cart::new(crate::minecart::CKind::Rideable, 900_103, [12.5, -60.0, 0.5]);
    p.s.cart_add(cart);
    let mut arrow2 = Proj::new(PKind::Arrow, 900_104, [11.5, -59.5, 0.5], stack("minecraft:arrow"));
    arrow2.owner = Some(p.s.uuid);
    arrow2.damage = 2.0;
    arrow2.vel = [1.0, 0.0, 0.0];
    p.s.proj_hit_entity(&mut arrow2, EntityHit { ent: Target::Cart(900_103), loc: [12.5, -59.5, 0.5] });
    assert_eq!(p.s.mobs.player_hit, None, "arrow on a minecart");

    let mut trident = Proj::new(PKind::Trident, 900_105, [8.5, -59.0, 0.5], stack("minecraft:trident"));
    trident.owner = Some(p.s.uuid);
    trident.vel = [1.0, 0.0, 0.0];
    let pos = p.s.mob_ref(z3).expect("zombie").pos;
    p.s.proj_hit_entity(&mut trident, living(z3, pos));
    assert!(p.s.mob_ref(z3).expect("zombie").health < 20.0, "the trident hurt");
    assert_eq!(p.s.mobs.player_hit, None, "the trident does not set the last victim");
    finish(p, dir);
}

/// Flaming arrow and trident on an enderman (who dodges): no bounce, the projectile
/// keeps its speed and the enderman keeps the fire set by the arrow.
#[test]
fn projectiles_pass_dodging_enderman() {
    let (mut p, dir) = open("ender");
    let e = mob(&mut p, Kind::Enderman, [3.5, -60.0, 0.5], 40.0);
    let mut arrow = Proj::new(PKind::Arrow, 900_201, [2.5, -58.5, 0.5], stack("minecraft:arrow"));
    arrow.owner = Some(p.s.uuid);
    arrow.damage = 2.0;
    arrow.vel = [2.0, 0.0, 0.0];
    arrow.fire = 100;
    let pos = p.s.mob_ref(e).expect("enderman").pos;
    p.s.proj_hit_entity(&mut arrow, living(e, pos));
    let m = p.s.mob_ref(e).expect("enderman");
    assert_eq!(m.health, 40.0, "enderman unharmed");
    assert_eq!(m.fire, 100, "the arrow's fire stays");
    assert_eq!(arrow.vel, [2.0, 0.0, 0.0], "arrow neither sent back nor slowed");
    assert!(!arrow.removed, "the arrow continues");

    let e2 = mob(&mut p, Kind::Enderman, [-3.5, -60.0, 0.5], 40.0);
    let mut trident = Proj::new(PKind::Trident, 900_202, [-2.5, -58.5, 0.5], stack("minecraft:trident"));
    trident.owner = Some(p.s.uuid);
    trident.vel = [-2.0, 0.0, 0.0];
    let yaw = trident.yaw;
    let pos = p.s.mob_ref(e2).expect("enderman").pos;
    p.s.proj_hit_entity(&mut trident, living(e2, pos));
    assert_eq!(p.s.mob_ref(e2).expect("enderman").health, 40.0, "enderman unharmed");
    assert_eq!(trident.vel, [-2.0, 0.0, 0.0], "trident neither sent back nor slowed");
    assert_eq!(trident.yaw, yaw, "no U-turn");
    finish(p, dir);
}

/// Damage potion thrown on an enderman: no damage, teleport; water potion: 1 damage
/// then teleport.
#[test]
fn thrown_potions_on_enderman() {
    let (mut p, dir) = open("enpot");
    let start = [0.5, -60.0, 10.5];
    let e = mob(&mut p, Kind::Enderman, start, 40.0);
    let mut harm = Proj::new(PKind::SplashPotion, 900_301, [0.5, -59.5, 10.5], potion("minecraft:splash_potion", "minecraft:harming"));
    p.s.proj_hit(&mut harm, ground([0.5, -59.5, 10.5]));
    let m = p.s.mob_ref(e).expect("enderman");
    assert_eq!(m.health, 40.0, "the damage potion does not hurt the enderman");
    assert_ne!(m.pos, start, "the enderman teleported");

    let start2 = [0.5, -60.0, -10.5];
    let e2 = mob(&mut p, Kind::Enderman, start2, 40.0);
    let mut water = Proj::new(PKind::SplashPotion, 900_302, [0.5, -59.5, -10.5], potion("minecraft:splash_potion", "minecraft:water"));
    p.s.proj_hit(&mut water, ground([0.5, -59.5, -10.5]));
    let m = p.s.mob_ref(e2).expect("enderman");
    assert_eq!(m.health, 39.0, "water hurts the enderman");
    assert_ne!(m.pos, start2, "then it teleports");
    finish(p, dir);
}

/// Water potion: every axolotl whose box touches the area (even beyond 4 blocks) gains
/// 1,800 air, without exceeding 6,000.
#[test]
fn water_potion_rehydrates_axolotls() {
    let (mut p, dir) = open("axo");
    let a1 = mob(&mut p, Kind::Axolotl, [10.5, -60.0, 14.8], 14.0);
    let a2 = mob(&mut p, Kind::Axolotl, [11.5, -60.0, 10.5], 14.0);
    p.s.mobs.get_mut(a1).expect("axolotl").air = 100;
    p.s.mobs.get_mut(a2).expect("axolotl").air = 5000;
    let mut water = Proj::new(PKind::SplashPotion, 900_401, [10.5, -60.0, 10.5], potion("minecraft:splash_potion", "minecraft:water"));
    p.s.proj_hit(&mut water, ground([10.5, -60.0, 10.5]));
    assert_eq!(p.s.mob_ref(a1).expect("axolotl").air, 1900, "at 4.3 blocks: rehydrated");
    assert_eq!(p.s.mob_ref(a2).expect("axolotl").air, 6000, "cap of 6,000");
    finish(p, dir);
}

/// Rocket that dives against the face of a honey block: it slides (vertical speed set,
/// horizontal slowed) before the rest of its flight.
#[test]
fn firework_slides_on_honey() {
    let (mut p, dir) = open("honey");
    p.run("setblock 20 -60 20 minecraft:honey_block\n");
    let mut fw = Proj::new(PKind::Firework, 900_501, [19.87, -59.5, 20.5], stack("minecraft:firework_rocket"));
    fw.vel = [0.1, -0.5, 0.0];
    let hit = Hit::Block(BlockHit { loc: [19.875, -59.5, 20.5], pos: [20, -60, 20], face: 4, inside: false, border: false });
    p.s.proj_hit(&mut fw, hit);
    let old = -0.5 / f64::from(0.98f32) + 0.08;
    let expect = [0.1 * (-0.05 / old), (-0.05 - 0.08) * f64::from(0.98f32), 0.0];
    assert_eq!(fw.vel, expect, "honey slide");
    assert!(!fw.removed, "without an explosion, the rocket continues");
    finish(p, dir);
}

/// Shulker bullet touching a large fireball: the ball leaves according to the bullet's momentum
/// (known speed normalised), not according to the shooter's look; zero momentum on the first tick.
#[test]
fn shulker_bullet_redirects_by_momentum() {
    let (mut p, dir) = open("bullet");
    p.s.yaw = -90.0; // player turned towards +x
    p.s.pitch = 0.0;
    for (n, (known, expect)) in [([0.0, 0.0, 0.3], [0.0, 0.0, 1.0]), ([0.0; 3], [0.0; 3])].into_iter().enumerate() {
        let fid = 900_601 + n as i32 * 10;
        let mut fb = Proj::new(PKind::LargeFireball, fid, [30.5, -55.0, 30.5], stack("minecraft:fire_charge"));
        fb.vel = [0.0, -0.1, 0.0];
        p.s.proj_add(fb);
        let mut b = Proj::new(PKind::ShulkerBullet, fid + 1, [30.5, -55.0, 29.5], stack("minecraft:air"));
        b.owner = Some(p.s.uuid);
        b.bullet = Some(Box::new(BulletState { known_speed: known, ..Default::default() }));
        p.s.proj_hit(&mut b, Hit::Entity(EntityHit { ent: Target::Proj(fid), loc: [30.5, -55.0, 30.3] }));
        let q = p.s.proj.list.iter().find(|q| q.id == fid).expect("fireball");
        assert_eq!(q.vel, expect, "momentum {known:?}");
        assert_eq!(q.owner, Some(p.s.uuid), "the bullet gives its author");
    }
    finish(p, dir);
}

/// The known speed of the bullet is its movement between the last two tick starts.
#[test]
fn shulker_bullet_known_speed_tracks_ticks() {
    let (mut p, dir) = open("bspeed");
    let mut b = Proj::new(PKind::ShulkerBullet, 900_701, [-30.5, -50.0, -30.5], stack("minecraft:air"));
    b.bullet = Some(Box::new(BulletState::default()));
    b.no_gravity = true;
    b.vel = [0.0, 0.0, 0.25];
    p.s.shulker_bullet_tick(&mut b);
    assert_eq!(b.bullet.as_ref().expect("state").known_speed, [0.0; 3], "first tick: zero");
    let before = b.pos;
    p.s.shulker_bullet_tick(&mut b);
    let after_first = b.bullet.as_ref().expect("state").known_speed;
    let start = [-30.5, -50.0, -30.5];
    assert_eq!(after_first, [before[0] - start[0], before[1] - start[1], before[2] - start[2]], "movement of tick 1");
    finish(p, dir);
}

/// Arrow, wind charge and sword hit of the player on a dragon part: the part is not a living being,
/// the player's last victim is cleared (wolves do not target the dragon).
#[test]
fn dragon_part_hits_clear_last_victim() {
    let (mut p, dir) = open("dragonpart");
    let z = mob(&mut p, Kind::Zombie, [-6.5, -60.0, 0.5], 20.0);
    let mut d = p.s.new_mob(Kind::EnderDragon, [0.5, -50.0, 8.5], 0.0);
    d.persistent = true;
    let part = [0.5, -60.0, 2.5];
    d.end.dragon.as_mut().expect("dragon").parts = [part; 8];
    let id = d.id;
    p.s.add_mob(d);
    let hit = EntityHit { ent: Target::Part(id, 0), loc: [0.5, -59.5, 2.5] };

    p.s.mobs.player_hit = Some((Ent::Mob(z), 1));
    let mut arrow = Proj::new(PKind::Arrow, 900_801, [0.5, -59.5, 1.5], stack("minecraft:arrow"));
    arrow.owner = Some(p.s.uuid);
    arrow.damage = 2.0;
    arrow.vel = [0.0, 0.0, 1.0];
    p.s.proj_hit_entity(&mut arrow, hit);
    assert_eq!(p.s.mobs.player_hit, None, "arrow on a dragon part");

    p.s.mobs.player_hit = Some((Ent::Mob(z), 2));
    let mut wind = Proj::new(PKind::WindCharge, 900_802, [0.5, -59.5, 1.5], stack("minecraft:wind_charge"));
    wind.owner = Some(p.s.uuid);
    wind.vel = [0.0, 0.0, 1.0];
    p.s.proj_hit(&mut wind, Hit::Entity(hit));
    assert_eq!(p.s.mobs.player_hit, None, "wind charge on a dragon part");

    p.s.mobs.player_hit = Some((Ent::Mob(z), 3));
    p.s.mobs.attack_ticker = 100;
    assert!(p.s.dragon_part_attacked(id + 1), "hit on the head handled");
    assert!(p.s.mob_ref(id).expect("dragon").health < 200.0, "the hit landed");
    assert_eq!(p.s.mobs.player_hit, None, "player's hit on a dragon part");
    finish(p, dir);
}

/// Damage without an author on an enderman: the 1-in-10 draw is always consumed, then the three
/// draws of the destination; as a passenger, it stays in its vehicle; dead, nothing more.
#[test]
fn enderman_unattributed_hurt_draws() {
    use crate::mobs::jrand::JRand;
    // Generator steps consumed between two states (a 1-in-10 or 1-in-64 draw = 1 step, a
    // double = 2 steps).
    fn steps(start: JRand, after: JRand) -> usize {
        let mut r = start;
        (0..500)
            .find(|_| {
                let (mut a, mut b) = (r, after);
                let same = (0..4).all(|_| a.next_int() == b.next_int());
                r.next_int();
                same
            })
            .expect("steps not found")
    }
    let (mut p, dir) = open("enrng");
    let mut counts = std::collections::BTreeSet::new();
    for seed in 0..40 {
        let mut m = p.s.new_mob(Kind::Enderman, [0.5, -60.0, 5.5], 0.0);
        m.mount.vehicle = Some(999_999);
        m.rng = JRand::new(seed);
        let start = m.pos;
        p.s.mob_hurt(&mut m, "minecraft:drown", 1.0, None, None);
        assert_eq!(m.pos, start, "passenger: no teleport (seed {seed})");
        counts.insert(steps(JRand::new(seed), m.rng));
    }
    // 1-in-10 draw at zero: nothing more; otherwise the three draws of the destination (5 steps).
    let v: Vec<usize> = counts.into_iter().collect();
    assert!(v.len() == 2 && v[1] - v[0] == 5, "steps consumed per seed: {v:?}");

    // Dead: the 1-in-10 draw stays consumed (one more step than with a living author).
    let mut dead = |by: Option<Ent>| {
        let mut m = p.s.new_mob(Kind::Enderman, [0.5, -60.0, -5.5], 0.0);
        m.health = 1.0;
        m.rng = JRand::new(7);
        p.s.mob_hurt(&mut m, "minecraft:drown", 10.0, by, None);
        assert!(!m.alive(), "enderman killed");
        steps(JRand::new(7), m.rng)
    };
    let (none, player) = (dead(None), dead(Some(Ent::Player)));
    assert_eq!(none, player + 1, "death without author: only the 1-in-10 draw more");
    finish(p, dir);
}
