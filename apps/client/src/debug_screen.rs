//! F3 debug screen (`docs/specs/debug-f3.md`): entries and statuses, profiles,
//! `debug-profile.json`, line collection, two-column layout, drawing.
//!
//! Cost: nothing is collected or drawn while no text entry is active (F3 closed,
//! default profile). Open: about thirty lines formatted per frame; the formatting
//! (styled characters, width) is redone only for the lines that changed.

use hashbrown::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use rc_text::StyledChar;

use crate::gui::Gui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    AlwaysOn,
    InOverlay,
    Never,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::AlwaysOn => "alwaysOn",
            Status::InOverlay => "inOverlay",
            Status::Never => "never",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "alwaysOn" => Some(Status::AlwaysOn),
            "inOverlay" => Some(Status::InOverlay),
            "never" => Some(Status::Never),
            _ => None,
        }
    }
}

/// Entry: id path, text (otherwise 3D rendering), allowed in reduced info.
pub struct EntryDef {
    pub id: &'static str,
    pub text: bool,
    pub reduced: bool,
}

const fn t(id: &'static str, reduced: bool) -> EntryDef {
    EntryDef { id, text: true, reduced }
}

const fn r(id: &'static str) -> EntryDef {
    EntryDef { id, text: false, reduced: false }
}

/// All entries, sorted by path (call order, § 1.3).
pub const ENTRIES: &[EntryDef] = &[
    r("3d_crosshair"),
    t("biome", false),
    r("chunk_borders"),
    t("chunk_generation_stats", false),
    t("chunk_render_stats", true),
    r("chunk_section_octree"),
    r("chunk_section_paths"),
    r("chunk_section_visibility"),
    t("chunk_source_stats", true),
    t("day_count", false),
    t("detailed_memory", true),
    r("entity_hitboxes"),
    t("entity_render_stats", true),
    t("entity_spawn_counts", false),
    t("fps", true),
    t("game_version", true),
    t("gpu_utilization", true),
    t("heightmap", false),
    t("light_levels", false),
    t("local_difficulty", false),
    t("looking_at_block_state", false),
    t("looking_at_block_tags", false),
    t("looking_at_entity", false),
    t("looking_at_entity_tags", false),
    t("looking_at_fluid_state", false),
    t("looking_at_fluid_tags", false),
    t("memory", true),
    t("particle_render_stats", false),
    t("player_position", false),
    t("player_section_position", true),
    t("player_speed", false),
    t("post_effects", false),
    t("simple_performance_impactors", true),
    t("sound_cache", true),
    t("sound_mood", false),
    t("system_specs", true),
    t("tps", true),
    r("visualize_block_light_levels"),
    r("visualize_chunks_on_server"),
    r("visualize_collision_boxes"),
    r("visualize_entity_supporting_blocks"),
    r("visualize_heightmap"),
    r("visualize_sky_light_levels"),
    r("visualize_sky_light_sections"),
    r("visualize_solid_faces"),
    r("visualize_water_levels"),
];

/// Index of an entry by its path (`fps`) or its id (`minecraft:fps`).
pub fn index(id: &str) -> Option<usize> {
    let path = id.strip_prefix("minecraft:").unwrap_or(id);
    ENTRIES.binary_search_by(|e| e.id.cmp(path)).ok()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Default,
    Performance,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::Default => "default",
            Profile::Performance => "performance",
        }
    }

    /// Profile statuses (§ 1.4); the rest `never`.
    fn statuses(self) -> &'static [(&'static str, Status)] {
        use Status::{AlwaysOn, InOverlay};
        match self {
            Profile::Default => &[
                ("3d_crosshair", InOverlay),
                ("game_version", InOverlay),
                ("fps", InOverlay),
                ("tps", InOverlay),
                ("memory", InOverlay),
                ("system_specs", InOverlay),
                ("player_position", InOverlay),
                ("player_section_position", InOverlay),
                ("simple_performance_impactors", InOverlay),
            ],
            Profile::Performance => &[
                ("fps", AlwaysOn),
                ("tps", InOverlay),
                ("memory", InOverlay),
                ("simple_performance_impactors", InOverlay),
                ("gpu_utilization", InOverlay),
            ],
        }
    }
}

/// Charts: pie chart (F3+1), frames (2), network (3), light map (4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chart {
    Profiler = 0,
    Fps = 1,
    Network = 2,
    Lightmap = 3,
}

/// Lines collected for a frame (empty string: empty row).
#[derive(Default)]
pub struct Collector {
    left: Vec<String>,
    right: Vec<String>,
    normal: Vec<String>,
    groups: Vec<(&'static str, Vec<String>)>,
}

impl Collector {
    fn clear(&mut self) {
        self.left.clear();
        self.right.clear();
        self.normal.clear();
        self.groups.clear();
    }

    /// Priority line: on the left as long as the left is not longer.
    pub fn priority(&mut self, s: String) {
        if self.left.len() > self.right.len() {
            self.right.push(s);
        } else {
            self.left.push(s);
        }
    }

    pub fn line(&mut self, s: String) {
        self.normal.push(s);
    }

    /// Named group (created empty if needed, order of first appearance).
    pub fn group(&mut self, name: &'static str) -> &mut Vec<String> {
        let i = match self.groups.iter().position(|g| g.0 == name) {
            Some(i) => i,
            None => {
                self.groups.push((name, Vec::new()));
                self.groups.len() - 1
            }
        };
        &mut self.groups[i].1
    }

