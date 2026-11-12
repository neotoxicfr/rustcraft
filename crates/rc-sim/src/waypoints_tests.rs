//! Rules of the locator bar waypoints (`waypoints.rs`) on hand-made snapshots, without a server:
//! precision by distance and view, ranges, spectators, riders, first tick, the half-degree step,
//! the game rule, the player removal quirk, `/waypoint modify` and team colours.

use rc_proto_v777::waypoint::{WaypointId, WaypointOp, WaypointPos};

use crate::waypoints::{Body, Icon, Outgoing, Player, Snapshot, Waypoints};

const FAR: f64 = 6.0e7;

fn body(id: i32, pos: [f64; 3]) -> Body {
    Body { id, uuid: 1000 + id as u128, pos, ticked: true, spectator: false, transmit_range: FAR, icon: Icon::default() }
}

/// Player `id` at `pos`, view distance 2, packets to seat `id`.
fn player(id: i32, pos: [f64; 3]) -> Player {
    let mut p = Player {
        body: body(id, pos),
        seat: Some(id as usize),
        receive_range: FAR,
        view_center: (0, 0),
        view: 2,
        vehicles: Vec::new(),
        sent_movement: false,
        generation: 1,
    };
    move_player(&mut p, pos);
    p
}

fn move_player(p: &mut Player, pos: [f64; 3]) {
    p.body.pos = pos;
    let b = crate::mobs::mth::block_of(pos);
    p.view_center = (b[0] >> 4, b[2] >> 4);
}

fn world(players: Vec<Player>, others: Vec<Body>) -> Snapshot {
    Snapshot { locator_bar: true, players, others }
}

fn sync(w: &mut Waypoints, snap: &Snapshot) -> Vec<Outgoing> {
    let mut out = Vec::new();
    w.sync(snap, &mut out);
    out
}

/// (operation, place) of the packets about `uuid` sent to `seat`.
fn told(out: &[Outgoing], seat: usize, uuid: u128) -> Vec<(WaypointOp, WaypointPos)> {
    out.iter()
        .filter(|o| o.seat == Some(seat) && o.packet.id == WaypointId::Uuid(uuid))
        .map(|o| (o.packet.op, o.packet.pos))
        .collect()
}

const R: i32 = 1;
const M: i32 = 50;
const M_UUID: u128 = 1050;

/// Exact block in view, chunk outside it, direction only beyond 332 blocks; each jump makes the
/// connection again (track). Sabotage verified (failure): distance compared with `>=`; chunk
/// test on the receiver's chunk instead of the transmitter's.
#[test]
fn precision_follows_distance_and_view() {
    let mut w = Waypoints::default();
    let r = player(R, [0.5, 64.0, 0.5]);
    let mut snap = world(vec![r], vec![body(M, [10.5, 64.0, 0.5])]);
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID), vec![(WaypointOp::Track, WaypointPos::Block([10, 64, 0]))]);
    snap.others[0].pos = [100.5, 64.0, 0.5];
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID), vec![(WaypointOp::Track, WaypointPos::Chunk([6, 0]))]);
    snap.others[0].pos = [400.5, 64.0, 0.5];
    let expected = crate::mobs::mth::atan2(0.5 - 400.5, -(0.5 - 0.5)) as f32;
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID), vec![(WaypointOp::Track, WaypointPos::Azimuth(expected))]);
    // 332 exactly is not "really far": the direction gives way to a chunk.
    snap.others[0].pos = [332.5, 64.0, 0.5];
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID), vec![(WaypointOp::Track, WaypointPos::Chunk([20, 0]))]);
}

