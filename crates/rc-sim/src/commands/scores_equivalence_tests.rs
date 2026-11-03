//! Equivalence proof of the scoreboard: 10,000 random operations with a fixed seed (scores
//! written by every command shape, resets, objectives, display slots, teams filled, joined and
//! left, entities removed, saves re-read) and, after every operation, its results, the packets
//! queued for the client and the save flag; at checkpoints, the encoded save (`scoreboard.dat`
//! bytes), the holder listing, the team, push rule, score list and decorated name of every holder
//! and member. Everything is folded into one digest, frozen from the board before the optimisation
//! (holders in a list searched linearly, teams scanned): any change of an observable result fails.

use rc_proto::nbt::Nbt;
use rc_text::Component;

use super::scores::{Board, BoardPaths, FORCED_PATHS, Objective, ScoreTarget, Team};
use crate::nbtio::uuid_text;

const OPERATIONS: usize = 10_000;
const CHECKPOINT_EVERY: usize = 25;
const REREAD_EVERY: usize = 1_000;

/// Digest of the trace (55,292 lines), recorded on the board before the optimisation.
const FROZEN_DIGEST: u64 = 0x0ff2_bf3d_ce5a_c03c;

const OBJECTIVES: [&str; 6] = ["a", "b", "trig", "deaths", "hp", "Zoé"];
const CRITERIA: [&str; 4] = ["dummy", "trigger", "deathCount", "health"];
const TEAMS: [&str; 5] = ["red", "blue", "green", "t3", "Équipe"];
const RULES: [&str; 4] = ["always", "never", "pushOtherTeams", "pushOwnTeam"];
const OPERATORS: [&str; 10] = ["=", "+=", "-=", "*=", "/=", "%=", "<", ">", "><", "+="];

/// xorshift64*: the same sequence on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len())]
    }

    /// Mostly small values, sometimes the extremes (wrapping arithmetic).
    fn value(&mut self) -> i32 {
        match self.below(10) {
            0 => i32::MAX,
            1 => i32::MIN,
            2 => 0,
            _ => self.below(30) as i32 - 8,
        }
    }
}

/// Holder names: plain names, entity UUIDs in canonical text, the same UUIDs in a form that is
/// not canonical (never an entity's name), and a long tail of rarely used names (table growth).
struct Pool {
    frequent: Vec<String>,
    rare: Vec<String>,
    /// UUIDs whose removal is tried: those of the canonical holders, those behind the
    /// non-canonical forms, and UUIDs never used as a name.
    removed_uuids: Vec<u128>,
}

impl Pool {
    fn new() -> Pool {
        let mut rng = Rng(0x5C0E_B0A2_D000_0001);
        let uuids: Vec<u128> = (0..24).map(|_| u128::from(rng.next()) << 64 | u128::from(rng.next())).collect();
        let mut frequent: Vec<String> =
            ["Steve", "Alex", "#fake", "#count", "Zoé", "jeb_", "a", "b", "c", "Notch"].map(String::from).to_vec();
        frequent.extend(uuids.iter().map(|u| uuid_text(*u)));
        frequent.extend(uuids[..4].iter().map(|u| uuid_text(*u).to_uppercase()));
        // Read as a UUID by the parser, but not its canonical text.
        frequent.push("1-2-3-4-5".into());
        let short = super::snbt::parse_uuid("1-2-3-4-5").expect("short UUID form");
        let mut removed_uuids = uuids.clone();
        removed_uuids.push(short);
        removed_uuids.extend((0..8).map(|_| u128::from(rng.next()) << 64 | u128::from(rng.next())));
        let rare = (0..300).map(|i| format!("n{i}")).collect();
        Pool { frequent, rare, removed_uuids }
    }

    fn holder(&self, rng: &mut Rng) -> String {
        if rng.chance(15) {
            self.rare[rng.below(self.rare.len())].clone()
        } else {
            self.frequent[rng.below(self.frequent.len())].clone()
        }
    }

    fn holders(&self, rng: &mut Rng, n: usize) -> Vec<String> {
        (0..n).map(|_| self.holder(rng)).collect()
    }
}

/// Display name the server would copy for an automatic-update objective (none for a plain
/// `#name`, like an absent player).
fn shown_name(holder: &str) -> Option<Nbt> {
    (!holder.starts_with('#')).then(|| Nbt::String(format!("<{holder}>")))
}

