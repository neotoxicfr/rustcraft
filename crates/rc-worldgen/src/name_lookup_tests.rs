//! Equivalence of the allocation-free name lookups (`RUSTCRAFT_OPT_TAG_BORROW`), of the properties
//! by index (`RUSTCRAFT_OPT_PROP_IDX`) and of the shape updates by precomputed families
//! (`RUSTCRAFT_OPT_SHAPE_KEYS`) with their old paths. Game data absent: test skipped,
//! failure under `RUSTCRAFT_OPT_REQUIRE=1`.

use crate::data::{full_id, with_file_key_mode, with_full_id_mode};
use crate::decor_blocks::{Blocks, Dir, Prop, State};
use crate::decor_data::DecorData;
use crate::decor_region::{DecoChunk, Region, add};

fn blocks(prop_idx: bool) -> Option<Blocks> {
    let jar = rc_testdata::need(
        rc_data::locate("26.3").ok().map(|i| i.client_jar).filter(|p| p.is_file()),
        "jar client 26.3",
    )?;
    let v = rc_testdata::version_data()?;
    let data = DecorData::from_jar(&jar).expect("decoration data");
    let mut b = Blocks::new(&v, &data).expect("blocks");
    b.opt_prop_idx = prop_idx;
    Some(b)
}

/// Deterministic test randomness (xorshift 64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn full_identifier_borrowed_identical_to_the_old() {
    let long = "x".repeat(200);
    let ids = ["leaves", "minecraft:leaves", "c:ores", "", ":", "é_à", "a:b:c", "#leaves", long.as_str()];
    for id in ids {
        let old = with_full_id_mode(id, false, str::to_owned);
        assert_eq!(with_full_id_mode(id, true, str::to_owned), old, "{id}");
        assert_eq!(old, full_id(id));
        for kind in ["tags_block", "template", "worldgen/placed_feature", long.as_str()] {
            let old = with_file_key_mode(kind, id, false, str::to_owned);
            assert_eq!(with_file_key_mode(kind, id, true, str::to_owned), old, "{kind} {id}");
        }
    }
    // Expected values stored.
    assert_eq!(with_full_id_mode("oak_log", true, str::to_owned), "minecraft:oak_log");
    assert_eq!(with_file_key_mode("template", "igloo/top", true, str::to_owned), "template/minecraft:igloo/top");
}

#[test]
fn borrowed_tags_identical_to_the_old() {
    let Some(b) = blocks(true) else { return };
    let names = b.tag_names();
    assert!(names.len() > 200, "too few tags: {}", names.len());
    let mut found = 0;
    for full in names {
        let short = full.strip_prefix("minecraft:").unwrap_or(full);
        for n in [full.to_owned(), short.to_owned(), format!("#{full}"), format!("#{short}"), format!("{short}_x")] {
            let old = b.tag_mode(&n, false);
            assert_eq!(b.tag_mode(&n, true), old, "tag {n}");
            assert_eq!(b.fluid_tag_mode(&n, true), b.fluid_tag_mode(&n, false), "fluid tag {n}");
            found += old.is_some() as usize;
        }
    }
    assert!(found > 800, "too few tags found: {found}");
    assert!(b.tag_mode("leaves", true).is_some() && b.tag_mode("leaves", true) == b.tag_mode("minecraft:leaves", false));
    assert!(b.fluid_tag_mode("water", true) == Some(3), "fluid tag water: {:?}", b.fluid_tag_mode("water", true));
}

