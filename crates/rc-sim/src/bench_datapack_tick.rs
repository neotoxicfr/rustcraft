//! Server tick of the bench's `datapack` scene on the flat world, from the scene bench's own
//! `perf` pack (so the micro-bench and the scene run the same commands): crowd of 1,000 farm
//! animals (`perf:mobs1000`), then its realistic `#tick` workload (`perf:datapack_tick`: tp of
//! the cows, scores, NBT test, effect, kill, particle, macro), then the 256 repeating command
//! blocks as well. The difference between cases is the cost of the commands. Needs
//! `RUSTCRAFT_PERF_PACK` (see `tests_perf_pack.rs`); run alone:
//! `cargo test --release -p rc-sim [--features memstats] bench_datapack_tick -- --ignored --nocapture`.

use crate::bench_kit::{self, cmd, function};

/// Crowd camera of the flat-world pack (`FLAT_SITES` of `scenes.py`: site (64, -61, 8)).
const CROWD_CAMERA: &str = "64.5 -52 -13.5";

#[derive(Clone, Copy, PartialEq)]
enum Load {
    Crowd,
    CrowdAndTick,
    CrowdTickAndCommandBlocks,
}

fn case(name: &str, load: Load) {
    let (mut p, dir) = bench_kit::flat_world_with_pack(&format!("dp-{name}"), &bench_kit::perf_pack_dir());
    cmd(&mut p, &format!("tp @a {CROWD_CAMERA}"));
    if load == Load::CrowdTickAndCommandBlocks {
        function(&mut p, "perf:datapack");
    } else {
        function(&mut p, "perf:mobs1000");
    }
    if load == Load::CrowdAndTick {
        cmd(&mut p, "scoreboard objectives add perf_dp dummy");
        cmd(&mut p, "data modify storage perf:dp args set value {value:7}");
    }
    if load != Load::Crowd {
        cmd(&mut p, "tag @a add perf_datapack");
    }
    let mobs = p.s.mobs.list.iter().flatten().count();
    assert!(mobs >= 1000, "{name}: {mobs} mobs, the crowd function did not run");
    let t = bench_kit::time_ticks(&mut p, WARMUP_TICKS, MEASURED_TICKS, RUNS);
    check_workload_ran(&p, name, load);
    bench_kit::report("datapack_tick", name, "mobs", mobs as f64);
    bench_kit::report_timing("datapack_tick", name, &t);
    crate::tests_g3f13::finish(p, dir);
}

const WARMUP_TICKS: u32 = 40;
const MEASURED_TICKS: u32 = 200;
const RUNS: usize = 3;

/// The case measured what it claims: the `#tick` workload ran (its macro wrote 7) and the
/// command blocks ran every tick (32 of the 256 add 1 to `#blocks`). Without this, a refused
/// `setblock` or a broken tick tag would leave the case timing the crowd alone.
fn check_workload_ran(p: &crate::blocks::probe::Probe, name: &str, load: Load) {
    let score = |holder: &str| p.s.cmd.scores.get(holder, "perf_dp");
    if load != Load::Crowd {
        assert_eq!(score("#macro"), Some(7), "{name}: the #tick workload did not run");
    }
    if load == Load::CrowdTickAndCommandBlocks {
        let ticks = i32::try_from(MEASURED_TICKS as usize * RUNS).expect("tick count");
        let blocks = score("#blocks").unwrap_or(0);
        assert!(
            blocks >= 32 * ticks,
            "{name}: #blocks = {blocks} after {ticks} ticks, the command blocks did not all run"
        );
    }
}

#[test]
#[ignore = "measurement bench (needs RUSTCRAFT_PERF_PACK)"]
fn bench_datapack_tick() {
    crate::mobs::index::set_verify(false);
    for _ in 0..2 {
        case("crowd", Load::Crowd);
        case("crowd_tick", Load::CrowdAndTick);
        case("crowd_tick_blocks", Load::CrowdTickAndCommandBlocks);
    }
}
