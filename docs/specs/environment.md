# Visual environment: attributes, timelines, fog, plain sky (26.3)

Clean-room spec, written from a reading of the decompiled 26.3 client (kept outside the repository). It describes the rules, not the code. Scope: terrain fog, background colour (frame clear), the plain sky dome, and the attribute system that feeds them. Clouds, celestial bodies and the lightmap are only flagged (§ 9).

Conventions:
- RGB colour = triple of floats in [0, 1], **plain sRGB** (no linearisation anywhere in this chain). `#RRGGBB` → (RR/255, GG/255, BB/255).
- ARGB colour = quadruple (r, g, b, a), `#AARRGGBB` → a = AA/255.
- `lerp(t, a, b) = a + t·(b − a)`, component by component for colours.
- Usual `clamp(x, lo, hi)`. "tick" = 1/20 s. `pt` = render partial tick (0..1).

---

## 1. Environment attributes

### 1.1 Visual attributes relevant here

All are "syncable" (sent to the client in the registries). Column "interp." = spatially smoothed between biomes (§ 4).

| Identifier (`minecraft:`…) | Type | Default | Bounds | Interp. |
|---|---|---|---|---|
| `visual/fog_color` | RGB | `#000000` | — | yes |
| `visual/fog_start_distance` | float | 0 | — | yes |
| `visual/fog_end_distance` | float | 1024 | ≥ 0 | yes |
| `visual/sky_fog_end_distance` | float | 512 | ≥ 0 | yes |
| `visual/cloud_fog_end_distance` | float | 2048 | ≥ 0 | yes |
| `visual/water_fog_color` | RGB | `#050533` | — | yes |
| `visual/water_fog_start_distance` | float | −8 | — | yes |
| `visual/water_fog_end_distance` | float | 96 | ≥ 0 | yes |
| `visual/sky_color` | RGB | `#000000` | — | yes |
| `visual/sunrise_sunset_color` | ARGB | `#00000000` | — | yes |
| `visual/sun_angle` | angle (degrees) | 0 | — | yes |
| `visual/moon_angle`, `visual/star_angle` | angle (degrees) | 0 | — | yes |
| `visual/star_brightness` | float | 0 | [0, 1] | yes |
| `visual/moon_phase` | enum | `full_moon` | — | no |
| `visual/cloud_color` | ARGB | `#00000000` | — | yes |
| `visual/cloud_height` | float | 192.33 | — | yes |
| `visual/block_light_tint` | RGB | `#FFD88C` | — | yes |
| `visual/sky_light_color` | RGB | `#FFFFFF` | — | yes |
| `visual/sky_light_factor` | float | 1 | [0, 1] | yes |
| `visual/night_vision_color` | RGB | `#999999` | — | yes |
| `visual/ambient_light_color` | RGB | `#000000` | — | yes |
| `gameplay/sky_light_level` | float | 15 | [0, 15] | non-positional |

Bounds: at the end of the full evaluation (all layers), an out-of-bounds value is **brought back** into the interval (clamp), not rejected. (When loading JSON, an out-of-bounds value is an error.)

### 1.2 JSON format of an attribute entry

In the `attributes` of a biome or a dimension type, each key is an attribute identifier and the value is either:
- the raw value → implicit `override` modifier;
- or an object `{ "modifier": "<op>", "argument": <arg> }`.

Accepted colour values: string `#RRGGBB` (RGB) or `#AARRGGBB` (ARGB), ARGB integer, or float list `[r,g,b]` / `[r,g,b,a]`.

### 1.3 Modifiers and formulas

`s` = value received from the previous layer, `a` = argument.

