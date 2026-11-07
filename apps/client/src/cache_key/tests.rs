use std::hash::{Hash, Hasher};

use hashbrown::HashMap;

use super::*;

/// Render identity lists like the ones the item models fill: a node address, then glint and
/// layer markers. `n` distinct lists, each of 3 to 8 words.
fn identity_lists(n: u64) -> Vec<Vec<u64>> {
    (0..n)
        .map(|i| {
            let mut v = vec![0x7ff6_1234_0000 + i * 0x40];
            v.extend((0..i % 6).map(|k| k + 1));
            v.push(i & 1);
            v
        })
        .collect()
}

/// The keys an atlas would be asked for over `frames` frames of an open inventory (46 slots,
/// some stacks repeated): one cell per key, as the item atlas allocates them.
fn atlas_cells(key: fn(&[u64]) -> u64, frames: usize) -> HashMap<u64, Vec<u64>> {
    let lists = identity_lists(30);
    let mut cells: HashMap<u64, Vec<u64>> = HashMap::new();
    for _ in 0..frames {
        for slot in 0..46 {
            let identity = &lists[slot % lists.len()];
            let cell = cells.entry(key(identity)).or_insert_with(|| identity.clone());
            assert_eq!(cell, identity, "two different renders share an atlas cell");
        }
    }
    cells
}

#[test]
fn same_value_gives_the_same_key_on_every_call() {
    let identity = [0x7ff6_1234_5678u64, 1, 0];
    // The same list built anew, as each frame's model evaluation does.
    let rebuilt = vec![0x7ff6_1234_5678u64, 1, 0];
    for key in [cache_key::<[u64]>, fast_cache_key::<[u64]>, sip_cache_key::<[u64]>] {
        assert_eq!(key(&identity), key(&identity));
        assert_eq!(key(&identity), key(&rebuilt));
    }
    // Every item slot (hotbar, menus, free item, debug switcher) folds its list the same way.
    assert_eq!(item_layers_key(&identity), item_layers_key(&rebuilt));
}

#[test]
fn fast_key_uses_a_fixed_seed() {
    // A seed drawn per process or per call changes this value.
    assert_eq!(fast_cache_key(&[1u64, 2, 3][..]), 0x0cf5_e720_c3dd_44bb);
}

#[test]
#[allow(clippy::disallowed_types, reason = "the former code is the reference of this test")]
fn sip_key_matches_the_former_code() {
    let identity = vec![0x7ff6_1234_5678u64, 3, 1];
    let mut former = std::hash::DefaultHasher::new();
    identity.hash(&mut former);
    assert_eq!(sip_cache_key(&identity), former.finish());
}

#[test]
fn atlas_cells_stay_stable_over_600_frames() {
    for key in [fast_cache_key::<[u64]>, sip_cache_key::<[u64]>] {
        assert_eq!(atlas_cells(key, 600).len(), 30, "one cell per distinct render, frame after frame");
    }
}

#[test]
fn distinct_renders_get_distinct_keys() {
    let lists = identity_lists(20_000);
    let keys: hashbrown::HashSet<u64> = lists.iter().map(|l| fast_cache_key(&l[..])).collect();
    assert_eq!(keys.len(), lists.len());
}

/// Nanoseconds per call of `f` over `items` (best of 7 rounds).
fn nanos_per_call<T>(items: &[T], mut f: impl FnMut(&T) -> u64) -> f64 {
    (0..7)
        .map(|_| {
            let t0 = std::time::Instant::now();
            let mut acc = 0u64;
            for _ in 0..200 {
                for item in items {
                    acc ^= f(std::hint::black_box(item));
                }
            }
            std::hint::black_box(acc);
            t0.elapsed().as_secs_f64() * 1e9 / (200 * items.len()) as f64
        })
        .fold(f64::MAX, f64::min)
}

/// Micro-bench (ignored; `cargo test --release -p rc-app bench_cache_key -- --ignored --nocapture`):
/// one key per item slot drawn, SipHash against foldhash. A frame keys a few dozen slots on the
/// main thread: one core only, nothing to spread over threads or the GPU.
#[test]
#[ignore]
fn bench_cache_key() {
    static SHARED_PROCESS_SEED: std::sync::LazyLock<hashbrown::DefaultHashBuilder> =
        std::sync::LazyLock::new(hashbrown::DefaultHashBuilder::default);
    let lists = identity_lists(64);
    let rows: [(&str, f64); 4] = [
        ("SipHash-1-3 (former)", nanos_per_call(&lists, |l| sip_cache_key(&l[..]))),
        ("foldhash quality, fixed seed (kept)", nanos_per_call(&lists, |l| fast_cache_key(&l[..]))),
        (
            "foldhash fast, fixed seed",
            nanos_per_call(&lists, |l| foldhash::fast::FixedState::with_seed(CACHE_KEY_SEED).hash_one(&l[..])),
        ),
        ("hashbrown default builder, one per process", nanos_per_call(&lists, |l| SHARED_PROCESS_SEED.hash_one(&l[..]))),
    ];
    for (name, ns) in rows {
        println!("{name:<45} {ns:>7.2} ns per key");
    }
}
