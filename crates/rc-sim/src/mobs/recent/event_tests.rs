//! Game events and sounds (patterns 8 and 10) on a copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`, zone 200..231): block placement and destruction, projectiles,
//! landing, wing flaps, splash, gliding, levers, doors, buttons,
//! containers, player steps (supporting block, swimming), attached sounds, sound variants.

use rc_client::Event;
use rc_proto_v777::play::SoundRef;

use super::super::mob::Ent;
use super::super::species::Kind;
use super::vibration::{Listener, Src, Who};
use crate::blocks::probe::Probe;
use crate::input::Input;

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -45 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    p.s.game_mode = 0;
    p.s.view = 8;
    p.s.loaded = true;
    p.s.on_ground = true;
    p.s.mobs.rec.vib.log.clear();
    let _ = p.take_events();
    Some((p, dir))
}

fn step(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.tick();
        p.s.mobs_tick();
        p.s.projectiles_tick();
    }
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::super::spawn::Reason::SpawnEgg, &mut g);
    m.persistent = true;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Events emitted with a given name: (position, source, state).
fn evs(p: &Probe, name: &str) -> Vec<([f64; 3], Option<Src>, Option<u32>)> {
    let full = format!("minecraft:{name}");
    p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == full).map(|e| (e.1, e.2, e.3)).collect()
}

/// Sound sent: name, attached entity, position, seed.
type Snd = (String, Option<i32>, Option<[f64; 3]>, i64);

/// Sounds sent: (name, attached entity, position).
fn sounds(p: &Probe, events: &[Event]) -> Vec<Snd> {
    let reg = p.s.data.registry("minecraft:sound_event").expect("sound registry");
    events
        .iter()
        .filter_map(|e| match e {
            Event::Sound(s) => {
                let name = match &s.sound {
                    SoundRef::Id(i) => reg.entries.get(*i as usize).cloned().unwrap_or_default(),
                    SoundRef::Named(n) => n.clone(),
                };
                Some((name, s.entity, s.pos, s.seed))
            }
            _ => None,
        })
        .collect()
}

fn has_sound(list: &[Snd], name: &str) -> bool {
    list.iter().any(|s| s.0 == format!("minecraft:{name}"))
}

fn block_name(p: &Probe, st: Option<u32>) -> String {
    st.and_then(|s| p.s.data.block_of(s)).map_or_else(String::new, |b| b.name.clone())
}

/// Dummy listener (block without radius): the player emits their steps only if someone listens.
fn dummy_listener(p: &mut Probe) {
    p.s.mobs.rec.vib.listeners.push(Listener { who: Who::Block([0, -64, 0]), section: [0, -4, 0], order: u64::MAX });
}

