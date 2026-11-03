//! Scoreboard and teams on the client (`docs/specs/scoreboard-teams.md`): model, sidebar,
//! formatting of names by team, score in the Tab list.

use std::sync::OnceLock;

use hashbrown::HashMap;
use rc_commands::suggest::cmp_ignore_case;

use rc_proto::nbt::Nbt;
use rc_proto_v777::scores::{NumberFormat, ScoreEvent, TeamParams};
use rc_text::{Component, StyledChar};

use crate::gui::{Gui, TextContext};
use crate::players::Revision;

const COLOR_NAMES: [&str; 16] = [
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

pub struct Objective {
    pub display: Option<Nbt>,
    pub hearts: bool,
    pub format: Option<NumberFormat>,
}

pub struct Score {
    pub value: i32,
    pub display: Option<Nbt>,
    pub format: Option<NumberFormat>,
}

pub struct Team {
    pub params: TeamParams,
}

#[derive(Default)]
pub struct Scoreboard {
    pub objectives: HashMap<String, Objective>,
    /// (holder, objective) → score.
    pub scores: HashMap<(String, String), Score>,
    /// 19 slots: 0 list, 1 sidebar, 2 below name, 3 + color.
    pub slots: [Option<String>; 19],
    pub teams: HashMap<String, Team>,
    /// Holder → team.
    pub members: HashMap<String, String>,
    /// Changes on every modification of the teams (formatted names to redo).
    pub version: u64,
    /// Changes on every packet that can modify the sidebar (objectives, slots, scores
    /// of an objective shown in the sidebar, teams).
    revision: u64,
    /// Changes on every packet that can modify the player list: scores of the objective in the
    /// list slot, that slot, objectives, teams (formatted names and sort).
    list_revision: Revision,
    /// Sidebar computed for (revision, local player).
    sidebar_cache: Option<SidebarCache>,
    /// Sidebars styled so far (the proofs check that the styled lines are really kept).
    #[cfg(test)]
    styled_builds: usize,
}

/// Borrowed key (holder, objective) of the score table: its derived hash is the one of the owned
/// `(String, String)` key (the two strings hashed in order), so a lookup copies neither name.
#[derive(Hash)]
struct ScoreKey<'a>(&'a str, &'a str);

impl hashbrown::Equivalent<(String, String)> for ScoreKey<'_> {
    fn equivalent(&self, k: &(String, String)) -> bool {
        k.0 == self.0 && k.1 == self.1
    }
}

/// Scores looked up with a borrowed key (default); `RUSTCRAFT_OPT_SCORE_KEY_BORROW=0`: both
/// names copied into an owned key at each lookup (old path). Read once.
fn score_key_borrow_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| rc_data::env_os!("RUSTCRAFT_OPT_SCORE_KEY_BORROW").is_none_or(|v| v != "0"))
}

/// Sidebar styled and measured once per rows and text context (default);
/// `RUSTCRAFT_OPT_SIDEBAR_STYLED_CACHE=0`: flattened and measured on every frame (old path). Only
/// acts with the kept rows (`RUSTCRAFT_OPT_SIDEBARCACHE`). Read once.
fn sidebar_styled_cache_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| rc_data::env_os!("RUSTCRAFT_OPT_SIDEBAR_STYLED_CACHE").is_none_or(|v| v != "0"))
}

/// Title and lines (formatted name, value) of the sidebar, in display order.
#[derive(Debug, Clone, PartialEq)]
struct SidebarRows {
    title: Option<Component>,
    rows: Vec<(Component, Option<Component>)>,
    /// Holders of the lines, for the equivalence proof up to ties.
    #[cfg(test)]
    holders: Vec<String>,
}

struct SidebarCache {
    revision: u64,
    local: String,
    rows: Option<SidebarRows>,
    /// The rows styled and measured, for the text context they were styled with.
    styled: Option<(TextContext, StyledSidebar)>,
}

/// One line of the sidebar ready to draw.
struct StyledLine {
    name: Vec<StyledChar>,
    score: Vec<StyledChar>,
    score_width: i32,
}

