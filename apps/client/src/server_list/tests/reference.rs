//! Drawing of the multiplayer screen as it was before the rows kept their texts and decoded
//! their icon on change: the reference of `screen_draws_identical_layers` and the "before" of the
//! frame micro-bench. Kept as it was written; only the receiver (`screen` for `self`) changed.

use super::super::*;

pub(super) fn draw(
    screen: &mut Multiplayer,
    gui: &mut Gui,
    q: &wgpu::Queue,
    o: &Options,
    mouse: Option<(f32, f32)>,
    time: f64,
) {
    screen.poll();
    screen.layout(gui);
    gui.next_layer();
    widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
    gui.next_layer();
    let w = gui.width as i32;
    widgets::centered(gui, q, &screen.title, w / 2, 12, 0xffff_ffff);
    let lang = std::sync::Arc::clone(&gui.lang);
    let ms = (time * 1000.0) as u64;
    let hide_ip = o.b("hideServerAddress");
    let focused = screen.panel.focus.is_none();
    let Multiplayer { panel, rows, lan_worlds, pinger, next_id, slots, .. } = screen;
    let nrows = rows.len();
    let mut tooltip: Option<Tooltip> = None;
    let selected = panel.list.as_ref().and_then(|l| l.selected);
    panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
        let hovered = mouse.and_then(|m| l.entry_under(m));
        for i in 0..l.entries.len() {
            let Some((left, top, rw, rh)) = l.entry_rect(i) else { continue };
            if top + rh < l.y || top > l.y + l.h {
                continue;
            }
            if selected == Some(i) {
                let c = if focused { 0xffff_ffff } else { GRAY };
                gui.fill(left as f32, top as f32, (left + rw) as f32, (top + rh) as f32, c);
                gui.fill(
                    (left + 1) as f32,
                    (top + 1) as f32,
                    (left + rw - 1) as f32,
                    (top + rh - 1) as f32,
                    0xff00_0000,
                );
            }
            let (x, y, lc) = (left + 2, top + 2, rw - 4);
            if let Some(row) = rows.get_mut(i) {
                // Query at first draw (§ 3.1).
                if matches!(row.ping, Ping::Initial) {
                    row.id = *next_id;
                    *next_id += 1;
                    pinger.submit(row.id, &row.entry.ip);
                    row.ping = Ping::Pinging;
                }
                let hover = hovered == Some(i);
                let tip = draw_server(gui, q, &lang, row, i, slots, (x, y, lc), ms, hover, mouse);
                if tip.is_some() {
                    tooltip = tip;
                }
                if hover {
                    draw_hover_icons(gui, (x, y), i, i + 1 >= nrows, mouse);
                }
            } else if i == rows.len() {
                // LAN search: centered label and animated dots.
                let label = chars(&tr(&lang, "lanServer.scanning", &[]));
                let cx = x + lc / 2;
                let tw = rc_text::width(gui, &label);
                gui.text(q, &label, (cx - tw / 2) as f32, (y + 2) as f32, 0xffff_ffff, true, 0);
                let frames = ["O o o", "o O o", "o o O", "o O o"];
                let dots = chars(frames[(ms / 300 % 4) as usize]);
                let dw = rc_text::width(gui, &dots);
                gui.text(q, &dots, (cx - dw / 2) as f32, (y + 17) as f32, GRAY, true, 0);
            } else if let Some((motd, addr)) = lan_worlds.get(i - rows.len() - 1) {
                let title = chars(&tr(&lang, "lanServer.title", &[]));
                gui.text(q, &title, (x + 35) as f32, (y + 1) as f32, 0xffff_ffff, true, 0);
                gui.text(q, &chars(motd), (x + 35) as f32, (y + 12) as f32, GRAY, true, 0);
                let a = if hide_ip { tr(&lang, "selectServer.hiddenAddress", &[]) } else { addr.clone() };
                gui.text(q, &chars(&a), (x + 35) as f32, (y + 23) as f32, GRAY, true, 0);
            }
        }
    });
    if let Some((lines, rect)) = tooltip {
        gui.next_layer();
        widgets::draw_tooltip(gui, q, &lines, rect, mouse);
    }
}

