//! Debug screen keys (`debug-f3.md` § 6): F3 alone on release, F3 + … actions,
//! and keyboard-less driving (`RUSTCRAFT_DEBUGTEST`).

use std::time::Instant;

use rc_text::Component;
use winit::keyboard::KeyCode;

use crate::debug_screen::Chart;

/// UTC calendar date (year, month, day, hour, minute, second, weekday 0 = Monday).
fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    let weekday = (days + 3).rem_euclid(7) as u32;
    (y, m, d, rem / 3600, rem / 60 % 60, rem % 60, weekday)
}

/// Timestamp of written files (`2026-10-12_14.03.07`, local time like the game).
pub(crate) fn file_stamp() -> String {
    let (y, m, d, h, mi, s) = crate::screenshot::local_now();
    format!("{y:04}-{m:02}-{d:02}_{h:02}.{mi:02}.{s:02}")
}

/// ISO date (`2026-09-15T11:20:48+00:00`) in the game's display format (`Tue Sep 15 11:20:48 UTC 2026`).
fn java_date(iso: &str) -> String {
    let n = |r: std::ops::Range<usize>| iso.get(r).and_then(|s| s.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) =
        (n(0..4), n(5..7), n(8..10), n(11..13), n(14..16), n(17..19))
    else {
        return iso.to_owned();
    };
    // Days since the epoch (inverse algorithm of `civil`).
    let (yy, mm) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let (.., wd) = civil(days * 86_400);
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {d:02} {h:02}:{mi:02}:{s:02} UTC {y}", DAYS[wd as usize], MONTHS[(mo - 1).clamp(0, 11) as usize])
}

/// Short name → key, for `RUSTCRAFT_DEBUGTEST`.
fn parse_key(s: &str) -> Option<KeyCode> {
    use KeyCode as K;
    let s = s.to_ascii_lowercase();
    let letters = [
        K::KeyA,
        K::KeyB,
        K::KeyC,
        K::KeyD,
        K::KeyE,
        K::KeyF,
        K::KeyG,
        K::KeyH,
        K::KeyI,
        K::KeyJ,
        K::KeyK,
        K::KeyL,
        K::KeyM,
        K::KeyN,
        K::KeyO,
        K::KeyP,
        K::KeyQ,
        K::KeyR,
        K::KeyS,
        K::KeyT,
        K::KeyU,
        K::KeyV,
        K::KeyW,
        K::KeyX,
        K::KeyY,
        K::KeyZ,
    ];
    let digits =
        [K::Digit0, K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8, K::Digit9];
    let fkeys = [K::F1, K::F2, K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12];
    let b = s.as_bytes();
    match s.as_str() {
        "esc" | "escape" => Some(K::Escape),
        "tab" => Some(K::Tab),
        "enter" => Some(K::Enter),
        "shift" => Some(K::ShiftLeft),
        _ if b.len() == 1 && b[0].is_ascii_lowercase() => Some(letters[usize::from(b[0] - b'a')]),
        _ if b.len() == 1 && b[0].is_ascii_digit() => Some(digits[usize::from(b[0] - b'0')]),
        _ if b.len() >= 2 && b[0] == b'f' => {
            s[1..].parse::<usize>().ok().and_then(|n| fkeys.get(n.wrapping_sub(1)).copied())
        }
        _ => None,
    }
}

/// Value of a digit key (top row or numeric keypad).
fn digit(code: KeyCode) -> Option<u32> {
    use KeyCode::*;
    Some(match code {
        Digit0 | Numpad0 => 0,
        Digit1 | Numpad1 => 1,
        Digit2 | Numpad2 => 2,
        Digit3 | Numpad3 => 3,
        Digit4 | Numpad4 => 4,
        Digit5 | Numpad5 => 5,
        Digit6 | Numpad6 => 6,
        Digit7 | Numpad7 => 7,
        Digit8 | Numpad8 => 8,
        Digit9 | Numpad9 => 9,
        _ => return None,
    })
}

