//! Block textures: PNG decoding to RGBA8, first frame of animated textures, resizing to
//! the common size, transparency analysis per area (rule of `docs/specs/models.md`).

use crate::source::{Assets, split_id};

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}×{})", self.width, self.height)
    }
}

/// RGBA8 image, rows from top to bottom.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transparency {
    /// At least one pixel with alpha 0.
    pub transparent: bool,
    /// At least one pixel with alpha strictly between 0 and 255.
    pub translucent: bool,
}

impl Image {
    /// Neutral checkerboard (dark gray / light gray) for missing textures.
    pub fn fallback(size: u32) -> Self {
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let on = ((x * 2 / size) + (y * 2 / size)).is_multiple_of(2);
                let v = if on { 70 } else { 140 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Image { width: size, height: size, rgba }
    }

    /// The game's missing texture: purple quarters (248, 0, 248) at the top right and bottom left,
    /// black elsewhere.
    pub fn missing(size: u32) -> Self {
        let half = size / 2;
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let pink = (y < half) != (x < half);
                rgba.extend_from_slice(if pink { &[248, 0, 248, 255] } else { &[0, 0, 0, 255] });
            }
        }
        Image { width: size, height: size, rgba }
    }

    pub fn decode_png(bytes: &[u8]) -> Option<Self> {
        Self::decode_png_typed(bytes).map(|(i, _)| i)
    }

    /// Like [`Image::decode_png`], with the pixel kind of the file after expansion (palette
    /// expanded to RGB or RGBA): grays keep their luminance in red.
    pub fn decode_png_typed(bytes: &[u8]) -> Option<(Self, png::ColorType)> {
        let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        buf.truncate(info.buffer_size());
        let (w, h) = (info.width, info.height);
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => {
                buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
            }
            png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return None,
        };
        Some((Image { width: w, height: h, rgba }, info.color_type))
    }

    /// First square frame of a vertical animated strip (width × width).
    pub fn first_frame(mut self) -> Self {
        if self.height > self.width && self.height.is_multiple_of(self.width) {
            self.rgba.truncate((self.width * self.width * 4) as usize);
            self.height = self.width;
        }
        self
    }

    /// Nearest-neighbour resizing (packs with textures of mixed sizes).
    pub fn resized(&self, size: u32) -> Self {
        if self.width == size && self.height == size {
            return self.clone();
        }
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let (sx, sy) = (x * self.width / size, y * self.height / size);
                let i = ((sy * self.width + sx) * 4) as usize;
                rgba.extend_from_slice(&self.rgba[i..i + 4]);
            }
        }
        Image { width: size, height: size, rgba }
    }

    /// Transparency of the area `[u0, u1) × [v0, v1)` in texture fractions.
    pub fn transparency(&self, u0: f32, v0: f32, u1: f32, v1: f32) -> Transparency {
        let whole = u0 == 0.0 && v0 == 0.0 && u1 == 1.0 && v1 == 1.0;
        let (x0, y0, x1, y1) = if whole {
            (0, 0, self.width, self.height)
        } else {
            (
                (u0 * self.width as f32).floor().max(0.0) as u32,
                (v0 * self.height as f32).floor().max(0.0) as u32,
                ((u1 * self.width as f32).ceil() as u32).min(self.width),
                ((v1 * self.height as f32).ceil() as u32).min(self.height),
            )
        };
        let mut t = Transparency::default();
        for y in y0..y1 {
            for x in x0..x1 {
                match self.rgba[((y * self.width + x) * 4 + 3) as usize] {
                    0 => t.transparent = true,
                    255 => {}
                    _ => t.translucent = true,
                }
            }
        }
        t
    }
}

impl Transparency {
    fn or(self, o: Self) -> Self {
        Transparency { transparent: self.transparent || o.transparent, translucent: self.translucent || o.translucent }
    }
}

