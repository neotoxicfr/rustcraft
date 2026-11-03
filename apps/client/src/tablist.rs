//! Player list (Tab key, `docs/specs/player-list.md`): listed players sorted, at most
//! 80, columns of 20 rows, header and footer, latency icons.
//!
//! What the list shows only changes with the player list, the scoreboard, the language, the
//! fonts, the bound keys, the header and footer and the screen width: those rows are built once
//! and kept while the list stays open (`RUSTCRAFT_OPT_TAB_LIST_CACHE`); hearts, latencies, heads
//! and placement are still done on every frame.

use std::cmp::Ordering;
use std::sync::OnceLock;

use hashbrown::HashMap;
use rc_commands::suggest::cmp_ignore_case;

use rc_proto::nbt::Nbt;
use rc_text::{Component, Lang, StyledChar};

use crate::gui::{Gui, TextContext};
use crate::players::{PlayerEntry, PlayerList, Revision};
use crate::scoreboard::Scoreboard;

/// Players shown at most.
const MAX_SHOWN: usize = 80;

/// Switches of the list, read once: rows kept between frames (`RUSTCRAFT_OPT_TAB_LIST_CACHE`,
/// `=0`: rebuilt on every frame) and sort keys computed once per player
/// (`RUSTCRAFT_OPT_TAB_SORT_KEY`, `=0`: team looked up at each comparison).
#[derive(Clone, Copy)]
struct TabListOpts {
    cache: bool,
    sort_key: bool,
}

impl Default for TabListOpts {
    fn default() -> TabListOpts {
        static OPTS: OnceLock<TabListOpts> = OnceLock::new();
        *OPTS.get_or_init(|| {
            let on = |v: Option<&std::ffi::OsStr>| v.is_none_or(|v| v != "0");
            TabListOpts {
                cache: on(rc_data::env_os!("RUSTCRAFT_OPT_TAB_LIST_CACHE")),
                sort_key: on(rc_data::env_os!("RUSTCRAFT_OPT_TAB_SORT_KEY")),
            }
        })
    }
}

/// Header and footer received (raw components), hearts state per player, faces.
#[derive(Default)]
pub struct TabList {
    pub header: Option<Nbt>,
    pub footer: Option<Nbt>,
    hearts: HashMap<u128, Hearts>,
    /// Face (8 × 8, opaque) and hat (8 × 8) of each known player's skin.
    skins: HashMap<u128, Face>,
    /// A downloaded skin arrived since the faces of the departed players were last forgotten.
    skin_added: bool,
    /// Content written into each `dyn/tab_face/<i>` slot: player, hat, flipped,
    /// generation of its skin.
    slots: Vec<Option<(u128, bool, bool, u32)>>,
    generation: u32,
    shown: bool,
    /// Rows of the last frame, kept while their inputs do not change.
    kept: Option<KeptRows>,
    opts: TabListOpts,
    /// Rows built so far (the proofs check that the kept rows are really used).
    #[cfg(test)]
    rebuilds: usize,
}

/// Face of a skin: face and hat (RGBA 8 × 8 each), generation on arrival.
struct Face {
    face: Vec<u8>,
    hat: Vec<u8>,
    generation: u32,
}

/// Face (8, 8) made opaque and hat (40, 8) of a 64 × 64 (or 64 × 32) skin.
pub(crate) fn face_parts(img: &rc_assets::texture::Image) -> Option<(Vec<u8>, Vec<u8>)> {
    if img.width < 48 || img.height < 16 {
        return None;
    }
    let mut face = Vec::with_capacity(256);
    let mut hat = Vec::with_capacity(256);
    for y in 0..8 {
        for x in 0..8 {
            let at = |u: u32| (((8 + y) * img.width + u + x) * 4) as usize;
            let (f, h) = (at(8), at(40));
            face.extend_from_slice(&[img.rgba[f], img.rgba[f + 1], img.rgba[f + 2], 255]);
            hat.extend_from_slice(&img.rgba[h..h + 4]);
        }
    }
    Some((face, hat))
}

/// Displayed face: face, hat on top if shown, rows reversed if flipped.
pub(crate) fn compose(face: &[u8], hat: &[u8], show_hat: bool, flip: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    for y in 0..8 {
        let sy = if flip { 7 - y } else { y };
        for x in 0..8 {
            let i = (sy * 8 + x) * 4;
            let ha = if show_hat { u32::from(hat[i + 3]) } else { 0 };
            let mix = |k: usize| ((u32::from(face[i + k]) * (255 - ha) + u32::from(hat[i + k]) * ha) / 255) as u8;
            out.extend_from_slice(&[mix(0), mix(1), mix(2), 255]);
        }
    }
    out
}

