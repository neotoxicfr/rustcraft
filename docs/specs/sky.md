# Complete sky: celestial bodies, stars, glow, clouds, End sky (26.3)

Clean-room spec, written from a reading of the decompiled 26.3 client (kept outside the repository). It **complements** `environment.md` without repeating it: attributes, timelines, probe, fog, plain dome (§ 8.2), dark disc (§ 8.3) and sky display conditions (§ 8.1) are there. Here: everything else of the Overworld and End sky, and the clouds.

Conventions (in addition to those of `environment.md`):
- World frame: +X is east, −X west, +Y up, +Z south, −Z north.
- All sky elements are expressed **relative to the camera**: the starting model-view matrix ("MV") is the only view rotation (no translation). The clouds too (positions already relative).
- Composed rotations "A then B" read `M = MV · A · B`, applied to the vertex `v`: `MV · A · B · v` (B acts first on the vertex). `Rx(t)`, `Ry(t)`, `Rz(t)`: direct (right-handed) rotations of angle t around the axis.
  - `Rx(t)·(x,y,z) = (x, y·cos t − z·sin t, y·sin t + z·cos t)`
  - `Ry(t)·(x,y,z) = (x·cos t + z·sin t, y, −x·sin t + z·cos t)`
  - `Rz(t)·(x,y,z) = (x·cos t − y·sin t, x·sin t + y·cos t, z)`
- `sinT(x)`, `cosT(x)`: the game's **tabulated** sine/cosine (§ 8.2), used where specified; elsewhere, normal 32-bit float trigonometry.
- Culling: active unless stated otherwise, back faces removed, front face = counter-clockwise (CCW) on screen, window origin at bottom left, depth [0, 1] (classic GL convention). Checked numerically: with the vertex orders given here, **all** the sky faces are seen from the front from the camera (scratchpad `ciel_verif.py`).
- Quads: each group of 4 vertices `(0,1,2,3)` is split into triangles `(0,1,2)` and `(2,3,0)`.
- Blends (src/dst notation):

| Name here | Colour | Alpha |
|---|---|---|
| **opaque** | no blend | — |
| **translucent** | `src·αs + dst·(1−αs)` | `αs·1 + αd·(1−αs)` |
| **additive-α** | `src·αs + dst` | `αs` (dst ignored) |

---

## 1. Drawing order of a frame

### 1.1 Sequence

1. Colour clear = `(fog.rgb, 0)`, depth = 0 (`environment.md` § 7.4).
2. **"Sky" pass** (if allowed, `environment.md` § 8.1), colour + depth target, no clear, **terrain** fog uniforms:
   - if `skybox = end`: End box (§ 6), then End flash if its intensity > 1e−5 (§ 6.3). Nothing else.
   - if `skybox = overworld`:
     1. plain dome (`environment.md` § 8.2);
     2. sunrise/sunset glow (§ 2) — omitted if `alpha ≤ 0.001`;
     3. sun (§ 3);
     4. moon (§ 3);
     5. stars (§ 4) — omitted if `star_brightness ≤ 0`;
     6. dark disc (`environment.md` § 8.3) — if the eye is below the horizon and not underwater.
3. Opaque terrain, entities, etc. (out of scope).
4. In classic transparency (not Fabulous): translucent features, translucent terrain, "after terrain" translucent features, **clouds** (§ 5), then weather (rain/snow), then world border.
5. In Fabulous mode (improved transparency, OIT): the clouds go through the OIT path (§ 5.9).

### 1.2 Render states per element

| Element | Depth test | Depth write | Culling | Blend | Fog | Shader (role) |
|---|---|---|---|---|---|---|
| Dome / dark disc | no | no | yes | opaque | sky (`sky_end`) | uniform colour + fog |
| Sunrise/sunset glow | no | no | yes | translucent | no | vertex colour × uniform colour, discard if vertex α = 0 |
| Sun, moon, End flash | no | no | yes | additive-α | no | texture × uniform colour, discard if texture α = 0 |
| Stars | no | no | yes | additive-α | no | uniform colour only |
| End box | no | no | yes | translucent | no | texture × vertex colour × uniform (1), discard if α = 0 |
| "Fancy" clouds | `≥` (reversed depth) | yes | yes | translucent | clouds (`cloud_end`) | § 5 |
| "Fast" clouds | `≥` | yes | **no** | translucent | clouds | § 5 |

