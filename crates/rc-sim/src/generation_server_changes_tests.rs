//! Server block changes and the light the generation thread sends back.
//!
//! The thread lights each new chunk, and relights the full chunks it already returned next to it,
//! from its own copy of their blocks. A change the server made in those chunks (a torch placed at
//! the edge of the explored world, a dug hole, flowing water) must reach that copy, or be relit by
//! the server when it reaches it too late: otherwise the relight brings back the light of blocks
//! that are gone, and the new chunk is lit as if the change never happened. The game has a single
//! light engine reading the live blocks, so its light always follows the current blocks.
//!
//! Every test places glowstone at the east edge of a full chunk at the edge of the generated area,
//! then generates the chunk beyond it, and compares the server's block light of both chunks with
//! the light recomputed from the server's own blocks. The world is flat without structures: the
//! proto-chunks around hold no light source, so the server's blocks alone give the right light.

use std::sync::Arc;

use rc_data::VersionData;
use rc_world::LightLayer;

use crate::server::Server;

/// Full chunk at the edge of the generated area, far from the spawn.
const EDGE: (i32, i32) = (100, 0);
/// Chunk generated next to it, to the east.
const NEXT: (i32, i32) = (101, 0);
/// Chunk far from both: generating it runs a batch that touches neither.
const FAR: (i32, i32) = (200, 0);

/// A position in the air above the flat ground, in the east column of [`EDGE`].
fn edge_column(z: i32, y: i32) -> [i32; 3] {
    [EDGE.0 * 16 + 15, y, EDGE.1 * 16 + z]
}

