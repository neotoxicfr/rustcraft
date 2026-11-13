//! The player's spear on the integrated server: the stab of a left click on two mobs in line, its
//! refusals (charge not ready, attack packet with a spear), the Lunge after it (and its conditions),
//! the charge of a held use, and the shared search of the reach. On a copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`); absent: tests skipped (failure under `RUSTCRAFT_OPT_REQUIRE=1`).
//! Every test below fails on the code before: the server dropped `player_action` 8, started no use
//! for a kinetic weapon, and had no `post_piercing_attack` effects.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto::varint::write_varint;
use rc_proto_v777::play::SoundRef;

use crate::blocks::probe::Probe;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::player::Stack;

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

/// The player at (110.5, -59, 110.5), looking south (+Z), level gaze, empty-handed.
fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [110.5, -59.0, 110.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.on_ground = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.food = 20;
    p.s.life.vit.exhaustion = 0.0;
    p.s.life.vit.impulse_grace = 0;
    p.s.life.vit.dead = false;
    p.s.life.vit.cooldown = 0;
    p.s.yaw = 0.0;
    p.s.pitch = 0.0;
    p.s.proj_player.known_move = [0.0; 3];
    p.s.mobs.player_vel = [0.0; 3];
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    Some((p, dir))
}

fn spear(list: &[(&str, i32)]) -> Stack {
    let mut s = Stack { id: "minecraft:iron_spear".to_owned(), count: 1, components: None };
    if !list.is_empty() {
        let fields = list.iter().map(|(e, l)| ((*e).to_owned(), Nbt::Int(*l))).collect();
        crate::items::set_component(&mut s, "minecraft:enchantments", Nbt::Compound(fields));
    }
    s
}

/// The spear in the main hand, its attributes applied, the attack fully charged.
fn hold(p: &mut Probe, s: Stack) {
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(s);
    p.s.equipment_tick();
    p.s.mobs.attack_ticker = 100;
}

fn wear(p: &Probe) -> i64 {
    let held = p.s.slots[36 + p.s.held as usize].as_ref();
    held.and_then(|s| crate::items::component(s, "minecraft:damage")).and_then(Nbt::as_i64).unwrap_or(0)
}

