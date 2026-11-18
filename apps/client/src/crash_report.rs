//! Crash reports in the game's format, written to `crash-reports/crash-<date>-client.txt` in the
//! game folder: header and randomly drawn remark, time, description, error, then the
//! faulty thread's section and the system details. A single formatter for the panic (any thread,
//! integrated server included), the deliberate F3+C crash and the profiling `system.txt`.

use std::path::PathBuf;
use std::sync::Mutex;

/// Header remarks (game text), chosen by the clock in nanoseconds.
const COMMENTS: [&str; 34] = [
    "Who set us up the TNT?",
    "Everything's going to plan. No, really, that was supposed to happen.",
    "Uh... Did I do that?",
    "Oops.",
    "Why did you do that?",
    "I feel sad now :(",
    "My bad.",
    "I'm sorry, Dave.",
    "I let you down. Sorry :(",
    "On the bright side, I bought you a teddy bear!",
    "Daisy, daisy...",
    "Oh - I know what I did wrong!",
    "Hey, that tickles! Hehehe!",
    "I blame Dinnerbone.",
    "You should try our sister game, Minceraft!",
    "Don't be sad. I'll do better next time, I promise!",
    "Don't be sad, have a hug! <3",
    "I just don't know what went wrong :(",
    "Shall we play a game?",
    "Quite honestly, I wouldn't worry myself about that.",
    "I bet Cylons wouldn't have this problem.",
    "Sorry :(",
    "Surprise! Haha. Well, this is awkward.",
    "Would you like a cupcake?",
    "Hi. I'm Minecraft, and I'm a crashaholic.",
    "Ooh. Shiny.",
    "This doesn't make any sense!",
    "Why is it breaking :(",
    "Don't do that.",
    "Ouch. That hurt :(",
    "You're mean.",
    "This is a token for 1 free hug. Redeem at your nearest Mojangsta: [~~HUG~~]",
    "There are four lights!",
    "But it works on my machine.",
];

/// Header remarks of the profiler results (game text).
const PROFILE_COMMENTS: [&str; 15] = [
    "I'd Rather Be Surfing",
    "Shiny numbers!",
    "Am I not running fast enough? :(",
    "I'm working as hard as I can!",
    "Will I ever be good enough for you? :(",
    "Speedy. Zoooooom!",
    "Hello world",
    "40% better than a crash report.",
    "Now with extra numbers",
    "Now with less numbers",
    "Now with the same numbers",
    "You should add flames to things, it makes them go faster!",
    "Do you feel the need for... optimization?",
    "*cracks redstone whip*",
    "Maybe if you treated it better then it'll have more motivation to work faster! Poor server.",
];

/// Kind of report: header title and remarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Crash,
    Profile,
}

/// Common header (`---- <title> ----`, remark, empty line); `nanos` draws the remark.
pub fn header(kind: Kind, nanos: u128) -> String {
    let (title, list): (&str, &[&str]) = match kind {
        Kind::Crash => ("Minecraft Crash Report", &COMMENTS),
        Kind::Profile => ("Minecraft Profiler Results", &PROFILE_COMMENTS),
    };
    format!("---- {title} ----\n// {}\n\n", list[(nanos % list.len() as u128) as usize])
}

/// Clock in nanoseconds (draw of the remarks).
pub fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// Known system details (updated by the application, read by the panic hook).
static SYSTEM: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// System details to attach to the reports (key, value), in display order.
pub fn set_system(lines: Vec<(String, String)>) {
    *SYSTEM.lock().unwrap_or_else(|p| p.into_inner()) = lines;
}

/// Recorded system details.
pub fn system() -> Vec<(String, String)> {
    SYSTEM.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Reported error: description, error text (message then locations), faulty thread.
pub struct Crash<'a> {
    pub title: &'a str,
    pub error: &'a str,
    pub thread: &'a str,
    /// Locations of the thread section (`file:line:column`), from deepest to highest.
    pub at: &'a [String],
}

