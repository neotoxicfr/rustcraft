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
