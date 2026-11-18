//! Entity rendering and glints (G1-F08): special glint of compasses and the clock, baby sniffer
//! texture, cape and elytra of avatars. Expected values written from the 26.3 game
//! (rules described in our own words), no extracted data.

use super::keyframes::testkit;
use super::*;
use crate::item_atlas::special_glint_uv;
use rc_assets::geom::Dir;
use rc_proto_v777::entity::Meta;

/// The game's projection redone step by step: position / k, half-turn around y, quarter turn
/// backwards around x, face-specific rotation, then (−x, −y) / 128.
fn decal_reference(p: [f32; 3], dir: Dir, k: f32) -> [f32; 2] {
    use glam::Mat3;
    let face = match dir {
        Dir::Down => Mat3::from_rotation_x(PI),
        Dir::Up => Mat3::IDENTITY,
        Dir::North => Mat3::from_rotation_x(PI / 2.0) * Mat3::from_rotation_z(PI),
        Dir::South => Mat3::from_rotation_x(PI / 2.0),
        Dir::West => Mat3::from_rotation_x(PI / 2.0) * Mat3::from_rotation_z(PI / 2.0),
        Dir::East => Mat3::from_rotation_x(PI / 2.0) * Mat3::from_rotation_z(-PI / 2.0),
    };
    let v = face * Mat3::from_rotation_x(-PI / 2.0) * Mat3::from_rotation_y(PI) * (Vec3::from(p) / k);
    [-v.x / 128.0, -v.y / 128.0]
}

/// The per-face table follows the game's sequence of rotations, for each face and each context
/// scale (interface 0.5, first person 0.75, elsewhere 1).
#[test]
fn special_glint_projection_matches_rotations() {
    let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.5], [0.25, 0.75, 0.5], [0.53125, 0.1, 0.96875], [1.0, 1.0, 1.0]];
    for dir in Dir::ALL {
        for k in [0.5, 0.75, 1.0] {
            for p in points {
                let (a, b) = (special_glint_uv(p, dir, k), decal_reference(p, dir, k));
                assert!(
                    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6,
                    "{dir:?} k {k} {p:?}: {a:?} ≠ {b:?}"
                );
            }
        }
    }
    // Hand-written values: top face in the interface, south face elsewhere.
    assert_eq!(special_glint_uv([1.0, 0.0, 0.5], Dir::Up, 0.5), [2.0 / 128.0, 1.0 / 128.0]);
    assert_eq!(special_glint_uv([0.5, 0.25, 0.0], Dir::South, 1.0), [0.5 / 128.0, -0.25 / 128.0]);
}

/// Light word: the special glint keeps the "item" mode and adds its flag, read by the
/// shader (bit 3 of the flags); the standard glint does not carry it.
#[test]
fn special_glint_reaches_the_shader() {
    assert_eq!(item_glint_bits(0), 0);
    assert_eq!(item_glint_bits(1), GLINT_ITEM);
    let s = item_glint_bits(2);
    assert_eq!(s & (3 << 16), GLINT_ITEM);
    assert_eq!((s >> 18) & 31, 8);
    assert_eq!((LIGHT_FIRST_PERSON >> 18) & 63, 16);
    assert_eq!((LIGHT_GUI >> 18) & 63, 32);
    let wgsl = include_str!("../entity.wgsl");
    assert!(wgsl.contains("(light >> 18u) & 63u"), "special glint flags cut from the shader");
}

/// Scale of the special glint depending on the network context of an item display (0 none,
/// 1-2 third person, 3-4 first person, 5 head, 6 interface, 7 ground, 8 frame,
/// 9 shelf): interface 0.5, first person 0.75, elsewhere 1.
#[test]
fn display_context_sets_glint_scale() {
    let want = [0, 0, 0, LIGHT_FIRST_PERSON, LIGHT_FIRST_PERSON, 0, LIGHT_GUI, 0, 0, 0];
    for (net, w) in want.into_iter().enumerate() {
        assert_eq!(context_glint_bits(objects::display_context(net as i8)), w, "network context {net}");
    }
    assert_eq!(context_glint_bits(objects::display_context(10)), 0);
    // Hand in first person: same indices as contexts 3 and 4.
    assert_eq!(context_glint_bits(Some(4)), LIGHT_FIRST_PERSON);
    assert_eq!(context_glint_bits(Some(5)), LIGHT_FIRST_PERSON);
}

