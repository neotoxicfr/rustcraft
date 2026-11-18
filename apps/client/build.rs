// Direct reads allowed (build script: cargo passes its settings through the environment); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Compiler version for the `Rust:` line of the debug screen (`debug-f3.md` § 3.5), and the
//! indices of the mimalloc options the client sets, read from the header of the linked library.

use std::path::PathBuf;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().nth(1).map(str::to_owned))
        .unwrap_or_else(|| "?".into());
    println!("cargo:rustc-env=RC_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");
    write_mimalloc_options();
}

/// Options the client sets with `mi_option_set` (`debug_sys.rs`), by their name in the header.
const OPTIONS: [(&str, &str); 2] =
    [("PAGE_COMMIT_ON_DEMAND", "mi_option_page_commit_on_demand"), ("PURGE_DELAY", "mi_option_purge_delay")];

/// Writes `mimalloc_options.rs` in `OUT_DIR`: the library version and the index of each option
/// of `OPTIONS` (`None` when this version has no such option, e.g. on-demand commit under v2).
/// The index is the position in the option enumeration: a bump that inserts an option moves the
/// following ones, and a hard-coded index would then silently set another option.
fn write_mimalloc_options() {
    // Exported by libmimalloc-sys (`links = "mimalloc"`) to the crates that depend on it directly.
    let include = std::env::var("DEP_MIMALLOC_INCLUDE_DIR")
        .expect("libmimalloc-sys must be a direct dependency: its include directory gives the option indices");
    let header_path = PathBuf::from(include).join("mimalloc.h");
    println!("cargo:rerun-if-changed={}", header_path.display());
    let header = std::fs::read_to_string(&header_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", header_path.display()));
    let version = library_version(&header).expect("MI_MALLOC_VERSION not found in mimalloc.h");
    let names = option_names(&header).expect("option enumeration of mimalloc.h not understood");
    let mut out = format!(
        "/// Version of the linked mimalloc (`MI_MALLOC_VERSION`: major, 2 digits minor, 2 digits patch).\n\
         pub const LIBRARY_VERSION: i32 = {version};\n"
    );
    for (constant, name) in OPTIONS {
        let index = names.iter().position(|n| n == name);
        let value = index.map_or_else(|| "None".to_owned(), |i| format!("Some({i})"));
        out += &format!("/// Index of `{name}` in the linked header.\npub const {constant}: Option<i32> = {value};\n");
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    std::fs::write(out_dir.join("mimalloc_options.rs"), out).expect("cannot write mimalloc_options.rs");
}

/// Value of `#define MI_MALLOC_VERSION <n>`.
fn library_version(header: &str) -> Option<i32> {
    header.lines().find_map(|l| l.trim().strip_prefix("#define MI_MALLOC_VERSION")?.split_whitespace().next()?.parse().ok())
}

/// Names of the option enumeration, in order, up to the end marker `_mi_option_last`. Fails if
/// a member before the marker has an explicit value: positions would no longer be indices.
fn option_names(header: &str) -> Option<Vec<String>> {
    let body = header.split_once("typedef enum mi_option_e {")?.1.split_once("_mi_option_last")?.0;
    let mut names = Vec::new();
    for line in body.lines() {
        let code = line.split_once("//").map_or(line, |(c, _)| c).trim();
        for member in code.split(',').map(str::trim).filter(|m| !m.is_empty()) {
            if member.contains('=') || !member.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            names.push(member.to_owned());
        }
    }
    Some(names)
}
