//! Tests on the real 26.3 data (prepared in the local cache). Silently ignored
//! if the cache does not exist, so as not to depend on a game installation.

use rc_data::{Bound, Phase, VersionData, flags};

fn cache() -> Option<(Vec<u8>, u64, std::path::PathBuf)> {
    let dir = rc_data::cache_root().join("26.3");
    let bytes = std::fs::read(dir.join("data.bin")).ok()?;
    let tag = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    Some((bytes, tag, dir))
}

fn data() -> Option<VersionData> {
    let (bytes, tag, _) = cache()?;
    Some(rc_data::decode(&bytes, tag).unwrap())
}

#[test]
fn round_trip_is_byte_identical() {
    let Some((bytes, tag, _)) = cache() else { return };
    let d = rc_data::decode(&bytes, tag).unwrap();
    assert_eq!(rc_data::encode(&d, tag), bytes);
}

#[test]
fn cache_matches_fresh_compilation() {
    let Some((bytes, tag, dir)) = cache() else { return };
    let d = rc_data::decode(&bytes, tag).unwrap();
    let mut fresh =
        rc_data::compile(&dir.join("facts.json"), &dir.join("generated").join("reports"), d.protocol).unwrap();
    // Dimension types come from the jar, read by the preparation and not by the compilation.
    fresh.dimension_types = d.dimension_types.clone();
    fresh.client_states = d.client_states.clone();
    fresh.biomes = d.biomes.clone();
    fresh.timelines = d.timelines.clone();
    fresh.entity_models = d.entity_models.clone();
    fresh.entity_animations = d.entity_animations.clone();
    fresh.creative = d.creative.clone();
    assert_eq!(fresh, d);
}

/// Name → identifier index against linear search, for each registry, each entry and
/// each block, on the two construction paths (decoding of `data.bin`, compilation of the
/// preparation files). Explicit failure if the 26.3 cache is missing.
#[test]
fn name_index_matches_linear_search() {
    let (bytes, tag, dir) = cache().expect("26.3 cache absent (data.bin): run the preparation");
    let decoded = rc_data::decode(&bytes, tag).unwrap();
    let compiled =
        rc_data::compile(&dir.join("facts.json"), &dir.join("generated").join("reports"), decoded.protocol).unwrap();
    for d in [&decoded, &compiled] {
        assert!(d.registries.len() > 50 && d.blocks.len() > 1000);
        let mut checked = 0usize;
        for r in &d.registries {
            assert!(std::ptr::eq(d.registry_with(&r.name, true).unwrap(), d.registry_with(&r.name, false).unwrap()));
            for e in &r.entries {
                let want = r.entries.iter().position(|x| x == e).map(|i| i as u32);
                assert_eq!(r.id_with(e, true), want, "{} : {e}", r.name);
                assert_eq!(r.id(e), want);
                checked += 1;
            }
            assert_eq!(r.id_with("minecraft:absent", true), None);
            assert_eq!(&*r.shared().iter().map(|s| s.to_string()).collect::<Vec<_>>(), r.entries.as_slice());
        }
        for b in &d.blocks {
            let want = d.blocks.iter().position(|x| x.name == b.name).map(|i| i as u32);
            assert_eq!(d.block_id_with(&b.name, true), want, "{}", b.name);
        }
        assert_eq!(d.block_id("minecraft:absent"), None);
        assert!(d.registry("minecraft:absent").is_none());
        let known = [
            (d.items(), "minecraft:item"),
            (d.entity_types(), "minecraft:entity_type"),
            (d.data_component_types(), "minecraft:data_component_type"),
            (d.potion_registry(), "minecraft:potion"),
            (d.sound_events(), "minecraft:sound_event"),
            (d.block_entity_types(), "minecraft:block_entity_type"),
            (d.particle_types(), "minecraft:particle_type"),
            (d.mob_effects(), "minecraft:mob_effect"),
        ];
        for (r, n) in known {
            assert!(std::ptr::eq(r.unwrap(), d.registry_with(n, false).unwrap()), "{n}");
        }
        assert!(checked > 7_000, "only {checked} entries");
    }
}

