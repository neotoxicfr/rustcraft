//! `scoreboard`, `team`, `trigger`: world scoreboard, packets to the client.

use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::scores::ScoreEvent;
use rc_text::{Component, HoverEvent};

use super::cmds_basic::Track;
use super::scores::{Board, Objective, SLOTS, Team};
use super::src::{Fail, Src};
use super::text::{self, lit};
use super::{Parsed, R, Seg};
use crate::server::Server;
use crate::tr;

/// Formatted objective name: brackets, hover = identifier.
pub fn objective_name(o: &Objective) -> Component {
    let mut d = rc_text::from_nbt(&o.display).unwrap_or_else(|| lit(o.name.clone()));
    d.style.hover = Some(Arc::new(HoverEvent::Text(lit(o.name.clone()))));
    text::brackets(d)
}

/// Formatted team name: brackets, insertion, hover, color.
pub fn team_name(t: &Team) -> Component {
    let mut d = rc_text::from_nbt(&t.display).unwrap_or_else(|| lit(t.name.clone()));
    d.style.insertion = Some(Arc::from(t.name.as_str()));
    d.style.hover = Some(Arc::new(HoverEvent::Text(lit(t.name.clone()))));
    let mut c = text::brackets(d);
    if let Some(col) = &t.color {
        c.style.color = rc_text::parse_color(col);
    }
    c
}

impl Server {
    /// Display name of a holder: the player, or the entity loaded with that UUID;
    /// `None` for a plain name (`#fake`, absent player).
    pub fn holder_display(&self, h: &str) -> Option<Component> {
        if h == self.name {
            return Some(self.player_display_name());
        }
        let e = self.ent_by_uuid(super::snbt::parse_uuid(h)?)?;
        Some(self.ent_display(e))
    }

    /// Name of a holder in the feedback: display name whose hover
    /// shows the scoreboard name, otherwise plain text.
    pub fn holder_name(&self, h: &str) -> Component {
        match self.holder_display(h) {
            Some(mut c) => {
                c.style.hover = Some(Arc::new(HoverEvent::Text(lit(h.to_owned()))));
                c
            }
            None => lit(h.to_owned()),
        }
    }

    /// Writing a score: display name copied if the objective asks for it.
    pub(crate) fn score_set(&mut self, h: &str, obj: &str, v: i32) {
        let mut sync = self.cmd.scores.touch(h, obj);
        self.score_write(h, obj, v, &mut sync, None);
    }

    /// Writing through an open access (`sync`: score created by this access and not yet sent;
    /// reset to false by the send). `shown`: display name of the holder when it can no longer be
    /// found by its UUID (mob that left the list during its tick).
    fn score_write(&mut self, h: &str, obj: &str, v: i32, sync: &mut bool, shown: Option<&Component>) {
        let name = if self.cmd.scores.objective(obj).is_some_and(|o| o.auto_update) {
            shown.cloned().or_else(|| self.holder_display(h)).map(|c| text::to_nbt(&c))
        } else {
            None
        };
        self.cmd.scores.write(h, obj, v, name, *sync);
        *sync = false;
    }

    /// Objectives of a criterion for this holder (forced write even when
    /// read-only or locked): `f` receives the old value (0 if new).
    pub(crate) fn criteria_update(&mut self, criteria: &str, h: &str, f: impl Fn(i32) -> i32) {
        self.criteria_update_shown(criteria, h, None, f);
    }

    /// [`Server::criteria_update`] with the holder display name provided by the caller.
    pub(crate) fn criteria_update_shown(
        &mut self,
        criteria: &str,
        h: &str,
        shown: Option<&Component>,
        f: impl Fn(i32) -> i32,
    ) {
        if !self.cmd.scores.objectives.iter().any(|o| o.criteria == criteria) {
            return;
        }
        for o in self.cmd.scores.objectives_of(criteria) {
            let mut sync = self.cmd.scores.touch(h, &o);
            let cur = self.cmd.scores.get(h, &o).unwrap_or(0);
            self.score_write(h, &o, f(cur), &mut sync, shown);
        }
    }

