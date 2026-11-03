//! Teams of entities found by UUID, never by formatting it (fresh flat world, 26.3 version data
//! required): the player's sweep that spares its allies (mobs, armor stands), the `team=`
//! selector option and the `team` entity predicate. Each must give the same result as before and
//! format no UUID for the team lookup.

use super::scores::{BoardPaths, FORCED_PATHS};
use crate::blocks::probe::Probe;
use crate::nbtio::UUID_TEXTS_FORMATTED;

const PIGS: usize = 8;
const STANDS: usize = 8;

fn formatted() -> u64 {
    UUID_TEXTS_FORMATTED.with(std::cell::Cell::get)
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    assert!(p.s.perform(src, c).0, "command refused: {c}");
}

/// Player on team `t` with pigs and armor stands right beside it, all of them on `t`.
fn world_of_allies(tag: &str) -> (Probe, std::path::PathBuf) {
    let on = BoardPaths { holder_map: true, one_lookup: true, member_index: true, uuid_set: true };
    FORCED_PATHS.with(|f| f.set(Some(on)));
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    FORCED_PATHS.with(|f| f.set(None));
    for c in ["team add t", "team add u", "team join t @p"] {
        cmd(&mut p, c);
    }
    for _ in 0..PIGS {
        cmd(&mut p, "summon minecraft:pig 1.0 -60 0.5 {NoAI:1b,Silent:1b}");
    }
    for _ in 0..STANDS {
        cmd(&mut p, "summon minecraft:armor_stand 1.2 -60 0.5");
    }
    cmd(&mut p, "team join t @e[type=!player]");
    let _ = p.take_events();
    (p, dir)
}

fn pig_healths(p: &Probe) -> Vec<f32> {
    p.s.mobs.list.iter().map(|m| m.health).collect()
}

/// The sweep of a player on a team spares every ally, and finds their teams without formatting
/// a single UUID; a pig that left the team is hit.
#[test]
fn sweep_spares_allies_without_formatting_uuids() {
    let (mut p, dir) = world_of_allies("team-uuid-sweep");
    let target = rc_physics::Aabb { min: [0.55, -60.0, 0.05], max: [1.45, -59.1, 0.95] };
    assert_eq!(pig_healths(&p).len(), PIGS, "test setup");
    let before = formatted();
    p.s.sweep_attack(&target, None, 1.0, 1.0, "minecraft:player_attack", None);
    assert_eq!(formatted() - before, 0, "the sweep formatted UUIDs to find the allies' teams");
    assert!(pig_healths(&p).iter().all(|h| *h == 10.0), "an allied pig was hit: {:?}", pig_healths(&p));
    let first = p.s.mobs.list.iter().next().expect("pig").uuid;
    cmd(&mut p, &format!("team leave {}", crate::nbtio::uuid_text(first)));
    p.s.sweep_attack(&target, None, 1.0, 1.0, "minecraft:player_attack", None);
    let hurt = p.s.mobs.list.iter().filter(|m| m.health < 10.0).map(|m| m.uuid).collect::<Vec<_>>();
    assert_eq!(hurt, vec![first], "only the pig that left the team is swept");
    crate::tests_g3f13::finish(p, dir);
}

/// `@e[team=…]` and `@e[predicate=…]` with a `team` condition select like before, and the team
/// test formats no UUID (same count as a plain `@e`).
#[test]
fn team_filters_select_without_formatting_uuids() {
    let (mut p, dir) = world_of_allies("team-uuid-select");
    let in_t = r#"{"condition":"minecraft:entity_properties","entity":"this","predicate":{"team":"t"}}"#;
    p.s.life.pack.insert_test_file("test:predicate/in_t", in_t);
    cmd(&mut p, "team join u @e[type=pig,limit=2,sort=nearest]");
    let src = p.s.player_source();
    let mut select = |text: &str| {
        let before = formatted();
        let found = p.s.select(&super::select::parse(text).expect("selector"), &src).len();
        (found, formatted() - before)
    };
    let (all, plain) = select("@e");
    assert_eq!(all, 1 + PIGS + STANDS, "test setup");
    for (text, expected) in [
        ("@e[team=t]", all - 2),
        ("@e[team=u]", 2),
        ("@e[team=!t]", 2),
        ("@e[team=]", 0),
        ("@e[predicate=test:in_t]", all - 2),
        ("@e[predicate=!test:in_t]", 2),
    ] {
        let (found, formats) = select(text);
        assert_eq!(found, expected, "{text}");
        assert_eq!(formats, plain, "{text}: the team test formatted UUIDs");
    }
    crate::tests_g3f13::finish(p, dir);
}
