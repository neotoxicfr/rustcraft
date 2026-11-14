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

/// The game's fonts drawn by the interface tests (the first is the default font).
#[cfg(test)]
pub(crate) const FONT_IDS: [&str; 4] = ["minecraft:default", "minecraft:uniform", "minecraft:alt", "minecraft:illageralt"];

/// Every font the stack defines, like the game: each `font/*.json` of every namespace (sub-folders
/// included, `include/space` for one), as `namespace:path`.
pub(crate) fn font_ids(assets: &rc_assets::Assets) -> Vec<String> {
    let files = assets.list("assets/*/font/", ".json");
    files
        .iter()
        .filter_map(|p| {
            let (ns, file) = p.strip_prefix("assets/")?.split_once("/font/")?;
            Some(format!("{ns}:{}", file.strip_suffix(".json")?))
        })
        .collect()
}

/// Packs removed by [`load_recovering`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dropped {
    Nothing,
    /// Player choices removed (server packs kept).
    Player,
    /// All packs removed.
    All,
}

/// Language of a resource load: the chosen code, and the table in use that a reload keeps when the
/// new translations fail, like the game (`None` at startup: the game's built-in English then).
pub(crate) struct LanguageToLoad<'a> {
    pub code: &'a str,
    pub in_use: Option<&'a Arc<rc_text::Lang>>,
}

/// Like the game: if loading fails (default font), the player's packs are removed
/// and loading restarts with the server's, then without any pack. `progress` follows the steps
/// of the stack being tried (loading screen bar).
pub(crate) fn load_recovering(
    data: &Arc<rc_data::VersionData>,
    server: &[PathBuf],
    player: &[PathBuf],
    lang: &LanguageToLoad<'_>,
    progress: &LoadProgress,
    on_assets: &mut dyn FnMut(&Arc<rc_assets::Assets>),
) -> (Loaded, Dropped) {
    recover(server, player, |packs| load(data, packs, lang, progress, &mut *on_assets))
}

/// Steps of a resource load (the startup milestones of [`load`]).
const LOAD_STEPS: u32 = 7;

/// Steps of the resource load in progress that are done, read by the loading screen: its bar
/// follows the real work instead of an assumed duration.
#[derive(Debug, Default)]
pub(crate) struct LoadProgress(std::sync::atomic::AtomicU32);

impl LoadProgress {
    /// Share of the steps done, 0 to 1.
    pub(crate) fn fraction(&self) -> f32 {
        let done = self.0.load(std::sync::atomic::Ordering::Relaxed).min(LOAD_STEPS);
        done as f32 / LOAD_STEPS as f32
    }