    /// Score packets pending (tracking was decided at write time, like the server);
    /// also flushed before each command feedback (the game broadcasts during the operation).
    pub(crate) fn flush_scores(&mut self) {
        let out = std::mem::take(&mut self.cmd.scores.out);
        for ev in out {
            let _ = self.send(Event::Score(ev));
        }
    }

    /// Entering the game: teams and displayed objectives.
    pub(crate) fn send_scoreboard(&self) {
        for ev in self.cmd.scores.join_packets() {
            let _ = self.send(Event::Score(ev));
        }
        self.send_bossbars();
    }

    fn objective_or_fail(&self, name: &str) -> Result<Objective, Fail> {
        self.cmd.scores.objective(name).cloned().ok_or_else(|| Fail(tr!("arguments.objective.notFound", name)))
    }

    fn writable(&self, name: &str) -> Result<Objective, Fail> {
        let o = self.objective_or_fail(name)?;
        if is_read_only(&o.criteria) {
            return Err(Fail(tr!("arguments.objective.readonly", name)));
        }
        Ok(o)
    }

    pub(crate) fn cmd_scoreboard(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        let r = match (w.get(1).copied(), w.get(2).copied()) {
            (Some("objectives"), Some("list")) => {
                let objs: Vec<Component> =
                    self.cmd.scores.objectives_listed().into_iter().map(objective_name).collect();
                let n = objs.len() as i32;
                if n == 0 {
                    self.ok(s, tr!("commands.scoreboard.objectives.list.empty"));
                } else {
                    self.ok(s, tr!("commands.scoreboard.objectives.list.success", n, text::list(objs)));
                }
                Ok(n)
            }
            (Some("objectives"), Some("add")) => {
                let name = a("objective");
                if self.cmd.scores.objective(&name).is_some() {
                    return Err(Fail(tr!("commands.scoreboard.objectives.add.duplicate")));
                }
                let criteria = a("criteria");
                let display = match p.arg(seg, "displayName") {
                    Some(t) => text::to_nbt(&self.arg_component(s, &t, None)?),
                    None => Nbt::String(name.clone()),
                };
                let render = if criteria == "health" { "hearts" } else { "integer" };
                let o = Objective {
                    name: name.clone(),
                    criteria,
                    display,
                    render: render.into(),
                    auto_update: false,
                    format: None,
                };
                self.cmd.scores.add_objective(o.clone());
                self.cmd.scores.dirty = true;
                self.ok(s, tr!("commands.scoreboard.objectives.add.success", objective_name(&o)));
                Ok(self.cmd.scores.objectives.len() as i32)
            }
            (Some("objectives"), Some("remove")) => {
                let o = self.objective_or_fail(&a("objective"))?;
                self.cmd.scores.remove_objective(&o.name);
                self.ok(s, tr!("commands.scoreboard.objectives.remove.success", objective_name(&o)));
                Ok(self.cmd.scores.objectives.len() as i32)
            }
            (Some("objectives"), Some("setdisplay")) => {
                let slot = a("slot");
                let idx = SLOTS.iter().position(|x| *x == slot).unwrap_or(0);
                match p.arg(seg, "objective") {
                    None => {
                        if !self.cmd.scores.slots.contains_key(&idx) {
                            return Err(Fail(tr!("commands.scoreboard.objectives.display.alreadyEmpty")));
                        }
                        self.cmd.scores.set_display_slot(idx, None);
                        self.ok(s, tr!("commands.scoreboard.objectives.display.cleared", slot));
                        Ok(0)
                    }
                    Some(name) => {
                        let o = self.objective_or_fail(&name)?;
                        if self.cmd.scores.slots.get(&idx) == Some(&o.name) {
                            return Err(Fail(tr!("commands.scoreboard.objectives.display.alreadySet")));
                        }
                        self.cmd.scores.set_display_slot(idx, Some(&o.name));
                        let disp = rc_text::from_nbt(&o.display).unwrap_or_else(|| lit(o.name.clone()));
                        self.ok(s, tr!("commands.scoreboard.objectives.display.set", slot, disp));
                        Ok(0)
                    }
                }
            }
            (Some("objectives"), Some("modify")) => self.objective_modify(s, p, seg, &w),
            (Some("players"), Some(op)) => self.players_op(s, p, seg, op, &w),
            _ => Err(Fail(tr!("command.unknown.command"))),
        };
        self.flush_scores();
        r
    }

