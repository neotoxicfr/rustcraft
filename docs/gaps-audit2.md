# Second audit: remaining differences and missions

Date: 2026-10-21. Base: main branch. Coverage: 78/78 batches, 798 differences kept by the batches, 4 refuted.

After merging duplicates (within each group, then across the six groups): **83 missions** and **716 entries**: **137 P1**, **290 P2**, **289 P3**. A difference whose fix touches the files of two missions appears twice, one part in each. 43 missions carry at least one P1, 37 stop at P2, 3 at P3.

Severity: P1 = visible or changes the game; P2 = noticeable but rare, or visible cosmetic; P3 = minor detail (most P3s are not verified in game). Effort: S, M, L.

Agents' work list: `missions.json` in the audit folder (same content, plus the `depend_de` field). The missions are sorted P1 first; a mission only starts after those in its "After" column. Two missions that touch the same file are either merged or put in sequence by this column.

## Points to settle and to do at merge

- **"Core" shaders of resource packs** (G1-F18): translate their GLSL or accept the difference; to be looked at with the video menu and shader packs, after optimisation.
- **Light activation queue** (A2-E3, P3): the investigation recommends noting the difference as accepted (only the pace at which columns appear changes).
- **Leash** (G4-F07): one batch says it already exists between mobs (`mobs/mounts/llama.rs`, `mobs/recent/happy_ghast.rs`), two others say the opposite; the mission reads these two files first.
- **Waiting for another mechanic, no mission**: the detector rail (after the minecarts, G5-F06) and the reading of a frame by the comparator (after the frames, G5-F08).
- **`perf:redstone` bench**: the phase shift observed is faithful to the game; only the bench scene is to be changed (A2-E2).
- **At merge**: `mod` declarations of the new modules; routing lines noted "by Gx-Fyy" in the fixes; updates of `docs/gaps-client.md` (l.126 the tutorial exists, l.163-164 IME, l.197 singleplayer pause, l.207 /place, minecarts to record), of `docs/quirks-to-reproduce.md` (l.22, dragon egg) and of `docs/specs/debug-f3.md`.

## Missions

| # | Id | Mission | Sev. | Effort | P1/P2/P3 | After | Origin |
|---|---|---|---|---|---|---|---|
| 1 | G1-F02 | Advancement triggers (fields, missing callers, sources) | P1 | M | 3/3/5 | - | G1-F02 |
| 2 | G1-F03 | Projectile advancements (target_hit, killed_by_arrow) | P1 | M | 1/1/0 | - | G1-F03 |
| 3 | G1-F06 | Resource pack stacking: fonts, .mcmeta, overlays, filter, blockstates | P1 | M | 1/2/3 | - | G1-F06, G4-F17, G3-F18, G2-F12 |
| 4 | G2-F09 | Structure block (buttons) and server dialogs | P1 | L | 1/7/3 | - | G2-F09, G3-F17, G1-F14 |
| 5 | G2-F07 | Chunks at respawn, name tags, targeting and outline, client-side border, quick launch | P1 | L | 2/6/8 | - | G2-F07, G1-F17, G6-F21 |
| 6 | G2-F10 | Client particles: block tint, harmful gas, dust, flames, block_marker | P1 | L | 1/7/1 | - | G2-F10, G1-F05, G3-F16, G5-F16 |
| 7 | G2-F02 | Mobs (client): phantom, sulfur cube, reins, panda, happy ghast, shadows | P1 | L | 2/6/4 | - | G2-F02 |
| 8 | G2-F04 | Dynamic item properties (recovery compass, rod, bundle, Christmas chest) | P2 | M | 0/5/5 | - | G2-F04, G2-F01, G4-F17 |
| 9 | G2-F03 | Humanoid models, held items, compass and clock outside the HUD | P1 | L | 1/7/10 | G2-F04 | G2-F03, G4-F16 |
| 10 | G2-F05 | Item entities (carried blocks, ominous spawner, wind charge) and maps | P1 | M | 2/3/3 | G2-F04, G2-F03 | G2-F05, G1-F05, G5-F16 |
| 11 | G3-F02 | Mining, striking and breaking blocks, block pick, incoming player packets | P1 | L | 7/3/3 | - | G3-F02, G6-F03 |
| 12 | G3-F03 | Integrated server loop, datapacks, experiments, functions and execute | P2 | L | 0/5/3 | G1-F02 | G3-F03 |
| 13 | G3-F05 | LAN guests (section fluids, relays, rights, leaving) and combat between players | P1 | M | 3/7/4 | G3-F02, G3-F03 | G3-F05, G4-F14, G6-F13 |
| 14 | G3-F12 | Scoreboard and bossbar | P1 | M | 1/7/3 | - | G3-F12, G6-F16 |
| 15 | G4-F02 | Monster spawning and finalisation, and variants | P1 | M | 4/3/2 | - | G4-F02 |
| 16 | G4-F03 | Goal and sensor AI (wolf, breeding, panic, cat, villager) | P1 | L | 3/6/11 | - | G4-F03 |
| 17 | G4-F04 | Projectiles: impacts | P1 | M | 1/3/2 | G1-F03 | G4-F04 |
| 18 | G4-F05 | Projectiles: throwing, trajectory and portals | P1 | L | 1/3/2 | - | G4-F05 |
| 19 | G4-F09 | Effects and player (turtle helmet, 1.21 effects, hurt sound) | P1 | M | 1/3/1 | G1-F02, G3-F03 | G4-F09 |
| 20 | G5-F01 | Server block entities: containers, hoppers, dispenser and dropper, composter, pressure plates, copper chests | P1 | L | 11/9/4 | - | G5-F01, G4-F14, G3-F01, G6-F04, G4-F08 |
| 21 | G5-F03 | Mobs: player interactions, synced data, generic state (cat, wolf, sheep, name tag, egg) | P1 | L | 6/12/7 | G4-F02, G4-F09 | G5-F03, G3-F14, G4-F01, G5-F02 |
| 22 | G5-F02 | Entity contact, fall and environment (blocks passed through, fire, rain, magma, berries, honey, armour) | P1 | L | 12/10/3 | G5-F03 | G5-F02, G6-F05 |
| 23 | G5-F04 | Combat and offensive enchantments (mace, mob thorns, credits) | P1 | L | 3/4/4 | G3-F05, G5-F03, G5-F02 | G5-F04 |
| 24 | G5-F05 | Orbs, dropped items and entities without AI (displays, interaction, marker) | P1 | M | 3/4/2 | G3-F02, G3-F03, G5-F03, G5-F02 | G5-F05, G1-F01, G4-F08 |
| 25 | G5-F06 | Minecarts in the integrated server | P1 | L | 1/0/0 | - | G5-F06, G4-F11 |
| 26 | G5-F08 | Decorative entities on the server side: paintings, frames, armor stand, cushion, mannequin | P1 | L | 3/0/0 | - | G5-F08, G4-F06 |
| 27 | G5-F09 | Block rules: growth, neighbourhood, projectiles | P1 | L | 5/7/2 | G3-F02, G4-F04, G5-F01 | G5-F09 |
| 28 | G5-F10 | Item uses: compass, eye of ender, horn, brush, maps, knowledge book | P1 | L | 6/6/2 | G4-F05, G5-F06, G5-F08 | G5-F10, G4-F05 |
| 29 | G5-F11 | Beacon (tick, menu, effects, construct_beacon), lectern, crafter and recipe book | P1 | L | 4/3/0 | G1-F02 | G5-F11, G3-F04, G1-F04 |
| 30 | G5-F14 | Pistons: client replay, animation, pushed and stuck entities; block-entity rendering | P1 | L | 2/2/4 | G2-F07, G2-F10, G2-F05, G5-F02 | G5-F14, pistons investigation, G2-F01 |
| 31 | G5-F15 | Client: interactions, passengers, targeting | P1 | M | 2/3/2 | G2-F07 | G5-F15 |
| 32 | G5-F16 | Client: block-entity rendering, particles and sounds | P1 | M | 1/4/2 | G2-F10, G2-F05, G5-F14 | G5-F16, G2-F01 |
| 33 | G6-F01 | Server-click blocks: jukebox, lectern, shelf, decorated pot, campfire, chiseled bookshelf, statue | P1 | L | 6/7/1 | - | G6-F01, G5-F12, G6-F02 |
| 34 | G4-F07 | Leash, leash knot and mounts (saddled pig, trader llamas) | P1 | L | 3/0/1 | G3-F03, G5-F03, G5-F08, G6-F01 | G4-F07, G5-F03, G6-F22 |
| 35 | G4-F10 | Allay and parrot dance near a jukebox (server and client) | P1 | M | 1/2/1 | G2-F02, G6-F01 | G4-F10, G2-F02, G4-F15 |
| 36 | G6-F06 | Block placement, survival and shapes (rules per family) | P1 | L | 14/9/8 | G3-F02, G5-F06, G5-F08, G5-F09 | G6-F06, G5-F07 |
| 37 | G6-F23 | Tripwire, hook and vine | P1 | S | 3/0/0 | G5-F09 | G6-F23 |
| 38 | G6-F07 | Bone meal | P1 | M | 2/1/0 | G5-F09, G6-F06 | G6-F07, G5-F09 |
| 39 | G6-F08 | Nature ticks and turtle eggs | P1 | L | 5/4/1 | G5-F02, G5-F09, G6-F06, G6-F23 | G6-F08, G5-F09 |
| 40 | G6-F09 | Sulfur and projectiles | P1 | L | 3/1/0 | G5-F09, G6-F06 | G6-F09, G5-F12 |
| 41 | G6-F10 | Vibrations and redstone: sculk sensors, allay, particle, wooden buttons, blocked chest | P1 | L | 1/5/0 | G5-F01, G6-F01, G4-F10 | G6-F10, G4-F10, G5-F13 |
| 42 | G6-F11 | Sleeping and beds (straw bed, waking, announcements), waxed signs, multiplayer spawns, dragon fight | P1 | M | 1/4/5 | G1-F02, G3-F12, G4-F05, G5-F10 | G6-F11, G4-F14, G3-F13, G5-F18, G6-F01 |
| 43 | G6-F12 | Block-entity tickers, conduit, containers | P1 | M | 2/1/1 | G5-F01, G6-F10 | G6-F12, G5-F12 |
| 44 | G6-F18 | Generation: ticks placed by decoration elements | P1 | M | 1/2/1 | - | G6-F18 |
| 45 | G6-F14 | Technical blocks in singleplayer (structure, jigsaw, test), templates, /place, /clone, /locate, border | P1 | L | 1/9/6 | G3-F02, G5-F05, G6-F11 | G6-F14, G3-F04, G3-F09, G6-F15 |
| 46 | G1-F07 | Advancement and loot predicates | P2 | M | 0/1/9 | - | G1-F07 |
| 47 | G1-F08 | Entity rendering and glints | P2 | M | 0/4/0 | G2-F07, G2-F03, G4-F10 | G1-F08, G2-F03, G2-F02 |
| 48 | G1-F09 | Text input, IME, chat and widgets | P2 | L | 0/5/6 | - | G1-F09, G2-F09, G2-F12 |
| 49 | G1-F11 | Item tints (firework, custom_model_data, team) | P2 | S | 0/2/1 | G2-F04 | G1-F11 |
| 50 | G1-F12 | HUD, bundle and spectator | P2 | S | 0/2/2 | G2-F04 | G1-F12 |
| 51 | G1-F13 | Bidirectional text and items in text | P2 | M | 0/2/0 | - | G1-F13, G2-F12, G3-F18 |
| 52 | G1-F14 | World screens and statistics | P2 | M | 0/2/6 | - | G1-F14 |
| 53 | G1-F15 | Singleplayer loading, title screen, symbolic links | P2 | M | 0/1/3 | G1-F06, G2-F07, G2-F04, G3-F03 | G1-F15, G3-F18, G6-F15 |
| 54 | G1-F10 | Main loop: pause, draw order, bed, crashes, toasts | P2 | L | 0/5/7 | G5-F14, G1-F15 | G1-F10, G2-F07, G3-F15, G2-F11 |
| 55 | G1-F16 | F3 and debugging | P2 | M | 0/2/7 | G5-F16 | G1-F16 |
| 56 | G2-F06 | First-person hand, leash, elytra and End sounds | P2 | M | 0/4/2 | G2-F10, G5-F15 | G2-F06, G3-F15 |
| 57 | G2-F08 | Singleplayer world: world pack, creation, world list, packs | P2 | L | 0/4/9 | G1-F09, G1-F14, G1-F15 | G2-F08 |
| 58 | G2-F11 | Inventory screens, recipe book and options menus | P2 | M | 0/4/6 | G5-F16, G1-F09, G1-F12, G1-F14 | G2-F11 |
| 59 | G2-F12 | Chat, blocked servers, block models (bake) and client data | P2 | L | 0/2/5 | G1-F06, G1-F09 | G2-F12 |
| 60 | G3-F06 | Gametest (/test, test instances) and admin broadcast of commands | P2 | L | 0/1/1 | G5-F01, G6-F14 | G3-F06 |
| 61 | G3-F07 | Basic commands and world creation | P2 | M | 0/3/6 | G2-F07, G3-F05 | G3-F07 |
| 62 | G3-F08 | Commands: compute, fetchprofile, posteffect, attributes, loot, enchant | P2 | L | 0/4/6 | G5-F05 | G3-F08 |
| 63 | G3-F10 | NBT, SNBT and /data | P2 | M | 0/1/9 | G5-F08 | G3-F10 |
| 64 | G3-F11 | Arguments, selectors, predicates and text and item components | P2 | L | 0/5/9 | G1-F07 | G3-F11 |
| 65 | G3-F13 | World tick order and chunks | P2 | L | 0/2/1 | G5-F01, G5-F02, G5-F05, G5-F10, G6-F12 | G3-F13 |
| 66 | G4-F08 | Monsters and entities: creeper, shulker, TNT, sulfur cube, creaking, warden | P2 | M | 0/4/9 | G3-F13 | G4-F08 |
| 67 | G4-F12 | Iron and copper golems | P2 | M | 0/2/2 | G5-F03, G6-F01 | G4-F12 |
| 68 | G4-F13 | Piglins and spear | P2 | S | 0/2/1 | - | G4-F13 |
| 69 | G4-F14 | Integrated server: pack tags, /reload, clocks | P2 | L | 0/2/2 | G6-F14, G3-F06, G3-F08 | G4-F14 |
| 70 | G4-F15 | Client: effects of animals and small mobs | P2 | M | 0/6/0 | G2-F02, G6-F10, G1-F08 | G4-F15 |
| 71 | G4-F16 | Client: monster effects (dragon, creaking, illagers, breeze, warden) | P2 | L | 0/6/0 | G1-F08 | G4-F16 |
| 72 | G4-F17 | Client: trading, input, tooltips, profiling and miscellaneous | P2 | M | 0/1/8 | G5-F14, G5-F15, G1-F09, G1-F11, G1-F15, G1-F10, G2-F11 | G4-F17 |
| 73 | G5-F17 | Spawns and spawn eggs | P2 | M | 0/3/7 | G4-F02, G4-F08 | G5-F17 |
| 74 | G6-F15 | Saving, /schedule, /data, world list | P2 | M | 0/1/6 | G4-F07, G6-F14, G2-F08, G3-F10, G3-F13, G4-F14, G4-F17 | G6-F15, G3-F06 |
| 75 | G6-F17 | Loot: functions, providers, entries | P2 | L | 0/1/7 | G1-F07, G3-F08, G3-F11 | G6-F17 |
| 76 | G6-F19 | Structures: saving of pieces and placed mobs | P2 | S | 0/2/4 | - | G6-F19 |
| 77 | G6-F20 | Navigation and maps | P2 | M | 0/1/4 | G5-F10, G4-F08 | G6-F20 |
| 78 | A2-W1 | Locator bar waypoints: integrated server, LAN and /waypoint | P2 | L | 0/2/0 | G4-F02, G4-F14 | G4-F14, G5-F05, G6-F20, G3-F08 |
| 79 | A2-E2 | Redstone (investigation): order of sending block changes, bench scene, client queue | P2 | M | 0/2/1 | G6-F14, G4-F14, G4-F17 | redstone investigation |
| 80 | A2-E3 | Meshing and occlusion (investigation): loaded unmeshed columns, light queue, walk distance | P2 | M | 0/3/1 | A2-E2 | G2-F12, meshing investigation |
| 81 | G1-F17 | Options and menus | P3 | S | 0/0/8 | G2-F06, G2-F11 | G1-F17 |
| 82 | G1-F18 | Pack core shaders (after optimisation, to be decided) | P3 | L | 0/0/1 | - | G1-F18 |
| 83 | G6-F21 | Client: targeting, particles, options, meshing | P3 | S | 0/0/3 | G5-F15, G2-F11, A2-E3 | G6-F21 |

## P1 and P2 differences, per mission

Each line: severity, originating batches, difference, then the proposed fix with our files and lines. Merged duplicates are carried over between brackets.

### G1-F02: Advancement triggers (fields, missing callers, sources)

Files: `crates/rc-sim/src/progress/trig.rs`, `crates/rc-sim/src/progress/mod.rs`, `crates/rc-sim/src/dims.rs`, `crates/rc-sim/src/mobs/golems/build.rs`, `crates/rc-sim/src/mobs/end/fight.rs`, `crates/rc-sim/src/mobs/effects.rs`, `crates/rc-sim/src/mobs/brainy/axolotl.rs`.

