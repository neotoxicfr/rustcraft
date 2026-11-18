//! In-place write of a partial remesh: the elements (faces, fluid faces, translucent faces) that
//! differ between the mesh and its base, when their numbers did not change, so that the GPU only
//! receives those.
//!
//! The flat sequences compared are those the GPU holds: the faces of the three layers end to end,
//! the fluid faces of the opaque and cutout layers end to end, the translucent faces.

use std::ops::Range;

use super::{BlockMesh, FLUID_WORDS, PackedView, fluid_words, untagged};
use crate::wire::Sections;

/// Ranges (in elements) of each flat sequence that differ from the base, sorted and disjoint;
/// empty for an identical sequence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InPlace {
    /// Faces of the three layers.
    pub faces: Vec<Range<u32>>,
    /// Fluids of the opaque and cutout layers.
    pub fluids: Vec<Range<u32>>,
    /// Translucent fluids.
    pub trans: Vec<Range<u32>>,
}

/// How [`in_place_diff_with`] finds the changed elements (same writes on the GPU at the end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InPlacePaths {
    /// Only the remeshed sections are compared, and the (layer, group, section) whose place moved
    /// (`RUSTCRAFT_OPT_IN_PLACE_TOUCHED_ONLY`): the other sections were copied from the base to the
    /// same place, so they cannot differ. Otherwise the whole column is compared.
    pub touched_only: bool,
    /// Several ranges per sequence (`RUSTCRAFT_OPT_IN_PLACE_RANGES`): two runs of changed elements
    /// separated by at most `gaps[k]` unchanged elements (faces, fluids, translucents) are written
    /// as one. `None`: a single range from the first change to the last.
    pub gaps: Option<[u32; 3]>,
}

impl InPlacePaths {
    /// The old path: whole column compared, one range per sequence.
    pub const OLD: InPlacePaths = InPlacePaths { touched_only: false, gaps: None };
}

/// Ranges to rewrite to go from the base `base` (flat, [`super::pack`]) to the mesh `m`, if the
/// numbers of faces, fluids and translucents are unchanged (otherwise `None`: new place). Old
/// path: the whole column compared from each end, one range per sequence.
pub fn in_place_diff(base: &[u32], m: &BlockMesh) -> Option<InPlace> {
    let v = PackedView::new(base)?;
    let n = same_counts(&v, m)?;
    // The base's faces without the GPU tag it may carry (`pack_tagged`).
    let base_faces = || v.faces.iter().flat_map(|f| f.as_chunks::<6>().0.iter().map(untagged));
    let new_faces = || m.layers.iter().flatten().copied();
    let words = |l: usize| v.fluids[l].as_chunks::<FLUID_WORDS>().0.iter().copied();
    let quads = |l: usize| m.fluids[l].iter().map(fluid_words);
    Some(InPlace {
        faces: diff_span(n[0], base_faces, new_faces).into_iter().collect(),
        fluids: diff_span(n[1], || words(0).chain(words(1)), || quads(0).chain(quads(1))).into_iter().collect(),
        trans: diff_span(n[2], || words(2), || quads(2)).into_iter().collect(),
    })
}

/// [`in_place_diff`] by the paths `paths`. `remeshed`: the sections the mesher redid (the job's
/// `reuse` mask), all the others copied from this very base; `None` (or a base of another column or
/// height, which the mesher would not have reused) compares the whole column.
pub fn in_place_diff_with(base: &[u32], m: &BlockMesh, remeshed: Option<Sections>, paths: InPlacePaths) -> Option<InPlace> {
    if paths == InPlacePaths::OLD {
        return in_place_diff(base, m);
    }
    let v = PackedView::new(base)?;
    let n = same_counts(&v, m)?;
    let copied_from_base = (v.x, v.z, v.sections) == (m.x, m.z, m.vis.len())
        && (0..3).all(|l| v.starts[l].len() == m.starts[l].len() && v.fluid_starts[l].len() == m.fluid_starts[l].len());
    let remeshed = remeshed.filter(|_| paths.touched_only && copied_from_base);
    let gaps = paths.gaps.map(|g| g.map(Some)).unwrap_or([None; 3]);
    let (base_faces, new_faces) = (FlatParts::new(v.faces.map(|w| w.as_chunks::<6>().0)), FlatParts::of(&m.layers));
    let faces = match remeshed {
        Some(mask) => touched_face_ranges(&v, m, mask),
        None => whole(n[0]),
    };
    let faces = changed_ranges(&faces, gaps[0], |i| untagged(base_faces.get(i)) != *new_faces.get(i));
    let fluid_ranges = |layers: Range<usize>, total: usize| match remeshed {
        Some(mask) => touched_fluid_ranges(&v, m, mask, layers),
        None => whole(total),
    };
    let base_fluids = |l: Range<usize>| {
        let mut parts: [&[[u32; FLUID_WORDS]]; 3] = [&[]; 3];
        for (k, l) in l.enumerate() {
            parts[k] = v.fluids[l].as_chunks::<FLUID_WORDS>().0;
        }
        FlatParts::new(parts)
    };
    let new_fluids = |l: Range<usize>| {
        let mut parts: [&[crate::fluid::FluidQuad]; 3] = [&[]; 3];
        for (k, l) in l.enumerate() {
            parts[k] = &m.fluids[l];
        }
        FlatParts::new(parts)
    };
    let (bf, nf) = (base_fluids(0..2), new_fluids(0..2));
    let fluids = changed_ranges(&fluid_ranges(0..2, n[1]), gaps[1], |i| fluid_words(nf.get(i)) != *bf.get(i));
    let (bt, nt) = (base_fluids(2..3), new_fluids(2..3));
    let trans = changed_ranges(&fluid_ranges(2..3, n[2]), gaps[2], |i| fluid_words(nt.get(i)) != *bt.get(i));
    Some(InPlace { faces, fluids, trans })
}

