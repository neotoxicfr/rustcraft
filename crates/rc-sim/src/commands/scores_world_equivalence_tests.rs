//! Scoreboard switches end to end on a fresh flat world (26.3 version data required): the same
//! command script (scores written by selectors, `execute as @e run team join t @s` then the team
//! of every entity, operations, `store`, decorated names in the feedback, mobs killed, then 1,000
//! markers removed while a single fake player holds a score; criteria scores the server writes with
//! the display name of a killed pig) run on the former paths and on the
//! optimised ones, each in its own thread (same seeds). The scoreboard packets, the chat lines,
//! the save and the team of each entity must be identical.

use std::fmt::Write as _;

use rc_client::Event;

use super::scores::{BoardPaths, FORCED_PATHS};
use crate::blocks::probe::Probe;
use crate::nbtio::uuid_text;

const PIGS: u32 = 30;
const MARKERS: u32 = 1000;

/// UUID of the `i`-th summoned entity of a kind (fixed, so the holder names and their listing
/// order are the same in every run), as the four ints of the `UUID` field and as a number.
fn fixed_uuid(kind: u32, i: u32) -> ([i32; 4], u128) {
    let ints = [0x1000 + kind as i32, i as i32, 0x4000_0000 ^ (i as i32), (i.wrapping_mul(7919) ^ kind) as i32];
    let u = ints.iter().fold(0u128, |u, x| u << 32 | u128::from(*x as u32));
    (ints, u)
}

/// Scoreboard packets and chat lines sent to the player since the last call.
fn record_events(p: &Probe, trace: &mut String) {
    for e in p.take_events() {
        match e {
            Event::Score(score) => writeln!(trace, "  {score:?}").expect("trace"),
            Event::Chat { component, .. } => writeln!(trace, "  chat {component:?}").expect("trace"),
            _ => {}
        }
    }
}

fn run_command(p: &mut Probe, c: &str, trace: &mut String) {
    let mut src = p.s.player_source();
    src.level = 4;
    let (ok, result) = p.s.perform(src, c);
    writeln!(trace, "{c} -> {ok} {result}").expect("trace");
    record_events(p, trace);
}

fn team_of_every_pig(p: &Probe, trace: &mut String) {
    for i in 0..PIGS {
        let (_, u) = fixed_uuid(1, i);
        let team = p.s.cmd.scores.team_of(&uuid_text(u)).map(|t| t.name.clone());
        let rule = p.s.cmd.scores.collision_rule_by_uuid(u);
        writeln!(trace, "pig {i} team={team:?} push={rule:?}").expect("trace");
    }
}

fn board_state(p: &Probe, trace: &mut String) {
    let mut save = Vec::new();
    rc_proto::nbt::write_named(&mut save, "", &p.s.cmd.scores.to_nbt());
    writeln!(trace, "save {} bytes {save:?}", save.len()).expect("trace");
    writeln!(trace, "holders {:?}", p.s.cmd.scores.holders()).expect("trace");
}

fn ticks(p: &mut Probe, n: u32, trace: &mut String) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
        record_events(p, trace);
    }
}

