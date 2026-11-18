//! The user's locale as the game reads it (its runtime's default locale, read once at startup):
//! language of the interface and its country. Windows: the interface language, with the country
//! of the regional format when both share their language; macOS: the first preferred language,
//! given the current region when it names none; elsewhere `LC_ALL`, `LC_MESSAGES` then `LANG`
//! ("C" or "POSIX": English, United States). Uses: Japanese glyph variants by default, country
//! of the periodic regional reminders.

use std::sync::OnceLock;

/// Language (ISO 639, lower case) and country (ISO 3166 alpha-2, upper case; empty if none).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Locale {
    pub language: String,
    pub country: String,
}

pub fn get() -> &'static Locale {
    static L: OnceLock<Locale> = OnceLock::new();
    L.get_or_init(read)
}

/// The interface language is Japanese.
pub fn japanese() -> bool {
    get().language == "ja"
}

/// Country as a three-letter code (`KOR`); `None` without country or for a code outside the table.
pub fn country_iso3() -> Option<&'static str> {
    iso3(&get().country)
}

/// Locale written like a POSIX locale name (`ll_CC.encoding@variant`): language before `_`,
/// country after it, encoding and variant dropped; the short names `ja`, `ko` and their English
/// names stand for Japan and Korea, as in the game's runtime.
#[cfg(any(not(windows), test))]
pub fn parse_posix(name: &str) -> Locale {
    let base = name.split(['.', '@']).next().unwrap_or("");
    let base = match base {
        "ja" | "japanese" => "ja_JP",
        "ko" | "korean" => "ko_KR",
        b => b,
    };
    let (language, country) = base.split_once('_').unwrap_or((base, ""));
    Locale { language: language.to_ascii_lowercase(), country: country.to_ascii_uppercase() }
}

/// POSIX locale of the messages: first non-empty of `LC_ALL`, `LC_MESSAGES`, `LANG`; none, "C"
/// or "POSIX": English (United States).
#[cfg(any(not(windows), test))]
pub fn posix(all: Option<&str>, messages: Option<&str>, lang: Option<&str>) -> Locale {
    let name = [all, messages, lang].into_iter().flatten().find(|v| !v.is_empty()).unwrap_or("C");
    parse_posix(if name == "C" || name == "POSIX" { "en_US" } else { name })
}

/// macOS: first preferred language (`ja`, `en-GB`, `zh-Hans`), completed with the region of the
/// current locale identifier (`en_FR`) when it has none (language alone, or language and script),
/// then read like a POSIX name. `None` without a preferred language.
#[cfg(any(target_os = "macos", test))]
pub fn apple(preferred: Option<&str>, current: Option<&str>) -> Option<Locale> {
    let p = preferred.filter(|p| !p.is_empty())?;
    let parts: Vec<&str> = p.split('-').collect();
    let script_only = parts.len() == 2 && parts[1].len() == 4;
    let region = if parts.len() == 1 || script_only {
        current.and_then(|c| c.rsplit_once('_')).map(|(_, r)| r).unwrap_or("")
    } else {
        // Language, optional script, then region.
        parts.iter().skip(1).find(|s| s.len() != 4).copied().unwrap_or("")
    };
    // No region anywhere: the language alone (Japanese and Korean then get their country).
    let name = if region.is_empty() { parts[0].to_owned() } else { format!("{}_{region}", parts[0]) };
    Some(parse_posix(&name))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn read() -> Locale {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
        fn GetUserDefaultLCID() -> u32;
        fn GetLocaleInfoW(lcid: u32, lctype: u32, data: *mut u16, len: i32) -> i32;
    }
    const LOCALE_SISO639LANGNAME: u32 = 0x59;
    const LOCALE_SISO3166CTRYNAME: u32 = 0x5a;
    // SAFETY: identifiers read without argument.
    let (ui, user) = unsafe { (GetUserDefaultUILanguage(), GetUserDefaultLCID()) };
    let lcid = display_lcid(ui, user);
    let info = |kind: u32| {
        let mut buf = [0u16; 16];
        // SAFETY: 16-character buffer announced as such.
        let n = unsafe { GetLocaleInfoW(lcid, kind, buf.as_mut_ptr(), 16) };
        if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { String::new() }
    };
    Locale { language: info(LOCALE_SISO639LANGNAME).to_ascii_lowercase(), country: info(LOCALE_SISO3166CTRYNAME) }
}

