//! G3-F06: in-game tests (`/test`, test instance blocks) and the relay of command successes to
//! the operators. Fresh flat world with a data pack carrying block-based test instances and
//! their templates (`data/gt/structure/*.nbt`): the 26.3 cache and the client jar are required
//! (their absence makes the tests FAIL).

use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto::{BlockPos, Reader};
use rc_text::{Component, Content};

use crate::blocks::probe::{Probe, state_string};
use crate::server::Server;

const Y: i32 = -60;
/// First test of a run: 3 blocks south of the player (8.5, Y, 8.5), at the surface.
const FIRST: [i32; 3] = [8, Y, 11];

struct W {
    p: Probe,
}

/// Test block of a template: relative position, mode, message.
fn test_block(pos: [i32; 3], mode: &str, message: &str, palette: usize) -> Nbt {
    Nbt::Compound(vec![
        ("pos".into(), Nbt::List(pos.iter().map(|&v| Nbt::Int(v)).collect())),
        ("state".into(), Nbt::Int(palette as i32)),
        (
            "nbt".into(),
            Nbt::Compound(vec![
                ("id".into(), Nbt::String("minecraft:test_block".into())),
                ("mode".into(), Nbt::String(mode.into())),
                ("message".into(), Nbt::String(message.into())),
                ("powered".into(), Nbt::Byte(0)),
            ]),
        ),
    ])
}

fn template(size: [i32; 3], blocks: &[([i32; 3], &str, &str)]) -> Vec<u8> {
    let modes: Vec<&str> = {
        let mut m: Vec<&str> = Vec::new();
        for b in blocks {
            if !m.contains(&b.1) {
                m.push(b.1);
            }
        }
        m
    };
    let palette = modes
        .iter()
        .map(|m| {
            Nbt::Compound(vec![
                ("Name".into(), Nbt::String("minecraft:test_block".into())),
                ("Properties".into(), Nbt::Compound(vec![("mode".into(), Nbt::String((*m).into()))])),
            ])
        })
        .collect();
    let list = blocks.iter().map(|b| test_block(b.0, b.1, b.2, modes.iter().position(|m| *m == b.1).unwrap_or(0))).collect();
    let root = Nbt::Compound(vec![
        ("size".into(), Nbt::List(size.iter().map(|&v| Nbt::Int(v)).collect())),
        ("palette".into(), Nbt::List(palette)),
        ("blocks".into(), Nbt::List(list)),
        ("entities".into(), Nbt::List(Vec::new())),
        ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
    ]);
    crate::nbtio::gzip(&crate::nbtio::encode_root(&root))
}

/// Data pack `gt`: accept (start next to accept), fail (start next to a fail block "boom",
/// accept out of reach, environment turning time off), a missing function, a flaky test.
fn write_pack(world: &std::path::Path) {
    let pack = world.join("datapacks").join("gt");
    for d in ["data/gt/test_instance", "data/gt/structure"] {
        std::fs::create_dir_all(pack.join(d)).expect("pack folders");
    }
    std::fs::write(pack.join("pack.mcmeta"), r#"{"pack":{"pack_format":121,"description":"gt"}}"#).expect("meta");
    let tests = [
        ("accept", r#"{"type":"block_based","environment":"minecraft:default","structure":"gt:accept","max_ticks":20}"#),
        (
            "fail",
            r#"{"type":"block_based","environment":{"type":"game_rules","rules":{"advance_time":false}},"structure":"gt:fail","max_ticks":20}"#,
        ),
        ("nofn", r#"{"type":"function","function":"gt:nope","environment":"minecraft:default","structure":"minecraft:empty","max_ticks":5}"#),
    ];
    for (name, body) in tests {
        std::fs::write(pack.join(format!("data/gt/test_instance/{name}.json")), body).expect("test instance");
    }
    let accept = template([2, 1, 1], &[([0, 0, 0], "start", ""), ([1, 0, 0], "accept", "")]);
    std::fs::write(pack.join("data/gt/structure/accept.nbt"), accept).expect("accept template");
    let fail = template([4, 1, 1], &[([0, 0, 0], "start", ""), ([1, 0, 0], "fail", "boom"), ([3, 0, 0], "accept", "")]);
    std::fs::write(pack.join("data/gt/structure/fail.nbt"), fail).expect("fail template");
}

fn world(tag: &str) -> W {
    let data = Arc::new(rc_testdata::version_data().expect("G3-F06: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "G3-F06: 26.3 client jar missing");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-g3f06-{tag}-{}", std::process::id())));
    let mut nw = crate::newworld::NewWorld::new(tag);
    nw.seed = 7;
    nw.preset = crate::newworld::Preset::Flat;
    nw.allow_commands = true;
    nw.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &nw).expect("world creation");
    write_pack(&dir);
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-2..=3).flat_map(|x| (-2..=3).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "G3-F06: flat world generation too slow");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.s.pos = [8.5, f64::from(Y), 8.5];
    p.s.game_mode = 1;
    let _ = p.take_events();
    W { p }
}

