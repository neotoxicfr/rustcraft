//! Command execution like the game (wave W11): `return run`, macros, line continuation,
//! rejection of an invalid function, command quota, `execute on`, scoreboard operations.
//! Test world `RUSTCRAFT_SOLO_WORLD`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::server::Server;

const FUNCTIONS: &[(&str, &str)] = &[
    // `return run`: the command runs, its result is returned, the rest is skipped.
    (
        "ret",
        "scoreboard players set #after w 0\nreturn run scoreboard players set #r w 5\nscoreboard players set #after w 1\n",
    ),
    // `return run` on a failing command: the function stops too.
    ("retfail", "return run kill @e[tag=nobody]\nscoreboard players set #after2 w 1\n"),
    ("mac", "# macro\n$scoreboard players set #m w $(v)\nscoreboard players add #m w 1\n"),
    ("cont", "scoreboard players \\\n    set #c w \\\n 3\n"),
    ("bad", "scoreboard players set #bad w 1\n/say hi\n"),
    ("badline", "scoreboard players set #bad w 1\nscoreboard bogus\n"),
    ("rec", "scoreboard players add #n w 1\nexecute if score #n w matches ..999 run function t:rec\n"),
    (
        "q",
        "scoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\nscoreboard players add #q w 1\n",
    ),
    ("val", "return 42\n"),
    ("noret", "scoreboard players set #nr w 1\n"),
    // Block tag defined by a pack: the function loads.
    (
        "ptag",
        "execute if block ~ ~ ~ #t:foo run scoreboard players set #pt w 1
scoreboard players set #pt2 w 1
",
    ),
    (
        "inc",
        "scoreboard players add #i w 1
",
    ),
    (
        "retmulti",
        "execute as @e[tag=w11pig] run return run function t:inc
scoreboard players set #rm w 1
",
    ),
    (
        "retexc",
        "return run execute in minecraft:nope run say a
scoreboard players set #re w 1
",
    ),
    (
        "partbad",
        "scoreboard players set #pb w 1
particle dust{scale:5} ~ ~ ~ 0 0 0 0 1
",
    ),
    ("outer", "scoreboard players set #o w 0\nreturn run function t:val\nscoreboard players set #o w 1\n"),
];

pub(super) fn open(tag: &str) -> Option<(Server, rc_client::Session, std::path::PathBuf)> {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data missing: test skipped");
        return None;
    };
    let copy = crate::tests::fresh_copy(&world, tag);
    let pack = copy.join("datapacks").join("w11");
    std::fs::create_dir_all(pack.join("data/t/function")).unwrap();
    std::fs::write(pack.join("pack.mcmeta"), r#"{"pack":{"pack_format":121,"description":"t"}}"#).unwrap();
    for (name, body) in FUNCTIONS {
        std::fs::write(pack.join(format!("data/t/function/{name}.mcfunction")), body).unwrap();
    }
    std::fs::create_dir_all(pack.join("data/t/tags/block")).unwrap();
    std::fs::write(pack.join("data/t/tags/block/foo.json"), r#"{"values":["minecraft:stone"]}"#).unwrap();
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("W11");
    let (session, peer) = rc_client::memory::session(Arc::new(data.clone()), uuid).expect("session");
    let opts = crate::Options {
        world: copy.clone(),
        name: "W11".into(),
        uuid,
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 12,
    };
    let s = match Server::open(opts, uuid, Arc::new(data), peer, Arc::new(AtomicBool::new(false))) {
        Ok(s) => s,
        Err((_, e)) => panic!("open: {e}"),
    };
    Some((s, session, copy))
}

pub(super) fn run(s: &mut Server, c: &str) -> (bool, i32) {
    let mut src = s.player_source();
    src.level = 4;
    s.perform(src, c)
}

pub(super) fn chats(session: &rc_client::Session) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(e) = session.events.try_recv() {
        if let rc_client::Event::Chat { component, .. } = e {
            out.push(format!("{component:?}"));
        }
    }
    out
}

