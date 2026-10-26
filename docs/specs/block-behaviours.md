# Server-side block behaviours (Rust singleplayer, `rc-sim`)

Behaviour specification, clean room. Target version: Java Edition 26.3 (protocol 777).
Context: `docs/architecture.md` § 14 (the integrated server recodes block behaviours).
Cross-references: fluid rendering and reading their state → `fluids.md`; light propagation
(client engine already checked against the server, reusable as is on the server side) → `lighting.md`;
client-side block entities → `block-entities.md`.

Conventions:
- Directions: down (−Y), up (+Y), north (−Z), south (+Z), west (−X), east (+X). "Horizontals in
  plan order" = north, east, south, west.
- Block ids (`minecraft:…`), state properties (`power`, `age`…), game rules
  (`randomTickSpeed`…) and game constants are given as is: they are data, not code.
- "scheduled tick" = delayed update of N ticks requested by a block; "random tick" =
  update drawn at random by the world; "block event" = action deferred to the end of the
  tick phase (pistons, chests, bells…) and broadcast to the clients.
- `R` = the world's main random generator (algorithm `JavaRandom`/Xoroshiro depending on the case,
  see § 1.9). The random draws are described in the order in which the game makes them: changing that order changes the
  results (test seeds, compared traces).

---

## 1. World tick engine

### 1.1 Server tick (all worlds)

A server tick (50 ms nominal) does, in this order:
1. server functions, if the game is running normally: `#minecraft:load` once right after a
   data (re)load, then `#minecraft:tick` (`/schedule` functions run in
   the world tick, § 1.2 step 6);
2. clocks (time of day); time synchronization to players every 20 ticks;
3. **each world in registration order**: surface (`overworld`), Nether, End, then the
   others; a world is ticked entirely (§ 1.2) before the next one;
4. network (packets received from players are processed here: a button click that arrived during tick N
   is applied at the end of tick N, so its scheduled effects start from that position in the tick);
5. player list (latency, etc.).

Freeze/step (`/tick freeze`, `/tick step`): if the world "is not running normally", time,
weather, scheduled ticks, raids, chunk ticks, block events and block entities are skipped; frozen
entities do not tick (players do).

### 1.2 World tick: phase order

1. invalidation of the environment attribute caches;
2. **world border** (interpolation);
3. **weather** (rain/thunder cycle, counters; see `weather-border.md` for the visual effect);
4. sleeping: if enough players are deeply asleep (`playersSleepingPercentage`), jump to
   the wake-up time (if `advance_time`), wake everyone, and end the rain (if `advance_weather`);
5. sky brightness recomputed (darkening: used by daylight sensors, by
   growth, by spawning);
6. **time**: `game_time += 1` then execution of the events scheduled by `/schedule` that fall due;
7. **scheduled block ticks** (at most 65,536 per tick), then **scheduled fluid ticks**
   (at most 65,536) — § 1.3. None of this in a "debug" world;
8. **raids**;
9. **chunk source** (§ 1.4): ticket purge, then for the chunks: counting of
   mobs, spawning chunks shuffled at random → lightning + natural spawning per chunk;
   then **random ticks** of each chunk that ticks (§ 1.5); then special spawns
   (patrols, cats, wandering trader…); then sending the modified blocks to the clients;
10. **block events** (§ 1.6);
11. if the world has not been empty for ≥ 300 ticks (without an active ticket):
    a. dragon fight (End);
    b. **entities** in the order of the list of ticking entities (insertion order), passengers
       after their mount;
    c. **ticking block entities** (§ 1.7);
12. entity management (loading/unloading of entity sections).

Fidelity consequence: within one tick, scheduled ticks run BEFORE random ticks,
which run BEFORE block events (pistons), which run BEFORE entities and then
block entities (hoppers, furnaces, moving pistons). A machine that depends on this order (e.g.
a hopper that reads a block pushed in the same tick) must behave the same way.

### 1.3 Scheduled ticks (blocks and fluids)

Two independent queues with the same mechanics: one for blocks, one for fluids.

A scheduled tick carries: type (block or fluid), position, **due tick**
(`game_time + delay`), **priority** (integer from −3 = highest to +3, default 0) and **sub-order**
(global world counter, incremented on every tick creation, never reset).

Rules:
- **Uniqueness**: two ticks of the same type cannot be scheduled at the same position. A request
  while a (type, position) tick is already pending is ignored (the first one keeps its due tick
  and its priority). "Already pending" = still in the chunk's queue; a tick being executed
  this tick is no longer part of it, so a block can reschedule itself during its own tick.
- **Per-chunk storage**: each chunk has its queue sorted by (due tick, priority, sub-order). The
  world remembers, per chunk, the due tick of its head.
- **Collection at tick T**:
  1. take all chunks whose head falls due at ≤ T **and** that are "within block tick range"
     (fully loaded and ticking chunks); the others stay pending (their late ticks
     will run later, with their original due tick);
  2. drain these chunks into a common queue in **(priority, sub-order)** order only —
     the due tick no longer matters once ≤ T (a tick 3 ticks late at priority 0 runs
     after a tick of the day at priority −1); the chunks are merged like a heap sort on their
     head;
  3. stop at 65,536 collected ticks; the remaining ones wait for the next tick.
- **Execution**: in the collected order. For each (position, type): if the block (resp. the
  fluid) present is still of the same type, its scheduled-tick behaviour is called; otherwise
  nothing. Ticks scheduled during execution (delay ≥ 1) go into the chunk queues and
  do not run this tick. Delay 0: due tick = T, collection already done → runs on the next tick.
- Query "will this tick run this tick?": true if (type, position) is in the collected
  list and not yet executed (used by repeaters and comparators).
- Save: remaining delay (`due tick − T`) and priority; on load, the sub-order is
  reassigned in reading order.

Priorities used (the other blocks have 0):

| Source | Priority |
|---|---|
| repeater whose output points at another diode (repeater/comparator) not facing it | −3 |
| repeater turning off (was on) | −2 |
| repeater other cases | −1 |
| comparator pointing at a diode not facing it | −1 |
| comparator other cases | 0 |

### 1.4 Ticking chunks

- Scheduled ticks, random ticks, block events and block entities run only
  in the chunks "within block tick range" (ticket level ≤ 32: full chunk whose 8
  neighbors are loaded). Entities require the "entities" range (level ≤ 31).
- Random ticks: the ticking chunks are walked in the order of the chunk table
  (not shuffled). Spawning and lightning, on the other hand, use a list **shuffled** by `R`.
- Lightning (thunderstorm): per spawning chunk within entity range, 1 chance in 100,000 per tick; target:
  `motion_blocking` height of a drawn position (§ 1.5), redirected to a lightning rod at the top
  within a radius of 128, otherwise to a living entity seeing the sky in a column widened by 3;
  skeleton horse trap depending on the difficulty (not if the block below is a lightning rod).

### 1.5 Random ticks

For each ticking chunk, with `n = randomTickSpeed` (default 3, rule `random_tick_speed`):
1. **Snow and ice**: `n` times, with probability 1/48 (`R.next_int(48) == 0`), handle the
   precipitation of a drawn column (§ 2.7).
2. **Sections**: if `n > 0`, for each section from bottom to top, if it contains at least one
   "random-ticking" block or fluid (counter kept by the section), draw **`n` positions**
   in the section; for each one: if the block state ticks randomly → its random tick;
   then if its fluid state ticks randomly → random tick of the fluid (lava: fire
   spreading; water: nothing except special cases).

Drawing a position (LCG generator dedicated to the world, distinct from `R`):
`v = v * 3 + 1013904223` (32-bit signed integer, overflow); `w = v >> 2` (arithmetic);
`x = min_x + (w & 15)`, `z = min_z + (w >> 8 & 15)`, `y = min_y + (w >> 16 & 15)` (for the
precipitation column, `y` does not matter). Initial value of `v`: random at startup.
The same draw is used for lightning (§ 1.4). The same position can come out twice.

With n = 3: each block of an active section has ~3/4096 chance per tick of being drawn, i.e. on
average once every ~68 s.

### 1.6 Block events

- **Ordered queue without duplicates** (same position, same block, same two parameters = a single
  event). Appended at the end.
- Execution in phase 10: remove from the head until the queue is empty, including the events
  added during execution (a piston that triggers another in the same phase). If the chunk
  is not within tick range, the event is put back for the next tick.
- An event runs only if the block present is still the event's block; if it
  returns "done", it is broadcast to the players within 64 blocks (block event packet:
  animation of pistons, chests, bells, enchanting table cries…).
- Users: pistons (§ 3.9), chests/barrels/Shulker boxes (opening), bells,
  mob spawners, End portals (radius), note blocks.

### 1.7 Ticking block entities

- Global list in insertion order; new tickers added during a tick wait for the
  start of the next walk; removed tickers are dropped along the way.
- A ticker ticks only if its chunk is within block tick range.
- Ticking on the server side: furnaces (3 kinds), brewing stands, hoppers (§ 3.17), beacons, conduits,
  bells, spawners and trial spawners, vaults (`vault`), moving pistons
  (§ 3.9), campfires, beehives (§ 2.12), regular and calibrated sculk sensors, shriekers,
  catalysts (§ 3.15, § 2.12), End gateways, jukebox (if a disc), signs (player
  editing), Shulker boxes (opening), heads (powered animation), creaking heart,
  crafter (display delay), daylight detectors (only in a dimension with
  sky light, § 3.14), `potent_sulfur`. Do NOT tick on the server side: chests and Ender
  chests (client animation), enchanting tables, barrels. Details per block entity:
  `block-entities.md` + § 2/3 here.

