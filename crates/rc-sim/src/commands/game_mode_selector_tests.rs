//! The mode selector (F3+F4, F3+N) asks the server by packet; the server answers like
//! `/gamemode`: the mode changes and the player reads "Set own game mode to …". A player below
//! the command's level changes nothing.

use std::sync::Arc;

use rc_client::Event;
use rc_proto::varint::write_varint;

use crate::blocks::probe::Probe;

fn open(tag: &str) -> (Probe, std::path::PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-mode-selector-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("mode-selector");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.loaded = true;
    p.s.game_mode = 0;
    (p, dir)
}

/// The mode-change packet asking for `mode`.
fn mode_packet(p: &Probe, mode: i32) -> Vec<u8> {
    let mut pkt = Vec::new();
    write_varint(&mut pkt, p.s.peer.ids().change_game_mode);
    write_varint(&mut pkt, mode);
    pkt
}

/// Chat lines sent to the player whose text is the translation `key`.
fn lines_with_key(events: &[Event], key: &str) -> usize {
    let has_key = |e: &Event| match e {
        Event::Chat { component, .. } => {
            matches!(&component.content, rc_text::Content::Translate { key: k, .. } if k == key)
        }
        _ => false,
    };
    events.iter().filter(|e| has_key(e)).count()
}

#[test]
fn mode_selector_announces_the_change_like_the_command() {
    let (mut p, _dir) = open("announce");
    p.s.cmd.op = 2;
    p.take_events();
    let pkt = mode_packet(&p, 1);
    assert!(p.s.life_packet(&pkt), "packet not handled");
    assert_eq!(p.s.game_mode, 1, "mode not changed");
    let events = p.take_events();
    assert_eq!(lines_with_key(&events, "commands.gamemode.success.self"), 1, "confirmation line");
    // The same mode again: no change, no line.
    p.s.life_packet(&pkt);
    assert_eq!(lines_with_key(&p.take_events(), "commands.gamemode.success.self"), 0, "same mode announced");
}

#[test]
fn mode_selector_refused_below_the_command_level() {
    let (mut p, _dir) = open("refused");
    p.s.cmd.op = 1;
    p.take_events();
    let pkt = mode_packet(&p, 3);
    p.s.life_packet(&pkt);
    assert_eq!(p.s.game_mode, 0, "mode changed without the level");
    assert_eq!(lines_with_key(&p.take_events(), "commands.gamemode.success.self"), 0);
}
