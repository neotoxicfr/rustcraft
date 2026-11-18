//! Tests of family F14 (warden, breeze, creaking, happy ghast).

use super::*;

#[test]
fn info_sheet_dimensions() {
    for (k, w, h) in [
        (Kind::Warden, 0.9f32, 2.9f32),
        (Kind::Breeze, 0.6, 1.77),
        (Kind::Creaking, 0.9, 2.7),
        (Kind::HappyGhast, 4.0, 4.0),
    ] {
        let i = info(k);
        assert_eq!((i.width, i.height), (w, h), "{k:?}");
    }
}
