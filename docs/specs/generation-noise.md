# Terrain generation, part 1: randomness, noises, density, biomes (26.3)

Sources: reading of the official 26.3 server (kept outside the repository) and of the
`data/minecraft/worldgen/**` data from the jar, notes in our own words. Context: `architecture.md` § 14
(the Rust singleplayer interprets the official data read at run time and must reproduce a seed's terrain
block for block). Part 2 (to be written): aquifers, material rules (surface, ore veins), carvers,
structures and "beards", blending with old chunks, decoration.

**Warning, 26.3 reworked this engine**: density functions compute in **f32** (no longer
f64), the JSON noise format changed, the cell size is no longer in the noise settings but in each
`interpolated` function, and the router now has only 8 fields. Knowledge of versions 1.18–1.21 is
wrong in several places; this spec is authoritative.

Unverified points are marked **[unverified]** and collected in § 7.

---

## 0. Numeric conventions (bit-exact reproduction)

- `f32`/`f64`: IEEE single/double floats; `i32`/`i64`: signed integers with modulo overflow
  (`wrapping_*` in Rust). Every operation is evaluated **left to right**, with no reassociation, no
  fused multiply-add (no `mul_add`, no "fast math" mode).
- `f64 → f32` conversion: round to nearest. `f32 → f64`: exact. Integer → float: round
  to nearest. Float → integer ("truncate"): toward zero, NaN → 0, saturating at the bounds.
- `ifloor(x)` on an f64 returns an i32 (truncation of `floor(x)`).
- Integer division written `÷`: **toward zero** (like Java); `floor_div(a,b)` = floor division;
  `floor_mod(a,b)` = floor remainder (sign of the divisor); `a % b` = truncated remainder (sign of the dividend).
- Mixed arithmetic: `i32 × f32` converts the integer to f32 then multiplies in f32; `i32 × f64` in f64.
- `lerp(a, p, q) = p + a × (q − p)` (in the type of the arguments).
- `lerp2(a1, a2, x00, x10, x01, x11) = lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))`.
- `lerp3(a1, a2, a3, x000, x100, x010, x110, x001, x101, x011, x111) = lerp(a3, lerp2(a1, a2, x000,
  x100, x010, x110), lerp2(a1, a2, x001, x101, x011, x111))`.
- `smooth(t) = t × t × t × (t × (t × 6 − 15) + 10)` in f32.
- `clamp(v, mn, mx)` (f32): `v < mn ? mn : min(v, mx)`.
- Numbers in the JSON files: f32-typed fields (density constants, `min`/`max`, thresholds, spline
  points…) are read **directly from the decimal text into f32**; f64 fields (`xz_scale`, `y_scale`,
  `base_amplitude`…) into f64. **[unverified: possible pass through f64 before f32]**.

---

## 1. Random generators

### 1.1 48-bit linear congruential ("legacy")

Identical to the `rc-audio` generator (`crates/rc-audio/src/rng.rs`, `JavaRandom`) for `next(bits)`,
bounded integer, f32 and f64. The following must be added:

| Operation | Definition |
|---|---|
| seed | `e = (g XOR 0x5DEECE66D) & (2^48 − 1)` |
| `next(b)` | `e = (e × 0x5DEECE66D + 0xB) & (2^48 − 1)`; returns `(i32)(e >> (48 − b))` |
| integer | `next(32)` |
| bounded integer `n` | power of 2: `(n × next(31)) >> 31` (in i64); otherwise draw `s = next(31)`, `m = s % n`, redraw while `s − m + (n − 1)` overflows to negative (i32); return `m` |
| long | `((i64)next(32) << 32) + (i64)next(32)` (the second one **sign-extended**) |
| boolean | `next(1) ≠ 0` |
| f32 | `next(24) × 2^−24` |
| f64 | `(((i64)next(26) << 27) + next(27)) × 2^−53` |
| skip `n` | `n` times "integer" |
| fork | new legacy generator with seed `long()` |
| positional fork | legacy positional factory with seed `long()` (§ 1.4) |

Gaussian (Marsaglia polar method, keeping the second value in reserve; reset when
reseeded): draw `x = 2·f64 − 1`, `y = 2·f64 − 1`, `r = x² + y²` until `0 < r < 1`;
`k = sqrt(−2 ln r / r)`; return `x·k`, keep `y·k`. Not used by this part.

### 1.2 Xoroshiro128++

State `(lo, hi)` (u64). If both are zero at creation: `lo = 0x9E3779B97F4A7C15`,
`hi = 0x6A09E667F3BCC909`.

- Step: `r = rotl(lo + hi, 17) + lo`; `h = hi XOR lo`; `lo = rotl(lo, 49) XOR h XOR (h << 21)`;
  `hi = rotl(h, 28)`; returns `r` (`rotl` = rotate left, additions modulo 2^64).
- **64-bit seed → state**: `l = g XOR 0x6A09E667F3BCC909`, `h = l + 0x9E3779B97F4A7C15`, then
  `lo = mix(l)`, `hi = mix(h)` with
  `mix(z): z = (z XOR z>>>30) × 0xBF58476D1CE4E5B9; z = (z XOR z>>>27) × 0x94D049BB133111EB;
  returns z XOR z>>>31` (logical shifts).
