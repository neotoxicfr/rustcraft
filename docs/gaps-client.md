# Gaps for a playable, identical client (survey of 12/10/2026)

Goal: full parity with the official 26.3 client first, optimisation afterwards. Survey
made on the main branch (commit `428eb36`) by comparing the inventory of screens, settings,
keys and behaviours of the official client (decompiled sources outside the repository) with the code. Each
status is proved by a file or a symbol; "in progress" = work by another agent at the time
of the survey (special screens, advancements and statistics, singleplayer milestone 1 `rc-sim`, world generation
parts 2 and 3a), not detailed here.

Legend — status: **DONE**, **PARTIAL** (what is missing is stated), **ABSENT**, *in progress*,
*out of scope* (Microsoft account, Realms, reporting). Effort: S (≤ ½ day), M (1–2 days),
L (more).

## Proposed missions (grouped by file)

Recommended order: M1 and M5 first (what prevents playing), then M2, M3, M6, M4; M7 after singleplayer
milestone 1; M8 last. The additions to `rc-client/src/link.rs` and `ids.rs` are
function additions at the end of the file: several missions can write there, easy merge.

| # | Mission | Contents | Main files |
|---|---|---|---|
| M1 | **Rebindable keys and inputs** | Action → key table read from `options.txt` (all game keys are hard-coded); crouch/sprint/attack/use toggles; sprint window; auto jump; cinematic camera; scroll wheel sensitivity and discrete mode; F2 capture into `screenshots/` + message; F11 fullscreen; pause on focus loss; inactive frame limit; quit shortcuts; quick actions key | new `keys.rs`, new `screenshot.rs`, `main.rs` (keyboard block 2486–2560, inputs 1630), `interact.rs` (`key`), `camera.rs`, `hud.rs` (`scroll`), `rc-physics` (auto jump, sprint window) |
| M2 | **Clickable text and full chat** (DONE 12/10, except `hideMatchedNames`: list to fill) | Click, hover and insertion events in components; chat links + confirmation screen + opening the browser; copy, suggest, run a command, change page; hover tooltips (text, item, entity); chat settings applied (scale, width, heights, line spacing, delay, links, hidden names, drafts, secure chat only); input field: copy/paste, Home/End, Delete, Ctrl+arrows, selection; IME input | `rc-text/src/lib.rs`, `chat.rs`, `commands.rs` (field keys), new `links.rs`, `main.rs` (`chat_key` only) |
| M3 | **Missing options screens and menus** | World options from the pause menu (difficulty, lock, personal game mode, in-game rules); Online; Telemetry; Credits and attributions; Font; server buttons of the pause menu; code of conduct screen; multiplayer warning; accessibility onboarding; social interactions (hide a player); cursor shapes | `menus.rs`, new `world_options.rs`, `front.rs`, `widgets.rs`, `server_list.rs`, `rc-client/src/link.rs` + `ids.rs` (`change_difficulty`, `lock_difficulty`, `set_game_rule`), `conn.rs` (code of conduct) |
| M4 | **Video and sound settings applied** | Vsync, mipmaps, filtering/anisotropy, ambient occlusion, biome blend radius, cutout leaves, improved transparency, section fade, update priority, entity shadows and distance, glints (speed, strength), darkness, high-contrast outline, exclusive fullscreen and resolution; sound: device, music frequency, music toast, directional audio | `render.rs`, `rc-mesh/src/blocks.rs`, `entity_render.rs`, `item_atlas.rs`/`item.wgsl`, `outline.rs`, `world_sounds.rs`, `rc-audio`, `main.rs` (`apply_options` only) |
| M5 | **End of the game, sleeping, spectator, spyglass** | Missing game events (0, 4, 6, 9, 10, 12); end screen (poem + credits, then respawn); chat in bed with "Leave Bed" and sleep fade; spectator menu (1–9, wheel, teleport, player outlines); spyglass (zoom + overlay) and bow field of view | new `win_screen.rs`, new `spectator.rs`, `main.rs` (`GameEvent` arms), `camera.rs`, `overlays.rs`, `hud_more.rs`, `rc-client/src/link.rs` (`teleport_to_entity`) |
| M6 | **Remaining items, effects and toasts** — DONE (12/10, except differences listed) | Bundles (tooltip, wheel, `bundle_item_selected`, predicted clicks, bar, sounds); effects next to the inventory and creative; particles 1500–3xxx and critical hits (`animate` 1/2); jukebox + "Now Playing"; music, system (insecure server), tutorial toasts; death screen (real score, hardcore); special icons (heads, banners, shields, chests, Shulker); player in the inventory | `world_fx.rs`, `inv_effects.rs`, `bundle.rs`, `item_special.rs`, `tutorial.rs`, `toasts.rs`, `death.rs` |
| M7 | **Singleplayer: world screens** — DONE (12/10, differences: `docs/specs/world-screens.md` § 6) | World list, creation (game/world/more tabs, rules, experiments, data packs, flat presets and single biome), editing (rename, backup, folder, optimise), deletion, recovery; "saving the world" screen; opening to the local network; autosave indicator; real pause | `front.rs` (Singleplayer button), new `world_list.rs`, `create_world.rs`, `edit_world.rs`, `rc-sim` |
| M8 | **Technical blocks and data queries** (DONE 12/10; differences: beam and error markers of the test instance, F3+I Shift on the client side) | Command block (and minecart) editors, structure, jigsaw, test block, test instance; F3+I with server data (block and entity tag queries) | new `tech_blocks.rs`, `debug_keys.rs` (`debug_copy_recreate`), `rc-client/src/link.rs` + `ids.rs` |

