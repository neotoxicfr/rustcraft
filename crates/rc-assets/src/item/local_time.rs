//! `local_time` property of the item definitions: ICU-style lettered date pattern (root
//! locale `""` by default), optional time zone, text redone at most once per second.
//!
//! Supported letters: G y Y u Q q M L d D F E e c a h H k K m s S A z O Z X x, text between
//! apostrophes, `''` for an apostrophe. Names: root locale (months `M01`…, days `Sun`…) or
//! English for any other locale (only English locales are exact). Not
//! supported (nothing written): weeks (w W, exact Y), U g b B v V.

use super::{EvalEnv, SelectProp, TimeZone};

const MONTHS_EN: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS_EN: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// Days since 1970 → (year, month 1..12, day 1..31).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// (year, month, day) → days since 1970.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn pad(out: &mut String, v: i64, width: usize) {
    use std::fmt::Write;
    let _ = write!(out, "{v:0width$}");
}

/// Offset `+hh[mm]` / `+hh:mm` (`colon`), minutes omitted if zero and `short`.
fn offset(out: &mut String, off: i32, colon: bool, short: bool) {
    out.push(if off < 0 { '-' } else { '+' });
    let a = off.unsigned_abs();
    pad(out, i64::from(a / 3600), 2);
    if short && a % 3600 / 60 == 0 {
        return;
    }
    if colon {
        out.push(':');
    }
    pad(out, i64::from(a % 3600 / 60), 2);
}

/// `GMT`, `GMT+1`, `GMT+5:30` (short) or `GMT+01:00` (long).
fn gmt(out: &mut String, off: i32, long: bool) {
    out.push_str("GMT");
    if off == 0 {
        return;
    }
    if long {
        return offset(out, off, true, false);
    }
    out.push(if off < 0 { '-' } else { '+' });
    let a = off.unsigned_abs();
    pad(out, i64::from(a / 3600), 1);
    if a % 3600 / 60 != 0 {
        out.push(':');
        pad(out, i64::from(a % 3600 / 60), 2);
    }
}

/// Text of a pattern at instant `ms` (UTC) shifted by `off` seconds.
pub(super) fn format(pattern: &str, locale: &str, ms: i64, off: i32, out: &mut String) {
    let root = locale.is_empty() || locale == "root";
    let t = ms + i64::from(off) * 1000;
    let days = t.div_euclid(86_400_000);
    let in_day = t.rem_euclid(86_400_000);
    let (y, mo, d) = civil(days);
    let wd = (days + 4).rem_euclid(7) as usize; // 0 = Sunday
    let doy = days - days_from_civil(y, 1, 1) + 1;
    let (h, mi, s, milli) = (in_day / 3_600_000, in_day / 60_000 % 60, in_day / 1000 % 60, in_day % 1000);
    // First day of the week: Monday (world, root locale), Sunday (English).
    let first = usize::from(root);
    let month = |out: &mut String, n: usize| {
        let i = mo as usize - 1;
        match n {
            1 | 2 => pad(out, i64::from(mo), n),
            5 if root => pad(out, i64::from(mo), 1),
            5 => out.push_str(&MONTHS_EN[i][..1]),
            _ if root => {
                out.push('M');
                pad(out, i64::from(mo), 2);
            }
            3 => out.push_str(&MONTHS_EN[i][..3]),
            _ => out.push_str(MONTHS_EN[i]),
        }
    };
    let day_name = |out: &mut String, n: usize| match n {
        5 => out.push_str(&DAYS_EN[wd][..1]),
        6 => out.push_str(&DAYS_EN[wd][..2]),
        4 if !root => out.push_str(DAYS_EN[wd]),
        _ => out.push_str(&DAYS_EN[wd][..3]),
    };
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if !c.is_ascii_alphabetic() {
            out.push(c);
            i += 1;
            continue;
        }
        let mut n = 1;
        while chars.get(i + n) == Some(&c) {
            n += 1;
        }
        i += n;
        match c {
            'G' => out.push_str(if root { "CE" } else if n == 4 { "Anno Domini" } else if n == 5 { "A" } else { "AD" }),
            'y' | 'Y' | 'u' => {
                if n == 2 {
                    pad(out, y.rem_euclid(100), 2)
                } else {
                    pad(out, y, n)
                }
            }
            'Q' | 'q' => {
                let q = (mo - 1) / 3 + 1;
                match n {
                    1 | 2 => pad(out, i64::from(q), n),
                    3 => {
                        out.push('Q');
                        pad(out, i64::from(q), 1);
                    }
                    _ if root => {
                        out.push('Q');
                        pad(out, i64::from(q), 1);
                    }
                    _ => out.push_str(["1st quarter", "2nd quarter", "3rd quarter", "4th quarter"][q as usize - 1]),
                }
            }
            'M' | 'L' => month(out, n),
            'd' => pad(out, i64::from(d), n),
            'D' => pad(out, doy, n),
            'F' => pad(out, i64::from((d - 1) / 7 + 1), n),
            'E' => day_name(out, n),
            'e' | 'c' if n <= 2 => pad(out, ((wd + 7 - first) % 7 + 1) as i64, n),
            'e' | 'c' => day_name(out, n),
            'a' => out.push_str(if h < 12 { "AM" } else { "PM" }),
            'h' => pad(out, if h % 12 == 0 { 12 } else { h % 12 }, n),
            'H' => pad(out, h, n),
            'k' => pad(out, if h == 0 { 24 } else { h }, n),
            'K' => pad(out, h % 12, n),
            'm' => pad(out, mi, n),
            's' => pad(out, s, n),
            // Fraction of a second: first `n` digits of the milliseconds (zeros beyond).
            'S' => {
                let digits = format!("{milli:03}");
                for k in 0..n {
                    out.push(digits.as_bytes().get(k).map_or('0', |&b| b as char));
                }
            }
            'A' => pad(out, in_day, n),
            'z' | 'O' => gmt(out, off, n >= 4),
            'Z' => match n {
                1..=3 => offset(out, off, false, false),
                4 => gmt(out, off, true),
                _ if off == 0 => out.push('Z'),
                _ => offset(out, off, true, false),
            },
            'X' | 'x' => {
                if c == 'X' && off == 0 {
                    out.push('Z');
                } else {
                    match n {
                        1 => offset(out, off, false, true),
                        2 | 4 => offset(out, off, false, false),
                        _ => offset(out, off, true, false),
                    }
                }
            }
            _ => {}
        }
    }
}

