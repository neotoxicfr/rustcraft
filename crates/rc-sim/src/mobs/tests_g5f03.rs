//! G5-F03: player interactions with mobs, synchronised data of the cat and wolf,
//! generic state (flags, forced age, fire). "World" tests on the copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`, zone 200..231); absent: tests skipped.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::jrand::JRand;
use super::mob::Mob;
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut g);
    m.pos = pos;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn with<R>(p: &mut Probe, id: i32, f: impl FnOnce(&mut Mob) -> R) -> R {
    let i = p.s.mob_index(id).expect("mob");
    f(p.s.mobs.list[i].as_mut().expect("mob"))
}

fn hold(p: &mut Probe, id: &str, count: i32) -> usize {
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(Stack { id: id.into(), count, components: None });
    slot
}

fn count_of(p: &Probe, slot: usize) -> i32 {
    p.s.slots[slot].as_ref().map_or(0, |s| s.count)
}

fn mobs_of(p: &Probe, k: Kind) -> Vec<i32> {
    p.s.mobs.list.iter().flatten().filter(|m| m.kind == k && !m.removed).map(|m| m.id).collect()
}

// ---------- Without a world ----------

/// Index of the wolf and cat data in the game's declaration order: collar as an integer
/// (wolf 21, cat 23), wolf anger end at 22, cat lying/relaxed at 21/22, variants at
/// 23/24 (wolf) and 20/24 (cat).
#[test]
fn wolf_and_cat_data_indices() {
    let mut w = Mob::new(Kind::Wolf, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    w.define_data();
    w.collar = 3;
    w.anger_end = 1234;
    w.variant = 5;
    w.sound_variant = 2;
    w.sync_data();
    assert_eq!(w.get(20), Some(&Meta::Bool(false)), "wolf: interested");
    assert_eq!(w.get(21), Some(&Meta::Int(3)), "wolf: collar");
    assert_eq!(w.get(22), Some(&Meta::Long(1234)), "wolf: anger end");
    assert_eq!(w.get(23), Some(&Meta::Variant(5, 25)), "wolf: variant");
    assert_eq!(w.get(24), Some(&Meta::Variant(2, 26)), "wolf: sound variant");
    let mut c = Mob::new(Kind::Cat, 2, 1, [0.0; 3], 0.0, JRand::new(1));
    c.define_data();
    c.collar = 4;
    c.cat_lying = true;
    c.variant = 6;
    c.sound_variant = 1;
    c.sync_data();
    assert_eq!(c.get(20), Some(&Meta::Variant(6, 21)), "cat: variant");
    assert_eq!(c.get(21), Some(&Meta::Bool(true)), "cat: lying");
    assert_eq!(c.get(22), Some(&Meta::Bool(false)), "cat: relaxed");
    assert_eq!(c.get(23), Some(&Meta::Int(4)), "cat: collar");
    assert_eq!(c.get(24), Some(&Meta::Variant(1, 22)), "cat: sound variant");
    // Data sorted by index (sending order).
    assert!(c.data.windows(2).all(|d| d[0].index < d[1].index));
}

/// Initial fire 0, and 0 read back by default (only the player goes back to −20 out of fire: lightning
/// therefore does not light a mob that was not burning for 8 s); entity flags and forced age read back then
/// rewritten; written only if set.
#[test]
fn initial_fire_and_flags_read_back() {
    let m = Mob::new(Kind::Cow, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    assert_eq!(m.fire, 0);
    let mut m = Mob::new(Kind::Cow, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    m.load(&Nbt::Compound(vec![]));
    assert_eq!(m.fire, 0, "fire absent from the NBT");
    let e = Nbt::Compound(vec![
        ("Silent".into(), Nbt::Byte(1)),
        ("CustomNameVisible".into(), Nbt::Byte(1)),
        ("Glowing".into(), Nbt::Byte(1)),
        ("NoGravity".into(), Nbt::Byte(1)),
        ("ForcedAge".into(), Nbt::Int(77)),
    ]);
    let mut m = Mob::new(Kind::Cow, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    m.load(&e);
    assert!(m.x.silent && m.x.name_visible && m.x.glowing && m.surf.no_gravity);
    assert_eq!(m.x.forced_age, 77);
    m.x.silent = false;
    let s = m.save();
    assert!(s.get("Silent").is_none(), "Silent erased");
    assert_eq!(crate::nbtio::int(&s, "Glowing"), Some(1));
    assert_eq!(crate::nbtio::int(&s, "NoGravity"), Some(1));
    assert_eq!(crate::nbtio::int(&s, "ForcedAge"), Some(77));
}

/// Feeding a baby: the advance is kept and becomes the age at adult age; seconds computed
/// on the integer division.
#[test]
fn baby_forced_age() {
    let mut m = Mob::new(Kind::Cow, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    m.age = -100;
    super::mounts::age_up(&mut m, 3, true);
    assert_eq!((m.age, m.x.forced_age, m.x.forced_timer), (-40, 60, 40), "60 ticks advanced and kept");
    let mut m = Mob::new(Kind::Cow, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    m.age = -100;
    super::mounts::age_up(&mut m, 10, true);
    assert_eq!(m.age, 100, "adult: delay = actual advance (100 ticks)");
    assert_eq!(super::mounts::feeding_secs(24000), 120);
    assert_eq!(super::mounts::feeding_secs(219), 1);
    assert_eq!(super::mounts::feeding_secs(199), 0);
}

// ---------- World ----------

/// Named tag: name set, persistence, one tag consumed; without a name: nothing.
#[test]
fn name_tag() {
    let Some((mut p, dir)) = probe("g5f03-name") else { return };
    let cow = spawn(&mut p, Kind::Cow, [216.5, -59.0, 205.5]);
    let slot = hold(&mut p, "minecraft:name_tag", 2);
    assert!(!p.s.player_interact(cow, 0), "name tag without a name accepted");
    crate::items::set_component(p.s.slots[slot].as_mut().unwrap(), "minecraft:custom_name", Nbt::String("Rex".into()));
    assert!(p.s.player_interact(cow, 0), "name tag refused");
    let (name, persistent) = with(&mut p, cow, |m| (m.custom_name.clone(), m.persistent));
    assert_eq!(name, Some(Nbt::String("Rex".into())));
    assert!(persistent);
    assert_eq!(count_of(&p, slot), 1);
    cleanup(p, dir);
}

/// Cow egg on a cow: baby at its position, egg consumed; pig egg: nothing.
#[test]
fn egg_on_the_same_species() {
    let Some((mut p, dir)) = probe("g5f03-egg") else { return };
    let cow = spawn(&mut p, Kind::Cow, [216.5, -59.0, 205.5]);
    let slot = hold(&mut p, "minecraft:pig_spawn_egg", 1);
    assert!(!p.s.player_interact(cow, 0), "egg of another species accepted");
    assert_eq!(mobs_of(&p, Kind::Pig).len(), 0);
    hold(&mut p, "minecraft:cow_spawn_egg", 2);
    assert!(p.s.player_interact(cow, 0), "egg refused");
    let cows = mobs_of(&p, Kind::Cow);
    assert_eq!(cows.len(), 2);
    let baby = *cows.iter().find(|&&i| i != cow).unwrap();
    let (age, pos) = with(&mut p, baby, |m| (m.age, m.pos));
    assert_eq!(age, -24000);
    assert_eq!(pos, [216.5, -59.0, 205.5]);
    assert_eq!(count_of(&p, slot), 1);
    // Creeper: never a baby, the egg does nothing.
    let c = spawn(&mut p, Kind::Creeper, [218.5, -59.0, 205.5]);
    hold(&mut p, "minecraft:creeper_spawn_egg", 1);
    assert!(!p.s.player_interact(c, 0));
    assert_eq!(mobs_of(&p, Kind::Creeper).len(), 1);
    cleanup(p, dir);
}

/// Sheep dyeing (and not of a sheared sheep, nor of the same colour); in the off hand.
#[test]
fn sheep_dye_in_off_hand() {
    let Some((mut p, dir)) = probe("g5f03-sheep") else { return };
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 205.5]);
    with(&mut p, sheep, |m| m.wool = 0);
    p.s.slots[45] = Some(Stack { id: "minecraft:blue_dye".into(), count: 3, components: None });
    let main = 36 + p.s.held as usize;
    p.s.slots[main] = None;
    assert!(p.s.player_interact(sheep, 1), "dyeing in the off hand refused");
    assert_eq!(with(&mut p, sheep, |m| m.wool), 11);
    assert_eq!(count_of(&p, 45), 2);
    assert!(!p.s.player_interact(sheep, 1), "same colour accepted");
    with(&mut p, sheep, |m| m.sheared = true);
    p.s.slots[45] = Some(Stack { id: "minecraft:red_dye".into(), count: 1, components: None });
    assert!(!p.s.player_interact(sheep, 1), "sheared sheep dyed");
    cleanup(p, dir);
}

/// Cat and wolf collar dyed by the owner (the cat becomes persistent); a tamed wolf
/// is healed by anyone; armour placed then worn down in place of health.
#[test]
fn wolf_collar_and_armor() {
    let Some((mut p, dir)) = probe("g5f03-collar") else { return };
    let me = p.s.uuid;
    let cat = spawn(&mut p, Kind::Cat, [216.5, -59.0, 205.5]);
    with(&mut p, cat, |m| {
        m.tame = true;
        m.owner = Some(me);
    });
    let slot = hold(&mut p, "minecraft:lime_dye", 2);
    assert!(p.s.player_interact(cat, 0));
    assert_eq!(with(&mut p, cat, |m| (m.collar, m.persistent)), (5, true));
    assert_eq!(count_of(&p, slot), 1);
    let wolf = spawn(&mut p, Kind::Wolf, [214.5, -59.0, 205.5]);
    with(&mut p, wolf, |m| {
        m.tame = true;
        m.owner = Some(me);
        m.attrs.max_health.base = 40.0;
        m.health = 40.0;
    });
    hold(&mut p, "minecraft:blue_dye", 1);
    assert!(p.s.player_interact(wolf, 0));
    assert_eq!(with(&mut p, wolf, |m| m.collar), 11);
    // Armour.
    let slot = hold(&mut p, "minecraft:wolf_armor", 1);
    assert!(p.s.player_interact(wolf, 0), "armour refused");
    assert_eq!(with(&mut p, wolf, |m| m.equipment[super::mounts::BODY].as_ref().map(|s| s.id.clone())).as_deref(), Some("minecraft:wolf_armor"));
    assert_eq!(count_of(&p, slot), 0);
    let i = p.s.mob_index(wolf).unwrap();
    let mut m = p.s.mobs.list[i].take().unwrap();
    p.s.mob_hurt(&mut m, "minecraft:cactus", 4.5, None, None);
    let health = m.health;
    let dmg = m.equipment[super::mounts::BODY].as_ref().map(|s| p.s.life.items.damage(s));
    p.s.mobs.list[i] = Some(m);
    assert_eq!(health, 40.0, "health moved under the armour");
    assert_eq!(dmg, Some(5), "wear = damage rounded up");
    cleanup(p, dir);
}

/// Breeding: wool mixed by the recipe (red + yellow = orange), default sound variant
/// for the cow (not the parent's).
#[test]
fn birth_inheritance() {
    let Some((mut p, dir)) = probe("g5f03-inheritance") else { return };
    let a = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 205.5]);
    let b = spawn(&mut p, Kind::Sheep, [217.5, -59.0, 205.5]);
    with(&mut p, a, |m| m.wool = 14);
    with(&mut p, b, |m| m.wool = 4);
    let i = p.s.mob_index(a).unwrap();
    let mut m = p.s.mobs.list[i].take().unwrap();
    p.s.breed(&mut m, b);
    p.s.mobs.list[i] = Some(m);
    let lamb = mobs_of(&p, Kind::Sheep).into_iter().find(|&x| x != a && x != b).expect("lamb");
    assert_eq!(with(&mut p, lamb, |m| m.wool), 1, "orange expected");
    let c1 = spawn(&mut p, Kind::Cow, [219.5, -59.0, 205.5]);
    let c2 = spawn(&mut p, Kind::Cow, [220.5, -59.0, 205.5]);
    let classic = p.s.regs.entries("minecraft:cow_sound_variant").iter().position(|x| x == "minecraft:classic").unwrap() as i32;
    let other = if classic == 0 { 1 } else { 0 };
    with(&mut p, c1, |m| m.sound_variant = other);
    let i = p.s.mob_index(c1).unwrap();
    let mut m = p.s.mobs.list[i].take().unwrap();
    p.s.breed(&mut m, c2);
    p.s.mobs.list[i] = Some(m);
    let calf = mobs_of(&p, Kind::Cow).into_iter().find(|&x| x != c1 && x != c2).expect("calf");
    assert_eq!(with(&mut p, calf, |m| m.sound_variant), classic);
    cleanup(p, dir);
}

/// Lamb at locked age: grazing does not make it grow.
#[test]
fn locked_lamb_does_not_grow() {
    let Some((mut p, dir)) = probe("g5f03-lamb") else { return };
    let s = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 205.5]);
    let i = p.s.mob_index(s).unwrap();
    let mut m = p.s.mobs.list[i].take().unwrap();
    m.age = -24000;
    m.mount.age_locked = true;
    p.s.mob_ate(&mut m);
    assert_eq!(m.age, -24000);
    m.mount.age_locked = false;
    p.s.mob_ate(&mut m);
    assert_eq!(m.age, -24000 + 1200);
    p.s.mobs.list[i] = Some(m);
    let _ = p.take_events();
    cleanup(p, dir);
}

