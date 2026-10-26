//! Equivalence proofs and micro-benches of the item switches (`Opt`): evaluation without
//! allocation, "environment read" flag, quads in columns and layers stored as pixels.
//!
//! Real data: the installed game 26.3 (`rc_data::locate`); absent, the tests on the jar
//! are skipped, except with `RUSTCRAFT_OPT_REQUIRE=1` where they fail.

use super::*;
use crate::cache::{decode, encode};

const ON: Opt = Opt { eval: true, env_flag: true, quad_col: true, quad_pack: true };
const OFF: Opt = Opt { eval: false, env_flag: false, quad_col: false, quad_pack: false };

fn with_opt<T>(o: Opt, f: impl FnOnce() -> T) -> T {
    let prev = OPT_TEST.with(|c| c.replace(Some(o)));
    let v = f();
    OPT_TEST.with(|c| c.set(prev));
    v
}

fn required() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_REQUIRE") == Some("1")
}

/// Resources of the installed game and identifiers of all its item definitions.
fn jar() -> Option<(Assets, Vec<String>)> {
    let found = rc_data::locate("26.3").ok().and_then(|i| Assets::for_install(&i.client_jar, &[], None).ok());
    let Some(a) = found else {
        assert!(!required(), "game 26.3 not found (RUSTCRAFT_OPT_REQUIRE=1)");
        eprintln!("game 26.3 not found: test skipped");
        return None;
    };
    let ids: Vec<String> = a
        .list("assets/minecraft/items/", ".json")
        .iter()
        .filter_map(|p| p.strip_prefix("assets/minecraft/items/")?.strip_suffix(".json"))
        .map(|s| format!("minecraft:{s}"))
        .collect();
    assert!(ids.len() > 1000, "item definitions of the jar: {}", ids.len());
    Some((a, ids))
}

// ---------------------------------------------------------------------------------------------
// Test stacks and environments

#[derive(Clone, Default)]
struct S {
    has: bool,
    damage: i32,
    max_damage: i32,
    count: i32,
    dyed: Option<u32>,
    potion: Option<u32>,
    fireworks: Vec<u32>,
    glint: bool,
    cmd_flag: Option<bool>,
    cmd_float: Option<f32>,
    cmd_str: Option<String>,
    cmd_color: Option<u32>,
    special: Vec<u32>,
    state: Option<String>,
    charge: &'static str,
    trim: Option<String>,
    lodestone: Option<(String, [i32; 3])>,
    selected: bool,
    using: Option<(i32, i32)>,
    unbreakable: bool,
}

impl StackView for S {
    // Damage: present as soon as a maximum is known (like the real stack); unbreakable apart.
    fn has(&self, c: &str) -> bool {
        match c {
            "minecraft:unbreakable" => self.unbreakable,
            "minecraft:max_damage" | "minecraft:damage" => self.has || self.max_damage > 0,
            _ => self.has,
        }
    }
    fn damage(&self) -> i32 {
        self.damage
    }
    fn max_damage(&self) -> i32 {
        self.max_damage
    }
    fn count(&self) -> i32 {
        self.count
    }
    fn dyed_color(&self) -> Option<u32> {
        self.dyed
    }
    fn potion_color(&self) -> Option<u32> {
        self.potion
    }
    fn firework_colors(&self, f: &mut dyn FnMut(u32)) {
        self.fireworks.iter().for_each(|&c| f(c));
    }
    fn custom_model_data_flag(&self, _i: usize) -> Option<bool> {
        self.cmd_flag
    }
    fn custom_model_data_float(&self, _i: usize) -> Option<f32> {
        self.cmd_float
    }
    fn custom_model_data_string(&self, _i: usize) -> Option<String> {
        self.cmd_str.clone()
    }
    fn custom_model_data_color(&self, _i: usize) -> Option<u32> {
        self.cmd_color
    }
    fn special_data(&self, _kind: &str) -> Vec<u32> {
        self.special.clone()
    }
    fn block_state(&self, _p: &str) -> Option<String> {
        self.state.clone()
    }
    fn charge_type(&self) -> &'static str {
        if self.charge.is_empty() { "none" } else { self.charge }
    }
    fn trim_material(&self) -> Option<String> {
        self.trim.clone()
    }
    fn lodestone(&self) -> Option<(String, [i32; 3])> {
        self.lodestone.clone()
    }
    fn glint(&self) -> bool {
        self.glint
    }
    fn selected(&self) -> bool {
        self.selected
    }
    fn using(&self) -> Option<(i32, i32)> {
        self.using
    }
}

