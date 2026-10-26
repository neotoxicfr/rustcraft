//! End flash: no flash during the first period of the clock, start detected once
//! per flash, light cut off by the "hide lightning" option, sound direction.

use super::*;

/// Clock ticks `range`: (tick, intensity, start).
fn run(f: &mut EndFlash, range: std::ops::Range<i64>) -> Vec<(i64, f32, bool)> {
    range
        .map(|t| {
            f.tick(t);
            (t, f.now, f.started())
        })
        .collect()
}

/// The game starts from a period 0 with no drawn parameters: no flash before tick 600.
#[test]
fn no_flash_in_first_period() {
    let mut f = EndFlash::default();
    assert!(run(&mut f, 0..600).iter().all(|&(_, now, start)| now == 0.0 && !start));
}

/// Each 600-tick period has exactly one flash, reported at its first tick of
/// positive intensity.
#[test]
fn one_start_per_flash() {
    let mut f = EndFlash::default();
    let ticks = run(&mut f, 0..6000);
    for period in 1..10 {
        let starts: Vec<_> = ticks.iter().filter(|(t, _, s)| *s && t / 600 == period).collect();
        assert_eq!(starts.len(), 1, "period {period}");
        let t = starts[0].0;
        assert!(starts[0].1 > 0.0);
        assert_eq!(ticks[(t - 1) as usize].1, 0.0);
    }
}

/// "Hide lightning" removes the flash from the world light (the sky always draws it).
#[test]
fn hide_flashes_removes_end_flash_light() {
    let mut e = EnvState { skybox: 2, ..Default::default() };
    let mut t = 600;
    while e.flash.now < 0.5 {
        e.flash.tick(t);
        t += 1;
    }
    assert!(e.end_flash_light(1.0) >= 0.5);
    e.boss_flags = 4;
    assert!((e.end_flash_light(1.0) - e.flash.now / 3.0).abs() < 1e-6);
    e.hide_flashes = true;
    assert_eq!(e.end_flash_light(1.0), 0.0);
    // Outside the End: never.
    let e = EnvState { skybox: 1, flash: e.flash, ..Default::default() };
    assert_eq!(e.end_flash_light(1.0), 0.0);
}

/// Sound direction (game formula, tabulated sines): pitch -30 and yaw 90 → toward -x and up.
#[test]
fn end_flash_direction_matches_game() {
    let d = end_flash_direction(-30.0, 90.0);
    assert!((d[0] + 0.866_025).abs() < 1e-4 && (d[1] - 0.5).abs() < 1e-4 && d[2].abs() < 1e-4, "{d:?}");
    let d = end_flash_direction(0.0, 0.0);
    assert!(d[0].abs() < 1e-6 && d[1].abs() < 1e-6 && (d[2] - 1.0).abs() < 1e-6, "{d:?}");
}