/// Break event of the wolf armour (65).
#[test]
fn wolf_armor_break() {
    let Some((mut p, dir)) = probe("g5f03-break") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [214.5, -59.0, 205.5]);
    let i = p.s.mob_index(wolf).unwrap();
    let mut m = p.s.mobs.list[i].take().unwrap();
    let mut armor = Stack { id: "minecraft:wolf_armor".into(), count: 1, components: None };
    let max = p.s.life.items.max_damage(&armor);
    crate::lootfn::set_damage(&mut armor, max - 1);
    m.equipment[super::mounts::BODY] = Some(armor);
    m.track.seen = true;
    let _ = p.take_events();
    p.s.mob_hurt(&mut m, "minecraft:cactus", 2.0, None, None);
    assert!(m.equipment[super::mounts::BODY].is_none(), "armour not broken");
    let id = m.id;
    p.s.mobs.list[i] = Some(m);
    let ev = p.take_events();
    let broke = ev.iter().position(|e| matches!(e, Event::Entity(EntityEvent::Event { id: x, event: 65 }) if *x == id));
    let empty = ev.iter().position(|e| matches!(e, Event::Entity(EntityEvent::Equipment { id: x, .. }) if *x == id));
    let (Some(broke), Some(empty)) = (broke, empty) else { panic!("event 65 or equipment missing") };
    // The client plays the break of the item still in the slot: the event comes first.
    assert!(broke < empty, "empty equipment sent before the break event");
    cleanup(p, dir);
}

