//! Paletted container in memory: same packing as the network (values aligned in
//! 64-bit words, without overlap), so reception is almost a copy.
//!
//! Fast paths behind switches (see [`opt`]); the old paths remain
//! callable (`*_reference`, `*_with`) for equivalence tests and benches.

use std::cell::RefCell;
use std::ops::Range;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::SECTION_BLOCKS;

/// Values of a container: either a single value, or packed indices, with or
/// without a palette (without a palette = direct global identifiers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Container {
    Single(u32),
    Packed { bits: u8, palette: Option<Box<[u32]>>, data: Box<[u64]> },
}

#[inline]
pub const fn values_per_long(bits: u8) -> usize {
    64 / bits as usize
}

/// Number of words for `entries` values of `bits` bits.
#[inline]
pub const fn longs_for(entries: usize, bits: u8) -> usize {
    entries.div_ceil(values_per_long(bits))
}

/// Optimisation switches of the container, read once (at the first call) from
/// the environment: absent = default value, `=0` turns off, `=1` forces.
pub mod opt {
    use super::{AtomicU32, Ordering};

    /// `pack`: reverse table value → index per thread instead of linear search.
    pub const PACK_TABLE: u32 = 1 << 0;
    /// `pack`: "same value as the previous one" shortcut (long runs).
    pub const PACK_RUN: u32 = 1 << 1;
    /// `get`: division replaced by the multiplier table (C2-M09 called it GETMUL).
    pub const GET_MAGIC: u32 = 1 << 2;
    /// `get` (if GET_MAGIC is off): width dispatched to a constant version.
    pub const CONTAINERDIV: u32 = 1 << 3;
    /// `unpack_into`: a single pass, constant version per width.
    pub const UNPACK_MONO: u32 = 1 << 4;
    /// `set` / repack: palette growth in one allocation, repack buffer on the stack.
    pub const SET_CAP: u32 = 1 << 5;
    /// repack: widen the raw indices without unfolding (non-canonical in-memory palette).
    pub const REPACK_WIDEN: u32 = 1 << 6;

    pub(super) const SWITCHES: [(&str, u32, bool); 7] = [
        ("RUSTCRAFT_OPT_PACK_TABLE", PACK_TABLE, true),
        ("RUSTCRAFT_OPT_PACK_RUN", PACK_RUN, true),
        ("RUSTCRAFT_OPT_GET_MAGIC", GET_MAGIC, true),
        ("RUSTCRAFT_OPT_CONTAINERDIV", CONTAINERDIV, true),
        ("RUSTCRAFT_OPT_UNPACK_MONO", UNPACK_MONO, true),
        ("RUSTCRAFT_OPT_SET_CAP", SET_CAP, true),
        // Off by default: the palette keeps its stale entries up to 8 bits, which weakens
        // the palette prefilters (light, POI, portals); to be settled on the quiet scene bench.
        ("RUSTCRAFT_OPT_REPACK_WIDEN", REPACK_WIDEN, false),
    ];
    const UNSET: u32 = u32::MAX;
    static FLAGS: AtomicU32 = AtomicU32::new(UNSET);

    /// Active flags (bits above).
    #[inline]
    pub fn flags() -> u32 {
        let f = FLAGS.load(Ordering::Relaxed);
        if f == UNSET { init() } else { f }
    }

    // Direct read allowed: rc-world depends on no package (no rc_data::env_*!), and the
    // value is kept in `FLAGS` from the first call (read once, like the macros).
    #[allow(clippy::disallowed_methods)]
    #[cold]
    #[inline(never)]
    fn init() -> u32 {
        let mut f = 0;
        for (name, bit, default) in SWITCHES {
            let on = match std::env::var(name).as_deref() {
                Ok("0") => false,
                Ok("1") => true,
                _ => default,
            };
            if on {
                f |= bit;
            }
        }
        FLAGS.store(f, Ordering::Relaxed);
        f
    }
}

/// Read path of `get` (benches and tests).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetMode {
    Reference,
    Magic,
    Mono,
}

/// Division by `vpl` without a division instruction: `index / vpl = (index * mul) >> 24`,
/// exact for `index < 4096` (error `index / 2^24 < 1 / vpl` with `mul` rounded up).
#[derive(Clone, Copy)]
struct Magic {
    vpl: u32,
    mul: u32,
    mask: u64,
}

const MAGIC_SHIFT: u32 = 24;

const fn magic_table(round_up: bool) -> [Magic; 64] {
    let mut t = [Magic { vpl: 1, mul: 1 << MAGIC_SHIFT, mask: 0 }; 64];
    let mut b = 1;
    while b <= 32 {
        let vpl = 64 / b;
        let mut mul = (1u32 << MAGIC_SHIFT) / vpl;
        if round_up && !(1u32 << MAGIC_SHIFT).is_multiple_of(vpl) {
            mul += 1;
        }
        t[b as usize] = Magic { vpl, mul, mask: (1u64 << b) - 1 };
        b += 1;
    }
    t
}

static MAGIC: [Magic; 64] = magic_table(true);

#[inline(always)]
fn raw_reference(data: &[u64], bits: u8, index: usize) -> u32 {
    let vpl = values_per_long(bits);
    ((data[index / vpl] >> ((index % vpl) * bits as usize)) & ((1u64 << bits) - 1)) as u32
}

#[inline(always)]
fn raw_magic_in(table: &[Magic; 64], data: &[u64], bits: u8, index: usize) -> u32 {
    let m = table[(bits & 63) as usize];
    debug_assert!(index < SECTION_BLOCKS, "read outside section: {index}");
    let q = (index * m.mul as usize) >> MAGIC_SHIFT;
    let r = index - q * m.vpl as usize;
    ((data[q] >> (r * bits as usize)) & m.mask) as u32
}

#[inline(always)]
fn raw_const<const B: usize>(data: &[u64], index: usize) -> u32 {
    let vpl = 64 / B;
    ((data[index / vpl] >> ((index % vpl) * B)) & ((1u64 << B) - 1)) as u32
}

macro_rules! by_width {
    ($bits:expr, $f:ident, $args:tt, $fallback:expr, [$($b:literal)*]) => {
        match $bits {
            $($b => $f::<$b> $args,)*
            _ => $fallback,
        }
    };
}

#[inline(always)]
fn raw_mono(data: &[u64], bits: u8, index: usize) -> u32 {
    by_width!(bits, raw_const, (data, index), raw_reference(data, bits, index),
        [1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32])
}