/// Sidebar ready to draw: styled title and lines, their widths and the width of the panel.
struct StyledSidebar {
    title: Vec<StyledChar>,
    title_width: i32,
    lines: Vec<StyledLine>,
    /// Widest of the title and of the lines (name, then ": " and the score if it has one).
    width: i32,
}

impl StyledSidebar {
    fn build(gui: &Gui, rows: &SidebarRows) -> StyledSidebar {
        let chars = |c: &Component| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
        let title = rows.title.as_ref().map(chars).unwrap_or_default();
        let title_width = rc_text::width(gui, &title);
        let separator = rc_text::width(gui, &chars(&Component::text(": ")));
        let mut width = title_width;
        let lines = rows
            .rows
            .iter()
            .map(|(name, score)| {
                let (name, score) = (chars(name), score.as_ref().map(chars).unwrap_or_default());
                let score_width = rc_text::width(gui, &score);
                let after_name = if score_width > 0 { separator + score_width } else { 0 };
                width = width.max(rc_text::width(gui, &name) + after_name);
                StyledLine { name, score, score_width }
            })
            .collect();
        StyledSidebar { title, title_width, lines, width }
    }

    /// Panel on the right of the screen, in a new stratum.
    fn draw(&self, gui: &mut Gui, queue: &wgpu::Queue, text_background: Option<u32>) {
        gui.next_layer();
        let (gw, gh) = (gui.width.floor() as i32, gui.height.floor() as i32);
        let n = self.lines.len() as i32;
        let bottom = gh / 2 + n * 9 / 3;
        let left = gw - self.width - 3;
        let right = gw - 1;
        let top = bottom - n * 9;
        // Backgrounds: 0.4 and 0.3 by default, or the opacity of the "everywhere" text background (option).
        let header = crate::hud::background_color(text_background, 0x6600_0000);
        let body = crate::hud::background_color(text_background, 0x4C00_0000);
        gui.fill((left - 2) as f32, (top - 10) as f32, right as f32, (top - 1) as f32, header);
        gui.fill((left - 2) as f32, (top - 1) as f32, right as f32, bottom as f32, body);
        let title_left = left + self.width / 2 - self.title_width / 2;
        gui.text(queue, &self.title, title_left as f32, (top - 9) as f32, 0xffff_ffff, false, 0);
        for (i, line) in self.lines.iter().enumerate() {
            let y = (bottom - (n - i as i32) * 9) as f32;
            gui.text(queue, &line.name, left as f32, y, 0xffff_ffff, false, 0);
            gui.text(queue, &line.score, (right - line.score_width) as f32, y, 0xffff_ffff, false, 0);
        }
    }
}

/// Lines at most in the sidebar.
const SIDEBAR_ROWS: usize = 15;

/// Sidebar kept from one frame to the next as long as the revision does not change (default);
/// `RUSTCRAFT_OPT_SIDEBARCACHE=0`: full recompute (walk and sort of all the scores) on
/// every frame. Read once.
fn sidebar_cache_on() -> bool {
    rc_data::env_os!("RUSTCRAFT_OPT_SIDEBARCACHE").is_none_or(|v| v != "0")
}

/// Display location (default color of the format).
#[derive(Clone, Copy)]
pub enum Place {
    Sidebar,
    List,
    BelowName,
}

fn component(n: &Option<Nbt>) -> Option<Component> {
    n.as_ref().and_then(rc_text::from_nbt)
}