/// Default skin of a UUID (before the download of its own).
pub fn default_skin(assets: &rc_assets::Assets, uuid: u128) -> Option<rc_assets::texture::Image> {
    let skin = crate::entity_render::default_skin(uuid);
    let kind = if skin < 9 { "slim" } else { "wide" };
    let name = crate::entity_render::SKINS[skin as usize % 9];
    assets.image(&format!("assets/minecraft/textures/entity/player/{kind}/{name}.png"))
}

/// Name of the dynamic sprite of head slot `slot` (`dyn/tab_face/<slot>`), built once instead of
/// formatted for each head of each frame.
fn face_sprite(slot: usize) -> &'static str {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    &NAMES.get_or_init(|| (0..MAX_SHOWN).map(|i| format!("dyn/tab_face/{i}")).collect())[slot]
}

/// List visible while the key is held: never on the integrated server with at most one
/// listed player and without a "list" objective.
pub fn visible(solo: bool, players: &PlayerList, scores: &Scoreboard) -> bool {
    !solo || players.map.values().filter(|p| p.listed).count() > 1 || scores.list_score("").is_some()
}

/// Hearts of a player (`scoreboard-teams.md` § 6.1): last value, displayed value, tick of the
/// last modification, end of blinking.
#[derive(Clone, Copy)]
struct Hearts {
    last: i32,
    shown: i32,
    changed: i64,
    blink_end: i64,
}

impl Hearts {
    /// Update at draw time of tick `t` with value `v`; true if the hearts blink.
    fn update(&mut self, v: i32, t: i64) -> bool {
        if v != self.last {
            self.blink_end = t + if v < self.last { 20 } else { 10 };
            self.last = v;
            self.changed = t;
        }
        if t - self.changed > 20 {
            self.shown = v;
        }
        self.blink_end > t && (self.blink_end - t) % 6 >= 3
    }
}

fn chars(c: &Component, lang: &Lang) -> Vec<StyledChar> {
    rc_text::styled_chars(&rc_text::flatten(c, lang))
}

/// Hearts of a slot (`scoreboard-teams.md` § 6.1) between `score_left` and `score_right`:
/// sprites, or "10.0hp" text colored from red to green if there is no room.
#[allow(clippy::too_many_arguments)]
fn hearts_row(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    v: i32,
    shown: i32,
    blink: bool,
    score_left: i32,
    score_right: i32,
    y: i32,
) {
    let full = (v.max(shown) + 1) / 2;
    if full <= 0 {
        return;
    }
    let total = v.max(shown).max(20) / 2;
    let step = ((score_right - score_left - 4) as f32 / total as f32).min(9.0).floor();
    if step <= 3.0 {
        let p = (v as f32 / 20.0).clamp(0.0, 1.0);
        let color = 0xff00_0000 | (((1.0 - p) * 255.0) as u32) << 16 | ((p * 255.0) as u32) << 8;
        let hp = format!("{:?}", v as f32 / 2.0);
        let long = Component::translate("multiplayer.player.list.hp", vec![rc_text::Arg::Text(hp.clone())]);
        let mut text = chars(&long, &gui.lang);
        if rc_text::width(gui, &text) > score_right - score_left {
            text = chars(&Component::text(hp.as_str()), &gui.lang);
        }
        let w = rc_text::width(gui, &text);
        gui.text(queue, &text, ((score_right + score_left - w) / 2) as f32, y as f32, color, true, 0);
        return;
    }
    let at = |k: i32| score_left as f32 + k as f32 * step;
    let container = if blink { "hud/heart/container_blinking" } else { "hud/heart/container" };
    for k in full..total {
        gui.sprite(container, at(k), y as f32, 9.0, 9.0, 0xffff_ffff);
    }
    for k in 0..full {
        gui.sprite(container, at(k), y as f32, 9.0, 9.0, 0xffff_ffff);
        if blink {
            if 2 * k + 1 < shown {
                gui.sprite("hud/heart/full_blinking", at(k), y as f32, 9.0, 9.0, 0xffff_ffff);
            }
            if 2 * k + 1 == shown {
                gui.sprite("hud/heart/half_blinking", at(k), y as f32, 9.0, 9.0, 0xffff_ffff);
            }
        }
        let extra = k >= 10;
        if 2 * k + 1 < v {
            let s = if extra { "hud/heart/absorbing_full_blinking" } else { "hud/heart/full" };
            gui.sprite(s, at(k), y as f32, 9.0, 9.0, 0xffff_ffff);
        }
        if 2 * k + 1 == v {
            let s = if extra { "hud/heart/absorbing_half_blinking" } else { "hud/heart/half" };
            gui.sprite(s, at(k), y as f32, 9.0, 9.0, 0xffff_ffff);
        }
    }
}

