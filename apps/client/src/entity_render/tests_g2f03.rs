//! Humanoid models, held items and item carriers outside the HUD (G2-F03): expected values
//! written from the 26.3 game (formulas copied in our own words), no extracted data.

use super::keyframes::testkit::{self, state, test_layer};
use super::*;
use rc_proto_v777::entity::Meta;

fn close(a: Mat4, b: Mat4) {
    assert!(a.abs_diff_eq(b, 1e-5), "{a:?}\n≠\n{b:?}");
}

fn hand() -> HandRef {
    HandRef {
        parent: Mat4::from_translation(Vec3::new(0.1, 0.2, 0.3)) * Mat4::from_rotation_y(0.4),
        arm: [-5.0, 2.0, 0.0, -0.7, 0.2, 0.1, 1.0, 1.0, 1.0],
        body: Some(Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0)) * Mat4::from_rotation_x(0.3)),
        right_arm_x: -0.6,
    }
}

/// Hand frame depending on the model: skeletons offset one pixel outward before the
/// arm rotation, slim avatar by half a pixel, vex and copper golem scaled down, allay below the
/// body; the others at the end of the arm.
#[test]
fn hand_frames_follow_each_model() {
    let h = hand();
    let shifted = |dx: f32| {
        let mut a = h.arm;
        a[0] += dx;
        h.parent * part_matrix(&a)
    };
    close(hand_frame("skeleton", true, &h, false, false), shifted(1.0));
    close(hand_frame("wither_skeleton", false, &h, false, false), shifted(-1.0));
    close(hand_frame("player", true, &h, true, false), shifted(0.5));
    close(hand_frame("player", false, &h, true, false), shifted(-0.5));
    close(hand_frame("player", true, &h, false, false), h.tip());
    close(hand_frame("zombie", true, &h, false, false), h.tip());
    close(
        hand_frame("vex", false, &h, false, false),
        h.tip()
            * Mat4::from_scale(Vec3::splat(0.55))
            * Mat4::from_translation(Vec3::new(-0.046875, -0.15625, 0.078125)),
    );
    close(
        hand_frame("copper_golem", true, &h, false, true),
        h.tip() * Mat4::from_rotation_y(deg(-90.0)) * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.125)),
    );
    close(
        hand_frame("copper_golem", false, &h, false, false),
        h.tip() * Mat4::from_scale(Vec3::splat(0.55)) * Mat4::from_translation(Vec3::new(-0.125, 0.3125, -0.1875)),
    );
    // Allay: same frame for both hands, right arm rotation.
    let allay = h.body.unwrap()
        * Mat4::from_translation(Vec3::new(0.0, 0.0625, 0.1875))
        * Mat4::from_rotation_x(-0.6)
        * Mat4::from_scale(Vec3::splat(0.7))
        * Mat4::from_translation(Vec3::new(0.0625, 0.0, 0.0));
    close(hand_frame("allay", true, &h, false, false), allay);
    close(hand_frame("allay", false, &h, false, false), allay);
}

/// Frame of a hidden part: chain of parents ignoring visibility (hand of the
/// armor stand without arms).
#[test]
fn raw_world_ignores_visibility() {
    let l = test_layer(&[("body", None), ("right_arm", Some(0))]);
    let parts = [[0.0, 2.0, 0.0, 0.3, 0.0, 0.0, 1.0, 1.0, 1.0], [-5.0, 2.0, 0.0, -0.4, 0.1, 0.0, 1.0, 1.0, 1.0]];
    let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
    close(raw_world(&l, &parts, m, 1), m * part_matrix(&parts[0]) * part_matrix(&parts[1]));
}

/// Copper golem head raised by 2 pixels and enlarged by 1/16; flower beds on the
/// antenna offset by a quarter block.
#[test]
fn copper_golem_head_and_antenna_offsets() {
    close(
        head_item_frame("copper_golem"),
        Mat4::from_translation(Vec3::new(0.0, 0.125, 0.0)) * Mat4::from_scale(Vec3::splat(1.0625)),
    );
    close(head_item_frame("zombie"), Mat4::IDENTITY);
    close(antenna_block_offset(true), Mat4::from_translation(Vec3::new(0.25, 0.0, 0.25)));
    close(antenna_block_offset(false), Mat4::IDENTITY);
}

