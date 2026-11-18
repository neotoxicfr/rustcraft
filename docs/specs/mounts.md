# Mounts and vehicles — client side (26.3)

Scope: what the client does when the **local player** is a passenger or a driver. The physics of the
player on foot is in `movement.md` (implemented in rc-physics): we refer to it every time
a mount reuses the same computation. Packets `move_vehicle` / `paddle_boat`: reminder
`protocol-supplements.md` § 7. Rendering of boats and paddles: `entities-objects.md` § 1. Jump bar
and mount health: `hud.md` § 4.6-4.7. Camera: `hand-camera.md` § 3.

Vocabulary: **root** = lowest vehicle of the stack (we go up `vehicle` as long as it
exists). **Controller** of a vehicle = passenger who drives it (rules § 2). **Local authority**:
the vehicle's controller is the local player; the client then simulates the vehicle.

---

## 1. Mounting, dismounting, seat

### 1.1 Receiving `set_passengers`

1. Unknown vehicle → ignored.
2. `already_mounted` = the local player is a direct or indirect passenger of this vehicle.
3. Clear the vehicle's passengers (each one "dismounts" locally, from last to first).
4. For each id of the list, in order, if the entity exists: make it mount **by force**
   (no room test nor delay):
   - if it was already on another vehicle, it dismounts from it first;
   - its pose becomes "standing";
   - the order of the list is kept as is (the server already puts players first);
   - equids (llamas included) and camels: on adding, the passenger immediately takes the mount's view yaw
     (mod 360) and pitch (clamped ±90), old values included (no
     smoothing);
   - local player: forget the remembered "vehicle removed" id (`entities.md` § 4.3); then, if
     `already_mounted` was false:
     - vehicle = boat or raft (with or without chest): the player's current yaw, old yaw and
       head yaw = the boat's yaw;
     - action bar: `mount.onboard` (or `mount.sit` for a cushion), argument = name of the
       "sneak" key; message also sent to the narrator;
     - looping mount sound depending on the vehicle (§ 5.4).
5. Local dismount of the player (they are no longer in the list): "busy hands" reset to false.
   **No** position correction on the client side: the server then sends the dismount
   position (`player_position`). Meanwhile, the player stays where their seat was.

### 1.2 Tick order with passengers

Client world tick, for each **root** entity (non-passenger):
1. "common tick": old position/rotations ← current; then a network interpolation step
   if it has one (`entities.md` § 4);
2. the root's own tick (boat, living mount…);
3. for each passenger, recursively: if the passenger was removed or no longer has this vehicle →
   it dismounts; otherwise (player, or entity in the list of ticked entities): common tick, then
   **passenger tick**:
   - velocity ← 0;
   - normal tick of the entity (for the local player: key reading, AI, movement… — its
     own movement has no visible effect because it is overwritten right after);
   - placement on the seat (§ 1.3);
   - living: fall distance reset to 0;
   - local player only: "busy hands" ← false; if the vehicle **it controls** is a
     boat: boat inputs ← (left, right, forward, back) of the raw keys, and "busy
     hands" ← at least one of these four keys held.

Consequence: the driven vehicle reads, at tick N, the input set by the player at tick N − 1
(boat keys, lateral/forward inputs, pitch/yaw, jump). Reproduce this one-tick lag.

After the entity tick: sending of the player's packets (§ 4.1).

### 1.3 Seat position

`seat = vehicle position + P − V`, where:
- **P** = the vehicle's "passenger" attachment point, indexed by the passenger's rank in the list
  (clamped to the last defined point), rotated around Y by −vehicle yaw (a point (0, y, z)
  becomes (−z·sin(yaw), y, z·cos(yaw)), yaw in radians, usual convention);
- **V** = the passenger's "vehicle" attachment point, rotated by its own yaw. Player: (0; 0.6;
  0) × player scale. Types declaring a "ride offset" r: (0; −r; 0). Otherwise (0, 0,
  0).

For a living vehicle, the P points are scaled like its dimensions (baby, attribute
`minecraft:scale`); "scale" s below = scale × age scale.

