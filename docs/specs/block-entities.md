# Block entities: reception and special rendering (26.3, protocol 777)

What is drawn "apart" from the chunk meshes: chest lids, sign text, banners, heads, beacon beam,
End portals, etc. The layered entity models (`minecraft:<name>#<part>`) are the ones described in
`entities.md` (§ 1–2: cubes, `entity_models.json` dump; § 3: render types, lightmap, diffuse
lighting): **we reuse the entity engine** (instanced parts, `PartMat`) for every model in this spec.
The same models are used for items in slots: `items.md` § 1.4 (`special` types), including the dye
colour table, reused here.

Conventions: block-local frame in block units, origin at the low north-west corner; `T` =
translation, `Rx/Ry/Rz(a)` = rotation (degrees unless stated), `S` = scale, product applied left to
right (matrix stack). `rot_y(dir)`: south 0, west 90, north 180, east 270.
`seg` (property `rotation`, 0–15) means `seg × 22.5°`. `pt` = fraction of a tick.

---

## 0. Important change in 26.x: many "block entities" are block models

- **Beds**: no block-entity rendering. Each half is an ordinary block model
  (`block/<colour>_bed_head|foot`, parent `template_bed_*`, `y` rotation in the blockstate) →
  normal chunk meshing (`models.md`). The `bed` block entity exists but draws nothing.
- **Signs and hanging signs**: the plank, the post and the chains are block models
  (`template_sign_rot_0..3`, `template_wall_sign`, `template_attached_hanging_sign_rot_*`,
  `template_*_hanging_sign*`, 16 rotations = 4 models × `y` rotation). The block entity draws
  **only the text** (§ 3).
- Bell (support), campfire, lectern, enchanting table, beacon, item frame and portal block,
  copper chest, shelf, spawner, vault, trial spawner, flower pot, piston, chiseled bookshelf,
  creaking heart: the fixed part is a block model; the block entity adds the animated or dynamic
  part.
- Blocks whose block model is **empty** (particle texture only) and which are therefore drawn
  entirely by the block entity: chests (4 kinds + 8 copper ones), Shulker boxes, banners,
  heads/skulls, decorated pot, conduit, copper golem statues, End portal and End gateway.
- The game also has a "special block models" table (heads, banners, Shulker, chests including
  Christmas, resting bell, conduit (shell), pot, closed enchanting-table book, portal/gateway cube,
  statues): it is used to draw these blocks **outside the world** (block carried by an enderman,
  display entity block, minecart, item held by a copper golem), with a frozen state (lid closed,
  flag at rest). See `items.md` § 1.4 and `entities-objects.md` (display entities). It does not
  apply to blocks in the world.

## 1. Reception and life cycle

### 1.1 Creation

- The client creates a block entity **from the block state** (the type is derived from the block),
  never from the packet alone.
- Chunk packet (`chunks.md`, "Block entities"): on load, all block entities of the chunk are first
  cleared; for each listed entry, the block entity is created at the position (if the block state
  carries one) then, if the NBT is present **and** the announced type equals the created type, its
  data is loaded. Entry of a different type: block entity created empty, NBT ignored.
- Block change (`block_update`, `section_blocks_update`, local prediction):
  - **different** block (not just a different state) and the old one carried a block entity →
    removal (except a few blocks that keep theirs when changing block: oxidising copper, etc.,
    per-block "keep the block entity" rule);
  - new state that carries a block entity: if a valid block entity exists, it simply receives the
    new state (orientation, chest `type`…); otherwise an **empty** block entity is created (default
    values until the next `block_entity_data`).
- Chunk unload: everything disappears with it.

### 1.2 `block_entity_data`

Content: position, type (id of the `block_entity_type` registry), NBT (compound). Applied **only
if** a block entity exists at the position **and** is of that type; otherwise ignored (no
creation).

Loading = **field-by-field replacement**: each field read by the type takes the NBT value, or its
default value if absent (no merge with the old one). Components (`components`) are replaced as a
whole (empty if absent). The same loading is used for the NBT of the chunk packet.

### 1.3 Data sent and kept per type (useful for rendering)