fn score(s: &Server, h: &str) -> Option<i32> {
    s.cmd.scores.get(h, "w")
}

/// Test threads with a large stack (deep function recursion, like the singleplayer thread).
pub(super) fn big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(crate::server::SOLO_STACK).spawn(f).unwrap().join().unwrap();
}

#[test]
fn functions_like_the_game() {
    big_stack(|| {
        let Some((mut s, session, _)) = open("w11-fn") else { return };
        s.loaded = true;
        assert!(run(&mut s, "scoreboard objectives add w dummy").0);
        chats(&session);

        // `return run` (before: "unknown command", the rest ran).
        run(&mut s, "function t:ret");
        assert_eq!(score(&s, "#r"), Some(5));
        assert_eq!(score(&s, "#after"), Some(0));
        let c = chats(&session);
        assert!(
            c.iter()
                .any(|m| m.contains("commands.function.result") && m.contains("Int(5)") || m.contains("Text(\"5\")")),
            "{c:?}"
        );
        run(&mut s, "function t:retfail");
        assert_eq!(score(&s, "#after2"), None);
        // `return run function` returns the result of the called function.
        run(&mut s, "function t:outer");
        assert_eq!(score(&s, "#o"), Some(0));
        run(&mut s, "execute store result score #st w run function t:outer");
        assert_eq!(score(&s, "#st"), Some(42));
        // Function without `return`: no result, `store` touches nothing.
        run(&mut s, "scoreboard players set #st2 w 9");
        run(&mut s, "execute store result score #st2 w run function t:noret");
        assert_eq!(score(&s, "#st2"), Some(9));
        assert_eq!(score(&s, "#nr"), Some(1));

        // Macros.
        chats(&session);
        run(&mut s, "function t:mac {v:7}");
        assert_eq!(score(&s, "#m"), Some(8));
        run(&mut s, "data modify storage t:s a set value {v:20b}");
        run(&mut s, "function t:mac with storage t:s a");
        assert_eq!(score(&s, "#m"), Some(21));
        chats(&session);
        assert!(!run(&mut s, "function t:mac").0);
        let c = chats(&session);
        assert!(
            c.iter().any(|m| m.contains("commands.function.instantiationFailure") && m.contains("missing_arguments")),
            "{c:?}"
        );
        run(&mut s, "function t:mac {w:1}");
        let c = chats(&session);
        assert!(c.iter().any(|m| m.contains("commands.function.error.missing_argument")), "{c:?}");
        run(&mut s, "function t:mac {v:\"x y\"}");
        let c = chats(&session);
        assert!(c.iter().any(|m| m.contains("commands.function.error.parse")), "{c:?}");
        run(&mut s, "function t:mac with storage t:s a.v");
        let c = chats(&session);
        assert!(c.iter().any(|m| m.contains("argument_not_compound") && m.contains("TAG_Byte")), "{c:?}");

        // Line continuation.
        run(&mut s, "function t:cont");
        assert_eq!(score(&s, "#c"), Some(3));

        // An invalid line makes the whole function rejected (nothing runs).
        chats(&session);
        for f in ["t:bad", "t:badline"] {
            assert!(!run(&mut s, &format!("function {f}")).0);
            let c = chats(&session);
            assert!(c.iter().any(|m| m.contains("arguments.function.unknown")), "{c:?}");
        }
        assert_eq!(score(&s, "#bad"), None);

        // Recursion of 1000 levels: the game has no depth limit (before: 512).
        super::exec::STACK_PROBE.with(|p| p.borrow_mut().clear());
        run(&mut s, "function t:rec");
        assert_eq!(score(&s, "#n"), Some(1000));
        let probe = super::exec::STACK_PROBE.with(|p| p.borrow().clone());
        let (top, low) = (probe[0].1, probe.iter().map(|p| p.1).min().unwrap());
        let per = (top - low) / (probe.len() - 1);
        eprintln!("stack: {per} bytes per function level ({} levels)", probe.len());
        // The depth guard fits in the stack of the singleplayer thread.
        assert!(per * super::exec::MAX_NESTING as usize * 2 < crate::server::SOLO_STACK, "{per}");

        // Command quota: call (1) + 9 commands = 10.
        run(&mut s, "gamerule max_command_sequence_length 10");
        run(&mut s, "function t:q");
        assert_eq!(score(&s, "#q"), Some(9));
        run(&mut s, "gamerule max_command_sequence_length 65536");

        // Fork limit: `execute as` on 3 entities with a limit of 2.
        run(&mut s, "gamerule max_command_forks 2");
        for _ in 0..3 {
            run(&mut s, "summon minecraft:armor_stand ~ ~ ~ {Tags:[\"w11f\"],NoGravity:1b}");
        }
        chats(&session);
        assert!(!run(&mut s, "execute as @e[tag=w11f] run scoreboard players add #fk w 1").0);
        // Error in forked mode: no message, nothing executed (like the game).
        assert!(chats(&session).is_empty());
        assert_eq!(score(&s, "#fk"), None);
        run(&mut s, "gamerule max_command_forks 65536");
        run(&mut s, "kill @e[tag=w11f]");

        // `return fail` from the keyboard: no message (before: empty red line).
        chats(&session);
        assert!(!run(&mut s, "return fail").0);
        assert!(chats(&session).is_empty());
    });
}