fn read_only(criteria: &str) -> bool {
    matches!(criteria, "health" | "food" | "air" | "armor" | "xp" | "level")
}

fn new_team(rng: &mut Rng, pool: &Pool, name: &str) -> Team {
    let count = rng.below(4);
    let mut players = pool.holders(rng, count);
    if rng.chance(10)
        && let Some(first) = players.first().cloned()
    {
        players.push(first); // the same member twice in one team (a hand-edited save)
    }
    Team {
        name: name.into(),
        display: Nbt::String(name.into()),
        color: rng.chance(30).then(|| rng.pick(&["red", "blue", "gold"]).to_owned()),
        friendly_fire: !rng.chance(20),
        see_invisibles: true,
        prefix: if rng.chance(30) { Nbt::String("[".into()) } else { Nbt::String(String::new()) },
        suffix: Nbt::String(String::new()),
        nametag: "always".into(),
        death: "always".into(),
        collision: rng.pick(&RULES).into(),
        players,
    }
}

fn new_objective(rng: &mut Rng, name: &str) -> Objective {
    let criteria = if rng.chance(50) { "dummy" } else { rng.pick(&CRITERIA) };
    Objective {
        name: name.into(),
        criteria: criteria.into(),
        display: Nbt::String(format!("[{name}]")),
        render: if rng.chance(20) { "hearts".into() } else { "integer".into() },
        auto_update: rng.chance(30),
        format: rng.chance(20).then(|| Nbt::Compound(vec![("type".into(), Nbt::String("blank".into()))])),
    }
}

fn random_format(rng: &mut Rng) -> Option<Nbt> {
    match rng.below(4) {
        0 => None,
        1 => Some(Nbt::Compound(vec![("type".into(), Nbt::String("blank".into()))])),
        2 => Some(Nbt::Compound(vec![
            ("type".into(), Nbt::String("fixed".into())),
            ("value".into(), Nbt::String("x".into())),
        ])),
        _ => Some(Nbt::Compound(vec![
            ("type".into(), Nbt::String("styled".into())),
            ("color".into(), Nbt::String("red".into())),
        ])),
    }
}

// ----- Command shapes, as the server runs them on the board -----

fn auto_update(b: &Board, objective: &str) -> bool {
    b.objective(objective).is_some_and(|o| o.auto_update)
}

fn copied_name(holder: &str, copies: bool) -> Option<Nbt> {
    if copies { shown_name(holder) } else { None }
}

/// `score_set` of the server: score opened, then written with the holder's name if asked.
fn score_set(b: &mut Board, holder: &str, objective: &str, value: i32) {
    if b.paths().one_lookup {
        let target = b.score_target(objective);
        b.update_score(holder, objective, target, copied_name(holder, target.auto_update), |_| value);
        return;
    }
    let sync = b.touch(holder, objective);
    let name = if auto_update(b, objective) { shown_name(holder) } else { None };
    b.write(holder, objective, value, name, sync);
}

/// `scoreboard players set|add|remove <holders> <objective> <value>`.
fn players_command(b: &mut Board, op: &str, holders: &[String], objective: &str, v: i32) -> String {
    match b.objective(objective) {
        None => return format!("players {op} {objective}: unknown"),
        Some(o) if read_only(&o.criteria) => return format!("players {op} {objective}: read-only"),
        Some(_) => {}
    }
    let mut out = format!("players {op} {objective} {v}");
    let new_value = |cur: i32| match op {
        "set" => v,
        "add" => cur.wrapping_add(v),
        _ => cur.wrapping_sub(v),
    };
    let target = b.score_target(objective);
    for h in holders {
        let nv = if b.paths().one_lookup {
            b.update_score(h, objective, target, copied_name(h, target.auto_update), new_value)
        } else {
            let nv = new_value(b.get(h, objective).unwrap_or(0));
            score_set(b, h, objective, nv);
            nv
        };
        out += &format!(" {h}={nv}");
    }
    out
}

/// `execute store result score <holders> <objective> run …`.
fn store_command(b: &mut Board, holders: &[String], objective: &str, v: i32) -> String {
    if b.objective(objective).is_none() {
        return format!("store {objective}: unknown");
    }
    for h in holders {
        score_set(b, h, objective, v);
    }
    format!("store {objective} {v} {holders:?}")
}

