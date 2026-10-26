//! Commands added by S9 (`commands.md` § 6.4): `advancement`, `stopwatch`, `dialog`,
//! `waypoint`, `ride`, `spreadplayers`, `fillbiome`, `datapack`, `reload`, `spectate`.

use std::collections::BTreeMap;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_text::Component;

use super::cmds_basic::Track;
use super::ents::E;
use super::src::{Fail, Src};
use super::text::{self, lit};
use super::{Parsed, R, Seg, args};
use crate::server::Server;
use crate::tr;

/// State of the S9 commands (stopwatches, waypoints, mounts).
#[derive(Debug, Default)]
pub struct State {
    /// Stopwatch: creation instant (monotonic ms), time already accumulated (ms).
    pub stopwatches: BTreeMap<String, (i64, i64)>,
    pub stopwatches_dirty: bool,
    pub stopwatches_loaded: bool,
    /// Waypoint icon per entity: color, style.
    pub waypoints: BTreeMap<i32, (Option<i32>, Option<String>)>,
    /// Player camera (`spectate`): followed entity.
    pub camera: Option<E>,
}

/// Monotonic clock in milliseconds.
pub fn millis() -> i64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64
}

impl Server {
    /// Dispatch of the S9 commands; `None` if the command is not one of them.
    pub(crate) fn dispatch_s9(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> Option<R> {
        let head = seg.path.first().map(String::as_str).unwrap_or("");
        Some(match head {
            "advancement" => self.cmd_advancement(s, p, seg),
            "stopwatch" => self.cmd_stopwatch(s, p, seg),
            "dialog" => self.cmd_dialog(s, p, seg),
            "waypoint" => self.cmd_waypoint(s, p, seg),
            "ride" => self.cmd_ride(s, p, seg),
            "spreadplayers" => self.cmd_spreadplayers(s, p, seg),
            "fillbiome" => self.cmd_fillbiome(s, p, seg),
            "datapack" => self.cmd_datapack(s, p, seg),
            "reload" => {
                self.ok(s, tr!("commands.reload.success"));
                self.s9_reload();
                Ok(0)
            }
            "spectate" => self.cmd_spectate(s, p, seg),
            "loot" => self.cmd_loot_s9(s, p, seg),
            "item" => self.cmd_item_s9(s, p, seg),
            "attribute" => self.cmd_attribute_s9(s, p, seg),
            "place" => self.cmd_place(s, p, seg),
            _ => return None,
        })
    }

    // ---- advancement ------------------------------------------------------------------------

    fn cmd_advancement(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let grant = w.get(1) == Some(&"grant");
        let key = if grant { "grant" } else { "revoke" };
        let targets = self.arg_players(s, &p.arg(seg, "targets").unwrap_or_default())?;
        let mode = w.get(3).copied().unwrap_or("everything");
        let tree = self.prog.tree.clone();
        let adv_id = p.arg(seg, "advancement").map(|a| args::id(&a));
        let target = match &adv_id {
            Some(id) => Some(tree.get(id).ok_or_else(|| Fail(tr!("advancement.advancementNotFound", id.clone())))?),
            None => None,
        };
        // Criterion alone.
        if let (Some(a), Some(crit)) = (target, p.arg(seg, "criterion")) {
            let Some(c) = tree.list[a].criteria.iter().position(|x| x.name == crit) else {
                return Err(Fail(tr!("commands.advancement.criterionNotFound", self.adv_name(a), crit)));
            };
            let mut t: Track<E> = Track::default();
            for e in &targets {
                let ok = if grant { self.adv_award(a, c) } else { self.adv_revoke(a, c) };
                t.track(*e, i32::from(ok));
            }
            let name = self.adv_name(a);
            if t.total == 0 {
                let (first, n) = t.pick(true);
                return Err(Fail(match first {
                    Some(e) => tr!(
                        &format!("commands.advancement.{key}.criterion.to.one.failure"),
                        crit.clone(),
                        name,
                        self.ent_display(*e)
                    ),
                    None => {
                        tr!(&format!("commands.advancement.{key}.criterion.to.many.failure"), crit.clone(), name, n)
                    }
                }));
            }
            let crit2 = crit.clone();
            let name2 = self.adv_name(a);
            return self.track_feedback(
                s,
                &t,
                false,
                None,
                |sv, e| {
                    tr!(
                        &format!("commands.advancement.{key}.criterion.to.one.success"),
                        crit.clone(),
                        sv.adv_name(a),
                        sv.ent_display(*e)
                    )
                },
                |n| {
                    tr!(
                        &format!("commands.advancement.{key}.criterion.to.many.success"),
                        crit2.clone(),
                        name2.clone(),
                        n
                    )
                },
            );
        }
        // Set of advancements according to the mode.
        let list: Vec<usize> = match (mode, target) {
            ("everything", _) | (_, None) => (0..tree.list.len()).collect(),
            (m, Some(a)) => {
                let (parents, children) = match m {
                    "only" => (false, false),
                    "through" => (true, true),
                    "from" => (false, true),
                    _ => (true, false),
                };
                let mut v = Vec::new();
                if parents {
                    let mut x = tree.list[a].parent;
                    while let Some(pa) = x {
                        v.push(pa);
                        x = tree.list[pa].parent;
                    }
                }
                v.push(a);
                if children {
                    fn add(t: &crate::progress::Tree, n: usize, out: &mut Vec<usize>) {
                        for &c in &t.list[n].children {
                            out.push(c);
                            add(t, c, out);
                        }
                    }
                    add(&tree, a, &mut v);
                }
                v
            }
        };
        let show = mode != "everything" || !grant;
        let mut t: Track<E> = Track::default();
        for e in &targets {
            if !show {
                self.adv_flush(true);
            }
            let mut n = 0;
            for &a in &list {
                let did = if grant {
                    if self.prog.done(a) {
                        false
                    } else {
                        let crits: Vec<usize> =
                            (0..tree.list[a].criteria.len()).filter(|&c| !self.prog.crit_done(a, c)).collect();
                        for c in crits {
                            self.adv_award(a, c);
                        }
                        true
                    }
                } else if !self.prog.has_progress(a) {
                    false
                } else {
                    let crits: Vec<usize> =
                        (0..tree.list[a].criteria.len()).filter(|&c| self.prog.crit_done(a, c)).collect();
                    for c in crits {
                        self.adv_revoke(a, c);
                    }
                    true
                };
                n += i32::from(did);
            }
            if !show {
                self.adv_flush(false);
            }
            t.track(*e, n);
        }
        let count = list.len();
        if count == 1 {
            let a = list[0];
            if t.total == 0 {
                let (first, n) = t.pick(true);
                let name = self.adv_name(a);
                return Err(Fail(match first {
                    Some(e) => {
                        tr!(&format!("commands.advancement.{key}.one.to.one.failure"), name, self.ent_display(*e))
                    }
                    None => tr!(&format!("commands.advancement.{key}.one.to.many.failure"), name, n),
                }));
            }
            let name = self.adv_name(a);
            return self.track_feedback(
                s,
                &t,
                false,
                None,
                |sv, e| {
                    tr!(&format!("commands.advancement.{key}.one.to.one.success"), sv.adv_name(a), sv.ent_display(*e))
                },
                |n| tr!(&format!("commands.advancement.{key}.one.to.many.success"), name.clone(), n),
            );
        }
        if t.total == 0 {
            let (first, n) = t.pick(true);
            return Err(Fail(match first {
                Some(e) => tr!(&format!("commands.advancement.{key}.many.to.one.failure"), count, self.ent_display(*e)),
                None => tr!(&format!("commands.advancement.{key}.many.to.many.failure"), count, n),
            }));
        }
        self.track_feedback(
            s,
            &t,
            false,
            None,
            |sv, e| tr!(&format!("commands.advancement.{key}.many.to.one.success"), count, sv.ent_display(*e)),
            |n| tr!(&format!("commands.advancement.{key}.many.to.many.success"), count, n),
        )
    }

    // ---- stopwatch --------------------------------------------------------------------------

    fn stopwatches_load(&mut self) {
        if self.cmd.s9.stopwatches_loaded {
            return;
        }
        self.cmd.s9.stopwatches_loaded = true;
        let path = self.world.join("data").join("minecraft").join("stopwatches.dat");
        let Ok(root) = crate::nbtio::read_dat(&path) else { return };
        let now = millis();
        if let Some(Nbt::Compound(f)) = root.get("data").and_then(|d| d.get("stopwatches")) {
            for (k, v) in f {
                self.cmd.s9.stopwatches.insert(k.clone(), (now, v.as_i64().unwrap_or(0)));
            }
        }
    }

    /// Stopwatches file (always written if there are any, like the game).
    pub(crate) fn stopwatches_job(&self) -> Option<crate::save::Job> {
        let c = &self.cmd.s9;
        if c.stopwatches.is_empty() && !c.stopwatches_dirty {
            return None;
        }
        let now = millis();
        let map = c.stopwatches.iter().map(|(k, (t0, acc))| (k.clone(), Nbt::Long(acc + now - t0))).collect();
        let root = Nbt::Compound(vec![
            ("data".into(), Nbt::Compound(vec![("stopwatches".into(), Nbt::Compound(map))])),
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
        ]);
        let path = self.world.join("data").join("minecraft").join("stopwatches.dat");
        Some(crate::save::Job::Dat { path, nbt: crate::nbtio::encode_root(&root), backup: false })
    }

    /// Seconds elapsed on stopwatch `id` (`None`: unknown), read by `stopwatch query` and
    /// `execute if|unless stopwatch`.
    pub(crate) fn stopwatch_seconds(&mut self, id: &str) -> Option<f64> {
        self.stopwatches_load();
        let (t0, acc) = self.cmd.s9.stopwatches.get(id).copied()?;
        Some((acc + millis() - t0) as f64 / 1000.0)
    }

    fn cmd_stopwatch(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        self.stopwatches_load();
        let id = args::id(&p.arg(seg, "id").unwrap_or_default());
        let sub = seg.path.get(1).map(String::as_str).unwrap_or("");
        let now = millis();
        let sw = &mut self.cmd.s9;
        match sub {
            "create" => {
                if sw.stopwatches.contains_key(&id) {
                    return Err(Fail(tr!("commands.stopwatch.already_exists", id)));
                }
                sw.stopwatches.insert(id.clone(), (now, 0));
                sw.stopwatches_dirty = true;
                self.ok(s, tr!("commands.stopwatch.create.success", id));
                Ok(1)
            }
            "query" => {
                let Some(secs) = self.stopwatch_seconds(&id) else {
                    return Err(Fail(tr!("commands.stopwatch.does_not_exist", id)));
                };
                let scale: f64 = p.arg(seg, "scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
                self.ok(s, tr!("commands.stopwatch.query", id, text::jd(secs)));
                Ok((secs * scale) as i32)
            }
            "restart" => {
                let Some(x) = sw.stopwatches.get_mut(&id) else {
                    return Err(Fail(tr!("commands.stopwatch.does_not_exist", id)));
                };
                *x = (now, 0);
                sw.stopwatches_dirty = true;
                self.ok(s, tr!("commands.stopwatch.restart.success", id));
                Ok(1)
            }
            _ => {
                if sw.stopwatches.remove(&id).is_none() {
                    return Err(Fail(tr!("commands.stopwatch.does_not_exist", id)));
                }
                sw.stopwatches_dirty = true;
                self.ok(s, tr!("commands.stopwatch.remove.success", id));
                Ok(1)
            }
        }
    }

    // ---- dialog -----------------------------------------------------------------------------

    fn cmd_dialog(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let targets = self.arg_players(s, &p.arg(seg, "targets").unwrap_or_default())?;
        let show = seg.path.get(1).map(String::as_str) == Some("show");
        let mut t: Track<E> = Track::default();
        if show {
            let text = p.arg(seg, "dialog").unwrap_or_default();
            let r = if text.trim_start().starts_with('{') {
                let n = super::snbt::parse(&text).map_err(|_| Fail(tr!("argument.nbt.expected.compound")))?;
                rc_client::extra::DialogRef::Inline(n)
            } else {
                let id = args::id(&text);
                let idx = self.regs.entries("minecraft:dialog").iter().position(|d| *d == id);
                let Some(i) = idx else {
                    return Err(Fail(tr!("argument.resource_or_id.failed_to_parse", id)));
                };
                rc_client::extra::DialogRef::Registry(i as i32)
            };
            for e in &targets {
                let ev = match &r {
                    rc_client::extra::DialogRef::Inline(n) => rc_client::extra::DialogRef::Inline(n.clone()),
                    rc_client::extra::DialogRef::Registry(i) => rc_client::extra::DialogRef::Registry(*i),
                };
                let _ = self.send(Event::ShowDialog(ev));
                t.track(*e, 1);
            }
            self.track_feedback(
                s,
                &t,
                false,
                None,
                |sv, e| tr!("commands.dialog.show.single", sv.ent_display(*e)),
                |n| tr!("commands.dialog.show.multiple", n),
            )
        } else {
            for e in &targets {
                let _ = self.send(Event::ClearDialog);
                t.track(*e, 1);
            }
            self.track_feedback(
                s,
                &t,
                false,
                None,
                |sv, e| tr!("commands.dialog.clear.single", sv.ent_display(*e)),
                |n| tr!("commands.dialog.clear.multiple", n),
            )
        }
    }

    // ---- waypoint ---------------------------------------------------------------------------

    fn cmd_waypoint(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        if w.get(1) == Some(&"list") {
            let dim = s.dim.clone();
            // Emitters: living entities with an emission range (the player by default).
            let mut names = Vec::new();
            if !self.life.vit.dead && dim == self.dim.name {
                let pos = crate::mobs::mth::block_of(self.pos);
                let mut c = self.ent_display(E::Player);
                text::suggest_click(&mut c, format!("/execute in {dim} run tp @s {} {} {}", pos[0], pos[1], pos[2]));
                text::hover_text(&mut c, tr!("chat.coordinates.tooltip"));
                let color = self.cmd.s9.waypoints.get(&self.player_id).and_then(|x| x.0).unwrap_or(-1);
                c.style.color = Some(color as u32 & 0xFF_FFFF);
                names.push(c);
            }
            if names.is_empty() {
                self.ok(s, tr!("commands.waypoint.list.empty", dim));
                return Ok(0);
            }
            let n = names.len();
            self.ok(s, tr!("commands.waypoint.list.success", n, dim, text::list(names)));
            return Ok(n as i32);
        }
        let e = self.arg_entity(s, &p.arg(seg, "waypoint").unwrap_or_default(), false)?;
        if !self.ent_is_living(e) {
            return Err(Fail(tr!("argument.waypoint.invalid")));
        }
        let id = self.ent_id(e);
        let entry = self.cmd.s9.waypoints.entry(id).or_default();
        match (w.get(3).copied(), w.get(4).copied()) {
            (Some("color"), Some("reset")) => {
                entry.0 = None;
                self.ok(s, tr!("commands.waypoint.modify.color.reset"));
            }
            (Some("color"), Some("hex")) => {
                let hex = p.arg(seg, "color").unwrap_or_default();
                let v = i32::from_str_radix(&hex, 16).unwrap_or(0);
                entry.0 = Some(v);
                let mut c = lit(format!("{:06X}", v & 0xFF_FFFF));
                c.style.color = Some(v as u32 & 0xFF_FFFF);
                self.ok(s, tr!("commands.waypoint.modify.color", c));
            }
            (Some("color"), _) => {
                let name = p.arg(seg, "color").unwrap_or_default();
                let rgb = team_color_rgb(&name);
                entry.0 = Some(rgb);
                let mut c = lit(name.clone());
                c.style.color = rc_text::parse_color(&name);
                self.ok(s, tr!("commands.waypoint.modify.color", c));
            }
            (Some("style"), Some("reset")) => {
                entry.1 = None;
                self.ok(s, tr!("commands.waypoint.modify.style"));
            }
            _ => {
                entry.1 = Some(args::id(&p.arg(seg, "style").unwrap_or_default()));
                self.ok(s, tr!("commands.waypoint.modify.style"));
            }
        }
        Ok(0)
    }

    // ---- ride -------------------------------------------------------------------------------

    /// Vehicle of an entity (mob mounts, F04).
    pub(crate) fn vehicle_of(&self, e: E) -> Option<E> {
        match e {
            E::Player => self.mobs.mounts.riding.vehicle.map(E::Mob).or_else(|| self.pcart.vehicle.map(E::Other)),
            E::Mob(i) => self.mobs.get(i).and_then(|m| m.mount.vehicle).map(|v| self.vehicle_e(v)),
            _ => None,
        }
    }

    /// G5-F06: vehicle designated by its identifier (mob or minecart).
    pub(crate) fn vehicle_e(&self, id: i32) -> E {
        if self.cart_ref(id).is_some() { E::Other(id) } else { E::Mob(id) }
    }

    fn cmd_ride(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        use crate::mobs::mob::Ent;
        let target = self.arg_entity(s, &p.arg(seg, "target").unwrap_or_default(), false)?;
        if seg.path.get(2).map(String::as_str) == Some("dismount") {
            let Some(v) = self.vehicle_of(target) else {
                return Err(Fail(tr!("commands.ride.not_riding", self.ent_display(target))));
            };
            // Dismount through the mount system.
            match (target, v) {
                // G5-F06: minecart.
                (E::Player, E::Other(_)) => self.player_stop_riding_cart(),
                (E::Mob(r), E::Other(cid)) => self.cart_dismount_mob(cid, r),
                (E::Player, _) => self.player_stop_riding(),
                (E::Mob(r), E::Mob(vid)) => {
                    if let (Some(ri), Some(vi)) = (self.mob_index(r), self.mob_index(vid))
                        && let (Some(mut rider), Some(mut veh)) = (self.mobs.list[ri].take(), self.mobs.list[vi].take())
                    {
                        self.mob_dismount_from(&mut rider, &mut veh);
                        self.send_passengers(&mut veh);
                        self.mobs.list[ri] = Some(rider);
                        self.mobs.list[vi] = Some(veh);
                    }
                }
                _ => {}
            }
            self.ok(s, tr!("commands.ride.dismount.success", self.ent_display(target), self.ent_display(v)));
            return Ok(1);
        }
        let vehicle = self.arg_entity(s, &p.arg(seg, "vehicle").unwrap_or_default(), false)?;
        if let Some(cur) = self.vehicle_of(target) {
            return Err(Fail(tr!("commands.ride.already_riding", self.ent_display(target), self.ent_display(cur))));
        }
        if vehicle == E::Player {
            return Err(Fail(tr!("commands.ride.mount.failure.cant_ride_players")));
        }
        // Loop: the vehicle is already carried (directly or not) by the target.
        let mut x = Some(vehicle);
        while let Some(v) = x {
            if v == target {
                return Err(Fail(tr!("commands.ride.mount.failure.loop")));
            }
            x = self.vehicle_of(v);
        }
        // G5-F06: minecart, forced mount.
        if let E::Other(cid) = vehicle
            && self.cart_ref(cid).is_some()
        {
            if !self.cart_ride_forced(cid, target) {
                return Err(Fail(tr!("commands.ride.mount.failure.generic")));
            }
            self.ok(s, tr!("commands.ride.mount.success", self.ent_display(target), self.ent_display(vehicle)));
            return Ok(1);
        }
        // Forced mount: mobs and minecarts serve as vehicles here.
        let E::Mob(vid) = vehicle else {
            return Err(Fail(tr!("commands.ride.mount.failure.generic")));
        };
        let Some(vi) = self.mob_index(vid) else {
            return Err(Fail(tr!("commands.ride.mount.failure.generic")));
        };
        let Some(mut veh) = self.mobs.list[vi].take() else {
            return Err(Fail(tr!("commands.ride.mount.failure.generic")));
        };
        let ok = match target {
            E::Player => {
                self.mobs.mounts.riding.vehicle = Some(veh.id);
                self.prog.ride_started = true; // W8c: `started_riding`
                veh.mount.passengers.push(Ent::Player);
                let rank = veh.mount.passengers.len() - 1;
                let seat = self.seat_of(&veh, rank, None);
                self.mobs.mounts.riding.last_good = veh.pos;
                self.teleport_to(seat);
                self.last_good = seat;
                true
            }
            E::Mob(r) => match self.mob_index(r).and_then(|ri| self.mobs.list[ri].take().map(|m| (ri, m))) {
                Some((ri, mut rider)) => {
                    let ok = self.mob_start_riding(&mut rider, &mut veh, true);
                    rider.needs_sync = true;
                    self.mobs.list[ri] = Some(rider);
                    ok
                }
                None => false,
            },
            _ => false,
        };
        if ok {
            self.send_passengers(&mut veh);
        }
        self.mobs.list[vi] = Some(veh);
        if !ok {
            return Err(Fail(tr!("commands.ride.mount.failure.generic")));
        }
        self.ok(s, tr!("commands.ride.mount.success", self.ent_display(target), self.ent_display(vehicle)));
        Ok(1)
    }

    // ---- spectate ---------------------------------------------------------------------------

    fn cmd_spectate(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let player = match p.arg(seg, "player") {
            Some(t) => self.arg_entity(s, &t, true)?,
            None => self.source_player(s)?,
        };
        let target = match p.arg(seg, "target") {
            Some(t) => Some(self.arg_entity(s, &t, false)?),
            None => None,
        };
        if target == Some(player) {
            return Err(Fail(tr!("commands.spectate.self")));
        }
        if self.game_mode != 3 {
            return Err(Fail(tr!("commands.spectate.not_spectator", self.ent_display(player))));
        }
        match target {
            Some(t) => {
                self.prog.cmds.camera = Some(t);
                let _ = self.send(Event::SetCamera(self.ent_id(t)));
                self.ok(s, tr!("commands.spectate.success.started", self.ent_display(t)));
            }
            None => {
                self.prog.cmds.camera = None;
                let _ = self.send(Event::SetCamera(self.player_id));
                self.ok(s, tr!("commands.spectate.success.stopped"));
            }
        }
        Ok(1)
    }

    // ---- spreadplayers ----------------------------------------------------------------------

    fn cmd_spreadplayers(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let center = args::vec2(s, &p.arg(seg, "center").unwrap_or_default());
        let spread: f64 = f64::from(p.arg(seg, "spreadDistance").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0));
        let range: f64 = f64::from(p.arg(seg, "maxRange").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0));
        let teams = p.arg(seg, "respectTeams").as_deref() == Some("true");
        let top = self.dim.height.min_y + self.dim.height.sections as i32 * 16;
        let max_height: i32 = p.arg(seg, "maxHeight").and_then(|v| v.parse().ok()).unwrap_or(top);
        let targets = self.arg_entities(s, &p.arg(seg, "targets").unwrap_or_default(), false)?;
        let min_y = self.dim.height.min_y;
        if max_height < min_y {
            return Err(Fail(tr!("commands.spreadplayers.failed.invalid.height", max_height, min_y)));
        }
        let (x0, z0, x1, z1) = (center[0] - range, center[1] - range, center[0] + range, center[1] + range);
        let groups: Vec<Option<String>> = if teams {
            let mut g: Vec<Option<String>> = Vec::new();
            for e in &targets {
                let t = if *e == E::Player {
                    self.cmd.scores.team_of(&self.ent_score_name(*e)).map(|t| t.name.clone())
                } else {
                    None
                };
                if !g.contains(&t) {
                    g.push(t);
                }
            }
            g
        } else {
            Vec::new()
        };
        let count = if teams { groups.len() } else { targets.len() };
        let mut rng = crate::rng::Rng::from_time();
        let mut pos: Vec<[f64; 2]> =
            (0..count).map(|_| [x0 + rng.next_double() * (x1 - x0), z0 + rng.next_double() * (z1 - z0)]).collect();
        let dist = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
        let mut collisions = true;
        let mut min_dist = f64::from(f32::MAX);
        let mut it = 0;
        while it < 10000 && collisions {
            collisions = false;
            min_dist = f64::from(f32::MAX);
            for i in 0..pos.len() {
                let mut n = 0;
                let mut avg = [0.0f64; 2];
                for j in 0..pos.len() {
                    if i == j {
                        continue;
                    }
                    let d = dist(pos[i], pos[j]);
                    min_dist = min_dist.min(d);
                    if d < spread {
                        n += 1;
                        avg[0] += pos[j][0] - pos[i][0];
                        avg[1] += pos[j][1] - pos[i][1];
                    }
                }
                if n > 0 {
                    avg[0] /= f64::from(n);
                    avg[1] /= f64::from(n);
                    let len = (avg[0] * avg[0] + avg[1] * avg[1]).sqrt();
                    if len > 0.0 {
                        pos[i][0] -= avg[0] / len;
                        pos[i][1] -= avg[1] / len;
                    } else {
                        pos[i] = [x0 + rng.next_double() * (x1 - x0), z0 + rng.next_double() * (z1 - z0)];
                    }
                    collisions = true;
                }
                let before = pos[i];
                pos[i][0] = pos[i][0].clamp(x0, x1);
                pos[i][1] = pos[i][1].clamp(z0, z1);
                if pos[i] != before {
                    collisions = true;
                }
            }
            if !collisions {
                for q in pos.iter_mut() {
                    if !self.spread_safe(*q, max_height) {
                        *q = [x0 + rng.next_double() * (x1 - x0), z0 + rng.next_double() * (z1 - z0)];
                        collisions = true;
                    }
                }
            }
            it += 1;
        }
        if min_dist == f64::from(f32::MAX) {
            min_dist = 0.0;
        }
        let c = [text::jf(center[0] as f32), text::jf(center[1] as f32)];
        if it >= 10000 {
            let kind = if teams { "teams" } else { "entities" };
            return Err(Fail(tr!(
                &format!("commands.spreadplayers.failed.{kind}"),
                pos.len(),
                c[0].clone(),
                c[1].clone(),
                format!("{min_dist:.2}")
            )));
        }
        let mut avg = 0.0;
        let mut next = 0;
        let mut team_pos: Vec<(Option<String>, usize)> = Vec::new();
        for e in &targets {
            let idx = if teams {
                let t = if *e == E::Player {
                    self.cmd.scores.team_of(&self.ent_score_name(*e)).map(|t| t.name.clone())
                } else {
                    None
                };
                match team_pos.iter().find(|x| x.0 == t) {
                    Some(x) => x.1,
                    None => {
                        team_pos.push((t, next));
                        next += 1;
                        next - 1
                    }
                }
            } else {
                next += 1;
                next - 1
            };
            let q = pos[idx];
            let y = self.spread_y(q, max_height);
            let rot = self.ent_rot(*e);
            if *e == E::Player {
                self.player_remove_vehicle(); // No longer a passenger
            }
            self.ent_teleport(*e, [q[0].floor() + 0.5, f64::from(y), q[1].floor() + 0.5], Some(rot));
            let closest =
                pos.iter().enumerate().filter(|(j, _)| *j != idx).map(|(_, o)| dist(q, *o)).fold(f64::MAX, f64::min);
            avg += closest;
        }
        let avg = if targets.len() < 2 { 0.0 } else { avg / targets.len() as f64 };
        let kind = if teams { "teams" } else { "entities" };
        self.ok(
            s,
            tr!(
                &format!("commands.spreadplayers.success.{kind}"),
                pos.len(),
                c[0].clone(),
                c[1].clone(),
                format!("{avg:.2}")
            ),
        );
        Ok(pos.len() as i32)
    }

