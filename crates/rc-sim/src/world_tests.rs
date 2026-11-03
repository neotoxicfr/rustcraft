//! Proofs of the dimension loads (`LoadPlan`) and of the section count: every variant gives the
//! old path's outcome for each chunk (SHA-256 of everything the outcome holds), on the test world
//! and on damaged region files; the deliberate faults are seen.

use super::*;
use crate::region::ReadSabotage;
use rc_proto::nbt::Nbt;
use rc_world::SECTION_BLOCKS;
use std::fmt::Write as _;

/// SHA-256 of a value's full debug text, fed piece by piece (outcomes are large).
fn digest(v: &impl std::fmt::Debug) -> Vec<u8> {
    struct Sha(ring::digest::Context);
    impl std::fmt::Write for Sha {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.update(s.as_bytes());
            Ok(())
        }
    }
    let mut h = Sha(ring::digest::Context::new(&ring::digest::SHA256));
    let _ = write!(h, "{v:?}");
    h.0.finish().as_ref().to_vec()
}

fn digests<T: std::fmt::Debug>(out: &[((i32, i32), T)]) -> Vec<((i32, i32), Vec<u8>)> {
    out.iter().map(|(p, l)| (*p, digest(l))).collect()
}

/// What a split load promises: each position's outcome, and the order of the positions within
/// each region (the order of the regions follows a hash table, old path included).
type SplitOutcome =
    (std::collections::BTreeMap<(i32, i32), Vec<u8>>, std::collections::BTreeMap<(i32, i32), Vec<(i32, i32)>>);

fn split_outcome<T: std::fmt::Debug>(out: &[((i32, i32), T)]) -> SplitOutcome {
    let mut order: std::collections::BTreeMap<(i32, i32), Vec<(i32, i32)>> = Default::default();
    for (p, _) in out {
        order.entry((p.0 >> 5, p.1 >> 5)).or_default().push(*p);
    }
    (digests(out).into_iter().collect(), order)
}

const ALL_READS: ReadPlan =
    ReadPlan { positioned: true, inflate_unlocked: true, libdeflate: true, sabotage: ReadSabotage::None };

/// The old path, each switch alone, then together.
fn plans() -> Vec<(&'static str, LoadPlan)> {
    let read = |positioned, inflate_unlocked, libdeflate| ReadPlan {
        positioned,
        inflate_unlocked,
        libdeflate,
        ..ReadPlan::OLD
    };
    let plan = |read, shared_handle, handle_cache| LoadPlan { read, shared_handle, handle_cache };
    vec![
        ("old", LoadPlan::OLD),
        ("positioned reads", plan(read(true, false, false), false, false)),
        ("inflate unlocked", plan(read(false, true, false), false, false)),
        ("libdeflate", plan(read(false, false, true), false, false)),
        ("shared handle", plan(ReadPlan::OLD, true, false)),
        ("handle cache", plan(ReadPlan::OLD, true, true)),
        ("all but the cache", plan(ALL_READS, true, false)),
        ("all", plan(ALL_READS, true, true)),
    ]
}

fn setup() -> Option<(PathBuf, VersionData, States)> {
    let world = crate::tests::test_world()?;
    let data = rc_testdata::version_data()?;
    let biomes: Vec<String> = data.biomes.iter().map(|b| b.name.clone()).collect();
    let states = States::new(&data, biomes);
    Some((world, data, states))
}

/// Every chunk of the three dimensions: same outcome for every plan, in the same order.
#[test]
fn load_everything_same_for_every_plan() {
    let Some((world, data, states)) = setup() else { return };
    for name in crate::tests::DIMENSIONS {
        let dim = Dimension::new(&world, name, crate::dimension_height(&data, name));
        let reference = digests(&dim.load_everything_with(&data, &states, LoadPlan::OLD));
        assert!(!reference.is_empty() || name != crate::dims::OVERWORLD, "no chunk read");
        for (label, plan) in plans() {
            let got = digests(&dim.load_everything_with(&data, &states, plan));
            assert!(got == reference, "{name}, {label}: outcomes differ");
        }
        eprintln!("{name}: {} chunks, same outcome for {} plans", reference.len(), plans().len());
    }
}

