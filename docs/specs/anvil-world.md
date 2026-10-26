# On-disk world format (26.3, data version 5023)

Sources: reading of the decompiled server (kept outside the repository), reworded; checked against the real world of
the test map (`server-test/rustcraft-test`, read from a copy, never opened for writing). Frame:
`docs/architecture.md` § 14 (Rust singleplayer reads and writes worlds in the official game's format). First
milestone: open an existing world, play in it, save it; no generation.

Conventions: all on-disk integers are **big-endian**. "NBT" = the game's binary format
(§ 1). File names, NBT keys and ids (`minecraft:…`) are data.

---

## 0. Layout of a 26.3 world

The layout changed compared with old versions (no more `playerdata/`, no more
`DIM-1`/`DIM1`, no more rules or weather in `level.dat`). The official game migrates old
worlds on opening ("file fixers"); Rust singleplayer **only opens worlds already in
26.3 format** (§ 9).

```
<world>/
  level.dat, level.dat_old          world settings (§ 2)
  session.lock                      lock (§ 8.1)
  icon.png                          optional
  datapacks/                        world data packs (not interpreted at milestone 1)
  data/minecraft/*.dat              global data (§ 6.1)
  data/minecraft/maps/<n>.dat       maps, + last_id.dat
  players/data/<uuid>.dat(+_old)    players (§ 5)
  players/advancements/<uuid>.json  advancements
  players/stats/<uuid>.json         statistics
  dimensions/<namespace>/<name>/    one dimension, e.g. dimensions/minecraft/overworld
    region/r.<X>.<Z>.mca            block chunks (§ 3, § 4)
    entities/r.<X>.<Z>.mca          entities (§ 4.8)
    poi/r.<X>.<Z>.mca               points of interest (§ 4.9)
    data/minecraft/*.dat            dimension data (§ 6.2)
```

Seen on the test map: `overworld`, `the_nether`, `the_end` (the latter with
`ender_dragon_fight.dat`). A dimension without any chunk may have only `data/`.

---

## 1. On-disk NBT

- Root: type byte (10 = compound), **name** (`u16` string + modified UTF-8, empty in practice),
  payload. (Network NBT has no root name: `chunks.md`.)
- Types: 1 byte, 2 `i16`, 3 `i32`, 4 `i64`, 5 `f32`, 6 `f64`, 7 byte array (`i32` + bytes),
  8 string, 9 list (element type + `i32` + elements), 10 compound (typed, named entries
  up to type 0), 11 `i32` array, 12 `i64` array.
- **Heterogeneous lists (26.3 rule)**: a list whose elements do not all have the same type is
  written as a list of compounds; each element that is not a compound (or that is a compound
  reduced to a single empty key) is wrapped in a compound with **a single entry named `""`**.
  On reading, a compound with a single entry with key `""` is unwrapped. A homogeneous list stays
  native (e.g. biome palette: list of strings). Seen in block state palettes (§ 4.3).
- `.dat` files: whole NBT compressed with **gzip**. Tolerant reading recommended (gzip, otherwise zlib,
  otherwise raw).
- The game bounds the nesting depth (512); refuse beyond that rather than crash.

---

## 2. `level.dat`

Gzip. Root `{ Data: { … } }`. Keys seen in 26.3 (test world):

| Key | Type | Role |
|---|---|---|
| `DataVersion` | int | **5023** for 26.3 |
| `Version` | compound | `Id` (5023), `Name` (`"26.3"`), `Series` (`"main"`), `Snapshot` (byte) |
| `version` | int | 19133 (constant of the "anvil" format) |
| `version_history` | list of int | data versions that opened the world (add 5023 if absent) |
| `LevelName` | string | displayed name |
| `LastPlayed` | long | ms since the epoch, rewritten on every save |
| `Time` | long | **game time** (ticks, +1 per tick; ≠ time of day, § 6.1 `world_clocks`) |
| `GameType` | int | default mode for new players (0 survival, 1 creative, 2 adventure, 3 spectator) |
| `difficulty_settings` | compound | `difficulty` (string `peaceful`/`easy`/`normal`/`hard`), `hardcore` (byte), `locked` (byte) |
| `spawn` | compound | `pos` (int[3]), `yaw`, `pitch` (float), `dimension` (string): the world's spawn point |
| `allowCommands` | byte | cheats allowed |
| `initialized` | byte | 1 |
| `WasModded` | byte | 0 (Rust singleplayer must not set it to 1, § 9) |
| `ServerBrands` | list of strings | brands that opened the world; add ours only if we accept that the official game shows the world as "modded" — **do not add it** at milestone 1 |
| `DataPacks` | compound | `Enabled`, `Disabled` (lists of strings: `vanilla`, `file/<name>`, experimental flags) |
| `enabled_features` | list | optional (feature flags) |
| `removed_features` | list | optional |
| `singleplayer_uuid` | int[4] | owner's UUID in singleplayer (§ 5); absent on a dedicated-server world |

Reading: `Time` absent ⇒ 0; `spawn` absent ⇒ (0, 64?, 0) at the surface — see uncertain points.
Writing: the official game **rebuilds** `Data` from its model (unknown keys lost);
Rust singleplayer does better: it starts from the compound it read, updates `Time`, `LastPlayed`, `spawn`,
`difficulty_settings`, `GameType`, `singleplayer_uuid`, `version_history`, `DataVersion`/`Version`
only if identical to 26.3, and **keeps everything else as is**.

Safe replacement (§ 8.2) with backup copy `level.dat_old`. On opening: if `level.dat` is
unreadable, try `level.dat_old`.

---

## 3. `.mca` region files

A region = 32 × 32 chunks. Name `r.<X>.<Z>.mca` with `X = floor(cx / 32)`, `Z = floor(cz / 32)`
(division rounded toward −∞). Local index `i = (cx & 31) + (cz & 31) · 32`.

### 3.1 Header (8,192 bytes)
- Bytes 0–4095: 1024 `u32` "location" integers: `sector << 8 | sector_count`
  (sector = 4,096 bytes, counted from the start of the file; 24 bits of sector, 8 bits of
  count). 0 = chunk absent.
- Bytes 4096–8191: 1024 "timestamp" integers (seconds since the epoch, last write or
  erasure of the chunk).
- Location rejected (treated as absent, with a warning): sector < 2 (overlaps the header),
  count 0, or sector beyond the end of the file. **Only the header is authoritative**: seen on the test
  map, an entity region with a completely empty header still contains stale data
  in its sectors.
- Truncated header (file < 8,192 bytes): read what exists, the rest is 0.

### 3.2 Chunk within its sectors
At `sector · 4096`: `u32 length` (= bytes of the compressed stream **+ 1**), `u8 type`, then the stream.
- Types: **1 gzip, 2 zlib ("deflate", the game's default), 3 none, 4 LZ4 (block format of
  `lz4-java`)**, 127 = custom compression (a modified UTF string follows: the game refuses ⇒
  chunk unreadable). Unknown type ⇒ unreadable.
- Bit `0x80` of the type: **external stream** in `c.<cx>.<cz>.mcc` (absolute chunk coordinates,
  in the region's folder); the sector then contains only a stub (`length = 1`,
  type `| 0x80`). The `.mcc` contains the compressed stream alone, with no header.
- Length 0 ⇒ "allocated but empty" (absent). Length − 1 > reserved space ⇒ truncated (unreadable).
- The choice of write type comes from the server property `region-file-compression`
  (`deflate`, `lz4`, `none`); Rust singleplayer writes **2 (zlib)**, and reads all four.

### 3.3 Writing a chunk (copy on write)
1. Compress; `size = 5 + stream`; `sectors = ceil(size / 4096)`.
2. If `sectors ≥ 256` (≈ 1 MiB): write the stream to a temporary file in the folder, reserve
   **1** sector, write the stub in it; the `.mcc` is put in place (rename) **after** the header.
   Otherwise: reserve `sectors` sectors and write the data in them; delete any old `.mcc`
   after the header.
3. Reservation: first free gap large enough in the map of occupied sectors (starting from the
   beginning; the end of the file counts as free). Sectors 0–1 are always occupied.
4. Update location and timestamp, **rewrite the whole header**, and only then free the
   old sectors. An abrupt stop thus leaves the old chunk intact.
5. Erasing a chunk (e.g. no entity left, § 4.8): location 0, timestamp = now,
   header rewritten, `.mcc` deleted, sectors freed.
6. On closing the file: **pad** up to the next multiple of 4,096 bytes (write a null byte
   at the last byte of the last sector), then force the write to disk. While the
   server is running, the size is therefore not necessarily a multiple of 4,096 (seen: 562 bytes left over).
7. "Synchronous writes" option (`sync-chunk-writes`, client setting `syncChunkWrites`): file
   opened in synchronous write mode. Rust singleplayer: normal write + `fsync` on closing and at the end
   of each full save.

---

## 4. Block chunk (NBT, `region/`)

### 4.1 Root
| Key | Type | Notes |
|---|---|---|
| `DataVersion` | int | 5023 |
| `xPos`, `zPos` | int | chunk coordinates |
| `yPos` | int | lowest section (−4 at the surface: −64 / 16) |
| `Status` | string | generation state; **empty or absent string ⇒ chunk ignored** |
| `LastUpdate` | long | the world's `Time` at the chunk's last save |
| `InhabitedTime` | long | cumulative ticks with a player nearby (used for local difficulty) |
| `sections` | list | § 4.2 |
| `isLightOn` | byte | written **only** if it is 1: stored light is valid |
| `Heightmaps` | compound | § 4.5 |
| `block_entities` | list | § 4.6 |
| `block_ticks`, `fluid_ticks` | lists | § 4.7 |
| `PostProcessing` | list of lists of `short` | one list per section (index 0 = lowest section); each `short` = packed local position `x | z << 4 | y << 8` of a block whose shape is to be recomputed on full load. Empty in practice in a `full` chunk |
| `structures` | compound | `starts` (structure starts, by id) and `References` (by id, long array = packed chunks, `x` low bits / `z` high bits, containing the start) |
| `entities` | list | **only** in non-`full` chunks (entities waiting for generation) |
| a chunk's upgrade data, `blending_data`, `below_zero_retrogen` | compounds | optional, old worlds or generation blending |

26.3 statuses (generation order, read from the registry): `empty`, `structure_starts`,
`structure_references`, `biomes`, `terrain` (groups noise, surface and caves of the old
versions), `features`, `initialize_light`, `light`, `spawn`, `full`. Seen on `r.0.0` of the test
map: 882 `full`, and at the border 68 `structure_starts`, 28 `initialize_light`, 24 `terrain`, 22
`biomes`.

**Milestone 1 (no generation)**: only `full` chunks are playable; an absent or non-`full` chunk is
treated as **empty** (air, full sky light, the dimension's default biome), it is
**never rewritten** (its original NBT stays in place, byte for byte), and a block placed in it
is refused (the chunk is not loaded on the server side).

### 4.2 Section
Compound: `Y` (byte, section index), `block_states` (§ 4.3), `biomes` (§ 4.3), `BlockLight`
and `SkyLight` (optional, 2,048 bytes, § 4.4).
- A section that would have nothing to write is not written. Order: bottom to top.
- **Light** sections may exist outside the world bounds (`Y` = −5 and 20 at the
  surface): they only have `SkyLight`/`BlockLight`. To be kept.
- Block section missing within the bounds ⇒ air, default biome.

### 4.3 Paletted containers (disk)
`{ palette: [...], data: long[]? }`.
- **Entries**: blocks 4,096, index `(y · 16 + z) · 16 + x`; biomes 64 (4×4×4 cells), index
  `(y · 4 + z) · 4 + x`.
- **Palette** always local on disk (never global ids).
  - Blocks: a block's **default** state is written as a **string** (`"minecraft:air"`);
    otherwise compound `{ id: "minecraft:oak_fence", properties: { east: "true", … } }` with **all**
    properties as strings. On reading, an absent property takes its default value;
    `properties` absent ⇒ default state. Since the palette mixes strings and compounds, the strings
    are wrapped as `{ "": "minecraft:stone" }` (§ 1); a palette of strings only is a list
    of strings.
  - Biomes: list of strings (`"minecraft:plains"`).
  - Unknown id: the game replaces it with the default value (air / default biome) while
    logging; Rust singleplayer does the same **and** refuses to rewrite this chunk (otherwise loss).
- **On-disk width** `b` depending on palette size `n`, `k = ceil(log2(n))`:
  - blocks: `n = 1` ⇒ 0 bits, no `data`; `k ≤ 4` ⇒ **4**; `k` from 5 to 8 ⇒ `k`; `k > 8` ⇒ `k`
    (in memory the game then switches to direct, but the disk keeps palette + `k` bits);
  - biomes: `n = 1` ⇒ 0 bits; otherwise `b = k` (1, 2, 3, then `k`).
- **`data`**: `b` bits per entry, `floor(64 / b)` entries per long, **never straddling two
  longs**, first entry in the low-order bits; length `ceil(entries / floor(64/b))`
  (checked: palette of 17 ⇒ 5 bits ⇒ 342 longs). Different length ⇒ corrupted chunk.
  The bits are palette indices (value outside the palette ⇒ corrupted).
- Writing: repack the palette (only the values present, in order of first
  appearance when walking the entries) before writing, like the game.

### 4.4 Light
`BlockLight`, `SkyLight`: 2,048 bytes = 4,096 nibbles, same index as the blocks; even entry
in the low nibble. Absent ⇒ unknown (not "zero"). If `isLightOn` is absent, the
official game recomputes all of the chunk's light on loading.
Rust singleplayer: on reading, if `isLightOn` is 1, the stored light is sent as is to the client
(format identical to `chunks.md` § "Chunk packet with light"); after block changes,
the light is recomputed by the engine of `lighting.md`, then rewritten with `isLightOn = 1`. Acceptable
fallback: write without `isLightOn` (the official game will repair it) — to be avoided, Rust singleplayer would
then reread wrong light.

### 4.5 Heightmaps
`Heightmaps`: one long array per type. 256 columns, index `x + z · 16`, value = `y_free −
y_min` (first height above the highest retained block; 0 = empty column), width
`ceil(log2(height + 1))` = 9 bits at the surface (384 + 1), 7 per long, **37 longs**; same packing
as § 4.3. Seen: value 5 on the flat map (grass at y −60, free at −59, `−59 − (−64) = 5`).
- Types of a `full` chunk: `WORLD_SURFACE` (non-air), `OCEAN_FLOOR` (blocks movement by
  material), `MOTION_BLOCKING` (tag `blocks_motion_in_heightmap` or non-empty fluid),
  `MOTION_BLOCKING_NO_LEAVES` (same excluding foliage). The `_WG` variants only exist during generation.
- The client receives `WORLD_SURFACE`, `MOTION_BLOCKING`, `MOTION_BLOCKING_NO_LEAVES`.
- Type absent on loading ⇒ the game recomputes it; a **wrong** type stays wrong. Hence: Rust singleplayer
  keeps the maps up to date on every block change, or failing that **deletes them** from the
  modified chunk before writing.
- Recomputation (seeding): for each column, descent from the top of the highest non-empty
  section; pure air is skipped, every other block is tested against each still-open map, the
  first one that accepts it takes `y + 1`. No block accepted: bottom of the world.
- Update on a placement at (x, y, z), maps in the order `MOTION_BLOCKING`,
  `MOTION_BLOCKING_NO_LEAVES`, `OCEAN_FLOOR`, `WORLD_SURFACE`; let `h` be the current free height:
  y ≤ h − 2 → nothing; block accepted and y ≥ h → `y + 1`; block refused and y = h − 1 → descent from
  y − 1 to the first accepted block (`+ 1`), otherwise bottom of the world. Done by Rust singleplayer
  (`rc-sim/chunk.rs`), maps written as is on save (W12, 18 Oct).

### 4.6 Block entities
List of compounds: `id`, `x`, `y`, `z` (absolute), `keepPacked` (byte), `components` (item
components carried by the block, often empty), plus the type's own fields (e.g. comparator:
`OutputSignal`; chest: `Items` [{`Slot`, `id`, `count`, `components`}], `CustomName`, `Lock`,
`LootTable`…; sign: `front_text`/`back_text`, `is_waxed`). Milestone 1: Rust singleplayer **keeps the
compound it read as is** and only modifies it for the blocks it knows how to handle; a block entity whose
block disappears is deleted; a placed block that requires one creates a minimal one (`id`, `x`, `y`, `z`,
`components: {}`).

### 4.7 Scheduled ticks
`block_ticks` / `fluid_ticks`: compounds `{ i: block or fluid id, x, y, z (absolute),
t: remaining delay (ticks), p: priority (int, −3 "extremely high" … 3 "extremely low", 0
normal) }`. On loading: due time = current `Time` + `t`. On writing: `t = due time − Time`.
The order of the list is the execution order at equal due time and priority: keep it. Seen: water
(`t` 4 or 9), observer (`t` 2). Engine: `block-behaviours.md` § 1.3. Milestone 1 without
behaviours: reread and rewrite the list **as is** (delays unchanged) for unmodified
chunks; see uncertain points for modified chunks.

### 4.8 Entity regions (`entities/`)
Same `.mca` container. Root: `DataVersion`, `Position` (int[2] = chunk), `Entities` (list).
An entity: `id`, `UUID` (int[4], most significant first), `Pos` (3 doubles), `Motion` (3 doubles),
`Rotation` (2 floats: yaw, pitch), `OnGround`, `fall_distance` (double), `Fire` (short), `Air`
(short), `PortalCooldown`, `Invulnerable`, optional `CustomName`, `CustomNameVisible`, `Silent`,
`NoGravity`, `Glowing`, `Tags`, `Passengers` (list of nested entities), `data`, then the specific
fields (e.g. item frame: `Item`, `Facing`, `block_pos`, `ItemRotation`, `ItemDropChance`, `Fixed`,
`Invisible`; mobs: `Health`, `attributes`, `equipment`, `Brain`, `PersistenceRequired`…).
- The entity chunk is written **in full** on each save of its entities; if none
  remain, the location is **erased** (§ 3.3.5).
- Milestone 1: entities kept as raw NBT, rewritten as is (except those removed by the
  player, § integrated-server.md).

### 4.9 Points of interest (`poi/`)
Root: `DataVersion`, `Sections` (compound indexed by the section Y as text: `"-4"`), each
section `{ Valid: byte, Records: [{ pos: int[3], type: "minecraft:home", free_tickets: int }] }`.
The official game only rebuilds a section **if `Valid` is 0 or if it is absent** (and the
block section contains a "point of interest" state: beds, workstations, bell, beehives,
Nether portal, lodestone…). Rust singleplayer rule: when a block change adds or removes a
point-of-interest state, write the matching `poi` section with `Valid = 0` (keeping its
`Records`); the official game will fix it.

---

## 5. Players (`players/data/<uuid>.dat`)

Gzip. UUID as text with dashes. In singleplayer, the game loads the file of the UUID `singleplayer_uuid`
from `level.dat` for the owner (whatever their session UUID); Rust singleplayer does the same:
local player's UUID = `singleplayer_uuid` if it exists, otherwise the offline UUID of the username, written to
`level.dat` on save.

Useful fields (seen and read):
| Key | Content |
|---|---|
| `DataVersion` | 5023 |
| `Pos`, `Motion` (3 doubles), `Rotation` (yaw, pitch), `OnGround`, `fall_distance` | physical state |
| `Dimension` | `"minecraft:overworld"` |
| `playerGameType`, `previousPlayerGameType` | mode (int; absent ⇒ the world's default mode) |
| `abilities` | `flying`, `mayfly`, `instabuild`, `invulnerable`, `mayBuild` (bytes), `walkSpeed` (0.1), `flySpeed` (0.05) |
| `Health` (float), `AbsorptionAmount`, `HurtTime`, `DeathTime`, `Fire`, `Air` | health |
| `foodLevel`, `foodSaturationLevel`, `foodExhaustionLevel`, `foodTickTimer` | hunger (`server-gameplay.md` § 1.2) |
| `XpLevel`, `XpP`, `XpTotal`, `XpSeed`, `Score` | experience |
| `SelectedItemSlot` | held slot 0–8 |
| `Inventory` | list `{ Slot: byte 0–35, id, count: int, components? }`; 0–8 hotbar, 9–35 bag; empty slots absent |
| `equipment` | optional compound `head`, `chest`, `legs`, `feet`, `offhand`, `body` → stack `{ id, count, components? }` |
| `EnderItems` | list `{ Slot 0–26, … }` |
| `attributes` | `[{ id, base, modifiers? }]` (e.g. interaction ranges 4.5 / 3.0) |
| `active_effects`, `respawn`, `LastDeathLocation`, `RootVehicle`, `ender_pearls`, `ShoulderEntityLeft/Right`, `entered_nether_pos`, `raid_omen_position`, `recipeBook`, `warden_spawn_tracker`, `seenCredits`, `post_effects`, `Brain`, `SleepTimer`, `spawn_extra_particles_on_fall`, `current_impulse_context_reset_grace_time` | optional: kept as is |

Item stack (common format): `id`, `count` (int, default 1), `components` (components differing
from the default values, format of `item-components.md`).

Reading: `.dat`, otherwise backup copy `<uuid>_corrupted_<date>.dat` then `.dat_old`. Missing
file ⇒ new player (world spawn point, `GameType`). Writing: temporary file
`<uuid>-<random>.dat` in `players/data`, then safe replacement with `.dat_old` (§ 8.2). The
`advancements/` and `stats/` files are not touched at milestone 1.

---

## 6. `data/` data

Each file: gzip, `{ data: { … }, DataVersion: int }`, written **directly** (without a temporary
file) by the game; Rust singleplayer still uses the safe replacement (§ 8.2) without `_old`.

### 6.1 Global (`<world>/data/minecraft/`)
- `game_rules.dat`: `data` = rules `"minecraft:<name>"` → byte (boolean) or int. E.g.
  `minecraft:advance_time`, `minecraft:advance_weather`, `minecraft:keep_inventory`,
  `minecraft:fall_damage`, `minecraft:player_movement_check`, `minecraft:max_entity_cramming`…
  Rule absent ⇒ the game's default value.
- `world_clocks.dat`: per clock (`minecraft:overworld`, `minecraft:the_end`) `{ total_ticks:
  long }` (+ any pause / speed state); this is the **time of day** (`environment.md` § 2).
- `weather.dat`: `raining`, `thundering` (bytes), `rain_time`, `thunder_time`,
  `clear_weather_time` (ints, remaining ticks).
- `world_gen_settings.dat`: `seed`, `generate_structures`, `bonus_chest`, `dimensions` (generator
  per dimension). Read for information at milestone 1 (generation later).
- `scoreboard.dat`, `custom_boss_events.dat`, `random_sequences.dat`, `scheduled_events.dat`,
  `stopwatches.dat`: kept as is at milestone 1.
- `maps/<n>.dat` (one map per file) and `maps/last_id.dat` (`data.map` = last number):
  kept; format described in `maps.md` on the client side.

### 6.2 Per dimension (`dimensions/…/data/minecraft/`)
- `world_border.dat`: `center_x`, `center_z`, `size`, `lerp_target`, `lerp_time`, `safe_zone`,
  `damage_per_block`, `warning_blocks`, `warning_time`.
- `chunk_tickets.dat`: persistent tickets (`/forceload`) `{ chunk_pos: int[2], level, type }`.
- `raids.dat`: `tick`, `next_id`, raids in progress.
- `ender_dragon_fight.dat` (End).
All kept as is at milestone 1, except `world_border` (read to send it to the client).

---

## 7. Unknown fields and rewrite fidelity

Principle: **Rust singleplayer rewrites the compound it read**, replacing only the keys it
handles; any unknown key (including from a future version or a mod) is copied as is.
- Modified chunk: replace `sections` (blocks, unchanged biomes copied, light), `Heightmaps`,
  `block_entities`, `block_ticks`/`fluid_ticks` if handled, `LastUpdate`, `InhabitedTime`; keep
  `structures`, `PostProcessing`, a chunk's upgrade data, `blending_data`, etc.
- Unmodified chunk: **do not rewrite it** (not even for `LastUpdate`).
- Unhandled entities and block entities: original raw NBT.
- Write `DataVersion = 5023` wherever we write.

---

## 8. Safe writing

### 8.1 `session.lock` lock
On opening: create/open `session.lock`, write the 3 UTF-8 bytes of "☃" (U+2603) in it, force
to disk, then take an **exclusive system lock** on the file (non-blocking). Failure ⇒ the
world is already open (official game, server or other instance): refuse to open. Keep the lock
until fully closed (after the last write). To know whether a world is locked
(world list): try the lock and release it immediately; access denied ⇒ locked; file
absent ⇒ free.

### 8.2 Replacing a file
1. Write the new content to a temporary file in the **same folder** (`level<random>.dat`,
   `<uuid>-<random>.dat`) and force it to disk.
2. If the target exists: delete the old backup copy, rename the target to the backup copy
   (`level.dat_old`, `<uuid>.dat_old`).
3. Delete the target if it still exists; rename the temporary file to the target.
4. Each step is retried up to 10 times (files locked by an antivirus on Windows).
   If the final rename fails, restore the target from the backup copy.
Regions: no temporary file, but copy on write of the sectors (§ 3.3).

### 8.3 Order of a full save (like the game)
Players → chunks of each dimension (blocks, entities, poi) → `level.dat` → `data/`. On
closing: same, then closing the regions (padding, § 3.3.6), then releasing the lock.

---

## 9. Compatibility and refusals

- `level.dat` `DataVersion` ≠ 5023 ⇒ refuse to open for writing and suggest going through the
  official 26.3 game first (which migrates). Newer ⇒ refuse (a world from a future version would be
  damaged).
- Chunk with `DataVersion` < 5023 in a 26.3 world (never reloaded since the migration):
  treated as not loadable (empty), never rewritten.
- World data pack other than `vanilla` that adds registries (biomes, dimensions…):
  at milestone 1, open read-only or refuse (unknown palette ids would be
  lost). Packs that only add functions (the case of the test map) have no effect.
- Never write `WasModded = 1` nor add a brand to `ServerBrands` without an explicit decision:
  the official game would flag the world as modded.

---

## Uncertain points

- `world_clocks.dat`: only the `total_ticks` are visible on the test map; the clock
  pause and speed fields (and their name) are to be confirmed on a world where a clock was
  paused.
- Default value of an absent `spawn` (exact height) not verified; all real 26.3 worlds
  have it.
- Scheduled ticks of a chunk modified without a behaviour engine: keeping them as is may
  trigger, once reopened by the official game, a tick on a replaced block (the game generally ignores a tick
  whose block no longer matches). To be confirmed block by block when the engine arrives.
- LZ4: exact stream format (`lz4-java` "block", with `LZ4Block` headers and xxhash
  checksum) to be checked on a real file before writing a reader; no test world uses it.
- `InhabitedTime`: exact increment rule (per tick, if a player is within the simulation radius)
  not reread; no visible effect at milestone 1.
- Exact list of "point of interest" states: to be extracted from the game's data by
  the extractor rather than by hand.