/// Translucent allay; wolf collar and animal equipment without damage overlay; wool and
/// slime shell as outline only when invisible and glowing.
#[test]
fn layer_rules() {
    assert!(default_body_pass("allay") == Pass::Translucent);
    assert!(default_body_pass("zombie") == Pass::Cutout);
    assert!(no_damage_overlay("wolf", "minecraft:wolf#main", Wool::Fixed(0xff00_00ff)));
    assert!(no_damage_overlay("horse", "minecraft:horse#saddle", Wool::None));
    assert!(no_damage_overlay("wolf", "minecraft:wolf_armor#main", Wool::None));
    assert!(!no_damage_overlay("wolf", "minecraft:wolf#main", Wool::None));
    assert!(!no_damage_overlay("cat", "minecraft:cat#main", Wool::Fixed(0xff00_00ff)));
    for l in ["minecraft:sheep#wool", "minecraft:sheep_baby#wool", "minecraft:slime#outer"] {
        assert!(outlined_when_invisible(l), "{l}");
    }
    assert!(!outlined_when_invisible("minecraft:sheep#wool_undercoat"));
}

fn piglin_posed(pose: u8, right: bool, pitch: f32) -> Vec<[f32; 9]> {
    let parts = [
        ("head", None),
        ("body", None),
        ("right_arm", None),
        ("left_arm", None),
        ("right_ear", None),
        ("left_ear", None),
    ];
    let l = test_layer(&parts);
    let mut all = testkit::spawn("piglin");
    let e = all.map.get_mut(&7).unwrap();
    e.meta.insert(15, Meta::Byte(if right { 0 } else { 2 }));
    let mut s = state(e, 0.0);
    s.piglin = pose;
    s.pitch = pitch;
    s.yaw = 0.2;
    s.use_ticks = 10.0;
    s.crossbow_charge = 25.0;
    let mut p = Pose::reset(&l, Vec::new(), Vec::new());
    animate(&mut p, e, &s, 0.0, true);
    p.parts.clone()
}

/// Piglin: admiring (head lowered by 0.5, arm opposite the main hand extended), crossbow
/// held and charging depending on the main hand; pose priority as in the game.
#[test]
fn piglin_arm_poses() {
    let p = piglin_posed(piglin_pose::ADMIRING, true, 0.3);
    assert_eq!((p[0][3], p[0][4]), (0.5, 0.0));
    assert_eq!((p[3][3], p[3][4]), (-0.9, 0.5));
    let p = piglin_posed(piglin_pose::ADMIRING, false, 0.3);
    assert_eq!((p[2][3], p[2][4]), (-0.9, -0.5));
    // Crossbow held on the right: right arm aiming (−π/2 + pitch + 0.1, −0.3 + yaw).
    let p = piglin_posed(piglin_pose::HOLD, true, 0.3);
    assert!((p[2][3] - (-PI / 2.0 + 0.3 + 0.1)).abs() < 1e-6 && (p[2][4] - (-0.3 + 0.2)).abs() < 1e-6);
    assert!((p[3][3] - (-1.5 + 0.3)).abs() < 1e-6 && (p[3][4] - (0.6 + 0.2)).abs() < 1e-6);
    // Held on the left: roles reversed.
    let p = piglin_posed(piglin_pose::HOLD, false, 0.3);
    assert!((p[3][4] - (0.3 + 0.2)).abs() < 1e-6 && (p[2][4] - (-0.6 + 0.2)).abs() < 1e-6);
    // Charge: 10 ticks out of 25.
    let p = piglin_posed(piglin_pose::CHARGE, true, 0.3);
    let k = 10.0 / 25.0;
    assert_eq!((p[2][3], p[2][4]), (-0.970_796_35, -0.8));
    assert!((p[3][4] - (0.4 + k * 0.45)).abs() < 1e-6);
    assert!((p[3][3] - (-0.970_796_35 + k * (-PI / 2.0 + 0.970_796_35))).abs() < 1e-6);
    assert_eq!(piglin_pose_of(true, true, true, true), piglin_pose::ADMIRING);
    assert_eq!(piglin_pose_of(false, true, true, true), piglin_pose::MELEE);
    assert_eq!(piglin_pose_of(false, false, true, true), piglin_pose::CHARGE);
    assert_eq!(piglin_pose_of(false, false, false, true), piglin_pose::HOLD);
    assert_eq!(piglin_pose_of(false, false, false, false), piglin_pose::DEFAULT);
}

