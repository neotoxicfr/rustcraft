//! G4-F08: monsters and entities. Opening shulker that pushes with collisions, primed TNT
//! power and block kept, sulfur cube explosion and contact damage, creaking attack sound,
//! warden targets and anger, breeze dangers, evoker summons, creaking sight, bat ceiling and
//! silence, spider path target, body rotation under mob passengers, game events. Fresh flat
//! world (no test world required; 26.3 version data mandatory).

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::Meta;

use crate::blocks::entities::EntKind;
use crate::blocks::probe::Probe;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::tests_g3f13::{finish, open};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn mobs(p: &Probe, k: Kind) -> Vec<&Mob> {
    p.s.mobs.of_kind(k).collect()
}

fn close(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() < 1.0e-4, "{what}: {got} instead of {want}");
}

/// A shulker on the floor opening completely sweeps one block upward and moves what it finds
/// there with its collisions: a zombie under a ceiling rises only to it (0.05), an item and
/// an orb rise the whole block; a vex (no physics) stays in place. Before: only mobs, placed
/// through the ceiling.
#[test]
fn opening_shulker_pushes_entities_with_collisions() {
    let (mut p, dir) = open("shulkerpush");
    cmd(&mut p, "setblock 4 -57 0 minecraft:stone");
    for x in [4.5, 20.5, 24.5] {
        cmd(&mut p, &format!("summon minecraft:shulker {x} -60 0.5 {{NoAI:1b,Peek:100b,PersistenceRequired:1b}}"));
    }
    cmd(&mut p, "summon minecraft:zombie 4.5 -59 0.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(&mut p, "summon minecraft:item 20.5 -59 0.5 {Item:{id:\"minecraft:stone\",count:1}}");
    cmd(&mut p, "summon minecraft:experience_orb 20.5 -59 0.5 {Value:3}");
    cmd(&mut p, "summon minecraft:vex 24.5 -59 0.5 {NoAI:1b,PersistenceRequired:1b}");
    // Weightless bodies: only the push moves them.
    p.s.life.drops[0].body.no_gravity = true;
    p.s.life.orbs[0].body.no_gravity = true;
    game_ticks(&mut p, 25);
    assert!(mobs(&p, Kind::Shulker).iter().all(|m| m.end.peek_amount == 1.0), "shulkers not open");
    close(mobs(&p, Kind::Zombie)[0].pos[1], -59.0 + 0.05, "zombie under the ceiling");
    close(p.s.life.drops[0].pos[1], -58.0, "item");
    close(p.s.life.orbs[0].pos[1], -58.0, "orb");
    close(mobs(&p, Kind::Vex)[0].pos[1], -59.0, "vex");
    finish(p, dir);
}

fn data_shows(events: &[Event], index: u8, want: &Meta) -> bool {
    events.iter().any(|e| {
        matches!(e, Event::Entity(EntityEvent::Data { values, .. }) if values.iter().any(|(i, v)| *i == index && v == want))
    })
}

/// Primed TNT: the `explosion_power` and `block_state` of its NBT count. The pairing shows the
/// block (data 9), a power of 0 leaves the ground whole where the default one digs it, and both
/// are written with the chunk then read back (with the falling block fields). Before: power 4
/// always, block ignored, neither entity saved.
#[test]
fn primed_tnt_keeps_power_and_block_through_a_save() {
    let (mut p, dir) = open("tntnbt");
    p.take_events();
    cmd(&mut p, "summon minecraft:tnt 30.5 -60 0.5 {fuse:5,explosion_power:0.0f,block_state:{Name:\"minecraft:stone\"}}");
    let stone = p.s.default_state("minecraft:stone");
    assert!(data_shows(&p.take_events(), 9, &Meta::State(stone as i32)), "block of the TNT not sent");
    cmd(&mut p, "summon minecraft:tnt 50.5 -60 0.5 {fuse:5}");
    game_ticks(&mut p, 8);
    assert!(p.s.bs.entities.list.iter().all(|e| e.removed), "TNT left");
    assert!(!p.s.is_air([30, -61, 0]), "a power of 0 dug the ground");
    assert!(p.s.is_air([50, -61, 0]), "the default power did not dig the ground");
    let log = "{Name:\"minecraft:oak_log\",Properties:{axis:\"x\"}}";
    cmd(&mut p, &format!("summon minecraft:tnt 70.5 -60 0.5 {{fuse:300,explosion_power:2.5f,block_state:{log}}}"));
    cmd(
        &mut p,
        "summon minecraft:falling_block 80.5 -60 0.5 {BlockState:{Name:\"minecraft:anvil\"},Time:5,\
         HurtEntities:1b,FallHurtAmount:2.0f,FallHurtMax:30,DropItem:0b}",
    );
    p.s.save(true);
    let data = p.s.data.clone();
    drop(p);
    let p = crate::tests_g3f13::reopen(&dir, data);
    let live: Vec<_> = p.s.bs.entities.list.iter().filter(|e| !e.removed).collect();
    let tnt = live.iter().find(|e| matches!(e.kind, EntKind::Tnt(_))).expect("TNT not read back");
    let oak = p.s.states.state_of(&p.s.data, &crate::commands::snbt::parse(log).unwrap()).unwrap();
    assert!(matches!(tnt.kind, EntKind::Tnt(300)), "{:?}", tnt.kind);
    assert_eq!((tnt.power, tnt.block), (2.5, Some(oak)));
    let anvil = live.iter().find(|e| matches!(e.kind, EntKind::Falling(_))).expect("falling block not read back");
    assert!(matches!(anvil.kind, EntKind::Falling(s) if s == p.s.default_state("minecraft:anvil")));
    assert_eq!((anvil.time, anvil.hurt, anvil.drop_item), (5, Some((2.0, 30)), false));
    finish(p, dir);
}

fn mob_at(p: &Probe, k: Kind, x: f64) -> &Mob {
    p.s.mobs.of_kind(k).find(|m| (m.pos[0] - x).abs() < 0.6).unwrap_or_else(|| panic!("no {k:?} at x = {x}"))
}

/// Warden targets: a mob of the warden's team, an invulnerable one or one outside the world
/// border is not a target, a plain one is. A suspect mob gone to another dimension keeps its
/// anger by UUID at the next anger tick. Before: alive and not a warden was enough, and the
/// anger of the suspect gone elsewhere was forgotten.
#[test]
fn warden_targets_and_anger_of_a_suspect_gone_elsewhere() {
    use crate::mobs::mob::Ent;
    use crate::mobs::recent::warden::can_target;
    let (mut p, dir) = open("warden");
    cmd(&mut p, "summon minecraft:warden 80.5 -60 0.5 {PersistenceRequired:1b}");
    for (x, extra) in [(82.5, ""), (84.5, ",Invulnerable:1b"), (30.5, ""), (86.5, "")] {
        cmd(&mut p, &format!("summon minecraft:zombie {x} -60 0.5 {{NoAI:1b,PersistenceRequired:1b{extra}}}"));
    }
    let uuid_of = |p: &Probe, k: Kind, x: f64| crate::nbtio::uuid_text(mob_at(p, k, x).uuid);
    let (warden, mate) = (uuid_of(&p, Kind::Warden, 80.5), uuid_of(&p, Kind::Zombie, 82.5));
    cmd(&mut p, "team add pack");
    cmd(&mut p, &format!("team join pack {warden}"));
    cmd(&mut p, &format!("team join pack {mate}"));
    cmd(&mut p, "worldborder center 80 0");
    cmd(&mut p, "worldborder set 60");
    let w = mob_at(&p, Kind::Warden, 80.5);
    let target = |x: f64| can_target(&p.s, w, Ent::Mob(mob_at(&p, Kind::Zombie, x).id));
    assert!(!target(82.5), "team mate targeted");
    assert!(!target(84.5), "invulnerable mob targeted");
    assert!(!target(30.5), "mob outside the border targeted");
    assert!(target(86.5), "plain mob not targeted");
    // A zombie standing in an End portal, suspect of the warden with anger 50.
    cmd(&mut p, "setblock 60 -60 0 minecraft:end_portal");
    cmd(&mut p, "summon minecraft:zombie 60.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    let z = mob_at(&p, Kind::Zombie, 60.5);
    let (zid, zuuid) = (z.id, z.uuid);
    let wid = mob_at(&p, Kind::Warden, 80.5).id;
    let i = p.s.mob_index(wid).expect("warden");
    p.s.mobs.list[i].as_mut().unwrap().rec.anger.suspects.push((Ent::Mob(zid), zuuid, 50));
    game_ticks(&mut p, 25);
    assert!(p.s.mob_ref(zid).is_none(), "the zombie did not go to the End");
    let w = p.s.mob_ref(wid).expect("warden");
    let kept = w.rec.anger.by_uuid.iter().find(|e| e.0 == zuuid).map(|e| e.1);
    assert!(kept.is_some_and(|v| (40..50).contains(&v)), "anger by UUID: {kept:?} ({:?})", w.rec.anger);
    finish(p, dir);
}

/// Mob of kind `k` at `x` taken out of the list (its slot, the mob).
fn take(p: &mut Probe, k: Kind, x: f64) -> (usize, Mob) {
    let id = mob_at(p, k, x).id;
    let i = p.s.mob_index(id).expect("slot");
    (i, *p.s.mobs.list[i].take().expect("mob"))
}

fn put(p: &mut Probe, i: usize, m: Mob) {
    p.s.mobs.list[i] = Some(Box::new(m));
}

fn stack(id: &str, count: i32) -> Option<crate::player::Stack> {
    Some(crate::player::Stack { id: id.into(), count, components: None })
}

fn sounds(p: &Probe, evs: &[Event], name: &str) -> usize {
    use rc_proto_v777::play::SoundRef;
    let id = p.s.sound_id(name).expect("known sound");
    evs.iter().filter(|e| matches!(e, Event::Sound(s) if s.sound == SoundRef::Id(id))).count()
}

/// Non-air blocks of the box `lo..=hi`, fire apart (count, fires).
fn census(p: &Probe, lo: [i32; 3], hi: [i32; 3]) -> (usize, usize) {
    let (mut solid, mut fire) = (0, 0);
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                match p.s.bname([x, y, z]) {
                    "minecraft:air" => {}
                    "minecraft:fire" => fire += 1,
                    _ => solid += 1,
                }
            }
        }
    }
    (solid, fire)
}