    fn update_objective(&mut self, o: &Objective) {
        if let Some(x) = self.cmd.scores.objectives.iter_mut().find(|x| x.name == o.name) {
            *x = o.clone();
        }
        self.cmd.scores.dirty = true;
        if self.cmd.scores.tracked(&o.name) {
            self.cmd.scores.out.push(ScoreEvent::Objective {
                name: o.name.clone(),
                mode: 2,
                display: Some(o.display.clone()),
                hearts: o.render == "hearts",
                format: super::scores::format_net(&o.format),
            });
        }
    }

    fn objective_modify(&mut self, s: &Src, p: &Parsed, seg: &Seg, w: &[&str]) -> R {
        let mut o = self.objective_or_fail(&p.arg(seg, "objective").unwrap_or_default())?;
        match w.get(4).copied() {
            Some("displayname") => {
                let c = self.arg_component(s, &p.arg(seg, "displayName").unwrap_or_default(), None)?;
                let n = text::to_nbt(&c);
                if !crate::items::nbt_eq(&n, &o.display) {
                    o.display = n;
                    self.update_objective(&o);
                    self.ok(
                        s,
                        tr!("commands.scoreboard.objectives.modify.displayname", o.name.clone(), objective_name(&o)),
                    );
                }
            }
            Some("rendertype") => {
                let r = w.get(5).copied().unwrap_or("integer");
                if o.render != r {
                    o.render = r.into();
                    self.update_objective(&o);
                    self.ok(s, tr!("commands.scoreboard.objectives.modify.rendertype", objective_name(&o)));
                }
            }
            Some("displayautoupdate") => {
                let v = p.arg(seg, "value").as_deref() == Some("true");
                if o.auto_update != v {
                    o.auto_update = v;
                    self.update_objective(&o);
                    let k = if v { "enable" } else { "disable" };
                    self.ok(
                        s,
                        tr!(
                            &format!("commands.scoreboard.objectives.modify.displayAutoUpdate.{k}"),
                            o.name.clone(),
                            objective_name(&o)
                        ),
                    );
                }
            }
            Some("numberformat") => {
                o.format = self.number_format(s, p, seg, w, 5)?;
                let set = o.format.is_some();
                self.update_objective(&o);
                let k = if set { "set" } else { "clear" };
                self.ok(s, tr!(&format!("commands.scoreboard.objectives.modify.objectiveFormat.{k}"), o.name.clone()));
            }
            _ => {}
        }
        Ok(0)
    }

    fn number_format(&mut self, s: &Src, p: &Parsed, seg: &Seg, w: &[&str], at: usize) -> Result<Option<Nbt>, Fail> {
        Ok(match w.get(at).copied() {
            Some("blank") => Some(Nbt::Compound(vec![("type".into(), Nbt::String("blank".into()))])),
            Some("fixed") => {
                let c = self.arg_component(s, &p.arg(seg, "contents").unwrap_or_default(), None)?;
                Some(Nbt::Compound(vec![
                    ("type".into(), Nbt::String("fixed".into())),
                    ("value".into(), text::to_nbt(&c)),
                ]))
            }
            Some("styled") => {
                let st =
                    super::snbt::parse(&p.arg(seg, "style").unwrap_or_default()).unwrap_or(Nbt::Compound(Vec::new()));
                let mut f = vec![("type".into(), Nbt::String("styled".into()))];
                if let Nbt::Compound(x) = st {
                    f.extend(x);
                }
                Some(Nbt::Compound(f))
            }
            _ => None,
        })
    }