    fn restart(&self) {
        self.0.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// One step done: startup milestone `name` and one more share of the bar.
    fn step(&self, name: &str) {
        crate::boot::mark(name);
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
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

/// Every font of the stack ([`font_ids`]) that loads; `None` if the default font fails (without a
/// pack, fallback tolerated).
fn load_fonts(
    asset_cache: &rc_assets::cache::AssetCache,
    assets: &rc_assets::Assets,
    packs: &[PathBuf],
) -> Option<Vec<(String, rc_assets::font::Font)>> {
    let fonts = asset_cache
        .get("fonts", assets, |a| {
            let lenient = packs.is_empty();
            rc_assets::font::Fonts(rc_assets::font::Font::try_load_all(a, &font_ids(a), Default::default(), lenient))
        })
        .0;
    if !fonts.iter().any(|f| f.0 == "minecraft:default") {
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

/// Loads the resources with `packs` (decreasing priority) above the jar; `lang`: the chosen
/// language and the table kept if its translations fail. Exits the process if the jar is unreadable. `None`: the default font
/// fails with these packs (never without a pack). `on_assets` receives the stack before baking (audio
/// engine started in the background, `audio_slot.rs`): from the index, before the fonts, hence also for a
/// stack that then fails (`RUSTCRAFT_OPT_AUDIOEARLY`, the next call replaces it); or after the
/// fonts, never for a failing stack (`=0`). `progress` counts the steps done (restarted here).
pub(crate) fn load(
    data: &Arc<rc_data::VersionData>,
    packs: &[PathBuf],
    lang: &LanguageToLoad<'_>,
    progress: &LoadProgress,
    on_assets: &mut dyn FnMut(&Arc<rc_assets::Assets>),
) -> Option<Loaded> {
    // Player resources: packs then the version's jar, models baked in parallel.
    let t = Instant::now();
    progress.restart();
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
    progress.step("resources indexed");
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
    progress.step("resources: block models");
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
    // Interface: sprite atlas, fonts, translations (English, then the chosen language, then renames).
    let t = Instant::now();
    let languages = stack_languages(&assets);
    let lang = translations_or_kept(&assets, lang, &languages);
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
    progress.step("resources: sky, fonts, language, items");
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
    progress.step("resources: entities");
    let particle_assets = asset_cache
        .get("particules", &assets, |a| particles::ParticleAssets::load(a, data, &models, &items));
    progress.step("resources: particles");
    let atlas = asset_cache.get("interface", &assets, rc_assets::gui::GuiAtlas::load);
    // Special models of the icons (`item_special.rs`): entity textures already in the cache.
    let special = crate::item_special::SpecialAssets::load(&assets, data);
    progress.step("resources: interface");
    // The same models drawn in the world (hand, ground, frames): layers and names only.
    entity_textures.special = special.geometry_only();
    let gui_assets = gui::GuiAssets { atlas, fonts, lang, vignette, items: Some(items), special };
    eprintln!(
        "interface: {} sprites, {} translations, in {:.0} ms",
        gui_assets.atlas.sprites.len(),
        gui_assets.lang.map.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    // Cold: textures read later on demand and all the languages (every pack's file of every
    // namespace, read at a language change), then write.
    let mut lazy_images = entity_textures.asset_paths();
    let mut langs: Vec<String> =
        assets.list(LANGUAGE_FOLDERS, ".json").iter().map(|p| rc_assets::source::stacked_read_key(p)).collect();
    // Read at renderer creation or in game: cracks, camera overlays (including those of
    // equippable items), block entity effects.
    let tex = |p: &str| {
        let (ns, path) = p.split_once(':').unwrap_or(("minecraft", p));
        format!("assets/{ns}/textures/{path}.png")
    };
    let cracks: Vec<String> = (0..10)
        .map(|i| tex(&format!("block/destroy_stage_{i}")))
        .chain((0..10).map(|i| tex(&format!("block/destroy_oit_stage_{i}"))))
        .collect();
    // Read as still textures (`texture::load`), which reads their `.mcmeta` too.
    langs.extend(cracks.iter().map(|p| format!("{p}.mcmeta")));
    lazy_images.extend(cracks);
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
    // the `sounds.json` of every pack put in the snapshot for sure, cold only.
    if !asset_cache.hot() {
        let _ = assets.list("assets/", ".ogg");
        let indexes = assets.list("assets/", "/sounds.json").into_iter().filter(|p| p.matches('/').count() == 2);
        langs.extend(indexes.map(|p| rc_assets::source::stacked_read_key(&p)));
    }
    asset_cache.finish(assets.clone(), lazy_images, langs);
    progress.step("resources: end");
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

/// The `lang` folder of every namespace (listing prefix).
const LANGUAGE_FOLDERS: &str = "assets/*/lang/";

/// Why the game keeps its previous translations instead of the new ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranslationError {
    /// A language of the stack has a file in no namespace of any pack (the game also shows an
    /// error screen, not reproduced).
    NoFile(String),
    /// A translation file is not a JSON object of plain values.
    Malformed,
}

/// Languages the packs declare, like the game: the game's own list first, then the `language`
/// section of each pack's `pack.mcmeta` from the bottom pack up, a code keeping its first
/// declaration.
pub(crate) fn stack_languages(assets: &rc_assets::Assets) -> Vec<menus::Language> {
    let game = assets.read("assets/pack.mcmeta");
    let packs = assets.read_all("pack.mcmeta");
    menus::languages_of_stack(game.iter().chain(packs.iter().rev()).map(Vec::as_slice))
}

/// Translations of `code`, like the game: English then `code` (only when a pack declares it), each
/// from the file of every namespace in every pack, bottom pack first, a later key winning; then the
/// renames of the game's own `deprecated.json`. Every place that loads a language goes through here.
pub(crate) fn load_translations(
    assets: &rc_assets::Assets,
    code: &str,
    languages: &[menus::Language],
) -> Result<rc_text::Lang, TranslationError> {
    let chosen = languages.iter().find(|l| l.code == code && code != "en_us");
    let codes = std::iter::once("en_us").chain(chosen.map(|l| l.code.as_str()));
    let listed = assets.list(LANGUAGE_FOLDERS, ".json");
    let mut files = Vec::new();
    for c in codes {
        let before = files.len();
        for ns in namespaces_in_game_order(language_namespaces(&listed, c)) {
            // One file per pack, top first: merged from the bottom up.
            files.extend(assets.read_all(&format!("assets/{ns}/lang/{c}.json")).into_iter().rev());
        }
        if files.len() == before {
            return Err(TranslationError::NoFile(c.to_owned()));
        }
    }
    let deprecated = builtin_file(assets, "assets/minecraft/lang/deprecated.json");
    let mut lang = rc_text::Lang::load_stack(files.iter().map(Vec::as_slice), deprecated.as_deref())
        .ok_or(TranslationError::Malformed)?;
    lang.bidirectional = chosen.is_some_and(|l| l.bidirectional);
    Ok(lang)
}

/// Translations of a resource load. When they fail, the game keeps the table it was showing: the
/// one in use on a reload, its built-in English at startup.
fn translations_or_kept(
    assets: &rc_assets::Assets,
    lang: &LanguageToLoad<'_>,
    languages: &[menus::Language],
) -> Arc<rc_text::Lang> {
    match load_translations(assets, lang.code, languages) {
        Ok(table) => Arc::new(table),
        Err(e) => {
            eprintln!("resources: translations not loaded ({e:?}), previous table kept");
            lang.in_use.cloned().unwrap_or_else(|| Arc::new(builtin_translations(assets)))
        }
    }
}

/// The game's built-in English (its own `en_us.json` and renames, no pack): what it shows when the
/// first load of the translations fails.
pub(crate) fn builtin_translations(assets: &rc_assets::Assets) -> rc_text::Lang {
    let english = builtin_file(assets, "assets/minecraft/lang/en_us.json").unwrap_or_default();
    rc_text::Lang::load(&english, None, builtin_file(assets, "assets/minecraft/lang/deprecated.json").as_deref())
}

/// The file of the bottom of the stack (the game's own resources), whatever the packs above hold.
fn builtin_file(assets: &rc_assets::Assets, path: &str) -> Option<Vec<u8>> {
    assets.read_all(path).pop()
}

/// Namespaces holding `assets/<namespace>/lang/<code>.json` among the listed language files.
fn language_namespaces<'a>(listed: &'a [String], code: &str) -> Vec<&'a str> {
    listed
        .iter()
        .filter_map(|p| {
            let (ns, file) = p.strip_prefix("assets/")?.split_once("/lang/")?;
            (file.strip_suffix(".json") == Some(code) && !ns.contains('/')).then_some(ns)
        })
        .collect()
}

/// Namespaces in the order the game's resource manager walks them, which decides between two
/// namespaces that define the same key: a Java hash map, here with its 16 first buckets (exact up
/// to 12 namespaces in the stack); names sharing a bucket in name order.
fn namespaces_in_game_order(mut namespaces: Vec<&str>) -> Vec<&str> {
    let bucket = |ns: &str| {
        let h = crate::servers::java_string_hash(ns) as u32;
        (h ^ (h >> 16)) & 15
    };
    namespaces.sort_by_key(|ns| (bucket(ns), *ns));
    namespaces
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pack folder for the translation tests, under the build queue's temporary folder.
    fn pack(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-lang-{name}-{}", std::process::id())));
        for (p, body) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().expect("parent")).expect("folder");
            std::fs::write(f, body).expect("write");
        }
        root
    }

    /// Stack of `packs` (top first) over a fake game holding English, French and the renames.
    fn stack_over_game(name: &str, packs: &[&[(&str, &str)]]) -> rc_assets::Assets {
        use rc_assets::source::Source;
        let game = pack(
            &format!("{name}-game"),
            &[
                ("assets/minecraft/lang/en_us.json", r#"{"menu.quit":"Quit Game","menu.options":"Options...","old.key":"Old"}"#),
                ("assets/minecraft/lang/fr_fr.json", r#"{"menu.quit":"Quitter le jeu"}"#),
                ("assets/minecraft/lang/deprecated.json", r#"{"removed":[],"renamed":{"old.key":"new.key"}}"#),
                (
                    "assets/pack.mcmeta",
                    r#"{"language":{"en_us":{"name":"English","region":"United States"},"fr_fr":{"name":"Français","region":"France"}}}"#,
                ),
            ],
        );
        let mut sources: Vec<Source> = packs
            .iter()
            .enumerate()
            .map(|(i, files)| Source::dir(&pack(&format!("{name}-{i}"), files)).as_pack())
            .collect();
        sources.push(Source::dir(&game));
        rc_assets::Assets::new(sources)
    }

    /// A pack's partial language file changes its keys only: the rest stays (the old code
    /// replaced the whole file and showed raw keys), a file of another namespace counts too, and the
    /// renames come from the game's own list.
    #[test]
    fn a_partial_pack_language_file_changes_only_its_keys() {
        let a = stack_over_game(
            "partial",
            &[&[
                ("pack.mcmeta", r#"{"pack":{"description":"p","pack_format":69}}"#),
                ("assets/minecraft/lang/en_us.json", r#"{"menu.quit":"Leave"}"#),
                ("assets/mymod/lang/en_us.json", r#"{"mymod.title":"My Mod","count":3}"#),
                ("assets/minecraft/lang/deprecated.json", r#"{"removed":["menu.options"],"renamed":{}}"#),
            ]],
        );
        let languages = stack_languages(&a);
        let lang = load_translations(&a, "en_us", &languages).expect("translations");
        let get = |k: &str| lang.map.get(k).map(String::as_str);
        assert_eq!(get("menu.quit"), Some("Leave"));
        assert_eq!(get("menu.options"), Some("Options..."), "kept from the game (pack renames ignored)");
        assert_eq!(get("mymod.title"), Some("My Mod"), "other namespace");
        assert_eq!(get("count"), Some("3"), "number kept as its text");
        assert_eq!((get("new.key"), get("old.key")), (Some("Old"), None), "game renames applied");
    }

    /// The chosen language goes over English, pack over game; a language a pack declares appears
    /// in the list; an undeclared code loads English alone; a malformed file keeps the previous
    /// table; a declared language without any file is refused.
    #[test]
    fn chosen_language_and_pack_declared_languages() {
        let a = stack_over_game(
            "chosen",
            &[
                &[
                    (
                        "pack.mcmeta",
                        r#"{"pack":{"description":"top"},"language":{"fr_fr":{"name":"Faux","region":"X"},"tlh_aa":{"name":"Klingon","region":"Qo'noS"}}}"#,
                    ),
                    ("assets/minecraft/lang/fr_fr.json", r#"{"menu.options":"Réglages"}"#),
                ],
                &[(
                    "pack.mcmeta",
                    r#"{"pack":{"description":"bottom"},"language":{"xx_yy":{"name":"Empty","region":"None"},"bad":{"name":""}}}"#,
                )],
            ],
        );
        let languages = stack_languages(&a);
        let codes: Vec<&str> = languages.iter().map(|l| l.code.as_str()).collect();
        assert_eq!(codes, ["en_us", "fr_fr", "tlh_aa"], "a malformed section declares nothing");
        assert_eq!(languages[1].name, "Français", "the game's declaration comes first");
        let fr = load_translations(&a, "fr_fr", &languages).expect("French");
        assert_eq!(fr.map.get("menu.quit").map(String::as_str), Some("Quitter le jeu"));
        assert_eq!(fr.map.get("menu.options").map(String::as_str), Some("Réglages"));
        let unknown = load_translations(&a, "zz_zz", &languages).expect("English alone");
        assert_eq!(unknown.map.get("menu.quit").map(String::as_str), Some("Quit Game"));
        assert_eq!(
            load_translations(&a, "tlh_aa", &languages).err(),
            Some(TranslationError::NoFile("tlh_aa".into()))
        );
        let broken = stack_over_game("broken", &[&[("assets/minecraft/lang/en_us.json", r#"{"menu.quit":["x"]}"#)]]);
        let broken_load = load_translations(&broken, "en_us", &stack_languages(&broken));
        assert_eq!(broken_load.err(), Some(TranslationError::Malformed));
        assert_eq!(builtin_translations(&broken).map.get("menu.quit").map(String::as_str), Some("Quit Game"));
    }

    /// Translations that fail on a reload keep the table in use, like the game: a broken pack
    /// language file, or a chosen language left without any file (the code fell back to the
    /// built-in English, without the translations of any pack). Only the first load falls back.
    #[test]
    fn failing_translations_keep_the_table_in_use() {
        let broken = stack_over_game(
            "kept",
            &[&[
                (
                    "pack.mcmeta",
                    r#"{"pack":{"description":"p"},"language":{"tlh_aa":{"name":"Klingon","region":"Q"}}}"#,
                ),
                ("assets/minecraft/lang/fr_fr.json", r#"{"menu.quit":["x"]}"#),
            ]],
        );
        let languages = stack_languages(&broken);
        let in_use = Arc::new(rc_text::Lang::load(br#"{"menu.quit":"Quitter (pack A)"}"#, None, None));
        for code in ["fr_fr", "tlh_aa"] {
            let reload = LanguageToLoad { code, in_use: Some(&in_use) };
            assert!(Arc::ptr_eq(&translations_or_kept(&broken, &reload, &languages), &in_use), "{code}");
        }
        let startup = translations_or_kept(&broken, &LanguageToLoad { code: "fr_fr", in_use: None }, &languages);
        assert_eq!(startup.map.get("menu.quit").map(String::as_str), Some("Quit Game"));
    }

    /// A server pack's own font (`pack:icons`) and the game's include fonts are listed and loaded
    /// with the others; only four fixed fonts were loaded before, the rest drawn as missing glyphs.
    #[test]
    fn every_font_of_every_namespace_is_loaded() {
        use rc_assets::source::Source;
        let space = |a: u32| format!(r#"{{"providers":[{{"type":"space","advances":{{"a":{a}}}}}]}}"#);
        let (default, include, icons) = (space(5), space(6), space(9));
        let reference = r#"{"providers":[{"type":"reference","id":"minecraft:include/space"}]}"#;
        let game = pack(
            "fonts-game",
            &[
                ("assets/minecraft/font/default.json", default.as_str()),
                ("assets/minecraft/font/include/space.json", include.as_str()),
                ("assets/minecraft/font/uses_include.json", reference),
                ("assets/minecraft/textures/font/not_a_font.json", "{}"),
            ],
        );
        let server = pack("fonts-server", &[("assets/pack/font/icons.json", icons.as_str())]);
        let a = rc_assets::Assets::new(vec![Source::dir(&server).as_pack(), Source::dir(&game)]);
        let ids = font_ids(&a);
        assert_eq!(ids, ["minecraft:default", "minecraft:include/space", "minecraft:uses_include", "pack:icons"]);
        let fonts = rc_assets::font::Font::try_load_all(&a, &ids, Default::default(), false);
        let advance = |id: &str| fonts.iter().find(|f| f.0 == id).and_then(|f| f.1.advance('a' as u32)).map(|a| a.0);
        assert_eq!(advance("pack:icons"), Some(9.0));
        assert_eq!(advance("minecraft:include/space"), Some(6.0));
        assert_eq!(advance("minecraft:uses_include"), Some(6.0), "reference shared with the font itself");
        assert_eq!(advance("minecraft:default"), Some(5.0));
    }

    /// Two namespaces defining the same key: the game's hash order decides. Buckets computed
    /// apart from the Java string hash: minecraft 0, custom 7, pack 13, mymod 14 (name order
    /// would put `custom` before `minecraft`, and `minecraft`'s value would win).
    #[test]
    fn namespaces_follow_the_game_hash_order() {
        let order = namespaces_in_game_order(vec!["mymod", "pack", "custom", "minecraft"]);
        assert_eq!(order, ["minecraft", "custom", "pack", "mymod"]);
        let a = stack_over_game(
            "namespaces",
            &[&[
                ("assets/minecraft/lang/en_us.json", r#"{"shared.key":"from minecraft"}"#),
                ("assets/custom/lang/en_us.json", r#"{"shared.key":"from custom"}"#),
            ]],
        );
        let lang = load_translations(&a, "en_us", &stack_languages(&a)).expect("translations");
        assert_eq!(lang.map.get("shared.key").map(String::as_str), Some("from custom"));
    }

    /// Cost of loading every font (cold, no cache): the four fonts loaded before, every font one by
    /// one, and every font with the providers shared between them; with the size of their cache
    /// section. Figures only (machine-dependent).
    /// `cargo test --release -p rc-app --bin rustcraft bench_font_loading -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench: 26.3 installation"]
    fn bench_font_loading() {
        use rc_assets::font::{Font, Fonts};
        let install = rc_data::locate("26.3").expect("26.3 installation");
        let index = install.asset_index();
        let idx = index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()));
        let assets = rc_assets::Assets::for_install(&install.client_jar, &[], idx).expect("resources");
        let four: Vec<String> = FONT_IDS.iter().map(|s| (*s).to_owned()).collect();
        let all = font_ids(&assets);
        let one_by_one = |ids: &[String]| -> Vec<(String, Font)> {
            let load = |id: &String| Some((id.clone(), Font::try_load(&assets, id, Default::default(), false)?));
            ids.iter().filter_map(load).collect()
        };
        let mut times: [Vec<f64>; 3] = Default::default();
        let mut sizes = [0usize; 3];
        for _ in 0..7 {
            for (k, times) in times.iter_mut().enumerate() {
                let t = Instant::now();
                let fonts = match k {
                    0 => one_by_one(&four),
                    1 => one_by_one(&all),
                    _ => Font::try_load_all(&assets, &all, Default::default(), false),
                };
                times.push(t.elapsed().as_secs_f64() * 1e3);
                sizes[k] = rc_assets::cache::encode(&Fonts(fonts)).len();
            }
        }
        let names = ["4 fonts", "all, one by one", "all, shared providers"];
        for (name, (t, size)) in names.iter().zip(times.iter_mut().zip(sizes)) {
            t.sort_by(f64::total_cmp);
            let mb = size as f64 / 1_048_576.0;
            eprintln!("{name}: median {:.1} ms (min {:.1}), cache section {mb:.2} MB", t[3], t[0]);
        }
    }

    /// The game's resources of a 26.3 installation (jar and asset index), `None` without one.
    fn game_assets() -> Option<rc_assets::Assets> {
        let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
        let index = rc_testdata::need(install.asset_index(), "26.3 asset index (languages)")?;
        let index = Some((index.0.as_path(), index.1.as_path()));
        let assets = rc_assets::Assets::for_install(&install.client_jar, &[], index);
        rc_testdata::need(assets.ok(), "26.3 game resources")
    }

    /// Every language file of the game reads the same through the tolerant reader as through the
    /// strict JSON reader used before: the game's own tables do not change.
    #[test]
    fn game_language_files_read_as_with_strict_json() {
        let Some(assets) = game_assets() else { return };
        let files = assets.list("assets/minecraft/lang/", ".json");
        let languages: Vec<&String> = files.iter().filter(|p| !p.ends_with("/deprecated.json")).collect();
        assert!(languages.len() > 100, "{} language files", languages.len());
        for path in languages {
            let bytes = assets.read(path).expect("language file");
            let strict = rc_text::Lang::load(&bytes, None, None);
            let tolerant = rc_text::Lang::load_stack([bytes.as_slice()], None).expect(path);
            assert_eq!(tolerant.map, strict.map, "{path}");
        }
    }

    /// Cost of reading the game's English and French files into a table: the strict JSON reader of
    /// before (whole document, then its strings) against the tolerant reader. Figures only.
    /// `cargo test --release -p rc-app --bin rustcraft bench_translation_reading -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench: 26.3 installation"]
    fn bench_translation_reading() {
        let assets = game_assets().expect("26.3 installation");
        let files: Vec<Vec<u8>> = ["en_us", "fr_fr"]
            .iter()
            .map(|c| assets.read(&format!("assets/minecraft/lang/{c}.json")).expect("language file"))
            .collect();
        let strict = || rc_text::Lang::load(&files[0], Some(&files[1]), None).map.len();
        let tolerant = || rc_text::Lang::load_stack(files.iter().map(Vec::as_slice), None).expect("tables").map.len();
        let mut times: [Vec<f64>; 2] = Default::default();
        for _ in 0..7 {
            for (k, times) in times.iter_mut().enumerate() {
                let t = Instant::now();
                std::hint::black_box(if k == 0 { strict() } else { tolerant() });
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
        }
        for (name, t) in ["strict JSON (before)", "tolerant reader"].iter().zip(&mut times) {
            t.sort_by(f64::total_cmp);
            eprintln!("{name}: median {:.2} ms (min {:.2})", t[3], t[0]);
        }
    }

    /// The loading screen's bar: share of the load's steps done, full after the last one, never
    /// past it, back to empty for the next stack tried.
    #[test]
    fn load_progress_counts_the_steps() {
        let p = LoadProgress::default();
        for k in 1..=LOAD_STEPS {
            p.step("test step");
            assert_eq!(p.fraction(), k as f32 / LOAD_STEPS as f32);
        }
        p.step("extra");
        assert_eq!(p.fraction(), 1.0);
        p.restart();
        assert_eq!(p.fraction(), 0.0);
    }

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
    /// starts earlier); on failure, never signalled and the baking is awaited all the same. The
    /// baking waits for the signal itself; the 120 s limit is only an anti-hang guard (a signal
    /// sent after the baking would otherwise wait forever).
    #[test]
    fn respar_signals_before_blocks_finish() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let r = fonts_then_blocks(
            true,
            || Some(1),
            move || tx.send(()).expect("signal"),
            move || rx.recv_timeout(std::time::Duration::from_secs(120)).is_ok(),
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
        let Some(index) = rc_testdata::need(install.asset_index(), "26.3 sound files and fonts (asset index)") else { return };
        let index = Some(index);
        let index = index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()));
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        let run = |par: bool| {
            let dir = rc_testdata::scratch(root.join(format!("i01-respar-{par}-{}", std::process::id())));
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
            let glyphs: Vec<(String, usize)> = fonts
                .iter()
                .map(|(id, f)| (id.clone(), (0..=0x10_FFFFu32).filter(|&c| f.advance(c).is_some()).count()))
                .collect();
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
        assert_eq!(serial.0, 80_011, "unexpected faces");
        // Every font of the game, include fonts too (only the first four were loaded before).
        let want = [
            ("minecraft:alt", 54),
            ("minecraft:default", 114_432),
            ("minecraft:illageralt", 69),
            ("minecraft:include/default", 2_413),
            ("minecraft:include/space", 2),
            ("minecraft:include/unifont", 114_432),
            ("minecraft:include/unifont_pua", 11_688),
            ("minecraft:uniform", 114_432),
        ];
        let got: Vec<(&str, usize)> = serial.1.iter().map(|(id, n)| (id.as_str(), *n)).collect();
        assert_eq!(got, want, "unexpected fonts or glyphs");
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