fn java_floor_div(x: i32, y: i32) -> i32 {
    let q = x.wrapping_div(y);
    if x.wrapping_rem(y) != 0 && ((x ^ y) < 0) { q - 1 } else { q }
}

fn java_floor_mod(x: i32, y: i32) -> i32 {
    let m = x.wrapping_rem(y);
    if m != 0 && ((m < 0) != (y < 0)) { m + y } else { m }
}

/// Write through an open access (`sync` cleared by the first write).
fn score_write(b: &mut Board, holder: &str, objective: &str, v: i32, sync: &mut bool) {
    let name = if auto_update(b, objective) { shown_name(holder) } else { None };
    b.write(holder, objective, v, name, *sync);
    *sync = false;
}

/// `open_for_write` of the server.
fn open_for_write(b: &mut Board, holder: &str, objective: &str) -> (i32, bool) {
    if b.paths().one_lookup {
        return b.open_score(holder, objective);
    }
    let created = b.touch(holder, objective);
    (b.get(holder, objective).unwrap_or(0), created)
}

/// `write_opened` of the server.
fn write_opened(b: &mut Board, holder: &str, objective: &str, target: ScoreTarget, v: i32, sync: &mut bool) {
    if !b.paths().one_lookup {
        return score_write(b, holder, objective, v, sync);
    }
    b.write_score(holder, objective, target, v, copied_name(holder, target.auto_update), *sync);
    *sync = false;
}

/// `scoreboard players operation <targets> <to> <op> <sources> <so>`.
fn operation_command(b: &mut Board, targets: &[String], to: &str, op: &str, sources: &[String], so: &str) -> String {
    match (b.objective(to), b.objective(so)) {
        (Some(t), Some(_)) if !read_only(&t.criteria) => {}
        _ => return format!("operation {to} {so}: refused"),
    }
    let mut out = format!("operation {to} {op} {so}");
    let (to_target, so_target) = (b.score_target(to), b.score_target(so));
    for h in targets {
        let (_, mut tsync) = open_for_write(b, h, to);
        for src in sources {
            let (y, mut ssync) = open_for_write(b, src, so);
            let x = b.get(h, to).unwrap_or(0);
            let v = match op {
                "=" => y,
                "+=" => x.wrapping_add(y),
                "-=" => x.wrapping_sub(y),
                "*=" => x.wrapping_mul(y),
                "/=" | "%=" if y == 0 => return out + " div0",
                "/=" => java_floor_div(x, y),
                "%=" => java_floor_mod(x, y),
                "<" => x.min(y),
                ">" => x.max(y),
                _ => {
                    write_opened(b, h, to, to_target, y, &mut tsync);
                    write_opened(b, src, so, so_target, x, &mut ssync);
                    continue;
                }
            };
            write_opened(b, h, to, to_target, v, &mut tsync);
        }
        out += &format!(" {h}={}", b.get(h, to).unwrap_or(0));
    }
    out
}

/// A criterion changed for a holder (statistics, health…): every objective of the criterion.
fn criteria_command(b: &mut Board, criteria: &str, holder: &str, add: Option<i32>, set: i32) -> String {
    if !b.objectives.iter().any(|o| o.criteria == criteria) {
        return format!("criteria {criteria}: none");
    }
    if b.paths().one_lookup {
        let copies = b.objectives.iter().any(|o| o.criteria == criteria && b.score_target(&o.name).auto_update);
        b.update_criteria(criteria, holder, copied_name(holder, copies), |cur| {
            add.map_or(set, |n| cur.wrapping_add(n))
        });
        return format!("criteria {criteria} {holder}");
    }
    for o in b.objectives_of(criteria) {
        let mut sync = b.touch(holder, &o);
        let cur = b.get(holder, &o).unwrap_or(0);
        let v = add.map_or(set, |n| cur.wrapping_add(n));
        score_write(b, holder, &o, v, &mut sync);
    }
    format!("criteria {criteria} {holder}")
}