"Discard" = the fragment is abandoned **before** the multiplication by the uniform colour, on the raw alpha tested `== 0`.

Consequence: no sky element writes depth; the terrain drawn afterwards always covers them.

### 1.3 When each element disappears (summary)

| Situation | Effect |
|---|---|
| Lava, powder snow, Blindness, Darkness, boss fog, `skybox = none` | no sky pass (`environment.md` § 8.1) |
| Rain (rain_level) | sun and moon: alpha × `(1 − rain_level)`; `star_brightness` forced toward 0 by the weather modifier (so stars omitted in full rain); glow multiplied by the weather modifier |
| Underwater | sky drawn with `sky_end` = end of the water fog; dark disc omitted; clouds: `cloud_end` = end of the water fog |
| `sunrise_sunset_color.a ≤ 0.001` | glow omitted |
| `star_brightness ≤ 0` | stars omitted |
| Clouds option = Off, or `floor(cloud_color.a·255) = 0` | clouds omitted (Nether, End: `cloud_color` transparent by default) |

### 1.4 Far plane

Projection: near 0.05, far `max(4·16·renderDistance, 16·cloudRange)`. Nothing protects the sky from the far plane: with a small render distance **and** a small cloud range (far < 512), the edge of the dome is cut (vanilla artefact, to be reproduced as is). With the defaults (range 64 → 1024) the problem does not appear.

---

## 2. Sunrise / sunset glow

### 2.1 Mesh (built once)

Triangle fan, **18 vertices**, position + RGBA colour:
- vertex 0 (centre): `(0, 100, 0)`, colour white, α = 1;
- vertices 1..17, for `i = 0..16`: `θ = i·2π/16` (float), `(120·sinT(θ), 120·cosT(θ), −40·cosT(θ))`, colour white, α = 0.
(Vertex i = 16 closes the fan onto vertex i = 0.)

### 2.2 Transform and colour

Inputs: `sa = sun_angle` in radians (degrees · π/180, probe value smoothed by `pt`), `C = sunrise_sunset_color` (smoothed ARGB).
- If `C.a ≤ 0.001`: nothing.
- `φ = 180°` if `sinT(sa) < 0`, otherwise `0°`.
- Matrix: `M = MV · Rx(90°) · Rz(φ + 90°) · Scale(1, 1, C.a)`.
- Uniform colour = `(C.r, C.g, C.b, C.a)`. Fragment = interpolated vertex colour × uniform → `rgb = C.rgb`, `α = αinterp · C.a` (αinterp goes from 1 at the centre to 0 on the ring).
- Translucent blend, no fog.

Geometric result (verified): for `sin(sa) > 0` (afternoon/sunset) the centre is at `(−100, 0, 0)` (west horizon); for `sin(sa) < 0` at `(+100, 0, 0)` (east horizon). The ring is a horizontal circle of radius 120 around the camera, raised by `+40·C.a` on the side of the glow and lowered by `40·C.a` on the opposite side (alpha thus flattens the fan in height).

The colour `sunrise_sunset_color` comes from the `day` timeline (32 keys, `override` track) and from the rain/thunder modifier (`environment.md` § 1.5, § 2.3).

---

## 3. Sun and moon

### 3.1 Textures

"celestials" atlas (`assets/minecraft/atlases/celestials.json`, source = folder `textures/environment/celestial/`, no prefix), no mipmap, **nearest neighbour, clamped edge** sampling. A sprite's UVs fall exactly on its edges (atlas margin ≥ 1 texel outside). Exact equivalent: one texture per body with UV 0..1, nearest neighbour, clamp.

