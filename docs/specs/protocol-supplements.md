# Accuracy packets: protocol supplements (26.3, protocol 777)

Sources: reading of the official 26.3 client and server, report `packets.json`. Covers section A
of `docs/gaps-protocol.md`. What is already described elsewhere is not repeated:

- frames, compression: `transport.md`; handshake, login, configuration, keep-alive,
  configuration and play ping/pong, code of conduct (response): `connection.md`;
- order of a client tick, sending the position: `movement.md` §1; `connection.md` "Client tick";
- batched entity pairing, interpolation cycle: `entities.md` (§ packets, §4);
- particle codec: `particles.md` §1.1; sound codec (registry reference, 0 = direct):
  `entities.md` (`sound_entity`); text components in NBT: `chat.md`;
- `servers.dat` storage (`acceptTextures`, `acceptedCodeOfConduct`), connection screen and
  transfer (intention 3): `title-screen-servers.md`; difficulty screen, dialog button
  of the pause menu: `option-menus.md`.

Types: `varint`, `i32`, `i64`, `f32`, `f64`, `bool`, `string(n)` (VarInt length + UTF-8, n
characters max, 32767 by default), `id` (identifier `namespace:path` as a string), `uuid` (2 × `i64`),
`vec3d` (3 × `f64`), `opt<T>` (`bool` present then T), `list<T>` (VarInt count then elements),
`bytes(n)` (VarInt length ≤ n then bytes), `nbt` (unnamed network tag, see `chunks.md`),
`text` (text component in NBT). Numeric ids: `packets.json`.

## 0. General rules of the official client

- **When applied**: the network thread decodes; most packets are queued and
  applied on the main thread **once per frame, before the ticks of that frame**, all those
  waiting at once, in arrival order. Exceptions handled right away on the network thread:
  `keep_alive` (reply deferred if the window is frozen, abandoned after 1 min), `pong_response`,
  `custom_payload` ignored (§13), start/end of chunk batch, `update_enabled_features`, `reset_chat`.
- **Processing error** (exception during processing, malformed packet, size exceeded):
  disconnect "disconnect.packetError"; the game writes a report `debug/disconnect-<date>-client.txt`
  and, if the server supplied a `report_bug` link (§11), offers it on the disconnect screen.
- Packets that target the player or the local world assume they exist; sent before
  `login`, they cause an error → disconnect. Rustcraft: ignoring without crashing is acceptable,
  but never answer in place of the official client.

## 1. `bundle_delimiter` (play 0x00, no field)

- One delimiter opens a batch, the next closes it. The packets received between the two are set aside
  without being applied; on closing, the batch becomes **a single packet** of the queue, applied in one
  block: each sub-packet is processed in sequence, in order, in the same frame, before the tick.
  No tick, no render sees an intermediate state.
- **Limit**: 4096 packets in a batch. The 4097th → decode error → disconnect.
- A "terminal" packet (phase change: `start_configuration`, end of configuration) inside a
  batch, or that would open a batch → error → disconnect. Batch not closed: its packets stay
  pending indefinitely (no timeout).
- Batches sent by the vanilla server: entity pairing (`entities.md`); velocity of a
  powered projectile (`set_entity_motion` + `projectile_power`). Paper may send others: the
  mechanism is generic.
- No response.

## 2. Server rate: `ticking_state` (play 0x82), `ticking_step` (play 0x83)

Fields:
- `ticking_state`: `rate: f32` (ticks/s), `frozen: bool`.
- `ticking_step`: `steps: varint` (number of ticks to run during the freeze).

Client state (one per world; **reset to 20 / not frozen / 0 steps on every new world**, login,
respawn, dimension change — the server sends both packets again right after, with the world
info):
- `rate = max(received, 1.0)`; `ms_per_tick = 1000 / rate`.
- `frozen`, `steps_remaining`.
- `runs_normally` (recomputed at the start of each client tick outside pause):
  `= !frozen || steps_remaining > 0`; then if `steps_remaining > 0` → `steps_remaining -= 1`.
  Packets ignored if no world exists yet.

Exact effects:
1. **Client tick frequency**: target duration of a tick = `max(50 ms, ms_per_tick)` if
   `runs_normally`, otherwise 50 ms. The client **never ticks faster than 20 Hz**: a sped-up
   server (`/tick rate 40`) changes nothing on the client side; a slowed-down server (`/tick rate 10`)
   slows down the whole client tick (100 ms), including the local player, interpolation and partial
   rendering. (Correction: `entities.md` §4.1 "or more if `ticking_state` speeds up" is wrong.)
