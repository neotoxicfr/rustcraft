//! Named timeline curves against values printed by the game itself (26.3 client, x at
//! 0, 0.1, 0.25, 0.3, 0.45, 0.5, 0.6, 0.75, 0.85, 0.95, 1; raw `f32` bits).

use super::named_ease;

const XS: [f32; 11] = [0.0, 0.1, 0.25, 0.3, 0.45, 0.5, 0.6, 0.75, 0.85, 0.95, 1.0];

/// (name, value bits at each of `XS`), in the game's registration order.
const GAME: [(&str, [u32; 11]); 30] = [
    ("in_back", [0x8000_0000, 0xbc6a_8632, 0xbd83_5a08, 0xbda4_3fa9, 0xbdc9_7fe6, 0xbdb3_9ac0, 0xbced_cb1a, 0x3e3a_f8f4, 0x3edc_03c7, 0x3f47_d4d1, 0x3f80_0000]),
    ("in_bounce", [0x0000_0000, 0x3c42_8f80, 0x3ce0_0000, 0x3d8e_1480, 0x3e7f_d70c, 0x3e70_0000, 0x3db8_51e0, 0x3f07_0000, 0x3f54_70a5, 0x3f7b_28f6, 0x3f80_0000]),
    ("in_circ", [0x0000_0000, 0x3ba4_4080, 0x3d02_10a0, 0x3d3c_aa40, 0x3ddb_13d8, 0x3e09_30a4, 0x3e4c_cccc, 0x3ead_5806, 0x3ef2_498a, 0x3f30_1064, 0x3f80_0000]),
    ("in_cubic", [0x0000_0000, 0x3a83_126f, 0x3c80_0000, 0x3cdd_2f1c, 0x3dba_9fbd, 0x3e00_0000, 0x3e5d_2f1c, 0x3ed8_0000, 0x3f1d_374c, 0x3f5b_7ced, 0x3f80_0000]),
    ("in_elastic", [0x0000_0000, 0x3b00_0000, 0xbbb5_04f3, 0xbb80_0004, 0x3c35_04ff, 0xbc7f_fff6, 0xbcff_fffc, 0x3db5_04f7, 0xbeb5_04f5, 0x3eb5_04ec, 0x3f80_0000]),
    ("in_expo", [0x0000_0000, 0x3b00_0000, 0x3bb5_04f3, 0x3c00_0001, 0x3cb5_04f2, 0x3d00_0000, 0x3d80_0001, 0x3e35_04f3, 0x3eb5_04f5, 0x3f35_04f2, 0x3f80_0000]),
    ("in_quad", [0x0000_0000, 0x3c23_d70b, 0x3d80_0000, 0x3db8_51ec, 0x3e4f_5c28, 0x3e80_0000, 0x3eb8_51ec, 0x3f10_0000, 0x3f38_f5c3, 0x3f67_0a3d, 0x3f80_0000]),
    ("in_quart", [0x0000_0000, 0x38d1_b719, 0x3b80_0000, 0x3c04_b5dd, 0x3d27_f62a, 0x3d80_0000, 0x3e04_b5dd, 0x3ea2_0000, 0x3f05_a234, 0x3f50_837a, 0x3f80_0000]),
    ("in_quint", [0x0000_0000, 0x3727_c5ae, 0x3a80_0000, 0x3b1f_40a3, 0x3c97_2a59, 0x3d00_0000, 0x3d9f_40a3, 0x3e73_0000, 0x3ee3_2d59, 0x3f46_1681, 0x3f80_0000]),
    ("in_sine", [0x0000_0000, 0x3c49_9dc0, 0x3d9b_e510, 0x3ddf_3368, 0x3e75_4b14, 0x3e95_f61a, 0x3ed3_09be, 0x3f1e_0876, 0x3f44_3a7b, 0x3f6b_e518, 0x3f80_0000]),
    ("in_out_back", [0x8000_0000, 0xbd19_ad0e, 0xbdcc_25fe, 0xbda1_7372, 0x3e84_d0e0, 0x3f00_0000, 0x3f68_faa0, 0x3f8c_c260, 0x3f88_bc14, 0x3f81_6e40, 0x3f80_0000]),
    ("in_out_bounce", [0x0000_0000, 0x3cf5_c290, 0x3df0_0000, 0x3d38_51e0, 0x3eec_a3d6, 0x3f00_0000, 0x3f26_b853, 0x3f62_0000, 0x3f77_1eb9, 0x3f7e_7ae1, 0x3f80_0000]),
    ("in_out_circ", [0x0000_0000, 0x3c25_8313, 0x3d89_30a3, 0x3dcc_ccce, 0x3e90_6984, 0x3f00_0000, 0x3f4c_cccd, 0x3f6e_d9ec, 0x3f7a_1aae, 0x3f7f_5bc0, 0x3f80_0000]),
    ("in_out_cubic", [0x0000_0000, 0x3b83_126f, 0x3d80_0000, 0x3ddd_2f1c, 0x3eba_9fbd, 0x3f00_0000, 0x3f3e_76c9, 0x3f70_0000, 0x3f7c_8b44, 0x3f7f_df3b, 0x3f80_0000]),
    ("in_out_elastic", [0x0000_0000, 0x39b1_d101, 0x3c44_1b78, 0x3cc4_1b80, 0x3d31_d0b1, 0x3f00_0000, 0x3f8f_08fb, 0x3f7c_ef92, 0x3f80_7848, 0x3f7f_c000, 0x3f80_0000]),
    ("in_out_expo", [0x0000_0000, 0x3b00_0000, 0x3c80_0000, 0x3d00_0001, 0x3e7f_fffd, 0x3f00_0000, 0x3f60_0001, 0x3f7c_0000, 0x3f7f_0000, 0x3f7f_c000, 0x3f80_0000]),
    ("in_out_quad", [0x0000_0000, 0x3ca3_d70b, 0x3e00_0000, 0x3e38_51ec, 0x3ecf_5c28, 0x3f00_0000, 0x3f2e_147c, 0x3f60_0000, 0x3f74_7ae2, 0x3f7e_b852, 0x3f80_0000]),
    ("in_out_quart", [0x0000_0000, 0x3a51_b719, 0x3d00_0000, 0x3d84_b5dd, 0x3ea7_f62a, 0x3f00_0000, 0x3f4b_923b, 0x3f78_0000, 0x3f7e_f694, 0x3f7f_fcb9, 0x3f80_0000]),
    ("in_out_quint", [0x0000_0000, 0x3927_c5ad, 0x3c80_0000, 0x3d1f_40a4, 0x3e97_2a59, 0x3f00_0000, 0x3f56_0e96, 0x3f7c_0000, 0x3f7f_b060, 0x3f7f_ffac, 0x3f80_0000]),
    ("in_out_sine", [0x8000_0000, 0x3cc8_6020, 0x3e15_f61a, 0x3e53_09be, 0x3ed7_f030, 0x3f00_0000, 0x3f27_8b7a, 0x3f5a_827a, 0x3f72_0b5c, 0x3f7e_6c47, 0x3f80_0000]),
    ("out_back", [0x0000_0000, 0x3ed1_51e8, 0x3f51_41c4, 0x3f68_39d2, 0x3f88_5a32, 0x3f8b_39ac, 0x3f8c_b78e, 0x3f88_35a0, 0x3f83_bbc4, 0x3f80_8054, 0x3f80_0000]),
    ("out_bounce", [0x0000_0000, 0x3d9a_e148, 0x3ef2_0000, 0x3f2e_3d71, 0x3f51_a3d8, 0x3f44_0000, 0x3f45_c28f, 0x3f79_0000, 0x3f71_f5c3, 0x3f7c_0a3d, 0x3f80_0000]),
    ("out_circ", [0x0000_0000, 0x3edf_2cf8, 0x3f29_53fd, 0x3f36_d211, 0x3f55_cd5a, 0x3f5d_b3d7, 0x3f6a_a0bd, 0x3f77_def6, 0x3f7d_1a87, 0x3f7f_ae07, 0x3f80_0000]),
    ("out_cubic", [0x0000_0000, 0x3e8a_c086, 0x3f14_0000, 0x3f28_3127, 0x3f55_6872, 0x3f60_0000, 0x3f6f_9db2, 0x3f7c_0000, 0x3f7f_22d1, 0x3f7f_f7cf, 0x3f80_0000]),
    ("out_elastic", [0x0000_0000, 0x3fa0_0000, 0x3f69_5f61, 0x3f60_0000, 0x3f85_a828, 0x3f82_0000, 0x3f7c_0000, 0x3f80_b505, 0x3f7f_a57d, 0x3f7f_d2bf, 0x3f80_0000]),
    ("out_expo", [0x0000_0000, 0x3f00_0000, 0x3f52_bec3, 0x3f60_0000, 0x3f74_afb1, 0x3f78_0000, 0x3f7c_0000, 0x3f7e_95f6, 0x3f7f_4afb, 0x3f7f_a57e, 0x3f80_0000]),
    ("out_quad", [0x0000_0000, 0x3e42_8f60, 0x3ee0_0000, 0x3f02_8f5c, 0x3f32_8f5c, 0x3f40_0000, 0x3f57_0a3e, 0x3f70_0000, 0x3f7a_3d71, 0x3f7f_5c29, 0x3f80_0000]),
    ("out_quart", [0x0000_0000, 0x3eb0_13ac, 0x3f2f_0000, 0x3f42_88cf, 0x3f68_930c, 0x3f70_0000, 0x3f79_7247, 0x3f7f_0000, 0x3f7f_ded3, 0x3f7f_ff97, 0x3f80_0000]),
    ("out_quint", [0x0000_0000, 0x3ed1_ab4c, 0x3f43_4000, 0x3f54_f95e, 0x3f73_1dad, 0x3f78_0000, 0x3f7d_60e9, 0x3f7f_c000, 0x3f7f_fb06, 0x3f7f_fffb, 0x3f80_0000]),
    ("out_sine", [0x0000_0000, 0x3e20_266d, 0x3ec3_ef15, 0x3ee8_6f34, 0x3f26_3e68, 0x3f35_04f3, 0x3f4f_1a43, 0x3f6c_835e, 0x3f78_eca6, 0x3f7f_3594, 0x3f80_0000]),
];