/// FNV-1a 64 fingerprint of a sequence of values, to store an expected result: the code common
/// to both paths (property write, decoration rules) cannot change without breaking it.
fn fnv(values: impl IntoIterator<Item = u64>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in values {
        for b in v.to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Expected fingerprints on the 26.3 data (taken on the by-name path, equal to the new one).
const PROP_FNV: u64 = 0xfd0f_9720_3d92_646b;
/// G6-F23: the "up" face of the vines reads the top face of the block above (fidelity); only
/// this line changed the fingerprint from 0x1c92_9594_7b38_ddba to the value below.
const SHAPE_FNV: u64 = 0x874a_f273_dc0c_34f2;

/// All the index-based reads and writes of a state, for every property (and an absent one).
fn prop_results(b: &Blocks) -> Vec<u64> {
    let mut out = Vec::new();
    let props: Vec<Prop> = (0..b.prop_count()).map(|i| Prop(i as u8)).chain([Prop(u8::MAX)]).collect();
    for s in 0..b.state_count() as State {
        for &p in &props {
            let has = b.has(s, p);
            if !has && p.0 != u8::MAX && !(s as usize + p.0 as usize).is_multiple_of(7) {
                // Absent: one sample is enough (same branch for all).
                out.push(0);
                continue;
            }
            out.push(has as u64 | (b.get_bool(s, p).map_or(2, |v| v as u64) << 1) | (b.get_dir(s, p).map_or(9, |d| d as u64) << 3));
            out.push(b.with_bool(s, p, true) as u64 | (b.with_bool(s, p, false) as u64) << 32);
            for d in Dir::ALL {
                out.push(b.with_dir(s, p, d) as u64);
            }
            for a in 0..3 {
                out.push(b.with_axis(s, p, a) as u64);
            }
        }
    }
    out
}

#[test]
fn index_properties_identical_to_the_old() {
    let Some(old) = blocks(false) else { return };
    let Some(new) = blocks(true) else { return };
    assert!(old.state_count() > 20_000 && old.prop_count() > 50);
    let (a, c) = (prop_results(&old), prop_results(&new));
    assert_eq!(a.len(), c.len());
    if let Some(i) = a.iter().zip(&c).position(|(x, y)| x != y) {
        panic!("difference at entry {i}: old {:#x}, new {:#x}", a[i], c[i]);
    }
    assert_eq!(fnv(a.iter().copied()), PROP_FNV, "fingerprint of the properties ({} values)", a.len());
    // Useful reads in number (otherwise the test would prove nothing).
    let bools = (0..old.state_count() as State).filter(|&s| old.get_bool(s, old.p.waterlogged) == Some(true)).count();
    assert!(bools > 1000, "too few waterlogged=true states: {bools}");
}

const P: [i32; 3] = [8, 8, 8];

/// Test case: state tested, 27 cells around (order x, y, z from −1 to 1), six neighbours passed
/// as arguments (outside the cells).
struct Case {
    s: State,
    cells: [State; 27],
    extra: [State; 6],
}

fn cases(b: &Blocks) -> Vec<Case> {
    let n = b.state_count() as State;
    let family: Vec<State> = (0..n).filter(|&s| b.shape.in_family(b.block(s))).collect();
    assert!(family.len() > 5000, "too few states of the families: {}", family.len());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let pick = |rng: &mut Rng| if rng.below(3) != 0 { family[rng.below(family.len())] } else { rng.below(n as usize) as State };
    let mut out = Vec::new();
    for s in 0..n {
        let trials = if b.shape.in_family(b.block(s)) { 4 } else { 1 };
        for _ in 0..trials {
            let cells = std::array::from_fn(|_| pick(&mut rng));
            let extra = std::array::from_fn(|_| pick(&mut rng));
            out.push(Case { s, cells, extra });
        }
    }
    out
}

/// Results of the updates of each case (neighbour read in the region, then neighbour imposed).
fn shape_results(b: &Blocks, keyed: bool, cases: &[Case]) -> Vec<State> {
    let mut chunks: Vec<DecoChunk> = (0..9).map(|k| DecoChunk::new(k % 3 - 1, k / 3 - 1, 0, 32)).collect();
    let arr: [&mut DecoChunk; 9] = {
        let mut it = chunks.iter_mut();
        std::array::from_fn(|_| it.next().unwrap())
    };
    let mut r = Region::new(arr, b, 4, 1);
    let mut out = Vec::with_capacity(cases.len() * 12);
    for c in cases {
        for (i, &st) in c.cells.iter().enumerate() {
            let q = [P[0] + i as i32 / 9 - 1, P[1] + i as i32 / 3 % 3 - 1, P[2] + i as i32 % 3 - 1];
            r.set_p(q, st, 2);
        }
        r.set_p(P, c.s, 2);
        for (k, d) in Dir::ALL.into_iter().enumerate() {
            for n in [r.get_p(add(P, d)), c.extra[k]] {
                out.push(if keyed {
                    crate::shape_update::update_keyed(&r, c.s, P, d, n)
                } else {
                    crate::shape_update::update_by_name(&r, c.s, P, d, n)
                });
            }
        }
    }
    out
}

#[test]
fn shape_updates_by_families_identical_to_the_old() {
    let Some(old) = blocks(false) else { return };
    let Some(new) = blocks(true) else { return };
    let cs = cases(&old);
    let reference = shape_results(&old, false, &cs);
    let changed = cs.iter().enumerate().filter(|(i, c)| reference[i * 12..i * 12 + 12].iter().any(|&x| x != c.s)).count();
    assert!(changed > 20_000, "too few updates that change the state: {changed}");
    // Blocks declared inert (`RUSTCRAFT_OPT_TREE_FACESKIP`): no update changes them.
    let mut inert = 0;
    for (i, c) in cs.iter().enumerate() {
        if old.shape.inert(old.block(c.s)) {
            inert += 1;
            if let Some(k) = (0..12).find(|&k| reference[i * 12 + k] != c.s) {
                panic!("{} declared inert, changed to {} (direction {:?})", old.describe(c.s), old.describe(reference[i * 12 + k]), Dir::ALL[k / 2]);
            }
        }
    }
    assert!(inert > 10_000, "too few inert states: {inert}");
    let print = fnv(reference.iter().map(|&s| u64::from(s)));
    assert_eq!(print, SHAPE_FNV, "fingerprint of the shapes ({} values)", reference.len());
    for (label, b) in [("families, properties by name", &old), ("families, properties by index", &new)] {
        let got = shape_results(b, true, &cs);
        if let Some(i) = reference.iter().zip(&got).position(|(x, y)| x != y) {
            let c = &cs[i / 12];
            panic!(
                "{label}: difference for {} direction {:?}: old {}, new {}",
                old.describe(c.s),
                Dir::ALL[i % 12 / 2],
                old.describe(reference[i]),
                old.describe(got[i])
            );
        }
    }
    // Old shape path with the properties by index: same result too.
    assert!(shape_results(&new, false, &cs) == reference, "old shape path, properties by index: difference");
}
