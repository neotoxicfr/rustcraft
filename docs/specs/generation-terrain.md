# Generation, part 2: terrain filling (singleplayer, `rc-sim`)

Target version 26.3. Spec in our own words, drawn from reading the decompiled official game (kept outside the repository)
and the `data/minecraft/worldgen/` data of the jar, which `rc-sim` reads at run time (architecture § 14).
Goal: same seed → same blocks, block for block.

**Cross-references.**
- `generation-noise.md` (in progress): random generators (Java-style LCG, Xoroshiro,
  positional factories `at(x, y, z)`, derivation by hashing a name), normal noises and
  simplex, density functions (`interpolated`, `cache`, `find_top_surface`, `slice`,
  `distance_to_point`, `blend_density`, `beardifier`…), noise router, biome sources. Here,
  "noise `k`" = the instance of the noise `minecraft:k` created by the world's random state, and
  "function `f`" = the sampler of the density function `f` (**f32** value).
- `weather-border.md`: temperature of a biome at a block (`frozen` modifier, noises 1234/3456).
- `lighting.md`: light stages. Structures, decoration elements ("features") and initial
  mobs: specs to come.

**Conventions.** `⌊·⌋` floor; `>>` arithmetic shift; `div` integer division truncated toward
0 (like Java) and `floor_div`/`floor_mod` toward −∞. "f32" flags a single-precision computation
that must be reproduced as is; otherwise f64. `(int)` = truncation toward 0.
**Table sine** (caves): `T[i] = (f32) sin(i / 10430.378350470453)` for `i ∈ [0, 65535]`;
`sinT(a) = T[((i64)(a·10430.378350470453)) & 65535]`, `cosT(a)` the same with `+ 16384.0` before the
truncation (argument in f64, result f32).

---

## 1. Stages of a chunk (statuses)

In 26.3 the old stages `noise`, `surface` and `carvers` are **merged** into a single
`terrain` stage. Ordered list (index = rank):

| # | status | work | direct dependencies (status ≥, radius in chunks) | write radius | heightmaps held afterwards |
|---|---|---|---|---|---|
| 0 | `empty` | nothing | — | — | WG |
| 1 | `structure_starts` | structure starts (if the "generate structures" option is true) | — | — | WG |
| 2 | `structure_references` | references | `structure_starts` r ≤ 8 | — | WG |
| 3 | `biomes` | biome filling | `structure_starts` r ≤ 8 | — | WG |
| 4 | `terrain` | noise → blocks (§ 3), surface (§ 4), carved caves (§ 7), then final maps | `structure_starts` r ≤ 8, `biomes` r ≤ 1 | 0 | final |
| 5 | `features` | decoration elements + border ticks from blending | `structure_starts` r ≤ 8, `terrain` r ≤ 1 | 1 | final |
| 6 | `initialize_light` | light sources | — | — | final |
| 7 | `light` | propagation | `initialize_light` r ≤ 1 | — | final |
| 8 | `spawn` | initial mobs | `biomes` r ≤ 1 | — | final |
| 9 | `full` | proto-chunk → live chunk (entities loaded, ticks registered) | — | — | final |

Each stage always requires the previous stage on the chunk itself (radius 0).
WG = `WORLD_SURFACE_WG` + `OCEAN_FLOOR_WG`; final = `WORLD_SURFACE`, `OCEAN_FLOOR`,
`MOTION_BLOCKING`, `MOTION_BLOCKING_NO_LEAVES`.

**Cumulative dependencies** (minimum status required at Chebyshev distance `r`, taking into account
what the neighbors themselves required):
- `terrain`: r 0–1 `biomes`, r 2–9 `structure_starts`.
- `features`: r 0–1 `terrain`, r 2 `biomes`, r 3–10 `structure_starts`.
- `light`: r 0–1 `initialize_light`, r 2 `terrain`, r 3 `biomes`, r 4–11 `structure_starts`.
- `full`: r 0 `spawn`, r 1 `initialize_light`, r 2 `terrain`, r 3 `biomes`, r 4–11 `structure_starts`.
Accumulation rule: for stage E with parent P, let `rp` be the largest radius where E's direct dependency
is ≥ P; cumul(E)[d] = max(direct(E)[d], cumul(P)[d − rp]) over the union of the domains.

A proto-chunk's "persistent" status only becomes E at the end of stage E. Any block placement
in a proto-chunk updates the heightmaps listed for the **persistent status**
(so both WG maps during the whole `terrain` stage), and creates/primes those that are missing.

Details of the stages other than `terrain`:
- `structure_references`: for each source chunk within ±8 (x then z), each valid start whose
  box intersects the target chunk's columns (x ∈ [x_min, x_min+15], z likewise) → reference added.
- `biomes`: biomes of each section per quart (4×4×4) via the biome source
  (`generation-noise.md`), with the blending of old chunks (out of scope here).
- `features`: spec to come; decoration seed at the chunk's origin.
- `spawn`: if `disable_mob_generation` is false and the chunk is not being upgraded,
  LCG randomness seeded by the decoration seed (§ 7.2) at (x_min, z_min) of the central chunk; spec to come.

---

## 2. Noise settings (`noise_settings`) relevant here

Keys: `noise.min_y`, `noise.height` (generated volume), `default_block`, `default_fluid`,
`sea_level`, `aquifers` (absent = aquifers disabled), `material_rule` (reference to
`worldgen/material_rule/`), `noise_router.final_density`, `noise_router.chunk_surface_level`,
`legacy_random_source`, `disable_mob_generation`, `spawn_target` (spawn point, out of scope).

| preset | min_y / height | block / fluid | sea | aquifers | rule | random |
|---|---|---|---|---|---|---|
| `overworld`, `large_biomes`, `amplified` | −64 / 384 | stone / water | 63 | yes | `overworld` | Xoroshiro |
| `nether` | 0 / 128 | netherrack / lava | 32 | no | `nether` | LCG |
| `end` | 0 / 128 | end_stone / air | 0 | no | `end` | LCG |
| `caves` | −64 / 192 | stone / water | 32 | no | `overworld_caves` | LCG |
| `floating_islands` | 0 / 256 | stone / water | −64 | no | `overworld_floating_islands` | LCG |

A chunk's volume is (16, height, 16) starting from (x_min, min_y, z_min), **clamped** to the world's
height (if the resulting height is ≤ 0, the `terrain` stage does nothing).