    /// Assembly of the columns (§ 2.3), footer not included.
    fn assemble(&mut self) {
        if !self.left.is_empty() {
            self.left.push(String::new());
        }
        if !self.right.is_empty() {
            self.right.push(String::new());
        }
        if !self.normal.is_empty() {
            let n = self.normal.len();
            let mid = n.div_ceil(2);
            let rest = self.normal.split_off(mid);
            self.left.append(&mut self.normal);
            self.left.push(String::new());
            let any = !rest.is_empty();
            self.right.extend(rest);
            if any {
                self.right.push(String::new());
            }
        }
        let g = self.groups.len();
        let mid = g.div_ceil(2);
        for (i, (_, lines)) in self.groups.drain(..).enumerate() {
            if lines.is_empty() {
                continue;
            }
            let col = if i < mid { &mut self.left } else { &mut self.right };
            col.extend(lines);
            col.push(String::new());
        }
    }
}

/// Ring of chart samples (240 at most).
pub struct Ring<T: Copy + Default> {
    pub data: Vec<T>,
    start: usize,
}

pub const RING: usize = 240;

impl<T: Copy + Default> Default for Ring<T> {
    fn default() -> Self {
        Ring { data: Vec::new(), start: 0 }
    }
}

impl<T: Copy + Default> Ring<T> {
    pub fn push(&mut self, v: T) {
        if self.data.len() < RING {
            self.data.push(v);
        } else {
            self.data[self.start] = v;
            self.start = (self.start + 1) % RING;
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Sample `i` counting from the oldest.
    pub fn get(&self, i: usize) -> T {
        self.data[(self.start + i) % self.data.len().max(1)]
    }
}

/// Formatted line kept from one frame to the next.
struct CachedLine {
    text: String,
    chars: Vec<StyledChar>,
    width: f32,
}

pub struct DebugScreen {
    status: Vec<Status>,
    /// Unknown ids read from the file: kept as they are.
    unknown: Vec<(String, String)>,
    pub profile: Option<Profile>,
    overlay: bool,
    /// Background blur of the F3+F6 screen (option `menuBackgroundBlurriness`).
    pub menu_blur: i32,
    server_reduced: bool,
    local_reduced: bool,
    active: Vec<bool>,
    /// At least one active entry, text or not (the game then draws charts and footer).
    any_active: bool,
    /// Incremented on every rebuild of the active list.
    pub version: u64,
    path: Option<PathBuf>,
    pub charts: [bool; 4],
    /// Modifier (F3) held, and used by an action during the press.
    pub modifier_down: bool,
    pub modifier_used: bool,
    // Frames per second and frame times.
    frames: u32,
    pub fps: u32,
    fps_since: Instant,
    last_frame: Option<Instant>,
    pub frame_ns: Ring<u64>,
    /// GPU: GPU time / frame time (%), sampled every second.
    pub gpu_percent: f64,
    // Network: smoothed averages of packets per second.
    pub tx_avg: f32,
    pub rx_avg: f32,
    net_prev: Option<(u64, u64)>,
    // Memory: allocation rate.
    alloc_prev: Option<(u64, Instant)>,
    pub alloc_rate: u64,
    /// Slow sample (process memory, GPU memory), refreshed every second.
    pub slow: SlowStats,
    slow_at: Option<Instant>,
    cpu: Option<String>,
    pub collector: Collector,
    cache: [Vec<CachedLine>; 2],
    /// Permission level received (entity events 24 to 28).
    pub op_level: i32,
    pub simulation_distance: i32,
    /// Tags received: (registry, [(tag, ids)]).
    pub tags: rc_proto_v777::play::Tags,
    /// Keyboard-free driving: step, elapsed ticks.
    pub test: (usize, u32),
    /// F3+C held: start, last message, messages sent.
    pub crash: Option<(Instant, Instant, u32)>,
    pub profiling: Option<Profiling>,
    /// Current game mode seen at the last tick and previous mode (F3+N, F3+F4).
    pub current_mode: i32,
    pub previous_mode: Option<i32>,
    /// Game mode selector open (F3+F4).
    pub switcher: Option<crate::debug_switcher::Switcher>,
    /// Samples of the network and server charts.
    pub logs: crate::debug_charts::ChartLogs,
    /// Debug options screen open (F3+F6).
    pub options_screen: Option<crate::debug_options::OptionsScreen>,
}

/// F3+L recording in progress (`profiling_zip.rs`): one row of samples per frame
/// (frame time in ns, loaded sections, drawn sections, view distance, GPU %).
pub struct Profiling {
    pub start: Instant,
    /// Stop requested (F3+L during the recording): effective at the end of the frame.
    pub stop: bool,
    pub frame_start: Option<Instant>,
    pub rows: Vec<[f64; 5]>,
}

#[derive(Default, Clone, Copy)]
pub struct SlowStats {
    pub total: u64,
    pub commit: u64,
    pub working_set: u64,
    pub gpu_allocated: u64,
    pub gpu_reserved: u64,
}

const MIB: u64 = 1024 * 1024;

impl DebugScreen {
    /// Startup state: `debug-profile.json` of the game folder (missing: default profile;
    /// unreadable: default profile and file rewritten).
    pub fn new(dir: Option<PathBuf>) -> DebugScreen {
        let mut d = DebugScreen {
            status: vec![Status::Never; ENTRIES.len()],
            unknown: Vec::new(),
            profile: None,
            overlay: false,
            server_reduced: false,
            local_reduced: false,
            active: vec![false; ENTRIES.len()],
            any_active: false,
            version: 0,
            path: dir.map(|d| d.join("debug-profile.json")),
            charts: [false; 4],
            modifier_down: false,
            modifier_used: false,
            frames: 0,
            fps: 0,
            fps_since: Instant::now(),
            last_frame: None,
            frame_ns: Ring::default(),
            gpu_percent: 0.0,
            tx_avg: 0.0,
            rx_avg: 0.0,
            net_prev: None,
            alloc_prev: None,
            alloc_rate: 0,
            slow: SlowStats::default(),
            slow_at: None,
            cpu: None,
            collector: Collector::default(),
            cache: [Vec::new(), Vec::new()],
            op_level: 0,
            simulation_distance: 0,
            tags: Vec::new(),
            test: (0, 0),
            crash: None,
            profiling: None,
            current_mode: -1,
            previous_mode: None,
            switcher: None,
            logs: Default::default(),
            options_screen: None,
            menu_blur: 5,
        };
        let text = d.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        match text {
            None => d.load_profile_quiet(Profile::Default),
            Some(t) => {
                if !d.parse(&t) {
                    d.load_profile_quiet(Profile::Default);
                    d.save();
                }
            }
        }
        d
    }

    /// Reading the file; false if unreadable.
    fn parse(&mut self, text: &str) -> bool {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return false };
        if let Some(p) = v.get("profile").and_then(|p| p.as_str()) {
            let p = match p {
                "default" => Profile::Default,
                "performance" => Profile::Performance,
                _ => return false,
            };
            self.load_profile_quiet(p);
            return true;
        }
        let Some(custom) = v.get("custom").and_then(|c| c.as_object()) else { return false };
        self.status.fill(Status::Never);
        self.unknown.clear();
        for (k, s) in custom {
            let Some(st) = s.as_str() else { continue };
            match (index(k), Status::parse(st)) {
                (Some(i), Some(st)) => self.status[i] = st,
                _ => self.unknown.push((k.clone(), st.to_owned())),
            }
        }
        self.profile = None;
        self.rebuild();
        true
    }

    /// Compact JSON (§ 5.2) with the data version.
    pub fn to_json(&self) -> String {
        let mut m = serde_json::Map::new();
        match self.profile {
            Some(p) => {
                m.insert("profile".into(), p.name().into());
            }
            None => {
                let mut c = serde_json::Map::new();
                for (i, e) in ENTRIES.iter().enumerate() {
                    if self.status[i] != Status::Never {
                        c.insert(format!("minecraft:{}", e.id), self.status[i].name().into());
                    }
                }
                for (k, v) in &self.unknown {
                    c.insert(k.clone(), v.clone().into());
                }
                m.insert("custom".into(), c.into());
            }
        }
        m.insert("DataVersion".into(), 5023.into());
        serde_json::Value::Object(m).to_string()
    }

    pub fn save(&self) {
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(p, self.to_json()) {
                eprintln!("debug-profile.json : {e}");
            }
        }
    }