#[test]
fn named_curves_match_the_game_bit_for_bit() {
    assert_eq!(rc_data::EASE_NAMES.len(), GAME.len());
    for (i, (name, bits)) in GAME.iter().enumerate() {
        assert_eq!(rc_data::EASE_NAMES[i], *name, "registration order");
        for (x, want) in XS.iter().zip(bits) {
            let got = named_ease(i as u8, *x);
            assert_eq!(got.to_bits(), *want, "{name}({x}) = {got}, game {}", f32::from_bits(*want));
        }
    }
}

/// Micro-bench: a named curve against the Bezier curve (30 curves x 10 000 points).
#[test]
#[ignore = "micro-bench"]
fn bench_named_curves() {
    let xs: Vec<f32> = (0..10_000).map(|i| i as f32 / 10_000.0).collect();
    for pass in 0..2 {
        let t = std::time::Instant::now();
        let mut s = 0.0f32;
        for i in 0..30u8 {
            for &x in &xs {
                s += named_ease(i, std::hint::black_box(x));
            }
        }
        let named = t.elapsed().as_nanos() as f64 / 300_000.0;
        let t = std::time::Instant::now();
        for _ in 0..30 {
            for &x in &xs {
                s += super::bezier([0.362, 0.241, 0.638, 0.759], std::hint::black_box(x));
            }
        }
        let bez = t.elapsed().as_nanos() as f64 / 300_000.0;
        eprintln!("pass {pass}: named {named:.2} ns, bezier {bez:.2} ns ({s})");
    }
}
