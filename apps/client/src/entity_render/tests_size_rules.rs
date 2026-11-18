//! The size rules stored at spawn (`RUSTCRAFT_OPT_KIND_SIZE_RULES`) give the same bits as the old
//! name matches for every type of the 26.3 registry over varied states; the frame's entity list
//! holds the same rows in the same walk order under `ENTITY_SORT_KEY` and `SIZE_AFTER_DISTANCE`;
//! micro-bench of the per-entity size work.

use super::tests_g2f03::{Kit, kit};
use super::*;
use crate::entities::{SizeRules, sizes_by_name as entity_by_name};
use rc_proto_v777::entity::Meta;

/// Every registry type spawned once (network id = type id), with the 26.3 dimension table.
fn every_type() -> Option<(Vec<Entity>, Vec<[f32; 4]>)> {
    let data = rc_testdata::version_data()?;
    let names = data.registry("minecraft:entity_type").expect("type registry").entries.clone();
    assert!(names.len() > 100, "type registry too short: {}", names.len());
    let mut all = crate::entities::Entities { type_names: names.clone(), ..Default::default() };
    for i in 0..names.len() as i32 {
        let add = rc_proto_v777::entity::AddEntity {
            id: i,
            uuid: 0,
            kind: i,
            pos: [0.0; 3],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(rc_client::EntityEvent::Add(Box::new(add)));
    }
    let mut ents: Vec<Entity> = all.map.into_values().collect();
    ents.sort_by_key(|e| e.kind_id);
    Some((ents, data.entity_dims.clone()))
}

/// Linear congruential draws for the varied states.
struct Draws(u64);

impl Draws {
    fn pick<T: Clone>(&mut self, from: &[T]) -> T {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        from[(self.0 >> 33) as usize % from.len()].clone()
    }
}

/// `e` in a varied state: pose, baby flags of both indices, the data read by the special sizes
/// (armor stand flags, interaction and cloud floats, cube, phantom and fish sizes, sniffer state,
/// shulker face, display range, crystal beam), scale, opening, standing bear, age.
fn varied(e: &Entity, draws: &mut Draws) -> Entity {
    let mut v = e.clone();
    let set = |v: &mut Entity, i: u8, m: Option<Meta>| {
        if let Some(m) = m {
            v.meta.insert(i, m);
        }
    };
    let pose = draws.pick(&[None, Some(0), Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7), Some(10), Some(13), Some(14)]);
    set(&mut v, 6, pose.map(Meta::Pose));
    let at16 = [None, Some(Meta::Bool(true)), Some(Meta::Bool(false)), Some(Meta::Int(3)), Some(Meta::Direction(0)), Some(Meta::Direction(1))];
    set(&mut v, 16, draws.pick(&at16));
    let at17 = [
        None,
        Some(Meta::Bool(true)),
        Some(Meta::Bool(false)),
        Some(Meta::Int(0)),
        Some(Meta::Int(1)),
        Some(Meta::Int(2)),
        Some(Meta::Float(0.5)),
        Some(Meta::Float(2.0)),
    ];
    set(&mut v, 17, draws.pick(&at17));
    set(&mut v, 15, draws.pick(&[None, Some(Meta::Byte(0)), Some(Meta::Byte(1)), Some(Meta::Byte(0x10)), Some(Meta::Byte(0x11))]));
    set(&mut v, 18, draws.pick(&[None, Some(Meta::Int(0)), Some(Meta::Int(3)), Some(Meta::Int(5)), Some(Meta::Int(200))]));
    let at8 = [None, Some(Meta::Float(0.0)), Some(Meta::Float(2.5)), Some(Meta::BlockPos(Some([0, 64, 0]))), Some(Meta::BlockPos(None))];
    set(&mut v, 8, draws.pick(&at8));
    set(&mut v, 9, draws.pick(&[None, Some(Meta::Float(0.0)), Some(Meta::Float(1.5))]));
    v.scale = draws.pick(&[1.0, 0.25, 2.5, 5.0, 0.0]);
    v.peek = draws.pick(&[0.0, 0.4, 1.0]);
    v.extra[0] = draws.pick(&[0.0, 3.0, 6.0]);
    v.age = draws.pick(&[0, 1, 2, 100]);
    v
}

