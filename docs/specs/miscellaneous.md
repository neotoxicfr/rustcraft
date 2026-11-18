# Miscellaneous additions (leads, pickup, subtitles, spyglass, isolated packets, image effects)

Clean-room spec, written from a reading of the decompiled 26.3 client (kept outside the repository). In our
own words; it complements without repeating: `entities.md` (packet formats § 1.16, 1.17, 1.20;
reactions § "entity packets"; visibility of leashed entities), `entities-objects.md`
§ 6 (lead knot), `sounds.md` § 2.3 and § 7 (subtitles), `hand-camera.md` (field of view,
hands), `hud.md` § 5.5 (camera overlays) and § 5.6 (death screen), `option-menus.md`
(sensitivity, subtitles), `interactions.md` § 5.3 (spyglass sounds), `particles.md`
(effect particles hidden near the spyglass), `debug-f3.md` (`Post:` line),
`creative-inventory.md` § 13.1 (toast manager).

Conventions: "rotate v by yaw θ" = rotation around Y by angle −θ (radians), the game's
convention where the offset (0, 0, 1) points to the front of the entity (yaw 0 = +Z):
`x' = x·cos(−θ) + z·sin(−θ)`, `z' = z·cos(−θ) − x·sin(−θ)`. `pt` = render tick fraction.
`L`, `H` = width and height of the entity's box; `eyes` = eye height.

## 1. Leads (`set_entity_link`, 0x66)

### 1.1 Reception and resolution
- The packet only applies to "attachable" entities: all mobs and all boats /
  rafts. Other id or unknown id: ignored.
- It replaces the entity's lead state with "pending holder = id" (0 = none) and
  detaches any already resolved holder (purely local: no packet, no dropped item).
- Lazy resolution: each time the holder is requested, if the pending id ≠ 0 and
  an entity with this id exists, it becomes the holder. The id can therefore arrive before
  the holding entity (common case: knot or player paired after the mob).

### 1.2 Who draws the rope
The **held** entity draws the rope in its base render (before its name tag), even if
it is invisible. Visibility: see `entities.md` (visible if its box, the holder's, or their
union is in the view cone).

### 1.3 Endpoints (one rope or four)
**Four-rope** mode if the holder accepts square leads (only the happy ghast) AND if
the held entity accepts them (horses of all kinds, donkeys, mules, skeleton and zombie horses,
camels, sniffers, boats and rafts; **not** llamas). Otherwise, a single rope.

One rope:
- start = interpolated position of the entity + `D` rotated by the entity's interpolated body yaw
  (`D` = attachment offset, § 1.4);
- end = the holder's grab point (§ 1.5);
- sag active.

