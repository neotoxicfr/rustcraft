# Per-entity rendering — non-living entities and projectiles (26.3)

Clean-room specification, written from a reading of the decompiled 26.3 client (protocol 777). No line of
source code; formulas, constants, resource paths and layer names only. Completes
`entities.md`, part "mission C" (referred to below as "MC", e.g. "MC § 4.1" = ground item): the model
conventions (pose, ZYX order, reset before animation), the name tag
(MC § 1.8) and the shadow (MC § 1.9) are not repeated here.

Conventions specific to this document:
- `pt` = fraction of a tick (0..1); `A` = age in ticks + pt; "yaw" and "pitch" = the entity's yaw and pitch, in
  degrees, interpolated by lerpAngle(pt, old, current) (yaw) and lerp (pitch) unless stated otherwise.
- Transformation stack written from left to right in the order of application on the stack (each
  operation post-multiplies): T = translation, S = scale, RX/RY/RZ = rotation around the axis, in degrees
  unless marked "rad". The starting point is the entity's interpolated position minus the camera, plus
  the kind's own render offset when there is one (frame, "new behaviour" minecart).
- Unlike living entities, **none** of these kinds applies the sequence "S(−1, −1, 1) then
  T(0, −1.501, 0)" of MC § 1.2 except where explicitly written.
