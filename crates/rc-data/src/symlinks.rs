//! Allowed symbolic links (`allowed_symlinks.txt` in the game folder), like the game: a world
//! or a pack that contains a link whose target is not allowed is not opened.
//!
//! File format: one rule per line; blank line or one starting with `#` ignored;
//! `[prefix]…` or bare text (start of the target), `[glob]…` or `[regex]…` (whole target). An
//! unknown type or an unclosed `[` makes the whole file invalid: no link allowed. The target
//! compared is the one written in the link (relative or not), system separators; on
//! Windows, case-insensitive patterns.
//!
//! Known difference: a Windows junction counts here as a link (the game walks it as a
//! folder); the standard library does not tell them apart.

use std::path::{Path, PathBuf};

/// Refused link: path of the link and its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub link: PathBuf,
    pub target: PathBuf,
}

enum Rule {
    Prefix(String),
    Pattern(fancy_regex::Regex),
}

/// Rules of `allowed_symlinks.txt`; empty: no link allowed.
#[derive(Default)]
pub struct SymlinkRules {
    rules: Vec<Rule>,
}

/// Special characters of a regular expression (escaped when literal).
const REGEX_META: &str = ".^$+{[]|()";
/// Special characters of a glob pattern.
const GLOB_META: &str = "\\*?[{";

/// Glob pattern (Java path syntax) translated into a regular expression; `windows`:
/// separator `\` (a `/` of the pattern equals `\`).
fn glob_regex(glob: &str, windows: bool) -> Result<String, String> {
    let not_sep = if windows { "[^\\\\]" } else { "[^/]" };
    let mut out = String::from("^");
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    let mut group = false;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '\\' => {
                let Some(&n) = chars.get(i) else { return Err("trailing escape".into()) };
                i += 1;
                if GLOB_META.contains(n) || REGEX_META.contains(n) {
                    out.push('\\');
                }
                out.push(n);
            }
            '/' if windows => out.push_str("\\\\"),
            '[' => {
                out.push_str(if windows { "[[^\\\\]&&[" } else { "[[^/]&&[" });
                match chars.get(i) {
                    Some('^') => {
                        out.push_str("\\^");
                        i += 1;
                    }
                    Some('!') => {
                        out.push('^');
                        i += 1;
                    }
                    _ => {}
                }
                if chars.get(i) == Some(&'-') {
                    out.push('-');
                    i += 1;
                }
                let mut closed = false;
                while let Some(&k) = chars.get(i) {
                    i += 1;
                    match k {
                        ']' => {
                            closed = true;
                            break;
                        }
                        '/' => return Err("separator in a class".into()),
                        '\\' | '[' => {
                            out.push('\\');
                            out.push(k);
                        }
                        '&' if chars.get(i) == Some(&'&') => out.push_str("\\&"),
                        k => out.push(k),
                    }
                }
                if !closed {
                    return Err("unclosed class".into());
                }
                out.push_str("]]");
            }
            '{' => {
                if group {
                    return Err("nested groups".into());
                }
                out.push_str("(?:(?:");
                group = true;
            }
            '}' if group => {
                out.push_str("))");
                group = false;
            }
            ',' if group => out.push_str(")|(?:"),
            '*' if chars.get(i) == Some(&'*') => {
                out.push_str(".*");
                i += 1;
            }
            '*' => {
                out.push_str(not_sep);
                out.push('*');
            }
            '?' => out.push_str(not_sep),
            c => {
                if REGEX_META.contains(c) || c == '\\' {
                    out.push('\\');
                }
                out.push(c);
            }
        }
    }
    if group {
        return Err("unclosed group".into());
    }
    out.push('$');
    Ok(out)
}

/// Target as the game compares it: system separators, without the namespace prefix
/// of Windows that the standard library returns (`\\?\C:\…`, `\\?\UNC\…`).
fn target_text(target: &Path) -> String {
    let s = target.to_string_lossy();
    if cfg!(windows) {
        let s = s.replace('/', "\\");
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        return match s.strip_prefix(r"\\?\") {
            Some(rest) => rest.to_owned(),
            None => s,
        };
    }
    s.into_owned()
}

/// Target of a link to follow: relative, taken from the link's folder (the game takes it from
/// its working folder, which in practice is the game folder; not reproduced); without the
/// namespace prefix.
fn follow(link: &Path) -> std::io::Result<PathBuf> {
    let t = std::fs::read_link(link)?;
    let t = match link.parent() {
        Some(p) if t.is_relative() => p.join(&t),
        _ => t,
    };
    Ok(PathBuf::from(target_text(&t)))
}

impl SymlinkRules {
    /// Rules of a text; `Err`: invalid file (the game then allows no link).
    pub fn parse(text: &str) -> Result<SymlinkRules, String> {
        Self::parse_for(text, cfg!(windows))
    }