/// Lists of chunks (present, absent, in a missing region file) through the three folders, in
/// tasks of 1, 4 or a whole region: same outcomes for every plan; the same dimension reused (the
/// kept files serve the second load).
#[test]
fn load_split_same_for_every_plan() {
    let Some((world, data, states)) = setup() else { return };
    let h = crate::dimension_height(&data, crate::dims::OVERWORLD);
    let dim = Dimension::new(&world, crate::dims::OVERWORLD, h);
    let (rx, rz) = dim.regions()[0];
    let mut list: Vec<(i32, i32)> = Vec::new();
    for dz in -40..40 {
        for dx in -40..40 {
            list.push(((rx << 5) + dx, (rz << 5) + dz));
        }
    }
    list.push((100_000, 100_000));
    for per_task in [1, 4, usize::MAX] {
        let reference = split_outcome(&dim.load_split_with(&list, &data, &states, per_task, LoadPlan::OLD));
        let raw = |plan| {
            ["entities", "poi", "region"].map(|f| split_outcome(&dim.load_raw_split_with(f, &list, per_task, plan)))
        };
        let raw_reference = raw(LoadPlan::OLD);
        assert!(reference.0.values().filter(|d| **d != digest(&Load::Absent)).count() > 100, "too few chunks");
        for (label, plan) in plans() {
            for round in 0..2 {
                let got = split_outcome(&dim.load_split_with(&list, &data, &states, per_task, plan));
                assert!(got == reference, "{label}, tasks of {per_task}, round {round}: outcomes differ");
                assert!(raw(plan) == raw_reference, "{label}, tasks of {per_task}: raw compounds differ");
            }
        }
    }
    dim.close_region_files();
    assert_eq!(dim.region_files.open_count(), 0);
}

/// A world folder whose region files hold every damaged case (`region::tests`): same outcome
/// (absent, skipped, unreadable and its text) for every plan, in the block and raw folders.
#[test]
fn damaged_regions_load_the_same() {
    let Some(data) = rc_testdata::version_data() else { return };
    let states = States::new(&data, Vec::new());
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-world-damaged-{}", std::process::id())));
    let dim = Dimension::new(&world, crate::dims::OVERWORLD, crate::dimension_height(&data, crate::dims::OVERWORLD));
    let mut chunks = Vec::new();
    for folder in [dim.region_dir(), dim.dir.join("entities")] {
        std::fs::create_dir_all(&folder).unwrap();
        chunks = crate::region::tests::write_damaged_regions(&folder);
    }
    let reference = split_outcome(&dim.load_split_with(&chunks, &data, &states, 4, LoadPlan::OLD));
    let raw_reference = split_outcome(&dim.load_raw_split_with("entities", &chunks, 4, LoadPlan::OLD));
    let everything = digests(&dim.load_everything_with(&data, &states, LoadPlan::OLD));
    for (label, plan) in plans() {
        assert!(split_outcome(&dim.load_split_with(&chunks, &data, &states, 4, plan)) == reference, "{label}");
        assert!(split_outcome(&dim.load_raw_split_with("entities", &chunks, 4, plan)) == raw_reference, "{label}: raw");
        assert!(digests(&dim.load_everything_with(&data, &states, plan)) == everything, "{label}: everything");
    }
    let unreadable = dim
        .load_split_with(&chunks, &data, &states, 4, LoadPlan::OLD)
        .into_iter()
        .filter(|(_, l)| matches!(l, Load::Unreadable(_)))
        .count();
    assert!(unreadable >= 5, "the damaged cases must reach the loads ({unreadable} unreadable)");
}

