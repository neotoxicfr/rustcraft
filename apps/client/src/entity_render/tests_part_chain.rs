//! Proofs of the part chain's switches (`part_chain.rs`): the closed-form rotation against a
//! double-precision reference, the affine chain against the full matrices (bit for bit up to the
//! sign of zeros), the baked rest matrices against the rebuilt ones (bit for bit), the layers that
//! share their matrices against the layers placed again (instance by instance), and the freed
//! baked faces. Benches at the end.

use super::part_chain::{PartFrames, is_affine};
use super::tests_c3m11::{canon, fnv, mat_bits, prep, scene};
use super::tests_g2f03::{Kit, kit};
use super::*;
use rc_proto_v777::entity::Meta;

/// Deterministic draws in [0, 1).
struct Draws(u64);

impl Draws {
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.unit() * n as f32) as usize).min(n - 1)
    }
}

/// Runs `f` with the frame's paths set to `paths` on this thread, then puts the new paths back.
fn with_paths<T>(paths: AnimationPaths, f: impl FnOnce() -> T) -> T {
    ANIMATION_PATHS.with(|p| p.set(paths));
    let out = f();
    ANIMATION_PATHS.with(|p| p.set(AnimationPaths::NEW));
    out
}

type Columns = [[f64; 4]; 4];

/// 3 × 3 product of column matrices.
fn product3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|j| std::array::from_fn(|i| (0..3).map(|k| a[k][i] * b[j][k]).sum()))
}