#[test]
fn execute_on_relations() {
    big_stack(|| {
        let Some((mut s, _session, _)) = open("w11-on") else { return };
        s.loaded = true;
        run(&mut s, "scoreboard objectives add w dummy");
        run(&mut s, "summon minecraft:pig ~ ~ ~ {Tags:[\"w11pig\"]}");
        run(&mut s, "summon minecraft:zombie ~ ~ ~ {Tags:[\"w11z\"],NoAI:1b}");
        assert!(run(&mut s, "ride @e[tag=w11z,limit=1] mount @e[tag=w11pig,limit=1]").0);
        run(&mut s, "execute as @e[tag=w11z] on vehicle run tag @s add w11v");
        assert_eq!(run(&mut s, "execute if entity @e[tag=w11pig,tag=w11v]"), (true, 1));
        run(&mut s, "execute as @e[tag=w11pig] on passengers run tag @s add w11p");
        assert_eq!(run(&mut s, "execute if entity @e[tag=w11z,tag=w11p]"), (true, 1));
        // Item dropped by the player: `origin` = the player.
        run(&mut s, "give @s minecraft:stone 1");
        s.drop_from_player_with(
            crate::player::Stack { id: "minecraft:stone".into(), count: 1, components: None },
            false,
        );
        assert!(run(&mut s, "execute as @e[type=item] on origin run scoreboard players set @s w 3").0);
        assert_eq!(s.cmd.scores.get("W11", "w"), Some(3));
    });
}

#[test]
fn score_operations_like_the_game() {
    let Some((mut s, session, _)) = open("w11-ops") else { return };
    s.loaded = true;
    run(&mut s, "scoreboard objectives add w dummy");
    run(&mut s, "scoreboard players set #a w 7");
    run(&mut s, "scoreboard players set #z w 0");
    chats(&session);
    for op in ["/=", "%="] {
        assert!(!run(&mut s, &format!("scoreboard players operation #a w {op} #z w")).0);
        let c = chats(&session);
        assert!(c.iter().any(|m| m.contains("arguments.operation.div0")), "{c:?}");
        assert_eq!(score(&s, "#a"), Some(7));
    }
    // `%=` of MIN by −1: 0, like Java's floor-rounded remainder (before: integrated server panic).
    run(&mut s, "scoreboard players set #min w -2147483648");
    run(&mut s, "scoreboard players set #m1 w -1");
    assert!(run(&mut s, "scoreboard players operation #min w %= #m1 w").0);
    assert_eq!(score(&s, "#min"), Some(0));
    run(&mut s, "scoreboard players set #min w -2147483648");
    assert!(run(&mut s, "scoreboard players operation #min w /= #m1 w").0);
    assert_eq!(score(&s, "#min"), Some(i32::MIN));
    run(&mut s, "scoreboard players set #n w -7");
    run(&mut s, "scoreboard players set #d w 2");
    run(&mut s, "scoreboard players operation #n w %= #d w");
    assert_eq!(score(&s, "#n"), Some(1));
}