| Sprite | File | Size |
|---|---|---|
| `sun` | `textures/environment/celestial/sun.png` | 32×32, opaque (palette without transparency) |
| `moon/<phase>` (8) | `textures/environment/celestial/moon/<phase>.png` | 32×32 each |
| `end_flash` | `textures/environment/celestial/end_flash.png` | 64×64 |

Phases (`moon_phase`, not interpolated) and index:

| Index | Name | Index | Name |
|---|---|---|---|
| 0 | `full_moon` | 4 | `new_moon` |
| 1 | `waning_gibbous` | 5 | `waxing_crescent` |
| 2 | `third_quarter` | 6 | `first_quarter` |
| 3 | `waning_crescent` | 7 | `waxing_gibbous` |

### 3.2 Unit quads

Quad in the plane y = 0, vertices in this order: `(−1,0,−1)`, `(1,0,−1)`, `(1,0,1)`, `(−1,0,1)`. UV (u0,v0 = top-left corner of the sprite, u1,v1 = bottom-right):

| Vertex | Sun / End flash | Moon (each phase) |
|---|---|---|
| (−1,0,−1) | (u0, v0) | (u1, v1) |
| (1,0,−1) | (u1, v0) | (u0, v1) |
| (1,0,1) | (u1, v1) | (u0, v0) |
| (−1,0,1) | (u0, v1) | (u1, v0) |

(The moon is thus rotated by 180° relative to the sun.) The 8 phases are stored one after the other (4 vertices each); phase `k` is drawn with a vertex offset of `4k`.

### 3.3 Transforms

`sa`, `ma` = `sun_angle`, `moon_angle` in radians (probe smoothed by `pt`).
- Sun: `M = MV · Ry(−90°) · Rx(sa) · Translation(0, 100, 0) · Scale(30, 1, 30)`.
- Moon: `M = MV · Ry(−90°) · Rx(ma) · Translation(0, 100, 0) · Scale(20, 1, 20)`.

So half-side 30 (sun) and 20 (moon), at distance 100; body centre = `(−100·sin a, 100·cos a, 0)`: angle 0 = zenith, 90° = west horizon, 180° = nadir, 270° = east horizon. The orbit plane is the X–Y plane (no tilt).

### 3.4 Colour and blend

- `rainLum = 1 − rain_level(pt)` (rain level only, not thunder).
- Uniform colour = `(1, 1, 1, rainLum)`. Fragment = `texel × (1,1,1,rainLum)` (discard if `texel.a = 0`).
- **Additive-α** blend: `dst.rgb += texel.rgb · texel.a · rainLum`. Black texels add nothing; there is no tint.
- No fog. Order: sun then moon.

---

## 4. Stars

### 4.1 Pseudo-random generator

