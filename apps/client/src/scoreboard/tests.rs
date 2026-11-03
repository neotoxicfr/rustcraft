//! Sidebar kept per revision (equivalence with the full recompute), styled lines kept (layers
//! equal to the old draw), borrowed score keys, revision of the player list, micro-benches.

use super::*;

/// Fixed generator (xorshift 64): the sequence of events depends only on the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Holders: case variants (ties of the case-insensitive sort), hidden `#` holder, local player.
const HOLDERS: [&str; 22] = [
    "Steve",
    "alex",
    "Alex",
    "ALEX",
    "bob",
    "Bob",
    "#hidden",
    "zoe",
    "Zoé",
    "ß",
    "SS",
    "ss",
    "İ",
    "i",
    "ΑΣ",
    "ας",
    "carl",
    "dave",
    "eve",
    "_x",
    "x_",
    "\u{1F600}",
];
const OBJECTIVES: [&str; 3] = ["kills", "deaths", "time"];
const TEAMS: [&str; 3] = ["red", "blue", "plain"];
const LOCAL: &str = "Steve";

fn text(s: &str) -> Option<Nbt> {
    Some(Nbt::String(s.to_owned()))
}

fn params(rng: &mut Rng) -> TeamParams {
    TeamParams {
        display: text("team"),
        prefix: (rng.below(2) == 0).then(|| text("[")).flatten(),
        suffix: (rng.below(2) == 0).then(|| text("]")).flatten(),
        nametag: 0,
        collision: 0,
        color: (rng.below(4) != 0).then(|| rng.below(16) as i32),
        options: 0,
    }
}

fn event(rng: &mut Rng) -> ScoreEvent {
    match rng.below(20) {
        0 => ScoreEvent::Objective {
            name: OBJECTIVES[rng.below(3)].to_owned(),
            mode: [0, 0, 1, 2][rng.below(4)],
            display: text(["Kills", "Deaths", "Time"][rng.below(3)]),
            hearts: false,
            format: [None, Some(NumberFormat::Blank), Some(NumberFormat::Fixed(text("-")))][rng.below(3)].clone(),
        },
        1..=2 => {
            // Slot 1 mostly, then team colors, sometimes list or clear.
            let slot = [1, 1, 3 + rng.below(16) as i32, 0, 2][rng.below(5)];
            let objective = if rng.below(6) == 0 { String::new() } else { OBJECTIVES[rng.below(3)].to_owned() };
            ScoreEvent::Display { slot, objective }
        }
        3..=12 => ScoreEvent::Score {
            holder: HOLDERS[rng.below(HOLDERS.len())].to_owned(),
            objective: OBJECTIVES[rng.below(3)].to_owned(),
            value: rng.below(5) as i32 - 2,
            display: (rng.below(8) == 0).then(|| text("alias")).flatten(),
            format: (rng.below(8) == 0).then_some(NumberFormat::Styled(None)),
        },
        13..=15 => ScoreEvent::Reset {
            holder: HOLDERS[rng.below(HOLDERS.len())].to_owned(),
            objective: (rng.below(3) != 0).then(|| OBJECTIVES[rng.below(3)].to_owned()),
        },
        _ => {
            let mode = [0, 1, 2, 2, 3, 3, 4][rng.below(7)];
            let members = (0..rng.below(4)).map(|_| HOLDERS[rng.below(HOLDERS.len())].to_owned()).collect();
            ScoreEvent::Team {
                name: TEAMS[rng.below(3)].to_owned(),
                mode,
                params: matches!(mode, 0 | 2).then(|| params(rng)),
                members,
            }
        }
    }
}

/// Sort key of a displayed holder: value and name (compared case-insensitively).
fn key<'a>(sb: &'a Scoreboard, objective: &str, h: &'a str) -> Option<(i32, &'a str)> {
    Some((sb.scores.get(&(h.to_owned(), objective.to_owned()))?.value, h))
}

