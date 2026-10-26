//! Optimisation switches of the generator, read once (first use) from
//! the environment: absent or any value other than `0` = new path, `0` = old path.
//! The equivalence tests call both paths explicitly, without going through here.
//! Dependencies: `NOISE_COLCONST`, `NOISE_SIMD` and `NOISE_AVX2` only act in the
//! precomputed path (`NOISE_YPRE`), and `NOISE_AVX2` only with `NOISE_SIMD`. To isolate a track
//! on the scene bench: ypre = YPRE=1 COLCONST=0 SIMD=0 against YPRE=0; colconst, simd = the single
//! track at 0; avx2 = AVX2=0 against all.

use std::sync::LazyLock;

fn on(names: &[&str]) -> bool {
    !names.iter().any(|n| rc_data::env_flags::read_os(n).and_then(|v| v.into_string().ok()).is_some_and(|v| v.trim() == "0"))
}

macro_rules! switch {
    ($(#[$m:meta])* $f:ident, $($name:literal),+) => {
        $(#[$m])*
        #[inline]
        pub fn $f() -> bool {
            static S: LazyLock<bool> = LazyLock::new(|| on(&[$($name),+]));
            *S
        }
    };
}

switch!(
    /// `floor` the Java way (truncation then correction) instead of the library call.
    floor_java,
    "RUSTCRAFT_OPT_FLOOR_JAVA"
);
switch!(
    /// Noise volume: work in y computed once per octave, not per column.
    noise_ypre,
    "RUSTCRAFT_OPT_NOISE_YPRE"
);
switch!(
    /// Noise volume: column whose y inputs are all equal computed once.
    noise_colconst,
    "RUSTCRAFT_OPT_NOISE_COLCONST"
);
switch!(
    /// Perlin: doubled permutation table (512) and gradients already in f32.
    perm512,
    "RUSTCRAFT_OPT_PERM512",
    "RUSTCRAFT_OPT_NOISE_PERM512"
);
switch!(
    /// Noise volume: runs of the same cell in y through a branchless kernel (vectorised).
    noise_simd,
    "RUSTCRAFT_OPT_NOISE_SIMD"
);
switch!(
    /// `sincos`: a single argument reduction and a single pair of series.
    sincos,
    "RUSTCRAFT_OPT_SINCOS"
);

switch!(
    /// Volume evaluator: choices (range, interval, lerp) that compute each branch only on
    /// the y rows where it is used.
    range_yvol,
    "RUSTCRAFT_OPT_RANGE_YVOL"
);

/// Switch off by default: only `=1` enables it.
fn opt_in(name: &str) -> bool {
    rc_data::env_flags::read_os(name).and_then(|v| v.into_string().ok()).is_some_and(|v| v.trim() == "1")
}

/// Evaluator: buffers taken back from the context, cache read without copy, cells
/// pre-sized. Off by default (`=1` enables it) until the quiet-machine bench: no gain
/// on the micro-bench with a loaded machine (1.29 ms/chunk against 1.29).
#[inline]
pub fn vol_pool() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| opt_in("RUSTCRAFT_OPT_VOL_POOL"));
    *S
}

/// Spline: point coordinates kept on the stack, sampling without dynamic call.
/// Off by default (`=1` enables it) until the quiet-machine bench (same figure as `vol_pool`).
#[inline]
pub fn spline_stack() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| opt_in("RUSTCRAFT_OPT_SPLINE_STACK"));
    *S
}

