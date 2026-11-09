//! Where the cloud faces are built, so that crossing a 12-block cell does not stall the frame:
//! - [`ParallelBuild`]: the disk order cut into slices on the common pool
//!   (`RUSTCRAFT_OPT_CLOUDPAR=1`, where the build waits behind the queued chunk meshes) or built
//!   on the calling thread alone (default);
//! - [`Speculation`]: a background thread builds the cell the camera will most likely enter next
//!   (`RUSTCRAFT_OPT_CLOUD_SPECULATIVE_BUILD`); the crossing then only swaps two buffers.
//!
//! The faces are the same words in the same order whatever the placement (proof:
//! `speculative_build_matches_a_direct_build`).

use std::sync::{Arc, mpsc};

use super::{CloudBuild, CloudCells, CloudKey, CloudMap, cloud_faces, cloud_par};

/// Precomputed disk order of a radius, shared with the background thread.
pub type Spiral = Arc<[[i16; 2]]>;

/// Where a cloud build runs.
pub enum ParallelBuild {
    /// The calling thread alone.
    CallingThread,
    /// Slices on the common pool (`RUSTCRAFT_OPT_CLOUDPAR=1`).
    CommonPool,
}

impl ParallelBuild {
    /// Placement chosen by the switch: the common pool, or the calling thread alone.
    pub fn new(common_pool: bool) -> ParallelBuild {
        if common_pool { ParallelBuild::CommonPool } else { ParallelBuild::CallingThread }
    }

    /// `build` of `key` into `out`, cells in the order of `spiral` (the disk order of its radius)
    /// or of the square sweep; in slices of the disk order (`parts` keeps their reserves) when the
    /// placement is parallel. Same words, same order everywhere.
    pub fn run<T: Copy + Send + 'static>(
        &self,
        build: CloudBuild<T>,
        map: &Arc<CloudMap>,
        key: CloudKey,
        spiral: Option<&Spiral>,
        (out, parts): (&mut Vec<T>, &mut Vec<Vec<T>>),
    ) {
        match (self, spiral) {
            (ParallelBuild::CommonPool, Some(s)) => {
                cloud_par(build, map, (key.cx, key.cz), s, key.side, key.flat, (out, parts));
            }
            _ => build(map, key.cx, key.cz, cells_of(key, spiral), key.side, key.flat, out),
        }
    }
}

/// Cells of `key` in the order of the precomputed `spiral` of its radius, or of the square sweep.
fn cells_of(key: CloudKey, spiral: Option<&Spiral>) -> CloudCells<'_> {
    spiral.map_or(CloudCells::Scan(key.radius), |s| CloudCells::Spiral(s))
}

/// Cell the camera enters next: the neighbour across the edge it reaches first, moving by
/// `motion` blocks from `inside` (its position in its 12-block cell). Standing still, the clouds
/// drift towards −x, so the camera moves towards +x relative to them. Wrapped to the map.
pub fn next_cell((cx, cz): (i32, i32), inside: (f64, f64), motion: (f64, f64), map_size: i32) -> (i32, i32) {
    let time_to_edge = |pos: f64, speed: f64| {
        if speed > 0.0 {
            (12.0 - pos) / speed
        } else if speed < 0.0 {
            pos / -speed
        } else {
            f64::INFINITY
        }
    };
    let (to_x, to_z) = (time_to_edge(inside.0, motion.0), time_to_edge(inside.1, motion.1));
    let (nx, nz) = if to_z < to_x {
        (cx, cz + motion.1.signum() as i32)
    } else if to_x.is_finite() {
        (cx + motion.0.signum() as i32, cz)
    } else {
        (cx + 1, cz)
    };
    (nx.rem_euclid(map_size), nz.rem_euclid(map_size))
}

/// One build handed to the background thread and back.
struct Job {
    key: CloudKey,
    /// Disk order of `key.radius`, `None` for the square sweep.
    spiral: Option<Spiral>,
    /// Output buffer (its reserve travels with it).
    faces: Vec<u32>,
}