/// Placement, destruction, lever, door, button, chest.
#[test]
fn block_events_and_sounds() {
    let Some((mut p, dir)) = probe("ev_blocks") else { return };
    // Placement by the player: `block_place`, player, placed state.
    p.s.game_mode = 1;
    assert!(p.s.place_item("minecraft:oak_planks", [210, -60, 210], crate::blocks::Dir::Up, [210.5, -59.0, 210.5]));
    let e = evs(&p, "block_place");
    assert_eq!(e.len(), 1, "placement: {e:?}");
    assert_eq!(e[0].1, Some(Src::Living(Ent::Player)));
    assert_eq!(block_name(&p, e[0].2), "minecraft:oak_planks");
    // Destruction not by a player: `block_destroy` with the old state.
    assert!(p.s.destroy_block([210, -59, 210], true, 512));
    let e = evs(&p, "block_destroy");
    assert_eq!(e.len(), 1, "destruction: {e:?}");
    assert_eq!(block_name(&p, e[0].2), "minecraft:oak_planks");
    // Lever: sound for everyone, event without source.
    p.run("setblock 212 -59 210 minecraft:lever[face=floor]\n");
    let _ = p.take_events();
    let lever = p.s.st([212, -59, 210]);
    p.s.lever_pull([212, -59, 210], lever);
    let snd = sounds(&p, &p.take_events());
    assert!(has_sound(&snd, "block.lever.click"), "silent lever: {snd:?}");
    let e = evs(&p, "block_activate");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].1, None);
    // Door by hand: player's event, sound excluded (predicted by the client).
    p.run("setblock 214 -59 210 minecraft:oak_door[half=lower]\nsetblock 214 -58 210 minecraft:oak_door[half=upper]\n");
    let _ = p.take_events();
    let door = p.s.st([214, -59, 210]);
    assert!(p.s.openable_use([214, -59, 210], door));
    let snd = sounds(&p, &p.take_events());
    assert!(!has_sound(&snd, "block.wooden_door.open"), "door sound sent to the player who opens: {snd:?}");
    let e = evs(&p, "block_open");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].1, Some(Src::Living(Ent::Player)));
    // Button by hand: player's `block_activate`, then released: sound for everyone, `block_deactivate`.
    p.run("setblock 216 -59 210 minecraft:stone_button[face=floor]\n");
    let _ = p.take_events();
    let button = p.s.st([216, -59, 210]);
    assert!(p.s.button_press([216, -59, 210], button, Some(Ent::Player)));
    let snd = sounds(&p, &p.take_events());
    assert!(!has_sound(&snd, "block.stone_button.click_on"), "click excluded: {snd:?}");
    let pressed = p.s.st([216, -59, 210]);
    p.s.button_tick(pressed, [216, -59, 210]);
    let snd = sounds(&p, &p.take_events());
    assert!(has_sound(&snd, "block.stone_button.click_off"), "release silent: {snd:?}");
    assert_eq!(evs(&p, "block_deactivate").len(), 1);
    // Chest: opening and closing (sound, lid, events).
    p.s.game_mode = 0;
    p.run("setblock 218 -59 210 minecraft:chest\n");
    let _ = p.take_events();
    assert!(p.s.use_block([218, -59, 210]));
    let ev = p.take_events();
    let snd = sounds(&p, &ev);
    assert!(has_sound(&snd, "block.chest.open"), "silent opening: {snd:?}");
    // Lid: block event queued, sent at the end of the tick.
    let _ = ev;
    p.s.tick_world_blocks();
    let ev = p.take_events();
    let lid = ev.iter().any(|e| matches!(e, Event::BlockEvent { pos: [218, -59, 210], b0: 1, b1: 1, .. }));
    assert!(lid, "lid motionless");
    assert_eq!(evs(&p, "container_open").len(), 1);
    p.s.close_menu(false);
    let ev = p.take_events();
    let snd = sounds(&p, &ev);
    assert!(has_sound(&snd, "block.chest.close"), "silent closing: {snd:?}");
    assert_eq!(evs(&p, "container_close").len(), 1);
    cleanup(p, dir);
}

/// Player's snowball: `projectile_shoot` (shooter) at the start, `projectile_land` at the block.
#[test]
fn projectile_events() {
    let Some((mut p, dir)) = probe("ev_proj") else { return };
    let owner = p.s.uuid;
    p.s.throw_snowball(owner, [210.5, -56.0, 210.5], [0.0, -1.0, 0.0], 0.5, 0.0);
    step(&mut p, 20);
    let shoot = evs(&p, "projectile_shoot");
    assert_eq!(shoot.len(), 1, "shot: {shoot:?}");
    assert_eq!(shoot[0].1, Some(Src::Living(Ent::Player)));
    let land = evs(&p, "projectile_land");
    assert_eq!(land.len(), 1, "impact: {land:?}");
    assert_eq!(land[0].1, Some(Src::Projectile(Some(Ent::Player))));
    assert_eq!(block_name(&p, land[0].2), "minecraft:stone");
    cleanup(p, dir);
}

