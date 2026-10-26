# Projectiles, fishing, maps: integrated server (S8)

Specification written in our own words from the behaviour of the official 26.3 server (tick-by-tick
comparisons on a local server, see § 8). Code: `crates/rc-sim/src/projectiles/`,
`maps.rs`, `lootseal.rs`; client side, `apps/client/src/projectile_sim.rs`.

## 1. Common projectile tick

- Before each tick: `ticks_lived + 1`, start position kept (`old_pos`, used by the pearl).
- End of the tick ("base tick"): water sampled by the box (currents applied to free
  projectiles), fire counted down by one, lava = 15 s of fire, water = fire put out, removal below `min_y − 64`.
- Owner "left" when none of the owner's boxes touches the projectile's box
  stretched by its velocity and inflated by 1; before that the projectile cannot hit the owner.
  Afterwards it can (arrow fired vertically that falls back onto the shooter).
- Block trace: collision shapes only, fluids ignored; cell walk with bounds
  extended by 1e-7; start point inside a shape = immediate impact at 0.001 of the segment.
- Entity trace: boxes inflated by the margin `clamp((ticks_lived − 2) / 20, 0, 0.3)`.
  Thrown items take the nearest one (distance to the entry point); arrows take
  all the boxes crossed (bare box first, otherwise inflated box confirmed by a second
  trace towards the center) sorted by distance to the target's feet.

## 2. Arrows and trident

Order: block at the tip full → stuck; shake −1; rain or water puts out fire.
Stuck: falls if the block changed and nothing is left within ±0.06 (velocity × random × 0.2), otherwise
despawns at 1,200 ticks (except a pickable loyal trident). In flight: water → ×0.6 (trident 0.99),
smoothed rotation (×0.2), trace, movement in steps (piercing: restart from the
target hit), then drag 0.99 out of water and gravity 0.05.

- Damage: `ceil(|v| × base)`, base 2 + power (0.5 × level + 0.5); critical: + random
  `[0, damage/2 + 2)`; on fire: target set ablaze for 5 s; rejected: fire given back, bounce (−0.1 × v,
  half-turn of 170 to 190°), and below 1e-7 of velocity the arrow drops as an item.
- Knockback: that of the hurt follows the velocity; punch: `level × 0.6 × (1 − resistance)`
  horizontal + 0.1 vertical.
- Impact on a block: step back of 0.05 × sign of each component (not the normalized direction),
  zero velocity, sound with random pitch, stuck, critical and piercing reset.
- Tipped arrow: potion effects × duration scale (0.125 for tipped arrows);
  stuck for 600 ticks → event 0 and plain arrow. Spectral: glow 200 ticks.
- Pickup: player's box inflated (1, 0.5, 1), stuck and without shake; "allowed"
  if fired by a player, "creative only" for an infinity or creative arrow.
- Trident: 8 damage + impaling (sensitive mobs), a single target then no more entity
  trace; bounce (0.02, 0.2, 0.02); loyalty: after 4 ticks stuck or a hit,
  flies back to the owner's eyes (`y += dy × 0.015 × L`, `v = v × 0.95 + dir × 0.05 L`),
  without physics; channeling in a thunderstorm (target or lightning rod open to the sky): lightning.

## 3. Player weapons and throwables

- The `use_item` packet carries yaw and pitch: applied before the use.
- Bow: 72,000 ticks of use; force `p = t/20`, `(p² + 2p)/3` capped at 1, refused below 0.1;
  velocity 3p, inaccuracy 1, critical at full force; ammunition: off-hand, main hand,
  then inventory (hotbar first), dummy arrow in creative; wear 1 per arrow.
- Shot from the rotation: vector by tabulated sines, `shoot` (normalized, + triangle(0, 0.0172275
  × inaccuracy) per axis, × power), then + known player movement (vertical zero on the ground).
