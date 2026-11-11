//! Active effects to the right of the inventory and the creative inventory (`inventories.md`): a
//! 32 px tile per effect (icon, name with level, duration), compact (icon only, tooltip on
//! hover) when space is lacking; the HUD display disappears while the screen shows them.

use std::cmp::Ordering;

use rc_text::{Component, StyledChar};

use crate::gui::Gui;
use crate::hud_extra::Effect;
use crate::kept_text::misc_screen_caches_on;

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
    Component::text(crate::tooltips::tick_duration(e.duration, tickrate.max(1.0)))
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

/// Text cut to `max` px with "...". `one_pass`: the cut found in a single pass over the advances
/// ([`rc_text::fit`], same sums and same rounding); otherwise the prefix measured again for
/// each character added (old path).
fn clip(gui: &Gui, chars: &[StyledChar], max: i32, one_pass: bool) -> Vec<StyledChar> {
    let dots = rc_text::styled_chars(&rc_text::flatten(&Component::text("..."), &gui.lang));
    let room = max - rc_text::width(gui, &dots);
    let mut out: Vec<StyledChar> = Vec::new();
    if one_pass {
        out.extend_from_slice(&chars[..rc_text::fit(gui, chars, room)]);
    } else {
        for c in chars {
            out.push(c.clone());
            if rc_text::width(gui, &out) > room {
                out.pop();
                break;
            }
        }
    }
    out.extend(dots);
    out
}

/// Whole seconds left of an effect at the server's tick rate, `None` if infinite: all that
/// its duration text depends on.
fn seconds_left(e: &Effect, tickrate: f32) -> Option<i32> {
    (e.duration >= 0).then(|| (e.duration as f32 / tickrate.max(1.0)).floor() as i32)
}

/// Texts of the tile of one effect at one level, kept between frames.
struct TileTexts {
    id: i32,
    amplifier: i32,
    label: Vec<StyledChar>,
    label_width: i32,
    /// Sprite of the icon (effect known to the registry).
    icon: Option<String>,
    /// Duration text, its width, and the seconds left it shows (`None` inside: infinite).
    time: Vec<StyledChar>,
    time_width: i32,
    time_of: Option<Option<i32>>,
    /// Label cut for a tile leaving `.0` px to the text.
    clipped: Option<(i32, Vec<StyledChar>)>,
}

impl TileTexts {
    fn new(gui: &Gui, e: &Effect, names: &[String]) -> TileTexts {
        let label = rc_text::styled_chars(&rc_text::flatten(&name(e, names), &gui.lang));
        let icon = usize::try_from(e.id)
            .ok()
            .and_then(|i| names.get(i))
            .map(|n| format!("mob_effect/{}", n.strip_prefix("minecraft:").unwrap_or(n)));
        TileTexts {
            id: e.id,
            amplifier: e.amplifier,
            label_width: rc_text::width(gui, &label),
            label,
            icon,
            time: Vec::new(),
            time_width: 0,
            time_of: None,
            clipped: None,
        }
    }

    /// Duration text brought up to date: built again only when the seconds shown change (true).
    fn update_time(&mut self, gui: &Gui, e: &Effect, tickrate: f32) -> bool {
        let shown = seconds_left(e, tickrate);
        if self.time_of == Some(shown) {
            return false;
        }
        self.time = rc_text::styled_chars(&rc_text::flatten(&duration(e, tickrate), &gui.lang));
        self.time_width = rc_text::width(gui, &self.time);
        self.time_of = Some(shown);
        true
    }

    /// Label cut to `room` px, cut once per room.
    fn clipped_label(&mut self, gui: &Gui, room: i32, one_pass: bool) -> &[StyledChar] {
        if self.clipped.as_ref().is_none_or(|(cut_for, _)| *cut_for != room) {
            self.clipped = Some((room, clip(gui, &self.label, room, one_pass)));
        }
        self.clipped.as_ref().map_or(&[][..], |(_, chars)| &chars[..])
    }
}

/// Tile texts kept between frames: valid for one language, one set of fonts and one effect
/// registry.
#[derive(Default)]
struct KeptTiles {
    built_for: Option<(crate::gui::TextContext, usize, usize)>,
    tiles: Vec<TileTexts>,
    /// Labels and duration texts built so far (a kept text is built once).
    texts_built: usize,
}