fn key(c: &Component) -> Option<String> {
    match &c.content {
        Content::Translate { key, .. } => Some(key.clone()),
        _ => c.extra.first().and_then(key),
    }
}

impl W {
    fn cmd(&mut self, text: &str) -> bool {
        let src = self.p.s.player_source();
        self.p.s.perform(src, text).0
    }

    fn tick(&mut self, n: usize) {
        for _ in 0..n {
            self.p.s.game_tick();
            self.p.s.tick += 1;
        }
    }

    fn chats(&mut self) -> Vec<Component> {
        self.p
            .take_events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Chat { component, .. } => Some(component),
                _ => None,
            })
            .collect()
    }

    fn name(&self, at: [i32; 3]) -> String {
        state_string(&self.p.s, self.p.s.get(at))
    }

    fn data(&self, at: [i32; 3]) -> crate::gametest::Data {
        self.p.s.ti_data(at).expect("test instance block")
    }

    fn action(&mut self, at: [i32; 3], action: i32, test: Option<&str>) -> Vec<Event> {
        let mut w = Vec::new();
        rc_proto_v777::special::write_test_instance_action(&mut w, BlockPos::new(at[0], at[1], at[2]), action, test, [1, 1, 1], 0, false);
        let id = self.p.s.peer.ids().test_instance_block_action;
        assert!(self.p.s.tech_packet(id, &mut Reader::new(&w)), "packet not consumed");
        self.p.take_events()
    }
}

fn plain(c: &Component) -> String {
    crate::commands::cmds_ents::plain(c)
}

fn texts(list: &[Component]) -> Vec<String> {
    list.iter().map(plain).collect()
}

/// `test run always_pass`: test instance block 3 blocks south of the player at the surface,
/// structure closed by barriers; one setup tick and the start delay, then it passes on its
/// third tick: green report to the players, block finished, barriers gone, summary.
#[test]
fn run_always_pass_lays_out_ticks_and_reports() {
    let mut w = world("always");
    assert!(w.cmd("test run always_pass"), "test run refused");
    let c = w.chats();
    let keys: Vec<String> = c.iter().filter_map(key).collect();
    assert!(keys.iter().any(|k| k == "commands.test.run.running"), "keys {keys:?}");
    assert_eq!(w.name(FIRST), "minecraft:test_instance_block", "block at the grid's first corner");
    let d = w.data(FIRST);
    assert_eq!((d.test.as_deref(), d.size, d.status), (Some("minecraft:always_pass"), [1, 1, 1], 0));
    let wall = [FIRST[0] - 1, FIRST[1] + 1, FIRST[2] + 1];
    assert_eq!(w.name(wall), "minecraft:barrier[waterlogged=false]", "structure closed");
    w.tick(2);
    assert!(!texts(&w.chats()).iter().any(|t| t.contains("passed")), "still in its setup ticks");
    w.tick(1);
    let c = w.chats();
    let t = texts(&c);
    let report = t.iter().find(|x| x.starts_with("minecraft:always_pass passed! (")).unwrap_or_else(|| panic!("{t:?}"));
    assert!(report.ends_with("ms / 0gameticks)"), "{report}");
    let green = c.iter().find(|x| plain(x) == *report).and_then(|x| x.style.color);
    assert_eq!(green, Some(0x55FF55));
    let keys: Vec<String> = c.iter().filter_map(key).collect();
    for k in ["commands.test.summary", "commands.test.summary.all_required_passed"] {
        assert!(keys.iter().any(|x| x == k), "{k} in {keys:?}");
    }
    assert_eq!(w.data(FIRST).status, 2, "finished");
    assert_eq!(w.name(wall), "minecraft:air", "barriers removed after the pass");
    assert!(w.p.s.cmd.tests.runner.as_ref().is_none_or(|r| r.ticked_len() == 0), "nothing left to tick");
}