2. **Frozen (`runs_normally` false)** — an entity is "frozen" if it is not a player and
   carries no player:
   - frozen entities no longer tick (no old position, no interpolation, no walk
     animation, no logic); at render time, the tick fraction is **1.0** (current position, no
     smoothing). The local player, other players and players' mounts continue
     normally (player physics intact, they can walk, fly);
   - if the camera follows a frozen entity, its tick fraction is also 1.0;
   - stopped: animated textures, random block effects (`animateTick`), particle engine,
     block entities, world border, advance of world clocks, weather effects, clearing
     of mining cracks, boss darkening, item activation (totem), minecart rolling sound,
     compass oscillation;
   - continue: sound (music, received sounds), HUD, chat, position sending, `client_tick_end`.
3. Displays: effect durations (inventory) and demo timer converted with `rate` (seconds =
   ticks / rate); F3: server line suffixed "(frozen)" or "(frozen - stepping)" if
   `steps_remaining > 0` (§13).
- No response.

## 3. `explode` (play 0x24)

Fields, in order:
1. `center: vec3d`
2. `radius: f32` (explosion power)
3. `block_count: i32` (blocks destroyed; used as a weight for the particles)
4. `knockback: opt<vec3d>` (present only for the player hit)
5. `particle: particle` (VarInt type + options; `explosion` or `explosion_emitter`)
6. `sound: sound` (registry reference, 0 = direct)
7. `block_particles: list<{ particle, scale: f32, speed: f32, weight: varint }>`
8. `play_sound: bool` (false if the source is silent)

The server sends the packet to each player within 64 blocks of the center; the knockback is only filled in
for that player if they were pushed (spectators and flying creative players excluded on the server side).

