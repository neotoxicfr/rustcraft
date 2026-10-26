# Particles (26.3, protocol 777)

Source: reading of the decompiled client (kept outside the repository), reworded.

Notation: `r` = uniform draw in [0, 1) (float), `g` = Gaussian draw (mean 0,
standard deviation 1), `rn(n)` = uniform integer in [0, n). Each particle has its own
generator; the factories receive the engine's. "tick" = 1/20 s. Velocities in blocks/tick.
Colours `(r, g, b)` as floats 0..1. Size = half-side of the quad (§ 2.4).

## 1. Network sources

### 1.1 `level_particles` (0x30, play, client-bound)

| field | type | note |
|---|---|---|
| particle | particle codec | type `id(minecraft:particle_type)` then options: see `entities.md` part 1 § 2.3 |
| force | `bool` | ignores distance and the setting ("override limiter") |
| always visible | `bool` | softens the "minimal" setting (see below) |
| x, y, z | 3 × `f64` | centre |
| dx, dy, dz | 3 × `f32` | spread per axis |
| vx, vy, vz | 3 × `f32` | maximum velocity **per axis** (new: three distinct values) |
| count | `varint` | |
| draw | `varint` | 0 default, 1 alternative, 2 alternative with velocity; out of range → 0 |

Client-side handling:

- `count = 0`: a single particle, exactly at `(x, y, z)`, velocity `(vx·dx, vy·dy, vz·dz)`
  (the spread then serves as the direction, the "velocity" as the scale).
- draw 0: for each particle, position `centre + (g·dx, g·dy, g·dz)`, velocity
  `(g·vx, g·vy, g·vz)` (6 independent Gaussian draws, in this order: 3 positions, 3 velocities).
- draw 1: position `centre + (r·dx, r·dy, r·dz)` (uniform, offset always positive),
  velocity `(vx, vy, vz)` as is.
- draw 2: as 1, then each velocity component is multiplied by an independent `r`.
- If creating a particle raises an error, the client logs it and abandons the rest of the
  packet.

The transmitted velocity is only a **parameter** of the type's factory: many types
normalise or ignore it (§ 2.2 and § 3).

### 1.2 Add filter (all sources going through the world's "add a particle")

1. Effective `force` = flag of the packet (or of the call) **or** flag of the type (list § 1.3).
   Forced → the particle is created without any other test.
2. Otherwise, if camera–position distance² > 1024 (32 blocks) → nothing.
3. Otherwise, the player's "Particles" setting: `minimal` → nothing; `decreased` → 1 chance in 3 of being
   treated as minimal (hence nothing); `all` → created. If "always visible" and setting minimal:
   1 chance in 10 of being treated as "decreased" (then again the 1/3 draw).
4. Creation by the type's factory; a factory may refuse (returns "nothing").

Exceptions: the block fragments of the full break (§ 1.4, 2001/2014/3008) and of mining
(2019/2020) are added **directly** to the engine, without distance filter or setting.

### 1.3 Identifiers and the type's "force" flag

Identifiers (built-in registry, verified against the `registries.json` report); `*` = type
forced by nature:

0 angry_villager · 1 block · 2 block_marker · 3 bubble · 4 sulfur_bubbles · 5 noxious_gas ·
6 noxious_gas_cloud · 7 geyser* · 8 geyser_base* · 9 geyser_poof* · 10 geyser_plume* · 11 cloud ·
12 copper_fire_flame · 13 crit · 14 damage_indicator* · 15 dragon_breath · 16 dripping_lava ·
17 falling_lava · 18 landing_lava · 19 dripping_water · 20 falling_water · 21 dust ·
22 dust_color_transition · 23 effect · 24 elder_guardian* · 25 enchanted_hit · 26 enchant ·
27 end_rod · 28 entity_effect · 29 explosion_emitter* · 30 explosion* · 31 gust* · 32 small_gust ·
33 gust_emitter_large* · 34 gust_emitter_small* · 35 sonic_boom* · 36 falling_dust · 37 firework ·
38 fishing · 39 flame · 40 infested · 41 cherry_leaves · 42 pale_oak_leaves · 43 red_poplar_leaves ·
44 orange_poplar_leaves · 45 yellow_poplar_leaves · 46 tinted_leaves · 47 sculk_soul ·
48 sculk_charge · 49 sculk_charge_pop* · 50 soul_fire_flame · 51 soul · 52 flash ·
53 happy_villager · 54 composter · 55 heart · 56 instant_effect · 57 item · 58 vibration · 59 trail ·
60 pause_mob_growth · 61 reset_mob_growth · 62 item_slime · 63 item_cobweb · 64 item_snowball ·
65 large_smoke · 66 lava · 67 mycelium · 68 note · 69 poof* · 70 portal · 71 rain · 72 smoke ·
73 white_smoke · 74 sneeze · 75 spit* · 76 squid_ink* · 77 sweep_attack* · 78 totem_of_undying ·
79 underwater · 80 splash · 81 witch · 82 bubble_pop · 83 current_down · 84 bubble_column_up ·
85 nautilus · 86 dolphin · 87 campfire_cosy_smoke* · 88 campfire_signal_smoke* · 89 dripping_honey ·
90 falling_honey · 91 landing_honey · 92 falling_nectar · 93 falling_spore_blossom · 94 ash ·
95 crimson_spore · 96 warped_spore · 97 spore_blossom_air · 98 dripping_obsidian_tear ·
99 falling_obsidian_tear · 100 landing_obsidian_tear · 101 reverse_portal · 102 white_ash ·
103 small_flame · 104 snowflake · 105 dripping_dripstone_lava · 106 falling_dripstone_lava ·
107 dripping_dripstone_water · 108 falling_dripstone_water · 109 glow_squid_ink* · 110 glow* ·
111 wax_on* · 112 wax_off* · 113 electric_spark* · 114 scrape* · 115 shriek · 116 egg_crack ·
117 dust_plume · 118 trial_spawner_detection* · 119 trial_spawner_detection_ominous* ·
120 vault_connection* · 121 dust_pillar · 122 ominous_spawning* · 123 raid_omen · 124 trial_omen ·
125 block_crumble · 126 firefly · 127 sulfur_cube_goo.

Additional useful options (the network codec is in `entities.md`): `dust` scale clamped
to [0.01, 4] on construction; `entity_effect` and `flash` carry an ARGB (alpha used);
`effect` / `instant_effect` carry an RGB and a power.

### 1.4 `level_event` (0x2F) — events producing particles

Format: `type: i32`, `position: block position` (packed `i64`), `data: i32`,
`global: bool`. `(px, py, pz)` = corner of the block. Only the events with particles are listed;
the associated sounds are mentioned without detail.

