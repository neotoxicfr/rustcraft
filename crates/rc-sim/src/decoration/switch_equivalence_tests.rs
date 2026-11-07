//! Equivalence of the decoration switches (`DecoSwitches`): each scenario runs on a fresh flat
//! world in a thread of its own (the test seeds of sounds and randoms follow the thread, so every
//! run draws the same values), once per path. Its trace (every event sent to the client, the
//! ground items, the encoded entity lists of each chunk) must be the same on every path and match
//! a stored fingerprint. Version data mandatory. Micro-benches at the end (ignored).

use super::DecoSwitches;
use crate::blocks::probe::Probe;
use crate::mobs::mob::Ent;
use crate::tests_g3f13::{finish, open};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn digest(lines: &[String]) -> u64 {
    lines.iter().fold(0xcbf2_9ce4_8422_2325, |h, l| fnv(fnv(h, l.as_bytes()), b"\n"))
}

const ALL_OLD: DecoSwitches = DecoSwitches { by_index: false, slot_hint: false, explosion_borrow: false, still_distance_first: false };
const ALL_NEW: DecoSwitches = DecoSwitches { by_index: true, slot_hint: true, explosion_borrow: true, still_distance_first: true };

/// Events sent to the client since the last read, one line each; the advancement updates are
/// left out (they carry the wall clock time of the recipes unlocked at the first tick).
fn record_events(p: &Probe, trace: &mut Vec<String>) {
    let lines = p.take_events().into_iter().map(|e| format!("{e:?}"));
    trace.extend(lines.filter(|l| !l.starts_with("Progress(Advancements(")));
}

/// What stays at the end: ground items, then each chunk's entity list in its order, encoded as
/// the entity chunk saves it.
fn record_world(p: &Probe, trace: &mut Vec<String>) {
    trace.extend(p.s.life.drops.iter().map(|d| format!("drop {} {} {:?} {:?}", d.stack.id, d.stack.count, d.pos, d.vel)));
    let mut chunks: Vec<_> = p.s.entities.iter().filter(|(_, list)| !list.is_empty()).collect();
    chunks.sort_by_key(|(c, _)| **c);
    for (c, list) in chunks {
        let mut bytes = Vec::new();
        for (id, n) in list {
            bytes.extend_from_slice(&id.to_le_bytes());
            rc_proto::nbt::write_named(&mut bytes, "", n);
        }
        trace.push(format!("chunk {c:?}: {} entities, {:016x}", list.len(), fnv(0xcbf2_9ce4_8422_2325, &bytes)));
    }
}

/// Runs `scene` on a fresh world, in its own thread, with `switches` on its server.
fn run_scene(tag: String, switches: DecoSwitches, scene: fn(&mut Probe, &mut Vec<String>)) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let (mut p, dir) = open(&tag);
        p.s.deco.switches = switches;
        let mut trace = Vec::new();
        scene(&mut p, &mut trace);
        record_world(&p, &mut trace);
        finish(p, dir);
        trace
    })
}

/// Every path gives the reference trace; the reference's fingerprint is the stored one
/// (Windows only: the libc's sine and cosine differ elsewhere, as for the other fingerprints).
fn assert_paths_agree(name: &str, scene: fn(&mut Probe, &mut Vec<String>), paths: &[(&str, DecoSwitches)], stored: u64) {
    let runs: Vec<_> = paths.iter().map(|(label, s)| (*label, run_scene(format!("{name}-{label}"), *s, scene))).collect();
    let traces: Vec<(&str, Vec<String>)> = runs.into_iter().map(|(l, h)| (l, h.join().expect("scene thread"))).collect();
    let (reference_label, reference) = &traces[0];
    assert!(reference.len() > 50, "{name}: trace too short ({} lines), the scene did nothing", reference.len());
    for (label, trace) in &traces[1..] {
        let first_diff = reference.iter().zip(trace).position(|(a, b)| a != b);
        assert!(
            trace == reference,
            "{name}: path {label} differs from {reference_label} (lengths {} / {}, first difference at line {first_diff:?}: {:?} / {:?})",
            reference.len(),
            trace.len(),
            first_diff.map(|k| &reference[k]),
            first_diff.map(|k| &trace[k]),
        );
    }
    let got = digest(reference);
    eprintln!("{name}: {} lines, fingerprint {got:#018x}", reference.len());
    if cfg!(windows) {
        assert_eq!(got, stored, "{name}: fingerprint changed (behaviour differs from the stored reference)");
    }
}

