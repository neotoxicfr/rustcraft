//! Player resources read from the source stack (`docs/specs/resource-packs.md` § 1, 6):
//! startup loading and full in-game reload (packs chosen or imposed by the
//! server, F3+T). The cache of baked resources (`rc_assets::cache`) has one entry per stack.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use rc_mesh::blocks::Tables;

use crate::{entity_render, front, gui, memstats, menus, panorama, particles, sky, weather_render};

/// Everything drawn from the resources (outside version data).
pub(crate) struct Loaded {
    pub assets: Arc<rc_assets::Assets>,
    pub models: Arc<rc_assets::BlockModels>,
    /// Images of the block textures, sent to the GPU then freed.
    pub textures: Vec<Vec<rc_assets::texture::Image>>,
    pub tables: Arc<Tables>,
    pub colormaps: rc_assets::texture::Colormaps,
    pub sky_assets: sky::SkyAssets,
    pub panorama_assets: Option<panorama::PanoramaAssets>,
    pub splashes: Vec<String>,
    pub gui_assets: gui::GuiAssets,
    pub entity_textures: entity_render::EntityTextures,
    pub particle_assets: particles::ParticleAssets,
    pub weather_images: weather_render::WeatherImages,
    pub languages: Vec<menus::Language>,
}

/// Fonts loaded (the first is the default font).
pub(crate) const FONT_IDS: [&str; 4] = ["minecraft:default", "minecraft:uniform", "minecraft:alt", "minecraft:illageralt"];

/// Packs removed by [`load_recovering`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dropped {
    Nothing,
    /// Player choices removed (server packs kept).
    Player,
    /// All packs removed.
    All,
}

/// Like the game: if loading fails (default font), the player's packs are removed
/// and loading restarts with the server's, then without any pack.
pub(crate) fn load_recovering(
    data: &Arc<rc_data::VersionData>,
    server: &[PathBuf],
    player: &[PathBuf],
    lang: &str,
    on_assets: &mut dyn FnMut(&Arc<rc_assets::Assets>),
) -> (Loaded, Dropped) {
    recover(server, player, |packs| load(data, packs, lang, &mut *on_assets))
}

/// Stacks tried by [`load_recovering`]; `load` returns `None` on failure (never without a pack).
fn recover<T>(server: &[PathBuf], player: &[PathBuf], mut load: impl FnMut(&[PathBuf]) -> Option<T>) -> (T, Dropped) {
    let all: Vec<PathBuf> = server.iter().chain(player).cloned().collect();
    if let Some(l) = load(&all) {
        return (l, Dropped::Nothing);
    }
    if !player.is_empty()
        && !server.is_empty()
        && let Some(l) = load(server)
    {
        return (l, Dropped::Player);
    }
    let dropped = if server.is_empty() { Dropped::Player } else { Dropped::All };
    match load(&[]) {
        Some(l) => (l, dropped),
        None => unreachable!("without a pack, the fonts load with fallback"),
    }
}

/// True (default): block models baked on a separate thread during the fonts.
/// `RUSTCRAFT_OPT_RESPAR=0`: fonts then models, on the calling thread (old path).
fn respar() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_RESPAR") != Some("0")
}

/// Fonts of [`FONT_IDS`] present; `None` if the default font fails (without a pack, fallback
/// tolerated).
fn load_fonts(
    asset_cache: &rc_assets::cache::AssetCache,
    assets: &rc_assets::Assets,
    packs: &[PathBuf],
) -> Option<Vec<(String, rc_assets::font::Font)>> {
    let fonts = asset_cache
        .get("fonts", assets, |a| {
            rc_assets::font::Fonts(
                FONT_IDS
                    .iter()
                    .filter_map(|id| {
                        let f = rc_assets::font::Font::try_load(a, id, Default::default(), packs.is_empty())?;
                        Some(((*id).to_owned(), f))
                    })
                    .collect(),
            )
        })
        .0;
    if !fonts.iter().any(|f| f.0 == FONT_IDS[0]) {
        eprintln!("resources: default font failed with {} pack(s)", packs.len());
        return None;
    }
    Some(fonts)
}