/// T(p/16) · Rz · Ry · Rx · S in double precision, with the library sine (columns).
fn reference_part_matrix(p: &[f32; 9]) -> Columns {
    let angle = |i: usize| f64::from(p[i]).sin_cos();
    let ((sx, cx), (sy, cy), (sz, cz)) = (angle(3), angle(4), angle(5));
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, sx], [0.0, -sx, cx]];
    let ry = [[cy, 0.0, -sy], [0.0, 1.0, 0.0], [sy, 0.0, cy]];
    let rz = [[cz, sz, 0.0], [-sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let r = product3(&rz, &product3(&ry, &rx));
    let mut m = [[0.0; 4]; 4];
    for j in 0..3 {
        for i in 0..3 {
            m[j][i] = r[j][i] * f64::from(p[6 + j]);
        }
    }
    m[3] = [f64::from(p[0]) / 16.0, f64::from(p[1]) / 16.0, f64::from(p[2]) / 16.0, 1.0];
    m
}

/// Normal matrix of a reference: cofactors of the 3 × 3 part, times the determinant's sign.
fn reference_normals(m: &Columns) -> [[f64; 3]; 3] {
    let v = |j: usize| [m[j][0], m[j][1], m[j][2]];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let (x, y, z) = (v(0), v(1), v(2));
    let c = [cross(y, z), cross(z, x), cross(x, y)];
    let det: f64 = (0..3).map(|i| x[i] * c[0][i]).sum();
    let sign = if det < 0.0 { -1.0 } else { 1.0 };
    c.map(|col| col.map(|v| v * sign))
}

/// Largest difference between a matrix and its reference, relative to each entry (at least 1).
fn worst_against(m: &Mat4, r: &Columns) -> f64 {
    let cols = m.to_cols_array_2d();
    (0..4)
        .flat_map(|j| (0..4).map(move |i| (j, i)))
        .map(|(j, i)| (f64::from(cols[j][i]) - r[j][i]).abs() / r[j][i].abs().max(1.0))
        .fold(0.0, f64::max)
}

/// Largest difference between the normal matrix of `m` and the reference's.
fn worst_normals_against(m: &PartMat, r: &Columns) -> f64 {
    let reference = reference_normals(r);
    (0..3)
        .flat_map(|j| (0..3).map(move |i| (j, i)))
        .map(|(j, i)| (f64::from(m.n[j][i]) - reference[j][i]).abs() / reference[j][i].abs().max(1.0))
        .fold(0.0, f64::max)
}

/// Poses of the closed-form test: every mask of turned axes, negative zeros, right angles, large
/// angles, mirrored and uneven scales, then random ones.
fn test_poses() -> Vec<[f32; 9]> {
    let angles = [0.0, -0.0, 1.0e-4, -0.3, 0.7, std::f32::consts::FRAC_PI_2, PI, -2.5, 6.2, 19.0];
    let scales = [[1.0, 1.0, 1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0], [0.5, 2.0, 1.25]];
    let mut poses = Vec::new();
    for mask in 0..8u32 {
        for (k, &a) in angles.iter().enumerate() {
            let b = angles[(k + 3) % angles.len()];
            let c = angles[(k + 7) % angles.len()];
            let s = scales[k % scales.len()];
            let pick = |bit: u32, v: f32| if mask & bit != 0 { v } else { 0.0 };
            poses.push([3.0, -7.5, 1.25, pick(1, a), pick(2, b), pick(4, c), s[0], s[1], s[2]]);
        }
    }
    let mut d = Draws(0x5eed_c323);
    for _ in 0..4000 {
        let turned = d.below(8) as u32;
        let angle = |d: &mut Draws, bit: u32| if turned & bit != 0 { d.range(-7.0, 7.0) } else { 0.0 };
        let s = scales[d.below(scales.len())];
        poses.push([
            d.range(-20.0, 20.0),
            d.range(-20.0, 20.0),
            d.range(-20.0, 20.0),
            angle(&mut d, 1),
            angle(&mut d, 2),
            angle(&mut d, 4),
            s[0],
            s[1],
            s[2],
        ]);
    }
    poses
}

/// The closed-form local matrix (and its normal matrix, mirrored parts included) stays within 1e-5
/// of a double-precision reference for every mask of turned axes; the quaternion path too. In-test
/// sabotages: the axes composed in the opposite order, the determinant's sign forgotten.
#[test]
fn fast_part_matrix_matches_reference() {
    let (mut closed, mut quaternion, mut normals) = (0.0f64, 0.0f64, 0.0f64);
    let (mut reversed_order, mut unsigned_normals) = (0.0f64, 0.0f64);
    for p in test_poses() {
        let reference = reference_part_matrix(&p);
        let fast = closed_form_part_matrix(&p);
        closed = closed.max(worst_against(&fast, &reference));
        quaternion = quaternion.max(worst_against(&quaternion_part_matrix(&p), &reference));
        normals = normals.max(worst_normals_against(&PartMat::new(&fast), &reference));
        // Sabotage: Rx · Ry · Rz instead of Rz · Ry · Rx.
        let wrong = Mat4::from_translation(Vec3::new(p[0], p[1], p[2]) / 16.0)
            * Mat4::from_rotation_x(p[3])
            * Mat4::from_rotation_y(p[4])
            * Mat4::from_rotation_z(p[5])
            * Mat4::from_scale(Vec3::new(p[6], p[7], p[8]));
        reversed_order = reversed_order.max(worst_against(&wrong, &reference));
        // Sabotage: the cofactors without the determinant's sign.
        let mut unsigned = PartMat::new(&fast);
        if p[6] * p[7] * p[8] < 0.0 {
            unsigned.n = unsigned.n.map(|c| [-c[0], -c[1], -c[2], c[3]]);
        }
        unsigned_normals = unsigned_normals.max(worst_normals_against(&unsigned, &reference));
    }
    eprintln!("worst difference to the reference: closed form {closed:e}, quaternions {quaternion:e}, normals {normals:e}");
    assert!(closed <= 1.0e-5, "closed form: {closed:e}");
    assert!(quaternion <= 1.0e-5, "quaternions: {quaternion:e}");
    assert!(normals <= 1.0e-5, "normal matrix: {normals:e}");
    assert!(reversed_order > 1.0e-2, "the test does not see the order of the axes: {reversed_order:e}");
    assert!(unsigned_normals > 0.5, "the test does not see the mirrored parts: {unsigned_normals:e}");
}

/// A random layer: `n` parts, each the child of an earlier one or a root, varied poses (turned
/// axes, mirrors), every other part with cubes.
fn random_layer(d: &mut Draws, n: usize) -> BakedLayer {
    let quad = Quad { pos: [[0.0; 3]; 4], uv: [[0.0; 2]; 4], normal: [0.0, 1.0, 0.0] };
    let parts = (0..n)
        .map(|i| {
            let parent = if i == 0 || d.unit() < 0.3 { u32::MAX } else { d.below(i) as u32 };
            let turned = d.below(8) as u32;
            let angle = |d: &mut Draws, bit: u32| if turned & bit != 0 { d.range(-3.2, 3.2) } else { 0.0 };
            let mirror = if d.unit() < 0.2 { -1.0 } else { 1.0 };
            let pose = [
                d.range(-8.0, 8.0),
                d.range(-24.0, 4.0),
                d.range(-8.0, 8.0),
                angle(d, 1),
                angle(d, 2),
                angle(d, 4),
                mirror,
                if d.unit() < 0.2 { -1.0 } else { 1.0 },
                d.range(0.8, 1.2),
            ];
            BakedPart::new(parent, pose, if i % 2 == 0 { vec![quad] } else { Vec::new() })
        })
        .collect();
    BakedLayer::new(parts, [u16::MAX; KNOWN], (0..n).map(|i| format!("part{i}")).collect())
}

/// Roots like the ones the renderer builds: entity in the world (scale, yaw, flip, lift), the
/// camera's layer offset on top, an interface root, a shulker attached to a wall.
fn test_roots(d: &mut Draws) -> Vec<Mat4> {
    let world = Mat4::from_translation(Vec3::new(d.range(-30.0, 30.0), d.range(-5.0, 5.0), d.range(-30.0, 30.0)))
        * Mat4::from_scale(Vec3::splat(d.range(0.5, 2.0)))
        * Mat4::from_rotation_y(d.range(-PI, PI))
        * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0))
        * Mat4::from_translation(Vec3::new(0.0, -1.501, 0.0));
    let eye = Vec3::new(d.range(-2.0, 2.0), d.range(-2.0, 2.0), d.range(-2.0, 2.0));
    let layered = crate::render::view_layering(eye, 1.0) * world;
    let interface = Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0))
        * Mat4::from_translation(Vec3::new(d.range(0.0, 400.0), d.range(0.0, 300.0), 50.0))
        * Mat4::from_scale(Vec3::new(30.0, 30.0, -30.0))
        * Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0))
        * Mat4::from_rotation_z(PI)
        * Mat4::from_rotation_x(d.range(-0.5, 0.5));
    let pivot = Vec3::new(0.0, 0.5, 0.0);
    let shulker = world
        * Mat4::from_translation(pivot)
        * Mat4::from_rotation_x(PI / 2.0)
        * Mat4::from_rotation_z(PI)
        * Mat4::from_translation(-pivot);
    vec![world, layered, interface, shulker]
}

