//! Rules every living entity shares in the game, read once and applied to each mob: the reaction
//! to an equipment write (sound and game event), the team handed over by a conversion, the "seen
//! as an enemy" test of a target, and the ghast's own invulnerability.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::Event;
use rc_proto_v777::play::{SoundPacket, SoundRef};

use crate::blocks::probe::Probe;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;
use crate::newworld::{NewWorld, Preset, create_world};

fn version_data() -> Arc<rc_data::VersionData> {
    let data = Arc::new(rc_testdata::version_data().expect("living entity rule tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "living entity rule tests: 26.3 client jar missing");
    data
}

/// Flat world, a survival player on the ground at the origin, chunks around loaded.
fn new_world(tag: &str) -> (Probe, PathBuf) {
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-living-rules-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    w.allow_commands = true;
    create_world(&dir, &version_data(), &w).expect("world creation");
    let mut p = Probe::open(&dir, version_data());
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.teleport = None;
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    let _ = p.take_events();
    (p, dir)
}

/// Events a server command sends.
fn run(p: &mut Probe, text: &str) -> Vec<Event> {
    let src = p.s.server_source();
    assert!(p.s.perform(src, text).0, "command refused: {text}");
    p.take_events()
}

/// The only mob of species `kind`.
fn only(p: &Probe, kind: Kind) -> &Mob {
    let mut of_kind = p.s.mobs.list.iter().filter(|m| m.kind == kind);
    let m = of_kind.next().unwrap_or_else(|| panic!("no {kind:?}"));
    assert!(of_kind.next().is_none(), "more than one {kind:?}");
    m
}

fn mob_mut(p: &mut Probe, id: i32) -> &mut Mob {
    p.s.mobs.get_mut(id).expect("mob in the list")
}

/// Sounds named `name` among `events`.
fn sounds<'a>(p: &Probe, events: &'a [Event], name: &str) -> Vec<&'a SoundPacket> {
    let id = p.s.sound_id(name).unwrap_or_else(|| panic!("unknown sound {name}"));
    events
        .iter()
        .filter_map(|e| match e {
            Event::Sound(s) if matches!(s.sound, SoundRef::Id(i) if i == id) => Some(s),
            _ => None,
        })
        .collect()
}

/// Game events named `name` given off since the log was last cleared, then the log cleared.
fn game_events(p: &mut Probe, name: &str) -> usize {
    let n = p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == name).count();
    p.s.mobs.rec.vib.log.clear();
    n
}

/// A command writing a mob's equipment goes through the living entity's reaction, like the game:
/// a wearable stack put in its own slot plays its equip sound (in the mob's sound source) and gives
/// off `equip`; a stack that is not wearable gives off `unequip` in silence; the same stack again
/// does nothing; a silent mob gives off the event without the sound; a mob not ticked yet equips in
/// silence. An armor stand reacts the same way. Fails on the code before (no sound, no event for a
/// mob or a stand written by a command); sabotage verified (failure): the first-tick test removed,
/// the silence test removed.
#[test]
fn a_command_equipping_a_mob_plays_its_equip_sound_and_game_event() {
    let (mut p, _dir) = new_world("command-equip");
    run(&mut p, "summon minecraft:zombie 0.5 -60 4.5 {NoAI:1b,PersistenceRequired:1b}");
    let zombie = only(&p, Kind::Zombie).id;
    let iron = "minecraft:item.armor.equip_iron";
    p.s.mobs.rec.vib.log.clear();

    let leather = run(&mut p, "item replace entity @e[type=minecraft:zombie] armor.head with minecraft:leather_helmet");
    assert!(sounds(&p, &leather, "minecraft:item.armor.equip_leather").is_empty(), "not ticked yet: silent");
    assert_eq!(game_events(&mut p, "minecraft:equip"), 0, "not ticked yet: no event");

    mob_mut(&mut p, zombie).tick_count = 1;
    let helmet = run(&mut p, "item replace entity @e[type=minecraft:zombie] armor.head with minecraft:iron_helmet");
    let played = sounds(&p, &helmet, iron);
    assert_eq!(played.len(), 1, "equip sound of the iron helmet");
    let source = crate::server::Server::mob_sound_source(only(&p, Kind::Zombie));
    assert_eq!((played[0].category, played[0].volume, played[0].pitch), (source, 1.0, 1.0));
    assert_eq!(game_events(&mut p, "minecraft:equip"), 1);

    let again = run(&mut p, "item replace entity @e[type=minecraft:zombie] armor.head with minecraft:iron_helmet");
    assert!(sounds(&p, &again, iron).is_empty(), "same stack: nothing");
    assert_eq!(game_events(&mut p, "minecraft:equip") + game_events(&mut p, "minecraft:unequip"), 0);

    let stick = run(&mut p, "item replace entity @e[type=minecraft:zombie] weapon.mainhand with minecraft:stick");
    assert!(!stick.iter().any(|e| matches!(e, Event::Sound(_))), "a stick is not wearable: no sound");
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1);

    mob_mut(&mut p, zombie).x.silent = true;
    let quiet = run(&mut p, "item replace entity @e[type=minecraft:zombie] armor.chest with minecraft:iron_chestplate");
    assert!(sounds(&p, &quiet, iron).is_empty(), "a silent mob plays no sound");
    assert_eq!(game_events(&mut p, "minecraft:equip"), 1, "but gives off the event");

    run(&mut p, "summon minecraft:armor_stand 2.5 -60 4.5");
    let stand = run(&mut p, "item replace entity @e[type=minecraft:armor_stand] armor.head with minecraft:iron_helmet");
    assert_eq!(sounds(&p, &stand, iron).len(), 1, "the armor stand's equip sound");
    assert_eq!(game_events(&mut p, "minecraft:equip"), 1, "the armor stand's event");
}

