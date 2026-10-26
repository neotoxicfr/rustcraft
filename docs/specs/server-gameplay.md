# Server-side game logic (26.3) — Rust singleplayer

Source: a reading of the decompiled server and of the jar's data (`data/minecraft/…`), kept outside
the repository, restated in our own words. Frame: `docs/architecture.md` § 14 (the integrated server
re-implements the game logic). This document describes what the SERVER decides; what the client sees
and sends is in `interactions.md` (clicks, mining, use), `inventories.md` (windows, click packets),
`item-components.md` and `items.md` (components and stacks), `hud.md` / `hud-advanced.md`
(hearts, hunger, experience bar), `commands.md` (client-side tree, suggestions).

Conventions: 1 tick = 1/20 s. "HP" = hit points (1 heart = 2 HP). The game's floats are
single precision unless stated otherwise (positions, velocities: double). Identifiers without a
namespace are in `minecraft:`. "Rule `x`" = game rule (`gamerule`).
Difficulty: `peaceful`, `easy`, `normal`, `hard`.

The data cited (`damage_type/*.json`, `enchantment/*.json`, `loot_table/**`, `recipe/*.json`,
`tags/**`) is read at run time from the player's jar (architecture § 14); the spec describes
how to interpret it, not its exhaustive content.

---

## 1. Player on the server side

### 1.1 Health

- Health = float in `[0, max HP]`, max HP = attribute `max_health` (base 20). Every write
  is clamped to this interval. Dead when health ≤ 0.
- Healing: only acts if health > 0 (a dead player is not resurrected); adds then clamps.
- Absorption: a separate reserve (attribute `max_absorption` as the cap), consumed BEFORE health
  (§ 2.3 step 9). Given by the `absorption` effect (§ 3.6).
- The player "is hurt" if `0 < health < max HP`.

#### Natural regeneration (rule `natural_health_regeneration`, true by default)

Run on every player tick, inside the hunger update (§ 1.2), in this exclusive order
(a single case per tick):

1. **Saturated**: saturation > 0, hurt, hunger = 20 → counter +1; at 10: heal
   `min(saturation, 6) / 6` HP, exhaustion += `min(saturation, 6)`, counter = 0.
   (So 1 HP every 10 ticks as long as saturation ≥ 6.)
2. **Fed**: hunger ≥ 18, hurt → counter +1; at 80: heal 1 HP, exhaustion += 6, counter 0.
3. **Starving**: hunger ≤ 0 → counter +1; at 80: if health > 10, or difficulty `hard`, or
   (health > 1 and difficulty `normal`) → `starve` damage of 1; counter 0. (On `easy` you
   go down to 10 HP, on `normal` to 1, on `hard` all the way to death; on `peaceful` hunger
   does not drop, see below.)
4. Otherwise counter = 0.

**Peaceful** (rule `natural_health_regeneration` true): additionally, before everything else in the
player's movement tick: every 20 ticks (`age % 20 == 0`) heal 1 HP if hurt and saturation
+1 (if < 20, not capped by hunger); every 10 ticks hunger +1 if < 20.

### 1.2 Hunger

State: hunger `0..20` (starts at 20), float saturation (starts at 5), float exhaustion (starts at 0,
capped at 40), tick counter. Save: `foodLevel`, `foodSaturationLevel`,
`foodExhaustionLevel`, `foodTickTimer`.

On every tick, BEFORE regeneration (§ 1.1):
- if exhaustion > 4: exhaustion −= 4; then if saturation > 0 → saturation = max(sat − 1, 0),
  otherwise if difficulty ≠ `peaceful` → hunger = max(hunger − 1, 0).

Eating: hunger = clamp(hunger + nutrition, 0, 20), THEN saturation = clamp(sat + given_saturation,
0, hunger) (saturation never exceeds hunger). The `food` component carries `nutrition` and
`saturation` directly (already multiplied). The old "modifier" form equals
`nutrition × modifier × 2`.

Can eat if: `invulnerable` ability (creative/spectator), or `can_always_eat` of the
component, or hunger < 20. Sprint allowed if hunger > 6 or if the player can fly.

Any exhaustion spending is ignored if the player has the `invulnerable` ability.

#### Exhaustion costs

| Action | Exhaustion |
|---|---|
| Jump | 0.05; sprint jump 0.2 |
| Block broken (on harvest, any block) | 0.005 |
| Attack that hits (normal hit or thrust) | 0.1 |
| Damage taken (after armour, if > 0) | `exhaustion` value of the damage type (§ 2.1) |
| Swimming (swim pose), walking underwater (eyes in water), walking in water | 0.01 per metre (3D distance rounded to the cm; horizontal for "in water") |
| Sprinting on the ground | 0.1 per horizontal metre |
| Walking, sneaking on the ground, climbing, gliding, riding | 0 |
| Regeneration (§ 1.1) | 6 per HP (fed); `min(sat, 6)` per saturated heal |
| `hunger` effect | 0.005 × (amplifier + 1) per tick |
| `apply_exhaustion` enchantment effect | computed value (§ 3.5) |

Distance: each tick, the player's movement (when not a passenger) is converted into whole
centimetres `round(√(…) × 100)` then `exhaustion += cost × cm × 0.01`. Order of the cases: swimming, eyes
underwater, in water, climbing, on the ground (sprint / sneak / walk), gliding, other.

### 1.3 Experience

State: level (integer ≥ 0), progress `[0, 1)` within the level, total (integer, statistic,
used for the score). Save: `XpLevel`, `XpP`, `XpTotal`, `XpSeed` (enchantment seed, § 4.9).

Points to go from level `n` to `n+1`:
- `n < 15`: `2n + 7`
- `15 ≤ n < 30`: `5(n − 15) + 37` (= `5n − 38`)
- `n ≥ 30`: `9(n − 30) + 112` (= `9n − 158`)

Cumulative total to reach level `L` (useful for `/xp` and display):
`L ≤ 16`: `L² + 6L`; `17 ≤ L ≤ 31`: `2.5L² − 40.5L + 360`; `L ≥ 32`: `4.5L² − 162.5L + 2220`.

Gaining `p` points (may be negative): score += p; progress += p / need(level);
total = clamp(total + p, 0, 2³¹−1); while progress < 0: `rest = progress ×
need(current level)`, then if level > 0 → level −1 and progress = 1 + rest /
need(new level), otherwise level −1 (→ reset to 0) and progress 0; while
progress ≥ 1: `(progress − 1) × need(old)` points remaining, level +1, progress =
remaining / need(new). The conversions go through single-precision floats.

Gaining levels: saturating addition; if the result < 0 → level 0, progress 0, total 0.
Sound `entity.player.levelup` if gain > 0, level a multiple of 5, and the last sound was more than
100 ticks ago; volume `0.75 × min(level/30, 1)`.

Enchanting spend (§ 4.9): level −= cost (floor of 0 which also resets progress and
total to 0), then a new random enchantment seed. The anvil, for its part, removes levels
through negative level gain.

#### Experience orbs

- **Splitting** a reward `N`: while `N > 0`, take the largest value of the
  list `2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3, 1` that is ≤ N, subtract it, create an orb
  (or merge, below).
- **Merge on creation**: look in a centred 1×1×1 cube for a non-removed orb of the SAME
  value with `(entity_id − r) % 40 == 0`, `r` random in `[0, 40)`; if found, its
  count +1 and its age reset to 0. Merge in flight: orbs of the same value and the same
  id class (mod 40) in the box inflated by 0.5 add up (counts summed,
  age = min).
- **Lifetime**: 6000 ticks.
- **Attraction**: towards the nearest non-spectator player within ≤ 8 blocks (target kept as long as
  it stays within ≤ 8 blocks); velocity += `dir × (1 − d/8)² × 0.1`, target = mid eye height.
- **Pickup** (contact, player box inflated by 1 / 0.5 / 1, living non-spectator player):
  if the player's pickup delay is 0 → delay = 2 ticks, then repair (§ 3.1, `mending`)
  with the value, the rest as experience points; orb count −1, removed at 0. So
  at most one orb every 2 ticks.
- Death of a mob: orbs if (always-dropping, e.g. player) or (hit by a player within
  the last 100 ticks AND the mob drops experience AND rule `mob_drops`).
  Reward of the dead player: `min(7 × level, 100)` if `keepInventory` is false and not
  spectator, otherwise 0.

### 1.4 Death

Order on the server for a player:
1. Combat-death packet to the player (death message, or empty if rule `show_death_messages`
   is false); message broadcast to everyone according to team visibility (`always` /
   `hideForOtherTeams` / `hideForOwnTeam` / `never`).
2. Rule `forgive_dead_players`: neutral mobs within 32 × 10 × 32 that were targeting the player
   forget their anger.
3. If not spectator: death loot (§ 5) then equipment:
   - rule `keep_inventory` false: items carrying the `prevent_equipment_drop` enchantment
     effect (curse of vanishing) removed, then the whole inventory
     dropped (items scattered around the player);
   - then experience orbs (§ 1.3).
4. Score "deathCount" +1, statistics, wither rose if killed by a wither (placed if rule
   `mob_griefing` and the spot is free, otherwise dropped).
5. Entity event 3 (death animation), fire extinguished, freeze reset to 0, last-death
   position remembered.

Totem (component `death_protection`): checked before death, unless the damage type is
in `bypasses_invulnerability`. Main hand then off hand: the first carrying item is
consumed (−1), health = 1, the component's effects applied (§ 3.4), entity event 35.

### 1.5 Respawn

The client sends "respawn" (rule `immediate_respawn`: no death screen). The server
recreates the player entity (new id) and chooses the place:

1. **Remembered point** (`respawn`: dimension, position, yaw, pitch, `forced`):
   - **Respawn anchor** if (forced or charge > 0) and the anchor works in that
     dimension (`respawn_anchor_works`): standing position found around the anchor; if not
     forced and the position exists, charge −1.
   - **Bed** if the dimension's bed rule allows setting the point there: standing position
     next to the bed (according to its orientation).
   - Other block and `forced` (point set by command): the block and the one above must be
     passable → `(x+0.5, y+0.1, z+0.5)` with the remembered yaw/pitch.
   - Otherwise failure: message "missing or obstructed bed" and fallback to the default point.
   - Orientation: facing the spawn block (the yaw is computed from the standing position
     to the centre of the block).
