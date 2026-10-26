# Title screen, server list, status and connection (26.3, protocol 777)

Source: reading of the decompiled client (kept outside the repository), reworded. See also `connection.md` (handshake,
login, configuration), `text.md` (text components), `sounds.md` § 6 (music).
Units: interface pixels (after GUI scale); `W`, `H` = screen width, height;
integer divisions unless stated otherwise. Colours in ARGB.

## 1. Title screen

### 1.1 Background: panorama

- Six faces `textures/gui/title/background/panorama_0.png` … `panorama_5.png`. **Trap**: in the
  jar these are 1 × 1 images (placeholders); the real ones (≈ 0.4 to 1 MB each) come from
  the asset index (`assets/indexes/<id>.json`, key `minecraft/textures/gui/title/background/…`),
  placed before the jar in the resource stack. Same for `panorama_overlay.png`.
- Cube texture: layers in the order +X, −X, +Y, −Y, +Z, −Z = faces `_1`, `_3`, `_5`, `_4`, `_0`,
  `_2`, each flipped vertically on copy. All faces must have the same size.
- Geometry: cube of side 2 centred on the origin (vertices ±1), 6 quads; a fragment's colour
  is the cube sample in the direction of the interpolated vertex position (no UVs).
- Perspective projection: vertical field 85°, planes 0.05 and 10, aspect ratio of the
  window (real pixels, not the GUI scale).
- View: rotation of 180° around X, then of **10°** around X (slight downward gaze), then
  of **−spin** degrees around Y. No useful depth write; drawn before the whole
  interface, over the whole window.
- Rotation: on each frame, `spin ← wrap_degrees(spin + Δt × speed × 0.1)` where Δt is the
  real time elapsed in ticks (ms / 50) and `speed` the option `panoramaScrollSpeed`
  (`options.accessibility.panorama_speed`, 0 to 1, default 1) ⇒ 2°/s by default. `spin` is
  global (survives screen changes); the title screen restarts the rotation when created.
- Overlay: `panorama_overlay.png` (16 × 128 texture) stretched over the whole screen (0, 0, W, H), full
  UVs, on top of the cube.
- **No blur nor menu background on the title screen** (its own background is empty). The other
  out-of-world screens (multiplayer, options, connection…) draw: panorama + overlay, then blur,
  then `textures/gui/menu_background.png` repeated in 32 × 32 tiles (in world:
  `inworld_menu_background.png`, without panorama).
