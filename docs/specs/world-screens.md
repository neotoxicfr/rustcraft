# World screens (singleplayer)

Expected behaviour of the 26.3 client, described in our own words. Code: `apps/client/src/world_list.rs`,
`create_world.rs`, `edit_world.rs`; disk: `crates/rc-sim/src/storage.rs`.

## 1. List ("Select World")

- Header of 49: title at y = 8, search 200 × 20 at y = 21 (initial focus, gray italic hint text). Footer of 60:
  grid of 4 columns of 71, spacing 8 × 4: "Play" and "Create" (150 each) at the top,
  "Edit", "Delete", "Re-Create", "Back" at the bottom.
- Worlds = folders of `saves` containing `level.dat`. Sort: last played (`LastPlayed`)
  descending, then folder name. Filter: name or folder contains the search (case ignored).
  No world: creation opens directly ("Cancel" goes back to the title).
- Row of 36 by 270: icon 32 (`icon.png` 64 × 64, otherwise the unknown server icon), then at
  x + 35: name (white), "folder (short local date)" gray, gray info line: mode (hardcore
  in pure red), ", Cheats", ", Experimental" (yellow), ", Version: X" (red if more
  recent, italic if older). Texts that are too long are cut, tooltip with the whole text.
- Hover: gray veil on the icon and pictogram (play; error; error + play marked for a
  more recent version; error + play marked for an unreadable world). Click on the icon or
  double click: play. Icon tooltips: locked, incompatible, more recent version.
- Locked world (other process): "Play", "Edit", "Re-Create" inactive. Unreadable
  world (`level.dat`): "Recover" button → recovery screen (state of `level.dat` and
  `level.dat_old`, "Try to restore" if only the old one is readable: zip copy then
  restore, then "Proceed" / "Back").
- More recent version: screen "backup then play / play without backup / cancel".
- As soon as "Play" is clicked, before any reading of the world (recovery, offered backup, pack): a
  disallowed symbolic link in the folder → symlink warning, "Back" (and Escape) to the title
  screen, nothing loaded.
- Then, in the game's order (`rc_sim::packs::backup_question`, `world_list.rs` `open_step`):
  old customized world or experimental settings (non-vanilla dimension, one of the three
  dimensions of the game missing from the settings and from an enabled world pack, registry entry of a
  pack from the enabled `datapacks` folder; read on a thread under "Loading resources") →
  same backup screen (uncolored question); world pack
  `resources.zip` (`resource-packs.md` § 5.2); less than 64 MiB free on the volume →
  "Low Disk Space" (red title, "Proceed" / "Back"). An abort removes the world
  pack and goes back to the list (to the title screen for a quick launch).
- List re-read empty (last world deleted): creation opens in its place; cancelling it goes back to
  the title screen.
- Alerts (re-creation impossible, recovery failed): a "Back" button (title at 70, message at
  90); Escape goes back to the title screen (quirk of the game).
- "Delete": confirmation, then deletion (lock taken; failure → notification).

## 2. Creation (tabs Game / World / More)

- Tab bar of 24 (width min(400, w) − 28, three equal parts), footer of 33 with "Create"
  and "Cancel" at h − 27, footer separator. Content placed at one sixth of the free height.
- Game: name (208, label above, tooltip "Will be saved in: folder"), mode
  (Survival, Hardcore, Creative), difficulty (forced to Hard and inactive in hardcore), cheats
  (default: enabled in creative; disabled in hardcore).
- World: type (Default, Superflat, Large Biomes, Amplified, Single Biome; tooltip for
  Amplified), "Customize" (flat and single biome), seed (308, hint "leave empty…"),
  "Structures" and "Bonus Chest" (inactive in hardcore) as switches of 44.
- More: game rules (same screen as the in-game version, game defaults), experiments (three
  feature packs), data packs (available / selected; "Default" fixed; adding
  an experimental pack → warning).
- Folder: name sanitized (forbidden characters and `.`, `/`, `"` → `_`, reserved Windows names
  surrounded by `_`), free "(n)" suffix. Empty name → "New World".
- "Create": screen "Preparing to create the world…", generation in a background task
  (`rc_sim::newworld::create_world`, then packs and features in `level.dat`), then opening.
- Re-create: name, mode, difficulty, cheats, seed, structures, chest, rules and packs carried over;
  type deduced from the generator like the game: flat → "Superflat" (layers kept), debug → debug,
  any noise generator → "Default" reset (quirk: an amplified, large biomes or single biome world
  is re-created as a normal world); other generator → "Custom"
  inactive, carried over as is.
- Changing the type resets the setting of the chosen type to its default (flat layers, single biome
  "Plains"): a customized setting is lost when cycling through the types.
- Enter creates the world when no component uses it (name field focus included).

## 3. Editing

Centered column of 200 (height 234, spacing 5), title at y = 15: name (Enter = save),
"Reset Icon" (deletes `icon.png`), world folder, "Make Backup"
(zip `backups/YYYY-MM-DD_HH-MM-SS_<folder>.zip`, entries `<folder>/…`, without `session.lock`,
notification "Saved: … / Size: n MB" rounded up to the next MiB), backups
folder, "Optimize World" (offered backup, "erase cache" checkbox, progress
screen: count, converted, skipped, bar per dimension, percentage). The world is
locked while editing.

## 4. Exit and autosave

- "Save and Quit": "Saving world…" on top of everything until the end of the
  write and the lock released, then the title screen.
- Indicator: "Saving world…" at the bottom right (5 from the edge), opacity smoothed by 0.2 per
  tick towards 1 during an integrated server save, 0 otherwise (`showAutosaveIndicator`).

## 5. Local network

The "Open to LAN" button of the world options is present but inactive: the integrated server
cannot yet open itself to the network (reason in the tooltip).

## 6. Known differences

- Conversion of worlds from old versions not done: "Upgrade & Play" inactive.
- No item icons in the lists of the flat worlds (empty slot).
- Data packs: no packs from the world folder at creation, no search, generic
  icon; "Open Pack Folder" inactive.
- "Debug Mode" type (Alt key) not offered; "old customized" worlds not handled.
- Optimization: chunks from an older version are counted "skipped" (no
  conversion); only `region` files are walked.
- "Report Bug" link opened without a confirmation screen.