P points (Y up, Z toward the vehicle's front):

| Vehicle | P |
|---|---|
| boat (wood) / chest boat | (0; h/3 = 0.1875; z) |
| raft / chest raft | (0; h × 0.8888889 = 0.5; z) |
| ↳ z (boats) | 1 passenger: 0 (chest versions: 0.15). 2 passengers: rank 0 → 0.2; rank 1 → −0.6; +0.2 if the passenger is an animal |
| horse | (0; 1.44375; 0) + rearing correction |
| donkey | (0; 1.1125; 0) + rearing |
| mule | (0; 1.2125; 0) + rearing |
| skeleton horse, zombie horse | (0; 1.31875; 0) + rearing (baby zombie horse: (0; 1.35; 0) × 0.7) |
| ↳ rearing correction | + (0; 0.15 × a; −0.7 × a) × s, rotated by the yaw, a = **old** value of the rearing animation (§ 3.3) |
| llama, trader llama | (0; 1.37; −0.3) |
| camel, camel husk | (0; y_c; z); z = 0.5 × s; if there is > 1 passenger: rank ≠ 0 → −0.7 × s, and +0.2 × s if animal; y_c § 3.4 |
| pig | (0; 0.86875; 0) |
| strider | (0; 1.7; 0) + **client oscillation** (0; 0.24 × cos(1.5 × walk_pos) × min(0.25; walk_speed) × s; 0) |
| happy ghast | rank 0 (0; 4; 1.7), rank 1 (−1.7; 4; 0), rank 2 (0; 4; −1.7), rank 3 (1.7; 4; 0) |
| nautilus, zombie nautilus | (0; 1.1375; 0) |
| minecarts (all) | (0; 0.1875; 0); villager and wandering trader: (0, 0, 0) |
| cushion | (0; 0.25; 0) (fallback: the entity's height) |
| other entity with no declared point | (0; height; 0) |

`walk_pos`/`walk_speed` = position and speed of the strider's walk animation
(`entities.md` § 1.4).

Additional placements done at the same time:
- equids and camels: the living passenger's body yaw ← the mount's body yaw;
- boats: rotation adjustment § 1.4;
- ordinary minecart in "new behavior" movement, accessibility option "turn
  with the minecart" active: § 3.8.

At render time, the passenger is interpolated like any entity between its old and new position
(both are successive seats): it therefore follows the mount without offset. Exception: "new
behavior" minecart (`entities.md`, passenger offset).

### 1.4 Passenger rotation

- **Boats** (except passengers of the tag `can_turn_in_boats` = breeze):
  1. if the passenger is simulated locally (local player): its yaw and head yaw +=
     the boat's `rotation_per_tick` (§ 3.2; is 0 when the boat is not under local authority);
  2. target body yaw: the boat's yaw; for an animal when the boat is full: boat
     yaw + (even id ? 90 : 270); the passenger's body yaw takes this value;
  3. non-mob passenger (player): d = wrap(yaw − body); yaw += clamp(d, −105, 105) − d.
     Mob: yaw = body, head clamped to the body.
  This clamping is also applied at each mouse movement (view rotation): after the
  rotation is computed, if the player has a vehicle, the vehicle applies the clamping and the **old**
  yaw receives the same correction (no interpolation jump).
- No other mount clamps the passenger's view. The camel limits the head of **its own**
  entity to 30° (not the player's).

### 1.5 Dismounting

- Only by the server. The client does nothing locally when "sneak" is pressed:
  the key state goes out in `player_input` (sent as soon as it changes); the
  server makes the player dismount on the next tick and sends back `set_passengers` then the position.
- During this short interval, the player's pose becomes "crouching" (the target pose is
  crouching, and a passenger always takes the target pose): crouched eye height
  and model for 1 to 2 ticks. The sneak slowdown flag, however, stays false as long as
  we are a passenger.
- Other dismounts (server): boat left 60 ticks underwater, downward bubble column
  under a boat (event 71), mount of the tag `dismounts_underwater` underwater, death, etc.

### 1.6 What changes for the passenger player

- Auto-jump: never. Slowdown by used item: not applied. Sneaking: see § 1.5.
- Sprint: possible only if the vehicle **can sprint** (camel only) **and** is under local
  authority; the sprint state goes out with the `move_vehicle` (§ 4.1).
- Creative flight: the double-jump toggle is only possible without a vehicle or if the controlled
  vehicle is a "jumper" (§ 3.3).
- **Busy hands** (driven boat + movement key held): attack/mining and
  use (right click) do nothing; the hands go down in first-person view
  (`hand-camera.md` § 1, `interactions.md`).
- Contextual bar: jump bar if the controlled vehicle is a jumper (`hud.md` § 4.6); mount
  health if it is living (`hud.md` § 4.7), which takes the place of hunger (`hud.md`
  § 4.4).
- Inventory key: if the (direct) vehicle has its own inventory — equids (horse, donkey, mule,
  llamas, skeleton/zombie horses), camels, nautiluses, chest boats and rafts — the client
  does **not** open its inventory: it sends `player_command` "open inventory" and waits for the
  server's screen (`open_horse_screen` / `open_screen`).

---

## 2. Who controls what

The controller is re-evaluated at **each** use (the held item, the saddle, the harness may change
along the way; authority then switches on the next tick).

| Vehicle | Controller (otherwise: none, simple passenger) |
|---|---|
| boat, raft, chest versions | 1st passenger if it is a player |
| horse, donkey, mule, skeleton horse, zombie horse | saddled **and** 1st passenger a player |
| camel, camel husk | saddled **and** 1st passenger a player (the 2nd seat is passive) |
| llama, trader llama | never (cannot wear a saddle) |
| pig | saddled, 1st passenger a player **holding a carrot on a stick** (either hand) |
| strider | saddled, 1st passenger a player **holding a warped fungus on a stick** |
| happy ghast | harness ("body" slot) **and** not "staying still" (entity data `stays_still` false) **and** 1st passenger a player |
| nautilus, zombie nautilus | saddled **and** 1st passenger a player |
| minecarts (all) | **never**: simulated by the server, whatever the behavior (§ 3.8) |
| cushion, other entities | never |

"Saddled" = the mount's saddle slot contains an item valid for that slot
(received through `set_equipment`). Mounting without control (untamed horse, pig without a stick…): the
vehicle stays interpolated from the server, the player is just moved back onto the seat.

For a vehicle under local authority, the movement packets received only update the
decoding base (`entities.md` § 4.3); only `teleport_entity` and `move_vehicle` move it.

---

## 3. Physics of driven mounts

### 3.1 Common to living mounts

The tick of a mount under local authority follows the generic AI (`movement.md` § 3) with these
differences:
- The mount's input does not come from its own inputs but from a **riding input**
  computed from the controller (the player's values set at the previous tick: lateral input L,
  forward input A, each ±0.98 on keyboard, "square" correction included, `movement.md` § 2.4).
- Order: riding input → **riding adjustment** (rotation, jump…) specific to each mount → mount
  speed ← **riding speed** → generic movement (`movement.md` § 4 ground/air, § 8
  water/lava) with this input, except for a mount that replaces the movement (ghast, nautilus).
- Differences from generic movement: air speed = speed × 0.1 (instead of 0.02); step
  height = max(attribute `step_height`, 1.0); no caution at the edge, no player
  sprint; water: same formula as the non-swimming player (water slowdown specific to the type:
  0.8, skeleton horse 0.96); mob stop thresholds (|v_x|, |v_z|, |v_y| < 0.003 → 0)
  instead of the player's.
- No "normal" jump: the mount's jump flag stays false; only ridden jumps
  (§ 3.3) exist.
- Under local authority, the effects of the blocks traversed apply on the client side (as for the
  player).

### 3.2 Boat and raft (identical physics)

Constants: gravity 0.04; hitbox 1.375 × 0.5625; step height 0; air drag
disabled; linear network interpolation 3 steps.

**State** recomputed at the start of each tick (old state ← state):
1. **Underwater**: let Y = top of the box + 0.001; walk the blocks x ∈ [⌊x_min⌋, ⌈x_max⌉),
   y ∈ [⌊y_max⌋, ⌈Y⌉), z likewise; a water block whose surface (y + fluid height) exceeds Y →
   if the water is not a source: **under flowing water** (immediately); otherwise mark **underwater**.
   In both cases, water level ← top of the box.
2. Otherwise **in water**: blocks y ∈ [⌊y_min⌋, ⌈y_min + 0.001⌉); water level = max of the water
   surfaces found (otherwise −∞); in water if a surface > y_min.
3. Otherwise **on land** if the average ground friction f > 0 (then ground_friction ← f), otherwise **in
   the air**.

Average ground friction: slice T = box reduced to [y_min − 0.001; y_min]. Walk x ∈
[⌊x_min⌋ − 1; ⌈x_max⌉ + 1), z likewise, y ∈ [⌊y_min − 0.001⌋ − 1; ⌈y_min⌉ + 1). Skip corner
columns (x and z both on the border). In a border column (only one coordinate on the border),
keep only the inner y layers (neither the first nor the last). Ignore lily pads.
Add up the friction of the blocks whose collision shape intersects T and count. f = sum /
count (0/0 = NaN → treated as "in the air"). Frictions: ice, packed ice, frosted
ice 0.98; blue ice 0.989; slime block 0.8; others 0.6.

"Out of control" counter: +1 per tick underwater (flowing or not), otherwise 0.

**Boat tick**, in order:
1. State (above), counter, then hit time − 1 and damage − 1 if they are > 0.
2. Base entity tick (fluids and water current push as `movement.md` § 8.1, fire,
   portals…).
3. **If local authority**:
   1. if the 1st passenger is not a player: paddles ← (false, false);
   2. **buoyancy** (below);
   3. **steering** (below);
   4. send `paddle_boat` (state of the two paddles);
   5. movement with collisions (`movement.md` § 6) of the velocity vector.
   **Otherwise**: velocity ← 0 and rotation_per_tick ← 0 (the position comes from interpolation).
4. Effects of the blocks traversed (two passes), bubble column (visual angle, `entities-objects.md`
   § 1.3), paddle positions (`entities-objects.md` § 1.4).
5. Pushable entities touching the box expanded by (0.2; −0.01; 0.2): the client only pushes
   them (making a mob mount is reserved to the server). Boat↔boat push only if
   the bottom of the other is below the top of this one; other entity only if its bottom ≤ the bottom
   of the boat.

**Buoyancy** (g = 0.04, v = −g, buoyancy b = 0, conservation k = 0.05):
- If old state = in the air and state ∉ {in the air, on land} (splashdown): water level ← current
  Y; target = water surface above − 0.5625 + 0.101; if the box moved to this
  height is free: Y ← target, v_y ← 0, last v_y remembered ← 0. State ← in water. **Nothing
  else this tick** (no damping).
  Surface above: for y from ⌊y_max⌋ to ⌈y_max − last v_y⌉ (excluded), height = max of the
  water heights of the blocks in the layer (layer stops as soon as 1); first layer < 1 →
  y + height; otherwise ⌈y_max − last v_y⌉ + 1.
- Otherwise depending on the state:
  - in water: b = (water level − Y) / 0.5625; k = 0.9;
  - under flowing water: v = −0.0007; k = 0.9;
  - underwater: b = 0.01; k = 0.45;
  - in the air: k = 0.9;
  - on land: k = ground_friction (the halving of ground_friction when a player drives is
    without effect: the value is recomputed on the next tick before use).
  Then v_x ×= k, v_z ×= k, v_y += v; rotation_per_tick ×= k; if b > 0:
  v_y = (v_y + b × 0.04/0.65) × 0.75.
- "Last v_y" = v_y remembered at the end of each movement (before the next damping).

**Steering** (only if the boat has at least one passenger); inputs L, R, Fw, Bk:
1. a = 0; L → rotation_per_tick −= 1; R → rotation_per_tick += 1;
2. if L ≠ R and neither Fw nor Bk: a += 0.005;
3. yaw += rotation_per_tick;
4. Fw → a += 0.04; Bk → a −= 0.005;
5. velocity += (sin(−yaw) × a; 0; cos(yaw) × a) (sin/cos table, yaw in radians);
6. paddles: left = (R and not L) or Fw; right = (L and not R) or Fw (metadata 11/12, rendering).

Orders of magnitude (deduced, calm water, no current): forward limit speed 0.04 × 0.9 /
(1 − 0.9) = 0.36 block/tick; limit rotation 0.9/0.1 = 9°/tick. On blue ice (k = 0.989):
≈ 3.6 blocks/tick (≈ 72 blocks/s). On ordinary land (k = 0.6): 0.06 block/tick.

**Bubble column**: on entity event 71 (downward): v_y −= 0.7; 72 (upward):
v_y = 0.6; only if the boat is simulated locally. On the column and not underwater, one
tick in 100: local splash sound (volume 1, pitch 0.8 + 0.4 × random), `splash` particle
at (x + random; y + 0.7; z + random).

**Paddle sounds**: played by the **server** (the client plays none). When a paddle is
active and its phase p satisfies `p mod 2π ≤ π/4` and `(p + π/8) mod 2π ≥ π/4`: sound
`entity.boat.paddle_water` (water or underwater) or `entity.boat.paddle_land` (on land), nothing in
the air; position boat + (±v_z; 0; ∓v_x) depending on the side (v = the boat's view direction),
volume 1, pitch 0.8 + 0.4 × random.

### 3.3 Equids (horse, donkey, mule, skeleton and zombie horses) and ridden jump

Attributes (received from the server through `update_attributes`; base values for the record):

| Mount | `movement_speed` | `jump_strength` | `step_height` |
|---|---|---|---|
| horse (random) | 0.1125 to 0.3375 (base 0.225) | 0.4 to 1.0 (base 0.7) | 1.0 |
| donkey, mule | 0.175 | 0.5 | 1.0 |
| skeleton horse | 0.2 | 0.4 to 1.0 | 1.0 |
| zombie horse | (9 + 3 random) / 42.16 ≈ 0.213 to 0.285 | 0.5 to 0.7 | 1.0 |
| camel | 0.09 | 0.42 | 1.5 |

**Jump charge** (local player, each tick in its AI, if the controlled vehicle is a jumper —
saddled equid or camel and not blocked, saddled nautilus — **and** its jump delay = 0; otherwise
charge ← 0):
1. if counter < 0: counter += 1; at 0: charge ← 0;
2. jump **released** (held at the previous tick, no longer now): counter ← −10; the mount
   receives n = ⌊charge × 100⌋; send `player_command` "start mount jump" with n;
3. otherwise jump **pressed** (not held before, held now): counter ← 0, charge ← 0;
4. otherwise jump held: counter += 1; charge = counter × 0.1 if counter < 10, otherwise 0.8 + 2 /
   (counter − 9) × 0.1 (1.0 at the 10th tick then goes back down toward 0.8).

**Mount receiving n** (horse and relatives): if saddled: (n < 0 → n = 0, otherwise
stand slide allowed + rear up if possible: rearing of 20 ticks — not for the camel)
; pending jump = 1 if n ≥ 90, otherwise 0.4 + 0.4 × n / 90.

**Riding adjustment** (horse and relatives, camel):
1. yaw ← player's yaw, pitch ← player's pitch × 0.5; old yaw = body yaw =
   head yaw = yaw;
2. if forward ≤ 0: gallop counter ← 0;
3. if on the ground: if pending jump > 0 → **ridden jump**; pending jump ← 0.

Ridden jump (equids): v_y = `jump_strength` × pending × block jump factor + jump
effect bonus (0.1 × (level + 1)); if forward > 0: velocity += (−0.4 × sin(yaw) × pending; 0;
0.4 × cos(yaw) × pending).

**Riding input**: null if on the ground, with no pending jump, rearing and without stand slide
allowed; otherwise (lateral × 0.5; 0; forward), with forward × 0.25 if forward ≤ 0 (slow reverse).
**Riding speed**: attribute `movement_speed`.

The generic mob blocking ("immobile": equid eating or rearing) resets to 0
the mount's **own** inputs, but the riding input does not use them: only the case
"on the ground, rearing, no stand slide" above stops a ridden mount.

Rearing animation (each tick, also used by the seat): old ← value; rearing: eating
← 0, value += (1 − value) × 0.4 + 0.05, clamped to 1; otherwise stand slide ← false, value +=
(0.8 × value³ − value) × 0.6 − 0.05, clamped to 0. Rearing lasts 20 ticks then stops.

Footstep sounds (played by the server, for the record): with a rider, gallop counter + 1 at each
step; > 5 and multiple of 3 → `entity.horse.gallop`; ≤ 5 → `entity.horse.step_wood`; volume
× 0.15. Jump sound (`entity.horse.jump`, 0.4): server, on receiving the jump packet.

### 3.4 Camel (and camel husk)

Same rules as § 3.3, with:
- **Sitting or in pose transition** ("refuses to move"): riding input null, riding
  rotation = current rotation (the player turns alone), not a jumper (no jump bar); on the ground,
  horizontal velocity ← 0. If forward > 0, sitting and not in transition: it stands up (sound
  `entity.camel.stand`, standing pose, change date ← game time).
  Sitting = metadata "pose change date" < 0; pose time = game time − |date|;
  transition if pose time < 40 (sitting) or < 52 (standing).
- **Riding speed** = `movement_speed` + 0.1 if the player sprints and the dash cooldown = 0.
  The camel can sprint (only mount).
- **Dash** (in place of the jump): receiving n only acts if saddled, dash cooldown ≤ 0 and on the
  ground. Execution: velocity += normalized horizontal look direction × 22.2222 × pending ×
  `movement_speed` × block speed factor + (0; 1.4285 × pending × jump strength; 0) with
  jump strength = `jump_strength` × block jump factor + effect bonus. Cooldown ← 55,
  "dash" (metadata) ← true.
- Each tick: if dashing and cooldown < 50 and (on the ground, in a liquid or passenger) → dash ← false;
  cooldown − 1 if it is > 0. Metadata "dash" received (outside the 1st tick): cooldown ← 55 if it was 0.
- Seat (height y_c, s = scale): base = height of the current dimensions − (baby 0.09375;
  adult 0.375); d = 1.43 × s; drop = d − 0.2 × s; low = d − drop.
  - in transition: duration = 40 (sits down) or 52 (stands up); midpoint = 28 (sits down); when
    standing up 24 (front) / 32 (rear); bend = sits down: 0.5 (front) / 0.1 (rear), stands up:
    0.6 / 0.35; t = clamp(pose time, 0, duration); 1st part if t < mid; part = t/mid or
    (t − mid)/(duration − mid); bend point = d − bend × drop;
    sits down: base += lerp(part, 1st ? d : bend_pt, 1st ? bend_pt : low);
    stands up: base += lerp(part, 1st ? low − d : low − bend_pt, 1st ? low − bend_pt : 0);
  - sitting outside transition: base += low;
  - "front" = driver (rank 0).
  The sitting dimensions (adult 1.7 × 0.945) serve as "dimension height" when the
  pose is sitting.
- Sounds (server): dash `entity.camel.dash`, dash ready `entity.camel.dash_ready`, step
  `entity.camel.step(_sand)`. Husk variant: `camel_husk` sounds.

### 3.5 Pig and strider (stick)

- Riding adjustment: yaw ← player's yaw, pitch ← pitch × 0.5; old yaw = body = head
  = yaw; boost tick (below). (Strider: same thing, done before the generic
  adjustment.)
- Riding input: always (0; 0; 1) — the mount moves forward by itself toward where the player looks.
- Riding speed: pig `movement_speed` (0.25) × 0.225 × boost factor; strider
  `movement_speed` (0.175) × (0.35 if "suffocating" (metadata), otherwise 0.55) × factor.
- **Boost** (stick used: decided by the server, which sends back the metadata "boost
  duration" T = 140 to 980 ticks): on receiving this metadata, the client goes into
  boost with t = 0. Each riding adjustment: if boosting, t += 1 and end when the
  old value of t exceeds T. Factor = 1 + 1.15 × sin(t / T × π) during the boost,
  otherwise 1. Right-clicking with the stick does nothing locally on the client (the server uses
  the stick).
- Strider on lava: a lava **source** whose block above is not lava
  offers a solid surface of height 0.5 (full column 0 → 8/16) if the strider is above;
  it does not "swim" in lava (normal ground/air movement). In non-supporting lava:
  velocity × 0.5 + (0; 0.05; 0) each tick, or "on ground" ← true if it is above the
  supporting surface.

### 3.6 Happy ghast

- Not a jumper; no gravity; own movement (replaces the generic one):
  s = `flying_speed` (0.05) × 5/3; in water: relative add(s, input), movement, velocity ×
  0.8; in lava: same × 0.5; elsewhere: same × 0.91. "Relative add" = `movement.md`
  (input normalized if length > 1, × s, rotated by the yaw).
- Riding input: lateral = player's L; if A ≠ 0: forward = cos(pitch), up =
  −sin(pitch), both × −0.5 if A < 0; otherwise forward = up = 0; if the player holds
  jump: up += 0.5; the whole × 3.9 × `flying_speed` (= 0.195). Note: A is only used by its
  sign.
- Riding adjustment: yaw += wrap(player's yaw − yaw) × 0.08 (slow turn); pitch ←
  player's pitch × 0.5; old yaw = body = head = yaw.
- Dismounting: only when looking downward ("sneak" makes you dismount from the mount).
- Up to 4 passengers; 3rd-person camera: `camera_distance` 8 (§ 5.2). Order of magnitude
  (deduced): 0.195 × 0.0833 / 0.09 ≈ 0.18 block/tick in level flight.

### 3.7 Nautilus (and zombie nautilus)

- Jumper (charge § 3.3): receiving n if saddled and dash cooldown ≤ 0 → pending = formula
  § 3.3 (no rearing).
- Riding input: like the ghast without the jump nor the factor (lateral, up, forward not scaled).
- Riding adjustment: yaw += wrap(player's yaw − yaw) × 0.5; pitch ← pitch × 0.5;
  old yaw = body = head; if pending > 0 and no own jump → **dash**; pending ← 0
  (even off the ground).
- Dash: velocity += **player's** look direction (3D) × (1.2 in water, 0.5 out of water)
  × pending × `movement_speed` × block speed factor; cooldown ← 40; metadata "dash" ← true.
  Each tick: dash ← false if cooldown < 35; cooldown − 1. Metadata "dash" received (outside the 1st tick):
  cooldown ← 40 if it was 0.
- Riding speed: in water 0.0325 × `movement_speed`, otherwise 0.02 × `movement_speed`
  (nautilus 1.0, zombie nautilus 1.1).
- In water, own movement: relative add(speed, input), movement, velocity × 0.9
  (all three axes, no gravity); not pushed by currents. Out of water: generic.
- A single passenger.

### 3.8 Minecart (server; what the client shows)

The client never simulates the minecart: it sends neither `move_vehicle` nor anything specific; the
server reads the forward/back/left/right keys of the last `player_input` and the player's yaw
(`move_player_rot`): if it is almost stopped (horizontal velocity² < 0.01), it adds
0.001 × the wanted direction (relative to the yaw). Rails, slopes, powered rails and braking
are computed on the server side.

Client display:
- **Classic behavior** (experimental rule "minecart improvements" inactive):
  3-step linear interpolation of the received positions (`entities.md`); with no interpolation in
  progress, the position is simply reapplied and the angles brought back modulo 360. The displayed
  speed (for the sound) comes from `set_entity_motion`.
- **New behavior** (rule active): no classic interpolation; the client receives
  trajectory steps (`move_minecart_along_track`: position, velocity, yaw, pitch, weight).
  Each tick: delay − 1; at 0: old point ← current state, the received list becomes the list
  in progress, total weight = sum, delay ← 3 (0 if total weight is zero). With a list in progress,
  position/velocity/angles = evaluation at `α = (3 − delay + pt) / 3`: we advance in the list
  to the step where the cumulative weight reaches α × total, local fraction = (α × total − previous
  cumulative) / step weight; interpolation between the previous step (or the old point) and this step
  (lerpAngle for the angles). The tick evaluates it at pt = 1; rendering and camera at the actual pt.
- Passenger camera in new behavior with a list in progress: minecart position evaluated
  at pt + (seat − minecart position − player's vehicle attachment) + smoothed eye height.
- Accessibility option "turn with the minecart" (default false), new behavior and
  ordinary minecart only: minecart rotation offset accumulated each tick where it moved
  by more than 0.01 (+= yaw variation mod 360); at the player's placement: y = lerpAngle(0.5;
  player offset; minecart offset), player yaw −= y − player offset, player offset
  ← y.

---

## 4. Packets sent

### 4.1 Each tick (after the entity and block-entity tick)

1. `player_input` if the key state (forward, back, left, right, jump, sneak,
   sprint) changed since the last send.
2. If passenger: `move_player_rot` (yaw, pitch, on ground, player's horizontal collision)
   **instead of** position packets; then if the **root** is under local authority:
   `move_vehicle` (position, yaw, pitch, on ground of the root; position = current
   interpolation target if there is one, otherwise position); then `player_command`
   start/stop sprint if the sprint state changed.
   Otherwise (not a passenger): normal position send.

### 4.2 During the tick

- `paddle_boat` (left, right): each tick of a boat under local authority, after the
  steering, before the movement (so before the `move_vehicle` of the same tick).
- `player_command` "start mount jump" with n = ⌊charge × 100⌋ (0 to 100): on jump
  release (§ 3.3). The server does nothing if n = 0 (no sound nor rearing on the server side). The
  client never sends "stop mount jump".
- `player_command` "open inventory": inventory key with a vehicle that has an inventory.

### 4.3 `move_vehicle` received (correction)

Reminder `protocol-supplements.md` § 7: only if the player is a passenger and their root
is under local authority; if the difference from the known target position > 1e-5: cancel
the interpolation, place exactly (yaw, pitch, old values = new ones); in all
cases, answer with a `move_vehicle` with the current state. The passengers are put back on the next
tick (placement on the seat).

---

## 5. Rendering, camera, sound

### 5.1 Player on their mount

- Position: lerp(pt) between two successive seats (§ 1.3); no network interpolation for the
  local player.
- Model: "sitting" pose of the passengers (`entities.md` § 3, arms and legs); walk
  animation at 0 for any passenger; body yaw clamped around the living mount's body
  (±85°, `entities.md`). No arm animation on the paddles: only the boat's paddles
  move (`entities-objects.md` § 1.4), the arms keep the sitting pose.
- 1st person: hands lowered when "busy hands" (`hand-camera.md` § 1); view bobbing
  null when mounted (`hand-camera.md`).

### 5.2 Camera

- Height: the player's lerp position (the seat) + smoothed eye height (`hand-camera.md`
  § 3.2); the eye height stays that of the standing player (1.62), crouched during the dismount
  (§ 1.5).
- 3rd person: distance = max(scale × the player's `camera_distance`, mount scale ×
  mount `camera_distance`) for a living mount (happy ghast: 8; others: 4 by
  default); non-living vehicles (boat, minecart) do not count. Pull-back by
  ray casts unchanged (`hand-camera.md` § 3.3).
- Orientation: the player's (their view is free, except for the boat clamping § 1.4).

### 5.3 Mount under local authority

Rendered with its own old/new values (position lerp, lerpAngle of the body
yaw); as the riding adjustment sets old yaw = yaw each tick, the mount's yaw
jumps from one tick to the next without smoothing (faithful to the official game).

### 5.4 Mount sounds (played by the client, looping)

When the local player mounts (§ 1.1):
- minecart: two sounds with variable volume, no attenuation, neutral category:
  `entity.minecart.inside_underwater` (active only if the player is underwater) and
  `entity.minecart.inside` (active only if not); volume = 0.75 × clamp(v, 0, 1)
  with v = the minecart's horizontal speed, if v ≥ 0.01 (and, in new behavior, if the
  minecart is on rails), otherwise 0; initial volume 0, updated each tick;
- happy ghast: `entity.happy_ghast.riding` (out of water), volume = 5 × clamp(v, 0, 1) with
  v = norm of the ghast's 3D velocity, if v ≥ 0.01, otherwise 0;
- nautilus: `entity.nautilus.riding` (underwater only), same formula (×5).
- Each sound follows the vehicle's position and stops when the player is no longer on this vehicle
  or the vehicle is removed; silent if the vehicle is "silent".
- All the other mount sounds (steps, gallop, paddles, dash, jump, splashes except bubble
  column) come from the server through sound packets.

---

## Uncertain points

- The player's own movement during their passenger tick is discarded, but its flags
  (horizontal collision, on ground) go out in `move_player_rot`: exact value not verified
  by capture; sending "false" is probably tolerated by the server.
- Cushion attachment point: deduced from the generic fallback (height 0.25), no override
  found; to confirm in game.
- Horse jump when the mount's "jump flag" is true: always false on the client side
  in the cases read; not verified for a horse whose server AI jumps at the moment control
  is taken back.
- Authority switch along the way (pig: change of held item): the client stops
  simulating on the next tick, but no explicit reset of the velocity nor of the
  interpolation was found; the next movement packet restarts the
  interpolation. To check visually (small jump possible).
- Actual volume of the ghast/nautilus mount sounds (up to 5): the sound engine probably clamps the
  volume to 1; to confirm in `sounds.md`/rc-audio.
- Ground/air physics of the nautilus out of water and of horses in water: assumed identical to the
  generic mob computation (`movement.md` § 4 and § 8 with the mob thresholds); the
  player/mob differences in `movement.md` are to be re-read before implementation.
- Exact order between receiving a `set_passengers` and the tick: handled on reception (network
  thread → main thread); if a `set_passengers` arrives while a boat input is
  pending, one tick's input is lost (no visible consequence).