/// Mats and frames of `layer` placed under `chain` with the paths `paths`, its pose randomised by
/// `d` (some parts hidden).
fn placed(layer: &BakedLayer, chain: &PartChain, paths: AnimationPaths, hidden: &[bool]) -> (Vec<PartMat>, Vec<Option<Mat4>>) {
    with_paths(paths, || {
        let mut pose = Pose::reset(layer, Vec::new(), Vec::new());
        for (v, &h) in pose.visible.iter_mut().zip(hidden) {
            *v = !h;
        }
        let mut frames = PartFrames::default();
        let mut mats = vec![PartMat::zeroed()];
        let first = place_part_chain(&pose, chain, &mut frames, &mut mats);
        assert_eq!(first, 1);
        let frames = (0..layer.parts.len()).map(|i| frames.get(i)).collect();
        (mats, frames)
    })
}

fn frame_bits(f: &Option<Mat4>) -> Option<Vec<u32>> {
    f.map(|m| m.to_cols_array().map(canon).to_vec())
}

/// The affine chain places the same matrices and frames as the full 4 × 4 products (up to the
/// sign of zeros) for random layers under every kind of root, with and without layer offset, cape
/// swing and parts hung on the root; every such root is affine.
#[test]
fn affine_chain_matches_full_matrices() {
    let mut d = Draws(0x5eed_a3f1);
    let full = AnimationPaths { affine_part_chain: false, ..AnimationPaths::NEW };
    let affine = AnimationPaths { affine_part_chain: true, ..AnimationPaths::NEW };
    let mut compared = 0;
    for round in 0..400 {
        let layer = random_layer(&mut d, 1 + round % 14);
        let hidden: Vec<bool> = (0..layer.parts.len()).map(|_| d.unit() < 0.15).collect();
        for root in test_roots(&mut d) {
            assert!(is_affine(&root), "{root:?}");
            let offset = crate::render::view_layering(Vec3::new(d.range(-3.0, 3.0), 1.6, d.range(-3.0, 3.0)), 1.0);
            assert!(is_affine(&offset));
            let turn = Mat4::from_rotation_y(-PI) * Mat4::from_rotation_x(d.range(0.0, 1.0));
            let n = layer.parts.len();
            let chains = [
                PartChain::under(root),
                PartChain::under(root).with_drawn_offset(offset),
                PartChain { geometry_turn: Some(turn), ..PartChain::under(root) },
                PartChain { hung_on_root: Some([Some(d.below(n)), None]), ..PartChain::under(root) },
            ];
            for chain in chains {
                let (full_mats, full_frames) = placed(&layer, &chain, full, &hidden);
                let (affine_mats, affine_frames) = placed(&layer, &chain, affine, &hidden);
                for (i, (a, b)) in full_mats.iter().zip(&affine_mats).enumerate() {
                    assert_eq!(mat_bits(a), mat_bits(b), "round {round}, part {i}");
                }
                let bits = |f: &[Option<Mat4>]| f.iter().map(frame_bits).collect::<Vec<_>>();
                assert_eq!(bits(&full_frames), bits(&affine_frames), "round {round}: frames");
                compared += full_frames.iter().flatten().count();
            }
        }
    }
    assert!(compared > 20_000, "{compared} frames compared");
}