**Generation context** (vertical anchors): `minGenY = max(world y_min, min_y)`,
`gen_depth = min(world height, height)`, `sea = sea_level`. Anchors: `absolute` → y;
`above_bottom` → `minGenY + o`; `below_top` → `gen_depth − 1 + minGenY − o`;
`relative_to_sea_level` → `sea + o`.

---

## 3. Noise filling

### 3.1 Iteration
1. Final density `final_density` sampled per block over the whole volume (f32 values).
2. For `z` from 0 to 15, then `x` from 0 to 15, then `y` **from top to bottom**:
   `s = substance(x, y, z, density)` (§ 3.3); if `s` is empty → `default_block`.
   If `s` is not air: write into the section (no other effect), update
   `OCEAN_FLOOR_WG` then `WORLD_SURFACE_WG` (§ 6), and if the aquifer asks for a fluid update
   and `s` contains a fluid → mark the position for post-processing (fluid tick when
   becoming a live chunk).
3. Then surface (§ 4), then carved caves (§ 7), then priming of the final maps (§ 6).

### 3.2 Global fluid selector
For a preset: `lava = (level −54, lava)`, `sea = (level sea, default_fluid)`.
`fluid(x, y, z) = lava if y < min(−54, sea), otherwise sea`.
A fluid status `(level, type)` gives block `y`: `type` if `y < level`, otherwise air.
Consequences: surface → water below 63 and lava below −54; Nether → lava below 32; End → air everywhere.

**Aquifers disabled** (Nether, End, caves, floating islands): `substance = empty if density > 0,
otherwise fluid(x, y, z).at(y)`; never any fluid update requested.

### 3.3 Noise aquifers
Functions read under `aquifers`: `barrier`, `fluid_level_floodedness`, `fluid_level_spread`,
`lava`, `exclusion`, `surface_level` (= `overworld/preliminary_surface_level`). Positional randomness:
factory derived from the name `minecraft:aquifer`. All function values are f32.

**Grid.** Cells of 16 × 12 × 16: `gx(b) = b >> 4`, `gy(b) = floor_div(b, 12)`,
`gz(b) = b >> 4`. The **center** of a cell (cx, cy, cz): randomness = factory.at(cx, cy, cz);
`X = cx·16 + next_int(10)`, `Y = cy·12 + next_int(9)`, `Z = cz·16 + next_int(10)` (in this order).
Cached per cell, along with its fluid status.

**Surface height** `S(x, z)`: x and z brought down to the quart (`(x >> 2) << 2`), then
`⌊surface_level(xq, 0, zq)⌋`, cached. `S' = S + 8`.

**Sampling ceiling** (once per chunk): min/max grid of the volume:
`minGX = gx(x_min − 5)`, `maxGX = gx(x_max − 5) + 1` (same for z),
`minGY = gy(y_min + 1) − 1`, `maxGY = gy(y_max + 1) + 1`. `M` = max of `S` over all the quarts of
`[minGX·16, maxGX·16 + 9] × [minGZ·16, maxGZ·16 + 9]` (step 4, anchored on `(q<<2)`).
`skipY = (gy(M + 8 + 12) + 1)·12 + 11 − 1`.

**Substance at block (x, y, z) for a density d:**
1. `d > 0` → empty (solid), no update.
2. `G = fluid(x, y, z)`. If `y > skipY` → `G.at(y)`, no update.
3. If `G.at(y)` is lava → lava, no update.
4. Anchor `ax = gx(x − 5)`, `ay = gy(y + 1)`, `az = gz(z − 5)`. For `ox ∈ {0,1}`, then
   `oy ∈ {−1,0,1}`, then `oz ∈ {0,1}`: center of the cell (ax+ox, ay+oy, az+oz), integer squared
   distance to the block. Keep the **4 closest** c1..c4 (d1 ≤ d2 ≤ d3 ≤ d4); a tie
   (`≥`) puts the new one **in front**.
5. `sim(a, b) = 1 − (b − a)/25`. `THRESHOLD = sim(100, 144) = −0.76`. `F1..F4` = statuses of the cells.
   `fl = F1.at(y)`.
6. If `s12 = sim(d1, d2) ≤ 0`: update = (`s12 ≥ THRESHOLD` and `F1 ≠ F2`); return `fl`.
7. Otherwise:
   - if `fl` is water and `fluid(x, y−1, z).at(y−1)` is lava → update true, return `fl`;
   - `b12 = s12 · P(F1, F2)`; if `d + b12 > 0` → empty (solid barrier), no update;
   - `s13 = sim(d1, d3)`; if `s13 > 0` and `d + s12·s13·P(F1, F3) > 0` → empty;
   - `s23 = sim(d2, d3)`; if `s23 > 0` and `d + s12·s23·P(F2, F3) > 0` → empty;
   - update = `F1 ≠ F2` or (`s23 ≥ THRESHOLD` and `F2 ≠ F3`) or (`s13 ≥ THRESHOLD` and `F1 ≠ F3`);
     if none of the three: update = `s13 ≥ THRESHOLD` and `sim(d1, d4) ≥ THRESHOLD` and `F1 ≠ F4`;
   - return `fl`.
   Status equality = same level and same type.

**Pressure P(A, B)** at block (x, y, z):
- `ta = A.at(y)`, `tb = B.at(y)`; if one is lava and the other water → **2.0**.
- `Δ = |nA − nB|`; if 0 → 0. `h = y + 0.5 − (nA + nB)/2`; `m = Δ/2 − |h|`.
- if `h > 0`: `c = m`, `g = c/1.5` if `c > 0` otherwise `c/2.5`;
  otherwise: `c = 3 + m`, `g = c/3` if `c > 0` otherwise `c/10`.
- `n = barrier(x, y, z)` if `−2 ≤ g ≤ 2` (sampled at most once per block), otherwise 0.
- `P = 2·(n + g)`.

**Fluid status of a center (X, Y, Z).** `G = fluid(X, Y, Z)`, `bottom = Y − 12`, `top = Y + 12`,
`lowest = +∞`, `centerSubmerged = false`. For each offset (in chunks, in this order)
`(0,0) (−2,−1) (−1,−1) (0,−1) (1,−1) (−3,0) (−2,0) (−1,0) (1,0) (−2,1) (−1,1) (0,1) (1,1)`:
point `(X + 16·ox, Z + 16·oz)`, `s = S(·)`, `s' = s + 8`, `isStart = (ox, oz) = (0, 0)`:
- if isStart and `bottom > s'` → return `G`;
- `exceeds = top > s'`; if exceeds or isStart: `G2 = fluid(px, s', pz)`; if `G2.at(s')` is
  not air: if isStart → `centerSubmerged = true`; if exceeds → return `G2`;