/// The kept files see a chunk the write thread moved after their opening (slot read again);
/// with the slot of the opening (sabotage), the old version comes back.
#[test]
fn kept_files_see_rewritten_chunks() {
    let Some(data) = rc_testdata::version_data() else { return };
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-world-kept-{}", std::process::id())));
    let dim = Dimension::new(&world, crate::dims::OVERWORLD, crate::dimension_height(&data, crate::dims::OVERWORLD));
    let folder = dim.dir.join("entities");
    std::fs::create_dir_all(&folder).unwrap();
    let compound =
        |v: i32| Nbt::Compound(vec![("version".into(), Nbt::Int(v)), ("pad".into(), Nbt::IntArray(vec![v; 3000]))]);
    let mut w = RegionFile::open_or_create(&folder.join("r.0.0.mca")).unwrap();
    for i in 0..8 {
        w.write(i, i as i32, 0, &crate::nbtio::encode_root(&compound(1))).unwrap();
    }
    let list: Vec<(i32, i32)> = (0..8).map(|i| (i, 0)).collect();
    let kept = LoadPlan { read: ALL_READS, shared_handle: true, handle_cache: true };
    let stale = LoadPlan { read: ReadPlan { sabotage: ReadSabotage::StaleSlot, ..ALL_READS }, ..kept };
    let read = |plan| dim.load_raw_split_with("entities", &list, 4, plan);
    assert!(read(kept).iter().all(|(_, n)| n.as_ref() == Some(&compound(1))));
    assert_eq!(dim.region_files.open_count(), 1, "file kept after the first load");
    // Rewritten larger (moved), then a neighbour written over the freed sectors.
    w.write(3, 3, 0, &crate::nbtio::encode_root(&compound(2))).unwrap();
    w.write(9, 9, 0, &crate::nbtio::encode_root(&compound(9))).unwrap();
    let after = read(kept);
    assert_eq!(after[3].1.as_ref(), Some(&compound(2)), "kept file missed the rewrite");
    assert!(read(stale)[3].1.as_ref() != Some(&compound(2)), "slot of the opening (sabotage) not seen");
    dim.close_region_files();
    assert_eq!(dim.region_files.open_count(), 0);
    w.close().unwrap();
}

/// Disk sections of the count proofs: every palette shape and damage the block read handles.
fn section_cases(data: &VersionData, states: &States) -> Vec<(&'static str, Option<Nbt>)> {
    let pal = |names: &[u32]| Nbt::List(names.iter().map(|&s| states.palette_entry(data, s)).collect());
    let stone = data.blocks.iter().find(|b| b.name == "minecraft:stone").unwrap().default_state;
    let air = states.air_state;
    let solid: Vec<u32> =
        (0..400u32).map(|i| (i * 37 + 1) % data.states.len() as u32).filter(|&s| !states.is_air(s)).collect();
    let packed = |bits: u8, n: usize, seed: u64, outside: Option<u64>| -> Nbt {
        let vpl = 64 / bits as usize;
        let mut s = seed;
        let mut data = vec![0i64; SECTION_BLOCKS.div_ceil(vpl)];
        for i in 0..SECTION_BLOCKS {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let mut v = (s >> 33) % n as u64;
            if i == 2000 {
                v = outside.unwrap_or(v);
            }
            data[i / vpl] |= (v << ((i % vpl) * bits as usize)) as i64;
        }
        Nbt::LongArray(data)
    };
    let section = |palette: Nbt, data: Option<Nbt>| {
        let mut f = vec![("palette".to_owned(), palette)];
        if let Some(d) = data {
            f.push(("data".to_owned(), d));
        }
        Some(Nbt::Compound(f))
    };
    let mixed16: Vec<u32> = (0..16).map(|i| if i % 3 == 0 { air } else { solid[i] }).collect();
    let mixed17: Vec<u32> = (0..17).map(|i| if i % 4 == 0 { air } else { solid[i] }).collect();
    let mixed300: Vec<u32> = (0..300).map(|i| if i % 5 == 0 { air } else { solid[i] }).collect();
    let mut unknown = vec![states.palette_entry(data, stone)];
    unknown.push(Nbt::String("minecraft:no_such_block".into()));
    vec![
        ("absent", None),
        ("single", section(pal(&[stone]), None)),
        ("single air", section(pal(&[air]), None)),
        ("empty palette", section(Nbt::List(Vec::new()), None)),
        ("data missing", section(pal(&[air, stone]), None)),
        ("data of the wrong size", section(pal(&[air, stone]), Some(Nbt::LongArray(vec![0; 10])))),
        ("two entries", section(pal(&[air, stone]), Some(packed(4, 2, 1, None)))),
        ("two entries, index outside", section(pal(&[air, stone]), Some(packed(4, 2, 1, Some(7))))),
        ("full palette, no air", section(pal(&solid[..16]), Some(packed(4, 16, 2, None)))),
        ("full palette, all air", section(pal(&[air; 16]), Some(packed(4, 16, 3, None)))),
        ("full palette, mixed", section(pal(&mixed16), Some(packed(4, 16, 4, None)))),
        ("17 entries", section(pal(&mixed17), Some(packed(5, 17, 5, None)))),
        ("17 entries, index outside", section(pal(&mixed17), Some(packed(5, 17, 5, Some(31))))),
        ("300 entries", section(pal(&mixed300), Some(packed(9, 300, 6, None)))),
        ("300 entries, index outside", section(pal(&mixed300), Some(packed(9, 300, 6, Some(400))))),
        ("unknown block", section(Nbt::List(unknown), Some(packed(4, 2, 7, None)))),
    ]
}

