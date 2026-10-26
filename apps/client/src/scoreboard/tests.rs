//! Sidebar kept per revision: equivalence with the full recompute, and micro-bench.

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