/// Mobs: landing, wing flaps, splash, damage, death.
#[test]
fn mob_motion_events() {
    let Some((mut p, dir)) = probe("ev_mobs") else { return };
    p.run("fill 220 -60 220 224 -59 224 minecraft:water\n");
    let pig = spawn(&mut p, Kind::Pig, [205.5, -55.0, 205.5]);
    let chicken = spawn(&mut p, Kind::Chicken, [208.5, -50.0, 208.5]);
    let diver = spawn(&mut p, Kind::Cow, [222.5, -54.0, 222.5]);
    step(&mut p, 60);
    let hit = evs(&p, "hit_ground");
    assert!(hit.iter().any(|e| e.1 == Some(Src::Living(Ent::Mob(pig)))), "pig without landing: {hit:?}");
    let flap = evs(&p, "flap");
    assert!(flap.iter().any(|e| e.1 == Some(Src::Living(Ent::Mob(chicken)))), "chicken without flap: {flap:?}");
    let splash = evs(&p, "splash");
    assert!(splash.iter().any(|e| e.1 == Some(Src::Living(Ent::Mob(diver)))), "cow without splash: {splash:?}");
    // Damage then death.
    let i = p.s.mob_index(pig).expect("pig");
    let mut m = p.s.mobs.list[i].take().expect("pig");
    p.s.mob_hurt(&mut m, "minecraft:generic", 1.0, None, None);
    m.damage_cooldown = 0;
    p.s.mob_hurt(&mut m, "minecraft:generic", 100.0, None, None);
    p.s.mobs.list[i] = Some(m);
    assert!(!evs(&p, "entity_damage").is_empty(), "damage without event");
    assert_eq!(evs(&p, "entity_die").len(), 1, "death without event");
    cleanup(p, dir);
}

/// Player steps: supporting block (carpet), swimming at the bottom, landing, gliding.
#[test]
fn player_step_events() {
    let Some((mut p, dir)) = probe("ev_steps") else { return };
    dummy_listener(&mut p);
    // Wool carpet: the vibration state is the carpet (dampened), not the stone below.
    p.run("fill 204 -59 204 215 -59 204 minecraft:white_carpet\nplayer 204.5 -58.9375 204.5 0 0\n");
    p.s.mobs.rec.vib.log.clear();
    let mut x = 204.5;
    for _ in 0..30 {
        x += 0.3;
        p.s.handle(Input::Move { pos: Some([x, -58.9375, 204.5]), rot: None, on_ground: true });
    }
    let st = evs(&p, "step");
    assert!(!st.is_empty(), "no step");
    assert!(
        st.iter().all(|e| block_name(&p, e.2) == "minecraft:white_carpet"),
        "steps outside carpet: {:?}",
        st.iter().map(|e| block_name(&p, e.2)).collect::<Vec<_>>()
    );
    // Sprint swimming at the bottom: `swim` without state, no `step`.
    p.run("fill 204 -59 208 215 -58 208 minecraft:water\nplayer 204.5 -59 208.5 0 0\n");
    p.s.life.vit.sprinting = true;
    p.s.handle(Input::Move { pos: Some([204.6, -59.0, 208.5]), rot: None, on_ground: true });
    p.s.mobs.rec.vib.log.clear();
    let mut x = 204.6;
    for _ in 0..30 {
        x += 0.3;
        // Swimming recomputed per tick, then the next tick's movement.
        p.s.sealife_player_tick();
        p.s.handle(Input::Move { pos: Some([x, -59.0, 208.5]), rot: None, on_ground: true });
    }
    assert!(p.s.mobs.sea.player_swimming, "swimming not recognized");
    assert!(evs(&p, "step").is_empty(), "steps while swimming: {:?}", evs(&p, "step"));
    assert!(!evs(&p, "swim").is_empty(), "silent swimming");
    p.s.life.vit.sprinting = false;
    // Climbable powder snow for the step accumulation.
    let snow = p.s.default_state("minecraft:powder_snow");
    assert!(p.s.climbable_state(snow));
    // Player landing.
    p.run("player 212.5 -56 212.5 0 0\n");
    p.s.mobs.rec.vib.log.clear();
    for y in [-56.5, -57.0, -57.5, -58.0, -58.5] {
        p.s.handle(Input::Move { pos: Some([212.5, y, 212.5]), rot: None, on_ground: false });
    }
    p.s.handle(Input::Move { pos: Some([212.5, -59.0, 212.5]), rot: None, on_ground: true });
    let hit = evs(&p, "hit_ground");
    assert_eq!(hit.len(), 1, "landing: {hit:?}");
    assert_eq!(block_name(&p, hit[0].2), "minecraft:stone");
    // Gliding: `elytra_glide` every 10 ticks.
    p.s.slots[6] = Some(crate::player::Stack { id: "minecraft:elytra".into(), count: 1, components: None });
    p.s.on_ground = false;
    p.s.flying = false;
    p.s.proj_player.fall_flying = true;
    p.s.mobs.rec.vib.log.clear();
    for _ in 0..20 {
        p.s.fall_flying_tick();
    }
    assert_eq!(evs(&p, "elytra_glide").len(), 2);
    cleanup(p, dir);
}

