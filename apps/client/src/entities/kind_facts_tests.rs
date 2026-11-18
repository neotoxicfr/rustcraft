//! Facts of every type of the 26.3 registry against the name rules they replace (the pick rule,
//! the movement family and the push rule are checked by their own modules).

use crate::entities::Entities;
use rc_client::EntityEvent;
use rc_proto_v777::entity::AddEntity;

/// Every registry type spawned once (id = type id).
fn every_type() -> Entities {
    let data = rc_testdata::version_data().expect("version data missing (extraction)");
    let names = data.registry("minecraft:entity_type").expect("type registry").entries.clone();
    assert!(names.len() > 100, "type registry too short: {}", names.len());
    let mut ents = Entities { type_names: names.clone(), ..Default::default() };
    for i in 0..names.len() as i32 {
        let add = AddEntity { id: i, uuid: 0, kind: i, pos: [0.0; 3], velocity: [0.0; 3], pitch: 0.0, yaw: 0.0, head_yaw: 0.0, data: 0 };
        ents.apply(EntityEvent::Add(Box::new(add)));
    }
    ents
}

/// Kinds whose facts differ from the name rules.
fn mismatches(ents: &Entities) -> Vec<String> {
    let rules = |e: &crate::entities::Entity| {
        let k = e.kind.as_str();
        let f = e.facts;
        [
            f.blocks_building == crate::interact::blocks_building_by_name(e),
            f.vehicle_like == (k.ends_with("boat") || k.ends_with("raft") || k.ends_with("minecart")),
            f.player == (k == "player"),
            f.trident == (k == "trident"),
            f.fishing_bobber == (k == "fishing_bobber"),
            f.item_frame == matches!(k, "item_frame" | "glow_item_frame"),
            f.box_cacheable == !matches!(k, "painting" | "shulker" | "polar_bear"),
        ]
    };
    ents.map.values().filter(|e| rules(e).contains(&false)).map(|e| e.kind.clone()).collect()
}

#[test]
fn facts_of_every_registry_type_match_the_names() {
    let mut ents = every_type();
    let differ = mismatches(&ents);
    assert!(differ.is_empty(), "{differ:?}");
    // Sabotage: a minecart that no longer blocks building is caught.
    let cart = ents.map.values_mut().find(|e| e.kind == "minecart").expect("minecart");
    cart.facts.blocks_building = false;
    assert_eq!(mismatches(&ents), vec!["minecart".to_owned()]);
}
