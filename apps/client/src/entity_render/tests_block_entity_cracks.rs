//! Mining cracks over the models of block entities: the chest, the banner (pole and flag), the
//! bell… hand their faces to the crack pass where they are drawn; the decorated pot does not, nor
//! a block that is not being mined. 26.3 data and game, windowless GPU; without them, test
//! skipped (failure under `RUSTCRAFT_OPT_REQUIRE=1`).

use super::tests_g2f03::{Kit, kit};
use super::*;
use crate::block_entities::BlockEntities;
use crate::crack::{CrackedBlock, ModelFace};

fn state_of(k: &Kit, block: &str, props: &[(&str, &str)]) -> u32 {
    let b = k.r.source.blocks.iter().find(|b| b.name == format!("minecraft:{block}")).expect(block);
    (b.first_state..b.first_state + b.state_count)
        .find(|&s| props.iter().all(|(n, v)| k.r.source.state_properties(s).any(|(a, b)| a == *n && b == *v)))
        .unwrap_or_else(|| panic!("{block} {props:?}"))
}

/// The crack faces of the frame where `block` stands at (0, 64, 0), mined at stage 6 when
/// `mined`.
fn crack_faces(k: &mut Kit, state: u32, mined: bool) -> Vec<ModelFace> {
    let pos = [0, 64, 0];
    let mut bes = BlockEntities::new(k.r.source.clone());
    bes.on_block_change(pos, 0, state);
    let cracked = [CrackedBlock { pos, state, stage: 6 }];
    let biome = |_, _, _| [0xff_ffff; 4];
    let frame = BlockFrame {
        pt: 0.0,
        origin: [0, 0],
        cam: [0.5, 65.0, 3.5],
        game_time: 0,
        min_y: -64,
        light: &|_, _, _| (15, 15),
        visible: &crate::render::FrameVisibility::everything(),
        items: None,
        render_distance: 12,
        top: 320,
        state: &|_, _, _| 0,
        scoping: false,
        biome_tints: &biome,
        cracked: if mined { &cracked[..] } else { &[] },
    };
    k.r.instances.iter_mut().for_each(Vec::clear);
    k.r.mats.clear();
    k.r.push_block_entities(&bes, &frame);
    let mut faces = Vec::new();
    k.r.crumbling_faces(&mut faces);
    faces
}

/// Faces of the visible parts of `layers`, as baked from the data.
fn model_faces(k: &Kit, layers: &[&str]) -> usize {
    layers
        .iter()
        .map(|l| bake_layer(&k.r.source.entity_models[k.r.layer_index[*l]]).parts.iter().map(|p| p.quads.len()).sum::<usize>())
        .sum()
}

#[test]
fn block_entity_models_take_the_cracks() {
    let Some(mut k) = kit() else { return };
    let chest = state_of(&k, "chest", &[("facing", "north"), ("type", "single")]);
    let faces = crack_faces(&mut k, chest, true);
    assert_eq!(faces.len(), model_faces(&k, &["minecraft:chest#main"]), "every face of the closed chest");
    for f in &faces {
        assert_eq!((f.pos, f.stage, f.translucent), ([0, 64, 0], 6, false));
        // A closed chest stays inside its block (frame of the cracks: camera origin (0, 0)).
        for c in f.corners {
            let inside = (-1e-3..=1.001).contains(&c.x) && (63.999..=65.001).contains(&c.y) && (-1e-3..=1.001).contains(&c.z);
            assert!(inside, "{c:?}");
        }
    }
    assert!(crack_faces(&mut k, chest, false).is_empty(), "not mined: no crack");
    // Banner: pole and bar, then the flag once (not each pattern over it).
    let banner = state_of(&k, "white_banner", &[("rotation", "0")]);
    let expected = model_faces(&k, &["minecraft:standing_banner#main", "minecraft:standing_banner#flag"]);
    assert_eq!(crack_faces(&mut k, banner, true).len(), expected, "banner");
    let bell = state_of(&k, "bell", &[("attachment", "floor"), ("facing", "north")]);
    assert_eq!(crack_faces(&mut k, bell, true).len(), model_faces(&k, &["minecraft:bell#main"]), "bell");
    // The decorated pot takes no crack over its model, like the game.
    let pot = state_of(&k, "decorated_pot", &[("facing", "north")]);
    assert!(crack_faces(&mut k, pot, true).is_empty(), "decorated pot");
}