    fn load_profile_quiet(&mut self, p: Profile) {
        self.status.fill(Status::Never);
        self.unknown.clear();
        for &(id, s) in p.statuses() {
            if let Some(i) = index(id) {
                self.status[i] = s;
            }
        }
        self.profile = Some(p);
        self.rebuild();
    }

    /// Profile chosen (options screen): applied and saved.
    pub fn load_profile(&mut self, p: Profile) {
        self.load_profile_quiet(p);
        self.save();
    }

    pub fn status_of(&self, i: usize) -> Status {
        self.status[i]
    }

    /// Status set by hand: custom profile, save.
    pub fn set_status(&mut self, i: usize, s: Status) {
        self.status[i] = s;
        self.profile = None;
        self.rebuild();
        self.save();
    }

    /// Quick toggle (§ 1.5): returns true if the entry is now shown.
    pub fn toggle(&mut self, id: &str) -> bool {
        let Some(i) = index(id) else { return false };
        let (s, shown) = match (self.status[i], self.overlay) {
            (Status::AlwaysOn, _) | (Status::InOverlay, true) => (Status::Never, false),
            (Status::InOverlay, false) | (Status::Never, false) => (Status::AlwaysOn, true),
            (Status::Never, true) => (Status::InOverlay, true),
        };
        self.set_status(i, s);
        shown
    }

    pub fn overlay(&self) -> bool {
        self.overlay
    }

    pub fn set_overlay(&mut self, on: bool) {
        if self.overlay != on {
            self.overlay = on;
            self.rebuild();
        }
    }

    /// Reduced info: server flag or local option.
    pub fn set_reduced(&mut self, server: Option<bool>, local: Option<bool>) {
        let before = self.reduced();
        if let Some(s) = server {
            self.server_reduced = s;
        }
        if let Some(l) = local {
            self.local_reduced = l;
        }
        if self.reduced() != before {
            self.rebuild();
        }
    }

    /// Entry into a game: simulation distance and reduced info announced by the server.
    pub fn joined(&mut self, simulation_distance: i32, reduced: bool) {
        self.simulation_distance = simulation_distance;
        self.set_reduced(Some(reduced), None);
    }

    /// Entity event of the local player: reduced info (22 yes, 23 no), permission (24 to 28).
    pub fn player_event(&mut self, event: i32) {
        match event {
            22 => self.set_reduced(Some(true), None),
            23 => self.set_reduced(Some(false), None),
            24..=28 => self.op_level = event - 24,
            _ => {}
        }
    }

    /// Tags received: replace those of the same registries.
    pub fn set_tags(&mut self, tags: &rc_proto_v777::play::Tags) {
        for (reg, list) in tags {
            match self.tags.iter_mut().find(|t| &t.0 == reg) {
                Some(t) => t.1.clone_from(list),
                None => self.tags.push((reg.clone(), list.clone())),
            }
        }
    }

    pub fn reduced(&self) -> bool {
        self.server_reduced || self.local_reduced
    }

    fn rebuild(&mut self) {
        let reduced = self.reduced();
        for (i, e) in ENTRIES.iter().enumerate() {
            let on = match self.status[i] {
                Status::AlwaysOn => true,
                Status::InOverlay => self.overlay,
                Status::Never => false,
            };
            self.active[i] = on && (!reduced || e.reduced);
        }
        self.any_active = self.active.iter().any(|&a| a);
        self.version += 1;
        crate::debug_sys::COUNT_ALLOCS
            .store(self.is_on("memory") || self.is_on("detailed_memory"), std::sync::atomic::Ordering::Relaxed);
    }

    /// Active entry (status, overlay, reduced info).
    pub fn is_on(&self, id: &str) -> bool {
        index(id).is_some_and(|i| self.active[i])
    }

    /// At least one active entry, whichever it is (rendering only included).
    pub fn any_active(&self) -> bool {
        self.any_active
    }

    /// Chart shown (overlay visible and chart chosen).
    pub fn chart(&self, c: Chart) -> bool {
        self.overlay && self.charts[c as usize]
    }