/// Transparency of an area of a texture, animated or not, like the game: the whole texture counts
/// all its frames (opaque everywhere ⇒ opaque); a sub-area takes the union of the same area over
/// each distinct frame that the animation shows (an opaque 1st frame is not enough).
pub fn sprite_transparency(
    first: &Image,
    anim: Option<(&[Image], &Animation)>,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
) -> Transparency {
    let Some((frames, a)) = anim.filter(|(f, _)| !f.is_empty()) else { return first.transparency(u0, v0, u1, v1) };
    let all = frames.iter().fold(Transparency::default(), |t, f| t.or(f.transparency(0.0, 0.0, 1.0, 1.0)));
    if all == Transparency::default() || (u0 == 0.0 && v0 == 0.0 && u1 == 1.0 && v1 == 1.0) {
        return all;
    }
    let mut seen = Vec::with_capacity(a.frames.len());
    let mut t = Transparency::default();
    for &(i, _) in &a.frames {
        if !seen.contains(&i)
            && let Some(f) = frames.get(i as usize)
        {
            seen.push(i);
            t = t.or(f.transparency(u0, v0, u1, v1));
        }
    }
    t
}

/// Loads `textures/<path>.png` of a texture identifier; `None` if absent or unreadable.
pub fn load(assets: &Assets, id: &str) -> Option<Image> {
    let (ns, path) = split_id(id);
    assets.image(&format!("assets/{ns}/textures/{path}.png")).map(Image::first_frame)
}

/// Animation of a texture (`animation` section of the `.png.mcmeta`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    /// Steps in playback order: (frame of the strip, duration in ticks).
    pub frames: Vec<(u32, u32)>,
    /// Fade from the current frame to the next one over the step.
    pub interpolate: bool,
}

crate::codec_struct!(Animation { frames, interpolate });

/// Texture with all its frames (`frames[i]` = frame i of the strip, row by row) and
/// their mip levels like the game (`levels[i][0]` = original image, ready to display).
pub struct Loaded {
    pub frames: Vec<Image>,
    pub animation: Option<Animation>,
    pub levels: Vec<Vec<Image>>,
}

/// `w` × `h` rectangle of an image starting at (x0, y0).
fn cut(img: &Image, x0: u32, y0: u32, w: u32, h: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y0 + h {
        let s = ((y * img.width + x0) * 4) as usize;
        rgba.extend_from_slice(&img.rgba[s..s + (w * 4) as usize]);
    }
    Image { width: w, height: h, rgba }
}

/// Mipmap strategy and alpha bias (`texture` section of the `.png.mcmeta`).
fn mip_meta(json: Option<&serde_json::Value>) -> (crate::mip::Strategy, f32) {
    let t = json.and_then(|v| v.get("texture"));
    let strategy = t
        .and_then(|t| t.get("mipmap_strategy"))
        .and_then(serde_json::Value::as_str)
        .and_then(crate::mip::Strategy::parse)
        .unwrap_or_default();
    let bias = t.and_then(|t| t.get("alpha_cutoff_bias")).and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
    (strategy, bias)
}

/// Mip levels of the game for an image of `side` pixels per side at the smallest.
fn game_levels(side: u32) -> u32 {
    crate::mip::GAME_LEVELS.min(side.max(1).ilog2())
}

/// Fixed texture: levels computed on the image alone.
fn single(img: Image, mip: (crate::mip::Strategy, f32)) -> Loaded {
    let levels = crate::mip::generate(img.clone(), game_levels(img.width.min(img.height)), mip.0, mip.1);
    Loaded { frames: vec![img], animation: None, levels: vec![levels] }
}

