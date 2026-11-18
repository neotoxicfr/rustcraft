//! Visual order of bidirectional text and contextual forms of Arabic letters (`text.md`
//! § 5.1).
//!
//! Like the official client, for each drawn line (§ codes already applied):
//! 1. Arabic letters take their presentation form (isolated, initial, medial, final)
//!    according to their neighbours, and lam followed by alef becomes a single ligature;
//! 2. the Unicode bidirectional algorithm splits the line into direction runs; the direction
//!    of the paragraph comes from its first strong letter, otherwise from the language (right to left for
//!    a "bidirectional" language);
//! 3. the runs are laid out in visual order, right-to-left ones read backwards with
//!    each character replaced by its mirror (parentheses, brackets…).
//!
//! The style of an output character is that of the INPUT character at the same UTF-16 rank: the
//! lam-alef ligature shortens the text without shortening the list of styles, which shifts the
//! styles that follow it. Quirk of game 26.3, reproduced.

use std::ops::Range;

use unicode_bidi::{BidiClass, Direction, Level};

use crate::StyledChar;

/// Visual order of a line: (drawn character, rank in `chars` of the character that gives it
/// its style). `None`: the line is drawn as is (common case, checked without allocation).
pub fn visual(chars: &[StyledChar], rtl: bool) -> Option<Vec<(char, u32)>> {
    if unchanged(chars, rtl) {
        return None;
    }
    let mut text: Vec<u16> = Vec::with_capacity(chars.len() + 4);
    let mut owner: Vec<u32> = Vec::with_capacity(chars.len() + 4);
    let mut buf = [0u16; 2];
    for (k, s) in chars.iter().enumerate() {
        for &u in s.c.encode_utf16(&mut buf).iter() {
            text.push(u);
            owner.push(k as u32);
        }
    }
    let shaped = shape(&text);
    let mut out = Vec::with_capacity(shaped.len());
    let mut start = 0;
    while start < shaped.len() {
        let end = shaped[start..]
            .iter()
            .position(|&u| u16_class(u) == BidiClass::B)
            .map_or(shaped.len(), |p| start + p + 1);
        reorder_paragraph(&shaped[start..end], &owner[start..end], rtl, &mut out);
        start = end;
    }
    Some(out)
}

/// True if neither the shaping nor the visual order changes anything: no character from
/// Hebrew onward (U+0590, first right-to-left block; bidirectional controls and
/// Arabic letters are beyond it), and a left-to-right paragraph (left-to-right language,
/// or first strong letter left-to-right: there is no other strong letter below U+0590).
pub fn unchanged(chars: &[StyledChar], rtl: bool) -> bool {
    let mut ltr = !rtl;
    for s in chars {
        let c = s.c;
        if c >= '\u{0590}' {
            return false;
        }
        if !ltr && (c.is_ascii_alphabetic() || (!c.is_ascii() && unicode_bidi::bidi_class(c) == BidiClass::L)) {
            ltr = true;
        }
    }
    ltr
}

fn u16_class(u: u16) -> BidiClass {
    char::from_u32(u32::from(u)).map_or(BidiClass::L, unicode_bidi::bidi_class)
}

/// A paragraph (final separator included) laid out in visual order.
fn reorder_paragraph(para: &[u16], owner: &[u32], rtl: bool, out: &mut Vec<(char, u32)>) {
    let level = match unicode_bidi::get_base_direction(para) {
        Direction::Ltr => Level::ltr(),
        Direction::Rtl => Level::rtl(),
        Direction::Mixed if rtl => Level::rtl(),
        Direction::Mixed => Level::ltr(),
    };
    let info = unicode_bidi::utf16::ParagraphBidiInfo::new(para, Some(level));
    let (levels, runs) = info.visual_runs(0..para.len());
    for run in runs {
        if levels[run.start].is_rtl() {
            backward(para, owner, run, out);
        } else {
            forward(para, owner, run, out);
        }
    }
}

fn forward(text: &[u16], owner: &[u32], r: Range<usize>, out: &mut Vec<(char, u32)>) {
    let mut i = r.start;
    while i < r.end {
        let (c, n) = match (text[i], text.get(i + 1).filter(|_| i + 1 < r.end)) {
            (h @ 0xD800..=0xDBFF, Some(&l @ 0xDC00..=0xDFFF)) => (pair(h, l), 2),
            (u, _) => (char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}'), 1),
        };
        out.push((c, owner[i]));
        i += n;
    }
}

