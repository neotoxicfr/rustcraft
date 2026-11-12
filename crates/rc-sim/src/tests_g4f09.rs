//! G4-F09: turtle helmet, 1.21 effects (wind charged, weaving, oozing, infested),
//! player hurt sound through the damage type's `effects` field, statistics of fall,
//! gliding, resistance and the straw bed. Fresh flat world: no external data other than
//! the 26.3 cache and the client jar, whose absence makes the test FAIL.

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::{Meta, Particle};
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
        assert!(t.elapsed().as_secs() < 300, "G4-F09: anti-hang guard: flat world generation stuck for 300 s");
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
        self.p.s.mobs.list.iter().filter(|m| m.kind == k && !m.removed).collect()
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

/// A list mixing tinted swirls and an effect's own particle: one particle per visible effect, in
/// order; the swirl carries the effect's colour (opaque, alpha 38 when ambient), an own particle
/// carries none; a hidden effect is skipped. Ids checked against a walk of the registry.
/// Sabotage verified (failure): swirl id reused for an own particle; ambient alpha ignored.
#[test]
fn particles_of_a_mixed_effect_list() {
    let w = world("mixed-particles");
    let s = &w.p.s;
    let reg = s.data.registry("minecraft:particle_type").expect("particle registry");
    let idx = |n: &str| reg.entries.iter().position(|e| e == n).expect("known particle") as i32;
    let colour = |id: &str| crate::effects::def(id).expect("known effect").color & 0xFF_FFFF;
    let list = [
        Inst::new("minecraft:speed", 100, 0),
        Inst { visible: false, ..Inst::new("minecraft:strength", 100, 0) },
        Inst::new("minecraft:infested", 100, 0),
        Inst { ambient: true, ..Inst::new("minecraft:haste", 100, 0) },
    ];
    let (parts, ambient) = s.effect_particles(&list);
    let swirl = idx("minecraft:entity_effect");
    let expected = [
        (swirl, 0xFF00_0000 | colour("minecraft:speed")),
        (idx("minecraft:infested"), 0),
        (swirl, 0x2600_0000 | colour("minecraft:haste")),
    ];
    assert_eq!(parts.iter().map(|p| (p.kind, p.color)).collect::<Vec<_>>(), expected);
    let plain = |p: &Particle| Particle { color: p.color, ..Particle::plain(p.kind) };
    assert!(parts.iter().all(|p| *p == plain(p)), "no other option set: {parts:?}");
    assert!(!ambient, "one visible effect is not ambient");
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
        assert_eq!(f.pos(), [8.5, f64::from(Y) + f64::from(1.8f32) / 2.0, 8.5]);
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
    assert!(fish.iter().all(|f| f.pos() == [4.5, mid, 4.5]), "silverfish outside the middle of the box");
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
    assert!(slimes.iter().all(|m| m.cube.size == 2 && m.pos() == [8.5, f64::from(Y) + 0.5, 8.5]));
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
    assert!(fish.iter().all(|f| f.pos()[1] == mid), "silverfish outside the middle: {:?}", fish[0].pos());
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

// ---------- Effect particles ----------

/// A spectator carries no effect swirl: entering spectator mode sends an empty particle list
/// (data 10), leaving it sends the list back, like the game which refreshes the effects'
/// visibility on every mode change. Sabotage verified (failure): spectators keep their list; the
/// mode change does not mark the effects for sending.
#[test]
fn spectator_carries_no_effect_particles() {
    let mut w = world("spectator-swirls");
    let sent_lists = |evs: Vec<Event>| -> Vec<usize> {
        let list_len = |values: &[(u8, rc_proto_v777::entity::Meta)]| {
            values.iter().find_map(|(i, m)| match (i, m) {
                (10, rc_proto_v777::entity::Meta::Particles(p)) => Some(p.len()),
                _ => None,
            })
        };
        evs.iter()
            .filter_map(|e| match e {
                Event::Entity(EntityEvent::Data { values, .. }) => list_len(values),
                _ => None,
            })
            .collect()
    };
    w.p.s.add_effect(Inst::new("minecraft:night_vision", 100_000, 0));
    w.p.s.sync_effect_data();
    assert_eq!(sent_lists(w.p.take_events()), [1], "effect swirl announced");
    w.p.s.set_game_mode(3);
    w.p.s.sync_effect_data();
    assert_eq!(sent_lists(w.p.take_events()), [0], "spectator: list emptied");
    w.p.s.sync_effect_data();
    assert!(sent_lists(w.p.take_events()).is_empty(), "nothing sent again while unchanged");
    w.p.s.set_game_mode(0);
    w.p.s.sync_effect_data();
    assert_eq!(sent_lists(w.p.take_events()), [1], "survival: list back");
}

/// Entity data values the player's own entity was sent since the last call, in sending order.
fn player_data_sent(w: &W) -> Vec<(u8, Meta)> {
    let player = w.p.s.player_id;
    let values_of = |e: Event| match e {
        Event::Entity(EntityEvent::Data { id, values }) if id == player => values,
        _ => Vec::new(),
    };
    w.p.take_events().into_iter().flat_map(values_of).collect()
}

/// One player tick, then the shared flag bytes (data 0) it sent.
fn shared_flags_sent_by_a_tick(w: &mut W) -> Vec<i8> {
    w.p.s.vitals_tick();
    let flags = |(index, value)| match (index, value) {
        (0, Meta::Byte(flags)) => Some(flags),
        _ => None,
    };
    player_data_sent(w).into_iter().filter_map(flags).collect()
}

/// A spectator carries the "invisible" shared flag (0x20) whatever its effects: the game forces it
/// together with the empty particle list, and it is what makes a spectator translucent for another
/// spectator. The flag goes out on the spectator's own tick (which skips the environment pass) and
/// drops on leaving the mode, unless an invisibility effect holds it. Sabotage verified (failure):
/// spectator mode ignored by the flag; flags not sent on a spectator's tick.
#[test]
fn spectator_has_the_invisible_shared_flag() {
    const INVISIBLE: i8 = 0x20;
    let mut w = world("spectator-invisible");
    assert!(shared_flags_sent_by_a_tick(&mut w).is_empty(), "survival: nothing lit, nothing sent");
    w.p.s.set_game_mode(3);
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [INVISIBLE], "spectator: invisible");
    assert!(shared_flags_sent_by_a_tick(&mut w).is_empty(), "nothing sent again while unchanged");
    w.p.s.set_game_mode(0);
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [0], "survival: visible again");
    // With the effect: the flag holds through the modes, and the spectator keeps it without it.
    w.p.s.add_effect(Inst::new("minecraft:invisibility", 100_000, 0));
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [INVISIBLE], "invisibility effect");
    w.p.s.set_game_mode(3);
    assert!(shared_flags_sent_by_a_tick(&mut w).is_empty(), "spectator with the effect: unchanged");
    w.p.s.remove_effect("minecraft:invisibility");
    assert!(shared_flags_sent_by_a_tick(&mut w).is_empty(), "spectator without the effect: still invisible");
    w.p.s.set_game_mode(0);
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [0], "survival without the effect: visible");
}

