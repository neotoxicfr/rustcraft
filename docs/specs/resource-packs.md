# Resource packs

Sources: reading of the official 26.3 client (server pack manager, download
queue, pack repository, selection screen, resource loading screen).
Complements `protocol-supplements.md` § 9 (packets, states, network-side state machine),
`option-menus.md` § 3.13 (drawing of the screen) and `title-screen-servers.md` (`acceptTextures`).

## 1. Source stack

Priority order, from strongest to weakest (the first one that has a path wins):

1. active server packs, the **last one pushed** first;
2. packs chosen by the player, in the order of the "Selected" list (top = priority);
3. `vanilla` pack (the launcher's jar and asset index). It is **required** (never removed)
   but not fixed: the player can drag a pack below it.

Ids: `vanilla`; optional built-in packs `programmer_art` and `high_contrast` (zips of the
asset index, `minecraft/resourcepacks/<name>.zip`, absent if the index does not have them); packs from the
`resourcepacks` folder: `file/<file name>`; server packs: `server/<8-hex number>/<uuid>`
(never saved).

Merge exceptions (the game merges instead of taking the first one): language files, sounds
(`sounds.json`), fonts, atlases. Rustcraft takes the first file found (known difference, § 7).

## 2. Packs in the `resourcepacks` folder

- `resourcepacks` folder inside the game folder, created if needed. A pack = a **folder** containing
  `pack.mcmeta`, or a **`.zip` file**. Other files ignored.
- `pack.mcmeta`, `pack` object:
  - `description`: text (string or JSON component);
  - supported versions, in order of preference: `min_format` + `max_format` (integer
    `M` or list `[M, m]`; default minimum `M.0`, default maximum `M.*`); otherwise
    `supported_formats` (integer, list `[a, b]` or object `min_inclusive`/`max_inclusive`, in
    major versions); otherwise `pack_format` alone (accepted only up to 64).
  - unreadable meta or no version: pack **ignored** (absent from the lists); only the description
    readable: compatibility "unknown".
- Resource format of 26.3: **97.1**. Compatibility: max < 97.1 → too old;
  97.1 < min → too new; otherwise compatible. An incompatible pack can still be selected after
  confirmation.
- Icon: `pack.png` at the root of the pack, otherwise `textures/misc/unknown_pack.png`.
- Title: file or folder name; `vanilla`: `resourcePack.vanilla.name` and description
  `resourcePack.vanilla.description`.

## 3. Player choice and `options.txt`

- `resourcePacks`: JSON list of the chosen ids, **from bottom to top** (increasing
  priority), non-fixed packs only (so `vanilla` is in it); `incompatibleResourcePacks`:
  those in the list that are incompatible.
- At startup: each id of `resourcePacks` is looked up (a name without a prefix is also
  tried with `file/`); absent → removed; became incompatible without being in
  `incompatibleResourcePacks` → removed; became compatible again → removed from the incompatible list. The required
  `vanilla` pack, if absent from the list, is put back at the very bottom.

## 4. "Resource Packs" screen (options, `options.resourcepack`)

Drawing: `option-menus.md` § 3.13. Title `resourcePack.title`.
- Two lists: "Available" (packs not chosen, alphabetical order of the ids) and
  "Selected" (top = strongest priority).
- Hovering the icon of an entry: veil, then arrows. Available: click on the icon →
  selection (at the top of the selected ones). Incompatible pack: confirmation screen
  `pack.incompatible.confirm.title` / `pack.incompatible.confirm.<old|new|unknown>`, Yes/No.
  Selected: left half → removal (at the top of the available ones; impossible for a required one),
  upper right quarter → move up, lower right quarter → move down (without crossing a fixed pack).
  Keyboard: Enter = select/remove the chosen entry, Shift+↑/↓ = move.
- Hovering an incompatible pack: name replaced by `pack.incompatible`, description by
  `pack.incompatible.<old|new|unknown>` (gray).
- Search field (hint `gui.packSelection.search`, initial focus): filters both lists
  (id, title or description containing the text, case-insensitive).
- "Open Pack Folder" (`pack.openFolder`, tooltip `pack.folderInfo`): opens the
  `resourcepacks` folder in the file explorer. The folder is watched: a change re-reads the packs
  (at most once per second) keeping the choices.
- Dropping files onto the window (`pack.dropInfo`): confirmation `pack.dropConfirm`, copy into
  the folder.
- "Done" / Escape: choice confirmed, `options.txt` rewritten; if the list of ids
  changed, resources are reloaded (§ 6).

## 5. Packs imposed by the server

### 5.1 Preference and prompt

Preference of the server list entry (`acceptTextures`: true = Enabled, false =
Disabled, absent = Prompt; direct connection without an entry = Prompt). Session status
set at login: Enabled → allowed, Disabled → declined, Prompt → pending.

Reception of a `resource_pack_push` (`id`, `url`, `fingerprint`, `required`, `prompt`):
1. URL not `http`/`https` or malformed → state **5**, nothing else.
2. Preference ≠ Prompt and (not required or preference ≠ Disabled) → direct add: status declined
   → **1**; otherwise the pack enters the list (and **3** if the status is allowed).
3. Otherwise: confirmation screen on top of the current screen (a second `push` while the prompt
   is open is added to its requests; title, message and required flag follow the last one):
   - title `multiplayer.requiredTexturePrompt.line1` (required) / `multiplayer.texturePrompt.line1`;
   - message `multiplayer.requiredTexturePrompt.line2` yellow bold (required) /
     `multiplayer.texturePrompt.line2`, wrapped by `multiplayer.texturePrompt.serverPrompt`
     (message, server prompt) if the server gives one;
   - buttons: required "Proceed" (`gui.proceed`) / "Disconnect" (`menu.disconnect`);
     otherwise "Yes" / "No".
   - Accept: back to the previous screen, preference ← Enabled, status ← allowed, each pending
     request added (→ **3**). Decline: status ← declined; required → disconnect
     `multiplayer.requiredTexturePrompt.disconnect` (preference unchanged); otherwise preference ←
     Disabled and each request → **1**. The preference is saved in `servers.dat`
     (entry with the same name and address).
4. A pack added with an `id` already present replaces the old one without a message.

**No network request is made before acceptance** (prompt or Enabled preference).

### 5.2 Download

- `downloads/` folder inside the game folder; one subfolder per `id` (UUID with dashes); the
  file is named after its SHA-1 in lowercase hexadecimal; log `downloads/log.json`
  (one JSON line per download: `id`, `url`, `time` ISO 8601, optional `hash`, then
  `file` {`name`, `size`} or `error`).
- Fingerprint taken into account only if it is 40 hexadecimal digits (case ignored).
  With a fingerprint: if `downloads/<id>/<sha1>` exists and has the right fingerprint, it is reused
  without a request (modification date refreshed); otherwise deleted then downloaded, and a different
  fingerprint on arrival = failure. Without a fingerprint: downloaded into a temporary file then renamed with
  its computed SHA-1.
- GET request, redirects followed, headers `X-Minecraft-Username`, `X-Minecraft-UUID` (without
  dashes), `X-Minecraft-Version` (`26.3`), `X-Minecraft-Version-ID` (`26.3`),
  `X-Minecraft-Pack-Format` (`97.1`, major.minor), `User-Agent: Minecraft Java/26.3`.
- Maximum size 262,144,000 bytes (announced or read): beyond that, failure.
- Packs accepted at the same time go out as a batch, one after the other on a background thread.
  System notification per batch (5 s, updated in place): title `download.pack.title` n/N on
  each file (reused ones included), message `download.pack.progress.percent` (bytes × 100 / announced
  size) or `.bytes` ("1.5 KB", one decimal, powers of 1024) on the response then on each
  read; on the last file: `download.pack.failed` (failures/N, no message) if there was a
  failure, otherwise it is hidden. Quirks: the final hiding only affects a notification already
  created (previous frame); a batch finished in the frame where it started (cached packs) therefore leaves it
  its 5 s. An announced size of zero makes the download fail (division by zero in the
  percentage) without changing the displayed message.
- Singleplayer world pack: `resources.zip` in the world folder (file), loaded on opening like
  an imposed pack (on top of the stack, source "World", no prompt or network response), after the
  copy offered for experimental settings and before the disk space warning; screen
  "Preparing resources…" during loading; failure (unreadable meta, fallback reload):
  confirmation `multiplayer.texturePrompt.failure.line1`/`line2`, "Proceed"
  (open without the pack) or "Cancel" (back to the list). Removed when leaving the world.
- At queue start: the folder is cleaned, only the **20** most recent files
  (modification date) are kept, empty folders deleted.

### 5.3 States and application

Order of the `resource_pack` responses for an accepted pack: **3** (accepted) → **4** (downloaded) →
**0** (loaded). Failures: **2** for the pack whose download fails and **7** for all the
other packs not yet active at that moment; failed reload → **6** for the packs being
activated, **7** for the inactive ones. Terminal responses go out when the pack leaves the
list.

When no accepted download is in progress any more and a pack must enter or leave:
resource reload (§ 6) with the server packs on top of the stack; an
unreadable pack (invalid meta) makes the reload fail. Success → **0** for each loaded pack.

`resource_pack_pop`: removes the pack (or all) then reloads; no response.
End of connection (except transfer): server packs removed (reload if some were
active), status reset to "pending".

In configuration, the vanilla server waits for a terminal response before continuing.

## 6. Resource reload

- Triggered by: change of the player's list (screen), server packs, F3+T.
- Loading screen: red background 0xEF323D (black if `darkMojangStudiosBackground`), logo
  `textures/gui/title/mojangstudios.png` cut into two halves of 120 × 60 (top, bottom) placed side
  by side, centered, height `min(W × 0.75, H) × 0.25`; white progress bar (1 px frame,
  smoothed fill) at `0.8325 × H`, width of the logo. Fades in over 500 ms when a screen
  is open, fades out over 1 s after the end.
- Everything is re-read from the new stack: block models and textures, items, entities,
  interface (sprites, fonts, language), sky, panorama, weather, particles, sounds. World
  sections are re-meshed.
- Reload failure: player choices cleared (`resourcePacks` and `incompatibleResourcePacks`
  too), new attempt with only the required packs, then notification `resourcePack.load_fail`.

## 7. Known differences in Rustcraft

- No merging between packs (languages, `sounds.json`, fonts, atlases): the first file found
  wins.
- A pack placed below `vanilla` in the selected list is not stacked (the jar stays the last
  source).
- Reload: the rendering engine is recreated on the same window (all GPU resources
  re-sent); skins are requested again from the disk cache, placed player heads come back on
  their next update.
- No reload failure other than
  an unreadable server pack meta (loading falls back to default values).
- Bench: `crates/rc-client/examples/pack_mock.rs` (fake server that pushes packs in
  configuration), demonstrations `RUSTCRAFT_RELOADTEST`, `RUSTCRAFT_PACKPROMPT`,
  `RUSTCRAFT_PACKSCREEN`.