### 1.8 Neighbor and shape updates

Two kinds of notifications when a block changes:
- **shape update** ("the neighbor in direction D became S"): the notified block can
  return a new state (connections of fences, walls, stairs, redstone dust, two-half
  doors, plants that no longer hold → air). Synchronous in spirit: it can destroy
  or change the notified block, which in turn notifies its neighbors (limited depth, see below);
- **neighbor changed** (BUD, "a block next to me changed, here is its type"): the notified block re-reads its
  surroundings (redstone, rails, pistons, sand that must fall…). No reliable direction
  information is passed (except in an experimental mode, ignored here).

Placing a state `S` at position `P` with **flags** (bits):

| Bit | Value | Meaning |
|---|---|---|
| neighbors | 1 | send "neighbor changed" to the 6 neighbors + comparator update |
| clients | 2 | send the block to the players |
| invisible | 4 | (client only) no re-render |
| immediate | 8 | (client) immediate re-render |
| known shape | 16 | do not send shape updates |
| no drops | 32 | destruction by shape drops nothing |
| moved by piston | 64 | passed on to "removed" and "placed" |
| no shape on dust | 128 | shape updates ignore redstone dust |
| no block entity effects | 256 | no ejection of the contents, etc. |
| no "placed" | 512 | do not call the "placed" behaviour |

Common uses: 3 (neighbors + clients) for most changes; 2 for a change
without BUD (dust that recomputes itself, the caller notifies by itself); 18 (2|16) for pistons.

Course of a placement (server):
1. if the state does not change → nothing at all (no notification);
2. write into the section; height maps; light (if light properties differ,
   recompute queue → `lighting.md`);
3. if the **block** changes (not only the state) and the old one had a block entity that is not kept:
   removal effects (ejection of the contents of containers, except bit 256) then removal of the
   block entity;
4. if the block changes (or if the new block is a rail) and (bit 1 or bit 64): **removal effects of
   the old one** ("old block removed, warn the neighbors": redstone, levers,
   buttons, torches, observers… send their neighbor updates from there);
5. if the written block was replaced in the meantime → end;
6. unless bit 512: **"placed"** behaviour of the new state, called on **every
   state change**, even if the block stays the same (each block itself tests "the old one was
   another block" when needed): dust computes itself, fluids schedule themselves, sand
   schedules its fall, fire schedules its tick, diodes warn what is ahead…;
7. creation of the block entity if needed;
8. bit 2 and ticking chunk: marked to be sent to the clients;
9. bit 1: **"neighbor changed"** to the 6 neighbors in the order **west, east, down, up, north, south**
   (type passed = the OLD block), then if the new state has an analog output, update
   of the comparators (§ 3.5);
10. unless bit 16, and if the remaining depth > 0: shape updates, with the flags
    `flags & ~(1|32)` and depth −1:
    a. "indirect" shapes of the old state (only redstone dust has any: its
       diagonals, § 3.3);
    b. shapes of the 6 neighbors in the order **west, east, north, south, down, up**;
    c. "indirect" shapes of the new state;
11. update of the points of interest (beds, bells, workstations, beehives, portals…).

Applying a shape update received by `Q`: if the flags contain 128 and `Q`
is redstone dust → ignored. Otherwise new state = the block's answer; if it differs:
air → destruction of `Q` (with drops unless bit 32, remaining depth); otherwise placement of the state with
`flags & ~32` and the remaining depth.

Depth of shape updates: 512 by default at placement, decremented at each level; at 0
shapes no longer propagate (this is what bounds a chain of destroyed bamboo/vines).

#### Notification scheduler (critical fidelity)

All notifications (plain neighbor changed, "full" neighbor changed with a supplied state, series
of 6 neighbors, shape update) go through a **single scheduler per world**:
- Counter `c` of the notifications of the current chain, stack `S` of jobs, list `A` of the
  jobs added during the current step.
- Adding: if `c ≥ limit` (server rule `max-chained-neighbor-updates`, default **1,000,000**;
  negative = unlimited) → the notification is **dropped** (one log line on the first rejection);
  otherwise, if a chain is already running → appended to the end of `A`, else pushed onto `S`. `c += 1` in
  all cases. If no chain was running, execution starts **immediately** (the caller
  waits for the end of the whole chain).
- Execution: while `S` or `A` is not empty: push the elements of `A` **in reverse
  order** (the first one added ends up on top), empty `A`; take the top without
  removing it; make it do **one step** at a time as long as `A` stays empty; when it is done, pop
  it. As soon as a step adds jobs, they are processed before the next step of the current
  job.
- A "6 neighbors" series does one step per direction (west, east, down, up, north, south, skipping
  the excluded direction); a step re-reads the neighbor's state at the time of the step.
- End of chain: `c = 0`, stacks emptied.

Consequences: **depth-first** traversal; at the root level (outside a chain), a placement with bit 1
finishes the whole BUD cascade of its 6 neighbors BEFORE doing its shape updates; inside
a chain, the shape updates and BUDs of a single placement are queued
then executed in the order of addition, each one depth-first. Placements made during a step
(e.g. dust that lights up) do not run their notifications on the spot but at the end of
the step. The Rust code must reproduce exactly this scheme (no direct recursion, no breadth-first
queue).

### 1.9 Randomness

- `R` (world randomness): used for the blocks' random ticks, the shuffling of chunks, lightning,
  random delays (fire, vines…). The generator of the random-tick positions is
  the LCG of § 1.5. Blocks receive `R` as an argument of their ticks.
- Shape updates also receive the world randomness (fire that reschedules its tick).
- Reproducing the exact sequence of `R` is only useful for compared traces; the mechanics
  (probabilities, order of the draws within a single behaviour) must always be reproduced.

## 2. Behaviours by family

Notation: "RT" = random tick (§ 1.5), "ST(n)" = tick scheduled in n ticks (§ 1.3),
"shape" = reaction to a shape update (§ 1.8), "BUD" = reaction to "neighbor changed".
Light: `raw(P)` = max(sky light, block light) at P without darkening;
`local(P)` = max(sky − sky darkening, block) (day/night value); `block(P)` = block
light alone. Propagation and values: `lighting.md`.

### 2.1 Fluids (server-side flow)

Data: a fluid is `empty`, `water`/`lava` **source** (amount 8) or **flowing** (`level`
1..8 = amount, `falling`). Water/lava block: property `level` 0..15 = 0 for a source,
`8 − amount` for a flowing one, +8 if `falling` (see `fluids.md` § 0 for the inverse reading).
Waterlogged blocks: fluid = source water if `waterlogged`.

Constants per fluid:

| | Water | Lava (surface) | Lava (Nether: dimension attribute "fast lava") |
|---|---|---|---|
| loss per cell ("loss") | 1 | 2 | 1 |
| slope search distance | 4 | 2 | 4 |
| tick delay | 5 | 30 | 10 |
| conversion to source (2 neighboring sources) | rule `water_source_conversion` | rule `lava_source_conversion` | same |

**Scheduling**: the liquid block schedules an ST(delay) of its fluid on placement (if the
water/lava contact did not transform it), on every BUD (same), and on every shape update
if it or the neighbor is a source. Waterlogged blocks schedule a water tick on
every shape update.

**Water-lava contact (in the lava block, on placement and on BUD)**: for the directions down,
south, north, east, west (in that order): if the cell **on the opposite side** contains water → the lava
becomes `obsidian` (source) or `cobblestone` (flowing), hissing (1501), end; if the lava is
above `soul_soil` and that cell is `blue_ice` → `basalt`. (`stone`
appears when lava flows **downward** into water: see "spread".)

**New fluid computed at P** (`compute(P)`):
1. for the 4 horizontals (north, east, south, west): neighbor of the same fluid and passage possible
   between the two shapes (§ below) → count the sources, keep the max amount;
2. ≥ 2 sources and conversion allowed and (block below "legacy solid" or source of the same
   fluid) → **source**;
3. otherwise, fluid of the same type above with passage → flowing amount 8, `falling`;
4. otherwise `max − loss`; ≤ 0 → empty, otherwise flowing (not falling).

**Passage between two cells** (A → B in direction d): false if one of the two collision
shapes is a full cube; true if both are empty; otherwise true if the merged faces
in direction d do not hide each other (same face occlusion test as rendering).

**Fluid tick at P**:
1. if not a source: `n = compute(P)`; delay = the fluid's delay (lava: ×4 with probability 3/4
   if neither the old nor the new one is falling and the new one is higher — `R` draw);
   empty → place air (flag 3); different → place the new block (flag 3) and ST(delay);
2. **spread** (with the possibly updated state):
   - downward if the cell below can receive it (not a source of the same type, block replaceable
     by a fluid — air, tag `washed_away_by_fluids`, or a block able to hold the fluid —, passage
     possible) and the fluid that would go there (`compute(below)`) can replace the existing one: write
     it there; then, if P has ≥ 3 horizontally neighboring sources, also spread to the sides; end;
   - otherwise, if P is a source or if the cell below is not a "hole": sides;
   - "hole" = downward passage possible and (below is the same fluid or can hold it).
