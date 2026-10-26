// Direct reads allowed (build script: cargo passes its settings through the environment); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Compiler version for the `Rust:` line of the debug screen (`debug-f3.md` § 3.5).

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
}