- Crossbow: charge `floor(1.25 s modified × 20)`; sounds at 20%, 50%, loading at 100%
  (component `charged_projectiles`); shot on the next click, 3.15 (1.6 if rocket), critical
  always for a player; multishot: 3 projectiles at 0 and ±10°, non-pickable copies;
  sound pitch 1 then `1/(r×0.5+1.8) + 0.63/0.43`; rocket: wear 3.
- Trident: at least 10 ticks; riptide: only when wet, spin attack 20 ticks
  (8 damage to the first mob hit).
- Thrown: snowball/egg/pearl 1.5; potions 0.5 with −20°; experience bottle 0.7 with −20°; start at the eyes
  − 0.1. Gravities: 0.03 (snowball, egg, pearl), 0.05 (potions), 0.07 (experience bottle); gravity then
  inertia (0.99, water 0.8) before the trace.
- Cooldown (`use_cooldown`): pearl 1 s per group; `use` refused during the cooldown.
- Rocket: on the ground, placed at 0.15 from the clicked point; when gliding, attached to the player. Duration
  `10 × (1 + flight) + rand(6) + rand(7)`. Free: ×1.15 horizontal (×1 against a wall) and
  +0.04 vertical per tick; explosion: 5 + 2 × stars, range 5, direct line of sight.

## 4. Specific impacts

Snowball: 0 damage (3 to a blaze), event 3. Egg: 1/8 a baby chick (1/32: 4), egg
variant. Pearl: teleports the player to the position at the start of the tick (rotation kept,
zero velocity), fall forgotten, 5 "pearl" damage, 5% endermite (difficulty > peaceful); the
player's pearls are saved in their file (`ender_pearls`, dimension recorded). Potion:
water = 1 damage to sensitive mobs, extinguishing, fire put out around the impact; splash: box
inflated (4, 2, 4), scale `1 − √d/4`, effects under 20 ticks ignored; lingering: cloud
(radius 3, −0.5 per victim, −radius/600 per tick, wait 10, effects every 5 ticks,
instant effects at 0.5, reapplication 20). Experience bottle: 3 + rand(5) + rand(5) experience, orbs thrown
along the normal of the face hit.

Reactions of the blocks hit: target (power depending on the distance to the face's center, 20 ticks
for an arrow, 8 otherwise), TNT primed by a burning projectile, bell, chorus flower and decorated
pot broken, pointed dripstone broken by a fast trident, campfire and candles lit.

## 5. Dispenser

0.7 in front of the center, +0.1 in height; shot along the axis, power 1.1, inaccuracy 6 (potions
and experience bottles ×1.25 / ×0.5); pickable arrows; rocket just outside the block (0.5 + 1e-5),
power 0.5; fire charge as a small fireball (inertia 0.95, acceleration 0.1).
Events: 1002 (1004 rocket, 1018 fireball), then 2000 (smoke).

## 6. Fishing

Cast: from the eyes offset by 0.3, velocity `(-sin, clamp(-sin/cos pitch), -cos) ×
(0.6/|v| + triangle(0.5, 0.0103365))`. States: flight (hooks an entity or an item on the ground),
in water → floating (×0.3, ×0.2, ×0.3), hooked. Floating: restoring force towards the
surface, wait `100..600 − lure × 100`, approach `20..80` (wake particles), bite
`20..40` ticks. Fishing speed +1 in rain (25%), −1 without sky (50%). Open water:
five 5×5 layers (source water then air, no block or lily pad). Draw: `gameplay/fishing` table
(named random sequence of the world), luck = sea luck + player luck; treasure
only in open water; each item receives a velocity towards the player and an orb
`rand(6) + 1`. Wear: 5 (entity), 3 (item), 1 (fish), 2 (bobber on the ground). Abandoned if the
rod leaves the hands or beyond 32 blocks.

## 7. Maps