| type | effect |
|---|---|
| 1500 | composter filling: `composter` particles (detail not noted) |
| 1501 | lava extinguished (+ sound): 8 × `large_smoke` at `(px + r, py + 1.2, pz + r)`, velocity 0 |
| 1502 | redstone torch burned out (+ sound): 5 × `smoke` at `p + 0.2 + 0.6·r` per axis, velocity 0 |
| 1503 | End portal frame filled (+ sound): 16 × `smoke` at `(px + (5 + 6r)/16, py + 0.8125, pz + (5 + 6r)/16)` |
| 1504 | dripstone drip: drip particle at the tip (detail not noted) |
| 1505 | bone meal (+ sound); data = n. "Growing" plant: `happy_villager` × n, scattered in the block (helper A, width 0.5, height = top of the shape or 1 if air, floats allowed). "Spreading" plant (grass…) or water block: × 3n, helper A width 3, height 1, **only** if the block under the point is not air |
| 2000 | shot smoke: helper B with `smoke`; data = face (0 down, 1 up, 2 north, 3 south, 4 west, 5 east) |
| 2001 | block break (+ break sound: volume `(v + 1)/2`, pitch `× 0.8`); data = state id; fragments: helper C |
| 2002 | splash potion shattered: helper D with `effect`; data = RGB |
| 2003 | Eye of Ender destroyed: 8 × `item` (Eye of Ender) at `(px + 0.5, py, pz + 0.5)`, velocity `(0.15g, 0.2r, 0.15g)`; then for 40 angles `a = k·π/20`: 2 × `portal` at `centre + (5 cos a, −0.4, 5 sin a)`, "velocity" `(−5 cos a, 0, −5 sin a)` then `(−7 cos a, 0, −7 sin a)` |
| 2004 | spawner spawn: 20 × (`smoke` then `flame`) at `centre + (r − 0.5)·2` per axis, velocity 0 |
| 2006 | dragon breath: 200 × `dragon_breath` (power = distance), same layout as helper D; sound if data = 1 |
| 2007 | instant potion: helper D with `instant_effect` |
| 2008 | block destroyed by the dragon: 1 × `explosion` at the block centre |
| 2009 | water evaporated: 8 × `cloud` at `(px + r, py + 1.2, pz + r)` |
| 2010 | shot white smoke: helper B with `white_smoke` |
| 2011 / 2012 | bee grows a plant / turtle egg placed: `happy_villager` × data, helper A width 0.5 in the block |
| 2013 | smash attack (mace): `dust_pillar` of the block state, n = data, `c` = block centre + (0, 0.5, 0); for integer `i` while `i < n/3` (float): position `c + (g/2, 0, g/2)`, velocity `0.2·g` per axis; then, `i` starting again from 0, while `i < n/1.5`: position `c + (3.5·cos i + g/2, 0, 3.5·sin i + g/2)`, velocity `0.05·g` per axis |
| 2014 | block fragments only (no sound); data = state id; helper C |
| 2015 / 2016 / 2017 | teleportation (dragon egg / shulker / consumption effect): helper E; data = packed offset of radii (16, 8, 16) / (8, 8, 8) / (127, 127, 127) |
| 2018 | enderman teleportation: 128 × `portal` like helper E but offsets `(r − 0.5)·0.6·2` in x/z and `r·2.9` in y |
| 2019 / 2020 | mining progress (2020 additionally plays the hit sound: volume `(v + 1)/8`, pitch `× 0.5`); data = face (clamped 0..5); helper F |
| 3000 | End gateway spawn: `explosion_emitter` forced, always visible, at the block centre (+ sound) |
| 3002 | sparks (lightning rod): data ∈ [0, 2] = axis (x, y, z) → 10..19 × `electric_spark` along the axis (helper G, radius 0.125); otherwise 3..5 per face (helper H) |
| 3003 / 3004 / 3005 | wax applied / wax removed / scraping: 3..5 per face (helper H) of `wax_on` / `wax_off` / `scrape` |
| 3006, 3007 | sculk charge, shrieker screech: `sculk_charge(_pop)`, `shriek` particles (detail not noted) |
| 3008 | brushing finished: helper C with the brushed state |
| 3009 | egg cracked: 3..6 per face (helper H) of `egg_crack` |
| 3011–3021 | trial spawner, vault, cobweb (`poof`): not detailed |

Helpers (positions in world coordinates):

- **A (scattering within a block)** — for n particles: velocity `(0.02g, 0.02g, 0.02g)`, position
  `(px + 0.5 − L + 2L·r, py + H·r, pz + 0.5 − L + 2L·r)`.
- **B (shot smoke)** — `n` = vector of the face; 10 times: `p = 0.2r + 0.01`;
  `x = px + 0.61·nx + 0.5 + (r − 0.5)·nz·0.5`; `y = py + 0.61·ny + 0.5 + (r − 0.5)·ny·0.5`;
  `z = pz + 0.61·nz + 0.5 + (r − 0.5)·nx·0.5`; velocity `n·p + 0.01g` per axis. (The spread in y
  uses `ny`, not a tangent component: reproduce as is.)
- **C (full break)** — if the state is not air and produces fragments: for each
  box of the shape (not the collision shape): `l = min(1, size)` per axis,
  `c = max(2, ⌈l / 0.25⌉)` cells per axis; for each cell `(i, j, k)`: `rel = (i + 0.5)/c`
  per axis, `block` particle of the state at `block_corner + box_corner + rel·l`, velocity parameter
  `rel − 0.5` per axis. A full block gives 4 × 4 × 4 = 64 fragments.
- **D (potion splash)** — centre = middle of the bottom of the block; 8 × `item` (splash potion),
  velocity `(0.15g, 0.2r, 0.15g)`; then 100 times: `d = 4r`, `a = 2π·r`, velocity
  `(d cos a, 0.01 + 0.5r, d sin a)`, position `centre + (0.1·vx, 0.3, 0.1·vz)`, colour
  `RGB × (0.75 + 0.25r)`, power `d`.
- **E (teleportation along a segment)** — destination = position + unpacked offset
  (`dx = ((data >> 16) & 0xFF) − Rx`, `dy = ((data >> 8) & 0xFF) − Ry`,
  `dz = (data & 0xFF) − Rz`); 128 times: `t = r` (double), "velocity" `(r − 0.5)·0.2` per axis,
  position `lerp(t, destination, origin) + (r − 0.5 + 0.5, r − 0.5, r − 0.5 + 0.5)`.
- **F (mining fragment)** — if the state is neither air nor invisible and produces fragments:
  `b` = bounding box of the shape; position uniform in `b` shrunk by 0.1 on each side,
  then the coordinate of the face's axis is set to 0.1 **outside** `b` on that face;
  `block` particle created with zero velocity parameter, then power 0.2 and scale 0.6
  (§ 2.2). The server emits this event on every mining tick to all nearby clients,
  2020 every 4 ticks and 2019 otherwise; the mining client adds one itself on the first hit.
- **G (along an axis)** — block centre + `u·0.5` on the axis and `u·radius` on the other two
  (`u` uniform in [−1, 1] per axis); velocity uniform [−1, 1] on the axis, 0 elsewhere.
