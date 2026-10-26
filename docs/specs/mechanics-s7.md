# Remaining survival mechanics of the integrated server (S7, 26.3)

Specification in our own words, read in the official 26.3 server; code: `rc-sim` `effects.rs`,
`potions.rs`, `brewing.rs`, `enchanting.rs`, `stations.rs`, `stations_srv.rs`, `lootchest.rs`,
`lootfn.rs`, `randsrc.rs`.

## 0. Two generators

- The server draws most of its random numbers from the "modern" generator (Xoroshiro128++, level
  random source, named sequences). Two mechanics use the **old 48-bit linear congruential generator**
  seeded explicitly: the **filling of a seeded container** (`LootTableSeed` ≠ 0) and the
  **enchanting table** (player seed). The two generators do not use the same formula
  for "integer in [0, n)"; the code keeps them apart (`Rand`).
- "Integer between a and b" in its utility form: `a` if `a ≥ b`, otherwise `a + next_int(b − a + 1)`.
- Shuffling a list: from the end to the start, swap slot i with a slot drawn in
  [0, i].

## 1. Status effects

- Instance: effect, amplifier (0..255), duration (−1 infinite), ambient, visible particles,
  icon (default = particles), optional **hidden effect** (longer instance of a weaker
  level, kept behind).
- Adding an effect that is already present (merge):
  1. stronger amplifier: if the new one is shorter, the old one becomes the hidden effect
     (keeping its own hidden one behind); amplifier and duration replaced;
  2. otherwise, if the current one is shorter: same amplifier → duration replaced; weaker
     amplifier → the new one becomes (or updates) the hidden effect;
  3. ambient, particles, icon copied if they differ (ambient only towards "not ambient"
     or if something else changed). "Changed" → update packet (no fade-in) and attributes
     recomputed. A new effect sends the packet with the fade-in flag (8).