impl Job {
    fn build(&mut self, map: &CloudMap) {
        let key = self.key;
        cloud_faces(map, key.cx, key.cz, cells_of(key, self.spiral.as_ref()), key.side, key.flat, &mut self.faces);
    }
}

/// Background thread that builds the faces of the predicted next cell, one job at a time.
pub struct Speculation {
    jobs: mpsc::Sender<Job>,
    finished: mpsc::Receiver<Job>,
    /// Key being built on the background thread.
    building: Option<CloudKey>,
    /// Last finished job, not used yet.
    done: Option<Job>,
    /// Next key to build as soon as the thread is free.
    wanted: Option<(CloudKey, Option<Spiral>)>,
    /// Buffer for the next job (the one a hit swapped out).
    spare: Vec<u32>,
}

impl Speculation {
    /// Starts the background thread (`None` if the system refuses it).
    pub fn start(map: Arc<CloudMap>) -> Option<Speculation> {
        let (jobs, todo) = mpsc::channel::<Job>();
        let (finish, finished) = mpsc::channel();
        let worker = move || {
            for mut job in todo {
                job.build(&map);
                if finish.send(job).is_err() {
                    return;
                }
            }
        };
        let started = std::thread::Builder::new().name("cloud speculation".into()).spawn(worker);
        if let Err(e) = started {
            eprintln!("[opt] cloud speculation not started: {e}");
            return None;
        }
        Some(Speculation { jobs, finished, building: None, done: None, wanted: None, spare: Vec::new() })
    }

    /// Faces of `key` if they were built ahead. When the background thread is building exactly
    /// `key`, waits for it: it started earlier, so it ends sooner than a build from scratch.
    pub fn take(&mut self, key: CloudKey) -> Option<Vec<u32>> {
        // The camera crossed: the prediction made at the previous crossing is stale.
        self.wanted = None;
        if self.building == Some(key) {
            match self.finished.recv() {
                Ok(job) => self.store(job),
                Err(_) => self.building = None,
            }
        } else {
            self.collect();
        }
        match self.done.take() {
            Some(job) if job.key == key => Some(job.faces),
            other => {
                self.done = other;
                None
            }
        }
    }

    /// Asks for `key` to be built ahead, as soon as the background thread is free.
    pub fn prepare(&mut self, key: CloudKey, spiral: Option<Spiral>) {
        self.wanted = Some((key, spiral));
        self.poll();
    }

    /// Collects a finished build and hands over the wanted one (once per frame: one channel
    /// check while a build runs).
    pub fn poll(&mut self) {
        self.collect();
        if self.building.is_some() {
            return;
        }
        let Some((key, spiral)) = self.wanted.take() else { return };
        if self.done.as_ref().is_some_and(|job| job.key == key) {
            return;
        }
        // The unused build (a wrong prediction) lends its buffer to the next one.
        let faces = self.done.take().map_or_else(|| std::mem::take(&mut self.spare), |job| job.faces);
        if self.jobs.send(Job { key, spiral, faces }).is_ok() {
            self.building = Some(key);
        }
    }

    /// Gives back a buffer for the next build (the largest reserve is kept).
    pub fn recycle(&mut self, faces: Vec<u32>) {
        if faces.capacity() > self.spare.capacity() {
            self.spare = faces;
        }
    }

    fn collect(&mut self) {
        if self.building.is_none() {
            return;
        }
        match self.finished.try_recv() {
            Ok(job) => self.store(job),
            Err(mpsc::TryRecvError::Empty) => {}
            // Thread gone: no more speculation, every crossing builds in place.
            Err(mpsc::TryRecvError::Disconnected) => self.building = None,
        }
    }

    fn store(&mut self, job: Job) {
        self.building = None;
        if let Some(old) = self.done.replace(job) {
            self.recycle(old.faces);
        }
    }
}