- **H (on the faces)** — for each of the 6 faces, k times (k drawn per face): position = centre
  + `0.55·normal` on the face's axis, uniform [−0.5, 0.5] on the other two; velocity
  uniform [−0.5, 0.5] on the tangent axes, 0 on the normal.

### 1.5 `entity_event` (0x22) and `animate` (0x02) — cases with particles

Entity notations: `X(s) = x + width·s`, `Y(s) = y + height·s`, `rX(s) = X((2r − 1)·s)`,
`rY = Y(r)`.

| event | entity | effect |
|---|---|---|
| 0 | tipped arrow | if colour ≠ −1: 20 × `entity_effect` (arrow's colour, alpha 1) at `(rX(0.5), rY, rZ(0.5))`, velocity 0 |
| 3 | snowball | 8 × particle of the item (`item_snowball` if item empty, otherwise `item` of the item) at the position, velocity 0 |
| 3 | egg | 8 × `item` of the item, velocity `(r − 0.5)·0.08` per axis |
| 6 / 7 | tameable animal | taming failed / succeeded: 7 × `smoke` / `heart` at `(rX(1), rY + 0.5, rZ(1))`, velocity `0.02g` per axis |
| 12 / 13 / 14 / 42 | villager | 5 × `heart` / `angry_villager` / `happy_villager` / `splash` at `(rX(1), rY + 1, rZ(1))`, velocity `0.02g` |
| 15 | witch | `10 + rn(35)` × `witch` at `(x + 0.13g, box_top + 0.5 + 0.13g, z + 0.13g)`, velocity 0 |
| 17 | firework rocket | explosion trigger (§ 3, firework) with the rocket's velocity and its explosions |
| 18 | animal | "in love": 7 × `heart` like 6/7 |
| 20 | mob | spawn: like 60 |
| 35 | any entity (local player included) | totem: follower emitter `totem_of_undying` for 30 ticks (+ sound; local player: item animation) |
| 46 | living entity | teleportation: 128 × `portal`, `t = i/127`, position `lerp(t, old, current) + ((r − 0.5)·2·width, r·height, (r − 0.5)·2·width)`, "velocity" `(r − 0.5)·0.2` per axis |
| 47–52, 65, 68 | living entity | item broken (hand, off-hand, head, chest, legs, feet, body, saddle): sound + 5 × `item` (helper I) |
| 53 / 54 | entity | slide / jump on a honey block (detail not noted) |
| 60 | living entity | death "poof": 20 times, `v = 0.02g` per axis, `poof` at `(rX(1) − 10·vx, rY − 10·vy, rZ(1) − 10·vz)`, velocity `v` |
| 67 | living entity | drowning: 8 × `bubble` at `position + (t, t, t)` (`t` triangular with mean 0 and extent ±1, drawn per axis), velocity = the entity's movement |
| others | fox (45, item eaten), wolf (8, shaking), dolphin, iron golem, rabbit, squid, ocelot, horse, allay, armor stand, creaking, ravager, warden, evoker fangs, bobber, boat, TNT minecart | not detailed |

**Helper I (item broken)**: for each particle, `d = ((r − 0.5)·0.1, 0.1r + 0.1, 0)` then
rotation of `−pitch` around X then `−yaw` around Y (degrees → radians); `p = ((r − 0.5)·0.3,
−0.6r − 0.3, 0.6)` same rotations, then `+ (x, y_eyes, z)`; velocity `(dx, dy + 0.05, dz)`.

`animate`: action 1 → follower emitter `crit`, action 2 → follower emitter `enchanted_hit`
(duration 3 ticks).

**Follower emitter** (no rendering): on creation it immediately runs a tick; on each
tick, 16 attempts: `a = (2r − 1)` per axis; if `|a|² ≤ 1`: particle at
`(X(ax/4), Y(0.5 + ay/4), Z(az/4))` of the entity **at that moment**, velocity parameter
`(ax, ay + 0.2, az)` (through the filter § 1.2). Removed when its counter reaches its duration (3 by
default). Emitters live in a separate list, ticked after the particle groups.

### 1.6 Potion effects (metadata)

Living entity, index 10 (particle list) and 11 (ambient), see `entities.md`. On each client
tick, if the list is not empty: bound = 15 if the entity is invisible, 4 otherwise; ×5 if
ambient; if `rn(bound) = 0`, a particle drawn at random from the list at
`(rX(0.5), rY, rZ(0.5))` with velocity parameter `(1, 1, 1)`. The server encodes each effect as
`entity_effect` of ARGB colour = the effect's colour with alpha 255, or 38 (= ⌊38.25⌋) if the effect
is ambient (beacon).

### 1.7 Other client-side sources (non-network, for the record)

- Rain: on each tick, if rain level `p > 0`: `n = ⌊0.225 · (2R + 1)² · p²⌋` (÷ 2 with the decreased
  setting; R = weather radius of the setting), columns drawn in the square around the
  camera, height = "blocks motion" map, kept if within ±10 of the camera and biome
  rainy; `rain` particle (or `smoke` on lava, magma block, lit campfire) at the top
  of the collision shape / of the fluid. With the minimal setting: no particle (sound only).
- Sprint: sprinting entity (not in water, lava, crouching, spectator): `block` of the block under the
  feet at `(x ± width/2, y + 0.1, z ± width/2)`, velocity `(−4·vx, 1.5, −4·vz)`.
- Ambience of blocks and fluids: § 4. Biome particles (environment attribute
  "ambient particles": `ash` 0.00625 soul sand valley, `white_ash` ≈ 0.1181 basalt deltas,
  `crimson_spore` 0.025, `warped_spore` 0.01428): § 4.

## 2. Engine

### 2.1 Groups, limits, adding

- Four groups according to the rendering kind: simple quads (all the types of § 3), item pickup,
  elder guardian, no rendering (emitters). Drawing order: quads, pickup, guardian.
- Each group is a FIFO queue of capacity **16,384**. On insertion: full → refused;
  from 12,288 elements on, accepted only if `r < ((16384 − n)/4096)²` (engine draw).
- Limit per category: `spore_blossom_air` (and other carriers of the category) ≤ 1000 alive;
  counter incremented on entering the waiting queue, decremented on death or refusal.
- New particles go into a waiting queue and only enter their group **at the end** of the
  engine tick (so they are not ticked on the tick they are created). Exception: a particle
  that calls its own tick in its constructor (`crit`, emitters).
- Engine tick: each group in insertion order → tick of each particle, removal
  of the dead; then emitters; then flushing of the waiting queue.
- World change: everything is cleared.

### 2.2 Common physics

Base construction (all particles): box 0.2 × 0.2 centred in x/z, base in y;
`old = position`; lifetime `⌊4 / (0.9r + 0.1)⌋` (4..40); gravity 0; friction 0.98;
collisions active; no ceiling acceleration.

"With velocity" construction (parameter `(a, b, c)`): in addition,
1. `v = (a, b, c) + ((2r − 1)·0.4)` per axis;
2. `s = (r + r + 1)·0.15`; `v = v / |v| · s · 0.4`;
3. `vy += 0.1`.
The final norm is therefore 0.06..0.18 (+ 0.1 in y) whatever the parameter, which only fixes the
direction. The "without velocity" types use the first form and set their velocity themselves.

Power `k`: `vx ×= k`, `vy = (vy − 0.1)·k + 0.1`, `vz ×= k`.
Scale `e`: quad size `× e`, box `0.2e × 0.2e`.

Standard tick:
1. `old = position`;
2. if `age++ ≥ lifetime` → dead (the particle lives `lifetime` movement ticks);
3. `vy −= 0.04 · gravity`;
4. movement by `v`;
5. if ceiling acceleration and `y` unchanged: `vx, vz ×= 1.1`;
6. `v ×= friction` (3 axes);
7. if on the ground: `vx, vz ×= 0.7`.

Movement by `(a, b, c)`:
- if the particle is "frozen" → nothing;
- if collisions are active, movement non-zero and `|m|² < 10,000`: collision of the box against the
  blocks' collision shapes (same algorithm as entities, without entities, "position only"
  context at height `y`);