2. **Default point**: the server's respawn dimension (overworld), the world's point
   (`spawn`: position, yaw, pitch). A free spot is searched around the point in a
   square of radius `respawn_radius` (rule, default 10; brought down to the distance to the border,
   minimum 1): candidates visited in a pseudo-random order (random start, step coprime
   with the number of candidates: 17, or n−1 if ≤ 16), at most 1024. A candidate
   is valid if the column has a floor (`motion_blocking` height, not under water) and the
   player's box fits there with no collision or liquid. Otherwise the point itself, moved up then down
   until it fits. In global adventure game mode: directly the point adjusted in height.

What is carried over to the new entity:
- always: game mode (current and previous), base values of the attributes, enchantment
  seed, Ender chest, recipe book, skin settings, last-death
  position, credits seen;
- death with `keep_inventory` true (or spectator): inventory, level, total and progress
  of experience; health = max HP; hunger and effects NOT kept (hunger 20, saturation 5);
- return from the End (not a death): everything (health, hunger, effects, inventory, experience,
  permanent modifiers).

### 1.6 Game modes

| Mode | `mayfly` | `instabuild` | `invulnerable` | forced `flying` | can build |
|---|---|---|---|---|---|
| survival (0) | no | no | no | no (cut) | yes |
| creative (1) | yes | yes | yes | — | yes |
| adventure (2) | no | no | no | no | no (except `can_place_on`/`can_break`) |
| spectator (3) | yes | no | yes | yes | no |

- Change: "change mode" game-event packet (value = id), then abilities
  sent back. To spectator: shoulders emptied, dismount, item use stopped,
  location-based enchantment effects stopped. From spectator: camera put back on the player.
- `invulnerable`: ignores all damage except `bypasses_invulnerability` types (void, `/kill`); no
  exhaustion.
- Creative: block interaction reach +0.5 and entities +2 (transient attribute
  modifiers applied every tick). Spectator: no collision, no pickup, no
  death loot.
- New player: the world's default mode (forced if `force_gamemode`).

---

## 2. Damage and combat

### 2.1 Damage types (`damage_type`)

Data-driven registry: one file per type, fields `message_id` (key of the death message
`death.attack.<id>`), `exhaustion` (exhaustion inflicted on the hit player), `scaling`
(`never`, `always`, `when_caused_by_living_non_player`), `effects` (sound/reaction: `hurt`
default, `thorns`, `drowning`, `burning`, `poking`, `freezing`), `death_message_type`
(`default`, `fall_variants`, `intentional_game_design`). A "damage source" = type +
direct entity (the arrow) + causing entity (the shooter) + optional source position.

**Difficulty scaling** (hit player only) if `scaling` = `always`, or
`when_caused_by_living_non_player` and the cause is a living non-player mob:
`peaceful` → 0 (the attack fails), `easy` → `min(d/2 + 1, d)`, `normal` → `d`, `hard` → `1.5 d`.

Tags (`tags/damage_type/*.json`) that drive the computation (26.3 content, extensible by packs):

| Tag | Effect | 26.3 members |
|---|---|---|
| `bypasses_armor` | skips armour (and armour wear) | cramming, dragon_breath, drown, ender_pearl, fall, fly_into_wall, freeze, generic, generic_kill, in_wall, indirect_magic, magic, on_fire, out_of_world, outside_border, sonic_boom, stalagmite, starve, wither |
| `bypasses_shield` | the shield does not block (via its `bypassed_by`) | cactus, campfire, cramming, dragon_breath, drown, dry_out, ender_pearl, fall, falling_anvil, falling_stalactite, fly_into_wall, freeze, generic, generic_kill, hot_floor, in_fire, in_wall, indirect_magic, lava, lightning_bolt, magic, on_fire, out_of_world, outside_border, sonic_boom, stalagmite, starve, sulfur_cube_hot, sweet_berry_bush, wither |
| `bypasses_invulnerability` | ignores invulnerability (creative, temporary) and totem | generic_kill, out_of_world |
| `bypasses_resistance` | ignores the resistance effect | generic_kill, out_of_world |
| `bypasses_effects` | ignores resistance AND enchantment protections | starve |
| `bypasses_enchantments` | ignores enchantment protections | sonic_boom |
| `bypasses_cooldown` | ignores the invulnerability window (§ 2.3) | (empty) |
| `is_fire` | cancelled by `fire_resistance`; immunity of fireproof mobs; rule `fire_damage` | campfire, fireball, hot_floor, in_fire, lava, on_fire, sulfur_cube_hot, unattributed_fireball |
| `is_fall` | rule `fall_damage`; mobs that are `fall_damage_immune` | ender_pearl, fall, stalagmite |
| `is_drowning` / `is_freezing` | rules `drowning_damage` / `freeze_damage`; freeze ×5 on `freeze_hurts_extra_types` | drown / freeze |
| `is_projectile` | blocking does not push the attacker back | arrow, fireball, mob_projectile, thrown, trident, unattributed_fireball, wind_charge, wither_skull |
| `is_explosion` | | bad_respawn_point, explosion, fireworks, player_explosion |
| `damages_helmet` | helmet damaged, damage × 0.75 if a helmet is worn | falling_anvil, falling_block, falling_stalactite |
| `no_knockback` | no knockback by default | (most environmental ones, explosions, spear) |
| `no_impact` | no "hurt" marking (no velocity refresh) | drown |
| `no_anger` | the victim does not target the attacker | mob_attack_no_aggro |
| `is_player_attack` | | mace_smash, player_attack, spear |

Exhaustion values: 0.1 for attacks and "hard" environmental types (arrow, cactus,
explosion, lava, in_fire, mob_attack, player_attack, thorns, …), 0 for fall, drown, magic,
starve, wither, in_wall, freeze, generic, out_of_world, etc. (read the file).

### 2.2 Preliminary immunities

An entity ignores the source (no effect) if: removed; invulnerable (permanent, or
temporary = the entity's invulnerability counter > 0) except type `bypasses_invulnerability`
or attacker in creative; `is_fire` and fireproof; `is_fall` and entity type
`fall_damage_immune`; immunity through the `damage_immunity` enchantment effect (§ 3.5).
Player additionally: damage rules (`drowning_damage`, `fall_damage`, `fire_damage`,
`freeze_damage`); while changing dimension (except `ender_pearl`); as long as the client has not
signalled "world loaded". `invulnerable` ability: refusal except `bypasses_invulnerability`.
Warden: also refuses while it digs or emerges (except `bypasses_invulnerability`).

**Command `/damage <target> <amount ≥ 0> [<type> [at <position> | by <entity> [from <cause>]]]`**
(default type `generic`): `at` gives a position alone; `by X` makes X both the direct entity
AND the cause; `by X from Y`: direct X, cause Y. The source position is `at`, otherwise
that of the direct entity (knockback direction). An entity that cannot be found is an argument error.
If the damage does not go through (immunity, invulnerability window, amount not greater than
the last hit), message "invulnerable". Item on the ground: 5 hit points (`Health`), refused if
the cause is a mob and `mob_griefing` is false, or if the stack resists the type
(component `damage_resistant`); at 0, it disappears and spills the contents of its
`container` or `bundle_contents` component. Experience orb: 5 hit points, removed at 0.

### 2.3 Full computation (living mob hit)

Input: source, amount `d`.
1. Immunities (§ 2.2); dead or dying → fail. Player: `invulnerable` → fail;
   shoulders emptied; difficulty scaling (§ 2.1), 0 → fail.
2. `is_fire` and effect `fire_resistance` → fail. Wake up if asleep. `d < 0` → 0.
3. **Blocking** (§ 2.6): `blocked = result`; `d −= blocked`.
4. `is_freezing` on `freeze_hurts_extra_types` → `d × 5`.
5. `damages_helmet` and helmet worn → helmet wear (`max(1, d/4)` truncated), `d × 0.75`.
6. NaN / infinity → largest float.
7. **Invulnerability window** (counter `cooldown`, except type `bypasses_cooldown`):
   - if `cooldown > 10`: if `d ≤ last_hit` → fail (nothing); otherwise only the
     excess `d − last_hit` is applied (steps 8-10), `last_hit = d`, and this hit is NOT
     "full" (no knockback, no hurt sound, no animation);
   - otherwise: `last_hit = d`, `cooldown = 20`, apply the whole `d`, `hurt_time =
     hurt_duration = 10` (full hit).
   The counter decreases by 1 per tick (for the player, in its server tick). Effect: a full
   hit opens 10 ticks during which only a stronger hit goes through, and only for the
   difference.
8. **Armour** (except `bypasses_armor`): wear on each armour piece (`max(1, d/4)` truncated,
   `equippable` pieces with `damage_on_hurt` that the source can damage), then:
   `t = 2 + toughness/4`; `a = clamp(armor − d/t, 0.2 × armor, 20)`; `f = a/25`;
   if the source's weapon has `armor_effectiveness` effects (Breach), `f = clamp(modified f,
   0, 1)`; `d ×= 1 − f`. `armor` = attribute `armor` rounded down to the integer,
   `toughness` = attribute `armor_toughness`.
9. **Magic** (except `bypasses_effects`):
   - effect `resistance` (except `bypasses_resistance`): `d = max(d × (25 − 5(amp+1)) / 25, 0)`
     (amp ≥ 4 → total immunity);
   - if `d ≤ 0` → 0; except `bypasses_enchantments`: `p` = sum of the
     `damage_protection` effects of all worn equipment (§ 3.5); if `p > 0`:
     `d ×= 1 − clamp(p, 0, 20)/25` (so 80 % max).
10. **Absorption**: `d' = max(d − absorption, 0)`; absorption −= `d − d'`. If `d' ≠ 0`:
    player → exhaustion += `exhaustion` of the type; recorded in the combat tracker;
    health −= `d'`.
