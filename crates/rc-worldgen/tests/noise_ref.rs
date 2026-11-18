//! Normal noises compared with the game (Java probe), point and volume paths, at 0 ulp.

mod common;

use rc_worldgen::noise::NormalNoise;
use rc_worldgen::rng::Rng;
use rc_worldgen::volume::Volume;

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
fn normal_noises_match_game() {
    let Some(pack) = common::data_pack() else { return };
    let mut checked = 0;
    for seed in [0i64, -6_214_586_474_519_478_921, 1_234_567_890_123_456_789] {
        let Some(lines) = common::read_lines(&format!("noises_{seed}.txt")) else { return };
        let root = Rng::xoroshiro(seed).fork_positional();
        for l in &lines {
            let t: Vec<&str> = l.split(' ').collect();
            let id = t[0];
            let params = NormalNoise::from_json(&pack.json("noise", id).unwrap()).unwrap();
            let noise = params.create(&mut root.from_hash_of(id));
            let range = noise.range();
            assert_eq!(range.min, common::hex_f32(t[2]), "{id} range");
            assert_eq!(range.max, common::hex_f32(t[3]), "{id} range");
            assert_eq!(t[4], "pts");
            let mut k = 5;
            for p in POINTS {
                let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
                for s in [1.0, 0.25, 0.666_666_666_666_666_6] {
                    let v = noise.get(x * s, y * s, z * s);
                    assert_eq!(v.to_bits(), common::hex_f32(t[k]).to_bits(), "{id} seed {seed} point {p:?} scale {s}");
                    k += 1;
                }
                let v = noise.get(x + 0.37, y * 0.5, z * 1.3);
                assert_eq!(v.to_bits(), common::hex_f32(t[k]).to_bits(), "{id} seed {seed} point {p:?} fractional");
                k += 1;
            }
            assert_eq!(t[k], "vol");
            k += 1;
            let vol = Volume::stepped(3, 5, 3, -7, 20, 11, 1, 2, 3);
            let mut buf = vec![0.0f32; vol.size()];
            noise.add_to_volume(&mut buf, &vol, 0.666_666_666_666_666_6, 1.0, 1.0);
            for (i, v) in buf.iter().enumerate() {
                assert_eq!(v.to_bits(), common::hex_f32(t[k + i]).to_bits(), "{id} seed {seed} volume {i}");
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked} noises checked");
}