#[inline(always)]
fn map_palette(palette: &Option<Box<[u32]>>, raw: u32) -> u32 {
    match palette {
        // An index outside the palette (corrupt data) gives state 0 rather than a panic.
        Some(p) => p.get(raw as usize).copied().unwrap_or(0),
        None => raw,
    }
}

thread_local! {
    /// Reverse table of `pack`: `value → index + 1` (0 = absent), reset to zero after each
    /// call on the entries set only.
    static PACK_INVERSE: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
}

/// Largest reverse table (values beyond: linear search in the palette).
const INVERSE_MAX_BITS: u8 = 16;

impl Container {
    /// Builds a packed container, checking the consistency of sizes.
    pub fn packed(entries: usize, bits: u8, palette: Option<Box<[u32]>>, data: Box<[u64]>) -> Option<Self> {
        (bits > 0 && bits <= 32 && data.len() == longs_for(entries, bits)).then_some(Container::Packed {
            bits,
            palette,
            data,
        })
    }

    #[inline]
    pub fn get(&self, index: usize) -> u32 {
        match self {
            Container::Single(v) => *v,
            Container::Packed { bits, palette, data } => {
                let f = opt::flags();
                let raw = if f & opt::GET_MAGIC != 0 {
                    raw_magic_in(&MAGIC, data, *bits, index)
                } else if f & opt::CONTAINERDIV != 0 {
                    raw_mono(data, *bits, index)
                } else {
                    raw_reference(data, *bits, index)
                };
                map_palette(palette, raw)
            }
        }
    }

    /// `get` by a given path, without reading the switches (tests, benches).
    #[doc(hidden)]
    #[inline]
    pub fn get_with(&self, index: usize, mode: GetMode) -> u32 {
        match self {
            Container::Single(v) => *v,
            Container::Packed { bits, palette, data } => {
                let raw = match mode {
                    GetMode::Reference => raw_reference(data, *bits, index),
                    GetMode::Magic => raw_magic_in(&MAGIC, data, *bits, index),
                    GetMode::Mono => raw_mono(data, *bits, index),
                };
                map_palette(palette, raw)
            }
        }
    }

