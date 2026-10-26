//! Quick play requested by the launcher (`--quickPlaySingleplayer`, `--quickPlayPath`):
//! direct opening of a singleplayer world, and log of the recent games that the launcher reads back
//! (JSON file rewritten on every entry into the game).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Kind of game of the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Singleplayer,
    Multiplayer,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Singleplayer => "singleplayer",
            Kind::Multiplayer => "multiplayer",
        }
    }
}

/// Quick play log: path (`--quickPlayPath`, relative to the game folder) and
/// last game opened (kind, id, name; name `None`: default server name).
#[derive(Debug, Default)]
pub struct Log {
    path: Option<PathBuf>,
    world: Option<(Kind, String, Option<String>)>,
}

impl Log {
    pub fn new(path: Option<&str>, game_dir: &Path) -> Log {
        Log { path: path.map(|p| game_dir.join(p)), world: None }
    }

    /// Game that opens (singleplayer world: folder and level name; server: address and name).
    pub fn set_world(&mut self, kind: Kind, id: &str, name: Option<&str>) {
        if self.path.is_some() {
            self.world = Some((kind, id.to_owned(), name.map(str::to_owned)));
        }
    }

    /// Entry into the game (`login`): log rewritten on a separate thread (a single entry), with the
    /// game mode; `default_name`: name of a server with no entry in the list.
    pub fn log(&self, game_mode: i32, default_name: impl FnOnce() -> String) {
        let (Some(path), Some((kind, id, name))) = (&self.path, &self.world) else {
            if self.path.is_some() {
                eprintln!("quick play: unknown game or game mode, log not written");
            }
            return;
        };
        let name = name.clone().unwrap_or_else(default_name);
        let json = entry_json(*kind, id, &name, SystemTime::now(), game_mode);
        let path = path.clone();
        let _ = std::thread::Builder::new().name("quick-play-log".into()).spawn(move || {
            let _ = std::fs::remove_file(&path);
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("quick play: cannot write the log ({e})");
            }
        });
    }
}

/// Log of a game, in the game's format: list of one entry (kind, id escaped the
/// Java way, name, ISO-8601 instant, game mode), compact JSON with HTML escaping.
pub fn entry_json(kind: Kind, id: &str, name: &str, when: SystemTime, game_mode: i32) -> String {
    let mode = match game_mode {
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => "survival",
    };
    format!(
        "[{{\"type\":{},\"id\":{},\"name\":{},\"lastPlayedTime\":{},\"gamemode\":{}}}]",
        json_str(kind.name()),
        json_str(&escape_java(id)),
        json_str(name),
        json_str(&iso_instant(when)),
        json_str(mode)
    )
}

/// JSON string like the game's library: quote, backslash and controls
/// escaped (`\uXXXX` in lowercase except \t \b \n \r \f), Unicode line separators and
/// HTML characters (`<`, `>`, `&`, `=`, `'`) as `\uXXXX`.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '=' | '\'' | '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Java escaping of a string: quote, backslash, short controls, then every
/// character outside 32..=127 as `\uXXXX` (uppercase, UTF-16 surrogate pairs).
pub fn escape_java(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (32..=0x7f).contains(&(c as u32)) => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{:04X}", u));
                }
            }
        }
    }
    out
}