Four ropes (i = 0..3):
- start_i = entity position + `Q_i` (entity's square) rotated by the entity's body yaw;
- end_i = interpolated position of the holder (feet, no eye height) + `P_i` (holder's
  square) rotated by the holder's body yaw;
- **no** sag (straight ropes).

Attachment square, parameters (`a` forward offset, `b` half-length, `c` half-width, `h`
height) as fractions of L and H: `A = a·L`, `B = b·L`, `C = c·L`, `Y = h·H`; points, in
order: `(−C, Y, B + A)`, `(−C, Y, −B + A)`, `(C, Y, −B + A)`, `(C, Y, B + A)`.

| Entity | a | b | c | h |
|---|---|---|---|---|
| horse, skeleton horse (equid default) | 0.04 | 0.52 | 0.23 | 0.87 |
| donkey, mule, zombie horse | 0.04 | 0.41 | 0.18 | 0.73 |
| camel (and its husk variant) | 0.02 | 0.48 | 0.25 | 0.82 |
| sniffer | −0.01 | 0.63 | 0.38 | 1.15 |
| boats and rafts | 0 | 0.64 | 0.382 | 0.88 |
| happy ghast as **holder** | −0.03125 | 0.4375 | 0.46875 | 0.03125 |

### 1.4 Attachment offset `D` (one rope)
Default: `(0, eyes, 0.4·L)`. Exceptions:

| Entity | D |
|---|---|
| allay | (0, 0.6·eyes, 0.1·L) |
| bee | (0, 0.5·eyes, 0.2·L) |
| ocelot, parrot | (0, 0.5·eyes, 0.4·L) |
| fox | (0, 0.55·eyes, 0.4·L) |
| pig, rabbit, wolf, strider | (0, 0.6·eyes, 0.4·L) |
| snow golem | (0, 0.75·eyes, 0.4·L) |
| iron golem | (0, 0.875·eyes, 0.4·L) |
| copper golem | (0, 0.75·eyes, 0) |
| llama, trader llama | (0, 0.75·eyes, 0.5·L) |
| camel | (0, animated body anchor height − 0.2·age scale, 0.56·L) |
| happy ghast | (0, 0, 0) |
| sulfur cube | (0, H/2, 0) |
| boats and rafts | (0, 0.88·H, 0.64·L) |

(The camel's anchor height is the one that places its rider, interpolated: see
`mounts.md`.)

### 1.5 Holder's grab point (one rope)
- Default (any entity): interpolated position + (0, 0.7·eyes, 0).
- Lead knot: position + (0, 0.2, 0).
- Villager and wandering trader: position + `(0, H − 1, 0.2)` rotated by the interpolated body yaw.
- Player: `x0 = 0.22` if their main arm is the left, `−0.22` if it is the right;
  pitch `φ = lerp(pt × 0.5, current pitch, previous pitch)` (sic: the order of the bounds
  is reversed); yaw `θ` = interpolated body yaw.
  - when gliding or spin-attacking: `(x0, −0.11, 0.85)` rotated first by roll
    `ψ` around Z (sign of `ψ` = sign of `vx·rz − vz·rx`, `|ψ| = acos(cos)` between the horizontal
    velocity `v` and the horizontal gaze `r`; 0 if either is zero), then by pitch
    `φ` around X, then by yaw `θ`;
  - when swimming (visual swim pose): `(x0, 0.2, −0.15)` rotated by pitch then by yaw;
  - otherwise: `(x0, H − 1, crouching ? −0.2 : 0.07)` rotated by yaw only.
- Local player, camera in first-person view (camera option, not the observed entity): interpolated
  eyes + `(0.39·s, −0.6, 0.3)` (`s = −1` right main arm, `+1` left) rotated by
  pitch then by yaw, each read as `lerp(pt × 0.5, current, previous)` (same inversion).
- Rope whose horizontal gap is zero in single precision: not drawn (the game's division by zero,
  undefined vertices).

### 1.6 Light
- Start block `Bs` = block containing the held entity's eyes; `Be` = that of the
  holder's eyes (even for a lead knot).
- Block light: the one that each entity's render uses for itself (15 if it
  is on fire, and the per-type overrides of `entities.md`); sky light: world value at the block.
- The four values (block and sky, at both ends) are the same for the four ropes.

### 1.7 Rope geometry
Frame: origin = render position of the held entity + start offset (so the vertices
are relative to the start). `(dx, dy, dz)` = end − start. Horizontal half-thickness:
`(ox, oz) = (dz, dx) × 0.025 / √(dx² + dz²)`.

For a step `k` ∈ [0, 24], `p = k / 24`:
- position on the rope: `x = dx·p`, `z = dz·p`; `y = dy·p` without sag; with
  sag: `y = dy·p²` if `dy > 0`, otherwise `y = dy − dy·(1 − p)²` (parabola whose lowest
  point is at the lowest end, horizontal tangent there);
- light: `block = ⌊lerp(p, start_block, end_block)⌋`, `sky = ⌊lerp(p, start_sky,
  end_sky)⌋` (truncation toward zero);
- base colour `(0.5, 0.4, 0.3)` (brown), opaque, multiplied by 0.7 every other step.

Two ribbons as triangle strips (each pair of vertices = one step):
1. ribbon A, `k` = 0 → 24: vertices `(x − ox, y + 0.05, z + oz)` then `(x + ox, y, z − oz)`;
   darkened (×0.7) when `k` is even;
2. ribbon B, `k` = 24 → 0: vertices `(x − ox, y, z + oz)` then `(x + ox, y + 0.05, z − oz)`;
   darkened when `k` is odd.

The two ribbons cross in an X (section: diamond 0.05 wide and 0.05 high) and their dark
bands are offset by one step; the colours are interpolated between two steps (gradient,
no flat fills). Render: no texture, colour × light table (lightmap), fog, depth test and
write, **no back-face culling**, opaque.