/// Kept bar equivalent to the full recompute up to ties: same title, same sequence of
/// keys (value, case-insensitive name), distinct holders, and each line that of its holder.
fn assert_equivalent(sb: &mut Scoreboard, step: usize) {
    let full = sb.sidebar_rows_full(LOCAL);
    let cached = sb.sidebar_rows(LOCAL).cloned();
    let (Some(full), Some(cached)) = (full, cached) else {
        assert_eq!(sb.sidebar_rows_full(LOCAL).is_some(), sb.sidebar_rows(LOCAL).is_some(), "step {step}");
        return;
    };
    let (name, o) = sb.sidebar_objective(LOCAL).expect("displayed objective");
    assert_eq!(cached.title, full.title, "step {step}: title");
    assert_eq!(cached.holders.len(), full.holders.len(), "step {step}: number of lines");
    let mut seen = std::collections::HashSet::new();
    for (i, (hc, hf)) in cached.holders.iter().zip(&full.holders).enumerate() {
        assert!(seen.insert(hc), "step {step}: holder {hc} duplicated");
        let kc = key(sb, name, hc);
        let kf = key(sb, name, hf);
        let same = match (kc, kf) {
            (Some((vc, nc)), Some((vf, nf))) => vc == vf && cmp_ignore_case(nc, nf).is_eq(),
            _ => false,
        };
        assert!(same, "step {step}, line {i}: {hc} ({kc:?}) instead of {hf} ({kf:?})");
        let s = &sb.scores[&(hc.clone(), name.clone())];
        let expected = sb.sidebar_build(o, &[(hc, s)]).rows.remove(0);
        assert_eq!(cached.rows[i], expected, "step {step}, line {i}: formatting of {hc}");
    }
}

/// Random sequence with a fixed seed: after each event, the kept bar equals the recompute.
/// Sabotages verified (failure): revision not changed by a score removal, then by a team
/// update (color).
#[test]
fn kept_sidebar_equals_recompute() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut sb = Scoreboard::default();
    // Start: a displayed, filled objective, the local player in a colored team.
    for (i, o) in OBJECTIVES.iter().enumerate() {
        sb.apply(ScoreEvent::Objective {
            name: (*o).to_owned(),
            mode: 0,
            display: text(o),
            hearts: false,
            format: None,
        });
        for h in HOLDERS {
            sb.apply(ScoreEvent::Score {
                holder: h.to_owned(),
                objective: (*o).to_owned(),
                value: (h.len() + i) as i32 % 3,
                display: None,
                format: None,
            });
        }
    }
    sb.apply(ScoreEvent::Display { slot: 1, objective: "kills".into() });
    let mut drawn = 0;
    for step in 0..20_000 {
        let ev = event(&mut rng);
        sb.apply(ev);
        assert_equivalent(&mut sb, step);
        drawn += usize::from(sb.sidebar_rows(LOCAL).is_some());
    }
    // The sequence must really display something most of the time.
    assert!(drawn > 10_000, "sidebar displayed in only {drawn} steps");
}

/// Change of local player: the objective followed by its team's color is taken up again.
#[test]
fn sidebar_follows_local_player() {
    let mut sb = Scoreboard::default();
    for o in ["a", "b"] {
        sb.apply(ScoreEvent::Objective { name: o.into(), mode: 0, display: text(o), hearts: false, format: None });
        sb.apply(ScoreEvent::Score { holder: "p".into(), objective: o.into(), value: 1, display: None, format: None });
    }
    sb.apply(ScoreEvent::Display { slot: 1, objective: "a".into() });
    sb.apply(ScoreEvent::Display { slot: 3 + 12, objective: "b".into() });
    let mut p = params(&mut Rng(1));
    p.color = Some(12);
    sb.apply(ScoreEvent::Team { name: "r".into(), mode: 0, params: Some(p), members: vec!["q".into()] });
    let title = |sb: &mut Scoreboard, local: &str| sb.sidebar_rows(local).and_then(|r| r.title.clone());
    assert_eq!(title(&mut sb, "p"), component(&text("a")));
    assert_eq!(title(&mut sb, "q"), component(&text("b")));
    assert_eq!(title(&mut sb, "p"), component(&text("a")));
}

