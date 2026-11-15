//! Recount of a lidded container's openers: five ticks after its first opener, then every five
//! ticks while someone remains, a chest, barrel or Ender chest counts its real users again. An
//! opener that left without closing (a golem pushed away mid-exchange, a player turned spectator)
//! no longer holds the lid open. Before, the count was only moved by arrivals and departures.

use rc_client::Event;

use super::P;
use super::test_area::{Area, open};
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;

fn openers(a: &Area, at: P) -> i32 {
    a.p.s.life.openers.get(&at).copied().unwrap_or(0)
}

/// Lid count signals (block event 1) sent for `at` among `evs`.
fn lid_signals(evs: &[Event], at: P) -> Vec<u8> {
    evs.iter()
        .filter_map(|e| match e {
            Event::BlockEvent { pos, b0: 1, b1, .. } if *pos == at => Some(*b1),
            _ => None,
        })
        .collect()
}

/// A copper golem standing by `at`, marked as holding it open (what its exchange does on its
/// first tick).
fn golem_opening(a: &mut Area, at: P) -> i32 {
    let pos = [f64::from(at[0]) + 1.5, f64::from(at[1]), f64::from(at[2]) + 0.5];
    let mut m = a.p.s.new_mob(Kind::CopperGolem, pos, 0.0);
    m.persistent = true;
    m.golem.opened_chest = Some(at);
    let id = m.id;
    a.p.s.add_mob(m);
    a.p.s.container_openers(at, Ent::Mob(id), 1);
    id
}

/// A golem interrupted mid-exchange forgets its chest without closing it: the recount closes the
/// lid (count 0 signalled) instead of leaving it open for good.
#[test]
fn golem_gone_mid_exchange_lets_the_lid_close() {
    let Some(mut a) = open("openers-golem") else { return };
    let chest: P = [110, -59, 110];
    a.set(chest, "minecraft:chest[facing=north]");
    let id = golem_opening(&mut a, chest);
    let evs = a.ticks(5);
    assert_eq!(openers(&a, chest), 1, "the golem still holds the chest");
    // The opening's signal, then the recount's.
    assert_eq!(lid_signals(&evs, chest), vec![1, 1], "the recount signals the count again");
    a.p.s.mobs.get_mut(id).expect("golem").golem.opened_chest = None;
    let evs = a.ticks(5);
    assert_eq!(openers(&a, chest), 0, "lid left open after the golem left");
    assert_eq!(lid_signals(&evs, chest), vec![0], "lid not closed by the recount");
    // Nobody left: no further recount is booked.
    assert!(!a.p.s.has_scheduled_tick(chest, a.p.s.view().block(a.p.s.get(chest))));
}

/// The golem's opened chest is one half of a double chest: both halves count it.
#[test]
fn golem_counts_on_both_halves_of_a_double_chest() {
    let Some(mut a) = open("openers-double") else { return };
    let (left, right): (P, P) = ([110, -59, 110], [111, -59, 110]);
    a.set(left, "minecraft:chest[facing=north,type=left]");
    a.set(right, "minecraft:chest[facing=north,type=right]");
    let id = golem_opening(&mut a, left);
    let _ = a.ticks(5);
    assert_eq!((openers(&a, left), openers(&a, right)), (1, 1), "golem not counted on each half");
    a.p.s.mobs.get_mut(id).expect("golem").golem.opened_chest = None;
    let _ = a.ticks(5);
    assert_eq!((openers(&a, left), openers(&a, right)), (0, 0));
}

/// A player who opened a barrel then turned spectator no longer holds it: the barrel closes.
/// While the player holds it in survival, the recount keeps it open.
#[test]
fn spectator_player_lets_the_barrel_close() {
    let Some(mut a) = open("openers-spectator") else { return };
    let barrel: P = [110, -59, 130];
    a.set(barrel, "minecraft:barrel[facing=up]");
    a.p.s.game_mode = 0;
    assert!(a.p.s.use_block(barrel), "barrel not opened");
    let _ = a.ticks(11);
    assert_eq!(openers(&a, barrel), 1, "the recount lost the player holding the barrel");
    assert!(a.name(barrel).contains("open=true"));
    a.p.s.game_mode = 3;
    let _ = a.ticks(5);
    assert_eq!(openers(&a, barrel), 0, "barrel kept open by a spectator");
    assert!(a.name(barrel).contains("open=false"), "{}", a.name(barrel));
}

/// The game's quirk (docs/quirks-to-reproduce.md): the count goes below 0. Opened in survival,
/// recounted to 0 while the player is a spectator, then closed back in survival: the barrel
/// counts −1. The next opening only brings it to 0: the lid stays shut, with no sound and no
/// recount booked, while the menu is open.
#[test]
fn opener_count_goes_below_zero_like_the_game() {
    let Some(mut a) = open("openers-below-zero") else { return };
    let barrel: P = [110, -59, 130];
    a.set(barrel, "minecraft:barrel[facing=up]");
    a.p.s.game_mode = 0;
    assert!(a.p.s.use_block(barrel), "barrel not opened");
    a.p.s.game_mode = 3;
    let _ = a.ticks(5);
    assert_eq!(openers(&a, barrel), 0, "recount with the player a spectator");
    a.p.s.game_mode = 0;
    a.p.s.close_menu(false);
    assert_eq!(openers(&a, barrel), -1, "the departure the recount already took off");
    let _ = a.p.take_events();
    assert!(a.p.s.use_block(barrel), "barrel not opened again");
    assert_eq!(openers(&a, barrel), 0, "arrival from -1");
    assert!(a.name(barrel).contains("open=false"), "lid opened from -1: {}", a.name(barrel));
    let sounds = a.p.take_events().into_iter().filter(|e| matches!(e, Event::Sound(_))).count();
    assert_eq!(sounds, 0, "opening sound played from -1");
    assert!(!a.p.s.has_scheduled_tick(barrel, a.p.s.view().block(a.p.s.get(barrel))), "recount booked from -1");
}

/// A player who turned spectator before closing takes nothing off the count (the game skips a
/// spectator's departure): the lid stays up until the recount closes it.
#[test]
fn spectator_closing_leaves_the_count_to_the_recount() {
    let Some(mut a) = open("openers-spectator-close") else { return };
    let barrel: P = [110, -59, 130];
    a.set(barrel, "minecraft:barrel[facing=up]");
    a.p.s.game_mode = 0;
    assert!(a.p.s.use_block(barrel), "barrel not opened");
    a.p.s.game_mode = 3;
    a.p.s.close_menu(false);
    assert_eq!(openers(&a, barrel), 1, "a spectator's departure counted");
    assert!(a.name(barrel).contains("open=true"), "{}", a.name(barrel));
    let _ = a.ticks(5);
    assert_eq!(openers(&a, barrel), 0, "the recount left the lid up");
    assert!(a.name(barrel).contains("open=false"), "{}", a.name(barrel));
}