/// Sulfur cube. Its explosion has the TNT interaction with the blocks (mob griefing on): every
/// destroyed block drops (no decay) and the archetype's fire is placed; a burning cube hurts the
/// mob it pushes; priming refused by the rule shows its message; priming with the last fire
/// charge counts as "used: air". Before: a mob's explosion (decay, no fire), contact damage on the
/// player only, no message, no statistic.
#[test]
fn sulfur_cube_explosion_contact_and_priming() {
    const BODY: usize = 6; // body slot of the equipment
    let (mut p, dir) = open("sulfur");
    for x in [40.5, 50.5, 60.5] {
        cmd(&mut p, &format!("summon minecraft:sulfur_cube {x} -60 0.5 {{PersistenceRequired:1b}}"));
    }
    cmd(&mut p, "summon minecraft:zombie 50.8 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    for (x, item) in [(40.5, "minecraft:tnt"), (50.5, "minecraft:magma_block"), (60.5, "minecraft:tnt")] {
        let (i, mut m) = take(&mut p, Kind::SulfurCube, x);
        m.equipment[BODY] = stack(item, 1);
        put(&mut p, i, m);
    }
    game_ticks(&mut p, 1);
    let (i, mut m) = take(&mut p, Kind::SulfurCube, 40.5);
    assert!(m.cube.explosion.is_some(), "explosive archetype not applied");
    m.cube.explosion = Some((3, true, 120));
    m.cube.fuse = 1;
    put(&mut p, i, m);
    let (lo, hi) = ([34, -64, -6], [46, -55, 6]);
    let (before, _) = census(&p, lo, hi);
    game_ticks(&mut p, 1);
    let (after, fires) = census(&p, lo, hi);
    let dirt: i32 = p.s.life.drops.iter().filter(|d| d.stack.id == "minecraft:dirt").map(|d| d.stack.count).sum();
    assert!(before > after, "nothing destroyed");
    assert_eq!(dirt as usize, before - after, "dropped dirt against destroyed blocks");
    assert!(fires > 0, "fire of the archetype not placed");
    // Contact damage on the pushed zombie.
    game_ticks(&mut p, 3);
    let z = mob_at(&p, Kind::Zombie, 50.8);
    assert!(z.health < 20.0, "zombie pushed by the burning cube not hurt: {}", z.health);
    // Priming refused by the rule: message; allowed: the last fire charge counts as air.
    cmd(&mut p, "gamerule minecraft:tnt_explodes false");
    let slot = p.s.hand_slot(0);
    p.s.slots[slot] = stack("minecraft:flint_and_steel", 1);
    p.take_events();
    let (i, mut m) = take(&mut p, Kind::SulfurCube, 60.5);
    assert_eq!(p.s.sulfur_interact(&mut m, slot), Some(false));
    let shown = p.take_events().into_iter().any(|e| {
        matches!(e, Event::ActionBar(c) if matches!(&c.content, rc_text::Content::Translate { key, .. } if key == "block.minecraft.tnt.disabled"))
    });
    assert!(shown, "no message for the refused priming");
    cmd(&mut p, "gamerule minecraft:tnt_explodes true");
    p.s.slots[slot] = stack("minecraft:fire_charge", 1);
    assert_eq!(p.s.sulfur_interact(&mut m, slot), Some(true));
    put(&mut p, i, m);
    assert_eq!(p.s.prog.stats.get("minecraft:used", "minecraft:air"), 1, "statistic of the last fire charge");
    finish(p, dir);
}

