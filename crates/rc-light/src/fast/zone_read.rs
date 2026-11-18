//! Zone read by row segments (mission C2-M10, `docs/perf.md`): the same arrays, cell for cell, as
//! the cell-by-cell read of `fast.rs` (`read_rows_cell_by_cell`), kept for `LIGHTREADROWS=0`.
//! - `LIGHTREADROWS`: in each section met, the source of the cell properties is chosen once
//!   ([`CellSource`]) and the cells are copied row segment by row segment (consecutive x: the order
//!   of the zone arrays and of the section alike). One start index per row instead of a zone index,
//!   a block index, a source match and five bounds checks per cell; the light nibbles are unpacked
//!   two by two from each byte; the palette properties live for the whole read instead of a 2 KB
//!   table zeroed per section.
//! - `LIGHTREADFILL` (with `LIGHTREADROWS`): what is uniform is filled instead of copied cell by
//!   cell: a section of a single state, rows outside the world or of an absent column, and the
//!   light sheets shared as uniform since C5-M06 (all 0: sky under the terrain, unlit block sheets;
//!   all 15: open sky), recognised by their address.

use std::ops::Range;
use std::sync::OnceLock;

use rc_world::LIGHT_BYTES;

use super::*;

/// Where the properties (shape, propagation byte, emission) of a section's cells come from,
/// chosen once per section.
pub(super) enum CellSource<'a> {
    /// The same state everywhere: a single-value section, or no section (air).
    Fixed((u32, u8, u8)),
    /// Palette indices unpacked into the index buffer, properties resolved once per palette entry.
    Palette,
    /// State identifiers unpacked into the index buffer (no palette, or more than 8 bits).
    Values,
    /// Too few cells read to unpack the section: each one looked up in the container.
    Cells(&'a Container),
}

/// Deliberate error of the tests: a row starting on an odd cell reads the wrong half of its first byte.
const SABOTAGE_ODD_ROW_READS_LOW_NIBBLE: u8 = 40;

/// Kinds of rows met by a read, counted on the test thread (coverage of the read proof).
#[derive(Clone, Copy)]
pub(super) enum RowKind {
    FixedState,
    Palette,
    Values,
    Cells,
    Outside,
    UniformSheet,
    UnpackedSheet,
}

#[cfg(test)]
thread_local! {
    /// Sections (or outside runs, or sheets) read on this thread, per [`RowKind`].
    pub(super) static ROWS_MET: std::cell::Cell<[usize; 7]> = const { std::cell::Cell::new([0; 7]) };
}

#[inline(always)]
fn met(kind: RowKind) {
    #[cfg(test)]
    ROWS_MET.with(|m| {
        let mut counts = m.get();
        counts[kind as usize] += 1;
        m.set(counts);
    });
    #[cfg(not(test))]
    let _ = kind;
}

/// Cells of a zone column: `x` and `z` world ranges inside one chunk.
struct ColumnSpan {
    x: Range<i32>,
    z: Range<i32>,
}

/// Row segments of a zone read inside one column: the cells `x` of the rows `(y, z)`, all inside
/// one section when they are read from it.
struct SectionRows {
    /// Index, in the output slice, of the first cell of the first row.
    first_cell: usize,
    /// Index of that cell in its section (block order, also the nibble order of a light sheet).
    first_block: usize,
    /// Cells per row.
    len: usize,
    rows_y: usize,
    rows_z: usize,
    plane: usize,
    ax: usize,
}

impl SectionRows {
    fn new(g: Geo, base: usize, span: &ColumnSpan, y: Range<i32>) -> Self {
        let (x, z) = (span.x.start, span.z.start);
        SectionRows {
            first_cell: g.at(x - g.lo[0], y.start - g.lo[1], z - g.lo[2]) - base,
            first_block: rc_world::block_index((x & 15) as usize, (y.start & 15) as usize, (z & 15) as usize),
            len: span.x.len(),
            rows_y: y.len(),
            rows_z: span.z.len(),
            plane: g.plane,
            ax: g.ax,
        }
    }