/// Value of a `local_time` property passed to `f` (without copy); `None` without a clock.
pub(super) fn with_value<R>(p: &SelectProp, env: &dyn EvalEnv, f: impl FnOnce(Option<&str>) -> R) -> R {
    let SelectProp::LocalTime { pattern, locale, zone, cache } = p else { return f(None) };
    let Some(now) = env.now_ms() else { return f(None) };
    let mut c = cache.0.borrow_mut();
    // Pacing on the monotonic clock, text on the wall clock (like the game).
    let up = env.uptime_ms();
    if up > c.0 {
        let off = match *zone {
            TimeZone::System => env.utc_offset_secs(now) as i32,
            TimeZone::Fixed(s) => s,
        };
        c.1.clear();
        format(pattern, locale, now, off, &mut c.1);
        c.0 = up.saturating_add(1000);
    }
    f(Some(&c.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(p: &str, locale: &str, ms: i64, off: i32) -> String {
        let mut s = String::new();
        format(p, locale, ms, off, &mut s);
        s
    }

    /// 2026-12-24 23:30:05.123 UTC (Thursday).
    const T: i64 = 1_798_155_005_123;

    #[test]
    fn numeric_patterns_and_time_zones() {
        assert_eq!(fmt("MM-dd", "", T, 0), "12-24");
        // One hour further east: already the 25th.
        assert_eq!(fmt("MM-dd", "", T, 3600), "12-25");
        assert_eq!(fmt("yyyy-MM-dd HH:mm:ss.SSS", "", T, 0), "2026-12-24 23:30:05.123");
        assert_eq!(fmt("yy D F h a k K S A", "", T, 0), "26 358 4 11 PM 23 11 1 84605123");
        assert_eq!(fmt("'the' dd '''x''' ''", "", T, 0), "the 24 'x' '");
        assert_eq!(fmt("Z ZZZZ ZZZZZ X XXX x z", "", T, -19_800), "-0530 GMT-05:30 -05:30 -0530 -05:30 -0530 GMT-5:30");
        assert_eq!(fmt("X x O", "", T, 0), "Z +00 GMT");
    }

    #[test]
    fn root_and_english_names() {
        assert_eq!(fmt("MMM MMMM MMMMM EEE EEEE e G QQQ", "", T, 0), "M12 M12 12 Thu Thu 4 CE Q4");
        assert_eq!(fmt("MMM MMMM MMMMM EEE EEEE EEEEE e G QQQQ", "en_US", T, 0), "Dec December D Thu Thursday T 5 AD 4th quarter");
    }

    #[test]
    fn time_zones_read() {
        use super::super::time_zone;
        assert_eq!(time_zone(None), TimeZone::System);
        assert_eq!(time_zone(Some("UTC")), TimeZone::Fixed(0));
        assert_eq!(time_zone(Some("GMT+1")), TimeZone::Fixed(3600));
        assert_eq!(time_zone(Some("GMT-05:30")), TimeZone::Fixed(-19_800));
        assert_eq!(time_zone(Some("GMT+0130")), TimeZone::Fixed(5400));
        assert_eq!(time_zone(Some("Etc/GMT+3")), TimeZone::Fixed(-10_800));
        assert_eq!(time_zone(Some("Europe/Paris")), TimeZone::System);
    }
}
