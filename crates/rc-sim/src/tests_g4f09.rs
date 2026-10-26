//! G4-F09: turtle helmet, 1.21 effects (wind charged, weaving, oozing, infested),
//! player hurt sound through the damage type's `effects` field, statistics of fall,
//! gliding, resistance and the straw bed. Fresh flat world: no external data other than
//! the 26.3 cache and the client jar, whose absence makes the test FAIL.

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::SoundRef;

use crate::blocks::probe::{Probe, state_string};
use crate::effects::Inst;
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;
use crate::player::Stack;

struct W {
    p: Probe,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// First layer of air of the flat world (grass at y = −61).
const Y: i32 = -60;

fn world(tag: &str) -> W {
    let data = rc_testdata::version_data().expect("G4-F09: 26.3 data cache missing");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G4-F09: 26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g4f09-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "G4-F09: flat world generation too slow");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:air\n", Y + 3));
    p.s.pos = [8.5, f64::from(Y), 8.5];
    p.s.on_ground = true;
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    let _ = p.take_events();
    W { p, dir }
}

impl W {
    fn sound_count(&self, evs: &[Event], name: &str) -> usize {
        let id = self.p.s.sound_id(name).expect("known sound");
        evs.iter().filter(|e| matches!(e, Event::Sound(s) if matches!(s.sound, SoundRef::Id(i) if i == id))).count()
    }

    fn mobs_of(&self, k: Kind) -> Vec<&crate::mobs::mob::Mob> {
        self.p.s.mobs.list.iter().flatten().filter(|m| m.kind == k && !m.removed).map(|m| &**m).collect()
    }

    fn cobwebs_around(&self, c: [i32; 3]) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        for x in c[0] - 1..=c[0] + 1 {
            for y in c[1] - 1..=c[1] + 1 {
                for z in c[2] - 1..=c[2] + 1 {
                    if state_string(&self.p.s, self.p.s.get([x, y, z])) == "minecraft:cobweb" {
                        out.push([x, y, z]);
                    }
                }
            }
        }
        out
    }
}

fn turtle_helmet() -> Option<Stack> {
    Some(Stack { id: "minecraft:turtle_helmet".into(), count: 1, components: None })
}

// ---------- Turtle helmet ----------

/// Worn on the head out of water: water breathing 200 ticks, no particles, with
/// icon, re-applied at each tick (update sent); in hand or head in water: nothing.
#[test]
fn turtle_helmet_water_breathing() {
    let mut w = world("turtle");
    let s = &mut w.p.s;
    s.slots[5] = turtle_helmet();
    s.vitals_tick();
    let e = s.life.fx.get("minecraft:water_breathing").cloned().expect("turtle helmet without water breathing");
    assert_eq!((e.dur, e.amp, e.ambient, e.visible, e.icon), (200, 0, false, false, true));
    let _ = w.p.take_events();
    w.p.s.vitals_tick();
    let evs = w.p.take_events();
    let e = w.p.s.life.fx.get("minecraft:water_breathing").expect("effect lost");
    assert_eq!(e.dur, 200, "duration not reset to 200");
    assert!(
        evs.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Effect { duration: 200, flags: 4, .. }))),
        "effect update not sent (icon alone)"
    );
    // In hand: the stack is not "worn" (its equipment slot is the head).
    let s = &mut w.p.s;
    s.clear_effects();
    s.slots[5] = None;
    s.slots[36 + s.held as usize] = turtle_helmet();
    s.vitals_tick();
    assert!(!s.life.fx.has("minecraft:water_breathing"), "helmet in hand counted as worn");
    // Component rewritten: "hand-equippable" helmet held in hand = worn; component removed
    // on the head = not worn.
    let comp = |k: &str, v: Nbt| Some(Nbt::Compound(vec![(k.into(), v)]));
    let slot = Nbt::Compound(vec![("slot".into(), Nbt::String("mainhand".into()))]);
    s.slots[36 + s.held as usize] =
        Some(Stack { id: "minecraft:turtle_helmet".into(), count: 1, components: comp("minecraft:equippable", slot) });
    s.vitals_tick();
    assert!(s.life.fx.has("minecraft:water_breathing"), "stack component slot ignored");
    s.clear_effects();
    s.slots[36 + s.held as usize] = None;
    s.slots[5] = Some(Stack {
        id: "minecraft:turtle_helmet".into(),
        count: 1,
        components: comp("!minecraft:equippable", Nbt::Compound(Vec::new())),
    });
    s.vitals_tick();
    assert!(!s.life.fx.has("minecraft:water_breathing"), "helmet without component counted as worn");
    // Head in water: nothing.
    s.slots[36 + s.held as usize] = None;
    s.slots[5] = turtle_helmet();
    w.p.run(&format!("setblock 8 {} 8 minecraft:water\n", Y + 1));
    let s = &mut w.p.s;
    s.vitals_tick();
    assert!(!s.life.fx.has("minecraft:water_breathing"), "breathing set with the head in water");
}

