//! Visibility filter of the meshing (`RUSTCRAFT_OPT_MESHVISIBLEONLY`) on a model of the client,
//! with its sabotages, and early release of the task's columns (`RUSTCRAFT_OPT_MESHDROPJOBEARLY`).

use super::*;
use hashbrown::HashSet;

/// Sections of a model column.
const SECTIONS: usize = 8;
/// Fixed seed of the trials (no dependence on the clock).
const SEED: u64 = 0x5eed_c204;

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// What decides the launch of a column: marks, sections in view (`None`: column not drawn),
/// kept base, priority remesh. The real one is [`sections_to_launch`].
type Policy = fn(Sections, Option<Sections>, bool, bool) -> Option<Sections>;

/// The old path: every dirty column, with all its marks.
fn launch_everything(dirty: Sections, _: Option<Sections>, _: bool, _: bool) -> Option<Sections> {
    Some(dirty)
}

/// Sabotage: a column never drawn is held like a hidden one (its view taken as empty).
fn hold_never_meshed(dirty: Sections, in_view: Option<Sections>, has_base: bool, urgent: bool) -> Option<Sections> {
    sections_to_launch(dirty, Some(in_view.unwrap_or_default()), has_base, urgent)
}

fn sections(list: &[i32]) -> Sections {
    let mut s = Sections::default();
    list.iter().for_each(|&i| s.insert(i));
    s
}

fn all_sections() -> Sections {
    sections(&(0..SECTIONS as i32).collect::<Vec<_>>())
}

/// Camera of the model at frame `f`: a quarter of the sections of each column in view, some
/// columns wholly behind the camera; the view turns every 3 frames.
fn view_of_frame(f: u32, (x, z): (i32, i32)) -> Sections {
    let mut h = (u64::from(f / 3) << 32) ^ (u64::from(x as u16) << 16) ^ u64::from(z as u16);
    // Finaliser of splitmix64: an even spread of the visible sections.
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    let mut seen = Sections::default();
    if h % 5 != 0 {
        for s in 0..SECTIONS {
            if (h >> (8 + 4 * s)) & 3 == 0 {
                seen.insert(s as i32);
            }
        }
    }
    seen
}

/// Model of the client: contents per section of the world and of the meshes drawn (a mesh copies
/// the contents at its launch), evicted bases, marks. Tasks are done at once and without a slot
/// limit, like a pool that keeps up.
#[derive(Default)]
struct Model {
    world: HashMap<(i32, i32), [u32; SECTIONS]>,
    drawn: HashMap<(i32, i32), [u32; SECTIONS]>,
    evicted: HashSet<(i32, i32)>,
    dirty: HashMap<(i32, i32), Sections>,
    /// Marks left at the end of the previous frame (sections held).
    carried: HashMap<(i32, i32), Sections>,
    /// Sections meshed, of which held for at least one frame first; frames ended with marks held.
    meshed: usize,
    late: usize,
    held_frames: usize,
}

impl Model {
    fn load(&mut self, key: (i32, i32)) {
        self.world.insert(key, [1; SECTIONS]);
        self.dirty.insert(key, Sections::ALL);
    }

    fn change(&mut self, key: (i32, i32), s: usize) {
        if let Some(c) = self.world.get_mut(&key) {
            c[s] += 1;
            self.dirty.entry(key).or_default().insert(s as i32);
        }
    }

    /// Column received again (all its sections changed): redone whole.
    fn replace(&mut self, key: (i32, i32)) {
        if let Some(c) = self.world.get_mut(&key) {
            c.iter_mut().for_each(|v| *v += 1);
            self.dirty.insert(key, Sections::ALL);
        }
    }

