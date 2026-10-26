//! Entity objects and maps (G2-F05): expected values written from the 26.3 game (formulas
//! copied in our own words), then end-to-end wiring through the GPU kit of G2-F03 (extracted
//! data and 26.3 jar; without them, test skipped, failure under `RUSTCRAFT_OPT_REQUIRE=1`).

use super::objects::{BlockTint, frame_visual_yaw, minecart_offset, ominous_spawner_pose};
use super::tests_g2f03::{Kit, kit};
use super::*;
use rc_proto_v777::entity::Meta;

/// Minecart offset: values of the official computation (wrapped 64-bit integers, 3 bits per
/// axis), recomputed outside Rust.
#[test]
fn minecart_offset_matches_the_game() {
    let want: [(i32, [f32; 3]); 4] = [
        (1, [0.00175, 0.00075, 0.00175]),
        (7, [-0.00025, -0.00025, 0.00075]),
        (8, [0.00125, -0.00075, -0.00025]),
        (-3, [0.00025, -0.00075, -0.00075]),
    ];
    for (id, w) in want {
        let got = minecart_offset(id);
        assert!(got.abs_diff_eq(Vec3::from(w), 1e-9), "{id}: {got:?}");
    }
}

/// Visual rotation of a frame: half-turn + quarter turn per horizontal direction + 45° per
/// notch, ±90° on the ceiling and the floor, brought into [−180, 180[.
#[test]
fn frame_visual_yaw_matches_the_game() {
    assert_eq!(frame_visual_yaw(3, 0), -180.0);
    assert_eq!(frame_visual_yaw(2, 0), 0.0);
    assert_eq!(frame_visual_yaw(4, 1), -45.0);
    assert_eq!(frame_visual_yaw(5, 7), 45.0);
    assert_eq!(frame_visual_yaw(0, 2), 90.0);
    assert_eq!(frame_visual_yaw(1, 0), -180.0);
}

/// Ominous spawner: scale age / 50 up to 50 ticks, then yaw of 40° per tick brought
/// into [−180, 180[.
#[test]
fn ominous_spawner_grows_then_spins() {
    let close = |a: Mat4, b: Mat4| assert!(a.abs_diff_eq(b, 1e-5), "{a:?}\n≠\n{b:?}");
    close(ominous_spawner_pose(25.0), Mat4::from_scale(Vec3::splat(0.5)) * Mat4::from_rotation_y(deg(-80.0)));
    close(ominous_spawner_pose(50.0), Mat4::from_rotation_y(deg(-160.0)));
    close(ominous_spawner_pose(60.5), Mat4::from_rotation_y(deg(-100.0)));
}

// ---------------------------------------------------------------------------------------------
// Wiring through the GPU kit.