impl KeptTiles {
    /// Forgets the tiles built from another language, other fonts or another registry, and
    /// every tile when nothing is kept (`keep` off: each frame builds its texts again).
    fn forget_stale(&mut self, gui: &Gui, names: &[String], keep: bool) {
        let built_for = Some((gui.text_context(), names.as_ptr() as usize, names.len()));
        // 64 tiles: more than the effects of the game at every level a player meets.
        if !keep || self.built_for != built_for || self.tiles.len() > 64 {
            self.tiles.clear();
        }
        self.built_for = built_for;
    }

    /// Place in `tiles` of the tile of effect `e`, built on first use.
    fn tile_index(&mut self, gui: &Gui, e: &Effect, names: &[String]) -> usize {
        let known = self.tiles.iter().position(|t| t.id == e.id && t.amplifier == e.amplifier);
        known.unwrap_or_else(|| {
            self.tiles.push(TileTexts::new(gui, e, names));
            self.texts_built += 1;
            self.tiles.len() - 1
        })
    }
}

thread_local! {
    static KEPT_TILES: std::cell::RefCell<KeptTiles> = std::cell::RefCell::new(KeptTiles::default());
}

/// Draws the tiles at `(x0, top)`; `mouse` in GUI pixels.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    effects: &[Effect],
    names: &[String],
    tickrate: f32,
    column: (i32, i32),
    top: i32,
    mouse: Option<(f32, f32)>,
) {
    KEPT_TILES.with(|kept| {
        let kept = &mut *kept.borrow_mut();
        draw_tiles(gui, queue, (effects, names, tickrate), column, top, mouse, (kept, misc_screen_caches_on()));
    });
}

