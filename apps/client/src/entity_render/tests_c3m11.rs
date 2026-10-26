//! Equivalence proofs of the entity-rendering switches (`EntOpt`): kind and item tables
//! against the old predicates, `MetaMap` index against the search, product by the identity, and
//! golden snapshot of `prepare` (≈ 90 entities of varied kinds, 26.3 data and game, windowless
//! GPU) identical under each switch turned off. Without GPU, data or game:
//! test skipped (failure under `RUSTCRAFT_OPT_REQUIRE=1`).

use super::tests_g2f03::{Kit, kit};
use super::*;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::item::ItemStack;

/// FNV-1a 64-bit fingerprint.
fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

/// Zeros brought back to +0 (the sign of a zero changes no pixel; see `identity_product_keeps_matrix`).
fn canon(v: f32) -> u32 {
    (v + 0.0).to_bits()
}

fn mat_bits(m: &PartMat) -> Vec<u32> {
    m.m.iter().flatten().chain(m.n.iter().flatten()).map(|&v| canon(v)).collect()
}

/// Fingerprint of everything `prepare` placed: matrices, instances per pass, silhouettes,
/// shadows, free geometry, crystals, number of entities drawn.
fn digest(r: &EntityRenderer) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for m in &r.mats {
        for b in mat_bits(m) {
            fnv(&mut h, &b.to_le_bytes());
        }
    }
    for (p, list) in r.instances.iter().enumerate() {
        fnv(&mut h, &(p as u32).to_le_bytes());
        for (src, first, count, inst) in list {
            fnv(&mut h, &[*src]);
            fnv(&mut h, &first.to_le_bytes());
            fnv(&mut h, &count.to_le_bytes());
            fnv(&mut h, bytemuck::bytes_of(inst));
        }
    }
    for (src, first, count, inst) in &r.glow_only {
        fnv(&mut h, &[*src]);
        fnv(&mut h, &first.to_le_bytes());
        fnv(&mut h, &count.to_le_bytes());
        fnv(&mut h, bytemuck::bytes_of(inst));
    }
    fnv(&mut h, bytemuck::cast_slice(&r.shadow_verts));
    for v in &r.free_verts {
        fnv(&mut h, bytemuck::cast_slice(v));
    }
    for (p, t) in &r.crystals {
        for c in p {
            fnv(&mut h, &c.to_le_bytes());
        }
        fnv(&mut h, &t.to_le_bytes());
    }
    fnv(&mut h, &(r.drawn as u64).to_le_bytes());
    h
}

/// Kinds of the scene (all present in the 26.3 registry, otherwise failure).
const KINDS: &[&str] = &[
    "zombie",
    "husk",
    "drowned",
    "zombie_villager",
    "zombified_piglin",
    "skeleton",
    "stray",
    "bogged",
    "wither_skeleton",
    "piglin",
    "piglin_brute",
    "player",
    "mannequin",
    "armor_stand",
    "villager",
    "iron_golem",
    "snow_golem",
    "cow",
    "pig",
    "chicken",
    "sheep",
    "wolf",
    "cat",
    "frog",
    "horse",
    "donkey",
    "skeleton_horse",
    "llama",
    "mooshroom",
    "rabbit",
    "parrot",
    "polar_bear",
    "axolotl",
    "salmon",
    "pufferfish",
    "tropical_fish",
    "ghast",
    "happy_ghast",
    "slime",
    "magma_cube",
    "creeper",
    "spider",
    "cave_spider",
    "enderman",
    "blaze",
    "vex",
    "allay",
    "glow_squid",
    "squid",
    "illusioner",
    "evoker",
    "vindicator",
    "witch",
    "phantom",
    "shulker",
    "ender_dragon",
    "wither",
    "end_crystal",
    "guardian",
    "bee",
    "fox",
    "goat",
    "camel",
    "sniffer",
    "warden",
    "breeze",
    "turtle",
    "strider",
    "hoglin",
    "zoglin",
    "copper_golem",
    "arrow",
    "spectral_arrow",
    "trident",
    "snowball",
    "item",
    "experience_orb",
    "painting",
    "item_frame",
    "minecart",
    "oak_boat",
    "tnt",
    "lightning_bolt",
    "firework_rocket",
    "leash_knot",
    "wind_charge",
    "llama_spit",
    "shulker_bullet",
    "wither_skull",
    "evoker_fangs",
    "area_effect_cloud",
    "eye_of_ender",
];

