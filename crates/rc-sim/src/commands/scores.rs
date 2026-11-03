//! World scoreboard (`data/minecraft/scoreboard.dat`): objectives, scores,
//! display slots, teams; reading, writing and packets to the client.

use std::collections::BTreeMap;
use std::path::Path;

use rc_proto::nbt::Nbt;
use rc_proto_v777::scores::{NumberFormat, ScoreEvent, TeamParams};
use rc_text::Component;

use crate::commands::hash_order::HashOrder;
use crate::commands::score_holders::{HolderScores, Holders, Reset};
use crate::commands::team_members::TeamMembers;
use crate::nbtio::{int, list};

pub const SLOTS: [&str; 19] = [
    "list",
    "sidebar",
    "below_name",
    "sidebar.team.black",
    "sidebar.team.dark_blue",
    "sidebar.team.dark_green",
    "sidebar.team.dark_aqua",
    "sidebar.team.dark_red",
    "sidebar.team.dark_purple",
    "sidebar.team.gold",
    "sidebar.team.gray",
    "sidebar.team.dark_gray",
    "sidebar.team.blue",
    "sidebar.team.green",
    "sidebar.team.aqua",
    "sidebar.team.red",
    "sidebar.team.light_purple",
    "sidebar.team.yellow",
    "sidebar.team.white",
];

pub const COLORS: [&str; 16] = [
    "black",
    "dark_blue",
    "dark_green",
    "dark_aqua",
    "dark_red",
    "dark_purple",
    "gold",
    "gray",
    "dark_gray",
    "blue",
    "green",
    "aqua",
    "red",
    "light_purple",
    "yellow",
    "white",
];

#[derive(Debug, Clone)]
pub struct Objective {
    pub name: String,
    pub criteria: String,
    pub display: Nbt,
    /// `integer` or `hearts`.
    pub render: String,
    pub auto_update: bool,
    pub format: Option<Nbt>,
}

#[derive(Debug, Clone)]
pub struct Score {
    pub value: i32,
    pub locked: bool,
    pub display: Option<Nbt>,
    pub format: Option<Nbt>,
}

/// New score of the game: locked (a `/trigger` requires `players enable`); only the re-read file
/// has `Locked` false by default.
impl Default for Score {
    fn default() -> Score {
        Score { value: 0, locked: true, display: None, format: None }
    }
}

#[derive(Debug, Clone)]
pub struct Team {
    pub name: String,
    pub display: Nbt,
    pub color: Option<String>,
    pub friendly_fire: bool,
    pub see_invisibles: bool,
    pub prefix: Nbt,
    pub suffix: Nbt,
    pub nametag: String,
    pub death: String,
    pub collision: String,
    pub players: Vec<String>,
}

/// Code of a team collision rule: 0 always, 1 never, 2 other teams, 3 own team.
pub fn collision_code(rule: &str) -> u8 {
    match rule {
        "never" => 1,
        "pushOtherTeams" => 2,
        "pushOwnTeam" => 3,
        _ => 0,
    }
}

/// Team collision rules for the push, former way (`RUSTCRAFT_OPT_TEAMMEMBERINDEX=0`): member →
/// (team rank, rule code), like the game's "member → team" table; rebuilt on demand after a change.
#[derive(Debug, Clone, Default)]
struct CollisionIndex {
    /// At least one team has a rule other than "always" (otherwise the push filters nothing).
    active: bool,
    /// Members whose name is a UUID in canonical text (entities).
    by_uuid: std::collections::HashMap<u128, (u32, u8)>,
    /// Other members (players, free names).
    by_name: std::collections::HashMap<String, (u32, u8)>,
}

/// Which optimised paths a board takes, each behind its switch (`0` restores the former way),
/// read once per process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardPaths {
    /// `RUSTCRAFT_OPT_SCOREHOLDERMAP`: holders in a hash table (`score_holders`).
    pub holder_map: bool,
    /// `RUSTCRAFT_OPT_SCOREONELOOKUP`: `players set/add/remove` and the criteria find each score
    /// they write once (`operation` reads its target again at each source: three holder lookups per
    /// source instead of five), the objective and display state once per command, and no write
    /// copies the objective name of an existing score.
    pub one_lookup: bool,
    /// `RUSTCRAFT_OPT_TEAMMEMBERINDEX`: member → team index kept up to date (`team_members`).
    pub member_index: bool,
    /// `RUSTCRAFT_OPT_SCOREUUIDSET`: an entity removal formats its UUID only when it names a
    /// holder or a member (most removed entities have neither score nor team).
    pub uuid_set: bool,
}

#[cfg(test)]
thread_local! {
    /// Tests: paths of the boards created by this thread (every combination of the equivalence
    /// proofs), without touching the environment.
    pub(crate) static FORCED_PATHS: std::cell::Cell<Option<BoardPaths>> = const { std::cell::Cell::new(None) };
}

impl BoardPaths {
    pub fn from_switches() -> BoardPaths {
        #[cfg(test)]
        if let Some(paths) = FORCED_PATHS.with(std::cell::Cell::get) {
            return paths;
        }
        BoardPaths {
            holder_map: rc_data::opt_switch!("RUSTCRAFT_OPT_SCOREHOLDERMAP"),
            one_lookup: rc_data::opt_switch!("RUSTCRAFT_OPT_SCOREONELOOKUP"),
            member_index: rc_data::opt_switch!("RUSTCRAFT_OPT_TEAMMEMBERINDEX"),
            uuid_set: rc_data::opt_switch!("RUSTCRAFT_OPT_SCOREUUIDSET"),
        }
    }

    /// The 16 combinations of the four switches, all off first.
    #[cfg(test)]
    pub fn every_combination() -> impl Iterator<Item = BoardPaths> {
        (0..16u8).map(|bits| BoardPaths {
            holder_map: bits & 1 != 0,
            one_lookup: bits & 2 != 0,
            member_index: bits & 4 != 0,
            uuid_set: bits & 8 != 0,
        })
    }
}