switch!(
    /// Biome tree: candidate distance carried, early exit at zero distance.
    btree_early,
    "RUSTCRAFT_OPT_BTREE_EARLY"
);
switch!(
    /// Flattened biome tree (contiguous children, i32 bounds as a structure of arrays).
    btree_soa,
    "RUSTCRAFT_OPT_BTREE_SOA",
    "RUSTCRAFT_OPT_BIOMETREE_FLAT"
);
/// Biome tree: memo of the last climate (identical target and last leaf). Off by
/// default (`=1` enables it): 96 repeated targets out of 1.57 million on the micro-bench, no gain.
#[inline]
pub fn btree_memo() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| {
        rc_data::env_flags::read_os("RUSTCRAFT_OPT_BTREE_MEMO").and_then(|v| v.into_string().ok()).is_some_and(|v| v.trim() == "1")
    });
    *S
}
/// Biome search over a sequence of points (`/locate biome`): stateless climate and leaf
/// computed in parallel batches, last leaf then applied in series. Off by default (`=1`
/// enables it) until the quiet-machine bench: the batches go through the common thread reserve, where
/// the client's generation and meshing (lowered-priority threads) make them wait; the
/// ×8.9 gain is only measured on an empty reserve.
#[inline]
pub fn btree_split() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| opt_in("RUSTCRAFT_OPT_BTREE_SPLIT"));
    *S
}
switch!(
    /// With `btree_split`: stateless search bounded by the distance of the neighbouring point's leaf
    /// (+ 1), instead of starting unbounded.
    btree_split_hint,
    "RUSTCRAFT_OPT_BTREE_SPLIT_HINT"
);
switch!(
    /// With `btree_split_hint`: the neighbouring leaf serves as candidate (fall back to the chained
    /// search if it stays the best and the last leaf is not at dmin); `=0`:
    /// bound d + 1 without candidate.
    btree_split_cand,
    "RUSTCRAFT_OPT_BTREE_SPLIT_CAND"
);
switch!(
    /// Biome zoom: memoised corner offsets (direct cache per corner).
    zoom_memo,
    "RUSTCRAFT_OPT_ZOOM_MEMO"
);

/// Flattened biome tree: distances of the children in AVX2, if the processor has it.
#[inline]
pub fn btree_simd() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| on(&["RUSTCRAFT_OPT_BTREE_SIMD"]) && avx2_available());
    *S
}

