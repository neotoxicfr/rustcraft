//! Equivalence of the typed property reads and writes with the reads and writes by name, on
//! every state of the 26.3 data: by index (`RUSTCRAFT_OPT_SERVER_PROPERTY_INDEX`), packed per
//! state (`RUSTCRAFT_OPT_PACKED_STATE_PROPERTIES`) and both; the property change rebuilt from
//! text (`Server::with_prop`) against its version by stride
//! (`RUSTCRAFT_OPT_PROPERTY_CHANGE_BY_STRIDE`); the liquid states by legacy level
//! (`RUSTCRAFT_OPT_LIQUID_STATE_TABLE`). Each comparison must catch a sabotaged table. Game data
//! absent: test skipped, failure under `RUSTCRAFT_OPT_REQUIRE=1`.

use rc_data::{PropKey, VersionData};

use super::Dir;
use super::table::{PropSwitches, Table, View};
use crate::containers::state_with_prop_rebuilt;
use crate::states::States;

const BY_NAME: PropSwitches = PropSwitches::ALL_OFF;
const BY_INDEX: PropSwitches = PropSwitches { by_index: true, ..PropSwitches::ALL_OFF };
const PACKED: PropSwitches = PropSwitches { packed_properties: true, ..PropSwitches::ALL_OFF };
const BOTH: PropSwitches = PropSwitches { by_index: true, packed_properties: true, ..PropSwitches::ALL_OFF };

/// Expected fingerprint of every read and write by name on the 26.3 data (see `results`): the
/// code common to all paths (state layout, value lists) cannot change without breaking it. A new
/// key adds its own reads and changes it (last one: the candles' `candles`).
const BY_NAME_FINGERPRINT: u64 = 0xf2be_375a_272e_59bd;

fn data() -> Option<VersionData> {
    rc_testdata::need(rc_testdata::version_data(), "26.3 data cache")
}