- box moved by the resulting movement, position = (centre x, bottom y, centre z) of the box;
- if `|b| ≥ 1e−5` and `|resulting b| < 1e−5` → **frozen for good** (no further movement,
  the tick continues);
- on the ground = (b was modified) and `b < 0`;
- x (resp. z) component cut → `vx = 0` (resp. `vz = 0`).

Several types replace the movement by "no collision" (box simply translated):
this is noted "passes through" in § 3. Beware: a type that modifies `x, y, z` directly without
resetting the box loses this offset on the first movement (case of the flame, § 3).

### 2.3 Sprites and atlas

- Particle atlas: all the PNGs of `textures/particle/` (source "directory",
  empty prefix); sprite identifier `minecraft:<name>`. Only `vibration.png` carries a
  `.mcmeta` animation (classic atlas animation).
- Definition per type: `particles/<type>.json` = `{"textures": [ordered list]}`. A type without a
  file has no sprite set (`block`, `item`, `explosion_emitter`…). Missing sprite → the atlas's
  "missing" sprite.
- Choice: random `list[rn(n)]` (fixed for the lifetime), first element, or **by age**:
  `list[age·(n − 1) / lifetime]` (integer division), recomputed on each tick by the animated types
  (not recomputed once dead).
- Block fragments: "particle" sprite of the state's model (blocks atlas). Item fragments:
  particle material of the item model in "ground" context, drawn at random from
  those of the model (items atlas; failing that the missing sprite of that atlas).

### 2.4 Quad, interpolation, colour, light

- Default size `0.1·(0.5r + 0.5)·2` (0.1..0.2). The quad goes from `−size` to `+size` on
  its two axes (side = 2 × size).