fn size_bits(s: Size) -> [u32; 5] {
    [s.w.to_bits(), s.h.to_bits(), s.eye.to_bits(), s.tag.to_bits(), u32::from(s.fixed)]
}

/// The rules stay small: they live in every entity, read on every frame.
#[test]
fn size_rules_stay_small() {
    eprintln!(
        "Entity {} bytes: kind at {}, class at {}, kind_id at {}, facts at {} ({} bytes, size rules {} bytes), meta at {}, scale at {}",
        std::mem::size_of::<Entity>(),
        std::mem::offset_of!(Entity, kind),
        std::mem::offset_of!(Entity, class),
        std::mem::offset_of!(Entity, kind_id),
        std::mem::offset_of!(Entity, facts),
        std::mem::size_of::<crate::entities::KindFacts>(),
        std::mem::size_of::<SizeRules>(),
        std::mem::offset_of!(Entity, meta),
        std::mem::offset_of!(Entity, scale),
    );
    // No float inside: one byte per rule, aligned on a byte, so they pack among the entity's
    // other small fields.
    assert_eq!((std::mem::size_of::<SizeRules>(), std::mem::align_of::<SizeRules>()), (7, 1));
}

/// Squared camera distances around every fixed limit and the young-projectile radius.
const SQUARED_DISTANCES: [f64; 14] = [0.0, 10.0, 12.0, 13.0, 500.0, 1000.0, 1100.0, 4000.0, 4100.0, 16000.0, 17000.0, 1e5, 1e6, 1e7];

/// Whatever the rules (stored at spawn) and the old name matches answer differently for `e`,
/// as text; also when a distance rule said to ignore the size reads it. Both paths are called
/// past `RUSTCRAFT_OPT_KIND_SIZE_RULES`, so the comparison holds whatever its value; the switched
/// `size` is checked to follow the path the switch picks.
fn rule_differences(e: &Entity, dims: &[[f32; 4]]) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |what: &str, same: bool| {
        if !same {
            out.push(what.to_owned());
        }
    };
    let living = e.class != Kind::Other;
    let old_base = entity_by_name::base_size(e, dims);
    let old_full = if living { old_base.scale(entity_by_name::attribute_scale(e)) } else { old_base };
    let new_base = e.base_size_by_rules(dims);
    let new_full = if living { new_base.scale(e.attribute_scale_by_rules()) } else { new_base };
    check("base_size", size_bits(new_base) == size_bits(old_base));
    check("size", size_bits(new_full) == size_bits(old_full));
    let switched = if kind_size_rules_on() { new_full } else { old_full };
    check("size past the switch", size_bits(e.size(dims)) == size_bits(switched));
    check("baby", e.baby_by_rules() == entity_by_name::baby(e));
    check("size_factor", e.size_factor_by_rules().map(f32::to_bits) == entity_by_name::size_factor(e).map(f32::to_bits));
    check("attribute_scale", e.attribute_scale_by_rules().to_bits() == entity_by_name::attribute_scale(e).to_bits());
    check("standing_eye", e.standing_eye_by_rules(dims).to_bits() == entity_by_name::standing_eye(e, dims).to_bits());
    // Under the rules, the living scale is the scale attribute's bound.
    check("living_scale", e.attribute_scale_by_rules().to_bits() == sizes_by_name::living_scale(e).to_bits());
    check("crouch_offset", crouch_offset_by_rules(e).to_bits() == sizes_by_name::crouch_offset(e).to_bits());
    check("shadow_scale", shadow_scale_by_rules(e).to_bits() == sizes_by_name::shadow_scale(e).to_bits());
    let ignores_size = !e.facts.size_rules.distance.reads_size();
    for d2 in SQUARED_DISTANCES {
        for view_scale in [0.5, 1.0, 2.5] {
            let reads = std::cell::Cell::new(0);
            let full = || {
                reads.set(reads.get() + 1);
                old_full
            };
            let new = within_render_distance_by_rules(e, d2, view_scale, full);
            let old = sizes_by_name::within_render_distance_of(e, d2, view_scale, full);
            check("within_render_distance", new == old);
            check("distance rule ignoring the size reads it", !ignores_size || reads.get() == 0);
        }
    }
    out
}