/// Latency icon.
fn ping_icon(latency: i32) -> &'static str {
    match latency {
        l if l < 0 => "icon/ping_unknown",
        l if l < 150 => "icon/ping_5",
        l if l < 300 => "icon/ping_4",
        l if l < 600 => "icon/ping_3",
        l if l < 1000 => "icon/ping_2",
        _ => "icon/ping_1",
    }
}

/// Draw parameters coming from the application.
pub struct Ctx<'a> {
    /// Server in online mode: player heads.
    pub online: bool,
    /// HUD ticks (blinking of the hearts).
    pub tick: i64,
    /// Background of the slots (0x20FFFFFF, or the text-background opacity outside chat).
    pub background: u32,
    /// The player with this UUID is present in the world (flipped heads of Dinnerbone/Grumm).
    pub in_world: &'a dyn Fn(u128) -> bool,
    /// Default skin of a UUID (before download).
    pub default_skin: &'a dyn Fn(u128) -> Option<rc_assets::texture::Image>,
}

/// List order (descending), then spectators after the others.
fn by_list_order(a: &PlayerEntry, b: &PlayerEntry) -> Ordering {
    b.list_order.cmp(&a.list_order).then((a.game_mode == 3).cmp(&(b.game_mode == 3)))
}

/// Listed players in the list's order (list order descending, spectators after, team name,
/// case-insensitive name; full ties keep the walk order of the table), at most 80.
fn sorted_entries<'a>(players: &'a PlayerList, scores: &Scoreboard, sort_key: bool) -> Vec<(u128, &'a PlayerEntry)> {
    let listed = players.map.iter().filter(|(_, p)| p.listed).map(|(&u, p)| (u, p));
    let team = |n: &str| scores.team_named(n).map_or("", |t| t.0);
    let mut list: Vec<(u128, &PlayerEntry)> = if sort_key {
        // Team of each player looked up once, instead of twice per comparison that reaches it.
        let mut keyed: Vec<(&str, u128, &PlayerEntry)> = listed.map(|(u, p)| (team(&p.name), u, p)).collect();
        keyed.sort_by(|(ta, _, a), (tb, _, b)| {
            by_list_order(a, b).then_with(|| ta.cmp(tb)).then_with(|| cmp_ignore_case(&a.name, &b.name))
        });
        keyed.into_iter().map(|(_, u, p)| (u, p)).collect()
    } else {
        let mut list: Vec<(u128, &PlayerEntry)> = listed.collect();
        list.sort_by(|(_, a), (_, b)| {
            by_list_order(a, b)
                .then_with(|| team(&a.name).cmp(team(&b.name)))
                .then_with(|| cmp_ignore_case(&a.name, &b.name))
        });
        list
    };
    list.truncate(MAX_SHOWN);
    list
}

/// Score of a row in the "list" objective (`scoreboard-teams.md` § 6): hearts, or text.
struct RowScore {
    hearts: bool,
    value: i32,
    text: Vec<StyledChar>,
    text_width: i32,
}

/// A listed player as the list shows it.
struct TabRow {
    uuid: u128,
    name: Vec<StyledChar>,
    spectator: bool,
    show_hat: bool,
    /// Dinnerbone or Grumm: head upside down while the player is in the world (checked on every frame).
    flips_in_world: bool,
    latency: i32,
    score: Option<RowScore>,
}

