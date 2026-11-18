//! Worn armor keeps every part of its own model: the pose comes from the wearer, never its hidden
//! parts (an armor stand without arms, a player whose hat layer is off). 26.3 data and game,
//! windowless GPU; without them, test skipped (failure under `RUSTCRAFT_OPT_REQUIRE=1`).

use super::tests_g2f03::{Kit, kit};
use super::*;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::item::ItemStack;

/// The matrices the instance of `layer` drew with, by part name (zero for a hidden part); `None`
/// when the layer was not drawn.
fn drawn_layer(k: &Kit, layer: &str) -> Option<HashMap<String, PartMat>> {
    let gl = &k.r.layers[*k.r.layer_index.get(layer)?];
    let (_, _, _, instance) =
        k.r.instances.iter().flatten().find(|(_, first, count, _)| *first == gl.first && *count == gl.count)?;
    let base = instance.base as usize;
    Some(gl.baked.names.iter().enumerate().map(|(i, name)| (name.clone(), k.r.mats[base + i])).collect())
}

fn drawn_parts(k: &Kit, layer: &str) -> HashMap<String, PartMat> {
    drawn_layer(k, layer).unwrap_or_else(|| panic!("{layer} not drawn"))
}

fn shown(m: &PartMat) -> bool {
    m.m[3][3] != 0.0
}

fn worn(k: &Kit, item: &str) -> Option<ItemStack> {
    Some(ItemStack { item: k.item(item), count: 1, added: vec![], removed: vec![] })
}

/// An armor stand without arms (the default) wearing a chestplate: the chestplate's sleeves are
/// drawn, though the stand's own arms are hidden.
#[test]
fn chestplate_keeps_its_sleeves_on_a_stand_without_arms() {
    let Some(mut k) = kit() else { return };
    let chestplate = worn(&k, "minecraft:iron_chestplate");
    let stand = Kit::entity("armor_stand", 1, |e| e.equipment[4] = chestplate);
    k.draw(&stand);
    let body = drawn_parts(&k, "minecraft:armor_stand#main");
    assert!(!shown(&body["right_arm"]) && !shown(&body["left_arm"]), "the stand has no arms");
    let armor = drawn_parts(&k, "minecraft:armor_stand#chestplate");
    for sleeve in ["right_arm", "left_arm"] {
        assert!(shown(&armor[sleeve]), "the chestplate's {sleeve} is hidden");
    }
}

/// A player with the hat layer of its skin off, wearing a helmet: the helmet's outer layer is
/// drawn, while the skin's hat is not.
#[test]
fn helmet_keeps_its_outer_layer_on_a_player_without_hat() {
    let Some(mut k) = kit() else { return };
    let helmet = worn(&k, "minecraft:iron_helmet");
    let player = Kit::entity("player", 2, |e| {
        e.equipment[5] = helmet;
        // Every skin layer but the hat (0x40).
        e.meta.insert(16, Meta::Byte(0x3f));
    });
    k.draw(&player);
    // Wide or slim model, after the default skin of its UUID.
    let model = ["minecraft:player", "minecraft:player_slim"]
        .into_iter()
        .find(|m| drawn_layer(&k, &format!("{m}#main")).is_some())
        .expect("player model drawn");
    assert!(!shown(&drawn_parts(&k, &format!("{model}#main"))["hat"]), "the skin's hat stays hidden");
    assert!(shown(&drawn_parts(&k, &format!("{model}#helmet"))["hat"]), "the helmet's outer layer is hidden");
}
