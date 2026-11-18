//! Player list: sorting and faces, equivalence of the kept rows with the old draw (frozen in
//! `reference.rs`), and the micro-bench of a full list drawn every frame.

mod reference;

use super::*;
use rc_proto_v777::players::{InfoEntry, InfoUpdate};
use rc_proto_v777::scores::{ScoreEvent, TeamParams};

use crate::gui::tests_opt::{self, Env};

#[test]
fn names_sort_like_the_game() {
    use std::cmp::Ordering::*;
    assert_eq!(cmp_ignore_case("alice", "Bob"), Less);
    assert_eq!(cmp_ignore_case("ABC", "abc"), Equal);
    assert_eq!(cmp_ignore_case("_x", "a"), Less);
}

#[test]
fn face_hat_and_flip() {
    let face: Vec<u8> = (0..64).flat_map(|i| [i as u8, 0, 0, 255]).collect();
    let mut hat = vec![0u8; 256];
    hat[3] = 255;
    hat[1] = 200;
    let up = compose(&face, &hat, true, false);
    assert_eq!(&up[0..4], &[0, 200, 0, 255]);
    assert_eq!(&compose(&face, &hat, false, false)[0..4], &[0, 0, 0, 255]);
    // Flipped: the first displayed row is the last one of the skin.
    assert_eq!(compose(&face, &hat, false, true)[0], 56);
}

#[test]
fn hidden_alone_on_integrated_server() {
    let mut players = PlayerList::default();
    players.map.insert(1, PlayerEntry { listed: true, ..Default::default() });
    let scores = Scoreboard::default();
    assert!(!visible(true, &players, &scores));
    assert!(visible(false, &players, &scores));
    players.map.insert(2, PlayerEntry { listed: true, ..Default::default() });
    assert!(visible(true, &players, &scores));
}

fn text(s: &str) -> Option<Nbt> {
    Some(Nbt::String(s.to_owned()))
}

/// Colored text component (display names, prefixes).
fn colored(s: &str, color: &str) -> Option<Nbt> {
    Some(Nbt::Compound(vec![("text".into(), Nbt::String(s.into())), ("color".into(), Nbt::String(color.into()))]))
}

/// Profile name of player `i`: case variants and accents (ties of the case-insensitive sort),
/// and the two names whose head is flipped.
fn player_name(i: usize) -> String {
    match i % 23 {
        0 => "Dinnerbone".into(),
        1 => "Grumm".into(),
        2 => format!("alex{}", i / 23),
        3 => format!("Alex{}", i / 23),
        4 => format!("Zoé_{i}"),
        _ => format!("Player{i}"),
    }
}

const LATENCIES: [i32; 7] = [-1, 40, 149, 299, 599, 999, 2500];

/// Player `i` added and listed: game mode (spectators every 7), list order, latency, own
/// display name every 5, hat every other.
fn added(i: usize) -> InfoEntry {
    InfoEntry {
        uuid: 1000 + i as u128,
        profile: Some((player_name(i), Vec::new())),
        game_mode: Some(if i.is_multiple_of(7) { 3 } else { (i % 3) as i32 }),
        listed: Some(true),
        latency: Some(LATENCIES[i % LATENCIES.len()]),
        display_name: Some(if i.is_multiple_of(5) { colored(&format!("Nick {i}"), "gold") } else { None }),
        list_order: Some((i % 4) as i32),
        show_hat: Some(i.is_multiple_of(2)),
        ..Default::default()
    }
}

/// `n` listed players and 10 unlisted.
fn crowd_players(n: usize) -> PlayerList {
    let mut players = PlayerList::default();
    let mut entries: Vec<InfoEntry> = (0..n).map(added).collect();
    entries.extend((n..n + 10).map(|i| InfoEntry { listed: Some(false), ..added(i) }));
    players.apply(InfoUpdate { actions: 0xff, entries });
    players
}

fn team_params(prefix: Option<Nbt>, suffix: Option<Nbt>, color: Option<i32>) -> TeamParams {
    TeamParams { display: text("team"), prefix, suffix, nametag: 0, collision: 0, color, options: 0 }
}