impl crate::App {
    /// Keyboard event: F3 keys, then the debug options screen if it is open.
    pub(crate) fn debug_key_event(&mut self, e: &winit::event::KeyEvent) -> bool {
        let consumed = match e.physical_key {
            winit::keyboard::PhysicalKey::Code(k) => {
                self.debug_key(k, e.state == winit::event::ElementState::Pressed, e.repeat)
            }
            _ => false,
        };
        consumed || self.debug_options_key(e)
    }

    /// Key seen before the other handling; true if the debug screen consumes it.
    pub(crate) fn debug_key(&mut self, code: KeyCode, pressed: bool, repeat: bool) -> bool {
        if self.chat.input.is_some() {
            return false;
        }
        let name = crate::options::key_name(code);
        let modifier = self.options.s("key_key.debug.modifier").to_owned();
        let overlay = self.options.s("key_key.debug.overlay").to_owned();
        let crash = self.options.s("key_key.debug.crash").to_owned();
        // F3+C held: releasing either of the two keys cancels the countdown.
        if !pressed && (name == crash || name == modifier) {
            self.debug.crash = None;
        }
        if pressed && code == KeyCode::Escape && self.debug.switcher.is_some() {
            self.debug.switcher = None;
            self.grab(true);
            return true;
        }
        if pressed {
            if name == modifier {
                if !repeat {
                    self.debug.modifier_down = true;
                    self.debug.modifier_used = false;
                }
                return true;
            }
            if name == overlay {
                if !repeat {
                    let v = !self.debug.overlay();
                    self.debug.set_overlay(v);
                }
                return true;
            }
            if !self.debug.modifier_down {
                // F3+1 pie chart: a digit goes down into a child (0: parent); the key
                // keeps its ordinary effect (toolbar), like the game.
                if self.debug.chart(Chart::Profiler)
                    && let Some(d) = digit(code)
                {
                    self.pie_key(d);
                }
                return false;
            }
            if name == crash && self.debug.crash.is_none() && !repeat {
                let now = Instant::now();
                self.debug.crash = Some((now, now, 0));
                self.debug.modifier_used |= overlay == modifier;
            }
            // F3+C pressed for over 100 ms: every other action is blocked.
            if self.debug.crash.is_some_and(|c| c.0.elapsed().as_millis() > 100) {
                return true;
            }
            if repeat {
                return self.debug_action_keys().iter().any(|a| self.options.s(&format!("key_key.debug.{a}")) == name);
            }
            if self.debug_action(name) {
                self.debug.modifier_used = true;
                return true;
            }
            false
        } else if name == modifier {
            self.debug.modifier_down = false;
            if self.debug.switcher.is_some() {
                self.debug_switcher_apply();
            }
            // F3 alone (overlay and modifier on the same key): toggles on release.
            if overlay == modifier && !self.debug.modifier_used {
                let v = !self.debug.overlay();
                self.debug.set_overlay(v);
            }
            true
        } else {
            false
        }
    }

    fn debug_action_keys(&self) -> [&'static str; 22] {
        [
            "reloadChunk",
            "showHitboxes",
            "clearChat",
            "showChunkBorders",
            "showAdvancedTooltips",
            "copyRecreateCommand",
            "spectate",
            "switchGameMode",
            "debugOptions",
            "focusPause",
            "dumpDynamicTextures",
            "reloadResourcePacks",
            "profiling",
            "copyLocation",
            "dumpVersion",
            "profilingChart",
            "fpsCharts",
            "networkCharts",
            "lightmapTexture",
            "improvedTransparency",
            "crash",
            "overlay",
        ]
    }

    /// Feedback message (§ 6.2): `[Debug]:` bold, yellow (red for a warning).
    pub(crate) fn debug_feedback(&mut self, warning: bool, body: Component) {
        let mut prefix = Component::translate("debug.prefix", Vec::new());
        prefix.style.bold = Some(true);
        prefix.style.color = Some(if warning { 0xFF5555 } else { 0xFFFF55 });
        let mut c = Component::text("");
        c.extra = vec![prefix, Component::text(" "), body];
        self.chat.add_client(c);
    }