/// The game's quirk: a block connection only breaks on a jump of more than one block, so a
/// transmitter walking away one block at a time stays an exact block well out of view.
/// Sabotage verified (failure): the block connection also broken when its chunk leaves the view.
#[test]
fn slow_walk_keeps_the_exact_block() {
    let mut w = Waypoints::default();
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5])], vec![body(M, [5.5, 64.0, 0.5])]);
    sync(&mut w, &snap);
    for x in 6..=120 {
        snap.others[0].pos = [f64::from(x) + 0.5, 64.0, 0.5];
        let out = told(&sync(&mut w, &snap), 1, M_UUID);
        assert_eq!(out, vec![(WaypointOp::Update, WaypointPos::Block([x, 64, 0]))], "step to {x}");
    }
    // The same place again: nothing to tell.
    assert!(sync(&mut w, &snap).is_empty());
}

/// No connection before the transmitter's first tick; after it, the next move (of either side)
/// makes one. Ranges: the smaller of transmit and receive decides. Sabotage verified (failure):
/// first tick ignored; the transmitter's range alone compared.
#[test]
fn first_tick_and_the_smaller_range() {
    let mut w = Waypoints::default();
    let mut m = body(M, [10.5, 64.0, 0.5]);
    m.ticked = false;
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5])], vec![m]);
    assert!(told(&sync(&mut w, &snap), 1, M_UUID).is_empty(), "connected before its first tick");
    snap.others[0].ticked = true;
    assert!(sync(&mut w, &snap).is_empty(), "nothing moved, nothing made");
    snap.players[0].sent_movement = true;
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID).len(), 1, "receiver's movement packet connects");
    snap.players[0].sent_movement = false;

    let mut w = Waypoints::default();
    let mut r = player(R, [0.5, 64.0, 0.5]);
    r.receive_range = 8.0;
    let snap = world(vec![r], vec![body(M, [10.5, 64.0, 0.5])]);
    assert!(sync(&mut w, &snap).is_empty(), "out of the receiver's range");
    let mut snap = snap;
    snap.others[0].pos = [7.5, 64.0, 0.5];
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID).len(), 1, "within both ranges");
    // Exactly at the range: out (the game compares with `>=`).
    snap.others[0].pos = [8.5, 64.0, 0.5];
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID), vec![(WaypointOp::Untrack, WaypointPos::Empty)]);
}

/// A spectator transmitter shows to nobody but spectators; a receiver never sees what it rides.
/// Sabotage verified (failure): spectator receiver ignored like the others; vehicles left out.
#[test]
fn spectators_and_riders() {
    const P: i32 = 2;
    let mut w = Waypoints::default();
    let mut ghost = player(P, [10.5, 64.0, 0.5]);
    ghost.body.spectator = true;
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5]), ghost], Vec::new());
    let out = sync(&mut w, &snap);
    assert!(told(&out, 1, 1002).is_empty(), "spectator shown to a player");
    assert_eq!(told(&out, 2, 1001).len(), 1, "a spectator sees the player");
    snap.players[0].body.spectator = true;
    snap.players[0].sent_movement = true;
    assert_eq!(told(&sync(&mut w, &snap), 1, 1002).len(), 1, "a spectator sees the spectator");

    let mut w = Waypoints::default();
    let mut rider = player(R, [0.5, 64.0, 0.5]);
    rider.vehicles = vec![77, M];
    let snap = world(vec![rider], vec![body(M, [0.5, 63.0, 0.5])]);
    assert!(sync(&mut w, &snap).is_empty(), "the mount shows to its rider");
}

/// A far transmitter's direction goes out again only past half a degree. Sabotage verified
/// (failure): any change sent.
#[test]
fn direction_resent_past_half_a_degree() {
    let mut w = Waypoints::default();
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5])], vec![body(M, [1000.5, 64.0, 0.5])]);
    sync(&mut w, &snap);
    // 2 blocks sideways at 1000 blocks: about 0.11 degree.
    snap.others[0].pos = [1000.5, 64.0, 2.5];
    assert!(sync(&mut w, &snap).is_empty(), "a tenth of a degree sent");
    snap.others[0].pos = [1000.5, 64.0, 20.5];
    let out = told(&sync(&mut w, &snap), 1, M_UUID);
    assert!(matches!(out.as_slice(), [(WaypointOp::Update, WaypointPos::Azimuth(_))]), "{out:?}");
}

