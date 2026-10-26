//! `recipe give|take` (the player's recipe book) and `bossbar` (custom boss bars).

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::{BossEvent, BossOp};
use rc_text::Component;

use super::cmds_basic::Track;
use super::ents::E;
use super::src::{Fail, Src};
use super::text::{self, lit};
use super::{Parsed, R, Seg, args};
use crate::server::Server;
use crate::tr;

/// Custom boss bar (`custom_boss_events.dat`).
#[derive(Debug, Clone)]
pub struct Bar {
    pub id: String,
    pub uuid: u128,
    pub name: Nbt,
    pub color: String,
    pub style: String,
    pub value: i32,
    pub max: i32,
    pub visible: bool,
    /// Registered players (UUID, connected or not), in the order they were added.
    pub players: Vec<u128>,
    pub darken: bool,
    pub music: bool,
    pub fog: bool,
}

const BAR_COLORS: [&str; 7] = ["pink", "blue", "red", "green", "yellow", "purple", "white"];
const BAR_STYLES: [&str; 5] = ["progress", "notched_6", "notched_10", "notched_12", "notched_20"];

impl Bar {
    /// Name in brackets; color (formatting of the bar color), hover and insertion
    /// set on the brackets: the name keeps its own color.
    fn display(&self) -> Component {
        let c = rc_text::from_nbt(&self.name).unwrap_or_else(|| lit(self.id.clone()));
        let mut b = text::brackets(c);
        let format = match self.color.as_str() {
            "pink" => "red",
            "blue" => "blue",
            "red" => "dark_red",
            "green" => "green",
            "yellow" => "yellow",
            "purple" => "dark_blue",
            _ => "white",
        };
        b.style.color = rc_text::parse_color(format);
        text::hover_text(&mut b, lit(self.id.clone()));
        b.style.insertion = Some(std::sync::Arc::from(self.id.as_str()));
        b
    }

    /// Shown to this player: visible and player registered.
    pub fn shown_to(&self, player: u128) -> bool {
        self.visible && self.players.contains(&player)
    }

    /// Packet flags: darken 1, music 2, fog 4.
    fn flags(&self) -> u8 {
        u8::from(self.darken) | u8::from(self.music) << 1 | u8::from(self.fog) << 2
    }

    fn progress(&self) -> f32 {
        if self.max <= 0 { 0.0 } else { (self.value as f32 / self.max as f32).clamp(0.0, 1.0) }
    }
}