3. **Sides**: if `amount − loss > 0` (or if P is falling): for each horizontal where the neighbor
   can receive: new fluid = `compute(neighbor)`; if it can fit there: distance = 0 if the
   neighbor is a hole, otherwise slope search; keep **all** the directions at minimal
   distance (a shorter distance empties the list) and whose present fluid accepts being
   replaced. Applied in the order **north, south, west, east** (the game's direction order).
4. **Slope search** from the cell C (depth p, coming from direction f): for each
   horizontal ≠ f where passage is possible and the cell can hold the flowing shape: hole →
   returns p; otherwise if p < max distance → recursion (p+1). Return = minimum (1000 if nothing).
   The states read and the holes are cached per relative (x, z) during a single computation.
5. **Writing** a fluid into a cell: block able to hold a fluid (waterloggable) → waterlog it
   (water only); otherwise: if non-air, destruction effects (water: drops of the block swept away;
   lava: hissing 1501) then placement of the liquid block (flag 3). Lava **downward into
   water**: if it is a water liquid block → `stone`; hissing; no write.
- Allowed replacements: water can be replaced only **from above** and by a fluid
  other than water; lava can be replaced by water if its height ≥ 4/9 (0.44444445).

**Random tick of lava** (only if a player is close enough: rule
`fire_spread_radius_around_player`, −1 = everywhere): `k = R.next_int(3)`; if k > 0: k steps, each
offsetting by (rand(3)−1, +1, rand(3)−1): air cell with a neighbor flammable by lava → fire,
end; non-empty cell of tag `blocks_lava_fire_spread` → end; other non-empty cell → keep going
up; otherwise (k = 0): 3 tries
at (rand(3)−1, 0, rand(3)−1): if the cell above is empty and the cell is flammable by lava → fire
above.

**Bubble columns**: a full source water above `soul_sand`/`magma_block` (tags)
schedules an ST(20) of its block to form the column (upward: soul sand; downward:
magma). Details: Uncertain points.

Evaporation: a water bucket or the melting of ice in a dimension where the attribute
"water evaporates" is true (Nether) yields air.

### 2.2 Gravity

| Block | Delay | Notes |
|---|---|---|
| `sand`, `red_sand`, `gravel` | 2 | — |
| `suspicious_sand`, `suspicious_gravel` | 2 | do not fall, become sand/gravel when falling if not brushed; separate (block entity) |
| concrete powder (16 colors) | 2 | becomes concrete if it touches water (see below) |
| `anvil`, `chipped_anvil`, `damaged_anvil` | 2 | damage to entities, may get damaged on landing |
| `dragon_egg` | 5 | teleports when clicked |
| `scaffolding`, `pointed_dripstone` | — | own logic (§ 2.12) |

- Placement and any shape update → ST(delay). Tick: if the block below is "free" (air, fire,
  liquid, replaceable block) and y ≥ min y → becomes a "falling block" entity (`entities.md`).
- Concrete powder: solidifies on placement if the replaced cell contains water, or if one of the
  directions (down excluded unless P itself contains water) has water whose face toward P is
  not solid; same when touching while falling and on any shape update.

### 2.3 Fire

- `fire`: ST(30 + R.next_int(10)) on placement and on every tick. Tick:
  1. reschedule (30..39); if no player is close enough (rule above) → nothing else;
  2. no longer held (support) → removed;
  3. block below is the dimension's `infiniburn` (netherrack, magma; bedrock in the End) → eternal
     fire (no extinguishing by rain nor by aging);
  4. otherwise, raining and rain at P or at one of its 4 horizontal neighbors and `R.next_float() <
     0.2 + age × 0.03` → extinguished;
  5. age += `R.next_int(3) / 2` (≤ 15), set with flag 260 (2|256) if it changes;
  6. (not eternal) no flammable block around → removed if the block below is not solid on top or
     age > 3; age 15 and `R.next_int(4) == 0` and block below not flammable → removed;
  7. **burning** of the 6 neighbors in the order east, west, down, up, north, south with chance 300
     (sides) / 250 (up and down), −50 if the attribute "accelerated extinguishing" (humid biomes):
     if `R.next_int(chance) < burnability`: if `R.next_int(age + 10) < 5` and no rain on the
     cell → the cell becomes fire (age + R.next_int(5)/4) otherwise it is removed; TNT is
     primed;
  8. **spread**: for x ∈ −1..1, z ∈ −1..1, y ∈ −1..4 (loop x, then z, then y) excluding P:
     `rate = 100 + (y > 1 ? (y − 1) × 100 : 0)`; air cell with `flammability` > 0 (max of the 6
     neighbors): `c = (flammability + 40 + difficulty × 7) / (age + 30)` (÷ 2 if accelerated
     extinguishing); if `c > 0` and `R.next_int(rate) ≤ c` and no rain near the cell → fire
     of age `min(15, age + R.next_int(5)/4)`.
- `soul_fire`: on soul sand/soil, does not spread; disappears if the support changes.
- Table (flammability / burnability); waterlogged blocks are 0/0:

| Values | Blocks |
|---|---|
| 5 / 5 | logs, wood, stripped (all wood types including poplar), `bamboo_block` (and stripped), `coal_block` |
| 5 / 20 | planks, slabs, stairs, fences, wooden fence gates, `bamboo_mosaic` (+ slab, stairs), `mangrove_roots`, `composter`, `beehive` |
| 30 / 20 | `bookshelf`, `lectern`, `bee_nest`, shelves (`*_shelf`) |
| 30 / 60 | leaves (all, including azalea and red/orange/yellow poplars), `azalea`, `flowering_azalea`, `dried_kelp_block`, `hanging_roots`, `straw_bed`, wool, wool stairs and slabs |
| 60 / 20 | `hay_block`, carpets |
| 60 / 60 | `bamboo`, `scaffolding` |
| 60 / 100 | grasses, ferns, flowers (single and double), dead/dry bushes, `pink_petals`, `wildflowers`, `leaf_litter`, `cactus_flower`, `sweet_berry_bush`, `spore_blossom`, big and small dripleaf, `firefly_bush`, `bush` |
| 15 / 100 | `tnt`, `vine`, `glow_lichen` |
| 15 / 60 | `cave_vines` (+ plant) |
| 15 / 20 | `target` |
| 5 / 100 | `pale_moss_block`, `pale_moss_carpet`, `pale_hanging_moss` |

### 2.4 Crops and plants

"Crop" growth (RT): if `raw(P) ≥ 9` and age < max: `v = speed(P)`;
`R.next_int(floor(25 / v) + 1) == 0` → age + 1 (flag 2).
`speed(P)` = 1 + sum over the 9 cells under the crop and around it (−1..1 in x, z): 1 if the block
is of tag `grows_crops` (farmland), 3 if it is moist (`moisture > 0`), divided by 4 away from the
center; then ÷ 2 if the same crop is present on both the X axis and the Z axis, or
(otherwise) diagonally. Crop survival: `raw ≥ 8` and tilled support.

| Block | Max age | Tick | Bone meal |
|---|---|---|---|
| `wheat`, `carrots`, `potatoes` | 7 | crop | +2..5 |
| `beetroots` | 3 | crop with an extra 1 in 3 chance of doing nothing | (2..5)/3 |
| `torchflower_crop` | 1 → becomes `torchflower` beyond | crop, 1 in 3 chance of nothing | +1 |
| `pitcher_crop` | 4 (two blocks from 3) | crop (same draw) | +1 |
| `pumpkin_stem`, `melon_stem` | 7 | crop; at 7: random horizontal direction, if air and the block below is of the support tag → fruit + attached stem turned toward it (flag 3) | +2..5 |
| `nether_wart` | 3 | `R.next_int(10) == 0` | no |
| `cocoa` | 2 | `R.next_int(5) == 0` (world random) | +1 |
| `sweet_berry_bush` | 3 | `R.next_int(5) == 0` and `raw(above) ≥ 9` | +1; harvest at 2-3 |
| saplings (`*_sapling`, `mangrove_propagule`) | `stage` 0..1 | `local(above) ≥ 9` and `R.next_int(7) == 0`: stage 0 → 1 (flag 260), otherwise tree (worldgen feature) | 45% success |
| `bamboo_sapling` | — | 1/3, above empty, `raw(above) ≥ 9` → bamboo | yes |
| `bamboo` | height 16 | `stage == 0`: 1/3, above empty, `raw(above) ≥ 9`, height < 16 → grows (leaves: small/large, `stage` 1 at height 15 or with 25% beyond 10) | yes |
| `sugar_cane` | `age` 0..15 | above empty and height < 3: age 15 → new block above and age 0, otherwise age + 1 (flag 260) | no |
| `cactus` | `age` 0..15 | above empty; height ≥ 3 and age 15 → nothing; age 8 and the cell above can carry a cactus → cactus flower (25% if height ≥ 3, otherwise 10%); age 15 and height < 3 → new cactus above, age 0, BUD to the new one; otherwise age + 1 | no |
| `kelp` | `age` 0..25 | plant that grows upward, 14% per RT in water | yes |
| `weeping_vines`, `twisting_vines`, `cave_vines` | 0..25 | 10% per RT, downward / upward / downward | yes |
| `chorus_flower` | 0..5 | branching growth (ends at 5) | no |
| `budding_amethyst` | — | 1/5: random direction (6): air/water → small bud; bud facing it → next stage | no |
| `brown_mushroom`, `red_mushroom` | — | 1/25: if ≤ 4 of the same mushroom within −4..4 × −1..1 × −4..4 → random walk of 4 steps, place with flag 2 on an empty cell where it survives | huge mushroom |
| `open_eyeblossom`/`closed_eyeblossom` | — | opens/closes depending on the environment attribute (night); propagates to identical flowers within ±3 × ±2 × ±3 with an ST(distance×5..distance×10) | — |
| `dried_ghast` | hydration 0..3 | waterlogged or hydrated → ST(5000) to advance | — |

Survival (shape downward or ST(1) then destruction): sugar cane (cane below, or block of the
support tag with water/frosted ice next to the support block); cactus (no horizontal neighbor
that is "legacy solid" nor lava, cactus or sand below, no liquid above); mushrooms
(`raw < 13` and opaque block below, or block below of the mycelium/podzol/nylium tag); cocoa (jungle
log on the `facing` side).

### 2.5 Grass, mycelium, nylium

- `grass_block`, `mycelium` (RT): **survival**: the block above is a one-layer snow, or is not
  a full fluid and lets light through downward (face attenuation < 15); otherwise →
  `dirt`. **Spread** if `local(above) ≥ 9`: 4 tries at (rand(3)−1, rand(5)−3, rand(3)−1):
  `dirt` cell that would survive and with no water above → grass/mycelium (`snowy` if snow above).
- `crimson_nylium`, `warped_nylium` (RT): if the block above blocks light (same test) →
  `netherrack`.

### 2.6 Leaves

- `distance` 1..7, `persistent`. Shape update: if (neighbor distance + 1) ≠ 1 or if the
  stored distance ≠ (neighbor distance + 1) → ST(1). Tick → `distance` = min over the 6 neighbors
  (down, up, north, south, west, east, stopping at 1) of (neighboring distance + 1); neighbor of tag
  `prevents_nearby_leaf_decay` (logs) = 0; neighbor without a distance = 7; set with flag 3.
- RT only if `distance == 7` and not persistent: drops and removal.
- Waterlogged leaves: water tick on every shape update.

### 2.7 Ice, snow

- `ice` (RT): if `block(P) > 11 − attenuation` (ice's attenuation = 1) → melts: water (placed
  with flag 3 + BUD on the cell) or air if water evaporates. Broken without an enchanted tool above
  a solid or liquid block → water.
- `snow` (layers, RT): if `block(P) > 11` → drops and removal.
- `frosted_ice` (ST 20..40): if `R.next_int(3) == 0` or < 4 frosted ice neighbors: if the
  light (End: block only; elsewhere: `local`) > `11 − age − 1` → melts one step (age 0..3, then
  water) and reschedules the neighbors that have not melted; otherwise reschedule.
- **Formation** ("snow and ice" phase of § 1.5, drawn column, `motion_blocking` height =
  `H`):
  - biome too cold for rain (temperature < 0.15 at the position, corrected by the altitude
    above sea level + 17) and `block < 10` and cell `H−1` = **source** water in a liquid block not
    surrounded by water on all 4 sides → `ice`;
  - if it is raining: snow possible at `H` (snow precipitation, `block < 10`, air or snow, snow would
    hold there): layer + 1 up to `min(snow_accumulation_height, 8)` (rule
    `max_snow_accumulation_height`), otherwise a new layer; then the block at `H−1` receives the
    precipitation: empty cauldron → 5% water (rain) / 10% powder snow (snow); cauldron being
    filled → +1 level.

### 2.8 Farmland

- `farmland` `moisture` 0..7. RT: water within −4..4 × 0..1 × −4..4 or rain above → 7 (if
  < 7); otherwise `moisture − 1`, and at 0 with no crop on top (tag `maintains_farmland`) → dirt.
- Solid block placed above → ST(1) → dirt. Fall of a living entity:
  `R.next_float() < fall height − 0.5` and volume (width² × height) > 0.512 and (player outside
  spawn protection, or `mob_griefing`) → dirt (entities pushed upward).

### 2.9 Corals

- Living coral blocks, corals, fans (wall ones included): on every shape update,
  if there is no water in contact (at least one direction with water; for plants:
  being waterlogged) → ST(60 + R.next_int(40)); tick: still without water → dead version
  (flag 2, plant: not waterlogged).

### 2.10 Vines and climbing plants

- `vine` (rule `spread_vines`): RT with a 1 in 4 chance: random direction among the 6;
  free horizontal → spreads sideways (clinging to the left/right or going around the
  corner), upward → copies faces onto the cell above, downward → goes down keeping
  random faces. Limit: at most 4 vines within −4..4 × −1..1 × −4..4 in order to spread.
  Details of the draws: Uncertain points.
- `weeping_vines`, `twisting_vines`, `cave_vines` (+ `kelp`): growing head (table § 2.4); at
  age 25 it stops growing; breaking a head turns the plant below it into a head; glow
  berries (`berries`): 11% on growth.

### 2.11 Copper (oxidation)

- Unwaxed copper blocks (full, cut, stairs, slabs, doors, trapdoors, grates, bulbs,
  bars, chains, lanterns, lightning rods, chests, statues): RT with `R.next_float() <
  0.05688889` → `next`:
  1. walk the oxidizable blocks of the same type and age within a Manhattan radius of 4 (excluding P): a
     younger one → abort; count older (`o`) and same age (`s`);
  2. `c = (o + 1) / (o + s + 1)`; chance = `c² × (0.75 if unoxidized, otherwise 1)`;
     `R.next_float() < chance` → next stage (set with flag 3).
- Lightning on a lightning rod: random de-oxidation around it.

### 2.12 Miscellaneous

| Block | Behaviour |
|---|---|
| `pointed_dripstone` | RT: dripping toward a cauldron up to 11 blocks below (water 17.578125%, lava 5.859375%, depending on the fluid above the support block); growth of stalactites/stalagmites (water above); a tip without support falls |
| `scaffolding` | distance to the support 0..7 (horizontal), ST(1) on every shape; > 7 → falls |
| `turtle_egg` | RT: hatching (age 0..2) at night, on sand; walking on it may break it |
| `sniffer_egg` | ST: 24,000 ticks (12,000 on moss) ± 300, 3 stages |
| `frogspawn` | ST(3600..12,000) → 2..5 tadpoles |
| `redstone_ore` (+ deepslate) | `lit` on touch/click; RT → turns off |
| `nether_portal` | RT: may spawn a zombified piglin (difficulty) |
| `chorus_flower` | see § 2.4, RT |
| beehives, nests | block entity: bees leaving/entering, `honey_level` 0..5 (+1 per bee returning with pollen), exit by day, not in the rain; campfire below = calm bees |
| `sculk_catalyst` | on the death of a mob with XP within ≤ 8 blocks: spread charges that turn blocks into `sculk`/veins, grow sensors and shriekers (`sculk_shrieker`: 4 shrieks → warden) |
| `creaking_heart` | linked to a creaking; active at night (attribute), analog output (§ 3.5) |

Bone meal: each block says whether it is a valid target and the success chance;
details to be picked up with `interactions.md` (crops: +2..5, saplings 45%, grass: vegetation
around, mushrooms → huge if there is room, bamboo +1..2, kelp/vines: +1, etc.).

### 2.13 Blocks that follow their support

General rule (shape update): when the support block disappears or changes, the block
that no longer "holds" returns `air` → destroyed with drops (except bit 32). Concerns: torches,
levers, buttons, plates, rails (via BUD, § 3.10), dust, diodes (shape downward + BUD),
flowers, grasses, crops, saplings, carpets, snow (block below not full or ice), lanterns,
bells, doors (linked halves), beds (linked halves), double plants, corals, cactus and cane
(via ST(1): never destroyed during the shape update itself).

Rules per family (checked only from the indicated face, otherwise on every shape):
banner, cake, candle cake: "solid" block below (down); wall banner: solid block
behind; hanging sign: center support under the block above (up); amethysts, wall fans: full face of the
holder; hanging roots: full lower face of the block above; spore blossom: center support above and no
water; wither rose, azaleas, Nether roots and fungi, Nether sprouts, dry grasses: tag
`supports_*` of the block below (any shape); leaf litter: full top face below; lily pad,
frogspawn: water below (or tag) and no fluid in place; sea pickle: non-empty or full top face
below. **Candles: never removed by a shape update** (survival tested only on placement).
Living corals out of water (neither waterlogged nor water on any of the 6 faces): tick in 60 + R(40) on
placement and on every shape; on tick, still without water → dead variant (flag 2, orientation
kept). Vines: each side face holds on a full face (support or collision) or by
the same face of the vine above; no face left → air (shape except from below).
Lichen and multi-face blocks: a face without support disappears, no face left → air. Climbing plants
(weeping, twisting, cave vines): support = same plant or full face on the side opposite
to the growth; loss → tick 1 then destruction; head/body swap depending on the next cell
(a new head draws its age 0..24 at random).

Note block: instrument depending on the block above if it plays from above (heads), otherwise depending on
the block below (harp if the block below is a head); recomputed on every vertical shape.

### 2.14 Drops without a player, rain, bone meal, tools and right click (singleplayer, W8b)

**Drops without a player.** Every destruction "with drops" (lost support, sweeping water, crushing piston,
ravager, harvesting villager, silverfish, decaying leaves, `/setblock …
destroy`) rolls the loot table `blocks/<name>` with a present but empty tool and the block entity still
in place, places each item at the block's center (offset ±0.25, rule `block_drops`), then the
after-break effects: ore experience (without Silk Touch), silverfish of infested blocks. Event
2001 goes out before the drops, the removal after (`blocks/level.rs`
`drop_resources`). The piston crushes without event 2001 on the server side and emits `block_destroy`.

**Rain.** "It is raining" = dimension with weather (sky, no ceiling, not the End) and rain level
> 0.2. "It is raining on this block": in addition, sky visible, above the "blocks motion" map
and a biome with precipitation whose adjusted temperature is ≥ 0.15 (otherwise snow). The
adjusted temperature applies the "frozen" modifier (noises 3456 and 2345) and, above
sea level + 17, the altitude drop (noise 1234). A column precipitation (1 chance in 48 per
chunk per tick): freezing of the water under the top (biome of the top, temperature of the block below),
then under the rain snow that accumulates up to the rule `max_snow_accumulation_height` (one more
layer, or a new layer) and precipitation received by the block below: empty cauldron → 1 level
of water (5%) or powder snow (10%), cauldron of the same type not full → +1 (draw made even if full).
The sky darkening seen by the daylight detector follows rain and thunder.

**Bone meal.** Valid target → bone meal consumed (not in creative), vibration
`item_interact_finish`, event 1505 (15); the success draw can fail after the
consumption. Crops: age + 2..5 (beetroot ÷ 3, torchflower + 1, flower at age 2). Stems:
+ 2..5 up to 7, then an immediate random tick. Berries + 1, cocoa + 1. Short grass, fern:
double plant if it can stand and the cell above is free. Tall flowers, full petals: one
item dropped; otherwise petals + 1. Mushrooms: 40%, giant feature (block removed, put back
on failure). Moss: moss carpet. Grass: 128 scattered tries, tall grass (10%), biome
flower (12.5%, features of the tag `can_spawn_from_bone_meal`) or grass (`grass_bonemeal`).
Saplings: 45%, advance of the tree (stage 0 → 1, then tree). Tree: 2×2 first if the species has one and
four saplings form a square, otherwise a single tree, "flowering" (bees) with a flower
within ±2/±1/±2; saplings removed without update, put back on failure. The features are placed
in the live world by the generator (3×3 chunks), seed drawn from the world random.

**Tools.** Axe, hoe and shovel follow the data transformers `block_transformer`
(per-block rules, forbidden faces, property copying, interaction drops, wear 1); placed with
flags 11, sound and particles sent to the other players only (the player's client predicts
them), `block_change`. Shield in the offhand without sneaking: no
transformation. Honeycomb: inverse of the axe's "de-waxing" rules, honeycomb consumed.

**Right click on a block.** Item on the block first: TNT with flint and steel (primed, removed), whole
cake + candle, candle (or candle cake aimed above 0.5) lit, extinguished with an empty hand,
flower pot (plant placed; full pot: nothing), composter (layer drawn when below 7, event 1500),
cauldrons (buckets, bottles, dye/banner/Shulker washing). Then empty hand: slice of cake
(hunger < 20 or invulnerable; 2 and 0.4 saturation; bite 7 → block removed), candle cake
(slice + candle dropped), plant taken back from the pot, bone meal from the full composter, bell (face on the
right side depending on the attachment, below 0.8124), dragon egg (1,000 tries within ±15/±7/±15).
Sponge: breadth-first walk (depth 6, 65 cells) over the water cells, then wet sponge.
Grass, podzol, mycelium: `snowy` depending on the `snow` tag of the block above. Heads: standing ones
turned like the gaze, wall ones in front of a non-replaceable block; banners standing on a
solid block, wall ones against a solid block. Pressure plates: press and release click
of the block type, `block_activate` / `block_deactivate`.


## 3. Redstone

### 3.1 Power model

Each block state exposes four functions (values 0..15):
- **source?**: the block emits by itself;
- **weak signal** `weak(Q, d)`: power that the block at `Q` gives to the neighbor located at `Q − d`
  (`d` = direction **from the receiver to the source**);
- **strong signal** `strong(Q, d)`: same convention, but this signal "charges" the receiving
  conductor block (it then becomes a weak source for all its neighbors);
- **conductor?**: by default "collision shape = full cube".
  **To be extracted by `rc-data` for each state** (the extractor queries the jar), the list
  below is only indicative. Non-conductors although full: glass (normal, stained, 16
  colors), all leaves, ice, frosted ice, glowstone, sea lantern, TNT, redstone
  block, observer, pistons, Shulker boxes, beacon, chorus flower, scaffolding, powder
  snow, moving piston, copper grates (to be confirmed), firefly bush. Always
  conductors despite their shape: soul sand, mud.

Composite reads (used everywhere, in this exact order when order matters):
- `signal(Q, d)` = `weak(Q, d)`; if `Q` is a conductor: `max(weak(Q,d), strongReceived(Q))`.
- `strongReceived(Q)` = max of `strong(neighbor, dir)` over down, up, north, south, west, east (stop at 15).
- `isPowered(P)` = some `signal(P+dir, dir) > 0` for dir in the order down,
  up, north, south, west, east (stop at the first).
- `bestSignal(P)` = max of `signal(P+dir, dir)` over the 6 directions in the order down, up,
  north, south, west, east (stop at 15).
- `sideInput(Q, d, diodesOnly)`: if diodesOnly → `strong(Q, d)` if Q is a
  repeater/comparator, otherwise 0; otherwise redstone block → 15; dust → its `power`;
  other source → `strong(Q, d)`; otherwise 0.

Table of emitters (weak / strong):

| Block | Weak | Strong |
|---|---|---|
| `redstone_block` | 15 everywhere | 0 |
| torch (ground) | 15 except downward (not its support) if `lit` | 15 toward the block above only |
| wall torch | 15 except toward its support | 15 toward the block above |
| lever, button | 15 everywhere if `powered` | 15 toward the support block only |
| pressure plate | its power everywhere | same toward the block below |
| dust | `power` toward the block below and toward the sides where it is connected, never toward the block above (§ 3.3) | equal to weak |
| repeater, comparator | output toward the front only | equal to weak |
| observer | 15 toward the back if `powered` | equal to weak |
| detector rail | 15 everywhere if `powered` | 15 downward |
| trapped chest | number of players (≤ 15) everywhere | same downward |
| sculk sensor | `power` everywhere | same downward; calibrated: nothing through its input face |
| lightning rod | 15 everywhere if `powered` | 15 toward its support |
| tripwire hook | 15 everywhere if `powered` | 15 toward its support |
| daylight detector | `power` everywhere | 0 |
| target | `power` everywhere during the pulse | 0 |
| lectern | 15 everywhere for 2 ticks on page turn | 15 downward |

### 3.2 Component update conventions

- "Warn around `P`" = series of 6 "neighbor changed" centered on `P` (§ 1.8).
- A **diode** (repeater, comparator, observer) warns: first the block in front of it (a single
  notification), then the series around that block, skipping the diode itself.
- Components placed or removed by a piston (`moved by piston`) do not warn on removal
  (dust, diodes, torches, levers…): it is the piston that notifies (§ 3.9).

### 3.3 Redstone dust (`redstone_wire`)

State: `power` 0..15, `north/east/south/west` ∈ {`none`, `side`, `up`}.

**Connections** (computed by shape updates, not by power):
- Support: solid top face of the block below, or hopper. Otherwise → destroyed (shape downward)
  or broken with drops (neighbor changed).
- `canClimb` = the block above is not a conductor (test done with the dust's position,
  not that of the block above — no effect for the current blocks).
- Side `D`, neighbor `N = P+D`:
  1. if `canClimb` and (N is a trapdoor or N offers support to dust) and the dust
     can connect to `N+up` → `up` if the face of N turned toward P is solid, otherwise `side`;
  2. otherwise `side` if N connects (redstone: always; repeater: if D is on its axis;
     observer: if D = its `facing` direction (output side); other source: yes) or if
     (N non-conductor and `N+down` connects); otherwise `none`.
- Dot / cross: a dust with no connection is a **dot**; a dust connected
  on a single side is extended toward the opposite side ("line"). Player right click: toggles
  dot ↔ cross if the shape allows (then warn the conductors whose connection changes).
- "Indirect" shapes (diagonals): for each connected side whose neighbor is not
  dust, send a shape update to the dusts located at `N+down` and `N+up`.

**Power** (default evaluator, to be reproduced as is):
- `target(P)` = `bestSignal(P)` computed **ignoring all dust** (during this computation,
  every dust declares itself a non-source and emits neither weak nor strong); if 15 → 15; otherwise
  `max(target, wired − 1)` where `wired` = max over the 4 horizontals of: the `power` of a
  dust at `N`; if `N` is a conductor and the block above P is not, that at
  `N+up`; if `N` is not a conductor, that at `N+down`.
- Update on `neighbor changed` (if it survives) or on placement/removal:
  1. compute `target`; if equal to `power` → **nothing** (no notification);
  2. if the state at P is still the one read → set `power = target` with flag 2 (no automatic
     BUD, no shape update on the client side only… bit 16 is not set, the
     shapes go out);
  3. build the set `{P, P+down, P+up, P+north, P+south, P+west, P+east}` and, **in the
     iteration order of a Java hashed set of positions**, "warn around" each one.

  Iteration order (critical fidelity — it is what makes redstone "locational"):
  table of 16 buckets; `h = (y + z·31)·31 + x` (32-bit signed integers, overflow);
  `bucket = (h ⊕ (h >>> 16)) & 15` (logical shift); the buckets are walked 0 → 15 and, within a
  bucket, the positions in their insertion order (P, down, up, north, south, west, east). With 7
  elements the table never grows. Do NOT replace by a fixed or "optimized" order.
- Placement ("placed", if the previous block was not dust): power update;
  then "warn around" P+up then P+down (order of the game's vertical plane: up first);
  then "corners" (below).
- Removal (except by piston): "warn around" the 6 neighbors (order down, up, north, south,
  west, east); power update (the position is already empty: nothing if no dust);
  then "corners".
- "Corners": for each horizontal (north, east, south, west): if P+D is dust,
  "warn around" P+D then its 6 neighbors (order down, up, north, south, west, east);
  then for each horizontal: if P+D is a conductor → same treatment at P+D+up, otherwise at
  P+D+down.
- `weak(P, d)` (reminder: `d` goes from the receiver to the dust): receiver above (`d` = down)
  → 0; `power = 0` → 0; receiver below (`d` = up) → `power`; receiver on a side →
  `power` only if the **recomputed** connection shape (as on placement, not the stored state)
  is connected toward the receiver, otherwise 0. Strong is equal to weak (a dust charges the block
  under it and the block it points to), except during a dust `target` computation where
  every dust returns 0.

### 3.4 Repeater (`repeater`)

State: `facing` (input side; output opposite), `delay` 1..4, `locked`, `powered`.
- Delay = `delay × 2` game ticks. Survival: rigid top face below.
- Input = `signal(P+facing, facing)`; if < 15 and the input block is dust,
  max with its `power`. Desired power-on if input > 0.
- Lock = max of the `sideInput(·, ·, diodesOnly = true)` on the left and right > 0
  (only repeaters and comparators lock). `locked` is recomputed by the shape updates
  coming from the sides.
- Neighbor changed: if not locked, and (powered ≠ desired), and no tick going out this tick →
  schedule a tick (delay, priority −3 if the output faces the back of a diode not facing it,
  otherwise −2 if it is powered, otherwise −1).
- Tick: if locked → nothing. If powered and not desired → turn off (flag 2). If off →
  turn on (flag 2), and if the input has already disappeared → reschedule a tick (delay, priority −2)
  : a pulse shorter than the delay is **lengthened** to the delay.
- The "placed" behaviour is called on **every state change** (same block, flag 2
  included, § 1.8 step 6) and, for any diode, it warns what is in front (§ 3.2): this is how
  the turn-on/turn-off propagates despite flag 2. Removal (not by piston): warn the front
  as well.
- Placed by a player with an active input: tick scheduled at 1.
- Right click: next `delay` (placement with flag 3).

### 3.5 Comparator (`comparator`)

State: `facing`, `mode` (`compare`/`subtract`), `powered`; block entity that remembers the **output**.
- Fixed delay 2 ticks. Never locked.
- Rear input `E`: like the repeater, then: if the rear block has an analog output →
  `E` = that output (read with the face turned toward the comparator); otherwise if `E < 15` and the
  rear block is a conductor → look at the next block (2 cells behind): `E` = max(output
  of the single item frame (entity) facing the comparator's direction in that cell, analog
  output of the block) if one exists.
- Side input `L` = max of the `sideInput(·, ·, false)` left/right (redstone block,
  dust, any source through its strong signal).
- Computed output: `E = 0` → 0; `L > E` → 0; `subtract` → `E − L`; otherwise `E`.
- Desired power-on: `E > 0` and (`E > L` or (`E = L` and mode `compare`)).
- Neighbor changed: if no tick goes out this tick and (computed output ≠ remembered output or
  `powered` ≠ desired) → tick in 2 (priority −1 if priority like the repeater, otherwise 0).
- Tick: remembered output ← computed output; if it changed or if mode is `compare`: set
  `powered` to desired (flag 2 if changed — which already warns the front via "placed") then
  warn the front **once more** (double notification to be kept).
- Right click: next `mode` (flag 2, so "placed" warns the front), then the same
  refresh as the tick, immediately.
- Emitted signal: remembered output if `powered`, otherwise 0.
- Comparator update (bit 1 of a placement whose new state has an analog output, and
  content changes of containers): for each horizontal (north, east, south, west) of the changed
  block: adjacent comparator → "neighbor changed"; otherwise if the neighbor is a conductor and a
  comparator is right behind it → "neighbor changed" to that comparator.

Analog outputs:

| Block | Value |
|---|---|
| containers (chests, doubles included; barrels, Shulker boxes, hoppers, dispensers, droppers, furnaces, brewing stands, decorated pot; the crafter is not a container here → see below) | `fill`: sum over the slots of `count / item stack max`, divided by the number of slots; 0 if empty, otherwise `1 + floor(r × 14)` |
| crafter | number of slots that are locked or occupied (rule of its block entity, 0..9) |
| water / powder snow cauldron | `level` (1..3); lava 3; empty 0 |
| composter | `level` (0..8) |
| cake | `(7 − bites) × 2`; candle cake 14 |
| beehive / nest | `honey_level` |
| End portal frame | 15 with an eye, otherwise 0 |
| jukebox | value specific to the disc (1..15) |
| lectern | `floor(page/(pages−1) × 14) + 1` (1 page: 15) |
| chiseled bookshelf | last slot touched + 1 |
| shelf (`shelf`) | 3 bits: slot 0 → 1, slot 1 → 2, slot 2 → 4; read only from the front |
| respawn anchor | `floor(charges / 4 × 15)` |
| sculk sensor | frequency of the last vibration if active, otherwise 0 |
| copper bulb | 15 if `lit` |
| command block | success count |
| creaking heart | 15 − floor(clamp(distance to the creaking, 0, 32)/32 × 15) |
| copper golem statue | pose number + 1 |
| detector rail | (if `powered`) success of the command block minecart, otherwise fill of the container minecart |
| item frame (entity) | rotation of the item + 1 (0..8), 0 if empty |

### 3.6 Redstone torch

- Input: ground torch → `signal(P+down, down) > 0`; wall torch → signal of the support block.
- Neighbor changed: if `lit` = input present (so it must toggle) and no tick this
  tick → tick in **2**.
- Tick: purge the **world-global** list of recent toggles (older than 60 ticks,
  from the head). If lit and input → turn off (set flag 3) and record a toggle; if it has ≥ 8 toggles
  recorded at its position in the list → **burnout**: sound/visual effect 1502 and tick in 160. If off and no
  input and not burnt out (counting without adding) → relight (flag 3).
- Placement and removal (not by piston): warn around each of the 6 neighbors (order down, up,
  north, south, west, east).

### 3.7 Observer (`observer`)

State: `facing` (observed face), `powered`. Output at the back.
- A **shape update** coming from the observed face, while it is off, schedules a
  tick in 2 (if there is not one already). It does NOT listen to "neighbor changed" (it is not a
  BUD: it sees state changes, including those placed with flag 2, but not those placed
  with bit 16).
- Tick: off → turn on (flag 2), tick in 2; on → turn off (flag 2). Then
  warn the front (the block behind it, output side). Pulse = 2 ticks.
- Placement: if it arrives powered (moved by piston) and with no tick planned → turns off (flag 18) and
  warns the front. Removal: if it was powered with a tick planned → warns the front.

### 3.8 Lever, buttons, plates

- **Lever**: click → toggles `powered` (flag 3), then warn around it then around
  its support. Removal while on (not by piston): same.
- **Button**: click if not pressed → `powered` (flag 3), warn (it then its support),
  tick in 20 (stone, polished blackstone: `polished_blackstone_button`, `stone_button`) or 30
  (wooden buttons, including bamboo, cherry, crimson, warped). Tick: wooden buttons →
  test for an arrow (entity) in its shape; if nothing left → released (flag 3, warn); otherwise
  tick again. Arrow touching a wooden button → pressed as long as it is there.
- **Plates**: detection in the box (1/16..15/16 × 0..4/16 × 1/16..15/16) of entities that are
  not spectators and not in the separate "does not trigger blocks" category. Wood → all entities; stone (and
  polished blackstone) → living entities; gold (`light_weighted_pressure_plate`) → `ceil(min(n,15)/15
  × 15)`; iron (`heavy_weighted_pressure_plate`) → `ceil(min(n,150)/150 × 15)`. An
  entity entering the block → test if the plate is at 0; tick while pressed: 20 ticks (10 for
  weighted plates). Change → set flag 2 then warn around it then around the block
  below.

### 3.9 Pistons

State: `piston`/`sticky_piston` (`facing`, `extended`), `piston_head` (`facing`, `type`,
`short`), `moving_piston` (`facing`, `type`) + block entity "moving block".

**Power detection** (quasi-connectivity): true if `signal(P+d, d) > 0` for some direction
`d` ≠ `facing` (order down, up, north, south, west, east), or if `signal(P, down) > 0` (!), or if for
the block above `A = P+up` some `signal(A+d, d) > 0` for d ≠ down. The piston is therefore powered
by anything that would power the block above it, **without being warned** when that block changes
(BUD): it only re-evaluates on the next "neighbor changed" it receives.

**Check** (`neighbor changed`, placement by a player, "placed" without block entity):
- powered and retracted → if the structure (below) is pushable, block event
  `(0, facing)`;
- not powered and extended → event `(1, facing)`, or `(2, facing)` if the cell `P+2·facing` is a
  moving block of this piston, extending, and (progress < 0.5 or it ticked this tick or
  the world is in the tick phase) — this is the **fast retraction** that "drops" the block
  (`0-tick`, the case of the sticky piston that does not grab).

**Event execution** (phase 10): re-test of the power:
- event 1/2 but powered again → set `extended = true` (flag 2), end, nothing broadcast;
- event 0 but no longer powered → end, nothing broadcast.
- **0 (extend)**: move the blocks (below); if failure → end; otherwise set the piston
  `extended` (flag 67 = 1|2|64); sound.
- **1/2 (retract)**: if the head (`P+facing`) is a moving block → **finalize** it
  immediately; place at P a `moving_piston` (flag 276 = 4|16|256) with block entity "source,
  retracting, content = the retracted piston"; warn around P. Sticky: if `P+2·facing` is a
  moving block of the same direction extending → finalize it (no pull); otherwise if
  event 1 and pullable block (not air, pushable backward, reaction `push_pull` or
  piston) → move in retraction; otherwise remove the head (`P+facing`). Non-sticky: remove the
  head. Event 2 = never pulls.

**Pushability** of a state at Q for a direction `d` (and a linking direction `c`): outside the
vertical limits or the border → no; air → yes; downward at minimal y or upward
at maximal y → no; pistons: yes if they are retracted and without block entity; others: hardness −1
(indestructible) → no; reaction `block` (immovable: obsidian, anvil… per data) → no;
`destroy` → only if destruction is accepted; `push_only` → only if `d = c` (not pulled,
not dragged sideways by glue); `normal`/`push_pull` → yes if without block entity. The
reactions come from the block data (`rc-data`); block entities = immovable.

**Structure** (extension: start `P+facing`, push `facing`; sticky retraction: start
`P+2·facing`, inverse push):
1. Start block not pushable: on extension, if it is "to be destroyed" → destroy list = [it],
   success; otherwise failure.
2. Otherwise "line" from the start, then for each block added to the list (increasing index,
   list growing during the walk) that is sticky (`slime_block`, `honey_block`):
   "branches".
3. **Line(S, dir)**: air, not pushable, the piston itself or already listed → nothing (success). Otherwise
   go back behind S (opposite to the push direction) as long as the current block is sticky and
   the next one is stickable (honey and slime do not stick together; otherwise at least one sticky),
   not air, pushable with link = opposite direction, not the piston; **limit 12** blocks in total
   otherwise failure. Add these blocks from the farthest to the closest to S. Then advance in front of S in
   the push direction: if a cell is already in the list → reorder (the blocks of the current line
   are moved just before the block hit) then launch "branches" on all the sticky
   blocks up to the hit index + added, success; air → success; not pushable
   (when accepting to destroy) or the piston → failure; to be destroyed → added to the destroy list,
   success; list already at 12 → failure; otherwise add and continue.
4. **Branches(B)**: for each direction (down, up, north, south, west, east) outside the
   push axis: if the neighbor and B stick → Line(neighbor, this direction); failure propagated.

**Movement** (in the event):
1. On retraction, if the head cell still contains the head → replace it with air (276).
2. Resolve the structure; failure → abort.
3. Memorize each block to push in a **Java hash table** (position → state) and in a
   list.
4. For the destroy list, from end to start: drop the loot (with block entity), set air
   (flag 18), game event "block destroyed".
5. For the push list, from end to start: target position = position + push direction; remove
   it from the table; place a `moving_piston` (flag 324 = 4|64|256) with block entity
   (content = memorized state, not source).
6. On extension: remove `P+facing` from the table; place there a `moving_piston` with block entity
   "source, content = piston head".
7. For each position remaining in the table (in the iteration order of the Java table: 16
   buckets, same hash as in § 3.3, at most 12 entries so no growth): set air
   (flag 82 = 2|16|64). Then, same order: indirect shapes of the old state, shapes of the 6
   neighbors of the air, indirect shapes of the air (flag 2).
8. Notifications: destroyed blocks (end → start): removal effects of the old state, indirect
   shapes, warn around; pushed blocks (end → start): warn around their OLD
   position (type = pushed block); on extension, warn around the head.

**Moving block** (ticking block entity, phase 11c): progress 0 → 0.5 → 1 (+0.5 per
tick, pushing/dragging the entities, see `entities.md`); on the tick where the previous progress
is already 1: remove the block entity, and if the cell is still a `moving_piston`:
final state = content adjusted by the shape updates with its neighbors; if it becomes air →
placement of the content (flag 340) then destruction; otherwise (removing `waterlogged`) set flag 67
then "neighbor changed" on the cell itself. **Forced finalization** (retraction during the
movement, or block entity removed): progress 1 and same placement (source → air), set
flag 3 and "neighbor changed" on the cell. A piston therefore takes 2 game ticks to finish advancing
and the cell is occupied by `moving_piston` during that time (non-conductor).

### 3.10 Powered rails, activator rails, detector rails

- **Powered rail / activator rail**: powered if `isPowered(P)` or if the search along the rail
  (8 rails maximum, in both directions) finds a rail of the same type, powered, and either `isPowered`
  or linked further. The search follows the shape (`north_south`, `east_west`, slopes: the next
  rail can be one step higher or lower; the slope changes the axis to straight). A rail
  of perpendicular axis stops the search. Change → placement (flag 3), warn around the
  block below and, for a slope, around the block above.
- Any rail: "neighbor changed" → if no more support (rigid block below; for a slope, the block
  on the uphill side too) → broken; otherwise its own update (simple rail: reorientation only if
  the source that changes is a redstone source and it has 3 possible connections — T-curves
  controlled by redstone).
- **Rail shape** (placement outside the piston-same-block case, and simple rail reoriented): each
  horizontal neighbor (at the same level, one above or one below) counts if it "can connect" —
  after forgetting its connections that no longer point to a rail that knows it, it has fewer
  than 2 connections or already has one toward us. North/south alone → `north_south`, west/east alone →
  `east_west`; non-straight rails: exclusive curves (S+E without N or W…). Otherwise: both axes →
  current shape; non-straight rails: curve chosen, the order of the tests (the last true wins)
  depends on `isPowered(P)` (powered: SE, SW, NE, NW; otherwise: NW, NE, SW, SE). A rail one step
  higher at the end of the axis → slope (north then south, east then west: the second wins). If the
  shape changes (or on first placement): set flag 3, then each linked rail that can connect
  adds the connection and re-places itself (flag 3, same shape rule without the signal choice).
  Straight rails: after the shape, a "neighbor changed" on itself (power).
- **Support on a moving block**: a moving piston block that has arrived at the end (progress 1)
  has the shape of the block it carries; it therefore supports rails, carpets, torches placed on the block that
  arrives before it (finalization order of the moved blocks).
- **Detector rail**: minecart in the box (0.2..0.8 × 0..0.8 × 0.2..0.8) → `powered` (flag 3),
  propagate to the connected rails, warn around it then the one below, comparator
  update; re-test every 20 ticks while it is lit.

### 3.11 Doors, trapdoors, fence gates

- Door: powered if one of its two halves is `isPowered`. Notifications coming from a
  door of the same type are ignored. If `powered` changes: `open = powered` (sound and game
  event if `open` changes) and set flag 2 on the notified half; the other half follows by shape
  update. Iron doors: only by redstone.
- Trapdoor: `isPowered(P)`; if `powered` changes → `open = powered`, set flag 2 (and water tick
  if waterlogged). Fence gate: like the trapdoor (`open`/`powered`).

### 3.12 Redstone lamp

- Neighbor changed: lit and no longer `isPowered` → tick in 4; off and `isPowered` → lit
  right away (flag 2). Tick: if still not powered → off (flag 2).
- Placement by a player: `lit = isPowered`.

### 3.13 Dispenser, dropper, crafter

- Trigger: `isPowered(P)` **or** `isPowered(P+up)` (quasi-connectivity) for
  dispenser/dropper; `isPowered(P)` only for the crafter.
- Rising edge (`triggered` false → true): tick in 4 and `triggered = true` (flag 2);
  falling edge: `triggered = false` (crafter: also `crafting = false`).
- Tick: dispenser → picks a non-empty slot at random (`R`, uniform draw among the slots
  that are non-empty, reservoir) and applies the item's behaviour (arrows, buckets, bone meal, fire…); dropper
  → drops or inserts into the container in front; crafter → crafts the recipe of its grid and
  ejects. Nothing to dispense → "failure" sound (1001).
- Comparator update on every content change.

### 3.14 Target, copper bulb, lightning rod, daylight detector

- **Target** (`target`): hit by a projectile, if no tick is planned: `power` =
  `max(1, ceil(15 × clamp((0.5 − dist)/0.5, 0, 1)))` where `dist` = max of the offsets from the center on the
  two axes of the hit face; set (flag 3) and tick in 20 (arrow/trident) or 8 (others);
  tick → `power = 0`. Connects to dust (source).
- **Copper bulb** (all oxidations, waxed or not): on every "neighbor changed" and on
  placement: if `isPowered ≠ powered`: on rising edge → toggles `lit`; sets `powered` (flag 3).
  Light 15/12/8/4 depending on oxidation. Analog output 15 if lit.
- **Lightning rod** struck: `powered` 8 ticks, warn around it then its support.
- **Daylight detector**: every 20 game ticks (`game_time % 20 == 0`): `sky − darkening`
  at its position; inverted → `15 − value`; otherwise if > 0: sun angle brought toward 0 or 2π by
  20%, value × cos(angle) rounded; clamped 0..15; set flag 3 if changed.

### 3.15 Sculk sensors (vibrations)

- `sculk_sensor`: listening radius 8; `calibrated_sculk_sensor`: 16 and frequency filter
  (input through its back = the accepted frequency if powered).
- Phases: `inactive` → (vibration received) `active` for 30 ticks (calibrated: 10) with
  `power = max(1, 15 − floor(15/radius × distance))` → `cooldown` 10 ticks → `inactive`. Each
  change: set flag 3, warn around it then the block below.
- Vibration: travel delay = distance in ticks; blocked by wool (and carpets) on the
  path; discreet (sneaking) entities do not emit steps.
- Frequencies per game event (comparator output value):

| Freq. | Events |
|---|---|
| 1 | step, swim, flap |
| 2 | projectile landing, hitting the ground, splash, bounce |
| 3 | item use finish, projectile shot, instrument play |
| 4 | entity action, elytra flight, equipment unequip |
| 5 | dismounting, equipping |
| 6 | entity interaction, shearing, mounting |
| 7 | damage taken by an entity |
| 8 | drinking, eating |
| 9 | container close, block close, block deactivate, detach |
| 10 | container open, block open, activate, attach, fuse ignition, note block |
| 11 | block state change |
| 12 | block destroyed, fluid pickup |
| 13 | block placed, fluid placement |
| 14 | entity placement, lightning, teleportation |
| 15 | entity death, explosion |

- Resonance: an amethyst block neighboring an active sensor re-emits the received frequency.
- Shrieker (`sculk_shrieker`) and catalyst: § 2.12.

### 3.16 Tripwire

- Hooks (`tripwire_hook`) linked by 1..40 strings (`tripwire`) in a straight line. An entity (not
  a spectator, not one that "ignores blocks") in the string → `powered` of the string; the hooks
  update (`attached`, `powered`), emit 15 (strong toward their support); re-test of the string
  every 10 ticks while it is occupied. String cut by shears → `disarmed` (no signal).
- Recomputation of a hook (placement by a player, scheduled tick, string placed or removed, hook removed
  attached or powered): advance from 1 to 41 cells in its direction; an opposite hook stops and
  serves as receiver; any cell that is not a string (except the one that has just changed, taken with
  its new state and that schedules a recomputation in 10 ticks) detaches. `attached` = receiver more than
  one cell away and complete line; `powered` = attached and an armed string occupied. The receiver is
  re-placed (flag 3) and warns around itself and its support; if the original hook disappeared in the
  meantime, its removal is processed and we stop. Then the original hook is re-placed
  (unless it is destroyed); if `attached` changed, each string of the line still in place is
  re-placed with the new `attached`. A string placed or removed restarts the hooks to the south then
  the west (first non-string cell). Only the detach sound draws a random number from the world.
- String: horizontal connections toward a string or a hook facing it.

### 3.17 Hopper

- `enabled = not isPowered(P)` (re-evaluated on every "neighbor changed" and on placement; set
  flag 2 if changed).
- Block entity: transfer cooldown 8 ticks; on every tick where it is at 0 and enabled:
  push 1 item to the targeted container, then suck from the container above or the items
  (entities) in the zone above. Details of the transfers and the slot order: to be specified
  with the inventories (`inventories.md`).

---

## 4. Invariants for an official machine to work the same

A redstone machine built on the official game must give, tick by tick, the same
block states in the Rust singleplayer. What follows is **mandatory**; any "optimization" that
touches one of these points is forbidden until it is proven equivalent by traces.

### 4.1 Orders to reproduce identically

1. Tick phases (§ 1.1, § 1.2): scheduled block ticks → fluids → (raids) → random
   ticks → block events → entities → block entities.
2. Scheduled tick queue: collection by (due tick ≤ T) then sort by **(priority, sub-order)**;
   sub-order = global creation counter; uniqueness (type, position); cap 65,536;
   diode priorities (§ 1.3, § 3.4, § 3.5); "runs this tick" test (§ 1.3).
3. Depth-first notification scheduler with queueing during a step
   (§ 1.8), order of the 6 neighbors **west, east, down, up, north, south** for BUDs and **west, east,
   north, south, down, up** for shapes; order of the steps of a placement (removal of the old one,
   "placed" of the new one, BUD, comparators, shapes).
4. Orders specific to signal reads (down, up, north, south, west, east with early stop):
   no effect on the result, except when a read has a side effect (none known) — they can be
   kept as is at no cost.
5. Redstone dust: iteration order of the hashed set (§ 3.3), order of placement/removal
   and of the "corners". This is the cause of **locational and directional** redstone: the same
   machine can behave differently depending on its position and orientation in the world;
   players exploit these effects (piston doors, clocks) and protect themselves from them with
   designs that resist them. The Rust code must have exactly the same.
6. Pistons: construction order of the list (lines, branches, reordering), cap 12,
   order of placements (destroyed end → start, pushed end → start, head), iteration order of the
   table of cells to empty (same hash as § 3.3), then order of the notifications (§ 3.9).
7. Block events: queue without duplicates, FIFO, re-entrant within the phase (§ 1.6).
8. Block entities: order of addition; new ones wait for the next tick.
9. Torch: single toggles list for the world, ordered by time, purged at the start of
   each torch tick (entries with `now − when > 60`). Since counting always follows
   a purge, this is equivalent to "number of extinctions of this position in the last 60
   ticks (bounds included)": a per-position structure is allowed if it gives exactly that count.

### 4.2 "Counter-intuitive" behaviours to keep

- **Quasi-connectivity**: pistons (powered by what would power the block above) and
  dispensers/droppers (`isPowered(P+up)`). Not the crafter.
- **BUD** (update detection): a piston/dispenser powered by quasi-connectivity
  only reacts to the next notification it receives; it stays "armed" without moving. Any
  notification (even from an unrelated block, e.g. placing a flower next to it) triggers it.
- **Observers**: react to **shape** updates (state change of the observed neighbor,
  even without BUD), not to BUDs; 2-tick pulse.
- **Pulse lengthening** by repeaters; locking by side diodes.
- **Comparators**: double notification in front; reading through a conductor block;
  subtraction; analog outputs (§ 3.5).
- **Fast retraction** of sticky pistons (event 2: the block is dropped) and immediate
  finalization of a moving block when the piston retracts during its extension (0-tick).
- **`moving_piston`** occupies the cell for 2 ticks, is not a conductor, propagates nothing:
  machines that rely on blocks "in transit" depend on this delay.
- **Shape updates with flag 2** (turning on a dust, a repeater): trigger
  observers but not BUDs.
- **Bit 128** (shapes that ignore dust) and **bit 16** (pistons: no shapes on placing
  moving blocks): to be respected, otherwise observers trigger wrongly.
- **Chained notification limit** (1,000,000 by default): beyond it, notifications
  are dropped (some "update suppression" machines depend on it); depth 512
  for shapes.
- **Torch clocks**: burnout at 8 toggles in 60 ticks, extinguished for 160 ticks.

### 4.3 Parallelism (architecture § 14)

The tick of a world is **sequential** in the game: a single random `R`, a single LCG of
positions, a single sub-order counter, a single torch list, a single scheduler of
notifications. Parallelizing the ticks of independent regions is only allowed if:
- the random draws of one region do not influence those of another (but `R` is shared:
  the chunk order changes all the sequences) — to be considered only for processing without
  draws (e.g. pure power recomputation, light), or by accepting different sequences
  outside deterministic machines (decision to be made at implementation time, measured);
- no notification or tick crosses the boundary during the parallel phase;
- the result is merged in sequential order (sub-orders of the scheduled ticks reassigned
  in the order in which the game would have created them).
Recommendation: **sequential** simulation of redstone and scheduled ticks, parallelism
reserved for generation, light and the random ticks proven not to
overlap (and only if the difference in the random sequence is accepted).

### 4.4 Verification

- Trace bench: same world (region copy), same player inputs → per-tick log of the
  (position, old state, new state, cause) on the official server side (via a local test server
  instrumented by commands: `/tick step`, block reads through RCON) and on the Rust side; diff.
- Tool: singleplayer probe (`rc-sim`, `blocks::probe`, ignored test `probe_script`) and
  `tools/testmap/blocks_probe.py`: the script (commands `setblock`/`fill`, `wait N`, `tick N`) is
  first played on the official server noting the real game time of each group of commands
  (the shared server may lag), then replayed in singleplayer with those gaps, and the states
  recorded tick by tick (snapshots framed by two identical `time query gametime`) are
  compared. Pitfall: a dedicated server pauses with no connected player
  (`pause-when-empty-seconds`); keep a client connected during the readings. Scheduled ticks
  survive the replacement of a block: empty the area then wait before building a
  new setup in the same place.
- Cases to cover at a minimum: 15-long dust line with forks, repeater clock,
  observer clock, 2×2 and 3×3 piston door, slowing machine (locational dust),
  piston BUD, sticky piston 0-tick, slime/honey in branches (limit 12),
  comparator on chest/furnace/cauldron, burnt-out torch, lamp, powered rail over 9 blocks,
  observer sugar cane farm, stone generator (water/lava), bamboo farm.

---

## Uncertain points

1. **Hash collisions in the piston table**: with ≤ 12 entries and 16 buckets, a bucket
   could receive ≥ 8 positions (extreme case of 2D slime); the game would then grow the
   table (64 buckets minimum before treeification). Recommendation: implement a faithful copy of the
   behaviour of the hashed table (insertion, growth, order) rather than a shortcut, and
   test with negative positions (the logical shift `>>> 16` matters).
2. **Chunk walk order** for random ticks (§ 1.4): we follow the order of the server's
   chunk table, which is not specified here; it only influences the sequence of
   `R`, not the mechanics. To be fixed if the random traces must coincide.
3. **Zone walk order** (walk of a box of positions) for eyeblossoms (delay draws) and
   oxidation (Manhattan, no effect on the result): x/y/z order to be checked before writing the
   code.
4. **Vines**: the "upward/downward" branch and the exact clinging conditions are
   summarized; to be detailed (`R` draws) before implementation.
5. **Bubble columns**, **chorus flower**, **speleothems**, **sulfur spike**
   (`sulfur_spike`, `potent_sulfur`, 26.x content), **beehives**, **sculk** (propagation of the
   charges), **vaults/trial spawners**: only summarized; a dedicated spec per family
   will be needed.
6. **Light attenuation** used by ice and frosted ice ("11 − attenuation"):
   value to be read in `rc-data` (block property), not copied here.
7. **Rails**: reorientation (shape depending on neighboring rails, T-curves) is not detailed
   — it belongs to placement (`interactions.md`); only the power part is described.
8. **Hoppers, dispensers, crafter**: slot order and transfer rules to be specified with
   the inventories.
9. **Experimental redstone mode** (feature flag "experimental redstone":
   orientation passed, other dust evaluator): deliberately ignored; standard official worlds
   do not enable it.
10. **Ticket level thresholds** (32 / 31): taken from the protocol memory, to be confirmed
    when writing `rc-sim`'s chunk manager.