- `lowest = min(lowest, s)`.

Then **level**:
- if `exclusion(X, Y, Z) > 0` → partial = total = −1;
- otherwise `gap = lowest + 8 − Y`; `f = mapClamped(gap, 0, 64, 1, 0)` if centerSubmerged otherwise 0;
  `v = clamp(fluid_level_floodedness(X, Y, Z), −1, 1)`; `total = v − map(f, 1, 0, −0.3, 0.8)`;
  `partial = v − map(f, 1, 0, −0.8, 0.4)` (`map` linear unclamped, `mapClamped` clamped).
- `total > 0` → level = level of `G`; otherwise `partial > 0` → random level; otherwise
  level = −32512 ("very low", no fluid).
- Random level: cell `(floor_div(X,16), floor_div(Y,40), floor_div(Z,16))`;
  `e = fluid_level_spread(cell_x, cell_y, cell_z) × 10` **in f32** (the function is evaluated at the
  cell indices, not at blocks); `q = ⌊e/3⌋·3`; level = `min(lowest, cell_y·40 + 20 + q)`.

**Type**: type of `G`; if level ≤ −10, level ≠ −32512 and `G` is not lava:
cell `(floor_div(X,64), floor_div(Y,40), floor_div(Z,64))`, if `|lava(cell)| > 0.3` → lava.

---

## 4. Material rules (surface)

In 26.3, the old "surface rule" becomes a registry of **material rules**
(`worldgen/material_rule/`) and **conditions** (`worldgen/material_condition/`), referenceable
by identifier. They also carry bedrock, deepslate and ore veins.

### 4.1 System and randomness
- World root randomness: positional factory `R` (Xoroshiro or LCG depending on
  `legacy_random_source`, `generation-noise.md`). Named factory `N` = `R` derived from the name then
  made positional (`minecraft:aquifer`, `minecraft:ore`, `minecraft:bedrock_floor`…).
- **Surface depth** `sd(x, z) = (int)(surface(x, 0, z)·2.75 + 3.0 + R.at(x, 0, z).next_double()·0.25)`.
- **Secondary** `ss(x, z) = surface_secondary(x, 0, z)`.
- **Minimum surface** `smin(x, z) = ⌊chunk_surface_level(x, 0, z)⌋ + sd(x, z) − 8` (function
  `noise_router.chunk_surface_level`, f32 value).
- **Badlands bands**: array of 192 blocks (§ 4.5).

### 4.2 Iteration over a chunk
The root rule is "compiled" once per chunk. For `x` from 0 to 15 then `z` from 0 to 15
(local), `X, Z` absolute, `top(x, z)` = y of the highest non-air block according to `WORLD_SURFACE_WG`:
1. `h0 = top(x, z) + 1`; `surfaceBiome` = biome at block (X, h0, Z) (§ 4.8).
2. If `surfaceBiome = eroded_badlands` → "pillars" extension (§ 4.6).
3. `h = top(x, z) + 1` (re-read: the extension may have raised the column).
4. Slopes: `gX = top(min(x+1,15), z) − top(max(x−1,0), z)`, `gZ` likewise along z (the chunk alone,
   bounds repeated).
5. `stoneAbove = 0`, `waterY = −∞`, `ceiling = +∞`. For `y` from `h` down to the chunk's min:
   - air → `stoneAbove = 0`, `waterY = −∞`;
   - fluid → if `waterY = −∞`: `waterY = y + 1`;
   - solid (neither air nor fluid):
     - if `ceiling ≥ y`: search below `y` (from `y−1` down to `min − 1`, air under the world
       stopping the search) for the first non-solid block `y'` → `ceiling = y' + 1`;
     - `stoneAbove += 1`; `stoneBelow = y − ceiling + 1`;
     - if `y` is in the world: `b = rule(X, y, Z)`; if not empty → place `b` (a placed fluid
       is marked for post-processing).
6. If `surfaceBiome ∈ {frozen_ocean, deep_frozen_ocean}` → "icebergs" extension (§ 4.6).

Block placements update the WG maps (§ 1): a column's extensions therefore change the
slopes seen by the columns processed after it.

Context seen by the conditions at block (X, y, Z): `sd`, `ss`, `smin`, `gX`, `gZ`, `stoneAbove`,
`stoneBelow`, `waterY`, `y`, biome at the block (lazy).

### 4.3 Conditions (`type`)
| type | fields | true if |
|---|---|---|
| `biome` | `biome_is` (list) | biome at the block ∈ list |
| `noise_threshold` | `noise`, `min_threshold`, `max_threshold`, `is_3d` (false) | `min ≤ v ≤ max`, `v = noise(X, 0, Z)` (2D) or `noise(X, y, Z)` (3D) |
| `vertical_gradient` | `random_name`, `true_at_and_below`, `false_at_and_above` | `y ≤ t` → true; `y ≥ f` → false; otherwise `N(random_name).at(X, y, Z).next_float() < map(y, t, f, 1, 0)` |
| `y_above` | `anchor`, `surface_depth_multiplier` (−20..20), `add_stone_depth` | `y + (add ? stoneAbove : 0) ≥ anchor + sd·mult` |
| `water` | `offset`, `surface_depth_multiplier`, `add_stone_depth` | `waterY = −∞`, or `y + (add ? stoneAbove : 0) ≥ waterY + offset + sd·mult` |
| `temperature` | — | the biome at the block is cold enough for snow at (X, y, Z) with the generator's sea (temperature < 0.15, `weather-border.md`) |
| `steep` | — | `gX ≤ −4` **or** `gZ ≥ 4` (asymmetric: slope rising toward −x or toward +z) |
| `not` | `invert` | not |
| `hole` | — | `sd ≤ 0` |
| `above_preliminary_surface` | — | `y ≥ smin` |
| `stone_depth` | `offset`, `add_surface_depth`, `secondary_depth_range`, `surface_type` (`floor`/`ceiling`) | `p ≤ 1 + offset + (add ? sd : 0) + sec`, `p = stoneBelow` if `ceiling` otherwise `stoneAbove`, `sec = 0` if range = 0 otherwise `(int) map(ss, −1, 1, 0, range)` |

The per-column and per-block caches are pure optimizations (no condition has a side
effect). Allowed optimization: at compile time, a `biome` condition none of whose biomes is present
in the palettes of the 3×3 neighboring chunks is always false; if all are there, always true.