/// What the writes of one command need to know about their objective, looked up once per
/// command (`RUSTCRAFT_OPT_SCOREONELOOKUP`): neither changes while a command writes its scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreTarget {
    /// The holder's display name is copied into the score (automatic display name update).
    pub auto_update: bool,
    /// Displayed in a slot: every change of a score is sent to the client.
    pub tracked: bool,
}

#[derive(Debug, Clone)]
pub struct Board {
    pub objectives: Vec<Objective>,
    /// Holder → objective → score, and the game's listing order of the holders.
    holders: Holders,
    pub slots: BTreeMap<usize, String>,
    /// Teams. Their options may be edited in place, followed by `touch_teams`; their members
    /// change only through the board's methods (the member index follows them).
    pub teams: Vec<Team>,
    pub dirty: bool,
    /// Packets to send to the client (flushed by the server).
    pub out: Vec<ScoreEvent>,
    /// Listing orders of the game (hash tables replayed): objectives, teams.
    obj_order: HashOrder,
    team_order: HashOrder,
    /// Collision rule index of the former way (empty until the first use after a change).
    collision: std::cell::OnceCell<CollisionIndex>,
    /// Team of every member (`RUSTCRAFT_OPT_TEAMMEMBERINDEX`; empty when off).
    members: TeamMembers,
    /// Some team has a collision rule other than "always" (kept by `touch_teams`).
    collision_rules_active: bool,
    paths: BoardPaths,
}

impl Default for Board {
    fn default() -> Board {
        Board::with_paths(BoardPaths::from_switches())
    }
}

/// Text field of a saved compound.
fn str_field(v: &Nbt, k: &str) -> Option<String> {
    v.get(k).and_then(Nbt::as_str).map(str::to_owned)
}

/// Boolean field of a saved compound (a byte), `default` when absent.
fn bool_field(v: &Nbt, k: &str, default: bool) -> bool {
    int(v, k).map_or(default, |x| x != 0)
}

fn empty_text() -> Nbt {
    Nbt::String(String::new())
}

/// Empty text with no style or child (default value of team prefixes and suffixes).
fn is_empty_text(n: &Nbt) -> bool {
    match n {
        Nbt::String(s) => s.is_empty(),
        Nbt::Compound(f) => matches!(f.as_slice(), [(k, Nbt::String(s))] if k == "text" && s.is_empty()),
        _ => false,
    }
}

/// Saved format → packet. `styled`: the style fields are flat next to `type`
/// (format of the file and of the command); the packet carries the compound without `type`.
pub fn format_net(f: &Option<Nbt>) -> Option<NumberFormat> {
    let f = f.as_ref()?;
    Some(match f.get("type").and_then(Nbt::as_str) {
        Some("blank") => NumberFormat::Blank,
        Some("fixed") => NumberFormat::Fixed(f.get("value").cloned()),
        _ => {
            let style = match f {
                Nbt::Compound(x) => x.iter().filter(|(k, _)| k != "type").cloned().collect(),
                _ => Vec::new(),
            };
            NumberFormat::Styled(Some(Nbt::Compound(style)))
        }
    })
}

/// Score packet sent to the client.
fn score_packet(holder: &str, obj: &str, sc: &Score) -> ScoreEvent {
    ScoreEvent::Score {
        holder: holder.to_owned(),
        objective: obj.to_owned(),
        value: sc.value,
        display: sc.display.clone(),
        format: format_net(&sc.format),
    }
}

/// A write applied to an opened score: `name` (given only when the objective copies the
/// holder's display name) and `value` stored; `true` if the score must be sent (changed, or
/// `sync`: created by this access and not sent yet).
fn apply_write(sc: &mut Score, value: i32, name: Option<Nbt>, sync: bool) -> bool {
    let mut changed = sync;
    if let Some(n) = name
        && sc.display.as_ref().is_none_or(|d| !crate::items::nbt_eq(d, &n))
    {
        sc.display = Some(n);
        changed = true;
    }
    if sc.value != value {
        sc.value = value;
        changed = true;
    }
    changed
}

/// What follows a write [`apply_write`] says must be sent: the packet when clients track the
/// objective, and the save to redo. One place for every write through an opened score.
fn record_write(out: &mut Vec<ScoreEvent>, dirty: &mut bool, tracked: bool, holder: &str, obj: &str, sc: &Score) {
    if tracked {
        out.push(score_packet(holder, obj, sc));
    }
    *dirty = true;
}

/// Objective displayed in at least one slot.
fn displayed(slots: &BTreeMap<usize, String>, obj: &str) -> bool {
    slots.values().any(|o| o == obj)
}

/// Field written only when it differs from its default value (the game's codecs).
fn push_unless_default(fields: &mut Vec<(String, Nbt)>, key: &str, value: Nbt, is_default: bool) {
    if !is_default {
        fields.push((key.into(), value));
    }
}

impl Board {
    /// Empty board taking the given paths.
    pub fn with_paths(paths: BoardPaths) -> Board {
        Board {
            objectives: Vec::new(),
            holders: Holders::new(paths.holder_map, paths.uuid_set),
            slots: BTreeMap::new(),
            teams: Vec::new(),
            dirty: false,
            out: Vec::new(),
            obj_order: HashOrder::new(16, 0.5),
            team_order: HashOrder::new(16, 0.75),
            collision: std::cell::OnceCell::new(),
            members: TeamMembers::default(),
            collision_rules_active: false,
            paths,
        }
    }

    pub fn paths(&self) -> BoardPaths {
        self.paths
    }