/// [`draw`] with its tile texts in `kept`; `keep` off: every text built again on every frame and
/// labels cut by re-measuring (old path).
fn draw_tiles(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    (effects, names, tickrate): (&[Effect], &[String], f32),
    (x0, avail): (i32, i32),
    top: i32,
    mouse: Option<(f32, f32)>,
    (kept, keep): (&mut KeptTiles, bool),
) {
    if effects.is_empty() {
        return;
    }
    kept.forget_stale(gui, names, keep);
    let max_w = if avail >= 120 { avail - 7 } else { 32 };
    let step = if effects.len() > 5 { 132 / (effects.len() as i32 - 1) } else { 33 };
    let mut list: Vec<&Effect> = effects.iter().collect();
    list.sort_by(|a, b| natural(a, b));
    gui.next_layer();
    let mut tooltip = None;
    let mut y0 = top;
    for e in list {
        let ambient = e.flags & 1 != 0;
        let index = kept.tile_index(gui, e, names);
        kept.texts_built += usize::from(kept.tiles[index].update_time(gui, e, tickrate));
        let tile = &mut kept.tiles[index];
        let (lw, tw) = (tile.label_width, tile.time_width);
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
            let shown = if clipped { tile.clipped_label(gui, room, keep) } else { &tile.label[..] };
            gui.text(queue, shown, (x0 + 32) as f32, (y0 + 7) as f32, 0xffff_ffff, true, 0);
            gui.text(queue, &tile.time, (x0 + 32) as f32, (y0 + 16) as f32, 0xff80_8080, true, 0);
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
            tooltip = Some(vec![tile.label.clone(), tile.time.clone()]);
        }
        if let Some(icon) = &tile.icon {
            gui.sprite(icon, (x0 + 7) as f32, (y0 + 7) as f32, 18.0, 18.0, 0xffff_ffff);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests_opt;

    /// Six effects at frame `frame` of the test, their durations running down by 7 ticks a
    /// frame: plain, level IV beyond 32,147 ticks, ambient with a level, infinite, a level without
    /// numeral, ambient beyond 32,147 ticks.
    fn six_effects(frame: i32) -> Vec<Effect> {
        let left = |ticks: i32| ticks - 7 * frame;
        vec![
            Effect { id: 0, amplifier: 0, duration: left(1_500), flags: 0 },
            Effect { id: 1, amplifier: 3, duration: left(40_000), flags: 0 },
            Effect { id: 9, amplifier: 1, duration: left(2_400), flags: 1 },
            Effect { id: 2, amplifier: 0, duration: -1, flags: 0 },
            Effect { id: 4, amplifier: 12, duration: left(72_000 + 2_000), flags: 0 },
            Effect { id: 14, amplifier: 0, duration: left(35_000), flags: 1 },
        ]
    }

    /// Tiles drawn from kept texts against texts built on every frame: layers equal to the byte
    /// over 120 frames of six effects running down, in a wide column, a narrow one (labels cut)
    /// and a compact one (icons, tooltip under the mouse), with another language (frame 40),
    /// other fonts (frame 70) and another tick rate (frame 95). The cut found in one pass equals
    /// the re-measured one for every room. The kept texts really are kept. Without an installed
    /// game or a GPU: skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
    #[test]
    fn kept_tiles_draw_the_layers_of_rebuilt_tiles() {
        let Some(data) = rc_testdata::version_data() else { return };
        let Some(e) = tests_opt::env() else { return };
        let names = data.registry("minecraft:mob_effect").expect("effect registry").entries.clone();
        let mut paths = [false, true].map(|keep| (keep, KeptTiles::default(), tests_opt::gui(&e, tests_opt::ALL)));
        let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
        let french = r#"{"effect.minecraft.speed": "Rapidité", "effect.duration.infinite": "infini"}"#;
        let mut seen = hashbrown::HashSet::new();
        for frame in 0..120 {
            let effects = six_effects(frame);
            let avail = [200, 125, 60][frame as usize % 3];
            let mouse = (frame % 4 != 0).then_some((310.0, 28.0 + (frame % 6) as f32 * 26.0));
            let tickrate = if frame < 95 { 20.0 } else { 10.0 };
            let mut layers = Vec::new();
            for (keep, kept, gui) in &mut paths {
                match frame {
                    40 => gui.lang = std::sync::Arc::new(rc_text::Lang::load(&base, Some(french.as_bytes()), None)),
                    70 => {
                        let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
                        let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
                        gui.set_fonts(crate::resources::FONT_IDS.iter().map(load).collect());
                    }
                    _ => {}
                }
                gui.begin(1600, 900);
                draw_tiles(gui, &e.queue, (&effects, &names, tickrate), (300, avail), 20, mouse, (kept, *keep));
                layers.push(tests_opt::layer_bytes(gui));
            }
            let first = layers[0].iter().zip(&layers[1]).position(|(a, b)| a != b);
            assert!(layers[0].len() == layers[1].len() && first.is_none(), "frame {frame}: layers differ (byte {first:?})");
            seen.insert(layers.swap_remove(0));
        }
        assert!(seen.len() > 60, "only {} distinct frames", seen.len());
        let (rebuilt, kept) = (paths[0].1.texts_built, paths[1].1.texts_built);
        assert_eq!(rebuilt, 120 * 12, "old path: a label and a duration text per tile and per frame");
        assert!(kept > 30 && kept * 3 < rebuilt, "kept texts: {kept} built against {rebuilt}");
        let gui = &paths[1].2;
        for label in ["Night Vision IV", "Résistance au feu", "iiiiiiiiiiiiiiiiiiii", "WWWWWWWW", ""] {
            let chars = crate::gui::plain(label);
            for room in -5..130 {
                assert_eq!(clip(gui, &chars, room, true), clip(gui, &chars, room, false), "{label:?} cut to {room}");
            }
        }
    }

    /// Micro-bench of the effect tiles (loaded machine possible): six effects in a narrow column
    /// (labels cut), texts built on every frame against kept texts; time and bytes allocated per
    /// frame. Main thread by nature. Without a GPU: skipped.
    /// `cargo test --release -p rc-app bench_effect_tiles -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench"]
    fn bench_effect_tiles() {
        let Some(data) = rc_testdata::version_data() else { return };
        let Some(e) = tests_opt::env() else { return };
        let names = data.registry("minecraft:mob_effect").expect("effect registry").entries.clone();
        let effects = six_effects(0);
        let mut gui = tests_opt::gui(&e, tests_opt::ALL);
        for pass in 0..2 {
            for (label, avail) in [("wide", 200), ("labels cut", 125)] {
                for (name, keep) in [("rebuilt", false), ("kept", true)] {
                    let mut kept = KeptTiles::default();
                    let mut once = |gui: &mut Gui| {
                        gui.begin(1600, 900);
                        draw_tiles(gui, &e.queue, (&effects, &names, 20.0), (300, avail), 20, None, (&mut kept, keep));
                    };
                    (0..200).for_each(|_| once(&mut gui));
                    let frames = 10_000;
                    let t = std::time::Instant::now();
                    (0..frames).for_each(|_| once(&mut gui));
                    let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                    let bytes = crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut gui))) / 100;
                    println!(
                        "effect tiles, pass {pass}, {label:<10} texts {name:<8}: {per_frame:>6.2} µs, {bytes:>6} bytes allocated per frame"
                    );
                }
            }
        }
    }
}
