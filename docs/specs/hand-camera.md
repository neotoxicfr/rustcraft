# First-person hand and camera (26.3)

Source: reading of the decompiled client (kept outside the repository), reworded. See also: `entities.md` (§1.10
attack swing, §3 player: model, slim/wide arms, sleeves, skin, entity light), `items.md` (item models, `display`
transforms), `environment.md` (fluid in the camera), `hud.md` §5.5 (camera overlays: spyglass, powder snow…),
`movement.md`.

Conventions. Angles in degrees unless stated otherwise. pt = partial tick. "Stack" = transform stack;
each operation is **post-multiplied** (applied in the current frame, like a pose stack):
T(x, y, z) translation, Rx/Ry/Rz(a) rotation around +X/+Y/+Z, S(…) scale,
"around(R, p)" = T(p) · R · T(−p). i = +1 for the right arm, −1 for the left. View frame:
+X right, +Y up, −Z forward. sin/cos in radians on the arguments shown (π explicit).

Camera orientation (yaw λ, pitch θ in degrees, θ clamped to [−90, 90] at each mouse
rotation, value sent as is to the server): camera = Ry(π − λ) · Rx(−θ), view = its
inverse = Rx(θ) · Ry(λ − π). No "look at" construction with a fixed +Y up: it
degenerates at θ = ±90 (in single precision cos(π/2) < 0, the horizontal gaze flips and the image
turns upside down by 180°). The screen axes useful for camera-facing signs, particles, the near
plane and the listener are read from the rows of this matrix (right, up, −gaze).

## 1. Projections and drawing order

### 1.1 World
- Perspective: vertical field = fov (§4), window aspect ratio, near plane
  **0.05**, far plane `far_plane = max(effective render distance × 16 × 4, cloud range × 16)`.
- **Reversed depth** ("reverse-Z"): the matrix is built with near and far
  swapped; depth 1 at the near plane, 0 at the far plane; the depth buffers are cleared to
  **0** (range [0, 1] if the device supports it, otherwise [−1, 1]).
  Default test "closer or equal" (≥); positive depth offsets = toward the camera.
- **Layer offset** (perspective): for armor, trims, lines, block outlines,
  filled debug boxes and shadows, the view is scaled by (1 − 1/4096) around the eye
  (same pixel, depth slightly closer); "toward the back" variant (1 + 1/4096) for the back of paintings
  and the frame of item frames. In orthographic projection, it is a z translation
  of 1/512.
- Bobbing: world projection matrix = P × B, where B = (empty) stack on which the damage shake
  (§5.2) is applied, then, if the option `bobView` is true, the view bob (§5.1).
  B thus applies in view space, after the camera rotation.
- Then portal/nausea distortion (out of scope, see `hud.md`).
- Culling frustum: same projection but with fov = max(current fov, option fov) —
  a reduced fov (bow, spyglass) does not make sections outside the reduced field disappear.

### 1.2 Hand ("3D HUD")
After the world render:
1. Own projection: perspective with field **hand_fov** (= 70, modified only by death and
   fluid, §4.3 — **not** by the fov option nor sprinting), near plane 0.05, far `far_plane`,
   same aspect ratio, same reversed depth. No nausea/portal distortion.
2. **Depth cleared to 0** (the hand is always in front of the world). If post-processing
   effects are active (view from an entity: creeper, spider, enderman…), a separate
   depth target is used, then copied into the main depth; otherwise the main
   depth itself.
3. "Hand" pass (§2) into the main colour.
4. "Screen effects" pass (fire, inside a block, underwater — `textures/misc/underwater.png`)
   with the same depth: they are drawn **on top of** the hand.
5. Possible 3D debug crosshair.

### 1.3 When the hand is drawn
All conditions: no panorama mode; a local player exists; camera in **first
person**; the camera entity is not sleeping; HUD not hidden (F1); game mode ≠ spectator.
Starting frame: view space (the camera rotation is cancelled), then damage shake (§5.2),
then view bob if `bobView` (§5.1), then §2.2.

## 2. Hand and held items

