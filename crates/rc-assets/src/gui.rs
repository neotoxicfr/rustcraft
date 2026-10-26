//! Atlas of the interface sprites (`textures/gui/sprites/**.png`) and their scaling
//! (`.mcmeta`: `gui.scaling` section — stretch, tile, nine-slice).

use hashbrown::HashMap;

use serde_json::Value;

use crate::Assets;
use crate::texture::Image;

/// Slots `dyn/server_icon/<i>` (64 × 64, empty) reserved for the server icons.
pub const SERVER_ICON_SLOTS: usize = 32;

/// Slots `dyn/pack_icon/<i>` (64 × 64, empty) for the icons of the packs screen.
pub const PACK_ICON_SLOTS: usize = 32;
/// Slots `dyn/adv_bg/<i>` (16 × 16) reserved for the backgrounds of the progress tabs, read according to
/// the server's packet (one per tab, 26 at most).
pub const ADV_BG_SLOTS: usize = 26;
/// Slots `dyn/face/<i>` (8 × 8): player faces of the spectator menu (one page).
pub const FACE_SLOTS: usize = 9;
/// Slots `dyn/tab_face/<i>` (8 × 8): heads of the players of the Tab list (80 at most).
pub const TAB_FACE_SLOTS: usize = 80;
/// Slots `dyn/text_obj/<i>` (64 × 64): atlas sprites drawn in text, copied on
/// demand (64 = multiple of the common sizes: nearest neighbour keeps the image exact).
pub const TEXT_OBJECT_SLOTS: usize = 32;
pub const TEXT_OBJECT_SIZE: u32 = 64;
/// Slots `dyn/text_head/<i>` (8 × 8): player heads drawn in text (80, like
/// the Tab list: one head per displayed player; a slot drawn in the frame is never
/// reused, the extra head takes the missing texture).
pub const TEXT_HEAD_SLOTS: usize = 80;
/// Missing texture (16 × 16): sprite of an item not found in text.
pub const MISSING_SPRITE: &str = "rustcraft/missing";

/// Atlases the game knows (full identifiers): an object from another atlas is drawn with
/// the missing glyph, that of an absent sprite with the missing texture.
pub const KNOWN_ATLASES: [&str; 12] = [
    "minecraft:banner_patterns",
    "minecraft:blocks",
    "minecraft:items",
    "minecraft:chests",
    "minecraft:decorated_pot",
    "minecraft:gui",
    "minecraft:map_decorations",
    "minecraft:paintings",
    "minecraft:particles",
    "minecraft:shield_patterns",
    "minecraft:shulker_boxes",
    "minecraft:celestials",
];