/// A zombie with lots of health at `z` in front of the player, added to the list.
fn zombie_at(p: &mut Probe, z: f64) -> i32 {
    let mut m: Mob = p.s.new_mob(Kind::Zombie, [110.5, -59.0, z], 180.0);
    m.attrs.max_health.base = 500.0;
    m.health = 500.0;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn health(p: &Probe, id: i32) -> f32 {
    p.s.mob_ref(id).expect("mob").health
}

fn sounds(p: &Probe, ev: &[Event], name: &str) -> usize {
    let Some(id) = p.s.sound_id(name) else { return 0 };
    ev.iter().filter(|e| matches!(e, Event::Sound(s) if s.sound == SoundRef::Id(id))).count()
}

/// `player_action` 8 (stab) as the client sends it, read by the server.
fn send_stab(p: &mut Probe) {
    let mut pkt = Vec::new();
    write_varint(&mut pkt, p.s.peer.ids().player_action);
    write_varint(&mut pkt, 8);
    pkt.extend(0i64.to_be_bytes());
    pkt.push(0);
    write_varint(&mut pkt, 0);
    assert!(p.s.life_packet(&pkt), "stab packet not handled");
}

/// `use_item` with the main hand, level gaze toward the south.
fn send_use(p: &mut Probe) {
    let mut pkt = Vec::new();
    write_varint(&mut pkt, p.s.peer.ids().use_item);
    write_varint(&mut pkt, 0);
    write_varint(&mut pkt, 1);
    pkt.extend(0f32.to_be_bytes());
    pkt.extend(0f32.to_be_bytes());
    assert!(p.s.life_packet(&pkt), "use packet not handled");
}

/// Damage a zombie (armor 2) takes from a hit of `amount`: the game's armor reduction.
fn after_zombie_armor(amount: f32) -> f32 {
    let armor = (2.0 - amount / 2.0).clamp(2.0 * 0.2, 20.0);
    amount * (1.0 - armor / 25.0)
}

/// Advancement tree with one `spear_mobs` criterion of `count`.
fn spear_mobs_tree(p: &mut Probe, count: i64) {
    let mut raw = std::collections::BTreeMap::new();
    raw.insert(
        "test:spear_mobs".to_owned(),
        serde_json::json!({"criteria": {"c": {"trigger": "minecraft:spear_mobs", "conditions": {"count": count}}}}),
    );
    p.s.prog.reload(std::sync::Arc::new(crate::progress::Tree::build(raw)));
}

fn advancement_done(p: &Probe, id: &str) -> bool {
    p.s.prog.tree.index.get(id).is_some_and(|&i| p.s.prog.done(i))
}

// ---------- Stab ----------

/// An iron spear (attack 3, full charge) stabs the two zombies in line within its reach (2 to 4.5
/// blocks): each takes 3 spear damage less armor and is pushed south; the one closer than the
/// minimum reach and the one beyond the maximum are left alone. Hit sound once, attack charge
/// back to zero, one point of wear and one use counted per living target. A second stab right
/// away is refused (charge below the spear's minimum of 1).
#[test]
fn stab_hits_every_mob_in_line_within_the_reach() {
    let Some((mut p, _dir)) = probe("spear-stab") else { return };
    hold(&mut p, spear(&[]));
    let near = zombie_at(&mut p, 112.5);
    let far = zombie_at(&mut p, 114.0);
    let too_close = zombie_at(&mut p, 111.4);
    let beyond = zombie_at(&mut p, 116.5);
    let _ = p.take_events();
    send_stab(&mut p);
    let ev = p.take_events();
    let expected = after_zombie_armor(3.0);
    for (id, what) in [(near, "near"), (far, "far")] {
        let lost = 500.0 - health(&p, id);
        assert!((lost - expected).abs() < 1e-4, "{what} zombie lost {lost}, expected {expected}");
        assert!(p.s.mob_ref(id).expect("mob").vel[2] > 0.0, "{what} zombie not pushed south");
    }
    assert_eq!(health(&p, too_close), 500.0, "zombie closer than the minimum reach was hit");
    assert_eq!(health(&p, beyond), 500.0, "zombie beyond the maximum reach was hit");
    assert_eq!(sounds(&p, &ev, "minecraft:item.spear.hit"), 1, "hit sound");
    assert_eq!(p.s.mobs.attack_ticker, 0, "attack charge not reset");
    assert_eq!(wear(&p), 2, "one point of wear per living target");
    assert_eq!(p.s.stat_value("minecraft:used", "minecraft:iron_spear"), 2, "uses counted");
    // Charge not ready: refused, nothing at all.
    send_stab(&mut p);
    let ev = p.take_events();
    assert!((500.0 - health(&p, near) - expected).abs() < 1e-4, "second stab landed");
    assert_eq!(sounds(&p, &ev, "minecraft:item.spear.hit"), 0, "second stab sounded");
}

/// The attack packet is ignored while the main hand holds a spear (its left click stabs); a sword
/// still hits.
#[test]
fn attack_packet_is_ignored_with_a_spear() {
    let Some((mut p, _dir)) = probe("spear-attack-packet") else { return };
    let z = zombie_at(&mut p, 112.0);
    let attack = |p: &mut Probe| {
        let mut pkt = Vec::new();
        write_varint(&mut pkt, p.s.peer.ids().attack);
        write_varint(&mut pkt, z);
        assert!(p.s.mobs_packet(&pkt), "attack packet not handled");
    };
    hold(&mut p, spear(&[]));
    attack(&mut p);
    assert_eq!(health(&p, z), 500.0, "the spear hit as a melee attack");
    hold(&mut p, Stack { id: "minecraft:diamond_sword".to_owned(), count: 1, components: None });
    attack(&mut p);
    assert!(health(&p, z) < 500.0, "the sword did not hit");
}

// ---------- Lunge ----------

/// Lunge II after a stab that hits nothing, on foot, fed: the player is sent 0.916 forward along
/// its level gaze (velocity sent to its client, fall grace of 10 ticks), gets 8 exhaustion, the
/// spear one point of wear, and the second lunge sound plays.
#[test]
fn lunge_pushes_the_player_forward_after_a_stab() {
    let Some((mut p, _dir)) = probe("spear-lunge") else { return };
    hold(&mut p, spear(&[("minecraft:lunge", 2)]));
    let _ = p.take_events();
    send_stab(&mut p);
    let ev = p.take_events();
    let magnitude = f64::from(0.458f32 + 0.458f32);
    assert_eq!(p.s.mobs.player_vel, [0.0, 0.0, magnitude], "impulse along the gaze");
    let id = p.s.player_id;
    let sent = ev.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Motion { id: m, .. }) if *m == id));
    assert!(sent, "impulse not sent to the client");
    assert!(p.s.life.vit.impulse_grace >= 10, "no fall grace after the impulse");
    assert_eq!(p.s.life.vit.exhaustion, 8.0, "exhaustion 4 per level");
    assert_eq!(wear(&p), 1, "lunge wear");
    assert_eq!(sounds(&p, &ev, "minecraft:item.spear.lunge_2"), 1, "sound of level 2");
}