impl Scoreboard {
    /// Scoreboard packet; the game's "→ disconnection" cases are ignored here.
    pub fn apply(&mut self, ev: ScoreEvent) {
        let bump = match &ev {
            ScoreEvent::Score { objective, .. } | ScoreEvent::Reset { objective: Some(objective), .. } => {
                self.on_sidebar(objective)
            }
            _ => true,
        };
        if bump {
            self.revision = self.revision.wrapping_add(1);
        }
        if self.touches_list(&ev) {
            self.list_revision.bump();
        }
        match ev {
            ScoreEvent::Objective { name, mode, display, hearts, format } => match mode {
                0 => {
                    self.objectives.entry(name).or_insert(Objective { display, hearts, format });
                }
                1 => {
                    if self.objectives.remove(&name).is_some() {
                        for s in &mut self.slots {
                            if s.as_deref() == Some(name.as_str()) {
                                *s = None;
                            }
                        }
                        self.scores.retain(|(_, o), _| *o != name);
                    }
                }
                2 => {
                    if let Some(o) = self.objectives.get_mut(&name) {
                        *o = Objective { display, hearts, format };
                    }
                }
                _ => {}
            },
            ScoreEvent::Display { slot, objective } => {
                let known = !objective.is_empty() && self.objectives.contains_key(&objective);
                self.slots[slot as usize] = known.then_some(objective);
            }
            ScoreEvent::Score { holder, objective, value, display, format } => {
                if self.objectives.contains_key(&objective) {
                    self.scores.insert((holder, objective), Score { value, display, format });
                }
            }
            ScoreEvent::Reset { holder, objective } => match objective {
                None => self.scores.retain(|(h, _), _| *h != holder),
                Some(o) => {
                    self.scores.remove(&(holder, o));
                }
            },
            ScoreEvent::Team { name, mode, params, members } => {
                self.version += 1;
                if mode == 0 {
                    self.teams.entry(name.clone()).or_insert_with(|| Team {
                        params: TeamParams {
                            display: Some(Nbt::String(name.clone())),
                            prefix: None,
                            suffix: None,
                            nametag: 0,
                            collision: 0,
                            color: None,
                            options: 0,
                        },
                    });
                } else if !self.teams.contains_key(&name) {
                    return;
                }
                if let Some(p) = params
                    && let Some(t) = self.teams.get_mut(&name)
                {
                    t.params = p;
                }
                match mode {
                    0 | 3 => {
                        for m in members {
                            self.members.insert(m, name.clone());
                        }
                    }
                    4 => {
                        for m in members {
                            if self.members.get(&m) == Some(&name) {
                                self.members.remove(&m);
                            }
                        }
                    }
                    1 => {
                        self.teams.remove(&name);
                        self.members.retain(|_, t| *t != name);
                    }
                    _ => {}
                }
            }
        }
    }

    /// The event can change what the player list shows: a score of the objective in the list slot
    /// (or all the scores of a holder while the slot is used), that slot, an objective, a team.
    fn touches_list(&self, ev: &ScoreEvent) -> bool {
        let list = self.slots[0].as_deref();
        match ev {
            ScoreEvent::Score { objective, .. } | ScoreEvent::Reset { objective: Some(objective), .. } => {
                list == Some(objective.as_str())
            }
            ScoreEvent::Reset { objective: None, .. } => list.is_some(),
            ScoreEvent::Display { slot, .. } => *slot == 0,
            ScoreEvent::Objective { .. } | ScoreEvent::Team { .. } => true,
        }
    }

    /// Revision of what the player list shows from the scoreboard.
    pub fn list_revision(&self) -> Revision {
        self.list_revision
    }

    /// Score of a holder in an objective.
    fn score(&self, holder: &str, objective: &str) -> Option<&Score> {
        if score_key_borrow_on() { self.score_borrowed(holder, objective) } else { self.score_owned(holder, objective) }
    }

    fn score_borrowed(&self, holder: &str, objective: &str) -> Option<&Score> {
        self.scores.get(&ScoreKey(holder, objective))
    }

    /// Old path: two names copied for each lookup.
    fn score_owned(&self, holder: &str, objective: &str) -> Option<&Score> {
        self.scores.get(&(holder.to_owned(), objective.to_owned()))
    }

    /// Team of a holder, with its name.
    pub fn team_named(&self, holder: &str) -> Option<(&str, &Team)> {
        let n = self.members.get(holder)?;
        Some((n.as_str(), self.teams.get(n)?))
    }

    pub fn team_of(&self, holder: &str) -> Option<&Team> {
        self.teams.get(self.members.get(holder)?)
    }

    /// RGB color of a holder's team (outline of glowing entities), if it has one.
    pub fn team_rgb(&self, holder: &str) -> Option<u32> {
        let i = self.team_of(holder)?.params.color?;
        rc_text::parse_color(COLOR_NAMES.get(usize::try_from(i).ok()?)?)
    }

