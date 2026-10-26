//! Portal tests (S6): pure geometry (frame, rectangle, spiral, End pattern,
//! coordinate conversion) and real runs on a fresh world (lighting, Nether round
//! trip, End through the frame).

use hashbrown::HashMap;

use crate::blocks::{Dir, P};
use crate::portal_shape::{self as ps, Axis, Cell};

/// Test grid: named cells, the rest empty.
fn grid(cells: &HashMap<P, Cell>) -> impl Fn(P) -> Cell + '_ {
    move |p| cells.get(&p).copied().unwrap_or(Cell::Empty)
}

/// Obsidian frame with interior `w`×`h`, interior low-left corner at `o`, X axis (right =
/// west, so the interior extends towards decreasing x).
fn frame_x(o: P, w: i32, h: i32, corners: bool) -> HashMap<P, Cell> {
    let mut m = HashMap::new();
    for i in -1..=w {
        for j in -1..=h {
            let edge = i == -1 || i == w || j == -1 || j == h;
            let corner = (i == -1 || i == w) && (j == -1 || j == h);
            if edge && (corners || !corner) {
                m.insert([o[0] - i, o[1] + j, o[2]], Cell::Frame);
            }
        }
    }
    // Floor all around so the descent stops (other).
    for x in -30..30 {
        m.entry([o[0] + x, o[1] - 2, o[2]]).or_insert(Cell::Other);
    }
    m
}

#[test]
fn nether_frame_found_on_both_axes_and_sizes() {
    for (w, h) in [(2, 3), (4, 5), (21, 21), (2, 21), (21, 3)] {
        let m = frame_x([0, 64, 0], w, h, false);
        let at = grid(&m);
        let s = ps::find_empty_shape(&at, -64, [0, 64, 0], Axis::X).expect("frame");
        assert_eq!((s.width, s.height, s.axis), (w, h, Axis::X), "size {w}×{h}");
        assert_eq!(s.bottom_left, [0, 64, 0]);
        assert_eq!(s.interior().len() as i32, w * h);
        // Lit from the top of the interior: the search goes down to the bottom.
        let s2 = ps::find_empty_shape(&at, -64, [-(w - 1), 64 + h - 1, 0], Axis::Z).expect("frame (other preferred axis)");
        assert_eq!(s2.bottom_left, [0, 64, 0]);
    }
    // Too big (22 wide) or too small (1 wide): nothing.
    for (w, h) in [(22, 3), (1, 3), (2, 2), (3, 22)] {
        let m = frame_x([0, 64, 0], w, h, true);
        assert!(ps::find_empty_shape(&grid(&m), -64, [0, 64, 0], Axis::X).is_none(), "{w}×{h}");
    }
}

#[test]
fn nether_frame_with_portal_blocks_is_not_empty_but_complete() {
    let mut m = frame_x([5, 70, -3], 2, 3, true);
    for p in [[5, 70, -3], [4, 70, -3], [5, 71, -3], [4, 71, -3], [5, 72, -3], [4, 72, -3]] {
        m.insert(p, Cell::Portal);
    }
    let at = grid(&m);
    assert!(ps::find_empty_shape(&at, 0, [5, 70, -3], Axis::X).is_none());
    let s = ps::find_any_shape(&at, 0, [5, 71, -3], Axis::X);
    assert!(s.is_complete());
    drop(at);
    // One obsidian block fewer: no longer complete.
    m.remove(&[6, 71, -3]);
    let at = grid(&m);
    assert!(!ps::find_any_shape(&at, 0, [5, 71, -3], Axis::X).is_complete());
}

#[test]
fn largest_rectangle_of_a_portal() {
    let inside: Vec<P> = (0..3).flat_map(|x| (10..14).map(move |y| [x, y, 7])).collect();
    let test = |p: P| inside.contains(&p);
    let r = ps::largest_rectangle([1, 12, 7], 0, 21, 1, 21, &test);
    assert_eq!((r.min, r.size1, r.size2), ([0, 10, 7], 3, 4));
    assert_eq!(ps::max_rectangle(&[2, 1, 5, 6, 2, 3]), ((2, 3), 5));
}

#[test]
fn spiral_starts_at_center_and_turns() {
    let s = ps::spiral([0, 0, 0], 16, Dir::East, Dir::South);
    assert_eq!(&s[..6], &[[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1], [-1, 0, 1], [-1, 0, 0]]);
    // The whole square of radius 16 is covered once.
    let mut seen = hashbrown::HashSet::new();
    for p in &s {
        assert!(seen.insert(*p));
    }
    assert!((-16..=16).all(|x| (-16..=16).all(|z| seen.contains(&[x, 0, z]))));
}

#[test]
fn coordinates_scale_between_dimensions() {
    assert_eq!(ps::teleport_scale(1.0, 8.0), 0.125);
    assert_eq!(ps::teleport_scale(8.0, 1.0), 8.0);
    // 100.5 × 1/8 = 12.5625 → block 12; −100.5 / 8 = −12.56 → block −13.
    assert_eq!((100.5f64 * 0.125).floor() as i32, 12);
    assert_eq!((-100.5f64 * 0.125).floor() as i32, -13);
}

#[test]
fn relative_and_exit_positions() {
    // Entry portal 2×3 X axis, corner (10, 64, 5); player in the middle, 0.2 forward.
    let rect = ps::Rect { min: [10, 64, 5], size1: 2, size2: 3 };
    let off = ps::relative_position(rect, Axis::X, [11.0, 64.0, 5.7], 0.6, 1.8);
    assert!((off[0] - 0.5).abs() < 1e-9 && off[1].abs() < 1e-9 && (off[2] - 0.2).abs() < 1e-9, "{off:?}");
    // Exit on the Z axis: rotation of 90°, right along z.
    let exit = ps::Rect { min: [-3, 40, 8], size1: 2, size2: 3 };
    let (p, rot) = ps::exit_position(exit, Axis::Z, Axis::X, off, 0.6, 1.8);
    assert_eq!(rot, 90.0);
    assert!((p[0] - (-3.0 + 0.7)).abs() < 1e-9 && (p[1] - 40.0).abs() < 1e-9 && (p[2] - 9.0).abs() < 1e-9, "{p:?}");
}