/// The impulse keeps only the horizontal part of the gaze: looking 30° up, it is shorter and flat.
#[test]
fn lunge_impulse_is_horizontal() {
    let Some((mut p, _dir)) = probe("spear-lunge-pitch") else { return };
    hold(&mut p, spear(&[("minecraft:lunge", 1)]));
    p.s.pitch = -30.0;
    send_stab(&mut p);
    let v = p.s.mobs.player_vel;
    let expected = f64::from(0.458f32) * (30f64.to_radians()).cos();
    assert_eq!(v[1], 0.0, "vertical impulse {v:?}");
    assert!((v[2] - expected).abs() < 1e-6, "horizontal impulse {v:?}, expected {expected}");
}

/// No Lunge in a vehicle, nor for a survival player short of food (6); a creative player short of
/// food still lunges.
#[test]
fn lunge_conditions() {
    let Some((mut p, _dir)) = probe("spear-lunge-conditions") else { return };
    hold(&mut p, spear(&[("minecraft:lunge", 1)]));
    let pig = p.s.new_mob(Kind::Pig, [120.5, -59.0, 120.5], 0.0);
    let pig_id = pig.id;
    p.s.add_mob(pig);
    p.s.mobs.mounts.riding.vehicle = Some(pig_id);
    send_stab(&mut p);
    assert_eq!(p.s.mobs.player_vel, [0.0; 3], "lunged in a vehicle");
    p.s.mobs.mounts.riding.vehicle = None;
    p.s.life.vit.food = 6;
    p.s.mobs.attack_ticker = 100;
    send_stab(&mut p);
    assert_eq!(p.s.mobs.player_vel, [0.0; 3], "lunged while hungry in survival");
    assert_eq!(p.s.life.vit.exhaustion, 0.0, "exhaustion without a lunge");
    p.s.game_mode = 1;
    p.s.mobs.attack_ticker = 100;
    send_stab(&mut p);
    assert!(p.s.mobs.player_vel[2] > 0.0, "no lunge in creative");
}

// ---------- Charge ----------

/// Runs `ticks` ticks of the use in progress (before each countdown, like the server's tick).
fn hold_use(p: &mut Probe, ticks: i32) {
    for _ in 0..ticks {
        let Some((hand, left)) = p.s.life.vit.using else { panic!("no use in progress") };
        p.s.w8c_use_tick(hand, left);
        p.s.life.vit.using = Some((hand, left - 1));
    }
}

