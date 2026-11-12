//! Proofs on the walk inputs kept by the uploads: after every event of a scripted load, the farthest
//! column and the connectivity snapshot equal what a measure over every column gives.

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
    /// What the upload tells: a column that was not loaded is added; a new or changed
    /// connectivity bumps the version and is noted.
    fn upload(&mut self, key: (i32, i32), vis: Connectivity, far: &mut FarthestColumn, store: &mut VisSnapshotStore) {
        let old = self.columns.insert(key, vis.clone());
        if old.is_none() {
            far.added(key);
        }
        if old.is_none_or(|o| o != vis) {
            self.vis_rev += 1;
            store.changed(key);
        }
    }

    fn remove(&mut self, key: (i32, i32), far: &mut FarthestColumn, store: &mut VisSnapshotStore) {
        if self.columns.remove(&key).is_some() {
            far.removed(key);
            self.vis_rev += 1;
            store.changed(key);
        }
    }

    fn clear(&mut self, far: &mut FarthestColumn, store: &mut VisSnapshotStore) {
        self.columns.clear();
        far.forget();
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

/// Outcome of a scripted load: where the kept values first differed from a measure, how often the
/// snapshot was patched in place, and the walks' snapshots that a patch changed under them.
#[derive(Debug, Default)]
struct LoadRun {
    radius_differs_at: Option<usize>,
    snapshot_differs_at: Option<usize>,
    patched_in_place: usize,
    walk_snapshots_changed: usize,
}

/// A scripted load (fixed seed): columns streaming in and out around a moving camera, columns
/// meshed again (same or other connectivity), an occasional clear; the frame measures the radius and
/// takes a snapshot every few events, sometimes while a walk still holds the previous one.
fn scripted_load(far_sabotaged: bool, snapshot_sabotaged: bool) -> LoadRun {
    let mut s = 0x9a1c_0de5u64;
    let mut far = FarthestColumn { removals_ignored: far_sabotaged, ..FarthestColumn::default() };
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
                loaded.upload(key, connectivity(lcg(&mut s) % 3), &mut far, &mut store);
            }
            45..=74 => {
                let key = (camera.0 + r(&mut s, 8), camera.1 + r(&mut s, 8));
                loaded.remove(key, &mut far, &mut store);
            }
            // The farthest column leaves (left behind), so that the radius shrinks.
            75..=89 => {
                let farthest = loaded.columns.keys().copied().max_by_key(|&k| (column_distance(k, camera), k));
                if let Some(key) = farthest {
                    loaded.remove(key, &mut far, &mut store);
                }
            }
            90..=92 => camera = (camera.0 + r(&mut s, 1), camera.1 + r(&mut s, 1)),
            _ => {}
        }
        if t % 3000 == 2999 {
            loaded.clear(&mut far, &mut store);
        }
        if t % 3 == 0 {
            let keys = || loaded.columns.keys().copied();
            if far.distance(camera, keys) != farthest_distance(camera, keys()) {
                run.radius_differs_at.get_or_insert(t);
            }
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

/// `RUSTCRAFT_OPT_INCREMENTAL_VIEW_RADIUS` and `RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT`: over a
/// scripted load, the kept radius and the patched snapshot equal a measure over every column at each
/// frame; the snapshot is patched in place most of the time, and never under a walk that still reads
/// it; a kept radius or a snapshot that ignores removals is seen.
#[test]
fn kept_walk_inputs_match_a_measure_over_every_column() {
    let run = scripted_load(false, false);
    assert_eq!(run.radius_differs_at, None, "{run:?}");
    assert_eq!(run.snapshot_differs_at, None, "{run:?}");
    assert_eq!(run.walk_snapshots_changed, 0, "{run:?}");
    assert!(run.patched_in_place > 1000, "too few snapshots patched in place: {run:?}");
    assert!(scripted_load(true, false).radius_differs_at.is_some(), "removals ignored by the radius not seen");
    assert!(scripted_load(false, true).snapshot_differs_at.is_some(), "removals ignored by the snapshot not seen");
}

/// The old path makes every snapshot from every column, as before.
#[test]
fn snapshot_store_without_the_incremental_path_remakes_from_every_column() {
    let mut store = VisSnapshotStore::new(false);
    let mut loaded = Loaded::default();
    let mut far = FarthestColumn::default();
    loaded.upload((0, 0), connectivity(1), &mut far, &mut store);
    let first = loaded.snapshot(&mut store);
    loaded.upload((1, 0), connectivity(2), &mut far, &mut store);
    assert!(store.changed.is_empty(), "the old path notes nothing");
    let second = loaded.snapshot(&mut store);
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(*second, loaded.all());
    assert_eq!(first.len(), 1);
}

/// Cost per frame of the radius and of the snapshot, measured again over every column or kept, with
/// 2,000 and 8,000 loaded columns and one column changed per frame (1 core: main thread):
/// `cargo test --release -p rc-app walk_inputs_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn walk_inputs_cost() {
    for n in [2000i32, 8000] {
        let side = (f64::from(n)).sqrt() as i32;
        let mut loaded = Loaded::default();
        let mut far = FarthestColumn::default();
        let mut stores = [VisSnapshotStore::new(false), VisSnapshotStore::new(true)];
        for i in 0..n {
            let key = (i % side - side / 2, i / side - side / 2);
            loaded.columns.insert(key, connectivity(u64::from(i as u32) % 3));
        }
        let frames = 2000;
        let (mut measured, mut kept) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        let mut snapshot_times = [std::time::Duration::ZERO; 2];
        for t in 0..frames {
            // A column meshed again on every frame (the columns' version changes, the radius of the
            // old path is measured again), the camera in a new column every 64 frames.
            let key = ((t * 7) % side - side / 2, (t * 13) % side - side / 2);
            loaded.columns.insert(key, connectivity(t as u64));
            stores.iter_mut().for_each(|store| store.changed(key));
            loaded.vis_rev += 1;
            let camera = (t / 64, 0);
            let started = std::time::Instant::now();
            std::hint::black_box(farthest_distance(camera, loaded.columns.keys().copied()));
            measured += started.elapsed();
            let started = std::time::Instant::now();
            std::hint::black_box(far.distance(camera, || loaded.columns.keys().copied()));
            kept += started.elapsed();
            for (store, time) in stores.iter_mut().zip(&mut snapshot_times) {
                let started = std::time::Instant::now();
                std::hint::black_box(loaded.snapshot(store));
                *time += started.elapsed();
            }
        }
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6 / f64::from(frames);
        eprintln!(
            "{n} columns, one changed per frame: radius {:.2} -> {:.2} µs, snapshot {:.2} -> {:.2} µs per frame",
            us(measured),
            us(kept),
            us(snapshot_times[0]),
            us(snapshot_times[1])
        );
    }
}