/// Test stacks: new, damaged and tinted, enchanted and loaded, model data, broken and in use.
fn stacks() -> Vec<S> {
    vec![
        S { count: 1, ..S::default() },
        S {
            has: true,
            damage: 30,
            max_damage: 250,
            count: 17,
            dyed: Some(0x00a0_6030),
            potion: Some(0x0020_80f0),
            fireworks: vec![0x00ff_0000],
            trim: Some("minecraft:iron".into()),
            state: Some("3".into()),
            special: vec![1, 2],
            ..S::default()
        },
        S {
            has: true,
            glint: true,
            count: 64,
            fireworks: vec![0x0011_2233, 0x0044_5566, 0x0077_8899],
            charge: "rocket",
            trim: Some("quartz".into()),
            cmd_flag: Some(true),
            cmd_float: Some(0.7),
            cmd_str: Some("a".into()),
            cmd_color: Some(0x0012_3456),
            lodestone: Some(("minecraft:overworld".into(), [10, 64, -20])),
            special: vec![3, 1, 4, 1, 5, 9],
            selected: true,
            ..S::default()
        },
        S {
            damage: 249,
            max_damage: 250,
            count: 2,
            charge: "arrow",
            using: Some((12, 60)),
            state: Some("minecraft:true".into()),
            cmd_str: Some("minecraft:b".into()),
            ..S::default()
        },
    ]
}

#[derive(Clone)]
struct E {
    holder: bool,
    dim: &'static str,
    tick: i64,
    sun: f32,
    moon: f32,
    rnd: f32,
    shift: bool,
    right: bool,
    ctx: &'static str,
    pos: [f64; 3],
    yaw: f32,
    spawn: Option<(String, [i32; 3])>,
    local: bool,
}

impl EvalEnv for E {
    fn has_holder(&self) -> bool {
        self.holder
    }
    fn dimension(&self) -> Option<String> {
        Some(self.dim.to_owned())
    }
    fn tick(&self) -> i64 {
        self.tick
    }
    fn sun_angle(&self) -> f32 {
        self.sun
    }
    fn moon_phase(&self) -> f32 {
        self.moon
    }
    fn random(&self) -> f32 {
        self.rnd
    }
    // 2026-12-24 12:00 UTC, frozen (no dependency on the clock).
    fn now_ms(&self) -> Option<i64> {
        Some(1_798_113_600_000)
    }
    fn shift_down(&self) -> bool {
        self.shift
    }
    fn right_handed(&self) -> bool {
        self.right
    }
    fn display_context(&self) -> &str {
        self.ctx
    }
    fn holder_pos(&self) -> [f64; 3] {
        self.pos
    }
    fn body_yaw(&self) -> f32 {
        self.yaw
    }
    fn spawn(&self) -> Option<(String, [i32; 3])> {
        self.spawn.clone()
    }
    fn local_ticking(&self) -> bool {
        self.local
    }
}

fn base_env() -> E {
    E {
        holder: true,
        dim: "minecraft:overworld",
        tick: 100,
        sun: 45.0,
        moon: 3.0,
        rnd: 0.25,
        shift: false,
        right: true,
        ctx: "gui",
        pos: [0.5, 64.0, 0.5],
        yaw: 30.0,
        spawn: Some(("minecraft:overworld".into(), [100, 70, 100])),
        local: true,
    }
}

/// Contexts: GUI slot, on the ground in the Nether, head without holder, left hand in 1st person with Shift.
fn envs() -> Vec<E> {
    let b = base_env();
    vec![
        b.clone(),
        E { dim: "minecraft:the_nether", ctx: "ground", tick: 101, rnd: 0.6, local: false, ..b.clone() },
        E { holder: false, ctx: "minecraft:head", ..b.clone() },
        E { right: false, shift: true, ctx: "firstperson_lefthand", yaw: -170.0, ..b },
    ]
}

/// Old and new evaluation path on each definition × stack × context: same layers and
/// same key. Returns the number of evaluations and layers compared.
/// FNV-1a fingerprint of the `Debug` form of values, without an intermediate string: expected value
/// stored in the tests, independent of the old path (which may disappear one day).
struct Fp(u64);

impl Fp {
    fn new() -> Fp {
        Fp(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, v: &impl std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self, "{v:?}|");
    }
}

impl std::fmt::Write for Fp {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0 = s.bytes().fold(self.0, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        Ok(())
    }
}

/// Old and new paths identical; returns (evaluations, layers, fingerprint of layers and keys).
fn compare_eval(m: &ItemModels, ids: &[String]) -> (usize, usize, u64) {
    let (mut evals, mut layers) = (0, 0);
    let mut fp = Fp::new();
    let mut seen = std::collections::HashMap::new();
    let (stacks, envs) = (stacks(), envs());
    for id in ids {
        let short = id.strip_prefix("minecraft:").unwrap_or(id);
        for (si, s) in stacks.iter().enumerate() {
            for e in &envs {
                for model_id in [id.as_str(), short] {
                    let (mut l0, mut k0, mut l1, mut k1) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                    m.layers_with(false, id, model_id, s, e, si as i32, &mut l0, &mut k0);
                    m.layers_with(true, id, model_id, s, e, si as i32, &mut l1, &mut k1);
                    assert_eq!(l0, l1, "{model_id} stack {si} context {}", e.ctx);
                    assert_eq!(k0, k1, "{model_id} stack {si} context {}: key", e.ctx);
                    // The keys carry node addresses: numbered in order of first
                    // encounter (deterministic) for the fingerprint.
                    let k: Vec<u64> = k1
                        .iter()
                        .map(|&v| {
                            if v <= u64::from(u32::MAX) {
                                v
                            } else {
                                let n = seen.len() as u64;
                                (1 << 40) + *seen.entry(v).or_insert(n)
                            }
                        })
                        .collect();
                    fp.add(&(&l1, &k));
                    evals += 1;
                    layers += l1.len();
                }
            }
        }
    }
    (evals, layers, fp.0)
}

