//! List of the saved servers (`servers.dat`, uncompressed NBT), `docs/specs/title-screen-servers.md`
//! § 2.1: reading, safe writing, hidden list and icon updates (screen: `server_list.rs`).

use std::path::{Path, PathBuf};

use base64::Engine;
use rc_proto::Reader;
use rc_proto::nbt::{self, Nbt};

/// Maximum number of hidden entries (from direct connections).
pub const HIDDEN_MAX: usize = 16;

/// Rustcraft's game folder (`RUSTCRAFT_GAME_DIR`, otherwise `%LOCALAPPDATA%\rustcraft`, next to the
/// cache): distinct from `.minecraft` so as never to rewrite the official game's list.
pub fn game_dir() -> PathBuf {
    let env = |v: Option<&std::ffi::OsStr>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    env(rc_data::env_os!("RUSTCRAFT_GAME_DIR"))
        .unwrap_or_else(|| env(rc_data::env_os!("LOCALAPPDATA")).unwrap_or_else(std::env::temp_dir).join("rustcraft"))
}

/// Allowed symbolic links (`allowed_symlinks.txt` of the game folder), read once on first
/// use like the game reads them at startup: worlds, resource packs, dropped packs.
pub fn allowed_symlinks() -> &'static rc_data::symlinks::SymlinkRules {
    static LIST: std::sync::OnceLock<rc_data::symlinks::SymlinkRules> = std::sync::OnceLock::new();
    LIST.get_or_init(|| rc_data::symlinks::SymlinkRules::load(&game_dir()))
}

/// Status of the server's resource packs for this entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PackStatus {
    Enabled,
    Disabled,
    #[default]
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerEntry {
    pub name: String,
    /// Address as typed.
    pub ip: String,
    /// Raw PNG (stored as base64 in the file).
    pub icon: Option<Vec<u8>>,
    pub pack_status: PackStatus,
    /// Fingerprint of the accepted code of conduct (0 = none).
    pub accepted_code_of_conduct: i32,
}

impl ServerEntry {
    pub fn new(name: impl Into<String>, ip: impl Into<String>) -> Self {
        ServerEntry { name: name.into(), ip: ip.into(), ..Default::default() }
    }

    fn from_nbt(t: &Nbt) -> Self {
        let s = |k: &str| t.get(k).and_then(Nbt::as_str).map(str::to_owned);
        ServerEntry {
            name: s("name").unwrap_or_default(),
            ip: s("ip").unwrap_or_default(),
            icon: s("icon").and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()),
            pack_status: match t.get("acceptTextures") {
                Some(Nbt::Byte(b)) if *b != 0 => PackStatus::Enabled,
                Some(Nbt::Byte(_)) => PackStatus::Disabled,
                _ => PackStatus::Prompt,
            },
            accepted_code_of_conduct: match t.get("acceptedCodeOfConduct") {
                Some(Nbt::Int(v)) => *v,
                _ => 0,
            },
        }
    }

    fn to_nbt(&self, hidden: bool) -> Nbt {
        let mut f = vec![("name".into(), Nbt::String(self.name.clone())), ("ip".into(), Nbt::String(self.ip.clone()))];
        if let Some(icon) = &self.icon {
            f.push(("icon".into(), Nbt::String(base64::engine::general_purpose::STANDARD.encode(icon))));
        }
        match self.pack_status {
            PackStatus::Enabled => f.push(("acceptTextures".into(), Nbt::Byte(1))),
            PackStatus::Disabled => f.push(("acceptTextures".into(), Nbt::Byte(0))),
            PackStatus::Prompt => {}
        }
        if self.accepted_code_of_conduct != 0 {
            f.push(("acceptedCodeOfConduct".into(), Nbt::Int(self.accepted_code_of_conduct)));
        }
        f.push(("hidden".into(), Nbt::Byte(hidden as i8)));
        Nbt::Compound(f)
    }
}