### 2.1 State per tick (local player)
Two hands (main P, secondary S), each with: **displayed** item, height `h` ∈ [0, 1] and
previous height. On each tick:
1. previous heights = heights; expected real items `eP`, `eS` (contents of the hands).
2. Immediate replacement of the displayed item by the expected item if: they are "the same"
   ignoring the `max_damage` and `damage` components (same item, same quantity, other components
   equal), **or** if the expected item does not play the swap animation (property
   `hand_animation_on_swap` at the root of the `items/<id>.json` file designated by `item_model`, default true; also true if the item has no
   `item_model` component).
3. If the hands are busy (the player is steering a boat by pressing a
   movement key): hP = clamp(hP − 0.4; 0; 1), likewise hS.
   Otherwise: target P = 0 if the displayed item ≠ eP, otherwise e³ with e = clamp((swap_counter + 1) /
   attack_delay; 0; 1); target S = 0 if the displayed item ≠ eS, otherwise 1. h += clamp(target − h;
   −0.4; 0.4).
4. If hP < 0.1: displayed item P = eP. Same for S.
- `attack_delay` = 20 / value of the `attack_speed` attribute (ticks; 5 bare-handed).
  `swap_counter`: +1 per player tick; reset to 0 (along with the attack strength counter) when
  the main-hand item changes **item** (not only components), on every entity attack, on a left click
  into the void, and when the mining of a block is interrupted. Effect: the
  main hand lowers then rises again after an attack or an item change.
- Use of an item accepted on the client side (right click that succeeds, or placement on a block
  that changes the quantity / in creative): height of the hand concerned set to 0 ("bounce").
- Render: `hh` (inverse height) = swap_scale × (1 − lerp(pt, previous h, h)), where
  swap_scale = `swap_animation_scale` of the `items/<id>.json` file (default 1; 1 without
  `item_model`).

### 2.2 Tilt following rotation ("hand sway")
Per-tick smoothing of the local player when it is the camera (when applying inputs):
`sway_x += (pitch − sway_x) × 0.5`, `sway_y += (yaw − sway_y) × 0.5` (previous values kept).
At render time, before the two hands: Rx((view_pitch − lerp(pt, prev_sway_x, sway_x)) × 0.1) then
Ry((view_yaw − lerp(pt, prev_sway_y, sway_y)) × 0.1), view pitch/yaw interpolated.

### 2.3 Which hands
Tests on the real contents of the hands (inventory), not on the item displayed during the swap.
- Neither bow nor crossbow in either hand: both.
- Otherwise, if the player is using nothing: main only if the main is a loaded crossbow,
  otherwise both.
- Otherwise, item in use = bow or crossbow: only the hand that is using it. Other item in use:
  main only if the use is in the main hand and the secondary is a loaded
  crossbow, otherwise both.
- Order: main then secondary. Arm drawn for a hand = main arm (metadata
  "main hand") for P, the opposite for S.
- **Spyglass** being used ("scoping"): no hand is drawn.
- Swing per hand: `a` = render value of the attack swing (`entities.md` §1.10) if the
  hand of the current swing is this hand (main hand by default if there is none),
  otherwise 0. We write s = √a.
- Light: that of the local player entity (block/sky light at its position, as for its
  third-person render). No overlay (white/red) on the hand nor on the item.

### 2.4 Empty arm (empty main hand)
Only for the main hand (an empty secondary hand shows nothing), and only if the
player is **not invisible**:
1. T(i × (−0.3 sin(sπ) + 0.64), 0.4 sin(2πs) − 0.6 − 0.6·hh, −0.4 sin(aπ) − 0.72)
2. Ry(45 i); Ry(70 i sin(sπ)); Rz(−20 i sin(a²π))
3. T(−i, 3.6, 3.5); Rz(120 i); Rx(200); Ry(−135 i); T(5.6 i, 0, 0)
4. Drawing the "arm" part of the player model (right or left, slim or wide depending on the skin,
   `entities.md` §3.2): pose reset to its default value (pivot (∓5, 2, 0) px, zero rotations)
   then Z rotation = +0.1 rad (right arm) / −0.1 rad (left arm); the sleeve (child, inflated by
   0.25 px) visible if the matching sleeve bit of the "skin parts" is set. Model coordinates in
   pixels /16 (like any model part), **without** the (−1, −1, 1) flip nor the 0.9375 scale
   of entity rendering. Render type `entity_translucent`, skin texture.