    pub fn read(world: &Path) -> Board {
        let d = crate::nbtio::read_dat(&world.join("data").join("minecraft").join("scoreboard.dat"))
            .ok()
            .and_then(|r| r.get("data").cloned());
        let mut b = Board::default();
        let Some(d) = d else { return b };
        for o in list(d.get("Objectives")) {
            let Some(name) = str_field(o, "Name") else { continue };
            b.obj_order.insert(&name);
            b.objectives.push(Objective {
                name,
                criteria: str_field(o, "CriteriaName").unwrap_or_else(|| "dummy".into()),
                display: o.get("DisplayName").cloned().unwrap_or_else(empty_text),
                render: str_field(o, "RenderType").unwrap_or_else(|| "integer".into()),
                auto_update: bool_field(o, "display_auto_update", false),
                format: o.get("format").cloned(),
            });
        }
        for e in list(d.get("PlayerScores")) {
            let (Some(name), Some(obj)) = (str_field(e, "Name"), str_field(e, "Objective")) else { continue };
            let sc = Score {
                value: int(e, "Score").unwrap_or(0) as i32,
                locked: bool_field(e, "Locked", false),
                display: e.get("display").cloned(),
                format: e.get("format").cloned(),
            };
            b.holders.entry(&name).insert(obj, sc);
        }
        if let Some(Nbt::Compound(f)) = d.get("DisplaySlots") {
            for (k, v) in f {
                if let (Some(i), Some(o)) = (SLOTS.iter().position(|x| x == k), v.as_str()) {
                    b.slots.insert(i, o.to_owned());
                }
            }
        }
        for t in list(d.get("Teams")) {
            let Some(name) = str_field(t, "Name") else { continue };
            b.team_order.insert(&name);
            b.teams.push(Team {
                display: t.get("DisplayName").cloned().unwrap_or_else(|| Nbt::String(name.clone())),
                name,
                color: str_field(t, "TeamColor").filter(|c| c != "reset"),
                friendly_fire: bool_field(t, "AllowFriendlyFire", true),
                see_invisibles: bool_field(t, "SeeFriendlyInvisibles", true),
                prefix: t.get("MemberNamePrefix").cloned().unwrap_or_else(empty_text),
                suffix: t.get("MemberNameSuffix").cloned().unwrap_or_else(empty_text),
                nametag: str_field(t, "NameTagVisibility").unwrap_or_else(|| "always".into()),
                death: str_field(t, "DeathMessageVisibility").unwrap_or_else(|| "always".into()),
                collision: str_field(t, "CollisionRule").unwrap_or_else(|| "always".into()),
                players: list(t.get("Players")).iter().filter_map(|p| p.as_str().map(str::to_owned)).collect(),
            });
        }
        b.index_members();
        b.touch_teams();
        b
    }

    /// Writing: like the game's codecs, a field equal to its default value is omitted (an
    /// empty board is written `data: {}`, taken from a save of the official game).
    pub fn to_nbt(&self) -> Nbt {
        let (objectives, scores, teams) = (self.objectives_nbt(), self.scores_nbt(), self.teams_nbt());
        let slots: Vec<(String, Nbt)> =
            self.slots.iter().map(|(i, o)| (SLOTS[*i].to_owned(), Nbt::String(o.clone()))).collect();
        let mut root = Vec::new();
        if !objectives.is_empty() {
            root.push(("Objectives".into(), Nbt::List(objectives)));
        }
        if !scores.is_empty() {
            root.push(("PlayerScores".into(), Nbt::List(scores)));
        }
        if !slots.is_empty() {
            root.push(("DisplaySlots".into(), Nbt::Compound(slots)));
        }
        if !teams.is_empty() {
            root.push(("Teams".into(), Nbt::List(teams)));
        }
        Nbt::Compound(root)
    }

    /// Saved objectives, in the game's stream order.
    fn objectives_nbt(&self) -> Vec<Nbt> {
        let saved = |o: &Objective| {
            let mut f = vec![("Name".into(), Nbt::String(o.name.clone()))];
            push_unless_default(&mut f, "CriteriaName", Nbt::String(o.criteria.clone()), o.criteria == "dummy");
            f.push(("DisplayName".into(), o.display.clone()));
            push_unless_default(&mut f, "RenderType", Nbt::String(o.render.clone()), o.render == "integer");
            push_unless_default(&mut f, "display_auto_update", Nbt::Byte(1), !o.auto_update);
            if let Some(fm) = &o.format {
                f.push(("format".into(), fm.clone()));
            }
            Nbt::Compound(f)
        };
        self.obj_order.iter_stream().filter_map(|n| self.objective(n)).map(saved).collect()
    }

    /// Saved scores: holders in the game's stream order, each holder's objectives in name order.
    fn scores_nbt(&self) -> Vec<Nbt> {
        let mut scores = Vec::new();
        for (h, m) in self.holders.streamed_scores() {
            for (o, sc) in m {
                let mut f =
                    vec![("Name".into(), Nbt::String(h.to_owned())), ("Objective".into(), Nbt::String(o.clone()))];
                push_unless_default(&mut f, "Score", Nbt::Int(sc.value), sc.value == 0);
                push_unless_default(&mut f, "Locked", Nbt::Byte(1), !sc.locked);
                if let Some(d) = &sc.display {
                    f.push(("display".into(), d.clone()));
                }
                if let Some(fm) = &sc.format {
                    f.push(("format".into(), fm.clone()));
                }
                scores.push(Nbt::Compound(f));
            }
        }
        scores
    }

    /// Saved teams, in the game's stream order.
    fn teams_nbt(&self) -> Vec<Nbt> {
        let saved = |t: &Team| {
            let mut f = vec![("Name".into(), Nbt::String(t.name.clone())), ("DisplayName".into(), t.display.clone())];
            if let Some(c) = &t.color {
                f.push(("TeamColor".into(), Nbt::String(c.clone())));
            }
            push_unless_default(&mut f, "AllowFriendlyFire", Nbt::Byte(0), t.friendly_fire);
            push_unless_default(&mut f, "SeeFriendlyInvisibles", Nbt::Byte(0), t.see_invisibles);
            push_unless_default(&mut f, "MemberNamePrefix", t.prefix.clone(), is_empty_text(&t.prefix));
            push_unless_default(&mut f, "MemberNameSuffix", t.suffix.clone(), is_empty_text(&t.suffix));
            push_unless_default(&mut f, "NameTagVisibility", Nbt::String(t.nametag.clone()), t.nametag == "always");
            push_unless_default(&mut f, "DeathMessageVisibility", Nbt::String(t.death.clone()), t.death == "always");
            push_unless_default(&mut f, "CollisionRule", Nbt::String(t.collision.clone()), t.collision == "always");
            let players = Nbt::List(t.players.iter().map(|p| Nbt::String(p.clone())).collect());
            push_unless_default(&mut f, "Players", players, t.players.is_empty());
            Nbt::Compound(f)
        };
        self.team_order.iter_stream().filter_map(|n| self.team(n)).map(saved).collect()
    }

