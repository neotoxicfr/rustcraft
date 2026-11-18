//! Repository environment variables (`RUSTCRAFT_*`, `RUSTCRAFT_OPT_*` switches), read
//! once per call site.
//!
//! Each call of [`env_os!`](crate::env_os), [`env_str!`](crate::env_str),
//! [`env_flag!`](crate::env_flag) or [`opt_switch!`](crate::opt_switch) has its own static
//! cell: the first call reads the process environment, the following ones return the kept
//! value (one atomic load instead of a system call that locks the environment and copies the
//! value). The cache is exact because nothing in the repository modifies the environment after
//! startup: `set_var` and `remove_var` are forbidden by `clippy.toml` (never reintroduce them,
//! even behind an `allow`).
//!
//! `clippy.toml` also forbids `std::env::var` and `std::env::var_os` everywhere except here: a
//! direct read added on a hot path makes `cargo clippy -D warnings` fail. Only
//! tests, build scripts and tools outside the game keep an explicit `allow`.

use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;

/// Raw read, no cache. Reserved for names computed at run time (read at startup) and for this
/// module; a fixed name goes through the macros.
#[allow(clippy::disallowed_methods)]
pub fn read_os(name: &str) -> Option<OsString> {
    #[cfg(test)]
    tests::count_read();
    std::env::var_os(name)
}

/// Rule shared by every `RUSTCRAFT_OPT_*` switch: `0` turns it off, `1` turns it on, anything
/// else (absent, empty, other text, not Unicode) keeps `default`. The value is compared as is,
/// without trimming: `" 0"` keeps the default.
#[inline]
pub fn switch_state(value: Option<&OsStr>, default: bool) -> bool {
    match value.and_then(OsStr::to_str) {
        Some("0") => false,
        Some("1") => true,
        _ => default,
    }
}