    /// Name formatted by the holder's team (§ 4): empty component in the team's color,
    /// children prefix, name, suffix.
    pub fn decorate(&self, holder: &str, name: Component) -> Component {
        let Some(t) = self.team_of(holder) else { return name };
        let mut c = Component::text("");
        c.style.color = t.params.color.and_then(|i| rc_text::parse_color(COLOR_NAMES[i as usize]));
        c.extra =
            [component(&t.params.prefix), Some(name), component(&t.params.suffix)].into_iter().flatten().collect();
        c
    }

    /// Text of a value according to the effective format (§ 2); `None` for `blank`.
    fn format_value(&self, s: &Score, o: &Objective, place: Place) -> Option<Component> {
        let fmt = s.format.as_ref().or(o.format.as_ref());
        let styled = |style: Option<&Nbt>| {
            let mut fields = match style {
                Some(Nbt::Compound(f)) => f.clone(),
                _ => Vec::new(),
            };
            fields.retain(|(k, _)| k != "text");
            fields.push(("text".into(), Nbt::String(s.value.to_string())));
            rc_text::from_nbt(&Nbt::Compound(fields))
        };
        match fmt {
            Some(NumberFormat::Blank) => None,
            Some(NumberFormat::Fixed(c)) => component(c),
            Some(NumberFormat::Styled(st)) => styled(st.as_ref()),
            None => {
                let mut c = Component::text(s.value.to_string());
                c.style.color = match place {
                    Place::Sidebar => rc_text::parse_color("red"),
                    Place::List => rc_text::parse_color("yellow"),
                    Place::BelowName => None,
                };
                Some(c)
            }
        }
    }

    /// Objective of the Tab list (slot 0) and score text of a player.
    pub fn list_score(&self, holder: &str) -> Option<(bool, i32, Option<Component>)> {
        let name = self.slots[0].as_ref()?;
        let o = self.objectives.get(name)?;
        let s = self.score(holder, name);
        let text = s.and_then(|s| self.format_value(s, o, Place::List));
        Some((o.hearts, s.map_or(0, |s| s.value), text))
    }

    /// "Below name" line (§ 7): formatted value, space, objective name.
    pub fn below_name(&self, holder: &str) -> Option<Component> {
        let name = self.slots[2].as_ref()?;
        let o = self.objectives.get(name)?;
        let s = self.score(holder, name)?;
        let mut c = Component::text("");
        if let Some(v) = self.format_value(s, o, Place::BelowName) {
            c.extra.push(v);
        }
        c.extra.push(Component::text(" "));
        if let Some(t) = component(&o.display) {
            c.extra.push(t);
        }
        Some(c)
    }

    /// Objective of one of the sidebar slots (1, or 3 to 18 per team color).
    fn on_sidebar(&self, objective: &str) -> bool {
        self.slots.iter().enumerate().any(|(i, s)| i != 0 && i != 2 && s.as_deref() == Some(objective))
    }

    /// Objective shown in the sidebar for the local player: that of its team's color,
    /// otherwise slot 1.
    fn sidebar_objective(&self, local: &str) -> Option<(&String, &Objective)> {
        let team_slot =
            self.team_of(local).and_then(|t| t.params.color).and_then(|c| self.slots[3 + c as usize].as_ref());
        let name = team_slot.or(self.slots[1].as_ref())?;
        Some((name, self.objectives.get(name)?))
    }

    /// Formatted title and lines of the retained scores.
    fn sidebar_build(&self, o: &Objective, rows: &[(&String, &Score)]) -> SidebarRows {
        let built = rows
            .iter()
            .map(|(h, s)| {
                let n = component(&s.display).unwrap_or_else(|| Component::text(h.as_str()));
                (self.decorate(h, n), self.format_value(s, o, Place::Sidebar))
            })
            .collect();
        SidebarRows {
            title: component(&o.display),
            rows: built,
            #[cfg(test)]
            holders: rows.iter().map(|(h, _)| (*h).clone()).collect(),
        }
    }