    pub fn objective(&self, name: &str) -> Option<&Objective> {
        self.objectives.iter().find(|o| o.name == name)
    }

    pub fn get(&self, holder: &str, obj: &str) -> Option<i32> {
        self.holders.get(holder)?.get(obj).map(|s| s.value)
    }

    pub fn score(&self, holder: &str, obj: &str) -> Option<&Score> {
        self.holders.get(holder)?.get(obj)
    }

    /// Every score of a holder, by objective name (`players list <holder>`).
    pub fn holder_scores(&self, holder: &str) -> Option<&HolderScores> {
        self.holders.get(holder)
    }

    /// Objective tracked by clients: displayed in at least one slot.
    pub fn tracked(&self, obj: &str) -> bool {
        displayed(&self.slots, obj)
    }

    /// Score created if needed; `true` if it was just created (it will be sent at the first write).
    fn score_mut(&mut self, holder: &str, obj: &str) -> (&mut Score, bool) {
        if self.paths.one_lookup {
            return self.holders.open(holder, obj);
        }
        let m = self.holders.entry(holder);
        let created = !m.contains_key(obj);
        (m.entry(obj.to_owned()).or_default(), created)
    }

    /// Packet of a score (tracked objective only) and save to redo.
    fn score_changed(&mut self, holder: &str, obj: &str) {
        if self.tracked(obj)
            && let Some(sc) = self.score(holder, obj)
        {
            let ev = score_packet(holder, obj, sc);
            self.out.push(ev);
        }
        self.dirty = true;
    }

    /// The objective state a command's writes need, looked up once (see [`ScoreTarget`]).
    pub fn score_target(&self, obj: &str) -> ScoreTarget {
        ScoreTarget { auto_update: self.objective(obj).is_some_and(|o| o.auto_update), tracked: self.tracked(obj) }
    }

    /// Opens a score for a command (created silently if needed, like [`Board::touch`]): its
    /// value and whether this access created it, in one lookup.
    pub fn open_score(&mut self, holder: &str, obj: &str) -> (i32, bool) {
        let (sc, created) = self.holders.open(holder, obj);
        (sc.value, created)
    }

    /// [`Board::write`] through an access opened by [`Board::open_score`], the objective state
    /// given by the command: one lookup.
    pub fn write_score(
        &mut self,
        holder: &str,
        obj: &str,
        target: ScoreTarget,
        value: i32,
        name: Option<Nbt>,
        sync: bool,
    ) {
        let (sc, _) = self.holders.open(holder, obj);
        if apply_write(sc, value, name.filter(|_| target.auto_update), sync) {
            record_write(&mut self.out, &mut self.dirty, target.tracked, holder, obj, sc);
        }
    }

    /// Opens a score and writes `f(current value)` (0 for a new score, sent even unchanged) in one
    /// lookup, like [`Board::touch`] then [`Board::write`]; returns the written value.
    pub fn update_score(
        &mut self,
        holder: &str,
        obj: &str,
        target: ScoreTarget,
        name: Option<Nbt>,
        f: impl FnOnce(i32) -> i32,
    ) -> i32 {
        let (sc, created) = self.holders.open(holder, obj);
        let value = f(sc.value);
        if apply_write(sc, value, name.filter(|_| target.auto_update), created) {
            record_write(&mut self.out, &mut self.dirty, target.tracked, holder, obj, sc);
        }
        value
    }

    /// Every objective of `criteria` written for `holder` (`f` receives the old value, 0 if
    /// new), without listing the objectives first; `name`: the holder's display name, copied
    /// into the objectives that ask for it.
    pub fn update_criteria(&mut self, criteria: &str, holder: &str, name: Option<Nbt>, f: impl Fn(i32) -> i32) {
        for o in self.objectives.iter().filter(|o| o.criteria == criteria) {
            // By name, like a command naming the objective (a hand-edited save may repeat one).
            let target = self.score_target(&o.name);
            let (sc, created) = self.holders.open(holder, &o.name);
            let value = f(sc.value);
            let shown = if target.auto_update { name.clone() } else { None };
            if apply_write(sc, value, shown, created) {
                record_write(&mut self.out, &mut self.dirty, target.tracked, holder, &o.name, sc);
            }
        }
    }

    /// Writes a score of a holder without a display name (name only).
    pub fn set(&mut self, holder: &str, obj: &str, value: i32) {
        self.set_shown(holder, obj, value, None);
    }

    /// Writing a score: created if needed; display name of the holder copied if the objective
    /// asks for it (automatic display name update); packet only if the score is new or changed.
    pub fn set_shown(&mut self, holder: &str, obj: &str, value: i32, name: Option<Nbt>) {
        let created = self.touch(holder, obj);
        self.write(holder, obj, value, name, created);
    }

    /// Access to a score: created if needed without packet or save (a simple read by the game
    /// creates the score); `true` if it is new, and each write through this same access will send it.
    pub fn touch(&mut self, holder: &str, obj: &str) -> bool {
        self.score_mut(holder, obj).1
    }

    /// Writing through an access opened with [`Board::touch`]: `sync` (score created by this access and
    /// not yet sent) forces the packet even without change.
    pub fn write(&mut self, holder: &str, obj: &str, value: i32, name: Option<Nbt>, sync: bool) {
        let auto = self.objective(obj).is_some_and(|o| o.auto_update);
        let (sc, _) = self.score_mut(holder, obj);
        if apply_write(sc, value, name.filter(|_| auto), sync) {
            self.score_changed(holder, obj);
        }
    }