/// Holding the use of an iron spear starts a charge (72,000 ticks). Moving at 5 blocks/s toward a
/// zombie: nothing during the spear's 12 ticks of delay, then one hit of 1 + floor(5 × 0.95) = 5
/// (armor taken off) without knockback (5 < 5.1 blocks/s), the hit feedback (event 2) and
/// `spear_mobs`; the contact cooldown keeps it from being hit again at once.
#[test]
fn charge_hits_once_past_the_delay() {
    let Some((mut p, _dir)) = probe("spear-charge") else { return };
    spear_mobs_tree(&mut p, 1);
    hold(&mut p, spear(&[]));
    let z = zombie_at(&mut p, 113.5);
    send_use(&mut p);
    assert_eq!(p.s.life.vit.using, Some((0, crate::player_spear::KINETIC_USE_TICKS)), "charge not started");
    p.s.proj_player.known_move = [0.0, 0.0, 0.25];
    hold_use(&mut p, 12);
    assert_eq!(health(&p, z), 500.0, "hit during the delay");
    let _ = p.take_events();
    hold_use(&mut p, 3);
    let ev = p.take_events();
    let lost = 500.0 - health(&p, z);
    let expected = after_zombie_armor(5.0);
    assert!((lost - expected).abs() < 1e-4, "charge damage {lost}, expected {expected}");
    assert_eq!(p.s.mob_ref(z).expect("zombie").vel[2], 0.0, "knocked back below the knockback speed");
    let id = p.s.player_id;
    let feedback = ev.iter().filter(|e| matches!(e, Event::Entity(EntityEvent::Event { id: i, event: 2 }) if *i == id));
    assert_eq!(feedback.count(), 1, "hit feedback");
    assert!(advancement_done(&p, "test:spear_mobs"), "spear_mobs not triggered");
}

/// Faster (6 blocks/s): the charge also knocks the zombie back (6 ≥ 5.1), for 1 + floor(6 × 0.95)
/// = 6 damage; a new use forgets the targets hit by the previous one.
#[test]
fn charge_knocks_back_above_the_knockback_speed() {
    let Some((mut p, _dir)) = probe("spear-charge-knockback") else { return };
    hold(&mut p, spear(&[]));
    let z = zombie_at(&mut p, 113.5);
    p.s.life.vit.kinetic_hits.push((crate::player_spear::SpearTarget::Mob(z), p.s.time));
    send_use(&mut p);
    assert!(p.s.life.vit.kinetic_hits.is_empty(), "targets of a previous use kept");
    p.s.proj_player.known_move = [0.0, 0.0, 0.3];
    hold_use(&mut p, 13);
    let lost = 500.0 - health(&p, z);
    let expected = after_zombie_armor(6.0);
    assert!((lost - expected).abs() < 1e-4, "charge damage {lost}, expected {expected}");
    assert!(p.s.mob_ref(z).expect("zombie").vel[2] > 0.0, "not knocked back");
}

/// Riding a minecart that moved 0.4 per tick (8 blocks/s), the charge takes the minecart's known
/// speed (nobody steers a minecart): the zombie ahead takes 1 + floor(8 × 0.95) = 8 and is knocked
/// back, though the player's own known movement is zero. Fails on the code before (the minecart
/// was ignored: no condition met, nothing hit).
#[test]
fn charge_in_a_minecart_takes_its_speed() {
    let Some((mut p, _dir)) = probe("spear-charge-minecart") else { return };
    hold(&mut p, spear(&[]));
    let z = zombie_at(&mut p, 113.5);
    p.s.next_entity += 1;
    let mut c = crate::minecart::Cart::new(crate::minecart::CKind::Rideable, p.s.next_entity, [110.5, -59.0, 110.5]);
    c.vel = [0.0, 0.0, 0.4];
    c.known_speed = [0.0, 0.0, 0.4];
    p.s.pcart.vehicle = Some(c.id);
    p.s.cart_add(c);
    send_use(&mut p);
    hold_use(&mut p, 13);
    let lost = 500.0 - health(&p, z);
    let expected = after_zombie_armor(8.0);
    assert!((lost - expected).abs() < 1e-4, "charge damage {lost}, expected {expected}");
    assert!(p.s.mob_ref(z).expect("zombie").vel[2] > 0.0, "not knocked back");
}

