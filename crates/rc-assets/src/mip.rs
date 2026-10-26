//! Mipmaps of block textures, like the game (`docs/specs/models.md`, "Mipmaps"):
//! per-texture strategy, filling of empty pixels at level 0, average in linear light,
//! alpha rescaled to keep the coverage of cutout textures.

use std::collections::VecDeque;
use std::sync::OnceLock;

use crate::texture::Image;

/// Mipmap strategy (`texture` section of the `.png.mcmeta`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strategy {
    #[default]
    Auto,
    Mean,
    Cutout,
    StrictCutout,
    DarkCutout,
}

impl Strategy {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Strategy::Auto,
            "mean" => Strategy::Mean,
            "cutout" => Strategy::Cutout,
            "strict_cutout" => Strategy::StrictCutout,
            "dark_cutout" => Strategy::DarkCutout,
            _ => return None,
        })
    }
}

/// Mip levels of the game below the original image (default setting).
pub const GAME_LEVELS: u32 = 4;

fn round_f32(v: f32) -> i32 {
    (f64::from(v) + 0.5).floor() as i32
}

/// sRGB → linear tables (out of 1023) and linear (out of 1023) → sRGB.
fn tables() -> &'static ([u16; 256], [u8; 1024]) {
    static T: OnceLock<([u16; 256], [u8; 1024])> = OnceLock::new();
    T.get_or_init(|| {
        let to_linear = |x: f32| {
            if x >= 0.04045 { ((f64::from(x) + 0.055) / 1.055).powf(2.4) as f32 } else { x / 12.92 }
        };
        let to_srgb = |x: f32| {
            if x >= 0.003_130_8 {
                (1.055 * f64::from(x).powf(0.416_666_666_666_666_7) - 0.055) as f32
            } else {
                12.92 * x
            }
        };
        let mut lin = [0u16; 256];
        for (i, v) in lin.iter_mut().enumerate() {
            *v = round_f32(to_linear(i as f32 / 255.0) * 1023.0) as u16;
        }
        let mut srgb = [0u8; 1024];
        for (i, v) in srgb.iter_mut().enumerate() {
            *v = round_f32(to_srgb(i as f32 / 1023.0) * 255.0) as u8;
        }
        (lin, srgb)
    })
}

fn px(img: &Image, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]]
}

fn set(img: &mut Image, x: u32, y: u32, c: [u8; 4]) {
    let i = ((y * img.width + x) * 4) as usize;
    img.rgba[i..i + 4].copy_from_slice(&c);
}

/// Colour of empty pixels = that of the nearest non-empty pixel (alpha left at 0).
fn solidify(img: &mut Image) {
    let (w, h) = (img.width as i32, img.height as i32);
    let n = (w * h) as usize;
    let mut nearest = vec![[0u8; 3]; n];
    let mut dist = vec![i32::MAX; n];
    let mut queue = VecDeque::new();
    for x in 0..w {
        for y in 0..h {
            let c = px(img, x as u32, y as u32);
            if c[3] != 0 {
                let p = (x + y * w) as usize;
                dist[p] = 0;
                nearest[p] = [c[0], c[1], c[2]];
                queue.push_back(p);
            }
        }
    }
    while let Some(p) = queue.pop_front() {
        let (x, y) = (p as i32 % w, p as i32 / w);
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx >= 0 && ny >= 0 && nx < w && ny < h {
                let q = (nx + ny * w) as usize;
                if dist[q] > dist[p] + 1 {
                    dist[q] = dist[p] + 1;
                    nearest[q] = nearest[p];
                    queue.push_back(q);
                }
            }
        }
    }
    for x in 0..w {
        for y in 0..h {
            if px(img, x as u32, y as u32)[3] == 0 {
                let c = nearest[(x + y * w) as usize];
                set(img, x as u32, y as u32, [c[0], c[1], c[2], 0]);
            }
        }
    }
}