/// A saddle put on a pig by the common reaction keeps the pig's own saddle sound.
#[test]
fn a_saddle_keeps_the_species_own_equip_sound() {
    let (mut p, _dir) = new_world("saddle-sound");
    run(&mut p, "summon minecraft:pig 0.5 -60 4.5 {NoAI:1b}");
    let pig = only(&p, Kind::Pig).id;
    mob_mut(&mut p, pig).tick_count = 1;
    let saddled = run(&mut p, "item replace entity @e[type=minecraft:pig] saddle with minecraft:saddle");
    assert_eq!(sounds(&p, &saddled, "minecraft:entity.pig.saddle").len(), 1);
    assert!(sounds(&p, &saddled, "minecraft:item.armor.equip_generic").is_empty());
}

/// `/summon` reads the compound's passengers like the game: each mob passenger is made at the
/// summon's position and rides its vehicle, the vehicle entering the world first; no spawn
/// finalisation once a compound is given (the skeleton gets no bow). Fails on the code before
/// (the passengers were ignored); sabotage verified (failure): passengers left out.
#[test]
fn summon_reads_the_passengers_of_its_compound() {
    let (mut p, _dir) = new_world("summon-passengers");
    let nbt = "{NoAI:1b,Passengers:[{id:\"minecraft:skeleton\",Passengers:[{id:\"minecraft:chicken\"}]}]}";
    run(&mut p, &format!("summon minecraft:spider 0.5 -60 6.5 {nbt}"));
    let order: Vec<Kind> = p.s.mobs.list.iter().map(|m| m.kind).collect();
    assert_eq!(order, vec![Kind::Spider, Kind::Skeleton, Kind::Chicken], "vehicle first, then its passengers");
    let (spider, skeleton, chicken) = (only(&p, Kind::Spider), only(&p, Kind::Skeleton), only(&p, Kind::Chicken));
    assert_eq!(spider.mount.passengers, vec![Ent::Mob(skeleton.id)]);
    assert_eq!((skeleton.mount.vehicle, chicken.mount.vehicle), (Some(spider.id), Some(skeleton.id)));
    assert_eq!(skeleton.pos(), [0.5, -60.0, 6.5], "placed at the summon's position");
    assert!(skeleton.equipment[0].is_none(), "not finalised: no bow");
}

/// Name of the team of the mob of `uuid`.
fn team_of(p: &Probe, uuid: u128) -> Option<String> {
    p.s.cmd.scores.team_of_uuid(uuid).map(|t| t.name.clone())
}

/// Takes the only mob of `kind` out of the list, runs `convert` on it, puts it back.
fn convert(p: &mut Probe, kind: Kind, convert: impl FnOnce(&mut crate::server::Server, &mut Mob) -> bool) -> u128 {
    let id = only(p, kind).id;
    let at = p.s.mobs.index_of(id).expect("listed");
    let mut m = p.s.mobs.list.take(at).expect("mob");
    assert!(convert(&mut p.s, &mut m), "{kind:?} converted");
    let uuid = m.uuid;
    p.s.mobs.list.put(at, m);
    uuid
}

