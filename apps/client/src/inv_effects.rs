//! Active effects to the right of the inventory and the creative inventory (`inventories.md`): a
//! 32 px tile per effect (icon, name with level, duration), compact (icon only, tooltip on
//! hover) when space is lacking; the HUD display disappears while the screen shows them.

use std::cmp::Ordering;

use rc_text::{Component, StyledChar};

use crate::gui::Gui;
use crate::hud_extra::Effect;

/// Column of tiles: start x and available width, if at least 32 px.
pub fn column(gui: &Gui, left: i32, image_w: i32) -> Option<(i32, i32)> {
    let x0 = left + image_w + 2;
    let avail = gui.width as i32 - x0;
    (avail >= 32).then_some((x0, avail))
}

/// Natural order of the game: non-ambient, finite, then increasing duration; beyond 32,147 ticks
/// (or both ambient), ambient flag then effect (effect color replaced by its index).
fn natural(a: &Effect, b: &Effect) -> Ordering {
    let amb = |e: &Effect| e.flags & 1 != 0;
    let inf = |e: &Effect| e.duration < 0;
    if (a.duration <= 32147 || b.duration <= 32147) && (!amb(a) || !amb(b)) {
        amb(a).cmp(&amb(b)).then(inf(a).cmp(&inf(b))).then(a.duration.cmp(&b.duration)).then(a.id.cmp(&b.id))
    } else {
        amb(a).cmp(&amb(b)).then(a.id.cmp(&b.id))
    }
}

/// Duration "mm:ss" ("hh:mm:ss" beyond an hour) at the server's tick rate, or "infinite".
fn duration(e: &Effect, tickrate: f32) -> Component {
    if e.duration < 0 {
        return Component::translate("effect.duration.infinite", vec![]);
    }
    let s = (e.duration as f32 / tickrate.max(1.0)).floor() as i32;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    Component::text(if h > 0 { format!("{h:02}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") })
}

/// Name of the effect, followed by the level in Roman numerals for amplifiers 1 to 9.
fn name(e: &Effect, names: &[String]) -> Component {
    let id = usize::try_from(e.id).ok().and_then(|i| names.get(i)).map_or("", String::as_str);
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    let mut c = Component::translate(format!("effect.{ns}.{path}"), vec![]);
    if (1..=9).contains(&e.amplifier) {
        c.extra.push(Component::text(" "));
        c.extra.push(Component::translate(format!("enchantment.level.{}", e.amplifier + 1), vec![]));
    }
    c
}

/// Text cut to `max` px with "...".
fn clip(gui: &Gui, chars: &[StyledChar], max: i32) -> Vec<StyledChar> {
    let dots = rc_text::styled_chars(&rc_text::flatten(&Component::text("..."), &gui.lang));
    let room = max - rc_text::width(gui, &dots);
    let mut out: Vec<StyledChar> = Vec::new();
    for c in chars {
        out.push(c.clone());
        if rc_text::width(gui, &out) > room {
            out.pop();
            break;
        }
    }
    out.extend(dots);
    out
}

/// Draws the tiles at `(x0, top)`; `mouse` in GUI pixels.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    effects: &[Effect],
    names: &[String],
    tickrate: f32,
    (x0, avail): (i32, i32),
    top: i32,
    mouse: Option<(f32, f32)>,
) {
    if effects.is_empty() {
        return;
    }
    let max_w = if avail >= 120 { avail - 7 } else { 32 };
    let step = if effects.len() > 5 { 132 / (effects.len() as i32 - 1) } else { 33 };
    let mut list: Vec<&Effect> = effects.iter().collect();
    list.sort_by(|a, b| natural(a, b));
    let flat = |gui: &Gui, c: &Component| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
    gui.next_layer();
    let mut tooltip = None;
    let mut y0 = top;
    for e in list {
        let ambient = e.flags & 1 != 0;
        let label = flat(gui, &name(e, names));
        let time = flat(gui, &duration(e, tickrate));
        let (lw, tw) = (rc_text::width(gui, &label), rc_text::width(gui, &time));
        let width = max_w.min((32 + lw + 7).max(32 + tw + 7));
        let bg = if ambient {
            "container/inventory/effect_background_ambient"
        } else {
            "container/inventory/effect_background"
        };
        gui.sprite(bg, x0 as f32, y0 as f32, width as f32, 32.0, 0xffff_ffff);
        let room = width - 32 - 7;
        let compact = if room > 0 {
            let clipped = lw > room;
            let shown = if clipped { clip(gui, &label, room) } else { label.clone() };
            gui.text(queue, &shown, (x0 + 32) as f32, (y0 + 7) as f32, 0xffff_ffff, true, 0);
            gui.text(queue, &time, (x0 + 32) as f32, (y0 + 16) as f32, 0xff80_8080, true, 0);
            clipped
        } else {
            true
        };
        if compact
            && let Some((mx, my)) = mouse
            && mx >= x0 as f32
            && mx <= (x0 + width) as f32
            && my >= y0 as f32
            && my <= (y0 + step) as f32
        {
            tooltip = Some(vec![label, time]);
        }
        if let Some(n) = usize::try_from(e.id).ok().and_then(|i| names.get(i)) {
            let path = format!("mob_effect/{}", n.strip_prefix("minecraft:").unwrap_or(n));
            gui.sprite(&path, (x0 + 7) as f32, (y0 + 7) as f32, 18.0, 18.0, 0xffff_ffff);
        }
        y0 += step;
    }
    if let (Some(lines), Some((mx, my))) = (tooltip, mouse) {
        gui.next_layer();
        crate::special::text_tooltip(gui, queue, &lines, (mx, my));
    }
}

/// Column of effects of the open screen (inventory or creative), with the top of the image.
pub fn screen_column(
    gui: &Gui,
    creative: bool,
    screens: &crate::screens::Screens,
    inv: &crate::inventory::PlayerInventory,
) -> Option<((i32, i32), i32)> {
    if creative {
        let (x, y) = crate::creative_screen::origin(gui);
        return column(gui, x, crate::creative_screen::W).map(|c| (c, y));
    }
    if !screens.open || inv.container.is_some() {
        return None;
    }
    let (gx, gy, w, _) = crate::screens::origin(gui, crate::menu::Kind::Player, inv.menu.len(), screens.book);
    column(gui, gx, w).map(|c| (c, gy))
}
