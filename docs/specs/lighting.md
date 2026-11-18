# Client-side terrain lighting (26.3)

Sources: reading of the official 26.3 client (kept outside the repository), notes in our own words. Complements `chunks.md` (chunk
packet format), `environment.md` (`visual/*` attributes and camera probe), `models.md` (directional
shading), `fluids.md` (§ 6, fluid light). Unverified points are marked **[not verified]**.

Notation: `B(p)` block light stored at p (0–15), `S(p)` sky light stored (0–15),
`D(state)` dampening ("light dampening", 0–15), `E(state)` emission (0–15).

---

## 1. Light data

### 1.1 Format received

Same data block in the chunk packet (after the block entities, see `chunks.md` § 5) and in the
"light update" packet. The update packet alone: `x` VarInt, `z` VarInt, then the block.

| Order | Field | Encoding |
|---|---|---|
| 1 | sky mask present | bit set: VarInt length in bytes, bytes (bit 0 = low bit of byte 0) |
| 2 | block mask present | same |
| 3 | sky mask empty | same |
| 4 | block mask empty | same |
| 5 | sky arrays | VarInt count, then each VarInt length (≤ 2048) + bytes |
| 6 | block arrays | same |

- Bit index `i` ↔ section `y = section_min − 1 + i`, for `i ∈ [0, sections + 2)`: one section below the
  world and one above.
- The arrays are in increasing order of the "present" bits of their layer.
- For each index: "present" bit → the section receives the array; otherwise "empty" bit → section
  filled with 0; otherwise nothing (the section keeps what it has). "Present" wins over "empty".
- 2048-byte array, 4 bits per block: index `n = y·256 + z·16 + x` (local coordinates),
  byte `n >> 1`, low nibble if `n` is even, high if odd.

### 1.2 Application on the client side

- The received data is not applied immediately: each packet queues a task. **On every
  frame** (before the render extraction), the client runs `k` tasks of the queue, with
  `k = max(10, size/10)` if `size < 1000`, otherwise the whole queue; then it runs the light engine
  (§ 1.4): block layer then sky layer.
- "Chunk packet" task: set the light data (without requesting a remesh), then declare
  each section non-empty / empty to the engine (§ 1.3), then mark for remeshing all the sections of the
  columns `x−1..x+1`, `z−1..z+1`.
- "Light update" task: set the data; for each section cited (present or empty),
  mark the section and its neighbours for remeshing.
- In both cases, the column is then declared "light-enabled" (§ 1.3).
- Chunk forget: column disabled, pending data erased, all sections declared empty.

### 1.3 Storage

- A section **carries light data** only if it is non-empty (at least one non-air block) or
  adjacent (3×3×3 cube, diagonals included) to a non-empty section. Data received for a section that
  does not carry any stays pending; it will be used when the section starts to carry some.
- Reading `B(p)`: 0 if the section does not carry data.
- Reading `S(p)` (dimension with sky):
  - if the section carries data → the stored value;
  - otherwise, go up section by section until the first one that carries data, and read its **bottom
    layer** (local y 0) at the same (x, z);
  - if there is none above (or if the column has no data) → 15.
- Dimension without sky (`has_skylight` false): no sky engine, `S = 0` everywhere.
- "Light-enabled" column: on activation, on the sky side, the sections that carry data but are still
  empty (no array, value 0) located entirely above the highest `sourceTop` of the 256
  columns of the chunk (§ 1.4) are filled with 15. An inactive column: emission ignored by the engine.
- Reading during meshing: the "visible" copy is read, replaced at the end of each engine pass.

### 1.4 Client light engine (local recomputation)

The client **recomputes** the light around each block change it applies (block packet,
multi-block packet, local prediction). It does no initial chunk computation: the base comes from the server.

**Trigger.** When a block changes state and the two states have different light
properties — different `D`, or different `E`, or one of the two uses its shape for light
occlusion — the client (1) updates the height of the column's sky sources, (2) adds the position
to the "to check" set. The computation is done at the next engine pass (same frame if the packet
is processed before).