| Op | Types | Argument | Result |
|---|---|---|---|
| `override` | all | value | `a` |
| `add` | float | float | `s + a` |
| `subtract` | float | float | `s − a` |
| `multiply` | float | float | `s · a` |
| `minimum` / `maximum` | float | float | `min(s,a)` / `max(s,a)` |
| `alpha_blend` | float | `v` or `{value, alpha=1}` (alpha ∈ [0,1]) | `lerp(alpha, s, value)` |
| `add` | RGB | RGB | `min(s + a, 1)` per channel |
| `add` | ARGB | **RGB** | rgb: `min(s + a, 1)`; alpha of s kept |
| `subtract` | RGB / ARGB | **RGB** | `max(s − a, 0)` per channel; alpha of s kept |
| `multiply` | RGB | RGB | per-channel product (if one of them is exactly (1,1,1), returns the other unchanged) |
| `multiply` | ARGB | **ARGB** | product of the 4 channels (same shortcut for (1,1,1,1)) |
| `alpha_blend` | RGB | **ARGB** | if `a.a = 0` → s; otherwise `lerp(a.a, s, a.rgb)` |
| `alpha_blend` | ARGB | ARGB | if `a.a = 1` → a; if 0 → s; otherwise `A = a.a + s.a·(1 − a.a)`, channel = `(a.c·a.a + s.c·(A − a.a)) / A`, alpha = A |
| `blend_to_gray` | RGB / ARGB | `{brightness, factor}` ∈ [0,1] | `g = 0.3·r + 0.59·g + 0.11·b`; `gray = clamp(g·brightness, 0, 1)` on r,g,b; result `lerp(factor, s, gray)` (alpha kept) |
| `and/or/nand/nor/xor/xnor` | boolean | boolean | usual logic |

Integer: `add, subtract, multiply, minimum, maximum`. List: `append`. Not used here.

### 1.4 Interpolation functions per type

Each type has four uses: *keyframe* (timeline), *state change* (weather), *spatial* (biomes), *partial tick* (render smoothing).

| Type | keyframe | state | spatial | partial tick |
|---|---|---|---|---|
| float | lerp | lerp | lerp | lerp |
| angle (degrees) | plain lerp (not wrapped) | lerp | lerp | "short angle": `d = wrap180(b − a)`; if `|d| ≥ 90` → b, otherwise `a + t·d` |
| RGB / ARGB | per-channel lerp (raw sRGB) | same | same | same |
| not interpolated (boolean, moon phase…) | `t ≥ 1 ? b : a` | `t ≥ 0 ? b : a` | `t ≥ 0.5 ? b : a` | `t ≥ 0 ? b : a` (always the new one) |

`wrap180(x)`: remainder of x by 360 (sign of the dividend), then −360 if ≥ 180, +360 if < −180.

The argument of a timeline track is interpolated with the keyframe function specific to the modifier: `override` → the one of the type; float `multiply/add/...` → lerp; colours → per-channel lerp; float `alpha_blend` → separate lerp of value and alpha; `blend_to_gray` → lerp of brightness and factor; booleans → hold the start value (no jump before the next key).

### 1.5 Layer stacking (exact order)

For a given attribute, the client starts from the default value and applies, in this order:

1. **Dimension**: `attributes` of the dimension type (constant layer).
2. **Biome**: positional layer (at the requested point; Gaussian blend for the camera, § 4). A biome that does not define the attribute lets the received value through.
3. **Timelines** of the dimension, in the order of the `timelines` list (tag resolved; for the Overworld: `day`, `moon`, `early_game`, after `#universal`, which has no visual track).
4. **Weather** (only if the dimension can have weather: `has_skylight` true, `has_ceiling` false, and not the End):
   - `thunder = thunder_level`; `rain = rain_level − thunder` (see § 2.5 for these levels);
   - if the attribute has a rain entry and `rain > 0`: `v = interpState(rain, v, rainMod(v))`;
   - then if it has a thunder entry and `thunder > 0`: `v = interpState(thunder, v, thunderMod(v))`.
5. **Client layers**: lightning (sky) — if the lightning counter is > 0 (and the "hide lightning flashes" option is off): `sky_color = lerp(0.22, sky_color, (0.8, 0.8, 1.0))` and `sky_light_factor = 1`.
6. Clamp to the bounds (§ 1.1).

So timelines apply **after** the biome: the `multiply` track of `day` multiplies the sky colour already chosen by the biome.

Built-in weather modifiers (hard-coded, not in the data):