#[test]
fn free_point_reaches_a_corner_between_two_blocks() {
    // Two crossing boxes leave only the corner x ≥ 1, z ≥ 1 free: the nearest point
    // is (1, y, 1), at √0.5; a projection on a single face does not find it (1,0).
    let allowed = ([0.0, 0.0, 0.0], [2.0, 1.0, 2.0]);
    let blocked = [([-1.0, -1.0, -1.0], [1.0, 2.0, 1.5]), ([-1.0, -1.0, -1.0], [1.5, 2.0, 1.0])];
    let p = ps::closest_free_point(allowed, &blocked, [0.5, 0.5, 0.5]).expect("free");
    assert_eq!(p, [1.0, 0.5, 1.0]);
    // Center already free: returned as is; all blocked: nothing.
    assert_eq!(ps::closest_free_point(allowed, &blocked, [1.7, 0.2, 1.8]), Some([1.7, 0.2, 1.8]));
    assert_eq!(ps::closest_free_point(allowed, &[([-1.0; 3], [3.0; 3])], [0.5; 3]), None);
    // Distance tie: the first box of the traversal (y, then x, then z) wins → low x.
    let wall = [([0.5, -1.0, -1.0], [1.5, 2.0, 3.0])];
    assert_eq!(ps::closest_free_point(allowed, &wall, [1.0, 0.5, 1.0]), Some([0.5, 0.5, 1.0]));
}

#[test]
fn end_frame_pattern_matches_only_complete_ring() {
    // 5×5 ring around the interior (x 0..2, z 0..2): faces turned towards the center.
    let mut eyes: HashMap<P, Dir> = HashMap::new();
    for i in 0..3 {
        eyes.insert([i, 60, -1], Dir::South);
        eyes.insert([i, 60, 3], Dir::North);
        eyes.insert([-1, 60, i], Dir::East);
        eyes.insert([3, 60, i], Dir::West);
    }
    let get = |e: &HashMap<P, Dir>| {
        let e = e.clone();
        move |p: P| e.get(&p).copied()
    };
    for &last in eyes.keys().collect::<Vec<_>>() {
        let f = ps::find_end_frame(last, &get(&eyes)).expect("pattern");
        assert_eq!([f[0] - 3, f[1], f[2] - 3], [0, 60, 0], "from {last:?}");
    }
    let mut missing = eyes.clone();
    missing.remove(&[1, 60, 3]);
    assert!(ps::find_end_frame([0, 60, -1], &get(&missing)).is_none());
    let mut wrong = eyes.clone();
    wrong.insert([1, 60, 3], Dir::South);
    assert!(ps::find_end_frame([0, 60, -1], &get(&wrong)).is_none());
}

