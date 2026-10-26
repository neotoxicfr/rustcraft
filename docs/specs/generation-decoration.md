# Generation, part 3: decoration and structures (26.3) — Rust singleplayer

Source: a reading of the decompiled server and of the jar's data (`data/minecraft/worldgen/…`,
`data/minecraft/structure/*.nbt`), kept outside the repository, restated in our own words. Frame:
`docs/architecture.md` § 14 (the singleplayer interprets the official data and must reproduce the
world of a seed down to the block). Neighbouring parts:

- `generation-noise.md` — random generators (the "legacy" congruential one, xoroshiro,
  positional factories), noises (Perlin, simplex, normal noise), density functions,
  biome sources, fuzzy biome zoom.
- `generation-terrain.md` — terrain filling (status `terrain`: density, aquifers,
  surface rules, cave carving), heightmaps, the structures' "beard"
  on the density side (§ 9.5 below gives its source).

This document covers what comes AFTER the terrain: decoration (step `features`) and the whole
life cycle of structures (starts, references, piece placement).

Conventions: "chunk" = a column of 16×16 blocks, "section" = 16×16×16. Identifiers without a
namespace are in `minecraft:`. `next_int(n)` (uniform integer in
[0, n[), `next_float()`, `next_double()`, `next_long()`, `next_boolean()`,
`next_gaussian()` and `reseed(g)` denote the draws of the current generator (defined in
`generation-noise.md`). "Feature"
= a configured decoration object: a tree, an ore vein…; "placed feature" = feature +
list of placement modifiers. All formulas are in signed 32-bit integers unless stated
otherwise (`long` = 64 bits, silent overflow as in Java).

---

## 1. Generation statuses and the place of decoration

Status pyramid in 26.3 (each row: status, dependencies, write radius):

| Status | Dependencies | Writes |
|---|---|---|
| `empty` | — | — |
| `structure_starts` | — | structure starts of the chunk (§ 8) |
| `structure_references` | `structure_starts` at 8 chunks | references (§ 9.1) |
| `biomes` | `structure_starts` at 8 | biomes of the chunk |
| `terrain` | `structure_starts` at 8, `biomes` at 1 | blocks of the chunk alone (radius 0) |
| `features` | `structure_starts` at 8, `terrain` at 1 | **decoration + structure pieces**, write radius 1 (3×3 chunks) |
| `initialize_light`, `light`, `spawn`, `full` | … | out of scope |

In 26.3, `terrain` groups the former noise/surface/carving statuses (see
`generation-terrain.md`). The decoration of a chunk C runs when its 8 neighbours are at
least at `terrain`; it reads and writes within the 3×3 chunks (generation region). A
write outside the 3×3 area is silently refused (the block is not placed, the
write function returns "failure"). Reads outside the area (Chebyshev distance 2 to 8)
fall on chunks at the `structure_starts` status only: no blocks (air), empty
heightmaps — a warning only; beyond 8: fatal error. In practice the official features
start from a point of the central chunk and stay within ±16 blocks.

The region also has its own randomness ("region random"): positional factory named
`worldgen_region_random` of the world's random state (construction: `generation-noise.md`),
taken at the block corner of the central chunk. It is only used for a few shape updates and for the
pale moss carpet (§ 6.2); it is independent of the decoration randomness.

**Consequence for the result**: the decoration of C can modify its neighbours, some of which
will themselves be decorated later. The final result therefore depends on the ORDER in which
chunks are decorated. The official server decorates in the order in which chunks are requested
(loading by players, multi-threaded). It is therefore NOT deterministic down to the block at
chunk borders when two neighbouring features overlap (overlapping trees,
lakes…). The singleplayer must: (a) reproduce exactly the algorithm for a given chunk; (b) for
verification (§ 13), compare while neutralising the order (generating the reference world and
ours in the same chunk order, or comparison limited to the undisputed blocks).

Heightmaps during decoration: chunks at status `terrain` or above keep the final
`WORLD_SURFACE`, `OCEAN_FLOOR`, `MOTION_BLOCKING`, `MOTION_BLOCKING_NO_LEAVES` maps up to date at
EVERY block placement. The `WORLD_SURFACE_WG` and
`OCEAN_FLOOR_WG` maps stay those computed at the end of the terrain (they do not follow the
decoration's placements). Reading a heightmap at (x, z) returns "y of the first free block above
the column" (= y of the highest retained block + 1; `min_y` if the column is empty).

Placing a block by the decoration (the region's write function):
- if the placed state carries a block entity, a "pending" entity is created (chests,
  beehives…); if the old state carried one and the new one does not, it is removed;
- unless the "no post-processing" flag (16), if the state requires post-processing (e.g.
  fluids, blocks depending on their neighbourhood), its position is marked for update when the
  chunk becomes complete (see `generation-terrain.md` for post-processing);
- most features place with flag 2 ("notify clients", with no neighbour
  update); some go through "place and update" (flag 3), with no neighbour
  effect during generation (no ticks), but which triggers the post-processing
  marking.

Post-processing when switching to an active chunk (light computed): each marked position receives the
shape update from its six neighbours. A mushroom (brown, red) only stays if the block
below lifts the light requirement (tag `overrides_mushroom_light_requirement`) or if the
raw brightness (max of sky and blocks) is below 13 and the block below is full for rendering; otherwise
it becomes air. The singleplayer therefore lights first the chunks that have marked mushrooms.

### 1.1 Secondary generators

- **Flat world**: the biome's feature list is recomposed once. If "lakes":
  `lake_lava_underground` then `lake_lava_surface` at step LAKES. If "decoration" (and the
  world is not entirely air, except biome `the_void`): all the biome's steps EXCEPT the
  underground and surface structures, and except LAKES if lakes are active. Each layer that
  does not block movement (`MOTION_BLOCKING` map) is removed from the fill and placed at the end
  of step `top_layer_modification` by a "fill the layer" feature (the whole chunk layer,
  where there is air, flag 2). The client validates the generator before opening a world: in
  singleplayer the recomposition precedes any chunk.
- **"Debug" world**: empty terrain; at decoration, a barrier at y = 60 over the whole chunk
  and, at y = 70, state no. `x/2 · L + z/2` (L = ceiling of √n as a float, n = number of states) at
  the odd positive cells x, z (air beyond L or n). No other placement is accepted in game.
- **Void platform**: chunks at Chebyshev distance ≤ 1 from chunk (0, 0); cells at most 16 from
  (8, 8), at y = world bottom + 3; cobblestone at (8, 8), stone elsewhere.
- **Checkerboard biome source**: biome `list[(qx >> s) + (qz >> s) mod n]`, s = scale + 2
  (low 5 bits only), direct list with duplicates, de-duplicated tag.

### 1.2 Regeneration below y = 0 (converted old chunk)

A chunk converted by the game carries `below_zero_retrogen` (target status, columns with no bedrock)
and stays at status `empty`. Its old blocks are kept; only the band −64..−1 is generated:
biomes of the band = the noise's cave biome (lush caves, dripstone caves, deep dark) otherwise
the column's old biome at y = 0; noise, surface, aquifers and carving on a generator bounded
to the band (no protection at the top for caves); then the old bedrock (y 0 to 4) becomes
deepslate and the columns with no bedrock are emptied over the band. While it is at the centre of
its decoration, any placement outside the band is refused (in the whole region). Missing
sections are plains. The blend with the old terrain (`blending_data`) is not done yet.

## 2. Order of a chunk's decoration

### 2.1 The 11 steps

Fixed order (rank = step index `s`, used in the seeds):

| s | Name (`step` of structures) | Typical content |
|---|---|---|
| 0 | `raw_generation` | small End islands, (nothing else on the surface) |
| 1 | `lakes` | lava lakes, sulfur pools |
| 2 | `local_modifications` | geodes, icebergs, forest rocks |
| 3 | `underground_structures` | monster rooms, fossils; underground structures |
| 4 | `surface_structures` | desert wells, blue ice spikes; surface structures |
| 5 | `strongholds` | (strongholds) |
| 6 | `underground_ores` | ores, disks, underwater magma |
| 7 | `underground_decoration` | dripstone formations, sculk patches, Nether ores… |
| 8 | `fluid_springs` | water and lava springs |
| 9 | `vegetal_decoration` | trees, flowers, grasses, mushrooms, cave vegetation |
| 10 | `top_layer_modification` | surface freezing (snow, ice) |

Each biome (`worldgen/biome/<name>.json`, key `features`) gives a list of 11 lists (one
per step; a shorter list = missing steps empty) of placed-feature identifiers.
Appendix A summarises the content by step and by biome.

### 2.2 Global index of a feature within a step (topological sort)

At generator start-up, for each step a single ordered list is built of ALL the
placed features of ALL the possible biomes of the dimension:

1. Order of the source biomes = order of "possible biomes" of the biome source: for the
   multi-noise source, order of first appearance in the parameter list
   (`multi_noise_biome_source_parameter_list`/preset, see `generation-noise.md`); for
   the End/the Nether likewise according to their source.
2. Each distinct placed feature is numbered (object identity: the same identifier = the same
   object) in order of first encounter, going biome by biome, step by step,
   feature by feature → "discovery number".
3. A graph is formed: node = (step, discovery number); for each biome, its list is flattened
   (all steps together, in order) and an edge is added from each feature
   to the next one in the flattened list.
4. Depth-first traversal: nodes are visited in increasing (step, number) order; the
   successors of each node also in increasing (step, number) order; each node is emitted
   on exit (post-order). The emitted list is reversed → topological order.
5. For each step `s`, the list `L_s` = features of the topological order whose step is
   `s`, in that order. A feature's global index = its position in `L_s`.
6. A cycle (two biomes ordering A and B in opposite ways) is a fatal error.

### 2.3 Decoration loop of a chunk (cx, cz)

```
origin = (16·cx, dimension_min_y, 16·cz)
deco_seed = decoration_seed(world_seed, 16·cx, 16·cz)        (§ 2.4)
possible_biomes = set of the biomes present in the sections of the 3×3 chunks
                   around C (all sections, "raw" biomes per 4×4×4 cell),
                   restricted to the possible biomes of the source
for s from 0 to max(11, num_steps) − 1:
    if structure generation is active:
        k = 0
        for each structure S of the registry whose "step" == s, in registry order:
            random.feature_seed(deco_seed, k, s)
            for each start of S referenced by C (§ 9.1):
                place its pieces in C (§ 9.2) with this random source
            k += 1          (k advances even if S has no start here)
    if s < num_feature_steps:
        I = { global index (in L_s) of each feature of step s of each possible biome }
        for each i of I sorted ascending:
            random.feature_seed(deco_seed, i, s)
            place L_s[i] from the origin WITH biome filter (§ 3)
```

Order of the structure registry: identifiers sorted (path then namespace, Java string
comparison). The same goes for all the registries loaded from the data.

### 2.4 Decoration generator and seeds

The decoration generator is a "generation generator" that wraps a **xoroshiro**
(not the congruential one):

- `next(bits)` = the HIGH-ORDER `bits` bits of a xoroshiro `next_long()`
  (`(int)(x >>> (64 − bits))`); all the other draws (`next_int(n)`, `next_long`,
  `next_float`, `next_double`, `next_boolean`) are built on `next(bits)` according to the
  "legacy" formulas (`next_long` = `(next(32) << 32) + next(32)` signed;
  `next_float` = `next(24) × 2⁻²⁴`; `next_double` = `((next(26) << 27) + next(27)) × 2⁻⁵³`;
  `next_int(n)`: if n is a power of 2, `(n × next(31)) >> 31`, otherwise reject while
  `next(31) − r + (n−1) < 0` with `r = next(31) mod n`).
- `next_gaussian`: Marsaglia polar method on the wrapper's `next_double`, with a cache of
  the second value. Trap: `reseed` of the wrapper does NOT reset this cache to zero (it only
  reinitialises the inner xoroshiro).
- `reseed(g)` reseeds the inner xoroshiro (64→128-bit extension, see
  `generation-noise.md`).

Formulas (all in `long`):

```
decoration_seed(G, x, z):            x, z = BLOCK coordinates of the chunk corner
    reseed(G)
    a = next_long() | 1
    b = next_long() | 1
    r = (x·a + z·b) XOR G
    reseed(r) ; return r

feature_seed(r, i, s):  reseed(r + i + 10000·s)

large_seed(G, cx, cz):                  cx, cz = CHUNK coordinates
    reseed(G) ; a = next_long() ; b = next_long()
    reseed((cx·a) XOR (cz·b) XOR G)

seed_with_salt(G, x, z, salt):  reseed(x·341873128712 + z·132897987541 + G + salt)
```

`large_seed` and `seed_with_salt` serve the structures (§ 7, § 8) and apply to a
generation generator wrapping a **congruential** one (there `next(bits)` = legacy
congruential).

## 3. Placing a placed feature

### 3.1 Algorithm

Input: placed feature (feature E + list of modifiers M₀…Mₙ₋₁), random source, origin, flag
"biome check" (true for the top-level features of biomes, false for a placed
feature nested in another feature).

- If n = 0: `E.place(origin)`.
- Otherwise, DEPTH-FIRST traversal with a stack (positions, modifier index): (origin, 0) is
  pushed. While the stack is not empty: pop (p, k); apply Mₖ to p, which
  produces a list of positions P (in production order, the randomness being consumed during
  this computation). If k+1 < n: push the positions of P in REVERSE order with index k+1 (so
  the first one produced is handled first, entirely to the end of the chain, before
  the second). If k+1 = n: call `E.place(q)` for each q of P, in order.

Consequence for the randomness: for the LAST modifier, all its positions are computed
(draws included) before the first placement; for the others, the placement of the first complete
branch precedes the draw of the second.

The result ("at least one placement succeeded") is only used by composite features (§ 5.8).

### 3.2 Modifiers (`placement[].type`)

| Type | Fields | Effect (1 input position p → outputs) |
|---|---|---|
| `count` | `count` (integer provider 0..4096) | draws N = count; outputs p N times |
| `noise_based_count` | `noise_to_count_ratio`, `noise_factor`, `noise_offset` (0) | N = ⌈(B(p.x/f, p.z/f) + offset) × ratio⌉ (B = biome-info noise, § 3.4); outputs p N times; no draw |
| `noise_threshold_count` | `noise_level`, `below_noise`, `above_noise` | v = B(p.x/200, p.z/200); N = `below_noise` if v < level otherwise `above_noise`; no draw |
| `count_on_every_layer` (obsolete) | `count` | see § 3.3 |
| `rarity_filter` | `chance` (integer > 0) | keeps p if `next_float() < 1/chance` (single-precision float) |
| `random_chance` | `chance` (0..1) | keeps p if `next_float() < chance` |
| `in_square` | — | x' = p.x + next_int(16); z' = p.z + next_int(16) (in that order); y unchanged |
| `height_range` | `height` (height provider § 3.5) | p with y = draw |
| `heightmap` | `heightmap` | h = map(p.x, p.z); outputs (p.x, h, p.z) if h > the region's min_y, otherwise nothing |
| `offset` | `x`, `y`, `z` (providers −16..16) | draws dx, then dy, then dz; outputs p + (dx, dy, dz) |
| `cuboid` | `xz_size`, `y_size` (1..16), `include_edges` (true), `include_interior` (true) | see § 3.3 |
| `environment_scan` | `direction_of_search` (up/down), `target_condition`, `allowed_search_condition` (true), `max_steps` (1..32) | see § 3.3 |
| `surface_relative_threshold_filter` | `heightmap`, `min_inclusive`, `max_inclusive` | h = map(p.x, p.z); keeps p if h+min ≤ p.y ≤ h+max (computed in `long`, absent bounds = ±∞) |
| `surface_water_depth_filter` | `max_water_depth` | keeps p if `WORLD_SURFACE(p) − OCEAN_FLOOR(p) ≤ max` |
| `block_predicate_filter` | `predicate` (§ 4) | keeps p if the predicate is true at p |
| `biome` | — | keeps p if the biome at p (ZOOMED biome, like a block's biome read in the game) contains the TOP-LEVEL placed feature in any of its steps. Error if used without a biome check |
| `randomly_selected` | `placements` (non-empty list) | chooses `placements[next_int(len)]` and applies it to p |
| `fixed_placement` | `positions` (list of absolute positions) | outputs each position of the list that falls in the same chunk as p, in order |

"map(x, z)" = heightmap read (§ 1), map types = `WORLD_SURFACE_WG`,
`WORLD_SURFACE`, `OCEAN_FLOOR_WG`, `OCEAN_FLOOR`, `MOTION_BLOCKING`,
`MOTION_BLOCKING_NO_LEAVES`.

### 3.3 Algorithmic modifiers

**`count_on_every_layer`** (still used by 8 placed features of the Nether):

```
layer = 0
repeat:
    found = false
    i = 0
    while i < count.sample():        ← the provider is RE-SAMPLED at every loop test
        x = p.x + next_int(16) ; z = p.z + next_int(16)
        y = find_layer(x, MOTION_BLOCKING(x, z), z, layer)
        if y exists: output (x, y, z) ; found = true
        i += 1
    layer += 1
while found
find_layer(x, y0, z, c): we go down from y = y0 to min_y+1 comparing the block at y
    ("current", initially the one at y0) and the one at y−1 ("below"); "empty" = air, water
    or lava. Each time below is non-empty, current empty and below ≠ bedrock: if the
    layer counter = c, return y; otherwise counter += 1. Then current = below.
```

**`cuboid`**: draws h = `y_size`, then l = `xz_size` (x width), then L = `xz_size`
(z length). Iterates x = 0..l, y = 0..h, z = 0..L (INCLUSIVE bounds, nested loops
x then y then z) and outputs p + (x, y, z) if:
- `include_edges` or the point is on no edge (an edge = two coordinates
  simultaneously at one of their bounds: (x∈{0,l} and y∈{0,h}), (z∈{0,L} and y∈{0,h}),
  (x∈{0,l} and z∈{0,L}));
- and `include_interior` or the point is on a face (at least one coordinate at a bound).

**`environment_scan`**: moving position q = p. If `allowed(q)` is false → nothing. Otherwise, for
i from 0 to max_steps−1: if `target(q)` → output q, end; q advances one step in the direction;
if q leaves the buildable height → nothing, end; if `allowed(q)` is false → leave the loop.
After the loop (exhaustion or stop): if `target(q)` → output q.

### 3.4 "Biome-info" noise

`B(x, z)` = 2D simplex noise built with a congruential generator of seed 2345, WITHOUT the
additional offset ("no offset 256" variant), value as a single-precision float × 70 (formula
in `generation-noise.md`). It serves the noise-based counts and the legacy snow/ice
layers. Constant for the whole world (independent of the seed).

### 3.5 Height providers and vertical anchors

Vertical anchor (object with ONE key): `absolute` (y), `above_bottom` (min_gen_y + o),
`below_top` (min_gen_y + gen_depth − 1 − o), `relative_to_sea_level` (sea_level + o, NEW
26.3). Context: `min_gen_y = max(world min_y, generator min_y)`,
`gen_depth = min(world height, generator depth)`.

| Type (`height.type`) | Draw (resolved min, max) |
|---|---|
| lone anchor (no `type`) or `constant` (`value`) | the value |
| `uniform` | if min > max: min (warning); otherwise `min + next_int(max−min+1)` |
| `biased_to_bottom` (`inner`=1) | if max−min−inner+1 ≤ 0: min; otherwise a = next_int(max−min−inner+1); `min + next_int(a + inner)` |
| `very_biased_to_bottom` (`inner`=1) | same guard; a = between(min+inner, max); b = between(min, a−1); `between(min, b−1+inner)` where between(u, v) = u if u ≥ v otherwise u + next_int(v−u+1) |
| `trapezoid` (`plateau`=0) | if min > max: min; r = max−min; if plateau ≥ r: uniform; otherwise d = (r−plateau)/2 (integer), e = r − d; `min + U(0, e) + U(0, d)` with U(a,b) = a + next_int(b−a+1), drawn in that order |
| `weighted_list` (`distribution`) | weighted choice (§ 3.6) then draw from the chosen provider |

### 3.6 Value providers

Integers (`type`; a bare integer = constant):

| Type | Draw |
|---|---|
| `constant` (`value`) | value |
| `uniform` (`min_inclusive`, `max_inclusive`) | `min + next_int(max−min+1)` |
| `biased_to_bottom` | `min + next_int(next_int(max−min+1) + 1)` |
| `very_biased_to_bottom` | `min + next_int(next_int(next_int(max−min+1) + 1) + 1)` |
| `clamped` (`source`, bounds) | clamp(draw of source) |
| `clamped_normal` (`mean`, `deviation`, bounds) | `(int) clamp(mean + (float)next_gaussian()·deviation)` (computed in single-precision float) |
| `weighted_list` (`distribution`) | weighted choice then draw |
| `trapezoid` (`min`, `max`, `plateau`) | if plateau = 0 and max = −min: `next_int(max+1) − next_int(max+1)`; otherwise if plateau = max−min: uniform; otherwise like the trapezoid height |

Floats: `constant`, `uniform` (`min_inclusive`, `max_exclusive`: `next_float()·(max−min)+min`),
`clamped_normal` (`mean`, `deviation`, `min`, `max`), `trapezoid` (`min`, `max`, `plateau`:
d = (r−plateau)/2, e = r−d, `min + next_float()·e + next_float()·d`).

Weighted list (`[{data, weight}]`): total = sum of the weights; t = `next_int(total)`; the first
element (list order) whose cumulative weight exceeds t is returned (zero weights
ignored). Drawing an element from a list = `list[next_int(len)]`. Randomly drawn tag set
= `next_int(size)` in the set's order (order of the resolved tag file).

## 4. Predicates

### 4.1 Block predicates (`type`, used by filters and features)

`offset` (vector, default 0, components −16..16) shifts the tested position.

| Type | True if |
|---|---|
| `true` | always |
| `matching_blocks` (`blocks`: list/tag/identifier) | block at p+offset ∈ blocks |
| `matching_block_tag` (`tag`) | block ∈ tag |
| `matching_fluids` (`fluids`) | the block's FLUID state ∈ fluids |
| `matching_biomes` (`biomes`) | zoomed biome at p (offset ignored) ∈ biomes |
| `has_sturdy_face` (`direction`) | the `direction` face of the block at p+offset is full ("complete" support shape) |
| `solid` (obsolete) | the block is "solid" (legacy property of the state) |
| `replaceable` | the state is replaceable (tall grass, air, fluids, snow layer…) |
| `would_survive` (`state`) | `state` could stand at p+offset (the block's survival rule) |
| `inside_world_bounds` | p+offset within the buildable height |
| `unobstructed` | no entity blocks a full cube at p — **`offset` is read but IGNORED** (the cube is always at p); in generation, no entities: always true in practice |
| `height_range` (`min_inclusive`, `max_inclusive`: anchors) | min ≤ p.y ≤ max |
| `volume_match` (`min`, `max`, `match`) | `match` true for all the offsets of the box, iterating x then z then y, stopping at the first false |
| `all_of` / `any_of` (`predicates`) | conjunction / disjunction, short-circuit in order |
| `not` (`predicate`) | negation |

### 4.2 Rule tests (`predicate_type`, ores and processors)

Test (state, position, random):

| Type | True if |
|---|---|
| `always_true` | always |
| `block_match` (`block`) | same block |
| `blockstate_match` (`block_state`) | same state |
| `tag_match` (`tag`) | block ∈ tag |
| `random_block_match` (`block`, `probability`) | same block AND `next_float() < p` (draw only if the block matches) |
| `random_blockstate_match` (`block_state`, `probability`) | same on the state |
| `height_match` (`min_inclusive`, `max_inclusive`) | min ≤ y ≤ max (NEW 26.3, serves the ores) |
| `all_of` / `any_of` (`rules`), `not` (`rule`) | combinations, short-circuit |

Position tests (`predicate_type`, processors only): `always_true`;
`linear_pos` (`min_chance`, `max_chance`, `min_dist`, `max_dist`): d = Manhattan distance
(world position, reference); true if `next_float() ≤ clamped_lerp(inv_lerp(d, min_dist,
max_dist), min_chance, max_chance)`; `axis_aligned_linear_pos` (+ `axis`, default y): d =
|difference on the axis| (integer), same formula. The draw is ALWAYS consumed.

### 4.3 Block-state providers

A provider is written either as a bare state `{"id": …, "properties": {…}}`, or
`{"type": …}`, or as an identifier of the registry `worldgen/block_state_provider/`
(e.g. `"flower_plain"`, `"soil_beneath_tree"`).

| Type | State at p |
|---|---|
| bare state / `simple` (`state`) | the state |
| `weighted` (weighted list of states) | weighted choice |
| `random_block` (`blocks`) | default state of a drawn block (`next_int(size)`); empty set → current block at p (or "none") |
| `randomized_int` (`source`, `property`, `values`) | state of the source then named integer property = draw from `values` (if the property does not exist: state unchanged, no draw) |
| `rotated` (`state`, optional `direction`) | direction = given, otherwise drawn (`next_int(6)` in the order down, up, north, south, west, east) BEFORE the source state; then `axis` = axis, `facing` = direction, and if horizontal `horizontal_facing`… each property only if it exists |
| `copy_properties` (`source`) | state of the source onto which the common properties of the current state at p are copied |
| `rule_based` (`rules` [{`if_true`, `then`}], optional `fallback`) | first rule whose predicate is true AND whose provider gives a state; otherwise fallback; otherwise "none" (and for the mandatory variant: current block) |
| `noise` (`seed`, `noise`, `scale`, `states`) | v = normal noise (congruential seed `seed`) at (x·scale, y·scale, z·scale); t = clamp((1+v)/2, 0, 0.9999); `states[(int)(t·len)]` |
| `noise_threshold` (+ `threshold`, `high_chance`, `default_state`, `low_states`, `high_states`) | v < threshold → `low_states[next_int]`; otherwise `next_float() < high_chance` → `high_states[next_int]`, otherwise default |
| `dual_noise` (+ `variety` [a, b], `slow_noise`, `slow_scale`) | w = slow noise; k = (int) clamped_map(w, −1, 1, a, b+1); candidates = for i < k: state chosen by the slow noise at p + (54545·i, 0, 34234·i); then choice among the candidates by the main noise (formula `noise`) |

The providers' noises are normal noises built with a generation generator
wrapping a congruential one of seed `seed` (fixed, independent of the world seed).

## 5. Features: common framework and trees

### 5.0 Common framework

A configured feature (`worldgen/feature/**.json`, sub-folders allowed, e.g. `coral/…`) is an
object `{"type": <type>, …fields}`; wherever a feature is expected one can give an
identifier of this registry or an inline object. A nested placed feature
(`{"feature": …, "placement": […]}`) is placed WITHOUT a biome check (§ 3.1): the `biome`
modifier is forbidden there.

Reading/writing: all reads go through the region (§ 1). Unless stated otherwise, a placement
uses flag 2 ("write"). "Place and update" = flag 3. Trees place
with 19 (= 16 + 2 + 1: without automatic post-processing).

**Java hash sets.** Several features (trees, fallen trunks) accumulate
positions in a hash set then iterate it while consuming randomness: the iteration
order is part of the result. The Java hash set must be reproduced:
- hash of a position = `(y + z·31)·31 + x` (32-bit integers);
- spreading: `h' = h XOR (h >>> 16)`; bucket = `h' AND (capacity − 1)`;
- initial capacity 16, threshold 0.75: when the size exceeds `0.75 × capacity` AFTER an
  insertion, the capacity doubles (splitting a bucket keeps the relative order);
- iteration: buckets by increasing index; within a bucket, insertion order (buckets
  turned into trees — 8 elements and capacity ≥ 64 — do not happen in practice;
  to be flagged if a verification fails);
- re-inserting a position already present changes nothing.
The "by y" sort that follows (§ 5.1.6) is STABLE: at equal y, the hash order is kept.

### 5.1 `tree` — trees

Fields: `trunk_provider`, `trunk_placer`, `foliage_provider`, `foliage_placer`,
`root_placer` (optional), `minimum_size`, `decorators`, `ignore_vines` (false),
`below_trunk_provider` (e.g. `soil_beneath_tree`: dirt if the block is not in
`cannot_replace_below_tree_trunk`).

"Valid tree position" = air or a block of tag `replaceable_by_trees` (leaves,
small flowers, grasses, water, vines, litter…). "Free" (for trunks) = valid or log
(`logs`).

#### 5.1.1 General algorithm

```
H = trunk_placer.height(random)                 base + next_int(a+1) + next_int(b+1)
hf = foliage_placer.foliage_height(random, H)
rf = foliage_placer.radius(random, H − hf)
T = root_placer ? origin + (0, trunk_offset_y.sample(), 0) : origin
y_min = min(origin.y, T.y) ; y_max = max(origin.y, T.y) + H + 1
if y_min < min_y+1 or y_max > max_y+1: failure       (max_y = last buildable y)
Hc = free_height(H, T)
if Hc < H and (minimum_size without min_clipped_height or Hc < min_clipped_height): failure
if root_placer and not place_roots(…): failure
attachments = trunk_placer.place_trunk(random, Hc, T)
for each attachment, in order: foliage_placer.foliage(random, Hc, attachment, hf, rf)
if no trunk or leaf placed: failure
decorators, in list order (§ 5.1.6)
recompute leaf distances and shape updates (§ 5.1.7)
```

`free_height(H, T)`: for y = 0..H+1, r = size(H, y); for x = −r..r, z = −r..r: if
position T+(x, y, z) is not free, or if `ignore_vines` is false and there is a vine
→ return y − 2. Otherwise return H. Sizes (`minimum_size.type`):
- `two_layers_feature_size` (`limit` 1, `lower_size` 0, `upper_size` 1): y < limit ? lower : upper;
- `three_layers_feature_size` (`limit` 1, `upper_limit` 1, `lower_size` 0, `middle_size` 1,
  `upper_size` 1): y < limit ? lower : (y ≥ H − upper_limit ? upper : middle);
- both accept `min_clipped_height` (0..80).

Placing a log ("place_log"): if the position is a valid tree position (for the
trunk with ascending branches: or a block of `can_grow_through`), placement of
`trunk_provider.state(p)` (possibly modified: forced axis…) and addition to the set of
trunks. The block under the trunk: `below_trunk_provider.optional_state(p)` placed if it exists
(added to the set of trunks as well!).

#### 5.1.2 Trunk placers (`trunk_placer.type`)

Common fields: `base_height` (0..32), `height_rand_a`, `height_rand_b` (0..24).
"Drawn horizontal direction" = `[north, east, south, west][next_int(4)]`.

| Type | Algorithm |
|---|---|
| `straight_trunk_placer` | below-trunk at T−1; logs T+(0,y,0), y = 0..H−1; attachment (T+(0,H,0), radius +0, simple) |
| `forking_trunk_placer` | below-trunk; d = drawn direction; hp = H − next_int(4) − 1; np = 3 − next_int(3); for y = 0..H−1: if y ≥ hp and np > 0: shift (x,z) by one step d, np−−; log; if placed, end = y+1. Attachment (x, end, z) radius +1 if a log was placed. Then d2 = drawn direction; if d2 ≠ d: b = hp − next_int(2) − 1; n = 1 + next_int(3); restart from (T.x, T.z); for y = b while y < H and n > 0 (n−− each turn): if y ≥ 1: step d2 then log (end = y+1 if placed); y++. Attachment (x, end, z) radius 0 if a log was placed |
| `giant_trunk_placer` | below-trunk at T−1, T−1+east, T−1+south, T−1+south+east; for y = 0..H−1: "if free" log at (0,y,0); if y < H−1: also (1,y,0), (1,y,1), (0,y,1) in that order. Attachment (T+(0,H,0), 0, double) |
| `mega_jungle_trunk_placer` | like `giant`; then for hb = H − 2 − next_int(4); while hb > H/2; hb −= 2 + next_int(4): a = next_float()·2π; for b = 0..4: bx = (int)(1.5 + cos(a)·b), bz = (int)(1.5 + sin(a)·b) (sin/cos = the game's TABLE, § 5.1.8); log at T+(bx, hb − 3 + b/2, bz). Attachment (T+(bx, hb, bz), radius −2, simple) |
| `dark_oak_trunk_placer` | 4 below-trunks; d = drawn; hp = H − next_int(4); np = 2 − next_int(3); ey = T.y + H − 1; for dy = 0..H−1: shift as forking; if the block (tx, y, tz) is air or leaves: logs at (tx,y,tz), +east, +south, +east+south. Attachment (tx, ey, tz, 0, double). Then for ox = −1..2, oz = −1..2: if (ox,oz) outside the square 0..1 AND next_int(3) ≤ 0: L = next_int(3) + 2; logs at (T.x+ox, ey − k − 1, T.z+oz) for k < L; attachment (T.x+ox, ey, T.z+oz, 0, simple) |
| `fancy_trunk_placer` | § 5.1.3 |
| `bending_trunk_placer` (`min_height_for_leaves` 1, `bend_length`) | d = drawn; below-trunk; p = T; for i = 0..H−1 (inclusive, H−1 = "logH"): if i + 1 ≥ (H−1) + next_int(2): p += d; if p valid: log; if i ≥ min_height: attachment (p, 0); p += up. Then L = bend_length.sample(); for i = 0..L: log if valid; attachment (p, 0); p += d |
| `upwards_branching_trunk_placer` (`extra_branch_steps`, `place_branch_per_log_probability`, `extra_branch_length`, `can_grow_through`) | for h = 0..H−1: if log placed at T+(0,h,0) AND h < H−1 AND next_float() < probability: d = drawn; L = extra_branch_length.sample(); bp = max(0, L − extra_branch_length.sample() − 1); n = extra_branch_steps.sample(); branch (below). If h = H−1: attachment (T+(0,h+1,0), 0). Branch: ha = y+bp; for i = bp while i < H and n > 0 (i++, n−−): if i ≥ 1: step d, y' = y+i, ha = y'; log at (x, y', z) → if placed ha = y'+1; attachment (position, 0). If ha − y > 1: attachments (x, ha, z) and (x, ha−2, z). No below-trunk |
| `cherry_trunk_placer` (`branch_count` 1..3, `branch_horizontal_length` 2..16, `branch_start_offset_from_top` uniform [a,b] b>a, `branch_end_offset_from_top`) | § 5.1.3 |
| `poplar_trunk_placer` (`trunk_height_above_branches` 0..8, `branch_amount` 1..4) | below-trunk; hb = H − trunk_height_above_branches.sample(); for y = 0..H−1: log T+(0,y,0); dirs = shuffle of the 6 directions (§ 5.1.8) without the vertical ones — drawn at EACH y; if y = hb−1: n = branch_amount.sample(); for k < n: log at T+(0,y,0)+dirs[k] with axis = axis of dirs[k]. Attachment (T+(0,hb,0), 0) |

"If free" log = `free(p)` then placement (with the usual validity test).

#### 5.1.3 "Fancy" trunks and cherry

**Fancy** (large oak):
```
h = H + 2 ; ht = floor(h · 0.618)
below-trunk
clusters_per_y = min(1, floor(1.382 + (h/13)²))        ← always equals 1 (quirk)
trunk_top = T.y + ht
coords = [ (T + (0, h−5, 0), base = trunk_top) ]
for ry = h − 5 down to 0:
    f = shape(h, ry) ; if f < 0: continue
    once: r = 1.0 · f · (next_float() + 0.328) ; a = next_float() · 2 · π (double)
          x = r·sin(a) + 0.5 ; z = r·cos(a) + 0.5        (EXACT sin/cos in double, standard library)
          start = T + (floor(x), ry − 1, floor(z)) ; end = start + (0,5,0)
          if limb(start, end, test only):
              dx = T.x − start.x ; dz = T.z − start.z
              hb = start.y − sqrt(dx²+dz²)·0.381 ; base = hb > trunk_top ? trunk_top : (int)hb
              if limb((T.x, base, T.z), start, test only): coords += (start, base)
limb(T, T + (0, ht, 0), PLACE)
for each coord (in order): if base ≠ … : if (T.x, base, T.z) ≠ coord.pos and
    base − T.y ≥ h·0.2: limb((T.x, base, T.z), coord.pos, PLACE)
attachments = coords whose base − T.y ≥ h·0.2 (in order), radius 0, simple
```
`shape(h, y)` (single-precision floats): if y < h·0.3 → −1; R = h/2; a = R − y; if a = 0: d = R;
otherwise if |a| ≥ R → 0; otherwise d = sqrt(R² − a²); return d · 0.5.
`limb(A, B, mode)`: if test only and A = B → true; Δ = B − A; n = max(|Δx|,|Δy|,|Δz|);
step = Δ/n as a single-precision float; for i = 0..n: p = A + (floor(0.5 + i·step.x), …); in PLACE:
log with axis = x if |Δx from p to A| is the non-zero horizontal max (x takes priority on a tie),
z otherwise, y if no horizontal difference; in test: if not free → false.

**Cherry**:
```
below-trunk
b1 = max(0, H − 1 + start.sample())              start = uniform [a, b]
b2 = max(0, H − 1 + start2.sample())             start2 = uniform [a, b−1]
if b2 ≥ b1: b2 += 1
n = branch_count.sample() ; middle = (n = 3) ; two = (n ≥ 2)
ht = middle ? H : (two ? max(b1, b2) + 1 : b1 + 1)
logs T+(0,y,0), y < ht
if middle: attachment (T + (0, ht, 0))
d = drawn direction
attachment += branch(d, b1, b1 < ht − 1)
if two: attachment += branch(opposite(d), b2, b2 < ht − 1)
branch(d, b, continues):
    p = T + (0, b, 0)
    e = H − 1 + end.sample()
    far = continues or e < b
    L = branch_horizontal_length.sample() + (far ? 1 : 0)
    end_pos = T + d·L + (0, e, 0)
    repeat (far ? 2 : 1) times: p += d ; sideways log (axis of d)
    v = end_pos.y > p.y ? up : down
    loop: dist = Manhattan(p, end_pos) ; if 0 → attachment (end_pos + (0,1,0)), end
          c = |end_pos.y − p.y| / dist (float) ; vertical = next_float() < c
          p += vertical ? v : d ; log (sideways if horizontal)
```

#### 5.1.4 Foliage placers (`foliage_placer.type`)

Common fields `radius`, `offset` (providers 0..16). `foliage(…)` draws D = `offset` then
calls the variant. Attachment = (position, radius offset `ro`, height offset `ho`
(always 0 in the current data), double = 2×2 trunk).

**Leaf row** `row(center, r, y, double)`: s = double ? 1 : 0; for dx = −r..r+s,
then dz = −r..r+s (dx outer loop): if not "skip_signed(dx, y, dz, r, double)":
try a leaf at center + (dx, y, dz).
`skip_signed`: if double, mx = min(|dx|, |dx−1|), mz likewise, otherwise |dx|, |dz|; then
`skip(mx, y, mz, r)` specific to each type.
**Leaf**: if the current block does not have `persistent=true` and is a valid tree position:
state = `foliage_provider.state(p)`; if it has `waterlogged`, = (current block is a water SOURCE);
placement, and addition to the set of leaves.

| Type | Height hf / radius | Construction (D = drawn offset) | `skip(dx, y, dz, r)` |
|---|---|---|---|
| `blob_foliage_placer` (`height`) | hf = height | for yo = D down to D − (hf+ho): r' = max(rf + ro − 1 − yo/2, 0) (integer division towards zero); row(pos, r', yo) | dx = r and dz = r and (next_int(2) = 0 or y = 0) — see the note below the table |
| `bush_foliage_placer` (`height`) | hf = height | yo from D to D − hf: r' = rf + ro − 1 − yo; row | dx = r and dz = r and next_int(2) = 0 |
| `fancy_foliage_placer` (`height`) | hf = height | yo from D to D − hf: r' = rf + (yo ≠ D and yo ≠ D − hf ? 1 : 0); row | (dx+0.5)² + (dz+0.5)² > r² (single-precision float) |
| `spruce_foliage_placer` (`trunk_height`) | hf = max(4, H − trunk_height.sample()) | c = next_int(2); max = 1; min = 0; for yo = D down to −(hf+ho): row(pos, c, yo); if c ≥ max: c = min, min = 1, max = min(max+1, rf+ro); otherwise c++ | dx = r and dz = r and r > 0 |
| `pine_foliage_placer` (`height`) | hf = height.sample(); rf = radius.sample() + next_int(max(trunk+1, 1)) | c = 0; yo from D to D − hf: row(c); if c ≥ 1 and yo = D − hf + 1: c−−; otherwise if c < rf + ro: c++ | dx = r and dz = r and r > 0 |
| `mega_pine_foliage_placer` (`crown_height`) | hf = crown_height.sample() | prev = 0; for yy = pos.y − hf + D .. pos.y + D: yo = pos.y − yy; s = rf + ro + floor(yo / hf · 3.5) (float); j = (yo > 0 and s = prev and yy even) ? s+1 : s; row((pos.x, yy, pos.z), j, 0); prev = s | dx+dz ≥ 7 or dx²+dz² > r² |
| `jungle_foliage_placer` (`height`) | hf = height | lh = (double ? hf : 1 + next_int(2)); yo from D to D − lh: r' = rf + ro + 1 − yo; row | dx+dz ≥ 7 or dx²+dz² > r² |
| `acacia_foliage_placer` | hf = 0 | c = pos + (0,D,0); row(c, rf+ro, −1−hf); row(c, rf−1, −hf); row(c, rf+ro−1, 0) | y = 0: (dx > 1 or dz > 1) and dx ≠ 0 and dz ≠ 0; otherwise dx = r and dz = r and r > 0 |
| `dark_oak_foliage_placer` | hf = 4 | c = pos + (0,D,0); double: rows (rf+2, −1), (rf+3, 0), (rf+2, 1), then if next_boolean(): (rf, 2); single: (rf+2, −1), (rf+1, 0) | see below |
| `cherry_foliage_placer` (`height` 4..16, `wide_bottom_layer_hole_chance`, `corner_hole_chance`, `hanging_leaves_chance`, `hanging_leaves_extension_chance`) | hf = height.sample() | c = pos+(0,D,0); r = rf + ro − 1; rows (r−2, hf−3), (r−1, hf−4), then (r, y) for y = hf−5 down to 0; then hanging row (r, −1), hanging row (r−1, −2) | below |
| `random_spread_foliage_placer` (`foliage_height` 1..512, `leaf_placement_attempts` 0..256) | hf = foliage_height.sample() | repeat N times: p = pos + (next_int(rf) − next_int(rf), next_int(hf) − next_int(hf), next_int(rf) − next_int(rf)); leaf | never |
| `poplar_foliage_placer` (`height` 5..16, `side_hole_chance`) | hf = height.sample() | below | below |

Note on `blob`: the condition is "dx = r AND dz = r AND (next_int(2) = 0 OR y = 0)" with
lazy left-to-right evaluation: the `next_int(2)` draw happens ONLY for corners
(dx = r and dz = r), and it happens even if y = 0. Same logic for `bush` (draw at corners).

`dark_oak`: special `skip_signed`: if y = 0 and double and (dx = −r or dx ≥ r) and (dz = −r or
dz ≥ r) → skip; otherwise normal computation. `skip`: y = −1 and not double → dx = r and dz = r;
y = 1 → dx + dz > 2r − 2; otherwise never.

`cherry`, `skip(dx, y, dz, r)` (in this order): if y = −1 and (dx = r or dz = r) and
next_float() < bottom_hole → skip. Then corner = (dx = r and dz = r); if r > 2: corner OR
(dx + dz > 2r − 2 AND next_float() < corner_hole); otherwise corner AND next_float() < corner_hole.
**Hanging row** (r, y): normal row, then for each horizontal direction e in
the order north, east, south, west: t = clockwise(e); dist = (t positive ? r + s : r); p = c +
(0, y−1, 0) + t·dist − e·r; for k = −r .. r+s−1: if a leaf was placed BY THIS TREE
at p + up: if extension(p, hanging_chance): extension(p − 1 in y, extension_chance);
p += e. `extension(p, ch)`: if Manhattan(p, c − (0,1,0)) ≥ 7 → no; otherwise if next_float() > ch
→ no; otherwise try a leaf at p (result = placed).

`poplar`: c = pos + (0, D, 0); r = rf + ro − 1; flipped = next_boolean(); special
rows (r−2, hf−1), (r−1, hf−2), (r−1, hf−3), then (r, y) for y = hf−4 down to 1; then
"inner logs" at level hf−4; then rows (r−1, 0) and (clamp(r−2, 1, 2), −1).
Special row: for dx = −r..r+s, dz = −r..r+s: partial = (y = hf−1 or y = hf−2);
small_corner = flipped ? ((dx>0 and dz>0) or (dx<0 and dz<0)) : ((dx>0 and dz<0) or (dx<0 and dz>0));
k = small_corner ? r−1 : (partial ? r+1 : r); edge = |dx| = r or |dz| = r; if partial and edge →
skip (no draw); otherwise hole = next_float() ≤ side_hole_chance ? 1 : 0; skip if
|dx| + |dz| > 2r − (k + hole). Inner logs: same loops; if |dx|+|dz| ≤ 2r − (k + 2)
and ((dz = 0 and r − |dx| ≥ 4) or (dx = 0 and r − |dz| ≥ 4)): if the current block EQUALS
`foliage_provider.state(p)` (one draw per test) then `trunk_provider.state(p)` is placed there with
axis x if dz = 0, z otherwise (and the position is counted as a LEAF).

#### 5.1.5 Root placer `mangrove_root_placer`

Fields: `trunk_offset_y`, `root_provider`, optional `above_root_placement`
(`above_root_provider`, `above_root_placement_chance`), `mangrove_root_placement`
(`can_grow_through`, `muddy_roots_in`, `muddy_roots_provider`, `max_root_width` 1..12,
`max_root_length` 1..64, `random_skew_chance`).

```
place_roots(origin, T):
    for y from origin.y to T.y − 1: if not root_possible(column) → failure
    list = [T − (0,1,0)]
    for d in north, east, south, west:
        L = [] ; if not simulate(T + d, d, T, L, 0) → failure
        list += L ; list += [T + d]
    for each p of list: place_root(p)
simulate(p, d, T, L, layer):
    if layer = max_length or |L| > max_length → false
    for q in candidates(p, d, T): if root_possible(q): L += q ; if not simulate(q, d, T, L, layer+1) → false
    true
candidates(p, d, T): w = Manhattan(p, T) ; W = max_root_width ; skew = random_skew_chance
    if W−3 < w ≤ W: next_float() < skew ? [p−y, p+d−y] : [p−y]
    otherwise if w > W: [p−y]
    otherwise if next_float() < skew: [p−y]
    otherwise next_boolean() ? [p+d] : [p−y]
root_possible(p) = valid tree position or block of can_grow_through
place_root(p): if p ∈ muddy_roots_in: muddy_roots_provider, waterlogged if water fluid;
    otherwise if root_possible: root_provider (waterlogged according to the fluid); then if above_root:
    if next_float() < chance and block above = air: above_root_provider above.
```

#### 5.1.6 Decorators (`decorators[].type`)

Context: lists `logs`, `leaves`, `roots` = copies of the hash sets (§ 5.0)
STABLY sorted by increasing y. Placing a decoration: flag 19 + addition to the set of
decorations. "Bottom" = `roots` if non-empty (and if `logs` is non-empty and the first root at the same y as the
first log: `logs` then `roots`), otherwise `logs`.

| Type | Algorithm |
|---|---|
| `trunk_vine` | for each log: for (west→vine facing east, east→west, north→south, south→north) in this order: if next_int(3) > 0 and neighbour = air: vine |
| `leave_vine` (`probability`) | for each leaf, same 4 sides: if next_float() < p and neighbour = air: vine + up to 4 vines below as long as air |
| `cocoa` (`probability`) | if next_float() < p: y0 = y of the 1st log; for each log with y − y0 ≤ 2: for d in north, east, south, west: if next_float() ≤ 0.25: c = log − d (opposite side); if air: cocoa age next_int(3), facing d |
| `beehive` (`probability`) | if logs empty → nothing; if next_float() ≥ p → nothing; yh = leaves non-empty ? max(y of 1st leaf − 1, y of 1st log + 1) : min(y of 1st log + 1 + next_int(3), y of last log); cand = for each log at y = yh (sorted order), its neighbours east, south, west (see note); shuffle (§ 5.1.8); first cand with air AND air to its south → bee nest facing south; bees: 2 + next_int(2) occupants, each with time next_int(599) |
| `alter_ground` (`provider`) | bottom; ymin = y of the first; for each p of bottom at y = ymin: circles at p+(−1,0,−1), p+(2,0,−1), p+(−1,0,2), p+(2,0,2); then 5 times: k = next_int(64); x = k mod 8, z = k / 8; if x ∈ {0,7} or z ∈ {0,7}: circle at p + (−3+x, 0, −3+z). Circle = 5×5 disk without corners; for each column, dy from 2 to −3: state = provider.optional; if it exists → placement, column done; otherwise if not air and dy < 0 → column done |
| `attached_to_leaves` (`probability`, `exclusion_radius_xz`, `exclusion_radius_y`, `block_provider`, `required_empty_blocks`, `directions`) | for each leaf of a shuffle of the leaves list: d = directions[next_int]; q = leaf + d; if q not excluded and next_float() < p and the `required` blocks in direction d are air: exclude the box around q, place provider.state(q) |
| `attached_to_logs` (`probability`, `block_provider`, `directions`) | for each log of a shuffle: d drawn; q = log + d; if next_float() ≤ p and q air: placement |
| `place_on_ground` (`tries` 128, `radius` 2, `height` 1, `block_state_provider`) | bottom; box of the bottom positions at the y of the first, inflated (r, h, r); tries times: p = (between(x_min,x_max), between(y_min,y_max), between(z_min,z_max)) drawn x then y then z; if above = air or vine, p "made solid" (full opaque) and `MOTION_BLOCKING_NO_LEAVES` height at p ≤ p.y+1: placement above |
| `pale_moss` (`leaves_probability`, `trunk_probability`, `ground_probability`) | L = shuffle of the logs (draws!); if empty → nothing; o = first log with the smallest y IN L; if next_float() < ground: place the feature `pale_moss_patch` at o + up (same random source); for each log (sorted order): if next_float() < trunk and below is air: hanging moss; same for leaves with leaves. Hanging moss at q: while air under q AND NOT (next_float() < 0.5): place moss (tip=false), go down; place moss tip=true |
| `creaking_heart` (`probability`) | logs non-empty and next_float() < p: shuffled copy; first log surrounded by logs on its 6 faces → creaking heart (dormant, natural) |
| `shelf_mushroom` (`probability`) | if next_float() ≥ p → nothing. If all the logs are at the same y (fallen trunk): dirs = x axis ? (north, south) : (east, west); for each log, each dir: if next_float() ≤ 0.25: m = log + dir; if replaceable, with no water at m nor on its 4 sides, and no shelf mushroom horizontally adjacent to m or to the log: placement (age next_int(2), facing dir). Otherwise (upright): d = drawn; dirs = (d, clockwise(d)); for each log with 1 ≤ y − y0 ≤ 4: for each dir: if next_float() ≤ 0.25 and m replaceable without water and no mushroom under m: placement, and move on to the next log |

Note `beehive`: the candidate directions are the horizontal ones except NORTH (the opposite of
the south orientation), in the order east, south, west… exactly: `[north, east, south, west]` minus
north = `[east, south, west]`, and the position must have air to the SOUTH of it.

#### 5.1.7 Leaf recomputation

Box = bounding box of all the positions (roots, logs, leaves, decorations). Voxel shape
F (bits) over the box: mark decorations and roots. Distance buckets 0..6; bucket 0 =
all the logs. While a non-empty bucket of smallest index d < 7 remains: remove one
element p (bucket order = hash set); if p is in the box: if d ≠ 0, re-place the block
at p with `distance = d`; mark F; for each direction (down, up, north, south, west, east)
: neighbour q in the box and not marked: if q has a `distance` property (leaves): nd =
min(current distance, d+1); if nd < 7: add q to bucket nd. Then, for each face of F
facing an unmarked cell, shape update of the two blocks (like a neighbour update, flag 2)
— this is what connects vines, fences, etc. The result is independent of the bucket order.

#### 5.1.8 Utilities

- `sin`/`cos` "table": table of 65536 single-precision floats `sin(i·2π/65536)`; sin(x) =
  T[(long)(x·10430.378350470453) AND 65535]; cos(x) = T[(long)(x·10430.378… + 16384) AND
  65535]. Not to be confused with the exact double sine used by the fancy trunk.
- List shuffle (`shuffle`): for i = size down to 2: swap [i−1] and
  [next_int(i)]. Order of the 6 directions before shuffling: down, up, north, south, west, east.

### 5.2 `fallen_tree` — fallen trunk

Fields: `trunk_provider`, `log_length` (0..16), `stump_decorators`, `log_decorators`.
```
stump: place_and_update(origin, trunk_provider.state) ; mark above (2 non-air
       blocks) for post-processing ; stump decorators with logs = {origin}
d = drawn direction ; L = log_length.sample() − 2
s = origin + d·(2 + next_int(2))
s += up ; up to 6 times: if s valid and load-bearing ground under s → stop ; otherwise s −= up
check: for i < L: s+i·d must be a valid tree position ; gap = number of consecutive
       positions WITHOUT a load-bearing top face below ; if gap > 2 → abort
if check OK: sideways logs (axis of d) at s + i·d, i < L, each "placed and updated"
             + marking above ; trunk decorators with logs = this set (§ 5.0)
always returns true
```

## 6. Features: complete list of the 26.3 types

Type registry (59): `bamboo`, `block_blob`, `block_column`, `block_pile`, `blue_ice`,
`bonus_chest`, `chorus_plant`, `coral_claw`, `coral_tree`, `delta_feature`, `disk`,
`end_gateway`, `end_island`, `end_platform`, `end_podium`, `end_spike`, `fallen_tree`,
`fill_layer`, `fossil`, `freeze_top_layer`, `geode`, `huge_brown_mushroom`, `huge_fungus`,
`huge_red_mushroom`, `iceberg`, `lake`, `large_dripstone`, `monster_room`,
`multiface_growth`, `netherrack_replace_blobs`, `no_op`, `ore`, `overlay`,
`projected_random_patchy_square`, `random_boolean_selector`, `random_neighbor_spread`,
`random_selector`, `replace_single_block`, `root_system`, `scattered_ore`, `sculk_patch`,
`sequence`, `simple_block`, `simple_random_selector`, `single_block_pillar`, `speleothem`,
`speleothem_cluster`, `spike`, `spring_feature`, `stepped_column_cluster`, `template`,
`tree`, `underwater_magma`, `vegetation_patch`, `vines`, `void_start_platform`,
`waterlogged_vegetation_patch`, `weighted_random_selector`.

Notable changes compared with earlier versions (for those who know the old
format): no more "random patch" — tufts of flowers/grasses are a simple feature
preceded by `count` + `offset` + a filter in the placed feature; corals are described in data
(`overlay` + `cuboid`); basalt columns, speleothems (dripstone AND sulfur),
Nether glowstone are generic; poplars, sulfur and fallen trunks appear.

### 6.1 Composition

| Type | Fields | Algorithm |
|---|---|---|
| `no_op` | — | nothing, returns true |
| `overlay` | `features` (list of placed features) | places each placed feature from the origin (§ 3.1, without biome check), in order; true if at least one placed something |
| `sequence` | `features` | same but stops and returns false at the first placed feature that placed nothing |
| `random_selector` (obsolete) | `features` [{`chance`, `feature`}], `default` | for each entry in order: if `next_float() < chance` → place this entry, return its result. Otherwise place `default` |
| `simple_random_selector` | `features` | `features[next_int(n)]` |
| `random_boolean_selector` | `feature_true`, `feature_false` | `next_boolean()` chooses |
| `weighted_random_selector` | `features` (weighted list `{data, weight}`) | weighted choice (§ 3.6); empty list → false |

### 6.2 Simple blocks and small shapes

| Type | Fields | Algorithm |
|---|---|---|
| `simple_block` | `to_place` (provider), `schedule_tick` (false) | state = optional provider at p; none → false. If the state cannot survive at p → false. Double plant (2 blocks): if the block above is not air and (fluid different from the state's OR not replaceable) → false; otherwise place bottom + top (each half waterlogged if water at its position). Pale moss carpet: placed with its own algorithm (sides drawn with the REGION'S RANDOM, not the feature's). Otherwise simple placement. If `schedule_tick`: tick scheduled in 1 tick for the placed block |
| `block_column` | `layers` [{`height`, `provider`}], `direction`, `allowed_placement`, `prioritize_tip` | heights drawn layer by layer; total 0 → false. Test: q = p + dir; for i < total: if `allowed(q)` is false → truncate to i, stop; q += dir. Truncation: remove (total − i) blocks starting from the FIRST layer if `prioritize_tip`, from the last otherwise. Then placement of the layers in order from p (p included), one state drawn per block |
| `single_block_pillar` | `block`, `can_replace` (true), `direction` (up/down), `chance_to_continue` (1), optional `cap_feature` | q = p; while `can_replace(q)` AND `next_float() < chance` AND q within the height: place, q += dir. Then q −= dir and place `cap_feature` at q if it exists (even if nothing was placed). True |
| `block_pile` | `state_provider` | if p.y < min_y+5 → false. rx = 2 + next_int(2), rz = 2 + next_int(2); for each q of the box p+(−rx,0,−rz)..p+(rx,1,rz) (order x, then y, then z — x varies fastest): if dx²+dz² ≤ next_float()·10 − next_float()·6 → attempt; otherwise if next_float() < 0.031 → attempt. Attempt: q empty AND (below = dirt path ? next_boolean() : top face of below load-bearing) → placement (flag 260) |
| `block_blob` | `state`, `can_place_on` | go down p while p.y > min_y+3 and not `can_place_on(p−y)`; if p.y ≤ min_y+3 → false. 3 times: rx, ry, rz = next_int(2) each; t = (rx+ry+rz)·0.333 + 0.5; for q of the box ±(rx,ry,rz) (order x,y,z): if dist²(q,p) ≤ t² → place and update; then p += (−1 + next_int(2), −next_int(2), −1 + next_int(2)) |
| `bamboo` | `probability` | if p empty and bamboo can survive: h = next_int(12) + 5; if next_float() < probability: r = next_int(4) + 1; for x from −r to r, z from −r to r (x outer) with x²+z² ≤ r²: q = (x, WORLD_SURFACE(x,z) − 1, z); if q ∈ `beneath_bamboo_podzol_replaceable` → podzol. Then while i < h and q empty: bamboo (age 1, no leaves, stage 0), go up. If the height reached ≥ 3: at the top large leaves stage 1, then below large leaves, then small. True if p was empty |
| `vines` | — | if p not empty → false; for d in (up, north, south, west, east) — direction order without down: if a vine can attach to neighbour d → vine on face d, true |
| `chorus_plant` | — | if p empty and below ∈ `supports_chorus_plant`: plant (§ 6.2.1); otherwise false |
| `blue_ice` | — | if p.y > sea_level − 1 → false; if neither p nor below is water → false; if no neighbour (except below) is packed ice → false; blue ice at p; 200 times: dy = next_int(5) − next_int(6); k = 3 + (dy < 2 ? dy/2 : 0); if k ≥ 1: q = p + (next_int(k) − next_int(k), dy, next_int(k) − next_int(k)); if q ∈ {air, water, packed ice, ice} and one of the 6 neighbours of q is blue ice → blue ice at q |
| `fill_layer` | `height`, `state` | for dx = 0..15, dz = 0..15: q = (p.x+dx, min_y + height, p.z+dz); if air → placement |
| `replace_single_block` | `targets` [{`target` (rule test), `state`}] | first target true on the block at p → place `state`. True |
| `netherrack_replace_blobs` | `target`, `state`, `radius` (0..12) | p clamped in y to [min_y+1, max_y]; go down until the `target` block is found (while y > min_y+1); nothing → false. rx, ry, rz = radius drawn 3 times; M = max; "by Manhattan distance" traversal (§ 6.2.2) up to rx+ry+rz, stopping as soon as a position has a distance > M; each block equal to target → `state` |
| `random_neighbor_spread` | `block`, `accepted_neighbors`, `can_replace`, `attempts` (1..3000), `xz_offset`, `y_offset` | places block at p (unconditionally); N = attempts; N times: q = p + (xz, y, xz) drawn in this order; if `can_replace(q)`: count the neighbours (order down, up, north, south, west, east) ∈ accepted, stopping at 2; if exactly 1 → placement. True (Nether glowstone) |
| `projected_random_patchy_square` | `block`, `project_through`, `size` (1..16), `max_projection_height` | s = size; b = s² + 1; for dx = −s..s, dz = −s..s: if next_int(b) < b − |dx|·|dz|: q = p + (dx,0,dz); k = max; while `project_through(q − y)`: q −= y; k−−; if k ≤ 0 stop; optional state at q → placement. True |
| `void_start_platform` | — | if the chunk is more than 1 away (Chebyshev distance) from chunk (0,0) → true without doing anything. Centre c = (8, p.y + 3, 8); for z then x of the chunk: if max(|x−8|, |z−8|) ≤ 16: placement at (x, c.y, z) of stone, except at c itself: cobblestone. True |
| `freeze_top_layer` | — | for dx, dz = 0..15: t = (x, MOTION_BLOCKING(x,z), z), d = t − y; biome = zoomed biome at t; if freeze(d) → ice at d; if snow(t) → snow layer at t, and if the block at d has `snowy` → `snowy=true` |
| `bonus_chest` | — | xs = shuffle of the chunk's 16 x, zs = shuffle of the 16 z (both by "shuffled list" § 5.1.8, x first); for x of xs, z of zs: c = `MOTION_BLOCKING_NO_LEAVES` position at (x, z); if c empty or without collision: chest (loot table `chests/spawn_bonus_chest`, loot seed drawn), torches on the 4 horizontal sides (north, east, south, west) if they hold; true. Otherwise false |

Freeze/snow (biome at t): temperature(p) = biome temperature modified (`frozen`: see
below) then, if p.y > sea_level + 17: − (8·S(x/8, z/8) + p.y − (sea_level + 17)) ·
0.05 / 40, with S = 2D simplex congruential seed 1234 without offset. "Warm enough for rain"
= temperature ≥ 0.15. Freeze(d): not warm, d within the height, block light < 10 (always true
in generation), block = liquid water (fluid block, not a waterlogged block) → true. Snow(t): biome
with precipitation and not warm; t within the height; block = air or snow; snow survives at
t. `frozen` modifier: g = F(x·0.05, z·0.05)·7 + B(x·0.2, z·0.2); if g < 0.3 and
B(x·0.09, z·0.09) < 0.8 → 0.2; otherwise base temperature. F = stack of 3 simplexes (congruential
seed 3456, created in order) of frequencies 1; 0.5; 0.25 and amplitudes 1/7;
2/7; 4/7. B = § 3.4.

#### 6.2.1 Chorus plant

```
place chorus (connections computed from the neighbours) at p ; grow(p, p, 8, 0)
grow(c, origin_point, 8, depth):
    h = next_int(4) + 1 (+1 if depth = 0)
    for i < h: q = c + (0, i+1, 0) ; if one of the 4 horizontal neighbours of q is not empty → END
               place chorus at q, re-place chorus at q − y (connections)
    placed = false
    if depth < 4: n = next_int(4) (+1 if depth = 0)
        n times: d = drawn horizontal direction ; q = c + (0,h,0) + d
            if |q.x − origin_point.x| < 8 and |q.z − origin_point.z| < 8 and q empty and q − y empty and the
               horizontal neighbours of q other than −d are empty:
               placed = true ; chorus at q and at q − d ; grow(q, origin_point, 8, depth+1)
    if not placed: chorus flower age 5 at c + (0,h,0)
```

#### 6.2.2 Manhattan-distance traversal

For a centre and ranges (rx, ry, rz): depth D = 0, 1, 2…; for each D: x from
−min(rx, D) to +min(rx, D); for each x: y from −min(ry, D−|x|) to +min(ry, D−|x|); z =
D − |x| − |y|; if z ≤ rz: emit (x, y, +z), then if z ≠ 0 emit (x, y, −z).

### 6.3 Ores, disks, lakes, springs

**`ore`** — fields `targets` [{`target` (rule test § 4.2), `state`}], `size` (0..64),
`discard_chance_on_air_exposure` (0..1).
```
a = next_float() · π (single-precision float) ; e = size / 8 (float)
R = ceil((size/16 · 2 + 1) / 2)
x0 = p.x + sin(a)·e ; x1 = p.x − sin(a)·e ; z0 = p.z + cos(a)·e ; z1 = p.z − cos(a)·e
      (EXACT sin/cos in double on the converted a)
y0 = p.y + next_int(3) − 2 ; y1 = p.y + next_int(3) − 2
xs = p.x − ceil(e) − R ; ys = p.y − 2 − R ; zs = p.z − ceil(e) − R
L = 2·(ceil(e) + R) ; Hy = 2·(2 + R)
if no column (x from xs to xs+L, z from zs to zs+L) has ys ≤ OCEAN_FLOOR_WG(x,z) → false
for i < size: t = i/size (float) ; c_i = lerp(t, (x0,y0,z0), (x1,y1,z1)) in double
    s = next_double() · size / 16 ; r_i = ((sinT(π·t) + 1) · s + 1) / 2     (sinT = sin TABLE, float argument)
elimination: for i < j (i increasing, j increasing), both of radius > 0:
    if (r_i − r_j)² > |c_i − c_j|² : if r_i − r_j > 0 → r_j = −1 ; otherwise r_i = −1
for each i with r_i ≥ 0 (increasing order):
    bounds: min = max(floor(c − r), start) ; max = max(floor(c + r), min) per axis
    for x, then y, then z within the bounds (x outer), with successive tests
      xd² < 1, xd² + yd² < 1, xd² + yd² + zd² < 1 (d = (coord + 0.5 − c)/r) and y within the height:
      index = (x−xs) + (y−ys)·L + (z−zs)·L·Hy ; if already tested → next ; mark tested
      if the position is within the write area (3×3) and its section exists:
        b = current block ; for each target in order:
          if target.test(b, pos, random) AND (air check ignored OR none of the 6 neighbours is air):
             write `state` DIRECTLY into the section (without updating the heightmaps,
             without block entity, without post-processing) ; count ; stop the targets
returns count > 0
```
Air check ignored: `discard` ≤ 0 → always ignored (no draw); `discard` ≥ 1 →
never ignored; otherwise ignored if `next_float() ≥ discard` (one draw, made AFTER the target's
test and only if it is true). The neighbours are read with the same direct view of the
sections (blocks already replaced by this vein included). The official 26.3 targets combine
`tag_match` (`stone_ore_replaceables`, `deepslate_ore_replaceables`,
`height_specific_ore_replaceables`…) and `height_match`: the stone/deepslate choice of the
ores now depends on the height for the blocks of the "height-specific" tag.

**`scattered_ore`** — same fields.
```
n = next_int(size + 1)
for i < n: m = min(i, 7) ; q = p + (round((next_float() − next_float())·m) ×3, axes x, y, z
             drawn in this order ; round = Java rounding, half towards +∞)
   b = block at q ; for each target: if target.test(b) and air check (neighbours read in the
   world) → place `state` (flag 2), stop
true
```

**`disk`** — `state_provider`, `target` (block predicate), `radius` (0..8), `half_height`
(0..4). r = radius; for each column (x from −r to r varying fastest, then z) with
x² + z² ≤ r²: for y from p.y + hh going down to p.y − hh (inclusive): if `target(q)`:
optional state → placement; if nothing was placed just above in this column → mark the
2 blocks above for post-processing; otherwise (target false) "placed above" = false.

**`lake`** (obsolete but used: lava, sulfur pool) — `fluid`, `barrier` (providers),
`can_place_feature`, `can_replace_with_air_or_fluid`, `can_replace_with_barrier`.
```
if p.y ≤ min_y + 4 → false
o = p + (−8, −4, −8) ; G[16][16][8] = false
n = next_int(4) + 4
n times: xr = next_double()*6 + 3 ; yr = next_double()*4 + 2 ; zr = next_double()*6 + 3
         xp = next_double()*(16 − xr − 2) + 1 + xr/2 ; yp = next_double()*(8 − yr − 4) + 2 + yr/2
         zp = next_double()*(16 − zr − 2) + 1 + zr/2
         for x = 1..14, z = 1..14, y = 1..6 : if ((x−xp)/(xr/2))² + … < 1 → G = true
fl = fluid.state(o)
edge(x,y,z) = not G and one of the 6 neighbours in the grid is G
check: for x, z, y (x outer, y inner) edge: b = block at o+(x,y,z) ;
    y ≥ 4 and b liquid → false ; y < 4 and b not "solid" and b ≠ fl → false ;
    not can_place_feature → false
carving: for x, z, y with G : if can_replace_with_air_or_fluid : y ≥ 4 → cave air
    (+ tick scheduled 0 + marking above) ; otherwise fl
barrier: br = barrier.state(o) ; if br not air : for x, z, y edge : if y < 4 or next_int(2) ≠ 0
    (draw ONLY if y ≥ 4) : if block "solid" and can_replace_with_barrier → br + marking
if fl is water: for x, z: q = o + (x, 4, z) ; if freeze(q) (zoomed biome at q) and
    can_replace_with_air_or_fluid(q) → ice
true
```

**`spring_feature`** — `state` (fluid state), `requires_block_below` (true), `rock_count` (4),
`hole_count` (1), `valid_blocks`. Conditions: above ∈ valid; if required, below ∈ valid;
p = air or valid. Count the "rocks" (west, east, north, south, below ∈ valid) and the
"holes" (same positions empty). If rocks = rock_count AND holes = hole_count: place the
fluid block and schedule an immediate fluid tick → true; otherwise false.

**`underwater_magma`** — `floor_search_range`, `placement_radius_around_floor`,
`placement_probability_per_valid_position`.
```
if p is not water → false
q = p ; for i = 1 .. range−1 while q is water: q −= y      (same upward sweep
     first, no effect here) ; if q is not water → floor = q.y, otherwise false
f = (p.x, floor, p.z) ; r = radius
for each position of the cube f ± r (order x, y, z, x fastest):
    if next_float() < probability AND valid(q) → magma
valid(q): q is neither water nor air ; the top face of the block under q is not "visible"
    (occlusion shape of the top face = full cube) ; and for each horizontal neighbour
    (north, east, south, west) the face turned towards q is full. "Visible" = occlusion shape
    empty or not full.
```
The upward sweep also takes place (before the downward one) but its result is not
used; it consumes no randomness.

### 6.4 Geodes, monster rooms, fossils

**`geode`** — fields: `blocks` (`filling_provider`, `inner_layer_provider`,
`alternate_inner_layer_provider`, `middle_layer_provider`, `outer_layer_provider`,
`inner_placements` (list of states), `cannot_replace`, `invalid_blocks`); `layers`
(`filling` 1.7, `inner_layer` 2.2, `middle_layer` 3.2, `outer_layer` 4.2); `crack`
(`generate_crack_chance` 1.0, `base_crack_size` 2.0, `crack_point_offset` 2);
`use_potential_placements_chance` (0.35), `use_alternate_layer0_chance` (0),
`placements_require_layer0_alternate` (true), `outer_wall_distance` (uniform 4..5),
`distribution_points` (3..4), `point_offset` (1..2), `min_gen_offset` (−16),
`max_gen_offset` (16), `noise_multiplier` (0.05), `invalid_blocks_threshold`.
```
n = distribution_points.sample()
noise N = "parity" normal noise (first octave −4, amplitudes [1]) built with a
          congruential generator of seed = WORLD SEED (recreated for each geode)
adj = n / outer_wall_distance.max
air = 1/√filling ; c0 = 1/√(inner + adj) ; c1 = 1/√(middle + adj) ; c2 = 1/√(outer + adj)
crack = 1/√(base_crack + next_double()/2 + (n > 3 ? adj : 0))
with_crack = next_float() < generate_crack_chance
invalid = 0 ; points = []
n times: (x, y, z) = 3 draws of outer_wall_distance ; q = p + (x,y,z)
         if q is air or ∈ invalid_blocks: invalid++ ; if invalid > threshold → FALSE (nothing placed)
         points += (q, point_offset.sample())          (the point is added even if invalid)
if with_crack: k = next_int(4) ; d = 2n + 1 ;
         offsets (d,·,0) / (0,·,d) / (d,·,d) / (0,·,0) for k = 0/1/2/3, at heights 7, 5, 1
crystals = []
for q in the cube p + [min_gen, max_gen]³ (x fastest, then y, then z):
    o = N(q.x, q.y, q.z) · noise_multiplier
    S = Σ_points (1/√(dist²(q, pt) + offset) + o)
    if S < c2: nothing
    otherwise if S ≥ air: safe placement of filling(q)
    otherwise: F = Σ_crack (1/√(dist²(q, f) + crack_point_offset) + o)
        if with_crack and F ≥ crack: air (safe placement) ; immediate tick for each neighbour
             (6 directions) containing a fluid
        otherwise if S ≥ c0: alt = next_float() < use_alternate ; safe placement of (alt ? alternate : inner)(q)
             if (not require_alt or alt) and next_float() < use_potential → crystals += q
        otherwise if S ≥ c1: middle(q) ; otherwise outer(q)
for c in crystals: e = inner_placements[next_int] ; for d in (down, up, north, south, west, east):
    e.facing = d if the property exists ; v = c + d ; e.waterlogged = (fluid at v is a source)
    if v is air or full water: safe placement of e at v ; go on to the next crystal
true
```
"Safe placement" = place (flag 2) only if the current block is NOT in `cannot_replace`.
`dist²` between integer positions (double). The inverse root is exact (1/√x in double).

**`monster_room`** — no fields.
```
xr = next_int(2) + 2 ; zr = next_int(2) + 2           (in this order)
X = [−xr−1, xr+1], Y = [−1, 4], Z = [−zr−1, zr+1]
holes = 0
for dx ∈ X, dy ∈ Y, dz ∈ Z : s = "solid" block
    dy = −1 and not s → false ; dy = 4 and not s → false
    on the perimeter (dx or dz at a bound), dy = 0, q empty and q+y empty → holes++
if not 1 ≤ holes ≤ 5 → false
for dx ∈ X, dy from 3 to −1, dz ∈ Z :
    on a wall (dx/dz at a bound or dy = −1 or dy = 4) :
        if q.y ≥ min_y and the block under q is not solid → cave air (direct placement)
        otherwise if q solid and not a chest : dy = −1 and next_int(4) ≠ 0 → mossy cobblestone, otherwise cobblestone (safe placements)
    otherwise if q is neither a chest nor a spawner → cave air (safe placement)
2 chests : each time up to 3 attempts : c = (p.x + next_int(2xr+1) − xr, p.y, p.z + next_int(2zr+1) − zr)
    if c empty and exactly 1 of the 4 horizontal neighbours is solid : chest reoriented (§ 6.4.1),
    table `chests/simple_dungeon`, loot seed = next_long() ; stop the attempts for this chest
spawner at p ; entity = [skeleton, zombie, zombie, spider][next_int(4)]
true
```
Safe placement here = unless the current block is in `features_cannot_replace`.

#### 6.4.1 Reorienting a chest

Go through north, east, south, west: if a neighbour is a chest → keep the state; count the
"full opaque" neighbours: if there is exactly one (direction d) → face = opposite(d).
Otherwise start from the current face f (north by default): if neighbour f is opaque → f =
opposite(f); if neighbour f is opaque → f = clockwise(f); if neighbour f is opaque → f =
opposite(f).

**`fossil`** — `fossil_structures`, `overlay_structures` (lists of template identifiers of
the same length), `fossil_processors`, `overlay_processors` (lists of processors § 10.4),
`max_empty_corners_allowed` (0..7).
```
rot = [none, 90° clockwise, 180°, 90° counter-clockwise][next_int(4)]
i = next_int(len)
clipping box = chunk of p widened by 16 blocks on each side, full height
T = size of template i after rotation ; corner = p − (T.x/2, 0, T.z/2)
ymin = min(p.y, OCEAN_FLOOR_WG over all the columns corner + [0,T.x) × [0,T.z))
target = max(ymin − 15 − next_int(10), min_y + 10)
pos = "zero" position of the template for the rotation, placed at the corner at y = target (§ 10.2)
if the number of corners of the template's box that are air/water/lava > max → false
place the base template with its processors, then the overlay with its own, same random source,
flags 260, same pos/pivot
true
```

### 6.5 Giant mushrooms, Nether fungi, corals

**`huge_brown_mushroom` / `huge_red_mushroom`** — `cap_provider`, `stem_provider`,
`foliage_radius` (2), `can_place_on`. Placement = "place and update" (flag 3), only
if the current block is air or ∈ `replaceable_by_mushrooms`.
```
H = next_int(3) + 4 ; if next_int(12) = 0 : H *= 2
valid: p.y ≥ min_y+1 and p.y + H + 1 ≤ max_y ; can_place_on(p − y) ;
       for dy = 0..H : r = radius(dy) ; the whole square ±r at height dy must be air or leaves
cap then stem (stem: dy = 0..H−1, state = stem_provider.state(p) drawn for EACH block,
always evaluated at the origin position p)
```
Brown: radius(dy) = dy ≤ 3 ? 0 : R. Cap: one layer at dy = H; for dx, dz ∈ [−R, R] except
corners: state = cap(p) with west = (dx = −R) or (z edge and dx = 1−R); east = (dx = R) or (z edge
and dx = R−1); north = (dz = −R) or (x edge and dz = 1−R); south = (dz = R) or (x edge and dz =
R−1) — only if the state has these 4 properties.
Red: radius(dy) = R if H−3 ≤ dy ≤ H, otherwise 0. Cap: dy = H−3..H; r = dy < H ? R : R−1;
c = R − 2; for dx, dz ∈ [−r, r]: if dy ≥ H or (x edge) ≠ (z edge): state = cap(p) with
up = (dy ≥ H−1), west = dx < −c, east = dx > c, north = dz < −c, south = dz > c.

**`huge_fungus`** — `valid_base_block`, `stem_state`, `hat_state`, `decor_state`,
`replaceable_blocks` (predicate), `planted` (false).
```
if the block under p is not valid_base_block → false
H = between(4, 13) ; if next_int(12) = 0 : H *= 2
if not planted and p.y + H + 1 ≥ gen_depth → false
huge = not planted and next_float() < 0.06
air at p (flag 260)
STEM: r = huge ? 1 : 0 ; for dx, dz ∈ [−r, r] ; corner = huge and |dx| = |dz| = r ;
      for dy < H : q ; if replaceable(q, with_plants) :
          planted: if below is not air → break q (with drops) ; place-and-update stem
          otherwise corner: if next_float() < 0.1 → stem ; otherwise stem
HAT: vines = (hat_state is a Nether wart block)
     h = min(next_int(1 + H/3) + 5, H) ; d = H − h
     for dy = d..H :
         r = dy < H − next_int(3) ? 2 : 1 ; if h > 8 and dy < d + 4 : r = 3 ; if huge : r++
         for dx, dz ∈ [−r, r] : edgeX, edgeZ ; interior = not edgeX and not edgeZ and dy ≠ H ;
             corner = edgeX and edgeZ ; bottom = dy < d + 3
             if replaceable(q, without_plants) :
                 (planted: break if below is not air)
                 bottom : if not interior : drip(q)
                 otherwise interior : hat(q, 0.1 ; 0.2 ; vines ? 0.1 : 0)
                 otherwise corner : hat(q, 0.01 ; 0.7 ; vines ? 0.083 : 0)
                 otherwise : hat(q, 0.0005 ; 0.98 ; vines ? 0.07 : 0)
hat(q, pd, ph, pl): if next_float() < pd → decor ; otherwise if next_float() < ph → hat, then
    if next_float() < pl → weeping vines under q
drip(q): if the block under q is already hat → hat ; otherwise if next_float() < 0.15 → hat, then if
    vines and next_int(11) = 0 → weeping vines under q
weeping vines(q): v = q − y ; if v empty: n = between(1, 5) ; if next_int(7) = 0 : n *= 2 ;
    for k = 0..n : if v empty : if k = n or the block under v is not empty → vine head
    (age between(23, 25)), end ; otherwise vine stem ; v −= y
replaceable(q, with_plants) = replaceable state, or (with_plants and replaceable_blocks(q))
```
All the fungus placements are "place and update", except the vines (flag 2).

**Corals** (`coral_tree`, `coral_claw`: field `feature` = placed feature of a "coral block").
The coral blocks are described in data: `coral/<colour>_block` = `overlay` (simple coral block,
then `coral/block_decoration`: with a +1 offset in y, weighted choice coral
(20) / sea pickle (3) / nothing (57); then for each face north, east, south, west, with
probability 0.2 and if the cell is water: wall coral facing outwards). The placed feature passed to the
shapes checks that the cell is water or a coral and that there is water above.
```
coral_tree : h = next_int(3) + 1
    for i < h : if not place(q) → return true ; q += up
    top = q ; n = next_int(3) + 2 ; dirs = shuffle of [north, east, south, west]
    for each d of the first n : q = top + d ; bh = next_int(5) + 2 ; seg = 0
        for j < bh while place(q) : seg++ ; q += up ;
            if j = 0 or (seg ≥ 2 and next_float() < 0.25) : q += d ; seg = 0
    true
coral_claw : if not place(p) → false
    g = drawn horizontal direction ; n = next_int(2) + 2
    dirs = shuffle of [g, clockwise(g), counter-clockwise(g)]
    for each b of the first n : q = p + b ; L1 = next_int(2) + 1
        if b = g : seg = g ; L2 = next_int(3) + 2
        otherwise : q += up ; seg = [b, up][next_int(2)] ; L2 = next_int(3) + 3
        for i < L1 while place(q) : q += seg
        q −= seg ; q += up
        for i < L2 : q += g ; if not place(q) → stop ; if next_float() < 0.25 : q += up
    true
```
The third coral pattern ("mushroom") no longer exists in code: it is the placed feature
`coral/<colour>_block` with `offset` (y −3..−1) + `cuboid` (xz 3..5, y 3..5, without edges or
interior) + `random_chance` 0.9 + filter — see `warm_ocean_vegetation`.

### 6.6 Ice: icebergs, ice spikes

**`iceberg`** — `state` (packed or blue ice). All placements = "place and
update". Single-precision floats (`f`) and doubles (`d`) as indicated; `pow` = exact power.
```
o = (p.x, SEA_LEVEL, p.z)
snow = next_double() > 0.7
angle = next_double() · 2π
A = 11 − next_int(5) ; C = 3 + next_int(3)
ellipse = next_double() > 0.7
Hh = ellipse ? next_int(6) + 6 : next_int(15) + 3
if not ellipse and next_double() > 0.9 : Hh += next_int(19) + 7
Hb = min(Hh + next_int(11), 18)
W = min(Hh + next_int(7) − next_int(5), 11)
a = ellipse ? A : 11
ABOVE-WATER PART: for xo = −a..a−1, zo = −a..a−1, y = 0..Hh−1 :
    r = ellipse ? ellipse_radius(y, Hh, W) : round_radius(y, Hh, W)
    if ellipse or xo < r : block(o, Hh, xo, y, zo, r, a)
smoothing(o, W, Hh)
SUBMERGED PART: for xo, zo likewise, y = −1 going down while y > −Hb :
    a' = ellipse ? ceil(a · (1 − y²/(Hb·8))) (f) : a
    r = steep_radius(−y, Hb, W)
    if xo < r : block(o, Hb, xo, y, zo, r, a')
carve = ellipse ? next_double() > 0.1 : next_double() > 0.7
if carve : carve_out(…)
true

block(o, H, xo, y, zo, r, a) :
    s = ellipse ? ellipse_dist(xo, zo, 0, a, c(y, H), angle) : circle_dist(xo, zo, r)
    if s < 0 : threshold = ellipse ? −0.5 : −6 − next_int(3)
               if s > threshold and next_double() > 0.9 → nothing
               place_block(o + (xo, y, zo), H − y, H)
place_block(q, hd, H) : if q ∈ {air, snow block, ice, water} :
    rnd = not ellipse or next_double() > 0.05
    div = ellipse ? 3 : 2
    if snow and q ≠ water and hd ≤ next_int(max(1, H/div)) + H·0.6 and rnd → snow block
    otherwise → state
    (lazy evaluation: `next_int` only if snow and q ≠ water)
c(y, H) = C − (y > 0 and H − y ≤ 3 ? 4 − (H − y) : 0)
circle_dist(x, z, r) = 10·clamp(next_float(), 0.2, 0.8)/r (f) + x² + z² − r²
ellipse_dist(x, z, centre, a, c, θ) = ((dx·cosθ − dz·sinθ)/a)² + ((dx·sinθ + dz·cosθ)/c)² − 1   (double)
round_radius(y, H, W) : k = 3.5 − next_float() ; e = (1 − y²/(H·k))·W
    if H > 15 + next_int(5) : y' = y < 3 + next_int(6) ? y/2 : y ; e = (1 − y'/(H·k·0.4))·W
    return ceil(e/2)
ellipse_radius(y, H, W) = ceil((1 − y²/H)·W / 2)
steep_radius(y, H, W) : k = 1 + next_float()/2 ; ceil((1 − y/(H·k))·W / 2)
smoothing : a = ellipse ? A : W/2 ; for x, z ∈ [−a, a], y = 0..Hh :
    if q ∈ {packed ice, snow block, blue ice, snow layer} :
        if below is air : air at q and at q + y
        otherwise if q ∈ {packed ice, snow block, blue ice} and ≥ 3 of the 4 horizontal
             neighbours (west, east, north, south) are not : air
carve_out : sx = next_boolean() ? −1 : 1 ; sz likewise
    dx = next_int(max(W/2 − 2, 1)) ; if next_boolean() : dx = W/2 + 1 − next_int(max(W − W/2 − 1, 1))
    dz likewise
    if ellipse : dx = dz = next_int(max(A − 5, 1))
    centre = (sx·dx, 0, sz·dz) ; θ = ellipse ? angle + π/2 : next_double()·2π
    for y = 0..Hh−4 : r = round_radius(y, Hh, W) ; dig(r, y, underwater = false)
    for y = −1 while y > −Hh + next_int(5) (re-drawn at each test) : r = steep_radius(−y, Hh, W) ; dig(r, y, true)
dig(r, y, water) : a = r + 1 + A/3 ; c = min(r − 3, 3) + C/2 − 1 ;
    for x, z ∈ [−a, a−1] : if ellipse_dist(x, z, centre, a, c, θ) < 0 and the block is iceberg
    ice or a snow block : water (underwater) or air + removal of a snow layer above
```

**`spike`** (ice spikes) — `state`, `can_place_on`, `can_replace`. Placements = "place and
update".
```
go down p while p is empty and p.y > min_y + 2
if not can_place_on(p) → false
p += (0, next_int(4), 0) ; H = next_int(4) + 7 ; W = H/4 + next_int(2)
if W > 1 and next_int(60) = 0 : p += (0, 10 + next_int(30), 0)
for y < H : s = (1 − y/H)·W (f) ; n = ceil(s)
    for x ∈ [−n, n] (dx = |x| − 0.25), z ∈ [−n, n] (dz = |z| − 0.25) :
        if ((x = 0 and z = 0) or dx²+dz² ≤ s²) and (point not on the edge of square n, or next_float() ≤ 0.75) :
            q = p + (x, y, z) : if air or can_replace → state
            if y ≠ 0 and n > 1 : q' = p + (x, −y, z) : same
pw = clamp(W − 1, 0, 1)
for x, z ∈ [−pw, pw] : c = p + (x, −1, z) ; L = 50 ; if |x| = |z| = 1 : L = next_int(5)
    while c.y > 50 (ABSOLUTE y) :
        if c is not air, not replaceable and ≠ state → stop
        state at c ; c −= y ; L−− ; if L ≤ 0 : c −= (next_int(5) + 1) in y ; L = next_int(5)
true
```
(The edge test only calls `next_float()` for the edge points.)

### 6.7 Nether and End

**`delta_feature`** — `contents`, `rim`, `size` (0..16), `rim_size` (0..16). Placements =
"place and update".
```
edge = next_double() < 0.9
bx = edge ? rim_size.sample() : 0 ; bz = edge ? rim_size.sample() : 0
with_rim = edge and bx ≠ 0 and bz ≠ 0
rx = size.sample() ; rz = size.sample() ; M = max(rx, rz)
for q in the Manhattan traversal (§ 6.2.2) of ranges (rx, 0, rz), stopping as soon as dist > M :
    if free(q) : if with_rim → rim at q ; q2 = q + (bx, 0, bz) ; if free(q2) → contents at q2
free(q) : block ≠ contents, ∉ {bedrock, Nether bricks, Nether brick fence,
    Nether brick stairs, Nether wart, chest, spawner} ; and for the 6
    neighbours : the neighbour ABOVE must be air, all the others NON-air
```
True if at least one placement.

**`stepped_column_cluster`** (basalt columns) — `block`, `continue_through`,
`can_replace`, `cannot_place_on`, `cluster_reach` (0..13), `column_count` (1..150),
`column_reach` (0..3), `height` (1..10). "Lava level" = the generator's sea level
(not used in the computation). Placements = "place and update".
```
if not possible(p) → false
H = height.sample() ; R = min(H, cluster_reach.sample()) ; N = column_count.sample()
N times : q = (p.x − R + next_int(2R+1), p.y + next_int(1), p.z − R + next_int(2R+1))   (x, y, z)
    b = H − Manhattan(q, p) ; if b ≥ 0 : column(q, b, column_reach.sample())
column(c, h, r) : for each position s of the square c ± r (x fastest) :
    k = Manhattan(s, c)
    base = can_replace(s) ? surface(s, k) : air_above(s, k)
    if base : n = h − k/2 ; while n ≥ 0 :
        if can_replace(cursor) : place block.state(cursor), go up
        otherwise if continue_through(cursor) : go up ; otherwise stop
        n−−
possible(s) : can_replace(s) and the block below is neither air nor ∈ cannot_place_on
surface(s, k) : while s.y > min_y + 1 and k > 0 : k−− ; if possible(s) → s ; s −= y ; → nothing
air_above(s, k) : while s.y ≤ max_y and k > 0 : k−− ; if s ∈ cannot_place_on → nothing ;
    if s air → s ; s += y ; → nothing
```

**`end_spike`** — `spikes` (list `{centerX, centerZ, radius, height, guarded}`),
`crystal_invulnerable` (false), optional `crystal_beam_target`. Empty list → the world's pillars:
```
k = (congruential(world_seed).next_long()) AND 0xFFFF
sizes = shuffle of [0..9] with congruential(k)
for i < 10 : x = floor(42·cos(2·(−π + π/10·i))) ; z = floor(42·sin(…)) (exact double)
    t = sizes[i] ; radius = 2 + t/3 ; height = 76 + 3t ; guarded = (t = 1 or t = 2)
```
For each pillar whose centre is in the chunk of p: for q of the box (cx−r, min_y,
cz−r)..(cx+r, h+10, cz+r) (x, y, z) : if (q.x−cx)² + (q.z−cz)² ≤ r²+1 and q.y < h → obsidian;
otherwise if q.y > 65 → air. If guarded: cage of iron bars on dx, dz ∈ [−2,2], dy ∈ [0,3]
(side faces and top; connections north = edgeX and dz ≠ −2, south = edgeX and dz ≠ 2, west =
edgeZ and dx ≠ −2, east = edgeZ and dx ≠ 2, where edgeX = |dx| = 2 or top, edgeZ = |dz| = 2 or
top). End crystal at (cx+0.5, h+1, cz+0.5), yaw = next_float()·360, beam target,
invulnerability; bedrock under it and fire at its position. Placements = "place and update".

**`end_island`** — t = next_int(3) + 4 (float); y = 0; while t > 0.5: for x from
floor(−t) to ceil(t), z likewise: if x² + z² ≤ (t+1)² → End stone at p + (x, y, z);
t −= next_int(2) + 0.5; y−−.

**`end_gateway`** — optional `exit`, `exact`. 3×5×3 box around p (x, y, z): centre →
End gateway (+ exit if provided); same y (except centre) → air; |dy| = 2 and same x and z →
bedrock; (same x or same z) and |dy| < 2 → bedrock; otherwise air.

**`end_platform`** — for dz, dx ∈ [−2,2], dy ∈ [−1,2]: obsidian at dy = −1, air otherwise (if
different), "place and update".

**`end_podium`** — `active`. For q of the box p+(−4,−1,−4)..p+(4,32,4): inside = dist²(q,p) <
2.5²; near = dist² < 3.5²; if inside or near: below p → bedrock (inside) or End stone;
above p → air; at the height of p → bedrock (near but not inside), portal (inside, active) or
air. (Active: replaces by breaking what exists.) Then bedrock at p..p+3, and wall torches on
the 4 horizontal faces of p+2, facing outwards.

### 6.8 Speleothems (dripstone and sulfur)

Common utilities ("base" = the feature's base block, "tip" = pointed block; for the
large dripstone, base = fixed dripstone block):
- `is_base(b)` = b is the base or ∈ `replaceable_blocks`; `empty_or_water(b)` = air or water;
  `empty_water_lava(b)` = air, water or lava.
- `place_base(q)`: if q ∈ replaceable → base (flag 2), true; otherwise false.
- `column(dir, L, merge)` produces in order: if L ≥ 3: `base` thickness then (L−3) times
  `middle`; if L ≥ 2: `frustum`; if L ≥ 1: `tip_merge` (merge) or `tip`. Each state =
  tip with `tip_direction` = dir and `thickness`.
- `grow(q, dir, L, merge)`: if the block at q − dir is a base (is_base): place the column
  from q advancing in dir, each tip waterlogged if water is in place.
- Shape height `h(d, R, scale, bluntness)` (double): d' = max(d, bluntness); r = d'/R·0.384;
  v = scale·(0.75·r^(4/3) − r^(2/3) − ln(r)/3); return max(v, 0)/0.384·R.
- `embedded_circle(c, r)`: false if c is empty/water/lava; for a = 0; a < 2π (float);
  a += 6/r: q = c + ((int)(cosT(a)·r), 0, (int)(sinT(a)·r)) (table); if q empty/water/lava → false.
  True.

**`speleothem`** — `base_block`, `pointed_block`, `replaceable_blocks`,
`chance_of_taller_generation` (0.2), `chance_of_directional_spread` (0.7),
`chance_of_spread_radius2` (0.5), `chance_of_spread_radius3` (0.5).
```
up = is_base(p + y) ; down = is_base(p − y)
dir = (up and down) ? (next_boolean() ? down : up) : up ? down : down ? up : none → false
r = p − dir ; place_base(r)
for d in north, east, south, west : if next_float() ≤ spread :
    q1 = r + d ; place_base(q1)
    if next_float() ≤ r2 : q2 = q1 + drawn direction (next_int(6), order down, up, north, south, west, east) ; place_base(q2)
        if next_float() ≤ r3 : q3 = q2 + drawn direction ; place_base(q3)
L = (next_float() < taller and p + dir empty or water) ? 2 : 1
grow(p, dir, L, no merge) ; true
```

**`speleothem_cluster`** — `base_block`, `pointed_block`, `replaceable_blocks`,
`floor_to_ceiling_search_range`, `height`, `radius`, `max_stalagmite_stalactite_height_diff`,
`height_deviation`, `speleothem_block_layer_thickness`, `density`, `wetness` (floats),
`chance_of_speleothem_at_max_distance_from_center`,
`max_distance_from_edge_affecting_chance_of_speleothem`,
`max_distance_from_center_affecting_height_bias`.
```
if p is not empty/water → false
H = height ; wet = wetness ; dens = density ; rx = radius ; rz = radius     (drawn in this order)
for dx = −rx..rx, dz = −rz..rz :
    edge = min(rx − |dx|, rz − |dz|)
    ch = clamped_map(edge, 0, max_edge, chance_at_max, 1)
    column(p + (dx, 0, dz))
column(q) :
    vertical sweep (like § 6.3 magma) with "inside" = empty/water, "edge" = neither empty nor water,
    range = search_range ; ceiling = y of the first edge above (if valid), floor = same
    below ; if neither ceiling nor floor → nothing (NO draw)
    pool = next_float() < wet
    if pool and floor and pool_possible(floor) : floor' = floor − 1 ; water at (q.x, floor, q.z) ; otherwise floor' = floor
    stalactite = next_double() < ch
    if ceiling and stalactite and the ceiling is not lava :
        e = layer_thickness ; base upwards from the ceiling, at most e blocks (stop at the first failure)
        max = floor' ? min(H, ceiling − floor') : H ; hc = height(dx, dz, dens, max)
    otherwise hc = 0
    stalagmite = next_double() < ch
    if floor' and stalagmite and floor' is not lava :
        e = layer_thickness ; base downwards from floor', at most e
        hs = ceiling ? max(0, hc + uniform(−diff, diff)) : height(dx, dz, dens, H)
    otherwise hs = 0
    if ceiling and floor' and ceiling − hc ≤ floor' + hs :
        low_c = max(ceiling − hc, floor' + 1) ; high_s = min(floor' + hs, ceiling − 1)
        b = uniform(low_c, high_s + 1) ; hc = ceiling − b ; hs = (b − 1) − floor'
    merge = next_boolean() and hc > 0 and hs > 0 and column height known and hc + hs = height
    if ceiling : grow((q.x, ceiling − 1, q.z), down, hc, merge)
    if floor' : grow((q.x, floor' + 1, q.z), up, hs, merge)
height(dx, dz, dens, max) : if next_float() > dens → 0 ;
    m = clamped_map(|dx|+|dz|, 0, max_center, max/2, 0) ;
    (int) clamp(m + next_gaussian()·height_deviation, 0, max)     (single-precision floats)
pool_possible(s) : the block is neither water, nor base, nor tip ; no water above ; the 4 horizontal
    neighbours and the one below are base stone (`base_stone_overworld`) or water
```
"Known column height" = ceiling and floor' present; height = ceiling − floor' − 1.
`clamped_map(v, a, b, c, d)` = c + (d − c)·clamp((v − a)/(b − a), 0, 1).

**`large_dripstone`** — `replaceable_blocks`, `floor_to_ceiling_search_range` (30),
`column_radius`, `height_scale`, `max_column_radius_to_cave_height_ratio`,
`stalactite_bluntness`, `stalagmite_bluntness`, `wind_speed`, `min_radius_for_wind`,
`min_bluntness_for_wind`.
```
if p is not empty/water → false
sweep : inside = empty/water ; edge = dripstone block, replaceable or lava
a ceiling AND a floor are needed, and height = ceiling − floor − 1 ≥ 4, otherwise false
rmax = clamp((int)(height · ratio), min(column_radius), max(column_radius))
R = uniform(min(column_radius), rmax)
top = shape(root (p.x, ceiling − 1, p.z), downwards, R, bluntness = stalactite_bluntness, scale = height_scale)
bottom = shape(root (p.x, floor + 1, p.z), upwards, R, stalagmite_bluntness, height_scale)
       (for each: bluntness drawn then scale)
wind : if both shapes have R ≥ min_radius_for_wind and bluntness ≥ min_bluntness_for_wind :
       v = wind_speed.sample() ; a = next_float()·π ; vector (cosT(a)·v, 0, sinT(a)·v) ; max offset 16 − R
       otherwise no wind
wind_offset(q) = q + (clamp(floor(vx·(p.y − q.y)), ±max), 0, clamp(floor(vz·(p.y − q.y)), ±max))
anchor(shape) : while r > 1 : try up to min(10, h(0)) positions moving back (upwards
    for the stalactite, downwards for the stalagmite) : lava → failure ;
    embedded_circle(wind_offset(position), r) → root = position, success ; then r /= 2. Failure.
if anchor(top) : place(top) ; if anchor(bottom) : place(bottom)     (both anchorings BEFORE the placements)
place(f) : for dx, dz ∈ [−r, r] : d = sqrt(dx²+dz²) (f) ; if d ≤ r : n = (int) h(d) ;
    if n > 0 : if next_float() < 0.2 : n = (int)(n · (0.8 + next_float()·0.2))
    q = root + (dx, 0, dz) ; outside = false ; ymax = pointing up ? WORLD_SURFACE_WG(q) : +∞
    for i < n while q.y < ymax : w = wind_offset(q) ;
        if w empty/water/lava : outside = true ; dripstone block at w
        otherwise if outside and w ∈ base_stone_overworld → stop
        q advances (up or down)
true
```
The height `h(d)` of a shape uses the current R (after reductions), its scale and its bluntness.

### 6.9 Cave vegetation, carpets, roots, sculk, templates

**`multiface_growth`** (glow lichen, sculk veins) — `block` ("multi-face" block),
`search_range` (10), `can_place_on_floor` (false), `can_place_on_ceiling` (false),
`can_place_on_wall` (false), `chance_of_spreading` (0.5), `can_be_placed_on`.
```
if p is neither air nor water → false
V = [up if ceiling] + [down if floor] + [north, east, south, west if wall]   (in this order)
D = shuffle of V
if attempt(p, D) → true
for s in D : D' = shuffle of (V without opposite(s))           (one shuffle per s)
    q = p ; repeat search_range times : q = p + s     ← ALWAYS p + s (the step does not advance: quirk)
        b = block at q ; if b is neither air nor water nor the block itself → stop
        if attempt(q, D') → true
false
attempt(q, dirs) : for d in dirs : if the neighbour q + d ∈ can_be_placed_on :
    state = placement state of the multi-face block at q with face d (added to the current state
           if it is already this block ; waterlogged if source water) ; impossible → FALSE (end of the attempt)
    place and update ; mark q for post-processing
    if next_float() < chance : spreading (§ 6.9.1) from face d, with post-processing
    true
false
```
As the search loop repeats the same cell, only the cells immediately adjacent to p
are tried (up to `search_range` times the same one, with no effect after the first).

#### 6.9.1 Multi-face spreading

`spread_from_face(state, q, face, random)`: dirs = shuffle of the 6 directions (5 draws);
for each s (in shuffled order): if s is on the same axis as `face` → next. The state must
have face `face` and not face s (or the source must be "another valid block").
Try in order: (q, face s) [same cell], (q + s, face `face`) [same plane],
(q + s + face, face opposite(s)) [wrap-around]; the first admissible target (replaceable cell
— air, this block, source water; and the face can attach to the neighbour) is placed (flag 2,
post-processing marked) → end. Otherwise next direction.
Sculk veins have their own admission rules (not against sculk/catalyst; no
fluid other than water; no fire; replaceable; refused if the wrap-around is blocked by a full
face).

**`vegetation_patch`** — `replaceable`, `ground_state`, `vegetation_feature` (placed feature),
`surface` (`floor`/`ceiling`), `depth` (1..128), `extra_bottom_block_chance`, `vertical_range`
(1..256), `vegetation_chance`, `xz_radius`, `extra_edge_column_chance`.
```
rx = xz_radius.sample() + 1 ; rz = xz_radius.sample() + 1
in = (floor ? down : up) ; out = opposite
S = empty hash set
for dx ∈ [−rx, rx], dz ∈ [−rz, rz] :
    corner = x edge and z edge ; edge_only = edge and not corner
    if corner → next
    if edge_only : if edge_chance = 0 → next ; otherwise if next_float() > edge_chance → next
    q = p + (dx, 0, dz)
    k = 0 ; while q air and k < vertical_range : q += in ; k++
    k = 0 ; while q not air and k < vertical_range : q += out ; k++
    s = q + in
    if q empty and the "out" face of s is full (load-bearing) :
        n = depth.sample() + (extra_bottom > 0 and next_float() < extra_bottom ? 1 : 0)
        ground(s, n) → if true : S += s
for s in S (HASH ORDER, § 5.0) : if vegetation_chance > 0 and next_float() < chance :
    place vegetation_feature at s + out
true if S non-empty
ground(s, n) : for i < n : e = ground_state.state(s) ; b = block at s ;
    if e is not the same block as b : if b ∉ replaceable → return (i ≠ 0) ; place e ; s += in
    (if same block : neither placement nor advance)
    return true
```

**`waterlogged_vegetation_patch`** — same fields. After the ground phase: W = { s ∈ S (hash
order) : no neighbour north, east, south, west, down "exposed" } (exposed = the face turned towards s
is not full); water at each s of W; then the vegetation is distributed over W (not S): the
feature is placed at s (= (s + up) − 1), and if a block was placed, the block at s + up receives
`waterlogged=true` if it has the property and is not already so.

**`root_system`** (rooted azalea, sulfur spring) — `feature` (placed feature of the tree),
`required_vertical_space_for_tree`, `level_test_distance`, `max_level_deviation`,
`root_radius`, `root_replaceable`, `root_state_provider`, `root_placement_attempts`,
`root_column_max_height`, `hanging_root_radius`, `hanging_roots_vertical_span`,
`hanging_root_state_provider`, `hanging_root_placement_attempts`,
`allowed_vertical_water_for_tree`, `allowed_tree_position`.
```
if p not air → false
q = p
for y < root_column_max_height :
    q += up
    if WORLD_SURFACE(q) < q.y → (tree impossible) go to END
    if allowed_tree_position(q) and place_for_tree(q) :
        if lava under q or block under q not solid → END
        if the tree is placed at q : rooted dirt from p.y to p.y + y − 1 ; then hanging roots ; END
END : true (in all cases)
place_for_tree(q) : for i = 1..required : b = q + i ; air OK ; otherwise water OK only if
    i + 1 ≤ allowed_vertical_water ; otherwise no.
    if level_test_distance > 0 : for i = 0..3 (directions south, west, north, east) : c = q + d·dist ;
    c − deviation must be non-air AND c + deviation air.
rooted dirt(level y) : working position w = (p.x, y, p.z) ;
    attempts times : w += (next_int(r) − next_int(r), 0, next_int(r) − next_int(r)) ;
    if w ∈ root_replaceable → root_state_provider ; then w returns to (p.x, ·, p.z)
hanging roots : attempts times : w = p + (next_int(R) − next_int(R), next_int(V) − next_int(V),
    next_int(R) − next_int(R)) ; if w empty : e = provider(w) ; if e survives and the bottom face of the block
    above is full → placement
```
(2D directions of index 0..3: south, west, north, east.)

**`sculk_patch`** — `charge_count` (1..32), `amount_per_charge` (1..500), `spread_attempts`
(1..64), `growth_rounds` (0..8), `spread_rounds` (0..8).
```
if not start_possible(p) → false
generation spreader : tag `sculk_replaceable_world_gen`, growth cost 50,
    no-growth radius 1, decay rate 5, additional decay 10
for round < spread + growth :
    charge_count times : add cursors (p, amount) in slices of 1000, at most 32 cursors
    veins = round < spread_rounds
    spread_attempts times : update the cursors (§ 6.9.2)
    clear the cursors
true
start_possible(p) : "sculk" block (sculk behaviour) ; or (air or non-source water) with at
    least one of the 6 neighbours having a full collision shape
```

#### 6.9.2 Sculk cursors (generation)

Cursor = (position, charge, update delay = 0, decay delay = 1, faces).
List update: for each cursor (list order) whose Chebyshev distance
to p ≤ 1024: `update(cursor)`; if charge ≤ 0 → removed; otherwise kept (in generation, two
cursors on the same cell do NOT merge; both are kept).
`update` (if charge > 0): if delay > 0 → delay−−; otherwise:
- behaviour C of the block at the position (sculk, vein, or "default" for any other);
- if veins: vein attempt (§ below); if successful and C may change, re-read the block;
- charge = C.use_charge(…); if ≤ 0: C.discharged; otherwise: target = valid_move;
  if target: C.discharged, position = target; otherwise (generation): C.discharged, charge = 0, end;
  if the new block is sculk/vein: faces = available faces of its state; decay
  delay = C.update_decay(old); delay = 1.
- Valid move: 18 "non-corner" neighbours (3×3×3 box without the 8 corners nor the centre,
  order x, y, z from −1 to 1) shuffled; for each: horizontal distance² to p ≤ 144; block with
  sculk behaviour; unobstructed movement (adjacent, or diagonal if one of the two
  orthogonal paths is not blocked by a full face) → candidate; if it is a vein
  with access to a substrate (`sculk_replaceable`) → it is taken and we stop; the last
  candidate otherwise.
- Behaviours:
  - default: vein: unknown faces → "same cell" vein spreading from all the
    faces; known non-empty faces → if air or water: push back a vein with the attachable
    faces (place and update); charge = decay_delay > 0 ? charge : 0;
    update_decay = max(a − 1, 0).
  - sculk: if charge ≠ 0 and next_int(5) = 0: near = dist(pos, p) < 1; if not near and
    growth possible (above is air or water, and at most 2 `sculk_growth_inhibitors`
    in the box pos+(−4,0,−4)..pos+(4,2,4)): if next_int(50) < charge → above: shrieker
    (if next_int(11) = 0, `can_summon` true) otherwise sensor (waterlogged if fluid); return
    max(0, charge − 50). Otherwise: if next_int(10) ≠ 0 → charge; otherwise charge − (near ? 1 :
    penalty) with penalty = max(1, (int)(charge · min(1, (√dist² − 1)² / 23²) · 0.5)).
    If no initial draw → charge. Does not change state on spreading.
  - vein: if veins and place_sculk succeeds → charge − 1; otherwise next_int(5) = 0 ?
    floor(charge · 0.5) : charge. place_sculk: for each direction of a shuffle of the 6: if the
    vein has face d and the support (pos + d) ∈ the spreader's tag: sculk at the support
    (place and update); vein spreading from all its faces; then every vein
    adjacent to the support (except the opposite side) is "discharged"; success.
    Discharged (vein): remove the faces stuck to a sculk block; no face left → air or
    water depending on the fluid; place and update.

**`template`** — `templates` (weighted list of `{id, rotations}`; default rotations: all
4), optional `processors`.
```
e = weighted choice ; rot = e.rotations[next_int(n)]
d = half-size : for the x axis : unit vector of direction rot(west) × (size.x / 2) ;
    for z : rot(north) × (size.z / 2)
pos = p + dx + dz
place the template (§ 10.2) at pos, pivot pos, rotation rot, shared random source, processors, flags 3
returns the result of the placement
```
(rot(west): 90° clockwise rotation applied to the west direction → north, etc.)

## 7. Structures: sets and placement

### 7.1 Data

- `worldgen/structure/<id>.json`: `type` (§ 8), `biomes` (set), `step` (decoration step
  where its pieces are placed, § 2.3), `spawn_overrides`, `terrain_adaptation`
  (`none` default, `bury`, `beard_thin`, `beard_box`, `encapsulate`, § 9.5) + fields of the type.
- `worldgen/structure_set/<id>.json`: `structures` [{`structure`, `weight` > 0}],
  `placement` (§ 7.2).
- "Possible" sets of a dimension = sets of the registry (sorted order) of which at least one
  structure has a biome among the source's possible biomes. A structure is attached to
  a placement only if it itself has a possible biome ("structure → placements" table, useful
  for search and rings). The DRAW within a set, however, keeps ALL its
  members and their weights: a structure with no possible biome stays there and fails its attempt
  (e.g. the ruined Nether portal counts in the draw of the surface portals).

26.3 values (all `random_spread` unless stated; spacing/separation in chunks):

| Set | Structures (weight) | spacing / separation | salt | Notes |
|---|---|---|---|---|
| `abandoned_camp` (new) | 18 biome variants (1) | 37 / 8 | 91231127 | |
| `ancient_cities` | ancient_city | 24 / 8 | 20083232 | |
| `buried_treasures` | buried_treasure | 1 / 0 | 0 | frequency 0.01 `legacy_type_2`, locate_offset (9,0,9) |
| `desert_pyramids` | desert_pyramid | 32 / 8 | 14357617 | |
| `end_cities` | end_city | 20 / 11 | 10387313 | triangular |
| `igloos` | igloo | 32 / 8 | 14357618 | |
| `jungle_temples` | jungle_pyramid | 32 / 8 | 14357619 | |
| `mineshafts` | mineshaft, mineshaft_mesa (1,1) | 1 / 0 | 0 | frequency 0.004 `legacy_type_3` |
| `nether_complexes` | fortress (2), bastion_remnant (3) | 27 / 4 | 30084232 | |
| `nether_fossils` | nether_fossil | 2 / 1 | 14357921 | |
| `ocean_monuments` | monument | 32 / 5 | 10387313 | triangular |
| `ocean_ruins` | ocean_ruin_cold, _warm | 20 / 8 | 14357621 | |
| `pillager_outposts` | pillager_outpost | 32 / 8 | 165745296 | frequency 0.2 `legacy_type_1`, exclusion `villages` at 10 chunks |
| `ruined_portals` | 7 variants | 40 / 15 | 34222645 | |
| `shipwrecks` | shipwreck, shipwreck_beached | 24 / 4 | 165745295 | |
| `strongholds` | stronghold | `concentric_rings` distance 32, spread 3, count 128, preferred `#stronghold_biased_to` | 0 | |
| `swamp_huts` | swamp_hut | 32 / 8 | 14357620 | |
| `trail_ruins` | trail_ruins | 34 / 8 | 83469867 | |
| `trial_chambers` | trial_chambers | 34 / 12 | 94251327 | |
| `villages` | 5 villages (1) | 34 / 8 | 10387312 | |
| `woodland_mansions` | mansion | 80 / 20 | 10387319 | triangular |

### 7.2 Placements (`placement.type`)

Common fields of the "spread" placements: `locate_offset` (0), `frequency_reduction_method`
(`default`), `frequency` (1.0), `salt` (≥ 0), optional `exclusion_zone` (`other_set`,
`chunk_count` 1..16).

`is_structure_chunk(cx, cz)` = `placement_chunk(cx, cz)` AND (frequency ≥ 1 OR
reducer(…)) AND (no exclusion OR no chunk of the other set in the square
[cx−n, cx+n] × [cz−n, cz+n], tested with ITS full `is_structure_chunk`).

**`random_spread`** (`spacing` > `separation`, `spread_type` `linear`/`triangular`):
```
gx = floor_div(cx, spacing) ; gz = floor_div(cz, spacing)
random = generation generator (congruential) ; seed_with_salt(G, gx, gz, salt)     (§ 2.4)
L = spacing − separation
ox = spread(L) ; oz = spread(L)         linear : next_int(L) ; triangular : (next_int(L) + next_int(L)) / 2
potential chunk = (gx·spacing + ox, gz·spacing + oz) ; placement_chunk ⇔ equality
```

**Frequency reducers** (p = frequency, G = world seed, fresh congruential generator):
| Method | Computation |
|---|---|
| `default` | seed_with_salt(G, **salt**, **cx**, **cz** as the salt) — argument order shifted: seed = salt·341873128712 + cx·132897987541 + G + cz; true if `next_float() < p` |
| `legacy_type_1` (outposts) | reseed((cx>>4) XOR ((cz>>4) << 4) XOR G); next_int(); true if `next_int((int)(1/p)) = 0` |
| `legacy_type_2` (treasures) | seed_with_salt(G, cx, cz, 10387320); `next_float() < p` |
| `legacy_type_3` (mineshafts) | large_seed(G, cx, cz); `next_double() < p` |
(`reseed` on the congruential wrapper = legacy scrambling `(s XOR 0x5DEECE66D) AND (2⁴⁸−1)`.)

**Flat world** (`flat` generator): structure sets = `structure_overrides` of the settings
(list of identifiers, one identifier or a tag `#…`, in this order; absent: all those of the
registry), then only those of which a structure accepts the single biome. World "flat" preset:
strongholds + villages; "Classic Flat" of the creation screen: villages. Seen by structures,
the generator starts at y 0 over a depth of 384, sea at −63; base height = 1 + y of the
highest layer that meets the heightmap criterion (world bottom if none); base column =
the layers from the world bottom, air above. The terrain has no adaptation (no
"beard"). Without decoration (`features` false), only the structures are placed at the steps, with
the features their pieces call (village trees). Strongholds and mineshafts,
lowered "under the sea" (−63) with this generator bottom at 0, stay above the flat ground.

**`concentric_rings`** (`distance`, `spread`, `count`, `preferred_biomes`): placement chunk
⇔ it appears in the list of ring positions, computed once per dimension:
```
if count = 0 : empty list
r = congruential ; r.reseed(ring_seed)        (ring_seed = world seed ; 0 for a flat world)
a = r.next_double() · 2π
k = 0 (position in the circle) ; c = 0 (circle) ; s = spread
for i from 0 to count−1 :
    d = 4·distance + distance·c·6 + (r.next_double() − 0.5)·(distance·2.5)
    x0 = round(cos(a)·d) ; z0 = round(sin(a)·d)          (exact standard-library maths, rounding half towards +∞)
    r_i = r.fork()   = new congruential with seed r.next_long()
    position_i = biome_search(centre (16·x0 + 8, 0, 16·z0 + 8), radius 112, preferred, r_i)
                 → chunk of the block found, otherwise (x0, z0)
    a += 2π / s
    k += 1
    if k = s : c += 1 ; k = 0 ; s += 2·s/(c+1) ; s = min(s, count − i) ; a += r.next_double()·2π
```
`biome_search(x, y, z, R, predicate, random)` (RAW biomes, resolver with cache): qx =
x>>2, qz = z>>2, qR = R>>2 (= 28), qy = y>>2; for dz from −qR to qR, for dx from −qR to qR (z
outer): biome at (qx+dx, qy, qz+dz); if it satisfies the predicate: if no result yet
→ result = this point (no draw); otherwise if `random.next_int(found + 1) = 0` → result = this
point; found += 1. Result in blocks = (4·(qx+dx), y, 4·(qz+dz)). (Reservoir sampling:
the retained point is uniform among the valid points.)
The computation of the 128 positions is done in parallel by the game but each position has its own
generator forked in order: deterministic result.

**`dimension_origin`**: placement chunk ⇔ the dimension's origin chunk (0,0).

### 7.3 Start in a chunk (status `structure_starts`)

```
for each possible set E (registry order) :
    if a structure of E already has a valid start in this chunk → skip
    if not is_structure_chunk(cx, cz) → skip
    if E has a single structure : attempt(it)
    otherwise : random = congruential ; large_seed(G, cx, cz) ; options = copy of the list ; T = sum of the weights
        while options non-empty :
            t = next_int(T) ; i = first index such that (t − Σ weights up to i inclusive) < 0
            if attempt(options[i]) → end of E
            T −= weight ; remove options[i]
attempt(S) : start = S.generate(…) (§ 8) ; if it is valid (at least one piece) → record it
    in the chunk (references = those of a previous start of S if there was one) ; true
```
Generation context of a structure (§ 8): random = congruential generation generator
initialised by `large_seed(G, cx, cz)`; valid biome = the RAW biome (no zoom) at the starting
point (x>>2, y>>2, z>>2) belongs to the structure's `biomes`.

### 7.4 References (status `structure_references`)

For chunk C: for sx from cx−8 to cx+8, for sz from cz−8 to cz+8 (sx outer): for
each valid start of chunk (sx, sz) whose bounding box (widened by 12 if the
structure has a terrain adaptation ≠ `none`) intersects the columns of C: add the key
`(sx AND 0xFFFFFFFF) | ((sz AND 0xFFFFFFFF) << 32)` to C's reference set for
this structure. These sets are open-addressing hash sets of longs (fastutil): their
iteration order (which fixes the placement order of two starts of the same structure in a
chunk, § 2.3) is:
- table of size 32 at the start (then doubled when the size reaches 0.75 × table size);
- slot = `mix(k) AND (size − 1)`, linear probing towards increasing indices (modulo);
  `mix(k)`: h = k × 0x9E3779B97F4A7C15; h ^= h >>> 32; h ^= h >>> 16;
- key 0 (chunk (0,0)) is stored separately and comes out FIRST;
- iteration of the slots by DECREASING index;
- on resizing, reinsertion of all the keys.
The starts are saved: a loaded world (not regenerated) re-reads the order from the
file (see `generation-terrain.md` / saves), which may differ — without effect
as long as two starts of the same structure do not overlap.

## 8. Start per structure type

Each type provides "find a generation point": either nothing, or (starting position,
piece builder). The starting position must be in a valid biome (§ 7.3) otherwise
nothing. The pieces built form the start; no piece → invalid start.
"Ground height (x, z, map)" = first FREE height given by the terrain generator
WITHOUT decoration ("base height" function of `generation-terrain.md`); "occupied height" =
this − 1. `cx`, `cz` = chunk; random = structure generator (§ 7.3).

Types (`type`): `jigsaw`, `buried_treasure`, `desert_pyramid`, `end_city`, `fortress`,
`igloo`, `jungle_temple`, `mineshaft`, `nether_fossil`, `ocean_monument`, `ocean_ruin`,
`ruined_portal`, `shipwreck`, `stronghold`, `swamp_hut`, `woodland_mansion`.

### 8.1 `jigsaw` (villages, outposts, bastions, ancient cities, trail ruins, trial chambers, abandoned camps)

Fields: `start_pool`, optional `start_jigsaw_name`, `size` (max depth 0..20),
`start_height` (height provider), `use_expansion_hack`, optional `project_start_to_heightmap`,
`max_distance_from_center` (integer 1..128 or `{horizontal, vertical}`; vertical
default = total world height), `pool_aliases` ([]), `dimension_padding` (0 or
`{bottom, top}`), `liquid_settings` (`apply_waterlogging`). Constraint: horizontal + (12 if
adaptation ≠ none) ≤ 128.

```
h = start_height.sample(random, context) ; P = (16·cx, h, 16·cz)
alias = alias resolution (§ 8.1.2) with P
rot = [none, 90 cw, 180, 90 ccw][next_int(4)]
pool = alias(start_pool) ; e = pool.random_element(random)       (unrolled weighted list, next_int(n) ; empty → empty element)
if e empty → nothing
if start_jigsaw_name : A = first jigsaw block with this name among
    e.shuffled_jigsaws(P, rot, random) ; none → nothing ; otherwise A = P
pos = P − (A − P)
central piece C = (e, pos, ground = e.ground_delta (1), rot, box = e.box(pos, rot))
cx_ = (C.x_max + C.x_min) / 2 ; cz_ = (C.z_max + C.z_min) / 2         (Java integer division)
if project_start_to_heightmap :
    if no valid biome cell in the column (cx_, cz_) from min_y to max_y → nothing
    bottom = P.y + ground_height(cx_, cz_, map)
otherwise bottom = pos.y
C is shifted vertically by bottom − (C.y_min + ground_delta)
if padding ≠ 0 and (C.y_min < min_y + bottom or C.y_max > max_y − top) → nothing
start = (cx_, bottom + (A.y − P.y), cz_)                            (serves the biome test)
builder : pieces = [C] ; if size > 0 : assembly (§ 8.1.1) with the initial
    free region = box [cx_ − H, cx_ + H + 1] × [max(cy − V, min_y + bottom), min(cy + V + 1,
    max_y + 1 − top)] × [cz_ − H, cz_ + H + 1] (real coordinates, cy = start.y) MINUS the
    box of C
```

#### 8.1.1 Assembly

Free region = a volume (a box minus a list of removed boxes). A box "fits" if,
shrunk by 0.25 on each face, it lies entirely in the free volume (so within the box
and overlapping none of the removed boxes — contact is allowed).

Priority queue: the entry of highest priority (`placement_priority`
of the jigsaw block that created it) is always extracted; at equal priority, insertion order (first in, first
out). C is processed (depth 0, context region), then while the queue is not empty.

```
process(S, ctx_free, depth) :
    rigidS = (S.element.projection = rigid) ; bS = box of S ; yS = bS.y_min
    free_S = nothing (created on demand = volume of bS alone)
    for each jigsaw J of S.element.shuffled_jigsaws(S.pos, S.rot, random) :
        dir = front face of J ; target = J.pos + dir ; yJ = J.pos.y − yS ; base = NOT COMPUTED
        pool = alias(J.pool) ; missing from the registry → next
        pool empty (and not `empty`) → next ; fallback = pool.fallback ; fallback empty (and not `empty`) → next
        inside = bS contains target
        free = inside ? (free_S, created = bS if it does not exist) : ctx_free
        candidates = (depth ≠ size ? shuffle(unrolled pool) : []) + shuffle(unrolled fallback)
        for each element E of candidates :
            if E is the empty element → STOP the list (next jigsaw)
            for each rotation r of shuffle([none, 90 cw, 180, 90 ccw]) :
                JT = E.shuffled_jigsaws(origin, r, random)
                hack = (use_expansion_hack and height of E.box(origin, r) ≤ 16) ?
                       max over the JT whose target (pos + front) is INSIDE E.box(origin, r) of
                       max(max_size(pool of this jigsaw), max_size(its fallback)) : 0
                       (max_size of a pool = greatest box height of its non-empty
                       elements at zero rotation; missing pool → 0)
                for each jigsaw T of JT :
                    if not attachable(J, T) → next
                    raw = target − T.pos ; bB = E.box(raw, r)
                    rigidE = (E.projection = rigid) ; yT = T.pos.y
                    dY = yJ − yT + dir.y
                    if rigidS and rigidE : yB = yS + dY
                    otherwise : if base not computed : base = ground_height(J.pos.x, J.pos.z, WORLD_SURFACE_WG)
                            yB = base − yT
                    shift = yB − bB.y_min ; bT = shifted bB ; posT = raw + (0, shift, 0)
                    if hack > 0 : n = max(hack + 1, bT.y_max − bT.y_min) ; bT.y_max = max(bT.y_max, bT.y_min + n)
                    if bT does not fit in free → next
                    remove bT from free
                    groundT = rigidE ? (S.ground − dY) : E.ground_delta
                    piece T' = (E, posT, groundT, r, bT)
                    yj = rigidS ? yS + yJ : (rigidE ? yB + yT : base + dY/2)    (base computed if needed)
                    S.junctions += (target.x, yj − yJ + S.ground, target.z, dY, projection of E)
                    T'.junctions += (J.pos.x, yj − yT + groundT, J.pos.z, −dY, projection of S)
                    pieces += T'
                    if depth + 1 ≤ size : queue += (T', free, depth + 1) with priority J.placement_priority
                    → next jigsaw of S (we leave the E/r/T loops)
attachable(J, T) : front(J) = opposite(front(T)) AND (J rollable OR up(J) = up(T)) AND J.target = T.name
```
`shuffled_jigsaws`: simple element: the palette's jigsaws (palette chosen by the position
seed of `pos`, § 10.2), positions transformed + pos, states rotated; shuffle with the random source;
then STABLE sort by decreasing `selection_priority`. List: those of the first element.
"Feature" element (`feature_pool_element`): a single jigsaw at pos (front = down, up =
south, rollable, empty name, pool `empty`, empty target), no shuffle. Empty: none. A
jigsaw WITHOUT a name accepts any target (the test "target = name" only applies if the name exists).
Boxes: simple = box of the rotated template (§ 10.2); list = bounding box of the non-empty elements;
feature = (pos, pos); empty: never requested.
`ground_delta` = 1 for all elements.

#### 8.1.2 Pool aliases

If `pool_aliases` is empty: identity. Otherwise alias_random = congruential(world seed) →
positional factory (seed = next_long()) → random at P (§ 10.4 `capped`: same construction).
For each binding in order: `direct` (`alias`, `target`); `random` (`alias`,
weighted `targets`: one draw); `random_group` (weighted `groups`: one draw, then each
binding of the chosen group, recursively).

#### 8.1.3 Pool elements (`element_type`)

| Type | Fields | Placement (§ 9.2) |
|---|---|---|
| `single_pool_element` | `location`, `processors`, `projection` (`rigid`/`terrain_matching`), optional `override_liquid_settings` | template with settings: clipping box = chunk area, rotation, known shape, entities included and finalised, liquids (override, otherwise those of the structure); processors in order: ignore structure blocks, jigsaw replacement, those of the element, then those of the projection (`terrain_matching`: gravity `WORLD_SURFACE_WG` offset −1). Random source of the settings NOT fixed (position seeds). Flags 18. Then processing of the data markers (no effect for this element) |
| `legacy_single_pool_element` | same | same but the first processor also ignores AIR (the template's air cells replace nothing) |
| `list_pool_element` | `elements`, `projection` | places each element in order, stopping at the first failure |
| `feature_pool_element` | `feature` (placed feature), `projection` | places the placed feature at pos with the placement random source |
| `empty_pool_element` | — | nothing |

Pool (`worldgen/template_pool`): `fallback` (pool), `elements` [{`element`, `weight` 1..150}];
unrolled list = each element repeated `weight` times, in order.

### 8.2 Coded template pieces: common rules

A "template piece" (ruined portal, igloo, shipwreck, underwater ruin, Nether fossil,
End city, mansion) keeps: template name, placement settings (§ 10.2, often WITHOUT a fixed
random source → position seeds), template position. Its box = box of the transformed template.
Placement in a chunk (§ 9.2): clipping box = chunk area; template placement with
the step random source (flags 2); if the placement succeeded: for each structure block of the template
(palette chosen, positions transformed, within the clipping box) in DATA mode → processing of the
type-specific marker (with the step random source); then each jigsaw block of the template
(same filter) is replaced by its `final_state` ("place and update").

Starts "on the chunk centre" ("on the centre"): point = (16cx+8, occupied height(16cx+8,
16cz+8, map), 16cz+8); if the type requires the check, at least one biome cell of the column
(from min_y−1 to max_y, column resolver) must first be valid.

### 8.3 Ruined portal (`ruined_portal`)

Field `setups`: list of {`placement` (`on_land_surface`, `partly_buried`, `on_ocean_floor`,
`in_mountain`, `underground`, `in_nether`), `air_pocket_probability`, `mossiness`, `overgrown`,
`vines`, `can_be_cold`, `replace_with_blackstone`, `weight`}.
```
if several setups : T = Σ weights (float) ; t = next_float() ; first s such that (t −= weight/T) < 0
pocket = (p = 0 → false ; p = 1 → true ; otherwise next_float() < p)
if next_float() < 0.05 : template = ruined_portal/giant_portal_{1..3}[next_int(3)]
otherwise template = ruined_portal/portal_{1..10}[next_int(10)]
rot = 4 rotations[next_int(4)] ; mirror = next_float() < 0.5 ? none : front_back
pivot = (size.x/2, 0, size.z/2) ; base = (16cx, 0, 16cz)
B = box(base, rot, pivot, mirror) ; c = centre(B)
surface = ground_height(c.x, c.z, map) − 1     map = OCEAN_FLOOR_WG if on_ocean_floor otherwise WORLD_SURFACE_WG
y = choose_y(…) ; origin = (16cx, y, 16cz)
choose_y :
    ymin = min_y + 15
    in_nether : pocket ? U(32,100) : (next_float() < 0.5 ? U(27,29) : U(29,100))
    in_mountain : U'(70, surface − height(B))   ; underground : U'(ymin, surface − height(B))
    partly_buried : surface − height(B) + U(2, 8)  ; others : surface
    (U(a,b) = a + next_int(b−a+1) ; U'(a,b) = a < b ? U(a,b) : b)
    terrain columns (without decoration) at the 4 bottom corners of B (x_min,z_min), (x_max,z_min), (x_min,z_max), (x_max,z_max)
    for y' from y down to ymin+1 : if at least 3 corners have at y' a block "opaque for the map"
        (OCEAN_FLOOR_WG if on_ocean_floor, otherwise WORLD_SURFACE_WG) → return y'
    return ymin
cold = can_be_cold and the RAW biome at the origin is "cold enough to snow" at the origin (§ 6.2)
```
Placement settings (no fixed random source): rotation, mirror, pivot; processors: ignore
structure blocks (and air if NO air pocket); rules: gold block → air with probability 0.3
(`random_block_match`); lava → magma block (on the ocean floor) / → netherrack (if cold) /
→ magma block with probability 0.2 (otherwise); if not cold: netherrack → magma block with probability 0.07;
then `block_age` (mossiness), `protected_blocks` (`#features_cannot_replace`),
`lava_submerged_block`, and `blackstone_replace` if requested.

Placement: only in the chunk that contains the CENTRE of the box; the clipping box is
then extended to the whole portal box (the entire portal is placed at once). Then, with the
step random source:
1. Netherrack spreading: follow = (on_land_surface or on_ocean_floor); c = centre;
   P = [1, 1, 1, 1, 1, 1, 1, 0.9, 0.9, 0.8, 0.7, 0.6, 0.4, 0.2] (14 values); L = (x width +
   z width)/2; a = next_int(max(1, 8 − L/2)); for x from c.x−14 to c.x+14, z from c.z−14 to c.z+14
   : d = |x−c.x| + |z−c.z| + a; if d < 14 and next_double() < P[d]: s = map(x,z) − 1; y =
   follow ? s : min(y_min, s); if |y − y_min| ≤ 3 and replaceable(x,y,z): stone-or-magma;
   if overgrown: leaves above; drip column under (x, y−1, z).
2. Drip columns under the portal: for x from x_min+1 to x_max−1, z from z_min+1 to z_max−1: if
   (x, y_min, z) is netherrack → drip column at (x, y_min−1, z).
3. If vines or overgrown: for each position of the box (order x, y, z): vines: if block neither
   air nor vine: d = drawn horizontal direction; if neighbour d = air and face d full → vine
   attached; overgrown: if next_float() < 0.5 and block = netherrack and air above →
   persistent jungle leaves above.
Stone-or-magma(q): if replaceable: (not cold and next_float() < 0.07) ? magma block : netherrack
(place and update). Drip column(q): stone-or-magma(q); up to 8 times as long as
next_float() < 0.5: go down, stone-or-magma. Replaceable = neither air, nor obsidian, nor
`#features_cannot_replace`, and (in the Nether or not lava).

### 8.4 Igloo, shipwreck, underwater ruins, Nether fossil

**Igloo** ("on the centre", `WORLD_SURFACE_WG`, with check): P = (16cx, 90, 16cz); rot drawn.
If next_double() < 0.5: n = next_int(8) + 4; piece `igloo/bottom` at depth 3n; for i <
n−1: `igloo/middle` at depth 3i; then `igloo/top` (depth 0). Pivots: top (3,5,5),
middle (1,3,1), bottom (3,6,7); offsets: top 0, middle (2,−3,4), bottom (0,−3,−2);
position = P + offset − (0, depth, 0). Settings: rotation, pivot, ignore structure
blocks, liquids ignored. Placement: entrance = position + transformed((3 − off.x, 0, −off.z));
h = WORLD_SURFACE_WG(entrance); the piece is placed shifted by (0, h − 90 − 1, 0); for the
top: trapdoor at position + transformed((3,0,5)): if the block below is neither air nor
ladder → snow block. "chest" marker: air, and the chest below receives
`chests/igloo_chest` (seed next_long()).

**Shipwreck** ("on the centre", `WORLD_SURFACE_WG` if beached otherwise `OCEAN_FLOOR_WG`, with check):
rot drawn; P = (16cx, 90, 16cz); template = list[next_int(n)], lists (prefix
`shipwreck/`, ORDER significant):
- beached (11): `with_mast`, `sideways_full`, `sideways_fronthalf`, `sideways_backhalf`,
  `rightsideup_full`, `rightsideup_fronthalf`, `rightsideup_backhalf`, `with_mast_degraded`,
  `rightsideup_full_degraded`, `rightsideup_fronthalf_degraded`, `rightsideup_backhalf_degraded`;
- ocean (20): `with_mast`, `upsidedown_full`, `upsidedown_fronthalf`, `upsidedown_backhalf`,
  `sideways_full`, `sideways_fronthalf`, `sideways_backhalf`, `rightsideup_full`,
  `rightsideup_fronthalf`, `rightsideup_backhalf`, `with_mast_degraded`,
  `upsidedown_full_degraded`, `upsidedown_fronthalf_degraded`, `upsidedown_backhalf_degraded`,
  `sideways_full_degraded`, `sideways_fronthalf_degraded`, `sideways_backhalf_degraded`,
  `rightsideup_full_degraded`, `rightsideup_fronthalf_degraded`, `rightsideup_backhalf_degraded`. Settings: rotation,
pivot (4,0,15), ignore structure and air. If the template exceeds 32 in x or in y: height
fixed from the start (beached: min of the 4 terrain corners − size.y/2 − next_int(3); otherwise
average of the occupied heights at the 4 corners). Otherwise, at the first placement: over the rectangle
position..position+(size.x−1, 0, size.z−1) (not rotated!) read the map (WORLD_SURFACE_WG or
OCEAN_FLOOR_WG): integer average and minimum; y = beached ? min − size.y/2 − next_int(3) :
average. Markers: `map_chest` → `chests/shipwreck_map`, `treasure_chest` →
`chests/shipwreck_treasure`, `supply_chest` → `chests/shipwreck_supply` (container below,
seed next_long()).
Trap: the height adjustment is memorised in the piece at the first chunk placed; the other
chunks reuse the same height (the piece is shared) — but if the world is reloaded
between two chunks, the "adjusted" state is re-read from the save.

**Underwater ruins** ("on the centre", `OCEAN_FLOOR_WG`, with check): P = (16cx, 90, 16cz);
rot drawn; big = next_float() ≤ `large_probability`; integrity = big ? 0.9 : 0.8;
piece(P, rot, big, integrity); if big and next_float() ≤ `cluster_probability`: cluster.
Lists (prefix `underwater_ruin/`, order significant): warm `warm_1`…`warm_8`;
big warm `big_warm_4`…`big_warm_7`; bricks `brick_1`…`brick_8`, cracked
`cracked_1`…`cracked_8`, mossy `mossy_1`…`mossy_8`; big: `big_brick_{1,2,3,8}`,
`big_cracked_{1,2,3,8}`, `big_mossy_{1,2,3,8}`.
Piece: warm → template drawn (`next_int`) from the small or big warm list; cold → i = next_int(n) in the "bricks" list (small or big), and three
pieces stacked at the same place: bricks[i] (integrity), cracked[i] (0.7), mossy[i]
(0.5). Settings: rotation, `block_rot` (integrity), ignore structure and air, then capped
(`capped`, limit 5) with a rule sand → suspicious sand (warm) / gravel → suspicious gravel
(cold) with `append_loot` `archaeology/ocean_ruin_warm|cold`.
Cluster: parent corner = transformed((15,0,15), rot) + (P.x, 90, P.z); parent box; bottom-left =
min of the x/z; 8 candidate positions relative to the bottom-left (`between` draws):
(−16+[1,8], 16+[1,7]), (−16+[1,8], [1,7]), (−16+[1,8], −16+[4,8]), ([1,7], 16+[1,7]),
([1,7], −16+[4,6]), (16+[1,7], 16+[3,8]), (16+[1,7], [1,7]), (16+[1,7], −16+[4,8]) (x then z
for each, in that order); n = between(4, 8); n times (as long as positions remain):
q = remove(positions, next_int(size)); r = drawn rot; box from q to q + transformed((5,0,6), r)
; if it does not intersect the parent box → small piece (integrity 0.8) at q, r.
Placing a piece: y = OCEAN_FLOOR_WG(position); then correction: over the rectangle position..
rotated corner, for each column go down from y−1 while air/water/ice (and > min_y+1) →
floor; min = smallest floor; area = number of columns with floor < y − 3; if
(y − 1) − min > 2 and area > |position.x − corner.x| − 2 → y = min + 1. Markers: "chest" →
chest (waterlogged if water) `chests/underwater_ruin_big|small` (seed next_long()); "drowned" →
persistent drowned + air (above sea level) or water.

**Nether fossil**: x = 16cx + next_int(16), z = 16cz + next_int(16); y = `height`.sample();
terrain column (without decoration) at (x, z); while y > sea_level: b = block(y), d =
block(y−1); y−−; if b is air and (d = soul sand or top face of d full) → stop. If
y ≤ sea_level → nothing. Start (x, y, z); rot drawn; template `nether_fossils/fossil_{1..14}`
[next_int(14)]; settings: rotation, ignore structure and air. Placement: the clipping box is
extended to the whole fossil box (each chunk that references it places it entirely);
then "dried ghast": random = congruential(world seed) → positional factory →
random at the centre of the box; if next_float() < 0.5: q = (x_min + next_int(x width), y_min, z_min +
next_int(z width)); if q is air and in the clipping box → dried ghast block
rotated by a drawn rotation.

### 8.5 End city (`end_city`)

Start: (16cx+7, ·, 16cz+7); a valid biome cell is needed in the column; rot
drawn; y = min of the occupied heights (`WORLD_SURFACE_WG`) at the corners of a 5×5 square (x, x+dx),
(z, z+dz) with (dx, dz) = (5,5) / rot 90 cw (−5,5) / 180 (−5,−5) / 90 ccw (5,−5); if y < 60 →
nothing.

Pieces = templates `end_city/<name>`; settings: entities IGNORED, ignore structure blocks
("overwrite") or structure + air ("no overwrite"), rotation. Adding a child piece to a
parent with a local offset o: position = parent position + (transformed_parent(o) −
transformed_child((0,0,0))) (§ 10.2, null pivots).
```
start : base_floor (overwrites) at P ; second_floor_1 at (−1,0,−1) (does not overwrite) ; third_floor_1 at
    (−1,4,−1) (does not overwrite) ; third_roof at (−1,8,−1) ; children(TOWER, depth 1, last)
children(G, depth, parent, o) : if depth > 8 → failure
    L = [] ; if G.generate(depth, parent, o, L) :
        tag = next_int() ; each piece of L receives "depth" = tag
        if a piece of L intersects an already accepted piece whose depth ≠ the parent's → failure
        otherwise accept L → success
    failure
TOWER(parent) : base = tower_base at (3+next_int(2), −3, 3+next_int(2)) ; tower_piece at (0,7,0) ;
    bridge = next_int(3) = 0 ? last : none ; h = 1 + next_int(3)
    for i < h : tower_piece at (0,4,0) ; if i < h−1 and next_boolean() : bridge = last
    if bridge : for (r, o) in [(none,(1,−1,0)), (90 cw,(6,−1,1)), (90 ccw,(0,−1,5)), (180,(5,−1,6))] :
                 if next_boolean() : b = bridge_end (parent rotation + r) at o from bridge ; children(BRIDGE, depth+1, b)
              tower_top at (−1,4,−1)
    otherwise : if depth ≠ 7 → return children(FAT_TOWER, depth+1, last) ; otherwise tower_top at (−1,4,−1)
    success
BRIDGE(parent) : n = next_int(4) + 1 ; bridge_piece at (0,0,−4) (depth −1) ; y = 0
    n times : if next_boolean() : bridge_piece at (0,y,−4) ; y = 0
             otherwise : next_boolean() ? bridge_steep_stairs at (0,y,−4) : bridge_gentle_stairs at (0,y,−8) ; y = 4
    if no ship yet (state reset at the start of each city) and next_int(10 − depth) = 0 :
        ship at (−8 + next_int(8), y, −70 + next_int(10)) ; ship created
    otherwise if not children(HOUSE, depth+1, last, (−3, y+1, −11)) → failure
    bridge_end (rotation + 180) at (4, y, 0) (depth −1) ; success
HOUSE(parent, o) : if depth > 8 → failure ; base_floor (overwrites) at o ; k = next_int(3)
    k=0 : base_roof at (−1,4,−1)
    k=1 : second_floor_2 (−1,0,−1) ; second_roof (−1,8,−1) ; children(TOWER, depth+1)
    k=2 : second_floor_2 (−1,0,−1) ; third_floor_2 (−1,4,−1) ; third_roof (−1,8,−1) ; children(TOWER, depth+1)
    success
FAT_TOWER(parent) : fat_tower_base at (−3,4,−3) ; fat_tower_middle at (0,4,0)
    for i < 2 while next_int(3) ≠ 0 (test made at each turn) :
        fat_tower_middle at (0,8,0) ; for (r, o) in [(none,(4,−1,0)), (90 cw,(12,−1,4)),
            (90 ccw,(0,−1,8)), (180,(8,−1,12))] : if next_boolean() : bridge_end → children(BRIDGE, depth+1)
    fat_tower_top at (−2,8,−2) ; success
```
"Does not overwrite" (the template's air is ignored): second_floor_1, third_floor_1,
second_floor_2, second_roof, third_floor_2; all the other pieces overwrite (only the structure
blocks are ignored). Pieces are added to the list of their generator
(accepted as a whole or rejected as a whole).
Markers: "Chest…" → chest below (if it is in the clipping box) `chests/end_city_treasure`
(seed next_long()); "Sentry…" → shulker at (x+0.5, y, z+0.5); "Elytra…" → item frame with
elytra facing south, rotated.

### 8.6–8.14 Structures with coded pieces: conventions

Conventions common to the three sections:
- "random" = structure generator (legacy congruential) initialised by large_seed(G, cx, cz) of the origin chunk; a single stream for the whole assembly, consumed in the order described.
- `n(k)` = uniform integer draw in [0, k); `f()` = float [0,1); `b()` = boolean; `L()` = 64-bit integer.
- Bounding box written (x0,y0,z0)–(x1,y1,z1), bounds included. "Collision" = intersection (bounds included) with the box of ANY piece already recorded in the builder (traversal in order of addition, first one found).
- Oriented box "w×h×d" from a foot (x,y,z) and a direction dir: if dir is north/south, the box goes from (x,y,z) to (x+w−1, y+h−1, z+d−1); if dir is east/west, from (x,y,z) to (x+d−1, y+h−1, z+w−1). The "legacy" pieces then apply a local offset (dx,dy,dz) to the foot, transformed according to dir (see 8.7/8.8: "offset oriented box"). For an offset oriented box of foot P, offset (a,b,c), size w×h×d:
  - north: x from P.x+a to P.x+a+w−1; z from P.z−d+1+c… concretely: (P.x+a, P.y+b, P.z−d+1+c)–(P.x+a+w−1, P.y+b+h−1, P.z+c);
  - south: (P.x+a, P.y+b, P.z+c)–(P.x+a+w−1, P.y+b+h−1, P.z+c+d−1);
  - west: (P.x−d+1+c, P.y+b, P.z+a)–(P.x+c, P.y+b+h−1, P.z+a+w−1);
  - east: (P.x+c, P.y+b, P.z+a)–(P.x+c+d−1, P.y+b+h−1, P.z+a+w−1).
  (Formula checked against the decompiled oriented-box utility; note that the offset c is NOT inverted for north/west. Unknown direction → south case.)
- Local coordinates of an oriented piece (x,y,z) → world: world_y = y + y_min; north: (x_min+x, z_max−z); south: (x_min+x, z_min+z); west: (x_max−z, z_min+x); east: (x_min+z, z_min+x). Piece WITHOUT orientation: coordinates passed as is (absolute).
- "Interior" (test used everywhere): the local block (x, y+1, z) converted to world is within the chunk's clipping box AND its y is strictly below the "generation ocean floor" height (OCEAN_FLOOR_WG) at world (x,z). Outside the clipping → false.
- Reading a local block outside the clipping box → returns AIR.
- "maybe box": for EACH position (order y, then x, then z), an `f()` is drawn BEFORE any other test; the position is placed if `f() ≤ p` (and skip-air / interior filters). "maybe block": `f() < p` (draw always made).

Additional conventions (§ 8.9–8.14): "Structure random" = the legacy congruential generator initialised by large_seed(G, cx, cz) (§ starts). "Step random" = the random source passed to the placement of a chunk. Indexed 3D directions: DOWN 0, UP 1, NORTH 2, SOUTH 3, WEST 4, EAST 5. Randomly drawn horizontal directions: `next_int(4)` on the list [NORTH, EAST, SOUTH, WEST]. "Local" coordinates = the piece's frame (primitive "place local block"). "Box (A, B)" = filled-box primitive with edge A and interior B, without "ignore air" unless stated.


### 8.6 Mineshaft (normal and mesa)

#### 8.6.1 Materials per variant

| variant | wood (pillars) | planks | fence |
|---|---|---|---|
| normal | `minecraft:oak_log` | `minecraft:oak_planks` | `minecraft:oak_fence` |
| mesa | `minecraft:dark_oak_log` | `minecraft:dark_oak_planks` | `minecraft:dark_oak_fence` |

Air placed = `minecraft:cave_air`. Chain = `minecraft:iron_chain`.
Replacement filter specific to the mineshaft: a placement never replaces a block that is already planks, wood, fence (of the variant) or chain (neighbouring pieces do not overwrite each other).

#### 8.6.2 Assembly

1. Finding the point: first draw an `f()` ("ritual" draw, result ignored). Reference point (for the biome test) = (chunk centre X, 50, chunk z_min) + final vertical offset.
2. Starting piece = **room** (8.6.4), depth 0, corner (cx·16+2, 50, cz·16+2). Added to the builder, then recursive expansion of its children (no queue: immediate **depth-first** exploration, each piece created expands its children before handing back control).
3. **Creating a next piece** from a foot (x,y,z), a direction dir and a depth p of the parent:
   - if p > 8 → nothing (no draw);
   - if |x − x_min(room)| > 80 or |z − z_min(room)| > 80 → nothing (no draw);
   - otherwise `r = n(100)`: r ≥ 80 → crossing; 70 ≤ r < 80 → stairs; r < 70 → corridor. The box search of the drawn type is made; if it fails (collision) → nothing (no fallback to another type). Otherwise the piece, of depth p+1, is added then immediately expands its children.
4. Final vertical adjustment:
   - normal: "move below sea level" with offset 10 (sea and min_y of the generator);
   - mesa: centre C of the bounding box of all the pieces (min + extent/2 per axis, integer division); S = WORLD_SURFACE_WG height at (C.x, C.z); target = sea if S ≤ sea, otherwise uniform integer in [sea, S] (inclusive; = sea + n(S − sea + 1)); offset = target − C.y applied to all the pieces.

#### 8.6.3 Common placement guard ("invalid location")

Each piece, when placed in a chunk, is entirely skipped if:
- the biome at the centre of (piece box widened by 1, intersected with the clipping) is in the tag `minecraft:mineshaft_blocking`, or
- a **liquid** block is present on one of the six faces of this widened-intersected box (faces y bottom/top, z bottom/top, x bottom/top visited in this order).
No draw is made in this case.

"Floor" (used by corridor and crossing): at a local position, if it is "interior" and the existing block has no solid top face, place the planks.

#### 8.6.4 Room (starting piece, no orientation)

- Box: (X, 50, Z)–(X+7+n(6), 54+n(6), Z+7+n(6)) — draws in the order x_max, y_max, z_max.
- Children: `hs = height − 4` (minimum 1). Four sides in the order north, south, west, east. For each, `pos = 0`; while `pos < extent` (X for north/south, Z for west/east): `pos += n(extent)`; if `pos + 3 > extent` → end of the side; `y = y_min + n(hs) + 1`; creation of a next piece (depth 0):
  - north: foot (x_min+pos, y, z_min−1), dir north; south: (x_min+pos, y, z_max+1), dir south; west: (x_min−1, y, z_min+pos), dir west; east: (x_max+1, y, z_min+pos), dir east.
  If a child is created, memorise an "opening" = the child's box restricted to the thickness 2 of the room's wall (north: z z_min..z_min+1; south: z_max−1..z_max; west: x x_min..x_min+1; east: x_max−1..x_max), over the child's x/z range and y. Then `pos += 4`.
  (The openings follow the structure's vertical shifts.)
- Placement (no draw): air over (x_min, y_min+1, z_min)–(x_max, min(y_min+3, y_max), z_max); for each opening, air over its box limited in y to [openingMaxY−2, openingMaxY]; then upper half-sphere of air over (x_min, y_min+4, z_min)–(x_max, y_max, z_max): position placed if (dx² + dy² + dz²) ≤ 1.05 with dx = (x − cx)/(Lx/2), dz likewise, dy = (y − y0)/Ly, cx = x0 + Lx/2 (32-bit floats). The room's floor (y = y_min) is not touched.

#### 8.6.5 Corridor (oriented)

- Box search: `k = n(3) + 2`; for k decreasing down to 1: length L = 5k, box 3 (wide) × 3 (high) × L (long) starting from the foot towards dir (north: z from foot−L+1 to foot; south: foot to foot+L−1; west/east symmetric in x), without offset; the first one without collision is kept; otherwise failure.
- Creation (draws after the box): `rails = (n(3) == 0)`; `spider = !rails && n(23) == 0` (the 2nd draw only happens without rails); number of sections = L/5.
- Children:
  1. `e = n(4)`, then the end exit (the height `n(3)` is drawn BEFORE the creation call, therefore even if the latter gives up) at y = y_min − 1 + n(3):
     - e ≤ 1: straight on — north (x_min, ·, z_min−1); south (x_min, ·, z_max+1); west (x_min−1, ·, z_min); east (x_max+1, ·, z_min);
     - e = 2: "left" turn — from north: west at (x_min−1, ·, z_min); from south: west at (x_min−1, ·, z_max−3); from west: north at (x_min, ·, z_min−1); from east: north at (x_max−3, ·, z_min−1);
     - e = 3: "right" turn — from north: east at (x_max+1, ·, z_min); from south: east at (x_max+1, ·, z_max−3); from west: south at (x_min, ·, z_max+1); from east: south at (x_max−3, ·, z_max+1).
     Depth passed on = depth of the corridor.
  2. If depth < 8: side branches every 5 blocks, at y = y_min. Corridor along X: for x = x_min+3, while x+3 ≤ x_max, step 5: `s = n(5)`; s = 0 → north at (x, y_min, z_min−1); s = 1 → south at (x, y_min, z_max+1). Corridor along Z: for z = z_min+3, … ≤ z_max: s = 0 → west at (x_min−1, y_min, z); s = 1 → east at (x_max+1, y_min, z). Depth passed on = depth + 1 (so the child will have depth + 2).
- Placement (local coordinates, x 0..2, z 0..Lz with Lz = 5·sections − 1), exact order of the draws:
  1. full air (0,0,0)–(2,1,Lz);
  2. maybe box p = 0.8 of air over (0,2,0)–(2,2,Lz) (3·(Lz+1) draws);
  3. if spider: maybe box p = 0.6 of `minecraft:cobweb` over (0,0,0)–(2,1,Lz), restricted to the interior (6·(Lz+1) draws);
  4. for each section s (z = 2 + 5s):
     a. **support** in z: only if the 3 blocks (0..2, 3, z) are all non-air (world read, outside the clipping = air) — otherwise no draw. Fences at x=0 and x=2 over y 0..1 (west/east side connection respectively, then reoriented); `n(4) == 0` → planks only at (0,2,z) and (2,2,z); otherwise plank beam (0..2, 2, z) then two "maybe blocks" p = 0.05 of wall torch at (1,2,z−1) facing south and at (1,2,z+1) facing north;
     b. cobwebs: 8 attempts in the order (0,2,z−1), (2,2,z−1), (0,2,z+1), (2,2,z+1) at p = 0.1, then (0,2,z−2), (2,2,z−2), (0,2,z+2), (2,2,z+2) at p = 0.05. For each attempt: if the position is not interior → no draw; otherwise `f() < p` AND at least 2 of the 6 neighbours (within the clipping) present a solid face towards the position → cobweb;
     c. `n(100) == 0` → **chest minecart** at (2,0,z−1); then `n(100) == 0` → at (0,0,z+1). Minecart: if the position is in the clipping, air, and the block below is non-air: `minecraft:rail` of north-south shape if `b()` otherwise east-west (shape reoriented by the placement), entity `minecraft:chest_minecart` at the block centre, table `minecraft:chests/abandoned_mineshaft`, loot seed `L()`; otherwise no extra draw;
     d. if spider and spawner not yet placed: `zz = z − 1 + n(3)`; if (1,0,zz) is in the clipping and interior: `minecraft:spawner` placed directly (no filter), entity `minecraft:cave_spider` (no draw: the potentials list is empty), and the "placed" flag becomes true.
  5. floor: for x 0..2, z 0..Lz, "floor" at y = −1 (no draw);
  6. double pillars under the 1st section (z = 2) and, if sections > 1, under the last (z = Lz − 2): for x = 0 then x = 2, if the block (x,−1,z) is a plank of the variant → **pillar or chain** (below);
  7. if rails: for z 0..Lz, if the block (1,−1,z) is not air and is a full opaque block: "maybe block" of north-south rail at (1,0,z) with p = 0.7 if interior, 0.9 otherwise.
- **Pillar or chain** (no draw) from the world position W of the plank block: for d = 1, 2, … alternating down then up:
  - down (while active): block B at W.y − d; "empty" = replaceable by structure (air, liquid, glow lichen, seagrass, tall seagrass) and not lava; if non-empty and top face solid → wood column from W.y−d+1 to W.y−1, end; down stays active if d ≤ 20, empty, and y > min_y + 1;
  - up (while active): block H at W.y + d; if not replaceable and can support at the centre of its bottom face and is not a falling block → fence (variant) at W.y+1, chain from W.y+2 to W.y+d−1, end; up stays active if d ≤ 50, replaceable, and y < max_y.
  These placements go directly to the world (without reorientation or filter), only if W is in the clipping.
- Note: the corridor's own "column down" differs from the general primitive (goes down to the first non-replaceable block then back up placing, only if this block has a solid top face). It is called by no corridor placement in this version (vestige).

#### 8.6.6 Crossing (no orientation of its own; keeps the arrival direction)

- Search: `n(4) == 0` → height 7 (y 0..6, two floors); otherwise height 3 (y 0..2). Box 5×h×5 according to dir: north (−1,0,−4)–(3,h−1,0); south (−1,0,0)–(3,h−1,4); west (−4,0,−1)–(0,h−1,3); east (0,0,−1)–(4,h−1,3), translated by the foot. Collision → failure. Two floors ⇔ height > 3.
- Children (depth passed on = that of the crossing), in order:
  - arrival north: north (x_min+1, y_min, z_min−1); west (x_min−1, y_min, z_min+1); east (x_max+1, y_min, z_min+1);
  - arrival south: south (x_min+1, y_min, z_max+1); west; east (same feet);
  - arrival west: north; south (x_min+1, y_min, z_max+1); west;
  - arrival east: north; south; east.
  - if two floors, four successive `b()` at y = y_min + 4: north (x_min+1, ·, z_min−1), west (x_min−1, ·, z_min+1), east (x_max+1, ·, z_min+1), south (x_min+1, ·, z_max+1) — each true `b()` launches a creation.
- Placement (absolute coordinates, no draw): "+" shape of air (two boxes: x reduced by 1 on each side over the whole depth, z reduced by 1 over the whole width), over y y_min..y_min+2 and y_max−2..y_max if two floors (plus the intermediate ceiling y_min+3 emptied inside x+1..x−1, z+1..z−1), otherwise over the whole height. Four plank posts at the inner corners (x_min+1/x_max−1, z_min+1/z_max−1) from y_min to y_max, only if the block above (y_max+1) is not air. Then "floor" over the whole box at y = y_min − 1.

#### 8.6.7 Stairs (oriented)

- Box: 3 wide, 8 high (y −5..2 relative to the foot), 9 long, towards dir (north (0,−5,−8)–(2,2,0), etc.). Collision → failure.
- Child: a single exit at the low end: north (x_min, y_min, z_min−1); south (x_min, y_min, z_max+1); west (x_min−1, y_min, z_min); east (x_max+1, y_min, z_min).
- Placement (local, no draw): air (0,5,0)–(2,7,1) (upper landing), air (0,0,7)–(2,2,8) (lower landing), and for i = 0..4 a slice of air at z = 2+i from y = 5−i−(1 if i<4) to 7−i. No steps placed (the natural ground forms the staircase).

#### 8.6.8 Measurement still to do

The mineshaft is entirely described above (simple geometry); only the exact order of the two-floor crossing's boxes deserves a check on the bench (no randomness involved).

### 8.7 Nether fortress

#### 8.7.1 Materials and conventions

Bricks = `minecraft:nether_bricks`; fence = `minecraft:nether_brick_fence` (explicit connections noted N/S/E/W, reoriented at placement); stairs = `minecraft:nether_brick_stairs`; air = `minecraft:air` (not cave_air). No location guard (unlike the mineshaft) and no replacement filter: any placement in the clipping overwrites.
"Column down" = general primitive: from the local block (x, −1, z), place bricks going down as long as the block is replaceable by structure (air, liquid, lichen, seagrass) and y > min_y + 1.
Chests: reoriented chest, table `minecraft:chests/nether_bridge`.
A piece is accepted ("valid box") if y_min of its box > 10 AND no collision.

#### 8.7.2 Weight tables

Two lists (copied into the starting piece, counters reset at each assembly); "max" = maximum number of placements (0 = unlimited); "in a row" = the same piece may be chosen twice in a row.

Bridge (outdoor):

| piece | weight | max | in a row |
|---|---|---|---|
| straight bridge | 30 | 0 | yes |
| bridge crossing | 10 | 4 | no |
| crossroads room | 10 | 4 | no |
| stairs room | 10 | 3 | no |
| spawner throne | 5 | 2 | no |
| castle entrance | 5 | 1 | no |

Castle (indoor):

| piece | weight | max | in a row |
|---|---|---|---|
| small corridor | 25 | 0 | yes |
| small corridor crossing | 15 | 5 | no |
| right turn | 5 | 10 | no |
| left turn | 5 | 10 | no |
| stair corridor | 10 | 3 | yes |
| T balcony | 7 | 2 | no |
| nether wart room | 5 | 2 | no |

The "last piece chosen" is ONE variable shared by both lists (memorised in the starting piece).

#### 8.7.3 Assembly

1. Reference point of the biome test: (chunk x_min, 64, chunk z_min) — not shifted.
2. Start: direction = [north, east, south, west][n(4)]; starting piece = bridge crossing (8.7.4) of box 19×10×19 placed at (cx·16+2, 64, cz·16+2)–(+18, 73, +18), depth 0, oriented according to the drawn direction. It immediately expands its children (like a bridge crossing).
3. Queue: each piece created by a child is added to the builder AND to the queue. While the queue is not empty: `i = n(size)`, remove element i (the following ones shift), expand its children.
4. End: "move between heights" 48..70.

**Creating a child** (foot P, direction dir, depth p of the parent, bridge or castle list):
- If |P.x − x_min(start)| > 112 or |P.z − z_min(start)| > 112: try a **bridge end** (8.7.5) at the foot; if it is created (and therefore draws its 32-bit `n()`), it is **thrown away** (neither added to the builder nor to the queue). End.
- Otherwise, weighted choice with depth q = p + 1:
  - total weight T = sum of the weights of ALL the entries still in the list; if no entry with "max > 0" still has margin → T = −1;
  - if T > 0 and q ≤ 30: up to 5 attempts. Attempt: `w = n(T)`; go through the list in order, `w -= weight`; at the first entry where w < 0 (and, trap, at ALL the following ones since w stays negative):
    - if the entry is exhausted, or if it is the last chosen and does not allow "in a row" → attempt finished (exit from the traversal);
    - otherwise try to create the corresponding piece; success → counter +1, becomes "last chosen", removed from the list if exhausted, returned; failure (invalid box) → **continue the traversal** with the next entry of the list (which also has w < 0, same tests).
  - after 5 unsuccessful attempts (or if T ≤ 0, or q > 30): try a bridge end at the foot (depth q).
  - the piece returned (including a bridge end) is added to the builder and to the queue.
- Feet of the exits of an oriented piece (offsets of the parent piece):
  - **forward (xo, yo)**: north (x_min+xo, y_min+yo, z_min−1); south (x_min+xo, y_min+yo, z_max+1); west (x_min−1, y_min+yo, z_min+xo); east (x_max+1, y_min+yo, z_min+xo); same direction as the parent;
  - **left (yo, zo)**: parent north or south → dir west, foot (x_min−1, y_min+yo, z_min+zo); parent west or east → dir north, foot (x_min+zo, y_min+yo, z_min−1);
  - **right (yo, zo)**: parent north or south → dir east, (x_max+1, y_min+yo, z_min+zo); parent west or east → dir south, (x_min+zo, y_min+yo, z_max+1).
  ("Left/right" are therefore fixed in the world, not relative to the direction of travel.)

#### 8.7.4 Bridge pieces

Box notation: "w×h×d, offset (a,b,c)" = offset oriented box of the common conventions (at the head of this draft).

**Bridge crossing** — 19×10×19, offset (−8,−3,0). Children (bridge list): forward (8,3), left (3,8), right (3,8). Placement without randomness: cross-shaped decks (bricks y 3..4, widths x 7..11 and z 7..11), air above y 5..7, one-brick parapets at y 5 along the four arms, arches under the arms (y 2 and 0..1 at the ends) and columns down under the first/last 3 blocks of each arm (x 7..11 × z 0..2 and 16..18; x 0..2 and 16..18 × z 7..11).

**Bridge end** — 5×10×8, offset (−1,−3,0). No children. On creation: own seed = signed 32-bit `n()` (draw from the assembly random source). On placement: new "simple" generator (legacy congruential) initialised with this seed — the step random source is NOT used. Order: for x 0..4, for y 3..4: `k = n(8)`, bricks from (x,y,0) to (x,y,k); then `n(8)` → row (0,5,0..k); `n(8)` → (4,5,0..k); for x 0..4: `n(5)` → (x,2,0..k); for x 0..4, y 0..1: `n(3)` → (x,y,0..k). Effect: a deck that crumbles.

**Straight bridge** — 5×10×19, offset (−1,−3,0). Child: forward (1,3) bridge list. Placement without randomness: deck y 3..4 over the whole length, air y 5..7 (x 1..3), parapets y 5 at x 0 and 4, arches (y 2 over z 0..5 and 13..18; y 0..1 over z 0..3 and 15..18), columns down under z 0..2 and 16..18, decorative fences on the flanks at z 1, 4, 14, 17.

**Crossroads room** — 7×9×7, offset (−2,0,0). Children (bridge list): forward (2,0), left (0,2), right (0,2). Placement: brick floor y 0..1, air y 2..7, corner pillars, fence windows on the four faces at y 5, lintels y 6; columns down under the whole 7×7. No randomness. No roof (the y 8 volume is not placed).

**Stairs room** — 7×11×7, offset (−2,0,0). Child: right (6,2), bridge list. Placement without randomness: walls, inner staircase in solid bricks climbing from (5,2,5) to (1,6,5), slab y 7, high side exit (air at (6,8,2..4)), fence windows; columns down under 7×7.

**Spawner throne** — 7×8×9, offset (−2,0,0). No children. Placement: brick steps, fence guard rail (one fence sticks out at y = 8, outside the nominal box — it is placed anyway if in the clipping). **Spawner**: if not yet placed and local (3,5,5) in the clipping → `minecraft:spawner` placed directly, entity `minecraft:blaze`, "placed" flag. No draw. Columns down under x 0..6, z 0..6 (not z 7..8).

**Castle entrance** — 13×14×13, offset (−5,−3,0). Child: forward (5,3) **castle** list. Placement without randomness: crossed deck y 3..4, hollow enclosure (walls y 5..12 with openings), battlements at y 13 alternating bricks/fences, fence windows y 10..11, inner balconies y 7..8; arches and columns like the crossing; in the centre: platform (5..7,5,5..7), air well (6,1..4,6), brick at (6,0,6) and source **lava** at (6,5,6) with an immediate fluid update scheduled.

#### 8.7.5 Castle pieces (castle list for all their children unless stated)

**Small corridor** — 5×7×5, offset (−1,0,0). Child: forward (1,0). Placement: floor y 0..1, air y 2..5, walls x 0 and 4 with two fence arrow slits (z 1 and 3, y 3..4), ceiling y 6, columns down. No randomness.

**Small corridor crossing** — 5×7×5, (−1,0,0). Children: forward (1,0), left (0,1), right (0,1). Placement: floor, air, four corner pillars, ceiling, columns. No randomness.

**Right turn** — 5×7×5, (−1,0,0). On creation: `chest = (n(3) == 0)`. Child: right (0,1). Placement: west and partial north walls with arrow slits; if a chest is planned and (1,2,3) is in the clipping → reoriented chest (seed `L()`), flag reset to false.
**Left turn** — same mirrored: child left (0,1), chest at (3,2,3).
Order note: the chest's `n(3)` is drawn at creation, therefore during the weighted-choice attempt (consumed only if the box is valid).

**Stair corridor** — 5×14×10, offset (−1,−7,0). Child: forward (1,0). Placement without randomness, for each step s = 0..9 (z = s): floor = max(1, 7−s), roof = min(max(floor+5, 14−s), 13); bricks from y 0 to floor, air between, steps (south face) at x 1..3 at floor+1 for s ≤ 6, roof, walls, N-S fences at even steps (y floor+2..floor+3), columns down.

**T balcony** — 9×7×9, offset (−3,0,0). Children: `zo = 5` if the piece is oriented west or north, otherwise 1; then left (0, zo) with list = castle if `n(8) > 0` otherwise bridge; then right (0, zo) with a new `n(8)` (drawn after the whole processing of the left). Placement without randomness: floor, room, partial ceiling (z 0..5), balcony open on the z 6..8 side with fence guard rail, columns down under z 0..5.

**Nether wart room** — 13×14×13, offset (−5,−3,0). Children: forward (5,3) then forward (5,11) (two on-axis exits at two heights). Placement without randomness: same enclosure as the entrance; grand central staircase climbing towards z (steps facing north at x 5..7); beds of `minecraft:soul_sand` (3..4 and 8..9, y 4, z 4..8) covered with `minecraft:nether_wart` (age 0) at y 5; borders and border steps; arches and columns.

#### 8.7.6 Spawners and fauna (reminder)

Only spawner: the throne (blaze). The spawns specific to the fortress (the fortress's enemy list: blaze 10 [2..3], zombified piglin 5 [4], wither skeleton 8 [5], skeleton 2 [5], magma cube 3 [4]) belong to the spawning chapter, not to generation.

#### 8.7.7 Measurement still to do

Done (X23, `structure_fortress.rs`): pieces placed from the 26.3 code, checked against an official server (seed 1234567890123456789): identical starts, 0 stable differences in the boxes. Assembly details: a bridge end out of range (> 112 blocks) is created, draws its own seed, but is not added; after a creation failure, the following types of the list are tried with the same draw; a limited type that reaches its maximum leaves the list; the final box is lifted by `48 + next_int(70 − 48 + 1 − height)`. The chest of the turns and the throne's spawner are only placed once (piece flag); the entrance's lava schedules an immediate fluid tick, like any placed cell that carries a fluid.

#### 8.7.8 Monument, mansion, End city, Nether fossil (X23)

Reproduced from the 26.3 code (`structure_monument.rs`, `structure_mansion.rs`, `structure_endcity.rs`, `nether_fossil` in `structure_templ.rs`) and checked against the official server: identical starts (all the piece boxes), stable differences in the boxes: monument 78 (kelp and seagrass decorated on top), mansion 33 (of which 28 leaves of neighbouring trees), End city 0, fossils 0. Notable points:
- Monument: the graph links the neighbours in z with north and south swapped; the random closures (2 per room at most, 5 attempts) are cancelled if the source is no longer reachable; the rooms claimed in the template order (XY, YZ, Z, X, Y, simple top, simple). Biomes: the whole box of quarts within 29 blocks around the point (x+9, sea level, z+9) must be in the surrounding tag.
- End city: each subtree receives a tag drawn after its generation; it is refused if it hits a piece of another tag in the list IN PROGRESS only (that of the parent subtree), not in the whole city.
- Mansion: stone foundations placed after all the pieces of the chunk, under each column whose cell at the bottom of the structure is full and in a room.
- Nether fossil: the chunk's clipping is widened to the whole fossil; it is therefore placed at once (neighbours in the write area included); dried ghast drawn from a position random source derived from the world seed.
- Templates without a known shape (everything except the jigsaw pieces): shape updates at the faces of the placed volume then of each block from its six neighbours (`shape_update.rs`). The sides of the walls test the exact areas of the collision bottom face of the block above.

### 8.8 Stronghold

#### 8.8.1 Specific materials and primitives

- **Random masonry** ("stronghold stone"): box where each position receives, if it is on the EDGE of the box: `f()` < 0.2 → `minecraft:cracked_stone_bricks`; < 0.5 → `minecraft:mossy_stone_bricks`; < 0.55 → `minecraft:infested_stone_bricks`; otherwise `minecraft:stone_bricks`; inside: `minecraft:cave_air` (no draw). Traversal order y, x, z.
  - "Skip air" variant (used for the shell of each piece): the position is skipped — **without a draw** — if the current block is air OR outside the clipping box (reading outside the clipping = air). The number of draws therefore depends on the terrain and the chunk (caves already carved).
  - Variant without skip (interior details): a draw for EACH edge position, including outside the clipping → fixed number of draws.
- **Door** (local foot (x,y,z), 3 wide × 3 high in the x/y plane) according to type:
  - opening: cave air (x..x+2, y..y+2, z);
  - wooden door: frame `minecraft:stone_bricks` (posts x and x+2 3 high, lintel y+2), `minecraft:oak_door` bottom at (x+1,y), top at (x+1,y+1);
  - grate: air at (x+1,y) and (x+1,y+1), `minecraft:iron_bars` on the other cells of the frame (east/west connections depending on the cell);
  - iron door: frame like wood, `minecraft:iron_door` bottom/top, `minecraft:stone_button` at (x+2, y+1, z+1) facing north and at (x+2, y+1, z−1) facing south.
- **Drawing an entrance door**: `n(5)` → 0 or 1: opening; 2: wood; 3: grate; 4: iron.
- No replacement filter, no location guard. Chests = reoriented chest (seed `L()`, drawn only if the chest is placed: position in the clipping and not already a chest).
- Valid box = y_min > 10 AND no collision.

#### 8.8.2 Weight table

| piece | weight | max | extra condition |
|---|---|---|---|
| straight corridor | 40 | 0 | — |
| prison | 5 | 5 | — |
| left turn | 20 | 0 | — |
| right turn | 20 | 0 | — |
| crossroads room | 10 | 6 | — |
| straight stairs | 5 | 5 | — |
| spiral staircase | 5 | 5 | — |
| five-way crossing | 5 | 4 | — |
| chest corridor | 5 | 4 | — |
| library | 10 | 2 | depth of the new piece > 4 |
| portal room | 20 | 1 | depth of the new piece > 5 |

Unlike the fortress, NO piece can be chosen twice in a row (not even the straight corridor).

#### 8.8.3 Assembly

1. Attempt t = 0, 1, 2, …:
   1. empty the builder; re-initialise the random source with large_seed(G + t, cx, cz) (for t = 0 this is the normal initialisation);
   2. reset the counters to zero, full list, "forced piece" = none, "last chosen" = none;
   3. start = **source spiral staircase**: direction [north, east, south, west][n(4)], box 5×11×5 placed at (cx·16+2, 64, cz·16+2)–(+4, 74, +4), depth 0, entrance door = opening (no draw); added; expansion: places the forced piece = five-way crossing, then forward exit (1,1);
   4. queue: while not empty, `i = n(size)`, remove element i, expand its children;
   5. "move below sea level" (offset 10);
   6. start again (t+1) if the builder is empty OR if no portal room was recorded (recording made when the portal room "expands its children", i.e. when it leaves the queue).
   The reference point of the biome test is the corner (cx·16, ·, cz·16) of the chunk; the "locator" (/locate, eye of ender) is the portal room.
2. **Creating a child** (foot P, direction dir, depth p of the parent):
   - p > 50 → nothing; |P.x − x_min(start)| > 112 or |P.z − z_min(start)| > 112 → nothing (no draw);
   - otherwise depth q = p + 1 and:
     - if no entry with "max > 0" has margin → nothing (not even a stop-gap);
     - T = sum of the weights of all the remaining entries;
     - if a forced piece exists: try to create it; the forced piece is cleared whatever happens; success → returned as is (**without** incrementing its counter or changing the "last chosen");
     - up to 5 attempts: `w = n(T)`, traversal of the list as for the fortress (w stays negative after the chosen entry ⇒ if creation fails we try the next entry); the attempt stops if the entry fails its condition (exhausted, insufficient depth) or is the last chosen; success → counter +1, last chosen, removal if exhausted;
     - after 5 attempts: **stop-gap** (8.8.4) if its box exists and has y_min > 1, otherwise nothing;
   - the piece obtained is added to the builder and to the queue.
3. Exits (identical to the fortress, but on the piece's depth): forward (xo, yo); left (yo, zo); right (yo, zo) — same formulas as in 8.7.3.

#### 8.8.4 Pieces

"At creation" draws = made after validation of the box, in the order given. All the pieces begin their placement with the random-masonry shell with air skipping over their whole box (except the portal room: no skipping), then the entrance door at (door_x, door_y, 0).

| piece | size w×h×d | offset | draws at creation | exits | entrance door |
|---|---|---|---|---|---|
| spiral staircase | 5×11×5 | (−1,−7,0) | door n(5) | forward (1,1) | (1,7,0) |
| straight corridor | 5×5×7 | (−1,−1,0) | door; left = n(2)==0; right = n(2)==0 | forward (1,1); if left: left (1,2); if right: right (1,2) | (1,1,0) |
| prison | 9×5×11 | (−1,−1,0) | door | forward (1,1) | (1,1,0) |
| left turn | 5×5×5 | (−1,−1,0) | door | oriented north or east: left (1,1); otherwise right (1,1) | (1,1,0) |
| right turn | 5×5×5 | (−1,−1,0) | door | oriented north or east: right (1,1); otherwise left (1,1) | (1,1,0) |
| crossroads room | 11×7×11 | (−4,−1,0) | door; type = n(5) | forward (4,1), left (1,4), right (1,4) | (4,1,0) |
| straight stairs | 5×11×8 | (−1,−7,0) | door | forward (1,1) | (1,7,0) |
| five-way crossing | 10×9×11 | (−4,−3,0) | door; bottom-left = b(); top-left = b(); bottom-right = b(); top-right = n(3) > 0 | A = 3, B = 5 (piece oriented west or north: A = 5, B = 3); forward (5,1); if bottom-left: left (A, 1); if top-left: left (B, 7); if bottom-right: right (A, 1); if top-right: right (B, 7) | (4,3,0) |
| chest corridor | 5×5×7 | (−1,−1,0) | door | forward (1,1) | (1,1,0) |
| library | 14×11×15, otherwise 14×6×15 | (−4,−1,0) | (tall if the first box is valid, otherwise low; failure if none) door | none (dead end) | (4,1,0) |
| portal room | 11×8×16 | (−4,−1,0) | none | none; registers itself as the portal room | fixed grate at (4,1,0) |
| stop-gap | 5×5×(2..4) | (−1,−1,0) | none | none | — |

Reminder: in "left (yo, zo)", the first number is the VERTICAL offset and the second the offset along the wall (see 8.7.3). For the five-way crossing, the A/B inversion for west/north swaps the heights, which makes the exits coincide with the openings cut (the piece being mirrored in these orientations).

**Stop-gap** (box search, no draw): box 5×5×4 offset (−1,−1,0); if there is NO collision → failure; otherwise, if the colliding piece has the same y_min as this box: for depth d = 2 then 1, if the box of depth d does not intersect the colliding piece → box kept with depth d+1; otherwise failure. Placement: tube of `minecraft:stone_bricks` (floor, ceiling, walls x 0 and 4) with cave air x 1..3, y 1..3, over the whole length. No draw.

Placements (order of the draws after the shell and the entrance door):

- **Spiral staircase**: low exit opening at (1,1,4); spiral of `minecraft:stone_bricks` and `minecraft:smooth_stone_slab` (13 fixed placements). No draw.
- **Straight corridor**: exit opening at (1,1,6); 4 "maybe blocks" p = 0.1 of wall torch in the order (1,2,1) facing east, (3,2,1) facing west, (1,2,5) facing east, (3,2,5) facing west; then side openings (0,1..3,2..4) if left, (4,1..3,2..4) if right.
- **Prison**: back opening (1..3,1..3,10); four cell walls in masonry without skipping (x = 4, y 1..3, z = 1, 3, 7, 9: 3 draws each); `minecraft:iron_bars` bars; two iron doors (west face) at (4,1..2,2) and (4,1..2,8). No spawner.
- **Turns**: side opening (x 0 or 4, y 1..3, z 1..3) on the side of the exit.
- **Crossroads room**: three openings (back (4..6,1..3,10), sides x 0 and 10 at z 4..6); according to type: 0 = central cobblestone pillar + 4 wall torches; 1 = fountain (5×5 cobblestone rim at y 1, central pillar, source water at (5,4,5)); 2 = storeroom (ring of `minecraft:cobblestone` at y 3, pillars, wall torch at (5,3,5), mezzanine of `minecraft:oak_planks` at y 3, ladder (9,1..3,3) facing west, **chest** at (3,4,8) table `minecraft:chests/stronghold_crossing`); 3 and 4 = empty room.
- **Straight stairs**: exit opening (1,1,7); `minecraft:cobblestone_stairs` steps (facing south) going down from (1..3, 6, 1) to (1..3, 1, 6) with `minecraft:stone_bricks` underneath. No draw.
- **Five-way crossing**: openings according to the four flags + back (5..7,1..3,10); details in masonry without skipping (fixed number of draws), smooth stone slabs (single and double), wall torch facing south at (6,5,6).
- **Chest corridor**: exit opening (1,1,6); cobblestone bench and cobblestone slabs; **chest** at (3,2,3) only once (flag), table `minecraft:chests/stronghold_corridor`.
- **Library**: "maybe box" p = 0.07 of `minecraft:cobweb` over (2,1,1)–(11,4,13) (520 draws, neither skip nor interior); `minecraft:bookshelf` shelves at x 1 and 12 (`minecraft:oak_planks` posts + wall torches every 4) and as islands (x 3..4, 6..7, 9..10, odd z 3..11); if tall: plank gallery at y 5, `minecraft:oak_fence` railing, ladder (10,1..7,13), fence chandelier and 6 torches in the centre; **chest** at (3,3,5) then, if tall, air at (12,9,1) and **chest** at (12,8,1); table `minecraft:chests/stronghold_library`.
- **Portal room** (shell WITHOUT air skipping → fixed number of draws: 652 for the shell); grate door; order of the draws of masonry without skipping: gallery y 6 (4 boxes: 14+14+14+7), bases of the side pools (8+8), base of the central pool (25), portal staircase (9+6+3); `minecraft:lava` at (1,1,1..3), (9,1,1..3) and (4..6,1,9..11); bar windows; `minecraft:stone_brick_stairs` steps facing north at (4..6, 1..3, 4..6); then **12 eye draws**: eye present if `f() > 0.9` (≈ 10 % per frame), in the order: frames facing north (4,3,8), (5,3,8), (6,3,8); facing south (4,3,12), (5,3,12), (6,3,12); facing east (3,3,9), (3,3,10), (3,3,11); facing west (7,3,9), (7,3,10), (7,3,11) (`minecraft:end_portal_frame`); if all 12 have an eye → `minecraft:end_portal` over (4..6, 3, 9..11); finally a `minecraft:silverfish` **spawner** at (5,3,6) if not yet placed and in the clipping (direct placement, flag).
  As the draws preceding the eyes are a fixed number, the eyes only depend on the step random source of the chunk being processed — but each chunk covered by the room redoes the 12 draws with ITS seed: frames located in different chunks are drawn independently.

#### 8.8.5 Measurement still to do

Dedicated file `docs/specs/structures/stronghold-blocks.md` to create: exact geometry of the library (gallery, chandelier, fence states), of the five-way crossing (inner boxes and slabs), of the type-2 crossroads room (storeroom) and of the portal room (positions of the bars, pools). The other pieces are sufficiently described above.


### 8.9 Ocean monument

#### 8.9.1 Materials

| Role | Block |
|---|---|
| grey (floor, fillings) | `minecraft:prismarine` |
| light (walls, decoration) | `minecraft:prismarine_bricks` |
| dark | `minecraft:dark_prismarine` |
| lamp | `minecraft:sea_lantern` |
| treasure | `minecraft:gold_block` (8 blocks, core room) |
| sponge | `minecraft:wet_sponge` |
| filling | `minecraft:water` (default state) |

**Water box** (primitive specific to the monument): for each cell (y then x then z increasing) of the local box, read the block present; if it is ice, packed ice, blue ice or water (comparison by block, any water level), do nothing; otherwise if the world Y ≥ sea level → air, otherwise → source water. Clipped by the chunk's box like "place local block".

**Fill-if-water**: place the target block only if the present state is exactly the default source water.

**Default floor** of an 8×8 cell (local offset xo, zo): if the cell has an opening towards DOWN: grey floor as a ring around a central 2×2 hole (x 3‑4, z 3‑4 left untouched), with a light edging on the 4 sides of the hole (x 3‑4 at z 2 and z 5; z 3‑4 at x 2 and x 5); otherwise full 8×8 grey slab. Only applies to cells of floor ≥ 1.

#### 8.9.2 Start and root piece

- Root piece ("building"): oriented box of 58 × 23 × 58 from (16cx−29, **39**, 16cz−29), orientation drawn (1st draw of the structure random source). **The Y is fixed at 39**: the starting position (ocean floor at the chunk centre) only serves as the structure's reference, not for placement.
- Only the root piece is serialised: when a save is re-read, the sub-pieces are **regenerated** by replaying the whole algorithm with a fresh random source initialised by large_seed(G, cx, cz), taking the saved box's min X / min Z and the saved orientation (the orientation draw is still consumed). The result is therefore identical to the initial generation.

#### 8.9.3 Room grid

Cells indexed `i = y·25 + z·5 + x` (x, z ∈ 0..4, y ∈ 0..2). Cell = 8 × 4 × 8 local blocks. Existing cells (46):
- floor 0: x 0..4, z 0..3 (20);
- floor 1: x 0..4, z 0..3 (20);
- floor 2: x 1..3, z 0..1 (6).

Special cells outside the grid (index ≥ 75): left wing (1001), right wing (1002), roof/penthouse (1003).

**Links**: each pair of neighbouring existing cells (6 directions) is linked in both ways. Careful, the grid's z axis is **inverted** with respect to the directions: the "NORTH" link of a cell leads to z+1, "SOUTH" to z−1; EAST = x+1, WEST = x−1, UP = y+1. Then:
- cell (2,2,0) —UP→ roof (1003);
- cell (0,1,0) —SOUTH→ left wing (1001); cell (4,1,0) —SOUTH→ right wing (1002);
- roof and wings marked "taken".
- **Source** = cell (2,0,0) (entrance).

**Exact algorithm, in the order of the draws** (structure random source, after the orientation draw):
1. **Core**: `x = next_int(4)`, core cell = (x, 0, 2). Mark as "taken" the cell and its 2×2×2 block: EAST, NORTH, EAST→NORTH, UP, EAST→UP, NORTH→UP, EAST→NORTH→UP.
2. For each grid cell (increasing index order): openings[d] = (link[d] exists). Likewise for the roof (its DOWN opening). The wings have **no** opening of their own. List L = grid cells in index order.
3. **Shuffle** of L: descending Fisher‑Yates — for i from |L| to 2: `j = next_int(i)`, swap L[i−1] and L[j].
4. **Closures**: for each cell c of L (shuffled order): counter closed = 0, attempts = 0; while closed < 2 and attempts < 5: attempts++; `f = next_int(6)` (drawn at each attempt); if c has an opening f: close it on both sides (c[f] and neighbour[opposite f]); then test accessibility: depth-first traversal from c following the openings, true as soon as the source is reached; if c reaches the source **and** the neighbour too (second test only if the first succeeds) → closed++, otherwise reopen both sides. The visit marks use a global counter that only grows (no effect on the randomness).
   - Consequence: the links towards the wings and the roof are never closed (the special side has no opening of its own or loses the only one it has → accessibility test fails).
5. Add to L: roof, left wing, right wing (special, ignored afterwards).
6. Mark the source "taken".

#### 8.9.4 Assigning the rooms

Child pieces, in this order: **entrance** (on the source), **core** (on the core cell), then for each cell of L not taken and not special, the **first** applicable template of the list:

| Order | Type | Condition (on cell c) | Cells taken | Size (cells x·y·z) |
|---|---|---|---|---|
| 1 | double X‑Y | opening EAST and EAST neighbour not taken; opening UP and UP neighbour not taken; the EAST neighbour has opening UP and its UP neighbour not taken | c, E, U, E→U | 2·2·1 |
| 2 | double Y‑Z | same with NORTH instead of EAST | c, N, U, N→U | 1·2·2 |
| 3 | double Z | opening NORTH and NORTH neighbour not taken | c, N | 1·1·2 |
| 4 | double X | opening EAST and EAST neighbour not taken | c, E | 2·1·1 |
| 5 | double Y | opening UP and UP neighbour not taken | c, U | 1·2·1 |
| 6 | "tall" simple (sponges) | no opening W, E, N, S nor UP (only DOWN possible) | c | 1·1·1 |
| 7 | simple | always | c | 1·1·1 |

Only the **simple** room (7) consumes the randomness at construction: `variant = next_int(3)`.

Note: the "not taken" check of template 1 does not test that the E→U cell itself is free beyond the link; this is the exact replica.

**Box of a room**: oriented box of size (w·8, h·4, d·8) at the origin, then translated according to the orientation, with (rx, ry, rz) = coordinates of the base cell:
- NORTH: (rx·8, ry·4, −(rz+d)·8 + 1); SOUTH: (rx·8, ry·4, rz·8);
- WEST: (−(rz+d)·8 + 1, ry·4, rx·8); EAST and default: (rz·8, ry·4, rx·8).
Finally all these boxes are translated by the world position of the building's local point (9, 0, 22). All the rooms have the building's orientation.

Then:
- `r = next_int()` (32-bit integer); left wing: box between the local points (1,1,1) and (23,8,21), variant `r & 1`; right wing: (34,1,1)–(56,8,21), variant `(r+1) & 1` → **the two wings always have opposite variants**.
- Penthouse: box (22,13,22)–(35,17,35).

Summary of the order of the structure random draws: orientation `next_int(4)` → core `next_int(4)` → shuffle (45 draws `next_int(i)`, i = 46..2) → closures (`next_int(6)` × 2 to 5 per cell, 46 cells) → `next_int(3)` per simple room, in the order of L → `next_int()` of the wings.

#### 8.9.5 Placing the building (per chunk)

Order:
1. Local water box (0,0,0)–(58, h, 58) with h = max(sea level, 64) − 39 (25 for sea 63). Also covers row 58 outside the piece's box (only the chunk's box clips).
2. Left wing (x offset 0) then right wing (x offset 33, "mirror" version): grey slab, water, light steps, decoration.
3. Entrance arches, entrance wall (z 21), central roof (z 21‑36, with 4 lamps at the top), low wall (edges x 0‑6, x 51‑57, z 51‑57), middle wall, high wall. Each block is only placed if the chunk intersects its horizontal footprint.
4. **Pillars**: 7 × 7 grid of 4×4 pillars with a step of 9 (local origin (9i, 0, 9j)); only the perimeter (i ∈ {0,6}: all j; otherwise j ∈ {0,6}), and pillar (3,0) is skipped (entrance passage). Each cell: light at y 0, then column down (light) from y −1.
5. Water skirt: for k 0..4, four vertical water boxes that widen by 1 and rise by 2 per k (edges x −1−k and 58+k, z −1−k and 58+k, y from 2k to 23).
6. Children (list order: entrance, core, rooms in the order of L, wing L, wing R, penthouse), each only if its box intersects the chunk, **with the same step random source** shared sequentially.

No random draw in steps 1‑5.

#### 8.9.6 Pieces and what they place

Common to the grid rooms: if floor ≥ 1, default floor of each base cell (according to its DOWN opening); if the upper cell has no UP link, grey ceiling as "fill‑if‑water" (y 4 or 8 depending on height). The remaining openings (after closures) become 2 × 2 **water doors** (water box) in the middle of the corresponding wall, at y 1‑2 (and 5‑6 for the upper level of the Y doubles).

| Piece | Local size | Content (summary) | Randomness at placement |
|---|---|---|---|
| Entrance | 8×4×8 | light U-shaped walls open to the south, doors N/W/E according to openings | none |
| Core | 16×8×16 | grey ceiling (fill‑if‑water) y 8, light belt y 7, patterned walls (grey at y 2 and 6), 4×4×4 dark core (6‑9, 3‑6) containing **2×2×2 gold blocks** (7‑8, 4‑5), 8 lamps at the corners of the core, light pillars and beams y 7 | none |
| Simple (3 variants) | 8×4×8 | v0: 4 corner pillars 3×3 with lamp, door frames; v1, v2: pillar/lamp layouts (measurement still to do); optional central 2×2 pillar | `next_boolean()` **only if variant ≠ 0** (short‑circuit), then condition: no DOWN or UP opening and ≥ 2 openings |
| Tall simple (sponges) | 8×4×8 | light ring y 1 and 3, dark y 2, dark reinforcements in the middle of the walls, south door if open | for x 1..6, then z 1..6: if `next_int(3) ≠ 0` → y0 = 2 if `next_int(4) == 0` otherwise 3; column of wet sponges from y0 to 3 |
| Double X | 16×4×8 | light/grey/light walls, low central partition (5‑10) with 2 lamps | none |
| Double Y | 8×8×8 | two levels, light mezzanine y 4 in a corner pattern, "arcade" walls according to the openings of each level | none |
| Double Z | 8×4×16 | corner pillars, central arch z 6‑9 with 4 lamps | none |
| Double X‑Y | 16×8×8 | patterned walls, two central columns, walkway y 7, 4 lamps y 4 | none |
| Double Y‑Z | 8×8×16 | dark central column (3‑4, 7‑8) with lamps at y 2 and 6; side balconies when the upper level has a W/E opening | none |
| Wing v0 | box 23×8×21 | stepped room, light floor, 2 side walls, lamps, **elder guardian** at local (11, 2, 16) | none |
| Wing v1 | same | platform y 3, 4 central columns with lamps, **elder guardian** at local (11, 5, 13) | none |
| Penthouse | 14×5×14 | light/grey slab at y −1, light crown with lamps, pillars, small water pool (6‑7, 3‑4), **elder guardian** at local (6, 1, 6) | none |

**Creating an elder guardian** (3 per monument: penthouse + one per wing): if the world position of the local point is in the chunk's box: create `minecraft:elder_guardian` (spawn reason "structure"), health at maximum, position (x+0.5; y; z+0.5), yaw and pitch 0, then spawn initialisation (local difficulty at its position, world random source, no group data), added to the world with passengers. Unlike the witch, the piece does not mark the guardian as persistent (see uncertain points). No "already created" flag is saved: creation only depends on the position belonging to the chunk (a single chunk contains it).

Block-by-block measurement remaining: simple rooms v1/v2, wings, exterior walls (the box lists are long but purely deterministic, to be transcribed from the decompiled code in local coordinates).

### 8.10 Woodland mansion

All the pieces are **templates** `minecraft:woodland_mansion/<name>` (§ 10), placed with: template entities ignored, rotation and mirror given, pivot (0,0,0), processor "ignore `minecraft:structure_block`". No other processor, no partial integrity.

Notation. R = rotation of the mansion (drawn at the start). For a rotation ρ and a direction D, "D_ρ(n)" = displacement of n blocks in the direction D rotated by ρ; without subscript, ρ = R. "ρ ∘ CW90" = ρ composed with a clockwise quarter turn (CCW90 counter‑clockwise, HALF half‑turn). Origin O = starting position (§ starts: (16cx+7, y, 16cz+7)).

#### 8.10.1 Grids

11 × 11 grid indexed (x, y) — **grid y = z axis** (no direction inversion: NORTH = y−1, SOUTH = y+1, EAST = x+1, WEST = x−1). Reading outside the grid = 5. Writing outside the grid ignored.

Base values: 0 empty, 1 corridor, 2 room, 3 starting room, 4 (test, unused), 5 blocked. "House" = value ∈ {1, 2, 3, 4}.

Room grids (one per floor, out-of-grid value 5): integer = flags | type | identifier (low 16 bits).
- type: 1×1 = 0x10000, 1×2 = 0x20000, 2×2 = 0x40000 (mask 0xF0000);
- flags: ORIGIN 0x100000, DOOR 0x200000, STAIRS 0x400000, CORRIDOR 0x800000.

Primitives: `fill(x0,y0,x1,y1,v)` (bounds included), `set_if(x,y,a,b)` = set b if the cell equals a.

#### 8.10.2 Ground-floor plan (base grid)

Entrance (ex, ey) = (7, 4). Initialisation, in order:
1. fill(7,4, 8,5, 3); fill(6,4, 6,5, 2); fill(9,2, 10,7, 5); fill(8,2, 8,3, 1); fill(8,6, 8,7, 1); (6,3) = 1; (6,6) = 1; fill(0,0, 11,1, 5); fill(0,9, 11,11, 5).
2. Recursive corridors (§ 8.10.3): (7,2, WEST, 6); (7,7, WEST, 6); (5,3, WEST, 3); (5,6, WEST, 3).
3. Edge cleanup repeated as long as it modifies something.

**Edge cleanup** (one pass, outer y, inner x, in-place modification): for each cell 0, count the direct "house" neighbours (x+1, x−1, y+1, y−1); if ≥ 3 → 2; if = 2 and diagonal house neighbours ≤ 1 → 2.

#### 8.10.3 Recursive corridor (random core of the plan)

`corridor(g, x, y, heading h, depth)`: if depth > 0:
1. (x,y) = 1; set_if((x,y)+h, 0, 1).
2. Up to 8 attempts: `d = dir2D(next_int(4))` with dir2D = [SOUTH, WEST, NORTH, EAST]; accepted if d ≠ opposite(h) **and** (d ≠ EAST or `next_boolean()` false) — the boolean is drawn **only** if d = EAST. If accepted: n = (x,y)+h; if g(n+d) == 0 and g(n+2d) == 0 → `corridor(g, n+d, d, depth−1)` then leave the attempts loop. (An accepted but blocked d consumes the attempt and we continue.)
3. Only afterwards (after the recursion returns), with cw/ccw = clockwise/counter‑clockwise rotations of h: set_if(·, 0, 2) at the cells (x,y)+cw, +ccw, +h+cw, +h+ccw, +2h, +2cw, +2ccw, in this order.

#### 8.10.4 Rooms of floors 1 and 2 (identification)

`identify(source, rooms)`:
1. List of the cells of value 2 in the source (outer y, inner x); descending Fisher‑Yates **shuffle** (same algorithm as in the monument: `next_int(i)` for i = n..2).
2. id = 10. For each cell (x, y) of the list with rooms(x,y) == 0: bounds x0=x1=x, y0=y1=y, type 1×1, then the first case that applies ("free" = rooms == 0 and source == 2 for each of the cells cited):
   - 2×2 towards (+x, +y); otherwise 2×2 towards (−x, +y); otherwise 2×2 towards (−x, −y) (the (+x, −y) case does not exist);
   - otherwise 1×2 towards +x; +y; −x; −y (first free);
   - otherwise 1×1.
3. Door: `px = next_boolean() ? x0 : x1` then `py = next_boolean() ? y0 : y1` (two draws always, even in 1×1). The cell must touch a corridor (one of the 4 neighbours of value 1 in the source). Otherwise try (px', py') (both flipped), otherwise flip only py, otherwise flip both, otherwise no door: DOOR flag absent and cell = (x0, y0).
4. Write: door cell = ORIGIN | DOOR (if door) | type | id; other cells = type | id. id++.

Order: identify(base, floor 0); identify(base, floor 1) (same source, new shuffle → different split); then cells (8,4) and (8,5) of floors 0 and 1 = CORRIDOR (placement of 0x800000, overwrites the value).

#### 8.10.5 Third level (floor 2)

Blank "top" grid (0, outside the grid 5).
1. Candidates = cells of floor 1 of type 1×2 with DOOR (outer y, inner x). None → top grid entirely at 5 (end).
2. `p = candidates[next_int(n)]`; add STAIRS to floor 1 at p. Direction of the room = first of [NORTH, EAST, SOUTH, WEST] whose neighbour carries the same id on floor 1; q = p + direction.
3. Top grid: non‑house cell in the base → 5; p → 3; q → 3 and floor 2 (q) = CORRIDOR; the rest stays 0.
4. Possible corridors from q = directions [N, E, S, W] whose neighbour is 0 in the top grid. None → top grid entirely at 5 and STAIRS is removed from p (the CORRIDOR mark of q on floor 2 remains, without effect). Otherwise `d = possible[next_int(n)]`, `corridor(top, q+d, d, 4)`, then edge cleanup repeated.
5. identify(top, floor 2).

**Complete order of the draws** (structure random source, after the rotation draw and without a draw for the height): 4 corridors → identification floor 0 → floor 1 → stairs choice, corridor choice, recursive corridor → identification floor 2 → placement of the rooms (§ 8.10.8: per floor 0, 1, 2; cells outer y, inner x).

#### 8.10.6 Exterior walls

Constants: sX = 8, sY = 5. Position of a cell: `P(base, x, y) = base + SOUTH(8 + (y−sY)·8) + EAST((x−sX)·8)`.

Placement state (position, rotation ρ, wall template). Operations:
- **wall**: current template at position + EAST_ρ(7), rotation ρ; then position += SOUTH_ρ(8).
- **corner**: position += SOUTH_ρ(−1); `wall_corner` at position, rotation ρ; position += SOUTH_ρ(−7) + WEST_ρ(−6); ρ = ρ ∘ CW90.
- **inner corner**: position += SOUTH_ρ(6) + EAST_ρ(8); ρ = ρ ∘ CCW90 (no piece).

**Traversal** (grid g, initial direction SOUTH, start (a,b), end (c,d)) — "do … while" loop not (cell == end and dir == SOUTH):
- if the neighbour in dir is not a house: corner; dir = clockwise(dir); if not (cell == end and dir == SOUTH): wall.
- otherwise if the neighbour in dir **and** the cell (neighbour + counter‑clockwise(dir)) are houses: inner corner; advance one cell; dir = counter‑clockwise(dir).
- otherwise: advance; if not (cell == end and dir == SOUTH): wall.

Sequence:
1. `entrance` at O + WEST(9), rotation R. State A = (O + SOUTH(16), R, `wall_flat`).
2. Traversal of the base with A, from (8,5) to (8,4).
3. State B = (O + SOUTH(16) + 8 up, R, `wall_window`); traversal of the base with B, from (8,5) to (8,4).
4. State C = (O + 19 up, R, `wall_window`): look for the first house cell of the top grid by increasing y then **decreasing x**; if found at (x,y): position = P(O+19, x, y), a **wall**, then traversal of the top grid from (x,y) to (x,y).

#### 8.10.7 Roofs

`roof(origin T, grid g, upper grid h or nothing)`, with Q = P(T, x, y):
1. For each house cell of g (outer y, inner x) **without** a house above in h: `roof` at Q + 3 up (R); then `roof_front`: if x+1 not a house → Q+EAST(6), R; if x−1 not a house → Q+SOUTH(7), R∘HALF; if y−1 not a house → Q+WEST(1), R∘CCW90; if y+1 not a house → Q+EAST(6)+SOUTH(6), R∘CW90.
2. If h exists, for each house cell of g **with** a house above: `small_wall`: x+1 free → Q+EAST(7), R; x−1 → Q+WEST(1)+SOUTH(6), R∘HALF; y−1 → Q+NORTH(1), R∘CCW90; y+1 → Q+EAST(6)+SOUTH(7), R∘CW90. Then `small_wall_corner`: if x+1 free: (y−1 free → Q+EAST(7)+NORTH(2), R); (y+1 free → Q+EAST(8)+SOUTH(7), R∘CW90). If x−1 free: (y−1 free → Q+WEST(2)+NORTH(1), R∘CCW90); (y+1 free → Q+WEST(1)+SOUTH(8), R∘HALF).
3. For each house cell of g without a house above: if x+1 free, with Q2 = Q+EAST(6): (y+1 free → `roof_corner` Q2+SOUTH(6), R; otherwise if (x+1,y+1) house → `roof_inner_corner` Q2+SOUTH(5), R); (y−1 free → `roof_corner` Q2, R∘CCW90; otherwise if (x+1,y−1) house → `roof_inner_corner` Q+EAST(9)+NORTH(2), R∘CW90). If x−1 free, Q2 = Q: (y+1 free → `roof_corner` Q2+SOUTH(6), R∘CW90; otherwise if (x−1,y+1) house → `roof_inner_corner` Q2+SOUTH(8)+WEST(3), R∘CCW90); (y−1 free → `roof_corner` Q2, R∘HALF; otherwise if (x−1,y−1) house → `roof_inner_corner` Q2+SOUTH(1), R∘HALF).

Calls: roof(O+16, base, top) then roof(O+27, top, nothing).

#### 8.10.8 Interior, per floor n = 0, 1, 2

Floor origin F = O + (8n + (n == 2 ? 3 : 0)) up. Grid g = base (n < 2) or top (n = 2). Room collection: floor 0 = "A", floors 1 and 2 = "B".

**Pass 1 — corridors** (cells g == 1, outer y, inner x), Q = P(F, x, y): `corridor_floor` at Q (R). A "connected" neighbour = neighbour of value 1 in g or carrying CORRIDOR in the floor's room grid.
- north connected → `carpet_north` at Q+EAST(1) + 1 up;
- east connected → `carpet_east` at Q+SOUTH(1)+EAST(5) + 1 up;
- south connected → `carpet_south_1` (n=0) / `carpet_south_2` at Q+SOUTH(5)+WEST(1) (no +1 up);
- west connected → `carpet_west_1` / `carpet_west_2` at Q+WEST(1)+NORTH(1) (no +1 up).

**Pass 2 — rooms** (outer y, inner x) for each cell g == 2, or (n = 2 and g == 3): v = room grid; type, id; tfs = (n = 2 and g == 3 and v has CORRIDOR) (true only for q).
1. Door: if v has DOOR, list of the directions [N, E, S, W] whose neighbour is 1 in g; if non‑empty → `door = list[next_int(size)]` (draw even if size is 1). Otherwise if v has ORIGIN → door = UP. Otherwise none.
2. Room base S = F + SOUTH(8 + (y−sY)·8) + EAST(−1 + (x−sX)·8). Wall templates `indoors_wall_1`/`_2`, doors `indoors_door_1`/`_2` (1 = floor 0):
   - west: if (x−1) house and different id → (door == W ? door : wall) at S, R;
   - east: if g(x+1) == 1 and not tfs → (door == E ? …) at S+EAST(8), R;
   - south: if (y+1) house and different id → (door == S ? …) at S+SOUTH(7)+EAST(7), R∘CW90;
   - north: if g(y−1) == 1 and not tfs → (door == N ? …) at S+NORTH(1)+EAST(7), R∘CW90.
   ("different id" compares the low 16 bits of the floor's room grid; two cells at 0 are "same id".)
3. Room:
   - 1×1 (always, even without a door): name = 1×1 draw; room rotation ρp: door E → none, N → CCW90, W → HALF, S → CW90; other (UP or nothing) → none and **new draw** secret name. Local offset d = (1,0,0) + (0,0,0) / CCW90: (0,0,6) / CW90: (6,0,0) / HALF: (6,0,6); then d rotated by R; placement at S + d, rotation ρp ∘ R.
   - 1×2 if door defined: dir = direction of the room (§ 8.10.5); table below.
   - 2×2 if door ∉ {nothing, UP}: dir = clockwise(door), or its opposite if the neighbouring cell in that direction does not have the same id; table below.
   - 2×2 if door = UP: `2x2_s1` at S+EAST(1), R.

1×2 rooms (door, dir → name, position, rotation, mirror):

| door | dir | template | position | rotation | mirror |
|---|---|---|---|---|---|
| E | S | lateral | S+EAST(1) | R | — |
| E | N | lateral | S+EAST(1)+SOUTH(6) | R | left‑right |
| W | N | lateral | S+EAST(7)+SOUTH(6) | R∘HALF | — |
| W | S | lateral | S+EAST(7) | R | front‑back |
| S | E | lateral | S+EAST(1) | R∘CW90 | left‑right |
| S | W | lateral | S+EAST(7) | R∘CW90 | — |
| N | W | lateral | S+EAST(7)+SOUTH(6) | R∘CW90 | front‑back |
| N | E | lateral | S+EAST(1)+SOUTH(6) | R∘CCW90 | — |
| S | N | frontal | S+EAST(1)+NORTH(8) | R | — |
| N | S | frontal | S+EAST(7)+SOUTH(14) | R∘HALF | — |
| W | E | frontal | S+EAST(15) | R∘CW90 | — |
| E | W | frontal | S+WEST(7)+SOUTH(6) | R∘CCW90 | — |
| UP | E | secret | S+EAST(15) | R∘CW90 | — |
| UP | S | secret | S+EAST(1) | R | — |
| other | | (nothing, no draw) | | | |

2×2 rooms (door, dir → EAST offset, SOUTH offset, rotation, mirror): (E,S) −7,0,R; (E,N) −7,6,R,left‑right; (N,E) 1,14,R∘CCW90; (N,W) 7,14,R∘CCW90,left‑right; (S,W) 7,−8,R∘CW90; (S,E) 1,−8,R∘CW90,left‑right; (W,N) 15,6,R∘HALF; (W,S) 15,0,R,front‑back; other → 0,0,R. Name drawn **after** the computation.

Name draws (one `next_int` each):

| Collection | 1×1 | 1×1 secret | 1×2 lateral | 1×2 frontal | 1×2 secret | 2×2 | 2×2 secret |
|---|---|---|---|---|---|---|---|
| A (floor 0) | `1x1_a`+(1..5) | `1x1_as`+(1..4) | `1x2_a`+(1..9) | `1x2_b`+(1..5) | `1x2_s`+(1..2) | `2x2_a`+(1..4) | `2x2_s1` (no draw) |
| B (floors 1, 2) | `1x1_b`+(1..5) | `1x1_as`+(1..4) | stairs: `1x2_c_stairs` **without a draw**, otherwise `1x2_c`+(1..4) | stairs: `1x2_d_stairs` without a draw, otherwise `1x2_d`+(1..5) | `1x2_se1` (draw `next_int(1)` consumed) | `2x2_b`+(1..5) | `2x2_s1` |

Only the cells carrying the door (or the origin) place a room; the other cells of a multi-cell room only place their walls.

**Order of the piece list** (= placement order in a chunk): entrance, floor 0 walls, floor 1 walls, top walls, low roof, high roof, then for n = 0, 1, 2: corridors and carpets, then inner walls and rooms.

#### 8.10.9 Data markers

After placing a template in the chunk, each structure block in DATA mode located in the chunk's box is processed according to its string:
- `Chest`, `ChestWest`, `ChestEast`, `ChestSouth`, `ChestNorth` (prefix "Chest"): chest facing the direction indicated rotated by the piece's rotation (`Chest` alone: default state, **no** automatic reorientation); chest primitive: if in the chunk and not already a chest, place (flags 2) and table `minecraft:chests/woodland_mansion`, seed `next_long()` of the step random source.
- `Mage` → 1 `minecraft:evoker`; `Warrior` → 1 `minecraft:vindicator`; `Group of Allays` → 1 to 3 `minecraft:allay` (number = `next_int(3)+1` from the **world random source**, non‑deterministic). Other strings: ignored.
- For each mob created: persistent, placed at the corner of the marker block (integer x, y, z, no +0.5), yaw/pitch 0, "structure" spawn initialisation with the local difficulty, added with passengers, then the marker's block is set to air (flags 2).
The structure block itself is never placed (ignore processor): under a chest marker, it is the chest; under a mob marker, air.

#### 8.10.10 Final filling

After placing all the pieces in a chunk (§ starts): y0 = min Y of the bounding box of all the pieces; for each column (x, z) of the chunk: if the block at (x, y0, z) is not empty, is in the bounding box and in at least one piece, go down from y0−1 to min_Y+1 placing `minecraft:cobblestone` (flags 2) as long as the block is empty or liquid; stop at the first solid non‑liquid block.

### 8.11 Desert pyramid

#### 8.11.1 Common base of the "scattered" pieces (also for 8.12 and 8.13)

Single oriented piece (horizontal direction drawn at construction: 1 `next_int(4)` of the structure random source, list [NORTH, EAST, SOUTH, WEST]), initial box at Y = 64, given (west, north) corner. "Placed height" field H, initially −1, **saved** with the piece (with width, height, depth).

Two height adjustments, called at the start of each chunk placement; if H ≥ 0, nothing is recomputed (success):
- **average** (temple, hut): integer average (truncated division) of the MOTION_BLOCKING_NO_LEAVES height over only the columns of the box that are **in the current chunk**; no column → failure, nothing is placed in this chunk. Otherwise H = average and the box shifted vertically so that min Y = H + offset. Consequence: the height depends on the **first chunk** that places the piece (generation order); it is then frozen.
- **minimum** (pyramid): min of the same heightmap over **all** the columns of the box (read even outside the chunk), then min Y = H + offset.
- If the height found is negative, H stays < 0 and the computation is redone at the next chunk (marginal case of worlds with a negative bottom; to reproduce as is).

"Chest (x,y,z, table)" = reoriented chest primitive of the spec (placed only if in the chunk and not already a chest; seed `next_long()` drawn **only** if placed). "Dispenser": same logic (in the chunk and not already a dispenser), oriented state passed through "place local block", table + seed `next_long()`.

#### 8.11.2 Pyramid

- Box 21 × 15 × 21 (w × h × d). Saved flags: chest placed [4] (2D index: SOUTH 0, WEST 1, NORTH 2, EAST 3).
- Placing a chunk:
  1. `d = next_int(3)` **drawn at each call**, even if H is already fixed; minimum adjustment with offset −d (0, −1 or −2), applied only the first time.
  2. Deterministic geometry (no draw): solid sandstone base y −4..0; 9 steps (sandstone ring then air interior at each level 1..9); sandstone column down from y −5 under each of the 21 × 21 columns; two 5 × 5 towers at the front corners (x 0‑4 and 16‑20, z 0‑4) with stairs and roof; entrance porch (z 0‑4), central room, orange/blue terracotta patterns and chiseled sandstone on the floor, treasure chamber (8‑12, −14..−1).
  3. Treasure chamber: cut sandstone cube, chiseled crown at y −10, air well (9‑11, −11..−1), stone pressure plate at (10, −11, 10), 3 × 3 ring of **TNT** at y −13 (8 TNT, centre air), 4 niches.
  4. **Chests**: for d in [NORTH, EAST, SOUTH, WEST] (this order), if not placed: chest at (10 + 2·dx, −11, 10 + 2·dz), table `minecraft:chests/desert_pyramid`; flag = result.
  5. **Cave** (local centre C = (16, −4, 13)): 3 sandstone stairs at (13,−1,17), (14,−2,17), (15,−3,17) (default state rotated by a counter‑clockwise quarter turn, then "place local block"); sand plug at z 17 with two sand/sandstone cells swapped according to a boolean drawn from the **world random source** (non‑deterministic); walls of cut sandstone (y −3 and −1) and chiseled (y −2) as a 7 × 7 ring replacing only the non‑air (box "skip air"); terracotta pattern on the floor y −4 (blue in the centre, orange around); collapsed 5 × 5 roof at y 0 (14‑18, 11‑15): each cell sandstone if `next_float() < 0.33` of the **world random source**, otherwise sand.
  6. Potential sand positions (recorded, **not placed** here): box (14‑18, −3..−1, 11‑15) + (13 and 19, −3..−2, 13) + (16, −3..−2, 10 and 16) = 83 world positions.
  7. Collapsed roof: positional random source (world seed, world position of local point (14, 0, 11)) → x = integer in [14, 18], then z in [11, 15]; position memorised.
- After placement (reminder § starts, details): the set of potential positions sorted by (Y, then Z, then X) — the order matters for the shuffle —; suspicious sand also at the collapsed roof's position; archaeology table `minecraft:archaeology/desert_pyramid`, seed = position packed into a 64-bit integer; the positions not retained receive sand (flags 2) if in the chunk. The shuffle centre is the centre of the bounding box of **all** the pieces (rounded by (size)/2 downwards from the min).
- Block-by-block measurement to do: towers, porch, central room (deterministic box lines).

### 8.12 Jungle temple

- Box 12 × 10 × 15. Saved flags: main chest, hidden chest, trap 1, trap 2.
- **Average** adjustment, offset 0.
- Random material "mossy stone": for **each** cell of a selector box (outer y, inner x, z; draw made even outside the chunk), `next_float() < 0.4` → `minecraft:cobblestone`, otherwise `minecraft:mossy_cobblestone`.
- Exact order of the draws (step random source), in code order:
  1. Selector boxes of the structure: base (0‑11, −4..0, 0‑14), walls and floors (2‑9 / 1‑10, y 1‑8), then air (rooms, doors), then facade pillars z 0 and 14 (x 2, 4, 7, 9, y 4‑5), lintel (5‑6, 6, 0), side pillars x 0 and 11 (z 2..12 step 2, y 4‑5) + (y 6, z 5 and 9), roof (corners y 7‑9, 4 blocks at y 9, ridge (5‑6, 9, 7)), cobblestone stairs, inner steps (4, 1, 9), (7, 1, 9), (4‑7, 1‑2, 10), (5‑6, 4, 5); descending staircase (4 steps);
  2. basement: air, pillars (1, −3..−2, odd z 1..13), slabs (1‑3, −1, even z 2..12), low walls (2‑5, −2, 1), (7‑9, −2, 1), (6, −3, 1), (6, −1, 1);
  3. trap 1 (east‑west corridor): hooks, tripwire, redstone, mossy stone (3, −3, 1); **dispenser 1** at (3, −2, 1) facing NORTH, table `minecraft:dispensers/jungle_temple` (if not yet placed); vine;
  4. trap 2 (north‑south corridor): hooks, tripwires, redstone, mossy stone (9, −3, 4); **dispenser 2** at (9, −2, 3) facing WEST, same table; 2 vines;
  5. **main chest** at (8, −3, 3), table `minecraft:chests/jungle_temple`;
  6. fixed mossy stones, selector box (9, −1, 1‑5);
  7. lever puzzle: 3 chiseled stone bricks (8‑10, −2, 11), 3 wall levers facing NORTH (8‑10, −2, 12), selector boxes (8, −3, 8‑10) and (10, −3, 8‑10), redstone, 3 sticky pistons, repeater;
  8. **hidden chest** at (9, −3, 10), same table.
- The oriented states (hooks, stairs, levers, pistons, tripwires) go through "place local block" (rotation/mirror of the orientation).
- Block-by-block measurement to do: complete list of the temple's and traps' boxes (exact coordinates and states, notably redstone connections and tripwires).

### 8.13 Swamp hut

- Start: piece placed at the chunk's corner (16cx, 64, 16cz), reference position = WORLD_SURFACE_WG surface at the chunk centre (no terrain refusal). Box 7 × 7 × 9. Saved flags: `Witch`, `Cat`.
- **Average** adjustment, offset 0. No random placement draw (neither chest nor random material).
- Geometry (local): spruce plank floor (1‑5, 1, 1‑7), flat roof (1‑5, 4, 2‑7), front step (2‑4, 1, 0), plank walls, 4 oak posts at the corners (x 1 and 5, z 2 and 7, y 0‑3), fences (2,3,2), (3,3,7), (1,2,1), (5,2,1), windows (air) (1,3,4), (5,3,4), (5,3,5), pot with red mushroom (1,3,5), crafting table (3,2,6), cauldron (4,2,6), roof edge of spruce stairs (y 4) with 4 "outer" corners, then 4 stilts: oak column down from y −1 under (1,2), (5,2), (1,7), (5,7).
- Entities, world position of local point (2, 2, 5), only if it is in the chunk and the flag false (flag set to true **before** the creation, even if it fails):
  1. **Witch** `minecraft:witch`: persistent, (x+0.5; y; z+0.5), yaw/pitch 0, "structure" spawn initialisation with the local difficulty at this position, added with passengers.
  2. **Cat** `minecraft:cat`: same steps (persistent, same position). The (black) variant is chosen by the cat's initialisation according to the structure present (see entities spec).

### 8.14 Buried treasure

- Start: reference = ocean floor (OCEAN_FLOOR_WG) at the chunk centre; piece = single-block box at (16cx+9, 90, 16cz+9). Nothing saved except the box.
- Placement (in the chunk that contains the column):
  1. y = OCEAN_FLOOR_WG height at (x, z) of the box. Go down while y > min_Y: let B = current block, S = block below.
  2. If S ∈ {sandstone, stone, andesite, granite, diorite}: "soft" block M = B if it is neither air nor water/lava, otherwise sand. For each direction in the order [DOWN, UP, NORTH, SOUTH, WEST, EAST]: neighbour V; if V is air or liquid: if the block under V is air or liquid **and** the direction is not UP → place S at V, otherwise place M at V (placement with update, flags 3). Then box = this block, reoriented **chest** at (x, y, z), table `minecraft:chests/buried_treasure`, seed `next_long()` of the step random source; end.
  3. Otherwise y −= 1.
- Note: "liquid" here only covers water and lava (comparison by block).



## 9. References, per-chunk placement, terrain adaptation

### 9.1 Boxes and references

- Box of a start = bounding box of all its pieces, widened by 12 on each face if
  `terrain_adaptation` ≠ `none` (this is the one used for the references § 7.4).
- The pieces are kept in construction order; the FIRST piece serves as the
  reference: reference point = (x centre of its box, y_min of its box, z centre), with
  "centre" of a box = `min + (max − min + 1)/2` per axis (integer division). Careful:
  the centre used by the jigsaw in § 8.1 is `(min + max)/2`, different.
- A start can be referenced at most once by the locate command (reference counter)
  — without effect on generation.

### 9.2 Placement in a chunk (structure step)

At the structure's `step` (§ 2.3), for each start referenced by
chunk C (reference order, § 7.4):
```
zone = (16cx, min_y + 1, 16cz) .. (16cx + 15, max_y, 16cz + 15)          (clipping box)
for each piece whose box intersects the zone (piece order):
    piece.place(level, step_random, zone, C, reference_point)
type-specific after-placement (pyramid: suspicious sand; mansion: cobblestone foundations)
```
The random source is the step's (`feature_seed(deco_seed, k, s)`, § 2.3), SHARED by all
the pieces and all the starts of this structure in this chunk, in this order. The
pieces only write in the zone of chunk C (except documented exceptions: ruined
portal, Nether fossil, which extend their clipping — limited by the 3×3 write area).

`spawn_overrides` (category → `bounding_box` `piece`/`full` + `spawns`): outside block
generation (natural spawns); see the entity specs.

### 9.3 Terrain adaptation (`terrain_adaptation`)

Structures with adaptation modify the terrain DENSITY (status `terrain`, before
decoration) through an additive contribution computed from the starts referenced by the
chunk (references: § 7.4); how this contribution is added to the final density is
in `generation-terrain.md`; here is the contribution itself.

Collection for a chunk C: for each referenced start of a structure with adaptation
≠ `none` (order: structures in the order of the reference table, then references); for
each "nearby" piece (box intersecting the columns of C widened by 12):
- jigsaw piece: if projection `rigid` → "rigid" (box, adaptation, ground_delta of the
  piece); and for each of its junctions whose (x, z) is strictly within ]16cx − 12,
  16cx + 15 + 12[ × ]16cz − 12, 16cz + 15 + 12[ → junction retained;
- other piece → "rigid" (box, adaptation, ground_delta = 0).
Effect zone = bounding box of the rigid ones and of the junctions' points (x, y_ground, z), widened by 24;
contribution zero outside this zone.

```
contribution(x, y, z) = Σ_rigid r + Σ_junctions j    (single-precision floats)
rigid (box B, adaptation A, delta d) :
    dx = max(0, B.x_min − x, x − B.x_max) ; dz likewise ; ground = B.y_min + d ; ds = y − ground
    dy = A ∈ {bury, beard_thin} : ds
         A = beard_box : max(0, ground − y, y − B.y_max)
         A = encapsulate : max(0, B.y_min − y, y − B.y_max)
    A = bury        : buried(dx, dy/2, dz)
    A = beard_thin, beard_box : 0.8 · beard(dx, dy, dz, ds)
    A = encapsulate : 0.8 · buried(dx/2, dy/2, dz/2)
junction (x0, y0, z0) : 0.4 · beard(x − x0, y − y0, z − z0, y − y0)
buried(a, b, c) : q = a² + b² + c² ; q ≥ 36 → 0 ; otherwise 1 − √q / 6
beard(dx, dy, dz, ds) : if dx, dy, dz ∉ [−12, 11] → 0
    h = ds + 0.5 ; q = dx² + h² + dz² (float)
    v = −h · fast_inv_sqrt(q / 2) / 2
    return v · K[dz+12][dx+12][dy+12]
K[i][j][k] = exp(−((j−12)² + (k−12+0.5)² + (i−12)²) / 16)   (table 24³ in single-precision float,
    computed in double then converted; index = z·576 + x·24 + y)
fast_inv_sqrt(x) (double) : half = 0.5x ; i = bits(x) ; i = 0x5FE6EB50C7B537AA − (i >> 1) ;
    y = double(i) ; return y · (1.5 − half · y · y)          (ONE Newton iteration: not the exact 1/√x!)
```
Note: the constant 6910469410427058090 = 0x5FE6EB50C7B537AA; `q/2` is computed in single-precision
float then promoted to double.

## 10. NBT templates, placement and processors

### 10.1 Template format

Files `data/<ns>/structure/<path>.nbt` (gzip-compressed NBT; 1511 files in 26.3, all
in `DataVersion` 5023 = current version: no data conversion is necessary for
the game's templates; an older template from a data pack would have to go through the data
upgrade, not covered here). Root:
- `size`: [sx, sy, sz];
- `palette`: list of block states (`Name`, `Properties`) — OR `palettes`: list of
  palettes (20 templates have 8 in 26.3: material variants);
- `blocks`: [{`pos` [x,y,z], `state` (index into the palette), optional `nbt` (block
  entity)}];
- `entities`: [{`pos` [x,y,z] doubles, `blockPos` [x,y,z], `nbt`}].

On loading, for each palette, the blocks are split into three lists, each sorted
by increasing (y, x, z): 1) blocks WITHOUT a block entity whose collision shape is a full
cube and without dynamic shape, 2) other blocks without an entity, 3) blocks WITH a block entity. The
palette's list = 1 then 2 then 3. This is the order of processing and placement. The
jigsaw blocks (`jigsaw`) of a palette = sub-list of the `jigsaw` blocks in this order, with their
data (`name`, `pool`, `target`, `joint` (`aligned`/`rollable`, default: `aligned` if the front
face is horizontal, otherwise `rollable`), `placement_priority` (0), `selection_priority` (0),
`final_state` (default `minecraft:air`)).

### 10.2 Transformations

Placement settings: mirror (`none`, `left_right` = z ↦ −z, `front_back` = x ↦ −x), rotation
(`none`, `clockwise_90`, `clockwise_180`, `counterclockwise_90`), pivot (px, pz), optional
clipping box, optional random source, palette, processors, "known shape", liquid
handling (`apply_waterlogging` default / `ignore_waterlogging`).
```
transform((x,y,z)) : mirror first, then
    none    : (x, y, z)
    90 cw   : (px + pz − z, y, pz − px + x)
    180     : (2px − x, y, 2pz − z)
    90 ccw  : (px − pz + z, y, px + pz − x)
world position of a block = transform(position in the template) + template_position
entities (double) : mirror x ↦ 1 − x / z ↦ 1 − z ; 90 cw : (px + pz + 1 − z, y, pz − px + x) ;
    180 : (2px + 1 − x, y, 2pz + 1 − z) ; 90 ccw : (px − pz + z, y, px + pz + 1 − x)
rotated size : 90° (either way) → (sz, sy, sx)
bounding box : the two corners (0,0,0) and size − 1 transformed ; min/max ; + position
"zero" position (origin corner so that a rotated template occupies the same footprint) :
    a = sx − 1, b = sz − 1 ; mx = (front_back ? a : 0) ; mz = (left_right ? b : 0)
    90 ccw : + (mz, 0, a − mx) ; 90 cw : + (b − mz, 0, mx) ; 180 : + (a − mx, 0, b − mz) ; none : + (mx, 0, mz)
```
States: mirror then rotate the state (per-block rules: faces, axes, rotations
of 0..15…).

Palette choice: `palettes[random.next_int(n)]` where random = the settings' random source if it is fixed,
otherwise a NEW congruential generator seeded with `position_seed(template_position)` (the same choice at
each call for a given position). `position_seed(x, y, z)` (long):
`s = (x·3129871) XOR (z·116129781) XOR y ; s = s·s·42317861 + s·11 ; s >> 16`.

### 10.3 Placing a template

```
place(level, position, reference, settings, random, flags) :
    if no palette → false
    L = chosen palette (§ 10.2)
    if L empty and (entities ignored or none) → false ; if a size dimension < 1 → false
    P = process(L)                                      (§ 10.4)
    for each b of P (order) :
        if clipping box and b.pos outside the box → next
        state = rotate(mirror(b.state))
        if b has a block entity : first place a barrier (flags 820 = no update)
        place state with `flags` ; if the placement succeeds :
            grow the box of the blocks placed ; memorise (pos, nbt)
            if nbt and a block entity exists : if it is a loot container (chests,
               barrels, dispensers, droppers, hoppers, crafters, shulker boxes, decorated pots ;
               NOT the suspicious blocks) : nbt.`LootTableSeed` =
               random.next_long() (random = placement parameter, NOT the settings') ; then load
               the block entity from nbt
            if waterlogging applied :
                if the placed state is a fluid source → "locked fluid"
                otherwise if the block can hold a liquid : pour into it the fluid that was there
                     before the placement ; if this fluid was not a source → "to fill"
    filling : as long as a pass filled something : for each position to fill
        (order), look among up, north, east, south, west for an unlocked source neighbour ;
        if found : pour this source into it ; remove it from the list
    if at least one block placed :
        if the shape is not "known" : shape updates at the edges (§ 5.1.7, but on the
           shape of the blocks placed), then for each block placed (order) : recompute the state
           from its neighbours (place with flags & ~1 | 16 if changed) and notify the neighbours
        for each block placed with nbt : block entity marked modified
    template entities (if not ignored) : position transformed ; if in the box : create
        the entity (UUID removed, `block_pos` rewritten), yaw = rotation(entity) + mirror − yaw,
        and if "finalise" and it is a mob : structure spawn
    true
```

### 10.4 Processors (`processor_list`: `{"processors": [...]}` or bare list)

Processing: "whole" = one of the processors evaluates the piece's whole state (`capped`). For
each block of the palette (order): world position = transformed + position; if not whole and
clipping box set and position outside the box → ignored; otherwise the processors are chained in
order (a processor can delete the block → end of chain). Then each processor receives
the final list for a finalisation pass (only `capped` makes use of it).

| `processor_type` | Effect on a block b (world position w, template position t) |
|---|---|
| `nop` | nothing |
| `block_ignore` (`blocks`: list of states, only the block counts) | deletes b if its block is listed |
| `block_rot` (`integrity`, optional `rottable_blocks`) | random = the settings' random source, otherwise congruential(position_seed(w)); if (no list or b ∈ list) and NOT (next_float() ≤ integrity) → deleted. (Draw only if b is concerned.) |
| `rule` (`rules` [{`input_predicate`, `location_predicate`, `position_predicate` (true), `output_state`, `block_entity_modifier` (`passthrough`)}]) | random = NEW congruential(position_seed(w)) per block; first rule such that input(b.state, w) AND location(WORLD block at w, w) AND position(t, w, reference) → b becomes output, nbt = modifier(random, nbt) |
| `gravity` (`heightmap` `WORLD_SURFACE_WG`, `offset` 0) | b moved to y = map(w.x, w.z) + offset + t.y (in generation the requested map is used as is) |
| `jigsaw_replacement` | if b is a jigsaw block: state = its `final_state` (parsed; `structure_void` → deleted), nbt removed |
| `lava_submerged_block` | if the world has lava at w and the shape of b is not a full cube → b becomes lava |
| `protected_blocks` (`value`: set) | deleted if the WORLD block at w ∈ set |
| `block_age` (`mossiness`) | see below |
| `blackstone_replace` | fixed stone → blackstone table (cobblestone/mossy cobblestone → blackstone; stone → polished blackstone; bricks → polished blackstone bricks; corresponding stairs, slabs, walls; chiseled bricks → chiseled polished blackstone; cracked → cracked; iron bars → chain) keeping `facing`, `half`, `type` |
| `capped` (`delegate`, `limit`) | finalisation: random = congruential(world seed) → positional factory (seed = next_long()) → random at the template's `position`; m = min(limit.sample(), n); if m ≥ 1: shuffled indices; for each index while < m replacements: apply `delegate` to the block processed; if it changes → replaced, counter++ |

`block_age`: random = the settings' random source, otherwise congruential(position_seed(w)).
- stone bricks / stone / chiseled stone bricks: if next_float() ≥ 0.5 → unchanged; otherwise
  (computed BEFORE the choice: "non-mossy" stairs randomly oriented = drawn horizontal direction then drawn half (`next_int(2)`), then mossy stairs likewise): if next_float() <
  mossiness → [mossy bricks, mossy stairs][next_int(2)], otherwise [cracked bricks,
  stairs][next_int(2)].
- stairs: next_float() ≥ 0.5 → unchanged; otherwise next_float() < mossiness → [mossy stairs
  (same properties), mossy slab][next_int(2)], otherwise [stone slab, brick slab][next_int(2)].
- slabs: next_float() < mossiness → mossy brick slab (same properties).
- walls: next_float() < mossiness → mossy brick wall.
- obsidian: next_float() < 0.15 → crying obsidian.

Block entity modifiers: `passthrough` (nbt unchanged), `clear` (empty nbt),
`append_static` (`data` merged), `append_loot` (`loot_table`; adds `LootTable` and
`LootTableSeed` = `random.next_long()`).

Position predicates (§ 4.2); `location_predicate` tests the state of the WORLD.

### 10.5 Contents and markers

- Structure blocks in `data` mode: after placing a coded template piece, each structure block
  (of the palette, in the clipping box) in DATA mode triggers the piece's own processing
  with its `metadata` string (e.g. "Chest", "Sentry", "Elytra"…).
- Jigsaw blocks remaining after placing a template piece (outside jigsaw, see § 8.2):
  replaced by their `final_state` ("place and update").
- Loot containers placed by a template: loot seed ALWAYS drawn at placement
  (`next_long()` of the placement random source, in block placement order), even if the nbt carried
  one (including the one from `append_loot`, overwritten).

## 11. Block-by-block verification against the official server

### 11.1 Reference world

1. Official 26.3 server launched in a NEW folder (never the shared test server),
   `server.properties`: `level-seed=<seed>`, `level-type=minecraft:normal`,
   `generate-structures=true`, `spawn-protection=0`, RCON enabled; no player connected
   (players trigger loadings in an unpredictable order).
2. Generation of a square area of N×N chunks through RCON: `forceload add` by blocks of
   chunks (the command's limit forces splitting), wait for the chunks to be
   COMPLETE (status `full`: query `/execute if loaded` or re-read the regions), then
   `save-all flush` and a clean shutdown.
3. Reading the region files (`region/r.X.Z.mca`, Anvil format: header of 1024
   slots, zlib-compressed NBT chunks): for each section, palette + packed bit array
   (width = max(4, ⌈log2(palette size)⌉), no overlap between long integers
   since 1.16) → state of each block; plus `block_entities` (chests: `LootTable`,
   `LootTableSeed`), `structures` (`starts`, `References`), `Heightmaps`, `PostProcessing`.
   Tool to write on the Rust side (Anvil reader, shared with the singleplayer's saves).

### 11.2 Our world

Same seed, same area, generated by the Rust singleplayer. "Trace" mode (off by default) that
logs: decoration seed of each chunk, for each (step, index) the placed
feature, its final positions and the number of draws consumed; structure starts
(type, chunk, pieces, boxes); order of the chunks decorated.

### 11.3 Comparison and triage of differences

1. **Structure starts** first: compare, chunk by chunk, the `starts` NBT
   (identifier, position, pieces and boxes) and the `References`. Independent of the generation
   order: must be 100 % identical. A difference here = placement error (§ 7) or
   assembly error (§ 8).
2. **Terrain without decoration**: handled by `generation-terrain.md` (comparison of the "base" blocks).
3. **Final blocks**: per-block difference, classified:
   - "order": differing block located within ≤ 16 blocks of a chunk border AND within the footprint
     of a feature that overflows (trees, lakes, geodes…) — expected, due to the decoration order
     (§ 1); they are rerun by generating OUR world in the exact chunk order of the server
     if we can reproduce it (see 11.4), otherwise they are counted separately;
   - "seed": difference concentrated in an entire chunk of the same step → wrong seed or feature
     index (topological sort § 2.2, order of possible biomes);
   - "feature": difference limited to the footprint of a feature → the feature's algorithm;
   - "structure": difference in a piece's box.
   Indicators: % of identical blocks outside the "order" category (target 100 %), number of
   chests with the same table and the same seed.
4. **Block entities**: compare `LootTableSeed` of structure chests (sensitive to the exact order
   of the draws); the spawners (entity); the beehives (number of bees).

### 11.4 Neutralising the decoration order

- Control zone: generate in the official server an isolated zone by forcing the loading of
  ONLY one row of chunks at a time, waiting for `full` before the next; the decoration order
  between neighbours then becomes (almost) deterministic: our singleplayer reproduces the same
  order (row by row, increasing x).
- Cross-check: generate the same zone twice in the official server with two different
  orders; the blocks that differ between these two official worlds form the
  "order-dependent" set, excluded from the strict comparison.

### 11.5 Targeted tests (without a full world)

- **Templates and processors**: official template placement command with explicit rotation, mirror,
  integrity and SEED (deterministic); compare with our placement (§ 10) on a flat
  ground. Covers palettes, transformations, `block_rot`, waterlogging, entities.
- **Structure assembly**: the official structure placement command builds the
  pieces deterministically from the world seed and the chunk; only the PLACEMENT draws
  use a non-reproducible random source: compare the pieces' boxes (read from the
  starts' NBT) rather than the blocks.
- **Isolated features**: the feature placement command uses the level's random source (not
  reproducible): unusable for a block-exact comparison; instead we test on a custom flat
  world (`flat` preset with a reduced list of biomes and features) where a single
  feature is active, which reduces decoration to a known draw per chunk.
- **Seeds**: unit tests of the formulas § 2.4 and § 7.2 against recorded values
  (positions of villages / strongholds known for a seed via the official locate
  command — `locate structure` returns the placement chunk).

## Appendix A. Placed features per step and per biome (26.3)

Automatic extraction of the `features` lists of the `worldgen/biome/*.json` files (67 biomes),
biomes grouped when their list is identical for the step. Order of the features = order of the
file (which constrains the topological sort § 2.2). Identifiers without `minecraft:`. The game re-reads
these files: this appendix serves as a check, not as a source.

### A.0 `raw_generation`

- **small_end_islands** : end_island_decorated

### A.1 `lakes`

- **badlands, bamboo_jungle, beach, birch_forest, cherry_grove, cold_ocean, dappled_forest, dark_forest, deep_cold_ocean, deep_frozen_ocean, deep_lukewarm_ocean, deep_ocean, desert, dripstone_caves, eroded_badlands, flower_forest, forest, frozen_ocean, frozen_peaks, frozen_river, grove, ice_spikes, jagged_peaks, jungle, lukewarm_ocean, lush_caves, mangrove_swamp, meadow, mushroom_fields, ocean, old_growth_birch_forest, old_growth_pine_taiga, old_growth_spruce_taiga, pale_garden, plains, river, savanna, savanna_plateau, snowy_beach, snowy_plains, snowy_slopes, snowy_taiga, sparse_jungle, stony_peaks, stony_shore, sunflower_plains, swamp, taiga, warm_ocean, windswept_forest, windswept_gravelly_hills, windswept_hills, windswept_savanna, wooded_badlands** : lake_lava_underground, lake_lava_surface
- **sulfur_caves** : lake_lava_underground, lake_lava_surface, rooted_sulfur_spring, sulfur_pool

### A.2 `local_modifications`

- **badlands, bamboo_jungle, beach, birch_forest, cherry_grove, cold_ocean, dappled_forest, dark_forest, deep_cold_ocean, deep_dark, deep_lukewarm_ocean, deep_ocean, desert, eroded_badlands, flower_forest, forest, frozen_peaks, frozen_river, grove, ice_spikes, jagged_peaks, jungle, lukewarm_ocean, lush_caves, mangrove_swamp, meadow, mushroom_fields, ocean, old_growth_birch_forest, pale_garden, plains, river, savanna, savanna_plateau, snowy_beach, snowy_plains, snowy_slopes, snowy_taiga, sparse_jungle, stony_peaks, stony_shore, sulfur_caves, sunflower_plains, swamp, taiga, warm_ocean, windswept_forest, windswept_gravelly_hills, windswept_hills, windswept_savanna, wooded_badlands** : amethyst_geode
- **deep_frozen_ocean, frozen_ocean** : iceberg_packed, iceberg_blue, amethyst_geode
- **old_growth_pine_taiga, old_growth_spruce_taiga** : amethyst_geode, forest_rock
- **dripstone_caves** : amethyst_geode, large_dripstone
- **soul_sand_valley** : basalt_pillar

### A.3 `underground_structures`

- **badlands, bamboo_jungle, beach, birch_forest, cherry_grove, cold_ocean, dappled_forest, dark_forest, deep_cold_ocean, deep_dark, deep_frozen_ocean, deep_lukewarm_ocean, deep_ocean, dripstone_caves, eroded_badlands, flower_forest, forest, frozen_ocean, frozen_peaks, frozen_river, grove, ice_spikes, jagged_peaks, jungle, lukewarm_ocean, lush_caves, meadow, mushroom_fields, ocean, old_growth_birch_forest, old_growth_pine_taiga, old_growth_spruce_taiga, pale_garden, plains, river, savanna, savanna_plateau, snowy_beach, snowy_plains, snowy_slopes, snowy_taiga, sparse_jungle, stony_peaks, stony_shore, sulfur_caves, sunflower_plains, taiga, warm_ocean, windswept_forest, windswept_gravelly_hills, windswept_hills, windswept_savanna, wooded_badlands** : monster_room, monster_room_deep
- **desert, mangrove_swamp, swamp** : fossil_upper, fossil_lower, monster_room, monster_room_deep

### A.4 `surface_structures`

- **deep_frozen_ocean, frozen_ocean** : blue_ice
- **basalt_deltas** : delta, small_basalt_columns, large_basalt_columns
- **desert** : desert_well
- **end_highlands** : end_gateway_return
- **ice_spikes** : ice_spike, ice_patch
- **the_end** : end_spike

### A.5 `strongholds`

### A.6 `underground_ores`

- **bamboo_jungle, beach, birch_forest, cold_ocean, dappled_forest, dark_forest, deep_cold_ocean, deep_dark, deep_frozen_ocean, deep_lukewarm_ocean, deep_ocean, desert, flower_forest, forest, frozen_ocean, frozen_river, ice_spikes, jungle, lukewarm_ocean, mushroom_fields, ocean, old_growth_birch_forest, old_growth_pine_taiga, old_growth_spruce_taiga, pale_garden, plains, river, savanna, savanna_plateau, snowy_beach, snowy_plains, snowy_taiga, sparse_jungle, stony_shore, sulfur_caves, sunflower_plains, taiga, warm_ocean, windswept_savanna** : ore_dirt, ore_gravel, ore_granite_upper, ore_granite_lower, ore_diorite_upper, ore_diorite_lower, ore_andesite_upper, ore_andesite_lower, ore_tuff, ore_coal_upper, ore_coal_lower, ore_iron_upper, ore_iron_middle, ore_iron_small, ore_gold, ore_gold_lower, ore_redstone, ore_redstone_lower, ore_diamond, ore_diamond_medium, ore_diamond_large, ore_diamond_buried, ore_lapis, ore_lapis_buried, ore_copper, underwater_magma, disk_sand, disk_clay, disk_gravel
- **cherry_grove, frozen_peaks, grove, jagged_peaks, meadow, snowy_slopes, stony_peaks, windswept_forest, windswept_gravelly_hills, windswept_hills** : ore_dirt, ore_gravel, ore_granite_upper, ore_granite_lower, ore_diorite_upper, ore_diorite_lower, ore_andesite_upper, ore_andesite_lower, ore_tuff, ore_coal_upper, ore_coal_lower, ore_iron_upper, ore_iron_middle, ore_iron_small, ore_gold, ore_gold_lower, ore_redstone, ore_redstone_lower, ore_diamond, ore_diamond_medium, ore_diamond_large, ore_diamond_buried, ore_lapis, ore_lapis_buried, ore_copper, underwater_magma, disk_sand, disk_clay, disk_gravel, ore_emerald
- **badlands, eroded_badlands, wooded_badlands** : ore_dirt, ore_gravel, ore_granite_upper, ore_granite_lower, ore_diorite_upper, ore_diorite_lower, ore_andesite_upper, ore_andesite_lower, ore_tuff, ore_coal_upper, ore_coal_lower, ore_iron_upper, ore_iron_middle, ore_iron_small, ore_gold, ore_gold_lower, ore_redstone, ore_redstone_lower, ore_diamond, ore_diamond_medium, ore_diamond_large, ore_diamond_buried, ore_lapis, ore_lapis_buried, ore_copper, underwater_magma, ore_gold_extra, disk_sand, disk_clay, disk_gravel
- **dripstone_caves** : ore_dirt, ore_gravel, ore_granite_upper, ore_granite_lower, ore_diorite_upper, ore_diorite_lower, ore_andesite_upper, ore_andesite_lower, ore_tuff, ore_coal_upper, ore_coal_lower, ore_iron_upper, ore_iron_middle, ore_iron_small, ore_gold, ore_gold_lower, ore_redstone, ore_redstone_lower, ore_diamond, ore_diamond_medium, ore_diamond_large, ore_diamond_buried, ore_lapis, ore_lapis_buried, ore_copper_large, underwater_magma, disk_sand, disk_clay, disk_gravel
- **lush_caves** : ore_dirt, ore_gravel, ore_granite_upper, ore_granite_lower, ore_diorite_upper, ore_diorite_lower, ore_andesite_upper, ore_andesite_lower, ore_tuff, ore_coal_upper, ore_coal_lower, ore_iron_upper, ore_iron_middle, ore_iron_small, ore_gold, ore_gold_lower, ore_redstone, ore_redstone_lower, ore_diamond, ore_diamond_medium, ore_diamond_large, ore_diamond_buried, ore_lapis, ore_lapis_buried, ore_copper, underwater_magma, ore_clay, disk_sand, disk_clay, disk_gravel
- **mangrove_swamp** : ore_dirt, ore_gravel, ore_granite_upper, ore_granite_lower, ore_diorite_upper, ore_diorite_lower, ore_andesite_upper, ore_andesite_lower, ore_tuff, ore_coal_upper, ore_coal_lower, ore_iron_upper, ore_iron_middle, ore_iron_small, ore_gold, ore_gold_lower, ore_redstone, ore_redstone_lower, ore_diamond, ore_diamond_medium, ore_diamond_large, ore_diamond_buried, ore_lapis, ore_lapis_buried, ore_copper, underwater_magma, disk_grass, disk_clay
- **swamp** : ore_dirt, ore_gravel, ore_granite_upper, ore_granite_lower, ore_diorite_upper, ore_diorite_lower, ore_andesite_upper, ore_andesite_lower, ore_tuff, ore_coal_upper, ore_coal_lower, ore_iron_upper, ore_iron_middle, ore_iron_small, ore_gold, ore_gold_lower, ore_redstone, ore_redstone_lower, ore_diamond, ore_diamond_medium, ore_diamond_large, ore_diamond_buried, ore_lapis, ore_lapis_buried, ore_copper, underwater_magma, disk_clay

### A.7 `underground_decoration`

- **cherry_grove, frozen_peaks, grove, jagged_peaks, meadow, snowy_slopes, stony_peaks, windswept_forest, windswept_gravelly_hills, windswept_hills** : ore_infested
- **basalt_deltas** : basalt_blobs, blackstone_blobs, spring_delta, patch_fire, patch_soul_fire, glowstone_extra, glowstone, brown_mushroom_nether, red_mushroom_nether, ore_magma, spring_closed_double, ore_gold_deltas, ore_quartz_deltas, ore_ancient_debris_large, ore_debris_small
- **crimson_forest** : spring_open, patch_fire, glowstone_extra, glowstone, ore_magma, spring_closed, ore_gravel_nether, ore_blackstone, ore_gold_nether, ore_quartz_nether, ore_ancient_debris_large, ore_debris_small
- **deep_dark** : sculk_vein, sculk_patch_deep_dark
- **dripstone_caves** : dripstone_cluster, pointed_dripstone
- **nether_wastes** : spring_open, patch_fire, patch_soul_fire, glowstone_extra, glowstone, brown_mushroom_nether, red_mushroom_nether, ore_magma, spring_closed, ore_gravel_nether, ore_blackstone, ore_gold_nether, ore_quartz_nether, ore_ancient_debris_large, ore_debris_small
- **soul_sand_valley** : spring_open, patch_fire, patch_soul_fire, glowstone_extra, glowstone, patch_crimson_roots, ore_magma, spring_closed, ore_soul_sand, ore_gravel_nether, ore_blackstone, ore_gold_nether, ore_quartz_nether, ore_ancient_debris_large, ore_debris_small
- **sulfur_caves** : sulfur_spike_cluster, sulfur_spike
- **warped_forest** : spring_open, patch_fire, patch_soul_fire, glowstone_extra, glowstone, ore_magma, spring_closed, ore_gravel_nether, ore_blackstone, ore_gold_nether, ore_quartz_nether, ore_ancient_debris_large, ore_debris_small

### A.8 `fluid_springs`

- **badlands, bamboo_jungle, beach, birch_forest, cherry_grove, cold_ocean, dappled_forest, dark_forest, deep_cold_ocean, deep_frozen_ocean, deep_lukewarm_ocean, deep_ocean, desert, dripstone_caves, eroded_badlands, flower_forest, forest, frozen_ocean, frozen_river, ice_spikes, jungle, lukewarm_ocean, lush_caves, mangrove_swamp, meadow, mushroom_fields, ocean, old_growth_birch_forest, old_growth_pine_taiga, old_growth_spruce_taiga, pale_garden, plains, river, savanna, savanna_plateau, snowy_beach, snowy_plains, snowy_taiga, sparse_jungle, stony_peaks, stony_shore, sulfur_caves, sunflower_plains, swamp, taiga, warm_ocean, windswept_forest, windswept_gravelly_hills, windswept_hills, windswept_savanna, wooded_badlands** : spring_water, spring_lava
- **frozen_peaks, grove, jagged_peaks, snowy_slopes** : spring_water, spring_lava, spring_lava_frozen

### A.9 `vegetal_decoration`

- **beach, snowy_beach, stony_shore** : glow_lichen, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **frozen_peaks, jagged_peaks, stony_peaks** : glow_lichen
- **badlands, eroded_badlands** : glow_lichen, patch_grass_badlands, patch_dry_grass_badlands, patch_dead_bush_badlands, brown_mushroom_normal, red_mushroom_normal, patch_sugar_cane_badlands, patch_pumpkin, patch_cactus_decorated, patch_firefly_bush_near_water
- **deep_dark, dripstone_caves** : glow_lichen, patch_tall_grass_2, trees_plains, flower_plains, patch_grass_plain, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin
- **deep_frozen_ocean, frozen_ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **ice_spikes, snowy_plains** : glow_lichen, trees_snowy, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **savanna, savanna_plateau** : glow_lichen, patch_tall_grass, trees_savanna, flower_warm, patch_grass_savanna, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **windswept_gravelly_hills, windswept_hills** : glow_lichen, trees_windswept_hills, patch_bush, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **bamboo_jungle** : glow_lichen, bamboo, bamboo_vegetation, flower_warm, patch_grass_jungle, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, vines, patch_melon
- **birch_forest** : glow_lichen, forest_flowers, wildflowers_birch_forest, trees_birch, patch_bush, flower_default, patch_grass_forest, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **cherry_grove** : glow_lichen, patch_tall_grass_2, patch_grass_plain, flower_cherry, trees_cherry
- **cold_ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, seagrass_cold, kelp_cold
- **crimson_forest** : spring_lava, brown_mushroom_normal, red_mushroom_normal, weeping_vines, crimson_fungi, crimson_forest_vegetation
- **dappled_forest** : glow_lichen, trees_dappled_forest, brown_mushroom_dappled_forest, patch_red_shrub, patch_grass_forest
- **dark_forest** : glow_lichen, dark_forest_vegetation, forest_flowers, flower_default, patch_grass_forest, brown_mushroom_normal, red_mushroom_normal, patch_leaf_litter, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **deep_cold_ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, seagrass_deep_cold, kelp_cold
- **deep_lukewarm_ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, seagrass_deep_warm, kelp_warm
- **deep_ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, seagrass_deep, kelp_cold
- **desert** : glow_lichen, flower_default, patch_grass_badlands, patch_dry_grass_desert, patch_dead_bush_2, brown_mushroom_normal, red_mushroom_normal, patch_sugar_cane_desert, patch_pumpkin, patch_cactus_desert
- **end_highlands** : chorus_plant
- **flower_forest** : glow_lichen, flower_forest_flowers, trees_flower_forest, flower_flower_forest, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **forest** : glow_lichen, forest_flowers, trees_birch_and_oak_leaf_litter, patch_bush, flower_default, patch_grass_forest, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **frozen_river** : glow_lichen, trees_water, patch_bush, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **grove** : glow_lichen, trees_grove, patch_pumpkin
- **jungle** : glow_lichen, bamboo_light, trees_jungle, flower_warm, patch_grass_jungle, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, vines, patch_melon
- **lukewarm_ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, seagrass_warm, kelp_warm
- **lush_caves** : glow_lichen, patch_tall_grass_2, lush_caves_ceiling_vegetation, cave_vines, lush_caves_clay, lush_caves_vegetation, rooted_azalea_tree, spore_blossom, classic_vines_cave_feature
- **mangrove_swamp** : glow_lichen, trees_mangrove, patch_grass_normal, patch_dead_bush, patch_waterlily, seagrass_swamp, patch_firefly_bush_near_water
- **meadow** : glow_lichen, patch_tall_grass_2, patch_grass_meadow, flower_meadow, trees_meadow, wildflowers_meadow
- **mushroom_fields** : glow_lichen, mushroom_island_vegetation, brown_mushroom_taiga, red_mushroom_taiga, patch_sugar_cane, patch_firefly_bush_near_water
- **nether_wastes** : spring_lava, brown_mushroom_normal, red_mushroom_normal
- **ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, seagrass_normal, kelp_cold
- **old_growth_birch_forest** : glow_lichen, forest_flowers, wildflowers_birch_forest, birch_tall, patch_bush, flower_default, patch_grass_forest, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **old_growth_pine_taiga** : glow_lichen, patch_large_fern, trees_old_growth_pine_taiga, flower_default, patch_grass_taiga, patch_dead_bush, brown_mushroom_old_growth, red_mushroom_old_growth, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, patch_berry_common
- **old_growth_spruce_taiga** : glow_lichen, patch_large_fern, trees_old_growth_spruce_taiga, flower_default, patch_grass_taiga, patch_dead_bush, brown_mushroom_old_growth, red_mushroom_old_growth, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, patch_berry_common
- **pale_garden** : glow_lichen, pale_garden_vegetation, pale_moss_patch, pale_garden_flowers, flower_pale_garden, patch_grass_forest, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **plains** : glow_lichen, patch_tall_grass_2, patch_bush, trees_plains, flower_plains, patch_grass_plain, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **river** : glow_lichen, trees_water, patch_bush, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, seagrass_river
- **snowy_slopes** : glow_lichen, patch_pumpkin
- **snowy_taiga** : glow_lichen, patch_large_fern, trees_taiga, flower_default, patch_grass_taiga_2, brown_mushroom_taiga, red_mushroom_taiga, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, patch_berry_rare
- **soul_sand_valley** : spring_lava
- **sparse_jungle** : glow_lichen, trees_sparse_jungle, flower_warm, patch_grass_jungle, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, vines, patch_melon_sparse
- **sulfur_caves** : glow_lichen, patch_tall_grass_2
- **sunflower_plains** : glow_lichen, patch_tall_grass_2, patch_sunflower, trees_plains, flower_plains, patch_grass_plain, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **swamp** : glow_lichen, trees_swamp, flower_swamp, patch_grass_normal, patch_dead_bush, patch_waterlily, brown_mushroom_swamp, red_mushroom_swamp, brown_mushroom_normal, red_mushroom_normal, patch_sugar_cane_swamp, patch_pumpkin, patch_firefly_bush_swamp, patch_firefly_bush_near_water_swamp, seagrass_swamp
- **taiga** : glow_lichen, patch_large_fern, trees_taiga, flower_default, patch_grass_taiga_2, brown_mushroom_taiga, red_mushroom_taiga, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, patch_berry_common
- **warm_ocean** : glow_lichen, trees_water, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water, warm_ocean_vegetation, seagrass_warm, sea_pickle
- **warped_forest** : spring_lava, brown_mushroom_normal, red_mushroom_normal, warped_fungi, warped_forest_vegetation, nether_sprouts, twisting_vines
- **windswept_forest** : glow_lichen, trees_windswept_forest, patch_bush, flower_default, patch_grass_badlands, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **windswept_savanna** : glow_lichen, trees_windswept_savanna, flower_default, patch_grass_normal, brown_mushroom_normal, red_mushroom_normal, patch_pumpkin, patch_sugar_cane, patch_firefly_bush_near_water
- **wooded_badlands** : glow_lichen, trees_badlands, patch_grass_badlands, patch_dry_grass_badlands, patch_dead_bush_badlands, brown_mushroom_normal, red_mushroom_normal, patch_sugar_cane_badlands, patch_pumpkin, patch_cactus_decorated, patch_firefly_bush_near_water

### A.10 `top_layer_modification`

- **badlands, bamboo_jungle, beach, birch_forest, cherry_grove, cold_ocean, dappled_forest, dark_forest, deep_cold_ocean, deep_dark, deep_frozen_ocean, deep_lukewarm_ocean, deep_ocean, desert, dripstone_caves, eroded_badlands, flower_forest, forest, frozen_ocean, frozen_peaks, frozen_river, grove, ice_spikes, jagged_peaks, jungle, lukewarm_ocean, lush_caves, mangrove_swamp, meadow, mushroom_fields, ocean, old_growth_birch_forest, old_growth_pine_taiga, old_growth_spruce_taiga, pale_garden, plains, river, savanna, savanna_plateau, snowy_beach, snowy_plains, snowy_slopes, snowy_taiga, sparse_jungle, stony_peaks, stony_shore, sulfur_caves, sunflower_plains, swamp, taiga, warm_ocean, windswept_forest, windswept_gravelly_hills, windswept_hills, windswept_savanna, wooded_badlands** : freeze_top_layer
- **the_end** : end_platform
- **the_void** : void_start_platform

## Appendix B. Named configured features, by type (26.3)

Files `worldgen/feature/**.json` (235). Many features are in addition defined INLINE
in others (sub-features): the types absent here (`coral_claw`, `coral_tree`,
`no_op`, `projected_random_patchy_square`, `sculk_patch`, `single_block_pillar`,
`speleothem`, `stepped_column_cluster`, `template`) only appear inline; `fill_layer` and
`replace_single_block` are used by no game data (flat worlds / packs).

- bamboo (2): bamboo_no_podzol, bamboo_some_podzol
- block_blob (1): forest_rock
- block_column (6): cactus, cave_vine, cave_vine_in_moss, kelp, sugar_cane, twisting_vines
- block_pile (5): pile_hay, pile_ice, pile_melon, pile_pumpkin, pile_snow
- blue_ice (1): blue_ice
- bonus_chest (1): bonus_chest
- chorus_plant (1): chorus_plant
- delta_feature (1): delta
- disk (5): disk_clay, disk_grass, disk_gravel, disk_sand, ice_patch
- end_gateway (2): end_gateway_delayed, end_gateway_return
- end_island (1): end_island
- end_platform (1): end_platform
- end_podium (2): end_podium_active, end_podium_inactive
- end_spike (1): end_spike
- fallen_tree (6): fallen_birch_tree, fallen_jungle_tree, fallen_oak_tree, fallen_poplar_tree, fallen_spruce_tree, fallen_super_birch_tree
- fossil (2): fossil_coal, fossil_diamonds
- freeze_top_layer (1): freeze_top_layer
- geode (1): amethyst_geode
- huge_brown_mushroom (1): huge_brown_mushroom
- huge_fungus (4): crimson_fungus, crimson_fungus_planted, warped_fungus, warped_fungus_planted
- huge_red_mushroom (1): huge_red_mushroom
- iceberg (2): iceberg_blue, iceberg_packed
- lake (1): lake_lava
- large_dripstone (1): large_dripstone
- monster_room (1): monster_room
- multiface_growth (2): glow_lichen, sculk_vein
- netherrack_replace_blobs (2): basalt_blobs, blackstone_blobs
- ore (30): ore_andesite, ore_blackstone, ore_clay, ore_coal, ore_coal_buried, ore_copper_large, ore_copper_small, ore_diamond_buried, ore_diamond_large, ore_diamond_medium, ore_diamond_small, ore_diorite, ore_dirt, ore_emerald, ore_gold, ore_gold_buried, ore_granite, ore_gravel, ore_gravel_nether, ore_infested, ore_iron, ore_iron_small, ore_lapis, ore_lapis_buried, ore_magma, ore_nether_gold, ore_quartz, ore_redstone, ore_soul_sand, ore_tuff
- overlay (10): basalt_pillar, coral/block_decoration, coral/brain_block, coral/bubble_block, coral/fire_block, coral/horn_block, coral/tube_block, desert_well, nylium_bonemeal, weeping_vines
- random_boolean_selector (2): lush_caves_clay, mushroom_island_vegetation
- random_neighbor_spread (1): glowstone_extra
- random_selector (21): bamboo_vegetation, birch_tall, dark_forest_vegetation, mangrove_vegetation, meadow_trees, pale_garden_vegetation, trees_badlands, trees_birch, trees_birch_and_oak_leaf_litter, trees_flower_forest, trees_grove, trees_jungle, trees_old_growth_pine_taiga, trees_old_growth_spruce_taiga, trees_plains, trees_savanna, trees_snowy, trees_sparse_jungle, trees_taiga, trees_water, trees_windswept_hills
- root_system (2): rooted_azalea_tree, rooted_sulfur_spring
- scattered_ore (2): ore_ancient_debris_large, ore_ancient_debris_small
- sequence (3): sculk_patch_ancient_city, sculk_patch_deep_dark, sulfur_pool
- simple_block (37): berry_bush, brown_mushroom, bush, crimson_forest_vegetation, crimson_roots, dead_bush, dry_grass, firefly_bush, flower_cherry, flower_default, flower_flower_forest, flower_meadow, flower_pale_garden, flower_plain, flower_swamp, grass, grass_jungle, large_fern, leaf_litter, melon, moss_vegetation, nether_sprouts, pale_forest_flower, pale_moss_vegetation, patch_fire, patch_soul_fire, pumpkin, red_mushroom, red_shrub, sea_pickle, spore_blossom, sunflower, taiga_grass, tall_grass, warped_forest_vegetation, waterlily, wildflower
- simple_random_selector (5): dripleaf, forest_flowers, pointed_dripstone, sulfur_spike, warm_ocean_vegetation
- speleothem_cluster (2): dripstone_cluster, sulfur_spike_cluster
- spike (1): ice_spike
- spring_feature (6): spring_lava_frozen, spring_lava_nether, spring_lava_overworld, spring_nether_closed, spring_nether_open, spring_water
- tree (45): acacia, azalea_tree, birch, birch_bees_0002, birch_bees_0002_leaf_litter, birch_bees_002, birch_bees_005, birch_leaf_litter, cherry, cherry_bees_005, dark_oak, dark_oak_leaf_litter, fancy_oak, fancy_oak_bees, fancy_oak_bees_0002_leaf_litter, fancy_oak_bees_002, fancy_oak_bees_005, fancy_oak_leaf_litter, jungle_bush, jungle_tree, jungle_tree_no_vine, mangrove, mega_jungle_tree, mega_pine, mega_spruce, oak, oak_bees_0002_leaf_litter, oak_bees_002, oak_bees_005, oak_leaf_litter, orange_poplar, orange_poplar_leaf_litter, pale_oak, pale_oak_bonemeal, pale_oak_creaking, pine, red_poplar, red_poplar_leaf_litter, spruce, super_birch_bees, super_birch_bees_0002, swamp_oak, tall_mangrove, yellow_poplar, yellow_poplar_leaf_litter
- underwater_magma (1): underwater_magma
- vegetation_patch (6): clay_with_dripleaves, moss_patch, moss_patch_bonemeal, moss_patch_ceiling, pale_moss_patch, pale_moss_patch_bonemeal
- vines (1): vines
- void_start_platform (1): void_start_platform
- waterlogged_vegetation_patch (1): clay_pool_with_dripleaves
- weighted_random_selector (8): large_basalt_columns, seagrass_mid, seagrass_short, seagrass_slightly_less_short, seagrass_tall, small_basalt_columns, sulfur_spring, trees_dappled_forest

Placement modifiers used by the 273 placed features (number of occurrences): biome=211, block_predicate_filter=155, count=201, count_on_every_layer=8, environment_scan=12, fixed_placement=1, height_range=86, heightmap=111, in_square=198, noise_based_count=4, noise_threshold_count=6, offset=93, rarity_filter=55, surface_relative_threshold_filter=3, surface_water_depth_filter=26. Not used in 26.3: `random_chance` (inline only),
`randomly_selected` and `cuboid` (inline), `fixed_placement` (1).



## Uncertain points

Decoration and features:
1. **Chunk decoration order**: the server decorates in an order that depends on
   requests and threads; overlaps at the borders are not reproducible down to the block
   without reproducing this order (§ 1, § 11.4). Measurement to do: size of the
   "order-dependent" set over a typical area.
2. **`_WG` maps frozen during decoration**: deduced from the code (only the maps of the current
   status are kept up to date); to be confirmed by a test (lake then tree at the same spot).
3. **Java hash sets** (trees, fallen trunks, vegetation patches): iteration
   model described in § 5.0; the "tree" buckets (≥ 8 collisions) are not modelled;
   to be confirmed by comparison on trees with vines / bees.
4. **Order of the references** (open-addressing set of longs, § 7.4): model of the
   fastutil 8.5.18 library (mixing, probing, decreasing iteration, key 0 first)
   re-read in the decompiled library; exact growth threshold (size ≥ 24 for 32
   slots, then required size for size+1) to be confirmed by a test. Only matters if two starts of the same structure touch a chunk
   (mineshafts mostly).
5. **Registry order** (index k of the structures in a step, § 2.3; order of the sets,
   § 7.3): sort by identifier (path then namespace) deduced from the registry loader;
   to be checked on a world with two structures of the same step in a chunk.
6. **Order of the possible biomes** of the multi-noise source (topological sort § 2.2): depends on
   the parameter list of the `overworld` preset; an order difference would change the feature
   indices and therefore all the feature seeds — test as a priority.
7. **Shape updates**  after trees and templates: we assume
   they do not consume the decoration randomness (they use the region random source);
   the exact traversal order of the voxel shape's faces is not described (assumed without effect).
8. **Noises**: "no offset" simplex, stack of simplexes of the `frozen` modifier, "parity"
   normal noise of the geodes and of the noise-based providers — exact construction deferred to
   `generation-noise.md`; to be cross-checked when that spec is written.
9. **Multi-face / sculk spreading**: described at the algorithm level; the attachment
   and placement rules of the multi-face blocks belong to the block
   behaviours and are not detailed here.
10. **`relative_to_sea_level` anchor** (new 26.3): sea level of the GENERATOR
    (not of the world) — consistent with the code, but no official feature uses it yet.
11. **Loot seed of containers placed by a template**: drawn at placement from the
    placement random source, which makes the seed depend on the order of the blocks and pieces in the chunk;
    comparing `LootTableSeed` is therefore a very sensitive test (good indicator, but any difference
    in order upstream breaks it).

Structures:
12. **Pieces shared between chunks** (shipwreck: adjusted height memorised at the first chunk
    placed; portal and fossil: extended clipping): behaviour tied to the piece object kept in
    memory; after a reload, the saved state is authoritative. To be checked on a world
    saved/re-read between two chunks.
13. **Block-by-block geometry of the coded structures** (mineshaft, fortress, stronghold,
    monument, mansion, temples, hut): the assembly is described (§ 8.6–8.14) but the exhaustive
    measurement of the boxes and blocks of each piece remains to be done in a dedicated spec
    (`generation-coded-structures.md`), piece by piece, with comparison to the server.
14. **Entities placed by structures** (drowned, shulkers, witches, guardians, villagers of the
    templates): outside block comparison; their finalisation (random equipment) uses
    the world random source, not reproducible.


Coded structures (measurements of § 8.6–8.8, prefix A; § 8.9–8.14, prefix B):
- (A1) **Mineshaft — persistent flags**: "spawner placed" (spider corridor), "chest placed" (stronghold chest corridor), "spawner placed" (throne, portal room) and "chest planned" (castle turns) are mutable states of the piece, modified during the placement of a chunk and re-read for the following chunks. The result therefore depends on the chunk generation order (and on the saving of these flags in the structure data). To be decided: reproduce this effect (chunk order) or place deterministically.
- (A2) **Mineshaft corridor — side branches**: depth passed on p+1 while the creation function adds 1 again; the side branches therefore have depth +2. Direct reading of the decompiled code, to be confirmed on the bench.
- (A3) **Mineshaft crossing**: it has no orientation of its own ⇒ its placements are in absolute, non-reoriented coordinates; check that the spec's "place local block" primitive handles "absent orientation = world coordinates".
- (A4) **Mineshaft room**: the half-sphere uses 32-bit floats; threshold 1.05 inclusive. Rounding edge cases to be checked on the bench.
- (A5) **Fortress — bridge end out of range**: it is created (draw `n()` consumed) then thrown away. Confirmed by reading, surprising: keep as is.
- (A6) **Fortress — weight traversal after failure**: continuing the traversal with negative w (trying the next entry) is a behaviour of the original code, not a reading error; same for the stronghold.
- (A7) **Fortress — throne**: a fence at y = 8 sticks out of the box (height 8 ⇒ y 0..7); placed if in the clipping. The columns down do not cover z 7..8.
- (A8) **Bridge end** (resolved): the "single-thread" generator initialised by its own seed is the same legacy congruential one (verified).
- (A9) **Stronghold — shell with air skipping**: the number of draws depends on the terrain already placed (caves, other structures, order of the decoration steps). Any terrain divergence shifts the piece's placement randomness (torches, cobwebs, chests, eyes; the doors and flags, however, are drawn at assembly and do not move). Effect to reproduce exactly for block-by-block fidelity.
- (A10) **Portal eyes** (resolved): eye present if `f() > 0.9`, i.e. ≈ 10 % per frame; complete portal ≈ 1e-12.
- (A11) **Stronghold — retry**: the retry condition only tests "builder empty or no portal room". A portal room added but never taken out of the queue is impossible (the queue is emptied), so it is equivalent to "portal room created".
- (A12) **Stronghold — forced piece**: the forced five-way crossing does not increment its counter and does not become "last chosen"; there can therefore be 5 in total.
- (A13) **Mesa mineshaft**: centre = min + (extent / 2) per axis (extent = max − min + 1, positive: no rounding ambiguity), on the bounding box of ALL the pieces. Resolved.
- (A14) **Mineshaft ritual draw**: the initial `f()` is consumed before the room is built; check that no other draw (spawn probability) precedes it, the frequency being handled by the placement.
- (B1) Monument: the fixed Y 39 and the water height `max(sea, 64) − 39` are read as is; to be confirmed by a capture on a world with a modified sea level.
- (B2) Monument: the initial water box overflows by one row (x, z = 58) outside the piece's box — check on a capture that the row is indeed placed.
- (B3) Monument: variants 1 and 2 of the simple room, interiors of the wings and exterior walls only summarised; block-by-block measurement to do.
- (B4) Elder guardian: persistence is not set by the piece (unlike the witch/the cat); despawn behaviour to be checked in the entities spec.
- (B5) Mansion: interpretation of the "D_ρ(n)" displacements with ρ rotating during the walk along the walls — to be validated by a comparison of piece positions on a known seed (list of the pieces of a saved structure).
- (B6) Mansion: the "Chest" markers without a suffix keep the default state (NORTH face, not rotated) — to be confirmed in the real templates.
- (B7) Mansion: the composition "ρp then R" of the 1×1 room and the rotation of the offset vector by R — to be validated on a capture.
- (B8) Non-deterministic chance: number of allays of the mansion, boolean and collapsed roof of the pyramid's cave (world random source) — impossible to reproduce down to the block; accept the difference (comparisons to exclude).
- (B9) Temple and hut: height frozen by the first chunk placed → depends on the chunk generation order; reproduce the order or accept differences of ±1 to 2 blocks.
- (B10) Pyramid: the offset draw `next_int(3)` is consumed at each chunk even when the height is already fixed (affects the seeds of the following chests).
- (B11) Hut cat: the black variant depends on the cat's spawn logic (not read here).