// ---------- Ender dragon and shulker bullets ----------

/// A flying Ender dragon (circling: it takes knockbacks) whose parts all lie far away, but for
/// `near` (part index, box at `at`).
fn dragon_with_part(p: &mut Probe, pos: [f64; 3], near: Option<(usize, [f64; 3])>) -> i32 {
    let mut d = p.s.new_mob(Kind::EnderDragon, pos, 0.0);
    d.persistent = true;
    let dragon = d.end.dragon.as_mut().expect("dragon");
    dragon.phase = crate::mobs::end::phases::Phase::HoldingPattern;
    let parts = &mut dragon.parts;
    *parts = [[60.5, -50.0, 60.5]; 8];
    if let Some((part, at)) = near {
        parts[part] = at;
    }
    let id = d.id;
    p.s.add_mob(d);
    id
}

/// The stab reaches the dragon through its parts only, like the game: on the head, the full 3;
/// on the body, 3 / 4 + 1; the dragon's own large box in the reach is no target. A part is not
/// pushed onto the dragon: it stays where it is. Fails on the code before (the dragon was hit
/// through its own box, at the body's damage, and knocked back).
#[test]
fn stab_reaches_the_dragon_through_its_parts() {
    use crate::mobs::end::dragon::{BODY, HEAD};
    let Some((mut p, _dir)) = probe("spear-dragon") else { return };
    hold(&mut p, spear(&[]));
    let far = [110.5, -50.0, 140.5];
    let head = dragon_with_part(&mut p, far, Some((HEAD, [110.5, -58.0, 113.0])));
    send_stab(&mut p);
    let lost = 200.0 - health(&p, head);
    assert!((lost - 3.0).abs() < 1e-4, "head took {lost}, expected 3");
    let d = p.s.mob_ref(head).expect("dragon");
    assert_eq!((d.pos(), d.vel), (far, [0.0; 3]), "the dragon moved");
    assert_eq!(p.s.stat_value("minecraft:custom", "minecraft:damage_dealt"), 0, "damage counted on a part");
    p.s.mobs.get_mut(head).expect("dragon").end.dragon.as_mut().expect("dragon").parts[HEAD] = [60.5, -50.0, 60.5];
    let body = dragon_with_part(&mut p, far, Some((BODY, [110.5, -58.5, 113.0])));
    p.s.mobs.attack_ticker = 100;
    send_stab(&mut p);
    let lost = 200.0 - health(&p, body);
    assert!((lost - 1.75).abs() < 1e-4, "body took {lost}, expected 1.75");
    p.s.mobs.get_mut(body).expect("dragon").end.dragon.as_mut().expect("dragon").parts[BODY] = [60.5, -50.0, 60.5];
    let boxed = dragon_with_part(&mut p, [110.5, -60.0, 114.0], None);
    p.s.mobs.attack_ticker = 100;
    send_stab(&mut p);
    assert_eq!(health(&p, boxed), 200.0, "the dragon's own box was hit");
}