/// Java-style string hash (`s[0]·31^(n−1) + …` over the UTF-16 units), for the code of conduct.
pub fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(u as i32))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerList {
    pub visible: Vec<ServerEntry>,
    pub hidden: Vec<ServerEntry>,
}

/// New icon (raw PNG, `None`: no icon) of the entry with this name and this address.
#[derive(Debug, Clone, Copy)]
pub struct IconUpdate<'a> {
    pub name: &'a str,
    pub ip: &'a str,
    pub icon: Option<&'a [u8]>,
}

impl ServerList {
    /// Decodes the content of a `servers.dat`; error ⇒ empty list.
    pub fn decode(bytes: &[u8]) -> Self {
        let mut list = ServerList::default();
        let Ok(Some((_, root))) = nbt::read_named(&mut Reader::new(bytes)) else { return list };
        if let Some(Nbt::List(items)) = root.get("servers") {
            for t in items.iter().filter(|t| matches!(t, Nbt::Compound(_))) {
                let e = ServerEntry::from_nbt(t);
                if matches!(t.get("hidden"), Some(Nbt::Byte(b)) if *b != 0) {
                    list.hidden.push(e);
                } else {
                    list.visible.push(e);
                }
            }
        }
        list
    }

    /// Visible first, then hidden; unnamed root.
    pub fn encode(&self) -> Vec<u8> {
        let items =
            self.visible.iter().map(|e| e.to_nbt(false)).chain(self.hidden.iter().map(|e| e.to_nbt(true))).collect();
        let root = Nbt::Compound(vec![("servers".into(), Nbt::List(items))]);
        let mut w = Vec::new();
        nbt::write_named(&mut w, "", &root);
        w
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join("servers.dat")).map(|b| Self::decode(&b)).unwrap_or_default()
    }

    /// Writes to a temporary file in the folder then replaces: the old `servers.dat` becomes
    /// `servers.dat_old`.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        tests::count_save();
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("servers{}.dat.tmp", std::process::id()));
        std::fs::write(&tmp, self.encode())?;
        let (cur, old) = (dir.join("servers.dat"), dir.join("servers.dat_old"));
        if cur.exists() {
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&cur, &old)?;
        }
        std::fs::rename(&tmp, &cur).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Search by exact address: visible, then hidden.
    pub fn find(&self, ip: &str) -> Option<&ServerEntry> {
        self.visible.iter().chain(&self.hidden).find(|e| e.ip == ip)
    }

    /// Add from the "add" screen: a hidden entry with the same address is unhidden (moved
    /// to the end of the visible ones, name and icon taken from the input).
    pub fn add(&mut self, entry: ServerEntry) {
        if let Some(i) = self.hidden.iter().position(|e| e.ip == entry.ip) {
            let mut e = self.hidden.remove(i);
            e.name = entry.name;
            e.icon = entry.icon;
            self.visible.push(e);
        } else {
            self.visible.push(entry);
        }
    }

    /// Direct connection: added at the head of the hidden ones, 16 at most (the oldest fall off).
    pub fn add_hidden(&mut self, entry: ServerEntry) {
        self.hidden.insert(0, entry);
        self.hidden.truncate(HIDDEN_MAX);
    }

    /// Icon received by ping: rereads the file, replaces the icon of the entry with the same name and same
    /// address, rewrites. Returns `true` if an entry was modified.
    pub fn update_icon(dir: &Path, name: &str, ip: &str, icon: Option<Vec<u8>>) -> std::io::Result<bool> {
        Self::update_icons(dir, &[IconUpdate { name, ip, icon: icon.as_deref() }]).map(|changed| changed > 0)
    }

    /// Icons received together: the file is reread once, each update replaces the icon of the first
    /// entry with its name and address (visible, then hidden), and the file is rewritten once if
    /// any entry was found. Leaves the same `servers.dat` as one [`ServerList::update_icon`] per
    /// update, in the same order. Returns the number of updates that found their entry.
    pub fn update_icons(dir: &Path, updates: &[IconUpdate]) -> std::io::Result<usize> {
        let mut list = Self::load(dir);
        let mut changed = 0;
        for update in updates {
            let mut entries = list.visible.iter_mut().chain(list.hidden.iter_mut());
            if let Some(entry) = entries.find(|e| e.name == update.name && e.ip == update.ip) {
                entry.icon = update.icon.map(<[u8]>::to_vec);
                changed += 1;
            }
        }
        if changed > 0 {
            list.save(dir)?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    thread_local! {
        /// Writes of `servers.dat` made by the current thread (each test has its own thread).
        static SAVES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    pub(super) fn count_save() {
        SAVES.with(|saves| saves.set(saves.get() + 1));
    }

    /// Writes of `servers.dat` made by the calling test so far.
    pub(crate) fn saves() -> usize {
        SAVES.with(std::cell::Cell::get)
    }

    fn sample() -> ServerList {
        let mut a = ServerEntry::new("Local", "127.0.0.1:25570");
        a.icon = Some(vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
        a.pack_status = PackStatus::Enabled;
        a.accepted_code_of_conduct = java_string_hash("Be nice");
        let mut b = ServerEntry::new("Café ☃ 𝄞", "[::1]:25565");
        b.pack_status = PackStatus::Disabled;
        ServerList { visible: vec![a, b], hidden: vec![ServerEntry::new("", "direct.example")] }
    }

    #[test]
    fn roundtrip_and_layout() {
        let l = sample();
        let bytes = l.encode();
        // Root: compound (10), empty name, then "servers" list of compounds.
        assert_eq!(&bytes[..3], &[10, 0, 0]);
        assert_eq!(&bytes[3..14], &[9, 0, 7, b's', b'e', b'r', b'v', b'e', b'r', b's', 10]);
        assert_eq!(ServerList::decode(&bytes), l);
        // Optional fields omitted.
        let plain = ServerList { visible: vec![ServerEntry::new("a", "b")], hidden: vec![] }.encode();
        let root = nbt::read_named(&mut Reader::new(&plain)).unwrap().unwrap().1;
        let Some(Nbt::List(items)) = root.get("servers") else { panic!() };
        let keys: Vec<&str> = match &items[0] {
            Nbt::Compound(f) => f.iter().map(|(k, _)| k.as_str()).collect(),
            _ => panic!(),
        };
        assert_eq!(keys, ["name", "ip", "hidden"]);
    }

    #[test]
    fn tolerant_decode() {
        assert_eq!(ServerList::decode(&[]), ServerList::default());
        assert_eq!(ServerList::decode(&[0x1f, 0x8b, 8, 0]), ServerList::default());
        // Invalid base64 icon ⇒ absent; `hidden` absent ⇒ visible.
        let root = Nbt::Compound(vec![(
            "servers".into(),
            Nbt::List(vec![Nbt::Compound(vec![
                ("ip".into(), Nbt::String("x".into())),
                ("icon".into(), Nbt::String("!!".into())),
            ])]),
        )]);
        let mut w = Vec::new();
        nbt::write_named(&mut w, "", &root);
        let l = ServerList::decode(&w);
        assert_eq!(l.visible, vec![ServerEntry::new("", "x")]);
    }

    #[test]
    fn hidden_rules() {
        let mut l = ServerList::default();
        for i in 0..20 {
            l.add_hidden(ServerEntry::new("", format!("h{i}")));
        }
        assert_eq!(l.hidden.len(), HIDDEN_MAX);
        assert_eq!(l.hidden[0].ip, "h19");
        assert_eq!(l.find("h4").map(|e| e.ip.as_str()), Some("h4"));
        assert!(l.find("h3").is_none());
        let mut e = ServerEntry::new("Named", "h10");
        e.icon = Some(vec![1]);
        l.add(e);
        assert_eq!(l.hidden.len(), HIDDEN_MAX - 1);
        assert_eq!(l.visible.last().map(|e| (e.name.as_str(), e.icon.clone())), Some(("Named", Some(vec![1]))));
    }

    #[test]
    fn save_keeps_old_and_updates_icon() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-servers-test-{}", std::process::id())));
        let l = sample();
        l.save(&dir).unwrap();
        assert!(!dir.join("servers.dat_old").exists());
        assert!(ServerList::update_icon(&dir, "Local", "127.0.0.1:25570", None).unwrap());
        assert!(!ServerList::update_icon(&dir, "Other", "127.0.0.1:25570", None).unwrap());
        assert_eq!(ServerList::load(&dir).visible.len(), 2);
        let old = ServerList::decode(&std::fs::read(dir.join("servers.dat_old")).unwrap());
        assert_eq!(old, l);
        assert_eq!(ServerList::load(&dir).visible[0].icon, None);
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 2, "no temporary file left: {names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Icons written together leave the same `servers.dat` as one write per icon, in one write:
    /// eight entries, one of them twice in the list (the first one takes the icon), one hidden,
    /// one update for an unknown entry, one that removes an icon.
    #[test]
    fn icons_written_together_leave_the_same_file_in_one_write() {
        let mut list = ServerList::default();
        for k in 0..8 {
            list.visible.push(ServerEntry::new(format!("Server {k}"), format!("s{k}.example:2556{k}")));
        }
        list.visible.push(ServerEntry::new("Server 3", "s3.example:25563"));
        list.visible[5].icon = Some(vec![9, 9, 9]);
        list.hidden.push(ServerEntry::new("Hidden", "direct.example"));
        let icons: Vec<Vec<u8>> = (0..8u8).map(|k| vec![0x89, b'P', b'N', b'G', k, k.wrapping_mul(7)]).collect();
        let mut updates: Vec<IconUpdate> = list.visible[..8]
            .iter()
            .zip(&icons)
            .map(|(e, icon)| IconUpdate { name: &e.name, ip: &e.ip, icon: Some(icon) })
            .collect();
        updates[5].icon = None;
        updates.push(IconUpdate { name: "Hidden", ip: "direct.example", icon: Some(&icons[0]) });
        updates.push(IconUpdate { name: "Unknown", ip: "nowhere.example", icon: Some(&icons[1]) });
        let folder = |name: &str| {
            let dir = rc_testdata::scratch(
                std::env::temp_dir().join(format!("rc-servers-icons-{name}-{}", std::process::id())),
            );
            list.save(&dir).unwrap();
            dir
        };
        let (one_by_one, together) = (folder("one-by-one"), folder("together"));
        let before = saves();
        for u in &updates {
            ServerList::update_icon(&one_by_one, u.name, u.ip, u.icon.map(<[u8]>::to_vec)).unwrap();
        }
        assert_eq!(saves() - before, 9, "one write per update that finds its entry");
        let before = saves();
        assert_eq!(ServerList::update_icons(&together, &updates).unwrap(), 9);
        assert_eq!(saves() - before, 1, "one write for the whole batch");
        let file = |dir: &Path| std::fs::read(dir.join("servers.dat")).unwrap();
        assert_eq!(file(&together), file(&one_by_one), "same servers.dat");
        let written = ServerList::load(&together);
        assert_eq!(written.visible[3].icon.as_deref(), Some(&icons[3][..]));
        assert_eq!(written.visible[8].icon, None, "the second entry of the same name and address is left alone");
        assert_eq!(written.visible[5].icon, None);
        assert_eq!(written.hidden[0].icon.as_deref(), Some(&icons[0][..]));
        // Nothing found: nothing written.
        let before = saves();
        let unknown = [IconUpdate { name: "Unknown", ip: "nowhere.example", icon: None }];
        assert_eq!(ServerList::update_icons(&together, &unknown).unwrap(), 0);
        assert_eq!(saves(), before);
    }

    #[test]
    fn string_hash() {
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("hello"), 99162322);
    }
}