- **P1** (L05) recipe_crafted reads recipe_id instead of recipes. Fix: trig.rs:337: read recipes; fix the comment trig.rs:651.
- **P1** (L05) player_generates_container_loot reads loot_table instead of loot_tables. Fix: trig.rs:411: accept a string or a list (recipe_in renamed id_in).
- **P1** (L05) summoned_entity never triggered for golems or the respawned dragon. Fix: Call trig_summoned in golems/build.rs:246 (players in the golem's box inflated by 5) and end/fight.rs:852 (fight players); remove the dead_code at trig.rs:536.
- **P2** (L05) effects_changed without source or renewal (kill_axolotl_target). Fix: Trigger in the effect add/update path (mobs/effects.rs) with the source; the axolotl passes its source (axolotl.rs:188-196); remove the sweep trig.rs:97-102.
- **P2** (L05) slide_down_block never triggered (Sticky Situation). Fix: In the server check of traversed blocks, detect sliding on honey_block with the honey block rule and call trig_slide (trig.rs:623) when game_time % 20 == 0.
- **P2** (L05) nether_travel: Nether departure not saved. Fix: Use dims.entered_nether (dims.rs:529-534) read before erasure; remove prog.nether_entry (mod.rs:277).

### G1-F03: Projectile advancements (target_hit, killed_by_arrow)

Files: `crates/rc-sim/src/projectiles/hits.rs`, `crates/rc-sim/src/projectiles/progress.rs`.

- **P1** (L05) target_hit rejects any projectile condition (Bullseye). Fix: progress.rs:70-77 and call hits.rs:1170: pass the projectile's position and type, evaluate c[projectile] like bolt_cond (trig.rs:742-787).
- **P2** (L05) killed_by_arrow ignores the victims accumulated by piercing, non-consuming conditions. Fix: Keep on the projectile the list of killed targets (piercing > 0), pass it to s8_killed_by_arrow (progress.rs:94-115) with the consuming logic of trig.rs:594 (read-only).

### G1-F06: Resource pack stacking: fonts, .mcmeta, overlays, filter, blockstates

Files: `crates/rc-assets/src/font.rs`, `crates/rc-assets/src/source.rs`, `crates/rc-assets/src/texture.rs`, `apps/client/src/pack_stack.rs`, `crates/rc-assets/src/packs.rs`, `crates/rc-assets/src/bake.rs`.

- **P1** (L12) Font definitions not stacked: a pack hides all the vanilla fonts. Fix: font.rs:316 + stack_read_all in source.rs:318: read the file in each pack, concatenate the providers from top to bottom, record each read in the cache.
- **P2** (L40) .mcmeta metadata of a lower pack. Fix: source.rs:319, texture.rs:214
- **P2** (L39) Resource pack overlays ignored. Fix: Read overlays in packs.rs, filter by format, stack (pack_stack.rs:166).

### G2-F09: Structure block (buttons) and server dialogs

Files: `apps/client/src/op_screens.rs`, `apps/client/src/dialog.rs`.

- **P1** (L14) Save/Load/Detect buttons of the structure block: wrong action sent. Fix: op_screens.rs:651-653: SAVE→(0,1), LOAD→(1,2), DETECT→(0,3).
- **P2** (L14+L37) Actions open_url/copy_to_clipboard/suggest_command/change_page ignored. Fix: dialog.rs: Action::Other goes through the click handling of links.rs:538 (read-only for links.rs). [Merge of duplicate G3-F17 "Static actions with no effect": Wire up the chat click actions (dialog.rs:478).]
- **P2** (L14+L37) Dialog inputs not editable. Fix: dialog.rs: editable widgets (text, multi-line, checkbox, cycling, slider) and escape in fill(). [Merge of duplicate G3-F17 "Dialog inputs neither displayed nor editable": Widgets of the options screens, default number_range = (start+end)/2 (dialog.rs:138-139).]
- **P2** (L14+L37) dialog_list without buttons. Fix: Dialog::parse: one Action::Show button per entry. [Merge of duplicate G3-F17 "dialog_list never displayed; pause, columns, widths, external_title ignored": Grid rendering like multi_action (dialog.rs:226).]
- **P2** (L14) Buttons in a single column, stacked footer, no scrolling. Fix: Grid of columns columns, footer row for Simple and Confirmation, scrollable area.
- **P2** (L14+L11) Body: item as text, width ignored, text clicks ignored. Fix: Item cell with tooltip (tooltips.rs), width width, clicks towards links.rs. [Merge of duplicate G1-F14 "Item body of server dialogs as text [id]": dialog.rs:184-187: draw the stack (16x16 + decorations) and the item tooltip on hover.]
- **P2** (L14) Warning button and disconnect screen absent. Fix: Top-right button and disconnect confirmation screen.
- **P2** (L14) run_command sent without the confirmation for unsigned commands. Fix: Call run_unattended (links.rs:596) instead of send_command.

### G2-F07: Chunks at respawn, name tags, targeting and outline, client-side border, quick launch

Files: `apps/client/src/main.rs`, `apps/client/src/flow.rs`, `apps/client/src/launch.rs`, `apps/client/src/interact.rs`, `apps/client/src/outline.rs`, `apps/client/src/entities.rs`, `apps/client/src/nametag.rs`.

- **P1** (L18) Chunk grid recreated with a radius of 16 at respawn and dimension change. Fix: main.rs:563 and flow.rs:674: server radius kept, as at login (main.rs:523).
- **P1** (L22) Visible name of an invisible armor stand not displayed (holograms). Fix: main.rs:3538-3562: armor_stand case before the invisibility rules, retained if and only if meta_bool(3).
- **P2** (L16+L20) --quickPlaySingleplayer ignored and --quickPlayPath log never written. Fix: launch.rs: keep the argument; flow.rs: open the world through the world list path; write the JSON log.
- **P2** (L18+L23+L06) No local refusal outside the world border; outline drawn outside the border. Fix: Border test in interact.rs start_destroy/use_item_on and before outline_target (main.rs:2351). [Merge of duplicate G1-F17 "World border not checked before entity interaction": interact.rs:1227: WorldBorder test from rc-client.]
- **P2** (L23+L67) Outline and targeting without random offset or shape depending on the held item. Fix: main.rs:2307/2351 and outline.rs:252: offset hash from rc-mesh; shapes of the light block and scaffolding according to the item in hand. [Merge of duplicate G6-F21 "Light block and scaffolding: targeting shape according to the held item absent": Compute the targeting shape according to the held item (main.rs:2308).]
- **P2** (L25) Outline never displayed in adventure mode. Fix: main.rs:2358-2362: test can_break/can_place_on of the item in hand.
- **P2** (L23) Local player lying down in first person not drawn. Fix: main.rs:1586: add || self.sleeping_pos().is_some().
- **P2** (L22) Fixed tag distances instead of attributes. Fix: main.rs:3528/3611: synchronised attributes, fallback 64 and 10.

### G2-F10: Client particles: block tint, harmful gas, dust, flames, block_marker

Files: `apps/client/src/particles.rs`, `crates/rc-proto-v777/src/entity.rs`, `crates/rc-data`, `apps/client/src/block_anim.rs`, `apps/client/src/block_entities.rs`, `apps/client/src/hand.rs`.

- **P1** (L19+L09) Wrong tint of block particles (grass, redstone, biome). Fix: particles.rs:413-425/616-634: tint per source, per state and per position (power, age, biome). [Merge of duplicate G1-F05 "Block particles: constant tint per name, without biome or state": Extract the particle tint per block (grass -1, water and bubble column as water kind); at emission (particles.rs:413, 617-634, 1660), resolve with the biome colours of rc-mesh (blocks.rs:789).]
- **P2** (L19+L30+L62) noxious_gas_cloud particle absent. Fix: New kind in particles.rs; emission from the active sulfur block in block_anim.rs. [Merge of duplicate G3-F16 "noxious_gas_cloud particle absent": New type in particles.rs, emitted every 20 ticks from block_entities.rs.] [Merge of duplicate G5-F16 "Powerful sulfur: no cloud": Client rendering of the cloud.]
- **P2** (L19+L30) dust_color_transition: end colour and scale ignored. Fix: entity.rs:385-388: read to_color and scale; use them in particles.rs. [Merge of duplicate G3-F16 "dust_color_transition network: end colour and scale lost": Decode to_color and scale (entity.rs:385-388), from_net (particles.rs:334-340).]
- **P2** (L19) Small flame offset by -0.05. Fix: particles.rs:1817-1820: method that sets the size and recentres (l.945-947).
- **P2** (L19+L65) falling_dust: average texture colour. Fix: rc-data: dust colour and map colour; official cascade in particles.rs. [Merge of duplicate G5-F16 "Falling dust: average colour of the texture": particles.rs:2136-2141.]
- **P2** (L19) Sprite of item shards: wrong source. Fix: Texture particle of the item model resolved by rc-assets.
- **P2** (L19) Effect particles visible near the eye with the spyglass. Fix: Alpha 0 with the spyglass and in first person.
- **P2** (L30) block_marker not forced. Fix: Add BlockMarker to Kind::forced (particles.rs:134-162).

### G2-F02: Mobs (client): phantom, sulfur cube, reins, panda, happy ghast, shadows

Files: `apps/client/src/entity_render/animals.rs`, `apps/client/src/entity_render/creatures.rs`, `apps/client/src/world_fx.rs`, `apps/client/src/level_events.rs`.

- **P1** (L22) Phantom: scale according to size and offset absent. Fix: creatures.rs:903/911: 1+0.15×size; translation (0; 1.3125; 0.1875) after rotation.
- **P1** (L23) Sulfur cube: contained block, carried head, fuse absent. Fix: creatures.rs:121/841: body slot (block or skull) and white fuse overlay.
- **P2** (L16) Camel reins never displayed when ridden. Fix: animals.rs:319-320: hide only without a passenger.
- **P2** (L16) Equine rein lines never displayed when ridden. Fix: animals.rs:503-505: same condition.
- **P2** (L17) Frog and turtle: water test approximated by the ground. Fix: animals.rs:1100/1217: touches_water.
- **P2** (L17+L22+L23) Panda frightened by the thunderstorm: pose and trembling. Fix: animals.rs:1007/1432: worried gene and client thunderstorm.
- **P2** (L17) Happy ghast harness goggles: ridden pose never applied. Fix: creatures.rs:447: according to the passengers.
- **P2** (L22) Wrong shadow radii for 13 kinds. Fix: Correct or add the entries of the shadow tables of animals.rs and creatures.rs (without touching entity_render.rs).

### G2-F04: Dynamic item properties (recovery compass, rod, bundle, Christmas chest)

Files: `crates/rc-assets/src/item.rs`, `apps/client/src/stack.rs`, `apps/client/src/item_special.rs`, `apps/client/src/bundle.rs`, `crates/rc-client`, `apps/client/src/entity_render/blocks.rs`, `apps/client/src/front.rs`.

- **P2** (L24) Recovery compass always random. Fix: last_death kept in rc-client; EvalEnv method implemented in HudEnv; item.rs:1239.
- **P2** (L23) fishing_rod/cast always false. Fix: item.rs:1186: wearer's active bobber, main hand rule.
- **P2** (L23) Bundle: has_selected_item and selected_item. Fix: Selected index exposed in StackView; model of the selected item (item.rs:1092/1187).
- **P2** (L24) Chest item without Christmas texture. Fix: item.rs:748-752 keeps texture; item_special.rs chooses from this field, christmas textures added.
- **P2** (L15) Tooltip of the item selected in a bundle badly placed. Fix: bundle.rs:418-419: shared positioner (screens.rs:950) and stack style.

### G2-F03: Humanoid models, held items, compass and clock outside the HUD

Files: `apps/client/src/entity_render.rs`, `apps/client/src/skins.rs`.

- **P1** (L24) Compass and clock frozen in hand and on the ground. Fix: entity_render.rs:6376/6389: world environment (dimension, wearer's position, yaw, tick, sun, spawn, last_death) on the logic of HudEnv (stack.rs:535, read-only); public constructor for G2-F05; last_death after G2-F04.
- **P2** (L16) Allay in opaque cutout. Fix: entity_render.rs:2727: Pass::Translucent.
- **P2** (L16) Item held by the allay badly placed. Fix: push_held: allay-specific transformation.
- **P2** (L17) Item in hand of the vex and the copper golem: generic placement. Fix: push_held: transformations per kind.
- **P2** (L17) Piglin: crossbow and admiring poses. Fix: entity_render.rs:6028: pose according to the held item and the charge.
- **P2** (L17) Illager mounted without seated pose. Fix: entity_render.rs:6063: vehicle branch.
- **P2** (L18) Spectator seen by a spectator: only the head. Fix: entity_render.rs:3884: hide body, arms, legs and layers.
- **P2** (L22+L50) Illusioner invisible without its 4 copies. Fix: entity_render.rs:3735: 4 instances with the illusion offsets. [Merge of duplicate G4-F16 "Illusioner: copies and clouds": entity_render.rs]

### G2-F05: Item entities (carried blocks, ominous spawner, wind charge) and maps

Files: `apps/client/src/entity_render/objects.rs`, `apps/client/src/entity_render/maps.rs`, `apps/client/src/entity_render/blocks.rs`, `apps/client/src/entity_render/misc.rs`.

- **P1** (L20+L09) Blocks drawn as entities: tint ignored, special models absent. Fix: objects.rs:1051/1083: default tint per group; call the special models of blocks.rs (read-only). [Merge of duplicate G1-F05 "Blocks drawn by entities or in motion without tint (enderman, piston, minecart, display, fall)": objects.rs:973-994/1034-1053, blocks.rs:1081, misc.rs:21: colour per tint index; colour without position for entities (to extract), biome at the position for piston and fall.]
- **P1** (L24) Compass and clock frozen in frames. Fix: objects.rs:889: use the world environment created by G2-F03 (to do after G2-F03).
- **P2** (L22+L55) Ominous item spawner not drawn. Fix: Carried item drawn like an item entity, scale and rotation according to age. [Merge of duplicate G5-F16 "Ominous item spawner invisible and without particles": objects.rs:324: carried item + 1 to 3 ominous_spawning every 5 ticks.]
- **P2** (L22) Wind charge without scrolling. Fix: objects.rs:277/669: breeze scrolling pass, u = age × 0.03.
- **P2** (L25) Names of map decorations absent in hand and in frame. Fix: maps.rs:49-83: text dec.name under the marker, like screens_work.rs:592.

### G3-F02: Mining, striking and breaking blocks, block pick, incoming player packets

Files: `crates/rc-sim/src/play.rs`, `crates/rc-sim/src/input.rs`, `crates/rc-sim/src/blocks/placement.rs`, `crates/rc-sim/src/blocks/attack.rs (new)`, `crates/rc-sim/src/blocks/nature.rs`, `crates/rc-sim/src/drops.rs`, `crates/rc-sim/src/fluid_use.rs`.

- **P1** (L38) Mining world events 2019/2020 never emitted. Fix: Emit from the mining tick at the official cadence; remember the face of action 1 (play.rs:286).
- **P1** (L38+L66+L67) Block attack never called at the start of mining. Fix: block_attack(p, state) called by on_action (play.rs:281-330): dragon egg (reuse use_block.rs:467), lit redstone ore. [Merge of duplicate G6-F03 "Block attack absent at the start of mining: dragon egg, note block, redstone ore": In play.rs:281 and 309-330, call the block's attack: egg teleport, note block note, redstone ore lighting (function provided by G6-F08).]
- **P1** (L33) Block pick: item named after the block instead of the clone item, data inclusion ignored. Fix: Read the data inclusion (input.rs:86-87), clone item table, block-entity data in creative (play.rs:445-456).
- **P1** (L39) Skin parts and main arm never applied (host). Fix: Decode skin_parts and main_hand (input.rs:94-96), emit metadata 16/15 and UPDATE_HAT. Guest relay: G3-F05.
- **P1** (L66) Ice broken above a solid block or a liquid: no water. Fix: Place the water in the player's break path (drops.rs:801) and emit the game event.
- **P1** (L67) Water bucket on a double slab: it gets waterlogged. Fix: Test the double type (or the ability to receive water according to the state) before waterlogging (fluid_use.rs:130, 158-171).
- **P1** (L66) Decorated pot not cracked by mining (break part). Fix: Switch the pot to the cracked state in the player's break path.
- **P2** (L33+L39) spectator_action ignored (camera not attached). Fix: Decode in input.rs; spectator checks, border, range +3, selectable; emit Event::SetCamera.
- **P2** (L38) Mining block_destruction never emitted for the other players. Fix: Emit steps 0-9 then -1 from the mining tick; LAN relay in G3-F05.
- **P2** (L67+L68) No experience when breaking sculk blocks and the spawner. Fix: Complete block_xp (drops.rs:82): sculk 1, sensors, shrieker and catalyst 5, spawner; nothing with Silk Touch.

### G3-F03: Integrated server loop, datapacks, experiments, functions and execute

Files: `crates/rc-sim/src/server.rs`, `crates/rc-sim/src/storage.rs`, `crates/rc-sim/src/datapack.rs`, `crates/rc-sim/src/commands/exec.rs`, `crates/rc-sim/src/progress/mod.rs`, `crates/rc-sim/src/stats.rs`.

- **P2** (L39) World datapacks: only functions, function tags and advancements read. Fix: Pack repository (datapacks folder + jar) according to the level.dat selection; stacked data registries.
- **P2** (L39) Experiments ignored (flags always vanilla). Fix: Union the flags of the selected packs (storage.rs:111) and send them (server.rs:447). LAN part: G3-F05.
- **P2** (L39) Changing slot while eating does not stop the use. Fix: On a different HeldSlot, using = None (server.rs:810).
- **P2** (L34) Nested function tags not resolved. Fix: Recursive resolution of the "#" with cycle guard and required (exec.rs:71-75, 913).
- **P2** (L36) execute if|unless stopwatch and slots absent. Fix: Two branches in exec.rs:687-756.

### G3-F05: LAN guests (section fluids, relays, rights, leaving) and combat between players

Files: `crates/rc-sim/src/lan/mod.rs`, `crates/rc-sim/src/lan/conn.rs`, `crates/rc-sim/src/lan/encode.rs`, `crates/rc-sim/src/mobs/combat.rs`, `crates/rc-sim/src/mobs/track.rs`, `crates/rc-sim/src/mobs/mounts/trap.rs`.

- **P1** (L38) pvp rule never read, no player-to-player hit. Fix: Accept a LAN player target in combat.rs:1044, check pvp and friendly fire.
- **P1** (L39) Skin parts and main arm of guests not relayed. Fix: Apply what is read in lan/conn.rs:329 and broadcast the metadata (continuation of G3-F02).
- **P1** (L69) Sections sent to LAN guests with zero fluid announced. Fix: Count the states with a non-empty fluid instead of 0 (encode.rs:550); encoding test on a section with water.
- **P2** (L27) LAN guest rights: 4 instead of 2, change not sent back. Fix: guest_op = 2 (lan/mod.rs:331); level event to each guest after modification (:848).
- **P2** (L34) Game rule change not transmitted to guests. Fix: Relay events 22-28 with the guest's id and allow game events 11/12 (lan/mod.rs:126-131).
- **P2** (L38) Spectator visible to non-spectators. Fix: Filter the adding and tracking of player entities (lan/mod.rs:518).
- **P2** (L38) A guest's view not capped by the host's. Fix: Cap and recompute the tracking (lan/mod.rs:439).
- **P2** (L38) block_destruction not relayed to guests. Fix: Relay the event emitted by G3-F02 to nearby players.
- **P2** (L39) Experiment function flags not sent to guests. Fix: Use the flags computed by G3-F03 in lan/conn.rs:233.
- **P2** (L40) A guest leaving: vehicle and pearls remain. Fix: lan/mod.rs:477

### G3-F12: Scoreboard and bossbar

Files: `crates/rc-sim/src/commands/cmds_scores.rs`, `crates/rc-sim/src/commands/scores.rs`, `crates/rc-sim/src/commands/cmds_more.rs`, `crates/rc-sim/src/progress/hooks.rs`, `crates/rc-sim/src/score_criteria.rs (new)`.

- **P1** (L34) setdisplay does not remove the objective that is no longer displayed. Fix: ScoreEvent method 1 and end of tracking (cmds_scores.rs:138-171).
- **P2** (L32) numberformat styled: style lost. Fix: Style from the flat fields (scores.rs:133-139), check scoreboard.dat.
- **P2** (L38+L78) Read-only criteria never updated. Fix: Compute health/food/air/armor/xp/level in flush_scores (cmds_scores.rs:883). [Merge of duplicate G6-F16 "Criteria health, food, air, armor, xp and level never updated": Compare at each player tick with the last values; on change, write the score and broadcast it. One-line call from the player tick, to add at merge.]
- **P2** (L35) Bossbar name: style on the inner name. Fix: Brackets first, then style (cmds_more.rs:34-48).
- **P2** (L35) Bossbar players as a boolean, flags lost. Fix: Vec<u128> and three booleans in NBT, flags of BossOp::Add (cmds_more.rs:27, 96-101, 201).
- **P2** (L78) New score not locked: /trigger without enable. Fix: Manual Default of Score with locked=true (scores.rs:66-72); NBT reading unchanged.
- **P2** (L78) Death or disappearance of a non-player entity: scores and team kept. Fix: Cleanup function in scores.rs, called at the definitive removal of the entity (routing at merge), with packet broadcast.
- **P2** (L78) automatic update of the displayed name never applied. Fix: In set() and add(), fill score.display if auto_update (cmds_scores.rs:222).

### G4-F02: Monster spawning and finalisation, and variants

Files: `crates/rc-sim/src/mobs/spawn.rs`, `crates/rc-sim/src/mobs/variants.rs`, `crates/rc-sim/src/mobs/aquatic/drowned.rs`, `crates/rc-sim/src/mobs/sealife/nautilus.rs`, `crates/rc-sim/src/attrs.rs`.

- **P1** (L50) Spider group effect on Hard never applied. Fix: spawn.rs:373-381: keep the drawn effect (option), apply it with infinite duration to each spider of the group, cave spider included
- **P1** (L51) Zombie reinforcements never called. Fix: Random modifier at finalisation (variants.rs:793, spawn.rs:339), reinforcement call when the zombie is hurt
- **P1** (L48) Variants and sound variants of five species neither saved nor re-read. Fix: variants.rs load_extra/save_extra: variant and sound_variant, defaults temperate/pale/classic
- **P1** (L47) Breeding statistic absent for the nautilus (part of the common difference). Fix: sealife/nautilus.rs:893: call the common breeding routine of brains.rs:458
- **P2** (L41+L50+L51) Halloween pumpkin of skeletons and zombies. Fix: Local date (Oct 20 to Nov 3), draws 0.25 then 0.1 at the same points of the sequence, drop chance 0
- **P2** (L51) Zombie villager cure: equipment destroyed instead of dropped. Fix: variants.rs:589-592: drop the guaranteed slots before emptying
- **P2** (L47+L51) Drowned never mounted on a zombie nautilus. Fix: drowned.rs:299-304: call the jockey creation (nautilus.rs:1072) with the official conditions, structure reason included

### G4-F03: Goal and sensor AI (wolf, breeding, panic, cat, villager)

Files: `crates/rc-sim/src/mobs/goals.rs`, `crates/rc-sim/src/mobs/surface/mod.rs`, `crates/rc-sim/src/mobs/goals/surface_goals.rs`, `crates/rc-sim/src/mobs/wild/mod.rs`, `crates/rc-sim/src/mobs/brainy/mod.rs`, `crates/rc-sim/src/mobs/brainy/brain.rs`, `crates/rc-sim/src/mobs/villager/behave.rs`, `crates/rc-sim/src/mobs/villager/brain.rs`.

- **P1** (L45) The wolf defends its owner without a target type filter. Fix: goals.rs:1103-1118: wants-to-attack function in both branches before the target test
- **P1** (L48) Wild wolves and cats can breed. Fix: goals.rs:2040: tamed and not sitting required for these species
- **P1** (L47) Breeding statistic and trigger absent (fox, goat, frog, axolotl, armadillo, sniffer). Fix: wild/mod.rs:519-558, brainy/mod.rs:1151: common routine of brains.rs:458; sweep the birth paths
- **P2** (L45) Breeding partner searched too far. Fix: goals.rs:2061: threshold 64
- **P2** (L45) Panic water search different. Fix: goals.rs:2011-2018: faithful traversal of brainy/mod.rs:1225
- **P2** (L45) Universal anger end incomplete. Fix: goals.rs:1119, 1386-1389: clear the aggressor, restart the timer, propagate if alert
- **P2** (L47) The tamed cat still follows the fish. Fix: goals.rs:832: untamed cat only
- **P2** (L47) Cat: crouching and sprinting according to speed absent. Fix: surface/mod.rs:549: extend the ocelot's logic
- **P2** (L45) Turtle egg trampled without particles. Fix: surface_goals.rs:669-690: egg and cloud particles at the existing draws

### G4-F04: Projectiles: impacts

Files: `crates/rc-sim/src/projectiles/hits.rs`, `crates/rc-sim/src/mobs/end/bullet.rs`.

- **P1** (L45) Last mob hit by the player recorded only in melee. Fix: Set player_hit and its timestamp at the impact of the player's arrows, tridents and wind charges (combat.rs:1122 as a model)
- **P2** (L52) Arrow or trident bounces off the enderman. Fix: hits.rs:245-253, 533: bounce, fire and sound only if the target receives the hit
- **P2** (L49) Enderman hurt by the damage potion, without teleporting. Fix: hits.rs:388: only water hurts; call the existing random teleport
- **P2** (L52) Water potion without rehydrating axolotls. Fix: hits.rs:970-1000: air at maximum

### G4-F05: Projectiles: throwing, trajectory and portals

Files: `crates/rc-sim/src/projectiles/weapons.rs`, `crates/rc-sim/src/projectiles/physics.rs`, `crates/rc-sim/src/portal.rs`, `crates/rc-sim/src/respawn.rs`.

- **P1** (L52) Projectiles blocked by portals (pearl towards the gateway included). Fix: Hook the projectile tick into portal.rs; the pearl takes its owner along
- **P2** (L48) Wither and dragon go through portals. Fix: portal.rs:1162-1174: exclude them
- **P2** (L49) Lit TNT does not go through portals. Fix: portal.rs: handle TNT like the falling block
- **P2** (L52) Wind charge hits a crystal or another charge. Fix: physics.rs:278-295: exclude these candidates

### G4-F09: Effects and player (turtle helmet, 1.21 effects, hurt sound)

Files: `crates/rc-sim/src/effects.rs`, `crates/rc-sim/src/potions.rs`, `crates/rc-sim/src/mobs/effects.rs`, `crates/rc-sim/src/vitals.rs`, `crates/rc-sim/src/stats.rs`.

- **P1** (L51) Turtle helmet without water breathing. Fix: Player tick: head out of the water and turtle helmet, 200 ticks of water breathing without particles, with icon
- **P2** (L43) Wind Charged, Weaving, Oozing, Infested without effect. Fix: Hooks on death and on hurt; wind burst, cobwebs (3018), slimes, silverfish
- **P2** (L43) Own particles of these effects absent. Fix: effects.rs:116-119
- **P2** (L43) Player hurt sound chosen by the type's name. Fix: vitals.rs:505-509: effects field of the type

### G5-F01: Server block entities: containers, hoppers, dispenser and dropper, composter, pressure plates, copper chests

Files: `crates/rc-sim/src/containers.rs`, `crates/rc-sim/src/blocks/entities.rs`, `crates/rc-sim/src/brewing.rs`, `crates/rc-sim/src/blocks/dispense_behaviors.rs (new)`, `crates/rc-sim/src/projectiles/dispense.rs`, `crates/rc-sim/src/level.rs`, `crates/rc-sim/src/blocks/level.rs`, `crates/rc-sim/src/blocks/hopper_items.rs`.

- **P1** (L56+L62) Container state duplicated: open menu, live furnaces and brewing stands keep their copy, hoppers and the next click overwrite it (duplication or loss). Fix: A single source of truth: the menu and slots/store_slots (entities.rs:868-893) read and write the block entity's Items, or go through life.furnaces/life.brewers when the entry exists (containers.rs:194-219, 325-351, 404-412, 623; brewing.rs:86-205). sync_container_back also notifies comparators.
- **P1** (L62) Hopper: faces and slots of furnaces and brewing stands not filtered. Fix: insert_one and hopper_suck (entities.rs:932-937, 1115-1118): placement rules per slot (fuel) and removal rules per furnace face; faces and rules of the brewing stand.
- **P1** (L62) Shelves, test block and test instance without a block entity. Fix: Add these cases in block_entity_id (entities.rs:77-129), plus potent_sulfur (its logic: G5-F12).
- **P1** (L64) Rail without support: disappears without leaving an item. Fix: entities.rs:1283: replace remove_block with a destruction with loot.
- **P1** (L29+L57+L66) Default item throw absent (dispenser and dropper). Fix: "Throw the item" fallback (0.7 from the centre, vertical offset according to the axis, official velocity, events 1000/2000) for the _ branch (entities.rs:1232) and the dropper without a container (1164-1166). [Merge of duplicate G5-F01 "The dispenser does not eject ordinary items": entities.rs:1232: default branch = ejection of one copy (dropper logic) + level_event 1000 and 2000 instead of 1001.] [Merge of duplicate G6-F04 "Dispenser and dropper: default ejection absent": Default behaviour (item on the ground, events 1000 and 2000), equippables port, spawn by egg (entities.rs:1163-1166, 1232); no sound at dropper insertion; block_activate when the reservoir is empty.]
- **P1** (L29) Specific dispenser behaviours absent. Fix: Module dispense_behaviors.rs routed by item id from dispense_from, fallback = default throw.
- **P1** (L66) Composter ignored by hoppers and stuck at level 7. Fix: Special container in hopper_items.rs (input from the top, output from the bottom); tick of 20 scheduled at placement in level.rs:804.
- **P1** (L69) Block entity of copper chests and statues destroyed on state change. Fix: In set_block_limit (blocks/level.rs:288), do not remove the block entity between two blocks of the same copper family; test: a full chest that oxidises keeps its items.
- **P1** (L67+L68) Pressure plates and wire: counting limited to the player, TNT and falling blocks (counting part). Fix: Extend count_entities_in (entities.rs:798) to mobs, items, orbs, projectiles and vehicles according to sensitivity. The call from entity movement belongs to G6-F05, and the arrows to G6-F09.
- **P1** (L66) Honey block: sliding effects of living entities (server part). Fix: Emit the sliding effects for living entities (entities.rs:538). The fall damage reduction belongs to G6-F05.
- **P1** (L66) Jukebox: container size (routing). Fix: Adapt entities.rs:838 to the disc storage provided by G6-F01.
- **P2** (L57) Dispenser: does not place Shulker boxes. Fix: *shulker_box branch: placement oriented according to the face (down if face up), components copied into the block entity, otherwise ejection.
- **P2** (L56) Ender chest menu never closed (distance, broken block). Fix: containers.rs:207-210: keep m.block = Some(pos) with m.ender; menu_valid (301-306) checks the ender_chest block and range.
- **P2** (L62+L43) Container lock (lock) never checked. Fix: use_block (containers.rs:160): evaluate the lock predicate on the held item, message and sound on failure. [Merge of duplicate G4-F14 "Container lock not applied": Opening path (containers.rs): predicate against the main hand, message, sound]
- **P2** (L29) Dispensed flint and steel: only fire in the air. Fix: Lit states, TNT priming, wear, sound 1001 on failure (entities.rs:1220-1227).
- **P2** (L29) Wind charge not fired, trident fired instead of thrown. Fix: Add wind_charge, remove trident in dispense.rs:13-30.
- **P2** (L66+L49) Concrete powder falling next to water: does not harden. Fix: Call touches_liquid on landing (entities.rs:661-669, 367). [Merge of duplicate G4-F08 "Falling concrete powder: water not detected": blocks/entities.rs:367-368, 666: radius and liquid test]
- **P2** (L66) Frosted ice placed outside enchantment: never melts. Fix: Schedule the tick at placement and react to neighbour change in level.rs.
- **P2** (L68) Wet sponge that does not dry in the Nether. Fix: Add the case at placement in level.rs:807.
- **P2** (L67) Dirt path that does not turn back into dirt under a solid block. Fix: Reaction to the neighbour above and scheduled tick, reusing the farmland conversion.

### G5-F03: Mobs: player interactions, synced data, generic state (cat, wolf, sheep, name tag, egg)

Files: `crates/rc-sim/src/mobs/brains.rs`, `crates/rc-sim/src/mobs/mob.rs`, `crates/rc-sim/src/mobs/variants.rs`, `crates/rc-sim/src/mobs/sealife/fish.rs`, `crates/rc-sim/src/mobs/mounts/mod.rs`, `crates/rc-sim/src/mobs/sealife/dolphin.rs`, `crates/rc-sim/src/mobs/mounts/feed.rs`, `crates/rc-sim/src/mobs/piglins/zombified.rs`, `crates/rc-sim/src/mobs/golems/iron.rs`, `crates/rc-sim/src/mobs/mounts/riding.rs`, `crates/rc-sim/src/vitals.rs`, `crates/rc-sim/src/lootchest.rs`.

- **P1** (L55+L60) Name tag without effect on a mob. Fix: interact_mob (brains.rs:776): name_tag with custom_name → m.custom_name, persistence, consumption outside creative.
- **P1** (L55+L60) Spawn egg on a mob of the same species: no baby. Fix: interact_mob: *_spawn_egg on the same Kind → baby (age -24000) at its position, consumed outside creative (brains.rs:831).
- **P1** (L59) Sheep dyeing absent. Fix: interact_mob: dye on Kind::Sheep → m.wool, sound, consumption.
- **P1** (L47+L48) Synced data indices of the cat and the wolf swapped. Fix: mob.rs:896-913 and 1027-1043: official order, collar as an integer, sound variant last; index test
- **P1** (L47+L48) Collar dyeing of the cat and the wolf absent. Fix: brains.rs:898-941: dye branch before the sit/stand toggle, collar synchronisation
- **P1** (L48) Wolf armour absent. Fix: Body slot, pose, removal, repair, cracks, absorption in the wolf's damage
- **P2** (L55+L33) Off-hand interaction refused on mobs. Fix: brains.rs:759: handle hand == 1 with the off-hand item. [Merge of duplicate G3-F14 "Off-hand interact refused": Accept hand == 1, slot 45, sweep the reads of held (brains.rs:758-760).]
- **P2** (L54) Mob flags Silent, CustomNameVisible, Glowing, NoGravity ignored. Fix: mob.rs:929-956, 1072-1189: fields + NBT; metadata: G5-F05; silence: G5-F04; gravity: G5-F02.
- **P2** (L54) Feeding a baby: forced age and particles absent. Fix: mob.rs:1260: read/write forced_age and its timer; brains.rs:957-961, dolphin.rs:204-207, mounts/mod.rs:378: fill on feeding; particles in the tick.
- **P2** (L54) Mob conversion: attributes not copied. Fix: converted() (variants.rs:467-520): fire, absorption, tags, custom_data, mounts, new flags.
- **P2** (L48+L46+L47) Inheritance at birth: wool, collar, sound variant. Fix: brains.rs:440-450: wool mixed by the recipe, collar of a random parent, default sound variant (classic for the cat)
- **P2** (L47) Cat eating sound absent. Fix: brains.rs:904-933: eating sound of the sound variant
- **P2** (L48) Wet wolf shaking off absent (server). Fix: Wet state and shaking in the wolf's tick, events 8/56 emitted
- **P2** (L49) Creeper: priming on fall, effect cloud, lighting with flint and steel. Fix: brains.rs:776 interaction with the tag; priming on fall; lingering effect cloud at explosion
- **P2** (L49) Enderman teleport: arrival box not checked. Fix: brains.rs:505: tag on the whole box, descent onto solid blocks, world border
- **P2** (L31) Enderman: carriedBlockState in the old format. Fix: Keys id and properties (brains.rs:1095-1099).
- **P2** (L38+L55) forgive_dead_players not applied. Fix: On death, clear target and anger of nearby neutrals; remove zombified.rs:114-116 and iron.rs:87. [Merge of duplicate G5-F02 "forgive_dead_players ignored by neutral mobs": die() (vitals.rs:718): clear the anger of neutral mobs targeting the player if the rule is active.]
- **P2** (L38) Synchronisable mob attributes never sent. Fix: update_attributes at pairing and on changes, scale attribute (riding.rs:662, mob.rs:141).

### G5-F02: Entity contact, fall and environment (blocks passed through, fire, rain, magma, berries, honey, armour)

Files: `crates/rc-sim/src/vitals.rs`, `crates/rc-sim/src/mobs/tick.rs`, `crates/rc-sim/src/mobs/body.rs`, `crates/rc-sim/src/mobs/recent/wind.rs`, `crates/rc-sim/src/inside_blocks.rs (new)`, `crates/rc-physics/src/lib.rs`, `crates/rc-sim/src/mobs/illagers/ravager.rs`.

- **P1** (L64+L65) Common pass for blocks passed through absent: fire does not burn mobs, wrong damage to the player, cactus without damage, campfire without damage. Fix: New inside_blocks.rs: walk the blocks touched by the box (player vitals.rs:920-985, mobs tick.rs:187) and apply fire, cactus, campfire, lava with the damage sources of the registry. Items on the ground: call from drops.rs (G5-F05).
- **P1** (L54) Rain does not extinguish burning entities. Fix: tick.rs:187-204, body.rs:73-78, vitals.rs:930-938: also extinguish under rain at the feet or head block (bs_precipitation_at).
- **P1** (L54) Mobs without their own step sound: no footstep noise. Fix: body.rs:414-438: failing a step_sound, the step sound of the block's sound group at 0.15 × volume, plus the amethyst chime.
- **P1** (L55) Wind charge jump: fall not bounded. Fix: wind.rs:216-221: remember the impact point and grace delay; vitals.rs:1127-1148: bound the fall distance.
- **P1** (L58) Thorns absent (player side + common function). Fix: Common function in inside_blocks.rs or a dedicated module: draw < 0.15 × level, 1 + an integer drawn between 0 and 3 thorns damage to the direct attacker, wear 2. Call after the player's damage (vitals.rs); mob side: G5-F04.
- **P1** (L58) Player armour wear: damage_on_hurt and fire resistance ignored. Fix: vitals.rs:455-460: skip the piece if equippable.damage_on_hurt=false or damage is_fire on a damage_resistant item.
- **P1** (L66) Farmland trampling absent. Fix: Hook the fall of the player and mobs (body.rs:373) to farmland, with draw and size test; the upward push and the game event in turn_to_dirt belong to G6-F08.
- **P1** (L66) Fall onto honey without damage reduction. Fix: Add honey_block (×0.2) in vitals.rs:1132 and in the mobs' fall.
- **P1** (L67) Fall onto a stalagmite: doubled damage absent or wrong. Fix: Common per-block fall rule in vitals.rs:1132 and body.rs:385: stalagmite only in the right state, powder snow without damage.
- **P1** (L67) Magma block without damage when walked on. Fix: hot_floor damage in vitals.rs:916 and body.rs, except when sneaking or fire-immune; also call the lighting of redstone ore on step (function of G6-F08).
- **P1** (L68) Berry bush without damage for whoever passes through it. Fix: Damage conditioned on movement, for the player (vitals.rs:971) and for mobs.
- **P1** (L67+L68) Pressure plates and wire: contact called by the player only (movement part). Fix: Call the plate and wire contact from the movement of all entities (counting belongs to G6-F04). Also call the turtle egg trampling (function of G6-F08).
- **P2** (L54) Sound entity.generic.extinguish_fire absent when an entity is extinguished. Fix: Play the sound when the fire goes from >0 to 0 (tick.rs, body.rs, vitals.rs).
- **P2** (L55) Fall: fall_damage_multiplier, block sound and dust absent. Fix: vitals.rs:1127-1155 and body.rs:398-411: attribute multiplier; block sound (×0.5 volume, ×0.75 pitch); block particles 150 × min(0.2 + p/15; 2.5), velocity 0.15.
- **P2** (L58) Player protections applied to bypasses_invulnerability damage. Fix: protection() (vitals.rs:367-387) returns 0 for this tag; mob side: G5-F04.
- **P2** (L58) Frost Walker and Soul Speed absent. Fix: "Block change" hook at the player's and mobs' tick: frosted ice in a disc, no hot_floor; soul_speed speed modifier, boot wear, soul particle and sound.
- **P2** (L56) Sound at the end of consumption absent (only the burp is played). Fix: finish_using (vitals.rs:1412-1418): sound of the consumable component (default entity.generic.eat), neutral, volume 1, pitch triangle(1; 0.4), before the burp.
- **P2** (L54) Mobs' NoGravity ignored in physics. Fix: body.rs: no gravity if m.no_gravity (field added by G5-F03).
- **P2** (L68) Wither rose without Wither effect. Fix: Add the case to block contact.
- **P2** (L66) Cauldrons without effect on the entity inside. Fix: Handle lava_cauldron, water_cauldron and powder_snow_cauldron in the contact effects (vitals.rs:927-930 and mobs).
- **P2** (L67) Powder snow that does not extinguish a burning entity. Fix: Add the case to contact for the player (vitals.rs:930) and for mobs.
- **P2** (L66) The ravager does not trample crops. Fix: Add crops to the ravager's contact effects.

### G5-F04: Combat and offensive enchantments (mace, mob thorns, credits)

Files: `crates/rc-sim/src/mobs/combat.rs`, `crates/rc-sim/src/projectiles/ench.rs`, `crates/rc-sim/src/enchanting.rs`.

- **P1** (L58+L60) Mace: no smash attack (Density, Breach, Wind Burst absent). Fix: player_attack_mob: fall > 1.5 outside flight → bonus according to fall_distance, sound, area knockback, event 2013, fall_distance = 0; smash_damage_per_fallen_block, armor_effectiveness, post_attack wind_burst (reuse wind.rs:142).
- **P1** (L55) Deaths caused by a tamed animal not credited to the owner. Fix: combat.rs:344: attacker with player owner → last_hurt_by_player = 100.
- **P1** (L58) Mob thorns and armour wear (damage_on_hurt, fire resistance). Fix: mob_hurt: call the thorns function of G5-F02; wear of mobs' pieces with the same exclusions.
- **P2** (L58) Mob protections applied to bypasses_invulnerability damage. Fix: mob_protection (combat.rs:422, 461) returns 0 for this tag.
- **P2** (L55) Armour toughness ignored for mobs. Fix: combat.rs:449: replace t = 2.0 with the toughness coming from the equipment modifiers and the species' base.
- **P2** (L55) No message to the owner when their animal dies. Fix: mob_die (combat.rs:501): death.* message to the owner if the rule is active.
- **P2** (L54) Silent flag not respected by mob sounds. Fix: mob_sound (combat.rs:45): nothing if m.silent (field added by G5-F03).

### G5-F05: Orbs, dropped items and entities without AI (displays, interaction, marker)

Files: `crates/rc-sim/src/drops.rs`, `crates/rc-sim/src/commands/cmds_ents.rs`, `crates/rc-sim/src/server.rs`, `crates/rc-sim/src/mobs/mod.rs`, `crates/rc-sim/src/display.rs (new)`.

- **P1** (L55+L58) Mending never applied when picking up orbs. Fix: drops.rs:610-623: damaged equipped item at random with repair_with_xp, repair min(value × 2, damage), remainder to the player, loop on the remainder.
- **P1** (L65+L49) Cactus and fire: damage to dropped items. Fix: Call the blocks-passed-through pass of G5-F02 for dropped items. [Merge of duplicate G4-F08 "Dropped items never burned or destroyed by cactus": drops.rs:1073: item damage on contact with lava, fire, cactus (fire resistance, sound), same for orbs]
- **P1** (L02+L54) Transformation and own fields of displays never sent to the client. Fix: In send_entity_meta (drops.rs:711), wire the *_display types to display.rs: read the transformation (decomposed form or 4x4 matrix row by row, decomposed by a faithful SVD: Jacobi 5 steps then 3 Givens QRs, thresholds 1e-6), emit indices 11-14 (Vector3, Quaternion, Vector3, Quaternion) and all own fields (interpolation, billboard, brightness, shadow, block, item, text). The client already knows how to read them (entity_render/objects.rs:1440). [Merge of duplicate G5-F05 "Display entities (block, item, text): data never sent": Keep the Display fields in the non-simulated entity (server.rs:738, cmds_ents.rs:197-229) and send them at the official indices in send_entity_meta (drops.rs:711-797), also after /data.]
- **P2** (L55) No periodic merging of orbs. Fix: orbs_tick (drops.rs:557): sweep every 20 ticks.
- **P2** (L54) Metadata of the mobs' generic flags. Fix: send_entity_meta: Silent, CustomNameVisible, Glowing, NoGravity (fields of G5-F03).
- **P2** (L55) minecraft:interaction entity inert. Fix: mobs/mod.rs:312-325: write attack/interaction and trigger the criterion; execute on attacker/target.
- **P2** (L60) Shears: wear ignored on zero-hardness blocks. Fix: drops.rs:812: for shears, ignore the hardness condition, exclude #fire.

### G5-F06: Minecarts in the integrated server

Files: `crates/rc-sim/src/minecart.rs (new)`, `crates/rc-sim/src/commands/cmds_s9c.rs`.

- **P1** (L53+L60+L52) No minecart entity: no placement, no movement on rails, no mounting, no variants. Fix: minecart.rs: physics on rails (non-experimental behaviour), mounting, chest, hopper, furnace, TNT, spawner variants, NBT; tick called from the command_minecarts_tick path (cmds_s9c.rs:46, already called by server.rs:872); rail placement function exposed, called by G5-F10. [Merge of duplicate G4-F11 "Minecarts absent from the integrated server": Record first in gaps-client.md; entity, rail placement, physics, breaking and loot, container and loot table, variants. Hook in the tick of server.rs (F14) after merge]

### G5-F08: Decorative entities on the server side: paintings, frames, armor stand, cushion, mannequin

Files: `crates/rc-sim/src/decoration/ (new)`, `crates/rc-sim/src/item_use.rs`, `crates/rc-sim/src/commands/ents.rs`.

- **P1** (L59+L49) Paintings, frames and glow frames impossible to place. Fix: Entities painting, item_frame, glow_item_frame: placement, survival, rotated item, breaking, saving; placement function called by G5-F07. [Merge of duplicate G4-F06 "Armor stand, frames, paintings without server logic": Survival and breaking of hanging entities, placement through item_use.rs, interactions, saving]
- **P1** (L59) Armor stand impossible to place. Fix: armor_stand entity: placement, equipment, breaking, NBT.
- **P1** (L49+L59) Cushion and mannequin without server behaviour. Fix: Add them to the decoration module [Merge of duplicate G5-F08 "Cushion impossible to place": Cushion entity, same wiring.]

### G5-F09: Block rules: growth, neighbourhood, projectiles

Files: `crates/rc-sim/src/blocks/level.rs`, `crates/rc-sim/src/blocks/shape.rs`, `crates/rc-sim/src/blocks/table.rs`, `crates/rc-sim/src/blocks/nature.rs`, `crates/rc-sim/src/blocks/growth.rs`, `crates/rc-sim/src/blocks/attach.rs`, `crates/rc-sim/src/projectiles/hits.rs`, `crates/rc-sim/src/mobs/wild/hive.rs`.

- **P1** (L64) Big dripleaf and its stem without logic. Fix: Kind in table.rs; scheduled ticks, neighbour reaction (level.rs), projectiles (hits.rs), shape (shape.rs), bone meal (growth.rs); "entity on top > 0.6875" function called by G5-F02.
- **P1** (L63) Note block: no sound or particle. Fix: level.rs:1005: instrument sound and pitch, head above, note particle (note/24); client: G5-F16.
- **P1** (L64) Bell: without survival, redstone response or correct hit. Fix: Survival and toggle (shape.rs), neighbour reaction (level.rs:924), correct hit test, block_change and stat (hits.rs:1181-1228).
- **P1** (L65) Chorus: no growth, connection or collapse. Fix: Growth (nature.rs random tick), connections (shape.rs, function exposed to G5-F07), survival with scheduled tick (attach.rs).
- **P1** (L65) Budding amethyst grows nothing. Fix: nature_random_tick (nature.rs:539): budding_amethyst (facing, waterlogged).
- **P2** (L64) Head powered after placement: never animated. Fix: neighbour reaction (level.rs:924): skull case.
- **P2** (L64) Projectile: does not make the amethyst chime. Fix: hits.rs:1159.
- **P2** (L64) Beehive: no reaction to explosion or neighbouring fire, stat. Fix: hive.rs:394-451; fire in shape.rs.
- **P2** (L64) upward push of entities absent (farmland → dirt). Fix: nature.rs:482: common function called by the conversions.
- **P2** (L65) Plain candle_cake missed by the suffix test (survival). Fix: attach.rs:98: n == "candle_cake" || ends_with("_candle_cake"); redstone: G5-F13; projectile: here in hits.rs:1208.
- **P2** (L65) Oxidation: oxidised neighbours not counted. Fix: copper_stage (nature.rs:138): recognise copper by family, not by random_ticks.
- **P2** (L65) Signal_fire not recomputed when the block below changes. Fix: shape.rs: signal_fire update.

### G5-F10: Item uses: compass, eye of ender, horn, brush, maps, knowledge book

Files: `crates/rc-sim/src/item_use.rs`, `crates/rc-sim/src/maps.rs`, `crates/rc-sim/src/projectiles/weapons.rs`, `crates/rc-sim/src/projectiles/chorus.rs`, `crates/rc-sim/src/projectiles/mod.rs`, `crates/rc-sim/src/mobs/wild/fox.rs`, `crates/rc-sim/src/portal.rs`, `crates/rc-sim/src/brush.rs (new)`, `crates/rc-sim/src/eye_of_ender.rs (new)`, `crates/rc-sim/src/projectiles/physics.rs`.

- **P1** (L57+L59) Lodestone compass absent. Fix: use on a block on a lodestone: sound lodestone_compass.lock, lodestone_tracker component, separate stack if > 1; inventory tick: remove the target if the POI is missing.
- **P1** (L59+L62) Brush without effect on suspicious sand and gravel. Fix: brush.rs: continuous use, dusted progress 0-3, LootTable/LootTableSeed loot, return to sand or gravel.
- **P1** (L59+L52) Thrown eye of ender absent. Fix: eye_of_ender.rs: entity, trajectory, breaking or dropping, structure search, trig_used_ender_eye. [Merge of duplicate G4-F05 "Eye of ender never thrown in singleplayer": weapons.rs: tag structure, entity, sound, trig_used_ender_eye, consumption; tick in physics.rs]
- **P1** (L59) Goat horn unusable. Fix: use_item: instrument sound, cooldown, vibration (client: G5-F15).
- **P1** (L60) Exploration maps treated as inert items. Fix: maps.rs:499, weapons.rs:173: test "map item" (map_id) instead of == filled_map.
- **P1** (L60) Placing the minecart on a rail. Fix: Call the placement of G5-F06 from item_use.rs.
- **P2** (L57) Chorus teleport: mount, particle flag, fox. Fix: chorus.rs:11-39.
- **P2** (L57) Eating fox: on_consume_effects ignored. Fix: fox.rs:246: run_consume_effects (potions.rs:136).
- **P2** (L57) item_interact_start / finish events absent. Fix: item_use.rs:141: emit unless use_effects.interact_vibrations = false.
- **P2** (L60) Knowledge book without effect. Fix: Unlock the component's recipes, consume, stat.
- **P2** (L57+L59) Debug stick without effect. Fix: Use on block and start of breaking: property remembered in debug_stick_state, next value, action bar.
- **P2** (L60) Shears on a climbing plant head: no trimming. Fix: Maximum age, sound, wear.

### G5-F11: Beacon (tick, menu, effects, construct_beacon), lectern, crafter and recipe book

Files: `crates/rc-sim/src/menus.rs`, `crates/rc-sim/src/stations_srv.rs`, `crates/rc-sim/src/beacon.rs (new)`, `crates/rc-sim/src/recipes.rs`, `crates/rc-sim/src/book.rs`, `crates/rc-sim/src/special.rs`.

- **P1** (L56+L62+L33+L39+L05) Beacon not simulated (tick, effects, menu, set_beacon, advancements). Fix: beacon.rs + Kind beacon in menus.rs, opening via station_menu (stations_srv.rs:31). [Merge of duplicate G3-F04 "SetBeacon ignored, beacon without server logic": Beacon menu (payment, levels, effects), set_beacon handling with checks, block-entity tick (pyramid, effects, beam).] [Merge of duplicate G1-F04 "construct_beacon never triggered, beacon not simulated": Simulate the beacon tick (levels, radius, effects); on activation, trig_construct_beacon (trig.rs:437, dead_code removed by F02) for the players in the box (x, y-4..y, z) inflated by 10/5/10.]
- **P1** (L56+L62) Lectern and crafter: no menu. Fix: Kinds lectern (pages, buttons 1/2/3/100+, signal) and crafter (9 slots, disabled slots); opening via station_menu.
- **P1** (L57) Crafting table repair limited to repairable items. Fix: recipes.rs:430: damageable(a) and (b) only; max_damage on the result.
- **P1** (L57) Recipe book: transmutation, dyeing, impregnation, stonecutting, smithing never sent. Fix: book.rs:146: missing branches.
- **P2** (L56) Armour or off hand through the inventory: no equip sound or event. Fix: menus.rs:374, 462-486: hook towards on_equip_item (item_use.rs:458).
- **P2** (L56) Double click active in the merchant screen. Fix: menus.rs:909-938: PICKUP_ALL exits if Kind::Merchant.
- **P2** (L57) Expansion of a max-scale map offered. Fix: special.rs:415-434: None if scale ≥ 4.

### G5-F14: Pistons: client replay, animation, pushed and stuck entities; block-entity rendering

Files: `crates/rc-sim/src/blocks/piston.rs`, `crates/rc-physics/src/collide.rs`, `crates/rc-physics/src/lib.rs`, `apps/client/src/main.rs`, `apps/client/src/block_entities.rs`, `apps/client/src/entity_render/blocks.rs`, `apps/client/src/crack.rs`, `apps/client/src/block_text.rs`.

- **P1** (L63) Pistons move no entity (push, honey, slime, collision of the moving block). Fix: Mover tick (piston.rs:379-420): entity sweep, honey, slime; collision shape of moving_piston (collide.rs:33, lib.rs:290-306), shared with the client prediction. [Pistons investigation: confirmed, the moving block's tick (piston.rs:394-417) neither pushes nor sticks any entity, no slime launch nor honey adhesion; unknown to the docs. On the client side, apply the same to the local player in the replayed moving block tick.]
- **P1** (pistons investigation+L20) Pistons: the client does not replay the block event, so neither moving block nor animation (the head appears all at once, the blocks jump into place 2 or 3 ticks later). Fix: Faithful server (crates/rc-sim/src/blocks/piston.rs:65-127, 319-338; blocks/ticks.rs:282-299): the moving blocks are placed without sending to the client and the official game counts on the client to recreate them by replaying the event. The comment at piston.rs:311 describes this replay, never written. Breaking point: apps/client/src/main.rs:821-823 forwards the event to block_entities.rs:737-753, which exits for lack of a block entity at the base and ignores any b0 != 1. Fix: (1) make the resolution of the pushed structure (piston.rs:136-290) generic over a block-reading trait and make it public (apps/client already depends on rc-sim); (2) at main.rs:821, if the block is piston or sticky_piston, replay the trigger without the signal check (b0 0 extend, 1 retract, 2 retract without pulling; b1 direction): local placement of the moving blocks, of air and of the base, and creation of the piston block entities (state, direction, extending, source, progress 0); (3) faithful client tick (block_entities.rs:1083-1086): step of 0.5, removal delay of 5 ticks after the end, the server remains master of the final block; (4) rendering (entity_render/blocks.rs:1075-1082): three cases, short head according to progress, fixed extended base for the retracting piston. Test (fails today): client world with a piston facing east at (0,64,0) and stone at (1,64,0), event b0=0 towards the east: (1,64,0) and (2,64,0) become moving blocks, (2,64,0) carries the stone as extending at progress 0, (1,64,0) the head with source, and after 2 ticks the progress is 1. [Merge of duplicate G2-F01 "Piston head animation incomplete": push_piston (blocks.rs:1075-1082): head, source and pushed block case; tint and light; data in block_entities.rs.]
- **P2** (L63) Retraction and destroyed blocks: neighbours' shape updates missing. Fix: piston.rs:94 and 350-356. [Confirmed by the redstone investigation (piston.rs:89-94: shape of the 6 neighbours of the base at the start of retraction; piston.rs:350-356: indirect shape update on destruction, wire only concerned); no effect on the bench scene.]
- **P2** (L20) No cracks on blocks drawn by entity model. Fix: Expose the geometry of blocks.rs for the mined block and pass it to crack.rs.

### G5-F15: Client: interactions, passengers, targeting

Files: `apps/client/src/interact.rs`, `apps/client/src/riding.rs`, `apps/client/src/pick.rs`, `crates/rc-physics/src/ray.rs`, `apps/client/src/tooltips.rs`.

- **P1** (L59) Client prediction: block_state and consumable fallback at placement. Fix: interact.rs:1004-1012.
- **P1** (L59) Goat horn and brush: use never started on the client side. Fix: interact.rs:1187 and use on block for the brush.
- **P2** (L53) Different minecart interaction result (gesture, Shift, occupied, TNT). Fix: interact.rs:902-911: rule per kind.
- **P2** (L53) Villager in a minecart: lowered attachment on the wrong side. Fix: riding.rs:631: test the passenger's kind when the vehicle is a minecart.
- **P2** (L61) Targeted face without interaction shape. Fix: ray.rs:142, pick.rs:196: second ray on the interaction shape.

### G5-F16: Client: block-entity rendering, particles and sounds

Files: `apps/client/src/block_entities.rs`, `apps/client/src/entity_render/objects.rs`, `apps/client/src/entity_events.rs`, `apps/client/src/particles.rs`, `apps/client/src/recipe_book.rs`, `apps/client/src/debug_screen.rs`.

- **P1** (L63) Note block: event ignored on the client side. Fix: block_entities.rs:737: handle the event (server: G5-F09).
- **P2** (L53+L61) Spawners (block and minecart): neither smoke nor flames. Fix: Common function (rotation + smoke/flame particles) for block_entities.rs:1041 and the spawner minecart (objects.rs:1390).
- **P2** (L63) Trial spawner and vault: ambient sounds absent. Fix: 2 % per tick, ominous variant.
- **P2** (L55) Event 55 (hand swap) not handled. Fix: entity_events.rs:254-470.
- **P2** (L57) Recipe book: SlotDisplay resolved approximately. Fix: recipe_book.rs:120-156: default components, demonstration dye, fuels from the table.

### G6-F01: Server-click blocks: jukebox, lectern, shelf, decorated pot, campfire, chiseled bookshelf, statue

Files: `crates/rc-sim/src/blocks/use_block.rs`, `crates/rc-sim/src/blocks/redstone.rs`, `crates/rc-sim/src/blocks/jukebox.rs (new)`, `crates/rc-sim/src/blocks/lectern.rs (new)`, `crates/rc-sim/src/mobs/golems/copper.rs`, `crates/rc-sim/src/mobs/recent/dried_ghast.rs`, `crates/rc-sim/src/blocks/campfire.rs (new)`, `crates/rc-sim/src/blocks/shelf.rs (new)`, `crates/rc-sim/src/mobs/surface/parrot.rs`.

- **P1** (L66+L60+L62) Jukebox non-functional in singleplayer. Fix: Insertion and ejection in use_block.rs, disc stored in blocks/jukebox.rs, events 1010/1011, signal 15 and comparator output in redstone.rs. The container_size line of blocks/entities.rs:838 belongs to G6-F04. [Merge of duplicate G5-F12 "Jukebox not simulated": Insertion/ejection, playback (duration from the jukebox_song registry), particles, events 1010/1011, comparator, parrot::set_record_playing_nearby.]
- **P1** (L66) Lectern non-functional in singleplayer. Fix: Placing and removing the book, lectern menu, 2-tick pulse, comparator (use_block.rs, redstone.rs, blocks/lectern.rs).
- **P1** (L66+L62) No item deposit in the decorated pot (click part). Fix: decorated_pot case in use_block.rs; the crack when mining is done by G6-F03. [Merge of duplicate G5-F12 "Decorated pot and shelves without interaction": use_block.rs:33-154: insertion into the pot; item swap of the shelves.]
- **P1** (L62+L65) Campfire: cooking, extinguishing with a shovel, interaction absent. Fix: campfire.rs: placing food (campfire recipe), cooking, ejection, shovel, stat (damage: G5-F02; signal_fire: G5-F07/F09).
- **P1** (L62+L65) Chiseled bookshelf: impossible to place or remove a book. Fix: Slot targeted from the click point, block entity, state, last_interacted_slot, sounds, event.
- **P1** (L67) Shelf without any server behaviour. Fix: Block entity with 3 slots, slot chosen according to the impact point, swaps, lateral chaining, comparator, sounds. One-line routing in use_block.rs and redstone.rs after the merge of G6-F01.
- **P2** (L66+L68) Copper golem statue: placement, axe and comparator. Fix: Click interaction (pose change) and axe in use_block.rs; comparator output in redstone.rs:1150.
- **P2** (L66) Dragon egg: teleport conditions and event 2015. Fix: Add the two conditions and event 2015 (packed position difference) in use_block.rs:467-482; the block's attack is done by G6-F03; correct line 22 of the quirks at merge.
- **P2** (L67) Note block: head impossible to place on it, statistics. Fix: In use_block.rs:97, let the placement through when the top face is targeted with an item of the tag; increment the statistics. The left click belongs to G6-F03.
- **P2** (L66) Sounds of the comparator, the copper bulb and the dried ghast absent. Fix: Emit these sounds in redstone.rs (540-720, 1130), in the copper bulb and in dried_ghast.rs.
- **P2** (L67) Light block: level cycling on click (server part). Fix: Cycle LEVEL on click in use_block.rs; the client-side targeting shape belongs to G6-F21.
- **P2** (L63) Shelves: side_chain never computed. Fix: shelf.rs: chain at placement, breaking and power change.
- **P2** (L60) Water bottle on dirt: no mud. Fix: use_block.rs:265: convertible_to_mud → mud, sounds, particles, empty bottle.

### G4-F07: Leash, leash knot and mounts (saddled pig, trader llamas)

Files: `crates/rc-sim/src/mobs/mounts/mod.rs`, `crates/rc-sim/src/mobs/mounts/goals.rs`, `crates/rc-sim/src/mobs/mounts/feed.rs`, `crates/rc-sim/src/mobs/mounts/llama.rs`, `crates/rc-sim/src/mobs/villager/trader.rs`, `crates/rc-sim/src/mobs/leash.rs (new)`, `crates/rc-sim/src/commands/exec.rs`, `crates/rc-sim/src/mobs/mounts/riding.rs`.

- **P1** (L46+L51) Wandering trader's llamas never on a leash. Fix: Read llama.rs and recent/happy_ghast.rs first (L51 says the leash is present, L46/L49 absent); leash between mobs, attachment at spawn (trader.rs:471-483), rules and defence of the trader. Hooking the leash item in item_use.rs (F06) and interact_mob (F01) after merge
- **P1** (L54+L55+L60+L49+L66) Leash not simulated (attach, fence knot, pull, break, transfer, shears). Fix: leash.rs: state per mob, interaction, fence knot, spring (0.8/0.2/0.8, stiffness 0.11), break at 10 blocks, leash NBT, set_entity_link packet; remove the message exec.rs:568. [Merge of duplicate G4-F07 "Leash knot without server logic": leash.rs: knot on fence, survival and breaking] [Merge of duplicate G6-F22 "Leash knot on a fence absent (leash not simulated in singleplayer)": leash_knot entity and link; routing of the click on a fence in use_block.rs after the merge of G6-F01.]
- **P1** (L59+L48) Pig: neither saddle nor carrot-on-a-stick steering. Fix: mounts/mod.rs:136 and riding.rs:194: add Pig on the strider's model (tick_boost, stick wear). [Merge of duplicate G4-F07 "Pig: saddle, riding and carrot on a stick absent": mounts/mod.rs:136: saddle, control by the held item, acceleration, wear (spec server-mobs.md:1484)]

### G4-F10: Allay and parrot dance near a jukebox (server and client)

Files: `crates/rc-sim/src/mobs/brainy/allay.rs`, `crates/rc-sim/src/blocks/jukebox.rs (new)`, `crates/rc-sim/src/mobs/recent/vibration.rs`, `crates/rc-sim/src/mobs/surface/parrot.rs`, `apps/client/src/entity_render/animals.rs`, `apps/client/src/world_fx.rs`, `apps/client/src/level_events.rs`.

- **P1** (L46) The allay never dances, duplication impossible. Fix: Emit start and end of playback from the jukebox; dance on reception, stop beyond 10 blocks; check the link with duplication
- **P2** (L48+L17+L25) Parrot dance (server side). Fix: parrot.rs:137: call set_record_playing_nearby from the same signal [Merge of duplicate G2-F02 "Parrots that do not dance near a jukebox": Client event 1010 (world_fx.rs or level_events.rs): mark and unmark; party pose in animals.rs; server call of set_record_playing_nearby from the jukebox.]
- **P2** (L48) Parrot dance (client side). Fix: world_fx.rs:613 marks nearby parrots, dance pose in animals.rs:1324

### G6-F06: Block placement, survival and shapes (rules per family)

Files: `crates/rc-sim/src/blocks/placement.rs`, `crates/rc-sim/src/blocks/shape.rs`, `crates/rc-sim/src/blocks/table.rs`, `crates/rc-sim/src/loot_be.rs`.

- **P1** (L66) Copper chests of different variants not joined, separated on oxidation. Fix: The whole family is connectable (placement.rs:541, shape.rs:533); copy the neighbour's state during the shape update.
- **P1** (L66+L67) Kelp and seagrass placeable on dry land. Fix: Refuse placement without source water (placement.rs:340).
- **P1** (L67) No stacking of sea pickles, pink petals, wildflowers or leaf litter. Fix: In replaceable_by (placement.rs:103-135) and at placement: same block in hand and count < 4, increment pickles, flower_amount or segment_amount. Also add the stacking of turtle eggs (logic of G6-F08).
- **P1** (L67) Mangrove propagule: wrong survival rule. Fix: In shape.rs:321: hanging, tag of supports above for hanging; otherwise ground tag below. Place with AGE=4.
- **P1** (L67) Small dripleaf: only one half, without survival. Fix: Classify small_dripleaf as a double plant with its survival rule; bone meal belongs to G6-F07.
- **P1** (L67) Pitcher plant crop inert (Kind and survival). Fix: Dedicated Kind in table.rs and survival of both halves; growth belongs to G6-F08 and bone meal to G6-F07.
- **P1** (L59+L65+L68) Hanging signs: free rotation, never attached, wall variant never placed. Fix: placement.rs:402, 648: dedicated rule (ceiling or wall according to the face, attached, rotation) on the standing_or_wall model. [Merge of duplicate G6-F06 "Wall placement of the hanging sign impossible": Wall Kind in table.rs:355-358, placement and survival on the axis (survival in shape.rs), fix the name in placement.rs:704.]
- **P1** (L60) Coral fans never placed on walls. Fix: placement.rs:743-757: *_coral_fan → *_coral_wall_fan according to the clicked face.
- **P1** (L60) Scaffolding on scaffolding: no extension. Fix: Position search: above going up, otherwise the horizontal direction of the gaze, 7 max.
- **P1** (L60) Powder snow bucket places nothing. Fix: item_block (placement.rs:26): powder_snow_bucket → powder_snow, sound empty_powder_snow, empty bucket outside creative.
- **P1** (L59) block_state component ignored at placement. Fix: placement.rs:285: apply the stack's block_state to the computed state (client prediction: G5-F15).
- **P1** (L59) Consumable fallback absent (edible seeds not eaten). Fix: After a place_item failure (placement.rs:194), if the item is consumable, use path (item_use.rs:334).
- **P1** (L64) Bell: wrong placement. Fix: placement.rs:647: dedicated placement Kind::Bell (floor, ceiling, single or double wall); survival and redstone: G5-F09.
- **P1** (L59) Wiring of placed entities (painting, frames, armor stand, cushion) and the minecart. Fix: Call the entity placement of G5-F08 before place_item (placement.rs:194).
- **P2** (L66) Pale hanging moss without server logic. Fix: Survival and shape update (in shape.rs).
- **P2** (L66) Faces of giant mushroom blocks not updated. Fix: Placement and update of the 6 faces for brown_mushroom_block, red_mushroom_block and mushroom_stem.
- **P2** (L67) Pale moss carpet without survival or sides. Fix: Dedicated Kind (table.rs:349), taking over rc-worldgen feature_veg.rs:512-547.
- **P2** (L67) Shelf fungus floating and badly oriented. Fix: Dedicated placement and survival (placement.rs:680), bounce sound.
- **P2** (L66) Flammability table incomplete. Fix: Complete fire_odds (table.rs:371) from the extracted table.
- **P2** (L65) Click under a hanging sign with a hanging sign: opens the editor. Fix: placement.rs:166: let it through for this case.
- **P2** (L59) Adventure: every use on a block refused, can_place_on ignored. Fix: placement.rs:171: test can_place_on on the clicked block.
- **P2** (L59) "Used" stat not counted for several uses on a block. Fix: Count at the successful return of the use-on-block chain in placement.rs.
- **P2** (L64+L65) Broken beehive: block name not carried over; campfire: signal_fire at placement; chorus: connections at placement. Fix: placement.rs:886: name of the broken block; signal_fire according to the block below; chorus connection state (function of G5-F09).

### G6-F23: Tripwire, hook and vine

Files: `crates/rc-sim/src/blocks/attach.rs`.

- **P1** (L68) Vine propagation: three neighbour tests shifted by one cell. Fix: Pass the right cell or test q directly (attach.rs:629, 631, 633, 653).
- **P1** (L68) Tripwire hook without sounds or game events. Fix: Fill hook_emit (attach.rs:379) with the sound and the game event, in the order receiver then source.
- **P1** (L68) Disarming the wire with shears absent. Fix: Test the tool before breaking and disarm (attach.rs:428-443).

### G6-F07: Bone meal

Files: `crates/rc-sim/src/blocks/growth.rs`, `crates/rc-worldgen/src/feature_nether.rs`, `crates/rc-sim/src/blocks/table.rs`.

- **P1** (L66+L67+L68+L59+L64+L65) Bone meal without effect on many plants (Nether vines, tall dry grass, nylium, giant mushrooms, small dripleaf, pitcher plant, etc.). Fix: Add these cases in grow_with_bone_meal (growth.rs:125-235), reusing the decoration elements of rc-worldgen; the full list of targets is in L66, L67 and L68-notes. [Merge of duplicate G5-F09 "Bone meal without effect: bamboo, vines (caves, weeping, twisting), aquatic plants": grow_with_bone_meal (growth.rs:113-235): bamboo(_sapling), cave_vines(_plant) berries, weeping/twisting vines, aquatic plant growth after crop growth failure.]
- **P1** (L72) Nether fungi: planted variant absent. Fix: Implement the planted branch of huge_fungus (feature_nether.rs:287-290); in growth.rs, crimson_fungus and warped_fungus: nylium below, 40 % chance, then place_world_feature *_fungus_planted.
- **P2** (L65) bush without bone meal; red_shrub without survival. Fix: growth.rs:126, table.rs:287.

### G6-F08: Nature ticks and turtle eggs

Files: `crates/rc-sim/src/blocks/nature.rs`, `crates/rc-sim/src/blocks/turtle_egg.rs (new)`, `crates/rc-sim/src/blocks/attach.rs`.

- **P1** (L68+L78) Turtle eggs never hatch (no block logic). Fix: turtle_egg.rs: random tick (sand below, chance according to the time, hatch +1, at 2 removal of the block and N babies with home_pos, on the model of brainy/eggs.rs), trampling, fall, breaking. Hook onto nature_random_tick; functions exposed for G6-F05 (trampling) and G6-F06 (stacking).
- **P1** (L66) Eyeblossoms neither open nor close. Fix: Random tick and scheduled tick according to the environment attribute, propagation within ±3/±2/±3 with a delay of distance×5 to ×10.
- **P1** (L67+L64) No fluid transfer by speleothems. Fix: Pointed speleothem case in nature_random_tick, event 1504. [Merge of duplicate G5-F09 "Stalactite above a cauldron: never fills": Tick in nature.rs or attach.rs: search for the stalactite, filling, events 1046/1047.]
- **P1** (L67) Pitcher plant crop: growth absent. Fix: Growth at the random tick (Kind created by G6-F06).
- **P1** (L66) Farmland: entity push and game event on conversion. Fix: Add in turn_to_dirt (nature.rs:482) the upward push of entities and the game event.
- **P2** (L67) Mushrooms: no spread at the random tick. Fix: Kind::Mushroom case in nature_random_tick (nature.rs:539).
- **P2** (L67) Redstone ore: no lighting on contact. Fix: Lighting function next to the extinguishing (nature.rs:674), called by G6-F03 (hit), G6-F05 (step) and G6-F01 (click).
- **P2** (L68) Grass and mycelium survive under a low slab or thick snow. Fix: Per-face attenuation in can_be_grass (nature.rs:747).
- **P2** (L68) Copper door and chest oxidise too fast. Fix: Filter half=lower and type!=right, exclude the open chest (nature.rs:177).

### G6-F09: Sulfur and projectiles

Files: `crates/rc-sim/src/blocks/speleothem.rs (new)`, `crates/rc-sim/src/blocks/potent_sulfur.rs (new)`, `crates/rc-sim/src/projectiles/hits.rs`, `crates/rc-sim/src/projectiles/fishing.rs`.

- **P1** (L68) Sulfur spike: no placement, survival, fall or growth logic. Fix: Logic shared with the pointed speleothem in speleothem.rs; breaking by trident (hits.rs:1195); routing in placement.rs and shape.rs after the merge of G6-F06.
- **P1** (L67+L62) Active sulfur without server behaviour. Fix: State at placement and on neighbour change, tick block entity (nausea, projection) in potent_sulfur.rs; routing after G6-F06. [Merge of duplicate G5-F12 "Potent sulfur: no server tick": Server tick (id: G5-F01; client cloud: G5-F16).]
- **P1** (L67) Pressure plates: arrows do not press (projectile part). Fix: Fill arrow_presses (hits.rs:1225).
- **P2** (L70) projectiles_can_break_blocks rule and impact_projectiles tag ignored. Fix: Test the rule and the tag before breaking (hits.rs:1184-1188, fishing.rs:326).

### G6-F10: Vibrations and redstone: sculk sensors, allay, particle, wooden buttons, blocked chest

Files: `crates/rc-sim/src/mobs/recent/vibration.rs`, `crates/rc-sim/src/mobs/brainy/allay.rs`, `apps/client/src/world_fx.rs`, `crates/rc-sim/src/blocks/redstone.rs`, `crates/rc-sim/src/mobs/recent/shrieker.rs`, `crates/rc-sim/src/containers.rs`, `crates/rc-sim/src/blocks/sculk_sensor.rs (new)`.

- **P1** (L70+L62+L65) Sculk sensors that receive no vibration at all (known, server-mobs.md:2882). Fix: Listener per sensor block entity (radius 8, or 16 with frequency filter for the calibrated one); on reception, active state with power and frequency, sculk_sensor_tendrils_clicking event. redstone.rs:171 already reads the power. [Merge of duplicate G5-F13 "Sculk sensors (normal and calibrated) deaf to vibrations": vibration.rs:375-383, shrieker.rs:72: block listener (tag sculk_sensor_can_listen, radii 8 and 16), phases, output, calibrated input; redstone.rs:171.]
- **P2** (L70+L46) The allay hears the note block without going through vibrations. Fix: Pass the note through vib_tick with occlusion by wool (allay.rs:453). [Merge of duplicate G4-F10 "Note block note perceived without vibrations": Go through vibration.rs]
- **P2** (L70) Vibration particle never sent (client part known). Fix: Emit the particle (destination, duration) at selection and at reload (vibration.rs:558-560); add the type in world_fx.
- **P2** (L65) Arrows do not press wooden buttons. Fix: redstone.rs:871: arrows in the box, reschedule the tick.
- **P2** (L65) Blocked chest: wrong criterion, double and cat not tested. Fix: blocks/containers.rs:178: conductor, neighbouring half, sitting cat; redstone.rs:1185: 0 if blocked.
- **P2** (L65) Plain candle_cake missed by the comparator. Fix: redstone.rs:1171.

### G6-F11: Sleeping and beds (straw bed, waking, announcements), waxed signs, multiplayer spawns, dragon fight

Files: `crates/rc-sim/src/respawn.rs`, `crates/rc-sim/src/weather.rs`, `crates/rc-sim/src/progress/hooks.rs`, `crates/rc-sim/src/mobs/phantom.rs`, `crates/rc-sim/src/mobs/illagers/patrol.rs`, `crates/rc-sim/src/mobs/end/fight.rs`, `crates/rc-sim/src/mobs/wither/boss.rs`, `crates/rc-sim/src/portal.rs`, `crates/rc-sim/src/mobs/brainy/eggs.rs`, `crates/rc-sim/src/mobs/villager/mod.rs`, `crates/rc-sim/src/solo_packets.rs`.

- **P1** (L64+L68) Going to bed and waking simplified: straw bed, waking position, gaze. Fix: weather.rs:169, 312: bed rule from the dimension attributes, straw_bed, getting-up position, animation 0; villager/mod.rs:1279. [Merge of duplicate G6-F11 "Straw bed treated as a normal bed": Read the bed type rule (respawn point, destruction on leaving or on use) in respawn.rs:320 and weather.rs:228/312; own statistic in progress/hooks.rs:247.]
- **P2** (L70) Phantoms: only the host is a candidate. Fix: Walk all players (time since rest, game mode) (phantom.rs:282).
- **P2** (L70) Patrols: player draw frozen to an integer draw over 1. Fix: an integer draw over the number of players, with the position and mode of the drawn player (patrol.rs:33).
- **P2** (L40+L38) Sleep announcements absent. Fix: weather.rs:341-366 [Merge of duplicate G3-F13 "Multiplayer sleep messages absent": sleep.* action bar at each state change.]
- **P2** (L62+L67) Waxed sign: click commands not executed, wrong failure sound. Fix: solo_packets.rs:462-466. [Merge of duplicate G6-F01 "Sign click commands not executed": On a click on a sign, read the click events of both faces and execute them through rc-commands with the player's source.]

### G6-F12: Block-entity tickers, conduit, containers

Files: `crates/rc-sim/src/blocks/ticks.rs`, `crates/rc-sim/src/life.rs`, `crates/rc-sim/src/containers.rs`, `crates/rc-sim/src/brewing.rs`, `crates/rc-sim/src/blocks/conduit.rs (new)`.

- **P1** (L69) Furnaces and brewing stands tick without a border or chunk level test. Fix: The same is_ticking(p) test (border + chunk level) for the generic tickers (ticks.rs:330) and the dedicated ones (life.rs:706, containers.rs:434, brewing.rs:111).
- **P1** (L66+L62) Conduit inactive on the server side. Fix: Conduit ticker: frame check, range, conduit_power effect, attack on monsters. [Merge of duplicate G5-F12 "Conduit not simulated": conduit.rs: frame, radius 16 by 7 blocks, hostile target in the water.]
- **P2** (L67) Shulker box that opens even when blocked. Fix: Test the collision of the open lid before the menu (containers.rs:212).

### G6-F18: Generation: ticks placed by decoration elements

Files: `crates/rc-worldgen/src/feature_basic.rs`, `crates/rc-worldgen/src/feature_geode.rs`, `crates/rc-worldgen/src/feature.rs`, `crates/rc-worldgen/src/decor_region.rs`, `crates/rc-sim/src/generation_nbt.rs`, `crates/rc-worldgen/src/tree.rs`, `crates/rc-worldgen/src/feature_veg.rs`.

- **P1** (L72) Sources without a fluid tick: waterfalls and lava falls frozen. Fix: Call r.schedule_fluid_tick(p) right after r.set_p (feature_basic.rs:453-455).
- **P2** (L72) Geode crack without a tick of the neighbouring fluids. Fix: After safe_set(air), schedule a fluid tick on the 6 neighbours that contain some (feature_geode.rs:160-161).
- **P2** (L72) simple_block ignores schedule_tick (eyeblossoms of the pale gardens). Fix: Block tick queue in Region, saved as block_ticks (generation_nbt.rs), fed from feature.rs:757.

### G6-F14: Technical blocks in singleplayer (structure, jigsaw, test), templates, /place, /clone, /locate, border

Files: `crates/rc-sim/src/commands/mod.rs`, `crates/rc-sim/src/commands/cmds_s9d.rs`, `crates/rc-worldgen/src/template.rs`, `crates/rc-worldgen/src/structure_place.rs`, `crates/rc-sim/src/progress/hooks.rs`, `crates/rc-sim/src/server.rs`, `crates/rc-sim/src/play.rs`, `crates/rc-sim/src/solo_packets.rs`, `crates/rc-sim/src/tech_blocks.rs (new)`, `crates/rc-sim/src/commands/cmds_world.rs`, `crates/rc-sim/src/commands/cmds_locate.rs`.

- **P1** (L39+L30+L33+L62+L68+L75) Structure, jigsaw, jigsaw_generate, test block, test instance: packets ignored. Fix: Decode the five packets, creative operator check, apply to the block entity; template save/load, size detection, jigsaw generation. Test instance actions: G3-F06. [Merge of duplicate G5-F18 "Structure, jigsaw and test blocks without effect": Packets set_structure_block, set_jigsaw_block, test_instance_block_action in solo_packets.rs.] [Merge of duplicate G6-F14 "Structure block inoperative in singleplayer (SAVE, LOAD, DETECT)": Handle set_structure_block in commands/mod.rs:513-548; capture (sorted blocks, block entities, entities, palette) and .nbt writing in template.rs.] [Merge of duplicate G6-F14 "Test blocks without server logic": Server logic of the start, log, fail and accept modes.]
- **P2** (L75+L36) /place template does not go through generation placement. Fix: Go through structure_place.rs (drawn palette, entities, waterlogging, shape pass, loot seed) (cmds_s9d.rs:140-203). [Merge of duplicate G3-F09 "/place template without entities or updates": Transformed entities, shape update outside strict (cmds_s9d.rs:185-200).]
- **P2** (L67) Mirror of the 16-step rotation inverted. Fix: Swap the two formulas (template.rs:568-569).
- **P2** (L66) Command block copied with auto:1 without a scheduled tick. Fix: Apply block_entity_data before set_placed_by, or schedule the tick when auto becomes true (progress/hooks.rs:82-92).
- **P2** (L68) Border warning default at 15 instead of 300. Fix: Replace 15.0 with 300.0 (commands/mod.rs:170, server.rs:561).
- **P2** (L68) The border does not block the server's entities. Fix: Implement border() in SimWorld (play.rs:45).
- **P2** (L78+L36) /clone does not copy scheduled ticks. Fix: Reschedule the ticks of the source box, shifted, with the same delay and the same priority; in move mode, remove those of the source (cmds_world.rs:209-316). [Merge of duplicate G3-F09 "/clone: validation, mask, placement order, ticks": cmds_world.rs:212-259.]
- **P2** (L36) /place structure and /place jigsaw always fail. Fix: Hook up the structure generation of rc-worldgen (cmds_s9d.rs:205-213).
- **P2** (L36) locate poi always returns not_found. Fix: poi_closest with the official radius (cmds_locate.rs:211).
- **P2** (L36) /clone ignores from/to <dimension>. Fix: Distinct source and target worlds (cmds_world.rs:209-215).

### G1-F07: Advancement and loot predicates

Files: `crates/rc-sim/src/progress/pred.rs`, `crates/rc-sim/src/loot.rs`.

- **P2** (L04) Smokey predicate: diverging copy of the beehive rule. Fix: pred.rs:452: call mobs/wild/hive.rs:123 (root cause: a single implementation).

### G1-F08: Entity rendering and glints

Files: `apps/client/src/entity_render.rs`, `apps/client/src/item_atlas.rs`, `apps/client/src/item.wgsl`, `apps/client/src/entity.wgsl`, `apps/client/src/entities.rs`, `apps/client/src/entity_render/animals.rs`, `apps/client/src/entity_render/keyframes.rs`, `apps/client/src/skins.rs`.

- **P2** (L01) Special glint of compasses and the clock rendered as a standard glint. Fix: Carry glint==2 (item_atlas.rs:532/634, entity_render.rs:5082) through to the shader; UV by decal projection (x0.5 GUI, x0.75 first person, rotations Y(pi) then X(-pi/2), dominant face, x1/128) in item.wgsl:43-49.
- **P2** (L07+L17) Baby sniffer: static transformation absent. Fix: animals.rs:1183-1198: apply sniffer.baby_transform extracted at t=0 (keyframes.rs). [Merge of duplicate G2-F02 "Snifflet without baby head transformation": Static scale and offset of the head (animals.rs near 1183).]
- **P2** (L07) Baby sniffer: adult texture instead of snifflet. Fix: entity_render.rs:2949-2953: baby sniffer case -> entity/sniffer/snifflet.
- **P2** (L09+L23+L26) Mannequin cape neither simulated nor drawn. Fix: entities.rs:874: avatar tick for mannequins; entity_render.rs:3968/1603: push_cape with the profile's cape. [Merge of duplicate G2-F03 "Mannequin without cape; profile elytra ignored; cape lost without skin": push_cape for mannequins; skins.rs: cape and ELYTRA independent of the skin.]

### G1-F09: Text input, IME, chat and widgets

Files: `apps/client/src/widgets.rs`, `apps/client/src/links.rs`, `apps/client/src/chat.rs`, `apps/client/src/book.rs`, `apps/client/src/sign_edit.rs`, `apps/client/src/pack_screen.rs`, `apps/client/src/notices.rs`.

- **P2** (L01,L11+L16) IME only in chat, composition drawn inline (merge L01+L11). Fix: sync_ime (links.rs:484-506) for any focused field with the cursor area x scale; shared composition window (sprite widget/preedit, 300 ms cursor) called by chat.rs:1180-1189, draw_field_blink (widgets.rs) and draw_area (book.rs). Update gaps-client.md:163-164 at merge. [Merge of duplicate G2-F12 "IME composition": chat.rs:1181-1189: shared rendering and splitting into blocks.]
- **P2** (L11) Click in a field: approximate position, no Shift, no double click, no drag. Fix: widgets.rs:763-768: real glyph width, move_to(k, shift), double click by words, drag in Panel::motion.
- **P2** (L11) Red prompt chat_screen.restricted absent. Fix: chat.rs:1003-1102: indicator from commands_refresh, red underlined text, click towards the restriction screen (links.rs).
- **P2** (L12+L14) Sign editor without selection, clipboard or word-wise movement. Fix: sign_edit.rs:62-101: take over the logic of book.rs:320-340. [Merge of duplicate G2-F09 "Sign field without selection, clipboard or word-wise movement": sign_edit.rs: take over the field helper of book.rs or widgets.rs.]
- **P2** (L13) Accessibility onboarding: Sounds/Accessibility/Language buttons without icon or tooltip. Fix: notices.rs:190-192: text+icon variant of widgets.rs and two tooltips.

### G1-F11: Item tints (firework, custom_model_data, team)

Files: `apps/client/src/stack.rs`, `crates/rc-assets/src/item.rs`.

- **P2** (L09) Firework star always grey. Fix: stack.rs: implement firework_colors from minecraft:firework_explosion (computation already in rc-assets item.rs:1152-1162).
- **P2** (L09) custom_model_data tint never read. Fix: stack.rs:482/491: custom_model_data_color and _string.

### G1-F12: HUD, bundle and spectator

Files: `apps/client/src/hud.rs`, `apps/client/src/hud_more.rs`, `apps/client/src/menu.rs`, `apps/client/src/bundle.rs`, `apps/client/src/spectator.rs`.

- **P2** (L10) Locator bar never drawn in spectator. Fix: hud.rs:587-589: the spectator return only skips the hotbar.
- **P2** (L12) Digit of the spectator menu slots frozen at 1-9. Fix: spectator.rs:363: label of the slot key (keys.rs).

### G1-F13: Bidirectional text and items in text

Files: `crates/rc-text/src/lib.rs`, `crates/rc-text/src/bidi.rs (new)`, `apps/client/src/gui.rs`, `apps/client/src/reload.rs`.

- **P2** (L10+L26) Neither bidi nor Arabic shaping. Fix: rc-text: logical to visual order (unicode-bidi) and Arabic contextual forms for bidi languages, at drawing (gui.rs:944) and at width computation. [Merge of duplicate G2-F12 "No bidi or Arabic shaping": bidirectional flag; unicode-bidi and Arabic shaping in rc-text and gui.]
- **P2** (L12+L32) Items in text: only the gui atlas drawn. Fix: reload.rs:284-286, lib.rs:200-203: blocks/items atlas and player heads; gui.rs:990-997: draw at y-1. [Merge of duplicate G3-F18 "Items in text: gui atlas only, player heads absent": Blocks/items atlas and Player 8x8 variant + hat (rc-text lib.rs:171-177, reload.rs:282-287).]

### G1-F14: World screens and statistics

Files: `apps/client/src/stats_screen.rs`, `apps/client/src/create_world.rs`, `apps/client/src/special.rs`, `apps/client/src/world_options.rs`.

- **P2** (L13) World options: LAN links/guest commands/imposed mode absent, port not validated. Fix: world_options.rs:596-633/1000-1015: constraints at each change, greyed-out buttons, tooltips; port 1024-65535 in red (101).
- **P2** (L12) Statistics tabs: wrong background and shifted x. Fix: stats_screen.rs:577 menu_background; 272-277 formula of create_world.rs.

### G1-F15: Singleplayer loading, title screen, symbolic links

Files: `apps/client/src/flow.rs`, `apps/client/src/front.rs`, `apps/client/src/world_list.rs`, `apps/client/src/pack_stack.rs`, `crates/rc-sim/src/prep.rs (new)`, `crates/rc-assets/src/packs.rs`, `apps/client/src/server_packs.rs`, `crates/rc-sim/src/storage.rs`.

- **P2** (L13+L39) Singleplayer loading screen without chunk map or bar. Fix: flow.rs:603 opens the singleplayer Loading screen directly; front.rs:845: chunk map (statuses from prep.rs), smoothed bar, 500 ms delay for a new world. [Merge of duplicate G3-F18 "Loading screen without chunk map or bar": Expose the statuses from load_progress.rs, draw in front.rs:823-845.]

### G1-F10: Main loop: pause, draw order, bed, crashes, toasts

Files: `apps/client/src/main.rs`, `apps/client/src/pause.rs (new)`, `apps/client/src/ticking.rs`, `apps/client/src/bed.rs`, `apps/client/src/debug_keys.rs`, `apps/client/src/profiling_zip.rs`, `apps/client/src/crash_report.rs (new)`, `apps/client/src/toasts.rs`, `apps/client/src/tutorial.rs`, `crates/rc-audio/src/mixer.rs`, `apps/client/src/flow.rs`.

- **P2** (L06+L14+L27) Singleplayer pause: client that keeps simulating, wrong pausing screens (partly known). Fix: Pauses-the-game flag per screen (pause.rs), read by flow.rs:628 (one line, merge after F15); main.rs:1923 skips game_tick; ticking.rs freezes the partial tick; rc-audio suspends everything except music and interface. [Merge of duplicate G2-F07 "Books, block editors and dialogs with pause=true do not freeze singleplayer": flow.rs solo_pause: take into account book.rs, op_screens.rs, dialog.rs (pause field), read-only.] [Merge of duplicate G3-F15 "Singleplayer pause does not cut sounds": rc-audio pause outside MUSIC/UI, skip the tick of sustained sounds.]
- **P2** (L04) No crash report on panic; F3+C in the reduced format. Fix: std::panic::set_hook (covers the integrated server thread) writing crash-reports/crash-<date>-client.txt in the game's format; single formatter reused by debug_keys.rs:489 and profiling_zip.rs:44.
- **P2** (L10) Closed chat still drawn with F1. Fix: main.rs:1338: draw only if !hud.hidden or chat open.
- **P2** (L13) Chat in bed closed then reopened at each Enter. Fix: main.rs:2630-2640: clear the field without closing or grabbing the mouse; bed.rs:121-140: on waking, also close a draft.
- **P2** (L09) Mannequin: description under the name absent. Fix: main.rs:3606-3614: description (or default translated label) under the name within 10 blocks.

### G1-F16: F3 and debugging

Files: `apps/client/src/debug_screen.rs`, `apps/client/src/debug_switcher.rs`, `apps/client/src/debug_charts.rs`, `apps/client/src/gpu_backend.rs`, `crates/rc-audio/src/engine.rs`, `crates/rc-sim/src/debug_report.rs (new)`.

- **P2** (L12) F3 toggle of an entry with no status: overlay instead of always. Fix: debug_screen.rs:514-523 Unset state distinct from Never; 458 write an explicit Never.
- **P2** (L13) Adventure icon of the F3+F4 switcher: simple map. Fix: debug_switcher.rs:9: buried_treasure_map.

### G2-F06: First-person hand, leash, elytra and End sounds

Files: `apps/client/src/hand.rs`, `apps/client/src/leash.rs`, `apps/client/src/environ.rs`, `apps/client/src/world_sounds.rs`, `apps/client/src/riding.rs`, `apps/client/src/entity_loops.rs`.

- **P2** (L20) Bow and crossbow: two hands drawn. Fix: hand.rs:730: hand selection.
- **P2** (L20) Leash attachment in first person. Fix: leash.rs:112: computation in first-person view.
- **P2** (L20+L27) Elytra gliding sound absent. Fix: Sound instance tracked when gliding starts. [Merge of duplicate G3-F15 "No elytra flight sound": Sustained loop after 20 ticks, volume and pitch according to speed.]
- **P2** (L25+L27) Sound weather.end_flash absent. Fix: environ.rs: sound 30 ticks after the start of the flash, in its direction. [Merge of duplicate G3-F15 "weather.end_flash never played": Sound delayed by 30 ticks, camera + direction×10, without attenuation (environ.rs:191).]

### G2-F08: Singleplayer world: world pack, creation, world list, packs

Files: `apps/client/src/world_list.rs`, `apps/client/src/create_world.rs`, `apps/client/src/server_packs.rs`, `apps/client/src/pack_screen.rs`.

- **P2** (L16+L26) The world's resources.zip never loaded. Fix: On opening (world_list.rs), stack resources.zip via server_packs.rs; confirmation on failure.
- **P2** (L26) No server pack download notification. Fix: download.pack.* toast in server_packs.rs.
- **P2** (L16) Enter in the name field does not create the world. Fix: create_world.rs:700: remove the focus condition.
- **P2** (L16) Recreation: Custom type, quirk not reproduced. Fix: create_world.rs:335-358: NORMAL or FLAT, then reset to Default.

### G2-F11: Inventory screens, recipe book and options menus

Files: `apps/client/src/recipe_book.rs`, `apps/client/src/recipe_app.rs`, `apps/client/src/menus.rs`, `apps/client/src/notices.rs`, `apps/client/src/hud_more.rs`, `apps/client/src/screens_work.rs`, `apps/client/src/screens_trade.rs`, `apps/client/src/creative_screen.rs`, `apps/client/src/world_options.rs`, `apps/client/src/spectator.rs`.

- **P2** (L15) Recipe book in narrow mode that reopens. Fix: recipe_book.rs:681: toggle button path (setting and packet).
- **P2** (L15) Anisotropy: raw key on an inactive button. Fix: menus.rs:279/714: slider disabled, label and value.
- **P2** (L15) Multiplayer warnings truncated, without frame or scrolling. Fix: notices.rs: scrolling text with frame, colour of the box.
- **P2** (L14) XP bar not forced in the anvil and enchanting. Fix: hud_more.rs:645.

### G2-F12: Chat, blocked servers, block models (bake) and client data

Files: `apps/client/src/exact.rs`, `apps/client/src/chat.rs`, `crates/rc-client/src/status.rs`, `crates/rc-net`, `crates/rc-assets/src/bake.rs`, `crates/rc-world/src/lib.rs`, `apps/client/src/debug_render.rs`.

- **P2** (L18) reset_chat erases the settings and the history. Fix: exact.rs:69: only clear the messages.
- **P2** (L19) Blocked servers list not verified. Fix: status.rs or rc-net: fingerprints; name, generic suffixes and IP.

### G3-F06: Gametest (/test, test instances) and admin broadcast of commands

Files: `crates/rc-sim/src/commands/test.rs (new)`, `crates/rc-sim/src/gametest/ (new)`, `crates/rc-sim/src/commands/mod.rs`.

- **P2** (L30) /test command and in-game gametest engine absent. Fix: /test tree, execution of block_based and function instances, grid, failure tracking, environment; test instance actions received by G3-F04.

### G3-F07: Basic commands and world creation

Files: `crates/rc-sim/src/commands/cmds_basic.rs`, `crates/rc-sim/src/commands/src.rs`, `crates/rc-sim/src/newworld.rs`, `crates/rc-sim/src/demo.rs (new)`, `apps/client/src/launch.rs`.

- **P2** (L34+L36) /difficulty bypasses the HARD lock of hardcore mode. Fix: Call set_difficulty(id, true) (solo_packets.rs:285-294) in cmd_difficulty (cmds_basic.rs:348-369).
- **P2** (L36) /kick absent. Fix: Disconnection of the guest (function provided by G3-F05), singleplayer.failed for the host.
- **P2** (L37) /unpublish always fails. Fix: lan_unpublish with the two official messages (cmds_basic.rs:316).

### G3-F08: Commands: compute, fetchprofile, posteffect, attributes, loot, enchant

Files: `crates/rc-sim/src/commands/cmds_s9a.rs`, `crates/rc-sim/src/commands/cmds_s9b.rs`, `crates/rc-sim/src/commands/cmds_ents.rs`.

- **P2** (L36) /compute absent. Fix: Add in dispatch_s9 with the numeric providers of the loot tables.
- **P2** (L36) /fetchprofile absent. Fix: name|id|entity through the profile cache.
- **P2** (L36) /posteffect absent on the server side. Fix: Emit the packet already handled by the client.
- **P2** (L36) /attribute base reset takes the player's default value. Fix: Attribute table of the targeted type (cmds_s9b.rs:882, 999).

### G3-F10: NBT, SNBT and /data

Files: `crates/rc-sim/src/commands/cmds_data.rs`, `crates/rc-sim/src/commands/snbt.rs`, `crates/rc-proto/src/nbt.rs`, `crates/rc-sim/src/commands/ents.rs`.

- **P2** (L35) data modify … compute absent. Fix: compute branch (cmds_data.rs:661-714), Nbt::Float or Nbt::Int.

### G3-F11: Arguments, selectors, predicates and text and item components

Files: `crates/rc-sim/src/commands/args.rs`, `crates/rc-sim/src/commands/select.rs`, `crates/rc-sim/src/progress/pred.rs`, `crates/rc-sim/src/items.rs`.

- **P2** (L28) Simplified item predicate (|, ~, count=, default components). Fix: Reuse ItemPredParser of rc-commands, evaluate in item_matches (args.rs:318-340).
- **P2** (L28) name= ignores the translated default name. Fix: Flatten with en_us (select.rs:370).
- **P2** (L32) nbt text component never resolved. Fix: Sources entity/block/storage, interpret, plain, separator (args.rs:473-535).
- **P2** (L29) potion_contents predicate misread. Fix: potions and effects (pred.rs:284-289).
- **P2** (L29) Component patch not normalised. Fix: Remove the entry equal to the default (items.rs:77).

### G3-F13: World tick order and chunks

Files: `crates/rc-sim/src/blocks/ticks.rs`, `crates/rc-sim/src/life.rs`, `crates/rc-sim/src/mobs/tick.rs`, `crates/rc-sim/src/drops.rs`, `crates/rc-sim/src/blocks/entities.rs`, `crates/rc-sim/src/projectiles/mod.rs`.

- **P2** (L38) Different world tick order. Fix: Follow block-behaviours.md §1.2, all entities before block entities (ticks.rs:222-223, life.rs:706-708, mobs/tick.rs:39).
- **P2** (L38) Non-mob entities ticked without a chunk test. Fix: chunk_entity_ticking (drops.rs:362, entities.rs:304, projectiles/mod.rs:740).

### G4-F08: Monsters and entities: creeper, shulker, TNT, sulfur cube, creaking, warden

Files: `crates/rc-sim/src/drops.rs`, `crates/rc-sim/src/mobs/end/shulker.rs`, `apps/client/src/entity_motion.rs`, `crates/rc-sim/src/blocks/entities.rs`, `crates/rc-sim/src/mobs/cubes.rs`, `crates/rc-sim/src/mobs/recent/creaking.rs`, `crates/rc-sim/src/mobs/recent/warden.rs`, `crates/rc-sim/src/mobs/recent/breeze.rs`, `crates/rc-sim/src/mobs/illagers/spells.rs`, `crates/rc-sim/src/mobs/surface/bat.rs`, `crates/rc-sim/src/mobs/nav.rs`, `crates/rc-sim/src/mobs/tick.rs`, `crates/rc-sim/src/mobs/brainy/sniffer.rs`, `crates/rc-sim/src/mobs/surface/turtle.rs`, `crates/rc-sim/src/mobs/surface/rabbit.rs`.

- **P2** (L49) Shulker push without the player or collisions. Fix: shulker.rs:211-225 with collisions; entity_motion.rs:51 for the local player
- **P2** (L49) TNT: power and block from the NBT ignored. Fix: blocks/entities.rs:206, 697: read and save
- **P2** (L50) Sulfur cube explosion treated as a mob. Fix: cubes.rs:731-742: TNT type, fire, calculator
- **P2** (L50) Creaking attack sound absent. Fix: creaking.rs: sound after a successful hit

### G4-F12: Iron and copper golems

Files: `crates/rc-sim/src/mobs/golems/iron.rs`, `crates/rc-sim/src/mobs/golems/copper.rs`, `crates/rc-sim/src/mobs/golems/mod.rs`, `crates/rc-sim/src/mobs/recent/mod.rs`.

- **P2** (L45) Village defence without line of sight. Fix: iron.rs:284,294
- **P2** (L47) Copper golem does not open doors. Fix: copper.rs:434-492: villagers' door behaviour

### G4-F13: Piglins and spear

Files: `crates/rc-sim/src/mobs/piglins/spear.rs`, `crates/rc-sim/src/mobs/piglins/behave.rs`.

- **P2** (L45+L46) Mounted spear: mount speed and knockback. Fix: spear.rs:461-517: mount multiplier (4.0, 1.4, 1.0), +2 knockback if mounted
- **P2** (L45) Piglins' nemesis without the Wither. Fix: behave.rs:129,221

### G4-F14: Integrated server: pack tags, /reload, clocks

Files: `crates/rc-sim/src/registries.rs`, `crates/rc-sim/src/commands/cmds_s9a.rs`, `crates/rc-sim/src/save.rs`, `crates/rc-sim/src/server.rs`, `crates/rc-sim/src/commands/mod.rs`.

- **P2** (L40) Pack tags not transmitted; /reload without resend. Fix: registries.rs:185, cmds_s9a.rs:1028
- **P2** (L43) Clock pause and speed lost. Fix: save.rs:224-240, server.rs:298

### G4-F15: Client: effects of animals and small mobs

Files: `apps/client/src/entity_render/animals.rs`, `apps/client/src/entity_render/creatures.rs`, `apps/client/src/entities.rs`, `apps/client/src/world_fx.rs`.

- **P2** (L48) Splashes of the wolf shaking off. Fix: animals.rs: particles at events 8/56
- **P2** (L48) Sniffer: index 17 instead of 18, digging particles and sound. Fix: entities.rs:473
- **P2** (L48) Particles of the panda, the glow squid, the rabbit. Fix: animals.rs:1773, 1825; creatures.rs
- **P2** (L50) Landing particles of slimes. Fix: creatures.rs slime tick
- **P2** (L47) Head rotation sound of the copper golem. Fix: creatures.rs:1141
- **P2** (L49) Guardian out of the water (part of the monster effects difference). Fix: creatures.rs:1218

### G4-F16: Client: monster effects (dragon, creaking, illagers, breeze, warden)

Files: `apps/client/src/entity_fx.rs`, `apps/client/src/entity_render.rs`, `apps/client/src/entity_render/dragon.rs`.

- **P2** (L48) Dragon sounds and particles. Fix: dragon.rs: client tick according to the phase
- **P2** (L49) Effects of the enderman, blaze, phantom, endermite, ravager. Fix: entity_fx.rs:33
- **P2** (L50) Creaking coming undone: blinking eyes. Fix: entity_render.rs:3665, 5823
- **P2** (L50) Breeze: local particles and sounds. Fix: entity_fx.rs
- **P2** (L50) Illager spellcasting particles. Fix: entity_fx.rs, synchronised spell
- **P2** (L50) Warden: heartbeat and particles. Fix: entity_render.rs:5765, entity_fx.rs

### G4-F17: Client: trading, input, tooltips, profiling and miscellaneous

Files: `apps/client/src/screens_trade.rs`, `apps/client/src/entity_render/blocks.rs`, `apps/client/src/front.rs`, `apps/client/src/stack.rs`, `apps/client/src/widgets.rs`, `apps/client/src/sign_edit.rs`, `apps/client/src/main.rs`, `apps/client/src/flow.rs`, `apps/client/src/creative_app.rs`, `apps/client/src/recipe_book.rs`, `apps/client/src/debug_options.rs`, `apps/client/src/tooltips.rs`, `apps/client/src/keys.rs`, `apps/client/src/profiling_zip.rs`, `apps/client/src/profiler.rs`, `crates/rc-data/src/prepare.rs`, `crates/rc-sim/src/storage.rs`, `apps/client/src/edit_world.rs`.

- **P2** (L51) Trading: offer uses not incremented. Fix: screens_trade.rs:44: increment when the result is taken

### G5-F17: Spawns and spawn eggs

Files: `crates/rc-sim/src/mobs/spawn.rs`, `crates/rc-sim/src/mobs/env.rs`, `crates/rc-sim/src/mobs/surface/bat.rs`, `crates/rc-sim/src/mobs/trial.rs`, `crates/rc-sim/src/mobs/explode.rs`.

- **P2** (L54+L60) Spawn egg: custom_name and entity_data ignored. Fix: spawn.rs:547-578: apply like the /summon NBT.
- **P2** (L61) No 24-block exclusion around the world spawn point. Fix: spawn.rs:799: distance² > 576.
- **P2** (L65) Copper grates allow spawns. Fix: spawn.rs:1438-1450: 8 grates in NEVER.

### G6-F15: Saving, /schedule, /data, world list

Files: `crates/rc-sim/src/save.rs`, `crates/rc-sim/src/commands/exec.rs`, `crates/rc-sim/src/commands/cmds_data.rs`, `crates/rc-sim/src/commands/cmds_world.rs`, `crates/rc-sim/src/blocks/ticks.rs`, `crates/rc-sim/src/storage.rs`, `apps/client/src/world_list.rs`.

- **P2** (L77+L37) Scheduled functions not saved with the world. Fix: Serialise cmd.scheduled (name, tick, tag) with the world data and re-read it at loading (field already present in commands/mod.rs:95, no change to that file). [Merge of duplicate G3-F06 "/schedule queue not saved": Serialise cmd.scheduled (mod.rs:95) into data/scheduled_events, reload, normalise the id.]

### G6-F17: Loot: functions, providers, entries

Files: `crates/rc-sim/src/loot.rs`, `crates/rc-sim/src/lootfn.rs`, `crates/rc-sim/src/commands/cmds_s9b.rs`, `crates/rc-sim/src/progress/pred.rs`, `crates/rc-sim/src/items.rs`.

- **P2** (L77) 20 loot function types not handled. Fix: Branches in lootfn.rs, reusing trades.rs:451/468 read-only; list operations replace, insert and append for lore, pages and custom_model_data.

### G6-F19: Structures: saving of pieces and placed mobs

Files: `crates/rc-worldgen/src/structure_nbt.rs`, `crates/rc-worldgen/src/structure.rs`, `crates/rc-worldgen/src/structure_scattered.rs`, `crates/rc-worldgen/src/structure_monument.rs`, `crates/rc-worldgen/src/structure_templ.rs`, `crates/rc-worldgen/src/density.rs`.

- **P2** (L73) Hut, temple or pyramid piece: box frozen at y=64, HPos not saved. Fix: Write state.bbox, Width, Height, Depth and HPos for CodedPiece::Scattered (structure_nbt.rs:170-182); re-read HPos at loading.
- **P2** (L74) Witch, elder guardians and allays placed without initialisation. Fix: rc_finalize_structure marker (structure_scattered.rs:251, structure_monument.rs:409-415, structure_templ.rs:741-750 for all the mansion's mobs); check that finalize_spawn of rc-sim covers these three types.

### G6-F20: Navigation and maps

Files: `crates/rc-sim/src/mobs/nav.rs`, `crates/rc-sim/src/mobs/mounts/camel.rs`, `crates/rc-sim/src/maps.rs`.

- **P2** (L76) The camel does not go over fences. Fix: can_walk_over_fences flag in the walk evaluator, raised for the camel, Fence exclusion lifted in this case (nav.rs:729).

### A2-W1: Locator bar waypoints: integrated server, LAN and /waypoint

Files: `crates/rc-sim/src/waypoints.rs (new)`, `crates/rc-sim/src/attrs.rs`, `crates/rc-sim/src/commands/cmds_s9a.rs`.

- **P2** (L40+L55+L78) Locator bar waypoints absent. Fix: waypoints.rs: tracking of emitters, attribute ranges, connection, movement, leaving [Merge of duplicate G5-F05 "Locator bar waypoints never emitted on LAN": waypoints.rs: track emitters and send track_waypoint according to the ranges.] [Merge of duplicate G6-F20 "No waypoint packet sent by the integrated server": waypoints.rs manager: entities with range > 0, per-player tracking, track/update/untrack packets. Not verified.]
- **P2** (L37) /waypoint modify with no network effect, list limited to the host. Fix: Waypoint manager (tracking, packets, transmit range), hook up modify/list (cmds_s9a.rs:374, 396).

### A2-E2: Redstone (investigation): order of sending block changes, bench scene, client queue

Files: `crates/rc-sim/src/server.rs`, `crates/rc-sim/src/play.rs`, `tools/bench/scenes.py`, `apps/client/src/main.rs`.

- **P2** (redstone investigation) Sending order to the client: the block changes of the tick phase and the block events do not go out in the game's order. Fix: Game: the changes accumulated during the tick phase go out before the block events; those created by events and block entities go out on the next tick. Us: events sent during their phase, then all the changes at the end of the tick (crates/rc-sim/src/server.rs:878, play.rs:464). Align the order, then a test: a piston triggered by a scheduled tick, order of the received packets.
- **P2** (redstone investigation) Client: the received packet queue is drained according to what it contains at the start of the frame and asynchronous meshing starts from the nearby columns; the same tick can be displayed over two frames (to measure). Fix: apps/client/src/main.rs:1078-1083 and 988-1036. One-frame effect, split by chunk and not by row; it is not the bench's symptom. Measure before any fix.

### A2-E3: Meshing and occlusion (investigation): loaded unmeshed columns, light queue, walk distance

Files: `apps/client/src/render.rs`, `crates/rc-mesh/src/visibility.rs`, `apps/client/src/main.rs`, `apps/client/src/remesh.rs`.

- **P2** (L25) Occlusion: squared distance, no vertical limit. Fix: render.rs:1875-1882 and visibility.rs:361-366: cylinder and |dy|.
- **P2** (meshing investigation) Visibility walk: a column that is loaded but not yet meshed is treated as absent (zones hidden at loading and after a teleport; nothing is drawn until the camera's column is meshed). Fix: The game traverses the air sections of a loaded, uncompiled column, and keeps the other sections as blocking nodes (except the camera's, which propagates everywhere). On our side, the graph is only fed by meshed columns and empty meshed columns (apps/client/src/render.rs:1895-1917, 1580-1590, 1363-1381); an absent column means "not loaded" (crates/rc-mesh/src/visibility.rs:361-367) and without a meshed camera column there is no starting point (visibility.rs:368-406). Fix: pending_vis table in the Renderer, one list per section (all open if the section is empty according to crates/rc-world/src/lib.rs:33, all closed otherwise), filled from main.rs on reception of a chunk as long as the column is not compiled, recomputed when a section goes from empty to non-empty or the reverse, cleared when the mesh is sent (render.rs:1363), on removal (1554-1564) and on clearing (1567), vis_rev incremented at each change; read as a last resort after columns then empty_vis. Pitfall: the list of reached sections also serves the visibility of entities (render.rs:733-764); filter it to the meshed columns for entities.
- **P2** (meshing investigation) Entities: we require a section reached by the occlusion walk, the game only the frustum and a compiled section (name tags and glow outlines behind a wall perhaps absent; to confirm in game before fixing). Fix: apps/client/src/render.rs:734-764 (Renderer::visibility). Check in game (glowing mob behind a wall, at the same spot in the official game) before fixing; if confirmed, test frustum membership and section compilation instead of reach by the walk.

## Investigations

### Pistons (mission G5-F14)

- **Root cause of the bug reported on 19/10** (P1, DIFFERENT): our server behaves like the game. Moving blocks are placed without being sent to the client, and the game counts on the client to recreate them by replaying the piston's block event. Our client replays nothing: `apps/client/src/main.rs:821-823` forwards the event to `block_entities.rs:737-753`, which exits for lack of a block entity at the piston's base and ignores any `b0 != 1`. Result: the head appears all at once and the blocks jump to their final place 2 or 3 ticks later. The rendering exists (`entity_render/blocks.rs:1075-1082`) but is never fed, and distinguishes neither the short head nor the fixed base of a retracting piston. The comment in `crates/rc-sim/src/blocks/piston.rs:311` announces this replay, never written.
- **Fix**: make the resolution of the pushed structure (`piston.rs:136-290`) generic and public, replay the trigger on the client side on a piston's event (without the signal check), place the moving blocks locally with their block entities, faithful client tick (step of 0.5, removal 5 ticks after the end), rendering in three cases. The server remains master of the final block. An automatic test (piston facing east in front of a stone) is described in the mission; it fails today.
- **Separate difference, unknown until now** (P1, ABSENT): the moving block's tick (`piston.rs:394-417`) moves no entity: no push, no slime launch, no honey adhesion, on the server side as for the local player. Merged with the G5-F14 difference that already noted it.

### Redstone: phase shift in the `perf:redstone` scene (mission A2-E2)

- **Verdict**: very probably faithful to the game. The 16 rows are identical and decoupled; the only source of randomness that touches the mechanism is the grass under the opaque blocks (lamp, observers, piston base), which turns back into dirt at a random tick and notifies the piston placed on it. The piston then rechecks whether it must extend and can go out with a one-tick offset, which propagates along the row. The official game must show the same effect, on other rows (other random generator). Our logic is the same (`crates/rc-sim/src/blocks/nature.rs:762-766`, `blocks/level.rs:331`, `level.rs:935`, `piston.rs:39-63`) and so is the moment of the random tick (`blocks/ticks.rs:180-220`).
- **Compared with no difference**: scheduled tick queue (order, priority, limit 65536, deferral outside the active chunk), block event queue, observer, resolution of the pushed structure.
- **Fix**: nothing in rc-sim. Place the scene on stone, or `gamerule random_tick_speed 0` at the head of `perf:redstone` (changes the measured load); add a clock + sticky piston + slime scenario on a pushable floor.
- **Differences found along the way**: order of sending block changes and events (P2, `server.rs:878`, `play.rs:464`); two missing shape updates of the piston (P3, already in G5-F14); client packet queue drained per frame with asynchronous meshing (P2 to measure, `apps/client/src/main.rs:1078-1083`).
- **Control protocol**: same bench world on the official server locally and on ours; `random_tick_speed 0`, `tick freeze`, `function perf:redstone`, then `tick step 1` and `save-all flush` at each step, reading the zone: no more phase shift expected on either side.

### Meshing (mission A2-E3)

- **(a) Rule of the 8 ready neighbours** (P3, DIFFERENT, already known in `docs/perf.md:128-131`): the game waits for the light of the 8 neighbouring columns to be activated, through a queue drained in batches at each frame outside pause; on our side the light arrives with the blocks (`apps/client/src/main.rs:800-819`) and `remesh.rs:182-211` only tests presence. Only the pace of appearance changes; noting it as accepted is recommended.
- **(b) Column loaded but not yet meshed** (P2, DIFFERENT): the game traverses the air sections of a loaded uncompiled column and keeps its other sections as blocking nodes; on our side an unmeshed column means "not loaded" (`crates/rc-mesh/src/visibility.rs:361-367`), hence zones hidden at loading and after a teleport, and no starting point of the walk until the camera's column is meshed (`visibility.rs:368-406`). Fix: provisional visibility lists (all open if the section is empty, all closed otherwise) for loaded unmeshed columns, filtering the entities to the meshed columns (`render.rs:733-764`).
- **Side leads**: entity visibility goes through the occlusion walk on our side whereas the game only uses the frustum and a compiled section (P2 to confirm in game, in the mission); the game only compiles the visible sections, we compile every dirty column (`main.rs:995-1050`, performance effect only, no mission).

## P3, per mission

### G1-F02: Advancement triggers (fields, missing callers, sources)

- (L05) recipe_unlocked reads recipe instead of recipes. trig.rs:230: read recipes.
- (L05) bee_nest_destroyed ignores blocks/state. trig.rs:555: test c[blocks]/c[state] through block_cond.
- (L04) entity_struck evaluated from the lightning's origin. trig.rs:780: origin = the player's position.
- (L04) Item reward: delay 40, no owner, empty loot context. progress/mod.rs:821/854: Ctx with this_entity = player and origin; local throw delay 0, player owner (without touching drops.rs, a file of F01).
- (L04) Advancement validation at loading absent. parse_adv (mod.rs:198): reject with a warning the 3 cases (inconsistent requirements/criteria, background with parent, root without background).

### G1-F06: Resource pack stacking: fonts, .mcmeta, overlays, filter, blockstates

- (L12) Suspicious glyphs not filtered in chat; bitmap width in greyscale on the alpha. font.rs:216-218 and 404-407: advance filter outside [0,32]; luminance for greyscale PNGs.
- (L40) filter section of pack.mcmeta ignored. source.rs
- (L26) Missing model invisible; blockstates not merged. bake.rs:508-526/602.

### G2-F09: Structure block (buttons) and server dialogs

- (L14) Wait button without countdown. Label with the seconds remaining.
- (L14) Dialog buttons without tooltip. Read tooltip and draw it on hover.
- (L37) Item body as text, macro variable absent. dialog.rs:184, 253-272.

### G2-F07: Chunks at respawn, name tags, targeting and outline, client-side border, quick launch

- (L20) attack_range ignored for targeting. Ray cast towards entities with attack_range.
- (L18) player_position applied to the passenger player. Do not move; acknowledge with the current position.
- (L18) teleport_entity of a removed vehicle not carried over to the player. entities.rs:1234: keep the identifier of the last vehicle and apply it to the player.
- (L18) End of world loading too early. flow.rs:731: wait for the camera section's meshing, plus 500 ms in a new singleplayer world.
- (L22) Sniffer tag height. Kind-specific tag attachment.
- (L23) deadmau5 name not raised. +0.25 height for this name.
- (L26) Tag background opacity frozen. nametag.rs: options backgroundForChatOnly and textBackgroundOpacity.
- (L14) Keys 1-9 on a disabled crafter slot. main.rs:3051: toggle the slot then SWAP.

### G2-F10: Client particles: block tint, harmful gas, dust, flames, block_marker

- (L29) Swap animation ignored for max_damage. hand.rs:407.

### G2-F02: Mobs (client): phantom, sulfur cube, reins, panda, happy ghast, shadows

- (L16) Camel walk cut when sitting or dashing. animals.rs:286-292: apply_walk without these conditions.
- (L16) Equine stride in water. animals.rs:473-474: ×0.2 if touches_water.
- (L22) Cat lying on a sleeping player. animals.rs:1374: cat offset.
- (L18) "Insecure server" toast repeated. world_fx.rs:886: "already seen" flag reset to false on a new connection or a transfer.

### G2-F04: Dynamic item properties (recovery compass, rod, bundle, Christmas chest)

- (L24+L26+L41) local_time in UTC, locale and time_zone ignored. stack.rs:584: local time; respect time_zone and locale. [Merge of duplicate G2-F01 "Date of splashes and the Christmas chest in UTC": front.rs:69-82: local date; function shared with blocks.rs:147-156.] [Merge of duplicate G4-F17 "Special dates in UTC": blocks.rs:148, front.rs:70, stack.rs:582: local time]
- (L24) Crossbow charge: first ammunition only. stack.rs:455-457: test all the ammunition.
- (L23) broken/damaged ignore unbreakable. item.rs:1172-1173.
- (L23) carried, keybind_down, component, view_entity not evaluated. item.rs:1183/1188-1190.
- (L25) Skin patch ignored on head items. item_special.rs:43-76: read the profile's patch.

### G2-F03: Humanoid models, held items, compass and clock outside the HUD

- (L16) Arms when gliding: raw pitch. entity_render.rs:1020: head angle after forcing.
- (L17) Creeper: leg phases inverted (quirk). Creeper variant of quadruped() (entity_render.rs:1350/5902).
- (L17) Skeleton: one-pixel arm offset. push_held: ±1/16 in x.
- (L17) Copper golem: item carried on the head. push_head (4267): copper_golem entry.
- (L17) Armor stand without arms: items not drawn. Arm matrix computed even with the part hidden.
- (L20) Copper golem's flower bed without offset. entity_render.rs:4748: translation.
- (L23) Invisible and glowing: layers without outline. Push these layers as outline only instead of skipping them.
- (L23) Wolf collar that turns red. Skip the damage overlay for this collar.
- (L23) Frightened panda: held item visible. push_mob_item (4525): worried gene and thunderstorm condition.
- (L18) Mannequin skin layers. entity_render.rs:5885: extend to the mannequin.

### G2-F05: Item entities (carried blocks, ominous spawner, wind charge) and maps

- (L22) Minecart without anti-flicker offset. objects.rs:1351: pseudo-random offset according to the identifier.
- (L22) Painting oriented by the spawn data. Read meta 8, fall back to e.data.
- (L18) Shulker bullet not oriented. apps/client/src/objects.rs:642: yaw and pitch before push_layers.

### G3-F02: Mining, striking and breaking blocks, block pick, incoming player packets

- (L39) Wrong height limit message. Key, value and red in placement.rs:268-270; play.rs:419/427.
- (L38) Entities not pushed when the snow thickens. Upward push after the thickening (nature.rs:858).
- (L68) Unstable TNT not primed on break. Test unstable on break. Not verified.

### G3-F03: Integrated server loop, datapacks, experiments, functions and execute

- (L36) execute: validation of store and facing. Validate at redirection, facing in f32 (exec.rs:584-590, 665-673).
- (L34) Fixed autosave interval. Computation from tickrate.rs, minimum 100 (server.rs:955-958).
- (L27) total_world_time frozen during the singleplayer pause. Increment in the pause loop (stats.rs:255).

### G3-F05: LAN guests (section fluids, relays, rights, leaving) and combat between players

- (L39) Official network checks absent for guests. Speed, pong, chunk flow control, spam (lan/conn.rs:160, encode.rs:843).
- (L38) Guests never candidates for lightning. Include the LAN players (trap.rs:98).
- (L38) Tracking range ignores passengers. Maximum over the passengers (track.rs:57).
- (L40) Arrival message with the raw name. lan/mod.rs

### G3-F12: Scoreboard and bossbar

- (L37) numberformat without format, holder name. cmds_scores.rs:553-566, 39.
- (L37) team join of a member already present. cmds_scores.rs:685-709.
- (L78) Criterion killedByTeam.<colour> not incremented. hooks.rs:183-186. Not verified.

### G4-F02: Monster spawning and finalisation, and variants

- (L52) Structure condition of the variants always false. spawn.rs:475: position within a piece of a structure of the tag
- (L51) Stony peaks: snow villagers. spawn.rs:528: exact list of biomes

### G4-F03: Goal and sensor AI (wolf, breeding, panic, cat, villager)

- (L45) Flee measured from the eyes. goals.rs:764
- (L45) Head tilt of the sitting wolf begging. goals.rs:2309
- (L45) Wolf revenge without exemption tag. goals.rs:1103-1109
- (L50) Spider carrying a passenger not refused in attack. goals.rs:911
- (L45) Following invisible mobs. surface_goals.rs:273
- (L47) Panic of the baby happy ghast on the ground. brainy/brain.rs:1939: aerial position
- (L45) Shared static range of the sensors. villager/brain.rs:628, brainy/brain.rs:965
- (L44) Villager: gaze blind to aquatic categories. villager/behave.rs:560-563
- (L44) Villager: pickup targets the block, border ignored. villager/behave.rs:1256-1258
- (L45) Villager: item sensor (height, box). villager/brain.rs:673
- (L51) Statistic talked to a villager never awarded. Increment in the villager and trader interaction (stats.rs:49 declared)

### G4-F04: Projectiles: impacts

- (L52) Rocket: entry effect of the block hit absent. hits.rs:1108
- (L52) Shulker bullet deflected according to the gaze. bullet.rs: deviation by momentum

### G4-F05: Projectiles: throwing, trajectory and portals

- (L52) Border ignored by the projectile ray and the mount dismount. physics.rs; respawn.rs:92
- (L40) Anchor depletion sound broadcast to everyone. respawn.rs:247: to the player only

### G4-F09: Effects and player (turtle helmet, 1.21 effects, hurt sound)

- (L51) Fall statistic (cm) never awarded; other statistics to sweep. vitals.rs (fall damage), stats.rs:76

### G5-F01: Server block entities: containers, hoppers, dispenser and dropper, composter, pressure plates, copper chests

- (L62) Hopper: fills jukebox and decorated pot without filter. insert_one: filter jukebox_playable and the pot's stack; trigger playback (function provided by G5-F12).
- (L62) Shulker box: neighbours not updated during the animation. containers.rs:691-791: neighbour updates at each animation tick.
- (L43) Container validity distance hard-coded. containers.rs:313
- (L68) Lit target without scheduled tick. Add the case at placement in level.rs:909. Not verified.

### G5-F03: Mobs: player interactions, synced data, generic state (cat, wolf, sheep, name tag, egg)

- (L54) Fish or axolotl bucket: flags lost. fish.rs:496-525, 666-676: save and reload the flags.
- (L54) Initial fire at -1: lightning lights 8 s too many. mob.rs:529 and 1089: fire = 0 by default (check trap.rs:424-435).
- (L48) Lamb eating grass with age locked. brains.rs:422: test the age lock
- (L50) Skeleton's weapon searched only in the main hand. brains.rs:395: also test the off hand
- (L39) Stats for throwing in creative and travelling on a mount. vitals.rs:1359, riding.rs:776.
- (L38) Simplified spawn point. vitals.rs:1285.
- (L38) Spectator: portal and container. vitals.rs:1458, lootchest.rs:147.

### G5-F02: Entity contact, fall and environment (blocks passed through, fire, rain, magma, berries, honey, armour)

- (L54) Mob pushing without team collision rule. body.rs:670-800: same filter as player_env.rs:82.
- (L54) No position readjustment after a size change. At refresh_dimensions, look for the nearest free position.
- (L68) Weaving effect ignored in cobwebs. Read the effect in the entanglement factor (rc-physics lib.rs:2191). Not verified.

### G5-F04: Combat and offensive enchantments (mace, mob thorns, credits)

- (L58) Fire aspect without the burning_time attribute. combat.rs:866-875, 1126: multiply by burning_time (rounded up).
- (L58) Value according to level exponent, exponential effect, approximated remove_binomial absent. ench.rs:183: add these types.
- (L58) No generic enchantment effect engine. Interpret the JSON's effect components with the existing primitives.
- (L58) Enchantment level not capped at 255. enchanting.rs: bound at addition and at merging.

### G5-F05: Orbs, dropped items and entities without AI (displays, interaction, marker)

- (L55) /summon marker sent to clients. cmds_ents.rs:196-224: no AddEntity for minecraft:marker.
- (L58) Durability: armour recognised by the slot and not by the item. drops.rs:931: tag minecraft:enchantable/armor.

### G5-F09: Block rules: growth, neighbourhood, projectiles

- (L64) Burning projectile: does not light the candle cake. hits.rs:1208.
- (L61) Biome zoom absent on the server side (biome_of). nature.rs:945: reuse the zoom of rc-mesh (env.rs: G5-F17).

### G5-F10: Item uses: compass, eye of ender, horn, brush, maps, knowledge book

- (L60) End of cooldown never sent. projectiles/mod.rs:807: Cooldown(group, 0).
- (L62) End gateway: age not kept. portal.rs:820-972: age, block_event 1, NBT Age.

### G5-F14: Pistons: client replay, animation, pushed and stuck entities; block-entity rendering

- (L65) Open Shulker box: suffocates anyway. lib.rs:293 suff_rule.
- (L20) Vault: a single copy of the item. blocks.rs:905: number of copies according to the count.
- (L20) Glowing text outline: distance criterion. block_text.rs:219: same criterion as the official one.
- (L25) Profile skin patch ignored on placed heads. block_entities.rs:811-830: read texture and model of the patch, priority to the imposed texture.

### G5-F15: Client: interactions, passengers, targeting

- (L60) Spyglass: no end-of-use sound. interact.rs:1166.
- (L57+L59) Tooltips: spawn_egg.peaceful, profile.dynamic, item.disabled, op warning. tooltips.rs.

### G5-F16: Client: block-entity rendering, particles and sounds

- (L63+L20) Trial spawner: rotation speed while waiting. block_entities.rs:268, 1059: exact state. [Merge of duplicate G2-F01 "Trial spawner: fast rotation while waiting for players": block_entities.rs:1059: distinguish waiting for players.]
- (L61) F3 line "Biome builder" absent. debug_screen.rs:62.

### G6-F01: Server-click blocks: jukebox, lectern, shelf, decorated pot, campfire, chiseled bookshelf, statue

- (L67) Redstone dust and diodes that no longer hold: removed without loot. Use destruction with loot (redstone.rs:489 and 621). Not verified.

### G4-F07: Leash, leash knot and mounts (saddled pig, trader llamas)

- (L52) Llama spit: margin and exit of the llama. llama.rs:96: shared projectile margin and test

### G4-F10: Allay and parrot dance near a jukebox (server and client)

- (L48) Parrot imitation without silence test. parrot.rs imitate_nearby

### G6-F06: Block placement, survival and shapes (rules per family)

- (L66) Daylight detector: inversion without immediate update. Recompute the signal on click, block_change, flag 2 (placement.rs:231). Not verified.
- (L66) End rod against another rod of the same orientation: no inversion. Test the neighbour (placement.rs:670). Not verified.
- (L66) Climbing plant placed against the same plant: head instead of body. Add the case (placement.rs:866). Not verified.
- (L66) Leaves distance not computed at placement. Compute the distance (placement.rs:536). Not verified.
- (L66) Jigsaw block orientation at placement. Add the case in placement.rs. Not verified.
- (L59) Game master blocks placed without permission. placement.rs:376: same test as the client.
- (L64) Extinguishing a fire by hand: no sound. placement.rs:924: level_event 1009.
- (L59) block_entity_data without type or permission check. loot_be.rs:137.

### G6-F08: Nature ticks and turtle eggs

- (L68) Sugar cane age sent to the client. Flags 260 instead of 258 (nature.rs:648). Not verified.

### G6-F11: Sleeping and beds (straw bed, waking, announcements), waxed signs, multiplayer spawns, dragon fight

- (L69) Exit portal search without the End gateways. Add minecraft:end_gateway (fight.rs:402, 439). Not verified. fight.rs is also touched by G1-F02: go after it.
- (L69) Explosions of the pillar summoning without source. The crystal as source (fight.rs:926). Not verified.
- (L70) global_sound_events rule ignored. Read the rule (wither/boss.rs:157, portal.rs:1069, dragon). Not verified.
- (L70) spectators_generate_chunks rule ignored. No generation of new chunks around a spectator if the rule is false. Not verified; chunk loading file to locate.
- (L67) Respawn anchor and sniffer egg: missing game event. Emit the two events (respawn.rs:257-317, brainy/eggs.rs). Not verified.

### G6-F12: Block-entity tickers, conduit, containers

- (L66) Ender chest openable under a conductor block. Take the chest test (containers.rs:177) over to line 207. Not verified.

### G6-F18: Generation: ticks placed by decoration elements

- (L72) Post-processing marking absent (discs, lakes, fallen trunks, lichen). Call r.mark_post_process on the column above, or on the position for lichen. Not verified.

### G6-F14: Technical blocks in singleplayer (structure, jigsaw, test), templates, /place, /clone, /locate, border

- (L75) World templates folder: structures/ instead of structure/. cmds_s9d.rs:161-163. Not verified.
- (L75) linear_pos position test not handled. PosPred case in Manhattan distance (structure_place.rs:119). Not verified.
- (L75) append_static without deep merge. Recursive merge of compounds (structure_place.rs:309-318). Not verified.
- (L36) /forceload: bound and sort. cmds_world.rs:338, 356-361.
- (L37) worldborder add with duration. Add the remaining duration (cmds_world.rs:461).
- (L36) /place feature: loading and randomness. 3x3 area, world randomness (cmds_s9d.rs:107).

### G1-F07: Advancement and loot predicates

- (L04) stepping_on and movement_affected_by: wrong block. pred.rs:721/756: player_on_pos (mobs/recent/mod.rs:999) and mob equivalent, offset 1e-5.
- (L04) looking_at without line of sight or exclusion of spectators. pred.rs:1058/1024: eye-target block ray, exclude spectators.
- (L04) raider: has_raid never true, captain approximated. pred.rs:905-911, loot.rs:298-299: raid state and patrol leader; default false.
- (L04) Player flags is_swimming/is_fall_flying/is_flying wrong. pred.rs:669-680/801-804: mobs.rec.player_swimming and fall_flying state.
- (L04) contains: one element removes only one test. pred.rs:358-366: retain of all the satisfied tests.
- (L04) Entity NBT compared with id. pred.rs:598: remove id before comparison.
- (L04) entity predicates: presence only. pred.rs:868-871: partial evaluation on the value.
- (L04) movement: position delta instead of velocity. pred.rs:707: mob's velocity.
- (L04) empty fishing_hook always false. pred.rs:912: true if in_open_water absent.

### G1-F09: Text input, IME, chat and widgets

- (L10) No hand on hovering clickable text. links.rs: request_cursor(hand) if click_event.
- (L11) Colour of field hints. widgets.rs:1086 -> 0xFF555555 (style of the priority hint); pack_screen.rs:271-274 -> 0xAAAAAA.
- (L11) Grey selection outline even with focus. widgets.rs:1185-1189, pack_screen.rs:476-480: white if focus.
- (L12) Tab and arrows: wrap-around and spatial fallback. widgets.rs:616-629/654-667: tab wraps around, arrow without focus starts from the opposite edge, distance between the midpoints of the sides.
- (L11) Icon buttons without tooltip (widgets part). widgets.rs:944: tooltip specific to Kind::Icon (front.rs on the F15 side, menus.rs on the F17 side).
- (L14) Sign closing distance. Interaction range + 4, distance from the eye to the AABB.

### G1-F11: Item tints (firework, custom_model_data, team)

- (L09) team tint ignores the wearer's team colour. item.rs:1150: team colour in EvalEnv (the caller on the rendering side will supply it, F08).

### G1-F12: HUD, bundle and spectator

- (L10) Bundle selection not reset to -1 on QUICK_MOVE/SWAP. menu.rs:480/571: deselection from bundle.rs:517-529 before sending.
- (L10) Camera overlay read from the default component. hud_more.rs:905-915: camera_overlay of the equipped stack's components.

### G1-F14: World screens and statistics

- (L11) Alt list of the world type absent. create_world.rs:99/738-739: extended presets (including debug_all_block_states) if Alt.
- (L13) Flat world layers at y=43. create_world.rs:1367: top 33, height h-33-64.
- (L13) Double click on a flat preset applies and closes. create_world.rs:1549: simple selection.
- (L13) Single biome: header 1 px too high, sort without collation. create_world.rs:1741/1668 positions 7/19; 1650 key without diacritics or case.
- (L13) Enchanting book 1.5 px too high. special.rs:127: centre = top + 17.
- (L13) Statistics: powder_snow without a row. stats_screen.rs:34-74: extracted block->item table or powder_snow_bucket exception.

### G1-F15: Singleplayer loading, title screen, symbolic links

- (L11) Language/Accessibility tooltips of the title screen (front part). front.rs:885-897: options.language.tooltip / options.accessibility.tooltip (Kind::Icon support by F09).
- (L11) Logo subjected to the fade after the accessibility onboarding. front.rs:906-925: keep_logo flag, logo at alpha 1.
- (L13+L39+L77) Symbolic link warning absent. world_list.rs and pack_stack.rs: detect, notice screen with help link and Back. [Merge of duplicate G3-F18 "Permissive format validation and symbolic links": packs.rs:77, 150; server_packs.rs:308.] [Merge of duplicate G6-F15 "No check of the worlds' symbolic links": Compare with allowed_symlinks.txt at loading (storage.rs). Not verified.]

### G1-F10: Main loop: pause, draw order, bed, crashes, toasts

- (L10) Subtitles under the in-game screens. main.rs:1373-1381: after screens.draw for an in-game screen, without hidden test.
- (L10) Save indicator badly ordered and hidden by F1. main.rs:1314-1321: after screens.draw, without hidden test, real partial tick.
- (L06) Window title without LAN variant. main.rs:1874: title.multiplayer.lan if the world is published.
- (L06) First server's Social Interactions toast absent. options.rs:175 (read); toast via toasts.rs and tutorial.rs.
- (L06) Periodic play-time reminders absent. toasts.rs: read regional_compliancies.json, toasts according to the locale's country.
- (L06+L18) Demo messages 101-104 with hard-coded keys. bed.rs:296-300: labels of the rebound keys. [Merge of duplicate G2-F11 "Demo keys hard-coded": bed.rs:296-300: translated key names.]
- (L01) F3+S without action. debug_keys.rs:223/360: dump the GPU atlases per mip level as PNG in screenshots/debug, translated message with link.

### G1-F16: F3 and debugging

- (L01) Sounds line: wrong counters. engine.rs:337 exposes static/stream and maximums; debug_screen.rs:1151.
- (L03) Display: vendor names and API version. debug_screen.rs:838-848 official table (fallback 0x..); 958-962 API version.
- (L12) Server F3 lines absent in singleplayer (SH, FC, sprint, local difficulty). debug_report.rs supplies the values; debug_screen.rs:1157/1129/1055/928-940.
- (L12) T: shows maxFps instead of the effective limit. debug_screen.rs:913-920: limiter's limit.
- (L12) Targeted fluid treated as a full block. debug_screen.rs:1201: fluid height from rc-physics.
- (L13) Profiler pie chart 5 px too low. debug_charts.rs:207: offset -5.
- (L03) No list of old Intel GPUs excluded from Vulkan. gpu_backend.rs:144-185: vendor 0x8086 + Windows driver + list -> DX12 first.

### G2-F06: First-person hand, leash, elytra and End sounds

- (L23) Exactly vertical leash drawn (quirk). leash.rs:181: draw nothing if dx and dz are zero.
- (L25) End flash despite "hide lightning flashes". environ.rs:317: condition on the option.

### G2-F08: Singleplayer world: world pack, creation, world list, packs

- (L16) Deletion of the last world; two-button recreation error. world_list.rs:686-687/1339-1345.
- (L16) Failed backup copy: edit screen stays. world_list.rs:624-628: return to the list.
- (L16) Low disk space and copy of an experimental world absent. Two confirmations in the opening flow.
- (L16) Details button of experimental features absent. create_world.rs:820-826.
- (L16) Bonus chest greyed out in hardcore. create_world.rs:907.
- (L16) World type changed: flat layers and biome kept. create_world.rs:737-742: reset.
- (L15) File drop without pack detection. pack_screen.rs:663-673: validate pack.mcmeta.
- (L15) Truncation without "...". pack_screen.rs:537-542.
- (L26) Pack format header without minor. server_packs.rs:187: major.minor.

### G2-F11: Inventory screens, recipe book and options menus

- (L15) Recipe book: thumbnails, arrows, bounce, en_pt egg. recipe_book.rs:1122/1052/972.
- (L15) Social interactions: All tab and text shadow. world_options.rs:916-922/1249-1254.
- (L15) Options: accessibility guide, Ctrl+wheel, language hint, key conflicts. menus.rs:1724/1140/1651/1630.
- (L14) Stonecutter and loom: scrolling and areas. screens_work.rs:336/1339/1431-1439.
- (L14) Merchant and creative: scrollbar and wheel. screens_trade.rs:295/391; creative_screen.rs:241/512.
- (L16) Spectator menu: sort by UUID and activated player. spectator.rs:62/236.

### G2-F12: Chat, blocked servers, block models (bake) and client data

- (L18) Edited message split across the full width. chat.rs:869: -15 px with icon.
- (L26) Vertices not recanonicalised after variant rotation. bake.rs:446-490.
- (L20) Multipart condition !a|b. bake.rs:318: negation value by value.
- (L21) Client heightmaps never updated (known). rc-world/src/lib.rs: update at placement and breaking, rule of rc-sim.
- (L21) F3 visualisations of occlusion and server chunks (known). debug_render.rs from rc-mesh visibility; chunk state exposed by rc-sim.

### G3-F06: Gametest (/test, test instances) and admin broadcast of commands

- (L27) No admin broadcast of commands. broadcast flag in ok() (mod.rs:594), chat.type.admin according to sendCommandFeedback.

### G3-F07: Basic commands and world creation

- (L36) /playsound: registry identifier instead of a direct sound. Encode directly without fixed range (cmds_basic.rs:1084-1089).
- (L37) /teammsg: recipients, key, return. Filter by team (cmds_basic.rs:196).
- (L37) /version: empty fields, subject to send_command_feedback. Values from rc-data, system message (cmds_basic.rs:306-308).
- (L34) Position and dimension of the Server source. Without +0.5, dimension of the spawn point (src.rs:93-107).
- (L34) Debug world settings not applied. peaceful locked, spectator, noon, advance_weather=false (newworld.rs:176-181).
- (L38) Demo mode parsed but never applied. Demo mode in rc-sim (events 101-104, end blocking), --demo option (launch.rs:206).

### G3-F08: Commands: compute, fetchprofile, posteffect, attributes, loot, enchant

- (L36) /loot insert/replace. Counter, can_place_item, entity slots (cmds_s9b.rs:650-705).
- (L36) /enchant limited to the player. Mob equipment (cmds_ents.rs:463).
- (L36) /ride: mount.failure.generic without arguments. Two names as arguments (cmds_s9a.rs:483-519).
- (L36) /datapack create: rich description, reserved names. cmds_s9a.rs:1008-1011.
- (L36) return run debug function not refused. cmds_ents.rs:1018.
- (L37) /spectate: untracked entities accepted. Tracking radius of the type (cmds_s9a.rs:527).

### G3-F10: NBT, SNBT and /data

- (L28) NBT paths: typed arrays ignored. step, path_remove, path_insert (cmds_data.rs:109-119, 279-307).
- (L28) data modify set: edge cases and messages. cmds_data.rs path_set, 314, 593.
- (L31) NBT string > 65535 bytes. Length 0 (nbt.rs:242).
- (L31) SNBT: bounds, typed arrays, empty key, \N{}. snbt.rs:96-333.
- (L31) SNBT bool(). snbt.rs:196.
- (L31) to_snbt: key sort and quotes. snbt.rs:384, 410.
- (L31) Heterogeneous NBT list not unwrapped. snbt.rs:237, nbt.rs:136, 260.
- (L36) execute items entity: mob equipment slots. cmds_data.rs:557.
- (L35) data merge entity changes the UUID. Keep m.uuid (ents.rs:225-245).

### G3-F11: Arguments, selectors, predicates and text and item components

- (L28) Item component values not validated. Validate against the codecs, arguments.item.component.malformed (args.rs:290-316).
- (L29) attribute_modifiers predicate not evaluated. pred.rs:351.
- (L28) Player name case-sensitive. eq_ignore_ascii_case (select.rs:451, args.rs:394).
- (L28) Selector error keys empty. args.rs:390-409.
- (L31) NBT comparison: expected list longer. select.rs:331.
- (L32) score component: number format, edge cases. args.rs:519-533.
- (L32) selector component: separator ignored. args.rs:512-518.
- (L29) damage predicate true without component. pred.rs:249-251.
- (L29) Book and firework predicates incomplete. pred.rs:319-348.

### G3-F13: World tick order and chunks

- (L38) global_sound_events and spectators_generate_chunks rules never read. Read at the relevant places.

### G4-F08: Monsters and entities: creeper, shulker, TNT, sulfur cube, creaking, warden

- (L50) Burning cube: contact damage limited to the player, message absent. cubes.rs:1333
- (L50) Warden: incomplete target test; anger of the removed suspect. warden.rs can_target, anger_tick
- (L50) Breeze: lava cauldron not dangerous. breeze.rs
- (L50) Evoker: placement event and vex team. spells.rs
- (L50) Creaking: copper bars opaque to targeting. creaking.rs visual_boxes
- (L45) Bat: ceiling and sound 1025. bat.rs:21
- (L45) Spider navigation: target memorised too late. nav.rs:1159
- (L44) Body rotation of a mob carrying a mob. tick.rs:703
- (L48) Game events of sniffer, turtle, rabbit; sniffer border. sniffer.rs:166, turtle.rs:99-106, rabbit.rs:242-244

### G4-F12: Iron and copper golems

- (L45) Flower offered without line of sight. iron.rs:395
- (L41) Spawn: border and bubble column. iron.rs:480, recent/mod.rs:789

### G4-F13: Piglins and spear

- (L50) Shield tested by identifier. behave.rs:815

### G4-F14: Integrated server: pack tags, /reload, clocks

- (L43) End clock absent. server.rs:300
- (L43) time set and tick fraction. commands/mod.rs:706, server.rs:569

### G4-F17: Client: trading, input, tooltips, profiling and miscellaneous

- (L41) C1 characters refused on input. rc-text filter on each field cited
- (L41) Tooltip effect duration over 20. tooltips.rs:51: tickrate
- (L41) Mouse smoothing: sign of zero. keys.rs:92
- (L41) Named timeline curves to linear. rc-env prepare.rs:590
- (L41) World folder name: counter and COM. storage.rs:32, 40
- (L43) Partial world optimisation. storage.rs:288, edit_world.rs:237-242
- (L42) Incomplete profiling archive. profiling_zip.rs:167-176
- (L42) profiling.txt without counters. profiler.rs, profiling_zip.rs:44

### G5-F17: Spawns and spawn eggs

- (L61) Starting block: full and opaque instead of conductor. spawn.rs:779, bat.rs:21.
- (L61) Per-player mob cap absent. spawn.rs:745.
- (L61) Cage: never a non-mob entity. spawn.rs:1238.
- (L55) No check of the world border. spawn.rs:937-967.
- (L61) ambient_light hard-coded; biome zoom absent. env.rs:150-155, 182.
- (L61) Explosion knockback without explosion_knockback_resistance. explode.rs:212.
- (L63) Trial spawner: player height fixed at 1.8. trial.rs.

### G6-F15: Saving, /schedule, /data, world list

- (L77) /schedule append stacks a duplicate. Do not push if the same name and the same tick already exist (exec.rs:1131-1136). Not verified.
- (L77) schedule clear from an expired function does not cancel the other deadlines of the same tick. Pop one deadline per turn (exec.rs:1148-1159). Not verified.
- (L76) Empty compound that does not remove the id from storage. Remove the id instead of inserting it (cmds_data.rs:507, exec.rs:426).
- (L76) level.dat: version and brand not rewritten. Write Version, DataVersion, version and ServerBrands (save.rs:205-222). Not verified.
- (L76) World with only level.dat_old absent from the list; restoration that overwrites the corrupted file. Accept these folders (world_list.rs:334); rename level.dat to level.dat_corrupted_<date> before the restoration (storage.rs:119-129). Not verified.
- (L78) Order of ticks reloaded from the save. Sub-orders -n..-1 in the order of the list (ticks.rs:385). Not verified.

### G6-F17: Loot: functions, providers, entries

- (L77) Providers score, storage, enchantment_level and environment_attribute not evaluated. loot.rs:604-608, 712. Not verified.
- (L77) value_check: value read as a constant. Same evaluator as loot.rs (pred.rs:1267-1274). Not verified.
- (L77) Nested table without list, tag or expand; slots entry absent. loot.rs:995, 896-923. Not verified.
- (L77) set_components without '!x' removal; copy_components only from the block entity. loot.rs:836-841, items.rs:77. Not verified.
- (L77) Arithmetic differences mod, ceil, division, binomial. loot.rs:679, 691, 567-570, 522-526. Not verified.
- (L77) Modifiers of a composite entry not applied to the children. Propagate conditions and functions at expansion (loot.rs:1035, 896-923). Not verified.
- (L76) Nested tables: depth 8 instead of a re-entry refusal. Stack of visited tables in run_raw (loot.rs:995). Not verified.

### G6-F19: Structures: saving of pieces and placed mobs

- (L73) Jigsaw piece: empty junctions, liquid_settings not written. structure_nbt.rs:127. Not verified.
- (L73) Placement at the dimension origin fixed at (0,0), rings without exclusion zone. structure.rs:364-374, 521. Not verified.
- (L74) Number of allays by a home-made hash. an integer draw over 3+1 on the region's randomness, at the same moment (structure_templ.rs:737, 955). Not verified.
- (L71) Bounds validations of the density functions not reproduced. Bounds ±1e6 and lower_bound range in parse_df (density.rs:305, 402-420, 435-442). Not verified.

### G6-F20: Navigation and maps

- (L76) Starting node on a fluid one stands on absent. "Stands on this fluid" branch (nav.rs:597-615).
- (L76) Spinning rotation of the Nether not applied to static banners and decorations. Pass the dimension (maps.rs:476, 560). Not verified.
- (L76) Banner at the same position but with another name: duplicate. Compare by id (maps.rs:471-478). Not verified.
- (L76) deco_tracks forgets 5 types. maps.rs:118-135. Not verified.

### A2-E2: Redstone (investigation): order of sending block changes, bench scene, client queue

- (redstone investigation) perf:redstone bench: the phase shift of one or two rows is faithful to the game (grass under an opaque block turns into dirt at the random tick and wakes up the piston placed on it). Nothing to fix in rc-sim (nature.rs:762-766 and piston.rs:39-63 identical to the game; tick queues, observer and push resolution compared with no difference). Bench side: place the scene on stone (tools/bench/scenes.py:181-194) or add gamerule random_tick_speed 0 at the head of perf:redstone (changes the measured load). Add a clock + sticky piston + slime scenario on a pushable floor (only observer.txt exists). Control: on both sides, random_tick_speed 0, tick freeze, function perf:redstone, then tick step 1 and save-all flush at each step: no more phase shift expected.

### A2-E3: Meshing and occlusion (investigation): loaded unmeshed columns, light queue, walk distance

- (meshing investigation) Rule of the 8 ready neighbours: no light activation queue, a column can be meshed a few frames earlier than in the game (pace of appearance only, final image identical). Already listed as known and minor (docs/perf.md:128-131). On our side the light arrives with the blocks (apps/client/src/main.rs:800-819) and surrounded() only tests the presence of the neighbours (remesh.rs:182-211). Recommendation: note the difference as accepted in the list of settled points (decision to take). Otherwise: light_pending set and column queue in remesh.rs; on reception, put the column in the queue without touching the neighbours; once per frame and outside pause, activate max(10, n/10) columns if n < 1000, all otherwise, then touch the neighbours; surrounded() requires activated neighbours. Test in remesh.rs: 9 columns activated at the first drain; 30 waiting come out by 10; the centre column is only dirty after the activation of its last neighbour.

### G1-F17: Options and menus

- (L06) darknessEffectScale linear. menus.rs:504: Sk::Square.
- (L06) OFF missing at 0 (darkness, glints, damage tilt). menus.rs:677-682: add the four keys.
- (L06) Brightness label. menus.rs:656-658: truncated integer, Default if 50.
- (L06) Chunk fade without two decimals. menus.rs:652.
- (L11) Section titles 2 px too high. menus.rs:1699-1704: 2 + top + 2 + margin.
- (L11) Friends button tooltip (menus part). menus.rs:1505: gui.friends.open.tooltip.
- (L06) japaneseGlyphVariants without a default according to the language. options.rs:85: default according to the system locale.
- (L06) Near plane of the fluid-camera test at 70°. environ.rs:367: effective FOV.

### G1-F18: Pack core shaders (after optimisation, to be decided)

- (L04) Core shaders of resource packs ignored. User decision: translate GLSL->WGSL (naga) with internal fallback, or accept the difference; linked to the video menu and shader packs.

### G6-F21: Client: targeting, particles, options, meshing

- (L67) Lever: no dust particle on click. Add the particle in predict_hand_sound (interact.rs). Not verified.
- (L70) max_minecart_speed listed without the minecart_improvements flag. Hide the entry (world_options.rs:253, 1504). Not verified.
- (L78) Removal of hidden faces without the union of the neighbour's boxes. 16x16 grid test like rc-light face_masks (rc-mesh blocks.rs:125-151). Not verified.