| type | NBT sent to the client | kept for rendering |
|---|---|---|
| `sign`, `hanging_sign` | `front_text`, `back_text` = {`messages` (4 text components), `filtered_messages` (4, optional = same), `color` (dye, default `black`), `has_glowing_text` (default false)}, `is_waxed` | both faces |
| `banner` | `patterns` = list of {`pattern` (id or definition), `color`}; optional name | layers (base colour = block) |
| `skull` | `profile` (resolvable profile), `note_block_sound`, name | profile (player head) |
| `decorated_pot` | `sherds` = {`back`,`left`,`right`,`front`} (optional items), `item` | decorations |
| `campfire` | `Items` (4 slots) | the 4 stacks |
| `shelf` | `Items` (3), `align_items_to_bottom` | stacks + alignment |
| `brushable_block` | `hit_direction` (optional), `item` (if not empty) | item, side |
| `mob_spawner` | spawner data except `SpawnPotentials` (`Delay`, `RequiredPlayerRange`…) | displayed entity, delay, range |
| `trial_spawner` | `next_mob_spawns_at` (active state), `spawn_data` | displayed entity |
| `vault` | `shared_data` (including `display_item`) | displayed item |
| `beacon` | primary/secondary effects, name | — (beam recomputed by the client, § 4.9) |
| `conduit` | `Target` (entity) | — (shape recomputed, § 4.12) |
| `end_gateway` | `Age`, exit | age |
| `structure_block`, `test_instance_block`, `jigsaw` | full data | box (§ 4.16) |
| `chest`, `ender_chest`, `trapped_chest`, `shulker_box`, `bell`, `enchanting_table`, `lectern`, `copper_golem_statue`, `bed` | nothing useful (no update packet) | local state animated by `block_event` / client tick |

Shulker box colour, banner colour, bed colour, and oxidation of statues and copper chests:
derived **from the block**, not from the NBT.

### 1.4 `block_event` (rendering only)

Packet: position, type 1 (`b0`), parameter (`b1`), block id. Forwarded to the block entity:

| block | `b0` | effect |
|---|---|---|
| chest, trapped chest, copper chests, ender chest | 1 | `b1 > 0` → lid must open, `0` → close (§ 4.1) |
| Shulker box | 1 | `b1 = 1` → open, `b1 = 0` → close (§ 4.2) |
| bell | 1 | `b1` = 3D direction of the hit (0 down, 1 up, 2 north, 3 south, 4 west, 5 east); restarts the swing (§ 4.3) |
| decorated pot | 1 | `b1` = wobble style 0 (positive, 7 ticks) or 1 (negative, 10 ticks) (§ 4.7) |
| End gateway | 1 | cooldown = 40 ticks (purple beam, § 4.11) |
| pistons | 0/1/2 | extend/retract/drop: out of scope (world mechanics); rendering of the moving block is § 4.17 |

The chest sound, the note, etc. are not covered here (`sounds.md`).

### 1.5 Client ticks

On every world tick, the client runs the animator of the following block entities (the others have
no client tick useful for rendering): chest lids (including Ender and copper), Shulker box, bell,
enchanting table, dragon and piglin heads, conduit, beacon, End gateway, spawner, trial spawner,
vault, moving piston block (and campfire particles).

## 2. Exhaustive list of registered special renderers (26.3)

26 types: `sign`, `hanging_sign`, `mob_spawner`, `piston`, `chest`, `ender_chest`,
`trapped_chest`, `enchanting_table`, `lectern`, `end_portal`, `end_gateway`, `beacon`, `skull`,
`banner`, `structure_block`, `test_instance_block`, `shulker_box`, `conduit`, `bell`, `campfire`,
`brushable_block`, `decorated_pot`, `trial_spawner`, `vault`, `copper_golem_statue`, `shelf`.

**No rendering** for: `bed`, `jigsaw`, `creaking_heart`, `chiseled_bookshelf`, `hopper`,
`furnace`…, `sculk_*`, `crafter`, `command_block`, `daylight_detector`, `comparator`, `beehive`,
`barrel`, `dispenser`, `jukebox`, `test_block`… (block model only).

Common to all:
- **Light**: that of the **position of the block itself** (sky, block), with the block light raised
  to at least the state's own emission; "emissive rendering" state → full light. Exceptions: double
  chest (max of the two halves, § 4.1), brushable block (neighbouring cell on the hit side, § 4.8),
  glowing text (full), beams (full).
- No ground shadow, no damage overlay (overlay texture "none").
- **Cracks**: if the position has a destroy stage (`interactions.md` § 2, highest stage, **without**
  the 32-block limit of ordinary blocks), the renderers marked "crack" below redraw their model a
  second time with the crack texture (same projected UVs as in `interactions.md` § 2.2, from the
  block position), drawn right after.