/// Right-to-left run: characters read backwards, each replaced by its mirror.
fn backward(text: &[u16], owner: &[u32], r: Range<usize>, out: &mut Vec<(char, u32)>) {
    let mut k = r.end;
    while k > r.start {
        let (c, at) = match (text[k - 1], k >= r.start + 2) {
            (l @ 0xDC00..=0xDFFF, true) if (0xD800..=0xDBFF).contains(&text[k - 2]) => (pair(text[k - 2], l), k - 2),
            (u, _) => (char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}'), k - 1),
        };
        out.push((unicode_bidi_mirroring::get_mirrored(c).unwrap_or(c), owner[at]));
        k = at;
    }
}

fn pair(h: u16, l: u16) -> char {
    char::from_u32(0x10000 + ((u32::from(h) - 0xD800) << 10) + (u32::from(l) - 0xDC00)).unwrap_or('\u{FFFD}')
}

// ---------------------------------------------------------------------------------------------
// Arabic contextual forms, as the client gets them from its Unicode library with
// the sole option "shape the letters" (text in logical order, variable length:
// lam-alef ligatures shorten the text, diacritics stay).
//
// Tables (Unicode data): for each character, joining bits (1: joins to the right,
// 2: to the left, 4: transparent, 16: lam, 32: alef) and, in the high byte, the offset of its
// first presentation form from U+FE70.

const JOIN_06: [u16; 178] = [
    0x1121, 0x1321, 0x1501, 0x1721, 0x1903, 0x1D21, 0x1F03, 0x2301, 0x2503, 0x2903, //
    0x2D03, 0x3103, 0x3503, 0x3901, 0x3B01, 0x3D01, 0x3F01, 0x4103, 0x4503, 0x4903, //
    0x4D03, 0x5103, 0x5503, 0x5903, 0x5D03, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0003, 0x6103, 0x6503, 0x6903, 0x6D13, 0x7103, 0x7503, 0x7903, 0x7D01, 0x7F01, //
    0x8103, 0x0004, 0x0004, 0x0004, 0x0004, 0x0004, 0x0004, 0x0004, 0x0004, 0x0004, //
    0x0004, 0x0004, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x8501, 0x8701, //
    0x8901, 0x8B01, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, //
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0004, 0x0000, //
    0x0021, 0x0021, 0x0000, 0x0021, 0x0001, 0x0001, 0x0003, 0x0003, 0x0003, 0x0003, //
    0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, //
    0x0003, 0x0003, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, //
    0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, //
    0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, //
    0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, //
    0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, //
    0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0001, 0x0003, //
    0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, 0x0001, //
    0x0003, 0x0001, 0x0003, 0x0003, 0x0003, 0x0003, 0x0001, 0x0001,
];

const JOIN_FE: [u16; 141] = [
    0x0003, 0x0003, 0x0003, 0x0000, 0x0003, 0x0000, 0x0003, 0x0003, 0x0003, 0x0003, //
    0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0003, 0x0000, 0x0020, 0x0021, 0x0020, //
    0x0021, 0x0000, 0x0001, 0x0020, 0x0021, 0x0000, 0x0002, 0x0003, 0x0001, 0x0020, //
    0x0021, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0001, 0x0000, 0x0002, 0x0003, //
    0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, //
    0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0001, 0x0000, //
    0x0001, 0x0000, 0x0001, 0x0000, 0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, //
    0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, //
    0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, //
    0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, //
    0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0010, //
    0x0012, 0x0013, 0x0011, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0002, 0x0003, //
    0x0001, 0x0000, 0x0002, 0x0003, 0x0001, 0x0000, 0x0001, 0x0000, 0x0001, 0x0000, //
    0x0002, 0x0003, 0x0001, 0x0000, 0x0001, 0x0000, 0x0001, 0x0000, 0x0001, 0x0000, //
    0x0001,
];

