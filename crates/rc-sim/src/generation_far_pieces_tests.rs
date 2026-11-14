//! A structure piece placed far from its starting chunk after that chunk left the generation
//! thread as a full chunk (a stronghold's chest corridor): the starting chunk the server holds
//! takes the piece's new state, and the chunk's next save writes it, as the game's starting chunk
//! holds the live start and writes it whenever it is saved. The whole production path is used:
//! generation thread, server, save to the region file. Fresh world with structures: the 26.3 data
//! cache and the client jar are required.

use rc_proto::nbt::Nbt;
use rc_worldgen::chunkgen::DimGen;
use rc_worldgen::structure::{Piece, Placement};
use rc_worldgen::structure_coded::CodedPiece;
use rc_worldgen::structure_stronghold::ShKind;

use crate::server::Server;

/// The seed of the stronghold tests of the generator (a chest corridor in its first strongholds).
const SEED: i64 = 12345;

/// Closest Chebyshev distance, in chunks, from a starting chunk to a chest corridor for the
/// corridor to stay undecorated while the starting chunk is made full (that decorates its eight
/// neighbours only).
const FAR: i32 = 3;

/// A chest corridor of one of the first strongholds, all of whose chunks lie at least `FAR`
/// chunks from the starting chunk.
struct FarCorridor {
    start: (i32, i32),
    /// Index of the corridor among the start's pieces (its place in `Children`).
    index: usize,
    chunks: Vec<(i32, i32)>,
}

fn world(tag: &str) -> Option<(Server, std::path::PathBuf)> {
    let data = std::sync::Arc::new(rc_testdata::version_data()?);
    crate::registries::client_jar(&data)?;
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-far-pieces-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = SEED;
    w.generate_structures = true;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    Some((crate::portal_tests::open_world(&dir, data), dir))
}

fn far_chest_corridor(dg: &DimGen) -> Option<FarCorridor> {
    let sg = dg.structures.as_ref()?;
    let t = dg.terrain_generator()?;
    let rings = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. }))?;
    sg.ring_positions(rings, t).iter().take(8).find_map(|&start| {
        let starts = dg.structure_starts(start.0, start.1);
        let st = starts.iter().find(|s| sg.defs[s.structure].id == "minecraft:stronghold")?;
        st.pieces.iter().enumerate().find_map(|(index, p)| {
            let Piece::Coded { data, .. } = p else { return None };
            let CodedPiece::Stronghold(sp) = &**data else { return None };
            if !matches!(sp.kind, ShKind::ChestCorridor) {
                return None;
            }
            let b = p.bbox();
            let chunks: Vec<(i32, i32)> = (b.min[0] >> 4..=b.max[0] >> 4)
                .flat_map(|x| (b.min[2] >> 4..=b.max[2] >> 4).map(move |z| (x, z)))
                .collect();
            let far = chunks.iter().all(|&(x, z)| (x - start.0).abs().max((z - start.1).abs()) >= FAR);
            far.then_some(FarCorridor { start, index, chunks })
        })
    })
}

/// `Chest` of the corridor in a chunk compound's `structures` (`None`: not recorded there).
fn chest_flag(root: &Nbt, index: usize) -> Option<i64> {
    let stronghold = root.get("structures")?.get("starts")?.get("minecraft:stronghold")?;
    let Some(Nbt::List(children)) = stronghold.get("Children") else { return None };
    children.get(index)?.get("Chest")?.as_i64()
}

/// The corridor is decorated once its starting chunk is already full on the server: the server's
/// chunk takes `Chest` 1 without being marked to save, and the save writes it into the region
/// file. Before: the compound kept the state of the chunk's own batch, `Chest` 0, on disk too.
#[test]
fn far_chest_corridor_reaches_the_saved_starting_chunk() {
    let Some((mut s, _dir)) = world("chest") else { return };
    let dg = s.generator.as_ref().and_then(|g| g.thread_generator(&s.dim.name, true)).expect("generation thread");
    let corridor = far_chest_corridor(&dg).expect("a chest corridor far from its starting chunk");
    s.load_now(&[corridor.start]);
    let held = |s: &Server| {
        s.chunks.get(&corridor.start).and_then(|c| c.as_ref()).and_then(|c| chest_flag(&c.root, corridor.index))
    };
    assert_eq!(held(&s), Some(0), "starting chunk full before its corridor is placed");
    s.load_now(&corridor.chunks);
    assert_eq!(
        held(&s),
        Some(1),
        "corridor placed at {:?}: chest not written into its starting chunk",
        corridor.chunks
    );
    s.save(true);
    let saved = s.dim.load_raw("region", &[corridor.start]);
    let root = saved.first().and_then(|(_, n)| n.as_ref()).expect("starting chunk saved");
    assert_eq!(chest_flag(root, corridor.index), Some(1), "chest not written into the saved starting chunk");
}

/// The generation thread's worker (without its thread) on the world in `dir`.
fn worker_on(dir: &std::path::Path) -> (super::Worker, crossbeam_channel::Receiver<super::Out>) {
    const DIM: &str = crate::dims::OVERWORLD;
    let data = std::sync::Arc::new(rc_testdata::version_data().expect("26.3 data"));
    let jar = crate::registries::client_jar(&data).expect("client jar");
    let regs = crate::registries::load(&jar, &data).expect("registries");
    let (spec, seed) = super::read_spec(dir, DIM).expect("generation settings");
    let (tx, rx) = crossbeam_channel::unbounded();
    let biome_registry = regs.entries("minecraft:worldgen/biome").to_vec();
    let height = crate::dimension_height(&data, DIM);
    let setup = super::Setup { data, world: dir.to_owned(), dimension: DIM.into(), height, biome_registry, spec, seed };
    (super::Worker::new(setup, tx).expect("generator"), rx)
}

/// Cost of the report of far pieces, once a stronghold's starting chunk and its far corridor are
/// generated: one call over every chunk the worker holds (more than any batch decorates). Printed,
/// never checked: `cargo test --release -p rc-sim -- --ignored bench_far_piece_report --nocapture`.
#[test]
#[ignore]
fn bench_far_piece_report() {
    let Some((s, dir)) = world("bench") else { return };
    drop(s);
    let (mut w, rx) = worker_on(&dir);
    let corridor = far_chest_corridor(&w.dg).expect("a chest corridor far from its starting chunk");
    w.process(&[corridor.start]);
    w.process(&corridor.chunks);
    let held: Vec<(i32, i32)> = w.map.keys().copied().collect();
    let _ = rx.try_iter().count();
    let runs: u32 = 200;
    let t = std::time::Instant::now();
    for _ in 0..runs {
        w.send_pieces_placed_far_from_their_start(&held);
    }
    let per_call = t.elapsed() / runs;
    let messages = rx.try_iter().count() / runs as usize;
    eprintln!(
        "[bench] far piece report: {} chunks walked, {messages} message(s) per call, {per_call:?} per call, {:?} per chunk",
        held.len(),
        per_call / held.len().max(1) as u32
    );
}