/// Mounted illager: arms at −π/5, legs folded and spread.
#[test]
fn mounted_illager_sits() {
    let parts = [("head", None), ("right_arm", None), ("left_arm", None), ("right_leg", None), ("left_leg", None)];
    let l = test_layer(&parts);
    let mut all = testkit::spawn("pillager");
    let e = all.map.get_mut(&7).unwrap();
    e.meta.insert(15, Meta::Byte(4));
    e.vehicle = Some(3);
    let mut s = state(e, 0.0);
    s.riding = true;
    let mut p = Pose::reset(&l, Vec::new(), Vec::new());
    animate(&mut p, e, &s, 0.0, true);
    assert_eq!(&p.parts[3][3..6], &[-1.413_716_7, PI / 10.0, 0.078_539_82]);
    assert_eq!(&p.parts[4][3..6], &[-1.413_716_7, -PI / 10.0, -0.078_539_82]);
}

/// Gliding: the arm poses read the raised head (−π/4), not the raw pitch.
#[test]
fn gliding_arm_pose_uses_forced_head_pitch() {
    let parts = [("head", None), ("body", None), ("right_arm", None), ("left_arm", None)];
    let l = test_layer(&parts);
    let all = testkit::spawn("player");
    let mut s = state(&all.map[&7], 0.0);
    s.fall_flying = true;
    s.pitch = 0.4;
    s.arm_pose = [arm::BOW, arm::EMPTY];
    let mut p = Pose::reset(&l, Vec::new(), Vec::new());
    humanoid(&mut p, &s);
    assert!((p.parts[2][3] - (-PI / 2.0 - PI / 4.0)).abs() < 1e-6, "{:?}", p.parts[2]);
}

/// Creeper: each leg takes the phase of the opposite leg of the quadruped (quirk).
#[test]
fn creeper_legs_are_crossed() {
    let parts = [
        ("head", None),
        ("right_hind_leg", None),
        ("left_hind_leg", None),
        ("right_front_leg", None),
        ("left_front_leg", None),
    ];
    let l = test_layer(&parts);
    let all = testkit::spawn("creeper");
    let mut s = state(&all.map[&7], 0.0);
    s.walk_pos = 1.3;
    s.walk_speed = 0.8;
    let q = 1.3f32 * 0.6662;
    let (a, b) = (q.cos() * 1.4 * 0.8, (q + PI).cos() * 1.4 * 0.8);
    let mut p = Pose::reset(&l, Vec::new(), Vec::new());
    animate(&mut p, &all.map[&7], &s, 0.0, true);
    assert_eq!([p.parts[1][3], p.parts[2][3], p.parts[3][3], p.parts[4][3]], [b, a, a, b]);
    let mut p = Pose::reset(&l, Vec::new(), Vec::new());
    quadruped(&mut p, &s);
    assert_eq!([p.parts[1][3], p.parts[2][3], p.parts[3][3], p.parts[4][3]], [a, b, b, a]);
}

/// Mannequin: skin layers hidden like a player's (index 16).
#[test]
fn mannequin_hides_skin_layers() {
    let parts = [("head", None), ("hat", Some(0)), ("body", None), ("jacket", Some(2))];
    let l = test_layer(&parts);
    let mut all = testkit::spawn("mannequin");
    let e = all.map.get_mut(&7).unwrap();
    e.meta.insert(16, Meta::Byte(0x40));
    let p = testkit::posed(&l, e, 0.0);
    assert!(p.visible[1] && !p.visible[3], "{:?}", p.visible);
}

/// Spectator seen by a spectator: head and hat only.
#[test]
fn spectator_avatar_keeps_head_only() {
    let parts = [
        ("head", None),
        ("hat", Some(0)),
        ("body", None),
        ("jacket", None),
        ("right_arm", None),
        ("right_sleeve", None),
        ("left_leg", None),
        ("left_pants", None),
    ];
    let l = test_layer(&parts);
    let mut p = Pose::reset(&l, Vec::new(), Vec::new());
    spectator_head_only(&mut p);
    assert_eq!(p.visible, [true, true, false, false, false, false, false, false]);
}