/// Three teams (prefix, suffix, color) sharing the players, and a "list" objective shown in
/// the list: hearts or values (large numbers, so that the text column is wide).
fn crowd_scores(n: usize, hearts: bool) -> Scoreboard {
    let mut sb = Scoreboard::default();
    let teams = [
        ("red", team_params(colored("[R] ", "red"), None, Some(12))),
        ("blue", team_params(None, text(" *"), Some(9))),
        ("plain", team_params(None, None, None)),
    ];
    for (t, (name, params)) in teams.into_iter().enumerate() {
        let members = (0..n).filter(|i| i % 4 == t).map(player_name).collect();
        sb.apply(ScoreEvent::Team { name: name.into(), mode: 0, params: Some(params), members });
    }
    sb.apply(ScoreEvent::Objective { name: "list".into(), mode: 0, display: text("List"), hearts, format: None });
    sb.apply(ScoreEvent::Display { slot: 0, objective: "list".into() });
    for i in 0..n {
        let value = if hearts { (i * 3 % 41) as i32 } else { (i * 7919 % 100_000) as i32 - 500 };
        sb.apply(ScoreEvent::Score {
            holder: player_name(i),
            objective: "list".into(),
            value,
            display: None,
            format: None,
        });
    }
    sb
}

/// Header and footer on several lines, the footer long enough to wrap.
fn banner() -> (Option<Nbt>, Option<Nbt>) {
    let header = text("§6Welcome to the test server\n§7Second line of the header");
    let footer = Some(Nbt::Compound(vec![
        ("text".into(), Nbt::String("Visit our website for the rules, the shop and the vote links. ".repeat(3))),
        ("color".into(), Nbt::String("aqua".into())),
    ]));
    (header, footer)
}

/// One way of drawing the list, with its own interface and list state.
struct DrawPath {
    name: &'static str,
    gui: Gui,
    tab: TabList,
    /// Draw of the old code (`reference.rs`) instead of [`TabList::draw`].
    reference: bool,
}

impl DrawPath {
    fn new(e: &Env, name: &'static str, opts: Option<TabListOpts>) -> DrawPath {
        let mut tab = TabList::default();
        if let Some(o) = opts {
            tab.opts = o;
        }
        (tab.header, tab.footer) = banner();
        DrawPath { name, gui: tests_opt::gui(e, tests_opt::ALL), tab, reference: opts.is_none() }
    }

    /// The application's draw: new layer, then the list.
    fn draw(&mut self, q: &wgpu::Queue, players: &PlayerList, scores: &Scoreboard, ctx: &Ctx<'_>, window: (u32, u32)) {
        self.gui.begin(window.0, window.1);
        self.gui.next_layer();
        self.tab.set_visible(true);
        if self.reference {
            reference::draw(&mut self.tab, &mut self.gui, q, players, ctx, scores);
        } else {
            self.tab.draw(&mut self.gui, q, players, ctx, scores);
        }
    }
}

/// The old code, then each switch alone and together.
fn all_paths(e: &Env) -> Vec<DrawPath> {
    let opts = |cache, sort_key| Some(TabListOpts { cache, sort_key });
    vec![
        DrawPath::new(e, "old code", None),
        DrawPath::new(e, "rebuilt every frame", opts(false, false)),
        DrawPath::new(e, "sort keys", opts(false, true)),
        DrawPath::new(e, "kept rows", opts(true, false)),
        DrawPath::new(e, "kept rows and sort keys", opts(true, true)),
    ]
}

/// Key action bound only by these proofs (no other test reads it).
const TEST_ACTION: &str = "key.rustcraft.tab_list_test";