### 1.8 Lead knot
Model, texture, shadow: `entities-objects.md` § 6. As a holder: § 1.5 (+0.2 in Y).

## 2. Pickup (`take_item_entity`, 0x7F)

Format: `entities.md` § 1.17. Sounds and removal: `entities.md` (reactions). Details:

1. Unknown picked-up entity: nothing (no sound, no animation).
2. Sound, unless the picked-up entity is silent, played at its position, players category, not
   deferred: experience orb → `entity.experience_orb.pickup`, volume 0.1, pitch
   `(r1 − r2)·0.35 + 0.9`; other → `entity.item.pickup`, volume 0.2, pitch
   `(r1 − r2)·1.4 + 2.0` (`r1`, `r2` uniform draws [0, 1) of the client's generator).
3. **Flying copy**: the full render state of the picked-up entity is frozen **as is, before any
   reduction of the stack** (current non-interpolated position, number of copies displayed
   from the whole stack, highlight outline removed). If the picker is known, this copy
   becomes a "pickup particle":
   - lifetime 3 ticks: age 0 at creation, +1 per tick, removed when the age reaches 3;
   - target = (x, (y + eye y) / 2, z) of the picker — middle of the body — re-read on each tick,
     with memory of the previous tick's value; at display time, target interpolated by `pt`;
   - progress `t = ((age + pt) / 3)²` (slow start, fast arrival);
   - displayed position = `lerp(t, frozen position, interpolated target)`, straight-line path;
     the item keeps its frozen appearance (no extra rotation or bobbing);
   - drawn like the original entity (same entity render), in the particle group
     dedicated to pickups.
4. Then: item entity → stack reduced by the quantity, entity removed if the stack is empty (otherwise
   it stays on the ground with the remainder, while the copy of the whole stack flies away); experience
   orb → **not removed** (the server removes it via `remove_entities`, or keeps it if it
   merged several orbs); any other entity (arrow, trident, spectral arrow…) →
   removed right away, only the copy stays visible for 3 ticks.

## 3. Subtitles

Entirely covered by `sounds.md` § 2.3 (`subtitle` key of the sound definition file) and
§ 7 (subscription, list, position, arrows `<` / `>`, fade, duration); option `showSubtitles` in
`option-menus.md` (Sound screen and Accessibility screen). Additions:
- The subtitle does **not** come from the packet: it comes from the sound event resolved on the client
  side (sound definition file). A sound sent "directly" (id + range) only has a
  subtitle if this id exists in the loaded definitions; an unknown id
  plays nothing and displays nothing.
- The subtitle notification happens **before** the volume computation: a sound whose category
  is set to 0 % still displays its subtitle. A "deliberately empty" sound displays none.
- Range transmitted = attenuation distance of the sound at its volume (`D` of `sounds.md` § 3.3).
- No limit on the number of lines: only the duration removes them.
- The subscription follows the option on each frame where the panel is prepared (enabling/disabling mid-game
  takes effect immediately; when disabled, the list is no longer fed).

## 4. Spyglass

"Aiming" = the local player is using an item and this item is a spyglass. Everything else is
already specified:

| Aspect | Rule | Spec |
|---|---|---|
| Zoom | first person only: field of view multiplier target = 0.1 directly (without the effect scale), smoothed like the others | `hand-camera.md` (field of view, step 3) |
| Overlay | first person: square `textures/misc/spyglass_scope.png`, scale smoothed toward 1.125 (restarts from 0.5 when not aiming), black bands | `hud.md` § 5.5 |
| Mouse | first person: rotation = movement × `f³` instead of `f³ × 8` (8 times less sensitive); the cinematic camera, if active, takes precedence (it keeps its smoothing and the normal sensitivity) | `option-menus.md` (sensitivity) |
| Sounds | `item.spyglass.use` at the start, `item.spyglass.stop_using` at the end, played locally | `interactions.md` § 5.3 |
| Hand | no hand nor held item drawn in first person while aiming | `hand-camera.md` |
| Third person | no zoom, no overlay, normal sensitivity; arm pose of the model (spyglass at the eye) | `entities.md` (arm poses) |
| Item model | "in hand" model except in the interface, on the ground, in a frame and on a shelf | item definition `items/spyglass.json` |
| Other | effect particles within 3 blocks of the eyes hidden; beacon beam radius not widened; text outline of glowing signs | `particles.md`, `block-entities.md` |

The crosshair stays drawn (next stratum, above the overlay).

## 5. Isolated packets

### 5.1 `player_combat_enter` (0x44) and `player_combat_end` (0x43)
`enter`: empty; `end`: `duration: varint` (ticks spent in combat). The client **does nothing**
with either (read and ignore).

### 5.2 `player_combat_kill` (0x45)
`player: varint` (entity id), `message: text component`. Only if the id is the local
player's: if the death screen is enabled (no immediate respawn, see the matching game
event), open the death screen with this message as the cause and the world's "hardcore" flag;
otherwise request respawn right away. Screen: `hud.md` § 5.6.