Named conditions (`material_condition/`): `on_floor` = stone_depth floor (0, without sd);
`under_floor` = floor (0, +sd); `deep_under_floor` = floor (0, +sd, sec 6);
`very_deep_under_floor` = floor (0, +sd, sec 30); `on_ceiling` / `under_ceiling` = same in
ceiling; `not_underwater` = water(−1, ×0, without stone); `not_under_deep_water` = water(−6, ×−1, +stone).

### 4.4 Rules (`type`)
- `block` (`result_state`): returns the state.
- `sequence` (`sequence`, ≥ 1): first non-empty sub-rule.
- `condition` (`if_true`, `then_run`): sub-rule if the condition is true, otherwise empty.
- `bandlands`: terracotta band (§ 4.5).
- `ore_vein`: ore vein (§ 5).
A string in place of an object = reference to a named rule/condition.

### 4.5 Badlands bands
Randomness (non-positional) = `R` derived from the name `minecraft:clay_bands`.
1. 192 `terracotta` cells.
2. `i = 0`; while `i < 192`: `i += next_int(5) + 1`; if `i < 192`: cell `i` = `orange_terracotta`; `i += 1`.
3. Three passes (base width, color): (1, `yellow_terracotta`), (2, `brown_terracotta`),
   (1, `red_terracotta`): `n = next_int(10) + 6`; n times: `w = base + next_int(3)`,
   `d = next_int(192)`, cells `d .. d+w−1` (clamped to 191).
4. White: `n = next_int(7) + 9`; `k = 0`, `d = 0`; while `k < n` and `d < 192`: cell `d` =
   `white_terracotta`; if `d − 1 > 0` and `next_boolean()` → cell `d−1` = `light_gray_terracotta`;
   if `d + 1 < 192` and `next_boolean()` → cell `d+1` = light gray; `k += 1`; `d += next_int(16) + 4`.

Band at a block: `o = round(clay_bands_offset(X, 0, Z)·4)` (Java rounding: `⌊v + 0.5⌋`);
cell `(y + o + 192) mod 192`.

### 4.6 Extensions outside the rules
**Eroded badlands pillars** (before the rules, `h0` of § 4.2):
`p = min(|badlands_surface(X, 0, Z)·8.25|, badlands_pillar(X·0.2, 0, Z·0.2)·15)`; if `p > 0`:
`r = |badlands_pillar_roof(X·0.75, 0, Z·0.75)·1.5|`;
`top = 64 + min(p²·2.5, ⌈r·50⌉ + 24)`; `yd = ⌊top⌋`. If `h0 ≤ yd`: descend from `yd`: at the first
`default_block` block we stop; if we meet **water** before, the column is abandoned.
Then from `yd` downward, while the block is air: place `default_block`.

