//! Scoreboard end to end (test world `RUSTCRAFT_SOLO_WORLD`): read-only criteria,
//! `/trigger` without `enable`, removed entities, `killedByTeam`, holder name.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rc_text::HoverEvent;

use crate::server::Server;

fn open(tag: &str) -> Option<(Server, rc_client::Session)> {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data missing: test skipped");
        return None;
    };
    let copy = crate::tests::fresh_copy(&world, tag);
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("Sc");
    let (session, peer) = rc_client::memory::session(Arc::new(data.clone()), uuid).expect("session");
    let opts = crate::Options {
        world: copy,
        name: "Sc".into(),
        uuid,
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 12,
    };
    let mut s = match Server::open(opts, uuid, Arc::new(data), peer, Arc::new(AtomicBool::new(false))) {
        Ok(s) => s,
        Err((_, e)) => panic!("opening: {e}"),
    };
    s.loaded = true;
    Some((s, session))
}

fn run(s: &mut Server, c: &str) -> bool {
    let mut src = s.player_source();
    src.level = 4;
    s.perform(src, c).0
}

#[test]
fn read_only_criteria_follow_the_player() {
    let Some((mut s, _session)) = open("sc-crit") else { return };
    for c in ["h health", "f food", "a air", "x xp", "l level", "r armor"] {
        assert!(run(&mut s, &format!("scoreboard objectives add {c}")));
    }
    s.life.vit.health = 13.5;
    s.life.vit.food = 17;
    s.life.vit.xp_level = 3;
    s.vitals_tick();
    // Health (+ absorption) rounded up; before: never written.
    assert_eq!(s.cmd.scores.get("Sc", "h"), Some(14));
    assert_eq!(s.cmd.scores.get("Sc", "f"), Some(17));
    assert_eq!(s.cmd.scores.get("Sc", "l"), Some(3));
    assert_eq!(s.cmd.scores.get("Sc", "a"), Some(s.life.vit.air));
    assert_eq!(s.cmd.scores.get("Sc", "r"), Some(0));
    // Objective created afterwards: empty as long as the value does not change (like the game).
    assert!(run(&mut s, "scoreboard objectives add f2 food"));
    s.vitals_tick();
    assert_eq!(s.cmd.scores.get("Sc", "f2"), None);
    s.life.vit.food = 16;
    s.vitals_tick();
    assert_eq!(s.cmd.scores.get("Sc", "f2"), Some(16));
    // Read-only: the command still refuses to write.
    assert!(!run(&mut s, "scoreboard players set Sc h 3"));
}

#[test]
fn trigger_needs_enable_after_a_plain_set() {
    let Some((mut s, _session)) = open("sc-trig") else { return };
    assert!(run(&mut s, "scoreboard objectives add t trigger"));
    // Score created by `players set`: locked (before: /trigger accepted).
    assert!(run(&mut s, "scoreboard players set Sc t 0"));
    assert!(!run(&mut s, "trigger t"));
    assert!(run(&mut s, "scoreboard players enable Sc t"));
    assert!(run(&mut s, "trigger t"));
    assert_eq!(s.cmd.scores.get("Sc", "t"), Some(1));
}