## The gaps that prevent normal play

1. **Keys not rebindable**: the binding screen writes `options.txt`, but the game reads
   fixed physical keys (`main.rs`: ZQSD/WASD 1630–1636, `T`, `/`, `E`, `Q`, `F`, `F1`,
   `F5`, `1`–`9`, `Tab` l. 1058). Only the F3 keys (`debug_keys.rs`) and the creative hotbars
   (`creative_app.rs:632`) follow the settings.
2. **Blocking end of the game**: game event 4 (leaving the End) is not handled
   (`main.rs:627`, `rc-env` `event`: only 1, 2, 3, 7, 8, 11, 13); with no end screen or
   respawn request, the player stays stuck in the End.
3. **Bed**: DONE (M5, `bed.rs`): chat in bed, "Leave Bed", fade, position and camera
   in bed; real going to bed not seen on screen (test server stuck on daytime).
4. ~~**Clickable text absent**~~ DONE (M2, 12/10): click, hover, insertion (`links.rs`).
5. **Spectator**: DONE (M5, `spectator.rs`): menu 1–9, wheel, middle button,
   `teleport_to_entity`; remaining: player outlines (no glowing outline pass).
6. **Bundles**: DONE (M6, `bundle.rs`): grid tooltip, wheel (`bundle_item_selected`), predicted clicks with sounds, bar; content fingerprint not computed (the server sends the slot back).
7. **Crouch/sprint toggles and auto jump**: settings saved, no effect.

## Screens