/// Base letter of each presentation form U+FE70..=U+FEFC (lam-alef ligatures brought back
/// to the intermediate codes U+065C..=U+065F).
const BASE_OF_FE: [u16; 141] = [
    0x064B, 0x064B, 0x064C, 0x064C, 0x064D, 0x064D, 0x064E, 0x064E, 0x064F, 0x064F, //
    0x0650, 0x0650, 0x0651, 0x0651, 0x0652, 0x0652, 0x0621, 0x0622, 0x0622, 0x0623, //
    0x0623, 0x0624, 0x0624, 0x0625, 0x0625, 0x0626, 0x0626, 0x0626, 0x0626, 0x0627, //
    0x0627, 0x0628, 0x0628, 0x0628, 0x0628, 0x0629, 0x0629, 0x062A, 0x062A, 0x062A, //
    0x062A, 0x062B, 0x062B, 0x062B, 0x062B, 0x062C, 0x062C, 0x062C, 0x062C, 0x062D, //
    0x062D, 0x062D, 0x062D, 0x062E, 0x062E, 0x062E, 0x062E, 0x062F, 0x062F, 0x0630, //
    0x0630, 0x0631, 0x0631, 0x0632, 0x0632, 0x0633, 0x0633, 0x0633, 0x0633, 0x0634, //
    0x0634, 0x0634, 0x0634, 0x0635, 0x0635, 0x0635, 0x0635, 0x0636, 0x0636, 0x0636, //
    0x0636, 0x0637, 0x0637, 0x0637, 0x0637, 0x0638, 0x0638, 0x0638, 0x0638, 0x0639, //
    0x0639, 0x0639, 0x0639, 0x063A, 0x063A, 0x063A, 0x063A, 0x0641, 0x0641, 0x0641, //
    0x0641, 0x0642, 0x0642, 0x0642, 0x0642, 0x0643, 0x0643, 0x0643, 0x0643, 0x0644, //
    0x0644, 0x0644, 0x0644, 0x0645, 0x0645, 0x0645, 0x0645, 0x0646, 0x0646, 0x0646, //
    0x0646, 0x0647, 0x0647, 0x0647, 0x0647, 0x0648, 0x0648, 0x0649, 0x0649, 0x064A, //
    0x064A, 0x064A, 0x064A, 0x065C, 0x065C, 0x065D, 0x065D, 0x065E, 0x065E, 0x065F, //
    0x065F,
];

/// Rank of the form to take according to [joining of the next][of the previous][of the character]
/// (two bits each): 0 isolated, 1 final, 2 initial, 3 medial.
const FORM: [[[u16; 4]; 4]; 4] = [
    [[0, 0, 0, 0], [0, 0, 0, 0], [0, 1, 0, 3], [0, 1, 0, 1]],
    [[0, 0, 2, 2], [0, 0, 1, 2], [0, 1, 1, 2], [0, 1, 1, 3]],
    [[0, 0, 0, 0], [0, 0, 0, 0], [0, 1, 0, 3], [0, 1, 0, 3]],
    [[0, 0, 1, 2], [0, 0, 1, 2], [0, 1, 1, 2], [0, 1, 1, 3]],
];

const TRANSPARENT: u16 = 4;
const LAM: u16 = 16;
const ALEF: u16 = 32;
/// Temporary mark of the alef absorbed by a ligature (removed at the end).
const ABSORBED: u16 = 0xFFFF;

fn link(u: u16) -> u16 {
    match u {
        0x0622..=0x06D3 => JOIN_06[usize::from(u - 0x0622)],
        0x200D => 3,
        0x206D..=0x206F => TRANSPARENT,
        0xFE70..=0xFEFC => JOIN_FE[usize::from(u - 0xFE70)],
        _ => 0,
    }
}

fn is_diacritic(u: u16) -> bool {
    (0x064B..=0x0652).contains(&u)
}

/// Intermediate code of the lam + this alef ligature (0: not an alef that joins the lam).
fn ligature(alef: u16) -> u16 {
    match alef {
        0x0622 => 0x065C,
        0x0623 => 0x065D,
        0x0625 => 0x065E,
        0x0627 => 0x065F,
        _ => 0,
    }
}

/// Letters that have only two forms (isolated, final): 1; diacritics: 2; others: 0.
fn kind(u: u16) -> u8 {
    match u {
        0x0622..=0x0625 | 0x0627 | 0x0629 | 0x062F..=0x0632 | 0x0648..=0x0649 => 1,
        0x064B..=0x0652 => 2,
        _ => 0,
    }
}