**Frozen ocean icebergs** (after the column's rules):
`i = min(|iceberg_surface(X, 0, Z)·8.25|, iceberg_pillar(X·1.28, 0, Z·1.28)·15)`; if `i > 1.8`:
`r = |iceberg_pillar_roof(X·1.17, 0, Z·1.17)·1.5|`; `t = min(i²·1.2, ⌈r·40⌉ + 14)`;
if the temperature of the surface biome at (X, sea, Z) is > 0.1 → `t −= 2`. If `t > 2`:
`bottom = sea − t − 7`, `top = sea + t`; randomness `R.at(X, 0, Z)` (new draw):
`snowMax = 2 + next_int(4)`, `snowMin = sea + 18 + next_int(10)`, `snow = 0`.
For `y` from `max(h0, (int)top + 1)` up to `smin(X, Z)` inclusive: if
(block air and `y < (int)top` and `next_double() > 0.01`) or (block water and `y > (int)bottom` and `y < sea` and
`next_double() > 0.15`) — short-circuit evaluation, a draw only happens if the previous tests
pass — then: if `snow ≤ snowMax` and `y > snowMin` → `snow_block`, `snow += 1`; otherwise
`packed_ice`.

### 4.7 Shipped rule trees (read at run time, summarized for checking)
Notation: `IF cond → …`, numbered sequences, `@name` = named rule.

**`overworld`**: 1 `@bedrock_floor`; 2 `@overworld/copper_ore_vein`; 3 `@overworld/iron_ore_vein`;
4 IF `above_preliminary_surface` → `@overworld/surface`; 5 `@overworld/underground`.
- `bedrock_floor`: IF vertical_gradient(`minecraft:bedrock_floor`, true ≤ above_bottom 0,
  false ≥ above_bottom 5) → bedrock.
- `bedrock_roof`: IF NOT vertical_gradient(`minecraft:bedrock_roof`, true ≤ below_top 5,
  false ≥ below_top 0) → bedrock.
- `underground`: 1 IF biome sulfur_caves → `@overworld/sulfur_cave_bands`;
  2 IF vertical_gradient(`minecraft:deepslate`, true ≤ 0, false ≥ 8) → deepslate.
- `sulfur_cave_bands` (3D noise `sulfur_cave_gradient`): [−0.4; −0.1] → cinnabar;
  [0; 0.4] → sulfur; ≥ 0.4 → cinnabar (converted f32 bounds: −0.4000000059604645 etc.).

**`overworld/surface`**:
1. IF on_floor: (a) IF biome wooded_badlands AND y_above(97, ×2) → noise `surface` ∈ [−0.909;
   −0.5454] or [−0.1818; 0.1818] or [0.5454; 0.909] → coarse_dirt, otherwise `@biome_surface/default`;
   (b) IF swamp AND y_above(62, ×0) AND NOT y_above(63, ×0) AND `surface_swamp` ≥ 0 → water;
   (c) same for mangrove_swamp with y_above(60).
2. IF biome badlands/eroded_badlands/wooded_badlands:
   (a) IF on_floor: y_above(256) → orange_terracotta; IF y_above(74, ×1, +stone) → (noise
   `surface` in the three ranges above → terracotta, otherwise bands); IF not_underwater →
   (on_ceiling → red_sandstone, otherwise red_sand); IF NOT hole → orange_terracotta;
   IF not_under_deep_water → white_terracotta; `@gravel_or_stone_if_ceiling`.
   (b) IF y_above(63, ×−1, +stone): (IF y_above(63) AND NOT y_above(74, ×1, +stone) →
   orange_terracotta); bands.
   (c) IF under_floor AND not_under_deep_water → white_terracotta.
3. IF on_floor AND not_underwater: (IF frozen_ocean/deep_frozen_ocean AND hole → air);
   `@overworld/biome_surface`.
4. IF not_under_deep_water: (IF on_floor AND frozen/deep_frozen_ocean AND hole → water);
   (IF under_floor → `@overworld/under_biome_surface`); (IF warm_ocean/beach/snowy_beach AND
   deep_under_floor → sandstone); (IF desert AND very_deep_under_floor → sandstone).
5. IF on_floor: frozen_peaks/jagged_peaks → stone; warm_ocean/lukewarm_ocean/deep_lukewarm_ocean
   → `@sand_or_sandstone_if_ceiling`; otherwise `@gravel_or_stone_if_ceiling`.

`gravel_or_stone_if_ceiling` = on_ceiling → stone, otherwise gravel;
`sand_or_sandstone_if_ceiling` = on_ceiling → sandstone, otherwise sand.

**`biome_surface`** (in order): frozen_peaks (steep → packed_ice; `packed_ice` ∈ [0; 0.2] →
packed_ice; `ice` ∈ [0; 0.025] → ice; not_underwater → snow_block); snowy_slopes (steep → stone;
powder snow; not_underwater → snow_block); jagged_peaks (steep → stone; not_underwater →
snow_block); grove (powder snow; not_underwater → snow_block); then stony_peaks (`calcite` ∈
[−0.0125; 0.0125] → calcite, otherwise stone); stony_shore (`gravel` ∈ [−0.05; 0.05] →
gravel_or_stone, otherwise stone); windswept_hills (`surface` ≥ 0.1212… → stone);
warm_ocean/beach/snowy_beach and desert → sand_or_sandstone; dripstone_caves → stone;
sulfur_caves → sulfur bands then stone; mangrove_swamp → mud; windswept_savanna
(`surface` ≥ 0.2121… → stone; ≥ −0.0606… → coarse_dirt); windswept_gravelly_hills (≥ 0.2424… →
gravel_or_stone; ≥ 0.1212… → stone; ≥ −0.1212… → default; otherwise gravel_or_stone);
old_growth_pine/spruce_taiga (≥ 0.2121… → coarse_dirt; ≥ −0.1151… → podzol); ice_spikes
(not_underwater → snow_block); mushroom_fields → mycelium; dappled_forest (`small_patch` ≥ 1.2 →
coarse_dirt); finally `default` = not_underwater → grass_block, otherwise dirt.
Powder snow: `powder_snow` ∈ [0.35; 0.6] and not_underwater → powder_snow (subsurface: [0.45; 0.58]).

**`under_biome_surface`**: same dispatch with variants: frozen_peaks (steep → packed_ice;
`packed_ice` ∈ [−0.5; 0.2]; `ice` ∈ [−0.0625; 0.025]; not_underwater → snow_block);
snowy_slopes (steep → stone; subsurface powder snow; not_underwater → snow_block);
jagged_peaks → stone; grove (subsurface powder snow; dirt); the central block identical to
`biome_surface` (stony_peaks … mangrove_swamp); windswept_savanna (≥ 0.2121… → stone);
windswept_gravelly_hills (as on the surface but "default" → dirt); otherwise dirt.

**`nether`**: bedrock_floor; bedrock_roof; IF y_above(below_top 5) → netherrack;
basalt_deltas (under_ceiling → basalt; under_floor → [`patch` ≥ −0.012 AND y+stone ∈ [30, 35[ →
gravel]; `nether_state_selector` ≥ 0 → basalt; blackstone); soul_sand_valley (under_ceiling →
`nether_state_selector` ≥ 0 → soul_sand otherwise soul_soil; under_floor → same gravel then
soul_sand/soul_soil); IF on_floor: (NOT y_above(32) AND hole → lava); warped_forest (NOT
`netherrack` ≥ 0.54 AND y_above(31) → `nether_wart` ≥ 1.17 → warped_wart_block otherwise warped_nylium);
crimson_forest (same → nether_wart_block / crimson_nylium); nether_wastes (under_floor AND
`soul_sand_layer` ≥ −0.012 → (NOT hole AND y+stone ∈ [30, 35[ → soul_sand) otherwise netherrack;
on_floor AND y_above(31) AND NOT y_above(35, +stone) AND `gravel_layer` ≥ −0.012 → (y_above(32) →
gravel; NOT hole → gravel)); finally netherrack.

**`end`**: end_stone everywhere. **`overworld_caves`**: bedrock_roof, bedrock_floor, veins,
surface (without the preliminary surface test), underground. **`overworld_floating_islands`**:
veins, surface, underground.

### 4.8 Biome "at the block" (zoom blur)
Reading used by the surface: zoom seed `gz` = first 8 bytes (little-endian) of the
SHA-256 of the 8 little-endian bytes of the world seed (same value as the "hashed seed"
sent to the client).
`lcg(r, c) = r·(r·6364136223846793005 + 1442695040888963407) + c` (wrapping i64).
At block (x, y, z): `a = x−2, b = y−2, c = z−2`; parent `(a>>2, b>>2, c>>2)`, fractions
`(a&3)/4` etc. For `i` from 0 to 7 (bit 4 → x, bit 2 → y, bit 1 → z; bit at 0 = parent, at 1 =
parent+1, distance = fraction or fraction−1): `r = gz` then successive `lcg` with cx, cy, cz,
cx, cy, cz; `fx = jitter(r)`; `r = lcg(r, gz)`; `fy = jitter(r)`; `r = lcg(r, gz)`;
`fz = jitter(r)`; `jitter(r) = (floor_mod(r >> 24, 1024)/1024 − 0.5)·0.9`;
score = `(dz+fz)² + (dy+fy)² + (dx+fx)²` (in this order). Keep the first strictly
minimal score; biome of the chosen quart (y clamped to the chunk's sections).
During `terrain`, the quarts are read from the chunks of the region (biomes done at r ≤ 1);
for the caves (§ 7), read directly from the biome source.

---

## 5. Ore veins (copper, iron)

`ore_vein` rule: `ore_block`, `raw_ore_block`, `filler_block`, `raw_ore_chance` (0..1),
functions `density`, `richness`, `filler_gap`. Randomness: factory `minecraft:ore`.
At the block: `d = density`; `d ≤ 0` → empty. `a = N(ore).at(X, y, Z)`; if `a.next_float() > d` → empty.
`r = richness`; if `a.next_float() < r` **and** `filler_gap < 0` (`filler_gap` evaluated only if
the first test passes) → `a.next_float() < raw_ore_chance` ? `raw_ore_block` : `ore_block`;
otherwise `filler_block`. `density` and `richness` are pre-sampled over the chunk's surface volume
(16×16 columns, from min_y to the top of the highest non-empty section), same values as at the
point.

Shipped data: copper = copper_ore / raw_copper_block / granite; iron = deepslate_iron_ore /
raw_iron_block / tuff; `raw_ore_chance` 0.02. Functions (`density_function/overworld/ore_vein/`):
- `toggle` = `ore_veininess` (scales 1.5) interpolated 4×8, 0 outside y ∈ [−64, 57[;
- `mask` = −1 if `toggle ∈ [−0.4; 0.4[`, otherwise `0.08 − max(|A|, |B|)` (A, B = `ore_vein_a`/`_b`
  scale 4, interpolated, 1 outside [−64, 57[);
- `copper_density` = 0.7 if y ∈ [0, 50[ and mask ≥ 0 and `toggle − 0.4 + clamp(min(50−y, y−0), 0, 20)·0.01 − 0.2 ≥ 0`, otherwise −1;
- `iron_density`: same with y ∈ [−60, −8[, `−toggle` and bounds −8/−60;
- `richness = clamp(|toggle|, 0.4, 0.6) − 0.3`; `gap = −0.3 − ore_gap`.
The veins come **before** the surface and deepslate in the `overworld` rule: they
replace any solid block visited (including at the surface if the altitude allows).

---

## 6. Heightmaps during generation

A map = per column, "first free y" (y of the highest block that counts + 1; chunk min
if none). Predicates: `WORLD_SURFACE(_WG)` = non-air; `OCEAN_FLOOR(_WG)` = tag
`minecraft:blocks_motion_in_heightmap`; `MOTION_BLOCKING` = that tag or fluid;
`MOTION_BLOCKING_NO_LEAVES` = `..._no_leaves` or fluid.
**Update** on a placement at (x, y, z), `f` = first free: if `y ≤ f − 2` → nothing; if the block
counts and `y ≥ f` → `f = y + 1`; if it does not count and `y = f − 1` → go back down to the first
block that counts (`f = y' + 1`, otherwise min).
**Priming** (end of `terrain`, for the 4 final maps): per column (x then z), descend
from the top of the highest non-empty section; at each non-air block, each map still
open whose predicate is true takes `y + 1` and closes; stop when all are closed.
The WG maps serve the surface (§ 4) and stay in the proto-chunk (not sent to the client).

---

## 7. Carved caves ("carvers")

### 7.1 Types and data
Types `cave` and `canyon`; three shipped caves use `cave` (`cave`,
`cave_extra_underground`, `nether_cave`), one uses `canyon`. List per biome (`carvers` of the biome):
56 surface biomes → [`cave`, `cave_extra_underground`, `canyon`]; 5 Nether ones → `nether_cave`;
6 (End, empty) → none.

| | `cave` | `cave_extra_underground` | `nether_cave` | `canyon` |
|---|---|---|---|---|
| probability | 0.15 | 0.07 | 0.2 | 0.01 |
| y | uniform [above_bottom 8; 180] | [above_bottom 8; 47] | [0; below_top 1] | [10; 67] |
| count | heavily biased low 0..14 | 0..14 | 0..9 | — |
| thickness | trapezoid 0..3 plateau 1 | same | trapezoid 0..6 plateau 2 | trapezoid 0..6 plateau 2 |
| weird_thickness_bias | true | true | false | — |
| horizontal_radius_multiplier | U[0.7; 1.4[ | same | 1 | — |
| vertical_radius_multiplier | U[0.8; 1.3[ | same | 1 | — |
| start_vertical_radius_multiplier | 1 (default) | 1 | 5 | — |
| room_vertical_radius_multiplier | U[0.1; 0.9[ | same | 0.5 | — |
| floor_level | U[−1; −0.4[ | same | −0.7 | — |

Canyon: `vertical_rotation` U[−0.125; 0.125[, `shape`: distance_factor U[0.75; 1[,
thickness trapezoid 0..6 plateau 2, width_smoothness 3, horizontal_radius_factor U[0.75; 1[,
vertical_radius_default_factor 1, vertical_radius_center_factor 0, y_scale 3.

**Providers** (draws): constant → no draw; uniform float `next_float()·(max−min)+min`;
trapezoid `e = (max−min−plateau)/2`, `min + next_float()·(max−min−e) + next_float()·e` (f32);
heavily biased low `min + next_int(next_int(next_int(max−min+1) + 1) + 1)`; uniform height
`next_int(max−min+1) + min` (if min > max → min, without a draw).

### 7.2 Loop per target chunk
- Size mask: y from `minGenY + 1` to `minGenY + gen_depth − 1 − 7` (top 7 blocks protected;
  0 if the chunk is being upgraded).
- For `dx` from −8 to 8 then `dz` from −8 to 8: source chunk (sx, sz); the source's biome = biome at the
  quart `(x_min_s >> 2, 0, z_min_s >> 2)` read from the biome source without blur; its caves in the order
  of the list, `k` = rank (0, 1, 2…).
- Randomness: Java LCG; "large structure seed": `reseed(seed + k)`; `a = next_long()`,
  `b = next_long()`; `reseed((sx·a) ^ (sz·b) ^ (seed + k))` (i64 products).
  (Decoration seed, for the record: `reseed(g)`, `a = next_long()|1`, `b = next_long()|1`,
  `reseed((x·a + z·b) ^ g)`.)
- If `next_float() ≤ probability` → size within the target chunk's mask.
- If the mask is not empty → application (§ 7.5).

### 7.3 `cave`
`D = 112` (reach 4 chunks: `(4·2 − 1)·16`). `n = count`. For each cave:
`x = x_min_s + next_int(16)`, `y = height` (integer → f64), `z = z_min_s + next_int(16)`, then in this
order `mH = horizontal_radius_multiplier`, `mV = vertical_radius_multiplier`,
`mS = start_vertical_radius_multiplier`, `floorLevel = floor_level`. `tunnels = 1`.
If `next_int(4) = 0`: `ys = room_vertical_radius_multiplier`, `th = 1 + next_float()·6` (f32),
**room**: `rh = 1.5 + sinT((f32)π/2)·th`, `rv = rh·ys`, ellipsoid at (x + 1, y, z) (without
multipliers); `tunnels += next_int(4)`.
For each tunnel: `rotH = next_float()·(f32)2π`, `rotV = (next_float() − 0.5)/4` (f32),
`th = thickness` (if weird and `next_int(10) = 0`: `th ×= next_float()·next_float()·3 + 1`),
`dist = D − next_int(D/4)`, then tunnel(seed = `next_long()`, x, y, z, th, rotH, rotV, step 0, dist,
scale = mS).

**Tunnel** (own randomness: LCG seeded by the seed): `split = next_int(dist/2) + dist/4`,
`sharp = next_int(6) = 0`, `vx = vy = 0` (f32). For `p` from `step` to `dist − 1`:
- `rh = 1.5 + sinT((f32)π·p/dist)·th` (argument in f32); `rv = rh·scale`;
- `c = cosT(rotV)`; `x += cosT(rotH)·c`; `y += sinT(rotV)`; `z += sinT(rotH)·c` (f32 products);
- `rotV ×= sharp ? 0.92 : 0.7`; `rotV += vx·0.1`; `rotH += vy·0.1`; `vx ×= 0.9`; `vy ×= 0.75`;
  `vx += (next_float() − next_float())·next_float()·2`; `vy += (next_float() − next_float())·next_float()·4`;
- if `p = split` and `th > 1`: two branches (for each: `next_long()` then `next_float()`),
  thickness `next_float()·0.5 + 0.5`, `rotH ∓ (f32)π/2` (first −, then +), `rotV/3`, step `p`,
  same `dist`, scale 1; then the tunnel **ends**;
- otherwise if `next_int(4) ≠ 0`: if not reachable → end; otherwise ellipsoid (x, y, z, `rh·mH`, `rv·mV`).
Skip (cave): `yd ≤ floorLevel` → skipped; otherwise skipped if `xd² + yd² + zd² ≥ 1`.

### 7.4 `canyon`
`x = x_min_s + next_int(16)`, `y = height` (integer), `z = z_min_s + next_int(16)`,
`rotH = next_float()·(f32)2π`, `rotV = vertical_rotation`, `ys = y_scale`, `th = thickness`,
`dist = (int)(112·distance_factor)` (f32), seed = `next_long()`.
Own randomness (seeded LCG): width factors, for `i` from 0 to `gen_depth − 1`: if `i = 0` or
`next_int(width_smoothness) = 0` → `w = 1 + next_float()·next_float()`; `L[i] = w²`.
For `p` from 0 to `dist − 1`:
- `rh = 1.5 + sinT(p·(f32)π/dist)·th`; `rv = rh·ys`; `rh ×= horizontal_radius_factor`;
  `m = 1 − |0.5 − p/dist|·2` (f32); `rv = (default + center·m)·rv·(next_float()·0.25 + 0.75)`;
- `c = cosT(rotV)`, `s = sinT(rotV)`; `x += cosT(rotH)·c`; `y += s`; `z += sinT(rotH)·c`;
- `rotV ×= 0.7`; `rotV += vx·0.05`; `rotH += vy·0.05`; `vx ×= 0.8`; `vy ×= 0.5`; same draws
  `vx`, `vy` as the cave;
- if `next_int(4) ≠ 0`: if not reachable → end; otherwise ellipsoid (rh, rv) with skip
  `(xd² + zd²)·L[y − minGenY − 1] + yd²/6 ≥ 1`.

**Reachable** (step `p` out of `total`, thickness `th`): `ax = x − (x_min + 8)`, `az = z − (z_min + 8)`
of the **target** chunk; `remaining = total − p`; `R = th + 2 + 16` (f32); true if
`ax² + az² − remaining² ≤ R²`.

**Ellipsoid** at center (x, y, z), radii (rh, rv), target chunk: ignored if
`|x − (x_min+8)| > 16 + 2·rh` or likewise in z. Local indices
`ix ∈ [max(⌊x − rh⌋ − x_min − 1, 0), min(⌊x + rh⌋ − x_min, 15)]` (z likewise),
`y ∈ ]max(⌊y − rv⌋ − 1, bottom of the mask), min(⌊y + rv⌋ + 1, top of the mask)]` (lower bound excluded),
iterate ix, then iz, then y descending; `xd = (X + 0.5 − x)/rh`, `zd` likewise; column kept if
`xd² + zd² < 1`; `yd = (y − 0.5 − yc)/rv`; if not skipped → mask bit set.

### 7.5 Applying the mask
Columns visited by increasing index (local x major, z minor) in contiguous ranges of bits; for
each range, `grass = false`, then `y` from the top of the range downward:
- block in the tag `minecraft:uncarvable` (bedrock only) → ignored;
- if the block is grass_block or mycelium → `grass = true` (stays true until the end of the range);
- `s = substance(X, y, Z, density 0)` (§ 3.3, same aquifers as the filling); if empty (aquifer
  barrier) → block unchanged; otherwise place `s` (+ mark if fluid and update requested);
- if `grass` and the block below is dirt: evaluate the root rule **at the block below** with
  `stoneAbove = stoneBelow = 1`, `waterY = y_below + 1` if `s` is a fluid otherwise −∞, slopes
  and `sd` of the column, biome at the block from the source without cache; if not empty → place (+ mark if
  fluid).
Blending with old chunks can filter positions (out of scope for a new world).

---

## 8. Nether and End

**Nether**: `final_density` = squeeze(interpolated 4×8 of [`blend_density`(lerps of
`nether/base_3d_noise` with 2.5 at the bottom (y −8→24) and 0.9375 at the top (104→128))]·0.64) + beardifier
(details: `generation-noise.md`). No aquifer: lava below y 32, air above. LCG randomness.
Nether climate noises created "the old way" (world seed + 0 / + 1). Bedrock at the bottom
(0→5) and at the top (below_top 5→0) through gradients. `nether_cave` caves: filled with lava below 32
via the global selector. `nether` rule (§ 4.7).

**End**: `final_density` = squeeze(interpolated 8×4 of [`blend_density`(lerps of `end/sloped_cheese`
with −0.234375 at the bottom (4→32) and −23.4375 at the top (56→312))]·0.64) + beardifier;
`sloped_cheese = islands + base_3d_noise` (old blended noise, scales 0.25, factors 80/160,
smoothing 4). `end/islands` = cache(max(central island, outer islands)):
- central island (JSON): slice at y 0 of `(clamp(100 − Euclidean distance to the point (0,0,0),
  −100, 80) − 8)·0.0078125`;
- outer islands (`end_outer_islands`): 2D simplex seeded by an LCG(world seed) after
  **17,292** draws consumed. At block (x, z): `sx = x div 8`, `sz = z div 8`, `cx = sx div 2`,
  `cz = sz div 2`, `ux = sx rem 2`, `uz = sz rem 2` (signed remainders, Java); `h = −100`; for
  `ox`, `oz` from −12 to 12: `tx = cx + ox`, `tz = cz + oz` (i64); if `tx² + tz² > 4096` and
  `simplex(tx, tz) < −0.9`: `size = (|tx|·3439 + |tz|·147) mod 13 + 9` (f32, float remainder),
  `dx = ux − 2·ox`, `dz = uz − 2·oz`, `v = clamp(100 − √(dx² + dz²)·size, −100, 80)` (f32),
  `h = max(h, v)`; result `(h − 8)/128` (f32), constant in y.
No aquifer or fluid (air), no carved caves, rule = end_stone everywhere, no initial
mobs. The platform, pillars and portals belong to the decoration elements.

**Summary of differences**: volume 0..128; LCG randomness (including for named factories, see
`generation-noise.md`); no veins; no badlands/iceberg extensions (biomes absent).

---

## 9. Terrain adjustment for structures ("beardifier")

Term added by `final_density` (type `beardifier`). For the chunk: structure starts whose
`terrain_adaptation ≠ none`; pieces within ≤ 12 blocks of the chunk. A "rigid"-profile piece →
box + ground offset; its junctions whose source is in ]x_min − 12, x_min + 27[ (z likewise) →
junctions. Other pieces → box, offset 0. Useful zone = bounding box inflated by 24 (outside the zone: 0).
At the block: for each box, `dx`, `dz` = distance outside the box (≥ 0), `ground = minY_box + offset`,
`dyS = y - ground`; depending on the adaptation: `bury` → `buried(dx, dyS/2, dz)`;
`beard_thin` → `beard(dx, dyS, dz, dyS)·0.8`; `beard_box` → `beard(dx, max(0, ground − y, y − y_max), dz, dyS)·0.8`;
`encapsulate` → `buried(dx/2, dy/2, dz/2)·0.8` with `dy` = vertical distance outside the box.
Junction: `beard(Δx, Δy, Δz, Δy)·0.4` (Δ from the source on the ground).
`buried(a, b, c)` = 0 if `a²+b²+c² ≥ 36`, otherwise `1 − √(·)/6` (f32).
`beard(dx, dy, dz, dyS)`: if an index `d + 12` falls outside [0, 24[ → 0; otherwise
`e = dyS + 0.5`, `q = dx² + e² + dz²`, `v = −e·isqrt(q/2)/2` with the fast inverse square root
(constant 0x5FE6EB50C7B537AA, one Newton iteration, in f64 then f32); result
`v·K[dz][dx][dy]`, `K = (f32) exp(−(dx² + (dy + 0.5)² + dz²)/16)` precomputed. Sum in f32.

---

## 10. Order, determinism, parallelism

- All `terrain` randomness is positional or seeded per chunk: the result does not depend on the order
  in which chunks are generated. `terrain` only reads the chunk, the biomes at r ≤ 1 and the structure
  starts at r ≤ 8: parallelizable per chunk on the pool.
- `features` writes at r ≤ 1: two chunks less than 3 apart cannot be decorated at the
  same time (region lock).
- The imposed iteration orders: filling (z, x, y↓) — no effect on the result; surface
  (x, z, y↓) — **with** effect (WG maps modified by the extensions, § 4.2); mask (x, z, y↓)
  — with effect (grass → above).

---

## Verification (rc-worldgen, part 2)

Implementation: `crates/rc-worldgen/src/{palette,proto,aquifer,material,carver,terrain}.rs`.
Java probe kept outside the repository (mode `terrain`: filling, surface and caves called one by one on a
proto-chunk, block tags bound from the data, biomes of the 3 × 3 chunks computed
by the `biomes` stage); test `tests/terrain_ref.rs`: 51 chunks (surface 3 seeds, including 27
chosen for their biome: eroded badlands, frozen oceans, peaks, swamps, sulfur caves…;
large biomes, amplified, Nether, End), blocks, `WORLD_SURFACE_WG`, `OCEAN_FLOOR_WG` and
post-processing marks: **0 differences** at each of the three sub-stages.

Points confirmed by this comparison: 2 (provided the **order** of the
samplings is reproduced in a single context with per-chunk caches: aquifer → density → points →
pre-filling of the veins over the reduced volume → lazy minimum surface → caves, whose
"material above" recompiles the rule over a one-block volume), 3, 4, 5, 6, 8. Pitfalls:
`default_block` is a string in 26.3; outside the server the block tags are not bound
(the probe binds them itself, otherwise bedrock is carved and `OCEAN_FLOOR_WG` stays at the floor).

## Uncertain points

1. **Statuses**: the mission listed `noise`, `surface`, `carvers`; in 26.3 they no longer exist
   (merged into `terrain`). Conversion of saves carrying the old names (data
   fixer) not read; to verify on a world from an earlier version.
2. **Per-block vs per-point sampling**: I assume the values of the functions
   pre-sampled over a volume (final density, veins, surface max of the aquifers with step 4)
   identical to the point values; to confirm via `generation-noise.md` (cell interpolation).
3. **Biome zoom seed**: byte order of the SHA-256 (little-endian assumed, like the
   "hashed seed" of the login packet); to compare with the value received from a server.
4. **Precision**: density functions return f32; thresholds and products (f32 `×10` of the
   aquifer level, island `size`) reproduced as read; roundings untested.
5. **Shortcut of the `biome` conditions** by the biomes of the 3×3 chunks: I believe it equivalent
   (the blur only reads within ±1 quart); if doubt remains, do not apply it.
6. **Asymmetric slope** (`gX ≤ −4` or `gZ ≥ 4`) and the effect of the extensions on the slopes of the
   following columns: read, not verified by world comparison.
7. **Blending with old chunks** (`blend_density`, mask filter, border ticks) and
   regeneration below zero of old worlds: out of scope.
8. **Biome of the caves**: read at the quart y = 0 of the source chunk's corner, without blur; the value cached
   per chunk in the region is the same computation (assumed).
9. Content specific to 26.3 (`sulfur_caves`, `cinnabar`, `sulfur`, `dappled_forest`): rules taken
   from the data, to revalidate if the data changes; everything goes through reading the JSON.