    /// Translated label of a debug action's key (`F3`, `L`…).
    fn debug_key_label(&self, action: &str) -> String {
        let name = self.options.s(&format!("key_key.debug.{action}"));
        match &self.renderer {
            Some(r) => crate::options::key_label(&r.gui.lang, name),
            None => name.to_owned(),
        }
    }

    pub(crate) fn feedback(&mut self, key: &str) {
        self.debug_feedback(false, Component::translate(key, Vec::new()));
    }

    /// Boolean option toggled, `options.txt` rewritten; returns the new value.
    fn flip_option(&mut self, key: &str) -> bool {
        let v = !self.options.b(key);
        self.options.set(key, crate::options::Val::B(v));
        self.save_options();
        self.apply_options();
        v
    }

    /// F3 + key actions (§ 6.3), in the game's order; true if an action took place.
    fn debug_action(&mut self, name: &str) -> bool {
        let is = |o: &crate::options::Options, a: &str| o.s(&format!("key_key.debug.{a}")) == name;
        let o = self.options.clone();
        let mut done = false;
        let has_player = self.positioned;
        let reduced = self.debug.reduced();
        if is(&o, "reloadChunk") {
            self.debug_reload_chunks();
            self.feedback("debug.reload_chunks.message");
            done = true;
        }
        if is(&o, "showHitboxes") && has_player && !reduced {
            let on = self.debug.toggle("entity_hitboxes");
            self.feedback(if on { "debug.show_hitboxes.on" } else { "debug.show_hitboxes.off" });
            done = true;
        }
        if is(&o, "clearChat") {
            self.chat.clear_messages();
            done = true;
        }
        if is(&o, "showChunkBorders") && has_player && !reduced {
            let on = self.debug.toggle("chunk_borders");
            self.feedback(if on { "debug.chunk_boundaries.on" } else { "debug.chunk_boundaries.off" });
            done = true;
        }
        if is(&o, "showAdvancedTooltips") {
            let on = self.flip_option("advancedItemTooltips");
            self.feedback(if on { "debug.advanced_tooltips.on" } else { "debug.advanced_tooltips.off" });
            done = true;
        }
        if is(&o, "copyRecreateCommand") {
            if has_player && !reduced {
                self.debug_copy_recreate(self.mods.shift_key());
            }
            done = true;
        }
        if is(&o, "spectate") {
            if !has_player || self.debug.op_level < 2 {
                self.feedback("debug.creative_spectator.error");
            } else if self.env.game_mode != 3 {
                let _ = self.session.link.send_change_game_mode(3);
            } else {
                let prev = self.debug.previous_mode.filter(|&m| (0..=3).contains(&m)).unwrap_or(1);
                let _ = self.session.link.send_change_game_mode(prev);
            }
            done = true;
        }
        let switching = self.debug.switcher.is_some();
        let no_screen = !self.screens.open
            && !self.menus.is_open()
            && self.death.is_none()
            && !self.debug.options_open()
            && !switching;
        if is(&o, "switchGameMode") && self.positioned && no_screen {
            if self.debug.op_level >= 2 {
                self.debug_switcher_open();
            } else {
                self.feedback("debug.gamemodes.error");
            }
            done = true;
        }
        if is(&o, "debugOptions") {
            self.debug_options_toggle();
            done = true;
        }
        if is(&o, "focusPause") {
            let on = self.flip_option("pauseOnLostFocus");
            self.feedback(if on { "debug.pause_focus.on" } else { "debug.pause_focus.off" });
            done = true;
        }
        if is(&o, "reloadResourcePacks") {
            // Message, then full reload of the resources (`reload.rs`).
            self.feedback("debug.reload_resourcepacks.message");
            self.reload_resources();
            done = true;
        }
        if is(&o, "profiling") {
            self.debug_profiling_toggle();
            done = true;
        }
        if is(&o, "copyLocation") && has_player && !reduced {
            self.feedback("debug.copy_location.message");
            let p = self.player.pos;
            crate::debug_sys::set_clipboard(&format!(
                "/execute in {} run tp @s {:.2} {:.2} {:.2} {:.2} {:.2}",
                self.dimension, p[0], p[1], p[2], self.yaw, self.pitch
            ));
            done = true;
        }
        if is(&o, "dumpVersion") {
            self.feedback("debug.version.header");
            self.debug_dump_version();
            done = true;
        }
        for (a, c) in [
            ("profilingChart", Chart::Profiler),
            ("fpsCharts", Chart::Fps),
            ("networkCharts", Chart::Network),
            ("lightmapTexture", Chart::Lightmap),
        ] {
            if is(&o, a) {
                self.debug.toggle_chart(c);
                done = true;
            }
        }
        if is(&o, "improvedTransparency") {
            let on = self.flip_option("improvedTransparency");
            self.feedback(if on { "debug.improvedTransparency.on" } else { "debug.improvedTransparency.off" });
            done = true;
        }
        if switching && is(&o, "switchGameMode") {
            self.debug_switcher_next();
            done = true;
        }
        done
    }

