//! A function tag that holds no function, unknown or defined empty: the game looks tags up with an
//! empty list as default, so `function` answers that the tag holds no function, `execute if|unless
//! function` queues nothing (the chain stops there) and `debug function` traces nothing.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::sync::Arc;

use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};

const PACK_META: &str = r#"{"pack":{"description":"empty tag","min_format":121,"max_format":121}}"#;

/// Flat world with pack `lt`, whose function tag `lt:empty` lists nothing, opened from
/// `<scratch>/saves/world`: `debug function` writes its trace in the game folder, the world's
/// grandparent, here inside the scratch folder.
fn new_world(tag: &str) -> Probe {
    let data = Arc::new(rc_testdata::version_data().expect("empty function tag tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "empty function tag tests: 26.3 client jar missing");
    let game = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-empty-tag-{tag}-{}", std::process::id())));
    let world = game.join("saves").join("world");
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    w.allow_commands = true;
    create_world(&world, &data, &w).expect("world creation");
    let pack = world.join("datapacks").join("lt");
    let empty_tag = pack.join("data").join("lt").join("tags").join("function").join("empty.json");
    std::fs::create_dir_all(empty_tag.parent().expect("tag folder")).expect("pack folders");
    std::fs::write(pack.join("pack.mcmeta"), PACK_META).expect("pack meta");
    std::fs::write(&empty_tag, r#"{"values":[]}"#).expect("empty tag");
    let mut p = Probe::open(&world, data);
    p.s.loaded = true;
    p.s.teleport = None;
    let src = p.s.server_source();
    assert!(p.s.perform(src, "scoreboard objectives add lt dummy").0, "objective");
    let _ = p.take_events();
    p
}

/// The command run by an operator: (success, value, what it sent).
fn operator(p: &mut Probe, text: &str) -> (bool, i32, String) {
    let _ = p.take_events();
    let mut src = p.s.player_source();
    src.level = 4;
    let (ok, value) = p.s.perform(src, text);
    let sent: Vec<String> = p.take_events().iter().map(|e| format!("{e:?}")).collect();
    (ok, value, sent.join("\n"))
}

/// `function` on a tag holding no function fails with the game's "no functions" reply. Before: an
/// unknown tag got the argument's "unknown tag" error instead.
#[test]
fn function_on_a_tag_holding_no_function_answers_no_functions() {
    let mut p = new_world("function");
    for tag in ["lt:missing", "lt:empty"] {
        let (ok, _, sent) = operator(&mut p, &format!("function #{tag}"));
        assert!(!ok, "{tag}: accepted");
        assert!(sent.contains("commands.function.scheduled.no_functions") && sent.contains(tag), "{tag}: {sent}");
    }
}

/// `execute if|unless function` on a tag holding no function stops the chain, whatever the
/// condition expects. Before: `unless` on the empty tag passed and ran its command, and an unknown
/// tag was refused with "unknown tag".
#[test]
fn a_function_condition_on_a_tag_holding_no_function_stops_the_chain() {
    let mut p = new_world("condition");
    for tag in ["lt:missing", "lt:empty"] {
        for condition in ["if", "unless"] {
            let holder = format!("#{condition}_{}", tag.trim_start_matches("lt:"));
            let command = format!("execute {condition} function #{tag} run scoreboard players set {holder} lt 1");
            let (_, _, sent) = operator(&mut p, &command);
            assert_eq!(p.s.cmd.scores.get(&holder, "lt"), None, "{condition} {tag}: the chain went on");
            assert!(!sent.contains("arguments.function.tag.unknown"), "{condition} {tag}: {sent}");
        }
    }
}

/// `debug function` on a tag holding no function traces nothing and answers with zero functions,
/// like the game. Before: an unknown tag was refused with "unknown tag".
#[test]
fn debug_function_on_a_tag_holding_no_function_traces_nothing() {
    let mut p = new_world("debug");
    let (ok, value, sent) = operator(&mut p, "debug function #lt:missing");
    assert!(ok, "{sent}");
    assert_eq!(value, 0);
    assert!(sent.contains("commands.debug.function.success.multiple"), "{sent}");
}