    /// One frame: launches chosen by `policy` from the view `policy_view`, meshes made, then the
    /// frame drawn checked against the view `must_be_fresh`.
    fn frame(
        &mut self,
        policy: Policy,
        policy_view: impl Fn((i32, i32)) -> Sections,
        must_be_fresh: impl Fn((i32, i32)) -> Sections,
        spare: bool,
    ) -> Result<(), String> {
        let Model { drawn, evicted, dirty, .. } = self;
        let in_view = |k: (i32, i32)| drawn.contains_key(&k).then(|| policy_view(k));
        let launchable = |k, marks| policy(marks, in_view(k), !evicted.contains(&k), urgent(k));
        let launches = pick_columns(dirty, usize::MAX, (0, 0), |_, _| true, launchable, spare);
        for (key, launch) in launches {
            self.mesh(key, launch);
        }
        self.check(must_be_fresh)?;
        self.held_frames += usize::from(!self.dirty.is_empty());
        self.carried = self.dirty.clone();
        Ok(())
    }

    /// Mesh of the `launch` sections of column `key`: all of them for a whole remesh or without a
    /// kept base (which comes back).
    fn mesh(&mut self, key: (i32, i32), launch: Sections) {
        let had_base = !self.evicted.remove(&key);
        let whole = launch.is_all() || !had_base;
        let world = self.world[&key];
        let carried = self.carried.get(&key).copied().unwrap_or_default();
        let mesh = self.drawn.entry(key).or_insert([0; SECTIONS]);
        for s in (0..SECTIONS).filter(|&s| whole || launch.contains(s)) {
            mesh[s] = world[s];
            self.meshed += 1;
            self.late += usize::from(carried.contains(s));
        }
    }

    /// Every loaded column has a mesh; the sections `must_be_fresh` of each, and all those of a
    /// priority column, are up to date.
    fn check(&self, must_be_fresh: impl Fn((i32, i32)) -> Sections) -> Result<(), String> {
        for (&key, world) in &self.world {
            let Some(mesh) = self.drawn.get(&key) else { return Err(format!("column {key:?} never meshed: a hole")) };
            let fresh = if urgent(key) { all_sections() } else { must_be_fresh(key) };
            if let Some(s) = (0..SECTIONS).find(|&s| fresh.contains(s) && mesh[s] != world[s]) {
                return Err(format!("column {key:?} section {s} drawn stale"));
            }
        }
        Ok(())
    }
}

/// Priority remesh of the model: the camera's column.
fn urgent(key: (i32, i32)) -> bool {
    key == (0, 0)
}

/// What a trajectory gives: sections meshed, of which held first; frames ended with marks held;
/// final meshes.
type Outcome = (usize, usize, usize, HashMap<(i32, i32), [u32; SECTIONS]>);

/// 400 frames: block changes, columns received again, bases evicted, new columns arriving. The
/// policy sees the view of `lag` frames before (the real meshing reads the last frame drawn: 1);
/// a section in view on two frames in a row must be fresh on the second (launched at the latest
/// one frame after it shows, as the game whose rebuilds run in the background). Then everything
/// comes into view: the meshes must be the world's.
fn trajectory(policy: Policy, lag: u32, spare: bool) -> Result<Outcome, String> {
    let mut m = Model::default();
    let mut rng = Rng(SEED);
    for x in 0..6 {
        for z in 0..4 {
            m.load((x, z));
        }
    }
    let key = |rng: &mut Rng| (rng.below(6) as i32, rng.below(6) as i32);
    for f in 0..400u32 {
        // New columns: never drawn, so outside the walk.
        if f % 10 == 5 && f < 120 {
            m.load((f as i32 / 10 % 6, 4 + f as i32 / 60));
        }
        for _ in 0..5 {
            let k = key(&mut rng);
            m.change(k, rng.below(SECTIONS as u64) as usize);
        }
        if f % 20 == 7 {
            m.replace(key(&mut rng));
        }
        if f % 7 == 3 {
            m.evicted.insert(key(&mut rng));
        }
        let policy_view = |k| view_of_frame(f.saturating_sub(lag), k);
        let fresh = |k| view_of_frame(f, k).intersection(view_of_frame(f.saturating_sub(1), k));
        m.frame(policy, policy_view, fresh, spare).map_err(|e| format!("frame {f}: {e}"))?;
    }
    m.frame(policy, |_| all_sections(), |_| all_sections(), spare).map_err(|e| format!("all in view: {e}"))?;
    Ok((m.meshed, m.late, m.held_frames, m.drawn))
}