fn events(session: &rc_client::Session) -> Vec<rc_client::Event> {
    let mut out = Vec::new();
    while let Ok(e) = session.events.try_recv() {
        out.push(e);
    }
    out
}

/// `tp`: relative flags sent to the player (momentum kept, view not re-aimed), look direction from
/// the feet, `rotate` = rotation-only packet, resolved against the source.
#[test]
fn teleport_and_rotate_like_the_game() {
    use rc_proto_v777::play::relative as r;
    let Some((mut s, session, _)) = open("w11-tp") else { return };
    s.loaded = true;
    s.yaw = 30.0;
    s.pitch = 10.0;
    let start = s.pos;
    events(&session);
    assert!(run(&mut s, "tp @s ~ ~ ~5").0);
    let tp = events(&session)
        .into_iter()
        .find_map(|e| if let rc_client::Event::Teleport(p) = e { Some(p) } else { None })
        .expect("position packet");
    // Relative X, Y, Z (+ velocity kept) and relative rotation +0 (before: everything absolute).
    assert_eq!(tp.relative, r::X | r::Y | r::Z | r::DELTA_X | r::DELTA_Y | r::DELTA_Z | r::YAW | r::PITCH);
    assert_eq!(tp.pos, [0.0, 0.0, 5.0]);
    assert_eq!((tp.yaw, tp.pitch), (0.0, 0.0));
    assert_eq!(s.pos, [start[0], start[1], start[2] + 5.0]);
    // Absolute on X only, rotation given as absolute yaw.
    assert!(run(&mut s, "tp @s 10 ~ ~ 90 ~").0);
    let tp = events(&session)
        .into_iter()
        .find_map(|e| if let rc_client::Event::Teleport(p) = e { Some(p) } else { None })
        .unwrap();
    assert_eq!(tp.relative, r::Y | r::Z | r::DELTA_Y | r::DELTA_Z | r::PITCH);
    assert_eq!(tp.pos[0], 10.5);
    assert_eq!((tp.yaw, tp.pitch), (90.0, 0.0));
    assert_eq!(s.yaw, 90.0);
    // `^`: relative on all three axes.
    assert!(run(&mut s, "tp @s ^ ^ ^1").0);
    let tp = events(&session)
        .into_iter()
        .find_map(|e| if let rc_client::Event::Teleport(p) = e { Some(p) } else { None })
        .unwrap();
    assert_eq!(tp.relative & (r::X | r::Y | r::Z), r::X | r::Y | r::Z);
    // Half-open limit: −30,000,000 accepted, y = 20,000,000 refused.
    assert!(run(&mut s, "tp @s -30000000 ~ ~").0);
    assert!(!run(&mut s, "tp @s ~ 20000000 ~").0);
    assert!(run(&mut s, "tp @s ~ 19999999 ~").0);
    run(&mut s, &format!("tp @s {} {} {}", start[0], start[1], start[2]));

    // `facing`: look direction from the player's feet (before: from the eyes).
    events(&session);
    let [x, y, z] = s.pos;
    assert!(run(&mut s, &format!("tp @s {x} {y} {z} facing {x} {y} {}", z + 10.0)).0);
    assert!((s.pitch).abs() < 1e-4, "{}", s.pitch);
    let look = events(&session)
        .into_iter()
        .find_map(|e| if let rc_client::Event::LookAt { eyes, .. } = e { Some(eyes) } else { None });
    assert_eq!(look, Some(false));
    // `execute anchored eyes`: from the eyes.
    assert!(run(&mut s, &format!("execute anchored eyes run tp @s {x} {y} {z} facing {x} {y} {}", z + 10.0)).0);
    assert!(s.pitch > 5.0, "{}", s.pitch);

    // `rotate @s ~10 ~`: rotation-only packet, yaw relative to the SOURCE.
    s.yaw = 0.0;
    s.pitch = 0.0;
    events(&session);
    assert!(run(&mut s, "rotate @s ~10 ~").0);
    let ev = events(&session);
    assert!(!ev.iter().any(|e| matches!(e, rc_client::Event::Teleport(_))));
    let rot = ev.iter().find_map(|e| match e {
        rc_client::Event::PlayerRotation { yaw, yaw_relative, pitch, pitch_relative } => {
            Some((*yaw, *yaw_relative, *pitch, *pitch_relative))
        }
        _ => None,
    });
    assert_eq!(rot, Some((10.0, true, 0.0, true)));
    assert_eq!(s.yaw, 10.0);
    // Relative against the source: a zombie turned by the player (yaw 10) takes 10 + 5.
    run(&mut s, "summon minecraft:zombie ~ ~ ~ {Tags:[\"w11r\"],NoAI:1b,Rotation:[100f,0f]}");
    assert!(run(&mut s, "rotate @e[tag=w11r,limit=1] ~5 ~").0);
    let z = s.arg_entity(&s.player_source(), "@e[tag=w11r,limit=1]", false).ok().unwrap();
    assert!((s.ent_rot(z)[0] - 15.0).abs() < 1e-4, "{:?}", s.ent_rot(z));
}

