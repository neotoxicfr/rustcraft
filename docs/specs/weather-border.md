# Weather and world border (26.3)

Clean-room spec, written from a reading of the decompiled 26.3 client (kept outside the repository). It describes the rules, not the code. It **complements** without repeating:

- `environment.md`: rain and thunder levels (§ 2.5), weather modifiers of the attributes (§ 1.5), rain fog (§ 5.4), background darkening (§ 6), sky "lightning" layer (§ 1.5 point 5);
- `sky.md`: pass order (§ 1, point 4: clouds → weather → border), sun, moon and stars in the rain (§ 1.3);
- `lighting.md`: lightmap sky factor during a lightning flash;
- `particles.md` § 1.7 (ground drops) and the `rain`, `splash`, `dripping_water` entries; `sounds.md` § 5.7 (rain sound);
- `hud.md` § 5.3 (vignette, including its red tint near the border);
- `entities-objects.md` § 11 (lightning drawing); `movement.md` (collisions).

Conventions: world frame +X east, +Y up, +Z south. `t` = game time (tick, 64-bit integer), `pt` = tick fraction. "Float" = 32 bits, "double" = 64 bits; 32-bit integers with silent overflow unless stated otherwise. "Java LCG" generator = that of `sky.md` § 4.1 (seed `(g XOR 0x5DEECE66D) & (2⁴⁸−1)`, `next(b)`, `next_int(n)`, `next_float = next(24)·2⁻²⁴`, `next_double = (next(26)·2²⁷ + next(27))·2⁻⁵³`).

---

## 1. Precipitation

### 1.1 State received

- Levels `rain_level` and `thunder_level`: `environment.md` § 2.5 (game events 1, 2, 7, 8). No interpolation on the client side: each packet sets the "old" and "current" value to the same value clamped to [0, 1]. Effective thunder = `thunder_level × rain_level`.
- The smoothness comes from the server: it varies each level by ±0.01 per tick (0 → 1 in 100 ticks) and sends an event 7 (resp. 8) on **every** tick where the value changes. When "it is raining" flips (rain level crossing 0.2), it first sends 1 (start, sets 0) or 2 (end, sets 1), then 7 and 8 with the real values. These packets go to all players, whatever their dimension.
- On login or dimension change, if it is raining: 1, then 7 (rain), then 8 with a value **already multiplied** by the rain on the server side (the client multiplies it again: vanilla quirk to keep, no effect at full rain).
- "It is raining" on the client side = the dimension can have weather (lit sky, no ceiling, not the End) **and** `rain_level > 0.2`. "There is thunder" = same condition and effective thunder `> 0.9`. These two tests serve the drips under leaves (§ 2.2) and the lightning rod particles (`particles.md`).

### 1.2 Where it rains, where it snows

Two different functions.

**Precipitation of a column (rendering, ground drops, sound)** at the point `P = (x, y, z)`:
1. Chunk `(x >> 4, z >> 4)` absent → none.
2. Biome at `P` (same biome read as for the rest of the rendering, see `environment.md`): if it has no precipitation (`has_precipitation` false in the biome registry) → none.
3. Otherwise: **rain** if `temperature(P) ≥ 0.15`, **snow** otherwise.

No dimension test nor rain level here: the Nether and the End only have biomes without precipitation.

**"It rains here"** (drips under leaves, § 2.2): it is raining (§ 1.1) **and** raw sky light at `P` = 15 **and** "blocks motion" height of the column ≤ `y` **and** column precipitation at `P` = rain.

**Temperature** at the point `P` (biome registry: `temperature` = base, `temperature_modifier` = `none` or `frozen`, sent to the client):
- Modified base: `none` → base. `frozen` → `a = F(x·0.05, z·0.05)·7 + B(x·0.2, z·0.2)`; if `a < 0.3` and `B(x·0.09, z·0.09) < 0.8` → **0.2**; otherwise base.
- Height: `threshold = sea level + 17` (sea level received on login). If `y > threshold`: `temp = base_mod − (8·T(x/8, z/8) + y − threshold)·0.05/40` (float computation, divisions `x/8` in float); otherwise `temp = base_mod`.

