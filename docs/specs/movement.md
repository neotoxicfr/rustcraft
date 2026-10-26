# Local player movement — behavior specification

Target version: Java Edition 26.3 (protocol 777). Specification written in a clean room:
it describes the behavior observed in the official client, in our own words, without reusing
its internal names. One tick = 1/20 s.

Conventions:
- `f32` / `f64` indicate the computation type when it changes the result. A constant written
  `0.98F` is a 32-bit float (to be converted to `f64` only at the moment it multiplies
  an `f64`); a constant written `0.98` is an `f64`.
- "Velocity" = the entity's velocity vector (`f64` ×3, in blocks/tick), kept from one tick
  to the next.
- Yaw in degrees (0 = +Z, 90 = −X), pitch in degrees (positive = looking down).
- **Tabulated sine/cosine**: all the physics uses a table of 65,536 sines.
  `sin_t(a) = TABLE[(i64)(a · 10430.378350470453) & 65535]`,
  `cos_t(a) = TABLE[(i64)(a · 10430.378350470453 + 16384.0) & 65535]`,
  with `TABLE[i] = (f32) sin(i / 10430.378350470453)`. The angle in radians passed as argument
  is computed in `f32`: `yaw_f32 · (f32)(π/180)`, then widened to `f64`. Using exact `sin`
  instead of the table shifts the position by a few 1e-5 and ends up causing
  server corrections.

Origin of the data:
- **Received from the server**: attributes of the player entity (base values and modifiers),
  abilities (`may fly`, `flying`, flying speed), potion effects, game mode,
  food level, item being used (and its use-effects component).
- **Game data** (already extracted): collision shape of each block state, friction,
  speed factor, jump factor, bounce restitution, block tags
  (`minecraft:climbable`, `minecraft:fences`, `minecraft:walls`, `minecraft:suppresses_bounce`…),
  fluid states and their height.
- **Hard-coded**: all the other constants in this document.

---

## 1. Order of a client tick

For each client tick, without pause:

1. Game mode tick, aiming, interface, then handling of keyboard shortcuts (interface
   closed only).
2. Tick of all the world's entities that are not passengers. For each one, in order:
   a. **Common tick**: remembers the old position and old rotation, increments the
      tick counter.
   b. **Entity tick**. For the local player, only once the client has signaled
      "player loaded"; otherwise nothing moves.
3. Tick of the block entities.
4. **Player synchronization**: sending of the input packet (`minecraft:player_input`) if
   the state of the 7 keys changed since the last send, then sending of the position (see the
   spec already written).

Detail of the local player's tick (step 2b), from the outside inward:

1. Spectator mode → the player passes through everything (no collision, see §6). In spectator or
   as a passenger, the "on ground" flag becomes false.
2. Update of "underwater": true if the eyes are in water. The fluid state
   **computed at the previous tick** is read.
3. **Base entity tick**:
   - fluid update (§8): water or lava height, eyes in the fluid, current
     push;
   - update of the swimming state (§8);
   - in lava: fall distance ×0.5.
4. Update of the item being used.
5. **Local player AI method** (§2), which calls the generic AI (§3–§8) at the end. The physical
   movement takes place inside it.
6. The X and Z coordinates are clamped to ±2.9999999E7.
7. **Pose update** (§6.1).

---

## 2. Inputs (local player AI method)

### 2.1 Reading the keys

Seven booleans: forward, back, left, right, jump, sneak (shift), sprint.
Forward impulse = +1 if forward alone, −1 if back alone, 0 otherwise. Lateral impulse = +1 if
left alone, −1 if right alone, 0 otherwise. The 2D vector (lateral, forward) is **normalized**
(a zero vector stays zero): a diagonal is (±0.7071, ±0.7071).

"Forward impulse present" = forward component of the normalized vector > 1.0E-5F.

### 2.2 Sequence of the local player AI, in order

1. If the forward double-tap delay (sprint) is > 0, decrement it.
2. Portal and nausea effects (visual only).
3. Remember the state **of the previous tick**: jump held, shift held, forward impulse present.
4. **Sneaking**: the player is sneaking if all these conditions are true:
   they are not flying, not swimming, not a passenger, the sneaking box fits without
   collision, and either shift is held (state of the previous tick), or they are not sleeping and the
   standing box does not fit.
5. Reading of the keys (§2.1).
6. Auto-jump (option): if in progress, decrement its counter and force
   "jump held" for this tick. *To complete if the option is implemented (disabled by default
   on the vanilla server side; not needed for conformance).*
7. **Exit from a full block** (except spectator and passenger): 4 points at the corners are tested,
   (x ± 0.35·width, z ± 0.35·width) at foot height, in the order
   (−,+), (−,−), (+,−), (+,+). For each point whose block **suffocates** the entity: among
   the 4 horizontal neighbors (order west, east, north, south) that do not suffocate, take the one
   whose face is closest to the point (distance = fractional part of the point toward the
   face, 1 − frac on the positive side). The velocity on this axis is then replaced by 0.1 × direction
   (the other horizontal axis and Y are kept).
8. The forward double-tap delay is reset to 0 if, at the previous tick, shift was held, or
   if a slowing item is being used (outside passenger), or if the back key
   is held now.
