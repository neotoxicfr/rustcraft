//! Disconnect report on an internal error (configuration or play): file
//! `<game folder>/debug/disconnect-<date>_<time>-client.txt`, whose folder is offered by
//! the "Open report folder" button of the disconnect screen, and bug report
//! link supplied by the server (`server_links`, type 0), offered by "Report to server".

use std::path::{Path, PathBuf};

use rc_client::extra::ServerLink;

/// Bug report link of the server (known type 0), if it gave one.
pub fn bug_link(links: &[ServerLink]) -> Option<String> {
    links.iter().find(|l| l.kind == Some(0)).map(|l| l.url.clone())
}

/// File name: `disconnect-yyyy-MM-dd_HH.mm.ss-client.txt`.
pub fn file_name(stamp: (u16, u16, u16, u16, u16, u16)) -> String {
    let (y, mo, d, h, mi, s) = stamp;
    format!("disconnect-{y:04}-{mo:02}-{d:02}_{h:02}.{mi:02}.{s:02}-client.txt")
}

/// Text of the report.
pub fn body(
    stamp: (u16, u16, u16, u16, u16, u16),
    exception: &str,
    bug_link: Option<&str>,
    details: &[(String, String)],
) -> String {
    let (y, mo, d, h, mi, s) = stamp;
    let mut out = String::from("---- Rustcraft network protocol error report ----\n");
    if let Some(l) = bug_link {
        out.push_str(&format!("// Server bug reporting link: {l}\n"));
    }
    out.push_str(&format!("Time: {y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}\n"));
    out.push_str("Description: Packet handling error\n\n");
    out.push_str(&format!("Internal Exception: {exception}\n"));
    if !details.is_empty() {
        out.push_str("\n-- Server details --\n");
        for (k, v) in details {
            out.push_str(&format!("{k}: {v}\n"));
        }
    }
    out
}

/// Writes the report into `<game folder>/debug`; path of the file if the write succeeded.
pub fn store(
    game_dir: &Path,
    stamp: (u16, u16, u16, u16, u16, u16),
    exception: &str,
    bug_link: Option<&str>,
    details: &[(String, String)],
) -> Option<PathBuf> {
    let dir = game_dir.join("debug");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(file_name(stamp));
    std::fs::write(&path, body(stamp, exception, bug_link, details)).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_follows_the_game() {
        assert_eq!(file_name((2026, 10, 5, 9, 4, 7)), "disconnect-2026-10-05_09.04.07-client.txt");
    }

    #[test]
    fn only_the_bug_report_link_counts() {
        let link = |kind, url: &str| ServerLink { kind, label: None, url: url.into() };
        let links = [link(Some(2), "https://a"), link(None, "https://b"), link(Some(0), "https://c")];
        assert_eq!(bug_link(&links).as_deref(), Some("https://c"));
        assert_eq!(bug_link(&links[..2]), None);
    }

    #[test]
    fn report_is_written_under_debug_and_carries_the_link() {
        let dir = std::env::temp_dir().join(format!("rc-disconnect-report-{}", std::process::id()));
        let stamp = (2026, 1, 2, 3, 4, 5);
        let p = store(&dir, stamp, "java.io.IOException: x", Some("https://bug"), &[("k".into(), "v".into())]).unwrap();
        assert_eq!(p.parent().unwrap(), dir.join("debug"));
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("Server bug reporting link: https://bug"));
        assert!(text.contains("Internal Exception: java.io.IOException: x"));
        assert!(text.contains("k: v"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