    /// Toggle of a chart (§ 6.3): overlay hidden → forces the display; frames, network and
    /// light map are mutually exclusive.
    pub fn toggle_chart(&mut self, c: Chart) {
        let i = c as usize;
        self.charts[i] = !self.overlay || !self.charts[i];
        if self.charts[i] {
            self.set_overlay(true);
            if c != Chart::Profiler {
                for o in [Chart::Fps, Chart::Network, Chart::Lightmap] {
                    if o != c {
                        self.charts[o as usize] = false;
                    }
                }
            }
        }
    }

    /// Start of frame: frame counter (reset every second), previous frame
    /// time, network averages and slow samples.
    pub fn on_frame(&mut self, now: Instant, net: &rc_client::netstats::NetStats) {
        if let Some(last) = self.last_frame {
            let ns = now.duration_since(last).as_nanos() as u64;
            self.frame_ns.push(ns);
        }
        self.last_frame = Some(now);
        // Ping delays already measured when the replies arrive (like the game).
        for d in net.take_pongs() {
            self.logs.ping.push(d);
        }
        self.frames += 1;
        if now.duration_since(self.fps_since).as_millis() >= 1000 {
            self.fps = self.frames;
            self.frames = 0;
            self.fps_since = now;
            let o = std::sync::atomic::Ordering::Relaxed;
            let (s, r) = (net.sent.load(o), net.received.load(o));
            if let Some((ps, pr)) = self.net_prev {
                self.tx_avg = 0.25 * (s - ps) as f32 + 0.75 * self.tx_avg;
                self.rx_avg = 0.25 * (r - pr) as f32 + 0.75 * self.rx_avg;
            }
            self.net_prev = Some((s, r));
        }
    }

    /// GPU measurement of the elapsed second (ms per frame) relative to the average frame time.
    pub fn set_gpu(&mut self, gpu_ms: Option<f64>) {
        let frame_ms = if self.fps > 0 { 1000.0 / f64::from(self.fps) } else { 0.0 };
        self.gpu_percent = match gpu_ms {
            Some(g) if frame_ms > 0.0 => g / frame_ms * 100.0,
            _ => 0.0,
        };
    }

    pub(crate) fn cpu(&mut self) -> &str {
        self.cpu.get_or_insert_with(|| {
            let n = std::thread::available_parallelism().map_or(1, |n| n.get());
            format!("{n}x {}", crate::debug_sys::cpu_name())
        })
    }

    /// Slow sample: process and GPU memory, at most once per second.
    fn refresh_slow(&mut self, now: Instant, gpu: impl FnOnce() -> Option<(u64, u64)>) {
        if self.slow_at.is_some_and(|t| now.duration_since(t).as_millis() < 1000) {
            return;
        }
        self.slow_at = Some(now);
        if self.slow.total == 0 {
            self.slow.total = crate::debug_sys::total_memory();
        }
        (self.slow.commit, self.slow.working_set) = crate::debug_sys::process_memory();
        if let Some((a, r)) = gpu() {
            (self.slow.gpu_allocated, self.slow.gpu_reserved) = (a, r);
        }
    }

    /// Allocation rate, recomputed at most every 500 ms.
    fn refresh_alloc(&mut self, now: Instant) {
        let total = crate::debug_sys::ALLOCATED.load(std::sync::atomic::Ordering::Relaxed);
        match self.alloc_prev {
            Some((prev, at)) => {
                let dt = now.duration_since(at).as_secs_f64();
                if dt >= 0.5 {
                    self.alloc_rate = ((total.saturating_sub(prev)) as f64 / dt).round() as u64;
                    self.alloc_prev = Some((total, now));
                }
            }
            None => self.alloc_prev = Some((total, now)),
        }
    }

    /// `memory` or `detailed_memory` lines (§ 3.4, native sources § 9).
    fn memory_lines(
        &mut self,
        c: &mut Collector,
        detailed: bool,
        now: Instant,
        gpu: impl FnOnce() -> Option<(u64, u64)>,
    ) {
        self.refresh_slow(now, gpu);
        let s = self.slow;
        let max = s.total.max(1);
        if detailed {
            let g = c.group("memory");
            g.push(format!(
                "Memory (heap): i={:03}MiB u={:03}MiB c={:03}MiB m={:03}MiB",
                0,
                s.working_set / MIB,
                s.commit / MIB,
                s.total / MIB
            ));
            g.push(format!(
                "Memory (non-heap): i={:03}MiB u={:03}MiB c={:03}MiB m={:03}MiB",
                0,
                s.gpu_allocated / MIB,
                s.gpu_reserved / MIB,
                0
            ));
        } else {
            self.refresh_alloc(now);
            let rate = self.alloc_rate / MIB;
            let g = c.group("memory");
            g.push(format!("Mem: {:2}% {:03}/{:03}MiB", s.working_set * 100 / max, s.working_set / MIB, s.total / MIB));
            g.push(format!("Allocation rate: {rate:03}MiB/s"));
            g.push(format!("Allocated: {:2}% {:03}MiB", s.commit * 100 / max, s.commit / MIB));
        }
    }

