//! A baby's held weapon and worn armor, as the 26.3 game draws them: expected values written from
//! the game's rules in our own words, no extracted data.

use super::keyframes::testkit::{self, state, test_layer};
use super::*;
use rc_proto_v777::item::ItemStack;

/// Pitch of the right arm of a zombified piglin, baby or not, holding something or not.
fn zombified_piglin_right_arm_pitch(baby: bool, armed: bool) -> f32 {
    let parts = [
        ("head", None),
        ("body", None),
        ("right_arm", None),
        ("left_arm", None),
        ("right_ear", None),
        ("left_ear", None),
    ];
    let l = test_layer(&parts);
    let mut all = testkit::spawn("zombified_piglin");
    let e = all.map.get_mut(&7).unwrap();
    if armed {
        e.equipment[0] = Some(ItemStack { item: 1, count: 1, added: vec![], removed: vec![] });
    }
    let mut s = state(e, 0.0);
    s.baby = baby;
    let mut p = Pose::reset(&l, Vec::new(), Vec::new());
    animate(&mut p, e, &s, 0.0, true);
    p.parts[2][3]
}

/// Zombified piglin: like every other zombie, the arms are raised only by an adult or by a baby
/// with an empty main hand; an armed baby keeps them down.
#[test]
fn armed_baby_zombified_piglin_keeps_its_arms_down() {
    let raised = -PI / 2.25;
    assert!((zombified_piglin_right_arm_pitch(false, true) - raised).abs() < 1e-6, "armed adult");
    assert!((zombified_piglin_right_arm_pitch(true, false) - raised).abs() < 1e-6, "unarmed baby");
    assert_eq!(zombified_piglin_right_arm_pitch(true, true), 0.0, "armed baby");
}

/// A baby wears the baby armor textures on every slot, leggings included; an adult keeps the
/// textures of each slot.
#[test]
fn baby_wears_baby_armor_textures_on_every_slot() {
    for slot_type in ["humanoid", "humanoid_leggings"] {
        assert_eq!(worn_layer_type(slot_type, true), "humanoid_baby", "{slot_type}");
        assert_eq!(worn_layer_type(slot_type, false), slot_type);
    }
}