// ---------- Own particles ----------

#[test]
fn own_particles_of_1_21_effects() {
    let w = world("particles");
    let s = &w.p.s;
    let reg = s.data.registry("minecraft:particle_type").expect("particle registry");
    let idx = |n: &str| reg.entries.iter().position(|e| e == n).expect("known particle") as i32;
    for (fx, part) in [
        ("minecraft:wind_charged", "minecraft:small_gust"),
        ("minecraft:weaving", "minecraft:item_cobweb"),
        ("minecraft:oozing", "minecraft:item_slime"),
        ("minecraft:infested", "minecraft:infested"),
    ] {
        let (parts, _) = s.effect_particles(&[Inst::new(fx, 100, 0)]);
        assert_eq!(parts.len(), 1);
        assert_eq!((parts[0].kind, parts[0].color), (idx(part), 0), "particle of {fx}");
    }
}

// ---------- Hurt sound ----------

/// The sound follows the type's `effects` field: freezing, berry bush, campfire (burning).
#[test]
fn hurt_sound_by_effects_field() {
    let mut w = world("sound");
    for (ty, sound) in [
        ("minecraft:freeze", "minecraft:entity.player.hurt_freeze"),
        ("minecraft:sweet_berry_bush", "minecraft:entity.player.hurt_sweet_berry_bush"),
        ("minecraft:campfire", "minecraft:entity.player.hurt_on_fire"),
        ("minecraft:drown", "minecraft:entity.player.hurt_drown"),
        ("minecraft:generic", "minecraft:entity.player.hurt"),
    ] {
        let s = &mut w.p.s;
        s.life.vit.cooldown = 0;
        s.life.vit.health = 20.0;
        let _ = w.p.take_events();
        assert!(w.p.s.hurt(ty, 1.0), "hit {ty} refused");
        let evs = w.p.take_events();
        assert_eq!(w.sound_count(&evs, sound), 1, "sound of {ty}");
    }
}

// ---------- Infestation ----------

/// Infested player: from time to time (≈ 1 hit in 10) 1 or 2 silverfish in the middle of the
/// box, thrown at 0.3 in the gaze direction (vertical × 1.5), with the hurt sound.
#[test]
fn player_infestation() {
    let mut w = world("infested");
    let s = &mut w.p.s;
    s.life.rng = crate::rng::Rng::new(11);
    s.pitch = 0.0;
    s.yaw = 0.0;
    s.add_effect(Inst::new("minecraft:infested", 100_000, 0));
    let mut sounds = 0;
    for _ in 0..200 {
        let s = &mut w.p.s;
        s.life.vit.cooldown = 0;
        s.life.vit.health = 20.0;
        let _ = w.p.take_events();
        assert!(w.p.s.hurt("minecraft:generic", 0.5));
        let evs = w.p.take_events();
        sounds += w.sound_count(&evs, "minecraft:entity.silverfish.hurt");
    }
    let fish = w.mobs_of(Kind::Silverfish);
    assert!(fish.len() >= 4 && fish.len() <= 60, "{} silverfish for 200 hits", fish.len());
    assert_eq!(sounds, fish.len(), "one sound per silverfish");
    for f in &fish {
        assert_eq!(f.pos, [8.5, f64::from(Y) + f64::from(1.8f32) / 2.0, 8.5]);
        let h = (f.vel[0] * f.vel[0] + f.vel[2] * f.vel[2]).sqrt();
        assert!((h - 0.3).abs() < 1.0e-6 && f.vel[1] == 0.0, "velocity {:?}", f.vel);
        // Gaze towards +Z (yaw 0): the drawn angle stays in [−π/2, π/2], hence vz ≥ 0.
        assert!(f.vel[2] >= -1.0e-7, "velocity outside the gaze half-plane: {:?}", f.vel);
    }
}

