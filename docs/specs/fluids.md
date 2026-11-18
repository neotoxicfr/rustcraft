# Fluid rendering (water, lava) in sections

Behaviour specification, clean room. Target version: Java Edition 26.3 (protocol 777).
Out of scope: server-side flow (only the reading of the received state is described here).

Conventions: "current block" = position P of the fluid being drawn. Vertex coordinates are relative to the
section (x, y, z = P & 15, as floats). Directions: north = −Z, south = +Z, west = −X, east = +X.
All heights and constants are 32-bit floats unless stated otherwise.

---

## 0. Input data on the world side

### 0.1 Fluid state of a block state
- Block `minecraft:water` / `minecraft:lava`, property `level` (0..15):
  - `level` = 0 → source fluid, amount 8, not falling.
  - `level` = 1..7 → flowing fluid, amount = 8 − `level`, not falling.
  - `level` ≥ 8 → flowing fluid, amount 8, **falling** (all values 8..15 give the same state).
- Block with `waterlogged=true` → source water (amount 8). `waterlogged=false` → no fluid.
- Always-submerged blocks: `minecraft:kelp`, `minecraft:kelp_plant`, `minecraft:seagrass`,
  `minecraft:tall_seagrass`, `minecraft:bubble_column` → source water.
- Any other block → empty fluid (nothing to draw).

Hard-coded on the client side (block state → fluid state table); derivable from block names and properties.

### 0.2 "Same fluid"
Source water and flowing water are the same fluid; likewise source lava / flowing lava. Water and lava are different.

### 0.3 Own height
Own height = amount / 9.0 (float). Source or falling: 8/9 = 0.8888889.

### 0.4 Height seen by the renderer, H(Q) for a position Q and the fluid F of the current block
1. If Q contains the same fluid as F:
   - if the position above Q also contains the same fluid → 1.0;
   - otherwise → own height of the fluid at Q.
2. Otherwise (other fluid or no fluid):
   - if the block at Q is "solid" in the historical sense (see 0.5) → −1.0 (value ignored in the averages);
   - otherwise → 0.0.

### 0.5 "Historically solid" flag of a block state
Computed once per block state:
1. Some blocks force it to true, others to false (per-block setting, **data to extract**, not listed here).
2. Otherwise: if the collision shape is empty → false.
3. Otherwise, let B be the bounding box of the collision: if (size X + size Y + size Z) / 3 ≥ 0.7291666666666666 (double) → true.
4. Otherwise: true if the Y size of B ≥ 1.0, false otherwise.

**To verify / to extract**: the list of blocks with forced true/false. Without it, steps 2–4 give the right result for the vast majority of blocks.

---

## 1. When and in which layer

1. When a section is compiled, every non-air position of the section is walked. If the block's fluid
   state (§0.1) is not empty, the fluid is meshed **before** the block's own model (a
   waterlogged block thus produces the water quads and then those of the block).
2. The fluid is meshed only if at least one of its six faces passes the first filter (§3.1).
3. Section layer: derived from the transparency of the fluid's textures (still, flowing, overlay if present):
   presence of a partial-alpha pixel → translucent; otherwise presence of an alpha-0 pixel → cutout; otherwise → opaque.
   With the standard textures: **water → translucent layer**, **lava → opaque (solid) layer**.
   For a simple implementation: water = translucent, lava = opaque, hard-coded.

### Textures (block atlas ids)
| Fluid | Still | Flowing | Overlay | Tint |
|---|---|---|---|---|
| water (source and flowing) | `minecraft:block/water_still` | `minecraft:block/water_flow` | `minecraft:block/water_overlay` | biome water colour |
| lava (source and flowing) | `minecraft:block/lava_still` | `minecraft:block/lava_flow` | none | none (white) |

The textures are animated (resource animation files); the animation is handled by the atlas, not by the fluid renderer.

---

## 2. Height of the 4 corners of the surface

