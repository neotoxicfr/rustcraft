//! Projectile benches (switches of `projectiles::paths`), each path alone over the old ones then
//! all of them:
//! - server ticks of a flat world holding 1,000 arrows stuck in the ground, plain then effect
//!   arrows whose stack has components (the tick copy, the stuck test, the clip);
//! - the target search and the owner lookup of one flying arrow over a crowd of 1,000 mobs
//!   (shooter alive, then a UUID no mob carries);
//! - the block clip of a projectile across slabs, fences and stone;
//! - one skeleton arrow hitting the player; one chorus fruit in a closed room; one open-water test
//!   of the bobber; the tick copy of one projectile under three layouts (full copy, heavy fields
//!   moved, heavy fields shared behind an `Arc`).
//!
//! Run alone: `cargo test --release -p rc-sim [--features memstats] bench_projectile -- --ignored
//! --nocapture --test-threads=1`. Times and allocations per call come from the tests' per-thread
//! allocation counter (without `memstats`).

use rc_physics::Aabb;

use super::paths::{FORCED_PROJECTILE_PATHS, ProjectilePaths};
use super::{PKind, Proj};
use crate::bench_kit::{cmd, report, report_timing, time_ticks};
use crate::blocks::probe::Probe;
use crate::mobs::jrand::JRand;
use crate::player::Stack;
use crate::tests_g3f13::{finish, open};

const BENCH: &str = "bench_projectile_tick";

/// Each new path alone over the old ones, then every new one.
fn paths() -> Vec<(&'static str, ProjectilePaths)> {
    let old = ProjectilePaths::OLD;
    vec![
        ("old", old),
        ("take-heavy-fields", ProjectilePaths { take_heavy_fields: true, ..old }),
        ("borrowed-shapes", ProjectilePaths { borrowed_shapes: true, ..old }),
        ("owner-index", ProjectilePaths { owner_index: true, ..old }),
        ("target-prefilter", ProjectilePaths { target_prefilter: true, ..old }),
        ("targets-in-place", ProjectilePaths { targets_in_place: true, ..old }),
        ("hit-without-mob-clone", ProjectilePaths { hit_without_mob_clone: true, ..old }),
        ("chorus-tag-bits", ProjectilePaths { chorus_tag_bits: true, ..old }),
        ("fishing-state-bits", ProjectilePaths { fishing_state_bits: true, ..old }),
        ("new", ProjectilePaths::NEW),
    ]
}

const TIPPED: &str = "item:{id:\"minecraft:tipped_arrow\",count:1,components:{\"minecraft:potion_contents\":{potion:\"minecraft:long_poison\"}}}";

/// Median of 7 runs of `n` calls of `f`: nanoseconds and allocations per call.
fn per_call(n: u32, f: &mut dyn FnMut()) -> (f64, f64) {
    let mut runs = Vec::new();
    for _ in 0..7 {
        let (t0, a0) = (std::time::Instant::now(), crate::test_alloc::allocations());
        for _ in 0..n {
            f();
        }
        let ns = t0.elapsed().as_secs_f64() * 1e9 / f64::from(n);
        runs.push((ns, (crate::test_alloc::allocations() - a0) as f64 / f64::from(n)));
    }
    runs.sort_by(|a, b| a.0.total_cmp(&b.0));
    runs[runs.len() / 2]
}

fn report_call(case: &str, (ns, allocs): (f64, f64)) {
    report(BENCH, case, "ns_per_call", ns);
    report(BENCH, case, "allocs_per_call", allocs);
}

// ---------- Server ticks with stuck arrows ----------

/// 1,000 arrows dropped on a 40 × 25 grid (they stick in the ground within a few ticks; measured
/// well before their 1,200 ticks of life end).
fn stuck_arrows(p: &mut Probe, extra: &str) {
    for k in 0..1000 {
        let (x, z) = (12.5 + f64::from(k % 40) * 2.0, -14.5 + f64::from(k / 40) * 1.2);
        cmd(p, &format!("summon minecraft:arrow {x} -59 {z} {{{extra}}}"));
    }
}

