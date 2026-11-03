//! Oracle of the equivalence proofs: the list's draw as it was before its rows were kept between
//! frames (sort, formatting and measuring of every player on every frame), frozen unchanged.

use super::super::*;

/// List score: hearts, value, formatted text.
type ListScore = (bool, i32, Vec<StyledChar>);

/// Whole list rebuilt and drawn, every frame.
pub(super) fn draw(
    tab: &mut TabList,
    gui: &mut Gui,
    queue: &wgpu::Queue,
    players: &PlayerList,
    ctx: &Ctx<'_>,
    scores: &Scoreboard,
) {
    let online = ctx.online;
    // Sort: list order descending, spectators after, team name, case-insensitive name.
    let mut list: Vec<(u128, &PlayerEntry)> =
        players.map.iter().filter(|(_, p)| p.listed).map(|(&u, p)| (u, p)).collect();
    // Hearts state cleared for the players that left the list.
    tab.hearts.retain(|u, _| players.map.get(u).is_some_and(|p| p.listed));
    tab.skins.retain(|u, _| players.map.contains_key(u));
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
            let s =
                scores.list_score(&p.name).map(|(h, v, t)| (h, v, t.map(|c| chars(&c, &gui.lang)).unwrap_or_default()));
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
    let header = wrap(gui, &tab.header);
    let footer = wrap(gui, &tab.footer);
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
            tab.head(gui, queue, i as usize, (*uuid, p.show_hat, upside_down), ctx, xo as f32, yo as f32);
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
                let st = tab.hearts.entry(*uuid).or_insert(Hearts { last: *v, shown: *v, changed: tick, blink_end: 0 });
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
