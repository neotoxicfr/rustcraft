//! Rules every living entity shares in the game, read once and applied to each mob: the reaction
//! to an equipment write (sound and game event), the team handed over by a conversion, the "seen
//! as an enemy" test of a target, and the ghast's own invulnerability.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::{SoundPacket, SoundRef};

use crate::blocks::probe::Probe;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::player::Stack;

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

/// Id of the item in a mob's main hand.
fn main_hand(m: &Mob) -> Option<&str> {
    m.equipment[0].as_ref().map(|s| s.id.as_str())
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
    game_events_named_any(p, &[name])
}

/// Game events named any of `names` given off since the log was last cleared, counted in a
/// single read (counting each name with `game_events` would clear the log after the first),
/// then the log cleared.
fn game_events_named_any(p: &mut Probe, names: &[&str]) -> usize {
    let n = p.s.mobs.rec.vib.log.iter().filter(|e| names.iter().any(|name| e.0 == *name)).count();
    p.s.mobs.rec.vib.log.clear();
    n
}

/// The two game events of an equipment write.
const EQUIP_EVENTS: [&str; 2] = ["minecraft:equip", "minecraft:unequip"];

/// A command writing a mob's equipment goes through the living entity's reaction, like the game:
/// a wearable stack put in its own slot plays its equip sound (in the mob's sound source) and gives
/// off `equip`; a stack that is not wearable gives off `unequip` in silence; the same stack again
/// does nothing; a silent mob gives off the event without the sound; a mob not ticked yet equips in
/// silence. An armor stand past its first tick reacts the same way. Fails on the code before (no
/// sound, no event for a mob or a stand written by a command); sabotage verified (failure): the
/// first-tick test removed, the silence test removed.
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
    assert_eq!(game_events_named_any(&mut p, &EQUIP_EVENTS), 0);

    let stick = run(&mut p, "item replace entity @e[type=minecraft:zombie] weapon.mainhand with minecraft:stick");
    assert!(!stick.iter().any(|e| matches!(e, Event::Sound(_))), "a stick is not wearable: no sound");
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1);

    mob_mut(&mut p, zombie).x.silent = true;
    let quiet = run(&mut p, "item replace entity @e[type=minecraft:zombie] armor.chest with minecraft:iron_chestplate");
    assert!(sounds(&p, &quiet, iron).is_empty(), "a silent mob plays no sound");
    assert_eq!(game_events(&mut p, "minecraft:equip"), 1, "but gives off the event");

    run(&mut p, "summon minecraft:armor_stand 2.5 -60 4.5");
    // Past its first tick, as in the game: there a stand equipped in its first tick stays silent.
    p.s.deco_tick();
    let stand = run(&mut p, "item replace entity @e[type=minecraft:armor_stand] armor.head with minecraft:iron_helmet");
    assert_eq!(sounds(&p, &stand, iron).len(), 1, "the armor stand's equip sound");
    assert_eq!(game_events(&mut p, "minecraft:equip"), 1, "the armor stand's event");
}

/// The game's first-tick rule for the living decorations: an armor stand or a mannequin equipped
/// in the tick it was summoned plays no sound and gives off no event; once ticked, it reacts.
/// Fails on the code before (the stand reacted from its first tick).
#[test]
fn a_living_decoration_equipped_in_its_first_tick_stays_silent() {
    let (mut p, _dir) = new_world("stand-first-tick");
    let iron = "minecraft:item.armor.equip_iron";
    for kind in ["armor_stand", "mannequin"] {
        run(&mut p, &format!("summon minecraft:{kind} 2.5 -60 4.5"));
        p.s.mobs.rec.vib.log.clear();
        let helmet = format!("item replace entity @e[type=minecraft:{kind}] armor.head with minecraft:iron_helmet");
        let first_tick = run(&mut p, &helmet);
        assert!(sounds(&p, &first_tick, iron).is_empty(), "{kind}, first tick: silent");
        assert_eq!(game_events_named_any(&mut p, &EQUIP_EVENTS), 0, "{kind}, first tick: no event");
        p.s.deco_tick();
        let chestplate =
            format!("item replace entity @e[type=minecraft:{kind}] armor.chest with minecraft:iron_chestplate");
        let ticked = run(&mut p, &chestplate);
        assert_eq!(sounds(&p, &ticked, iron).len(), 1, "{kind}, past its first tick: the equip sound");
        assert_eq!(game_events(&mut p, "minecraft:equip"), 1, "{kind}, past its first tick: the event");
    }
}