/// Block-based test from a data pack (its template read from the pack): the start block powers
/// the accept block next to it, the test passes when it starts (setup 0: second tick).
#[test]
fn block_based_test_from_a_pack_passes_on_its_accept_block() {
    let mut w = world("accept");
    assert!(w.p.s.template_get("gt:accept").is_some(), "template of the data pack");
    assert!(w.cmd("test run gt:accept"));
    let start = [FIRST[0], FIRST[1] + 1, FIRST[2] + 1];
    assert_eq!(w.name(start), "minecraft:test_block[mode=start]");
    assert_eq!(w.name([start[0] + 1, start[1], start[2]]), "minecraft:test_block[mode=accept]");
    let _ = w.chats();
    w.tick(1);
    assert!(!texts(&w.chats()).iter().any(|t| t.contains("passed")));
    w.tick(1);
    let t = texts(&w.chats());
    assert!(t.iter().any(|x| x.starts_with("gt:accept passed! (") && x.ends_with("ms / 0gameticks)")), "{t:?}");
    assert_eq!(w.p.s.be_get(start).and_then(|b| crate::nbtio::int(b, "powered")), Some(1), "start block left powered");
}

/// A triggered fail block fails the test with its message; it stays triggered, so the next tick
/// (when the failure ends the test) fails again: the report carries tick 1. The environment
/// (time stopped) holds during the batch only; the failed test is remembered for `runfailed`.
#[test]
fn fail_block_fails_with_its_message_and_environment_is_restored() {
    let mut w = world("fail");
    assert!(w.p.s.rule("minecraft:advance_time", true));
    assert!(w.cmd("test run gt:fail"));
    assert!(!w.p.s.rule("minecraft:advance_time", true), "environment set up with the batch");
    let _ = w.chats();
    w.tick(2);
    assert!(!texts(&w.chats()).iter().any(|t| t.contains("failed")), "failure seen, test not finished yet");
    w.tick(1);
    let c = w.chats();
    let t = texts(&c);
    assert!(t.iter().any(|x| x == "gt:fail failed! boom on tick 1"), "{t:?}");
    let keys: Vec<String> = c.iter().filter_map(key).collect();
    assert!(keys.iter().any(|k| k == "commands.test.summary.failed"), "{keys:?}");
    let d = w.data(FIRST);
    assert_eq!(d.status, 2);
    assert_eq!(d.error.as_ref().map(plain).as_deref(), Some("boom on tick 1"));
    assert!(w.p.s.rule("minecraft:advance_time", true), "environment torn down after the batch");
    assert_eq!(w.p.s.cmd.tests.failed, vec!["gt:fail".to_owned()]);
    assert!(w.cmd("test runfailed"));
    let keys: Vec<String> = w.chats().iter().filter_map(key).collect();
    assert!(keys.iter().any(|k| k == "commands.test.run.running"), "{keys:?}");
}

/// A function test naming a function the game does not have fails when it starts.
#[test]
fn unknown_test_function_fails_at_start() {
    let mut w = world("nofn");
    assert!(w.cmd("test run gt:nofn"));
    w.tick(3);
    let t = texts(&w.chats());
    assert!(t.iter().any(|x| x == "gt:nofn failed! Trying to access missing test function: gt:nope"), "{t:?}");
}

/// `test run always_pass 2`: retry report padded like the game, then a second run in place.
#[test]
fn retries_report_and_run_again_in_place() {
    let mut w = world("retry");
    assert!(w.cmd("test run always_pass 2"));
    w.tick(3);
    let t = texts(&w.chats());
    let first = t.iter().find(|x| x.starts_with("[Run:")).unwrap_or_else(|| panic!("{t:?}"));
    let head = "[Run:    1, Ok:    1, Fail:    0, Left:    1]";
    assert!(first.starts_with(&format!("{head:<53}minecraft:always_pass passed! ")), "{first:?}");
    assert!(first.ends_with("ms"));
    w.tick(3);
    let t = texts(&w.chats());
    assert!(t.iter().any(|x| x.starts_with("[Run:    2, Ok:    2, Fail:    0, Left:    0]")), "{t:?}");
    let summary = w.p.s.cmd.tests.runner.as_ref().map(|r| r.infos.len());
    assert_eq!(summary, Some(2), "two tests tracked");
}