/// Common body of [`env_os!`](crate::env_os). Inlined so that a filled cell costs one atomic
/// load at the call site (the cold first read stays out of line inside `get_or_init`).
#[doc(hidden)]
#[inline]
pub fn site(cell: &'static OnceLock<Option<OsString>>, name: &str) -> Option<&'static OsStr> {
    cell.get_or_init(|| read_os(name)).as_deref()
}

/// Common body of [`env_str!`](crate::env_str): the text is validated once, at the first read,
/// instead of at every call while the variable is set.
#[doc(hidden)]
#[inline]
pub fn text_site(cell: &'static OnceLock<Option<String>>, name: &str) -> Option<&'static str> {
    cell.get_or_init(|| read_os(name).and_then(|v| v.into_string().ok())).as_deref()
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
    ($name:literal) => {{
        static CELL: ::std::sync::OnceLock<::std::option::Option<::std::string::String>> =
            ::std::sync::OnceLock::new();
        $crate::env_flags::text_site(&CELL, $name)
    }};
}

/// Variable present, whatever its value (`std::env::var_os(..).is_some()` kept). Never use it
/// for a 0/1 switch: `NAME=0` counts as present.
#[macro_export]
macro_rules! env_flag {
    ($name:literal) => {
        $crate::env_os!($name).is_some()
    };
}

/// Optimisation switch, read once per call site with the rule of
/// [`switch_state`](crate::env_flags::switch_state):
/// - `opt_switch!("RUSTCRAFT_OPT_X")`: on by default, `RUSTCRAFT_OPT_X=0` restores the old path;
/// - `opt_switch!("RUSTCRAFT_OPT_X", default_off)`: off by default, `RUSTCRAFT_OPT_X=1` turns it on.
///
/// The kept value is the final `bool`: after the first call, a read is one atomic load.
#[macro_export]
macro_rules! opt_switch {
    ($name:literal) => {
        $crate::opt_switch!(@cached $name, true)
    };
    ($name:literal, default_off) => {
        $crate::opt_switch!(@cached $name, false)
    };
    (@cached $name:literal, $default:literal) => {{
        static STATE: ::std::sync::OnceLock<bool> = ::std::sync::OnceLock::new();
        // The environment is read inside the closure: only the first call pays for it.
        *STATE.get_or_init(|| {
            $crate::env_flags::switch_state($crate::env_flags::read_os($name).as_deref(), $default)
        })
    }};
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::ffi::{OsStr, OsString};

    thread_local! {
        /// Raw environment reads made by the current thread (each test has its own thread).
        static READS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn count_read() {
        READS.with(|r| r.set(r.get() + 1));
    }

    fn reads() -> usize {
        READS.with(Cell::get)
    }

    /// One read through a macro site.
    type Read = fn() -> bool;

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
        assert!(crate::opt_switch!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"));
        assert!(!crate::opt_switch!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a", default_off));
    }

    // Each macro site reads the environment at its first call only. Fails if the value is
    // computed outside its cache (the old client helper that took the value as an argument
    // re-read it at every call).
    #[test]
    fn each_site_reads_the_environment_once() {
        const CALLS: usize = 50;
        let cases: [(&str, Read); 5] = [
            ("opt_switch!", || crate::opt_switch!("RUSTCRAFT_ENVTEST_SWITCH_ON_7f3a")),
            ("opt_switch! default_off", || crate::opt_switch!("RUSTCRAFT_ENVTEST_SWITCH_OFF_7f3a", default_off)),
            ("env_os!", || crate::env_os!("PATH").is_some()),
            ("env_str!", || crate::env_str!("PATH").is_some()),
            ("env_flag!", || crate::env_flag!("RUSTCRAFT_ENVTEST_FLAG_7f3a")),
        ];
        for (label, read) in cases {
            let before = reads();
            let first = read();
            for _ in 1..CALLS {
                assert_eq!(read(), first, "{label}: value changed between calls");
            }
            assert_eq!(reads() - before, 1, "{label}: {CALLS} calls must read the environment once");
        }
    }

    /// Values a developer may set, including the awkward ones.
    fn sample_values() -> Vec<Option<OsString>> {
        let mut values: Vec<Option<OsString>> =
            [None, Some(""), Some("0"), Some("1"), Some(" 0"), Some("0 "), Some("00"), Some("true"), Some("off")]
                .into_iter()
                .map(|v| v.map(OsString::from))
                .collect();
        values.push(Some(not_unicode()));
        values
    }

    #[cfg(windows)]
    fn not_unicode() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        // A lone surrogate is a valid Windows environment value but not Unicode text.
        OsString::from_wide(&[0xD800, u16::from(b'0')])
    }

    #[cfg(unix)]
    fn not_unicode() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![0xFF, b'0'])
    }

    // The shared rule gives, for every value, what each reader it replaces gave: the
    // `env_str!(..) != Some("0")` / `== Some("1")` forms of the client and the
    // `0` / `1` / default matches of rc-audio, rc-data `jar` and rc-sim.
    #[test]
    fn switch_state_matches_the_replaced_readers() {
        fn three_state(v: Option<&OsStr>, default: bool) -> bool {
            match v.and_then(OsStr::to_str) {
                Some("0") => false,
                Some("1") => true,
                _ => default,
            }
        }
        for value in sample_values() {
            let v = value.as_deref();
            let text = v.and_then(OsStr::to_str);
            assert_eq!(super::switch_state(v, true), text != Some("0"), "{value:?} default on");
            assert_eq!(super::switch_state(v, false), text == Some("1"), "{value:?} default off");
            for default in [false, true] {
                assert_eq!(super::switch_state(v, default), three_state(v, default), "{value:?} {default}");
            }
        }
    }

    // Each site has its own cell: two different names do not get mixed up.
    #[test]
    fn one_cell_per_site() {
        let a = crate::env_os!("PATH");
        let b = crate::env_os!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a");
        assert!(a.is_some());
        assert!(b.is_none());
    }

    /// Reader in service before this change, kept here for the micro-bench only: out of line,
    /// a second cell for the re-read switch, then the site cell.
    #[inline(never)]
    fn previous_site(
        cell: &'static std::sync::OnceLock<Option<OsString>>,
        name: &str,
    ) -> Option<&'static OsStr> {
        static READ_ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let once = *READ_ONCE.get_or_init(|| super::read_os("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a").is_none());
        let kept = cell.get_or_init(|| super::read_os(name));
        if !once {
            let fresh = super::read_os(name);
            if fresh != *kept {
                return fresh.map(|v| &*Box::leak(v.into_boxed_os_str()));
            }
        }
        kept.as_deref()
    }

    /// Nanoseconds per call of `read`, over `calls` calls.
    fn time_per_call(calls: u32, mut read: impl FnMut() -> bool) -> f64 {
        let t = std::time::Instant::now();
        for _ in 0..calls {
            std::hint::black_box(read());
        }
        t.elapsed().as_nanos() as f64 / f64::from(calls)
    }

    /// Micro-bench (loaded machine): `cargo test --release -p rc-data env_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn env_bench() {
        use std::hint::black_box;
        use std::sync::OnceLock;
        const N: u32 = 10_000_000;
        static ABSENT_OLD: OnceLock<Option<OsString>> = OnceLock::new();
        static PATH_OLD: OnceLock<Option<OsString>> = OnceLock::new();
        static SWITCH_OLD: OnceLock<bool> = OnceLock::new();
        let raw = time_per_call(N / 100, || super::read_os(black_box("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a")).is_some());
        println!("raw var_os, absent key       {raw:8.2} ns/read");
        let rows: [(&str, Read, Read); 3] = [
            (
                "env_flag!, absent key",
                || previous_site(&ABSENT_OLD, "RUSTCRAFT_ENVTEST_NEVER_SET_7f3a").is_some(),
                || crate::env_flag!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"),
            ),
            (
                "env_str!, set key (PATH)",
                || previous_site(&PATH_OLD, "PATH").and_then(OsStr::to_str).is_some(),
                || crate::env_str!("PATH").is_some(),
            ),
            (
                "switch (value before cache)",
                || {
                    let value = previous_site(&ABSENT_OLD, "RUSTCRAFT_ENVTEST_NEVER_SET_7f3a").and_then(OsStr::to_str);
                    *SWITCH_OLD.get_or_init(|| value != Some("0"))
                },
                || crate::opt_switch!("RUSTCRAFT_ENVTEST_NEVER_SET_7f3a"),
            ),
        ];
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        for (label, previous, current) in rows {
            let (old, new) = (time_per_call(N, previous), time_per_call(N, current));
            println!("{label:29} 1 thread: previous {old:6.2} ns, now {new:6.2} ns");
            // All cores at once: the cells are shared read-only, nothing is locked.
            let mut count = 2;
            while count <= threads {
                let wall = |read: Read| {
                    let t = std::time::Instant::now();
                    std::thread::scope(|s| {
                        for _ in 0..count {
                            s.spawn(move || time_per_call(N / 4, read));
                        }
                    });
                    t.elapsed().as_nanos() as f64 / f64::from(N / 4)
                };
                let (old, new) = (wall(previous), wall(current));
                println!("{label:29} {count:2} threads: previous {old:6.2} ns, now {new:6.2} ns (wall time per call)");
                count *= 2;
            }
        }
    }
}