#[test]
fn rejects_corruption_and_stale_cache() {
    let Some((bytes, tag, _)) = cache() else { return };
    assert!(rc_data::decode(&bytes[..bytes.len() - 1], tag).is_err());
    assert!(rc_data::decode(&bytes[..bytes.len() / 2], tag).is_err());
    let mut magic = bytes.clone();
    magic[0] ^= 0xff;
    assert!(rc_data::decode(&magic, tag).is_err());
    assert!(matches!(rc_data::decode(&bytes, tag ^ 1), Err(rc_data::Error::Stale(_))));
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(rc_data::decode(&extra, tag).is_err());
}

#[test]
fn known_facts() {
    let Some(d) = data() else { return };
    assert_eq!(d.protocol, 777);
    assert_eq!(d.state_bits(), 16);
    let by_name = |n: &str| d.blocks.iter().find(|b| b.name == n).unwrap();

    let air = d.state(by_name("minecraft:air").default_state).unwrap();
    assert!(air.has(flags::AIR));
    assert!(d.shape(air.collision).is_empty());

    let stone = d.state(by_name("minecraft:stone").default_state).unwrap();
    let cube = d.shape(stone.collision);
    assert_eq!(cube.len(), 1);
    assert_eq!((cube[0].min, cube[0].max), ([0.0; 3], [1.0; 3]));
    assert_eq!(stone.light_dampening, 15);

    assert_eq!(by_name("minecraft:ice").friction, 0.98);
    assert_eq!(by_name("minecraft:soul_sand").speed_factor, 0.4);
    assert_eq!(d.state(by_name("minecraft:torch").default_state).unwrap().light_emission, 14);

    let stairs = by_name("minecraft:oak_stairs");
    let props: Vec<_> = d.state_properties(stairs.default_state).collect();
    assert_eq!(props, [("facing", "north"), ("half", "bottom"), ("shape", "straight"), ("waterlogged", "false")]);

    assert!(d.packet_id(Phase::Login, Bound::ToClient, "minecraft:login_compression").is_some());
    assert!(d.registry("minecraft:entity_type").is_some_and(|r| r.entries.len() > 100));
    let overworld = d.dimension_types.iter().find(|t| t.name == "minecraft:overworld").unwrap();
    assert_eq!((overworld.min_y, overworld.height), (-64, 384));
    assert_eq!(d.client_states.len(), d.states.len());
    let glass = d.client_states[by_name("minecraft:glass").default_state as usize];
    assert_eq!(glass.skip_self, 0b11_1111);
    let grass = d.client_states[by_name("minecraft:grass_block").default_state as usize];
    assert_eq!(grass.tint_kind[0], rc_data::tint::GRASS);
    // Block pieces: grass untinted, water in the biome's water colour; falling dust:
    // sand's own colour, black for the dragon egg, stone's map colour.
    use rc_data::tint::{CONSTANT, NONE, WATER};
    let cs = |n: &str| d.client_states[by_name(n).default_state as usize];
    assert_eq!(grass.particle_tint, (CONSTANT, 0xffff_ffff));
    assert_eq!(cs("minecraft:water").particle_tint, (WATER, 0x15));
    assert_eq!(cs("minecraft:stone").particle_tint.0, NONE);
    assert_eq!(cs("minecraft:sand").dust, (CONSTANT, 0xdb_d3a0));
    assert_eq!(cs("minecraft:dragon_egg").dust, (CONSTANT, 0xff00_0000));
    assert_eq!(cs("minecraft:stone").dust, (CONSTANT, 0x70_7070));
    let swamp = d.biomes.iter().find(|b| b.name == "minecraft:swamp").unwrap();
    assert_eq!((swamp.grass_modifier, swamp.water_color, swamp.sky_color), (2, 0x617b64, Some(0x78a7ff)));
}