/// New flat world without structures, opened; `keep`: cap of the generator's table.
fn flat_world(tag: &str, keep: usize) -> (Server, std::path::PathBuf, Arc<VersionData>) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 data missing (rc-data cache and client jar)"));
    crate::registries::client_jar(&data).expect("26.3 client jar missing");
    let dir =
        rc_testdata::scratch(std::env::temp_dir().join(format!("rc-gen-server-changes-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 20_261_109;
    w.preset = crate::newworld::Preset::Flat;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    super::TEST_KEEP.with(|k| k.set(keep));
    let s = crate::portal_tests::open_world(&dir, data.clone());
    super::TEST_KEEP.with(|k| k.set(super::KEEP));
    (s, dir, data)
}

/// Glowstone placed at `p` like any block change, then the end of the tick (server relight).
fn place_glowstone(s: &mut Server, p: [i32; 3]) {
    let state = s.view().default_state(s.table().block_id("minecraft:glowstone"));
    assert!(s.set_block(p, state, crate::blocks::flag::ALL), "glowstone not placed at {p:?}");
    s.flush_block_changes();
    assert_eq!(block_light_at(s, p), 15, "the server did not light its own glowstone at {p:?}");
}

/// Light level stored in `layers` at cell `i` of layer `layer` (absent layer: 0).
fn level(layers: &[LightLayer], layer: usize, i: usize) -> u8 {
    layers.get(layer).and_then(Option::as_ref).map_or(0, |a| (a[i / 2] >> ((i & 1) * 4)) & 15)
}

/// The server's block light at `p`.
fn block_light_at(s: &Server, p: [i32; 3]) -> u8 {
    let Some(Some(c)) = s.chunks.get(&(p[0] >> 4, p[2] >> 4)) else { panic!("chunk of {p:?} not loaded") };
    let ly = p[1] - s.dim.height.min_y;
    let i = rc_world::block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize);
    level(&c.block, (ly >> 4) as usize + 1, i)
}

/// Block light of chunk `p` recomputed from the server's own blocks of its 3 × 3.
fn block_light_from_server_blocks(s: &Server, (x, z): (i32, i32)) -> Vec<LightLayer> {
    let columns: Vec<Option<Vec<u32>>> = (0..9)
        .map(|i| match s.chunks.get(&(x + i % 3 - 1, z + i / 3 - 1)) {
            Some(Some(c)) => Some(c.unpack_blocks()),
            _ => None,
        })
        .collect();
    let refs: rc_light::Columns = std::array::from_fn(|i| columns[i].as_deref());
    rc_light::light_chunk(&s.light, refs, s.dim.height.sections as i32 * 16, s.dim_kind().sky).1
}

/// The server's block light of chunk `p` equals the light recomputed from its blocks.
fn assert_block_light_follows_server_blocks(s: &Server, p: (i32, i32), what: &str) {
    let Some(Some(c)) = s.chunks.get(&p) else { panic!("{what}: chunk {p:?} not loaded") };
    let want = block_light_from_server_blocks(s, p);
    let mut wrong = Vec::new();
    for layer in 0..want.len().max(c.block.len()) {
        for i in 0..4096 {
            let (got, expected) = (level(&c.block, layer, i), level(&want, layer, i));
            if got != expected {
                let y = s.dim.height.min_y + (layer as i32 - 1) * 16 + (i >> 8) as i32;
                let at = [p.0 * 16 + (i & 15) as i32, y, p.1 * 16 + ((i >> 4) & 15) as i32];
                wrong.push(format!("{at:?}: {got} instead of {expected}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{what}: block light of chunk {p:?} differs from its blocks in {} cells, including {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(6)]
    );
}

/// Positions the server relit itself because they reached the generator after its light.
fn relit_by_server(s: &Server) -> u64 {
    s.generator.as_ref().expect("generator").block_changes.relit_by_server
}

/// Both chunks lit from the current blocks, the glowstone at `p` and its light in the new chunk.
fn assert_edge_and_next_lit_from_current_blocks(s: &Server, p: [i32; 3], what: &str) {
    assert_block_light_follows_server_blocks(s, EDGE, what);
    assert_block_light_follows_server_blocks(s, NEXT, what);
    assert_eq!(block_light_at(s, p), 15, "{what}: glowstone at {p:?} went dark");
    assert_eq!(block_light_at(s, [p[0] + 1, p[1], p[2]]), 14, "{what}: no glowstone light in the new chunk");
}

/// Anti-hang guard of the waits on the generation thread (never the normal path: a loaded
/// machine only makes the batches slower).
const GUARD: std::time::Duration = std::time::Duration::from_secs(300);

/// Waits until every output of the generator's batches received so far is taken.
fn wait_for_batch(s: &mut Server, batch: u64) {
    let t0 = std::time::Instant::now();
    while s.generator.as_ref().expect("generator").seen < batch {
        assert!(t0.elapsed() < GUARD, "anti-hang guard: batch {batch} never ended");
        s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// The generator still holds the edge chunk in its table: the next batch relights it.
#[test]
fn relit_edge_chunk_keeps_a_block_placed_before_the_batch() {
    let (mut s, dir, _) = flat_world("held", super::KEEP);
    s.load_now(&[EDGE]);
    let p = edge_column(8, -55);
    place_glowstone(&mut s, p);
    s.load_now(&[NEXT]);
    assert_edge_and_next_lit_from_current_blocks(&s, p, "edge chunk held by the generator");
    // Sent before the request: the generator's own light was right, nothing relit after it.
    assert_eq!(relit_by_server(&s), 0, "the change did not reach the generator before its batch");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The block is placed after the batch computed its light and before the server takes it: the
/// server relights the change after taking the batch's light.
#[test]
fn block_placed_while_the_batch_runs_keeps_its_light() {
    let (mut s, dir, _) = flat_world("in-flight", super::KEEP);
    s.load_now(&[EDGE]);
    wait_for_batch(&mut s, 1);
    s.ensure_loaded(&[NEXT]);
    // The batch's first output: its light is computed, without the glowstone below.
    let ready = {
        let g = s.generator.as_ref().expect("generator");
        let mut sel = crossbeam_channel::Select::new();
        sel.recv(&g.rx);
        sel.ready_timeout(GUARD).is_ok()
    };
    assert!(ready, "anti-hang guard: the batch of {NEXT:?} sent nothing");
    let p = edge_column(8, -55);
    place_glowstone(&mut s, p);
    s.load_now(&[NEXT]);
    assert_edge_and_next_lit_from_current_blocks(&s, p, "block placed during the batch");
    assert!(relit_by_server(&s) > 0, "the late change was not relit by the server");
    // The generator applies it after its batch and says so: the server forgets it.
    let t0 = std::time::Instant::now();
    while s.generator.as_ref().expect("generator").block_changes.sent_not_applied_count() > 0 {
        assert!(t0.elapsed() < GUARD, "anti-hang guard: change never reported");
        s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The generator set the edge chunk aside (table capped at 0) before the change: it takes it back
/// from its waiting table for the relight.
#[test]
fn set_aside_edge_chunk_gets_the_changes_made_meanwhile() {
    let (mut s, dir, _) = flat_world("set-aside", 0);
    s.load_now(&[EDGE]);
    let p = edge_column(8, -55);
    place_glowstone(&mut s, p);
    s.load_now(&[NEXT]);
    assert_edge_and_next_lit_from_current_blocks(&s, p, "edge chunk set aside by the generator");
    assert_eq!(relit_by_server(&s), 0, "the set-aside copy did not catch up by itself");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The edge chunk is saved, then dropped by the generator, then read back from disk for the
/// relight: one glowstone placed before the save (on disk), one after (not yet on disk).
#[test]
fn edge_chunk_read_back_from_disk_gets_the_changes_made_since_the_save() {
    let (mut s, dir, _) = flat_world("reread", 0);
    s.load_now(&[EDGE]);
    let before_save = edge_column(4, -55);
    place_glowstone(&mut s, before_save);
    s.save(true);
    // A batch elsewhere: the generator forgets the edge chunk, now on disk.
    s.load_now(&[FAR]);
    let after_save = edge_column(12, -50);
    place_glowstone(&mut s, after_save);
    s.load_now(&[NEXT]);
    assert_edge_and_next_lit_from_current_blocks(&s, before_save, "glowstone saved before the read");
    assert_edge_and_next_lit_from_current_blocks(&s, after_save, "glowstone placed after the save");
    assert_eq!(relit_by_server(&s), 0, "the copy read back from disk did not catch up by itself");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The edge chunk comes from an earlier session (full on disk, never relit by this generator): the
/// new chunk next to it is lit with its current blocks.
#[test]
fn chunk_of_an_earlier_session_lights_its_new_neighbour_with_its_current_blocks() {
    let (mut s, dir, data) = flat_world("earlier-session", super::KEEP);
    s.load_now(&[EDGE]);
    s.stop_gen();
    s.save(true);
    drop(s);
    let mut s = crate::portal_tests::open_world(&dir, data);
    s.load_now(&[EDGE]);
    let p = edge_column(8, -55);
    place_glowstone(&mut s, p);
    s.load_now(&[NEXT]);
    assert_edge_and_next_lit_from_current_blocks(&s, p, "edge chunk of an earlier session");
    assert_eq!(relit_by_server(&s), 0, "the copy read from disk did not catch up by itself");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