| Attribute | Rain | Thunder |
|---|---|---|
| `sky_color` | `blend_to_gray` brightness 0.6, factor 0.75 | `blend_to_gray` 0.24, 0.94 |
| `fog_color` | `multiply` (127, 127, 153)/255 | `multiply` (63, 63, 76)/255 |
| `cloud_color` | `blend_to_gray` 0.24, 0.5 | `blend_to_gray` 0.095, 0.94 |
| `sunrise_sunset_color` | `multiply` ARGB (1, 127/255, 127/255, 153/255) | `multiply` (1, 63/255, 63/255, 76/255) |
| `sky_light_color` | `alpha_blend` towards `#7A7AFF` with alpha 79/255 | same, alpha 134/255 |
| `sky_light_factor` | `alpha_blend` value 0.24, alpha 0.3125 | value 0.24, alpha 0.52734375 |
| `sky_light_level` | `alpha_blend` value 4, alpha 0.3125 | value 4, alpha 0.52734375 |
| `star_brightness` | `override` 0 | `override` 0 |

(The colours above come from floats converted to bytes by truncation: 0.5·255 → 127, 0.6·255 → 153, 0.25·255 → 63, 0.3·255 → 76, 0.3125·255 → 79, 0.52734375·255 → 134.)

### 1.6 Cache

Without a position (or for an attribute with no positional layer): value computed once per tick. The tick cache (and the one of the timeline tracks) is invalidated at the end of each client world tick and on every reception of the time packet.

---

## 2. Timelines and clocks

### 2.1 Format `data/minecraft/timeline/<name>.json`

```
{
  "clock": "minecraft:overworld",        // clock (world_clock registry)
  "period_ticks": 24000,                  // optional, integer > 0: loop
  "tracks": { "<attribute>": <track>, ... },
  "time_markers": { ... }                 // command markers, no visual effect
}
```

Track:
```
{ "modifier": "<op>" (default override),
  "ease": <easing> (default "linear"),
  "keyframes": [ { "ticks": <int ≥ 0>, "value": <argument> }, ... ] }
```
Validation rules: at least one key; keys sorted by increasing `ticks`; at most 2 keys on the same tick; with a period, each `ticks` ∈ [0, period].

`ease`: string among `constant` (always returns 0 → start value held until the next key), `linear`, `in_/out_/in_out_` × `back, bounce, circ, cubic, elastic, expo, quad, quart, quint, sine`; or object `{ "cubic_bezier": [x1, y1, x2, y2] }` (x1, x2 ∈ [0,1]). The non-linear named easings are not used by the visual vanilla data: not detailed here.

### 2.2 Evaluating a track at time T (integer, clock ticks)

1. A single key → constant value.
2. Segment construction:
   - with period P: an entry wrap segment (last key placed at `t_last − P` → first key at `t_first`), then one segment per pair of consecutive keys, then an exit wrap segment (last key at `t_last` → first key at `t_first + P`);
   - without period: only the segments between consecutive keys.