    /// Formatting of a line, taken from the previous frame if the text has not changed.
    fn styled<'a>(cache: &'a mut Vec<CachedLine>, i: usize, text: &str, gui: &Gui) -> &'a CachedLine {
        if cache.len() <= i {
            cache.resize_with(i + 1, || CachedLine { text: String::new(), chars: Vec::new(), width: 0.0 });
        }
        let c = &mut cache[i];
        if c.text != text || (c.chars.is_empty() && !text.is_empty()) {
            c.text.clear();
            c.text.push_str(text);
            c.chars = crate::gui::plain(text);
            c.width = rc_text::width(gui, &c.chars) as f32;
        }
        &cache[i]
    }

    /// Own scale of the screen (§ 2.4): (placement factor, virtual width and height).
    pub fn scale(gui: &Gui, window: (u32, u32), option: i32) -> (f32, f32, f32) {
        let gs = gui.scale as i32;
        let max = crate::gui::auto_scale(window.0, window.1) as i32;
        let e = match option {
            -1 => gs,
            0 => max / 2,
            n => crate::gui::calculate_scale(n as u32, window.0, window.1) as i32,
        };
        if e > 0 && e < gs {
            (e as f32 / gs as f32, (window.0 as i32 / e) as f32, (window.1 as i32 / e) as f32)
        } else {
            (1.0, gui.width, gui.height)
        }
    }

    /// Drawing of the two columns (§ 2.5) in a layer above everything.
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, window: (u32, u32), scale_option: i32, max_fps: i32) {
        let (pose, width, height) = Self::scale(gui, window, scale_option);
        gui.next_layer();
        let old_pose = gui.pose;
        gui.pose = pose;
        let cols = [std::mem::take(&mut self.collector.left), std::mem::take(&mut self.collector.right)];
        for (side, lines) in cols.iter().enumerate() {
            let cache = &mut self.cache[side];
            for (i, line) in lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                let c = Self::styled(cache, i, line, gui);
                let x = if side == 0 { 2.0 } else { width - 2.0 - c.width };
                let y = 2.0 + 9.0 * i as f32;
                gui.fill(x - 1.0, y - 1.0, x + c.width + 1.0, y + 8.0, 0x9050_5050);
            }
            for (i, line) in lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                let c = &cache[i];
                let x = if side == 0 { 2.0 } else { width - 2.0 - c.width };
                let y = 2.0 + 9.0 * i as f32;
                gui.text(q, &c.chars, x, y, 0xFFE0_E0E0, false, 0);
            }
        }
        let [left, right] = cols;
        self.collector.left = left;
        self.collector.right = right;
        gui.next_layer();
        self.draw_charts(gui, q, width, height, max_fps);
        gui.pose = old_pose;
    }
}

/// Label of an action's key: `[F3+2]` (modifier omitted if not assigned).
pub fn keybind(o: &crate::options::Options, lang: &rc_text::Lang, action: &str) -> String {
    let m = o.s("key_key.debug.modifier");
    let k = o.s(&format!("key_key.debug.{action}"));
    let label = |n: &str| crate::options::key_label(lang, n);
    if m.is_empty() || m == "key.keyboard.unknown" {
        format!("[{}]", label(k))
    } else {
        format!("[{}+{}]", label(m), label(k))
    }
}

fn present_mode(m: wgpu::PresentMode) -> &'static str {
    match m {
        wgpu::PresentMode::Immediate => " (immediate)",
        wgpu::PresentMode::Mailbox => " (mailbox)",
        wgpu::PresentMode::Fifo => " (fifo)",
        wgpu::PresentMode::FifoRelaxed => " (fifo relaxed)",
        _ => "",
    }
}

fn device_type(t: wgpu::DeviceType) -> &'static str {
    match t {
        wgpu::DeviceType::IntegratedGpu => " (iGPU)",
        wgpu::DeviceType::DiscreteGpu => " (dGPU)",
        wgpu::DeviceType::VirtualGpu => " (vGPU)",
        wgpu::DeviceType::Cpu => " (software)",
        wgpu::DeviceType::Other => "",
    }
}

fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10DE => "NVIDIA Corporation",
        0x1002 | 0x1022 => "ATI Technologies Inc.",
        0x8086 => "Intel",
        0x13B5 => "ARM",
        0x5143 => "Qualcomm",
        0x106B => "Apple",
        _ => "<unknown>",
    }
}

fn backend_name(b: wgpu::Backend) -> &'static str {
    match b {
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Dx12 => "DX12",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Gl => "OpenGL",
        _ => "Unknown",
    }
}

/// Angle brought into [−180, 180).
pub(crate) fn wrap_degrees(a: f32) -> f32 {
    let mut v = a % 360.0;
    if v >= 180.0 {
        v -= 360.0;
    }
    if v < -180.0 {
        v += 360.0;
    }
    v
}

/// Height of a received heightmap (`heightmaps` of a column) at the local point (x, z).
fn heightmap_at(maps: &[(i32, Box<[i64]>)], kind: i32, x: i32, z: i32, height: i32, min_y: i32) -> Option<i32> {
    let (_, longs) = maps.iter().find(|h| h.0 == kind)?;
    let bits = (32 - (height as u32 + 1).saturating_sub(1).leading_zeros()).max(1);
    let per = 64 / bits;
    let i = (x.rem_euclid(16) + z.rem_euclid(16) * 16) as u32;
    let long = *longs.get((i / per) as usize)? as u64;
    let v = (long >> ((i % per) * bits)) & ((1u64 << bits) - 1);
    Some(v as i32 + min_y)
}