/// A villager taking an item in hand or putting it away goes through the living entity's
/// equipment reaction, like the game: the farmer's bone meal, the trades shown to a player and the
/// wandering trader's potion each give off `unequip` (not wearable) or `equip` (a chestplate shown)
/// when taken and `unequip` when put away, a vibration a sculk sensor hears. Fails on the code
/// before (no event at all).
#[test]
fn a_villager_taking_an_item_in_hand_gives_off_a_vibration() {
    use crate::mobs::villager::behave::Beh;
    use crate::mobs::villager::trader::TGoal;
    let (mut p, _dir) = new_world("villager-hand");
    run(&mut p, "summon minecraft:villager 0.5 -60 4.5 {NoAI:1b}");
    run(&mut p, "summon minecraft:wandering_trader 4.5 -60 4.5 {NoAI:1b}");
    let take = |p: &mut Probe, kind: Kind| {
        let at = p.s.mobs.index_of(only(p, kind).id).expect("listed");
        let mut m = p.s.mobs.list.take(at).expect("mob");
        m.tick_count = 1; // past the first tick, where the game equips in silence
        (at, m)
    };

    let (at, mut farmer) = take(&mut p, Kind::Villager);
    let mut brain = p.s.make_brain(&mut farmer, Default::default());
    let mut bone_meal = Beh::Bonemeal { next_cycle: 0, last_session: 0, worked: 0, crop: None };
    p.s.mobs.rec.vib.log.clear();
    p.s.beh_start(&mut farmer, &mut brain, &mut bone_meal, 0);
    assert_eq!(main_hand(&farmer), Some("minecraft:bone_meal"));
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "bone meal taken out");
    p.s.beh_stop(&mut farmer, &mut brain, &mut bone_meal, 0);
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "bone meal put away");

    let shown = |id: &str| Some(Stack { id: id.into(), count: 1, components: None });
    p.s.display_held(&mut farmer, shown("minecraft:emerald"));
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "an emerald shown");
    p.s.display_held(&mut farmer, shown("minecraft:iron_chestplate"));
    assert_eq!(game_events(&mut p, "minecraft:equip"), 1, "a wearable stack shown");
    p.s.display_held(&mut farmer, None);
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "the trades put away");
    p.s.mobs.list.put(at, farmer);

    let (at, mut trader) = take(&mut p, Kind::WanderingTrader);
    let mut drink = TGoal::UseItem { potion: true };
    p.s.tgoal_start(&mut drink, &mut trader);
    assert_eq!(main_hand(&trader), Some("minecraft:potion"));
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "potion taken out");
    p.s.tgoal_stop(&mut drink, &mut trader);
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "potion put away");
    p.s.mobs.list.put(at, trader);
}

/// A fox eating up a bowl of mushroom stew is left with the bowl through the living entity's
/// equipment reaction, like the game (an `unequip` vibration); a stack with pieces left is written
/// back as it is, without a reaction. Fails on the code before (no event for the bowl).
#[test]
fn a_fox_left_with_the_bowl_of_its_stew_gives_off_a_vibration() {
    let (mut p, _dir) = new_world("fox-bowl");
    run(&mut p, "summon minecraft:fox 0.5 -60 4.5 {NoAI:1b}");
    let at = p.s.mobs.index_of(only(&p, Kind::Fox).id).expect("listed");
    let mut fox = p.s.mobs.list.take(at).expect("fox");
    fox.tick_count = 1;
    let food = |id: &str, count: i32| Some(Stack { id: id.into(), count, components: None });

    fox.equipment[0] = food("minecraft:sweet_berries", 2);
    p.s.mobs.rec.vib.log.clear();
    crate::mobs::wild::fox::eat_mouth_item(&mut p.s, &mut fox);
    assert_eq!(fox.equipment[0].as_ref().map(|s| s.count), Some(1), "one berry left");
    assert_eq!(game_events_named_any(&mut p, &EQUIP_EVENTS), 0, "same stack");

    fox.equipment[0] = food("minecraft:mushroom_stew", 1);
    crate::mobs::wild::fox::eat_mouth_item(&mut p.s, &mut fox);
    assert_eq!(main_hand(&fox), Some("minecraft:bowl"), "the stew's bowl");
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "the bowl taken in the mouth");
    p.s.mobs.list.put(at, fox);
}