/// The per-site loops the chain replaced, kept as the reference of the move: parent before child,
/// root parts under `root` (or only the `hung` ones, the other root parts dropped), the cape swing
/// after the local matrix of the parts with faces, the layer offset on the drawn matrix only.
fn old_loop(pose: &Pose<'_>, root: Mat4, offset: Option<Mat4>, turn: Option<Mat4>, hung: Option<[Option<usize>; 2]>) -> Vec<PartMat> {
    let parts = &pose.layer.parts;
    let mut world: Vec<Option<Mat4>> = vec![None; parts.len()];
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let parent = match hung {
            Some(h) if Some(i) == h[0] || Some(i) == h[1] => Some(root),
            Some(_) if part.parent == u32::MAX => None,
            _ if part.parent == u32::MAX => Some(root),
            _ => world[part.parent as usize],
        };
        let own = match turn {
            Some(t) if !part.quads.is_empty() => quaternion_part_matrix(&pose.parts[i]) * t,
            _ => quaternion_part_matrix(&pose.parts[i]),
        };
        let pm = parent.filter(|_| pose.visible[i]).map(|p| p * own);
        world[i] = pm;
        out.push(match (pm, offset) {
            (Some(pm), Some(lay)) => PartMat::new(&(lay * pm)),
            (Some(pm), None) => PartMat::new(&pm),
            (None, _) => PartMat::zeroed(),
        });
    }
    out
}

/// The one chain places what each removed loop placed: bit for bit on the old paths, up to the
/// sign of zeros on the exact new ones (affine products, baked rest matrices), for random layers
/// posed away from and at their definition pose, under every kind of chain.
#[test]
fn chain_matches_the_old_loops() {
    let mut d = Draws(0x5eed_100b);
    let old = AnimationPaths {
        closed_form_rotation: false,
        cached_static_parts: false,
        affine_part_chain: false,
        ..AnimationPaths::NEW
    };
    let exact_new = AnimationPaths { closed_form_rotation: false, affine_part_chain: true, ..AnimationPaths::NEW };
    let mut differ = std::collections::BTreeSet::new();
    for round in 0..400 {
        let layer = random_layer(&mut d, 1 + round % 14);
        let n = layer.parts.len();
        let mut pose = Pose::reset(&layer, Vec::new(), Vec::new());
        for (i, (q, v)) in pose.parts.iter_mut().zip(pose.visible.iter_mut()).enumerate() {
            if i % 3 == 0 {
                q[3] += d.range(-1.0, 1.0);
            }
            *v = d.unit() > 0.15;
        }
        let root = test_roots(&mut d)[round % 4];
        let offset = crate::render::view_layering(Vec3::new(0.4, 1.6, -0.3), 1.0);
        let turn = Mat4::from_rotation_y(-PI) * Mat4::from_rotation_x(d.range(0.0, 1.0));
        let hung = [Some(d.below(n)), (d.unit() < 0.5).then(|| d.below(n))];
        for (name, offset, turn, hung) in [
            ("plain", None, None, None),
            ("layer offset", Some(offset), None, None),
            ("identity offset", Some(Mat4::IDENTITY), None, None),
            ("cape swing", None, Some(turn), None),
            ("hung on the root", None, None, Some(hung)),
        ] {
            let reference = old_loop(&pose, root, offset, turn, hung);
            let chain = PartChain { root, drawn_offset: offset, geometry_turn: turn, hung_on_root: hung };
            for (paths, exact_bits) in [(old, true), (exact_new, false)] {
                let mats = with_paths(paths, || {
                    let (mut frames, mut mats) = (PartFrames::default(), Vec::new());
                    place_part_chain(&pose, &chain, &mut frames, &mut mats);
                    mats
                });
                assert_eq!(mats.len(), reference.len());
                let same = mats.iter().zip(&reference).all(|(a, b)| {
                    if exact_bits { bytemuck::bytes_of(a) == bytemuck::bytes_of(b) } else { mat_bits(a) == mat_bits(b) }
                });
                if !same {
                    differ.insert(format!("{name}, {}", if exact_bits { "old paths" } else { "new paths" }));
                }
            }
        }
    }
    assert!(differ.is_empty(), "not the old loops: {differ:?}");
}