/// An entity from the real registry (`kind` without namespace), with id `id`.
fn spawn(k: &Kit, kind: &str, id: i32, edit: impl FnOnce(&mut Entity)) -> Entity {
    let names = k.r.source.registry("minecraft:entity_type").map(|r| r.entries.clone()).unwrap_or_default();
    let idx = names.iter().position(|n| *n == format!("minecraft:{kind}")).unwrap_or_else(|| panic!("{kind}"));
    let mut all = crate::entities::Entities { type_names: names, ..Default::default() };
    let add = rc_proto_v777::entity::AddEntity {
        id,
        uuid: 1000 + id as u128,
        kind: idx as i32,
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

/// What a frame placed: instances (source, first vertex, count, instance) and matrices.
struct Frame {
    inst: Vec<(u8, u32, u32, Instance)>,
    mats: Vec<[u8; std::mem::size_of::<PartMat>()]>,
}

/// Prepares the single entity `e` seen from 5 blocks, biome with colours `biome`.
fn draw(k: &mut Kit, e: &Entity, biome: [u32; 4]) -> Frame {
    let state = |_: i32, _: i32, _: i32| 0u32;
    let bright = |_: i32, _: i32, _: i32| 15i32;
    let tints = move |_: i32, _: i32, _: i32| biome;
    let world = ShadowWorld { state: &state, brightness: &bright, ambient: 0.0, biome_tints: &tints };
    k.r.prepare(
        std::iter::once(e),
        0.0,
        [0, 0],
        [0.5, 65.0, 5.5],
        &|_, _, _| (15, 15),
        &|_, _| true,
        Some(&k.items),
        &world,
    );
    Frame {
        inst: k.r.instances.iter().flatten().copied().collect(),
        mats: k.r.mats.iter().map(|m| bytemuck::bytes_of(m).try_into().expect("size")).collect(),
    }
}

/// RGBA instance bytes of an ARGB colour.
fn rgba(c: u32) -> u32 {
    (c >> 16 & 0xff) | (c & 0xff00) | (c & 0xff) << 16 | (c & 0xff00_0000)
}

fn state_of(k: &Kit, block: &str) -> u32 {
    k.r.source.blocks.iter().find(|b| b.name == format!("minecraft:{block}")).map(|b| b.default_state).expect(block)
}

/// State of `block` with the given properties (the others free).
fn state_with(k: &Kit, block: &str, props: &[(&str, &str)]) -> u32 {
    let b = k.r.source.blocks.iter().find(|b| b.name == format!("minecraft:{block}")).expect(block);
    (b.first_state..b.first_state + b.state_count)
        .find(|&s| props.iter().all(|(n, v)| k.r.source.state_properties(s).any(|(a, b)| a == *n && b == *v)))
        .expect("state")
}

fn colors(f: &Frame) -> Vec<u32> {
    f.inst.iter().map(|i| i.3.color).collect()
}

/// TNT carrying a state: position-less colours of the tint sources (grass: default colour of
/// the palette; oak leaves 0xFF48B518; lily pad "in hand" 0xFF71C35C), instead of
/// white; falling block: colours of the block's biome (lily pad: its world constant).
#[test]
fn carried_and_falling_blocks_take_their_tints() {
    let Some(mut k) = kit() else { return };
    let cs = |k: &Kit, b: &str| k.r.source.client_states[state_of(k, b) as usize];
    assert_eq!(cs(&k, "grass_block").entity_tint[0], rc_data::tint::GRASS_DEFAULT);
    assert_eq!(cs(&k, "oak_leaves").entity_tint[0], 0xff48_b518);
    assert_eq!(cs(&k, "lily_pad").entity_tint[0], 0xff71_c35c);
    assert_eq!(cs(&k, "stone").entity_tint[0], 0xffff_ffff);
    let grass = k.r.objects.grass_default;
    assert_ne!(grass & 0xff_ffff, 0xff_ffff, "grass palette read");
    let biome = [0x11_2233, 0x44_5566, 0x77_8899, 0xaa_bbcc];
    let carried = |k: &mut Kit, b: &str| {
        let s = state_of(k, b) as i32;
        let e = spawn(k, "tnt", 7, |e| {
            e.meta.insert(9, Meta::BlockState(Some(s)));
        });
        colors(&draw(k, &e, biome))
    };
    let falling = |k: &mut Kit, b: &str| {
        let s = state_of(k, b) as i32;
        let e = spawn(k, "falling_block", 7, |e| e.data = s);
        colors(&draw(k, &e, biome))
    };
    assert!(carried(&mut k, "grass_block").contains(&rgba(grass)));
    assert!(carried(&mut k, "oak_leaves").contains(&rgba(0xff48_b518)));
    assert!(carried(&mut k, "lily_pad").contains(&rgba(0xff71_c35c)));
    assert!(falling(&mut k, "grass_block").contains(&rgba(0xff11_2233)));
    assert!(falling(&mut k, "oak_leaves").contains(&rgba(0xff44_5566)));
    assert!(falling(&mut k, "lily_pad").contains(&rgba(0xff20_8030)));
    assert!(falling(&mut k, "stone").iter().all(|&c| c == 0xffff_ffff));
    // Transient block: colours of the biome at its position.
    k.r.instances.iter_mut().for_each(Vec::clear);
    k.r.push_transient(
        std::iter::once(([0, 64, 0], state_of(&k, "grass_block"))),
        [0, 0],
        &|_, _, _| (15, 15),
        &|_, _, _| biome,
    );
    assert!(k.r.instances.iter().flatten().any(|i| i.3.color == rgba(0xff11_2233)));
    // Colour of a layer: world CONSTANT, biome kind, no tint.
    let lily = state_of(&k, "lily_pad");
    assert_eq!(k.r.block_tint(lily, 0, BlockTint::World(biome)), 0xff20_8030);
    assert_eq!(k.r.block_tint(lily, -1, BlockTint::Entity), 0xffff_ffff);
}

/// Blocks with a special model carried or displayed: the block's entity layer is placed (heads,
/// Shulker, bell, banner, enchanting table book, pot, statue, conduit, double chest), the portal
/// cube added to the portals; the chest follows the state's type and orientation.
#[test]
fn special_block_models_drawn_as_entities() {
    let Some(mut k) = kit() else { return };
    let layer_first = |k: &Kit, l: &str| k.r.layers[k.r.layer_index[l]].first;
    let cases = [
        ("player_head", "minecraft:player_head#main"),
        ("skeleton_skull", "minecraft:skeleton_skull#main"),
        ("shulker_box", "minecraft:shulker_box#main"),
        ("red_shulker_box", "minecraft:shulker_box#main"),
        ("bell", "minecraft:bell#main"),
        ("white_banner", "minecraft:standing_banner#flag"),
        ("blue_wall_banner", "minecraft:wall_banner#flag"),
        ("enchanting_table", "minecraft:book#main"),
        ("decorated_pot", "minecraft:decorated_pot_base#main"),
        ("copper_golem_statue", "minecraft:copper_golem#main"),
        ("conduit", "minecraft:conduit#shell"),
        ("ender_chest", "minecraft:chest#main"),
    ];
    for (block, layer) in cases {
        let s = state_of(&k, block) as i32;
        let e = spawn(&k, "tnt", 7, |e| {
            e.meta.insert(9, Meta::BlockState(Some(s)));
        });
        let f = draw(&mut k, &e, [0xff_ffff; 4]);
        let first = layer_first(&k, layer);
        assert!(f.inst.iter().any(|i| i.0 == 0 && i.1 == first), "{block}: layer {layer}");
    }
    // Left chest facing east: left half, matrix different from the north chest.
    let left = state_with(&k, "chest", &[("type", "left"), ("facing", "east")]);
    let north = state_with(&k, "chest", &[("type", "left"), ("facing", "north")]);
    let draw_chest = |k: &mut Kit, s: u32| {
        let e = spawn(k, "tnt", 7, |e| {
            e.meta.insert(9, Meta::BlockState(Some(s as i32)));
        });
        draw(k, &e, [0xff_ffff; 4])
    };
    let a = draw_chest(&mut k, left);
    let first = layer_first(&k, "minecraft:double_chest_left#main");
    assert!(a.inst.iter().any(|i| i.0 == 0 && i.1 == first), "left half");
    let b = draw_chest(&mut k, north);
    assert_ne!(a.mats, b.mats, "orientation of the state");
    // End gateway: 6-face cube in the frame's portals.
    let s = state_of(&k, "end_gateway") as i32;
    let e = spawn(&k, "tnt", 7, |e| {
        e.meta.insert(9, Meta::BlockState(Some(s)));
    });
    draw(&mut k, &e, [0xff_ffff; 4]);
    assert_eq!(k.r.objects.portals.len(), 36);
    // Stone: no special model.
    let s = state_of(&k, "stone") as i32;
    let e = spawn(&k, "tnt", 7, |e| {
        e.meta.insert(9, Meta::BlockState(Some(s)));
    });
    let f = draw(&mut k, &e, [0xff_ffff; 4]);
    assert!(f.inst.iter().all(|i| i.0 == 1), "mesh only");
}

/// Moving blocks (falling, transient): state model only, like the game; neither entity layer
/// (chest, head, banner, Shulker box), nor portal cube (portal, gateway).
#[test]
fn moving_blocks_draw_no_special_model() {
    let Some(mut k) = kit() else { return };
    let blocks = ["chest", "player_head", "white_banner", "shulker_box", "end_portal", "end_gateway"];
    for block in blocks {
        let s = state_of(&k, block);
        let e = spawn(&k, "falling_block", 7, |e| e.data = s as i32);
        let f = draw(&mut k, &e, [0xff_ffff; 4]);
        assert!(f.inst.iter().all(|i| i.0 != 0), "{block} falling: no entity layer");
        assert!(k.r.objects.portals.is_empty(), "{block} falling: no portal");
        k.r.objects.portals.clear();
        k.r.instances.iter_mut().for_each(Vec::clear);
        k.r.push_transient(std::iter::once(([0, 64, 0], s)), [0, 0], &|_, _, _| (15, 15), &|_, _, _| [0xff_ffff; 4]);
        assert!(k.r.instances.iter().flatten().all(|i| i.0 != 0), "{block} transient: no entity layer");
        assert!(k.r.objects.portals.is_empty(), "{block} transient: no portal");
    }
}

/// Projectiles and objects: Shulker bullet oriented by its yaw, scrolling wind charge,
/// minecarts offset by their id, painting oriented by value 8, ominous spawner
/// drawn (copies depending on the quantity), compass of a frame specific to the frame.
#[test]
fn objects_wiring_through_prepare() {
    let Some(mut k) = kit() else { return };
    let white = [0xff_ffff; 4];
    // Shulker bullet.
    let a = spawn(&k, "shulker_bullet", 7, |_| {});
    let b = spawn(&k, "shulker_bullet", 7, |e| {
        e.yaw = 90.0;
        e.yaw_o = 90.0;
    });
    let (fa, fb) = (draw(&mut k, &a, white), draw(&mut k, &b, white));
    assert!(!fa.inst.is_empty());
    assert_ne!(fa.mats, fb.mats, "bullet oriented");
    // Wind charge: flat and scrolling flags, u = age × 0.03.
    let w = spawn(&k, "wind_charge", 7, |e| e.age = 10);
    let f = draw(&mut k, &w, white);
    assert!(!f.inst.is_empty());
    for i in &f.inst {
        assert_eq!(i.3.light & (LIGHT_FLAT | LIGHT_SCROLL), LIGHT_FLAT | LIGHT_SCROLL);
        assert_eq!(i.3.overlay, ((10.0f32 * 0.03) % 1.0 * 65535.0) as u32);
    }
    // Minecarts: two ids, two offsets.
    let (c7, c8) = (spawn(&k, "minecart", 7, |_| {}), spawn(&k, "minecart", 8, |_| {}));
    let (f7, f8) = (draw(&mut k, &c7, white), draw(&mut k, &c8, white));
    assert!(!f7.inst.is_empty());
    assert_ne!(f7.mats, f8.mats, "offset by id");
    // Painting: direction from value 8 takes priority over the spawn value.
    k.r.set_variants("minecraft:painting_variant", &[("minecraft:kebab".into(), None)]);
    let painting = |k: &Kit, data: i32, dir: Option<i32>| {
        spawn(k, "painting", 7, |e| {
            e.data = data;
            e.meta.insert(9, Meta::Int(0));
            if let Some(d) = dir {
                e.meta.insert(8, Meta::Direction(d));
            }
        })
    };
    let (pn, pm, ps) = (painting(&k, 2, None), painting(&k, 3, Some(2)), painting(&k, 3, None));
    let north = draw(&mut k, &pn, white);
    let moved = draw(&mut k, &pm, white);
    let south = draw(&mut k, &ps, white);
    assert!(!north.inst.is_empty());
    assert_eq!(north.mats, moved.mats, "value 8");
    assert_ne!(north.mats, south.mats);
    // Ominous spawner: object drawn, two copies for 10 objects.
    let stone =
        rc_proto_v777::item::ItemStack { item: k.item("minecraft:stone"), count: 1, added: vec![], removed: vec![] };
    let spawner = |k: &Kit, count: i32| {
        let s = rc_proto_v777::item::ItemStack { count, ..stone.clone() };
        spawn(k, "ominous_item_spawner", 7, |e| {
            e.age = 60;
            e.meta.insert(8, Meta::Item(Some(s)));
        })
    };
    let (s1, s10) = (spawner(&k, 1), spawner(&k, 10));
    let one = draw(&mut k, &s1, white);
    let many = draw(&mut k, &s10, white);
    assert!(!one.inst.is_empty(), "spawner drawn");
    assert_eq!(many.inst.len(), 2 * one.inst.len());
    assert!(one.inst.iter().all(|i| i.3.light & 0xffff == 0xf0f0), "full light");
    // Frames holding a compass in the Nether (no target): seed = frame.
    k.r.world = crate::stack::HudEnv {
        dimension: "minecraft:the_nether".into(),
        game_time: 500,
        ..crate::stack::HudEnv::empty()
    };
    let compass =
        rc_proto_v777::item::ItemStack { item: k.item("minecraft:compass"), count: 1, added: vec![], removed: vec![] };
    let frame = |k: &Kit, id: i32| {
        spawn(k, "item_frame", id, |e| {
            e.meta.insert(8, Meta::Direction(3));
            e.meta.insert(9, Meta::Item(Some(compass.clone())));
        })
    };
    let (e7, e8) = (frame(&k, 7), frame(&k, 8));
    let (g7, g8) = (draw(&mut k, &e7, white), draw(&mut k, &e8, white));
    let firsts = |f: &Frame| f.inst.iter().map(|i| (i.1, i.2)).collect::<Vec<_>>();
    assert!(g7.inst.len() > 1, "frame and compass");
    assert_ne!(firsts(&g7), firsts(&g8), "two frames, two needles");
}

/// Map decoration names: placed for the visible decorations (in a frame: only
/// those shown there), with the received text, marked "hand" for the held map.
#[test]
fn map_decoration_names_become_labels() {
    let Some(mut k) = kit() else { return };
    use rc_proto_v777::map::{MapDecoration, MapItemData, MapPatch};
    let id = 987_654;
    let name = |s: &str| Some(rc_proto::nbt::Nbt::String(s.into()));
    crate::maps::store().apply(MapItemData {
        id,
        scale: 0,
        locked: false,
        decorations: Some(vec![
            // Frame (visible in a frame), player (in hand only), frame without a name.
            MapDecoration { kind: 1, x: 0, y: 0, rot: 0, name: name("Base") },
            MapDecoration { kind: 0, x: 10, y: 10, rot: 4, name: name("Me") },
            MapDecoration { kind: 1, x: -20, y: 5, rot: 0, name: None },
        ]),
        patch: Some(MapPatch { x0: 0, y0: 0, width: 128, height: 128, colors: vec![0; 128 * 128] }),
    });
    let labels = |k: &mut Kit, hand: bool| {
        k.r.objects.texts.clear();
        k.r.objects.labels.clear();
        assert!(k.r.push_map_contents(Mat4::IDENTITY, id, 0xf0f0, hand, true));
        let texts: Vec<(u32, bool)> = k.r.objects.texts.iter().map(|t| (t.label, t.hand)).collect();
        (texts, k.r.objects.labels.clone())
    };
    let (framed, l) = labels(&mut k, false);
    assert_eq!(framed, vec![(0, false)]);
    assert_eq!(l, vec![rc_proto::nbt::Nbt::String("Base".into())]);
    let (held, l) = labels(&mut k, true);
    assert_eq!(held, vec![(0, true), (1, true)]);
    assert_eq!(l[1], rc_proto::nbt::Nbt::String("Me".into()));
}