/// The whole script on a fresh world whose board takes `paths`.
fn script(paths: BoardPaths) -> String {
    FORCED_PATHS.with(|f| f.set(Some(paths)));
    let (mut p, dir) = crate::tests_g3f13::open("scores-eq");
    assert_eq!(p.s.cmd.scores.paths(), paths, "the world's board ignores the forced paths");
    let mut trace = String::new();
    let _ = p.take_events();
    for c in [
        "gamerule minecraft:mob_drops false",
        "scoreboard objectives add o dummy",
        "scoreboard objectives add k dummy",
        "scoreboard objectives modify k displayautoupdate true",
        "scoreboard objectives setdisplay sidebar o",
        "scoreboard objectives setdisplay list k",
        "team add t",
        "team add u",
        "team modify t collisionRule pushOwnTeam",
        "team modify u color red",
        "team modify u prefix \"[u] \"",
    ] {
        run_command(&mut p, c, &mut trace);
    }
    for i in 0..PIGS {
        let ([a, b, c, d], _) = fixed_uuid(1, i);
        let (x, z) = (2 + (i % 6) as i32 * 2, -4 + (i / 6) as i32 * 2);
        let summon = format!("summon minecraft:pig {x} -60 {z} {{NoAI:1b,Silent:1b,UUID:[I;{a},{b},{c},{d}]}}");
        run_command(&mut p, &summon, &mut trace);
    }
    run_command(&mut p, "execute as @e[type=pig] run team join t @s", &mut trace);
    team_of_every_pig(&p, &mut trace);
    for c in [
        "scoreboard players add @e[type=pig] o 1",
        "scoreboard players add @e[type=pig] o 2",
        "execute as @e[type=pig] store result score @s k run data get entity @s UUID[3]",
        "scoreboard players operation @e[type=pig] o += @e[type=pig,limit=3,sort=arbitrary] k",
        "scoreboard players set #fake o 7",
        "scoreboard players operation #fake o >< @e[type=pig,limit=1,sort=arbitrary] o",
        "team join u @e[type=pig,limit=5,sort=arbitrary]",
        "execute as @e[type=pig,team=u] run scoreboard players add @s o 100",
        "scoreboard players list @e[type=pig,limit=1,sort=arbitrary]",
        "scoreboard players list",
        "scoreboard players get @e[type=pig,team=u,limit=1,sort=arbitrary] o",
        "team list t",
        "team leave @e[type=pig,limit=2,sort=arbitrary]",
        // Pigs left with a team and no score: their removal must still empty their team slot.
        "scoreboard players reset @e[type=pig,team=u]",
    ] {
        run_command(&mut p, c, &mut trace);
    }
    team_of_every_pig(&p, &mut trace);
    // Scores the server writes itself (criteria), copying the holder's display name: the player
    // of a blue team kills two pigs of the red team `u`; each victim scores `killedByTeam.blue`
    // under the name of a mob already out of the list, the player scores `teamkill.red`.
    for c in [
        "team add b",
        "team modify b color blue",
        "team join b @p",
        "scoreboard objectives add kbt killedByTeam.blue",
        "scoreboard objectives modify kbt displayautoupdate true",
        "scoreboard objectives setdisplay below_name kbt",
        "scoreboard objectives add tk teamkill.red",
        "scoreboard objectives modify tk displayautoupdate true",
        "scoreboard objectives setdisplay sidebar.team.blue tk",
        "execute as @e[type=pig,team=u,limit=2,sort=nearest] run damage @s 100 minecraft:player_attack by @p",
    ] {
        run_command(&mut p, c, &mut trace);
    }
    ticks(&mut p, 30, &mut trace);
    run_command(&mut p, "kill @e[type=pig,limit=10,sort=arbitrary]", &mut trace);
    ticks(&mut p, 30, &mut trace);
    team_of_every_pig(&p, &mut trace);
    board_state(&p, &mut trace);
    // One fake player's score, 1,000 entities removed: only the marker that holds a score loses it.
    // `#fake` written again after the reset: a forgotten holder comes back as a new one.
    for c in [
        "scoreboard players reset *",
        "team empty t",
        "team empty u",
        "scoreboard players set #fake o 2",
        "scoreboard players list",
        "scoreboard players reset #fake",
        "scoreboard players set #only o 1",
    ] {
        run_command(&mut p, c, &mut trace);
    }
    for i in 0..MARKERS {
        let ([a, b, c, d], _) = fixed_uuid(2, i);
        let summon = format!("summon minecraft:marker {} -59 {} {{UUID:[I;{a},{b},{c},{d}]}}", i % 40, i / 40);
        let mut src = p.s.player_source();
        src.level = 4;
        assert!(p.s.perform(src, &summon).0, "{summon}");
    }
    let _ = p.take_events();
    let (_, scored) = fixed_uuid(2, 17);
    run_command(&mut p, &format!("scoreboard players set {} o 5", uuid_text(scored)), &mut trace);
    run_command(&mut p, "kill @e[type=marker]", &mut trace);
    board_state(&p, &mut trace);
    crate::tests_g3f13::finish(p, dir);
    FORCED_PATHS.with(|f| f.set(None));
    trace
}

#[test]
fn optimised_paths_play_the_script_like_the_former_ones() {
    let off = BoardPaths { holder_map: false, one_lookup: false, member_index: false, uuid_set: false };
    let on = BoardPaths { holder_map: true, one_lookup: true, member_index: true, uuid_set: true };
    let mut variants = vec![off, on];
    variants.extend([
        BoardPaths { holder_map: true, ..off },
        BoardPaths { one_lookup: true, ..off },
        BoardPaths { member_index: true, ..off },
        BoardPaths { uuid_set: true, ..off },
    ]);
    let mut reference: Option<String> = None;
    for paths in variants {
        let trace = std::thread::spawn(move || script(paths)).join().expect("script thread");
        let Some(expected) = &reference else {
            // The script did what it claims on the former paths.
            assert!(trace.matches("Team { name: \"t\", mode: 3").count() >= PIGS as usize, "pigs did not join t");
            assert!(trace.contains("chat"), "no feedback recorded");
            assert!(trace.contains("pig 0 team=Some(\"t\") push=Some((0, 3))"), "team of the pigs not recorded");
            let named = |obj: &str| {
                trace
                    .lines()
                    .filter(|l| l.contains(&format!("objective: \"{obj}\"")) && l.contains("display: Some("))
                    .count()
            };
            let around: Vec<&str> =
                trace.lines().filter(|l| l.contains("damage") || l.contains("kbt") || l.contains("\"tk\"")).collect();
            assert_eq!(named("kbt"), 2, "the killed pigs' killedByTeam scores lack their display name: {around:#?}");
            assert!(named("tk") >= 1, "the player's teamkill score lacks its display name");
            let last_save = trace.lines().rev().find(|l| l.starts_with("holders")).expect("final holders");
            assert_eq!(last_save, "holders [\"#only\"]", "the marker's score survived, or #only went");
            reference = Some(trace);
            continue;
        };
        if trace != *expected {
            let first = expected.lines().zip(trace.lines()).enumerate().find(|(_, (a, b))| a != b);
            panic!(
                "{paths:?} differs from the former paths ({} lines against {}), first different line: {first:?}",
                trace.lines().count(),
                expected.lines().count()
            );
        }
    }
}