/// The filter holds sections out of view and meshes them once they show; nothing drawn twice in a
/// row is stale, no column is left unmeshed, priority columns are never held, and once everything
/// is in view the meshes are those of the old path. Work of both paths stored (fixed seed).
#[test]
fn filter_meshes_what_comes_into_view_and_ends_like_the_old_path() {
    let (old_work, old_late, _, old_drawn) = trajectory(launch_everything, 1, false).expect("old path");
    let (new_work, new_late, held, new_drawn) = trajectory(sections_to_launch, 1, false).expect("filter");
    assert_eq!(old_late, 0, "the old path holds nothing");
    assert!(new_late > 100 && held > 300, "the trajectory must hold sections then show them ({new_late}, {held})");
    assert_eq!(new_drawn, old_drawn, "final meshes differ from the old path");
    assert_eq!((old_work, new_work, new_late), (EXPECTED_OLD_WORK, EXPECTED_NEW_WORK, EXPECTED_LATE));
    let (spare_work, ..) = trajectory(sections_to_launch, 1, true).expect("filter with the spare slots");
    assert!(spare_work >= new_work, "the cautious variant does at least the filter's work");
}

const EXPECTED_OLD_WORK: usize = 2667;
const EXPECTED_NEW_WORK: usize = 2339;
const EXPECTED_LATE: usize = 1228;

/// The model catches a filter that holds the columns never drawn (the walk cannot reach them: a
/// hole at loading) and a filter that reads a view three frames old (stale sections drawn).
#[test]
fn filter_sabotages_are_caught() {
    let hole = trajectory(hold_never_meshed, 1, false).expect_err("never meshed columns held");
    assert!(hole.contains("hole"), "{hole}");
    let stale = trajectory(sections_to_launch, 3, false).expect_err("view three frames late");
    assert!(stale.contains("stale"), "{stale}");
}

/// Each case of the launch rule.
#[test]
fn sections_to_launch_cases() {
    let (marks, view) = (sections(&[1, 2, 5]), sections(&[2, 3]));
    assert_eq!(sections_to_launch(marks, None, true, false), Some(marks), "never drawn: launched");
    assert_eq!(sections_to_launch(marks, Some(view), true, true), Some(marks), "priority: never held");
    assert_eq!(sections_to_launch(marks, Some(sections(&[0, 7])), true, false), None, "out of view: held");
    assert_eq!(sections_to_launch(marks, Some(view), true, false), Some(sections(&[2])), "only what shows");
    assert_eq!(sections_to_launch(marks, Some(view), false, false), Some(marks), "no base: the whole column");
    assert_eq!(sections_to_launch(Sections::ALL, Some(view), true, false), Some(Sections::ALL), "whole");
    assert_eq!(sections_to_launch(Sections::ALL, Some(Sections::default()), true, false), None, "whole, hidden");
}