    /// Old path: all the scores of the objective (`#` holders excluded), stable sort by decreasing
    /// value then case-insensitive name, first 15. Full ties keep the table's walk order, like
    /// the game (no tie-break on the exact name).
    fn sidebar_rows_full(&self, local: &str) -> Option<SidebarRows> {
        let (name, o) = self.sidebar_objective(local)?;
        let mut rows: Vec<(&String, &Score)> = self
            .scores
            .iter()
            .filter(|((h, ob), _)| ob == name && !h.starts_with('#'))
            .map(|((h, _), s)| (h, s))
            .collect();
        rows.sort_by(|a, b| b.1.value.cmp(&a.1.value).then_with(|| cmp_ignore_case(a.0, b.0)));
        rows.truncate(SIDEBAR_ROWS);
        Some(self.sidebar_build(o, &rows))
    }

    /// Same result as [`Self::sidebar_rows_full`] by selection: the walk position serves as the
    /// last criterion (total order identical to the stable sort), the first 15 are isolated in
    /// linear time then sorted alone.
    fn sidebar_rows_select(&self, local: &str) -> Option<SidebarRows> {
        let (name, o) = self.sidebar_objective(local)?;
        let mut rows: Vec<(usize, &String, &Score)> = self
            .scores
            .iter()
            .filter(|((h, ob), _)| ob == name && !h.starts_with('#'))
            .enumerate()
            .map(|(i, ((h, _), s))| (i, h, s))
            .collect();
        let order = |a: &(usize, &String, &Score), b: &(usize, &String, &Score)| {
            b.2.value.cmp(&a.2.value).then_with(|| cmp_ignore_case(a.1, b.1)).then(a.0.cmp(&b.0))
        };
        if rows.len() > SIDEBAR_ROWS {
            rows.select_nth_unstable_by(SIDEBAR_ROWS - 1, order);
            rows.truncate(SIDEBAR_ROWS);
        }
        rows.sort_unstable_by(order);
        let rows: Vec<(&String, &Score)> = rows.into_iter().map(|(_, h, s)| (h, s)).collect();
        Some(self.sidebar_build(o, &rows))
    }

    /// Sidebar of the local player, recomputed only when the revision or the player changes.
    fn sidebar_rows(&mut self, local: &str) -> Option<&SidebarRows> {
        let stale = self.sidebar_cache.as_ref().is_none_or(|c| c.revision != self.revision || c.local != local);
        if stale {
            let rows = self.sidebar_rows_select(local);
            self.sidebar_cache =
                Some(SidebarCache { revision: self.revision, local: local.to_owned(), rows, styled: None });
        }
        self.sidebar_cache.as_ref().and_then(|c| c.rows.as_ref())
    }

    /// Kept sidebar styled and measured, again only when its rows or the text context change.
    fn styled_sidebar(&mut self, gui: &Gui, local: &str) -> Option<&StyledSidebar> {
        self.sidebar_rows(local)?;
        let cache = self.sidebar_cache.as_mut()?;
        let rows = cache.rows.as_ref()?;
        let context = gui.text_context();
        if cache.styled.as_ref().is_none_or(|(c, _)| *c != context) {
            cache.styled = Some((context, StyledSidebar::build(gui, rows)));
            #[cfg(test)]
            {
                self.styled_builds += 1;
            }
        }
        cache.styled.as_ref().map(|(_, s)| s)
    }

    /// Sidebar (§ 5), new stratum; `local` = profile name of the local player.
    pub fn draw_sidebar(&mut self, gui: &mut Gui, queue: &wgpu::Queue, local: &str, text_background: Option<u32>) {
        if !sidebar_cache_on() {
            if let Some(rows) = self.sidebar_rows_full(local) {
                StyledSidebar::build(gui, &rows).draw(gui, queue, text_background);
            }
        } else if !sidebar_styled_cache_on() {
            if let Some(rows) = self.sidebar_rows(local) {
                StyledSidebar::build(gui, rows).draw(gui, queue, text_background);
            }
        } else if let Some(styled) = self.styled_sidebar(gui, local) {
            styled.draw(gui, queue, text_background);
        }
    }
}

#[cfg(test)]
mod tests;