### 2.5 Transforms common to items
- Item arm `A(hh)`: T(0.56 i, −0.52 − 0.6·hh, −0.72).
- "Whack" hit `W(a)`:
  T(−0.4 i sin(√a π), 0.2 sin(2π√a), −0.2 sin(aπ)); Ry(i × (45 − 20 sin(a²π)));
  Rz(−20 i sin(√a π)); Rx(−80 sin(√a π)); Ry(−45 i).
- Charge shake (bow, crossbow, trident) with t = time held, p = power:
  if p > 0.1: T(0, 0.004 × sin((t − 0.1) × 1.3) × (p − 0.1), 0).
- Time held: t = item use duration − (remaining use ticks − pt + 1).
- Final drawing of the item: item model resolved in the context `firstperson_righthand` if the arm
  is right, `firstperson_lefthand` otherwise (so a left-hander's main hand uses the left
  context), without overlay. Application of the context's `display` transform:
  T(tx, ty, tz); Euler rotation X then Y then Z (rx, ry, rz); S(sx, sy, sz); T(−0.5, −0.5, −0.5).
  Left context: **always** tx, ry, rz taken with the opposite sign (mirror), whether the transform
  comes from an explicit `firstperson_lefthand` key or from the fallback on `firstperson_righthand` (fallback
  applied when the model file has no left key). JSON values: translation /16 clamped to
  ±5, scale clamped to ±4. Base examples: `item/generated` and `item/handheld` right
  rotation (0, −90, 25); `block/block` right (0, 45, 0), left (0, 225, 0).
- The "item in use" case below applies if: the player is using an item, use ticks
  remain > 0, and the hand used is this hand.

### 2.6 Any item (except map and crossbow)
1. **In use**: if the use animation has no "own arm transform"
   (all except `eat`, `drink`, `spear`): A(hh). Then depending on the animation (`consumable`/components,
   see `item-components.md`):
   - `none`, `spyglass`, `toot_horn`, `crossbow` (apart from the real crossbow): nothing more.
   - `eat`, `drink`: c = remaining − pt + 1; f = c / use duration; if f < 0.8:
     T(0, |cos(c/4 × π) × 0.1|, 0); j = 1 − f²⁷; T(0.6 j i, −0.5 j, 0); Ry(90 i j); Rx(10 j);
     Rz(30 i j); then A(hh).
   - `block`: if the item is **not** a shield: T(−0.14142136 i, 0.08, 0.14142136);
     Rx(−102.25); Ry(13.365 i); Rz(78.05 i). Shield: nothing (the raised pose comes from its model).
   - `bow`: T(−0.2785682 i, 0.18344387, 0.15731531); Rx(−13.935); Ry(35.3 i); Rz(−9.785 i);
     p = t/20; p = (p² + 2p)/3; p = min(p, 1); shake; T(0, 0, 0.04 p); S(1, 1, 1 + 0.2 p);
     Ry(−45 i).
   - `trident`: T(−0.5 i, 0.7, 0.1); Rx(−55); Ry(35.3 i); Rz(−9.785 i); p = min(t/10, 1);
     shake; T(0, 0, 0.2 p); S(1, 1, 1 + 0.2 p); Ry(−45 i).
   - `brush`: r = remaining mod 10; u = 1 − (r − pt + 1)/10; θ = −15 + 75 cos(2π u).
     Right arm: T(−0.25, 0.22, 0.35); Rx(−80); Ry(90); Rx(θ).
     Left arm: T(0.1, 0.83, 0.35); Rx(−80); Ry(−90); Rx(θ); T(−0.3, 0.22, 0.35).
   - `bundle`: W(a).
   - `spear` (spear, 26.x): T(0.56 i, −0.52, −0.72) (without hh) then §2.8 "use".
2. **Otherwise, Riptide spin attack in progress**: A(hh); T(−0.4 i, 0.8, 0.3); Ry(65 i);
   Rz(−85 i).
3. **Otherwise**: A(hh); if a swing is in progress **for this hand**: type `whack` → W(a);
   type `stab` → §2.8 "attack"; `none` → nothing.
4. Drawing the item (§2.5).