9. **Sprint start** (§2.3).
10. **Sprint stop** (§2.3).
11. **Double jump for flight** (§5.1).
12. Glider start: if jump was just pressed (not held at the previous tick), without a
    flight toggle at this tick, without being on a climbable block, and if the glider can start:
    player command "start gliding". *Glider physics: out of scope, to
    complete.*
13. If the player is in water, shift is held and fluids affect them (they are not
    flying): velocity Y −= 0.04F.
14. Underwater vision counter (visual only).
15. Flight: ascent/descent (§5.2).
16. Mount jump (out of scope).
17. **Generic AI** (§3).
18. If the player is on the ground, flying and not in spectator: flight stops and the client
    sends the abilities packet (`minecraft:player_abilities`).

### 2.3 Sprint

"Sprint possible" (parameter: shallow water tolerated or not) = all these conditions:
- the player is not blinded (effect `minecraft:blindness`);
- outside a mount: food > 6 **or** ability "may fly";
- shallow water tolerated, or the player is not in shallow water (in water but
  eyes out of water).

**Can start** if all these conditions are true:
- they are not already sprinting;
- forward impulse present (this tick);
- sprint possible (shallow water tolerated only if flying);
- no slowing item being used (item in use whose use effects
  forbid sprinting);
- if gliding, they must be underwater;
- if sneaking or crawling (swim pose out of water), they must be underwater.

If so:
- if the forward impulse **was not** present at the previous tick (rising edge of
  "forward"): if the double-tap delay is > 0 → sprint; otherwise the delay takes the value
  of the "sprint window" option (integer 0–10, 7 by default; 0 disables);
- if the sprint key is held → sprint.

Effective window: with the value 7, the second press must come at most 6 ticks after the
first, because the delay is decremented at the start of the tick.

**Stop** (tested if the player is sprinting):
- if swimming: stop if sprint is not possible (shallow water tolerated), or if they are no
  longer in water, or if there is no forward impulse **and** they are not on the ground **and**
  shift is not held;
- otherwise: stop if sprint is not possible (shallow water tolerated only if flying),
  or if there is no forward impulse, or if there was a **non-minor** horizontal collision
  in the previous movement (§6.6).

**Sprint effect**: starting to sprint adds to the attribute `minecraft:movement_speed` a
transient "multiplied total" modifier of +0.3F (0.30000001192092896 in `f64`); stopping
removes it. The client does this itself as soon as it changes state (it does not wait for the server).
The change is signaled to the server by player command (already specified).

### 2.4 Conversion of keys into movement input

Done by the generic AI (§3, step 6) when the local player is the camera:

1. Zero vector → zero input.
2. Otherwise ×0.98F.
3. Item being used (outside passenger): × the speed multiplier of its use
   effects (item data; 0.2 for most consumable items, to check in the
   component data).
4. Sneaking or crawling: × the value of the attribute `minecraft:sneaking_speed` (base 0.3, received
   from the server, in `f32`).
5. **"Square" correction**: let L be the length of the vector and d its unit direction. With
   t = min(|dx|,|dy|) / max(|dx|,|dy|), the length is replaced by
   `min(L · sqrt(1 + t²), 1.0)`, direction unchanged. Result: a diagonal goes as fast
   as a straight line, even when slowed. All in `f32`.
   Examples: straight ahead 0.98 → 0.98; diagonal 0.98 → 1.0; sneaking straight ahead 0.294,
   sneaking diagonally 0.294·√2 ≈ 0.41578.
6. The two components give the lateral input (`strafe`) and the forward input (`forward`).
   Vertical input = 0. "Jump held" = jump key.

Otherwise (the player is not the camera): inputs ×0.98F only.

### 2.5 Double-tap window

The duration armed by the first forward press (§ 2.3) is the setting `sprintWindow` (0 to 10 ticks,
7 by default); at 0, the double tap never triggers sprint.

### 2.6 Auto-jump (setting `autoJump`)

Auto-jump counter: if it is greater than 0 right after reading the keys, it is
decremented and the jump key is forced for this tick (it also goes out in the input
packet); a jump forced this way does not arm the flight double-tap.

After each movement with collisions, if the setting is active, the counter is 0, the
player on the ground, without the sneak key, with a non-zero key vector and a block jump
factor ≥ 1:

1. Direction = actual horizontal movement (f32); if it is almost zero (square ≤ 0.001), the
   key vector × walking speed (attribute) is taken, rotated by the yaw; if it is still
   almost zero, nothing.
2. If the direction opposes the look (horizontal dot product with the view vector, pitch
   included, < −0.15), nothing.
3. The two blocks above the head (column of the feet, from the top of the box) must be
   without collision.
4. Reach = max(speed × 7, movement length). Two parallel segments, at 0.51 above
   the feet, offset by half a width to the left and right, from the position to
   position + movement + direction × reach. The collision boxes of the zone covering these
   segments (inflated by the width, player height), walked x first then y then z: the
   first one that touches a segment gives the obstacle height (its top); the block above
   its center, if it has a collision, replaces this height (top of its shape), and if it
   exceeds the feet by more than 1.2, nothing.
5. If the obstacle exceeds the feet by more than 0.5 and at most 1.2: counter = 1 (jump on the next
   tick). Under the "Jump Boost" effect, the 1.2 bound (here and in step 4) gains
   0.75 × (amplifier + 1).