fn summon(p: &mut Probe, what: &str) {
    cmd(p, &format!("summon {what}"));
}

/// Wall of item frames (fixed or not, glowing or not, with items) and paintings on the north face
/// of a stone wall at z 10, x from `x0` to `x1`.
fn hanging_wall(p: &mut Probe, x0: i32, x1: i32) {
    cmd(p, &format!("fill {x0} -60 10 {x1} -56 10 minecraft:stone"));
    for x in x0..=x1 {
        let (kind, fixed) = match x % 4 {
            0 => ("item_frame", "1b"),
            1 => ("glow_item_frame", "0b"),
            _ => ("item_frame", "0b"),
        };
        summon(
            p,
            &format!(
                "minecraft:{kind} {x}.5 -59.5 9.96875 {{Facing:2b,Fixed:{fixed},block_pos:[I;{x},-60,9],\
                 Item:{{id:\"minecraft:diamond\",count:1}}}}"
            ),
        );
        if x % 3 == 0 {
            summon(p, &format!("minecraft:painting {x}.5 -57.5 9.96875 {{facing:2b,variant:\"minecraft:kebab\",block_pos:[I;{x},-58,9]}}"));
        }
    }
}

// ---------- Explosion ----------

/// Decorations near and far from the blasts: frames and paintings, armor stands (equipped,
/// invisible, marker, invulnerable), mannequins, fence knots; then blasts by a mob with
/// `mob_griefing` off, from a wet source, without block-like entities, and an ordinary one.
fn explosion_scene(p: &mut Probe, trace: &mut Vec<String>) {
    hanging_wall(p, 18, 43);
    for (k, x) in (20..42).step_by(2).enumerate() {
        let extra = match k % 4 {
            0 => "equipment:{mainhand:{id:\"minecraft:diamond_sword\",count:1},head:{id:\"minecraft:iron_helmet\",count:1}}",
            1 => "Invisible:1b",
            2 => "Marker:1b",
            _ => "Invulnerable:1b",
        };
        summon(p, &format!("minecraft:armor_stand {x}.5 -60 6.5 {{{extra}}}"));
    }
    for x in (21..42).step_by(3) {
        summon(p, &format!("minecraft:mannequin {x}.5 -60 4.5"));
    }
    for x in [24, 30, 36, 44] {
        cmd(p, &format!("setblock {x} -60 2 minecraft:oak_fence"));
        p.s.knot_create([x, -60, 2]);
    }
    // Far from every blast (beyond twice the radius).
    for k in 0..40 {
        summon(p, &format!("minecraft:armor_stand {}.5 -60 {}.5 {{equipment:{{head:{{id:\"minecraft:iron_helmet\",count:1}}}}}}", 90 + k % 10, k / 10));
    }
    game_ticks(p, 2);
    record_events(p, trace);
    let center = [30.5, -59.0, 7.0];
    let mob = Some(Ent::Mob(424_242));
    cmd(p, "gamerule mob_griefing false");
    p.s.deco_explosion(center, 4.0, "minecraft:explosion", mob, false, true);
    record_events(p, trace);
    cmd(p, "gamerule mob_griefing true");
    p.s.deco_explosion(center, 4.0, "minecraft:explosion", None, true, true);
    record_events(p, trace);
    p.s.deco_explosion([25.5, -59.0, 5.0], 3.0, "minecraft:explosion", None, false, false);
    record_events(p, trace);
    game_ticks(p, 3);
    record_events(p, trace);
    p.s.deco_explosion([36.5, -59.0, 7.0], 3.0, "minecraft:player_explosion", Some(Ent::Player), false, true);
    p.s.deco_explosion(center, 4.0, "minecraft:explosion", None, false, true);
    record_events(p, trace);
    game_ticks(p, 30);
    record_events(p, trace);
    // Wind bursts (knockback alone, same reach reckoning as the explosion): on the frames and
    // mannequins left at the end of the wall, then on the far stands, block-like or not.
    p.s.deco_blast_push([41.5, -59.0, 7.0], 2.5, 1.2, true);
    p.s.deco_blast_push([94.5, -60.0, 2.0], 3.0, 1.0, false);
    p.s.deco_blast_push([94.5, -60.0, 2.0], 3.0, 1.0, true);
    record_events(p, trace);
    game_ticks(p, 10);
    record_events(p, trace);
}

