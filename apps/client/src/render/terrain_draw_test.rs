//! Proofs on the terrain draw commands: the indexed form of the instanced commands, and the batch loop
//! that skips the bindings the pass already holds (`RUSTCRAFT_OPT_TERRAIN_BIND_STATE`).

use super::*;
use crate::render::face_indices;

/// Indexed commands (`RUSTCRAFT_OPT_FACEIDX`): same faces, in the same order, as the
/// instanced commands, including beyond an index slice; one instance, no
/// vertex offset (false on DX12 without indirect validation).
#[test]
fn indexed_args_draw_the_same_faces_in_order() {
    let a = |n, first| DrawArgs { vertex_count: 6, instance_count: n, first_vertex: 0, first_instance: first };
    let args = [a(40_000, 7), a(1, 100_000), a(INDEXED_FACES, 3), a(INDEXED_FACES + 1, 9)];
    let mut out = Vec::new();
    indexed_args(&args, &mut out);
    let index = face_indices(INDEXED_FACES);
    let drawn: Vec<u32> = out
        .iter()
        .flat_map(|c| {
            assert!(c.index_count as usize <= index.len() && c.index_count % 6 == 0, "{c:?}");
            assert_eq!((c.instance_count, c.first_index, c.base_vertex), (1, 0, 0), "{c:?}");
            index[..c.index_count as usize].chunks(6).map(move |t| c.first_instance + u32::from(t[0] >> 2))
        })
        .collect();
    let expected: Vec<u32> =
        args.iter().flat_map(|a| a.first_instance..a.first_instance + a.instance_count).collect();
    assert_eq!(drawn, expected);
    assert_eq!(out.len(), 3 + 1 + 1 + 2);
}

/// What the GPU holds at a draw (`None` / `false`: unknown, left by something else).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PassState {
    pipeline: Option<(usize, bool)>,
    region_group: Option<(i32, i32)>,
    vertex_slot: Option<VertexSlot>,
    face_index: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Recorded {
    Features,
    Indexed { batch: usize, state: PassState },
    Instanced { batch: usize, first: u32, count: u32, state: PassState },
}

/// A pass that follows its bindings like the GPU and notes the state of each draw.
#[derive(Default)]
struct Recorder {
    state: PassState,
    recorded: Vec<Recorded>,
    bindings_set: usize,
}

impl TerrainCommands for Recorder {
    fn features(&mut self) {
        self.recorded.push(Recorded::Features);
        // Entities, texts, particles... bind their own pipelines, groups and buffers.
        self.state = PassState::default();
    }

    fn pipeline(&mut self, layer: usize, fluid: bool) {
        self.state.pipeline = Some((layer, fluid));
    }

    fn region_group(&mut self, region: (i32, i32)) {
        self.bindings_set += 1;
        self.state.region_group = Some(region);
    }

    fn vertex_slot(&mut self, source: VertexSlot) {
        self.bindings_set += 1;
        self.state.vertex_slot = Some(source);
    }

    fn face_index(&mut self) {
        self.bindings_set += 1;
        self.state.face_index = true;
    }

    fn draw_indexed(&mut self, batch: usize) {
        self.recorded.push(Recorded::Indexed { batch, state: self.state });
    }

    fn draw_instanced(&mut self, batch: usize, first: u32, count: u32) {
        self.recorded.push(Recorded::Instanced { batch, first, count, state: self.state });
    }
}

/// Test generator with a fixed seed (independent of the clock).
fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *s >> 33
}

/// Batches laid out like the draw lists: the six lists in draw order (layer, then block faces before
/// fluids), regions in the same rank order in each, the translucent fluids in one shared batch.
fn scripted_batches(s: &mut u64, regions: &[(i32, i32)]) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut first = 0;
    for (layer, fluid) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
        let homes: Vec<(i32, i32)> = if fluid && layer == 2 {
            if lcg(s).is_multiple_of(4) { Vec::new() } else { vec![TRANS_KEY] }
        } else {
            regions.iter().copied().filter(|_| !lcg(s).is_multiple_of(3)).collect()
        };
        for home in homes {
            let count = 1 + (lcg(s) % 4) as u32;
            batches.push((layer, fluid, home, first, count));
            first += count;
        }
    }
    batches
}