/// Windows: the interface language's locale, or the regional format's when both have the same
/// primary language (the interface list only names languages, not their regions).
#[cfg(any(windows, test))]
fn display_lcid(ui: u16, user: u32) -> u32 {
    if user & 0x3ff == u32::from(ui & 0x3ff) { user } else { u32::from(ui) }
}

#[cfg(target_os = "macos")]
fn read() -> Locale {
    apple(apple_cf::preferred().as_deref(), apple_cf::current().as_deref()).unwrap_or_else(env_locale)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn read() -> Locale {
    env_locale()
}

#[cfg(not(windows))]
fn env_locale() -> Locale {
    posix(rc_data::env_str!("LC_ALL"), rc_data::env_str!("LC_MESSAGES"), rc_data::env_str!("LANG"))
}

/// Core Foundation: preferred languages and current locale identifier.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod apple_cf {
    use std::ffi::{CStr, c_char, c_void};

    type Ref = *const c_void;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> Ref;
        fn CFArrayGetCount(array: Ref) -> isize;
        fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
        fn CFLocaleCopyCurrent() -> Ref;
        fn CFLocaleGetIdentifier(locale: Ref) -> Ref;
        fn CFStringGetCString(s: Ref, buf: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(o: Ref);
    }

    fn string(s: Ref) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 128];
        // SAFETY: string object owned by the caller, 128-byte buffer announced as such.
        if unsafe { CFStringGetCString(s, buf.as_mut_ptr(), 128, UTF8) } == 0 {
            return None;
        }
        // SAFETY: zero-terminated by the call above.
        unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().ok().map(str::to_owned)
    }

    pub fn preferred() -> Option<String> {
        // SAFETY: copied array released after reading its first element.
        unsafe {
            let a = CFLocaleCopyPreferredLanguages();
            if a.is_null() {
                return None;
            }
            let s = if CFArrayGetCount(a) > 0 { string(CFArrayGetValueAtIndex(a, 0)) } else { None };
            CFRelease(a);
            s
        }
    }

    pub fn current() -> Option<String> {
        // SAFETY: copied locale released after reading its identifier (not owned).
        unsafe {
            let l = CFLocaleCopyCurrent();
            if l.is_null() {
                return None;
            }
            let s = string(CFLocaleGetIdentifier(l));
            CFRelease(l);
            s
        }
    }
}

/// ISO 3166 alpha-2 then alpha-3 codes, sorted (regions of the system's culture data).
const ISO3: &str = "ADANDAEAREAFAFGAGATGAIAIAALALBAMARMAOAGOARARGASASMATAUTAUAUSAWABWAXALAAZAZEBABIHBBBRBBDBGDBEBELBFBFABGBGRBHBHRBIBDIBJBENBLBLMBMBMUBNBRNBOBOLBQBESBRBRABSBHSBTBTNBWBWABYBLRBZBLZCACANCCCCKCDCODCFCAFCGCOGCHCHECICIVCKCOKCLCHLCMCMRCNCHNCOCOLCRCRICUCUBCVCPVCWCUWCXCXRCYCYPCZCZEDEDEUDJDJIDKDNKDMDMADODOMDZDZAECECUEEESTEGEGYERERIESESPETETHFIFINFJFJIFKFLKFMFSMFOFROFRFRAGAGABGBGBRGDGRDGEGEOGFGUFGGGGYGHGHAGIGIBGLGRLGMGMBGNGINGPGLPGQGNQGRGRCGTGTMGUGUMGWGNBGYGUYHKHKGHNHNDHRHRVHTHTIHUHUNIDIDNIEIRLILISRIMIMNININDIOIOTIQIRQIRIRNISISLITITAJEJEYJMJAMJOJORJPJPNKEKENKGKGZKHKHMKIKIRKMCOMKNKNAKPPRKKRKORKWKWTKYCYMKZKAZLALAOLBLBNLCLCALILIELKLKALRLBRLSLSOLTLTULULUXLVLVALYLBYMAMARMCMCOMDMDAMEMNEMFMAFMGMDGMHMHLMKMKDMLMLIMMMMRMNMNGMOMACMPMNPMQMTQMRMRTMSMSRMTMLTMUMUSMVMDVMWMWIMXMEXMYMYSMZMOZNANAMNCNCLNENERNFNFKNGNGANINICNLNLDNONORNPNPLNRNRUNUNIUNZNZLOMOMNPAPANPEPERPFPYFPGPNGPHPHLPKPAKPLPOLPMSPMPNPCNPRPRIPSPSEPTPRTPWPLWPYPRYQAQATREREUROROURSSRBRURUSRWRWASASAUSBSLBSCSYCSDSDNSESWESGSGPSHSHNSISVNSJSJMSKSVKSLSLESMSMRSNSENSOSOMSRSURSSSSDSTSTPSVSLVSXSXMSYSYRSZSWZTCTCATDTCDTGTGOTHTHATJTJKTKTKLTLTLSTMTKMTNTUNTOTONTRTURTTTTOTVTUVTWTWNTZTZAUAUKRUGUGAUMUMIUSUSAUYURYUZUZBVAVATVCVCTVEVENVGVGBVIVIRVNVNMVUVUTWFWLFWSWSMYEYEMYTMYTZAZAFZMZMBZWZWE";