### 2.7 Crossbow (item `minecraft:crossbow`)
A(hh) first, then:
- Charging (used by this hand, not loaded): T(−0.4785682 i, −0.094387,
  0.05731531); Rx(−11.935); Ry(65.3 i); Rz(−9.785 i); p = min(t / charge duration, 1);
  shake; T(0, 0, 0.04 p); S(1, 1, 1 + 0.2 p); Ry(−45 i). Charge duration = 1.25 s = 25
  ticks modified by enchantments (Quick Charge).
- Otherwise: W(a) (whatever the swing type); then if loaded, a < 0.001 and main
  hand: T(−0.641864 i, 0, 0); Ry(10 i) (aiming).
- Drawing the item.

### 2.8 Spear (component `kinetic_weapon`)
Easing functions (x ∈ [0, 1]): easeInQuad = x²; easeInOutSine = (1 − cos(πx))/2;
easeOutBack = 1 + 2.70158 (x−1)³ + 1.70158 (x−1)²; easeOutCubic = 1 − (1−x)³; easeOutQuart = 1 − (1−x)⁴;
easeOutCirc = √(1 − (x−1)²); easeInCirc = 1 − √(1 − x²);
easeInOutExpo = 0 if x = 0, 1 if x = 1, 2^(20x−10)/2 if x < 0.5, otherwise (2 − 2^(−20x+10))/2;
easeInOutBack = 4x²(7.189819 x − 2.5949094)/2 if x < 0.5, otherwise (d²(3.5949094 d + 2.5949094) + 2)/2
with d = 2x − 2;
easeInOutElastic = 0 / 1 at the bounds, otherwise with σ = sin((20x − 11.125) × 4π/9):
−2^(20x−10) σ / 2 if x < 0.5, otherwise 2^(−20x+10) σ / 2 + 1.
prog(v, a, b) = clamp((v − a)/(b − a), 0, 1).

Use parameters at time t (ticks held): D = `delay_ticks`;
swing_end = D + `max_duration_ticks` of `dismount_conditions` (0 if absent);
lower_end = D + same of `knockback_conditions`; raise_end = D + same of
`damage_conditions`.
- lift = prog(t, 0, D); liftStart = prog(lift, 0, 0.5); liftMid = prog(lift, 0.5, 0.8);
  liftEnd = prog(lift, 0.8, 1);
- swing = prog(t, swing_end − 20, swing_end);
- lower = easeOutCubic(easeInOutElastic(prog(t − 20, lower_end − 40, lower_end)));
- raise = prog(t, raise_end − 5, raise_end);
- intensity = 2 easeOutCirc(swing) − 2 easeInCirc(raise); slow = sin(t × 19°) × intensity;
  fast = sin(t × 30°) × intensity.

Impact recoil: r(τ) = 0.4 × (easeOutQuart(prog(τ, 1, 3)) − easeInOutSine(prog(τ, 3, 10))), τ = ticks
since the last kinetic impact (current world time − impact time + pt; 0 if there
has never been one; an impact is only recorded if more than 10 ticks have elapsed since the previous one).

**Use** (after T(0.56 i, −0.52, −0.72)):
1. T(i (0.15 lift − 0.05 liftEnd − 0.1 swing + 0.005 slow), −0.075 lift + 0.075 liftMid + 0.01 fast,
   0.05 liftStart − 0.05 liftEnd + 0.005 slow)
2. around(Rx(−65 easeInOutBack(lift) − 35 lower + 100 raise − 0.5 fast), (0, 0.1, 0))
3. around(R−Y(i (−90 prog(lift, 0.5, 0.55) + 90 swing + 2 slow)), (0.15 i, 0, 0)) — rotation around
   −Y, i.e. Ry of the opposite.
4. T(0, −r(τ), 0).

**"Stab" attack** (after A(hh)): st = easeInOutSine(prog(a, 0, 0.05)); md = easeOutBack(prog(a, 0.05,
0.2)); en = easeInOutExpo(prog(a, 0.4, 1));
T(0.1 i (st − md), −0.075 (st − en), 0.65 (st − md)); Rx(−70 (st − en)); T(0, 0, −0.25 (en − md)).

### 2.9 Maps (item with `map_id` component)
Map in the main hand **and** empty secondary hand → two-handed map; otherwise one-handed map
(in the hand where it is).