impl TabRow {
    fn build(gui: &Gui, scores: &Scoreboard, uuid: u128, p: &PlayerEntry) -> TabRow {
        // Own display name (not formatted), otherwise profile name formatted by the team.
        let display = match p.display_name.as_ref().and_then(rc_text::from_nbt) {
            Some(c) => c,
            None => scores.decorate(&p.name, Component::text(p.name.as_str())),
        };
        let spectator = p.game_mode == 3;
        let mut name = chars(&display, &gui.lang);
        if spectator {
            for ch in &mut name {
                ch.style.italic = true;
            }
        }
        let score = scores.list_score(&p.name).map(|(hearts, value, text)| {
            let text = text.map(|c| chars(&c, &gui.lang)).unwrap_or_default();
            RowScore { hearts, value, text_width: rc_text::width(gui, &text), text }
        });
        TabRow {
            uuid,
            name,
            spectator,
            show_hat: p.show_hat,
            flips_in_world: p.name == "Dinnerbone" || p.name == "Grumm",
            latency: p.latency,
            score,
        }
    }
}

/// Header or footer: wrapped lines and their widths.
struct Banner {
    lines: Vec<(Vec<StyledChar>, i32)>,
}

impl Banner {
    /// Lines of a received component wrapped at `wrap_width`; a component with empty text shows nothing.
    fn build(gui: &Gui, raw: &Option<Nbt>, wrap_width: i32) -> Option<Banner> {
        let text = chars(&rc_text::from_nbt(raw.as_ref()?)?, &gui.lang);
        if text.is_empty() {
            return None;
        }
        let lines = rc_text::wrap(gui, &text, wrap_width as f32).into_iter().map(|l| {
            let width = rc_text::width(gui, &l);
            (l, width)
        });
        Some(Banner { lines: lines.collect() })
    }

    fn height(&self) -> i32 {
        self.lines.len() as i32 * 9
    }

    /// Lines centered on the screen from `top`.
    fn draw(&self, gui: &mut Gui, queue: &wgpu::Queue, screen: i32, top: i32) {
        for (i, (line, width)) in self.lines.iter().enumerate() {
            let y = top + i as i32 * 9;
            gui.text(queue, line, (screen / 2 - width / 2) as f32, y as f32, 0xffff_ffff, true, 0);
        }
    }
}

/// Everything the list shows that does not change from one frame to the next.
struct TabRows {
    rows: Vec<TabRow>,
    /// Widest name.
    name_width: i32,
    /// Score column: 90 with hearts, otherwise the widest text and the space before it.
    score_width: i32,
    header: Option<Banner>,
    footer: Option<Banner>,
}

impl TabRows {
    fn build(
        gui: &Gui,
        players: &PlayerList,
        scores: &Scoreboard,
        banners: [&Option<Nbt>; 2],
        sort_key: bool,
    ) -> TabRows {
        let rows: Vec<TabRow> = sorted_entries(players, scores, sort_key)
            .into_iter()
            .map(|(uuid, p)| TabRow::build(gui, scores, uuid, p))
            .collect();
        let name_width = rows.iter().map(|r| rc_text::width(gui, &r.name)).max().unwrap_or(0);
        let hearts = rows.iter().any(|r| r.score.as_ref().is_some_and(|s| s.hearts));
        let space = rc_text::width(gui, &chars(&Component::text(" "), &gui.lang));
        let score_width = if hearts {
            90
        } else {
            let text = |s: &RowScore| if s.text_width > 0 { space + s.text_width } else { 0 };
            rows.iter().filter_map(|r| r.score.as_ref()).map(text).max().unwrap_or(0)
        };
        let wrap_width = gui.width as i32 - 50;
        TabRows {
            rows,
            name_width,
            score_width,
            header: Banner::build(gui, banners[0], wrap_width),
            footer: Banner::build(gui, banners[1], wrap_width),
        }
    }

    /// Latencies read again (they change without the rest).
    fn refresh_latencies(&mut self, players: &PlayerList) {
        for row in &mut self.rows {
            if let Some(p) = players.map.get(&row.uuid) {
                row.latency = p.latency;
            }
        }
    }
}

/// What the rows were built from: they stay valid as long as this does not change.
#[derive(Clone, Copy, PartialEq)]
struct RowsKey {
    players: Revision,
    scores: Revision,
    /// Language, fonts and bound keys.
    text: TextContext,
    /// Screen width (wrapping of the header and footer).
    screen: i32,
}

impl RowsKey {
    fn of(gui: &Gui, players: &PlayerList, scores: &Scoreboard) -> RowsKey {
        RowsKey {
            players: players.revision(),
            scores: scores.list_revision(),
            text: gui.text_context(),
            screen: gui.width as i32,
        }
    }
}

/// Rows kept between frames, with what they were built from.
struct KeptRows {
    key: RowsKey,
    /// Header and footer as received: compared on every frame (the application assigns them directly).
    header: Option<Nbt>,
    footer: Option<Nbt>,
    latencies: Revision,
    rows: TabRows,
}