11. Culprits: last attacker (if the source is living and not `no_anger`, nor a wind charge
    on `no_anger_from_wind_charge`); last attacking player remembered for 100 ticks (a tamed
    wolf counts for its owner) — used for the `killed_by_player` loot and for experience.
12. Full hit: if blocked → shield block sound, otherwise broadcast of a damage
    event (packet `damage_event`: type, cause, direct, position). Outside `no_impact` and
    except for a total block: velocity marked for synchronisation. Outside `no_knockback` and
    except for a total block: default knockback (§ 2.4).
13. Death? → totem (§ 1.4) otherwise death sound and death; otherwise (full hit) hurt sound
    (+ thorns sound if type `thorns`).
14. Success = not blocked or remainder > 0: last source remembered for 40 ticks (death message),
    status effects notified (e.g. `infested`, `oozing`… react to hits).

### 2.4 Knockback

`knockback(strength, dx, dz)`: `strength ×= 1 − knockback_resistance`; if ≤ 0 nothing. If `dx²+dz² <
1e-5`, draw `dx, dz` at random (`(r1 − r2) × 0.01`) until it gets out. `v = normalize(dx, 0, dz)
× strength`; new velocity = `(vx/2 − v.x, on ground ? min(0.4, vy/2 + strength) : vy, vz/2 − v.z)`.

- **Default** (step 12): strength 0.4; direction = from the victim towards the source position
  (or, for a projectile, the opposite of its horizontal flight direction).
- **Player attack** (extra, § 2.5): strength `k = weapon_knockback/2 + 0.5 if full sprint`,
  where `weapon_knockback` = attribute `attack_knockback` modified by the weapon's `knockback` effects
  (Knockback: +1 per level); direction = the attacker's yaw (`sin(yaw)`, `−cos(yaw)`).
  Non-living: push `(−sin·k, 0.1, cos·k)`. Then the attacker: horizontal velocity × 0.6
  and sprint cut. If the target is a player, its velocity is sent to it immediately.
- **Partial block** by a mob: the attacker receives a knockback of strength 0.5 from the
  defender.

### 2.5 Player melee attack

Reception (`attack`, `interactions.md`): client loaded, not spectator, target existing within the
border, in reach (reach of the item `attack_range`, otherwise attribute
`entity_interaction_range`, margin 3), weapon without `piercing_weapon` (spears go through
the thrust). Forbidden targets (item on the ground, orb, oneself, non-attackable arrow) →
disconnect "invalid entity attacked". Silent refusal if `minimum_attack_charge` is not
reached (tolerance 5 ticks).

**Recharge**: counter `ticker` +1 per tick; reset to 0 by the `punch` packet, by the
successful interaction with a swing, by dropping an item, and when the item in hand changes
type. `delay = 20 / attack_speed` ticks; `s = clamp((ticker + 0.5) / delay, 0, 1)`.