| Element | Status | Proof / what is missing | Effort |
|---|---|---|---|
| Title (panorama, logo, splash, fade, music) | DONE | `front.rs` `title_widgets`, `draw_title`; Singleplayer → world screens (M7), Realms and friends inactive (intended) | — |
| Multiplayer: list, status, LAN, add/edit, direct connection, deletion | DONE | `server_list.rs`, `front.rs` `Screen` | — |
| Connection, disconnection, world loading | DONE | `front.rs` `Connecting`, `Disconnected`, `Loading` | — |
| In-game reconfiguration | DONE (M3) | `notices.rs` `Reconfig` screen (button active after 30 s); world not cleared before the new `login` | — |
| Server code of conduct | DONE (M3) | `notices.rs`: screen, refusal (disconnection), `acceptedCodeOfConduct` memory in `servers.dat` | — |
| Multiplayer warning (1st time) | DONE (M3) | `notices.rs` `Notice::Multiplayer` | — |
| Accessibility onboarding (1st launch) | DONE (M3) | `notices.rs` `Onboarding` (narrator inactive) | — |
| Pause menu | DONE (M3) | world options, server button (tag `pause_screen_additions` / links), social interactions; the open dialog returns to the game, not to the pause menu; external icons *out of scope* | — |
| World options (difficulty, lock, personal mode, in-game rules, LAN) | DONE (M3) | `world_options.rs`; singleplayer: difficulty, lock and game rules (read, write, bounds) applied by the integrated server (W8a, `rc-sim/solo_packets.rs`), personal mode sent, local network active (S9: port, guest commands, imposed mode); default mode / commands shown with no effect | — |
| Options: main screen | DONE (M3) | telemetry inactive with `options.telemetry.disabled` (no telemetry) | — |
| Graphics, Sound, Chat, Accessibility, Controls, Mouse, Skin, Language | DONE (screens) | `menus.rs` `rows`; effects of the settings: see "Options" | — |
| Key binding | DONE (screen) | `menus.rs` `Id::Keybinds`; no effect in game (see Keys) | — |
| Font (forced Unicode, Japanese variants) | DONE (C3) | screen + effect: fonts reloaded on change (`video.rs` `font_opts`, `Gui::set_fonts`), seen (forced Unicode); Japanese variants: `jp` filter not seen | — |
| Online (Realms notifications, server list) | DONE (M3) | friends inactive (Microsoft account) | — |
| Telemetry (info) | DONE (M3) | button inactive + tooltip, like the game without telemetry | — |
| Credits and attributions | DONE (M3, W6b) | `credits_roll.rs`: Space speed-up (×5) / Ctrl (+15 each), Up arrow rewind, credits vignette (W6b, 18/10); code still separate from `win_screen.rs` | — |
| Resource packs (+ drag and drop) | DONE (differences G1-F06) | `pack_screen.rs`, `reload.rs`; load failure (default font): player packs then server packs removed, `resourcePack.load_fail` toast (`resources.rs` `load_recovering`). Differences: second failure with server packs without disconnection or return to the title screen; OpenType fonts with CFF outlines not read (empty provider, the game reads them through FreeType) | S |
| Server pack (request, download) | DONE | `server_packs.rs` | — |
| Server dialogs | DONE | `dialog.rs` | — |
| Death screen | PARTIAL | `death.rs`: score always 0 (l. 65), no "Spectate World" in hardcore mode nor confirmation | S |
| End screen (poem, credits) | ABSENT | event 4 not handled | M |
| Chat in bed ("Leave Bed") | DONE (not seen: night required) | `bed.rs`; spec `end-sleep-spectator.md` § 2 | — |
| Chat, command suggestions | DONE | `chat.rs`, `commands.rs`; field gaps: see Behaviours | — |
| Link opening confirmation | DONE | `links.rs` `Confirm` (link and command to confirm) | — |
| Inventory, creative, recipe book | DONE | `inventory.rs`, `creative_screen.rs`, `recipe_book.rs`; active effects on the right (M6, `inv_effects.rs`); local player drawn in the frame (M6) | — |
| Chests, dispenser, hopper, crafting table, furnaces, brewing stand | DONE | `menu.rs` `Kind` | — |
| Workbenches, merchant, mounts, books, signs, anvil, beacon… | *in progress* | special screens (branches in progress); Shulker box and crafter to check after merge (`Kind::Generic` today) | — |
| Advancements, statistics | DONE (screens; integrated server S9: jar triggers, rewards, saves, `award_stats`) | `advancements-stats.md` § 7-8; W8c (18/10): totem, shield, buckets, bundles, recipe book, loot (shifted location, direct attacker, source, vehicle, block entity contents, providers) and missing triggers added; remaining unobtainable for lack of a mechanic: useful `started_riding` (boats absent), `crafter_recipe_crafted` (the crafter does not craft), `spear_mobs` (spear absent), `lightning_strike` (lightning only visual), `kill_mob_near_sculk_catalyst` (catalyst not simulated) | — |
| Command block (+ minecart), structure, jigsaw, test, test instance | DONE | `op_screens.rs` (full editors, command suggestions), `tech_blocks.rs` (data, structure boxes); remaining: coloured beam and error markers of the test instance | — |
| Spectator menu | DONE | `spectator.rs` (teams: order by name, the official one follows its table) | — |
| Mode switcher F3+F4 | DONE | `debug_switcher.rs` | — |
| Debug options F3+F6 | DONE | `debug_options.rs` | — |
| Social interactions (P) | PARTIAL (M3) | screen and hiding done (from the pause menu); P key (M1), player faces absent | S |
| Friends, reporting, banning, Realms | *out of scope* | buttons inactive (`front.rs:556`, `menus.rs:1179`) | — |
| Singleplayer: list, creation, editing, backup, LAN, recovery | ABSENT (*rc-sim in progress*) | see Singleplayer | L |

## Options (`options.txt`)

All the official keys are read and rewritten (`options.rs`, checked key by key). The
table only lists their **effect** in game.