/// Changes of the shared player list and scoreboard at fixed frames: one score of the "list"
/// objective alone, a score of another objective, latencies alone, teams, display name and game
/// mode, arrival beyond 80 players, departure, unlisting, list slot emptied then filled, order.
fn change_lists(frame: u32, players: &mut PlayerList, scores: &mut Scoreboard, hearts: bool) {
    let score = |holder: String, objective: &str, value: i32| ScoreEvent::Score {
        holder,
        objective: objective.into(),
        value,
        display: None,
        format: None,
    };
    let entry = |i: usize, e: InfoEntry| InfoEntry { uuid: 1000 + i as u128, ..e };
    let mut update = |entries: Vec<InfoEntry>| players.apply(InfoUpdate { actions: 0xff, entries });
    match frame {
        0 => {
            let keybind = Nbt::Compound(vec![("keybind".into(), Nbt::String(TEST_ACTION.into()))]);
            let translated = Nbt::Compound(vec![("translate".into(), Nbt::String("gui.done".into()))]);
            update(vec![
                entry(6, InfoEntry { display_name: Some(Some(keybind)), ..Default::default() }),
                entry(11, InfoEntry { display_name: Some(Some(translated)), ..Default::default() }),
            ]);
            let other =
                ScoreEvent::Objective { name: "other".into(), mode: 0, display: text("Other"), hearts, format: None };
            scores.apply(other);
        }
        20 => scores.apply(score(player_name(8), "list", if hearts { 3 } else { 123_456 })),
        30 => scores.apply(score(player_name(9), "other", 5)),
        40 => {
            let latency = |i: usize| InfoEntry { latency: Some(LATENCIES[(i + 3) % 7]), ..Default::default() };
            update((10..15).map(|i| entry(i, latency(i))).collect());
        }
        90 => scores.apply(ScoreEvent::Team {
            name: "red".into(),
            mode: 2,
            params: Some(team_params(colored("<Red> ", "dark_red"), text("!"), Some(4))),
            members: Vec::new(),
        }),
        95 => scores.apply(ScoreEvent::Team {
            name: "blue".into(),
            mode: 3,
            params: None,
            members: vec![player_name(12), player_name(13)],
        }),
        100 => {
            let renamed = colored("Renamed", "green");
            update(vec![entry(
                16,
                InfoEntry { display_name: Some(renamed), game_mode: Some(3), ..Default::default() },
            )]);
        }
        110 => update(vec![added(200)]),
        115 => players.remove(&[1017]),
        118 => update(vec![entry(18, InfoEntry { listed: Some(false), ..Default::default() })]),
        170 => scores.apply(ScoreEvent::Display { slot: 0, objective: String::new() }),
        180 => scores.apply(ScoreEvent::Display { slot: 0, objective: "list".into() }),
        185 => update(vec![entry(20, InfoEntry { list_order: Some(9), ..Default::default() })]),
        _ => {}
    }
}