- Rendered position: `lerp(partial_tick, old, position) − camera`.
- Orientation: camera rotation quaternion (facing the camera, on 3 axes); if roll
  ≠ 0, additional rotation around Z of `lerp(t, old_roll, roll)`. (A "face
  camera around Y only" mode exists, not used by the types of § 3.)
- Vertices, in order: `(+1, −1) → (u1, v1)`, `(+1, +1) → (u1, v0)`, `(−1, +1) → (u0, v0)`,
  `(−1, −1) → (u0, v1)`; corner = rotation(corner) × size + position.
- Vertex colour = ARGB of `(alpha, r, g, b)` floats (default opaque white).
- Default light: light coordinates of the block containing the position (sky and
  block light; a block with emissive rendering gives full lighting; the block's own emission raises
  the block light); chunk absent → sky 15, block 0. Variants:
  - "block 15": block light forced to 15, sky kept;
  - "full": sky 15, block 15 (0xF000F0);
  - "growing emission f": in smooth units (block × 16), `block = min(block + ⌊clamp(f)·240⌋, 240)`.
- Visibility: only the (non-interpolated) position is tested against the view frustum.

### 2.5 Layers and render passes

- Layer = (translucent?, atlas). Six layers: opaque / translucent × blocks, items,
  particles atlas. Block and item fragments choose according to their sprite
  (translucent if the sprite has semi-transparent pixels); the other types fix their layer
  (§ 3).
- Opaque: drawn in the solid pass (with the opaque entities), no blending. Translucent:
  "after the terrain" pass (see `entities.md`, pass order), classic alpha blending;
  with improved transparency (OIT) active, OIT variant. **No sorting** by distance: insertion
  order in the queues, grouped by layer.
- Shader: colour = texture × vertex colour × modulator; **reject if alpha < 0.1**;
  vertex colour previously multiplied by the lightmap sample of the light
  coordinates; standard fog (spherical and cylindrical). Same depth state for both
  layers (default test and write).

## 3. Parameters per type

Abbreviations: **life** = lifetime (ticks); **V** = initial velocity; "base(p)" = construction
with velocity of parameter `p` (§ 2.2); **G** = gravity (subtracts `0.04·G` per tick unless noted
"direct"); **F** = friction; **T** = size; "grows" = size × `clamp((age + t)/life·32, 0, 1)`
(appearance over the first 1/32 of the life); **anim** = sprite according to age; **rand** = fixed
random sprite; layer **O** opaque / **Tr** translucent (particles atlas unless noted). Normal light
unless noted.

#### block (block fragments)
Refused if state = air, moving piston, or block without fragments. base(p), G 1, F 0.98, colour
0.6 × block tint (index 0 "particle" tint if the block is tinted: grass,
leaves…), T / 2, UV: sub-square of 1/4 of the sprite at offset `(uo, vo) = (3r, 3r)`:
`u0 = U((uo + 1)/4)`, `u1 = U(uo/4)`, `v0 = V(vo/4)`, `v1 = V((vo + 1)/4)` (u inverted). Base life.
Layer according to the sprite (blocks atlas). Variants: `block_crumble` (velocity 0, life `rn(10) + 1`),
`dust_pillar` (V = `(g/30, p_y + g/2, g/30)`, life `20 + rn(20)`).

#### item (item fragments), item_snowball, item_slime, item_cobweb
Construction without parameter then base(0) ×0.1 + parameter: `v = 0.1·base(0) + p`. G 1, T / 2,
UV sub-square 1/4 like `block`, base life, layer according to the sprite (items atlas). The three simple
variants use the snowball / slime ball / cobweb, **without** adding the
parameter (pure base(0) velocity).

#### poof
Without base velocity: `v = p + (2r − 1)·0.05` per axis. G −0.1 (rises), F 0.9, grey
`0.7 + 0.3r`, T `0.1·(6·r·r + 1)`, life `⌊16/(0.8r + 0.2)⌋ + 2`, anim `generic_7 → generic_0`
(shrinks), O.

#### smoke, large_smoke ("ash/smoke" template)
Template (dir, max_colour, max_life, G, collisions): base(0) then `v = v·dir + p`; grey
`r·max_colour`; T × `0.75·scale`; life `max(1, ⌊max_life/(0.8r + 0.2)·scale⌋)`; F 0.96;
ceiling acceleration; grows; anim; O.
- `smoke`: dir (0.1, 0.1, 0.1), max_colour 0.3, max_life 8, G −0.1, collisions, scale 1;
  sprites `generic_7 → generic_0`.
- `large_smoke`: same with scale 2.5.
- `white_smoke`: same family (not noted in detail).

#### ash, white_ash
Template above, dir (0.1, −0.1, 0.1), max_life 20, no collisions, scale 1, single sprite
`generic_0`.
- `ash`: parameter (0, 0, 0), max_colour 0.5, G 0.1.
- `white_ash`: fixed colour `0xBAB1C2`, G 0.0125; parameter drawn by the factory:
  `(−1.9·r·r·0.1, −0.5·r·r·0.5, −1.9·r·r·0.1)`.

#### flame, soul_fire_flame, small_flame, copper_fire_flame
base(p) then `v = 0.01·v + p`; position `+ (r − r)·0.05` per axis (without resetting the box: the
offset disappears on the first movement); life `⌊8/(0.8r + 0.2)⌋ + 4`; F 0.96; G 0; passes through;
T × `(1 − s²/2)` with `s = (age + t)/life`; light "growing emission s"; O; sprite
`flame` / `soul_fire_flame` / `copper_fire_flame`. `small_flame`: scale 0.5.

#### lava
base(0) ×0.8 then `vy = 0.4r + 0.05`; G 0.75; F 0.999; T × `(0.2 + 2r)`; life
`⌊16/(0.8r + 0.2)⌋`; T × `(1 − s²)`; light "block 15"; O; sprite `lava`. On each tick,
if alive, with probability `1 − age/life`: a `smoke` at its position with its velocity.

#### dripping_water, dripping_lava (hanging drop)
Without velocity, box 0.01, G 0.06 × 0.02 = 0.0012 (direct), life 40 (counted down **before** the
movement); after movement `v ×= 0.02` then `v ×= 0.98`; on expiry: dead + the "falling" particle
at its position with its velocity. If inside a fluid of the same type under its surface →
dead. O; sprite `drip_hang`.
- water: colour (0.2, 0.3, 1) → `falling_water`.
- lava: cooling colour, recomputed before each countdown (`n` = remaining life):
  `(1, 16/(40 − n + 16), 4/(40 − n + 8))` → `falling_lava`.

#### falling_water, falling_lava (falling drop)
G 0.06 (direct), F 0.98, life `⌊64/(0.8r + 0.2)⌋` counted down; on the ground: dead + `splash` (water) or
`landing_lava` at its position, velocity 0. Water colour (0.2, 0.3, 1), lava (1, 0.2857, 0.0833).
Sprite `drip_fall`. `landing_lava`: life `⌊16/(0.8r + 0.2)⌋`, same physics, sprite `drip_land`.

#### rain (and base of splash)
base(0) then `vx, vz ×= 0.3`, `vy = 0.2r + 0.1`; box 0.01; G 0.06 (direct); life
`⌊8/(0.8r + 0.2)⌋` counted down; F 0.98; on the ground: 1 chance in 2 of dying on each tick, then
`vx, vz ×= 0.7`; dead if its height within the block is below the top of the collision shape or of the
fluid. O; rand among `splash_0..3`.

#### splash
Like `rain` but G 0.04 (direct); if `p_y = 0` and `(p_x, p_z) ≠ 0`: `v = (p_x, 0.1, p_z)`.

#### bubble
Without velocity: `v = 0.2·p + (2r − 1)·0.02` per axis; box 0.02; T × `(0.6r + 0.2)`; life
`⌊8/(0.8r + 0.2)⌋` counted down; tick: `vy += 0.002`, movement, `v ×= 0.85`; dead out of
water (fluid of the current block). O; sprite `bubble`.

#### bubble_pop
`v = p`; life 4; G 0.008 (direct), no friction; anim `bubble_pop_0..4`; O.

#### crit, enchanted_hit, damage_indicator
base(0) ×0.1 then `+ 0.4·p`; F 0.7; G 0.5; grey `0.6 + 0.3r`; T × 0.75; life
`max(1, ⌊6/(0.8r + 0.6)⌋)`; passes through; grows; **one tick run in the constructor**; on each
tick `v ×= 0.96`, `b ×= 0.9` (turns yellow-orange); O.
- `crit`: sprite `critical_hit`.
- `enchanted_hit`: `r ×= 0.3`, `v ×= 0.8` on creation; sprite `enchanted_hit`.
- `damage_indicator`: parameter `p_y + 1`, life 20; sprite `damage`.

#### heart, angry_villager
base(0) ×0.01 then `vy += 0.1`; F 0.86; ceiling acceleration; T × 1.5; life 16; passes through;
grows; O. `heart`: sprite `heart`. `angry_villager`: position `y + 0.5`, sprite `angry`.

#### happy_villager (and composter, egg_crack, mycelium, dolphin)
base(p) ×0.02; box 0.02; T × `(0.6r + 0.5)`; grey `0.2 + 0.1r`; life `⌊20/(0.8r + 0.2)⌋`
counted down; tick: movement that passes through, `v ×= 0.99`, neither gravity nor standard friction; O.
- `happy_villager`, `egg_crack`: white colour; sprite `glint` (happy).
- `composter`: white, life `3 + rn(5)`.
- `mycelium`: grey, sprite `generic_0`.
- `dolphin`: colour (0.3, 0.5, 1), alpha `1 − 0.7r`, life / 2.

#### note
base(0) ×0.01 then `vy += 0.2`; F 0.66; ceiling acceleration; collisions; T × 1.5; life 6;
grows; colour from `c = p_x` (pitch / 24): `r = max(0, 0.65·sin(2π·c) + 0.35)`,
`g = max(0, 0.65·sin(2π(c + 1/3)) + 0.35)`, `b = max(0, 0.65·sin(2π(c + 2/3)) + 0.35)`; O;
sprite `note`.

#### portal (and reverse_portal not detailed)
The parameter is an **offset**, not a velocity. Start `S` = given position; T
`0.1·(0.2r + 0.5)`; brightness `c = 0.6r + 0.4`, colour `(0.9c, 0.3c, c)`; life `40 + ⌊10r⌋`;
tick: `q = age/life` (after increment), `f = 1 − (2q² − q)`, position `S + p·f + (0, 1 − q, 0)`;
T × `(1 − (1 − s)²)`; light "growing emission (age/life)⁴"; O; sprite
`generic_0 → generic_7` chosen at creation at random (rand).

#### enchant (and nautilus, vault_connection)
The parameter is an offset: the particle is born at `S + p` and converges towards `S`. T
`0.1·(0.5r + 0.2)`; brightness `c = 0.6r + 0.4`, colour `(0.9c, 0.9c, c)`; passes through; life
`30 + ⌊10r⌋`; tick: `k = 1 − age/life`, `e = (age/life)⁴`, position `S + p·k − (0, 1.2·e, 0)`;
light "growing emission (age/life)⁴"; O; rand among `sga_a..sga_z`. `vault_connection`:
alpha 0 → 0.6 between 25 % and 100 % of the life (Tr), light "block 15", scale 1.5.

#### explosion
Static; life `6 + rn(4)`; grey `0.4 + 0.6r`; T `2·(1 − p_x/2)`; full light; anim
`explosion_0..15`; O.

#### explosion_emitter
No rendering; life 8; on each tick: 6 × `explosion` at `position + (r − r)·4` per axis (double
draws), parameter `(age/8, 0, 0)`; `age += 1`; dead when `age = 8`.

#### cloud (and sneeze)
base(0) ×0.1 + `p`; F 0.96; grey `1 − 0.3r`; T × 1.875; life
`max(1, ⌊⌊8/(0.8r + 0.3)⌋·2.5⌋)`; passes through; grows; anim `generic_7 → generic_0`; **Tr**. If a
player is within 2 blocks and under the particle: `y += (y_player − y)·0.2`,
`vy += (vy_player − vy)·0.2`. `sneeze`: colour (0.22, 1, 0.53), alpha 0.4.

#### dust
base(p) ×0.1; F 0.96; ceiling acceleration; T × `0.75·scale`; life
`max(1, ⌊⌊8/(0.8r + 0.2)⌋·scale⌋)`; grows; colour = RGB of the options × factor
`f = 0.4r + 0.6` × `(0.2r + 0.8)` per channel; anim `generic_7 → generic_0`; O.
`dust_color_transition`: same base, interpolated colour (not detailed).

#### entity_effect, effect, instant_effect, witch ("spell" template)
Construction base(`(0.5 − r', p_y, 0.5 − r')`) with `r'` drawn from a generator **shared** by all
spells; then `vy ×= 0.2`; if `p_x = 0` and `p_z = 0`: `vx, vz ×= 0.1`; F 0.96; G −0.1 (rises);
ceiling acceleration; T × 0.75; life `⌊8/(0.8r + 0.2)⌋`; passes through; anim; **Tr**. Alpha:
each tick `α = lerp(0.05, α, α_origin)`; α = 0 if the local player is looking through a spyglass in
first person and the particle is within 3 blocks of their eyes.
- `entity_effect`: colour and alpha from the ARGB; sprites `effect_7 → effect_0`.
- `effect`: RGB + power (§ 2.2); sprites `effect_7 → effect_0`.
- `instant_effect`: same; sprites `spell_7 → spell_0`.
- `witch`: colour `(c, 0, c)`, `c = 0.5r + 0.35`; sprites `spell_7 → spell_0`.

#### campfire_cosy_smoke, campfire_signal_smoke
Without velocity; scale 3 then box 0.25; life `80 + rn(50)` (signal: `280 + rn(50)`); G 3e−6
(direct); `v = (p_x, p_y + r/500, p_z)`; tick: `vx, vz += ±r/5000` (sign drawn), `vy −= G`,
movement **with** collisions, no friction; in the last 60 ticks and if α > 0.01:
`α −= 0.015`; dead if age ≥ life or α ≤ 0. Initial alpha 0.9 (signal 0.95). **Tr**; rand among
`big_smoke_0..11`. The campfire emits `p = (0, 0.07, 0)` (forced, always visible).

#### falling_dust
Refused if the state is invisible (and not air). Without velocity; colour = dust colour of the block
if it falls (sand…), otherwise "particle" tint, otherwise map colour; T × 0.675; life
`max(1, ⌊⌊32/(0.8r + 0.2)⌋·0.9⌋)`; initial roll `2π·r`, roll speed `(r − 0.5)·0.1`;
tick: anim, `old_roll = roll`, `roll += 2π·roll_speed` (reset to 0 on the ground), movement,
then `vy −= 0.003`, `vy = max(vy, −0.14)`; no friction; grows; anim
`generic_7 → generic_0`; O.

#### snowflake
Without velocity: `v = p + (2r − 1)·0.05`; G 0.225; F 1; then each tick `vx, vz ×= 0.95`,
`vy ×= 0.9`; T `0.1·(r·r + 1)`; life `⌊16/(0.8r + 0.2)⌋ + 2`; colour (0.923, 0.964, 0.999);
anim `generic_7 → generic_0`; O.

#### glow (and wax_on, wax_off, scrape, electric_spark)
Template: base(p); F 0.96; ceiling acceleration; T × 0.75; passes through; light "growing
emission (age + t)/life"; anim; O.
- `glow`: base(`(0.5 − r, p_y, 0.5 − r)`), colour (0.6, 1, 0.8) or (0.08, 0.4, 0.4) (½);
  `vy ×= 0.2`; if `p_x = p_z = 0`: `vx, vz ×= 0.1`; life `⌊8/(0.8r + 0.2)⌋`; sprite `glow`.
- `electric_spark`: colour (1, 0.9, 1), velocity `0.25·p`, life `2 + rn(2)`.
- `wax_on` (0.91, 0.55, 0.08) / `wax_off` (1, 0.9, 1): velocity `(0.005·p_x, 0.01·p_y, 0.005·p_z)`,
  life `10 + rn(30)`.
- `scrape`: (0.29, 0.58, 0.51) or (0.43, 0.77, 0.62), velocity `0.01·p`, life `10 + rn(30)`.

#### totem_of_undying, end_rod ("simple animated" template)
Template: without velocity, F 0.91; full light; **Tr**; anim; after half the life
(`age > ⌊life/2⌋`): `α = 1 − (age − ⌊life/2⌋)/life`, and if there is a fade colour: each component
`+= (fade − c)·0.2`.
- `totem_of_undying`: G 1.25, F 0.6, `v = p`, T × 0.75, life `60 + rn(12)`; colour: 1 time in 4
  `(0.6 + 0.2r, 0.6 + 0.3r, 0.2r)`, otherwise `(0.1 + 0.2r, 0.4 + 0.3r, 0.2r)`; sprites
  `glitter_7 → glitter_0`.
- `end_rod`: G 0.0125, `v = p`, T × 0.75, life `60 + rn(12)`, fade towards `0xF2DEC9`, passes through;
  sprites `glitter_7 → glitter_0`.

#### sweep_attack
Static; life 4; grey `0.4 + 0.6r`; T `1 − p_x/2`; full light; anim `sweep_0..7`; O.

#### white_smoke, dragon_breath, sculk_charge(_pop), shriek, trial detection, omens, vault, dripstone drops (noted 13/10)
- `white_smoke`: `smoke` template (direction 0.1, max colour 0.3, life 8, G −0.1, passes through), then
  colour set to `(0.729, 0.694, 0.761)`; O; grows; anim `generic_7 → generic_0`.
- `dragon_breath`: option = power (network float). `v = p`, then power (§ 2.2); F 0.96;
  colour `(0.718..0.875, 0, 0.824..0.976)`; T × 0.75; life `⌊20/(0.8r + 0.2)⌋`; no collision;
  tick: anim `generic_5 → generic_7`, movement, if no vertical movement `vx, vz ×= 1.1`, then
  `vx, vz ×= 0.96`; grows; O.
- `sculk_charge`: option = roll (network float). Imposed velocity `p`, F 0.96, T × 1.5, life `rn(12) + 8`,
  no collision, block light 15, **Tr**; anim `sculk_charge_0..6`. `sculk_charge_pop`: T × 1, life
  `rn(4) + 6`, anim `sculk_charge_pop_0..3`.
- `shriek`: option = delay (varint). T 0.85, life 30, `v = (0, 0.1, 0)`, G 0; nothing happens during
  the delay; α = `1 − age/life`, size `T·clamp(age/life·0.75, 0, 1)`; block light 15; **Tr**; drawn
  as two planes tilted at 60° (X rotation of −60°, then its mirror image), not facing the camera.
- `trial_spawner_detection(_ominous)`: `smoke` template (direction `(0, 0.9, 0)`, G −0.1, F 0.96) with
  scale 1.5, life `⌊8/rand(0.5..1)·1.5⌋`; block light 15; O; faces the camera around the
  vertical axis only; grows.
- `trial_omen`, `raid_omen`: "spell" template without colour (white), **Tr**.
- `ominous_spawning`: like `enchant` without the fall (`pos = start + p·(1 − age/life)`), T `0.1·(0.2..0.7)·(3..5)`,
  life `25 + rn(5)`, colour from `0x45aefe` to white, block light 15; O.
- `vault_connection`: like `enchant`, T × 1.5, bright, α from 0 to 0.6 between 25 % and 100 % of the life; **Tr**.
- Dripstone drops: `dripping_dripstone_water/lava` (hanging, 40 ticks, blue or cooling),
  `falling_dripstone_water/lava` (life `⌊64/(0.8r + 0.2)⌋`); on impact: `splash` or `landing_lava`, sound
  `block.pointed_dripstone.drip_water/lava` (volume 0.3 to 1, pitch 1).
- Event 1504: root = first block above the column (11 steps); fluid above (mud: water outside the
  Nether), otherwise the dimension's default drop (lava in the Nether); position `(x + 0.5 + δx, y + 0.25, z + 0.5 + δz)`,
  δ ≤ 0.125 drawn from the position seed. Event 3006: per face (six or mask), `rn(n + 1)` charges at
  `0.65` (top, bottom) or `0.57` (sides) from the centre, `0.35` for the mask; roll π on the bottom face (full) or
  top face (mask); without charge: 40 (20 if not full) `sculk_charge_pop` at ±0.45 (0.25). Event 3007: ten `shriek`
  with delay `5i`. 3013/3019: `30 + 5·min(players, 10)` detections; 3020: 30 ominous detections + 20 × (`trial_omen`,
  soul fire flame).

#### firework (spark), flash, trigger
- Spark: "simple animated" template, G 0.1, `v = p`, T × 0.75, life `48 + rn(12)`, alpha 0.99
  (hence Tr); sprites `spark_7 → spark_0`; colour (RGB integer) and fade set by the
  trigger. Twinkle: not drawn if `age ≥ life/3` and `⌊(age + life)/3⌋` is odd. Trail:
  in the first half of the life, one tick in two (`(age + life)` even), a motionless spark with the same colour/fade/twinkle
  is added with `age = life/2`.
- `flash`: life 4, Tr, colour from the option; at display `α = 0.6 − (age + t − 1)·0.125`,
  T `7.1·sin((age + t − 1)·π/4)`; sprite `flash`.
- Trigger (entity event 17): no rendering; life `2·explosion_count − 1` (+15 if an
  explosion twinkles); tick 0: explosion sound (large if ≥ 3 explosions or a large ball;
  distant variant if camera distance² ≥ 256). On even ticks `2i`: explosion `i` (empty colours
  → firework black), then a `flash` of the first colour at the centre. Shapes
  (each spark: colour drawn from the list, fade drawn if the list is non-empty):
  - small ball (velocity 0.25, step 2) / large ball (0.5, 4): for `(y, x, z)` in
    `[−step, step]³` (shell only: inside in y and x, z jumps by `2·step − 1`),
    direction `(x, y, z) + (r − r)·0.5`, `L = |dir|/velocity + 0.05g`, velocity `dir/L`;
  - star / creeper: outline of 2D points (not copied here, 6 and 12 points), 3 rotations
    around Y of angle `θ0 + k·π·m` (`θ0 = π·r`, `m` = 0.34 star, 0.034 creeper), 4 sub-steps
    per segment, ± symmetry, velocity 0.5;
  - burst: 70 sparks, `v = (0.5·vfx + 0.15g + ox, 0.5·vfy + 0.5r, 0.5·vfz + 0.15g + oz)`,
    `ox, oz = 0.05g` shared.
  End: twinkle sound if needed.

## 4. Ambient particles (blocks and fluids)

Each client tick, around the player's position `(xt, yt, zt)`: **667** iterations of two
draws, radius 16 then radius 32; position `xt + rn(R) − rn(R)` per axis (triangular
distribution). For each position: the block's animation, then the fluid's; if the fluid has a
drop (`dripping_water` / `dripping_lava`) and 1 chance in 10: drop under the block according to the shape
below; in creative with a barrier or a light block in hand: `block_marker`
at the centre of the blocks of this type; if the block is not a full collision cube: each
ambient particle of the biome with its probability, at `block + (r, r, r)`, velocity 0.

Two generators: the world's (positions, fluid drop, biome ambience, and the blocks
that read it themselves: lightning rod, redstone ore, beehive) and the animation's (the
rest). Implementation: `rc-app/src/block_anim.rs` (called after the world tick, before that
of the particles, world not frozen).