/// End to end (real engine, GPU and 26.3 jar): a glint compass placed by a stack in
/// the interface, first-person or third-person context carries the scale flag
/// on each of its instances, in addition to the special glint.
#[test]
fn special_glint_scale_reaches_instances() {
    let Some(mut k) = super::tests_g2f03::kit() else { return };
    let glint = k.r.ids.of("minecraft:enchantment_glint_override").expect("glint component");
    let compass = rc_proto_v777::item::ItemStack {
        item: k.item("minecraft:compass"),
        count: 1,
        added: vec![(glint, Box::new([1u8]))],
        removed: vec![],
    };
    for (ctx, want) in [(0, LIGHT_GUI), (4, LIGHT_FIRST_PERSON), (5, LIGHT_FIRST_PERSON), (2, 0)] {
        k.r.instances.iter_mut().for_each(Vec::clear);
        k.r.mats.clear();
        assert!(k.r.push_stack(&k.items, &compass, Some(ctx), None, Mat4::IDENTITY, 0), "compass drawn");
        let lights: Vec<u32> = k.r.instances.iter().flatten().map(|(_, _, _, i)| i.light).collect();
        assert!(!lights.is_empty());
        for l in lights {
            assert_eq!(l & (LIGHT_GLINT_SPECIAL | LIGHT_FIRST_PERSON | LIGHT_GUI), LIGHT_GLINT_SPECIAL | want, "context {ctx}");
        }
    }
}

/// The entity shader, run on the GPU, projects the special glint like the reference table
/// (`special_glint_uv`) for the 6 faces, and chooses the scale of its flags.
#[test]
fn entity_shader_glint_table_matches_reference() {
    use wgpu::util::DeviceExt;
    let Some((device, queue)) = crate::render::upload::tests::gpu() else { return };
    let src = include_str!("../entity.wgsl");
    let func = |name: &str| {
        let start = src.find(&format!("fn {name}(")).unwrap_or_else(|| panic!("{name} missing from the shader"));
        let len = src[start..].find("\n}\n").expect("end of function") + 3;
        &src[start..start + len]
    };
    let probe = format!(
        "{}\n{}\n@group(0) @binding(0) var<storage, read> inp: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> outp: array<vec4<f32>>;
@compute @workgroup_size(64)
fn probe(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = id.x;
    if i >= arrayLength(&outp) {{ return; }}
    let a = inp[2u * i];
    let b = inp[2u * i + 1u];
    let k = glint_scale(u32(b.w));
    outp[i] = vec4<f32>(special_glint_uv(a.xyz, b.xyz, k), k, 0.0);
}}",
        func("special_glint_uv"),
        func("glint_scale"),
    );
    let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.5], [0.25, 0.75, 0.5], [0.53125, 0.1, 0.96875], [1.0, 1.0, 1.0]];
    let (mut inp, mut want) = (Vec::new(), Vec::new());
    for dir in Dir::ALL {
        for (flags, k) in [(0u32, 1.0f32), (16, 0.75), (32, 0.5), (8 | 16, 0.75), (8 | 32, 0.5)] {
            for p in points {
                let n = dir.normal();
                inp.push([p[0] * 16.0, p[1] * 16.0, p[2] * 16.0, 0.0]);
                inp.push([n[0], n[1], n[2], flags as f32]);
                let uv = special_glint_uv(p, dir, k);
                want.push((dir, flags, p, [uv[0], uv[1], k]));
            }
        }
    }
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(probe.into()),
    });
    let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&inp),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let ob = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &vec![0u8; want.len() * 16],
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipe.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: ib.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: ob.as_entire_binding() },
        ],
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipe);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups((want.len() as u32).div_ceil(64), 1, 1);
    }
    queue.submit([enc.finish()]);
    let got: Vec<[f32; 4]> = bytemuck::cast_slice(&crate::render::upload::tests::read(&device, &queue, &ob)).to_vec();
    assert_eq!(got.len(), want.len());
    for (g, (dir, flags, p, w)) in got.iter().zip(&want) {
        assert!(
            (0..3).all(|c| (g[c] - w[c]).abs() < 1e-6),
            "{dir:?} flags {flags} {p:?}: GPU {g:?}, reference {w:?}"
        );
    }
}