/// Screen of a test instance block: description and size answered; "run" starts the test in
/// place, its summary relayed to the host by the server source.
#[test]
fn test_instance_screen_queries_and_runs() {
    let mut w = world("screen");
    let at = [2, Y, 2];
    assert!(w.cmd(&format!("setblock {} {} {} minecraft:test_instance_block", at[0], at[1], at[2])));
    let ev = w.action(at, 0, Some("minecraft:always_pass"));
    let status = ev.iter().find_map(|e| match e {
        Event::TestInstanceStatus { status, size } => Some((status.clone(), *size)),
        _ => None,
    });
    let (status, size) = status.expect("status answered");
    assert_eq!(status.as_ref().and_then(key).as_deref(), Some("test_instance.description.type"));
    assert_eq!(size, None, "initial query: no size");
    let text = status.as_ref().map(plain).unwrap_or_default();
    assert!(text.contains("minecraft:always_pass") && text.contains("minecraft:empty"), "{text}");
    let ev = w.action(at, 1, Some("minecraft:always_pass"));
    assert!(ev.iter().any(|e| matches!(e, Event::TestInstanceStatus { size: Some([1, 1, 1]), .. })), "size query");
    let ev = w.action(at, 6, Some("minecraft:always_pass"));
    let keys: Vec<String> = ev
        .iter()
        .filter_map(|e| match e {
            Event::Chat { component, .. } => key(component),
            _ => None,
        })
        .collect();
    assert!(keys.iter().any(|k| k == "test_instance_block.starting"), "{keys:?}");
    w.tick(3);
    let c = w.chats();
    assert!(texts(&c).iter().any(|x| x.starts_with("minecraft:always_pass passed!")));
    let admin: Vec<String> = c.iter().filter(|x| key(x).as_deref() == Some("chat.type.admin")).map(plain).collect();
    assert!(admin.iter().any(|x| x.starts_with("[Server: Game Test complete!")), "{admin:?}");
    assert_eq!(w.data(at).status, 2);
}

/// `test create`, the reset error of the game (the clearing's), `test clearall`.
#[test]
fn create_reset_and_clear_test_structures() {
    let mut w = world("create");
    assert!(w.cmd("test create gt:made 3 2 3"));
    let d = w.data(FIRST);
    assert_eq!((d.test.as_deref(), d.size), (Some("gt:made"), [3, 2, 3]));
    let floor = [FIRST[0] + 2, FIRST[1] + 1, FIRST[2] + 3];
    assert_eq!(w.name(floor), "minecraft:bedrock");
    let _ = w.chats();
    assert!(!w.cmd("test resetclosest"), "unknown test: nothing to reset");
    let keys: Vec<String> = w.chats().iter().filter_map(key).collect();
    assert!(keys.iter().any(|k| k == "commands.test.error.non_existant_test"), "{keys:?}");
    assert!(keys.iter().any(|k| k == "commands.test.clear.error.no_tests"), "{keys:?}");
    assert!(w.cmd("test clearall"));
    assert_eq!(w.name(FIRST), "minecraft:air");
    assert_eq!(w.name(floor), "minecraft:air");
}

/// `test clearthat` looks through the player's camera: spectating an armor stand that faces the
/// test structure, the player (looking up) clears it.
#[test]
fn looked_at_test_goes_through_the_camera() {
    let mut w = world("camera");
    assert!(w.cmd("test create gt:made"));
    assert_eq!(w.name(FIRST), "minecraft:test_instance_block");
    assert!(w.cmd(&format!("summon minecraft:armor_stand 8.5 {Y} 6.5 {{Rotation:[0f,0f],NoGravity:1b}}")));
    assert!(w.cmd("gamemode spectator"));
    assert!(w.cmd("spectate @e[type=minecraft:armor_stand,limit=1]"));
    w.p.s.pitch = -90.0;
    let _ = w.chats();
    assert!(w.cmd("test clearthat"), "{:?}", texts(&w.chats()));
    assert_eq!(w.name(FIRST), "minecraft:air", "the structure the camera looks at");
}

