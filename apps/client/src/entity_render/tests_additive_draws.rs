//! Additive draws of the world reported to the fog cull (`render::fog_cull`): plain fog adds its
//! colour to the pixel under them instead of hiding them, so the cull must know how far they
//! reach. Energy swirl of a charged creeper (measured), lightning and death rays of the dragon
//! (not measured); nothing for an entity without such a layer.

use rc_proto_v777::entity::Meta;

use super::tests_g2f03::{Kit, kit};
use super::*;

/// Eye of the kit's frame and feet of its entity (`Kit::draw`, `Kit::entity`).
const KIT_EYE: Vec3 = Vec3::new(0.5, 65.0, 5.5);
const KIT_FEET: Vec3 = Vec3::new(0.5, 64.0, 0.5);

/// Reach of the additive draws of a frame that holds the single entity `e`.
fn additive_reach_of(k: &mut Kit, e: &Entity) -> f32 {
    k.draw(e);
    k.r.farthest_additive_draw()
}

#[test]
fn additive_draws_report_how_far_they_reach() {
    let Some(mut k) = kit() else { return };
    // No additive layer: nothing to report.
    let creeper = Kit::entity("creeper", 7, |_| {});
    assert_eq!(additive_reach_of(&mut k, &creeper), 0.0);
    // Charged creeper: its swirl is held within twice its box around its feet.
    let charged = Kit::entity("creeper", 7, |e| {
        e.meta.insert(17, Meta::Bool(true));
    });
    let size = k.r.size_of(&charged);
    assert!(size.w > 0.0 && size.h > 0.0, "the kit gives the creeper no box");
    let expected = KIT_FEET.distance(KIT_EYE) + 2.0 * (size.w + size.h);
    let swirl_reach = additive_reach_of(&mut k, &charged);
    assert!((swirl_reach - expected).abs() < 1e-3, "{swirl_reach} for {expected}");
    // The next frame forgets the reach of the one before.
    assert_eq!(additive_reach_of(&mut k, &creeper), 0.0);
    // Lightning and the death rays of the dragon: not measured, so they reach any distance.
    assert_eq!(additive_reach_of(&mut k, &Kit::entity("lightning_bolt", 7, |_| {})), f32::INFINITY);
    let dying_dragon = Kit::entity("ender_dragon", 7, |e| e.death_time = 50);
    assert_eq!(additive_reach_of(&mut k, &dying_dragon), f32::INFINITY);
    assert_eq!(additive_reach_of(&mut k, &Kit::entity("ender_dragon", 7, |_| {})), 0.0);
}