    /// Display name of a score (`players display name`): packet if new or changed.
    pub fn set_display(&mut self, holder: &str, obj: &str, name: Option<Nbt>) {
        let (sc, created) = self.score_mut(holder, obj);
        let same = match (&sc.display, &name) {
            (Some(a), Some(b)) => crate::items::nbt_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if created || !same {
            sc.display = name;
            self.score_changed(holder, obj);
        }
    }

    /// Own format of a score (`players display numberformat`): always sent back.
    pub fn set_number_format(&mut self, holder: &str, obj: &str, format: Option<Nbt>) {
        self.score_mut(holder, obj).0.format = format;
        self.score_changed(holder, obj);
    }

    pub fn set_locked(&mut self, holder: &str, obj: &str, locked: bool) {
        let (sc, created) = self.score_mut(holder, obj);
        sc.locked = locked;
        if created {
            self.score_changed(holder, obj);
        }
        self.dirty = true;
    }

    /// Reset: unknown holder = nothing;
    /// emptied holder = forgotten, packet without objective (if an objective is tracked).
    pub fn reset(&mut self, holder: &str, obj: Option<&str>) {
        match self.holders.reset(holder, obj) {
            Reset::UnknownHolder | Reset::NothingRemoved => {}
            Reset::HolderForgotten => {
                if !self.slots.is_empty() {
                    self.out.push(ScoreEvent::Reset { holder: holder.to_owned(), objective: None });
                }
                self.dirty = true;
            }
            Reset::ScoreRemoved => {
                if let Some(o) = obj.filter(|o| self.tracked(o)) {
                    self.out.push(ScoreEvent::Reset { holder: holder.to_owned(), objective: Some(o.to_owned()) });
                }
                self.dirty = true;
            }
        }
    }

    /// Rank of `holder`'s team (the first one listing it).
    fn team_rank(&self, holder: &str) -> Option<usize> {
        if self.paths.member_index {
            // No member at all (the common case of an entity removal): no UUID text to read.
            if self.members.is_empty() {
                return None;
            }
            return self.members.rank(holder).map(|r| r as usize);
        }
        self.teams.iter().position(|t| t.players.iter().any(|p| p == holder))
    }

    /// Removal of a member from their team (removal packet).
    pub fn leave_team(&mut self, holder: &str) -> bool {
        let Some(rank) = self.team_rank(holder) else { return false };
        let t = &mut self.teams[rank];
        t.players.retain(|p| p != holder);
        let name = t.name.clone();
        if self.paths.member_index {
            self.members.remove(holder, &self.teams);
        } else {
            self.touch_teams();
        }
        self.out.push(ScoreEvent::Team { name, mode: 4, params: None, members: vec![holder.to_owned()] });
        self.dirty = true;
        true
    }

    /// Joining a team: first leaves their own, even if it is
    /// this one (the game then counts a success and sends removal then addition).
    pub fn join_team(&mut self, holder: &str, team: &str) -> bool {
        self.leave_team(holder);
        let Some(rank) = self.teams.iter().position(|t| t.name == team) else { return false };
        self.teams[rank].players.push(holder.to_owned());
        if self.paths.member_index {
            self.members.add(holder, rank as u32);
        } else {
            self.touch_teams();
        }
        self.out.push(ScoreEvent::Team {
            name: team.to_owned(),
            mode: 3,
            params: None,
            members: vec![holder.to_owned()],
        });
        self.dirty = true;
        true
    }

    /// Does it hold anything that an entity removal should erase (test without allocation)?
    pub fn has_entries(&self) -> bool {
        !self.holders.is_empty() || self.teams.iter().any(|t| !t.players.is_empty())
    }

    /// Entity (non-player) permanently removed from the world: scores erased,
    /// team left, packets broadcast.
    pub fn entity_removed(&mut self, holder: &str) {
        self.reset(holder, None);
        self.leave_team(holder);
    }

    /// [`Board::entity_removed`] for the entity of `uuid` (named by its canonical text). Called for
    /// every entity leaving the world once a single score or member exists: with
    /// `RUSTCRAFT_OPT_SCOREUUIDSET`, an entity that has neither costs a set lookup, without
    /// formatting its UUID.
    pub fn remove_entity(&mut self, uuid: u128) {
        if !self.has_entries() || (self.paths.uuid_set && !self.may_name_entity(uuid)) {
            return;
        }
        self.entity_removed(&crate::nbtio::uuid_text(uuid));
    }

    /// Could the canonical text of `uuid` name a holder or a member? Exact for the holders; for
    /// the members, exact with the member index, otherwise "yes" as soon as a team has members.
    fn may_name_entity(&self, uuid: u128) -> bool {
        if self.holders.names_uuid(uuid) != Some(false) {
            return true;
        }
        if self.paths.member_index {
            self.members.rank_by_uuid(uuid).is_some()
        } else {
            self.teams.iter().any(|t| !t.players.is_empty())
        }
    }

    /// Objectives of a criterion, in creation order.
    pub fn objectives_of(&self, criteria: &str) -> Vec<String> {
        self.objectives.iter().filter(|o| o.criteria == criteria).map(|o| o.name.clone()).collect()
    }

    /// Number of slots displaying this objective.
    fn slot_count(&self, obj: &str) -> usize {
        self.slots.values().filter(|o| *o == obj).count()
    }

    /// Display of a slot on the server side: the old objective still
    /// displayed elsewhere receives the new display, otherwise it is no longer tracked (removal on the
    /// client side); the new one, if it was not tracked, is created with its slots and scores.
    pub fn set_display_slot(&mut self, slot: usize, obj: Option<&str>) {
        let new_tracked = obj.is_some_and(|o| self.tracked(o));
        let old = match obj {
            Some(o) => self.slots.insert(slot, o.to_owned()),
            None => self.slots.remove(&slot),
        };
        if let Some(old) = old.filter(|x| Some(x.as_str()) != obj) {
            if self.slot_count(&old) > 0 {
                self.out.push(ScoreEvent::Display { slot: slot as i32, objective: obj.unwrap_or("").to_owned() });
            } else {
                self.out.push(ScoreEvent::Objective { name: old, mode: 1, display: None, hearts: false, format: None });
            }
        }
        if let Some(o) = obj {
            if new_tracked {
                self.out.push(ScoreEvent::Display { slot: slot as i32, objective: o.to_owned() });
            } else if let Some(x) = self.objective(o) {
                let p = self.start_tracking_packets(x);
                self.out.extend(p);
            }
        }
        self.dirty = true;
    }

    /// Tracked holders, in the game's listing order (streaming traversal); those whose scores have all
    /// disappeared with their objective stay tracked (until they are reset or on reload).
    pub fn holders(&self) -> Vec<String> {
        self.holders.streamed().map(str::to_owned).collect()
    }

    /// Objectives in the game's listing order.
    pub fn objectives_listed(&self) -> Vec<&Objective> {
        self.obj_order.iter().filter_map(|n| self.objective(n)).collect()
    }

    /// Teams in the game's listing order.
    pub fn teams_listed(&self) -> Vec<&Team> {
        self.team_order.iter().filter_map(|n| self.team(n)).collect()
    }

    pub fn add_objective(&mut self, o: Objective) {
        self.obj_order.insert(&o.name);
        self.objectives.push(o);
    }

    /// Removal of an objective: its slots emptied one by one in the order of the
    /// slots (display packets then end of tracking), then its scores.
    pub fn remove_objective(&mut self, name: &str) {
        self.obj_order.remove(name);
        let slots: Vec<usize> = self.slots.iter().filter(|(_, o)| *o == name).map(|(i, _)| *i).collect();
        for i in slots {
            self.set_display_slot(i, None);
        }
        self.objectives.retain(|x| x.name != name);
        self.holders.remove_objective(name);
        self.dirty = true;
    }

    /// A team added (it may come with members: a save, a test).
    pub fn add_team(&mut self, t: Team) {
        self.team_order.insert(&t.name);
        if self.paths.member_index {
            let rank = self.teams.len() as u32;
            for p in &t.players {
                self.members.add(p, rank);
            }
        }
        self.teams.push(t);
        self.touch_teams();
    }

    pub fn remove_team(&mut self, name: &str) {
        self.team_order.remove(name);
        self.teams.retain(|x| x.name != name);
        // The teams after it moved down one rank.
        self.index_members();
        self.touch_teams();
    }

    /// Member index rebuilt from the teams (`RUSTCRAFT_OPT_TEAMMEMBERINDEX`).
    fn index_members(&mut self) {
        if self.paths.member_index {
            self.members = TeamMembers::of(&self.teams);
        }
    }

    /// Teams modified outside the methods above (a team's options): push filter state
    /// recomputed, collision rule index of the former way rebuilt at the next use.
    pub fn touch_teams(&mut self) {
        self.collision.take();
        self.collision_rules_active = self.teams.iter().any(|t| t.collision != "always");
    }

    /// Push filter: some team has a collision rule other than "always" (otherwise nothing is
    /// filtered and nothing needs to be looked up).
    pub fn collision_rules_active(&self) -> bool {
        if self.paths.member_index { self.collision_rules_active } else { self.collision_index().active }
    }

    /// Team rank and collision rule code of the member named by `uuid`'s canonical text (an
    /// entity); `None` without team, or while no team has a rule (filter inactive).
    pub fn collision_rule_by_uuid(&self, uuid: u128) -> Option<(u32, u8)> {
        if !self.paths.member_index {
            return self.collision_index().by_uuid.get(&uuid).copied();
        }
        let rank = self.members.rank_by_uuid(uuid).filter(|_| self.collision_rules_active)?;
        Some((rank, collision_code(&self.teams[rank as usize].collision)))
    }

    /// [`Board::collision_rule_by_uuid`] for a member whose name is not a canonical UUID text (a
    /// player); `None` for a name that is one.
    pub fn collision_rule_by_name(&self, name: &str) -> Option<(u32, u8)> {
        if !self.paths.member_index {
            return self.collision_index().by_name.get(name).copied();
        }
        let rank = self.members.rank_by_plain_name(name).filter(|_| self.collision_rules_active)?;
        Some((rank, collision_code(&self.teams[rank as usize].collision)))
    }

    /// Collision rule index of the former way, built at the first use after a team change. A
    /// member registered in two teams (save) keeps the first, like `team_of`.
    fn collision_index(&self) -> &CollisionIndex {
        self.collision.get_or_init(|| {
            let mut ix =
                CollisionIndex { active: self.teams.iter().any(|t| t.collision != "always"), ..Default::default() };
            if !ix.active {
                return ix;
            }
            for (i, t) in self.teams.iter().enumerate() {
                let v = (i as u32, collision_code(&t.collision));
                for p in &t.players {
                    match crate::commands::snbt::parse_uuid(p).filter(|u| crate::nbtio::uuid_text(*u) == *p) {
                        Some(u) => {
                            ix.by_uuid.entry(u).or_insert(v);
                        }
                        None => {
                            ix.by_name.entry(p.clone()).or_insert(v);
                        }
                    }
                }
            }
            ix
        })
    }

    pub fn team(&self, name: &str) -> Option<&Team> {
        self.teams.iter().find(|t| t.name == name)
    }

    /// Team of `holder` (the first one listing it).
    pub fn team_of(&self, holder: &str) -> Option<&Team> {
        self.team_rank(holder).map(|r| &self.teams[r])
    }

    /// [`Board::team_of`] the entity of `uuid` (named by its canonical text): with the member
    /// index, its text is never formatted.
    pub fn team_of_uuid(&self, uuid: u128) -> Option<&Team> {
        if !self.paths.member_index {
            return self.team_of(&crate::nbtio::uuid_text(uuid));
        }
        self.members.rank_by_uuid(uuid).map(|r| &self.teams[r as usize])
    }

    /// Is the entity of `uuid` on the team named `team`? (See [`Board::team_of_uuid`].)
    pub fn uuid_in_team(&self, uuid: u128, team: &str) -> bool {
        self.team_of_uuid(uuid).is_some_and(|t| t.name == team)
    }

    /// Name decorated by the team: prefix, color, suffix.
    pub fn decorate(&self, holder: &str, name: Component) -> Component {
        let Some(t) = self.team_of(holder) else { return name };
        let mut c = Component::text("");
        if let Some(p) = rc_text::from_nbt(&t.prefix) {
            c.extra.push(p);
        }
        c.extra.push(name);
        if let Some(sf) = rc_text::from_nbt(&t.suffix) {
            c.extra.push(sf);
        }
        if let Some(col) = &t.color {
            c.style.color = rc_text::parse_color(col);
        }
        c
    }

    pub fn team_params(t: &Team) -> TeamParams {
        let vis = |v: &str| match v {
            "never" => 1,
            "hideForOtherTeams" => 2,
            "hideForOwnTeam" => 3,
            _ => 0,
        };
        TeamParams {
            display: Some(t.display.clone()),
            prefix: Some(t.prefix.clone()),
            suffix: Some(t.suffix.clone()),
            nametag: vis(&t.nametag),
            collision: i32::from(collision_code(&t.collision)),
            color: t.color.as_ref().and_then(|c| COLORS.iter().position(|x| x == c)).map(|i| i as i32),
            options: i8::from(t.friendly_fire) | if t.see_invisibles { 2 } else { 0 },
        }
    }

    /// Entering-the-game packets: teams, displayed objectives and their scores.
    pub fn join_packets(&self) -> Vec<ScoreEvent> {
        let mut out = Vec::new();
        for t in self.teams_listed() {
            out.push(ScoreEvent::Team {
                name: t.name.clone(),
                mode: 0,
                params: Some(Board::team_params(t)),
                members: t.players.clone(),
            });
        }
        let mut sent = std::collections::BTreeSet::new();
        for o in self.slots.values() {
            if let Some(obj) = self.objective(o)
                && sent.insert(o.as_str())
            {
                out.extend(self.start_tracking_packets(obj));
            }
        }
        out
    }

    /// Start of tracking of an objective: creation, slots that display it, then its scores.
    pub fn start_tracking_packets(&self, o: &Objective) -> Vec<ScoreEvent> {
        let mut out = vec![ScoreEvent::Objective {
            name: o.name.clone(),
            mode: 0,
            display: Some(o.display.clone()),
            hearts: o.render == "hearts",
            format: format_net(&o.format),
        }];
        for (slot, x) in &self.slots {
            if *x == o.name {
                out.push(ScoreEvent::Display { slot: *slot as i32, objective: o.name.clone() });
            }
        }
        for (h, m) in self.holders.listed_scores() {
            if let Some(sc) = m.get(&o.name) {
                out.push(score_packet(h, &o.name, sc));
            }
        }
        out
    }

    /// The holders' table, listing order and UUID set, and the member index, agree with each
    /// other and with the teams.
    #[cfg(test)]
    pub fn check_consistent(&self) {
        self.holders.check_consistent();
        if self.paths.member_index {
            self.members.check_consistent(&self.teams);
        } else {
            assert!(self.members.is_empty(), "member index kept on the former path");
        }
        assert_eq!(self.collision_rules_active, self.teams.iter().any(|t| t.collision != "always"));
    }
}

#[cfg(test)]
mod tests {
    use super::{Board, Objective, Team};
    use rc_proto::nbt::Nbt;
    use rc_proto_v777::scores::{NumberFormat, ScoreEvent};