// ---------------------------------------------------------------------------------------------
// Evaluation without allocation (`RUSTCRAFT_OPT_ITEMEVAL`)

#[test]
fn norm_without_string_identical() {
    let long = "x".repeat(200);
    let vals = [
        "", ":", "a", "gui", "minecraft:gui", "minecraft:", ":gui", "iron", "minecraft:iron", "other:iron", "a:b:c",
        "minecraft:a:b", "MINECRAFT:gui", "é:ü", "minecraft:é", "1", "true", "minecraft:true", long.as_str(),
    ];
    for a in vals {
        assert_eq!(with_norm(a, str::to_owned), norm(a), "with_norm({a})");
        for b in vals {
            assert_eq!(norm_eq_fast(a, b), norm_eq(a, b), "norm_eq({a}, {b})");
        }
    }
}

#[test]
fn inline_tints() {
    let s = &stacks()[2];
    for n in 0..7 {
        let tints: Vec<Tint> = (0..n)
            .map(|i| match i % 4 {
                0 => Tint::Dye(0xff00_0000 | i),
                1 => Tint::Firework(0x1234),
                2 => Tint::CustomModelData { index: 0, default: 7 },
                _ => Tint::Constant(i),
            })
            .collect();
        let node = Node::Model { model: 3, tints, local: IDENTITY };
        let m = ItemModels::load(&Assets::new(vec![]), []);
        let (mut l0, mut k0, mut l1, mut k1) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        m.eval_old(&node, s, &base_env(), 0, 1, &mut l0, &mut k0);
        m.eval_new(&node, s, &base_env(), 0, 1, &mut l1, &mut k1);
        assert_eq!((l0, k0), (l1, k1), "{n} tints");
    }
}