fn record(batches: &[Batch], which: TerrainLoop, paths: TerrainLoopPaths, missing: Option<(i32, i32)>) -> Recorder {
    let mut out = Recorder::default();
    let drawn = encode_terrain_loop(batches, which, paths, |k| Some(k) != missing, &mut out);
    assert_eq!(drawn, out.recorded.contains(&Recorded::Features));
    out
}

/// Every draw of the path that sets everything holds what its batch needs.
fn assert_draws_hold_their_bindings(batches: &[Batch], recorded: &[Recorded], indexed: bool) {
    for r in recorded {
        let (batch, state) = match r {
            Recorded::Features => continue,
            Recorded::Indexed { batch, state } | Recorded::Instanced { batch, state, .. } => (*batch, *state),
        };
        let (layer, fluid, region, ..) = batches[batch];
        let slot = if !fluid && indexed { VertexSlot::FaceIds } else { VertexSlot::instanced(region, layer, fluid) };
        assert_eq!(state.pipeline, Some((layer, fluid)), "batch {batch}");
        assert_eq!(state.region_group, Some(region), "batch {batch}");
        assert_eq!(state.vertex_slot, Some(slot), "batch {batch}");
        if !fluid && indexed {
            assert!(state.face_index, "batch {batch}: indexed draw without its indices");
        }
    }
}

/// A batch list, the loop that draws it, the face form, and a region the renderer does not know.
struct LoopCase {
    batches: Vec<Batch>,
    which: TerrainLoop,
    indexed: bool,
    missing: Option<(i32, i32)>,
}

impl LoopCase {
    fn record(&self, keep_bindings: bool, sabotage: Option<BindingSabotage>) -> Recorder {
        record(&self.batches, self.which, paths(self.indexed, keep_bindings, sabotage), self.missing)
    }
}

/// Every loop kind and face form, over scripted batch lists (fixed seed).
fn loop_cases() -> Vec<LoopCase> {
    let mut s = 0x7e44_a1d5u64;
    let regions = [(0, 0), (1, 0), (0, 1), (-1, 2), (3, -3)];
    let mut cases = Vec::new();
    for i in 0..400 {
        let mut batches = scripted_batches(&mut s, &regions[..1 + i % regions.len()]);
        // Sometimes the same region twice in a row (a list that holds a single region).
        if i % 7 == 0 {
            batches.retain(|b| b.2 == regions[0] || b.2 == TRANS_KEY);
        }
        let missing = (i % 5 == 0).then(|| regions[(lcg(&mut s) % 5) as usize]);
        for which in [
            TerrainLoop::WorldPass { improved_transparency: false },
            TerrainLoop::WorldPass { improved_transparency: true },
            TerrainLoop::TranslucentStage,
        ] {
            for indexed in [true, false] {
                cases.push(LoopCase { batches: batches.clone(), which, indexed, missing });
            }
        }
    }
    cases
}

fn paths(indexed_faces: bool, keep_bindings: bool, sabotage: Option<BindingSabotage>) -> TerrainLoopPaths {
    TerrainLoopPaths { indexed_faces, keep_bindings, sabotage }
}