    /// Values of a vertical stack `(x, z)` of a 16³ section, from `ys.start` to `ys.end - 1`:
    /// a single division per stack, then advance of 256 entries by carry. Same sequence as
    /// `get(block_index(x, y, z))`.
    pub fn column_iter(&self, x: usize, z: usize, ys: Range<usize>) -> ColumnIter<'_> {
        debug_assert!(x < 16 && z < 16 && ys.end <= 16, "stack outside section");
        let left = ys.end.saturating_sub(ys.start);
        match self {
            Container::Single(v) => ColumnIter { data: &[], palette: None, single: *v, bits: 0, vpl: 1, word: 0, slot: 0, dw: 0, ds: 0, left },
            Container::Packed { bits, palette, data } => {
                let vpl = values_per_long(*bits);
                let first = (ys.start << 8) | (z << 4) | x;
                ColumnIter {
                    data,
                    palette: palette.as_deref(),
                    single: 0,
                    bits: *bits,
                    vpl,
                    word: first / vpl,
                    slot: first % vpl,
                    dw: 256 / vpl,
                    ds: 256 % vpl,
                    left,
                }
            }
        }
    }

    /// Decompresses all values into `out` (size = number of entries). Fast path
    /// for meshing and lighting: one pass per word, no division per value.
    pub fn unpack_into(&self, out: &mut [u32]) {
        if opt::flags() & opt::UNPACK_MONO != 0 { self.unpack_mono(out) } else { self.unpack_reference(out) }
    }

    /// Old decompression in two passes (indices, then palette).
    #[doc(hidden)]
    pub fn unpack_reference(&self, out: &mut [u32]) {
        match self {
            Container::Single(v) => out.fill(*v),
            Container::Packed { bits, palette, data } => {
                let bits = *bits as usize;
                let vpl = 64 / bits;
                let mask = (1u64 << bits) - 1;
                for (word, chunk) in data.iter().zip(out.chunks_mut(vpl)) {
                    let mut w = *word;
                    for slot in chunk {
                        *slot = (w & mask) as u32;
                        w >>= bits;
                    }
                }
                if let Some(p) = palette {
                    for v in out.iter_mut() {
                        *v = p.get(*v as usize).copied().unwrap_or(0);
                    }
                }
            }
        }
    }

    /// Decompression in one pass, width as a constant (inner loop unrolled), palette
    /// applied on the fly by a zero-padded table (outside the palette → 0).
    #[doc(hidden)]
    pub fn unpack_mono(&self, out: &mut [u32]) {
        let Container::Packed { bits, palette, data } = self else {
            return self.unpack_reference(out);
        };
        let covered = match palette {
            Some(p) if *bits <= 8 => {
                let mut pad = [0u32; 256];
                let n = p.len().min(256);
                pad[..n].copy_from_slice(&p[..n]);
                by_width!(*bits, unpack_words, (data, out, &pad), unreachable!(), [1 2 3 4 5 6 7 8])
            }
            None => by_width!(*bits, unpack_words_direct, (data, out), return self.unpack_reference(out),
                [1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16]),
            Some(_) => return self.unpack_reference(out),
        };
        // Like the old path: cells beyond the words keep their content, palette applied.
        if covered < out.len()
            && let Some(p) = palette
        {
            for v in &mut out[covered..] {
                *v = p.get(*v as usize).copied().unwrap_or(0);
            }
        }
    }

    /// Repacks values with the smallest representation: single value, palette
    /// of width `min_palette_bits..=max_palette_bits`, otherwise direct on `direct_bits`.
    /// Palette in order of first appearance (save and network bit for bit).
    pub fn pack(values: &[u32], min_palette_bits: u8, max_palette_bits: u8, direct_bits: u8) -> Self {
        let f = opt::flags();
        Self::pack_with(
            values,
            min_palette_bits,
            max_palette_bits,
            direct_bits,
            f & opt::PACK_TABLE != 0,
            f & opt::PACK_RUN != 0,
        )
    }

    /// Old `pack` (linear search in the palette per value): reference.
    #[doc(hidden)]
    pub fn pack_reference(values: &[u32], min_palette_bits: u8, max_palette_bits: u8, direct_bits: u8) -> Self {
        let mut palette: Vec<u32> = Vec::new();
        let limit = 1usize << max_palette_bits;
        let mut overflow = false;
        for &v in values {
            if !palette.contains(&v) {
                if palette.len() == limit {
                    overflow = true;
                    break;
                }
                palette.push(v);
            }
        }
        if !overflow && palette.len() == 1 {
            return Container::Single(palette[0]);
        }
        let (bits, palette) = if overflow {
            (direct_bits, None)
        } else {
            let need = (usize::BITS - (palette.len() - 1).leading_zeros()) as u8;
            (need.max(min_palette_bits), Some(palette))
        };
        let vpl = values_per_long(bits);
        let mut data = vec![0u64; longs_for(values.len(), bits)];
        for (i, &v) in values.iter().enumerate() {
            let raw = match &palette {
                Some(p) => p.iter().position(|&x| x == v).unwrap_or(0) as u64,
                None => u64::from(v),
            };
            data[i / vpl] |= raw << ((i % vpl) * bits as usize);
        }
        Container::Packed { bits, palette: palette.map(Vec::into_boxed_slice), data: data.into_boxed_slice() }
    }

    /// `pack` with the chosen approaches: `table` (reverse table per thread), `run` (run
    /// shortcut). A single pass writes the raw indices, then packing word by word.
    #[doc(hidden)]
    pub fn pack_with(
        values: &[u32],
        min_palette_bits: u8,
        max_palette_bits: u8,
        direct_bits: u8,
        table: bool,
        run: bool,
    ) -> Self {
        // Outside the sections case (≤ 4096 entries, palette ≤ 8 bits): old path.
        if !(table || run)
            || values.is_empty()
            || values.len() > SECTION_BLOCKS
            || max_palette_bits > 8
            || direct_bits == 0
            || direct_bits > 32
        {
            return Self::pack_reference(values, min_palette_bits, max_palette_bits, direct_bits);
        }
        let limit = 1usize << max_palette_bits;
        let mut palette = [0u32; 256];
        let mut n = 0usize;
        let mut idx = [0u8; SECTION_BLOCKS];
        let overflow = if table {
            PACK_INVERSE.with_borrow_mut(|t| {
                let want = 1usize << direct_bits.min(INVERSE_MAX_BITS);
                if t.len() < want {
                    t.resize(want, 0);
                }
                let o = first_seen(values, limit, run, &mut palette, &mut n, &mut idx, Some(t));
                for &v in &palette[..n] {
                    if let Some(e) = t.get_mut(v as usize) {
                        *e = 0;
                    }
                }
                o
            })
        } else {
            first_seen(values, limit, run, &mut palette, &mut n, &mut idx, None)
        };
        if !overflow && n == 1 {
            return Container::Single(palette[0]);
        }
        let bits = if overflow {
            direct_bits
        } else {
            ((usize::BITS - (n - 1).leading_zeros()) as u8).max(min_palette_bits)
        };
        let vpl = values_per_long(bits);
        let b = bits as usize;
        let mut data = vec![0u64; longs_for(values.len(), bits)].into_boxed_slice();
        if overflow {
            for (w, chunk) in data.iter_mut().zip(values.chunks(vpl)) {
                let mut acc = 0u64;
                for (j, &v) in chunk.iter().enumerate() {
                    acc |= u64::from(v) << (j * b);
                }
                *w = acc;
            }
            Container::Packed { bits, palette: None, data }
        } else {
            for (w, chunk) in data.iter_mut().zip(idx[..values.len()].chunks(vpl)) {
                let mut acc = 0u64;
                for (j, &r) in chunk.iter().enumerate() {
                    acc |= u64::from(r) << (j * b);
                }
                *w = acc;
            }
            Container::Packed { bits, palette: Some(palette[..n].into()), data }
        }
    }

    /// Number of entries (among `entries`) whose value satisfies `pred`, without unfolding: histogram
    /// of the indices, then `pred` once per palette entry (several entries may answer
    /// yes; index outside the palette = value 0, like `get`).
    pub fn count_where(&self, entries: usize, pred: impl Fn(u32) -> bool) -> usize {
        match self {
            Container::Single(v) => {
                if pred(*v) {
                    entries
                } else {
                    0
                }
            }
            // No palette entry answers yes (nor the value 0 of out-of-palette indices):
            // zero without reading the data (common case: section without water).
            Container::Packed { palette: Some(p), .. } if !pred(0) && !p.iter().any(|&v| pred(v)) => 0,
            Container::Packed { bits, palette: Some(p), data } if *bits <= 8 => {
                // Four histograms in alternation: a run of equal indices no longer depends
                // on the previous increment.
                let mut hist = [[0u32; 256]; 4];
                by_width!(*bits, hist_words, (data, entries, &mut hist), unreachable!(), [1 2 3 4 5 6 7 8]);
                (0..1usize << *bits)
                    .map(|i| (i, hist[0][i] + hist[1][i] + hist[2][i] + hist[3][i]))
                    .filter(|&(i, h)| h != 0 && pred(p.get(i).copied().unwrap_or(0)))
                    .map(|(_, h)| h as usize)
                    .sum()
            }
            c if entries <= SECTION_BLOCKS => {
                let mut v = [0u32; SECTION_BLOCKS];
                c.unpack_into(&mut v[..entries]);
                v[..entries].iter().filter(|&&s| pred(s)).count()
            }
            c => (0..entries).filter(|&i| pred(c.get(i))).count(),
        }
    }

    /// Canonical palette (order of first appearance, no duplicate nor unused entry) of the
    /// first `entries` values, as [`Container::pack`] would compute it on the unfolded
    /// values: `palette[..n]` receives the values, `idx[..entries]` the indices into this
    /// palette, and the function returns `n`. `None` beyond `limit` distinct values (≤ 256).
    /// In-memory palette ≤ 8 bits: work on the raw indices (one table per raw index, one
    /// lookup per palette entry met), without unfolding. Otherwise: unfolding then a single
    /// pass of `pack` (reverse table for values < 2^`value_bits`, at most 2^16).
    /// Index outside the palette = value 0, like `get`.
    pub fn first_seen_indices(
        &self,
        entries: usize,
        limit: usize,
        value_bits: u8,
        palette: &mut [u32; 256],
        idx: &mut [u8; SECTION_BLOCKS],
    ) -> Option<usize> {
        assert!(entries > 0 && entries <= SECTION_BLOCKS && (1..=256).contains(&limit));
        match self {
            Container::Single(v) => {
                palette[0] = *v;
                idx[..entries].fill(0);
                Some(1)
            }
            Container::Packed { bits, palette: Some(p), data } if *bits <= 8 && data.len() * values_per_long(*bits) >= entries => {
                let b = *bits;
                let mask = (1u64 << b) - 1;
                for (word, chunk) in data.iter().zip(idx[..entries].chunks_mut(values_per_long(b))) {
                    let mut w = *word;
                    for s in chunk {
                        *s = (w & mask) as u8;
                        w >>= b;
                    }
                }
                // Raw index → canonical index + 1 (0 = not yet seen).
                let mut map = [0u16; 256];
                let mut n = 0usize;
                let mut prev = u16::MAX;
                let mut prev_m = 0u8;
                for s in &mut idx[..entries] {
                    let r = *s;
                    if u16::from(r) == prev {
                        *s = prev_m;
                        continue;
                    }
                    let m = match map[r as usize] {
                        0 => {
                            let v = p.get(r as usize).copied().unwrap_or(0);
                            let m = match palette[..n].iter().position(|&x| x == v) {
                                Some(k) => k,
                                None => {
                                    if n == limit {
                                        return None;
                                    }
                                    palette[n] = v;
                                    n += 1;
                                    n - 1
                                }
                            } as u8;
                            map[r as usize] = u16::from(m) + 1;
                            m
                        }
                        e => (e - 1) as u8,
                    };
                    *s = m;
                    prev = u16::from(r);
                    prev_m = m;
                }
                Some(n)
            }
            c => {
                let mut values = [0u32; SECTION_BLOCKS];
                c.unpack_into(&mut values[..entries]);
                let mut n = 0usize;
                let overflow = PACK_INVERSE.with_borrow_mut(|t| {
                    let want = 1usize << value_bits.min(INVERSE_MAX_BITS);
                    if t.len() < want {
                        t.resize(want, 0);
                    }
                    let o = first_seen(&values[..entries], limit, true, palette, &mut n, idx, Some(t));
                    for &v in &palette[..n] {
                        if let Some(e) = t.get_mut(v as usize) {
                            *e = 0;
                        }
                    }
                    o
                });
                (!overflow).then_some(n)
            }
        }
    }

    /// Writes a value in place, like the game: index already in the palette → only its bits;
    /// palette not full → entry added; otherwise repacking (wider palette, or direct).
    /// The palette may keep unused entries; `get` always returns the right value.
    pub fn set(&mut self, index: usize, value: u32, min_palette_bits: u8, max_palette_bits: u8, direct_bits: u8) {
        let f = opt::flags();
        self.set_with(
            index,
            value,
            min_palette_bits,
            max_palette_bits,
            direct_bits,
            f & opt::SET_CAP != 0,
            f & opt::REPACK_WIDEN != 0,
        );
    }

    /// `set` with the chosen approaches (tests, benches).
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn set_with(
        &mut self,
        index: usize,
        value: u32,
        min_palette_bits: u8,
        max_palette_bits: u8,
        direct_bits: u8,
        cap: bool,
        widen: bool,
    ) {
        let write = |data: &mut [u64], bits: u8, raw: u64| {
            let vpl = values_per_long(bits);
            let shift = (index % vpl) * bits as usize;
            let mask = ((1u64 << bits) - 1) << shift;
            data[index / vpl] = (data[index / vpl] & !mask) | (raw << shift);
        };
        match self {
            Container::Single(v) if *v == value => {}
            Container::Packed { bits, palette: None, data } if u64::from(value) < 1u64 << *bits => {
                write(data, *bits, u64::from(value));
            }
            Container::Packed { bits, palette: Some(p), data } => {
                if let Some(raw) = p.iter().position(|&x| x == value) {
                    write(data, *bits, raw as u64);
                } else if p.len() < 1usize << *bits {
                    let raw = p.len();
                    if cap {
                        // A single allocation at the exact size (instead of a doubling then a
                        // shrinking).
                        let mut grown = Vec::with_capacity(raw + 1);
                        grown.extend_from_slice(p);
                        grown.push(value);
                        *p = grown.into_boxed_slice();
                    } else {
                        let mut grown = std::mem::take(p).into_vec();
                        grown.push(value);
                        *p = grown.into_boxed_slice();
                    }
                    write(data, *bits, raw as u64);
                } else if widen && *bits < max_palette_bits {
                    self.widen(index, value, min_palette_bits);
                } else {
                    self.repack(index, value, min_palette_bits, max_palette_bits, direct_bits, cap);
                }
            }
            _ => self.repack(index, value, min_palette_bits, max_palette_bits, direct_bits, cap),
        }
    }

    fn repack(
        &mut self,
        index: usize,
        value: u32,
        min_palette_bits: u8,
        max_palette_bits: u8,
        direct_bits: u8,
        cap: bool,
    ) {
        if cap {
            let mut values = [0u32; SECTION_BLOCKS];
            self.unpack_into(&mut values);
            values[index] = value;
            *self = Container::pack(&values, min_palette_bits, max_palette_bits, direct_bits);
        } else {
            let mut values = vec![0u32; SECTION_BLOCKS];
            self.unpack_into(&mut values);
            values[index] = value;
            *self = Container::pack(&values, min_palette_bits, max_palette_bits, direct_bits);
        }
    }

    /// Palette full under the maximum width: raw indices copied one bit wider, value
    /// added at the end of the palette (non-canonical in-memory palette: unused entries kept).
    fn widen(&mut self, index: usize, value: u32, min_palette_bits: u8) {
        let Container::Packed { bits, palette: Some(p), data } = self else { unreachable!() };
        let old = *bits;
        let slots = data.len() * values_per_long(old);
        // Number of entries deduced from the words: section (4096) or biomes (64).
        let entries = if slots >= SECTION_BLOCKS { SECTION_BLOCKS } else { crate::SECTION_BIOMES.min(slots) };
        let new = (old + 1).max(min_palette_bits);
        // Raw indices by whole words (width < 8: full palette ≤ 128 entries).
        let mut raw = [0u8; SECTION_BLOCKS];
        let omask = (1u64 << old) - 1;
        for (word, chunk) in data.iter().zip(raw[..entries].chunks_mut(values_per_long(old))) {
            let mut w = *word;
            for s in chunk {
                *s = (w & omask) as u8;
                w >>= old;
            }
        }
        raw[index] = p.len() as u8;
        let mut out = vec![0u64; longs_for(entries, new)].into_boxed_slice();
        for (w, chunk) in out.iter_mut().zip(raw[..entries].chunks(values_per_long(new))) {
            let mut acc = 0u64;
            for (j, &r) in chunk.iter().enumerate() {
                acc |= u64::from(r) << (j * new as usize);
            }
            *w = acc;
        }
        let mut grown = Vec::with_capacity(p.len() + 1);
        grown.extend_from_slice(p);
        grown.push(value);
        *self = Container::Packed { bits: new, palette: Some(grown.into_boxed_slice()), data: out };
    }

    /// Bytes occupied in memory (enum header excluded).
    pub fn heap_bytes(&self) -> usize {
        match self {
            Container::Single(_) => 0,
            Container::Packed { palette, data, .. } => palette.as_ref().map_or(0, |p| p.len() * 4) + data.len() * 8,
        }
    }
}