/// Synthetic scoreboard: `n` holders in a displayed objective.
fn synthetic(n: usize) -> Scoreboard {
    let mut sb = Scoreboard::default();
    sb.apply(ScoreEvent::Objective { name: "o".into(), mode: 0, display: text("Score"), hearts: false, format: None });
    sb.apply(ScoreEvent::Display { slot: 1, objective: "o".into() });
    let mut rng = Rng(7);
    for i in 0..n {
        sb.apply(ScoreEvent::Score {
            holder: format!("Player{i}"),
            objective: "o".into(),
            value: rng.below(1000) as i32,
            display: None,
            format: None,
        });
    }
    sb
}

/// Micro-bench: per frame, full recompute against kept bar, and cost of a rebuild
/// (selection of the 15 against full sort). `cargo test --release -p rc-app bench_sidebar -- --ignored
/// --nocapture`.
#[test]
#[ignore = "bench"]
fn bench_sidebar() {
    use std::hint::black_box;
    use std::time::Instant;
    for n in [20, 200, 2000] {
        let mut sb = synthetic(n);
        let frames = 2000;
        let time = |f: &mut dyn FnMut()| {
            f();
            let t = Instant::now();
            for _ in 0..frames {
                f();
            }
            t.elapsed().as_secs_f64() * 1e6 / f64::from(frames)
        };
        let full = time(&mut || {
            black_box(sb.sidebar_rows_full(black_box(LOCAL)));
        });
        let select = time(&mut || {
            black_box(sb.sidebar_rows_select(black_box(LOCAL)));
        });
        let cached = time(&mut || {
            black_box(sb.sidebar_rows(black_box(LOCAL)).is_some());
        });
        // Sort from before the fidelity fix (two lowercase strings allocated per comparison), alone.
        let lower = time(&mut || {
            let mut rows: Vec<(&String, &Score)> = sb.scores.iter().map(|((h, _), s)| (h, s)).collect();
            rows.sort_by(|a, b| b.1.value.cmp(&a.1.value).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            black_box(rows.len());
        });
        println!(
            "n = {n:>5}: full {full:>9.2} µs, selection {select:>9.2} µs, kept {cached:>7.3} µs, \
             lowercase sort alone {lower:>9.2} µs"
        );
    }
}

/// Oracle of the sidebar proofs: the draw of the kept rows as it was before the styled lines were
/// kept (flattened, styled and measured on every frame, the score measured twice), frozen unchanged.
fn reference_draw_sidebar(sidebar: &SidebarRows, gui: &mut Gui, queue: &wgpu::Queue, text_background: Option<u32>) {
    let lang = &gui.lang;
    let chars = |c: &Component| rc_text::styled_chars(&rc_text::flatten(c, lang));
    let title = sidebar.title.as_ref().map(chars).unwrap_or_default();
    let lines: Vec<(Vec<StyledChar>, Vec<StyledChar>)> =
        sidebar.rows.iter().map(|(name, score)| (chars(name), score.as_ref().map(chars).unwrap_or_default())).collect();
    gui.next_layer();
    let w = |gui: &Gui, c: &[StyledChar]| rc_text::width(gui, c);
    let lt = w(gui, &title);
    let sep = w(gui, &rc_text::styled_chars(&rc_text::flatten(&Component::text(": "), &gui.lang)));
    let mut width = lt;
    for (n, s) in &lines {
        let ls = w(gui, s);
        width = width.max(w(gui, n) + if ls > 0 { sep + ls } else { 0 });
    }
    let (gw, gh) = (gui.width.floor() as i32, gui.height.floor() as i32);
    let n = lines.len() as i32;
    let bottom = gh / 2 + n * 9 / 3;
    let left = gw - width - 3;
    let right = gw - 1;
    let top = bottom - n * 9;
    let header = crate::hud::background_color(text_background, 0x6600_0000);
    let body = crate::hud::background_color(text_background, 0x4C00_0000);
    gui.fill((left - 2) as f32, (top - 10) as f32, right as f32, (top - 1) as f32, header);
    gui.fill((left - 2) as f32, (top - 1) as f32, right as f32, bottom as f32, body);
    gui.text(queue, &title, (left + width / 2 - lt / 2) as f32, (top - 9) as f32, 0xffff_ffff, false, 0);
    for (i, (name, score)) in lines.iter().enumerate() {
        let y = (bottom - (n - i as i32) * 9) as f32;
        gui.text(queue, name, left as f32, y, 0xffff_ffff, false, 0);
        let ls = w(gui, score);
        gui.text(queue, score, (right - ls) as f32, y, 0xffff_ffff, false, 0);
    }
}

/// Borrowed key of the score table: same hash as the owned key with the table's own hasher, and
/// every lookup (holders, objectives, absent ones) finds the same score as the owned key, after
/// each event of a random sequence. Sabotage verified (failure): fields swapped in the borrowed key.
#[test]
fn borrowed_score_key_finds_the_same_scores() {
    use std::hash::BuildHasher;
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut sb = Scoreboard::default();
    for o in OBJECTIVES {
        sb.apply(ScoreEvent::Objective { name: o.into(), mode: 0, display: text(o), hearts: false, format: None });
    }
    let objectives = OBJECTIVES.iter().copied().chain(["absent", ""]);
    let mut found = 0;
    for step in 0..5_000 {
        sb.apply(event(&mut rng));
        for h in HOLDERS.iter().copied().chain(["nobody"]) {
            for o in objectives.clone() {
                let owned = (h.to_owned(), o.to_owned());
                let hasher = sb.scores.hasher();
                assert_eq!(hasher.hash_one(ScoreKey(h, o)), hasher.hash_one(&owned), "hash of ({h}, {o})");
                let (a, b) = (sb.score_borrowed(h, o), sb.score_owned(h, o));
                assert!(a.map(std::ptr::from_ref) == b.map(std::ptr::from_ref), "step {step}: score of ({h}, {o})");
                found += usize::from(a.is_some());
            }
        }
    }
    assert!(found > 50_000, "only {found} scores found: the sequence must fill the table");
}

/// What the player list shows from the scoreboard for one holder: list score (hearts, value,
/// text), name formatted by the team, team name of the sort.
type ListedHolder = (Option<(bool, i32, Option<Component>)>, Component, Option<String>);

/// [`ListedHolder`] of every holder.
fn list_view(sb: &Scoreboard) -> Vec<ListedHolder> {
    HOLDERS
        .iter()
        .map(|h| {
            let team = sb.team_named(h).map(|t| t.0.to_owned());
            (sb.list_score(h), sb.decorate(h, Component::text(*h)), team)
        })
        .collect()
}

/// After each event of a random sequence where the list slot is often used: whenever what the
/// player list shows changed, the list revision changed too (rows kept by the list are never
/// stale). Sabotages verified (failure): a score of the list objective, then a team update,
/// leaving the revision unchanged.
#[test]
fn list_revision_moves_with_what_the_list_shows() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut sb = Scoreboard::default();
    for o in OBJECTIVES {
        sb.apply(ScoreEvent::Objective { name: o.into(), mode: 0, display: text(o), hearts: false, format: None });
    }
    sb.apply(ScoreEvent::Display { slot: 0, objective: "kills".into() });
    let mut changes = 0;
    for step in 0..20_000 {
        // The list slot taken again regularly (the generator mostly moves the sidebar).
        let ev = if step % 50 == 0 {
            ScoreEvent::Display { slot: 0, objective: OBJECTIVES[rng.below(3)].to_owned() }
        } else {
            event(&mut rng)
        };
        let (before, revision) = (list_view(&sb), sb.list_revision());
        sb.apply(ev);
        if list_view(&sb) != before {
            changes += 1;
            assert_ne!(sb.list_revision(), revision, "step {step}: the list changed, its revision did not");
        }
    }
    assert!(changes > 2_000, "only {changes} changes of the list");
}