1. Read the 6 direct neighbours (block state + fluid state): below, above, north, south, west, east.
2. h_self = H(P) (§0.4).
3. If h_self ≥ 1.0 (same fluid above): the 4 corners are 1.0.
4. Otherwise: h_N = H(north), h_S = H(south), h_E = H(east), h_W = H(west), then:
   - corner NE = average(h_self, side1 = h_E, side2 = h_N, diagonal = P + north + east)
   - corner NW = average(h_self, h_W, h_N, P + north + west)
   - corner SE = average(h_self, h_E, h_S, P + south + east)
   - corner SW = average(h_self, h_W, h_S, P + south + west)

### Average of a corner
1. If side1 ≥ 1.0 or side2 ≥ 1.0 → 1.0.
2. Accumulators sum = 0, weight = 0.
3. If side1 > 0 or side2 > 0: compute h_diag = H(diagonal) (with its own check of the block above).
   If h_diag ≥ 1.0 → return 1.0. Otherwise add h_diag.
   (If neither side has fluid, the diagonal is not consulted.)
4. Add h_self, then side1, then side2.
5. Return = sum / weight.

Rule "add a height h":
- h ≥ 0.8 → sum += h × 10.0, weight += 10.0;
- 0.0 ≤ h < 0.8 → sum += h, weight += 1;
- h < 0 (solid block, −1.0) → ignored.

Since h_self > 0 always, the weight is never zero. Notable cap constant: 0.8888889 (8/9).

---

## 3. Faces drawn

### 3.1 First filter (before the corner computation)
Notation "face hidden by a state S in direction D with height h" (§3.4).

- **Top**: candidate if the block above does NOT contain the same fluid. (No test on the current block's shape.)
- **Bottom**: candidate if
  (a) the block below does not contain the same fluid, AND
  (b) the bottom face is not hidden by the current block itself (§3.3), AND
  (c) the face is not hidden by the block below, direction down, height 0.8888889.
- **North / south / west / east**: candidate if the neighbour in that direction does not contain the same fluid AND
  the face is not hidden by the current block itself (§3.3).

If none of the six is a candidate → nothing is produced.

### 3.2 Second filter (after the corner computation)
- **Top**: drawn if candidate AND not hidden by the block above, direction up, with
  h = min of the 4 corner heights (before the 0.001 subtraction).
- **Bottom**: no extra test.
- **Sides**: drawn if candidate AND not hidden by the neighbour, direction of the face, with
  h = max of the two corner heights of that side (values AFTER the possible 0.001 subtraction of §4.1).

### 3.3 Hiding by the current block (waterlogged blocks)
The fluid's face in direction D is removed if the occlusion shape of the current block, on its face D,
entirely covers the unit square (test §3.4 with height 1.0 in the opposite direction, applied to the current
block). E.g.: waterlogged stairs → the face on the stairs' full back side is not drawn.

### 3.4 Test "face hidden by a state S, direction D, height h"
Let O = the occlusion shape of S on its face opposite to D (the face of S turned toward the fluid).
A state's occlusion shape is empty if the block does not occlude (glass, leaves… according to the
already-extracted "can occlude" data), otherwise it is its occlusion shape; cut by face.
1. O empty → not hidden.
2. O = full cube: hidden, EXCEPT if D = up and h ≠ 1.0 exactly (a water surface under a full block
   stays visible as long as it does not reach 1.0).
3. Otherwise: fluid shape = box (0,0,0)–(1,h,1). The face is hidden if the slice of this box adjoining
   the common boundary is entirely covered by the slice of O adjoining that boundary
   (tolerance 1.0E-7 on the coincidence with the boundary). Consequence: in direction up with h < 1,
   never hidden; on a side, hidden if the rectangle [0..1] × [0..h] is covered by the face of O.

---

## 4. Geometry and UVs

Constants: anti-flicker offset ε = 0.001; bottom_y = 0.001 if the bottom face is drawn, otherwise 0.0.
The UVs "sprite(a, b)" denote u = u0 + (u1 − u0) × a, v = v0 + (v1 − v0) × b, where (u0, v0, u1, v1) are
the exact bounds of the image in the atlas (no edge shrinking, bounds = pixels of the sprite
excluding the padding margin).

