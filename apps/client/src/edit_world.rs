//! "Edit World" (`docs/specs/world-screens.md` § 3): rename, reset the icon,
//! open the folder, zip backup into `backups`, backups folder, optimization
//! (backup offered, then rewriting of the chunks with progress).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rc_text::StyledChar;

use crate::gui::Gui;
use crate::options::Options;
use crate::widgets::{self, Event, Kind, Panel, TextField, Widget, chars, tr};
use crate::world_list::{After, BackupConfirm, In, Job, Msg, Scr, Summary, background, colored, feed, t};

const NAME: u32 = 1;
const RESET_ICON: u32 = 2;
const FOLDER: u32 = 3;
const BACKUP: u32 = 4;
const BACKUP_FOLDER: u32 = 5;
const OPTIMIZE: u32 = 6;
const SAVE: u32 = 7;
const CANCEL: u32 = 8;

pub struct EditWorld {
    pub panel: Panel,
    size: (i32, i32),
    dir: PathBuf,
    name: String,
    /// World lock held during editing (released when the screen closes).
    _lock: rc_sim::lock::SessionLock,
    top: i32,
}

impl EditWorld {
    /// World locked for editing (refused if it is open elsewhere).
    pub fn new(s: &Summary) -> Result<EditWorld, String> {
        let lock = rc_sim::lock::SessionLock::take(&s.dir).map_err(|e| e.to_string())?;
        let level = rc_sim::level::Level::read(&s.dir).map_err(|e| e.to_string())?;
        let name = if level.name.is_empty() { s.id.clone() } else { level.name };
        Ok(EditWorld { panel: Panel::default(), size: (-1, -1), dir: s.dir.clone(), name, _lock: lock, top: 0 })
    }

    fn value(&self) -> String {
        match self.panel.get(NAME).map(|w| &w.kind) {
            Some(Kind::Field(f)) => f.value(),
            _ => self.name.clone(),
        }
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 == -1;
        if !first {
            self.name = self.value();
        }
        self.size = size;
        let (w, h) = size;
        // Column of 200, spacing 5, centered: 20 + 9 + 20 + 5 × 20 + 20 + 20 and 9 gaps.
        let x = (w - 200) / 2;
        let top = (h - 234) / 2;
        self.top = top;
        let mut f = TextField::new(Vec::new());
        f.text = self.name.chars().collect();
        f.cursor = f.text.len();
        f.anchor = f.cursor;
        let field = Widget::new(NAME, Kind::Field(Box::new(f)), x, top + 39, 200, 20, Vec::new());
        let mut reset = Widget::button(RESET_ICON, x, top + 64, 200, &t(gui, "selectWorld.edit.resetIcon"));
        reset.active = self.dir.join("icon.png").is_file();
        let mut save = Widget::button(SAVE, x, top + 214, 98, &t(gui, "selectWorld.edit.save"));
        save.active = !self.name.trim().is_empty();
        let focus = self.panel.focus;
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(vec![
            field,
            reset,
            Widget::button(FOLDER, x, top + 89, 200, &t(gui, "selectWorld.edit.openFolder")),
            Widget::button(BACKUP, x, top + 114, 200, &t(gui, "selectWorld.edit.backup")),
            Widget::button(BACKUP_FOLDER, x, top + 139, 200, &t(gui, "selectWorld.edit.backupFolder")),
            Widget::button(OPTIMIZE, x, top + 164, 200, &t(gui, "selectWorld.edit.optimize")),
            save,
            Widget::button(CANCEL, x + 102, top + 214, 98, &t(gui, "gui.cancel")),
        ]);
        self.panel.keyboard = keyboard;
        if first {
            self.panel.focus_widget(NAME);
        } else {
            self.panel.focus = focus;
        }
    }

    fn rename(&self) -> Vec<Msg> {
        let name = self.value();
        if let Err(e) = rc_sim::storage::rename(&self.dir, &name) {
            eprintln!("renaming {}: {e}", self.dir.display());
            let id = self.dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            return vec![
                Msg::Out(crate::world_list::Out::toast(
                    rc_text::Component::translate("selectWorld.access_failure", vec![]),
                    Some(rc_text::Component::text(id)),
                )),
                Msg::ToList,
            ];
        }
        vec![Msg::ToList]
    }

