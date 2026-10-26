//! Unit tests of family F15 (no world).

use super::dragon::{Dragon, History};
use super::phases::Phase;
use super::shulker::{physical_peek, progress_box, snap};

#[test]
fn phases_ids_and_sitting() {
    for id in 0..11 {
        assert_eq!(Phase::from_id(id) as i32, id);
    }
    assert_eq!(Phase::from_id(42), Phase::HoldingPattern);
    assert!(Phase::Hover.sitting() && !Phase::Hover.sitting_class());
    assert!(Phase::SittingScanning.sitting_class());
    assert!(!Phase::Dying.sitting());
    assert_eq!(Phase::ChargingPlayer.fly_speed(), 3.0);
    assert_eq!(Phase::Landing.fly_speed(), 1.5);
}

#[test]
fn phase_states_survive_switches() {
    let mut d = Dragon::default();
    d.set_phase_initial();
    assert_eq!(d.phase, Phase::Hover);
    d.set_phase(Phase::SittingFlaming);
    assert_eq!(d.ph.flame_count, 1);
    d.ph.flame = Some(77);
    d.set_phase(Phase::SittingScanning);
    // End of the breath: cloud to remove.
    assert_eq!(d.discard_clouds, vec![77]);
    d.set_phase(Phase::SittingFlaming);
    assert_eq!(d.ph.flame_count, 2);
    // Same phase: nothing.
    d.set_phase(Phase::SittingFlaming);
    assert_eq!(d.ph.flame_count, 2);
}

#[test]
fn history_ring() {
    let mut h = History::default();
    h.record(10.0, 5.0);
    assert_eq!(h.get(0), (10.0, 5.0));
    assert_eq!(h.get(63), (10.0, 5.0));
    h.record(11.0, 6.0);
    assert_eq!(h.get(0), (11.0, 6.0));
    assert_eq!(h.get(1), (10.0, 5.0));
}

#[test]
fn turn_speed_landing() {
    let mut d = Dragon::default();
    d.set_phase(Phase::HoldingPattern);
    let v = [0.3, 0.0, 0.4];
    let normal = d.turn_speed(v);
    d.set_phase(Phase::Landing);
    let land = d.turn_speed(v);
    assert!((normal - 0.7 / 1.5 / 1.5).abs() < 1e-6);
    assert!((land - 1.0).abs() < 1e-6);
}

#[test]
fn shulker_boxes() {
    // Closed: unit block; open upward: box stretched by 1 above.
    let p = [0.5, 64.0, 0.5];
    let closed = progress_box(1, physical_peek(0.0), p);
    assert!((closed.max[1] - 65.0).abs() < 1e-6);
    let open = progress_box(1, physical_peek(1.0), p);
    assert!((open.max[1] - 66.0).abs() < 1e-3, "{open:?}");
    assert_eq!(open.min[1], 64.0);
    // Attached to the ceiling (opens downward).
    let down = progress_box(0, 1.0, p);
    assert!((down.min[1] - 63.0).abs() < 1e-9);
    assert_eq!(snap([3.7, 10.6, -2.2]), [3.5, 11.0, -2.5]);
}