    /// Arrival height: first full block under two cells of air.
    fn spread_y(&mut self, q: [f64; 2], max_height: i32) -> i32 {
        let (x, z) = (q[0].floor() as i32, q[1].floor() as i32);
        self.ensure_loaded(&[(x >> 4, z >> 4)]);
        let air = |sv: &Server, y: i32| sv.states.is_air(sv.get([x, y, z]));
        let mut y = max_height + 1;
        let mut air2 = air(self, y);
        y -= 1;
        let mut air1 = air(self, y);
        while y > self.dim.height.min_y {
            y -= 1;
            let cur = air(self, y);
            if !cur && air1 && air2 {
                return y + 1;
            }
            air2 = air1;
            air1 = cur;
        }
        max_height + 1
    }

    fn spread_safe(&mut self, q: [f64; 2], max_height: i32) -> bool {
        let y = self.spread_y(q, max_height) - 1;
        let p = [q[0].floor() as i32, y, q[1].floor() as i32];
        let st = self.get(p);
        let name = self.data.block_of(st).map_or("", |b| b.name.as_str()).to_owned();
        y < max_height
            && self.fluid_at(p).kind == 0
            && self.p_tag("minecraft:block", "minecraft:entities_can_teleport_to", &name)
    }

    // ---- fillbiome --------------------------------------------------------------------------

