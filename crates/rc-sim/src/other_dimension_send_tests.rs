//! Commands run in another dimension (`execute in`): what they do to the players and to the
//! whole server reaches the players exactly as without `execute in`; only what happens in that
//! world (particles, sounds heard there, its own border) stays there, no player standing in it.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::{EntityEvent, Event};
use rc_proto_v777::border::BorderPacket;
use rc_proto_v777::play::{BossOp, SoundRef};

use crate::blocks::probe::Probe;
use crate::dims::{NETHER, OVERWORLD};
use crate::newworld::{NewWorld, Preset, create_world};

fn version_data() -> Arc<rc_data::VersionData> {
    let data = Arc::new(rc_testdata::version_data().expect("other dimension send tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "other dimension send tests: 26.3 client jar missing");
    data
}

pub(crate) fn new_world(tag: &str) -> (Probe, PathBuf) {
    let dir = create_flat_world(tag);
    (open_world(&dir), dir)
}

fn create_flat_world(tag: &str) -> PathBuf {
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-other-dim-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    w.allow_commands = true;
    create_world(&dir, &version_data(), &w).expect("world creation");
    dir
}

/// Opens `dir` with a survival player standing on the flat ground, chunks around loaded.
pub(crate) fn open_world(dir: &std::path::Path) -> Probe {
    let mut p = Probe::open(dir, version_data());
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.teleport = None;
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    let _ = p.take_events();
    p
}

/// Events a server command sends.
pub(crate) fn run(p: &mut Probe, text: &str) -> Vec<Event> {
    let src = p.s.server_source();
    assert!(p.s.perform(src, text).0, "command refused: {text}");
    p.take_events()
}

/// Short name of an event: `Difficulty`, `Title`, `Entity Add`…
fn kind(e: &Event) -> String {
    let text = format!("{e:?}");
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).take(2).collect();
    match words[..] {
        ["Entity" | "Misc", inner] => format!("{} {inner}", words[0]),
        _ => words[0].to_owned(),
    }
}

fn kinds(evs: &[Event]) -> Vec<String> {
    evs.iter().map(kind).collect()
}

/// `difficulty`, `title`, `give` and `effect` run from the Nether send the player the same
/// packets as run from the player's world (before: nothing at all, every send of the Nether cut).
#[test]
fn commands_run_in_another_dimension_reach_the_players() {
    let (mut p, _dir) = new_world("commands");
    let pairs = [
        ("difficulty hard", "execute in minecraft:the_nether run difficulty easy"),
        ("title @a title \"Here\"", "execute in minecraft:the_nether run title @a title \"There\""),
        ("give @a minecraft:diamond", "execute in minecraft:the_nether run give @a minecraft:diamond"),
        ("effect give @a minecraft:speed 10", "execute in minecraft:the_nether run effect give @a minecraft:haste 10"),
    ];
    for (here, there) in pairs {
        let expected = kinds(&run(&mut p, here));
        assert!(!expected.is_empty(), "{here}: nothing sent");
        assert_eq!(kinds(&run(&mut p, there)), expected, "{there}");
    }
}

/// What a command makes happen in the Nether stays there: no particle, no sound reaches a
/// player standing in the overworld.
#[test]
fn world_events_of_another_dimension_stay_there() {
    let (mut p, _dir) = new_world("world-events");
    let near_player = |what: &str| format!("{what} 0 -60 0");
    assert!(kinds(&run(&mut p, &near_player("particle minecraft:flame"))).contains(&"Particles".to_owned()));
    // Nobody in the Nether sees it: the command fails, like the game's (L13: before, the player
    // standing in the overworld counted as a viewer).
    let src = p.s.server_source();
    let nether_particle = format!("execute in minecraft:the_nether run {}", near_player("particle minecraft:flame"));
    assert!(!p.s.perform(src, &nether_particle).0, "Nether particle seen");
    let sent = p.take_events();
    assert!(sent.iter().all(|e| !matches!(e, Event::Particles(_))), "Nether particle sent: {:?}", kinds(&sent));
    let sound = "playsound minecraft:entity.item.pickup master @a 0 -60 0";
    assert!(kinds(&run(&mut p, sound)).contains(&"Sound".to_owned()));
    // Nobody in the Nether hears it: the command fails, like the game's.
    let src = p.s.server_source();
    assert!(!p.s.perform(src, &format!("execute in minecraft:the_nether run {sound}")).0, "Nether sound heard");
    let sent = p.take_events();
    assert!(sent.iter().all(|e| !matches!(e, Event::Sound(_))), "Nether sound sent: {:?}", kinds(&sent));
}

/// `execute in minecraft:overworld run …` run while the server works in the Nether (a command
/// block or a function ticking there while the players stand in the overworld) sends like a
/// command run in the overworld: the `give` packets, the particles and the sound of the players'
/// world reach them (before: still cut by the visit to the Nether). A chain of `execute in` typed
/// by a player never nests: only its last dimension is visited.
#[test]
fn command_from_a_parked_world_back_to_the_players_world_sends_open() {
    let (mut p, _dir) = new_world("nested");
    let from_nether = |p: &mut Probe, text: &str| {
        let command = format!("execute in minecraft:overworld run {text}");
        let done = p.s.in_world(NETHER, |s| {
            let src = s.server_source();
            s.perform(src, &command).0
        });
        assert!(done, "command refused: {command}");
        p.take_events()
    };
    for text in ["give @a minecraft:diamond", "particle minecraft:flame 0 -60 0"] {
        let expected = kinds(&run(&mut p, text));
        assert_eq!(kinds(&from_nether(&mut p, text)), expected, "{text} from the Nether");
    }
    let sent = from_nether(&mut p, "playsound minecraft:entity.item.pickup master @a 0 -60 0");
    assert!(sent.iter().any(|e| matches!(e, Event::Sound(_))), "sound of the players' world not sent: {:?}", kinds(&sent));
}

/// A custom boss bar belongs to no world: shown, renamed and hidden from the Nether, it reaches
/// the player.
#[test]
fn a_custom_boss_bar_changed_from_another_dimension_reaches_the_players() {
    let (mut p, _dir) = new_world("boss-bar");
    let nether = |text: &str| format!("execute in minecraft:the_nether run {text}");
    let _ = run(&mut p, &nether("bossbar add t:bar \"Bar\""));
    let shown = run(&mut p, &nether("bossbar set t:bar players @a"));
    assert!(shown.iter().any(|e| matches!(e, Event::Boss(b) if matches!(b.op, BossOp::Add { .. }))), "bar not shown");
    let renamed = run(&mut p, &nether("bossbar set t:bar name \"New\""));
    assert!(renamed.iter().any(|e| matches!(e, Event::Boss(b) if matches!(b.op, BossOp::Name(_)))), "bar not renamed");
    let hidden = run(&mut p, &nether("bossbar set t:bar visible false"));
    assert!(hidden.iter().any(|e| matches!(e, Event::Boss(b) if matches!(b.op, BossOp::Remove))), "bar not hidden");
}

/// `give` run from the Nether drops what found no room, and its dummy items, at the player's
/// feet in the player's own world, like the game (before: into the Nether, at the player's
/// overworld coordinates).
#[test]
fn give_from_another_dimension_drops_in_the_players_world() {
    let (mut p, _dir) = new_world("give");
    let sent = run(&mut p, "execute in minecraft:the_nether run give @a minecraft:diamond 2500");
    let nether_drops = p.s.dims.parked.iter().find(|w| w.dim.name == NETHER).map_or(0, |w| w.drops.len());
    assert_eq!(nether_drops, 0, "items dropped into the Nether");
    let surplus: i32 = p.s.life.drops.iter().filter(|d| d.stack.id == "minecraft:diamond" && d.age != 5999).map(|d| d.stack.count).sum();
    assert_eq!(surplus, 2500 - 36 * 64, "surplus at the player's feet");
    assert!(sent.iter().any(|e| matches!(e, Event::Sound(_))), "pickup sound not sent");
}

/// Items `id` lying on the ground: (in the players' world, in the Nether).
fn on_the_ground(p: &Probe, id: &str) -> (i32, i32) {
    let here = p.s.life.drops.iter().filter(|d| d.stack.id == id).map(|d| d.stack.count).sum();
    let nether = p.s.dims.parked.iter().filter(|w| w.dim.name == NETHER).flat_map(|w| &w.drops);
    (here, nether.filter(|d| d.stack.id == id).map(|d| d.stack.count).sum())
}

/// The packet that makes the player's own client flash and tilt the camera on a hit.
fn is_hurt_packet_of(e: &Event, player: i32) -> bool {
    matches!(e, Event::Entity(EntityEvent::Hurt { id, .. }) if *id == player)
}

/// `kill` run from the Nether kills the player in their own world, like the game: the inventory
/// drops there and the place of death is the overworld, while the hurt packet, which the game sends
/// through the Nether's entity tracking, reaches nobody. Before: the items dropped into the Nether
/// at the player's overworld coordinates, the place of death was the Nether, the packet was sent.
#[test]
fn killing_the_player_from_another_dimension_happens_in_the_players_world() {
    let (mut p, _dir) = new_world("kill");
    let _ = run(&mut p, "item replace entity @a hotbar.0 with minecraft:diamond 5");
    let player = p.s.player_id;
    let sent = run(&mut p, "execute in minecraft:the_nether run kill @a");
    assert!(p.s.life.vit.dead, "player not killed");
    assert_eq!(on_the_ground(&p, "minecraft:diamond"), (5, 0), "diamonds on the ground: (overworld, Nether)");
    let place_of_death = p.s.life.vit.last_death.as_ref().map(|(dimension, _)| dimension.as_str());
    assert_eq!(place_of_death, Some(OVERWORLD));
    assert!(sent.iter().any(|e| matches!(e, Event::Died { .. })), "death not sent: {:?}", kinds(&sent));
    assert!(!sent.iter().any(|e| is_hurt_packet_of(e, player)), "hurt packet sent: {:?}", kinds(&sent));
}

/// `damage` run from the Nether: the player hears their hurt sound, which the game plays in the
/// player's own world, and gets no hurt packet, which it sends through the Nether's entity
/// tracking. Before: the sound was cut with the Nether's sends and the packet was sent.
#[test]
fn damaging_the_player_from_another_dimension_sends_what_the_game_sends() {
    let (mut p, _dir) = new_world("damage");
    let player = p.s.player_id;
    let here = run(&mut p, "damage @p 2");
    assert!(here.iter().any(|e| is_hurt_packet_of(e, player)), "hurt packet: {:?}", kinds(&here));
    assert!(here.iter().any(|e| matches!(e, Event::Sound(_))), "hurt sound: {:?}", kinds(&here));
    p.s.life.vit.cooldown = 0; // the next hit lands whole
    let health = p.s.life.vit.health;
    let there = run(&mut p, "execute in minecraft:the_nether run damage @p 2");
    assert_eq!(p.s.life.vit.health, health - 2.0, "damage dealt");
    assert!(there.iter().any(|e| matches!(e, Event::Sound(_))), "hurt sound not sent: {:?}", kinds(&there));
    assert!(!there.iter().any(|e| is_hurt_packet_of(e, player)), "hurt packet sent: {:?}", kinds(&there));
}

/// `xp add` run from the Nether: the level-up sound plays in the player's own world, like the
/// game's. Before: it was cut with the Nether's sends.
#[test]
fn level_up_sound_of_experience_given_from_another_dimension_reaches_the_player() {
    let (mut p, _dir) = new_world("experience");
    p.s.mobs.player_ticks = 200; // the sound waits 100 ticks after the last one
    let sent = run(&mut p, "execute in minecraft:the_nether run xp add @a 5 levels");
    assert_eq!(p.s.life.vit.xp_level, 5);
    assert!(sent.iter().any(|e| matches!(e, Event::Sound(_))), "level-up sound not sent: {:?}", kinds(&sent));
}

/// Mobs living in the players' world and in the Nether.
fn mob_counts(p: &Probe) -> (usize, usize) {
    let nether = p.s.dims.parked.iter().filter(|w| w.dim.name == NETHER).map(|w| w.mobs.list.len()).sum();
    (p.s.mobs.list.len(), nether)
}

/// An infested player hurt by a command run from the Nether: the silverfish appear in the Nether,
/// the world the game hands to the hit (at the player's coordinates), none next to the player.
/// Hits are dealt until the infestation's one-in-ten draw comes out (the world's seeded draws: the
/// same hit every run).
#[test]
fn infestation_from_a_hit_dealt_in_another_dimension_spawns_its_silverfish_there() {
    let (mut p, _dir) = new_world("infested");
    let _ = run(&mut p, "effect give @p minecraft:infested 1000");
    let before = mob_counts(&p);
    for _ in 0..200 {
        p.s.life.vit.cooldown = 0;
        p.s.life.vit.health = 20.0;
        let _ = run(&mut p, "execute in minecraft:the_nether run damage @p 1");
        if mob_counts(&p) != before {
            break;
        }
    }
    let (here, nether) = mob_counts(&p);
    assert_eq!(here, before.0, "silverfish next to the player");
    assert!(nether > before.1, "no silverfish in the Nether after 200 hits");
}

/// `loot give` run from the Nether gives the same items with the same packets as run from the
/// player's world and drops nothing anywhere: the game only adds the loot to the inventory. Read
/// in the game's code and checked here; no difference was found on this command.
#[test]
fn loot_given_from_another_dimension_only_fills_the_inventory() {
    let (mut p, _dir) = new_world("loot");
    let give = "loot give @a loot minecraft:blocks/dirt";
    let here = kinds(&run(&mut p, give));
    let there = kinds(&run(&mut p, &format!("execute in minecraft:the_nether run {give}")));
    assert_eq!(there, here);
    let in_inventory: i32 = p.s.slots.iter().flatten().filter(|s| s.id == "minecraft:dirt").map(|s| s.count).sum();
    assert_eq!(in_inventory, 2, "one dirt per command");
    assert_eq!(on_the_ground(&p, "minecraft:dirt"), (0, 0), "dirt on the ground: (overworld, Nether)");
}

/// Border size saved in the folder of `dimension`.
fn saved_border_size(p: &Probe, dimension: &str) -> Option<f64> {
    let folder = crate::world::dimension_dir(&p.s.world, dimension);
    let file = folder.join("data").join("minecraft").join("world_border.dat");
    let root = crate::nbtio::read_dat(&file).ok()?;
    root.get("data")?.get("size").and_then(crate::mobs::mob::f64_of)
}

/// The value a command returns.
pub(crate) fn value_of(p: &mut Probe, text: &str) -> i32 {
    let src = p.s.server_source();
    let (done, value) = p.s.perform(src, text);
    assert!(done, "command refused: {text}");
    value
}

/// Each dimension keeps its own border, like the game's per-world saved data: a border set from
/// the Nether changes only the Nether's, sends nothing to the player standing in the overworld,
/// reaches the player when they enter the Nether and is saved in the Nether's folder. Before: one
/// border for every dimension, so the overworld's became the Nether's size without a packet.
#[test]
fn each_dimension_keeps_its_own_world_border() {
    let (mut p, _dir) = new_world("border");
    let _ = run(&mut p, "worldborder set 100");
    let sent = run(&mut p, "execute in minecraft:the_nether run worldborder set 50");
    assert!(!sent.iter().any(|e| matches!(e, Event::Border(_))), "Nether border sent: {:?}", kinds(&sent));
    assert_eq!(p.s.cmd.border.size, 100.0, "overworld border");
    assert_eq!(value_of(&mut p, "execute in minecraft:the_nether run worldborder get"), 50);
    let arrival = run(&mut p, "execute in minecraft:the_nether run tp @a 0.5 80 0.5");
    assert_eq!(p.s.dim.name, NETHER, "player not in the Nether");
    let border_at_arrival = arrival.iter().find_map(|e| match e {
        Event::Border(BorderPacket::Init { old_size, .. }) => Some(*old_size),
        _ => None,
    });
    assert_eq!(border_at_arrival, Some(50.0), "border sent on arrival");
    assert_eq!(value_of(&mut p, "execute in minecraft:overworld run worldborder get"), 100);
    p.s.save(true);
    assert_eq!(saved_border_size(&p, OVERWORLD), Some(100.0));
    assert_eq!(saved_border_size(&p, NETHER), Some(50.0));
}

/// A border moving in a dimension nobody stands in keeps moving, like every world's border in the
/// game, and the players' own border stays where it is. Before: the single border moved for all.
#[test]
fn a_border_keeps_moving_in_a_dimension_nobody_stands_in() {
    let (mut p, _dir) = new_world("moving-border");
    let overworld_size = p.s.cmd.border.size;
    let _ = run(&mut p, "execute in minecraft:the_nether run worldborder set 10 20t");
    for _ in 0..20 {
        p.s.game_tick();
        p.s.tick += 1;
    }
    assert_eq!(value_of(&mut p, "execute in minecraft:the_nether run worldborder get"), 10);
    assert_eq!(p.s.cmd.border.size, overworld_size, "overworld border moved");
}

/// Adds to the world settings of `dir` a data pack dimension `name` (the overworld's settings)
/// whose saved border shrinks from 1000 to 10 over `ticks` ticks.
fn add_dimension_with_moving_border(dir: &std::path::Path, name: &str, ticks: i64) {
    use crate::nbtio::{encode_root, get_mut, gzip};
    use rc_proto::nbt::Nbt;
    let settings_file = dir.join("data").join("minecraft").join("world_gen_settings.dat");
    let mut settings = crate::nbtio::read_dat(&settings_file).expect("world settings");
    let dimensions = get_mut(&mut settings, "data").and_then(|d| get_mut(d, "dimensions"));
    let Some(Nbt::Compound(dimensions)) = dimensions else { panic!("no dimensions in the world settings") };
    let overworld = dimensions.iter().find(|(k, _)| k == OVERWORLD).map(|(_, v)| v.clone());
    dimensions.push((name.to_owned(), overworld.expect("overworld settings")));
    std::fs::write(&settings_file, gzip(&encode_root(&settings))).expect("world settings written");
    let border = Nbt::Compound(vec![
        ("size".into(), Nbt::Double(1000.0)),
        ("lerp_target".into(), Nbt::Double(10.0)),
        ("lerp_time".into(), Nbt::Long(ticks)),
    ]);
    let border_dir = crate::world::dimension_dir(dir, name).join("data").join("minecraft");
    std::fs::create_dir_all(&border_dir).expect("border folder");
    let root = Nbt::Compound(vec![("data".into(), border)]);
    std::fs::write(border_dir.join("world_border.dat"), gzip(&encode_root(&root))).expect("border written");
}

/// The border of a data pack dimension nobody visits moves from the opening, like every loaded
/// world's border in the game. Before: only the game's three dimensions were read at opening; a
/// pack dimension's border was read at its first visit, its transition starting again from the
/// saved state.
#[test]
fn a_data_pack_dimension_border_moves_from_the_opening() {
    let dir = create_flat_world("pack-dimension-border");
    add_dimension_with_moving_border(&dir, "p:extra", 20);
    let mut p = open_world(&dir);
    for _ in 0..20 {
        p.s.game_tick();
        p.s.tick += 1;
    }
    let size = p.s.cmd.other_borders.iter().find(|b| b.dimension == "p:extra").map(|b| b.border.size);
    assert_eq!(size, Some(10.0), "border of the pack dimension after its transition");
}

/// A Nether portal clamps its approximate exit inside the border of the ARRIVAL world, like the
/// game. Overworld border 1000 wide, Nether border untouched: a player at x = 200 in the Nether
/// (x 1600 in the overworld) arrives inside the overworld border, on a portal built on the flat
/// ground near its edge. Before: the point was clamped by the Nether's border and stayed at 1600,
/// no site inside the overworld border was within reach and the fallback platform was built in
/// the air at the entry height.
#[test]
fn nether_portal_exit_is_clamped_by_the_arrival_worlds_border() {
    let (mut p, _dir) = new_world("portal-border");
    let _ = run(&mut p, "worldborder set 1000");
    let _ = run(&mut p, "execute in minecraft:the_nether run tp @a 200.5 80 0.5");
    assert_eq!(p.s.dim.name, NETHER, "player not in the Nether");
    let exit = p.s.nether_destination([200, 80, 0], [200.5, 80.0, 0.5], 0.6, 1.8, false).expect("portal exit");
    assert_eq!(exit.dim, OVERWORLD);
    assert!(exit.pos[0] > 480.0 && exit.pos[0] < 500.0, "exit x {} not near the overworld border", exit.pos[0]);
    assert!(exit.pos[1] < 0.0, "exit y {}: built in the air instead of on the flat ground", exit.pos[1]);
}

/// `item replace` into a player's equipment slot equips the stack like the game: the equip sound,
/// heard in the player's own world even when the command runs in another dimension; the same
/// stack again or a plain inventory slot plays none. Before: no sound at all.
#[test]
fn item_replaced_into_an_equipment_slot_plays_the_equip_sound() {
    let (mut p, _dir) = new_world("equip");
    let id = p.s.sound_id("minecraft:item.armor.equip_iron").expect("known sound");
    let is_equip_sound = |e: &Event| matches!(e, Event::Sound(s) if matches!(s.sound, SoundRef::Id(i) if i == id));
    let equip_sounds = |evs: &[Event]| evs.iter().filter(|e| is_equip_sound(e)).count();
    let chest = "item replace entity @a armor.chest with minecraft:iron_chestplate";
    assert_eq!(equip_sounds(&run(&mut p, chest)), 1, "from the player's world");
    assert_eq!(equip_sounds(&run(&mut p, chest)), 0, "same stack again");
    let legs = "execute in minecraft:the_nether run item replace entity @a armor.legs with minecraft:iron_leggings";
    assert_eq!(equip_sounds(&run(&mut p, legs)), 1, "from the Nether");
    let hotbar = "item replace entity @a hotbar.0 with minecraft:iron_boots";
    assert_eq!(equip_sounds(&run(&mut p, hotbar)), 0, "inventory slot");
}

/// Sky layers stored in the columns `around`.
fn sky_layers(s: &crate::server::Server, around: &[(i32, i32)]) -> usize {
    around.iter().filter_map(|k| s.chunks.get(k)?.as_deref()).map(|c| c.sky.iter().flatten().count()).sum()
}

/// A block changed in the Nether leaves no sky light behind: the game has no sky light engine in a
/// world without sky, so no sky layer is sent or saved there. Before: the relight around the change
/// made "implied" sky layers, full of 15, then sent them to the client and saved them.
#[test]
fn a_change_in_a_world_without_sky_makes_no_sky_light() {
    let (mut p, _dir) = new_world("skyless-light");
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    let (generated, relit) = p.s.in_world(NETHER, |s| {
        s.load_now(&around);
        let generated = sky_layers(s, &around);
        let glowstone = s.default_state("minecraft:glowstone");
        s.set_block([0, 64, 0], glowstone, 3);
        s.relight_changed_blocks();
        (generated, sky_layers(s, &around))
    });
    assert_eq!(generated, 0, "sky layers made by the generation");
    assert_eq!(relit, 0, "sky layers made by the relight");
}