/// Infested mob: same effect at each hit that lands.
#[test]
fn mob_infestation() {
    let mut w = world("infested-mob");
    let mut z = w.p.s.new_mob(Kind::Zombie, [4.5, f64::from(Y), 4.5], 0.0);
    assert!(w.p.s.give_mob_effect(&mut z, "minecraft:infested", 100_000, 0));
    for _ in 0..200 {
        z.damage_cooldown = 0;
        z.health = 20.0;
        assert!(w.p.s.mob_hurt(&mut z, "minecraft:generic", 0.5, None, None));
    }
    let fish = w.mobs_of(Kind::Silverfish);
    assert!(!fish.is_empty(), "no silverfish for 200 hits");
    let mid = f64::from(Y) + f64::from(z.height()) / 2.0;
    assert!(fish.iter().all(|f| f.pos == [4.5, mid, 4.5]), "silverfish outside the middle of the box");
}

/// Rotation around the game's Y: zero angle = identity; −π/2: (x, z) → (−z, x) exactly.
#[test]
fn rotation_y_single_precision() {
    let v = [0.25f32, -0.5, 0.75];
    assert_eq!(crate::mobs::effects::rotate_y(v, 0.0), v);
    let half = std::f64::consts::FRAC_PI_2 as f32;
    assert_eq!(crate::mobs::effects::rotate_y(v, -half), [-0.75, -0.5, 0.25]);
}

// ---------- Death: weaving, oozing, wind charged ----------

/// Player dead with the three effects: at 20 ticks, puff (event 60), gust of radius 3..5,
/// 2 or 3 cobwebs around them (event 3018 each), 2 slimes of size 2; list emptied.
#[test]
fn player_death_removal_effects() {
    let mut w = world("player-death");
    let s = &mut w.p.s;
    for id in ["minecraft:weaving", "minecraft:oozing", "minecraft:wind_charged"] {
        s.add_effect(Inst::new(id, 100_000, 0));
    }
    assert!(s.hurt("minecraft:generic_kill", 1000.0));
    assert!(s.life.vit.dead);
    let _ = w.p.take_events();
    let mut evs = Vec::new();
    for _ in 0..19 {
        w.p.s.vitals_tick();
        evs.extend(w.p.take_events());
    }
    assert!(w.cobwebs_around([8, Y, 8]).is_empty(), "cobwebs before the end of death");
    w.p.s.vitals_tick();
    w.p.s.flush_block_changes();
    evs.extend(w.p.take_events());
    assert!(evs.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Event { event: 60, .. }))), "puff missing");
    let webs = w.cobwebs_around([8, Y, 8]);
    assert!(webs.len() == 2 || webs.len() == 3, "{} cobwebs", webs.len());
    assert!(webs.iter().all(|p| p[1] == Y), "cobweb outside the layer on the grass");
    let ev3018 = evs.iter().filter(|e| matches!(e, Event::LevelEvent { kind: 3018, .. })).count();
    assert_eq!(ev3018, webs.len(), "one event 3018 per cobweb");
    let slimes = w.mobs_of(Kind::Slime);
    assert_eq!(slimes.len(), 2, "slimes of the oozing");
    assert!(slimes.iter().all(|m| m.cube.size == 2 && m.pos == [8.5, f64::from(Y) + 0.5, 8.5]));
    let burst = evs.iter().find_map(|e| match e {
        Event::Explode(x) => Some(x.radius),
        _ => None,
    });
    assert!(burst.is_some_and(|r| (3.0..5.0).contains(&r)), "wind gust: {burst:?}");
    assert!(w.p.s.life.fx.list.is_empty(), "effects kept after removal");
    // Nothing more afterwards (respawn does not replay the effects).
    let n = w.mobs_of(Kind::Slime).len();
    w.p.s.perform_respawn();
    assert_eq!(w.mobs_of(Kind::Slime).len(), n);
}

