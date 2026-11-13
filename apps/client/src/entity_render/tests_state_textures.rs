//! Textures picked by an entity's state and age (bee, fox, panda, goat, dolphin, armadillo), no
//! adult texture on a baby model, and the entity sheet placing a tall image whole. 26.3 data and
//! game, windowless GPU; without them, test skipped (failure under `RUSTCRAFT_OPT_REQUIRE=1`).

use super::tests_g2f03::{Kit, kit};
use super::*;
use rc_proto_v777::entity::Meta;

fn baby(e: &mut Entity) {
    e.meta.insert(16, Meta::Bool(true));
}

/// Anger that ends long after any game time a test sets.
fn angry(e: &mut Entity) {
    e.meta.insert(19, Meta::Long(i64::MAX / 2));
}

fn genes(e: &mut Entity, main: i8, hidden: i8) {
    e.meta.insert(21, Meta::Byte(main));
    e.meta.insert(22, Meta::Byte(hidden));
}

/// An entity of `kind` with its real type id (the renderer keeps the layers it resolved per type
/// id), prepared by `edit`.
fn entity(k: &Kit, kind: &str, id: i32, edit: fn(&mut Entity)) -> Entity {
    let names = k.r.source.registry("minecraft:entity_type").expect("types").entries.clone();
    let type_id = names.iter().position(|n| n.strip_prefix("minecraft:") == Some(kind)).expect(kind);
    let mut all = crate::entities::Entities { type_names: names, ..Default::default() };
    let add = rc_proto_v777::entity::AddEntity {
        id,
        uuid: 1000 + id as u128,
        kind: type_id as i32,
        pos: [0.5, 64.0, 0.5],
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: 0,
    };
    all.apply(rc_client::EntityEvent::Add(Box::new(add)));
    let mut e = all.map.remove(&id).expect("entity added");
    edit(&mut e);
    e
}

/// Each state and age draws the game's texture.
#[test]
fn state_and_baby_textures_follow_the_game() {
    let Some(mut k) = kit() else { return };
    type Edit = fn(&mut Entity);
    let cases: [(&str, Edit, &str); 17] = [
        ("bee", |_| {}, "entity/bee/bee"),
        ("bee", |e| e.meta.insert(18, Meta::Byte(8)), "entity/bee/bee_nectar"),
        ("bee", angry, "entity/bee/bee_angry"),
        (
            "bee",
            |e| {
                angry(e);
                e.meta.insert(18, Meta::Byte(8));
                baby(e);
            },
            "entity/bee/bee_angry_nectar_baby",
        ),
        ("fox", |e| e.meta.insert(18, Meta::Int(1)), "entity/fox/fox_snow"),
        ("fox", |e| e.meta.insert(19, Meta::Byte(32)), "entity/fox/fox_sleep"),
        // An unknown variant reads as red.
        ("fox", |e| e.meta.insert(18, Meta::Int(7)), "entity/fox/fox"),
        (
            "fox",
            |e| {
                e.meta.insert(18, Meta::Int(1));
                e.meta.insert(19, Meta::Byte(32));
                baby(e);
            },
            "entity/fox/fox_snow_sleep_baby",
        ),
        ("panda", |e| genes(e, 1, 0), "entity/panda/panda_lazy"),
        // Brown is recessive: shown only with two brown genes.
        ("panda", |e| genes(e, 4, 4), "entity/panda/panda_brown"),
        ("panda", |e| genes(e, 4, 2), "entity/panda/panda"),
        ("panda", |e| genes(e, 6, 5), "entity/panda/panda_aggressive"),
        ("panda", |e| genes(e, -3, 0), "entity/panda/panda"),
        (
            "panda",
            |e| {
                genes(e, 5, 5);
                baby(e);
            },
            "entity/panda/weak_panda_baby",
        ),
        ("goat", baby, "entity/goat/goat_baby"),
        ("dolphin", baby, "entity/dolphin/dolphin_baby"),
        ("armadillo", baby, "entity/armadillo/armadillo_baby"),
    ];
    for (n, (kind, edit, texture)) in cases.into_iter().enumerate() {
        let e = entity(&k, kind, 10 + n as i32, edit);
        let expected = k.r.atlas.rect(texture).unwrap_or_else(|| panic!("{texture} missing")).map(f32::to_bits);
        let (drawn, _, _) = k.draw(&e);
        assert!(drawn.iter().any(|d| d.2 == expected), "{kind} case {n}: {texture} not drawn");
    }
}

/// No kind draws a baby model with an adult texture: no layer of a `_baby` model takes a texture
/// that the adult of the same kind draws.
#[test]
fn baby_models_take_baby_textures() {
    let Some(k) = kit() else { return };
    let kinds = k.r.source.registry("minecraft:entity_type").expect("types").entries.clone();
    let mut babies = 0;
    for kind in kinds.iter().filter_map(|n| n.strip_prefix("minecraft:")) {
        let adult: Vec<String> = k.r.layer_names(kind, false, 0).into_iter().map(|l| l.1).collect();
        for (model, texture, _, _) in k.r.layer_names(kind, true, 0) {
            // A layer whose model is missing is never drawn (the renderer skips it).
            if model.contains("_baby#") && k.r.layer_index.contains_key(&model) {
                babies += 1;
                assert!(!adult.contains(&texture), "{kind}: baby model {model} with the adult's {texture}");
            }
        }
    }
    assert!(babies > 30, "{babies} baby models");
}

/// Tall images without an animation are placed whole in the entity sheet (a 1×2 painting, the
/// witch and her hat, the strider and its saddle, the End crystal's beam); the conduit wind keeps
/// its first frame at the declared height.
#[test]
fn tall_entity_textures_are_placed_whole() {
    let Some(mut k) = kit() else { return };
    for (texture, size) in [
        ("painting/graham", [16, 32]),
        ("entity/witch/witch", [64, 128]),
        ("entity/strider/strider", [64, 128]),
        ("entity/equipment/strider_saddle/saddle", [64, 128]),
        ("entity/end_crystal/end_crystal_beam", [16, 256]),
        ("entity/conduit/wind", [64, 32]),
    ] {
        let rect = k.r.atlas.rect(texture).unwrap_or_else(|| panic!("{texture} missing"));
        assert_eq!([rect[2], rect[3]], size.map(|v| v as f32), "{texture}");
    }
}

/// The villager hats come from the textures' `.mcmeta` read once with the other entity
/// metadata: butcher partial, farmer and the desert type full, in both families.
#[test]
fn villager_hats_from_the_shared_metadata() {
    let Some(k) = kit() else { return };
    let hats = &k.r.villager_hats;
    for family in ["villager", "zombie_villager"] {
        assert_eq!(hats.get(&format!("entity/{family}/profession/butcher")), Some(&1), "{family}");
        assert_eq!(hats.get(&format!("entity/{family}/profession/farmer")), Some(&2), "{family}");
    }
    assert_eq!(hats.get("entity/villager/type/desert"), Some(&2));
    assert_eq!(hats.len(), 14, "{hats:?}");
}
