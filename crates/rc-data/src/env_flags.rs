//! Repository environment variables (`RUSTCRAFT_*`, `RUSTCRAFT_OPT_*` switches), read
//! once per call site.
//!
//! Each call of [`env_os!`](crate::env_os), [`env_str!`](crate::env_str) or
//! [`env_flag!`](crate::env_flag) has its own static cell: the first call reads
//! the process environment, the following ones return the kept value (an atomic read instead
//! of a system call that locks the environment and copies the value). The cache is exact because
//! nothing in the repository modifies the environment after startup: `set_var` and `remove_var`
//! are forbidden by `clippy.toml` (never reintroduce them, even behind an `allow`).
//!
//! `clippy.toml` also forbids `std::env::var` and `std::env::var_os` everywhere except here: a
//! direct read added on a hot path makes `cargo clippy -D warnings` fail. Only
//! tests, build scripts and tools outside the game keep an explicit `allow`.
//!
//! `RUSTCRAFT_OPT_ENVONCE=0` restores the old path (re-read on each call) for an A/B in
//! a single executable; the switch itself is read once.

use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;

/// Raw read, no cache. Reserved for names computed at run time (read at startup) and for this
/// module; a fixed name goes through the macros.
#[allow(clippy::disallowed_methods)]
pub fn read_os(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// True (default): each site reads its environment once. False (`RUSTCRAFT_OPT_ENVONCE=0`):
/// re-read on each call.
pub fn read_once() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| read_os("RUSTCRAFT_OPT_ENVONCE").is_none_or(|v| v != "0"))
}

/// Common body of the macros (one cell per site). In re-read mode, the fresh value is returned
/// if it differs from the kept value (never in practice: the environment does not change); its
/// copy is then left alive until the end of the process to keep a `&'static`.
#[doc(hidden)]
pub fn site(cell: &'static OnceLock<Option<OsString>>, name: &str) -> Option<&'static OsStr> {
    site_mode(cell, name, read_once())
}

fn site_mode(cell: &'static OnceLock<Option<OsString>>, name: &str, once: bool) -> Option<&'static OsStr> {
    let kept = cell.get_or_init(|| read_os(name));
    if !once {
        let fresh = read_os(name);
        if fresh != *kept {
            return fresh.map(|v| &*Box::leak(v.into_boxed_os_str()));
        }
    }
    kept.as_deref()
}

/// Raw value of the variable (`std::env::var_os` kept), `None` if absent.
#[macro_export]
macro_rules! env_os {
    ($name:literal) => {{
        static CELL: ::std::sync::OnceLock<::std::option::Option<::std::ffi::OsString>> =
            ::std::sync::OnceLock::new();
        $crate::env_flags::site(&CELL, $name)
    }};
}

/// Text value of the variable (`std::env::var(..).ok()` kept): `None` if absent or if it
/// is not valid Unicode, like `std::env::var`.
#[macro_export]
macro_rules! env_str {
    ($name:literal) => {
        $crate::env_os!($name).and_then(::std::ffi::OsStr::to_str)
    };
}

/// Variable present, whatever its value (`std::env::var_os(..).is_some()` kept).
#[macro_export]
macro_rules! env_flag {
    ($name:literal) => {
        $crate::env_os!($name).is_some()
    };
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    // `PATH` and `SystemRoot` always exist on Windows (and `PATH` everywhere): we compare with the
    // raw reads instead of modifying the environment (forbidden).
    #[test]
    fn macros_match_raw_reads() {
        let raw = super::read_os("PATH");
        assert!(raw.is_some(), "PATH absent: pointless test");
        for _ in 0..3 {
            assert_eq!(crate::env_os!("PATH"), raw.as_deref());
            assert_eq!(crate::env_str!("PATH"), raw.as_deref().and_then(OsStr::to_str));
            assert!(crate::env_flag!("PATH"));
        }
        let absent = "RUSTCRAFT_ENVTEST_NEVER_SET_7f3a";
        assert!(super::read_os(absent).is_none());
        assert_eq!(crate::env_os!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"), None);
        assert_eq!(crate::env_str!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"), None);
        assert!(!crate::env_flag!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"));
    }

    // Old path (`RUSTCRAFT_OPT_ENVONCE=0`): same value as the raw read, and a cell
    // already filled with a stale value is corrected by the re-read.
    #[test]
    fn reread_mode_matches_raw() {
        use std::ffi::OsString;
        use std::sync::OnceLock;
        static FRESH: OnceLock<Option<OsString>> = OnceLock::new();
        static STALE: OnceLock<Option<OsString>> = OnceLock::new();
        let raw = super::read_os("PATH");
        for _ in 0..3 {
            assert_eq!(super::site_mode(&FRESH, "PATH", false), raw.as_deref());
        }
        STALE.set(Some("stale".into())).unwrap();
        assert_eq!(super::site_mode(&STALE, "PATH", false), raw.as_deref());
        assert_eq!(super::site_mode(&STALE, "PATH", true), Some(OsStr::new("stale")));
    }

    // Each site has its own cell: two different names do not get mixed up.
    #[test]
    fn one_cell_per_site() {
        let a = crate::env_os!("PATH");
        let b = crate::env_os!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a");
        assert!(a.is_some());
        assert!(b.is_none());
    }

    /// Micro-bench (loaded machine): `cargo test --release -p rc-data env_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn env_bench() {
        use std::hint::black_box;
        use std::time::Instant;
        const N: u32 = 1_000_000;
        let mut cases = vec![("absent", "RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"), ("PATH", "PATH")];
        if super::read_os("RUSTCRAFT_TICKLOG").is_some() {
            cases.push(("TICKLOG", "RUSTCRAFT_TICKLOG"));
        }
        for (label, name) in cases {
            let t = Instant::now();
            for _ in 0..N {
                black_box(super::read_os(black_box(name)).is_some());
            }
            let raw = t.elapsed().as_nanos() as f64 / f64::from(N);
            let t = Instant::now();
            for _ in 0..N {
                black_box(crate::env_flag!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"));
            }
            let cached = t.elapsed().as_nanos() as f64 / f64::from(N);
            println!("{label:8} var_os {raw:7.1} ns/read; kept macro {cached:5.2} ns/read");
        }
        // All cores: getenv takes an environment lock, the macro does not.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        for (label, cached) in [("var_os", false), ("macro", true)] {
            let t = Instant::now();
            std::thread::scope(|s| {
                for _ in 0..threads {
                    s.spawn(move || {
                        for _ in 0..N / 4 {
                            if cached {
                                black_box(crate::env_flag!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"));
                            } else {
                                black_box(super::read_os(black_box("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a")).is_some());
                            }
                        }
                    });
                }
            });
            let per = t.elapsed().as_nanos() as f64 / f64::from(N / 4);
            println!("{threads} threads, {label:6}: {per:7.1} ns per read and per thread (wall time)");
        }
    }
}