/// Fonts (`fonts`, `None` on failure), then `retained` (retained stack), then the models
/// (`blocks`). `par`: `blocks` runs on a separate thread from the start and `retained` is called
/// before it ends; on failure, its baking is awaited then discarded. In both cases, `retained`
/// is never called for a failing stack and the result does not depend on `par`.
fn fonts_then_blocks<F, B: Send>(
    par: bool,
    fonts: impl FnOnce() -> Option<F>,
    retained: impl FnOnce(),
    blocks: impl FnOnce() -> B + Send,
) -> Option<(F, B)> {
    if !par {
        let f = fonts()?;
        retained();
        return Some((f, blocks()));
    }
    std::thread::scope(|s| {
        let cook = std::thread::Builder::new()
            .name("block-baking".into())
            .spawn_scoped(s, blocks)
            .unwrap_or_else(|e| panic!("block baking thread: {e}"));
        let f = fonts();
        if f.is_some() {
            retained();
        }
        let b = cook.join().unwrap_or_else(|p| std::panic::resume_unwind(p));
        Some((f?, b))
    })
}

/// Check of the fonts `fonts` (`None`: stack failing) framed by the hook `on_assets`:
/// called before it, hence also for a stack that then fails (`early`, audio engine built in
/// parallel; the call of the next stack replaces it), or after it on the only stack that passes.
fn fonts_with_hook<A: ?Sized, F>(
    early: bool,
    assets: &A,
    on_assets: &mut dyn FnMut(&A),
    fonts: impl FnOnce() -> Option<F>,
) -> Option<F> {
    if early {
        on_assets(assets);
    }
    let f = fonts()?;
    if !early {
        on_assets(assets);
    }
    Some(f)
}