    fn parse_for(text: &str, windows: bool) -> Result<SymlinkRules, String> {
        let mut specs = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let Some(rest) = line.strip_prefix('[') else {
                specs.push((None, line));
                continue;
            };
            let Some(end) = rest.find(']') else { return Err(format!("unclosed type: '{line}'")) };
            let (kind, pattern) = (&rest[..end], &rest[end + 1..]);
            match kind {
                "glob" | "regex" | "prefix" => specs.push((Some(kind), pattern)),
                _ => return Err(format!("unknown type: '{line}'")),
            }
        }
        // A pattern that does not compile silences the whole list (no link allowed).
        let mut rules = Vec::with_capacity(specs.len());
        let case = if windows { "(?i)" } else { "" };
        for (kind, pattern) in specs {
            let expr = match kind {
                None | Some("prefix") => {
                    rules.push(Rule::Prefix(pattern.to_owned()));
                    continue;
                }
                Some("glob") => match glob_regex(pattern, windows) {
                    Ok(e) => e,
                    Err(_) => return Ok(SymlinkRules::default()),
                },
                _ => format!("^(?:{pattern})$"),
            };
            match fancy_regex::Regex::new(&format!("{case}{expr}")) {
                Ok(r) => rules.push(Rule::Pattern(r)),
                Err(_) => return Ok(SymlinkRules::default()),
            }
        }
        Ok(SymlinkRules { rules })
    }

    /// `allowed_symlinks.txt` of `game_dir`; absent or invalid: no link allowed.
    pub fn load(game_dir: &Path) -> SymlinkRules {
        let path = game_dir.join("allowed_symlinks.txt");
        let Ok(bytes) = std::fs::read(&path) else { return SymlinkRules::default() };
        match SymlinkRules::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("{} unreadable ({e}): no symbolic link allowed", path.display());
                SymlinkRules::default()
            }
        }
    }

    /// Target allowed?
    pub fn allows(&self, target: &Path) -> bool {
        let text = target_text(target);
        self.rules.iter().any(|r| match r {
            Rule::Prefix(p) => text.starts_with(p.as_str()),
            Rule::Pattern(re) => re.is_match(&text).unwrap_or(false),
        })
    }

    /// Link `link` checked: refusal added to `out`.
    pub fn check_link(&self, link: &Path, out: &mut Vec<Refused>) -> std::io::Result<()> {
        let target = std::fs::read_link(link)?;
        if !self.allows(&target) {
            out.push(Refused { link: link.to_owned(), target });
        }
        Ok(())
    }

    /// The whole folder `dir` walked without following links, each link checked (the folder
    /// itself included). Absent: nothing.
    pub fn check_tree(&self, dir: &Path, out: &mut Vec<Refused>) -> std::io::Result<()> {
        let meta = match std::fs::symlink_metadata(dir) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            return self.check_link(dir, out);
        }
        if !meta.is_dir() {
            return Ok(());
        }
        let mut stack = vec![dir.to_owned()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)? {
                let e = e?;
                let t = e.file_type()?;
                if t.is_symlink() {
                    self.check_link(&e.path(), out)?;
                } else if t.is_dir() {
                    stack.push(e.path());
                }
            }
        }
        Ok(())
    }

    /// Folder of a world to open: the folder itself may be a link (followed without a check),
    /// all its contents are checked. A file instead: error.
    pub fn check_world(&self, dir: &Path) -> std::io::Result<Vec<Refused>> {
        let mut out = Vec::new();
        let meta = match std::fs::symlink_metadata(dir) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        if meta.is_file() {
            return Err(std::io::Error::other(format!("{} is not a folder", dir.display())));
        }
        let root = if meta.file_type().is_symlink() { follow(dir)? } else { dir.to_owned() };
        self.check_tree(&root, &mut out)?;
        Ok(out)
    }

    /// Entry of a pack folder: link checked (refusal in `out`, then `None`), folder
    /// with `pack.mcmeta` none of whose inner links is refused, or file whose name (of the
    /// target for a link) ends in `.zip`. Returns the pack path (the target for a link).
    pub fn pack_entry(&self, content: &Path, out: &mut Vec<Refused>) -> std::io::Result<Option<PathBuf>> {
        let mut path = content.to_owned();
        let mut meta = match std::fs::symlink_metadata(content) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            let before = out.len();
            self.check_link(content, out)?;
            if out.len() > before {
                return Ok(None);
            }
            path = follow(content)?;
            meta = std::fs::symlink_metadata(&path)?;
        }
        if meta.is_dir() {
            let before = out.len();
            self.check_tree(&path, out)?;
            if out.len() > before {
                return Ok(None);
            }
            return Ok(path.join("pack.mcmeta").is_file().then_some(path));
        }
        let zip = meta.is_file() && path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".zip"));
        Ok(zip.then_some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(text: &str, windows: bool) -> SymlinkRules {
        SymlinkRules::parse_for(text, windows).expect("list")
    }

    fn ok(l: &SymlinkRules, s: &str) -> bool {
        let text = s.to_owned();
        l.rules.iter().any(|r| match r {
            Rule::Prefix(p) => text.starts_with(p.as_str()),
            Rule::Pattern(re) => re.is_match(&text).unwrap_or(false),
        })
    }

    #[test]
    fn rules_like_the_game() {
        let l = list("# comment\n\n/data/\n[prefix]/srv/\n[glob]/home/*/worlds/**\n[regex]/opt/[a-z]+\n", false);
        assert!(ok(&l, "/data/x"));
        assert!(ok(&l, "/srv/y"));
        assert!(ok(&l, "/home/neo/worlds/a/b"));
        assert!(!ok(&l, "/home/a/b/worlds/c"), "* does not cross a separator");
        assert!(ok(&l, "/opt/abc"));
        assert!(!ok(&l, "/opt/abc/d"), "whole regex");
        assert!(!ok(&l, "/other"));
        // Groups, classes, ?, escapes.
        let g = list("[glob]/a/{x,y}?.[!0-9]z\n[glob]/b\\*c\n", false);
        assert!(ok(&g, "/a/x1.az") && ok(&g, "/a/yq.bz"));
        assert!(!ok(&g, "/a/x1.5z") && !ok(&g, "/a/w1.az"));
        assert!(ok(&g, "/b*c") && !ok(&g, "/bxc"));
        // Windows: `/` of the pattern = `\`, case ignored.
        let w = list("[glob]C:/Games/**\n[regex]d:\\\\x\n", true);
        assert!(ok(&w, "c:\\games\\a\\b") && ok(&w, "D:\\X"));
        assert!(!ok(&w, "C:/Games/a"));
        // Invalid file: error (no link allowed).
        assert!(SymlinkRules::parse_for("[glob\n", false).is_err());
        assert!(SymlinkRules::parse_for("[pattern]x\n", false).is_err());
        // Pattern that does not compile: silent list.
        assert!(list("/ok\n[glob]/a[b\n", false).rules.is_empty());
        assert!(SymlinkRules::default().rules.is_empty());
    }

    /// Bench (`--ignored --nocapture`): walk of a world of 12,000 files (3 dimensions ×
    /// 4 folders × 1,000), like opening a big world.
    #[test]
    #[ignore]
    fn bench_check_world() {
        let root = std::env::temp_dir().join(format!("rc-symlinks-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dim in ["", "DIM-1", "DIM1"] {
            for sub in ["region", "entities", "poi", "data"] {
                let d = root.join(dim).join(sub);
                std::fs::create_dir_all(&d).expect("folder");
                for i in 0..1000 {
                    std::fs::write(d.join(format!("r.{i}.0.mca")), b"").expect("file");
                }
            }
        }
        let l = SymlinkRules::default();
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            assert!(l.check_world(&root).expect("walk").is_empty());
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!("walk of 12,000 files: {best:.2} ms (best of 5)");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn namespace_prefix_dropped() {
        if cfg!(windows) {
            assert_eq!(target_text(Path::new(r"\\?\C:\a/b")), r"C:\a\b");
            assert_eq!(target_text(Path::new(r"\\?\UNC\srv\p")), r"\\srv\p");
        }
        assert_eq!(target_text(Path::new("rel")), "rel");
    }

    /// Real links (if the system allows creating them): world and packs refused without a list,
    /// allowed with the right rule; a linked zip pack takes the name of its target.
    #[test]
    fn real_links() {
        let root = std::env::temp_dir().join(format!("rc-symlinks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let world = root.join("world");
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(world.join("region")).expect("folder");
        std::fs::create_dir_all(&outside).expect("folder");
        std::fs::write(outside.join("p.zip"), b"x").expect("file");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&outside, world.join("region").join("link")).is_ok()
            && std::os::windows::fs::symlink_file(outside.join("p.zip"), root.join("pack-link")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, world.join("region").join("link")).is_ok()
            && std::os::unix::fs::symlink(outside.join("p.zip"), root.join("pack-link")).is_ok();
        if !made {
            eprintln!("symbolic links not allowed on this system: test ignored");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let none = SymlinkRules::default();
        let found = none.check_world(&world).expect("walk");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].link, world.join("region").join("link"));
        let mut out = Vec::new();
        assert_eq!(none.pack_entry(&root.join("pack-link"), &mut out).expect("pack"), None);
        assert_eq!(out.len(), 1);
        let prefix = target_text(&outside);
        let some = SymlinkRules::parse(&prefix).expect("list");
        assert!(some.check_world(&world).expect("walk").is_empty());
        let mut out = Vec::new();
        assert_eq!(some.pack_entry(&root.join("pack-link"), &mut out).expect("pack"), Some(outside.join("p.zip")));
        assert!(out.is_empty());
        // Ordinary folder with no link: nothing; ordinary non-zip file: not a pack.
        assert!(none.check_world(&outside).expect("walk").is_empty());
        std::fs::write(root.join("note.txt"), b"x").expect("file");
        assert_eq!(none.pack_entry(&root.join("note.txt"), &mut Vec::new()).expect("pack"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