    fn obj(b: &mut Board, name: &str) {
        b.add_objective(Objective {
            name: name.into(),
            criteria: "dummy".into(),
            display: Nbt::String(name.into()),
            render: "integer".into(),
            auto_update: false,
            format: None,
        });
    }

    fn team(b: &mut Board, name: &str) {
        b.add_team(Team {
            name: name.into(),
            display: Nbt::String(name.into()),
            color: None,
            friendly_fire: true,
            see_invisibles: true,
            prefix: Nbt::String(String::new()),
            suffix: Nbt::String(String::new()),
            nametag: "always".into(),
            death: "always".into(),
            collision: "always".into(),
            players: Vec::new(),
        });
    }

    /// Readable summary of pending packets (empties the queue).
    fn drain(b: &mut Board) -> Vec<String> {
        std::mem::take(&mut b.out)
            .into_iter()
            .map(|e| match e {
                ScoreEvent::Objective { name, mode, .. } => format!("obj {name} {mode}"),
                ScoreEvent::Display { slot, objective } => format!("disp {slot} {objective}"),
                ScoreEvent::Score { holder, objective, value, .. } => format!("score {holder} {objective} {value}"),
                ScoreEvent::Reset { holder, objective } => format!("reset {holder} {objective:?}"),
                ScoreEvent::Team { name, mode, members, .. } => format!("team {name} {mode} {members:?}"),
            })
            .collect()
    }