/// The fingerprint tracks each of the synchronised attributes (base and modifier) and only them.
#[test]
fn fingerprint_tracks_synced_attributes() {
    let mut m = Mob::new(Kind::Zombie, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    for (i, name) in super::mob::SYNCABLE_MODELED.into_iter().enumerate() {
        let before = super::mob::syncable_fingerprint(&m.attrs);
        m.attrs.by_name_mut(name).unwrap().base += 0.25;
        let after = super::mob::syncable_fingerprint(&m.attrs);
        let moved: Vec<usize> = (0..after.len()).filter(|&j| after[j] != before[j]).collect();
        assert_eq!(moved, vec![i], "{name}: only its fingerprint changes");
        let before = super::mob::syncable_fingerprint(&m.attrs);
        m.attrs.by_name_mut(name).unwrap().set_mod("minecraft:trial", 0.5, 0);
        assert_ne!(super::mob::syncable_fingerprint(&m.attrs), before, "{name}: modifier not tracked");
    }
    let before = super::mob::syncable_fingerprint(&m.attrs);
    m.attrs.follow.base += 3.0;
    assert_eq!(super::mob::syncable_fingerprint(&m.attrs), before, "follow range not synchronised");
}

/// Bench: fingerprint of the synchronisable attributes (computed every tick for every tracked mob).
/// `cargo test --release -p rc-sim --lib bench_attribute_fingerprint -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_attribute_fingerprint() {
    let mut m = Mob::new(Kind::Zombie, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    m.attrs.speed.set_mod("minecraft:baby", 0.5, 1);
    let n = 2_000_000u32;
    let t = std::time::Instant::now();
    let mut acc = 0u64;
    for i in 0..n {
        m.attrs.armor.base = f64::from(i & 1);
        acc ^= super::mob::syncable_fingerprint(std::hint::black_box(&m.attrs)).iter().fold(0, |a, h| a ^ h);
    }
    let ns = t.elapsed().as_nanos() as f64 / f64::from(n);
    eprintln!("attribute fingerprint: {ns:.1} ns per mob per tick ({acc:x})");
}