/// After each event of a random sequence where slot 2 is often used: whenever the "below name"
/// line of a holder changed, the "below name" revision changed too (keys kept by the name tags are
/// never stale). Sabotage verified (failure): a score of the slot's objective leaving the
/// revision unchanged.
#[test]
fn below_name_revision_moves_with_the_below_lines() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut sb = Scoreboard::default();
    for o in OBJECTIVES {
        sb.apply(ScoreEvent::Objective { name: o.into(), mode: 0, display: text(o), hearts: false, format: None });
    }
    let lines = |sb: &Scoreboard| HOLDERS.iter().map(|h| sb.below_name(h)).collect::<Vec<_>>();
    let mut changes = 0;
    for step in 0..20_000 {
        // Slot 2 taken again regularly (the generator mostly moves the sidebar).
        let ev = if step % 50 == 0 {
            ScoreEvent::Display { slot: 2, objective: OBJECTIVES[rng.below(3)].to_owned() }
        } else {
            event(&mut rng)
        };
        let (before, revision) = (lines(&sb), sb.below_name_revision());
        sb.apply(ev);
        if lines(&sb) != before {
            changes += 1;
            assert_ne!(sb.below_name_revision(), revision, "step {step}: a line changed, its revision did not");
        }
    }
    assert!(changes > 2_000, "only {changes} changes of the lines");
}