/// Avatars wearing what the golden scene lacks (cape, elytra, chestplate under a cape, deadmau5
/// ears, spin attack, shoulder parrots, stuck arrows and stingers), drawn in the world, in the
/// interface and as the first-person arm: each exact switch of the chain alone and off places the
/// old path's matrices, and the shared layers draw the same matrices.
#[test]
fn avatar_extras_same_under_every_chain_switch() {
    let Some(mut k) = kit() else { return };
    let player = scene(&k).into_iter().find(|e| e.kind == "player").expect("player");
    let stack = |k: &Kit, n: &str| {
        Some(rc_proto_v777::item::ItemStack { item: k.item(n), count: 1, added: vec![], removed: vec![] })
    };
    let mut ents = Vec::new();
    for i in 0..8 {
        let mut e = player.clone();
        e.id = 30_000 + i;
        e.uuid = 800_000 + i as u128;
        e.pos[0] += f64::from(i) * 1.5 - 5.0;
        e.pos_o[0] += f64::from(i) * 1.5 - 5.0;
        e.meta.insert(16, Meta::Byte(0x7f));
        match i {
            0 | 2 => {
                k.r.custom_capes.insert(e.uuid, [0.0, 0.0, 0.0625, 0.03125]);
            }
            1 => e.equipment[4] = stack(&k, "minecraft:elytra"),
            3 => {
                k.r.extra_ears.insert(e.uuid);
            }
            4 => e.meta.insert(8, Meta::Byte(4)),
            5 => {
                e.meta.insert(19, Meta::OptionalInt(Some(1)));
                e.meta.insert(20, Meta::OptionalInt(Some(3)));
            }
            6 => {
                e.meta.insert(12, Meta::Int(5));
                e.meta.insert(13, Meta::Int(2));
            }
            _ => e.equipment[3] = stack(&k, "minecraft:iron_leggings"),
        }
        if i == 2 {
            e.equipment[4] = stack(&k, "minecraft:iron_chestplate");
        }
        ents.push(e);
    }
    let draw = |k: &mut Kit, opt: EntOpt| {
        prep(k, &ents, opt);
        let paths = AnimationPaths::of(&opt);
        with_paths(paths, || {
            k.r.push_gui_entity(&ents[0], Mat4::from_scale(Vec3::splat(30.0)), Some(&k.items));
            for right in [true, false] {
                k.r.push_hand_arm(&ents[3], right, Mat4::from_translation(Vec3::new(0.3, -0.4, -0.7)), 0xf0f0);
            }
        });
        k.r.mats.len()
    };
    let exact_new =
        EntOpt { poly_rotations: false, closed_form_part_rotation: false, share_layer_matrices: false, ..EntOpt::NEW };
    let old = EntOpt { animation_sine_table: true, ..EntOpt::OLD };
    draw(&mut k, exact_new);
    draw(&mut k, old);
    let reference = super::tests_c3m11::digest(&k.r);
    assert!(k.r.drawn == ents.len(), "{} avatars drawn", k.r.drawn);
    let cape = k.r.layer_index["minecraft:player#cape"];
    let wings = k.r.layer_index["minecraft:elytra#main"];
    let drawn_layers: Vec<u32> = k.r.instances.iter().flatten().map(|(_, first, _, _)| *first).collect();
    for layer in [cape, wings] {
        assert!(drawn_layers.contains(&k.r.layers[layer].first), "{} not drawn", k.r.layers[layer].name);
    }
    let off = |f: fn(&mut EntOpt)| {
        let mut o = exact_new;
        f(&mut o);
        o
    };
    for (name, opt) in [
        ("every exact path", exact_new),
        ("CACHED_STATIC_PART_MATRICES=0", off(|o| o.cached_static_part_matrices = false)),
        ("AFFINE_PART_CHAIN=0", off(|o| o.affine_part_chain = false)),
        ("CONTIGUOUS_DEFAULT_POSE=0", off(|o| o.contiguous_default_pose = false)),
        ("BASE_POSE_BY_SWAP=0", off(|o| o.base_pose_by_swap = false)),
        ("CACHED_STATIC_PART_MATRICES alone", EntOpt { cached_static_part_matrices: true, ..old }),
        ("AFFINE_PART_CHAIN alone", EntOpt { affine_part_chain: true, ..old }),
        ("CONTIGUOUS_DEFAULT_POSE alone", EntOpt { contiguous_default_pose: true, ..old }),
        ("BASE_POSE_BY_SWAP alone", EntOpt { base_pose_by_swap: true, ..old }),
    ] {
        draw(&mut k, opt);
        assert_eq!(super::tests_c3m11::digest(&k.r), reference, "{name}");
    }
    let shared = EntOpt { share_layer_matrices: true, ..exact_new };
    draw(&mut k, exact_new);
    let placed_again = drawn_digest(&k.r);
    draw(&mut k, shared);
    assert_eq!(drawn_digest(&k.r), placed_again, "shared layers");
}

