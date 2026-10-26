# Integrated server: the minimum to join and play (26.3, Rust singleplayer `rc-sim`)

Sources: reading of the decompiled server and integrated server (kept outside the repository), reworded. Context:
`docs/architecture.md` § 14. First milestone: open an **existing** world (`anvil-world.md`), move
around in it, place and break blocks, manage the inventory, let time and weather pass, save;
no generation, no mob AI.

Cross-references (not repeated here): handshake, configuration and entering the game on the client side →
`connection.md`; chunk packet contents, registries, paletted containers → `chunks.md`;
player physics and sending its position → `movement.md`; mining, placing, sequences and
prediction → `interactions.md`; tick rate (`ticking_state`), miscellaneous → `protocol-supplements.md`;
entity packets → `entities.md`; windows and clicks → `inventories.md`; clocks and time →
`environment.md` § 2; order of the phases of a world tick, scheduled ticks →
`block-behaviours.md` § 1; health, hunger, modes, respawn → `server-gameplay.md` § 1.

---

## 0. Client ↔ simulation boundary

- Same process, thread(s) dedicated to `rc-sim`, **in-memory channels** in both directions. No
  encoding, no compression, no encryption: the simulation directly produces the
  events that the client today draws from network decoding (`rc-client`: `Event`, entity
  events, already-decoded chunks), and receives the messages that `Link` serializes today (position,
  inputs, actions, clicks…). Above this boundary, the client keeps exactly its code.
- In what follows, packet names (`login`, `player_position`…) designate the corresponding
  **logical message**; their content is that of the packet described in the client specs.
- The local player is the **singleplayer owner**: the official game spares them the speed check
  (§ 4.2) and the keep-alive (§ 2.5).
- As long as the official game itself considers the connection "in memory", it sends **all**
  pending chunks at each tick (no quota, § 3.2): same rule here.

---

## 1. Opening the world (before the player)

1. Take the `session.lock` lock (`anvil-world.md` § 8.1); refusal ⇒ message "world already open".
2. Read `level.dat` (§ 2 of `anvil-world.md`), `data/minecraft/game_rules.dat`, `weather.dat`,
   `world_clocks.dat`, `world_gen_settings.dat` (seed, flat type), and per dimension
   `world_border.dat`. Refusal if the data version is not 5023 (`anvil-world.md` § 9).
3. Registries and tags: built from the jar's data (rc-data / extractor), **the same as
   those of a vanilla server** (dimensions, biomes, damage types, variants…), in the same order
   of entries (§ 2.2).
4. Worlds: overworld, Nether, End (registration order), each with its height (dimension
   type) and its `dimensions/minecraft/<name>/` folder.
5. Nothing is loaded in advance (the official game loads a radius around the spawn point at
   startup; unnecessary for milestone 1).

---

## 2. Player connection

### 2.1 Configuration phase (in the official server's order)
1. `custom_payload` server brand (`minecraft:brand`, "vanilla" on the official side; ours may
   say "rustcraft", with no effect on the world).
2. (`server_links`: never in singleplayer.)
3. `update_enabled_features`: world flags (`level.dat` → `enabled_features`, otherwise
   `minecraft:vanilla`).
4. Negotiation of known packs (`select_known_packs`) then, for **each synchronized registry**,
   `registry_data`, then `update_tags`. In memory, the negotiation may be skipped: the simulation
   hands the client the full content of the registries (this is the "no known pack" case of
   `connection.md`).