/// Sidebar of the same kept rows drawn three ways: the old code, styled again on every frame,
/// kept styled lines. Layers equal to the byte over 120 frames with score changes, a language
/// change (frame 50), new fonts (frame 70) and another local player (frames 90 to 99); the
/// styled lines are really kept (styled again only after a change). Without an installed game
/// or a GPU, skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
#[test]
fn sidebar_styled_cache_draws_identical_layers() {
    use crate::gui::tests_opt;
    let Some(e) = tests_opt::env() else { return };
    let mut reference = tests_opt::gui(&e, tests_opt::ALL);
    let (mut old, mut new) = (tests_opt::gui(&e, tests_opt::ALL), tests_opt::gui(&e, tests_opt::ALL));
    let mut sb = synthetic(40);
    // Translated title and a keybind line, so that the language and the keys show.
    sb.apply(ScoreEvent::Objective {
        name: "o".into(),
        mode: 2,
        display: Some(Nbt::Compound(vec![("translate".into(), Nbt::String("gui.done".into()))])),
        hearts: false,
        format: None,
    });
    let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    let mut rng = Rng(0x0BAD_5EED_1234_5678);
    let mut previous = None;
    let (mut frames_changed, mut drawn) = (0, 0);
    for frame in 0..120u32 {
        if frame % 3 == 0 {
            sb.apply(ScoreEvent::Score {
                holder: format!("Player{}", rng.below(40)),
                objective: "o".into(),
                value: rng.below(1000) as i32,
                display: None,
                format: None,
            });
        }
        for g in [&mut reference, &mut old, &mut new] {
            match frame {
                50 => g.lang = rc_text::Lang::load(&base, Some(r#"{"gui.done": "Terminé"}"#.as_bytes()), None),
                70 => {
                    let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
                    let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
                    g.set_fonts(crate::resources::FONT_IDS.iter().map(load).collect());
                }
                _ => {}
            }
            g.begin(1600, 900);
        }
        let local = if (90..100).contains(&frame) { "Player3" } else { LOCAL };
        if let Some(rows) = sb.sidebar_rows(local) {
            reference_draw_sidebar(rows, &mut reference, &e.queue, None);
            StyledSidebar::build(&old, rows).draw(&mut old, &e.queue, None);
        }
        if let Some(styled) = sb.styled_sidebar(&new, local) {
            styled.draw(&mut new, &e.queue, None);
            drawn += 1;
        }
        let want = tests_opt::layer_bytes(&reference);
        for (name, g) in [("styled every frame", &old), ("kept styled lines", &new)] {
            let got = tests_opt::layer_bytes(g);
            let first = want.iter().zip(&got).position(|(a, b)| a != b);
            assert!(
                got.len() == want.len() && first.is_none(),
                "frame {frame}, {name}: layers differ (byte {first:?})"
            );
        }
        frames_changed += usize::from(previous.as_ref() != Some(&want));
        previous = Some(want);
    }
    assert!(drawn == 120 && frames_changed >= 20, "{drawn} frames drawn, {frames_changed} changed");
    assert!((40..=60).contains(&sb.styled_builds), "{} stylings in 120 frames", sb.styled_builds);
}

/// Micro-bench of the sidebar drawn (15 lines kept): styled again on every frame against the
/// kept styled lines. Main thread by nature (a few µs). Without a GPU: skipped.
/// `cargo test --release -p rc-app bench_sidebar_draw -- --ignored --nocapture`.
#[test]
#[ignore = "bench"]
fn bench_sidebar_draw() {
    use crate::gui::tests_opt;
    use std::time::Instant;
    let Some(e) = tests_opt::env() else { return };
    let mut g = tests_opt::gui(&e, tests_opt::ALL);
    let mut sb = synthetic(200);
    let frames = 5000;
    for styled_cache in [false, true] {
        let mut once = |g: &mut Gui| {
            g.begin(1600, 900);
            if styled_cache {
                if let Some(s) = sb.styled_sidebar(g, LOCAL) {
                    s.draw(g, &e.queue, None);
                }
            } else if let Some(rows) = sb.sidebar_rows(LOCAL) {
                StyledSidebar::build(g, rows).draw(g, &e.queue, None);
            }
        };
        for _ in 0..100 {
            once(&mut g);
        }
        let t = Instant::now();
        for _ in 0..frames {
            once(&mut g);
        }
        let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
        let bytes = crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut g))) / 100;
        let name = if styled_cache { "kept styled lines" } else { "styled every frame" };
        println!("sidebar 15 lines, {name:<20}: {per_frame:>7.2} µs, {bytes:>6} bytes allocated per frame");
    }
}