#[test]
fn environment_attributes_and_timelines() {
    let Some(d) = data() else { return };
    let ow = d.dimension_types.iter().find(|t| t.name == "minecraft:overworld").unwrap();
    let fog = ow.attributes.iter().find(|e| e.attr == "visual/fog_color").unwrap();
    assert_eq!((fog.op, fog.arg), (rc_data::EnvOp::Override, [192.0 / 255.0, 216.0 / 255.0, 1.0, 1.0]));
    assert_eq!(
        ow.timelines,
        ["minecraft:villager_schedule", "minecraft:day", "minecraft:moon", "minecraft:early_game"]
    );
    assert!(ow.timelines.iter().any(|n| n == "minecraft:day"));
    assert_eq!((ow.skybox, ow.has_skylight, ow.has_ceiling), (1, true, false));
    let nether = d.dimension_types.iter().find(|t| t.name == "minecraft:the_nether").unwrap();
    let end = d.dimension_types.iter().find(|t| t.name == "minecraft:the_end").unwrap();
    assert_eq!((ow.ambient_light, nether.ambient_light, end.ambient_light), (0.0, 0.1, 0.25));
    assert_eq!(nether.skybox, 0);
    let day = d.timelines.iter().find(|t| t.name == "minecraft:day").unwrap();
    assert_eq!((day.clock.as_str(), day.period), ("minecraft:overworld", 24000));
    let sky = day.tracks.iter().find(|t| t.attr == "visual/sky_color").unwrap();
    assert_eq!((sky.op, sky.keys.len(), sky.keys[2].0), (rc_data::EnvOp::Multiply, 4, 13670));
    let sun = day.tracks.iter().find(|t| t.attr == "visual/sun_angle").unwrap();
    assert_eq!(sun.ease, rc_data::Ease::Bezier([0.362, 0.241, 0.638, 0.759]));
    let ocean = d.biomes.iter().find(|b| b.name == "minecraft:warm_ocean").unwrap();
    let w = ocean.attributes.iter().find(|e| e.attr == "visual/water_fog_color").unwrap();
    assert_eq!(w.arg, [4.0 / 255.0, 31.0 / 255.0, 51.0 / 255.0, 1.0]);
    let swamp = d.biomes.iter().find(|b| b.name == "minecraft:swamp").unwrap();
    let end = swamp.attributes.iter().find(|e| e.attr == "visual/water_fog_end_distance").unwrap();
    assert_eq!((end.op, end.arg[0]), (rc_data::EnvOp::Multiply, 0.85));
    assert!(!d.biomes.iter().find(|b| b.name == "minecraft:desert").unwrap().has_precipitation);
    let frozen: Vec<_> = d.biomes.iter().filter(|b| b.frozen).map(|b| b.name.as_str()).collect();
    assert_eq!(frozen, ["minecraft:deep_frozen_ocean", "minecraft:frozen_ocean"]);
}

#[test]
fn light_bits() {
    let Some(d) = data() else { return };
    let bits = |n: &str| d.states[d.blocks.iter().find(|b| b.name == n).unwrap().default_state as usize].light;
    use rc_data::light_bits::{DARK_SHADE, EMISSIVE};
    assert_eq!(bits("minecraft:magma_block") & EMISSIVE, EMISSIVE);
    assert_eq!(bits("minecraft:stone"), DARK_SHADE);
    assert_eq!(bits("minecraft:mud"), DARK_SHADE);
    assert_eq!(bits("minecraft:glass"), 0);
    assert_eq!(bits("minecraft:barrier"), 0);
    assert_eq!(bits("minecraft:oak_slab"), 0);
}

#[test]
fn item_component_registries() {
    let Some(d) = data() else { return };
    let dct = &d.registry("minecraft:data_component_type").unwrap().entries;
    assert_eq!(dct.len(), 122);
    assert_eq!(
        (dct[0].as_str(), dct[24].as_str(), dct[77].as_str()),
        ("minecraft:custom_data", "minecraft:consumable", "minecraft:container")
    );
    assert_eq!(dct[121], "minecraft:cushion/color");
    let ce = &d.registry("minecraft:consume_effect_type").unwrap().entries;
    assert_eq!(
        ce,
        &[
            "minecraft:apply_effects",
            "minecraft:remove_effects",
            "minecraft:clear_all_effects",
            "minecraft:teleport_randomly",
            "minecraft:play_sound"
        ]
    );
}