Fluid drop under a block (fluid at `p`, block below `b`): nothing if `b` contains a fluid;
if the top of `b`'s collision shape is < 1: drop flush with the top (`b.y + 0.95`, whole
square) only if the bottom face of the fluid block is sturdy; otherwise, if `b` is not
"impermeable" (glass, barrier): under `b` (bottom of its shape − 0.05) if its shape does not touch
the ground, otherwise at `b.y − 0.05` when the block under `b` has a top < 1 and no fluid; position
drawn within the shape's x/z footprint.

Fluids: source or falling water → 1/10 `underwater` in the block; flowing flat water → 1/64
sound `block.water.ambient` (volume 0.75–1, pitch 0.5–1.5); lava with air above →
1/100 `lava` flush (+ sound `block.lava.pop`), and 1/200 sound `block.lava.ambient`.

Biome ambience (environment attribute; values of the base biomes): soul sand valley
`ash` 0.00625, basalt deltas `white_ash` 0.118, crimson forest `crimson_spore` 0.025, warped
forest `warped_spore` 0.01428; draw `f ≤ p` at each non-full position.

Blocks (rn = animation generator, unless noted):

| block(s) | effect |
|---|---|
| torch (fire, soul, copper) | `smoke` + flame of the type at `(0.5, 0.7, 0.5)`, on each pass; wall: offset by 0.27 towards the wall and +0.22 in y |
| lit redstone torch (wall: same offset) | red `dust` at `(0.5, 0.7, 0.5) ± 0.1` |
| powered redstone wire | per horizontal side (north, east, south, west): connected up → vertical segment then the side's segment; side → segment of 0.5; not connected → 0.3; chance `0.2 × length`, colour according to the power |
| powered repeater | `dust` at 0.4 height, ½ on the delay torch, otherwise at −5/16 |
| lit redstone ore | `dust` on each non-opaque face (world generator) |
| powered lever | ¼: `dust` of scale 0.5 near the handle |
| fire, soul fire | 1/24 sound; placed on nothing flammable or sturdy: 2 `large_smoke` per flammable side (soul: all), otherwise 3 above |
| lit campfire | 1/10 sound; 1/5 `lava` (normal campfire only); smoke from the block entity: 11 % per tick, 2–3 columns (signal on hay bale), 4 smokes above each food item (20 %) |
| furnace / blast furnace | 10 % sound; `smoke` (+ `flame` for the furnace) in front of the face, height ≤ 6/16 or 9/16 |
| smoker | 10 % sound; `smoke` above (y + 1.1) |
| brewing stand | `smoke` above the centre |
| lit candles, candle cake | per wick: 30 % `smoke` (17 % sound), then `small_flame` |
| enchanting table | for each of the 32 bookshelf positions: 1/16 and bookshelf with a clear path → `enchant` towards the table |
| Nether portal | 1/100 sound; 4 × `portal` coming out of the face |
| ender chest | 3 × `portal` at the corners |
| End gateway | one `portal` per visible face |
| End portal | `smoke` at 0.8 |
| End rod | 1/5 `end_rod` along its axis |
| falling blocks (sand, gravel, powders, anvils, egg), brushable block | 1/16 `falling_dust` under the block if the space below is free; sand: 1/2100 desert sound (sand columns 8 blocks away in the 4 directions) |
| leaves | rain above: 1/15 `dripping_water` below; poplars: 1/300 sound if neighbouring trunk and 3 identical leaves; falling leaves: `f < chance` (oak, birch, jungle, acacia, dark oak, mangrove 1 % foliage-tinted; cherry 10 %; pale oak 2 %; poplars and azaleas 1 %) if the top face of the block below is not full |
| wet sponge, crying obsidian (1/5) | drop on a drawn face (not the top) that the neighbour does not block |
| full beehive (level ≥ 5) | 70 %: `dripping_honey` under the shape |
| charged respawn anchor | 1/100 sound; `reverse_portal` above |
| mycelium | 1/10 `mycelium` |
| spore blossom | `falling_spore_blossom`; 14 attempts of `spore_blossom_air` (±10 in x/z, 0 to −9 in y) in non-full blocks (≤ 1000 alive) |
| bubble column | upward: 2 `bubble_column_up`; downward: `current_down`; 1/200 sound; "always visible" |
| wither rose | 3 × ½: `smoke` around the flower's offset centre |
| lightning rod | thunderstorm, `rn₂₀₀ ≤ time % 200` (world generator), highest block of the column: 1–2 `electric_spark` along the axis |
| firefly bush | 1/30 sound at night under open sky; light ≤ 13 and 70 %: `firefly` within ±5 |
| pointed dripstone (free tip) | `f ≤ 0.12`: fluid above the root (column ≤ 10); drop if water/lava or `f < 0.02` |
| active sculk sensor | `dust_color_transition` sculk → red on a side face |
| dried ghast | 1/40 sound (dry: on sand/soul soil); 1/6 `white_smoke` (dry) or `happy_villager` (submerged) |
| dead bush, dry grass | 1/130 and 1/200 desert sounds (two "desert" blocks below; grass: player-linked sound) |
| open eyeblossom, hanging moss, creaking heart | sounds (1/700 on pale moss; 1/500 under pale oak; 1/16 at night surrounded by pale oak logs) |
| active sulfur under source water | 2 `sulfur_bubbles` always visible, 1/10 sound |
| barrier / light block (creative, in hand) | `block_marker` at the centre |

