//! Icon of a singleplayer world (`icon.png` in its folder, 64 × 64), taken from the view like the
//! game does: while the world is played without one, at most one attempt a second; an attempt
//! finds the file there (nothing more to do until the next world) or, once more than 10 sections
//! are drawn and no section waits for its mesh, asks the renderer for the image of the frame
//! before the interface. That image is cut to its centred square, reduced to 64 × 64 and written
//! as a PNG off the render thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Side of the icon, in pixels.
pub(crate) const ICON_SIDE: u32 = 64;

/// Time between two attempts.
const ATTEMPT_INTERVAL: Duration = Duration::from_secs(1);

/// Sections the frame must draw (strictly more) before an icon is taken.
pub(crate) const MIN_DRAWN_SECTIONS: usize = 10;

/// Icon state of the world being played.
#[derive(Debug, Default)]
pub(crate) struct WorldIcon {
    /// `icon.png` of the singleplayer world; `None` outside one.
    file: Option<PathBuf>,
    /// The file was seen: no more attempts for this world.
    present: bool,
    last_attempt: Option<Instant>,
}

impl WorldIcon {
    /// The icon of the singleplayer world in `dir`, not looked at yet.
    pub(crate) fn for_world(dir: &Path) -> Self {
        WorldIcon { file: Some(dir.join("icon.png")), present: false, last_attempt: None }
    }

    /// The file the frame must capture now, if any: an attempt at most once a second while the
    /// world has no icon; it captures only when `ready` (enough sections drawn, none waiting).
    pub(crate) fn due(&mut self, now: Instant, ready: impl FnOnce() -> bool) -> Option<PathBuf> {
        let file = self.file.as_ref().filter(|_| !self.present)?;
        if self.last_attempt.is_some_and(|t| now.saturating_duration_since(t) < ATTEMPT_INTERVAL) {
            return None;
        }
        self.last_attempt = Some(now);
        if file.is_file() {
            self.present = true;
            return None;
        }
        ready().then(|| file.clone())
    }
}

/// The centred square of a frame read back (rows of `row` bytes, `width` × `height`, BGRA when
/// `bgra`), reduced to 64 × 64 opaque RGBA: each icon pixel is the mean of the frame pixels its
/// area covers (the game's reduction is a filtered one too; a few levels of difference at most).
pub(crate) fn icon_pixels(data: &[u8], row: usize, width: u32, height: u32, bgra: bool) -> Vec<u8> {
    let side = width.min(height) as usize;
    let (x0, y0) = ((width as usize - side) / 2, (height as usize - side) / 2);
    let n = ICON_SIDE as usize;
    let mut out = Vec::with_capacity(n * n * 4);
    for iy in 0..n {
        let (ya, yb) = (y0 + iy * side / n, y0 + ((iy + 1) * side / n).max(iy * side / n + 1));
        for ix in 0..n {
            let (xa, xb) = (x0 + ix * side / n, x0 + ((ix + 1) * side / n).max(ix * side / n + 1));
            let mut sum = [0u32; 3];
            for y in ya..yb {
                let line = &data[y * row..];
                for x in xa..xb {
                    let p = &line[x * 4..x * 4 + 4];
                    for (s, &v) in sum.iter_mut().zip(&p[..3]) {
                        *s += u32::from(v);
                    }
                }
            }
            let count = ((yb - ya) * (xb - xa)) as u32;
            let mean = sum.map(|s| ((s + count / 2) / count) as u8);
            let (r, b) = if bgra { (mean[2], mean[0]) } else { (mean[0], mean[2]) };
            out.extend_from_slice(&[r, mean[1], b, 0xff]);
        }
    }
    out
}

/// Reduces a frame read back to the icon and writes it to `file` as a PNG, on another thread.
pub(crate) fn save(file: PathBuf, data: Vec<u8>, row: usize, width: u32, height: u32, bgra: bool) {
    std::thread::spawn(move || {
        let pixels = icon_pixels(&data, row, width, height, bgra);
        if let Err(e) = write_png(&file, &pixels) {
            eprintln!("world icon {}: {e}", file.display());
        }
    });
}

fn write_png(file: &Path, rgba: &[u8]) -> std::io::Result<()> {
    let out = std::io::BufWriter::new(std::fs::File::create(file)?);
    let mut enc = png::Encoder::new(out, ICON_SIDE, ICON_SIDE);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(rgba).map_err(std::io::Error::other)?;
    wr.finish().map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One attempt a second; the file seen ends the attempts; a capture only when ready.
    #[test]
    fn attempts_follow_the_game() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-world-icon-{}", std::process::id())));
        std::fs::create_dir_all(&dir).expect("world folder");
        let mut icon = WorldIcon::for_world(&dir);
        let t0 = Instant::now();
        assert_eq!(icon.due(t0, || false), None, "not ready: no capture");
        assert_eq!(icon.due(t0 + Duration::from_millis(999), || true), None, "within the second");
        let file = icon.due(t0 + Duration::from_secs(1), || true).expect("capture once ready");
        assert_eq!(file, dir.join("icon.png"));
        std::fs::write(&file, b"png").expect("icon written");
        assert_eq!(icon.due(t0 + Duration::from_secs(2), || panic!("the file is there")), None);
        assert!(icon.present);
        assert_eq!(icon.due(t0 + Duration::from_secs(9), || true), None, "no more attempts");
        // Outside a singleplayer world: never.
        assert_eq!(WorldIcon::default().due(t0, || true), None);
    }

    /// The centred square of a wide frame, reduced by area means, channels put back in order.
    #[test]
    fn icon_is_the_centred_square_reduced() {
        // 192 × 128 BGRA frame, rows padded to 1024 bytes: left and right bands of 32 px in blue,
        // the centred 128 × 128 square in red (top half) and green (bottom half).
        let (w, h, row) = (192u32, 128u32, 1024usize);
        let mut data = vec![0u8; row * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let rgb = match (x, y) {
                    (x, _) if !(32..160).contains(&x) => [0, 0, 255],
                    (_, y) if y < 64 => [255, 0, 0],
                    _ => [0, 255, 0],
                };
                data[y * row + x * 4..y * row + x * 4 + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
            }
        }
        let icon = icon_pixels(&data, row, w, h, true);
        assert_eq!(icon.len(), 64 * 64 * 4);
        let px = |x: usize, y: usize| &icon[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert_eq!(px(0, 0), [255, 0, 0, 255]);
        assert_eq!(px(63, 31), [255, 0, 0, 255]);
        assert_eq!(px(0, 32), [0, 255, 0, 255]);
        // A square frame smaller than the icon still fills it.
        let tiny = icon_pixels(&[10, 20, 30, 0], 4, 1, 1, false);
        assert_eq!(&tiny[..4], &[10, 20, 30, 255]);
    }
}
