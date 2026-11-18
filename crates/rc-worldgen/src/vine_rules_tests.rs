//! G6-F23: "up" face of a vine placed by generation. The game requires the TOP face of the
//! block above (quirk reproduced): a bottom slab above makes the vine fall, a
//! top slab keeps it, at survival as at shape update. Game data absent:
//! test skipped, failure under `RUSTCRAFT_OPT_REQUIRE=1`.

use crate::decor_blocks::{Blocks, Dir};
use crate::decor_data::DecorData;
use crate::decor_region::{DecoChunk, Region, add};

const P: [i32; 3] = [8, 8, 8];

#[test]
fn vine_up_face_reads_the_top_of_the_block_above() {
    let Some(jar) = rc_testdata::need(
        rc_data::locate("26.3").ok().map(|i| i.client_jar).filter(|p| p.is_file()),
        "jar client 26.3",
    ) else {
        return;
    };
    let Some(v) = rc_testdata::version_data() else { return };
    let data = DecorData::from_jar(&jar).expect("decoration data");
    let b = Blocks::new(&v, &data).expect("blocks");
    let mut chunks: Vec<DecoChunk> = (0..9).map(|k| DecoChunk::new(k % 3 - 1, k / 3 - 1, 0, 32)).collect();
    let arr: [&mut DecoChunk; 9] = {
        let mut it = chunks.iter_mut();
        std::array::from_fn(|_| it.next().unwrap())
    };
    let mut r = Region::new(arr, &b, 4, 1);
    let vine_id = b.block_id("minecraft:vine").expect("vine");
    let vine = b.with_bool(b.default_state(vine_id), b.prop("up").expect("up"), true);
    let slab_id = b.block_id("minecraft:smooth_stone_slab").expect("slab");
    let ty = b.prop("type").expect("type");
    for (slab, kept) in [("bottom", false), ("top", true)] {
        let above = b.with_str(b.default_state(slab_id), ty, slab);
        r.set_p(add(P, Dir::Up), above, 2);
        r.set_p(P, vine, 2);
        assert_eq!(crate::decor_rules::can_survive(&r, vine, P), kept, "survival, slab {slab}");
        let got = crate::decor_rules::update_shape(&r, vine, P, Dir::Up, above);
        assert_eq!(got == vine, kept, "shape update, slab {slab}");
    }
}
