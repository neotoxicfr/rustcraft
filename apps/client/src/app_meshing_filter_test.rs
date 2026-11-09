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

/// Sections the model's walk number `walk` reaches in column `(x, z)`: about half, none in one
/// column out of six (a column the walk does not enter).
fn reached_by_walk(walk: u32, (x, z): (i32, i32)) -> Sections {
    let mut h = (u64::from(walk) << 40) ^ (u64::from(x as u16) << 16) ^ u64::from(z as u16) ^ 0x9e37_79b9;
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    let mut reached = Sections::default();
    if h % 6 != 0 {
        (0..SECTIONS).filter(|s| (h >> (8 + s)) & 1 == 1).for_each(|s| reached.insert(s as i32));
    }
    reached
}

/// What the pick of a frame sees: the number of its view (the same number, the same view) and of
/// its walk, the sections the walk reached and those in the frustum, per column. A drawn column's
/// sections in view are both.
struct Sight<'a> {
    view_id: u32,
    walk_id: u32,
    reached: &'a dyn Fn((i32, i32)) -> Sections,
    field: &'a dyn Fn((i32, i32)) -> Sections,
}

impl Sight<'_> {
    fn in_view(&self, key: (i32, i32)) -> Sections {
        (self.reached)(key).intersection((self.field)(key))
    }
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
    /// Marks made so far (the world's touch counter of the client).
    marks_rev: u64,
    /// What the last pick that held every dirty column saw (`HeldSignature` of the client: view,
    /// walk, marks, columns drawn), and the frames that skipped their pick for it.
    all_held: Option<(u32, u32, u64, usize)>,
    skipped: usize,
    /// The client's candidate list, and the columns the picks tested against the view.
    candidates: MeshCandidates,
    tested: std::cell::Cell<usize>,
}

/// Inputs of the held verdicts the model's skip compares: the view, the marks, the columns drawn.
/// `Sabotage` forgets the marks.
#[derive(Clone, Copy, PartialEq)]
enum Skip {
    Never,
    Unchanged,
    Sabotage,
}

/// Columns the pick tests: every dirty one (`Off`), the client's candidate list (`Listed`: made on
/// the second pick with one key), the list made at every new key and used at once (`EveryKey`: a
/// column marked for the first time is then tested only through the list), and the sabotages of
/// the latter: a key blind to the walk or to the marks, a list that forgets the columns never drawn
/// or the priority columns.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Candidates {
    Off,
    Listed,
    EveryKey,
    BlindToWalk,
    BlindToMarks,
    ForgetsUndrawn,
    ForgetsUrgent,
}

/// How the model picks: the launch policy, the skip of unchanged held picks and the candidate list.
#[derive(Clone, Copy)]
struct Pick {
    policy: Policy,
    skip: Skip,
    candidates: Candidates,
}

impl Pick {
    fn of(policy: Policy) -> Pick {
        Pick { policy, skip: Skip::Never, candidates: Candidates::Off }
    }
}

impl Model {
    fn load(&mut self, key: (i32, i32)) {
        self.world.insert(key, [1; SECTIONS]);
        self.dirty.insert(key, Sections::ALL);
        self.marks_rev += 1;
    }

    fn change(&mut self, key: (i32, i32), s: usize) {
        if let Some(c) = self.world.get_mut(&key) {
            c[s] += 1;
            self.dirty.entry(key).or_default().insert(s as i32);
            self.marks_rev += 1;
        }
    }

    /// Column received again (all its sections changed): redone whole.
    fn replace(&mut self, key: (i32, i32)) {
        if let Some(c) = self.world.get_mut(&key) {
            c.iter_mut().for_each(|v| *v += 1);
            self.dirty.insert(key, Sections::ALL);
            self.marks_rev += 1;
        }
    }