    fn cells(&self) -> usize {
        self.len * self.rows_y * self.rows_z
    }

    /// Calls `f(output index, block index)` at the start of every row segment. The block index is
    /// only meaningful for rows of one section.
    #[inline]
    fn for_each_row(&self, mut f: impl FnMut(usize, usize)) {
        for iy in 0..self.rows_y {
            for iz in 0..self.rows_z {
                f(self.first_cell + iy * self.plane + iz * self.ax, self.first_block + (iy << 8) + (iz << 4));
            }
        }
    }
}

/// Properties of the palette entries of the current section, kept for a whole read.
struct PaletteProps {
    props: [(u32, u8, u8); 256],
    /// Entries written by an earlier section (`props[..written]`). The old read started each section
    /// from a zeroed table, so the entries past the current palette size are put back to zero (an
    /// index can only pass the palette size on malformed data, shorter than its section).
    written: usize,
    /// One of the current palette's entries emits light.
    emits: bool,
}

impl PaletteProps {
    fn new() -> Self {
        PaletteProps { props: [(0, 0, 0); 256], written: 0, emits: false }
    }

    /// Resolves the `1 << bits` entries of a palette (an entry past its end reads as state 0).
    fn resolve(&mut self, t: &LightTables, palette: &[u32], bits: u8) {
        let n = 1usize << bits;
        self.emits = false;
        for (k, e) in self.props.iter_mut().enumerate().take(n) {
            *e = props(t, palette.get(k).copied().unwrap_or(0));
            self.emits |= e.2 > 0;
        }
        if self.written > n {
            self.props[n..self.written].fill((0, 0, 0));
        }
        self.written = n;
    }
}

/// Rows `ya..yb` (world y, within the zone) written into `out`, the arrays of the cell-by-cell read.
pub(super) fn read_section_rows(
    grid: &impl LightGrid,
    c: &Ctx,
    g: Geo,
    ya: i32,
    yb: i32,
    mut out: ZoneSlice,
    raw: &mut Vec<u32>,
) {
    raw.resize(rc_world::SECTION_BLOCKS, 0);
    let mut palette = PaletteProps::new();
    let (x0, x1) = (g.lo[0], g.lo[0] + g.size[0]);
    let (z0, z1) = (g.lo[2], g.lo[2] + g.size[2]);
    for cz in (z0 >> 4)..=((z1 - 1) >> 4) {
        for cx in (x0 >> 4)..=((x1 - 1) >> 4) {
            let span =
                ColumnSpan { x: x0.max(cx * 16)..x1.min(cx * 16 + 16), z: z0.max(cz * 16)..z1.min(cz * 16 + 16) };
            let mut read = ColumnRead { c, g, span: &span, out: &mut out, raw: &mut *raw, palette: &mut palette };
            read.rows(grid.column(cx, cz), ya..yb);
        }
    }
}

/// Read of the rows of one zone column, and what it reuses from one section to the next.
struct ColumnRead<'r, 'a> {
    c: &'r Ctx<'r>,
    g: Geo,
    span: &'r ColumnSpan,
    out: &'r mut ZoneSlice<'a>,
    raw: &'r mut Vec<u32>,
    palette: &'r mut PaletteProps,
}