/// An invulnerable player (spectator, creative) never holds more than one tick of fire, like the
/// game, which caps the counter each time it is written for them. A player burning when it turns
/// spectator keeps its "on fire" flag for that one tick, is out at the next, and does not burn
/// again once back in survival. Before, a spectator's counter was frozen (8 s of fire left came
/// back with their damage after any stay) and a creative player's ran down uncapped. Sabotage
/// verified (failure): counter not run down on a spectator's tick; counter written uncapped.
#[test]
fn invulnerable_player_holds_one_tick_of_fire_at_most() {
    const ON_FIRE: i8 = 1;
    const INVISIBLE: i8 = 0x20;
    let mut w = world("invulnerable-fire");
    w.p.s.life.vit.fire = 160;
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [ON_FIRE], "survival: burning");
    w.p.s.set_game_mode(3);
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [ON_FIRE | INVISIBLE], "spectator: its one tick of fire");
    assert_eq!(w.p.s.life.vit.fire, 1, "counter capped");
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [INVISIBLE], "spectator: out");
    w.p.s.set_game_mode(0);
    let health = w.p.s.life.vit.health;
    assert_eq!(shared_flags_sent_by_a_tick(&mut w), [0], "survival again: not burning");
    for _ in 0..40 {
        w.p.s.vitals_tick();
    }
    assert!(w.p.s.life.vit.fire <= 0, "fire left after the stay: {}", w.p.s.life.vit.fire);
    assert!(w.p.s.life.vit.health >= health, "burnt after the stay");

    // Creative: the same cap, on the countdown and on an ignition (lightning, lava).
    w.p.s.life.vit.fire = 160;
    w.p.s.set_game_mode(1);
    w.p.s.vitals_tick();
    assert_eq!(w.p.s.life.vit.fire, 1, "creative: counter capped");
    w.p.s.vitals_tick();
    assert!(w.p.s.life.vit.fire <= 0, "creative: out");
    w.p.s.player_ignite(160);
    assert_eq!(w.p.s.life.vit.fire, 1, "lit while invulnerable: one tick");
}