Noises (classic 2D simplex with 12 gradients, result `70 × sum of the corners`, radius 0.5):
- Each noise is built from a Java LCG: 3 doubles drawn then **discarded** (offsets forced to 0), then 256-byte permutation table shuffled by `for i = 0..255: j = i + next_int(256 − i); swap(p[i], p[j])` (initial table `p[i] = i`).
- 2D simplex at the point `(u, v)`: `s = (u + v)·F2`, `F2 = (√3 − 1)/2`, `G2 = (3 − √3)/6`; `i = ⌊u + s⌋`, `j = ⌊v + s⌋`; corner 0 = `(u − i + (i+j)·G2, v − j + (i+j)·G2)`; second corner `(1,0)` if `x0 > y0`, otherwise `(0,1)`; corner gradient = `p[(ii + corner_i + p[(jj + corner_j) & 255]) & 255] mod 12` with `ii = i & 255`, `jj = j & 255`; contribution `max(0, 0.5 − dx² − dy²)⁴ · (gx·dx + gy·dy)`. Gradients (x, y) in order: (1,1), (−1,1), (1,−1), (−1,−1), (1,0), (−1,0), (1,0), (−1,0), (0,1), (0,−1), (0,1), (0,−1). Result converted to float.
- `T`: seed 1234, a single noise.
- `B`: seed 2345, a single noise.
- `F`: seed 3456, **three** noises built one after the other on the same generator; `F(u, v) = (1/7)·S₁(u, v) + (2/7)·S₂(u/2, v/2) + (4/7)·S₃(u/4, v/4)` (amplitudes in float 0.14285715 / 0.2857143 / 0.5714286, frequencies 1 / 0.5 / 0.25, sum in float).

### 1.3 "Blocks motion" heightmap

- Received in the chunk packet (entry `MOTION_BLOCKING`), stored per column as "first free y" (y of the highest blocking block + 1; world minimum Y if none).
- The client **keeps it up to date** on every block change: a block is "blocking" if it carries the block tag `minecraft:blocks_motion_in_heightmap` (received from the server with the other tags) **or** if it contains a fluid. For a block placed at `y` in a column of height `h`:
  - if `y ≤ h − 2`: nothing;
  - if the new state is blocking and `y ≥ h`: `h = y + 1`;
  - if the new state is not blocking and `y = h − 1`: go down from `y − 1` to the first blocking block `y'` → `h = y' + 1` (world minimum Y if none).
- Reading `height(x, z)`: chunk loaded → stored value; chunk absent → world minimum Y; coordinates beyond ±30,000,000 → sea level + 1.

### 1.4 Column rendering: selection (each frame)

If `I = rain_level` (the current value, § 1.1) ≤ 0: nothing. Otherwise, with `R` = option "weather radius" (3 to 10, default 10; `option-menus.md`) and `(cx, cy, cz)` = the camera's block (integer part of the render position):

For `z` from `cz − R` to `cz + R`, then `x` from `cx − R` to `cx + R` (z in the outer loop):
1. `h = height(x, z)` (§ 1.3).
2. `y0 = max(cy − R, h)`, `y1 = max(cy + R, h)`. If `y0 = y1`: column ignored.
3. Column precipitation at the point `(x, cy, z)` (§ 1.2). None → ignored.
4. Java LCG generator reset with the **32-bit integer** seed `g = (x·x·3121 + x·45238971) XOR (z·z·418711 + z·13761)`, sign-extended to 64 bits. The queue of pending Gaussian values is emptied on every reset.
5. Light `L` = light at the point `(x, max(cy, h), z)`: raw (block, sky) pair; if the block state at this point has emissive rendering → (15, 15); otherwise block light raised to at least the light emitted by this block.
6. Rain → rain column (§ 1.5); snow → snow column (§ 1.6). The rain and snow columns go into two separate lists, each in walk order.

### 1.5 Rain column

- `tickOffset = (x·x·3121 + x·45238971 + z·z·418711 + z·13761) & 255` (32-bit integer sum, without XOR this time).
- `speed = 3 + next_float()`.
- `o = −((t & 131071) + tickOffset + pt) / 32 × speed` (float; the integer is converted before adding `pt`).
- `vOffset = o mod 32` (float remainder with the sign of the dividend, hence ≤ 0); `uOffset = 0`.
- Light: `L` as is.