/// Valid item-icon shader (special glint coordinates included).
#[test]
fn item_shader_validates() {
    let m = wgpu::naga::front::wgsl::parse_str(include_str!("../item.wgsl")).expect("parse");
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&m)
    .expect("validation");
}

/// Baby sniffer: baby layer, "snifflet" texture; adult unchanged.
#[test]
fn baby_sniffer_uses_snifflet_texture() {
    assert_eq!(sniffer_layer(true), ("minecraft:sniffer_baby#main", "entity/sniffer/snifflet"));
    assert_eq!(sniffer_layer(false), ("minecraft:sniffer#main", "entity/sniffer/sniffer"));
}

/// Empty partial profile followed by a patch giving a cape and/or elytra.
fn patched_profile(cape: Option<&str>, elytra: Option<&str>) -> Meta {
    let mut b = vec![0, 0, 0, 0, 0];
    for t in [cape, elytra] {
        match t {
            Some(t) => {
                b.push(1);
                b.push(t.len() as u8);
                b.extend_from_slice(t.as_bytes());
            }
            None => b.push(0),
        }
    }
    b.push(0);
    Meta::Profile(b.into())
}

fn atlas_with(names: &[&str]) -> crate::entity_atlas::EntityAtlas {
    let sources = names
        .iter()
        .map(|n| {
            let img = rc_assets::texture::Image { width: 2, height: 2, rgba: vec![255; 16] };
            ((*n).to_owned(), crate::entity_atlas::Source::Image(img))
        })
        .collect();
    crate::entity_atlas::EntityAtlas::new(sources, None)
}

/// Cape and elytra of avatars: player from its downloaded textures; mannequin from its
/// profile patch; other entities none.
#[test]
fn avatar_cape_and_elytra_sources() {
    let mut atlas = atlas_with(&["entity/test/cape", "entity/test/wings"]);
    let mut capes = HashMap::new();
    let mut elytras = HashMap::new();
    capes.insert(1u128, [1.0, 2.0, 3.0, 4.0]);
    let all = testkit::spawn("player");
    assert_eq!(avatar_extras(&mut atlas, &capes, &elytras, &all.map[&7]), (Some([1.0, 2.0, 3.0, 4.0]), None));
    elytras.insert(1u128, [5.0, 6.0, 7.0, 8.0]);
    assert_eq!(avatar_extras(&mut atlas, &capes, &elytras, &all.map[&7]).1, Some([5.0, 6.0, 7.0, 8.0]));

    let mut all = testkit::spawn("mannequin");
    let e = all.map.get_mut(&7).unwrap();
    assert_eq!(avatar_extras(&mut atlas, &capes, &elytras, e), (None, None));
    e.meta.insert(17, patched_profile(Some("minecraft:entity/test/cape"), Some("entity/test/wings")));
    let (cape, elytra) = avatar_extras(&mut atlas, &capes, &elytras, e);
    assert!(cape.is_some() && elytra.is_some() && cape != elytra, "{cape:?} {elytra:?}");

    let pig = testkit::spawn("pig");
    assert_eq!(avatar_extras(&mut atlas, &capes, &elytras, &pig.map[&7]), (None, None));
}

/// Cape shown for the player and the mannequin (bit 0 of the skin parts); elytra: elytra
/// texture of the skin even when the cape is hidden, otherwise the cape only if it is shown.
#[test]
fn cape_shown_and_wings_texture() {
    for kind in ["player", "mannequin"] {
        let mut all = testkit::spawn(kind);
        let e = all.map.get_mut(&7).unwrap();
        assert!(!shows_cape(e), "{kind}");
        e.meta.insert(16, Meta::Byte(1));
        assert!(shows_cape(e), "{kind}");
    }
    let mut all = testkit::spawn("zombie");
    all.map.get_mut(&7).unwrap().meta.insert(16, Meta::Byte(1));
    assert!(!shows_cape(&all.map[&7]));
    let (c, w) = (Some([1.0; 4]), Some([2.0; 4]));
    assert_eq!(wings_texture((c, w), false), w);
    assert_eq!(wings_texture((c, None), true), c);
    assert_eq!(wings_texture((c, None), false), None);
    assert_eq!(wings_texture((None, None), true), None);
}