/// `scoreboard players enable <holders> <objective>`.
fn enable_command(b: &mut Board, holders: &[String], objective: &str) -> String {
    if b.objective(objective).is_none_or(|o| o.criteria != "trigger") {
        return format!("enable {objective}: refused");
    }
    let mut out = format!("enable {objective}");
    for h in holders {
        let locked = b.score(h, objective).is_none_or(|x| x.locked);
        if locked {
            b.set_locked(h, objective, false);
        }
        out += &format!(" {h}={locked}");
    }
    out
}

/// `trigger <objective>` by `holder`.
fn trigger_command(b: &mut Board, holder: &str, objective: &str) -> String {
    if b.objective(objective).is_none_or(|o| o.criteria != "trigger") {
        return format!("trigger {objective}: refused");
    }
    let Some(sc) = b.score(holder, objective).cloned() else { return format!("trigger {objective}: unprimed") };
    if sc.locked {
        return format!("trigger {objective}: locked");
    }
    let nv = sc.value.wrapping_add(1);
    score_set(b, holder, objective, nv);
    b.set_locked(holder, objective, true);
    format!("trigger {holder} {objective} {nv}")
}

/// `team empty <team>`: one removal per member, like the command.
fn team_empty_command(b: &mut Board, team: &str) -> String {
    let Some(t) = b.team(team).cloned() else { return format!("empty {team}: unknown") };
    let left: Vec<bool> = t.players.iter().map(|m| b.leave_team(m)).collect();
    format!("empty {team} {left:?}")
}

// ----- What the trace reads from the board -----

/// `scoreboard players list <holder>`: objective, value and stored fields, objective order.
fn holder_entries(b: &Board, holder: &str) -> String {
    let Some(m) = b.holder_scores(holder) else { return "-".into() };
    m.iter().map(|(o, sc)| format!("{o}={}/{}/{:?}/{:?}", sc.value, sc.locked, sc.display, sc.format)).collect()
}

/// Push rule of a member as the mob push reads it: (filter active, team rank and rule code).
fn push_rule(b: &Board, holder: &str) -> String {
    let canonical = super::snbt::parse_uuid(holder).filter(|u| uuid_text(*u) == holder);
    let rule = match canonical {
        Some(u) => b.collision_rule_by_uuid(u),
        None => b.collision_rule_by_name(holder),
    };
    format!("{}/{rule:?}", b.collision_rules_active())
}

/// Entity removed from the world (death, kill, despawn), as the server reports it.
fn remove_entity(b: &mut Board, uuid: u128) {
    b.remove_entity(uuid);
}

fn new_board() -> Board {
    Board::default()
}