/// Builder of the cloud faces (`face` format): ahead of time by the speculation when it guessed
/// the cell, otherwise in place by the chosen placement.
pub struct FaceBuilder {
    /// Faces of the last build (reserve kept).
    pub faces: Vec<u32>,
    /// Slices of the parallel build (reserves kept).
    parts: Vec<Vec<u32>>,
    speculation: Option<Speculation>,
}

impl FaceBuilder {
    pub fn new(speculation: Option<Speculation>) -> FaceBuilder {
        FaceBuilder { faces: Vec::new(), parts: Vec::new(), speculation }
    }

    /// Faces of `key` into [`Self::faces`] (cells in the order of `spiral`, the disk order of its
    /// radius, or of the square sweep), then `next` asked ahead. True if the speculation had them.
    pub fn build(
        &mut self,
        map: &Arc<CloudMap>,
        key: CloudKey,
        spiral: Option<&Spiral>,
        next: CloudKey,
        placement: &ParallelBuild,
    ) -> bool {
        let ahead = self.speculation.as_mut().and_then(|s| s.take(key));
        let hit = ahead.is_some();
        match ahead {
            Some(faces) => {
                let old = std::mem::replace(&mut self.faces, faces);
                if let Some(s) = &mut self.speculation {
                    s.recycle(old);
                }
            }
            None => placement.run(cloud_faces, map, key, spiral, (&mut self.faces, &mut self.parts)),
        }
        if let Some(s) = &mut self.speculation {
            s.prepare(next, spiral.cloned());
        }
        hit
    }

    /// Hands the wanted build to the background thread when it becomes free (once per frame).
    pub fn poll(&mut self) {
        if let Some(s) = &mut self.speculation {
            s.poll();
        }
    }
}

#[cfg(test)]
impl Speculation {
    /// Tests: waits until the background thread has built what it was last asked for, so that the
    /// hits no longer depend on the speed of the machine.
    fn settle(&mut self) {
        loop {
            if self.building.is_some() {
                match self.finished.recv() {
                    Ok(job) => self.store(job),
                    Err(_) => self.building = None,
                }
            }
            self.poll();
            if self.building.is_none() {
                return;
            }
        }
    }
}

