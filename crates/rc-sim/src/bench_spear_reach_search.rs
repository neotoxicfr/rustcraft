//! The player's spear reach search (stab, and every tick of a charge) among 1,000 zombies spread
//! over the flat world: µs per search with the search box (`RUSTCRAFT_OPT_SPEAR_SEARCH_AREA`, the
//! default) and without it (ray tests on every mob). Both find the same targets (checked here, and
//! for any box by `player_spear::tests::reach_search_area_keeps_every_hit`).
//! Run: `cargo test --release -p rc-sim bench_spear_reach_search -- --ignored --nocapture --test-threads=1`.

use crate::bench_kit::{median, report};
use crate::mobs::species::Kind;

#[test]
#[ignore = "micro-bench: run alone on a quiet machine"]
fn bench_spear_reach_search() {
    let (mut p, _dir) = crate::tests_g3f13::open("bench-spear-search");
    p.s.yaw = -90.0;
    p.s.pitch = 0.0;
    let spear = crate::player::Stack { id: "minecraft:iron_spear".into(), count: 1, components: None };
    p.s.slots[36 + p.s.held as usize] = Some(spear.clone());
    p.s.equipment_tick();
    // 1,000 zombies on a grid over the loaded chunks, a few of them in the spear's reach (east).
    for k in 0..1_000 {
        let pos = [-30.5 + f64::from(k % 50) * 3.7, -60.0, -30.5 + f64::from(k / 50) * 3.6];
        let z = p.s.new_mob(Kind::Zombie, pos, 0.0);
        p.s.add_mob(z);
    }
    for (x, z) in [(3.0, 0.5), (4.6, 0.6)] {
        let m = p.s.new_mob(Kind::Zombie, [x, -60.0, z], 0.0);
        p.s.add_mob(m);
    }
    let searches = 2_000;
    let mut found = Vec::new();
    for (case, use_search_area) in [("search_box", true), ("every_mob", false)] {
        let mut runs = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            for _ in 0..searches {
                found = std::hint::black_box(p.s.player_spear_targets_with(Some(&spear), use_search_area));
            }
            runs.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(searches));
        }
        report("spear_reach_search", case, "us_per_search", median(&runs));
        report("spear_reach_search", case, "targets", found.len() as f64);
    }
    // Deterministic count behind the timings: the mobs that get the ray tests in each case.
    let eye = [p.s.pos[0], p.s.player_eye_y(), p.s.pos[2]];
    let look = crate::mobs::cubes::view_vector(p.s.pitch, p.s.yaw);
    let seg = p.s.weapon_reach_segment(eye, look, (2.0, 4.5), [0.0; 3]).expect("segment");
    let area = seg.search_area(f64::from(0.125f32));
    let tested = p.s.mobs.list.iter().filter(|o| o.touches(&area)).count();
    report("spear_reach_search", "search_box", "ray_tests", tested as f64);
    report("spear_reach_search", "every_mob", "ray_tests", p.s.mobs.list.iter().count() as f64);
    let with_box = p.s.player_spear_targets_with(Some(&spear), true);
    assert_eq!(with_box, p.s.player_spear_targets_with(Some(&spear), false), "the search box changed the targets");
    assert_eq!(with_box.len(), 2, "targets in reach: {with_box:?}");
}