/// Attached sounds (shearing), seed drawn, sound variants (adult, baby).
#[test]
fn entity_sounds_and_variants() {
    let Some((mut p, dir)) = probe("ev_sounds") else { return };
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 204.5]);
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = Some(crate::player::Stack { id: "minecraft:shears".into(), count: 1, components: None });
    let _ = p.take_events();
    p.s.mobs.rec.vib.log.clear();
    assert!(p.s.player_interact(sheep, 0));
    let snd = sounds(&p, &p.take_events());
    let shear = snd.iter().find(|s| s.0 == "minecraft:entity.sheep.shear").expect("silent shearing");
    assert_eq!((shear.1, shear.2), (Some(sheep), None), "shearing sound not attached");
    assert_eq!(evs(&p, "shear").first().map(|e| e.1), Some(Some(Src::Living(Ent::Player))));
    // "big" variant of the pig: adult hurt then baby.
    let pig = spawn(&mut p, Kind::Pig, [220.5, -59.0, 204.5]);
    let big = p.s.regs.entries("minecraft:pig_sound_variant").iter().position(|e| e == "minecraft:big").expect("big");
    let i = p.s.mob_index(pig).expect("pig");
    let mut m = p.s.mobs.list[i].take().expect("pig");
    m.sound_variant = big as i32;
    let _ = p.take_events();
    p.s.mob_hurt(&mut m, "minecraft:generic", 1.0, None, None);
    let snd = sounds(&p, &p.take_events());
    assert!(has_sound(&snd, "entity.pig_big.hurt"), "variant ignored: {snd:?}");
    m.age = -24000;
    m.damage_cooldown = 0;
    p.s.mob_hurt(&mut m, "minecraft:generic", 1.0, None, None);
    let snd = sounds(&p, &p.take_events());
    assert!(has_sound(&snd, "entity.baby_pig.hurt"), "baby sound missing: {snd:?}");
    p.s.mobs.list[i] = Some(m);
    // Player's positional sound: seed drawn, not zero.
    p.s.play_sound("minecraft:entity.player.burp", 7, p.s.pos, 0.5, 1.0);
    p.s.play_sound("minecraft:entity.player.burp", 7, p.s.pos, 0.5, 1.0);
    let snd = sounds(&p, &p.take_events());
    let seeds: Vec<i64> = snd.iter().filter(|s| s.0 == "minecraft:entity.player.burp").map(|s| s.3).collect();
    assert_eq!(seeds.len(), 2);
    assert!(seeds[0] != 0 && seeds[0] != seeds[1], "seeds {seeds:?}");
    cleanup(p, dir);
}

/// Corrected sound names: present in the registry (otherwise the packet is never sent).
#[test]
fn fixed_sound_names_exist() {
    let Some(data) = rc_testdata::version_data() else { return };
    let reg = data.registry("minecraft:sound_event").expect("registry");
    for n in [
        "minecraft:entity.allay.item_thrown",
        "minecraft:entity.goat.horn_break",
        "minecraft:block.sniffer_egg.plop",
        "minecraft:entity.turtle.egg_break",
        "minecraft:entity.minecart.inside.underwater",
    ] {
        assert!(reg.entries.iter().any(|e| e == n), "{n} absent");
    }
    // Names derived from block sets (doors, fence gates, buttons).
    for (block, open) in [
        ("minecraft:oak_door", true),
        ("minecraft:iron_trapdoor", false),
        ("minecraft:cherry_fence_gate", true),
        ("minecraft:oak_fence_gate", false),
        ("minecraft:copper_door", true),
        ("minecraft:bamboo_trapdoor", true),
    ] {
        let n = crate::blocks::redstone::openable_sound_name(block, open);
        assert!(reg.entries.contains(&n), "{n} absent");
    }
    for b in [
        "minecraft:oak_button",
        "minecraft:stone_button",
        "minecraft:crimson_button",
        "minecraft:polished_blackstone_button",
    ] {
        let n = crate::blocks::redstone::button_sound(b, true);
        assert!(reg.entries.contains(&n), "{n} absent");
    }
}