/// Every conversion hands the old mob's team to the new one and takes the old one out of it, like
/// the game's common conversion: a pig struck by lightning, a villager turned witch (now through
/// the common conversion, which also hands over the rest: name, effects, flags), each cube of a
/// split. Fails on the code before (the new mobs had no team, the old ones kept theirs).
#[test]
fn a_converted_mob_joins_the_team_of_the_old_one() {
    let (mut p, _dir) = new_world("conversion-team");
    // Outside peaceful: lightning converts (a fresh world is already in normal difficulty).
    assert_ne!(p.s.level.difficulty_id(), 0);
    run(&mut p, "team add reds");
    run(&mut p, "summon minecraft:pig 0.5 -60 4.5 {NoAI:1b}");
    run(&mut p, "summon minecraft:villager 4.5 -60 4.5 {NoAI:1b,CustomName:\"Bob\"}");
    run(&mut p, "summon minecraft:slime 8.5 -60 4.5 {NoAI:1b,Size:1}");
    run(&mut p, "team join reds @e[type=!minecraft:player]");

    let pig = convert(&mut p, Kind::Pig, |s, m| s.pig_struck(m));
    assert_eq!(team_of(&p, only(&p, Kind::ZombifiedPiglin).uuid).as_deref(), Some("reds"));
    assert_eq!(team_of(&p, pig), None, "the struck pig left the team");

    let villager = convert(&mut p, Kind::Villager, |s, m| s.villager_struck(m));
    let witch = only(&p, Kind::Witch);
    assert_eq!(team_of(&p, witch.uuid).as_deref(), Some("reds"));
    assert!(witch.no_ai && witch.custom_name.is_some(), "the common conversion's flags and name");
    assert_eq!(team_of(&p, villager), None);

    let slime = convert(&mut p, Kind::Slime, |s, m| {
        m.health = 0.0;
        s.cube_split(m);
        true
    });
    let cubes: Vec<u128> = p.s.mobs.list.iter().filter(|m| m.kind == Kind::Slime && m.uuid != slime).map(|m| m.uuid).collect();
    assert!(cubes.len() >= 2, "split into {}", cubes.len());
    for cube in cubes {
        assert_eq!(team_of(&p, cube).as_deref(), Some("reds"), "every cube of the split");
    }
    assert_eq!(team_of(&p, slime), None);
}

/// An invulnerable mob, by its flag or for a while, is not seen as an enemy: an iron golem does not
/// take it as a target, like the game's targeting. Fails on the code before (the golem took it).
/// The evoker's fangs skip it too (no change there: the hit was already refused).
#[test]
fn an_invulnerable_mob_is_not_seen_as_an_enemy() {
    let (mut p, _dir) = new_world("seen-as-enemy");
    run(&mut p, "summon minecraft:iron_golem 0.5 -60 4.5 {NoAI:1b}");
    run(&mut p, "summon minecraft:zombie 2.5 -60 4.5 {NoAI:1b}");
    let golem = only(&p, Kind::IronGolem).id;
    let zombie = only(&p, Kind::Zombie).id;
    let targetable = |p: &mut Probe| {
        let at = p.s.mobs.index_of(golem).expect("golem");
        let mut g = p.s.mobs.list.take(at).expect("golem");
        let ok = p.s.targeting_ok(&mut g, Ent::Mob(zombie), true, 16.0, false, false);
        p.s.mobs.list.put(at, g);
        ok
    };
    assert!(targetable(&mut p), "an ordinary zombie is a target");
    mob_mut(&mut p, zombie).invulnerable = true;
    assert!(!targetable(&mut p), "invulnerable by its flag");
    mob_mut(&mut p, zombie).invulnerable = false;
    mob_mut(&mut p, zombie).temporary_invulnerability_ticks = 40;
    assert!(!targetable(&mut p), "invulnerable for a while");
}

/// An invulnerable ghast takes no hit from a creative player: the game's ghast keeps its own
/// invulnerability rule, without the creative exception every other mob has. Fails on the code
/// before (the creative hit landed).
#[test]
fn an_invulnerable_ghast_takes_no_hit_from_a_creative_player() {
    let (mut p, _dir) = new_world("ghast-creative");
    run(&mut p, "summon minecraft:ghast 0.5 -55 8.5 {NoAI:1b,Invulnerable:1b}");
    run(&mut p, "summon minecraft:zombie 4.5 -60 4.5 {NoAI:1b,Invulnerable:1b}");
    p.s.game_mode = 1;
    let hit = |p: &mut Probe, kind: Kind| {
        let id = only(p, kind).id;
        let at = p.s.mobs.index_of(id).expect("listed");
        let mut m = p.s.mobs.list.take(at).expect("mob");
        let from = p.s.pos;
        let landed = p.s.mob_hurt(&mut m, "minecraft:player_attack", 1.0, Some(Ent::Player), Some(from));
        p.s.mobs.list.put(at, m);
        landed
    };
    assert!(!hit(&mut p, Kind::Ghast), "the invulnerable ghast");
    assert!(hit(&mut p, Kind::Zombie), "any other invulnerable mob: the creative hit lands");
}