    #[test]
    fn display_slots_track_like_the_server() {
        let mut b = Board::default();
        obj(&mut b, "a");
        obj(&mut b, "b");
        b.set("p", "a", 3);
        assert!(drain(&mut b).is_empty(), "untracked objective: no packet");
        // Start of tracking: creation, slots, then scores.
        b.set_display_slot(1, Some("a"));
        assert_eq!(drain(&mut b), ["obj a 0", "disp 1 a", "score p a 3"]);
        // Second slot of a tracked objective: display only.
        b.set_display_slot(0, Some("a"));
        assert_eq!(drain(&mut b), ["disp 0 a"]);
        // The old one stays displayed elsewhere: the slot receives the new one, then the new one is tracked.
        b.set_display_slot(1, Some("b"));
        assert_eq!(drain(&mut b), ["disp 1 b", "obj b 0", "disp 1 b"]);
        // The old one is no longer displayed anywhere: end of tracking (removal on the client side).
        b.set_display_slot(0, None);
        assert_eq!(drain(&mut b), ["obj a 1"]);
        b.set("p", "a", 4);
        assert!(drain(&mut b).is_empty(), "no longer tracked: no more packets");
        // Removal of an objective displayed twice: display emptied, then end of tracking.
        b.set_display_slot(2, Some("b"));
        drain(&mut b);
        b.remove_objective("b");
        assert_eq!(drain(&mut b), ["disp 1 ", "obj b 1"]);
        assert!(b.slots.is_empty());
    }