const EXPLOSION_FINGERPRINT: u64 = 0xe705_965b_85a1_c8c9;

/// The explosion reads the compounds in place: same events, drops and compounds as the former
/// copy of every compound, and the wind bursts (sharing its reach reckoning) keep their stored
/// result. Sabotages seen failing: the reach taken as the radius instead of twice it; a killed
/// attached decoration decided before the reach test; a blast push given to the next entry.
#[test]
fn explosion_paths_agree() {
    let borrow_only = DecoSwitches { explosion_borrow: true, ..ALL_OLD };
    assert_paths_agree(
        "deco-explosion",
        explosion_scene,
        &[("old", ALL_OLD), ("new", ALL_NEW), ("borrow", borrow_only)],
        EXPLOSION_FINGERPRINT,
    );
}

// ---------- Ticks ----------

/// 100 frames and paintings, 20 armor stands falling (some sliding across a chunk border), 10
/// mannequins hurt until most die, a wall segment removed (its frames drop): 400 ticks.
fn tick_scene(p: &mut Probe, trace: &mut Vec<String>) {
    hanging_wall(p, 6, 80);
    for k in 0..20 {
        let x = 10 + 3 * k;
        cmd(p, &format!("fill {x} -60 6 {x} -55 6 minecraft:stone"));
        // Every fourth one slides along x: across the border of chunk x 16, 32, 48 or 64.
        let motion = if k % 4 == 0 { "Motion:[0.6d,0.0d,0.0d]," } else { "" };
        summon(p, &format!("minecraft:armor_stand {x}.5 -54 6.5 {{{motion}Invisible:0b}}"));
    }
    for k in 0..10 {
        summon(p, &format!("minecraft:mannequin {}.5 -60 3.5", 12 + 5 * k));
    }
    game_ticks(p, 5);
    record_events(p, trace);
    for k in 0..20 {
        let x = 10 + 3 * k;
        cmd(p, &format!("fill {x} -60 6 {x} -55 6 minecraft:air"));
    }
    // Sorted: the entity map's order changes from one server to the next.
    let mut mannequins: Vec<i32> = p
        .s
        .entities
        .values()
        .flat_map(|l| l.iter())
        .filter(|(_, e)| super::kind_of(e) == Some(super::DKind::Mannequin))
        .map(|(id, _)| *id)
        .collect();
    mannequins.sort_unstable();
    for t in 0..400 {
        if t % 37 == 0 {
            for (k, id) in mannequins.iter().enumerate() {
                p.s.mannequin_hurt(*id, "minecraft:generic", 2.0 + k as f32, None, Some([40.0, -60.0, 0.0]));
            }
        }
        if t == 150 {
            cmd(p, "fill 20 -60 10 29 -56 10 minecraft:air");
        }
        game_ticks(p, 1);
        record_events(p, trace);
    }
}

const TICK_FINGERPRINT: u64 = 0x663c_417e_54b2_43ad;

/// The lookups by registry index and by remembered slot find the same compounds as the walks by
/// identifier: same packets and same entity lists over 400 ticks, each lead alone and together.
/// Sabotages seen failing: the remembered slot used without its identifier check; the survival
/// check always true; the edit by index applied to the next entry.
#[test]
fn tick_lookup_paths_agree() {
    assert_paths_agree(
        "deco-ticks",
        tick_scene,
        &[
            ("old", ALL_OLD),
            ("new", ALL_NEW),
            ("by-index", DecoSwitches { by_index: true, ..ALL_OLD }),
            ("slot-hint", DecoSwitches { slot_hint: true, ..ALL_OLD }),
        ],
        TICK_FINGERPRINT,
    );
}

// ---------- Still compounds in a blast ----------