/// Single pass of `pack`: palette in order of first appearance, raw indices in
/// `idx`; `true` if the palette overflows `limit` (same stopping point as the old code).
/// `table`: reverse table `value → index + 1` (values outside the table: linear search).
#[inline(always)]
fn first_seen(
    values: &[u32],
    limit: usize,
    run: bool,
    palette: &mut [u32; 256],
    n: &mut usize,
    idx: &mut [u8; SECTION_BLOCKS],
    mut table: Option<&mut Vec<u16>>,
) -> bool {
    let mut prev = !values[0];
    let mut prev_i = 0u8;
    for (k, &v) in values.iter().enumerate() {
        if run && v == prev {
            idx[k] = prev_i;
            continue;
        }
        let found = match table.as_deref().and_then(|t| t.get(v as usize)) {
            Some(&e) => e.checked_sub(1).map(|e| e as u8),
            None => palette[..*n].iter().position(|&x| x == v).map(|p| p as u8),
        };
        let i = match found {
            Some(i) => i,
            None => {
                if *n == limit {
                    return true;
                }
                let i = *n as u8;
                palette[*n] = v;
                *n += 1;
                if let Some(e) = table.as_deref_mut().and_then(|t| t.get_mut(v as usize)) {
                    *e = u16::from(i) + 1;
                }
                i
            }
        };
        idx[k] = i;
        prev = v;
        prev_i = i;
    }
    false
}