/// Illusioner: spread at the last tick of the damage overlay (±3 per half block, 0 or 1 in y),
/// return to the centre at the first, 3-tick transition at weight ((t − pt) / 3)^¼.
#[test]
fn illusioner_copies() {
    let mut all = testkit::spawn("illusioner");
    let e = all.map.get_mut(&7).unwrap();
    e.age = 5;
    e.hurt_time = 1;
    // Invisible: nothing.
    assert!(!illusion_tick(e, |_| 0));
    e.meta.insert(0, Meta::Byte(0x20));
    let mut draws = [0, 5, 11, 7, 3, 0, 1, 2, 4, 9, 6, 1].into_iter();
    assert!(illusion_tick(e, |n| draws.next().unwrap().min(n - 1)));
    assert_eq!(e.extra[0], 3.0);
    let cur = illusion_offsets(e, 0.0);
    // Draws (0, 5, 11) → (−3, 1, 2.5); (7, 3, 0) → (0.5, 0, −3).
    assert_eq!(e.history[4], [-3.0, 1.0, 2.5]);
    assert_eq!(e.history[5], [0.5, 0.0, -3.0]);
    let s = (f64::from(3.0f32 - 0.0) / 3.0).powf(0.25);
    assert!((cur[0][0] - (-3.0 * (1.0 - s))).abs() < 1e-9);
    let half = illusion_offsets(e, 0.5);
    let s = (2.5f64 / 3.0).powf(0.25);
    assert!((half[0][1] - (1.0 - s)).abs() < 1e-6, "{half:?}");
    // Three ticks later: current offsets.
    e.hurt_time = 0;
    for _ in 0..3 {
        e.age += 1;
        assert!(!illusion_tick(e, |_| 0));
    }
    assert_eq!(illusion_offsets(e, 0.7)[0], [-3.0, 1.0, 2.5]);
    // Start of the overlay: copies brought back to the centre, without clouds.
    e.hurt_time = 9;
    assert!(!illusion_tick(e, |_| 0));
    assert_eq!(e.history[4], [0.0; 3]);
    assert_eq!(e.history[0], [-3.0, 1.0, 2.5]);
    let w = illusion_wobble(2, 10.0);
    let c = |k: f32| f64::from((2.0f32 + 10.0 * k).cos());
    assert_eq!(w, [c(0.5) * 0.025, c(0.75) * 0.0125, c(0.7) * 0.025]);
}

/// Ground item seen by the needles: half-turn minus its own rotation at the middle of the tick.
#[test]
fn ground_item_owner_rotation() {
    let y = ground_item_yaw(10, 1.0);
    let expect = 180.0 - ((10.5 / 20.0) + 1.0) / (PI * 2.0) * 360.0;
    assert!((y - expect).abs() < 1e-4, "{y} {expect}");
}

// ---------------------------------------------------------------------------------------------
// End-to-end wiring: real entity render engine (windowless GPU, game data and 26.3
// jar), instances placed by `prepare`. Without GPU, data or game: test skipped (failure under
// `RUSTCRAFT_OPT_REQUIRE=1`).

/// Placed instance: first vertex, vertex count, texture rectangle (bits).
pub(super) type Drawn = (u32, u32, [u32; 4]);

pub(super) struct Kit {
    pub(super) r: EntityRenderer,
    pub(super) items: rc_assets::item::ItemModels,
    pub(super) item_names: Vec<String>,
}