/// The creaking's landed hit plays its attack sound. Before: silent.
#[test]
fn creaking_hit_plays_its_attack_sound() {
    use crate::mobs::mob::Ent;
    let (mut p, dir) = open("creaking");
    cmd(&mut p, "summon minecraft:creaking 70.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(&mut p, "summon minecraft:zombie 71.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    let zid = mob_at(&p, Kind::Zombie, 71.5).id;
    p.take_events();
    let (i, mut c) = take(&mut p, Kind::Creaking, 70.5);
    assert!(p.s.mob_attack(&mut c, Ent::Mob(zid)), "hit not landed");
    put(&mut p, i, c);
    let evs = p.take_events();
    assert_eq!(sounds(&p, &evs, "minecraft:entity.creaking.attack"), 1);
    finish(p, dir);
}

/// Dangerous blocks per species (breeze jump, spawning): the lava cauldron burns, a fire-immune
/// strider fears no fire, a fox fears no berry bush (its "immune to" tag), a zombie fears both;
/// copper bars are transparent to the creaking's gaze. Before: lava cauldron safe for the
/// breeze, fire refused for every species, copper bars opaque.
#[test]
fn dangerous_blocks_per_species_and_copper_bars_sight() {
    let (mut p, dir) = open("danger");
    cmd(&mut p, "setblock 80 -60 0 minecraft:lava_cauldron");
    cmd(&mut p, "setblock 81 -60 0 minecraft:fire");
    cmd(&mut p, "setblock 82 -60 0 minecraft:sweet_berry_bush");
    cmd(&mut p, "setblock 83 -60 0 minecraft:waxed_copper_bars");
    let d = |k: Kind, x: i32| p.s.block_dangerous([x, -60, 0], k);
    assert!(d(Kind::Breeze, 80), "lava cauldron safe for the breeze");
    assert!(!d(Kind::Strider, 81) && d(Kind::Zombie, 81), "fire");
    assert!(!d(Kind::Fox, 82) && d(Kind::Zombie, 82), "berry bush");
    assert!(p.s.visual_boxes([83, -60, 0]).is_empty(), "copper bars block the gaze");
    finish(p, dir);
}

/// Evoker: fangs and each vex announce an "entity placed" event from the evoker, and the vexes
/// join the evoker's team. Before: no event, no team.
#[test]
fn evoker_summons_emit_events_and_share_the_team() {
    use crate::mobs::mob::Ent;
    use crate::mobs::recent::vibration::Src;
    let (mut p, dir) = open("evoker");
    cmd(&mut p, "summon minecraft:evoker 90.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    let eu = crate::nbtio::uuid_text(mob_at(&p, Kind::Evoker, 90.5).uuid);
    cmd(&mut p, "team add coven");
    cmd(&mut p, &format!("team join coven {eu}"));
    p.s.mobs.rec.vib.log.clear();
    let (i, mut e) = take(&mut p, Kind::Evoker, 90.5);
    let eid = e.id;
    p.s.create_fangs(&e, 92.5, 0.5, -60.0, -59.0, 0.0, 0);
    p.s.summon_vexes(&mut e);
    put(&mut p, i, e);
    let placed: Vec<_> = p.s.mobs.rec.vib.log.iter().filter(|l| l.0 == "minecraft:entity_place").collect();
    assert_eq!(placed.len(), 4, "{placed:?}");
    assert!(placed.iter().all(|l| l.2 == Some(Src::Living(Ent::Mob(eid)))), "{placed:?}");
    let vexes: Vec<String> = p.s.mobs.of_kind(Kind::Vex).map(|v| crate::nbtio::uuid_text(v.uuid)).collect();
    assert_eq!(vexes.len(), 3);
    assert!(vexes.iter().all(|v| p.s.cmd.scores.team_of(v).is_some_and(|t| t.name == "coven")), "vex outside the team");
    finish(p, dir);
}

/// Bat: a redstone block above (opaque, not a conductor) does not hold a resting bat, and a
/// silent bat takes off without the 1025 event. Before: held by any opaque block, event always.
#[test]
fn bat_ceiling_is_a_conductor_and_silent_bat_takes_off_quietly() {
    let (mut p, dir) = open("bat");
    for (x, top, extra) in [(100, "minecraft:redstone_block", ""), (102, "minecraft:stone", ""), (104, "minecraft:glass", ",Silent:1b")] {
        cmd(&mut p, &format!("setblock {x} -54 0 {top}"));
        let c = f64::from(x) + 0.5;
        cmd(&mut p, &format!("summon minecraft:bat {c} -54.9 0.5 {{BatFlags:1b,PersistenceRequired:1b{extra}}}"));
    }
    let mut resting = Vec::new();
    p.take_events();
    for x in [100.5, 102.5, 104.5] {
        let (i, mut b) = take(&mut p, Kind::Bat, x);
        crate::mobs::surface::bat::custom_ai_step(&mut p.s, &mut b);
        resting.push(b.surf.bat_flags & 1 != 0);
        put(&mut p, i, b);
    }
    assert_eq!(resting, [false, true, false], "redstone block, stone, glass");
    let events: Vec<[i32; 3]> = p
        .take_events()
        .into_iter()
        .filter_map(|e| if let Event::LevelEvent { kind: 1025, pos, .. } = e { Some(pos) } else { None })
        .collect();
    assert_eq!(events, [[100, -55, 0]], "take-off events");
    finish(p, dir);
}

/// A wall climber keeps the asked position as is (here in the air), not the surface below it.
#[test]
fn spider_keeps_the_raw_path_target() {
    let (mut p, dir) = open("spider");
    cmd(&mut p, "summon minecraft:spider 110.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    let (i, mut s) = take(&mut p, Kind::Spider, 110.5);
    let _ = p.s.create_path(&mut s, [110, -50, 6], 0);
    assert_eq!(s.nav.path_to, Some([110, -50, 6]));
    put(&mut p, i, s);
    finish(p, dir);
}

/// A standing mob whose first passenger is a mob keeps its body rotation when its head turns
/// (a spider jockey); without a passenger the body follows. Before: both followed.
#[test]
fn mob_carrying_a_mob_keeps_its_body_rotation() {
    let (mut p, dir) = open("jockey");
    cmd(&mut p, "summon minecraft:spider 120.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(&mut p, "summon minecraft:skeleton 120.5 -59 0.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(&mut p, "summon minecraft:spider 124.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    let (i, mut spider) = take(&mut p, Kind::Spider, 120.5);
    let (j, mut skeleton) = take(&mut p, Kind::Skeleton, 120.5);
    assert!(p.s.mob_start_riding(&mut skeleton, &mut spider, true), "skeleton not mounted");
    put(&mut p, i, spider);
    put(&mut p, j, skeleton);
    game_ticks(&mut p, 2);
    for x in [120.5, 124.5] {
        let (i, mut s) = take(&mut p, Kind::Spider, x);
        (s.yaw, s.body_yaw, s.head_yaw) = (0.0, 0.0, 100.0);
        put(&mut p, i, s);
    }
    game_ticks(&mut p, 1);
    assert_eq!(mob_at(&p, Kind::Spider, 120.5).body_yaw, 0.0, "jockey turned its body");
    assert!(mob_at(&p, Kind::Spider, 124.5).body_yaw != 0.0, "plain spider kept its body");
    finish(p, dir);
}

/// Game events: a digging sniffer acts every 10 ticks at its head, a laying turtle on sand acts,
/// a rabbit eating one age of a carrot changes the block; the sniffer digs only inside the
/// world border. Before: none of these events, and digging spots outside the border.
#[test]
fn sniffer_turtle_rabbit_game_events_and_sniffer_border() {
    use crate::mobs::brainy::sniffer;
    let (mut p, dir) = open("events");
    cmd(&mut p, "summon minecraft:sniffer 130.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(&mut p, "setblock 136 -61 0 minecraft:sand");
    cmd(&mut p, "summon minecraft:turtle 136.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(&mut p, "setblock 140 -61 0 minecraft:farmland");
    cmd(&mut p, "setblock 140 -60 0 minecraft:carrots[age=7]");
    cmd(&mut p, "summon minecraft:rabbit 140.5 -60 1.5 {NoAI:1b,PersistenceRequired:1b}");
    p.s.mobs.rec.vib.log.clear();
    let (i, mut sn) = take(&mut p, Kind::Sniffer, 130.5);
    sn.bm.sniffer = sniffer::DIGGING;
    sn.tick_count = 20;
    sniffer::pre_tick(&mut p.s, &mut sn);
    sn.tick_count = 21;
    sniffer::pre_tick(&mut p.s, &mut sn);
    sn.bm.sniffer = 0;
    put(&mut p, i, sn);
    let (i, mut t) = take(&mut p, Kind::Turtle, 136.5);
    (t.surf.laying, t.surf.lay_counter) = (true, 5);
    crate::mobs::surface::turtle::post_ai_step(&mut p.s, &mut t);
    put(&mut p, i, t);
    let (i, mut r) = take(&mut p, Kind::Rabbit, 140.5);
    crate::mobs::surface::rabbit::eat_carrot(&mut p.s, &mut r, [140, -61, 0]);
    put(&mut p, i, r);
    let names: Vec<&str> = p.s.mobs.rec.vib.log.iter().map(|l| l.0.as_str()).collect();
    assert_eq!(names, ["minecraft:entity_action", "minecraft:entity_action", "minecraft:block_change"]);
    // Border of 1 block around the sniffer: every digging spot (10 blocks and more) is outside.
    cmd(&mut p, "worldborder center 130.5 0.5");
    cmd(&mut p, "worldborder set 1");
    let (i, mut sn) = take(&mut p, Kind::Sniffer, 130.5);
    let brain = sn.bm.brain.take().expect("brain");
    let spots: Vec<_> = (0..20).filter_map(|_| sniffer::dig_position(&mut p.s, &mut sn, &brain)).collect();
    sn.bm.brain = Some(brain);
    put(&mut p, i, sn);
    assert!(spots.is_empty(), "digging spots outside the border: {spots:?}");
    finish(p, dir);
}

/// Micro-bench (ignored, `--ignored --nocapture`): one push of an opening shulker whose slice
/// touches nothing, among 300 mobs, 1,000 items and 100 orbs: the former scan (mobs only)
/// against the new one (mobs, primed TNT and falling blocks, items, orbs).
#[test]
#[ignore]
fn bench_g4f08_shulker_push_scan() {
    let (mut p, dir) = open("benchshulker");
    for i in 0..300 {
        let m = p.s.new_mob(Kind::Zombie, [f64::from(i % 30) + 20.5, -60.0, f64::from(i / 30) + 0.5], 0.0);
        p.s.add_mob(m);
    }
    for i in 0..1000 {
        let pos = [f64::from(i % 50) + 20.5, -60.0, f64::from(i / 50) + 20.5];
        p.s.spawn_item(pos, [0.0; 3], stack("minecraft:stone", 1).unwrap(), 0);
    }
    for i in 0..100 {
        p.s.summon_orb([f64::from(i) + 20.5, -60.0, 60.5], None);
    }
    let sh = p.s.new_mob(Kind::Shulker, [200.5, -60.0, 0.5], 0.0);
    let area = crate::mobs::end::shulker::progress_delta_box(1, 0.1, 0.2, sh.pos);
    for _ in 0..2 {
        let t0 = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..1000 {
            n += p.s.mobs.list.iter().flatten().filter(|o| o.id != sh.id && o.alive() && o.touches(&area)).count();
        }
        let old = t0.elapsed().as_secs_f64() * 1.0e3;
        let t0 = std::time::Instant::now();
        for _ in 0..1000 {
            p.s.shulker_shove(&sh, &area, [0.0, 0.05, 0.0]);
        }
        let new = t0.elapsed().as_secs_f64() * 1.0e3;
        eprintln!("shulker push scan: former {old:.2} us, new {new:.2} us per push ({n} touched)");
    }
    finish(p, dir);
}