/// Immediate respawn (before 20 ticks): the removal effects go off at respawn.
#[test]
fn respawn_before_death_ends() {
    let mut w = world("respawn");
    let s = &mut w.p.s;
    s.add_effect(Inst::new("minecraft:oozing", 100_000, 0));
    assert!(s.hurt("minecraft:generic_kill", 1000.0));
    s.vitals_tick();
    assert!(w.mobs_of(Kind::Slime).is_empty());
    w.p.s.perform_respawn();
    assert_eq!(w.mobs_of(Kind::Slime).len(), 2, "slimes missing at immediate respawn");
}

/// Dead mob: effects at its removal (20 ticks), cobwebs subject to `mob_griefing`;
/// oozing limited by `max_entity_cramming`.
#[test]
fn mob_death_removal_effects() {
    let mut w = world("mob-death");
    let pos = [4.5, f64::from(Y), 4.5];
    let mut z = w.p.s.new_mob(Kind::Zombie, pos, 0.0);
    assert!(w.p.s.give_mob_effect(&mut z, "minecraft:weaving", 100_000, 0));
    assert!(w.p.s.give_mob_effect(&mut z, "minecraft:oozing", 100_000, 0));
    assert!(w.p.s.mob_hurt(&mut z, "minecraft:generic_kill", 1000.0, None, None));
    for _ in 0..30 {
        if z.removed {
            break;
        }
        w.p.s.tick_mob(&mut z);
    }
    assert!(z.removed);
    w.p.s.flush_block_changes();
    let webs = w.cobwebs_around([4, Y, 4]);
    assert!(webs.len() == 2 || webs.len() == 3, "{} cobwebs", webs.len());
    assert_eq!(w.mobs_of(Kind::Slime).len(), 2);
    // Without mob_griefing: no cobweb; cramming 3 with 2 slimes around: only one more.
    for c in ["gamerule minecraft:mob_griefing false", "gamerule minecraft:max_entity_cramming 3"] {
        let src = w.p.s.player_source();
        assert!(w.p.s.perform(src, c).0, "command refused: {c}");
    }
    let pos = [11.5, f64::from(Y), 11.5];
    w.p.run("fill 0 -60 0 15 -58 15 minecraft:air\n");
    let mut z = w.p.s.new_mob(Kind::Zombie, pos, 0.0);
    assert!(w.p.s.give_mob_effect(&mut z, "minecraft:weaving", 100_000, 0));
    assert!(w.p.s.give_mob_effect(&mut z, "minecraft:oozing", 100_000, 0));
    for _ in 0..2 {
        let mut sl = w.p.s.new_mob(Kind::Slime, [12.0, f64::from(Y), 12.0], 0.0);
        sl.set_size(1, true);
        w.p.s.add_mob(sl);
    }
    let before = w.mobs_of(Kind::Slime).len();
    assert!(w.p.s.mob_hurt(&mut z, "minecraft:generic_kill", 1000.0, None, None));
    for _ in 0..30 {
        if z.removed {
            break;
        }
        w.p.s.tick_mob(&mut z);
    }
    w.p.s.flush_block_changes();
    assert!(w.cobwebs_around([11, Y, 11]).is_empty(), "cobwebs despite mob_griefing false");
    assert_eq!(w.mobs_of(Kind::Slime).len(), before + 1, "cramming limit ignored");
}