/// Displays, markers and interactions near and far, among decorations, pushed by bursts: the
/// distance tested before or after the type gives the same motions and packets.
fn still_scene(p: &mut Probe, trace: &mut Vec<String>) {
    hanging_wall(p, 10, 30);
    for k in 0..30 {
        let (x, z) = (10 + k, 1 + k % 5);
        let what = match k % 4 {
            0 => "minecraft:block_display",
            1 => "minecraft:marker",
            2 => "minecraft:interaction",
            _ => "minecraft:text_display",
        };
        summon(p, &format!("{what} {x}.5 -59.25 {z}.5"));
    }
    game_ticks(p, 1);
    record_events(p, trace);
    for (k, x) in [12.5, 20.5, 27.5, 35.5].into_iter().enumerate() {
        p.s.blast_push_still([x, -59.0, 3.0], 2.0 + k as f32, 1.0 + 0.25 * k as f64);
        // Sorted: the pass walks the entity map, whose chunk order changes from one server to
        // the next (the packets of one blast leave in that order, on every path alike).
        let mut blast = Vec::new();
        record_events(p, &mut blast);
        blast.sort_unstable();
        trace.extend(blast);
    }
    // Every kept compound's motion, in list order.
    let mut chunks: Vec<_> = p.s.entities.iter().collect();
    chunks.sort_by_key(|(c, _)| **c);
    for (_, list) in chunks {
        for (id, n) in list {
            trace.push(format!("{id} {:?}", n.get("Motion")));
        }
    }
}

const STILL_FINGERPRINT: u64 = 0x80ef_103d_3e84_e275;

/// The still compounds' blast pass tests the distance before or after the type: same motions
/// and packets. Sabotage seen failing: the reach halved on the distance-first path.
#[test]
fn still_push_paths_agree() {
    assert_paths_agree(
        "still-push",
        still_scene,
        &[("old", ALL_OLD), ("distance-first", DecoSwitches { still_distance_first: true, ..ALL_OLD })],
        STILL_FINGERPRINT,
    );
}

// ---------- Micro-benches (ignored: `--ignored --nocapture`, release build) ----------

/// `rows` rows of fixed item frames holding a diamond, x from 10 to 134, on a stone wall at z 10.
fn fixed_frames(p: &mut Probe, rows: i32) {
    cmd(p, "fill 10 -60 10 134 -55 10 minecraft:stone");
    for y in -60..-60 + rows {
        for x in 10..135 {
            summon(
                p,
                &format!(
                    "minecraft:item_frame {x}.5 {}.5 9.96875 {{Facing:2b,Fixed:1b,block_pos:[I;{x},{y},9],\
                     Item:{{id:\"minecraft:diamond\",count:1}}}}",
                    y
                ),
            );
        }
    }
}

fn micros_per_call(n: u32, mut f: impl FnMut()) -> f64 {
    let t = std::time::Instant::now();
    for _ in 0..n {
        f();
    }
    t.elapsed().as_secs_f64() * 1e6 / f64::from(n)
}

/// 64 explosions among 500 fixed frames holding items and 200 equipped armor stands out of
/// reach: a blast touching nothing (the scan alone), and a blast by a mob with `mob_griefing`
/// off in the middle of the frames (in reach, nothing broken: repeatable), each path.
#[test]
#[ignore]
fn bench_explosion_among_decorations() {
    let (mut p, dir) = open("deco-bench-blast");
    fixed_frames(&mut p, 4);
    for k in 0..200 {
        let equipped = "{equipment:{mainhand:{id:\"minecraft:diamond_sword\",count:1},head:{id:\"minecraft:iron_helmet\",count:1}}}";
        summon(&mut p, &format!("minecraft:armor_stand {}.5 -60 {}.5 {equipped}", 10 + k % 100, 30 + k / 100));
    }
    cmd(&mut p, "gamerule mob_griefing false");
    let mob = Some(Ent::Mob(424_242));
    for pass in 0..2 {
        for (label, borrow) in [("copy of every compound", false), ("read in place", true)] {
            p.s.deco.switches.explosion_borrow = borrow;
            let scan = micros_per_call(64, || p.s.deco_explosion([0.5, 100.0, 0.5], 4.0, "minecraft:explosion", None, false, true));
            let near = micros_per_call(64, || p.s.deco_explosion([60.5, -58.0, 8.0], 4.0, "minecraft:explosion", mob, false, true));
            eprintln!("pass {pass}, {label}: blast out of reach {scan:.1} us, blast among frames {near:.1} us");
        }
    }
    finish(p, dir);
}