- Light: (block, sky) levels at the block of the entity's "light probe" — the interpolated eye
  position (interpolated position + the type's eye height), not the feet; block = 15 if the entity
  is burning. "Full light" = raw coordinates (block 240, sky 240), i.e. levels 15/15. When a
  kind forces "block = 15", the sky stays that of the world.
- Render types: "cutout" = entity_cutout (alpha test 0.1, no back-face culling),
  "cutout cull" = with back-face culling, "solid" = entity_solid, "translucent" =
  entity_translucent (alpha blending), unless specified.
- "White" overlay: 16×16 overlay texture; rows 0‑7 = red
  (0xB2FF0000, "hurt" tint); rows 8‑15 = white of alpha floor((1 − u/15 × 0.75) × 255). The
  fragment becomes mix(overlay_colour, colour, overlay_alpha). For u = 15, row 10:
  alpha 63 → about 75 % white.
- Shadow: radius 0 (no shadow) when nothing is indicated.
- All the entities of this document also draw, after their own geometry and outside their stack,
  what the base rendering of any entity draws: the leashes they hold or are attached by
  (not specified here) and the name tag (MC § 1.8, "non-living" rule). Exceptions noted.

Metadata indices: Entity 0 to 7 (reminder), then the continuation specific to each inheritance chain, given
in each section. Order established from the order of **declaration of the identifiers** in each class
(that is what fixes the number, not the order of the default values).

---

## 1. Boats and rafts

### 1.1 Kinds, layers, textures

Woods: `oak`, `spruce`, `birch`, `jungle`, `acacia`, `cherry`, `dark_oak`, `pale_oak`, `mangrove`,
`poplar` (new in 26.3) and `bamboo` (raft).

| Kind | Layer | Texture |
|---|---|---|
| `<wood>_boat` | `minecraft:boat/<wood>#main` (128×64) | `textures/entity/boat/<wood>.png` |
| `<wood>_chest_boat` | `minecraft:chest_boat/<wood>#main` (128×128) | `textures/entity/chest_boat/<wood>.png` |
| `bamboo_raft` | `minecraft:boat/bamboo#main` (128×64) | `textures/entity/boat/bamboo.png` |
| `bamboo_chest_raft` | `minecraft:chest_boat/bamboo#main` (128×128) | `textures/entity/chest_boat/bamboo.png` |

Rule: texture = `textures/entity/` + layer path without "#main" + `.png`. The 22 layers and the
22 textures exist. The layer `minecraft:boat#water_patch` (a single `water_patch` part) is used by
all non-raft boats.

Parts: boat = `bottom`, `back`, `front`, `right`, `left`, `left_paddle`, `right_paddle`;
raft = `bottom` (two cuboids), `left_paddle`, `right_paddle`; chest versions = same parts +
`chest_bottom`, `chest_lid`, `chest_lock` (static, no animation).

### 1.2 Metadata

| Index | Type | Meaning |
|---|---|---|
| 8 | integer | "hurt" time, default 0 |
| 9 | integer | hurt direction, default 1 (±1) |
| 10 | float | accumulated damage, default 0 |
| 11 | boolean | left paddle active |
| 12 | boolean | right paddle active |
| 13 | integer | bubble column time |

(8 to 10 are common to all vehicles: boats and minecarts.)

Local evolution per tick (client included): if hurt > 0 → hurt − 1; if damage > 0 → damage − 1.

### 1.3 Transformation stack

Shadow: radius 0.8.

1. T(0, 0.375, 0).
2. RY(180 − yaw), yaw = lerpAngle(pt, previous yaw, yaw).
3. Hurt shake: h = hurt − pt; d = max(damage − pt, 0); if h > 0: RX(sin(h) × h × d / 10 ×
   direction) (sin in radians of h, result in degrees).
4. Bubble column tilt: if the entity is not "underwater" (eyes in water) and the
   angle b ≠ 0 (compared up to epsilon): rotation by the **non-normalised** quaternion (x, y, z,
   w) = (sin(b′/2), 0, sin(b′/2), cos(b′/2)) with b′ = b in radians. Converted to a matrix by the usual homogeneous formula,
   it is equivalent to a rotation of angle 2·atan(√2 · tan(b′/2)) ≈ b′√2 around the axis
   (1, 0, 1)/√2, multiplied by the uniform scale 1 + sin²(b′/2) (≤ 0.8 % for b ≤ 10°). To be reproduced
   as is for exactness; the difference is imperceptible.
5. S(−1, −1, 1).
6. RY(90).
7. Model (cutout), entity's light, neutral overlay.
8. Non-raft boats, and only if not underwater: part `water_patch` drawn with the same stack
   in the "water mask" type (vertices position only, depth test and write): used to
   prevent the water surface from being drawn inside the hull.

Bubble angle b: every client tick, mult += 0.05 if bubble time (metadata 13) > 0, otherwise mult −= 0.1;
mult clamped to [0, 1]; b = 10 × sin(0.5 × ticks_lived) × mult (degrees); rendering: lerp(pt, previous b, b).

### 1.4 Paddle animation

For each side s (0 = left, 1 = right):
- Position p_s (radians): every tick, if the paddle is active (metadata 11/12 true AND the boat has a
  driver), p_s += π/8; otherwise p_s = 0.
- Time t_s = if active: p_s − π/8 + pt × π/8; otherwise 0.
- rot_x = lerp(clamp((sin(−t) + 1)/2, 0, 1); −π/3 → −π/12) (absolute value, replaces the pose).
- rot_y = lerp(clamp((sin(−t + 1) + 1)/2, 0, 1); −π/4 → π/4).
- Right side: rot_y = π − rot_y.
- rot_z stays that of the pose (0.19634955 rad = π/16).

At rest (t = 0): rot_x = −5π/24, rot_y = lerp(0.9207; −π/4 → π/4) ≈ 0.6609 (right: π − 0.6609).

---

## 2. Minecarts

### 2.1 Kinds, layers, content

All minecarts use the same texture `textures/entity/minecart/minecart.png` (64×32) and one
layer per kind, of identical geometry (parts `bottom`, `front`, `back`, `left`, `right`, no
animation).

| Kind | Layer | Block displayed by default | Default offset |
|---|---|---|---|
| `minecart` | `minecraft:minecart#main` | none (air) | 6 |
| `chest_minecart` | `minecraft:chest_minecart#main` | chest, facing north | 8 |
| `furnace_minecart` | `minecraft:furnace_minecart#main` | furnace, facing north, `lit` = metadata 13 | 6 |
| `hopper_minecart` | `minecraft:hopper_minecart#main` | hopper (default state) | 1 |
| `tnt_minecart` | `minecraft:tnt_minecart#main` | TNT | 6 |
| `spawner_minecart` | `minecraft:spawner_minecart#main` | spawner | 6 |
| `command_block_minecart` | `minecraft:command_block_minecart#main` | command block | 6 |

### 2.2 Metadata

| Index | Type | Meaning |
|---|---|---|
| 8, 9, 10 | integer, integer, float | hurt, direction, damage (as § 1.2) |
| 11 | optional block state | custom displayed block; absent → the kind's default block |
| 12 | integer | vertical offset of the displayed block, in pixels (default: table above) |
| 13 | boolean | (furnace) has fuel |
| 13, 14 | text, component | (command block) command, last output — no effect on rendering |

### 2.3 Transformation stack

Shadow: radius 0.7. The base rendering (leashes, name) is submitted **before** the geometry (no visible effect).

1. Small anti-flicker offset, specific to each entity (signed 64-bit integers, modular
   arithmetic): g = id × 493286711; g2 = g × g × 4392167121 + g × 98761;
   ox = ((((g2 >> 16) & 7) + 0.5) / 8 − 0.5) × 0.004; oy likewise with >> 20; oz likewise with >> 24
   (arithmetic shift). T(ox, oy, oz).
2. Orientation, depending on the world's movement behaviour:
   - **Old behaviour** (default; experimental flag `minecart_improvements` absent):
     - Rail probe (below) at the entity's interpolated point (x, y, z). If it finds a rail:
       c = point on the rail; f = probe offset by +0.3; b = probe offset by −0.3 (each replaced
       by c if it fails); T(c.x − x, (f.y + b.y)/2 − y, c.z − z); d = b − f; if |d| ≠ 0:
       d normalised, rot = atan2(d.z, d.x) in degrees, pitch = atan(d.y) × 73. Otherwise rot = yaw,
       pitch = the entity's pitch (interpolated).
     - Then T(0, 0.375, 0), RY(180 − rot), RZ(−pitch).
   - **New behaviour** (flag present): RY(yaw), RZ(−pitch), T(0, 0.375, 0). Yaw,
     pitch and position come from the step interpolation of the new behaviour when it is
     active (the interpolated position then replaces the entity's position through the render offset);
     otherwise non-interpolated angles of the entity. Detail of the steps: out of scope (network mission).
3. Hurt shake: identical to the boat (§ 1.3 step 3).
4. Displayed block (if it is not air) — stack pushed/popped: S(0.75), T(−0.5, (offset − 8)/16,
   0.5), RY(90), then block model (cube 0..1): see § 13.1 for rendering a block "as an item".
   TNT minecart: § 4.2.
5. S(−1, −1, 1), minecart model (cutout), entity's light, neutral overlay.

Rail probe (old behaviour), for a point (x, y, z):
- (xt, yt, zt) = integer parts; if the block at (xt, yt − 1, zt) carries the tag `rails`, yt −= 1.
- If the block at (xt, yt, zt) is not a rail: failure.
- Exits (e0, e1) according to the rail shape: north-south (N, S); east-west (W, E); ascending east (W-down, E);
  ascending west (W, E-down); ascending north (N, S-down); ascending south (N-down, S); south-east (S, E);
  south-west (S, W); north-west (N, W); north-east (N, E). N = (0, 0, −1), S = (0, 0, 1), W = (−1, 0, 0),
  E = (1, 0, 0), "-down" = same vector with y = −1.
- p0 = (xt + 0.5 + e0.x/2, yt + 0.0625 + e0.y/2, zt + 0.5 + e0.z/2), p1 likewise with e1;
  D = (p1.x − p0.x, 2 (p1.y − p0.y), p1.z − p0.z).
- Progress k: if D.x = 0: k = z − zt; otherwise if D.z = 0: k = x − xt; otherwise
  k = 2 ((x − p0.x) D.x + (z − p0.z) D.z).
- Point = p0 + D × k; then y += 1 if D.y < 0, y += 0.5 if D.y > 0.
- Probe offset by ε: same start; y = yt (+1 if ascending rail); u = (e1 − e0) horizontal normalised;
  x += u.x ε, z += u.z ε; if e0.y ≠ 0 and (floor(x) − xt, floor(z) − zt) = (e0.x, e0.z): y += e0.y;
  otherwise same test with e1; then normal probe at the resulting point.

### 2.4 Rendering of the displayed block, light

The block is drawn as an isolated block model (all faces, no ambient occlusion, entity light
combined by max with the block's own emission): § 13.1. The chest and other blocks with
special rendering use their special model (chest: normal chest model).

---

## 3. Item frames (`item_frame`, `glow_item_frame`)

### 3.1 Metadata

Chain: Entity → hanging entity (8 = direction, "direction" type 0 down, 1 up, 2 north, 3 south,
4 west, 5 east) → frame: 9 = carried item (item stack), 10 = rotation (integer 0..7).

### 3.2 Frame model

The frame is a **block model** taken from fictitious block states:
- `item_frame`: `blockstates/item_frame.json`, variant `map=false` → `block/item_frame`, `map=true`
  → `block/item_frame_map`;
- `glow_item_frame`: `blockstates/glow_item_frame.json` → `block/glow_item_frame` /
  `block/glow_item_frame_map`.
`map=true` when the carried item is a map whose data is known to the client.
The frame is not drawn if the entity is invisible (bit 0x20 of metadata 0).

### 3.3 Transformation stack

The common dispatcher first applies the frame's render offset: (sx × 0.3, −0.25, sz × 0.3)
(s = unit vector of the direction) — it serves to place the name tag; the geometry cancels it:
1. T(−render offset).
2. T(s × 0.46875).
3. Horizontal direction: RX(0) then RY(180 − direction_yaw) (direction_yaw: south 0, west 90,
   north 180, east 270). Vertical direction: RX(−90 × sign) then RY(180), sign = +1 for up, −1 for
   down.
4. Frame (if visible): pushed stack, T(−0.5, −0.5, −0.5), block model (§ 13.1) with a slight
   depth offset (like decals), entity's light; popped.
5. T(0, 0, 0.4375) — or T(0, 0, 0.5) if the entity is invisible.
6. Content:
   - **Map**: RZ((rotation mod 4) × 90), RZ(180), S(1/128), T(−64, −64, 0), T(0, 0, −1); quad
     from (0, 0) to (128, 128) at z = −0.01 (corner (0, 128) ↔ UV (0, 1), (128, 128) ↔ (1, 1), (128, 0) ↔ (1, 0),
     (0, 0) ↔ (0, 0)), dynamic map texture (128×128), "text" render type, white colour.
     Decorations: only those marked "visible in frame"; each: T(dx/2 + 64, dy/2 + 64,
     −0.02), RZ(rot × 22.5), S(4, 4, 3), T(−0.125, 0.125, 0), quad (−1, 1)…(1, −1) at z = −0.001 × n
     (n = rank of the decoration), sprite of the decorations atlas; optional name: width w (font),
     scale k = clamp(25 / w, 0, 6/9), T(dx/2 + 64 − w k / 2, dy/2 + 68, −0.025), S(k, k, −1),
     T(0, 0, 0.1), white text without shadow, normal mode, black background of alpha 0x80 (pass of order 1).
   - **Other non-empty item**: RZ(rotation × 45), S(0.5), item model in `fixed` display
     context.
7. Content light: normal frame = entity's light; glow frame = raw coordinates (block
   240, sky 240) for an item, (block 210, sky 240) for a map ("map slightly less bright").

Glow frame light: block level = max(5, actual level).

Name tag: only if the interface is not hidden, the frame is targeted by the crosshair and
the carried item has a custom name; text = the item's display name.

---

## 4. Primed TNT (`tnt`) and TNT minecart

### 4.1 Primed TNT

Metadata: 8 = fuse (integer, default 80), 9 = block state (default: TNT). Every client tick:
fuse −= 1 (local value); the entity disappears at 0.

Shadow: radius 0.5.

f = fuse − pt + 1.
- Swell: g = clamp(1 − f/10, 0, 1); g = g⁴; swell(f) = 0.3 g (only f < 10; otherwise 0).
- Flash: lit(f) = f ≥ 0 AND floor(f / 5) even.

Stack: T(0, 0.5, 0); if f < 10: S(1 + swell); RY(−90); T(−0.5, −0.5, 0.5); RY(90); block (§ 13.1).
Net equivalent: the block's cube from (−0.5, 0, −0.5) to (0.5, 1, 0.5) around the position, scaled
around its centre (0, 0.5, 0). If lit(f): white overlay u = 15, row 10
(≈ 75 % white), otherwise neutral.

### 4.2 TNT minecart

As § 2.3, with at step 4, before the block: f = (fuse ≥ 0 ? fuse − pt + 1 : −1); if −1 < f < 10:
T(−swell/2, 0, −swell/2), S(1 + swell); white overlay if lit(f) (§ 4.1).
The minecart's fuse is not metadata: entity event 70 sets it to 80 on the client side, then
−1 per tick; −1 = not primed.

---

## 5. Falling block (`falling_block`)

- Block state: "data" field of the spawn packet = block state identifier. Metadata 8 =
  start position (block position), used as the model variant seed.
- Not drawn if the world block at the entity's block position is exactly the same state
  (avoids the duplicate on landing), nor if the block's render shape is not "model".
- Shadow: radius 0.5.
- Stack: T(−0.5, 0, −0.5), then block drawn **as a terrain block** (same meshing as the
  chunk geometry, same ambient occlusion and directional shading) placed alone in a
  fictitious world: neighbours = air (all faces visible), block position = (x, box y_max, z) rounded,
  light read in the real world at that position, biome tints of the biome at that position, model
  random seed = start position.

---

## 6. Leash knot (`leash_knot`)

- Layer `minecraft:leash_knot#main` (32×32, part `knot`, 6×8×6 cuboid), texture
  `textures/entity/lead_knot/lead_knot.png`. No metadata of its own, no animation.
- Stack: S(−1, −1, 1), model (cutout). No shadow.
- The rope itself is drawn by the base rendering of the attached entity (out of scope).

---

## 7. End crystal (`end_crystal`)

### 7.1 Data

Metadata: 8 = beam target (optional block position), 9 = show the base (boolean,
default true). Time: integer counter randomly initialised (0..99999) on creation, +1 per tick;
A_c = counter + pt.

Layer `minecraft:end_crystal#main` (64×32): `outer_glass` (pose y = 24) → child `inner_glass`
(scale 0.875) → child `cube` (scale 0.765625); `base` (base 12×4×12). Texture
`textures/entity/end_crystal/end_crystal.png`. Cutout. Shadow: radius 0.5.

### 7.2 Stack and animation

Height: h = sin(0.2 A_c)/2 + 0.5; h = (h² + h) × 0.4; height(A_c) = h − 1.4.

Stack: S(2), T(0, −0.5, 0), model — **without** the flip (−1, −1, 1).

Animation (after reset):
- `base` visible iff metadata 9.
- a = 3 A_c (degrees).
- `outer_glass`: y += height × 16 / 2 (pixels); part rotation = Ry(a) · R(axis n, π/3).
- `inner_glass`: rotation = R(axis n, π/3) · Ry(a) (relative to the parent).
- `cube`: rotation = R(axis n, π/3) · Ry(a) (relative to the parent).
- n = (sin 45°, 0, sin 45°). "·" = composition, the right one applied first to the vertex.
  (The parts have a zero pose rotation; the computed rotation therefore replaces the pose. The engine
  converts it to ZYX angles — no effect on the result.)

### 7.3 Beam

If a target exists: Δ = centre of the target block − interpolated position of the crystal. After the model (entity's
stack, outside S(2)): T(Δ), then beam of vector v = (−Δx, −Δy + height(A_c), −Δz):
- T(0, 2, 0); RY_rad(−atan2(v.z, v.x) − π/2); RX_rad(−atan2(√(v.x² + v.z²), v.y) − π/2);
  L = |v|.
- v0 = −0.01 A_c; v1 = L/32 − 0.01 A_c.
- 8 quads (i = 1..8): (s_i, c_i) = (sin, cos)(2π i/8) × 0.75, (s_0, c_0) = (0, 0.75); u_i = i/8.
  Quad i: (0.2 s_{i−1}, 0.2 c_{i−1}, 0) opaque black colour UV (u_{i−1}, v0);
  (s_{i−1}, c_{i−1}, L) white UV (u_{i−1}, v1); (s_i, c_i, L) white UV (u_i, v1);
  (0.2 s_i, 0.2 c_i, 0) black UV (u_i, v0). Normal (0, −1, 0).
- Texture `textures/entity/end_crystal/end_crystal_beam.png`; "crystal beam" type:
  alpha cutout 0.1, no face culling, opaque, no overlay, entity's light.
- The crystal stays drawn outside the view if a target exists (the beam may be visible on its own).

---

## 8. "Item sprite" projectiles

### 8.1 Kinds and parameters

All draw an item model in `ground` context, turned to face the camera.

| Kind | Item (metadata 8, default) | Scale | Light |
|---|---|---|---|
| `snowball` | `snowball` | 1 | normal |
| `egg` | `egg` | 1 | normal |
| `ender_pearl` | `ender_pearl` | 1 | normal |
| `splash_potion` | `splash_potion` | 1 | normal |
| `lingering_potion` | `lingering_potion` | 1 | normal |
| `experience_bottle` | `experience_bottle` | 1 | normal |
| `eye_of_ender` | `ender_eye` | 1 | block = 15 |
| `fireball` (large, ghast) | `fire_charge` | 3 | block = 15 |
| `small_fireball` (blaze) | `fire_charge` | 0.75 | block = 15 |

Metadata 8 = displayed item stack (the server may replace it: coloured potion, etc.); the item model
resolves tints and variants as for a ground item (components of the stack).

Stack: S(scale), rotation = camera orientation (like the orb, MC § 4.2), item model
(`ground` display transform of the model). No shadow.

Hiding at the start: these kinds (thrown, fireballs, eye) are not drawn while their age
< 2 ticks AND distance² to the camera < 12.25 (avoids the sprite stuck to the thrower's screen).

### 8.2 Dragon fireball (`dragon_fireball`)

- Texture `textures/entity/enderdragon/dragon_fireball.png`, cutout; block light = 15.
- Stack: S(2), camera orientation, then a quad in the z = 0 plane from (−0.5, −0.25) to (0.5, 0.75),
  UVs like the orb: bottom-left (0, 1), bottom-right (1, 1), top-right (1, 0), top-left (0, 0); normal
  (0, 1, 0), white colour. With outline (glow): same quad in the outline type.
- No metadata of its own; no hiding at the start.

### 8.3 Firework rocket (`firework_rocket`)

Metadata: 8 = item stack (rocket), 9 = carrying entity (optional integer: elytra flight rocket),
10 = shot at an angle (boolean).
Stack: camera orientation; if shot at an angle: RZ(180), RY(180), RX(90); `ground` item model.
No shadow, normal light.

---

## 9. Projectiles with a model

### 9.1 Wither skull (`wither_skull`)

- Layer `minecraft:wither_skull#main` (64×64, part `head`, 8×8×8 cuboid UV (0, 35)).
- Texture: metadata 8 ("dangerous", boolean) true → `textures/entity/wither/wither_invulnerable.png`,
  otherwise `textures/entity/wither/wither.png`. Translucent type. Block light = 15.
- Stack: S(−1, −1, 1), model. Animation: `head` rot_y = yaw × π/180, rot_x = pitch × π/180
  (interpolated angles of the entity).

### 9.2 Shulker bullet (`shulker_bullet`)

- Layer `minecraft:shulker_bullet#main` (64×32, part `main`: three cuboids in a cross), texture
  `textures/entity/shulker/spark.png`. Block light = 15. No metadata of its own.
- Stack: T(0, 0.15, 0), RY(sin(0.1 A) × 180), RX(cos(0.1 A) × 180), RZ(sin(0.15 A) × 360),
  S(−0.5, −0.5, 0.5):
  1. model in cutout, white colour;
  2. S(1.5) (cumulative: 0.75), model in translucent, colour 0x26FFFFFF (alpha 38/255), pass of order 1
     (after the first): halo.

### 9.3 Llama spit (`llama_spit`)

- Layer `minecraft:llama_spit#main` (64×32, part `main`), texture
  `textures/entity/llama/llama_spit.png`, cutout.
- Stack: T(0, 0.15, 0), RY(yaw − 90), RZ(pitch), model (without flip).

### 9.4 Wind charge (`wind_charge`, `breeze_wind_charge`)

- Same rendering for both. Layer `minecraft:wind_charge#main` (64×32: `bone` → `wind_charge`
  (4×4×4 cube) and `wind` (rings)). Texture `textures/entity/projectiles/wind_charge.png`.
- "Breeze wind" type: sorted translucent, alpha cutout 0.1, no face culling, no
  overlay, **no directional shading**, with texture offset (u + d_u, v) where
  d_u = (0.03 A) mod 1.
- No transformation (model drawn as is at the position).
- Animation: `wind_charge` rot_y = −16 A × π/180; `wind` rot_y = 16 A × π/180 (absolute values: the
  pose rotation −π/4 of `wind` is overwritten).

### 9.5 Thrown trident (`trident`)

- Metadata: arrow (8 = flags, 9 = piercing level, 10 = in ground) then 11 = loyalty
  (byte), 12 = enchanted ("glint", boolean).
- Layer `minecraft:trident#main` (32×32: `pole` and children), texture
  `textures/entity/trident/trident.png`. **Solid** type; if metadata 12 true: solid + enchantment
  glint (glint pass on top).
- Stack: RY(yaw − 90), RZ(pitch + 90), model. No shake on impact (unlike the
  arrow, MC § 4.3). Visibility box enlarged by 1.5 in all directions.

---

## 10. Fishing bobber (`fishing_bobber`)

### 10.1 Data

- The spawn packet carries the owner player's identifier in its "data" field; without a known
  owner the entity is removed on the client side. Metadata: 8 = hooked entity (id + 1,
  0 = none), 9 = "biting" (boolean).
- Not drawn if the owner is unknown; never culled by the view frustum.

### 10.2 Bobber

Texture `textures/entity/fishing/fishing_hook.png`, cutout cull. Stack: S(0.5), camera orientation,
quad (−0.5, −0.5)…(0.5, 0.5) at z = 0, UV: (−0.5, −0.5) ↔ (0, 1), (0.5, −0.5) ↔ (1, 1),
(0.5, 0.5) ↔ (1, 0), (−0.5, 0.5) ↔ (0, 0); normal (0, 1, 0), white, entity's light.

### 10.3 Line

- Hand holding the rod: owner's main arm if the main hand holds a rod, otherwise
  the other; inv = +1 if it is the right arm, −1 otherwise.
- Swing: s = progress of the owner's hit animation (0..1, interpolated);
  σ = sin(√s × π).
- Hand point M:
  - Owner = local player in first-person view: fov = field of view of the options (integer);
    near plane at 0.05: height hp = tan(fov/2) × 0.05, width wp = hp × (window width/height); point =
    forward × 0.05 + up × hp × (−0.1) − left × wp × (inv × 0.525) (forward, up,
    left = unit axes of the camera); scaled × 960/fov; then Y rotation of σ × 0.5 rad
    and X rotation of −σ × 0.7 rad ("vector" rotations: Y: x′ = x cos + z sin, z′ = z cos − x sin;
    X: y′ = y cos + z sin, z′ = z cos − y sin); M = interpolated eye position + this vector.
  - Otherwise: θ = lerp(pt, previous body yaw, body yaw) in radians; e = owner's scale;
    d = inv × 0.35 e; f = 0.8 e; dy = −0.1875 if crouching, otherwise 0;
    M = interpolated eyes + (−cos θ × d − sin θ × f, dy − 0.45 e, −sin θ × d + cos θ × f).
- O = M − (interpolated bobber position + (0, 0.25, 0)).
- Curve (entity frame): P(a) = (O.x a, O.y (a² + a)/2 + 0.25, O.z a), a ∈ [0, 1].
- 16 segments a_i = i/16 → a_{i+1}: one line segment P(a_i)–P(a_{i+1}), each vertex having as
  normal the unit direction towards the other end; opaque black colour; "lines" type;
  thickness = line thickness adapted to the window.

---

## 11. Lightning bolt (`lightning_bolt`)

- Seed: 64-bit integer drawn at random on creation on the client side (not transmitted), renewed on each
  new "flash" (the entity's life logic, also run on the client side) — the drawing therefore changes
  on each flash; the rendering uses no other data.
- Generator: classic 48-bit linear congruential generator (the same as MC § 4.1), integer bounded
  by this generator's standard algorithm (next_int(n)).
- Trunk offsets: with a generator seeded G: for k = 7 → 0: X[k] = xo, Z[k] = zo;
  xo += next_int(11) − 5; zo += next_int(11) − 5. (xF, zF) = final values of (xo, zo).
- For each layer c = 0..3: generator reseeded to G; for each branch b = 0..2:
  trunk = (b = 0); k_start = 7 − b; k_end = trunk ? 0 : k_start − 2;
  (sx, sz) = (X[k_start] − xF, Z[k_start] − zF); for k = k_start → k_end (decreasing):
  (ex, ez) = (sx, sz); trunk: sx += next_int(11) − 5, sz += next_int(11) − 5; branch:
  sx += next_int(31) − 15, sz += next_int(31) − 15; r_top = r_bottom = 0.1 + 0.2 c; if trunk:
  r_top ×= 0.1 k + 1; r_bottom ×= 0.1 (k − 1) + 1.
  Four quads, each: (sx ± r_bottom, 16 k, sz ± r_bottom), (ex ± r_top, 16 (k+1), ez ± r_top),
  (ex ± r_top, 16 (k+1), ez ± r_top), (sx ± r_bottom, 16 k, sz ± r_bottom), with the signs
  ("right" side x, z; "left" side x, z): (−,−;+,−), (+,−;+,+), (+,+;−,+), (−,+;−,−) — the first two
  vertices use the "right" signs, the last two the "left" signs.
- Units: blocks; total height 128 blocks (8 segments of 16). Colour of each vertex
  (0.45, 0.45, 0.5, 0.3).
- "Lightning" type: position + colour, additive blending (source × alpha + destination), fog
  applied, default depth test. With order-independent transparency: additive variant.
- No name tag, no leash; never culled by the view frustum. Side effect outside entity rendering:
  while life ≥ 0, the client world receives a 2-tick "sky flash".

---

## 12. Ominous item spawner (`ominous_item_spawner`)

- Metadata 8 = item stack. Nothing is drawn if it is empty. No shadow.
- Stack: if A ≤ 50: S(A / 50); RY(wrap(A × 40)) (wrap = brought into [−180, 180)); then the copies
  of the ground item (MC § 4.1, "copy count" part, "random draw" and 3D/flat layouts, same
  seeds), `ground` context, **without** bobbing or −y_min translation, in full light.

---

## 13. Display entities (`block_display`, `item_display`, `text_display`)

### 13.1 Block "as an item" (common: minecarts, TNT, frames, block display)

Block model resolved from the state (fixed variant seed 42), drawn without a world: all faces,
no ambient occlusion, in the "item" type on the blocks atlas (same shader as held
items: directional lighting, overlay texture active, entity vertex format), in
alpha cutout 0.1 — or translucent if the model has translucent parts. Tints: those of the model
(default tint colours, no biome). Light = per-channel max (light supplied, block
emission); a block with "emissive rendering" is full light. Blocks with a special model (chest,
sign, head, banner…) are drawn by their special rendering with the same light. The block's cube
occupies 0..1 on each axis from the current origin.

### 13.2 Metadata

Common (display class):

| Index | Type | Default | Meaning |
|---|---|---|---|
| 8 | integer | 0 | delay before transformation interpolation starts (ticks) — always sent again |
| 9 | integer | 0 | transformation interpolation duration (ticks) |
| 10 | integer | 0 | position/rotation interpolation duration ("teleport", 0..59) |
| 11 | vector 3 | (0, 0, 0) | translation |
| 12 | vector 3 | (1, 1, 1) | scale |
| 13 | quaternion | identity | left rotation |
| 14 | quaternion | identity | right rotation |
| 15 | byte | 0 | billboard: 0 fixed, 1 vertical, 2 horizontal, 3 centre (out of range → 0) |
| 16 | integer | −1 | forced brightness (block × 16 + sky × 2²⁰, levels 0..15); −1 = none |
| 17 | float | 1 | view range |
| 18 | float | 0 | shadow radius |
| 19 | float | 1 | shadow strength |
| 20 | float | 0 | width (visibility box) |
| 21 | float | 0 | height (visibility box) |
| 22 | integer | −1 | forced glow colour (outline); −1 = team colour |

Block: 23 = block state (default air). Item: 23 = item stack (default empty), 24 = display
context (byte: 0 `none`, 1 `thirdperson_lefthand`, 2 `thirdperson_righthand`,
3 `firstperson_lefthand`, 4 `firstperson_righthand`, 5 `head`, 6 `gui`, 7 `ground`, 8 `fixed`,
9 `on_shelf`; default 0). Text: 23 = text (component, default empty), 24 = line width (integer,
default 200), 25 = background colour (ARGB, default 0x40000000), 26 = text opacity (signed byte,
default −1 = 255), 27 = flags (byte: 1 shadow, 2 see-through, 4 default background, 8 left-aligned,
16 right-aligned; neither 8 nor 16 = centred; 8 takes precedence over 16).

### 13.3 Render state and interpolation

- Transformation T = translation matrix · R_left · scale · R_right (in this order, the right one
  applied first to the vertex).
- "Render state" (transformation, billboard, brightness, shadow radius and strength, glow) and
  "sub-state" (block / item+context / text) are only rebuilt on the client **tick**, when a
  relevant metadata has changed since the previous tick (11 to 16, 18, 19 for the state; 23‑24 or
  23‑27 for the sub-state). As long as no state exists, nothing is drawn.
- Processing order on the tick: (1) if metadata 8 was received: start_tick = ticks_lived + metadata 8;
  (2) if metadata 9 was received: duration = metadata 9; (3) rebuild if needed.
- Progress: if duration ≤ 0: 1; otherwise q = clamp((ticks_lived − start_tick + pt) / duration, 0, 1). The
  last value computed at render time is remembered (q_mem).
- Rebuild with duration ≠ 0 and an existing state: new interpolator from "current value of the
  old state at q_mem" to "new metadata": translation and scale linear,
  rotations in spherical interpolation, shadow radius/strength linear; at q ≥ 1: exact target value.
  Otherwise (duration 0 or first state): constant values. Billboard, brightness and glow
  are never interpolated.
- Text: opacity interpolated by integer (a + floor(q × (b − a))) **on the signed byte values**
  (−1 for 255: from 255 to 64 the value goes through −1, 0, … 64, which gives a jump); background
  interpolated channel by channel ARGB the same way (unsigned). Text, line width and flags:
  immediate.
- A "received" metadata is enough (same value as before): the server sends 8 again every time.
- Spherical interpolation: cos = dot product of the two quaternions; if 1 − |cos| > 1e‑6,
  ω = atan2(√(1 − cos²), |cos|) and weights sin((1 − q)ω)/sin ω, sin(qω)/sin ω; otherwise weights 1 − q and q.
  The second weight changes sign if cos < 0 (shortest path). No normalisation of the result.
- The received quaternions are not normalised; their matrix is the homogeneous form
  (w² + x² − y² − z², …): a quaternion of norm n rotates and scales by n².
- Entity position/rotation: linear interpolation of the packets over metadata 10 ticks (0 = immediate).

### 13.4 Visibility, light, shadow

- Visibility box: if width = 0 or height = 0 → never culled by the view frustum; otherwise
  box (x ± w/2, y..y + h, z ± w/2), shifted by the difference interpolated position − position.
- Distance: drawn if distance² < (range × 64 × entity distance factor of the options)².
- Light: if forced brightness ≠ −1 → block and sky levels extracted from metadata 16 (replace the
  two); otherwise normal light.
- Shadow: radius and strength of the render state (interpolated); no state → no shadow.
- Glow ("glowing" effect): colour = metadata 22 if ≠ −1, otherwise team colour.

### 13.5 Common stack

Base rendering (name, leashes) first, then (if there is a state AND a sub-state):
1. Billboard orientation, "YXZ" quaternion R = Ry(−ψ) · Rx(θ) (angles in radians):
   - fixed: ψ = entity yaw, θ = entity pitch (interpolated);
   - horizontal: ψ = entity yaw, θ = −camera pitch;
   - vertical: ψ = camera yaw − 180, θ = entity pitch;
   - centre: ψ = camera yaw − 180, θ = −camera pitch.
2. Multiplication by the transformation T(q).
3. Sub-state (below), entity's light (§ 13.4).

### 13.6 Block display

Block (metadata 23) drawn by § 13.1 from the origin (the cube's corner is on the entity's
position, not its centre). Neutral overlay.

### 13.7 Item display

If the stack is not empty: RY_rad(π), then item model with the display transform of the
chosen context (metadata 24; `none` = no display transform).

### 13.8 Text display

- Splitting: the text is cut into lines of width ≤ line width (metadata 24, in font pixels)
  by the font's line-wrapping algorithm; w_i = width of each line; W = max w_i
  (0 without lines). Result cached until the next rebuild of the sub-state.
- Frame: RY_rad(π) then S(−0.025, −0.025, −0.025) — i.e. net (0.025, −0.025, 0.025) — then
  T(1 − W/2, −H, 0) with H = 10 n − 1 (n lines, line height 9 + line spacing 1).
- Background: if flag 4 → black of alpha floor(background opacity of the options × 255) (option "background for
  chat only" active, which is the default: 0.25 → 63); otherwise interpolated background colour (metadata 25).
  If the colour ≠ 0: rectangle from (−1, −1) to (W, H).
- Lines: line i at y = 10 i; x = 0 (left), W − w_i (right), W/2 − w_i/2 (centre); ARGB colour
  = (opacity << 24) | 0xFFFFFF (the component's colours keep this alpha); drop shadow if
  flag 1 (offset by one pixel, RGB colour × 0.25).
- Mode: flag 2 → "see-through" (no depth test, like pass 1 of MC § 1.8);
  otherwise "polygon offset" (normal text with a forward depth bias).
- The text shader rejects fragments of alpha < 0.1: an opacity ≤ 25 makes the text
  invisible (the background stays).
- Light: entity's light (§ 13.4) for the text and the background; the "see-through" mode uses
  neither the lightmap nor fog (colour only).
- Empty text: no line (n = 0, H = −1); the background is then a rectangle of zero height, invisible.
  A text ending with a line break keeps a last empty line.
- Depths (font pixels, towards the front of the text): background z = −0.01; with shadow, shadow at z = 0 and
  text at z = 0.03; without shadow, text at z = 0. Back faces culled (text, shadow and background): a frozen
  display can only be read from the +z side of its frame.
- Interpolation (duration ≠ 0 and an old text state): opacity = a + floor(p × (b − a)) on the **signed**
  bytes (−1 = opaque) then truncated to the byte; background = same formula channel by channel (A, R, G, B).
- Shadow of an explicit component colour ("shadow_color"): alpha = that of the shadow colour if
  the text is opaque, otherwise the product of the two alphas.

---

## 13 bis. Cushion (`cushion`)

- Metadata 8: dye colour (identifier 0‑15, white by default). Position received as is (no
  client recomputation), placed on top of the supporting block.
- Layer `minecraft:cushion#main` (one 16 × 4 × 16 px cuboid, deformation −0.005, 64 × 64 texture),
  texture `entity/cushion/<colour>_cushion`, cutout.
- Orientation: horizontal direction of the yaw (not interpolated) = quarter turn
  floor(yaw / 90 + 0.5) & 3 (0 south, 1 west, 2 north, 3 east), angle = index × 90°.
- Stack: RY(180 − angle), RX(180), T(0, −0.25, 0) — the cuboid occupies y ∈ [0, 0.25] above the
  position. Entity's light, no shadow.

---

## 14. Entities without rendering of their own

`interaction` (metadata 8 width, 9 height, 10 response), `marker` (no metadata) and `area_effect_cloud`
(metadata 8 radius, 9 waiting, 10 particle) use the empty rendering: no geometry, no shadow.
Only the base rendering of any entity remains: leashes and **name tag** (MC § 1.8, non-living
rule: custom name with "name always visible", or custom name targeted by the crosshair) — a named
interaction therefore displays its name. The effect cloud only produces particles (tick
logic, outside entity rendering).

---

## Uncertain points

1. **Boat water mask**: the shader writes a colour equal to the colour modulator with an alpha
   blend; the expected effect (depth only, no visible colour) assumes a zero alpha modulator
   or a write mask — not followed through to the end. To be checked visually (no
   veil in the hull).
2. **End crystal base**: with S(2), T(0, −0.5, 0) and without flip, the base (y from 0 to
   4 px) would fall between −1 and −0.5 block below the entity's position; to be confirmed visually (the
   base must rest on the supporting block).
3. **Map decorations in a frame**: the list of decorations "visible in frame" depends on the decoration
   type (registry `map_decoration_type`, field "show in frame"); the dynamic map texture
   and the decorations come from the map packets (network/interface mission).
4. **New minecart behaviour** (experimental flag): only the stack is described; the interpolation steps
   come from the minecart movement packet (network mission).
5. **Full light of the map in a glow frame**: raw coordinates (block 210, sky 240) noted;
   the fractional level 13.125 depends on the sampling of the light table.
6. **Falling block**: rendering "as terrain" from the structure of the fictitious world; the exact shading
   (smooth ambient occlusion depending on the option) follows the terrain mission.
7. **Lightning, depth**: the "default" depth state (test + write) is assumed from the
   name; not verified.
8. **Leash rope** (drawn by the held entity) and **sky flash**: out of scope, to be
   specified elsewhere.

## 13. Client-side movement (entities simulated on both sides)

The server only resends the position of small entities every 10 to 20 ticks (items,
orbs and falling blocks 20, TNT 10, projectiles 10 or 20) and their velocity when it changes;
the official client makes them advance by itself on every tick with the same physics, otherwise they
stay frozen at the spawn point then jump to the arrival point. Order of the client tick:
old values = current, optional interpolation, then the tick of its own described here. The
position packets (without interpolation for items, falling blocks, TNT) and velocity packets
simply replace the simulated state. Code: `rc_physics::body`, `rc-app/src/entity_motion.rs`,
`rc-app/src/projectile_sim.rs`. Game float constants: taken in single precision then
widened (0.98 becomes 0.98000001907…).

Server schedule: within a tick, the network tracking (positions, velocities) goes out BEFORE the tick
of the entities; an entity added between two ticks (command, use of an item) is announced
immediately, then the first position send (forced at counter 0) arrives on the next tick with the
spawn position. The client, which has often already run a tick in between, is brought back to the
starting point: this one-tick setback at spawn is normal (same schedule as the game) and
is caught up at the next position correction. The singleplayer server (`rc-sim`) follows the same order for items,
orbs, TNT, falling blocks, projectiles and the bobber.

### 13.1 Common movement (items, orbs, falling blocks, TNT, rockets)

- Stuck multiplier (cobweb 0.25/0.05/0.25; powder snow 0.9/1.5/0.9 for any
  non-living entity): multiplies the wanted movement, then is cancelled together with the velocity.
- Collisions per axis without step-up (Y first, then X and Z, the larger first);
  world border included. Horizontal collision: difference of at least 1e-5 on X or Z.
- Solid entities: the boxes of boats and rafts, of living shulkers (cube pushed towards the
  face opposite the attachment by the "physical" opening 0.5 − sin((0.5 + opening) π) / 2,
  the opening following data 17 / 100 by 0.05 per tick) and of living adult happy ghasts
  (stationary, data 18; or, for the local player, when their feet are above their
  box) count as blocks, searched within the movement's extended box (and of the walk)
  inflated by 1e-7, nothing if its mean size is below 1e-7. Also applies to the local player
  (one stands on a boat or a shulker), to piloted vehicles (except themselves) and to the
  "no collision" test (stuck orbs).
- Currents: for an entity other than a player, the sum of the flows touched is brought back to
  a length of 1 (the player takes their average instead), then × 0.014 (water) or 0.0023 / 0.007
  (lava); minimum 0.0045 if the entity is almost motionless in X and Z.
- Ground and vertical collision recomputed only if the wanted vertical movement is non-zero;
  ground = vertical hit when going down; supporting block like the player (search offset by the
  movement if nothing is under the box).
- Impact: blocked component cancelled; on arrival on a bouncy block, restitution = that
  of the block × 0.8 (non-living) if the fall exceeds gravity and the block is not in the tag
  "cancels bounce". Vertical velocity = (fraction travelled × gravity − vy) × corrected
  friction × restitution.
- Speed factor of the block one is in, otherwise (outside a water block or bubble column: a block
  that is merely waterlogged does not count) that of the braking block (0.500001 under the feet; 0.999999
  for ground items and orbs), applied to X and Z. The ground friction of items and
  orbs is read at the same block at 0.999999: placed on a support higher than half a block
  (soul sand, mud, honey, thick snow), the block below is the one that counts.
- Boat and raft (non-living): block restitution × 0.8 too.
- Effects of the blocks crossed: slime (on the ground, |vy| < 0.1: X and Z × (0.4 + 0.2 |vy|)), cobweb,
  powder snow, honey sliding, bubble columns (above: towards 1.8 or −0.9;
  inside: towards 0.7 or −0.3).

### 13.2 Ground item (`item`, box 0.25)

1. Fluids touched, push of the currents.
2. Water higher than 0.1: X, Z × 0.99, vy + 0.0005 if vy < 0.06; lava higher than 0.1:
   same with 0.95; otherwise gravity 0.04 (except "no gravity").
3. On the ground, horizontal velocity² ≤ 1e-5 and (age + identifier) mod 4 ≠ 0: no movement
   (gravity accumulates and resumes on the next tick that is a multiple of 4); otherwise movement, block effects,
   X and Z × (0.98 × friction of the braking block, computed in single precision), Y ×
   0.98, then on the ground negative vy × −0.5.
4. Fluids and currents a second time.

### 13.3 Falling block (box 0.98) and primed TNT (box 0.98)

- Falling block: gravity 0.04, movement, block effects, velocity × 0.98 (placing, the
  bounce on the ground and the disappearance are the server's; no fluids).
- TNT: gravity 0.04, movement, velocity × 0.98, on the ground × (0.7, −0.5, 0.7); fuse (data
  8) counted down on the client side every tick: at 0 the entity also disappears on the client; otherwise
  fluids and currents.

### 13.4 Experience orb (box 0.5)

Linear interpolation in 3 steps of the received positions; the first client tick does nothing.
Fluids and currents; eyes in water: X, Z × 0.99 and vy = min(vy + 0.0005, 0.06); otherwise
gravity 0.03 if the box touches no block. In lava: random velocity (±0.2, 0.2,
±0.2). Tracking: taken again only if there is none, if it became a spectator or if it is more than
8 away; the nearest player within 8, spectators included, is then taken, and rejected
if it is a spectator or dying (a nearer spectator therefore hides a valid player; a tracked
player who dies remains tracked). Game mode of the other players: player list. Attraction: velocity +
direction × (1 − distance/8)² × 0.1 towards its feet + eyes/2. Server side only: stuck without
a tracked player and still stuck on the next tick, it leaves towards the nearest free face (see
§ 13.7). Then
movement, × 0.98 (× ground friction), and bounce vy = −fall × 0.4 if it was falling faster
than gravity. The actual movement also shifts the interpolation target if the space there is
free.

### 13.5 Projectiles

- Thrown items (snowball, egg, pearl 0.03; potions 0.05; experience bottle 0.07): gravity, inertia 0.99
  (0.8 in water), ray against the collision shapes: stop at the point hit, orientation
  towards the velocity smoothed by a fifth.
- Arrows and tridents: tip in a block = stuck (zero velocity); in water inertia
  0.6 (trident 0.99); orientation from the velocity at the start of the tick; movement by the whole
  velocity before inertia; impact: setback of 0.05 against each moving axis, stuck,
  7-tick shake; inertia 0.99 out of water then gravity 0.05 if not stuck. A
  non-zero velocity received unsticks it. Without physics (loyalty): advances without impact or gravity.
- Powered (fireballs, wither skull 0.73 if "dangerous", wind charges without
  acceleration or inertia): velocity + direction × 0.1, × 0.95 (0.8 in water), orientation
  "towards the movement" by a fifth, ray then placement.
- Llama spit: disappears if its box touches anything other than air (for us: a block
  with a collision shape, known difference for flowers and grass) or if it is in water; otherwise × 0.99,
  gravity 0.06. Shulker bullet: advances by its velocity, orientation by a half. Eye of Ender:
  advances by its velocity. Free rocket: thrust × 1.15 (1.0 after a horizontal impact) and + 0.04
  in Y except shot at an angle, movement with collisions, velocity unchanged.
- Base of the tick of all projectiles: fluids touched and push of the currents (rule for
  non-players). Thrown and powered items: AFTER placement; spit, shulker bullet, eye,
  rocket: BEFORE the rest of the tick; arrow: at the end, without push once stuck (stuck:
  no tick base). The "in water" inertia uses the state of the previous tick's base.
- Effects of the blocks crossed (bubble columns, cobweb…) between the old and the new
  position: thrown, powered, shulker bullet, arrows. Thrown items, first tick:
  bubble columns touched by the box.
- Powered: the acceleration power is sent by the server with each velocity (packet
  "projectile power"); default 0.1 (0 for wind charges).
- Fishing bobber (box 0.25, linear interpolation 3, target shifted by the simulated
  movement): "synchronised" random seeded on every tick by the low 64 bits of the UUID mixed with the game
  time; tick base (currents); water height of the feet block. In flight: known hook →
  zero velocity, hooked state; in water → velocity × (0.3, 0.2, 0.3), floating state; otherwise
  ray towards the next block: velocity brought onto its direction by the SQUARE of the distance to the point
  hit (game quirk reproduced). Hooked: placed on the entity at 0.8 of its height, in
  flight if it disappears. Floating: force = y + vy − block y − water height (± 0.1 added if
  |force| < 0.01, 0 for 0); X, Z × 0.9, vy − force × own random × 0.2; biting and in water:
  vy − 0.1 × two synchronised draws. Then gravity 0.03 out of water, on the ground and without a hook;
  movement with collisions; in flight and on the ground or against a wall: stop; × 0.92. Bite received
  (data 9 set to true): vy = −0.4 × (0.6 + synchronised draw × 0.4).
- Entity hits stay on the server.

### 13.6 Interpolation of other entities

Living: in steps, duration = the type's tracking interval (players, mannequins, allays 2;
others 3), head in 3 steps. Shulker: never interpolated. Boats, rafts, minecarts, bobber,
orbs: linear 3 steps. Displays: linear over the duration of data 10, direct placement
(no smoothing) at 0. Frames, paintings, knots: direct placement.

### 13.7 Singleplayer server (`rc-sim`)

Ground items, orbs, primed TNT and falling blocks use the same physics as the client
(`rc_physics::body`, single-precision constants, braking block, water, lava and currents,
block effects, solid entities); target correction: 0 (measured: items and TNT < 0.0002 block).
Differences specific to the server: the orb does not skip its first tick; an item whose box
(shrunk by 1e-7) touches a collision passes through without collision and leaves towards the nearest
free face (north, south, west, east, up: neighbour without a full shape; velocity 0.1 to 0.3 on that axis,
the others × 0.75); a stuck orb without a tracked player does the same if it would still be stuck on the
next tick. Network tracking: TNT every 10 ticks, falling block and items 20, before the tick.
`/summon`: TNT (`fuse`, 80), falling block (`BlockState`, sand by default and in place
of air; `Time`), orb (`Value`, `Count`, `Age`, `Health`) and projectiles are
simulated with their `Motion` (component beyond 10 read as 0, as everywhere on loading).

Falling block (server): fall distance accumulated during movement (even in water:
this entity never updates its "in water" state); on landing, the block hit
passes it on (powder snow: nothing; stalagmite tip: + 2.5; beds and shelf
fungus: half) and, if the block hurts (anvil: 2 per block, 40 at most; summon:
`HurtEntities`, `FallHurtAmount`, `FallHurtMax`), each living mob and the player (neither
creative nor spectator) in its box take min(⌊⌈fall − 1⌉ × damage per block⌋, cap)
(type "anvil", "stalactite" or "falling block"). The anvil then wears with a
chance of 0.05 + 0.05 × ⌈fall − 1⌉ (damaged, chipped, then broken). Arrival: replaceable
cell and block that can stand → placed (anvil: event 1031); otherwise, or lost block (out of
the world after 100 ticks, or 600 ticks), dropped as an item at its position (delay 10, rule
`entity_drops`, `DropItem`) after the break event (anvil 1029, stalactite 1045, sulfur
spike 1052, suspicious sand and gravel: break 2001). Suspicious sand and gravel (and a broken
anvil) always break without an item.

Primed TNT explosion (rule `tnt_explodes`, otherwise nothing is computed): common
server explosion, centre at 0.0625 × height above the feet, power 4, block resistance
from the registry; damage and knockback of mobs and the player ("explosion" type), loot of
destroyed blocks (decay according to `tnt_explosion_drop_decay`, false by default), explosion packet
(particle, sound, knockback). A blown-up TNT becomes a primed TNT with fuse 10 to 29. The blast
also hits non-living entities: primed TNT (direction from its feet, never
hurt), falling block and orb (always), ground item (only if `mob_griefing` or if
the explosion destroys blocks); items and orbs lose health points (destroyed at 0);
all are pushed, the velocity being sent back to the client at the next tracking.