    /// One frame: launches chosen by `pick` from what `sight` sees, meshes made, then the frame
    /// drawn checked against the view `must_be_fresh`. With a skip, the pick is skipped while
    /// nothing changed since a pick that held every dirty column, as the client does.
    fn frame(
        &mut self,
        pick: Pick,
        sight: &Sight<'_>,
        must_be_fresh: impl Fn((i32, i32)) -> Sections,
    ) -> Result<(), String> {
        let marks = if pick.skip == Skip::Sabotage { 0 } else { self.marks_rev };
        let signature = (pick.skip != Skip::Never).then_some((sight.view_id, sight.walk_id, marks, self.drawn.len()));
        if signature.is_some() && signature == self.all_held {
            self.skipped += 1;
        } else {
            let listed = self.candidates_of_frame(pick.candidates, sight);
            let Model { drawn, evicted, dirty, candidates, tested, .. } = self;
            let in_view = |k: (i32, i32)| drawn.contains_key(&k).then(|| sight.in_view(k));
            let launchable = |k, marks| {
                tested.set(tested.get() + 1);
                (pick.policy)(marks, in_view(k), !evicted.contains(&k), urgent(k))
            };
            let among = listed.then_some(Among { columns: &candidates.columns, dirty_in_progress: 0 });
            let launches = pick_columns(dirty, among, usize::MAX, (0, 0), |_, _| true, launchable).launches;
            self.all_held = signature.filter(|_| launches.is_empty() && !self.dirty.is_empty());
            for (key, launch) in launches {
                self.mesh(key, launch);
            }
        }
        self.check(must_be_fresh)?;
        self.held_frames += usize::from(!self.dirty.is_empty());
        self.carried = self.dirty.clone();
        Ok(())
    }