/// 40 item frames holding a diamond (4 rows, x 56 to 65, wall at z 10) and 20 equipped armor
/// stands in front of them: everything an ordinary blast at (60.5, -58, 8) of radius 4 breaks.
fn breakable_wall(p: &mut Probe) {
    for y in -60..-56 {
        for x in 56..66 {
            summon(
                p,
                &format!(
                    "minecraft:item_frame {x}.5 {y}.5 9.96875 {{Facing:2b,block_pos:[I;{x},{y},9],\
                     Item:{{id:\"minecraft:diamond\",count:1}}}}"
                ),
            );
        }
    }
    let equipped = "{equipment:{mainhand:{id:\"minecraft:diamond_sword\",count:1},head:{id:\"minecraft:iron_helmet\",count:1}}}";
    for k in 0..20 {
        summon(p, &format!("minecraft:armor_stand {}.5 -60 {}.5 {equipped}", 56 + k % 10, 7 + k / 10));
    }
}

/// An ordinary blast (no author) that breaks everything in reach: the wall of
/// [`breakable_wall`] rebuilt before each of the 32 blasts (rebuild and clean-up not timed),
/// each path; microseconds per blast.
#[test]
#[ignore]
fn bench_explosion_breaking_everything() {
    let (mut p, dir) = open("deco-bench-blast-breaking");
    cmd(&mut p, "fill 50 -60 10 70 -55 10 minecraft:stone");
    for pass in 0..3 {
        for (label, borrow) in [("copy of every compound", false), ("read in place", true)] {
            p.s.deco.switches.explosion_borrow = borrow;
            let mut timed = std::time::Duration::ZERO;
            for _ in 0..32 {
                breakable_wall(&mut p);
                let start = std::time::Instant::now();
                p.s.deco_explosion([60.5, -58.0, 8.0], 4.0, "minecraft:explosion", None, false, true);
                timed += start.elapsed();
                assert!(p.s.deco.live.iter().all(|l| l.removed), "the blast left a decoration standing");
                // Drops, packets and removed entries cleared, so every blast starts the same.
                p.s.life.drops.clear();
                p.s.deco_post();
                let _ = p.take_events();
            }
            eprintln!("pass {pass}, {label}: {:.1} us per blast", timed.as_secs_f64() * 1e6 / 32.0);
        }
    }
    finish(p, dir);
}

/// The blast pass over the still compounds among 2,000 markers and 500 frames, none in reach
/// then a blast near a corner of the markers, then one among the frames only (the case where
/// testing the distance first does more work): type tested first (former) or distance first.
#[test]
#[ignore]
fn bench_still_push_distance_first() {
    let (mut p, dir) = open("deco-bench-still");
    fixed_frames(&mut p, 4);
    for k in 0..2000 {
        summon(&mut p, &format!("minecraft:marker {}.5 -60 {}.5", 10 + k % 100, 20 + k / 100));
    }
    for pass in 0..2 {
        for (label, distance_first) in [("type first", false), ("distance first", true)] {
            p.s.deco.switches.still_distance_first = distance_first;
            let far = micros_per_call(200, || p.s.blast_push_still([0.5, 200.0, 0.5], 4.0, 1.0));
            let near = micros_per_call(200, || p.s.blast_push_still([12.5, -59.0, 22.5], 2.0, 0.0));
            let frames = micros_per_call(200, || p.s.blast_push_still([60.5, -58.0, 8.0], 4.0, 0.0));
            eprintln!("pass {pass}, {label}: nothing in reach {far:.1} us, near a corner {near:.1} us, among frames {frames:.1} us");
        }
    }
    finish(p, dir);
}

/// 500 frames and 100 armor stands falling from high up: microseconds per decoration tick
/// (the decoration walk alone), 40 ticks, each lookup lead alone and together.
#[test]
#[ignore]
fn bench_decoration_ticks_with_falling_stands() {
    let paths = [
        ("old", ALL_OLD),
        ("by index", DecoSwitches { by_index: true, ..ALL_OLD }),
        ("slot hint", DecoSwitches { slot_hint: true, ..ALL_OLD }),
        ("both", ALL_NEW),
    ];
    for pass in 0..4 {
        for (label, switches) in paths {
            let (mut p, dir) = open(&format!("deco-bench-ticks-{pass}-{}", label.replace(' ', "-")));
            p.s.deco.switches = switches;
            fixed_frames(&mut p, 4);
            for k in 0..100 {
                summon(&mut p, &format!("minecraft:armor_stand {}.5 300 {}.5", 10 + k, 20 + k % 3));
            }
            let per_tick = micros_per_call(40, || {
                p.s.time += 1;
                p.s.deco_tick();
            });
            let _ = p.take_events();
            eprintln!("pass {pass}, {label}: {per_tick:.1} us per decoration tick");
            finish(p, dir);
        }
    }
}
