//! Game identity for `/version`: `version.json` of the jar read once per process, build date
//! written like a Java date text (English, local time zone).

use std::sync::OnceLock;

use rc_data::{BuildInfo, VersionData};

/// Identity of the played version's jar; `None` if the jar or its file is missing.
pub(crate) fn get(data: &VersionData) -> Option<BuildInfo> {
    static INFO: OnceLock<Option<BuildInfo>> = OnceLock::new();
    INFO.get_or_init(|| crate::registries::client_jar(data).and_then(|jar| rc_data::build_info(&jar).ok())).clone()
}

/// Instant (ms since 1970) of an ISO 8601 date with offset (`2026-09-15T11:20:48+00:00`, `Z`,
/// fractional second allowed).
pub fn parse_iso(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    let num = |a: usize, n: usize| -> Option<i64> { std::str::from_utf8(b.get(a..a + n)?).ok()?.parse().ok() };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, s) = (num(0, 4)?, num(5, 2)?, num(8, 2)?, num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let mut i = 19;
    let mut frac_ms = 0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let digits = &t[start..i];
        frac_ms = format!("{digits:0<3}")[..3].parse::<i64>().ok()?;
    }
    let offset = match b.get(i)? {
        b'Z' => 0,
        c @ (b'+' | b'-') => {
            let v = num(i + 1, 2)? * 3600 + num(i + 4, 2)? * 60;
            if *c == b'-' { -v } else { v }
        }
        _ => return None,
    };
    let days = crate::storage::days_from_civil(y, mo as u32, d as u32);
    Some((days * 86_400 + h * 3600 + mi * 60 + s - offset) * 1000 + frac_ms)
}

/// Short time zone name as Java writes it for the Windows key: table recorded on a JVM 25
/// (zone of region "001" of the Windows mapping file, English names); zone without a name
/// `GMT±hh:mm`, like Java.
pub fn java_zone_name(key: &str, dst: bool, offset_secs: i64) -> String {
    let pair = match key {
        "AUS Central Standard Time" | "Cen. Australia Standard Time" => Some(("ACST", "ACDT")),
        "AUS Eastern Standard Time" | "E. Australia Standard Time" | "Tasmania Standard Time" => Some(("AEST", "AEDT")),
        "Alaskan Standard Time" => Some(("AKST", "AKDT")),
        "Atlantic Standard Time" => Some(("AST", "ADT")),
        "W. Australia Standard Time" => Some(("AWST", "AWDT")),
        "Namibia Standard Time" | "South Sudan Standard Time" | "Sudan Standard Time" => Some(("CAT", "CAT")),
        "Central Europe Standard Time"
        | "Central European Standard Time"
        | "Romance Standard Time"
        | "W. Europe Standard Time" => Some(("CET", "CEST")),
        "Canada Central Standard Time"
        | "Central America Standard Time"
        | "Central Standard Time"
        | "Central Standard Time (Mexico)"
        | "China Standard Time"
        | "Cuba Standard Time"
        | "Taipei Standard Time" => Some(("CST", "CDT")),
        "E. Africa Standard Time" => Some(("EAT", "EAT")),
        "E. Europe Standard Time"
        | "Egypt Standard Time"
        | "FLE Standard Time"
        | "GTB Standard Time"
        | "Kaliningrad Standard Time"
        | "Libya Standard Time"
        | "Middle East Standard Time"
        | "West Bank Standard Time" => Some(("EET", "EEST")),
        "Eastern Standard Time"
        | "Eastern Standard Time (Mexico)"
        | "Haiti Standard Time"
        | "Turks And Caicos Standard Time"
        | "US Eastern Standard Time" => Some(("EST", "EDT")),
        "GMT Standard Time" => Some(("GMT", "BST")),
        "Greenwich Standard Time" | "Sao Tome Standard Time" => Some(("GMT", "GMT")),
        "Morocco Standard Time" => Some(("GMT", "GMT+01:00")),
        "Aleutian Standard Time" => Some(("HAST", "HADT")),
        "Hawaiian Standard Time" => Some(("HST", "HDT")),
        "Israel Standard Time" => Some(("IST", "IDT")),
        "India Standard Time" | "Sri Lanka Standard Time" => Some(("IST", "IST")),
        "Tokyo Standard Time" => Some(("JST", "JDT")),
        "Korea Standard Time" | "North Korea Standard Time" => Some(("KST", "KDT")),
        "Belarus Standard Time" | "Russian Standard Time" | "Volgograd Standard Time" => Some(("MSK", "MSK")),
        "Mountain Standard Time" | "Mountain Standard Time (Mexico)" | "US Mountain Standard Time" => {
            Some(("MST", "MDT"))
        }
        "Yukon Standard Time" => Some(("MST", "MST")),
        "Newfoundland Standard Time" => Some(("NST", "NDT")),
        "New Zealand Standard Time" => Some(("NZST", "NZDT")),
        "Pakistan Standard Time" => Some(("PKT", "PKST")),
        "Pacific Standard Time" | "Pacific Standard Time (Mexico)" => Some(("PST", "PDT")),
        "South Africa Standard Time" => Some(("SAST", "SAST")),
        "UTC" => Some(("UTC", "UTC")),
        "W. Central Africa Standard Time" => Some(("WAT", "WAT")),
        _ => None,
    };
    if let Some((std, summer)) = pair {
        return if dst { summer } else { std }.to_owned();
    }
    if offset_secs == 0 {
        return "GMT".to_owned();
    }
    let a = offset_secs.abs() / 60;
    format!("GMT{}{:02}:{:02}", if offset_secs < 0 { '-' } else { '+' }, a / 60, a % 60)
}

/// Text of a Java date (`Tue Sep 15 13:20:48 CEST 2026`) for an instant, an offset and a
/// zone name.
pub fn java_date(ms: i64, offset_secs: i64, zone: &str) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let local = ms.div_euclid(1000) + offset_secs;
    let (y, mo, d, h, mi, s) = crate::storage::civil_from_secs(local);
    let dow = DAYS[local.div_euclid(86_400).rem_euclid(7) as usize];
    format!("{dow} {} {d:02} {h:02}:{mi:02}:{s:02} {zone} {y:04}", MONTHS[mo as usize - 1])
}

/// Build date written like the game, in the system's time zone; raw text if unreadable.
pub(crate) fn java_date_local(iso: &str) -> String {
    let Some(ms) = parse_iso(iso) else { return iso.to_owned() };
    let (offset, dst, key) = crate::storage::local_zone_at(ms)
        .unwrap_or_else(|| (crate::storage::local_offset_secs(), false, String::new()));
    java_date(ms, offset, &java_zone_name(&key, dst, offset))
}