/// Placement of the slots for one frame.
struct Layout {
    screen: i32,
    /// Width of the head before the name (online server), 0 otherwise.
    head: i32,
    /// Slots per column.
    per_column: i32,
    slot_width: i32,
    /// Left edge of the first column, top of the first row.
    left: i32,
    top: i32,
    /// Width of the dark bands: the columns, or the widest header or footer line.
    band_width: i32,
    name_width: i32,
    score_width: i32,
}

impl Layout {
    fn of(rows: &TabRows, screen: i32, online: bool) -> Layout {
        let slots = rows.rows.len() as i32;
        let (mut per_column, mut columns) = (slots, 1);
        while per_column > 20 {
            columns += 1;
            per_column = (slots + columns - 1) / columns;
        }
        let head = if online { 9 } else { 0 };
        let slot_width = (columns * (head + rows.name_width + rows.score_width + 13)).min(screen - 50) / columns;
        let columns_width = slot_width * columns + (columns - 1) * 5;
        let banner_lines = [&rows.header, &rows.footer].into_iter().flatten().flat_map(|b| &b.lines);
        let band_width = banner_lines.map(|(_, w)| *w).fold(columns_width, i32::max);
        Layout {
            screen,
            head,
            per_column,
            slot_width,
            left: screen / 2 - columns_width / 2,
            top: 10 + rows.header.as_ref().map_or(0, |h| h.height() + 1),
            band_width,
            name_width: rows.name_width,
            score_width: rows.score_width,
        }
    }

    /// Dark band behind rows `y0..y1`.
    fn band(&self, gui: &mut Gui, y0: i32, y1: i32) {
        let (center, half) = (self.screen / 2, self.band_width / 2);
        gui.fill((center - half - 1) as f32, (y0 - 1) as f32, (center + half + 1) as f32, y1 as f32, 0x8000_0000);
    }
}

impl TabList {
    /// Downloaded skin of a player.
    pub fn skin(&mut self, uuid: u128, image: &rc_assets::texture::Image) {
        if let Some((face, hat)) = face_parts(image) {
            self.generation = self.generation.wrapping_add(1);
            self.skins.insert(uuid, Face { face, hat, generation: self.generation });
            self.skin_added = true;
        }
    }

    /// Show or hide (each frame): the hearts and the kept rows are forgotten on every change.
    pub fn set_visible(&mut self, visible: bool) {
        if self.shown != visible {
            self.hearts.clear();
            self.kept = None;
            self.shown = visible;
        }
    }

    /// 8 × 8 head of a player in slot `slot`, rewritten if its content changes.
    #[allow(clippy::too_many_arguments)]
    fn head(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        slot: usize,
        p: (u128, bool, bool),
        ctx: &Ctx<'_>,
        x: f32,
        y: f32,
    ) {
        let (uuid, hat, flip) = p;
        let s = match self.skins.entry(uuid) {
            hashbrown::hash_map::Entry::Occupied(e) => e.into_mut(),
            hashbrown::hash_map::Entry::Vacant(e) => {
                let Some(img) = (ctx.default_skin)(uuid) else { return };
                let Some((face, hat)) = face_parts(&img) else { return };
                // Generation 0: replaced by the downloaded skin as soon as it arrives.
                e.insert(Face { face, hat, generation: 0 })
            }
        };
        let key = (uuid, hat, flip, s.generation);
        if self.slots.len() <= slot {
            self.slots.resize(slot + 1, None);
        }
        let name = face_sprite(slot);
        if self.slots[slot] != Some(key) {
            if !gui.write_sprite(queue, name, (8, 8), &compose(&s.face, &s.hat, hat, flip)) {
                return;
            }
            self.slots[slot] = Some(key);
        }
        gui.sprite(name, x, y, 8.0, 8.0, 0xffff_ffff);
    }

    /// Hearts and faces of the players who left the list (or the server) forgotten. Only needed
    /// when the players changed or a face arrived since the last time: between two such events
    /// it would remove nothing.
    fn forget_departed(&mut self, players: &PlayerList) {
        self.hearts.retain(|u, _| players.map.get(u).is_some_and(|p| p.listed));
        self.skins.retain(|u, _| players.map.contains_key(u));
        self.skin_added = false;
    }