type Counted = Result<(rc_world::Container, i16), String>;

/// Old block read: index check, then a separate count.
fn counted_old(c: Option<&Nbt>, data: &VersionData, states: &States, lossy: &mut bool) -> Counted {
    let b = chunk::container(c, SECTION_BLOCKS, true, states.air_state, |e| states.state_of(data, e), lossy)?;
    let n = chunk::count_non_empty(&b, states);
    Ok((b, n))
}

/// Names of the cases where `counted` differs from the old read (outcome or `lossy`).
fn count_differences(
    data: &VersionData,
    states: &States,
    counted: &dyn Fn(Option<&Nbt>, &mut bool) -> Counted,
) -> Vec<&'static str> {
    section_cases(data, states)
        .into_iter()
        .filter(|(_, c)| {
            let (mut a, mut b) = (false, false);
            (counted_old(c.as_ref(), data, states, &mut a), a) != (counted(c.as_ref(), &mut b), b)
        })
        .map(|(name, _)| name)
        .collect()
}

/// The count taken in the index check = the old check then count, on every palette shape and
/// damage, and on every section of the test world; a count that skips the check, or counts the
/// air, is seen.
#[test]
fn section_count_same_both_ways() {
    let Some(data) = rc_testdata::version_data() else { return };
    let states = States::new(&data, Vec::new());
    let new = |c: Option<&Nbt>, lossy: &mut bool| {
        chunk::block_container_counted(c, states.air_state, |e| states.state_of(&data, e), lossy, &states)
    };
    assert_eq!(count_differences(&data, &states, &new), Vec::<&str>::new());
    let damage_read_as_air = |c: Option<&Nbt>, lossy: &mut bool| -> Counted {
        // Sabotage: a damaged section read as empty instead of refused.
        new(c, lossy).or(Ok((rc_world::Container::Single(states.air_state), 0)))
    };
    let counts_air = |c: Option<&Nbt>, lossy: &mut bool| -> Counted {
        // Sabotage: the air counted instead.
        new(c, lossy).map(|(b, n)| (b, SECTION_BLOCKS as i16 - n))
    };
    assert!(!count_differences(&data, &states, &damage_read_as_air).is_empty(), "damage read as air not seen");
    assert!(!count_differences(&data, &states, &counts_air).is_empty(), "air counted not seen");
    let Some((world, data, states)) = setup() else { return };
    let mut n = 0;
    for name in crate::tests::DIMENSIONS {
        let dim = Dimension::new(&world, name, crate::dimension_height(&data, name));
        for (rx, rz) in dim.regions() {
            let Ok(mut f) = RegionFile::open(&dim.region_dir().join(format!("r.{rx}.{rz}.mca")), false) else {
                continue;
            };
            let present: Vec<usize> = f.present().collect();
            for i in present {
                let (x, z) = ((rx << 5) + (i & 31) as i32, (rz << 5) + (i >> 5) as i32);
                let Ok(Some(bytes)) = f.read(i, x, z) else { continue };
                let Ok(root) = crate::nbtio::parse_root(&bytes) else { continue };
                let one = chunk::parse_with(root.clone(), &data, &states, dim.height, true);
                let two = chunk::parse_with(root, &data, &states, dim.height, false);
                assert!(digest(&one) == digest(&two), "{name} ({x}, {z})");
                n += 1;
            }
        }
    }
    eprintln!("{n} chunks parsed the same both ways");
    assert!(n > 0);
}

/// Thread counts of the benches: 1, then powers of 2 up to the logical cores.
fn thread_counts() -> Vec<usize> {
    let all = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut v: Vec<usize> = std::iter::successors(Some(1usize), |n| Some(n * 2)).take_while(|&n| n < all).collect();
    v.push(all);
    v
}

fn bench_world() -> Option<PathBuf> {
    rc_data::env_os!("RUSTCRAFT_OPT_WORLD").map(PathBuf::from).or_else(crate::tests::test_world)
}

