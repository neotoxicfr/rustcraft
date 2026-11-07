//! The borrowed identifier comparisons give the answers of `commands::args::id` (`RUSTCRAFT_OPT_IDEQ`):
//! every identifier of the 26.3 registries, written full, short and with an empty namespace,
//! against every full identifier of the item registry and odd strings.

use super::{full_id_borrowed, id_eq_borrowed};
use crate::commands::args::id as old_full_id;

/// Strings a pack may write that are no registry entry.
const ODD: [&str; 12] =
    ["", ":", "minecraft:", "::", "a:b:c", "#minecraft:logs", "#logs", "stone", ":stone", "c:", ":c:d", "é:à"];

fn raw_forms(full: &str) -> Vec<String> {
    let mut v = vec![full.to_owned()];
    if let Some(path) = full.strip_prefix("minecraft:") {
        v.push(path.to_owned());
        v.push(format!(":{path}"));
    }
    v
}

fn check(raw: &str, full: &str) {
    assert_eq!(id_eq_borrowed(raw, full), old_full_id(raw) == full, "id_eq({raw:?}, {full:?})");
}

#[test]
fn borrowed_comparison_matches_the_allocating_one() {
    let mut raws: Vec<String> = ODD.iter().map(|s| (*s).to_owned()).collect();
    let mut fulls: Vec<String> = ODD.iter().map(|s| (*s).to_owned()).collect();
    if let Some(data) = rc_testdata::version_data() {
        let all: Vec<&str> = data.registries.iter().flat_map(|r| r.entries.iter().map(String::as_str)).collect();
        assert!(all.len() > 5_000, "too few registry entries: {}", all.len());
        // Every identifier of every registry against its own forms and the odd strings.
        for full in &all {
            for raw in raw_forms(full) {
                check(&raw, full);
                for odd in ODD {
                    check(&raw, odd);
                    check(odd, full);
                }
            }
        }
        // Every pair of the item registry (all three forms of the raw side).
        let items = data.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default();
        assert!(items.len() > 1_000, "too few items: {}", items.len());
        raws.extend(items.iter().flat_map(|i| raw_forms(i)));
        fulls.extend(items);
    }
    for raw in &raws {
        assert_eq!(full_id_borrowed(raw), old_full_id(raw), "full_id({raw:?})");
        for full in &fulls {
            check(raw, full);
        }
    }
    // Expected values stored.
    assert!(id_eq_borrowed("oak_log", "minecraft:oak_log"));
    assert!(id_eq_borrowed(":oak_log", "minecraft:oak_log"));
    assert!(!id_eq_borrowed("oak_log", "oak_log"));
    assert!(!id_eq_borrowed("c:oak_log", "minecraft:oak_log"));
    assert!(id_eq_borrowed("", "minecraft:"));
    assert_eq!(full_id_borrowed(":x"), "minecraft:x");
    assert!(matches!(full_id_borrowed("c:x"), std::borrow::Cow::Borrowed("c:x")));
}

/// A comparison of a full identifier allocates nothing.
#[test]
fn borrowed_comparison_does_not_allocate() {
    let before = crate::test_alloc::allocations();
    let mut n = 0;
    for _ in 0..1_000 {
        n += usize::from(id_eq_borrowed(std::hint::black_box("oak_log"), "minecraft:oak_log"));
        n += usize::from(id_eq_borrowed(std::hint::black_box("c:logs"), "c:logs"));
        n += full_id_borrowed(std::hint::black_box("minecraft:stone")).len();
    }
    assert_eq!(crate::test_alloc::allocations() - before, 0);
    assert_eq!(n, 2_000 + 15_000);
}