/// Another language: the translated display name of player 11 changes.
fn other_language(e: &Env) -> Lang {
    let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    Lang::load(&base, Some(r#"{"gui.done": "Terminé, merci !"}"#.as_bytes()), None)
}

/// "Force Unicode font" fonts: other widths.
fn unicode_fonts(e: &Env) -> Vec<(String, rc_assets::font::Font)> {
    let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
    let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
    crate::resources::FONT_IDS.iter().map(load).collect()
}

/// Changes of one path at fixed frames: skin of a listed player, then of an unknown one,
/// language, header, footer removed, fonts.
fn change_path(frame: u32, path: &mut DrawPath, e: &Env, skin: &rc_assets::texture::Image) {
    match frame {
        50 => path.tab.skin(1008, skin),
        52 => path.tab.skin(999_999, skin),
        60 => path.gui.lang = std::sync::Arc::new(other_language(e)),
        120 => path.tab.header = text("§eNew header\nand a second line"),
        125 => path.tab.footer = None,
        140 => path.gui.set_fonts(unicode_fonts(e)),
        _ => {}
    }
}

/// Window of a frame (narrower screen for 10 frames: header and footer wrapped again).
fn window(frame: u32) -> (u32, u32) {
    if (70..80).contains(&frame) { (1280, 720) } else { (1600, 900) }
}

/// 80 players over 200 frames with the changes above (plus the bound key changed at frame 130,
/// heads hidden for 5 frames, list hidden for one frame), text scores then hearts: the layers
/// of every path equal those of the old code to the byte on every frame, and the kept rows are
/// really used (rebuilt only after a change).
#[test]
fn tab_list_paths_draw_identical_layers() {
    let Some(e) = tests_opt::env() else { return };
    let skin = default_skin(&e.assets, 7).expect("default skin");
    let default_skin = |u| default_skin(&e.assets, u);
    let in_world = |u: u128| u.is_multiple_of(2);
    for hearts in [false, true] {
        let mut players = crowd_players(80);
        let mut scores = crowd_scores(80, hearts);
        let mut paths = all_paths(&e);
        let mut previous: Option<Vec<u8>> = None;
        let mut frames_changed = 0;
        for frame in 0..200u32 {
            change_lists(frame, &mut players, &mut scores, hearts);
            match frame {
                0 => rc_text::set_keybinds([(TEST_ACTION.to_owned(), "key.keyboard.j".to_owned())]),
                130 => rc_text::set_keybinds([(TEST_ACTION.to_owned(), "key.keyboard.k".to_owned())]),
                _ => {}
            }
            let online = !(150..155).contains(&frame);
            let tick = i64::from(frame);
            let ctx = Ctx { online, tick, background: 0x20ff_ffff, in_world: &in_world, default_skin: &default_skin };
            let mut want: Option<Vec<u8>> = None;
            for path in &mut paths {
                change_path(frame, path, &e, &skin);
                if frame == 160 {
                    path.tab.set_visible(false);
                    continue;
                }
                path.draw(&e.queue, &players, &scores, &ctx, window(frame));
                let got = tests_opt::layer_bytes(&path.gui);
                let Some(want) = &want else {
                    want = Some(got);
                    continue;
                };
                let first = want.iter().zip(&got).position(|(a, b)| a != b);
                assert!(
                    got.len() == want.len() && first.is_none(),
                    "hearts {hearts}, frame {frame}, {}: layers differ (byte {first:?}, {} against {} bytes)",
                    path.name,
                    got.len(),
                    want.len()
                );
            }
            if let Some(want) = want {
                frames_changed += usize::from(previous.as_ref() != Some(&want));
                previous = Some(want);
            }
        }
        assert!(frames_changed >= 15, "hearts {hearts}: only {frames_changed} frames differ from the previous one");
        for path in paths.iter().filter(|p| !p.reference && p.tab.opts.cache) {
            let n = path.tab.rebuilds;
            assert!((15..=40).contains(&n), "hearts {hearts}, {}: {n} rebuilds in 200 frames", path.name);
        }
    }
}

/// Micro-bench of the list held open: 80 listed players (teams with prefixes and colors, own
/// display names, spectators), hearts or text scores, header and footer on several lines,
/// heads (online server); the old code and each switch, drawn in turn on every frame (a load
/// that drifts during the run weighs on all of them alike). Main thread by nature (draw calls
/// into the interface, a few tens of µs: nothing to expect from other threads or the GPU).
/// `cargo test --release -p rc-app tablist_bench -- --ignored --nocapture`.
#[test]
#[ignore = "bench"]
fn tablist_bench() {
    use std::time::{Duration, Instant};
    let Some(e) = tests_opt::env() else { return };
    let default_skin = |u| default_skin(&e.assets, u);
    let in_world = |_| true;
    for hearts in [false, true] {
        let players = crowd_players(80);
        let scores = crowd_scores(80, hearts);
        let kind = if hearts { "hearts" } else { "text" };
        let once = |path: &mut DrawPath, tick: i64| {
            let ctx =
                Ctx { online: true, tick, background: 0x20ff_ffff, in_world: &in_world, default_skin: &default_skin };
            path.draw(&e.queue, &players, &scores, &ctx, (1600, 900));
        };
        let mut paths = all_paths(&e);
        let mut spent = vec![Duration::ZERO; paths.len()];
        let frames = 2000;
        for tick in 0..50 + i64::from(frames) {
            for (path, spent) in paths.iter_mut().zip(&mut spent) {
                let t = Instant::now();
                once(path, tick);
                if tick >= 50 {
                    *spent += t.elapsed();
                }
            }
        }
        for (path, spent) in paths.iter_mut().zip(&spent) {
            let per_frame = spent.as_secs_f64() * 1e6 / f64::from(frames);
            let start = 50 + i64::from(frames);
            let bytes = crate::debug_sys::allocated_bytes(|| (start..start + 100).for_each(|t| once(path, t))) / 100;
            println!(
                "80 players, {kind:>6} scores, {:<24}: {per_frame:>8.2} us, {bytes:>7} bytes allocated per frame",
                path.name
            );
        }
    }
}