/// Template priority: the world's `generated` folder before the packs.
#[test]
fn world_templates_come_before_the_packs() {
    let mut w = world("prio");
    let file = w.p.s.world.join("generated").join("gt").join("structure").join("accept.nbt");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("folder");
    std::fs::write(&file, template([5, 1, 1], &[([0, 0, 0], "start", "")])).expect("world template");
    let t = w.p.s.template_get("gt:accept").expect("template");
    assert_eq!(t.t.size, [5, 1, 1], "the world's copy wins");
    assert_eq!(w.p.s.template_get("gt:fail").map(|t| t.t.size), Some([4, 1, 1]), "the pack's otherwise");
    // An identifier climbing out of the structure folder never reads a pack's other files.
    let outside = w.p.s.world.join("datapacks").join("gt").join("x.nbt");
    std::fs::write(&outside, template([2, 1, 1], &[([0, 0, 0], "start", "")])).expect("file outside data/");
    assert!(w.p.s.template_get("gt:../../../x").is_none(), "path left the structure folder");
}

/// Test blocks around a corner, one per quadrant and mode, for the search passes.
fn scatter_test_blocks(w: &mut W, origin: [i32; 3]) {
    let modes = ["start", "log", "fail", "accept"];
    for (k, (dx, dz)) in [(4, 4), (-4, 4), (-4, -4), (4, -4), (9, 1), (-1, 9), (-9, -1), (1, -9)].into_iter().enumerate() {
        let q = [origin[0] + dx, origin[1] + (k as i32 % 3), origin[2] + dz];
        assert!(w.cmd(&format!("setblock {} {} {} minecraft:test_block[mode={}]", q[0], q[1], q[2], modes[k % 4])));
    }
}

/// The section pass finds the same test blocks, in the same order, as the cell by cell pass,
/// under the four rotations (a box crossing chunk and section borders).
#[test]
fn test_block_search_matches_the_cellwise_reference() {
    let mut w = world("scan");
    let origin = [1, Y + 1, 1];
    scatter_test_blocks(&mut w, origin);
    for rot in rc_worldgen::template::Rot::ALL {
        let rel = [12, 20, 12];
        let cells = w.p.s.test_blocks_cellwise(origin, rot, rel);
        let sections = w.p.s.test_blocks_by_section(origin, rot, rel);
        assert_eq!(cells, sections, "rotation {rot:?}");
        assert!(cells.iter().map(Vec::len).sum::<usize>() >= 2, "blocks seen under {rot:?}: {cells:?}");
    }
}

/// Figures of both passes on the largest structure (48³, two test blocks): run by hand with
/// `--ignored --nocapture` on a quiet machine.
#[test]
#[ignore]
fn bench_test_block_search() {
    let mut w = world("scanbench");
    let origin = [-20, Y + 1, -20];
    scatter_test_blocks(&mut w, [origin[0] + 24, origin[1], origin[2] + 24]);
    let rel = [48, 48, 48];
    let rot = rc_worldgen::template::Rot::None;
    for (name, f) in [
        ("cellwise", Server::test_blocks_cellwise as fn(&Server, [i32; 3], rc_worldgen::template::Rot, [i32; 3]) -> [Vec<[i32; 3]>; 4]),
        ("by section", Server::test_blocks_by_section),
    ] {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..20 {
                std::hint::black_box(f(&w.p.s, origin, rot, rel));
            }
            best = best.min(t.elapsed().as_secs_f64() * 1e3 / 20.0);
        }
        eprintln!("G3-F06 bench: {name}: {best:.3} ms per pass (48^3)");
    }
}