fn ticks_case(tag: &str, setup: fn(&mut Probe)) {
    for (label, paths) in paths() {
        FORCED_PROJECTILE_PATHS.set(Some(paths));
        let (mut p, dir) = open(&format!("bench-proj-{tag}-{label}"));
        setup(&mut p);
        let t = time_ticks(&mut p, 60, 200, 5);
        report_timing(BENCH, &format!("{tag}/{label}"), &t);
        // Deterministic count behind the timing: allocations of the server thread per tick.
        let a0 = crate::test_alloc::allocations();
        crate::bench_kit::run_ticks(&mut p, 50);
        report(
            BENCH,
            &format!("{tag}/{label}"),
            "thread_allocs_per_tick",
            (crate::test_alloc::allocations() - a0) as f64 / 50.0,
        );
        finish(p, dir);
    }
    FORCED_PROJECTILE_PATHS.set(None);
}

#[test]
#[ignore = "measurement bench"]
fn bench_projectile_tick() {
    crate::mobs::index::set_verify(false);
    for _ in 0..2 {
        ticks_case("stuck-arrows", |p| stuck_arrows(p, ""));
        ticks_case("stuck-effect-arrows", |p| stuck_arrows(p, TIPPED));
    }
}

// ---------- Target search, owner lookup, block clip ----------

/// 1,000 mobs of eight species on a 60 × 60 square (no ticks: only the searches are measured).
fn crowd(p: &mut Probe) -> Vec<u128> {
    use crate::mobs::species::Kind;
    let kinds =
        [Kind::Zombie, Kind::Skeleton, Kind::Pig, Kind::Slime, Kind::Chicken, Kind::Spider, Kind::Vex, Kind::Cow];
    let mut r = JRand::new(0x5EED);
    let mut uuids = Vec::new();
    for k in 0..1000 {
        let pos = [r.next_double() * 60.0 - 30.0, -60.0, r.next_double() * 60.0 - 30.0];
        let m = p.s.new_mob(kinds[k % kinds.len()], pos, 0.0);
        uuids.push(m.uuid);
        p.s.add_mob(m);
    }
    uuids
}

/// One arrow flying at 1.5 blocks a tick, its search area as the tick builds it: the nearest hit
/// (every tick of every flying projectile), with the shooter in the crowd then absent.
#[test]
#[ignore = "measurement bench"]
fn bench_projectile_targets_and_owner() {
    crate::mobs::index::set_verify(false);
    let (mut p, dir) = open("bench-proj-targets");
    let uuids = crowd(&mut p);
    let mut arrow = Proj::new(
        PKind::Arrow,
        p.s.proj_new_id(),
        [0.5, -59.5, 0.5],
        Stack { id: "minecraft:arrow".into(), count: 1, components: None },
    );
    arrow.vel = [1.5, 0.0, 0.3];
    arrow.left_owner = true;
    let from = arrow.pos;
    let to = [from[0] + arrow.vel[0], from[1], from[2] + arrow.vel[2]];
    let b = arrow.bbox().expand_towards(arrow.vel);
    let area = Aabb { min: b.min.map(|v| v - 1.0), max: b.max.map(|v| v + 1.0) };
    for (owner, shooter) in [("shooter-alive", uuids[500]), ("shooter-absent", 0x0bad_u128)] {
        arrow.owner = Some(shooter);
        for (label, paths) in paths() {
            FORCED_PROJECTILE_PATHS.set(Some(paths));
            let s = &p.s;
            let nearest = per_call(2000, &mut || {
                std::hint::black_box(s.entity_hit_nearest(&arrow, from, to, &area, 0.1, false));
            });
            report_call(&format!("nearest-target/{owner}/{label}"), nearest);
            let lookup = per_call(20_000, &mut || {
                std::hint::black_box(s.proj_owner(&arrow));
            });
            report_call(&format!("owner-lookup/{owner}/{label}"), lookup);
        }
    }
    FORCED_PROJECTILE_PATHS.set(None);
    finish(p, dir);
}