/// Standing dead player: their pose stays "standing" on the server side (never "dying"); the gust
/// starts from the middle of their 1.8 box (y + 0.9), as do the silverfish of the fatal hit.
#[test]
fn dead_player_keeps_pose() {
    let mut w = world("death-pose");
    let s = &mut w.p.s;
    s.add_effect(Inst::new("minecraft:wind_charged", 100_000, 0));
    assert!(s.hurt("minecraft:generic_kill", 1000.0));
    let _ = w.p.take_events();
    let mut evs = Vec::new();
    for _ in 0..20 {
        w.p.s.vitals_tick();
        evs.extend(w.p.take_events());
    }
    let c = evs.iter().find_map(|e| match e {
        Event::Explode(x) => Some(x.center),
        _ => None,
    });
    let mid = f64::from(Y) + f64::from(1.8f32 / 2.0);
    assert_eq!(c, Some([8.5, mid, 8.5]), "center of the dead player's gust");
    // Infested fatal hit: the hook runs after death, silverfish in the middle of the standing box.
    let mut w = world("death-infested");
    w.p.s.life.rng = crate::rng::Rng::new(3);
    for _ in 0..200 {
        if !w.mobs_of(Kind::Silverfish).is_empty() {
            break;
        }
        let s = &mut w.p.s;
        s.life.vit.dead = false;
        s.life.vit.health = 20.0;
        s.life.vit.cooldown = 0;
        s.add_effect(Inst::new("minecraft:infested", 100_000, 0));
        assert!(s.hurt("minecraft:generic", 1000.0));
        assert!(s.life.vit.dead, "non-fatal hit");
    }
    let fish = w.mobs_of(Kind::Silverfish);
    assert!(!fish.is_empty(), "no silverfish on the fatal hit");
    assert!(fish.iter().all(|f| f.pos[1] == mid), "silverfish outside the middle: {:?}", fish[0].pos);
}

/// Turtle helmet while swimming just under the surface: eyes at 0.4 (swimming pose) in the water, hence
/// no water breathing, and the air drops. Low flowing water under the eyes: out of water.
#[test]
fn turtle_helmet_by_pose_and_fluid_height() {
    let mut w = world("turtle-swim");
    w.p.run(&format!("setblock 8 {Y} 8 minecraft:water\nsetblock 8 {} 8 minecraft:water\n", Y + 1));
    let s = &mut w.p.s;
    s.slots[5] = turtle_helmet();
    s.pos = [8.5, f64::from(Y) + 0.5, 8.5];
    s.mobs.sea.player_swimming = true;
    s.life.vit.air = 300;
    s.vitals_tick();
    assert!(!s.life.fx.has("minecraft:water_breathing"), "breathing set while swimming under the surface");
    assert!(s.life.vit.air < 300, "air does not drop while swimming under the surface");
    // Standing at the same place: eyes at y + 2.12, out of water.
    s.mobs.sea.player_swimming = false;
    s.vitals_tick();
    assert!(s.life.fx.has("minecraft:water_breathing"), "eyes out of water standing");
    // Flowing water of height 2/9 in the eye cell (y + 1.62): below the eye, hence out of water.
    let mut w = world("turtle-current");
    w.p.run(&format!("setblock 8 {} 8 minecraft:water[level=6]\n", Y + 1));
    let s = &mut w.p.s;
    s.slots[5] = turtle_helmet();
    s.vitals_tick();
    assert!(s.life.fx.has("minecraft:water_breathing"), "low flowing water counted at the eyes");
}

/// Prey swallowed by a frog: "killed" removal, so the wind charged gust goes off.
#[test]
fn frog_prey_removal_effects() {
    let mut w = world("frog");
    let mut sl = w.p.s.new_mob(Kind::Slime, [5.5, f64::from(Y), 5.5], 0.0);
    sl.set_size(1, true);
    assert!(w.p.s.give_mob_effect(&mut sl, "minecraft:wind_charged", 100_000, 0));
    let id = sl.id;
    w.p.s.add_mob(sl);
    let mut f = w.p.s.new_mob(Kind::Frog, [4.5, f64::from(Y), 4.5], 0.0);
    f.bm.tongue = Some(id);
    let _ = w.p.take_events();
    crate::mobs::brainy::frog::eat_entity(&mut w.p.s, &mut f);
    let evs = w.p.take_events();
    assert!(w.mobs_of(Kind::Slime).is_empty(), "prey not removed");
    assert!(evs.iter().any(|e| matches!(e, Event::Explode(_))), "gust missing at prey removal");
}