- No renderer depends on the graphics quality; only the chunk fade-in difference (§ 5) and the
  ranges (§ 5) hide them.

## 3. Signs and hanging signs (text)

Plank/post: block models (§ 0). For each face (front, back), the block entity draws the 4 lines of
text.

| | sign | hanging sign |
|---|---|---|
| max line width (font px) | 90 | 60 |
| line height `h` | 10 | 9 |

### 3.1 Transformation of each face

Angle `a` = `seg × 22.5` (on the ground / on the ceiling) or `rot_y(facing)` (on the wall).

- Sign: `T(0.5, 0.5, 0.5) · Ry(−a)`; on the wall only `· T(0, −0.3125, −0.4375)`; back face
  `· Ry(180)`; then `· T(0, 1/3, 0.046666667) · S(1/96, −1/96, 1/96)`.
- Hanging (ceiling, attached or not, and wall: same formula): `T(0.5, 0.9375, 0.5) · Ry(−a) ·
  T(0, −0.3125, 0)`; back face `· Ry(180)`; then `· T(0, −0.32, 0.073) ·
  S(0.0140625, −0.0140625, 0.0140625)`.

### 3.2 Lines

- Messages: `filtered_messages` if the client option "text filtering" is active, otherwise
  `messages`. Each message is split at the max width (rules of `text.md`) and **only the first
  line of the split** is kept (the rest is lost, no line wrapping). Empty message → empty line.
  Cached per face (recomputed if the filtering changes).
- Line `i` (0–3): `x = −width/2` (integer division), `y = i × h − 2h`; no drop shadow.
- Colour `c` = text colour of the dye `color`:

| dye | RGB | dye | RGB |
|---|---|---|---|
| white | FFFFFF | gray | 808080 |
| orange | FF681F | light_gray | D3D3D3 |
| magenta | FF00FF | cyan | 00FFFF |
| light_blue | 9AC0CD | purple | A020F0 |
| yellow | FFFF00 | blue | 0000FF |
| lime | BFFF00 | brown | 8B4513 |
| pink | FF69B4 | green | 00FF00 |
| red | FF0000 | black | 000000 |

- "Dark colour" `d`: `F0EBCC` if `black` + glowing, otherwise `c × 0.4` per channel (integer).
- **Not glowing**: text colour `d`, block light, no outline.
- **Glowing** (`has_glowing_text`): text colour `c`, **full light** (15/15); outline of colour `d`
  if the dye is `black`, **or** if the camera is within 16 blocks (distance² < 256 between the
  camera entity and the block centre), **or** if the local player is aiming through a spyglass in
  first person. Outline = 8 copies of the text offset by (±1, ±1, and cross) × the glyph's shadow
  offset (1 px for bitmap fonts), colour `d`, without effects (underline…), drawn in "normal" mode,
  then the text on top.
- Text drawing mode (and the text above the outline): "polygon offset" (world text pipeline,
  translucent blending, depth test with write, slope bias 1, constant 10) — `text.md` § 6.
- **Range**: the common range of 64 blocks (§ 5); beyond it the text disappears, the plank stays
  (chunk mesh). No other threshold.

## 4. Rendering per type

### 4.1 Chests (`chest`, `trapped_chest`, `ender_chest`, copper chests)

- Layers: `chest` (single), `double_chest_left`, `double_chest_right` according to the `type`
  property (`single`/`left`/`right`); parts `bottom`, `lid`, `lock`. The two halves of a double
  are **two** block entities, each drawn with its own layer (the joining faces are absent from the
  model).
- Texture: `chest` atlas, sprite `entity/chest/<name>`:
  - Ender: `ender` (always single);
  - copper (and waxed): `copper`, `copper_exposed`, `copper_weathered`, `copper_oxidized`;
  - otherwise, if the machine's local date is **24, 25 or 26 December**: `christmas` (chest and
    trapped chest); decided **once** when the renderers are created (resource loading), not on
    every frame;
  - otherwise `trapped` (trapped) or `normal`;
  - suffix `_left` / `_right` for the halves of a double (except Ender).
- "Cutout entity with back-face culling" render type. Crack.
- Orientation: `Ry(−rot_y(facing))` around (0.5, 0, 0.5).
- Lid: raw opening `t` (0–1) advanced per client tick by ±0.1 towards the target given by
  `block_event` (§ 1.4), interpolated `lerp(pt, old, current)`. Double: `t` = **max** of the two
  halves; light = max of the lights of the two positions. Curve: `o = 1 − (1 − t)³`;
  `lid.rot_x = lock.rot_x = −o × π/2`. Duration: 10 ticks to open or close.