/// The block clip of a projectile across a field of slabs, fences and stone (every cell of the
/// segment looked at, most of them with a shape).
#[test]
#[ignore = "measurement bench"]
fn bench_projectile_block_clip() {
    let (mut p, dir) = open("bench-proj-clip");
    cmd(&mut p, "fill 2 -60 -2 30 -60 2 minecraft:stone_slab");
    cmd(&mut p, "fill 2 -59 -2 30 -59 2 minecraft:oak_fence");
    cmd(&mut p, "fill 31 -60 -2 31 -55 2 minecraft:stone");
    // Grazing the tops of the fences (no hit before the stone wall).
    let (from, to) = ([1.5, -57.49, 0.5], [32.0, -57.49, 0.7]);
    for (label, paths) in [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)] {
        FORCED_PROJECTILE_PATHS.set(Some(paths));
        let s = &p.s;
        let clip = per_call(20_000, &mut || {
            std::hint::black_box(s.proj_clip(from, to));
        });
        report_call(&format!("block-clip/{label}"), clip);
        let low = per_call(20_000, &mut || {
            std::hint::black_box(s.proj_clip([1.5, -59.6, 0.5], [32.0, -59.6, 0.5]));
        });
        report_call(&format!("block-clip-into-fences/{label}"), low);
    }
    FORCED_PROJECTILE_PATHS.set(None);
    finish(p, dir);
}

// ---------- Hit on the player, chorus fruit, open water ----------

/// One skeleton arrow on the player (the shooter in the list), old and new paths.
#[test]
#[ignore = "measurement bench"]
fn bench_projectile_hit_on_player() {
    use crate::mobs::mob::Ent;
    let (mut p, dir) = open("bench-proj-hit");
    cmd(&mut p, "effect give @a minecraft:resistance infinite 4 true");
    let bow = "equipment:{mainhand:{id:\"minecraft:bow\",count:1}}";
    cmd(&mut p, &format!("summon minecraft:skeleton 4.5 -60 0.5 {{NoAI:1b,PersistenceRequired:1b,{bow}}}"));
    let id =
        p.s.mobs.list.iter().find(|m| m.kind == crate::mobs::species::Kind::Skeleton).map(|m| m.id).expect("skeleton");
    eprintln!("{BENCH}: size of a mob {} bytes", std::mem::size_of::<crate::mobs::mob::Mob>());
    for (label, paths) in [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)] {
        FORCED_PROJECTILE_PATHS.set(Some(paths));
        let hit = per_call(2000, &mut || {
            p.s.life.vit.cooldown = 0;
            p.s.proj_damage(Ent::Player, "minecraft:arrow", 2.0, Some(Ent::Mob(id)), (0.1, 0.0));
        });
        let _ = p.take_events();
        report_call(&format!("hit-on-player/{label}"), hit);
    }
    FORCED_PROJECTILE_PATHS.set(None);
    finish(p, dir);
}

/// One chorus fruit eaten in a closed stone room (16 attempts, most of them walk down a whole
/// stone column before failing), the player put back after each fruit.
#[test]
#[ignore = "measurement bench"]
fn bench_projectile_chorus_fruit() {
    let (mut p, dir) = open("bench-proj-chorus");
    for room in ["stone", "end_stone"] {
        cmd(&mut p, &format!("fill -12 -63 -12 12 -40 12 minecraft:{room}"));
        cmd(&mut p, "fill -1 -60 -1 1 -58 1 minecraft:air");
        for (label, paths) in [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)] {
            FORCED_PROJECTILE_PATHS.set(Some(paths));
            let fruit = per_call(50, &mut || {
                p.s.pos = [0.5, -60.0, 0.5];
                std::hint::black_box(p.s.chorus_teleport(16.0, true));
            });
            let _ = p.take_events();
            report_call(&format!("chorus-fruit/{room}-room/{label}"), fruit);
        }
    }
    FORCED_PROJECTILE_PATHS.set(None);
    finish(p, dir);
}