### 1.6 Snow column

- `time = (t & 131071) + pt` (float).
- `u = next_double() + time·0.01·gauss₁`; `v = next_double() + time·gauss₂·0.001`. Order of the draws: double, gaussian, double, gaussian. The products are in float (the gaussian converted to float), the addition to the double then conversion to float. The gaussians come from Marsaglia's polar method (like the Java LCG: pair `x, y = 2·next_double() − 1` until `0 < x² + y² < 1`, factor `√(−2·ln s / s)`, returns `x·factor` and keeps `y·factor` for the next call): `gauss₂` is thus the second value of the pair drawn for `gauss₁`.
- `vOffset = −((t & 511) + pt)/512 + v`; `uOffset = u`.
- Brightened light: `block' = (block·3 + 15) div 4`, `sky' = (sky·3 + 15) div 4` (integer division).

### 1.7 Geometry

Fixed 32 × 32 table, for `a, b ∈ [0, 31]`: `dx = a − 16`, `dz = b − 16`, `d = √(dx² + dz²)`; `tx[b][a] = −dz/d`, `tz[b][a] = dx/d` (floats). For `a = b = 16` the division gives NaN: **the camera block's column produces a degenerate quad, hence invisible** (to reproduce, e.g. by not drawing this column).

For each column `(x, z, y0, y1, uOffset, vOffset, L)`, with `C` the exact camera position:
- `rx = (x + 0.5 − Cx)`, `rz = (z + 0.5 − Cz)` (doubles converted to float), `d² = rx² + rz²`.
- `alpha = lerp(min(d²/R², 1), αmax, 0.5) × I` with `αmax = 1` (rain) or `0.8` (snow) — opaque at the centre, half at the edge. White colour, alpha as a byte `⌊alpha·255⌋`.
- Half-widths: `(hx, hz) = (tx, tz)[z − ⌊Cz⌋ + 16][x − ⌊Cx⌋ + 16] / 2`: quad one block wide, perpendicular to the camera → column direction (orientation quantised per column, not a real camera-facing billboard).
- `X0 = rx − hx`, `X1 = rx + hx`, `Z0 = rz − hz`, `Z1 = rz + hz`, `Yt = y1 − Cy`, `Yb = y0 − Cy`.
- `u0 = uOffset`, `u1 = uOffset + 1`; `v0 = y0·0.25 + vOffset`, `v1 = y1·0.25 + vOffset` (one texture height per 4 blocks; the texture's full width covers the block).
- Four vertices in this order: `(X0, Yt, Z0 ; u0, v0)`, `(X1, Yt, Z1 ; u1, v0)`, `(X1, Yb, Z1 ; u1, v1)`, `(X0, Yb, Z0 ; u0, v1)`, all with the same colour and the same light `L`.

Effect: `vOffset` decreases with time, so the pattern goes down; a rain column scrolls one texture (4 blocks) in `32 / speed` ticks (8 to 10.7 ticks). Snow goes down one texture in 512 ticks, plus a drift specific to each column (gaussians) in u and v. The jumps of `t & 511` are invisible (whole-texture offset); that of `t & 131071` produces a visible jump every ~109 game minutes (vanilla).

### 1.8 Drawing

- Textures `textures/environment/rain.png` and `textures/environment/snow.png` (64 × 256 each), **nearest-neighbour, repeat** sampling on both axes, no mipmap.
- Particle program (`particles.md`: camera-relative position, colour × lightmap sampled at `L` (linear filtering, clamped edge), spherical and cylindrical terrain fog, **discard if final alpha < 0.1**).
- Classic alpha blend (`src·α + dst·(1 − α)`; alpha: `src + dst·(1 − α)`), no back-face culling, "closer or equal" depth test (reversed depth), **no depth write**.
- A single vertex buffer: all the rain columns then all the snow ones; two calls: rain with `rain.png`, then snow with `snow.png`.
- Order in the frame: after the clouds, before the border (`sky.md` § 1). With improved transparency (OIT), the weather is drawn in each OIT stage, in the common pass "translucent objects, border, weather", border first.

### 1.9 What changes in 26.3 (for those who know earlier versions)

- The radius is no longer tied to the graphics mode: it is the "weather radius" option 3–10 (presets 5 / 10 / 10, `option-menus.md`).
- The "blocks motion" heightmap is defined by a **block tag** sent by the server, and no longer by a hard-coded block property.
- The temperature noises are described by a new structure (stack of simplex noises with explicit frequencies and amplitudes); the formulas above are those of 26.3. Exact equivalence with earlier versions has not been verified.
- The extraction (column selection, § 1.4) is separate from the drawing; the drawing also exists in an OIT version.

---

## 2. Ground effects

### 2.1 Drops, smoke and sound (each client tick)

Already described in `particles.md` § 1.7 and `sounds.md` § 5.7; here is the exact order of the draws, which makes it a deterministic computation. Run at the client world tick only if the world is running normally (not frozen by the tick rate command), and if `p = rain_level > 0`:

1. Java LCG generator with seed `t × 312987231` (64 bits). `B` = block containing the camera position. `D = 2R + 1`.
2. `n = ⌊0.225·D²·p²⌋` (product in float, truncated), divided (integer division) by 2 if particles are "decreased".
3. Repeat `n` times: `dx = next_int(D) − R`, `dz = next_int(D) − R`; `H` = "first free y" position of the column `B + (dx, 0, dz)` (§ 1.3). Kept if `H.y > world minimum Y`, `|H.y − B.y| ≤ 10` and column precipitation at `H` = rain (§ 1.2). Then `S = H − (0, 1, 0)` (the block below) is stored; if particles are "minimal": leave the loop; otherwise `fx = next_double()`, `fz = next_double()`, height = `max(top of S's collision shape at the point (fx, fz), fluid height of S)`; particle `smoke` if S's fluid is lava, if `S` is a magma block or a lit campfire, otherwise `rain`; position `S + (fx, height, fz)`, zero velocity.
4. Sound: if a position `S` was stored **and** `next_int(3) < counter` (then `counter += 1` in all cases where the test is evaluated), then `counter = 0` and rain sound (`sounds.md` § 5.7; `weather.rain.above` if `S.y > B.y + 1` and the height of `B`'s column > `⌊camera y⌋`, otherwise `weather.rain`), played at the centre of `S`, without distance delay.

### 2.2 Drips under leaves

Animation tick of the leaf blocks (`particles.md` § 4, usual animation draw): if "it rains here" at the block **above** the leaves (§ 1.2) and `next_int(15) = 1`, and the block below is not (occluding **and** with a full top face): particle `dripping_water` at `(x + next_double(), y − 0.05, z + next_double())`, zero velocity. The leaves' ambient sound is drawn afterwards (outside the weather).

### 2.3 Other effects

- Lightning rod: sparks during the thunderstorm, `particles.md` (table § 4).
- Server side only (for the record, nothing to do on the client): an empty cauldron in the rain fills with water (5 % per precipitation pass), in the snow with powder snow (10 %); fire goes out in the rain; farmland gets wet. The client only sees the block changes.

---

## 3. Thunderstorm

- **Lightning bolt**: entity `lightning_bolt`, drawing in `entities-objects.md` § 11. Its life logic also runs on the client: initial life 2, number of flashes `next_int(3) + 1` (entity's own generator, hence local); on each tick, if life = 2: sounds (below); then life −1; if life < 0: no flash left → the entity disappears, otherwise if `life < −next_int(10)`: one flash less, life = 1, new drawing seed; finally, if life ≥ 0, the world's lightning counter is set to 2.
- **Sounds** (client, on the first tick, if the entity is not silent), weather category, at the entity's position, without distance delay: `entity.lightning_bolt.thunder` volume 10,000, pitch `0.8 + next_float·0.2`; then `entity.lightning_bolt.impact` volume 2, pitch `0.5 + next_float·0.2`. Attenuation range per `sounds.md` § 3.3 (the huge volume makes the thunder audible from everywhere).
- **Sky flash**: lightning counter decremented on every client tick (even if the world is frozen); if it is > 0 and the option "hide lightning flashes" is disabled, sky colour drawn 22 % toward (0.8; 0.8; 1) and sky light factor forced to 1: `environment.md` § 1.5 point 5, `lighting.md`.
- **Darkening**: "thunder" modifiers of the attributes (`environment.md` § 1.5), second darkening of the background colour (§ 6), sky light (`sky_light_level`). Nothing else on the rendering side: no thunderstorm-specific columns or particles.

---

## 4. World border

### 4.1 Packets (play, server → client)

| Packet | Fields (order) | Effect on the client |
|---|---|---|
| `initialize_border` | `x` double, `z` double, old size double, new size double, duration varlong (ticks), absolute max size varint, warning blocks varint, warning time varint (ticks) | centre ← (x, z); if duration > 0: transition (old → new, duration) otherwise fixed size = new; then max size, warning blocks and time |
| `set_border_center` | `x` double, `z` double | centre ← (x, z) |
| `set_border_lerp_size` | old size double, new size double, duration varlong (ticks) | transition (or fixed size if old = new) |
| `set_border_size` | size double | fixed size |
| `set_border_warning_delay` | time varint (ticks) | warning time |
| `set_border_warning_distance` | blocks varint | warning blocks |

Durations are in **ticks** (the server sends, at initialisation, the remaining ticks of the current transition and its target size). Initial state of a client world before any packet: centre (0, 0), size 59,999,968 (float 5.999997e7), absolute max size 29,999,984, 5 blocks and 15 ticks of warning.

### 4.2 Bounds and transition

- **Fixed size** `s`: `x_min = clamp(cx − s/2, −M, M)`, `x_max = clamp(cx + s/2, −M, M)`, same in z (`M` = absolute max size). Recomputed when the centre or `M` change. "Fixed" state.
- **Transition** from `a` to `b` over `N` ticks: counter `k = N`; `size = a + (b − a)·(N − k)/N` (or `b` if the progress reaches 1); `previous = size` at the start. On each client tick (world not frozen, before the rest of the world tick): `k −= 1`, `previous = size`, `size` recomputed; if `k ≤ 0` → fixed size `b`. "Growing" state if `b > a`, "shrinking" if `b < a`.
- Interpolated bounds (rendering): `x_min(pt) = clamp(cx − lerp(pt, previous, size)/2, −M, M)`, etc.
- **"Current" bounds** (collision, distance, interactions, vignette) = interpolated bounds with `pt = 0`, hence computed on the **previous** size during a transition (one tick behind, vanilla).
- Transition speed = `|a − b| / N` blocks per tick (0 when fixed). Target = `b` (size when fixed).
- Distance to the border of a point `(x, z)` = `min(x − x_min, x_max − x, z − z_min, z_max − z)` (current bounds); negative outside.
- "Within bounds" `(x, z)` with margin `m`: `x_min − m ≤ x < x_max + m` and the same in z. For a block: its integer coordinates.

### 4.3 Wall rendering

**Preparation (each frame)** — `D` = effective render distance × 16 (blocks; `option-menus.md`), `C` = camera position, bounds = interpolated bounds (§ 4.2):
- Visible only if the camera is not more than `D` inside all the faces (`C.x < x_max − D` and `C.x > x_min + D` and `C.z < z_max − D` and `C.z > z_min + D` → invisible) and not more than `D` outside a face (`C.x < x_min − D` or `C.x > x_max + D` or the same in z → invisible).
- `alpha = clamp((1 − distance(C)/D)⁴, 0, 1)`, distance taken on the **current** bounds (§ 4.2). Outside, the distance is negative → alpha 1.
- Tint depending on the state: growing `#40FF80`, shrinking `#FF3030`, fixed `#20A0FF`.

**Geometry** (16 vertices, 4 quads, positions relative to a stored origin `(Ox, Oz)`), rebuilt only if the interpolated bounds have changed since the last build, or if the camera changed chunk section (repositioning of the render area):
- `bMinX = x_min + 0.01`, `bMaxX = x_max − 0.01`, same in z (wall detached by 0.01 toward the inside).
- `z0 = max(⌊C.z − D⌋, bMinZ)`, `z1 = min(⌈C.z + D⌉, bMaxZ)`; `x0`, `x1` likewise in x. `Ox = x0`, `Oz = z0`.
- `H = far_plane` (far plane, `hand-camera.md`); `v0 = −frac(C.y·0.5)`, `v1 = v0 + H`.
- `ux0 = (⌊x0⌋ & 1)·0.5`, `ux1 = (x1 − x0)/2`; `uz0 = (⌊z0⌋ & 1)·0.5`, `uz1 = (z1 − z0)/2` (two blocks per horizontal repeat, parity aligned on the world).
- Quad 0 (south, z = `bMaxZ`): `(0, −H, bMaxZ−z0 ; ux0, v1)`, `(x1−x0, −H, bMaxZ−z0 ; ux1+ux0, v1)`, `(x1−x0, H, bMaxZ−z0 ; ux1+ux0, v0)`, `(0, H, bMaxZ−z0 ; ux0, v0)`.
- Quad 1 (west, x = 0 local): `(0, −H, 0 ; uz0, v1)`, `(0, −H, z1−z0 ; uz1+uz0, v1)`, `(0, H, z1−z0 ; uz1+uz0, v0)`, `(0, H, 0 ; uz0, v0)`.
- Quad 2 (north, z = 0 local): `(x1−x0, −H, 0 ; ux0, v1)`, `(0, −H, 0 ; ux1+ux0, v1)`, `(0, H, 0 ; ux1+ux0, v0)`, `(x1−x0, H, 0 ; ux0, v0)`.
- Quad 3 (east, x = `bMaxX`): `(bMaxX−x0, −H, z1−z0 ; uz0, v1)`, `(bMaxX−x0, −H, 0 ; uz1+uz0, v1)`, `(bMaxX−x0, H, 0 ; uz1+uz0, v0)`, `(bMaxX−x0, H, z1−z0 ; uz0, v0)`.

**Drawing**:
- Model offset `(Ox − C.x, −C.y, Oz − C.z)`: in y the vertices are thus **absolute**, the wall covers `y ∈ [−H, H]` of the world (not centred on the camera); the vertical texture offset `v0` is only reread on rebuild (small vertical jump of the pattern at each rebuild, vanilla). One vertical repeat = 2 blocks.
- Texture matrix: translation `(o, o)` with `o = (wall clock in ms mod 3000)/3000`: diagonal scrolling of one texture every 3 s, **real time**, independent of the game.
- Uniform colour `(r, g, b, alpha)` of the tint. Fragment: texel; discard if `texel.a = 0`; colour = `texel × (r, g, b, alpha)`. No fog, no lightmap.
- Texture `textures/misc/forcefield.png` (16 × 16), with mipmaps (4 levels under the base image, "auto" strategy, `models.md` § Mipmaps), linear minification with mipmaps, nearest-neighbour magnification, repeat.
- Weighted additive blend: `rgb = src.rgb·src.a + dst.rgb`; alpha = `src.a`. No back-face culling. "Closer or equal" depth test, depth **write** enabled, depth offset (slope 3, constant 3). OIT variant: no depth write, additive mode.
- Faces drawn: the four directions sorted by increasing distance from the camera to the interpolated bounds (north `C.z − z_min`, south `z_max − C.z`, west `C.x − x_min`, east `x_max − C.x`), only those at distance < `D`. Quad number = south 0, west 1, north 2, east 3.
- Order in the frame: right after the weather (`sky.md` § 1); in OIT, before the weather in each stage.

### 4.4 Warning vignette

`hud.md` § 5.3 gives the formula (red = brightness × (1 − w), green and blue raised). Details:
- Distance `d` = distance from the camera entity (tick position, not interpolated) to the current bounds (§ 4.2).
- Threshold `S = max(warning blocks, min(speed × warning time, |target − size|))`: speed in blocks/tick and time in ticks, `size` = current size of the transition.
- `w = 1 − d/S` if `d < S`, clamped to [0, 1] (hence 1 outside). Nothing if the "vignette" option is disabled: the whole vignette is not drawn, warning included.

### 4.5 Collision

Complements `movement.md` (collisions, list of shapes):
- The border takes part in the collisions of an entity with box `A` if `d < 2·m` **and** the entity is within the bounds with margin `m`, where `m = max(max(|X width of A|, |Z width of A|), 1)` and `d` = distance from the entity's position to the current bounds.
- Shape added: all of space **except** the slab `[⌊x_min⌋, ⌈x_max⌉] × ]−∞, +∞[ × [⌊z_min⌋, ⌈z_max⌉]` (current bounds rounded outward to whole blocks). With a fixed size the shape is computed once; during a transition, on each request.
- It also counts in the "the pose fits" test (`movement.md`, poses).
- The collision shapes of neighbouring solid entities only count if their box is within the bounds (min and max − 1e−5 corners).

### 4.6 Aiming and actions beyond the border

- The crosshair ray cast **ignores** the border: a block or entity beyond it is aimed at and highlighted normally (`targeting.md`).
- Starting to break a block outside the bounds (block's integer coordinates): refused locally, no packet. In creative, continuing the destruction also tests the bounds.
- Using an item on a block whose aimed position is outside the bounds: immediate failure, no packet, no arm swing, and the right click stops there (neither use of the item in the void, nor attempt with the other hand). Only the clicked block's position is tested, not the one where the block would be placed.
- Interacting with an entity (right click) whose feet block is outside the bounds: nothing. The attack is not filtered on the client side.
- Simulated projectiles (arrows, etc.): their ray cast stops on the border if they start from the inside (impact point brought back within the bounds, `max − 1e−5`, face = dominant direction of the movement).
- Damage outside the border: server only.

---

## 5. Constants

| Element | Value |
|---|---|
| Weather radius | option 3–10, default 10 |
| Column seed | `(x²·3121 + x·45238971) XOR (z²·418711 + z·13761)` (32 bits) |
| Tick offset (rain) | `(x²·3121 + x·45238971 + z²·418711 + z·13761) & 255` |
| Rain speed | `3 + next_float`, scrolling `/32` |
| Snow | `±time·0.01` in u, `time·0.001` in v, descent `/512` |
| Max alpha rain / snow / edge | 1 / 0.8 / 0.5 (× rain level) |
| Vertical UV | 0.25 per block |
| Snow light | `(n·3 + 15) div 4` |
| Ground drops | `0.225·(2R+1)²·p²`, seed `t·312987231`, ±10 blocks |
| Snow threshold | temperature < 0.15; `sea + 17`, slope `0.05/40` per block, noise ×8 at `/8` |
| Border, colours | growing `#40FF80`, shrinking `#FF3030`, fixed `#20A0FF` |
| Border, alpha | `(1 − d/D)⁴` |
| Border, inset / scrolling | 0.01 block / 3000 ms |
| Border, texture | `misc/forcefield.png` 16 × 16, 2 blocks per repeat |
| Border, client default | size 5.999997e7, max 29,999,984, 5 blocks, 15 ticks |

---

## Uncertain points

- **"Current" bounds one tick behind** during a transition (§ 4.2): read as is (interpolation at `pt = 0` on previous / current size); the server does the same computation, so collisions are consistent. To be confirmed in game by a capture during a `/worldborder set` with a duration.
- **Wall geometry frozen between two rebuilds**: the extent `[x0, x1]` depends on the camera at the time of the rebuild, which only happens on a section or bounds change. Walking along a fixed wall one changes section every 16 blocks, so the extent (± `D`) remains sufficient; not verified on screen.
- **Wall absolute in y** (`[−far_plane, +far_plane]` of the world): read that way (the model offset in y is `−C.y`, without an origin). To be checked when flying very high (above `far_plane`, e.g. render distance 2 and minimal cloud range).
- **Camera block's column invisible** (NaN in the orientation table): deduced from the reading, not yet seen in game (one has to be in the rain in the open, looking up).
- **Biome read** for the column precipitation and the temperature: block position (`x`, `cy`, `z`) passed as is to the client world's biome read; I assume the same "at the block" read as `environment.md` (with the biome zoom blur), without having retraced it here.
- **Evolution in 26.3** (§ 1.9): the radius tied to the graphics mode and the hard-coded "blocks motion" property come from memories of earlier versions, not from a reread of their code.
- **Login sequence** (1 then 7 then 8 ×rain): read in the vanilla server's sending of world information; Paper may differ.
- "auto" mipmap strategy of `forcefield.png` (partial transparency): referred to `models.md` without checking that the computation is identical outside an atlas.