All vertices: normal (0, 1, 0), no damage overlay texture.

### 4.1 Top (if §3.2 allows it)
1. Subtract 0.001 from the 4 corner heights (these lowered values are then also used for the sides).
   If the top is not drawn, the corners keep their full value for the sides.
2. Flow vector f = flow of the fluid at P (§4.4).
3. If f.x == 0 and f.z == 0 (zero flow): **still** texture, UVs:
   NW = (u0, v0), SW = (u0, v1), SE = (u1, v1), NE = (u1, v0).
4. Otherwise: **flowing** texture, angle a = atan2(f.z, f.x) − π/2 (angle converted to float),
   s = sin(a) × 0.25, c = cos(a) × 0.25 (sine/cosine from a 65536-entry table, index = (long)(a × 10430.378350470453) & 65535,
   cos = same table with +16384 before the mask), then:
   - NW: sprite(0.5 + (−c − s), 0.5 + (−c + s))
   - SW: sprite(0.5 + (−c + s), 0.5 + (c + s))
   - SE: sprite(0.5 + (c + s),  0.5 + (c − s))
   - NE: sprite(0.5 + (c − s),  0.5 + (−c − s))
   The official atan2 is a fast approximation; a standard atan2 gives an invisible difference (**to verify** if bit-for-bit identity is wanted).
5. Quad, in this order: (x, y+h_NW, z), (x, y+h_SW, z+1), (x+1, y+h_SE, z+1), (x+1, y+h_NE, z).
6. Colour = tint × "up" shading (§5). Light = §6 with P.
7. **Back face**: the same quad is additionally added in reverse order (vertices 0, 3, 2, 1) if, among the 9
   positions of the layer above P (P+up offset by −1..1 in X and in Z), at least one does not contain
   the same fluid AND does not have a "solid-rendered" block state (occlusion = full cube). This is used to see the
   surface from below the water.

### 4.2 Bottom (if candidate)
**Still** texture, quad at y + bottom_y:
(x, z) uv (u0, v0) → (x+1, z) uv (u1, v0) → (x+1, z+1) uv (u1, v1) → (x, z+1) uv (u0, v1).
No back face. Colour = tint × "down" shading. Light = §6 with P−1 (below).

### 4.3 Sides (walk order: north, south, west, east)
For each side: (h0, h1) = heights of the two corners, and the two vertical edges:
| Side | h0 / h1 | vertex 0 (x, z) | vertex 1 (x, z) |
|---|---|---|---|
| north | NW / NE | (x, z+ε) | (x+1, z+ε) |
| south | SE / SW | (x+1, z+1−ε) | (x, z+1−ε) |
| west | SW / NW | (x+ε, z+1) | (x+ε, z) |
| east | NE / SE | (x+1−ε, z) | (x+1−ε, z+1) |

1. Texture: **flowing**, unless the fluid has an overlay texture (water only) AND the
   neighbouring block on that side is a "semi-transparent" block or leaves → **overlay** texture.
   Semi-transparent blocks: `minecraft:glass`, the 16 `minecraft:*_stained_glass`, `minecraft:tinted_glass`,
   `minecraft:ice`, `minecraft:frosted_ice`, `minecraft:blue_ice`, `minecraft:slime_block`, `minecraft:honey_block`,
   all the copper grates (`minecraft:*copper_grate`, including oxidised and waxed ones);
   leaves: all `minecraft:*_leaves` blocks (including azalea). Hard-coded (list to be kept up to date).
   Glass panes (`*_glass_pane`) and `minecraft:packed_ice` are NOT part of it.
2. UVs: u_a = sprite_u(0.0), u_b = sprite_u(0.5); v_0 = sprite_v((1 − h0) × 0.5), v_1 = sprite_v((1 − h1) × 0.5),
   v_bottom = sprite_v(0.5). (Only the top-left half of the texture is used.)