fn text(s: &str) -> Meta {
    Meta::Text(Some(rc_proto::nbt::Nbt::String(s.into())))
}

/// The scene: kinds of `KINDS` in a grid in front of the camera, real ids, varied states
/// (babies, burning, named, equipped, invisible).
fn scene(k: &Kit) -> Vec<Entity> {
    let names = k.r.source.registry("minecraft:entity_type").expect("type registry").entries.clone();
    let mut all = crate::entities::Entities { type_names: names.clone(), ..Default::default() };
    let stack = |n: &str| Some(ItemStack { item: k.item(n), count: 1, added: vec![], removed: vec![] });
    let mut out = Vec::new();
    for (i, kind) in KINDS.iter().enumerate() {
        let id = names
            .iter()
            .position(|n| n.strip_prefix("minecraft:") == Some(kind))
            .unwrap_or_else(|| panic!("kind {kind} missing from the 26.3 registry"));
        let add = rc_proto_v777::entity::AddEntity {
            id: i as i32 + 1,
            uuid: 5000 + i as u128,
            kind: id as i32,
            pos: [(i % 10) as f64 * 2.5 - 11.0, 64.0 + (i % 3) as f64 * 0.3, 6.0 + (i / 10) as f64 * 3.0],
            velocity: [0.0; 3],
            pitch: (i as f32 * 7.0) % 60.0 - 30.0,
            yaw: i as f32 * 37.0,
            head_yaw: i as f32 * 41.0,
            data: 0,
        };
        all.apply(rc_client::EntityEvent::Add(Box::new(add)));
        let mut e = all.map.remove(&(i as i32 + 1)).expect("entity added");
        e.age = 100 + i as u32 * 13;
        e.body_o = e.body - 5.0;
        e.pos_o = [e.pos[0] - 0.1, e.pos[1], e.pos[2] - 0.05];
        match *kind {
            "zombie" => {
                e.equipment[0] = stack("minecraft:iron_sword");
                e.equipment[4] = stack("minecraft:diamond_chestplate");
                e.equipment[5] = stack("minecraft:iron_helmet");
                e.meta.insert(0, Meta::Byte(0x01));
            }
            "husk" | "cow" | "pig" | "chicken" => e.meta.insert(16, Meta::Bool(true)),
            "drowned" => {
                e.equipment[0] = stack("minecraft:trident");
                e.meta.insert(15, Meta::Byte(4));
            }
            "skeleton" | "stray" => {
                e.equipment[0] = stack("minecraft:bow");
                e.meta.insert(15, Meta::Byte(4));
            }
            "bogged" => e.equipment[0] = stack("minecraft:iron_spear"),
            "piglin" => e.equipment[0] = stack("minecraft:crossbow"),
            "player" => {
                e.equipment[0] = stack("minecraft:shield");
                e.meta.insert(8, Meta::Byte(1));
            }
            "mannequin" => e.equipment[1] = stack("minecraft:crossbow"),
            "sheep" => {
                e.meta.insert(2, text("jeb_"));
                e.meta.insert(17, Meta::Byte(5));
            }
            "wolf" => {
                e.meta.insert(18, Meta::Byte(4));
                e.meta.insert(24, Meta::Variant(1, 25));
                e.equipment[6] = stack("minecraft:wolf_armor");
            }
            "cat" => e.meta.insert(19, Meta::Variant(2, 21)),
            "horse" => {
                e.equipment[6] = stack("minecraft:iron_horse_armor");
                e.equipment[7] = stack("minecraft:saddle");
            }
            "rabbit" => e.meta.insert(2, text("Toast")),
            "parrot" => e.meta.insert(2, text("Dinnerbone")),
            "creeper" => e.meta.insert(17, Meta::Bool(true)),
            "blaze" => e.meta.insert(16, Meta::Byte(1)),
            "spider" | "illusioner" => e.meta.insert(0, Meta::Byte(0x20)),
            "enderman" => e.meta.insert(0, Meta::Byte(0x21)),
            "item" => e.meta.insert(8, Meta::Item(stack("minecraft:apple"))),
            "experience_orb" => e.meta.insert(8, Meta::Int(40)),
            "slime" | "magma_cube" => e.meta.insert(16, Meta::Int(3)),
            _ => {}
        }
        out.push(e);
    }
    out
}

/// Prepares the scene and returns its fingerprint.
fn run(k: &mut Kit, ents: &[Entity], opt: EntOpt) -> u64 {
    prep(k, ents, opt);
    digest(&k.r)
}