Drawing the map `C(main?)`: Ry(180); Rz(180); S(0.38); T(−0.5, −0.5, 0); S(1/128);
quadrilateral z = 0 from (−7, −7) to (135, 135), UV (0,0) at (−7,−7), (1,0) at (135,−7), (1,1) at
(135,135), (0,1) at (−7,135), white colour, hand light, "text" render type;
texture `textures/map/map_background_checkerboard.png` if the map data is known,
otherwise `textures/map/map_background.png`. Then the map content (pixels, decorations; out of
scope) in this frame (0..128).

One hand: T(0.125 i, −0.125, 0); if not invisible: [Rz(10 i); full arm §2.4 with hh and a]
(isolated); then isolated: T(0.51 i, −0.08 − 1.2 hh, −0.75); x = sin(sπ);
T(−0.5 x i, 0.4 sin(2πs) − 0.3 x, −0.3 sin(aπ)); Rx(−45 x); Ry(−30 i x); C(arm = main arm).

Two hands: T(0, 0.1 sin(aπ), −0.4 sin(sπ)); tilt m: k = clamp(1 − pitch/45 + 0.1, 0,
1), m = 0.5 − 0.5 cos(kπ); T(0, 0.04 − 1.2 hh − 0.5 m, −0.72); Rx(−85 m);
if not invisible: isolated [Ry(90); right hand; left hand], each "map hand" = isolated
[Ry(92); Rx(45); Rz(−41 i); T(0.3 i, −1.1, 0.45); arm (step 4 of §2.4 only)];
then Rx(20 sin(sπ)); S(2); C(main).

### 2.10 Invisibility, water, miscellaneous
- Invisible: no arm (empty, map); the **items** and the map stay drawn normally.
- Underwater: no particular hand transform; only hand_fov changes (§4.3) and the
  `underwater` overlay goes on top. [Fog during the hand pass: not verified.]
- Spectator, sleeping, F1, third person: no hand (§1.3).

## 3. Camera

### 3.1 Modes
`first_person` → `third_person_back` → `third_person_front` → … (key `key_key.togglePerspective`,
F5 by default). The mode is **not** saved in options.txt (first person at launch).
On switching first ↔ third person, the post-processing effect of the spectated entity is
re-evaluated (removed in third person).

### 3.2 Position and orientation (each frame)
- Eye height smoothed per tick: `eye += (entity eye height − eye) × 0.5` (previous value
  kept); at render time lerp(pt, previous, eye).
- Position = lerp(pt) of the camera entity's position + (0, eye, 0). Special case: passenger
  of a minecart in "new behaviour" with interpolation active: interpolated position of the
  minecart + passenger point offset (out of detailed scope).
- Orientation: view yaw/pitch of the entity (interpolated). Quaternion = YXZ rotation
  (π − yaw in rad, −pitch in rad, 0); forward = rotation of (0, 0, −1), up of (0, 1, 0),
  left of (−1, 0, 0). View matrix = inverse rotation (conjugate), camera translation handled
  separately (camera-relative rendering).
- camera partial tick = 1 if the entity is frozen by the tick rate manager.

### 3.3 Third person
- Front (`third_person_front`): yaw + 180, opposite pitch.
- Requested distance: d₁ = entity scale (attribute `scale`) × attribute `camera_distance`
  (default **4**, clamped [0, 32], synchronised by the server) if it is living, otherwise 1 × 4; if
  it is a passenger of a living mount, d₂ = scale × `camera_distance` of the mount;
  d = max(d₁, d₂).
- Pull-back ("max zoom"): 8 rays, one per corner of a cube of half-side 0.1: start = camera
  position + (±0.1, ±0.1, ±0.1), end = start − forward × d; cast against the blocks with the
  **visual shape** (fluids ignored, context = camera entity). If hit and
  |impact − camera position|² < d²: d = that distance (measured from the camera position, not the
  ray start). The smallest is kept.
- Visual shape = collision shape, except: glass and "transparent" blocks → empty; bars
  (iron, copper…) → empty; powder snow → empty; mud and soul sand → full cube; fences →
  outline shape; snow layers → outline shape depending on the number of layers. So the camera
  passes through panes, bars, powder snow.
- Then displacement of −d along forward (pull-back). No distance smoothing.
- Local player: drawn as a normal entity when the camera is detached (third person)
  or when the camera entity is sleeping; in first person the camera entity is not drawn. Another
  local player (split screen) is never drawn unless it is the camera.