/// Numbers of faces, fluids and translucents of `m`, if the base has the same.
fn same_counts(v: &PackedView<'_>, m: &BlockMesh) -> Option<[usize; 3]> {
    let n = [m.layers.iter().map(Vec::len).sum::<usize>(), m.fluids[0].len() + m.fluids[1].len(), m.fluids[2].len()];
    let base_fluids = |l: usize| v.fluids[l].len() / FLUID_WORDS;
    let base = [v.faces.iter().map(|f| f.len() / 6).sum::<usize>(), base_fluids(0) + base_fluids(1), base_fluids(2)];
    (base == n).then_some(n)
}

/// Elements that differ between two sequences of the same length `n`: from the first different to the
/// last (end excluded), `None` if they are identical.
fn diff_span<A, B>(n: usize, a: impl Fn() -> A, b: impl Fn() -> B) -> Option<Range<u32>>
where
    A: DoubleEndedIterator,
    B: DoubleEndedIterator,
    A::Item: PartialEq<B::Item>,
{
    let first = a().zip(b()).position(|(x, y)| x != y)?;
    let last = n - 1 - a().rev().zip(b().rev()).position(|(x, y)| x != y)?;
    Some(first as u32..last as u32 + 1)
}

/// A flat sequence made of up to three slices put end to end, read by element index.
struct FlatParts<'a, T> {
    parts: [&'a [T]; 3],
    /// Index of the first element after each part.
    ends: [usize; 3],
}

impl<'a, T> FlatParts<'a, T> {
    fn new(parts: [&'a [T]; 3]) -> Self {
        let ends = [parts[0].len(), parts[0].len() + parts[1].len(), parts[0].len() + parts[1].len() + parts[2].len()];
        FlatParts { parts, ends }
    }

    fn of(layers: &'a [Vec<T>; 3]) -> Self {
        Self::new(layers.each_ref().map(Vec::as_slice))
    }

    fn get(&self, i: usize) -> &'a T {
        if i < self.ends[0] {
            &self.parts[0][i]
        } else if i < self.ends[1] {
            &self.parts[1][i - self.ends[0]]
        } else {
            &self.parts[2][i - self.ends[1]]
        }
    }
}

/// The whole sequence of `n` elements as the only range to compare.
fn whole(n: usize) -> Vec<Range<usize>> {
    std::iter::once(0..n).filter(|r| !r.is_empty()).collect()
}

/// Adds a flat range to the sorted list `out`, merged with the last one if they touch.
fn push_range(out: &mut Vec<Range<usize>>, r: Range<usize>) {
    match out.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => out.push(r),
    }
}

/// Flat ranges of the faces (layers end to end) that may differ from the base: every (layer,
/// group, section) of a remeshed section, and every one whose place in the flat sequence moved.
/// The others were copied from the base by the mesher, group by group, to the same place.
fn touched_face_ranges(v: &PackedView<'_>, m: &BlockMesh, remeshed: Sections) -> Vec<Range<usize>> {
    let sections_plus_one = m.vis.len() + 1;
    let mut out = Vec::new();
    let (mut new_at, mut base_at) = (0, 0);
    for l in 0..3 {
        let (new_len, base_len) = (m.layers[l].len(), v.faces[l].len() / 6);
        cells_to_compare(&m.starts[l], new_len, v.starts[l], base_len, (new_at, base_at), &mut out, |k| {
            remeshed.contains(k % sections_plus_one)
        });
        (new_at, base_at) = (new_at + new_len, base_at + base_len);
    }
    out
}