impl crate::App {
    /// Collection of the frame's lines (§ 2.2, § 3) in `self.debug.collector`; returns true if
    /// drawing is needed. Nothing is done without an active entry (text or not).
    pub(crate) fn debug_collect(&mut self, hit: crate::pick::Hit) -> bool {
        let now = Instant::now();
        let visible = !self.hud.hidden || self.screens.open || self.menus.is_open() || self.debug.options_open();
        if !self.debug.any_active() || !visible {
            return false;
        }
        // Integrated server: footer labels and bandwidth chart (§ 7).
        self.debug.logs.local = self.solo.is_some();
        self.debug.collector.clear();
        let Some(r) = self.renderer.as_ref() else { return false };
        let o = &self.options;
        let mut c = std::mem::take(&mut self.debug.collector);
        let report = || r.allocator_report().map(|a| (a.total_allocated_bytes, a.total_reserved_bytes));
        let feet = self.player.pos;
        let block = feet.map(|v| v.floor() as i32);
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        for (i, e) in ENTRIES.iter().enumerate() {
            if !e.text || !self.debug.active[i] {
                continue;
            }
            match e.id {
                "game_version" => {
                    let v = &self.data.version;
                    c.priority(format!("Minecraft {v} ({v}/vanilla)"));
                }
                "fps" => {
                    let limit = o.i("maxFps");
                    let hz = r.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).unwrap_or(0);
                    let hz =
                        if hz % 1000 == 0 { (hz / 1000).to_string() } else { format!("{:.2}", f64::from(hz) / 1000.0) };
                    c.priority(format!(
                        "{} fps T: {}{} @{hz}Hz",
                        self.debug.fps,
                        if limit >= 260 { "inf".to_owned() } else { limit.to_string() },
                        present_mode(r.present_mode)
                    ));
                }
                "tps" if self.positioned => {
                    // Cadence of the client's world (singleplayer included: `ticking_state` goes through the
                    // events, not through the network samples).
                    let t = &self.exact.rate;
                    let run = if t.steps > 0 {
                        " (frozen - stepping)"
                    } else if t.frozen {
                        " (frozen)"
                    } else {
                        ""
                    };
                    let (tx, rx) = (self.debug.tx_avg, self.debug.rx_avg);
                    if self.solo.is_some() {
                        // Integrated server: smoothed tick duration (method) and target.
                        let mspt = 1000.0 / t.rate.max(1.0);
                        let smooth = self.debug.logs.smoothed_tick_ms;
                        c.line(format!("Integrated server @ {smooth:.1}/{mspt:.1} ms{run}, {tx:.0} tx, {rx:.0} rx"));
                    } else {
                        let brand = self.session.link.stats.brand().unwrap_or_else(|| "null".into());
                        c.line(format!("\"{brand}\" server{run}, {tx:.0} tx, {rx:.0} rx"));
                    }
                }
                "detailed_memory" | "memory" => {
                    self.debug.memory_lines(&mut c, e.id == "detailed_memory", now, report);
                }
                "system_specs" => {
                    let info = &r.adapter_info;
                    let (pw, ph) = r.size();
                    let sf = r.window.scale_factor();
                    let (lw, lh) = ((f64::from(pw) / sf).round(), (f64::from(ph) / sf).round());
                    let cpu = self.debug.cpu().to_owned();
                    let g = c.group("system");
                    g.push(format!("Rust: {}", env!("RC_RUSTC_VERSION")));
                    g.push(format!("CPU: {cpu}"));
                    g.push(format!("Display: {pw}x{ph} ({})", vendor_name(info.vendor)));
                    g.push(format!("Window: {lw}x{lh} ({sf:.2}x pixel density)"));
                    g.push(format!("{}{}", info.name, device_type(info.device_type)));
                    let driver = format!("{} {}", info.driver, info.driver_info);
                    g.push(format!("{} {}", backend_name(info.backend), driver.lines().next().unwrap_or("").trim()));
                }
                "gpu_utilization" => {
                    let p = self.debug.gpu_percent;
                    c.line(if p > 100.0 { "GPU: §c100%".into() } else { format!("GPU: {}%", p.round() as i64) });
                }
                "simple_performance_impactors" => {
                    let clouds = match o.s("renderClouds") {
                        "false" => "",
                        "fast" => "fast-clouds ",
                        _ => "fancy-clouds ",
                    };
                    let tr = if o.b("improvedTransparency") { "improved-transparency " } else { "" };
                    c.line(format!("{tr}{clouds}B: {}", o.i("biomeBlendRadius")));
                    let lang = &r.gui.lang;
                    let name = |k: &str| crate::widgets::tr(lang, &format!("options.textureFiltering.{k}"), &[]);
                    c.line(match o.i("textureFiltering") {
                        1 => format!("Filtering: {}", name("rgss")),
                        2 => {
                            format!("Filtering: {} {}x", name("anisotropic"), 1 << o.i("maxAnisotropyBit").clamp(0, 4))
                        }
                        _ => format!("Filtering: {}", name("none")),
                    });
                    c.line(format!(
                        "Terrain Rendering: {}",
                        if r.uses_indirect() { "multidrawindirect" } else { "naive" }
                    ));
                }
                "chunk_render_stats" => {
                    let sections = (self.world.height / 16).max(1) as usize;
                    let dist = o.i("renderDistance").min(self.env.server_view.max(2));
                    let free = rayon::current_num_threads().saturating_sub(self.in_flight);
                    c.line(format!(
                        "C: {}/{} {}D: {dist}, pC: {:03}, aB: {:02}",
                        r.drawn.0,
                        self.world.grid.len() * sections,
                        if r.occlusion { "(s) " } else { "" },
                        self.in_flight,
                        free
                    ));
                }
                "entity_render_stats" => {
                    let drawn = r.entities.as_ref().map_or(0, |e| e.drawn);
                    c.line(format!(
                        "E: {drawn}/{}, SD: {}",
                        self.entities.map.len() + 1,
                        self.debug.simulation_distance
                    ));
                }
                "chunk_source_stats" => {
                    let range = self.env.server_view.max(2) + 3;
                    let cap = (2 * range + 1) * (2 * range + 1);
                    let mut sections = HashSet::new();
                    let mut chunks = HashSet::new();
                    for e in self.entities.map.values() {
                        let p = e.pos.map(|v| (v.floor() as i32) >> 4);
                        sections.insert((p[0], p[1], p[2]));
                        chunks.insert((p[0], p[2]));
                    }
                    c.line(format!(
                        "Chunks[C] W: {cap}, {} E: {},{},{}",
                        self.world.grid.len(),
                        self.entities.map.len() + 1,
                        sections.len(),
                        chunks.len()
                    ));
                }
                "particle_render_stats" => c.line(format!("P: {}", self.particles.len())),
                "player_position" => {
                    let (cx, cz) = (block[0] >> 4, block[2] >> 4);
                    // Quarter turn computed in double, like the game.
                    let dir = ((f64::from(self.yaw) / 90.0 + 0.5).floor() as i32) & 3;
                    let (name, text) = match dir {
                        0 => ("south", "Towards positive Z"),
                        1 => ("west", "Towards negative X"),
                        2 => ("north", "Towards negative Z"),
                        _ => ("east", "Towards positive X"),
                    };
                    let g = c.group("position");
                    g.push(format!("XYZ: {:.3} / {:.5} / {:.3}", feet[0], feet[1], feet[2]));
                    g.push(format!("Block: {} {} {}", block[0], block[1], block[2]));
                    g.push(format!(
                        "Chunk: {cx} {} {cz} [{} {} in r.{}.{}.mca]",
                        block[1] >> 4,
                        cx & 31,
                        cz & 31,
                        cx >> 5,
                        cz >> 5
                    ));
                    g.push(format!(
                        "Facing: {name} ({text}) ({:.1} / {:.1})",
                        wrap_degrees(self.yaw),
                        wrap_degrees(self.pitch)
                    ));
                    g.push(format!("{} FC: 0", self.dimension));
                }
                "player_section_position" => {
                    let [x, y, z] = block.map(|v| v & 15);
                    c.group("position").push(format!("Section-relative: {x:02} {y:02} {z:02}"));
                }
                "player_speed" => {
                    let d: [f64; 3] = std::array::from_fn(|a| self.player.pos[a] - self.prev_pos[a]);
                    let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    c.group("position").push(format!("Speed: {s:.3} blocks/tick"));
                }
                "looking_at_block_state" | "looking_at_fluid_state" => {
                    let fluid = e.id == "looking_at_fluid_state";
                    let group = if fluid { "looking_at_fluid" } else { "looking_at_block" };
                    let target = self.debug_target(fluid);
                    let g = c.group(group);
                    let Some(pos) = target else { continue };
                    let state = view.state(pos[0], pos[1], pos[2]);
                    let what = if fluid { "Fluid" } else { "Block" };
                    g.push(format!("§nTargeted {what}: {}, {}, {}", pos[0], pos[1], pos[2]));
                    if fluid {
                        let (name, props) = fluid_state(&self.data, state);
                        g.push(name);
                        g.extend(props);
                    } else {
                        g.push(self.data.block_of(state).map_or_else(|| "?".into(), |b| b.name.clone()));
                        for (k, v) in self.data.state_properties(state) {
                            let color = match v {
                                "true" => "§a",
                                "false" => "§c",
                                _ => "",
                            };
                            g.push(format!("{k}: {color}{v}"));
                        }
                    }
                }
                "looking_at_block_tags" | "looking_at_fluid_tags" => {
                    let fluid = e.id == "looking_at_fluid_tags";
                    let group = if fluid { "looking_at_fluid" } else { "looking_at_block" };
                    let target = self.debug_target(fluid);
                    let g = c.group(group);
                    let Some(pos) = target else { continue };
                    let state = view.state(pos[0], pos[1], pos[2]);
                    let (reg, id) = if fluid {
                        ("minecraft:fluid", self.data.state(state).map_or(0, |s| i32::from(s.fluid)))
                    } else {
                        ("minecraft:block", self.data.state(state).map_or(-1, |s| i32::from(s.block)))
                    };
                    g.extend(tag_lines(&self.debug.tags, reg, id));
                }
                "looking_at_entity" | "looking_at_entity_tags" => {
                    let g = c.group("looking_at_entity");
                    let crate::pick::Hit::Entity { id, .. } = hit else { continue };
                    let Some(ent) = self.entities.map.get(&id) else { continue };
                    if e.id == "looking_at_entity" {
                        g.push("§nTargeted Entity".into());
                        g.push(format!("minecraft:{}", ent.kind));
                    } else {
                        g.extend(tag_lines(&self.debug.tags, "minecraft:entity_type", ent.kind_id));
                    }
                }
                "light_levels" => {
                    let (b, s) = view.light_pair(block[0], block[1], block[2]);
                    c.group("light").push(format!("Client Light: {} ({s} sky, {b} block)", b.max(s)));
                }
                "heightmap" => {
                    let Some(col) = self.world.grid.get(block[0] >> 4, block[2] >> 4) else { continue };
                    let (h, my) = (self.world.height, self.world.min_y);
                    let v = |k: i32| {
                        heightmap_at(&col.heightmaps, k, block[0], block[2], h, my)
                            .map_or_else(|| "?".into(), |v| v.to_string())
                    };
                    let g = c.group("heightmaps");
                    g.push(format!("CH S: {} M: {} ML: {}", v(1), v(4), v(5)));
                    g.push("SH S: ?? O: ?? M: ?? ML: ??".into());
                }
                "biome" => {
                    if block[1] >= self.world.min_y && block[1] < self.world.min_y + self.world.height {
                        let b = self.env.biome_full_at(&view, feet) as usize;
                        c.line(format!(
                            "Biome: {}",
                            self.biome_names.get(b).map_or_else(|| format!("[unregistered {b}]"), Clone::clone)
                        ));
                    }
                }
                "day_count" => {
                    if let Some(tl) = self.data.timelines.iter().find(|t| t.name == "minecraft:overworld_day")
                        && tl.period > 0
                    {
                        let ticks = self.env.env.clocks.total_of(&tl.clock);
                        c.line(format!("Day #{}", ticks.div_euclid(i64::from(tl.period))));
                    }
                }
                "sound_mood" => {
                    let used = self.audio.peek().map_or(0, |a| a.active());
                    let mood = (self.world_sounds.mood() * 100.0).round() as i32;
                    c.line(format!("Sounds: {used}/247 + 0/8 (Mood {mood}%)"));
                }
                "sound_cache" => {
                    let (n, b) = self.audio.peek().map_or((0, 0), |a| (a.cache_len(), a.cache_bytes()));
                    c.line(format!("Sound cache: {n} buffers, {} MiB", (b as u64).div_ceil(MIB)));
                }
                // Singleplayer only (local difficulty, spawns, generation) and effects: nothing.
                _ => {}
            }
        }
        c.assemble();
        if self.debug.overlay() {
            let lang = &r.gui.lang;
            let st = |b: bool| if b { "visible" } else { "hidden" };
            let k = |a: &str| keybind(o, lang, a);
            c.left.push(String::new());
            let solo = self.solo.is_some();
            c.left.push(format!(
                "Debug charts: {} Profiler {}; {} {} {};",
                k("profilingChart"),
                st(self.debug.charts[0]),
                k("fpsCharts"),
                if solo { "fps + tps" } else { "fps" },
                st(self.debug.charts[1])
            ));
            c.left.push(format!(
                "{} {} {}; {} Lightmap {}",
                k("networkCharts"),
                if solo { "Ping" } else { "Bandwidth + Ping" },
                st(self.debug.charts[2]),
                k("lightmapTexture"),
                st(self.debug.charts[3])
            ));
            c.left.push(format!("To edit: press {}", k("debugOptions")));
        }
        self.debug.collector = c;
        true
    }

    /// Block (or fluid) targeted at 20 blocks from the eyes (§ 3.10).
    fn debug_target(&self, fluid: bool) -> Option<[i32; 3]> {
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let eye = [self.player.pos[0], self.player.pos[1] + self.player.eye_height(), self.player.pos[2]];
        let dir = crate::camera::forward(self.yaw, self.pitch);
        let to = [eye[0] + f64::from(dir.x) * 20.0, eye[1] + f64::from(dir.y) * 20.0, eye[2] + f64::from(dir.z) * 20.0];
        let data = &self.data;
        static FULL: [rc_data::Aabb; 1] = [rc_data::Aabb { min: [0.0; 3], max: [1.0; 3] }];
        // Outline shape seen by the player (held item, block offset), like the aiming.
        let (full, held) = (self.act.full_shape, self.held_name());
        let shape = |x: i32, y: i32, z: i32| -> crate::pick::Shape<'_> {
            let s = view.state(x, y, z);
            match data.state(s) {
                Some(i) if fluid && i.fluid != 0 => crate::pick::plain(&FULL),
                _ => crate::aim_shape(data, full, held, s, [x, y, z])
                    .map_or_else(|| crate::pick::plain(&[]), |(i, shift)| (data.shape(i), shift)),
            }
        };
        match crate::pick::clip_blocks(eye, to, &shape) {
            Some(crate::pick::Hit::Block { pos, .. }) => Some(pos),
            _ => None,
        }
    }
}