/// Types whose rules differ from the name matches somewhere over 300 varied states each.
fn mismatching_types(ents: &[Entity], dims: &[[f32; 4]]) -> Vec<String> {
    let mut differ = Vec::new();
    for e in ents {
        let mut draws = Draws(0x5eed ^ e.kind_id as u64);
        let mut what: Vec<String> = (0..300).flat_map(|_| rule_differences(&varied(e, &mut draws), dims)).collect();
        what.dedup();
        if !what.is_empty() {
            differ.push(format!("{}: {}", e.kind, what.join(", ")));
        }
    }
    differ
}

/// The rules stored at spawn give the old name matches' bits for every registry type and every
/// varied state (sizes, baby, scale bound, standing eyes, shadow factor, crouch offset, render
/// distance at every view scale), and a distance rule said to ignore the size never reads it.
/// Each varied state also varies the data read by the other kinds, so a rule given to the wrong
/// kind shows.
#[test]
fn size_rules_match_the_name_rules_for_every_type() {
    let Some((mut ents, dims)) = every_type() else { return };
    let differ = mismatching_types(&ents, &dims);
    assert!(differ.is_empty(), "rules not the name matches: {differ:?}");
    // Visible sabotage: a camel without its sitting size, a phantom drawn by size, a piglin whose
    // baby flag is read at the usual index are each caught, and only them.
    let mut sabotage = |kind: &str, edit: &dyn Fn(&mut SizeRules)| {
        let e = ents.iter_mut().find(|e| e.kind == kind).expect("kind in the registry");
        let kept = e.facts.size_rules;
        edit(&mut e.facts.size_rules);
        let caught = mismatching_types(std::slice::from_ref(&*e), &dims);
        e.facts.size_rules = kept;
        assert_eq!(caught.len(), 1, "sabotage of {kind} not caught");
    };
    sabotage("camel", &|r| r.class = SizeClass::Plain);
    sabotage("phantom", &|r| r.distance = DistanceRule::BySize);
    sabotage("piglin", &|r| r.baby_flag_index = 16);
    sabotage("turtle", &|r| *r = r.with_baby_shadow_of(""));
}

