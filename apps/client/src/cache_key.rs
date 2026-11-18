//! One 64-bit key for client cache entries built from game data: the item atlas cell of an item
//! render, the swallowed content of a sulfur cube, a worn armor trim texture, the recipe book's
//! "craftable" stamp, the tooltip log. Every call site goes through [`cache_key`], so two screens
//! that draw the same item ask the atlas for the same cell.

use std::hash::{BuildHasher, Hash};

/// Seed of the cache keys. Any constant works: a key is only compared with keys computed earlier
/// in the same process, so the seed has to be fixed for the process, never drawn per call.
const CACHE_KEY_SEED: u64 = 0x5275_7374_6372_6166;

/// Key of `value`, the same on every call for equal values.
///
/// `RUSTCRAFT_OPT_FAST_CACHE_KEY=0` restores the former hasher (the standard library's SipHash).
pub fn cache_key<T: Hash + ?Sized>(value: &T) -> u64 {
    if rc_data::opt_switch!("RUSTCRAFT_OPT_FAST_CACHE_KEY") { fast_cache_key(value) } else { sip_cache_key(value) }
}

/// Atlas cell key of an item render: the identity list the item model evaluation fills, folded into
/// one number. The hotbar, the menus, the free item drawings and the debug switcher all use it.
pub fn item_layers_key(identity: &[u64]) -> u64 {
    cache_key(identity)
}

/// foldhash (quality variant) with a fixed seed: a folded multiply per word where SipHash runs
/// its rounds. The quality variant keeps a good spread of the 64 bits, since two different
/// renders that collide would share an atlas cell.
pub fn fast_cache_key<T: Hash + ?Sized>(value: &T) -> u64 {
    foldhash::quality::FixedState::with_seed(CACHE_KEY_SEED).hash_one(value)
}

/// Former key: the standard library's SipHash-1-3 with its fixed zero key.
#[allow(clippy::disallowed_types, reason = "former path kept behind RUSTCRAFT_OPT_FAST_CACHE_KEY=0")]
pub fn sip_cache_key<T: Hash + ?Sized>(value: &T) -> u64 {
    std::hash::BuildHasherDefault::<std::hash::DefaultHasher>::default().hash_one(value)
}

#[cfg(test)]
mod tests;