/// Nether portal blocks of a zone of chunks of dimension `dim`, sorted.
fn portal_blocks(
    s: &mut Server,
    dim: &str,
    cx: std::ops::RangeInclusive<i32>,
    cz: std::ops::RangeInclusive<i32>,
) -> Vec<P> {
    let list: Vec<(i32, i32)> = cx.flat_map(|x| cz.clone().map(move |z| (x, z))).collect();
    s.in_world(dim, |s| {
        s.ensure_loaded(&list);
        let np = s.tb().block_id("nether_portal");
        let mut out = Vec::new();
        for &(x, z) in &list {
            for bx in 0..16 {
                for bz in 0..16 {
                    for y in s.dim.height.min_y..s.top_y() {
                        let p = [x * 16 + bx, y, z * 16 + bz];
                        if s.v().block(s.get(p)) == np {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out.sort();
        out
    })
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let t = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &t);
        } else if e.file_name() != "session.lock" {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

/// Parity with the official server (`S6/ref-portal.ps1`): same items thrown into both
/// portals of a copy of the "before" world; created portals and arrival points compared with the
/// "after" world and the recorded positions (`RUSTCRAFT_S6_REF=<S6 folder>`).
#[test]
#[ignore]
fn portal_parity_with_official_server() {
    let Ok(refdir) = std::env::var("RUSTCRAFT_S6_REF") else { return };
    let refdir = std::path::PathBuf::from(refdir);
    let data = Arc::new(rc_testdata::version_data().expect("data"));
    let work = std::env::temp_dir().join(format!("rc-s6-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    copy_dir(&refdir.join("ref-before"), &work);
    let mut s = open_world(&work, data.clone());
    let mut after = open_world(&refdir.join("ref-after"), data);
    s.enter_world("minecraft:overworld");
    s.load_now(&[(12, 12)]);
    let a = s.nether_destination([201, 101, 200], [201.5, 101.2, 200.5], 0.25, 0.25, false).expect("to the Nether");
    s.enter_world("minecraft:the_nether");
    s.load_now(&[(-4, 2)]);
    let b = s.nether_destination([-60, 71, 41], [-59.5, 71.2, 41.5], 0.25, 0.25, false).expect("to the surface");
    eprintln!("rc-sim: Nether {:?}; surface {:?}", a.pos, b.pos);
    let mine_n = portal_blocks(&mut s, "minecraft:the_nether", -1..=3, -1..=3);
    let theirs_n = portal_blocks(&mut after, "minecraft:the_nether", -1..=3, -1..=3);
    let mine_o = portal_blocks(&mut s, "minecraft:overworld", -33..=-28, 18..=23);
    let theirs_o = portal_blocks(&mut after, "minecraft:overworld", -33..=-28, 18..=23);
    eprintln!("Nether: rc-sim {mine_n:?}\n         official {theirs_n:?}");
    eprintln!("surface: rc-sim {mine_o:?}\n          official {theirs_o:?}");
    assert_eq!(mine_n, theirs_n);
    assert_eq!(mine_o, theirs_o);
    // Positions recorded on the official server (items without gravity).
    assert_eq!(a.pos, [25.5, 77.2, 25.5]);
    assert!((b.pos[0] + 472.5).abs() < 1e-9 && (b.pos[1] + 4.8).abs() < 1e-9 && (b.pos[2] - 333.5).abs() < 1e-9);
    let _ = std::fs::remove_dir_all(&work);
}

/// Demo worlds for the client captures (`RUSTCRAFT_S6_PREP=<folder>`,
/// `RUSTCRAFT_S6_SCENARIO=nether|end`): fresh world, survival player placed in a lit
/// portal (Nether) or in an End portal opened by 12 eyes (End).
#[test]
#[ignore]
fn prep_world() {
    let (Ok(dir), Ok(sc)) = (std::env::var("RUSTCRAFT_S6_PREP"), std::env::var("RUSTCRAFT_S6_SCENARIO")) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let data = Arc::new(rc_testdata::version_data().expect("data"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(&format!("S6 {sc}"));
    w.seed = 1_234_567_890_123_456_789;
    crate::newworld::create_world(&dir, &data, &w).expect("creation");
    let mut s = open_world(&dir, data);
    ready(&mut s);
    s.game_mode = 0;
    let spawn = s.level.spawn_pos;
    let base = build_lit_frame(&mut s, [spawn[0] + 3, 0, spawn[2] + 3]);
    match sc.as_str() {
        "nether" => {
            s.pos = [f64::from(base[0]), f64::from(base[1]), f64::from(base[2]) + 0.5];
            s.yaw = 0.0;
        }
        "nether2" => {
            // Passage done here; player placed 3 blocks in front of the exit portal, turned towards it.
            s.pos = [f64::from(base[0]), f64::from(base[1]), f64::from(base[2]) + 0.5];
            ticks(&mut s, 81);
            assert_eq!(s.dim.name, "minecraft:the_nether");
            let a = s.pos;
            s.pos = [a[0], a[1], a[2] + 3.0];
            s.yaw = 180.0;
            s.pitch = 10.0;
        }
        _ => {
            let c = [base[0] - 8, base[1], base[2] - 6];
            let frame = st(&s, "end_portal_frame");
            s.load_now(&[
                (c[0] >> 4, c[2] >> 4),
                ((c[0] - 2) >> 4, (c[2] - 2) >> 4),
                ((c[0] + 4) >> 4, (c[2] + 4) >> 4),
            ]);
            for i in 0..3 {
                for (p, d) in [
                    ([c[0] + i, c[1], c[2] - 1], Dir::South),
                    ([c[0] + i, c[1], c[2] + 3], Dir::North),
                    ([c[0] - 1, c[1], c[2] + i], Dir::East),
                    ([c[0] + 3, c[1], c[2] + i], Dir::West),
                ] {
                    let fs = s.v().set_dir(frame, "facing", d);
                    let fs = s.v().set_bool(fs, "eye", true);
                    s.set_block(p, fs, crate::blocks::flag::ALL);
                }
            }
            // Last eye placed by the item: the portal opens.
            let last = [c[0] + 1, c[1], c[2] + 3];
            let fs = s.v().set_dir(frame, "facing", Dir::North);
            s.set_block(last, fs, crate::blocks::flag::ALL);
            s.slots[36 + s.held as usize] =
                Some(crate::player::Stack { id: "minecraft:ender_eye".into(), count: 1, components: None });
            assert!(s.use_ender_eye(last));
            s.pos = [f64::from(c[0]) + 1.5, f64::from(c[1]) + 0.4, f64::from(c[2]) + 1.5];
        }
    }
    s.save(true);
    eprintln!("world ready: {} ({sc}), player {:?}", dir.display(), s.pos);
}

// --- Real runs (fresh world, game data required). ---

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::server::Server;

pub(crate) fn open_new_world(tag: &str, seed: i64) -> Option<(Server, std::path::PathBuf)> {
    let (s, session, dir) = open_new_world_session(tag, seed)?;
    std::mem::forget(session);
    Some((s, dir))
}

/// Like `open_new_world`, with the local player's session (packets received).
fn open_new_world_session(tag: &str, seed: i64) -> Option<(Server, rc_client::Session, std::path::PathBuf)> {
    let data = Arc::new(rc_testdata::version_data()?);
    crate::registries::client_jar(&data)?;
    let dir = std::env::temp_dir().join(format!("rc-s6-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = seed;
    crate::newworld::create_world(&dir, &data, &w).expect("creation");
    let (s, session) = open_world_session(&dir, data);
    Some((s, session, dir))
}

/// Server opened on a world; the client session is kept alive (intentional leak, test).
pub(crate) fn open_world(dir: &std::path::Path, data: Arc<rc_data::VersionData>) -> Server {
    let (s, session) = open_world_session(dir, data);
    std::mem::forget(session);
    s
}

pub(crate) fn open_world_session(dir: &std::path::Path, data: Arc<rc_data::VersionData>) -> (Server, rc_client::Session) {
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("AgentS6");
    let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
    let opts = crate::Options {
        world: dir.to_owned(),
        name: "AgentS6".into(),
        uuid,
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 12,
    };
    match Server::open(opts, uuid, data, peer, Arc::new(AtomicBool::new(false))) {
        Ok(s) => (s, session),
        Err((_, e)) => panic!("opening: {e}"),
    }
}

/// Sounds `name` received by the local player since the last read.
fn sounds_heard(s: &Server, session: &rc_client::Session, name: &str) -> usize {
    let id = s.sound_id(name).expect("known sound");
    session
        .events
        .try_iter()
        .filter(|e| {
            matches!(e, rc_client::Event::Sound(p) if matches!(p.sound, rc_proto_v777::play::SoundRef::Id(i) if i == id))
        })
        .count()
}

fn st(s: &Server, name: &str) -> u32 {
    let b = s.tb().block_id(name);
    s.v().default_state(b)
}

fn ready(s: &mut Server) {
    s.loaded = true;
    s.teleport = None;
}

fn ticks(s: &mut Server, n: usize) {
    for _ in 0..n {
        s.game_tick();
        s.tick += 1;
    }
}

/// 4×5 frame X axis placed on the ground near `at` (surface), lit by fire: returns the low corner
/// of the interior.
fn build_lit_frame(s: &mut Server, at: P) -> P {
    s.load_now(&[(at[0] >> 4, at[2] >> 4), ((at[0] - 4) >> 4, at[2] >> 4)]);
    let mut y = s.top_y() - 1;
    while y > s.dim.height.min_y && s.v().is_air(s.get([at[0], y, at[2]])) {
        y -= 1;
    }
    let base = [at[0], y + 1, at[2]];
    let obs = st(s, "obsidian");
    let air = s.air();
    for i in -1..=2 {
        for j in -1..=3 {
            let p = [base[0] - i, base[1] + j, base[2]];
            let edge = i == -1 || i == 2 || j == -1 || j == 3;
            s.set_block(p, if edge { obs } else { air }, crate::blocks::flag::ALL);
        }
    }
    let fire = s.fire_state_for(base);
    s.set_block(base, fire, crate::blocks::flag::ALL);
    base
}

#[test]
fn nether_round_trip_and_end_on_new_world() {
    let Some((mut s, dir)) = open_new_world("portal", 1_234_567_890_123_456_789) else { return };
    ready(&mut s);
    s.game_mode = 0;
    let spawn = s.level.spawn_pos;
    let at = [spawn[0] + 6, 0, spawn[2] + 3];
    let base = build_lit_frame(&mut s, at);
    let np = s.tb().block_id("nether_portal");
    let lit: Vec<P> = (0..2).flat_map(|i| (0..3).map(move |j| [base[0] - i, base[1] + j, base[2]])).collect();
    assert!(lit.iter().all(|&p| s.v().block(s.get(p)) == np), "portal lit");
    assert_eq!(s.v().get(s.get(base), "axis"), Some("x"));
    // Player in the portal: 80 ticks of waiting (default rule), then the Nether.
    s.pos = [f64::from(base[0]), f64::from(base[1]), f64::from(base[2]) + 0.5];
    ticks(&mut s, 80);
    assert_eq!(s.dim.name, "minecraft:overworld", "not yet");
    ticks(&mut s, 1);
    assert_eq!(s.dim.name, "minecraft:the_nether");
    let arrive = s.pos;
    eprintln!("Nether arrival {arrive:?} (entry {base:?})");
    let b = [arrive[0].floor() as i32, arrive[1].floor() as i32, arrive[2].floor() as i32];
    assert_eq!(s.v().block(s.get(b)), np, "exit portal created");
    assert!((arrive[0] - f64::from(base[0]) / 8.0).abs() < 20.0 && (arrive[2] - f64::from(base[2]) / 8.0).abs() < 20.0);
    // Stays in the portal: never sent back (return delay reset at each tick).
    ready(&mut s);
    ticks(&mut s, 120);
    assert_eq!(s.dim.name, "minecraft:the_nether");
    // Leaves, then comes back: return to the original portal.
    let out = [arrive[0], arrive[1], arrive[2] + 3.0];
    s.pos = out;
    ticks(&mut s, 40);
    s.pos = arrive;
    ticks(&mut s, 81);
    assert_eq!(s.dim.name, "minecraft:overworld");
    let back = s.pos;
    eprintln!("return {back:?}");
    assert!((back[0] - f64::from(base[0])).abs() < 3.0 && (back[2] - f64::from(base[2])).abs() < 3.0, "same portal");
    // Save: two dimensions written, player on the surface.
    s.save(true);
    assert!(
        crate::world::dimension_dir(&dir, "minecraft:the_nether")
            .join("region")
            .read_dir()
            .is_ok_and(|mut d| d.next().is_some())
    );
    // End: frame of 12 eyes placed around an interior, player inside.
    ready(&mut s);
    let c = [spawn[0] - 10, base[1] + 6, spawn[2] - 10];
    let frame = st(&s, "end_portal_frame");
    let ring: Vec<(P, Dir)> = (0..3)
        .flat_map(|i| {
            [
                ([c[0] + i, c[1], c[2] - 1], Dir::South),
                ([c[0] + i, c[1], c[2] + 3], Dir::North),
                ([c[0] - 1, c[1], c[2] + i], Dir::East),
                ([c[0] + 3, c[1], c[2] + i], Dir::West),
            ]
        })
        .collect();
    for (p, d) in &ring {
        let fs = s.v().set_dir(frame, "facing", *d);
        s.set_block(*p, fs, crate::blocks::flag::ALL);
    }
    s.game_mode = 1;
    for (p, _) in &ring {
        s.slots[36 + s.held as usize] =
            Some(crate::player::Stack { id: "minecraft:ender_eye".into(), count: 1, components: None });
        assert!(s.use_ender_eye(*p));
    }
    let ep = s.tb().block_id("end_portal");
    assert!((0..3).all(|x| (0..3).all(|z| s.v().block(s.get([c[0] + x, c[1], c[2] + z])) == ep)), "End portal");
    // Return delay (10 ticks) elapsed outside portals, then in the End portal.
    s.pos = [f64::from(c[0]) + 1.5, f64::from(c[1]) + 3.0, f64::from(c[2]) + 1.5];
    ticks(&mut s, 11);
    assert_eq!(s.dim.name, "minecraft:overworld");
    s.pos = [f64::from(c[0]) + 1.5, f64::from(c[1]) + 0.4, f64::from(c[2]) + 1.5];
    ticks(&mut s, 1);
    assert_eq!(s.dim.name, "minecraft:the_end");
    assert_eq!(s.pos, [100.5, 49.0, 0.5]);
    assert_eq!(s.v().block(s.get([100, 48, 0])), s.tb().block_id("obsidian"), "platform");
    assert!(s.v().is_air(s.get([100, 49, 0])) && s.v().is_air(s.get([102, 51, 2])));
    // Gateway without exit: exit created towards the outside (islands), player placed on it.
    ready(&mut s);
    ticks(&mut s, 11);
    let gw = [96, 75, 0];
    s.load_now(&[(gw[0] >> 4, gw[2] >> 4)]);
    let gws = st(&s, "end_gateway");
    s.set_block(gw, gws, crate::blocks::flag::ALL);
    s.pos = [96.5, 75.0, 0.5];
    ticks(&mut s, 1);
    eprintln!("gateway → {:?}", s.pos);
    assert_eq!(s.dim.name, "minecraft:the_end");
    assert!(s.pos[0] > 600.0, "exit far away: {:?}", s.pos);
    // End exit portal without credits seen: credits, then respawn on the surface.
    ready(&mut s);
    ticks(&mut s, 11);
    let exit = [0, 64, 0];
    s.load_now(&[(0, 0)]);
    let eps = st(&s, "end_portal");
    s.set_block(exit, eps, crate::blocks::flag::ALL);
    s.pos = [0.5, 64.4, 0.5];
    ticks(&mut s, 1);
    assert!(s.dims.won_game && s.dims.seen_credits);
    s.perform_respawn();
    assert_eq!(s.dim.name, "minecraft:overworld");
    assert!((s.pos[0] - f64::from(spawn[0])).abs() <= 11.0, "world point {:?}", s.pos);
    s.save(true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn portal_search_and_creation_respect_the_world_border() {
    // Border of 20 blocks centered on 0: the portal created far outside comes back inside the border
    // (fallback brought back by the clamping into the bounds), the search ignores portal blocks outside the border.
    let Some((mut s, dir)) = open_new_world("border", 42) else { return };
    ready(&mut s);
    s.cmd.border.center = [0.0, 0.0];
    s.cmd.border.size = 20.0;
    let r = s.create_portal([100, 70, 100], Axis::X).expect("portal");
    assert_eq!((r.min, r.size1, r.size2), ([9, 70, 9], 2, 3));
    let np = s.tb().block_id("nether_portal");
    assert_eq!(s.v().block(s.get([10, 70, 9])), np, "second column (x = 10, outside the border)");
    assert_eq!(s.find_closest_portal([14, 70, 9], true), Some([9, 70, 9]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn players_cannot_break_or_use_blocks_outside_the_world_border() {
    // Outside the border, neither mining nor placing; inside, everything goes.
    let Some((mut s, dir)) = open_new_world("border-interact", 43) else { return };
    ready(&mut s);
    s.cmd.border.center = [0.0, 0.0];
    s.cmd.border.size = 20.0;
    assert!(s.may_interact([9, 70, 0]));
    assert!(!s.may_interact([10, 70, 0]));
    let stone = s.tb().block_id("stone");
    let st = s.default_state("minecraft:stone");
    assert_eq!(s.v().block(st), stone);
    s.pos = [9.5, 70.0, 0.5];
    s.game_mode = 1;
    for p in [[9, 70, 1], [10, 70, 1]] {
        s.set_block(p, st, 3);
    }
    s.on_action(0, [10, 70, 1], 1, 1);
    assert_eq!(s.get([10, 70, 1]), st, "block outside the border broken");
    s.on_action(0, [9, 70, 1], 1, 2);
    assert_ne!(s.get([9, 70, 1]), st, "block inside the border not broken");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- G4-F05: projectiles, TNT and bosses facing portals; border; anchor sound. ---

use crate::dims::NETHER;
use crate::mobs::species::Kind;
use crate::projectiles::{PKind, Proj};

fn parked_world<'a>(s: &'a Server, name: &str) -> &'a crate::dims::World {
    s.dims.parked.iter().find(|w| w.dim.name == name).expect("parked dimension")
}

/// New projectile of `kind` at `pos`, thrown at `vel`, added to the world; returns its UUID.
fn throw(s: &mut Server, kind: PKind, pos: [f64; 3], vel: [f64; 3], owner: Option<u128>) -> u128 {
    let id = s.proj_new_id();
    let item = crate::player::Stack { id: kind.default_item().into(), count: 1, components: None };
    let mut p = Proj::new(kind, id, pos, item);
    p.vel = vel;
    p.owner = owner;
    let uuid = p.uuid;
    s.proj_add(p);
    uuid
}

/// Through the Nether portal, yaw added and velocity kept, rotated by the
/// yaw difference; pearl through a gateway (nothing relative): stopped dead.
#[test]
fn portal_transition_turns_or_stops_motion() {
    use crate::dims::{Transition, rel};
    use crate::portal::transition_motion;
    let t = |relative, yaw| Transition {
        dim: NETHER.into(),
        pos: [0.0; 3],
        yaw,
        pitch: 0.0,
        relative,
        sound: false,
        ticket: false,
        missing_respawn: false,
    };
    let (v, yaw, pitch) = transition_motion(&t(rel::DELTA | rel::ROTATION, 90.0), [1.0, 0.5, 0.0], 10.0, 5.0);
    assert_eq!((yaw, pitch), (100.0, 5.0));
    assert!(v[0].abs() < 1.0e-3 && (v[2] - 1.0).abs() < 1.0e-6 && v[1] == 0.5, "{v:?}");
    // End: yaw set to 90, the difference depends on the previous yaw.
    let (v, yaw, _) = transition_motion(&t(rel::DELTA | rel::X_ROT, 90.0), [0.0, 0.0, 1.0], 90.0, 0.0);
    assert_eq!(yaw, 90.0);
    assert_eq!(v, [0.0, 0.0, 1.0]);
    let (v, ..) = transition_motion(&t(0, 0.0), [1.0, 0.5, 0.0], 10.0, 0.0);
    assert_eq!(v, [0.0; 3]);
}

/// Micro-bench (ignored): cost of the portal survey on the path of an arrow (3 blocks per
/// tick), far from any portal (common case, simple read of the bounding box) then through
/// a portal (exact sweep). `cargo test --release -p rc-sim -- --ignored bench_portal_sweep`.
#[test]
#[ignore]
fn bench_portal_sweep() {
    let Some((mut s, dir)) = open_new_world("g4f05-bench", 81) else { return };
    ready(&mut s);
    let spawn = s.level.spawn_pos;
    let base = build_lit_frame(&mut s, [spawn[0] + 6, 0, spawn[2] + 3]);
    let (bx, by, bz) = (f64::from(base[0]), f64::from(base[1]) + 1.0, f64::from(base[2]));
    let n = 200_000;
    for (label, from, to) in [
        ("far", [bx + 20.0, by, bz - 1.5], [bx + 20.0, by, bz + 1.5]),
        ("beside", [bx + 3.0, by, bz - 1.5], [bx + 3.0, by, bz + 1.5]),
        ("through", [bx, by, bz - 1.5], [bx, by, bz + 1.5]),
    ] {
        let mut found = 0;
        let t = std::time::Instant::now();
        for i in 0..n {
            let d = f64::from(i % 7) * 1.0e-3;
            found += s.portals_touched_swept([from[0] + d, from[1], from[2]], to, 0.5, 0.5).len();
        }
        let ns = t.elapsed().as_nanos() as f64 / f64::from(n);
        eprintln!("portal survey {label}: {ns:.0} ns per call ({found} touched)");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An arrow, a primed TNT, a pig pass through the Nether portal; the wither does not.
#[test]
fn arrows_tnt_and_mobs_pass_nether_portal_but_not_wither() {
    let Some((mut s, dir)) = open_new_world("g4f05-pass", 1_234_567_890_123_456_789) else { return };
    ready(&mut s);
    s.game_mode = 1;
    let spawn = s.level.spawn_pos;
    let base = build_lit_frame(&mut s, [spawn[0] + 6, 0, spawn[2] + 3]);
    // Player far from the portal (it must not pass).
    s.pos = [f64::from(base[0]), f64::from(base[1]), f64::from(base[2]) + 8.5];
    let (bx, by, bz) = (f64::from(base[0]), f64::from(base[1]), f64::from(base[2]));
    let arrow = throw(&mut s, PKind::Arrow, [bx, by + 1.5, bz - 2.0], [0.0, 0.0, 0.6], None);
    s.tnt_prime([base[0], base[1] + 1, base[2]]);
    // Mobs in a second portal (outside the arrow's path).
    let b2 = build_lit_frame(&mut s, [spawn[0] - 10, 0, spawn[2] + 3]);
    let (cx, cy, cz) = (f64::from(b2[0]), f64::from(b2[1]), f64::from(b2[2]) + 0.5);
    let pig = s.new_mob(Kind::Pig, [cx, cy, cz], 0.0);
    let pig_id = pig.id;
    s.add_mob(pig);
    let wither = s.new_mob(Kind::Wither, [cx, cy, cz], 0.0);
    let wither_id = wither.id;
    s.add_mob(wither);
    let mut arrow_seen = false;
    for _ in 0..8 {
        ticks(&mut s, 1);
        let nether = parked_world(&s, NETHER);
        if nether.proj.list.iter().any(|p| p.uuid == arrow) {
            arrow_seen = true;
            assert!(!s.proj.list.iter().any(|q| q.uuid == arrow && !q.removed), "arrow still on the surface");
        }
    }
    assert_eq!(s.dim.name, "minecraft:overworld");
    let nether = parked_world(&s, NETHER);
    // Pig passed; wither stayed (false). Checked first: a passed wither
    // would wait for the arrow at arrival (shared exit portal) and stop it.
    assert!(nether.mobs.list.iter().flatten().any(|m| m.kind == Kind::Pig), "pig passed");
    assert!(s.mobs.list.iter().flatten().all(|m| m.id != pig_id));
    assert!(s.mobs.list.iter().flatten().any(|m| m.id == wither_id), "wither stayed");
    assert!(nether.mobs.list.iter().flatten().all(|m| m.kind != Kind::Wither));
    assert!(arrow_seen, "arrow passed into the Nether");
    // TNT: passed with the "portal used" flag, nothing left on the surface.
    let tnt = |e: &&crate::blocks::entities::SimEntity| matches!(e.kind, crate::blocks::entities::EntKind::Tnt(_));
    assert!(nether.bs.entities.list.iter().filter(tnt).any(|e| e.used_portal), "TNT passed");
    assert!(!s.bs.entities.list.iter().filter(tnt).any(|e| !e.removed), "TNT stayed");
    // Dragon: never marked either.
    let dragon = s.new_mob(Kind::EnderDragon, [bx, by, bz + 0.5], 0.0);
    assert!(!s.mob_can_use_portal(&dragon));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The player's pearl passes the portal, falls into the Nether and takes its player along (5 damage,
/// return delay set: the pearl was on delay).
#[test]
fn ender_pearl_through_portal_takes_its_owner_along() {
    let Some((mut s, session, dir)) = open_new_world_session("g4f05-pearl", 1_234_567_890_123_456_789) else { return };
    ready(&mut s);
    s.game_mode = 0;
    let spawn = s.level.spawn_pos;
    let base = build_lit_frame(&mut s, [spawn[0] + 6, 0, spawn[2] + 3]);
    let (bx, by, bz) = (f64::from(base[0]), f64::from(base[1]), f64::from(base[2]));
    s.pos = [bx, by, bz - 6.5];
    let owner = Some(s.uuid);
    let health = s.life.vit.health;
    throw(&mut s, PKind::EnderPearl, [bx, by + 2.0, bz - 1.5], [0.0, 0.0, 0.5], owner);
    for _ in 0..40 {
        ticks(&mut s, 1);
        if s.dim.name == NETHER {
            break;
        }
    }
    assert_eq!(s.dim.name, NETHER, "player taken by their pearl");
    assert!(s.dims.player.cooldown > 0, "return delay set");
    assert!(s.life.vit.health < health, "pearl damage");
    assert!(!crate::projectiles::holds_player_pearl(&s.proj, s.uuid));
    // Teleport sound played after the passage, in the arrival dimension: heard.
    assert_eq!(sounds_heard(&s, &session, "minecraft:entity.player.teleport"), 1, "pearl sound");
    let _ = std::fs::remove_dir_all(&dir);
}

/// TNT passed through a portal does not break the Nether portal; an ordinary TNT does.
#[test]
fn tnt_that_used_a_portal_spares_nether_portal_blocks() {
    let Some((mut s, dir)) = open_new_world("g4f05-tntboom", 77) else { return };
    ready(&mut s);
    s.game_mode = 1;
    let spawn = s.level.spawn_pos;
    let base = build_lit_frame(&mut s, [spawn[0] + 6, 0, spawn[2] + 3]);
    s.pos = [f64::from(base[0]), f64::from(base[1]), f64::from(base[2]) + 30.5];
    let np = s.tb().block_id("nether_portal");
    let lit: Vec<P> = (0..2).flat_map(|i| (0..3).map(move |j| [base[0] - i, base[1] + j, base[2]])).collect();
    let c = [f64::from(base[0]), f64::from(base[1]) + 1.5, f64::from(base[2]) + 1.5];
    s.tnt_explode(-1, c, 4.0, None, false, true);
    assert!(lit.iter().all(|&p| s.v().block(s.get(p)) == np), "portal intact");
    s.tnt_explode(-1, c, 4.0, None, false, false);
    assert!(lit.iter().any(|&p| s.v().block(s.get(p)) != np), "portal blown up");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A wind charge targets neither an End crystal nor another wind charge; a
/// snowball does.
#[test]
fn wind_charge_ignores_crystals_and_other_wind_charges() {
    use crate::projectiles::physics::Target;
    let Some((mut s, dir)) = open_new_world("g4f05-wind", 78) else { return };
    ready(&mut s);
    let at = [s.pos[0] + 20.0, s.pos[1] + 30.0, s.pos[2]];
    s.spawn_crystal(at, 0.0, false);
    throw(&mut s, PKind::WindCharge, [at[0] + 1.0, at[1], at[2]], [0.0; 3], None);
    throw(&mut s, PKind::WindCharge, [at[0] - 1.0, at[1], at[2]], [0.0; 3], None);
    throw(&mut s, PKind::Snowball, [at[0], at[1] + 1.0, at[2]], [0.0; 3], None);
    let area = rc_physics::Aabb { min: at.map(|v| v - 4.0), max: at.map(|v| v + 4.0) };
    let targets = |s: &Server, i: usize| -> Vec<Target> {
        s.proj_targets(&s.proj.list[i], &area, false).into_iter().map(|t| t.0).collect()
    };
    let wind = targets(&s, 0);
    assert!(!wind.iter().any(|t| matches!(t, Target::Crystal(_))), "crystal targeted: {wind:?}");
    let other = s.proj.list[1].id;
    assert!(!wind.contains(&Target::Proj(other)), "other charge targeted");
    let snow = targets(&s, 2);
    assert!(snow.iter().any(|t| matches!(t, Target::Crystal(_))), "control: the snowball targets the crystal");
    assert!(snow.contains(&Target::Proj(other)), "control: the snowball targets the charge");
    let _ = std::fs::remove_dir_all(&dir);
}

/// World border: the arrow bounces off it, the snowball breaks on it; dismounting
/// refuses a place outside the border.
#[test]
fn projectiles_and_dismount_respect_the_world_border() {
    let Some((mut s, dir)) = open_new_world("g4f05-border", 79) else { return };
    ready(&mut s);
    s.cmd.border.center = [0.0, 0.0];
    s.cmd.border.size = 20.0;
    s.load_now(&[(0, 0), (-1, 0), (0, -1), (-1, -1)]);
    s.pos = [0.5, 300.0, 0.5];
    let y = 250.0;
    let arrow = throw(&mut s, PKind::Arrow, [8.5, y, 0.5], [3.0, 0.0, 0.0], None);
    let snow = throw(&mut s, PKind::Snowball, [8.5, y, 4.5], [3.0, 0.0, 0.0], None);
    ticks(&mut s, 1);
    let a = s.proj.list.iter().find(|p| p.uuid == arrow).expect("arrow");
    assert!(a.pos[0] < 10.0 && a.vel[0] < 0.0, "arrow sent back: {:?} {:?}", a.pos, a.vel);
    assert!(!s.proj.list.iter().any(|p| p.uuid == snow), "snowball broken on the border");
    // Llama spit: its own ray also stops at the border (spit removed).
    let spit = crate::mobs::mounts::llama::Spit {
        id: 999_999,
        owner: -1,
        pos: [8.0, y, 8.5],
        vel: [3.0, 0.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        rng: crate::mobs::jrand::JRand::new(1),
        removed: false,
        left_owner: true,
        tick: 0,
    };
    s.mobs.mounts.spits.push(spit);
    s.spits_tick();
    assert!(
        !s.mobs.mounts.spits.iter().any(|x| x.id == 999_999 && !x.removed),
        "spit passed through the border: {:?}",
        s.mobs.mounts.spits.iter().map(|x| x.pos).collect::<Vec<_>>()
    );
    // Dismount: stone floor at x = 5 and x = 12, only the place inside the border counts.
    let stone = s.default_state("minecraft:stone");
    for x in [5, 12] {
        s.set_block([x, 199, 0], stone, crate::blocks::flag::ALL);
    }
    assert!(s.safe_dismount([5, 200, 0], false).is_some());
    assert!(s.safe_dismount([12, 200, 0], false).is_none(), "place outside the border refused");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Anchor depletion sound on respawn: to the player alone, not to guests.
#[test]
fn anchor_deplete_sound_goes_to_the_respawning_player_only() {
    let Some((mut s, session, dir)) = open_new_world_session("g4f05-anchor", 80) else { return };
    ready(&mut s);
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    let req = crate::lan::Publish { open: true, port: 0, guest_commands: false, force_mode: Some(0) };
    s.lan_publish(&req).expect("opening");
    let uuid = rc_client::offline_uuid("Guest");
    let mut ctx = s.new_player_ctx("Guest", uuid, 2, 0, Some(0));
    ctx.loaded = true;
    let (out, from_server) = crossbeam_channel::unbounded();
    let (_keep, inp) = crossbeam_channel::unbounded();
    s.lan.as_mut().unwrap().guests.push(crate::lan::Guest {
        name: "Guest".into(),
        uuid,
        out,
        inp,
        ctx: Some(Box::new(ctx)),
        enc: Default::default(),
        gone: false,
        joined: true,
        net: Default::default(),
        req_view: 2,
        chunks: Default::default(),
        moves: Default::default(),
    });
    // Anchor loaded in the Nether, free platform around.
    let anchor = [0, 64, 0];
    s.in_world(NETHER, |w| {
        w.load_now(&[(0, 0), (-1, 0), (0, -1), (-1, -1)]);
        let obs = st(w, "obsidian");
        let air = w.air();
        for dx in -2..=2 {
            for dz in -2..=2 {
                w.set_block([dx, 63, dz], obs, crate::blocks::flag::ALL);
                for dy in 64..=66 {
                    w.set_block([dx, dy, dz], air, crate::blocks::flag::ALL);
                }
            }
        }
        let a = st(w, "respawn_anchor");
        let a = w.v().set_int(a, "charges", 1);
        w.set_block(anchor, a, crate::blocks::flag::ALL);
    });
    s.life.vit.respawn = Some((NETHER.to_owned(), anchor, 0.0, 0.0, false));
    while from_server.try_recv().is_ok() {}
    let _ = sounds_heard(&s, &session, "minecraft:block.respawn_anchor.deplete");
    let ids = *s.peer.ids();
    let id_of = |p: &[u8]| rc_proto::varint::read_varint(&mut rc_proto::Reader::new(p)).ok();
    let (pos, ..) = s.respawn_point_dims();
    assert_eq!(s.dim.name, NETHER);
    assert!((pos[0] - 0.5).abs() <= 2.0 && (pos[2] - 0.5).abs() <= 2.0, "standing near the anchor: {pos:?}");
    let sounds = from_server.try_iter().filter(|p| id_of(p) == Some(ids.sound)).count();
    assert_eq!(sounds, 0, "anchor sound received by the guest");
    assert_eq!(sounds_heard(&s, &session, "minecraft:block.respawn_anchor.deplete"), 1, "anchor sound to the player");
    s.lan_close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Loyalty trident left in the parked Nether (portal ticket): it returns to the player's real
/// coordinates (resolved in any dimension), never to the fictitious position of the parked
/// tick, which sent it under the world.
#[test]
fn loyal_trident_in_a_parked_dimension_is_not_lost() {
    let Some((mut s, dir)) = open_new_world("g4f05-loyal", 82) else { return };
    ready(&mut s);
    s.game_mode = 0;
    let owner = s.uuid;
    let trident = s.in_world(NETHER, |w| {
        w.load_now(&[(0, 0), (-1, 0), (0, -1), (-1, -1)]);
        let id = w.proj_new_id();
        let item = crate::player::Stack { id: PKind::Trident.default_item().into(), count: 1, components: None };
        let mut p = Proj::new(PKind::Trident, id, [0.5, 100.0, 0.5], item);
        p.owner = Some(owner);
        p.loyalty = 3;
        p.dealt_damage = true;
        let uuid = p.uuid;
        w.proj_add(p);
        uuid
    });
    s.place_portal_ticket(NETHER);
    ticks(&mut s, 3);
    let nether = parked_world(&s, NETHER);
    let p = nether.proj.list.iter().find(|p| p.uuid == trident && !p.removed).expect("trident kept");
    assert!(p.pos[1] > 90.0, "trident gone towards the void: {:?}", p.pos);
    let _ = std::fs::remove_dir_all(&dir);
}

/// TNT whose fuse ends in the tick where it passes the portal: the old one explodes on the
/// surface, the copy (fuse from before) in the Nether, like game 26.3.
#[test]
fn primed_tnt_at_the_end_of_its_fuse_explodes_on_both_sides_of_a_portal() {
    use crate::blocks::entities::EntKind;
    let Some((mut s, dir)) = open_new_world("g4f05-tnt2", 1_234_567_890_123_456_789) else { return };
    ready(&mut s);
    s.game_mode = 1;
    let spawn = s.level.spawn_pos;
    let base = build_lit_frame(&mut s, [spawn[0] + 6, 0, spawn[2] + 3]);
    s.pos = [f64::from(base[0]), f64::from(base[1]), f64::from(base[2]) + 30.5];
    crate::mobs::explode::TNT_BLASTS.with_borrow_mut(Vec::clear);
    s.tnt_prime([base[0], base[1] + 1, base[2]]);
    let id = s.bs.entities.list.last().expect("TNT").id;
    for _ in 0..12 {
        // Passage due at the next tick: fuse brought back to 1.
        if s.dims.ents.get(&id).is_some_and(|p| p.proc.is_some())
            && let Some(e) = s.bs.entities.list.iter_mut().find(|e| e.id == id)
        {
            e.kind = EntKind::Tnt(1);
        }
        ticks(&mut s, 1);
    }
    let blasts = crate::mobs::explode::TNT_BLASTS.take();
    assert_eq!(blasts, vec![("minecraft:overworld".to_owned(), false), (NETHER.to_owned(), true)]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Item through the End portal: yaw set to 90, its velocity rotates by the difference with its
/// own yaw (drawn at its creation), not with a zero yaw.
#[test]
fn item_through_end_portal_turns_its_motion_by_its_own_yaw() {
    let Some((mut s, dir)) = open_new_world("g4f05-enditem", 83) else { return };
    ready(&mut s);
    let spawn = s.level.spawn_pos;
    let at = [spawn[0] + 6, 200, spawn[2] + 3];
    s.load_now(&[(at[0] >> 4, at[2] >> 4)]);
    let portal = s.default_state("minecraft:end_portal");
    s.set_block(at, portal, crate::blocks::flag::ALL);
    let stack = crate::player::Stack { id: "minecraft:stone".into(), count: 1, components: None };
    let pos = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 0.4, f64::from(at[2]) + 0.5];
    s.spawn_item(pos, [0.0, 0.0, 0.3], stack, 0);
    let id = s.life.drops.last().expect("item").id;
    s.life.drops.last_mut().expect("item").yaw = 90.0;
    for _ in 0..3 {
        if !s.life.drops.iter().any(|d| d.id == id) {
            break;
        }
        s.drops_portal_tick();
    }
    assert!(!s.life.drops.iter().any(|d| d.id == id), "item passed");
    let end = parked_world(&s, crate::dims::END);
    let d = end.drops.last().expect("item in the End");
    assert_eq!(d.yaw, 90.0);
    // Zero difference: velocity unchanged (with a zero yaw, it would have rotated by -90°).
    assert!(d.vel[0].abs() < 1.0e-6 && (d.vel[2] - 0.3).abs() < 1.0e-6, "{:?}", d.vel);
    let _ = std::fs::remove_dir_all(&dir);
}