3. `t = T mod P` (positive modulo) if there is a period, otherwise `t = T`.
4. Segment = the first one whose `t < end`; failing that, the last one.
5. If `t ≤ start` → start value; if `t ≥ end` → end value; otherwise `α = (t − start)/(end − start)`, `α' = ease(α)`, result = interpKeyframe(α', start, end).

Two keys on the same tick create a zero-length segment (never chosen, since `t < end` cannot hold on it before the next one): this is how `sun_angle` (keys 6000→360 then 6000→0) gives a continuous ramp from 0 to 360° over one period, starting at noon (6000).

**Important**: the track is sampled at the **integer tick** of the clock, once per tick; the clock's fraction is not used. Smoothing between ticks comes only from the camera probe (§ 4.3).

Cubic Bézier: curve in x and in y with P0 = 0, P3 = 1, coefficients `a = 3c1 − 3c2 + 1`, `b = −6c1 + 3c2`, `c = 3c1`, `f(t) = ((a·t + b)·t + c)·t`.
Solving t for a given x: start with `t = x`; 4 Newton iterations: error `e = fx(t) − x`, stop if `|e| < 1e-5`; derivative `(3a·t + 2b)·t + c`, stop (fall back to bisection) if derivative `< 1e-5`; step `t −= clamp(e/derivative, −0.25, 0.25)`. If not converged: bisection on [0,1] starting from the current t (`e < 0` → low bound = t, otherwise high bound = t, t = midpoint), stop when `|e| < 1e-5`. Result `fy(t)`.

### 2.3 Useful vanilla data (Overworld)

`day` (clock `overworld`, period 24000), visual tracks:

| Attribute | Op | Ease | Keys (tick: value) |
|---|---|---|---|
| `sky_color` | multiply | linear | 133: `#FFFFFF`, 11867: `#FFFFFF`, 13670: `#000000`, 22330: `#000000` |
| `fog_color` | multiply | linear | 133: `#FFFFFF`, 11867: `#FFFFFF`, 13670: `#0F0F16`, 22330: `#0F0F16` |
| `cloud_color` | multiply | linear | 133: `#FFFFFFFF`, 11867: same, 13670: `#FF191926`, 22330: same |
| `sky_light_color` | multiply | linear | 730: `#FFFFFF`, 11270: same, 13140: `#7A7AFF`, 22860: same |
| `sky_light_factor` | multiply | linear | 730: 1, 11270: 1, 13140: 0.24, 22860: 0.24 |
| `sun_angle`, `star_angle` | override | Bézier (0.362, 0.241, 0.638, 0.759) | 6000: 360, 6000: 0 |
| `moon_angle` | override | same Bézier | 6000: 540, 6000: 180 |
| `star_brightness` | maximum | linear | 12 keys (see the JSON) |
| `sunrise_sunset_color` | override | linear | 32 ARGB keys (see the JSON) |

`moon` (period 192000): `moon_phase` in steps of 24000. `early_game`: nothing visual.

### 2.4 Client-side clocks

- `world_clock` registry: `overworld` and `the_end` (empty objects). Each timeline names its clock; the `default_clock` of the dimension type is **not** used by the timelines (it serves the End lightning).
- Client state per clock: `total_ticks` (64-bit integer), `partial` (float), `rate` (default 1; 0 = paused). Created as 0/0/1 on first access.
- Advance: on every client world tick (if the tick rate is running normally), `game_time += 1` then, with `Δ = game_time − gameTimeAtLastAdvance`: for each clock, `x = partial + Δ·rate` (as a double), `n = floor(x)`, `partial = x − n`, `total_ticks += n`.
- **Set Time** packet (play, clientbound): `game_time` (i64), then a clock → state table: registry id (VarInt), `total_ticks` (VarLong), `partial_tick` (f32), `rate` (f32). Handling: set the world's game_time, first advance all clocks with Δ as above, then replace the state of the listed clocks, and finally invalidate the attribute caches.
- Server emission: every 20 ticks with an **empty** table (only resynchronises game_time, hence Δ); full table on entering a world / respawn and when the time-advance rule changes; a single clock after a command (`time set/add`, pause, speed). The server sends `rate = 0` if the clock is paused or if the time-advance rule is false.

### 2.5 Rain and thunder levels on the client

The client does not make them evolve itself: "game event" packet type 7 → `rain_level = clamp(param, 0, 1)`, type 8 → `thunder_level`. The "old" and "current" values are equal, so independent of `pt`. Effective `thunder_level` = received value × `rain_level`. (Types 1 and 2 set the rain to 0 and 1 respectively, before the level values sent afterwards.)

---

## 3. Full chain for one frame (summary)

1. Client tick (20 Hz): clocks advance (§ 2.4), world tick (cache invalidation), camera tick → probe (§ 4.1), counters (underwater vision, boss darkening, lightning, effects).
2. Frame: interpolated camera position → fluid type (§ 5.1) → fog colour (§ 6) and distances (§ 5.3–5.4), attributes read through the probe with `pt` (§ 4.3).
3. Clear to the fog colour (§ 7.4) → sky (§ 8) → terrain and the rest with fog (§ 7).

---

## 4. Camera probe

All fog and sky attributes are read through a probe bound to the camera.

### 4.1 Update (20 Hz, at the game render tick)

On every client tick, with `P` = camera position of the **last render**:
1. Advance the value probes (§ 4.3).
2. Recompute the biome weights:
   - `q = P / 4 − (0.5, 0.5, 0.5)` ("quart" coordinates); `i = floor(q)`, `f = q − i` per axis;
   - kernel K = [0, 1, 4, 6, 4, 1, 0]; for k = 0..5: weight `w_k(f) = lerp(f, K[k+1], K[k])`, i.e. `1−f, 4−3f, 6−2f, 4+2f, 1+3f, f` (sum 16);
   - 216 samples: for z from 0 to 5, then x from 0 to 5, then y from 0 to 5 (this order matters), biome at the quart `(i.x−2+x, i.y−2+y, i.z−2+z)`, weight `w_x·w_y·w_z`;
   - biome at the quart: loaded chunk section, quart Y brought back within the world height; chunk not loaded → `plains`;
   - the weights are **accumulated per biome** (per attribute set), in order of first appearance.

### 4.2 Spatial blending of an interpolated attribute

Base value `v0` = output of the dimension layer. For each distinct biome, `vb = biome.apply(v0)` (v0 if the biome lacks the attribute).
- A single biome → `vb`.
- Otherwise, in order of appearance: `result = vb₁`, `total = w₁`; for each following one, `total += wᵢ`, `result = interpSpatial(wᵢ/total, result, vbᵢ)`. For lerp, this is an exact weighted average (up to float rounding).
- Non-interpolated attribute: biome at the exact point (quart of `P` via `floor(P/4)` per axis, without the zoom blur).

Then timelines, weather, client layers (§ 1.5).

### 4.3 Partial-tick smoothing

For each attribute read, the probe keeps `old` and `new`:
- on the first read: both = current value;
- on every tick: if `new` exists, `old = new`, `new = empty`; if no read happened during the elapsed tick, the probe is deleted;
- on a read with `pt`: if `new` is empty, it is computed now (current world state and biome weights); result = `interpPartialTick(pt, old, new)`.

Effect: the displayed values lag by one tick and are interpolated between the last two ticks. A camera reset (world change) clears everything.

---

## 5. Fog: choosing the environment

### 5.1 Fluid "in the camera"

`C` = camera position (interpolated), `B = floor(C)`.
1. If the fluid at B is water and `C.y < B.y + cameraHeight(B)` → **WATER**.
2. Otherwise, 5 points of the near plane (distance 0.05, half-height `tan(fov/2)·0.05`, half-width × window aspect ratio; `fov` = the user's setting, in whole degrees, without modifiers): front centre, then top-left, top-right, bottom-left, bottom-right corners. For each one, in order, `p = C + offset`, `b = floor(p)`:
   - lava fluid at b and `p.y ≤ b.y + cameraHeight(b)` → **LAVA**;
   - otherwise (no lava) powder snow block at b → **POWDER SNOW**.
3. Otherwise → **ATMOSPHERIC**.

`cameraHeight` = 1 if the fluid is a source and the block above has a full bottom face; otherwise the normal height of the fluid (see `fluids.md`).

### 5.2 Priority order

Ordered list: Lava, Powder Snow, Blindness, Darkness, Water, Atmospheric.
- **Distances**: the first applicable environment provides the distances.
- **Colour**: the first applicable one that *provides a colour* (Blindness and Darkness do not).
- **Darkening**: the first applicable one that modifies it (Blindness, Darkness).

Applicability: Lava/Powder Snow/Water/Atmospheric according to § 5.1; Blindness / Darkness if the camera entity is alive and has the effect.

### 5.3 Distances per environment

`R` = effective render distance in blocks = `16 · min(option, server distance if > 0)`.

| Environment | env. start | env. end | sky_end | cloud_end |
|---|---|---|---|---|
| Lava, normal | 0.25 | 1 | = end | = end |
| Lava, fire resistance | 0 | 5 | = end | = end |
| Lava, spectator | −8 | R/2 | = end | = end |
| Powder snow | 0 | 2 | = end | = end |
| Powder snow, spectator | −8 | R/2 | = end | = end |
| Blindness | d/4 | d | 0.8·d | 0.8·d |
| Darkness | 0.75·d | d | d | d |
| Water | `water_fog_start_distance` | `water_fog_end_distance · max(0.25, waterVision)` | = end | = end |
| Atmospheric | see § 5.4 | | | |

- Blindness: `d = 5` if infinite duration, otherwise `lerp(min(1, duration/20), R, 5)` (remaining duration in ticks).
- Darkness: `d = lerp(fadeFactor(pt), R, 15)`; fade factor of the effect: rises/falls by 1/22 per tick (fade duration 22), interpolated between ticks by `pt`.
- For all: then `renderStart = R − clamp(R/10, 4, 64)`, `renderEnd = R`.
- Background darkening (§ 6): the darkness `n` is the maximum of the void's and the effect's; Blindness: `n = 1` while the effect lasts more than 19 ticks, otherwise `max(duration/20, n)`; Darkness: `max(fadeFactor, n)`. Blindness takes precedence over Darkness (first environment that modifies).
- The sky (dome, sun, moon, stars, End sky) is not drawn when the camera has Blindness or Darkness; only the fog and the background colour remain.
- Implementation: `rc_env::fog` receives `blindness` (remaining ticks, −1 infinite) and `darkness_blend` (frame fade); `environ.rs` cuts the sky (`eye_effects`), `video.rs` follows both effects.

### 5.4 Atmospheric

Before the distances, update the **rain multiplier** `m` (persistent state, updated only when this environment is chosen, once per frame):
- `sky = clamp((skyLight(B) − 8)/7, 0, 1)` (raw sky light from the light engine, 0..15);
- `target = rain_level · sky · (biome at B has precipitation ? 1 : 0.5)` (biome at the block, full resolution);
- `m += (target − m) · Δticks · 0.2` (Δticks = frame duration in ticks; no clamping).

Then:
- `start = fog_start_distance − 160·m`
- `end0 = fog_end_distance`; `end = max(min(96, end0), end0 − 256·m)`
- `sky_end = min(R, sky_fog_end_distance)`
- `cloud_end = min(16·cloudRange, cloud_fog_end_distance)`
- If a boss bar asks for world fog: `start = min(start, 10)`, `end = min(end, 96)`, `sky_end = cloud_end = end`.

### 5.5 Local player's underwater vision

Integer counter `t` ∈ [0, 600], on the player's tick: if the eye is in water, `t += 1` (spectator: +10); otherwise if `t > 0`, `t −= 10`; clamped.
`waterVision`: 0 if the eye is not in water; 1 if `t ≥ 600`; otherwise `a = clamp(t/100, 0, 1)`, `b = (t < 100) ? 0 : clamp((t − 100)/500, 0, 1)`, result `0.6·a + 0.39999998·b`.
"Eye in water" (entity physics): the column `(floor x, floor z)` of the entity contains a water block such that `y_block ≤ eyeY ≤ y_block + cameraHeight`. If the camera is not the local player (another entity), the value is 1 for the colour brightening and the end is not multiplied.

---

## 6. Fog colour

Computed every frame (`pt` = world partial tick).

1. **Base colour** (providing environment):
   - Lava: `#991900`; Powder snow: `#9FBBCC`; Water: `water_fog_color` (probe);
   - Atmospheric: see 6.1.
2. **Void darkness**: `range = 32` (1 if the world is "flat" according to the login packet); `n = clamp((range + y_min − C.y)/range, 0, 1)`.
   - Blindness: if the effect ends in ≤ 19 ticks, `n = max(duration/20, n)`, otherwise `n = 1`.
   - Darkness: `n = max(fadeFactor(pt), n)`.
   - If `n > 0` and type ≠ Lava and ≠ Powder snow: rgb ×= `(1 − n)²`.
3. **Boss darkening** `k` (0..1, interpolated by `pt`): rises by 0.05/tick while a boss bar darkens the screen (max 1), otherwise falls by 0.0125/tick. If `k > 0`: `r = lerp(k, r, 0.7r)`, `g = lerp(k, g, 0.6g)`, `b = lerp(k, b, 0.6b)`.
4. **Brightening** `e`: type Water → `waterVision` (1 if the camera is not the local player); otherwise night vision active without Darkness → `nvScale`; otherwise 0. If r, g and b are all non-zero: `s = 1/max(r,g,b)`, each channel `c = lerp(e, c, c·s)`.
   `nvScale` = 1 if the effect still lasts more than 200 ticks (or infinite); otherwise `0.7 + 0.3·sin((duration − pt)·π·0.2)`.
5. Alpha = 1.

### 6.1 Atmospheric base colour

`D` = render distance in **chunks**.
1. `c = fog_color` (probe).
2. If `D ≥ 4`: `θ = sun_angle` in radians; `sx = (sin θ > 0) ? −1 : 1`; `f = forward.x · sx` (x of the camera's unit view vector); if `f > 0` and `α = sunrise_sunset_color.a > 0`: `c = lerp(f·α, c, sunrise_sunset_color.rgb)`.
3. `sky = sky_color` (probe, which already includes the weather gray), darkened a second time: if `rain > 0`: r,g ×= `1 − 0.5·rain`, b ×= `1 − 0.4·rain`; if `thunder > 0`: rgb ×= `1 − 0.5·thunder` (each product clamped to [0,1]; `rain`, `thunder` = the levels of § 2.5).
4. `sf = min(sky_fog_end_distance/16, D)`; `u = lerpClamped(sf/32, 0.25, 1)` (= 0.25 if sf/32 < 0, 1 if > 1, otherwise `0.25 + 0.75·sf/32`); `mix = 1 − u^0.25`.
5. Base = `lerp(mix, c, sky)`.

Example: D = 12, `sky_fog_end` = 512 → sf = 12, u = 0.53125, mix ≈ 0.1463.

---

## 7. Application in the shaders

"Fog" uniform block (std140): colour vec4, then 6 floats: env start, env end, render start, render end, sky_end, cloud_end. "No fog" variant: colour 0 and all distances = max float (used for the interface, the hand, etc., and if fog is disabled for debugging).

### 7.1 Distances at the vertex

`p` = vertex position **relative to the camera**, in world coordinates, before the view rotation (the terrain shader rebuilds it in three pieces: position within the section + (section origin − camera block) + (camera block − exact camera position), to keep float precision).
- spherical `ds = |p|`;
- cylindrical `dc = max(√(p.x² + p.z²), |p.y|)`.
Computed per vertex, interpolated (perspective-correct) up to the fragment.

### 7.2 Fog value

`lin(d, s, e)` = 0 if `d ≤ s`, 1 if `d ≥ e`, otherwise `(d − s)/(e − s)` (the `≤ s` test comes first, so s = e gives a step).
`v = max(lin(ds, envStart, envEnd), lin(dc, renderStart, renderEnd))`.
`final_rgb = lerp(v · colour.a, rgb, colour.rgb)`; alpha unchanged.

### 7.3 Passes concerned

Same terrain shader for: solid, cutout (discard if alpha < 0.5), translucent (discard if alpha < 0.1; water is in this layer), and the multi-draw variants. Order in the fragment:
1. colour = texture × vertex colour (lightmap included);
2. progressive section fade-in: `vis = lerp(clamp((|p| − 16)/16, 0, 1), 1, sectionVisibility)`, `colour = lerp(vis, (fog.rgb, colour.a), colour)` (the fade goes towards the fog colour);
3. optional alpha discard;
4. fog (§ 7.2). In ordered transparency (OIT, accumulation) the fog colour rgb is premultiplied by the fragment's alpha.

Entities, particles, block entities and lines apply the same formula with the same uniforms.

### 7.4 Background colour (clear)

Before anything of the world is drawn, the frame's colour is cleared to `(fog.r, fog.g, fog.b, 0)` (depth cleared to 0, reversed depth). No other background "sky colour": this is what shows where neither the sky nor the terrain draws (Nether, lava, powder snow, blindness, darkness, boss fog).

---

## 8. Plain sky (no celestial bodies)

### 8.1 When the sky is drawn

Only if: the dimension's sky type (`skybox`, default `overworld`) is not `none`; no boss fog; camera fluid type ≠ Lava and ≠ Powder snow; the camera entity has neither Blindness nor Darkness. `skybox = end`: textured End sky, without fog (out of scope). The sky is drawn **after** the clear and **before** the terrain, with the terrain's fog uniforms.

### 8.2 Dome (top disc)

Triangle fan, 10 vertices, in camera-relative coordinates (only the view rotation applies, no translation): centre (0, 16, 0) then, for an angle a from −180° to +180° in steps of 45° (9 vertices): `(512·cos a, 16, 512·sin a)`.
Plain colour = `sky_color` (probe, alpha 1).
Sky fog: `v = max(lin(ds, 0, sky_end), lin(dc, sky_end, sky_end))` then `rgb = lerp(v, sky_color, fog.rgb)` (fog.a = 1), with ds and dc computed at the fan's vertices then interpolated: centre ds = dc = 16; edge ds ≈ 512.25, dc = 512. With the defaults (sky_end = min(R, 512)), any fragment whose interpolated dc exceeds sky_end takes exactly the fog colour. For a pixel-exact rendering, **this mesh and this interpolation** must be reproduced, not a distance computed per pixel.

### 8.3 Dark disc

Same mesh with y = −16 and negative x (vertices `(−512·cos a, −16, 512·sin a)`), drawn in opaque black, shifted by +12 in y by the view matrix (the fog distances are still computed on the unshifted position). Drawn after the celestial bodies, only if `eyeY(pt) − horizon < 0` and the player is not underwater; `horizon = 63` (or the world's `y_min` if the world is flat).

### 8.4 Underwater

The sky is drawn (Water type allowed) with `sky_end = end of the water fog`: the dome's centre (distance 16) gets `v = 16/sky_end`, the rest quickly turns to the water fog colour.

---

## 9. Out of scope, flagged

- **Sunrise/sunset glow**: coloured fan (`sunrise_sunset_color`, alpha that overwrites the fan in depth) drawn after the dome, oriented according to the sign of `sin(sun_angle)`. Not fogged (no-fog pipeline).
- **Celestial bodies**: `sun_angle`, `moon_angle`, `star_angle` (radians = degrees·π/180), `star_brightness`, `moon_phase`, rain brightness `1 − rain_level`. To be specified with the full sky.
- **Clouds**: colour `cloud_color`, altitude `cloud_height`, transparency × `(1 − lin(ds, 0, cloud_end))` (spherical distance only).
- **Lightmap**: uses `block_light_tint`, `sky_light_factor`, `sky_light_color`, `ambient_light_color`, `night_vision_color` through the same probe; separate spec.
- **Lightning**: counter set to 2 by a lightning bolt, decremented every tick (§ 1.5).

---

## 10. Data to provide to the client

The registries `worldgen/biome`, `dimension_type`, `world_clock` and `timeline` are synchronised during configuration. If the client announces an **empty** list of known packs, the server sends their full content (network format = the JSON format as NBT, limited to the syncable attributes, which includes all those of § 1.1). The tags (`timeline`: `in_overworld`, etc.) arrive through the tags packet. For operation without a server (tests, bench), extract from the jar:

| File | Useful fields |
|---|---|
| `data/minecraft/dimension_type/*.json` (overworld, overworld_caves, the_nether, the_end) | `attributes` (`visual/*` keys, `gameplay/sky_light_level`), `timelines` (tag or list), `default_clock`, `skybox`, `has_skylight`, `has_ceiling`, `min_y`, `height` |
| `data/minecraft/worldgen/biome/*.json` (67) | `attributes` (`visual/sky_color`, `fog_color`, `water_fog_color`, `water_fog_end_distance`…), `has_precipitation` |
| `data/minecraft/timeline/*.json` (day, moon, early_game, villager_schedule) | `clock`, `period_ticks`, `tracks.*.{modifier, ease, keyframes}` |
| `data/minecraft/tags/timeline/*.json` | `in_overworld`, `in_nether`, `in_end`, `universal` |
| `data/minecraft/world_clock/*.json` | names only (empty objects) |

Notable vanilla values: Overworld `fog_color #C0D8FF`, `sky_color #78A7FF` (then biome); Nether `fog_start 10`, `fog_end 96`, `skybox none`, fog colour per biome (`#330808`, `#330303`, `#1A051A`, `#1B4745`, `#685F70`); End `fog_color #181318`, `sky_color #000000`, `skybox end`. Biomes: `mangrove_swamp` and `swamp` multiply `water_fog_end_distance` by 0.85.

To hard-code on the client side: defaults and bounds of § 1.1, weather modifiers of § 1.5, lava and powder snow colours, constants of § 5–8.

---

## 11. Unverified points

- Formulas of the named easings other than `linear`/`constant`/Bézier: not re-read (unused by the visual vanilla data).
- Scaling of the sunrise/sunset fan and rendering of the celestial bodies: not re-read in detail.
- Behaviour of the Bézier bisection if it never converges to 1e-5: not analysed (pathological case).
- Exact determination of "flat world" (login packet field): assumed, see `connection.md`.