**Properties per state** (to extract from the game, § 6):

| Property | Default rule |
|---|---|
| `E` emission | given per block/state |
| solid render | the occlusion shape covers the whole cube |
| `D` dampening | solid render → 15; otherwise lets the sky through → 0; otherwise 1 (overridden by some blocks) |
| passage opacity | `max(1, D)` |
| face shape (direction d) | empty if the state cannot occlude or does not use its shape for light; otherwise 2D projection onto face d of the cells of the occlusion shape that touch this face (empty if none) |
| block outside a loaded chunk | treated as bedrock (solid render, `D = 15`) |

**Occlusion between two blocks** (from `a` to `b` in direction d): blocked if the union of the face shape
of `a` on side d and the face shape of `b` on the opposite side covers the **whole** unit square (a full face
on one side is enough). Two empty faces do not block.

**Target stable rule** (the engine converges to this fixed point; this is what has to be reproduced):

- Block: `B(p) = max(E'(p), max over the 6 non-occluded neighbours n of (B(n) − max(1, D(p))))`, clamped to 0,
  where `E'(p) = E(p)` if the column has active light, 0 otherwise.
- Sky: `S(p) = 15` if `p.y ≥ sourceTop(x, z)`; otherwise `max over the 6 non-occluded neighbours n of
  (S(n) − max(1, D(p)))`, clamped to 0. No "lossless downward" rule outside the source column:
  under a leaf (D = 1) the sky is 14, then 13 below, etc.
- Only positions in sections that carry data are written; a position outside the data propagates
  nothing.

**Sky source height** `sourceTop(x, z)`: the smallest y such that all the positions from y
up to the top are sources (value 15). Computation: go down the column from the top of the highest
non-empty section; for each pair (upper, lower) of consecutive blocks, the edge is **occluded** if
`D(lower) ≠ 0` or if the bottom face of `upper` and the top face of `lower` occlude (rule above). The
first occluded edge met gives `sourceTop = y(upper)`. No occluded edge → −∞ (the whole column is a
source). After a change at y: only the edges (y+1, y) and (y, y−1) are re-examined; an edge that became
occluded above the old height raises it; if the edge that fixed the height no longer is, the
descent resumes from there.

**Algorithm (exact game order)**, two FIFO queues per layer ("decrease", "increase") of entries
`(position, level, allowed directions, flags)`:

1. For each position to check (order of a hashed set, no effect on the final result):
   - Block: if `E'(p) < B(p)` → `B(p) = 0`, decrease entry (old level, 6 directions); otherwise "suction"
     decrease entry (level 1, 6 directions). Then if `E'(p) > 0` → "emission" increase entry
     (level `E'`, 6 directions).
   - Sky: first recomputation of the column's sources (if light-enabled): positions under the new
     height that were 15 → 0 with a decrease entry (top to bottom, stop at the first non-15);
     positions from the height up to the top of the data that are not 15 → 15 (stop at the first 15),
     with an increase entry if `y` is the height or if `y` is under the height of one of the 4
     horizontal neighbours. Then: if `p` is a source → decrease (15, except upward) + increase (15, except
     upward); otherwise like the block (0 + decrease of the old level, or suction).
2. Empty the whole decrease queue: for each entry of level `v` and each allowed direction toward `q`
   (in data, `L(q) ≠ 0`): if `L(q) ≤ v − 1` → `L(q) = 0`, decrease entry (old `L(q)`, except
   return) and, for the block, emission increase if `E'(q) > 0`; otherwise → increase from `q` limited to the
   return direction (level `L(q)`).
3. Empty the whole increase queue: an entry is only processed if the stored level still equals the entry's
   level (an emission entry first raises the stored one if it is lower). For each allowed
   direction toward `q`: `new = L(p) − max(1, D(q))`; if `new > L(q)` and no occlusion `p→q` →
   `L(q) = new`, and if `new > 1` increase entry from `q` (all directions except return).