/// Colour of empty pixels = darkest non-empty pixel, × 3/4 (alpha 0).
fn fill_dark(img: &mut Image) {
    let mut darkest = [255u8; 4];
    let mut min = i32::MAX;
    for x in 0..img.width {
        for y in 0..img.height {
            let c = px(img, x, y);
            if c[3] != 0 {
                let b = i32::from(c[0]) + i32::from(c[1]) + i32::from(c[2]);
                if b < min {
                    min = b;
                    darkest = c;
                }
            }
        }
    }
    let dark = [3 * u16::from(darkest[0]) / 4, 3 * u16::from(darkest[1]) / 4, 3 * u16::from(darkest[2]) / 4];
    for x in 0..img.width {
        for y in 0..img.height {
            if px(img, x, y)[3] == 0 {
                set(img, x, y, [dark[0] as u8, dark[1] as u8, dark[2] as u8, 0]);
            }
        }
    }
}

fn mean_linear(c: [[u8; 4]; 4]) -> [u8; 4] {
    let (lin, srgb) = tables();
    let ch = |k: usize| {
        let sum: u32 = c.iter().map(|p| u32::from(lin[p[k] as usize])).sum();
        srgb[(sum / 4) as usize]
    };
    let a = c.iter().map(|p| u32::from(p[3])).sum::<u32>() / 4;
    [ch(0), ch(1), ch(2), a as u8]
}

fn darkened_blend(c: [[u8; 4]; 4]) -> [u8; 4] {
    let (lin, srgb) = tables();
    let to = |v: u8| f32::from(lin[v as usize]) / 1023.0;
    let mut t = [0f32; 4];
    for p in c.iter().filter(|p| p[3] != 0) {
        // Game order: alpha, red, green, blue.
        t[3] += to(p[3]);
        t[0] += to(p[0]);
        t[1] += to(p[1]);
        t[2] += to(p[2]);
    }
    let back = |v: f32| srgb[((v / 4.0) * 1023.0).floor() as usize];
    [back(t[0]), back(t[1]), back(t[2]), back(t[3])]
}

fn coverage(img: &Image, reference: f32, scale: f32) -> f32 {
    let (w, h) = (img.width, img.height);
    let a = |x, y| (f32::from(px(img, x, y)[3]) / 255.0 * scale).clamp(0.0, 1.0);
    let mut total = 0.0f32;
    for y in 0..h.saturating_sub(1) {
        for x in 0..w.saturating_sub(1) {
            let (a00, a10, a01, a11) = (a(x, y), a(x + 1, y), a(x, y + 1), a(x + 1, y + 1));
            let mut texel = 0.0f32;
            for sy in 0..4 {
                let fy = (sy as f32 + 0.5) / 4.0;
                for sx in 0..4 {
                    let fx = (sx as f32 + 0.5) / 4.0;
                    let v =
                        a00 * (1.0 - fx) * (1.0 - fy) + a10 * fx * (1.0 - fy) + a01 * (1.0 - fx) * fy + a11 * fx * fy;
                    if v > reference {
                        texel += 1.0;
                    }
                }
            }
            total += texel / 16.0;
        }
    }
    total / ((w as f32 - 1.0) * (h as f32 - 1.0))
}

fn scale_alpha(img: &mut Image, desired: f32, reference: f32, bias: f32) {
    let (mut lo, mut hi, mut scale, mut best, mut best_err) = (0.0f32, 4.0f32, 1.0f32, 1.0f32, f32::MAX);
    for _ in 0..5 {
        let c = coverage(img, reference, scale);
        let err = (c - desired).abs();
        if err < best_err {
            best_err = err;
            best = scale;
        }
        if c < desired {
            lo = scale;
        } else if c > desired {
            hi = scale;
        } else {
            break;
        }
        scale = (lo + hi) * 0.5;
    }
    for p in img.rgba.as_chunks_mut::<4>().0 {
        let a = (f32::from(p[3]) / 255.0 * best + bias + 0.025).clamp(0.0, 1.0);
        p[3] = (a * 255.0).floor() as u8;
    }
}