/// Equipment a mob loses or is given in play goes through the living entity's equipment
/// reaction, like the game: a helmet dropped at death, a guaranteed drop let go (a copper golem
/// turning into a statue), the poppy an iron golem hands to a copper golem; each gives off
/// `unequip`. Fails on the code before (no event).
#[test]
fn equipment_dropped_or_handed_over_in_play_gives_off_a_vibration() {
    let (mut p, _dir) = new_world("drops-in-play");
    let helmet = "{equipment:{head:{id:\"minecraft:iron_helmet\",count:1}},drop_chances:{head:2.0f}}";
    run(&mut p, &format!("summon minecraft:zombie 0.5 -60 4.5 {helmet}"));
    let zombie = only(&p, Kind::Zombie).id;
    mob_mut(&mut p, zombie).tick_count = 1;
    p.s.mobs.rec.vib.log.clear();
    run(&mut p, "kill @e[type=minecraft:zombie]");
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "the helmet dropped at death");

    run(&mut p, "summon minecraft:copper_golem 4.5 -60 4.5 {NoAI:1b}");
    let at = p.s.mobs.index_of(only(&p, Kind::CopperGolem).id).expect("listed");
    let mut golem = p.s.mobs.list.take(at).expect("golem");
    golem.tick_count = 1;
    crate::mobs::golems::give_antenna_poppy(&mut p.s, &mut golem);
    assert_eq!(golem.equipment[7].as_ref().map(|s| s.id.as_str()), Some("minecraft:poppy"));
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "the poppy handed over");
    p.s.drop_preserved_equipment(&mut golem);
    assert!(golem.equipment[7].is_none());
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "the guaranteed poppy let go");
    p.s.mobs.list.put(at, golem);
}