- Menu blur: option `menuBackgroundBlurriness` (`options.accessibility.menu_background_blurriness`,
  integer 0 to 10, default 5); < 1 ⇒ no blur. Six box-blur passes (three pairs
  horizontal then vertical, the game's `blur` effect file; see `option-menus.md`) of radius r = option on the already drawn image; each pass sums
  bilinear samples at offsets −r+0.5, −r+2.5, … (step of 2 texels, ≤ r) plus the sample at +r weighted
  ½, the whole divided by r + 0.5.

### 1.2 Logo and edition

- Logo `textures/gui/title/minecraft.png` (1024 × 256 file, treated as 256 × 64): rectangle
  256 × 44 at (W/2 − 128, 30), UV (0, 0)–(1, 44/64).
- Rare variant: probability 10⁻⁴ drawn when the logo renderer is created,
  `textures/gui/title/minceraft.png` instead.
- Edition `textures/gui/title/edition.png` (512 × 64, treated as 128 × 16): 128 × 14 at
  (W/2 − 64, 30 + 44 − 7 = 67), UV (0, 0)–(1, 14/16).
- White tint, alpha = fade alpha (§ 1.6).

### 1.3 "Splash" text

- Chosen once, on the screen's first `init` (kept if resized); nothing if
  the option `hideSplashTexts` is true (`options.hideSplashTexts`, default false).
- File `texts/splashes.txt` (454 lines in 26.3): each line trimmed of whitespace; the line
  "This message will never appear…" is discarded (filtered by fingerprint). Literal text, colour
  **yellow 0xFFFFFF00**.
- Dates (local time zone, day and month): 24 December → "Merry X-mas!", 1 January → "Happy new
  year!", 31 October → "OOoooOOOoooo! Spooky!". Otherwise empty list → no splash; otherwise draw
  an integer in [0, n): if it equals **42** (and there is a user), text
  `<USERNAME IN UPPERCASE> IS YOU`; otherwise second draw of a uniform index.
- Drawing (each frame): `w` = text width;
  `phase = 1.8 − |sin((ms mod 1000) / 1000 × 2π)| × 0.1`;
  `scale = phase × 100 / (w + 32)`.
  Matrix: translation (W/2 + 123, 69), rotation −20° (−π/9, clockwise on screen with y downward),
  uniform scale; text placed at (−w/2, −8) with shadow, alpha = fade.

### 1.4 Buttons (normal game)

y₀ = H/4 + 48, vertical step 24, buttons 20 high.

| Row | y | Content |
|---|---|---|
| 1 | y₀ | "Singleplayer" `menu.singleplayer`, 200 wide, x = W/2 − 100 |
| 2 | y₀ + 24 | "Multiplayer" `menu.multiplayer`, 200 |
| 3 | y₀ + 48 | "Realms" `menu.online`, 200 |
| 4 | y₀ + 72 | three 20 × 20 icon buttons (friends, language, accessibility), gap 4: x = W/2 − 34 + (i−1) × 24 |
| 5 | y₀ + 96 | "Options..." `menu.options` 98 wide at W/2 − 100; "Quit Game" `menu.quit` 98 at W/2 + 2 |

- Multiplayer and Realms disabled (with tooltip) if multiplayer is forbidden
  (`title.multiplayer.disabled`, or `…banned.name` / `…banned.temporary` / `…banned.permanent`).
- Multiplayer first opens the safety warning as long as the option `skipMultiplayerWarning`
  is false, otherwise directly the server list (§ 2).
- Demo mode: rows 1–2 replaced by "Play Demo World" and "Reset Demo World"; the
  rest shifts accordingly.
- Escape does not close the title screen; it is not a pause screen.

### 1.5 Version and copyright

- Bottom left: `Minecraft <version name>` (+ ` Demo`, + suffix `menu.modded` if modified client)
  at (2, H − 10), white, alpha = fade, with shadow.
- Bottom right: text button `title.credits` of width w (text width), 10 high, at
  (W − w − 2, H − 10); click → credits and attributions screen.

### 1.6 Opening fade

Only on the first title screen after the game loads: t = (ms − start) / 2000 clamped to
[0, 1]; alpha of the widgets, logo and splash = 0 for t ≤ 0.5, then linear from 0 to 1 over
[0.5; 1]. The panorama is not faded. Beyond 2 s, no more fade.

### 1.7 Music

No player ⇒ menu music `music.menu` (delays 20–600 ticks, replaces the current music):
see `sounds.md` § 6.

## 2. Server list (multiplayer)

### 2.1 File `servers.dat`

- In the game folder. **Uncompressed** NBT (no gzip), root compound without a meaningful
  name, key `servers`: list of compounds.
- Fields of an entry:

| Key | Type | Meaning |
|---|---|---|
| `name` | string | displayed name (default "") |
| `ip` | string | typed address (`host`, `host:port`, `[v6]:port`) |
| `icon` | string, optional | PNG in standard base64 (without `data:` prefix) |
| `acceptTextures` | boolean byte, optional | true = server packs accepted, false = declined, absent = prompt |
| `acceptedCodeOfConduct` | int, optional | fingerprint (Java string hash) of the accepted code of conduct; omitted if 0 |
| `hidden` | boolean byte | hidden entry (from a direct connection) |

- Reading: entries with `hidden` true → hidden list, otherwise visible list; error → empty list.
- Writing: visible first (`hidden` = 0), then hidden (`hidden` = 1), into a temporary
  file in the folder, then safe replacement: the old `servers.dat` becomes
  `servers.dat_old`.
- Hidden list: addition at the head, at most **16** (the oldest drop out). Lookup by
  address: visible then hidden, exact equality of the `ip` string.
- Adding a server whose address is in the hidden list "unhides" it (moved to the end
  of the visible ones, name and icon taken from the input).
- Update of an icon received by ping: reread the file, replace the entry with the same name and
  same address, rewrite (sequential I/O task).

### 2.2 Screen

- Title `multiplayer.title` in a header of 33; footer of 60; list between the two, rows
  **36** high, row width **305**, centred (x = W/2 − 152). Content of a row:
  the row's rectangle shrunk by 2 on each side (x + 2, y + 2, width − 4, height − 4).
- Row order: saved servers, then the "LAN scan" row, then the LAN worlds.
- Selection: full frame of the row, white if the list has focus otherwise 0xFF808080, black
  interior (inset 1).
- Footer: two centred rows, gap 4 horizontally and vertically:
  - row 1 (width 100): "Join Server" `selectServer.select`, "Direct Connection"
    `selectServer.direct`, "Add Server" `selectServer.add`;
  - row 2 (width 74): "Edit" `selectServer.edit`, "Delete"
    `selectServer.delete`, "Refresh" `selectServer.refresh`, "Back" `gui.back`.
- Activation: nothing selected or LAN-scan row → Join, Edit, Delete inactive;
  LAN world → Join only; saved server → all three.
- Refresh (or **F5**): recreates the screen (rereads `servers.dat`, queries everything again).
- Delete: confirmation `selectServer.deleteQuestion` / `selectServer.deleteWarning` (name) /
  `selectServer.deleteButton` / Cancel; yes → removal, save.
- Leaving the screen: stops LAN detection and closes all queries in progress (reason
  `multiplayer.status.cancelled`).

### 2.3 "Saved server" entry

Origin (X, Y) = corner of the row's content, Lc = content width.

- 32 × 32 icon at (X, Y): the server icon's texture, otherwise `textures/misc/unknown_server.png`.
  The decoded icon must be **exactly 64 × 64**, otherwise it is rejected, cleared from the entry
  and the list is saved.
- Name: (X + 35, Y + 1), white, shadow.
- MOTD: wrapped into lines at width Lc − 34, **at most 2 lines**, at (X + 35, Y + 12 + 9i),
  grey 0xFF808080 (the component's own colours take precedence).
- Status icon 10 × 8 at (X + Lc − 15, Y).
- Status text to its left: x = (X + Lc − 15) − width − 5, y = Y + 1, grey 0xFF808080:
  "players" `multiplayer.status.player_count` (`%s/%s`, numbers in grey, slash in dark grey),
  or `???` dark grey if the response has no players; if **incompatible**, the server's
  version name in red instead.
- Status sprites (`server_list/…`):
  - in progress: animation `pinging_1`…`pinging_5`, i = (ms/100 + 2 × row index) & 7, if
    i > 4 then i = 8 − i (back and forth); before the first draw: `ping_1`;
  - response: latency < 150 → `ping_5`, < 300 → `ping_4`, < 600 → `ping_3`, < 1000 → `ping_2`,
    otherwise `ping_1`; tooltip `multiplayer.status.ping` (`%s ms`);
  - unreachable: `unreachable`, tooltip `multiplayer.status.no_connection`;
  - incompatible: `incompatible`, tooltip `multiplayer.status.incompatible`.
- Players tooltip when hovering the status text (response or incompatible): one name per line
  (§ 3.4).
- Row hover: overlay 0xA0909090 on the icon, then 32 × 32 sprites on the icon:
  `join` (or `join_highlighted` if the mouse is in the right half), `move_up` if it is not
  the first (highlighted in the top-left quarter), `move_down` if it is not the last
  (bottom-left quarter). Click in these zones: join / swap with the neighbour (and save).
- Double-click or Enter/Space: join. Shift + Up/Down: move the row.
- Failure messages (in the MOTD, red 0xFFFF0000): `multiplayer.status.cannot_resolve`
  (unknown host), `multiplayer.status.cannot_connect` (other error).

### 2.4 LAN

- "Scan" row: `lanServer.scanning` centred, followed by animated dots.
- Detection: UDP multicast listening **224.0.2.60:4445** (1024-byte buffer, 5 s timeout); text
  datagram `[MOTD]<motd>[/MOTD][AD]<port>[/AD]`; address kept = source IP + `:` + port. Message
  without `[/MOTD]`, with two `[/MOTD]` or without `[AD]…[/AD]` ignored; missing MOTD → `missing no`.
  Duplicate (same address): simple refresh.
- LAN entry: `lanServer.title` in white (X + 35, Y + 1); MOTD (Y + 12) and address
  (Y + 23, or `selectServer.hiddenAddress` if the option `hideServerAddress`) in 0xFF808080.
  Join = connection with name = MOTD.

### 2.5 "Add / edit" screen

- Title `manageServer.add.title` or `manageServer.edit.title`, centred at y = 17, white.
- Label `manageServer.enterName` at (W/2 − 99, 53), 0xFFA0A0A0; name field 200 × 20 at
  (W/2 − 100, 66), 32 characters max, greyed hint `selectServer.defaultName`.
- Label `manageServer.enterIp` at (W/2 − 99, 94); address field 200 × 20 at (W/2 − 100, 106),
  **128** characters max.
- Cycling button `manageServer.resourcePack` (Enabled / Disabled / Prompt) 200 × 20 at
  (W/2 − 100, H/4 + 72).
- "Done" `gui.done` at (W/2 − 100, H/4 + 114); "Cancel" at (W/2 − 100, H/4 + 138).
- Done active only if the address is valid (§ 3.1); empty name → `selectServer.defaultName`.
- Edit: copy of the entry; validation → name, address, pack status copied back, state reset
  to "initial" (re-query), save. Add: at the end of the visible list.

### 2.6 Direct connection

- Title `selectServer.direct` centred at y = 20; label `manageServer.enterIp` at (W/2 − 99, 100);
  field 200 × 20 at (W/2 − 100, 116), 128 max, prefilled with the option `lastServer`.
- "Join Server" at (W/2 − 100, H/4 + 108), "Cancel" at (W/2 − 100, H/4 + 132). Enter valid
  also if the field has focus. On closing, the input is written to `lastServer` (options
  saved).
- Validation: if the address is already known (visible or hidden), that entry is joined; otherwise
  a hidden entry is created (name `selectServer.defaultName`), save, then connection.

## 3. Status query (list "ping")

### 3.1 Address and resolution

- Parsing the input: form host[:port] (brackets for IPv6), default port **25565**;
  empty host or invalid syntax → invalid address (`server.invalid:25565`). The host is converted
  to ASCII (IDN/punycode); failure → "".
- Validity (to enable the buttons): non-empty host convertible to IDN.
- Resolution:
  1. system resolution (A/AAAA) of the host;
  2. blocklist (Mojang services, optional for us) applied to the host and to the IP;
  3. **SRV only if the port is 25565**: DNS query `_minecraft._tcp.<host>` (a single
     retry); if a record exists, take **the first one returned** (no sorting by
     priority/weight), target = host and port of the record, then resolve that target
     (it replaces the result of step 1);
  4. no result → "unknown host".
- Each query runs on a group of at most 5 threads; it starts at the first draw of the
  entry ("initial" → "in progress" state).

### 3.2 Exchange ("status" state)

TCP connection to the resolved address, ordinary frames (VarInt length), without compression nor
encryption.

1. Handshake (`intention`, id 0x00): protocol **777**, host = **typed host** (not the SRV
   target), port = **typed port**, intention **1** (status). Format in `connection.md`.
2. Status request (id 0x00, empty).
3. Status response (id 0x00): JSON string (≤ 32767 characters, tolerant reading), § 3.3.
4. As soon as the response arrives: ping request (id 0x01, `i64` = clock in ms); store the time.
5. Pong (id 0x01, `i64` returned): latency = now − stored time (ms, client side, the
   returned value is not used). Close (`multiplayer.status.finished`).

- Second status response → disconnect (`multiplayer.status.unrequested`).
- Final state: protocol of the response = 777 → "succeeded", otherwise "incompatible".
- Disconnect before any response → MOTD `cannot_connect`, empty status text, then attempt of the
  legacy ping (§ 3.5).

### 3.3 Response JSON

All fields are optional and read tolerantly (invalid field = absent).

| Field | Content | Absent / invalid |
|---|---|---|
| `description` | text component (JSON: string, object or array, see `text.md`) | empty |
| `players` | `{ "max": int, "online": int, "sample": [ { "id": UUID with dashes, "name": string } ] }` (`sample` optional) | text `???` |
| `version` | `{ "name": string, "protocol": int }` | version `multiplayer.status.old`, protocol 0 |
| `favicon` | string `data:image/png;base64,<base64>` (`\n` line breaks removed before decoding) | icon unchanged |
| `enforcesSecureChat` | boolean | false |

- Description "sanitised" before display: resolution of the dynamic parts with depth
  ≤ 16 (the rest is discarded), "player sprite" objects refused; on error, empty.
- Favicon: if the bytes differ from the known icon, the PNG header is checked (≤ 1024 × 1024,
  otherwise icon removed), it is stored and the list is saved. Display then requires
  64 × 64 (§ 2.3).

### 3.4 Player list (tooltip)

For each `sample` entry: `multiplayer.status.anonymous_player` if the UUID is null and the
name is "Anonymous Player", otherwise the name. If the sample is shorter than `online`, final line
`multiplayer.status.and_more` (`online − size`).

### 3.5 Legacy ping (servers ≤ 1.6)

Sent only after the modern ping fails, on a new TCP connection (without framing):

- bytes `FE 01 FA`, legacy string `MC|PingHost`, `u16` size of the rest, then byte **127**
  (legacy protocol), legacy string of the host, `i32` port. Legacy string = `u16` character
  count + big-endian UTF-16.
- Response: byte `FF`, legacy string split on `\0` into at most 6 parts:
  `§1`, protocol, version, MOTD, players, max (malformed integers → 0 for the protocol, −1 for
  the players). Other prefix: ignored. Close after reading.
- Result: **incompatible** entry, literal version and MOTD, player count, without
  a sample.

## 4. Connecting to a server

### 4.1 "Connection" screen

- Opened by Join (and transfers). If a connection screen is already active, the request is
  ignored. The current world is left first.
- Background: that of the menus (§ 1.1). Status text centred at (W/2, H/2 − 50), white. Button
  "Cancel" `gui.cancel` 200 × 20 at (W/2 − 100, H/4 + 132). Escape inactive.
- Successive text states:
  `connect.connecting` (start, also during resolution) → `connect.authorizing` (reception
  of the server's `hello`, before session authentication) → `connect.encrypting` (after
  authentication, just before sending the key) → `connect.joining` (`login_finished`). `connect.negotiating` on each custom
  login query; `connect.transferring` from the start for a transfer. Allowed
  transitions: authorizing ← connecting; encrypting ← authorizing; joining ← encrypting or
  connecting (offline server).
- Flow (separate thread): resolution § 3.1 → failure: disconnect screen "unknown host"
  (`disconnect.genericReason` + `disconnect.unknownHost`); otherwise TCP connection, then
  handshake with intention **2** (login, or **3** for a transfer) with **the resolved host and port**
  (after SRV, unlike the ping), then `hello` (username, UUID). Continuation:
  `connection.md`.
- Network error: disconnect screen with the exception's message (address removed from the text).
- Cancel: marks the abort, cuts the TCP attempt in progress, closes the connection
  (`connect.aborted`), back to the parent screen (the list).
- The status of the entry's server packs (accept/decline/prompt) is passed to the
  pack manager from the connection onward.

### 4.2 Disconnect screen

- Column centred vertically and horizontally on the screen: title (one line), reason
  (multi-line component, centred, max width W − 50), spacing 10 around these two; then,
  spacing 2: optional server bug report button (link), optional "open the
  report folder" button, back button 200 wide.
- Title: `connect.failed` (failure during connection), `connect.failed.transfer` (transfer),
  `disconnect.lost` (loss in game).
- Button: `gui.toMenu` (back to the list) by default, `gui.back` for Realms,
  `gui.toTitle` (to the title screen) if multiplayer is forbidden. Escape inactive.
- Parent after a loss in game: server list (whose parent is the title screen) if we
  came from a server, otherwise title screen.
- Reason: component received in `disconnect` (login or play) or local message.

### 4.3 World loading screen

- Shown on receiving `login` (and on each `respawn` that changes world), as long as the world
  is not ready. Escape inactive, no pause.
- Background depending on the cause: change from/to the Nether (without death) → Nether portal
  particle sprite stretched over the screen; from/to the End → full-screen "End portal" render;
  otherwise menu background (panorama, blur, `menu_background`).
- Text `multiplayer.downloadingTerrain` centred, white, at y = H/2 − 50 (remote server; in
  singleplayer, above the chunk map). Progress bar (singleplayer only): 200 × 2 under
  the text (+12), black background, filled part green 0xFF00FF00, smoothed by 20 % per tick.
- End of the wait: on `login`, state "waiting for the server"; on game event **13**
  (`game_event` "start of chunk loading") → "waiting for the player's chunk"; ready when
  the player's section is compiled, or immediately if the camera is outside the height of the
  world, if the player is a spectator or dead; maximum delay **30 s** since `login`. The screen
  closes on the next tick (close delay 0 in multiplayer) and the client then signals to the
  server that the player has loaded (packet `player_loaded`).

## Uncertain points

- Exact faces ↔ cube axes correspondence: layer order and flip read, but the final
  direction (with the 180° rotation around X) is to be validated against a capture; same for the
  apparent rotation direction of the panorama. *Seen on a Rustcraft capture (12 Oct, `RUSTCRAFT_PANORAMA=1`)*:
  at spin 0 face `_0` is frontal, upright, not mirrored, horizon above the centre (gaze
  10° downward), continuous seams with `_1`; the view turns to the right (scene toward the
  left). Not compared with a capture of the official game.
- Panorama and overlay: the real images come from the asset index; verified on the local index
  (`34.json`), not on other index versions.
- The splash text: file lines treated as literal text; the interpretation of any `§`
  codes by the font has not been verified.
- `sample[].id`: UUID in text form with dashes (strict text codec); a UUID without dashes
  would make the whole `players` fail (tolerant reading → `???`). Not tested on a real server.
- Legacy ping: only the 1.6 format (`FE 01 FA`); no earlier server to test. Low usefulness
  for us (servers ≤ 1.6 rare): optional implementation.
- SRV: the client takes the first record returned by the resolver, without sorting; a
  different resolver (Rust) may order differently. The SRV target's possible trailing dot is passed as is
  to the system resolver.
- Server blocklist (Mojang services): not specified here.
- Positions of the icon buttons (friends, language, accessibility), of the multiplayer
  warning screen and of the Realms notifications: not detailed (out of scope).
- Singleplayer loading screen (coloured chunk map, colours per status): not covered, not applicable
  to a multiplayer-only client.