/// [`touched_face_ranges`] for the fluid faces of the layers `layers` put end to end (starts per
/// section only).
fn touched_fluid_ranges(v: &PackedView<'_>, m: &BlockMesh, remeshed: Sections, layers: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut new_at, mut base_at) = (0, 0);
    for l in layers {
        let (new_len, base_len) = (m.fluids[l].len(), v.fluids[l].len() / FLUID_WORDS);
        cells_to_compare(&m.fluid_starts[l], new_len, v.fluid_starts[l], base_len, (new_at, base_at), &mut out, |s| {
            remeshed.contains(s)
        });
        (new_at, base_at) = (new_at + new_len, base_at + base_len);
    }
    out
}

/// Cells of one layer (`starts`: start of each cell, the next start or the layer's length ends it)
/// to compare: those `touched`, and those whose flat place (`at`: flat start of the layer, new
/// then base) differs from the base's. Added to `out` in flat order.
fn cells_to_compare(
    new_starts: &[u32],
    new_len: usize,
    base_starts: &[u32],
    base_len: usize,
    (new_at, base_at): (usize, usize),
    out: &mut Vec<Range<usize>>,
    touched: impl Fn(usize) -> bool,
) {
    let end = |starts: &[u32], k: usize, len: usize| starts.get(k + 1).map_or(len, |&v| v as usize);
    for k in 0..new_starts.len() {
        let new = new_at + new_starts[k] as usize..new_at + end(new_starts, k, new_len);
        let base = base_at + base_starts[k] as usize..base_at + end(base_starts, k, base_len);
        if !new.is_empty() && (touched(k) || new != base) {
            push_range(out, new);
        }
    }
}

/// Changed elements among `candidates` (sorted, disjoint flat ranges; the elements outside them are
/// known identical): one range from the first change to the last (`gap` `None`, both ends found
/// without walking the middle), or each run of changed elements, two runs separated by at most
/// `gap` unchanged elements merged.
fn changed_ranges(candidates: &[Range<usize>], gap: Option<u32>, differs: impl Fn(usize) -> bool) -> Vec<Range<u32>> {
    let mut changed = candidates.iter().flat_map(Range::clone).filter(|&i| differs(i));
    let Some(gap) = gap else {
        let Some(first) = changed.next() else { return Vec::new() };
        let last = candidates.iter().rev().flat_map(|r| r.clone().rev()).find(|&i| differs(i)).unwrap_or(first);
        return std::iter::once(first as u32..last as u32 + 1).collect();
    };
    let mut out: Vec<Range<u32>> = Vec::new();
    for i in changed.map(|i| i as u32) {
        match out.last_mut() {
            Some(last) if i - last.end <= gap => last.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Face;

    /// Runs of changes merged under the gap; one range without a gap; nothing for no change.
    #[test]
    #[expect(clippy::single_range_in_vec_init, reason = "lists of candidate ranges, one of them")]
    fn changed_runs_merge_under_the_gap() {
        let changed = [2usize, 3, 7, 20, 21];
        let differs = |i| changed.contains(&i);
        let all = [0..30];
        assert_eq!(changed_ranges(&all, None, differs), vec![2..22]);
        assert_eq!(changed_ranges(&all, Some(0), differs), vec![2..4, 7..8, 20..22]);
        assert_eq!(changed_ranges(&all, Some(3), differs), vec![2..8, 20..22]);
        assert_eq!(changed_ranges(&all, Some(12), differs), vec![2..22]);
        assert!(changed_ranges(&all, Some(4), |_| false).is_empty());
        // Only the candidates are looked at.
        assert_eq!(changed_ranges(&[0..5, 18..25], Some(0), differs), vec![2..4, 20..22]);
        assert_eq!(changed_ranges(&[5..19], None, differs), vec![7..8]);
    }

    /// A face of the flat sequence read in the right part.
    #[test]
    fn flat_parts_read_across_layers() {
        let layers: [Vec<Face>; 3] = [vec![[1; 6]; 2], Vec::new(), vec![[3; 6]; 3]];
        let p = FlatParts::of(&layers);
        assert_eq!((0..5).map(|i| p.get(i)[0]).collect::<Vec<_>>(), [1, 1, 3, 3, 3]);
    }
}