### 5.3 `post_effects` (play 0x53, configuration 0x0A)
Exists in 26.3. Format: list (`varint` count) of ids. Used by the server to impose
**full-screen image effects** on the player (server command `/posteffect add|remove|clear|list`,
list saved with the player). The client replaces its local list with the one received (order
kept). Sent on login, on respawn, on dimension change and on every
modification. Received during configuration (no player yet), the official client raises an error
(disconnect): in practice the server only sends it in game; we will ignore it without error.
Each id `ns:path` designates the file `ns:post_effect/<path>.json` of the resources;
application: § 6.3. Files provided by the game: `blur`, `creeper`, `spider`, `invert`
(`entity_outline` is internal to the glow outline). Debugging: `debug-f3.md`.

### 5.4 `projectile_power` (0x8A)
Format: `entities.md` § 1.20. Target: "powered" projectiles (large and small fireballs,
dragon fireball, wither skull, wind charges of the player and of the breeze). The client stores
the **acceleration power** (default 0.1 at creation) and replays it in its own simulation
of the projectile, on every tick:
`v ← (v + normalised(v) × power) × inertia`, inertia 0.95 (0.8 in water, with 4 bubbles
behind; "dangerous" wither skull: 0.73; wind charges: 1.0 everywhere), then ray cast
along the movement, rotation toward the direction (factor 0.2), movement, particle
trail. The server sends this packet grouped with the velocity (`set_entity_motion`) whenever
it resynchronises the velocity of such a projectile (deflection: power reset to 0.1 if deflected by
a hit, divided by 2 otherwise — decided on the server side).

### 5.5 `add_transient_block` (0x25)
Format: `position: block position (packed i64)`, `state: varint` (block state id).
Sent by the server when a **falling block** (sand, gravel, anvil…) lands, right after
the `block_update` of the same block and before the removal of the entity. Goal: avoid the one-frame "hole"
between the disappearance of the falling entity and the rebuilding of the section mesh.
Client:
- record `(position, state, creation time)`, lifetime 1 s; an addition at the same
  position replaces the previous one;
- draw each transient block like an isolated moving block (same render as the falling block:
  block model, biome tint at the position, world lighting), at its position, as long as
  it lives;
- remove it when its duration has elapsed, or when a mesh of **its section** whose
  compilation **started after** its creation becomes visible (the block is then in the
  mesh).

### 5.6 `move_minecart_along_track` (0x38)
Format: `entities.md` § 1.20 (angles in angle bytes, `f32` weights). Only concerns
minecarts when the experimental "minecart improvements" feature is active
(`minecart_improvements` in the feature list received during configuration). In this mode,
the minecart does **not** use the generic interpolation of entity movements; it follows
**steps**:
- reception: the steps are appended to a queue `F`;
- client tick (`r` = counter, initially 0): `r −= 1`; if `r ≤ 0`: store the current state
  as the start step `E0` (position, velocity, yaw, pitch, weight 0), empty the current list
  `C`, and if `F` is not empty: `C ← F`, empty `F`, `W` = sum of the weights of `C`,
  `r = W == 0 ? 0 : 3`. Then, if `C` is not empty, the entity's position, velocity, yaw and pitch
  ← values interpolated at `pt = 1` (below). Then "on rails" = the current block
  (or the one below) is a rail;
