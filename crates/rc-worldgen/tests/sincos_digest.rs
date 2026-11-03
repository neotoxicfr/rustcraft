//! Frozen digest of the bits of `jmath` sin, cos and `sincos_shared`, recorded with the code
//! before the optimisation switches of `jmath`: every switch combination must give it again.
//! Only angles our own code computes (10^-8 <= |x| <= 10^5): beyond, the system library answers
//! and differs between Windows and Linux.

use rc_worldgen::jmath;

/// Digest recorded with the old code (library `fma`, both series run one after the other).
const EXPECTED_DIGEST: u64 = 0xc0fd_0c47_1648_9322;

/// Order-dependent mix of 64-bit words (multiply then rotate).
fn mix(digest: u64, word: u64) -> u64 {
    (digest ^ word).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29)
}

/// 10^6 pseudo-random angles in ±10^5 (splitmix64, fixed seed) then a regular grid near zero.
fn angles() -> impl Iterator<Item = f64> {
    let random = (0..1_000_000u64).map(|i| {
        let mut z = i.wrapping_add(7).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        ((z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 1e5
    });
    let grid = (-20_000..20_000).map(|i| f64::from(i) * 1e-3 + 1e-6);
    random.chain(grid).filter(|x| x.abs() >= 1e-8)
}

fn digest() -> u64 {
    angles().fold(0, |d, x| {
        let (s, c) = jmath::sincos_shared(x);
        let d = mix(mix(d, jmath::sin(x).to_bits()), jmath::cos(x).to_bits());
        mix(mix(d, s.to_bits()), c.to_bits())
    })
}

#[test]
fn sin_cos_bits_match_frozen_digest() {
    let got = digest();
    assert_eq!(got, EXPECTED_DIGEST, "sin/cos bits changed: digest {got:#018x}");
}