/// Inverse of `escape_java` for a command-line argument: octal (`\0` to `\377`),
/// `\uXXXX` (one or more `u`, `+` allowed), short controls, `\\`, `\"`, `\'`; a backslash
/// in front of anything else disappears.
pub fn unescape_java(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut units: Vec<u16> = Vec::new();
    let flush = |units: &mut Vec<u16>, out: &mut String| {
        out.extend(char::decode_utf16(units.drain(..)).map(|r| r.unwrap_or('\u{fffd}')));
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            flush(&mut units, &mut out);
            if c != '\\' {
                out.push(c);
            }
            i += 1;
            continue;
        }
        let n = chars[i + 1];
        // \uXXXX: UTF-16 units grouped (surrogate pairs).
        if n == 'u' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] == 'u' {
                j += 1;
            }
            if j < chars.len() && chars[j] == '+' {
                j += 1;
            }
            let hex: String = chars.get(j..j + 4).map(|h| h.iter().collect()).unwrap_or_default();
            if hex.len() == 4
                && let Ok(v) = u16::from_str_radix(&hex, 16)
            {
                units.push(v);
                i = j + 4;
                continue;
            }
        }
        flush(&mut units, &mut out);
        if ('0'..='7').contains(&n) {
            // Octal: up to three digits, the first at most 3 for three digits.
            let mut v = n.to_digit(8).unwrap_or(0);
            let mut j = i + 2;
            let max = if n <= '3' { 3 } else { 2 };
            while j < chars.len() && j < i + 1 + max && ('0'..='7').contains(&chars[j]) {
                v = v * 8 + chars[j].to_digit(8).unwrap_or(0);
                j += 1;
            }
            out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            i = j;
            continue;
        }
        match n {
            'b' => out.push('\u{8}'),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'f' => out.push('\u{c}'),
            'r' => out.push('\r'),
            '\\' | '"' | '\'' => out.push(n),
            other => out.push(other),
        }
        i += 2;
    }
    flush(&mut units, &mut out);
    out
}

/// Instant in the game's ISO-8601 format (UTC, `Z`): fractional second omitted if zero, otherwise in
/// milliseconds, microseconds or nanoseconds depending on its precision.
fn iso_instant(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let nanos = d.subsec_nanos();
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, day) = civil(days);
    let frac = if nanos == 0 {
        String::new()
    } else if nanos.is_multiple_of(1_000_000) {
        format!(".{:03}", nanos / 1_000_000)
    } else if nanos.is_multiple_of(1_000) {
        format!(".{:06}", nanos / 1_000)
    } else {
        format!(".{nanos:09}")
    };
    format!("{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}{frac}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Civil date (proleptic Gregorian) of a number of days since 1 January 1970.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Singleplayer quick play world: given id, or the last world played if empty;
/// `None` if no world of that name (the game's error screen).
pub fn resolve_world(id: &str, game_dir: &Path) -> Option<PathBuf> {
    let id = if id.trim().is_empty() {
        crate::world_list::scan(game_dir).into_iter().next().map(|s| s.id)?
    } else {
        id.to_owned()
    };
    // The game only checks the existence of the folder (a folder without a level fails on opening).
    let dir = game_dir.join("saves").join(&id);
    (!id.trim().is_empty() && dir.is_dir()).then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Expected entry as the game writes it (field by field, Java escapes then JSON).
    #[test]
    fn journal_entry_matches_game_format() {
        let when = UNIX_EPOCH + Duration::new(1_792_665_045, 123_456_700);
        let j = entry_json(Kind::Singleplayer, "My \"world\" é", "Name <1>", when, 1);
        assert_eq!(
            j,
            "[{\"type\":\"singleplayer\",\"id\":\"My \\\\\\\"world\\\\\\\" \\\\u00E9\",\
             \"name\":\"Name \\u003c1\\u003e\",\"lastPlayedTime\":\"2026-10-22T10:30:45.123456700Z\",\
             \"gamemode\":\"creative\"}]"
        );
        let j = entry_json(Kind::Multiplayer, "mc.example.net:25570", "Server", UNIX_EPOCH, 0);
        assert!(j.contains("\"type\":\"multiplayer\"") && j.contains("\"lastPlayedTime\":\"1970-01-01T00:00:00Z\""));
        assert!(j.ends_with("\"gamemode\":\"survival\"}]"), "{j}");
    }

    #[test]
    fn iso_instant_precision() {
        assert_eq!(iso_instant(UNIX_EPOCH + Duration::new(0, 5_000_000)), "1970-01-01T00:00:00.005Z");
        assert_eq!(iso_instant(UNIX_EPOCH + Duration::new(951_782_400, 7_000)), "2000-02-29T00:00:00.000007Z");
    }

    #[test]
    fn java_unescape_of_arguments() {
        assert_eq!(unescape_java("My\\ world\\u00e9\\t\\101\\\\"), "My world\u{e9}\tA\\");
        assert_eq!(unescape_java("\\uD83D\\uDE00"), "\u{1F600}");
        assert_eq!(escape_java("\u{1F600}\""), "\\uD83D\\uDE00\\\"");
        assert_eq!(unescape_java(&escape_java("a\\b\"c é")), "a\\b\"c é");
    }
}