/// Effect particle data of one sync: the list (data 10, its length) and the ambient flag (11).
#[derive(Debug, PartialEq)]
enum EffectData {
    Particles(usize),
    Ambient(bool),
}

/// Syncs the player's effect data, then what went out of data 10 and 11.
fn effect_data_sent_by_a_sync(w: &mut W) -> Vec<EffectData> {
    w.p.s.sync_effect_data();
    let effect_data = |(index, value)| match (index, value) {
        (10, Meta::Particles(list)) => Some(EffectData::Particles(list.len())),
        (11, Meta::Bool(ambient)) => Some(EffectData::Ambient(ambient)),
        _ => None,
    };
    player_data_sent(w).into_iter().filter_map(effect_data).collect()
}

/// The particle list (data 10) and the ambient flag (data 11) each go out only when their own
/// value changes, like the game's synced data. The game rewrites the flag only for a player that
/// has effects and is not a spectator: without effects, or as a spectator, the list is emptied and
/// the flag keeps its last value. Before, any change sent both, the flag turned true whenever the
/// last effect ended, and both went out again at every dimension change. Sabotage verified
/// (failure): both sent on any change; flag recomputed for a player without effects; flag
/// recomputed for a spectator; a shortened list not seen as a change; both forgotten with the
/// absorption at a dimension change.
#[test]
fn particle_list_and_ambient_flag_sent_separately() {
    use EffectData::{Ambient, Particles};
    let lasting = |id: &str| Inst::new(id, 100_000, 0);
    let ambient = |id: &str| Inst { ambient: true, ..Inst::new(id, 100_000, 0) };
    let mut w = world("effect-data");
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [], "new player entity: the game's defaults hold");

    w.p.s.add_effect(lasting("minecraft:speed"));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(1)], "first effect: flag already false");
    w.p.s.add_effect(lasting("minecraft:haste"));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(2)], "second effect: list alone");
    w.p.s.add_effect(Inst { visible: false, ..lasting("minecraft:strength") });
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [], "hidden effect: neither changes");
    w.p.s.clear_effects();
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(0)], "no effect left: flag left alone");

    w.p.s.add_effect(ambient("minecraft:speed"));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(1), Ambient(true)], "ambient effect: both change");
    w.p.s.clear_effects();
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(0)], "no effect left: flag stays true");
    w.p.s.add_effect(ambient("minecraft:speed"));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(1)], "ambient again: flag was still true");
    w.p.s.add_effect(lasting("minecraft:haste"));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(2), Ambient(false)], "a non-ambient one joins");

    // Spectator: the list is emptied and the flag left alone, whatever the effects become.
    w.p.s.set_game_mode(3);
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(0)], "spectator: list alone");
    w.p.s.clear_effects();
    w.p.s.add_effect(ambient("minecraft:speed"));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [], "spectator: effects changed, nothing goes out");
    w.p.s.set_game_mode(0);
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(1), Ambient(true)], "survival: both recomputed");

    // The flag alone: an effect with its own particle looks the same ambient or not.
    w.p.s.clear_effects();
    w.p.s.add_effect(ambient("minecraft:infested"));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(1)], "another particle, flag still true");
    w.p.s.add_effect(Inst::new("minecraft:infested", 200_000, 0));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Ambient(false)], "same particle, flag alone");

    // Dimension change: the effects are announced again, not the entity data the client keeps.
    w.p.s.send_all_effects();
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [], "dimension change: nothing resent");
}