/// The rest matrix baked with a part is bit for bit the one rebuilt from its pose under both
/// rotation formulas, and the cached chain places exactly the rebuilt chain's matrices (no
/// canonical zeros: the same function on the same floats).
#[test]
fn cached_static_parts_match_rebuilt_matrices() {
    for p in test_poses() {
        let part = BakedPart::new(u32::MAX, p, Vec::new());
        for closed_form in [false, true] {
            let rebuilt = part_matrix_with(&p, closed_form).to_cols_array().map(f32::to_bits);
            assert_eq!(part.rest_local[usize::from(closed_form)].to_cols_array().map(f32::to_bits), rebuilt, "{p:?}");
        }
    }
    let mut d = Draws(0x5eed_ca5e);
    let mut cached_parts = 0;
    for round in 0..300 {
        let layer = random_layer(&mut d, 1 + round % 14);
        let root = test_roots(&mut d)[round % 4];
        for closed_form in [false, true] {
            let paths = |cached| AnimationPaths {
                cached_static_parts: cached,
                closed_form_rotation: closed_form,
                ..AnimationPaths::NEW
            };
            // Half the parts animated away from their definition pose, the rest left alone.
            let mut animated = Pose::reset(&layer, Vec::new(), Vec::new());
            for (i, q) in animated.parts.iter_mut().enumerate() {
                if i % 2 == 1 {
                    q[3] += 0.25;
                } else {
                    cached_parts += 1;
                }
            }
            let mut out = Vec::new();
            for cached in [false, true] {
                out.push(with_paths(paths(cached), || {
                    let mut frames = PartFrames::default();
                    let mut mats = Vec::new();
                    place_part_chain(&animated, &PartChain::under(root), &mut frames, &mut mats);
                    mats.iter().map(|m| bytemuck::bytes_of(m).to_vec()).collect::<Vec<_>>()
                }));
            }
            assert_eq!(out[0], out[1], "round {round}, closed form {closed_form}");
        }
    }
    assert!(cached_parts > 1000);
}

/// Parts in each geometry range of the baked layers: how many matrices an instance drawing that
/// layer reads.
fn parts_by_geometry(r: &EntityRenderer) -> HashMap<(u32, u32), usize> {
    r.layers.iter().map(|l| ((l.first, l.count), l.baked.parts.len())).collect()
}

/// Fingerprint of what each instance draws, wherever its matrices sit: its fields but the index of
/// its first matrix, then the matrices it reads (its layer's parts; one for the other geometries).
pub(super) fn drawn_digest(r: &EntityRenderer) -> u64 {
    let parts = parts_by_geometry(r);
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let lists = r.instances.iter().chain(std::iter::once(&r.glow_only));
    for (p, list) in lists.enumerate() {
        fnv(&mut h, &(p as u32).to_le_bytes());
        for (src, first, count, inst) in list {
            fnv(&mut h, &[*src]);
            fnv(&mut h, &first.to_le_bytes());
            fnv(&mut h, &count.to_le_bytes());
            fnv(&mut h, bytemuck::bytes_of(&Instance { base: 0, ..*inst }));
            let read = if src & 0x0f == 0 { parts.get(&(*first, *count)).copied().unwrap_or(1) } else { 1 };
            let base = inst.base as usize;
            for m in &r.mats[base..(base + read).min(r.mats.len())] {
                for b in mat_bits(m) {
                    fnv(&mut h, &b.to_le_bytes());
                }
            }
        }
    }
    h
}