#[test]
fn removed_entity_loses_scores_and_team_and_killed_by_team_counts() {
    let Some((mut s, _session)) = open("sc-ent") else { return };
    let p = s.pos;
    assert!(run(&mut s, &format!("summon minecraft:pig {} {} {}", p[0], p[1], p[2])));
    assert!(run(&mut s, "scoreboard objectives add o dummy"));
    assert!(run(&mut s, "scoreboard objectives add k killedByTeam.red"));
    assert!(run(&mut s, "team add t"));
    assert!(run(&mut s, "team add me"));
    assert!(run(&mut s, "team modify me color red"));
    assert!(run(&mut s, "team join me Sc"));
    assert!(run(&mut s, "scoreboard players set @e[type=pig] o 5"));
    assert!(run(&mut s, "team join t @e[type=pig]"));
    let m = s.mobs.list.iter().flatten().find(|m| m.kind == crate::mobs::species::Kind::Pig).expect("pig");
    let (uuid, id) = (crate::nbtio::uuid_text(m.uuid), m.id);
    assert_eq!(s.cmd.scores.get(&uuid, "o"), Some(5));
    // Killed by the red team's player: the victim scores killedByTeam.red.
    let m = s.mobs.list.iter().flatten().find(|m| m.id == id).unwrap().clone();
    s.s9_mob_died(&m, "minecraft:player_attack", None);
    assert_eq!(s.cmd.scores.get(&uuid, "k"), None, "player credit required");
    if let Some(m) = s.mobs.list.iter_mut().flatten().find(|m| m.id == id) {
        m.last_hurt_by_player = 100;
    }
    let m = s.mobs.list.iter().flatten().find(|m| m.id == id).unwrap().clone();
    s.s9_mob_died(&m, "minecraft:player_attack", None);
    assert_eq!(s.cmd.scores.get(&uuid, "k"), Some(1));
    // Permanent removal: scores and team erased (before: kept).
    if let Some(m) = s.mobs.list.iter_mut().flatten().find(|m| m.id == id) {
        m.removed = true;
    }
    s.mobs_tick();
    assert!(s.mobs.list.iter().flatten().all(|m| m.id != id));
    assert_eq!(s.cmd.scores.get(&uuid, "o"), None);
    assert_eq!(s.cmd.scores.get(&uuid, "k"), None);
    assert!(s.cmd.scores.team_of(&uuid).is_none());
    assert!(s.cmd.scores.team_of("Sc").is_some(), "the player keeps their team");
}

#[test]
fn auto_update_and_feedback_names() {
    let Some((mut s, _session)) = open("sc-name") else { return };
    assert!(run(&mut s, "scoreboard objectives add o dummy"));
    assert!(run(&mut s, "scoreboard objectives modify o displayautoupdate true"));
    assert!(run(&mut s, "scoreboard players set Sc o 2"));
    // Automatic update of the displayed name: displayed name of the player copied into the score (before: never).
    assert!(s.cmd.scores.score("Sc", "o").and_then(|x| x.display.clone()).is_some());
    // Return name: hover = scoreboard name (before: entity hover).
    let n = s.holder_name("Sc");
    assert!(matches!(n.style.hover.as_deref(), Some(HoverEvent::Text(_))), "{n:?}");
    // Simple name: raw text, no displayed name copied.
    assert!(run(&mut s, "scoreboard players set #f o 1"));
    assert!(s.cmd.scores.score("#f", "o").unwrap().display.is_none());
    assert!(s.holder_name("#f").style.hover.is_none());
}

/// Events received by the client, summarized: scoreboard packets and chat lines.
fn events(session: &rc_client::Session) -> Vec<String> {
    use rc_proto_v777::scores::ScoreEvent;
    let mut out = Vec::new();
    while let Ok(e) = session.events.try_recv() {
        match e {
            rc_client::Event::Score(ScoreEvent::Team { name, mode, .. }) => out.push(format!("team {name} {mode}")),
            rc_client::Event::Score(ScoreEvent::Objective { name, mode, .. }) => out.push(format!("obj {name} {mode}")),
            rc_client::Event::Score(ScoreEvent::Display { slot, objective }) => {
                out.push(format!("disp {slot} {objective}"))
            }
            rc_client::Event::Score(ScoreEvent::Score { holder, objective, value, .. }) => {
                out.push(format!("score {holder} {objective} {value}"))
            }
            rc_client::Event::Score(_) => out.push("score?".into()),
            rc_client::Event::Chat { .. } => out.push("chat".into()),
            _ => {}
        }
    }
    out
}

/// Scoreboard packets broadcast during the operation, therefore BEFORE the return line (before: after).
#[test]
fn board_packets_precede_the_feedback_line() {
    let Some((mut s, session)) = open("sc-order") else { return };
    events(&session);
    assert!(run(&mut s, "team add t"));
    assert_eq!(events(&session), ["team t 0", "chat"]);
    assert!(run(&mut s, "scoreboard objectives add o dummy"));
    assert!(run(&mut s, "scoreboard players set A o 2"));
    events(&session);
    assert!(run(&mut s, "scoreboard objectives setdisplay sidebar o"));
    assert_eq!(events(&session), ["obj o 0", "disp 1 o", "score A o 2", "chat"]);
    assert!(run(&mut s, "scoreboard players set A o 3"));
    assert_eq!(events(&session), ["score A o 3", "chat"]);
}