Application, in order:
1. If `play_sound`: local sound at the center, "blocks" category, volume 4.0, pitch
   `(1 + (r1 − r2) × 0.2) × 0.7` (r1, r2 drawn from the client world's random generator),
   no distance delay.
2. One `particle` at the center, speed (1.0, 0, 0).
3. If the list is not empty: store (center, radius, block_count, list) for the **next world
   tick**. At that tick, if the particles option ≠ "All": forget everything. Otherwise:
   `total = Σ block_count` of the stored explosions, `n = min(total, 512)`; n times: pick an
   explosion at random weighted by `block_count`, then:
   - direction `d` = normalise(3 floats in [−1, 1)); `r = cbrt(random) × radius`; `p = center + d·r`;
   - if the block at `p` is air: `speed = 0.5 / (r/radius + 0.1) × random × random + 0.3`;
     pick an entry of the list at random weighted by `weight`; particle at
     `center + d·r·scale`, speed `d × speed × entry_speed`.
   Then empty the memory.
4. If `knockback` present: **added as is to the local player's velocity** (`velocity += knockback`),
   only if the three components are finite. No reset, no accounting for the
   on-ground state: the next tick's physics does the rest.
- No response. The destroyed blocks arrive through the usual block packets.

## 4. `player_rotation` (play 0x4A)

Fields: `yaw: f32`, `yaw_relative: bool`, `pitch: f32`, `pitch_relative: bool`.

1. `yaw' = (relative ? current_yaw : 0) + yaw`;
   `pitch' = clamp((relative ? current_pitch : 0) + pitch, −90, 90)`.
   Non-finite value: ignored (rotation kept).
2. Apply to the local player; stored pitch = `clamp(t mod 360, −90, 90)`; yaw without
   wrapping. The **old** values (interpolation) receive the new ones (no smoothing).
   The velocity is not rotated.
3. **Immediate response**: `move_player_rot` (yaw, pitch, `on_ground = false`, `collision = false`) —
   flags always false, whatever the real state. The next tick will send the rotation back normally
   if it differs from the last one sent by the movement logic.

## 5. `player_look_at` (play 0x48)

Fields: `from: varint` (0 feet, 1 eyes), `x, y, z: f64`, `to_entity: bool`, then if true:
`entity: varint`, `target_anchor: varint` (0 feet, 1 eyes).

1. Target = fixed position; if `to_entity` and the entity exists on the client side: its position (+ eye
   height if anchor "eyes"); unknown entity → the packet's fixed position.
2. Origin = the player's feet, or feet + eye height.
3. `dx, dy, dz` = target − origin; `h = √(dx² + dz²)`;
   `pitch = wrap(−atan2(dy, h) in degrees)`, `yaw = wrap(atan2(dz, dx) in degrees − 90)`
   (wrap into [−180, 180)); head yaw = yaw; old values = new ones.
- **No response**; the rotation will go out with the next position send.

## 6. `set_camera` (play 0x5F)

Field: `entity: varint`.

- Entity unknown on the client side → ignored. Otherwise the camera follows this entity (position at the eyes,
  the entity's rotation, interpolated). `entity` = local player → back to normal.
- The server sends it when a spectator enters an entity (click) and to leave it; it also
  teleports the player onto the entity every tick and takes them out when the client signals
  "sneaking" in `player_input` (or on the entity's death).
- Rendering: image effect in first-person view: creeper → `creeper`, spider → `spider`,
  enderman → `invert` (`post_effect` shaders); other entity or third person → none; re-evaluated on every
  F5 change. The followed entity is not drawn in first person; the local player is only drawn
  if they are the camera. Aiming ray cast (aimed block/entity) from the camera. F3
  (position, biome, light…) reads the camera entity.
- Controls when the camera is not the player: movement keys are **not**
  applied to the player (no movement), the player's position sending is **suppressed**;
  `player_input` is still sent on every change (this is how "sneaking" makes them exit);
  `client_tick_end` continues. The mouse still rotates the local player.
- No response. A new world (respawn, dimension) puts the camera back on the player.

## 7. Vehicles: `move_vehicle` (play 0x3A client-bound, 0x22 server-bound), `paddle_boat` (0x23)

**Local authority**: the client drives the player's root vehicle if the passenger who controls that
vehicle is the local player: boat (1st player passenger), saddled equids and camels
with a player as 1st passenger, saddled pig + player holding a carrot on a stick, saddled strider +
player holding a warped fungus on a stick, happy ghast wearing a harness (body
armor) outside the standstill pause, saddled nautilus. Minecarts: never.

- Client-bound `move_vehicle`: `position: vec3d`, `yaw: f32`, `pitch: f32`.
  1. If the player is not a passenger or if the root vehicle is not under local authority →
     ignored.
  2. If the distance between the target and the vehicle's known target position (current
     interpolation target, otherwise position) is > 1e-5: cancel the current interpolation, place the vehicle
     exactly (position; yaw `mod 360`; pitch clamped to ±90; old values = new ones).
  3. **Always reply** with a server-bound `move_vehicle` with the current state.
- Server-bound `move_vehicle`: `position: vec3d`, `yaw: f32`, `pitch: f32`, `on_ground: bool`.
- Regular sending (player synchronisation, every tick, world loaded): if the player is a passenger,
  `move_player_rot` (yaw, pitch, on_ground, horizontal collision) **instead of** sending the position,
  then, if the root vehicle is under local authority, `move_vehicle` with its position, then
  the possible sprint packet.
- `paddle_boat`: `left: bool`, `right: bool`. Sent **on every tick** of the boat under local
  authority, during the entity tick (so before the same tick's `move_vehicle`). Left paddle =
  `(right && !left) || forward` ; right paddle = `(left && !right) || forward` (player's
  keys). Boat physics: out of scope (vehicles spec to be written).
- Server corrections (vanilla), useful to understand the replies: non-finite position →
  kick "invalid_vehicle_movement"; distance² from the last valid point − speed² > 100 →
  correction `move_vehicle`; after server simulation, distance² > 0.0625 and collision → correction;
  player who sends without controlling the vehicle → correction at most once per second (or if the gap
  > 1 block); vehicle floating in the air for 80 ticks without flight allowed → kick.

## 8. Cookies: `cookie_request` (login 0x05, configuration 0x00, play 0x15), `store_cookie` (configuration 0x0B, play 0x7A)

- `cookie_request`: `key: id`. **Immediate response** `cookie_response` (login 0x04,
  configuration 0x01, play 0x15): `key: id`, `value: opt<bytes(5120)>` — value stored for
  this key, absent otherwise.
- `store_cookie`: `key: id`, `value: bytes(5120)` → replaces the key's value. No
  response. More than 5120 bytes → error → disconnect.
- Memory: key → bytes table **specific to a connection**, in memory only (never on disk),
  kept across login → configuration → play → reconfiguration, **passed to the next
  server on a `transfer`** (§10), lost on any other disconnect.
- Vanilla server: never requests a cookie; an unsolicited response causes a kick.
  Never send `cookie_response` without a request.

## 9. Resource packs: `resource_pack_push` (configuration 0x09, play 0x52), `resource_pack_pop` (configuration 0x08, play 0x51)

- `resource_pack_push`: `id: uuid`, `url: string(32767)`, `fingerprint: string(40)`,
  `required: bool`, `prompt: opt<text>`.
- `resource_pack_pop`: `id: opt<uuid>` (absent = all).
- Response `resource_pack` (configuration 0x06, play 0x32): `id: uuid`, `status: varint` —
  0 loaded successfully, 1 declined, 2 download failed, 3 accepted, 4 downloaded, 5 invalid URL,
  6 reload failed, 7 discarded. 3 and 4 are intermediate, the others terminal.

Per-server preference (`servers.dat`: `acceptTextures` true = Enabled, false = Disabled, absent
= Prompt) → session prompt status, set when the connection opens: Enabled →
"allowed", Disabled → "declined", Prompt → "pending". Direct connection without a list
entry: "Prompt".

On receiving a `push`:
1. URL not `http`/`https` or malformed → immediate `5`, nothing else.
2. If preference ≠ Prompt **and** (pack not required **or** preference ≠ Disabled) → no prompt:
   - status "declined" → immediate `1`;
   - status "allowed" → pack added and immediate `3`.
3. Otherwise (Prompt, or pack required while Disabled) → confirmation screen (if a prompt
   screen is already open, the pack is added to it; text and required flag from the last packet):
   - title: `multiplayer.requiredTexturePrompt.line1` (required) or `multiplayer.texturePrompt.line1`;
   - message: `multiplayer.requiredTexturePrompt.line2` in yellow bold (required) or
     `multiplayer.texturePrompt.line2`; if `prompt`: `multiplayer.texturePrompt.serverPrompt`
     (message, prompt);
   - buttons: required → "Proceed" / "Disconnect"; otherwise "Yes" / "No".
   - **Accept**: back to the previous screen, preference ← Enabled (saved), status ← allowed,
     `3` for each pending pack.
   - **Decline a non-required pack**: preference ← Disabled (saved), status ← declined, `1` for each
     pending pack.
   - **Decline a required pack**: local disconnect `multiplayer.requiredTexturePrompt.disconnect`
     (no `1` has time to go out); preference unchanged.
4. A pack added with an `id` already present replaces the old one (the old one disappears without a message).

Follow-up for an accepted pack (on every client tick):
- download (batched for the packs accepted at the same time): headers `X-Minecraft-Username`,
  `X-Minecraft-UUID` (without dashes), `X-Minecraft-Version`, `X-Minecraft-Version-ID`,
  `X-Minecraft-Pack-Format`, `User-Agent: Minecraft Java/<version>`; 250 MB maximum
  (262,144,000 bytes); SHA-1 fingerprint verified only if it is 40 hexadecimal digits;
  disk cache `downloads/` (20 packs kept at most);
- success → `4`; if a download of the batch fails: `2` for each failed pack and `7` for
  all the other packs not yet active (even successful ones);
- when no download is in progress any more: resource reload with the server
  packs on top of the stack; success → `0` for each loaded pack; failure → `6` for the packs
  being activated, `7` for the inactive ones.
- `pop`: removes the pack (or all) and reloads; **no message** sent for a requested removal.

Vanilla server: in configuration, it waits for a **terminal** response before continuing (without a
response, the client stays stuck on loading); a `1` while the pack is required → kick.
On a normal disconnect, the server packs are removed and the status goes back to "pending";
on a `transfer`, they are kept.

## 10. `transfer` (configuration 0x0C, play 0x84)

Fields: `host: string(32767)`, `port: varint`.

1. The client enters "transfer" mode: from then on, it **only applies** `store_cookie`
   and `transfer` still in the queue; everything else is ignored.
2. Closes the connection (reason `disconnect.transfer`) without a disconnect screen, keeps the server
   resource packs.
3. Opens the connection screen (status `connect.transferring`, failure title
   `connect.failed.transfer`) toward `host:port`, **with the same server entry** (pack
   preferences, code of conduct) and handshake intention **3**. Return after failure: the original
   screen (server list) or the title screen.
4. Data passed to the new connection: cookies (§8), players already seen (player-list
   profiles) and "insecure chat warning already seen". Everything else starts from zero:
   server brand, links, report details, chat state, world.
- Vanilla target server: refuses intention 3 if `accepts-transfers=false` (disconnect).
- Singleplayer game: error (not applicable to Rustcraft).

## 11. Configuration and session metadata

- **`update_enabled_features`** (configuration 0x0D): `flags: list<id>`. Replaces the set
  of enabled features (default: `minecraft:vanilla`). Known in 26.3: `minecraft:vanilla`,
  `minecraft:trade_rebalance`, `minecraft:redstone_experiments`, `minecraft:minecart_improvements`;
  unknown → warning in the log, ignored. Client uses: items visible in the creative inventory
  and the operator tab (non-enabled item = hidden), in-game rules screen,
  accessibility option tied to minecarts, behaviour of the new minecarts
  (`move_minecart_along_track`, `entities.md`). No response.
- **`reset_chat`** (configuration 0x06, no field): on a reconfiguration, the client
  normally carries the chat state (displayed messages, send history, deferred
  deletions) over to the next game session; this packet discards it: the chat starts empty again. No
  response.
- **`custom_report_details`** (configuration 0x10, play 0x8B): `details: list ≤ 32 of { key:
  string(128), value: string(4096) }`. Replaces the table; added as is (category
  "Custom Server Details") to crash reports and error disconnect reports. No display.
- **`server_links`** (configuration 0x11, play 0x8C): `links: list<{ known: bool, then if true
  type: varint, otherwise label: text; url: string(32767) }>`. Types: 0 `report_bug`,
  1 `community_guidelines`, 2 `support`, 3 `status`, 4 `feedback`, 5 `community`, 6 `website`,
  7 `forums`, 8 `news`, 9 `announcements` (out of range → 0). Each URL must have an `http`
  or `https` scheme (otherwise link ignored, warning). Replaces the list. Uses: dialog
  `minecraft:server_links` (pause menu button, `option-menus.md`), bug report link on
  the disconnect screen after an error.
- **`code_of_conduct`**: response described in `connection.md`. Full behaviour: second send
  in the same configuration → error → disconnect. If the server entry stores the fingerprint of
  this text (Java string hash), accept without displaying anything. Otherwise a screen (title
  `multiplayer.codeOfConduct.title` in bold, plain text, checkbox `multiplayer.codeOfConduct.check`
  "do not show again", buttons "Acknowledge" / "Disconnect", Escape disabled):
  accept → `accept_code_of_conduct`, store the fingerprint if the box is checked, otherwise clear it;
  decline → local disconnect `multiplayer.disconnect.code_of_conduct` without a packet; the entry is saved in both cases.

## 12. Server dialogs: `show_dialog` (configuration 0x13, play 0x8F), `clear_dialog` (configuration 0x12, play 0x8E)

- `show_dialog` play: `dialog: varint` (id + 1 in the `minecraft:dialog` registry received
  in configuration); 0 → direct dialog as `nbt` following. In configuration: always the direct
  `nbt`, without a prefix.
- `clear_dialog`: no field.
- NBT format of a dialog (= format of the `data/*/dialog/*.json` files): `type` among `notice`,
  `confirmation`, `multi_action`, `server_links`, `dialog_list`; common: `title` (text),
  `external_title` (text, opt.), `can_close_with_escape` (default true), `pause` (default true;
  only has an effect in singleplayer), `after_action` (`close` default, `none`, `wait_for_response`), `body`
  (list of `plain_message` / `item`), `inputs` (list of `text`, `boolean`, `single_option`,
  `number_range`, each with a `key`).
- Button actions: click events allowed from the server (`open_url`,
  `run_command`, `suggest_command`, `change_page`, `copy_to_clipboard`, `show_dialog`, `custom`),
  plus `dynamic/run_command` (template `$(key)` filled with the input values) and
  `dynamic/custom` (`id` + NBT `additions` completed by one entry per `key`).
- Executing an action: next screen = the dialog itself (`none`), the previous screen (`close`)
  or a wait screen (`wait_for_response`: title `gui.waitingForResponse.title`, back button
  visible after 1 s, active after 5 s, countdown). Then:
  - `run_command`: in game, command sent via `chat_command` if it parses without error, without
    signable argument and without special permission; otherwise confirmation screen
    (`multiplayer.confirm_command.parse_errors`, `.signature_required`, `.permissions_required`).
    In configuration: ignored (log only). Leading slash removed.
  - `show_dialog`: opens that dialog (the "previous" stays the screen before the first dialog).
  - `custom` / `dynamic/custom`: sends **`custom_click_action`** (configuration 0x08, play 0x44):
    `id: id`, then `payload: VarInt length ≤ 65536` followed by an optional `nbt` tag (byte 0
    = absent; depth ≤ 16, 32 KiB counted).
  - others: like a click in the chat (link with confirmation, clipboard…).
- Closing with Escape (if allowed): "cancel" action of the dialog type, then back to the previous
  screen (like `close`).
- New `show_dialog` during a dialog or the wait screen: replaces the screen, the "previous" is kept.
  Each dialog has a warning button at the top right opening a confirmation
  ("Disconnect" / back); if it is open, the new dialog becomes its return screen.
- `clear_dialog`: if a dialog is displayed → back to its previous screen; if the warning
  confirmation is open on top, it is its return screen that is replaced. Otherwise nothing.
- Layout and drawing of dialogs: interface spec to be written (outside this document).

## 13. Miscellaneous play

- **`chunks_biomes`** (play 0x0D): `chunks: list<{ position: i64 (packed like `forget_level_chunk`),
  data: bytes(2 MiB) }>`. For each entry: chunk not in the cache or absent → ignored
  (warning); otherwise reread **the biome containers of all sections**, bottom to top,
  in the chunk packet format (`chunks.md`, paletted container of 64 entries). Then, for each
  entry: invalidate the tint caches (grass, foliage, water) of this chunk; finally mark for
  rebuild all the sections of the 3 × 3 chunks centered on it. Sent by `/fillbiome`. No response.
- **`set_simulation_distance`** (play 0x71): `distance: varint`. Default before `login`: 3; `login`
  sets it. Client uses: death animation (counter, falling sideways) of living entities only if
  the Chebyshev distance in chunks to the player ≤ distance; F3 "SD: n". No response.
- **`game_rule_values`** (play 0x28): `rules: list<{ key: id (registry `minecraft:game_rule`),
  value: string }>`. Only the response to `client_command` action **2** (rules request),
  sent by the "Game Rules" screen of the world options; the server only answers
  level-2 operators. Applied only if this screen is open; otherwise ignored. The client
  uses **no** game rule elsewhere (the rules that concern it arrive through
  `game_event` / `entity_event`). Modification: `set_game_rule` (play 0x3A): list of { key,
  value as a string }.
- **`change_difficulty`** (play 0x0A): `difficulty: varint` (0 peaceful, 1 easy, 2 normal,
  3 hard; out of range → id 0), `locked: bool`. Stored in the client world data
  (kept on respawn); refreshes the world options screen if it is open. Sent
  on login, on respawn and on every change. No response.
- **`custom_payload`** (configuration 0x01, play 0x18): `channel: id`, then the rest of the packet
  (≤ 1 MiB, otherwise error → disconnect).
  - `minecraft:brand`: `brand: string(32767)` → stored for the session (kept across
    reconfigurations, lost on transfer). F3 display (server section):
    `"<brand>" server[ (frozen)| (frozen - stepping)], <n> tx, <n> rx` (packets per second
    sent/received, rounded); unknown brand → `"null"`.
  - **any other channel** (`minecraft:register`, `bungeecord:main`, mod channels, Paper's…):
    skipped doing nothing, no log, no response.
  - Sent by the client: right after the login-finished acknowledgement, before `client_information`,
    `custom_payload` (configuration 0x02) `minecraft:brand` with the string `vanilla`. The vanilla
    server ignores any client payload.
- **`pong_response`** (play 0x3F): `time: i64`. Response to `ping_request` (play 0x26, `time: i64`
  = client ms clock), which the client sends **on every tick while the F3 network graphs
  are displayed**, and only then. Handled on the network thread: latency sample
  = now − time, added to the F3 "ping" graph. Nothing else; not to be confused with
  `ping`/`pong` (`connection.md`).

## Uncertain points

- Camera return when the followed entity disappears on the client side without `set_camera` (chunk unloaded):
  the client presumably keeps a camera on a removed entity until the server's packet; not verified.
- First-person hand rendering when the camera is not the player: not verified.
- Keeping the server packs on transfer: deduced from the order of the calls (closing is done
  with "keep the packs", the next connection finds no old connection to clean up);
  to be confirmed by capture.
- Brand choice for Rustcraft: `vanilla` makes the client indistinguishable from the official client;
  another string would be visible to server plugins. Decision to be made.
- Physics of the boat, mounts and happy ghast under local authority: not described here; without
  it, Rustcraft must at least answer `move_vehicle` (step 3 of §7) so as not to be endlessly
  corrected.
- Rate > 20: only the client is capped at 20 Hz; the animations received from the server (positions)
  arrive faster, the 3-step interpolation (`entities.md`) absorbs them — visual behaviour not
  measured.