/// Image (first frame of an animation) of sprite `sprite` of atlas `atlas`, from the
/// sources of `atlases/<atlas>.json`: directory (`source` + `prefix`), single file (`resource`,
/// `sprite`) and palette permutations; the other kinds of source are not read.
pub fn atlas_sprite(assets: &Assets, atlas: &str, sprite: &str) -> Option<Image> {
    let split = |id: &str| -> (String, String) {
        match id.split_once(':') {
            Some((ns, p)) => (ns.to_owned(), p.to_owned()),
            None => ("minecraft".to_owned(), id.to_owned()),
        }
    };
    let (ans, apath) = split(atlas);
    let (sns, spath) = split(sprite);
    let def: Value = serde_json::from_slice(&assets.read(&format!("assets/{ans}/atlases/{apath}.json"))?).ok()?;
    let image = |ns: &str, path: &str| assets.image(&format!("assets/{ns}/textures/{path}.png")).map(Image::first_frame);
    for s in def["sources"].as_array()? {
        let kind = s["type"].as_str().map(|t| t.strip_prefix("minecraft:").unwrap_or(t));
        let found = match kind {
            Some("directory") => {
                let (prefix, source) = (s["prefix"].as_str().unwrap_or(""), s["source"].as_str().unwrap_or(""));
                spath.strip_prefix(prefix).and_then(|rel| image(&sns, &format!("{source}/{rel}")))
            }
            Some("single") => {
                let Some(res) = s["resource"].as_str() else { continue };
                let name = s["sprite"].as_str().unwrap_or(res);
                let (n_ns, n_path) = split(name);
                if (n_ns.as_str(), n_path.as_str()) != (sns.as_str(), spath.as_str()) {
                    continue;
                }
                let (r_ns, r_path) = split(res);
                image(&r_ns, &r_path)
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    crate::palette::Permutations::of_atlas(assets, &apath).iter().find_map(|p| p.resolve(assets, sprite))
}

/// Scaling of a sprite drawn at a size different from its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scaling {
    Stretch,
    Tile { width: u32, height: u32 },
    NineSlice { width: u32, height: u32, border: [u32; 4], stretch_inner: bool },
}

/// Position of a sprite in the atlas (pixels) and its scaling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub scaling: Scaling,
}

pub struct GuiAtlas {
    pub image: Image,
    /// Name without namespace or extension, e.g. `hud/crosshair`.
    pub sprites: HashMap<String, Sprite>,
}

impl crate::cache::Codec for Scaling {
    fn enc(&self, w: &mut crate::cache::W) {
        match self {
            Scaling::Stretch => 0u8.enc(w),
            Scaling::Tile { width, height } => {
                1u8.enc(w);
                width.enc(w);
                height.enc(w);
            }
            Scaling::NineSlice { width, height, border, stretch_inner } => {
                2u8.enc(w);
                width.enc(w);
                height.enc(w);
                border.enc(w);
                stretch_inner.enc(w);
            }
        }
    }
    fn dec(r: &mut crate::cache::R<'_>) -> Option<Self> {
        use crate::cache::Codec;
        Some(match u8::dec(r)? {
            0 => Scaling::Stretch,
            1 => Scaling::Tile { width: u32::dec(r)?, height: u32::dec(r)? },
            2 => Scaling::NineSlice {
                width: u32::dec(r)?,
                height: u32::dec(r)?,
                border: Codec::dec(r)?,
                stretch_inner: bool::dec(r)?,
            },
            _ => return None,
        })
    }
}
crate::codec_struct!(Sprite { x, y, width, height, scaling });
crate::codec_struct!(GuiAtlas { image, sprites });

fn scaling(meta: Option<&Value>) -> Scaling {
    let Some(s) = meta.and_then(|m| m.get("gui")).and_then(|g| g.get("scaling")) else { return Scaling::Stretch };
    let n = |k: &str| s.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    match s.get("type").and_then(Value::as_str) {
        Some("tile") => Scaling::Tile { width: n("width"), height: n("height") },
        Some("nine_slice") => {
            let border = match s.get("border") {
                Some(Value::Object(b)) => {
                    let g = |k: &str| b.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
                    [g("left"), g("top"), g("right"), g("bottom")]
                }
                Some(v) => [v.as_u64().unwrap_or(0) as u32; 4],
                None => [0; 4],
            };
            let stretch_inner = s.get("stretch_inner").and_then(Value::as_bool).unwrap_or(false);
            Scaling::NineSlice { width: n("width"), height: n("height"), border, stretch_inner }
        }
        _ => Scaling::Stretch,
    }
}

impl GuiAtlas {
    /// Loads and packs all the sprites (shelves by decreasing height, 1 pixel gap).
    pub fn load(assets: &Assets) -> GuiAtlas {
        const PREFIX: &str = "assets/minecraft/textures/gui/sprites/";
        // Effect icons: directory added with the prefix `mob_effect/`.
        const EFFECTS: &str = "assets/minecraft/textures/mob_effect/";
        const BANNERS: &str = "assets/minecraft/textures/entity/banner/";
        const DECOS: &str = "assets/minecraft/textures/map/decorations/";
        // Container screen backgrounds (256 × 256): prefix `container_bg/`.
        const CONTAINERS: &str = "assets/minecraft/textures/gui/container/";
        const BACKGROUNDS: [&str; 26] = [
            "inventory",
            "generic_54",
            "dispenser",
            "hopper",
            "crafting_table",
            "furnace",
            "blast_furnace",
            "smoker",
            "brewing_stand",
            "gamemode_switcher",
            "creative_inventory/tab_items",
            "creative_inventory/tab_item_search",
            "creative_inventory/tab_inventory",
            // Special screens (`special-screens.md`).
            "villager",
            "horse",
            "nautilus",
            "grindstone",
            "stonecutter",
            "loom",
            "cartography_table",
            "smithing",
            "enchanting_table",
            "anvil",
            "beacon",
            "crafter",
            "shulker_box",
        ];
        // Menu screen backgrounds and list separators (`textures/gui/*.png`): prefix `bg/`.
        const GUI: &str = "assets/minecraft/textures/gui/";
        // Plus the progress window and the statistics header background.
        const MENU_BACKGROUNDS: [&str; 11] = [
            "advancements/window",
            "tab_header_background",
            "menu_background",
            "inworld_menu_background",
            "menu_list_background",
            "inworld_menu_list_background",
            "header_separator",
            "inworld_header_separator",
            "footer_separator",
            "inworld_footer_separator",
            "recipe_book",
        ];
        // Title screen and server list: logo, edition, unknown server icon (prefix `tex/`).
        const TEX: &str = "assets/minecraft/textures/";
        // Plus the resource loading screen logo and the unknown pack icon.
        // Plus the thumbnail of the end screen.
        // Plus the backgrounds of the portal loading screen (animated Nether, End sky and layers).
        const TITLE: [&str; 10] = [
            "block/nether_portal",
            "environment/end_sky",
            "entity/end_portal/end_portal",
            "gui/title/minecraft",
            "gui/title/minceraft",
            "gui/title/edition",
            "misc/unknown_server",
            "gui/title/mojangstudios",
            "misc/unknown_pack",
            "misc/credits_vignette",
        ];
        let mut items: Vec<(String, Image, Scaling)> = assets
            .list(PREFIX, ".png")
            .into_iter()
            .chain(assets.list(EFFECTS, ".png"))
            .chain(BACKGROUNDS.iter().map(|n| format!("{CONTAINERS}{n}.png")))
            .chain(MENU_BACKGROUNDS.iter().map(|n| format!("{GUI}{n}.png")))
            // Books and sign editors: `bg/book`, `bg/signs/<wood>`, `bg/hanging_signs/<wood>`.
            .chain(std::iter::once(format!("{GUI}book.png")))
            .chain(assets.list(&format!("{GUI}signs/"), ".png"))
            .chain(assets.list(&format!("{GUI}hanging_signs/"), ".png"))
            // Banner patterns (loom thumbnails): `banner/<pattern>`.
            .chain(assets.list(BANNERS, ".png"))
            .chain(TITLE.iter().map(|n| format!("{TEX}{n}.png")))
            // Map decorations (cartography table preview): `map_deco/<type>`.
            .chain(assets.list(DECOS, ".png"))
            .filter_map(|path| {
                let img = assets.image(&path)?;
                let meta =
                    assets.read(&format!("{path}.mcmeta")).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                // Animated sprite: vertical strip of frames (height `animation.height`, otherwise
                // square); the first under the sprite's name, the following ones as `<name>#<i>`.
                let anim = meta.as_ref().and_then(|m| m.get("animation"));
                let frame_h = anim
                    .and_then(|a| a.get("height"))
                    .and_then(Value::as_u64)
                    .map_or(img.width, |h| h as u32)
                    .clamp(1, img.height.max(1));
                let frames: Vec<Image> = if anim.is_some() {
                    (0..(img.height / frame_h).max(1))
                        .map(|i| {
                            let row = (img.width * 4) as usize;
                            let s = i as usize * frame_h as usize * row;
                            let rgba = img.rgba[s..s + frame_h as usize * row].to_vec();
                            Image { width: img.width, height: frame_h, rgba }
                        })
                        .collect()
                } else {
                    vec![img]
                };
                let name = if let Some(n) = path.strip_prefix(PREFIX) {
                    n.strip_suffix(".png")?.to_owned()
                } else if let Some(n) = path.strip_prefix(CONTAINERS) {
                    format!("container_bg/{}", n.strip_suffix(".png")?)
                } else if let Some(n) = path.strip_prefix(BANNERS) {
                    format!("banner/{}", n.strip_suffix(".png")?)
                } else if let Some(n) = path.strip_prefix(EFFECTS) {
                    format!("mob_effect/{}", n.strip_suffix(".png")?)
                } else if let Some(n) = path.strip_prefix(DECOS) {
                    format!("map_deco/{}", n.strip_suffix(".png")?)
                } else if let Some(n) = path.strip_prefix(TEX).filter(|n| TITLE.iter().any(|t| n.starts_with(t))) {
                    format!("tex/{}", n.strip_suffix(".png")?)
                } else {
                    format!("bg/{}", path.strip_prefix(GUI)?.strip_suffix(".png")?)
                };
                let sc = scaling(meta.as_ref());
                Some(
                    frames
                        .into_iter()
                        .enumerate()
                        .map(|(i, f)| (if i == 0 { name.clone() } else { format!("{name}#{i}") }, f, sc))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();
        // Reserved slots, written at run time: 64 × 64 icons of the server list.
        for i in 0..SERVER_ICON_SLOTS {
            let img = Image { width: 64, height: 64, rgba: vec![0; 64 * 64 * 4] };
            items.push((format!("dyn/server_icon/{i}"), img, Scaling::Stretch));
        }
        for i in 0..PACK_ICON_SLOTS {
            let img = Image { width: 64, height: 64, rgba: vec![0; 64 * 64 * 4] };
            items.push((format!("dyn/pack_icon/{i}"), img, Scaling::Stretch));
        }
        for i in 0..ADV_BG_SLOTS {
            let img = Image { width: 16, height: 16, rgba: vec![0; 16 * 16 * 4] };
            items.push((format!("dyn/adv_bg/{i}"), img, Scaling::Stretch));
        }
        for i in 0..FACE_SLOTS {
            let img = Image { width: 8, height: 8, rgba: vec![0; 8 * 8 * 4] };
            items.push((format!("dyn/face/{i}"), img, Scaling::Stretch));
        }
        for i in 0..TAB_FACE_SLOTS {
            let img = Image { width: 8, height: 8, rgba: vec![0; 8 * 8 * 4] };
            items.push((format!("dyn/tab_face/{i}"), img, Scaling::Stretch));
        }
        for i in 0..TEXT_OBJECT_SLOTS {
            let n = TEXT_OBJECT_SIZE;
            let img = Image { width: n, height: n, rgba: vec![0; (n * n * 4) as usize] };
            items.push((format!("dyn/text_obj/{i}"), img, Scaling::Stretch));
        }
        for i in 0..TEXT_HEAD_SLOTS {
            let img = Image { width: 8, height: 8, rgba: vec![0; 8 * 8 * 4] };
            items.push((format!("dyn/text_head/{i}"), img, Scaling::Stretch));
        }
        items.push((MISSING_SPRITE.to_owned(), Image::missing(16), Scaling::Stretch));
        // Map preview of the cartography table (128 × 128, rewritten at run time).
        items.push((
            "dyn/map/0".to_owned(),
            Image { width: 128, height: 128, rgba: vec![0; 128 * 128 * 4] },
            Scaling::Stretch,
        ));
        // Subtitle of the title screen, drawn by us (not a game resource).
        items.push(("rustcraft/edition".to_owned(), edition_image(), Scaling::Stretch));
        items.sort_by(|a, b| b.1.height.cmp(&a.1.height).then(a.0.cmp(&b.0)));
        let width = 2048u32;
        let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
        let mut placed = Vec::with_capacity(items.len());
        for (name, img, sc) in items {
            if x + img.width > width {
                x = 0;
                y += shelf + 1;
                shelf = 0;
            }
            placed.push((name, img, sc, x, y));
            x += placed.last().map_or(0, |p| p.1.width) + 1;
            shelf = shelf.max(placed.last().map_or(0, |p| p.1.height));
        }
        // Height to the nearest multiple of 4, not to the power of 2: VRAM only pays for what is useful
        // (coordinates are normalised by the real size, nothing requires a power of 2).
        let height = (y + shelf + 1).next_multiple_of(4);
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let mut sprites = HashMap::new();
        for (name, img, sc, px, py) in placed {
            for row in 0..img.height {
                let src = (row * img.width * 4) as usize;
                let dst = (((py + row) * width + px) * 4) as usize;
                rgba[dst..dst + (img.width * 4) as usize]
                    .copy_from_slice(&img.rgba[src..src + (img.width * 4) as usize]);
            }
            sprites.insert(name, Sprite { x: px, y: py, width: img.width, height: img.height, scaling: sc });
        }
        GuiAtlas { image: Image { width, height, rgba }, sprites }
    }
}

/// Square letters of "RUST EDITION", 7 rows, drawn for Rustcraft.
const EDITION_GLYPHS: [(char, [&str; 7]); 9] = [
    ('R', ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"]),
    ('U', ["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    ('S', [".####", "#....", "#....", ".###.", "....#", "....#", "####."]),
    ('T', ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."]),
    ('E', ["#####", "#....", "#....", "####.", "#....", "#....", "#####"]),
    ('D', ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."]),
    ('I', ["###..", ".#...", ".#...", ".#...", ".#...", ".#...", "###.."]),
    ('O', [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    ('N', ["#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#", "#...#"]),
];

/// 512 × 64 image of the "RUST EDITION" subtitle of the title screen, in the same format as the
/// game's subtitle (displayed at 128 × 14): square letters in a light gradient, bevel, dark
/// extrusion towards the bottom right, black outline. Entirely generated here.
fn edition_image() -> Image {
    const W: usize = 512;
    const H: usize = 64;
    const CELL: usize = 7;
    const OUTLINE: i32 = 3;
    const DEPTH: i32 = 4;
    // Strokes thickened by one pixel on each side (solid lettering).
    const BOLD: i32 = 1;
    let text = "RUST EDITION";
    let glyph = |c: char| EDITION_GLYPHS.iter().find(|g| g.0 == c).map(|g| g.1);
    let width_of = |c: char| match c {
        ' ' => 3,
        'I' => 3,
        _ => 5,
    };
    let cells: usize = text.chars().map(width_of).sum::<usize>() + text.chars().count() - 1;
    let mut x0 = (W - cells * CELL) / 2;
    let top = (OUTLINE + BOLD) as usize;
    let mut face = vec![false; W * H];
    for c in text.chars() {
        if let Some(rows) = glyph(c) {
            for (r, row) in rows.iter().enumerate() {
                for (k, b) in row.bytes().enumerate() {
                    if b != b'#' {
                        continue;
                    }
                    for dy in 0..CELL {
                        for dx in 0..CELL {
                            face[(top + r * CELL + dy) * W + x0 + k * CELL + dx] = true;
                        }
                    }
                }
            }
        }
        x0 += (width_of(c) + 1) * CELL;
    }
    let at = |m: &[bool], x: i32, y: i32| {
        x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H && m[y as usize * W + x as usize]
    };
    let thin = face;
    let face: Vec<bool> = (0..W * H)
        .map(|i| {
            let (x, y) = ((i % W) as i32, (i / W) as i32);
            (-BOLD..=BOLD).any(|dy| (-BOLD..=BOLD).any(|dx| at(&thin, x + dx, y + dy)))
        })
        .collect();
    // Extrusion: the face shifted diagonally, under the face.
    let mut depth = vec![0u8; W * H];
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            if at(&face, x, y) {
                continue;
            }
            if let Some(d) = (1..=DEPTH).find(|&d| at(&face, x - d, y - d)) {
                depth[y as usize * W + x as usize] = d as u8;
            }
        }
    }
    let solid: Vec<bool> = (0..W * H).map(|i| face[i] || depth[i] > 0).collect();
    let face_top = top as f32;
    let face_bottom = (top + 7 * CELL) as f32;
    let mut rgba = vec![0u8; W * H * 4];
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let i = y as usize * W + x as usize;
            let c: [u8; 4] = if face[i] {
                let t = ((y as f32 - face_top) / (face_bottom - face_top)).clamp(0.0, 1.0);
                let mix = |a: f32, b: f32| (a + (b - a) * t) as u8;
                if !at(&face, x - 2, y) || !at(&face, x, y - 2) {
                    [250, 247, 246, 255]
                } else if !at(&face, x + 2, y) || !at(&face, x, y + 2) {
                    [132, 124, 122, 255]
                } else {
                    [mix(236.0, 170.0), mix(230.0, 160.0), mix(228.0, 158.0), 255]
                }
            } else if depth[i] > 0 {
                let g = 70 - depth[i] * 6;
                [g, g - 2, g - 2, 255]
            } else if (-OUTLINE..=OUTLINE).any(|dy| {
                (-OUTLINE..=OUTLINE).any(|dx| dx * dx + dy * dy <= OUTLINE * OUTLINE && at(&solid, x + dx, y + dy))
            }) {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba[i * 4..i * 4 + 4].copy_from_slice(&c);
        }
    }
    Image { width: W as u32, height: H as u32, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn png(path: &std::path::Path, w: u32, h: u32, v: u8) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        let f = std::fs::File::create(path).expect("png");
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        e.set_color(png::ColorType::Rgba);
        let mut wr = e.write_header().expect("header");
        wr.write_image_data(&vec![v; (w * h * 4) as usize]).expect("pixels");
    }

    /// Sprites of an atlas from its sources: directory with prefix (other namespaces
    /// included), single renamed file; a sprite outside the sources is not found (missing
    /// texture), like `item/diamond` looked up in the game's block atlas.
    #[test]
    fn atlas_sprites_follow_the_atlas_sources() {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp").join(format!("g1f13-atlas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let atlases = d.join("assets/minecraft/atlases");
        std::fs::create_dir_all(&atlases).expect("atlases");
        std::fs::write(
            atlases.join("blocks.json"),
            br#"{"sources":[{"type":"minecraft:directory","prefix":"block/","source":"block"},
                {"type":"single","resource":"minecraft:entity/bell/bell_body","sprite":"minecraft:bell"}]}"#,
        )
        .expect("blocks.json");
        png(&d.join("assets/minecraft/textures/block/stone.png"), 16, 16, 7);
        png(&d.join("assets/mod/textures/block/ore.png"), 16, 32, 9);
        png(&d.join("assets/minecraft/textures/entity/bell/bell_body.png"), 32, 32, 3);
        png(&d.join("assets/minecraft/textures/item/diamond.png"), 16, 16, 1);
        let a = crate::Assets::new(vec![crate::source::Source::dir(&d)]);
        let stone = atlas_sprite(&a, "minecraft:blocks", "minecraft:block/stone").expect("stone");
        assert_eq!((stone.width, stone.height, stone.rgba[0]), (16, 16, 7));
        // Animated strip: first frame only.
        let ore = atlas_sprite(&a, "minecraft:blocks", "mod:block/ore").expect("ore");
        assert_eq!((ore.width, ore.height, ore.rgba[0]), (16, 16, 9));
        assert_eq!(atlas_sprite(&a, "minecraft:blocks", "minecraft:bell").map(|i| i.rgba[0]), Some(3));
        assert!(atlas_sprite(&a, "minecraft:blocks", "minecraft:item/diamond").is_none());
        assert!(atlas_sprite(&a, "minecraft:blocks", "minecraft:entity/bell/bell_body").is_none());
        assert!(atlas_sprite(&a, "minecraft:items", "minecraft:item/diamond").is_none(), "atlas without definition");
        let _ = std::fs::remove_dir_all(&d);
    }
}