- integer = low 32 bits of the step; long = step; boolean = bit 0 of the step.
- f32 = `(step >>> 40) × 2^−24`; f64 = `(step >>> 11) × 2^−53`.
- bounded integer `n` (Lemire's method): `r = (u32)integer`; `m = r × n` (u64); `low = m & 0xFFFFFFFF`;
  if `low < n`: `t = (2^32 − n) mod n`; while `low < t`: new `r`, `m = r × n`, `low = m & 0xFFFFFFFF`;
  return `m >> 32`. `n = 1` still consumes one step.
- skip `n`: `n` steps. Fork: new state `(step, step)` (first one drawn → `lo`), **without mixing**.
  Positional fork: positional factory `(step, step)` (§ 1.4).

### 1.3 Seed drawn from a name

`MD5(name in UTF-8)` gives 16 bytes; `lo` = bytes 0–7 read **big-endian**, `hi` = bytes 8–15.
The name is the exact string: full identifier (`minecraft:offset`) for noises and terrain
randomness, bare string for octaves (`octave_-3`).

### 1.4 Positional factories

Position hash (`hp`): `s = (i64)(x ×₃₂ 3129871) XOR (i64)z × 116129781 XOR (i64)y` (the first
product in **i32** modulo, then extended); `s = s × s × 42317861 + s × 11` (i64 modulo); returns `s >> 16`
(arithmetic shift).

| Factory | from the position (x,y,z) | from a name | from a seed `g` |
|---|---|---|---|
| Xoroshiro `(L, H)` | Xoroshiro with state `(hp XOR L, H)` | `(md5lo XOR L, md5hi XOR H)` | `(g XOR L, g XOR H)` |
| Legacy `G` | legacy with seed `hp XOR G` | legacy with seed `(i64)(string hash of the name) XOR G` | legacy with seed `g` |

String hash (legacy): `h = 0; for each UTF-16 unit c: h = 31·h + c` in i32, then sign-extended
to i64. The Xoroshiro states created here are **not** mixed (only the 64-bit seed is).

### 1.5 Who uses what

The `legacy_random_source` setting of the `noise_settings` chooses the algorithm: `false` for
`overworld`, `amplified`, `large_biomes`; `true` for `nether`, `end`, `caves`, `floating_islands`.

| Use | Generator |
|---|---|
| World root `R` | `(legacy(seed) or Xoroshiro(seed)).positional fork` |
| Named noise `N` (§ 2.5) | `R.from_name("minecraft:N")` |
| Noises `minecraft:nether/temperature` and `minecraft:nether/vegetation` | **always** legacy with seed `seed + 0` / `seed + 1`, special initialization § 2.6 (whatever the setting) |
| `old_blended_noise` | legacy setting: legacy(seed); otherwise `R.from_name("minecraft:terrain")` |
| `end_outer_islands` | legacy(seed), skip 17,292, then simplex (§ 2.3) |
| Named factories of part 2 (e.g. `minecraft:aquifer`) | `R.from_name(name).positional fork` |

---

## 2. Noises

### 2.1 Improved Perlin

**Initialization** from a generator `r`: `ox = r.f64 × 256`, `oy = r.f64 × 256`, `oz = r.f64 × 256`
(in this order), table `P[0..256] = 0..255`, then for `i` from 0 to 255: `j = i + r.bounded(256 − i)`,
swap `P[i]` and `P[j]` (including `i = 255`, which consumes a draw). `p(v) = P[v & 255]`.

**Gradients** (index `h & 15`):
`(1,1,0) (−1,1,0) (1,−1,0) (−1,−1,0) (1,0,1) (−1,0,1) (1,0,−1) (−1,0,−1) (0,1,1) (0,−1,1) (0,1,−1)
(0,−1,−1) (1,1,0) (0,−1,1) (−1,1,0) (0,−1,−1)`. Product `g·(a,b,c) = gx·a + gy·b + gz·c` in f32.

**Wrap** (f64): if `−H ≤ v < H` with `H` = the largest f64 < 2^24, `v` unchanged; otherwise
`v − floor_f64(v / 2^25 + 0.5) × 2^25`.

**Point evaluation** `perlin(x, y, z)` (f64 inputs, f32 output):
1. `X' = wrap(x) + ox`, same for `Y'`, `Z'` (f64); `X = ifloor(X')`…; `fx = (f32)(X' − X)`, same for `fy`, `fz`.
2. `a0 = p(X)`, `a1 = p(X+1)`; `b00 = p(a0+Y)`, `b01 = p(a0+Y+1)`, `b10 = p(a1+Y)`, `b11 = p(a1+Y+1)`.
3. Corners: `d000 = g(p(b00+Z))·(fx, fy, fz)`, `d100 = g(p(b10+Z))·(fx−1, fy, fz)`,
   `d010 = g(p(b01+Z))·(fx, fy−1, fz)`, `d110 = g(p(b11+Z))·(fx−1, fy−1, fz)`, and the same with `Z+1`
   and `fz−1` for `d001 d101 d011 d111`.
4. Returns `lerp3(smooth(fx), smooth(fy), smooth(fz), d000, d100, d010, d110, d001, d101, d011, d111)`.

The 2D noise `perlin(x, z)` equals `perlin(wrap(x), 0, wrap(z))`. Declared range `[−2; 2]` (§ 3.6).

**Volume path** (same result bit for bit for a given coordinate): iterate `z`, then `x`, then
`y`; the coordinates passed are `block × scale` where `scale` is the scale **already multiplied** by
the layer's frequency (§ 2.4); each corner is computed `(gx·fx + gz·fz) + gy·fy` — equal to the
point order because a gradient always has one zero component.

### 2.2 "Smeared" Perlin (legacy terrain)

Parameter `s` (f64, "vertical smear scale"). Evaluation of `(x, y, z)`:
`X' Y' Z'`, `X Y Z`, `fx`, `fz` as in § 2.1, but `fy64 = Y' − Y` (f64);
`lim = (y ≥ 0 and y < fy64) ? y : fy64` (with the **input** `y`, not wrapped);
`shift = ifloor(lim / s + 1.0000000116860974e−7) × s` (the constant is the f32 `1e−7` seen as f64);
`fy' = (f32)(fy64 − shift)`. The corner products use `fy'` (and `fy' − 1`), the vertical smoothed
factor uses `smooth((f32)fy64)`. Volume path identical.

### 2.3 2D simplex (End islands)

Initialization like Perlin but the three offsets are multiplied by 0 (the three f64 are **still
drawn**). `F2 = 0.5 × (√3 − 1)`, `G2 = (3 − √3) / 6` (f64). Evaluation of `(x, y)`:
`x += ox`, `y += oy`; `s = (x + y) × F2`; `i = ifloor(x + s)`, `j = ifloor(y + s)`;
`t = (i + j) × G2`; `x0 = x − (i − t)`, `y0 = y − (j − t)`; `(i1, j1) = x0 > y0 ? (1,0) : (0,1)`;
`x1 = x0 − i1 + G2`, `y1 = y0 − j1 + G2`, `x2 = x0 − 1 + 2·G2`, `y2 = y0 − 1 + 2·G2`;
`ii = i & 255`, `jj = j & 255`; `g0 = p(ii + p(jj)) % 12`, `g1 = p(ii+i1 + p(jj+j1)) % 12`,
`g2 = p(ii+1 + p(jj+1)) % 12`; corner `n(g, a, b) = t<0 ? 0 : t⁴ × (gx·a + gy·b + gz·0)` with
`t = 0.5 − a·a − b·b − 0·0` (f64); returns `(f32)(70 × (n0 + n1 + n2))`. The 3D simplex is not used
by this part.

### 2.4 Layer stack

Ordered list of layers `(noise, frequency f64, amplitude f32)`. Point:
`v = 0; for each layer: v += amplitude × noise(x·f, y·f, z·f)` (f32, in order).
Volume: for each layer, `buffer[i] += (A × amplitude) × noise(...)` with `A` the amplitude requested by
the caller (1 for a simple noise, 4 for `shift_b`) and the coordinates `block × (scale × f)`.

**Point/volume difference**: in point mode a layer's coordinate is `(block × scale) × f`, in volume mode
`block × (scale × f)`. The two coincide when `scale` or `f` is a power of 2, but **not**
for the second half of the layers of a normal noise (`f = 2^k × 1.0181268882175227`) combined with a
scale such as `0.67`, `0.7142857…`, `0.6666…` or `1500`. Both paths must therefore be implemented
as they are and used where the game uses them (§ 3.8).

### 2.5 Normal noise (`worldgen/noise/*.json`)

26.3 format (no vanilla file uses `normalize`):

| Key | Type | Default |
|---|---|---|
| `base_octave` | integer −32…32 | required |
| `octave_count` | integer 1…32 | 1 |
| `base_amplitude` | f64 | 1.0 |
| `normalize` | `true` / `false` / `"legacy"` | `true` |
| `amplitude_modifiers` | list of f64 (size = `octave_count`, or empty = all 1) | empty |

**Octaves**: `freq = 2^base_octave`, `amp = base_amplitude`; if `normalize ≠ false`:
`amp ×= 2^(n−1) / (2^n − 1)` (n = `octave_count`, f64 computation via exact powers). For `i` from 0 to
n−1: `m = modifier[i]`; if `m ≠ 0`, keep the octave `(index = base_octave + i, freq, amp × m)`;
then `freq ×= 2`, `amp ×= 0.5`.

**Normalization factor** `N` (f64):
- `target = Σ |amp_k|` over the kept octaves, **Kahan compensated sum** (running sum `s`,
  compensation `c`: `t = a − c; v = s + t; c = (v − s) − t; s = v`), final result `s − c`
  **[unverified: end-of-sum variant depending on the Java version]**;
- `σ = sqrt(Σ (0.2702247831245211 × |amp_k|)²)` (naive sum in order);
- `σ = 0` ⇒ `N = 0`; otherwise `N = (target × 0.3333333333333333) / (σ × sqrt(2))`.
- `"legacy"` variant (if `N ≠ 0`): `span` = gap between the first and last non-zero modifier
  indices; `N' = base_amplitude × 0.5 × 0.3333333333333333 / (0.1 × (1 + 1 / (span + 1)))`;
  `target ×= N' / N`; `N = N'`.
- Declared range: `±(f32)(target × 0.3333333333333333 × 6)`.

**Instantiation** from the noise's generator `r` (§ 1.5): `A = r.positional`, `B = r.positional`
(in this order); for each kept octave, in order: `k = (f32)(N × amp)`;
layer `(Perlin(A.from_name("octave_" + index)), freq, k)` then
layer `(Perlin(B.from_name("octave_" + index)), freq × 1.0181268882175227, k)`.
The layer order is the summation order.

### 2.6 Nether climate noises (legacy initialization)

For `nether/temperature` (seed + 0) and `nether/vegetation` (seed + 1), with a legacy `r`:
`amps` = modifiers, or n times 1.0; two stacks are built **with the same `r`**, the first then the
second, by the procedure `legacy_stack(r, base_octave, amps)`:
1. `z = −base_octave` (requires `z ≥ n − 1`, otherwise error); create `Perlin(r)` (always, even if unused);
   it becomes level `z` if `z < n` and `amps[z] ≠ 0`.
2. For `i` from `z − 1` down to 0: if `i < n` and `amps[i] ≠ 0`: level `i = Perlin(r)`; otherwise skip 262.
3. `f = 2^−z`, `v = 2^(n−1) / (2^n − 1)`; for `i` from 0 to n−1: if the level is present, layer
   `(level, f, (f32)(v × amps[i]))`; `f ×= 2`, `v /= 2`.

Final noise: `k = (f32)(N × base_amplitude)` (N from § 2.5 computed on the file's parameters);
layers of the first stack `(noise, f × 1, a × k)` then of the second `(noise, f × 1.0181268882175227,
a × k)` (amplitude product in f32).

### 2.7 `old_blended_noise` (density function, legacy terrain noise)

Fields (f64): `xz_scale`, `y_scale`, `xz_factor`, `y_factor` (0.001…1000), `smear_scale_multiplier`
(1…8). `Mxz = 684.412 × xz_scale`, `My = 684.412 × y_scale`, `Sl = My × smear_scale_multiplier`,
`Sm = Sl / y_factor`.

Three stacks drawn **sequentially** from the same generator (§ 1.5): `min = fbm(16, Sl, 0.9999847412109375)`,
`max = fbm(16, Sl, 0.9999847412109375)`, `main = fbm(8, Sm, 12.75)`, where `fbm(n, S, v)`:
`v /= 2^n − 1; f = 1; n times: layer (smeared Perlin(r, S × f), frequency f, amplitude (f32)v);
f /= 2; v ×= 2` (the first layer created is the finest and the weakest).

Value: `α = clamp(main(Mxz / xz_factor, My / y_factor) + 0.5, 0, 1)` then
`α = 0 ⇒ min(Mxz, My)`; `α = 1 ⇒ max(Mxz, My)`; otherwise `lerp(α, min, max)` in f32 — each stack evaluated
as an unshifted density noise with the given scales (§ 3.4 `noise`). Domain: x, y, z.

---

## 3. Density functions

### 3.1 Model

A density function associates an f32 with an integer block position `(x, y, z)`. It has two paths:
- **point**: one position;
- **volume**: a grid `V = (size x, y, z; origin; step x, y, z)` filled into an f32 buffer
  with index `y + (x + z × size_x) × size_y` (y fastest), iterated `z`, then `x`, then `y`.

The two paths give the same result **except** for the differences listed in § 3.8. JSON reading: a number
is a constant; a string is a reference to `worldgen/density_function/<id>.json` (replaced by
its content, a simple macro); an object carries `type` and its fields.

### 3.2 Preparation (compilation)

Before use, the tree is prepared; only the steps marked ⚠ change values:
1. References are expanded.
2. Each `cache` is deduplicated by **structural** equality of its input (two `cache`s with equal
   inputs share one cache slot); the input is prepared recursively on its own.
3. Domain axes (subset of {x, y, z}): a child whose axes are strictly included in
   those of the parent is wrapped in `slice` at coordinate 0 on the missing axes (x, then z, then y;
   not constants nor gradients). No effect on the value as long as the declared axes are right (§ 3.4);
   optional for us, but to keep if the same calls are wanted.
4. ⚠ Constants: `div(f, c)` becomes `f × (1/c)` (f32 inverse then product); `sub(f, c)` becomes
   `f + (−c)` (identical); `pow` with a constant exponent 0.5 / 1 / 2 / 3 becomes root / identity / square /
   cube, under a `reciprocal` if the exponent is negative.
5. ⚠ `min`/`max` whose ranges (§ 3.6) do not overlap are replaced by the winning side **without
   evaluating the other**.

### 3.3 Complete list of 26.3 types

`constant`, `blend_alpha`, `blend_offset`, `beardifier`, `noise`, `end_outer_islands`,
`distance_to_point`, `gradient`, `shift_a`, `shift_b`, `shift`, `abs`, `square`, `cube`, `sqrt`,
`half_negative`, `quarter_negative`, `reciprocal`, `negate`, `squeeze`, `log`, `sign`, `floor`, `round`,
`ceil`, `truncate`, `add`, `sub`, `mul`, `div`, `min`, `max`, `pow`, `spline`, `lerp`, `clamp`,
`range_choice`, `interval_select`, `cache`, `blend_density`, `interpolated`, `slice`,
`find_top_surface`, `old_blended_noise` (44 types, all prefixed `minecraft:`).

**No longer exist**: `flat_cache`, `cache_2d`, `cache_once`, `cache_all_in_cell` (→ a single `cache`),
`y_clamped_gradient` (→ `gradient` y axis, `clamp_to_edge`), `shifted_noise` (→ `noise` with
`shift_x/y/z`), `weird_scaled_sampler` (the caves express it with `interval_select`/`range_choice`),
`end_islands` (→ `end_outer_islands`), `spline` keeps its name but not its internal format.

Types actually used by the vanilla data (number of occurrences): add 80, mul 71, noise 67,
lerp 37, gradient 37, cache 32, range_choice 21, interpolated 21, sub 19, min 18, abs 18, clamp 18,
beardifier 11, squeeze 11, max 11, blend_density 11, blend_alpha 9, spline 9, quarter_negative 6,
interval_select 3, old_blended_noise 3, blend_offset 3, half_negative 3, square 3, find_top_surface 3,
div 3, cube 2, slice 1, negate 1, shift_a 1, distance_to_point 1, shift_b 1, end_outer_islands 1.

### 3.4 Semantics by type

Notation: `e(…)` = value of the input at the same point. "Axes" = declared domain axes.

| Type (fields) | Value | Axes |
|---|---|---|
| `constant` (`value` f32), or bare number | `value` | ∅ |
| `blend_alpha` / `blend_offset` | supplied by the blending with old chunks (part 2); without blending: 1 / 0 | x, z |
| `beardifier` | terrain adaptation to structures (part 2); without structures: 0 | x, y, z |
| `noise` (`noise`, `xz_scale` f64, `y_scale` f64, optional `shift_x/y/z`, default 0) | without shift: `noise(x·xz, y·ys, z·xz)`; with: `noise(x·xz + sx, y·ys + sy, z·xz + sz)` (f64 sums, f32 shifts evaluated at the same point) | all, minus y if `y_scale = 0`, minus x and z if `xz_scale = 0`, plus the axes of the shifts |
| `shift_a` (`noise`) | `noise(x/4, 0, z/4) × 4` (i.e. `x × 0.25`) | x, z |
| `shift_b` (`noise`) | `noise(z × 0.25, x × 0.25, 0) × 4` (swapped axes) | x, z |
| `shift` (`noise`) | `noise(x × 0.25, y × 0.25, z × 0.25) × 4` | x, y, z |
| `gradient` (`axis`, `tiling` default `clamp_to_edge`, `from_coordinate`, `to_coordinate` distinct integers, `from_value`, `to_value` f32) | `L = to_c − from_c`; `k = (to_v − from_v) / (f32)L`; `c` = axis coordinate; `clamp_to_edge`: `from_v + (f32)(clamp_int(c, min, max) − from_c) × k`; `repeat`: `from_v + (f32)floor_mod(c − from_c, L) × k`; `mirrored_repeat`: `t = floor_div(c − from_c, L)`, `l = (c − from_c) − t·L`, `t` even ⇒ `from_v + l·k`, odd ⇒ `from_v + (L − l)·k` | the axis |
| `distance_to_point` (`point` [x,y,z] integers, `metric`) | `d = (f32)(point − block)` per axis; `euclidean`: `(f32)sqrt(dx·dx + dy·dy + dz·dz)`; `euclidean_squared`: the sum; `manhattan`: `|dx| + |dy| + |dz|`; `chebyshev`: `max(max(|dx|, |dy|), |dz|)` (f32) | x, y, z |
| `end_outer_islands` | § 3.5 | x, z |
| `abs`, `negate` | `|e|`, `−e` | of the input |
| `square`, `cube` | `e·e`, `e·e·e` | same |
| `sqrt`, `log` | `(f32)sqrt(e)`, `(f32)ln(e)` | same |
| `half_negative`, `quarter_negative` | `e > 0 ? e : e × 0.5` / `e × 0.25` | same |
| `reciprocal` | `1 / e` | same |
| `squeeze` | `c = clamp(e, −1, 1)`; `c / 2 − c·c·c / 24` | same |
| `sign` | f32 sign (±1, ±0, NaN preserved) | same |
| `floor` / `round` / `ceil` / `truncate` (`input`, `multiple` default 1) | if `multiple` is the constant 1: `rnd(e)`; otherwise `m = multiple(…)`, `m = 0 ⇒ e`, otherwise `rnd(e / m) × m`. `rnd`: floor, `round` = round to nearest, half toward +∞, computed exactly (not `floor(e + 0.5)` in f32), result saturated i32 (NaN → 0) then f32, ceiling, truncation toward zero | union |
| `add` / `sub` / `mul` / `div` (`left`, `right`) | f32; see § 3.2 for constants; point: `mul` and `div` return **0 without evaluating the right side** if the left is 0 | union |
| `min` / `max` | point: if the left ≤ the right's declared min (resp. ≥ max) it is returned without evaluating the right; otherwise f32 `min`/`max`; volume: the right replaces the left if strictly smaller (resp. larger) | union |
| `pow` (`base`, `exponent`) | `(f32)(f64(base) ^ f64(exponent))`; constant cases § 3.2 | union |
| `lerp` (`alpha`, `first`, `second`) | `α = 0 ⇒ first` (only one evaluated), `α = 1 ⇒ second`, otherwise `lerp(α, first, second)` | union |
| `clamp` (`input`, `min`, `max` f32, min ≤ max) | `clamp(e, min, max)` | input |
| `range_choice` (`input`, `min_inclusive`, `max_exclusive`, `when_in_range`, `when_out_of_range`) | `min_inclusive ≤ e < max_exclusive` ⇒ first branch, otherwise second (NaN ⇒ second) | union |
| `interval_select` (`input`, increasing `thresholds`, `functions` = thresholds + 1) | first `i` such that `e < threshold[i]`, otherwise the last function; only the chosen one is evaluated in point mode | union |
| `spline` (`spline`) | § 3.5 | union of the coordinates |
| `cache` (`input`) | `e` (pure cache, § 3.7) | input |
| `blend_density` (`input`) | `e` transformed by the blending (part 2); without blending: `e` | input |
| `interpolated` (`input`, `cell_size_xz` > 0, `cell_size_y` > 0) | § 3.8 | input |
| `slice` (`axis`, `coordinate`, `input`) | `e` with the axis coordinate replaced by `coordinate` | input minus the axis |
| `find_top_surface` (`density`, `upper_bound` function, `lower_bound` integer, `cell_height` > 0) | evaluated at `y = 0`; `h = floor(upper_bound / cell_height) × cell_height` (f32 division); if `h ≤ lower_bound` ⇒ `lower_bound`; otherwise for `p = h, h − cell_height, …` while `p ≥ lower_bound`: if `density(x, p, z) > 0` (**point**) ⇒ `p`; otherwise `lower_bound` | (axes of both) minus y |
| `old_blended_noise` | § 2.7 | x, y, z |

The outputs of `find_top_surface` and `round` are integers converted to f32.

### 3.5 Spline and End islands

**Spline**: JSON `{"coordinate": function, "points": [{"location": f32, "value": f32 or spline,
"derivative": f32}, …]}` (at least one point, increasing locations). Evaluation, `c` = coordinate at the
current point (each distinct coordinate function is evaluated only once per point):
`s = (first i such that c < loc[i]) − 1` (binary search, `n` if none);
- `s < 0`: `ext(0)`; `s = n − 1`: `ext(n−1)`, with `ext(k) = d[k] = 0 ? v[k] : v[k] + d[k] × (c − loc[k])`;
- otherwise `x1 = loc[s]`, `x2 = loc[s+1]`, `t = (c − x1) / (x2 − x1)`, `y1 = v[s]`, `y2 = v[s+1]` (sub-splines
  evaluated recursively), `a = d[s] × (x2 − x1) − (y2 − y1)`, `b = −d[s+1] × (x2 − x1) + (y2 − y1)`,
  value `lerp(t, y1, y2) + t × (1 − t) × lerp(t, a, b)` — all in f32, in this order.

**`end_outer_islands`** (simplex of § 2.3, integer inputs converted to f64):
`sx = x ÷ 8`, `sz = z ÷ 8` (division toward zero); `cx = sx ÷ 2`, `cz = sz ÷ 2`, `rx = sx % 2`, `rz = sz % 2`;
`h = −100`; for `ox` from −12 to 12, then `oz` from −12 to 12: `X = cx + ox`, `Z = cz + oz` (i64);
if `X² + Z² > 4096` and `simplex(X, Z) < −0.9` (f32):
`size = (|(f32)X| × 3439 + |(f32)Z| × 147) % 13 + 9` (f32 remainder); `dx = (f32)(rx − 2·ox)`,
`dz = (f32)(rz − 2·oz)`; `n = clamp(100 − (f32)sqrt(dx·dx + dz·dz) × size, −100, 80)`; `h = max(h, n)`.
Value: `(h − 8) / 128`. Declared range `[−0.84375; 0.5625]`.

### 3.6 Declared ranges (needed by the ⚠ rules of § 3.2)

Ranges are **not** guaranteed bounds (that of a normal noise ignores the factor `N`), so the
`min`/`max` shortcuts can change an extreme value: they must be reproduced. f32 intervals
`[a; b]`, plus an "undefined" value that propagates.

| Type | Range |
|---|---|
| constant | `[v; v]` |
| `noise` | range of the normal noise (§ 2.5); `shift*`: × 4 |
| `gradient` | `[min(fv, tv); max(fv, tv)]` |
| `distance_to_point` | `[0; +∞]`; `end_outer_islands`: `[−0.84375; 0.5625]` |
| `blend_alpha` | `[0; 1]`; `blend_offset`, `beardifier`: `[−∞; +∞]` |
| `old_blended_noise` | sum over the 16 `min` layers of `[±(|S·f| + 2)] × (f32)v` |
| `add`, `sub` | `[a1+a2; b1+b2]`, `[a1−b2; b1−a2]` |
| `mul` | min and max of the 4 products of bounds, a product being 0 as soon as one bound is zero |
| `div` | `mul` by the inverse: `[1/b; 1/a]` if 0 ∉; `b = 0` ⇒ `[−∞; 1/a]`; `a = 0` ⇒ `[1/b; +∞]`; otherwise `[−∞; +∞]`; `[0;0]` ⇒ undefined |
| `min`, `max` | bound to bound |
| `clamp` | `a ≥ max` ⇒ `[max;max]`; `b ≤ min` ⇒ `[min;min]`; otherwise intersection |
| `abs`, `square` | `[0; max]` if 0 ∈, otherwise `[min; max]` of the images |
| `cube`, `half/quarter_negative`, `squeeze`, `sqrt`, `log` (input clipped to ≥ 0), roundings | images of the bounds (monotonic functions) |
| `reciprocal`, `negate`, `sign` | like `div(1, ·)`, `[−b; −a]`, signs of the bounds |
| `lerp` | envelope of the 4 combinations `first`/`second` × `alpha.min`/`alpha.max`, `p + α(q − p)` |
| `range_choice`, `interval_select` | envelope of the branches |
| `spline` | envelope of the point values, of the linear extensions if the coordinate's range overflows, and for each interval with non-zero derivatives `min/max(y) + 0.25 × [p1 − max2 + min1; …]` **[detail to be transcribed during implementation]** |
| `cache`, `slice`, `interpolated`, `blend_density` | that of the input |
| `find_top_surface` | `[lower; max(lower, max(upper_bound))]` |
| `pow` | case-by-case rules (constant base, integer exponent…) — not used by vanilla |

### 3.7 Caches

A `cache` remembers, per evaluation context, the last volume computed (reused if
requested again identically or if the requested point is part of it) and the last point value.
**Not quite neutral**: a point request that falls in the last cached volume returns the value
**computed in volume mode**, which can differ from the point value (§ 2.4, § 3.8). `rc-worldgen` therefore reproduces
the cells as they are (one per deduplicated cache and per context; context without caches for
isolated evaluations, with caches for filling a chunk and for a chunk's climate).

### 3.8 Cells and interpolation (`interpolated`)

Sizes `Cx = cell_size_xz`, `Cy = cell_size_y` (overworld: `final_density` 4 × 8, `chunk_surface_level`
16 × 1; Nether 4 × 8; End 8 × 4). f32 inverses `Ix = 1/Cx`, `Iy = 1/Cy`.

**Volume** with step 1 (the chunk case):
1. If the volume already has steps `Cx, Cy, Cx` and an aligned origin: go straight to the input.
2. Cells covered: `cxmin = floor_div(xmin, Cx)`, `cxmax = floor_div(xmax, Cx)`, `nX = cxmax − cxmin + 1` (same
   for y, z). Corner grid: sizes `nX` if `floor_mod(xmax, Cx) = 0` otherwise `nX + 1` (same), origin
   `(cxmin·Cx, cymin·Cy, czmin·Cx)`, step `(Cx, Cy, Cx)`. The input is evaluated **in volume mode** on this grid.
3. For each cell (order `z`, `x`, then `y`): corners `v000…v111` (next index clamped to the grid size
   − 1). For each block of the cell included in the volume, order `z`, `x`, `y`:
   - `αz = kz × Ix`; `a = lerp(αz, v000, v001)`, `b = lerp(αz, v010, v011)`, `c = lerp(αz, v100, v101)`,
     `d = lerp(αz, v110, v111)`;
   - `αx = kx × Ix`; `low = lerp(αx, a, c)`, `high = lerp(αx, b, d)`;
   - `step = (high − low) × Iy`; `v = low + step × y0` (y0 = first `ky` covered, 0 for a chunk);
     then for each increasing `ky`: write `v`, then `v += step` (f32 **accumulation**).
   (`kx, ky, kz` = position in the cell; `v010` = y+1 corner, `v100` = x+1 corner, `v001` = z+1 corner.)
4. Volume with step ≠ 1: computed at step 1 over the covered box then subsampled.

**Point**: if `(kx, ky, kz) = (0, 0, 0)` ⇒ point value of the input; otherwise 2 × 2 × 2 grid of the
cell's corners evaluated in volume mode, then
`lerp3((f32)kx / Cx, (f32)ky / Cy, (f32)kz / Cx, v000, v100, v010, v110, v001, v101, v011, v111)`.

**Differences between paths** (interpolation order x→y→z versus incremental z→x→y, division versus
multiplication by the inverse): the point result is **not** bit for bit equal to the volume. Rule: use the
path that the official caller uses:
- terrain filling (`final_density`): **volume** 16 × height × 16, origin `(chunk x_min,
  min_y, chunk z_min)`; isolated column (base height, noise column): volume 1 × height × 1,
  same result as the corresponding column of the chunk;
- biomes: volume (§ 5.3), but the vanilla climate functions contain neither `interpolated` nor an unshifted
  noise with an irrational scale: point and volume coincide there;
- `find_top_surface` probes its density in point mode (no `interpolated` in vanilla);
- part 2 will specify the calls of the aquifers and the material rules.

### 3.9 Filling a chunk

`final_density` is evaluated over the chunk's volume; then, for `z`, `x`, and `y` **descending**: the
density `d` (f32) is passed to the aquifer (part 2); aquifer absent: `d > 0` ⇒ default block
(`default_block`), otherwise global fluid (below sea level: `default_fluid`, part 2) or air.

---

## 4. Noise settings and router (`worldgen/noise_settings/*.json`)

| Key | Role |
|---|---|
| `noise` | `{min_y, height}` (multiples of 16) — no more cell size here |
| `default_block`, `default_fluid` | default solid block and fluid |
| `noise_router` | 8 functions, below |
| `aquifers` (optional; absent = aquifers disabled) | `barrier`, `fluid_level_floodedness`, `fluid_level_spread`, `lava` (noises), `exclusion` (aquifer-free zone: overworld `min(−0.225 − erosion, max(depth − 0.9, 0))`), `surface_level` (overworld: `overworld/preliminary_surface_level`) — part 2 |
| `material_rule` | surface rules; the **ore veins** now live there (`ore_vein` rule: `density`, `richness`, `filler_gap`, blocks, `raw_ore_chance`) — part 2 |
| `sea_level`, `legacy_random_source`, `spawn_target`, `disable_mob_generation`, `debug_functions` | sea level, § 1.5, spawn point search, —, F3 display |

26.3 router:

| Field | Role |
|---|---|
| `temperature`, `vegetation` | climate: temperature, humidity |
| `continents`, `erosion`, `depth`, `ridges` | climate: continentalness, erosion, depth, weirdness (§ 5); in the End, `erosion` = `end/islands` is used to choose biomes |
| `chunk_surface_level` | interpolated preliminary surface (overworld: `interpolated` 16 × 1 of `preliminary_surface_level`), supplied to the material rules |
| `final_density` | terrain density (> 0 = solid) |

Old fields that disappeared: `barrier`, `fluid_level_floodedness`, `fluid_level_spread`, `lava` → block
`aquifers`; `vein_toggle`, `vein_ridged`, `vein_gap` → `ore_vein` rule (functions
`overworld/ore_vein/*`); `initial_density_without_jaggedness` → `preliminary_surface_level`
(`find_top_surface` on a density without jaggedness, step 8, bounded by an estimate), from which
`chunk_surface_level` derives.

Vanilla values: overworld/amplified/large_biomes `min_y −64`, height 384, sea 63, Xoroshiro,
aquifers; Nether 0/128, sea 32, legacy; End 0/128, sea 0, legacy; caves −64/192, sea 32, legacy;
floating_islands 0/256, sea −64, legacy.

---

## 5. Biomes

### 5.1 Climate

Six f32 values evaluated at the block `(4·qx, 4·qy, 4·qz)` of a "quart": `temperature`, `vegetation`
(humidity), `continents`, `erosion`, `depth`, `ridges` (weirdness). Quantization of each one:
`(i64)(v × 10000)` — **f32** product then truncation toward zero. Seventh "offset" dimension of the
target: 0.

### 5.2 Parameter list

Entry = biome + 7 quantized intervals `[min; max]` (temperature, humidity, continentalness,
erosion, depth, weirdness, offset; the offset is a point). JSON: each parameter is a
number or `[min, max]` (f32 in `[−2; 2]`, `offset` in `[0; 1]`), quantized as in § 5.1.

- `multi_noise_biome_source_parameter_list/*.json` only contains `{"preset": "minecraft:overworld"}` or
  `"minecraft:nether"`: the lists are **in the code**. Usable source: the official report
  `reports/biome_parameters/minecraft/{overworld,nether}.json` (server data generator; already
  present in `rustcraft-re/26.3/reports/`), 7,594 overworld entries **in order**, to be kept
  (the order affects the tree).
- Nether (5 entries, all other values 0): `nether_wastes`; `soul_sand_valley` humidity −0.5;
  `crimson_forest` temperature 0.4; `warped_forest` humidity 0.5, offset 0.375; `basalt_deltas`
  temperature −0.5, offset 0.175.

Distance from an interval to `t`: `t > max ⇒ t − max`; otherwise `max(min − t, 0)`. Distance of a node:
sum of squares over the 7 dimensions (i64).

### 5.3 Search tree and tie rule

**Construction** (19 children per node) from the leaves in list order:
- 1 child ⇒ that child; ≤ 19 ⇒ node whose children are sorted (**stable** sort) by
  `Σ_d |(min_d + max_d) ÷ 2|`;
- otherwise, for `d` = 0..6: sort the list (in place, stable) by centers `(min + max) ÷ 2` in
  dimension `d`, then `d+1`, …, `d+6` (modulo 7); split into consecutive buckets of
  `k = (i32)19^floor(ln(size − 0.01) / ln 19)`; cost = sum over the buckets of the sum over dimensions of `|max − min|` of their
  bounding box; keep the first `d` with strictly minimal cost;
- sort the retained buckets by the same keys in **absolute value** of the centers, starting from the
  retained dimension; each bucket becomes a recursively built node. A node's box: union of the
  boxes of its children.

**Search** with an initial candidate (the last leaf found **by this thread of execution**, or
none): `best = distance(candidate)` (or +∞); for each child in order: `dc = distance(child)`;
if `best > dc`: `f = search(child, current candidate)`, `df = (f = child) ? dc : distance(f)`;
if `best > df`: keep `f`. A leaf returns itself.

Strict comparisons: on a tie it is the initial candidate or the first one found that wins. The result
therefore depends on the previous search of the same thread (case of a target in two boxes at equal distance);
to match, keep a "last leaf" per thread and the same call order (§ 7).

**Call order for a chunk**: climate evaluated in volume `4 × (height/4) × 4` with step 4 at the chunk's
origin (`min_y` of the dimension); then sections from bottom to top, and in each section `x`, `y`, `z` from 0
to 3. The result is stored per quart in the section (network format already known). Search outside a chunk
(commands, structures): point path, context with cache.

### 5.4 End

Dedicated biome source: quart `(qx, qy, qz)`, block `(4qx, 4qy, 4qz)`, chunk `(cx, cz) = (block >> 4)`;
`cx² + cz² ≤ 4096` (i64) ⇒ `the_end`; otherwise `h = erosion(16·cx + 8, 4qy, 16·cz + 8)` (point):
`h > 0.25` ⇒ `end_highlands`; `h ≥ −0.0625` ⇒ `end_midlands`; `h < −0.21875` ⇒ `small_end_islands`;
otherwise `end_barrens`.

### 5.5 Biome at block resolution ("zoom", client and server)

Zoom seed: first 8 bytes, read little-endian, of the SHA-256 of the 8 little-endian bytes of the
world seed (this is the hashed seed received by the client). For a block `(x, y, z)`:
`a = (x−2, y−2, z−2)`, `p = a >> 2`, `fr = (a & 3) / 4.0`; for `i` = 0..7 (bit 4 → x, 2 → y, 1 → z;
bit at 0 = corner `p`, at 1 = `p + 1` and `fr − 1`): `r = seed`; `r = nxt(r, cx)`, `nxt(r, cy)`,
`nxt(r, cz)`, then again `cx`, `cy`, `cz`; `ex = noise(r)`; `r = nxt(r, seed)`; `ey = noise(r)`;
`r = nxt(r, seed)`; `ez = noise(r)`; distance `(dz + ez)² + (dy + ey)² + (dx + ex)²`; the first
`i` with strictly minimal distance designates the quart whose biome is taken. `nxt(r, c) =
r × (r × 6364136223846793005 + 1442695040888963407) + c` (i64 modulo); `noise(r) =
(floor_mod(r >> 24, 1024) / 1024 − 0.5) × 0.9`.

---

## 6. Verification

### 6.1 Reference bench (bit for bit, top priority)

A Java probe kept outside the repository, launched like the extractor (`tools/extractor/README.md`: classpath of the
server jar, call of the public API like a mod, nothing copied), writes for a seed and a preset:
1. the first 16 outputs of the root, of `from_name` for a few names, of `from_position`;
2. for each named noise: offsets and `P[0]`, `P[255]` of each Perlin, factor `N`, amplitudes;
3. the values (f32 bits) of each noise and of each named function of `density_function/` over a
   grid of points, **in point mode and in volume mode**;
4. the `final_density` buffer of a few chunks (including negative coordinates and distant chunks,
   |x| > 2^21, for the wrap);
5. the 6 quantized climate values and the biome of each quart of these chunks; the quantized
   parameter list (integers, to avoid any decimal round trip).

The Rust tests compare bit for bit; at the first divergence we walk up the tree comparing the
named sub-functions. Seeds: 0, a negative one, a large one; presets normal, `large_biomes`,
`amplified`; Nether and End.

**Done (X19)**: probe and references in `%LOCALAPPDATA%\rustcraft-re\26.3\worldgen-ref` (`Probe.java`,
`run-probe.ps1`; modes `synthetic <jsonl>`, `zoom`, `column`), read by the tests in
`crates/rc-worldgen/tests/*_ref.rs` (folder overridden by `RUSTCRAFT_WORLDGEN_REF`, tests skipped if
missing). 0 differences on: randomness, all normal noises (point and volume), all named functions
(point, block volume, cell volume, ranges, axes), 59 synthetic functions covering the
rare types and the caches, the final density of 23 chunks (5 presets, x up to 3.2 M) and of
isolated columns, the climate and biome of 48,640 quarts, 4,410 zoomed blocks.

### 6.2 End to end against the official server

**Separate** instance (never the shared test server on port 25570): new folder,
`level-seed=<seed>`, `generate-structures=false`, and a data pack that removes carvers and decoration from
each biome (`"carvers": {}`, `"features": []`, generated from the jar's JSON) — the world then contains
only the noise fill, the surface and the aquifers. `forceload add` of a square, `save-all
flush`, stop, then reading the Anvil regions:
- **biomes**: per-section palettes compared at 100% (no later step modifies them, apart from
  blending of old chunks);
- **terrain**: as long as part 2 does not exist, compare the "solid / not solid" mask below the
  surface, or also neutralize `material_rule` and `aquifers` in the pack for an exact block-by-block
  comparison (stone / water / lava / air).

`/locate biome` (spiral search order, default resolution 32 × 64) gives a quick manual
check; `/fillbiome` writes instead of reading, useless here.

---

## 7. Uncertain points

1. **Compensated sum** of the normalization factor (§ 2.5): the game runs on Java 25, whose end of
   sum is `s − c`; implemented that way, all vanilla noises match (no vanilla case
   separates the two variants).
2. **Reading JSON floats**: text → f32 directly, **confirmed** (all vanilla constants
   match; `rc-worldgen` keeps the number text so as not to go through f64).
3. **Ties in the biome tree**: the official game keeps the last leaf per thread. **Measured**:
   336,654 pairs of overworld entries overlap (272,416 of different biomes) and
   random targets fall in a tie 1.3% of the time, but **0 out of 10,752** real quart climates. The
   fill reproduces the game's order (sections from bottom to top, x, y, z) with a "last leaf"
   held by the caller; a difference is only possible on a real tie handled in another order.
4. **Point/volume paths** (§ 2.4, § 3.8): the list of part 2 callers (aquifers,
   surface) will have to say, call by call, which path is used.
5. **Declared ranges** (§ 3.6): the `spline` row is transcribed in `rc-worldgen` (`spline_range`)
   and verified (ranges of all named and synthetic functions identical to the game).
6. `ln`, `pow` and `sqrt`: `sqrt` is exact everywhere; `ln` and `pow` (non-strict Java) are not
   used by the vanilla data of this part, except exact `pow(2, k)`. The synthetic functions
   `log` and `pow` match on the points tested, with no general guarantee.
7. `biome_parameters` report: the bounds are written `(f32)l / 10000`; `l` is recovered exactly by
   searching for the integer whose dequantization gives back the float read. **Verified** against the integers
   exported by the probe (7,594 + 5 entries, order included).
8. Blending with old chunks (`blend_*`) and `beardifier`: assumed neutral for a new world without
   structures; described in part 2.