/// Full text of the report; `time` in the format `yyyy-MM-dd HH:mm:ss`, `nanos` draws the remark.
pub fn format(c: &Crash<'_>, system: &[(String, String)], time: &str, nanos: u128) -> String {
    let mut s = header(Kind::Crash, nanos);
    // The error is a printed trace: it ends with a line break, followed by two others.
    let end = if c.error.ends_with('\n') { "" } else { "\n" };
    s += &format!(
        "Time: {time}\nDescription: {}\n\n{}{end}\n\n\
         A detailed walkthrough of the error, its code path and all known details is as follows:\n{}\n\n",
        c.title,
        c.error,
        "-".repeat(87)
    );
    if !c.at.is_empty() {
        s.push_str(&format!("-- Head --\nThread: {}\nStacktrace:\n", c.thread));
        for a in c.at {
            s.push_str(&format!("\tat {a}\n"));
        }
        s.push('\n');
    }
    s.push_str("-- System Details --\nDetails:");
    for (k, v) in system {
        s.push_str(&format!("\n\t{k}: {v}"));
    }
    s
}

/// Writes the report into `crash-reports` in the game folder; returns its path.
pub fn save(text: &str) -> Option<PathBuf> {
    let dir = crate::servers::game_dir().join("crash-reports");
    let path = dir.join(format!("crash-{}-client.txt", crate::debug_keys::file_stamp()));
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, text).ok()?;
    Some(path)
}

/// Local time of the report (`yyyy-MM-dd HH:mm:ss`).
pub fn now_text() -> String {
    let (y, m, d, h, mi, s) = crate::screenshot::local_now();
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Report written now (current time and remark); returns its path.
pub fn write(c: &Crash<'_>) -> Option<PathBuf> {
    save(&format(c, &system(), &now_text(), nanos()))
}

/// Process panic hook (main thread, integrated server, background threads): the
/// "Unexpected error" report is written before the usual message and the stop.
pub fn install_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "Box<dyn Any>".to_owned());
        let at: Vec<String> =
            info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).into_iter().collect();
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let error = match at.first() {
            Some(a) => format!("panic: {msg}\n\tat {a}"),
            None => format!("panic: {msg}"),
        };
        let crash = Crash { title: "Unexpected error", error: &error, thread: name, at: &at };
        if let Some(p) = write(&crash) {
            eprintln!("crash report: {}", p.display());
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Game layout: header, remark, time, description, error, 87-dash line,
    /// thread section then system details with tabs.
    #[test]
    fn report_follows_the_game_layout() {
        let at = ["src/main.rs:10:5".to_owned()];
        let c = Crash { title: "Unexpected error", error: "panic: boom\n\tat src/main.rs:10:5", thread: "main", at: &at };
        let sys = [("Minecraft Version".to_owned(), "26.3".to_owned()), ("CPUs".to_owned(), "8".to_owned())];
        let text = format(&c, &sys, "2026-10-24 12:00:00", 3);
        let want = format!(
            "---- Minecraft Crash Report ----\n// Oops.\n\nTime: 2026-10-24 12:00:00\nDescription: Unexpected error\n\n\
             panic: boom\n\tat src/main.rs:10:5\n\n\n\
             A detailed walkthrough of the error, its code path and all known details is as follows:\n{}\n\n\
             -- Head --\nThread: main\nStacktrace:\n\tat src/main.rs:10:5\n\n\
             -- System Details --\nDetails:\n\tMinecraft Version: 26.3\n\tCPUs: 8",
            "-".repeat(87)
        );
        assert_eq!(text, want);
        assert_eq!(format(&c, &sys, "t", 34 + 3), text.replace("2026-10-24 12:00:00", "t"));
        let ended = Crash { error: "panic: boom\n\tat src/main.rs:10:5\n", ..c };
        assert_eq!(format(&ended, &sys, "2026-10-24 12:00:00", 3), text, "final line break already present");
        assert_eq!(header(Kind::Profile, 16), "---- Minecraft Profiler Results ----\n// Shiny numbers!\n\n");
    }
}