### 4.2 Shulker boxes (`shulker_box`)

- Layer `shulker_box` (parts `base`, `lid`). Atlas `shulker_boxes`, sprite `entity/shulker/shulker`
  (undyed box) or `entity/shulker/shulker_<colour>`. "Cutout entity" render type. Crack.
- Transformation: `T(0.5, 0.5, 0.5) · S(0.9995) · R(facing) · S(1, −1, −1) · T(0, −1, 0)`
  with `R`: up = identity, down = `Rx(π)`, north = `Rxyz(π/2, 0, π)`, south = `Rx(π/2)`,
  west = `Rxyz(π/2, 0, π/2)`, east = `Rxyz(π/2, 0, −π/2)` (angles in radians, order X then Y then Z).
- Animation: `p` = progress, **linear**, +0.1/tick when opening, −0.1/tick when closing
  (10 ticks), interpolated by `pt`. Lid: position `(0, 24 − 8p, 0)` px,
  `rot_y = 270° × p` (it rises by half a block while turning three quarters of a turn).
- Note: the open box also has a collision that pushes entities (physics, out of scope).

### 4.3 Bell (`bell`)

- Support: block model (`bell_floor`, `bell_ceiling`, `bell_wall`, `bell_between_walls`).
- Body: layer `bell` (part `bell_body` and child `bell_base`), blocks atlas, sprite
  `entity/bell/bell_body`, "solid entity" render type. Crack. No transformation of its own (the
  model is already placed in the block: pivot at (8, 12, 8) px).
- Swing: `block_event` resets `ticks = 0` and `shaking = true`; client tick `ticks++` while
  shaking; at 50, shaking = false, `ticks = 0`. Base angle `b = sin(τ/π) / (4 + τ/3)` with
  `τ = ticks + pt`; depending on the face hit: north `rot_x = −b`, south `rot_x = +b`, east
  `rot_z = −b`, west `rot_z = +b` (up/down: none). At rest, angles are zero.

### 4.4 Banners (`banner`)

- Layers: `standing_banner` (`pole` + `bar`) / `wall_banner` (bar only), then
  `standing_banner#flag` / `wall_banner#flag` (part `flag`, 20 × 40 × 1 px).
- Transformation: `T(0.5, 0, 0.5) · Ry(−a) · S(2/3, −2/3, −2/3)`, `a = seg × 22.5` (ground)
  or `rot_y(facing)` (wall).
- Waving: `phase = (positive_mod(7x + 9y + 13z + game_time, 100) + pt) / 100` (x, y, z = block
  position); `flag.rot_x = (−0.0125 + 0.01 × cos(2π × phase)) × π`. Pole and bar are still.