/// `locator_bar` off: every connection ended, nothing made while off, everyone updated when it
/// comes back. Sabotage verified (failure): connections kept when the rule goes off.
#[test]
fn game_rule_off_and_on() {
    let mut w = Waypoints::default();
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5])], vec![body(M, [10.5, 64.0, 0.5])]);
    sync(&mut w, &snap);
    snap.locator_bar = false;
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID), vec![(WaypointOp::Untrack, WaypointPos::Empty)]);
    snap.others[0].pos = [11.5, 64.0, 0.5];
    snap.players[0].sent_movement = true;
    assert!(sync(&mut w, &snap).is_empty(), "made while the rule is off");
    snap.players[0].sent_movement = false;
    snap.locator_bar = true;
    assert_eq!(told(&sync(&mut w, &snap), 1, M_UUID), vec![(WaypointOp::Track, WaypointPos::Block([11, 64, 0]))]);
}

/// The game's quirk: a player whose receive range falls to zero leaves the receivers AND stops
/// being tracked, although it still transmits; the others lose it until it is tracked again.
/// Sabotage verified (failure): the removal leaves its own waypoint tracked.
#[test]
fn deaf_player_also_disappears() {
    const A: i32 = 2;
    let mut w = Waypoints::default();
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5]), player(A, [5.5, 64.0, 0.5])], Vec::new());
    snap.players[1].sent_movement = true;
    let out = sync(&mut w, &snap);
    assert_eq!(told(&out, 1, 1002).len(), 1, "R sees A");
    assert_eq!(told(&out, 2, 1001).len(), 1, "A sees R");
    snap.players[1].sent_movement = false;
    snap.players[1].receive_range = 0.0;
    let out = sync(&mut w, &snap);
    assert_eq!(told(&out, 2, 1001), vec![(WaypointOp::Untrack, WaypointPos::Empty)], "A told R is gone");
    assert_eq!(told(&out, 1, 1002), vec![(WaypointOp::Untrack, WaypointPos::Empty)], "R told A is gone");
    move_player(&mut snap.players[1], [6.5, 64.0, 0.5]);
    move_player(&mut snap.players[0], [1.5, 64.0, 0.5]);
    assert!(sync(&mut w, &snap).is_empty(), "A came back without being tracked again");
    assert_eq!(w.tracked(), vec![R]);
}

/// `/waypoint modify`: ended then made again with the new icon; an entity with no range is then
/// tracked (listed) without any connection. Sabotage verified (failure): retrack skipped.
#[test]
fn modify_tracks_again_with_the_new_icon() {
    let mut w = Waypoints::default();
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5])], vec![body(M, [10.5, 64.0, 0.5])]);
    sync(&mut w, &snap);
    snap.others[0].icon = Icon { style: "minecraft:bowtie", color: Some(0xFF_0000) };
    w.retrack(M, false);
    let out = sync(&mut w, &snap);
    let ours: Vec<_> = out.iter().filter(|o| o.packet.id == WaypointId::Uuid(M_UUID)).collect();
    assert_eq!(ours.len(), 2);
    assert_eq!(ours[0].packet.op, WaypointOp::Untrack);
    assert_eq!((ours[1].packet.op, ours[1].packet.style.as_str(), ours[1].packet.color), (WaypointOp::Track, "minecraft:bowtie", Some(0xFF_0000)));

    const QUIET: i32 = 60;
    let mut quiet = body(QUIET, [3.5, 64.0, 0.5]);
    quiet.transmit_range = 0.0;
    w.retrack(QUIET, false);
    snap.others.push(quiet);
    assert!(told(&sync(&mut w, &snap), 1, 1060).is_empty(), "no range, no connection");
    assert_eq!(w.tracked(), vec![R, M, QUIET], "listed in tracking order");
}