/// Command block successes reach the host (operator) as `[@: …]`, grey italic, when the game
/// relays that message and the block informs the operators; queries stay with the block;
/// `LastOutput` follows `send_command_feedback`, not `command_block_output`.
#[test]
fn command_block_successes_reach_the_operators_like_the_game() {
    let mut w = world("admin");
    let run = |w: &mut W, cmd: &str, extra: &str| -> (Vec<String>, bool) {
        let at = [0, Y, 0];
        let _ = w.cmd(&format!("setblock {} {} {} minecraft:air", at[0], at[1], at[2]));
        assert!(w.cmd(&format!(
            "setblock {} {} {} minecraft:command_block{{auto:1b,Command:\"{cmd}\"{extra}}}",
            at[0], at[1], at[2]
        )));
        let _ = w.chats();
        w.tick(2);
        let admin = w.chats().iter().filter(|c| key(c).as_deref() == Some("chat.type.admin")).map(plain).collect();
        let out = w.p.s.be_get(at).is_some_and(|b| b.get("LastOutput").is_some());
        (admin, out)
    };
    let (admin, out) = run(&mut w, "time set 1000", "");
    assert_eq!(admin, vec!["[@: Set minecraft:overworld to 1000 tick(s)]".to_owned()]);
    assert!(out);
    let (admin, out) = run(&mut w, "difficulty", "");
    assert!(admin.is_empty(), "a query stays with the block: {admin:?}");
    assert!(out);
    let (admin, out) = run(&mut w, "time set 2000", ",TrackOutput:0b");
    assert!(admin.is_empty() && !out, "no tracked output: no output at all");
    assert!(w.cmd("gamerule command_block_output false"));
    let (admin, out) = run(&mut w, "time set 3000", "");
    assert!(admin.is_empty(), "{admin:?}");
    assert!(out, "the block still keeps its output");
    assert!(w.cmd("gamerule command_block_output true"));
    assert!(w.cmd("gamerule send_command_feedback false"));
    let (admin, out) = run(&mut w, "time set 4000", "");
    assert!(admin.is_empty() && !out, "no feedback: neither relayed nor kept");
    // The host's own commands are never relayed back to the host.
    assert!(w.cmd("gamerule send_command_feedback true"));
    let _ = w.chats();
    assert!(w.cmd("time set 5000"));
    assert!(!w.chats().iter().any(|c| key(c).as_deref() == Some("chat.type.admin")));
}

/// A command block's output closes with its command, like the game's: `execute as @p run test
/// run` lays its grid around the player (not the block); the first batch starts inside the
/// command (kept in `LastOutput`, relayed once); the reports of the following ticks are neither
/// kept nor relayed. A profile lookup answered later leaves the block's output alone too.
#[test]
fn command_block_output_closes_with_its_command() {
    let mut w = world("cbclose");
    let at = [0, Y, 0];
    let place = |w: &mut W, cmd: &str| {
        let _ = w.cmd(&format!("setblock {} {} {} minecraft:air", at[0], at[1], at[2]));
        assert!(w.cmd(&format!(
            "setblock {} {} {} minecraft:command_block{{auto:1b,Command:\"{cmd}\"}}",
            at[0], at[1], at[2]
        )));
        let _ = w.chats();
    };
    let admin = |w: &mut W| -> Vec<String> {
        w.chats().iter().filter(|c| key(c).as_deref() == Some("chat.type.admin")).map(plain).collect()
    };
    let output = |w: &W| w.p.s.be_get(at).and_then(|b| b.get("LastOutput")).map(crate::commands::snbt::to_snbt);
    place(&mut w, "execute as @p run test run always_pass");
    let mut ticks = 0;
    while output(&w).is_none() {
        assert!(ticks < 4, "the command block never ran");
        w.tick(1);
        ticks += 1;
    }
    assert_eq!(w.name(FIRST), "minecraft:test_instance_block", "grid around the executing player");
    let kept = output(&w);
    assert!(kept.as_deref().is_some_and(|o| o.contains("commands.test.batch.starting")), "{kept:?}");
    assert_eq!(admin(&mut w).len(), 1, "the first batch is announced while the command runs");
    w.tick(6);
    assert_eq!(w.data(FIRST).status, 2, "the test finished");
    assert_eq!(admin(&mut w), Vec::<String>::new(), "nothing relayed once the command is over");
    assert_eq!(output(&w), kept, "nothing kept once the command is over");

    w.p.s.cmd.s9.profiles.services = crate::commands::cmds_s9e::Services { id_by_name: |_| None, profile: |_| None };
    place(&mut w, "fetchprofile name nobody");
    for _ in 0..100 {
        w.tick(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(output(&w), None, "the failure comes after the command: the block keeps nothing");
    assert_eq!(admin(&mut w), Vec::<String>::new());
}