/// Content of a server row (§ 2.3); returns the possible hovered tooltip.
#[allow(clippy::too_many_arguments)]
fn draw_server(
    gui: &mut Gui,
    q: &wgpu::Queue,
    lang: &rc_text::Lang,
    row: &mut Row,
    i: usize,
    slots: &mut [Option<Vec<u8>>],
    (x, y, lc): (i32, i32, i32),
    ms: u64,
    hover: bool,
    mouse: Option<(f32, f32)>,
) -> Option<Tooltip> {
    // Icon: atlas slot rewritten if the PNG changed; refused if not 64 × 64.
    let mut icon = "tex/misc/unknown_server".to_owned();
    if let Some(png) = &row.entry.icon {
        match Favicon::from_png(png.clone()).and_then(|f| f.rgba()) {
            Some(rgba) if i < slots.len() => {
                let name = format!("dyn/server_icon/{i}");
                if slots[i].as_deref() != Some(&png[..]) && gui.write_sprite(q, &name, (64, 64), &rgba) {
                    slots[i] = Some(png.clone());
                }
                if slots[i].is_some() {
                    icon = name;
                }
            }
            Some(_) => {}
            None => row.entry.icon = None,
        }
    }
    gui.sprite(&icon, x as f32, y as f32, 32.0, 32.0, 0xffff_ffff);
    gui.text(q, &chars(&row.entry.name), (x + 35) as f32, (y + 1) as f32, 0xffff_ffff, true, 0);
    // MOTD (2 lines at most) or failure message in red.
    let motd = match &row.ping {
        Ping::Done(s) => styled(&s.motd, lang),
        Ping::Failed(k) => styled(&colored(&tr(lang, k, &[]), 0xff_0000), lang),
        _ => Vec::new(),
    };
    for (k, line) in rc_text::wrap(gui, &motd, (lc - 34) as f32).iter().take(2).enumerate() {
        gui.text(q, line, (x + 35) as f32, (y + 12 + 9 * k as i32) as f32, GRAY, true, 0);
    }
    // State: 10 × 8 icon and text to its left.
    let (sprite, tip, text): (String, Option<String>, Vec<StyledChar>) = match &row.ping {
        Ping::Initial | Ping::Pinging => {
            let mut k = ((ms / 100) as usize + 2 * i) & 7;
            if k > 4 {
                k = 8 - k;
            }
            (format!("server_list/pinging_{}", k + 1), Some(tr(lang, "multiplayer.status.pinging", &[])), Vec::new())
        }
        Ping::Failed(_) => {
            ("server_list/unreachable".into(), Some(tr(lang, "multiplayer.status.no_connection", &[])), Vec::new())
        }
        Ping::Done(s) if !s.compatible => (
            "server_list/incompatible".into(),
            Some(tr(lang, "multiplayer.status.incompatible", &[])),
            styled(&colored(&s.version, 0xff_5555), lang),
        ),
        Ping::Done(s) => {
            let n = match s.latency {
                0..150 => 5,
                150..300 => 4,
                300..600 => 3,
                600..1000 => 2,
                _ => 1,
            };
            let text = match s.players {
                Some((on, max)) => {
                    let c = Component::translate(
                        "multiplayer.status.player_count",
                        vec![
                            rc_text::Arg::Component(colored(&on.to_string(), 0xaa_aaaa)),
                            rc_text::Arg::Component(colored(&max.to_string(), 0xaa_aaaa)),
                        ],
                    );
                    let mut c = c;
                    c.style.color = Some(0x55_5555);
                    styled(&c, lang)
                }
                None => styled(&colored("???", 0x55_5555), lang),
            };
            (format!("server_list/ping_{n}"), Some(tr(lang, "multiplayer.status.ping", &[s.latency.to_string()])), text)
        }
    };
    let ix = x + lc - 15;
    gui.sprite(&sprite, ix as f32, y as f32, 10.0, 8.0, 0xffff_ffff);
    let tw = rc_text::width(gui, &text);
    let tx = ix - tw - 5;
    if !text.is_empty() {
        gui.text(q, &text, tx as f32, (y + 1) as f32, GRAY, true, 0);
    }
    let m = mouse.filter(|_| hover)?;
    // Game tooltip zones: bounds inclusive, in whole pixels (10 × 8 icon → 11 × 9).
    let (mx, my) = (m.0.floor() as i32, m.1.floor() as i32);
    let inside = |rx: i32, ry: i32, rw: i32, rh: i32| mx >= rx && mx <= rx + rw && my >= ry && my <= ry + rh;
    if inside(ix, y, 10, 8) {
        return tip.map(|t| (vec![chars(&t)], (ix, y, 10, 8)));
    }
    if let Ping::Done(s) = &row.ping
        && inside(tx, y, tw, 8)
        && !s.sample.is_empty()
    {
        let lines = s
            .sample
            .iter()
            .map(|n| match n.strip_prefix('\u{1}') {
                Some("anonymous") => chars(&tr(lang, "multiplayer.status.anonymous_player", &[])),
                Some(more) => {
                    let k = more.strip_prefix("more:").unwrap_or("0").to_owned();
                    chars(&tr(lang, "multiplayer.status.and_more", &[k]))
                }
                None => chars(n),
            })
            .collect();
        return Some((lines, (tx, y, tw, 9)));
    }
    None
}