/// UTF-16 text (logical order) with its Arabic letters in presentation form.
pub fn shape(text: &[u16]) -> Vec<u16> {
    if !text.iter().any(|&u| (0x0622..=0x06D3).contains(&u) || (0xFE70..=0xFEFC).contains(&u)) {
        return text.to_vec();
    }
    // The walk is done on a reversed copy (from the end of the array to the start, hence in
    // logical order), forms already present brought back to their base letter.
    let mut d: Vec<u16> = text
        .iter()
        .rev()
        .map(|&u| if (0xFE70..=0xFEFC).contains(&u) { BASE_OF_FE[usize::from(u - 0xFE70)] } else { u })
        .collect();
    let at = |i: isize| i as usize;
    let mut i = d.len() as isize - 1;
    let mut curr = link(d[at(i)]);
    let (mut next, mut prev, mut last) = (0u16, 0u16, 0u16);
    let mut last_pos = i;
    let mut nx: isize = -2;
    let mut ligatured = false;
    while i >= 0 {
        if curr & 0xFF00 != 0 || is_diacritic(d[at(i)]) {
            // Next non-transparent character (toward the front of the text).
            let mut nw = i - 1;
            nx = -2;
            while nx < 0 {
                if nw == -1 {
                    next = 0;
                    nx = isize::MAX;
                } else {
                    next = link(d[at(nw)]);
                    if next & TRANSPARENT == 0 {
                        nx = nw;
                    } else {
                        nw -= 1;
                    }
                }
            }
            if curr & ALEF != 0 && last & LAM != 0 {
                ligatured = true;
                let lig = ligature(d[at(i)]);
                if lig != 0 {
                    d[at(i)] = ABSORBED;
                    d[at(last_pos)] = lig;
                    i = last_pos;
                }
                last = prev;
                curr = link(lig);
            }
            let u = d[at(i)];
            let k = kind(u);
            let mut form = FORM[usize::from(next & 3)][usize::from(last & 3)][usize::from(curr & 3)];
            if k == 1 {
                form &= 1;
            } else if k == 2 {
                let joined = last & 2 != 0
                    && next & 1 != 0
                    && u != 0x064C
                    && u != 0x064D
                    && !(next & ALEF != 0 && last & LAM != 0);
                form = u16::from(joined);
            }
            d[at(i)] = if k == 2 { 0xFE70 + 2 * (u - 0x064B) + form } else { 0xFE70 + (curr >> 8) + form };
        }
        if curr & TRANSPARENT == 0 {
            prev = last;
            last = curr;
            last_pos = i;
        }
        i -= 1;
        if i == nx {
            curr = next;
            nx = -2;
        } else if i != -1 {
            curr = link(d[at(i)]);
        }
    }
    if ligatured {
        // The game then removes ALL U+FFFF marks, including those of the original text.
        d.retain(|&u| u != ABSORBED);
    }
    d.reverse();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CharStyle;

    fn line(text: &str, styles: &str) -> Vec<StyledChar> {
        // One style per UTF-16 unit in the oracle: we take that of the first unit.
        let units: Vec<u32> = styles.chars().map(|d| d.to_digit(10).unwrap_or(0)).collect();
        let mut out = Vec::new();
        let mut pos = 0;
        for c in text.chars() {
            let st = units.get(pos).copied().unwrap_or(0);
            out.push(StyledChar { c, style: CharStyle { color: Some(st), ..CharStyle::default() } });
            pos += c.len_utf16();
        }
        out
    }

    /// Results of the official client's Unicode library (same calls as it), recorded
    /// by a throwaway program outside the repository: visual text, then style of each character.
    const CASES: [(&str, &str, bool, &str, &str); 16] = [
        ("abc def", "0001111", false, "abc def", "0001111"),
        ("שלום עולם", "000001111", false, "\u{5dd}\u{5dc}\u{5d5}\u{5e2} \u{5dd}\u{5d5}\u{5dc}\u{5e9}", "111100000"),
        ("Hello שלום (world) 123", "", false, "Hello \u{5dd}\u{5d5}\u{5dc}\u{5e9} (world) 123", ""),
        ("Hello שלום (world) 123", "", true, "Hello \u{5dd}\u{5d5}\u{5dc}\u{5e9} (world) 123", ""),
        (
            "\u{627}\u{644}\u{633}\u{644}\u{627}\u{645} \u{639}\u{644}\u{64a}\u{643}\u{645}",
            "000000011111",
            true,
            "\u{fee2}\u{fedc}\u{fef4}\u{fee0}\u{fecb} \u{fee1}\u{fefc}\u{feb4}\u{fedf}\u{fe8d}",
            "11110000000",
        ),
        ("12 / 34", "", true, "34 / 12", ""),
        ("[שלום] <a>", "", true, "<a> [\u{5dd}\u{5d5}\u{5dc}\u{5e9}]", ""),
        (
            "\u{641}\u{627}\u{631}\u{633}\u{6cc} \u{6f1}\u{6f2}\u{6f3}",
            "",
            true,
            "\u{6f1}\u{6f2}\u{6f3} \u{6cc}\u{feb3}\u{fead}\u{fe8e}\u{fed3}",
            "",
        ),
        ("-5", "", true, "5-", ""),
        (
            "\u{628}\u{650}\u{633}\u{652}\u{645}\u{650} \u{644}\u{627} \u{644}\u{623}",
            "",
            false,
            "\u{fef7} \u{fefb} \u{fe7a}\u{fee2}\u{fe7f}\u{feb4}\u{fe7b}\u{fe91}",
            "",
        ),
        ("x \u{1f600} אב", "", false, "x \u{1f600} \u{5d1}\u{5d0}", ""),
        ("\u{644}\u{627}\u{644}\u{627}\u{644}\u{627}", "012345", true, "\u{fefb}\u{fefb}\u{fefb}", "210"),
        ("abc", "", true, "abc", ""),
        ("123", "", true, "123", ""),
        ("\u{647}\u{644} (\u{646}\u{639}\u{645})\u{61f}", "", true, "\u{61f}(\u{fee2}\u{fecc}\u{fee7}) \u{fede}\u{feeb}", ""),
        ("\u{fefb}\u{fe8d}\u{640}\u{628}", "", true, "\u{fe90}\u{640}\u{fe8d}\u{fefb}", ""),
    ];

    #[test]
    fn matches_the_official_unicode_library() {
        for (text, styles, rtl, want, want_styles) in CASES {
            let chars = line(text, styles);
            let got: Vec<(char, u32)> = visual(&chars, rtl)
                .unwrap_or_else(|| chars.iter().enumerate().map(|(k, s)| (s.c, k as u32)).collect());
            let shown: String = got.iter().map(|g| g.0).collect();
            assert_eq!(shown, want, "visual text of {text:?}");
            let st: String = got.iter().map(|&(_, k)| char::from(b'0' + chars[k as usize].style.color.unwrap_or(0) as u8)).collect();
            let want_st = if want_styles.is_empty() { "0".repeat(got.len()) } else { want_styles.to_owned() };
            assert_eq!(st, want_st, "styles of {text:?}");
        }
    }

    #[test]
    fn latin_text_is_left_untouched() {
        assert!(visual(&line("Hello, world (42)!", ""), false).is_none());
        assert!(visual(&line("Hello, world (42)!", ""), true).is_none());
        assert!(visual(&line("éàü ß", ""), true).is_none());
        // Without a strong letter, a right-to-left language goes through the full algorithm.
        assert!(visual(&line("12 / 34", ""), true).is_some());
    }

    /// Micro-bench (`cargo test --release -p rc-text bench_visual -- --ignored --nocapture`):
    /// cost per line of the fast path and the full path.
    #[test]
    #[ignore]
    fn bench_visual() {
        let lines = [
            ("latin 60", line(&"Hello world, this is a chat line! 1234 ".repeat(2)[..60], "")),
            ("arabic 60", line(&"\u{627}\u{644}\u{633}\u{644}\u{627}\u{645} \u{639}\u{644}\u{64a}\u{643}\u{645} ".repeat(6), "")),
            ("hebrew 60", line(&"שלום עולם (abc) ".repeat(4), "")),
        ];
        for rtl in [false, true] {
            for (name, l) in &lines {
                let n = 20_000;
                let t = std::time::Instant::now();
                let mut k = 0usize;
                for _ in 0..n {
                    k += visual(std::hint::black_box(l), rtl).map_or(0, |v| v.len());
                }
                let ns = t.elapsed().as_nanos() as f64 / f64::from(n);
                println!("{name} rtl={rtl}: {ns:.0} ns/line ({k})");
                // Cost of a hit in the interface cache: text fingerprint then comparison.
                let map: hashbrown::HashMap<u64, Box<[char]>> = hashbrown::HashMap::new();
                let kept: Box<[char]> = l.iter().map(|s| s.c).collect();
                let t = std::time::Instant::now();
                let mut hits = 0usize;
                for _ in 0..n {
                    use std::hash::{BuildHasher, Hasher};
                    let mut h = map.hasher().build_hasher();
                    for s in std::hint::black_box(l) {
                        h.write_u32(s.c as u32);
                    }
                    h.write_u8(u8::from(rtl));
                    let key = h.finish();
                    hits += usize::from(key != 0 && kept.iter().zip(l.iter()).all(|(a, s)| *a == s.c));
                }
                let ns = t.elapsed().as_nanos() as f64 / f64::from(n);
                println!("  cache {name} rtl={rtl}: {ns:.0} ns/line ({hits})");
            }
        }
    }

    #[test]
    fn absorbed_alef_shortens_and_stray_marks_go() {
        // U+FFFF already present: removed only when a ligature was made.
        assert_eq!(shape(&[0xFFFF, 0x0644, 0x0627]), vec![0xFEFB]);
        assert_eq!(shape(&[0xFFFF, 0x0628]), vec![0xFFFF, 0xFE8F]);
    }
}
