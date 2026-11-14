//! A spectator's camera on an entity (`spectator_camera.rs`): the player rides along with the
//! watched mob and the camera comes back on sneak, death, a mode change or a command teleport,
//! like the game. Each test fails on the code before it, where the camera was only ever given
//! back by `/spectate` without a target and the player stayed where they clicked. Flat world of
//! `tests_g3f13`: player standing at (0.5, -60, 0.5).

use rc_client::Event;

use crate::blocks::probe::Probe;
use crate::commands::ents::E;
use crate::input::Input;
use crate::mobs::species::Kind;

/// A spectator and a still pig two blocks east, the camera already in the pig; returns its id.
fn spectating_a_pig(p: &mut Probe) -> i32 {
    p.s.game_mode = 3;
    let mut pig = p.s.new_mob(Kind::Pig, [2.5, -60.0, 0.5], 30.0);
    pig.no_ai = true;
    let id = pig.id;
    p.s.add_mob(pig);
    p.s.on_spectator_action(Some(id));
    assert_eq!(p.s.spectated_entity(), Some(E::Mob(id)), "camera not set on the pig");
    p.take_events();
    id
}

fn one_game_tick(p: &mut Probe) {
    p.s.game_tick();
    p.s.tick += 1;
}

fn camera_packets(events: &[Event]) -> Vec<i32> {
    events.iter().filter_map(|e| if let Event::SetCamera(id) = e { Some(*id) } else { None }).collect()
}

fn teleports(events: &[Event]) -> Vec<[f64; 3]> {
    events.iter().filter_map(|e| if let Event::Teleport(t) = e { Some(t.pos) } else { None }).collect()
}

/// Setting the camera puts the player on the watched entity (teleport, then the camera packet);
/// asking again for the same camera, or for the player's own eyes while already there, sends
/// nothing.
#[test]
fn setting_the_camera_moves_the_player_onto_the_entity_once() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-set");
    p.s.game_mode = 3;
    let mut pig = p.s.new_mob(Kind::Pig, [2.5, -60.0, 0.5], 0.0);
    pig.no_ai = true;
    let id = pig.id;
    p.s.add_mob(pig);
    p.take_events();
    p.s.set_camera(None);
    assert!(p.take_events().is_empty(), "own camera asked again: nothing to send");
    p.s.on_spectator_action(Some(id));
    let events = p.take_events();
    assert_eq!(teleports(&events), vec![[2.5, -60.0, 0.5]], "player not put on the pig");
    assert_eq!(camera_packets(&events), vec![id]);
    let teleport_at = events.iter().position(|e| matches!(e, Event::Teleport(_)));
    let camera_at = events.iter().position(|e| matches!(e, Event::SetCamera(_)));
    assert!(teleport_at < camera_at, "the game teleports before it sends the camera");
    p.s.on_spectator_action(Some(id));
    assert!(camera_packets(&p.take_events()).is_empty(), "same camera: nothing sent");
}

/// Each tick the player stands on the watched pig's position and rotation, so the tracked area
/// follows it when it goes away.
#[test]
fn the_player_follows_the_watched_mob() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-follow");
    let pig = spectating_a_pig(&mut p);
    let far = [100.5, -60.0, 8.5];
    let m = p.s.mobs.get_mut(pig).expect("pig");
    m.set_pos(far);
    m.yaw = 75.0;
    m.pitch = 12.0;
    one_game_tick(&mut p);
    assert_eq!(p.s.pos, far, "player left behind");
    assert_eq!([p.s.yaw, p.s.pitch], [75.0, 12.0], "player does not look like the pig");
    p.s.update_tracking();
    assert_eq!(p.s.center, Some((6, 0)), "chunks not tracked around the pig");
    assert_eq!(p.s.spectated_entity(), Some(E::Mob(pig)));
}

/// The sneak key held: the camera comes back, the player standing where the pig is.
#[test]
fn sneaking_gives_the_camera_back() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-sneak");
    spectating_a_pig(&mut p);
    p.s.handle(Input::Keys(32));
    one_game_tick(&mut p);
    let events = p.take_events();
    assert_eq!(camera_packets(&events), vec![p.s.player_id]);
    assert_eq!(teleports(&events), vec![[2.5, -60.0, 0.5]], "player not left on the pig's spot");
    assert_eq!(p.s.spectated_entity(), None);
}