48-bit linear congruential generator, **identical to Java's**, but without a lock:
- Initialisation: `s = (seed XOR 0x5DEECE66D) & (2⁴⁸ − 1)`.
- `next(b)`: `s = (s·0x5DEECE66D + 0xB) & (2⁴⁸ − 1)`, returns `(s >> (48 − b))` interpreted as a signed 32-bit integer.
- `next_float()` = `next(24) · 2⁻²⁴` (32-bit float, exact).
- `next_double()` = `(next(26)·2²⁷ + next(27)) · 2⁻⁵³` (64-bit double; the game's multiplier, written as a 32-bit float, is exactly 2⁻⁵³ — verified).

### 4.2 Generation (once, at the start of sky rendering)

Seed **10842**. For `n = 0..1499` (1500 attempts) — all in 32-bit float unless stated otherwise:
1. `x = 2·next_float() − 1`, `y = 2·next_float() − 1`, `z = 2·next_float() − 1`.
2. `t = 0.15 + 0.1·next_float()` (half-side of the quad).
3. `L = x² + y² + z²`. If `L ≤ 0.010000001` or `L ≥ 1`: attempt rejected, **nothing more is drawn** for this attempt (go to the next).
4. Centre `c = (x, y, z) · (100 / √L)` (computation: `k = (1/√L)·100`, then each component × k).
5. `ζ = float32( next_double() · float32(π) · 2 )` (drawn **only** if the attempt is accepted).
6. Basis: `d = −c/|c|` (toward the camera), `g = normalise(up × d)` with `up = (0,1,0)`, `h = d × g`. Matrix `B` with columns `(g, h, d)` (sends local +Z to d). Rotation `R = B · Rz(−ζ)`.
7. Quad vertices, in order: `R·(t, −t, 0) + c`, `R·(t, t, 0) + c`, `R·(−t, t, 0) + c`, `R·(−t, −t, 0) + c`.

Verified result (script): **780 stars accepted** → 3120 vertices, 4680 indices. Test vectors:

| Star | Attempt | Centre ≈ | Half-side t | ζ |
|---|---|---|---|---|
| 1st | 1 | (−53.2469, 69.9257, 47.6987) | 0.15018426 | 4.4662609 |
| 2nd | 5 | (−96.8909, −18.4667, 16.4663) | 0.24897167 | 3.7730346 |
| 3rd | 6 | (−98.7993, 9.6515, 12.0643) | 0.19748931 | 5.3536968 |
| last | 1499 | (−19.1850, 92.1324, −33.8165) | 0.19358836 | 5.5966301 |

First vertex of the 1st star ≈ (−53.2247, 70.0560, 47.5324).

### 4.3 Drawing

- `st = star_angle` (radians), `b = star_brightness` (smoothed probe). If `b ≤ 0`: nothing.
- `M = MV · Ry(−90°) · Rx(st)` (no translation nor scale: the stars are already at distance 100).
- Uniform colour = `(b, b, b, b)`; fragment = this colour (no texture, no per-vertex colour).
- Additive-α blend: `dst.rgb += b · b` (the square of the brightness, on the three channels). No fog.

---

## 5. Clouds

### 5.1 Source texture and cells

`textures/environment/clouds.png`: 256×256, 1-bit grey levels with transparency on grey 0 → black pixels **transparent**, white opaque (18,103 full cells). Read **on the CPU** when the resources load (no GPU sampling):
- Cell empty if `alpha < 10` (out of 255).
- For a full cell at (x, y), store four "empty neighbour" flags, with wrapping (positive modulo): north = (x, y−1), east = (x+1, y), south = (x, y+1), west = (x−1, y). (The game wraps x with the image **height**; no effect for a square image.)
- The pixel's colour is **not** used at render time; only presence counts.
- Column x ↔ world X axis, row y ↔ Z axis.

### 5.2 Constants

| Constant | Value |
|---|---|
| Cell size | 12 × 4 × 12 blocks (X × Y × Z) |
| Speed | 0.030000001 block/tick toward **−X** (west), i.e. 0.6 block/s; 400 ticks per cell |
| Fixed Z offset | +3.96 blocks |
| Period of the time offset | `width·400` = 102,400 ticks |
| Empty cell threshold | alpha < 10 |

### 5.3 Inputs per frame

- `color` = `cloud_color` (ARGB, probe smoothed by the world's `pt`) **converted to 8 bits per channel**: `byte = floor(v·255)`, then brought back to a float `byte/255` for the shader. Clouds omitted if the alpha byte is 0.
- `baseHeight` = `cloud_height` (probe; only re-read if alpha is > 0, otherwise the old value stays). Overworld: 192.33, colour `#CCFFFFFF` (α = 0.8) modulated by the `day` timeline and the weather.
- `mode`: Fancy (volumes) or Fast (flat). `range` in chunks.
- `gameTime` = **game_time** of the client world (64-bit integer, +1 per tick, resynchronised by the Set Time packet — not a timeline clock), `pt` = world tick fraction.
- Camera position `cam` (double).

### 5.4 Placement computation

1. `cellRadius = ceil(range·16 / 12)`.
2. `bottom = float32(baseHeight − cam.y)`, `top = bottom + 4`. Relative position: **above** if `top < 0`, **below** if `bottom > 0`, otherwise **inside**.
3. `offset = float32(gameTime mod 102400) + pt` (32-bit float).
4. `nx = cam.x + offset·0.030000001` (product in 32-bit float, sum in double); `nz = cam.z + 3.96`.
5. Bring back into the texture: `nx −= floor(nx / 3072)·3072`, same for `nz` (3072 = 256·12).
6. `cx = floor(nx / 12)`, `cz = floor(nz / 12)`; `fx = float32(nx − 12·cx)`, `fz = float32(nz − 12·cz)`.
7. Mesh offset (camera-relative): `O = (−fx, bottom, −fz)`.

A relative mesh cell `(i, k)` (integers) corresponds to the texture cell `((cx + i) mod 256, (cz + k) mod 256)` and occupies in camera-relative terms `[12i − fx, 12i − fx + 12] × [bottom, bottom + 4] × [12k − fz, 12k − fz + 12]`.

The mesh is only rebuilt if `cx`, `cz`, the relative position or the mode change (or after a reload); otherwise only `O` changes.

### 5.5 Cell walk (order = drawing order)

From nearest to farthest, by rings of Manhattan distance:
- for `r = 0 .. 2·cellRadius`; for `i = −r .. r`: `k = r − |i|`;
- keep if `k ≤ cellRadius` and `i² + k² ≤ cellRadius²`;
- if `k ≠ 0`: process cell `(i, −k)`; then process `(i, k)`.
An empty texture cell produces nothing.

### 5.6 Faces emitted per full cell

Direction numbers: 0 down, 1 up, 2 north (−Z), 3 south (+Z), 4 west (−X), 5 east (+X).

**Fast**: a single face, "down" (plane y = bottom), with the "top colour" flag. No culling (visible from both sides).

**Fancy**, in this order:
1. top, unless the camera is **below**;
2. bottom, unless the camera is **above**;
3. north if north neighbour empty **and** `k > 0`;
4. south if south neighbour empty **and** `k < 0`;
5. west if west neighbour empty **and** `i > 0`;
6. east if east neighbour empty **and** `i < 0`;
7. if `|i| ≤ 1` and `|k| ≤ 1`: the 6 **inner** faces (order down, up, north, south, west, east), with no neighbour or position condition, with reversed vertex order (seen from inside).

### 5.7 Geometry of a face (unit cube vertices, then × (12, 4, 12))

| Face | Vertices in order |
|---|---|
| down | (1,0,0) (1,0,1) (0,0,1) (0,0,0) |
| up | (0,1,0) (0,1,1) (1,1,1) (1,1,0) |
| north | (0,0,0) (0,1,0) (1,1,0) (1,0,0) |
| south | (1,0,1) (1,1,1) (0,1,1) (0,0,1) |
| west | (0,0,1) (0,1,1) (0,1,0) (0,0,0) |
| east | (1,0,0) (1,1,0) (1,1,1) (1,0,1) |

Inner face: vertices taken in order 3, 2, 1, 0. Camera-relative position = `vertex·(12,4,12) + (12i, 0, 12k) + O`.

Vanilla encoding (for information, reproducible as is on the GPU): 3 signed bytes per face — `i >> 1`, `k >> 1`, then `direction | 16·inner | 32·topColor | 128·(i & 1) | 64·(k & 1)`; buffer of signed 8-bit integer texels; the shader rebuilds `i = (byte << 1) | bit`.

### 5.8 Colour, shading, fog, blend

- Face factor: down 0.7, up 1.0, north/south 0.8, west/east 0.9; "top colour" (Fast clouds) → 1.0. Factor applied as (f, f, f, 1).
- `vertexColor = factor × color` (8-bit colour of § 5.3, alpha included).
- Fog distance = **spherical** distance to the vertex (`|relative position|`), interpolated.
- Fragment: `rgb = vertexColor.rgb`; `α = vertexColor.a · (1 − lin(d, 0, cloud_end))` where `lin` is the clamped linear ramp of `environment.md` and `cloud_end = min(16·range, cloud_fog_end_distance)` in atmospheric (other environments: `environment.md` § 5.3–5.4). **No** blend toward the fog colour: the clouds fade only through alpha.
- Translucent blend; "closer or equal" depth test (reversed depth) **with write**. Combined with the near → far order, a farther face covered by an already drawn face is rejected: do not sort otherwise.

### 5.9 Options and defaults

| Setting | Raw option (default) | Fast preset | Fancy preset (game default) | Fabulous preset |
|---|---|---|---|---|
| Clouds | Fancy | Fast | Fancy | Fancy |
| Cloud range (chunks, 2–128) | 128 | 32 | 64 | 128 |
| Improved transparency (OIT) | — | no | no | yes |

The default graphics preset (Fancy) is applied at startup: in practice **Fancy, range 64** (`cloud_end` = min(1024, 2048) = 1024). In Fabulous, the clouds are drawn through the OIT path (dedicated depth pass, weighted accumulation): not specified here.

---

## 6. End sky

### 6.1 Box

Six quads, each obtained by transforming the base quad, vertices in order:
`(−100, −100, −100)` UV (0, 0); `(−100, −100, 100)` UV (0, 16); `(100, −100, 100)` UV (16, 16); `(100, −100, −100)` UV (16, 0).

| Face | Transform | Face obtained |
|---|---|---|
| 0 | identity | bottom (y = −100) |
| 1 | `Rx(+90°)` | north (z = −100) |
| 2 | `Rx(−90°)` | south (z = +100) |
| 3 | `Rx(180°)` | top (y = +100) |
| 4 | `Rz(+90°)` | east (x = +100) |
| 5 | `Rz(−90°)` | west (x = −100) |

Vertex colour: `#FF282828` (r = g = b = 40/255, α = 1). Texture `textures/environment/end_sky.png` (128×128, opaque), **repeating, nearest neighbour** sampling (plain texture without metadata): UV 0..16 → the texture is repeated **16 × 16 times per face** (each texel covers 200/(16·128) ≈ 0.098 block at distance 100).

Fragment = `texel × (40/255, 40/255, 40/255, 1)`, discard if α = 0; translucent blend (opaque texture → replaces). No fog, no depth, no animation: the box is fixed (only the view rotation applies).

### 6.2 What the End does not have

No dome, no glow, no sun, moon, stars, no dark disc; no clouds (`cloud_color` transparent by default).

### 6.3 End flash (recent addition, present in 26.3)

Active if `skybox = end`. State updated **on every client tick** with `T` = `total_ticks` of the dimension's default clock (`default_clock`; 0 if there is none):
1. `g = T div 600` (integer division). If `g` differs from the stored `g`: new generator (§ 4.1) with seed `g`; one `next_float()` discarded; `offset = next_int(201)` (0..200); `duration = 100 + next_int(min(380, 600 − offset) − 100 + 1)`; `angleX = next_float()·70 − 60` (degrees, [−60, 10)); `angleY = next_float()·360 − 180` (degrees); store `g`.
   - `next_int(n)`: if n is a power of 2, `(n · next(31)) >> 31`; otherwise repeat `a = next(31)`, `m = a mod n` while `a − m + (n − 1)` overflows negative (signed 32-bit integer); returns `m`.
2. `previous = intensity`; `u = T mod 600`; `intensity = sinT((u − offset)·π / duration)` if `offset ≤ u ≤ offset + duration`, otherwise 0.
3. Render: `I = lerp(pt, previous, intensity)`; drawn if `I > 1e−5`.

Drawing: quad of the `end_flash` sprite (same quad and UVs as the sun, § 3.2), `M = MV · Ry(180° − angleY) · Rx(−90° − angleX) · Translation(0, 100, 0) · Scale(60, 1, 60)`; uniform colour `(I, I, I, I)`; additive-α blend → `dst.rgb += texel.rgb · texel.a · I²`. Centre ≈ `(−100·cos aX·sin aY, −100·sin aX, 100·cos aX·cos aY)`.
(A directional sound is also played 30 ticks after the start of a flash: out of scope.)

---

## 7. Resources and constants

### 7.1 Files to load from the client jar

| Path | Use |
|---|---|
| `assets/minecraft/atlases/celestials.json` | atlas definition (folder `environment/celestial`) — optional if the PNGs are loaded directly |
| `assets/minecraft/textures/environment/celestial/sun.png` | sun |
| `assets/minecraft/textures/environment/celestial/moon/{full_moon, waning_gibbous, third_quarter, waning_crescent, new_moon, waxing_crescent, first_quarter, waxing_gibbous}.png` | moon phases |
| `assets/minecraft/textures/environment/celestial/end_flash.png` | End flash |
| `assets/minecraft/textures/environment/end_sky.png` | End box |
| `assets/minecraft/textures/environment/clouds.png` | cloud map (CPU read) |
| `data/minecraft/timeline/day.json`, `moon.json` | `sun_angle`, `moon_angle`, `star_angle`, `star_brightness`, `sunrise_sunset_color`, `cloud_color`, `moon_phase` (already listed in `environment.md` § 10) |
| `data/minecraft/dimension_type/*.json` | `skybox`, `default_clock`, `cloud_color`, `cloud_height` |

### 7.2 Constants to code

| Name | Value |
|---|---|
| Glow: vertices / steps / radius / centre height / depth | 18 / 16 / 120 / 100 / 40 |
| Glow: alpha omission threshold | 0.001 |
| Sun: distance / half-side | 100 / 30 |
| Moon: distance / half-side | 100 / 20 |
| Base rotation of bodies and stars | `Ry(−90°)` |
| Stars: seed / attempts / accepted / distance | 10842 / 1500 / 780 / 100 |
| Stars: half-side | 0.15 + 0.1·u |
| Stars: rejection | `L ≤ 0.010000001` or `L ≥ 1` |
| End box: half-side / UV repeat / colour | 100 / 16 / `#FF282828` |
| End flash: interval / max offset / min duration / max duration / distance / half-side | 600 / 200 / 100 / 380 / 100 / 60 |
| End flash: angleX / angleY | [−60°, 10°) / [−180°, 180°) |
| Clouds: cell | 12 × 4 × 12 |
| Clouds: speed / Z offset | 0.030000001 block/tick toward −X / 3.96 |
| Clouds: face factors down / up / N-S / W-E | 0.7 / 1.0 / 0.8 / 0.9 |
| Clouds: texture alpha threshold | 10 |
| Sine table | 65,536 entries |

### 7.3 Tabulated sine

`T[j] = float32(sin(j / 10430.378350470453))` for `j = 0..65535` (10430.378… = 65536/2π). `sinT(x) = T[(int64(x·10430.378350470453)) & 65535]`, `cosT(x) = T[(int64(x·10430.378350470453 + 16384)) & 65535]` (truncation toward zero of the product in double, then mask). Used for: glow mesh and orientation, End flash intensity. The plain dome (`environment.md` § 8.2) uses it too.

---

## 8. Unverified or approximated points

- OIT (Fabulous) path of the clouds: not specified.
- The exact rotation functions (the game's 32-bit float quaternions) are not reproduced bit for bit; expected difference ~1e−6, no visible effect.
- Rotation basis of the stars: "local +Z → direction" convention of the third-party math library, recalled from memory then confirmed only by the visibility test (the quads face the camera); the in-plane orientation (rotation ζ) depends on this convention — to be rechecked by capture if a difference in star orientation shows up.
- GPU format of the cloud face buffer: signed 8-bit integers (read); range `|i| ≤ 255` hence `cellRadius ≤ 171` at range 128: fits.
- The rain level used for the bodies is the world's `rain_level(pt)`; `environment.md` § 2.5 states that it is in practice constant between two packets.
- Filling of the atlas margin (sprite edges): not read; no effect with exact UVs and nearest neighbour, except sampling rounding right at the edge.