- interpolation at `pt`: `α = (3 − r + pt) / 3`; walk through `C` accumulating weights > 0
  (weights ≤ 0 skipped) up to the first step `i` where the total ≥ `W·α`; fraction
  `f = (α·W − total before i) / weight_i`; failing that, `i` = last and `f = 1`. Result =
  `lerp(f, previous step, step i)` for position and velocity, angle interpolation
  (shortest path) for yaw and pitch; the previous step of the first one is `E0`;
- render: when `C` is not empty, position, yaw and pitch of the minecart model = values
  interpolated at the render `pt`; passengers receive a render offset =
  (position interpolated by steps − classic interpolated position of the minecart); the camera of a
  passenger of an ordinary minecart is placed at the stepped position + seat offset + interpolated
  eye height.
The received batch thus plays over 3 ticks, each step occupying a share proportional to its weight.

### 5.7 `low_disk_space_warning` (0x33)
Empty. Sent by a dedicated server only to administrator players when the server's disk
is running out of space. Client: display (or restart, if already displayed) the
**system toast** "low disk space": title `chunk.toast.lowDiskSpace`, message
`chunk.toast.lowDiskSpace.description`. System toast: background `toast/system`, width =
max(160, widest line) + 30 (lines wrapped at 200), height = 20 + 12 × (title lines − 1)
+ 12 × max(message lines, 1); title in yellow `0xFFFFFF00` from (18, 7) (or (18, 12)
without a message), message in white below, 12 px per line, no shadow; stays displayed
`10,000 ms × toast display time` since the last (re)start. Manager:
`creative-inventory.md` § 13.1.

### 5.8 `server_data` (0x58)
Format: `MOTD: text component`, `icon: present boolean` then, if present,
`bytes: varint length + bytes` (PNG). Client, if the connection comes from a server list
entry: the entry's MOTD ← received (in memory); icon ← received if it is a valid PNG
of at most 1024 × 1024 (otherwise ignored, the old one stays); then, as a background task, reload
`servers.dat`, replace the entry with the **same name and same address** (visible list, otherwise hidden
list of direct connections) and rewrite the file. Nothing is displayed in game. List and
file: `title-screen-servers.md`.

## 6. Image effects (post-processing)

### 6.1 Effect of the observed entity
Triggered by the change of camera entity (`set_camera`, typically in spectator mode after a
click on an entity):
- creeper → effect `minecraft:creeper`;
- spider **and** cave spider → `minecraft:spider`;
- enderman → `minecraft:invert`;
- any other entity (including the player itself) or none → effect removed.
The effect is **active** as soon as it is set. Switching to third-person view (F5) removes it; returning to
first person recomputes it from the camera entity. The "Toggle Spectator Shader Effects"
key (`key.toggleSpectatorShaderEffects`, F4 by default) inverts the active state without
forgetting the effect. With no world loaded, the effect is removed.

### 6.2 Order and moment of application
On each frame, requested list = `minecraft:end_of_frame` (exists only if a resource pack
provides it; absent, ignored without a warning — this is the natural hook point for
shader packs), then the server's list (§ 5.3) in order, then the entity effect if it is
active. Applied only if there is a player, **after** all the world rendering, including the
first-person hand, the 3D overlays (fire, block in the head) and the glow
outlines, and **before** the interface (HUD, screens). An effect that cannot be found or that fails is set
aside (not requested again) until the next resource reload.

### 6.3 What the three effects do (all output an alpha of 1)
A "pass" reads one or more images (the main image or intermediate targets of
the same size) and writes into a target. Texture coordinates `u, v` ∈ [0, 1] with `v = 0` at the
**bottom** of the image (OpenGL convention: to be flipped under wgpu).

**`invert` (enderman)**: `c' = lerp(0.8, c, 1 − c) = 0.8 − 0.6·c` per channel; then copy
× (1, 1, 1). Negative image, reduced contrast.

**`creeper`**: pixelated green vision.
1. Colour matrix: `R = 0`, `G = 0.3·r + 0.59·g + 0.11·b`, `B = 0`; then saturation 1.8:
   `l = 0.3·R + 0.59·G + 0.11·B`, `c' = l + (c − l)·1.8`.