/// Every write of the absorption reserve stays within the maximum absorption, like the game's:
/// the absorption effect started under a lowered maximum (base -2, plus the effect's own 4) fills
/// only up to 2. Fails on the code before (4, above the maximum).
#[test]
fn the_absorption_effect_fills_up_to_the_maximum_absorption() {
    let (mut p, _dir) = new_world("absorption-cap");
    run(&mut p, "attribute @p minecraft:max_absorption base set -2");
    run(&mut p, "effect give @p minecraft:absorption 60 0");
    assert_eq!(p.s.life.fx.absorption, 2.0);
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

/// `/summon` places every passenger of a minecart's compound at the summon's position, like the
/// game, and seats it (a zombie on the minecart, a chicken on the zombie). Fails on the code
/// before (the passengers read at the origin of the world, their compounds having no position).
#[test]
fn summon_places_the_passengers_of_a_minecart_at_its_position() {
    let (mut p, _dir) = new_world("summon-cart-passengers");
    let chicken = "{id:\"minecraft:chicken\",NoAI:1b}";
    let zombie = format!("{{id:\"minecraft:zombie\",NoAI:1b,Passengers:[{chicken}]}}");
    run(&mut p, &format!("summon minecraft:minecart 4.5 -60 4.5 {{Passengers:[{zombie}]}}"));
    let cart = p.s.carts.list.iter().flatten().next().expect("the minecart").id;
    let (zombie, chicken) = (only(&p, Kind::Zombie), only(&p, Kind::Chicken));
    assert_eq!((zombie.pos(), chicken.pos()), ([4.5, -60.0, 4.5], [4.5, -60.0, 4.5]), "at the summon's position");
    assert_eq!((zombie.mount.vehicle, chicken.mount.vehicle), (Some(cart), Some(zombie.id)), "seated");
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
    let cubes: Vec<u128> =
        p.s.mobs.list.iter().filter(|m| m.kind == Kind::Slime && m.uuid != slime).map(|m| m.uuid).collect();
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

/// A cured zombie villager lets go of its preserved equipment through the common routine, like
/// the game: a helmet with a guaranteed drop leaves its slot with an `unequip` vibration and
/// falls; boots cursed with binding go into the villager's inventory instead. Fails on the code
/// before (no vibration: the slots were emptied in place).
#[test]
fn a_cured_zombie_villager_lets_go_of_its_preserved_equipment() {
    let (mut p, _dir) = new_world("cure-drops");
    let cursed = "{\"minecraft:enchantments\":{\"minecraft:binding_curse\":1}}";
    let helmet = "head:{id:\"minecraft:iron_helmet\",count:1}";
    let boots = format!("feet:{{id:\"minecraft:iron_boots\",count:1,components:{cursed}}}");
    let gear = format!("{{NoAI:1b,equipment:{{{helmet},{boots}}},drop_chances:{{head:2.0f}}}}");
    run(&mut p, &format!("summon minecraft:zombie_villager 0.5 -60 4.5 {gear}"));
    let at = p.s.mobs.index_of(only(&p, Kind::ZombieVillager).id).expect("listed");
    let mut zombie = p.s.mobs.list.take(at).expect("zombie villager");
    zombie.tick_count = 1; // past its first tick, where the game equips in silence
    zombie.x.cure_time = 1;
    let items_before = p.s.life.drops.len();
    p.s.mobs.rec.vib.log.clear();
    assert!(p.s.variant_pre_tick(&mut zombie), "cured");
    assert_eq!(game_events(&mut p, "minecraft:unequip"), 1, "the helmet let go");
    let dropped: Vec<&str> = p.s.life.drops[items_before..].iter().map(|d| d.stack.id.as_str()).collect();
    assert_eq!(dropped, ["minecraft:iron_helmet"], "the helmet falls, the cursed boots do not");
    p.s.mobs.list.put(at, zombie);
    let inventory = &only(&p, Kind::Villager).npc.as_ref().expect("villager data").inv;
    assert_eq!(inventory[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:iron_boots"), "the cursed boots kept");
}

/// A mob bucket hands the mob what any stack placing a mob hands, like the game: its custom data
/// (kept as the mob's `data`) and its entity data over the finalised mob, before the bucket's own
/// data; the sulfur cube's absorbed block with them; entity data naming another type is ignored.
/// Fails on the code before (the custom data and the entity data were left out) and on the first
/// version of the fix (the entity data read whatever type it named).
#[test]
fn a_mob_bucket_hands_its_custom_data_and_entity_data() {
    let (mut p, _dir) = new_world("bucket-data");
    let quest = Nbt::Compound(vec![("quest".into(), Nbt::Int(3))]);
    let bucket = |item: &str, mob: &str, extra: Vec<(String, Nbt)>| {
        let entity_data = Nbt::Compound(vec![("id".into(), Nbt::String(mob.into())), ("Silent".into(), Nbt::Byte(1))]);
        let mut components =
            vec![("minecraft:custom_data".into(), quest.clone()), ("minecraft:entity_data".into(), entity_data)];
        components.extend(extra);
        Stack { id: item.into(), count: 1, components: Some(Nbt::Compound(components)) }
    };
    let data_of = |m: &Mob| m.nbt.as_ref().and_then(|kept| kept.get("data")).cloned();

    let salmon_bucket = bucket("minecraft:salmon_bucket", "minecraft:salmon", Vec::new());
    p.s.spawn_from_bucket(Kind::Salmon, &salmon_bucket, [0, -60, 4]);
    let salmon = only(&p, Kind::Salmon);
    assert_eq!(data_of(salmon), Some(quest.clone()), "the salmon's custom data");
    assert!(salmon.x.silent, "the salmon's entity data");

    let content = Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:tnt".into()))]);
    let absorbed = vec![("minecraft:sulfur_cube_content".into(), content)];
    let cube_bucket = bucket("minecraft:sulfur_cube_bucket", "minecraft:sulfur_cube", absorbed);
    p.s.slots[36] = Some(cube_bucket);
    assert!(p.s.sulfur_bucket_use(36, [4, -61, 4], 1), "the cube released on the floor");
    let cube = only(&p, Kind::SulfurCube);
    assert_eq!(data_of(cube), Some(quest.clone()), "the cube's custom data");
    assert!(cube.x.silent, "the cube's entity data");
    assert_eq!(cube.equipment[6].as_ref().map(|s| s.id.as_str()), Some("minecraft:tnt"), "its absorbed block");

    // Entity data naming another type is ignored, like the game.
    let mislabelled = bucket("minecraft:cod_bucket", "minecraft:salmon", Vec::new());
    p.s.spawn_from_bucket(Kind::Cod, &mislabelled, [-4, -60, 4]);
    assert!(!only(&p, Kind::Cod).x.silent, "the salmon's entity data not read by the cod");
}

/// `entity_place` and `fluid_place` game events given off since the log was last cleared, read
/// once, then the log cleared.
fn placement_events(p: &mut Probe) -> (usize, usize) {
    let count = |p: &Probe, name: &str| p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == name).count();
    let counts = (count(p, "minecraft:entity_place"), count(p, "minecraft:fluid_place"));
    p.s.mobs.rec.vib.log.clear();
    counts
}

/// An emptied mob bucket gives off `entity_place` for the mob it places and nothing for its water,
/// like the game (a mob bucket plays its own sound instead of the water's, the one giving off
/// `fluid_place`); emptied by a player it counts as used. Fails on the code before (a player's
/// bucket gave off nothing and counted nothing, a dispenser's gave off `fluid_place` too).
#[test]
fn an_emptied_mob_bucket_gives_off_the_placed_mob_only() {
    let (mut p, _dir) = new_world("bucket-events");
    let one = |id: &str| Stack { id: id.into(), count: 1, components: None };
    p.s.mobs.rec.vib.log.clear();
    p.s.slots[36] = Some(one("minecraft:salmon_bucket"));
    assert!(p.s.fish_bucket_use(36, [0, -61, 4], 1), "the salmon released");
    assert_eq!(placement_events(&mut p), (1, 0), "a player's fish bucket");
    assert_eq!(p.s.stat_value("minecraft:used", "minecraft:salmon_bucket"), 1);

    p.s.slots[36] = Some(one("minecraft:sulfur_cube_bucket"));
    assert!(p.s.sulfur_bucket_use(36, [4, -61, 4], 1), "the cube released");
    assert_eq!(placement_events(&mut p), (1, 0), "a player's sulfur cube bucket");
    assert_eq!(p.s.stat_value("minecraft:used", "minecraft:sulfur_cube_bucket"), 1);

    run(&mut p, "setblock 8 -60 8 minecraft:dispenser[facing=north]");
    p.s.mobs.rec.vib.log.clear();
    p.s.dispense_item([8, -60, 8], crate::blocks::Dir::North, 0, one("minecraft:cod_bucket"));
    assert_eq!(only(&p, Kind::Cod).block_pos(), [8, -60, 7], "the cod released in front");
    assert_eq!(placement_events(&mut p), (1, 0), "a dispenser's fish bucket");
}

/// After the end credits the new player entity takes its kept health and its effects before its
/// inventory, like the game: the health is capped by the maximum of the bases and the saved
/// modifiers alone (a health boost does not hold it up), and the absorption effect fills its
/// reserve under a maximum the worn equipment does not lower until the new entity's first tick
/// spots that equipment again. Fails on the code before (health 28 kept; the reserve held at 0 by
/// the helmet).
#[test]
fn after_the_end_credits_health_and_effects_come_before_the_equipment() {
    let (mut p, _dir) = new_world("credits-order");
    run(&mut p, "effect give @p minecraft:health_boost 60 1");
    p.s.life.vit.health = 28.0;
    let modifier = "{type:\"minecraft:max_absorption\",amount:-4.0d,id:\"rustcraft:test_helmet\",operation:\"add_value\",slot:\"head\"}";
    let helmet = format!("minecraft:iron_helmet[attribute_modifiers=[{modifier}]]");
    run(&mut p, &format!("item replace entity @p armor.head with {helmet}"));
    p.s.equipment_tick();
    run(&mut p, "effect give @p minecraft:absorption 60 0");
    assert_eq!(p.s.life.fx.absorption, 0.0, "the helmet holds the maximum absorption at 0");
    p.s.dims.won_game = true;
    p.s.respawn_after_credits();
    assert_eq!(p.s.life.vit.health, 20.0, "the kept health under the bare maximum");
    assert_eq!(p.s.life.fx.absorption, 4.0, "the effect's reserve, the helmet not counted yet");
    p.s.equipment_tick();
    assert_eq!(p.s.life.fx.absorption, 0.0, "the helmet spotted again caps the reserve");
}

/// Ids of the entities removed from the client among `events`.
fn removed_ids(events: &[Event]) -> Vec<i32> {
    let removals = events.iter().filter_map(|e| match e {
        Event::Entity(EntityEvent::Remove(ids)) => Some(ids.iter().copied()),
        _ => None,
    });
    removals.flatten().collect()
}

/// Ids of the entities added to the client among `events`.
fn added_ids(events: &[Event]) -> Vec<i32> {
    let adds = events.iter().filter_map(|e| match e {
        Event::Entity(EntityEvent::Add(add)) => Some(add.id),
        _ => None,
    });
    adds.collect()
}

/// Complete server ticks like the server loop (chunks read, game tick, client view, chunks
/// sent), the events sent meanwhile.
fn game_ticks(p: &mut Probe, ticks: usize) -> Vec<Event> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        p.s.poll_loader();
        p.s.game_tick();
        p.s.tick += 1;
        p.s.update_tracking();
        p.s.poll_gen();
        p.s.send_chunks();
        events.extend(p.take_events());
    }
    events
}

/// A player respawning before its removal is removed first, like the game's respawn: its client is
/// told the entities it sees are gone before the respawn reaches it, but no chunk is forgotten
/// (the game tells only a living player to forget a chunk), and the respawn sends the chunks again.
/// Fails on the code before (every chunk held forgotten before the respawn).
#[test]
fn a_player_respawning_before_its_removal_keeps_its_chunks_on_screen() {
    let (mut p, _dir) = new_world("early-respawn");
    run(&mut p, "summon minecraft:item 3.5 -60 3.5 {Item:{id:\"minecraft:stone\",count:1}}");
    game_ticks(&mut p, 3);
    let item = p.s.life.drops[0].id;
    run(&mut p, "kill @p");
    game_ticks(&mut p, 5);
    assert!(!p.s.sent.is_empty(), "the client holds chunks");
    p.s.perform_respawn();
    let events = p.take_events();
    let respawn = events.iter().position(|e| matches!(e, Event::Respawn { .. })).expect("the respawn");
    let forgotten = events.iter().filter(|e| matches!(e, Event::ForgetChunk { .. })).count();
    assert_eq!(forgotten, 0, "no chunk forgotten");
    assert!(removed_ids(&events[..respawn]).contains(&item), "the item forgotten before the respawn");
    (p.s.loaded, p.s.teleport) = (true, None); // the client's answers
    let respawned = game_ticks(&mut p, 3);
    assert!(respawned.iter().any(|e| matches!(e, Event::Chunk(_))), "the chunks sent again");
}

/// A dead player whose view moves (teleported during its death) is never told to forget the
/// chunks left behind, like the game, which tells only a living player; they leave its view all
/// the same. Fails on the code before (one forget per chunk left).
#[test]
fn a_dead_player_is_never_told_to_forget_a_chunk() {
    let (mut p, _dir) = new_world("dead-view-moves");
    game_ticks(&mut p, 3);
    run(&mut p, "kill @p");
    game_ticks(&mut p, 2);
    let held: Vec<(i32, i32)> = p.s.sent.iter().copied().collect();
    assert!(!held.is_empty(), "the client holds chunks");
    p.s.pos = [16.0 * 64.0 + 0.5, -60.0, 0.5];
    p.s.update_tracking();
    let forgotten = p.take_events().iter().filter(|e| matches!(e, Event::ForgetChunk { .. })).count();
    assert_eq!(forgotten, 0, "no chunk forgotten");
    assert!(held.iter().all(|c| !p.s.sent.contains(c)), "the chunks left its view");
}

/// A dead player leaves the world 20 ticks after its death, like any living entity of the game:
/// its client is told every entity it sees is gone (the decorations of the chunks it holds, the
/// items and orbs, then the mobs at their tracking pass) but forgets no chunk, the world staying
/// behind the death screen; nothing is simulated around it any more (it holds no chunk ticket),
/// and the respawn sends the world again. Fails on the code before this series (the entities
/// kept, the zombie still ticking) and on its first version (every chunk forgotten).
#[test]
fn a_dead_player_leaves_the_world_until_it_respawns() {
    let (mut p, _dir) = new_world("dead-player");
    run(&mut p, "summon minecraft:zombie 0.5 -60 6.5 {NoAI:1b,PersistenceRequired:1b}");
    run(&mut p, "summon minecraft:armor_stand 2.5 -60 4.5");
    // Out of the player's reach: the orb follows a player within 8 blocks.
    run(&mut p, "summon minecraft:experience_orb 12.5 -60 0.5 {Value:3}");
    run(&mut p, "summon minecraft:item 3.5 -60 3.5 {Item:{id:\"minecraft:stone\",count:1}}");
    game_ticks(&mut p, 3);
    let zombie = only(&p, Kind::Zombie).id;
    assert!(only(&p, Kind::Zombie).track.seen, "the client sees the zombie");
    let is_stand = |e: &&(i32, Nbt)| e.1.get("id").and_then(Nbt::as_str) == Some("minecraft:armor_stand");
    let stand = p.s.entities.values().flatten().find(is_stand).expect("the armor stand kept").0;
    let (orb, item) = (p.s.life.orbs[0].id, p.s.life.drops[0].id);

    run(&mut p, "kill @p");
    let dying = game_ticks(&mut p, 19);
    assert!(!dying.iter().any(|e| matches!(e, Event::ForgetChunk { .. })), "still in the world while dying");
    let held = p.s.sent.len();
    assert!(held > 0, "the client holds chunks");
    let removal = game_ticks(&mut p, 2);
    let forgotten = removal.iter().filter(|e| matches!(e, Event::ForgetChunk { .. })).count();
    assert_eq!(forgotten, 0, "no chunk forgotten: the world stays behind the death screen");
    assert!(p.s.sent.is_empty(), "nothing tracked any more");
    let removed = removed_ids(&removal);
    for (what, id) in [("zombie", zombie), ("armor stand", stand), ("orb", orb), ("item", item)] {
        assert!(removed.contains(&id), "the {what} forgotten: {removed:?}");
    }
    let ticks_before = only(&p, Kind::Zombie).tick_count;
    let removed_while_dead = game_ticks(&mut p, 5);
    assert_eq!(only(&p, Kind::Zombie).tick_count, ticks_before, "nothing simulated around a removed player");
    assert!(!removed_while_dead.iter().any(|e| matches!(e, Event::Chunk(_))), "no chunk sent to a removed player");

    p.s.perform_respawn();
    (p.s.loaded, p.s.teleport) = (true, None); // the client's answers
    let respawned = game_ticks(&mut p, 3);
    assert!(respawned.iter().any(|e| matches!(e, Event::Chunk(_))), "the chunks sent again");
    let added = added_ids(&respawned);
    for (what, id) in [("zombie", zombie), ("orb", orb), ("item", item)] {
        assert!(added.contains(&id), "the {what} announced again: {added:?}");
    }
    assert!(only(&p, Kind::Zombie).tick_count > ticks_before, "simulated again");
}
