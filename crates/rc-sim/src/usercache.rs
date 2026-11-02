//! The name cache of player profiles kept by the game in its folder (`usercache.json`): name and
//! identifier of every player who joined or was looked up by name, valid one month. A player's
//! join records it (the old name it carried gives the "formerly known as" join message);
//! `/fetchprofile name` reads it before asking the profile service, and records what it found (an
//! unknown name gets its offline identifier). Written whenever an entry is added: the 1000 most
//! recently used, the latest first, in the game's JSON layout and date format.

use std::path::PathBuf;

use hashbrown::HashMap;

/// One recorded profile.
#[derive(Debug, Clone)]
struct Entry {
    id: u128,
    name: String,
    /// Expiry date as written (`yyyy-MM-dd HH:mm:ss Z`) and as an instant (ms).
    expires: String,
    expires_ms: i64,
    /// Order of the last use (most recent: highest).
    last: u64,
}

/// The cache: entries by lower-case name and by identifier (a renamed player keeps its old name
/// entry until it expires, like the game's two maps).
#[derive(Debug, Default)]
pub struct UserCache {
    path: Option<PathBuf>,
    list: Vec<Entry>,
    by_name: HashMap<String, usize>,
    by_id: HashMap<u128, usize>,
    ops: u64,
}

/// Entries written at most.
const MRU_LIMIT: usize = 1000;

/// Identifier in its textual form with dashes.
fn uuid_text(id: u128) -> String {
    let h = format!("{id:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Local offset (seconds) at instant `ms`, with the system's daylight saving rules when known.
fn offset_at(ms: i64) -> i64 {
    crate::storage::local_zone_at(ms).map_or_else(crate::storage::local_offset_secs, |z| z.0)
}

/// `yyyy-MM-dd HH:mm:ss Z` of instant `ms`, in the local time zone of that instant.
fn format_date(ms: i64) -> String {
    let off = offset_at(ms);
    let (y, mo, d, h, mi, s) = crate::storage::civil_from_secs(ms.div_euclid(1000) + off);
    let m = off / 60;
    let sign = if m < 0 { '-' } else { '+' };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} {sign}{:02}{:02}", m.abs() / 60, m.abs() % 60)
}

/// One month after instant `ms`, on the local calendar (same time of day, the day brought back
/// to the length of the month), like the game's expiry.
fn month_later(ms: i64) -> i64 {
    let off = offset_at(ms);
    let (y, mo, d, h, mi, s) = crate::storage::civil_from_secs(ms.div_euclid(1000) + off);
    let (ny, nm) = if mo == 12 { (y + 1, 1) } else { (y, mo + 1) };
    let last = (crate::storage::days_from_civil(if nm == 12 { ny + 1 } else { ny }, if nm == 12 { 1 } else { nm + 1 }, 1)
        - crate::storage::days_from_civil(ny, nm, 1)) as u32;
    let local = crate::storage::days_from_civil(ny, nm, d.min(last)) * 86_400
        + i64::from(h) * 3600
        + i64::from(mi) * 60
        + i64::from(s);
    // The offset of the date reached (daylight saving may differ from now's).
    let guess = (local - off) * 1000;
    (local - offset_at(guess)) * 1000 + ms.rem_euclid(1000)
}

/// JSON string as the game's writer escapes it (HTML characters as `\u00XX`).
fn json_str(s: &str) -> String {
    let mut out = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    for (c, e) in [('<', "\\u003c"), ('>', "\\u003e"), ('&', "\\u0026"), ('=', "\\u003d"), ('\'', "\\u0027")] {
        out = out.replace(c, e);
    }
    out
}

impl UserCache {
    /// Cache read from file `path` (absent or unreadable: empty), its entries used from the last
    /// to the first so that the first stays the most recent.
    pub fn load(path: Option<PathBuf>) -> UserCache {
        let mut c = UserCache { path, ..UserCache::default() };
        let text = c.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let read: Vec<Entry> = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|o| {
                let id = rc_client::auth::parse_uuid(o["uuid"].as_str()?)?;
                let name = o["name"].as_str()?.to_owned();
                let expires = o["expiresOn"].as_str()?.to_owned();
                let expires_ms = crate::progress::parse_date(&expires)?;
                Some(Entry { id, name, expires, expires_ms, last: 0 })
            })
            .collect();
        for e in read.into_iter().rev() {
            c.put(e);
        }
        c
    }

    /// Entry recorded under its name and identifier, as the latest used.
    fn put(&mut self, mut e: Entry) {
        self.ops += 1;
        e.last = self.ops;
        let i = self.list.len();
        self.by_name.insert(e.name.to_lowercase(), i);
        self.by_id.insert(e.id, i);
        self.list.push(e);
    }

    /// Name recorded for identifier `id` (used now), expired or not.
    pub fn name_of(&mut self, id: u128) -> Option<String> {
        let i = *self.by_id.get(&id)?;
        self.ops += 1;
        self.list[i].last = self.ops;
        Some(self.list[i].name.clone())
    }

    /// Records a profile for one month from now, then writes the file.
    pub fn add(&mut self, id: u128, name: &str) {
        let now = crate::progress::now_ms();
        let expires_ms = month_later(now);
        self.put(Entry { id, name: name.to_owned(), expires: format_date(expires_ms), expires_ms, last: 0 });
        self.save();
    }

    /// Identifier recorded for `name` (any case) and still valid; an expired entry is dropped
    /// (both of its keys) and the file written. `None`: to look up.
    pub fn id_of(&mut self, name: &str) -> Option<u128> {
        let i = *self.by_name.get(&name.to_lowercase())?;
        if crate::progress::now_ms() >= self.list[i].expires_ms {
            let (id, key) = (self.list[i].id, self.list[i].name.to_lowercase());
            self.by_id.remove(&id);
            self.by_name.remove(&key);
            self.save();
            return None;
        }
        self.ops += 1;
        self.list[i].last = self.ops;
        Some(self.list[i].id)
    }

    /// The file: the most recently used entries by identifier, the latest first (none for a world
    /// outside a game folder: kept in memory only).
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let mut live: Vec<&Entry> = self.by_id.values().map(|&i| &self.list[i]).collect();
        live.sort_by(|a, b| b.last.cmp(&a.last));
        let body: Vec<String> = live
            .iter()
            .take(MRU_LIMIT)
            .map(|e| {
                format!(
                    "{{\"uuid\":{},\"name\":{},\"expiresOn\":{}}}",
                    json_str(&uuid_text(e.id)),
                    json_str(&e.name),
                    json_str(&e.expires)
                )
            })
            .collect();
        let _ = std::fs::write(path, format!("[{}]", body.join(",")));
    }
}