// ---------- Statistics ----------

/// Fall of 5 m: `fall_one_cm` = 500; creative: nothing; 1.5 m: nothing.
#[test]
fn fall_statistic() {
    let mut w = world("fall");
    let get = |w: &W| w.p.s.stat_value("minecraft:custom", "minecraft:fall_one_cm");
    let fall = |w: &mut W, h: f64| {
        let s = &mut w.p.s;
        s.life.vit.fall = 0.0;
        s.life.vit.cooldown = 0;
        s.life.vit.health = 20.0;
        let before = [8.5, f64::from(Y) + h, 8.5];
        s.pos = [8.5, f64::from(Y), 8.5];
        s.on_ground = true;
        s.after_move(before, false);
    };
    fall(&mut w, 5.0);
    assert_eq!(get(&w), 500);
    fall(&mut w, 1.5);
    assert_eq!(get(&w), 500, "fall of under 2 m counted");
    w.p.s.game_mode = 1;
    fall(&mut w, 5.0);
    assert_eq!(get(&w), 500, "fall counted in creative");
}

/// Gliding: `aviate_one_cm` (3D distance); resistance: `damage_resisted` (player) and
/// `damage_dealt_resisted` (mob hit by the player); straw bed.
#[test]
fn gliding_resistance_straw_statistics() {
    let mut w = world("stats");
    let s = &mut w.p.s;
    s.on_ground = false;
    s.proj_player.fall_flying = true;
    let before = s.pos;
    s.pos = [before[0] + 3.0, before[1] + 5.0, before[2] + 4.0];
    s.progress_after_move(before, false);
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:aviate_one_cm"), 707);
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:fly_one_cm"), 0);
    s.proj_player.fall_flying = false;
    s.pos = before;
    s.on_ground = true;
    // Resistance I: 20% of 10 → 2 points resisted = 20 tenths.
    s.add_effect(Inst::new("minecraft:resistance", 1000, 0));
    s.life.vit.cooldown = 0;
    assert!(s.hurt("minecraft:magic", 10.0));
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:damage_resisted"), 20);
    let mut z = s.new_mob(Kind::Zombie, [4.5, f64::from(Y), 4.5], 0.0);
    assert!(s.give_mob_effect(&mut z, "minecraft:resistance", 1000, 0));
    assert!(s.mob_hurt(&mut z, "minecraft:magic", 10.0, Some(Ent::Player), None));
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:damage_dealt_resisted"), 20);
    s.s9_slept(true, false);
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:sleep_in_straw_bed"), 1);
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:sleep_in_bed"), 0);
}

// ---------- Bench (loaded machine: order of magnitude only) ----------

/// Cost of the turtle helmet per player tick.
/// `cargo test --release -p rc-sim bench_turtle_helmet -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_turtle_helmet() {
    let mut w = world("bench-turtle");
    let n = 20_000;
    let run = |w: &mut W, helmet: bool| {
        w.p.s.slots[5] = if helmet { turtle_helmet() } else { None };
        w.p.s.clear_effects();
        for _ in 0..200 {
            w.p.s.vitals_tick();
            w.p.s.sync_attributes();
        }
        let _ = w.p.take_events();
        let t = std::time::Instant::now();
        for i in 0..n {
            w.p.s.vitals_tick();
            w.p.s.sync_attributes();
            if i % 256 == 0 {
                let _ = w.p.take_events();
            }
        }
        t.elapsed().as_secs_f64() * 1.0e6 / f64::from(n)
    };
    for pass in 0..2 {
        let a = run(&mut w, false);
        let b = run(&mut w, true);
        eprintln!("pass {pass}: player tick without helmet {a:.2} µs, with helmet {b:.2} µs (+{:.2} µs)", b - a);
    }
}