// ----- Trace -----

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn digest(lines: &[String]) -> u64 {
    lines.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, l| {
        let h = l.bytes().fold(h, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        (h ^ u64::from(b'\n')).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Encoded save of the board (what `scoreboard.dat` holds under `data`).
fn save_bytes(b: &Board) -> Vec<u8> {
    let mut bytes = Vec::new();
    rc_proto::nbt::write_named(&mut bytes, "", &b.to_nbt());
    bytes
}

/// Everything a player or a server can observe of the board at a checkpoint.
fn snapshot(b: &Board, pool: &Pool, trace: &mut Vec<String>) {
    let bytes = save_bytes(b);
    trace.push(format!("save {} {:016x}", bytes.len(), fnv(&bytes)));
    trace.push(format!("holders {}", b.holders().join(",")));
    trace.push(format!("objectives {:?}", b.objectives_listed().iter().map(|o| &o.name).collect::<Vec<_>>()));
    trace.push(format!("teams {:?}", b.teams_listed().iter().map(|t| &t.name).collect::<Vec<_>>()));
    trace.push(format!("join packets {:016x}", fnv(format!("{:?}", b.join_packets()).as_bytes())));
    trace.push(format!("has entries {}", b.has_entries()));
    for o in OBJECTIVES {
        trace.push(format!("objective {o} tracked={}", b.tracked(o)));
    }
    for c in CRITERIA {
        trace.push(format!("criteria {c} objectives={:?}", b.objectives_of(c)));
    }
    // Not traced (it did not exist before): the team found by UUID is the team of its text.
    for u in &pool.removed_uuids {
        let by_text = b.team_of(&uuid_text(*u)).map(|t| &t.name);
        assert_eq!(b.team_of_uuid(*u).map(|t| &t.name), by_text, "team of {u:032x}");
    }
    let members: Vec<String> = b.teams.iter().flat_map(|t| t.players.iter().cloned()).collect();
    for h in pool.frequent.iter().chain(&members) {
        let team = b.team_of(h).map(|t| t.name.clone());
        let decorated = format!("{:?}", b.decorate(h, Component::text(h.clone())));
        trace.push(format!(
            "holder {h} team={team:?} push={} scores={} name={:016x}",
            push_rule(b, h),
            holder_entries(b, h),
            fnv(decorated.as_bytes())
        ));
    }
}

/// The board written to `scoreboard.dat` and read back (a world reopened).
fn reread(b: &Board, dir: &std::path::Path) -> Board {
    let path = dir.join("data").join("minecraft");
    std::fs::create_dir_all(&path).expect("save folder");
    let root = Nbt::Compound(vec![("data".into(), b.to_nbt())]);
    std::fs::write(path.join("scoreboard.dat"), crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).expect("save");
    Board::read(dir)
}

fn random_operation(b: &mut Board, rng: &mut Rng, pool: &Pool) -> String {
    let h = pool.holder(rng);
    let o = rng.pick(&OBJECTIVES);
    match rng.below(100) {
        0..=11 => {
            let count = 1 + rng.below(3);
            let holders = pool.holders(rng, count);
            let op = rng.pick(&["set", "add", "remove"]);
            let v = rng.value();
            players_command(b, op, &holders, o, v)
        }
        12..=17 => {
            let v = rng.value();
            b.set(&h, o, v);
            format!("set {h} {o} {v}")
        }
        18..=21 => {
            let v = rng.value();
            b.set_shown(&h, o, v, shown_name(&h));
            format!("set_shown {h} {o} {v}")
        }
        22..=25 => {
            let created = b.touch(&h, o);
            if rng.chance(50) {
                let v = rng.value();
                b.write(&h, o, v, shown_name(&h), created);
            }
            format!("touch {h} {o} {created}")
        }
        26..=28 => {
            let name = rng.chance(70).then(|| Nbt::String(format!("D{}", rng.below(3))));
            b.set_display(&h, o, name);
            format!("display {h} {o}")
        }
        29..=30 => {
            b.set_number_format(&h, o, random_format(rng));
            format!("numberformat {h} {o}")
        }
        31..=33 => {
            let locked = rng.chance(50);
            b.set_locked(&h, o, locked);
            format!("locked {h} {o} {locked}")
        }
        34..=35 => {
            let count = 1 + rng.below(2);
            enable_command(b, &pool.holders(rng, count), o)
        }
        36..=37 => trigger_command(b, &h, o),
        38..=44 => {
            b.reset(&h, Some(o));
            format!("reset {h} {o}")
        }
        45..=47 => {
            b.reset(&h, None);
            format!("reset {h}")
        }
        48..=53 => {
            let (targets_count, sources_count) = (1 + rng.below(2), 1 + rng.below(2));
            let targets = pool.holders(rng, targets_count);
            let sources = pool.holders(rng, sources_count);
            let (op, so) = (rng.pick(&OPERATORS), rng.pick(&OBJECTIVES));
            operation_command(b, &targets, o, op, &sources, so)
        }
        54..=58 => {
            let criteria = rng.pick(&CRITERIA);
            let add = rng.chance(50).then(|| rng.value());
            let set = rng.value();
            criteria_command(b, criteria, &h, add, set)
        }
        59..=65 => {
            let team = rng.pick(&TEAMS);
            format!("join {h} {team} {}", b.join_team(&h, team))
        }
        66..=69 => format!("leave {h} {}", b.leave_team(&h)),
        70..=72 => {
            b.entity_removed(&h);
            format!("entity removed {h}")
        }
        73..=78 => {
            let u = pool.removed_uuids[rng.below(pool.removed_uuids.len())];
            remove_entity(b, u);
            format!("entity uuid removed {u:032x}")
        }
        79..=82 => {
            let slot = if rng.chance(70) { rng.below(3) } else { rng.below(19) };
            if rng.chance(25) {
                if !b.slots.contains_key(&slot) {
                    return format!("setdisplay {slot} clear: already empty");
                }
                b.set_display_slot(slot, None);
                return format!("setdisplay {slot} clear");
            }
            if b.objective(o).is_none() || b.slots.get(&slot).is_some_and(|x| x == o) {
                return format!("setdisplay {slot} {o}: refused");
            }
            b.set_display_slot(slot, Some(o));
            format!("setdisplay {slot} {o}")
        }
        83..=85 => {
            if b.objective(o).is_some() {
                return format!("add objective {o}: duplicate");
            }
            b.add_objective(new_objective(rng, o));
            b.dirty = true;
            format!("add objective {o}")
        }
        86 => {
            if b.objective(o).is_none() {
                return format!("remove objective {o}: unknown");
            }
            b.remove_objective(o);
            format!("remove objective {o}")
        }
        87..=88 => {
            let team = rng.pick(&TEAMS);
            if b.team(team).is_some() {
                return format!("add team {team}: duplicate");
            }
            let t = new_team(rng, pool, team);
            let players = t.players.clone();
            b.add_team(t);
            b.dirty = true;
            format!("add team {team} {players:?}")
        }
        89 => {
            let team = rng.pick(&TEAMS);
            if b.team(team).is_none() {
                return format!("remove team {team}: unknown");
            }
            b.remove_team(team);
            b.dirty = true;
            format!("remove team {team}")
        }
        90..=91 => {
            let team = rng.pick(&TEAMS);
            let Some(i) = b.teams.iter().position(|t| t.name == team) else { return format!("modify {team}: unknown") };
            let t = &mut b.teams[i];
            match rng.below(4) {
                0 => t.collision = rng.pick(&RULES).into(),
                1 => t.color = rng.chance(50).then(|| "green".to_owned()),
                2 => t.prefix = Nbt::String(format!("{}", rng.below(3))),
                _ => t.friendly_fire = !t.friendly_fire,
            }
            b.touch_teams();
            b.dirty = true;
            format!("modify {team}")
        }
        92..=93 => {
            let start = rng.below(pool.rare.len() - 20);
            let holders = pool.rare[start..start + 20].to_vec();
            players_command(b, "add", &holders, o, 1)
        }
        94 => team_empty_command(b, rng.pick(&TEAMS)),
        _ => {
            let (v, count) = (rng.value(), 1 + rng.below(3));
            store_command(b, &pool.holders(rng, count), o, v)
        }
    }
}

/// Trace of the whole run on boards taking `paths` (the re-read ones included).
fn run(paths: BoardPaths) -> Vec<String> {
    FORCED_PATHS.with(|p| p.set(Some(paths)));
    let pool = Pool::new();
    let mut rng = Rng(0x5C0E_5EED_0000_0042);
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-scores-eq-{}", std::process::id())));
    let mut b = new_board();
    let mut trace = Vec::new();
    for i in 1..=OPERATIONS {
        trace.push(random_operation(&mut b, &mut rng, &pool));
        for e in std::mem::take(&mut b.out) {
            trace.push(format!("  {e:?}"));
        }
        trace.push(format!("  dirty {}", b.dirty));
        b.dirty = false;
        if i % CHECKPOINT_EVERY == 0 {
            b.check_consistent();
            snapshot(&b, &pool, &mut trace);
        }
        if i % REREAD_EVERY == 0 {
            b = reread(&b, &dir);
            assert_eq!(b.paths(), paths, "re-read board on other paths");
            b.check_consistent();
            trace.push("reread".into());
            snapshot(&b, &pool, &mut trace);
        }
    }
    FORCED_PATHS.with(|p| p.set(None));
    trace
}

/// Every combination of the four switches gives the trace of the board before the optimisation.
#[test]
fn random_operations_match_the_frozen_trace_on_every_path() {
    let mut reference: Option<Vec<String>> = None;
    for paths in BoardPaths::every_combination() {
        let trace = run(paths);
        let got = digest(&trace);
        if got != FROZEN_DIGEST {
            let file = std::env::temp_dir().join("scores-trace-mismatch.txt");
            let _ = std::fs::write(&file, trace.join("\n"));
            let first_difference = reference.as_ref().and_then(|r| {
                r.iter().zip(&trace).enumerate().find(|(_, (a, b))| a != b).map(|(i, (a, b))| (i, a.clone(), b.clone()))
            });
            panic!(
                "{paths:?}: trace changed (digest {got:#018x}, {} lines, written to {}); first difference with \
                 the former paths: {first_difference:?}",
                trace.len(),
                file.display()
            );
        }
        reference.get_or_insert(trace);
    }
}