- Drawing (identical to `items.md` § 1.4.2, with the phase above): pole/bar then flag with atlas
  `banner_patterns` / sprite `entity/banner/banner_base` (solid); on the flag only, layer
  `entity/banner/base` dyed with the base colour (colour of the **block**) at order 1, then at most
  **16** layers from `patterns` (sprite `entity/banner/<asset_id>`, tint = diffuse colour of the
  layer's dye, table in `items.md` § 1.4) at orders `i + 2`, "banner pattern" render type
  (translucent, `entities.md` § 3.2). Crack on pole/bar and on the flag at order
  `layer_count + 2`.
- Footprint: the flag hangs below the block (standing banner: from the top of the pole ≈ 2 blocks).

### 4.5 Heads and skulls (`skull`)

- One layer per type (`items.md` § 1.4.9): `skeleton_skull`, `wither_skeleton_skull`,
  `zombie_head`, `creeper_head`, `dragon_skull`, `piglin_head`, `player_head`. File texture
  (outside the atlas) of the type. "Cutout entity with Z offset" render type. Crack.
- Player head: without `profile` → default skin (`items.md` § 1.4.11) in cutout-offset; with
  `profile` → skin of the profile (downloaded, or default skin derived from the UUID in the
  meantime) in "**translucent** entity" rendering.
- Transformation: ground `T(0.5, 0, 0.5) · Ry(−seg × 22.5) · S(−1, −1, 1)`; wall
  `T(0.5 − 0.25·dx, 0.25, 0.5 − 0.25·dz) · Ry(−rot_y(opposite(facing))) · S(−1, −1, 1)`
  ((dx, dz) = step of `facing`).
- Animation (dragon and piglin only, client tick): if the state has `powered = true`, counter
  `n++` per tick and `anim = n + pt`; otherwise `anim = n` (frozen where it stopped, no reset).
  Dragon: `jaw.rot_x = (sin(anim × π × 0.2) + 1) × 0.2`. Piglin: left ear
  `rot_z = −(cos(anim × π × 0.2 × 1.2) + 2.5) × 0.2`, right `rot_z = (cos(anim × π × 0.2) + 2.5) × 0.2`.
  The other heads do not move.

### 4.6 Beds

No block-entity rendering (§ 0): colour = block, two halves = two block models.

### 4.7 Decorated pot (`decorated_pot`)

- Layers `decorated_pot_base` (`neck`, `top`, `bottom`, sprite `entity/decorated_pot/decorated_pot_base`)
  and `decorated_pot_sides` (`front`, `back`, `left`, `right`), atlas `decorated_pot`, all in solid
  rendering. Face sprite: `entity/decorated_pot/<pattern asset_id>` if the item of the face carries
  the pottery pattern component, otherwise `decorated_pot_side` (brick or empty).
- Orientation: `Ry(180 − rot_y(facing))` around (0.5, 0.5, 0.5).
- Wobble (after `block_event`): `p = (game_time − start + pt) / duration` (duration 7 or 10
  depending on the style received). If `0 ≤ p ≤ 1`: `Ry(sin(−3πp) × 0.125 × (1 − p))` (radians)
  around (0.5, 0, 0.5). **Quirk of the official game**: the style is never passed to the drawing
  step, so the "positive" variant (tilt in X/Z) is never drawn; both styles give this Y rotation,
  only the duration differs. To be reproduced as is.
- No crack drawn on the pot.

### 4.8 Campfire (`campfire`), shelf (`shelf`), vault, brushable block: items put on display

Items drawn like item stacks (`items.md`), block light:
- **Campfire**: 4 slots, "fixed" context, seed `(i32)(packed position) + i`. Slot `i`:
  `T(0.5, 0.44921875, 0.5) · Ry(−rot_y(d)) · Rx(90) · T(−0.3125, −0.3125, 0) · S(0.375)` with
  `d` = horizontal direction of index `(i + index_2d(facing)) mod 4` (indices: south 0, west 1,
  north 2, east 3). The fire itself: block model.
- **Shelf**: 3 slots, "on shelf" context, seed = 32-bit fold of the packed position
  (`high XOR low`) + i. `T(0.5, 0.5, 0.5) · Ry(r) · T((i − 1) × 0.3125, b, −0.25) ·
  S(0.25) · T(0, −y_min [− (y_max − y_min)/2 if not bottom-aligned], 0)`, `r = −rot_y(facing)`,
  `b = −0.25` if `align_items_to_bottom`, otherwise 0; y_min/y_max = box of the item's model.
- **Vault**: if `display_item` is not empty: `T(0.5, 0.4, 0.5) · Ry(spin)` then drawn as a cluster
  like a ground item ("ground" context, copy count and stack seed: `entities.md` mission C § 4.1).
  `spin` += 10°/tick (client side, wrapped into ±180), interpolated.
- **Brushable block** (suspicious sand/gravel): if `dusted > 0`, `hit_direction` known and item
  not empty: `T(0, 0.5, 0) · T(tx, ty, tz) · Ry(75) · Ry(11 + 90 if hit east/west) · S(0.5)`,
  "fixed" context. `(tx, ty, tz)` starts from (0.5, 0, 0.5) with `c = dusted / 10 × 0.75`: east
  `tx = 0.73 + c`, west `tx = 0.25 − c`, up `ty = 0.25 + c`, down `ty = −0.23 − c`, north
  `tz = 0.25 − c`, south `tz = 0.73 + c`. Light taken from the **neighbouring cell on the hit side**.

### 4.9 Beacon (`beacon`): beam

- **Sections computed by the client** (tick): vertical sweep of 10 blocks per tick from the
  beacon's block up to the surface height (`WORLD_SURFACE`) of the column. Block that tints the beam
  (stained glass, stained glass pane, the beacon itself = white): colour = diffuse colour of its
  dye; the **first two** encounters each open a section; after that, same colour → current section
  +1, otherwise a new section of the **average** colour (per channel, integer division) of the
  previous one and the new one. Other block: if there is no section or if its light attenuation
  is ≥ 15 (except bedrock) → empty beam; otherwise current section +1. The result replaces the old
  one when the sweep reaches the surface.
- Levels recomputed by the client every 80 ticks (pyramid of 1 to 4 tiers of blocks of the
  "beacon base" tag); **levels = 0 → no beam**.
- Drawing: sections stacked from local y = 0; the **last** one has a height of 2048.
  `t = positive_mod(game_time, 40) + pt`. Centre `T(0.5, 0, 0.5)`.
  - Radius factor `k = 1` if the player is aiming through a spyglass, otherwise `max(1, dh / 96)`
    (dh = horizontal distance camera–block centre): the beam widens in the distance.
  - Core: square rotated `Ry(2.25 t − 45)`, vertices at (0, r), (r, 0), (−r, 0), (0, −r) with
    `r = 0.2 k`; 4 vertical faces; u 0→1; `scroll = −t`; `off = frac(0.2 scroll − floor(0.1 scroll))`;
    `v` from `−1 + off` (bottom) to `h × (0.5 / r) + (−1 + off)` (top), h = section height; opaque
    section colour; "opaque beam" pipeline (no blending, depth written).
  - Halo: unrotated square of half-side `0.25 k`; `v` up to `h + (−1 + off)`; same colour with
    alpha 32/255; "translucent beam" pipeline (classic blending, no depth write).
  - Texture `textures/entity/beacon/beacon_beam.png` repeated; full light; fog applied; no
    diffuse lighting.
- The beacon block itself: block model.

### 4.10 End portal (`end_portal`)

- Geometry: **top and bottom faces only** of a cube, transformed `T(0, 0.375, 0) ·
  S(1, 0.375, 1)` → planes at y = 0.375 and y = 0.75.
- Effect (dedicated pipeline, vertices = positions only): texture coordinates **projected onto the
  screen** (the texture does not follow the surface: window effect onto a starry sky fixed relative
  to the view). Colour = `end_sky × C₀` + sum over `i = 1..N` of `end_portal_tex(proj ×
  M_i) × C_{i−1}`, with `end_sky` = `textures/environment/end_sky.png`, `end_portal_tex` =
  `textures/entity/end_portal/end_portal.png`, `N = 15`, and 16 constant colours `C`
  (dark blue-greens, from (0.022, 0.098, 0.111) to (0.081, 0.315, 0.661) — exact table in the
  shader `rendertype_end_portal.fsh` of the resources). Each layer `M_i` = uniform scale
  `(4.5 − i/4) × 2`, in-plane rotation by angle `(4321 i² + 9 i) × 2` degrees, translation
  `(17/i, (2 + i/1.5) × 1.5 × G)` where `G = (game_time mod 24000 + pt) / 24000`, applied after
  a scale of 0.5 and an offset of 0.25 of the projected frame. Result: layers of dots that scroll
  at different speeds (parallax). Opaque, fog applied, alpha 1.
- No light and no crack.

### 4.11 End gateway (`end_gateway`)

- Full cube 0–1, faces drawn only if the face would be visible against the neighbour (same rule
  as block face culling). Same effect as § 4.10 with `N = 16`.
- Beam (same code as § 4.9, texture `textures/entity/end_portal/end_gateway_beam.png`,
  radii 0.15 / 0.175, k = 1, height = `2h` starting from `−h`, v multiplied by `s`):
  - age < 200 (spawning): `p = clamp((age + pt)/200)`, magenta colour (`C74EBD`),
    max height = world max Y;
  - otherwise cooldown > 0: `p = 1 − clamp((cooldown − pt)/40)`, purple colour
    (`8932B8`), max height 50;
  - `s = sin(π p)`, `h = floor(s × max height)`; nothing if `h = 0`.
  - Client tick: age +1, cooldown −1 if it is > 0.
- **Own range: 256 blocks.**

### 4.12 Conduit (`conduit`)

- Layers `conduit#shell`, `conduit#cage`, `conduit#wind`, `conduit#eye`; blocks atlas, sprites
  `entity/conduit/{base, cage, wind, wind_vertical, open_eye, closed_eye}`.
- Client state recomputed every 40 ticks (game time): active if the 27 cells 3×3×3 centred on it
  are water **and** at least 16 blocks of the "conduit effect block" tag occupy the cells of the
  frame (cells at distance 2 on one of the three centred crosses); "hunting" if ≥ 42 such blocks.
  Client tick: counter `n++`; if active, `rot++`. `angle = (rot + pt) × −0.0375` (pt = 0 if
  inactive).
- Inactive: `T(0.5, 0.5, 0.5) · Ry(angle in degrees)` shell `base`, solid rendering, crack.
- Active, with `τ = n + pt`, `hh = sin(0.1 τ)/2 + 0.5`, `hh = hh² + hh`:
  - cage: `T(0.5, 0.3 + 0.2 hh, 0.5)` then rotation of `angle` radians around the normalised axis
    (0.5, 1, 0.5), sprite `cage`, cutout, crack;
  - wind: phase `(n / 66) mod 3`; `T(0.5, 0.5, 0.5)`, then phase 1 `Rx(90)` (sprite
    `wind_vertical`), phase 2 `Rz(90)` (sprite `wind`), phase 0 nothing (`wind`); second wind
    `T(0.5, 0.5, 0.5) · S(0.875) · Rxyz(π, 0, π)`; cutout;
  - eye: `T(0.5, 0.3 + 0.2 hh, 0.5) · S(0.5) · camera_orientation · Rz(π) · Ry(π) · S(4/3)`
    (always facing the camera), sprite `open_eye` if hunting, otherwise `closed_eye`, cutout.

### 4.13 Enchanting table (`enchanting_table`) and lectern (`lectern`): book

Layer `book`, blocks atlas, sprite `entity/enchantment/enchanting_table_book`, solid rendering, crack.
Model pose: with `o` = effective opening, `left cover rot_y = π + o`, right `−o`,
left pages `o`, right `−o`, turning page 1 `o − 2o f₁`, page 2 `o − 2o f₂`, and
`x = sin(o)` for the pages and the two turning pages. `o = (sin(0.02 τ) × 0.1 + 1.25) × open`.

- **Table**: client tick: nearest player (non-spectator) within 3 blocks of the block centre →
  target `target_rot = atan2(dz, dx)`, `open += 0.1`, and if `open < 0.5` or 1 chance in 40:
  `flip_target += rand(4) − rand(4)` (repeated until it changes); otherwise `target_rot += 0.02`,
  `open −= 0.1`. Then `rot += wrap(target_rot − rot) × 0.4` (angles brought into [−π, π)),
  `open` clamped to [0, 1], `time++`, `diff = clamp((flip_target − flip) × 0.4, −0.2, 0.2)`,
  `flip_speed += (diff − flip_speed) × 0.9`, `flip += flip_speed`. Randomness: own generator, not
  shared (free approximation).
  Drawing: `T(0.5, 0.75, 0.5) · T(0, 0.1 + 0.01 sin(0.1 τ), 0) · Ry(−interpolated rot, radians,
  short path) · Rz(80)`; `τ = time + pt`; `f₁ = clamp(frac(flip + 0.25) × 1.6 − 0.3)`,
  `f₂ = clamp(frac(flip + 0.75) × 1.6 − 0.3)` (interpolated flip); `open` interpolated.
- **Lectern**: if `has_book`: `T(0.5, 1.0625, 0.5) · Ry(−rot_y(clockwise(facing))) ·
  Rz(67.5) · T(0, −0.125, 0)`, fixed state (τ = 0, f₁ = 0.1, f₂ = 0.9, open = 1.2). No
  animation.

### 4.14 Spawner, trial spawner: spinning entity

- Displayed entity created **on the client side** from the entity described by spawn data
  (spawner) or `spawn_data.entity` (trial spawner); an `id` is required; otherwise nothing. Trial
  spawner: only in the states `waiting_for_players` and `active` (state property
  `trial_spawner_state`).
- Drawing: entity state extracted normally (`entities.md`), its light forced to that of the block;
  `T(0.5, 0.4, 0.5) · Ry(spin) · T(0, −0.2, 0) · Rx(−30) · S(s)` then the usual entity rendering
  at the origin; `s = 0.53125`, divided by `max(width, height)` of the entity's box if > 1.
  `spin = lerp(pt, old, current) × 10` (degrees).
- Rotation (client tick):
  - spawner: if a living player is within `RequiredPlayerRange` of the centre: `Delay` −1 (if > 0),
    `spin = (spin + 1000 / (Delay + 200)) mod 360`, smoke + flame particles; otherwise old = current;
  - trial spawner: `spin = (spin + v / (max(0, next_mob_spawns_at − game_time) + 200)) mod 360`,
    `v` = 200 (waiting for players) or 1000 (active).
- The block (cage): block model (translucent/cutout depending on the model).

### 4.15 Copper golem statue (`copper_golem_statue`)

Layer according to the `copper_golem_pose` property: `copper_golem` (standing), `copper_golem_running`,
`copper_golem_sitting`, `copper_golem_star`. Root forced to `y = 0` and `rot_z = π`. File texture
`textures/entity/copper_golem/copper_golem[_exposed|_weathered|_oxidized].png` (waxed =
same as unwaxed). `T(0.5, 0, 0.5) · Ry(−rot_y(opposite(facing)))`. "Cutout entity" render type,
crack. No animation, no eyes layer.

### 4.16 Structure block, test instance block

Drawn only if the local player can use operator blocks (creative and permission) or is a spectator.
Structure block in save or load mode: wireframe outline of the box (light grey); in save mode with
"show air", small wireframe cubes on air, structure void, barriers, light blocks. Test instance:
the same box + a beacon beam (§ 4.9) + error markers (translucent red cube and text). Range 96
(box), off-screen renderers (§ 5). Low priority: debugging tools.

### 4.17 Moving piston block (`piston`)

Draws the moved block with its **block model** (world lighting at its position, biome tint),
offset by `step(direction) × e` where `e = p − 1` (extending) or `1 − p` (retracting),
`p = lerp(pt, old, current)` with `p += 0.5` per tick. Piston head: short shape if
`p ≤ 0.5` (when extending); retracting source piston: head (short if `p ≥ 0.5`) moving + fixed
extended base. Range 68. The mechanics (creation of the moving block by `block_event`) are out of
scope of this spec.

## 5. Cost and culling

- Block entities are not individually tested against the field of view: we walk the **visible
  sections** of the frame (frustum + occlusion culling, `visibility.md`) and their list of block
  entities (frozen when the section was meshed); a section still fading in (visibility < 0.3 with
  the chunk fade option) draws none.
- Then, per block entity: type valid for the block state, then **range**: distance between the
  block centre and the camera < range (sphere). Default range **64**; piston 68; structure 96;
  End gateway 256; beacon = effective render distance × 16, measured **horizontally** only; test
  instance = max(beacon, box).
- "Off-screen" renderers (beacon, structure, test instance): kept in a separate global list, added
  on creation, **drawn even if their section is not visible** (a beam stays visible when the beacon
  is behind you or underground); they do not go through the section list (no double drawing).
- No bounding box of their own (2-block banners, beams… may disappear if their section is culled,
  except off-screen renderers) — behaviour to reproduce, not to fix.
- Order within the frame: entities, then block entities (camera-relative frame), then block
  cracks, transient blocks, particles; the actual passes are those of the render types
  (`entities.md` § 3.11).
- For Rustcraft: static models (pot, statue, closed Shulker, non-animated heads, closed chests,
  banner without waving…) can be pre-instantiated; only the matrices of the animated parts change
  per frame (existing entity engine).

## Uncertain points

1. **Player head profile**: resolution (partial profile → session request), caching and default
   skin: deferred to `items.md` § 1.4.11; the effect of `skin_patch` is not tracked.
2. Detail of the crack UVs on entity models (same projection as blocks, presumed) — like
   `interactions.md`, point 4.
3. Sign text: exact behaviour of the splitting with styles (bold, fonts) and of unresolved
   components (the server normally resolves the lines): assumed to conform to `text.md`.
4. Bell "resonance" (raiders highlighted and particles when raiders are nearby): not covered
   (entity effects, not block rendering).
5. Beacon: the first section starts from the beacon's own block (local y = 0, white); check on
   screen that the colour displayed above a single stained glass is indeed the pure colour
   ("first two encounters" rule).
6. End portal: the exact table of the 16 colours and the constant `G` were read; the look also
   depends on the sampling (repeat) of the two textures, assumed "repeat, nearest neighbour".
7. The "order-independent transparency" mode (OIT pipelines) exists in 26.x for text, beams and
   translucency; not described here (separate rendering option).
8. Christmas chest: the date is the machine's at resource loading; a client launched on
   23 December only switches on reload (F3+T) — faithful, not to be "fixed".
9. Spawner: change of entity (`SpawnData`) during a game — the displayed entity is assumed
   recreated when the new NBT is loaded.