/// Prepares the scene (fixed world, light and camera).
fn prep(k: &mut Kit, ents: &[Entity], opt: EntOpt) {
    k.r.opt = opt;
    let state = |x: i32, y: i32, z: i32| if y < 64 && (x + z) & 3 == 0 { 1u32 } else { 0 };
    let bright = |x: i32, y: i32, z: i32| (x * 3 + y + z * 5).rem_euclid(16);
    let tints = |_: i32, _: i32, _: i32| [0xff_ffff; 4];
    let world = ShadowWorld { state: &state, brightness: &bright, ambient: 0.1, biome_tints: &tints };
    let light = |x: i32, y: i32, z: i32| ((x * 7 + y * 3 + z).rem_euclid(16) as u8, (x + y * 5 + z * 3).rem_euclid(16) as u8);
    k.r.prepare(ents.iter(), 0.37, [0, 0], [0.5, 66.0, -2.5], &light, &|_, _| true, Some(&k.items), &world);
}

/// Expected fingerprint of the scene (26.3 data and game): any change to what `prepare`
/// places makes it fail. Batch 5 integration: 0xf3e2_3c99_f7b8_6fb5 -> current value, sole
/// cause G2-F05 fidelity (items and blocks drawn as entities; fixed white biome tint
/// in `prep`), verified on c3-m11 + g2-f05 alone: same value, all variants equal.
const GOLDEN: u64 = 0x7f87_d9a9_7d93_dba3;

/// `prepare` places exactly the same thing under each switch turned off, alone or all, and
/// the fingerprint is the kept one.
#[test]
fn prepare_snapshot_same_under_every_switch() {
    let Some(mut k) = kit() else { return };
    let ents = scene(&k);
    // Invisible glowing entity and ghost seen by the local player.
    k.r.glow.insert(5000 + KINDS.iter().position(|n| *n == "spider").unwrap() as u128, 0xff00ff);
    k.r.ghosts.insert(5000 + KINDS.iter().position(|n| *n == "enderman").unwrap() as u128);
    let all_new = EntOpt {
        equip_flag: true,
        kind_info: true,
        size_once: true,
        obj_early: true,
        light_once: true,
        reg_idx: true,
        item_flags: true,
    };
    // First pass: caches (layers, item meshes, atlas) filled.
    run(&mut k, &ents, all_new);
    let reference = run(&mut k, &ents, EntOpt::OLD);
    assert!(k.r.drawn > 60, "scene almost empty: {} entities drawn", k.r.drawn);
    assert!(k.r.mats.len() > 500, "{} matrices", k.r.mats.len());
    let new = run(&mut k, &ents, all_new);
    assert_eq!(new, reference, "all the switches");
    let off = |f: &dyn Fn(&mut EntOpt)| {
        let mut o = all_new;
        f(&mut o);
        o
    };
    for (name, o) in [
        ("EQUIPFLAG", off(&|o| o.equip_flag = false)),
        ("KINDINFO", off(&|o| o.kind_info = false)),
        ("SIZEONCE", off(&|o| o.size_once = false)),
        ("OBJEARLY", off(&|o| o.obj_early = false)),
        ("LIGHTONCE", off(&|o| o.light_once = false)),
        ("REGIDX", off(&|o| o.reg_idx = false)),
        ("ITEMFLAGS", off(&|o| o.item_flags = false)),
    ] {
        assert_eq!(run(&mut k, &ents, o), reference, "{name}=0");
    }
    // Each track alone.
    for (name, o) in [
        ("EQUIPFLAG", EntOpt { equip_flag: true, ..EntOpt::OLD }),
        ("KINDINFO", EntOpt { kind_info: true, ..EntOpt::OLD }),
        ("SIZEONCE", EntOpt { size_once: true, ..EntOpt::OLD }),
        ("OBJEARLY", EntOpt { obj_early: true, ..EntOpt::OLD }),
        ("LIGHTONCE", EntOpt { light_once: true, ..EntOpt::OLD }),
    ] {
        assert_eq!(run(&mut k, &ents, o), reference, "{name} alone");
    }
    assert_eq!(reference, GOLDEN, "scene fingerprint changed: {reference:#x}");
}