---

## 3. Generic AI (in order)

1. Double-tap delay for flight: decremented if it is > 0 (§5.1).
2. If the player is flying and not a passenger: fall distance reset to 0.
3. Delay between jumps: decremented if it is > 0.
4. **Minimum velocity threshold**: if vx² + vz² < 9.0E-6, then vx = vz = 0 (test on the
   horizontal norm, specific to the player). If |vy| < 0.003, then vy = 0.
5. Equipment (no effect on movement).
6. Inputs: §2.4. If the player is immobilized (dead, sleeping…): jump false and
   inputs zero.
7. **Jump** (§4.5).
8. Glider: wear (server side), out of scope.
9. Effects `minecraft:slow_falling` or `minecraft:levitation` active → fall distance to 0.
10. **Movement** (§4, §5, §8), with the input (lateral, 0, forward).
11. **Effects of the blocks traversed** (§7): walking on the block, "inside" blocks, cobwebs…
12. Walk animation (visual).
13. Push by neighboring entities (*to complete: soft collisions between entities*).
14. Back in the player AI: the remembered base speed takes the value of the attribute
    `movement_speed` (in `f32`), then item pickup.

---

## 4. Ground and air movement (general case)

### 4.1 Mode choice

- The player is in water or lava **and** fluids affect them (they are not flying) → fluid
  mode (§8).
- Otherwise, if gliding → glider mode (out of scope).
- Otherwise → air/ground mode, described below. Creative flight also goes through this mode (§5).

### 4.2 Block that gives the friction

"Position under the feet with offset h":
- if there is a **main supporting block** (§6.5):
  - if h ≤ 1.0E-5F, it is that block;
  - otherwise its X and Z are kept and Y = floor(y_feet − h) is taken. Exception: the supporting block is kept
    as is if it is a fence (`minecraft:fences`, only when h ≤ 0.5), a wall
    (`minecraft:walls`) or a fence gate (whatever h);
- otherwise: (floor(x), floor(y_feet − h), floor(z)).

Offsets used:
- **0.500001F**: block that gives the friction, the speed factor and the jump factor.
- **0.2F**: block "walked on" (sounds, fall damage, bounce, walking effects).
- 1.0E-5F: raw supporting block.

### 4.3 Formula for one tick in air/ground mode

Inputs: the input (lateral s, forward f), the velocity v.

1. **Block friction** `fb` (`f32`): if the player is on the ground, friction of the block at the position
   under the feet with offset 0.500001F, corrected by the attribute `minecraft:friction_modifier`
   (base 1.0):
   `corrected(x, m) = clamp(1 − (1 − x)·m, 0, 1)` in `f32`. In the air: fb = 1.0.
   (Block frictions, to confirm in the data: 0.6 by default, 0.98 for ice, 0.989 for blue ice, 0.8 for
   the slime block… data already extracted.)
2. **Acceleration speed** `a` (`f32`):
   - on the ground: if fb > 0.6 (comparison in `f64`, **beware**: 0.6F is 0.6000000238 and
     therefore passes the test), `a = speed · (0.21600002F / (fb·fb·fb))`; otherwise `a = speed`.
     Here `speed` = current value of the attribute `movement_speed` in `f32` (base 0.1F, i.e.
     ≈ 0.13 when sprinting), effects and modifiers received from the server included;
   - in the air: "air" speed = 0.02F, or 0.025999999F when sprinting. In creative flight, see §5.
3. **Acceleration according to the yaw**: the following vector is added to v
   - E = (s, 0, f); if |E|² < 1.0E-7, nothing;
   - if |E|² > 1, E is normalized; then E ×= a;
   - with `S = sin_t(yaw·π/180)` and `C = cos_t(yaw·π/180)` (in `f32`):
     Δx = E.x·C − E.z·S, Δy = E.y, Δz = E.z·C + E.x·S.
4. **Ladder** (§7.1): velocity clamping if the player is on a climbable block.
5. **Movement with collisions** (§6) of the velocity v.
6. Re-reading of v (the movement modified it: blocked axes, block speed factor).
   If (horizontal collision **or** jump held) **and** (on a climbable block, or in powder
   snow at the previous tick with boots that allow walking on it): vy = 0.2.
7. **Gravity** on vy:
   - levitation: vy += (0.05·(amplifier+1) − vy)·0.2;
   - otherwise, if the chunk containing the friction block is loaded: vy −= g, with g =
     effective gravity = attribute `minecraft:gravity` (base 0.08; 0 if the entity has no
     gravity). With `minecraft:slow_falling` and vy ≤ 0 (read **before** the tick's
     acceleration), g = min(g, 0.01);
   - chunk not loaded: vy = −0.1 if y > the world's minimum Y, otherwise 0.
8. **Frictions**:
   - air = corrected(0.91F, attribute `minecraft:air_drag_modifier`, base 1.0);
   - horizontal: vx, vz ×= (fb · air) in `f32` (0.546 on a normal block, 0.91 in the air);
   - vertical: vy ×= corrected(0.98F, same attribute).
   (An entity may ignore frictions; this is never the case for the player.)

### 4.4 Block speed factor

