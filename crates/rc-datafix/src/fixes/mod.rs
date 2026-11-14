//! The conversion rules themselves, grouped by what they change. Each is a function of one
//! node of its rule's kind; [`crate::chain`] says when and where it applies.

pub(crate) mod blocks;
pub(crate) mod chunks;
pub(crate) mod items;
pub(crate) mod profiles;

use rc_proto::nbt::Nbt;

/// Field `name` of a compound, to change.
pub(crate) fn field_mut<'a>(value: &'a mut Nbt, name: &str) -> Option<&'a mut Nbt> {
    match value {
        Nbt::Compound(entries) => entries.iter_mut().find(|(k, _)| k == name).map(|(_, v)| v),
        _ => None,
    }
}

/// Removes field `name` of a compound; its value if it was there.
pub(crate) fn remove(value: &mut Nbt, name: &str) -> Option<Nbt> {
    let Nbt::Compound(entries) = value else { return None };
    let at = entries.iter().position(|(k, _)| k == name)?;
    Some(entries.remove(at).1)
}

/// Sets field `name` of a compound (replaced in place when present, added last otherwise).
pub(crate) fn set(value: &mut Nbt, name: &str, new: Nbt) {
    let Nbt::Compound(entries) = value else { return };
    match entries.iter_mut().find(|(k, _)| k == name) {
        Some((_, v)) => *v = new,
        None => entries.push((name.to_owned(), new)),
    }
}

/// Moves field `from` to `to` when present (replacing any `to`), like the game's field rename.
pub(crate) fn rename(value: &mut Nbt, from: &str, to: &str) {
    if let Some(v) = remove(value, from) {
        set(value, to, v);
    }
}

/// The `translate` key of a text that is only `{translate: ...}`.
pub(crate) fn plain_translation_key(text: &Nbt) -> Option<&str> {
    match text {
        Nbt::Compound(entries) if entries.len() == 1 => text.get("translate").and_then(Nbt::as_str),
        _ => None,
    }
}

/// A compound of the given fields, in order.
pub(crate) fn compound(fields: impl IntoIterator<Item = (&'static str, Nbt)>) -> Nbt {
    Nbt::Compound(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}
