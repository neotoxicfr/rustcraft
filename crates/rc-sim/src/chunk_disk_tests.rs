//! Saved chunk compound: `/fillbiome` reaches the disk, on a chunk read from an official world and
//! on a chunk the generator has just made.

use std::sync::Arc;

use crate::server::Server;
use crate::world::{Dimension, Load};

/// Biome of the 4×4×4 cell holding block `p` in the chunk `c`.
fn biome_at(c: &crate::chunk::Chunk, p: [i32; 3], min_y: i32) -> u32 {
    let ly = p[1] - min_y;
    let cell = (((ly & 15) >> 2) << 4) | (((p[2] & 15) >> 2) << 2) | ((p[0] & 15) >> 2);
    c.sections[(ly >> 4) as usize].biomes.get(cell as usize)
}

/// Runs `/fillbiome` over the 4×4×4 cell at `p` (as the server).
fn fill_cell(s: &mut Server, p: [i32; 3], biome: &str) {
    let text = format!("fillbiome {} {} {} {} {} {} {biome}", p[0], p[1], p[2], p[0] + 3, p[1] + 3, p[2] + 3);
    let src = s.server_source();
    let (ok, n) = s.perform(src, &text);
    assert!(ok && n > 0, "{text}: refused or nothing changed");
}

/// Chunk `col` as it reads back from the disk.
fn reread(dir: &std::path::Path, data: &rc_data::VersionData, states: &crate::states::States, col: (i32, i32)) -> crate::chunk::Chunk {
    let h = crate::dimension_height(data, crate::dims::OVERWORLD);
    let dim = Dimension::new(dir, crate::dims::OVERWORLD, h);
    let loaded = dim.load(&[col], data, states);
    let Load::Loaded(c) = &loaded[0].1 else { panic!("chunk {col:?} not read back: {:?}", loaded[0].1) };
    (**c).clone()
}

/// A biome changed by `/fillbiome` is the one read back after the save, in a chunk of the official
/// world (its sections already hold biomes on disk) and in a chunk generated during the session
/// (the generator wrote them). The old save re-encoded the blocks of the marked sections but kept
/// the biomes read: the change was lost at the next opening.
#[test]
fn fillbiome_survives_the_save() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    let dir = crate::tests::fresh_copy(&world, "fillbiome-save");
    let data = Arc::new(data);
    let mut s = crate::portal_tests::open_world(&dir, data.clone());
    let states = s.states.clone();
    let min_y = s.dim.height.min_y;
    let near = ((s.pos[0].floor() as i32) >> 4, (s.pos[2].floor() as i32) >> 4);
    // Far from anything saved: made by the generator, never written yet.
    let far = (near.0 + 300, near.1 + 300);
    s.load_now(&[near, far]);
    assert!(s.chunks.get(&far).is_some_and(Option::is_some), "generated chunk {far:?} not loaded");
    let desert = states.biomes["minecraft:desert"];
    let cells: Vec<((i32, i32), [i32; 3])> = [near, far].iter().map(|&(cx, cz)| ((cx, cz), [cx * 16 + 4, 64, cz * 16 + 8])).collect();
    for &(col, p) in &cells {
        let c = s.chunks[&col].as_ref().unwrap();
        assert_ne!(biome_at(c, p, min_y), desert, "{col:?} already desert: the test would prove nothing");
    }
    for &(_, p) in &cells {
        fill_cell(&mut s, p, "minecraft:desert");
    }
    s.save(true);
    drop(s);
    for &(col, p) in &cells {
        let c = reread(&dir, &data, &states, col);
        assert_eq!(biome_at(&c, p, min_y), desert, "{col:?}: biome set by /fillbiome lost at the save");
    }
}