/// `particle`: options passed through (color and size of `dust`, block state, item), `delta`
/// resolved against the source, distance from the center of the player's block.
#[test]
fn particle_options_like_the_game() {
    let Some((mut s, session, _)) = open("w11-part") else { return };
    s.loaded = true;
    let part = |session: &rc_client::Session| {
        events(session).into_iter().find_map(|e| if let rc_client::Event::Particles(p) = e { Some(p) } else { None })
    };
    events(&session);
    assert!(run(&mut s, "particle dust{color:[1.0,0.5,0.0],scale:2} ~ ~ ~ 0 0 0 0 1").0);
    let p = part(&session).expect("particle");
    assert_eq!(p.particle.color, 0xFFFF_7F00);
    assert_eq!(p.particle.scale, 2.0);
    assert!(run(&mut s, "particle dust{color:16711680,scale:1} ~ ~ ~ 0 0 0 0 1").0);
    assert_eq!(part(&session).unwrap().particle.color, 0xFF0000);
    // Transition: end color and scale passed through.
    assert!(run(&mut s, "particle dust_color_transition{from_color:255,to_color:65280,scale:3} ~ ~ ~ 0 0 0 0 1").0);
    let p = part(&session).unwrap().particle;
    assert_eq!((p.color, p.color2, p.scale), (255, 65280, 3.0));
    assert!(run(&mut s, "particle block{block_state:\"minecraft:stone\"} ~ ~ ~ 0 0 0 0 1").0);
    let stone = s.states.state_of(&s.data, &rc_proto::nbt::Nbt::String("minecraft:stone".into())).unwrap();
    assert_eq!(part(&session).unwrap().particle.state, stone as i32);
    assert!(
        run(&mut s, "particle block{block_state:{Name:\"minecraft:oak_log\",Properties:{axis:\"x\"}}} ~ ~ ~ 0 0 0 0 1")
            .0
    );
    assert_ne!(part(&session).unwrap().particle.state, 0);
    assert!(run(&mut s, "particle item{item:\"minecraft:diamond\"} ~ ~ ~ 0 0 0 0 1").0);
    assert!(part(&session).unwrap().particle.item > 0);
    assert!(run(&mut s, "particle entity_effect{color:[1.0,0.0,0.0,0.5]} ~ ~ ~ 0 0 0 0 1").0);
    assert_eq!(part(&session).unwrap().particle.color, 0x7FFF_0000);
    // `delta` `~1 ~1 ~1` = source position + 1 (like the game).
    let [x, y, z] = s.pos;
    assert!(run(&mut s, "particle flame ~ ~ ~ ~1 ~1 ~1 0 1").0);
    let d = part(&session).unwrap().delta;
    assert_eq!(d, [(x + 1.0) as f32, (y + 1.0) as f32, (z + 1.0) as f32]);
    // Scale out of bounds refused.
    assert!(!run(&mut s, "particle dust{color:[1.0,0.0,0.0],scale:5} ~ ~ ~ 0 0 0 0 1").0);
    // Distance from the center of the block: 31.9 from the center passes, 32 does not.
    let cx = s.pos[0].floor() + 0.5;
    let (py, pz) = (s.pos[1].floor() + 0.5, s.pos[2].floor() + 0.5);
    assert!(run(&mut s, &format!("particle flame {} {py} {pz} 0 0 0 0 1", cx + 31.9)).0);
    assert!(!run(&mut s, &format!("particle flame {} {py} {pz} 0 0 0 0 1", cx + 32.0)).0);
}