#[inline(always)]
fn unpack_words<const B: usize>(data: &[u64], out: &mut [u32], pad: &[u32; 256]) -> usize {
    let vpl = 64 / B;
    let mask = (1u64 << B) - 1;
    let full = data.len().min(out.len() / vpl);
    for (word, chunk) in data[..full].iter().zip(out.chunks_exact_mut(vpl)) {
        let mut w = *word;
        for slot in chunk {
            *slot = pad[(w & mask) as usize];
            w >>= B;
        }
    }
    let mut covered = full * vpl;
    if let Some(&word) = data.get(full) {
        let mut w = word;
        for slot in &mut out[covered..] {
            *slot = pad[(w & mask) as usize];
            w >>= B;
            covered += 1;
        }
    }
    covered
}

#[inline(always)]
fn hist_words<const B: usize>(data: &[u64], entries: usize, hist: &mut [[u32; 256]; 4]) {
    let vpl = 64 / B;
    let mask = (1u64 << B) - 1;
    let full = data.len().min(entries / vpl);
    for &word in &data[..full] {
        let mut w = word;
        for j in 0..vpl {
            hist[j & 3][(w & mask) as usize] += 1;
            w >>= B;
        }
    }
    if let Some(&word) = data.get(full) {
        let mut w = word;
        for j in 0..(entries - full * vpl).min(vpl) {
            hist[j & 3][(w & mask) as usize] += 1;
            w >>= B;
        }
    }
}

#[inline(always)]
fn unpack_words_direct<const B: usize>(data: &[u64], out: &mut [u32]) -> usize {
    let vpl = 64 / B;
    let mask = (1u64 << B) - 1;
    let full = data.len().min(out.len() / vpl);
    for (word, chunk) in data[..full].iter().zip(out.chunks_exact_mut(vpl)) {
        let mut w = *word;
        for slot in chunk {
            *slot = (w & mask) as u32;
            w >>= B;
        }
    }
    let mut covered = full * vpl;
    if let Some(&word) = data.get(full) {
        let mut w = word;
        for slot in &mut out[covered..] {
            *slot = (w & mask) as u32;
            w >>= B;
            covered += 1;
        }
    }
    covered
}

/// Sequence of values of a vertical stack (see [`Container::column_iter`]).
pub struct ColumnIter<'a> {
    data: &'a [u64],
    palette: Option<&'a [u32]>,
    single: u32,
    bits: u8,
    vpl: usize,
    word: usize,
    slot: usize,
    dw: usize,
    ds: usize,
    left: usize,
}

impl Iterator for ColumnIter<'_> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        if self.bits == 0 {
            return Some(self.single);
        }
        let raw = ((self.data[self.word] >> (self.slot * self.bits as usize)) & ((1u64 << self.bits) - 1)) as u32;
        self.word += self.dw;
        self.slot += self.ds;
        if self.slot >= self.vpl {
            self.slot -= self.vpl;
            self.word += 1;
        }
        Some(match self.palette {
            Some(p) => p.get(raw as usize).copied().unwrap_or(0),
            None => raw,
        })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl ExactSizeIterator for ColumnIter<'_> {}

/// Canonical palette (order of first appearance, deduplicated values, unused entries
/// removed) of raw indices `indices` into `palette`: rewrites the indices in place and returns the
/// new palette. Index outside the palette = value 0, like `get`. Same palette and same indices
/// as `Container::pack` on the unfolded values (except width overflow).
pub fn remap_first_seen(indices: &mut [u32], palette: &[u32]) -> Vec<u32> {
    // Last cell: all indices outside the palette.
    let mut map = vec![u32::MAX; palette.len() + 1];
    let mut out: Vec<u32> = Vec::new();
    for i in indices.iter_mut() {
        let slot = (*i as usize).min(palette.len());
        let mut m = map[slot];
        if m == u32::MAX {
            let v = palette.get(slot).copied().unwrap_or(0);
            m = match out.iter().position(|&x| x == v) {
                Some(p) => p as u32,
                None => {
                    out.push(v);
                    out.len() as u32 - 1
                }
            };
            map[slot] = m;
        }
        *i = m;
    }
    out
}