/// Definitions written to cover every node and every property (values with and without a
/// namespace, non-textual `when`, more than four tints).
fn synthetic_defs() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "tints5",
            r#"{"type":"model","model":"item/a","tints":[{"type":"dye","default":-1},{"type":"constant","value":[0.5,0.2,1]},{"type":"potion","default":255},{"type":"firework","default":3},{"type":"custom_model_data","default":9}]}"#,
        ),
        (
            "tints3",
            r#"{"type":"minecraft:model","model":"a","tints":[{"type":"team","default":1},{"type":"grass","temperature":0.5,"downfall":1},{"type":"unknown"}]}"#,
        ),
        (
            "sel_trim",
            r#"{"type":"select","property":"trim_material","cases":[{"when":"iron","model":{"type":"model","model":"t1"}},{"when":["minecraft:quartz","x"],"model":{"type":"model","model":"t2"}}],"fallback":{"type":"model","model":"t0"}}"#,
        ),
        (
            "sel_ctx",
            r#"{"type":"select","property":"display_context","cases":[{"when":"gui","model":{"type":"model","model":"c1"}},{"when":"minecraft:ground","model":{"type":"model","model":"c2"}},{"when":"head","model":{"type":"model","model":"c3"}}]}"#,
        ),
        (
            "sel_dim",
            r#"{"type":"select","property":"context_dimension","cases":[{"when":"the_nether","model":{"type":"model","model":"d1"}},{"when":"minecraft:overworld","model":{"type":"empty"}}],"fallback":{"type":"model","model":"d0"}}"#,
        ),
        (
            "sel_hand",
            r#"{"type":"select","property":"main_hand","cases":[{"when":"left","model":{"type":"model","model":"h1"}},{"when":"minecraft:right","model":{"type":"model","model":"h2"}}],"fallback":{"type":"model","model":"h0"}}"#,
        ),
        (
            "sel_charge",
            r#"{"type":"select","property":"charge_type","cases":[{"when":"rocket","model":{"type":"model","model":"r1"}},{"when":"minecraft:arrow","model":{"type":"model","model":"r2"}}],"fallback":{"type":"model","model":"r0"}}"#,
        ),
        (
            "sel_time",
            r#"{"type":"select","property":"local_time","pattern":"MM-dd","cases":[{"when":"12-24","model":{"type":"model","model":"x1"}}],"fallback":{"type":"model","model":"x0"}}"#,
        ),
        (
            "sel_cmd",
            r#"{"type":"select","property":"custom_model_data","cases":[{"when":[1,"a"],"model":{"type":"model","model":"m1"}},{"when":"b","model":{"type":"model","model":"m2"}},{"when":true,"model":{"type":"model","model":"m3"}}],"fallback":{"type":"model","model":"m0"}}"#,
        ),
        (
            "sel_state",
            r#"{"type":"select","property":"block_state","block_state_property":"level","cases":[{"when":"3","model":{"type":"model","model":"s1"}},{"when":"true","model":{"type":"model","model":"s2"}}],"fallback":{"type":"model","model":"s0"}}"#,
        ),
        (
            "sel_entity",
            r#"{"type":"select","property":"context_entity_type","cases":[{"when":"player","model":{"type":"model","model":"e1"}}],"fallback":{"type":"model","model":"e0"}}"#,
        ),
        (
            "sel_component",
            r#"{"type":"select","property":"component","component":"x","cases":[],"fallback":{"type":"model","model":"o0"}}"#,
        ),
        (
            "conds",
            r#"{"type":"composite","models":[
              {"type":"condition","property":"using_item","on_true":{"type":"model","model":"u1"},"on_false":{"type":"model","model":"u0"}},
              {"type":"condition","property":"broken","on_true":{"type":"model","model":"b1"},"on_false":{"type":"model","model":"b0"}},
              {"type":"condition","property":"damaged","on_true":{"type":"model","model":"g1"}},
              {"type":"condition","property":"has_component","component":"dyed_color","on_true":{"type":"model","model":"k1"},"on_false":{"type":"empty"}},
              {"type":"condition","property":"selected","on_true":{"type":"model","model":"q1"},"on_false":{"type":"empty"}},
              {"type":"condition","property":"extended_view","on_true":{"type":"model","model":"v1"},"on_false":{"type":"empty"}},
              {"type":"condition","property":"view_entity","on_true":{"type":"model","model":"w1"},"on_false":{"type":"empty"}},
              {"type":"condition","property":"custom_model_data","on_true":{"type":"model","model":"y1"},"on_false":{"type":"empty"}}
            ]}"#,
        ),
        (
            "ranges",
            r#"{"type":"composite","models":[
              {"type":"range_dispatch","property":"damage","entries":[{"threshold":0.1,"model":{"type":"model","model":"n1"}},{"threshold":0.9,"model":{"type":"model","model":"n2"}}],"fallback":{"type":"model","model":"n0"}},
              {"type":"range_dispatch","property":"count","normalize":false,"entries":[{"threshold":16,"model":{"type":"model","model":"c16"}}]},
              {"type":"range_dispatch","property":"time","source":"daytime","entries":[{"threshold":0.1,"model":{"type":"model","model":"tm1"}}]},
              {"type":"range_dispatch","property":"time","source":"random","wobble":false,"entries":[{"threshold":0.5,"model":{"type":"model","model":"tr1"}}]},
              {"type":"range_dispatch","property":"time","source":"moon_phase","entries":[{"threshold":0.3,"model":{"type":"model","model":"tp1"}}]},
              {"type":"range_dispatch","property":"compass","target":"spawn","entries":[{"threshold":0.5,"model":{"type":"model","model":"cs1"}}]},
              {"type":"range_dispatch","property":"compass","target":"lodestone","entries":[{"threshold":0.5,"model":{"type":"model","model":"cl1"}}]},
              {"type":"range_dispatch","property":"compass","target":"none","entries":[{"threshold":0.5,"model":{"type":"model","model":"cn1"}}]},
              {"type":"range_dispatch","property":"use_duration","scale":0.05,"entries":[{"threshold":0.5,"model":{"type":"model","model":"ud1"}}]},
              {"type":"range_dispatch","property":"use_cycle","period":10,"entries":[{"threshold":1,"model":{"type":"model","model":"uc1"}}]},
              {"type":"range_dispatch","property":"crossbow/pull","entries":[{"threshold":0.5,"model":{"type":"model","model":"cp1"}}]},
              {"type":"range_dispatch","property":"custom_model_data","entries":[{"threshold":0.5,"model":{"type":"model","model":"cf1"}}]},
              {"type":"range_dispatch","property":"cooldown","entries":[]}
            ]}"#,
        ),
        (
            "special",
            r#"{"type":"composite","models":[{"type":"special","base":"item/b","model":{"type":"banner","color":"red"}},{"type":"special","base":"item/c","model":{"type":"minecraft:unknown"}},{"type":"bundle/selected_item"},{"type":"unknown"}]}"#,
        ),
    ]
}

/// Resource folder of the test definitions, in the repository's `target` (erased after use).
struct Defs(std::path::PathBuf);