/// The frame's list holds the same rows (entity, distance, sizes, kind flags) walked in the same
/// order under `ENTITY_SORT_KEY` and `SIZE_AFTER_DISTANCE`, alone or together, over every
/// registry type near the camera, beyond its render distance and newly spawned (young
/// projectiles), with several entities at the same distance (ties keep the listing order).
#[test]
fn frame_list_rows_and_order_match_under_every_switch() {
    let Some(mut k) = kit() else { return };
    let names = k.r.source.registry("minecraft:entity_type").expect("types").entries.clone();
    let mut all = crate::entities::Entities { type_names: names.clone(), ..Default::default() };
    let mut ents = Vec::new();
    for (i, _) in names.iter().enumerate() {
        // Near, at the same distance as the next type (a tie), far, and young near the camera.
        for (copy, pos) in [[3.0, 66.0, 1.0], [-2.5, 66.0, -2.0], [180.0, 70.0, 90.0], [0.5, 66.0, -1.0]].into_iter().enumerate() {
            let id = (i * 4 + copy) as i32 + 1;
            let add = rc_proto_v777::entity::AddEntity {
                id,
                uuid: id as u128,
                kind: i as i32,
                pos,
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            all.apply(rc_client::EntityEvent::Add(Box::new(add)));
            let mut e = all.map.remove(&id).expect("entity added");
            e.age = if copy == 3 { 1 } else { 100 };
            ents.push(e);
        }
    }
    let walked = |k: &mut Kit, opt: EntOpt| {
        k.r.opt = opt;
        let list = k.r.frame_list(ents.iter(), 0.37, [0.5, 66.0, -2.5]);
        let order = k.r.entity_order.clone();
        (0..list.len())
            .map(|i| {
                let (e, d, sizes, info) = list[draw_order::row_to_draw(&order, i)];
                let sizes = sizes.map(|(b, f)| (size_bits(b), size_bits(f)));
                (std::ptr::from_ref(e) as usize, d.to_bits(), sizes, info)
            })
            .collect::<Vec<_>>()
    };
    let reference = walked(&mut k, EntOpt { entity_sort_key: false, size_after_distance: false, ..EntOpt::NEW });
    assert!(reference.len() > 2 * names.len(), "list almost empty: {}", reference.len());
    assert!(reference.len() < ents.len(), "no entity beyond its render distance");
    for (entity_sort_key, size_after_distance) in [(true, false), (false, true), (true, true)] {
        let opt = EntOpt { entity_sort_key, size_after_distance, ..EntOpt::NEW };
        assert!(walked(&mut k, opt) == reference, "ENTITY_SORT_KEY={entity_sort_key} SIZE_AFTER_DISTANCE={size_after_distance}");
    }
    // Every old sort as well (pairs of keys gathered, then the merge sort of the rows).
    let old = walked(&mut k, EntOpt { entity_sort_key: false, distance_sort_keys: false, size_after_distance: false, ..EntOpt::NEW });
    assert!(old == reference, "merge sort of the rows");
}

/// Per-entity size work of the frame's list and of the shadow, the old name matches against the
/// rules stored at spawn, both called past the switch in the same process, alternated; then each
/// function alone. 1 core, median of 101 rounds, ns per entity.
/// `cargo test --release -p rc-app -- --ignored bench_size_rules --nocapture`.
#[test]
#[ignore]
fn bench_size_rules() {
    let Some((ents, dims)) = every_type() else { return };
    // Every registry type in varied states, then the four farm animals of `crowd-1000` (adults,
    // no pose, scale 1).
    let mut draws = Draws(0xc0ffee);
    let varied_types: Vec<Entity> = (0..1000).map(|i| varied(&ents[(i * 37) % ents.len()], &mut draws)).collect();
    let farm: Vec<&Entity> = ["cow", "pig", "sheep", "chicken"]
        .iter()
        .map(|k| ents.iter().find(|e| e.kind == *k).expect("farm animal in the registry"))
        .collect();
    let farm_crowd: Vec<Entity> = (0..1000).map(|i| farm[i % 4].clone()).collect();
    for (label, crowd) in [("every type, varied", &varied_types), ("crowd-1000 animals", &farm_crowd)] {
        bench_size_work(label, crowd, &dims);
    }
}

/// `prepare` on the copied scene of `bench_entity_prepare` (1,000 and 5,000 entities) under this
/// mission's switches only, every other one on: each configuration in turn, 301 rounds, median and
/// minimum in µs (the minimum resists a loaded machine better). Run once more with
/// `RUSTCRAFT_OPT_KIND_SIZE_RULES=0` to compare the size rules at the level of the frame.
/// `cargo test --release -p rc-app -- --ignored bench_entity_list_switches --nocapture`.
#[test]
#[ignore]
fn bench_entity_list_switches() {
    let Some(mut k) = kit() else { return };
    let base = super::tests_c3m11::scene(&k);
    let all = EntOpt::NEW;
    let configs = [
        ("all", all),
        ("ENTITY_SORT_KEY=0", EntOpt { entity_sort_key: false, ..all }),
        ("PASS_KIND_FROM_LIST=0", EntOpt { pass_kind_from_list: false, ..all }),
        ("SIZE_AFTER_DISTANCE=0", EntOpt { size_after_distance: false, ..all }),
        ("FAR_TO_NEAR_SORT_KEY=0", EntOpt { far_to_near_sort_key: false, ..all }),
        (
            "the four =0",
            EntOpt {
                entity_sort_key: false,
                pass_kind_from_list: false,
                size_after_distance: false,
                far_to_near_sort_key: false,
                ..all
            },
        ),
    ];
    println!("size rules: {}", if kind_size_rules_on() { "on" } else { "off (RUSTCRAFT_OPT_KIND_SIZE_RULES=0)" });
    for n in [1000usize, 5000] {
        let ents = super::tests_c3m11::copied_scene(&base, n);
        let rounds = 301;
        let mut times = vec![Vec::with_capacity(rounds); configs.len()];
        for round in 0..rounds + 5 {
            for (i, (_, o)) in configs.iter().enumerate() {
                let t = std::time::Instant::now();
                super::tests_c3m11::prep(&mut k, &ents, *o);
                std::hint::black_box(k.r.build_batches());
                if round >= 5 {
                    times[i].push(t.elapsed().as_secs_f64() * 1e6);
                }
            }
        }
        for v in &mut times {
            v.sort_by(f64::total_cmp);
        }
        let (ref_median, ref_min) = (times[0][rounds / 2], times[0][0]);
        println!("{n} entities ({} drawn), prepare + batches:", k.r.drawn);
        for ((name, _), v) in configs.iter().zip(&times) {
            let (median, min) = (v[rounds / 2], v[0]);
            println!(
                "  {name:>24}: median {median:>8.1} µs ({:+.1}%), min {min:>8.1} µs ({:+.1}%)",
                (median / ref_median - 1.0) * 100.0,
                (min / ref_min - 1.0) * 100.0
            );
        }
    }
}

/// Work timed for each entity of a crowd.
type PerEntityWork<'a> = &'a dyn Fn(&Entity) -> f64;

/// One crowd of [`bench_size_rules`]: three alternated rounds, then each function alone.
fn bench_size_work(label: &str, crowd: &[Entity], dims: &[[f32; 4]]) {
    let per_entity = |f: &dyn Fn(&Entity) -> f64| {
        let mut times: Vec<f64> = (0..104)
            .map(|_| {
                let t = std::time::Instant::now();
                let mut acc = 0.0;
                for e in crowd {
                    acc += f(e);
                }
                std::hint::black_box(acc);
                t.elapsed().as_secs_f64() * 1e9 / crowd.len() as f64
            })
            .collect();
        times.drain(..3);
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    // Both paths called directly, past the switch (what each costs once chosen).
    let by_name = |e: &Entity| {
        let b = entity_by_name::base_size(e, dims);
        let full = if e.class != Kind::Other { b.scale(entity_by_name::attribute_scale(e)) } else { b };
        let within = sizes_by_name::within_render_distance_of(e, 400.0, 1.0, || full);
        f64::from(full.h) + f64::from(sizes_by_name::shadow_scale(e)) + sizes_by_name::crouch_offset(e) + f64::from(u8::from(within))
    };
    let by_rules = |e: &Entity| {
        let b = e.base_size_by_rules(dims);
        let full = if e.class != Kind::Other { b.scale(e.attribute_scale_by_rules()) } else { b };
        let within = within_render_distance_by_rules(e, 400.0, 1.0, || full);
        f64::from(full.h) + f64::from(shadow_scale_by_rules(e)) + crouch_offset_by_rules(e) + f64::from(u8::from(within))
    };
    for round in 0..3 {
        let (old, new) = (per_entity(&by_name), per_entity(&by_rules));
        println!(
            "{label}, round {round}: sizes, distance, shadow factor, crouch: by name {old:.2} ns, rules {new:.2} ns ({:+.1}%)",
            (new / old - 1.0) * 100.0
        );
    }
    // Each function alone, both paths.
    let full = Size::scalable(0.9, 1.4);
    let one_by_one: [(&str, PerEntityWork, PerEntityWork); 5] = [
        ("base_size", &|e| f64::from(entity_by_name::base_size(e, dims).h), &|e| f64::from(e.base_size_by_rules(dims).h)),
        ("attribute_scale", &|e| f64::from(entity_by_name::attribute_scale(e)), &|e| f64::from(e.attribute_scale_by_rules())),
        (
            "within_render_distance",
            &|e| f64::from(u8::from(sizes_by_name::within_render_distance_of(e, 400.0, 1.0, || full))),
            &|e| f64::from(u8::from(within_render_distance_by_rules(e, 400.0, 1.0, || full))),
        ),
        ("shadow_scale", &|e| f64::from(sizes_by_name::shadow_scale(e)), &|e| f64::from(shadow_scale_by_rules(e))),
        ("baby", &|e| f64::from(u8::from(entity_by_name::baby(e))), &|e| f64::from(u8::from(e.baby_by_rules()))),
    ];
    let parts: Vec<String> = one_by_one
        .iter()
        .map(|(name, old, new)| format!("{name} {:.2} -> {:.2}", per_entity(*old), per_entity(*new)))
        .collect();
    println!("{label}, each alone, by name -> rules, ns: {}", parts.join(", "));
}