4. Sky only: when a write crosses a horizontal chunk boundary from a position at
   local y 0 under which the sections do not carry data, the same value (increase) or 0 (decrease)
   is copied over the whole height of the sections carrying data located under it, in the arrival column,
   down to the bottom of those empty sections.
5. Publish the visible copy; mark for remeshing, for each written position, its section and the
   neighbouring sections it touches (a position on a section edge touches up to 8 sections).

**Order relative to meshing**: packets (blocks placed, sections marked) → start of frame: light queue
then engine → sections marked by the light → extraction and meshing. Meshing thus reads the
up-to-date light when the packet was processed before the frame. The server then sends back its own data
(§ 1.2), which overwrites the cited sections and requests a remesh again.

---

## 2. Per-vertex light at meshing

### 2.1 Light value of a position ("light coordinates")

For a position `p` read with a state `s`:

1. if `s` has **emissive rendering** (magma block; active sculk sensor / calibrated sculk sensor) → `(block 15, sky 15)`;
2. otherwise `(B(p), S(p))`, with the block component raised to `E(s)` if it is lower.

"Packed" integer encoding: `block·16 + (sky·16) << 16`. The vertices then carry two 16-bit integers
`u = smoothed_block` (0–240) and `v = smoothed_sky` (0–240); without smoothing they are multiples of 16.

### 2.2 Path choice

Smoothing + ambient occlusion (AO) if: option "ambient occlusion" enabled (**default: yes**) AND
`E(block state) = 0` AND the first piece of the model has `ambientocclusion` true (inherited from the nearest
parent that defines it, default true; false for `cross`, `cross_emissive`, torches, etc.). Otherwise flat path.
The choice holds for the whole block (all its quads).

### 2.3 Geometry of a quad