/// Micro-bench of a score lookup (list score per listed player, "below name" line per entity on
/// every frame): owned key built against borrowed key, 200 holders.
/// `cargo test --release -p rc-app bench_score_lookup -- --ignored --nocapture`.
#[test]
#[ignore = "bench"]
fn bench_score_lookup() {
    use std::hint::black_box;
    use std::time::Instant;
    let sb = synthetic(200);
    let holders: Vec<String> = (0..200).map(|i| format!("Player{i}")).collect();
    let rounds = 2000;
    for borrowed in [false, true] {
        let lookups = || {
            for h in &holders {
                let s = if borrowed { sb.score_borrowed(h, "o") } else { sb.score_owned(h, "o") };
                black_box(s.map(|s| s.value));
            }
        };
        lookups();
        let t = Instant::now();
        for _ in 0..rounds {
            lookups();
        }
        let per_lookup = t.elapsed().as_secs_f64() * 1e9 / f64::from(rounds) / holders.len() as f64;
        let bytes = crate::debug_sys::allocated_bytes(lookups) / holders.len() as u64;
        let name = if borrowed { "borrowed key" } else { "owned key" };
        println!("score lookup, {name:<12}: {per_lookup:>6.1} ns, {bytes:>3} bytes allocated per lookup");
    }
}