Computation:
1. `base` = attribute `attack_damage` (or the damage of the trident's spin attack);
   source = type carried by the weapon's `damage_type` component, otherwise the item's own type
   (falling mace → `mace_smash`), otherwise `player_attack`.
2. `magic_bonus = s × (enchanted damage(base) − base)` (the weapon's `damage` effects: Sharpness
   +0.5·lvl+0.5, Smite / Bane of Arthropods +2.5·lvl if the target has the right tag, Impaling…).
3. `base ×= 0.2 + 0.8 s²`.
4. Redirectable projectile hit (`redirectable_projectile`) → sent back in the direction of the
   gaze, end.
5. If `base > 0` or `bonus > 0`: `full = s > 0.9`; `sprint_knockback = sprint and full` (sound
   `attack.knockback`).
6. `base += item bonus` (mace: § 2.8).
7. **Critical**: full AND fall in progress (`fall_distance > 0`), not on the ground, not on a
   ladder/vine, not in water, not blinded/restricted mobility, not a passenger, living
   target, NOT sprinting → `base × 1.5`.
8. `total = base + magic_bonus`.
9. **Sweep**: full, not critical, not sprint_knockback, on the ground, known horizontal velocity² <
   (speed × 2.5)², item in hand in `#swords`.
10. Inflict `total`. If it lands: extra knockback (§ 2.4); sweep; sounds/particles
    (critical: sound `attack.crit` + particles; otherwise `attack.strong` or `attack.weak`; magic
    bonus → enchanted particles); weapon effects (item's own reaction, e.g. mace;
    `post_attack` effects on the victim then the attacker; if the item has the `weapon` component:
    wear of `item_damage_per_attack`, item removed from the hand if it breaks); heart
    particles (`damage_indicator`, `(int)(actual damage × 0.5)` if > 2); exhaustion 0.1.
    Otherwise sound `attack.nodamage`.

**Sweep**: sound `attack.sweep`; damage `1 + sweeping_damage_ratio × base` (base of
step 6), then the weapon's `damage` effects × `s`; hits every living entity in the target's box
inflated by (1; 0.25; 1), ≠ attacker and ≠ target, not an ally, not a marker armour stand,
within 3 blocks (distance² < 9) of the attacker; knockback 0.4 along the yaw;
`post_attack` effects. `sweep_attack` particle in front of the attacker.

**Thrust (spear, `piercing_weapon` / `kinetic_weapon`)**: same steps with no critical or
sweep; if the item is not being used, bonus and base are scaled by `s` as
above; knockback 0.4 then weapon knockback; can unseat riders (except
`cannot_be_dismounted_by_item_usage`). Target detection details: `interactions.md`.

### 2.6 Shields (component `blocks_attacks`)

Blocks if the entity has been using the item for at least `block_delay_seconds × 20`
ticks rounded (shield: 0.25 s → 5 ticks) and the type is not in `bypassed_by`
(shield: `#bypasses_shield`). Piercing arrow → no block.

- Angle: if the source has a position, `angle = acos(horizontal_dir_to_source · gaze)`
  (gaze = head yaw, pitch 0), otherwise π.
- Blocked amount = `clamp(Σ reductions, 0, d)`, each reduction (`horizontal_blocking_angle`
  default 90°, optional `type`, `base`, `factor`) is worth 0 if angle > limit or type not listed,
  otherwise `clamp(base + factor × d, 0, d)`. Shield: one 90° reduction, base 0, factor 1
  (total frontal block).
- Wear: `item_damage` (`threshold`, `base`, `factor`): 0 if blocked < threshold, otherwise
  `⌊base + factor × blocked⌋` (shield: threshold 3, base 1, factor 1).
- Melee attack (not `is_projectile`) by a mob: disabling if the attacker's weapon
  has `weapon.disable_blocking_for_seconds` > 0 and it is the active item
  (axes: 5 s) → cooldown of `round(s × disable_cooldown_scale × 20)` ticks on
  the item, use stopped, sound `disabled_sound`. Partial block → knockback on the attacker
  (§ 2.4).

### 2.7 Projectiles

- **Arrows**: base damage 2 (NBT `damage`), modified by the `damage` effects of the firing
  weapon (Power); `damage = ⌈clamp(|velocity| × base, 0, 2³¹−1)⌉`; critical (bow fully drawn,
  crossbow): `+ random in [0, damage/2 + 2)`; flaming → target on fire for 5 s. Source `arrow`
  (cause = shooter, otherwise the arrow). If it hits: stuck-arrow counter +1 (except
  piercing), weapon knockback (Punch: `push(normalize(vx, 0, vz) × k × 0.6 × (1 − resistance),
  y 0.1)`), `post_attack` effects, sound; removed except piercing (pierces `level` targets, at
  most `level+1` hit). Not hit but the target undergoes the contact effects: the target's fire
  restored, arrow sent back the opposite way ("reverse" deflection, factor 0.2);
  almost motionless → dropped as an item (if pickable) and removed.
- **Bow**: power `p = (t² + 2t)/3` with `t = drawn_ticks/20`, capped at 1; < 0.1 →
  nothing; velocity `3p`, inaccuracy 1, critical if `p = 1`.
- **Crossbow**: velocity 3.15 (rocket 1.6), always critical for arrows.
- Thrown **trident**: 8 damage, type `trident`; Channeling, Loyalty, Riptide through its enchantment
  effects.
- **Fireballs, skulls, wind charges, eggs, snowballs**: types `fireball`,
  `wither_skull`, `wind_charge`, `thrown`; their own damage is in the entities spec
  (`entities-objects.md` for rendering; behaviour to be specified with the mobs).
- Firing a projectile: effects `projectile_count` (Multishot: 3), `projectile_spread`,
  `projectile_piercing`, `ammo_use` (Infinity: normal arrow not consumed),
  `projectile_spawned` (Flame: sets the arrow on fire).

### 2.8 Mace

Attack with a falling mace (`fall_distance > 1.5` and not gliding):
- damage bonus (step 6): `h ≤ 3 → 4h`; `h ≤ 8 → 12 + 2(h − 3)`; otherwise `22 + (h − 8)`,
  plus `h × Σ smash_damage_per_fallen_block` (Density +0.5·lvl); source `mace_smash`
  (Breach reduces armour, § 2.3);
- on contact: the attacker's vertical velocity = 0.01, ignores the fall damage of the
  current impulse, velocity sent back to the client; sound `smash_ground(_heavy)` if
  the target is on the ground (heavy if fall > 5) otherwise `smash_air`; level event 2013
  (particles); shockwave: every living entity within ≤ 3.5 blocks of the target (except attacker, target,
  allies, the attacker's tamed animals, markers, flying creative players,
  spectators) receives `push(dir × (3.5 − dist) × 0.7 × (2 if fall > 5) × (1 − resistance),
  0.7, …)`;
- after the hit: fall distance reset to 0 (no fall damage).
  Wind (Wind Burst): `post_attack` effect of the enchantment.

---

## 3. Items on the server side

The components themselves (codecs, default values): `item-components.md`. Here, what the
server does with them.

### 3.1 Wear and repair

- An item is "usable" if it has `max_damage` AND `damage` and not `unbreakable`. Stackable
  only if `max_stack_size > 1` and not worn.
- **Inflicting `n` points of wear**: 0 if not usable or if the holder has infinite materials
  (creative); if `n > 0`, `n` goes through the `item_damage` effects of its enchantments (Unbreaking:
  `remove_binomial`, each point cancelled with probability `p`, armour `p = 2n/(5n+5)`,
  others `p = n/(n+1)`). Then `damage += n'`; if `damage ≥ max_damage` → stack −1, "item broken"
  callback (sound `break_sound`, particles, statistic, removal from the slot,
  for armour reset to empty). "Without breaking" variant (elytra, etc.): capped at
  `max − 1`.
- **Binomial draw** (`remove_binomial`, probability `p`, input `n`): if `n ≤ 128` or
  `n·p < 20` or `n(1−p) < 20` → `n` draws `float < p`; otherwise normal approximation
  `round(⌊n p⌋ + gauss × √(n p (1−p)))` clamped to `[0, n]`. Result `n − removed`.
- **Sources of wear**: tool (`tool.damage_per_block`, default 1) on any block of hardness ≠ 0
  broken by a player; weapon (`weapon.item_damage_per_attack`) on a hit that lands; armour
  (§ 2.3); shield (§ 2.6); enchantment effects `change_item_damage` (Thorns: +2 on the
  piece); items' own uses (flint and steel, shears, rod…).
- **Mending** (`repair_with_xp`): orb picked up → one worn item chosen at random among those
  that have the effect and are damaged (equipment: hands, armour); `capacity = value ×
  factor` (Mending ×2); `repaired = min(capacity, wear)`; leftover XP = `value −
  repaired × value / capacity` (integer), fed back recursively onto another item; what
  remains at the end goes to the player.
- **Anvil** (§ 4.8), **grindstone** (removes non-curse enchantments, returns XP),
  **grid combination** (special recipe `repair_item`: two identical items →
  wear `max − (rest1 + rest2 + max×5/100)` with `max` = the larger of the two, only
  curses are kept, at the higher level of the two; new item with no other
  component).

### 3.2 Mining checked by the server

`player_action` packets (`interactions.md`). For each action: reach (block within
`block_interaction_range` + 1) otherwise ignored (abort if it was the stop); `y > max height`
→ the block's real state is sent back.

- **Start**: spawn-point protection, the allowed-interaction test (border…), otherwise the block is sent
  back. Creative (`instabuild`) → immediate destruction. Adventure mode without a compatible
  `can_break` → refused. Otherwise: the tool's `hit_block` enchantment effects, the
  block's reaction to the click (`attack`: note block, iron door…), `progress = speed / hardness / (30 if good
  tool or block with no requirement, otherwise 100)`; if ≥ 1 → broken right away, otherwise mining
  in progress (progress sent to the others: `block_destruction`, state `⌊progress × 10⌋`).
- **Speed**: the tool's speed (`tool`: the first rule whose
  blocks contain the state and that has a `speed`, otherwise `default_mining_speed`, 1 with no tool);
  if > 1: `+ mining_efficiency` (Efficiency: lvl² + 1); Haste/Conduit Power:
  `× (1 + 0.2 (amp+1))`; Mining Fatigue: `× 0.3^(amp+1)`; `× block_break_speed`; eyes in
  water: `× submerged_mining_speed` (Aqua Affinity raises it to 1); not on the ground: `/ 5`.
  Hardness −1 → unbreakable (progress 0).
- **Good tool**: block without `requires_correct_tool_for_drops`, or a rule of the tool that
  covers it with `correct_for_drops` true (first rules that cover the block).
- **End** (`stop`): if `progress_per_tick × (elapsed ticks + 1) ≥ 0.7` → broken (30 %
  tolerance for latency); otherwise "deferred break": the server keeps accumulating on
  each tick and breaks as soon as ≥ 1 (if the block becomes air in the meantime, cancelled).
- **Cancel**: progress −1 sent.
- **Breaking**: the item in hand must be able to break (sword in creative: `can_destroy_blocks_in_creative`
  false); game master blocks (command, structure…) reserved to operators;
  block reaction before breaking (bed / door → other half, particles,
  piglin anger…); removal; if the player does not block loot (creative): wear of
  the tool, then if good tool: statistics, exhaustion 0.005, the block's loot (§ 5.4) and the block's
  XP (ores, via `block_experience`: Silk Touch → 0).
- Each action is acknowledged by `block_changed_ack` (sequence number) at the end of the tick
  (`interactions.md`).

### 3.3 Use and consumption

- Use refused in spectator, or if the item is on cooldown (`use_cooldown` / cooldown
  group).
- **Consumable** (`consumable`: `consume_seconds` default 1.6, `animation`, `sound`,
  `has_consume_particles`, `on_consume_effects`): can it be consumed? (if `food`: the
  player must be able to eat, § 1.2). Duration > 0 → start of use; otherwise immediate effect. At the
  end of use: particles + sound, then each "listening" component reacts in component
  order: `food` (hunger + saturation, the component's sound then burp `entity.player.burp`),
  `potion_contents` (effects, duration × `potion_duration_scale`), `suspicious_stew_effects`,
  `ominous_bottle_amplifier` (Bad Omen); then `on_consume_effects` (server only);
  drink/eat game event; stack −1 (except infinite materials).
- During use: particles + sound when `used_ticks > ⌊duration × 0.21875⌋` and
  `remaining_ticks % 4 == 0`.
- **After use**: `use_remainder` (if the stack shrank: the remaining item is given — if the
  stack is empty it replaces it, otherwise it goes to the inventory or drops); `use_cooldown`
  (cooldown `seconds × 20` ticks on the item or its group, packet `cooldown`).
- Consumption effects (`on_consume_effects`):
  - `apply_effects` (`effects`, `probability` default 1): if `float < probability`, adds
    each effect;
  - `remove_effects` (list of effects); `clear_all_effects` (milk);
  - `teleport_randomly` (`diameter` default 16): 16 attempts, `x, y, z ± diameter/2` (y clamped to
    the logical height), valid random teleport (floor under the feet, not in
    `consumable_does_not_teleport_to`), dismounts; sound and game event if it succeeds;
    cooldown of 20 ticks (player);
  - `play_sound`.

### 3.4 Death protection (`death_protection`)

Totem: `death_effects` = list of consumption effects (§ 3.3) applied after resetting to
1 HP (vanilla totem: clears all effects, then Regeneration II 45 s, Absorption II 5 s,
Fire Resistance 40 s). See § 1.4.

### 3.5 Data-driven enchantments

Each file `enchantment/<id>.json`:

| Field | Role |
|---|---|
| `description` | displayed name |
| `supported_items` | accepted items (anvil, command `/enchant`) |
| `primary_items` | items offered by the table (default = `supported_items`) |
| `exclusive_set` | incompatible enchantments (compatibility is symmetric: A≠B and neither is in the other's set) |
| `weight` | draw weight (table, loot) |
| `max_level` | 1..255 |
| `min_cost`, `max_cost` | `{base, per_level_above_first}`: cost = `base + per_level × (lvl − 1)` |
| `anvil_cost` | cost per level at the anvil (§ 4.8) |
| `slots` | slot groups where it acts (`any`, `hand`, `mainhand`, `offhand`, `armor`, `feet`, `legs`, `chest`, `head`, `body`, `saddle`) |
| `effects` | effect components (below) |

**Level-dependent values**: a number = constant; `linear`
(`base + per_level_above_first × (n−1)`), `levels_squared` (`n² + added`), `clamped`
(`value`, `min`, `max`), `fraction` (`numerator / denominator`, 0 if the denominator is 0),
`exponent` (`base ^ power`), `lookup` (`values[n−1]` otherwise `fallback`).

**Value effects** (modify a number): `add` (+ value), `multiply` (× `factor`),
`set` (= value), `remove_binomial` (§ 3.1, `chance`), `exponential` (× `base^exponent`),
`all_of` (chains). Applied in the order of the item's enchantments then of the entries of
the list, each only if its `requirements` (a loot condition, § 5.3) pass in the
given context.

**Effect components** (where the server reads them, which condition context):

| Component | When | Context of the `requirements` |
|---|---|---|
| `damage_protection` | damage taken, sum over all equipment (§ 2.3 step 9) | damage: `this` = victim, `damage_source`, attackers, level |
| `damage_immunity` | total immunity (§ 2.2) | damage |
| `damage` | weapon damage (melee, arrow, trident) | damage (`this` = victim) |
| `smash_damage_per_fallen_block` | mace (§ 2.8) | damage |
| `knockback` | weapon knockback (§ 2.4) | damage |
| `armor_effectiveness` | fraction of the victim's armour (Breach) | damage |
| `post_attack` | after a successful hit; fields `enchanted` (bearer of the enchantment: `attacker` / `victim`) and `affected` (target of the effect: `attacker`, `damaging_entity`, `victim`) | damage |
| `post_piercing_attack` | after a thrust | entity |
| `hit_block` | projectile or click on a block | block hit (`this`, position, `block_state`) |
| `item_damage` | wear (§ 3.1) | item (`tool`) |
| `equipment_drops` | chance to drop the equipment of a killed mob (Looting) | damage |
| `location_changed` | on every block change of the bearer; enabled/disabled (Frost Walker, Soul Speed) | location (`enchantment_active`) |
| `tick` | every tick of the bearer | entity |
| `ammo_use`, `projectile_piercing`, `projectile_count`, `projectile_spread`, `projectile_spawned` | firing (§ 2.7) | item / entity |
| `trident_return_acceleration`, `trident_spin_attack_strength`, `trident_sound` | trident | entity / none |
| `fishing_time_reduction`, `fishing_luck_bonus` | fishing | entity |
| `block_experience`, `mob_experience` | block / mob XP | item / entity |
| `repair_with_xp` | mending (§ 3.1) | item |
| `crossbow_charge_time`, `crossbow_charging_sounds` | crossbow (Quick Charge) | none |
| `attributes` | attribute modifiers while the item is in a slot of the group; identifier `<id>.<slot>`, amount = value at the level (Efficiency, Sweeping Edge, Respiration, Depth Strider, Swift Sneak…) | none |
| `prevent_equipment_drop`, `prevent_armor_change` | curses (vanishing, binding) | none |

**Entity effects** (`post_attack`, `tick`, `hit_block`, `projectile_spawned`,
`location_changed` in part): `all_of`, `apply_mob_effect` (effect drawn from a list,
duration and amplifier min/max), `change_item_damage`, `damage_entity` (random damage
between `min_damage` and `max_damage`, given type, cause = bearer), `explode`, `ignite`
(fire `duration` s), `apply_impulse`, `apply_exhaustion`, `play_sound`, `replace_block`,
`replace_disk` (Frost Walker), `run_function`, `set_block_properties`,
`spawn_particles`, `summon_entity` (Channeling: lightning); plus `attribute` for
`location_changed`.

The effective level of an enchantment on an entity = the highest among the items worn
in its slots. Only enchantments whose slot matches act
(the matching slot), except for effects read directly from the item (damage, wear, firing).

26.3 examples: Protection `damage_protection` +lvl (except `bypasses_invulnerability`),
Feather Falling +3·lvl if `is_fall`, Sharpness `damage` +0.5·lvl+0.5, Smite +2.5·lvl on
`sensitive_to_smite`, Fire Aspect `ignite` 4·lvl s (direct attack), Thorns (15 %·lvl)
`damage_entity` 1-5 `thorns` + wear 2, Looting `equipment_drops` +0.01·lvl (player attacker),
Fortune and Looting also act through the loot functions (§ 5).

### 3.6 Status effects

Instance: effect, duration (ticks, −1 = infinite), amplifier (0 = level I), ambient,
visible particles, icon, hidden effect (stack of weaker but longer instances).

- **Adding**: refused if immune (types `immune_to_infested`, `immune_to_oozing`,
  `ignores_poison_and_regen`). New → added, attribute modifiers set, packet
  `update_mob_effect`. Existing → merge:
  - stronger amplifier: if the new one is shorter, the old one is kept as a hidden effect;
    the new one's amplifier and duration are taken;
  - otherwise if the old one is shorter (an infinite duration is never shorter): same
    amplifier → duration of the new one; weaker amplifier → stored as a hidden effect
    (recursive merge);
  - ambient / visible / icon flags taken from the new one.
  - Then "effect started" (Absorption: reserve = `max(current, 4 (amp+1))`).
- **Server tick** (every tick of the entity): counter = duration (or the entity's age if
  infinite); if the effect acts this tick, its action; if the action returns false the effect is
  removed (Absorption when the reserve is empty); duration −1 (and that of the hidden effects); at 0
  with a hidden effect → the hidden effect takes its place (update packet). Removed at duration 0.
- **Rhythm and action**:

| Effect | Acts when | Action |
|---|---|---|
| `regeneration` | `duration % (50 >> amp) == 0` (always if 0) | heal 1 if hurt |
| `poison` | `% (25 >> amp)` | 1 `magic` damage if health > 1 |
| `wither` | `% (40 >> amp)` | 1 `wither` damage |
| `hunger` | every tick | exhaustion `0.005 (amp+1)` |
| `absorption` | every tick | kept as long as reserve > 0 |
| `saturation` (instant) | duration ≥ 1 | eats `amp+1` hunger, modifier 1 (saturation `2(amp+1)`) |
| `instant_health` / `instant_damage` | duration ≥ 1 or direct application | heal `4 << amp` / `magic` damage `6 << amp` (inverted for the undead); per splash potion: `(int)(scale × base + 0.5)`, `indirect_magic` damage if a source exists |

- **Attribute modifiers** (amount × (amp+1)): `speed` +20 % (total multiplied) speed,
  `slowness` −15 %, `haste` +10 % attack speed, `mining_fatigue` −10 %, `strength`
  +3 damage, `weakness` −4, `health_boost` +4 max HP, `absorption` +4 max absorption,
  `jump_boost` +1 safe fall distance, `luck` / `unluck` ±1 luck, `invisibility`
  locator bar mask. The other effects (resistance, fire resistance, water breathing,
  night vision, blindness, levitation, slow falling, dolphin's grace, conduit power…) are read
  by the code concerned (§ 2.3, movement, rendering).
- **Instant** effects in a potion: applied directly (no instance). Effects of a lingering
  potion / cloud: duration × factor (floor 1).

---

## 4. Inventories and menus on the server side

Packets and click logic: `inventories.md` § 1-2. The server runs EXACTLY the same
click algorithm as the one the client predicts (`inventories.md` § 2.2-2.8: pickup,
quick move, swap, clone, throw, drag, collect); it is authoritative.

### 4.1 Validating a click (`container_click`)

1. Window id ≠ open menu → ignored.
2. Spectator or dead → full state sent back (`container_set_content`), nothing else.
3. Menu no longer valid (block still of the right type and player within block
   interaction reach + 4; for a container, its own rule; living and nearby entity
   for mounts/merchants) → ignored (the server tick will close the menu).
4. Slot out of bounds (except −999 = outside the window, and −1) → ignored.
5. `desync = packet's state number ≠ current number`.
6. Updates to the client suspended; click executed; then the server records, for
   each slot listed in the packet, the FINGERPRINT announced by the client as "what the
   client believes" (`inventories.md` § 1.7), and likewise for the carried item.