5. (Code of conduct, server resource pack: never in singleplayer.)
6. **Spawn preparation** (§ 2.3) — the configuration waits until it is finished.
7. `finish_configuration`; the client answers with its acknowledgement ⇒ play phase (§ 2.4).
Meanwhile the client sends its information (language, view distance, skin parts, hand…):
keep them (requested view distance, § 3.1; skin parts and hand for the player's data).

### 2.2 Registries and tags
- Registries synchronized in 26.3 (fixed list of the game, about thirty): notably `dimension_type`,
  `worldgen/biome`, `damage_type`, `chat_type`, `enchantment`, `painting_variant`, `wolf_variant`
  and the other animal variants (+ sound variants), `trim_pattern`, `trim_material`,
  `banner_pattern`, `jukebox_song`, `instrument`, `dialog`, `timeline`, `world_clock`, …
- Order of the entries = numeric identifier in game (container biomes, dimension types of the
  `login`, variants in entity data): **must** be that of a vanilla server.
- `update_tags`: for each registry carrying tags (blocks, items, fluids, entity types,
  game events, and data registries), each tag with the **flattened** list of numeric
  identifiers (`#other_tag` references resolved). Block tags are
  essential to the client's physics (climbing, friction, `blocks_motion_in_heightmap`…).

### 2.3 Spawn preparation
1. Read the owner's `players/data/<uuid>.dat` (`anvil-world.md` § 5).
2. Dimension = the file's `Dimension` if it exists, otherwise that of `spawn` in `level.dat`, otherwise the
   overworld. Position = the file's `Pos`, otherwise search for a safe spot around `spawn.pos`
   (`server-gameplay.md` § 1.5). Rotation = the file's `Rotation`, otherwise `spawn.yaw`/`spawn.pitch`.
3. Load the chunks within a **radius of 3 chunks** around the spawn chunk (temporary
   ticket), wait for loading to finish.
4. Create the player entity (server-unique entity id), apply the rest of the file
   (inventory, health, mode: `playerGameType` otherwise the world's `GameType`, abilities…).

### 2.4 Entering the game (official order, in a single send)
1. `login`: entity id, hardcore flag (`hardcore`), keys of all dimensions, max players
   (8 in singleplayer), view distance, simulation distance, reduced debug info (rule
   `reduced_debug_info`), death screen (= not `immediate_respawn`), limited crafting (rule
   `limited_crafting`), then spawn info: dimension type (index), dimension
   key, **hashed seed** (first 8 bytes of the SHA-256 of the seed's 8 little-endian bytes,
   read little-endian), mode, previous mode, debug (false), **flat** (generator
   `minecraft:flat` in `world_gen_settings`), last death, portal cooldown, **sea level**
   (63 in the normal overworld, −63 in a flat world); online mode, enforced secure chat: false.
2. `change_difficulty` (difficulty, locked).
3. `player_abilities` (flags, flying speed, field of view).
4. `set_held_slot` (`SelectedItemSlot`).
5. `update_recipes` (synchronized item sets, stonecutter recipes) — may be
   empty in milestone 1.
6. `entity_event` permission level (24 + level, 0–4; 4 if `allowCommands`), then `commands`
   (tree: root only in milestone 1).
7. Initial recipe book, scoreboard (`scoreboard.dat`: teams, displayed objectives).
8. System message "joined the game" (yellow).
9. **`player_position`** (teleport no. 1: absolute position and rotation, zero movement); the
   server waits for the acknowledgement (§ 4.1).
10. `player_info_update` (all players, self included: profile, mode, latency, listed).
11. World information: `initialize_border`, **full `set_time`** (game time + state of
    all clocks), `set_default_spawn_position`, then if raining: `game_event` 1 (rain
    start), 7 (rain level), 8 (thunder level); then `game_event` 13 (start of chunk
    loading); then `ticking_state` (tick rate, frozen).
12. Adding the player to the world: chunk tracking (§ 3) ⇒ `set_chunk_cache_center` then chunks
    in batches at the next tick; entity tracking (§ 5).
13. Persistent boss bars, active effects (`update_mob_effect`), inventory:
    **`container_set_content` window 0** (46 slots + carried item, state number).
On the first tick after that: `set_health` (health, hunger, saturation), `set_experience`,
`set_entity_data` and `update_attributes` of the player for their non-default values.

### 2.5 Keep-alive
The official server sends `keep_alive` every 15 s and cuts the connection without a reply, **except for the
singleplayer owner**. In memory: send nothing. LAN guests (`lan/mod.rs`):
challenge = clock in ms, smoothed latency (old × 3 + measurement) / 4, `disconnect.timeout` if the
previous challenge was left unanswered (or if the reply does not match); everyone's latencies sent
every 600 ticks (`UPDATE_LATENCY`), the host's at 0.

### 2.6 "Loaded" player
The player's actions (movement, mining, use, world clicks) are **ignored** until the
client has sent `player_loaded` or 60 ticks have elapsed since entering (same
rule after a respawn).

---

## 3. Chunks around the player

### 3.1 Tracked area
- Effective view distance = `clamp(distance requested by the client, 2, server distance)`;
  in singleplayer, server distance = `max(2, client render distance)`, and simulation distance =
  `max(2, client setting)` (changes ⇒ `set_chunk_cache_radius` /
  `set_simulation_distance`).
- Chunk `(x, z)` tracked if, with `dx = max(0, |x − cx| − 1)` and `dz = max(0, |z − cz| − 1)`
  (center = the player's chunk), `dx² + dz² < view²`.
- When the player's chunk changes (or the distance): `set_chunk_cache_center` if the center has
  changed, then difference between old and new area: entering ⇒ "to send", leaving ⇒
  removed from the queue if they were in it, otherwise `forget_level_chunk`.
- A chunk is only sent once **loaded and complete**; in milestone 1, a missing or non-`full`
  chunk is sent **empty** (air sections, default biome, sky light 15 above the ground…)
  so that the client does not end up with a hole: to be decided (uncertain points).

### 3.2 Sending in batches and acknowledgements
- Queue of pending chunks per player. At the end of the server tick, for each player, if the number
  of unacknowledged batches < maximum (1 at the start, 10 after the first acknowledgement):
  - quota = `min(quota + wanted, max(1, wanted))` (wanted = 9 at the start); nothing if quota < 1;
  - **in-memory connection: take the whole queue**, sorted by distance to the player (otherwise: the
    `floor(quota)` closest);
  - `chunk_batch_start`, one `level_chunk_with_light` per chunk (blocks, biomes, client
    heightmaps, block entities with their update data, stored light), `chunk_batch_finished`
    (count); quota −= count.
- Acknowledgement `chunk_batch_received(wanted)`: unacknowledged batches − 1 (≥ 0); wanted = `clamp(wanted, 0.01,
  64)` (NaN ⇒ 0.01); if no batch is pending any more, quota = 1; maximum = 10.

### 3.3 Block changes toward the client
Each modified block is recorded per section. During the "chunk source" phase of the world tick
(`block-behaviours.md` § 1.2 step 9), for each modified and tracked chunk:
1. if the light changed: `light_update` of the sections concerned (to the players tracking the
   chunk, border included);
2. per section: a single block ⇒ `block_update`; several ⇒ `section_blocks_update`; then, for
   each block that carries a block entity, `block_entity_data`.

---

## 4. The player in the tick

### 4.1 Server teleports
- Each `player_position` carries a number (incremented, wraps to 0 before the maximum of an `i32`); the
  server places the player at the destination and remembers an "expected position".
- `accept_teleportation(n)`: if `n` = last number and expected position present ⇒ the player is
  placed at the expected position (it becomes the "last good one"), the wait is lifted, then the
  packet is processed as a movement. Non-finite values ⇒ disconnect.
- While a teleport is pending: received movements only change the rotation, and the
  teleport is **resent** if more than 20 ticks have elapsed since the last send.

### 4.2 Validating movements (`move_player_*`)
In order:
1. NaN coordinate or non-finite rotation ⇒ disconnect "invalid movement". Two packets with a
   position in the same tick (between two `client_tick_end`) ⇒ disconnect.
2. Player not yet loaded (§ 2.6) ⇒ ignored. Teleport pending ⇒ § 4.1.
3. Target clamped: x, z within ±3·10⁷; y within ±2·10⁷; rotations brought within ±180°.
4. Passenger: only the rotation is taken.
5. Asleep: displacement² > 1 ⇒ snap back in place.
6. **Speed check** — skipped for the singleplayer owner (and if the rule
   `player_movement_check` is false, or while gliding if `elytra_movement_check` is false):
   `d² = |target − tick start|²`, `expected = |server velocity|²`; if `d² − expected > 100 × n`
   (300 while gliding; `n` = movement packets received since the previous tick, 1 if > 5) ⇒
   snap back to the server position.
7. Server movement: the server replays the movement `target − last good` with its collisions
   (`movement.md` § 6), jump detected if the player was on the ground, no longer is and rises.
8. Remaining difference = target − position obtained, **the vertical difference always being ignored** (the official
   game has an always-true condition here). Difference² > 0.0625 outside creative/spectator/sleeping/
   grace period after an impulse ⇒ "failure".
9. Accepted if: no collision (entity without physics), or asleep, or [no failure **or** the old box
   was already colliding] **and** the new box touches no shape that it did not touch
   before. Accepted ⇒ position = exact client target, client ground / horizontal collision,
   fall damage, statistics, "last good" = target. Refused ⇒ `player_position` to the
   position at the start of processing (snap back).
10. "Floating": if the player has no flight permission, no levitation, no glider, touches nothing below,
    is not descending (dy ≥ −0.03125) and has no block around, a counter goes up each tick; beyond
    80 ticks (weighted by gravity) ⇒ disconnect "flying". **The integrated server always
    allows flight** (only the dedicated server reads `allow-flight`): this check therefore never applies
    in singleplayer — do not implement it.
- `client_tick_end` resets "position received this tick" and the known movement (zero if no
  movement was received).

### 4.3 Player tick (in the "network" phase of the server tick)
1. Send `block_changed_ack(s)` if a sequence is pending (§ 6.3), only once.
2. If the server is not paused: reference position = current position, player tick
   (health, hunger, effects, deferred mining § 6.1, synchronization of the open window: modified
   slots ⇒ `container_set_slot`), then the player is **put back** at the tick-start position
   (movements only come from the client); "player loaded" countdown; "floating" check.

---

## 5. Entities (milestone 1)

- Entities are read from the `entities/` regions (`anvil-world.md` § 4.8) with their chunks.
- An entity is shown to the player when its chunk has been sent to them and it is within tracking range
  (range of its type, bounded by the view distance): `add_entity`, `set_entity_data` (non-default
  values), `update_attributes` and `set_equipment` for mobs, `set_passengers`; removal
  by `remove_entities` when it leaves.
- **Milestone 1: frozen entities** (no tick, no AI, no physics); only those whose
  NBT → entity data conversion is written are shown; all are rewritten as they are.

---

## 6. Placing and breaking (minimal rules)

### 6.1 Breaking (`player_action` 0 start, 1 abort, 2 stop)
Processed only if the player is loaded; in all cases (except range), the action is acknowledged
(§ 6.3).
- Block out of range (eyes → block box distance ≥ interaction range + 1): nothing, except an
  abort which cancels the mining in progress.
- Above the world ⇒ `block_update` of the true state.
- **Start**: interaction forbidden ⇒ `block_update`. Creative (`instabuild`) ⇒ immediate break.
  Adventure without right ⇒ `block_update`. Otherwise: progress = player's mining speed on this block
  (`server-gameplay.md` § 3.2); if progress ≥ 1 ⇒ immediate break; otherwise start of the server countdown
  (start tick, position, face; `block_destruction` crack at stage `progress × 10` to the other players).
- **Stop**: on the same position, progression = progress × (ticks elapsed + 1); ≥ 0.7 ⇒ break; otherwise
  deferred break: the server continues alone and breaks as soon as the progression reaches 1.
- **Abort**: end of the mining, crack cleared (−1).
- During mining, each tick: progression, crack if the stage changes, world event
  2020 every 4 ticks otherwise 2019 (hit sounds), not for the miner themselves.
- Break: the held item must be able to break (sword in creative: no); operator block without
  right ⇒ refused; removal of the block (water if waterlogged), destruction effects for the other players
  (event 2001, except to the miner), tool wear, loot if suitable tool and not creative (in milestone 1
  with no item entities: **no loot**, to be decided). Break failure ⇒ `block_update`.

### 6.2 Place / use on a block (`use_item_on`)
1. Acknowledge the sequence (§ 6.3). Ignored if the player is not loaded or the item is not enabled.
2. Targeted block in range (range + 1) and hit point within 1.0000001 of the block's center on each
   axis, otherwise ignored (logged).
3. Height outside the world ⇒ build limit message (action bar).
4. If no teleport is pending and interaction is allowed: same sequence as the client's prediction
   (`interactions.md` § 3.2) — block reaction, then item use; for a block item:
   position (replaceable ⇒ the clicked one, otherwise the neighboring one), placement state, survival, obstruction
   (no entity whose box blocks, the player included), placement, "placed" sound to the **other** players,
   count −1 outside infinite materials, block entity created if the block carries one (stack data
   applied). **The same placement code as the client's prediction** must serve both.
5. Always afterwards: `block_update` of the clicked position **and** of its neighbor on the face side (this is
   what corrects a wrong prediction).
6. Top face at the top of the world, or bottom at the bottom, on a block item that did nothing ⇒ limit
   message.

### 6.3 Sequence acknowledgements
`ack = max(received sequences)` during the tick; sent once at the start of the next player tick
(§ 4.3) — hence **after** the `block_update`s broadcast during the world tick. Actions acknowledged:
start / stop / abort of mining, `use_item_on`, `use_item`.

### 6.4 Inventory
- Window clicks (`container_click`): replay the rules of `inventories.md` § 2 on the server side;
  different state number ⇒ resend the whole content (`container_set_content`); otherwise apply and
  resend only the slots that differ from what the client announced.
- Creative (`set_creative_mode_slot`): if the player has infinite materials, slot 1–45 valid and
  quantity ≤ maximum stack ⇒ the slot takes the stack; slot < 0 ⇒ item dropped (rate-limited).
- `set_carried_item` (0–8), offhand swap (`player_action` 6), drop (3, 4).
- End of the player's tick: window differences ⇒ `container_set_slot`.

---

## 7. Tick loop

- **20 ticks per second** (50 ms; changeable with `/tick rate`). Next tick deadline +=
  50 ms; if the server is more than 1 s + 20 ticks behind, it skips the lag (warning at most
  every 10 s); otherwise it catches up by chaining ticks without waiting.
- One tick: (1) process the queued messages received from the client; (2) server tick
  (`block-behaviours.md` § 1.1: functions, clocks, time sync every 20 ticks,
  each world § 1.2, network = player ticks § 4.3, player list); (3) **sending of
  chunks** to each player (§ 3.2) then flushing the sends; (4) autosave countdown
  (§ 8).
- Time: `Time` += 1 per tick (overworld); clocks: if the rule `advance_time` is true,
  each clock advances according to its speed. `set_time`: full on login and at each clock
  change (command, sleeping), otherwise every 20 ticks with the game time only (the client's clocks
  advance on their own).
- Weather (if the dimension has weather and rule `advance_weather`): if `clear_weather_time` > 0,
  it decreases and rain/thunder are forced to false (counters reset to 0 or 1); otherwise for thunder then
  rain: counter > 0 ⇒ −1, at 0 it flips; zero counter ⇒ uniform draw (thunder: active
  3,600–15,600, wait 12,000–180,000; rain: active 12,000–24,000, wait 12,000–180,000).
  Rain and thunder levels ±0.01 per tick toward 1 or 0; change ⇒ `game_event` 7 / 8;
  rain start / end ⇒ `game_event` 1 / 2 then 7 and 8.

---

## 8. Saves

- **Automatic**: first countdown 6,000 ticks, then `max(100, tick rate × 300)` ticks
  (5 minutes). Full save without waiting for disk writes to finish.
- **Full** (official order): players (`players/data`) → chunks of each dimension (only
  the modified chunks and entities, `anvil-world.md` § 7) → `level.dat` (with `singleplayer_uuid`) →
  `data/` files. To be done off the tick thread (snapshot of the modified data, write on
  a worker thread), without blocking the game.
- **Entering pause**: full save (§ 9).
- **Closing** (leaving the world): save and remove the player, unload the chunks,
  full save **waiting** for the writes to finish, close the regions (padding), data,
  then release the lock. The client waits for the end before going back to the menu.
- Chunks unloaded during play (the player moves away): written if modified.

---

## 9. Singleplayer pause

- The server is **paused** when the client is paused (pause screen or another "pausing"
  screen open, in singleplayer not opened to the network) **or** there is no player. The client
  signals this flag to the simulation at each change.
- Entering pause ⇒ full save. While paused: only message processing and the "connection"
  part run (no world, player, clock or weather tick); the statistic
  "total time in the world" advances anyway.
- Leaving pause ⇒ `set_time` (game time) to resynchronize, then normal resumption.
- The client freezes its own tick on its side when paused (existing client behavior).

---

## 10. Dimensions and portals (S6)

- All the world's dimensions (the game's three, then those of the settings) can be loaded
  together; the player's is active, the others are parked with their chunks, tick queues,
  entities, furnaces and generator. Save: each dimension writes its modified chunks. A
  parked dimension does not tick, except during a portal ticket (300 ticks after an arrival):
  falling items and blocks continue there (mobs: no).
- Nether portal: lit by a fire placed in an empty obsidian frame (2 to 21 wide, 3 to
  21 tall, X axis preferred), including by lighter and fireball; broken as soon as a block of the frame
  is missing (shape update). Player delay: rule `players_nether_portal_default_delay` (80) or
  `..._creative_delay` (0) if invulnerable; other entities: immediate; return delay 10 (player)
  or 300. Arrival: coordinates × scale (1/8 toward the Nether), nearest existing portal within
  a square of 16 (toward the Nether) or 128 blocks (distance then lowest Y), otherwise creation:
  spiral of radius 16, columns scanned from the `MOTION_BLOCKING` height, complete frame preferred,
  otherwise partial, otherwise 3×2 obsidian platform in the air (Y 70 at least). Relative position in
  the portal kept, yaw + 90° if the axes differ, velocity kept. Packets: `respawn`
  (data kept), difficulty, operator level, position, abilities, border, times, spawn
  point, rain (overworld), waiting for chunks, inventory, sound 1032.
- End: 12 eyes placed on the frame (5×5 pattern searched like the game) open the 3×3 portal
  (global event 1038). Arrival at (100.5; 49; 0.5), yaw 90, 5×5 obsidian platform
  dug out (blocks broken). Portal in the End: credits the first time (event 4), then respawn
  point; items: world point. Gateways: 40-tick delay, exit created at ~1024 blocks
  (islet if the chunk is empty), return gateway placed 10 blocks above.
- Respawn: remembered point looked up in ITS dimension (anchor charged where it works, bed
  where the bed rule allows it, forced point), otherwise the world point of the spawn dimension;
  anchor: glowstone charge, one charge consumed per respawn. Bed outside the overworld and
  anchor outside the Nether: explosion of power 5 with fire (`bad_respawn_point`).
- Known differences: no `poi/` files written (the official server rebuilds them from the
  blocks); gateway without age or periodic radius; explosions without loot; mobs not simulated
  in a parked dimension; the client's loading screen on `respawn` not displayed.

## 11. Opening to the local network (S9, `rc-sim/src/lan/`)

- Trigger: "LAN" button of the world options (port, guest commands, enforced mode) or `/publish`. The server listens on TCP on all interfaces (`RUSTCRAFT_LAN_BIND=127.0.0.1` to limit it to the machine) and announces the world every 1.5 s on 224.0.2.60:4445: `[MOTD]<player> - <world>[/MOTD][AD]<port>[/AD]`. Message to the host: `menu.multiplayerOptions.publish.started.lan` (copyable port), closing `menu.multiplayerOptions.publish.stopped`.
- Guest connection: handshake, status (JSON: version, players, description), **authenticated and encrypted** connection like the game (`lan/auth.rs`: 1024-bit RSA keys created at opening, 4-byte challenge, AES/CFB8, `hasJoined` at the `verify` address of the discovery document; unrecognized guest or services unreachable: entry anyway with the offline profile, like the game's singleplayer; same UUID already in game: the old connection receives `duplicate_login`; host's username: refused — the game, for its part, would kick the host and close the world), status like the game (description as plain text, at most 12 players from a drawn offset then shuffled, "Anonymous Player" for whoever refuses to be listed, the world's 64 × 64 `icon.png` icon, redone every 5 s), compression at threshold 256, configuration (brand, features, known packs `core`, registries, tags), then play. An opened world no longer pauses.
- Processing: the server stays single-player in its code; each player's state (position, inventory, health, menus, advancements, statistics…) is swapped with the host's for the time needed to process their packets and their tick (`ctx.rs`). "World" events (blocks, entities, time, weather, sounds): to everyone; the others: to the current player. Guests receive encoded packets (`encode.rs`), the host the in-memory events.
- Players seen: entry in the player list, "player" entity with position, rotation, equipment; `multiplayer.player.joined` / `left` in yellow. `players/data/`, `players/advancements/`, `players/stats/` files of each guest written when they leave and at each save.
- Tick: the world systems (time, weather, night skip, functions, blocks, entities, command block minecarts) run once per tick, in the host's context; the night skip counts all players (in bed and in deep sleep, spectators excluded). Each guest then has their own tick: mining, survival, held item and combat memory, parrots, swimming, stingers, mount, mob pushing, contact with slimes and pufferfish, picked-up arrows, spin attack, held maps (send state specific to each holder), advancements. Player body swapped with the context: parrots, swimming, stingers, mount, speed seen by slimes.
- Known differences: guests in the same dimension as the host only (no portals), mobs tracked around the host, no item pickup by guests, no fishing rod for guests (a single hook per world), guests' chat relayed unsigned ("not secure" mark; the game relays signed chat), guest commands at the chosen level (0 or 4).

## 12. Decorations (G5-F08, `rc-sim/src/decoration/`)

Paintings, item frames, armor stands, cushions, mannequins: the NBT compound stays the truth, a
light registry carries the unsaved state. Simulated only in an "entity ticking" chunk
(simulated, 8 neighbors loaded). Player seated on a cushion: they get off on `/tp`, on
`/spreadplayers`, on a pearl, on a chorus fruit, on respawn, on a dimension change
and on switching to spectator, as from a mount; the player's re-mount delay is reset to 0 at
each dismount (the game overwrites it after the mount's 60). Comparator behind a
conductive block: reads the frame facing it (rotation % 8 + 1 with an item, 0 without).

Known differences (not faithful):
- Invulnerable mannequin: in the game it is a living entity (health 20, base attributes) that
  takes hits and explosions, recoils if not immobile and dies. Here the hit is
  absorbed and the explosion ignores it. Cause: the living-entity damage chain only
  covers mobs, the mannequin is an NBT compound; to be wired up with it.
- Item held on a stand or a mannequin: only the named tag acts (items
  to equip on click are not tried). Mobs do not accept the tag yet.
- Direct source of an End crystal explosion: never "in water" (crystals without fluid
  state); the game then spares the hanging items.

## Uncertain points

- Exact order of the entries of the synchronized registries and exact list of the registries: to be taken from
  the reference capture of a vanilla 26.3 server (`rc-testdata`) rather than assuming the alphabetical
  order of the files.
- Missing or non-`full` chunks in milestone 1: send an empty chunk (the player can fall into the
  void) or send nothing (the client shows a hole and blocks the player at the edge, like an unloaded
  chunk) — to be decided; the official game, for its part, generates them.
- Loot on break and item entities: without entity simulation, no loot in milestone 1;
  to confirm with the roadmap (survival unusable without it).
- NBT → entity data conversion (`set_entity_data`) per type: work specific to each type, not
  specified here (`entities.md` gives the client-side indices).
- Minimal content of `update_recipes` and of the recipe book accepted by the client without error:
  to verify on our client (empty lists expected to work fine).
- `player_info_update` of the local player in singleplayer: the displayed latency and the skin properties (singleplayer
  has no online account) — take what the client expects for its own model.