#[cfg(test)]
impl FaceBuilder {
    /// Tests: [`Speculation::settle`].
    fn settle(&mut self) {
        if let Some(s) = &mut self.speculation {
            s.settle();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_map;
    use super::super::{CloudSide, cloud_spiral};
    use super::*;

    const SIDES: [CloudSide; 3] = [CloudSide::Above, CloudSide::Inside, CloudSide::Below];

    fn xorshift(s: &mut u64) -> u64 {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        *s
    }

    /// 1,000 crossings: mostly into the predicted cell, the others into another neighbour, a
    /// distant cell, or with a change of side, radius or "fast" mode that makes the build ahead
    /// stale. Every buffer taken at a crossing is, word for word, the direct build of its key; the
    /// speculation was really used, and missed too; on the common pool and on the calling
    /// thread, by disk order and by sweep.
    #[test]
    fn speculative_build_matches_a_direct_build() {
        let map = Arc::new(test_map());
        let radii = [3, 20, 40];
        let spirals: Vec<Spiral> = radii.iter().map(|&r| Spiral::from(cloud_spiral(r))).collect();
        for (placement, use_spiral) in [(ParallelBuild::CommonPool, true), (ParallelBuild::CallingThread, false)] {
            let mut builder = FaceBuilder::new(Speculation::start(Arc::clone(&map)));
            let mut seed = 0x5eed_c10d_5eed_c10du64;
            let mut key = CloudKey { cx: 10, cz: 20, side: CloudSide::Below, radius: 20, flat: false };
            let predict = |k: CloudKey| CloudKey { cx: (k.cx + 1).rem_euclid(64), ..k };
            let (mut hits, mut stale_changes, mut want) = (0, 0, Vec::new());
            for n in 0..1000 {
                let r = xorshift(&mut seed);
                let next = predict(key);
                let new_key = match r % 16 {
                    0 => CloudKey { side: SIDES[(r >> 8) as usize % 3], ..next },
                    1 => CloudKey { radius: radii[(r >> 8) as usize % 3], ..next },
                    2 => CloudKey { flat: !key.flat, ..next },
                    3 => CloudKey { cz: (key.cz - 1).rem_euclid(64), ..key },
                    4 => CloudKey { cx: (r >> 8) as i32 % 64, cz: (r >> 16) as i32 % 64, ..key },
                    _ => next,
                };
                if new_key.side != key.side || new_key.radius != key.radius || new_key.flat != key.flat {
                    stale_changes += 1;
                }
                let spiral = use_spiral.then(|| &spirals[radii.iter().position(|&r| r == new_key.radius).unwrap()]);
                // The prediction made at the previous crossing is ready, whatever the machine's speed
                // (a slow CI runner otherwise misses most of them).
                builder.settle();
                hits += usize::from(builder.build(&map, new_key, spiral, predict(new_key), &placement));
                let cells = cells_of(new_key, spiral);
                cloud_faces(&map, new_key.cx, new_key.cz, cells, new_key.side, new_key.flat, &mut want);
                assert!(builder.faces == want, "crossing {n} {new_key:?}: buffer differs from the direct build");
                key = new_key;
            }
            assert!(hits > 500, "speculation used {hits} times out of 1000");
            assert!(hits < 900, "speculation never missed ({hits} hits): the check proves nothing on misses");
            assert!(stale_changes > 100, "{stale_changes} changes of side, radius or mode");
        }
    }

    /// The comparison above is sensitive: the faces of the predicted cell (what a stale buffer
    /// would hold) differ from those of the current one at almost every crossing.
    #[test]
    fn neighbour_cells_have_different_faces() {
        let map = test_map();
        let spiral = cloud_spiral(20);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut same = 0;
        for cx in 0..64 {
            for side in SIDES {
                cloud_faces(&map, cx, 7, CloudCells::Spiral(&spiral), side, false, &mut a);
                cloud_faces(&map, cx + 1, 7, CloudCells::Spiral(&spiral), side, false, &mut b);
                same += usize::from(a == b);
            }
        }
        assert_eq!(same, 0, "neighbour cells with identical faces");
    }

    /// The prediction: the edge reached first along the motion, the clouds' drift when standing
    /// still, wrapped to the map.
    #[test]
    fn next_cell_follows_the_motion() {
        // Standing still: drift towards +x.
        assert_eq!(next_cell((5, 5), (6.0, 6.0), (0.0, 0.0), 64), (6, 5));
        // Drift alone (a small +x motion) and a fast flight towards −z.
        assert_eq!(next_cell((5, 5), (6.0, 6.0), (0.001, 0.0), 64), (6, 5));
        assert_eq!(next_cell((5, 5), (6.0, 6.0), (0.001, -0.5), 64), (5, 4));
        // Diagonal: the nearer edge wins (2 blocks to the +x edge, 9 to the +z edge).
        assert_eq!(next_cell((5, 5), (10.0, 3.0), (1.0, 1.0), 64), (6, 5));
        assert_eq!(next_cell((5, 5), (3.0, 10.0), (1.0, 1.0), 64), (5, 6));
        assert_eq!(next_cell((5, 5), (3.0, 10.0), (-1.0, 0.1), 64), (4, 5));
        // Wrapped at the edges of the map.
        assert_eq!(next_cell((63, 0), (11.0, 1.0), (1.0, -0.01), 64), (0, 0));
        assert_eq!(next_cell((63, 0), (11.0, 0.5), (0.1, -1.0), 64), (63, 63));
    }
}