Vertex order of a face of direction d (imposed at baking if the element has no rotation of its own;
`x0/x1` = min/max of the quad's bounding box, same for y, z):

| Face | vertex 0 | vertex 1 | vertex 2 | vertex 3 |
|---|---|---|---|---|
| down (y0) | x0, z1 | x0, z0 | x1, z0 | x1, z1 |
| up (y1) | x0, z0 | x0, z1 | x1, z1 | x1, z0 |
| north (z0) | x1, y1 | x1, y0 | x0, y0 | x0, y1 |
| south (z1) | x0, y1 | x0, y0 | x1, y0 | x1, y1 |
| west (x0) | y1, z0 | y0, z0 | y0, z1 | y1, z1 |
| east (x1) | y1, z1 | y0, z1 | y0, z0 | y1, z0 |

Triangle split **always** `(0,1,2)` then `(2,3,0)`: **no** diagonal flip depending on the
AO.

Per-quad indicators (coordinates in block units, ε = 0.0001):
- **partial**: the box does not cover the whole square of the face (`min ≥ ε` or `max ≤ 1 − ε` on one of the
  two axes of the plane);
- **flush** ("cubic"): the quad is flat on the normal axis AND (it is on the cube's edge on side d —
  coordinate `< ε` for down/north/west, `> 1 − ε` for up/south/east — OR the block's collision shape is
  a full cube).

### 2.4 AO path

Notation: `P` block position, `d` quad direction, `base = P + d` if flush, otherwise `P`.
The four lateral directions `c0..c3` of each face:

| Face | c0 | c1 | c2 | c3 |
|---|---|---|---|---|
| down | west | east | north | south |
| up | east | west | north | south |
| north | up | down | east | west |
| south | west | east | down | up |
| west | up | down | north | south |
| east | down | up | north | south |

Reads (each position with **its own state** for § 2.1 and for the shade):

1. Sides `i = 0..3`: `Li = lum(base + ci)`, `Oi = shade(base + ci)`.
2. Permeability `Pi` = the state at `base + ci + d` (one layer **further** than `base`) lets the
   light through: `not solid render OR D = 0`.
3. Diagonals, for the pairs `(0,2) (0,3) (1,2) (1,3)`: if `Pi` and `Pj` are both false →
   `Lij = L0`, `Oij = O0` (**always side c0**, even for pairs 1-2 and 1-3: a quirk of the game,
   to keep); otherwise `Lij = lum(base + ci + cj)`, `Oij = shade(base + ci + cj)`.
4. Light centre: `Lc = lum(P)` with the block's state; then if flush OR the state at `P + d` is not solid
   render → `Lc = lum(P + d)`.
5. Shade centre: `Oc = shade(base)`.

Occlusion shade `shade(q)` ("shade brightness"): 0.2 if the state's collision shape is a full cube,
otherwise 1.0; exceptions: mud and soul sand 0.2; snow layer 0.2 if 8 layers, otherwise 1.0;
barrier, light block, structure void, "transparent" blocks (glass & co) 1.0.

Values at the 4 corners of the face:

| Corner | sides | light | shade |
|---|---|---|---|
| A | c0, c3 | `mix(L3, L0, L03, Lc)` | `(O3 + O0 + O03 + Oc) / 4` |
| B | c0, c2 | `mix(L2, L0, L02, Lc)` | `(O2 + O0 + O02 + Oc) / 4` |
| C | c1, c2 | `mix(L2, L1, L12, Lc)` | `(O2 + O1 + O12 + Oc) / 4` |
| D | c1, c3 | `mix(L3, L1, L13, Lc)` | `(O3 + O1 + O13 + Oc) / 4` |

`mix(n1, n2, n3, c)` on packed values:
- if `sky(c) > 2` or `block(c) > 2`: for each `nk` — if it is exactly 0 (block 0 and sky 0, typical case
  of an opaque neighbour) it is replaced by `c`; otherwise if its sky is 0, it takes the sky of `c`
  (block kept);
- otherwise no replacement;
- result per component: `(n1 + n2 + n3 + c) >> 2` on the ×16 values, i.e. an integer 0–240
  (truncation, no rounding).

Assignment to the vertices:
- **Non-partial quad**: each vertex receives the corner whose two sides point toward it (e.g. down face,
  vertex 0 at (x0, z1) → west-south corner = A). Light = corner value; colour =
  `grey(floor(shade × 255))`.
- **Partial quad**: bilinear interpolation of the 4 corners at the position (bounding box, table § 2.3) of the
  vertex. For a corner with sides (s, t), weight = `fs · ft` with `fs = u` if s is the positive side of the axis
  (east, up, south) and `1 − u` otherwise, `u` being the vertex coordinate on that axis (0–1, not clamped).
  - vertex shade = `clamp(Σ weight × cornerShade, 0, 1)` then `grey(floor(×255))`;
  - light: sky = `(int)(Σ weight × cornerSky)`, block = `(int)(Σ weight × cornerBlock)` (truncation, on
    0–240).
- Then **directional shading**: each channel × face factor (table in `models.md`, depending on the dimension's
  `cardinal_light`; direction = element's shading override otherwise `d`), integer truncation,
  clamped 0–255.
- Then **tint** (biome, etc.): `channel × tint / 255` in integers (truncation).

Final vertex colour = `grey(AO) × directionalShading × tint`; the AO is **never** in the
lightmap, only in the colour.

### 2.5 Flat path

- Colour of the 4 vertices = `grey(floor(directionalFactor × 255))`, then tint.
- Single light for the quad, read with **the block's own state** (its emission and its emissive rendering, not
  those of the neighbour):
  - quad tied to a cullable face (cull face d): position `P + d`, computed once per direction;
  - free quad: `P + d` if it is flush (§ 2.3), otherwise `P`.
- So an emissive block (`E > 0`) always has at least `E` block light on all its faces, and the
  magma block is in full light (15, 15).

### 2.6 Emission per model element

An element may carry `light_emission` (0–15, e.g. `cross_emissive`). When writing the vertex:
if `e > 0`, `block = max(floor(smoothed_block/16), e)`, `sky = max(floor(smoothed_sky/16), e)`, then
repacking in multiples of 16 (the smoothed value is thus requantised). If `e = 0`, nothing changes.

---

## 3. Fluids

According to `fluids.md` § 6 (verified): a single light per face, component-by-component `max` of
`lum(Q)` and `lum(Q + up)` (reading § 2.1 with the state at each position); `Q = P` for the top and
sides, `Q = P − 1` for the bottom. No AO; colour = tint × directional shading.

---

## 4. Lightmap

### 4.1 Texture and sampling

- 16×16 RGBA 8-bit **non-sRGB** texture, initially white. Column `i` = block level,
  row `j` = sky level. Computed on the GPU by a full-screen triangle; each texel has
  `block = i/15`, `sky = j/15`.
- Recomputed only when the "to update" flag is set, i.e. **once per client
  tick** (20 Hz), on the first frame that follows; the interpolated factors use the tick progress of
  that frame.
- Terrain shader, **per vertex**: `coord = clamp((u, v)/256 + 1/32, 1/32, 31/32)` then **bilinear**
  read, clamped edges. A smoothed value (e.g. block 200) falls between two texels:
  linear interpolation between levels `floor(200/16)` and the next. Multiples of 16 fall
  exactly at the centre of a texel.
- `vertexColour = colour(§ 2) × lightmap(coord)` then interpolated over the triangle, multiplied by the
  texture then the fog. No sRGB ↔ linear conversion anywhere.

### 4.2 Formula of a texel

```
curve(l)    = l / (4 − 3·l)
bf          = curve(block) × blockFactor
sf          = curve(sky) × skyFactor
c           = max(ambientColor, nightVisionColor × nightVisionIntensity)   (per channel)
c          += skyColor × sf
tintB       = mix(blockTint, (1,1,1), 0.9 × (2·block − 1)²)
c          += tintB × bf
c           = mix(c, c × (0.7; 0.6; 0.6), bossDarkening)
c          −= darknessScale                                      (same value on the 3 channels)
c           = clamp(c, 0, 1)
m           = max(c.r, c.g, c.b)
c'          = c × (1 − (1 − m)⁴) / m                             (if m = 0: division 0/0, see § 7)
c           = mix(c, c', brightness)
```

### 4.3 Inputs (computed on the CPU at each update)

| Input | Value |
|---|---|
| skyFactor | attribute `sky_light_factor` (camera probe); in the End, `+ flashIntensity` (or `+ intensity/3` if a boss bar creates the world fog), except with the option "hide lightning flashes" |
| skyColor | attribute `sky_light_color` (RGB / 255) |
| blockTint | attribute `block_light_tint` |
| ambientColor | attribute `ambient_light_color` |
| nightVisionColor | attribute `night_vision_color` |
| blockFactor | `1.4 + flicker` |
| nightVisionIntensity | night vision effect: 1 if more than 200 ticks remain, otherwise `0.7 + 0.3·sin((duration − progress)·π·0.2)`; otherwise, if "conduit" effect and underwater vision > 0: the underwater vision (see `environment.md`); otherwise 0 |
| brightness | `max(0, setting − dkn)` with "brightness" setting (**default 0.5**), `dkn = darknessBlend × optionScale` |
| darknessScale | `max(0, cos((ageTicks − progress)·π·0.025) × 0.45 × dkn) × optionScale` |
| bossDarkening | `lerp(progress, previous, current)`; each tick: +0.05 (max 1) if a boss bar darkens the screen, otherwise −0.0125 while > 0 (only if the world is running normally) |

- `optionScale` = option "darkness effect" (default 1; the slider stores the square of its position).
- `darknessBlend`: fade factor of the darkness effect, interpolated; each tick it advances by
  `±1/22` toward 1 if the effect is active (and does not end within 22 ticks), toward 0 otherwise.
- Flicker (per tick): `s += (r1 − r2)·r3·r4·0.1` then `s ×= 0.9`, `rk` uniform floats [0, 1)
  from a generator of the client's own (exact value impossible to reproduce, only the law matters).
- Thunderstorm lightning: goes through the attribute (`sky_light_factor = 1` during the lightning counter,
  `environment.md` § 1.5); nothing else in the lightmap.
- End flash (dimensions with "end" sky): default clock of the dimension `t`; per 600-tick
  period, seed `t / 600` → Java LCG generator: one float discarded, then offset ∈ [0, 200], duration ∈
  [100, min(380, 600 − offset)] (integers, bounds included), then two angles (sound); intensity =
  `sin((t mod 600 − offset)·π/duration)` if `offset ≤ t mod 600 ≤ offset + duration`, otherwise 0;
  interpolated between ticks.

### 4.4 Useful default values (Overworld by day, no effect)

`skyFactor = 1`, `skyColor = white`, `blockTint = #FFD88C`, `ambientColor = #0A0A0A` (dimension
type), `blockFactor ≈ 1.4`, brightness 0.5. Example: sky 15, block 0 → `c = 0.039 + 1 = 1` →
white. Sky 0, block 0 → `0.039` then brightness correction: `m = 0.039`, `c' = 1 − 0.961⁴ ≈ 0.147`,
mix at 0.5 → ≈ 0.093 per channel.

---

## 5. Dimension

| Element | Effect |
|---|---|
| `has_skylight` false (Nether) | no sky layer: `S = 0` everywhere, at meshing as in the lightmap |
| `ambient_light` (type's float) | **not used** by the lightmap nor the terrain in 26.3 (only serves the interface vignette and AI computations: `lerp(ambient, curve(n/15), 1)`); replaced by the attribute `ambient_light_color` |
| `ambient_light_color` | Overworld `#0A0A0A`, Nether `#302821`, End `#3F473F` |
| `sky_light_factor` | Nether 0, End 0 (the End keeps stored sky at 15 but with no contribution, except flashes) |
| `cardinal_light` | directional shading table (`default` / `nether`), see `models.md` |
| "end" sky | enables the flashes (§ 4.3) |

---

## 6. Data to extract from the game (per block state)

| Data | Use | Current extractor |
|---|---|---|
| emission `E` | § 1.4, § 2.1, AO choice | yes (`light_emission`) |
| dampening `D` | § 1.4, sky sources | yes (`light_dampening`) |
| solid render | AO permeability, AO centre | yes (flag `SOLID_RENDER`) |
| lets the sky through | check | yes (`SKYLIGHT_THROUGH`) |
| can occlude / shape for light occlusion | face shapes | yes (`CAN_OCCLUDE`, `SHAPE_LIGHT_OCCLUSION`) |
| occlusion shape | face shapes (per-direction projection to compute) | yes (`occlusion`) |
| full-cube collision | default AO shade, "flush" quad | yes (`FULL_COLLISION`, computed without a world) |
| **emissive rendering** | § 2.1 | **no** — to add (magma block, active sculk sensors) |
| **occlusion shade** (0.2 / 1.0) | § 2.4 | **no** — to add (derivable from `FULL_COLLISION` except the 7 listed exceptions) |
| `ambientocclusion`, element `light_emission`, shading override | § 2.2, § 2.6 | in the JSON models (resources), not in the extractor |

---

## 7. Uncertain points

- Lightmap with `m = 0` (all black after darkness): the shader divides 0 by 0; result dependent on the
  GPU (in practice black). Keep 0.
- Blocks with a dynamic shape (no shape cache) compute the shade and "flush" with the real world;
  the extractor evaluates them empty. Rare cases, not catalogued.
- Transient engine values: the iteration order of the "to check" set does not change the fixed
  point, but the game's engine is not proven exact in all decrease cases; the stable rule of
  § 1.4 is the target, to be validated on captures (placing/breaking a torch, a block under the sky).
- Exact implementation of the generator's "integer between bounds" draw (End flash): to be taken from the
  generator spec if we write it.
