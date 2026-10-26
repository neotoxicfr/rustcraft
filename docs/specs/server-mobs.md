# Server-side mobs — behaviour specification (26.3)

Purpose: what the integrated server of the Rust singleplayer (`docs/architecture.md` § 14) must simulate so
that mobs behave as in the official 26.3 game: physics, spawning and
despawning, intelligence (goals, brains, navigation), the particularities of each mob, and
what guarantees a faithful tick-by-tick reproduction.

Already specified elsewhere, not repeated here:
- `movement.md`: physics of the local player (movement formulas, collisions, step-up,
  ladders, cobwebs, water, lava). Mobs use **the same routines**; only the
  differences are listed in § 1.
- `entities.md`, `entities-animals.md`, `entities-humanoids.md`, `entities-objects.md`,
  `mounts.md`: network packets, synchronised metadata, client-side rendering and animations.
  The integrated server must **produce** this metadata; its format is described there.
- `weather-border.md`, `environment.md`: weather, day cycle, environment attributes.

Conventions: distances in blocks, time in ticks (20 per second), "random draw" = a draw on the
random generator indicated (the world's or the entity's, see § 5). Game identifiers
(`minecraft:…`, tags `#minecraft:…`, attributes `max_health`, memories `walk_target`)
are those of the game data. The numeric values come from the decompiled 26.3 game (read outside the
repository, clean room).

---

## 1. Common physics of entities on the server

### 1.1 Where the physics runs

On the server, every living mob whose AI is "effective" (not controlled by a passenger player)
computes its own movement each tick. The path is the player's
(`movement.md` § 3 and § 4), with these differences:

1. **Minimum speed threshold** (step 4 of the generic AI): for everything that is not a
   player, the test is done **axis by axis**: |vx| < 0.003 → vx = 0; |vz| < 0.003 → vz = 0;
   |vy| < 0.003 → vy = 0 (the player tests the horizontal norm against 9.0E-6).
2. **Inputs**: there are no keys. The inputs (sideways, vertical, forward) and the
   "jump held" flag are written by the move and jump controls (§ 3.4). Just before
   the AI, the sideways and forward inputs of the previous tick are multiplied by 0.98F.
3. **Server AI** (§ 3.1): run at step 6 (after the threshold, before the jump), unless
   the entity is immobilised (dead, asleep…), in which case jump = false and inputs = 0.
4. **Ground speed**: the "speed" of the formula in `movement.md` § 4.3 is not the raw
   attribute but the `speed` value set by the move control (`movement_speed` attribute ×
   goal modifier). **Air speed**: constant 0.02F (no sprint bonus); if a
   player controls the mount: speed × 0.1F.
5. **Ladders**: like the player (clamping to ±0.15, fall ≥ −0.15), but a mob can
   never "hold" the ladder (holding while sneaking is specific to the player). A mob on a
   climbable block is **not pushable**.
6. **Jump from the ground**: the player's formula without the sprint bonus (a mob that
   "sprints" in the sense of the flag gets one, but mobs do not set that flag
   except in special cases). 10-tick delay between jumps like the player.
7. **Jump in a liquid**: if the jump is held in water or lava, the mob gets
   +0.04F in vy **if its navigation can float** (most land mobs, see
   § 3.5); otherwise +0.3 at once (mobs whose navigation does not float).
8. **Gravity**: `gravity` attribute (0.08 by default). Mobs without gravity (`NoGravity`
   flag, or flyers such as the ghast/phantom/vex/blaze, which have their own mode):
   see § 1.6.
9. **Water**: as in `movement.md` § 8.4, horizontal "water" slowdown = 0.8F by default
   (redefined: 0.96F skeleton horse, 0.98F polar bear; swimmers have their own mode,
   § 1.6), speed 0.02F, `water_movement_efficiency` attribute (halved off the ground) and
   `dolphins_grace` effect (0.96) as for the player. Leaving the water: if there is a horizontal collision and
   free space 0.6 higher → vy = 0.3F.
10. **Lava**: like the player (0.5 horizontal, gravity/4). Mobs that "stand"
    on a fluid (strider on lava) do not switch to fluid mode on that fluid.
11. **Powder snow and freezing**: like the player; each tick outside powder snow, the freeze
    counter drops by 2; every 40 ticks, a fully frozen mob that can freeze takes
    1 damage (`freeze`).
12. **Water sensitivity** (enderman, blaze, snow golem, strider): 1 damage
    (`drown`) every tick in water or in rain.

### 1.2 Order of a living entity's tick (server)

1. The common tick (shared by all entities): invulnerability delay −1, memorisation of the
   previous position and angles, tick counter +1.
2. Base tick of the living entity:
   1. arm animation;
   2. worn enchantment effects;
   3. generic base tick: measured speed, leaving a removed vehicle, boarding
      delay, portals, fluid and swimming detection, **fire** (1 damage every
      20 ticks of remaining fire outside lava; fire-immune: fire extinguished), fall distance
      ×0.5 in lava, death below the world (y < y_min − 64), leash;
   4. if alive: suffocation inside a block (1 `in_wall` damage): eye box (width × 0.8
      per side, 1e-6 tall) touched by the collision of a **suffocating** block — by default tag
      `causes_suffocation` and full collision; never glass (stained, tinted), foliage,
      mangrove roots, copper grates, moving piston; always farmland,
      soul sand, path, mud; piston only retracted, shulker box only closed;
      same rule for the player; **air** underwater (eyes
      in water outside a bubble column): air −1 per tick (except water breathing,
      Respiration enchantment: chance not to lose any), at −20 → air = 0 and 2 `drown` damage;
      out of the water, air +4 per tick up to the max (300); change of block → movement
      enchantment effects;
   5. "recently hurt" counter −1, damage delay −1; death in progress → death tick
      (at 20 ticks: loot, orbs, disappearance);
   6. forgetting the "last attacker": if dead, or if more than **100 ticks** since the hit;
   7. potion effects;
   8. **Mob**: its ambient sound if the random draw `next_int(1000) < counter++` (the counter restarts
      at −interval, default interval 80 ticks) — this draw happens **every tick**
      and consumes the entity's random generator.
3. Tick of the living entity: item in use, stuck arrows/darts that fall off,
   equipment update, then **the AI step** (below), then body rotation towards the
   walking direction and wrapping of the angles into ±180.
4. The AI step: jump delay −1; speed threshold (§ 1.1-1); server AI (§ 3.1); jump (§ 1.1-6/7);
   movement (`travel`); effects of the blocks crossed; freezing; **pushing** (§ 1.4);
   water damage for the sensitive ones. For a Mob, then: **burning in sunlight** of the
   mobs of the tag `#minecraft:burn_in_daylight` (§ 1.7) then **item pickup** (§ 1.8).
5. Every 5 ticks (`ticks_lived % 5 == 0`): update of the goals' control flags
   (§ 3.2) depending on whether the mob is controlled by a mob passenger or is in a boat.

### 1.3 Collisions and step-up

Identical to the player (`movement.md` § 6) with the mob's box (dimensions per type and
per pose, ×`scale`, ×0.5 for a baby of most species). Step height:
`step_height` attribute (0.6 by default; 1.0 for horse, camel…, see § 4). No caution
at the edge (reserved for the sneaking player): it is the path evaluators that avoid voids.

### 1.4 Pushing between entities

At the end of the life tick (AI step), the entity lists the pushable entities that **intersect** its box
(spectators excluded; team rules `collisionRule`). For each one, applied in the order of
the list:
- **Cramming**: if the rule `maxEntityCramming` (24 by default) is > 0, the list holds
  more than `max − 1` entities and the random draw `next_int(4) == 0` succeeds, we count those that are not
  passengers; if that count > `max − 1` → 6 `cramming` damage.
- **Mutual push**: dx, dz = difference of the positions (other − self); d = max(|dx|, |dz|);
  if d ≥ 0.01F: d = √d; dx/=d, dz/=d; p = min(1/d, 1); dx, dz ×= p × 0.05F. Self receives
  (−dx, 0, −dz) if self has no passenger and is pushable; the other receives (dx, 0, dz) under
  the same conditions. No push between passengers of the same vehicle, nor if one of the two
  passes through blocks (entity without physics). A sleeping entity is never pushed.
- Pushable: living, not a spectator, not on a climbable block, and in an area where
  entities tick (except "always active" entities). Some mobs are not
  pushable (shulker, warden while emerging, attached mobs…): § 4.

### 1.5 Falling and fall damage

- Accumulation and reset of the fall distance: `movement.md` § 4.6; on the server
  also: ×0.5 per tick in lava, reset to 0 under `slow_falling`/`levitation`, as a
  passenger (at each passenger tick).
- On landing (distance > 0): call "fall on" of the block touched (block with offset 0.2F: the
  hay bale, the bed, the slime… change the multiplier or cancel it; the slime and the bed
  make the entity bounce), game event `hit_ground`.
- **Damage**: power = distance + 1.0E-6 − `safe_fall_distance` (3 by default);
  damage = floor(power × block multiplier × `fall_damage_multiplier`). Types of the
  tag `#minecraft:fall_damage_immune` never take any. The fall propagates to passengers.
- **Recent impulse** (wind charge, breeze breath…): during the grace period
  (40 ticks), the distance counted is min(distance, y_impact − y); if ≤ 0, nothing.
- Fall particles: client-side from the network event, see `particles.md`.

### 1.6 Special movement modes

- **Flyers without gravity** (ghast, happy ghast, phantom, vex, allay, bee, parrot in
  flight, blaze partially): "flying" movement: relative acceleration (0.02F in water
  and lava, own speed in the air), movement with collisions, then ×0.8F in water,
  ×0.5 in lava, ×0.91F in the air; no gravity. Control details: § 3.4.
- **Swimmers** (fish, dolphin, guardian, axolotl, tadpole, turtle in water, nautilus): in
  water, relative acceleration (fish and tadpole 0.01F, guardian 0.1F, dolphin its current
  speed, others per § 4), movement, ×0.9, then −0.005 in vy if they have no target
  (drift downwards; the guardian only if it is not moving as well); the squid propels itself in bursts (§ 4); out of water, they fall
  and flop (§ 4).
- **Climbers** (spiders): horizontal collision → "climbing" flag; a mob that
  climbs behaves as on a ladder.
- **"Omnidirectional in the air" mobs** (flyers): the vertical friction equals the
  horizontal friction of the air (0.91F instead of 0.98F).

### 1.7 Burning in sunlight

For the types of the tag `#minecraft:burn_in_daylight` (zombie, skeleton, phantom, drowned out of
the water…, the environment attribute `monsters_burn` must be true at the position):
- l = "magic" light value at the eyes: v = max raw brightness / 15;
  curve = v / (4 − 3v); l = lerp(ambient light of the dimension, curve, 1);
- burn tick if l > 0.5 **and** random draw `next_float()×30 < (l − 0.4)×2` **and** not in water,
  rain or powder snow **and** the sky is visible above the eyes;
- if an item occupies the protective slot (head, or another depending on the type): if it is
  usable, it loses `next_int(2)` points; otherwise the mob catches fire for 8 seconds.

### 1.8 Item pickup

If the mob can pick up items (flag `CanPickUpLoot`, drawn at spawn according to the local
difficulty for zombies/skeletons, permanent for foxes, piglins, villagers, allays…), is
alive and the rule `mobGriefing` is true: every tick, items on the ground in its box
enlarged by (1, 0, 1), with no pickup delay, that the mob "wants" — ALL those of the
list taken at the start of the pass, each rechecked in turn (not just one per tick); pickup =
equip if better (weapon/armour comparison), or store in its inventory (villager,
piglin, allay…). A picked-up item makes the mob persistent.

---

## 2. Spawning and despawning

### 2.1 Categories

| Category | Cap (per 289 chunks) | Friendly | Persistent | Despawn distance |
|---|---|---|---|---|
| `monster` | 70 | no | no | 128 |
| `creature` | 10 | yes | **yes** | 128 |
| `ambient` | 15 | yes | no | 128 |
| `axolotls` | 5 | yes | no | 128 |
| `underground_water_creature` | 5 | yes | no | 128 |
| `water_creature` | 5 | yes | no | 128 |
| `water_ambient` | 20 | yes | no | **64** |
| `misc` | — (never natural) | yes | yes | 128 |

"No despawn" distance: 32 for all.

### 2.2 Natural spawning cycle (every world tick, within the chunk ticks)

Run in the tick of the world's chunk source (after the scheduled block and
fluid ticks and the raids, before the block events and the entities; see § 5.1).

1. **Census**: all the entities of the world are walked. Ignored are persistent mobs
   (named, having picked up an item, `PersistenceRequired`) and those that require
   their own persistence (passenger, leashed…), as well as the `misc` category. For
   each remaining entity whose chunk is loaded:
   - if the biome at its position has a spawn cost for its type (`spawn_costs`), we
     add a point **charge** (its position, `charge`) to the potential computation (§ 2.5);
   - if it is a Mob: it counts toward the **local** cap of each player
     near its chunk (§ 2.3);
   - it counts toward the **global** total of its category.
2. **Categories allowed this tick**: if the rule `doMobSpawning` (`spawn_mobs`) is true,
   we keep the categories other than `misc` such that: (monsters allowed **or** friendly category)
   **and** (persistence tick **or** non-persistent category) **and** global cap not
   reached. "Monsters allowed" = rules `spawn_mobs` and `spawn_monsters` true.
   "Persistence tick" = game time % 400 == 0: the `creature` category is therefore
   only attempted **one tick in 400**.
   Global cap = category cap × number of "spawn" chunks / 289, in
   integer division; number of spawn chunks = chunks at Chebyshev distance ≤ 8
   (in chunks) from a non-spectator player (17 × 17 = 289 for a lone player).
3. **Candidate chunks**: chunks that "tick" among those at distance ≤ 8 from a player, and
   whose **centre** (x·16+8, z·16+8) is at horizontal Euclidean distance **< 128** from a
   non-spectator player. The list is **shuffled** with the world's random generator (the
   game's Fisher-Yates shuffle).
4. For each chunk in that order: inhabited time +1; if it is within the entity tick area,
   lightning/snow tick of the chunk (weather, `weather-border.md`); then if categories are
   allowed and the chunk can spawn entities: for each
   allowed category, **in the order of the § 2.1 table**, if the local cap allows it
   (§ 2.3), a group attempt (§ 2.4).
5. Then the random block ticks, then, if `spawn_mobs`, the **special spawns**
   (§ 2.8) in this order: phantoms, patrols, cats, zombie siege, wandering trader.

### 2.3 Local cap (per player)

A player is "near" a chunk if they are not a spectator and the horizontal distance
from the chunk centre to their position is < 128. Each player has a counter per category. A
counted mob (or one spawned during this tick) increments the counter of **all** the players
near its chunk. A category can spawn in a chunk if **at least one** nearby
player has a counter < the category cap (70, 10, 15…) — or has no counter yet.

### 2.4 Group attempt in a chunk

1. Starting position: random x, z in the chunk (`next_int(16)` ×2, world random generator),
   y uniform between the world's minimum Y and (`WORLD_SURFACE` height at (x, z)) + 1 inclusive.
   If y < y_min + 1: abort. If the starting block is a redstone conductor (full
   opaque block): abort.
2. Up to **3 groups**; for each: x, z restart from the start; current type = none;
   provisional max size = ceil(`next_float()`×4); then up to that max size, attempts:
   1. x += `next_int(6) − next_int(6)`, z += same; y fixed; tested point = (x+0.5, y, z+0.5);
   2. nearest non-spectator player (3D distance, unlimited range; on a tie, the
      first in the player list). If there is none: next attempt;
   3. refusal if distance² to the player ≤ 576 (24 blocks), or if the point is within < 24 blocks of the world
      spawn point (same dimension), or if the chunk of the point is neither this chunk
      nor a chunk where entities can spawn;
   4. at the first valid attempt of the group: weighted draw of the type in the list of spawns
      of the category at this point (§ 2.6); empty list → end of the group. The max size then becomes
      the `count` draw of the entry (uniform range or fixed number);
   5. validation of the point for this type (§ 2.5) and of the **energy budget** (§ 2.5);
   6. creation of the mob; position (x+0.5, y, z+0.5), yaw = `next_float()`×360;
   7. final validation: if distance² > (despawn distance)² and the mob
      would despawn from afar → refusal; spawn rules specific to the instance (path
      mobs: walk value of the block ≥ 0, see below) and absence of obstruction (no
      liquid in the box, except aquatic mobs; no collision with another solid entity);
   8. finalisation (§ 2.7) with reason `natural`, addition to the world (with passengers), update
      of the potential and of the counters;
   9. stop of **the whole attempt** (the 3 groups) if the total spawned ≥ the mob's max
      cluster size: 4 by default; 8 for wolf and fish; 6 for equids; max
      school size for schooling fish; 1 for ghast, happy ghast and pillager. Stop
      of **the group only** if "group size reached": never by default; for the tropical
      fish, from the first individual if it is not a school variant.
3. `water_ambient` category in a biome of the tag `#minecraft:reduced_water_ambient_spawns`:
   the type draw fails in 98 % of cases (random draw `next_float() < 0.98`).

**Walk value** (path mobs): monsters: −(l − 0.5) where l is the magic light
value (§ 1.7); animals: 10 if the block below is a grass block, otherwise l − 0.5. The
"≥ 0" rule therefore forbids animals in the dark outside grass and monsters in full light
(redundant with the light test).

### 2.5 Validating the position for a type

In order:
1. type of category `misc` → refusal;
2. if the type cannot spawn far from the player (all except pillager, shulker… per-type
   flag) and distance² > (despawn distance of its category)² → refusal;
3. non-summonable type, or type absent from the spawn list at this point → refusal;
4. **placement** of the type (placement table, game data):
   - `on_ground`: block below that "allows spawning" of this type (block property:
     full top faces, except refusing blocks, e.g. glass, leaves except for ocelot/
     parrot, magma for the non-fire-immune…), and the block at the point **and** the one
     above are **valid empty blocks**: no full collision shape, not a redstone
     signal source, no fluid, not of the tag `#minecraft:prevent_mob_spawning_inside`, not
     dangerous for this type (fire, magma, sweet berry, cactus, wither rose… unless immune);
   - `in_water`: water fluid at the point and non-conductor block above;
   - `in_lava`: lava fluid at the point;
   - `no_restrictions`: always true.
   All require being inside the world border.
5. **spawn rule** of the type (world random generator) — families:
   - **generic mob**: reason "generator" or valid block below;
   - **monster**: (reason that ignores light **or** dark enough) and generic rule;
     "dark enough": sky light at the point > `next_int(32)` → refusal; if the dimension's block
     light limit < 15 and block light > limit → refusal; then max
     raw brightness (during a thunderstorm: taking a darkening of 10 into account) ≤ draw of the dimension's
     light level. Surface: block limit 0, uniform draw 0–7; Nether:
     limit 15 (ignored), level 7; End: limit 0, level 15;
   - **monster in any light** (blaze, breeze, zoglin, pillagers via the rule below):
     generic rule only;
   - **surface monster** (husk, parched, camel_husk): monster rule and (generator or sky
     visible);
   - **patroller**: block light > 8 → refusal, otherwise the "any light" rule;
   - **animal**: block below in `#minecraft:animals_spawnable_on` and (reason that ignores
     light, or raw brightness > 8);
   - own rules: axolotl, cod/salmon/pufferfish (surface water: between sea level
     −13 and sea level, water above…), tropical fish, squid/dolphin, glow
     squid, **slime** (not peaceful; generator: generic rule; biome of the tag
     `#minecraft:allows_surface_slime_spawns` and 50 < y < 70: `next_float()` < environment
     attribute `surface_slime_spawn_chance` and brightness ≤ `next_int(8)`; otherwise slime
     chunk — generator seeded by (chunk x, z, world seed, salt 987234911),
     `next_int(10) == 0` — and `next_int(10) == 0` and y < 40), **bat** (below the surface
     `WORLD_SURFACE`, `next_boolean()` false, brightness ≤ `next_int(4)`, block below of the tag
     `#minecraft:bats_spawnable_on`), magma cube, sulfur cube, phantom, piglin,
     hoglin, zombified piglin, strider, ocelot, parrot, rabbit, polar bear, goat, wolf,
     fox, frog, turtle, camel, armadillo, mooshroom, stray (sky visible), endermite, silverfish,
     guardian, nautilus, skeleton horse: see § 4;
   - in peaceful difficulty, any type not allowed in peaceful → refusal;
6. **energy budget** (biomes with `spawn_costs`, e.g. soul sand valley, warped forest):
   potential change = type charge × Σ (charge_i / distance(point, position_i)) over the counted
   charges; zero distance → infinity. Refusal if the change > `energy_budget`.
7. no block collision for the type's spawn box at the point.

### 2.6 Spawn lists (`spawners`, `spawn_costs`)

- In 26.3, the lists are carried by the environment attribute
  `minecraft:gameplay/natural_mob_spawns` of each biome (`worldgen/biome/*.json`, field
  `attributes`): `spawns_by_category` (category → list of entries `type`, `weight`, `count`
  integer or uniform range) and `spawn_costs` (type → `charge`, `energy_budget`). The
  value is read at the exact position (full-resolution biomes).
- **Structures**: before the biome, we look for the structures present at the point; if one has a
  `spawn_overrides` for the category and the point is within its box (`full`) or within one of
  its pieces (`piece`), the structure's list replaces the biome's (witch hut,
  monument, outpost, ancient city…).
- **Nether fortress**: in category `monster`, if the block under the point is Nether
  bricks and the point is inside a fortress, the list is the fixed list of fortress
  enemies.
- The weighted draw uses the world's random generator (sum of the weights, `next_int(total)`, walk).

### 2.7 Spawn finalisation

On any spawn (natural, generation, egg, summon, event, reinforcement, conversion,
patrol…), the mob receives a "finalisation" that depends on the local difficulty at its
position:
- draw of a random `follow_range` modifier (Gaussian × 0.05, for the mobs
  that provide for it);
- left hand (left-handed) with probability 5 %;
- default equipment (weapons and armour according to local difficulty, § 4) and enchantments;
- `CanPickUpLoot` according to local difficulty (zombies, skeletons: 0.55 × clamped local
  difficulty factor);
- variants (sheep colour, cat/wolf/rabbit/frog type according to biome…);
- **group data**: the finalisation receives and returns a value shared by the group
  (e.g. same variant for a group of horses or rabbits, "baby" for zombies),
  reset to empty between two groups.

Local difficulty: see `docs/specs` (to be written with the difficulty); depends on the world
difficulty, the chunk's inhabited time, the game time and the moon phase.

### 2.8 Spawning at chunk generation

During generation ("spawns" step of a new chunk), if `spawn_mobs`:
- we read the biome's `creature` list at the source point and the probability
  `minecraft:gameplay/creature_world_gen_spawn_probability` (0.1 by default);
- while `next_float() < probability` (dedicated generation random generator): draw of an entry;
  number = `count` draw; starting point (x, z) random in the chunk; for each
  individual, up to 4 attempts: y = height of the heightmap of the type's placement (in
  a dimension with a ceiling: go down to the air then to the ground) then placement
  adjustment (on ground: if the block below is "walkable", we go down one notch); position
  bounded in the chunk to ± width; placement, collision and spawn-rule tests
  with reason `chunk_generation`; finalisation (with group data); addition. After each
  attempt, x and z move by `next_int(5) − next_int(5)` and are redrawn around the start while
  they leave the chunk.
- These animals are de facto persistent (persistent `creature` category).

### 2.9 Despawning

Checked for each entity **before** its tick, every tick (§ 5.1):
1. peaceful difficulty and type not allowed in peaceful → immediate removal;
2. otherwise, "persistent" = flag `PersistenceRequired` or own persistence (passenger,
   attached…); several mobs make the remote-despawn test false (tamed animals,
   villagers, golems, named mobs, those with a picked-up item, cats, trusting
   foxes, fish released from a bucket, patroller on patrol within 128…);
3. nearest non-spectator player (creative included), d = distance²:
   - non-persistent, d > (category despawn distance)² (128² or 64²) and
     the remote-despawn test → immediate removal;
   - non-persistent, idle counter > 600, random draw `next_int(800) == 0` (the
     entity's random generator), d > 32² and the remote-despawn test → removal;
   - otherwise, if d < 32² → idle counter reset to 0.
   The idle counter increases by 1 on each server AI (§ 3.1); some actions
   reset it to 0 (being hurt…).
4. With no player in the world: nothing.

Since the `creature` category is persistent, animals never despawn by distance
(unless the remote-despawn test is true, which is the case for no ordinary animal; the non-
`creature` mobs such as the skeleton horse/wandering trader have their own rules).

### 2.10 Special spawns

All run on the world's random generator, except the wandering trader (own generator).

**Phantoms** (if monsters allowed and rule `doInsomnia`): counter; at 0, counter +=
(60 + `next_int(60)`) × 20. If the sky darkening ≥ 5 (night or thunderstorm) or the dimension has no
sky light: for each non-spectator player at y ≥ sea level and seeing the sky
(conditions ignored without sky light): if the effective local difficulty > `next_float()`×3,
v = clamp(statistic "time since rest", 1, max); if `next_int(v)` ≥ 72000 (3 days):
point = player + (−10+`next_int(21)`, 20+`next_int(15)`, −10+`next_int(21)`) (draws in the order
y, x, z); if valid empty block: group of 1 + `next_int(difficulty + 1)` phantoms
(peaceful 0, easy 1, normal 2, hard 3), finalised `natural` with group data.

**Patrols** (if monsters allowed and `doPatrolSpawning`): counter; at 0, counter +=
12000 + `next_int(1200)`. If the world is lit (day), then `next_int(5) == 0`, at least one
player: player drawn `next_int(n)`; non-spectator; not within ≤ 2 sections of a village; offset
x = (24 + `next_int(24)`) × (`next_boolean()` ? −1 : 1), z same; chunks loaded within ±10;
environment attribute `can_pillager_patrol_spawn` true; size = ceil(effective local
difficulty) + 1; for each member: y = `MOTION_BLOCKING_NO_LEAVES` height; the first is
the leader (banner, patrol target) and its failure cancels everything; each member requires a valid
empty block and the patroller rule (reason `patrol`); then x, z += `next_int(5) − next_int(5)`.
All are pillagers.

**Cats** (no monster condition): every 1200 ticks; random player; offset
(8 + `next_int(24)`) × sign, for x then z; chunks loaded within ±10; valid cat placement;
if within ≤ 2 sections of a village: if there are > 4 occupied `home` POIs within 48 blocks and < 5
cats in a ±48/±8/±48 box → a cat; otherwise, if within a structure piece of the tag
`#minecraft:cats_spawn_in` (witch hut) and no cat within ±16/±8/±16 → a persistent
black cat. The cat is finalised **before** being placed.

**Zombie siege** (if monsters allowed and night): at the clock marker
`roll_village_siege` (time of day 18000), siege tonight if `next_int(10) == 0`. Setup
(once): first non-spectator player in a village (≤ 1 section from a village) outside
biomes `#minecraft:without_zombie_sieges`: up to 10 attempts at angle `next_float()`×2π, point =
player + (floor(cos×32), 0, floor(sin×32)), valid if a spawn position is found →
20 zombies to spawn. Then, one zombie every 3 ticks (delay 2 then spawn):
position = 10 attempts of (point + `next_int(16)` − 8) at the `WORLD_SURFACE` height, in a village
and the monster rule (reason `event`). During the day, the siege is cancelled.

**Wandering trader** (rule `doTraderSpawning`): 1200-tick counter; each time, the
saved delay (24000 at start) drops by 1200; at ≤ 0: delay = 24000, saved
chance c (25 at start), then chance = clamp(c + 25, 25, 75); if `next_int(100)` ≤ c and the
spawn succeeds → chance = 25. Spawn: random player (none → "succeeded");
`next_int(10) != 0` → failure; reference point = `meeting` POI (bell) within 48 of the player, otherwise
the player; 10 position attempts ±48 at the placement height, valid ground placement;
2×1×2 blocks without collision; biome outside `#minecraft:without_wandering_trader_spawns`; trader
(reason `event`) + 2 trader llama attempts within ±4, leashed to the trader; despawn
delay 48000; wander target and home = reference point (radius 16).

---

## 3. Intelligence

### 3.1 A mob's server AI (one tick)

Run in the AI step of the tick (§ 1.2, step 4), if the AI is effective:
1. idle counter +1;
2. line-of-sight cache cleared (§ 3.6);
3. **goal selectors**: if (`ticks_lived` + the entity's network id) is **odd** and
   `ticks_lived` > 1: we only "tick" the running goals that require an update
   every tick, first for the **target** selector, then for the **goal**
   selector. Otherwise (even tick): full tick of the target selector then of the goal selector
   (§ 3.2). Goals are therefore re-evaluated **one tick in two**, with a phase specific to each
   entity;
4. **navigation** (§ 3.5);
5. **species-specific tick**: mobs with a brain tick their brain here (§ 3.3) then
   update their activity; others run their mechanics here (anger, growth of
   animals: age advances by +1 per tick towards 0 for a baby, goes back by −1 for an adult in
   breeding delay; love that fades if age ≠ 0…);
6. **move** control, then **look**, then **jump** (§ 3.4).

### 3.2 Goal system

Each mob with goals has two selectors: **goals** and **targets**. A goal has a priority
(integer, smaller = higher priority), a set of **control flags** among
`MOVE`, `LOOK`, `JUMP`, `TARGET`, and the operations: "can start", "can continue"
(by default = can start), "interruptible" (true by default), start, stop, tick,
"update every tick" (false by default).

Full tick of a selector:
1. **cleanup**: in the order the goals were added, every running goal that carries a disabled
   flag or can no longer continue is stopped. Then we release the flags whose holding goal
   is no longer running;
2. **start**: in the order added, every stopped goal, with no disabled flag, **each** of whose
   flags is free or held by an interruptible goal of **strictly** greater priority value
   (lower priority), and which can start: we stop the current holder of each of its
   flags, it takes the flags, then it starts. A goal with no flags starts freely;
3. **tick** of all the running goals (in the order added).
Partial tick (odd ticks): only the running goals with "update every tick" are ticked.

Consequences: "can start" is only evaluated one tick in two; most of the
timers expressed in ticks in goals without a per-tick update are **halved
rounded up** ("reduced delay"): a wander interval of 120 becomes a
draw `next_int(60)`. "Every tick" goals keep their values.

Disabled flags (every 5 ticks, § 1.2): `MOVE` and `LOOK` disabled if the mob is
controlled by a mob passenger; `JUMP` disabled in that case or in a boat. Other
one-off disablings per species (sitting, asleep…).

**Common goals** (usual parameters; per-species figures in § 4):
- **Float** (`JUMP`, every tick): active if the floatable fluid (water) exceeds the jump
  threshold, or in lava; jump requested with probability 0.8 per tick. Enables the
  navigation's floating.
- **Wander** (`MOVE`): does not start if a passenger is in control, or if the idle
  counter ≥ 100 (mob far from any player, except a variant that ignores it); draw
  `next_int(reduced delay(interval))` == 0 (interval 120 by default); random position
  (§ 3.7) ±10 horizontal, ±7 vertical; continues as long as the navigation is not done.
  Variant "avoiding water": in water, land position ±15/7 (otherwise generic); out
  of the water, with probability 1 − 0.001, land position ±10/7, otherwise generic.
- **Look at the player / an entity** (`LOOK`): starts with probability 0.02 (per
  evaluation); nearest entity of the type in range (non-combat, players that are not
  passengers of the mob); lasts `reduced delay(40 + next_int(40))`; looks at the eyes.
- **Look around** (`MOVE`+`LOOK`, every tick): probability 0.02; random direction
  2π·`next_double()`; 20 + `next_int(20)` ticks.
- **Panic** (`MOVE`): if the last damage source is of the tag
  `#minecraft:panic_causes` (or other depending on the species); when on fire, looks for water within
  Manhattan distance ≤ 5 (vertical 1); otherwise random position ±5/±4; speed ×1.25 to ×2 depending on
  the species; continues until the path ends.
- **Avoid an entity** (`MOVE`): nearest entity of the type in a box ±dist/±3/±dist
  matching the predicate; "opposite" position ±16/7; refusal if that position is closer
  to the threat than the mob is; slow speed, or fast within 7 blocks.
- **Breeding** (`MOVE`+`LOOK`): the mob is in love; partner = nearest compatible
  animal in love, not panicking, in the +8 box; walks towards it; after
  `reduced delay(60)` ticks and within 3 blocks: birth (§ 3.11). Gives up if the partner
  is no longer in love, is panicking, or after 60 ticks of counter.
- **Temptation** (`MOVE`+`LOOK`): nearest player within `tempt_range` (10) holding
  the food item in the main or off hand; after stopping, calm of `reduced delay(100)`. The
  "skittish" species (chicken, ocelot…) give up if, within 6 blocks, the player
  moves (> 0.1 block) or turns their head (> 5°). Stops at `stop_distance` (2.5 by default).
- **Follow a parent**: babies only (age < 0); nearest adult of the same class
  within ±8/±4/±8; does not start within < 3 blocks; stops at < 3 or > 16; path recomputed every
  `reduced delay(10)`.
- **Follow the owner** (tamed): starts beyond `start_distance`, stops at
  `stop_distance`; water cost set to 0 during the goal; beyond 12 blocks (144), every
  `reduced delay(10)`, teleport around the owner: 10 attempts of offset x, z in
  [−3, 3] (at least one of the two ≥ 2 in absolute value), y in [−1, 1]; `walkable` cell, not
  on leaves (except flyers), no collision.
- **Sit on command**: blocks `MOVE` and `JUMP`; active if sitting is ordered (or in the air
  and owner absent: no); stops the navigation.
- **Leap at the target**, **open/break doors**, **return to village**, **eat a
  block**, **ranged attack**: § 3.8 and § 4.

**Targeting goals** (`TARGET` flag) — common base:
- "can continue": target (or remembered target) present, attackable, not on the same team,
  at distance ≤ `follow_range`; if "must see": the unseen counter is reset to 0 when line of
  sight exists, otherwise +1, and abandon beyond `reduced delay(memory)` (memory 60 ticks by
  default, **300** for retaliation); the target is re-asserted each time;
- "must reach": reachability cache recomputed every `reduced delay(10 + next_int(5))`
  evaluations: path to the target (reach 0) whose last node is at horizontal
  distance² ≤ 2.25 from the target;
- the target must also be inside the mob's "home" area if it has one;
- stop: target cleared.
Variants:
- **Nearest attackable**: draw `next_int(reduced delay(interval))` (interval 10 by
  default → `next_int(5)`); for players, nearest player (from the eyes) meeting
  the combat conditions; otherwise entities of the type in the ±`follow_range` box; nearest
  by distance², first found on a tie. Line of sight is always required
  when choosing the target; the goal's "must see" flag only matters for the pursuit.
- **Retaliation**: if the "hurt by" timestamp has changed and the attacker exists (rule
  `universalAnger`: does not retaliate against a player); ignores line of sight and
  invisibility; can **call for help** mobs of the exact same class in a
  box ±`follow_range`/±10/±`follow_range` around a unit cube at the position, without a target,
  not allied to the attacker (same owner for tamed ones), except excluded classes.
- **Defend the owner** / **attack what the owner attacks** (tamed), **resettable
  anger** (neutral mobs, § 3.9).

Targeting conditions ("combat" or not): the target is not self, is visible to anyone
(not a spectator nor creative for combat, alive), passes the selector; in combat, the
mob can attack it and is not allied; distance ≤ max(range × visibility, 2);
visibility = 1, × 0.8 if sneaking ("discreet"), × 0.7 × max(armour coverage, 0.1) if
invisible, × the factor of the `mob_visibility` component of each worn piece of equipment that
targets this type (skeleton, creeper, zombie, piglin heads… → 0.5); then line of sight if
required (cache § 3.6). Without a targeting mob (global sensors): in peaceful, no combat
target.

### 3.3 Brains (memory, sensors, activities, behaviours)

Used by: villager, piglin, piglin brute, hoglin, zoglin, axolotl, frog, tadpole,
goat, warden, sniffer, armadillo, breeze, camel (and camel husk), allay, happy ghast,
copper golem, creaking, nautilus, zombie nautilus. Details of each: § 4.

- **Memories**: typed slots (`walk_target`, `look_target`, `attack_target`,
  `path`, `nearest_visible_player`, `hurt_by`, `angry_at`, `home`, `job_site`…) with an
  optional value and an optional **lifetime**; at each brain tick, each lifetime drops
  by 1 and the memory is erased at 0. A "registered" but empty memory is distinct from an
  unregistered memory. Persisted in NBT for those that are
  serialisable.
- **Sensors**: each has a period (20 ticks by default) and a random initial offset
  `next_int(period)` drawn at brain creation; on its turn, it rereads `follow_range` and
  writes its memories. Examples: nearby living entities (box ±`follow_range`, sorted by
  distance², then "visible" filtering), nearby players (non-spectators, within
  < `follow_range`, sorted by distance; nearest visible, nearest visible attackable),
  hurt by (last source and entity), wanted items (box ±32/±16/±32), nearby adults,
  hostiles for villagers, detected golem, nearest bed, secondary points of interest,
  temptation, water…
- **Activities**: a set of always-active "core" activities + **one** current activity
  (default `idle`). Each activity has **entry conditions** (memories present/absent)
  and memories to erase when it stops. Changing activity erases the memories of the
  activities left. Possible activities: `core`, `idle`, `work`, `play`, `rest`, `meet`,
  `panic`, `raid`, `pre_raid`, `hide`, `fight`, `celebrate`, `admire_item`, `avoid`, `ride`,
  `play_dead`, `long_jump`, `ram`, `tongue`, `swim`, `lay_spawn`, `sniff`, `investigate`,
  `roar`, `emerge`, `dig`. Villagers follow a **schedule** (environment attribute
  `villager_activity` read at their position, updated at most every 20
  ticks); the others choose, every tick, the first valid activity of an
  ordered list specific to the species.
- **Behaviours**: arranged by priority (ascending order), then by activity (hash
  table order: to be reproduced, § 5), then by order added. One brain tick: (1) age
  the memories, (2) tick the sensors (order added), (3) **start** each
  stopped behaviour of an active activity if its required memories are in the wanted state and its
  own conditions are true, (4) **tick or stop** each running behaviour (stop if
  duration exceeded or if it can no longer continue). Starting a "timed" behaviour draws its
  duration `min + next_int(max + 1 − min)` on the **world's random generator** (even if min = max). The
  "declarative" behaviours (instant triggers) run in a single tick.
- Generic behaviours: walk towards `walk_target` (computes a reach-0 path;
  duration 150–250; if unreachable, remembers `cant_reach_walk_target_since` and tries a random
  partial step ±10/7 towards the target; recomputes if the target moves by > 2 blocks; if stuck,
  random pause `next_int(40)`), look at `look_target`, random wandering, follow
  temptation, panic, swim, melee attack (memory `attack_cooling_down` with a duration),
  stop attacking if the target is invalid or unreachable for **200 ticks**, etc.

### 3.4 Move, look, jump and body controls

**Move** (default) — states: wait, move to, strafe, jump:
- *move to* (set by the navigation or by a goal with a speed modifier): goes back to
  wait; Δ = target − position; if Δ² < 2.5000003E-7: forward input = 0, end. Otherwise target yaw
  = atan2(Δz, Δx)·180/π − 90, yaw ← approach limited to 90° per tick (result wrapped into
  [0, 360]); speed = modifier × `movement_speed` (which also writes the forward input =
  speed); **jump** if (Δy > step height and horizontal distance² < max(1, width))
  or if the mob is inside the non-empty collision shape of its block (below its top), except
  doors and fences → goes to the jump state;
- *jump*: speed maintained; back to wait on the ground or in a liquid;
- *strafe* (bow shooting…): speed = modifier × attribute; forward/sideways inputs
  normalised to that speed; if the target cell is not `walkable` for the evaluator, we
  go back to pure forward movement; then wait;
- *wait*: forward input = 0.
Variants: flight (gravity off, `flying_speed` speed in the air, vertical input ± speed,
bounded pitch), gentle swimming (dolphin, axolotl, turtle…), own controls (slime: jumps;
ghast, phantom, vex, bee, blaze: direct flight; spider: climbing).

**Look**: the choice of the look (default speed 10°/tick
and pitch 40°, specific to the species) arms a 2-tick countdown. At the tick: pitch
reset to 0 (except species that keep it); if count > 0: −1, head yaw approaches the
target yaw (limited to the speed), pitch approaches; otherwise head yaw returns towards the body at
10°/tick. Then, if the navigation is running, head limited to ±75° (by default) from the body.

**Jump**: the requested "jump" flag is copied into "jump held" then cleared.

**Body** (in the end-of-tick rotation): if the mob has moved horizontally
(Δ² > 2.5000003E-7), body = yaw and head limited to the body; otherwise, without a mob passenger:
if the head has turned by > 15° since the last stable position, the body follows within the limit
of ±75°; otherwise after 10 stable ticks, the body catches up with the head over 10 ticks.

### 3.5 Navigation and pathfinding

**Navigation** (one per mob, depending on the medium: land, climbing, flying, aquatic,
amphibious):
- maximum path range = max(current `follow_range`, required length (16 by default));
  maximum number of nodes visited = floor(range × 16) × multiplier (1 by default);
- **create a path** to a set of positions with a reach r: refused below the
  world, or if the navigation cannot update (land: only on the ground, in
  a liquid or as a passenger). If the current path is not finished and already targets this target, it
  is reused. Search region: cube ±(range + 8) around the position (± 16 and one block
  higher for a target entity, except land which targets the entity's block with ±8);
- land: the target is first brought back to the **surface** (if air: we go down to the
  first non-air block +1, otherwise we go up; if solid: we go up to the first non-solid),
  except mobs allowed to target below the surface;
- **follow the path** (every tick): counter +1; deferred recomputation if requested (at most one
  every 20 ticks); tolerance at the node = width/2 if width > 0.75, otherwise
  0.75 − width/2; we move on to the next node if |Δx| and |Δz| (at the block centre) < tolerance and
  |Δy| < 1, or if we can cut the corner (node neither a door nor next to fire/danger, mob
  within < 2 blocks of the node and, either: direct movement possible, or closer to the next node,
  or within < √0.5 of the node, with opposite directions); then "move to" control towards
  the current node (position at the centre, Y of the block's floor, or the node's Y if the block below is
  air);
- **stuck detection**: every 100 ticks, if the mob has travelled less than
  (v ≥ 1 ? v : v²) × 100 × 0.25 blocks (v = current speed), it is "stuck" and the path
  is abandoned. Per node: maximum delay = distance to the node / v × 20; abandon if the time
  spent on that node exceeds 3 times that delay;
- **cleanup** of the path at the start: nodes inside a cauldron raised by one block; mobs
  that "avoid the sun" (zombies, skeletons by day): if the mob is in shade, the path
  is truncated at the first node under open sky;
- climbing (spiders): when the path is finished, continues straight towards the target position
  until within less than one width; flying: can cut towards any node in a direct line;
  aquatic: nodes in water only.

**A\* search**:
- nodes identified by (x, y, z) in a table specific to the search (created on demand);
- start: the mob's block, with Y = fluid surface if it floats, Y = floor(y + 0.5)
  on the ground, or first passable block below it if it is in the air; if that block is not
  valid (type `open` or penalty < 0), we try the 4 corners of its box;
- g(start) = 0, h = min over the targets of the Euclidean distance (in `f32`);
- loop: while the heap is not empty and the counter (incremented **before** the test)
  < max nodes: extract the node with minimal f (binary heap: on a tie, the heap order
  decides; sift-ups/downs only swap on "strictly less", and on
  sift-down we choose the left child only if it is strictly cheaper than the right);
  mark it closed; any target within Manhattan distance ≤ reach is "reached" → end;
  if the (Euclidean) distance from the start < range: neighbours; for each neighbour:
  d = distance(current, neighbour); walked distance(neighbour) = walked(current) + d (written even
  if the neighbour is not improved); g' = g(current) + d + penalty(neighbour); if walked < range
  and (neighbour not in the heap or g' < g): parent = current, g = g', h = min distance to the targets ×
  **1.5**, f = g + h (insertion or update in the heap);
- each computation of h updates the "best node" of each target (strictly smaller
  h);
- result: if targets are reached, path reconstructed towards the best node of
  each, we keep the shortest in number of nodes; otherwise partial path towards the best
  node, we keep the one whose final distance to its target is minimal, then the shortest.

**Land evaluator** (nodes):
- footprint: width = floor(box width + 1), height = floor(height + 1);
- type of a block (for a mob): types of each block of its footprint; closed wooden
  doors → "walkable door" if the mob can open doors and pass; open doors →
  blocked if it does not pass doors; rails → "unpassable rail" if the mob
  is not itself on a rail. A single type → that one; otherwise fence, then unpassable
  rail take priority; otherwise a type with penalty < 0 wins; otherwise the type with the highest
  penalty; correction for big mobs (> 1 block): `big_near_danger` if the
  central block is cheaper; small ones: `open` if the central block is open and the highest
  penalty is 0;
- static type of a block: air → `open`; trapdoors, lily pad, big dripleaf →
  `trapdoor`; powder snow → `powder_snow`; cactus, berry bush → `danger`;
  honey → `sticky_honey`; cocoa → `cocoa`; wither rose, dripstone →
  `danger_caution`; lava → `lava`; fire, lava, magma, lit campfire, lava cauldron
  → `fire`; doors → `open_door` / `closed_wood_door` (hand-openable) /
  `closed_iron_door`; rails → `rail`; leaves → `leaves`; fences, walls, closed fence
  gates → `fence`; not passable → `blocked`; water → `water`; otherwise `open`;
- an `open` block (above the minimum Y + 1) takes the type of what is below it: open,
  water, lava, walkable → `open`; fire → `fire`; dangerous → `danger`; honey → honey;
  powder snow → `on_powder_snow`; caution → `danger_caution`; trapdoor →
  `on_trapdoor`; otherwise `walkable`, transformed by the 24 neighbours (dx, dy, dz in
  [−1, 1], excluding the central column dx = dz = 0): danger → `danger_adjacent`, fire or lava →
  `fire_adjacent`, water → `water_edge`, caution → `danger_caution` (first found in
  the order dx, dy, dz from −1 to 1);
- **default penalties**: `blocked` −1, `open` 0, `walkable` 0, `walkable_door` 0, `trapdoor`
  0, `powder_snow` −1, `on_powder_snow` 0, `fence` −1, `lava` −1, `water` 8,
  `water_edge` 8, `rail` 0, `unpassable_rail` −1, `fire_adjacent` 8, `fire` 16,
  `danger_adjacent` 8, `danger` −1, `open_door` 0, `closed_wood_door` −1,
  `closed_iron_door` −1, `breach` 4, `leaves` −1, `sticky_honey` 8, `cocoa` 0,
  `danger_caution` 0, `on_trapdoor` 0, `big_near_danger` 4. Each mob
  can redefine a penalty (per-mob table, § 4: e.g. water −1 for undead
  other than the drowned, lava 0 for the strider, fire 0 for the fire-immune); a passenger inherits
  the penalties of a mount that allows it;
- **neighbours**: the 4 directions in the order **north, east, south, west**, then the 4 diagonals
  (north-east, east-south, south-west, west-north). Jump possible of floor(max(1, step
  height)) blocks if the block above the current one has a penalty ≥ 0 and the current one is not
  honey. For a neighbouring cell: refusal if the target floor is higher than the current floor by more than
  max(1.125, step height); node at the same level if penalty ≥ 0; if the current one is a
  fence or a closed door, the box must be able to slide into it without collision; if the type
  is not `walkable`: attempt to **climb** one block (if no valid node, jump
  possible, not a fence except for mobs that cross them, not an unpassable rail, trapdoor
  or powder snow; for a mob of width < 1 whose cell above is open or
  walkable, collision test of a box above the starting cell), or to
  **descend** into water (non-swimmers: down to the first non-water) or into the void (down to the
  first floor, refusal beyond the tolerated fall: 3 blocks, or more if the mob has a target:
  (HP − 33 % of max HP) − (3 − difficulty) × 4, at least 0, + 3);
  a diagonal is valid only if both orthogonal cells exist, are not higher,
  are not doors, have a penalty ≥ 0 (or are lower; or are two fences for a
  mob of width < 0.5), and for a mob of width > 1 if neither has a penalty > 0;
  the neighbouring node keeps the **maximum** of the penalties encountered;
- valid node: not closed and penalty ≥ 0 (or the current one itself has a penalty < 0);
- **swim** evaluators (6 directions in the order down, up, north, south, west, east, then
  horizontal diagonals if both orthogonals are free), **fly** (6 directions then
  12 edges and 8 corners, each if the orthogonal neighbours allow it), **amphibious**
  (land + up/down; +1 penalty for water more than 10 blocks below sea level
  for those that prefer the surface).

### 3.6 Line of sight and perception

- Line of sight: same world, eye→eye distance ≤ 128, then ray cast (collision
  shapes, fluids ignored) from the mob's eyes to the target's eyes (the target's X, Z).
  Per-tick cache (cleared at the start of the AI): each target is only tested once per
  tick.
- Mobs hear through **vibrations** (warden, sculk): see `block-entities.md` for
  the game event; the others have no sound perception.

### 3.7 Random positions (wander, flee)

- Random direction: x = `next_int(2h+1)` − h, y = `next_int(2v+1)` − v, z same (in this
  order); direction in a cone: angle = atan2(dz, dx) − π/2 + (2·`next_float()` − 1) ×
  half-angle, distance = lerp(√`next_double()`, min, max) × √2; refusal if |x| or |z| > max;
  y = `next_int(2v+1)` − v + flight height.
- Bias towards home: if the mob has a home and h > 1, we shift x and z by
  `next_double()` × h/2 towards the home centre.
- Validation (land): within the world limits, not outside the home if restricted,
  "stable" position for the navigation (block below solid as rendered), then move up out
  of the solid, refusal in water or on a non-zero penalty.
- **Always 10 draws**; we keep the position with the best "walk value" (§ 2.4,
  strictly better: the first wins on a tie); result at the bottom centre of the block.

### 3.8 Attack

- **Melee (goal)** (`MOVE`+`LOOK`, every tick): only evaluates "can start" once per
  20 game ticks; requires a living target and a path to it (or being already in range).
  Continues if the target lives, and either (mode "follows without seeing") target within the home and not a
  spectator/creative player, or navigation running. Tick: looks at the target (30°/30°); if
  (follows without seeing or line of sight) and recompute delay elapsed and (first time, or target
  moved by at least 1 block, or `next_float()` < 0.05): delay = 4 + `next_int(7)`, +10 beyond
  32 blocks, +5 beyond 16, +15 if the path fails; strikes if the attack delay (20
  ticks) has elapsed, the target is in range and visible. Stop: aggression cut off, target cleared if
  it is in creative/spectator.
- **Melee range**: the mob's box (extended to its mount) inflated horizontally by
  √2.04 − 0.6 ≈ 0.828 (or the weapon's range, `attack_range` component, with minimum range)
  that intersects the target's hit box.
- **Hit**: damage = `attack_damage` + enchantments + weapon bonus; if it connects: extra
  knockback (attribute `attack_knockback` + enchantments), weapon wear, post-attack
  effects (fire aspect…), "last mob hit". Mechanics of damage and of the knockback
  received: shared with the player (to be specified with combat).
- **Bow**: if the target exists and the mob holds a bow; sight counter (+1 in sight, −1
  otherwise, reset to 0 on change); in range (species radius, 15 for skeletons) and seen
  for ≥ 20 ticks: navigation stopped and strafing (direction drawn at 30 % every 20
  ticks; backs off within 25 % of the range², advances beyond 75 %); otherwise approaches. Shoots when
  the bow has been drawn ≥ 20 ticks (bow power according to time), minimum delay between two shots
  (skeletons: 20 on hard, 40 otherwise; re-evaluated when the weapon or the difficulty changes); lowers the bow if invisible
  for 60 ticks.
- **Crossbow**, **throws** (witch, snow golem), **fireballs** (ghast, blaze),
  **trident** (drowned), **spit** (llama): § 4.

### 3.9 Neutral mobs, anger, forgetting

- A neutral mob (wolf, bee, enderman, zombified piglin, iron golem…) keeps an
  **anger end date** (game time, NBT `anger_end_time`) and its anger target
  (`angry_at`). Duration drawn from a range specific to the mob (20–39 s for wolf, bee, enderman,
  zombified piglin, iron golem). Update: if the previous target (mob) is dead and
  was the anger target → end of anger; otherwise a new target becomes the anger target
  and restarts the timer (also restarted every tick with a target in the species that "stay
  angry as long as the target is there"); anger expired without a valid player target → end;
  target a creative or spectator player or peaceful difficulty → end. Rules `forgiveDeadPlayers`
  and `universalAnger` (anger against all players).
  `angry_at` is a UUID reference, player OR mob: on reload, the entity that
  carries that UUID (if loaded) becomes the target again; "angry at X" is tested by
  UUID equality. A golem angry at a zombie stays so after reloading; it does not
  turn against the player. The reference is written even if its entity is not loaded.
- Forgetting an attacker: 100 ticks (§ 1.2); forgetting an unseen target: § 3.2; brains:
  memory `angry_at` with a duration (600 ticks for piglins…).

### 3.10 Fleeing, wandering, food

- Fleeing: panic / avoid goals (§ 3.2); brains: `panic` activity (memory `is_panicking`
  set by a hit or a danger, duration), `avoid` (piglins facing soul fire, hoglins facing
  blue mushrooms).
- Wandering: § 3.2 and § 3.7; brains: wandering on the ground (±10/7), in flight, in water.
- Food: each species has its food (tag `#minecraft:*_food`); feeding a baby
  advances its age by floor((−age)/20 × 0.1) seconds (i.e. 10 % of the remaining time); feeding an
  adult of age 0 that can love puts it in love for 600 ticks (event 18, hearts); the golden
  dandelion freezes or releases a baby's age (delay 40 ticks). Eating grass (sheep): § 4.

### 3.11 Breeding

Love: 600 ticks, reset to 0 if age ≠ 0 or if the animal is hurt; every 10 ticks
of love, the animal draws 3 Gaussians and 3 random positions (heart particles with no effect
server-side but **random draws consumed**). Birth: baby of the same species (variant depending on
the species), age −24000, placed at the parent's position, statistic and
advancement trigger for the responsible player; parents: age 6000 (delay), love reset to 0;
experience orb of `next_int(7)` + 1 if the rule `doMobLoot` is true. Growth: +1 per
tick towards 0; an adult in delay decreases towards 0.

### 3.12 Taming

Tameable mobs (wolf, cat, parrot; horses, llamas, camels by riding;
fox by trust; axolotl and allay by bond): owner = UUID (`Owner`), sitting state
(`Sitting`/order), goals follow the owner, sit on command, defend the owner. Chance
to tame with each food: 1 in 3 for wolf and cat, 1 in 10 for the parrot (the entity's
random generator); success: owner set, navigation stopped, target cleared, sitting, event 7
(hearts); failure: event 6 (smoke). Untamed ridden equids: on each tick of the
"fidget" goal, with probability 1/`reduced delay(50)`: tamed if `next_int(max temper)`
< temper (max 100; 30 for the llama), otherwise the rider is thrown off and the temper
increases by 5; feeding also increases the temper (depending on the food).

---

## 4. Mobs one by one

### 4.1 Surface monsters and illagers

Source: decompiled 26.3, read on 2026-10-12. Written in our own words.

Default values (when an attribute is not listed): `max_health` 20, `movement_speed` 0.7, `follow_range` 32 (16 for any mob), `attack_damage` 2 (monsters), `armor` 0, `knockback_resistance` 0, `step_height` 0.6, `safe_fall_distance` 3, `flying_speed` 0.4, `spawn_reinforcements` 0.
Burning in sunlight: generic, driven by the tag `minecraft:burn_in_daylight` (skeleton, stray, wither_skeleton, bogged, zombie, zombie_horse, zombie_villager, drowned, zombie_nautilus, phantom). Rule: server-side, if the environment allows monsters to burn, "magic" brightness > 0.5, draw `rand*30 < (lum-0.4)*2`, not in water/rain/powder snow (current or previous tick), sky visible from eye height → if a helmet is present: the helmet loses 0 or 1 durability; otherwise fire for 8 s. Husk and parched are NOT in the tag.

#### Zombie family

##### `minecraft:zombie` (monster)
- Attributes: HP 20; speed 0.23; damage 3; follow range 35; armour 2; `spawn_reinforcements` base drawn at spawn in [0; 0.1[.
- AI: 1 break door (only if able, and on Hard); 2 spear use (speeds 1.0/1.0, range 10, 2); 3 zombie melee (×1.0, does NOT pursue without sight; arms raised); 4 trample turtle eggs (×1.0, vertical search 3, acceptance distance 1.14); 6 cross village (×1.0, at night only, distance 4, doors if able); 7 wander avoiding water ×1.0; 8 look at player 8; 8 look around.
  Targeting: 1 retaliation (also alerts zombified_piglin); 2 nearest player (sight required); 3 villager (incl. wandering trader, no sight requirement); 3 iron golem (sight); 5 baby turtle on land (1 chance in 10 per tick, sight).
- Particularities:
  - Baby: 5 % at spawn; speed +50 % (multiplier on the base); XP ×2.5. If baby: 5 % to ride an existing unmounted chicken within ±5/3/5, otherwise 5 % to spawn a jockey chicken.
  - Picks up loot: probability 0.55 × special difficulty multiplier (except on conversion). Does not pick up the glow ink sac; a mounted baby does not hold eggs.
  - Door breaker: probability 0.1 × special multiplier; the goal only acts on Hard.
  - Equipment: 5 % (Hard) / 1 % (otherwise) → hand: 1/6 iron sword, 1/6 iron spear, 4/6 iron shovel; + generic monster armour/enchantments. Halloween: 25 % carved pumpkin (10 % of those cases jack o'lantern) on the head if empty, never dropped.
  - Random attributes at spawn: knockback +[0;0.05[; follow ×(1 + r) if r = rand×1.5×mult > 1; "leader" with probability mult×0.05 → reinforcements +[0.5;0.75[, HP ×(1 + [1;4[) (full heal except on conversion/loading), door breaker forced.
  - Reinforcements (Hard only, if the game spawns monsters): on each injury, if rand < `spawn_reinforcements` and a known target (or the attacker): up to 50 position attempts at ±[7;40] on each axis (each axis ×{-1,0,1}), valid spawn rules, no player within 7, no collision or liquid; the reinforcement receives the target; the caller loses 0.05 chance (cumulative), the reinforcement too −0.05.
  - Empty hand + on fire: on impact, probability effective_difficulty×0.3 to ignite the target for 2×⌊difficulty⌋ s.
  - Kills a villager: Normal → 50 %, Hard → 100 % conversion into zombie_villager (villager data, gossip, offers, XP kept; event 1026). Easy/Peaceful: never.
  - Drowning: eyes underwater 600 cumulated ticks (reset to zero as soon as it gets out) → conversion started, 300 ticks later becomes `minecraft:drowned` (attributes redrawn). Event 1040.
  - Burns in sunlight (tag).

##### `minecraft:husk` (monster)
- Attributes: identical to the zombie.
- AI: identical to the zombie.
- Particularities: does not burn in sunlight. Empty hand: on impact, hunger 140×⌊effective_difficulty⌋ ticks. Drowning 600+300 ticks → becomes `minecraft:zombie` (event 1041). Natural spawn: if the space of a `camel_husk` is free, 10 % → husk receives an iron spear, rides a camel_husk, and a `minecraft:parched` rides behind. Baby, chicken jockey, reinforcements, leader: inherited from the zombie.

##### `minecraft:drowned` (monster; own spawn rule)
- Attributes: zombie + `step_height` 1.0. Amphibious navigation, water path cost = 0.
- AI: (inherits 4 turtle eggs, 8 look at player 8, 8 look around); 1 go to water (by DAY only, out of the water: 10 attempts at a water block within ±10 horizontal, +2..−5 vertical); 2 ranged trident (×1.0, interval 40 ticks, radius 10; only if a trident is in hand); 2 zombie melee (×1.0, no pursuit out of sight; only if a valid target); 5 go to the beach (at night, in water, Y ≥ sea level −3; passable block with 2 of air above, radius 8, vertical 2); 6 swim up (at night, in water, Y < sea −2: aims at sea −1 in steps of 4/8; sets the "seeks land" mark); 7 wander ×1.0 (without avoiding water).
  Targeting: 1 retaliation (ignores other drowned as a cause; alerts zombified_piglin); 2 player (1/10, sight) if valid target; 3 villager (no sight); 3 iron golem (sight); 3 axolotl (sight, without "nearby" visibility requirement); 5 baby turtle on land (1/10, sight).
  Valid target: at night always; by day only if the target is in water.
- Particularities:
  - Equipment: 10 % (rand > 0.9) → 10/16 trident, 6/16 fishing rod (NO generic armour/enchantment, replaces the zombie's). 3 % nautilus shell in the off hand, always dropped, never replaced. Does not pick up spears.
  - Trident: projectile shot at the target (Y aim at 1/3 of its height + horizontal distance ×0.2), power 1.6, inaccuracy 14 − 4×difficulty_id.
  - Mount: natural or structure spawn, trident in hand, adult, biome not in `more_frequent_drowned_spawns` → 50 % rides a `zombie_nautilus` (persistent if structure).
  - Swimming: is not pushed by the current when swimming; wants to swim if "seeks land" or target in water; then when immersed: acceleration 0.01, damping 0.9; aquatic move control: +0.002 vertical if target higher or seeks land, speed smoothed 0.125 towards speed×attribute, push (x×0.005, y×0.1, z×0.005); when not swimming: −0.008/tick vertical in the air.
  - Spawn: block under the feet = water (except generator); biome `more_frequent_drowned_spawns` (rivers) 1/15, otherwise 1/40 and Y < sea −5; monster brightness.
  - Does not convert in water. Burns in sunlight (tag). Inherits reinforcements/baby/leader/villager conversion from the zombie (jockey possible as a baby).

##### `minecraft:zombie_villager` (monster)
- Attributes: identical to the zombie. AI: identical to the zombie.
- Particularities:
  - Villager data (type according to biome, random profession by default), gossip, offers, XP kept.
  - Does not convert into drowned. Burns in sunlight. Not despawned if being cured or villager XP > 0.
  - Curing: click with a golden apple (consumed) IF the weakness effect is active → duration 3600 + [0;2400] ticks; removes weakness; strength (amplifier min(difficulty_id−1, 0), hence 0) for the whole duration; entity event 16 (curing sound). Without weakness: apple not consumed.
  - Progress per tick: 1; with a 1 % chance per tick, sweeps a cube [−4;+4[ around, up to 14 blocks of the tag `speeds_up_zombie_villager_curing` (iron bars, beds) and adds +1 per block with 30 %.
  - End: becomes `minecraft:villager`; worn equipment kept (except items with a curse of vanishing, dropped) in the villager's inventory; nausea 200 ticks; event 1027; if the initiating player is online: advancement criterion and "cured villager" reputation event.
  - Reverse conversion (villager → zombie_villager): when any zombie (zombie, husk, drowned, zombie_villager) kills a villager; Normal 50 %, Hard 100 %; the new one appears as a "conversion" with no forced baby but jockey allowed, receives data/gossip/offers/XP.

##### `minecraft:giant` (monster, never natural)
- Attributes: HP 100; speed 0.5; damage 50; `camera_distance` 16.
- AI: NO goal (neither movement nor targeting). Walk preference: favours dark positions (cost according to light).
- Particularities: none; does not burn (outside the tag).

#### Skeleton family (common base)

Common base skeleton/stray/bogged/parched:
- Attributes: HP 20; speed 0.25; damage 2; follow range 16.
- AI: 2 stay in the shade (avoids the sun in path choice); 3 flee the sun ×1.0 (if on fire/in the sun, looks for shelter); 3 flee wolves (radius 6, ×1.0 walking, ×1.2 close); 4 weapon: if bow in hand → bow shooting (×1.0, radius 15, minimum interval see below) otherwise melee (×1.2, no pursuit out of sight) — goal re-evaluated at every equipment change and on loading; 5 wander avoiding water ×1.0; 6 look at player 8; 6 look around.
  Targeting: 1 retaliation (no call for help); 2 nearest player (sight); 3 iron golem (sight); 3 baby turtle on land (1/10, sight).
- Bow shooting: sight counter (+1/tick visible, −1 otherwise, reset to 0 on change); at ≤ 15 and seen for ≥ 20 ticks → stops the navigation and circles (strafe 0.5 forward/backward, 0.5 sideways); every 20 strafe ticks: 30 % reverse the rotation direction, 30 % reverse forward/backward; backs off if dist² < 25 % of radius², advances if > 75 %; otherwise walks towards the target. Draw: releases after ≥ 20 ticks of drawing (if in sight), power according to time (like the player's bow); gives up if out of sight for 60 ticks. Delay between shots: 20 ticks (Hard) / 40 (others); bogged and parched: 50 / 70.
- Arrow: Y aim at 1/3 of the target height + horiz_dist×0.2; speed 1.6; inaccuracy 14 − 4×difficulty_id. Projectile taken from the inventory / bow (effect arrows possible through enchantment/item).
- Equipment: bow in hand always + generic monster armour/enchantments; picks up loot with 0.55×special mult; does not pick up spears; Halloween 25 % pumpkin (10 % lantern).
- "Shivers" when fully frozen (for skeleton: during the conversion into stray).

##### `minecraft:skeleton` (monster)
- Attributes/AI: common base.
- Particularities: burns in sunlight. Immune to freezing (does not freeze). Conversion into stray: in powder snow 140 ticks in a row (leaving resets to zero) → conversion started, 300 ticks later becomes `minecraft:stray` (event 1048).

##### `minecraft:stray` (monster; spawn rule: standard monster + sky visible above the powder snow column, except generator)
- Attributes/AI: common base.
- Particularities: arrows with slowness 600 ticks. Burns in sunlight.

##### `minecraft:bogged` (monster)
- Attributes: HP 16; rest common base.
- AI: common base; shot delay 50 (Hard) / 70.
- Particularities: arrows with poison 100 ticks. Shearable (shears, once; durability −1; drops the `bogged_shear` table (mushrooms) at the height of the box; "sheared" state saved). Burns in sunlight.

##### `minecraft:parched` (monster; desert)
- Attributes: HP 16; rest common base.
- AI: common base; shot delay 50 / 70.
- Particularities: arrows with weakness 600 ticks; immune to the weakness effect. Does NOT burn in sunlight (outside the tag). Can spawn at the back of a camel_husk ridden by a husk (see husk).

#### Spiders

##### `minecraft:spider` (monster)
- Attributes: HP 16; speed 0.3; damage 2; follow range 16.
- AI: 1 float; 2 flee non-scared armadillos (radius 6, ×1.0/×1.2); 3 leap at the target (vertical impulse 0.4; only on the ground, target at distance 2..4 blocks, ~1 chance in 5 ticks; horizontal impulse = direction×0.4 + current speed×0.2); 4 melee (×1.0, PURSUES even without sight; inactive if the spider carries a passenger; in light ≥ 0.5, 1 % per tick to give up the target); 5 wander avoiding water ×0.8; 6 look at player 8; 6 look around.
  Targeting: 1 retaliation; 2 player (sight) only if brightness < 0.5; 3 iron golem (sight) same condition. → neutral by day/in light unless hit.
- Particularities:
  - Climbs: "climbing" flag = horizontal collision this tick (computed server-side); wall-climbing navigation; considered on a ladder when climbing.
  - Does not stay stuck in cobwebs. Immune to poison.
  - Jockey: 1 % at spawn → skeleton (bow, normal equipment) mounted on it.
  - Effects: on Hard, probability 0.1×special mult → permanent effect: 2/5 speed, 1/5 strength, 1/5 regeneration, 1/5 invisibility (shared by the spawn group).

##### `minecraft:cave_spider` (monster; mineshaft spawners)
- Attributes: HP 12; rest like spider.
- AI: identical to spider.
- Particularities: on impact, poison (level I) 7 s on Normal, 15 s on Hard, nothing on Easy. NO skeleton jockey nor random effect at spawn. Climbing, immunities: like spider.

#### `minecraft:creeper` (monster)
- Attributes: HP 20; speed 0.25; damage 2 (unused); follow range 16.
- AI: 1 float; 2 swell (active if already swelling OR living target within < 3 blocks; stops the navigation; every tick: target > 7 blocks or out of sight → deflates, otherwise swells); 3 flee ocelots (6, ×1.0/×1.2); 3 flee cats (6, ×1.0/×1.2); 4 melee ×1.0 no pursuit out of sight (the attack itself does nothing, it is used to approach); 5 wander avoiding water ×0.8; 6 look at player 8; 6 look around.
  Targeting: 1 nearest player (sight); 2 retaliation. Refuses any goat as a target.
- Particularities:
  - Fuse: counter 0..`Fuse` (30 ticks by default, saved); direction ±1 synchronised; going from 0 to 1 → priming sound + game event "prime_fuse". Ignited (flint and steel, fireball, tag `creeper_igniters`: consumed or worn by 1) → swells with no return until the explosion ("ignited" saved).
  - Fall: adds ⌊distance×1.5⌋ to the counter, capped at Fuse − 5.
  - Explosion: radius `ExplosionRadius` (3) ×2 if charged, "mob" interaction (breaks blocks if mob_griefing); then the creeper disappears (counted as killed for death effects). If it had effects: lingering cloud radius 2.5 (−0.5 per use, wait 10, duration 300, shrinks linearly, effect duration ×0.25).
  - Charged: struck by lightning → "powered" (saved). A charged creeper whose explosion kills a mob makes it drop the `charged_creeper` table (heads) only once.
  - Gunpowder: through the mob's loot table (and disc if killed by a skeleton, also through the table).

#### Endermen and parasites

##### `minecraft:enderman` (monster)
- Attributes: HP 40; speed 0.3 (+0.15 additive while it has a target); damage 7; follow range 64; `step_height` 1.0. Water path cost −1 (impassable).
- AI: 0 float; 1 freeze when stared at (target = player within ≤ 16 who stares at it: stops the navigation, looks at them; blocks movement and jump); 2 melee ×1.0 no pursuit out of sight; 7 wander avoiding water ×1.0 (probability 0 of choosing off dry ground); 8 look at player 8; 8 look around; 10 place the carried block; 11 take a block.
  Targeting: 1 look for the provoking player (1/10, see below); 2 retaliation; 3 endermite (sight, without "nearby" requirement); 4 reset universal anger (no alert).
- Particularities:
  - Gaze: a player "stares" at it if they are not wearing a disguise item (carved pumpkin / equivalent component on the head), dot product gaze·direction > 1 − 0.025/distance, towards the enderman's eye height, line of sight through collisions (transparent blocks block). Provocation: stare OR persistent anger against them, and not an (indirect) passenger of the enderman. Delay before engaging: ~5 ticks (adjusted); it looks at them during this time; synchronised "stared at" flag ("stare" sound client-side, minimum 400 ticks between two).
  - Once the target is fixed: if the player still stares at it and is within < 4 blocks → random teleport; if target > 16 blocks and does not stare at it, every ~30 ticks → teleport towards the target. The pursuit then continues without a sight requirement.
  - Persistent anger (universal/neutral): 20 to 39 s, saved.
  - "Creepy" flag (open mouth, scream) = has a target.
  - Random teleport: X,Z ±32 (uniform), integer Y ±32; towards a target: 16 blocks in the direction of the target ±4 horizontal, Y ±8; attempt via the standard ground search; refuses if the arrival block has water below, tag `enderman_does_not_teleport_to` excluded, or if it is a passenger. Game event "teleport", level event 2018 (particles between old and new position).
  - Day: if day, target changed for ≥ 600 ticks, brightness > 0.5, sky visible and draw `rand*30 < (lum−0.4)*2` → loses its target and teleports.
  - Damage: projectile (non-passenger) → no damage, up to 64 teleport attempts. Thrown potion → only if the potion is of the tag `hurts_water_sensitive_entities` (water), then 64 attempts. Damage without a living attacker → 90 % teleport.
  - Water: water-sensitive → 1 drowning damage per tick in water or in rain.
  - Block taken: if mob_griefing, ~1/20 per tick (adjusted): random position within ±2 horizontal, 0..3 vertical, block of the tag `enderman_holdable`, reachable by ray cast from the centre of its block → removes the block (default state carried), event "block_destroy".
  - Block placed: ~1/2000 per tick (adjusted) if mob_griefing: position ±1 horizontal, 0..2 vertical, air, solid below (not air, not bedrock), the block can survive there, no entity in the cube → places (shapes updated), event "block_place".
  - Carried block saved; carrying enderman not despawnable; on death, drops the block as if mined by an enchanted diamond axe (provider `enderman_loot_drop`, silk touch).

##### `minecraft:endermite` (monster)
- Attributes: HP 8; speed 0.25; damage 2; follow range 16. XP 3.
- AI: 1 float; 1 climb on powder snow; 2 melee ×1.0 no pursuit out of sight; 3 wander avoiding water ×1.0; 7 look at player 8; 8 look around.
  Targeting: 1 retaliation (alerts all nearby endermites); 2 player (sight).
- Particularities: lifetime 2400 ticks (`Lifetime` counter saved, does not advance if persistent) then disappears. Body always aligned with the head. Emits step events (no walking sound). Spawn: "monster in any light" rules, refused if a player is within ≤ 5 blocks (except generator). Preferred target of endermen (see enderman). Also born from ender pearls (outside this file).

##### `minecraft:silverfish` (monster; infested blocks)
- Attributes: HP 8; speed 0.25; damage 1; follow range 16.
- AI: 1 float; 1 climb on powder snow; 3 wake up friends; 4 melee ×1.0 no pursuit out of sight; 5 wander ×1.0 (interval 10) with merging into stone.
  Targeting: 1 retaliation (alerts all); 2 player (sight).
- Particularities:
  - Wake-up: when it takes damage from an entity (or of a type of the tag `always_triggers_silverfish`, e.g. poison/magic), arms a ~20-tick counter; when due, walks Y 0,+1,−1…±5 and X/Z ±10 (from the inside outwards): each infested block is destroyed (with drop → releases its silverfish) if mob_griefing, otherwise turned back into a host block; after each block found, 50 % to stop.
  - Merging: without target, navigation finished, mob_griefing, ~1/10 tick (adjusted): random direction from Y+0.5; if the neighbouring block is a compatible host (stone, stone bricks, cobblestone, deepslate…) → becomes its infested version, the silverfish disappears (with spawn animation).
  - Prefers to walk on compatible hosts (value 10). Body aligned with the head. Spawn: refused if a player is within ≤ 5 (except generator).

#### `minecraft:slime` (monster)
- Attributes (depend on size T, 1..127, synchronised, saved as `Size` = T−1): HP T²; speed 0.2 + 0.1×T; damage T; XP T; follow range 16.
- AI ("cube" base, shared with magma_cube): 1 float (in liquid: 80 % to jump every tick, impulse ×1.2); 2 attack (with an attackable target: looks at the target, advances towards it in aggressive mode; tires after ~300 ticks); 4 random direction (without target, on ground/liquid/levitation: new angle 0..359 every 40..99 adjusted ticks); 5 keep jumping (if not a passenger, ×1.0).
  Targeting: 1 player (1/10, sight) only if |ΔY| ≤ 4; 3 iron golem (sight).
- Movement: by jumps only; on the ground, delay between jumps 10..29 ticks, divided by 3 in aggressive mode; no movement between jumps; in the air, speed = mult×attribute.
- Damage: on contact (player who touches it, or pushed iron golem), if size > 1, AI active, compatible teams, within melee range and in sight → damage T. Size-1 slimes do not hurt.
- Spawn sizes: exponent e ∈ {0,1,2}; if e < 2, probability 0.5×special mult to add 1 → size 1, 2 or 4. Never a baby.
- Splitting: on death (not on a simple removal), if T > 1 → 2 to 4 small ones of size ⌊T/2⌋ (full HP), offset by ±width/4 in X/Z, +0.5 in Y, random orientation, same team, conversion "split_on_death".
- Spawn: not in Peaceful; generator → standard mob rules; surface: biome tag `allows_surface_slime_spawns` (swamp), 50 < Y < 70, draw < environment chance `surface_slime_spawn_chance` (depends on the moon) and max light ≤ rand(0..7); slime chunk (world seed, salt 987234911, 1/10 of chunks): 1/10 and Y < 40.

#### Pillagers (common "patroller" + "raider" base)

Tags: `raiders` = evoker, pillager, ravager, vindicator, illusioner, witch; `illager` = evoker, illusioner, pillager, vindicator.
- Patroller: 4 long-distance patrol (×0.7 for followers, ×0.595 for the leader; patrol target = ±500 in X/Z; leader arrived within < 10 → new target; followers follow the leader; navigation failure → pause 200 ticks). Patrol leader: 6 % at spawn (outside patrol/event/structure, if the mob can be a leader) → pillager banner on the head, always dropped (probability 2.0). Spawned by a patrol → "on patrol"; on patrol, despawns only beyond 128 blocks. Natural spawn of patrollers: block light ≤ 8.
- Raid pillager: 1 recover the raid leader's banner (if a raid has no captain, picks up the banner item within the follow radius ×8 vertical, pickup at 1.414); 3 go to the raid; 4 cross the village ×1.05 (POI distance 1); 5 celebrate (after a raid victory: sound ~1/100, jump ~1/50). Every 20 ticks without a raid: joins an active raid at its position. Idle counter ×2 during a raid; reset to 0 if it targets a player or golem. The detail of the raid mechanics is outside this sheet.

##### `minecraft:witch` (monster; also raider)
- Attributes: HP 26; speed 0.25; damage 2; follow range 16.
- AI: raider base (1 banner, 3 raid, 4 village, 5 celebrate) + patrol base (4); 1 float; 2 ranged attack (×1.0, interval 60 ticks, radius 10); 2 wander avoiding water ×1.0; 3 look at player 8; 3 look around.
  Targeting: 1 retaliation (ignores raiders as a cause); 2 heal a raider (during a raid, non-witch raider, sight; cooldown 200 ticks); 3 player (1/10, sight) — disabled while the healing cooldown runs.
- Particularities:
  - Drinking (if not already drinking; draws in order): 15 % water breathing if eyes underwater and without the effect; otherwise 15 % fire resistance if on fire or last damage source of fire type, without the effect; otherwise 5 % instant health if HP < max; otherwise 50 % speed if target at > 11 blocks and without speed. Potion put in hand, duration = use duration of the item (32 ticks), speed −0.25 additive while drinking; at the end: effects applied, hand emptied, event "drink".
  - Throwing (not while drinking): aim anticipating the target's movement (pos + velocity), Y = target eyes −1.1, +dist×0.2; power 0.75 (0.45 if ≤ 2 blocks); fixed inaccuracy 8. Choice: raider target → instant health if HP ≤ 4 otherwise regeneration, then forgets the target; otherwise slowness if dist ≥ 8 and without slowness; otherwise poison if target HP ≥ 8 and without poison; otherwise weakness if dist ≤ 3, without weakness, 25 %; otherwise instant damage.
  - Defence: 0 damage from its own potions; damage of the tag `witch_resistant_to` (magic, indirect_magic, sonic_boom, thorns) ×0.15.
  - "Witch" particles: 0.075 % per tick (entity event 15).
  - Cannot be a patrol leader; no raid bonus. Also born from a villager struck by lightning (outside the sheet).

"Hold position" (common goal pillager/vindicator/evoker…): outside a raid, on patrol, with a target, not yet aggressive, and last attacker a non-player → stops, passes its target to all raiders within ≤ 8 (without sight); target > 10 blocks: looks at it (shout ~1/50); ≤ 10 → becomes aggressive; at the end, all raiders within ≤ 8 become aggressive towards the target.

##### `minecraft:pillager` (monster; patrols, outposts)
- Attributes: HP 24; speed 0.35; damage 5; follow range 32.
- AI: raider/patrol base; 0 float; 1 flee the creaking (8, ×1.0/×1.2); 2 hold position (10); 3 crossbow (×1.0, radius 8); 8 wander ×0.6; 9 look at player 15 (probability 1.0); 10 look at mob 15.
  Targeting: 1 retaliation (ignores raiders, alerts all); 2 player (sight); 3 villager (no sight); 3 iron golem (sight).
- Crossbow: sight counter like the bow; must move if dist > 8 or sight < 5 ticks and no delay in progress (repath every 1–2 s; speed ×0.5 if the crossbow is not empty); stopped: draws the crossbow (synchronised "charge" flag) up to the weapon's charge duration (25 ticks without enchantment), then waits 20..39 ticks, then fires if in sight (speed 1.6, like the player).
- Particularities: crossbow in hand always; 1/300 special enchantment (provider `pillager_spawn_crossbow`) on top of the generic one. 5-slot inventory (saved, slots 300+); always picks up loot after loading; in a raid wants the white banner (in the inventory) and banners. Walk preference null (0) everywhere. Max spawn group 1. Raid bonus: wave > Normal's groups → enchanted crossbow "post-wave 5", > Easy → "post-wave 3" (according to the raid's enchantment probability).

##### `minecraft:vindicator` (monster; mansions, raids)
- Attributes: HP 24; speed 0.35; damage 5; follow range 12.
- AI: raider/patrol base; 0 float; 1 flee the creaking (8, ×1.0/×1.2); 2 break doors (in a raid only, Normal/Hard, ~1/10 tick adjusted, break duration 6; resets idle to 0); 3 open doors (raid); 4 hold position (10); 5 melee ×1.0 no pursuit out of sight; 8 wander ×0.6; 9 look at player 3 (probability 1.0); 10 look at mob 8.
  Targeting: 1 retaliation (ignores raiders, alerts all); 2 player (sight); 3 villager (sight); 3 iron golem (sight); 4 "Johnny": any attackable living entity (sight), only if named.
- Particularities: iron axe in hand outside a raid; in a raid: iron axe, enchanted according to the raid probability (post-wave 5 provider if wave > Normal's groups). Can open doors if its position is in a village under raid (re-evaluated every AI tick). Name "Johnny" (label) → `Johnny` flag saved, attacks everything except… ("attackable" filter only; the other illagers are allies through the team/alliance, to be checked). Arm pose: attacks if aggressive, celebration, otherwise crossed.

"Spellcaster" base (evoker, illusioner): current spell synchronised (none, summon_vex, fangs, wololo, disappear, blindness); "cast" goal (priority 1) active as long as the casting counter > 0: stops the navigation, looks at the target; the counter decreases every AI tick. A spell: usable if living target, no other spell in progress, and time ≥ next slot; at start: preparation (20 ticks by default, adjusted), casting duration set, next slot = now + interval; the spell acts when the preparation drops to 0.

##### `minecraft:evoker` (monster; mansions, raids)
- Attributes: HP 24; speed 0.5; damage 2; follow range 12. XP 10.
- AI: raider/patrol base; 0 float; 1 cast; 2 flee players (8, ×0.6/×1.0); 3 flee the creaking (8, ×0.6/×1.0); 4 summon vexes; 5 fangs; 6 "wololo"; 8 wander ×0.6; 9 look at player 3 (1.0); 10 look at mob 8.
  Targeting: 1 retaliation (ignores raiders, alerts all); 2 player (sight; out-of-sight memory 300 ticks); 3 villager (no sight, 300); 3 iron golem (no sight).
- Spells:
  - Vex: extra condition rand(1..8) > number of vexes within ≤ 16; casting 100, interval 340; spawns 3 vexes at ±2 in X/Z, +1 in Y around it; owner = the evoker, bound origin = this point, limited life 20×(30..119) ticks; same team.
  - Fangs: casting 40, interval 100. Target within < 3 blocks: 5 fangs in a circle of radius 1.5 (step 0.4π) without delay + 8 fangs in a circle of radius 2.5 (offset by 2π/5) with delay 3. Otherwise: line of 16 fangs towards the target, spaced 1.25 apart, delay i ticks. Each fang placed on the first block with a solid top face going down from max(Y)+1 to min(Y)−1 (height adjusted to the top of the collision shape); otherwise no fang.
  - Fang (`minecraft:evoker_fangs`): after its delay, at −8 ticks deals 6 indirect magic damage to living entities in its box enlarged by 0.2 (except owner and allies); lives 22 ticks; event 4 client-side.
  - Wololo: no target, mob_griefing; a BLUE sheep within ≤ 16 (vertical 4) at random; preparation 40, casting 60, interval 140 → sheep turns RED.
- Particularities: considers as allies the vexes whose ownership root is itself or an ally. No raid bonus. Drops the totem of undying (loot table).

##### `minecraft:illusioner` (monster; command only)
- Attributes: HP 32; speed 0.5; damage 2; follow range 18. XP 5.
- AI: raider/patrol base; 0 float; 1 cast; 3 flee the creaking (8, ×1.0/×1.2); 4 mirror spell; 5 blindness spell; 6 bow (×0.5, interval 20, radius 15); 8 wander ×0.6; 9 look at player 3; 10 look at mob 8.
  Targeting: 1 retaliation (ignores raiders, alerts); 2 player (sight, memory 300); 3 villager (no sight, 300); 3 golem (no sight, 300).
- Spells: mirror (if not invisible): casting 20, interval 340 → invisibility 1200 ticks on itself. Blindness: only on a NEW target (id different from the last) and if the effective local difficulty exceeds Normal's; casting 20, interval 180 → blindness 400 ticks.
- Particularities: bow in hand at spawn; shooting like the skeleton (speed 1.6, inaccuracy 14−4×diff_id). Illusions (4 copies offset ±3) purely client-side, triggered on injury or every 1200 ticks.

##### `minecraft:vex` (monster; summoned)
- Attributes: HP 14; damage 4; speed 0.7 (default); `flying_speed` default. XP 3.
- AI: 0 float; 4 charge (living target, no movement wanted in progress, ~1/7 tick adjusted, target at > 2 blocks: aims at the target's eyes ×1.0, "charge" flag; if boxes overlap → strikes, end of charge; if < 3 blocks, readjusts the aim); 8 random movement (~1/7: 3 air attempts within ±7 / ±5 vertical / ±7 around the bound origin (or its position), speed ×0.25); 9 look at player 3; 10 look at mob 8.
  Targeting: 1 retaliation (ignores raiders, alerts); 2 copy the owner's target (no sight); 3 player (sight).
- Particularities: passes through blocks (no collision during its tick); no gravity; flight control: accelerates by Δ×(mult×0.05/|Δ|) towards the point, arrived when |Δ| < box size (speed ×0.5). Limited life: on expiry, 1 hunger damage every 20 ticks. Iron sword in hand, never dropped (+ generic enchantments). Remaining life saved (`life_ticks`), bound origin saved.

##### `minecraft:ravager` (monster; raids, patrols)
- Attributes: HP 100; speed 0.3 (base rewritten every tick, see below); damage 12; `attack_knockback` 1.5; knockback 0.75; follow range 32; `step_height` 1.0. XP 20. Leaves path cost = 0.
- AI: raider/patrol base; 0 float; 4 melee ×1.0 PURSUES without sight; 5 wander avoiding water ×0.4; 6 look at player 6; 10 look at mob 8.
  Targeting: 2 retaliation (ignores raiders, alerts); 3 player (sight); 4 adult villager (sight); 4 iron golem (sight).
- Particularities:
  - Speed: immobile (attack/stunned/roar) → base 0; otherwise base smoothed (10 %/tick) towards 0.35 with a target, 0.3 without.
  - Ridden by a non-raider mob → its movement/look/jump/targeting goals are cut (controlled by the rider); in a boat, no jump.
  - Horizontal collision + mob_griefing → breaks (with drop) the leaves in its box +0.2; if it broke nothing and is on the ground, jumps.
  - Attack: blocks for 10 ticks (event 4); attack box reduced by 0.05 in X/Z.
  - Shield: if its hit is blocked and not roaring: 50 % stunned 40 ticks (event 39, the defender pushes it back); otherwise big knockback on the defender. At the end of the stun → roar: 20 ticks, at 10 remaining: all living entities (except ravagers, except armour-wearers without mob_griefing) in its box +4 → 6 damage (except illagers), strong knockback for non-players (client-side for players, event 69); strong knockback = push (dx/d²×4, 0.2, dz/d²×4) with d² ≥ 0.001.
  - Sees nothing during the stun/roar. No raid bonus; never a leader. Refuses to spawn in a liquid. Typically ridden by a pillager/vindicator/evoker in a raid.

#### `minecraft:phantom` (monster; spawned by insomnia, special generator outside the sheet)
- Attributes: HP 20; damage 6 + size (size 0..64, default 0, saved `size`); follow range 16; speed unused (own flight). XP 5. Dimensions ×(1 + 0.15×size).
- AI: 1 attack strategy (if attackable target); 2 swoop; 3 circle around the anchor (without target or in "circle" phase).
  Targeting: 1 player: sweep every ~60 ticks (first after ~20) of the attackable players within ≤ 64, in the box ±16 / ±64 vertical, sorted from HIGHEST to lowest; keeps the target as long as it remains attackable.
- Mechanics:
  - Anchor: at spawn, position +5 in Y (saved `anchor_pos`). Strategy at start: circle phase, next swoop in ~10 ticks, anchor = target +20..39 in Y (at least sea level +1). While circling, countdown → goes into SWOOP, anchor replaced above the target, next swoop in (8..11)×20 ticks. On stop (no more target): anchor = surface (blocking height) +10..29.
  - Circle: radius 5..15, height −4..+5 (relative to anchor −4), random direction; advances 15° at each point reached; ~1/350 new height; ~1/250 radius +1 (beyond 15 → 5 and direction reversed); ~1/450 random angle; avoids ground/ceiling (height ≥ 1 if block below, ≤ −1 if block above).
  - Swoop: aims at the target's centre; contact (box +0.2) → strikes, back to circle (event 1039); horizontal collision or recently hurt → back to circle. Gives up (and loses the target) if target dead, spectator/creative, or if a living cat is within ≤ 16 (checked every 20 ticks; the cats hiss).
  - Flight: movement without gravity; internal speed starts at 0.1, tends towards 1.8 when aligned (step 0.005×1.8/v), otherwise towards 0.2 (step 0.025); yaw rotation max 4°/tick; speed = 20 % interpolation towards the wanted speed; on collision, U-turn (180°) and speed 0.1.
- Particularities: burns in sunlight (tag); undead.

#### `minecraft:creaking` (monster; born from the creaking heart, or egg)
- Attributes: HP 1; speed 0.4; damage 3; follow range 32; `step_height` 1.0625. XP 0. Walk preference null. Can float.
- Brain: sensors `nearest_living_entities`, `nearest_players`. Key memories: `nearest_players`, `nearest_visible_attackable_player(s)`, `attack_target`, `walk_target`, `look_target`.
  - CORE: swim (0.8, only if it can move); look at the target (45..90 ticks); follow the walk target.
  - IDLE (prio 10): start attacking (only if "active") the nearest visible attackable player; look at an entity from time to time (8 blocks, every 30..60); one of: wander ×0.3 (weight 2), walk towards the look target ×0.3 to 3 (weight 2), nothing 30..60 ticks (weight 1).
  - FIGHT (if `attack_target`): approach ×1.0 if out of range; melee every 40 ticks only if it can move; gives up if the target is no longer in the list of visible attackable players.
  - Activity: frozen → default activity (core/idle); otherwise FIGHT then IDLE.
- Gaze / freezing (every server tick, synchronised flag `can_move`): no nearby player → can move (and deactivates if it was active). For each nearby attackable, non-allied player: if they look at it (cone 0.5 not weighted by distance, line of sight through transparent blocks, towards eyes / bottom+0.5×scale / middle) — for an already active creaking, only if the player is not wearing a disguise (pumpkin) — then: active → FROZEN; inactive and player within < 12 blocks → activates (target = this player) and frozen. No attackable player → deactivates. Frozen: dead stop, no pushing, no knockback, no attack, no swimming. Transitions → "entity_action" event.
- Bound to the heart (`home_pos` saved; "transient"):
  - Path penalty 8 on damage/powder snow/lava, 0 on fire; navigation: any point beyond 32 blocks (dist² > 1024) from the heart and farther than the current position is blocked. Immune to fire; does not take portals.
  - Invulnerable: any damage (outside `bypasses_invulnerability`) only plays the 8-tick animation (event 66); ignored if it comes neither from a living entity, nor a projectile, nor a responsible player; if a player is responsible, the heart reacts: particles, and if it is awake 2..3 resin clumps placed on neighbouring pale oak logs (walk ≤ depth 2, ≤ 64), cooldown 100 ticks.
  - Dies (HP 0) every tick if the heart no longer recognises it as a protector.
  - Removal by the heart: end of night (environment attribute `creaking_active` false, if it is not persistent), creaking beyond 34 blocks from the heart, or player whose eyes are in its box > 4 checks in a row → disappearance (particles, sound). Heart broken by a player or an explosion → "shaken" death 45 ticks (flag `is_tearing_down`) then disappearance, without equipment loot.
- Heart (block entity, summary): check every 20..24 ticks; awake/dormant state according to `creaking_active`; uprooted without the required logs; awake, monsters allowed, not Peaceful, player within ≤ 32 → 5 spawn attempts within ±16 / ±8 on a collider (not leaves); entity event 60.
- Unbound (egg): normally vulnerable (1 HP).

#### `minecraft:breeze` (monster; trial chambers, trial spawner)
- Attributes: HP 30; speed 0.63; damage 3; follow range 24. XP 10.
- Tags: `deflects_projectiles`, `fall_damage_immune`, `no_anger_from_wind_charge`, `can_turn_in_boats`. Reflects (reverses) any projectile except wind charges ("deflect" sound). Invulnerable to damage coming from another breeze. Fluid jump threshold = eye height. Only attacks players and iron golems. Emits step events.
- Brain: sensors `nearest_living_entities`, `hurt_by`, `nearest_players`, `breeze_attack_entity_sensor` (attackable non-creative/spectator players → `nearest_attackable`). Default activity FIGHT; update: FIGHT otherwise IDLE.
  Own memories: `breeze_shoot`, `breeze_shoot_charging`, `breeze_shoot_recovering`, `breeze_shoot_cooldown`, `breeze_jump_inhaling`, `breeze_jump_target`, `breeze_jump_cooldown`, `breeze_leaving_water`; plus `attack_target`, `walk_target`, `hurt_by`, `nearest_attackable`.
  - CORE: swim 0.8; look at the target (45..90).
  - IDLE: 0 attack `nearest_attackable`; 1 attack the author of `hurt_by` (if alive); 2 "slide towards the walk target" (timeout 20..40; SLIDING pose during; at the end, if attack target: `breeze_shoot` 60 ticks); 3 one of: nothing 20..100 (weight 1), wander ×0.6 (weight 2).
  - FIGHT (requires `attack_target`, no `walk_target`): 0 give up if the target has not been attackable in the last 100 ticks; 1 shoot; 2 big jump; 3 shoot if stuck; 4 slide.
- Slide: on the ground, out of the water, standing pose, without `breeze_jump_cooldown` nor `breeze_shoot`. If the target is in the inner circle (≤ 4 horiz., ≤ 10 vert. from the centre of its block): random retreat point (5/5), visible and farther from the target; otherwise 50/50: point behind the target (target's head yaw +180° ± Gaussian×45°, radius 4..8) or point in the middle ring (distance to target − lerp(rand, 8, 4)). → `walk_target` ×0.6, proximity 1.
- Shot: standing pose, target within < 16 blocks (otherwise clears `breeze_shoot`), requires `breeze_shoot` and no charge/recovery/cooldown nor jump target. SHOOTING pose; charge 15 ticks; then fires a `breeze_wind_charge` from mid-height +0.3 towards the target (Y at 0.3 of its height, 0.8 if passenger), speed 0.7, inaccuracy 5 − 4×difficulty_id; recovery 4; max duration 20; at the end: cooldown 10, clears `breeze_shoot`.
- Breeze wind charge: 1 damage on direct contact; bursts into a gust of radius 3 ("trigger" interaction: operates buttons/doors, does not break blocks, big knockback), no fire.
- Big jump (max duration 200): on the ground or in water, not in deep fluid nor lava. Existing jump target → OK. Otherwise: target outside the follow range → forgotten; target within ≤ 4 → no; no honey block below, 4 blocks of air (or water) above; point behind the target brought back to the surface (vertical radius 10 down then up); block below not dangerous; line of sight to the point or 4 above. Inhale 10 ticks (INHALING pose, looks at it), then ballistic vector for a random angle among {40,55,60,75,80}° with max speed 0.0583×follow (1.4 at 24) (+ jump boost); long_jumping pose, no friction. Landing (ground, or water if it did not start from water) → standing, jump cooldown 10 (2 if recently hurt), `breeze_shoot` 100 ticks.
- Shoot if stuck: passenger, in water or levitating → `breeze_shoot` 60 ticks.

#### Undead horses (horse base: jump 0.7, HP 53, speed 0.225, `step_height` 1.0, `safe_fall_distance` 6, `fall_damage_multiplier` 0.5, then overridden)

Common horse goals: 0 float; 1 buck when an untamed one rides it (×1.2); 1 panic if not ridden by a mob (×1.2); 2 breeding ×1.0; 3 temptation ×1.25; 4 follow the parent ×1.0; 6 wander avoiding water ×0.7; 7 look at player 6; 8 look around; 9 rear up at random (if able).

##### `minecraft:zombie_horse` (creature; natural spawn now)
- Attributes: HP 25 fixed; jump 0.5 + 3×U(0;1/15); speed (9 + 3×U(0;1)) / 42.16 (≈ 0.213..0.285); rest horse base.
- AI: 0 float; 3 temptation ×1.25 through the tag `zombie_horse_food` (red mushroom); + common goals (except those replaced).
- Particularities: NATURAL spawn → a zombie (normal equipment, then iron spear in hand) rides it. Always despawnable (even tamed? the rule returns true, but any player interaction makes it persistent). No love/breeding (false), does not grow up. Food: red mushroom (healing/taming through the horse mechanism). Untamed + non-food item in hand → gets angry (rears). All equipment slots allowed (horse armour possible, tag `can_wear_horse_armor`). Sun protection = BODY slot (armour): burns in sunlight (tag) except with armour, which wears down. Leash possible only if tamed or not ridden by a mob. Charge (horse jump) ×1.4.

##### `minecraft:skeleton_horse` (creature; thunderstorm trap)
- Attributes: HP 15; speed 0.2; jump 0.4 + 3×U(0;0.2); rest horse base.
- AI: common goals. Trap: goal 1 "skeleton trap" added when `SkeletonTrap`.
- Particularities:
  - Trap spawn: during a thunderstorm, per chunk, 1/100,000 per tick: lightning at a random point of the chunk (lightning target); if it rains there, if `spawn_mobs` and rand < effective_difficulty × 0.01 and no lightning rod below → trap skeleton horse (adult) + lightning.
  - Trigger: living player within ≤ 10 → no longer a trap, tamed, adult; VISUAL-only lightning; a skeleton rides it; + 3 other tamed skeleton horses, persistent, invulnerable 60 ticks, pushed (triangle ±1.1485 in X/Z), each ridden by a skeleton. Each skeleton: normal equipment, persistent, invulnerable 60 ticks, iron helmet if head empty, weapon and helmet re-enchanted (provider `mob_spawn_equipment`).
  - Untriggered trap: disappears after 18,000 ticks if not persistent (`SkeletonTrapTime` saved).
  - Untamed: no interaction (no taming). In water: slowdown 0.96 (swims better). Does not grow up. Does not burn (outside the tag). All equipment slots allowed.

#### `minecraft:bat` (ambient)
- Attributes: HP 6; follow range 16; speed 0.7 (default, unused). No goal (AI coded in the server tick).
- Resting (synchronised flag, `BatFlags` saved; born resting server-side): zero speed, stuck under the block (Y = ⌊Y⌋+1−height). If the block above is not "conductive" (full opaque) → flies off; otherwise 1/200 to turn the head at random, and flies off if a player is within ≤ 4 and visible (default targeting conditions: line of sight required). Taking off → level event 1025. Any injury wakes it up.
- Flight: target cleared if it is no longer air or below the world; new target if none, 1/30 per tick, or target within < 2: position + (rand(7)−rand(7), rand(6)−2, rand(7)−rand(7)). Velocity += ((sign(dx)×0.5 − vx)×0.1, (sign(dy)×0.7 − vy)×0.1, (sign(dz)×0.5 − vz)×0.1) towards the centre (Y+0.1); yaw = flight direction; advances 0.5. Every tick outside rest: vertical speed ×0.6. 1/100 per tick to land if the block above is solid.
- Particularities: no fall damage, ignores block triggers (pressure plates…), does not push and is not pushed (to be checked), emits step events. Volume 0.1.
- Spawn: below the world surface (heightmap world_surface), 50 % refusal, local max light ≤ rand(0..3), block below of the tag `bats_spawnable_on`, + mob rules.

### 4.2 Nether, End and bosses

Default attribute values (reminder): max_health 20, movement_speed 0.7, attack_damage 2, follow_range 32 (but every mob receives a base of 16, unless overridden), armor 0, knockback_resistance 0, attack_knockback 0, flying_speed 0.4, tempt_range 10 (animals).
Spawn category: all `monster` except strider (`creature`).

##### `minecraft:blaze` (monster)
- Attributes: HP 20; speed 0.23; damage 6; follow range 48; armour 0; fire-immune. XP 10.
- AI: 4 blaze attack (see below); 5 return to restriction zone ×1.0; 7 wander avoiding water ×1.0 (probability 0); 8 look at player 8; 8 look around. Targeting: 1 retaliation (alerts the other blazes); 2 nearest player (sight required).
- Pathfinding: water forbidden (penalty −1), lava penalty 8, fire and fire neighbourhood penalty 0.
- Particularities:
  - Slow fall: in the air and vertical speed < 0 → Y speed ×0.6 every tick.
  - Desired height: every 100 ticks, allowed offset drawn as a triangle(0.5, ±6.891). If the target's eye is higher than the blaze's eye + offset → vertical push vy += (0.3 − vy)·0.3.
  - Attack (timer decremented every tick): distance² < 4 and sight → melee hit every 20 ticks + advances towards the target. Otherwise if distance < follow range (48) and sight: salvo cycle — step 1: wait 60 ticks, "charged" state (visual flame); steps 2 to 4: one shot every 6 ticks (3 small fireballs); step 5: pause 100 ticks, end of charge, back to 0. Horizontal spread triangle(dx, 2.297·√√dist·0.5); shot from middle Y +0.5. Otherwise, if seen less than 5 ticks ago → advances towards the target.
  - Small fireball: 5 damage (type fireball), ignites for 5 s; places fire on the block hit (rule mobGriefing).
  - Water-sensitive (damage in water/rain). Does not burn in sunlight.

##### `minecraft:ghast` (monster)
- Attributes: HP 10; follow range 100; flying_speed 0.06; camera_distance 8; fire-immune. XP 5. No attack_damage (it is not a "Monster" class mob but counts as an enemy).
- AI: 5 random floating; 7 look (orients the body towards the target if < 64 blocks, otherwise towards its velocity); 7 fireball shooting. Targeting: 1 nearest player, checked every ~10 ticks (1 chance in 10), sight required, |ΔY| ≤ 4.
- Particularities:
  - Movement: free flight (friction 0.02). Control: every 2 to 6 ticks (2 + rand(5)), adds to the velocity direction·(flying_speed·5/3) ≈ 0.1 towards the wanted point; if the path meets a collision → stop (wait).
  - Random floating: restarts if there is no destination, or destination at < 1 block or > 60 blocks. Point drawn in a ±16 cube around it (64 attempts if restriction zone). The first valid point is returned as is: the correction "Y brought down (Y − |ΔY|) when the point exceeds the surface" only happens after 64 failures, so never for a ghast without a restriction zone (checked in the 26.3 code, F12).
  - Shot: target within < 64 blocks and in sight → counter +1 per tick; at 10 warning sound; at 20 fires a large fireball starting 4 blocks ahead (sight), middle Y +0.5, then counter = −40 (hence ~60 ticks between shots). Out of sight, counter goes back down. Visual "charging" if counter > 10.
  - Default explosion power 1 (saved "ExplosionPower"), the explosion sets fire.
  - Deflection: a large fireball sent back by a player deals 1000 damage to the ghast (instant death, advancement); only this case bypasses its invulnerabilities.
  - No fall damage. Spawn: 1 chance in 20 on top of the normal rules, max group 1.

##### Common "cubes" base (slime, magma_cube, sulfur_cube)
- Size 1..127 (natural ≤ 4), saved "Size" = size − 1. Box = base box × size. Max HP = size² (except sulfur), speed = 0.2 + 0.1·size.
- Size at spawn: 2^k, k = rand(3) (0,1,2); if k < 2, +1 with probability 0.5 × special difficulty multiplier → 1, 2 or 4.
- Common goals: 1 float (in a liquid, jumps 80 % of ticks, advances ×1.2); 4 random direction (without target, on ground/liquid/levitation: new angle 0..359 every 40 to 99 ticks); 5 jump nonstop (advances ×1.0, except if passenger).
- Movement by jumps: on the ground, delay between jumps = 10 + rand(20) ticks, divided by 3 if "aggressive" (pursuing a target and able to hurt); between two jumps speed 0; in the air advances at speed × attribute. Rotation towards the wanted direction ≤ 90°/tick.
- Attack: on contact (player who touches it, or pushed iron golem), if within melee range and in sight → damage = attack_damage. The smallest (size 1) do not hurt (except magma).
- Attack goal (prio 2): looks at the target and jumps towards it; gives up after 300 ticks (gets "tired"), then may resume.
- Death: if size > 1, splits into 2 + rand(3) cubes of size/2 (sulfur: 2), offset by ±width/4, +0.5 in Y, random orientation, same team.

##### `minecraft:magma_cube` (monster)
- Attributes: HP size² (1/4/16); speed 0.2+0.1·size (declared base 0.2); attack_damage = size, real damage = size + 2 (3/4/6); armour = size × 3 (3/6/12); fire-immune. XP = size.
- AI: cubes base + 2 attack. Targeting: 1 nearest player (1/10 per tick, sight, |ΔY| ≤ 4); 3 iron golem.
- Particularities: delay between jumps ×4 (40..116 ticks, /3 if aggressive); jump impulse = normal power (0.42) + 0.1·size; in lava jumps with vy = 0.22 + 0.05·size; even size 1 deals damage; never a baby. Spawn: only outside peaceful (no light condition).

##### `minecraft:sulfur_cube` (monster category, but neutral sound type, peaceful)
- Attributes: simple mob attributes (no attack_damage); HP = 4 × size (adult size 2 → 8 HP, baby size 1 → 4 HP); speed 0.2+0.1·size; tempt_range 8. Adult XP 1 + rand(2), baby 0.
- Sizes: only 1 (baby) or 2 (adult). Spawn: adult size 2, baby size 1; adult → size 2 transition. Splitting on death: 2 babies (none if primed). Spawn rule: always true (on the ground).
- AI: cubes base + 2 temptation (adult: swallowable items; baby: its food, tag `sulfur_cube_food` = slime ball); 3 look for an item on the ground (adult, pickup timer at 0): nearest swallowable item within ±8 blocks, heads for it in aggressive mode. No targeting goal, never deals attack damage.
- Swallowing a block ("body" slot): tag `sulfur_cube_swallowable` = union of the 12 archetypes. Pickup from the ground (if slot empty, not a baby, timer ≤ 0) or given by hand by the player (swap: the old block is spat out; same block refused). Held item: drop guaranteed.
- When it carries a block: ALL goals are removed, speed 0, no longer walks (move control inactive, no 0-block walk), stays aligned at 90°, breathes underwater, does not freeze, can be leashed, becomes persistent, omnidirectional aerial movement. It becomes a "physical object": gets pushed by the player (horizontal distance < 1.3: push ∝ player speed ×2 ×0.3 (×0.16 if in a vehicle), capped at 0.5, vertical component 0.3 if on the ground, × (1 − knockback resistance)).
- Archetypes (data registry `sulfur_cube_archetype`, chosen by item tag): each modifies knockback_resistance, explosion_knockback_resistance, bounciness, friction_modifier, air_drag_modifier, horizontal/vertical knockback power (default without archetype 0.33/0.06), floating, sounds.
  - bouncy (planks, logs, bamboo): kb_res −2, bounce +0.9, friction ×0.3, air drag ×0.01, floats, knockback 0.4125/0.105.
  - regular (dirt, mud, clay, concrete powder, coal, bone): kb_res −1, bounce 0.5, friction ×0.3, drag ×0.1, floats, 0.4125/0.09.
  - slow_flat (metal blocks/iron-gold-copper ores, netherite, debris): kb_res +0.5, bounce 0.4, friction ×0.4, drag ×0.1, 0.4125/0.105.
  - fast_flat (corals, sponges, moss, resin, pumpkin, melon, hay, froglights): kb_res −1, bounce 0.5, friction ×0.2, drag ×0.01, 0.9125/0.09.
  - light (wool): kb_res −1, bounce 1.0, friction ×0.3, drag ×1.8, floats, 0.4125/0.18.
  - fast_sliding (blue/packed ice, snow block): kb_res +0.5, bounce 0.1, friction ×0.05, drag ×0.01, 0.6625/0.09.
  - slow_sliding (mushroom blocks, mycelium, warts, shroomlight): kb_res +0.8, bounce 0.1, friction ×0.05, drag ×0.01, 0.4125/0.09.
  - sticky (honeycomb block): kb_res −2, bounce 0, friction ×2, drag ×0.01, 0.4125/0.09.
  - high_resistance (sand/soul soil): kb_res +0.7, bounce 0.2, friction ×1, drag ×0.01, 0.4125/0.09.
  - slow_bouncy (stones, bricks, concrete, terracotta, stone ores, quartz, obsidian, sulfur/cinnabar…): kb_res +0.4, bounce 0.6, friction ×0.3, drag ×0.05, 0.4125/0.24.
  - hot (magma block): like regular + floats + contact damage 1 (type `sulfur_cube_hot`) to any pushed entity or player who pushes it.
  - explosive (TNT): like regular + floats + explosion power 3, no fire, fuse 120 ticks.
- Special knockback (if it carries a block and the attacker is an entity): the direction is deflected according to the attacker's aim angle (×1.6 horizontal), the power is transferred horizontal↔vertical depending on whether the hit aims at the top or bottom of the cube (×0.5), then rotated according to the difference in feet height (×0.8); × √damage; × (1 − kb_res); horizontal ×0.4; vertical ×1.2 added to vy. Bounds ±128.
- Immunity with a block: damage of the tag `sulfur_cube_with_block_immune_to` (arrow, fall, melee, mob projectile, explosion, freeze, cactus, trident, wind charge, spear, mace…) is cancelled but it still takes the knockback.
- Explosive version (swallowed TNT): primed by fire damage or a flaming arrow (full fuse 120), by an explosion (short fuse = rand(max(1, 120/4)) + 120/8 = 15..44), by a redstone signal at its position, or by the player's flint and steel/fireball. Primed: invulnerable, fuse counts down, at 0 explodes (rule `tnt_explodes`; breaks blocks if mobGriefing) then disappears without splitting. Saved "fuse".
- Player: shears → spits out the block, pickup timer 100 ticks; empty bucket → sulfur cube bucket (keeps age/age lock and contents); baby + slime ball → speeds up growth.

##### Cubes in singleplayer (rc-sim, `mobs/cubes.rs`, F03)
- Class: "age" base (not "monster") → metadata 16 baby, 17 age frozen, 18 size (integer, default 1); sulfur: 19 came out of a bucket, 20 max fuse (−1). No "no action" wear-down in light. Construction: base HP 20 and speed 0.7 as long as the size is not set (finalisation, reload or split).
- NBT reload: `Size` applied BEFORE the saved attributes (the attributes read win); slime and magma neither write nor read `Age`; sulfur: `Age`, `ForcedAge`, `AgeLocked`, `pickup_timer`, `from_bucket`, `fuse`.
- Tick: squish (`squish`) before the common tick; afterwards, on landing, 2 float draws × (width × 2 × 16) for particles with no effect server-side, then the squish sound (pitch `((r−r)·0.2+1)/0.8`); death → removal at 20 ticks → split (parent's random generator: count, then one yaw per small one).
- Sounds: volume 0.4 × size; jump pitch ×1.4 (size 1) or ×0.8; sulfur in the "neutral" category. Contact: attack box = box + `√2.04f − 0.6f`, direct line of sight; `slime.attack` sound.
- Physics added to the common engine: the entity's own bounce on walls and ground (`bounciness`, max with the block's), friction modifiers on the ground and in the air (`friction_modifier`, `air_drag_modifier`), vertical drag = horizontal drag if "omnidirectional" (full sulfur), walk 0 with a block.
- Checked against the official server (25570, 2026-10-12): knockback of a sulfur cube hit by an entity (`damage … mob_attack by`) bit-for-bit identical (test `recul_du_soufre_comme_le_serveur_officiel`).
- Remaining differences: structure spawns (Nether fortress for the magma) not read; iron golem simulated since F06 (targeting and contact damage when it pushes a cube); own signal of a plate under the approached cube (neighbour signal + non-full blocks); "burning" contact damage on the other pushed mobs not done (player only); "TNT disabled" message not sent; golden dandelion (frozen age) not handled; bucket placed via "use on a block" (the game goes through "use" and a ray cast); bucket components (`bucket_entity_data`, `sulfur_cube_content`) not translated for the network; "synchronisable" attributes (bounce, frictions) not sent to the client.

##### Common piglin base (piglin, piglin_brute)
- Pathfinding: fire forbidden (−1), fire neighbourhood penalty 16; opens doors; picks up loot by default.
- Zombification: if the environment attribute `piglins_zombify` is true at its position (true everywhere except the Nether dimension, which sets it to false), not immune, AI active → counter "TimeInOverworld" +1/tick (reset to 0 otherwise); > 300 ticks → becomes `zombified_piglin` (keeps equipment, name…), which receives nausea 200 ticks. Saved "IsImmuneToZombification", "TimeInOverworld".
- The player and entity sensor uses follow_range as its range; sweep every 20 ticks (random initial offset).

##### `minecraft:piglin` (monster)
- Attributes: HP 16; speed 0.35 (baby +20 % multiplicative on the base); damage 5; follow range 16 (mob default). XP 5.
- Spawn: forbidden on a Nether wart block. Outside a structure spawn: 20 % baby; otherwise armed adult: 50 % crossbow, otherwise gold sword (90 %) or gold spear (10 %). Adult: each gold armour piece 10 % (independent). Initial memory `hunted_recently` for 30..120 s (600..2400 ticks). 8-slot inventory (slots 300..307), emptied on death and on conversion.
- Brain — sensors: nearby living entities, nearby players, nearby items (±32 horiz, ±16 vert), hurt by, piglin-specific sensor (every 20 ticks) that fills: `nearest_repellent` (block of the tag piglin_repellents — fire/torch/lantern/lit soul campfire — within Manhattan radius 8 horiz/4 vert), `nearest_visible_nemesis` (wither_skeleton or wither), `nearest_visible_huntable_hoglin` (huntable adult hoglin), `nearest_visible_baby_hoglin`, `nearest_visible_zombified` (zombified_piglin or zoglin), `nearest_targetable_player_not_wearing_gold` (no piece of the tag piglin_safe_armor), `nearest_player_holding_wanted_item`, `nearby_adult_piglins`, `nearest_visible_adult_piglins`, `visible_adult_piglin_count`, `visible_adult_hoglin_count`.
- Activities (selection priority: admire_item > fight > avoid > celebrate > ride > idle):
  - core: look at the target, follow the path, doors; baby: copies the seen nemesis into `avoid_target` for 5..7 s; any piglin within < 6 blocks of a zombified one: `avoid_target` = zombified 5..7 s; drop the admired item when the admiration ends; start admiring a liked item seen on the ground (119 ticks); celebrate if the target dies (300 ticks, dance 10 % if the victim was a hoglin); stop being angry if the target is dead.
  - idle: look at a player holding a liked item (14 blocks); adult: start attacking the best target; hunt a hoglin (if allowed); flee repellents (8 blocks, ×1.0); baby: ride a baby hoglin from time to time (attempt every 10..40 s, duration 10..30 s); random gaze (player/piglin/anyone 8 blocks, or nothing 30..60 ticks); wander ×0.6, or go towards a piglin (8 blocks), or walk towards the looked-at point, or nothing 30..60; interact with player at 4.
  - fight (memory `attack_target`): give up if the target is no longer the best; with crossbow, backs off if target < 5 blocks (×0.75); approach ×1.0; spear behaviours (approach ×1.0 range 10, attack, retreat); melee every 20 ticks; crossbow shot (power 1.6); if a hoglin is killed → hunt memory; clears the target if a zombified one is within < 6 blocks.
  - admire_item (memory `admiring_item`): walk towards the liked item (if off hand free) ×1.0 at ≤ 9 blocks; stop if item > 9 blocks; stop if > 200 ticks trying to reach it (then walking to the item disabled for 200 ticks).
  - avoid (memory `avoid_target`): move away from the target ×1.0 up to 12 blocks; ends if target absent, if hoglin and piglins ≥ hoglins, or if zombified one that is no longer the nearest visible.
  - celebrate (memory `celebrate_location`): flee repellents, look at player with gold, attack if target; go to the celebration spot (×1.0 at 2 blocks, or ×0.6 at 4 while dancing); look at piglin / wander ×0.6 / wait 10..20.
  - ride (memory `ride_target`): mount ×0.8; dismount if mount at > 8 blocks or if the mount is no longer a baby, is dead, or one of the two was recently hurt. A baby riding a hoglin climbs to the top of a stack of 3 passengers max.
- Target choice (adult): none if a zombified one within < 6; otherwise `angry_at` (even out of sight); otherwise if universal anger: visible attackable player; otherwise visible nemesis; otherwise visible player without gold armour.
- Bartering: gold ingot (only currency). Adult not disabled, not already admiring: player interaction → takes 1 ingot in the off hand, admires 119 ticks, stops. On the ground: picks up gold if it wants (rule mobGriefing required to pick up). End of admiration: if ingot and bartering allowed → throws to the nearest visible player (otherwise random position at 4 blocks) the loot of the table `gameplay/piglin_bartering` (1 weighted draw; 19 entries, total weight 469: obsidian, crying obsidian, fire charge, leather, soul sand, Nether brick, spectral arrow, gravel, blackstone at 40; quartz, string 20; iron nugget, ender pearl, potions, dried_ghast 10; iron boots, splash potion, potion 8; enchanted book 5). Other non-currency liked item: equips it or goes to the inventory. Baby: keeps the item in the main hand (does not barter). Gold nuggets: takes the whole stack. Food (raw/cooked porkchop): "eats" if it has not eaten for 200 ticks. Baby ignores leather; nobody picks up repellents.
- Anger for gold: opening a chest/barrel/ender chest/shulker box/copper chest/chest boat/chest minecart (only piglins that see the player), or breaking a block of the tag guarded_by_piglins (gold block, gold ores, gilded blackstone, raw gold block, chests…, even without sight) → all piglins in the idle activity within ±16 blocks get angry at the player (`angry_at` 600 ticks; with the universalAnger rule: at the nearest visible player + `universal_anger` 600).
- Injury by a non-piglin: drops the admired item without bartering, clears celebration/dance/admiration; if player → `admiring_disabled` 400 ticks. Baby: flees the attacker 100 ticks and alerts the adults. Adult attacked by a hoglin while visible hoglins outnumber piglins (+1) → flees 5..20 s (and no hunting for 30..120 s), propagated to visible piglins. Otherwise retaliates (if not fleeing, target not much farther than the current target, margin 4): `angry_at` 600 ticks and alerts nearby adult piglins (which take the nearer of the two targets; do not target a non-huntable hoglin).
- Hoglin hunting: adult, visible huntable hoglin, not angry, no `hunted_recently` on it nor on visible piglins → anger against the hoglin, alert, and the whole group receives `hunted_recently` 30..120 s. `CannotHunt` saved (bastion piglins).

##### `minecraft:piglin_brute` (monster)
- Attributes: HP 50; speed 0.35; damage 7; follow range 12. XP 20.
- Spawn: always gold axe in hand; memory `home` = spawn position. Only picks up gold axes. Does not hunt, does not barter, ignores gold.
- Brain: sensors nearby entities, players, items, hurt by, brute sensor (nemesis = nearest visible wither_skeleton/wither; nearby adult piglins). Activities: fight > idle.
  - core: look, follow path, doors, end of anger if target dead.
  - idle: attack the best target; look (player/piglin/brute/anyone 8 or nothing 30..60); wander ×0.6, go towards piglin or brute (8), return towards `home` (×0.6, within 2, give up beyond 100), wander around home (radius 5), nothing 30..60; interact with player at 4.
  - fight: give up if no longer the best target; approach ×1.0; melee every 20 ticks.
- Target: `angry_at` (even out of sight), otherwise any visible attackable player (gold or not), otherwise nemesis.
- Retaliation when hit by a non-piglin (same logic as the piglin: `angry_at` 600 + alert of adults). Anger sound 1.25 %/tick in combat. Zombification like the piglin (> 300 ticks outside the Nether).

##### `minecraft:zombified_piglin` (monster)
- Attributes: HP 20; speed 0.23 (+0.05 additive when angry, adults only; baby +50 % multiplicative like the zombie); damage 5; follow range 35; armour 2; reinforcements 0 (never reinforcements); fire-immune. Pathfinding: lava penalty 8.
- AI (inherits from the zombie for looking): 1 spear use (×1.0); 2 zombie melee (×1.0, does not pursue without sight); 7 wander avoiding water ×1.0; 8 look at player 8; 8 look around. Targeting: 1 retaliation (alerts the other zombified_piglin); 2 nearest player it is angry at (1/10 per tick, sight required); 3 universal anger reset.
- Group anger: neutral as long as it is not hit. Retaliation → target + persistent anger 20..39 s (400..780 ticks), restarted as long as the target is present; saved "anger_end_time" + "angry_at" (UUID). While it has a target: every 4..6 s (80..120 ticks), if it sees it, it gives its target to all zombified_piglin without a target within a box of ±35 horiz (follow range) × ±10 vert, except allies of the target. The first anger sound is delayed 0..1 s. Anger dropped if the target (mob) dies, if the player goes creative/spectator or peaceful; rule universalAnger: anger against all players if anger without a specific target. Prevents the player from sleeping if it is angry at them.
- Equipment: gold sword (95 %) or gold spear (1/20). Baby 5 % (zombie), with 5 % chicken jockey (existing chicken within 5 blocks, otherwise 5 % to create one). Picks up loot 55 % × special multiplier. Does not drown into a zombie (no conversion in water). Would burn in sunlight but is fire-immune → does not burn.
- Spawn: outside peaceful, not on a Nether wart block, not in a liquid. Result of the conversion of a piglin/brute (nausea 200 ticks).
- Nether portal (random tick of each portal block): if the rules `spawn_mobs` and `spawn_monsters` are true, outside peaceful, environment attribute `gameplay/nether_portal_spawns_piglin` true at the position (Overworld only, false by default), draw `next_int(2000) < difficulty id` (tick's random generator) and a non-spectator player within 128 blocks (horizontal) of the chunk centre: we go down as long as the block is portal; if the first non-portal block accepts the zombified piglin's spawn (block predicate: magma only for fire-immune mobs), it is born above, at the block centre, without searching for the ground, reason "structure" (orientation drawn then full finalisation, ambient sound). A portal delay is set on it and its mount.
- Pig struck by lightning outside peaceful: simple conversion into a zombified piglin (the pig's equipment not carried over, pickup kept), gold weapon drawn with the pig's random generator (spear 1/20, otherwise sword), persistent; in peaceful, ordinary lightning.

##### `minecraft:hoglin` (monster)
- Attributes: HP 40; speed 0.3; damage 6 (baby 0.5); knockback resistance 0.6; attack knockback 1; follow range 16. XP 5 (baby 3). Leash possible.
- Spawn: not on a Nether wart block; 20 % baby. Remote despawn always allowed (except persistence after a successful interaction).
- Brain — sensors: living entities, players, nearest adult (for the young), hoglin sensor (`nearest_repellent`: block of the tag hoglin_repellents — warped fungus, potted, Nether portal, respawn anchor — Manhattan 8 horiz/4 vert; nearest visible adult piglin; number of visible adult piglins; nearby adult hoglins and their number).
- Activities (fight > avoid > idle):
  - core: look at the target, follow the path.
  - idle: repellent detected → `pacified` 200 ticks; breeding (×0.6); move away from the repellent ×1.0 (8 blocks); attack the nearest visible attackable player (except pacified or breeding); adult: move away from the visible adult piglin ×0.4 (8); sometimes look at an entity (8, every 30..60 ticks); young: follow an adult (distance 5..16, ×0.6); wander ×0.4 / walk towards the gaze ×0.4 / nothing 30..60.
  - fight (`attack_target`, set for 200 ticks): repellent → pacified; breeding; approach ×1.0; adult melee every 40 ticks, baby every 15; give up if target invalid; clears the target if it breeds.
  - avoid (`avoid_target` 5..20 s): move away ×1.3 up to 15 blocks; wander; ends if adult and piglins are no longer more numerous.
- Attack: adult damage = damage/2 + rand(damage) → 3..8; then launch if attack knockback − target's resistance > 0: horizontal (power × (0.2..0.7)), rotated by a random value −10..10 (passed as is to the rotation, hence in radians), vertical power × (0..0.5). Blocking with a shield still launches (adult). Attack animation 10 ticks.
- When hitting a piglin while visible adult piglins > visible hoglins + 1 → flight propagated to visible adult hoglins; otherwise propagates the target to visible adult hoglins (except pacified ones, they take the nearest).
- Injured: clears pacified and partner; baby → flees; adult → retaliates (except against a hoglin, except if it is fleeing a piglin, if attacker not much farther than the current target +4) and alerts.
- Breeding: food = crimson fungus; young persistent. Prefers to walk on crimson nylium (value 10), refuses positions within < 8 of a repellent (−1).
- Zombification outside the Nether (same attribute `piglins_zombify`, not immune, AI active): > 300 ticks → `zoglin` (nausea 200 ticks). "CannotBeHunted" saved; huntable = adult and unmarked.

##### `minecraft:zoglin` (monster)
- Attributes: HP 40; speed 0.3; damage 6 (baby 0.5); knockback resistance 0.6; attack knockback 1; follow range 16; fire-immune. XP 5. Leash possible.
- Spawn: 20 % baby (mostly via conversion of the hoglin, which keeps the age).
- Brain: sensors living entities + players only. Activities fight > idle.
  - idle: attack the nearest visible mob that is neither zoglin nor creeper (and attackable); sometimes look (8, 30..60 ticks); wander ×0.4 / walk towards the gaze ×0.4 / nothing 30..60.
  - fight: approach ×1.0; adult melee 40 ticks, baby 15; give up if invalid.
- Hurt by an attackable mob (not much farther than the current target +4) → target 200 ticks. Attack and launch identical to the hoglin. No repellent, no breeding, no flight.

##### `minecraft:strider` (creature)
- Attributes: HP 20; speed 0.175 (−34 % multiplicative on the base when it "freezes"); tempt_range 10; fire-immune. No damage (passive).
- Pathfinding: water forbidden (−1), lava/fire/fire neighbourhood penalty 0; walk value: lava 10, other 0, and −∞ outside lava if it is already in lava. Ground navigation that accepts lava as a stable destination. Passengers inherit its penalties.
- AI: 1 panic ×1.65; 2 breeding ×1.0; 3 temptation ×1.4 (warped fungus or warped fungus on a stick); 4 go to lava ×1.0 (outside lava; lava block with passable top, search 8 horiz / 2 vert, recomputed every 20 ticks); 5 follow the parent ×1.0; 7 wander ×1.0 (interval 60); 8 look at player 8; 8 look around; 9 look at another strider 8.
- Cold: every tick (AI active), "warm" if the occupied block or the block under the feet is lava (tag strider_warm_blocks) or if it touches lava, or if it rides a strider that is itself warm; otherwise "suffocating" (shivering, speed penalty). Water-sensitive (damage). Never displayed on fire.
- Lava: stands on lava (solid surface 8/16 high). In lava: if above the surface and no lava above → considered on the ground, otherwise speed ×0.5 + 0.05 upwards. No fall damage on arriving in lava.
- Saddle: saddle slot (living adult only), can be placed by a dispenser. Interaction without food, saddled, without passenger, without sneaking → the player mounts. Controlled only if the player holds a warped fungus on a stick; it always advances straight ahead according to the player's orientation (pitch ×0.5). Ridden speed = speed × 0.55 (0.35 if it freezes) × boost factor. Boost (stick, uses 1 durability, stick has 100 durability): duration 140 + rand(841) ticks, factor 1 + 1.15·sin(π·t/duration). No new passenger if already ridden or if its eyes are in lava. Dismounting: looks for a non-lava floor around (5 directions ±22.5°/±45°).
- Breeding: food warped_fungus (tag strider_food).
- Spawn: valid position if, going up through the lava, we reach air. Adult: 1/30 (if monster spawning is allowed) → zombified_piglin jockey (baby 5 %) holding a warped fungus on a stick, saddled strider (saddle guaranteed in the loot); otherwise 1/10 → a baby strider on its back (age −24000); otherwise "50 % babies" group data NEW for each adult: their size is 0, so no young is ever drawn by the group (nor any random draw) — natural young only come from mounts (F12, 26.3 code).

##### `minecraft:wither_skeleton` (monster)
- Attributes: HP 20; speed 0.25; damage 4 (set at spawn; base 2 before); follow range 16; fire-immune and immune to wither roses (tag wither_skeleton_immune_to). Immune to the wither effect. Pathfinding: lava penalty 8.
- AI (skeleton): 2 stay in the shade; 3 flee the sun ×1.0; 3 flee wolves (6 blocks, ×1.0/×1.2); 4 weapon: melee ×1.2 (no pursuit out of sight) or, if it holds a bow, bow shooting (×1.0, interval 20 ticks on hard / 40 otherwise, range 15); 5 wander avoiding water ×1.0; 6 look at player 8; 6 look around. Targeting: 1 retaliation; 2 nearest player (sight); 3 piglins (piglin and brute); 3 iron golem; 3 baby turtle on land.
- Particularities: spawns with a stone sword, without equipment enchantments; refuses to pick up bows and crossbows; picks up loot 55 % × special multiplier; pumpkin at Halloween 25 %. Successful hit → wither effect 200 ticks (level I) on the target. Arrows (if bow forced) set on fire for 100 s. The sun would make it burn but it is fire-immune.

##### `minecraft:shulker` (monster)
- Attributes: HP 30; armour 0 open, +20 ("covered" modifier) when closed; follow range 16; fire-immune. XP 5. No speed (immobile: speed always zero, impulses ignored, position snapped to the block centre).
- AI: 1 look at player 8 (probability 0.02, only along the axis); 4 attack; 7 peek; 8 look around. Targeting: 1 retaliation (alerts the other shulkers); 2 nearest player (not in peaceful); 3 if in a team: nearest monsters ("Enemy") (1/10 per tick). Flattened search zone: ±follow range on both axes of the attachment plane, ±4 on the axis of the attachment face.
- Attachment: synchronised attachment face (default down). Must have a solid block on that side and room to open fully; otherwise looks for another face (order down, up, north, south, west, east), otherwise teleports.
- Opening ("peek", 0..100): closed 0, peeking 30, open 100; the displayed value advances by 0.05/tick towards the target; opening grows the box outwards and pushes entities (not shulkers). Peeking: without target, 1 chance in 40 per tick, for 20·(1..3) ticks, then closes.
- Attack (not in peaceful): opens to 100, first shot after 20 ticks then every 20 + rand(10)·10 ticks (20..110); target at > 20 blocks → loses the target. Closes at the end.
- Shulker bullet: homing, speed 0.15, advances in segments along the axes (10..50 steps per segment), accelerates ×1.025; 4 damage + levitation 200 ticks; destroyed by a hit or a block; gravity 0.04 if no target.
- Injuries: closed → immune to arrows. If HP < 50 %: 1 chance in 4 to teleport. Otherwise, hit by a shulker bullet while open → teleports then duplication: number of living shulkers within ±8 around the old box = n, fails with probability (n − 1)/5, otherwise a new shulker (same colour) appears at the old position.
- Teleport: 5 attempts, random offset −8..8 on each axis, empty position, inside the border, above the world bottom, without collision, with an attachable face whose block is not bedrock (tag shulker_does_not_teleport_to); dismounts, closes, loses the target. Pushed by a shulker box (piston) → teleports.
- Variant: colour 0..15 or 16 = no colour ("Color"). No change of orientation at spawn (rotation 0).

##### `minecraft:guardian` (monster)
- Attributes: HP 30; speed 0.5; damage 6; follow range 16. XP 10. Water: penalty 0; aquatic navigation; walk value in water 10 + light cost.
- AI: 4 beam attack; 5 return to restriction zone ×1.0; 7 wander ×1.0 (interval 80, restarted immediately when it is hurt or finishes an attack); 8 look at player 8; 8 look at another guardian 12 (probability 0.01); 9 look around. Targeting: 1 nearest player, squid or axolotl, at more than 3 blocks, sight required (1/10 per tick).
- Beam: on activation, wait 10 ticks (counter −10 → 0), at 0 the target is locked (synchronised); the target must stay in sight otherwise it is lost; stops if the target comes within ≤ 3 blocks (normal guardian). After 80 ticks of lock: indirect magic damage 1 (+2 on hard) then a normal melee attack (attack_damage 6), then the target is released. Immobile during the beam (navigation stopped), body aligned with the head.
- Thorns: if it is not moving (not travelling) and the direct attacker is alive, the attacker takes 2 thorns damage (except magic, thorns or explosion damage — tag avoids_guardian_thorns).
- Physics: in water, air always full; swimming: acceleration 0.1, friction ×0.9, sinks by 0.005/tick if immobile and without a target. Out of water on the ground: jumps at random (+0.5 vertical, ±0.4 horizontal, random orientation). Movement with sinusoidal undulation (±0.05).
- Spawn: outside peaceful, in water with water below; and either 1 chance in 20, or no sky visible from under the water (monuments).

##### `minecraft:elder_guardian` (monster)
- Attributes: HP 80; speed 0.3; damage 8; follow range 16. XP 10. Always persistent.
- AI: the guardian's, wandering with interval 400. Beam in 60 ticks; magic damage 1 + 2 (elder) + 2 on hard = 3 or 5, then melee 8; does not release the target when it is within ≤ 3 blocks. Identical thorns (2).
- Mining fatigue: when (age in ticks + id) is a multiple of 1200, applies mining fatigue III (amplifier 2) for 6000 ticks to any survival player (non-allied) within < 50 blocks who lacks the effect, or has a weaker one, or one that ends in < 1200 ticks; these players receive the game event "elder guardian appearance" (value 1, or 0 if it is silent).
- Home: if it has none, sets its restriction zone to its position, radius 16.

##### `minecraft:warden` (monster)
- Attributes: HP 500; speed 0.3; damage 30; knockback resistance 1.0; attack knockback 1.5; follow range 24. XP 5. Never removed from afar. Cannot ride a vehicle. Disables a shield for 5 s. Dampens vibrations (like wool).
- Pathfinding: floats in liquids; unpassable rails penalty 0, dangerous blocks 8, powder snow 8, lava 8, fire and neighbourhood 0; walk value 0 everywhere; path distance computed in XZ only.
- Brain — sensors: nearby players; warden sensor (living entities within the follow range 24, then `nearest_attackable` = nearest targetable player, otherwise nearest other targetable mob). Targetable: alive, same dimension, neither creative nor spectator, non-allied, not an armour stand, not another warden, not invulnerable, inside the border. Key memories: `dig_cooldown`, `is_emerging`, `roar_target`, `attack_target`, `disturbance_location`, `is_sniffing`, `sniff_cooldown`, `sonic_boom_cooldown`, `recent_projectile`, `touch_cooldown`, `vibration_cooldown`, `nearest_attackable`.
- Activities (order: emerge > dig > roar > fight > investigate > sniff > idle):
  - core: swim (×0.8); look towards the roar target or the disturbance location (if no attack target); look at the target; follow the path.
  - emerge (memory `is_emerging`, 134 ticks): "emerging" pose, immobile, then standing.
  - dig (if no `roar_target` nor `dig_cooldown`): forces dismounting, then digs for 100 ticks ("digging" pose, on the ground or in a liquid) and disappears (removed from the world, no loot). The `dig_cooldown` delay = 1200 ticks, set at spawn and renewed at each anger increase, disturbance, loss of target, every tick in combat, and permanently if it is persistent (named…) — so it only goes back underground after 60 s of calm.
  - idle: choose a roar target (the entity it is "angry" at); try to sniff (if `nearest_attackable` present, no disturbance, no cooldown: sniffing pose, next delay 100..200 ticks); wander ×0.5 or nothing 30..60 (not during the sniff).
  - investigate (`disturbance_location`): roar if angry; go to the location ×0.7 up to 2 blocks.
  - sniff (`is_sniffing`, 84 ticks): at the end, if `nearest_attackable` at ≤ 6 horiz / 20 vert → +35 anger; if there is no disturbance location, sets one on the entity.
  - roar (`roar_target`, 84 ticks): roar pose, +20 anger against the target, sound after 25 ticks; at the end the target becomes `attack_target` (sonic attack delay 200).
  - fight (`attack_target`): give up if no longer "angry" or target not targetable (and clears the anger if not targetable); look at the target; approach ×1.2; sonic attack; melee every 18 ticks.
- Anger (0..150 per suspect, capped): levels calm < 40 ≤ agitated < 80 ≤ angry. Every 20 ticks: −1 for each suspect (removed at ≤ 1 or if no longer valid; a player who disconnected/changed dimension keeps their anger by UUID). Priority suspect: first the angry ones, then players, then the highest anger. Standard increase +35; injury → +100 (80 + 20) against the author and, if it has no target and the hit is direct or the author within < 5 blocks → immediate target; projectile heard: owner within < 30 blocks → +10 the first time, +35 if a projectile was heard < 100 ticks ago (and the disturbance is placed on the shooter); physical contact (push) → +35 and disturbance, at most once per 20 ticks. If a player becomes "angry" while the current target is not a player → target cleared (switches to the player).
- Vibrations: listens to the events of the tag `warden_can_listen` within radius 16, not while digging/emerging, not if `vibration_cooldown` (40 ticks after each reception), ignores non-targetable entities. Reception: +35 anger against the source (or the projectile rule); if it is not yet angry, sets `disturbance_location` (100 ticks) at the position (if projectile, or no suspect, or the suspect is the source).
- Disturbance: only if it is neither angry nor in combat: sniff delay 100, look at the position 100, `disturbance_location` 100, clears the walk.
- Sonic attack: target at ≤ 15 horiz / 20 vert, `sonic_boom_cooldown` absent (200 after acquiring a target, 40 after each melee hit or after a sonic attack); lasts 60 ticks (attack on cooldown during this time), discharge at tick 34: 10 damage of type sonic_boom (ignores armour), push 2.5 horizontal and 0.5 vertical × (1 − knockback resistance), passes through blocks (no line test).
- Darkness: when (age + id) is a multiple of 120, darkness effect 260 ticks to any survival player within < 20 blocks (rule "< 200 ticks remaining" to refresh).
- Invulnerable (except damage that bypasses) and immune to explosions while digging/emerging; box 1 block tall at that time; not pushable.
- Spawn: summoned by the sculk shrieker with reason "triggered" → emerges (134 ticks, agitated sound). Per-player warning tracking: level 0..4, +1 per shriek at most every 200 ticks (players within 16 blocks share), drops every 12000 ticks; at level 4 a warden is summoned (shrieker details outside this sheet).

##### `minecraft:wither` (monster, boss)
- Attributes: HP 300; speed 0.6; flying_speed 0.6; follow range 40; armour 4; fire-immune and immune to wither roses (blocks' tag wither_immune_to). XP 50. No effect applies (refuses any effect). Does not ride, does not take portals, does not despawn (except in peaceful).
- Navigation: flight (flight control, max pitch 10), floats, does not open doors; ignores the slowdown of cobwebs/berries.
- AI: 0 do nothing during the summoning invulnerability; 2 ranged attack (×1.0, every 40 ticks, range 20) = central head; 5 random flight avoiding water ×1.0; 6 look at player 8; 7 look around. Targeting: 1 retaliation; 2 nearest living mob (checked every tick, no sight required) that is not in the tag wither_friends (= #undead) and is attackable.
- Summoning (wither skull block on a soul T): 220 ticks of invulnerability, HP set to 1/3 of max (100); heals 10 HP every 10 ticks (→ full HP); boss bar fills (1 − remaining/220); at the end: explosion power 7 at eye height, no fire, breaks blocks ("mob" interaction), global sound. Saved "Invul".
- Movement: every tick, vertical speed ×0.6; if it follows its target (head 0): rises (vy = max(0,vy) + 0.3 − vy·0.6) if lower than the target, or if lower than target + 5 while it is not "armoured"; approaches horizontally (+0.3 towards the target − 0.6·v) as long as > 3 blocks away. Orientation towards its velocity if it is notable.
- Side heads (2, indices 1 and 2): each every 10..19 ticks: on normal/hard, if it has not fired for > 15 updates, fires a "dangerous" (blue) skull towards a random point (±10 horiz, ±5 vert); if it has a valid target (attackable, ≤ 30 blocks, in sight) → fires a black skull and next update in 40..59 ticks; otherwise loses the target; without target → picks one at random among the valid mobs within ±20 horiz/±8 vert (range 20). The central head follows the main target; its shots have a 0.1 % chance of being blue. Each injury adds +3 to the heads' idle counter (no more blue skulls).
- Wither skull: 8 damage (the wither recovers 5 HP if the target dies); without owner 5 magic damage; wither II effect for 10 s (normal) / 40 s (hard), nothing on easy; explosion power 1 on impact (no fire, breaks according to the mob rule). Blue skull: inertia 0.73 (slower), and blocks destructible by the wither have at most 0.8 resistance against its explosion.
- Block destruction: any injury arms a 20-tick countdown (if there is not one already); at 0, if mobGriefing: breaks (with loot) all non-air blocks outside the tag wither_immune (bedrock, barrier, End portals/frames, command blocks, structure, jigsaw, moving piston, light, reinforced deepslate, test blocks) in the box around it (±(width/2 + 1) horiz, from its feet to +height).
- Phase 2 ("armoured"): HP ≤ 50 % (150) → immune to arrows and wind charges (direct projectile), and no longer rises to +5 above its target (stays level). Armour visual.
- Regeneration: +1 HP every 20 ticks outside invulnerability. Immunities: drowning damage (tag), damage coming from another wither or from any mob of the tag wither_friends (undead).
- Purple boss bar, darkens the screen, visible to the players tracking it. Death: Nether star with extended lifetime.

##### `minecraft:ender_dragon` (monster, boss)
- Attributes: HP 200; follow range 16; camera_distance 16; fire-immune. No collision with blocks (entity without physics). No effect, does not ride, no portal, never removed. No goals: phase machine, synchronised by an integer (`DragonPhase`, saved, along with `DragonDeathTime` and `sitting_damage_received`).
- Parts (8 sub-entities, positioned every tick relative to the body): head 1×1, neck 3×3, body 5×3, 3 tail segments 2×2, 2 wings 4×2. Head and neck advanced by 6.5/5.5 according to yaw; tail follows the flight history (samples 12, 14, 16); tilt drawn from the history (samples 5 and 10).
- Flight graph: 24 nodes around the fight origin — 12 on a circle of radius 60 (ground height +5), 8 on a radius of 40 (ground +15), 4 on a radius of 20 (ground +5), min height 73; fixed adjacency by bit masks; A* search. The outer nodes 0..11 are only used while living crystals remain; otherwise only 12..23 (inner ring). Each waypoint receives a height + random 0..20.
- Common steering (towards the phase's "target position"): vy += clamp(Δy/horiz_dist, ±phase_speed)·0.01; rotation towards the target limited to ±50°/tick, damped (yaw_speed ×0.8 + Δ·rotation_speed, applied ×0.1); forward push 0.06 × (alignment·span + 1 − span), span = 2/(dist²+1); horizontal friction 0.8..0.95 depending on alignment, vertical ×0.91; ×0.8 of movement if it crosses blocks. Default phase speed 0.6; rotation speed = 0.7 / min(v+1, 40) / (v+1).
- Contacts (if not recently hurt): wings (box inflated 4,2,4 offset by −2 in Y) → launches mobs (horizontal push 4/d², vertical 0.2) and, outside the sitting phase and if the target has not been hurt for 2 ticks, 5 damage; head and neck (box +1) → 10 damage.
- Breaks blocks: head, neck and body; any non-air block outside the tag dragon_transparent (light, fires) is removed without loot if mobGriefing and outside the tag dragon_immune (bedrock, obsidian, crying obsidian, end stone, iron bars, anchor, portals/frames, command blocks…); otherwise it is a "wall" that slows it down.
- End crystals: 1 time in 10 per tick, looks for the nearest crystal within ±32; if it has one, +1 HP every 10 ticks (healing beam). Crystal destroyed (any damage except coming from the dragon): explosion power 6 (unless destroyed by an explosion), fire on the ground; if it was the crystal healing it → 10 explosion damage to the dragon's head, attributed to the player (or to the nearest player within 64). The phase is notified (in circular flight: switches to "strafe" that player).
- Damage: nothing during death; the phase may modify; hits outside head/neck → damage/4 + min(damage, 1); ignored if < 0.01; only those from a player or of the tag always_hurts_ender_dragons (explosions) count. In the sitting phase: arrows and wind charges deal nothing (and the projectile catches fire for 1 s); no knockback; if the cumulative damage in the sitting position exceeds 25 % of max HP (50) → takeoff.
- Fatal blow: if it is not sitting → HP set back to 1 and phase "death".
- Phases:
  - hover ("Hover", initial phase at creation): stays in place (target position = current position), speed 1.0, counts as sitting.
  - holding_pattern (circle): new target if within < 10 or > 150 blocks of the point, or collision. When the path is finished: 1 chance in (crystals + 3) → landing approach; otherwise nearest player to the podium (no sight required): if present and (1/(⌊d²/512⌋+2) or 1/(crystals+2)) → strafe that player. Otherwise new path: nearest node, ±1 in the current direction, 1/8 to reverse direction and skip 6 nodes; outer ring modulo 12 if crystals remain (≥ 0 in this case — always true if fight present), otherwise inner nodes 12..19.
  - strafe_player (strafe): path to a point above the player (height min(0.4 + d/80 − 1, 10)); if player within < 64 and in sight: charge +1/tick, otherwise −1; at charge ≥ 5 and heading aligned within < 10° → dragon fireball from the head, then back to circular flight. Without player → circular flight.
  - landing_approach: path to the node opposite the nearest player to the podium (−40·dir, y 105) then final node = podium; path finished → landing.
  - landing: aims at the top of the podium (centre), speed 1.5, fast rotation (min(v+1,40)/(v+1)); within < 1 block → flame counter reset to 0, phase "sitting scan".
  - sitting_scanning: player within ≤ 20 blocks and |ΔY| ≤ 10: after 25 ticks → "sitting attack"; before that, pivots towards them if not aligned (±10°). Otherwise after 100 ticks → takeoff, or charge towards the nearest player within ≤ 150.
  - sitting_attacking (roar): 40 ticks then "sitting breath".
  - sitting_flaming (breath): at tick 10, area effect cloud placed in front of the head (2.5 blocks) on the first ground below: radius 5, duration 200, instant damage (duration scale 0.25); lasts 200 ticks then back to the scan, or takeoff after 4 breaths. Cloud removed at the end of the phase.
  - takeoff: path to the node opposite the head's direction (−40·gaze, y 105); as soon as it is > 10 blocks from the podium → circular flight.
  - charging_player (charge): speed 3.0 towards the player's memorised position; on arrival (< 10 or > 150, or collision) waits 10 ticks then circular flight.
  - dying (death): aims at the podium, speed 3.0; HP kept at 1 until it has arrived (< 10 blocks, or collision), then HP 0.
- Dragon fireball: on impact (except on its launcher) → breath cloud: radius 3 growing to 7 over 600 ticks, instant damage II (scale 0.25), centred on a mob within < 4 blocks if there is one.
- Death: 200 ticks of animation, rises 0.1/tick; total XP 12000 for the first dragon of the world, 500 afterwards: 8 % every 5 ticks after tick 150, then 20 % at tick 200 (rule mob_drops); global sound at tick 1; at tick 200, signals victory to the fight and disappears (exit portal, egg: handled by the fight, outside the sheet).

### 4.3 Land and flying animals, golems

Common reminders (valid for all entries below unless stated):
- "Mob" base: `max_health` 20, `follow_range` 16, `knockback_resistance` 0, `armor` 0 (defaults of living attributes). "Animal" base = mob base + `tempt_range` 10.
- Animals: classic breeding (food of the tag `<species>_food` → in love 600 ticks, baby age −24000, parents waiting +6000), baby grows through food (10 % of remaining time per item, unless stated).
- "Panic" = flight at speed ×N when hurt (or on fire), towards a random point / water if on fire.
- "Temptation" = follow a player holding the item (range `tempt_range`), stops if the player moves suddenly when the "scared" flag is true.
- AI notation: `prio goal (parameters)`; separate targeting.
- Immune to fall damage (tag `fall_damage_immune`) in this part: copper_golem, iron_golem, snow_golem, allay, bee, cat, chicken, happy_ghast, ocelot, parrot.
- Golden dandelion on a baby (except tag `cannot_be_age_locked`): locks/unlocks its growth.

#### Equids

##### `minecraft:horse` (creature)
- Attributes: HP drawn at spawn 15 + U[0,7] + U[0,8] (15..30); speed (0.45 + 3 × U[0, 0.3]) × 0.25 (0.1125..0.3375); `jump_strength` 0.4 + 3 × U[0, 0.2] (0.4..1.0); `step_height` 1; `safe_fall_distance` 6; `fall_damage_multiplier` 0.5; base defaults before the draw: HP 53, speed 0.225, jump 0.7.
- AI: 0 float; 1 mount panic ×1.2 (only if not controlled by a rider); 1 "run in circles" ×1.2 (taming); 2 breeding ×1.0 (with equids); 3 temptation ×1.25 (tag `horse_tempt_items`, not scareable); 4 follow parent ×1.0; 6 wander avoiding water ×0.7; 7 look at player 6; 8 look around; 9 rear up at random (if the species can rear). No targeting.
- Particularities:
  - Taming: `temper` 0..100 (max 100). When a player rides it untamed: every tick, 1 chance in (50 adjusted) to test; if draw U[0,99] < temper → tamed by that player; otherwise temper +5, throws the rider and rears up ("angry").
  - Food (healing / age advance in ticks / temper): wheat 2/20/3; sugar 1/30/3; hay bale 20/180/0; apple 3/60/3; red mushroom 3/0/3; carrot 3/60/3; golden carrot 4/60/5 (+ in love if tamed adult); golden apple and enchanted 10/240/10 (+ in love if tamed adult). Temper only increases if untamed (or item used for love).
  - Out of combat: 1/900 per tick → heals 1 HP. Grazes: if on `grass_block`, not ridden, 1/300 per tick → eating animation 50 ticks (purely visual server-side, no block eaten).
  - Hurt: 1/3 to rear up.
  - Breeding: horse × horse → horse; horse × donkey → mule. Foal attributes: parents' average + deviation × quality, where deviation = |A−B| + 2 × 15 % of the range, quality = (mean of 3 U[0,1]) − 0.5; if out of range, reflected on the bound. Variant (coat): 4/9 parent A, 4/9 parent B, 1/9 random; markings: 2/5 A, 2/5 B, 1/5 random.
  - Spawn: group of up to 6 sharing the same coat, markings random; 5 % babies in the group (horse group data; 20 % for donkey and mule, see § 4.F04). 7 coats × 5 markings.
  - Equipment: saddle, horse armour (body slot); inventory opens only if tamed.

##### `minecraft:donkey` (creature)
- Attributes: HP 15..30 (same draw as the horse); fixed speed 0.175; `jump_strength` 0.5; rest like horse (step 1, safe fall 6, mult. 0.5).
- AI: identical to the horse (random rearing included).
- Particularities: can carry a chest (15 slots, 5 columns × 3); same foods and taming (temper max 100); donkey × donkey → donkey, donkey × horse → mule. Baby at scale 0.5.

##### `minecraft:mule` (creature)
- Attributes: like the donkey (HP 15..30, speed 0.175, jump 0.5).
- AI: identical to horse/donkey.
- Particularities: chest like the donkey; cannot breed (no valid partner), but if forced the foal is a mule.

##### `minecraft:llama` (creature)
- Attributes: HP 15..30 (horse draw); speed 0.175; `jump_strength` 0.5; step 1; safe fall 6; mult. 0.5.
- AI: 0 float; 1 run in circles ×1.2 (taming); 2 follow caravan ×2.1; 3 ranged attack (spit; ×1.25, interval 40 ticks, range 20); 3 panic ×1.2; 4 breeding ×1.0; 5 temptation ×1.25 (tag `llama_tempt_items`); 6 follow parent ×1.0; 7 wander avoiding water ×0.7; 8 look at player 6; 9 look around. Targeting: 1 retaliation (stops as soon as it has spat once); 2 nearest untamed wolf (follow range reduced to 25 %, sight not required but must be able to reach it, draw 1/16).
- Particularities:
  - Strength 1..3 (4 % chance that the draw goes up to 5); chest columns = strength (3 × strength slots). Baby: strength U[1, max(strengthA, strengthB)] +1 with 3 % chance (cap 5); variant of one of the two parents (50/50).
  - Spit: aimed projectile (speed 1.5, inaccuracy depending on difficulty), damage 1.
  - Taming: max temper 30. Food: wheat (heal 2, age +10, temper +3); hay bale (heal 10, age +90, temper +6, + in love if tamed adult).
  - Caravan: a free llama (not leashed, not already in a caravan) looks in a ±9/±4/±9 box for a llama/trader llama at the end of a caravan (or leashed); only attaches if the head of the chain (≤ 8 links) is leashed and the distance > 2 blocks. If > 26 blocks from its predecessor: speed ×1.2 (cap 3) then gives up after 40 ticks. Follows at distance max(d − 2, 0).
  - Variants: creamy (default), white, brown, gray; same variant for a spawn group.
  - Carpet (body slot) decorative; saddle impossible (not rideable).

##### `minecraft:trader_llama` (creature)
- Attributes: like the llama.
- AI: all of the llama + 1 panic ×2.0. Extra targeting: 1 defend the wandering trader it is attached to (targets what hurt the trader); 2 nearest zombie (except zombified piglin); 2 nearest illager.
- Particularities: scheduled despawn 47999 ticks; if attached to the wandering trader, its counter follows the trader's − 1; at 0, leash removed and entity deleted. Counter frozen (no despawn) if it is tamed or leashed by something other than the trader.

#### Farm animals

##### `minecraft:cow` (creature)
- Attributes: HP 10; speed 0.2.
- AI: 0 float; 1 panic ×2.0; 2 breeding ×1.0; 3 temptation ×1.25 (tag `cow_food`); 4 follow parent ×1.25; 5 wander avoiding water ×1.0; 6 look at player 6; 7 look around. No targeting.
- Particularities: empty bucket on an adult → milk bucket. Variants `temperate` (fallback), `warm` (biomes of the tag `spawns_warm_variant_farm_animals`), `cold` (tag `spawns_cold_variant_farm_animals`), chosen at spawn according to the biome; sound variant drawn at random at spawn. Baby: variant of a parent.

##### `minecraft:mooshroom` (creature)
- Attributes: like the cow (HP 10, speed 0.2).
- AI: identical to the cow.
- Particularities:
  - Bowl on an adult → mushroom stew, or suspicious stew if an effect is memorised (the effect is then consumed).
  - Adult brown variant + flower (item with a suspicious stew effect) → memorises the effects of that flower (flower consumed); if an effect is already memorised, smoke and refusal.
  - Shears on an adult → conversion into a cow (keeps name, etc.), explosion particle, drops the shearing loot (mushrooms of its colour, table `shear_mooshroom`), tool damaged.
  - Lightning: toggles red ↔ brown (once per lightning bolt, memorises the bolt's UUID).
  - Breeding: baby = colour of a parent (50/50); if parents have the same colour, 1/1024 for the opposite colour.
  - Spawn: on a block of the tag `mooshrooms_spawnable_on` and bright enough; prefers to walk on mycelium (path weight 10). Variants `red` (default), `brown`.

##### `minecraft:pig` (creature)
- Attributes: HP 10; speed 0.25.
- AI: 0 float; 1 panic ×1.25; 3 breeding ×1.0; 4 temptation ×1.2 (carrot on a stick); 4 temptation ×1.2 (tag `pig_food`); 5 follow parent ×1.1; 6 wander avoiding water ×1.0; 7 look at player 6; 8 look around.
- Particularities:
  - Saddle (saddle slot, adult only, can also be placed by a dispenser). Click without food on a saddled unridden pig → the player mounts.
  - Controlled only if the rider holds a carrot on a stick; ridden speed = `movement_speed` × 0.225 × boost factor. Boost (click with the stick): duration U[140, 980] ticks, factor 1 + 1.15 × sin(π × t / duration); costs 7 durability to the stick (25 max).
  - Lightning (outside peaceful) → conversion into a zombified piglin (default equipment: gold sword, persistent); in peaceful, normal lightning damage.
  - Variants `temperate`/`warm`/`cold` according to biome (same tags as the cow) + random sound variant.

##### `minecraft:sheep` (creature)
- Attributes: HP 8; speed 0.23.
- AI: 0 float; 1 panic ×1.25; 2 breeding ×1.0; 3 temptation ×1.1 (tag `sheep_food`); 4 follow parent ×1.1; 5 graze; 6 wander avoiding water ×1.0; 7 look at player 6; 8 look around.
- Particularities:
  - Grazing: triggered at 1/1000 per tick (1/50 for a baby) if the block at the feet is of the tag `edible_for_sheep` or if the block below is `grass_block`. Animation 40 ticks; 4 ticks before the end: destroys the plant (no drop) or turns the grass below into dirt (event 2001), only if `mob_griefing`; in all cases the "has eaten" effect applies: wool regrows (no longer sheared), baby ages by 60 ticks (applies even without griefing).
  - Shears on an unsheared adult → sheared, loot `shear_sheep` (wool of its colour, 1..3 historically — loot table).
  - Dyeing: colours the wool. Baby: mixed colour of the parents (dye-mixing recipe; otherwise one of the two).
  - Colour at spawn (weights out of 100) — temperate: black 5, gray 5, light gray 5, brown 3, "common white" 82; warm: gray 5, light gray 5, white 5, black 3, "common brown" 82; cold: light gray 5, gray 5, white 5, brown 3, "common black" 82. "Common" = 499/500 the colour, 1/500 pink. Warm/cold biome via the tags `spawns_warm_variant_farm_animals` / `spawns_cold_variant_farm_animals`.
  - Sheep named "jeb_": rainbow colour (client only).

##### `minecraft:chicken` (creature)
- Attributes: HP 4; speed 0.25.
- AI: 0 float; 1 panic ×1.4; 2 breeding ×1.0; 3 temptation ×1.0 (tag `chicken_food`); 4 follow parent ×1.1; 5 wander avoiding water ×1.0; 6 look at player 6; 7 look around.
- Particularities:
  - Slowed fall: in the air and descending, vertical speed × 0.6 every tick; immune to fall damage (tag `fall_damage_immune`).
  - Egg laying: counter U[6000, 11999] ticks; adult, not a jockey → drops the loot `chicken_lay` (egg depending on the variant: normal, brown for `warm`, blue for `cold`), then new counter.
  - Jockey: a baby zombie (and variants) has a 5 % chance at spawn to ride a chicken (existing unmounted chicken within ±5/±3/±5, otherwise a new one). Jockey chicken: does not lay, despawns far from players like a monster, 10 XP instead of the default.
  - Variants `temperate`/`warm`/`cold` according to biome + sound variant.

#### Small animals and companions

##### `minecraft:rabbit` (creature)
- Attributes: HP 3; speed 0.3; damage 3. Killer: `armor` 8, damage +5 (additive modifier "evil" → 8).
- AI: 1 float; 1 climb out of powder snow; 1 panic ×2.2; 2 breeding ×0.8; 3 temptation ×1.0 (tag `rabbit_food`); 4 flee player 8 blocks ×2.2; 4 flee wolf 10 ×2.2; 4 flee monster 4 ×2.2 (flights disabled for the killer); 5 raid garden; 6 wander avoiding water ×0.6; 11 look at player 10. Killer additionally: 4 melee ×1.4 (pursuit without sight); targeting 1 retaliation (alerts neighbouring rabbits), 2 nearest player, 2 nearest wolf.
- Particularities:
  - Movement by hops: navigation speed imposed by the current goal; jump power 0.3 (0.2 if modifier ≤ 0.6; 0.5 if the next path step is > 0.5 block higher or horizontal collision), normalised against the standard jump 0.42. Delay on the ground after landing: 10 ticks (3 if speed ≥ 2.2). Horizontal momentum at jump (0.1 with component 1.5 in height; 0.5 for a baby).
  - Killer: when it has no delay and its target is within < 4 blocks, it jumps straight at it.
  - Garden raiding (only if `mob_griefing`): looks (radius 16, speed 0.7) for a block of the tag `supports_crops` carrying a ripe carrot, if its "more carrots" counter is at 0; on arrival: age-0 carrot destroyed, otherwise age −1 (event 2001); counter = 40 ticks, decremented by U[0,2] per tick. Retries 10 ticks later.
  - Variants: biome of the tag `spawns_white_rabbits` → white 80 %, white splotched 20 %; tag `spawns_gold_rabbits` → gold; otherwise brown 50 %, salt and pepper 40 %, black 10 %. Killer (`evil`, id 99) never natural (command / data). Same variant for the whole spawn group. Baby: 1/20 biome variant, otherwise 50/50 one of the parents. The killer receives the translated name `entity.minecraft.killer_bunny` if it is not named.

##### `minecraft:cat` (creature)
- Attributes: HP 10; speed 0.3; damage 3.
- AI: 1 float; 1 tamed panic ×1.5; 2 sit on command; 3 lie on the sleeping owner; 4 temptation ×0.6 (tag `cat_food`, scareable if untamed); 4 flee players 16 blocks ×0.8/×1.33 (untamed only); 5 lie on a block ×1.1 (radius 8, tag `cats_can_lie_on`, air above); 6 follow owner ×1.0 (starts at 10 blocks, stops at 5); 7 sit on a block ×0.8 (tamed not sitting; unopened chest, lit furnace, bed outside the head; tag `cats_can_sit_on`); 8 leap at target 0.3; 9 feline attack; 10 breeding ×0.8; 11 wander avoiding water ×0.8 (probability 1e-5 per tick to choose a point); 12 look at player 10. Targeting: 1 random rabbit (if untamed); 1 baby turtle on land (if untamed).
- Particularities:
  - Feline attack: speed ×1.33 if outside melee range but within < 4 blocks, otherwise ×0.6 if < 15 blocks, otherwise ×0.8; gives up beyond 15 blocks once the path is finished; hit every 20 ticks on contact.
  - Taming: item of the tag `cat_food` held by a player, 1/3 success per item (sits immediately). Tamed: food heals 1 × nutrition (fallback 1) if hurt; dye (tag `cat_collar_dyes`) = collar; otherwise click = sit/stand. Untamed despawns far from players after 2400 ticks of existence.
  - Lying on the sleeping owner's bed (≤ 10 blocks, spot not taken by another cat within ±2): on waking, if the player slept ≥ 100 ticks and draw < gift chance (environment attribute, 0.7 between hours 23667 and 362 of the day, 0 otherwise) → random teleport ±5/±2/±5 around (or around the leash holder) then loot `cat_morning_gift` in front of them.
  - Scares: creepers flee cats and ocelots at 6 blocks (×1.0/×1.2); phantoms, every 20 ticks, look for a cat at 16 blocks → the cat hisses and the phantom gives up swooping.
  - Variants: tabby, black (default), red, siamese, british_shorthair, calico, persian, ragdoll, white, jellie (all unconditional), all_black (priority 1 in the structures of the tag `cats_spawn_as_black` = witch hut; priority 0 if moon brightness ≥ 0.9). Baby: variant of a parent 50/50, collar = mix of the collars if tamed. Random sound variant. Red collar by default.
  - Village spawn: dedicated generator every 1200 ticks, position at 8..31 blocks from the player on each axis; if > 4 occupied beds (home POI) within 48 blocks and < 5 cats within ±48/±8/±48 → a cat; in a witch hut: black cat if < 1 cat within ±16/±8.

##### `minecraft:ocelot` (creature)
- Attributes: HP 10; speed 0.3; damage 3.
- AI: 1 float; 3 temptation ×0.6 (tag `ocelot_food`, scareable while it has no trust); 4 flee players 16 blocks ×0.8/×1.33 (without trust); 7 leap 0.3; 8 feline attack (like the cat); 9 breeding ×0.8; 10 wander ×0.8 (1e-5); 11 look at player 10. Targeting: 1 nearest chicken (no sight requirement); 1 baby turtle on land (draw 1/10).
- Particularities: trust ("Trusting"): player within < 3 blocks while temptation is active, item `ocelot_food` → 1/3 trust. Not tameable. Despawns far from players after 2400 ticks if without trust. Spawn: 2/3 of attempts accepted, above sea level, on grass or leaves, without liquid.

##### `minecraft:wolf` (creature)
- Attributes: HP 8 (40 once tamed, back to 8 if untamed); speed 0.3; damage 4.
- AI: 1 float; 1 tamed panic ×1.5 (environmental causes only, damage tag `panic_environmental_causes`); 2 sit on command; 3 flee llamas 24 blocks ×1.5 (untamed, if llama strength ≥ U[0,4]); 4 leap 0.4; 5 melee ×1.0 (pursuit without sight); 6 follow owner ×1.0 (10 → 2 blocks); 7 breeding ×1.0; 8 wander avoiding water ×1.0; 9 beg (player within 8 blocks holding a bone or food); 10 look at player 8; 10 look around. Targeting: 1 what hurt the owner; 2 what the owner hit; 3 retaliation (alerts the pack); 4 player it is angry at (draw 1/10); 5 random sheep/rabbit/fox (untamed); 6 baby turtle on land (untamed); 7 nearest skeleton; 8 reset universal anger.
- Particularities:
  - Taming: bone (not angry), 1/3 → tamed, sitting, target cleared. Tamed: food `wolf_food` heals 2 × nutrition (fallback 2); dye `wolf_collar_dyes` = collar (red by default); click = sit/stand; hurt → stands up.
  - Attack orders: never attacks creeper, ghast, armour stand, tamed horses, tamed animals, wolves of the same owner, non-attackable players (PvP).
  - Anger: neutral; duration drawn 20..39 s (400..780 ticks); cannot be leashed while angry.
  - Wolf armour (body slot, tamed adult by its owner): absorbs damage except tag `bypasses_wolf_armor` (armour wear instead of HP); repair with the repair item while sitting: 12.5 % of max durability per item; removable with shears by the owner.
  - Wet: shakes once on dry ground when stopped (≈ 40 ticks), purely visual + sound.
  - Variants (biome, high priority): pale (fallback), spotted (savannas), snowy (grove), black (old growth pine taiga), ashen (snowy taiga), rusty (jungles), woods (forest), chestnut (old growth spruce taiga), striped (badlands). The whole pack shares the variant; max group 8. Tamed parents' baby: tamed by the same owner, mixed collar. Random sound variant.
  - Avoids powder snow (path cost −1 = forbidden).

#### Wild animals

##### `minecraft:fox` (creature)
- Attributes: HP 10; speed 0.3; damage 2; `follow_range` 32; `safe_fall_distance` 5.
- AI: 0 float (fox variant); 0 climb out of powder snow; 1 "head stuck" (40 ticks immobile after a failed pounce into snow); 2 panic ×2.2; 3 breeding ×1.0; 4 flee players 16 blocks ×1.6/×1.4 (except trusted players, non-sneaking/creative, and if it is not defending); 4 flee untamed wolves 8 ×1.6/×1.4; 4 flee polar bears 8; 5 stalk prey (prey target > 6 blocks: crouches and approaches); 6 pounce on prey; 6 look for shelter ×1.25; 7 melee ×1.2 (pursuit without sight); 7 sleep; 8 follow parent ×1.25; 9 wander around the village (radius 32, interval 200); 10 eat berries ×1.2 (radius 12, height 1); 10 leap 0.4; 11 wander avoiding water ×1.0; 11 look for items on the ground; 12 look at player 24; 13 perch and look around (2 % per tick, 2..4 gazes of 80..99 ticks). Targeting: 3 defend a trusted being (targets what hurt it); depending on variant — red: 4 chicken/rabbit (draw 1/10, no sight), 4 baby turtle on land, 6 schooling fish (1/20); arctic: 4 schooling fish, 6 chicken/rabbit, 6 baby turtle.
- Particularities:
  - Item in the mouth (main hand): at spawn 20 % to have one — emerald 5 %, egg 15 %, rabbit foot or hide 20 % (50/50), wheat 20 %, leather 20 %, feather 20 % (proportions of this sub-draw). Picks up an item on the ground if mouth empty (or replaces a non-edible with an edible); spits out the old one.
  - Eats the edible item in its mouth after 600 ticks without eating (on the ground, no target, awake); signs from 560 ticks (10 %/tick).
  - Berries: sweet berry bush of age ≥ 2 or glow berry vines; picks 1 + U[0,1] (+1 if age 3) berries: one in the mouth if empty, the rest on the ground; the bush falls back to age 1.
  - Sleep: by day, immobile, sheltered (sky not visible from the top of its box), no frightening entity within ±12/±6/±12, outside powder snow; random delay U[0, 140) before falling asleep; wakes if hurt or threatened. Look for shelter: during a thunderstorm if it sees the sky, or by day (test every 100 ticks) if it sees the sky outside a village.
  - Pounce: crouched, target moving in its direction and clear path → jump towards the target; landing in snow → head stuck.
  - Trust: list of trusted UUIDs (2 max). Baby born from breeding: trusts the players who fed its parents. Does not flee these players and defends them.
  - Variants: `red` (default), `snow` (biomes of the tag `spawns_snow_foxes`); spawn group: same variant, from the 3rd member on they are babies.

##### `minecraft:panda` (creature)
- Attributes: HP 20 (10 if "weak" gene expressed); speed 0.15 (0.07 if "lazy"); damage 6.
- AI: 0 float; 2 panic ×2.0 (environmental causes; stops if sitting); 2 breeding ×1.0 (requires bamboo); 3 melee ×1.2; 4 temptation ×1.0 (tag `panda_food`); 6 flee player 8 ×2.0 ("worried" only); 6 flee monster 4 ×2.0 ("worried"); 7 sit down to eat an item on the ground (tag `panda_eats_from_ground`); 8 lie on its back ("lazy" 1/400 per tick); 8 sneeze (babies); 9 look at player 6; 10 look around; 12 roll (babies or "playful"); 13 follow parent ×1.25; 14 wander avoiding water ×1.0. Targeting: 1 retaliation (only calls "aggressive" pandas; a non-aggressive panda gives up after one bite or if it receives bamboo).
- Particularities:
  - Genes: main + hidden, each drawn out of 16: lazy 1, worried 1, playful 1, aggressive 1, weak 5 (values 3 and 5..8), brown 2, normal 5. Recessive: brown, weak (expressed only if both genes are identical, otherwise "normal"). The others are dominant (the main gene decides).
  - Heredity: each gene of the baby = one of a parent's two genes (random), the main one coming from one parent and the hidden one from the other; 1/32 random mutation for each.
  - Breeding: requires a bamboo block within radius 8 and 3 layers high; otherwise "unhappy" 32 ticks, retry after 600 ticks.
  - Food: bamboo; baby → advances 10 %; adult ready → in love; otherwise sits and eats (keeps the item in hand, eats it in stages, 1/80 to start, end after 80 ticks then 1/20).
  - Sneeze (babies, 1/6000 per tick; 1/500 if "weak"): lasts 20 ticks, makes all adult pandas within 10 blocks jump, drops the loot `panda_sneeze` (slime ball) if `mob_drops`.
  - "Worried": during a thunderstorm, out of the water, stays seated and stops eating.
  - Cannot be leashed. Golden dandelion on a baby locks/unlocks its growth (common rule for animals).

##### `minecraft:polar_bear` (creature)
- Attributes: HP 30; speed 0.25; damage 6; `follow_range` 20.
- AI: 0 float; 1 own melee ×1.25 (pursuit without sight); 1 panic ×2.0 (baby: all panic causes; adult: environmental only); 4 follow parent ×1.25; 5 wander ×1.0; 6 look at player 6; 7 look around. Targeting: 1 retaliation (a hurt baby calls the adults; only adults join); 2 players near a cub (adult with a baby within ±8/±4/±8; draw 1/20, sight required, follow range × 0.5); 3 player it is angry at; 4 nearest fox (adult, 1/10, sight required, must be able to reach it); 5 reset universal anger.
- Particularities:
  - Attack: on contact, strikes and sits back down; within (target width + 3) of distance, rears up when ≤ 10 ticks remain before the hit (warning sound every 40 ticks max).
  - Neutral: anger 20..39 s. No food (no item-based breeding).
  - Spawn: group in which only the first is an adult, the following are babies; in biomes of the tag `polar_bears_spawn_on_alternate_blocks`, may spawn on blocks of the tag `polar_bears_spawnable_on_alternate` (ice) if bright enough.

#### Flyers (classic goals)

##### `minecraft:bee` (creature)
- Attributes: HP 10; `flying_speed` 0.6; speed 0.3; damage 2.
- AI: 0 attack (sting) ×1.4 (pursuit without sight; only when angry and not having stung yet); 1 enter the hive; 2 breeding ×1.0; 3 temptation ×1.25 (tag `bee_food`); 3 validate hive (every 20..40 ticks); 3 validate flower (20..40); 4 pollinate; 5 follow parent ×1.25; 5 locate a hive; 5 go to the hive; 6 go to the known flower; 7 grow crops; 8 flying wander; 9 float. Targeting: 1 retaliation (alerts neighbouring bees); 2 player it is angry at (1/10, sight required); 3 reset universal anger.
- Particularities:
  - Sting: damage = `attack_damage` rounded; poison 10 s on normal, 18 s on hard (none on easy/peaceful), level 0; increments the target's stinger counter; the bee loses its anger and becomes "has stung". Then, every 5 ticks, chance 1/(1200 − t) (bound ≥ 1) to die (damage = remaining HP) → near-certain death within 1200 ticks.
  - Anger: neutral, 20..39 s; visual roll if angry within < 2 blocks of its target. Harvesting a hive without a campfire below / breaking a hive: all bees within ±8/±6/±8 target a player picked at random in this area.
  - Pollination: flowers of the tag `bee_attractive` within radius 5; hover 0.6 above, speed ×0.35, changes point 1/25; ≥ 400 ticks to obtain nectar, gives up after 600. Delay before looking for a flower: 20..60 ticks (200 after a failure). Goes to the known flower after 600 ticks without pollinating.
  - Return to the hive if: nectar, or > 3600 ticks without nectar since leaving, or environment attribute "bees stay in the hive" (night: hours 12542 → 23460, rain and thunderstorm); never if it has stung, has a target, or if the hive is near a fire. Hive search radius 20, direct path if < 16; gives up if > 48; max trip 2400 ticks; 3 hives blacklisted max.
  - Hive (block): 3 occupants max; stay ≥ 2400 ticks with nectar, ≥ 600 without; cannot leave/re-enter before 400 ticks; an exit with nectar raises the honey level by 1 (2 with 1/100), max 5.
  - Crop growth: with nectar, 70 % of attempts, 1/30 per tick; block below of the tag `bee_growables` (crops +1 age, stems +1, berry bush +1, cave vines → berries); 10 crops max between two pollinations; event 2011.
  - Drowning: > 20 ticks underwater → 1 damage per tick.

##### `minecraft:parrot` (creature)
- Attributes: HP 6; `flying_speed` 0.4; speed 0.2; damage 3.
- AI: 0 tamed panic ×1.25; 0 float; 1 look at player 8; 2 sit on command; 2 follow owner ×1.0 (5 → 1 block); 2 flying wander ×1.0 (prefers nearby leaves/logs); 3 land on the owner's shoulder; 3 follow another mob ×1.0 (stops at 3, searches within 7 blocks). No targeting.
- Particularities:
  - Taming: item of the tag `parrot_food` (seeds), 1/10 per item. Item of the tag `parrot_poisonous_food` (cookie) → poison 900 ticks and infinite damage (death), even tamed. Neither food nor breeding.
  - Tamed, on the ground, by the owner: click = sit/stand.
  - Shoulder: lands if not sitting, owner neither spectator, nor creative-flying, nor in water or powder snow, and delay > 100 ticks since the last dismount. The player releases it (after ≥ 20 ticks perched) if fall > 0.5, in water, flying, sleeping, in powder snow, on death, as a spectator, in a spin attack (trident).
  - Imitation: 1/400 per tick, then 1/2 → plays the "imitated" sound of a non-silent mob picked at random within 20 blocks (mapping table per monster type). Ambient sound: 1/1000 (outside peaceful) to imitate a random monster.
  - Dance: a jukebox playing within ≤ 3.46 blocks puts it in party mode; stops if the jukebox disappears or moves away.
  - Variants at spawn, uniform: red_blue (default), blue, green, yellow_blue, gray.
  - No fall damage (fall check neutralised; same as bee).

#### Golems (classic goals)

##### `minecraft:iron_golem` (misc)
- Attributes: HP 100; speed 0.25; `knockback_resistance` 1.0; damage 15; `step_height` 1.
- AI: 1 melee ×1.0 (pursuit without sight); 2 move towards the target ×0.9 (range 32); 2 return to the village ×0.6; 4 wander in the village ×0.6; 5 offer a flower; 7 look at player 6; 8 look around. Targeting: 1 defend the village; 2 retaliation; 3 player it is angry at (1/10, sight required); 3 any "enemy" monster except creeper (1/5, no sight); 4 reset universal anger.
- Particularities:
  - Hit: damage = D/2 + U[0, D−1] with D = `attack_damage` (15 → 7.5..21.5); launches the target upward by 0.4 × (1 − knockback resistance). Hit animation 10 ticks.
  - Village defence: within ±10/±8/±10, a villager whose reputation of a nearby player is ≤ −100 → that player becomes the target (players filtered by the golem's targeting conditions).
  - Contact: a monster (except creeper) that pushes it becomes its target with 1/20.
  - Created by a player (pumpkin on an iron structure): never attacks a player. Never attacks a creeper.
  - Neutral: anger 20..39 s.
  - Repair: iron ingot heals 25 HP (refused if HP full). Visual cracks according to the HP fraction (thresholds < 75 %, < 50 %, < 25 %).
  - Flower: by day, 1/8000 per tick, if there is a villager within ±6/±2/±6 → holds out a poppy for 400 ticks; baby villagers can take it.
  - Spawn: summoned by villagers (panic/meeting) or built; no natural spawn by biome.

##### `minecraft:snow_golem` (misc)
- Attributes: HP 4; speed 0.2.
- AI: 1 ranged shooting ×1.25 (interval 20 ticks, range 10); 2 wander avoiding water ×1.0 (probability 1e-5); 3 look at player 6; 4 look around. Targeting: 1 nearest "enemy" mob (1/10, sight required, no reach requirement).
- Particularities:
  - Snowball: aimed at the target's eyes − 1.1 with elevation 0.2 × horizontal distance, speed 1.6, inaccuracy 12. Damage 3 to blazes, 0 to others (knockback only).
  - Snow trail: if `mob_griefing`, every tick, places a snow layer on the 4 corners (±0.25) of its position if the spot is air and snow can stay there.
  - Melting: 1 fire damage per tick where the environment attribute "snow golem melts" is true (warm biomes); water-sensitive (damage in rain / in water).
  - Pumpkin: present by default; shears → remove it (drops a carved pumpkin).

#### Animals with a brain (activities / behaviours)

Brain reminder: CORE activity always active; a single "main" activity chosen every tick by priority order among those whose memory conditions are met. "Run one of" behaviours = weighted draw.

##### `minecraft:goat` (creature)
- Attributes: HP 10; speed 0.2; damage 2 adult, 1 baby (set on age change).
- Sensors: nearby living entities, nearby players, nearby items, nearest adult, hurt by, food temptation. Key memories: `long_jump_cooldown_ticks`, `ram_cooldown_ticks`, `ram_target`, `tempting_player`, `breed_target`, `walk_target`, `temptation_cooldown_ticks`.
- Activities (priority RAM > LONG_JUMP > IDLE):
  - CORE: swim (0.8); panic ×2.0; look at the target; go to the target; count down the 3 timers (temptation, long jump, ram).
  - IDLE (requires both timers running): 0 look at a player within 6 from time to time (30..60 ticks); 0 breeding (goats); 1 follow temptation ×1.25 (tag `goat_food`); 2 baby follows an adult (5..16) ×1.25; 3 one of: wander ×1.0 (weight 2), walk towards what it looks at (2), nothing 30..60 ticks (1).
  - LONG_JUMP (jump timer finished, no temptation/breeding/walk): jump towards a random position up to 5 high and 5 wide, angles 65/70/75/80°, max speed 3.5714 × base jump, 20 attempts, landing on a full block with no path penalty; next timer U[600, 1200] ticks.
  - RAM (ram timer finished): 1 preparation: nearest visible target at 4..7 blocks (not a goat; armour stand only if `mob_griefing`; inside the border), approach ×1.25 then immobile 20 ticks; 0 charge ×3.0: on impact, damage = `attack_damage` (source without aggro), knockback = factor × clamp(speed × 1.65; 0.2; 3.0) + 0.25 × (Speed level − Slowness level), factor 2.5 adult / 1.0 baby, × 0.5 if shield raised. If it hits a block of the tag `snaps_goat_horn` (log, stone, ores…): loses a horn (drops a goat horn with an instrument depending on the type). Next timer U[600, 6000] (screaming U[100, 300]).
- Particularities: screaming at 2 % at spawn (baby: screaming if a randomly drawn parent is, or 2 %). Adult: 10 % to have only one horn (left or right). Bucket → milk. Avoids powder snow. Initial memories: jump U[600,1200], ram U[600,6000]. Fall damage reduced by 10 (normal calculation − 10).

##### `minecraft:sniffer` (creature)
- Attributes: HP 14; speed 0.1.
- Sensors: nearby living entities, hurt by, nearby players, temptation. Key memories: `sniffer_explored_positions` (last 20 dug positions, persisted), `sniffer_sniffing_target`, `sniffer_digging`, `sniffer_happy`, `sniff_cooldown`, `is_panicking`, `walk_target`.
- Activities (priority DIG > SNIFF > IDLE):
  - CORE: swim 0.8; panic ×2.0 (cancels digging/sniffing); go to the target (delay 500..700); count down temptation.
  - IDLE: 0 breeding; 1 follow temptation ×1.25 (stop range 2.5 baby / 3.5 adult; tag `sniffer_food` = torchflower seed); 2 look at the target; 3 "happy" 40..100 ticks (after a find); 4 one of: walk towards the gaze (2), sniff the air 40..80 (1), scent 40..80 (1), look at player 6 (1), wander (1), nothing 5..20 (2).
  - Scenting (adult, without `sniff_cooldown` timer, on the ground, out of water, not tempted/panicking/in love/ridden/leashed): at the end, looks for a block to dig: 5 attempts at a random ground position at 10 + 2i blocks (height 3), block below of the tag `sniffer_diggable_block`, not already explored, reachable → target + walk ×1.25.
  - SNIFF: walks towards the target; on arrival → memory `sniffer_digging`.
  - DIG (digging 160..180 ticks): on successful end, drops the loot `sniffer_digging` (torchflower seed or pitcher pod) at head level, memorises the explored position, sets `sniff_cooldown` 9600 ticks; then "stand up" 40 ticks and becomes happy.
- Particularities: breeding → no baby but a sniffer egg (item) dropped; the egg hatches as a block (out of scope). Large size, baby ("snifflet") does not dig.

##### `minecraft:armadillo` (creature)
- Attributes: HP 12; speed 0.14.
- Sensors: nearby living entities, hurt by, temptation, nearest adult, fear detector (every 5 ticks: an entity within ±7/±2/±7 that scares it → `danger_detected_recently` 80 ticks). Scary: undead (tag `undead`), its last aggressor, player (non-spectator) who sprints or is mounted.
- Key memories: `danger_detected_recently`, `is_panicking`, `temptation_cooldown_ticks`, `gaze_cooldown_ticks`.
- Activities (priority PANIC > IDLE):
  - CORE: swim; unroll if no more danger; look at the target; go to the target (except rolled up); count down temptation and gaze.
  - IDLE: 0 look at player 6 occasionally; 1 breeding ×1.0; 2 one of: follow temptation ×1.25 (stop 1 baby / 2 adult; tag `armadillo_food` = spider eye), baby follows adult (5..16) ×1.25; 3 look around (150..250 ticks); 4 one of: wander, walk towards the gaze, nothing 30..60.
  - PANIC (danger present and not in flight panic): ball — stays rolled up for up to 6000 ticks; "peeks" when the danger returns (100..400 ticks + animation duration 50); starts unrolling (30 ticks) when the danger's expiry < 30 ticks; rolls up again if the danger lasts.
- Particularities:
  - States: IDLE, ROLLING (10 ticks, hidden after 5), SCARED (hidden), UNROLLING (30 ticks, hidden for 26).
  - Rolled up: damage received reduced to (d − 1) / 2. Hurt by a living entity → danger 80 ticks + rolls up (if possible: on the ground, out of water, not ridden, not leashed); hurt by an environmental cause (fire…) → unrolls and panics.
  - Scute: drops a scute every U[6000, 11999] ticks (loot `armadillo_shed`); brush on adult → loot `armadillo_brush` (scute), brush −16 durability.

##### `minecraft:frog` (creature)
- Attributes: HP 10; speed 1.0 (low multipliers on the brain side); damage 10; `step_height` 1.
- Sensors: nearby living entities, hurt by, prey (`frog_attackables`: attackable, edible, not marked unreachable, within < 10 blocks), temptation (tag `frog_food` = slime ball), in water. Key memories: `long_jump_cooldown_ticks`, `long_jump_mid_jump`, `is_in_water`, `is_pregnant`, `attack_target`, `nearest_attackable`, `unreachable_tongue_targets` (5 max, 100 ticks), `tempting_player`, `breed_target`.
- Activities (priority TONGUE > LAY_SPAWN > LONG_JUMP > SWIM > IDLE):
  - CORE: panic ×2.0; look at the target; go to the target; count down temptation and jump.
  - IDLE (out of water, not mid-jump): 0 look at player 6 occasionally; 0 breeding; 1 temptation ×1.25; 2 start attacking the nearest prey (except when breeding); 3 look for land (radius 6, ×1.0); 4 one of (if no walk target): wander ×1.0 (1), walk towards the gaze (1), croak (3), nothing if on the ground (2).
  - SWIM (in water): same 0..2; 3 look for land (radius 8, ×1.5); 4 one of: swim ×0.75, wander towards water, walk towards the gaze, nothing (5).
  - LAY_SPAWN (pregnant): attack; look for land near a liquid of the tag `frog_tries_to_find_land_near` (radius 8); lay a frogspawn block (`frogspawn`) on the water at the edge; then wander / croak.
  - LONG_JUMP (out of water, timer finished, no temptation/breeding): jump up to 2 high, 4 wide, max speed 3.5714, prefers blocks of the tag `frog_prefer_jump_to` (probability 0.5); landing without fluid, avoids trapdoors; timer U[100, 140].
  - TONGUE (target present): approaches to < 1.75 blocks (speed ×0.75), tongue 10 ticks, swallows: normal hit (`attack_damage` 10); killed target removed without standard loot; unreachable target → blacklist 100 ticks; gives up after 100 ticks.
- Particularities: prey = entity tag `frog_food` (slimes and magma cubes of size 1 only). Magma cube eaten → froglight whose colour depends on the variant (via loot table). Breeding: no direct baby, the mother receives `is_pregnant` and lays spawn (tadpoles). Variants `temperate` (fallback), `warm` (tag `spawns_warm_variant_frogs`), `cold` (tag `spawns_cold_variant_frogs`); the tadpole turned frog takes the variant of its biome. Amphibious navigation.

##### `minecraft:camel` (creature)
- Attributes: HP 32; speed 0.09; `jump_strength` 0.42; `step_height` 1.5; safe fall 6; fall mult. 0.5 (equid base).
- Sensors: nearby living entities, hurt by, temptation, nearest adult. Memories: `temptation_cooldown_ticks`, `gaze_cooldown_ticks`, `is_panicking`, `walk_target`, `look_target`, `breed_target`, `nearest_visible_adult`.
- Activities (IDLE only):
  - CORE: swim 0.8; panic ×4.0 (stands up instantly); look at the target; go to the target; count down temptation and gaze.
  - IDLE: 0 look at player 6 occasionally; 1 breeding ×1.0; 2 one of: temptation ×2.5 (stop 2.5 baby / 3.5 adult; tag `camel_food` = cactus), baby follows adult (5..16) ×2.5 (except sitting); 3 look around 150..250; 4 one of (without walk target): wander ×2.0, walk towards the gaze ×2.0 (except sitting), sit/stand at random (if ≥ 20 s in the pose, out of water, not leashed, on the ground, without rider, space free), nothing 30..60.
- Particularities:
  - Sitting: 40-tick transition to sit, 52 to stand; refuses to move when sitting or in transition; stands up if in water or if the rider advances. Pose memorised (tick of last change, negative = sitting).
  - Ridden (saddled, 2 seats): speed = `movement_speed` + 0.1 if the rider sprints and no cooldown. Dash (rider's jump, saddled, on the ground, cooldown 0): horizontal impulse 22.2222 × power × speed × block factor + vertical 1.4285 × power × jump; cooldown 55 ticks; "dash" state ≥ 5 ticks then stops on the ground (< 50 remaining).
  - Cactus food: heal 2, in love if tamed adult, baby +10 % (age).
  - No random draw of attributes (unlike horses).

##### `minecraft:camel_husk` (monster)
- Attributes: identical to the camel (HP 32, speed 0.09, jump 0.42, step 1.5).
- AI: camel brain.
- Particularities: spawns with a husk (desert zombie): on the natural spawn of a husk, a single attempt per group, if the space is free, 10 % → the husk receives an iron spear, rides a camel_husk, and a "parched" (desert skeleton) rides as a passenger. Despawns far from players like a monster (persistent after a player's interaction). Controlled by the rider mob; cannot be leashed in that case. Food: tag `camel_husk_food`. Charge multiplier for spear riders: ×4.0.

##### `minecraft:happy_ghast` (creature)
- Attributes: HP 20; `tempt_range` 16; `flying_speed` 0.05; speed 0.05; `follow_range` 16; `camera_distance` 8.
- Hybrid AI: the BABY ("ghastling", scale 0.2375) uses a brain, the ADULT classic goals (switch on age change: goals and memories cleared).
  - Baby — sensors: nearby living entities, hurt by, temptation, nearest adult of any type, nearby players. Activities (PANIC > IDLE): CORE swim 0.8, panic ×2.0, look at the target, go to the target, count down temptation; IDLE 1 temptation ×1.25 (stop at 3, tag `happy_ghast_food`), 2 follow the nearest visible player (3..16) ×1.1, 3 follow the nearest visible adult (3..16) ×1.1, 4 one of: random flight ×1.0, go towards the gaze. Flight navigation.
  - Adult — goals: 3 float (own); 4 temptation (for non-pathfinders) ×1.0, range 7 — items of the tag `happy_ghast_tempt_items` (harness) if it has no harness, otherwise tag `happy_ghast_food`; 5 random floating (radius 16).
- Particularities:
  - Harness (body slot, adult): click without sneaking → the player mounts; 4 passengers max; the first player pilots if harnessed and not in "stop". Pilot input: sideways = strafe, forward = cos(pitch) / climb = −sin(pitch) (×0.5 when going backwards), +0.5 climbing if jump; all × 3.9 × `flying_speed`. Rotation: pitch × 0.5, pilot's yaw; smoothed turn 0.08.
  - "Stay still": a player (non-spectator, not mounted on a ghast) standing on top (box enlarged by 1 and half a height tall above) → stop timer 10 ticks, refreshed while they are there; counted down after 60 ticks of life; a passenger dismounting → 10 ticks; during the stop, precise position synchronised and no piloting.
  - Passive healing: 1 HP every 20 ticks in clouds or under precipitation, otherwise every 600.
  - Residence zone: without leash or passenger, centred on its position, radius 64 (adult without harness) or 32; recentred if it leaves by more than 16; cleared when the last passenger dismounts.
  - Leash: elastic distance 10, break 16; can also hold other entities on a leash (leash holder 5 ticks after a pull).
  - Food `happy_ghast_food` (snowball): growth of babies. Birth via the "dried ghast" block (out of scope). Immune to fall damage.

##### `minecraft:allay` (creature)
- Attributes: HP 20; `flying_speed` 0.1; speed 0.1; damage 2; `follow_range` 16 (mob base).
- Sensors: nearby living entities, nearby players, hurt by, nearby items. Key memories: `liked_player` (UUID), `liked_noteblock_position` (global position), `liked_noteblock_cooldown_ticks`, `item_pickup_cooldown_ticks`, `nearest_visible_wanted_item`, `is_panicking`, `walk_target`, `look_target`.
- Activities (IDLE only):
  - CORE: swim 0.8; panic ×2.5; look at the target; go to the target; count down note and pickup.
  - IDLE (in order): go to the wanted item ×1.75 (radius 32); go to the drop-off point ×2.25 and throw its items there (throw speed 0.2/0.3/0.2) when in range, then `item_pickup_cooldown_ticks` 60; stay near the drop-off point (4..16 blocks) ×2.25 if it sees no wanted item; look at an entity within 6 occasionally; one of: flight ×1.0 (2), go towards the gaze (2), nothing 30..60 (1).
  - Drop-off point: liked note block (above it) if in the same dimension within ≤ 1024 blocks, still a note block, and timer active; otherwise the liked player (same dimension, ≤ 1024).
- Particularities:
  - Giving an item (allay's hand empty) → it takes 1 copy, the player becomes "liked". Empty-hand click → returns the held item, drops its inventory (1 slot), forgets the player.
  - Only picks up items identical to the one held (same potion), if `mob_griefing` and room in the inventory; pickup range +1 block.
  - Note block: listens to vibrations (radius 16); a note block sound → memorises the position, timer 600 ticks (refreshed on each note of the same block; only notes of THIS block are then heard).
  - Dances near a playing jukebox (event radius); stops (check every 20 ticks) if the jukebox stops/moves away/disappears; no dancing while panicking.
  - Duplication: while dancing, item of the tag `duplicates_allays` (amethyst shard), if timer at 0 → new persistent allay, timer 6000 ticks for both.
  - Invulnerable to the liked player's hits; considers them an ally. Healing 1 HP every 10 ticks. Immune to fall damage.

##### `minecraft:copper_golem` (misc)
- Attributes: HP 12; speed 0.2; `step_height` 1.
- Sensors: nearby living entities, hurt by. Key memories: `transport_items_cooldown_ticks` (initialised to U[60, 99] at creation), `visited_block_positions` (10 max, 6000 ticks), `unreachable_transport_block_positions` (50 max), `gaze_cooldown_ticks`, `is_panicking`, `walk_target`, `look_target`.
- Activities (IDLE only):
  - CORE: panic ×1.5; look at the target; go to the target; open/close doors; count down gaze and transport.
  - IDLE: 0 transport items (if no timer, not panicking, not leashed); 1 look at player 6 occasionally (40..80); 2 one of (without walk target and timer active): wander ×1.0 (radius 2/2), nothing 30..60.
- Transport:
  - Empty hand → looks for a non-empty copper chest (tag `copper_chests`); full hand → looks for an empty chest or trapped chest or one already containing the same item. Search within 32 horizontal / 8 vertical (by chunks), the nearest not visited and not unreachable.
  - Queue if another golem interacts with the target within ≤ 3 blocks; interaction when its box touches the target's shape enlarged by 0.5 (1.0 if end of path); continues as long as ≤ 2. Interaction duration 60 ticks (chest opening at 1 tick, sound at 9).
  - Take: up to 16 items from a stack; deposit: fills identical stacks (same components) then empty slots; stays in hand if full.
  - Visited position memorised; when a valid target is found, visit memories cleared; if no target: timer 140 ticks.
- Particularities:
  - Oxidation: states unaffected → exposed → weathered → oxidized; next stage at U[504000, 552000] game ticks; waxed (honeycomb, marker −2) no longer oxidises; axe: removes the wax, otherwise goes back one stage; lightning: goes back one stage (once per lightning bolt).
  - Fully oxidised: every tick, if its block is air, 0.58 % to become an oxidised copper golem statue (random pose) — entity removed.
  - Antenna (saddle slot): can carry an item (flower); shears remove it if the item is of the tag `shearable_from_copper_golem`.
  - Creation: copper block + carved pumpkin (out of scope); oxidation state inherited from the block. Immune to fall damage.

#### Golems in singleplayer (F06, `mobs/golems/`)

- **Building from blocks**: when a carved pumpkin or a jack o'lantern is placed (block different
  from the old one), three patterns of one slice are searched in the order snow (pumpkin on 2 snow blocks),
  iron (T of 4 iron blocks, top corners in air), copper (pumpkin on a block of the tag `copper`). Search:
  for each position of `pos + [0, d−1]³` (x first, then y, then z; d = largest dimension), each of the
  24 frames (front, up) in the order of the directions down, up, north, south, west, east; a state read is kept
  for the whole search. Golems lying down or upside down are therefore born as in the game.
- Birth: pattern cells set to air (flag 2) with event 2001 of the state read (air included), golem
  placed at `cell + (0.5, 0.05, 0.5)`, yaw 0, without finalisation (no follow bonus nor left-handedness), then neighbours
  notified. Iron: bottom middle cell, "built by a player". Snow: lowest cell. Copper: pumpkin's cell;
  the copper block becomes the copper chest of the same oxidation (wax included), oriented
  like the pumpkin, linked to a neighbouring single copper chest of the same orientation (right first); a
  double chest takes the weakest oxidation, and if only one half is waxed both lose the wax;
  golem oxidation = that of the block; spawn sound.
- Dispenser: a carved pumpkin is only placed in front if the cell is air and a "base" pattern (without the pumpkin)
  is complete there.
- Metadata: iron 16 byte (bit 1 built); snow 16 byte (16 = pumpkin, default); copper 16 oxidation
  (serialiser 38), 17 interaction state (serialiser 37: rest, pickup, failed pickup, deposit, failed deposit).
  Events: iron 4 (hit), 11 (flower held out), 34 (flower put away); snowball 3 (shards).
- NBT: iron `PlayerCreated`, `anger_end_time`, `angry_at`; snow `Pumpkin`; copper `next_weather_age`
  (−1 to draw, −2 waxed, 0 oxidised), `weather_state`, a brain's memories (`is_panicking`, visited and
  unreachable positions with remaining duration), `drop_chances` (antenna and hand kept at 2.0).
- Copper golem brain, order of a tick: forgetting expired memories; sensors (nearby living entities and
  injury, every 20 ticks with an initial delay drawn on the entity's random generator when the brain is created);
  starting stopped behaviours by increasing priority (at equal priority, core then rest); tick of the
  running behaviours in the same order. Each start draws `min + next_int(max + 1 − min)` on the world's
  random generator, even when min = max (draw of `next_int(1)`). "One of" shuffles its two choices with its own
  random generator (−r, the largest first).
- Transport: on arrival (same tick as the switch to interaction) the counter goes to 1 → chest open
  (lid, chest sound, golem state); golem sound at tick 9; at tick 60 chest closed, pickup (stack
  of 16 at most, guaranteed drop) or deposit, then back to travel. Hurt golem → "rest" state.
- Iron: every base tick, if horizontal speed² > 2.5e−7, 1 chance in 5 of running particles (two
  draws of the entity's random generator when the block below is not invisible); never loses air underwater.
- Snow: melting according to the biome's attribute, failing that the dimension's (Nether); trail placed with update
  (flag 3).
- Remaining differences: village (POI, reputation, villagers wanting a golem) wired to neutral functions
  pending F05 (`sections_to_village` = outside village, reputation 0, no requesting villager) — return
  to the village and defence never trigger, "village" wandering falls back on any cell with
  the same draws; spawning by villagers (`summon_golem_for_villager`, "old" strategy) written
  but not called; doors not opened by the copper golem; panic without water search when it burns;
  order of equidistant chests (the game's hash table); snowballs: simplified collision (like
  arrows), held item not sent; client animations (cracks, flower, rotations) on the rendering side.

### 4.4 Aquatic mobs, villagers and trader

Source: decompiled 26.3, read on 2026-10-12, reworded. Default attribute values (reminder): speed 0.7, follow range 16, armour 0, knockback 0, damage absent unless stated.

#### Common "aquatic animal" base (fish, squid, dolphin outside its own breathing)
- Water path penalty = 0; not pushed by fluids; not leashable (except dolphin/turtle which redefine).
- Air out of water: -1 per tick; when air reaches ≤ -20 → 2 drowning damage and air reset to 0 (so 2 damage every 20 ticks after 15 s); in water air reset to 300.
- XP: 1 + rand(0..2).
- "Surface" spawn rule: y between (sea level - 13) and sea level, block below = water (fluid), block above = water.

#### Common "fish" base (cod, salmon, tropical_fish, pufferfish)
- Attributes: HP 3; rest default (speed 0.7).
- AI: 0 panic ×1.25; 2 flee player (radius 8, ×1.6 walk / ×1.4 sprint, excluding spectators); 4 random swim (×1.0, interval 40 ticks); schools: 5 follow the school leader. No targeting.
- Swim control: +0.005 upwards per tick if the eyes are in water; speed smoothed towards target (lerp 0.125); max rotation 90°/tick.
- Out of water, on the ground with vertical collision: "flopping" jump vy +0.4, horizontal ±0.05 random.
- Bucket: can be picked up with a water bucket (takes the data; flag "FromBucket" ⇒ persistent, never despawns). Otherwise despawns only if neither bucket nor name.
- Spawn group size: 8 (non-schooling fish).
- School (cod, salmon, tropical): spawn group = max school size; the 1st fish of the group becomes the leader, the following ones follow it. Leader: about every 1/200 ticks, if ≤ 1 fellow within ±8 blocks, school size reset to 1. Follower: "in range" if distance² ≤ 121 (11 blocks); path recomputed towards the leader every 10 ticks (×1.0). An isolated fish looks for a leader within ±8 blocks; delay between searches = 200 + (rand(0..199) % 20) ticks (reduced).

##### `minecraft:cod` (water_ambient)
- Attributes: fish base.
- AI: base + school (max size 8, default).
- Particularities: cod bucket; nothing else.

##### `minecraft:salmon` (water_ambient)
- Attributes: fish base.
- AI: base + school, max school size 5.
- Particularities: 3 sizes drawn at spawn, weighting small 30 / medium 50 / large 15; box scale 0.5 / 1.0 / 1.5; default medium. Size kept in the bucket.

##### `minecraft:tropical_fish` (water_ambient)
- Attributes: fish base.
- AI: base + school (max 8); a "rare" fish (see below) does not form a school (is not a candidate).
- Variants: packed integer = pattern (low 16 bits) | base colour << 16 | pattern colour << 24. Pattern = shape (small 0 / large 1) | index << 8. Small: kob 0, sunstreak 1, snooper 2, dasher 3, brinely 4, spotty 5. Large: flopper, stripey, glitter, blockfish, betty, clayfish (index 0..5). Default: white/white kob.
- Spawn: if the group already has a variant → the same; otherwise 90 % one of the 22 "common" variants (fixed list, shared by the group), 10 % pattern + 2 colours drawn uniformly (solitary fish, no school).
- Spawn rule: water below and above, and (biome tag `allows_tropical_fish_spawns_at_any_height` OR surface rule).
- 22 common (pattern, base, pattern colour): stripey orange/gray; flopper gray/gray; flopper gray/blue; clayfish white/gray; sunstreak blue/gray; kob orange/white; spotty pink/light_blue; blockfish purple/yellow; clayfish white/red; spotty white/yellow; glitter white/gray; clayfish white/orange; dasher cyan/pink; brinely lime/light_blue; betty red/white; snooper gray/red; blockfish red/white; flopper white/yellow; kob red/white; sunstreak gray/white; dasher cyan/yellow; flopper yellow/yellow.

##### `minecraft:pufferfish` (water_ambient)
- Attributes: fish base.
- AI: base + 1 "puff up" (active as long as a "scary" living entity is within ≤ 2 blocks of the enlarged box; ignores sight and invisibility). No school.
- Scary = any living entity except a creative player and except the entity tag `not_scary_for_pufferfish`.
- Puffing: state 0 small / 1 medium / 2 full (box scale 0.5 / 0.7 / 1.0). Puff counter: starts at 1 → state 1 immediately; > 40 ticks → state 2. Once the goal has stopped: deflation, state 2→1 after 60 ticks, 1→0 after 100 ticks (shared counter).
- Sting (if state > 0): every tick, mobs in the box enlarged by 0.3 (same conditions) and player in contact: damage 1 + state (2 or 3), poison level I duration 60 × state ticks (60 or 120). Player hit: game event packet "pufferfish sting" (if not silent).
- NBT: `PuffState` (capped at 2).

#### "Aquatic creature with age" base (squid, glow_squid, dolphin)
- Aged being (baby/adult, standard growth 24000 ticks), but fish breathing (except dolphin): air -1/tick out of water, ≤ -20 → 2 damage, reset to 300 in water. XP 1 + rand(0..2). Not pushed by fluids.
- "Surface" spawn rule identical to the fish's (y ∈ [sea-13, sea], water below and above).

##### `minecraft:squid` (water_creature)
- Attributes: HP 10; rest default.
- AI: 0 own random movement; 1 flee. No targeting.
- Random movement: every tick, if idle for > 100 ticks ("no action" counter) → zero vector; otherwise with probability 1/50 (reduced), or if it was not in water, or without a vector: new vector = uniform angle, horizontal 0.2 (cos, sin), vertical -0.1 + rand×0.2.
- Propulsion: tentacle phase advances by `tentacle_speed` per tick (speed = 0.2 / (1 + rand), redrawn with probability 1/10 at each 2π cycle, entity event 19 broadcast at each cycle); in the 1st half of the cycle, when phase/π > 0.75, the velocity is forced to the movement vector; in the 2nd half velocity ×0.9 per tick. Out of water: zero horizontal velocity, gravity (or levitation 0.05×(level+1)) × drag.
- Flee: active if in water and the recent attacker is within < 10 blocks (distance² < 100). Vector = position - attacker; if the target cell is water or air: intensity 3, reduced by (dist-5)/5 beyond 5 blocks; if air, vertical component cancelled; movement vector = flee / 20.
- Ink: on each damage taken from a mob (non-null attacker) → cloud of ink particles (server-side, particles broadcast) + sound.
- Babies: 5 % chance of baby at spawn ("age" group 0.05). No breeding food (no player breeding; breeding only through the generic age system). Leashable.

##### `minecraft:glow_squid` (underground_water_creature)
- Attributes: identical to the squid (HP 10).
- AI: identical to the squid.
- Particularities: "dimmed" 100 ticks after any damage (synchronised counter `DarkTicksRemaining`, decremented every tick, saved in NBT); glowing ink. Spawn: y ≤ sea level - 33, raw brightness 0 (without adjusted sky), block = water.

##### `minecraft:dolphin` (water_creature)
- Attributes: HP 10, speed 1.2, damage 3.
- AI: 0 breathe (if air < 140: swims to the first "breathable" block of the column up to +8 — without fluid or bubble column, and passable — otherwise towards +8; not interruptible); 0 look for a liquid (fluid tag `dolphin_tries_to_find`, if outside a liquid); 1 guide to treasure; 2 swim with player ×4.0; 4 random swim ×1.0 interval 10; 4 look around; 5 look at player 6; 5 jump (interval 10); 6 melee ×1.2 (pursuit even without sight); 7 go to item; 8 play with item; 8 follow an entity ridden by a player (boat); 8 same (nautilus); 9 flee guardian (8, ×1.0/×1.0).
- Targeting: 1 retaliation, calls nearby dolphins for help; does not retaliate against `guardian` (excluded).
- Breathing: max air 4800 (4 min); standard living-entity air (loss underwater, instant refill to the maximum as soon as it breathes); standard drowning (≤ -20 → 2 damage).
- Moistness: max 2400; reset to 2400 if in water or in rain, otherwise -1/tick; at ≤ 0 → 1 "drying out" damage per tick. Out of water on the ground: random jumps vy +0.5, horizontal ±0.2, random yaw. NoAI ⇒ air always full.
- Swimming in water: push = current speed; speed ×0.9 per tick; -0.005 downwards if no target.
- Jump ("breach"): probability 1/interval(10, reduced) per tick; requires, in the movement direction, at steps 0, 1, 4, 5, 6, 7: water without a block of the tag `blocks_dolphin_jump`, and air at heights +1 and +2. Impulse: direction ×0.6 horizontal, +0.7 vertical. Continues as long as not on the ground and not (vertical speed² < 0.03 with almost zero pitch in water).
- Feeding: fish (item tag `fishes`) on click → if baby: consumed, grows (standard acceleration); otherwise consumed and flag "has had a fish" (`GotFish`).
- Guide to treasure: condition = has had a fish and air ≥ 100. At the start, searches for the nearest structure of the tag `dolphin_located` (radius 50 chunks, not only unexplored ones: false); if nothing → blocked. Advances in steps: random point towards the target (16 h, 1 v, cone π/8), otherwise (8, 4, π/2), redone if not water (8, 5, π/2); speed ×1.3; restarts when within < 12 blocks of the point or path finished. Ends if within < 4 blocks (horizontally, at its height) of the treasure, or air < 100; flag cleared if reached/blocked/no target. Entity event 38 at the start and randomly (1/80) for the particles.
- Swim with player: nearest player within ≤ 10 blocks (no sight required) who is swimming and is not its target; continues as long as the player swims and within < 16 blocks; stops within < 2.5 blocks; gives "dolphin's grace" 100 ticks at the start then 1 chance in 6 per tick.
- Items: picks up items (main hand only, if empty). Go to item: living items on the ground, without pickup delay, in water, within ±8 blocks, speed ×1.2; on stop, if it was dropping an item → delay rand(0..99) ticks before starting again. Play: if it holds an item, drops it (thrown forward ~0.3 + small spread).
- Babies: 10 % at spawn ("age" group 0.1), scale 0.65. Full air at spawn. No food breeding. Leashable.
- NBT: `GotFish`, `Moistness`.

##### `minecraft:turtle` (creature)
- Attributes: HP 30, speed 0.25, step height 1.0 (animal base: follow range 16 by default).
- AI: 0 panic ×1.2 (first looks for water within ≤ 7 blocks, otherwise random point); 1 breeding ×1.0 (unless carrying an egg); 1 lay eggs ×1.0; 2 temptation ×1.1 (item tag `turtle_food`, not scared by movement); 3 go to water ×1.0 (×2.0 intended for the baby, but read at construction, when the turtle is not yet small: 1.0 for all); 4 return to home beach ×1.0; 7 travel at sea ×1.0; 8 look at player 8; 9 land wander ×1.0 interval 100. No targeting.
- Amphibious navigation; water penalty 0; doors (closed iron/wood, open) forbidden. Walk value: water 10 (except when returning to the nest), sand (the "on sand" test) 10, otherwise according to light. Stable destination: water during a trip, otherwise non-air block below.
- Swimming: push 0.1, ×0.9, -0.005 downwards except target or returning to nest within < 20 blocks of the nest. Control: +0.005 upwards in water; speed /2 (min 0.08) if > 16 blocks from the nest; baby /3 (min 0.06); on the ground /2 (min 0.06).
- Home beach (`home_pos`): spawn position (NBT, default = current position on load).
- Return to nest: never baby; always if carrying an egg; otherwise probability 1/700 (reduced) per tick and at > 64 blocks from the nest. Advances in steps towards the nest (16,3,π/10; otherwise 8,7,π/2; outside radius 16, if the point is not water: 16,5,π/2). End: within < 7 blocks, blocked, or > 600 ticks (adjusted) spent within < 16 blocks.
- Go to water: water block within ≤ 24 blocks (search starts at y-1); adult only if neither returning, nor egg, out of water; baby as soon as it is out of water; gives up after 1200 ticks; path recomputed every 160.
- Trip: in water, without return/egg; random target ±512 horizontal, ±4 vertical (vertical 0 if it would exceed sea-1); steps (16,3,π/10 / 8,7,π/2); cancelled if the chunks ±34 around the point are not loaded; ends if path finished, blocked, return, in love or egg.
- Breeding: food `turtle_food` (seagrass). Cannot fall in love if carrying an egg. Mating does NOT create a baby: the turtle gets "carries an egg" (`has_egg`), both get age 6000 (wait), love reset to 0, XP orb 1..7 (if drops rule).
- Laying: if egg and within < 9 blocks of the nest; looks for sand (turtle egg sand tag) with air above, radius 16. Arrival out of water → "laying"; counter +1/tick; every 5 ticks block particles (level event 2001) + game event; after > 200 ticks (adjusted): `turtle_egg` block above the sand with 1..4 eggs, no more egg, love 600 ticks.
- Baby: scale 0.3; on becoming an adult → gift loot table "turtle growth" (scute) if drops rule.
- Lightning: infinite damage (dies). Not leashable. Not pushed by fluids.
- Spawn: y < sea + 4, on sand (turtle egg rule), sufficient light (animal rule).

##### `minecraft:axolotl` (axolotls)
- Attributes: HP 14, speed 1.0, damage 2, step height 1.0 (animal base).
- Brain. Sensors: nearby living entities, nearby adult, hurt by, axolotl's attackables, food temptations.
- Activities (chosen every tick, in order: play_dead > fight > idle; play_dead is not left by this choice):
  - core: look at the gaze target (45/90 ticks), move towards the walk target, validate play-dead (`play_dead_ticks` countdown -1/tick; at 0 clears this counter and `hurt_by_entity` then default activity), `temptation_cooldown_ticks` countdown.
  - idle: 0 look at a player ≤ 6 blocks from time to time (every 30..60 ticks); 1 mating (×0.2, distance 2); 2 one of { follow temptation (×0.5 water / ×0.15 ground), baby follows adult (range 5..16, ×0.6 / ×0.15) }; 3 start attacking (memory `nearest_attackable`, except while breeding); 3 look for a liquid (radius 6, ×0.15, fluid tag `axolotl_tries_to_find`); 4 if no walk target: a weighted random one among random swim ×0.5 (2), random walk ×0.15 (2), walk towards the gaze target (×0.5/0.15, close 3, only if water/land identical to its own) (3), do nothing in water (5), do nothing on the ground (5).
  - fight (requires `attack_target`): stops if target invalid (on stopping, if the target died from a hit by a player present within ≤ 20 blocks → support effects); go towards the target if out of range (×0.6 water / ×0.15 ground); melee, cooldown 20 ticks; clears the target if breeding.
  - play_dead (requires `play_dead_ticks`, clears this counter on exit): 0 play dead (also requires `hurt_by_entity`; only in water; clears walk and gaze targets; regeneration I 200 ticks; max duration 200); 1 clears `play_dead_ticks` if breeding.
- On leaving fight (towards something else): `has_hunting_cooldown` = true for 2400 ticks.
- Attackables sensor: living entity within ≤ 8 blocks (dist² ≤ 64), visible, IN water, attackable, and (tag `axolotl_always_hostiles`, or tag `axolotl_hunt_targets` if no hunting cooldown) → `nearest_attackable`.
- Playing dead (triggered on each damage): 1 chance in 3, and (rand(0..2) < damage or HP < 50 %), damage < current HP, in water, source with an entity, not already doing so → `play_dead_ticks` = 200. During this time: not seen as an enemy (mobs no longer target it), swim control frozen. Synchronised flag "playing dead".
- Support effects for the player: if the player has no regeneration or it ends in < 2399 ticks → regeneration I of min(2400, 100 + remaining duration); always removes mining fatigue.
- Air: max 6000; out of water AND out of rain -1/tick, ≤ -20 → 2 "drying out" damage, air to 0; otherwise air at max. "Rehydrate" (+1800, capped) when a player splashes it (called elsewhere: water potion). NoAI ⇒ no air handling.
- Swimming: push = speed, ×0.9; smooth swim control (max rotation 85, pitch 10, water acceleration 0.1, air 0.5). Water penalty 0.
- Variants: lucy 0, wild 1, gold 2, cyan 3 (common), blue 4 (rare). Spawn: group of 2 common variants drawn for the group, each one picks one of the two; from the 3rd member (group size ≥ 2 already) → baby (age -24000). Spawn from a bucket: nothing is drawn (bucket data).
- Breeding: food tag `axolotl_food` (tropical fish bucket, returns a water bucket). Baby: 1/1200 → rare variant (blue), otherwise variant of a random parent; persistent baby.
- Bucket: can be picked up; the bucket keeps variant, `Age`, `AgeLocked`, `HuntingCooldown` (remaining duration). Persistent if "FromBucket". Despawns only if neither bucket nor name.
- Spawn: block below in the tag `axolotls_spawnable_on`.

##### `minecraft:tadpole` (creature; does not spawn naturally, hatches from frogspawn)
- Attributes: HP 6, speed 1.0 (animal base).
- Inherits the "fish" base: goals 0 panic ×1.25, 2 flee player 8 (×1.6/×1.4), 4 random swim; fish breathing (air out of water, 2 damage at ≤ -20); flopping on the ground; bucket. No school. In addition, a brain:
- Sensors: nearby living entities, nearby players, hurt by, "frog" temptations.
- Activities: core = animal panic ×2.0, look at target (45/90), go to the target, `temptation_cooldown_ticks` countdown; idle = 0 look at a player ≤ 6 from time to time (30..60 ticks); 1 follow temptation ×1.25; 2 if no walk target: random swim ×0.5 (2), walk towards gaze target ×0.5 close 3 (3), nothing if in water (5). Chosen activity: always idle.
- Water-bound navigation; smooth swim control (85, 10, water 0.02, air 0.1, applies gravity); smooth look 10.
- Growth: own age (NOT the animals' age), +1 per tick server-side unless locked; at ≥ 24000 → conversion into `minecraft:frog` (simple conversion, equipment not kept), the frog goes through spawn initialisation (reason "conversion", hence variant according to biome), persistent, position adjusted.
- Feeding: tag `frog_food` (slime ball) if not locked → consumed, age += (remaining/20 × 0.1) s × 20 ticks (i.e. +10 % of the remainder), "happy villager" particles.
- Age lock: golden dandelion (if baby, particle cooldown at 0, type outside the tag `cannot_be_age_locked`) → toggles the lock, age reset to 0, cooldown 40 ticks.
- NBT/bucket: `Age`, `AgeLocked`.

#### "Nautilus" base (nautilus, zombie_nautilus) — tameable animal, jumping mount
- Base attributes: HP 15, speed 1.0, damage 3, knockback resistance 0.3 (+ animal base).
- Water-bound navigation; walk value 0 everywhere; water penalty 0; not pushed by fluids. Swimming: push = speed, ×0.9. Smooth swim control (85, 10, water 0.011, air 0, gravity).
- Natural spawn (nautilus): y ∈ [sea-25, sea-5], water below and above, no entity obstruction.
- Initial memories: `attack_target_cooldown` = 2400..3600 ticks (drawn). Sensors: nearby living entities, nearby adult, nearby players, hurt by, "nautilus" temptations.
- Attack choice (common): never if breeding, out of water, baby or tamed. Priority to `angry_at` (entity in water, attackable even without sight). Otherwise, if `attack_target_cooldown` present → nothing; otherwise cooldown removed (2400..3600) and 50 %: nothing, 50 %: the nearest visible one, in water, of the entity tag `nautilus_hostiles`.
- Anger: on each damage taken from an attackable living entity → `angry_at` = it for 400 ticks (clears `cant_reach_walk_target_since`).
- Charge (fight, requires target, neither temptation, nor partner, nor `charge_cooldown_ticks`): vector = direction towards the target × charge speed, imposed every tick; looks at the target; on the 1st living entity touched (combat conditions: armor_stand excluded if mob_griefing false, inside the border; never its own passenger) → damage = damage attribute, post-attack enchantment effects, extra knockback = clamp(charge_speed × attr_speed, 0.2, 2.0) + 0.25×(Speed level - Slowness level), × strength 2.0; end. Also stops if tamed, if ≥ 12 blocks travelled, target at ≥ 11 blocks, sight lost. At the end: `charge_cooldown_ticks` = 80 and target cleared.
- Taming (untamed adult): item of the tag `nautilus_taming_items` → consumed, 1 chance in 3 to tame (event 7, navigation stopped), otherwise event 6. Tamed: food `nautilus_food`; healing if hurt = 2 × nutrition (or 1 if inedible); sneak + click = inventory; simple click = mount.
- Equipment: saddle and body slots (nautilus armour) usable only on a living tamed adult; dispenser can equip saddle/body. Passenger: only one. Controller = player if saddled.
- Mount: input = player's sideways; forward/backward according to pitch (forward = cos(pitch), vertical = -sin(pitch); reverse ×-0.5); rotation: yaw catches up 50 % of the gap per tick, pitch = half of the player's. Ridden speed = 0.0325 × speed (water) or 0.02 × speed (land).
- Dash (mount jump): if saddled and cooldown at 0; impulse = gaze direction × (1.2 in water / 0.5 on land) × strength × speed × block speed factor; cooldown 40 ticks, synchronised "dash" flag, lowered when cooldown < 35.
- Player passenger: "nautilus's breath" effect 60 ticks (ambient, visible) given if it lacks it or every 40 game ticks.
- Residence zone (tamed, not leashed, without passenger): radius 32 if adult without saddle, otherwise 16; recentred if no longer within < radius + 8 of the centre or radius changed.
- Immune to poison. Can despawn except when tamed (persistent). Leash: forbidden if angry/attacking.

##### `minecraft:nautilus` (water_creature)
- Attributes: base (HP 15, speed 1.0, damage 3, knockback 0.3).
- Brain, activities: core = panic ×1.6, look at target (45/90), go to target, countdowns `temptation_cooldown_ticks`, `charge_cooldown_ticks`, `attack_target_cooldown`; idle = 1 mating ×0.4 distance 2; 2 follow temptation ×1.3 (stops at 2.5 baby / 3.5 adult); 3 start attacking (base rule); 4 if no walk target: random swim ×1.0 (2) or walk towards gaze target ×1.0 close 3 (3); fight = charge (cooldown 80, speed 0.6, knockback 2.0, 12 / 11 blocks). Choice: fight > idle.
- Air: max 300; out of water -1/tick, ≤ -20 → 2 "drying out" damage and air 0; in water reset to 300.
- Breeding: food `nautilus_food`; baby tamed if the parent is (same owner). Baby scale 0.5.
- Extra memories: `angry_at`, `attack_target_cooldown`.

##### `minecraft:zombie_nautilus` (monster)
- Attributes: base but speed 1.1.
- Brain: core = same as nautilus WITHOUT panic; idle = 1 follow temptation ×0.9 (2.5/3.5); 2 start attacking (same rule as the nautilus); 3 random swim ×1.0 / walk towards gaze target ×1.0; fight = charge (cooldown 80, speed 0.5, knockback 2.0, 12 / 11). No mating.
- No baby, no breeding. Burns in sunlight like the undead, protected by the BODY slot (nautilus armour) and not by a helmet.
- Variants (data registry): `temperate` (fallback, priority 0) and `warm` (coral model, biomes of the tag `spawns_coral_variant_zombie_nautilus`, priority 1); chosen at spawn according to the biome.
- Spawn: no natural spawn of its own; drowned's mount: on the natural or structure spawn of an adult drowned armed with a trident, 50 % chance, outside biomes of the tag `more_frequent_drowned_spawns` → a zombie_nautilus is created under it (persistent if structure).
- Leash also forbidden if ridden by a mob.

##### Useful entity tags (26.3 data)
- `can_breathe_under_water`: #undead, axolotl, frog, guardian, elder_guardian, turtle, glow_squid, cod, pufferfish, salmon, squid, tropical_fish, tadpole, armor_stand, copper_golem, nautilus (⇒ no drowning underwater for them; zombie_nautilus through #undead if present in this tag).
- `burn_in_daylight`: …, drowned, zombie_nautilus, … (burning in sunlight handled by the generic mob, protection slot specific to the type).
- `axolotl_hunt_targets`: tropical_fish, pufferfish, salmon, cod, squid, glow_squid, tadpole. `axolotl_always_hostiles`: drowned, guardian, elder_guardian.
- `nautilus_hostiles`: pufferfish.
- `not_scary_for_pufferfish`: turtle, guardian, elder_guardian, cod, pufferfish, salmon, tropical_fish, dolphin, squid, glow_squid, tadpole, nautilus, zombie_nautilus, sulfur_cube.

#### `minecraft:villager` (misc; spawns through village generation, breeding, curing, egg)
- Attributes: speed 0.5; rest default (HP 20, follow range 16). Base speed of behaviours: ×0.5.
- Navigation: opens doors, floats, required path length 48. Picks up items (8-slot inventory). Never despawns.
- Baby size 0.49 × 0.98.

##### Brain: sensors
- nearby living entities, nearby players, nearby items, nearest bed, hurt by, villager hostiles, baby villagers, secondary POIs, detected golem.

##### Schedule ("villager_schedule" timeline, period 24000 ticks on the world clock; activity drawn from the environment attribute)
- Adult: 10 idle; 2000 work; 9000 meet; 11000 idle; 12000 rest (until the next day 10).
- Baby: 10 idle; 3000 play; 6000 idle; 10000 play; 12000 rest.
- Default outside the timeline: idle. The activity change is applied by the behaviour "update the activity according to the schedule" (priority 99 in work/play/rest/meet/idle) and on (re)loading of the brain.
- Available activities: adult = work (requires `job_site`), core, meet (requires `meeting_point`), rest, idle, panic, pre_raid, raid, hide; baby = play instead of work. The brain is rebuilt when the baby becomes an adult, on loading and on profession change.

##### Activities and behaviours (priority: behaviour)
- core (always active): 0 swim (80 % chance to jump per tick in a liquid); 0 open/close doors; 0 look at the target (45/90); 0 panic trigger; 0 wake up; 0 react to the bell; 0 set the raid state; 0 validate the held job POI (`job_site`); 0 validate the potential job POI (`potential_job_site`); 1 go to the walk target; 2 scan for POI competitors; 3 look at and follow the trading player (×0.5); 5 go to wanted item (×0.5, radius 4); 6 acquire a job POI (acquirable types, → `potential_job_site`, with reservation); 7 go to the potential workstation (×0.5); 8 give up its workstation (×0.5); 10 acquire a bed (POI home, → `home`, adults and babies, entity event 14 on success, valid bed = block tag `villagers_can_sleep_on_bed` and unoccupied); 10 acquire a meeting point (POI meeting = bell, → `meeting_point`, adults only, event 14); 10 take the profession of the workstation; 10 lose the profession.
- work (adult, requires `job_site`): 5 a weighted random one among { work at the POI (farmer: composter) 7; wander around the workstation ×0.4 radius 4 → 2; wander towards the workstation ×0.4 (1..10) → 5; wander towards a secondary POI ×0.5 (1..6) → 5; harvest fields → 2 (farmer) / 5; bone meal → 4 (farmer) / 7 }; 10 show its trades to the player (400..1600 ticks); 10 look at and interact with player (≤ 4); 2 walk to the workstation (×0.5, close 9, gives up > 100 blocks or 1200 ticks without reaching); 3 gift to the hero (delay 100); 5 minimal gaze; 99 schedule.
- play (baby): 0 go to the target (duration 80..120); 5 full gaze; 5 play tag with other children; 5 if no baby visible: one of { interact with villager ≤ 8 (×0.5, close 2) 2; with cat 1; village-bound wander 1; walk towards gaze target (close 2) 1; jump on a bed 2; nothing 20..40 ticks 2 }; 99 schedule.
- rest: 2 walk to `home` (×0.5, close 1, gives up 150 blocks/1200 ticks); 3 validate the home POI; 3 sleep in the bed; 5 if no `home`: one of { go to the nearest free bed (×0.5) 1; Brownian walk indoors 4; go to the nearest village (×0.5, close 4) 2; nothing 20..40 → 2 }; 5 minimal gaze; 99 schedule.
- meet (requires `meeting_point`): 2 a random one (shuffled trigger) among { wander around the point ×0.4 radius 40; socialise at the bell }; 10 show trades; 10 look at/interact with player ≤ 4; 2 walk to the point (×0.5, close 6, gives up 100/200); 3 gift to the hero; 3 validate the meeting POI; 3 if `interaction_target`: trade with villager; 5 full gaze; 99 schedule.
- idle: 2 one of { interact with villager ≤ 8 (2); go to a villager able to breed, if self can (→ `breed_target`) (1); interact with cat (1); village-bound wander (1); walk towards gaze target (1); jump on a bed (1); nothing 30..60 (1) }; 3 gift to the hero; 3 look at/interact with player ≤ 4; 3 show trades; 3 trade with villager (if `interaction_target`); 3 breed (if `breed_target`); 5 full gaze; 99 schedule.
- panic (flight speed ×0.75 = 0.5 × 1.5): 0 calm down; 1 flee `nearest_hostile` (×0.75, radius 6); 1 flee `hurt_by_entity` (same); 3 village-bound wander ×0.75 (2, 2); 5 minimal gaze.
- pre_raid: 0 ring the bell; 0 a random one: walk to `meeting_point` ×0.75 (close 2, 150/200) weight 6, or wander ×0.75 weight 2; 5 minimal gaze; 99 reset the raid state.
- raid: 0 if a raid here is won: go to a spot seeing the sky ×0.5 (5) or wander ×0.55 (2); 0 celebrate the victory (600..600 ticks); 2 if raid active (neither won nor lost): look for a hiding place (radius 24, ×0.7, close 1); 5 minimal gaze; 99 reset the raid state.
- hide: 0 hidden state (duration 15 s, radius 3); 1 look for a hiding place (radius 32, ×0.625, close 2); 5 minimal gaze.
- Full gaze = one of: cat 8 blocks (8), villager (2), player (2), categories creature / water_creature / axolotls / underground_water_creature / water_ambient / monster (1 each), nothing 30..60 (2). Minimal gaze = villager (2), player (2), nothing 30..60 (8).

##### Villager profession, workstation and POI
- Villager data (`VillagerData`): type (biome), profession, level 1..5. XP thresholds to reach the next level: 10 (→2), 70 (→3), 150 (→4), 250 (→5).
- Type: at spawn according to the biome (desert/badlands → desert; jungles → jungle; savannas → savanna; frozen/snowy/peaks biomes → snow; swamp/mangrove → swamp; taigas/windswept hills/windswept forest → taiga; rest → plains). Spawn by breeding: profession reset to `none`.
- Professions and workstation (POI): armorer = blast furnace; butcher = smoker; cartographer = cartography table; cleric = brewing stand; farmer = composter; fisherman = barrel; fletcher = fletching table; leatherworker = cauldrons (empty/water/lava/powder snow); librarian = lectern; mason = stonecutter; shepherd = loom; toolsmith = smithing table; weaponsmith = grindstone. `none`: no workstation held, can acquire any workstation of the tag `acquirable_job_site`. `nitwit`: no workstation, never acquires one.
- Farmer: requested items (picked up) wheat, wheat seeds, beetroot seeds, bone meal; secondary POI = farmland.
- Trades: each profession has a set of trades per level 1..5 (data registry `trade_set`); on each level-up, the offers of the new level are ADDED.
- Acquiring a workstation (core 6, only if no `job_site`, adult, and no `potential_job_site` already): scheduled every 20 + rand(0..19) ticks (first attempt within rand(0..19)); looks for the 5 nearest POIs of the wanted type, within a radius of 48, HAVING a free slot, not marked "retry not yet due"; creates a path to the set (tolerance = largest validity radius of the types); if reachable: takes a ticket on the targeted POI, memory = global position, entity event (14 for bed and bell), clears the retry cache; otherwise each candidate POI receives a retry marker: delay = previous delay + 40..79, capped at 400 ticks; marker forgotten 400 ticks after the last attempt.
- Bed (core 10, adults AND babies) → `home`; valid bed = block tag `villagers_can_sleep_on_bed` and unoccupied. Bell (core 10, adults) → `meeting_point`.
- Go to the potential workstation (core 7): only in idle/work/play activity; sets walk/look target on the workstation (×0.5, close 1) every tick; max duration 1200 ticks; on stop, releases the ticket of the potential POI and clears the memory.
- Taking the profession (core 10): when within < 2 blocks of the potential workstation → `job_site` = this workstation, `potential_job_site` cleared, event 14; if without profession, takes the 1st profession (registry order) whose held workstation matches, and rebuilds the brain.
- Losing the profession (core 10): without `job_site`, if profession ≠ none/nitwit, XP = 0 and level ≤ 1 → profession `none`, brain rebuilt. (A villager who has traded keeps its profession even without a workstation.)
- Giving up the workstation (core 8): a jobless adult aiming at a potential workstation gives it to the 1st nearby living villager (nearby living entities memory) that has no potential workstation, whose profession holds this POI type, and (without `job_site`: can reach it; with: it is already its own). It clears walk/look/potential; the receiver, if it has no `job_site`, receives `potential_job_site` + walk target (×0.5, close 1).
- Competition (core 2): among nearby living villagers having the SAME `job_site` and a profession that holds it, the one with the most XP wins (tie: the second examined wins); the losers forget `job_site`.
- Validation (core 0, rest 3, meet 3): if the memorised POI is within < 16 blocks and in the same dimension: cleared if the POI no longer exists (type); for an occupied bed (block occupied and self not asleep) → cleared, and if nobody is really sleeping in it, the ticket is released.
- Death / conversion into a witch: releases the 4 POIs (home, job_site, potential_job_site, meeting_point) if the type still matches.
- Working at the workstation (work, weight 7): 300-tick cooldown between checks, 50 % chance, requires being within < 1.73 blocks of the workstation; memorises `last_worked_at_poi` = now, looks at the workstation, profession sound, workstation-specific action, then restocking if due.
- Farmer at the composter: (1) bread: if ≤ 36 breads, crafts min(3, wheat/3) breads (3 wheat each), surplus dropped on the ground; (2) if composter full (level 8) collects the bone meal; then composts at most 20 seeds (wheat/beetroot), keeping at least 10 of each kind (inventory walked in reverse), stops at level 7; level event 1500 (filled or not).
- Harvesting (work, farmer only, rule mob_griefing true, requires `secondary_job_site`, no walk/look target): cells of the surrounding 3×3×3 cube = ripe crop, or air on farmland; picks one at random, walks to it (×0.5); within < 1 block: breaks the ripe crop (with drops); if air on farmland and it holds seeds of the tag `villager_plantable_seeds` → plants the 1st found (planting sound); if the cell contains an unripe crop → moves on to another cell (pause 20 ticks); max duration 200 ticks; on stop, pause 40 ticks.
- Bone meal (work): checks every 10 ticks of existence and ≥ 160 ticks after the last session; if it has bone meal and an unripe crop in the 3×3×3 cube (uniform choice by reservoir sampling); holds the bone meal in hand; within < 1 block, makes it grow (event 1505), moves on to the next target, 40 ticks between two; max duration 80 ticks.
- Secondary POI sensor: every 40 ticks, cube ±4 horizontal, ±2 vertical: blocks of the profession's "secondary POI" → `secondary_job_site` (list) otherwise cleared.
- "Walk to a memorised POI" (generic; work 2: job_site close 9 / far 100 / 1200 ticks; rest 2: home 1 / 150 / 1200; meet 2: meeting_point 6 / 100 / 200; pre_raid: meeting_point 2 / 150 / 200): only without a walk target. If another dimension, or unreachable for longer than the time limit (`cant_reach_walk_target_since`) → releases the POI, clears the memory, `cant_reach_walk_target_since` = now. If Manhattan distance > "far": intermediate point towards the POI (15 h, 7 v, cone π/2) until one is found at ≤ "far" (1000 attempts, otherwise same give-up as above). Otherwise, if > "close": direct walk.

##### Trades, XP, restocking (twice a day)
- Player click: ignored (generic behaviour) if a villager egg is in hand, dead, already trading or asleep. Baby: "unhappy" (synchronised 40-tick counter, "no" sound). Adult without offers: unhappy (main hand only); otherwise opens the trade (stat "talked to villager" on main hand). Offers generated on the 1st consultation if absent.
- Generation: the profession's trade set for the current level (`trade_set`), number of offers drawn by the set, random draw without replacement of the trades (or with duplicates if the set allows), loot context "villager trade".
- On each trade: uses +1; villager XP += offer's XP; XP orb 3..6 (+5 on level-up) if the offer rewards; if XP ≥ next level threshold (level < 5) → level +1, new offers added, regeneration I 200 ticks. On the next AI tick: reputation event TRADE for the player, entity event 14.
- Price: cost A = base + max(0, ⌊base × demand × multiplier⌋) + special difference, clamped to [1, max stack size]. Special difference recomputed on opening (and after any change of reputation/gossip/restock during the trade): -⌊reputation × multiplier⌋; if "hero of the village" effect of level n: -max(1, ⌊(0.3 + 0.0625 × n) × base⌋).
- Demand at each update: demand += uses - (max - uses).
- Restocking: only during "work at the workstation" (hence within < 1.73 blocks of the workstation, in work activity). Allowed if 0 restocks today, or < 2 and more than 2400 ticks since the last one; and at least one offer has been used. Effect: demand update, uses reset to 0, day counter +1, date of last restock = now.
- New "day": if > 12000 ticks since the last restock, or if the number of elapsed days of the `overworld_day` timeline has increased since the last check. Then: catch-up (if fewer than 2 restocks done: uses reset to 0, then (2 - restocks) demand updates), day counter reset to 0, date of last restock = now.
- Showing its trades (work/meet/idle): if the interaction target is a living player at distance² ≤ 17 and self is an adult: looks at the player; if the player holds an item that matches cost A or B of non-exhausted offers → holds the result in hand (drop chance 0), cycles every 40 ticks if several, look duration raised to 900; otherwise hand emptied (drop chance 0.085) and look ≤ 40 ticks. Duration 400..1600 ticks. Clears `interaction_target` at the end.
- Follow the trading player (core 3): looks at and follows the trading player (×0.5).

##### Gossip and reputation
- Types (weight, max, loss/day, loss on transfer): major_negative (-5, 100, 10, 10); minor_negative (-1, 200, 20, 20); minor_positive (+1, 25, 1, 5); major_positive (+5, 20, 0, 20); trading (+1, 25, 2, 20).
- A player's reputation = Σ value × weight over all types.
- Events: zombie-villager cure → +20 major_positive and +25 minor_positive; trade → +2 trading; villager hurt by X → +25 minor_negative for X (all villagers concerned by the reputation event; entity event 13 if X is a player); villager killed by X → +25 major_negative on each witness (entities visible from the dead one receiving the reputation events).
- Adding: if the sum exceeds the max, value = max(max, old); a value < 2 is deleted.
- Decay: every 24000 game ticks (per villager), each value -= loss/day; < 2 → deleted.
- Gossip exchange (during "trade with villager", at distance² ≤ 5): only if neither has chatted for 1200 ticks; the villager draws 10 gossips from the other (draw weighted by |value × weight|, with replacement but without duplicates); value received = value - transfer loss, kept if ≥ 2, merge = max(old, new). Then golem attempt (5 agreements required).
- Max 10 gossip subjects (constant present, use not seen); NBT `Gossips`, `LastGossipDecay`.

##### Item exchange between villagers (same behaviour as gossip)
- At distance² ≤ 5, every tick: if the villager has ≥ 24 food points and (it is a farmer or the other has < 12) → throws half a stack of food; farmer with > 32 wheat → throws half a stack of wheat; items requested by the other's profession and not by its own → throws half a stack. "Half a stack": 1st matching stack of more than half a max stack → half; otherwise of more than 24 → the excess beyond 24. Thrown towards the other's position.

##### Food, breeding, beds
- Food = items carrying the `villager_food` component (value per item, read from the 26.3 item registry: bread 4, carrot 1, potato 1, beetroot 1). Internal food level `FoodLevel` (byte).
- Can breed: internal food + points in the inventory ≥ 12, not asleep, age 0.
- Pickup: items of the tag `villager_picks_up`, or with `villager_food`, or requested by the profession, if the inventory has room (8-slot inventory). Go to wanted item: ×0.5, radius 4.
- Breeding (idle 3, requires valid villager `breed_target`, both can breed): duration 350 ticks; birth planned after 275..324 ticks; they walk towards each other (×0.5, close 2), events 18 at the start, 12 random (1/35); when due, if distance² ≤ 5: each "eats until full" (consumes its inventory food up to 12) then loses 12; then a FREE BED is required: takes a ticket on a reachable home POI within a radius of 48; without a bed → events 13 (anger), no baby; with a bed → baby (age -24000) placed on the parent, parents age 6000, event 12, the baby receives `home` = this bed (if creation fails, ticket released).
- Baby: type = 50 % local biome, 25 % parent 1, 25 % parent 2; profession none; data finalised.
- Sleeping (rest 3): requires `home` in the same dimension, not a passenger, not woken for 1..99 ticks, within < 2 blocks of the bed, valid unoccupied bed block; first closes the doors it opened; lies down, `last_slept` = now; continues as long as the rest activity is active, y > bed + 0.4 and within < 1.14 of the bed; on stop gets up, pause 40 ticks. Waking (core 0): if the activity is no longer rest. Getting up → `last_woken` = now.
- Rest without `home`: go to the nearest bed (sensor "nearest bed": babies only, every 20 ticks, 5 candidates per batch within a radius of 48, cache 40 ticks), indoor Brownian walk, go to the nearest village (close 4), or nothing.
- Jump on a bed (idle/play, babies only, requires `nearest_bed` and no walk target): walks to the bed (close 0); 100 ticks max to reach it; on a block of the tag `villager_babies_can_jump_on_bed`: 3..6 jumps, 5 ticks between two.

##### Panic and golem
- Hostiles sensor (`nearest_hostile`, nearest visible): drowned 8, evoker 12, husk 8, illusioner 12, pillager 15, ravager 12, vex 8, vindicator 10, zoglin 10, zombie 8, zombie_villager 8 (blocks).
- `hurt_by` = last damage source (the game forgets it 40 ticks after the last hit); `hurt_by_entity` = living author (cleared if it dies/changes world).
- Trigger (core 0): if hurt or hostile nearby → clears path, walk/look/partner/interaction target (if not already panicking) and switches to panic. While it runs: every 100 game ticks (time % 100 = 0) golem attempt with 3 agreements.
- Calming down (panic 0): no more `hurt_by`, no more hostile and the author not within ≤ 6 blocks → clears `hurt_by`/`hurt_by_entity`, goes back to the schedule.
- Flight: move away from the hostile or the author (×0.75, distance 6), otherwise village-bound wander ×0.75.
- Golem: a villager "wants a golem" if it slept less than 24000 ticks ago (`last_slept`) and does not have `golem_detected_recently`. If so: villagers within ±10 blocks, at most 5 that want a golem are taken; if there are ≥ N (5 by gossip, 3 in panic) → attempt to spawn an `iron_golem` (reason "summoned by mob"): 10 attempts, offset ±8 horizontal, vertical search from +6 down to -6, solid ground (or powder snow) with air/liquid above, outside glass/stained glass/leaves/cobweb/cactus/conduit/ice/TNT/glowstone/beacon/sea lantern; spawn rules and the golem's non-obstruction. Success → all villagers in the area receive `golem_detected_recently` (599 ticks).
- Golem sensor: every 200 ticks, if an iron golem is among nearby living entities → `golem_detected_recently` 599 ticks.

##### Bell, raid, hiding place
- Bell rung (by anyone): all living entities within ≤ 32 blocks receive `heard_bell_time` (search ±48 at most every 60 ticks); raiders within ≤ 48: glowing 60 ticks (at the end of the bell animation).
- React to the bell (core 0): if `heard_bell_time` and no raid here → hide activity.
- hide: hiding place = bed (home POI) within ≤ 2 blocks, otherwise a random bed within a radius of 32 (occupied or not), otherwise its `home`; clears path/look/partner/interaction, `hiding_place` = position, walks ×0.625 (close 2). Stays hidden 15 s (300 ticks counted only within < 3 blocks of the hiding place); gives up if 300 ticks since the bell; then clears bell + hiding place and goes back to the schedule.
- Raid state (core 0, 1 chance in 20 per tick): raid here ⇒ if 1st wave spawned and not between two waves → default activity raid, otherwise pre_raid. Reset (pre_raid/raid 99, 1/20): no raid, raid stopped or lost → default idle + schedule.
- pre_raid: ring the bell (if `meeting_point` within < 3 blocks: with probability 5 % per tick, rings a real bell) + gathering at the bell.
- raid: hiding place as above (radius 24, ×0.7, close 1) as long as the raid is active; victory → go out to see the sky and celebrate (600 ticks).
- Entity event 42 (1/100 per AI tick) when an active unfinished raid is here (anger particles client-side).

##### Gift to the hero (work/meet/idle 3)
- If the nearest visible player has "hero of the village": initial countdown 600 ticks (decremented only while the hero is visible), then goes to them (×0.5, close 5); within < 5 blocks and after 20 ticks of head rotation, throws the gift loot: the profession's table (`armorer_gift`… `weaponsmith_gift`), `baby_villager_gift` for a baby, `unemployed_gift` otherwise (none/nitwit). Next delay 600..6600 ticks; max duration 100.

##### Lightning and death
- Lightning (difficulty ≠ peaceful): conversion into `minecraft:witch` (without keeping equipment), spawn initialisation "conversion", persistent, releases its 4 POIs; if the conversion fails → normal lightning damage. In peaceful: normal damage.
- Death: "killed" event on witnesses if a killer exists; releases its POIs; stops the trade. No natural despawn.

##### Social movements
- Village-bound wander (×0.5; panic ×0.75 with 2/2): if the position is "village" (section within ≤ 1 section of a village centre, see POI) → random land point (10 h, 7 v); otherwise looks, within ≤ 2 sections, for the section nearest to a village and walks towards it (cone π/2); otherwise random point.
- Socialise at the bell (meet): 1 chance in 100, within < 4 blocks of the point, if a villager is visible: the nearest at distance² ≤ 32 becomes `interaction_target`, walks towards it ×0.3 (close 1).
- Go to the nearest village (rest, outside village): 5 attempts of a land point (15 h, 7 v) preferring fewer sections to the village; takes the first that gets closer, otherwise one at equal distance; close 4.
- Indoor Brownian walk (rest): if it does not see the sky: a random cell of the 3×3×3 cube, without sky, passable.
- Go to the nearest bed without `home`: every 20 ticks (+ offset 0..19), nearest bed (occupied or not) ≤ 48, ignored if at distance² ≤ 4; batch of 5 candidates, cache 40 ticks; walks if reachable (close 1).
- Tag between children (play): 1 chance in 10; if another child is chasing me → flee towards a village land point (20 h, 8 v, 10 attempts) ×0.6; otherwise chase the child chased by the fewest chasers (1..5), or any, ×0.6 (close 1); the target becomes `interaction_target`.
- After a raid victory: if under a roof, go to a point seeing the sky (±10 h, ±3 v, 10 attempts, terrain height ≤ self); celebration 600 ticks if under the sky: 1/100 joyful shout, 1/200 firing of a random firework rocket (flight duration 0..2).

- NBT: `VillagerData` (type, profession, level), `VillagerDataFinalized`, `FoodLevel`, `Gossips`, `Xp`, `LastRestock`, `LastGossipDecay`, `RestocksToday`, `Offers`, inventory; brain (memories) saved.
- Not leashable. Client-side entity events: 12 hearts, 13 anger, 14 happy, 42 raid splash (particles).

#### `minecraft:wandering_trader` (creature; spawns by event, not by the natural cycle)
- Attributes: mob defaults (HP 20, speed 0.7, follow range 16). Never despawns "from afar"; disappears by delay.
- AI (goals): 0 float; 0 drink an invisibility potion if night outdoors ("dark outdoors") and not invisible; 0 drink a milk bucket if day outdoors and invisible; 1 trade with player (stands still); 1 flee zombie 8, evoker 12, vindicator 8, vex 8, pillager 15, illusioner 12, zoglin 10 (×0.5 / ×0.5; the "zombie" class also covers husk, drowned, zombie_villager, zombified_piglin); 1 panic ×0.5; 1 look at the trading player; 2 go to the wander target (stops at 2 blocks; if > 10 blocks away, 10-block steps in its direction, otherwise direct; ×0.35; target cleared on stop); 4 return to the residence zone ×0.35; 8 wander avoiding water ×0.35; 9 interact with player 3 blocks (probability 1.0); 10 look at a mob 8 blocks. No targeting.
- Trades: offers = sets `wandering_trader_buying` + `wandering_trader_uncommon` + `wandering_trader_common` (registry `trade_set`, data folder `trade_set/wandering_trader`). No level or progress bar, no restocking. A single XP orb of value 3..6 per trade if the offer rewards (no splitting, no merging; same as villager). Click: refuses if villager egg, trading, baby; stat "talked to villager"; without offers → nothing; otherwise opens the trade (displayed level 1).
- Disappearance: delay `DespawnDelay` decremented every tick (not during a trade); at 0 → removed. Set to 48000 by the event spawn. NBT: `DespawnDelay`, `wander_target`.
- No breeding (no baby). Inherits from the trader: inventory 8, unhappiness, `Offers`.
- Spawn (special world generator, rule `spawn_wandering_traders`): checked every 1200 ticks; saved counter (default 24000) -= 1200; at ≤ 0: reset to 24000, current chance C (saved, default 25); new chance = clamp(C + 25, 25, 75); if rand(0..99) ≤ C → attempt; success (or no player) → chance reset to 25.
- Attempt: random player; only 1 chance in 10; reference = a bell (POI meeting, any occupancy) within ≤ 48 of the player, otherwise the player; 10 attempts of a random position ±48 around the reference at the spawn heightmap height, valid "on ground" placement; free space 2×3×2 without collision; biome outside the tag `without_wandering_trader_spawns`. Creates the trader (reason "event"), 2 llamas (`trader_llama`) at ±4 leashed to it, delay 48000, wander target = reference, residence zone = reference radius 16 (saved separately: `home_pos`, `home_radius`, kept when the wander target is reached). Generator ticked only if `spawn_mobs` is true (like the phantom generator).
- Trader llamas: own delay 47999; while attached to the trader, delay = trader's delay - 1; removed (leash taken off) at 0, except if tamed, attached to something else, ridden by exactly one player, age locked or persistent. Goals: 1 panic ×2.0; targeting 1 defend the trader (its attacker), 2 visible zombies (except zombified_piglin), 2 illagers. Not rideable while attached to the trader.

#### Generic POI (points of interest) mechanism
- Type registry: (block states, max tickets, validity radius). A block state belongs to at most one type. Types: armorer (blast furnace) 1/1; butcher (smoker) 1/1; cartographer (cartography table) 1/1; cleric (brewing stand) 1/1; farmer (composter) 1/1; fisherman (barrel) 1/1; fletcher (fletching table) 1/1; leatherworker (cauldron, water/lava/powder snow cauldron, all states) 1/1; librarian (lectern) 1/1; mason (stonecutter) 1/1; shepherd (loom) 1/1; toolsmith (smithing table) 1/1; weaponsmith (grindstone) 1/1; home (bed HEAD only, all colours/states) 1/1; meeting (bell) 32/6; beehive 0/1; bee_nest 0/1; nether_portal 0/1; lodestone 0/1; test_instance 0/1; lightning_rod (all lightning rods) 0/1.
- Type tags: `acquirable_job_site` = the 13 workstations; `village` = workstations + home + meeting; `bee_home` = beehive + bee_nest.
- Storage per chunk section (16³): records (position, type, free tickets), saved with the world. Update: on every block state change, if the POI type of the old and new state differs → removal of the old and/or addition of the new, deferred to the end of the server tick (scheduled task). On addition, free tickets = max. When a section loads, consistency is checked against the real blocks.
- Tickets: take = free tickets -1 (refused at 0); release = +1 (capped at max). "Has room" = free > 0; "occupied" = free < max. Occupancy filters: has room / occupied / all.
- Searches: square by chunks (radius/16 + 1 chunks around), filtered by |dx| ≤ r and |dz| ≤ r, then disc: distance² ≤ r² (no vertical limit other than the spherical radius). Variants: all, nearest first (sorted by distance²), first, nearest, random (shuffled list), take (first that has room and passes the filter → ticket taken).
- Distance to the village: one tracker per section (levels 0..7); a section is a "village centre" if it contains an OCCUPIED POI of the tag `village` (at least one ticket taken); propagation of distance to neighbouring sections, values > 6 not stored (= 7). "In a village" ⇔ section within ≤ 1 of a centre. Used for villagers' wandering, raids, children's tag, wandering cats, etc.
- Request queue (acquisition by villagers, detailed above): individual scheduling every 20..39 ticks; 5 candidate POIs at most per attempt (radius 48, having room); multi-target path; per-position retry cache with increasing delay +40..79 ticks capped at 400, forgotten 400 ticks after the last attempt; cache cleared after a success. The bed sensors (babies) and "go to the nearest bed" use a simple 40-tick cache and batches of 5.
- Release: death, conversion, unreachable for too long, bed occupied by something else, lost competition (this last case only clears the loser's memory, without releasing the ticket — the winner already holds the same POI).

---

### 4.F01 Singleplayer implementation: hostile and pest variants (2026-10-12)

Code: `crates/rc-sim/src/mobs/{species,variants,effects,pests,phantom}.rs`, tests `tests_f01.rs`.
Simulated species: husk, zombie_villager, stray, bogged, parched, giant, cave_spider, silverfish,
endermite, phantom. The variants share the engine of their base species (`Kind::base`).

- Details found in the code (complement the sheets above):
  - Conversion by affliction (drowning of the zombie and husk, freezing of the skeleton): affliction
    counter +1 per afflicted tick; reaching the total (600 / 140) → conversion armed at 300;
    each following afflicted tick decrements it, conversion when it goes below 0; a non-afflicted
    tick sets the counter to −1 and cancels the conversion. Save: conversion time (−1
    otherwise) and affliction counter (−1 if not afflicted at the moment). The converted mob
    is new: position, rotation, velocity, equipment and drop chances, fall, invulnerability,
    effects, baby, hand, AI, persistence, name carried over; health NOT carried over (full). Converted zombie
    (drowning): random zombie attributes redrawn without healing. Level events: 1040
    zombie → drowned, 1041 husk → zombie, 1048 skeleton → stray.
  - Villager killed by a zombie (any zombie species): the "one chance in two" draw (Normal)
    comes from the zombie's random generator; successful conversion → the villager drops neither loot nor experience.
    The zombie villager born this way is finalised (reason "conversion": no pickup, no equipment,
    no baby drawn, but follow bonus, hand and doors drawn), then receives the villager's data.
  - Zombie villager curing: advanced before the common tick; end → villager finalised as
    "conversion", data carried over, nausea 200 ticks, event 1027.
  - Venomous spider: its finalisation does nothing (not even the mobs' common part).
  - Effect arrows (stray, bogged, parched): effect added to the arrow, full duration (no
    division: the projectile has no duration scale); colour (metadata 11) = effect colour,
    opaque; stuck for 600 ticks: event 0 and becomes a plain arrow again.
  - Status effects: update rules (stronger and shorter → the old one becomes "hidden" and
    comes back at the end), tick on the remaining duration (poison: interval `25 >> level`, 1 magic
    damage if HP > 1; hunger: exhaustion 0.005 × (level + 1) per tick), immunities (undead:
    poison and regeneration; spiders: poison; parched: weakness; silverfish: only
    "infested"). Player: effect packets, attribute modifiers (slowness −15 %/level
    total multiplicative, weakness −4, strength +3), save `active_effects`.
  - Phantom spawner: countdown decremented every tick when monsters
    spawn and the rule `spawn_phantoms` is true; at 0: +(60..119) × 20 ticks; night
    (sky darkening ≥ 5), non-spectator player above sea level and under the
    sky, local difficulty > random × 3, `next_int(max(1, time since rest)) ≥ 72000`;
    position +20..34 in Y, ±10 in X/Z; group 1..(1 + difficulty id) finalised together. The
    statistic "time since rest" advances by one per tick of the non-sleeping player, reset to 0
    on death (and on going to bed).
- Remaining differences (singleplayer): mounts (chicken jockey, skeleton on
  spider); ender pearls (endermite); block spawners (mines, fortresses, trial
  chambers); the pests' "climb on powder snow" goal; mobs' effect particles
  (metadata 10); sleeping (reset of the time since rest); cured villager's
  inventory (equipment not transferred).

### 4.F04 Singleplayer implementation: mounts (2026-10-13)

Code: `rc-sim/src/mobs/mounts/` (sheets, state, data, NBT, sounds, food, breeding,
passengers, llama, camel brain, trap, menu) and `mobs/goals/mount_goals.rs` (goals).

- **Sheets**: horse, donkey, mule, llama, trader llama, skeleton horse, zombie horse
  (monster category, animal class: neutral sounds, animal experience, no wear-down in
  light), camel, camel husk (monster). Common base: step 1 (camel 1.5), safe fall
  6, multiplier 0.5, volume 0.8, ambient sound 400. Base jump 0.7 (donkey, mule, llamas 0.5;
  camel 0.42). Babies: ×0.7 (horse, skeleton, zombie), ×0.5 (donkey, mule, llamas), camel
  0.95 × 1.4 (sitting 0.95 × 0.425; sitting adult 1.7 × 0.945).
- **Draws at spawn** (world random generator, in this order): horse → coat U[0,7[ (shared
  by the group), markings U[0,5[, then HP 15 + U[0,8[ + U[0,9[, speed (0.45 + 3 × 0.3 U) ×
  0.25, jump 0.4 + 3 × 0.2 U; donkey, mule, llamas → HP only; llama → strength (U < 0.04 ? 5 : 3,
  then 1 + U[0,max[), variant U[0,4[ of the group, then HP; skeleton → jump; zombie → jump
  0.5 + 3 U/15 then speed (9 + 3 U)/42.16; camel → pose "standing for 53 ticks".
  Group babies: 5 % for horses and llamas (own group data), 20 % for the others (default
  equid data); the first is never a baby. Groups of 6 at most.
- **Natural zombie horse**: a zombie is finalised first (world random generator), receives an iron spear and
  rides it; then the horse. **Natural husk** (10 %, free space): iron spear, camel husk
  finalised and ridden by force, parched finalised and ridden behind.
- **Goals** (order added): equids → 1 frantic running ×1.2, 2 breeding, 4 follow the parent,
  6 wander ×0.7, 7 look at the player 6, 8 look around, 9 random rearing (except llamas and
  camels), then 0 float, 1 panic ×1.2, 3 temptation ×1.25 (zombie: float, temptation
  only; skeleton: none); trap added at the end. Llama: 0 float, 1 frantic running, 2
  caravan ×2.1, 3 spit (40 ticks, range 20), 3 panic ×1.2, 4 breeding, 5 temptation,
  6 parent, 7 wander, 8 gaze, 9 look around; targets 1 retaliation (stops after one spit), 2 untamed
  wolf (follow range ×0.25, draw 1/8 reduced); trader llama: + panic ×2,
  zombies, illagers.
- **Frantic running**: without taming, ridden and not steered by a mob; 1 chance in 25 per
  tick: player → U[0, max temper[ < temper → tamed (event 7); otherwise temper + 5; all
  dismount, temper (rearing + cry), event 6.
- **Immobile** when eating or rearing (llama: when eating): neither AI nor walking this tick.
- **Food**: the game's tables (wheat, sugar, hay, apple, red mushroom, carrot, golden carrot and
  apples); llama (wheat, hay); camel (cactus: heal 2, love, +10 s). Fed baby: three
  "particle" draws on the entity's random generator, then the age advances.
- **Equipment**: saddle by click (component `equippable`, entities allowed by tag) or menu;
  armour or carpet in the body slot; chest (donkey, mule, llamas; 5 columns, llama = strength).
  Menu `mount_screen_open` (saddle, body, chest, the game's quick move), closed if the animal
  dies, moves away (interaction range + 4) or changes chest. Guaranteed drop chances
  for what the player places; chest and contents dropped on death even for a baby.
- **Passengers**: a passenger is ticked by its vehicle (then seated on its seat: attachment
  points of the type, rearing, camel according to its pose); it does not despawn. Steering rider
  (zombie horse, camel husk): its goals act on the navigation and the control of
  the mount, whose movement, jump and look goals are cut (every 5 ticks).
- **Mounted player**: right click (not sneaking) → `set_passengers`, teleport to the seat;
  saddled mount piloted by the client (`move_vehicle`: collisions then the client's position;
  refused if the horizontal gap exceeds 0.25 in a free space); no server AI or movement,
  rotation taken from the player (pitch × 0.5); mount jump (`player_command` 3):
  rearing and sound; sneaking → dismount on the main-hand side (free spot). Vehicle recorded
  in the player's file (`RootVehicle`) and removed from the chunks on close; recreated and
  remounted on arrival.
- **Llama**: spit in front of the mouth (width + 1) / 2, towards a third of the target's height,
  speed 1.5, inaccuracy 10; gravity 0.06, friction 0.99, 1 "spit" damage; removed on a
  block, in water or when full. Caravan written to the game but inert (leash not simulated). Trader
  llama: 47,999 ticks then removed (suspended if tamed, ridden by a lone player, locked,
  persistent).
- **Camel**: brain (sensors every 20 ticks with a start drawn on the entity's random generator;
  "core" then "rest" activities by priority 0 to 5, durations drawn on the world's random generator, single
  choices shuffled by their own random generator): swim, panic ×4 (gets up at once), look at the
  target, go to the target; look at a player from time to time (30 to 60), mate,
  temptation (cactus, ×2.5, 2.5/3.5) or baby that follows an adult (5 to 16), look around (150 to
  250), wander ×2, walk towards the gaze, sit/stand (≥ 400 ticks in the posture),
  do nothing (30 to 60). Sitting: refuses to move, gets up in water, when hit, when led.
  Dash: sound, "dash" state, cooldown 55 ticks.
- **Skeleton horse trap**: per thunderstorm chunk, 1 in 100,000 (block world random generator);
  lightning target (height, mobs under the sky); rain → trap if `spawn_mobs` and
  U < effective difficulty × 0.01; trap horse without finalisation. Player within less than 10 →
  visual lightning, tamed horse, helmeted skeleton mounted, three other horses finalised, tamed,
  persistent, pushed (triangle ±1.1485), each one mounted. Without a player: removed after 18,000 ticks.
- **NBT**: `EatingHaystack`, `Bred`, `Temper`, `Tame`, `Owner`, `AgeLocked`, jump in
  `attributes`, `Variant`, `ChestedHorse`/`Items`, `Strength`, `DespawnDelay`, `SkeletonTrap(Time)`,
  `LastPoseTick`, `Passengers` (riders), `drop_chances` (saddle, body).
- **Remaining differences**: leash and caravan (leash not simulated); wandering trader's llama
  (trader not simulated); real lightning (fire, damage: only the visual lightning is sent);
  lightning rods (point-of-interest search); enchantments of the trap skeletons'
  equipment (not applied, like the rest of the engine); skeleton horse swimming sounds; walking
  "towards a neighbouring position" when a camel's path is not found; rider's path
  computed with its own box (the mount follows it); chicken jockey and skeleton on spider
  (riders' families); name of the "sneaking" interaction taken from the keys and not from the packet.

### 4.F05 Singleplayer implementation: full villager and wandering trader (2026-10-13)

- **POI**: type per block state (table built on first use; bed = head only,
  cauldrons and lightning rods in all their forms); per-section registry, iteration
  order of the game's sets reproduced within a section (hash bucket of the position, then
  arrival order); chunks walked x then z, sections from bottom to top. Block state change →
  removal/addition deferred to the next tick. Reading of `poi/` with the chunk (valid sections
  kept, otherwise a sweep that keeps the known tickets); writing of `poi/` on save
  (`Sections` → `Valid`, `Records` `pos`/`type`/`free_tickets`). Distance to the village: smallest
  Chebyshev distance (sections) to a section that contains an occupied POI of the tag `village`,
  capped at 7; "village" ⇔ ≤ 1.
- **Offers**: set `trade_set/<profession>/level_<n>` (wandering trader: `buying`, `uncommon`,
  `common`), number drawn by the `amount` provider, draws without replacement over the list of the tag
  `villager_trade` (file order, nested tags flattened); the random generator is the game's **named sequence**
  (`random_sequence`, saved in `random_sequences.dat`): a given world gives the same offers
  as the official server in the same order of appearance. Trader predicate: villager
  type. Modifiers: random enchantment (level `1 + next_int(max)`, extra
  cost `2 + next_int(5 + 10n) + 3n`), enchantment by levels (cost + 2 draws
  of enchantability ÷ 4, spread ±15 %, weighted draw then `next_int(50) ≤ cost` with cost ÷ 2,
  incompatible enchantments removed), random dyes (the game's intensity average), fixed
  or random potion (tag), stew effect, filter (items, enchantments, map); price doubled for
  `double_trade_price`. Price: base + max(0, ⌊base × demand × multiplier⌋) + special difference,
  clamped to [1, max stack]; selection index 0 never forced (quirk).
- **Trading**: menu 19 (payment 0 and 1, result 2, inventory 3..38); result recomputed on
  every payment change (A then B, otherwise B then A); taking the result: payment removed,
  use +1, experience +xp, orb 3..6 (+5 and regeneration 200 ticks on level-up,
  new offers added); quick move = repeated purchases as long as the payment suffices;
  `select_trade`: payments returned to the inventory then refilled from the inventory. The next
  AI tick gives the "trade" reputation event (+2) and event 14; the special
  difference is recomputed and the offers resent. Menu valid as long as the player is within
  7 blocks of the trader's box.
- **Brain**: memories (places, walk and look targets, timestamps, golem seen 599 ticks),
  nine sensors (periods 20, 40 for the secondary POIs, 200 for the golem, initial offset
  `next_int(period)` on the mob's random generator), core/work/play/meet/rest/
  idle/panic activity packs arranged by priority, durations drawn on the world's random generator, "random"
  gates (the single-choice gate) shuffled by their own random generator. Schedule: `villager_schedule` timeline on
  the Overworld clock, in ALL dimensions (universal timeline, Nether and End included).
  Memorised places (home, workstation, potential workstation, meeting): position AND dimension, saved
  as is; a place in another dimension counts as present but is neither aimed at, nor
  validated, nor worked at; the "walk to the place" behaviour forgets it (and would release it in
  its dimension: not reachable as long as a single dimension runs). Home = the 16 coloured beds
  only (not the straw bed). Witnesses of a murder: all the villagers of the list of
  living entities, visibility evaluated for each. "Recently hurt" (stops trading and following) =
  injury shake in progress, 10 ticks after any full damage. Rebuild (taking or losing a profession, switch to
  adulthood): behaviours stopped, places and timestamps kept.
- Farmer's work at the composter: breads (≤ 36), bone meal collected at level 8, composting
  of seeds by layers of the `compostable` component (provider `compostable/low`: 1 on an
  empty composter, otherwise weighted list 30/70); full composter → scheduled tick 20 → level 8
  and "ready" sound. Harvest (rule `mob_griefing`), bone meal (age + 2..5, beetroot ÷ 3;
  torchflower growth: +1 without a draw, age 2 = the placed flower),
  restocking twice a day (demand catch-up on the new day).
- Breeding: inventory food (component `villager_food`), reachable free bed taken,
  baby (type 50 % biome, 25 % each parent, data fixed, WITHOUT finalisation: no follow
  bonus, no left-handedness, no world draw) with that bed in memory; anger (event 13) without a
  bed. Sleeping: occupied bed, lying pose and bed position (metadata 6 and 14).
- Wandering trader: the game's goals (drink an invisibility potion at night, milk by day, trade,
  flee zombies, panic, look at the trading player, go to the wander target in steps
  of 10 blocks, return to the 16 zone, wander avoiding water, interact, look); despawn
  delay counted down outside trading; spawner every 1200 ticks (`wandering_trader.dat`:
  `spawn_delay`, `spawn_chance`), own random generator seeded from the clock, bell at 48 blocks as reference,
  10 ground position attempts, 2×3×2 space, biome outside the tag.
- Remaining differences (singleplayer): raids and ringing bell (activities `pre_raid`, `raid`, `hide` never
  active), lightning → witch (conversion ready in F07, lightning bolts not simulated), trader llamas' leash, exploration
  maps (structure search not simulated: the offer is discarded as if no
  structure were found), statistics (talking, trading), local sound of the quick
  move, order of activities of the same priority (the game's identity hash table: core
  first here), simplified bed get-up position, trader's item-use sequence
  (sounds every 4 ticks, 32 ticks) approximated.

### 4.INT Families F01–F06 wired to singleplayer (2026-10-13)

- **Status effects**: a single effect instance (the player's, `effects.rs`) for the player
  and the mobs; shared table (colours, modifiers: speed, slowness, haste, fatigue in
  plain floats widened like the game). Mob: acceptance (silverfish
  and "infested", slime and "oozing", undead and poison/regeneration, spiders and
  poison, parched and weakness), tick at the end of the base tick (regeneration, poison, wither,
  instant health and damage inverted for the undead), modifiers (speed, damage,
  max health, safe fall), particles (data 10 and 11) and invisible / glowing flags.
  No `update_mob_effect` packet for a mob (like the game). Command `effect`: real
  on the simulated mobs.
- **Equipment at spawn**: enchanted by the provider `mob_spawn_equipment` (weapon 25 %,
  armour 50 % per piece, × special multiplier; cost 5 + U[0, ⌊special × 17⌋], enchanting
  table, tag `on_mob_spawn_equipment`); trap skeletons: weapon and helmet
  disenchanted then enchanted on the skeleton's random generator.
- **Villager ↔ golem**: the spawning of a golem when needed (panic every 100 ticks, gossip) makes
  the iron golem appear ("old" strategy, 10 attempts); all the villagers of the
  box enlarged by 10 "see" the golem (599 ticks). Golem: player's reputation through
  gossip, distance to the village through the POI registry, wandering towards a villager who wants a
  golem (box enlarged by 32) or towards an occupied POI of a village section (cube of radius 2),
  the land position draw towards a point.
- **Wandering trader**: enters the world, then its two llamas ("event", never babies).
- **Dimensions**: POI registry, spits, trap lightning bolts and snowballs specific to
  each dimension; POIs of parked dimensions written on save. Portal: a passenger
  does not go through, nor does a loaded vehicle here; the mob's real box.
- **Differences**: thrown potions (entity not simulated, no mob hit), absorption of a
  mob (effect kept, reserve not simulated), "on hit" and "on death" effects (infested,
  oozing, weaving, wind charged), trader llamas' leash, mount that crosses a
  portal with its passengers, parked dimensions without mob ticks (wandering trader and phantom
  spawners only when the Overworld is active), two enchantment draws
  (table S7 for equipment, copy of F05 for offers).

### 4.F08 Singleplayer implementation: drowned, guardian, elder guardian (2026-10-13)

Code: `rc-sim/src/mobs/aquatic/` (sheets, state, metadata, NBT, goals, hooks), `drowned.rs`,
`guardian.rs`, `trident.rs`, `spawn.rs`; swim evaluator `mobs/nav/swim_nav.rs`.

- **Sheets**: drowned = zombie (HP 20, speed 0.23, damage 3, follow range 35, armour 2) + step 1;
  baby 0.49 × 0.98 (eyes 0.775). Guardian 0.85 (eyes 0.425), HP 30, speed 0.5, damage 6,
  experience 10, ambient sound every 160. Elder guardian 1.9975 (eyes 0.99875), HP 80, speed
  0.3 (float), damage 8, client follow range 10, persistent from construction.
- **Breathing**: `#can_breathe_under_water` = undead (zombies and drowned included) and guardians.
  The zombie therefore no longer drowns (it converts: 600 + 300 ticks, event 1040, attributes
  redrawn); the guardian resets its air to 300 at each AI step in water.
- **Navigation**: amphibious drowned (water 0, walk 6, water edge 4 during the search); swimming guardians:
  start at the bottom of the box + 0.5, neighbours in the order down, up, north, south, west, east
  then horizontal diagonals (north-east, east-south, south-west, west-north) between two neighbours without a
  negative penalty; a cell counts as "water" if the whole footprint is water and the last cell
  walked is passable in water (doors, fences, stairs, walls… never; trapdoor if
  submerged); "breach" (air above a passable cell) refused. Path updated only
  in a liquid, target not brought back to the surface, mob's point at mid-height,
  node reached within 0.5 in height, stable destination = non-full-opaque cell.
- **Drowned**: goals (order added) 8 look at the player, 8 look around, 1 go to water (by day,
  out of water: 10 draws `next_int(20)−10`, `2−next_int(8)`, `next_int(20)−10` on the
  mob's random generator, water block only), 2 trident (the ranged attack goal 1.0 / 40 / 10, trident in hand; arm
  raised = item in use, index 8), 2 zombie melee filtered by "valid target", 5 beach
  (the move-to-block goal 8 × 2, two cells of air on a block with a sturdy top face; countdown
  drawn before the night, water and height ≥ sea − 3 conditions; "seeks land"
  reset to false), 6 swim up (night, water, y < sea − 2: "seeks land", steps of 4 × 8 towards
  sea − 1 when the navigation is finished or the path target is within < 2 blocks; stuck without
  position), 7 wander. Targets: 1 retaliation (not against a drowned), 2 player (filter "night or target
  in water"), 3 villager, 3 iron golem, 3 axolotl (interval draw only), 5 baby
  turtle on land. Swimming = effective AI, eyes in water on the previous tick, in water and wanting to
  swim (seeks land or target in water): shared flag 0x10, no longer pushed by the current,
  acceleration 0.01 and damping 0.9; swim control: +0.002 vertical if the target is
  higher or if it seeks land, speed smoothed 0.125, push (x gap × 0.005, normalised
  vertical gap × 0.1, z gap × 0.005) × speed; out of swimming −0.008 in the air then the common
  control. Aquatic sounds (`ambient_water`, `hurt_water`, `death_water`) in water.
- **Trident**: starts at the eyes − 0.1, aimed at a third of the target's height
  + horizontal distance × 0.2, power 1.6, inaccuracy 14 − 4 × difficulty (trident's random generator),
  shot sound (pitch `1/(r×0.4+0.8)`, drowned's random generator); flight: gravity 0.05, friction 0.99 in
  air as in water; 8 "trident" damage to the first entity (none afterwards), sound
  `item.trident.hit`, deflection (speed × −0.01 / −0.1 / −0.01, yaw + 170..190); stuck:
  `item.trident.hit_ground`, pickup forbidden, removed after 1,200 ticks.
- **Equipment at spawn**: `r > 0.9` then `next_int(16)` < 10 trident, otherwise fishing rod
  (neither the zombie's armour nor weapon), common enchantments; then nautilus shell in the left hand
  (3 %, certain drop); then the draw of the mounted zombie nautilus (natural, trident, 50 %).
- **Guardians**: goals 4 beam, 5 return to residence (the position draw towards a point 16 × 7, π/2; elder
  guardian only), 7 wander (80 / 400, movement + look, restarted by force by an injury or
  the end of the beam: neither idle delay nor interval draw as long as no position is
  found), 8 look at the player, 8 look at a guardian (12 blocks, 0.01), 9 look around; maximum
  head pitch 180. Target: player beyond 3 blocks (sight, interval 10). Beam: −10 at
  start (navigation stopped, look 90/90), target lost when out of sight, lock at 0 (index 17 =
  network id of the target, event 21), at 80 (elder 60): indirect magic 1 (+2 hard,
  +2 elder) then melee hit, target forgotten; stops if the target comes within ≤ 3 blocks (except elder).
  Control: "go to" operation never put back on wait, yaw smoothed 90, speed smoothed 0.125,
  undulation `sin((age+id)×0.5)×0.05` and `sin((age+id)×0.75)×0.05×(sin+cos)×0.25`, look smoothed
  0.125 forward, "moving" flag (index 16). Swimming: acceleration 0.1, ×0.9, −0.005
  if it is immobile and without a target. Out of water on the ground: jump (±0.4, +0.5, random yaw).
  Thorns: 2 damage to the living direct attacker if the guardian is immobile (except thorns and tag
  `avoids_guardian_thorns`), before any damage check.
- **Elder guardian**: every 1,200 ticks (age + id), mining fatigue III 6,000 ticks to the
  survival or adventure player within 50 blocks who lacks it, has a weaker one or whose ends
  in ≤ 1,199 ticks, and game event 10 (value 1); residence set at its position (radius 16,
  NBT `home_pos` / `home_radius`).
- **Spawn**: "in water" placement (water at the position, non-conductor block above), without
  rejecting the liquid at the obstruction check; drowned: water below, dark (brightness draw) and water at the
  position, then river (`more_frequent_drowned_spawns`) `next_int(15) == 0`, otherwise
  `next_int(40) == 0` and y < sea − 5; guardians: `next_int(20) == 0` or sky not visible from the water,
  water at the position and below. Lists replaced by structures (`spawn_overrides`, whole
  box or piece): the monument gives the guardians (2 to 4) throughout its box. The three
  elder guardians placed by rc-worldgen (`structure_monument.rs`) are adopted as mobs
  when the generated chunk arrives; their residence is set at the first AI step.
- **Drop chances** (all mobs): `drop_chances` written only if it differs from
  0.085 everywhere (drowned's shell at 2, saddle, placed armour…), on reading otherwise reset to 0.085.
- **Swimming and splash** (common engine, `aquatic/splash.rs`): when crossing the next
  step on a non-empty block, a "ground" step (block sound) only on the ground or on a ladder and
  without swimming; otherwise, in water, swim sound, volume `min(1, √(x²×0.2 + y² + z²×0.2) × 0.35)`,
  pitch `1 + (r − r) × 0.4` (entity's random generator); otherwise the step stays due. Sounds: drowned
  `drowned.swim`, turtle `turtle.swim` (× 1.5), skeleton horse (`step_water` 0.3 on the ground, otherwise
  `swim` ≤ 0.1), monsters `hostile.swim`, others `generic.swim`. Entering the water (not on the first
  tick): volume × 0.2 of the movement before the current, `< 0.25` splash of the species
  (`hostile.splash` or `generic.splash`), otherwise `generic.splash`; then particle draws
  (not sent): `n` = integers `i < 1 + width × 20`, 3 draws per bubble then 2 per droplet.
- **Remaining differences**: zombie nautilus (F09) not created; axolotl, squid (targets) not simulated;
  item pickup by monsters not simulated (spears refused, shell never replaced: no
  effect); swim and splash volume of a steered mount (rider modulator 0.4 /
  0.9) and gallop of the skeleton horse ridden in water; structure starts in
  unloaded chunks ignored; structure spawns at generation (reason "structure")
  absent.

### 4.F07 Singleplayer implementation: illagers, patrols and raids (2026-10-13)

Code: `rc-sim/src/mobs/illagers/` (`mod.rs` sheets, state, data, NBT, registered goals,
hooks; `goals.rs` own goals; `crossbow.rs` crossbow; `spells.rs` spells and fangs;
`vex.rs`; `ravager.rs`; `witch.rs` drunk and thrown potions, struck villager; `patrol.rs`
spawner; `raids.rs` raids, boss bar, banner, doors; unit and world tests).

- **Sheets**: pillager, vindicator, evoker, illusioner 0.6 × 1.95 (eyes 0.85 h); HP 24 / 24 /
  24 / 32, speed 0.35 (f32) / 0.35 / 0.5 / 0.5, follow range 32 / 12 / 12 / 18, attack 5 (pillager,
  vindicator); evoker 10 experience. Vex 0.4 × 0.8 (eyes 0.51875), 14 HP, 4 attack,
  3 experience, fire-immune, follow range 16. Ravager 1.95 × 2.2, 100 HP, 0.3, 12 attack,
  knockback taken 0.75, knockback dealt 1.5, step 1, follow range 32, 20 experience, passable foliage (penalty 0),
  head 45°. Witch 0.6 × 1.95 (eyes 1.62), 26 HP, 0.25. All "monsters" (5 unless stated).
- **Data**: raid pillagers index 16 celebration (boolean); pillager 17 crossbow charging;
  evoker and illusioner 17 spell (byte: 0 none, 1 vex, 2 fangs, 3 wololo, 4 mirror, 5 blindness);
  witch 17 drinks; vex 16 flags (1 = charge) and 5 "no gravity" from its first tick;
  index 8 "item in use" during charging and drinking; bit 0x20 of index 0 under
  the invisibility effect.
- **Finalisation** (world random generator): pillager → crossbow, weapon enchantment (25 % ×
  special, provider `mob_spawn_equipment`, table S7), Piercing I one time in 300; vex →
  iron sword (never dropped), same enchantment;
  illusioner → bow; then for raid pillagers: "can join a raid" (false for the
  naturally born witch), leader draw 6 % (except patrol and event; ravager and
  witch never leaders) → ominous banner on the head (certain drop), patroller if
  reason "patrol"; then the common follow bonus; vindicator afterwards: doors, iron axe
  outside a raid, enchantment draw.
- **Goals**: common (distant patrol 4, banner 1, go to raid 3, cross village 4,
  celebration 5) then the species', in the game's order. Patrol: leader that picks a target at
  ±500 blocks, companions at 16 blocks, 10-block steps towards a target rotated by 90 RADIANS
  (tabulated sine), failure → wander ±8 and 200 ticks of waiting. Hold position (10 blocks): alert
  of pillagers at 8 blocks. Crossbow: path delay 20-40, charge 25 ticks (Quick Charge −5 per
  level), loading sounds at 20 %, 50 %, 100 % (end on the world's random generator), charged projectile
  visible to the client (`charged_projectiles`), delay 20-39 then shot when the target is seen: bolt
  (base damage 2, sound `item.crossbow.hit`), power 1.6, inaccuracy 14 − 4 × difficulty,
  vector normalised in float. Spells: reduced delay (20 → 10, wololo 40 → 20), casting
  40 / 100 / 60 / 20 / 20, interval 100 / 340 / 140 / 340 / 180; fangs in a circle (5 at 1.5 blocks,
  8 at 2.5 blocks offset by 3 ticks) within 3 blocks, otherwise line of 16; 6 magic at −8 ticks,
  allies spared; vex: 3 at ±2 blocks, life 30-119 s, bound to the evoker; wololo: blue sheep seen at
  16 blocks, turns red; mirror: invisibility 1,200; blindness 400 if local difficulty > 2.
  Vex: charge (1 chance in 4 on a partial tick) towards the target's eyes, hits on contact,
  wander at ±7/±5/±7 of the origin, movement without collision, 0.91 / 0.98. Ravager: hit
  (immobile 10 ticks, event 4), stunned 40 ticks then roar at 10 (6 damage except
  illagers, push 4/d² except player, event 69), foliage torn off on collision. Witch:
  drinks (32 ticks, −0.25 speed), throws harming / healing / regeneration (allies) / slowness /
  poison / weakness, power 0.45 or 0.75, inaccuracy 8; splash at 4 blocks, durations at
  scale 1 − d/4, effects of 20 ticks or less ignored; magic taken × 0.15.
- **Melee damage**: `attack_damage` modifiers of the held weapon added (vindicator 5 + 8,
  vex 4 + 5; also armed zombies), then the weapon's enchantments (S7: sharpness, smite,
  bane of arthropods). Defence side: resistance, protections, then witch × 0.15 (tag `witch_resistant_to`),
  and 0 if the source is the witch itself (its own damage potion).
- **Patrols**: every 12,000-13,199 ticks, by day, 1 in 5, non-spectator player and far
  from a village (2 sections), at 24-47 blocks on each axis, chunks within ±10 loaded, attribute
  `can_pillager_patrol_spawn` (mushrooms: no; `early_game` timeline: from 120,000
  clock ticks), ⌈effective difficulty⌉ + 1 pillagers offset by ±4; leader first, block
  light ≤ 8. Draws: for each axis the distance (`next_int(24)`) THEN the sign (`next_boolean`).
  The leader's distant target is chosen before the pillager is placed: it is at ±500 blocks
  of the world ORIGIN, y = 0 (patrols walk towards the centre).
- **Structure spawns** (`mobs/structure_spawn.rs`, common to all families): `spawn_overrides`
  list of a structure referenced by the chunk, one of whose starts covers the position
  (whole box, enlarged by 12 if the structure adapts the terrain: outpost → pillagers;
  "piece": witch hut → witch, cat), Nether fortress (block below is
  Nether brick) → fixed list (blaze, zombified piglin, wither skeleton, skeleton, magma
  cube); each position of the group checks that the drawn entry is still in it
  (the mob's spawn test); only the pillager may be born beyond the despawn distance.
- **Raids**: Bad Omen in a village (section within ≤ 1 of an occupied village POI) →
  Raid Omen 600 ticks at the same level, position noted (`raid_omen_position`); at the end,
  raid created at the centre of the occupied village POIs within 64 blocks (ids from 2), omen
  level added (5 at most). Tick: active if the centre is loaded, centre moved to a
  neighbouring village section, lost (or stopped) outside a village, 48,000 ticks at most; 300 ticks
  between waves (progress bar), spawn position on a circle around the centre
  (the raid's own random generator), outside the village as long as more than 7 s remain, ravager spot on the ground;
  waves 3 / 5 / 7 (+1 bonus beyond level 1): the game's species table, bonus according to
  difficulty, ravager riders (pillager at wave 5, evoker then vindicators from
  the 7th), designated leader; reinforcements (Quick Charge, Sharpness according to the wave and level); raid
  horn at 13 blocks from the player; victory after 40 ticks without a pillager → Hero of the Village (48,000 ticks,
  level − 1); celebration 600 ticks. Red bar with 10 notches, name "Raid", "Raid - Pillagers remaining:
  N" at 2 or fewer, victory / defeat. Pillagers: recruitment at 16 blocks every 20 ticks,
  join a neighbouring raid, leaving the raid (112 blocks, 30 idle checks outside a village), death →
  leader removed and hero noted. Doors: broken down by the vindicator in a raid (240 ticks, normal or
  hard, rule `mob_griefing`), opened then closed again on the next tick by the pillager.
- **NBT**: `patrol_target`, `PatrolLeader`, `Patrolling`, `Wave`, `CanJoinRaid`, `RaidId`;
  pillager `Inventory` (reread: picks up items); vindicator `Johnny` (also set by a
  "Johnny" name); casters `SpellTicks`; vex `bound_pos`, `life_ticks`, `owner`; ravager
  `AttackTick`, `StunTick`, `RoarTick`. Raids in `data/minecraft/raids.dat` (`raids`, `next_id`,
  `tick`).
- **Loot**: the game's tables; predicate `type_specific/raider` (in a raid, captain); captain's
  ominous bottle.
- **Despawning**: patroller only beyond 128 blocks; raid pillager never (and
  persistent).
- Remaining differences (singleplayer): structure start in an unloaded chunk ignored (the game
  would load it), lightning not simulated (the villager → witch conversion is ready), creaking not
  simulated (flee ineffective), generic equipment pickup by pillagers, player's shield (stunning the
  ravager), enchantment effects on bolts (piercing), order of pillagers within a
  wave (the game's hash sets), raid statistics and advancements.

### 4.F09 Singleplayer implementation: aquatic fauna and nautiluses (2026-10-13)

Code: `rc-sim/src/mobs/sealife/` (renamed from `aquatic/` on merging with F08; sheets, state, metadata, NBT, spawn, dispatch:
`mod.rs`; fish, schools, pufferfish, bucket: `fish.rs`; squids: `squid.rs`; dolphin and
"swimmer" controls: `dolphin.rs`; nautiluses and their brain: `nautilus.rs`; goals:
`goals.rs`), swimmer path evaluator `mobs/nav/swim_nav.rs` (shared with F08).

- **Categories**: four aquatic categories added in the game's order (axolotls, underground
  aquatic, aquatic, water ambient: caps 5, 5, 5, 20; immediate despawn at 64
  blocks for water ambient, 128 otherwise). Census and spawn loop over the seven
  categories; water ambient refused at 98 % in `reduced_water_ambient_spawns` biomes.
- **"In water" placement**: water at the cell, nothing conductive above; obstruction = no
  entity in the box (fluids are allowed). Rules: surface water (sea level − 13 to
  sea level, water below and above) for cod, salmon, pufferfish, squid, dolphin;
  tropical fish: same except in `allows_tropical_fish_spawns_at_any_height` biomes;
  glow squid: ≤ sea − 33, raw light 0, water block; nautiluses: sea − 25 to sea − 5. Groups:
  8 (fish), 5 (salmon); the non-schooling tropical fish stops its group.
- **Physics**: not pushed by currents; fish: rise +0.005 with eyes in water,
  speed smoothed ×0.125, climb towards the target, movement in water at 0.01, ×0.9, −0.005 without
  a target; flopping out of water (two draws, +0.4, the species' "flop" sound). Dolphin and
  nautiluses: "swimmer" controls (yaw 10°, pitch 85° and 5° per tick, speed ×0.02 / ×0.011
  in water, ×0.1 / 0 out of water), "swimmer" look (head ±10° of the body, body ±4°).
  Squids: movement vector applied at the power phase of the stroke, ×0.9 at rest, vertical fall
  damped out of water (levitation included), no move control.
- **Air**: fish, squids, nautilus: −1 per tick out of water, 2 damage (drowning; nautilus:
  drying out, nothing without AI) at −20, 300 in water. Dolphin: 4,800, immediate regain out of
  water, drowning under water; moistness 2,400 (rain or water), 1 drying damage at 0, hops on the
  ground. Zombie nautilus: no air rule of its own (breathes underwater).
- **Swim navigation**: water cells (whole footprint), breach at the surface for the dolphin
  (penalty +8 outside fluid), neighbours down/up/north/south/west/east then horizontal diagonals;
  stable destination = not full as rendered; vertical gap 0.5; straight line without fluids.
- **Fish**: panic ×1.25, flee the player (8; ×1.6 / ×1.4), random swim (interval 40,
  10 attempts), schools (leader of the spawn group, search every
  200 + (U(200) mod 20) reduced, addition limit applied before discarding the leader, back to 1 when
  it is alone without detaching its followers). Pufferfish: puffing (1 then 2 after 40 ticks,
  deflating at 60 then 100), sting of mobs (1 + state, poison 60 × state) and of the player
  (event 9). Salmon: small 30 / medium 50 / large 15 (entity's random generator, before finalisation),
  box ×0.5 / ×1 / ×1.5. Tropical fish: 90 % common variant (22, world random generator) shared by
  the group, otherwise pattern and colours drawn and non-schooling. Water bucket: the fish's bucket (name, entity
  data, size, patterns), fish removed.
- **Squids**: stroke speed 1 / (U + 1) × 0.2 drawn at construction and 1 time in 10 at
  each cycle (event 19), random heading (1 in 25 per full tick, out of water, without a vector;
  motionless after 100 ticks of inaction), flee the aggressor (within 10 blocks), ink (sound, 30
  particles, three draws each) when hit by an entity; glow squid: dark 100 ticks,
  three doubles drawn per tick for its glows (particles with no effect server-side).
- **Dolphin**: the game's goals (breathe, find water, treasure, swim with the player — dolphin's
  grace 100 ticks, then 1 in 6 —, random swim 10, look, jumps, melee ×1.2, items, follow
  a ridden nautilus; retaliation that alerts the other dolphins). Fish given: baby → grows;
  adult → `GotFish`, treasure = nearest structure of the tag `dolphin_located` (radius 50 chunks).
  Picks up items (empty hand, rule `mob_griefing`) and throws them back (delay 40).
- **Nautiluses**: tamed with pufferfish (1 in 3, events 7 / 6), healing with the nautilus's
  food (2 × nutritional value, 1 for a bucket), buckets returned as a water bucket, saddle and armour (tamed
  adult), mount inventory (sneaking), mounts (saddled: piloted by the client, rotation halfway
  to the rider's gaze, dash: sound and cooldown of 40), nautilus's breath on the rider
  (60 ticks, refreshed every 40). Brain: sensors (entities at 16, injury, temptation; five
  delays drawn at construction), anger 400 ticks against the attackable aggressor, cooldown of
  non-player attacks 2,400 to 3,600 (world random generator), one chance in two of charging the
  nearest seen pufferfish, charge (dash 0.6 / 0.5, hit, knockback ×2, cooldown 80), panic
  ×1.6 (land point only), temptation ×1.3 / ×0.9, love (nautilus only, baby
  tamed if the parent is), swim wander by steps. Return zone of the tamed
  nautilus (32 without saddle, 16 otherwise). Zombie nautilus: temperate or coral variant (biomes
  `spawns_coral_variant_zombie_nautilus`), burns in sunlight (body armour as protection), has neither
  age nor panic nor love; created under a drowned by `zombie_nautilus_jockey` (to be called by F08).
- **Metadata**: fish 16 bucket (+ 17 puffing / size / packed variant); squids and
  dolphin 16 baby, 17 age lock, glow squid 18 dimming, dolphin 18 fish received,
  19 moistness; nautiluses 18 taming flags, 19 owner, 20 dash, zombie nautilus 21
  variant (serialiser 32).
- **NBT**: `FromBucket`, `PuffState`, `type` (salmon), `Variant` (tropical fish),
  `DarkTicksRemaining`, `GotFish`, `Moistness`, `AgeLocked`, `Owner`, `Sitting`, `drop_chances`,
  `variant` (zombie nautilus), a brain's memories (anger with duration, cooldowns, panic).
- **Emptied fish bucket** (click on a block, after the use of the block itself): targeted cell if
  it accepts a liquid (liquid-containing block), otherwise the face's cell; water placed (replaceable
  or non-solid block destroyed first, containing, evaporation in the Nether), fallback to the
  touched face; fish created at the targeted cell (vertical offset, yaw from the world's random generator,
  "bucket" finalisation, name, size or patterns of the item, bucket data, "from bucket" flag),
  ambient sound; empty bucket returned outside creative. Differences: bucket sound sent to the player (the official
  client plays it by prediction), destruction without loot (common engine).
- **Remaining differences**: enlarged box of the pufferfish without position readjustment; splash sounds on entering the water
  (common engine); treasure: walks to the structure without checking the map of already known
  structures; boat following (boats not simulated); guardian flight (guardians not simulated);
  entity list order approximated by mob order; return zone with no effect on
  random positions (restriction not applied by the common engine).

### 4.F10 Singleplayer implementation: animals with a brain (2026-10-13)

Code: `rc-sim/src/mobs/brainy/` — generic brain (`brain.rs`), sheets, state, metadata,
NBT, spawn and hooks (`mod.rs`), registered memories (`nbt.rs`), one species per file
(`goat.rs` with the shared long jump, `frog.rs` with the tadpole, `armadillo.rs`, `sniffer.rs`,
`axolotl.rs`, `allay.rs`).

- **Generic brain**: memories with a lifetime (a memory set for n ticks lives n + 1
  agings; an empty list counts as "absent"), sensors (initial delays drawn on the entity's
  random generator at construction, in list order; period 20, armadillo's fear detector
  5), activities with conditions and memories erased on exit, behaviours arranged by
  priority then by hash table slot of the activity name (core always before the animals' other
  activities), then order added. One tick: age, sensors, start the stopped ones
  of the active activities, then tick or stop all those that are running (even outside the activity). Duration
  drawn on the world's random generator for each "timed" behaviour (even if min = max); the
  triggers draw none. "One of": key −U^(1/weight) on its own random generator, stable sort;
  "try everything" ordered gates (frog swimming, tadpole and axolotl rest).
- **Common behaviours**: swim (jumps 0.8 on the entity's random generator), panic (100..120, flight
  point on land 5/4 or nearest water when on fire), look at the target (45..90; head speed
  of the species, max pitch), go to the target (150..250; path kept, partial step 10/7 towards
  an unreachable target, recomputation if the target moves by more than 2 blocks, pause 0..39 when stuck),
  countdowns, "from time to time" look (30..60), love (110; birth after
  60..109 ticks within 3 blocks), temptation (endless; 100 ticks of cooldown), baby that follows
  the adult (5..16), land wander, swimming (steps 1/1 to 10/7) and flying (gaze cone,
  −2), walk towards the gaze, do nothing, attack (start, stop if invalid or unreachable
  for 200 ticks, approach, melee, cooldown), look for land, land near water, a
  liquid, random gaze.
- **Goat**: sensors (entities, players, items, adult, injury, food); activities
  ram > long jump > rest. Long jump: candidates of the 5/5 box (weight ⌈distance²⌉, world
  random generator), angles 65/70/75/80 shuffled, speed ≤ 3.571 × jump, parabola without collision (pose
  box × 0.7), refused if a short path (8) leads there; dash after 40 ticks, cooldown 600..1,200
  (half on failure). Ram: nearest seen target (not a goat; combat, sight, no
  range), start at 4..7 blocks along one of the four axes, 20 ticks of waiting (events 58/59), hit
  × 3.0: damage without anger, knockback = factor (2.5 adult, 1 baby) × clamp(speed × 1.65; 0.2;
  3) + 0.25 × (speed − slowness), horn broken on `snaps_goat_horn` (instrument of the tag drawn by
  the UUID). Screaming 2 %, single horn 10 %, damage 1/2 depending on age, milking, fall −10, head ±15°.
- **Frog**: sensors (entities, injury, prey, temptation, water); tongue > spawn laying > long
  jump > swim > rest. Prey: tag `frog_food`, size-1 cubes; tongue within less than 1.75 blocks
  (path up to 1.75 otherwise blacklist of 5 for 100 ticks), capture on the 7th tick, normal hit,
  prey removed; the magma cube drops its froglight through the loot table (condition
  on the killer's variant). Frogspawn laying (on the ground out of water, neighbouring water, air above).
  Long jump 2/4 towards the preferred blocks one time in two, landing without fluid. Croak
  60 ticks (pose 8), tongue (pose 9), jump (pose 6). Variants by biome, never a baby, fall −5,
  head speed 35, head ±5°, supple swimming, not pushed by currents.
- **Tadpole**: the fish's goals + brain (panic, look, temptation, swim); own age (+1 per
  tick unless locked), frog at 24,000 (conversion, "conversion" finalisation, persistent,
  sound), fed (+10 % of the remainder), golden dandelion (lock, age 0, cooldown 40), water bucket.
- **Armadillo**: states rest/rolling/scared/unrolling (durations 10, 50, 30), ball when hit
  by an entity (danger 80 ticks) or facing an undead, its aggressor, a player who
  sprints or rides (within 7/2/7, every 5 ticks), peeks (event 64), damage
  (d − 1)/2 when balled, body and head frozen, scute every 6,000..11,999 ticks, brush.
- **Sniffer**: scent (40..80) then dig target (5 attempts 10 + 2i, blocks of the tag never
  dug, reachable), search (600), dig (160..180; digging dimensions, seed at
  tick + 120), stand up (40), happy (40..100), cooldown 9,600; breeding → sniffer
  egg; baby at −48,000.
- **Axolotl**: play dead (1/3 then 1/3 on damage, in water; regeneration 200),
  hunt (always the hostiles of the tag, prey outside the 2,400 cooldown), support of the player who kills the
  target (regeneration, fatigue removed), air 6,000 (rain included, drying out), group
  variants (2 common, baby from the 3rd member), rare baby 1/1,200, bucket (variant, age, lock,
  hunting cooldown), "tropical fish bucket" food returned as a water bucket.
- **Allay**: given item (liked player), identical items picked up (range +1), drop-off at the liked
  note block (600 ticks, 1,024 blocks) or at the liked player (64 blocks), throw (0.2/0.3/0.2; sound one
  tick in 7), flight without gravity (flying control 20°), healing 1 HP / 10 ticks, duplication with
  amethyst while dancing (cooldown 6,000), insensitive to the liked player's hits, network tracking every
  2 ticks.
- **Metadata**: age 16/17; goat 18 screaming, 19/20 horns; frog 18 variant
  (serialiser 27), 19 tongue target; armadillo 18 state (36); sniffer 18 state (35), 19 tick of
  the seed; axolotl 18 variant, 19 plays dead, 20 from bucket; allay 16 dance, 17 duplication;
  tadpole 16 from bucket, 17 lock; pose (6) for jumps, croak, tongue.
- **NBT**: `IsScreamingGoat`, `HasLeftHorn`, `HasRightHorn`, `variant` (frog), `Age`/
  `AgeLocked` (tadpole), `state`, `scute_time`, `Variant`, `FromBucket`, `Inventory`,
  `DuplicationCooldown`, a brain's memories (cooldowns, panic, danger, digs, liked player and note
  block…).
- **Remaining differences**: player's shield not simulated (ram knockback never divided); jukebox
  (no playback simulated: the allay therefore never dances); note block vibrations without travel
  delay or occlusion; frog evaluator ("open" cell above the preferred blocks, shallow
  swimming, corners of water edges) not carried over; position readjustment
  after the tadpole's conversion; leash; particles (client-side); "player" sounds (milking,
  item given to the allay) sent to the player instead of being predicted by their client; residence
  zone ignored by random positions (common engine).

### 4.F11 Singleplayer implementation: bee, fox, panda (2026-10-13)

Code: `rc-sim/src/mobs/wild/` (sheets, state, metadata, NBT, hooks: `mod.rs`; bee:
`bee.rs`; hives and nests: `hive.rs`; fox: `fox.rs`; panda: `panda.rs`; goals: `goals.rs`;
aerial and land random positions: `rpos.rs`).

- **Sheets**: bee 0.55 × 0.5 (eyes 0.3), 10 HP, speed 0.3, flight 0.6, volume 0.4, follow range 8,
  no fall nor step nor ambient sound; fox 0.6 × 0.7 (eyes 0.4; baby × 0.6), 10 HP, follow range 32,
  safe fall 5; panda 1.3 × 1.25 (baby ½, eyes 0.28125), 20 HP, 0.15, 6 damage, step "panda.step".
  All three: animals, food `bee_food` / `fox_food` / `panda_food`, floating navigation
  (the "float" goal, built after the bee's flying navigation, makes it floating).
- **Metadata**: 16 baby, 17 age frozen; bee 18 flags (2 roll, 4 has stung, 8 nectar),
  19 end of anger (long); fox 18 variant (int), 19 flags (1 sitting, 4 crouching,
  8 interested, 16 pouncing, 32 sleeping, 64 head stuck, 128 defending), 20 and 21 trust (UUID); panda
  18 unhappy, 19 sneeze, 20 meal, 21 main gene, 22 hidden gene, 23 flags (2 sneezes,
  4 rolls, 8 sitting, 16 on back). Item in hand resent when it changes.
- **NBT**: bee `hive_pos`, `flower_pos`, `HasNectar`, `HasStung`, `TicksSincePollination`,
  `CannotEnterHiveTicks`, `CropsGrownSincePollination`, `anger_end_time`, `angry_at`; fox
  `Trusted`, `Sleeping`, `Type`, `Sitting`, `Crouching`; panda `MainGene`, `HiddenGene`; all three
  `NoGravity`, `AgeLocked`.
- **Bee**: flying control (turn 20, hover: gravity stays off), look
  absent when angry, pitch kept during pollination; the game's goals (sting ×1.4 when angry without
  having stung, entering the hive, breeding, temptation, hive and flower validations
  20 to 40 ticks drawn at construction, pollination, following the parent, hive search (POI at 20,
  3 blacklisted), trip to the hive (aerial steps beyond 16, direct path otherwise, 60 ticks
  stuck), known flower after 600 ticks, crop growth, wander (hover, otherwise
  air and water), float; retaliation that only alerts the bees in sight, anger against the player,
  universal anger). Persistent anger 20 to 39 s, drowning after 20 ticks
  in water, slow death after the sting (1 in 1,200 − t every 5 ticks), poison 10 s (normal)
  or 18 s (hard), stinger stuck in the player (metadata 13, removed every 20 × (30 − n)
  ticks). Flowers with effects given: open eyeblossom (poison 25), wither rose (wither 40).
  "Stay in the hive": night of the timeline (12,542 → 23,460) in the normal world, rain or thunderstorm.
- **Hive / nest**: 3 occupants (entity data without the ignored fields, ticks spent, 2,400
  with nectar otherwise 600), block entity that ticks with the hoppers, exit refused at night, in
  rain and in front of a block (except emergency), bee exiting without gravity, reattached to the hive, age
  and love reduced by the time spent, hive flower taken back 9 times in 10, nectar deposited (honey +1,
  1 in 100: +2), buzzing 1 in 200. Harvest (honey 5) with shears (table `harvest/beehive`)
  or bottle; without a lit campfire below (5 blocks, smoke stopped by a block touching the
  central post): anger of neighbouring bees and emergency exit (bees within 4 blocks target
  the player, or stay outside 400 ticks if smoked). Broken by the player without silk touch:
  emergency exit and anger.
- **Fox**: variant of the biome (`spawns_snow_foxes`) or of the group, babies from the 3rd of the group,
  item in the mouth (20 %; emerald, egg, rabbit foot / hide, wheat, leather, feather), targeting goals
  according to variant; 21 goals of the game (daytime sleep in shelter, stalking and pounce, head stuck in
  snow, shelter, berries, items, perch, village wander at night, melee without gesture with
  audible bite…); defence of trusted beings (growl, waking); meal of the item in the mouth after
  600 ticks (sound and particles drawn on its random generator), pickup (one piece, the old item spat out,
  delay 40), item dropped on death; night cry at volume 2 with no player within 16 blocks.
- **Panda**: genes drawn out of 16 (world random generator), recessive brown and weak, attributes (weak 10 HP,
  lazy 0.07), heredity (one gene from each parent, 1 in 32 mutation); babies at 20 %; the game's
  goals (panic stopped when sitting, breeding requiring bamboo at 8 blocks over 3 layers otherwise
  unhappy 32 ticks and look at the player, melee, temptation, flight of the worried, sitting to eat,
  on the back, sneeze (slime ball, neighbouring adults jump), 32-tick roll);
  fear of thunderstorms; bamboo given: baby grows, adult in love or sitting to eat.
- **Spawn**: fox and panda without placement restriction; fox rule
  (`foxes_spawnable_on`, light > 8), panda: animal rule. Generated nests tick as soon as their
  chunk is loaded.
- **Remaining differences**: explosion and neighbouring fire of a hive (emergency exit) not wired;
  hive broken in creative (item with bees); block entry effect (open eyeblossom, wither
  rose, berry bush) not simulated by the common engine; use of an item consumed by the
  fox without consumption effects; block steps for the fox (common
  engine); mob that entered a hive on its first tick never announced to the client (tracking at
  end of tick); client rendering: panda genes (brown texture, weak…) not drawn.

### 4.F12 Singleplayer implementation: Nether hostiles (2026-10-13)

Module `rc-sim/src/mobs/nether/` (`mod.rs` sheets, `Neth` state, data, NBT, hooks; `blaze.rs`,
`ghast.rs`, `strider.rs`, `goals.rs` goals `Goal::Neth`) and `mobs/fireballs.rs` (shooting and deflecting fireballs; flight and impacts in the common engine `projectiles/`, see `server-projectiles.md`).
The wither skeleton is a skeleton-base variant (`Kind::base`): goals, bow, arrows and
weapon re-evaluation come from F01.

- **Blaze**: 0.6 × 1.8, HP 20, damage 6, speed 0.23 (f32), follow range 48, 10 experience, fire-immune,
  hurt by water and rain (1/tick). Light "magic" value 1: idle time always wears down by 2. Start of the AI step: in the
  air and falling, vertical speed × 0.6. The species' own server AI step:
  height-change countdown; at 0 → 100 and offset = triangle(0.5; 6.891) on the
  entity's random generator; target higher than the eye + offset and attackable → vy += (0.3 − vy) × 0.3. Goals: 4
  attack (movement + look, every tick), 7 wander avoiding water (probability 0: always a
  "land" point, draw `next_float` consumed), 8 look at player 8, 8 look around; targets:
  1 retaliation with alert, 2 player (sight, interval 10). Attack: timer −1 per tick; sight
  → "seen since" 0 otherwise +1; distance² < 4: without sight nothing, otherwise hit every 20 and wanted point
  on the target; otherwise distance² < follow² and sight: when due step +1 (1 → 60 ticks, charged;
  2 to 4 → 6 ticks; 5 → 100 ticks, step 0, discharged) and, beyond step 1, event 1018 then a
  small fireball (direction = triangle(dx, 2.297 × √√d × 0.5), dy, triangle(dz, …)), start at mid-height
  + 0.5; look (10, 10); otherwise seen less than 5 ticks ago → wanted point. Stop: discharged. Data 16:
  byte, bit 1 = charged (the client shows it on fire).
- **Small fireball** (0.3125): speed = direction × 0.1, acceleration 0.1, inertia 0.95
  (0.8 in water), smoothed rotation 0.2. Entity hit: fire 5 s, 5 `fireball` damage
  (`unattributed_fireball` without an author); fire given back if the damage is refused (blaze, ghast
  immune). Block hit: fire placed in the cell in front of the face if the author is not a
  mob or if `mob_griefing`. Removed on impact, when its author is removed from the world or out of a
  loaded chunk. Network tracking: spawn with the author, "on fire" flag, position and
  velocity every 10 ticks.
- **Ghast**: 4 × 4 (eyes 2.6), HP 10, follow range 100, `flying_speed` 0.06, volume 5, fire-immune,
  no fall damage, is not a code "monster" (no wear-down in light, walk value
  0) but remains in the monster category (hostile sounds, not in peaceful). Movement: flying
  movement (air 0.91, water 0.8, lava 0.5, input × 0.02, no gravity). Control: in "go to", on
  expiry (post-decrement) duration += 2..6 (entity's random generator) then, if the path is clear, velocity
  += direction × 0.1, otherwise wait. Clear path: walk of the game's cells (box at the start, "farthest"
  corner walked cell by cell with a box the size of the entity, arrival box;
  axis combination of the corner reproduced as is), each non-empty cell outside the start box
  tested by sliding the box. Goals: 5 floating (without a wanted point, or < 1 or > 60 blocks;
  never continued; point ±16 on the entity's random generator), 7 look (yaw towards the target at < 64, otherwise
  towards the velocity; body aligned), 7 shooting (without flag); target: player at 4 blocks of height at most
  (sight, interval 10). Shot: target at < 64 and sight → charge +1 (10: event 1015; 20: event
  1016, large fireball starting 4 blocks ahead in the plane, mid-height + 0.5, charge −40); otherwise
  charge −1 if > 0; data 16 (charge) = charge > 10. Spawn: difficulty, 1/20, support block;
  group 1. NBT `ExplosionPower` (byte, 1).
- **Large fireball** (1 × 1): hit → 6 `fireball` damage, then explosion of its power
  (type `player_explosion` with an author, otherwise `explosion`; fire follows `mob_griefing`: one cell
  in three destroyed, empty on a full block, on the world's random generator after the shuffle). Player's hit
  (range 3 + 3): velocity = player's gaze, the player becomes the author, acceleration 0.1, sound
  `attack.nodamage`. Deflected onto a ghast: 1,000 damage despite fire immunity. The small
  fireball cannot be deflected (hit without effect).
- **Wither skeleton**: 0.7 × 2.4 (eyes 2.1), skeleton base (speed 0.25), fire-immune,
  burns in sunlight (no effect), immune to the wither effect; lava penalty 8; rider offset
  −0.875. Targets: 3 piglins (before the skeleton's; `Who::Piglin`, active as soon as F13 creates the
  piglins). Finalisation: after the common spawn finalisation, stone sword (no armour draw, no
  enchantment), weapon re-evaluated (melee × 1.2), pickup 0.55 × special, base damage 4 (saved
  with the attributes). Successful hit → wither 10 s. Arrows (bow forced by NBT) set on fire: fire 5 s
  to the entity hit (given back if refused), extinguished in water.
- **Strider**: animal 0.9 × 1.7 (baby 0.45 × 0.85, eyes 0.4375), HP 20, speed 0.175, fire-immune,
  hurt by water. Own collision world: a lava source with no lava above carries
  a 0..0.5 shape when the strider's bottom (at the start of the move) is above
  y + 0.5 − 1e-5. Lava cell → "ground" movement (no swimming). Fall reset to zero in
  lava. End of tick: in lava, standing if above the shape with no lava above, otherwise
  speed × 0.5 + 0.05 in y. Before the tick: sounds (tempted 1/140 "happy", panic 1/60 "retreat",
  entity's random generator) then cold = neither lava in/under it nor lava height nor strider mount
  that is warm → `suffocating` modifier −0.34 (base) and data 19; attributes resent to the client on
  each change. Goals: 1 panic 1.65, 2 breeding, 3 temptation 1.4 (`strider_tempt_items`),
  4 go to lava (range 8/2, targets the lava block, path recomputed every 20 ticks, stops in
  the lava), 5 follow the parent, 7 wander (60), 8 look at player, 8 look around, 9 look at another
  strider. Penalties: water −1, lava and fire 0. Walk value: lava 10, otherwise −∞ if it is in
  the lava, 0 elsewhere; lava is a stable destination. Interaction: without food, saddled,
  without passenger, not sneaking → mount (refused with eyes in lava); otherwise food (love, baby
  that grows), golden dandelion, saddle (adult, `split(1)` even in creative, certain drop); eating
  sound. Steering: saddled + player first + stick in hand (indicator recomputed every tick);
  the client moves (speed × 0.55, 0.35 chilled, × boost); server: rotation, the acceleration tick. Stick
  used on the steered strider → boost 140 + U(841) ticks (strider's random generator, data 18), stick
  worn by 1, broken → fishing rod. Dismounting: five directions (gaze, ±22.5, ±45), from the top of the
  box to 0.5 below the bottom, outside lava. Spawn: "in lava" placement, lava column that
  opens onto air, obstruction by entities only; finalisation described in § 4.2 (zombified piglin
  rider created as soon as F13 provides the species; the saddle is placed in all cases).
- **Wiring**: effects (S7: wither, wither skeleton refusal), common explosion (creeper and
  ghast fireball, `explode_core`), commands (S5: `summon` finalises without compound), dimensions (S6:
  Nether ambient light 0.1 in the "magic" value), fortress spawns (F07,
  `structure_spawn.rs`).

### 4.F14 Singleplayer implementation: warden, breeze, creaking, happy ghast (2026-10-17)

Module `rc-sim/src/mobs/recent/`: `mod.rs` (sheets, per-mob `Rec` state and world `RecWorld`,
poses, data, NBT, hooks of the common engine and of the block engine), one sheet per species
(`warden.rs`, `breeze.rs`, `creaking.rs`, `happy_ghast.rs`), the linked blocks (`heart.rs` creaking
heart, `shrieker.rs` sculk shrieker, `dried_ghast.rs` dried ghast), `vibration.rs` (game
events and vibrations), `wind.rs` (wind charges, carried by the common projectile engine) and
`xbrain.rs` (extension of the F10 brain: memories `K::X`, behaviours `Bk::X`, sensors `Sn::X`).

- **Sheets**: warden 0.9 × 2.9 (eyes 0.85 × h), HP 500, speed 0.3, damage 30, follow range 24, knockback
  resistance 1, volume 4, fire-immune; breeze 0.6 × 1.77 (eyes 1.3452), HP 30, speed 0.63,
  damage 3, 10 experience; creaking 0.9 × 2.7 (eyes 2.3), HP 1, speed 0.4, damage 3, follow range 32,
  step 1.0625, no experience; happy ghast, animal 4 × 4 (eyes 2.6), HP 20, flight 0.05, baby at
  scale 0.2375, no fall damage, fixed voice 1 (no draw), neutral sound category,
  volume 4 (1 baby).
- **Warden**: brain (emerge, dig, roar, fight, investigate, sniff, rest). Anger per
  suspect (150 at most, −1 per second, players first); hit, +100 against the author even if the
  hit is refused, target taken if the hit is direct or within 5 blocks; pushed, +35 once per
  second. Vibrations (radius 16 at the eyes, cooldown 40): +35 against the source, +10 the first time
  for a projectile (35 if recent), otherwise disturbance to investigate. Sonic beam (10 damage, knockback
  2.5 / 0.5), darkness at 20 blocks every 6 s, invulnerable while digging or emerging, not
  pushable then, ignores explosions. Spawn: by the shrieker only (emerges,
  reason "triggered"); dig cooldown of 1,200 set at finalisation, refreshed if
  persistent or ridden. Steps: sound 10 every 0.55 of distance.
- **Breeze**: brain (rest, fight: shot, long jump behind the target, slide); ambient sounds and
  particles client-side but their draws made at the server tick; landing sound beyond
  3 blocks of fall; immune to a breeze's damage; deflects any projectile except wind
  charges ("deflect" sound bound to the entity, U-turn × 0.5, rotation 170..190, once per breeze).
  Steps: event only (no sound).
- **Wind charges** (S8 engine, kinds `wind_charge` and `breeze_wind_charge`): 0.3125, box
  lowered by 0.15, inertia 1 (air and liquid), acceleration 0, neither fire nor item (no data 8,
  no `Item` in NBT), do not hit other charges. Entity hit: the target becomes the
  author's last victim, 1 `wind_charge` damage, then gust at the position; block: gust at
  0.25 in front of the face; beyond the maximum height + 30: gust. Gust (interaction
  "trigger", no damage): radius 3 (breeze, big puff, sound `breeze.wind_burst`) or 1.2
  with knockback × 1.22 (player, small puff, sound `wind_charge.wind_burst`); rays as in an
  explosion (only the `blocks_wind_charge_explosions` blocks stop them), knockback according to the visible
  share, blocks triggered in a shuffled order (levers, buttons, non-iron doors and trapdoors,
  fence gates, candles). Player: charge at the eyes, shot 1.5 / inaccuracy 1, then throw sound (world
  random generator), stack consumed, component cooldown.
- **Creaking**: brain (core, rest, fight). Looked at by an attackable player (cone 0.5, three
  heights, line of sight cleared by the visual shapes; pumpkin mask ignored once active):
  frozen (navigation, controls, jump and body stopped; neither knockback nor push), activated within
  12 blocks. Bound to a heart: invulnerable (except `bypasses_invulnerability`), animation 8, event
  66, heart's distress call if a player hits; fire-immune; path limited to 32 blocks from the
  heart; dead or heart broken: undoes itself in 45 ticks (wood and heart fragments, death sound).
- **Creaking heart** (block entity, ticker): comparator signal (distance to the creaking, 0 uprooted),
  state re-evaluated every 20..24 ticks (uprooted without aligned logs, awake from 12,600 to 23,400
  in the normal world, otherwise dormant), protector placed at night near a player (≤ 32) by
  the utility spawn (5 attempts, ±16 / 8, support without leaves) then sounds, sent back if it is
  day without persistence, beyond 34 blocks or if the player stays stuck in it; distress: 100
  ticks, resin scattered 2..3 times (awake); broken by a player: protector killed, 20..24
  experience for a natural heart; removed: protector undone.
- **Sculk shrieker**: listens (radius 8); a player who walks on it makes it shriek (sneaking
  included); player's warning level (cooldown 200, decreases every 12,000); response after
  90 ticks: warden at level 4 (20 attempts ±5/6), otherwise distant sound; darkness at 40 blocks.
- **Vibrations**: broadcast to the listeners of the neighbouring sections (radius 16, 32 for the shriek), filter
  (listening label, spectator, not sneaking-cautious, entity or block that dampens), occlusion by
  wool, selection of the nearest at the tick, travel of one tick per block. Emitted: steps of mobs and
  of the player (the game emits the step even for a "sounds" emission), swimming, block destroyed by the player,
  creaking actions, gusts, shriek.
- **Happy ghast**: baby led by a brain, adult by goals (float, temptation, random
  floating away from blocks); harness (body slot): four seats (front, left, back,
  right at 4 high), piloted by the first player passenger outside stillness; yaw that catches up
  8 % of the gap, pitch by half; still for 10 ticks at each dismount or when a player stands
  on it (position resynchronised at once, then precise position on each send); dismount on the
  top of the box. Dried ghast: random tick → scheduled tick at 5,000; waterlogged,
  hydration up to 3 then hatching of a baby turned like the block.
- **Save**: anger (`anger.suspects`), listener (`listener`), serialisable brain memories
  (dig, scent, sonic beam cooldowns, breeze's shot and jump state, jump target, temptation),
  creaking's `home_pos`, happy ghast's `still_timeout`,
  `creaking` (UUID) in the heart's block entity.
- **Remaining differences**: (trial spawner done in W9, `mobs/trial.rs`, see § "Spawner
  blocks" below); sculk sensors simulated since G6-F10 (`blocks/sculk_sensor.rs`: listening, phases, calibrated, not forced, resonance, clicking towards the shrieker);
  vibration particle sent since G6-F10 (destination and path); trail particle not sent (our particle format has no
  destination); the gust pushes neither items on the ground nor projectiles; few game events emitted
  outside steps, broken blocks and gusts (block placement, container opening, eating… missing);
  deflection of a wind charge hit by the player not done; "author" spawn data
  of the breeze's charges at 0 (the breeze is out of the list during its tick).

### 4.F15 Singleplayer implementation: Ender Dragon, End fight, shulker (2026-10-17)

Module `rc-sim/src/mobs/end/`: `mod.rs` (sheets, `EndMob` state, `EndWorld` world state, hooks),
`dragon.rs` (own tick), `phases.rs` (11 phases), `graph.rs` (flight graph), `crystal.rs`,
`fight.rs` (fight), `breath.rs` and `bullet.rs` (on the common projectile engine, S8),
`shulker.rs`, `goals.rs` (`Goal::End`).

- **Dragon**: 16 × 8, HP 200, follow range 10 chunks, volume 5, fire-immune, without physics (never
  on the ground, no block collision: the phases' collision tests are never true), never
  removed by the despawn test, refuses any *applied* effect (but remains "affectable": a cloud or a
  instant damage potion touches it, only a player's damage counts). Own tick: no
  common AI or movement; flight history (64 height/yaw samples); phase ticked,
  re-ticked once if it changed; steering towards the phase's target (vertical speed
  limited by the flight speed, damped turn 0.8, push 0.06 weighted by alignment, drag
  0.8..0.95, vertical × 0.91; × 0.8 in a "wall"). Eight parts (head, neck, body, three tails,
  two wings; ids dragon + 1..8) placed after the steering; wings: knockback (× 4, + 0.2) and
  5 damage outside the sitting phase if the last injury by a mob dates from at least 3 ticks; head
  and neck: 10 damage; contacts tested only without `hurt_time`, creative/spectator excluded.
  "Walls": head, neck, body; non-empty blocks outside `dragon_transparent` removed if
  `mob_griefing` and outside `dragon_immune` (event 2008 on a drawn cell), otherwise wall. Nearest
  crystal (1 time in 10, box + 32): + 1 HP every 10 ticks. Dying: three draws of the
  entity's random generator per tick (client particles), also between 180 and 200 ticks of agony.
  Damage per part: nothing during agony; sitting phase: arrow, trident, wind charge → 0 and the
  arrow catches fire 1 s; outside head and neck → d/4 + min(d, 1); applied only if the author is
  a player or if the type carries `always_hurts_ender_dragons` (the hit still "lands"); sitting,
  more than 25 % of max HP accumulated → takeoff. Fatal blow outside the sitting phase: HP 1 and death phase
  ("dying" posture sent). Agony 200 ticks: + 0.1 in height per tick, experience (12,000
  for the world's first dragon, 500 afterwards: 8 % every 5 ticks after 150, 20 % at the end),
  global event 1028 on the first tick, victory signalled to the fight. NBT `DragonPhase`,
  `DragonDeathTime`, `sitting_damage_received`. Data 16: phase.
- **Projectiles and parts**: the dragon is never targeted, its parts are (`Part` target); a
  part hit passes the damage on to the dragon through that part, without knockback or arrow effect (it
  is not a living entity); a fishing rod can hook a part (no pull).
- **Graph**: 24 nodes (12 at 60 blocks, 8 at 40 + 10 higher, 4 at 20; height at least 73 on
  "motion blocking without leaves"), neighbourhoods by masks; with no living crystal, only
  nodes 12..23 are used. A* on a binary heap (ties broken like the game), partial path towards
  the nearest node reached, final node added.
- **Dragon fireball** (`dragon_fireball`, 1 × 1, acceleration 0.1, no fire, tracking 4/10): start
  1 block in front of the head, + 1 in height; on impact (except on its launcher, a part is not
  one): event 2006 then breath cloud (radius 3 → 7 over 600 ticks, instant damage II, duration
  scale 0.25), centred on the first living entity of the 4/2/4 inflated box within 4.
- **Cloud**: common engine (`projectiles/cloud.rs`), own particle `dragon_breath` (NBT
  `custom_particle` without `power` when it is 1; data 10 = particle only, serialiser 16);
  on a player, the instant damage of a cloud whose author is a mob is attributed
  indirect magic (difficulty, knockback from the cloud, death message). Sitting dragon's flame:
  cloud of radius 5 for 200 ticks (instant damage I), removed at the end of the phase.
- **Crystal**: 2 × 2, tracking 16 without position update; fire maintained below during a
  fight; destroyed by the slightest hit (except from the dragon, or invulnerable outside a creative player),
  explosion 6 of "block" type unless it dies from an explosion; fight notified; beam and base
  synchronised; placed by the item on obsidian or bedrock (free space), which tries the
  respawn.
- **Fight** (`<dimension>/data/minecraft/ender_dragon_fight.dat`): pink boss bar (UUID drawn
  from the world's random generator, music and fog) for living players within 192 blocks of (0, 128, 0),
  reviewed every 20 ticks; arena kept loaded; first pass: inactive exit portal placed
  if missing, dragon kept only if the portal was active; dragon searched/created (at (0, 128,
  0), yaw on the world's random generator, phase "circle") if none seen for 1,200 ticks; pillar
  crystals counted every 100 ticks. Victory: active portal, gateway (last of the list
  of 20 shuffled by the seed, radius 96, y 75, event 3000), egg for the first dragon. Respawn:
  four crystals around the portal → podium removed, steps (beams, pillars rebuilt, new
  dragon), abandoned if a respawn crystal disappears.
- **Shulker**: 1 × 1 (eyes 0.5, stretched by the opening when placed on the ground), HP 30, 5 experience,
  fire-immune, immobile (speed always zero, position snapped to the block centre), never
  removed far from players. Attachment: full face, room to open; otherwise new face, otherwise
  teleport (5 attempts at ±8, empty cell, no collision, attachable face outside
  `shulker_does_not_teleport_to`, event 2016, opening reset to 0 without armour or sound).
  Opening 0..100 → box that grows outwards and pushes entities; closed: armour + 20
  ("covered"), opening and closing sounds, immune to arrows and tridents. Hurt: below
  half HP, 1 chance in 4 (drawn even when dead) to teleport; otherwise hit by a
  shulker bullet while open: teleport, then duplication (fails with probability (n − 1)/5, n = living
  shulkers within ±8, world random generator) into a shulker of the same colour, without finalisation. Goals: 1 look at
  the player in the plane (0.02), 4 attack (open to 100, shot every 20 + U(10) × 10 ticks within
  20 blocks, sound 2.0), 7 peeking (30), 8 look around; targets: retaliation (not against a
  shulker) with alert, player. NBT `AttachFace`, `Peek`, `Color`; data 16..18. Spawn:
  End city sentinels (generation), no natural spawn.
- **Shulker bullet** (0.3125, gravity 0.04 without a living target, tracking 8 chunks / 3 ticks, no
  physics): starts at the shulker's centre, "up" direction, next segment chosen at once away from
  the attachment axis; segments of 10..50 ticks on a free axis towards the target (draw of a random
  direction always made), target speed 0.15 × 1.025 per tick; impact: 4 "mob projectile"
  damage, levitation 10 s; block: explosion (2 particles) and sound; destroyed by a hit, an
  explosion or in peaceful. NBT `Target`, `Dir`, `Steps`, `TXD`/`TYD`/`TZD`.
- **Wiring**: projectile engine (S8: cloud, fireball, bullet, targeted parts), common explosion
  (`explode_with`, crystals and bullets hit), commands (`summon`, `kill`), dimensions
  (End fight parked with its dimension), End generation (`feature_end.rs`: pillars,
  crystals).

### 4.F16 Singleplayer implementation: wither (2026-10-17)

Module `rc-sim/src/mobs/wither/` (`mod.rs` sheet, `Wither` state, data, NBT, `Goal::Wither` goals,
flight control; `boss.rs` behaviour, bar, damage, death; `summon.rs` construction; `skull.rs`
skulls). The skulls fly in the common projectile engine (kind `WitherSkull`, see
`server-projectiles.md`); the construction pattern reuses the golems' (`golems/build.rs`).

- **Sheet**: monster 0.9 × 3.5 (eyes 2.975), HP 300, speed and `flying_speed` 0.6 (f32), follow range 40,
  armour 4, 50 experience, fire-immune, undead (`wither_friends` = `#undead`), client
  tracking 10 chunks, velocity never tracked (only on forced synchronisation).
  Sounds `entity.wither.ambient/hurt/death`. Never despawns outside peaceful (idle reset to 0
  at each test). Refuses any effect (false, command included). Flying navigation (closed
  doors, floats); flight control with turn 10, no hover (gravity given back when stopped,
  data 5 `NoGravity`).
- **Construction**: a wither skeleton skull (floor or wall) placed by a player or a
  dispenser, outside peaceful and above the world bottom, completes the pattern "^^^ / ### / ~#~"
  (`^` skull, `#` tag `wither_summon_base_blocks`, `~` air) searched from the skull in the 24
  frames. The wither is built (entity UUID then bar UUID on its random generator, without
  finalisation), the pattern erased (air flag 2, event 2001 per cell, air cells included), then
  placed at the foot of the T (+0.5, +0.55, +0.5), yaw and body 0 if the pattern's front axis is x, otherwise 90,
  head 0; summoning: 220 ticks, bar at 0, HP = max/3; player's `summoned_entity` advancement within 50
  blocks; added to the world; cell neighbours notified. Dispenser: empty cell in front, two blocks above
  the world bottom, base pattern (without skulls) → skull placed turned like the facade (segment
  = 2D value × 4: south 0, west 4, north 8, east 12, vertical 0) then same check; otherwise failure
  (1001), the equipment of a mob in front is not simulated.
- **Start of the AI step**: speed y × 0.6; if the central head has a target (network id):
  below it, or without armour within 5 blocks above, vy = max(vy, 0) then vy += 0.3 − vy × 0.6;
  more than 3 blocks away horizontally, vx/vz += direction × 0.3 − v × 0.6. Horizontal speed² > 0.05:
  yaw = atan2(vz, vx) − 90. End of the AI step: rotations of the side heads (server only), then
  particle draws that advance the random generator: per head 3 Gaussians (entity), and in armour
  `next_int(4)` (world) then 3 Gaussians if 0; while summoning 3 × (Gaussian, float,
  Gaussian).
- **Summoning**: count − 1, bar = 1 − count/220; at 0 explosion of
  power 7 at eye level (mob interaction, type `player_explosion`, wither excluded) and
  global event 1023 if it is not silent; healing of 10 every 10 ticks (`ticks_lived`). No
  other mechanic during this time; the "do nothing" goal (0; movement, jump, look) blocks
  the others, targets are still chosen.
- **Side heads** (outside summoning): for i = 1, 2, when due, next = tick + 10 + U(10);
  on normal and hard, wait +1 and beyond 15 fires a blue skull towards a point U(±10, ±5, ±10)
  then wait 0; known target: attackable, at 30 blocks at most and in sight → skull towards the middle of
  its eye height, next = tick + 40 + U(20), wait 0; otherwise target forgotten; without target:
  draw of a non-undead living entity in the box inflated by 20 / 8 / 20, at 20 blocks and in
  sight. Then central target = goal's target, blocks destroyed, healing of 1 every 20 ticks, bar = HP/max.
- **Goals**: 0 do nothing (during summoning), 2 ranged attack (1.0; 40; 20: approach if
  > 20 blocks or seen for < 5 ticks, look 30/30, central head fires every 40 ticks if
  in sight; blue skull 1 time in 1,000), 5 random flight avoiding water (1/120, hover point
  in front then air or water), 6 look at player 8, 7 look around; targets: 1 retaliation, 2 living
  entity other than undead (interval 0, sight always checked).
- **Shot**: event 1024 if it is not silent, skull placed at the head (central: y + 3; side heads:
  y + 2.2 and ±1.3 according to the body's yaw), velocity = normalised direction × 0.1, author's rotation.
- **Skull** (0.3125, tracking 4 chunks every 10 ticks): inertia 0.95 (blue 0.73; water 0.8), never
  on fire, cannot be deflected. Entity hit: with author 8 `wither_skull` damage (victim killed → author
  healed by 5), without author 5 `magic`; if hit: wither level 2 for 10 s (normal) or 40 s
  (hard). Then explosion of power 1 (`player_explosion` with author), no fire; blue skull:
  resistance of destructible blocks capped at 0.8 (water and obsidian included). NBT
  `acceleration_power`, `dangerous`; data 8 = blue.
- **Damage**: refused for fire, drowning (`wither_immune_to`), during summoning (except
  `bypasses_invulnerability`), in armour for arrows, tridents and wind charges, and when
  the author is a wither or an undead. Accepted: block destruction armed at 20 ticks (if at 0)
  and head wait + 3. Destruction (rule `mob_griefing`): cells of the box ⌊w/2 + 1⌋ around,
  height ⌊h⌋ above (x, then y, then z), outside air and tag `wither_immune`, destroyed with loot;
  event 1022 if at least one.
- **Boss bar**: purple, continuous, darkened screen; UUID drawn at construction, name = displayed
  name at the time ("entity" hover and insertion of the construction UUID; reread with a
  custom name: redone), player registered when they see the wither, removed when it leaves sight or on
  removal; progress sent only if it changes. Death freezes the bar (no more AI).
- **Death**: loot, then Nether star (long lifetime, age −6,000); experience. A mob
  or the player whose credited killer is a wither leaves a wither rose on its cell (empty,
  `mob_griefing`, support) or drops it.
- **NBT**: `Invul` (integer), `NoGravity`; attributes `flying_speed` and `armor` added to the engine's.

## 5. What guarantees a faithful reproduction

### 5.1 What "faithful" means here

The official game's random generators (each world's and each entity's)
are seeded at creation by a "unique" seed derived from the clock: two
official games never draw the same numbers. A **bit-for-bit** reproduction of an
official game is therefore impossible and is not aimed at. The fidelity aimed at is:
1. same rules, same constants, same formulas at the same precision (`f32`/`f64`);
2. same **consumption structure** of the random draws (same generator, same number of draws,
   same order, including the "useless" draws server-side); thus the distributions,
   the correlations and the edge cases are identical;
3. same update **order** (world, chunks, entities, goals, behaviours);
4. on the Rust side, generators **seeded reproducibly** (world seed, counter) to
   replay a game identically and write non-regression tests.

Generator: the game's "historical" generator (Java's 48-bit linear congruential:
multiplier 0x5DEECE66D, increment 0xB, `next_int(n)` with rejection, `next_float` on 24 bits,
`next_double` on 53 bits, polar `next_gaussian` with reserve) for the world and for each
entity; this is what `rc-audio` already reproduces (`JavaRandom`). The integer draw between two inclusive bounds (a,
b) = `next_int(b − a + 1) + a`.

### 5.2 Order of a world tick (server)

1. world border, **weather**; sleeping (skip to morning); sky brightness; **time** +1;
2. scheduled **block** ticks, then **fluid** ticks (65536 at most each);
3. **raids**;
4. **chunk source**: spawn census (§ 2.2), allowed categories,
   **shuffled** candidate chunks (world random generator), then for each: inhabited time, lightning and
   snow/ice, natural spawning; then random block ticks of the chunks that
   tick; then **special spawns** (phantoms, patrols, cats, siege, trader);
   sending of block changes;
5. **block events** (pistons, bells…);
6. if the world has not been empty for 300 ticks: dragon fight (End), then **entities**;
   then block entities;
7. entity management (loading/unloading of sections).

**Entities**: the list of ticking entities is a table **ordered by insertion**
(an entity enters it when it is added in an active zone or when its section becomes
active, and leaves it when it is removed or its section goes to sleep: on coming back, it goes
to the end). Iteration is done on a **snapshot**: an entity added during the entity
tick is only ticked on the next tick; an entity removed along the way is skipped. For each one, in this
order: if removed, nothing; if frozen (command `tick freeze`), nothing; otherwise **despawn
check** (§ 2.9); then, if the entity is a player or in a chunk with entity
ticks: if it is a passenger, it is skipped (its vehicle will tick it); otherwise common
tick + the entity's tick, then recursively its passengers (common tick + "passenger
tick"), in the order of the passenger list; a passenger is only ticked if it is
a player or in the list of ticking entities.

**Entity ids**: global increasing counter (the first free one ≥ 1). The parity of
(`ticks_lived` + id) sets the even/odd ticks of the goals (§ 3.1): ids must be assigned
in the same order (creation at load, spawns, projectiles…).

### 5.3 Which random generator for what

- **World generator**: natural spawns (position, type, size, yaw, budget), shuffling
  of the chunks, lightning, random ticks, special spawns (except trader), **start
  durations of brain behaviours** (even with a fixed duration: `next_int(1)` is drawn),
  random pause after a brain path is blocked, spawn rules (light),
  finalisations that ask for it.
- **Entity generator**: ambient sounds (one `next_int(1000)` **on every base tick**),
  random despawn (`next_int(800)`, only if the preconditions pass —
  the order of the short-circuited tests matters: persistence, idle > 600, then the draw),
  burning in sunlight (one `next_float` on every tick where light > 0.5), cramming
  (`next_int(4)` when the list exceeds the threshold), all the goals (wander, look, target,
  path recomputation, panic, random positions: **10 full draws** at each
  search), initial offset of the sensors (at brain creation), heart particles
  (§ 3.11), taming, variants, equipment, dropped items.
- **Generation generator**: spawns at chunk generation (seed derived from the
  chunk and the world seed; to be specified with generation).
- **The wandering trader's own generator** (a generator dedicated to the special spawn).
- **Server-side visual effects**: many "particle" or "sound" calls draw
  random numbers on the server even when the effect is not displayed there (particle positions,
  sound pitch). They must be **kept** as draws, even if Rust does nothing with them.

A forgotten or added draw shifts all of the entity's subsequent behaviours; the
non-regression tests must compare traces of draws (caller name, value) between our
server and an instrumented reference model.

### 5.4 Internal orders to reproduce

- **Goals**: order added in each selector (ordered table), targets before goals (§ 3.2).
- **Brains**: increasing priorities; at equal priority, iteration order of a hash
  table indexed by activity whose hash is that of the activity's **name** (Java string
  hash), with the bucket distribution of Java's hash table (index =
  (h ^ (h >>> 16)) & (capacity − 1), initial capacity 16, doubled beyond 0.75); then
  order added. Sensors: order added. Memories: table with no significant order.
- **Entity queries by box**: the entity sections (16³) are walked by increasing section
  X, then in the order of the compact (sorted) section key for Y and Z;
  within a section, insertion order; the box is enlarged by 2 blocks in X/Z and 4 downwards
  for the choice of sections. The dragon's parts are added at the end. "Nearest"
  selections keep the **first** on a tie (except "follow a parent", which
  keeps the last: "not greater" comparison).
- **Players**: the world's player list in arrival order (nearest on a tie:
  the first).
- **A\***: order of neighbours (§ 3.5), binary heap with strict comparisons, update of the
  walked distance even without improvement.

### 5.5 Arithmetic

- Angles: `sin`/`cos` by a 65536-value table (`movement.md`, header); the game's **approximate
  `atan2`** (used by the move and look controls, the body, the
  directions): symmetries (|y|, |x|, swap if |y| > |x|), normalisation by the fast inverse
  square root (magic constant 0x5FE6EB50C7B537AA, one Newton iteration), table of
  257 arcsines/cosines (asin(i/256)) indexed by the low 32 bits of the representation of
  (bias + y) with bias = 2^44 (bits 0x42B0000000000000), cubic correction ((6 + s²) × s / 6), then inverse symmetries.
  To be ported as is: it differs from the exact `atan2` by a few 1e-9 rad.
- Respect the game's `f32`/`f64` conversions: speeds, frictions, rotations in `f32`;
  positions and distances in `f64`; path nodes in `f32`; thresholds written in `f32` then
  promoted (2.5000003E-7, 0.6000000238…).
- `floor` on `f64` and on `f32`, truncated integer divisions (category caps,
  reduced delays = ceiling of n/2).

### 5.6 Type tags and attributes

- The game's entity type tags (fall, underwater breathing, undead,
  arthropods, aquatic fauna, pufferfish, underwater descent, wind charge, rider
  that does not steer, mounted floating, age lock, powder snow, freezing, skeletons,
  pillagers, sun, poison) are frozen into species predicates (`Kind::…`). A test compares
  each one, species by species, with the jar's tag: any added species must be filed there.
- Enchantments filtered by type in melee (mob, player, spear): smite, bane of arthropods,
  impaling (trident also held in hand); after a direct hit, bane of arthropods applies slowness IV
  of 1.5 s to 1.5 s + 0.5 s per level (three draws on the victim's random generator) and fire aspect
  ignites the target.
- Registered attributes: base and **permanent** modifiers (zombies' spawn bonus,
  shulker's "covered" armour, `/attribute`, reread from NBT); transient modifiers
  (effects, equipment, states) are not; unmodelled attributes are
  copied as is. An age threshold (goat's, hoglin's damage) only applies to the
  baby/adult switch, never every tick nor when loading an adult.
- Freezing: +1 per tick in powder snow up to 140, −2 otherwise; slowdown
  −0.05 × frozen share above a block; 1 freezing damage every 40 ticks once frozen
  (×5 for the strider, blaze, magma cube); leather armour and species of the tag
  immune; counter in data 7 and `TicksFrozen`.
- Powder snow: low shape (0.9) for an entity that has fallen more than 2.5 blocks, full
  block for a species that walks on it (or wearing leather boots) whose bottom is above.
- Bell rung: census of entities within 48 blocks (at most every 60 ticks); 5 ticks
  later, a pillager within 32 blocks makes it resonate; 40 ticks later, the pillagers within 48 blocks
  glow for 60 ticks. The villagers' "bell heard" memory waits for the "hide" activity
  (shelter, exit), not simulated.

---

## Uncertain points

### General (§ 1, 2, 3, 5)

- **Local difficulty** (inhabited time, moon phase, "special" factor): used everywhere
  (equipment, phantoms, patrols, reinforcements) but not specified here; to be written with
  difficulty and combat (damage taken, armour, knockback, 10-tick invulnerability).
- **Block property "allows spawning"** per type (refused blocks: glass, leaves,
  magma…): read in the game case by case; to be extracted into `rc-data` rather than copied.
- **Generator used to create the brain** (initial offset of the sensors): assumed
  to be the entity's random generator, not verified.
- Exact order of activities of the same priority: depends on Java's hash table
  (§ 5.4); to be validated on a real brain (villager) by trace.
- **Generation random generator** for spawns at generation: derived seed not read here.
- **Entity sections** and active/asleep transition (which sets the order of the list of ticking
  entities): described in broad strokes; to be specified with the integrated server's
  chunk management.
- "Always active" mobs and the entity tick zone (simulation distance): to be
  revisited with chunk tickets.
- Fall damage per block (multiplier of the hay bale, bed, powder
  snow…) and bounces: listed per block in `block-entities.md` or to be extracted.
- The § 4 values come from four parallel readings; cross-checking against a game
  trace remains to be done, especially for the brains (villager, piglin, warden).


### Part 4.1 Surface monsters and illagers

- Difficulty constants: the "special multiplier" and "effective difficulty" come from the local difficulty instance (chunk's inhabited time, moon phase) — formula not reread here.
- The reduced tick delay / the adjusted tick delay: the "~1/N per tick" probabilities depend on the goal evaluation frequency (goals re-evaluated one tick in two except the every-tick update test); the values given are the code's N, not the measured real frequency.
- Baby zombies and sun: no exception found in the generic rule (they burn too), not verified in game.
- Vindicator "Johnny": the filter only keeps "attackable"; the exclusion of allies (other illagers/team) depends on the generic targeting rules, not reread.
- Endermite / ender pearl, witch / lightning on villager, phantom spawn (insomnia), full raid mechanics, loot table (gunpowder, heads, discs, totem): outside the files read.
- Bat: push overrides present, exact behaviour (without push?) not read.
- Zombie horse: "always despawnable" but becomes persistent on any interaction; behaviour once tamed by another means not verified. Baby: own dimensions exist; no age (does not grow up).
- Witch drinking duration: taken from the potion item (32 ticks in the standard game), not reread.
- Crossbow charge duration (25 ticks without quick charge): item value, not reread.
- Parched: neither the spawn rule nor a possible biome bonus was reread (only attributes/arrows).

### Part 4.2 Nether, End and bosses

- Dragon: the phase actually set when a fight starts (the manager starts in "hover"; the End fight code, not read here, probably replaces it with circular flight). Circular flight tests "crystals ≥ 0" (always true with a fight) whereas strafing and takeoff test "> 0": to be reproduced as is, the exact effect on the ring choice without crystals remains to be checked in game. Graph A* algorithm (costs, decoded adjacency masks) not detailed: copy the table of the 24 masks in our own terms during implementation.
- Hoglin/zoglin: the launch deviation angle (−10..10) is passed to a rotation that expects radians — official behaviour, but to be confirmed whether to keep it as is.
- Sulfur cube: semantics of the new attributes `bounciness`, `friction_modifier`, `air_drag_modifier`, `explosion_knockback_resistance` (read elsewhere in the physics) not studied; biomes and natural spawn rules not read (placement rule: always true on the ground). Its declared category is `monster` although it is peaceful: to be confirmed on the spawn cap side.
- Piglin: spear behaviours (approach/attack/retreat, parameters 10 and 2) and generic crossbow shooting not detailed; quantities of the bartering table (number functions) not extracted; conditions for "huntable" for bastion hoglins (`CannotBeHunted`/`CannotHunt` set by structure generation, not read).
- Generic brain melee (attack range, cooldown) and generic "retaliate / alert" goal: only the parameters passed are noted, the common logic is in another part of the table.
- Warden: the full cycle of the sculk shrieker and the exact summoning condition were only skimmed through the warning tracking.
- Wither: the visibility of the boss bar and the phase 2 visual armour are client-side; summoning (block pattern) is in the skull block, not read in detail.
- Sensor analyses: default cadence of 20 ticks confirmed for the piglin/hoglin/warden sensors (no specific cadence).

### Part 4.3 Land and flying animals, golems

- Exact quantities of the shearing, laying, cat gift, scenting, scute and sneeze loot: defined in the loot tables (data), not read here. Same for froglight colour by variant.
- Horse: priority 3 temptation comes from a separate addition ("behaviour" base); exact evaluation order at equal priority 1 (mount panic / run in circles) not verified in game.
- Wolf: precise triggering conditions for anger against a player outside retaliation (universal anger, pack alert) — follow the common "neutral" mechanic (neutral monsters part), not reread here.
- Rabbit: no reading of the detailed jump control (speed applied in flight, rotation) beyond the figures cited; killer: spawn only by data/command (no natural path found, non-exhaustive).
- Cat: relative weights between the 10 "all conditions" variants assumed equal (selector without priority); check the variant selector rule (common part).
- Fox: exact vector of the pounce (power, angle) and "head stuck" rule (snow block on landing) not quantified.
- Panda: durations of rolling, lying on the back and sitting (partly random) not all recorded; the priority 7 "sit" goal combines picking up/eating an item on the ground.
- Polar bear: exact spawn conditions outside the alternate block = common animal rules.
- Bee: exact anger duration when the hive is harvested/broken (same 20..39 s draw assumed); hive "emergency exit" behaviour.
- Iron golem: summoning conditions by villagers (villager part). Snow golem: exact damage due to water (common "water-sensitive" rule: 1 damage per wet tick, to be confirmed).
- Camel / camel_husk: tamed state of the camel_husk at spawn and exact saddle rules not reread; exact selector of the husk's own attributes (same provider as the camel confirmed).
- Happy ghast: birth rule (dried ghast block) and growth out of scope; exact behaviour of the adult "float" goal.
- Copper golem: way of placing the antenna item (interaction) not reread; oxidation does not alter speed according to the code read (to be confirmed).
- All "spawn categories" are those of the entity type; the spawn rules per biome (weights, group sizes) are in the biome data, not read.

### Part 4.4 Aquatic mobs, villagers and trader

- Villagers'/traders' air underwater: standard for living entities (not verified here). Zombie_nautilus air: depends on zombie_nautilus belonging to the tag `undead` (included via #undead in `can_breathe_under_water`) — tag `undead` not opened.
- Dolphin air handling: I read that its own air function is empty and that air follows the generic living-entity rule (loss underwater, refill to max out of water); the generic rule itself (loss -1/tick, Respiration enchantment) was not reread in this pass.
- Tadpole: it inherits both the "fish" goals (panic, flee the player, swim) AND a brain; verified on rereading: its fish goals (panic 1.25, flee the player, swim) are added on top of the brain; both systems run together.
- Low-level behaviours not reread in detail: swim (0.8 jump probability), doors (open/close), look at the target, go to the target, interaction with villager/cat, "look at and interact with player", follow temptation, animal panic, generic animal mating, brain random walk/random swim, liquid search. The parameters cited come from the calls, not from the bodies of these behaviours.
- Constant "max 10 gossip subjects": declared in the villager but I did not see where it applies (the transfer uses 10 draws).
- For the nautilus: exact meaning of the last two parameters of "follow temptation" (2.5 baby / 3.5 adult) interpreted as stop distance.
- Villager equipment at spawn: none (the "show trades" behaviour and bone meal temporarily put an item in hand).
- Gift table, contents of the `trade_set` and loot tables: data, not detailed here.
- Raid: the conditions "raid active/won/lost/between waves" depend on the raid system (other part). Details per species: § 4.

## Spawner blocks and simulation ranges (W9, `rc-sim/src/mobs/trial.rs`)

### Ranges (chunk tickets)
- Simulation distance `S` = the client's `simulationDistance` setting (at least 2), announced in
  `login` then by `set_simulation_distance` when the setting changes. Players' "simulation" ticket
  at level 31 − S, +1 per chunk (Chebyshev).
- Ticking mobs: chunk within min(view, S) of a player (the view's loading ticket
  puts the chunk in "ticking entities") or force-loaded chunk. Random ticks and thunderstorm:
  S. Scheduled ticks, block events, block entities: S + 1 (loaded neighbours).
- Spawn caps: only the mobs of chunks with loaded entities, within view
  + 2 of a player count (the game unloads the others; we keep them in memory without counting them).
  Number of chunks for the cap: those within 8 at most of a player, loaded or not (289 only).

### Monster spawner
- Block entity ticker (order added), after the entities. Living player (creative included) within
  `RequiredPlayerRange`; type's spawn rules with reason "generator": support
  block and sky not required, darkness required; own rules (`custom_spawn_rules`) otherwise.
- Spawn egg on a spawner: the next spawn data's id changes, no mob is
  born; rule `spawner_blocks_work` false: message `advMode.notEnabled.spawner`.

### Trial spawner
- Normal and ominous settings: id of the `trial_spawner` registry (data pack) or
  inline setting; defaults: range 4, 6 in total, 2 at a time, +2 / +1 per extra player,
  40 ticks between two, "consumable" and "key" loot at equal weights.
- States (property `trial_spawner_state`): inactive → waiting as soon as a spawn data has
  a known id; waiting → active when a player is detected; active: spawns up to the
  total, then waiting for reward when all its mobs are dead (or lost: removed,
  other dimension, more than 47 blocks away); shutter open 40 ticks later; one loot ejection per
  registered player every 30 ticks; then cooldown (36,000 ticks by default); a player detected
  during the cooldown restarts the active state.
- Detection: once every 20 ticks according to position; non-creative and non-spectator players within
  range (14, whole cells), clear line of sight (visual shapes) for the
  first; new registrant: next spawn at least 40 ticks later, event 3013
  (3019 ominous) with the number of registrants.
- Omen: a player seen with Trial Omen, or failing that Bad Omen (turned into
  Trial Omen of 18,000 ticks per level), makes the spawner ominous (events 3020),
  removes its mobs (guaranteed-drop items dropped), pushes back spawns and throwers.
- Spawn: position drawn within range (or `Pos`), free space, clear line of sight from the
  centre, the type's rules (light ignored, support block not required), own rules, finalisation
  if only the id is given, persistent mob, equipment table; flames 3011 and
  3012 (soul if ominous), event `entity_place`.
- Ominous: every 160 ticks, an ominous item spawner above a registrant or a
  mob (coin toss), item drawn from a fixed-seed table per 30 × 20 × 30 zone;
  after 60 to 120 ticks it drops the item or shoots it downwards (arrows, potions, charges).
- Data sent to the client: next spawn (active state) and spawn data.

### Vault
- Active when a non-spectator unrewarded player is within 4 blocks (inactive beyond 4.5),
  re-evaluated every 20 ticks; displayed item drawn from the table every 20 ticks.
- Key (same item and components, sufficient count) on an active vault: loot drawn (luck,
  tool = key), key consumed (not in creative), player rewarded (128 at most), unlocking
  (sound, 14 ticks), then ejection of one item every 20 ticks (event 3017, sound pitch
  according to progress). Wrong key or already rewarded player: refusal sound (15-tick gap).
- Differences: ominous thrower not saved (life of 6 s at most); chunks never unloaded
  (only the cap counting takes it into account); per-player local caps not done.