/// Bars read from `data/minecraft/custom_boss_events.dat`.
pub fn read_bars(world: &std::path::Path) -> Vec<Bar> {
    let p = world.join("data").join("minecraft").join("custom_boss_events.dat");
    let Some(Nbt::Compound(f)) = crate::nbtio::read_dat(&p).ok().and_then(|r| r.get("data").cloned()) else {
        return Vec::new();
    };
    let mut rng = crate::rng::Rng::from_time();
    f.iter()
        .map(|(id, v)| {
            let b = |k: &str, d: bool| crate::nbtio::int(v, k).map_or(d, |x| x != 0);
            let s = |k: &str, d: &str| v.get(k).and_then(Nbt::as_str).unwrap_or(d).to_owned();
            let mut players: Vec<u128> = Vec::new();
            for u in crate::nbtio::list(v.get("Players")).iter().filter_map(|u| crate::nbtio::uuid(Some(u))) {
                if !players.contains(&u) {
                    players.push(u); // set of UUIDs
                }
            }
            Bar {
                id: id.clone(),
                uuid: (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128,
                name: v.get("Name").cloned().unwrap_or_else(|| Nbt::String(id.clone())),
                color: s("Color", "white"),
                style: s("Overlay", "progress"),
                value: crate::nbtio::int(v, "Value").unwrap_or(0) as i32,
                max: crate::nbtio::int(v, "Max").unwrap_or(100) as i32,
                visible: b("Visible", false),
                players,
                darken: b("DarkenScreen", false),
                music: b("PlayBossMusic", false),
                fog: b("CreateWorldFog", false),
            }
        })
        .collect()
}

/// Compound of a bar: like the game's codec, a field equal to its default value is omitted.
fn bar_nbt(b: &Bar) -> Nbt {
    let mut f = vec![("Name".into(), b.name.clone())];
    let mut opt = |k: &str, v: Nbt, default: bool| {
        if !default {
            f.push((k.into(), v));
        }
    };
    opt("Visible", Nbt::Byte(1), !b.visible);
    opt("Value", Nbt::Int(b.value), b.value == 0);
    opt("Max", Nbt::Int(b.max), b.max == 100);
    opt("Color", Nbt::String(b.color.clone()), b.color == "white");
    opt("Overlay", Nbt::String(b.style.clone()), b.style == "progress");
    opt("DarkenScreen", Nbt::Byte(1), !b.darken);
    opt("PlayBossMusic", Nbt::Byte(1), !b.music);
    opt("CreateWorldFog", Nbt::Byte(1), !b.fog);
    opt(
        "Players",
        Nbt::List(b.players.iter().map(|u| crate::nbtio::uuid_nbt(*u)).collect()),
        b.players.is_empty(),
    );
    Nbt::Compound(f)
}

/// `custom_boss_events.dat` to write.
pub(crate) fn bars_job(world: &std::path::Path, bars: &[Bar]) -> crate::save::Job {
    let data = bars.iter().map(|b| (b.id.clone(), bar_nbt(b))).collect();
    let root = Nbt::Compound(vec![
        ("data".into(), Nbt::Compound(data)),
        ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
    ]);
    crate::save::Job::Dat {
        path: world.join("data").join("minecraft").join("custom_boss_events.dat"),
        nbt: crate::nbtio::encode_root(&root),
        backup: false,
    }
}

/// Special recipe type (never added to the book).
fn special(v: &serde_json::Value) -> bool {
    let t = v.get("type").and_then(serde_json::Value::as_str).unwrap_or("");
    t.starts_with("minecraft:crafting_special_") || t == "minecraft:crafting_decorated_pot"
}

impl Server {
    pub(crate) fn cmd_recipe(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let give = seg.path.get(1).map(String::as_str) == Some("give");
        let targets = self.arg_players(s, &p.arg(seg, "targets").unwrap_or_default())?;
        let which = p.arg(seg, "recipe");
        let all: Vec<String> = match &which {
            Some(r) => {
                let id = args::id(r);
                if self.life.pack.get("recipe", &id).is_none() {
                    return Err(Fail(tr!("recipe.notFound", id)));
                }
                vec![id]
            }
            None => self.cmd.server_regs.get("minecraft:recipe").cloned().unwrap_or_default(),
        };
        let mut t: Track<Component> = Track::default();
        for e in targets {
            if e != E::Player {
                continue;
            }
            let n = if give {
                let new: Vec<String> = all
                    .iter()
                    .filter(|id| !self.life.known.contains(*id))
                    .filter(|id| self.life.pack.get("recipe", id).is_some_and(|v| !special(&v)))
                    .cloned()
                    .collect();
                self.award_recipes(&new);
                new.len() as i32
            } else {
                let gone: Vec<String> = all.iter().filter(|id| self.life.known.contains(*id)).cloned().collect();
                let mut idx = Vec::new();
                for id in &gone {
                    self.life.known.remove(id);
                    self.life.highlight.remove(id);
                    if let Some(i) = self.life.recipes.index(id) {
                        idx.extend(self.life.recipes.displays(i));
                    }
                }
                if !idx.is_empty() {
                    let _ = self.send(Event::Recipes(rc_client::recipes::RecipeEvent::Remove(idx)));
                }
                gone.len() as i32
            };
            t.track(self.ent_display(e), n);
        }
        let k = if give { "give" } else { "take" };
        let total = t.total;
        self.track_feedback(
            s,
            &t,
            false,
            Some(Fail(tr!(&format!("commands.recipe.{k}.failed")))),
            |_, n| tr!(&format!("commands.recipe.{k}.success.single"), total, n.clone()),
            |c| tr!(&format!("commands.recipe.{k}.success.multiple"), total, c),
        )
    }

    pub(crate) fn bar(&self, id: &str) -> Result<Bar, Fail> {
        self.cmd.bars.iter().find(|b| b.id == id).cloned().ok_or_else(|| Fail(tr!("commands.bossbar.unknown", id)))
    }

    fn bar_shown(&self, b: &Bar) -> bool {
        b.shown_to(self.uuid)
    }

    fn bar_send(&self, b: &Bar, op: BossOp) {
        if self.bar_shown(b) {
            let _ = self.send(Event::Boss(BossEvent { id: b.uuid, op }));
        }
    }

    fn bar_add_packet(&self, b: &Bar) -> BossOp {
        BossOp::Add {
            name: Some(b.name.clone()),
            progress: b.progress(),
            color: BAR_COLORS.iter().position(|c| *c == b.color).unwrap_or(6) as i32,
            style: BAR_STYLES.iter().position(|c| *c == b.style).unwrap_or(0) as i32,
            flags: b.flags(),
        }
    }

    fn bar_update(&mut self, b: Bar) {
        if let Some(x) = self.cmd.bars.iter_mut().find(|x| x.id == b.id) {
            *x = b;
        }
    }

    /// Bars visible on joining the game.
    pub(crate) fn send_bossbars(&self) {
        for b in &self.cmd.bars {
            if self.bar_shown(b) {
                let _ = self.send(Event::Boss(BossEvent { id: b.uuid, op: self.bar_add_packet(b) }));
            }
        }
    }

    pub(crate) fn cmd_bossbar(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let id = args::id(&p.arg(seg, "id").unwrap_or_default());
        self.cmd.bars_dirty |= w.get(1).copied() != Some("list") && w.get(1).copied() != Some("get");
        match w.get(1).copied() {
            Some("list") => {
                let names: Vec<Component> = self.cmd.bars.iter().map(Bar::display).collect();
                if names.is_empty() {
                    self.ok(s, tr!("commands.bossbar.list.bars.none"));
                } else {
                    self.ok(s, tr!("commands.bossbar.list.bars.some", names.len(), text::list(names)));
                }
                Ok(self.cmd.bars.len() as i32)
            }
            Some("add") => {
                if self.cmd.bars.iter().any(|b| b.id == id) {
                    return Err(Fail(tr!("commands.bossbar.create.failed", id)));
                }
                let c = self.arg_component(s, &p.arg(seg, "name").unwrap_or_default(), None)?;
                let uuid = (self.life.rng.next_long() as u128) << 64 | self.life.rng.next_long() as u64 as u128;
                let b = Bar {
                    id,
                    uuid,
                    name: text::to_nbt(&c),
                    color: "white".into(),
                    style: "progress".into(),
                    value: 0,
                    max: 100,
                    visible: true,
                    players: Vec::new(),
                    darken: false,
                    music: false,
                    fog: false,
                };
                let d = b.display();
                self.cmd.bars.push(b);
                self.ok(s, tr!("commands.bossbar.create.success", d));
                Ok(self.cmd.bars.len() as i32)
            }
            Some("remove") => {
                let b = self.bar(&id)?;
                self.bar_send(&b, BossOp::Remove);
                self.cmd.bars.retain(|x| x.id != id);
                self.ok(s, tr!("commands.bossbar.remove.success", b.display()));
                Ok(self.cmd.bars.len() as i32)
            }
            Some("get") => {
                let b = self.bar(&id)?;
                let (k, v) = match w.get(3).copied() {
                    Some("max") => ("commands.bossbar.get.max", b.max),
                    Some("visible") => {
                        let k = if b.visible {
                            "commands.bossbar.get.visible.visible"
                        } else {
                            "commands.bossbar.get.visible.hidden"
                        };
                        self.ok(s, tr!(k, b.display()));
                        return Ok(i32::from(b.visible));
                    }
                    Some("players") => {
                        // Connected players who see it (the only simulated player: the host).
                        if b.players.contains(&self.uuid) {
                            let names = self.player_display_name();
                            self.ok(s, tr!("commands.bossbar.get.players.some", b.display(), 1, names));
                            return Ok(1);
                        }
                        self.ok(s, tr!("commands.bossbar.get.players.none", b.display()));
                        return Ok(0);
                    }
                    _ => ("commands.bossbar.get.value", b.value),
                };
                self.ok(s, tr!(k, b.display(), v));
                Ok(v)
            }
            Some("set") => {
                let mut b = self.bar(&id)?;
                let what = w.get(3).copied().unwrap_or("");
                let r = match what {
                    "name" => {
                        let c = self.arg_component(s, &p.arg(seg, "name").unwrap_or_default(), None)?;
                        let n = text::to_nbt(&c);
                        if crate::items::nbt_eq(&n, &b.name) {
                            return Err(Fail(tr!("commands.bossbar.set.name.unchanged")));
                        }
                        b.name = n.clone();
                        self.bar_send(&b, BossOp::Name(Some(n)));
                        self.ok(s, tr!("commands.bossbar.set.name.success", b.display()));
                        0
                    }
                    "color" | "style" => {
                        let v = w.get(4).copied().unwrap_or("").to_owned();
                        let cur = if what == "color" { &mut b.color } else { &mut b.style };
                        if *cur == v {
                            return Err(Fail(tr!(&format!("commands.bossbar.set.{what}.unchanged"))));
                        }
                        *cur = v;
                        let color = BAR_COLORS.iter().position(|c| *c == b.color).unwrap_or(6) as i32;
                        let style = BAR_STYLES.iter().position(|c| *c == b.style).unwrap_or(0) as i32;
                        self.bar_send(&b, BossOp::Style { color, style });
                        self.ok(s, tr!(&format!("commands.bossbar.set.{what}.success"), b.display()));
                        0
                    }
                    "value" | "max" => {
                        let v: i32 = p.arg(seg, what).and_then(|x| x.parse().ok()).unwrap_or(0);
                        let cur = if what == "value" { b.value } else { b.max };
                        if cur == v {
                            return Err(Fail(tr!(&format!("commands.bossbar.set.{what}.unchanged"))));
                        }
                        if what == "value" {
                            b.value = v;
                        } else {
                            b.max = v;
                        }
                        self.bar_send(&b, BossOp::Progress(b.progress()));
                        self.ok(s, tr!(&format!("commands.bossbar.set.{what}.success"), b.display(), v));
                        v
                    }
                    "visible" => {
                        let v = p.arg(seg, "visible").as_deref() == Some("true");
                        if b.visible == v {
                            let k = if v { "visible" } else { "hidden" };
                            return Err(Fail(tr!(&format!("commands.bossbar.set.visibility.unchanged.{k}"))));
                        }
                        let was = self.bar_shown(&b);
                        b.visible = v;
                        self.bar_toggle(&b, was);
                        let k = if v { "visible" } else { "hidden" };
                        self.ok(s, tr!(&format!("commands.bossbar.set.visible.success.{k}"), b.display()));
                        0
                    }
                    _ => {
                        // players [targets]: the list becomes exactly these players (those registered
                        // who are absent from the list, even offline, are removed).
                        let targets: Vec<u128> = match p.arg(seg, "targets") {
                            Some(t) => self
                                .arg_entities_opt(s, &t)
                                .into_iter()
                                .filter(|e| *e == E::Player)
                                .map(|_| self.uuid)
                                .collect(),
                            None => Vec::new(),
                        };
                        let mut list: Vec<u128> = b.players.iter().copied().filter(|u| targets.contains(u)).collect();
                        for u in &targets {
                            if !list.contains(u) {
                                list.push(*u);
                            }
                        }
                        let same = list.len() == b.players.len() && list.iter().all(|u| b.players.contains(u));
                        if same {
                            return Err(Fail(tr!("commands.bossbar.set.players.unchanged")));
                        }
                        let was = self.bar_shown(&b);
                        b.players = list;
                        self.bar_toggle(&b, was);
                        let online = i32::from(b.players.contains(&self.uuid));
                        if online == 0 {
                            self.ok(s, tr!("commands.bossbar.set.players.success.none", b.display()));
                        } else {
                            let names = self.player_display_name();
                            self.ok(
                                s,
                                tr!("commands.bossbar.set.players.success.some", b.display(), targets.len(), names),
                            );
                        }
                        online
                    }
                };
                self.bar_update(b);
                Ok(r)
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    fn bar_toggle(&self, b: &Bar, was: bool) {
        let now = self.bar_shown(b);
        if now && !was {
            let _ = self.send(Event::Boss(BossEvent { id: b.uuid, op: self.bar_add_packet(b) }));
        } else if was && !now {
            let _ = self.send(Event::Boss(BossEvent { id: b.uuid, op: BossOp::Remove }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bar, bar_nbt};
    use rc_proto::nbt::Nbt;
    use rc_text::{Arg, Content};

    fn bar() -> Bar {
        Bar {
            id: "minecraft:b".into(),
            uuid: 7,
            name: Nbt::Compound(vec![
                ("text".into(), Nbt::String("Boss".into())),
                ("color".into(), Nbt::String("aqua".into())),
            ]),
            color: "red".into(),
            style: "progress".into(),
            value: 0,
            max: 100,
            visible: true,
            players: vec![1, 2],
            darken: true,
            music: false,
            fog: true,
        }
    }

    #[test]
    fn display_styles_the_brackets() {
        let d = bar().display();
        // Formatting color of the bar (red → dark red), hover and insertion on the
        // brackets; the name keeps its color.
        assert_eq!(d.style.color, rc_text::parse_color("dark_red"));
        assert!(d.style.hover.is_some());
        assert_eq!(d.style.insertion.as_deref(), Some("minecraft:b"));
        let Content::Translate { key, with, .. } = &d.content else { panic!("{d:?}") };
        assert_eq!(key, "chat.square_brackets");
        let Some(Arg::Component(inner)) = with.first() else { panic!("{with:?}") };
        assert_eq!(inner.style.color, rc_text::parse_color("aqua"));
        assert!(inner.style.hover.is_none());
    }

    #[test]
    fn players_and_flags_saved() {
        let b = bar();
        assert_eq!(b.flags(), 1 | 4);
        assert!(b.shown_to(2) && !b.shown_to(3));
        let n = bar_nbt(&b);
        assert!(matches!(n.get("Players"), Some(Nbt::List(l)) if l.len() == 2));
        assert_eq!(crate::nbtio::int(&n, "DarkenScreen"), Some(1));
        assert_eq!(crate::nbtio::int(&n, "CreateWorldFog"), Some(1));
        // Default values omitted (the game's codec).
        for k in ["PlayBossMusic", "Value", "Max", "Overlay"] {
            assert!(n.get(k).is_none(), "{k}");
        }
    }

    #[test]
    fn bars_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("rc-bars-{}", std::process::id()));
        let crate::save::Job::Dat { path, nbt, .. } = super::bars_job(&dir, &[bar()]) else { panic!() };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, crate::nbtio::gzip(&nbt)).unwrap();
        let back = super::read_bars(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let b = &back[0];
        assert_eq!(b.players, vec![1, 2]);
        assert_eq!((b.darken, b.music, b.fog, b.visible), (true, false, true, true));
        assert_eq!(b.color, "red");
    }
}