## 5. Entity trails (client tick)

Implementation: `projectile_sim.rs`, `entity_motion.rs`, `entity_fx.rs`.

- Primed TNT: a `smoke` at y + 0.5 per tick while the fuse burns.
- Arrows: in water, 4 `bubble` a quarter of the movement behind; critical: 4 `crit` along the
  movement (opposite velocity, +0.2 in y); tipped arrow: 2 `entity_effect` per tick in flight, 1 every
  5 ticks when stuck; spectral arrow: a white `effect` per tick in flight.
- Fireballs, skull: `smoke` at y + 0.5 after the movement; dragon fireball: `dragon_breath`;
  wind charges: nothing; in water: 4 `bubble` before the inertia.
- Shulker bullet: `end_rod` one movement behind, +0.15; Eye of Ender: `portal` (±0.3, −0.5)
  or 4 `bubble` in water; rocket: `firework` spark (velocity `(g·0.05, −vy/2, g·0.05)`) on each tick.
- Powered furnace minecart: ¼ `large_smoke` at y + 0.8; bee loaded with nectar: 5 %, 1–2
  `falling_nectar` at mid-height.
- Entering water (any entity, except on the first tick; box shrunk by 0.001 touching a
  water fluid): `⌈1 + 20·width⌉` `bubble` then as many `splash` above the feet block;
  local player: sound `entity.player.splash` (fast beyond 0.25).

## Uncertain points

- Biome ambient particles: the attribute is not extracted; the four base biomes are
  coded by name (a data pack that changes them is not tracked).
- Not done (incomplete network codec, `rc-proto-v777`): `vibration`, `trail`, arrival colour and
  scale of `dust_color_transition` received, `geyser*`, `noxious_gas_cloud`; elder guardian (model
  rendering); TNT minecart smoke (fuse given by entity event 10).
- Particles not detailed: `composter`, vault emission by its block (connection, waiting
  particles), `nautilus`, item pickup (groups with dedicated rendering).
- Collision: the entity collision algorithm may include the world border; not
  verified for particles (no entity passed).
- Depth state and back-face culling of the particle layers: read as
  "default"; depth write of the translucent ones to be confirmed on screen.
- "Other" entity events (§ 1.5) not detailed.
- Firework star and creeper outlines: noted (6 and 12 points) in `particles.rs`.
- Weather radius: default value of the setting not noted here (see `environment.md`).