/// A respawned player is a new entity at the game's defaults: nothing is sent for it, and its
/// first effect sends the list alone. The other players keep the entity they knew: just before
/// the respawn they are told that its list and its flag are back to the defaults. Sabotage
/// verified (failure): nothing told before the respawn; held values kept through the respawn.
#[test]
fn respawn_puts_the_effect_data_back_to_the_defaults() {
    use EffectData::{Ambient, Particles};
    let mut w = world("respawn-effect-data");
    w.p.s.add_effect(Inst { ambient: true, ..Inst::new("minecraft:speed", 100_000, 0) });
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(1), Ambient(true)]);
    assert!(w.p.s.hurt("minecraft:generic_kill", 1000.0));
    let _ = w.p.take_events();
    w.p.s.perform_respawn();
    let evs = w.p.take_events();
    let respawn_at = evs.iter().position(|e| matches!(e, Event::Respawn { .. })).expect("respawn sent");
    let player = w.p.s.player_id;
    let data_of_the_old_entity: Vec<&Vec<(u8, Meta)>> = evs[..respawn_at]
        .iter()
        .filter_map(|e| match e {
            Event::Entity(EntityEvent::Data { id, values }) if *id == player => Some(values),
            _ => None,
        })
        .collect();
    let back_to_defaults = vec![(10, Meta::Particles(Vec::new())), (11, Meta::Bool(false))];
    assert_eq!(data_of_the_old_entity, [&back_to_defaults], "told before the respawn");
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [], "new entity: the defaults hold");
    w.p.s.add_effect(Inst::new("minecraft:haste", 100_000, 0));
    assert_eq!(effect_data_sent_by_a_sync(&mut w), [Particles(1)], "first effect of the new entity: list alone");
}

/// Effects touched without a visible change (one renewed every tick, like the turtle helmet's):
/// the sync compares the particle list in place and allocates nothing. Sabotage verified
/// (failure): list built before the comparison.
#[test]
fn sync_of_unchanged_effect_data_does_not_allocate() {
    let mut w = world("effect-data-allocations");
    for id in ["minecraft:speed", "minecraft:haste", "minecraft:regeneration", "minecraft:infested"] {
        w.p.s.add_effect(Inst::new(id, 100_000, 0));
    }
    w.p.s.sync_effect_data();
    let _ = w.p.take_events();
    let before = crate::test_alloc::allocations();
    for _ in 0..100 {
        w.p.s.life.fx.dirty = true;
        w.p.s.sync_effect_data();
    }
    // The `memstats` build replaces the counting allocator: the count then stays at zero.
    assert_eq!(crate::test_alloc::allocations() - before, 0, "allocations of 100 syncs without a change");
    assert!(player_data_sent(&w).is_empty(), "and nothing sent");
}

/// Same rule on a mob: when its last effect ends, only the emptied list (data 10) is marked for
/// sending; the ambient flag (data 11) keeps its value. Before, the flag turned true and went out
/// with the list. Sabotage verified (failure): flag rewritten for a mob without effects.
#[test]
fn mob_without_effects_keeps_its_ambient_flag() {
    let mut w = world("mob-ambient");
    let s = &mut w.p.s;
    let changed_data = |mob: &mut crate::mobs::mob::Mob| -> Vec<u8> {
        mob.take_dirty().into_iter().map(|(index, _)| index).filter(|i| matches!(i, 10 | 11)).collect()
    };
    let mut zombie = s.new_mob(Kind::Zombie, [4.5, f64::from(Y), 4.5], 0.0);
    let _ = zombie.take_dirty();
    assert!(s.give_mob_effect(&mut zombie, "minecraft:speed", 1000, 0));
    assert_eq!(changed_data(&mut zombie), [10], "first effect: list alone (flag already false)");
    assert!(s.clear_mob_effects(&mut zombie));
    assert_eq!(changed_data(&mut zombie), [10], "no effect left: list alone");
    assert_eq!(zombie.get(11), Some(&Meta::Bool(false)), "ambient flag left alone");
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
