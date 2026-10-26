# Per-entity rendering — animals and mobs (26.3)

Clean-room specification, written from a reading of the decompiled 26.3 client (protocol 777). No line of
source code; formulas, constants, resource paths and layer names only. It completes `entities.md`, part
"mission C" (referred to below as **C**): the cross-references "C §x" point to its sections (C §1 generic
rendering of a living entity, C §1.4 limb swing, C §2.4 quadruped, C §3.5 equipment layers…). Part angles in
radians, entity angles in degrees unless stated.

Common notations (identical to C): `P` = walk_position, `V` = walk_speed (C §1.4),
`age` = age_ticks = ticks_lived + pt, `head_yaw` = relative head yaw (degrees), `pitch` = pitch (degrees),
`pt` = fraction of a tick. "=" writes an absolute value, "+=" an increment on the base pose (all
parts return to their definition pose before each animation, C §0).

---

## 0. Rules common to the batch

### 0.1 Layers and textures

- All the layers cited exist in `entity_models.json` (verified). The "baked-in" scales (living horse
  ×1.1; donkey ×0.87; mule ×0.92; elder guardian ×2.35; ghast ×4.5; happy ghast ×4 and baby
  ×0.2375 in addition; salmon ×0.5/×1.5) are **already applied** in the extracted layers: do not
  reapply them at render time.
- Texture paths: `textures/<path>.png` under `assets/minecraft/`; all verified present.
- Body render type: cutout ("entity_cutout", C §1.7) unless stated otherwise.
- Baby model: when the baby flag (index 16) is true, the renderer replaces the adult model by the
  baby model (layer `<kind>_baby#main`); no additional reduction. The shadow radius is
  multiplied by `age_scale` (0.5 by default; exceptions: camel 0.6, happy ghast 0.2375) and by
  the `scale` attribute (C §1.5).

### 0.2 Metadata indices (continuation of C, part 1 § 3)

Inheritance: entity 0‑7, living 8‑14, mob 15, then:

| Kind(s) | Chain | Own indices |
|---|---|---|
| horse | ageable mob (16 baby, 17 age locked) → equine | 18 flags byte; 19 variant integer |
| donkey, mule | → chested equine | 18 flags; 19 chest boolean |
| skeleton horse, zombie horse | → equine | 18 flags |
| llama, trader llama | → chested equine → llama | 18 flags; 19 chest; 20 strength integer; 21 variant integer |
| camel, camel husk | → equine → camel | 18 flags; 19 dash boolean; 20 long tick of the last pose change |
| mooshroom | ageable mob → animal → cow | 18 variant integer |
| ocelot | ageable mob → animal | 18 trusting boolean (no rendering effect) |
| rabbit | ageable mob → animal | 18 variant integer |
| parrot | ageable mob → tameable (18 flags byte, 19 owner) | 20 variant integer |
| polar bear | ageable mob → animal | 18 standing boolean |
| axolotl | ageable mob → animal | 18 variant integer; 19 playing-dead boolean; 20 from-bucket boolean |
| cod, salmon, pufferfish, tropical fish | mob → water mob → fish (16 from-bucket boolean) | salmon 17 size; pufferfish 17 puff state; tropical 17 packed variant integer |
| squid | ageable mob → ageable water mob | none |
| glow squid | → squid | 18 integer remaining darkness ticks |
| guardian, elder guardian | mob → monster | 16 moving boolean; 17 integer beam target id (0 = none) |
| slime, magma cube | ageable mob → cube mob | 18 size integer |
| sulfur cube | → cube mob | 18 size; 19 integer max fuse (−1 = not primed); 20 from-bucket boolean |
| ghast | mob | 16 charging (shooting) boolean |
| happy ghast | ageable mob → animal | 18 holds leashes boolean; 19 stays still boolean |
| Ender Dragon | mob | 16 phase integer |
| nautilus | ageable mob → tameable (18, 19) → nautilus | 20 dash boolean |
| zombie nautilus | → nautilus | 21 variant (registry `minecraft:zombie_nautilus_variant`) |

Equine flags (index 18): 0x02 tamed, 0x08 bred, 0x10 eating, 0x20 rearing, 0x40 mouth
open. Tameable flags (index 18): 0x01 sitting.

### 0.3 Equipment of animals

Saddles, horse and nautilus armour, llama carpets, ghast harnesses: items received through the equipment
packet in the `saddle` and `body` slots. Rendered by equipment layer exactly as
C §3.5: the item's `equippable` component is read; nothing if `asset_id` is absent; the file
`equipment/<asset>.json` gives, for the requested layer type, the list of textures
`textures/entity/equipment/<type>/<texture>.png` (optional `dyeable` tint: leather horse armour,
default colour 0xFFA06540, plus a second undyed `leather_overlay` layer). The layer model
runs the same animation as the body (C §1.2, step 8).
- **Babies**: unless stated otherwise (baby happy ghast harness, trader llama carpet), no equipment
  layer is drawn on a baby (no baby model provided).
- "Order" below = submission rank in the pass (0 by default; a larger rank is drawn
  after, a negative rank before); used to break ties between coplanar surfaces.

### 0.4 Keyframe animations

Some kinds (camel, rabbit, baby axolotl, nautilus) animate their parts through client-specific
**keyframe tables**, absent from `entity_models.json`. They must be added to the extraction
(proposal: `entity_animations.json`, one entry per animation: `length` in seconds, `looping`
boolean, and per part name a list of channels `{target: position|rotation|scale, keyframes: [{t,
pre:[x,y,z], post:[x,y,z], interp: linear|catmullrom}]}`). Values to be stored **already converted**:
rotation in radians (degrees × π/180), position with y negated (x, −y, z), scale as a difference from 1 (s − 1).

Applying an animation at a time t (seconds) with an intensity factor k:
1. if `looping`, t = t mod length;
2. for each channel: i = first index whose timestamp ≥ t; prev = max(0, i − 1);
   next = min(n − 1, prev + 1); α = clamp((t − t_prev)/(t_next − t_prev), 0, 1) if next ≠ prev, otherwise 0;
3. interpolation chosen by the **next** keyframe:
   - linear: v = lerp(α, post[prev], pre[next]) × k;
   - Catmull-Rom: with p0 = post[max(0, prev−1)], p1 = post[prev], p2 = post[next],
     p3 = post[min(n−1, next+1)], per component:
     v = 0.5 × (2p1 + (p2 − p0)α + (2p0 − 5p1 + 4p2 − p3)α² + (3p1 − p0 − 3p2 + p3)α³) × k;
4. the result is **added** to the part: position += v (pixels), rotation += v, scale += v.

Two driving modes:
- **by state**: an animation state remembers the starting client tick (or "stopped"); if started,
  t_ms = truncate((age − start_tick) × 50) then t = t_ms / 1000, k = 1. "Start" resets the
  start; "start if stopped" does nothing if it is already running; "stop" makes it inactive.
- **by walk** (parameters f, g): t_ms = truncate(P × 50 × f), k = min(V × g, 1).

---

## 1. Equines: horse, donkey, mule, skeleton horse, zombie horse

### 1.1 Models, textures, variants

