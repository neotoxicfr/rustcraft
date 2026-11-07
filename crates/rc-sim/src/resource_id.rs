//! Resource identifiers compared and completed without allocating (`RUSTCRAFT_OPT_IDEQ`, on by
//! default; `=0` builds the full identifier with [`crate::commands::args::id`] each time, as before).
//!
//! A data pack may write an identifier without its namespace (`oak_log`) or with an empty one
//! (`:oak_log`): both mean `minecraft:oak_log`. Comparing such a raw identifier with a full one
//! used to allocate the full form first; the borrowed forms here give the same answers.

use std::borrow::Cow;

#[cfg(test)]
#[path = "resource_id_tests.rs"]
mod tests;

/// Whether identifiers are compared and completed by borrowing, read once.
#[inline]
pub(crate) fn borrowed() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_IDEQ")
}

/// `raw` completed with the default namespace equals `full` (the answer of
/// `commands::args::id(raw) == full`).
#[inline]
pub(crate) fn id_eq(raw: &str, full: &str) -> bool {
    if borrowed() { id_eq_borrowed(raw, full) } else { crate::commands::args::id(raw) == full }
}

/// [`id_eq`] without the allocation.
pub(crate) fn id_eq_borrowed(raw: &str, full: &str) -> bool {
    match raw.split_once(':') {
        // `oak_log` and `:oak_log` both name the default namespace.
        None => full.strip_prefix("minecraft:") == Some(raw),
        Some(("", path)) => full.strip_prefix("minecraft:") == Some(path),
        Some(_) => raw == full,
    }
}

/// Full form of `raw` (that of `commands::args::id`), borrowed when `raw` already names its
/// namespace.
#[inline]
pub(crate) fn full_id(raw: &str) -> Cow<'_, str> {
    if borrowed() { full_id_borrowed(raw) } else { Cow::Owned(crate::commands::args::id(raw)) }
}

/// [`full_id`], borrowed whenever the namespace is written and not empty.
pub(crate) fn full_id_borrowed(raw: &str) -> Cow<'_, str> {
    match raw.split_once(':') {
        Some((namespace, _)) if !namespace.is_empty() => Cow::Borrowed(raw),
        _ => Cow::Owned(crate::commands::args::id(raw)),
    }
}