    /// Rows of the previous frame if none of their inputs changed, otherwise rebuilt; the
    /// latencies are read again on their own when only they changed.
    fn take_kept_rows(&mut self, gui: &Gui, players: &PlayerList, scores: &Scoreboard) -> KeptRows {
        let key = RowsKey::of(gui, players, scores);
        let mut kept = match self.kept.take() {
            Some(k) if k.key == key && k.header == self.header && k.footer == self.footer => k,
            _ => {
                #[cfg(test)]
                {
                    self.rebuilds += 1;
                }
                self.forget_departed(players);
                let banners = [&self.header, &self.footer];
                KeptRows {
                    key,
                    header: self.header.clone(),
                    footer: self.footer.clone(),
                    latencies: players.latency_revision(),
                    rows: TabRows::build(gui, players, scores, banners, self.opts.sort_key),
                }
            }
        };
        if self.skin_added {
            self.forget_departed(players);
        }
        if kept.latencies != players.latency_revision() {
            kept.rows.refresh_latencies(players);
            kept.latencies = players.latency_revision();
        }
        kept
    }

    /// List displayed (Tab key).
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        players: &PlayerList,
        ctx: &Ctx<'_>,
        scores: &Scoreboard,
    ) {
        if self.opts.cache {
            let kept = self.take_kept_rows(gui, players, scores);
            self.draw_rows(gui, queue, ctx, &kept.rows);
            self.kept = Some(kept);
        } else {
            self.forget_departed(players);
            let rows = TabRows::build(gui, players, scores, [&self.header, &self.footer], self.opts.sort_key);
            self.draw_rows(gui, queue, ctx, &rows);
        }
    }

    /// Header, slots and footer of one frame.
    fn draw_rows(&mut self, gui: &mut Gui, queue: &wgpu::Queue, ctx: &Ctx<'_>, rows: &TabRows) {
        let layout = Layout::of(rows, gui.width as i32, ctx.online);
        if let Some(header) = &rows.header {
            layout.band(gui, 10, 10 + header.height());
            header.draw(gui, queue, layout.screen, 10);
        }
        layout.band(gui, layout.top, layout.top + layout.per_column * 9);
        for (index, row) in rows.rows.iter().enumerate() {
            self.draw_slot(gui, queue, ctx, &layout, index, row);
        }
        if let Some(footer) = &rows.footer {
            let top = layout.top + layout.per_column * 9 + 1;
            layout.band(gui, top, top + footer.height());
            footer.draw(gui, queue, layout.screen, top);
        }
    }

    /// One slot: background, head, name, score (integer right-aligned in its column, or
    /// hearts), latency icon.
    fn draw_slot(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        ctx: &Ctx<'_>,
        layout: &Layout,
        index: usize,
        row: &TabRow,
    ) {
        let i = index as i32;
        let (column, line) = (i / layout.per_column, i % layout.per_column);
        let left = layout.left + column * layout.slot_width + column * 5;
        let top = layout.top + line * 9;
        gui.fill(left as f32, top as f32, (left + layout.slot_width) as f32, (top + 8) as f32, ctx.background);
        if ctx.online {
            let upside_down = row.flips_in_world && (ctx.in_world)(row.uuid);
            self.head(gui, queue, index, (row.uuid, row.show_hat, upside_down), ctx, left as f32, top as f32);
        }
        let name_left = left + layout.head;
        let color = if row.spectator { 0x90ff_ffff } else { 0xffff_ffff };
        gui.text(queue, &row.name, name_left as f32, top as f32, color, true, 0);
        let score_left = name_left + layout.name_width + 1;
        let score_right = score_left + layout.score_width;
        if let Some(score) = &row.score
            && !row.spectator
            && score_right - score_left > 5
        {
            if score.hearts {
                let v = score.value;
                let hearts_state = self.hearts.entry(row.uuid).or_insert(Hearts {
                    last: v,
                    shown: v,
                    changed: ctx.tick,
                    blink_end: 0,
                });
                let blink = hearts_state.update(v, ctx.tick);
                hearts_row(gui, queue, v, hearts_state.shown, blink, score_left, score_right, top);
            } else {
                let x = score_right - score.text_width;
                gui.text(queue, &score.text, x as f32, top as f32, 0xffff_ffff, true, 0);
            }
        }
        let ping_left = left + layout.slot_width - 11;
        gui.sprite(ping_icon(row.latency), ping_left as f32, top as f32, 10.0, 8.0, 0xffff_ffff);
    }
}

#[cfg(test)]
mod tests;
