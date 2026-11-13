//! Trapped chest: a weak signal equal to its openers (15 at most) on every side, and a strong
//! one towards the block below only. Before, it gave no signal at all.

use rc_data::PropKey;

use super::test_area::open;
use super::{Dir, P};
use crate::blocks::rules::*;
use crate::mobs::mob::Ent;

fn lit(s: &crate::server::Server, at: P) -> bool {
    s.view().get_bool(s.get(at), PropKey::Lit)
}

/// Opening lights a lamp beside the chest (weak signal) and a lamp beside the stone under it
/// (strongly powered block); the signal is the opener count; closing turns both off.
#[test]
fn trapped_chest_signals_its_openers() {
    let Some(mut a) = open("trapped-chest") else { return };
    let chest: P = [110, -58, 110];
    let (beside_chest, beside_stone): (P, P) = ([109, -58, 110], [111, -59, 110]);
    a.set([110, -59, 110], "minecraft:stone");
    a.set(chest, "minecraft:trapped_chest[facing=north]");
    a.set(beside_chest, "minecraft:redstone_lamp");
    a.set(beside_stone, "minecraft:redstone_lamp");
    let _ = a.ticks(5);
    assert!(!lit(&a.p.s, beside_chest) && !lit(&a.p.s, beside_stone), "lamps lit before opening");

    a.p.s.container_openers(chest, Ent::Player, 1);
    let _ = a.ticks(1);
    assert!(lit(&a.p.s, beside_chest), "lamp beside the open trapped chest not lit (weak signal)");
    assert!(lit(&a.p.s, beside_stone), "lamp beside the block under the chest not lit (strong signal)");
    let s = a.p.s.get(chest);
    assert_eq!(a.p.s.weak(s, chest, Dir::East), 1, "weak signal = one opener");
    assert_eq!(a.p.s.strong(s, chest, Dir::Up), 1, "strong signal towards the block below");
    assert_eq!(a.p.s.strong(s, chest, Dir::East), 0, "no strong signal sideways");

    // A second opener: signal 2. A plain chest never signals.
    a.p.s.container_openers(chest, Ent::Mob(7), 1);
    assert_eq!(a.p.s.weak(a.p.s.get(chest), chest, Dir::East), 2);
    let plain: P = [120, -59, 120];
    a.set(plain, "minecraft:chest[facing=north]");
    a.p.s.container_openers(plain, Ent::Player, 1);
    assert_eq!(a.p.s.weak(a.p.s.get(plain), plain, Dir::East), 0, "plain chest signals");

    a.p.s.container_openers(chest, Ent::Mob(7), -1);
    a.p.s.container_openers(chest, Ent::Player, -1);
    let _ = a.ticks(5);
    assert!(!lit(&a.p.s, beside_chest) && !lit(&a.p.s, beside_stone), "lamps still lit after closing");
}