Data: `data/minecraft/maps/<n>.dat`, last number `maps/last_id.dat`. Blank map:
center aligned on a grid of 128 × 2^scale, next number, cartography table pickup sound.
Scan (map held, not locked, same dimension): one column in 16 per tick plus the
neighbors of a change; radius 128/2^scale (half under a ceiling); most frequent color
of each cell (first seen on a tie), `WORLD_SURFACE` height recomputed (the heightmaps
of modified chunks are not tracked), water depth, relief compared to the previous
cell, checkerboard at the border. Markers: player (off the map up to 320 cells, out of limits
if unlimited tracking), clicked banners, targets of explorer maps. Packet: modified
rectangle, markers at most one time in 5. Cartography table: paper = enlarge, glass pane =
lock, blank map = clone (2); enlarging also at the crafting table (8 papers); the
post-processing creates a new map. Exploration (loot, trading): nearest structure
of the tag (radius 50 chunks), map tracked without limit, preview of the water biomes, "+" target.

## 8. Checks (2026-10-13, local official server `server-s8`)

- 8 projectiles × 60 ticks summoned while frozen then advanced one tick at a time (`tick freeze/step`):
  positions and velocities bit-for-bit identical (`tests_s8::trajectories_trace`), health of the pig
  hit identical (7).
- Fishing loot: 40 `loot … fish` draws identical (`fishing_loot_sequence`,
  `fishing_loot_offline`); named sequence identical to `random value`.
- End to end: bow, crossbow, trident, snowball, dispenser, blank map, comparator in front of
  a sealed chest.

## Single projectile engine (S8 × mobs merge, 2026-10-16)

A single engine (`rc-sim/src/projectiles/`) makes all the projectiles fly, hit, be tracked on the network and
be saved, whether they come from the player, a dispenser or a mob.
Mobs keep only their shooting gesture:

- Skeletons, stray, bogged, parched, wither skeleton (`mobs/arrows.rs`, common bow shot):
  arrow created at the mob's eyes − 0.1, ammunition held (off-hand then main hand,
  among the arrows) otherwise plain arrow, weapon worn (Power, Punch, Flame, piercing),
  base damage `2 × power + triangle(0.11 × difficulty, 0.57425)` drawn on the arrow's
  random source, effect added to the stack for the variants (slowness 600, poison 100, weakness 600;
  color 11 recomputed), fire 2,000 ticks for the wither skeleton; pickup forbidden.
- Pillagers' crossbow (`mobs/illagers/crossbow.rs`): loading of the held ammunition (arrows,
  rockets) with intangible copies of the multishot, shot of each loaded projectile towards the third
  of the target's height in the multishot fan (aimed vector rotated around its
  perpendicular), "crossbow" sound on impact, crossbow wear (rocket 3, otherwise 1).
- Drowned tridents (`mobs/aquatic/trident.rs`), snow golem snowballs
  (`mobs/golems/snowball.rs`), witch potions (`mobs/illagers/witch.rs`): common
  projectile with the mob as author. The witch hit by her own damage potion takes
  0 (the hit counts).
- Fireballs (`mobs/fireballs.rs`): small (blaze) and large (ghast, kind `LargeFireball`,
  `ExplosionPower` saved) in the common engine; "fireball" or "unattributed" damage,
  explosion of the large ball (fire depending on `mob_griefing`), deflection by a player hit (large
  ball only), 1,000 damage to the ghast when the player deflected it. A removed author is no
  longer resolved: the ball continues (the game's removal test never fires).
- Wither skulls (F16, `mobs/wither/skull.rs`, kind `WitherSkull`): same flight as the balls
  (inertia 0.73 for a blue skull out of water), never on fire, not deflectable, without an item;
  NBT `dangerous`; hit 8 `wither_skull` (5 `magic` without author) and wither effect, then explosion
  of power 1 (blue skull: resistance of destructible blocks ≤ 0.8). Detail: `server-mobs.md`
  § 4.F16.
- The mob shooter receives its target as "last mob hurt" before the arrow's
  damage.
- F14: wind charges (player's `wind_charge`, breeze's `breeze_wind_charge`) in this same
  engine, like balls without fire or item, inertia 1 and box lowered by 0.15; burst on
  impact (`server-mobs.md` § 4.F14). Any mob of the `deflects_projectiles` tag (the
  breeze) deflects the other projectiles before impact (arrows included), once per mob.
