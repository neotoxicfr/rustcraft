//! World scoreboard (`data/minecraft/scoreboard.dat`): objectives, scores,
//! display slots, teams; reading, writing and packets to the client.

use std::collections::BTreeMap;
use std::path::Path;

use rc_proto::nbt::Nbt;
use rc_proto_v777::scores::{NumberFormat, ScoreEvent, TeamParams};
use rc_text::Component;

use crate::commands::hash_order::HashOrder;
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

#[derive(Debug, Clone)]
pub struct Board {
    pub objectives: Vec<Objective>,
    /// Holder → objective → score (insertion order of the holders kept).
    pub scores: Vec<(String, BTreeMap<String, Score>)>,
    pub slots: BTreeMap<usize, String>,
    pub teams: Vec<Team>,
    pub dirty: bool,
    /// Packets to send to the client (flushed by the server).
    pub out: Vec<ScoreEvent>,
    /// Listing orders of the game (hash tables replayed): objectives, holders, teams.
    obj_order: HashOrder,
    holder_order: HashOrder,
    team_order: HashOrder,
}

impl Default for Board {
    fn default() -> Board {
        Board {
            objectives: Vec::new(),
            scores: Vec::new(),
            slots: BTreeMap::new(),
            teams: Vec::new(),
            dirty: false,
            out: Vec::new(),
            obj_order: HashOrder::new(16, 0.5),
            holder_order: HashOrder::new(16, 0.5),
            team_order: HashOrder::new(16, 0.75),
        }
    }
}

fn s(v: &Nbt, k: &str) -> Option<String> {
    v.get(k).and_then(Nbt::as_str).map(str::to_owned)
}