- Tick (end of the player's base tick, after air, fire, wall): for each effect, if duration
  remains, test "applies this tick" on the counter (remaining duration, or age if infinite):
  regeneration every `50 >> amp` ticks (heal 1 if hurt), poison `25 >> amp` (1 magic
  damage if health > 1), wither `40 >> amp` (1 "wither" damage), hunger every tick
  (exhaustion 0.005 × (amp+1)), absorption every tick (stops when the reserve is empty),
  instant effects as soon as the duration ≥ 1. Then duration −1 (hidden ones included); at 0 with a hidden one,
  the hidden one takes its place (packet). End of duration: removal (removal packet). Every 600 ticks
  of remaining duration: update packet.
- Instant effects: heal 4 << amp, magic damage 6 << amp, saturation: food +(amp+1),
  saturation +2 × (amp+1). Drunk as a potion: multiplied by the scale (1 for a drunk potion,
  rounded `(int)(e × base + 0.5)`).
- Attributes (amount × (amp+1)): speed +20% of the total, slowness −15%, haste +10% of the
  attack speed, mining fatigue −10%, strength +3 damage, weakness −4, jump boost +1 safe
  fall distance, health boost +4 max health, absorption +4 max reserve, luck ±1,
  invisibility −100% of the beacon emission range. Modifier ids:
  `minecraft:effect.<name>`.
- Absorption: at start, reserve = max(reserve, 4 × (amp+1)). Damage (after armor,
  Resistance, protections) eats into the reserve first.
- Resistance: damage × (25 − 5 × (amp+1)) / 25, except types "ignores effects" or "ignores
  resistance". Fire resistance: any fire-type damage cancelled before the invulnerability
  window.
- Water breathing (or conduit, or nautilus breath): no air loss.
- Slow falling or levitation: fall distance reset every tick.
- Mining: tool speed (+ level² + 1 from Efficiency if > 1), × (1 + 0.2 × (amp+1)) with
  haste or conduit (the stronger one), × 0.3^(amp+1) with mining fatigue, × 0.2 underwater (× 5 per level
  of Aqua Affinity), ÷ 5 when off the ground.
- Player entity data: list of visible effect particles (index 10; ARGB color
  of the effect, alpha 38 if ambient; own particle for omens), "all ambient" (11),
  absorption reserve (17); shared flags: invisible 0x20, glowing 0x40.
- Joining the game and dimension change: all effects re-sent. Respawn after
  death: no effect or absorption left. Save: `active_effects` (amplifier as a byte),
  `AbsorptionAmount`.
- Milk: all effects removed. Honey: poison removed. Golden apple, pufferfish…: effects of the
  `consumable.on_consume_effects` component (a single probability draw for the list).
  Ominous bottle: bad omen 120,000 ticks, amp = component, no particles.

## 2. Potions and brewing stand

- Potion effects: fixed table of the game (durations 3600/9600/1800…; e.g. turtle master:
  slowness IV + resistance III, 400 ticks). Contents of a stack: effects of the potion then added
  effects, durations multiplied by `potion_duration_scale` (at least 1 tick, infinite and 0
  unchanged).
- 26.3: mixes are no longer hardcoded. Pack `minecraft:brewing` recipes
  (`recipe/brewing/*.json`): input (item + accepted potions), reagent, product. First
  recipe (in id order) that accepts the bottle and the reagent.
- Sets sent to the client: `brewing_input` (items of the inputs) and `brewing_reagent`.
  Bottle slot: known input or tag `brewing_potion_inputs` (empty bottles included),
  one per slot. Fuel slot: component `brewing_fuel`.
- Brewing stand tick: with no fuel left and with a `brewing_fuel` item: fuel =
  "uses" (pack provider, 20 for blaze powder), speed = provider
  (1.0), the item is consumed (any remainder dropped if the stack is not empty). If a brew
  is in progress: time −1; at 0 and still brewable → mix; otherwise, if no longer brewable or the
  reagent changed → time to 0. Otherwise, if brewable and fuel > 0: fuel −1, time
  = ⌈400 / speed⌉. The mix replaces each bottle that has a recipe, consumes one
  reagent, event 1035. Block properties `has_bottle_0..2` tracked (placement without neighbors).
- Menu (type 11): 5 slots, 4 data values (remaining time, fuel, total time, total fuel).
  Quick move from the inventory: fuel first (if it fits, stop there), otherwise
  reagent, otherwise bottles, otherwise between the main inventory and the hotbar.

## 3. Enchanting

- Pack definitions (`enchantment/*.json`): weight, max level, min/max costs
  (`base + per_level × (level − 1)`), anvil cost, supported / primary items, exclusive
  set. Two enchantments are compatible if they differ and neither is in the
  exclusive set of the other.
- Candidates for a value: enchantments of the source whose item is primary (everything for
  a book); for each one, the highest level whose [min, max] interval contains the value.
- Draw (`enchantable` = e): value += 1 + next_int(e/4 + 1) + next_int(e/4 + 1); deviation
  = (next_float() + next_float() − 1) × 0.15; value = round(value + value × deviation), at least 1.
  First weighted pick (next_int(total) then subtracting the weights in order); then while
  next_int(50) ≤ value: candidates incompatible with the last one removed, new weighted pick,
  value /= 2.
- Table: bookshelves = "power provider" blocks on the 5 × 5 ring (|dx| = 2
  or |dz| = 2), heights 0 and 1, intermediate "transmitter" slot (air). Generator seeded by
  the player seed; for each row i: `s = next_int(8) + 1 + b/2 + next_int(b + 1)` (b ≤ 15),
  row 0: max(s/3, 1), row 1: 2s/3 + 1, row 2: max(s, 2b); cost < i + 1 → 0. Then for
  each non-zero row: generator reseeded with seed + i, list drawn (a book loses one at
  random if it has several), then an index drawn in the list → enchantment (network id)
  and level shown.
- Accepted item: component `enchantable` and empty `enchantments`.
- Button i: lapis ≥ i + 1 and levels ≥ max(i + 1, cost) (except creative), non-empty list →
  levels −(i+1) (at 0: progress and total to 0), new player seed (player
  random source), book → enchanted book, enchantments added (at the highest level), lapis −(i+1),
  view recomputed, sound `block.enchantment_table.use`.
- Menu (type 13): 2 slots; 10 data values: costs ×3, seed (16 bits sent), indices ×3,
  levels ×3 (−1 without an index). Quick move: lapis to its slot, other item: a single
  copy placed if the slot is empty.

## 4. Anvil, grindstone, smithing table, stonecutter

- Anvil (type 8, 3 slots, 1 data value = cost): base cost 1, tax = repair costs of the
  two stacks. Repair material (`repairable`): per unit, wear −min(wear, max/4),
  +1 per unit. Otherwise (same item or book): durability merge (+12% of max, +2 if it
  repairs), then for each enchantment of the addition: level = n+1 if equal, otherwise the max;
  compatible if supported (always in creative or on a book); each conflict +1; compatible:
  level capped, anvil cost (half for a book, at least 1) × level; stack of more than
  one item: price forced to 40. Nothing compatible → no result. Name: different from the
  displayed English name → +1; name cleared when there was one → +1. Cost = tax + price (0 if
  price is zero); simple rename capped at 39; ≥ 40 outside creative → no result. Repair cost
  of the result: max of the two, then ×2+1 except for a simple rename. Taking the result: levels
  removed (except creative), addition consumed (or only the repair units), anvil wear
  at 12% (broken → event 1029, otherwise 1030). Name filtered (no `§` or control
  characters), 50 characters at most.
- Grindstone (type 15): a single enchanted item → non-cursed enchantments removed; two identical
  items → durability added + 5%, enchantments merged then non-cursed removed;
  book without an enchantment → book; repair cost = 2^n − 1 for n remaining curses.
  Experience: sum of the minimum costs of the non-cursed enchantments of both inputs, h =
  ⌈s/2⌉, gain h + next_int(h) (level random source), event 1042.
- Smithing table (type 21, 4 slots): first `smithing_transform` / `smithing_trim` recipe whose
  three ingredients match (absent ingredient = empty slot). Transform: result item
  with the components of the base. Trim: material = component
  `provides_trim_material` of the addition; same trim already applied → nothing. "Error" data value =
  three full inputs and no result. Taking the result: one unit of each input, event 1044.
- Stonecutter (type 24): recipes of the input in recipe order (list sent to the client
  in `update_recipes`), choice by button, result = product of the recipe. Taking the result: one input
  consumed, result put back; quick move: the rest that does not fit is dropped.

## 5. Loot containers

- `LootTable` (+ `LootTableSeed`) on chest, trapped chest, barrel, dispenser, dropper,
  hopper, Shulker box: table drawn on opening (player luck) or on breaking
  (no player), then forgotten. A spectator cannot open a container that is still sealed.
- Generator: seed ≠ 0 → congruential seeded with it; otherwise the sequence named by the table.
- Filling: empty slots shuffled; generated items (stacks already split to their max
  size); stacks of more than one item set aside; while there are more slots than items:
  a stack drawn, split by an amount drawn in [1, n/2], each piece of more than one item
  put back aside on a coin flip; everything is gathered, shuffled, then each stack goes to the last
  slot of the shuffled list.
- Added functions: random enchantment (options, compatibility except for a book; level drawn
  between 1 and max), enchantment by levels (same draw as the table), wear (fraction drawn),
  fixed enchantments, potion, suspicious stew (one effect drawn, duration × 20 except instant),
  name, goat horn (instrument drawn from the tag), ominous bottle, filter. Explorer map: the
  structure search is not simulated (empty map unchanged).

## 6. Special crafting table recipes

Rocket (paper + 1 to 3 gunpowder + stars: flight = number of gunpowder, explosions of the stars),
star (gunpowder, dyes → firework colors, shape by item, diamond = trail,
glowstone dust = twinkle), star fade, written book copy (generation + 1, at most
copy of a copy; the source book stays in the grid), shield decoration (patterns and base color
of the banner), banner copy (same color, ≤ 6 patterns, the source banner stays),
decorated pot (four sides in a 3 × 3 grid). These recipes do not enter the recipe
book. Map extension: not simulated (no maps in singleplayer).

## 7. Checks against the official server (13/10)

- Fixed-seed chests (dungeon 42, End city 7, stronghold library 123456789, buried treasure
  −5, ancient city 99): 81 slots out of 81 identical, enchantments included
  (`tests_s7::seeded_chest_loot`).
- Brewing stand: 3 water bottles + nether wart + blaze powder → 3 awkward potions, fuel 20 → 19,
  400 ticks; in singleplayer, awkward potions + sugar → swiftness seen on screen.
- Table (seed 12345, 26 bookshelves, diamond sword): costs 6/20/30, indices Sharpness I,
  Bane of Arthropods III, Knockback II; row 3 drawn: Sharpness III, Looting III, Sweeping Edge III,
  Knockback II, Fire Aspect II — identical in singleplayer (captures `S7/off-e*`, `S7/solo-e*`).
- Anvil: sword + Sharpness III book → 3 levels on both.

## 8. Known differences

- Comparator or hopper against a sealed container: the game draws the loot (slot
  reads); singleplayer only does it on opening and on breaking.
- Effects without simulation: bad omen → raid omen (villages and raids absent),
  weaving, oozing, infestation, wind charge (mobs), hero of the village.
- Thrown, lingering potions, tipped arrows: player projectiles absent from singleplayer.
- Order of the enchantments of a stack: that of the component (the game follows the order of an
  identity hash table, not reproducible); only matters for an incompatible addition at the anvil.