/// `players operation`: target and sources created without a packet, only the writes send.
#[test]
fn operation_creates_scores_silently() {
    let Some((mut s, session)) = open("sc-op") else { return };
    assert!(run(&mut s, "scoreboard objectives add o dummy"));
    assert!(run(&mut s, "scoreboard objectives setdisplay sidebar o"));
    events(&session);
    // Division by zero on two new holders: failure, scores created, no packet
    // (before: "score A o 0" and "score B o 0" made A and B appear).
    assert!(!run(&mut s, "scoreboard players operation A o /= B o"));
    assert_eq!(events(&session), ["chat"]);
    assert_eq!((s.cmd.scores.get("A", "o"), s.cmd.scores.get("B", "o")), (Some(0), Some(0)));
    // Score already created: writing 0 changes nothing, so nothing is sent.
    assert!(run(&mut s, "scoreboard players set A o 0"));
    assert_eq!(events(&session), ["chat"]);
    // Source merely read: created (before: absent); new target sent even at 0.
    assert!(run(&mut s, "scoreboard players operation D o = C o"));
    assert_eq!(s.cmd.scores.get("C", "o"), Some(0));
    assert_eq!(events(&session), ["score D o 0", "chat"]);
    // Swap: the target first, then the source.
    assert!(run(&mut s, "scoreboard players set E o 5"));
    events(&session);
    assert!(run(&mut s, "scoreboard players operation D o >< E o"));
    assert_eq!(events(&session), ["score D o 5", "score E o 0", "chat"]);
}

/// Mob killed during its own tick (out of the list): `killedByTeam` with
/// `displayautoupdate true` still copies its displayed name (before: no name).
#[test]
fn killed_by_team_copies_the_name_of_a_mob_dying_in_its_tick() {
    let Some((mut s, _session)) = open("sc-kbt") else { return };
    let p = s.pos;
    assert!(run(&mut s, &format!("summon minecraft:pig {} {} {}", p[0], p[1], p[2])));
    assert!(run(&mut s, "scoreboard objectives add k killedByTeam.red"));
    assert!(run(&mut s, "scoreboard objectives modify k displayautoupdate true"));
    assert!(run(&mut s, "team add me"));
    assert!(run(&mut s, "team modify me color red"));
    assert!(run(&mut s, "team join me Sc"));
    let i = s.mobs.list.iter().position(|m| m.as_ref().is_some_and(|m| m.kind == crate::mobs::species::Kind::Pig));
    let i = i.expect("pig");
    let mut m = s.mobs.list[i].take().unwrap();
    m.last_hurt_by_player = 100;
    let uuid = crate::nbtio::uuid_text(m.uuid);
    s.mob_die(&mut m, "minecraft:on_fire", None);
    s.mobs.list[i] = Some(m);
    let sc = s.cmd.scores.score(&uuid, "k").expect("victim's score");
    assert_eq!(sc.value, 1);
    assert!(sc.display.is_some(), "victim's displayed name copied");
}

/// Micro-cost of tracking criteria without an objective (common case): `cargo test --release
/// criteria_tick_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn criteria_tick_cost() {
    let Some((mut s, _session)) = open("sc-cost") else { return };
    let n = 200_000;
    let t = std::time::Instant::now();
    for _ in 0..n {
        s.criteria_tick();
    }
    let tick = t.elapsed().as_nanos() as f64 / f64::from(n);
    let t = std::time::Instant::now();
    let mut x = 0.0;
    for _ in 0..n {
        x += std::hint::black_box(s.player_attribute("minecraft:armor_toughness"));
    }
    let tough = t.elapsed().as_nanos() as f64 / f64::from(n);
    eprintln!("criteria_tick {tick:.1} ns/tick; toughness avoided {tough:.1} ns/tick ({x})");
}