/// `RUSTCRAFT_OPT_TERRAIN_BIND_STATE`: the loop that skips the bindings in place leaves every draw
/// with exactly the pipeline, region group, vertex slot and indices of the loop that sets them all
/// (each loop kind, both face forms, missing regions, features in the middle), and sets fewer;
/// each sabotage (a region buffer in slot 0 not noted, bindings kept across the features) is seen.
#[test]
fn kept_bindings_leave_every_draw_with_the_same_state() {
    let cases = loop_cases();
    let (mut before, mut after) = (0, 0);
    for (i, case) in cases.iter().enumerate() {
        let all = case.record(false, None);
        assert_draws_hold_their_bindings(&case.batches, &all.recorded, case.indexed);
        let kept = case.record(true, None);
        let LoopCase { batches, which, indexed, .. } = case;
        assert_eq!(kept.recorded, all.recorded, "case {i}: {which:?}, indexed {indexed}, {batches:?}");
        assert!(kept.bindings_set <= all.bindings_set, "case {i}");
        (before, after) = (before + all.bindings_set, after + kept.bindings_set);
    }
    assert!(after * 10 < before * 9, "too few bindings saved to test anything: {before} -> {after}");
    for sabotage in [BindingSabotage::RegionSlotNotNoted, BindingSabotage::KeptAcrossFeatures] {
        let seen = cases.iter().any(|case| case.record(true, Some(sabotage)).recorded != case.record(false, None).recorded);
        assert!(seen, "sabotage {sabotage:?} not seen");
    }
}

/// The sky drawn after the opaque terrain comes with the features (`RUSTCRAFT_OPT_SKY_AFTER_OPAQUE`,
/// first thing of `TerrainPass::features`): the classic world pass calls them once, after every
/// opaque and cutout draw and before every translucent one, when a translucent batch exists; without
/// one, and with improved transparency, never (the frame then draws the sky after the loop).
#[test]
fn features_come_between_the_cutout_and_the_translucent_terrain() {
    let classic = TerrainLoop::WorldPass { improved_transparency: false };
    let world_passes = loop_cases().into_iter().filter(|c| matches!(c.which, TerrainLoop::WorldPass { .. }));
    let mut between = 0;
    for (i, case) in world_passes.enumerate() {
        let recorded = case.record(true, None).recorded;
        let layer = |r: &Recorded| match r {
            Recorded::Indexed { batch, .. } | Recorded::Instanced { batch, .. } => Some(case.batches[*batch].0),
            Recorded::Features => None,
        };
        let calls: Vec<usize> = (0..recorded.len()).filter(|&k| recorded[k] == Recorded::Features).collect();
        let has_translucent = case.batches.iter().any(|b| b.0 == 2);
        let expected = usize::from(case.which == classic && has_translucent);
        assert_eq!(calls.len(), expected, "case {i}: {:?}", case.which);
        if let Some(&at) = calls.first() {
            assert!(recorded[..at].iter().all(|r| layer(r).is_some_and(|l| l < 2)), "case {i}: blended before");
            assert!(recorded[at + 1..].iter().all(|r| layer(r) == Some(2)), "case {i}: solid after");
            between += 1;
        }
    }
    assert!(between > 100, "too few cases with features: {between}");
}

/// Bindings set per frame by a terrain list of `regions` regions laid out like a far view (every
/// region in the opaque and cutout lists, half with fluids, a quarter with translucent faces):
/// `cargo test --release -p rc-app terrain_bindings_per_frame -- --ignored --nocapture`.
#[test]
#[ignore]
fn terrain_bindings_per_frame() {
    for regions in [1, 4, 16, 36] {
        let keys: Vec<(i32, i32)> = (0..regions).map(|i| (i % 6, i / 6)).collect();
        let mut batches = Vec::new();
        let mut first = 0;
        for (layer, fluid, share) in [(0, false, 1), (0, true, 2), (1, false, 1), (1, true, 3), (2, false, 4)] {
            for k in keys.iter().step_by(share) {
                batches.push((layer, fluid, *k, first, 3));
                first += 3;
            }
        }
        batches.push((2, true, TRANS_KEY, first, 1));
        let which = TerrainLoop::WorldPass { improved_transparency: false };
        let all = record(&batches, which, paths(true, false, None), None).bindings_set;
        let kept = record(&batches, which, paths(true, true, None), None).bindings_set;
        eprintln!("{regions} regions, {} batches: {all} bindings set -> {kept}", batches.len());
    }
}