| Kind | Adult / baby layer | Adult / baby texture | Layers |
|---|---|---|---|
| horse | `minecraft:horse#main` / `minecraft:horse_baby#main` | `entity/horse/horse_<colour>[_baby]` | markings, armour (`minecraft:horse_armor#main`), saddle (`minecraft:horse#saddle`) |
| donkey | `minecraft:donkey#main` / `minecraft:donkey_baby#main` | `entity/horse/donkey[_baby]` | saddle `minecraft:donkey#saddle`, layer `donkey_saddle` |
| mule | `minecraft:mule#main` / `minecraft:mule_baby#main` | `entity/horse/mule[_baby]` | saddle `minecraft:mule#saddle`, layer `mule_saddle` |
| skeleton_horse | `minecraft:skeleton_horse#main` / `minecraft:skeleton_horse_baby#main` | `entity/horse/horse_skeleton[_baby]` | armour `minecraft:undead_horse_armor#main`, saddle `minecraft:skeleton_horse#saddle` (`skeleton_horse_saddle`) |
| zombie_horse | `minecraft:zombie_horse#main` / `minecraft:zombie_horse_baby#main` | `entity/horse/horse_zombie[_baby]` | armour `minecraft:undead_horse_armor#main`, saddle `minecraft:zombie_horse#saddle` (`zombie_horse_saddle`) |

Shadow radius 0.75 (× age_scale 0.5 for a foal).

Horse — variant (index 19): colour = v & 0xFF, marking = (v >> 8) & 0xFF. Colour out of
range: taken modulo 7; marking: identifiers 0‑4.

| Colour | Texture | | Marking | Texture (adult; baby `_baby`) |
|---|---|---|---|---|
| 0 white | `horse_white` | | 0 none | — (layer not drawn) |
| 1 creamy | `horse_creamy` | | 1 white stockings | `entity/horse/horse_markings_white` |
| 2 chestnut | `horse_chestnut` | | 2 white field | `entity/horse/horse_markings_whitefield` |
| 3 brown | `horse_brown` | | 3 white dots | `entity/horse/horse_markings_whitedots` |
| 4 black | `horse_black` | | 4 black dots | `entity/horse/horse_markings_blackdots` |
| 5 gray | `horse_gray` | | | |
| 6 dark brown | `horse_darkbrown` | | | |

### 1.2 Layers

1. **Markings** (horse only): body model redrawn with the marking texture, **translucent** type
   ("entity_translucent"), order 1, red overlay identical to the body; not drawn if the entity is
   invisible or if marking = 0.
2. **Horse armour** (`body` slot, equipment layer `horse_body`): horse → layer
   `minecraft:horse_armor#main` (body inflated by 0.1, ×1.1), order 2; skeleton and zombie → layer
   `minecraft:undead_horse_armor#main` (inflated 0.1, without ×1.1), order 0. Donkey and mule: no layer.
3. **Saddle** (`saddle` slot): layer `<kind>#saddle`, layer type `horse_saddle`,
   `donkey_saddle`, `mule_saddle`, `skeleton_horse_saddle` or `zombie_horse_saddle` (vanilla texture
   `entity/equipment/<type>/saddle`), order 2 for the horse, 0 for the others. Parts
   `left_saddle_line` and `right_saddle_line` (reins, children of `head_parts`) visible only if
   the equine carries a passenger.
4. **Chest** (donkey, mule): parts `left_chest` and `right_chest` (children of `body`) of the body model,
   visible only if index 19 is true (otherwise hidden). Foals have no visible ones.

### 1.3 Animated quantities (client side, every tick)

- Eating E, rearing S, mouth B (values "O" remembered before the update, rendering = lerp(pt, O, current)):
  - E: if flag 0x10, E += (1 − E) × 0.4 + 0.05 (cap 1); otherwise E += (0 − E) × 0.4 − 0.05
    (floor 0);
  - S: if flag 0x20, E = 0 and E_O = 0, then S += (1 − S) × 0.4 + 0.05 (cap 1); otherwise
    S += (0.8 × S³ − S) × 0.6 − 0.05 (floor 0);
  - B: if flag 0x40, B += (1 − B) × 0.7 + 0.05 (cap 1); otherwise B += (0 − B) × 0.7 − 0.05
    (floor 0).
- Tail: counter q; on every tick, with probability 1/200, q = 1; then if q > 0, q += 1 and q > 8 → q = 0.
  "Animated tail" = q > 0.
- Entity in water.

### 1.4 Animation (all the body parts, of the armour and saddle layers)

