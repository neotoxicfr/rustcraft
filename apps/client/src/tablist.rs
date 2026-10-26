//! Player list (Tab key, `docs/specs/player-list.md`): listed players sorted, at most
//! 80, columns of 20 rows, header and footer, latency icons.

use hashbrown::HashMap;
use rc_commands::suggest::cmp_ignore_case;

use rc_text::{Lang, StyledChar};

use crate::gui::Gui;
use crate::players::{PlayerEntry, PlayerList};

/// List score: hearts, value, formatted text.
type ListScore = (bool, i32, Vec<StyledChar>);

/// Header and footer received (raw components), hearts state per player, faces.
#[derive(Default)]
pub struct TabList {
    pub header: Option<rc_proto::nbt::Nbt>,
    pub footer: Option<rc_proto::nbt::Nbt>,
    hearts: HashMap<u128, Hearts>,
    /// Face (8 × 8, opaque) and hat (8 × 8) of each known player's skin.
    skins: HashMap<u128, Face>,
    /// Content written into each `dyn/tab_face/<i>` slot: player, hat, flipped,
    /// generation of its skin.
    slots: Vec<Option<(u128, bool, bool, u32)>>,
    generation: u32,
    shown: bool,
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

/// List visible while the key is held: never on the integrated server with at most one
/// listed player and without a "list" objective.
pub fn visible(solo: bool, players: &PlayerList, scores: &crate::scoreboard::Scoreboard) -> bool {
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

fn chars(c: &rc_text::Component, lang: &Lang) -> Vec<StyledChar> {
    rc_text::styled_chars(&rc_text::flatten(c, lang))
}

/// Hearts of a slot (`scoreboard-teams.md` § 6.1) between `g` and `d`: sprites, or "10.0hp" text
/// colored from red to green if there is no room.
#[allow(clippy::too_many_arguments)]
fn hearts_row(gui: &mut Gui, queue: &wgpu::Queue, v: i32, shown: i32, blink: bool, g: i32, d: i32, y: i32) {
    let full = (v.max(shown) + 1) / 2;
    if full <= 0 {
        return;
    }
    let total = v.max(shown).max(20) / 2;
    let step = ((d - g - 4) as f32 / total as f32).min(9.0).floor();
    if step <= 3.0 {
        let p = (v as f32 / 20.0).clamp(0.0, 1.0);
        let color = 0xff00_0000 | (((1.0 - p) * 255.0) as u32) << 16 | ((p * 255.0) as u32) << 8;
        let hp = format!("{:?}", v as f32 / 2.0);
        let long = rc_text::Component::translate("multiplayer.player.list.hp", vec![rc_text::Arg::Text(hp.clone())]);
        let mut text = chars(&long, &gui.lang);
        if rc_text::width(gui, &text) > d - g {
            text = chars(&rc_text::Component::text(hp.as_str()), &gui.lang);
        }
        let w = rc_text::width(gui, &text);
        gui.text(queue, &text, ((d + g - w) / 2) as f32, y as f32, color, true, 0);
        return;
    }
    let at = |k: i32| g as f32 + k as f32 * step;
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

impl TabList {
    /// Downloaded skin of a player.
    pub fn skin(&mut self, uuid: u128, image: &rc_assets::texture::Image) {
        if let Some((face, hat)) = face_parts(image) {
            self.generation = self.generation.wrapping_add(1);
            self.skins.insert(uuid, Face { face, hat, generation: self.generation });
        }
    }

    /// Show or hide (each frame): the hearts are forgotten on every change.
    pub fn set_visible(&mut self, visible: bool) {
        if self.shown != visible {
            self.hearts.clear();
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
        let name = format!("dyn/tab_face/{slot}");
        if self.slots[slot] != Some(key) {
            if !gui.write_sprite(queue, &name, (8, 8), &compose(&s.face, &s.hat, hat, flip)) {
                return;
            }
            self.slots[slot] = Some(key);
        }
        gui.sprite(&name, x, y, 8.0, 8.0, 0xffff_ffff);
    }

    /// List displayed (Tab key).
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        players: &PlayerList,
        ctx: &Ctx<'_>,
        scores: &crate::scoreboard::Scoreboard,
    ) {
        let online = ctx.online;
        // Sort: list order descending, spectators after, team name, case-insensitive name.
        let mut list: Vec<(u128, &PlayerEntry)> =
            players.map.iter().filter(|(_, p)| p.listed).map(|(&u, p)| (u, p)).collect();
        // Hearts state cleared for the players that left the list.
        self.hearts.retain(|u, _| players.map.get(u).is_some_and(|p| p.listed));
        self.skins.retain(|u, _| players.map.contains_key(u));
        let team = |n: &str| scores.team_named(n).map_or("", |t| t.0);
        list.sort_by(|(_, a), (_, b)| {
            b.list_order
                .cmp(&a.list_order)
                .then((a.game_mode == 3).cmp(&(b.game_mode == 3)))
                .then_with(|| team(&a.name).cmp(team(&b.name)))
                .then_with(|| cmp_ignore_case(&a.name, &b.name))
        });
        list.truncate(80);
        let names: Vec<Vec<StyledChar>> = list
            .iter()
            .map(|(_, p)| {
                // Own display name (not formatted), otherwise profile name formatted by the team.
                let c = match p.display_name.as_ref().and_then(rc_text::from_nbt) {
                    Some(c) => c,
                    None => scores.decorate(&p.name, rc_text::Component::text(p.name.as_str())),
                };
                let mut v = chars(&c, &gui.lang);
                if p.game_mode == 3 {
                    for ch in &mut v {
                        ch.style.italic = true;
                    }
                }
                v
            })
            .collect();
        let width = |gui: &Gui, c: &[StyledChar]| rc_text::width(gui, c);
        let max_name = names.iter().map(|n| width(gui, n)).max().unwrap_or(0);
        // Score of the "list" objective (`scoreboard-teams.md` § 6): text, or hearts (width 90).
        let list: Vec<(u128, &PlayerEntry, Option<ListScore>)> = list
            .into_iter()
            .map(|(u, p)| {
                let s = scores
                    .list_score(&p.name)
                    .map(|(h, v, t)| (h, v, t.map(|c| chars(&c, &gui.lang)).unwrap_or_default()));
                (u, p, s)
            })
            .collect();
        let hearts = list.iter().any(|(_, _, s)| s.as_ref().is_some_and(|s| s.0));
        let tick = ctx.tick;
        let space = width(gui, &chars(&rc_text::Component::text(" "), &gui.lang));
        let score_w = if hearts {
            90
        } else {
            list.iter()
                .filter_map(|(_, _, s)| s.as_ref())
                .map(|s| {
                    let w = width(gui, &s.2);
                    if w > 0 { space + w } else { 0 }
                })
                .max()
                .unwrap_or(0)
        };
        let slots = list.len() as i32;
        let (mut rows, mut cols) = (slots, 1);
        while rows > 20 {
            cols += 1;
            rows = (slots + cols - 1) / cols;
        }
        let screen = gui.width as i32;
        let head = if online { 9 } else { 0 };
        let slot_w = (cols * (head + max_name + score_w + 13)).min(screen - 50) / cols;
        let xxo = screen / 2 - (slot_w * cols + (cols - 1) * 5) / 2;
        let mut yyo = 10;
        let mut max_line = slot_w * cols + (cols - 1) * 5;
        // Component with empty text: nothing displayed.
        let wrap = |gui: &Gui, n: &Option<rc_proto::nbt::Nbt>| -> Option<Vec<Vec<StyledChar>>> {
            let c = chars(&rc_text::from_nbt(n.as_ref()?)?, &gui.lang);
            (!c.is_empty()).then(|| rc_text::wrap(gui, &c, (screen - 50) as f32))
        };
        let header = wrap(gui, &self.header);
        let footer = wrap(gui, &self.footer);
        for lines in [&header, &footer].into_iter().flatten() {
            for l in lines {
                max_line = max_line.max(width(gui, l));
            }
        }
        let band = |gui: &mut Gui, y0: i32, y1: i32| {
            gui.fill(
                (screen / 2 - max_line / 2 - 1) as f32,
                (y0 - 1) as f32,
                (screen / 2 + max_line / 2 + 1) as f32,
                y1 as f32,
                0x8000_0000,
            );
        };
        let centered = |gui: &mut Gui, lines: &[Vec<StyledChar>], y: &mut i32| {
            for l in lines {
                let w = width(gui, l);
                gui.text(queue, l, (screen / 2 - w / 2) as f32, *y as f32, 0xffff_ffff, true, 0);
                *y += 9;
            }
        };
        if let Some(lines) = &header {
            band(gui, yyo, yyo + lines.len() as i32 * 9);
            centered(gui, lines, &mut yyo);
            yyo += 1;
        }
        band(gui, yyo, yyo + rows * 9);
        for (i, ((uuid, p, score), name)) in list.iter().zip(&names).enumerate() {
            let i = i as i32;
            let (col, row) = (i / rows, i % rows);
            let mut xo = xxo + col * slot_w + col * 5;
            let yo = yyo + row * 9;
            gui.fill(xo as f32, yo as f32, (xo + slot_w) as f32, (yo + 8) as f32, ctx.background);
            if online {
                let upside_down = (p.name == "Dinnerbone" || p.name == "Grumm") && (ctx.in_world)(*uuid);
                self.head(gui, queue, i as usize, (*uuid, p.show_hat, upside_down), ctx, xo as f32, yo as f32);
            }
            xo += head;
            let color = if p.game_mode == 3 { 0x90ff_ffff } else { 0xffff_ffff };
            gui.text(queue, name, xo as f32, yo as f32, color, true, 0);
            // Score: integer right-aligned in its column, or hearts.
            let (g, d) = (xo + max_name + 1, xo + max_name + 1 + score_w);
            if let Some((h, v, text)) = score
                && p.game_mode != 3
                && d - g > 5
            {
                if *h {
                    let st =
                        self.hearts.entry(*uuid).or_insert(Hearts { last: *v, shown: *v, changed: tick, blink_end: 0 });
                    let blink = st.update(*v, tick);
                    hearts_row(gui, queue, *v, st.shown, blink, g, d, yo);
                } else {
                    let w = width(gui, text);
                    gui.text(queue, text, (d - w) as f32, yo as f32, 0xffff_ffff, true, 0);
                }
            }
            let ping = match p.latency {
                l if l < 0 => "icon/ping_unknown",
                l if l < 150 => "icon/ping_5",
                l if l < 300 => "icon/ping_4",
                l if l < 600 => "icon/ping_3",
                l if l < 1000 => "icon/ping_2",
                _ => "icon/ping_1",
            };
            gui.sprite(ping, (xo - head + slot_w - 11) as f32, yo as f32, 10.0, 8.0, 0xffff_ffff);
        }
        if let Some(lines) = &footer {
            yyo += rows * 9 + 1;
            band(gui, yyo, yyo + lines.len() as i32 * 9);
            centered(gui, lines, &mut yyo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sort_like_the_game() {
        use std::cmp::Ordering::*;
        assert_eq!(cmp_ignore_case("alice", "Bob"), Less);
        assert_eq!(cmp_ignore_case("ABC", "abc"), Equal);
        assert_eq!(cmp_ignore_case("_x", "a"), Less);
    }

    #[test]
    fn face_hat_and_flip() {
        let face: Vec<u8> = (0..64).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        let mut hat = vec![0u8; 256];
        hat[3] = 255;
        hat[1] = 200;
        let up = compose(&face, &hat, true, false);
        assert_eq!(&up[0..4], &[0, 200, 0, 255]);
        assert_eq!(&compose(&face, &hat, false, false)[0..4], &[0, 0, 0, 255]);
        // Flipped: the first displayed row is the last one of the skin.
        assert_eq!(compose(&face, &hat, false, true)[0], 56);
    }

    #[test]
    fn hidden_alone_on_integrated_server() {
        let mut players = PlayerList::default();
        players.map.insert(1, PlayerEntry { listed: true, ..Default::default() });
        let scores = crate::scoreboard::Scoreboard::default();
        assert!(!visible(true, &players, &scores));
        assert!(visible(false, &players, &scores));
        players.map.insert(2, PlayerEntry { listed: true, ..Default::default() });
        assert!(visible(true, &players, &scores));
    }
}