    /// Access without writing (reading of an operation): score created, no packet or save; the
    /// first write through this access sends even if unchanged, the following only on change.
    #[test]
    fn touched_score_is_silent_until_written() {
        let mut b = Board::default();
        obj(&mut b, "o");
        b.set_display_slot(1, Some("o"));
        drain(&mut b);
        b.dirty = false;
        assert!(b.touch("A", "o"));
        assert!(!b.touch("A", "o"), "already created");
        assert_eq!(b.get("A", "o"), Some(0));
        assert!(drain(&mut b).is_empty() && !b.dirty);
        // Later access (no longer new): writing 0 sends nothing.
        b.set("A", "o", 0);
        assert!(drain(&mut b).is_empty());
        let created = b.touch("B", "o");
        b.write("B", "o", 0, None, created);
        b.write("B", "o", 0, None, false);
        assert_eq!(drain(&mut b), ["score B o 0"]);
    }

    #[test]
    fn scores_written_like_score_access() {
        let mut b = Board::default();
        obj(&mut b, "a");
        b.set_display_slot(1, Some("a"));
        drain(&mut b);
        // New score: locked (a /trigger requires enable).
        b.set("p", "a", 0);
        assert!(b.score("p", "a").unwrap().locked);
        assert_eq!(drain(&mut b), ["score p a 0"], "new score sent even at 0");
        b.set("p", "a", 0);
        assert!(drain(&mut b).is_empty(), "unchanged value: no packet");
        // Automatic display name update: the holder's display name is copied.
        b.objectives[0].auto_update = true;
        b.set_shown("p", "a", 0, Some(Nbt::String("P".into())));
        assert_eq!(b.score("p", "a").unwrap().display, Some(Nbt::String("P".into())));
        assert_eq!(drain(&mut b), ["score p a 0"]);
        // Reset: the last score of a holder forgets it (packet without objective).
        b.set("q", "a", 1);
        b.reset("q", Some("a"));
        assert_eq!(drain(&mut b), ["score q a 1", "reset q None"]);
        b.reset("unknown", None);
        assert!(drain(&mut b).is_empty(), "unknown holder: nothing");
    }

    #[test]
    fn entity_removed_clears_scores_and_team() {
        let mut b = Board::default();
        obj(&mut b, "a");
        team(&mut b, "t");
        b.set_display_slot(1, Some("a"));
        b.set("u", "a", 2);
        assert!(b.join_team("u", "t"));
        drain(&mut b);
        // Joining one's own team: success, removal then addition.
        assert!(b.join_team("u", "t"));
        assert_eq!(drain(&mut b), ["team t 4 [\"u\"]", "team t 3 [\"u\"]"]);
        b.entity_removed("u");
        assert_eq!(drain(&mut b), ["reset u None", "team t 4 [\"u\"]"]);
        assert_eq!(b.get("u", "a"), None);
        assert!(b.team_of("u").is_none());
        assert!(!b.has_entries());
    }

    #[test]
    fn styled_number_format_reads_flat_style() {
        // Style fields flat next to `type` (file and command).
        let f = Some(Nbt::Compound(vec![
            ("type".into(), Nbt::String("styled".into())),
            ("color".into(), Nbt::String("red".into())),
        ]));
        assert_eq!(
            super::format_net(&f),
            Some(NumberFormat::Styled(Some(Nbt::Compound(vec![("color".into(), Nbt::String("red".into()))]))))
        );
    }

    #[test]
    fn save_omits_default_fields() {
        // Empty board: `data: {}` (taken from a save of the official game).
        assert_eq!(Board::default().to_nbt(), Nbt::Compound(Vec::new()));
        let mut b = Board::default();
        obj(&mut b, "a");
        team(&mut b, "t");
        b.set("p", "a", 0);
        b.set_locked("p", "a", false);
        let n = b.to_nbt();
        let first = |k: &str| match n.get(k) {
            Some(Nbt::List(l)) => l[0].clone(),
            x => panic!("{k}: {x:?}"),
        };
        let keys = |v: Nbt| match v {
            Nbt::Compound(f) => f.into_iter().map(|(k, _)| k).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        assert_eq!(keys(first("Objectives")), ["Name", "DisplayName"]);
        assert_eq!(keys(first("PlayerScores")), ["Name", "Objective"]);
        assert_eq!(keys(first("Teams")), ["Name", "DisplayName"]);
        assert!(n.get("DisplaySlots").is_none());
    }

    #[test]
    fn holders_listed_in_game_order_and_kept_when_their_objective_goes() {
        // Lists taken from the official server (`scoreboard players list`, same sequence: 19
        // holders, then three reset).
        let names = "Steve,Alex,#fake,Notch,jeb_,Dinnerbone,Grumm,a,b,c,d,e,f,g,h,i,j,k,l";
        let mut b = Board::default();
        for n in names.split(',') {
            b.set(n, "o", 1);
        }
        assert_eq!(b.holders().join(","), "Notch,Dinnerbone,Steve,#fake,a,e,i,j,Grumm,h,jeb_,l,f,b,d,Alex,k,g,c");
        for n in ["Alex", "c", "g"] {
            b.reset(n, None);
        }
        assert_eq!(b.holders().join(","), "Notch,Dinnerbone,Steve,#fake,a,e,i,j,Grumm,h,jeb_,l,f,b,d,k");
        // Objective removed: holders without a score stay tracked.
        b.remove_objective("o");
        assert_eq!(b.holders().len(), 16);
        b.check_consistent();
    }
}