Parts: `head_parts` (carries `head`, mane, muzzle…), `body` (carries `tail`), four legs
`right_hind_leg`, `left_hind_leg`, `right_front_leg`, `left_front_leg` (children of the root on the
adult, of `body` on the donkey and mule foals: use the layer's tree).

Constants depending on the model:

| | Adults (5 kinds) | Foal (horse, skeleton, zombie) | Donkey/mule foal |
|---|---|---|---|
| Ang (rearing, hind legs) | π/12 | π/12 | π/3 |
| Ys (front legs rise) | 12 | 4 | 1 |
| Zs (front legs advance) | 4 | 0 | 0.5 |
| Xo (front legs rearing) | −π/3 | −π/3 | 0 |
| Qo (tail offset) | 0 | −π/2 | −π/4 |

Computation:
1. yR = clamp(head_yaw, −20, 20); xR = pitch × π/180; if V > 0.2: xR += cos(P × 0.8) × 0.15 × V.
2. M = 1 − max(S, E);
   `head_parts.rot_x` = S × (π/12 + xR) + E × (2.1816616 + sin(age) × 0.05) + M × (π/6 + xR + B × sin(age) × 0.05);
   `head_parts.rot_y` = (S + M) × yR × π/180.
3. Head position:
   - adults: `head_parts.y` += lerp(E, lerp(S, 0, −8), 7); `head_parts.z` = lerp(S, z_base, −4);
   - foal: `head_parts.y` += lerp(E, lerp(S, 0, −2), 2); `head_parts.z` = lerp(S, z_base, −4);
   - donkey/mule foal: `head_parts.y` = lerp(E, y_base, −1.2); `head_parts.z` = lerp(S, z_base, −3.6).
4. `body.rot_x` = S × (−π/4) + (1 − S) × xRot_base(body).
5. `left_front_leg.y` −= Ys × S; `left_front_leg.z` += Zs × S; `right_front_leg` takes the same y, z.
6. w = 0.2 in water, 1 otherwise; L = cos(w × P × 0.6662 + π); A = L × 0.8 × V; o = cos(age × 0.6 + π).
   - `left_front_leg.rot_x` = (Xo + o) × S + A × (1 − S);
   - `right_front_leg.rot_x` = (Xo − o) × S − A × (1 − S);
   - `left_hind_leg.rot_x` = Ang × S − L × 0.5 × V × (1 − S);
   - `right_hind_leg.rot_x` = Ang × S + L × 0.5 × V × (1 − S).
7. Donkey/mule foal only: `left_hind_leg.y` = lerp(S, y_base, −0.3); `right_hind_leg.y` =
   lerp(S, new left_hind_leg.y, −0.3).
8. Tail: `tail.rot_x` = Qo + π/6 + V × 0.75; `tail.y` += V × age_scale; `tail.z` += 2 × V × age_scale;
   `tail.rot_y` = cos(age × 0.7) if the tail is animated, otherwise 0.
9. Donkey/mule foal only, after everything above: the pitch is **forced to −30°** (and the render state
   keeps this value for the rest of the frame), then `head_parts.rot_x` is recomputed:
   S × (π/12 − π/6) + E × (π/2 + sin(age) × 0.05) + M × (B × sin(age) × 0.05).

The saddle and armour models run exactly the same computation (they have the same parts).

---

## 2. Camel and camel husk

### 2.1 Models and layers

| Kind | Adult / baby layer | Texture | Saddle |
|---|---|---|---|
| camel | `minecraft:camel#main` / `minecraft:camel_baby#main` | `entity/camel/camel` / `entity/camel/camel_baby` | layer `minecraft:camel#saddle`, layer `camel_saddle` |
| camel_husk | `minecraft:camel#main` (no baby: always the adult model) | `entity/camel/camel_husk` | layer `minecraft:camel_husk#saddle` (same geometry), layer `camel_husk_saddle` |

- The camel husk **has no body layer of its own**: it reuses `minecraft:camel#main`.
- Shadow radius 0.7 (baby ×0.6). Saddle: part `reins` visible only when ridden.

### 2.2 Client-side states

- pose_time = world game time − |index 20|; sitting = index 20 < 0;
  visually sitting = (pose_time < 0) XOR sitting; in transition = pose_time < (sitting ? 40 : 52);
  sitting down = sitting AND 0 ≤ pose_time < 40.
- Every (client) tick:
  - idle: timer; if ≤ 0: timer = random[0, 40) + 80 and **start** "idle"; otherwise −1;
  - if visually sitting: stop "stand up" and "dash"; if sitting down: start if stopped
    "sit", stop "sit pose"; otherwise stop "sit", start if stopped "sit pose";
  - otherwise: stop "sit" and "sit pose"; "dash" active as long as index 19 is true;
    "stand up" active as long as in transition AND pose_time ≥ 0.
- Dash cooldown: on every change of the received index 19 (except the first tick), cooldown = 55 if it
  was 0; −1 per tick. Render quantity: jc = max(cooldown − pt, 0).
- Limb swing (replaces C §1.4): target = min(distance × 6, 1) if the pose (index 6) is
  "standing" and "dash" is not started, otherwise 0; speed += (target − speed) × 0.2;
  position_scale 3 for a baby.

### 2.3 Animation

1. Head (`head`, child of `body`): y = clamp(head_yaw, −30, 30); x = clamp(pitch, −25, 45); if jc > 0:
   x = clamp(x + 45 × jc / 55, −25, 70); `head.rot_y` = y × π/180; `head.rot_x` = x × π/180.
2. Walk: "walk" table driven by the walk with f = 2, g = 2.5 (§0.4).
3. Then, in this order, per state: "sit", "sit pose", "stand up", "idle", "dash".
Tables (separate adult / baby, 6 each): to be extracted (§0.4).

---

## 3. Llamas: llama, trader llama

| Kind | Adult / baby layer | Decor layer adult / baby |
|---|---|---|
| llama | `minecraft:llama#main` / `minecraft:llama_baby#main` | `minecraft:llama#decor` (inflated 0.5) / `minecraft:llama_baby#decor` (inflated 0.2) |
| trader_llama | `minecraft:trader_llama#main` / `minecraft:trader_llama_baby#main` (same geometry as the llama) | same |

- Variant (index 21, out of range → clamped to 0‑3): 0 `entity/llama/llama_creamy`, 1 `llama_white`,
  2 `llama_brown`, 3 `llama_gray`; baby: suffix `_baby`. Same table for both kinds.
- Shadow radius 0.7.
- Chests: parts `right_chest` and `left_chest` visible only if index 19 true AND adult (applies to
  the body and to the decor model).
- **Decor** (layer `llama_body`, item of the `body` slot):
  1. if the item has an `asset_id` AND the llama is an adult: layer of this asset (carpet: `<colour>_carpet`
     → `entity/equipment/llama_body/<colour>.png`) on `minecraft:llama#decor`;
  2. otherwise, if it is a trader llama: asset `trader_llama` (adult, `minecraft:llama#decor`, texture
     `entity/equipment/llama_body/trader_llama`) or `trader_llama_baby` (baby, `minecraft:llama_baby#decor`,
     texture `.../trader_llama_baby`). An adult trader with a carpet therefore shows the carpet.
- Animation (body and decor, adult and baby): `head.rot_x` = pitch × π/180; `head.rot_y` = head_yaw × π/180;
  legs as C §2.4 (`right_hind_leg`, `left_front_leg`: cos(P × 0.6662) × 1.4 × V; `left_hind_leg`,
  `right_front_leg`: same + π).

---

## 4. Mooshroom

- Layers `minecraft:mooshroom#main` / `minecraft:mooshroom_baby#main` (adult and baby cow
  geometries); cow animation (C §2.4). Shadow radius 0.7.
- Variant (index 18, clamped): 0 red `entity/cow/mooshroom_red[_baby]`, 1 brown
  `entity/cow/mooshroom_brown[_baby]`.
- **Mushrooms** (adult only; not drawn if invisible, except "glowing" entity: outline only):
  three models of the `red_mushroom` or `brown_mushroom` **block** (default state, block model), with the
  entity's light and its red overlay. Transformations in the model frame (after C §1.2 step 7):
  1. translation (0.2, −0.35, 0.5), Y rotation −48°, scale (−1, −1, 1), translation (−0.5, −0.5, −0.5);
  2. translation (0.2, −0.35, 0.5), Y rotation 42°, translation (0.1, 0, −0.6), Y rotation −48°,
     scale (−1, −1, 1), translation (−0.5, −0.5, −0.5);
  3. transformation of the `head` part (animated pose), translation (0, −0.7, −0.2), Y rotation −78°,
     scale (−1, −1, 1), translation (−0.5, −0.5, −0.5).

---

## 5. Ocelot

- Layers `minecraft:ocelot#main` / `minecraft:ocelot_baby#main` (feline geometries); textures
  `entity/cat/ocelot` / `entity/cat/ocelot_baby`. Shadow radius 0.4. No layer.
- Animation: **identical to the already implemented cat** (same adult / baby feline formulas), with only
  two states: crouching = pose `crouching` (index 6), sprinting = bit 0x08 of index 0. Sitting, lying, collar
  and "lying on the player" offset are always false / absent.

---

## 6. Rabbit

- Layers `minecraft:rabbit#main` / `minecraft:rabbit_baby#main`. Shadow radius 0.3.
- Variant (index 18; sparse identifiers, unknown → brown):

| Id | Texture (baby: `_baby`) |
|---|---|
| 0 brown | `entity/rabbit/rabbit_brown` |
| 1 white | `entity/rabbit/rabbit_white` |
| 2 black | `entity/rabbit/rabbit_black` |
| 3 white splotched | `entity/rabbit/rabbit_white_splotched` |
| 4 gold | `entity/rabbit/rabbit_gold` |
| 5 salt | `entity/rabbit/rabbit_salt` |
| 99 killer | `entity/rabbit/rabbit_caerbannog` |

  Custom name exactly "Toast": `entity/rabbit/rabbit_toast[_baby]` whatever the variant.
- Jump (client): entity event 1 → duration = 15, counter = 0; every tick (after the state update):
  if counter ≠ duration, counter += 1; otherwise if duration ≠ 0, counter = duration = 0.
- States (every tick, before the jump): initial idle timer random[0, 40) + 180;
  if timer ≤ 0 AND not leashed AND without the "no AI" flag (index 15 bit 0x01): timer =
  random[0, 40) + 180, **start** "head tilt"; otherwise if jump counter > 0: start if
  stopped "jump", stop "tilt"; otherwise timer −1, stop "jump". Leashing →
  stop "tilt".
- Animation: if "tilt" is not started: `head.rot_y` = head_yaw × π/180, `head.rot_x` =
  pitch × π/180 (`head` child of `body`). Then tables "jump" and "tilt" per state (separate adult / baby,
  to be extracted, §0.4).

---

## 7. Parrot

- Layer `minecraft:parrot#main` (no baby). Shadow radius 0.3.
- Variant (index 20, clamped 0‑4): 0 `entity/parrot/parrot_red_blue`, 1 `parrot_blue`, 2 `parrot_green`,
  3 `parrot_yellow_blue`, 4 `parrot_grey`.
- Flapping (client, every tick): flapO = flap; speedO = speed; speed += (airborne and not a passenger ? 4 : −1)
  × 0.3, bounded [0, 1]; if airborne and momentum < 1: momentum = 1; momentum ×= 0.9; flap += momentum × 2.
  F = (sin(lerp(pt, flapO, flap)) + 1) × lerp(pt, speedO, speed). "Airborne" = on-ground flag false.
- Pose (priority): **party** (a jukebox is playing within ≈ 3.46 blocks, see uncertain points) > **sitting**
  (index 18 bit 0x01) > **flying** (not on the ground) > **standing**. On a player's shoulder: pose "shoulder".
- Preparation: flying: `left_leg.rot_x`, `right_leg.rot_x` += 2π/9; sitting: `head`, `tail`, `body`,
  `left_wing`, `right_wing`, `left_leg`, `right_leg`: y += 1.9; `tail.rot_x` += π/6;
  `left_wing.rot_z` = −0.0873; `right_wing.rot_z` = 0.0873; legs rot_x += π/2; party: `left_leg.rot_z` =
  −π/9, `right_leg.rot_z` = π/9.
- Then `head.rot_x` = pitch × π/180, `head.rot_y` = head_yaw × π/180, and depending on the pose:
  - standing: `left_leg.rot_x` += cos(P × 0.6662) × 1.4 × V; `right_leg.rot_x` += cos(P × 0.6662 + π) × 1.4 × V;
    then the "flying" block below;
  - flying and shoulder: b = F × 0.3; `head.y`, `tail.y`, `body.y`, `left_wing.y`, `right_wing.y`,
    `left_leg.y`, `right_leg.y` += b; `tail.rot_x` += cos(P × 0.6662) × 0.3 × V;
    `left_wing.rot_z` = −0.0873 − F; `right_wing.rot_z` = 0.0873 + F;
  - sitting: nothing more;
  - party: cx = cos(age), sy = sin(age); `head`: x += cx, y += sy, rot_x = rot_y = 0, rot_z = sin(age) × 0.4;
    `body`, `left_wing`, `right_wing`, `tail`: x += cx, y += sy; wings rot_z as when flying.
- **On the shoulder** (player layer; variants = indices 19/20 of the player, C part 1 § 3.4): in the player
  model's frame, translation (+0.4 on the left / −0.4 on the right; −1.3 if crouching otherwise −1.5; 0), parrot
  model in pose "shoulder", F = 0, P, V, age, head_yaw and pitch those of the player, without red overlay.

---

## 8. Polar bear

- Layers `minecraft:polar_bear#main` / `minecraft:polar_bear_baby#main`; textures
  `entity/bear/polarbear` / `entity/bear/polarbear_baby`. Shadow radius 0.9.
- Standing (client, every tick): d = clamp(d ± 1, 0, 6) according to index 18; s = lerp(pt, d_O, d) / 6;
  s2 = s². (The collision box also grows by ×(1 + d/6) in height.)
- Animation: quadruped C §2.4, then (k = age_scale):
  `body.rot_x` −= s2 × π × 0.35; `body.y` += s2 × k × 2;
  `right_front_leg`: y −= s2 × k × 20, z += s2 × k × 4, rot_x −= s2 × π × 0.45;
  `left_front_leg`: y, z = those of the right leg, rot_x −= s2 × π × 0.45;
  `head`: y −= s2 × 24, z += s2 × 13, rot_x += s2 × π × 0.15.

---

## 9. Axolotl

- Layers `minecraft:axolotl#main` / `minecraft:axolotl_baby#main`. Shadow radius 0.5.
- Variant (index 18; out of range → 0): 0 lucy, 1 wild, 2 gold, 3 cyan, 4 blue; texture
  `entity/axolotl/axolotl_<name>` (baby `axolotl_<name>_baby`).
- "Moving" = non-zero swing speed OR yaw or pitch changed since the previous tick.

### 9.1 Adult (formulas)

Smoothed factors: four counters 0..10, +1 per tick if the condition is true, −1 otherwise;
factor = ease((lerp(pt, old, current)) / 10) with ease(x) = (1 − cos(πx)) / 2.
Exclusive state: playing dead (index 19) > in water > on ground > in the air; counters "dead", "water",
"ground" follow the state; counter "movement" follows "moving".

With D, W, G, M these factors, N = 1 − M and R = 1 − min(G, M):
1. `body.rot_y` += head_yaw × π/180.
2. Swimming (k = min(M, W), ignored if k ≤ 1e‑5): a = age × 0.33, s = sin a, c = cos a, b = 0.13 s;
   `body.rot_x` += (pitch × π/180 + b) k; `head.rot_x` −= 1.8 b k; `body.y` −= 0.45 c k;
   `top_gills.rot_x` += (−0.5 s − 0.8) k; g = (0.3 s + 0.9) k: `left_gills.rot_y` += g, `right_gills.rot_y` −= g;
   `tail.rot_y` += 0.3 cos(0.9 a) k; `left_hind_leg`: rot_x += 1.8849558 k, rot_y += −0.4 s k, rot_z += π/2 k;
   `left_front_leg`: rot_x += 1.8849558 k, rot_y += (−0.2 c − 0.1) k, rot_z += π/2 k.
3. Still floating (k = min(N, W)): a = age × 0.075, c = cos a, s = 0.15 sin a; bx = (−0.15 + 0.075 c) k;
   `body.rot_x` += bx; `body.y` −= s k; `head.rot_x` −= bx; `top_gills.rot_x` += 0.2 c k;
   g = (−0.3 c − 0.19) k: `left_gills.rot_y` += g, `right_gills.rot_y` −= g;
   `left_hind_leg`: rot_x += (3π/4 − 0.11 c) k, rot_y += 0.47123894 k, rot_z += 1.7278761 k;
   `left_front_leg`: rot_x += (π/4 − 0.2 c) k, rot_y += 2.042035 k; `tail.rot_y` += 0.5 c k.
4. Crawling on the ground (k = min(M, G)): a = age × 0.11, c = cos a; h = (c² − 2c)/5; f = 0.7 c;
   `head.rot_y`, `tail.rot_y` += 0.09 c k; g = (0.6 − 0.08 (c² + 2 sin a)) k: `top_gills.rot_x` += g,
   `left_gills.rot_y` −= g, `right_gills.rot_y` += g;
   `left_hind_leg`: rot_x += 0.9424779 k, rot_y += (1.5 − h) k, rot_z += −0.1 k;
   `left_front_leg`: rot_x += 1.0995574 k, rot_y += (π/2 − f) k;
   `right_hind_leg`: rot_x += 0.9424779 k, rot_y += (−1 − h) k;
   `right_front_leg`: rot_x += 1.0995574 k, rot_y += (−π/2 − f) k.
5. Still on the ground (k = min(N, G)): a = age × 0.09, s = sin a, c = cos a; m = s² − 2s; m2 = c² − 3s;
   `head.rot_x` += −0.09 m k; `head.rot_z` += −0.2 k; `tail.rot_y` += (−0.1 + 0.1 m) k;
   g = (0.6 + 0.05 m2) k: `top_gills.rot_x` += g, `left_gills.rot_y` −= g, `right_gills.rot_y` += g;
   `left_hind_leg`: rot_x += 1.1 k, rot_y += 1.0 k; `left_front_leg`: rot_x += 0.8 k, rot_y += 2.3 k, rot_z −= 0.5 k.
6. Playing dead (k = D): `left_hind_leg`: rot_x += 1.4137167 k, rot_y += 1.0995574 k, rot_z += π/4 k;
   `left_front_leg`: rot_x += π/4 k, rot_y += 2.042035 k; `body.rot_x` += −0.15 k; `body.rot_z` += 0.35 k.
7. Mirror (k = R): right hind leg += (rot_x, −rot_y, −rot_z) of the left hind leg × k; same for the front.

### 9.2 Baby (keyframes)

Seven exclusive states, chosen every tick (the chosen one is "start if stopped", the others stopped):
playing dead → "dead"; otherwise moving: in water and not on the ground → "swim"; out of water and on the ground
→ "walk"; otherwise → "underwater walk"; still: in water and not on the ground → "underwater idle";
in water and on the ground → "bottom idle"; otherwise → "ground idle".
Rendering: if "walk" is started, table "ground walk" driven by the walk (f = 15, g = 30); then per
state: "swim", "underwater walk" (driven by the **walk** state, see uncertain points), "ground idle",
"underwater idle", "bottom idle", "dead". Tables to be extracted (§0.4).

---

## 10. Fish

### 10.1 Cod

- Layer `minecraft:cod#main`, texture `entity/fish/cod`. Shadow radius 0.3.
- Rotations (after C §1.3): Y rotation of 4.3 × sin(0.6 × age) degrees; out of water: translation
  (0.1, 0.1, −0.1) then Z rotation 90°.
- Animation: `tail_fin.rot_y` = −a × 0.45 × sin(0.6 × age), a = 1 in water, 1.5 out of water.

### 10.2 Salmon

- Size (index 17, clamped): 0 small → `minecraft:salmon_small#main`; 1 medium → `minecraft:salmon#main`;
  2 large → `minecraft:salmon_large#main`. Single texture `entity/fish/salmon`. Shadow radius 0.4.
- Rotations: (a, b) = (1, 1) in water, (1.3, 1.7) out of water; Y rotation of a × 4.3 × sin(b × 0.6 × age)
  degrees; out of water: translation (0.2, 0.1, 0) then Z rotation 90°.
- Animation: `body_back.rot_y` = −a × 0.25 × sin(b × 0.6 × age).

### 10.3 Pufferfish

- Puff state (index 17): 0 → `minecraft:pufferfish_small#main`; 1 → `minecraft:pufferfish_medium#main`;
  2 and more → `minecraft:pufferfish_big#main`. Single texture `entity/fish/pufferfish`.
- Shadow radius 0.1 + 0.1 × state.
- Before C §1.3: translation (0, cos(age × 0.05) × 0.08, 0) (bobbing).
- Animation: small: `right_fin.rot_z` = −0.2 + 0.4 sin(age × 0.2), `left_fin.rot_z` = 0.2 − 0.4 sin(age × 0.2);
  medium and big: same formulas on `right_blue_fin` / `left_blue_fin`.

### 10.4 Tropical fish

- Packed variant (index 17): pattern = v & 0xFFFF; base colour = (v >> 16) & 0xFF; pattern
  colour = (v >> 24) & 0xFF (dye colours 0‑15, out of range → 0 white).
- Pattern (identifier = shape | index << 8; unknown → kob):

| Shape 0 "small" (`tropical_fish_small`) | Id | Pattern texture | | Shape 1 "large" (`tropical_fish_large`) | Id | Pattern texture |
|---|---|---|---|---|---|---|
| kob | 0 | `entity/fish/tropical_a_pattern_1` | | flopper | 1 | `entity/fish/tropical_b_pattern_1` |
| sunstreak | 256 | `tropical_a_pattern_2` | | stripey | 257 | `tropical_b_pattern_2` |
| snooper | 512 | `tropical_a_pattern_3` | | glitter | 513 | `tropical_b_pattern_3` |
| dasher | 768 | `tropical_a_pattern_4` | | blockfish | 769 | `tropical_b_pattern_4` |
| brinely | 1024 | `tropical_a_pattern_5` | | betty | 1025 | `tropical_b_pattern_5` |
| spotty | 1280 | `tropical_a_pattern_6` | | clayfish | 1281 | `tropical_b_pattern_6` |

- Body: small shape → `minecraft:tropical_fish_small#main` + `entity/fish/tropical_a`; large →
  `minecraft:tropical_fish_large#main` + `entity/fish/tropical_b`; **multiplicative tint** = base
  colour (opaque ARGB, table below).
- Pattern layer: layer `minecraft:tropical_fish_small#pattern` or
  `minecraft:tropical_fish_large#pattern`, inflated by 0.008; cutout, tint = pattern colour, order 1;
  not drawn if invisible.
- Colours (texture tint, RGB): 0 white F9FFFE · 1 orange F9801D · 2 magenta C74EBD · 3 light blue
  3AB3DA · 4 yellow FED83D · 5 lime 80C71F · 6 pink F38BAA · 7 gray 474F52 · 8 light gray 9D9D97 ·
  9 cyan 169C9C · 10 purple 8932B8 · 11 blue 3C44AA · 12 brown 835432 · 13 green 5E7C16 · 14 red B02E26 ·
  15 black 1D1D21.
- Shadow radius 0.15. Rotations like the cod but out-of-water translation (0.2, 0.1, 0) then Z 90°.
- Animation: `tail.rot_y` = −a × 0.45 × sin(0.6 × age) (a = 1 in water, 1.5 out), same values for the
  pattern layer.

---

## 11. Squid and glow squid

- squid: `minecraft:squid#main` / `minecraft:squid_baby#main`, `entity/squid/squid[_baby]`;
  glow_squid: `minecraft:glow_squid#main` / `minecraft:glow_squid_baby#main`, `entity/squid/glow_squid[_baby]`.
  Shadow radius 0.7.
- **Own rotations** (replace C §1.3, including death and upside-down head): translation (0, 0.5 adult,
  0.25 baby, 0); Y rotation (180 − rendered body yaw); X rotation of xBody degrees; Y rotation of zBody degrees;
  translation (0, −1.2 adult, −0.6 baby, 0).
- Animation: each tentacle (`tentacle0` to `tentacle7`): rot_x = tentacle angle.
- Quantities (client, every tick; values "O" remembered, rendering = lerp pt):
  - movement += speed (initial speed = 0.2 / (random[0, 1) + 1)); if movement > 2π: movement = 2π
    (the server resets it to 0 through entity event 19);
  - in water: if movement < π: u = movement/π, angle = sin(u² π) × π × 0.25, and rotSpeed = 1 if u > 0.75,
    otherwise rotSpeed ×= 0.8; otherwise angle = 0, rotSpeed ×= 0.99. Then, with m = entity velocity:
    bodyYaw += (−atan2(m.x, m.z) in degrees − bodyYaw) × 0.1; zBody += π × rotSpeed × 1.5 (degrees);
    xBody += (−atan2(horizontal length of m, m.y) in degrees − xBody) × 0.1;
  - out of water: angle = |sin(movement)| × π × 0.25; xBody += (−90 − xBody) × 0.02.
- Glow squid: **block light** level used at render time = n = truncate(lerpClamped(1 − darkness/10, 0, 15))
  with darkness = index 18; if n = 15 → 15, otherwise max(n, actual block light). No emissive layer.

---

## 12. Guardian and elder guardian

### 12.1 Model, textures

- guardian: `minecraft:guardian#main`, `entity/guardian/guardian`, shadow 0.5, attack duration 80 ticks;
  elder_guardian: `minecraft:elder_guardian#main` (same model ×2.35 baked), `entity/guardian/guardian_elder`,
  shadow 1.2, attack duration 60.
- Parts: `head` (body root), 12 spikes `spike0`‑`spike11`, `eye`, tail `tail0` → `tail1` → `tail2`.

### 12.2 Quantities (client, every tick, alive)

- Tail: out of water: tailSpeed = 2; in water and moving (index 16): if tailSpeed < 0.5 → 4, otherwise
  tailSpeed += (0.5 − tailSpeed) × 0.1; still: tailSpeed += (0.125 − tailSpeed) × 0.2. tail += tailSpeed. Rendering: lerp pt.
- Spikes: out of water: sp = random[0, 1); moving: sp += (0 − sp) × 0.25; still:
  sp += (1 − sp) × 0.06. Rendering: lerp pt.
- Attack: on every received change of index 17, time = 0; if target ≠ 0 and time < duration: time += 1.
  Attack scale = (time + pt) / duration.

### 12.3 Animation

- `head.rot_y` = head_yaw × π/180, `head.rot_x` = pitch × π/180.
- Spikes: r = (1 − sp) × 0.55; for i = 0..11: o_i = 1 + cos(age × 1.5 + i) × 0.01 − r;
  `spike_i`: x = SX[i] × o_i, y = 16 + SY[i] × o_i, z = SZ[i] × o_i, with
  SX = (0, 0, 8, −8, −8, 8, 8, −8, 0, 0, 8, −8), SY = (−8, −8, −8, −8, 0, 0, 0, 0, 8, 8, 8, 8),
  SZ = (8, −8, 0, 0, −8, −8, 8, 8, 8, −8, 0, 0) (spike rotations: those of the layer).
- Eye: gazed target = the beam target if it exists, otherwise the camera entity. If it exists:
  `eye.y` = 0 if (y of the target's eyes − y of the guardian's eyes) > 0, otherwise 1; v = guardian's gaze
  vector flattened (y = 0); d = normalise(guardian_eye − target_eye, flattened) rotated by +π/2 around Y;
  p = v·d; `eye.x` = √|p| × 2 × sign(p). `eye` always visible.
- Tail: `tail0.rot_y` = sin(q) × π × 0.05; `tail1.rot_y` = sin(q) × π × 0.1; `tail2.rot_y` = sin(q) × π × 0.15.

### 12.4 Beam (if target ≠ 0 and resolved)

- Origin: guardian's eyes (translation (0, eye height, 0) from the interpolated position);
  end: interpolated position of the target + half its height. Vector b = end − eyes;
  length L = |b| + 1; b̂ normalised.
- Frame: Y rotation of (π/2 − atan2(b̂.z, b̂.x)), then X rotation of acos(b̂.y); the beam follows +Y.
- t = time + pt; rot = −0.075 × t; s2 = scale²; colour (R, G, B) = (64 + truncate(191 s2),
  32 + truncate(191 s2), 128 − truncate(64 s2)), alpha 255, full light (15/15), normal (0, 1, 0), no
  overlay. Texture `entity/guardian/guardian_beam`, cutout.
- Two crossed planes of radius 0.2: points at angle rot + π (W), rot (E), rot + π/2 (N), rot + 3π/2 (S);
  vOff = (t × 0.5) mod 1; v0 = −1 + vOff; v1 = v0 + 2.5 L; quads (W top, W bottom, E bottom, E top) and
  (N top, N bottom, S bottom, S top), u = 0.4999 for W/N, 0 for E/S, v = v1 at the top (y = L), v0 at the bottom (y = 0).
- Cap at the top (y = L), radius 0.282, angles rot + 3π/4 (wn), rot + π/4 (en), rot + 5π/4 (ws),
  rot + 7π/4 (es): quad wn (u 0.5), en (u 1), es (u 1), ws (u 0.5), v = vb + 0.5 for wn/en, vb for es/ws,
  vb = 0.5 if floor(t) is even, otherwise 0.
- Not subject to view-frustum culling if the box enclosing the two ends is visible.

---

## 13. Cube mobs: slime, magma cube, sulfur cube

### 13.1 Common

- Size n = index 18 (1 to 127). Shadow radius n × 0.25 (× age_scale).
- Squish (client, every tick, before the rest): squishO = squish; squish += (target − squish) × 0.5; then on
  landing (on ground and not on the previous tick) target = −0.5; on takeoff target = 1; finally
  target ×= 0.6 (magma: × 0.9). Rendering e = lerp(pt, squishO, squish).
- Own scale (C §1.2 step 6): w = 1 / (e / (n × 0.5 + 1) + 1); scale (w n, n / w, w n).
- Slime and sulfur cube only, before this scale: scale 0.999 then translation (0, 0.001, 0).

### 13.2 Slime

- Body: `minecraft:slime#main` (inner cube, eyes, mouth), texture `entity/slime/slime`, cutout.
- Outer layer: `minecraft:slime#outer`, same texture, **translucent**, order 1; if invisible:
  nothing, except glowing entity → outline only. No part animation.

### 13.3 Magma cube

- `minecraft:magma_cube#main`, texture `entity/slime/magmacube`. Block light always 15 (full).
- Animation: 8 slices `cube0`‑`cube7`: y = −(4 − i) × max(0, e) × 1.7.

### 13.4 Sulfur cube

- Adult: `minecraft:sulfur_cube#main` + texture `entity/sulfur_cube/sulfur_cube_outer`; baby (index 16):
  `minecraft:sulfur_cube_small#main` + `sulfur_cube_outer_small`. The body is rendered **translucent**.
- Own scale, in order: 0.999 / translation 0.001; size and squish (§13.1, but squish
  zero if a block is contained); if fuse m satisfies 0 < m < 10: scale 1 + g with
  g = (clamp(1 − m/10, 0, 1))⁴ × 0.3; then scale k (0.5 adult, 1 baby); translation
  (0, h − (invisible ? 0 : 1/16), 0) with h = 0.98 adult, 1.24 baby.
- Fuse (client): m_tick = index 19 at each reception, then −1 per tick while > 0; primed if ≥ 0.
  Render value m = m_tick − pt + 1 if primed, otherwise 0.
- Inner layer (unless a head is worn):
  - overlay: if m > 0 and truncate(m / 5) is even → solid white (u 15, v 10 of the overlay texture), otherwise
    normal overlay;
  - if a **block** is contained (item of the `body` slot, non-skull block; block state taken from the
    `block_state` component applied to the default state): X rotation 180°, scale 0.5 if baby,
    translation (−0.5, −0.518, −0.5), block model with the entity's light;
  - otherwise, if visible: `minecraft:sulfur_cube#inner` + `entity/sulfur_cube/sulfur_cube_inner` (baby:
    `minecraft:sulfur_cube_small#inner` + `sulfur_cube_inner_small`), translucent, **order −1**.
- Worn head (`body` item = skull or head): "custom head" layer on the `cube` part, offset
  (0, 0.625, 0), scale 0.84210527 (see uncertain points).
- No part animation.

---

## 14. Ghast and happy ghast

### 14.1 Ghast

- `minecraft:ghast#main`; texture `entity/ghast/ghast`, or `entity/ghast/ghast_shooting` if index 16 is
  true. Shadow radius 1.5.
- Animation: nine tentacles `tentacle0`‑`tentacle8`: rot_x = 0.2 × sin(age × 0.3 + i) + 0.4.

### 14.2 Happy ghast

- `minecraft:happy_ghast#main` / `minecraft:happy_ghast_baby#main`; textures `entity/ghast/happy_ghast` /
  `happy_ghast_baby`. Shadow radius 2 (baby × 0.2375).
- Animation: tentacles like the ghast (children of `body`); if the `body` slot carries an item:
  `body` scale (0.9375, 0.9375, 0.9375).
- Harness (`body` slot, layer `happy_ghast_body`, e.g. `white_harness` →
  `entity/equipment/happy_ghast_body/white_harness`): `minecraft:happy_ghast_harness#main` (adult) or
  `minecraft:happy_ghast_baby_harness#main` (**babies included**). Goggles (`goggles`): ridden → rot_x = 0,
  y = 14; otherwise rot_x = −0.7854, y = 9.
- Ropes: if index 18 is true AND the `body` item is a harness (item tag
  `minecraft:harnesses`): `minecraft:happy_ghast_ropes#main` (baby `minecraft:happy_ghast_baby_ropes#main`,
  body inflated by 0.2), texture `entity/ghast/happy_ghast_ropes`, cutout, no overlay.
- View-frustum culling: entity box extended downwards by half its height.

---

## 15. Ender Dragon

### 15.1 Rendering (own renderer, not the living-entity one)

- Layer `minecraft:ender_dragon#main`; textures `entity/enderdragon/dragon`, eyes
  `entity/enderdragon/dragon_eyes`, dissolve `entity/enderdragon/dragon_exploding`. Shadow radius 0.5.
  Never culled by the view frustum.
- Stack (from the interpolated position minus the camera): Y rotation of −yaw(7) degrees; X rotation of
  (y(5) − y(10)) × 10 degrees; translation (0, 0, 1); scale (−1, −1, 1); translation (0, −1.501, 0).
  (yaw(k), y(k): samples of the flight history, §15.2.)
- Alive: model with the dragon texture (cutout), red overlay if hurt_time > 0 (no death tint).
- Dying (death > 0, death = counter + pt, 0..200): "cutout with dissolve" type (no face sorting,
  alpha threshold 0.1): white colour of alpha a = 1 − death/200; fragment discarded if a < alpha of the
  `dragon_exploding` mask at the same UV; otherwise alpha forced to 1. No overlay.
- Always: "eyes" layer (eyes type: no light, translucent blending, depth test without
  write, like spider eyes).
- Dying: rays — translation (0, −1, −2) then colour-only triangle geometry, additive blending
  (source alpha, one):
  - τ = death/200; overheat = min(τ > 0.8 ? (τ − 0.8)/0.2 : 0, 1); centre colour = white of alpha
    1 − overheat; outer colour 0x00FF00FF (magenta of alpha 0);
  - pseudo-random generator with fixed seed 432 (reproducible sequence: the exact sequence of the game's
    generator is needed for an identical rendering, see uncertain points); count = floor((τ + τ²)/2 × 60);
  - for each ray (cumulative rotation, not reset): XYZ rotation (3 draws × 2π) then XYZ rotation
    (2 draws × 2π, 3rd draw × 2π + τ × π/2); length = draw × 20 + 5 + 10 × overheat; width =
    draw × 2 + 1 + 2 × overheat; vertices left (−√3/2 l, L, −l/2), right (√3/2 l, L, −l/2),
    bottom (0, L, l); three triangles (centre, left, right), (centre, right, bottom), (centre, bottom, left).
- Crystal beam: if an End crystal is the nearest (searched on the client 1 tick in 10 among the
  crystals within 32 blocks of the box), Δ = crystal position + (0, hc, 0) − dragon position,
  hc = ((sin(tc × 0.2)/2 + 0.5)² + sin(tc × 0.2)/2 + 0.5) × 0.4 − 1.4 with tc = crystal age + pt;
  outside the model's stack: translation (0, 2, 0); Y rotation of −atan2(Δz, Δx) − π/2; X rotation of
  −atan2(√(Δx² + Δz²), Δy) − π/2; texture `entity/end_crystal/end_crystal_beam` (cutout, threshold 0.1, no face
  sorting or overlay); L = |Δ|; v0 = −age × 0.01; v1 = L/32 − age × 0.01; 8 quads: for i = 1..8,
  angle i × 2π/8, (s, c) = 0.75 × (sin, cos), previous (s', c') (start (0, 0.75)), u = i/8:
  (0.2 s', 0.2 c', 0) black v0; (s', c', L) white v1; (s, c, L) white v1; (0.2 s, 0.2 c, 0) black v0;
  entity's light, normal (0, −1, 0).

### 15.2 Flight history and flapping (client, every tick)

- Circular history of 64 samples (y, yaw): at the first recording, the whole array is
  filled; sample(k) = the one recorded k ticks earlier; interpolated sample(k) =
  (lerp(pt′, y(k+1), y(k)), lerpAngle(pt′, yaw(k+1), yaw(k))) with pt′ = pt, or 0 if the dragon is dead
  or dying.
- Flapping (if alive): v = entity velocity; f = 0.2 / (horizontal length of v × 10 + 1) ×
  2^(v.y); flap += 0.1 if phase "perched", f × 0.5 if the dragon is inside a wall, f otherwise; yaw brought
  into [−180, 180); if "no AI": flap = 0.5, otherwise record (y, yaw). Rendering: lerp(pt, flapO, flap).
- Phase (index 16): 0 holding pattern, 1 strafing, 2 landing approach, 3 landing, 4 takeoff,
  5 perched (breath), 6 perched (searching), 7 perched (attack), 8 charge, 9 dying, 10 hover.
  "Perched" = 5, 6, 7 and 10; "landing or takeoff" = 3 or 4.
- Distance to the egg: square of the distance from the dragon to the centre of the block on top of the highest column
  (heightmap "motion-blocking without leaves") above the podium (0, 0 of the fight area).

### 15.3 Animation

Notations: φ = flap × 2π; S(k) = interpolated sample k; vertical offset H(i, base, point) =
i / max(egg_dist/4, 1) if landing/takeoff; i if perched; 0 if i = 6; otherwise y(point) − y(base).
1. `jaw.rot_x` = (sin φ + 1) × 0.2; bounce β = sin(φ − 1) + 1; β = (β² + 2β) × 0.05;
   model root: y = (β − 2) × 16, z = −48, rot_x = 2β × π/180.
2. Neck (`neck0`‑`neck4`, children of the root): start (x, y, z) = base position of `neck0`;
   s = S(6); r2 = wrap(yaw S(5) − yaw S(10)); r = wrap(yaw S(5) + r2/2); for i = 0..4, p = S(5 − i):
   rot_y = wrap(yaw p − yaw s) × π/180 × 1.5;
   rot_x = cos(0.45 i + φ) × 0.15 + H(i, s, p) × π/180 × 7.5;
   rot_z = −wrap(yaw p − r) × π/180 × 1.5; position = current (x, y, z); then
   x −= sin(rot_y) cos(rot_x) × 10; y += sin(rot_x) × 10; z −= cos(rot_y) cos(rot_x) × 10.
3. `head`: position = final (x, y, z); c = S(0); rot_y = wrap(yaw c − yaw s) × π/180;
   rot_x = wrap(H(6, s, c)) × π/180 × 7.5; rot_z = −wrap(yaw c − r) × π/180.
4. `body.rot_z` = −r2 × 1.5 × π/180.
5. Wings: `left_wing` rot_x = 0.125 − cos φ × 0.2, rot_y = −0.25, rot_z = −(sin φ + 0.125) × 0.8;
   `left_wing_tip.rot_z` = (sin(φ + 2) + 0.5) × 0.75; right: rot_x identical, rot_y and rot_z opposite,
   `right_wing_tip.rot_z` = −left.
6. Legs (both sides, `<side>_hind_leg`, `_hind_leg_tip`, `_hind_foot`, `_front_leg`,
   `_front_leg_tip`, `_front_foot`): hind rot_x = 1 + 0.1β, tip 0.5 + 0.1β, foot 0.75 + 0.1β;
   front rot_x = 1.3 + 0.1β, tip −0.5 − 0.1β, foot 0.75 + 0.1β.
7. Tail (`tail0`‑`tail11`): start = base position of `tail0`; s = S(11); cumulative t = 0; for
   i = 0..11, p = S(12 + i): t += sin(0.45 i + φ) × 0.05;
   rot_y = (wrap(yaw p − yaw s) × 1.5 + 180) × π/180; rot_x = t + (y p − y s) × π/180 × 7.5;
   rot_z = wrap(yaw p − r) × π/180 × 1.5; current position; then y += sin(rot_x) × 10;
   z −= cos(rot_y) cos(rot_x) × 10; x −= sin(rot_y) cos(rot_x) × 10.

---

## 16. Nautilus and zombie nautilus

- nautilus: `minecraft:nautilus#main` / `minecraft:nautilus_baby#main`; textures
  `entity/nautilus/nautilus` / `nautilus_baby`. Shadow radius 0.7.
- zombie_nautilus (no baby): variant (index 21) in the dynamic registry
  `minecraft:zombie_nautilus_variant` (vanilla entries: `temperate` → `asset_id`
  `minecraft:entity/nautilus/zombie_nautilus`, model `normal` → `minecraft:zombie_nautilus#main`;
  `warm` → `minecraft:entity/nautilus/zombie_nautilus_coral`, model `warm` →
  `minecraft:zombie_nautilus_coral#main`). Variant absent: nothing is drawn.
- Layers (adult only): armour (`body` slot, layer `nautilus_body`, e.g. `diamond` →
  `entity/equipment/nautilus_body/diamond`) on `minecraft:nautilus_armor#main`; saddle (`saddle`, layer
  `nautilus_saddle`) on `minecraft:nautilus#saddle`.
- Animation (body and layers): `body.rot_y` = clamp(head_yaw, −10, 10) × π/180; `body.rot_x` =
  clamp(pitch, −10, 10) × π/180 (`body` child of `root`); then "swim" table driven by the walk with
  P′ = P + age/5, V′ = V + 0.2, f = 2, g = 3 (permanent swimming even when still; table to be extracted, §0.4).
- Coral variant: part `corals` (child of `shell`) visible only if the `body` slot is empty.

---

## Uncertain points

1. **Keyframe tables** not extracted: camel (adult and baby, 6 each), rabbit (adult and baby, 2
   each), baby axolotl (7), nautilus (1). To be added to the extraction (§0.4); without them, camel, rabbit,
   baby axolotl and nautilus stay frozen in the base pose (animated head only).
2. Baby axolotl: the "underwater walk" table is driven by the **walk** state (and not by the
   "underwater walk" state, which is never read at render time); this is the client's behaviour, to be reproduced as
   is.
3. Donkey/mule foal: forcing the pitch to −30° modifies the shared render state; no known visible effect
   (no equipment layer on babies), but the order matters if a layer reads the pitch afterwards.
4. Velocity of remote entities on the client side (squid, dragon, fish, "airborne" guardian): it comes
   from the velocity packets and the position corrections; the squid's angles and the dragon's flapping
   depend on it. Possible differences if the velocity is not kept up to date as in the client.
5. Parrot in party mode: triggered when a jukebox plays nearby (world notification to nearby living
   entities) and stopped if the jukebox is more than 3.46 blocks from the centre or is no longer a jukebox; the exact
   path of the notification (level event or block state) was not verified.
6. Dragon death rays: the exact distribution depends on the sequence of the game's pseudo-random
   generator (seed 432); another sequence gives an equivalent but not identical look.
7. Sulfur cube: the "custom head" layer (skulls and player heads) is not specified here
   (offset and scale only); the rendering of contained blocks assumes the existing block model
   engine.
8. Camel: the client locally resets the dash flag to false when the cooldown drops below 50 on the ground;
   effect only on the "dash" state.
9. Equipment rendering (order of tinted layers, trims): assumed to conform to C §3.5, not re-read for
   the types `horse_body`, `llama_body`, `nautilus_body`, `happy_ghast_body`.
