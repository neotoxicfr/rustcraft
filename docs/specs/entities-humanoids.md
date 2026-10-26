# Per-entity rendering — humanoids and illagers (26.3)

Clean-room specification, written from a reading of the decompiled 26.3 client (protocol 777). No line
of source code; formulas, constants, resource paths and model layer names only. Complements
`entities.md`, part "mission C" ("§ x.y" cross-references = sections of this part). Part angles are in
radians, entity angles in degrees. Notation: P = walk position, V = walk speed,
A = age in ticks + pt, yR / xR = head yaw / pitch relative, in radians, s = progress of the
attack swing (§ 1.10).

Conventions specific to this document:
- "model layer" = name as it appears in `entity_models.json`; all "baked" scales
  (giant ×6, husk ×1.0625, wither skeleton ×1.2, illagers ×0.9375, small armor stand) are ALREADY
  in the pose of the root or of the parts of the model layer: add nothing at render time.
- Texture = `assets/minecraft/textures/<path>.png`; all the textures cited have been checked.
- "Cutout" = render type `entity_cutout` (§ 1.7); "translucent" = `entity_translucent`
  (alpha blending, no per-entity sorting).
- "Copied layer": overlay model animated with the SAME state as the body (§ 1.2 point 8), rendered as
  cutout, white colour, red hurt overlay applied, and **not drawn if the entity is
  invisible** (flag 0x20).
- Metadata indices: Entity 0‑7, the "living entity" layer 8‑14, Mob 15 (byte: 0x01 no AI, 0x02 left-handed,
  0x04 aggressive), the "growing creature" layer 16‑17. The intermediate classes "monster", "path-finding mob",
  "patroller", "abstract skeleton", "abstract illager" add no data.
- Common layers of "mob" humanoids (unless stated otherwise): armor (§ 3.5) with the model layers
  `<kind>#helmet|chestplate|leggings|boots` (baby: `<kind>_baby#…`), worn head (skull or block
  in the head slot), wings (elytra), held item (§ 3.4).

---

## 1. Derived zombies

Common base: zombie renderer (§ 2.2) — shadow 0.5, humanoid animation (§ 2.1) followed by the "zombie
arms" (§ 2.2), shaking during a conversion. Zombie metadata: 16 baby (boolean),
17 special type (integer, unused at render time), 18 conversion to drowned (boolean → shaking § 1.3).

### 1.1 Husk (`husk`)
1. Model layers: adult `minecraft:husk#main` (zombie geometry ×1.0625 baked), baby
   `minecraft:husk_baby#main` (same geometry as `zombie_baby`). Textures: `entity/zombie/husk`
   (64×64), baby `entity/zombie/husk_baby` (64×64). No variant.
2. Layers: armor `husk#…` / `husk_baby#…` (adult armor is also ×1.0625); common ones.
3. Shadow 0.5 (× 0.5 if baby, § 1.5). No transform of its own.
4. Animations: strictly those of the zombie (§ 2.2).

### 1.2 Drowned (`drowned`)
1. Model layers: `minecraft:drowned#main` (64×64 humanoid with left arm and leg with their own texture),
   baby `minecraft:drowned_baby#main` (`zombie_baby` geometry). Textures `entity/zombie/drowned`,
   baby `entity/zombie/drowned_baby` (64×64).
2. "Seaweed" layer: copied layer, model layer `minecraft:drowned#outer` (baby
   `minecraft:drowned_baby#outer`), texture `entity/zombie/drowned_outer_layer` (baby
   `entity/zombie/drowned_outer_layer_baby`), cutout. Armor `drowned#…` / `drowned_baby#…`.
3. Swim transform, added AFTER the generic rotations (§ 1.3): if w = swim rate > 0
   (same counter as the player: +0.09 per tick in "swimming" pose, −0.09 otherwise, clamped to [0, 1], interpolated
   by pt — it is kept for every living entity), rotation around the X axis of lerp(w, 0, −10 − pitch)
   degrees, around the point (0, box height / 2 / scale, 0) (frame not yet flipped: the
   rotation pivots at the middle of the body). Shadow 0.5.