| Settings | Status | Proof / what is missing | Effort |
|---|---|---|---|
| Volumes of the 11 categories, gamma, clouds and distance, vignette, particles, render and simulation distance (sent), field of view and effects, view bobbing, damage tilt, interface scale, windowed fullscreen, max FPS, advanced tooltips, subtitles, attack indicator, menu blur, notification duration, text opacities, "chat only" background, hidden lightning flashes, dark background, panorama, splash, language, main hand, skin parts, weather radius, operator tab, autosuggestions, chat colours, reduced debug info, hidden address | DONE | `main.rs` `apply_options` 2155+, reads `options.*(...)` | — |
| `enableVsync`, `maxFps` | DONE | `video.rs` (`present_modes`, `FrameLimiter`), `Renderer::set_vsync`; `RUSTCRAFT_VSYNC=0\|1` forces vsync; captures are not limited except with `RUSTCRAFT_FPSLIMIT=1`. C3: `Fifo` mode requested for vsync (not `AutoVsync`/FifoRelaxed); on the RTX 5080 windowed Fifo presentation did not block (3,700 fps): software cap at the monitor refresh rate when vsync is on (117 fps seen at 120 Hz); `render` returns "frame presented" and the counter no longer counts surfaces without a frame | — |
| `mipmapLevels`, `textureFiltering`, `maxAnisotropyBit` | DONE | sampler rebuilt on the fly (`Renderer::set_atlas_sampler`), RGSS in `terrain.wgsl`; mip levels taken when leaving the video screen like the game (no texture reload: mips bounded by the sampler) | — |
| `ao` | DONE | `MeshOptions::ao` (flat light otherwise), full re-meshing on change | — |
| `biomeBlendRadius` | DONE | `MeshOptions::blend` (0..7), full re-meshing | — |
| `cutoutLeaves`, `chunkSectionFadeInTime`, `prioritizeChunkUpdates` | DONE | opaque leaves + faces between leaves removed (`blocks.rs`); `Renderer::fade_secs`; synchronous meshing (`VideoState::mesh_sync`, per column, distance to the column centre) | — |
| `improvedTransparency` | ABSENT (documented difference, C3) | the game renders translucents (terrain, particles, weather, clouds) into separate targets recomposed by a multi-target post-processing effect; the Rust pipeline only sorts translucent columns. Redoing this touches every pass of `render.rs`: out of reach to do cleanly | L |
| `entityShadows`, `entityDistanceScaling` | DONE | `EntityRenderer::shadows`, `within_render_distance` (the game's rules per kind, scale `clamp(distance/8, 1, 2.5) × setting`) | — |
| `glintSpeed`, `glintStrength` | DONE | `ItemAtlas::glint` (interface items; world glints absent anyway) | — |
| `darknessEffectScale` | DONE (C1) | lightmap + fog and sky of Darkness and Blindness (`rc-env` `fog`, `environ.rs`) | — |
| `highContrastBlockOutline` | DONE | `outline.rs`: 7 px black background + opaque 0x57FFE1 outline, depth offset | — |
| `highContrast` (pack) | ABSENT | inactive button `menus.rs` | S |
| `exclusiveFullscreen`, `fullscreenResolution` | DONE | `video.rs` `fullscreen_for` / `fullscreen_tick`; mode slider in the video screen; mode applied when leaving the screen like the game; current mode approximated (screen size, highest refresh rate) | — |
| `overrideWidth/Height` | ABSENT | launch window size | S |
| `soundDevice` | DONE | `rc_audio::output_devices`, `AudioEngine::with_device` ("OpenAL Soft on …" names kept), cycle button of the sound screen | — |
| `directionalAudio` | ABSENT (documented difference, C3) | the game's option enables OpenAL's binaural HRTF rendering, with its filter sets; the `rc-audio` mixer only does constant-power panning and ships no HRTF filter set | M |
| `musicFrequency` | DONE | `Music::set_frequency` (delay removed like the game) | — |
| `musicToast` | DONE (M6) | `world_fx.rs` `MusicToast`: sliding toast ("pause and toast") and fixed in the pause menu; title of the file played (`Music::now_playing_key`). Not seen on screen (game music after 10–20 min); absent from the title screen | — |
| `inactivityFpsLimit`, `pauseOnLostFocus` | DONE | `keys.rs` `frame_limit` (maxFps, 60 outside a world, 10 minimised, 30/10 inactive); pause according to the setting (`main.rs` `menus_tick`) | — |
| `preferredGraphicsBackend` | DONE | read at startup (`gpu_backend.rs`): "default"/"vulkan" → Vulkan then DX12, "opengl" → DX12 then Vulkan; red banner and "restart required" tooltip in the video screen; `RUSTCRAFT_BACKEND` always forces | S |
| `toggleCrouch`, `toggleSprint`, `toggleAttack`, `toggleUse` | DONE | `keys.rs` (toggles, memory when a screen closes) | — |
| `sprintWindow` | DONE | `rc-physics` `sprint_window` | — |
| `autoJump` | DONE | `rc-physics` `update_auto_jump` (`movement.md` § 2.6), improved jump included (17/10) | — |
| `mouseWheelSensitivity`, `discrete_mouse_scroll`, `allowCursorChanges` | PARTIAL | wheel DONE (`keys.rs` `game_scroll`, horizontal axis, spectator flight); cursor shapes DONE (M3, `widgets::request_cursor`) | S |
| `quitShortcuts`, `narratorHotkey` | PARTIAL | `quitShortcuts` DONE (`keys.rs` `quit_allowed`); narrator absent | S |
| `chatScale`, `chatWidth`, `chatHeightFocused/Unfocused`, `chatLineSpacing`, `chatDelay` | DONE | `chat.rs` `apply_options`, geometry `geo`, `queue` | — |
| `chatLinks`, `chatLinksPrompt` | DONE | `links.rs` `click_event` | — |
| `saveChatDrafts`, `onlyShowSecureChat` | DONE | `chat.rs` `close_with`, `show` | — |
| `hideMatchedNames` | DONE | `Chat::blocked` filter, list filled by social interactions (M3, hiding in `rc-client/chat.rs`) | — |
| `showAutosaveIndicator` | DONE | "Saving world…" indicator of singleplayer (`world_list.rs`) | — |
| `narrator` | ABSENT | narrator unavailable (`menus.rs:291`); see Behaviours | L |
| `tutorialStep` | ABSENT | no tutorial | S |
| `forceUnicodeFont`, `japaneseGlyphVariants` | DONE (C3) | see "Font" above | — |
| `skipMultiplayerWarning`, `onboardAccessibility`, `joinedFirstServer` | ABSENT | corresponding screens absent | S |
| `realmsNotifications`, `allowServerListing`, `telemetryOptInExtra` | DONE (M3) | Online screen; `telemetryOptInExtra` irrelevant | — |
| `rotateWithMinecart` | ABSENT | inactive button `menus.rs:332` (experimental) | S |

## Keys and combinations

| Key (default) | Status | Proof / what is missing | Effort |
|---|---|---|---|
| Movement, jump, crouch, sprint | DONE | `keys.rs` (options.txt table re-read every tick, conflicts, repeats) | — |
| Inventory E, drop Q / Ctrl+Q, swap F, 1–9, wheel | DONE | `keys.rs` (game, container screens `screen_key_actions`, creative) | — |
| Attack / use / pick block (mouse) | DONE | rebindable (mouse or keyboard, side buttons) `keys.rs`; block pick `interact.rs` `pick_block` | — |
| Chat T, command / | DONE | `keys.rs` | — |
| Player list Tab | DONE | `keys.rs` `PLAYERLIST` | — |
| F1 hide the interface | DONE | `keys.rs` | — |
| F5 view | DONE | `keys.rs` | — |
| F4 spectator effects | DONE | `keys.rs` | — |
| F2 screenshot | DONE | `screenshot.rs` (PNG in `screenshots/`, message; link click: M2) | — |
| F11 fullscreen | DONE | `keys.rs` `global_input` (windowed fullscreen) | — |
| Cinematic camera (no key) | DONE | `keys.rs` `mouse_frame` (per-frame smoothing) | — |
| Advancements L | *in progress* | — | — |
| Quick actions G | DONE | `keys.rs` `quick_actions` (tag `quick_actions`); not checked on screen (test server without tag) | — |
| Social interactions P, friends O | ABSENT / *out of scope* | — | — |
| Creative hotbars C+n / X+n | DONE | `creative_app.rs` `hotbar_activator` | — |
| Spectator: movement (forced flight, never on the ground, passes through blocks, no block effect or ladder) | DONE (17/10, `rc-physics` `Player::spectator`, test `spectator_always_flies_and_passes_through_blocks`); before: flight cut on the ground then endless fall into the void (black screen in the End, singleplayer and multiplayer); leaving a suffocating block (§ 2.2 step 7) DONE on 17/10 (`move_towards_closest_space`) | — |
| Local physics: effects (improved jump, levitation, slow falling, dolphin's grace, blindness without sprint), crawling pose, ladders and speed factor ignored in flight | DONE (17/10, `rc-physics` `MoveEffects`, `Pose`, tests `crawls_under_a_one_block_ceiling`, `flying_ignores_*`…); W4a (18/10): elytra gliding (start, command 6, physics, pose, rockets), knockback received from the server, sprint ×1.3 (`minecraft:sprinting` modifier), scaffolding and powder snow according to the player, trapdoor on ladder, pushing by neighbours (team rules), `f32` box — DONE (`movement.md` § 9, seen in game on a vanilla server); remaining: client-side Riptide trigger, minecart pushing; W3b (18/10): elytra wings and poses (flight, swimming, bed, Riptide) in rendering — coded and tested (unit tests, `entities.md` W3b addendum), not yet seen in game | M |
| Spectator: middle wheel, outlines | PARTIAL | middle button: `spectator.rs`; wheel (menu, flight speed) through the shared wheel `keys.rs` `game_scroll` (sensitivity, discrete mode, accumulation, horizontal axis for the menu); glowing outline of glowing entities DONE (C1, `entity_outline.rs`); "spectator outlines" key has no default, unbound | S |
| F3 alone and all F3+A/B/C/D/F6/G/H/I/L/N/P/Q/T/F4/1–4/Esc… combinations | DONE | `debug_keys.rs` `debug_action` (follows the settings); F3+I with server query (`tech_blocks.rs`); Shift+F3+I: client entity data not reconstructed | — |
| F7 free camera | specific to Rustcraft | only with `RUSTCRAFT_FREECAM=1` (`keys.rs` `game_keyboard`) | — |
| Narrator Ctrl+B | ABSENT | narrator absent | — |

## Visible behaviours outside the protocol

| Behaviour | Status | Proof / what is missing | Effort |
|---|---|---|---|
| Clickable chat links and actions, books, death screen; hover | DONE | `links.rs`; signs: click handled by the server. Hand when hovering clickable text (Shift: chat insertions); differences: simplified `show_item` tooltip (name, lore, identifier), disguised message not delayed by `chatDelay` | — |
| Copy to the clipboard | DONE | system clipboard (read `links::get_clipboard`); chat field: copy/cut/paste, keyboard and mouse selection, Home/End, Delete, Ctrl+arrows, Ctrl+Backspace; IME (composition displayed) | — |
| IME input | DONE | `ime.rs`: input allowed as long as an editable field has focus (fields, book area, multi-line dialog inputs, sign, chat), candidates placed on the cursor, composition in the floating frame `widget/preedit` (targeted block underlined, blinking cursor); committed text handled as a keystroke by each screen | — |
| Screenshot in `screenshots/` + clickable message | DONE | see F2; the click depends on clickable text (M2) | — |
| Subtitles | DONE | `subtitles.rs` | — |
| Toasts | PARTIAL | `toasts.rs`: recipes, advancements, system (insecure server, M6), tutorial (M6, `tutorial.rs`), music (M6); missing: file drop, narrator, world save/access (singleplayer) | S |
| Narrator | ABSENT | to decide (system speech synthesis) | L |
| Window: game title and icon | PARTIAL | title "Rustcraft" (`main.rs:2303`, `flow.rs:123`); to decide | S |
| Cursor: capture, release | DONE | `main.rs` `grab`; cursor shapes (M3, `widgets::request_cursor`) | — |
| Pause on focus loss | DONE | see options | — |
| Pack drag and drop | DONE | `pack_screen.rs` | — |
| Held item name above the hotbar | DONE | `hud.rs` `tick_held` | — |
| "Now Playing" message and jukebox music | DONE (M6) | `world_fx.rs`: `jukebox_song` registry, sound at the block (records category, volume 4), message with cycling colours | — |
| World event particles | DONE except absent types | `world_fx.rs`: 1500–1505, 2000–2018, 3000–3021 and missing sounds (3000, 3001, 3006, 3007, 3008); types added since (C1): `white_smoke`, `dragon_breath`, `sculk_charge(_pop)`, `shriek`, spawner detection, omen, vault connection, dripstone drip; block client tick (C2, `block_entities.rs` `emit_particles`): smoke, flames and vault connection particles, trial spawner flames and smoke according to its state; campfire smoke (W1). Random animation of blocks and fluids (667 × 2 draws, `block_anim.rs`), 43 types added (fireworks, leaves, spores, drips, bubbles…) and entity trails (W1, 18/10); vibration signal (G6-F10: destination, path, two oriented planes, animated strip); remaining: `trail`, `geyser*` (codec) and the elder guardian | S |
| Game events 0, 6, 9, 10, 12 (no bed, arrow, pufferfish, elder guardian, limited crafting) | DONE except the elder guardian particle (model rendering absent) and the demo welcome window | `bed.rs` `m5_game_event` | S |
| Spyglass (zoom, overlay); bow that narrows the view | DONE | `camera.rs` `UseFov`, `overlays.rs` `Scope`, sensitivity ÷ 8 | — |
| Bundles | DONE (M6) | `bundle.rs` | — |
| Item tooltips: potion "When Applied", trims, banner patterns, containers (5 lines then "and N more"), written book, fireworks, loaded crossbow, decorated pot, beehive, disc, horn, smithing templates, "Can break / be placed on" | DONE (C2 `tooltips.rs`, `tooltips_items.rs`; C3 `tooltips_more.rs`): tropical fish bucket, painting (title, author, dimensions; variants sent without data: language keys + extracted sizes), spawner (mob from `SpawnData` or explanation), sign text, suspicious stew (creative inventory only), seen. Remaining: "Disabled in Peaceful mode" of the eggs (`entity_data`, needs the difficulty and the entity category) | S |
| Item in use: `using_item`, `use_duration`, `use_cycle` models, crossbow charge (shield raised, bow drawn, horn, trident); hand pose for bow, trident, food and drink | DONE (C2, `stack::set_using`, `hand.rs` `use_transform`) for the local player; C3: brush and bundle in first person (seen: brush), arm poses of remote players and mannequins (held item, bow, shield, trident, held/charging crossbow, spyglass, horn, brush; `entity_render.rs` `pose_arms`, seen: item and bow), off hand already in the hotbar. ARMOUR (19/10): spear in 3rd person (held "spear" pose, kinetic use, arm and item thrust; `kinetic_weapon` parameters extracted into rc-data, seen: held pose), armour trims worn and as icon (palettes, `trim_overrides`; seen), Quick Charge read for the `crossbow/pull` property and the charging pose of other entities. Remaining: kinetic recoil of the item held by other entities, player spear in singleplayer (use damage, knockback, dismounting, thrust), crossbow charge of remote players without progress (use duration unknown), view of the local player's use in 3rd person | S |
| Head icons with the real skin, profile reduced to a name or an identifier resolved by the profile service (one-day disk cache) | DONE (C2, `item_atlas.rs` skin slots, `skins.rs` `resolve_profile`); C3: profile with identifier without `textures` → default skin of the UUID for the icon (`item_special.rs` `take_default_heads`, unit test); players in the world and placed heads: already by UUID. Remaining: profile with a name only whose resolution fails (offline UUID not computed) | S |
| Active effects in the inventory | DONE (M6) | `inv_effects.rs`: tiles, compact mode with tooltip, the game's sort (effect colour replaced by its index) | — |
| "Insecure server" warning | ABSENT | — | S |

## Singleplayer

Milestone 1 *in progress* (crate `rc-sim`: open, play, save an existing world); world generation
parts 2 and 3a *in progress*. What will be missing afterwards on the client side:

| Element | Status | Effort |
|---|---|---|
| Singleplayer button of the title screen | ABSENT (inactive `front.rs:554`) | S |
| World list (icons, search, sort, folders, locks, version) | ABSENT | M |
| World creation: tabs, mode, difficulty, commands, seed, type, structures, bonus chest, rules, experiments, data packs, flat presets and single biome | ABSENT | L |
| Editing, renaming, backup (zip), opening the folder, optimisation, deletion, data recovery | ABSENT | M |
| "Saving the world" screens, progress, data pack error | ABSENT | S |
| Real world pause (pause menu, F3+Esc), autosave indicator | ABSENT | S |
| Opening to the local network, world options (shared with M3) | DONE (S9, `rc-sim/src/lan/`); differences: offline guests without encryption, same dimension as the host, no pickup by guests, mobs tracked around the host | M |
| World icon (capture on first entry) | ABSENT | S |
| Hardcore mode: death → spectator, deletion offered | ABSENT | S |
| Integrated server, milestone 2 (survival): validated menus and clicks, crafting table, chests, furnaces, recipe book, components sent, block loot, items on the ground and orbs, health/hunger/damage/death/respawn, weather, sleeping | DONE (S2, `rc-sim` `life.rs`…) | — |
| Integrated server, S7 mechanics: status effects (tick, merge, attributes, resistance, absorption, mining), drunk potions, milk/honey/golden apple, brewing stand (pack recipes), enchanting table (player seed), anvil, grindstone, smithing (netherite, trims), stonecutter, loot chests and containers (seed, the game's filling, enchant functions), special recipes (fireworks, banner, shield, book, pot) | DONE (S7, `effects.rs`, `potions.rs`, `brewing.rs`, `enchanting.rs`, `stations*.rs`, `lootchest.rs`, `special.rs`); remaining: loom, effects on mobs, omens/raids (cartography table, map extension, thrown/lingering potions, effect arrows, exploration maps, loot read by comparator/hopper: done in S8) | M |
| Integrated server, mobs F07: pillager, vindicator, evoker (fangs, vex, wololo), vex, illusioner, ravager, witch (drunk and thrown potions); patrols; omens and raids (waves, boss bar, hero of the village); structure spawns (outpost, witch hut, fortress) | DONE (F07, `mobs/illagers/`, `mobs/structure_spawn.rs`); remaining: lightning (villager → witch ready), player shield against the ravager, bolt piercing, raid statistics and advancements, structure start in an unloaded chunk; client: item worn on the head not drawn (captain's banner) | M |
| Integrated server, mobs F12: blaze (volleys of three small fireballs, height, slow fall), ghast (jerky flight, large explosive deflectable fireball), wither skeleton (sword, wither effect, flaming arrows, targets piglins), strider (walks on lava, cold, saddle, rides with the fishing rod, boost, mounts at spawn) | DONE (F12, `mobs/nether/`, `mobs/fireballs.rs`); remaining: zombified piglin rider of the strider (waits for F13, the strider is already saddled), fireballs not saved in chunks, reactions of the blocks hit (TNT, target, bell), item pickup by monsters (shared), penalties inherited by passengers, blaze restriction area | M |
| Integrated server, mobs F13: piglin (brain: gold barter, admiration, anger at chest and gold block, hunting hoglins, dancing, crossbow, rides baby hoglins), brute (guard, axe, home), zombified piglin (contagious anger, born from portals, lightning-struck pig), hoglin (charge and toss, warped fungus, breeding with crimson fungus), zoglin (attacks everything), zombification outside the Nether | DONE (F13, `mobs/piglins/`); remaining: bastion brutes and piglins (entities of structure templates not placed by generation), portal delay for mobs (they do not go through portals), natural lightning on pigs (entry point only), barter table quantities to compare with the server |
| Integrated server, S8: player projectiles (bow, crossbow with multishot/piercing/rockets, trident with loyalty/channeling/riptide, snowball, egg, pearl with teleportation/endermite/save in the player, thrown potions and lingering cloud, experience bottle, effect and spectral arrows, rockets and elytra on the server side), pickup, dispenser, fishing (bobber, wait, bite, loot, hook), maps (colour survey, markers, banners, cartography table, expansion, lock, cloning, exploration maps from loot and trading), chorus fruit, sealed loot read by comparator/hopper, `loot … fish` | DONE (S8, `projectiles/`, `maps.rs`, `lootseal.rs`; trajectories and fishing loot identical to the official server); remaining: bubble columns (pearl stasis), shield deflection, real channeling lightning (flash only visual), frames (placed maps), maps outside loaded chunks (the game loads them), `skip_existing_chunks`, statistics; client side: fishing line, map preview in the table, cooldown received on the client (`rc-client/src/cooldowns.rs`, 18/10) but hotbar overlay not yet drawn | M |
| Integrated server, commands (S5): tree by permission level (extractor `CommandsExtractor.java`), parse errors, server suggestions, `execute` (as, at, positioned, rotated, facing, align, anchored, in, summon, on, if/unless block·blocks·entity·score·data·dimension·loaded·biome·function·items, store), gamemode, defaultgamemode, difficulty, seed, time (clocks, markers, pause, speed), weather, gamerule, say, me, msg/tell/w, teammsg, tellraw, title, list, help, kill, xp, give, clear, tag, tp, spawnpoint, setworldspawn, playsound, stopsound, particle, random, version, summon, effect, enchant, damage, rotate, attribute (base), scoreboard, team, trigger, data (get/merge/modify/remove), item replace, loot give/spawn (tables), setblock, fill, clone, forceload, worldborder, tick, swing, function, schedule, return; command blocks (impulse, repeating, chain, conditional, output, comparator) | PARTIAL (minecarts, including command block ones, simulated: G5-F06 22/10, `minecart/`; checked on screen against the official server: 30 commands, same chat lines), locate (structures and biomes), recipe, bossbar (saved); remaining: fillbiome, place, spreadplayers, ride, advancement, datapack, dialog, waypoint, stopwatch, item modify, loot insert/replace/mine/kill/fish, attribute modifier, execute in (waits for S6), effects applied to the game (speed…), `predicate`, selector `advancements=` | M |
| Integrated server, minecarts (G5-F06): six kinds, rails, mounting, breaking, contents, variants, saving, fishing hook, portals (empty or with the seated player), block effects (cobweb, powder snow, fire, cactus, cauldrons, redstone ore, freezing, burning state), fall (blocks, passengers), sweeping hit | DONE (22/10, `minecart/`); remaining: minecart carrying a mob stopped by portals (same difference as loaded mounts), effects not reproduced (bubble column, honey slide, turtle eggs trampled or crushed, sculk sensor trampled, falling big dripleaf), powder snow flakes drawn from the world's block randomness, fall sounds of passenger mobs, end of the behaviour (rotation, pushes) of a minecart removed during its movement | M |
| "In bed" screen (leave the bed) on the client | ABSENT (the server puts the player to bed and skips the night) | S |
| Generation: all 26.3 structures (fortress, monument, mansion, Nether fossil, End city included), structure entities, starts and references on disk, fluid ticks, neighbour light, flying spectator | DONE (X23, checked against an official server); X24: flat world structures (0 differences apart from order over 1,984 chunks), well barriers, icebergs and barriers against trees fixed; remaining are decoration ORDER differences between neighbouring chunks (Nether basalt deltas, vegetation in wells, monument kelp, leaves around mansions): two official servers with the same seed differ from each other in the same way (Nether 30,962 blocks over 100 chunks, main world 193,471 over 433), not reproducible block for block; W12 (18/10): heightmaps kept up to date at each placement (9,558 official chunks re-primed identically), flat world recomposed like the game (lava lakes, biome structures removed, non-blocking layers as the last step), "debug" world, void platform, bonus chest, End return gateway, checkerboard source, `fill_layer`/`replace_single_block`/`simple`, village mobs and ruin drowned finalised, exploration map references, mushrooms in the sun removed in post-processing, regeneration below y = 0 of converted chunks (against official server: debug and void 0 differences, flat 0 outside the dedicated server's spawn area, checkerboard biomes identical); remaining: old/new terrain blending, recorded starts of converted worlds, generation F3 lines (client-side heightmaps: done, G2-F12) | M |
| Items with their own ray (18/10, tester feedback "boat impossible to place"): shared fluid ray `rc-physics/ray.rs`; client: boats/rafts, buckets, bottle, lily pad, frog spawn and spawn eggs predicted like the game (gesture, bounce, blocks and sounds "except the author") and lily pad/frog spawn inert on a targeted block (before: never placeable in shallow water); integrated server: buckets on the world (sources, waterlogged, powder snow, evaporation), bottle, lily pad, frog spawn, spawn eggs in water | DONE (`interact/fluid_use.rs`, `rc-sim/fluid_use.rs`, `interactions.md` § 3.5); differences: stack transformation (bucket → water bucket) not predicted on the client (corrected by the server on the next tick), dragon breath in a bottle absent in singleplayer, adventure (`can_place_on`) refused in singleplayer | — |
| **Boats and rafts in singleplayer** (PLANNED): entity absent from the integrated server, the item does nothing in singleplayer (multiplayer: placed by the server). To do: boat/raft entity (and chest variant) on the physics of `rc-physics/mounts.rs` (unpiloted boat: buoyancy, gravity, ice), placement through `fluid_use.rs` (ray already ready: free spot, player yaw, item consumed), mounting/dismounting of the player and mobs (`mounts/riding.rs` is written for mobs), `move_vehicle`/`paddle_boat` accepted, breaking and loot, chest (menu), saving, sinking under a bubble column, boat tracking by mobs | PLANNED | L |