/// Loads the resources with `packs` (decreasing priority) above the jar; `lang`: code of
/// the chosen language. Exits the process if the jar is unreadable. `None`: the default font
/// fails with these packs (never without a pack). `on_assets` receives the stack before baking (audio
/// engine started in the background, `audio_slot.rs`): from the index, before the fonts, hence also for a
/// stack that then fails (`RUSTCRAFT_OPT_AUDIOEARLY`, the next call replaces it); or after the
/// fonts, never for a failing stack (`=0`).
pub(crate) fn load(
    data: &Arc<rc_data::VersionData>,
    packs: &[PathBuf],
    lang: &str,
    on_assets: &mut dyn FnMut(&Arc<rc_assets::Assets>),
) -> Option<Loaded> {
    // Player resources: packs then the version's jar, models baked in parallel.
    let t = Instant::now();
    let install = rc_data::locate("26.3").unwrap_or_else(|e| {
        eprintln!("installation: {e}");
        std::process::exit(1);
    });
    // Index of the integrated server's jar (`rc_data::jar`) built on a background thread during the
    // loading of the resources: the first world opening finds it ready.
    // `RUSTCRAFT_OPT_JARPRELOAD=0`: built at the first opening (old timing).
    if rc_data::jar::enabled() && rc_data::env_str!("RUSTCRAFT_OPT_JARPRELOAD") != Some("0") {
        let jar = install.client_jar.clone();
        let _ = std::thread::Builder::new().name("index-jar".into()).spawn(move || {
            let _ = rc_data::jar::index(&jar);
        });
    }
    let index = install.asset_index();
    let index = index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()));
    // Cache of baked resources (`rc_assets::cache`): valid, the jar is not opened.
    let cache_dir = rc_data::cache_root().join("26.3");
    let key_extra: Vec<std::path::PathBuf> =
        std::env::current_exe().ok().into_iter().chain([cache_dir.join("data.bin")]).collect();
    let (assets, asset_cache) =
        rc_assets::cache::open_assets(&cache_dir, "26.3", &install.client_jar, packs, index, &key_extra)
            .unwrap_or_else(|e| {
                eprintln!("resources: {e}");
                std::process::exit(1);
            });
    eprintln!("resources: {} pack(s), cache {}", packs.len(), if asset_cache.hot() { "valid" } else { "to write" });
    memstats::step("resource index");
    let assets = Arc::new(assets);
    crate::boot::mark("resources indexed");
    // Fonts first: a failing default font fails the whole loading, like the
    // game (a failing font is absent from the list). Without a pack, fallback tolerated. The block
    // models bake meanwhile (`RUSTCRAFT_OPT_RESPAR`), handed to the cache only if the
    // stack is retained. Audio engine started in the background by the hook: before the fonts
    // (`RUSTCRAFT_OPT_AUDIOEARLY`) or right after, on the retained stack only (`=0`).
    let early = crate::audio_slot::opt().early;
    let (fonts, (mut models, blocks_bytes)) = fonts_then_blocks(
        respar(),
        || fonts_with_hook(early, &assets, on_assets, || load_fonts(&asset_cache, &assets, packs)),
        || {},
        || asset_cache.get_unkept("blocks", &assets, |a| rc_assets::build(a, data)),
    )?;
    asset_cache.keep("blocks", blocks_bytes);
    memstats::step("model baking");
    crate::boot::mark("resources: block models");
    // Texture images: sent to the GPU then freed (the mesher reads only the faces).
    let textures = std::mem::take(&mut models.textures);
    let models = Arc::new(models);
    eprintln!(
        "resources: {} baked faces, {} textures, in {:.0} ms",
        models.quads.len(),
        textures.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    let tables = Arc::new(Tables::new(data, models.clone()));
    let colormaps = rc_assets::texture::Colormaps::load(&assets);
    // Sky: sun then the 8 moon phases, cloud map.
    let celestial_names = [
        "sun",
        "moon/full_moon",
        "moon/waning_gibbous",
        "moon/third_quarter",
        "moon/waning_crescent",
        "moon/new_moon",
        "moon/waxing_crescent",
        "moon/first_quarter",
        "moon/waxing_gibbous",
        "end_flash",
    ];
    let celestial: Vec<_> = celestial_names
        .iter()
        .filter_map(|n| rc_assets::texture::load(&assets, &format!("minecraft:environment/celestial/{n}")))
        .map(rc_assets::texture::Image::first_frame)
        .collect();
    let sky_assets = sky::SkyAssets {
        celestial: if celestial.len() == celestial_names.len() { celestial } else { Vec::new() },
        clouds: rc_assets::texture::load(&assets, "minecraft:environment/clouds"),
        end_sky: rc_assets::texture::load(&assets, "minecraft:environment/end_sky"),
    };
    let panorama_assets = panorama::PanoramaAssets::load(&assets);
    let splashes = assets
        .read("assets/minecraft/texts/splashes.txt")
        .map_or_else(Vec::new, |b| front::splash_lines(&String::from_utf8_lossy(&b)));
    // Interface: sprite atlas, fonts, translations (English, then renames).
    let t = Instant::now();
    let languages = assets.read("assets/pack.mcmeta").map(|b| menus::languages(&b)).unwrap_or_default();
    let bidirectional = languages.iter().any(|l| l.code == lang && l.bidirectional);
    let chosen = match lang {
        "en_us" => None,
        l => assets.read(&format!("assets/minecraft/lang/{l}.json")),
    };
    let mut lang = rc_text::Lang::load(
        &assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default(),
        chosen.as_deref(),
        assets.read("assets/minecraft/lang/deprecated.json").as_deref(),
    );
    lang.bidirectional = bidirectional;
    let vignette = rc_assets::texture::load(&assets, "minecraft:misc/vignette");
    let weather_images = weather_render::WeatherImages {
        rain: rc_assets::texture::load(&assets, "minecraft:environment/rain"),
        snow: rc_assets::texture::load(&assets, "minecraft:environment/snow"),
        forcefield: rc_assets::texture::load(&assets, "minecraft:misc/forcefield"),
    };
    let t_items = Instant::now();
    let items = asset_cache
        .get("items", &assets, |a| rc_assets::item::ItemModels::load(a, data.items.iter().map(|i| i.model.as_str())));
    eprintln!(
        "items: {} definitions, {} models, {} sprites, in {:.0} ms",
        items.len(),
        items.baked.len(),
        items.sprites.len(),
        t_items.elapsed().as_secs_f64() * 1e3
    );
    #[cfg(feature = "memstats")]
    eprintln!(
        "  item quads {:.1} MB (quads of {} B), images {:.1} MB",
        items.baked.iter().map(|b| b.heap_bytes()).sum::<usize>() as f64 / 1_048_576.0,
        std::mem::size_of::<rc_assets::item::ItemQuad>(),
        items.sprites.iter().chain(items.frames.iter().flatten()).map(|i| i.rgba.capacity()).sum::<usize>() as f64
            / 1_048_576.0
    );
    crate::boot::mark("resources: sky, fonts, language, items");
    let t_ent = Instant::now();
    let mut entity_textures = entity_render::EntityTextures::load(&assets, &items.sprites)
        .with_item_animations(&items.animations, &items.frames)
        .with_blocks(models.clone(), &textures);
    eprintln!(
        "entities: {} textures (loaded on demand), {} model layers, in {:.0} ms",
        entity_textures.sources.len(),
        data.entity_models.len(),
        t_ent.elapsed().as_secs_f64() * 1e3
    );
    crate::boot::mark("resources: entities");
    let particle_assets = asset_cache
        .get("particules", &assets, |a| particles::ParticleAssets::load(a, data, &models, &items));
    crate::boot::mark("resources: particles");
    let atlas = asset_cache.get("interface", &assets, rc_assets::gui::GuiAtlas::load);
    // Special models of the icons (`item_special.rs`): entity textures already in the cache.
    let special = crate::item_special::SpecialAssets::load(&assets, data);
    crate::boot::mark("resources: interface");
    // The same models drawn in the world (hand, ground, frames): layers and names only.
    entity_textures.special = special.geometry_only();
    let gui_assets = gui::GuiAssets { atlas, fonts, lang, vignette, items: Some(items), special };
    eprintln!(
        "interface: {} sprites, {} translations, in {:.0} ms",
        gui_assets.atlas.sprites.len(),
        gui_assets.lang.map.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    // Cold: textures read later on demand and all the languages, then write.
    let mut lazy_images = entity_textures.asset_paths();
    let mut langs = assets.list("assets/minecraft/lang/", ".json");
    // Read at renderer creation or in game: cracks, camera overlays (including those of
    // equippable items), block entity effects.
    let tex = |p: &str| {
        let (ns, path) = p.split_once(':').unwrap_or(("minecraft", p));
        format!("assets/{ns}/textures/{path}.png")
    };
    lazy_images.extend((0..10).map(|i| tex(&format!("block/destroy_stage_{i}"))));
    lazy_images.extend((0..10).map(|i| tex(&format!("block/destroy_oit_stage_{i}"))));
    // Window icons, set at its creation (`launch.rs`).
    lazy_images.extend(crate::launch::ICONS.map(String::from));
    // Animated textures of the entity sheet, read at startup (`entity_render.rs`): their
    // meta (the images follow `asset_paths`).
    langs.extend(entity_render::animated_paths().map(|p| p.1));
    // Backgrounds of the advancement tabs, read according to the server's packet (`advancements_screen.rs`).
    lazy_images.extend(
        ["stone", "adventure", "husbandry", "nether", "end"].map(|b| tex(&format!("gui/advancements/backgrounds/{b}"))),
    );
    let mut overlays: Vec<String> =
        ["misc/underwater", "misc/powder_snow_outline", "misc/nausea", "misc/pumpkinblur", "misc/spyglass_scope"]
            .map(tex)
            .to_vec();
    overlays.extend(data.items.iter().filter(|i| !i.equip_overlay.is_empty()).map(|i| tex(&i.equip_overlay)));
    overlays.sort_unstable();
    overlays.dedup();
    langs.extend(overlays.iter().map(|p| format!("{p}.mcmeta")));
    lazy_images.extend(overlays);
    langs.extend(
        [
            "entity/beacon/beacon_beam",
            "entity/end_portal/end_gateway_beam",
            "entity/end_portal/end_sky",
            "entity/end_portal/end_portal",
        ]
        .map(tex),
    );
    // Texts of the end screen (`win_screen.rs`).
    langs.extend(["end.txt", "credits.json", "postcredits.txt"].map(|n| format!("assets/minecraft/texts/{n}")));
    // Periodic reminders per country (`toasts.rs`), read at renderer creation.
    langs.push("assets/minecraft/regional_compliancies.json".to_owned());
    // Data read when connecting to a server at the known packet: damage types (injury
    // sound, `entity_events.rs`) and dialog boxes (`dialog.rs`).
    langs.extend(assets.list("data/minecraft/damage_type/", ".json"));
    // Goat horn instruments (sound, duration, range: `interact/instrument.rs`).
    langs.extend(assets.list("data/minecraft/instrument/", ".json"));
    langs.extend(assets.list("data/minecraft/dialog/", ".json"));
    // Sound table (read on the "audio-engine" thread, maybe not there yet): its two lists and
    // its `sounds.json` put in the snapshot for sure, cold only.
    if !asset_cache.hot() {
        let _ = assets.list("assets/", ".ogg");
        langs.extend(assets.list("assets/", "/sounds.json").into_iter().filter(|p| p.matches('/').count() == 2));
    }
    asset_cache.finish(assets.clone(), lazy_images, langs);
    crate::boot::mark("resources: end");
    Some(Loaded {
        assets,
        models,
        textures,
        tables,
        colormaps,
        sky_assets,
        panorama_assets,
        splashes,
        gui_assets,
        entity_textures,
        particle_assets,
        weather_images,
        languages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_stack_drops_player_packs_then_server_packs() {
        let p = |s: &str| PathBuf::from(s);
        // Failure as soon as a "broken" pack is in the stack.
        let fake = |broken: &'static str| move |packs: &[PathBuf]| (!packs.iter().any(|x| x == broken)).then_some(packs.len());
        assert_eq!(recover(&[p("srv")], &[p("player")], fake("none")), (2, Dropped::Nothing));
        // Player pack broken: server packs kept.
        assert_eq!(recover(&[p("srv")], &[p("player")], fake("player")), (1, Dropped::Player));
        // Server pack broken: no pack any more.
        assert_eq!(recover(&[p("srv")], &[p("player")], fake("srv")), (0, Dropped::All));
        assert_eq!(recover(&[], &[p("player")], fake("player")), (0, Dropped::Player));
    }

    /// `RUSTCRAFT_OPT_RESPAR` 0 and 1: same retained stack, same `Dropped`, same models, and the
    /// retained stack alone signalled (never a failing stack).
    #[test]
    fn respar_same_stack_and_drop_in_both_paths() {
        let p = |s: &str| PathBuf::from(s);
        let cases: [(&[PathBuf], &[PathBuf], &str); 4] = [
            (&[p("srv")], &[p("player")], "none"),
            (&[p("srv")], &[p("player")], "player"),
            (&[p("srv")], &[p("player")], "srv"),
            (&[], &[p("player")], "player"),
        ];
        for (server, player, broken) in cases {
            let run = |par: bool| {
                let mut retained = Vec::new();
                let got = recover(server, player, |packs| {
                    fonts_then_blocks(
                        par,
                        || (!packs.iter().any(|x| x == broken)).then_some(packs.len()),
                        || retained.push(packs.to_vec()),
                        || packs.len() * 10,
                    )
                });
                (got, retained)
            };
            let (serial, par) = (run(false), run(true));
            assert_eq!(serial, par, "stack {broken} broken");
            let ((fonts, blocks), _) = serial.0;
            assert_eq!(blocks, fonts * 10, "models of another stack");
            assert_eq!(serial.1.len(), 1, "failing stack signalled");
            assert_eq!(serial.1[0].len(), fonts);
        }
    }

    /// Parallel path: the retained stack is signalled before the end of the baking (the audio engine
    /// starts earlier); on failure, never signalled and the baking is awaited all the same.
    #[test]
    fn respar_signals_before_blocks_finish() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let r = fonts_then_blocks(
            true,
            || Some(1),
            move || tx.send(()).expect("signal"),
            move || rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok(),
        );
        assert_eq!(r, Some((1, true)), "baking finished before the retained stack's signal");
        let done = std::sync::atomic::AtomicBool::new(false);
        let r = fonts_then_blocks(
            true,
            || None::<u8>,
            || panic!("failing stack signalled"),
            || done.store(true, std::sync::atomic::Ordering::SeqCst),
        );
        assert_eq!(r, None);
        assert!(done.load(std::sync::atomic::Ordering::SeqCst), "baking not awaited");
    }

    /// Real resources, cold cache: fonts and models identical to the bit between the two
    /// paths (encoded sections), and their expected sizes.
    #[test]
    fn respar_real_assets_identical_cold() {
        let install = rc_data::locate("26.3").expect("installation 26.3 absent: test impossible");
        let data = rc_data::load_or_prepare("26.3", |_| {}).expect("26.3 data absent: test impossible");
        let index = install.asset_index();
        let index = index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()));
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        let run = |par: bool| {
            let dir = root.join(format!("i01-respar-{par}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let (assets, cache) =
                rc_assets::cache::open_assets(&dir, "26.3", &install.client_jar, &[], index, &[]).expect("resources");
            assert!(!cache.hot(), "cold cache expected");
            let t = Instant::now();
            let (fonts, (models, bytes)) = fonts_then_blocks(
                par,
                || load_fonts(&cache, &assets, &[]),
                || {},
                || cache.get_unkept("blocks", &assets, |a| rc_assets::build(a, &data)),
            )
            .expect("default font");
            eprintln!(
                "RESPAR={}: fonts and models in {:.0} ms (loaded machine)",
                par as u8,
                t.elapsed().as_secs_f64() * 1e3
            );
            let glyphs: Vec<usize> =
                fonts.iter().map(|(_, f)| (0..=0x10_FFFFu32).filter(|&c| f.advance(c).is_some()).count()).collect();
            let fonts = rc_assets::cache::encode(&rc_assets::font::Fonts(fonts));
            let models_enc = rc_assets::cache::encode(&models);
            assert_eq!(bytes.as_deref(), Some(&models_enc[..]), "bytes handed over differ from the models");
            drop(cache);
            let _ = std::fs::remove_dir_all(&dir);
            (models.quads.len(), glyphs, rc_assets::cache::fnv(&fonts), rc_assets::cache::fnv(&models_enc))
        };
        // Two alternating passes: the first read of the jar does not weigh on a single path.
        let (serial, par, _, _) = (run(false), run(true), run(false), run(true));
        eprintln!("faces {}, glyphs per font {:?}", serial.0, serial.1);
        assert_eq!(serial, par, "fonts or models changed by the parallel path");
        assert_eq!((serial.0, &serial.1[..]), (80_011, &[114_432, 114_432, 54, 69][..]), "unexpected faces or glyphs");
    }

    /// Moment of the resource hook (audio engine) around the font check, on the stacks
    /// tried by the fallback: before the fonts on every stack, failed ones included (`AUDIOEARLY`),
    /// or once after them on the retained one (`=0`).
    #[test]
    fn resource_hook_runs_before_fonts_on_every_stack_tried_when_early() {
        let p = |s: &str| PathBuf::from(s);
        let name = |packs: &[PathBuf]| packs.iter().map(|x| x.display().to_string()).collect::<Vec<_>>().join("+");
        let calls = |early: bool| {
            let log = std::cell::RefCell::new(Vec::new());
            let mut hook = |packs: &[PathBuf]| log.borrow_mut().push(format!("hook({})", name(packs)));
            let (n, dropped) = recover(&[p("srv")], &[p("player")], |packs| {
                fonts_with_hook(early, packs, &mut hook, || {
                    let ok = !packs.iter().any(|x| x == "srv");
                    log.borrow_mut().push(format!("{}({})", if ok { "fonts" } else { "failure" }, name(packs)));
                    ok.then_some(packs.len())
                })
            });
            assert_eq!((n, dropped), (0, Dropped::All));
            log.into_inner()
        };
        assert_eq!(
            calls(true),
            ["hook(srv+player)", "failure(srv+player)", "hook(srv)", "failure(srv)", "hook()", "fonts()"]
        );
        assert_eq!(calls(false), ["failure(srv+player)", "failure(srv)", "fonts()", "hook()"]);
    }
}