impl ColumnRead<'_, '_> {
    /// Rows `ys` of the column (`col` absent: neither light nor passage), section by section.
    fn rows(&mut self, col: Option<(&[Section], &[LightLayer], &[LightLayer])>, ys: Range<i32>) {
        let h = self.c.height;
        let mut y = ys.start;
        while y < ys.end {
            let Some((sections, sky_sheets, block_sheets)) = col.filter(|_| (0..h).contains(&y)) else {
                // Outside the world, or absent column: all such rows up to the world (or the end).
                let end = if col.is_some() && y < 0 { ys.end.min(0) } else { ys.end };
                let sky_value = if col.is_some() && y >= h { 15 } else { 0 };
                self.outside_rows(y..end, sky_value);
                y = end;
                continue;
            };
            let sec = y >> 4;
            let end = ys.end.min(sec * 16 + 16).min(h);
            let rows = SectionRows::new(self.g, self.out.base, self.span, y..end);
            let blocks = sections.get(sec as usize).map(|s| &s.blocks);
            let source = cell_source(self.c.t, blocks, rows.cells(), self.raw, self.palette);
            self.copy_props(&rows, &source);
            let fill = self.c.o.fill_uniform_rows;
            if self.out.want_sky {
                copy_sheet(self.out.sky, &rows, sheet_of(sky_sheets, sec), fill);
            }
            if self.out.want_blk {
                copy_sheet(self.out.blk, &rows, sheet_of(block_sheets, sec), fill);
            }
            let may_emit = seed_at_read::may_emit(&source, self.palette.emits);
            seed_at_read::prepare_section(self.out, self.g, self.span.x.clone(), y..end, self.span.z.clone(), may_emit);
            y = end;
        }
    }

    /// Rows outside the world or of an absent column: air without a sheet, `sky_value` for the sky
    /// (15 without the sheet bit above the world, 0 elsewhere).
    fn outside_rows(&mut self, ys: Range<i32>, sky_value: u8) {
        met(RowKind::Outside);
        let rows = SectionRows::new(self.g, self.out.base, self.span, ys);
        let air = props(self.c.t, 0);
        self.copy_props(&rows, &CellSource::Fixed(air));
        if self.out.want_sky {
            fill_rows(self.out.sky, &rows, sky_value);
        }
        if self.out.want_blk {
            fill_rows(self.out.blk, &rows, 0);
        }
    }

    /// Shape, propagation byte and emission of the cells of `rows`, from `source`.
    fn copy_props(&mut self, rows: &SectionRows, source: &CellSource) {
        let (t, raw, palette) = (self.c.t, &*self.raw, &*self.palette);
        match *source {
            CellSource::Fixed(p) if self.c.o.fill_uniform_rows => fill_props(self.out, rows, p),
            CellSource::Fixed(p) => copy_props_rows(self.out, rows, |_| p),
            CellSource::Palette => copy_props_rows(self.out, rows, |k| palette.props[raw[k] as usize]),
            CellSource::Values => copy_props_rows(self.out, rows, |k| props(t, raw[k])),
            CellSource::Cells(b) => copy_props_rows(self.out, rows, |k| props(t, b.get(k))),
        }
    }
}

/// Light sheet of section `sec` (sheets start one section below the world), if present.
fn sheet_of(sheets: &[LightLayer], sec: i32) -> Option<&[u8; LIGHT_BYTES]> {
    sheets.get(sec as usize + 1).and_then(|l| l.as_deref())
}

/// Source of the cell properties of a section of which `cells` cells are read (the choice of the
/// cell-by-cell read): unpacks its indices into `raw` and resolves its palette when the copy reads them.
fn cell_source<'a>(
    t: &LightTables,
    blocks: Option<&'a Container>,
    cells: usize,
    raw: &mut [u32],
    palette: &mut PaletteProps,
) -> CellSource<'a> {
    let source = match blocks {
        None => CellSource::Fixed(props(t, 0)),
        Some(Container::Single(v)) => CellSource::Fixed(props(t, *v)),
        Some(b) if cells < 512 => CellSource::Cells(b),
        Some(Container::Packed { bits, palette: Some(p), data }) if *bits <= 8 => {
            palette.resolve(t, p, *bits);
            unpack_raw(*bits, data, raw);
            CellSource::Palette
        }
        Some(b) => {
            b.unpack_into(raw);
            CellSource::Values
        }
    };
    met(match source {
        CellSource::Fixed(_) => RowKind::FixedState,
        CellSource::Palette => RowKind::Palette,
        CellSource::Values => RowKind::Values,
        CellSource::Cells(_) => RowKind::Cells,
    });
    source
}