/// Like [`load`], with the animation frames when the `.png.mcmeta` describes one.
/// Metadata absent or unreadable: fixed texture (first frame), without error.
pub fn load_animated(assets: &Assets, id: &str) -> Option<Loaded> {
    let (ns, path) = split_id(id);
    let file = format!("assets/{ns}/textures/{path}.png");
    let img = assets.image(&file)?;
    let json =
        assets.read(&format!("{file}.mcmeta")).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let mip = mip_meta(json.as_ref());
    let Some(meta) = json.as_ref().and_then(|v| v.get("animation")) else {
        return Some(single(img.first_frame(), mip));
    };
    Some(split_frames(img, meta, mip))
}

/// Splits a strip of frames according to the animation metadata (spec: `docs/specs/models.md`);
/// the mips are computed on the whole strip then split frame by frame, like the game.
pub fn split_frames(img: Image, meta: &serde_json::Value, mip: (crate::mip::Strategy, f32)) -> Loaded {
    let positive = |k: &str| meta.get(k).and_then(serde_json::Value::as_u64).filter(|&v| v > 0).map(|v| v as u32);
    let side = img.width.min(img.height);
    let (fw, fh) = match (positive("width"), positive("height")) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, img.height),
        (None, Some(h)) => (img.width, h),
        (None, None) => (side, side),
    };
    let per_row = img.width / fw;
    let count = per_row * (img.height / fh);
    if count == 0 {
        return single(img.first_frame(), mip);
    }
    let strip = crate::mip::generate(img, game_levels(fw.min(fh)), mip.0, mip.1);
    let levels: Vec<Vec<Image>> = (0..count)
        .map(|i| {
            let (x0, y0) = ((i % per_row) * fw, (i / per_row) * fh);
            strip.iter().enumerate().map(|(l, lv)| cut(lv, x0 >> l, y0 >> l, fw >> l, fh >> l)).collect()
        })
        .collect();
    let frames: Vec<Image> = levels.iter().map(|l| l[0].clone()).collect();
    let time = positive("frametime").unwrap_or(1);
    let steps: Vec<(u32, u32)> = match meta.get("frames").and_then(serde_json::Value::as_array) {
        None => (0..count).map(|i| (i, time)).collect(),
        Some(list) => list
            .iter()
            .filter_map(|f| match f {
                serde_json::Value::Number(n) => Some((n.as_u64()?, u64::from(time))),
                serde_json::Value::Object(o) => {
                    let index = o.get("index")?.as_u64()?;
                    Some((index, o.get("time").map_or_else(|| Some(u64::from(time)), serde_json::Value::as_u64)?))
                }
                _ => None,
            })
            .filter(|&(i, t)| i < u64::from(count) && t > 0)
            .map(|(i, t)| (i as u32, t.min(u64::from(u32::MAX)) as u32))
            .collect(),
    };
    let interpolate = meta.get("interpolate").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let animation = (steps.len() > 1).then_some(Animation { frames: steps, interpolate });
    Loaded { frames, animation, levels }
}

/// Biome colour palettes (`textures/colormap/*.png` of the jar or the packs).
pub struct Colormaps {
    pub grass: Option<Image>,
    pub foliage: Option<Image>,
    pub dry_foliage: Option<Image>,
}

impl Colormaps {
    pub fn load(assets: &Assets) -> Self {
        let get = |n: &str| load(assets, &format!("minecraft:colormap/{n}"));
        Colormaps { grass: get("grass"), foliage: get("foliage"), dry_foliage: get("dry_foliage") }
    }
}