    /// All columns re-meshed (F3+A).
    fn debug_reload_chunks(&mut self) {
        let keys: Vec<(i32, i32)> = self.world.grid.iter().map(|c| (c.x, c.z)).collect();
        for (x, z) in keys {
            self.world.touch(x, z);
        }
    }

    /// F3+V: version information (the game's `version.json` file).
    fn debug_dump_version(&mut self) {
        let v = self
            .assets
            .read("version.json")
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .unwrap_or_default();
        let s =
            |k: &str| v.get(k).map_or_else(String::new, |x| x.as_str().map_or_else(|| x.to_string(), str::to_owned));
        let pack = |a: &str, b: &str| {
            let p = &v["pack_version"];
            format!("{}.{}", p[a].as_i64().unwrap_or(0), p[b].as_i64().unwrap_or(0))
        };
        let protocol = v["protocol_version"].as_i64().unwrap_or_else(|| i64::from(self.data.protocol));
        let lines: Vec<(&str, Vec<String>)> = vec![
            ("commands.version.id", vec![s("id")]),
            ("commands.version.name", vec![s("name")]),
            ("commands.version.data", vec![s("world_version")]),
            ("commands.version.series", vec![s("series_id")]),
            ("commands.version.protocol", vec![protocol.to_string(), format!("0x{protocol:x}")]),
            ("commands.version.build_time", vec![java_date(&s("build_time"))]),
            ("commands.version.pack.resource", vec![pack("resource_major", "resource_minor")]),
            ("commands.version.pack.data", vec![pack("data_major", "data_minor")]),
        ];
        for (key, args) in lines {
            let args = args.into_iter().map(rc_text::Arg::Text).collect();
            self.chat.add_client(Component::translate(key, args));
        }
        let stable = v["stable"].as_bool().unwrap_or(true);
        let key = if stable { "commands.version.stable.yes" } else { "commands.version.stable.no" };
        self.chat.add_client(Component::translate(key, Vec::new()));
    }

    /// F3+L: 10 s recording (profiler and per-frame readings, `profiling_zip.rs`); F3+L
    /// during the recording stops it at the end of the frame.
    fn debug_profiling_toggle(&mut self) {
        if let Some(p) = &mut self.debug.profiling {
            p.stop = true;
            return;
        }
        self.debug.profiling = Some(crate::debug_screen::Profiling {
            start: Instant::now(),
            stop: false,
            frame_start: None,
            rows: Vec::new(),
        });
        crate::profiler::set(crate::profiler::RECORD, true);
        // The game also passes the two keys ("rebindable" text taken under this key).
        let args = vec![
            rc_text::Arg::Text("10".into()),
            rc_text::Arg::Text(self.debug_key_label("modifier")),
            rc_text::Arg::Text(self.debug_key_label("profiling")),
        ];
        self.debug_feedback(false, Component::translate("debug.profiling.start", args));
    }