Applied **at the end of each movement with collisions** (§6.7), on vx and vz only:
- block at the player's position (floor of the feet coordinates): if it is water or a
  bubble column, its factor is taken as is; otherwise, if its factor is 1.0, that of
  the block under the feet with offset 0.500001F is taken;
- final factor = lerp(attribute `minecraft:movement_efficiency` (base 0), factor, 1.0)
  = factor + (1 − factor)·efficiency.
- in creative flight (or gliding), the factor is always 1.0: no slowdown.
(Examples to confirm in the extracted data: soul sand and honey block 0.4.)

### 4.5 Jump (step 7 of the generic AI)

If jump held and fluids affect the player (they are not flying):
- h = lava height if the player is in lava, otherwise water height (§8.1); threshold
  = 0.4 (0.0 if the eye height is < 0.4);
- "in water with height" = in water and h > 0;
- if in water with height and (not on the ground or h > threshold) → **swim upward**: vy += 0.04F;
- otherwise, if the player is in lava and not (on the ground and lava ≤ threshold) → vy += 0.04F;
- otherwise, if (on the ground, or in water with h ≤ threshold) and the delay between jumps is 0 →
  **jump from the ground**, then delay between jumps = 10.

Jump not held (or player flying) → delay between jumps = 0. Holding the key therefore gives one jump
every 10 ticks at most; releasing then pressing again allows jumping again as soon as the
ground is touched.

**Jump from the ground**:
1. Power P (`f32`) = attribute `minecraft:jump_strength` (base 0.42F) × block jump factor
   + bonus of the effect `minecraft:jump_boost` (0.1F × (amplifier + 1), 0 without the effect).
   Block jump factor: that of the block at the player's position if it differs from 1.0,
   otherwise that of the block under the feet with offset 0.500001F (honey block: 0.5, to confirm in the data).
2. If P ≤ 1.0E-5F: nothing.
3. vy = max(P, vy).
4. When sprinting: v += (−sin_t(r)·0.2, 0, cos_t(r)·0.2), with r = yaw·(f32)(π/180) in `f32`.

The jump applies **before** the acceleration and gravity of the same tick: on the first tick,
vy = (0.42 − 0.08)·0.98 = 0.3332 (without collision).

### 4.6 Falling

The fall distance (`f64`) increases by −dy at each downward movement out of water.
It is reset to 0 on touching the ground (after the "fall on the block" call of the block at
offset 0.2F), in water, on a ladder, in a cobweb, in flight, and in a movement
≥ 1 block that crosses a "fall-cancelling" block. Damage is computed by the server;
on the client side, this distance serves edge caution (§6.4) and bounces.

---

## 5. Creative flight

### 5.1 Activation (double tap on jump)

If the player has the "may fly" ability:
- in spectator: if they are not flying, they fly at once (toggle noted for this tick) and the client
  sends `minecraft:player_abilities`;
- otherwise, on a rising edge of jump (not held at the previous tick, held now, outside auto
  jump):
  - if the double-tap delay is 0: it becomes **7**;
  - otherwise, if the player is not swimming (and is not on a mount, except a jumping mount): flight
    is toggled. If it has just been activated and the player is on the ground → **jump from the
    ground** (§4.5). Toggle noted, sending of `minecraft:player_abilities`, delay reset to 0.

The delay is decremented later in the same tick (§3 step 1). The second press must therefore
come at most 6 ticks after the first.

### 5.2 Ascent and descent

Right before the generic AI, if the player is flying and is the camera:
k = (+1 if jump held) + (−1 if shift held). If k ≠ 0: vy += k × flying speed × 3.0F.
Flying speed: ability received from the server, 0.05F by default.

### 5.3 Horizontal and vertical physics

The air/ground mode (§4.3) applies, with these differences:
- in flight, fluids do not affect the player: no fluid mode, no current, no
  swimming;
- acceleration speed = flying speed (×2.0F when sprinting) when the player is not on the ground;
- vy is remembered **before** the air/ground mode; after it (acceleration, collisions, gravity,
  frictions), vy is **replaced** by `remembered_vy × 0.6`. Gravity and vertical friction
  therefore have no effect in flight; the horizontal keeps ×0.91 (fb = 1 in the air).
- Horizontal limit speed ≈ a·0.91/(1−0.91): about 0.506 block/tick (10.1 m/s) in normal
  flight (≈ 0.495 with the input 0.98 straight ahead), double when sprinting.

### 5.4 Stopping

- On the ground (after the movement) and outside spectator: flight stops (§2.2 step 18).
- Double tap (§5.1).
- No edge caution (§6.4) in flight. The sneaking pose is not targeted in flight, but
  shift is used to descend.

---

## 6. Collisions

### 6.1 Player box and poses

Axis-aligned box, centered in X/Z on the position, base at the feet (y).

| Pose | Width | Height | Eyes |
|---|---|---|---|
| standing | 0.6 | 1.8 | 1.62 |
| sneaking | 0.6 | 1.5 | 1.27 |
| swimming, glider, spin attack | 0.6 | 0.6 | 0.4 |
| sleeping (fixed size, unscaled) | 0.2 | 0.2 | 0.2 |

The dimensions are multiplied by the attribute `minecraft:scale` (*to check: exact application
of the scale*).