/// True if `(indices, palette)` is already canonical (what [`remap_first_seen`] would return): each
/// index is an already seen index or the next one, all entries are used, values distinct.
pub fn is_canonical(indices: &[u32], palette: &[u32]) -> bool {
    let mut next = 0u32;
    for &i in indices {
        if i == next {
            next += 1;
        } else if i > next {
            return false;
        }
    }
    if next as usize != palette.len() {
        return false;
    }
    // Distinct values (short palette: pairwise comparison).
    palette.iter().enumerate().all(|(k, v)| !palette[..k].contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widening stays off by default until the scene bench has settled it.
    #[test]
    fn repack_widen_is_off_by_default() {
        let (_, _, on) = opt::SWITCHES.iter().find(|(_, b, _)| *b == opt::REPACK_WIDEN).copied().unwrap();
        assert!(!on);
    }

    #[test]
    fn get_matches_unpack_and_pack_round_trips() {
        let values: Vec<u32> = (0..4096u32).map(|i| (i * 7919) % 37).collect();
        let c = Container::pack(&values, 4, 8, 16);
        assert!(matches!(c, Container::Packed { bits: 6, palette: Some(_), .. }));
        let mut out = vec![0; 4096];
        c.unpack_into(&mut out);
        assert_eq!(out, values);
        for (i, &v) in values.iter().enumerate() {
            assert_eq!(c.get(i), v);
        }
    }

    #[test]
    fn representations() {
        assert_eq!(Container::pack(&[5; 4096], 4, 8, 16), Container::Single(5));
        let two: Vec<u32> = (0..4096).map(|i| i % 2).collect();
        assert!(matches!(Container::pack(&two, 4, 8, 16), Container::Packed { bits: 4, .. }));
        let many: Vec<u32> = (0..4096).collect();
        let direct = Container::pack(&many, 4, 8, 16);
        assert!(matches!(direct, Container::Packed { bits: 16, palette: None, .. }));
        assert_eq!(direct.get(4095), 4095);
        // 64 biome entries, minimum width 1.
        let b: Vec<u32> = (0..64).map(|i| i % 3).collect();
        let c = Container::pack(&b, 1, 3, 7);
        assert!(matches!(c, Container::Packed { bits: 2, .. }));
        assert_eq!(c.get(5), 2);
    }

    #[test]
    fn widths_not_dividing_64_leave_padding() {
        // 5 bits: 12 values per word, 4 padding bits.
        assert_eq!(values_per_long(5), 12);
        assert_eq!(longs_for(4096, 5), 342);
        assert_eq!(longs_for(4096, 16), 1024);
        assert!(Container::packed(4096, 5, None, vec![0; 341].into()).is_none());
    }

    /// Test xorshift generator.
    fn rng(seed: u64) -> impl FnMut(u64) -> u64 {
        let mut s = seed | 1;
        move |n| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % n
        }
    }

    /// Value sets: runs (stone/air), disorder, 256/257 distinct (overflow), biomes,
    /// large identifiers (outside the reverse table).
    fn value_sets() -> Vec<(Vec<u32>, u8, u8, u8)> {
        let mut r = rng(0x9e37_79b9_7f4a_7c15);
        let mut sets = Vec::new();
        for distinct in [1u32, 2, 3, 12, 16, 17, 50, 255, 256, 257, 300, 5000] {
            sets.push(((0..4096u32).map(|i| (i * 7919) % distinct).collect(), 4, 8, 15));
            sets.push(((0..4096u32).map(|_| r(u64::from(distinct)) as u32 * 37).collect(), 4, 8, 15));
            // Long runs: layers of 256 cells.
            sets.push(((0..4096u32).map(|i| ((i >> 8) * 13 + (i & 1) * (i >> 11)) % distinct).collect(), 4, 8, 15));
        }
        // Values beyond the reverse table (2^16) and beyond the direct width.
        sets.push(((0..4096u32).map(|i| 70_000 + (i % 5)).collect(), 4, 8, 15));
        sets.push(((0..4096u32).map(|i| if i % 3 == 0 { 1 << 20 } else { i % 7 }).collect(), 4, 8, 15));
        for distinct in [1u32, 2, 5, 8, 9, 64] {
            sets.push(((0..64u32).map(|_| r(u64::from(distinct)) as u32).collect(), 1, 3, 7));
        }
        sets.push((vec![3, 4, 3], 4, 8, 15));
        sets
    }

    #[test]
    fn pack_variants_match_reference_bit_for_bit() {
        for (values, lo, hi, direct) in value_sets() {
            let reference = Container::pack_reference(&values, lo, hi, direct);
            for (table, run) in [(true, true), (true, false), (false, true), (false, false)] {
                let c = Container::pack_with(&values, lo, hi, direct, table, run);
                assert_eq!(c, reference, "table {table} run {run}, {} values", values.len());
            }
            assert_eq!(Container::pack(&values, lo, hi, direct), reference);
        }
    }

    /// The test above sees a different palette order: a sorted palette (indices recomputed,
    /// `get` identical) must be refused.
    #[test]
    fn pack_check_catches_sorted_palette() {
        let values: Vec<u32> = (0..4096u32).map(|i| 40 - (i * 7919) % 37).collect();
        let reference = Container::pack_reference(&values, 4, 8, 15);
        let Container::Packed { bits, palette: Some(p), .. } = &reference else { panic!() };
        let mut sorted = p.to_vec();
        sorted.sort_unstable();
        let mut data = vec![0u64; longs_for(4096, *bits)];
        let vpl = values_per_long(*bits);
        for (i, v) in values.iter().enumerate() {
            let raw = sorted.iter().position(|x| x == v).unwrap() as u64;
            data[i / vpl] |= raw << ((i % vpl) * *bits as usize);
        }
        let saboteur = Container::Packed { bits: *bits, palette: Some(sorted.into()), data: data.into() };
        assert!((0..4096).all(|i| saboteur.get(i) == reference.get(i)));
        assert_ne!(saboteur, reference);
    }

    /// All random words, all widths, all indices of a section and of biomes.
    fn check_raw(table: &[Magic; 64]) -> Result<(), String> {
        let mut r = rng(0x5151_aaaa);
        let data: Vec<u64> = (0..4096).map(|_| r(u64::MAX)).collect();
        for bits in 1..=32u8 {
            for index in 0..4096 {
                let want = raw_reference(&data, bits, index);
                if raw_magic_in(table, &data, bits, index) != want {
                    return Err(format!("magic: {bits} bits, index {index}"));
                }
                if raw_mono(&data, bits, index) != want {
                    return Err(format!("constant: {bits} bits, index {index}"));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn get_paths_exhaustive() {
        check_raw(&MAGIC).unwrap();
        // With palette (unused entries and out-of-palette indices) and without.
        let mut r = rng(77);
        for bits in 1..=32u8 {
            for entries in [64usize, 4096] {
                let data: Box<[u64]> = (0..longs_for(entries, bits)).map(|_| r(u64::MAX)).collect();
                let pal: Option<Box<[u32]>> = (bits <= 8).then(|| (0..(1u32 << bits) / 2 + 1).map(|v| v * 3 + 1).collect());
                let c = Container::packed(entries, bits, pal, data).unwrap();
                for i in 0..entries {
                    let want = c.get_with(i, GetMode::Reference);
                    assert_eq!(c.get_with(i, GetMode::Magic), want);
                    assert_eq!(c.get_with(i, GetMode::Mono), want);
                    assert_eq!(c.get(i), want);
                }
            }
        }
    }

    /// Sabotage: multipliers rounded down => the exhaustive test must fail.
    #[test]
    fn get_check_catches_rounded_down_multiplier() {
        assert!(check_raw(&magic_table(false)).is_err());
    }

    fn unpack_cases() -> Vec<(Container, usize)> {
        let mut r = rng(4242);
        let mut cases = Vec::new();
        for bits in 1..=32u8 {
            for entries in [64usize, 4096] {
                let data: Box<[u64]> = (0..longs_for(entries, bits)).map(|_| r(u64::MAX)).collect();
                cases.push((Container::packed(entries, bits, None, data.clone()).unwrap(), entries));
                if bits <= 12 {
                    // Short palette (out-of-palette indices), duplicate entry, full palette.
                    let short: Box<[u32]> = (0..(1u32 << bits).div_ceil(3)).map(|v| v * 5 + 2).collect();
                    let full: Box<[u32]> = (0..1u32 << bits).map(|v| v % 7 + 9).collect();
                    cases.push((Container::packed(entries, bits, Some(short), data.clone()).unwrap(), entries));
                    cases.push((Container::packed(entries, bits, Some(full), data).unwrap(), entries));
                }
            }
        }
        for (values, lo, hi, direct) in value_sets() {
            let n = values.len();
            cases.push((Container::pack_reference(&values, lo, hi, direct), n));
        }
        cases
    }

    #[test]
    fn unpack_mono_matches_reference() {
        for (c, entries) in unpack_cases() {
            let mut want = vec![0u32; entries];
            let mut got = vec![u32::MAX; entries];
            c.unpack_reference(&mut want);
            c.unpack_mono(&mut got);
            assert_eq!(got, want, "{c:?}");
            for (i, &w) in want.iter().enumerate() {
                assert_eq!(c.get(i), w);
            }
        }
        // Output longer than the words: content kept then palette applied, as before.
        let c = Container::packed(3, 4, Some(vec![7, 8].into()), vec![0x10].into()).unwrap();
        let (mut a, mut b) = ([1u32, 0, 0, 1, 1, 5], [1u32, 0, 0, 1, 1, 5]);
        c.unpack_reference(&mut a[..1]);
        c.unpack_mono(&mut b[..1]);
        assert_eq!(a, b);
        let (mut a, mut b) = (vec![1u32; 20], vec![1u32; 20]);
        c.unpack_reference(&mut a);
        c.unpack_mono(&mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn column_iter_is_a_run_of_get() {
        for (c, entries) in unpack_cases() {
            if entries != 4096 {
                continue;
            }
            for (x, z) in [(0, 0), (15, 15), (3, 9), (11, 2)] {
                for ys in [0..16, 0..1, 5..12, 15..16, 7..7] {
                    let want: Vec<u32> = ys.clone().map(|y| c.get(crate::block_index(x, y, z))).collect();
                    let got: Vec<u32> = c.column_iter(x, z, ys).collect();
                    assert_eq!(got, want);
                }
            }
        }
        assert_eq!(Container::Single(9).column_iter(1, 2, 3..6).collect::<Vec<_>>(), [9, 9, 9]);
    }

    #[test]
    fn count_where_matches_unpacked_count() {
        for (c, entries) in unpack_cases() {
            let mut v = vec![0u32; entries];
            c.unpack_reference(&mut v);
            // Last case: no value answers (fast exit through the palette).
            for pred in [|s: u32| s != 0, |s: u32| s % 3 == 1, |s: u32| s > 9, |s: u32| s > u32::MAX - 1] {
                assert_eq!(c.count_where(entries, pred), v.iter().filter(|&&s| pred(s)).count(), "{c:?}");
            }
        }
        assert_eq!(Container::Single(0).count_where(4096, |s| s != 0), 0);
        assert_eq!(Container::Single(4).count_where(64, |s| s != 0), 64);
    }

    #[test]
    fn remap_first_seen_matches_pack_and_is_canonical() {
        let mut r = rng(999);
        for case in 0..400 {
            let plen = 1 + r(40) as usize;
            // Network palettes with duplicates and unused entries.
            let palette: Vec<u32> = (0..plen).map(|_| r(12) as u32).collect();
            let n = if case % 2 == 0 { 4096 } else { 64 };
            let used = 1 + r(plen as u64 + 2);
            let indices: Vec<u32> = (0..n).map(|_| r(used) as u32).collect();
            let values: Vec<u32> = indices.iter().map(|&i| palette.get(i as usize).copied().unwrap_or(0)).collect();
            let mut remapped = indices.clone();
            let canon = remap_first_seen(&mut remapped, &palette);
            for (k, &i) in remapped.iter().enumerate() {
                assert_eq!(canon[i as usize], values[k]);
            }
            match Container::pack_reference(&values, 1, 8, 15) {
                Container::Single(v) => assert_eq!(canon, [v]),
                Container::Packed { palette: Some(p), .. } => assert_eq!(&canon[..], &p[..]),
                Container::Packed { palette: None, .. } => unreachable!(),
            }
            assert!(is_canonical(&remapped, &canon));
            let same = remapped == indices && canon == palette;
            assert_eq!(is_canonical(&indices, &palette), same, "case {case}");
        }
        // Duplicate value used twice: not canonical.
        assert!(!is_canonical(&[0, 1, 0], &[5, 5]));
        assert!(!is_canonical(&[0, 2, 1], &[1, 2, 3]));
        assert!(!is_canonical(&[0, 1], &[1, 2, 3]));
        assert!(is_canonical(&[0, 0, 1, 0], &[4, 2]));
    }

    /// Successive writes: all `set` approaches give the same values everywhere;
    /// without widening, the exact same representation as the old path.
    #[test]
    fn set_variants_match_reference() {
        let mut r = rng(31337);
        for distinct in [1u32, 3, 16, 17, 255, 256, 300, 5000] {
            let values: Vec<u32> = (0..4096u32).map(|i| (i * 7919) % distinct).collect();
            let base = Container::pack_reference(&values, 4, 8, 15);
            let mut variants = [base.clone(), base.clone(), base.clone(), base.clone()];
            let mut reference = base;
            for _ in 0..400 {
                let (i, v) = (r(4096) as usize, r(u64::from(distinct) + 400) as u32);
                reference.set_with(i, v, 4, 8, 15, false, false);
                for (k, c) in variants.iter_mut().enumerate() {
                    c.set_with(i, v, 4, 8, 15, k & 1 != 0, k & 2 != 0);
                }
                assert_eq!(variants[1], reference);
                for c in &variants {
                    assert!((0..4096).all(|j| c.get(j) == reference.get(j)), "{distinct} values");
                }
            }
        }
        // Biomes: widening on 64 entries.
        let mut c = Container::pack_reference(&(0..64).map(|i| i % 2).collect::<Vec<_>>(), 1, 3, 7);
        let mut want: Vec<u32> = (0..64).map(|i| i % 2).collect();
        for (i, v) in [(5usize, 7u32), (9, 3), (10, 4), (11, 5), (12, 6)] {
            c.set_with(i, v, 1, 3, 7, true, true);
            want[i] = v;
            assert_eq!((0..64).map(|j| c.get(j)).collect::<Vec<_>>(), want);
        }
    }

    /// Expected canonical palette, computed naively on the unfolded values.
    fn naive_first_seen(values: &[u32], limit: usize) -> Option<(Vec<u32>, Vec<u8>)> {
        let mut pal: Vec<u32> = Vec::new();
        let mut idx = Vec::with_capacity(values.len());
        for &v in values {
            let k = match pal.iter().position(|&x| x == v) {
                Some(k) => k,
                None if pal.len() == limit => return None,
                None => {
                    pal.push(v);
                    pal.len() - 1
                }
            };
            idx.push(k as u8);
        }
        Some((pal, idx))
    }

    /// Same values under several in-memory representations: canonical packed, shuffled palette
    /// with duplicates and unused entries, out-of-palette indices, wider than necessary,
    /// direct, widened by `set`.
    fn representations_of(values: &[u32], lo: u8, hi: u8, direct: u8, r: &mut impl FnMut(u64) -> u64) -> Vec<Container> {
        let n = values.len();
        let mut out = vec![Container::pack_reference(values, lo, hi, direct)];
        let mut pal: Vec<u32> = Vec::new();
        for &v in values {
            if !pal.contains(&v) {
                pal.push(v);
            }
        }
        // Shuffled palette, duplicates and unused entries, on 8 bits.
        if pal.len() <= 120 {
            let mut p: Vec<u32> = pal.iter().rev().copied().collect();
            p.extend(pal.iter().copied());
            p.push(999_999);
            let mut data = vec![0u64; longs_for(n, 8)];
            for (i, &v) in values.iter().enumerate() {
                let hits: Vec<usize> = p.iter().enumerate().filter(|&(_, &x)| x == v).map(|(k, _)| k).collect();
                let k = hits[r(hits.len() as u64) as usize];
                data[i / 8] |= (k as u64) << ((i % 8) * 8);
            }
            out.push(Container::Packed { bits: 8, palette: Some(p.into()), data: data.into() });
        }
        // Out-of-palette indices (value 0): short palette on 5 bits.
        if pal.len() <= 20 {
            let mut data = vec![0u64; longs_for(n, 5)];
            for (i, &v) in values.iter().enumerate() {
                let k = if v == 0 && r(2) == 0 {
                    20 + r(12) as usize
                } else {
                    pal.iter().position(|&x| x == v).unwrap()
                };
                data[i / 12] |= (k as u64) << ((i % 12) * 5);
            }
            let p: Vec<u32> = pal.clone();
            let c = Container::Packed { bits: 5, palette: Some(p.into()), data: data.into() };
            if (0..n).all(|i| c.get(i) == values[i]) {
                out.push(c);
            }
        }
        // Direct (raw identifiers).
        if values.iter().all(|&v| v < 1 << 20) {
            let mut data = vec![0u64; longs_for(n, 20)];
            for (i, &v) in values.iter().enumerate() {
                data[i / 3] |= u64::from(v) << ((i % 3) * 20);
            }
            out.push(Container::Packed { bits: 20, palette: None, data: data.into() });
        }
        // Widened by `set` from a single value (non-canonical palette, stale entries).
        if n == SECTION_BLOCKS || n == crate::SECTION_BIOMES {
            let mut c = Container::Single(values[0] ^ 1);
            for (i, &v) in values.iter().enumerate() {
                c.set_with(i, v ^ 1, lo, hi, direct, true, true);
                c.set_with(i, v, lo, hi, direct, true, true);
            }
            assert!((0..n).all(|i| c.get(i) == values[i]));
            out.push(c);
        }
        out
    }

    #[test]
    fn first_seen_indices_matches_naive_palette() {
        let mut r = rng(4242);
        let mut seen_noncanonical = 0;
        for (values, lo, hi, direct) in value_sets() {
            let n = values.len();
            // Values too wide for the direct width: no faithful direct representation.
            if naive_first_seen(&values, 1 << hi).is_none() && values.iter().any(|&v| v >= 1 << direct) {
                continue;
            }
            for c in representations_of(&values, lo, hi, direct, &mut r) {
                let mut unpacked = vec![0u32; n];
                c.unpack_reference(&mut unpacked);
                assert_eq!(unpacked, values);
                if let Container::Packed { palette: Some(p), .. } = &c
                    && p.len() > 1
                {
                    seen_noncanonical += 1;
                }
                for limit in [8usize, 256] {
                    let mut pal = [0u32; 256];
                    let mut idx = [0u8; SECTION_BLOCKS];
                    let got = c.first_seen_indices(n, limit, direct, &mut pal, &mut idx);
                    match naive_first_seen(&values, limit) {
                        None => assert_eq!(got, None, "limit {limit}, {c:?}"),
                        Some((p, i)) => {
                            assert_eq!(got, Some(p.len()), "limit {limit}");
                            assert_eq!(&pal[..p.len()], &p[..]);
                            assert_eq!(&idx[..n], &i[..]);
                        }
                    }
                }
            }
        }
        assert!(seen_noncanonical > 20, "too few palettes tested: {seen_noncanonical}");
    }
}