/// The charge takes a dragon part back to the dragon itself, like the game: with the head and the
/// neck both in the reach, the dragon is remembered once and hit once, at the body's rule:
/// 1 + floor(6 × 0.95) = 6, so 6 / 4 + 1. Fails on the code before (the dragon's own box) and
/// without the part-to-dragon step (two parts remembered, the head's full 6).
#[test]
fn charge_hits_the_dragon_once_through_its_parts() {
    use crate::mobs::end::dragon::{HEAD, NECK};
    use crate::player_spear::SpearTarget;
    let Some((mut p, _dir)) = probe("spear-charge-dragon") else { return };
    hold(&mut p, spear(&[]));
    let dragon = dragon_with_part(&mut p, [110.5, -50.0, 140.5], Some((HEAD, [110.5, -58.0, 113.0])));
    let neck = [110.5, -59.0, 114.5];
    p.s.mobs.get_mut(dragon).expect("dragon").end.dragon.as_mut().expect("dragon").parts[NECK] = neck;
    send_use(&mut p);
    p.s.proj_player.known_move = [0.0, 0.0, 0.3];
    hold_use(&mut p, 13);
    let lost = 200.0 - health(&p, dragon);
    assert!((lost - 2.5).abs() < 1e-4, "dragon took {lost}, expected 2.5");
    let remembered: Vec<SpearTarget> = p.s.life.vit.kinetic_hits.iter().map(|(t, _)| *t).collect();
    assert_eq!(remembered, vec![SpearTarget::Mob(dragon)], "targets remembered");
}

/// A shulker bullet in the reach is a target: the stab destroys it (its hurt sound), the spear's
/// hit sound plays, the player spends 0.1 exhaustion; no wear, the bullet being no living target.
/// Fails on the code before (bullets were left out of the targets).
#[test]
fn stab_destroys_a_shulker_bullet() {
    use crate::projectiles::{PKind, Proj};
    let Some((mut p, _dir)) = probe("spear-bullet") else { return };
    hold(&mut p, spear(&[]));
    let air = Stack { id: "minecraft:air".to_owned(), count: 1, components: None };
    let mut b = Proj::new(PKind::ShulkerBullet, 900_901, [110.5, -57.5, 113.0], air);
    b.bullet = Some(Box::default());
    p.s.proj_add(b);
    let _ = p.take_events();
    send_stab(&mut p);
    let ev = p.take_events();
    assert!(p.s.proj.list.iter().filter(|q| q.id == 900_901).all(|q| q.removed), "bullet not destroyed");
    assert_eq!(sounds(&p, &ev, "minecraft:entity.shulker_bullet.hurt"), 1, "bullet hurt sound");
    assert_eq!(sounds(&p, &ev, "minecraft:item.spear.hit"), 1, "hit sound");
    assert!((p.s.life.vit.exhaustion - 0.1).abs() < 1e-6, "exhaustion {}", p.s.life.vit.exhaustion);
    assert_eq!(wear(&p), 0, "wear on a bullet");
}

// ---------- Reach search ----------

/// The box around the reach segment, which spares the ray tests on far entities, never leaves out
/// a box the segment hits (margin included): of 4,000 boxes scattered around a segment, every one
/// hit meets it, so the search finds the same targets with or without it.
#[test]
fn reach_search_area_keeps_every_hit() {
    let Some((p, _dir)) = probe("spear-search-area") else { return };
    let eye = [110.5, -57.38, 110.5];
    let look = crate::mobs::cubes::view_vector(-20.0, 35.0);
    let seg = p.s.weapon_reach_segment(eye, look, (2.0, 4.5), [0.0, 0.0, 0.1]).expect("segment");
    let margin = f64::from(0.125f32);
    let area = seg.search_area(margin);
    let mut seed = 0x5DEE_CE66u64;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let middle: [f64; 3] = std::array::from_fn(|i| (seg.from[i] + seg.to[i]) / 2.0);
    let mut hits = 0;
    for _ in 0..4_000 {
        let center: [f64; 3] = std::array::from_fn(|i| middle[i] + (next() - 0.5) * 7.0);
        let half = [0.1 + next() * 0.6, 0.1 + next(), 0.1 + next() * 0.6];
        let bb = rc_physics::Aabb {
            min: std::array::from_fn(|i| center[i] - half[i]),
            max: std::array::from_fn(|i| center[i] + half[i]),
        };
        if p.s.reach_segment_hits(&seg, &bb, margin) {
            hits += 1;
            assert!(area.intersects(&bb), "a hit box lies outside the search area: {bb:?}");
        }
    }
    assert!(hits > 20, "too few hits to prove anything: {hits}");
}