4. Animations:
   - Arm pose: main arm in `throw_trident` (§ 2.1.1) if the drowned is aggressive (Mob 0x04) AND
     holds a trident in that arm's hand; otherwise generic pose (`EMPTY`/`SPEAR`).
   - Zombie arms (§ 2.2): an arm in trident pose is not overwritten.
   - Swimming (replaces step 8 of § 2.1 when w > 0), with rl(t, a, b) = shortest-path angular
     interpolation:
     - right arm rot_x = rl(w, rot_x, −4π/5) + w × 0.35 × sin(0.1 A);
     - left arm rot_x = rl(w, rot_x, −4π/5) − w × 0.35 × sin(0.1 A);
     - right arm rot_z = rl(w, rot_z, −0.15); left arm rot_z = rl(w, rot_z, 0.15);
     - left leg rot_x −= w × 0.55 × sin(0.1 A); right leg rot_x += w × 0.55 × sin(0.1 A);
     - head rot_x = 0.
   - The baby model uses the same formulas on its parts.

### 1.3 Giant (`giant`)
1. Model layer `minecraft:giant#main` (humanoid ×6 baked: root at y = −120.08, scale 6). Texture
   `entity/zombie/zombie` (64×64). No baby.
2. Layers: held item THEN armor `giant#…` (armor ×6 baked). NO worn head nor wings.
3. Shadow 0.5 × 6 = 3.0.
4. Animations: humanoid (§ 2.1) + zombie arms (§ 2.2) with **"aggressive" always false** (the
   flag is not read for the giant: arms stretched out at −π/2.25). Generic arm poses.

### 1.4 Zombie villager (`zombie_villager`)
1. Model layers: `minecraft:zombie_villager#main`, baby `minecraft:zombie_villager_baby#main`. Base
   textures `entity/zombie_villager/zombie_villager`, baby `entity/zombie_villager/zombie_villager_baby`
   (64×64). The generic rule only covers the adult without layers: the baby and the profession layer are needed.
2. "Profession" layer (also valid for the villager, root path `entity/villager/…`), if the entity
   is not invisible and the villager data is known (metadata 20, same format as
   villager metadata 18: type, profession, level):
   - hat of the type h_t and of the profession h_m read from the `villager.hat` section of the texture's `.mcmeta`
     (`none` by default, `partial`, `full`): `entity/zombie_villager/type/<type>.png.mcmeta` and
     `…/profession/<profession>.png.mcmeta` (in 26.3: professions butcher = partial; farmer, fisherman,
     fletcher, librarian, shepherd = full; no zombie type has a file);
   - pass 1 (order 1): texture `entity/zombie_villager/type/<type>` (baby:
     `entity/zombie_villager/baby/<type>`); model = body model layer if h_m = none, or if h_m = partial
     and h_t ≠ full; otherwise "no hat" model layer `minecraft:zombie_villager_no_hat#main` (baby
     `minecraft:zombie_villager_baby_no_hat#main`), whose head is empty;
   - pass 2 (order 2), adult only and profession ≠ `none`: `entity/zombie_villager/profession/<profession>`
     on the body model layer;
   - pass 3 (order 3), adult, profession ≠ `none` and ≠ `nitwit`: `entity/zombie_villager/profession_level/
     <level>` with level clamped to [1, 5] → stone, iron, gold, emerald, diamond.
   - All cutout, animated like the body. `<type>`, `<profession>` = path of the registry id.
   Armor `zombie_villager#…` / `zombie_villager_baby#…`. Worn head with offsets (y −0.1171875,
   skull y −0.07421875, horizontal scale 1).
3. Shadow 0.5. Shaking if metadata 19 (conversion in progress) is true, or conversion to drowned.
4. Animations: zombie (§ 2.2). Own metadata: 19 conversion (boolean), 20 villager data,
   21 "data finalised" (boolean, no effect at render time).

### 1.5 Zombified piglin (`zombified_piglin`)
1. Model layers `minecraft:zombified_piglin#main` (= adult piglin geometry), baby
   `minecraft:zombified_piglin_baby#main` (= baby piglin). Textures `entity/piglin/zombified_piglin`,
   baby `entity/piglin/zombified_piglin_baby` (64×64).