/// Micro-bench (measurement, not a proof): every chunk of the bench world's overworld
/// (`RUSTCRAFT_OPT_WORLD`, otherwise the test world), per plan and thread count, 2 passes.
#[test]
#[ignore = "measurement bench"]
fn bench_load_everything() {
    let (Some(world), Some(data)) = (bench_world(), rc_testdata::version_data()) else { return };
    let biomes: Vec<String> = data.biomes.iter().map(|b| b.name.clone()).collect();
    let states = States::new(&data, biomes);
    let dim = Dimension::new(&world, crate::dims::OVERWORLD, crate::dimension_height(&data, crate::dims::OVERWORLD));
    let _ = dim.load_everything_with(&data, &states, LoadPlan::OLD); // file cache warmed
    for threads in thread_counts() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        for pass in 0..2 {
            for (label, plan) in plans().into_iter().filter(|(l, _)| *l != "handle cache") {
                let t = std::time::Instant::now();
                let n = pool.install(|| dim.load_everything_with(&data, &states, plan)).len();
                eprintln!(
                    "bench_load_everything {threads} threads, pass {pass}, {label}: {n} chunks, {:.1} ms",
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
        }
    }
}

/// Micro-bench (measurement, not a proof): the loader's way (slices of 32 chunks, tasks of 4,
/// the three folders) over 41 × 41 chunks of the bench world, per plan and thread count.
#[test]
#[ignore = "measurement bench"]
fn bench_load_slices() {
    let (Some(world), Some(data)) = (bench_world(), rc_testdata::version_data()) else { return };
    let biomes: Vec<String> = data.biomes.iter().map(|b| b.name.clone()).collect();
    let states = States::new(&data, biomes);
    let h = crate::dimension_height(&data, crate::dims::OVERWORLD);
    let probe = Dimension::new(&world, crate::dims::OVERWORLD, h);
    let mut regions = probe.regions();
    regions.sort_unstable();
    let (rx, rz) = regions[regions.len() / 2];
    let mut list = Vec::new();
    for dz in -20..=20 {
        for dx in -20..=20 {
            list.push(((rx << 5) + 16 + dx, (rz << 5) + 16 + dz));
        }
    }
    for threads in thread_counts() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        for pass in 0..2 {
            for (label, plan) in plans() {
                let dim = Dimension::new(&world, crate::dims::OVERWORLD, h);
                let t = std::time::Instant::now();
                let mut n = 0;
                pool.install(|| {
                    for slice in list.chunks(32) {
                        n += dim.load_split_with(slice, &data, &states, 4, plan).len();
                        n += dim.load_raw_split_with("entities", slice, 4, plan).len();
                        n += dim.load_raw_split_with("poi", slice, 4, plan).len();
                    }
                });
                eprintln!(
                    "bench_load_slices {threads} threads, pass {pass}, {label}: {n} reads, {:.1} ms",
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
        }
    }
}

/// Micro-bench (measurement, not a proof): block containers of every section of the bench
/// world, index check then count apart, against the count taken in the check; one thread.
#[test]
#[ignore = "measurement bench"]
fn bench_section_count() {
    use std::time::Instant;
    let (Some(world), Some(data)) = (bench_world(), rc_testdata::version_data()) else { return };
    let states = States::new(&data, Vec::new());
    let dim = Dimension::new(&world, crate::dims::OVERWORLD, crate::dimension_height(&data, crate::dims::OVERWORLD));
    let mut sections: Vec<Nbt> = Vec::new();
    for (_, nbt) in dim.load_raw_split_with("region", &all_positions(&dim), usize::MAX, LoadPlan::OLD) {
        let Some(root) = nbt else { continue };
        if root.get("Status").and_then(Nbt::as_str) != Some("minecraft:full") {
            continue;
        }
        sections.extend(crate::nbtio::list(root.get("sections")).iter().filter_map(|s| s.get("block_states").cloned()));
    }
    for pass in 0..3 {
        for one_pass in [false, true] {
            let t = Instant::now();
            let mut total = 0i64;
            for s in &sections {
                let mut lossy = false;
                let r = if one_pass {
                    chunk::block_container_counted(
                        Some(s),
                        states.air_state,
                        |e| states.state_of(&data, e),
                        &mut lossy,
                        &states,
                    )
                } else {
                    counted_old(Some(s), &data, &states, &mut lossy)
                };
                total += r.map_or(0, |(_, n)| i64::from(n));
            }
            eprintln!(
                "bench_section_count pass {pass} ({}): {} sections, {total} blocks, {:.1} ms",
                if one_pass { "in the check" } else { "apart" },
                sections.len(),
                t.elapsed().as_secs_f64() * 1e3
            );
        }
    }
}

/// Every chunk position of the dimension's region files.
fn all_positions(dim: &Dimension) -> Vec<(i32, i32)> {
    dim.regions()
        .into_iter()
        .flat_map(|(rx, rz)| (0..1024).map(move |i| ((rx << 5) + (i & 31), (rz << 5) + (i >> 5))))
        .collect()
}

/// Micro-bench (measurement, not a proof): where the time of a chunk read goes, one thread:
/// file reads (old and positioned), inflate (flate2, libdeflate), NBT, chunk parse (count apart
/// or in the check), and the palette lookups (`state_of`) alone.
#[test]
#[ignore = "measurement bench"]
fn bench_load_stages() {
    use std::time::Instant;
    let (Some(world), Some(data)) = (bench_world(), rc_testdata::version_data()) else { return };
    let biomes: Vec<String> = data.biomes.iter().map(|b| b.name.clone()).collect();
    let states = States::new(&data, biomes);
    let dim = Dimension::new(&world, crate::dims::OVERWORLD, crate::dimension_height(&data, crate::dims::OVERWORLD));
    let mut files: Vec<(RegionFile, i32, i32)> = dim
        .regions()
        .into_iter()
        .filter_map(|(rx, rz)| {
            Some((RegionFile::open(&dim.region_dir().join(format!("r.{rx}.{rz}.mca")), false).ok()?, rx, rz))
        })
        .collect();
    let chunks: usize = files.iter().map(|f| f.0.present().count()).sum();
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    for pass in 0..2 {
        let mut bytes = Vec::new();
        for plan in [ReadPlan::OLD, ReadPlan { positioned: true, inflate_unlocked: true, ..ReadPlan::OLD }] {
            let t = Instant::now();
            bytes.clear();
            for (f, rx, rz) in &mut files {
                f.read_plan = ReadPlan { libdeflate: false, ..plan };
                let present: Vec<usize> = f.present().collect();
                for i in present {
                    let (x, z) = ((*rx << 5) + (i & 31) as i32, (*rz << 5) + (i >> 5) as i32);
                    if let Ok(Some(b)) = f.read(i, x, z) {
                        bytes.push(b);
                    }
                }
            }
            eprintln!(
                "bench_stages pass {pass}: read + flate2 inflate ({}): {chunks} chunks, {:.1} ms",
                if plan.positioned { "positioned" } else { "old" },
                ms(t)
            );
        }
        let streams: Vec<Vec<u8>> = bytes.iter().map(|b| crate::nbtio::zlib(b)).collect();
        for libdeflate in [false, true] {
            let t = Instant::now();
            let n: usize = streams.iter().map(|s| crate::region::tests::inflate_len(s, libdeflate)).sum();
            eprintln!(
                "bench_stages pass {pass}: inflate {} ({n} bytes): {:.1} ms",
                if libdeflate { "libdeflate" } else { "flate2" },
                ms(t)
            );
        }
        let t = Instant::now();
        let roots: Vec<Nbt> = bytes.iter().filter_map(|b| crate::nbtio::parse_root(b).ok()).collect();
        eprintln!("bench_stages pass {pass}: NBT parse: {:.1} ms", ms(t));
        for one_pass in [false, true] {
            let t = Instant::now();
            let ok = roots
                .iter()
                .filter(|r| chunk::parse_with((*r).clone(), &data, &states, dim.height, one_pass).is_ok())
                .count();
            eprintln!(
                "bench_stages pass {pass}: chunk parse (count {}): {ok} chunks, {:.1} ms (root clone included)",
                if one_pass { "in the check" } else { "apart" },
                ms(t)
            );
        }
        let t = Instant::now();
        let clones: usize = roots.iter().map(|r| usize::from(matches!(r.clone(), Nbt::Compound(_)))).sum();
        eprintln!("bench_stages pass {pass}: root clone alone ({clones}): {:.1} ms", ms(t));
        let entries: Vec<&Nbt> = roots
            .iter()
            .flat_map(|r| crate::nbtio::list(r.get("sections")))
            .flat_map(|s| crate::nbtio::list(s.get("block_states").and_then(|b| b.get("palette"))))
            .collect();
        let t = Instant::now();
        let found = entries.iter().filter(|e| states.state_of(&data, e).is_some()).count();
        eprintln!("bench_stages pass {pass}: state_of: {} entries ({found} found), {:.1} ms", entries.len(), ms(t));
    }
}