/// The scene of the shared layers: the golden scene, then villagers and zombie villagers of every
/// type, profession and level (four layers of one model, some hatless), visible spiders and
/// endermen (eyes), babies.
fn shared_layers_scene(k: &Kit) -> Vec<Entity> {
    let mut out = scene(k);
    let professions = k.r.source.registry("minecraft:villager_profession").map_or(1, |r| r.entries.len());
    let types = k.r.source.registry("minecraft:villager_type").map_or(1, |r| r.entries.len());
    let template = |kind: &str| out.iter().find(|e| e.kind == kind).cloned().unwrap_or_else(|| panic!("{kind}"));
    let (villager, zombie, spider, enderman) =
        (template("villager"), template("zombie_villager"), template("spider"), template("enderman"));
    let mut extra = Vec::new();
    for i in 0..48 {
        let mut e = match i % 6 {
            0..=2 => villager.clone(),
            3 => zombie.clone(),
            4 => spider.clone(),
            _ => enderman.clone(),
        };
        e.id = 20_000 + i;
        e.uuid = 700_000 + i as u128;
        let shift = [(i % 8) as f64 * 1.6 - 6.0, 0.0, 3.0 + (i / 8) as f64 * 1.8];
        for (a, s) in shift.into_iter().enumerate() {
            e.pos[a] += s;
            e.pos_o[a] += s;
        }
        e.meta.insert(0, Meta::Byte(0));
        let data = [i % types as i32, i % professions as i32, 1 + i % 5];
        e.meta.insert(if e.kind == "villager" { 19 } else { 20 }, Meta::Villager(data));
        if i % 7 == 0 {
            e.meta.insert(16, Meta::Bool(true));
        }
        extra.push(e);
    }
    out.extend(extra);
    out
}

/// Every instance draws the same matrices whether a layer of the model already placed shares them
/// or places them again, and the sharing places fewer matrices.
#[test]
fn shared_layer_matrices_draw_the_same() {
    let Some(mut k) = kit() else { return };
    let ents = shared_layers_scene(&k);
    let placed_again = EntOpt { share_layer_matrices: false, ..EntOpt::NEW };
    prep(&mut k, &ents, EntOpt::NEW);
    prep(&mut k, &ents, placed_again);
    let (reference, again_mats) = (drawn_digest(&k.r), k.r.mats.len());
    assert!(k.r.drawn > 100, "{} entities drawn", k.r.drawn);
    prep(&mut k, &ents, EntOpt::NEW);
    assert_eq!(drawn_digest(&k.r), reference, "shared layers draw other matrices");
    let shared_mats = k.r.mats.len();
    eprintln!("matrices placed: {again_mats} placed again, {shared_mats} shared");
    assert!(shared_mats + 500 < again_mats, "the scene shares almost nothing: {again_mats} → {shared_mats}");
    // Every switch of the chain alone against the shared default.
    for (name, opt) in [
        ("SHARE_LAYER_MATRICES alone", EntOpt { share_layer_matrices: true, ..EntOpt::OLD }),
        ("BASE_POSE_BY_SWAP=0", EntOpt { base_pose_by_swap: false, ..EntOpt::NEW }),
    ] {
        let reference_of = |k: &mut Kit, o: EntOpt| {
            prep(k, &ents, o);
            drawn_digest(&k.r)
        };
        let own = reference_of(&mut k, opt);
        let again = reference_of(&mut k, EntOpt { share_layer_matrices: false, ..opt });
        assert_eq!(own, again, "{name}");
    }
}

/// The baked faces are freed after the upload (the renderer of the kit runs with the default
/// switches) and each part keeps whether it had any; the bytes freed are counted.
#[test]
fn freed_baked_quads_keep_the_geometry_flag() {
    let Some(k) = kit() else { return };
    let mut parts_with_cubes = 0;
    let mut freed = 0usize;
    for (gl, def) in k.r.layers.iter().zip(&k.r.source.entity_models) {
        let rebaked = bake_layer(def);
        for (part, again) in gl.baked.parts.iter().zip(&rebaked.parts) {
            assert_eq!(part.has_geometry, !again.quads.is_empty(), "{}", gl.name);
            assert!(part.quads.is_empty(), "{}: faces kept", gl.name);
            parts_with_cubes += usize::from(part.has_geometry);
            freed += again.quads.capacity() * std::mem::size_of::<Quad>();
        }
    }
    let parts: usize = k.r.layers.iter().map(|l| l.baked.parts.len()).sum();
    eprintln!(
        "{} layers, {parts} parts ({parts_with_cubes} with cubes): {:.2} MB of faces freed, rest matrices {:.2} MB",
        k.r.layers.len(),
        freed as f64 / 1.0e6,
        (parts * std::mem::size_of::<[Mat4; 2]>()) as f64 / 1.0e6
    );
    assert!(parts_with_cubes > 1000 && freed > 100_000, "{parts_with_cubes} parts, {freed} bytes");
}