#[test]
fn item_defaults() {
    let Some(d) = data() else { return };
    let reg = &d.registry("minecraft:item").unwrap().entries;
    assert_eq!(d.items.len(), reg.len());
    let at = |n: &str| &d.items[reg.iter().position(|e| e == n).unwrap()];
    let sword = at("minecraft:diamond_sword");
    assert_eq!((sword.max_stack_size, sword.max_damage, sword.rarity), (1, 1561, 0));
    assert_eq!(
        (sword.name_key.as_str(), sword.model.as_str()),
        ("item.minecraft.diamond_sword", "minecraft:diamond_sword")
    );
    assert_eq!(at("minecraft:nether_star").glint, 2);
    assert_eq!(at("minecraft:stone").name_key, "block.minecraft.stone");
    assert_eq!(at("minecraft:ender_pearl").max_stack_size, 16);
}

#[test]
fn potion_colors() {
    let Some(d) = data() else { return };
    let potions = &d.registry("minecraft:potion").unwrap().entries;
    let effects = &d.registry("minecraft:mob_effect").unwrap().entries;
    let healing = &d.potions[potions.iter().position(|p| p == "minecraft:strong_healing").unwrap()];
    assert_eq!(healing.len(), 1);
    assert_eq!(
        (effects[healing[0].effect as usize].as_str(), healing[0].amplifier, healing[0].visible),
        ("minecraft:instant_health", 1, true)
    );
    assert!(d.potions[potions.iter().position(|p| p == "minecraft:water").unwrap()].is_empty());
    assert_eq!(d.effect_colors.len(), effects.len());
}

#[test]
fn entity_models() {
    let Some(d) = data() else { return };
    if d.entity_models.is_empty() {
        return;
    }
    let zombie = d.entity_layer("minecraft:zombie#main").unwrap();
    assert_eq!(zombie.texture, [64, 64]);
    let leg = zombie.parts.iter().find(|p| p.name == "left_leg").unwrap();
    assert_eq!(leg.pose[..3], [1.9, 12.0, 0.0]);
    assert!(leg.cubes[0].mirror);
    assert_eq!(zombie.parts[0].parent, u32::MAX);
    assert!(d.entity_models.len() > 300);
}

#[test]
fn entity_animations() {
    let Some(d) = data() else { return };
    if d.entity_animations.is_empty() {
        return;
    }
    assert!(d.entity_animations.len() >= 70);
    let walk = d.entity_animation("camel.walk").unwrap();
    assert_eq!((walk.length, walk.looping), (1.5, true));
    let leg = walk
        .channels
        .iter()
        .find(|c| c.part == "right_front_leg" && c.target == rc_data::AnimTarget::Rotation)
        .unwrap();
    // 22.5° in radians, Catmull-Rom, identical before and after values.
    let k = leg.keyframes[0];
    assert!((k.post[0] - 22.5f32.to_radians()).abs() < 1e-6 && k.catmull && k.pre == k.post);
    assert!(d.entity_animation("warden.emerge").is_some_and(|a| !a.looping));
    // No empty channel.
    for a in &d.entity_animations {
        assert!(a.channels.iter().all(|c| !c.keyframes.is_empty()), "{}", a.name);
    }
}

#[test]
fn item_equippable() {
    let Some(d) = data() else { return };
    let reg = &d.registry("minecraft:item").unwrap().entries;
    let at = |n: &str| &d.items[reg.iter().position(|e| e == n).unwrap()];
    let helmet = at("minecraft:iron_helmet");
    assert_eq!((helmet.equip_slot, helmet.equip_asset.as_str()), (5, "minecraft:iron"));
    assert_eq!(at("minecraft:leather_leggings").equip_slot, 3);
    assert_eq!(at("minecraft:stone").equip_slot, 0);
    let pumpkin = at("minecraft:carved_pumpkin");
    assert_eq!((pumpkin.equip_slot, pumpkin.equip_overlay.as_str()), (5, "minecraft:misc/pumpkinblur"));
    assert!(helmet.equip_overlay.is_empty());
}

#[test]
fn entity_dims() {
    let Some(d) = data() else { return };
    let reg = &d.registry("minecraft:entity_type").unwrap().entries;
    let at = |n: &str| d.entity_dims[reg.iter().position(|e| e == n).unwrap()];
    assert_eq!(d.entity_dims.len(), reg.len());
    assert_eq!(at("minecraft:cow"), [0.9, 1.4, 1.3, 1.4]);
    assert_eq!(at("minecraft:player"), [0.6, 1.8, 1.62, 1.8]);
}