/// Level-up sounds received since the last read.
fn levelups(s: &Server, session: &rc_client::Session) -> usize {
    let id = s
        .data
        .registry("minecraft:sound_event")
        .and_then(|r| r.entries.iter().position(|e| e == "minecraft:entity.player.levelup"))
        .unwrap() as i32;
    events(session)
        .into_iter()
        .filter(|e| matches!(e, rc_client::Event::Sound(p) if p.sound == rc_proto_v777::play::SoundRef::Id(id)))
        .count()
}

/// `xp add`: level-up sound on a multiple of 5 (100 ticks between two), saturated addition,
/// total reset to 0 below level 0, `xp set … points` clamped without touching the total.
#[test]
fn experience_like_the_game() {
    let Some((mut s, session, _)) = open("w11-xp") else { return };
    s.loaded = true;
    s.mobs.player_ticks = 1000;
    s.life.vit.last_level_up = 0;
    run(&mut s, "xp set @s 0 levels");
    events(&session);
    run(&mut s, "xp add @s 5 levels");
    assert_eq!(levelups(&s, &session), 1);
    // Less than 100 ticks later: no second sound.
    s.mobs.player_ticks += 50;
    run(&mut s, "xp add @s 5 levels");
    assert_eq!(levelups(&s, &session), 0);
    // Points crossing level 15, 101 ticks later: sound.
    s.mobs.player_ticks += 101;
    run(&mut s, "xp set @s 14 levels");
    run(&mut s, "xp add @s 40 points");
    assert_eq!(s.life.vit.xp_level, 15);
    assert_eq!(levelups(&s, &session), 1);
    // Saturated addition.
    run(&mut s, "xp add @s 2147483647 levels");
    run(&mut s, "xp add @s 2147483647 levels");
    assert_eq!(s.life.vit.xp_level, i32::MAX);
    // Negative points at level 0: total reset to 0.
    run(&mut s, "xp set @s 0 levels");
    run(&mut s, "xp set @s 3 points");
    s.life.vit.xp_total = 50;
    run(&mut s, "xp add @s -10 points");
    assert_eq!((s.life.vit.xp_level, s.life.vit.xp_total), (0, 0));
    // `xp set … points`: progress clamped, total unchanged.
    s.life.vit.xp_total = 77;
    run(&mut s, "xp set @s 5 levels");
    assert!(run(&mut s, "xp set @s 16 points").0);
    assert_eq!(s.life.vit.xp_total, 77);
    assert!((s.life.vit.xp_progress - 16.0 / 17.0).abs() < 1e-6);
}