/// Kind table (by type id) = old string predicates for every registry name; family = first
/// skin function that answers; item kind = `push_object` answers; the table follows the ids
/// (kind changed by hand: recomputed).
#[test]
fn kind_table_matches_predicates() {
    let Some(mut k) = kit() else { return };
    let names = k.r.source.registry("minecraft:entity_type").expect("types").entries.clone();
    assert!(names.len() > 100);
    let mut all = crate::entities::Entities { type_names: names.clone(), ..Default::default() };
    k.r.opt = EntOpt::OLD;
    for (i, full) in names.iter().enumerate() {
        let n = full.strip_prefix("minecraft:").unwrap_or(full);
        let info = KindInfo::of(n);
        let old: [(u32, bool); 21] = [
            (K_BIG_BOX, matches!(n, "ender_dragon" | "ghast" | "happy_ghast" | "giant" | "wither")),
            (K_ILLUSIONER, n == "illusioner"),
            (K_DISPLAY, matches!(n, "block_display" | "item_display" | "text_display")),
            (K_ALWAYS, matches!(n, "lightning_bolt" | "fishing_bobber" | "ender_dragon")),
            (K_END_CRYSTAL, n == "end_crystal"),
            (K_ENDER_DRAGON, n == "ender_dragon"),
            (K_OBJECT, false),
            (K_PAINTING, n == "painting"),
            (K_ORB, n == "experience_orb"),
            (K_ITEM, n == "item"),
            (K_FLAME_IMMUNE, FLAME_IMMUNE.contains(&n)),
            (K_BLAZE, n == "blaze"),
            (K_AVATAR, n == "player" || n == "mannequin"),
            (K_PLAYER, n == "player"),
            (K_MANNEQUIN, n == "mannequin"),
            (
                K_SPEAR_MOB,
                matches!(n, "zombie" | "husk" | "zombie_villager" | "drowned" | "zombified_piglin") || is_skeleton(n),
            ),
            (K_SKELETON, is_skeleton(n)),
            (K_DROWNED, n == "drowned"),
            (K_PIGLIN, n == "piglin" || n == "piglin_brute"),
            (K_SELF_LIT, matches!(n, "vex" | "magma_cube" | "wither" | "blaze" | "allay")),
            (K_GLOW_SQUID, n == "glow_squid"),
        ];
        for (f, want) in old {
            if f != K_OBJECT {
                assert_eq!(info.has(f), want, "{n}: flag {f:#x}");
            }
        }
        // Real entity of this kind: skin of the families, item, table by id.
        let add = rc_proto_v777::entity::AddEntity {
            id: 1,
            uuid: 77,
            kind: i as i32,
            pos: [0.5, 64.0, 3.5],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(rc_client::EntityEvent::Add(Box::new(add)));
        let e = all.map.remove(&1).expect("entity");
        assert_eq!(e.kind, n);
        assert_eq!(k.r.kind_of(&e), info, "{n}: table by id");
        let fams = [
            animals::skin(&k.r, &e).is_some(),
            creatures::skin(&k.r, &e).is_some(),
            dragon::skin(&k.r, &e).is_some(),
            wither::skin(&k.r, &e).is_some(),
            shulker::skin(&k.r, &e).is_some(),
        ];
        let first = fams.iter().position(|&b| b).map_or(0, |i| i as u8 + 1);
        assert_eq!(info.family, first, "{n}: family");
        let light = |_: i32, _: i32, _: i32| (0u8, 15u8);
        let state = |_: i32, _: i32, _: i32| 0u32;
        let bright = |_: i32, _: i32, _: i32| 15i32;
        let tints = |_: i32, _: i32, _: i32| [0xff_ffff; 4];
        let world = ShadowWorld { state: &state, brightness: &bright, ambient: 0.0, biome_tints: &tints };
        let drawn = k.r.push_object(&e, 0.0, [0.0; 3], e.pos, [0.5, 65.0, 0.5], &light, Some(&k.items), &world);
        assert_eq!(info.has(K_OBJECT), drawn.is_some(), "{n}: item kind");
        let mut renamed = e.clone();
        renamed.kind = if n == "cow" { "zombie".into() } else { "cow".into() };
        assert_eq!(k.r.kind_of(&renamed), KindInfo::of(&renamed.kind), "{n} renamed");
    }
    // Visible sabotage: a changed kind in the table would no longer pass.
    assert_ne!(KindInfo::of("zombie"), KindInfo::of("husk_"));
}

/// Flags per item = old predicates on the name, for every registry item.
#[test]
fn item_bits_match_names() {
    let Some(data) = rc_testdata::version_data() else { return };
    let items = data.registry("minecraft:item").expect("items");
    let mut seen = [0usize; 4];
    for n in &items.entries {
        let b = item_bits(n);
        let short = n.strip_prefix("minecraft:");
        for (k, (bit, want)) in [
            (I_CROSSBOW, short == Some("crossbow")),
            (I_BOW, short == Some("bow")),
            (I_TRIDENT, short == Some("trident")),
            (I_SPEAR, n.ends_with("_spear")),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(b & bit != 0, want, "{n}");
            seen[k] += usize::from(want);
        }
    }
    assert_eq!(&seen[..3], &[1, 1, 1]);
    assert!(seen[3] >= 5, "spears: {}", seen[3]);
    for n in ["crossbow", "bow", "trident"] {
        assert_ne!(item_bit_of(n), 0);
    }
    assert_eq!(item_bit_of("shield"), 0);
}

/// Product by the identity: same matrix up to the sign of zeros (no pixel changes),
/// normal matrix included; a negative zero may become positive.
#[test]
fn identity_product_keeps_matrix() {
    let mut seed: u64 = 0x5eed_c311;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 40) as f32 / (1u64 << 24) as f32) * 8.0 - 4.0
    };
    for i in 0..2000 {
        let mut c: [f32; 16] = std::array::from_fn(|_| next());
        // Zeros of both signs at varied places.
        c[i % 16] = if i % 2 == 0 { -0.0 } else { 0.0 };
        c[(i * 7 + 3) % 16] = -0.0;
        let m = Mat4::from_cols_array(&c);
        let a = PartMat::new(&(Mat4::IDENTITY * m));
        let b = PartMat::new(&m);
        assert_eq!(mat_bits(&a), mat_bits(&b), "{c:?}");
    }
    // The sign of a zero may differ without canonicalisation (reason for `canon`).
    let m = Mat4::from_cols_array(&[-0.0, 1.0, 2.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let p = Mat4::IDENTITY * m;
    assert_eq!(canon(p.x_axis.x), canon(m.x_axis.x));
}

/// `MetaMap` index: variant at the smallest index carrying the serializer and custom name
/// as plain text, identical to the search and to the analysis after random sequences of writes
/// (mixed types, rewrites of the same index, name received then lost).
#[test]
fn meta_map_indexes_match_scan() {
    let mut seed: u64 = 0x5eed_c311;
    let mut next = |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let names = ["Dinnerbone", "Grumm", "jeb_", "Toast", "§cRed"];
    let mut all = crate::entities::Entities { type_names: vec!["minecraft:cow".into()], ..Default::default() };
    let add = rc_proto_v777::entity::AddEntity {
        id: 1,
        uuid: 1,
        kind: 0,
        pos: [0.0; 3],
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: 0,
    };
    all.apply(rc_client::EntityEvent::Add(Box::new(add)));
    let mut e = all.map.remove(&1).expect("entity");
    for step in 0..20_000 {
        let i = next(24) as u8;
        let v = match next(6) {
            0 | 1 => Meta::Variant(next(5) as i32, [21, 23, 25, 28][next(4) as usize]),
            2 => Meta::Int(next(9) as i32),
            3 => Meta::Byte(next(3) as i8),
            4 => text(names[next(5) as usize]),
            _ => Meta::Text(None),
        };
        e.meta.insert(i, v);
        for ser in [21, 23, 25, 27, 28, 30] {
            assert_eq!(e.meta.variant_indexed(ser), e.variant_scan(ser), "step {step}, serializer {ser}");
        }
        let parsed = crate::entities::parse_custom_name(e.meta.get(&2));
        assert_eq!(e.meta.custom_name(), parsed.as_deref(), "step {step}");
    }
    // Name lost.
    e.meta.insert(2, Meta::Text(None));
    assert_eq!(e.meta.custom_name(), None);
    e.meta.insert(2, text("Grumm"));
    assert_eq!(e.meta.custom_name(), Some("Grumm"));
}

/// `prepare` micro-bench (main thread, 1 core): scene copied up to 100, 1,000 and
/// 5,000 entities, each configuration measured alternately, median in µs per call.
/// `cargo test --release -p rc-app -- --ignored bench_entity_prepare --nocapture`.
#[test]
#[ignore]
fn bench_entity_prepare() {
    let Some(mut k) = kit() else { return };
    let base = scene(&k);
    let all_new = EntOpt {
        equip_flag: true,
        kind_info: true,
        size_once: true,
        obj_early: true,
        light_once: true,
        reg_idx: true,
        item_flags: true,
    };
    let configs = [
        ("old", EntOpt::OLD),
        ("all", all_new),
        ("EQUIPFLAG", EntOpt { equip_flag: true, ..EntOpt::OLD }),
        ("KINDINFO", EntOpt { kind_info: true, ..EntOpt::OLD }),
        ("SIZEONCE", EntOpt { size_once: true, ..EntOpt::OLD }),
        ("OBJEARLY", EntOpt { obj_early: true, ..EntOpt::OLD }),
        ("LIGHTONCE", EntOpt { light_once: true, ..EntOpt::OLD }),
        ("REGIDX+ITEMFLAGS", EntOpt { reg_idx: true, item_flags: true, ..EntOpt::OLD }),
        ("OBJEARLY+SIZEONCE", EntOpt { obj_early: true, size_once: true, ..EntOpt::OLD }),
        ("all except LIGHTONCE", EntOpt { light_once: false, ..all_new }),
        ("all except KINDINFO", EntOpt { kind_info: false, ..all_new }),
        ("all except SIZEONCE", EntOpt { size_once: false, ..all_new }),
        ("all except OBJEARLY", EntOpt { obj_early: false, ..all_new }),
        ("all except ITEMFLAGS", EntOpt { reg_idx: false, item_flags: false, ..all_new }),
    ];
    for n in [100usize, 1000, 5000] {
        let mut ents = Vec::with_capacity(n);
        let mut c = 0;
        while ents.len() < n {
            for e in &base {
                if ents.len() == n {
                    break;
                }
                let mut e = e.clone();
                e.id += 1000 * c;
                e.uuid += 100_000 * c as u128;
                let d = [((c % 3) as f64 - 1.0) * 26.0, ((c / 9) as f64 - 3.0) * 8.0, ((c / 3) % 3) as f64 * 10.0];
                for (i, d) in d.into_iter().enumerate() {
                    e.pos[i] += d;
                    e.pos_o[i] += d;
                }
                ents.push(e);
            }
            c += 1;
        }
        let rounds = if n >= 5000 { 31 } else { 101 };
        let mut times = vec![Vec::new(); configs.len()];
        for _ in 0..3 {
            for (_, o) in &configs {
                prep(&mut k, &ents, *o);
            }
        }
        for _ in 0..rounds {
            for (i, (_, o)) in configs.iter().enumerate() {
                let t = std::time::Instant::now();
                prep(&mut k, &ents, *o);
                times[i].push(t.elapsed().as_secs_f64() * 1e6);
            }
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let old = med(&mut times[0]);
        println!("{n} entities ({} drawn):", k.r.drawn);
        for (i, (name, _)) in configs.iter().enumerate() {
            let m = med(&mut times[i]);
            println!("  {name:>22}: {m:>9.1} µs ({:+.1}%)", (m / old - 1.0) * 100.0);
        }
    }
}

/// Micro-bench of the per-frame reads of `MetaMap` (variant of a cow, name of a named sheep):
/// index kept at reception against search and parsing at each read, ns per read.
#[test]
#[ignore]
fn bench_meta_lookups() {
    let mut all = crate::entities::Entities { type_names: vec!["minecraft:cow".into()], ..Default::default() };
    let add = rc_proto_v777::entity::AddEntity {
        id: 1,
        uuid: 1,
        kind: 0,
        pos: [0.0; 3],
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: 0,
    };
    all.apply(rc_client::EntityEvent::Add(Box::new(add)));
    let mut e = all.map.remove(&1).expect("entity");
    for i in 0..17 {
        e.meta.insert(i, Meta::Byte(0));
    }
    e.meta.insert(17, Meta::Variant(2, 23));
    e.meta.insert(2, text("Dinnerbone"));
    let n = 200_000;
    let time = |f: &dyn Fn() -> bool| {
        let t = std::time::Instant::now();
        let mut hits = 0;
        for _ in 0..n {
            hits += usize::from(std::hint::black_box(f()));
        }
        assert_eq!(hits, n);
        t.elapsed().as_secs_f64() * 1e9 / n as f64
    };
    for pass in 0..2 {
        let scan = time(&|| e.variant_scan(std::hint::black_box(23)) == Some(2));
        let idx = time(&|| e.meta.variant_indexed(std::hint::black_box(23)) == Some(2));
        let parse = time(&|| crate::entities::parse_custom_name(e.meta.get(&2)).is_some_and(|s| s == "Dinnerbone"));
        let kept = time(&|| e.meta.custom_name().is_some_and(|s| s == "Dinnerbone"));
        println!("pass {pass}: variant {scan:.1} -> {idx:.1} ns; name {parse:.1} -> {kept:.1} ns");
    }
}