    fn players_op(&mut self, s: &Src, p: &Parsed, seg: &Seg, op: &str, w: &[&str]) -> R {
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        match op {
            "list" => match p.arg(seg, "target") {
                None => {
                    let hs = self.cmd.scores.holders();
                    if hs.is_empty() {
                        self.ok(s, tr!("commands.scoreboard.players.list.empty"));
                    } else {
                        // Tracked holders = plain names for the game (plain text, even a player).
                        let names = hs.iter().map(|h| lit(h.clone())).collect();
                        self.ok(s, tr!("commands.scoreboard.players.list.success", hs.len(), text::list(names)));
                    }
                    Ok(hs.len() as i32)
                }
                Some(t) => {
                    let h = self.arg_score_holders(s, &t)?.remove(0);
                    let entries: Vec<(String, i32)> = self
                        .cmd
                        .scores
                        .scores
                        .iter()
                        .find(|(x, _)| *x == h)
                        .map(|(_, m)| m.iter().map(|(o, sc)| (o.clone(), sc.value)).collect())
                        .unwrap_or_default();
                    let hn = self.holder_name(&h);
                    if entries.is_empty() {
                        self.ok(s, tr!("commands.scoreboard.players.list.entity.empty", hn));
                    } else {
                        self.ok(s, tr!("commands.scoreboard.players.list.entity.success", hn, entries.len()));
                        for (o, v) in &entries {
                            if let Some(obj) = self.cmd.scores.objective(o).cloned() {
                                self.ok(
                                    s,
                                    tr!("commands.scoreboard.players.list.entity.entry", objective_name(&obj), *v),
                                );
                            }
                        }
                    }
                    Ok(entries.len() as i32)
                }
            },
            "get" => {
                let h = self.arg_score_holders(s, &a("target"))?.remove(0);
                let o = self.objective_or_fail(&a("objective"))?;
                let Some(v) = self.cmd.scores.get(&h, &o.name) else {
                    return Err(Fail(tr!(
                        "commands.scoreboard.players.get.null",
                        o.name.clone(),
                        self.holder_name(&h)
                    )));
                };
                self.ok(s, tr!("commands.scoreboard.players.get.success", self.holder_name(&h), v, objective_name(&o)));
                Ok(v)
            }
            "set" | "add" | "remove" => {
                let hs = self.arg_score_holders(s, &a("targets"))?;
                let o = self.writable(&a("objective"))?;
                let v: i32 = a("score").parse().unwrap_or(0);
                let mut t: Track<String> = Track::default();
                for h in &hs {
                    let cur = self.cmd.scores.get(h, &o.name).unwrap_or(0);
                    let nv = match op {
                        "set" => v,
                        "add" => cur.wrapping_add(v),
                        _ => cur.wrapping_sub(v),
                    };
                    self.score_set(h, &o.name, nv);
                    t.track(h.clone(), nv);
                }
                let on = objective_name(&o);
                let any = op != "set";
                let total = t.total;
                let r = self.track_feedback(
                    s,
                    &t,
                    any,
                    None,
                    |me, h| match op {
                        "set" => {
                            tr!("commands.scoreboard.players.set.success.single", on.clone(), me.holder_name(h), v)
                        }
                        _ => tr!(
                            &format!("commands.scoreboard.players.{op}.success.single"),
                            v,
                            on.clone(),
                            me.holder_name(h),
                            total
                        ),
                    },
                    |c| match op {
                        "set" => tr!("commands.scoreboard.players.set.success.multiple", on.clone(), c, v),
                        _ => tr!(&format!("commands.scoreboard.players.{op}.success.multiple"), v, on.clone(), c),
                    },
                );
                self.flush_scores();
                r
            }
            "reset" => {
                let hs = self.arg_score_holders(s, &a("targets"))?;
                let mut t: Track<String> = Track::default();
                match p.arg(seg, "objective") {
                    None => {
                        for h in &hs {
                            self.cmd.scores.reset(h, None);
                            t.track(h.clone(), 1);
                        }
                        self.track_feedback(
                            s,
                            &t,
                            false,
                            None,
                            |me, h| tr!("commands.scoreboard.players.reset.all.single", me.holder_name(h)),
                            |c| tr!("commands.scoreboard.players.reset.all.multiple", c),
                        )
                    }
                    Some(on) => {
                        let o = self.objective_or_fail(&on)?;
                        for h in &hs {
                            self.cmd.scores.reset(h, Some(&o.name));
                            t.track(h.clone(), 1);
                        }
                        let n = objective_name(&o);
                        self.track_feedback(
                            s,
                            &t,
                            false,
                            None,
                            |me, h| {
                                tr!("commands.scoreboard.players.reset.specific.single", n.clone(), me.holder_name(h))
                            },
                            |c| tr!("commands.scoreboard.players.reset.specific.multiple", n.clone(), c),
                        )
                    }
                }
            }
            "enable" => {
                let hs = self.arg_score_holders(s, &a("targets"))?;
                let o = self.objective_or_fail(&a("objective"))?;
                if o.criteria != "trigger" {
                    return Err(Fail(tr!("commands.scoreboard.players.enable.invalid")));
                }
                let mut t: Track<String> = Track::default();
                for h in &hs {
                    let locked = self.cmd.scores.score(h, &o.name).is_none_or(|x| x.locked);
                    if locked {
                        self.cmd.scores.set_locked(h, &o.name, false);
                        t.track(h.clone(), 1);
                    } else {
                        t.track(h.clone(), 0);
                    }
                }
                let n = objective_name(&o);
                self.track_feedback(
                    s,
                    &t,
                    false,
                    Some(Fail(tr!("commands.scoreboard.players.enable.failed"))),
                    |me, h| tr!("commands.scoreboard.players.enable.success.single", n.clone(), me.holder_name(h)),
                    |c| tr!("commands.scoreboard.players.enable.success.multiple", n.clone(), c),
                )
            }
            "operation" => {
                let ts = self.arg_score_holders(s, &a("targets"))?;
                let to = self.writable(&a("targetObjective"))?;
                let ss = self.arg_score_holders(s, &a("source"))?;
                let so = self.objective_or_fail(&a("sourceObjective"))?;
                let oper = a("operation");
                let mut t: Track<String> = Track::default();
                // Like the game: target then each source opened (created without a packet), only
                // writes send; a score created by the current access is sent at its
                // first write, even unchanged.
                for h in &ts {
                    let mut tsync = self.cmd.scores.touch(h, &to.name);
                    for src in &ss {
                        let mut ssync = self.cmd.scores.touch(src, &so.name);
                        let x = self.cmd.scores.get(h, &to.name).unwrap_or(0);
                        let y = self.cmd.scores.get(src, &so.name).unwrap_or(0);
                        let v = match oper.as_str() {
                            "=" => y,
                            "+=" => x.wrapping_add(y),
                            "-=" => x.wrapping_sub(y),
                            "*=" => x.wrapping_mul(y),
                            // Zero divisor: failure; opened scores already created, previous
                            // targets already written.
                            "/=" | "%=" if y == 0 => return Err(Fail(tr!("arguments.operation.div0"))),
                            "/=" => floor_div(x, y),
                            "%=" => floor_mod(x, y),
                            "<" => x.min(y),
                            ">" => x.max(y),
                            _ => {
                                // Swap: the target first, then the source.
                                self.score_write(h, &to.name, y, &mut tsync, None);
                                self.score_write(src, &so.name, x, &mut ssync, None);
                                continue;
                            }
                        };
                        self.score_write(h, &to.name, v, &mut tsync, None);
                    }
                    t.track(h.clone(), self.cmd.scores.get(h, &to.name).unwrap_or(0));
                }
                let n = objective_name(&to);
                let total = t.total;
                let r = self.track_feedback(
                    s,
                    &t,
                    true,
                    None,
                    |me, h| {
                        tr!("commands.scoreboard.players.operation.success.single", n.clone(), me.holder_name(h), total)
                    },
                    |c| tr!("commands.scoreboard.players.operation.success.multiple", n.clone(), c),
                );
                self.flush_scores();
                r
            }
            "display" => {
                let hs = self.arg_score_holders(s, &a("targets"))?;
                let o = self.objective_or_fail(&a("objective"))?;
                let what = w.get(3).copied().unwrap_or("name");
                let mut t: Track<String> = Track::default();
                let mut set_name: Option<Component> = None;
                let mut format_set = false;
                for h in &hs {
                    if what == "name" {
                        let c = match p.arg(seg, "name") {
                            Some(x) => Some(self.arg_component(s, &x, None)?),
                            None => None,
                        };
                        set_name = c.clone();
                        self.cmd.scores.set_display(h, &o.name, c.as_ref().map(text::to_nbt));
                    } else {
                        let f = self.number_format(s, p, seg, w, 6)?;
                        format_set = f.is_some();
                        self.cmd.scores.set_number_format(h, &o.name, f);
                    }
                    t.track(h.clone(), 1);
                }
                let n = objective_name(&o);
                let r = if what == "name" {
                    match set_name {
                        Some(c) => self.track_feedback(
                            s,
                            &t,
                            false,
                            None,
                            |me, h| {
                                tr!(
                                    "commands.scoreboard.players.display.name.set.success.single",
                                    c.clone(),
                                    me.holder_name(h),
                                    n.clone()
                                )
                            },
                            |k| {
                                tr!(
                                    "commands.scoreboard.players.display.name.set.success.multiple",
                                    c.clone(),
                                    k,
                                    n.clone()
                                )
                            },
                        ),
                        None => self.track_feedback(
                            s,
                            &t,
                            false,
                            None,
                            |me, h| {
                                tr!(
                                    "commands.scoreboard.players.display.name.clear.success.single",
                                    me.holder_name(h),
                                    n.clone()
                                )
                            },
                            |k| tr!("commands.scoreboard.players.display.name.clear.success.multiple", k, n.clone()),
                        ),
                    }
                } else {
                    let k = if format_set { "set" } else { "clear" };
                    self.track_feedback(
                        s,
                        &t,
                        false,
                        None,
                        |me, h| {
                            tr!(
                                &format!("commands.scoreboard.players.display.numberFormat.{k}.success.single"),
                                me.holder_name(h),
                                n.clone()
                            )
                        },
                        |c| {
                            tr!(
                                &format!("commands.scoreboard.players.display.numberFormat.{k}.success.multiple"),
                                c,
                                n.clone()
                            )
                        },
                    )
                };
                self.flush_scores();
                r
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    pub(crate) fn cmd_team(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let r = self.team_op(s, p, seg);
        self.flush_scores();
        r
    }

    fn team_op(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        let team_of =
            |me: &Server, n: &str| me.cmd.scores.team(n).cloned().ok_or_else(|| Fail(tr!("team.notFound", n)));
        match w.get(1).copied() {
            Some("list") => match p.arg(seg, "team") {
                None => {
                    let ts: Vec<Component> = self.cmd.scores.teams_listed().into_iter().map(team_name).collect();
                    let n = ts.len() as i32;
                    if n == 0 {
                        self.ok(s, tr!("commands.team.list.teams.empty"));
                    } else {
                        self.ok(s, tr!("commands.team.list.teams.success", n, text::list(ts)));
                    }
                    Ok(n)
                }
                Some(tn) => {
                    let t = team_of(self, &tn)?;
                    let n = t.players.len() as i32;
                    if n == 0 {
                        self.ok(s, tr!("commands.team.list.members.empty", team_name(&t)));
                    } else {
                        let mut ps = t.players.clone();
                        ps.sort();
                        let list = text::list(ps.into_iter().map(|x| text::colored(lit(x), text::GREEN)).collect());
                        self.ok(s, tr!("commands.team.list.members.success", team_name(&t), n, list));
                    }
                    Ok(n)
                }
            },
            Some("add") => {
                let name = a("team");
                if self.cmd.scores.team(&name).is_some() {
                    return Err(Fail(tr!("commands.team.add.duplicate")));
                }
                let display = match p.arg(seg, "displayName") {
                    Some(x) => text::to_nbt(&self.arg_component(s, &x, None)?),
                    None => Nbt::String(name.clone()),
                };
                let t = Team {
                    name: name.clone(),
                    display,
                    color: None,
                    friendly_fire: true,
                    see_invisibles: true,
                    prefix: Nbt::String(String::new()),
                    suffix: Nbt::String(String::new()),
                    nametag: "always".into(),
                    death: "always".into(),
                    collision: "always".into(),
                    players: Vec::new(),
                };
                self.cmd.scores.out.push(ScoreEvent::Team {
                    name: name.clone(),
                    mode: 0,
                    params: Some(Board::team_params(&t)),
                    members: Vec::new(),
                });
                self.cmd.scores.add_team(t.clone());
                self.cmd.scores.dirty = true;
                self.cmd.data.teams.push(name);
                self.ok(s, tr!("commands.team.add.success", team_name(&t)));
                Ok(self.cmd.scores.teams.len() as i32)
            }
            Some("remove") => {
                let t = team_of(self, &a("team"))?;
                self.cmd.scores.remove_team(&t.name);
                self.cmd.data.teams.retain(|x| *x != t.name);
                self.cmd.scores.dirty = true;
                self.cmd.scores.out.push(ScoreEvent::Team {
                    name: t.name.clone(),
                    mode: 1,
                    params: None,
                    members: Vec::new(),
                });
                self.ok(s, tr!("commands.team.remove.success", team_name(&t)));
                Ok(self.cmd.scores.teams.len() as i32)
            }
            Some("empty") => {
                let t = team_of(self, &a("team"))?;
                if t.players.is_empty() {
                    return Err(Fail(tr!("commands.team.empty.unchanged")));
                }
                // One removal packet per member, like the game.
                let n = t.players.len();
                for m in &t.players {
                    self.cmd.scores.leave_team(m);
                }
                self.ok(s, tr!("commands.team.empty.success", n, team_name(&t)));
                Ok(n as i32)
            }
            Some("join") => {
                let t = team_of(self, &a("team"))?;
                let members = match p.arg(seg, "members") {
                    Some(m) => self.arg_score_holders(s, &m)?,
                    None => vec![
                        s.entity
                            .map(|e| self.ent_score_name(e))
                            .ok_or_else(|| Fail(tr!("permissions.requires.entity")))?,
                    ],
                };
                let mut tr_: Track<String> = Track::default();
                for m in &members {
                    let joined = self.cmd.scores.join_team(m, &t.name);
                    tr_.track(m.clone(), i32::from(joined));
                }
                let tn = team_name(&t);
                self.track_feedback(
                    s,
                    &tr_,
                    false,
                    None,
                    |me, h| tr!("commands.team.join.success.single", me.holder_name(h), tn.clone()),
                    |c| tr!("commands.team.join.success.multiple", c, tn.clone()),
                )
            }
            Some("leave") => {
                let members = self.arg_score_holders(s, &a("members"))?;
                let mut tr_: Track<String> = Track::default();
                for m in &members {
                    let left = self.cmd.scores.leave_team(m);
                    tr_.track(m.clone(), i32::from(left));
                }
                self.track_feedback(
                    s,
                    &tr_,
                    false,
                    None,
                    |me, h| tr!("commands.team.leave.success.single", me.holder_name(h)),
                    |c| tr!("commands.team.leave.success.multiple", c),
                )
            }
            Some("modify") => self.team_modify(s, p, seg, &w),
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    fn team_modify(&mut self, s: &Src, p: &Parsed, seg: &Seg, w: &[&str]) -> R {
        let name = p.arg(seg, "team").unwrap_or_default();
        let Some(mut t) = self.cmd.scores.team(&name).cloned() else { return Err(Fail(tr!("team.notFound", name))) };
        let opt = w.get(3).copied().unwrap_or("");
        let val = p
            .arg(seg, "value")
            .or_else(|| p.arg(seg, "allowed"))
            .or_else(|| w.get(4).map(|x| (*x).to_owned()))
            .unwrap_or_default();
        let r: R = match opt {
            "displayName" => {
                let c = self.arg_component(s, &p.arg(seg, "displayName").unwrap_or_default(), None)?;
                let n = text::to_nbt(&c);
                if crate::items::nbt_eq(&n, &t.display) {
                    return Err(Fail(tr!("commands.team.option.name.unchanged")));
                }
                t.display = n;
                self.ok(s, tr!("commands.team.option.name.success", team_name(&t)));
                Ok(0)
            }
            "color" => {
                let c = p.arg(seg, "value").unwrap_or_default();
                if c == "reset" {
                    if t.color.is_none() {
                        return Err(Fail(tr!("commands.team.option.color.unchanged")));
                    }
                    t.color = None;
                    self.ok(s, tr!("commands.team.option.color.clear.success", team_name(&t)));
                } else {
                    if t.color.as_deref() == Some(c.as_str()) {
                        return Err(Fail(tr!("commands.team.option.color.unchanged")));
                    }
                    t.color = Some(c.clone());
                    self.ok(s, tr!("commands.team.option.color.success", team_name(&t), c));
                }
                Ok(0)
            }
            "friendlyFire" | "seeFriendlyInvisibles" => {
                let v = val == "true";
                let cur = if opt == "friendlyFire" { t.friendly_fire } else { t.see_invisibles };
                let k = if opt == "friendlyFire" { "friendlyfire" } else { "seeFriendlyInvisibles" };
                if cur == v {
                    let e = if v { "alreadyEnabled" } else { "alreadyDisabled" };
                    return Err(Fail(tr!(&format!("commands.team.option.{k}.{e}"))));
                }
                if opt == "friendlyFire" {
                    t.friendly_fire = v;
                } else {
                    t.see_invisibles = v;
                }
                let e = if v { "enabled" } else { "disabled" };
                self.ok(s, tr!(&format!("commands.team.option.{k}.{e}"), team_name(&t)));
                Ok(0)
            }
            "nametagVisibility" | "deathMessageVisibility" | "collisionRule" => {
                let cur = match opt {
                    "nametagVisibility" => &mut t.nametag,
                    "deathMessageVisibility" => &mut t.death,
                    _ => &mut t.collision,
                };
                if *cur == val {
                    return Err(Fail(tr!(&format!("commands.team.option.{opt}.unchanged"))));
                }
                *cur = val.clone();
                let disp = if opt == "collisionRule" {
                    tr!(&format!("team.collision.{val}"))
                } else {
                    tr!(&format!("team.visibility.{val}"))
                };
                self.ok(s, tr!(&format!("commands.team.option.{opt}.success"), team_name(&t), disp));
                Ok(0)
            }
            "prefix" | "suffix" => {
                let c = self.arg_component(s, &p.arg(seg, opt).unwrap_or_default(), None)?;
                if opt == "prefix" {
                    t.prefix = text::to_nbt(&c);
                } else {
                    t.suffix = text::to_nbt(&c);
                }
                self.ok(s, tr!(&format!("commands.team.option.{opt}.success"), c));
                Ok(1)
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        };
        if let Some(x) = self.cmd.scores.teams.iter_mut().find(|x| x.name == t.name) {
            *x = t.clone();
        }
        self.cmd.scores.dirty = true;
        self.cmd.scores.out.push(ScoreEvent::Team {
            name: t.name.clone(),
            mode: 2,
            params: Some(Board::team_params(&t)),
            members: Vec::new(),
        });
        r
    }

    pub(crate) fn cmd_trigger(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let holder = match s.entity {
            Some(super::ents::E::Player) => self.name.clone(),
            _ => return Err(Fail(tr!("permissions.requires.player"))),
        };
        let o = self.objective_or_fail(&p.arg(seg, "objective").unwrap_or_default())?;
        if o.criteria != "trigger" {
            return Err(Fail(tr!("commands.trigger.failed.invalid")));
        }
        let Some(sc) = self.cmd.scores.score(&holder, &o.name).cloned() else {
            return Err(Fail(tr!("commands.trigger.failed.unprimed")));
        };
        if sc.locked {
            return Err(Fail(tr!("commands.trigger.failed.unprimed")));
        }
        let v: i32 = p.arg(seg, "value").and_then(|x| x.parse().ok()).unwrap_or(1);
        let (nv, msg) = match seg.path.get(2).map(String::as_str) {
            Some("add") => (sc.value.wrapping_add(v), tr!("commands.trigger.add.success", objective_name(&o), v)),
            Some("set") => (v, tr!("commands.trigger.set.success", objective_name(&o), v)),
            _ => (sc.value.wrapping_add(1), tr!("commands.trigger.simple.success", objective_name(&o))),
        };
        self.score_set(&holder, &o.name, nv);
        self.cmd.scores.set_locked(&holder, &o.name, true);
        self.flush_scores();
        self.ok(s, msg);
        Ok(nv)
    }
}

/// Read-only criteria (computed by the game).
fn is_read_only(c: &str) -> bool {
    matches!(c, "health" | "food" | "air" | "armor" | "xp" | "level")
}

/// Integer division rounded down, like Java.
fn floor_div(x: i32, y: i32) -> i32 {
    let q = x.wrapping_div(y);
    if x.wrapping_rem(y) != 0 && ((x ^ y) < 0) { q - 1 } else { q }
}

/// Remainder of the division rounded down, like Java (MIN % -1 = 0, no overflow).
fn floor_mod(x: i32, y: i32) -> i32 {
    let m = x.wrapping_rem(y);
    if m != 0 && ((m < 0) != (y < 0)) { m + y } else { m }
}