Targeted pose (end of tick, §1 step 7): sleeping → sleeping; swimming →
swimming; gliding → glider; spin attack → it; shift held and not flying →
sneaking; otherwise standing.

Final choice, only if the swim box fits:
- spectator, passenger, or the targeted pose fits → targeted pose;
- otherwise, the sneaking box fits → sneaking;
- otherwise → swimming (crawling under a block).

"Fits" = no collision (blocks, solid entities, world border) with this pose's box,
shrunk by 1.0E-7 on each face.

### 6.2 Movement with collisions: sequence

Input: the desired movement D (in normal mode, this is the velocity v).

0. Spectator (passes through everything): position += D; the horizontal, vertical and minor
   collision flags become false; end.
1. **Slowdown by a block** (§7.2): if a blocking multiplier is pending
   (norm² > 1.0E-7), D is multiplied component by component by this multiplier, the
   multiplier is cleared and **the velocity v is set to zero**.
2. **Edge caution** (§6.4): D may be reduced in X/Z.
3. **Resolution** (§6.3): the actual movement M is obtained.
4. If |M|² > 1.0E-7 or |D|² − |M|² < 1.0E-7: position += M (otherwise, the position does not move).
   If the fall distance is non-zero and |M|² ≥ 1: cast a ray over min(|M|, 8) blocks;
   if it hits a fall-cancelling block, the fall distance is reset to 0.
5. Flags:
   - collision X = |D.x − M.x| ≥ 1.0E-5 (same for Z); **horizontal collision** = X or Z;
   - **vertical collision** = D.y ≠ M.y (exact equality); "collision below" =
     vertical collision and D.y < 0;
   - **on ground** = collision below; recomputation of the supporting block (§6.5);
   - **minor collision** (§6.6).
6. Fall distance and "fall on" the block at offset 0.2F (§4.6).
7. **Bounce / stop** (§6.7) if there was a horizontal collision, or a non-zero
   vertical movement that hit something.
8. Block speed factor (§4.4) on vx, vz.

The movement M is also remembered, for the walk over traversed blocks (§7).

### 6.3 Per-axis resolution and step-up

**Collection of shapes** in a box B (the player's box extended by D):
- collision shapes of blocks: the integer positions are walked from
  floor(min − 1.0E-7) − 1 to floor(max + 1.0E-7) + 1 on each axis, with 3 rules:
  - cells located on 3 edges at once (grid corners) are ignored;
  - cells on exactly 2 edges only count for a moving piston;
  - cells on 1 edge only count if the block's shape overflows its cube [0,1]³
    (fences, walls…);
  - an unloaded chunk gives no shape;
  - a full cube counts if it intersects B; another shape counts if it has a non-zero
    common volume with B;
- shapes of nearby solid entities (boats, minecarts, shulkers…) in the box extended by
  D then by the step height upward. *To complete: list of solid entities.*
- world border if the player is inside and near it.
A block's collision shape may depend on the player (scaffolding: it is passed through when
descending with shift; powder snow: it is walked on with leather boots; etc.).
*To check: list of blocks whose shape depends on the player.*

**Per-axis sweep**: start from M = 0. Axis order: **Y first**, then X then Z if
|D.x| ≥ |D.z|, otherwise Z then X. For each axis whose component is non-zero: the permitted
distance along this axis is computed for the box **already moved by M**, against all
the shapes, and written into M.
Permitted distance on an axis, for a desired distance d:
- if |d| < 1.0E-7 → 0 (tested before each shape);
- for each elementary box of the shapes that strictly overlaps the moving box on the
  two other axes (with a margin of 1.0E-7):
  - d > 0 and obstacle's low face ≥ box's high face − 1.0E-7:
    d = min(d, obstacle's low face − box's high face);
  - d < 0 symmetrically, with max.
  An already interpenetrated obstacle does not block.

**Step-up** (height h_m = attribute `minecraft:step_height`, base 0.6; at least 1.0
for a mount driven by a player). After the sweep, if h_m > 0, if (Y was blocked
while descending, or the player was already on the ground before the movement) and if there is a collision
in X or Z (exact equality D ≠ M):
1. Starting box Bs = initial box lowered by M.y if Y was blocked while descending, otherwise
   the initial box.
2. Search zone = Bs extended by (D.x, h_m, D.z); if Y was not blocked while descending,
   it is also extended by −1.0E-5F downward. The shapes of this zone are collected.
3. Candidate heights: for each shape, its cut Y coordinates (edges of its boxes),
   taken in increasing order and relative to Bs.y_min (in `f32`). Those that are ≥ 0
   and different from (f32)M.y are kept, and the shape is stopped as soon as a value exceeds h_m. The
   unique values are sorted in increasing order.
4. For each candidate height c, in order: per-axis sweep of (D.x, c, D.z) from
   Bs. If the horizontal movement obtained is **strictly longer** (horizontal norm²)
   than that of M, this result is returned, with Y corrected by −(initial box.y_min − Bs.y_min).
   End.
5. If no height fits: M stays as is.

Consequence: the player steps onto the lowest step that lets them advance further.
There is no second descent: we stay at the step's height.

### 6.4 Edge caution (sneaking)

Applied if all these conditions are true: the player is not flying, D.y ≤ 0, the
movement is voluntary (not a piston push), shift is held (**this tick**), and the
player is "above the ground": on the ground, or fall distance < h_m and no void of
(h_m − fall distance) under them.