    /// The client's candidate list for this frame ([`MeshCandidates::list`]) with the key and the
    /// test of the client ([`candidate_key`], [`listed_as_priority`]), or of a sabotage feeding them
    /// blind inputs: the columns drawn stand for the renderer's connectivity revision (a column
    /// appears, none leaves), the walk's number for its generation, the priority column for the
    /// player's changed column (option 1). `false`: no list this frame, every dirty column is tested.
    fn candidates_of_frame(&mut self, mode: Candidates, sight: &Sight<'_>) -> bool {
        if mode == Candidates::Off {
            return false;
        }
        let reach = ReachStamp {
            frame_drawn: true,
            walk_generation: if mode == Candidates::BlindToWalk { 0 } else { u64::from(sight.walk_id) },
            connectivity_revision: self.drawn.len() as u64,
        };
        let marks = if mode == Candidates::BlindToMarks { 0 } else { self.marks_rev };
        let key = candidate_key(marks, reach, 1, 1, (0, 0));
        let drawn = &self.drawn;
        let may_launch = |k, marks| {
            let known = drawn.contains_key(&k) || mode == Candidates::ForgetsUndrawn;
            let changed_by_player = urgent(k) && mode != Candidates::ForgetsUrgent;
            view_may_launch(marks, known.then(|| (sight.reached)(k)), listed_as_priority(changed_by_player, k, (0, 0), 1))
        };
        if mode == Candidates::Listed {
            return self.candidates.list(key, &self.dirty, may_launch).is_some();
        }
        self.candidates.refresh(key, &self.dirty, may_launch);
        true
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
/// comes into view: the meshes must be the world's. The walk reaches every section.
fn trajectory(policy: Policy, lag: u32) -> Result<Outcome, String> {
    run(Trial { pick: Pick::of(policy), lag, still: false, walk: false }).map(|r| r.outcome)
}

/// One trial of the model: how it picks, the lag of the view the pick reads, a still camera on a
/// resting world before everything comes into view (`still`: the client's static scene, the
/// columns held out of view stay held frame after frame; one section in view changes once), and a
/// walk that reaches about half the sections and changes every 20 frames, blocks changing one frame
/// in four (`walk`; otherwise the walk reaches every section and blocks change every frame).
#[derive(Clone, Copy)]
struct Trial {
    pick: Pick,
    lag: u32,
    still: bool,
    walk: bool,
}

/// What a trial gives: its outcome, the frames that skipped their pick and the columns the picks
/// tested against the view.
#[derive(Debug)]
struct TrialRun {
    outcome: Outcome,
    skipped: usize,
    tested: usize,
}

/// Plays [`trajectory`]'s 400 frames under `trial`.
fn run(trial: Trial) -> Result<TrialRun, String> {
    let Trial { pick, lag, still, walk } = trial;
    let reached = |s: u32, k| if walk { reached_by_walk(s / 20, k) } else { Sections::ALL };
    let walk_id = |s: u32| if walk { s / 20 } else { 0 };
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
        // With the walk, blocks change one frame in four: the frames in between mark nothing, as
        // when turning in place (the candidate list's key repeats).
        let changes = if !walk || f % 4 == 0 { 5 } else { 0 };
        for _ in 0..changes {
            let k = key(&mut rng);
            m.change(k, rng.below(SECTIONS as u64) as usize);
        }
        if f % 20 == 7 {
            m.replace(key(&mut rng));
        }
        if f % 7 == 3 {
            m.evicted.insert(key(&mut rng));
        }
        // The view of `view_of_frame` turns every 3 frames.
        let seen = f.saturating_sub(lag);
        let (reached_seen, field_seen) = (|k| reached(seen, k), |k| view_of_frame(seen, k));
        let sight = Sight { view_id: seen / 3, walk_id: walk_id(seen), reached: &reached_seen, field: &field_seen };
        let in_view = |s: u32, k| reached(s, k).intersection(view_of_frame(s, k));
        let fresh = |k| in_view(f, k).intersection(in_view(f.saturating_sub(1), k));
        m.frame(pick, &sight, fresh).map_err(|e| format!("frame {f}: {e}"))?;
    }
    if still {
        let (reached_last, field_last) = (|k| reached(399, k), |k| view_of_frame(399, k));
        let sight = Sight { view_id: u32::MAX - 1, walk_id: walk_id(399), reached: &reached_last, field: &field_last };
        let mut drawn: Vec<_> = m.drawn.keys().copied().collect();
        drawn.sort_unstable();
        let shown =
            drawn.into_iter().find_map(|k| (0..SECTIONS).find(|&s| sight.in_view(k).contains(s)).map(|s| (k, s)));
        for q in 0..12 {
            if q == 6
                && let Some((k, s)) = shown
            {
                m.change(k, s);
            }
            m.frame(pick, &sight, |k| sight.in_view(k)).map_err(|e| format!("still frame {q}: {e}"))?;
        }
    }
    let everything = |_| all_sections();
    let sight = Sight { view_id: u32::MAX, walk_id: u32::MAX, reached: &everything, field: &everything };
    m.frame(pick, &sight, everything).map_err(|e| format!("all in view: {e}"))?;
    let outcome = (m.meshed, m.late, m.held_frames, m.drawn);
    Ok(TrialRun { outcome, skipped: m.skipped, tested: m.tested.get() })
}

/// Skipping the pick while nothing changed since one that held every dirty column (the client's
/// `RUSTCRAFT_OPT_MESH_SKIP_UNCHANGED_HELD`) gives the same meshes, work and late sections as
/// picking every frame; a skip that forgets the new marks draws stale sections.
#[test]
fn unchanged_held_pick_is_skipped_without_changing_the_meshes() {
    let trial = |skip| Trial { pick: Pick { skip, ..Pick::of(sections_to_launch) }, lag: 1, still: true, walk: false };
    let picked_each_frame = run(trial(Skip::Never)).expect("filter").outcome;
    let skipping = run(trial(Skip::Unchanged)).expect("filter skipping unchanged picks");
    assert!(skipping.skipped > 0, "the trajectory never repeats an all-held pick: test without object");
    assert_eq!(skipping.outcome, picked_each_frame, "skipping changed the outcome");
    let stale = run(trial(Skip::Sabotage)).expect_err("skip blind to new marks");
    assert!(stale.contains("stale"), "{stale}");
}

/// The candidate list (`RUSTCRAFT_OPT_MESH_PICK_REACHED_MARKS`) launches exactly what testing every
/// dirty column launches (same meshes, work, late sections and held frames, with and without the
/// skip of unchanged picks, on a walk that reaches half the sections and changes every 20 frames,
/// and on one that reaches them all), as the list made at every new key does, while testing far
/// fewer columns against the view. With the list used at every new key, a list kept across a walk
/// change or new marks, or that forgets the columns never drawn or the priority column, is caught.
#[test]
fn candidate_list_launches_what_testing_every_column_launches() {
    let pick = |skip, candidates| Pick { skip, candidates, ..Pick::of(sections_to_launch) };
    let mut tested = Vec::new();
    for walk in [true, false] {
        for skip in [Skip::Never, Skip::Unchanged] {
            let trial = |candidates| Trial { pick: pick(skip, candidates), lag: 1, still: true, walk };
            let every = run(trial(Candidates::Off)).expect("every dirty column tested");
            let listed = run(trial(Candidates::Listed)).expect("candidate list");
            let every_key = run(trial(Candidates::EveryKey)).expect("list made at every new key");
            for (name, other) in [("candidate list", &listed), ("list made at every new key", &every_key)] {
                assert_eq!(other.outcome, every.outcome, "walk {walk}: the {name} changed the outcome");
                assert_eq!(other.skipped, every.skipped, "walk {walk}: not the same skipped picks ({name})");
            }
            tested.push((every.tested, listed.tested, every_key.tested));
        }
    }
    let walked = run(Trial { pick: pick(Skip::Never, Candidates::Listed), lag: 1, still: true, walk: true });
    let (work, late, held, _) = walked.expect("candidate list").outcome;
    assert_eq!((work, late, held), EXPECTED_WALK_OUTCOME, "outcome of the walk trial");
    assert_eq!(tested, EXPECTED_TESTED, "columns tested against the view (every column, listed, every key)");
    let trial = |candidates| Trial { pick: pick(Skip::Never, candidates), lag: 1, still: true, walk: true };
    let stale = run(trial(Candidates::BlindToWalk)).expect_err("list kept across a walk change");
    assert!(stale.contains("stale"), "{stale}");
    let stale = run(trial(Candidates::BlindToMarks)).expect_err("list kept across new marks");
    assert!(stale.contains("stale") || stale.contains("never meshed"), "{stale}");
    let hole = run(trial(Candidates::ForgetsUndrawn)).expect_err("list without the columns never drawn");
    assert!(hole.contains("never meshed"), "{hole}");
    let urgent = run(trial(Candidates::ForgetsUrgent)).expect_err("list without the priority column");
    assert!(urgent.contains("(0, 0)") && urgent.contains("stale"), "{urgent}");
}

/// Sections meshed, of which late, and frames ended with marks held, on the walk trial.
const EXPECTED_WALK_OUTCOME: (usize, usize, usize) = (1119, 386, 408);
/// Columns tested against the view (every dirty column, candidate list, list made at every new key):
/// walk with and without the skip, then a walk that reaches every section.
const EXPECTED_TESTED: [(usize, usize, usize); 4] =
    [(8148, 4874, 3150), (6552, 4356, 2607), (9987, 9987, 9987), (9779, 9779, 9779)];

/// The filter holds sections out of view and meshes them once they show; nothing drawn twice in a
/// row is stale, no column is left unmeshed, priority columns are never held, and once everything
/// is in view the meshes are those of the old path. Work of both paths stored (fixed seed).
#[test]
fn filter_meshes_what_comes_into_view_and_ends_like_the_old_path() {
    let (old_work, old_late, _, old_drawn) = trajectory(launch_everything, 1).expect("old path");
    let (new_work, new_late, held, new_drawn) = trajectory(sections_to_launch, 1).expect("filter");
    assert_eq!(old_late, 0, "the old path holds nothing");
    assert!(new_late > 100 && held > 300, "the trajectory must hold sections then show them ({new_late}, {held})");
    assert_eq!(new_drawn, old_drawn, "final meshes differ from the old path");
    assert_eq!((old_work, new_work, new_late), (EXPECTED_OLD_WORK, EXPECTED_NEW_WORK, EXPECTED_LATE));
}

const EXPECTED_OLD_WORK: usize = 2667;
const EXPECTED_NEW_WORK: usize = 2339;
const EXPECTED_LATE: usize = 1228;

/// The model catches a filter that holds the columns never drawn (the walk cannot reach them: a
/// hole at loading) and a filter that reads a view three frames old (stale sections drawn).
#[test]
fn filter_sabotages_are_caught() {
    let hole = trajectory(hold_never_meshed, 1).expect_err("never meshed columns held");
    assert!(hole.contains("hole"), "{hole}");
    let stale = trajectory(sections_to_launch, 3).expect_err("view three frames late");
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
/// their marks and take no slot; a column
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
    let keys = |p: &Picked| p.launches.iter().map(|l| l.0.0).collect::<Vec<_>>();
    let mut d = dirty.clone();
    let picked = pick_columns(&mut d, None, 2, (0, 0), |_, _| true, launchable);
    assert_eq!(keys(&picked), [3, 4]);
    assert_eq!(d.get(&(3, 0)), Some(&sections(&[0])), "column 3 keeps its mark out of view");
    assert!(!d.contains_key(&(4, 0)) && d.contains_key(&(5, 0)) && d.contains_key(&(0, 0)));
    let mut d = dirty.clone();
    let picked = pick_columns(&mut d, None, 2, (0, 0), |_, _| true, |_, m| Some(m));
    assert_eq!(keys(&picked), [0, 1], "old path: the closest");
    let mut d = dirty.clone();
    let picked = pick_columns(&mut d, None, 6, (0, 0), |(x, _), _| x != 4, launchable);
    assert_eq!(keys(&picked), [3, 5], "a column in progress waits");
    // Among a candidate list: only the listed dirty columns are tested (a listed column no longer
    // dirty is passed over), the others keep their marks untouched.
    let mut d = dirty;
    d.remove(&(4, 0));
    let tested = std::cell::Cell::new(0);
    let counted = |k, m| {
        tested.set(tested.get() + 1);
        launchable(k, m)
    };
    let among = Among { columns: &[(5, 0), (4, 0), (0, 0)], dirty_in_progress: 0 };
    let picked = pick_columns(&mut d, Some(among), 6, (0, 0), |_, _| true, counted);
    assert_eq!(keys(&picked), [5], "only the listed columns launch");
    assert_eq!(tested.get(), 2, "the listed column no longer dirty is not tested");
    assert_eq!(d.len(), 4, "the columns left out keep their marks");
    assert!(d.contains_key(&(3, 0)) && !d.contains_key(&(5, 0)));
}

/// The held-columns gauge counts the same with the candidate list as when every column is tested:
/// the columns in progress are left out, listed or not (`mesh_columns_held_out_of_view` is compared
/// between both states of `RUSTCRAFT_OPT_MESH_PICK_REACHED_MARKS`).
#[test]
fn held_gauge_is_the_same_with_and_without_the_candidate_list() {
    let dirty: HashMap<(i32, i32), Sections> = (0..6).map(|x| ((x, 0), sections(&[0, 1]))).collect();
    // Columns 0 to 2 held out of view, 3 to 5 in view; 1 (unlisted) and 4 (listed) in progress.
    let launchable = |(x, _): (i32, i32), marks: Sections| (x >= 3).then_some(marks);
    let may_start = |(x, _): (i32, i32), _| x != 1 && x != 4;
    let every = pick_columns(&mut dirty.clone(), None, 0, (0, 0), may_start, launchable);
    assert_eq!(every.held, 2, "columns 0 and 2: the one in progress is not held");
    let among = Among { columns: &[(3, 0), (4, 0), (5, 0)], dirty_in_progress: 2 };
    let listed = pick_columns(&mut dirty.clone(), Some(among), 0, (0, 0), may_start, launchable);
    assert_eq!(listed.held, every.held, "the gauge must not count the unlisted column in progress");
}

/// The candidate test: a column never drawn or a priority one is always listed, a drawn one only
/// for a mark on a reached section. Every column the pick's priority test (`wants_sync`) answers yes
/// for, under each priority option, is listed ([`listed_as_priority`]): the player's changed columns
/// (options 1 and 2) and the nearby priority remesh (`prioritizeChunkUpdates` 2, the client's own
/// rule), which never reaches a column more than two columns from the eye's wherever the eye stands
/// in its column (a bound one ring short is caught).
#[test]
fn candidate_test_and_priority_columns() {
    let (marks, reached) = (sections(&[1, 6]), sections(&[0, 6, 7]));
    assert!(view_may_launch(marks, None, false), "never drawn");
    assert!(view_may_launch(marks, Some(Sections::default()), true), "priority");
    assert!(view_may_launch(marks, Some(reached), false), "a mark on a reached section");
    assert!(!view_may_launch(marks, Some(sections(&[0, 2, 7])), false), "marks out of the walk");
    assert!(!view_may_launch(Sections::ALL, Some(Sections::default()), false), "column the walk misses");
    assert!(!listed_as_priority(true, (0, 0), (0, 0), 0), "no priority option: nothing listed for it");
    assert!(!listed_as_priority(false, (3, 0), (0, 0), 2), "beyond the nearby rings");
    let mut video = crate::video::VideoState::default();
    let mut rng = Rng(SEED ^ 0x0e7e);
    let column = |rng: &mut Rng| rng.below(125) as i32 - 62;
    for _ in 0..40 {
        video.mark_player_change(column(&mut rng), column(&mut rng));
    }
    let (mut nearby, mut farthest, mut by_player) = (0, 0, 0);
    for _ in 0..2000 {
        let at = |rng: &mut Rng| rng.below(2_000_000) as f64 / 1000.0 - 1000.0;
        let eye = [at(&mut rng), 70.0, at(&mut rng)];
        let eye_column = ((eye[0] / 16.0).floor() as i32, (eye[2] / 16.0).floor() as i32);
        for (dx, dz, mode) in (-4..=4).flat_map(|dx| (-4..=4).flat_map(move |dz| (0..=2).map(move |m| (dx, dz, m)))) {
            let key = (eye_column.0 + dx, eye_column.1 + dz);
            if !video.wants_sync(key.0, key.1, eye, mode) {
                continue;
            }
            let changed_by_player = video.changed_by_player(key.0, key.1);
            by_player += usize::from(changed_by_player && mode == 1);
            if mode == 2 && !changed_by_player {
                nearby += 1;
                farthest = farthest.max(dx.abs().max(dz.abs()));
            }
            let listed = listed_as_priority(changed_by_player, key, eye_column, mode);
            assert!(listed, "eye {eye:?}, option {mode}: column {key:?} left out");
        }
    }
    assert!(by_player > 100, "the trial must meet the player's columns ({by_player})");
    assert!(nearby > 2000 && farthest == 2, "the trial must reach the second ring ({nearby}, {farthest})");
}

/// The candidate list's key changes with each of its inputs (a list kept across one of them would
/// leave a column that can now show untested); the eye's column counts only for the nearby priority.
#[test]
fn candidate_key_follows_each_input() {
    let reach = ReachStamp { frame_drawn: true, walk_generation: 5, connectivity_revision: 7 };
    let key = |marks, reach, sync_mode, player_marks, eye| candidate_key(marks, reach, sync_mode, player_marks, eye);
    let base = key(1, reach, 2, 0, (0, 0));
    let changed = [
        ("marks", key(2, reach, 2, 0, (0, 0))),
        ("frame drawn", key(1, ReachStamp { frame_drawn: false, ..reach }, 2, 0, (0, 0))),
        ("walk", key(1, ReachStamp { walk_generation: 6, ..reach }, 2, 0, (0, 0))),
        ("connectivity", key(1, ReachStamp { connectivity_revision: 8, ..reach }, 2, 0, (0, 0))),
        ("priority option", key(1, reach, 1, 0, (0, 0))),
        ("player's marks", key(1, reach, 2, 1, (0, 0))),
        ("eye column, nearby priority", key(1, reach, 2, 0, (1, 0))),
    ];
    for (input, other) in changed {
        assert_ne!(other, base, "{input}: same key");
    }
    assert_eq!(key(1, reach, 1, 0, (0, 0)), key(1, reach, 1, 0, (9, 9)), "the eye column only counts for option 2");
}

/// The candidate list is made again only when its key changes: same key, same list even if the
/// dirty columns changed meanwhile; a new key reads them again.
#[test]
fn candidate_list_is_kept_while_its_key_repeats() {
    let reach = ReachStamp { frame_drawn: true, walk_generation: 2, connectivity_revision: 3 };
    let key = candidate_key(1, reach, 0, 0, (0, 0));
    let mut dirty: HashMap<(i32, i32), Sections> = (0..4).map(|x| ((x, 0), sections(&[1]))).collect();
    let mut list = MeshCandidates::default();
    list.refresh(key, &dirty, |(x, _), _| x % 2 == 0);
    let sorted = |l: &MeshCandidates| {
        let mut v = l.columns.clone();
        v.sort_unstable();
        v
    };
    assert_eq!(sorted(&list), [(0, 0), (2, 0)]);
    dirty.insert((4, 0), sections(&[1]));
    list.refresh(key, &dirty, |_, _| true);
    assert_eq!(sorted(&list), [(0, 0), (2, 0)], "same key: list kept");
    list.refresh(CandidateKey { marks: 2, ..key }, &dirty, |(x, _), _| x % 2 == 0);
    assert_eq!(sorted(&list), [(0, 0), (2, 0), (4, 0)], "new key: made again");
    // The pick's list: none the first time a key is seen, made on the second pick in a row with it,
    // then served while the key repeats.
    let mut list = MeshCandidates::default();
    let even = |(x, _): (i32, i32), _| x % 2 == 0;
    assert!(list.list(key, &dirty, even).is_none(), "first pick with a key: every column tested");
    assert_eq!(list.list(key, &dirty, even).map(<[_]>::len), Some(3), "second pick: list made");
    assert_eq!(list.list(key, &dirty, |_, _| true).map(<[_]>::len), Some(3), "key repeats: same list");
    let other = CandidateKey { marks: 9, ..key };
    assert!(list.list(other, &dirty, |_, _| true).is_none(), "key changed: every column tested");
    assert_eq!(list.list(other, &dirty, |_, _| true).map(<[_]>::len), Some(5), "made again from the dirty columns");
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
    let picked = pick_columns(&mut dirty, None, 0, (0, 0), |_, _| true, |(x, _), m| (x < 2).then_some(m));
    assert!(picked.launches.is_empty() && dirty == before, "a full queue launches nothing");
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