switch!(
    /// Terrain: block identifiers (dirt, grass, mycelium, water, lava) resolved once per
    /// generator, and table lookup without allocation for an already full name.
    terrain_ids,
    "RUSTCRAFT_OPT_TERRAIN_IDS"
);
switch!(
    /// Aquifers, filling: window of the 12 neighbouring cells kept per column (x, z, y cell).
    aq_colwin,
    "RUSTCRAFT_OPT_AQ_COLWIN"
);
switch!(
    /// Aquifers, filling: 4 nearest through a branchless comparison network (with
    /// `AQ_COLWIN` only).
    aq_simd,
    "RUSTCRAFT_OPT_AQ_SIMD"
);
switch!(
    /// Filling: positive density (default block) tested before the call to the aquifer.
    aq_pretest,
    "RUSTCRAFT_OPT_AQ_PRETEST"
);
switch!(
    /// Base height: density of the column by slices aligned on the cells, from the top
    /// downwards, stopped at the first block that satisfies the predicate.
    baseh_lazy,
    "RUSTCRAFT_OPT_BASEH_LAZY"
);
switch!(
    /// Base height: memo shared per generator, key (x, z, type).
    baseh_memo,
    "RUSTCRAFT_OPT_BASEH_MEMO"
);
switch!(
    /// Carved caves: normalised vertical difference computed once per y and per ellipsoid.
    carver_yd,
    "RUSTCRAFT_OPT_CARVER_YD"
);
switch!(
    /// Canyons: width buffer reused instead of one allocation per canyon.
    carver_widths,
    "RUSTCRAFT_OPT_CARVER_WIDTHS"
);
switch!(
    /// Carved caves: carving of each source recorded once then replayed per target chunk.
    carver_replay,
    "RUSTCRAFT_OPT_CARVER_REPLAY"
);
switch!(
    /// Volume: index of a block by shift and mask when the steps are powers of 2.
    vol_pow2,
    "RUSTCRAFT_OPT_VOL_POW2"
);
switch!(
    /// Materials: index of the block in the vein volume drawn from a column base.
    ore_idx,
    "RUSTCRAFT_OPT_ORE_IDX"
);
switch!(
    /// Materials: vein skipped on the y rows where its density is surely a constant ≤ 0.
    ore_early,
    "RUSTCRAFT_OPT_ORE_EARLY"
);
switch!(
    /// Surface: root rule not evaluated on the rows where it surely returns "nothing".
    surf_skip,
    "RUSTCRAFT_OPT_SURF_SKIP"
);
switch!(
    /// Surface: rule tree specialised per y band (conditions decided by y removed).
    surf_bands,
    "RUSTCRAFT_OPT_SURF_BANDS"
);
switch!(
    /// Vein pre-filling: only the y rows where the density is not constant.
    vein_yrange,
    "RUSTCRAFT_OPT_VEIN_YRANGE"
);
switch!(
    /// Vein pre-filling: richness only on the rows where a density is positive.
    vein_lazyrich,
    "RUSTCRAFT_OPT_VEIN_LAZYRICH"
);
switch!(
    /// Vein pre-filling: a single y range for all the veins (shared caches).
    vein_shared,
    "RUSTCRAFT_OPT_VEIN_SHARED"
);
switch!(
    /// Materials: evaluation state and buffers reused per thread, lazy cave veins.
    mat_reuse,
    "RUSTCRAFT_OPT_MAT_REUSE"
);
switch!(
    /// Materials: column constants (bands, temperature) kept per column.
    mat_colcache,
    "RUSTCRAFT_OPT_MAT_COLCACHE"
);
switch!(
    /// Lookups by name (tags, placed features) without allocated string: full name borrowed,
    /// `minecraft:` prefix composed on the stack.
    tag_borrow,
    "RUSTCRAFT_OPT_TAG_BORROW"
);
switch!(
    /// Shape updates: families as block sets and properties resolved once, block
    /// outside any family sent directly to the decoration rules.
    shape_keys,
    "RUSTCRAFT_OPT_SHAPE_KEYS"
);
switch!(
    /// Position sets of the trees: buckets kept at clear, in-place doubling, cursor
    /// of the first non-empty bucket.
    jset_keep,
    "RUSTCRAFT_OPT_JSET_KEEP"
);
switch!(
    /// Tree decorators: lists copied into reused buffers, exclusion set
    /// kept, trunks poured without an intermediate list.
    tree_take,
    "RUSTCRAFT_OPT_TREE_TAKE"
);
switch!(
    /// Faces on the edge of a tree: shape update skipped for a block it never changes.
    tree_faceskip,
    "RUSTCRAFT_OPT_TREE_FACESKIP"
);
switch!(
    /// Icebergs: sine and cosine of the angle computed once per iceberg and per slice.
    berg_trig,
    "RUSTCRAFT_OPT_BERG_TRIG"
);
switch!(
    /// Veins: chunk of each column (x, z) resolved once per x in the sphere loop.
    ore_col,
    "RUSTCRAFT_OPT_ORE_COL"
);
switch!(
    /// Freezing and snow: noise-free temperature when the altitude noise bound proves the
    /// biome stays warm enough to rain.
    freeze_threshold,
    "RUSTCRAFT_OPT_FREEZE_THRESHOLD"
);
switch!(
    /// Surface freezing: frozen part and altitude noise computed once for y and y − 1.
    freeze_col,
    "RUSTCRAFT_OPT_FREEZE_COL"
);
switch!(
    /// Geodes: 1/√ terms of the first pass reused in the second.
    geode_reuse,
    "RUSTCRAFT_OPT_GEODE_REUSE"
);
switch!(
    /// Geodes: row in x skipped when its possible maximum stays under the outer crust.
    geode_rowskip,
    "RUSTCRAFT_OPT_GEODE_ROWSKIP"
);
switch!(
    /// Lookups by name of the features (bamboo, monster room, chorus, paths, shelf mushrooms,
    /// pale moss patch) resolved once.
    decor_ids,
    "RUSTCRAFT_OPT_DECOR_IDS"
);
switch!(
    /// Boolean properties and directions by precomputed value index, presence of a
    /// property by block bit mask.
    prop_idx,
    "RUSTCRAFT_OPT_PROP_IDX"
);