"Can fall at least h by shifting by (dx, dz)" = the box
[x_min + 1.0E-7 + dx, y_min − h − 1.0E-7, z_min + 1.0E-7 + dz] → [x_max − 1.0E-7 + dx, y_min,
z_max − 1.0E-7 + dz] is free (no collision).

With a step of 0.05 (h = h_m):
1. While dx ≠ 0 and one can fall with (dx, 0): if |dx| ≤ 0.05, dx = 0 and we exit;
   otherwise dx −= sign(D.x)·0.05.
2. Same for dz with (0, dz).
3. While dx ≠ 0, dz ≠ 0 and one can fall with (dx, dz): both are reduced the same
   way (each set to 0 when it goes below 0.05).
4. D = (dx, D.y, dz).

### 6.5 Main supporting block

After each movement that updates "on ground":
- on the ground: test zone = slice [x_min, y_min − 1.0E-6, z_min] → [x_max, y_min, z_max]. Among the
  blocks whose collision shape touches this zone (same collection as in §6.3: margin of one block
  around the zone, edge cells kept only for a shape that overflows its cube;
  standing on a fence or a low wall, 1.5 high, the support is therefore the block located under the
  feet cell), the one whose center is closest to the player's position is kept
  (distance² to the block's center). On a tie, the greatest in the order (Y, then Z, then X) is taken. If
  nothing is found although there was a support before, the test is redone with the zone shifted by
  (−M.x, 0, −M.z);
- not on the ground: no support anymore.

### 6.6 Minor horizontal collision

Computed for the local player at each horizontal collision. The desired direction
(lateral input s, forward f) rotated by the yaw is taken: gx = s·C − f·S, gz = f·C + s·S.
If |g|² ≥ 1.0E-5F and |M_horiz|² ≥ 1.0E-5F: minor collision if the angle between g and M_horiz
is < 0.13962634F rad (8°). Otherwise: not minor. A minor collision (brushing a wall at
an angle) does not stop the sprint.

### 6.7 Stop on the blocked axis and bounces

After the movement (if horizontal collision, or vertical movement requested and vertical
collision):
- restitution r = 0 if shift is held, otherwise the attribute `minecraft:bounciness` (base 0);
- X axis blocked: vx = −vx·r. Same for Z. With r = 0, the component becomes 0;
- vertical collision:
  - if it is below: r = max(r, restitution of the block at offset 0.2F) when
    −vy > effective gravity, shift is not held and the block does not have the tag
    `minecraft:suppresses_bounce`; otherwise r = 0. Block restitution (data): slime 1.0,
    beds and shelf fungus 0.75, 0 elsewhere;
  - if r > 0: p = M.y / vy, vy = (p·g − vy)·lerp(p, 1, vertical air friction)·r,
    with g the effective gravity and a vertical friction of corrected(0.98F, air_drag);
  - otherwise vy = 0 (for example: ground or ceiling touched).
- The slime block **walked on** ("walk on" step, §7) also multiplies vx, vz
  by (0.4 + |vy|·0.2) when |vy| < 0.1 and shift is not held.

---

## 7. Climbable blocks and slowing blocks

### 7.1 Ladders and climbable blocks

"On a climbable block" (never in spectator nor creative flight), depending on the block state **at the
player's position** (floor of the feet position):
- when gliding, a block carrying the tag `minecraft:can_glide_through` does not count;
- a block of the tag `minecraft:climbable` (ladder, vines, scaffolding…) counts;
- an open trapdoor counts if the block just below is a ladder oriented in the same
  direction as the trapdoor.

Effects (air/ground mode, §4.3 steps 4 and 6):
- before the movement: fall distance to 0; vx and vz clamped to [−0.15F, 0.15F];
  vy ≥ −0.15F; if vy < 0, shift held, not flying and block different from a scaffolding → vy = 0
  (we stay hooked);
- after the movement: horizontal collision or jump held → vy = 0.2 (we go up).
- Gravity and friction (0.98) then apply: when climbing, final vy =
  (0.2 − 0.08)·0.98 = 0.1176 per tick.

In water mode (§8.2): horizontal collision on a climbable block → vy = 0.2 before
friction.

### 7.2 Cobwebs and slowing blocks (generic mechanism)

After the movement (§3 step 11):
1. If the player is on the ground: "walk on" action of the block at offset 0.2F (slime, magma…).
2. The blocks traversed during the tick's movements are walked, axis by axis in the order
   of §6.3, with a player box shrunk by 1.0E-5F (16 iterations at most per
   movement). For each block whose "inside" shape overlaps the path, its
   "entity inside" action is called.
3. Slowing blocks go through a **blocking multiplier** (vector), used at the **next**
   movement (§6.2 step 1): the movement is multiplied by this vector and the
   velocity is reset to zero. Fall distance reset to 0 too. Values noted:
   - cobweb: (0.25, 0.05F, 0.25), or (0.5, 0.25, 0.5) with the Weaving effect;
   - sweet berry bush: (0.8F, 0.75, 0.8F);
   - powder snow: (0.9F, 1.5, 0.9F), only if the block at the player's position is
     itself powder snow.
   None of this in (creative) flight.
4. Swept zone: for each movement of the tick, segment by segment along a single axis
   (Y, then X and Z, the smaller of the two last), the blocks touched by the
   envelope of the start box and the end box (shrunk by 1.0E-5F) are visited. Each
   block counts only once per tick. If there was no movement, the segment
   "position at the start of the tick → current position" is taken. A pass is "precise" if the segment
   exceeds 0.99999 block or if the end box overlaps the block's cube.
5. Honey block (sliding): off the ground, feet below y_block + 0.9375 − 1e-7, and falling:
   vy_before = vy / 0.98F + 0.08 < −0.08, and the player stuck to a face (|x_block + 0.5 − x| or
   |z_block + 0.5 − z|, plus 1e-7, exceeds 0.4375 + width/2). Then vy = (−0.05 − 0.08)·0.98F;
   if vy_before < −0.13, vx and vz are multiplied by −0.05 / vy_before. Fall distance to 0.
6. Bubble column, only on a precise pass and outside flight. "Free above": the block
   above has neither collision nor fluid.
   - upward: free above vy = min(1.8, vy + 0.1), otherwise vy = min(0.7, vy + 0.06);
   - downward: free above vy = max(−0.9, vy − 0.03), otherwise vy = max(−0.3, vy − 0.03).
7. Slime block "walked on" (on the ground, block at offset 0.2F): §6.7.

The data tables give, per block, the bounce restitution and the effect type
(cobweb, berries, powder snow, honey, bubble column, slime), read from the installed game.

---

## 8. Water and lava

### 8.1 Detection (base tick, each tick)

Detection box = player's box shrunk by 0.001 on each face.
- If no neighboring section contains fluid, or if a chunk of the zone enlarged by a
  block in X/Z is not loaded: the player is in no fluid.
- Otherwise, for each block between floor(min) and ceil(max) − 1 that contains a fluid:
  height of the fluid top = y_block + fluid height at this position. If it is
  ≥ y_min of the box:
  - submerged height of the fluid type = max(fluid top − y_min of the player's box);
  - eyes in the fluid if the column is the eyes' column (same integer X and Z), and the eyes
    are ≥ y_block and ≤ y_block + the fluid's "camera" height (*to check: difference from the
    normal height*);
  - if the player is pushed by fluids (they are not flying): the fluid's flow vector is
    accumulated, multiplied by the accumulated height if it is < 0.4.
- **In water** = water height > 0. **Underwater** = eyes in water at the previous tick
  **and** in water. **Shallow water** = in water but not underwater.
- In water: fall distance to 0.
- **Current** (if pushed by fluids): for the player, impulse = sum of the
  flows / number of flows (average, without normalization), × 0.014 for water,
  × 0.0023333333333333335 for lava (0.007 in a "fast lava" dimension). If
  |vx| < 0.003, |vz| < 0.003 and |impulse| < 0.0045, the impulse is brought to a length
  of 0.0045. It is added to v. Nothing if the sum has a norm² < 1.0E-5F.

### 8.2 Swimming (state)

- If the player is swimming: they continue as long as they sprint, are in water and are not a passenger.
- Otherwise: they start swimming if they sprint, are underwater, are not a passenger and the block
  at their position contains water (tag `minecraft:water`).
- In flight or in spectator, they never swim.

### 8.3 Swim adjustment before the movement

If the player is swimming (and is not a passenger): let ly = −sin_t(pitch·π/180) (vertical
component of the look). Coefficient k = 0.085 if ly < −0.2, otherwise 0.06. If ly ≤ 0, or jump held,
or there is a fluid at the block (x, y + 0.9, z): vy += (ly − vy)·k.

### 8.4 Movement in water

1. vy ≤ 0 at the start → "falling". Remember y0. g = effective gravity.
2. Horizontal friction `fh` = 0.9F when sprinting, otherwise 0.8F. Acceleration a = 0.02F.
3. Aquatic efficiency w = attribute `minecraft:water_movement_efficiency` (base 0), ×0.5F if
   not on the ground. If w > 0: fh += (0.54600006F − fh)·w; a += (base speed − a)·w.
4. Effect `minecraft:dolphins_grace`: fh = 0.96F.
5. Acceleration according to the yaw (§4.3 step 3) with a.
6. Movement with collisions (§6).
7. Horizontal collision and climbable block → vy = 0.2.
8. vx ×= fh, vy ×= 0.8F, vz ×= fh.
9. **Gravity in a fluid** (if g ≠ 0 and not sprinting): if falling and
   |vy − 0.005| ≥ 0.003 and |vy − g/16| < 0.003 → vy = −0.003; otherwise vy −= g/16
   (0.005 by default).
10. **Exit from the water**: if horizontal collision and the box shifted by
    (vx, vy + 0.6F − y + y0, vz) is free (no collision, no liquid) → vy = 0.3F.

Shift in water: −0.04F (§2.2 step 13). Jump in water: +0.04F (§4.5).

### 8.5 Movement in lava

1. Acceleration 0.02F according to the yaw, then movement with collisions.
2. If lava height ≤ threshold (0.4): vx, vz ×0.5, vy ×0.8F, then gravity in a fluid
   (§8.4 step 9). Otherwise: v ×0.5.
3. If g ≠ 0: vy −= g/4.
4. Exit from the lava, as for water (§8.4 step 10).

---

## 9. Cases specific to the local player (wave W4a, 18/10/2026)

### 9.1 Velocity imposed by the server (knockback)

The entity movement packet targeting the local player replaces their velocity as is
(hits, arrows, wind charges, Riptide as seen by the server). The next tick takes over.

### 9.2 Sprint and received attributes

The sprint modifier carries the id `minecraft:sprinting` (+0.3F, multiplicative on the
total). A received attribute replaces base and modifiers entirely: the one the server sends back
after `START_SPRINTING` takes the place of ours instead of adding to it (speed ×1.3, not ×1.69).

### 9.3 Box in `f32`

Width, height and eyes are `f32` multiplied by the scale in `f32`; the half-width is
`width / 2` in `f32` then widened to `f64` (0.30000001192… at scale 1). Against a wall, we
stop at 1 − 0.30000001192… from the block's edge.

### 9.4 Shapes that depend on the player

All the player's collision searches (movement, edge caution, supporting block,
poses, exit from water, auto-jump) see these blocks according to them; "above" means
feet > block base + shape top − 1e-5F; "descends" = Shift pressed.