/// `clear` with a maximum count: hotbar first (internal inventory order), then the crafting
/// grid, then the cursor item; `give`: dummy item, surplus without delay.
#[test]
fn clear_and_give_like_the_game() {
    let Some((mut s, _session, _)) = open("w11-clear") else { return };
    s.loaded = true;
    for x in s.slots.iter_mut() {
        *x = None;
    }
    let dirt = |n: i32| Some(crate::player::Stack { id: "minecraft:dirt".into(), count: n, components: None });
    s.slots[1] = dirt(3); // crafting grid
    s.slots[9] = dirt(4); // main inventory
    s.slots[36] = dirt(2); // hotbar, slot 0
    s.life.inv_menu.carried = dirt(6);
    // The game removes the hotbar first (2), then the main inventory (3 of 4); the grid stays.
    assert_eq!(run(&mut s, "clear @s minecraft:dirt 5"), (true, 5));
    assert_eq!(s.slots[36], None);
    assert_eq!(s.slots[9].as_ref().map(|x| x.count), Some(1));
    assert_eq!(s.slots[1].as_ref().map(|x| x.count), Some(3));
    // Count: cursor included (1 + 3 + 6).
    assert_eq!(run(&mut s, "clear @s minecraft:dirt 0"), (true, 10));
    assert!(run(&mut s, "clear @s minecraft:dirt").0);
    assert_eq!(s.life.inv_menu.carried, None);

    // `give`: stack stored → dummy item (never pickable, age 5999).
    let before = s.life.drops.len();
    assert!(run(&mut s, "give @s minecraft:stone 1").0);
    let fake = s.life.drops.get(before).expect("dummy item");
    assert_eq!((fake.pickup_delay, fake.age, fake.stack.count), (32767, 5999, 1));
    // Survival, inventory almost full: 2 diamonds stored, the 3rd dropped without delay (before:
    // lost).
    run(&mut s, "gamemode survival");
    for i in 9..45 {
        s.slots[i] = Some(crate::player::Stack { id: "minecraft:bedrock".into(), count: 64, components: None });
    }
    s.slots[9] = Some(crate::player::Stack { id: "minecraft:diamond".into(), count: 62, components: None });
    s.slots[45] = None;
    let before = s.life.drops.len();
    assert!(run(&mut s, "give @s minecraft:diamond 3").0);
    assert_eq!(s.slots[9].as_ref().map(|x| x.count), Some(64));
    let d = s.life.drops.iter().skip(before).find(|d| d.stack.id == "minecraft:diamond").expect("surplus");
    assert_eq!((d.pickup_delay, d.stack.count), (0, 1));
    assert_eq!(d.thrower, None);
}