3. Quad: (s0, y+h0) uv (u_a, v_0) → (s1, y+h1) uv (u_b, v_1) → (s1, y+bottom_y) uv (u_b, v_bottom) → (s0, y+bottom_y) uv (u_a, v_bottom).
4. Back face (order 0, 3, 2, 1) added **always**, except when the overlay texture is used.
5. Colour = tint × (up shading × side shading), side shading = that of **north** for the
   north/south faces, that of **west** for the west/east faces. Light = §6 with P.

### 4.4 Flow vector (read from the received state, no simulation)
For the fluid F at P, of own height hp (§0.3):
1. fx = fz = 0 (double).
2. For each horizontal neighbour V (north, south, west, east), if V's fluid is empty or the same as F:
   - hv = own height of V's fluid (0 if empty); d = 0;
   - if hv == 0: if V's block does not have the tag `minecraft:blocks_fluid_flow` (tag received from the server) and
     the fluid under V is empty or the same, with own height hb > 0 → d = hp − (hb − 0.8888889);
   - otherwise if hv > 0 → d = hp − hv;
   - if d ≠ 0: fx += stepX(V) × d, fz += stepZ(V) × d.
3. f = (fx, 0, fz).
4. If the fluid at P is falling: for each horizontal direction V in the order north, south, west, east,
   if V's face (or the face of the block above V) is "solid for the fluid" in direction V:
   f = normalise(f) + (0, −6, 0), then stop the loop.
   "Solid for the fluid": false if the position contains the same fluid; false if it is ice
   (`minecraft:ice`, `minecraft:frosted_ice`); otherwise true if the block's face in direction V is "sturdy"
   (full face of the support shape).
5. Return = normalise(f). Normalise: if the length < 1.0E-5 → zero vector.
Only the X and Z components are used for rendering (the "zero" test of §4.1 applies to f.x and f.z after normalisation).

---

## 5. Tint, shading, anti-flicker

- Base tint: water → biome water colour, averaged (option "biome blend", radius r default 2,
  0..7): integer average (truncated division) of R, G, B over the square (2r+1)² centred on P in X/Z at the same Y;
  alpha forced to 255. If r = 0: biome colour at P with no average. The water colour comes from the
  `water_color` field of the biome effects, **received from the server** in the biome registry.
  Lava → opaque white (0xFFFFFFFF), no tint.
- Shading per face (depends on the `cardinal_light` field of the dimension type, **received from the server**, default "default"):
  | type | down | up | north | south | west | east |
  |---|---|---|---|---|---|---|
  | default | 0.5 | 1.0 | 0.8 | 0.8 | 0.6 | 0.6 |
  | nether | 0.9 | 0.9 | 0.8 | 0.8 | 0.6 | 0.6 |
  Final colour of a channel = (int)(channel × factor) clamped to 0..255, alpha kept.
  Effective factors: top = up; bottom = down; north/south = up × north; west/east = up × west.
- Anti-flicker: top corners −0.001; sides moved in by 0.001 toward the inside; bottom raised by 0.001
  when the bottom face is drawn (and the sides then go down to that same height).
- No ambient occlusion on fluids: a single colour and a single light per quad.

---

## 6. Light (principle, to be implemented later)

- One light value per face, identical for its 4 vertices.
- Value at a position Q = combined (block, sky) light of Q; if the state at Q has the "emissive rendering" emission
  → full light; otherwise the block light is raised to at least the light emission of the state at Q
  (lava emits 15, so its faces are fully lit on the block light side).
- Light of a face = maximum, component by component (block and sky separately), between the value at Q and at Q+up.
  Top and sides: Q = P. Bottom: Q = P−1 (so max between below and P).

---

## Uncertain points
- List of blocks with a forced "historically solid" flag (§0.5): to be extracted as data.
- Official approximate atan2 vs standard atan2 (§4.1): visually negligible difference, not measured.
- `water_color` received as a raw integer (and not as a hexadecimal string) could arrive without alpha; without biome
  blending (r = 0) the alpha would then be the one received. Forcing alpha = 255 on the client side carries no visual risk.