impl Defs {
    fn new(tag: &str) -> Defs {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/c4m12-trials")
            .join(format!("{tag}-{}", std::process::id()));
        let dir = root.join("assets/minecraft/items");
        std::fs::create_dir_all(&dir).expect("trial directory");
        for (name, json) in synthetic_defs() {
            std::fs::write(dir.join(format!("{name}.json")), format!(r#"{{"model":{json}}}"#)).expect("write");
        }
        Defs(root)
    }
}

impl Drop for Defs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn evaluation_of_test_definitions() {
    let d = Defs::new("eval");
    let a = Assets::new(vec![crate::source::Source::dir(&d.0)]);
    let ids: Vec<String> = synthetic_defs().iter().map(|(n, _)| format!("minecraft:{n}")).collect();
    let m = ItemModels::load(&a, ids.iter().map(String::as_str));
    assert_eq!(m.len(), ids.len());
    let (evals, layers, fp) = compare_eval(&m, &ids);
    eprintln!("test definitions: {evals} evaluations, {layers} identical layers, fingerprint {fp:016x}");
    assert!(layers > evals, "too few layers compared: {layers}");
    assert_eq!(fp, FP_EVAL_TRIAL, "layers of the test definitions changed: {fp:016x}");
}

/// Expected fingerprints (layers and keys, quads), taken on the code delivered with C4-M12; quads
/// re-recorded at the final integration of series 2: fidelity puts the missing texture and model
/// where the game puts them (baking changed, same quads in the three shapes). Layers
/// re-recorded with G2-F04: broken / damaged only apply to a usable non-unbreakable stack,
/// chest texture drawn from the definition, viewed entity, local time computed here.
const FP_EVAL_TRIAL: u64 = 0xfcce_3ed5_6ca0_aae9;
const FP_EVAL_JAR: u64 = 0x6cdc_e8df_a319_8f31;
const FP_QUADS_JAR: u64 = 0x7f49_a953_fc43_c9e9;
const FP_EXTRUSION: u64 = 0xe2fe_268d_57f7_1f09;

#[test]
fn evaluation_of_all_the_game_definitions() {
    let Some((a, ids)) = jar() else { return };
    let m = with_opt(ON, || ItemModels::load(&a, ids.iter().map(String::as_str)));
    let (evals, layers, fp) = compare_eval(&m, &ids);
    let mut fp = Fp(fp);
    // Hand swap: all the items, full and short identifiers, other namespaces.
    for id in &ids {
        let short = id.strip_prefix("minecraft:").unwrap_or(id);
        for k in [id.as_str(), short, "other:apple", ""] {
            let new = with_opt(ON, || m.swap_animation(k));
            assert_eq!(with_opt(OFF, || m.swap_animation(k)), new, "swap {k}");
            fp.add(&new);
        }
    }
    let fp = fp.0;
    eprintln!("jar: {} definitions, {evals} evaluations, {layers} identical layers, fingerprint {fp:016x}", ids.len());
    assert_eq!(fp, FP_EVAL_JAR, "layers of jar 26.3 changed: {fp:016x}");
    assert!(!m.swap.is_empty(), "no particular swap animation in the jar");
}

/// Micro-bench (1 thread): allocations and duration of one pass over all the game's definitions, GUI
/// slot and first-person hand, new and loaded stacks. Run alone (`--test-threads=1`) for an
/// exact allocation count.
#[test]
fn evaluation_bench() {
    let Some((a, ids)) = jar() else { return };
    let m = with_opt(ON, || ItemModels::load(&a, ids.iter().map(String::as_str)));
    let (stacks, envs) = (stacks(), envs());
    let (mut out, mut key) = (Vec::with_capacity(64), Vec::with_capacity(256));
    let mut line = String::from("evaluation bench:");
    for fast in [false, true, false, true] {
        let mut best = f64::MAX;
        let mut allocs = 0;
        let mut n = 0;
        for _ in 0..5 {
            let a0 = crate::cache::tests::ALLOCS.load(std::sync::atomic::Ordering::Relaxed);
            let t = std::time::Instant::now();
            n = 0;
            for id in &ids {
                for s in [&stacks[0], &stacks[2]] {
                    for e in [&envs[0], &envs[3]] {
                        out.clear();
                        key.clear();
                        m.layers_with(fast, id, id, s, e, 0, &mut out, &mut key);
                        n += 1;
                    }
                }
            }
            best = best.min(t.elapsed().as_secs_f64());
            allocs = crate::cache::tests::ALLOCS.load(std::sync::atomic::Ordering::Relaxed) - a0;
        }
        line += &format!(
            " {} {:.1} ns/eval {:.3} alloc/eval;",
            if fast { "new" } else { "old" },
            best * 1e9 / n as f64,
            allocs as f64 / n as f64
        );
    }
    eprintln!("{line} ({} evaluations per pass)", ids.len() * 4);
}

/// Micro-bench (1 thread): full `layers()`, wrappers of the "environment read" flag included
/// (`ITEMENVFLAG` at 1) or not (at 0), fast evaluation in both cases. Alternating passes.
#[test]
fn environment_flag_bench() {
    let Some((a, ids)) = jar() else { return };
    let m = with_opt(ON, || ItemModels::load(&a, ids.iter().map(String::as_str)));
    let (stacks, envs) = (stacks(), envs());
    let (mut out, mut key) = (Vec::with_capacity(64), Vec::with_capacity(256));
    let mut line = String::from("layers() bench:");
    for flag in [false, true, false, true, false, true] {
        let mut best = f64::MAX;
        let mut n = 0;
        with_opt(Opt { env_flag: flag, ..ON }, || {
            for _ in 0..7 {
                let t = std::time::Instant::now();
                n = 0;
                for id in &ids {
                    for s in [&stacks[0], &stacks[2]] {
                        for e in [&envs[0], &envs[3]] {
                            out.clear();
                            key.clear();
                            std::hint::black_box(m.layers(id, id, s, e, 0, &mut out, &mut key));
                            n += 1;
                        }
                    }
                }
                best = best.min(t.elapsed().as_secs_f64());
            }
        });
        line += &format!(" flag {} {:.1} ns;", u8::from(flag), best * 1e9 / n as f64);
    }
    eprintln!("{line} ({} calls per pass)", ids.len() * 4);
}

/// The key of the baked sections follows the switches that decide the written shape of the items
/// (columns, pixels), and only them: relaunching with one of them at 0 opens another file
/// instead of re-reading the shape of the previous session.
#[test]
fn cache_key_follows_the_item_shape() {
    let key = |o: Opt| with_opt(o, || crate::cache::sections_key_text("26.3", &[]));
    let base = key(ON);
    assert_ne!(base, key(Opt { quad_col: false, ..ON }), "columns absent from the key");
    assert_ne!(base, key(Opt { quad_pack: false, ..ON }), "pixels absent from the key");
    assert_ne!(key(Opt { quad_pack: false, ..ON }), key(Opt { quad_col: false, ..ON }));
    assert_ne!(base, key(OFF));
    // Evaluation and flag do not change what is written: same file.
    assert_eq!(base, key(Opt { eval: false, env_flag: false, ..ON }));
    assert!(base.starts_with(&crate::cache::key_text("26.3", &[])));
}

// ---------------------------------------------------------------------------------------------
// "Environment read" flag (`RUSTCRAFT_OPT_ITEMENVFLAG`) and generation

#[test]
#[allow(clippy::type_complexity)]
fn every_environment_read_raises_the_flag() {
    let e = base_env();
    let probes: [(&str, &dyn Fn(&WatchEnv)); 21] = [
        ("has_holder", &|w| _ = w.has_holder()),
        ("living_holder", &|w| _ = w.living_holder()),
        ("holder_type", &|w| _ = w.holder_type()),
        ("dimension", &|w| _ = w.dimension()),
        ("tick", &|w| _ = w.tick()),
        ("sun_angle", &|w| _ = w.sun_angle()),
        ("moon_phase", &|w| _ = w.moon_phase()),
        ("random", &|w| _ = w.random()),
        ("now_ms", &|w| _ = w.now_ms()),
        ("uptime_ms", &|w| _ = w.uptime_ms()),
        ("utc_offset_secs", &|w| _ = w.utc_offset_secs(0)),
        ("last_death", &|w| _ = w.last_death()),
        ("key_down", &|w| _ = w.key_down("key.use")),
        ("view_entity", &|w| _ = w.view_entity()),
        ("shift_down", &|w| _ = w.shift_down()),
        ("right_handed", &|w| _ = w.right_handed()),
        ("display_context", &|w| _ = w.display_context()),
        ("holder_pos", &|w| _ = w.holder_pos()),
        ("body_yaw", &|w| _ = w.body_yaw()),
        ("spawn", &|w| _ = w.spawn()),
        ("local_ticking", &|w| _ = w.local_ticking()),
    ];
    for (name, f) in probes {
        let read = Cell::new(false);
        f(&WatchEnv { e: &e, read: &read });
        assert!(read.get(), "{name} does not raise the flag");
    }
    // Stack: only the holder's state raises the flag.
    let s = stacks()[2].clone();
    for (name, f, expect) in [
        ("using", &(|w: &WatchStack| _ = w.using()) as &dyn Fn(&WatchStack), true),
        ("selected", &|w: &WatchStack| _ = w.selected(), true),
        ("fishing_cast", &|w: &WatchStack| _ = w.fishing_cast(), true),
        ("carried", &|w: &WatchStack| _ = w.carried(), true),
        ("bundle_selected", &|w: &WatchStack| _ = w.bundle_selected(&mut |_, _, _| {}), true),
        ("has", &|w: &WatchStack| _ = w.has("x"), false),
        ("damage", &|w: &WatchStack| _ = w.damage(), false),
        ("glint", &|w: &WatchStack| _ = w.glint(), false),
    ] {
        let read = Cell::new(false);
        f(&WatchStack { s: &s, read: &read });
        assert_eq!(read.get(), expect, "stack: {name}");
    }
}

/// Three instants: time, sun, chance, use and selected slot change; the stack does not.
fn instants() -> [(E, S); 3] {
    let b = base_env();
    let s = S { count: 1, max_damage: 384, ..S::default() };
    [
        (b.clone(), s.clone()),
        (E { tick: 157, sun: 170.0, moon: 6.0, rnd: 0.83, pos: [40.0, 70.0, -9.0], yaw: 200.0, ..b.clone() }, S {
            using: Some((5, 67)),
            selected: true,
            ..s.clone()
        }),
        (E { tick: 9001, sun: 300.0, moon: 1.0, rnd: 0.07, pos: [-300.0, 12.0, 77.0], yaw: -45.0, ..b }, S {
            using: Some((30, 42)),
            ..s
        }),
    ]
}

#[test]
fn false_flag_means_stable_layers_over_three_instants() {
    let Some((a, ids)) = jar() else { return };
    let m = with_opt(ON, || ItemModels::load(&a, ids.iter().map(String::as_str)));
    let (mut stable, mut moving) = (0, Vec::new());
    for id in &ids {
        let runs: Vec<(bool, Vec<Layer>, Vec<u64>)> = with_opt(ON, || {
            instants()
                .iter()
                .map(|(e, s)| {
                    let (mut l, mut k) = (Vec::new(), Vec::new());
                    let f = m.layers(id, id, s, e, 0, &mut l, &mut k);
                    (f, l, k)
                })
                .collect()
        });
        if runs[0].0 {
            moving.push(id.as_str());
            continue;
        }
        stable += 1;
        for r in &runs[1..] {
            assert!(!r.0, "{id}: flag that changes with the instant");
            assert_eq!((&r.1, &r.2), (&runs[0].1, &runs[0].2), "{id}: false flag but different layers");
        }
    }
    eprintln!("flag: {stable} stable definitions, {} dependent ({:?}…)", moving.len(), &moving[..moving.len().min(8)]);
    for id in ["minecraft:compass", "minecraft:recovery_compass", "minecraft:clock", "minecraft:bow", "minecraft:crossbow"] {
        assert!(moving.contains(&id), "{id} should depend on the environment");
    }
    assert!(stable > 1000, "too few stable definitions: {stable}");
}

#[test]
fn nether_compass_marked_dependent() {
    let d = Defs::new("compass");
    let a = Assets::new(vec![crate::source::Source::dir(&d.0)]);
    let m = ItemModels::load(&a, ["minecraft:ranges"]);
    let e = E { dim: "minecraft:the_nether", ..base_env() };
    let s = S::default();
    let (mut l, mut k) = (Vec::new(), Vec::new());
    assert!(with_opt(ON, || m.layers("minecraft:compass", "minecraft:ranges", &s, &e, 0, &mut l, &mut k)));
    // Without the switch: always dependent (no memoisation possible).
    let plain = Defs::new("plain");
    std::fs::write(plain.0.join("assets/minecraft/items/stone.json"), r#"{"model":{"type":"model","model":"item/stone"}}"#)
        .expect("write");
    let a = Assets::new(vec![crate::source::Source::dir(&plain.0)]);
    let m = ItemModels::load(&a, ["minecraft:stone"]);
    assert!(!with_opt(ON, || m.layers("minecraft:stone", "minecraft:stone", &s, &e, 0, &mut l, &mut k)));
    assert!(with_opt(Opt { env_flag: false, ..ON }, || m.layers("minecraft:stone", "stone", &s, &e, 0, &mut l, &mut k)));
}

#[test]
fn fresh_generation_at_each_load() {
    let a = Assets::new(vec![]);
    let (g1, g2) = (ItemModels::load(&a, []).generation(), ItemModels::load(&a, []).generation());
    assert_ne!(g1, g2);
    let b = encode(&ItemModels::load(&a, []));
    let g3 = decode::<ItemModels>(&b).expect("re-read").generation();
    let g4 = decode::<ItemModels>(&b).expect("re-read").generation();
    assert!(g3 > g2 && g4 > g3, "re-reads: generations {g3}, {g4} after {g2}");
}

// ---------------------------------------------------------------------------------------------
// Quads: pixel layers (`RUSTCRAFT_OPT_ITEMQUADPACK`) and columns (`RUSTCRAFT_OPT_ITEMQUADCOL`)

/// Pseudo-random alpha image (density `fill` out of 256).
fn noise_image(w: u32, h: u32, seed: u64, fill: u64) -> Image {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let rgba = (0..w * h)
        .flat_map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let a = if x % 256 < fill { (x >> 8) as u8 | 1 } else { 0 };
            [1, 2, 3, a]
        })
        .collect();
    Image { width: w, height: h, rgba }
}

#[test]
fn unfolded_pixels_identical_to_extrusion() {
    let mut cases = 0;
    let mut fp = Fp::new();
    for (i, &(w, h)) in [(1, 1), (2, 2), (16, 16), (7, 5), (32, 16), (64, 64)].iter().enumerate() {
        for fill in [0, 40, 128, 230, 256] {
            let img = noise_image(w, h, i as u64 * 31 + fill, fill);
            // Animations: frames of the same size, smaller and larger than the first.
            let frame_sets = [
                vec![],
                vec![noise_image(w, h, 7, fill), noise_image(w, h, 8, 200)],
                vec![noise_image(w.div_ceil(2), h, 9, fill), noise_image(w + 3, h + 1, 10, 100)],
            ];
            for frames in &frame_sets {
                for force in [false, true] {
                    let mut old = Vec::new();
                    extrude(&img, frames, 5, 2, force, &mut old);
                    let p = pack_layer(&img, frames, 5, 2, force);
                    let mut new = Vec::new();
                    unfold(&p, &mut new);
                    assert_eq!(old, new, "{w}×{h} density {fill} frames {} forced {force}", frames.len());
                    fp.add(&new);
                    cases += 1;
                }
            }
        }
    }
    eprintln!("{cases} identical extruded layers, fingerprint {:016x}", fp.0);
    assert_eq!(fp.0, FP_EXTRUSION, "extruded quads changed: {:016x}", fp.0);
}

/// Same quads, same flags and same transformations, model by model.
fn same_baked(a: &ItemModels, b: &ItemModels) {
    assert_eq!(a.baked.len(), b.baked.len());
    for (i, (x, y)) in a.baked.iter().chain([&a.missing]).zip(b.baked.iter().chain([&b.missing])).enumerate() {
        assert_eq!(x.quads(), y.quads(), "model {i}: quads");
        assert_eq!(
            (x.gui_front, x.display, x.displays, x.animated),
            (y.gui_front, y.display, y.displays, y.animated),
            "model {i}"
        );
    }
}

#[test]
fn game_quads_identical_in_each_shape() {
    let Some((a, ids)) = jar() else { return };
    let load = |o: Opt| with_opt(o, || ItemModels::load(&a, ids.iter().map(String::as_str)));
    let (old, packed, cols) = (load(OFF), load(ON), load(Opt { quad_pack: false, ..ON }));
    // Memory before any quad request: pixel layers against unfolded quads.
    let heap = |m: &ItemModels| m.baked.iter().map(BakedItem::heap_bytes).sum::<usize>();
    let (h_old, h_packed) = (heap(&old), heap(&packed));
    let n_quads: usize = old.baked.iter().map(|b| b.quads().len()).sum();
    let n_packed = packed.baked.iter().filter(|b| !b.packed.is_empty()).count();
    eprintln!(
        "quads: {n_quads} × {} B; heap {:.2} MB -> {:.2} MB as pixels ({n_packed}/{} models stored)",
        size_of::<ItemQuad>(),
        h_old as f64 / 1_048_576.0,
        h_packed as f64 / 1_048_576.0,
        packed.baked.len()
    );
    assert!(n_packed > 500 && h_packed < h_old / 4, "pixel storage ineffective");
    same_baked(&old, &packed);
    same_baked(&old, &cols);
    let mut fp = Fp::new();
    for b in packed.baked.iter().chain([&packed.missing]) {
        fp.add(&(b.quads(), b.gui_front, b.display, b.displays, b.animated));
    }
    eprintln!("jar quads: fingerprint {:016x}", fp.0);
    assert_eq!(fp.0, FP_QUADS_JAR, "quads of jar 26.3 changed: {:016x}", fp.0);
    // Cache: each shape read back (with and without unfolding on re-read) gives the same quads and
    // is rewritten identically.
    let mut line = String::from("items decoding (1 thread, best of 5):");
    for (name, m) in [("rows", &old), ("columns", &cols), ("pixels", &packed)] {
        let b = encode(m);
        for o in [OFF, ON] {
            let back = with_opt(o, || decode::<ItemModels>(&b)).expect("re-read");
            same_baked(&old, &back);
            assert!(encode(&back) == b, "{name}: different rewrite");
        }
        let eager = with_opt(OFF, || decode::<ItemModels>(&b)).expect("re-read");
        assert!(eager.baked.iter().all(|x| x.quads.get().is_some()), "{name}: quads not unfolded on re-read");
        let mut best = f64::MAX;
        let mut allocs = 0;
        for _ in 0..5 {
            let a0 = crate::cache::tests::ALLOCS.load(std::sync::atomic::Ordering::Relaxed);
            let t = std::time::Instant::now();
            let v = with_opt(ON, || decode::<ItemModels>(&b)).expect("re-read");
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
            allocs = crate::cache::tests::ALLOCS.load(std::sync::atomic::Ordering::Relaxed) - a0;
            drop(v);
        }
        line += &format!(" {name} {:.1} MB {best:.2} ms {allocs} alloc.;", b.len() as f64 / 1_048_576.0);
        // Truncated: None, no panic.
        assert!(decode::<ItemModels>(&b[..b.len() / 2]).is_none());
    }
    eprintln!("{line}");
    // Unfolding of all the stored models of a fresh re-read (cost of the first requests, 1 thread).
    let fresh = with_opt(ON, || decode::<ItemModels>(&encode(&packed))).expect("re-read");
    assert!(fresh.baked.iter().any(|b| b.quads.get().is_none()), "re-read already unfolded");
    let t = std::time::Instant::now();
    let unfolded: usize = fresh.baked.iter().map(|b| b.quads().len()).sum();
    eprintln!("unfolding of all models: {unfolded} quads in {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
}
