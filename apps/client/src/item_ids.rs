//! Ids looked up by name through the name index of the version data's registries, in place of a
//! walk over the ~1,500 item names at each lookup (`RUSTCRAFT_OPT_INDEXED_NAME_LOOKUPS`; `=0`:
//! the walks).

use rc_data::{Registry, VersionData};

/// `RUSTCRAFT_OPT_INDEXED_NAME_LOOKUPS` (on; `=0`: walk over the names, old path).
pub(crate) fn indexed_name_lookups() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_INDEXED_NAME_LOOKUPS")
}

/// Id of the item whose full name is `full` (`minecraft:stone`). `names` are the item registry's
/// entries (the walk of the old path).
pub(crate) fn item_id(data: &VersionData, names: &[String], full: &str, indexed: bool) -> Option<i32> {
    let id = if indexed { data.items()?.id(full)? as usize } else { names.iter().position(|n| n == full)? };
    Some(id as i32)
}

/// Id of the item named `path` without its namespace (`stone`): the first entry that reads `path`
/// once a `minecraft:` namespace is taken off.
pub(crate) fn item_id_by_path(data: &VersionData, names: &[String], path: &str, indexed: bool) -> Option<i32> {
    if !indexed {
        return names.iter().position(|n| n.strip_prefix("minecraft:").unwrap_or(n) == path).map(|i| i as i32);
    }
    registry_id_by_path(data.items()?, path).map(|i| i as i32)
}

/// Id of `path` in `registry`: the first entry that reads `path` once a `minecraft:` namespace is
/// taken off, that is `minecraft:<path>`, or `<path>` itself unless it starts with that namespace.
/// The game's registries have no bare entry; looking up both keeps the answer of the walk for any
/// list.
pub(crate) fn registry_id_by_path(registry: &Registry, path: &str) -> Option<u32> {
    let full = rc_data::ids::with_key(&["minecraft:", path], |name| registry.id(name));
    let bare = if path.starts_with("minecraft:") { None } else { registry.id(path) };
    match (full, bare) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every item of the game, by full name and by path, answers what the walk answers, and so do
    /// absent names; a list with bare entries and repeats keeps the first match of the walk.
    #[test]
    fn indexed_item_ids_match_the_walk() {
        let Some(data) = rc_testdata::version_data() else { return };
        let names = data.items().expect("item registry").entries.clone();
        assert!(names.len() > 1_000, "{} items", names.len());
        let absent = ["", "minecraft:", "minecraft:not_an_item", "stone:minecraft", "minecraft:minecraft:stone"];
        for full in names.iter().map(String::as_str).chain(absent) {
            let path = full.strip_prefix("minecraft:").unwrap_or(full);
            for (label, indexed) in [("walk", false), ("index", true)] {
                let want = names.iter().position(|n| n == full).map(|i| i as i32);
                assert_eq!(item_id(&data, &names, full, indexed), want, "{label}: {full}");
                let want = names.iter().position(|n| n.strip_prefix("minecraft:").unwrap_or(n) == path);
                assert_eq!(item_id_by_path(&data, &names, path, indexed), want.map(|i| i as i32), "{label}: {path}");
            }
        }
        let odd = ["stone", "minecraft:dirt", "dirt", "minecraft:stone", "x:sand", "sand", "minecraft:minecraft:clay"];
        let odd = Registry::new("odd", odd.map(String::from).to_vec());
        for path in ["stone", "dirt", "sand", "gravel", "minecraft:clay", "minecraft:stone", "x:sand", ""] {
            let want =
                odd.entries.iter().position(|n| n.strip_prefix("minecraft:").unwrap_or(n) == path).map(|i| i as u32);
            assert_eq!(registry_id_by_path(&odd, path), want, "{path}");
        }
    }
}
