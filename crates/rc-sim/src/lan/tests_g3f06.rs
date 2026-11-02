//! G3-F06: relay of command successes in an open world. The host is the only operator of the
//! integrated server (the guests get a permission level, never a place on the operator list):
//! a guest's relayed success reaches the host as `[Guest: …]`; the host's own commands reach
//! nobody. Version data required (absent: explicit failure).

use rc_client::Event;
use rc_text::{Component, Content};

use super::tests_g3f05::{drain, join, open, publish};

fn admin_lines(events: &[Event]) -> Vec<String> {
    fn is_admin(c: &Component) -> bool {
        matches!(&c.content, Content::Translate { key, .. } if key == "chat.type.admin")
    }
    events
        .iter()
        .filter_map(|e| match e {
            Event::Chat { component, .. } if is_admin(component) => Some(crate::commands::cmds_ents::plain(component)),
            _ => None,
        })
        .collect()
}

#[test]
fn guest_successes_reach_the_host_only() {
    let mut t = open("admin");
    t.s.level.allow_commands = true;
    t.s.cmd.op = 4;
    publish(&mut t, true);
    let g = join(&mut t, "Guest", 2);
    let _ = drain(&t);
    let _: Vec<Vec<u8>> = g.from.try_iter().collect();
    let run = |t: &mut super::tests_g3f05::T, c: &str| {
        t.s.as_guest(0, |sv| {
            let src = sv.player_source();
            assert_eq!(src.level, 2, "guest at the game master level");
            sv.perform(src, c).0
        })
    };
    assert!(run(&mut t, "time set 1000"));
    assert_eq!(admin_lines(&drain(&t)), vec!["[Guest: Set minecraft:overworld to 1000 tick(s)]".to_owned()]);
    assert!(run(&mut t, "difficulty"));
    assert!(admin_lines(&drain(&t)).is_empty(), "a query is kept by its source");
    // The host's command: the guest is no operator, nobody receives it.
    let src = t.s.player_source();
    assert!(t.s.perform(src, "time set 2000").0);
    assert!(admin_lines(&drain(&t)).is_empty());
    let packets: Vec<Vec<u8>> = g.from.try_iter().collect();
    let needle = b"chat.type.admin";
    assert!(!packets.iter().any(|p| p.windows(needle.len()).any(|w| w == needle)), "no relay to the guest");
}

fn has(packets: &[Vec<u8>], needle: &[u8]) -> bool {
    packets.iter().any(|p| p.windows(needle.len()).any(|w| w == needle))
}

/// A guest who started a test run leaves before its summary: seats move, but the reports never
/// reach the guest now in their seat; the summary is lost for the one who left, and its relay
/// still reaches the host (the game's player object stays the source).
#[test]
fn test_run_reports_follow_their_player_after_a_departure() {
    let mut t = open("gtleave");
    t.s.level.allow_commands = true;
    t.s.cmd.op = 4;
    publish(&mut t, true);
    let a = join(&mut t, "Alice", 2);
    let b = join(&mut t, "Bob", 2);
    let _ = drain(&t);
    let _: Vec<Vec<u8>> = a.from.try_iter().chain(b.from.try_iter()).collect();
    assert!(t.s.as_guest(0, |sv| {
        let src = sv.player_source();
        sv.perform(src, "test run always_pass").0
    }));
    let _: Vec<Vec<u8>> = b.from.try_iter().collect();
    let _ = drain(&t);
    t.s.lan_leave(0);
    let _ = drain(&t);
    let _: Vec<Vec<u8>> = b.from.try_iter().collect();
    for _ in 0..8 {
        t.s.game_tick();
        t.s.tick += 1;
    }
    assert!(t.s.cmd.tests.runner.as_ref().is_none_or(|r| r.ticked_len() == 0), "the test finished");
    let to_bob: Vec<Vec<u8>> = b.from.try_iter().collect();
    assert!(!has(&to_bob, b"commands.test.summary"), "Bob never gets Alice's reports");
    let host = admin_lines(&drain(&t));
    assert!(host.iter().any(|l| l.starts_with("[Alice: ") && l.contains("1")), "relay to the host: {host:?}");
}