    fn cmd_fillbiome(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let a = self.loaded_pos(s, &p.arg(seg, "from").unwrap_or_default())?;
        let b = self.loaded_pos(s, &p.arg(seg, "to").unwrap_or_default())?;
        let biome = args::id(&p.arg(seg, "biome").unwrap_or_default());
        let names = self.states.biome_names.clone();
        let Some(bid) = names.iter().position(|n| *n == biome) else {
            return Err(Fail(tr!("argument.resource.not_found", biome, "minecraft:worldgen/biome")));
        };
        let filter = p.arg(seg, "filter");
        let q = |v: i32| (v >> 2) << 2;
        let (lo, hi) = (
            [q(a[0]).min(q(b[0])), q(a[1]).min(q(b[1])), q(a[2]).min(q(b[2]))],
            [q(a[0]).max(q(b[0])), q(a[1]).max(q(b[1])), q(a[2]).max(q(b[2]))],
        );
        let volume = i64::from(hi[0] - lo[0] + 1) * i64::from(hi[1] - lo[1] + 1) * i64::from(hi[2] - lo[2] + 1);
        let limit = self.rules.get("minecraft:max_block_modifications").and_then(Nbt::as_i64).unwrap_or(32768);
        if volume > limit {
            return Err(Fail(tr!("commands.fillbiome.toobig", limit, volume)));
        }
        for cz in lo[2] >> 4..=hi[2] >> 4 {
            for cx in lo[0] >> 4..=hi[0] >> 4 {
                if !matches!(self.chunks.get(&(cx, cz)), Some(Some(_))) {
                    return Err(Fail(tr!("argument.pos.unloaded")));
                }
            }
        }
        let direct = (usize::BITS - names.len().saturating_sub(1).leading_zeros()) as u8;
        let tags = &self.regs.tags;
        let in_filter = |cur: usize| -> bool {
            let Some(f) = &filter else { return true };
            let cur_name = names.get(cur).map_or("", String::as_str);
            match f.strip_prefix('#') {
                Some(t) => {
                    let t = args::id(t);
                    tags.iter()
                        .find(|(r, _)| r == "minecraft:worldgen/biome")
                        .and_then(|(_, l)| l.iter().find(|(n, _)| *n == t))
                        .is_some_and(|(_, ids)| ids.contains(&(cur as i32)))
                }
                None => args::id(f) == cur_name,
            }
        };
        let min_y = self.dim.height.min_y;
        let mut changed = 0;
        let mut touched = Vec::new();
        for cz in lo[2] >> 4..=hi[2] >> 4 {
            for cx in lo[0] >> 4..=hi[0] >> 4 {
                let Some(Some(c)) = self.chunks.get_mut(&(cx, cz)) else { continue };
                let before = changed;
                for (si, sec) in c.sections.iter_mut().enumerate() {
                    for i in 0..64 {
                        let (qx, qz, qy) = (i & 3, (i >> 2) & 3, i >> 4);
                        let bx = cx * 16 + (qx as i32) * 4;
                        let bz = cz * 16 + (qz as i32) * 4;
                        let by = min_y + si as i32 * 16 + (qy as i32) * 4;
                        if !(lo[0]..=hi[0]).contains(&bx)
                            || !(lo[1]..=hi[1]).contains(&by)
                            || !(lo[2]..=hi[2]).contains(&bz)
                        {
                            continue;
                        }
                        let cur = sec.biomes.get(i) as usize;
                        if !in_filter(cur) || cur == bid {
                            continue;
                        }
                        sec.biomes.set(i, bid as u32, 1, 3, direct);
                        changed += 1;
                    }
                }
                if changed != before {
                    c.dirty = true;
                    for x in c.changed.iter_mut() {
                        *x = true;
                    }
                    touched.push((cx, cz));
                }
            }
        }
        if changed == 0 {
            return Err(Fail(tr!("commands.fillbiome.no_changes")));
        }
        for p2 in touched {
            if self.sent.contains(&p2)
                && let Some(Some(c)) = self.chunks.get(&p2)
            {
                let packet = self.chunk_packet(c);
                let _ = self.send(Event::Chunk(Box::new(packet)));
            }
        }
        self.ok(s, tr!("commands.fillbiome.success.count", changed, lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]));
        Ok(changed)
    }

    // ---- datapack ---------------------------------------------------------------------------

    /// Available packs (`packs.rs`): built-in ones read at opening, world folder re-read.
    fn datapack_repo(&self) -> Vec<crate::packs::Pack> {
        crate::packs::with_world(&self.cmd.builtin_packs, &self.world)
    }

    /// Chosen packs, in order (level.dat, rewritten at opening and at each reload).
    fn datapack_enabled(&self) -> Vec<String> {
        crate::packs::config_of(self.level.root.get("Data")).0
    }

    fn pack_link(&self, p: &crate::packs::Pack, enabled: bool) -> Component {
        let mut name = tr!("pack.nameAndSource", lit(p.id.clone()), tr!(p.source.key()));
        name.style.color = Some(text::GRAY);
        let mut c = text::brackets(name);
        c.style.color = Some(if enabled { text::GREEN } else { text::RED });
        c.style.insertion = Some(std::sync::Arc::from(escape_if_required(&p.id).as_str()));
        let mut tip = lit("");
        tip.extra = vec![p.title(), lit("\n"), p.description_text()];
        text::hover_text(&mut c, tip);
        c
    }

    fn cmd_datapack(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let repo = self.datapack_repo();
        let mut enabled = self.datapack_enabled();
        let features = self.features.clone();
        let feat_ok = |e: &crate::packs::Pack| e.features.iter().all(|f| features.contains(f));
        match w.get(1).copied() {
            Some("list") => {
                let which = w.get(2).copied();
                let mut total = 0;
                if which != Some("available") {
                    let sel: Vec<Component> = enabled
                        .iter()
                        .filter_map(|id| repo.iter().find(|r| r.id == *id))
                        .map(|r| self.pack_link(r, true))
                        .collect();
                    if sel.is_empty() {
                        self.ok(s, tr!("commands.datapack.list.enabled.none"));
                    } else {
                        let n = sel.len();
                        self.ok(s, tr!("commands.datapack.list.enabled.success", n, text::list(sel)));
                        total += n as i32;
                    }
                }
                if which != Some("enabled") {
                    let av: Vec<Component> = repo
                        .iter()
                        .filter(|r| !enabled.contains(&r.id) && feat_ok(r))
                        .map(|r| self.pack_link(r, false))
                        .collect();
                    if av.is_empty() {
                        self.ok(s, tr!("commands.datapack.list.available.none"));
                    } else {
                        let n = av.len();
                        self.ok(s, tr!("commands.datapack.list.available.success", n, text::list(av)));
                        total += n as i32;
                    }
                }
                Ok(total)
            }
            Some("enable") | Some("disable") => {
                let en = w[1] == "enable";
                let id = p.arg(seg, "name").unwrap_or_default();
                let id = id.trim_matches('"').to_owned();
                let Some(entry) = repo.iter().find(|r| r.id == id).cloned() else {
                    return Err(Fail(tr!("commands.datapack.unknown", id)));
                };
                let is_on = enabled.contains(&id);
                if en && is_on {
                    return Err(Fail(tr!("commands.datapack.enable.failed", id)));
                }
                if !en && !is_on {
                    return Err(Fail(tr!("commands.datapack.disable.failed", id)));
                }
                if !en && !entry.features.is_empty() && entry.source == crate::packs::Source::Feature {
                    return Err(Fail(tr!("commands.datapack.disable.failed.feature", id)));
                }
                if !feat_ok(&entry) {
                    return Err(Fail(tr!("commands.datapack.enable.failed.no_flags", id, entry.features.join(", "))));
                }
                if en {
                    match (w.get(3).copied(), p.arg(seg, "existing")) {
                        (Some("first"), _) => enabled.insert(0, id.clone()),
                        (Some("before"), Some(x)) | (Some("after"), Some(x)) => {
                            let x = x.trim_matches('"').to_owned();
                            let Some(i) = enabled.iter().position(|e| *e == x) else {
                                return Err(Fail(tr!("commands.datapack.unknown", x)));
                            };
                            let at = if w[3] == "before" { i } else { i + 1 };
                            enabled.insert(at, id.clone());
                        }
                        _ => enabled.push(id.clone()),
                    }
                    self.ok(s, tr!("commands.datapack.modify.enable", self.pack_link(&entry, true)));
                } else {
                    enabled.retain(|e| *e != id);
                    self.ok(s, tr!("commands.datapack.modify.disable", self.pack_link(&entry, true)));
                }
                self.reload_packs(&repo, &enabled);
                Ok(enabled.len() as i32)
            }
            _ => {
                let id = p.arg(seg, "id").unwrap_or_default();
                let id = id.trim_matches('"').to_owned();
                let valid = !id.is_empty()
                    && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.".contains(c))
                    && id != "."
                    && id != "..";
                if !valid {
                    return Err(Fail(tr!("commands.datapack.create.invalid_name", id)));
                }
                let full = format!("file/{id}");
                if repo.iter().any(|r| r.id == full) || self.world.join("datapacks").join(&id).exists() {
                    return Err(Fail(tr!("commands.datapack.create.already_exists", id)));
                }
                let desc = p.arg(seg, "description").unwrap_or_default();
                let dir = self.world.join("datapacks").join(&id);
                let desc_json = match super::snbt::parse(&desc) {
                    Ok(Nbt::String(t)) => serde_json::Value::String(t),
                    _ => serde_json::Value::String(desc.trim_matches('"').to_owned()),
                };
                let meta = serde_json::json!({
                    "pack": { "description": desc_json, "min_format": crate::progress::DATA_PACK_FORMAT, "max_format": crate::progress::DATA_PACK_FORMAT }
                });
                let text = serde_json::to_string_pretty(&meta).unwrap_or_default();
                let ok = std::fs::create_dir_all(dir.join("data")).is_ok()
                    && std::fs::write(dir.join("pack.mcmeta"), text).is_ok();
                if !ok {
                    return Err(Fail(tr!("commands.datapack.create.io_failure", id)));
                }
                self.ok(s, tr!("commands.datapack.create.success", id));
                Ok(1)
            }
        }
    }

    /// `/reload`: chosen packs, plus the packs that appeared since and are not disabled.
    pub(crate) fn s9_reload(&mut self) {
        let repo = self.datapack_repo();
        let (mut enabled, disabled, _) = crate::packs::config_of(self.level.root.get("Data"));
        for p in &repo {
            if !disabled.contains(&p.id) && !enabled.contains(&p.id) {
                enabled.push(p.id.clone());
            }
        }
        self.reload_packs(&repo, &enabled);
    }

    /// Data reload with the `enabled` packs (in order): selection rewritten
    /// in level.dat, stack rebuilt, then tags (sent back to the client), functions and advancements.
    fn reload_packs(&mut self, repo: &[crate::packs::Pack], enabled: &[String]) {
        let sel = crate::packs::explicit(repo, enabled, &self.features);
        if let Some(d) = crate::nbtio::get_mut(&mut self.level.root, "Data") {
            crate::packs::write_config(d, &sel);
        }
        self.wgs = crate::dims::GenSettings::read(&self.world);
        let Some(mut jar) =
            crate::registries::client_jar(&self.data).and_then(|j| crate::registries::Jar::read(&j).ok())
        else {
            eprintln!("singleplayer: reload impossible, game jar unreadable");
            return;
        };
        crate::packs::stack(&mut jar, repo, &sel);
        // Registries frozen at opening, like the game: only the tags are rebuilt.
        self.set_registry_tags(crate::registries::tags_for(&jar, &self.data, &self.regs.synced));
        self.cmd.jar_tags = super::registry_tags(&self.data, &self.regs);
        for (reg, map) in &self.cmd.jar_tags {
            self.cmd.data.set_tags(reg, map.clone());
        }
        super::exec::apply_pack_tags(&jar, &mut self.cmd.data, &self.cmd.jar_tags);
        let (functions, tags) = super::exec::load_functions(&jar, &self.cmd.levels, &self.cmd.data);
        self.cmd.functions = functions;
        self.cmd.function_tags = tags;
        self.cmd.post_reload = true;
        self.prog.reload(crate::progress::Tree::shared(&jar, sel.vanilla_only()));
    }
}

/// Team color.
fn team_color_rgb(name: &str) -> i32 {
    rc_text::parse_color(name).map_or(0xFF_FFFF, |c| c as i32)
}

/// Quotes if necessary.
fn escape_if_required(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-.+".contains(c));
    if plain { s.to_owned() } else { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) }
}
