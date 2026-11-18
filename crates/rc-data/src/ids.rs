//! Lookup keys composed without allocation (`RUSTCRAFT_OPT_TAG_BORROW`, default 1): a key
//! made of pieces (`minecraft:` + name, `minecraft:end_city/` + piece…) is assembled in a
//! stack buffer and lent to the lookup; a string is allocated only beyond the buffer or on
//! the old path (`=0`). Shared by the generator, the integrated server and the client.

/// Size of the stack buffer; a longer key goes through an allocated string.
const STACK: usize = 192;

/// New path of composed-name lookups, read once: absent or other than `0` = active.
#[inline]
pub fn tag_borrow() -> bool {
    static S: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *S.get_or_init(|| crate::env_str!("RUSTCRAFT_OPT_TAG_BORROW").is_none_or(|v| v.trim() != "0"))
}

/// Calls `f` with the concatenation of `parts`, on the switch's path.
#[inline]
pub fn with_key<R>(parts: &[&str], f: impl FnOnce(&str) -> R) -> R {
    with_key_mode(parts, tag_borrow(), f)
}

/// [`with_key`] on the chosen path (equivalence proofs): `borrow` = stack buffer, otherwise allocated
/// string as before.
#[inline]
pub fn with_key_mode<R>(parts: &[&str], borrow: bool, f: impl FnOnce(&str) -> R) -> R {
    let n: usize = parts.iter().map(|p| p.len()).sum();
    if !borrow || n > STACK {
        return f(&parts.concat());
    }
    let mut buf = [0u8; STACK];
    let mut i = 0;
    for p in parts {
        buf[i..i + p.len()].copy_from_slice(p.as_bytes());
        i += p.len();
    }
    // Concatenation of valid UTF-8 texts.
    f(std::str::from_utf8(&buf[..n]).unwrap_or_default())
}

/// Calls `f` with the full identifier of `id` (`minecraft:` added if it has no
/// namespace), on the switch's path.
#[inline]
pub fn with_full_id<R>(id: &str, f: impl FnOnce(&str) -> R) -> R {
    with_full_id_mode(id, tag_borrow(), f)
}

/// [`with_full_id`] on the chosen path.
#[inline]
pub fn with_full_id_mode<R>(id: &str, borrow: bool, f: impl FnOnce(&str) -> R) -> R {
    if id.contains(':') { f(id) } else { with_key_mode(&["minecraft:", id], borrow, f) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected stored values, two paths, buffer exceeded and empty.
    #[test]
    fn composed_keys() {
        let long = "x".repeat(STACK);
        let cases: [(&[&str], String); 5] = [
            (&["minecraft:", "stone"], "minecraft:stone".into()),
            (&["minecraft:end_city/", "base_floor"], "minecraft:end_city/base_floor".into()),
            (&["a", "/", "", "é"], "a/é".into()),
            (&[], String::new()),
            (&["minecraft:", &long], format!("minecraft:{long}")),
        ];
        for (parts, want) in &cases {
            for borrow in [true, false] {
                assert_eq!(with_key_mode(parts, borrow, str::to_owned), *want, "{parts:?} {borrow}");
            }
        }
        for borrow in [true, false] {
            assert_eq!(with_full_id_mode("oak_log", borrow, str::to_owned), "minecraft:oak_log");
            assert_eq!(with_full_id_mode("c:logs", borrow, str::to_owned), "c:logs");
            assert_eq!(with_full_id_mode("", borrow, str::to_owned), "minecraft:");
        }
    }
}