/// The colour a connection copies stays until it is made again: a team change remakes a
/// player's connections, never a mob's (the game's quirk). Sabotage verified (failure): every
/// update sends the current icon.
#[test]
fn team_colour_copied_into_the_connection() {
    const A: i32 = 2;
    let mut w = Waypoints::default();
    let mut m = body(M, [10.5, 64.0, 0.5]);
    m.icon.color = Some(0xFF_5555);
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5]), player(A, [5.5, 64.0, 0.5])], vec![m]);
    snap.players[1].sent_movement = true;
    sync(&mut w, &snap);
    snap.players[1].sent_movement = false;
    snap.others[0].icon.color = Some(0x55_55FF);
    snap.others[0].pos = [11.5, 64.0, 0.5];
    let out = sync(&mut w, &snap);
    let update = out.iter().find(|o| o.seat == Some(1) && o.packet.id == WaypointId::Uuid(M_UUID)).unwrap();
    assert_eq!((update.packet.op, update.packet.color), (WaypointOp::Update, Some(0xFF_5555)), "mob keeps its old colour");
    snap.players[1].body.icon.color = Some(0x55_55FF);
    w.remake(A);
    let out = sync(&mut w, &snap);
    assert_eq!(
        out.iter().find(|o| o.packet.id == WaypointId::Uuid(1002)).map(|o| (o.seat, o.packet.op, o.packet.color)),
        Some((Some(1), WaypointOp::Track, Some(0x55_55FF))),
        "player remade with its team colour"
    );
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Micro-bench of the pass (`cargo test --release -p rc-sim --lib bench_waypoint_pass -- --ignored
/// --nocapture`), 7 alternated passes, median:
/// - the rules alone (`Waypoints::sync`) on made-up snapshots: 1 or 8 players, 0 or 100 mobs
///   that transmit, everyone moving a little each pass; each pair refreshed once or twice
///   (`refresh_pair_once`);
/// - the whole server pass (`waypoints_sync`: snapshot of the players, scan of the mobs, rules)
///   in a flat world of 0, 1,000 and 5,000 mobs, none transmitting (the usual case, mob scan
///   skipped or not) then 1 % of them; microseconds and allocations per pass.
#[test]
#[ignore = "micro-bench"]
fn bench_waypoint_pass() {
    for (players, mobs) in [(1, 0), (8, 0), (1, 100), (8, 100)] {
        let snap = world(
            (1..=players).map(|i| player(i, [f64::from(i) * 3.0, 64.0, 0.5])).collect(),
            (0..mobs).map(|i| body(100 + i, [f64::from(i % 40) * 9.0, 64.0, f64::from(i / 40) * 9.0])).collect(),
        );
        let mut cases: Vec<(bool, Waypoints, Snapshot, Vec<f64>)> = [false, true]
            .into_iter()
            .map(|once| {
                let mut w = Waypoints::default();
                w.refresh_pair_once = once;
                w.sync(&snap, &mut Vec::new());
                (once, w, snap.clone(), Vec::new())
            })
            .collect();
        let rounds = 2_000;
        let mut out = Vec::new();
        for _pass in 0..7 {
            for (_, w, snap, times) in &mut cases {
                let start = std::time::Instant::now();
                for step in 0..rounds {
                    let dx = if step % 2 == 0 { 0.3 } else { -0.3 };
                    snap.players.iter_mut().for_each(|p| p.body.pos[0] += dx);
                    snap.others.iter_mut().for_each(|m| m.pos[2] += dx);
                    out.clear();
                    w.sync(snap, &mut out);
                    std::hint::black_box(out.len());
                }
                times.push(start.elapsed().as_secs_f64() / f64::from(rounds));
            }
        }
        for (once, _, _, times) in &mut cases {
            let how = if *once { "each pair once" } else { "pairs twice" };
            println!("[bench] waypoint rules, {players} players, {mobs} transmitting mobs, {how}: {:.2} µs per pass", median(times) * 1e6);
        }
    }
    for n in [0usize, 1000, 5000] {
        for (transmitting, skip_idle_scan) in [(false, false), (false, true), (true, true)] {
            let (mut p, _dir) = crate::tests_g3f13::open(&format!("waypoint-bench-{n}-{transmitting}-{skip_idle_scan}"));
            p.s.waypoints.skip_idle_mob_scan = skip_idle_scan;
            for i in 0..n {
                let at = [f64::from((i % 70) as u32) + 0.5, -60.0, f64::from((i / 70) as u32) + 0.5];
                let mut m = p.s.new_mob(crate::mobs::species::Kind::Cow, at, 0.0);
                m.tick_count = 1;
                if transmitting && i % 100 == 0 {
                    m.attrs.by_name_mut(crate::waypoints::TRANSMIT_RANGE).unwrap().base = 100.0;
                }
                p.s.add_mob(m);
            }
            p.s.waypoints_sync();
            let rounds = 500;
            let mut times = Vec::new();
            let mut allocations = 0;
            for _pass in 0..7 {
                let before = crate::test_alloc::allocations();
                let start = std::time::Instant::now();
                for step in 0..rounds {
                    p.s.pos[0] += if step % 2 == 0 { 0.3 } else { -0.3 };
                    p.s.waypoints_sync();
                }
                times.push(start.elapsed().as_secs_f64() / f64::from(rounds));
                allocations = (crate::test_alloc::allocations() - before) / rounds as u64;
            }
            let what = match (transmitting, skip_idle_scan) {
                (true, _) => "1 % transmitting",
                (false, true) => "none transmitting, idle scan skipped",
                (false, false) => "none transmitting, list walked",
            };
            println!("[bench] waypoint server pass, {n} mobs ({what}): {:.2} µs per pass, {allocations} allocations", median(&mut times) * 1e6);
        }
    }
}