    /// Debug screen tick: F3+C countdown, profiling, `RUSTCRAFT_DEBUGTEST`
    /// driving, previous mode.
    pub(crate) fn debug_tick(&mut self) {
        if self.env.game_mode != self.debug.current_mode {
            if self.debug.current_mode >= 0 {
                self.debug.previous_mode = Some(self.debug.current_mode);
            }
            self.debug.current_mode = self.env.game_mode;
        }
        if let Some((start, reported, count)) = self.debug.crash {
            let now = Instant::now();
            let remaining = 10_000 - now.duration_since(start).as_millis() as i64;
            if remaining < 0 {
                self.debug_crash();
            }
            if now.duration_since(reported).as_millis() >= 1000 {
                if count == 0 {
                    let args = vec![
                        rc_text::Arg::Text(self.debug_key_label("modifier")),
                        rc_text::Arg::Text(self.debug_key_label("crash")),
                    ];
                    self.debug_feedback(false, Component::translate("debug.crash.message", args));
                } else {
                    let secs = (remaining as f64 / 1000.0).ceil() as i64;
                    self.debug_feedback(
                        true,
                        Component::translate("debug.crash.warning", vec![rc_text::Arg::Text(secs.to_string())]),
                    );
                }
                self.debug.crash = Some((start, now, count + 1));
            }
        }
        self.debug_charts_tick();
        self.debugtest_tick();
    }

    /// Deliberate crash (F3+C 10 s): report written, exit with an error code.
    fn debug_crash(&mut self) -> ! {
        // Same formatter as the panic (`crash_report.rs`), system details up to date.
        crate::crash_report::set_system(self.system_lines());
        let title = "Manually triggered debug crash";
        let at = [format!("{}:{}", file!(), line!())];
        let error = format!("{title}\n\tat {}", at[0]);
        let thread = std::thread::current();
        let crash = crate::crash_report::Crash { title, error: &error, thread: thread.name().unwrap_or("main"), at: &at };
        if let Some(path) = crate::crash_report::write(&crash) {
            eprintln!("deliberate crash (F3+C): report {}", path.display());
        }
        self.session.link.disconnect();
        std::process::exit(255);
    }

    /// `RUSTCRAFT_DEBUGTEST=steps`: one step every 20 ticks, 12 s after arrival.
    /// `f3` short press, `f3+b` combination, `down:c` / `up:c` press or release alone,
    /// `wait` nothing.
    fn debugtest_tick(&mut self) {
        let Some(spec) = rc_data::env_str!("RUSTCRAFT_DEBUGTEST") else { return };
        if !self.positioned {
            return;
        }
        self.debug.test.1 += 1;
        if self.debug.test.1 < 240 || !self.debug.test.1.is_multiple_of(20) {
            return;
        }
        let Some(step) = spec.split(',').map(str::trim).filter(|s| !s.is_empty()).nth(self.debug.test.0) else {
            return;
        };
        self.debug.test.0 += 1;
        eprintln!("F3 demo: {step}");
        let step = step.to_owned();
        if let Some(xy) = step.strip_prefix("click:") {
            // Simulated click in the options screen (GUI units), mouse forced like `RUSTCRAFT_MENUTEST`.
            if let Some((x, y)) = xy.split_once(':')
                && let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>())
            {
                self.menutest.2 = Some((x, y));
                self.debug_options_click((x, y));
            }
        } else if let Some(k) = step.strip_prefix("down:").and_then(parse_key) {
            self.debug_key(k, true, false);
        } else if let Some(k) = step.strip_prefix("up:").and_then(parse_key) {
            self.debug_key(k, false, false);
        } else if let Some((a, b)) = step.split_once('+') {
            if let (Some(a), Some(b)) = (parse_key(a), parse_key(b)) {
                self.debug_key(a, true, false);
                self.debug_key(b, true, false);
                self.debug_key(b, false, false);
                self.debug_key(a, false, false);
            }
        } else if let Some(k) = parse_key(&step) {
            self.debug_key(k, true, false);
            self.debug_key(k, false, false);
        }
    }
}