7. Resume: if desync → full resend; otherwise broadcast only the differences (§ 4.2).

The menu button (`container_button_click`: enchanting, stonecutter, loom,
lectern…): refused in spectator; if the menu accepts it → broadcast the differences.
Creative (`set_creative_mode_slot`): only with infinite materials; slot 1..45 of the
player inventory (empty item or count ≤ max) → placed as is; slot < 0 → dropped (rate
limited); item of a disabled feature → ignored.

### 4.2 Synchronisation

The server keeps, per slot, a "remote copy" (exact stack, or only the fingerprint
received from the client). Every tick (and after each action):
- for each slot: if the remote copy does not match the real stack (exact equality,
  or equal fingerprint → the copy becomes the stack) → `container_set_slot` (window,
  incremented state number, slot, stack);
- carried item → `set_cursor_item`;
- data (`container_set_data`): each integer that changed.
- Full resend (`container_set_content`): on opening, on desync, after a
  forced close.
State number: 15 bits, `+1` on each send that carries it.

Closing (`container_close` from the client, or server): the menu returns to the player the content of the
temporary slots (crafting grid, anvil, enchanting table, stonecutter, loom,
grindstone, smithing table…): into the inventory, otherwise dropped; the carried item is returned the same way. The inventory
menu (window 0) never closes; its 2×2 grid is emptied.

### 4.3 Crafting (crafting table 3×3, inventory 2×2, crafter)

On each change of the grid:
1. **Normalised input**: the grid is trimmed to the smallest rectangle containing the non-empty
   slots (left/right/top/bottom); the corner position is kept.
2. **Lookup**: first the previously used recipe (hint), otherwise the first
   recipe of type `crafting` (in data load order) that matches.
3. **Rule `limited_crafting`**: a non-special recipe absent from the player's
   recipe book gives nothing.
4. Result placed in slot 0 (immediate `container_set_slot`, state incremented); item
   of a disabled feature → empty.

**Matching by type**:
- `crafting_shaped` (`pattern` rows of characters, `key` character → ingredient, space =
  empty, `result`, `show_notification`): the pattern is first trimmed of its empty
  rows/columns. Matches if the number of non-empty slots is the same, the trimmed dimensions
  are equal, and each slot tests the expected ingredient (empty ↔ empty), either directly
  or horizontally MIRRORED (`x → width − 1 − x`); the mirror is only tried if the pattern
  is not symmetric. Mirror attempted first.
- `crafting_shapeless` (`ingredients`): same number of non-empty slots as ingredients;
  a single one: direct test; otherwise a one-to-one assignment slots ↔ ingredients is needed
  (bipartite matching: each ingredient must find its own stack).
