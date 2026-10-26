//! Random generators compared with the game's outputs (Java probe).

mod common;

use rc_worldgen::rng::{Rng, biome_zoom_seed};

fn root(alg: &str, seed: i64) -> Rng {
    Rng::for_world(seed, alg == "legacy")
}

#[test]
fn sequences_match_game() {
    let Some(lines) = common::read_lines("rng.txt") else { return };
    let mut checked = 0;
    for l in &lines {
        let t: Vec<&str> = l.split(' ').collect();
        match t[0] {
            "seq" => {
                let seed: i64 = t[2].parse().unwrap();
                let mut r = root(t[1], seed);
                let mut it = t[3..].iter().map(|s| s.parse::<i64>().unwrap());
                for _ in 0..4 {
                    assert_eq!(r.next_long(), it.next().unwrap(), "{l}");
                }
                for _ in 0..4 {
                    assert_eq!(i64::from(r.next_int()), it.next().unwrap(), "{l}");
                }
                for n in [1, 2, 3, 7, 16, 100, 256, 1_000_000_007] {
                    assert_eq!(i64::from(r.next_int_bounded(n)), it.next().unwrap(), "{l} bound {n}");
                }
                for _ in 0..3 {
                    assert_eq!(r.next_double().to_bits() as i64, it.next().unwrap(), "{l}");
                }
                for _ in 0..3 {
                    assert_eq!(i64::from(r.next_float().to_bits() as i32), it.next().unwrap(), "{l}");
                }
                assert_eq!(i64::from(r.next_bool()), it.next().unwrap());
                r.consume(5);
                assert_eq!(r.next_long(), it.next().unwrap(), "{l}");
            }
            "hash" => {
                let seed: i64 = t[2].parse().unwrap();
                let mut q = root(t[1], seed).fork_positional().from_hash_of(t[3]);
                assert_eq!(q.next_long(), t[4].parse::<i64>().unwrap(), "{l}");
                assert_eq!(q.next_long(), t[5].parse::<i64>().unwrap(), "{l}");
            }
            "at" => {
                let seed: i64 = t[2].parse().unwrap();
                let p: Vec<i32> = t[3..6].iter().map(|s| s.parse().unwrap()).collect();
                let mut q = root(t[1], seed).fork_positional().at(p[0], p[1], p[2]);
                assert_eq!(q.next_long(), t[6].parse::<i64>().unwrap(), "{l}");
                assert_eq!(q.next_long(), t[7].parse::<i64>().unwrap(), "{l}");
            }
            "zoom" => {
                assert_eq!(biome_zoom_seed(t[1].parse().unwrap()), t[2].parse::<i64>().unwrap(), "{l}");
            }
            _ => continue,
        }
        checked += 1;
    }
    assert!(checked > 50, "{checked} lines checked");
}

/// Fixed numeric extract (seed 0): usable without the reference files.
#[test]
fn small_inline_extract() {
    let mut r = Rng::legacy(0);
    assert_eq!(r.next_long(), -4_962_768_465_676_381_896);
    let mut h = Rng::legacy(0).fork_positional().from_hash_of("minecraft:offset");
    assert_eq!(h.next_long(), 444_257_796_709_329_839);
}