/// With few free slots, the launches in view go first, the closest first; the held columns keep
/// their marks and take no slot, unless the cautious variant gives them the spare ones; a column
/// launched in part keeps its other marks.
#[test]
fn pick_columns_serves_what_is_in_view_first() {
    let mut dirty = HashMap::new();
    for x in 0..6 {
        dirty.insert((x, 0), sections(&[0, 1]));
    }
    // Columns 3 to 5 in view, column 3 for section 1 only.
    let launchable = |(x, _): (i32, i32), marks: Sections| match x {
        3 => Some(sections(&[1])),
        4 | 5 => Some(marks),
        _ => None,
    };
    let keys = |v: &[Launch]| v.iter().map(|l| l.0.0).collect::<Vec<_>>();
    let mut d = dirty.clone();
    let picked = pick_columns(&mut d, 2, (0, 0), |_, _| true, launchable, false);
    assert_eq!(keys(&picked), [3, 4]);
    assert_eq!(d.get(&(3, 0)), Some(&sections(&[0])), "column 3 keeps its mark out of view");
    assert!(!d.contains_key(&(4, 0)) && d.contains_key(&(5, 0)) && d.contains_key(&(0, 0)));
    let mut d = dirty.clone();
    let picked = pick_columns(&mut d, 5, (0, 0), |_, _| true, launchable, true);
    assert_eq!(keys(&picked), [3, 4, 5, 0, 1], "spare slots to the closest held columns");
    assert_eq!(picked[3].1, sections(&[0, 1]), "a held column goes with all its marks");
    let mut d = dirty.clone();
    let picked = pick_columns(&mut d, 2, (0, 0), |_, _| true, |_, m| Some(m), false);
    assert_eq!(keys(&picked), [0, 1], "old path: the closest");
    let mut d = dirty;
    let picked = pick_columns(&mut d, 6, (0, 0), |(x, _), _| x != 4, launchable, false);
    assert_eq!(keys(&picked), [3, 5], "a column in progress waits");
}

/// The held-columns gauge of the perf log stays current with a full queue: the pick still runs,
/// launches nothing and leaves every mark; without the perf log, a full queue skips it.
#[test]
fn full_queue_still_counts_the_held_columns() {
    assert!(pick_this_frame(0, 5, true), "full queue, perf log on: the gauge would go stale");
    assert!(pick_this_frame(0, 0, true), "nothing dirty: the gauge falls to zero");
    assert!(!pick_this_frame(0, 5, false) && !pick_this_frame(3, 0, false));
    assert!(pick_this_frame(3, 5, false));
    let mut dirty: HashMap<(i32, i32), Sections> = (0..4).map(|x| ((x, 0), sections(&[1]))).collect();
    let before = dirty.clone();
    let picked = pick_columns(&mut dirty, 0, (0, 0), |_, _| true, |(x, _), m| (x < 2).then_some(m), true);
    assert!(picked.is_empty() && dirty == before, "a full queue launches nothing");
}

/// The early release hands over the same base and lets the job's nine columns go before the end
/// of the task; the old path keeps them in the job until the result is sent.
#[test]
fn early_release_lets_the_columns_go_before_the_end_of_the_task() {
    let column = |x, z| {
        Arc::new(rc_world::Column {
            x,
            z,
            sections: vec![rc_world::Section::empty(0, 0); 2].into_boxed_slice(),
            sky_light: vec![None; 4].into_boxed_slice(),
            block_light: vec![None; 4].into_boxed_slice(),
            heightmaps: Vec::new(),
        })
    };
    let center = column(0, 0);
    let around: HashMap<(i32, i32), Arc<rc_world::Column>> =
        rc_mesh::wire::NEIGHBORS.iter().map(|&k| (k, column(k.0, k.1))).collect();
    let base: blocks::Packed = Arc::new(vec![1u32, 2, 3]);
    let marks = sections(&[1]);
    for release in [false, true] {
        let mut job = MeshJob::gather(center.clone(), 7, |x, z| around.get(&(x, z)).cloned());
        job.reuse = Some((marks, base.clone()));
        let (reuse, kept) = take_base(job, release);
        let (m, w) = reuse.expect("base handed over");
        assert!(m == marks && Arc::ptr_eq(&w, &base), "same base on both paths");
        // References beyond the test's own: the job's, if it still lives.
        let held = usize::from(!release);
        assert_eq!(Arc::strong_count(&center) - 1, held, "centre (release {release})");
        assert!(around.values().all(|c| Arc::strong_count(c) - 1 == held), "neighbours (release {release})");
        assert_eq!(kept.is_some(), !release);
    }
}