/// Departures: a transmitter gone is ended for its receivers; a respawned player (new entity)
/// is told its connections ended and is ended for the others, then shows again after its first
/// tick. Sabotage verified (failure): generation ignored.
#[test]
fn departures_and_respawn() {
    const A: i32 = 2;
    let mut w = Waypoints::default();
    let mut snap = world(vec![player(R, [0.5, 64.0, 0.5]), player(A, [5.5, 64.0, 0.5])], vec![body(M, [10.5, 64.0, 0.5])]);
    snap.players[1].sent_movement = true;
    sync(&mut w, &snap);
    snap.players[1].sent_movement = false;
    snap.others.clear();
    let out = sync(&mut w, &snap);
    assert_eq!(told(&out, 1, M_UUID), vec![(WaypointOp::Untrack, WaypointPos::Empty)]);
    assert_eq!(told(&out, 2, M_UUID), vec![(WaypointOp::Untrack, WaypointPos::Empty)]);
    snap.players[1].generation = 2;
    snap.players[1].body.ticked = false;
    let out = sync(&mut w, &snap);
    // The old entity's connection ends, the new one is connected at once (R is past its first tick).
    let again = vec![(WaypointOp::Untrack, WaypointPos::Empty), (WaypointOp::Track, WaypointPos::Block([0, 64, 0]))];
    assert_eq!(told(&out, 2, 1001), again, "respawned player");
    assert_eq!(told(&out, 1, 1002), vec![(WaypointOp::Untrack, WaypointPos::Empty)], "R told the old A ended");
    snap.players[1].body.ticked = true;
    move_player(&mut snap.players[1], [6.5, 64.0, 0.5]);
    assert_eq!(told(&sync(&mut w, &snap), 1, 1002).len(), 1, "A shows again once it moved after its first tick");
    snap.players.truncate(1);
    let out = sync(&mut w, &snap);
    assert_eq!(told(&out, 1, 1002), vec![(WaypointOp::Untrack, WaypointPos::Empty)], "A left");
    assert!(out.iter().all(|o| o.seat == Some(1)), "nothing sent to the one who left");
}
