//! Screenshot (key `key.screenshot`, F2): window image as PNG in
//! `<game folder>/screenshots/<date>_<time>.png` (local time, `_2`, `_3`… if the name is
//! taken), written off the render thread, then message "screenshot.success" with the underlined name
//! that opens the file on click (or "screenshot.failure").
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rc_text::{Arg, ClickEvent, Component};

use crate::App;

/// Write results waiting to be announced (write thread → tick).
static DONE: Mutex<Vec<Result<PathBuf, String>>> = Mutex::new(Vec::new());

/// Local time (year, month, day, hour, minute, second).
pub(crate) fn local_now() -> (u16, u16, u16, u16, u16, u16) {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            millis: u16,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLocalTime(t: *mut SystemTime);
        }
        let mut t = SystemTime::default();
        // SAFETY: the structure has the expected layout and lives during the call.
        unsafe { GetLocalTime(&mut t) };
        (t.year, t.month, t.day, t.hour, t.minute, t.second)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let (y, m, d) = crate::stack::civil(secs.div_euclid(86_400));
        let r = secs.rem_euclid(86_400);
        (y as u16, m as u16, d as u16, (r / 3600) as u16, (r / 60 % 60) as u16, (r % 60) as u16)
    }
}

/// First free name `yyyy-MM-dd_HH.mm.ss[_n].png` in the folder.
pub fn free_name(dir: &Path, stamp: (u16, u16, u16, u16, u16, u16)) -> PathBuf {
    let (y, mo, d, h, mi, s) = stamp;
    let base = format!("{y:04}-{mo:02}-{d:02}_{h:02}.{mi:02}.{s:02}");
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{base}.png") } else { format!("{base}_{n}.png") }))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{base}.png")))
}

/// Pixels read from the surface (rows of `row` bytes, BGRA or RGBA) → opaque PNG, written on
/// another thread.
pub fn save(path: PathBuf, data: Vec<u8>, row: usize, w: u32, h: u32, bgra: bool) {
    std::thread::spawn(move || {
        let res = encode(&path, &data, row, w, h, bgra).map(|()| path).map_err(|e| e.to_string());
        if let Ok(mut d) = DONE.lock() {
            d.push(res);
        }
    });
}

fn encode(path: &Path, data: &[u8], row: usize, w: u32, h: u32, bgra: bool) -> std::io::Result<()> {
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        for p in data[y * row..y * row + w as usize * 4].as_chunks::<4>().0 {
            let (r, b) = if bgra { (p[2], p[0]) } else { (p[0], p[2]) };
            px.extend_from_slice(&[r, p[1], b, 0xFF]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(&px).map_err(std::io::Error::other)?;
    wr.finish().map_err(std::io::Error::other)
}

/// Game message for a successful capture: underlined name, click = open the file.
fn success(path: &Path) -> Component {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut link = Component::text(name);
    link.style.underlined = Some(true);
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    link.style.click = Some(std::sync::Arc::new(ClickEvent {
        action: "open_file".into(),
        value: abs.to_string_lossy().into_owned(),
        nbt: None,
    }));
    Component::translate("screenshot.success", vec![Arg::Component(link)])
}

impl App {
    /// Capture key: the current frame is read at the end of its render (`render.rs`).
    pub(crate) fn take_screenshot(&mut self) {
        let dir = crate::servers::game_dir().join("screenshots");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.screenshot_message(Err(e.to_string()));
            return;
        }
        let path = free_name(&dir, local_now());
        if let Some(r) = &mut self.renderer {
            r.request_capture(path);
        }
    }

    /// Captures written since the last tick: message in the chat.
    pub(crate) fn screenshot_poll(&mut self) {
        let done = match DONE.lock() {
            Ok(mut d) if !d.is_empty() => std::mem::take(&mut *d),
            _ => return,
        };
        for r in done {
            self.screenshot_message(r);
        }
    }

    fn screenshot_message(&mut self, r: Result<PathBuf, String>) {
        let c = match &r {
            Ok(p) => success(p),
            Err(e) => Component::translate("screenshot.failure", vec![Arg::Text(e.clone())]),
        };
        match &r {
            Ok(p) => eprintln!("screenshot: {}", p.display()),
            Err(e) => eprintln!("screenshot failed: {e}"),
        }
        self.chat.add_client(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_game_and_skip_taken_ones() {
        let dir = std::env::temp_dir().join(format!("rc-shot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = (2026, 10, 12, 9, 5, 3);
        let a = free_name(&dir, t);
        assert_eq!(a.file_name().unwrap(), "2026-10-12_09.05.03.png");
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(free_name(&dir, t).file_name().unwrap(), "2026-10-12_09.05.03_2.png");
        // Readable PNG, channels put back in order.
        let p = dir.join("t.png");
        let row = 256;
        let mut data = vec![0u8; row * 2];
        data[..4].copy_from_slice(&[10, 20, 30, 0]);
        encode(&p, &data, row, 1, 2, true).unwrap();
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&p).unwrap()));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        r.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..4], &[30, 20, 10, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
