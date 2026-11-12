//! Proofs on the walk inputs kept by the uploads: after every event of a scripted load, the
//! connectivity snapshot equals what a measure over every column gives.

use super::*;

/// Test generator with a fixed seed (independent of the clock).
fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *s >> 33
}

/// Connectivity of a column: 24 sections, one of a few patterns.
fn connectivity(pattern: u64) -> Connectivity {
    (0..24).map(|i| pattern.wrapping_mul(0x9e37_79b9).rotate_left(i) & ((1 << 36) - 1)).collect()
}

type ConnectivityMap = HashMap<(i32, i32), Connectivity>;

/// Loaded columns (with a mesh or empty: the walk reads both) as the renderer's hooks see them.
#[derive(Default)]
struct Loaded {
    columns: ConnectivityMap,
    vis_rev: u64,
}

impl Loaded {
    /// What the upload tells: a new or changed connectivity bumps the version and is noted.
    fn upload(&mut self, key: (i32, i32), vis: Connectivity, store: &mut VisSnapshotStore) {
        let old = self.columns.insert(key, vis.clone());
        if old.is_none_or(|o| o != vis) {
            self.vis_rev += 1;
            store.changed(key);
        }
    }

    fn remove(&mut self, key: (i32, i32), store: &mut VisSnapshotStore) {
        if self.columns.remove(&key).is_some() {
            self.vis_rev += 1;
            store.changed(key);
        }
    }

    fn clear(&mut self, store: &mut VisSnapshotStore) {
        self.columns.clear();
        store.forget();
        self.vis_rev += 1;
    }

    fn all(&self) -> ConnectivityMap {
        self.columns.clone()
    }

    /// The snapshot the frame hands to a walk.
    fn snapshot(&self, store: &mut VisSnapshotStore) -> VisSnapshot {
        let connectivity = |k| self.columns.get(&k).cloned();
        store.current(self.vis_rev).unwrap_or_else(|| store.remake(self.vis_rev, connectivity, || self.all()))
    }
}

/// Outcome of a scripted load: where the kept snapshot first differed from a measure, how often it
/// was patched in place, and the walks' snapshots that a patch changed under them.
#[derive(Debug, Default)]
struct LoadRun {
    snapshot_differs_at: Option<usize>,
    patched_in_place: usize,
    walk_snapshots_changed: usize,
}

/// A scripted load (fixed seed): columns streaming in and out around a moving camera, columns
/// meshed again (same or other connectivity), an occasional clear; the frame takes a snapshot every
/// few events, sometimes while a walk still holds the previous one.
fn scripted_load(snapshot_sabotaged: bool) -> LoadRun {
    let mut s = 0x9a1c_0de5u64;
    let mut store = VisSnapshotStore::new(true);
    store.removals_ignored = snapshot_sabotaged;
    let mut loaded = Loaded::default();
    let (mut camera, mut run) = ((0, 0), LoadRun::default());
    // A walk still running: its snapshot, and a copy of what it held when it started.
    let mut running_walk: Option<(VisSnapshot, ConnectivityMap)> = None;
    for t in 0..20_000 {
        let r = |s: &mut u64, k: u64| (lcg(s) % (2 * k + 1)) as i32 - k as i32;
        match lcg(&mut s) % 100 {
            0..=44 => {
                let key = (camera.0 + r(&mut s, 8), camera.1 + r(&mut s, 8));
                loaded.upload(key, connectivity(lcg(&mut s) % 3), &mut store);
            }
            45..=89 => {
                let key = (camera.0 + r(&mut s, 8), camera.1 + r(&mut s, 8));
                loaded.remove(key, &mut store);
            }
            90..=92 => camera = (camera.0 + r(&mut s, 1), camera.1 + r(&mut s, 1)),
            _ => {}
        }
        if t % 3000 == 2999 {
            loaded.clear(&mut store);
        }
        if t % 5 == 0 {
            // The walk launched last time ends now, or (one time in four) is still running.
            if let Some((held, seen)) = running_walk.take()
                && *held != seen
            {
                run.walk_snapshots_changed += 1;
            }
            let stale = store.current(loaded.vis_rev).is_none();
            let before = store.held.as_ref().map(|(_, s)| Arc::as_ptr(s));
            let snapshot = loaded.snapshot(&mut store);
            if stale && before == Some(Arc::as_ptr(&snapshot)) {
                run.patched_in_place += 1;
            }
            if *snapshot != loaded.all() {
                run.snapshot_differs_at.get_or_insert(t);
            }
            if lcg(&mut s).is_multiple_of(4) {
                let seen = (*snapshot).clone();
                running_walk = Some((snapshot, seen));
            }
        }
    }
    run
}

/// `RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT`: over a scripted load, the patched snapshot equals a
/// measure over every column at each frame; it is patched in place most of the time, and never
/// under a walk that still reads it; a snapshot that ignores removals is seen.
#[test]
fn kept_walk_inputs_match_a_measure_over_every_column() {
    let run = scripted_load(false);
    assert_eq!(run.snapshot_differs_at, None, "{run:?}");
    assert_eq!(run.walk_snapshots_changed, 0, "{run:?}");
    assert!(run.patched_in_place > 1000, "too few snapshots patched in place: {run:?}");
    assert!(scripted_load(true).snapshot_differs_at.is_some(), "removals ignored by the snapshot not seen");
}

/// The old path makes every snapshot from every column, as before.
#[test]
fn snapshot_store_without_the_incremental_path_remakes_from_every_column() {
    let mut store = VisSnapshotStore::new(false);
    let mut loaded = Loaded::default();
    loaded.upload((0, 0), connectivity(1), &mut store);
    let first = loaded.snapshot(&mut store);
    loaded.upload((1, 0), connectivity(2), &mut store);
    assert!(store.changed.is_empty(), "the old path notes nothing");
    let second = loaded.snapshot(&mut store);
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(*second, loaded.all());
    assert_eq!(first.len(), 1);
}

/// Cost per frame of the snapshot, made again from every column or patched, with 2,000 and 8,000
/// loaded columns and one column changed per frame (1 core: main thread):
/// `cargo test --release -p rc-app walk_inputs_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn walk_inputs_cost() {
    for n in [2000i32, 8000] {
        let side = (f64::from(n)).sqrt() as i32;
        let mut loaded = Loaded::default();
        let mut stores = [VisSnapshotStore::new(false), VisSnapshotStore::new(true)];
        for i in 0..n {
            let key = (i % side - side / 2, i / side - side / 2);
            loaded.columns.insert(key, connectivity(u64::from(i as u32) % 3));
        }
        let frames = 2000;
        let mut snapshot_times = [std::time::Duration::ZERO; 2];
        for t in 0..frames {
            // A column meshed again on every frame.
            let key = ((t * 7) % side - side / 2, (t * 13) % side - side / 2);
            loaded.columns.insert(key, connectivity(t as u64));
            stores.iter_mut().for_each(|store| store.changed(key));
            loaded.vis_rev += 1;
            for (store, time) in stores.iter_mut().zip(&mut snapshot_times) {
                let started = std::time::Instant::now();
                std::hint::black_box(loaded.snapshot(store));
                *time += started.elapsed();
            }
        }
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6 / f64::from(frames);
        eprintln!(
            "{n} columns, one changed per frame: snapshot {:.2} -> {:.2} µs per frame",
            us(snapshot_times[0]),
            us(snapshot_times[1])
        );
    }
}