- `crafting_transmute` (`input`, `material`, `material_count` default exactly 1, `result`
  {optional `id`, `count`, `components`}, `add_material_count_to_result`): number of non-empty
  slots within `[min+1, max+1]`; exactly one `input` stack; all the others are
  `material`, at most `max`; count within the bounds; if the result would be 1 item, it must
  DIFFER from the input (item or components). Result = item `result.id` (or the input's)
  with ALL the input's modified components, then `result.components` on top;
  quantity `count` (+ number of materials if requested).
- `crafting_dye` (`target`, `dye`, `result`): ≥ 2 stacks, a single `target`, the others
  dyes (with component `dye`). Colour: average of the R, G, B components of the current
  colour (if dyed) and of the dyes' diffuse colours, then rescaled: `c × (average
  of the maxima) / max(average R, G, B)`, integers truncated; the input's components kept.
- `crafting_imbue` (`source`, `material`, `result`): full 3×3 grid, centre = `source`,
  8 others = `material`; result with the source's `potion_contents` (tipped arrows).
- `crafting_decorated_pot` (`back`, `left`, `right`, `front`): 3×3 diamond pattern.
- `crafting_special_*` (hard-coded logic, the data only enables it): `bannerduplicate`,
  `bookcloning`, `firework_rocket`, `firework_star`, `firework_star_fade`, `mapextending`,
  `repairitem` (§ 3.1), `shielddecoration`.

**Taking the result** (slot 0): statistics, recipe unlocked (non-special), then for
each slot of the trimmed input: stack −1, and the recipe's "remainder" (by default the
item's `crafting_remainder`: milk bucket → bucket, etc.) goes into the slot if it is
empty or compatible (merge), otherwise into the inventory, otherwise onto the ground. Quick move on
the result: repeats as long as the result stays identical and there is room.

### 4.4 Furnaces (furnace, blast furnace, smoker) and campfire

Slots: 0 ingredient, 1 fuel, 2 result. Data: `lit_time_remaining`,
`lit_total_time`, `cooking_time_spent`, `cooking_total_time`, speed multiplier.
Recipe types: `smelting`, `blasting`, `smoking` (`ingredient`, `result`, `experience`,
`cookingtime` mandatory in the file: 200 for the furnace, 100 for the fast furnaces in the
vanilla recipes), `campfire_cooking`.

Fuel: component `cooking_fuel` of the item = duration (`burn_time`) and multiplier
(`speed_multiplier`) given by data-driven value providers
(`context_int_provider/cooking/time_*.json`, `context_float_provider/cooking/*.json`) that
depend on the block: `time_coal` = `1600 / (2 if block ∈ predicate block/fast_cooking —
smoker, blast furnace — otherwise 1)`. (The others: lava bucket 20,000, coal block 16,000,
blaze rod 2400, planks/wood 300, slabs 150, small items 100, etc., all divided by
2 in the fast furnaces; to be read from the data.)

Server tick:
1. If lit: `lit −1`.
2. If (lit) or (fuel and ingredient): with an ingredient and a recipe (cache of the
   last recipe) whose result fits in slot 2 (same item and components, sum ≤
   min(64, the item's max)):
   - not lit: new duration and multiplier taken from the fuel; if a cook was
     under way, its progress is kept in proportion to the new total time; if duration >
     0: fuel −1 (its remainder: lava bucket → bucket, placed or dropped), lit;
   - lit: progress +1; at `total_time = ⌈cookingtime / multiplier⌉`: progress
     0, result added, ingredient −1 (wet sponge + empty bucket as fuel → water
     bucket), recipe counted for XP;
   - otherwise progress 0.
3. Otherwise, if progress > 0: progress −2 (clamped to 0).
4. Lit/unlit change → the block's `lit` property.
XP: when a player takes out the result (or when the block is destroyed), for each recipe
counted `n` times: `x = n × experience`; `⌊x⌋` + 1 with the probability of the
fractional part; orbs to the player. Recipes unlocked for them.

Campfire: 4 slots, each item cooks independently (the recipe's `cookingtime`,
600 in vanilla), no fuel; the result is dropped above.

### 4.5 Brewing stand

Slots 0-2 bottles, 3 reagent, 4 fuel. Data: brewing time, remaining
fuel.

- Data-driven recipes `recipe/brewing/*.json` (type `brewing`, 279 in 26.3):
  `input` (item predicate: `item` + components, e.g. `potion_contents.potions`), `reagent`
  (predicate), `output` (full item). Matching: first recipe whose `input` tests
  the bottle and `reagent` the reagent. Result = `output` as is.
- Fuel: component `brewing_fuel` → number of uses (`context_int_provider/brewing/
  uses_default` = 20) and multiplier (`context_float_provider/brewing/speed_default` = 1).
  Reloaded as soon as the remaining fuel is 0 and a fuel item is present (stack
  −1, possible remainder).
- "Brewable": reagent present, belonging to the set of known reagents, and at least
  one bottle with a recipe.
- Tick: in progress → time −1; at 0 and brewable → brewing (each bottle replaced by
  the result of its recipe if there is one, reagent −1 with its remainder, event 1035);
  if no longer brewable or reagent changed → stop (time 0). Not in progress, brewable and
  fuel > 0 → fuel −1, time = `⌈400 / multiplier⌉`, reagent remembered.
- The block's `has_bottle_0..2` properties updated.

### 4.6 Smithing table (`smithing`)

Slots: 0 template, 1 base, 2 addition, 3 result. Recipes:
- `smithing_transform` (`template`, `base`, `addition`, `result`): result = the result
  item with all of the base's modified components (enchantments, wear, name…).
- `smithing_trim` (`template`, `base`, `addition`, `pattern`): base in
  `#trimmable_armor`, material = that of the addition (`provides_trim_material`); result = copy
  of the base with component `trim` {material, pattern}; empty if the base already has exactly this
  trim.
Taking: each input −1, smithing sound.

### 4.7 Stonecutter

`stonecutting` recipes (`ingredient`, `result`). List of the recipes valid for the input
item, in the order of the "selectable" set (sorted as on the client side, see
`inventories.md`); the button chooses the index; taking: input −1, sound (at most once per
tick).

### 4.8 Anvil

Inputs: 0 item, 1 addition; typed name (`rename_item`, ≤ 50 characters after filtering).
Computation (on each change):
1. Empty item or one that cannot carry enchantments → nothing. `base_cost = repair_cost(item) +
   repair_cost(addition)`; `price = 0`.
2. Addition present:
   - **Repair by material** (item usable and addition in the item's `repairable.items`):
     `r = min(wear, max/4)`; if 0 → nothing; repeat while `r > 0` and materials remain:
     wear −= r, price +1, `r` recomputed; that number of materials will be consumed.
   - Otherwise: an enchanted book (`stored_enchantments`) is needed, or the same usable item.
     - Same usable item: `new wear = max − (item_rest + addition_rest + max × 12/100)`
       (≥ 0); if better: applied, price +2.
     - For each enchantment of the addition: level `n = (current == addition ? addition + 1 :
       max(addition, current))`; applicable if the enchantment accepts the item (`supported_items`)
       or if the player is in creative or item = enchanted book; each already-present
       incompatible enchantment → not applicable and price +1; applicable → level capped at max, set,
       `price += cost × n` with `cost = anvil_cost` (divided by 2, min 1, if the addition is
       a book); if the item is a stack of > 1 → price = 40.
     - Only non-applicable enchantments → nothing.
3. Name: different from the displayed name → literal `custom_name`, price +1; empty name while
   the item has a name → name removed, price +1.
4. `cost = price ≤ 0 ? 0 : base_cost + price`; price ≤ 0 → nothing. If only the name changes and cost ≥ 40 →
   cost 39 (renaming always possible). Cost ≥ 40 outside creative → nothing ("Too Expensive!").
5. `repair_cost` of the result = `max(item, addition)` then, except for a pure rename, `2 × x + 1`.
Taking: only if cost > 0 and (creative or level ≥ cost); levels −cost; addition: −n
materials, or removed (except for a pure rename); item removed; anvil damaged with probability
0.12 outside creative (`anvil → chipped → damaged → destroyed`, events 1029/1030).
The cost is data 0 of the menu.

### 4.9 Enchanting table

Generator: the game's 48-bit linear congruential generator ("legacy random", the same as
Java's), reseeded on every computation. Seed = the player's enchantment seed
(`XpSeed`).

**Offers** (on each change of the item): non-empty and enchantable item (component
`enchantable`, not yet enchanted except a book), otherwise offers cleared.
1. Bookshelves: positions `(dx, dy, dz)` with `dx, dz ∈ [−2, 2]`, `dy ∈ {0, 1}`, `|dx| = 2`
   or `|dz| = 2` (32 positions); counts if the block is in `enchantment_power_provider` and
   the block halfway (`dx/2, dy, dz/2` truncated) in `enchantment_power_transmitter`.
   `b = min(count, 15)`.
2. `reseed(seed)`; for i = 0, 1, 2: `s = next_int(8) + 1 + b/2 + next_int(b + 1)`;
   `cost[0] = max(s/3, 1)`, `cost[1] = 2s/3 + 1`, `cost[2] = max(s, 2b)`; if `cost[i] < i+1` → 0.
3. For each offer with cost > 0: list = selection(seed + i, cost) (below); displayed
   hint = one element drawn with `next_int(size)` using the current generator (the one of step 2
   reseeded by the selection) → enchantment id and level (menu data 4-9;
   costs = data 0-2, displayed seed = data 3).

**Selection(seed, cost)**: `reseed(seed + i)`; set = tag `#in_enchanting_table`;
`e = enchantable.value`; `cost += 1 + next_int(e/4 + 1) + next_int(e/4 + 1)`; `r =
(next_float + next_float − 1) × 0.15`; `cost = clamp(round(cost + cost × r), 1, ∞)`;
candidates = enchantments whose `primary_items` contains the item (all for a book), at the
highest level `n` (from max down to min) such that `min_cost(n) ≤ cost ≤ max_cost(n)`; weighted
draw (`weight`, `next_int(total)`); then while `next_int(50) ≤ cost`: remove the
candidates incompatible with the last one chosen, if any remain draw again, `cost /= 2`.
Book: if > 1 result, one result removed at random (`next_int(size)`).

**Click on offer i**: lapis ≥ i+1 and level ≥ max(i+1, cost[i]) (except creative);
selection recomputed; if non-empty: levels −(i+1) and NEW player seed (§ 1.3),
book → enchanted book, enchantments applied, lapis −(i+1), offers recomputed with the
new seed, sound.

### 4.10 Trading (villagers, wandering trader)

**Data-driven offers**:
- `trade_set/<profession>/level_<n>.json`: `trades` (trade tag `#<profession>/level_<n>`),
  `amount` (number of offers added at this level, integer provider), `allow_duplicates`
  (default false), `random_sequence` (the world's named random sequence).
- `villager_trade/<profession>/<n>/*.json`: `wants` {item, `count`, required components}
  (cost A), `additional_wants` (cost B), `gives` (item given), `max_uses` (default 4), `xp`
  (default 1), `reputation_discount` (price multiplier, default 0), `merchant_predicate`
  (loot condition on the merchant: villager type, biome…), `given_item_modifier`
  (loot function applied to the given item: random enchantment, map…),
  `double_trade_price_enchantments` (if the given item stores one of them, the surcharge is doubled).
- Creating an offer: merchant condition, given item (+ modifier; empty → no
  offer), surcharge removed from the given item's `additional_trade_cost` component, cost A =
  `wants` + surcharge (< 1 → no offer), cost B, `max_uses ≥ 1`, `xp ≥ 0`.

**Displayed/paid price** of cost A: `base + max(0, ⌊base × demand × multiplier⌋) +
special_difference`, clamped to `[1, max stack size]`. Cost B is not modified.
- `demand`: on each restock `demand += uses − (max − uses)`
  (can become negative; only the positive part weighs on the price).
- `special_difference` (recomputed when a player opens the trade): `−⌊reputation ×
  multiplier⌋` (the player's reputation with the villager: trading, curing, attacks…);
  Hero of the Village effect: `−max(⌊(0.3 + 0.0625 × amp) × base⌋, 1)`.
- Transaction: costs A and B present and sufficient (item tested with its required components),
  quantities removed, uses +1, villager XP += the offer's `xp`; orbs `3 +
  random in [0, 3]` (+5 on a level-up) if the offer rewards XP; level-up
  at thresholds 10 / 70 / 150 / 250 → new offers of the next level, Regeneration 10 s.
- Offer sold out when `uses ≥ max`.
- **Restock**: at the workstation, at most 2 times a day (the 2nd at least 2400
  ticks after the 1st) and only if an offer needs it: demand updated, uses
  reset to 0, special prices recomputed. New day (12,000 ticks since the last one, or calendar
  day changed): counter reset to 0. Catch-up on respawn / loading:
  `2 − restocks of the day` demand updates.

---

## 5. Loot (`loot_table`)

### 5.1 Format 26.3

File `loot_table/<path>.json`:
- `type`: expected parameter set (`block`, `entity`, `chest`, `fishing`, `gift`,
  `archaeology`, `vault`, `shearing`, `equipment`, `barter`, `command`, `selector`…) —
  used for validation;
- `pools`: list of rolls;
- `modifier` (optional): function(s) applied to every item the table produces;
- `random_sequence` (optional): name of the world's random sequence (§ 5.5).

**New in 26.3**: a single `condition` field (instead of a list; a list is done with
`all_of`) and a single `modifier` field (one function, a LIST of functions = sequence, or the
name of a registered function/condition, e.g. `"minecraft:tool/can_silk_touch"` →
`predicate/tool/can_silk_touch.json`). Everywhere: table, pool, entry, function.

**Pool** (`pool`): `entries`, `condition`, `modifier`, `rolls` (integer provider),
`bonus_rolls` (float provider, default 0).
1. If `condition` fails → nothing.
2. `n = rolls + ⌊bonus_rolls × luck⌋` (`luck` = the `luck` attribute of the player concerned).
3. Repeat `n` times: expand all the entries (§ 5.2) into candidates of weight > 0;
   if there is only one → it produces; otherwise `r = next_int(Σ weights)`, and the weights are
   subtracted in order until `r < 0`. The items produced go through the pool's `modifier` then
   the table's.
4. Output: stacks larger than the max size are split; items of a disabled
   feature removed.

### 5.2 Entries

Common fields: `condition`, `modifier`; simple entries: `weight` (default 1), `quality`
(default 0) → effective weight `max(⌊weight + quality × luck⌋, 0)`.

| Type | Effect |
|---|---|
| `item` (`name`) | a stack of 1 of the item |
| `tag` (`name`/`items`, `expand`) | `expand` false: produces one copy of EACH item of the tag; true: each item becomes a separate candidate of the same weight |
| `loot_table` (`value`: name or inline table) | runs the nested table (guard against loops) |
| `dynamic` (`name`) | content supplied by the block (`contents`: content of a shulker box, a decorated pot…) |
| `slots` (`slot_source`) | items read from slots of an entity/container of the context (new) |
| `empty` | nothing (takes up weight) |
| `alternatives` (`children`) | the FIRST child whose condition passes is expanded, the following ones ignored |
| `sequence` (`children`) | children expanded in order until the first one whose condition fails |
| `group` (`children`) | all the children expanded |

A composite entry whose own `condition` fails "fails" (counts for
`alternatives`/`sequence`). Its `modifier` applies to the items of its children.

### 5.3 Conditions (predicates)

`inverted` (`term`), `any_of` / `all_of` (`terms`), `random_chance` (`chance` provider:
`float < chance`), `random_chance_with_enchanted_bonus` (`enchantment`,
`unenchanted_chance`, `enchanted_chance` per-level value; level = highest on
the attacker), `entity_properties` (`entity`: `this`, `attacker`, `direct_attacker`,
`attacking_player`… + entity predicate), `killed_by_player` (parameter
`last_damage_player` present), `entity_scores`, `match_block` (new: blocks + properties),
`match_tool` (item predicate on `tool`), `table_bonus` (`enchantment`, `chances[]`: chance
= `chances[min(level, size−1)]`), `survives_explosion` (if the explosion radius is known:
`float ≤ 1/radius`, otherwise true), `damage_source_properties`, `location_check` (location
predicate + offset), `weather_check`, `time_check`, `int_value_check`, `float_value_check`,
`enchantment_active_check` (location-based enchantment effects), `environment_attribute_check`.

Item, entity, location and damage predicates: same formats as advancements and
commands (`items.md` for the component predicates).

### 5.4 Functions (`modifier`)

Each function can carry its own `condition`. 26.3 list: `set_count` (`count`, `add`
: replaces or adds), `set_item`, `set_damage` (fraction of remaining durability), `set_name`,
`set_lore`, `set_components`, `set_custom_data`, `copy_custom_data`, `copy_components`,
`copy_name`, `copy_state` (block properties → `block_state`), `set_attributes`,
`set_enchantments`, `enchant_with_levels` (like the table, § 4.9, with a provided cost),
`enchant_randomly` (a random compatible enchantment, random level), `apply_bonus`,
`enchanted_count_increase`, `explosion_decay`, `limit_count`, `furnace_smelt` (replaces with
the smelting result), `exploration_map`, `set_stew_effect`, `set_contents`,
`modify_contents`, `set_loot_table` (chest to fill later), `fill_player_head`,
`set_banner_pattern`, `set_potion`, `set_random_potion`, `set_random_dyes`,
`set_instrument`, `set_fireworks`, `set_firework_explosion`, `set_book_cover`,
`set_written_book_pages`, `set_writable_book_pages`, `toggle_tooltips`,
`set_ominous_bottle_amplifier`, `set_custom_model_data`, `filtered` (applies only if
the item passes a predicate), `sequence`, `discard` (removes the item).

Formulas that matter for drops:
- `apply_bonus` (`enchantment`, `formula`) — level read from the context's TOOL:
  - `ore_drops`: if level > 0: `b = max(next_int(level + 2) − 1, 0)`, `quantity × (b+1)`;
  - `uniform_bonus_count` (`bonusMultiplier`): `+ next_int(mult × level + 1)`;
  - `binomial_with_bonus_count` (`extra`, `probability`): `level + extra` attempts, +1 per
    `float < probability`.
- `enchanted_count_increase` (`enchantment`, `count` float, `limit`) — level = highest on
  the living attacker: `+ round(level × count)`, then cap `limit` if > 0.
- `explosion_decay`: if the radius is known, each item of the stack survives if `float ≤ 1/radius`.
- `limit_count`: min/max bound.

**Number providers** (new in 26.3, shared with the furnaces/brewing stand/trading):
a literal number = constant; `uniform` (`min`, `max`: integer `min + next_int(max − min
+ 1)` if min < max otherwise min; float `min + next_float × (max − min)`), `binomial` (`n`, `p`
: n draws), `constant`, `weighted_list`, `conditional` (`condition`, `on_true`,
`on_false`), operators `add`, `sub`, `mul`, `div`, `mod`, `floor_div`, `floor_mod`, `pow`,
`min`, `max`, `avg`, `abs`, `negate`, `floor`, `ceil`, `round`, `truncate`, `sqrt`, `sin`,
`cos`, `length`, conversions `from_int` / `from_float`, reads `score`, `storage`,
`enchantment_level`, `environment_attribute`, `number_dispatcher`, `context`. A provider
can be the name of a `context_int_provider/…` or `context_float_provider/…` file.

### 5.5 Context and randomness

Possible parameters: `this_entity`, `origin`, `damage_source`, `attacking_entity`,
`direct_attacking_entity`, `last_damage_player`, `block_state`, `block_entity`, `tool`,
`explosion_radius`, `enchantment_level`, `enchantment_active`, plus the `luck`.

Generator: in priority order, (a) an explicit non-zero seed (`LootTableSeed`
of a chest, entity seed) → legacy Java-style congruential generator initialised with it;
(b) otherwise the table's named random sequence `random_sequence`: a Xoroshiro128++
generator specific to the world, created on first use from the seed `(world
seed ^ salt)` widened to 128 bits and XORed with the hash of the sequence name, then mixed,
and SAVED in the world (`random_sequences.dat`); (c) otherwise the level's randomness.
→ To get drops identical to the official game on the same world, these sequences must be
persisted and resumed. The `loot` command follows the same rule for its four sources (`loot`, `fish`,
`kill`, `mine`): never the level's randomness when the table has a sequence, otherwise the draw
depends on what the ticks consumed between two commands.

### 5.6 Tables used

| Situation | Table | Parameters |
|---|---|---|
| Block broken by a player with the right tool (§ 3.2), by explosion, piston, water… | `blocks/<block>` (block property; none for air, bedrock…) | `origin` = centre, `tool` (empty except player), `this_entity` (breaker), `block_entity`, `block_state`, `explosion_radius` (explosion) |
| Mob killed | `entities/<type>` (variants: `entities/sheep/<colour>`…); if rule `mob_drops`; plus equipment (drop chances) | `this_entity`, `origin`, `damage_source`, `attacking_entity`, `direct_attacking_entity`, `last_damage_player` + luck if killed by a player within 100 ticks |
| Chest, barrel, minecart, pot, treasure chest… with `LootTable` | `chests/…`, `pots/…`, etc. | `origin`, `this_entity` (the player who opens) + their luck; filled on first opening (or destruction, or hopper) |
| Fishing | `gameplay/fishing` (→ `fish`, `junk`, `treasure`) | `origin`, `tool`, `this_entity`, luck = Luck of the Sea + attribute |
| Piglin bartering, gifts (cat, hero of the village), sniffer, armadillo… | `gameplay/…` | depending on the case |
| Shearing, brushing, tilling, carving (pumpkin) | `shearing/…`, `brush/…`, `till/…`, `carve/…` | tool, entity/block |
| Vault / trial spawner | `chests/trial_chambers/…`, `spawners/…` | |
| Archaeology | `archaeology/…` | |
| Equipment of spawned mobs | `equipment/…` | |

**Filling a container** (§ 5.5 for the randomness): items generated; empty slots listed
then shuffled; empty stacks removed; stacks of quantity > 1 set aside and, as long as
there are more free slots than items (single stacks + stacks set aside), a random stack
(`uniform integer [0, size−1]`) is cut by `uniform integer [1, q/2]` items, and
each piece goes back to the splitting with a one-in-two chance if it is larger than 1, otherwise goes
into the list; leftovers added, list shuffled; each item goes into the LAST slot of the
shuffled list of free slots (removed).

**Placing a block's items**: each stack appears as an item entity at the centre of the block +
`uniform(−0.25; 0.25)` on each axis (y: minus half the item entity's height, 0.125),
level randomness, default pickup delay (10 ticks); only if the rule
`block_drops` is true (same for block XP). Block XP: ores,
spawners, sculk… through the spawning after destruction (if the tool does not have Silk Touch, § 3.5).

---

## 6. Basic commands (singleplayer)

Client side (received tree, parsing, suggestions, display): `commands.md`. The server
owns the full tree; it sends each player (on connection, on change of
permission level, of game mode or of rule `send_command_feedback`… — each time the rights
change) the `commands` packet containing ONLY the nodes that this player can use
(redirects are rewired to the copies).

### 6.1 Permissions

Levels: 0 everyone, 1 moderator, 2 game master (`gamemode`, `tp`, `give`, `time`, `weather`,
`gamerule`, `effect`, `enchant`, `xp`, `kill`…), 3 administrator, 4 owner. Singleplayer:
the world's owner is an operator if "allow commands" (world option
`allowCommands`) → level 4; otherwise 0. Opening to LAN: guests level 2 if guest
access is ticked, 0 otherwise.

### 6.2 Execution

1. The client sends `chat_command` (unsigned) or `chat_command_signed`.
2. Parsing on the tree with the "player" source (position, rotation, dimension, permission,
   entity). Parse error → red message (error text), then a grey context line:
   `…` if cursor > 10, the 10 characters before the cursor, the rest in red
   underlined, then `<--[HERE]` (`command.context.here`) in red italics; click = puts the
   command back in the field.
3. Execution (execution queue, function depth limited by the rule
   `max_command_sequence_length`); syntax exception at execution → red message;
   unexpected exception → `command.failed` with the error on hover.
4. Feedback: success → system message to the source (except silent source); if the feedback
   is "broadcast": to the other operators in grey italics `chat.type.admin` (rule
   `send_command_feedback`) and to the server log (rule `log_admin_commands`).
   A false `send_command_feedback` rule also cuts the feedback to the player source.

### 6.3 Minimal commands

**`/gamemode <mode> [<targets: players>]`** (level 2) — modes `survival`, `creative`,
`adventure`, `spectator`. For each target whose mode changes (§ 1.6): feedback
`commands.gamemode.success.self` (if the target is the source) otherwise
`commands.gamemode.success.other` + message `gameMode.changed` to the target (if
`send_command_feedback`). Result = number of players changed (0 → no particular
error, nothing is said). `/defaultgamemode` changes the mode of new players.

**`/teleport`** (alias `/tp` by redirect, level 2):
- `<location>`: the source (must be an entity);
- `<destination: entity>`;
- `<targets> <location> [<rotation> | facing <location> | facing entity <entity> [eyes|feet]]`;
- `<targets> <destination: entity>`.
Absolute coordinates, relative `~` (to the source) or local `^` (rotation frame).
Position out of limits (block with `x` or `z` outside `[−30000000, 30000000[`, or height
outside the "can spawn" range, ±20000000) → `commands.teleport.invalidPosition`. For each target: relative components handled as an offset
("relative" teleport sent to the client, `player_position` with flags), angles
normalised; dimension change if the destination is an entity elsewhere;
gaze turned if `facing`; except when gliding: vertical velocity cancelled and target placed on the ground;
mob navigation stopped. Feedback `commands.teleport.success.location.single`
(coordinates to 2 decimals) / `…entity.single` / `…multiple`.

**`/give <targets> <item>[components] [<quantity ≥ 1>]`** (level 2) — max quantity =
`max_size × 100` otherwise error `commands.give.failed.toomanyitems`. For each player, in
chunks of `max_size`: added to the inventory; if everything went in → "ghost" item (dummy item
entity that disappears at once, for the pickup animation) + sound
`entity.item.pickup` (volume 0.2, pitch `((r1 − r2) × 0.7 + 1) × 2`); otherwise the rest is
dropped at the player's feet, with no pickup delay and reserved for them. Feedback
`commands.give.success.single` / `.multiple`.

**`/time`** (level 2) — 26.3: time goes through CLOCKS (`world_clock`, e.g.
`overworld`, `the_end`) and timelines (`timeline/day.json`, period 24,000);
the default clock is that of the dimension type (error if none).
- `set <duration>` (ticks, suffixes `d` = 24,000, `s` = 20, `t`) → clock total;
  identical → error; `set <marker>`: timeline markers (`day` 1000, `noon` 6000,
  `night` 13,000, `midnight` 18,000) → advances to the next occurrence (`total +
  (marker − total mod period)`, or + period if ≤ 0); already on it → error.
- `add <duration>` (can be negative, total floored at 0).
- `query time | gametime | <timeline> [repetition]`.
- `pause`, `resume`, `rate <0.00001..1000>` (clock speed).
- `of <clock> …`: same subcommands on a given clock.
Result: value modulo 2³¹−1. Clients receive the time update
(`set_time`); see `sky.md` / `environment.md`.

**`/weather clear|rain|thunder [<duration>]`** (level 2) — without a duration: uniform draw
`clear` 12,000-180,000, `rain` 12,000-24,000, `thunder` 3,600-15,600 ticks. `clear`:
delay before rain = duration, rain and thunder cut; `rain`: rain for the duration, without
thunder; `thunder`: rain + thunder for the duration. Effect on the weather cycle and game
event packets (rain start/end, levels): `weather-border.md`.

**`/gamerule <rule> [<value>]`** (level 2) — each rule is reachable by its
short identifier (`keep_inventory`) and qualified one (`minecraft:keep_inventory`); without a value
→ `commands.gamerule.query`; identical value → error `commands.gamerule.not_set`;
otherwise written (with reactions: `reduced_debug_info`, `immediate_respawn`,
`limited_crafting`, `locator_bar`… sent to the clients) and `commands.gamerule.set`. Result
= value (boolean → 0/1). Rules cited in this document: `natural_health_regeneration`,
`keep_inventory`, `show_death_messages`, `immediate_respawn`, `forgive_dead_players`,
`respawn_radius`, `drowning_damage`, `fall_damage`, `fire_damage`, `freeze_damage`,
`mob_drops`, `block_drops`, `mob_griefing`, `limited_crafting`, `send_command_feedback`,
`log_admin_commands`.

To follow (beyond the minimum): `effect`, `enchant`, `xp`, `kill`, `clear`, `summon`, `setblock`,
`fill`, `spawnpoint`, `setworldspawn`, `difficulty`, `locate`, `seed`, `say`, `tellraw`… —
each relying on sections 1-5.

---

## 6 bis. Singleplayer implementation (rc-sim, milestone 2)

What is simulated and where: menus and clicks (§ 4.1-4.2, `menus.rs`, same algorithm as
`inventories.md` § 2; fingerprints compared after the same network formatting as the client),
crafting and cooking (§ 4.3-4.4, `recipes.rs`, `containers.rs`), block loot (§ 5,
`loot.rs`, random sequences resumed from `random_sequences.dat`, `rng.rs`), ground items and
orbs (`drops.rs`, saved in `entities/`), health, hunger, environmental damage, death and
respawn (§ 1-2, `vitals.rs`), weather and sleep (`weather.rs`).

Default components of items: reports of the official generator kept in the data
cache (`<cache>/<version>/generated/reports/minecraft/components/item/`). Crafting remainders
(buckets, bottles): an item property of the game, not a datum; fixed in `items.rs`.

Known differences: no status effects, attacks or mobs; chests with a `LootTable` not
filled; respawn anchor not handled; special recipes hard-coded only for
`repairitem`; level randomness = Xoroshiro drawn from the clock (the game: legacy generator); enchantment
protections coded for the five basic protections (not read from `enchantment/`).
Player sounds (hurt, fall, burning): the game sends them to everyone EXCEPT the player themself
(their client plays them, from the damage event and its own physics); our client does not
play them yet, so the singleplayer sends them to it directly — to be removed once the client plays them.

Readings on the official server (12 Oct, same figures as the singleplayer): a 10-block fall → 7
damage; merging on the ground 40 + 30 → none, 40 + 20 → 60, 63 + 1 → 64. As the test server was on
peaceful, regeneration outside peaceful is checked against the code (tick-by-tick walk-through).

## 7. Uncertain points

1. **Recipe order**: the first matching recipe is taken in registry order (file
   load order, a priori alphabetical by identifier). Two competing
   recipes would give a different result if our order differs: to check on a real case
   (e.g. dyes).
2. **Matching of shapeless recipes**: the official assignment-search algorithm
   (the "stacked contents" structure) is not detailed here; any correct bipartite matching gives
   the same YES/NO, but not necessarily the same consumption if two ingredients
   overlap (no effect: one slot = one item).
3. **Mobs**: own damage of mob projectiles, mob melee attacks
   (`attack_damage` + weapon effects, with no recharge or critical), wind charge
   knockback, explosions: to be described with the mob AI.
4. **Villager reputation** (gossip, decay, sharing) and the choice of offers from a
   `trade_set` (draw with the named random sequence, duplicates): not detailed.
5. **Location-based enchantment effects** (`location_changed`): exact order of activation /
   deactivation when equipment changes: to check on Frost Walker.
6. **Item predicates with components** (`input` of brewing recipes, `wants` of trading,
   `match_tool`): the full grammar of the component sub-predicates is in
   `items.md` on the client side; to confirm that it covers `potion_contents.potions`.
7. **Exact fuel values**: only `time_coal` (1600) and the fast-furnace factors
   were read; the other values (`time_*`) are to be read from the data, not
   copied from memory.
8. **Number providers**: the exact semantics of `context`, `storage`,
   `number_dispatcher` and `environment_attribute` was not read (not used by the
   basic loot tables).
9. **Stonecutter**: exact order of the list of offered recipes (sorting) not checked
   on the server side.
10. **Commands**: the list of rules that trigger a resend of the command tree
    and the format of each argument's error messages are in `commands.md`;
    not re-checked here.