/// Fluid state of a block state: id and properties (`falling`, `level`).
fn fluid_state(data: &rc_data::VersionData, state: u32) -> (String, Vec<String>) {
    let Some(s) = data.state(state).filter(|s| s.fluid != 0) else { return ("minecraft:empty".into(), Vec::new()) };
    let name = data.fluids.get(usize::from(s.fluid)).cloned().unwrap_or_else(|| "?".into());
    let level = data.state_properties(state).find(|p| p.0 == "level").and_then(|p| p.1.parse::<i32>().ok());
    let falling = level.is_some_and(|l| l >= 8);
    let b = |v: bool| if v { "§atrue" } else { "§cfalse" };
    let mut props = vec![format!("falling: {}", b(falling))];
    if name.contains("flowing") {
        props.push(format!("level: {}", s.fluid_amount));
    }
    (name, props)
}

/// `#tag` lines of an id in a received tag registry.
fn tag_lines(tags: &rc_proto_v777::play::Tags, registry: &str, id: i32) -> Vec<String> {
    let mut out = Vec::new();
    if let Some((_, list)) = tags.iter().find(|t| t.0 == registry) {
        for (name, ids) in list {
            if ids.contains(&id) {
                out.push(format!("#{name}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_sorted() {
        assert!(ENTRIES.windows(2).all(|w| w[0].id < w[1].id));
        assert_eq!(index("minecraft:fps"), Some(ENTRIES.iter().position(|e| e.id == "fps").unwrap()));
    }

    #[test]
    fn toggle_table() {
        let mut d = DebugScreen::new(None);
        let i = index("entity_hitboxes").unwrap();
        assert_eq!(d.status_of(i), Status::Never);
        assert!(d.toggle("entity_hitboxes"));
        assert_eq!(d.status_of(i), Status::AlwaysOn);
        assert!(!d.toggle("entity_hitboxes"));
        d.set_overlay(true);
        assert!(d.toggle("entity_hitboxes"));
        assert_eq!(d.status_of(i), Status::InOverlay);
        assert!(d.profile.is_none());
    }

    #[test]
    fn profile_json_round_trip() {
        let mut d = DebugScreen::new(None);
        assert_eq!(d.to_json(), r#"{"DataVersion":5023,"profile":"default"}"#);
        d.set_status(index("fps").unwrap(), Status::AlwaysOn);
        let j = d.to_json();
        let mut e = DebugScreen::new(None);
        assert!(e.parse(&j));
        assert_eq!(e.status_of(index("fps").unwrap()), Status::AlwaysOn);
        assert!(e.is_on("fps"));
        assert!(!e.parse("{"));
    }

    #[test]
    fn columns_example() {
        // § 2.6: priority ones alternated, normal ones cut in two, groups spread.
        let mut c = Collector::default();
        c.priority("fps".into());
        c.priority("version".into());
        c.group("memory").extend(["m1".to_string(), "m2".into(), "m3".into()]);
        c.group("position").push("p".into());
        c.line("a".into());
        c.line("b".into());
        c.line("c".into());
        c.group("system").push("s".into());
        c.line("tps".into());
        c.assemble();
        assert_eq!(c.left, ["fps", "", "a", "b", "", "m1", "m2", "m3", "", "p", ""]);
        assert_eq!(c.right, ["version", "", "c", "tps", "", "s", ""]);
    }

    #[test]
    fn heightmap_decode() {
        // Height 384: 9 bits, 7 values per long; cell (1, 0) = 70 → y = 70 − 64.
        let maps = vec![(1, vec![70i64 << 9].into_boxed_slice())];
        assert_eq!(heightmap_at(&maps, 1, 1, 0, 384, -64), Some(6));
    }
}