/// Colour of a palette for a temperature and a humidity (official rule):
/// humidity × temperature, index `(1 − h) × 255` as row, `(1 − t) × 255` as column.
pub fn colormap(map: Option<&Image>, temperature: f32, downfall: f32, default: u32) -> u32 {
    let t = f64::from(temperature.clamp(0.0, 1.0));
    let r = f64::from(downfall.clamp(0.0, 1.0)) * t;
    let (x, y) = (((1.0 - t) * 255.0) as u32, ((1.0 - r) * 255.0) as u32);
    let Some(m) = map.filter(|m| m.width == 256 && m.height == 256) else { return default };
    let i = (((y << 8) | x) * 4) as usize;
    u32::from_be_bytes([0, m.rgba[i], m.rgba[i + 1], m.rgba[i + 2]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparency_rules() {
        let mut img = Image::fallback(16);
        assert_eq!(img.transparency(0.0, 0.0, 1.0, 1.0), Transparency::default());
        img.rgba[3] = 0; // pixel (0, 0) transparent
        img.rgba[(15 * 16 + 15) * 4 + 3] = 128; // pixel (15, 15) translucent
        assert_eq!(img.transparency(0.0, 0.0, 1.0, 1.0), Transparency { transparent: true, translucent: true });
        // Area that excludes both pixels.
        assert_eq!(img.transparency(0.25, 0.25, 0.75, 0.75), Transparency::default());
        assert!(img.transparency(0.0, 0.0, 0.1, 0.1).transparent);
    }

    #[test]
    fn animated_transparency_unions_frames() {
        // Two 2×2 images: the 1st opaque, the 2nd with a hole at (1, 1).
        let opaque = Image { width: 2, height: 2, rgba: vec![255; 16] };
        let mut holed = opaque.clone();
        holed.rgba[15] = 0;
        let frames = [opaque.clone(), holed];
        let both = Animation { frames: vec![(0, 1), (1, 1)], interpolate: false };
        let only_first = Animation { frames: vec![(0, 1), (0, 2)], interpolate: false };
        let t = |a: &Animation, u0, v0, u1, v1| sprite_transparency(&opaque, Some((&frames, a)), u0, v0, u1, v1);
        // Whole texture: all the frames count, even a frame never displayed.
        assert!(t(&both, 0.0, 0.0, 1.0, 1.0).transparent);
        assert!(t(&only_first, 0.0, 0.0, 1.0, 1.0).transparent);
        // Sub-area containing the hole: union over the displayed frames only.
        assert!(t(&both, 0.5, 0.5, 1.0, 1.0).transparent);
        assert!(!t(&only_first, 0.5, 0.5, 1.0, 1.0).transparent);
        // Sub-area without the hole: opaque.
        assert_eq!(t(&both, 0.0, 0.0, 0.5, 0.5), Transparency::default());
        // Without animation: the only frame.
        assert_eq!(sprite_transparency(&opaque, None, 0.0, 0.0, 1.0, 1.0), Transparency::default());
    }

    #[test]
    fn frames_and_resize() {
        let strip = Image { width: 2, height: 6, rgba: vec![9; 2 * 6 * 4] };
        let f = strip.first_frame();
        assert_eq!((f.width, f.height, f.rgba.len()), (2, 2, 16));
        let big = Image::fallback(16).resized(32);
        assert_eq!((big.width, big.height), (32, 32));
    }

    #[test]
    fn animation_metadata() {
        // 2×6 strip: three square frames, the pixel value gives the frame number.
        let strip = || Image { width: 2, height: 6, rgba: (0..48).map(|i| (i / 16) as u8).collect() };
        let meta = serde_json::json!({});
        let l = split_frames(strip(), &meta, Default::default());
        assert_eq!(l.frames.len(), 3);
        assert_eq!(l.frames[2].rgba[0], 2);
        assert_eq!(l.animation, Some(Animation { frames: vec![(0, 1), (1, 1), (2, 1)], interpolate: false }));
        // Explicit list, default and own duration, invalid index discarded, fade.
        let meta = serde_json::json!({"frametime": 4, "interpolate": true,
            "frames": [2, {"index": 0, "time": 9}, 7, {"index": 1, "time": 0}]});
        let l = split_frames(strip(), &meta, Default::default());
        assert_eq!(l.animation, Some(Animation { frames: vec![(2, 4), (0, 9)], interpolate: true }));
        // A single step: fixed texture.
        let l = split_frames(strip(), &serde_json::json!({"frames": [1]}), Default::default());
        assert_eq!(l.animation, None);
    }
}