/// Micro-bench of the chain alone (local matrix, product, matrix and normals drawn), every baked
/// layer of the game posed like an animation does (legs about X, heads about X and Y, the rest at
/// rest) under an entity root, each configuration in turn, ns per part (median of the rounds).
/// `cargo test --release -p rc-app -- --ignored bench_part_chain --nocapture`.
#[test]
#[ignore]
fn bench_part_chain() {
    let Some(k) = kit() else { return };
    let mut d = Draws(0x5eed_be9c);
    let mut posed = Vec::new();
    for gl in &k.r.layers {
        let mut parts: Vec<[f32; 9]> = gl.baked.default_pose.clone();
        for (q, name) in parts.iter_mut().zip(&gl.baked.names) {
            if name.ends_with("_leg") || name.ends_with("_arm") {
                q[3] = d.range(-0.8, 0.8);
            } else if name == "head" {
                (q[3], q[4]) = (d.range(-0.5, 0.5), d.range(-0.9, 0.9));
            }
        }
        posed.push(parts);
    }
    let total: usize = k.r.layers.iter().map(|l| l.baked.parts.len()).sum();
    let root = test_roots(&mut d)[0];
    let old = AnimationPaths {
        closed_form_rotation: false,
        cached_static_parts: false,
        affine_part_chain: false,
        contiguous_default_pose: false,
        ..AnimationPaths::NEW
    };
    let default = AnimationPaths::NEW;
    let configs = [
        ("old", old),
        ("default", default),
        ("default + AFFINE", AnimationPaths { affine_part_chain: true, ..default }),
        ("CLOSED_FORM alone", AnimationPaths { closed_form_rotation: true, ..old }),
        ("CACHED_STATIC alone", AnimationPaths { cached_static_parts: true, ..old }),
        ("AFFINE alone", AnimationPaths { affine_part_chain: true, ..old }),
        ("default but CLOSED_FORM", AnimationPaths { closed_form_rotation: false, ..default }),
        ("default but CACHED_STATIC", AnimationPaths { cached_static_parts: false, ..default }),
    ];
    let mut frames = PartFrames::default();
    let mut mats = Vec::with_capacity(total + 16);
    let mut buffers = (Vec::new(), Vec::new());
    let mut times = vec![Vec::new(); configs.len()];
    for round in 0..61 {
        for (c, (_, paths)) in configs.iter().enumerate() {
            let ns = with_paths(*paths, || {
                let t = std::time::Instant::now();
                for (gl, parts) in k.r.layers.iter().zip(&posed) {
                    let mut pose =
                        Pose::reset(&gl.baked, std::mem::take(&mut buffers.0), std::mem::take(&mut buffers.1));
                    pose.parts.copy_from_slice(parts);
                    place_part_chain(&pose, &PartChain::under(root), &mut frames, &mut mats);
                    buffers = (pose.parts, pose.visible);
                }
                let ns = t.elapsed().as_secs_f64() * 1e9 / total as f64;
                mats.clear();
                ns
            });
            if round > 0 {
                times[c].push(ns);
            }
        }
    }
    println!("part chain, {total} parts, ns per part (median of 60 rounds; pose reset included):");
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let base = median(&mut times[0]);
    for (c, (name, _)) in configs.iter().enumerate() {
        let m = median(&mut times[c]);
        println!("  {name:>22}: {m:>7.2} ns ({:+.1}%)", (m / base - 1.0) * 100.0);
    }
}

/// Micro-bench of the pose reset alone (side-by-side copy against the gather out of the parts),
/// every baked layer in turn, ns per part.
#[test]
#[ignore]
fn bench_pose_reset() {
    let Some(k) = kit() else { return };
    let total: usize = k.r.layers.iter().map(|l| l.baked.parts.len()).sum();
    let mut buffers = (Vec::new(), Vec::new());
    let mut times = [Vec::new(), Vec::new()];
    for round in 0..201 {
        for (c, contiguous) in [false, true].into_iter().enumerate() {
            let paths = AnimationPaths { contiguous_default_pose: contiguous, ..AnimationPaths::NEW };
            let ns = with_paths(paths, || {
                let t = std::time::Instant::now();
                for gl in &k.r.layers {
                    let pose = Pose::reset(&gl.baked, std::mem::take(&mut buffers.0), std::mem::take(&mut buffers.1));
                    buffers = (pose.parts, pose.visible);
                }
                t.elapsed().as_secs_f64() * 1e9 / total as f64
            });
            if round > 0 {
                times[c].push(ns);
            }
        }
    }
    for (c, name) in ["gather (old)", "side-by-side copy"].into_iter().enumerate() {
        times[c].sort_by(f64::total_cmp);
        println!("pose reset, {name}: {:.3} ns per part", times[c][times[c].len() / 2]);
    }
}