/// Three-letter code of a two-letter country (binary search in `ISO3`).
pub fn iso3(alpha2: &str) -> Option<&'static str> {
    if alpha2.len() != 2 {
        return None;
    }
    let n = ISO3.len() / 5;
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let rec = &ISO3[mid * 5..mid * 5 + 5];
        match rec[..2].cmp(alpha2) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => return Some(&rec[2..]),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(language: &str, country: &str) -> Locale {
        Locale { language: language.into(), country: country.into() }
    }

    /// POSIX: `LC_ALL` first, empty values skipped, encoding and variant dropped, "C" is
    /// English (United States), the short Japanese and Korean names have their country.
    #[test]
    fn posix_locale_like_the_runtime() {
        assert_eq!(posix(None, None, Some("ja_JP.UTF-8")), loc("ja", "JP"));
        assert_eq!(posix(Some(""), Some("ko_KR.UTF-8@euro"), Some("en_US.UTF-8")), loc("ko", "KR"));
        assert_eq!(posix(Some("C"), Some("ja_JP.UTF-8"), None), loc("en", "US"), "LC_ALL first");
        assert_eq!(posix(None, None, None), loc("en", "US"), "no locale");
        assert_eq!(posix(None, None, Some("POSIX")), loc("en", "US"));
        assert_eq!(posix(None, None, Some("japanese")), loc("ja", "JP"));
        assert_eq!(posix(None, None, Some("ko")), loc("ko", "KR"));
        assert_eq!(posix(None, None, Some("jam_JM")), loc("jam", "JM"));
        assert_eq!(posix(None, None, Some("fr")), loc("fr", ""));
    }

    /// macOS: the first preferred language decides; language alone or with a script takes the
    /// current region; no preferred language: none (environment then).
    #[test]
    fn apple_preferred_language() {
        assert_eq!(apple(Some("ja-JP"), Some("en_US")), Some(loc("ja", "JP")));
        assert_eq!(apple(Some("ja"), Some("fr_FR")), Some(loc("ja", "FR")), "region of the current locale");
        assert_eq!(apple(Some("zh-Hans"), Some("en_CN")), Some(loc("zh", "CN")));
        assert_eq!(apple(Some("zh-Hant-TW"), Some("en_US")), Some(loc("zh", "TW")));
        assert_eq!(apple(Some("ko"), None), Some(loc("ko", "KR")), "no region at all: short name");
        assert_eq!(apple(Some("de"), None), Some(loc("de", "")));
        assert_eq!(apple(None, Some("ja_JP")), None);
        assert_eq!(apple(Some(""), Some("ja_JP")), None);
    }

    /// Windows: interface locale, or the regional format's when they share their primary language.
    #[test]
    fn windows_display_locale() {
        assert_eq!(display_lcid(0x0409, 0x040c), 0x0409, "English interface, French format: English (US)");
        assert_eq!(display_lcid(0x0409, 0x0809), 0x0809, "English interface, UK format: English (UK)");
        assert_eq!(display_lcid(0x0411, 0x0411), 0x0411);
    }

    /// Two-letter to three-letter countries: the table is sorted (binary search), Korea for the
    /// game's regional reminders.
    #[test]
    fn iso3_table() {
        assert_eq!(ISO3.len() % 5, 0);
        let keys: Vec<&str> = (0..ISO3.len() / 5).map(|i| &ISO3[i * 5..i * 5 + 2]).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicate");
        assert_eq!(iso3("KR"), Some("KOR"));
        assert_eq!(iso3("FR"), Some("FRA"));
        assert_eq!(iso3("AD"), Some("AND"));
        assert_eq!(iso3("ZW"), Some("ZWE"));
        assert_eq!((iso3("XK"), iso3(""), iso3("KOR")), (None, None, None));
    }
}