/// The chorus fruit's two tag tests on every block state: the cost of one cell of its descent,
/// averaged over the block list (the old search by name costs more the further the block sits in
/// the list).
#[test]
#[ignore = "measurement bench"]
fn bench_projectile_chorus_tag_per_cell() {
    let (p, dir) = open("bench-proj-chorus-tags");
    let states = p.s.data.states.len() as u32;
    let tags = ["minecraft:entities_can_teleport_to", "minecraft:consumable_does_not_teleport_to"];
    for (label, paths) in [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)] {
        FORCED_PROJECTILE_PATHS.set(Some(paths));
        let s = &p.s;
        let (ns, allocs) = per_call(3, &mut || {
            for st in 0..states {
                for tag in tags {
                    std::hint::black_box(s.chorus_block_tag(st, tag));
                }
            }
        });
        let tests = f64::from(states) * 2.0;
        report_call(&format!("chorus-tag-test/{label}"), (ns / tests, allocs / tests));
    }
    FORCED_PROJECTILE_PATHS.set(None);
    finish(p, dir);
}

/// One open-water test of the bobber (100 cells) on a 9 × 9 pond, open then roofed.
#[test]
#[ignore = "measurement bench"]
fn bench_projectile_open_water() {
    let (mut p, dir) = open("bench-proj-water");
    cmd(&mut p, "fill 2 -62 -4 10 -61 4 minecraft:water");
    for roof in [false, true] {
        if roof {
            cmd(&mut p, "fill 2 -59 -4 10 -59 4 minecraft:stone");
        }
        for (label, paths) in [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)] {
            FORCED_PROJECTILE_PATHS.set(Some(paths));
            let s = &p.s;
            let test = per_call(20_000, &mut || {
                std::hint::black_box(s.open_water_at([6, -61, 0]));
            });
            report_call(&format!("open-water/{}/{label}", if roof { "roofed" } else { "open" }), test);
        }
    }
    FORCED_PROJECTILE_PATHS.set(None);
    finish(p, dir);
}

/// The tick copy of one effect arrow (stack with components, data, pierced list) under three
/// layouts: the full copy written back (old), the heavy fields moved out and back (new), and a
/// model of heavy fields shared behind an `Arc` (copy = light copy + count up and down; any write
/// during the tick would copy every heavy field, the slot holding the second count).
#[test]
#[ignore = "measurement bench"]
fn bench_projectile_copy_layouts() {
    use std::sync::Arc;
    let (mut p, dir) = open("bench-proj-layouts");
    cmd(&mut p, &format!("summon minecraft:arrow 4.5 -59 0.5 {{{TIPPED},PierceLevel:2b}}"));
    p.s.proj.list[0].pierced = vec![1, 2, 3];
    let full = per_call(200_000, &mut || {
        let q = p.s.proj.list[0].clone();
        p.s.proj.list[0] = std::hint::black_box(q);
    });
    let moved = per_call(200_000, &mut || {
        let q = p.s.take_for_tick(0, ProjectilePaths::NEW);
        p.s.proj.list[0] = std::hint::black_box(q);
    });
    // Model: the light part copied, the heavy fields shared.
    let mut light = p.s.proj.list[0].clone();
    let shared = Arc::new(light.take_heavy_fields());
    let arc = per_call(200_000, &mut || {
        let q = (light.clone(), Arc::clone(&shared));
        drop(std::hint::black_box(q));
    });
    for (layout, result) in [("full-copy", full), ("heavy-moved", moved), ("heavy-shared-arc", arc)] {
        report_call(&format!("copy-layout/{layout}"), result);
    }
    finish(p, dir);
}