pub(super) fn kit() -> Option<Kit> {
    let data = rc_testdata::version_data()?;
    let install = rc_testdata::need(rc_data::locate("26.3").ok(), "game 26.3 installed")?;
    let assets = rc_testdata::need(
        rc_assets::source::Assets::for_install(&install.client_jar, &[], None).ok(),
        "resources of the 26.3 jar",
    )?;
    let assets = Arc::new(assets);
    crate::render::upload::tests::gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok();
    // A software adapter (WARP, llvmpipe) is not the real GPU these proofs are about.
    let hardware = adapter.filter(|a| a.get_info().device_type != wgpu::DeviceType::Cpu);
    let adapter = rc_testdata::need(hardware, "hardware GPU adapter")?;
    let (device, _queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
    let items = rc_assets::item::ItemModels::load(&assets, data.items.iter().map(|i| i.model.as_str()));
    let mut blocks = rc_assets::build(&assets, &data);
    let textures = std::mem::take(&mut blocks.textures);
    let tex = EntityTextures::load(&assets, &items.sprites).with_blocks(Arc::new(blocks), &textures);
    let item_names = data.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default();
    let layout = crate::render::camera_layout(&device);
    let r = EntityRenderer::new(&device, &layout, wgpu::TextureFormat::Bgra8UnormSrgb, Arc::new(data), tex);
    Some(Kit { r, items, item_names })
}

impl Kit {
    pub(super) fn item(&self, name: &str) -> i32 {
        self.item_names.iter().position(|n| n == name).unwrap_or_else(|| panic!("item {name}")) as i32
    }

    /// An entity `kind` with id `id`, prepared by `edit`.
    pub(super) fn entity(kind: &str, id: i32, edit: impl FnOnce(&mut Entity)) -> Entity {
        let mut all = crate::entities::Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
        let add = rc_proto_v777::entity::AddEntity {
            id,
            uuid: 1000 + id as u128,
            kind: 0,
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

    /// Prepares the single entity `e` seen from 5 blocks: placed instances (first vertex, count,
    /// rectangle), their count and that of the silhouette-only ones.
    pub(super) fn draw(&mut self, e: &Entity) -> (Vec<Drawn>, usize, usize) {
        let state = |_: i32, _: i32, _: i32| 0u32;
        let bright = |_: i32, _: i32, _: i32| 15i32;
        let tints = |_: i32, _: i32, _: i32| [0xff_ffff; 4];
        let world = ShadowWorld { state: &state, brightness: &bright, ambient: 0.0, biome_tints: &tints };
        self.r.prepare(
            std::iter::once(e),
            0.0,
            [0, 0],
            [0.5, 65.0, 5.5],
            &|_, _, _| (15, 15),
            &|_, _| true,
            Some(&self.items),
            &world,
        );
        let list: Vec<Drawn> = self
            .r
            .instances
            .iter()
            .flatten()
            .map(|(_, first, count, i)| (*first, *count, i.rect.map(f32::to_bits)))
            .collect();
        let n = list.len();
        (list, n, self.r.glow_only.len())
    }
}

/// Invisible illusioner: the body is kept and drawn four times (illusion copies);
/// invisible but glowing mooshroom and snow golem: all their instances, mushrooms
/// and pumpkin included, switch to silhouette only; two zombies with different ids
/// holding a compass in the Nether (no target) show two different needles, like
/// two compasses on the ground or carried by two item displays.
#[test]
fn wiring_through_prepare() {
    let Some(mut k) = kit() else { return };
    let invisible = |e: &mut Entity| e.meta.insert(0, Meta::Byte(0x20));

    // Illusioner.
    let (_, seen, _) = k.draw(&Kit::entity("illusioner", 7, |_| {}));
    let (_, ghost, _) = k.draw(&Kit::entity("illusioner", 7, invisible));
    assert!(seen > 0);
    assert_eq!(ghost, 4 * seen, "invisible illusioner: four copies of the body");

    // Invisible and glowing mooshroom and snow golem.
    // Snow golem with its pumpkin (bit 0x10 of meta 16).
    let pumpkin = |e: &mut Entity| {
        if e.kind == "snow_golem" {
            e.meta.insert(16, Meta::Byte(0x10));
        }
    };
    for kind in ["mooshroom", "snow_golem"] {
        let (_, visible, _) = k.draw(&Kit::entity(kind, 7, pumpkin));
        let (_, body, _) = k.draw(&Kit::entity(kind, 7, |_| {}));
        if kind == "snow_golem" {
            assert!(visible > body, "pumpkin drawn");
        }
        let glowing = Kit::entity(kind, 7, |e| {
            pumpkin(e);
            invisible(e);
        });
        k.r.glow.insert(glowing.uuid, 0xffffff);
        let (_, drawn, outline) = k.draw(&glowing);
        k.r.glow.clear();
        assert_eq!(drawn, 0, "{kind}: nothing outside the silhouettes");
        assert_eq!(outline, visible, "{kind}: body and carried block as silhouette");
        // Without glow: carried block hidden too.
        let (_, hidden, outline) = k.draw(&Kit::entity(kind, 7, |e| {
            pumpkin(e);
            invisible(e);
        }));
        assert_eq!((hidden, outline), (0, 0), "{kind} invisible without glow");
    }

    // Compasses without target: the carrier's seed.
    k.r.world = crate::stack::HudEnv {
        dimension: "minecraft:the_nether".into(),
        game_time: 500,
        ..crate::stack::HudEnv::empty()
    };
    let compass =
        rc_proto_v777::item::ItemStack { item: k.item("minecraft:compass"), count: 1, added: vec![], removed: vec![] };
    let mut needles = |kind: &str, put: &dyn Fn(&mut Entity, &rc_proto_v777::item::ItemStack)| {
        let a = k.draw(&Kit::entity(kind, 7, |e| put(e, &compass))).0;
        let b = k.draw(&Kit::entity(kind, 8, |e| put(e, &compass))).0;
        assert!(!a.is_empty(), "{kind}: compass drawn");
        assert_ne!(a, b, "{kind}: two carriers, two needles");
    };
    needles("zombie", &|e, s| e.equipment[0] = Some(s.clone()));
    needles("item", &|e, s| e.meta.insert(8, Meta::Item(Some(s.clone()))));
    needles("snowball", &|e, s| e.meta.insert(8, Meta::Item(Some(s.clone()))));
}