/// The watched pig dies: the camera comes back on the next tick.
#[test]
fn the_watched_mob_dying_gives_the_camera_back() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-death");
    let pig = spectating_a_pig(&mut p);
    p.s.mobs.get_mut(pig).expect("pig").health = 0.0;
    one_game_tick(&mut p);
    assert_eq!(camera_packets(&p.take_events()), vec![p.s.player_id]);
    assert_eq!(p.s.spectated_entity(), None);
}

/// Leaving the spectator mode gives the camera back after the mode announcement.
#[test]
fn leaving_spectator_mode_gives_the_camera_back() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-mode");
    spectating_a_pig(&mut p);
    p.s.set_game_mode(1);
    let events = p.take_events();
    assert_eq!(camera_packets(&events), vec![p.s.player_id]);
    let mode_at = events.iter().position(|e| matches!(e, Event::GameEvent { kind: 3, .. }));
    let camera_at = events.iter().position(|e| matches!(e, Event::SetCamera(_)));
    assert!(mode_at < camera_at, "the game announces the mode first");
    assert_eq!(p.s.spectated_entity(), None);
}

/// `/tp` of the spectator gives the camera back before the teleport itself.
#[test]
fn teleport_command_gives_the_camera_back() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-tp");
    spectating_a_pig(&mut p);
    let mut src = p.s.player_source();
    src.level = 4;
    assert!(p.s.perform(src, "tp @s 10 -60 10").0, "teleport refused");
    let events = p.take_events();
    assert_eq!(camera_packets(&events), vec![p.s.player_id]);
    let camera_at = events.iter().position(|e| matches!(e, Event::SetCamera(_)));
    let last_teleport = events.iter().rposition(|e| matches!(e, Event::Teleport(_)));
    assert!(camera_at < last_teleport, "camera given back after the teleport");
    assert_eq!(p.s.pos, [10.5, -60.0, 10.5], "whole coordinates are block centres");
    assert_eq!(p.s.spectated_entity(), None);
}

/// `/spectate` without a target while the player already looks through their own eyes sends
/// nothing (the game only tells a change).
#[test]
fn spectate_command_without_change_sends_no_camera() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-spectate");
    p.s.game_mode = 3;
    let mut src = p.s.player_source();
    src.level = 4;
    p.take_events();
    assert!(p.s.perform(src, "spectate").0, "spectate refused");
    assert!(camera_packets(&p.take_events()).is_empty(), "camera sent without a change");
}

/// Moves the watched pig far away, so that standing on it cannot pass for the respawn point.
fn pig_far_away(p: &mut Probe, pig: i32) -> [f64; 3] {
    let far = [100.5, -60.0, 8.5];
    p.s.mobs.get_mut(pig).expect("pig").set_pos(far);
    far
}

/// The player stands where the respawn put them, the tick after, and looks through their own eyes.
fn stays_on_the_respawn_point(p: &mut Probe, pig_at: [f64; 3]) {
    assert_eq!(p.s.spectated_entity(), None, "the new player still looks through the pig");
    let respawned_at = p.s.pos;
    assert_ne!(respawned_at, pig_at);
    (p.s.loaded, p.s.teleport) = (true, None); // the client's answers
    one_game_tick(p);
    assert_eq!(p.s.pos, respawned_at, "the respawned player moved onto the old camera");
}

/// A spectator watching a pig dies and respawns: the new player looks through their own eyes, as
/// the game makes a new player. Fails without the reset: the next tick put the player on the pig.
#[test]
fn respawning_gives_the_camera_back() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-respawn");
    let pig = spectating_a_pig(&mut p);
    let pig_at = pig_far_away(&mut p, pig);
    let mut src = p.s.player_source();
    src.level = 4;
    assert!(p.s.perform(src, "kill @s").0, "kill refused");
    assert!(p.s.life.vit.dead, "the spectator did not die");
    p.s.perform_respawn();
    stays_on_the_respawn_point(&mut p, pig_at);
}

/// Same after the end credits, where the game also makes a new player.
#[test]
fn respawning_after_the_credits_gives_the_camera_back() {
    let (mut p, _dir) = crate::tests_g3f13::open("camera-credits");
    let pig = spectating_a_pig(&mut p);
    let pig_at = pig_far_away(&mut p, pig);
    p.s.dims.won_game = true;
    p.s.perform_respawn();
    stays_on_the_respawn_point(&mut p, pig_at);
}