    pub(crate) fn input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Msg> {
        self.layout(gui);
        // Enter in the name field: save.
        if let In::Key(k @ (winit::keyboard::KeyCode::Enter | winit::keyboard::KeyCode::NumpadEnter), _) = i
            && self.panel.focus.and_then(|f| self.panel.widgets.get(f)).is_some_and(|w| w.id == NAME)
        {
            let _ = k;
            return self.rename();
        }
        let (ev, escape) = feed(&mut self.panel, i);
        let mut out = Vec::new();
        if escape {
            out.push(Msg::ToList);
        }
        for e in ev {
            match e {
                Event::Edit(NAME) => {
                    let blank = self.value().trim().is_empty();
                    if let Some(s) = self.panel.get_mut(SAVE) {
                        s.active = !blank;
                    }
                }
                Event::Press(RESET_ICON) => {
                    let _ = std::fs::remove_file(self.dir.join("icon.png"));
                    if let Some(b) = self.panel.get_mut(RESET_ICON) {
                        b.active = false;
                    }
                }
                Event::Press(FOLDER) => crate::links::open_file(&self.dir.to_string_lossy()),
                Event::Press(BACKUP) => out.push(Msg::Job(Job::Backup(self.dir.clone(), After::ToList))),
                Event::Press(BACKUP_FOLDER) => {
                    let d = crate::servers::game_dir().join("backups");
                    let _ = std::fs::create_dir_all(&d);
                    crate::links::open_file(&d.to_string_lossy());
                }
                Event::Press(OPTIMIZE) => {
                    out.push(Msg::Push(Scr::Backup(Box::new(BackupConfirm::optimize(gui, &self.dir)))));
                }
                Event::Press(SAVE) => out.extend(self.rename()),
                Event::Press(CANCEL) => out.push(Msg::ToList),
                _ => {}
            }
        }
        out
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let w = gui.width as i32;
        widgets::centered(gui, q, &chars(&t(gui, "selectWorld.edit.title")), w / 2, 15, 0xffff_ffff);
        let label = colored(&t(gui, "selectWorld.enterName"), 0xaa_aaaa);
        gui.text(q, &label, ((w - 200) / 2) as f32, (self.top + 25) as f32, 0xffff_ffff, true, 0);
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Optimization ----------

const O_CANCEL: u32 = 1;

pub struct Optimizing {
    pub panel: Panel,
    size: (i32, i32),
    title: String,
    progress: Arc<rc_sim::storage::Optimize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Optimizing {
    pub fn new(dir: &Path, erase: bool) -> Optimizing {
        let name = rc_sim::level::Level::read(dir).map(|l| l.name).unwrap_or_default();
        let progress = Arc::new(rc_sim::storage::Optimize::default());
        let p = progress.clone();
        let d = dir.to_owned();
        let thread = std::thread::Builder::new()
            .name("world-optimization".into())
            .spawn(move || rc_sim::storage::optimize(&d, erase, &p))
            .ok();
        Optimizing { panel: Panel::default(), size: (-1, -1), title: name, progress, thread }
    }

    /// End of the work: back to the list.
    pub(crate) fn tick(&mut self) -> Vec<Msg> {
        if self.thread.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            return vec![Msg::ToList];
        }
        Vec::new()
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        self.panel = Panel::new(vec![Widget::button(O_CANCEL, w / 2 - 100, h / 4 + 150, 200, &t(gui, "gui.cancel"))]);
    }

    pub(crate) fn input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Msg> {
        self.layout(gui);
        let (ev, _) = feed(&mut self.panel, i);
        if ev.contains(&Event::Press(O_CANCEL)) {
            self.progress.cancel.store(true, Ordering::Release);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            return vec![Msg::ToList];
        }
        Vec::new()
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let (w, h) = (gui.width as i32, gui.height as i32);
        let p = &self.progress;
        let title = chars(&tr(&gui.lang, "optimizeWorld.title", std::slice::from_ref(&self.title)));
        widgets::centered(gui, q, &title, w / 2, 20, 0xffff_ffff);
        let (x0, x1) = (w / 2 - 150, w / 2 + 150);
        let y0 = h / 4 + 100;
        let y1 = y0 + 10;
        let status = match p.stage.load(Ordering::Acquire) {
            0 => "optimizeWorld.stage.counting",
            1 => "optimizeWorld.stage.upgrading",
            4 => "optimizeWorld.stage.failed",
            _ => "optimizeWorld.stage.finished",
        };
        const GRAY: u32 = 0xffa0_a0a0;
        widgets::centered(gui, q, &chars(&t(gui, status)), w / 2, y0 - 9 - 2, GRAY);
        let total = p.total.load(Ordering::Relaxed);
        if total > 0 {
            let f = |v: i32| v as f32;
            gui.fill(f(x0 - 1), f(y0 - 1), f(x1 + 1), f(y1 + 1), 0xff00_0000);
            let (conv, skip) = (p.converted.load(Ordering::Relaxed), p.skipped.load(Ordering::Relaxed));
            let line = |gui: &mut Gui, key: &str, v: u32, y: i32| {
                let s: Vec<StyledChar> = chars(&tr(&gui.lang, key, &[v.to_string()]));
                gui.text(q, &s, x0 as f32, y as f32, GRAY, true, 0);
            };
            line(gui, "optimizeWorld.info.converted", conv, 40);
            line(gui, "optimizeWorld.info.skipped", skip, 40 + 12);
            line(gui, "optimizeWorld.info.total", total, 40 + 24);
            // Bar: one color per dimension, length according to its progress.
            let colors = [0xff33_6622, 0xff66_4433, 0xff77_8855];
            let mut x = 0;
            for (k, c) in colors.iter().enumerate() {
                let n = p.per_dim[k].load(Ordering::Relaxed);
                if n == 0 {
                    continue;
                }
                let dp = p.done[k].load(Ordering::Relaxed) as f32 / total as f32;
                let len = (dp * (x1 - x0) as f32).floor() as i32;
                gui.fill(f(x0 + x), f(y0), f(x0 + x + len), f(y1), *c);
                x += len;
            }
            let done = conv + skip;
            let counter =
                chars(&tr(&gui.lang, "optimizeWorld.progress.counter", &[done.to_string(), total.to_string()]));
            let pct = (done as f32 / total as f32 * 100.0).floor() as i32;
            let percent = chars(&tr(&gui.lang, "optimizeWorld.progress.percentage", &[pct.to_string()]));
            widgets::centered(gui, q, &counter, w / 2, y0 + 2 * 9 + 2, GRAY);
            widgets::centered(gui, q, &percent, w / 2, y0 + (y1 - y0) / 2 - 9 / 2, GRAY);
        }
        self.panel.draw(gui, q, mouse, time);
    }
}
