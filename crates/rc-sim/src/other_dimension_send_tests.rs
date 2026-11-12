//! Commands run in another dimension (`execute in`): what they do to the players and to the
//! whole server reaches the players exactly as without `execute in`; only what happens in that
//! world (particles, sounds heard there) stays there, no player standing in it.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::Event;
use rc_proto_v777::play::BossOp;

use crate::blocks::probe::Probe;
use crate::dims::NETHER;
use crate::newworld::{NewWorld, Preset, create_world};

fn version_data() -> Arc<rc_data::VersionData> {
    let data = Arc::new(rc_testdata::version_data().expect("other dimension send tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "other dimension send tests: 26.3 client jar missing");
    data
}

fn new_world(tag: &str) -> (Probe, PathBuf) {
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-other-dim-{tag}-{}", std::process::id())));
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
    let sent = run(&mut p, &format!("execute in minecraft:the_nether run {}", near_player("particle minecraft:flame")));
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