// ---- Templates and jigsaw (C1-M11). The proofs force a path on their thread through `force`, because
// these paths are buried under assembly and placement (no parameter to pass down).

#[cfg(test)]
thread_local! {
    static FORCED: std::cell::RefCell<Vec<(&'static str, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Forces (tests) the value of the named switches on the current thread; `None` = environment.
#[cfg(test)]
pub(crate) fn force(names: &[&'static str], v: Option<bool>) {
    FORCED.with(|f| {
        let mut f = f.borrow_mut();
        f.retain(|(n, _)| !names.contains(n));
        if let Some(v) = v {
            f.extend(names.iter().map(|n| (*n, v)));
        }
    });
}

#[cfg(test)]
fn forced(name: &str) -> Option<bool> {
    FORCED.with(|f| f.borrow().iter().find(|(n, _)| *n == name).map(|(_, v)| *v))
}

/// Switches of C1-M11, in track order (names without the `RUSTCRAFT_OPT_` prefix).
#[cfg(test)]
pub(crate) const C1M11: [&str; 10] = [
    "TEMPL_TABLE",
    "ELEM_ONCE",
    "POOLS_TABLE",
    "JIGSAW_PRECALC",
    "JIGSAW_IDX",
    "PROCS_SHARED",
    "PROCS_TABLES",
    "PROCESS_BOX",
    "LOCKED_SET",
    "STATETRANS_FAST",
];

macro_rules! switch_f {
    ($(#[$m:meta])* $f:ident, $short:literal) => {
        $(#[$m])*
        #[inline]
        pub fn $f() -> bool {
            #[cfg(test)]
            if let Some(v) = forced($short) {
                return v;
            }
            static S: LazyLock<bool> = LazyLock::new(|| on(&[concat!("RUSTCRAFT_OPT_", $short)]));
            *S
        }
    };
}

switch_f!(
    /// Templates: table frozen at construction (bytes + single-fill cell per id),
    /// read without lock, borrowed key; a single decoding per template even with several threads.
    templ_table,
    "TEMPL_TABLE"
);
switch_f!(
    /// "Single template" element: template resolved once and kept in the element.
    elem_once,
    "ELEM_ONCE"
);
switch_f!(
    /// Pools, processor lists and maximum size of a pool: frozen tables with single
    /// cells, read without lock.
    pools_table,
    "POOLS_TABLE"
);
switch_f!(
    /// Assembly: junctions transformed and box precomputed per (element, rotation) at
    /// zero position; box of each trial deduced by displacement.
    jigsaw_precalc,
    "JIGSAW_PRECALC"
);
switch_f!(
    /// Assembly: candidates and junctions shuffled by indices (no copy of the elements).
    jigsaw_idx,
    "JIGSAW_IDX"
);
switch_f!(
    /// Placement: shared processor list (prefix, common list, suffix) instead of a copy
    /// per placed piece; inline processors of an element analysed once.
    procs_shared,
    "PROCS_SHARED"
);
switch_f!(
    /// Processors: marker blocks, blackstone table, ageing classes and
    /// loot containers resolved once per block.
    procs_tables,
    "PROCS_TABLES"
);
switch_f!(
    /// Placement: only the template blocks in the y slice of the area are walked.
    process_box,
    "PROCESS_BOX"
);
switch_f!(
    /// Placement: positions of the locked sources in a hashed set.
    locked_set,
    "LOCKED_SET"
);
switch_f!(
    /// State transformation tables: blocks without an oriented property copied as is.
    statetrans_fast,
    "STATETRANS_FAST"
);

/// Vectorised kernel compiled for AVX2, if the processor has it (detection done once).
#[inline]
pub fn noise_avx2() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| on(&["RUSTCRAFT_OPT_NOISE_AVX2"]) && avx2_available());
    *S
}

/// AVX2 present on this processor.
pub fn avx2_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}