/// FNV-1a 64, to fold the results into one stored number.
fn fnv(h: &mut u64, v: u64) {
    for b in v.to_le_bytes() {
        *h = (*h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
}

fn text_hash(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in s.as_bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Values written by text and by integer for `key` in state `s`: every value of the property
/// (if the block has it), absent values, and integers around each value (negative and far
/// out of range included).
fn probes(v: &View, s: u32, key: PropKey) -> (Vec<String>, Vec<i32>) {
    let mut texts: Vec<String> = ["", "nope", "true", "false", "north", "0"].map(str::to_owned).to_vec();
    let mut ints = vec![-1, 0, 1, 15, 16, 26, i32::MIN, i32::MAX];
    if let Some(values) =
        v.props_sorted(s).into_iter().find(|(name, _, _)| *name == key.name()).map(|(_, values, _)| values)
    {
        texts.extend(values.iter().cloned());
        ints.extend(values.iter().filter_map(|x| x.parse::<i32>().ok()).flat_map(|n| [n - 1, n, n + 1]));
    }
    (texts, ints)
}

/// Every typed read and write of state `s` for `key`, as numbers.
fn results(v: &View, s: u32, key: PropKey, texts: &[String], ints: &[i32], out: &mut Vec<u64>) {
    out.clear();
    out.push(u64::from(v.has(s, key)));
    out.push(v.get(s, key).map_or(u64::MAX, text_hash));
    out.push(u64::from(v.get_bool(s, key)));
    out.push(u64::from(v.get_int(s, key) as u32));
    out.push(v.get_dir(s, key).map_or(9, |d| d as u64));
    out.push(v.facing(s) as u64);
    out.push(u64::from(v.cycle(s, key)));
    out.push(u64::from(v.set_bool(s, key, true)) | u64::from(v.set_bool(s, key, false)) << 32);
    out.extend(Dir::ALL.map(|d| u64::from(v.set_dir(s, key, d))));
    out.extend(texts.iter().map(|t| u64::from(v.set(s, key, t))));
    out.extend(ints.iter().map(|&n| u64::from(v.set_int(s, key, n))));
}

/// What a comparison went through.
#[derive(Debug)]
struct Compared {
    /// Results compared per candidate.
    checks: usize,
    /// Fingerprint of the reference's results.
    print: u64,
    /// (state, key) pairs where the block has the key.
    present: usize,
}

/// Compares every typed read and write of each candidate with `reference` over all states and
/// keys; the first difference as an error.
fn compare(reference: &View, candidates: &[(&str, View)]) -> Result<Compared, String> {
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let mut out = Compared { checks: 0, print: 0xcbf2_9ce4_8422_2325, present: 0 };
    for s in 0..reference.data.states.len() as u32 {
        for &key in PropKey::ALL {
            let (texts, ints) = probes(reference, s, key);
            out.present += usize::from(reference.has_named(s, key.name()));
            results(reference, s, key, &texts, &ints, &mut a);
            for (label, candidate) in candidates {
                results(candidate, s, key, &texts, &ints, &mut b);
                if let Some(i) = a.iter().zip(&b).position(|(x, y)| x != y) {
                    return Err(format!(
                        "{label}: state {s} ({}), {}: result {i} by name {:#x}, candidate {:#x}",
                        reference.data.block_of(s).map_or("?", |b| b.name.as_str()),
                        key.name(),
                        a[i],
                        b[i]
                    ));
                }
            }
            out.checks += a.len();
            a.iter().for_each(|&x| fnv(&mut out.print, x));
        }
    }
    Ok(out)
}

#[test]
fn typed_property_paths_identical_to_the_names() {
    let Some(d) = data() else { return };
    let by_name = Table::with_switches(&d, BY_NAME);
    let reference = View { table: &by_name, data: &d };
    assert!(d.states.len() > 25_000, "too few states: {}", d.states.len());
    let tables = [BY_INDEX, PACKED, BOTH].map(|switches| Table::with_switches(&d, switches));
    let candidates = [("by index", 0), ("packed", 1), ("both", 2)]
        .map(|(label, i)| (label, View { table: &tables[i], data: &d }));
    let c = compare(&reference, &candidates).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("{} results identical per path, {} states × keys present, fingerprint {:#018x}", c.checks, c.present, c.print);
    // Useful pairs in number (otherwise the test would prove nothing).
    assert!(c.present > 60_000, "too few properties found: {}", c.present);
    assert_eq!(c.print, BY_NAME_FINGERPRINT, "fingerprint of the results by name ({} values)", c.checks);
}

#[test]
fn packed_fields_hold_every_state_of_the_data() {
    let Some(d) = data() else { return };
    let table = Table::with_switches(&d, BOTH);
    let unusable: Vec<&str> = PropKey::ALL
        .iter()
        .filter(|k| k.packed_field().is_some() && !table.index().packed_usable(**k))
        .map(|k| k.name())
        .collect();
    assert!(unusable.is_empty(), "packed fields left unused (value out of their width): {unusable:?}");
    let v = View { table: &table, data: &d };
    let count = |key| (0..d.states.len() as u32).filter(|&s| v.get_bool(s, key)).count();
    // Expected numbers stored: the packing of each boolean is read back, not only compared.
    let counts = [PropKey::Waterlogged, PropKey::Powered, PropKey::Lit].map(count);
    assert_eq!(counts, [13_325, 3_344, 221], "waterlogged, powered, lit = true states");
}

/// Sabotages caught: a boolean property with `true` and `false` swapped, a property that
/// lacks `true` but is written as if it had it (the "unchanged" fallback lost), one state's
/// packed bits flipped.
#[test]
fn sabotaged_tables_are_caught() {
    let Some(d) = data() else { return };
    let by_name = Table::with_switches(&d, BY_NAME);
    let reference = View { table: &by_name, data: &d };
    let property = |name: &str, with_true: bool| {
        d.properties
            .iter()
            .position(|p| p.name == name && p.values.iter().any(|v| v == "true") == with_true)
            .unwrap_or_else(|| panic!("property {name}")) as u16
    };
    let caught = |table: &Table| compare(&reference, &[("sabotaged", View { table, data: &d })]).is_err();
    let mut swapped = Table::with_switches(&d, BY_INDEX);
    swapped.index_mut().sabotage_value_facts(property("powered", true), |f| f.bool_index.swap(0, 1));
    assert!(caught(&swapped), "true/false swap not caught");
    let mut fallback = Table::with_switches(&d, BY_INDEX);
    fallback.index_mut().sabotage_value_facts(property("facing", false), |f| f.bool_index[1] = 0);
    assert!(caught(&fallback), "lost fallback not caught");
    let mut packed = Table::with_switches(&d, PACKED);
    let lamp = (0..d.states.len() as u32)
        .find(|&s| d.block_of(s).is_some_and(|b| b.name == "minecraft:redstone_lamp"))
        .expect("redstone lamp");
    let word = packed.index().packed_word(lamp);
    let lit = PropKey::Lit.packed_field().expect("lit packed").shift;
    packed.index_mut().sabotage_packed(lamp, word ^ 1 << lit);
    assert!(caught(&packed), "flipped packed bit not caught");
}

/// `Server::with_prop` without rebuilding the palette entry, identical to the rebuild for every
/// state, key and value (absent values give the default, absent keys the state itself).
fn with_prop_mismatch(view: &View, states: &States) -> Option<String> {
    let d = view.data;
    for s in 0..d.states.len() as u32 {
        for &key in PropKey::ALL {
            // A key the block lacks takes one branch whatever the value: sampled (each rebuild
            // allocates a whole palette entry).
            if !view.has_named(s, key.name()) && !(s as usize + key as usize).is_multiple_of(13) {
                continue;
            }
            let (texts, ints) = probes(view, s, key);
            for t in &texts {
                let old = state_with_prop_rebuilt(d, states, s, key.name(), t);
                if view.set_or_default(s, key, t) != old {
                    return Some(format!("state {s}, {} = {t:?}: rebuilt {old:?}", key.name()));
                }
            }
            for &n in &ints {
                let old = state_with_prop_rebuilt(d, states, s, key.name(), &n.to_string());
                if view.set_int_or_default(s, key, n) != old {
                    return Some(format!("state {s}, {} = {n}: rebuilt {old:?}", key.name()));
                }
            }
        }
    }
    None
}

#[test]
fn property_change_by_index_identical_to_the_rebuild() {
    let Some(d) = data() else { return };
    let states = States::new(&d, Vec::new());
    let table = Table::with_switches(&d, BY_INDEX);
    let view = View { table: &table, data: &d };
    if let Some(e) = with_prop_mismatch(&view, &states) {
        panic!("{e}");
    }
    // Unknown state: no state, on both paths.
    let past = d.states.len() as u32;
    assert_eq!(view.set_or_default(past, PropKey::Age, "1"), None);
    assert_eq!(state_with_prop_rebuilt(&d, &states, past, "age", "1"), None);
    // Sabotage of the integer positions: the first integer of `age` shifted by one.
    let mut shifted = Table::with_switches(&d, BY_INDEX);
    let age = d.properties.iter().position(|p| p.name == "age" && p.values.len() > 2).expect("age") as u16;
    shifted.index_mut().sabotage_value_facts(age, |f| f.int_first = f.int_first.map(|x| x + 1));
    assert!(with_prop_mismatch(&View { table: &shifted, data: &d }, &states).is_some(), "shifted age not caught");
    // Sabotage of the default rule: a value the property lacks keeps the current value.
    let mut no_default = Table::with_switches(&d, BY_INDEX);
    no_default.index_mut().sabotage_default_rule();
    assert!(
        with_prop_mismatch(&View { table: &no_default, data: &d }, &states).is_some(),
        "missing value kept instead of the default not caught"
    );
}

#[test]
fn liquid_states_by_level_identical_to_the_names() {
    let Some(d) = data() else { return };
    let table = Table::with_switches(&d, BY_INDEX);
    let by_name = Table::with_switches(&d, BY_NAME);
    let v = View { table: &by_name, data: &d };
    let mut levels = 0;
    for (kind, block) in [(1u8, table.id.water), (2, table.id.lava)] {
        for level in 0..=16 {
            let old = v.set_int(v.default_state(block), PropKey::Level, level);
            assert_eq!(table.liquid_state(kind, level), Some(old), "kind {kind} level {level}");
            levels += usize::from(old != v.default_state(block));
        }
        for level in [-1, 17, i32::MAX] {
            assert_eq!(table.liquid_state(kind, level), None, "kind {kind} level {level}");
        }
    }
    assert_eq!(table.liquid_state(0, 0), None);
    assert_eq!(table.liquid_state(3, 0), None);
    // 15 non-default levels per liquid (level 0 is the default state, 16 does not exist).
    assert_eq!(levels, 30, "levels written");
}
