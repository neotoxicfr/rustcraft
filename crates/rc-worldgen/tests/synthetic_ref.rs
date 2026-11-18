//! Synthetic density functions (types little or not used by the vanilla data, unaligned
//! interpolation paths, caches), decoded and evaluated by the game, compared at 0 ulp.

mod common;

use std::sync::Arc;

use rc_worldgen::generator::{Defs, NoiseSettings};
use rc_worldgen::{Ctx, Generator, Volume, density, json};

const POINTS: [[i32; 3]; 12] = [
    [0, 0, 0],
    [1, 2, 3],
    [-17, 64, -33],
    [123, -50, -456],
    [1000, 100, -1000],
    [-2_097_153, 70, 2_097_155],
    [30_000_000, -64, -29_999_999],
    [7, 319, -9],
    [-1, -1, -1],
    [4, 8, 4],
    [-5, 12, 9],
    [16, 40, -16],
];

#[test]
fn synthetic_functions_match_game() {
    let Some(pack) = common::data_pack() else { return };
    let Some(dir) = common::ref_dir() else { return };
    let src = std::fs::read_to_string(dir.join("synthetic.jsonl")).unwrap();
    let funcs: Vec<_> = src
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| density::parse(&json::parse(l).unwrap()).unwrap())
        .collect();
    let defs = Arc::new(Defs::load(&pack).unwrap());
    let vols = [
        Volume::new(4, 9, 4, -6, -3, 5),
        Volume::stepped(3, 4, 3, 8, -64, -12, 4, 8, 4),
        Volume::stepped(3, 4, 3, 1, -3, 2, 2, 3, 2),
    ];
    let inner = [[-6, -3, 5], [-5, 0, 6], [-3, 5, 8], [-4, 4, 7], [0, 0, 0]];
    let mut failures = Vec::new();
    for seed in [0i64, -6_214_586_474_519_478_921, 1_234_567_890_123_456_789] {
        let lines = common::read_lines(&format!("synthetic_{seed}.txt")).unwrap();
        let mut g = Generator::with_defs(defs.clone(), NoiseSettings::load(&pack, "overworld").unwrap(), seed).unwrap();
        for (n, l) in lines.iter().enumerate() {
            let t: Vec<&str> = l.split(' ').collect();
            let f = &funcs[n];
            let (range, axes) = g.declared_of(f);
            if axes.to_string() != t[5] {
                failures.push(format!("f{n} axes {axes} ≠ {}", t[5]));
            }
            let mut check = |what: String, v: f32, r: &str| {
                let r = common::hex_f32(r);
                if v.to_bits() != r.to_bits() {
                    failures.push(format!("f{n} seed {seed} {what}: {v} ≠ {r}"));
                }
            };
            check("range min".into(), range.min, t[2]);
            check("range max".into(), range.max, t[3]);
            let s = g.compile_function(f).unwrap();
            let mut k = 7;
            for p in POINTS {
                check(format!("point {p:?}"), g.value(s, &mut Ctx::uncached(), p[0], p[1], p[2]), t[k]);
                k += 1;
            }
            for v in &vols {
                k += 1;
                let out = g.volume(s, &mut Ctx::uncached(), v);
                for (i, x) in out.iter().enumerate() {
                    check(format!("volume {v:?} [{i}]"), *x, t[k + i]);
                }
                k += v.size();
            }
            let mut ctx = Ctx::cached();
            k += 1;
            let out = g.volume(s, &mut ctx, &vols[0]);
            for (i, x) in out.iter().enumerate() {
                check(format!("volume with caches [{i}]"), *x, t[k + i]);
            }
            k += vols[0].size() + 1;
            for p in inner {
                check(format!("point with caches {p:?}"), g.value(s, &mut ctx, p[0], p[1], p[2]), t[k]);
                k += 1;
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures[..failures.len().min(40)].join("\n"));
}