- Scaffolding: platform (extracted shape) if above the whole block and not descending; otherwise,
  for a `bottom` scaffolding of non-zero distance, 2/16 slab at the base if the feet are
  above the block's base; otherwise nothing (it is entered from the side, descended while sneaking).
- Powder snow: fall > 2.5 → full shape 0.9F high; leather boots on the feet, above,
  not descending → full block; otherwise nothing. Being in the snow this tick (`in_powder_snow`) serves on the
  next tick: in leather boots, horizontal collision or jump → vy = 0.2 (climbing).

### 9.5 Ladders (complement to § 7.1)

Bounds ±0.15F (widened to `f64`). Shift holds the descent except in a scaffolding. The climb
test (vy = 0.2) redoes "climbable" at the position after the movement. An open trapdoor
placed on a ladder of the same orientation is climbable; when gliding, a block of the tag
`can_glide_through` is not.

### 9.6 Speed factor (complement to § 4.4)

Only the occupied block counts if it is the water block or a bubble column (a block
simply flooded looks at the block that brakes).

### 9.7 Unloaded chunk

Under the feet, chunk absent: vy = −0.1 if the feet are above the bottom of the dimension's world, 0 otherwise.

### 9.8 Push by neighbors

On the client side, only the local player is pushed. At each tick, each living entity that overlaps their box
(except bat, armor stand, parrot, vex, spectator player), and each boat whose
box inflated by 0.2F (−0.01F in height) touches them while their feet are not higher than
the bottom of the boat, pushes them away: difference (dx, dz) = neighbor's position − theirs; d = max(|dx|, |dz|);
if d ≥ 0.01F: d ← √d, dx, dz ÷ d, × min(1, 1/d), × 0.05F; velocity −= (dx, 0, dz). Nothing if they are
spectator or on a ladder. Team rules (`collisionRule`, the neighbor's and their own):
"never" on one side → nothing; "own team" on one side between teammates → nothing; otherwise
neither side must be "other teams", or they must be teammates.

### 9.9 Gliding (elytra)

- Start: new press on jump, outside a creative flight toggle this tick, outside a ladder, not already
  gliding, glider worn (component `glider` on an item worn in its slot, not about to
  break), off the ground, without levitation nor creative flight, out of water and lava → flag set, player
  command `START_FALL_FLYING` (before the input and position packets). Then the flag follows
  the entity data (index 0, bit 0x80) sent back by the server (end on landing).
- Before the movement: if vy > −0.5 and fall > 1, fall = 1.
- Movement (outside fluid): on a ladder, normal movement and end of gliding. Otherwise, with
  the look (tabulated sines), the pitch θ in radians `f32`, l = horizontal length of the look,
  h = horizontal speed: vy += g × (−1 + 0.75 cos²θ); if vy < 0 and l > 0: c = −0.1 vy cos²θ,
  v += (look.x c / l, c, look.z c / l); if θ < 0 and l > 0: c = h × (−sin θ) × 0.04,
  v += (−look.x c / l, 3.2 c, −look.z c / l); if l > 0: vx += (look.x / l × h − vx) × 0.1
  (same for z); then v × (0.99F, 0.98F, 0.99F) and movement with collisions.
- Pose 0.6 × 0.6 (eyes 0.4), priority after swimming.
- Rocket attached to the player (rocket's data 9): on its tick, while gliding,
  v += look × 0.1 + (look × 1.5 − v) × 0.5.

### 9.10 Riptide (trident)

On release: impulse of the Riptide strength along the look axis (`f32` components,
normalized), 20 ticks of spin attack (pose 0.6 × 0.6), and on the ground a movement of
1.1999999F upward. Each tick: countdown; an entity in the swept zone stops the attack
(living: v × −0.2), otherwise a horizontal collision.

---

## Uncertain points / to complete

- Auto-jump (client option): not detailed.
- Speed multiplier of items being used: comes from an item component;
  default value to be noted in the data.
- "Camera" height of a fluid for the eyes test.
- Push: minecarts not handled; width of the neighbors without their scale attribute.
- Riptide: the client-side trigger (enchantment strength, rain) is not wired up.