/// Properties of every cell of `rows`, by block index; one closure per source, so each copy loop
/// is compiled for its source with no match left inside.
fn copy_props_rows(out: &mut ZoneSlice, rows: &SectionRows, props_at: impl Fn(usize) -> (u32, u8, u8)) {
    let len = rows.len;
    let ZoneSlice { face, prop, emit, want_blk, .. } = out;
    rows.for_each_row(|dst, block| {
        let (face, prop) = (&mut face[dst..dst + len], &mut prop[dst..dst + len]);
        if *want_blk {
            let emit = &mut emit[dst..dst + len];
            for i in 0..len {
                (face[i], prop[i], emit[i]) = props_at(block + i);
            }
        } else {
            for i in 0..len {
                (face[i], prop[i], _) = props_at(block + i);
            }
        }
    });
}

/// The same properties `p` over every cell of `rows`.
fn fill_props(out: &mut ZoneSlice, rows: &SectionRows, p: (u32, u8, u8)) {
    let len = rows.len;
    let ZoneSlice { face, prop, emit, want_blk, .. } = out;
    rows.for_each_row(|dst, _| {
        face[dst..dst + len].fill(p.0);
        prop[dst..dst + len].fill(p.1);
        if *want_blk {
            emit[dst..dst + len].fill(p.2);
        }
    });
}

/// The value `v` over every cell of `rows`.
fn fill_rows(values: &mut [u8], rows: &SectionRows, v: u8) {
    rows.for_each_row(|dst, _| values[dst..dst + rows.len].fill(v));
}

/// Light values of one sheet over `rows`: the stored value with the sheet bit `DATA`, 0 without a sheet.
fn copy_sheet(values: &mut [u8], rows: &SectionRows, sheet: Option<&[u8; LIGHT_BYTES]>, fill: bool) {
    let Some(sheet) = sheet else {
        fill_rows(values, rows, 0);
        return;
    };
    if fill && let Some(v) = uniform_sheet_value(sheet) {
        met(RowKind::UniformSheet);
        fill_rows(values, rows, DATA | v);
        return;
    }
    met(RowKind::UnpackedSheet);
    rows.for_each_row(|dst, block| unpack_sheet_row(sheet, block, &mut values[dst..dst + rows.len]));
}

/// Value of every cell of `sheet` when it is one of the process-wide uniform sheets (C5-M06): all 0
/// or all 15, recognised by its address (two clones kept here, so no reference count moves per read).
fn uniform_sheet_value(sheet: &[u8; LIGHT_BYTES]) -> Option<u8> {
    static SHARED: OnceLock<[std::sync::Arc<[u8; LIGHT_BYTES]>; 2]> = OnceLock::new();
    let [zero, full] = SHARED.get_or_init(|| [rc_world::zero_layer(), rc_world::full_layer()]);
    if std::ptr::eq(sheet, &**zero) {
        Some(0)
    } else if std::ptr::eq(sheet, &**full) {
        Some(15)
    } else {
        None
    }
}

/// Light values of the sheet cells `first..first + out.len()` with the sheet bit: the values of
/// `nibble`, two cells per byte read (low half first), in a loop the compiler can vectorise.
fn unpack_sheet_row(sheet: &[u8; LIGHT_BYTES], first: usize, out: &mut [u8]) {
    let (mut cell, mut out) = (first, out);
    if cell & 1 == 1 && !out.is_empty() {
        // A row starting on an odd cell: its first value is the high half of a byte.
        let shift = if sabotage(SABOTAGE_ODD_ROW_READS_LOW_NIBBLE) { 0 } else { 4 };
        let (head, rest) = std::mem::take(&mut out).split_at_mut(1);
        head[0] = DATA | (sheet[cell >> 1] >> shift) & 15;
        out = rest;
        cell += 1;
    }
    let bytes = &sheet[cell >> 1..];
    let (pairs, rest) = out.as_chunks_mut::<2>();
    for (pair, &byte) in pairs.iter_mut().zip(bytes) {
        *pair = [DATA | byte & 15, DATA | byte >> 4];
    }
    if let [last] = rest {
        *last = DATA | bytes[pairs.len()] & 15;
    }
}