fn flag(v: &Nbt, k: &str, d: bool) -> bool {
    int(v, k).map_or(d, |x| x != 0)
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

impl Board {
    pub fn read(world: &Path) -> Board {
        let d = crate::nbtio::read_dat(&world.join("data").join("minecraft").join("scoreboard.dat"))
            .ok()
            .and_then(|r| r.get("data").cloned());
        let mut b = Board::default();
        let Some(d) = d else { return b };
        for o in list(d.get("Objectives")) {
            let Some(name) = s(o, "Name") else { continue };
            b.obj_order.insert(&name);
            b.objectives.push(Objective {
                name,
                criteria: s(o, "CriteriaName").unwrap_or_else(|| "dummy".into()),
                display: o.get("DisplayName").cloned().unwrap_or_else(empty_text),
                render: s(o, "RenderType").unwrap_or_else(|| "integer".into()),
                auto_update: flag(o, "display_auto_update", false),
                format: o.get("format").cloned(),
            });
        }
        for e in list(d.get("PlayerScores")) {
            let (Some(name), Some(obj)) = (s(e, "Name"), s(e, "Objective")) else { continue };
            let sc = Score {
                value: int(e, "Score").unwrap_or(0) as i32,
                locked: flag(e, "Locked", false),
                display: e.get("display").cloned(),
                format: e.get("format").cloned(),
            };
            b.entry(&name).insert(obj, sc);
        }
        if let Some(Nbt::Compound(f)) = d.get("DisplaySlots") {
            for (k, v) in f {
                if let (Some(i), Some(o)) = (SLOTS.iter().position(|x| x == k), v.as_str()) {
                    b.slots.insert(i, o.to_owned());
                }
            }
        }
        for t in list(d.get("Teams")) {
            let Some(name) = s(t, "Name") else { continue };
            b.team_order.insert(&name);
            b.teams.push(Team {
                display: t.get("DisplayName").cloned().unwrap_or_else(|| Nbt::String(name.clone())),
                name,
                color: s(t, "TeamColor").filter(|c| c != "reset"),
                friendly_fire: flag(t, "AllowFriendlyFire", true),
                see_invisibles: flag(t, "SeeFriendlyInvisibles", true),
                prefix: t.get("MemberNamePrefix").cloned().unwrap_or_else(empty_text),
                suffix: t.get("MemberNameSuffix").cloned().unwrap_or_else(empty_text),
                nametag: s(t, "NameTagVisibility").unwrap_or_else(|| "always".into()),
                death: s(t, "DeathMessageVisibility").unwrap_or_else(|| "always".into()),
                collision: s(t, "CollisionRule").unwrap_or_else(|| "always".into()),
                players: list(t.get("Players")).iter().filter_map(|p| p.as_str().map(str::to_owned)).collect(),
            });
        }
        b
    }

    /// Writing: like the game's codecs, a field equal to its default value is omitted (an
    /// empty board is written `data: {}`, taken from a save of the official game).
    pub fn to_nbt(&self) -> Nbt {
        let objs: Vec<Nbt> = self
            .obj_order
            .iter_stream()
            .filter_map(|n| self.objective(n))
            .map(|o| {
                let mut f = vec![("Name".into(), Nbt::String(o.name.clone()))];
                if o.criteria != "dummy" {
                    f.push(("CriteriaName".into(), Nbt::String(o.criteria.clone())));
                }
                f.push(("DisplayName".into(), o.display.clone()));
                if o.render != "integer" {
                    f.push(("RenderType".into(), Nbt::String(o.render.clone())));
                }
                if o.auto_update {
                    f.push(("display_auto_update".into(), Nbt::Byte(1)));
                }
                if let Some(fm) = &o.format {
                    f.push(("format".into(), fm.clone()));
                }
                Nbt::Compound(f)
            })
            .collect();
        let mut scores = Vec::new();
        for (h, m) in self.holder_order.iter_stream().filter_map(|h| self.scores.iter().find(|(x, _)| x == h)) {
            for (o, sc) in m {
                let mut f = vec![("Name".into(), Nbt::String(h.clone())), ("Objective".into(), Nbt::String(o.clone()))];
                if sc.value != 0 {
                    f.push(("Score".into(), Nbt::Int(sc.value)));
                }
                if sc.locked {
                    f.push(("Locked".into(), Nbt::Byte(1)));
                }
                if let Some(d) = &sc.display {
                    f.push(("display".into(), d.clone()));
                }
                if let Some(fm) = &sc.format {
                    f.push(("format".into(), fm.clone()));
                }
                scores.push(Nbt::Compound(f));
            }
        }
        let slots: Vec<(String, Nbt)> =
            self.slots.iter().map(|(i, o)| (SLOTS[*i].to_owned(), Nbt::String(o.clone()))).collect();
        let teams: Vec<Nbt> = self
            .team_order
            .iter_stream()
            .filter_map(|n| self.team(n))
            .map(|t| {
                let mut f =
                    vec![("Name".into(), Nbt::String(t.name.clone())), ("DisplayName".into(), t.display.clone())];
                if let Some(c) = &t.color {
                    f.push(("TeamColor".into(), Nbt::String(c.clone())));
                }
                let mut opt = |k: &str, v: Nbt, default: bool| {
                    if !default {
                        f.push((k.into(), v));
                    }
                };
                opt("AllowFriendlyFire", Nbt::Byte(0), t.friendly_fire);
                opt("SeeFriendlyInvisibles", Nbt::Byte(0), t.see_invisibles);
                opt("MemberNamePrefix", t.prefix.clone(), is_empty_text(&t.prefix));
                opt("MemberNameSuffix", t.suffix.clone(), is_empty_text(&t.suffix));
                opt("NameTagVisibility", Nbt::String(t.nametag.clone()), t.nametag == "always");
                opt("DeathMessageVisibility", Nbt::String(t.death.clone()), t.death == "always");
                opt("CollisionRule", Nbt::String(t.collision.clone()), t.collision == "always");
                opt(
                    "Players",
                    Nbt::List(t.players.iter().map(|p| Nbt::String(p.clone())).collect()),
                    t.players.is_empty(),
                );
                Nbt::Compound(f)
            })
            .collect();
        let mut root = Vec::new();
        if !objs.is_empty() {
            root.push(("Objectives".into(), Nbt::List(objs)));
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

    fn entry(&mut self, holder: &str) -> &mut BTreeMap<String, Score> {
        if let Some(i) = self.scores.iter().position(|(h, _)| h == holder) {
            return &mut self.scores[i].1;
        }
        self.scores.push((holder.to_owned(), BTreeMap::new()));
        self.holder_order.insert(holder);
        let n = self.scores.len() - 1;
        &mut self.scores[n].1
    }

    pub fn objective(&self, name: &str) -> Option<&Objective> {
        self.objectives.iter().find(|o| o.name == name)
    }

    pub fn get(&self, holder: &str, obj: &str) -> Option<i32> {
        self.scores.iter().find(|(h, _)| h == holder)?.1.get(obj).map(|s| s.value)
    }

    pub fn score(&self, holder: &str, obj: &str) -> Option<&Score> {
        self.scores.iter().find(|(h, _)| h == holder)?.1.get(obj)
    }

    /// Objective tracked by clients: displayed in at least one slot.
    pub fn tracked(&self, obj: &str) -> bool {
        self.slots.values().any(|o| o == obj)
    }

    /// Score created if needed; `true` if it was just created (it will be sent at the first write).
    fn score_mut(&mut self, holder: &str, obj: &str) -> (&mut Score, bool) {
        let m = self.entry(holder);
        let created = !m.contains_key(obj);
        (m.entry(obj.to_owned()).or_default(), created)
    }

    /// Packet of a score (tracked objective only) and save to redo.
    fn score_changed(&mut self, holder: &str, obj: &str) {
        if self.tracked(obj)
            && let Some(sc) = self.score(holder, obj)
        {
            let ev = ScoreEvent::Score {
                holder: holder.to_owned(),
                objective: obj.to_owned(),
                value: sc.value,
                display: sc.display.clone(),
                format: format_net(&sc.format),
            };
            self.out.push(ev);
        }
        self.dirty = true;
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
        let mut changed = sync;
        if auto
            && let Some(n) = name
            && sc.display.as_ref().is_none_or(|d| !crate::items::nbt_eq(d, &n))
        {
            sc.display = Some(n);
            changed = true;
        }
        if sc.value != value {
            sc.value = value;
            changed = true;
        }
        if changed {
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
        let Some(i) = self.scores.iter().position(|(h, _)| h == holder) else { return };
        let removed = match obj {
            Some(o) => self.scores[i].1.remove(o).is_some(),
            None => {
                self.scores[i].1.clear();
                false
            }
        };
        if self.scores[i].1.is_empty() {
            self.scores.remove(i);
            self.holder_order.remove(holder);
            if !self.slots.is_empty() {
                self.out.push(ScoreEvent::Reset { holder: holder.to_owned(), objective: None });
            }
            self.dirty = true;
        } else if let Some(o) = obj.filter(|_| removed) {
            if self.tracked(o) {
                self.out.push(ScoreEvent::Reset { holder: holder.to_owned(), objective: Some(o.to_owned()) });
            }
            self.dirty = true;
        }
    }

    /// Removal of a member from their team (removal packet).
    pub fn leave_team(&mut self, holder: &str) -> bool {
        let Some(t) = self.teams.iter_mut().find(|t| t.players.iter().any(|p| p == holder)) else { return false };
        t.players.retain(|p| p != holder);
        let name = t.name.clone();
        self.out.push(ScoreEvent::Team { name, mode: 4, params: None, members: vec![holder.to_owned()] });
        self.dirty = true;
        true
    }

    /// Joining a team: first leaves their own, even if it is
    /// this one (the game then counts a success and sends removal then addition).
    pub fn join_team(&mut self, holder: &str, team: &str) -> bool {
        self.leave_team(holder);
        let Some(t) = self.teams.iter_mut().find(|t| t.name == team) else { return false };
        t.players.push(holder.to_owned());
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
        !self.scores.is_empty() || self.teams.iter().any(|t| !t.players.is_empty())
    }

    /// Entity (non-player) permanently removed from the world: scores erased,
    /// team left, packets broadcast.
    pub fn entity_removed(&mut self, holder: &str) {
        self.reset(holder, None);
        self.leave_team(holder);
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
        self.holder_order.iter_stream().map(str::to_owned).collect()
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
        for (_, m) in &mut self.scores {
            m.remove(name);
        }
        self.dirty = true;
    }

    pub fn add_team(&mut self, t: Team) {
        self.team_order.insert(&t.name);
        self.teams.push(t);
    }

    pub fn remove_team(&mut self, name: &str) {
        self.team_order.remove(name);
        self.teams.retain(|x| x.name != name);
    }

    pub fn team(&self, name: &str) -> Option<&Team> {
        self.teams.iter().find(|t| t.name == name)
    }

    pub fn team_of(&self, holder: &str) -> Option<&Team> {
        self.teams.iter().find(|t| t.players.iter().any(|p| p == holder))
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
        let col = |v: &str| match v {
            "never" => 1,
            "pushOtherTeams" => 2,
            "pushOwnTeam" => 3,
            _ => 0,
        };
        TeamParams {
            display: Some(t.display.clone()),
            prefix: Some(t.prefix.clone()),
            suffix: Some(t.suffix.clone()),
            nametag: vis(&t.nametag),
            collision: col(&t.collision),
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
        for (h, m) in self.holder_order.iter().filter_map(|h| self.scores.iter().find(|(x, _)| x == h)) {
            if let Some(sc) = m.get(&o.name) {
                out.push(ScoreEvent::Score {
                    holder: h.clone(),
                    objective: o.name.clone(),
                    value: sc.value,
                    display: sc.display.clone(),
                    format: format_net(&sc.format),
                });
            }
        }
        out
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
        for (_, m) in &mut b.scores {
            m.remove("o");
        }
        assert_eq!(b.holders().len(), 16);
    }
}