2. **Body rendered translucent** (like every piglin model). Armor `zombified_piglin#…` /
   `zombified_piglin_baby#…` (without ears). Worn head with horizontal scale 1.0019531.
3. Shadow 0.5; no shaking of its own (freezing only).
4. Animations: humanoid (§ 2.1), then ears (§ 3.1, base angle 30° adult, 5° baby), and in the
   "attack" step: generic attack then zombie arms (§ 2.2, aggressive = Mob 0x04).
   Metadata: those of the zombie (16 baby…).

---

## 2. Derived skeletons

Common base (skeleton, § 2.3): shadow 0.5; "bow" pose of the main arm if aggressive and bow in the main
hand; attack without a bow; item offset by ±1 px. No common metadata.

### 2.1 Stray (`stray`)
1. Model layer `minecraft:stray#main` (64×32 skeleton). Texture `entity/skeleton/stray` (64×32).
2. "Clothing" layer: copied layer, model layer `minecraft:stray#outer` (humanoid inflated by 0.25,
   animated by the skeleton's formulas), texture `entity/skeleton/stray_overlay` (64×32). Armor `stray#…`.
3. Shadow 0.5. 4. Skeleton animations.

### 2.2 Bogged (`bogged`)
1. Model layer `minecraft:bogged#main` (skeleton + group `mushrooms` child of `head`, six mushroom
   planes), texture `entity/skeleton/bogged` (64×32).
2. "Clothing" layer: model layer `minecraft:bogged#outer` (inflated 0.2), texture
   `entity/skeleton/bogged_overlay`, copied layer. Armor `bogged#…`.
3. Shadow 0.5.
4. Skeleton animations; in addition, group `mushrooms` hidden if metadata 16 (sheared, boolean) is
   true.

### 2.3 Parched (`parched`)
1. Model layer `minecraft:parched#main`: "double body" 64×64 skeleton (each part carries the bone AND its
   overlay in the same model layer: arms at pivots (±5.5, 2, 0), legs (±2, 12, 0)). Texture
   `entity/skeleton/parched` (64×64).
2. No clothing layer (the model layer `minecraft:parched#outer` exists but is not used).
   Armor `parched#…`.
3. Shadow 0.5. 4. Skeleton animations (item offset by ±1 px, even though the arms are at ±5.5).

### 2.4 Wither skeleton (`wither_skeleton`)
1. Model layer `minecraft:wither_skeleton#main` (skeleton ×1.2 baked). Texture
   `entity/skeleton/wither_skeleton` (64×32). 2. Armor `wither_skeleton#…` (×1.2 baked).
3. Shadow 0.5 (not enlarged). 4. Skeleton animations.

---

## 3. Piglins

### 3.1 Piglin (`piglin`) — additions
1. Model layers `minecraft:piglin#main` (wide player mesh with jacket, sleeves, pant legs; head with
   ears `left_ear`/`right_ear`; empty `hat` part), baby `minecraft:piglin_baby#main` (ears
   on two levels: `left_ear` then child `left_ear_r1` tilted by ∓0.6109). Textures
   `entity/piglin/piglin`, baby `entity/piglin/piglin_baby` (64×64).
2. **Body translucent.** Armor `piglin#…` / `piglin_baby#…` (ears removed). Worn head:
   horizontal scale 1.0019531.
3. Shadow 0.5. Shaking during zombification: true if immunity (metadata 16) is false, if
   the mob has its AI (Mob 0x01 not set) and if the environment attribute "piglins zombify" is true
   at the position (true outside the Nether in vanilla).
4. Animations, in order:
   - full humanoid (§ 2.1), except the "attack" step: if s > 0 and pose = melee weapon →
     "downward weapon swing" (below), otherwise generic attack (§ 2.1.2);
   - ears: d = base angle (30° adult, 5° baby) in radians; f = A × 0.1 + P × 0.5;
     a = 0.08 + V × 0.4; left ear rot_z = −d − cos(1.2 f) × a; right = d + cos(f) × a
     (absolute values on the `left_ear`/`right_ear` parts);
   - piglin-specific pose (choice below):
     - dance: t = A / 60; right ear rot_z = π/6 + (π/180) × sin(30 t) × 10; left
       rot_z = −π/6 − (π/180) × cos(30 t) × 10; head x += sin(10 t); head y += sin(40 t) + 0.4;
       right arm rot_z = (π/180)(70 + cos(40 t) × 10); left arm rot_z = −right arm rot_z; right arm
       y += sin(40 t) × 0.5 − 0.5; left arm y += sin(40 t) × 0.5 + 0.5; body y += sin(40 t) × 0.35;
     - melee weapon AND s = 0: main arm rot_x = −1.8 (weapon raised);
     - crossbow held: as `crossbow_hold` (§ 2.1.1), holding hand = main arm;
     - crossbow charging: as `crossbow_charge`, charge duration of the item in use, use ticks;
     - admiring: head rot_x = 0.5, rot_y = 0; SECONDARY arm: rot_y = ±0.5 (left +, right −),
       rot_x = −0.9.
   - Downward weapon swing (main arm M, other arm O): a2 = sin(sπ); a1 = sin((1 − (1 − s)²)π);
     rot_z = 0 on both arms; right arm rot_y = π/20, left rot_y = −π/20; M rot_x = −1.8849558 +
     cos(0.09 A) × 0.15 + a2 × 2.2 − a1 × 0.4; O rot_x = cos(0.19 A) × 0.5 + a2 × 1.2 − a1 × 0.4; then
     idle sway (§ 2.1 step 7).
   - Pose choice (piglin): dance (metadata 19) > admiring (item in the secondary hand in tag
     `piglin_loved`) > melee weapon (aggressive AND main item carrying the `tool` component) > charging
     (metadata 18) > crossbow held (crossbow in hand AND charged) > default.
   - Metadata: 16 zombification immunity, 17 baby, 18 crossbow charge, 19 dance.

### 3.2 Piglin brute (`piglin_brute`)
1. Model layer `minecraft:piglin_brute#main` for the adult AND the baby (same geometry as the adult piglin;
   no real baby). Single texture `entity/piglin/piglin_brute` (64×64), whatever the baby flag.
2. Translucent; armor `piglin_brute#…` (also for the baby). Worn head like the piglin.
3. Shadow 0.5; zombification shaking like the piglin (metadata 16).
4. Piglin animations, ear angle 30°; possible poses: melee weapon (aggressive AND tool in
   the main hand) or default only.

---

## 4. Illagers

### 4.1 Common base
1. Model layers `minecraft:pillager#main`, `vindicator#main`, `evoker#main`, `illusioner#main` (same
   geometry: root at scale 0.9375 baked; parts `head` (children `nose`, `hat`), `body`,
   `arms` (crossed arms, base pose (0, 3, −1) rot_x −0.75, child `left_shoulder`), `right_arm`,
   `left_arm`, `right_leg`, `left_leg`). 64×64 textures: `entity/illager/pillager`, `…/vindicator`,
   `…/evoker`, `…/illusioner`.
2. The `hat` part is **hidden** (except illusioner). Layers: worn head (default transforms),
   held item (conditions per kind). No armor.
3. Shadow 0.5. No baby.
4. Animation:
   - head: rot_y = yR, rot_x = xR;
   - passenger: arms rot_x = −π/5, rot_y = rot_z = 0; legs as § 2.1 step 3;
   - otherwise: right arm rot_x = cos(0.6662 P + π) × V; left rot_x = cos(0.6662 P) × V; right leg
     rot_x = cos(0.6662 P) × 0.7 V; left rot_x = cos(0.6662 P + π) × 0.7 V; other angles = 0
     (legs half as wide-swinging as a humanoid's);
   - then depending on the pose:
     - **crossed arms**: part `arms` visible, `right_arm` and `left_arm` hidden; in all the
       other poses it is the reverse;
     - **attack**: empty main hand → zombie arms (§ 2.2) with aggressive true and "raise" true,
       then idle sway; item in hand → downward weapon swing (§ 3.1);
     - **spell**: right arm (x, z) = (−5, 0), left (5, 0); rot_x = cos(0.6662 A) × 0.25 on both;
       right arm rot_z = 3π/4, left rot_z = −3π/4; rot_y = 0;
     - **bow**: right arm rot_y = −0.1 + head rot_y, rot_x = −π/2 + head rot_x; left arm
       rot_x = −0.9424779 + head rot_x, rot_y = head rot_y − 0.4, rot_z = π/2;
     - **crossbow held / charging**: formulas `crossbow_hold` / `crossbow_charge` (§ 2.1.1) with the
       RIGHT hand always holding, even for a left-hander;
     - **celebration**: right arm (x, z) = (−5, 0), rot_x = cos(0.6662 A) × 0.05, rot_z = 2.670354,
       rot_y = 0; left arm (5, 0), same rot_x, rot_z = −3π/4, rot_y = 0;
     - **neutral**: nothing more (arms dangling).
   - No generic attack (§ 2.1.2) nor idle sway outside the cases cited.
   - A mob's main arm: Mob 0x02 → left-handed.
   - Metadata: 16 celebration (boolean, common to the raiders: illagers and ravager).

### 4.2 Pillager (`pillager`)
- Pose: crossbow charge (metadata 17) > crossbow held (crossbow in one hand) > attack if
  aggressive > neutral. Never crossed arms.
- Held item always drawn (right or left hand depending on the arm).
- Charge duration: that of the crossbow in use (Quick Charge enchantment included); use
  ticks interpolated by pt.

### 4.3 Vindicator (`vindicator`)
- Pose: attack if aggressive > celebration (metadata 16) > crossed arms.
- Held item drawn **only if aggressive** (the axe disappears with crossed arms or in celebration).

### 4.4 Evoker (`evoker`)
- Pose: spell if metadata 17 (spell in progress, byte) > 0 > celebration > crossed arms.
- Held item drawn only during a spell.
- Spell particles (every client tick while the byte > 0): two tinted `entity_effect` particles,
  at positions (x ± cos(α) × 0.6 × scale, y + 1.8 × scale, z ± sin(α) × 0.6 × scale)
  with α = body yaw in radians + cos(ticks_lived × 0.6662) × 0.25. Colour by byte
  value: 1 vex summoning (0.7; 0.7; 0.8); 2 fangs (0.4; 0.3; 0.35); 3 "wololo"
  (0.7; 0.5; 0.2); 4 disappearance (0.3; 0.3; 0.8); 5 blindness (0.1; 0.1; 0.2); 0 or out of
  range → no spell.

### 4.5 Illusioner (`illusioner`)
- `hat` part **visible** (hood).
- Pose: spell (metadata 17 > 0) > bow if aggressive > crossed arms (no celebration).
- Held item drawn during a spell or if aggressive. Spell particles as § 4.4.
- Invisibility: the body is always drawn opaque (never translucent nor hidden). If the entity is
  invisible, the full render (model, layers, label) is repeated 4 times, each copy i (0‑3)
  translated by o_i + (cos(i + 0.5 A) × 0.025, cos(i + 0.75 A) × 0.0125, cos(i + 0.7 A) × 0.025) blocks.
  Visibility box enlarged by 3 blocks in x and z.
- Offsets o_i (client, updated in the living tick while the entity is invisible): two sets
  "old" and "new" (initially zero), counter c ≥ 0 decremented every tick.
  - If hurt_time = 1 or ticks_lived is a multiple of 1200: c = 3; old = new; new_i =
    ((−6 + random[0, 12]) × 0.5, max(0, random[0, 5] − 4), (−6 + random[0, 12]) × 0.5); 16 `cloud` particles;
    "mirror" sound.
  - Otherwise if hurt_time = hurt duration − 1: c = 3; old = new; new = 0.
  - Render: if c ≤ 0, o_i = new_i; otherwise k = ((c − pt) / 3)^0.25 and o_i = new_i × (1 − k) +
    old_i × k.

### 4.6 Vex (`vex`)
1. Model layer `minecraft:vex#main` (root → part `root` at y −2.5 → `head`, `body`; `body` carries
   `right_arm`, `left_arm`, `right_wing`, `left_wing`). Texture `entity/illager/vex` (32×32), or
   `entity/illager/vex_charging` if bit 0x01 of metadata 16 (flags byte: charging) is set.
2. **Body translucent.** Held item layer: attachment point = root → `root` → `body` → arm,
   then scale 0.55, then translation (±0.046875; −0.15625; 0.078125) block (+ right hand, − left).
3. Block light forced to 15 (always lit). Shadow 0.3.
4. Animations:
   - head rot_y = yR, rot_x = xR;
   - b = cos(A × 5.5 × π/180) × 0.1; right arm rot_z = π/5 + b; left rot_z = −(π/5 + b);
   - charging: body rot_x = 0; with no item in either hand: right arm (rot_x −1.2217305,
     rot_y π/12, rot_z −0.47123888 − b), left (−1.2217305, −π/12, 0.47123888 + b); otherwise only the arm
     holding an item: right (7π/6, π/12, −0.47123888 − b), left (7π/6, −π/12, 0.47123888 + b);
   - not charging: body rot_x = π/20;
   - wings: left rot_y = 1.0995574 + cos(A × 45.836624 × π/180) × (π/180) × 16.2; right
     rot_y = −left; rot_x = 0.47123888 on both; rot_z left −0.47123888, right +0.47123888.

### 4.7 Evoker fangs (`evoker_fangs`) — non-living entity
1. Model layer `minecraft:evoker_fangs#main`: `base` (pose (−5, 24, −5)) with `upper_jaw` and `lower_jaw`.
   Texture `entity/illager/evoker_fangs` (64×32). Cutout, no hurt overlay.
2. No layer. No shadow (radius 0).
3. Stack: nothing is drawn if progress b = 0. Otherwise Y rotation of (90 − entity yaw)
   degrees (yaw not interpolated), scale (−1, −1, 1), translation (0, −1.501, 0), model.
4. Progress and animation:
   - on the client side, entity event 4 starts the attack; life = 22 ticks, decremented every tick
     once started (at life = 14: 12 `crit` particles);
   - b = 0 while not started; otherwise r = life − 2; b = 1 if r ≤ 0, otherwise 1 − (r − pt) / 20;
   - m = min(2b, 1); m = 1 − m³; `upper_jaw` rot_z = π − 0.35π m; `lower_jaw` rot_z = π + 0.35π m;
   - `base` y −= (b + sin(2.7 b)) × 7.2;
   - k = 1, and if b > 0.9: k = (1 − b) / 0.1; root y = 24 − 20 k; root scale (k, k, k).

### 4.8 Ravager (`ravager`)
1. Model layer `minecraft:ravager#main` (128×128: `neck` → `head` → `mouth`, horns; `body` lying down
   rot_x π/2; four legs). Texture `entity/illager/ravager`.
2. No layer. 3. Shadow 1.1. No baby.
4. Counters (client, decremented every tick while alive): attack (event 4 → 10), stunned
   (event 39 → 40; when it falls back to 0 → roar = 20), roar. Render values:
   a = attack − pt if > 0, otherwise 0; e = stunned − pt if > 0, otherwise 0; g = (20 − roar + pt) / 20
   if roar > 0, otherwise 0.
   - if a > 0: t = triangle wave(a, 10) with wave(x, p) = (\|x mod p − p/2\| − p/4) / (p/4);
     h = ((1 + t) × 0.5)³ × 12; `neck` z = −6.5 + h, y = −7; `mouth` rot_x = sin((a − 4) / 4) × 0.4π if
     a > 5, otherwise (π/20) × sin(π a / 10);
   - otherwise: `neck` (x, y, z) = (0, −7, 5.5); stunned (e > 0): `neck` rot_x = 0.21991149,
     `mouth` rot_x = 0.05π, `neck` x = sin(e / 40 × 10) × 3; not stunned: `mouth` rot_x = 0.01π, and if
     g > 0: `mouth` rot_x = (π/2) × sin(g × π × 0.25);
   - (the neck's vertical offset, proportional to sin(neck rot_x) read BEFORE its update, is
     always 0: y = −7 in both cases);
   - `head` rot_x = xR, rot_y = yR;
   - legs, l = 0.4 V: back right and front left rot_x = cos(0.6662 P) × l; back left and
     front right rot_x = cos(0.6662 P + π) × l.
   - Metadata: 16 celebration (no effect at render time).

---

## 5. Zoglin (`zoglin`) — and hoglin
1. Model layers `minecraft:zoglin#main` (128×64), baby `minecraft:zoglin_baby#main` (64×64). Textures
   `entity/hoglin/zoglin`, baby `entity/hoglin/zoglin_baby`. Baby = metadata 16 (boolean; the zoglin
   is not an "ageable", hence index 16). (Hoglin: same formulas, model layers `hoglin`/`hoglin_baby`,
   textures `entity/hoglin/hoglin[_baby]`, baby = 16, immunity = 18, shaking if converting.)
2. No layer. 3. Shadow 0.7 (× 0.5 baby).
4. Animations:
   - ears: right rot_z = −2π/9 − V × sin(P); left rot_z = 2π/9 + V × sin(P) (also overwrites the
     baby's base pose ±0.8727);
   - head rot_y = yR (no pitch);
   - headbutt: r = remaining attack ticks (event 4 → 10, −1 per tick, not interpolated);
     f = 1 − \|10 − 2r\| / 10; head rot_x = lerp(f, 0.87266463, −π/9); baby: head y += f × 2.5;
   - legs: front right rot_x = cos(P) × 1.2 V; front left = cos(P + π) × 1.2 V; back right =
     front left; back left = front right (frequency 1, not 0.6662).

---

## 6. Armor stand (`armor_stand`)
1. Model layers `minecraft:armor_stand#main` (normal), `minecraft:armor_stand_small#main` (small: baked
   humanoid baby rule). Texture `entity/armorstand/armorstand` (64×64). Parts: `head` (child
   `hat`, **hidden**), `body`, `right_arm`, `left_arm`, `right_leg`, `left_leg`, `right_body_stick`,
   `left_body_stick`, `shoulder_stick`, `base_plate`.
2. Layers: armor (model layers `armor_stand#…`, small `armor_stand_small#…`, model animated by the same
   poses as the body), held item, wings, worn head. Armor and items stay visible if the
   stand is invisible.
3. Transforms: rotations replaced entirely by: Y rotation of (180 − body yaw), then,
   if w = (game time − last hit) + pt < 5, an extra Y rotation of sin(w / 1.5 × π) × 3
   degrees (wobble; last hit = game time when entity event 32 is received, 0 at the
   start). No death flip nor upside-down head. Body yaw = entity yaw, without a
   body controller. Shadow 0 (none). Name label: only if "name always visible".
4. Animations (absolute values, in degrees converted to radians, after the humanoid animation which they
   overwrite):
   - `head` (x, y, z) = head pose; `body` = body pose; `left_arm` / `right_arm` = arm poses;
     `left_leg` / `right_leg` = leg poses;
   - `right_body_stick`, `left_body_stick`, `shoulder_stick` = body pose;
   - `base_plate` rot_y = −(interpolated entity yaw): the base compensates for the body rotation;
   - visibility: arms if "arms" flag; base unless "no base" flag. The held item stays
     attached to the arm even when hidden.
   - Metadata: 15 flags byte (0x01 small, 0x04 arms, 0x08 no base, 0x10 marker);
     16 head, 17 body, 18 left arm, 19 right arm, 20 left leg, 21 right leg — each three
     `f32` (x, y, z in degrees), reduced modulo 360 (NaN/infinity → 0). Defaults: head (0, 0, 0), body
     (0, 0, 0), left arm (−10, 0, −10), right arm (−15, 0, 10), left leg (−1, 0, −1), right leg
     (1, 0, 1).
   - Marker: normal cutout render if visible; invisible but seen (spectator) → translucent;
     otherwise nothing, and no "glowing" outline.

---

### Uncertain points
- `minecraft:parched#outer` is defined but no layer uses it in 26.3 (the parched carries its
  overlay in its main model layer): do not draw it.
- Invisible illusioner: the name label and the shadow seem to be submitted for each copy (full
  render repeated); not verified in game.
- Piglin shaking: depends on an environment attribute evaluated on the client side (dimension/biome);
  the source of this attribute on our side remains to be wired up.
- Profession layer: the current villager implementation does not draw it yet; the same rule applies
  to it with the path `entity/villager/…` (types `desert` and `snow` have a hat `.mcmeta` there).
- Exact order of the piglin's and vex's translucent passes relative to other entities: not studied
  (global sorting out of scope).