2. Mosaic and posterisation: each 4 × 4 pixel block takes the colour of its corner pixel
   (the bottom-left corner in texture coordinates); each channel is truncated to 16 levels
   (`c − frac(16·c)/16`); then saturation 1.5 (same formula).

**`spider`**: "multiple eyes" view.
1. Horizontal then vertical box blur of radius 15 of the image → `blur_large`; same with radius 7 →
   `blur_small`. Box blur of radius R = uniform average of `2R + 1` aligned pixels (the game
   gets it with bilinear reads between pixels).
2. Five successive "eye" passes, each laying a vignette over the result of the
   previous one (`background`). For a pass with parameters (source, scale `(sx, sy)`, offset
   `(ox, oy)`, rotation `ρ` degrees, crop `S = (x0, y0, x1, y1)`, fade `V = (x0, y0, x1, y1)`):
   sampling coordinate `s = rot(u, v; ρ) · (sx, sy) + (ox, oy)` with
   `rot = (u·cos ρ − v·sin ρ, v·cos ρ + u·sin ρ)`; colour = source(s); if `s` leaves `S`,
   colour = background(u, v); between the edge of `S` and that of `V`, linear mix from the background (at the edge
   of `S`) to the source (at the edge of `V`), axis by axis and applied successively for x0, y0,
   x1, y1.

   | # | source | background | scale | offset | ρ | crop S | fade V |
   |---|---|---|---|---|---|---|---|
   | 1 | sharp image | blur_large | (1.25; 2.0) | (−0.125; −0.1) | 0 | (0, 0, 1, 1) | (0.1; 0.1; 0.9; 0.9) |
   | 2 | blur_small | result 1 | (2.35; 4.2) | (−1.1; −1.5) | −45 | (0.21; 0; 0.79; 1) | (0.31; 0.1; 0.69; 0.9) |
   | 3 | blur_small | result 2 | (2.35; 4.2) | (0.45; −4.45) | +45 | (0.21; 0; 0.79; 1) | (0.31; 0.1; 0.69; 0.9) |
   | 4 | blur_small | result 3 | (2.35; 2.35) | (−0.385; −1.29) | 0 | (0, 0, 1, 1) | (0.31; 0.1; 0.69; 0.9) |
   | 5 | blur_small | result 4 | (2.35; 2.35) | (−0.965; −1.29) | 0 | (0, 0, 1, 1) | (0.31; 0.1; 0.69; 0.9) |

   Result: a large sharp eye in the centre (pass 1) on a very blurry background, four small blurry eyes
   around (two tilted at ±45°, two upright).
3. Final copy × (1, 0.8, 0.8): reddish tint.

The effects imposed by the server follow the resources' `post_effect/*.json` files; for
us, only these three (and `blur`, already described for the menus in `option-menus.md`) are to be
hard-wired; an unknown id is ignored. Possible graphics setting: video menu.

## 7. Uncertain points

- Lead rope: the two ribbons are emitted one after the other in the same triangle strip, and
  all the ropes of a frame share the same buffer; the junction triangles between ribbon A
  and ribbon B (and between two ropes) are either degenerate or cut by the driver. For us:
  one strip per ribbon (or indexed triangles), no junction.
- Vertical rope (`dx = dz = 0`): the half-thickness is undefined (division by zero) in the
  game; provide a guard (zero-thickness ribbon or arbitrary orientation).
- Pitch of the player's grab point: inverted, half-speed interpolation
  (`lerp(pt/2, current, previous)`), read as is; tiny visual difference, to be reproduced.
- Flying copy of the pickup: frozen at the time of the packet, hence without the bobbing or rotation
  of the item on the ground during the flight (code reading; to be confirmed by eye).
- Crosshair visible on top of the spyglass overlay: deduced from the order of the strata.
- `move_minecart_along_track`: only minecarts of the experimental mode; vanilla server without the
  feature → packet never received. Behaviour not verified in game.
- `post_effects` in the configuration phase: error on the official client; our choice (ignore)
  deliberately differs.
- Spider effect: vertical flip of the coordinates (OpenGL → wgpu) to be checked against a capture; the
  offsets in the table assume `v = 0` at the bottom.
- Observed-entity effect and server list: the same effect requested twice would be applied
  twice (no deduplication seen).