### 3.4 Sleeping (first person)
Yaw = bed orientation − 180 (0 if unknown), pitch 0, then a rise of 0.15 along up.

## 4. Field of view

### 4.1 Option
`fov`: integer 30 to 110, default 70; options.txt stores (fov − 70)/40 (real). Labels: 70 →
`options.fov.min` ("Normal"), 110 → `options.fov.max` ("Quake Pro"). `fovEffectScale`: real
0 to 1, default 1; the slider displays √value (value = position²). `bobView` (bool, default true).
`damageTiltStrength`: 0 to 1, default 1.

### 4.2 Modifier (per tick)
Target (camera entity = player; otherwise 1):
1. m = 1; if flying (abilities): m × 1.1.
2. v = walking speed of the abilities (packet `player_abilities`, default 0.1); if v ≠ 0:
   m × ((attribute `movement_speed` / v + 1) / 2). Sprinting (+30 % total multiplicative), Speed,
   Slowness, powder snow (modifier `minecraft:powder_snow` = −0.05 × freeze fraction, set by the
   server and received through the attributes update) all go through this attribute.
3. In use: bow → m × (1 − min(use ticks/20, 1)² × 0.15); otherwise spyglass in
   first person → the target is **0.1** directly (without the effect scale).
4. target = lerp(fovEffectScale, 1, m).
Smoothing: previous = mod; mod += (target − mod) × 0.5; mod = clamp(mod, 0.1, 1.5).

### 4.3 fov per frame
- fov = fov option × lerp(pt, previous mod, mod) (90 in panorama mode, no other modification).
- Death: if the camera entity is living and dying, d = min(death_time + pt, 20),
  fov /= (1 − 500/(d + 500)) × 2 + 1.
- Fluid in the camera (`environment.md`) water or lava: fov × lerp(fovEffectScale, 1, 6/7)
  (0.85714287). Powder snow: nothing.
- hand_fov = 70 with the same two corrections (death, fluid).

## 5. View bob and damage shake

### 5.1 View bob (option `bobView`)
Only if the camera entity is a player.
- Per player tick (before the AI): b_target = min(0.1, horizontal speed) if on the ground, not dying
  and not swimming, otherwise 0; previous = bob; bob += (b_target − bob) × 0.4. When mounting:
  previous = bob, bob = 0.
- Walk distance (local player only): on each movement, += 0.6 × length of the effective
  horizontal movement; previous value stored at the start of the tick.
- At render time: w = −(walk + (walk − previous) × pt) (extrapolation, negative sign);
  b = lerp(pt, previous bob, bob);
  T(sin(wπ) × b × 0.5, −|cos(wπ) × b|, 0); Rz(sin(wπ) × b × 3); Rx(|cos(wπ − 0.2) × b| × 5).
- Applied to the world projection (§1.1) and to the hand stack (§1.3); for the hand, this is
  the same stack that makes it oscillate (no extra bobbing of its own).

### 5.2 Damage shake and death roll
If the camera entity is living:
- Dying: Rz(40 − 8000 / (d + 200)), d = min(death_time + pt, 20).
- h = hurt_time − pt; if h < 0: stop. h = sin((h / hurt duration)⁴ × π);
  Ry(−dir); Rz(−h × 14 × damageTiltStrength); Ry(dir), where dir = hurt direction
  (`hurt_animation`: yaw received; see `entities.md`), hurt_time/duration = 10 on receipt.
- Order: shake **before** the view bob, on the world and on the hand.

## 6. Uncertain points
- Fog active during the hand pass (underwater / lava): not tracked.
- Fallback `firstperson_lefthand` → `firstperson_righthand`: done when loading each model
  file; the exact interaction with `parent` inheritance (per-context resolution, `items.md`) has not
  been verified.
- Hand light = local player's entity light; exact sampling point: that of the
  generic entity render (`entities.md`), not re-verified here.
- `spyglass` and `toot_horn` use animations: no own hand transform found
  (the spyglass hides the hands; the horn has only A(hh)).
- Minecart "new behaviour": passenger offset not detailed.
- Visual shape: list of exceptions established by searching the overrides; other blocks inheriting
  from "transparent" (tinted glass, etc.) follow the same empty rule.