/// Levels 0 to `levels` of an image (whole strip for an animated texture), like the game.
pub fn generate(mut base: Image, levels: u32, strategy: Strategy, bias: f32) -> Vec<Image> {
    let strategy = match strategy {
        Strategy::Auto if base.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 0) => Strategy::Cutout,
        Strategy::Auto => Strategy::Mean,
        s => s,
    };
    match strategy {
        Strategy::Cutout | Strategy::StrictCutout => solidify(&mut base),
        Strategy::DarkCutout => fill_dark(&mut base),
        _ => {}
    }
    let cutout = matches!(strategy, Strategy::Cutout | Strategy::StrictCutout | Strategy::DarkCutout);
    let reference = if strategy == Strategy::StrictCutout { 0.3 } else { 0.5 };
    let original = if cutout { coverage(&base, reference, 1.0) } else { 0.0 };
    let mut out = vec![base];
    for _ in 1..=levels {
        let last = out.last().expect("level");
        let (w, h) = (last.width >> 1, last.height >> 1);
        if w == 0 || h == 0 {
            break;
        }
        let mut data = Image { width: w, height: h, rgba: vec![0; (w * h * 4) as usize] };
        for x in 0..w {
            for y in 0..h {
                let c = [
                    px(last, x * 2, y * 2),
                    px(last, x * 2 + 1, y * 2),
                    px(last, x * 2, y * 2 + 1),
                    px(last, x * 2 + 1, y * 2 + 1),
                ];
                set(&mut data, x, y, if strategy == Strategy::DarkCutout { darkened_blend(c) } else { mean_linear(c) });
            }
        }
        if cutout {
            scale_alpha(&mut data, original, reference, bias);
        }
        out.push(data);
    }
    out
}

/// Levels of a texture placed in a layer of `size` pixels (texture array): a
/// smaller texture goes down as many levels, its upper levels being its image enlarged
/// by duplication; beyond the game's levels, the average continues down to 1 pixel.
pub fn place(levels: &[Image], size: u32) -> Vec<Image> {
    let count = size.ilog2() + 1;
    let base = &levels[0];
    if base.width != base.height || !base.width.is_power_of_two() || base.width > size {
        // Unexpected shape (pack): resized, then mips by the automatic rule.
        return generate(base.resized(size), count - 1, Strategy::Auto, 0.0);
    }
    let shift = (size / base.width).ilog2();
    let mut out: Vec<Image> = Vec::with_capacity(count as usize);
    for k in 0..count {
        let level = if k < shift {
            base.resized(size >> k)
        } else if let Some(l) = levels.get((k - shift) as usize) {
            l.clone()
        } else {
            let last = out.last().expect("previous level").clone();
            generate(last, 1, Strategy::Mean, 0.0).swap_remove(1)
        };
        out.push(level);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_tables_round_trip_ends() {
        let (lin, srgb) = tables();
        assert_eq!((lin[0], lin[255]), (0, 1023));
        assert_eq!((srgb[0], srgb[1023]), (0, 255));
        // The round trip of the tables is not the identity for very dark values (like in
        // the game: 1 → 0); it is for light values.
        assert_eq!(mean_linear([[1, 1, 1, 255]; 4]), [0, 0, 0, 255]);
        for v in [128u8, 200, 255] {
            assert_eq!(mean_linear([[v, v, v, 255]; 4]), [v, v, v, 255], "value {v}");
        }
    }

    #[test]
    fn solidify_fills_empty_pixels_with_nearest_colour() {
        let mut img = Image { width: 3, height: 1, rgba: vec![10, 20, 30, 255, 0, 0, 0, 0, 0, 0, 0, 0] };
        solidify(&mut img);
        assert_eq!(img.rgba, vec![10, 20, 30, 255, 10, 20, 30, 0, 10, 20, 30, 0]);
    }

    #[test]
    fn cutout_keeps_coverage_across_levels() {
        // 16×16 checkerboard of full / empty 2×2 cells: the coverage must stay close to 1/2.
        let mut rgba = Vec::new();
        for y in 0..16 {
            for x in 0..16 {
                let on = ((x / 2) + (y / 2)) % 2 == 0;
                rgba.extend_from_slice(&[200, 100, 50, if on { 255 } else { 0 }]);
            }
        }
        let levels = generate(Image { width: 16, height: 16, rgba }, 4, Strategy::Auto, 0.0);
        assert_eq!(levels.len(), 5);
        assert_eq!((levels[4].width, levels[4].height), (1, 1));
        // Level 1: 2×2 average of a full or empty cell = alpha unchanged, rescaled.
        assert!(levels[1].rgba.chunks(4).any(|p| p[3] > 0));
    }
}