/// Small differences in the "commands" batch: `gamerule` compared by value with the short name,
/// `setworldspawn` wrapped and clamped, oversized `item … with` refused, the game's `swing`.
#[test]
fn small_command_details() {
    let Some((mut s, session, _)) = open("w11-misc") else { return };
    s.loaded = true;
    run(&mut s, "gamerule random_tick_speed 3");
    chats(&session);
    assert!(!run(&mut s, "gamerule random_tick_speed 03").0);
    let c = chats(&session);
    assert!(
        c.iter().any(|m| m.contains("commands.gamerule.not_set")
            && m.contains("\"random_tick_speed\"")
            && !m.contains("minecraft:random_tick_speed")),
        "{c:?}"
    );
    assert!(run(&mut s, "gamerule minecraft:random_tick_speed 4").0);
    run(&mut s, "gamerule random_tick_speed 3");

    assert!(run(&mut s, "setworldspawn ~ ~ ~ 270 120").0);
    assert_eq!((s.level.spawn_yaw, s.level.spawn_pitch), (-90.0, 90.0));

    chats(&session);
    assert!(!run(&mut s, "item replace entity @s weapon.mainhand with minecraft:diamond_sword 5").0);
    let c = chats(&session);
    assert!(c.iter().any(|m| m.contains("arguments.item.overstacked")), "{c:?}");
    assert!(run(&mut s, "item replace entity @s weapon.mainhand with minecraft:stone 5").0);

    // `swing`: the game's message; non-living entity refused.
    chats(&session);
    assert!(run(&mut s, "swing @s offhand stab 10t").0);
    let c = chats(&session);
    assert!(c.iter().any(|m| m.contains("commands.swing.success.single")), "{c:?}");
    run(&mut s, "summon minecraft:marker ~ ~ ~ {Tags:[\"w11i\"]}");
    chats(&session);
    assert!(!run(&mut s, "swing @e[tag=w11i]").0);
    assert!(chats(&session).iter().any(|m| m.contains("commands.swing.failed.notliving")));
}

/// Game functions: pack tags known at compile time, `return run function` with several sources,
/// exception after `return run`, particle options refused at parse time, `tp` that resets the
/// horizontal speed to zero on absolute axes.
#[test]
fn functions_and_tp_like_the_game_2() {
    big_stack(|| {
        let Some((mut s, session, _)) = open("w11-fn2") else { return };
        s.loaded = true;
        assert!(run(&mut s, "scoreboard objectives add w dummy").0);
        chats(&session);

        // Block tag of a pack: the function is loaded, its other lines run.
        run(&mut s, "function t:ptag");
        assert_eq!(score(&s, "#pt2"), Some(1));

        // Several sources: only the first calls the function.
        run(&mut s, "summon pig ~ ~ ~ {Tags:[\"w11pig\"]}");
        run(&mut s, "summon pig ~ ~ ~ {Tags:[\"w11pig\"]}");
        run(&mut s, "function t:retmulti");
        assert_eq!(score(&s, "#i"), Some(1));
        assert_eq!(score(&s, "#rm"), None);

        // Exception after `return run`: the function stops.
        run(&mut s, "function t:retexc");
        assert_eq!(score(&s, "#re"), None);

        // Invalid particle options: refused at parse time.
        run(&mut s, "function t:partbad");
        assert_eq!(score(&s, "#pb"), None);
        assert!(!run(&mut s, "particle dust ~ ~ ~ 0 0 0 0 1").0);
        let n = s.mobs.list.iter().flatten().count();
        assert!(!run(&mut s, "execute summon pig run particle dust{scale:5} ~ ~ ~ 0 0 0 0 1").0);
        assert_eq!(s.mobs.list.iter().flatten().count(), n);
        assert!(run(&mut s, "particle dust{color:16711680,scale:1} ~ ~ ~ 0 0 0 0 1").0);

        // `tp`: horizontal speed lost on absolute axes, kept on relative ones.
        run(&mut s, "kill @e[type=item]");
        let item = "summon item ~ ~ ~ {Item:{id:\"minecraft:stone\",count:1},Motion:[0.5,0.5,0.5]}";
        run(&mut s, item);
        let vel = |s: &Server| s.life.drops.iter().last().map(|d| d.vel).unwrap();
        assert!(vel(&s)[0] != 0.0);
        run(&mut s, "tp @e[type=item] 0 64 0");
        let v = vel(&s);
        assert_eq!((v[0], v[1], v[2]), (0.0, 0.0, 0.0), "{v:?}");
        run(&mut s, "kill @e[type=item]");
        run(&mut s, item);
        run(&mut s, "tp @e[type=item] ~1 64 0");
        let v = vel(&s);
        assert!(v[0] != 0.0 && v[2] == 0.0 && v[1] == 0.0, "{v:?}");
    });
}
