# Unhandled server → client packets (survey of 12/10/2026, updated in the evening)

Starting point is an automatic survey: the packet list of the official report (`reports/packets.json`,
26.3) minus the ones `rc-client` recognised. Play: 71 of 144; configuration: 13 of 21.
Sorted by consequence for an exact replacement of the client.

## A. Game or connection correctness — DONE (`protocol-supplements.md`)

Batches, tick rate and freeze, explosions, forced rotation and camera, vehicle (receiving and sending;
mount physics: mission in progress, `mounts.md`), cookies, resource packs (replies;
download not done: refusal or disconnection if required), transfer, server dialogs (simple
screen), features, biomes, simulation distance, difficulty, game rules, brand
(`vanilla`), ping. Not tested in game: cookies, packs (the test server does not send any).

## B. Visible in game

- DONE: block entities (`block_entity_data`, `block_event`), interactions (`block_changed_ack`,
  `block_destruction`, `cooldown`, `animate`…), border (`initialize_border`, `set_border_*`),
  `waypoint`.
- In progress: `map_item_data` (maps), `merchant_offers`, `mount_screen_open`, `open_book`,
  `open_sign_editor` (special screens).
- DONE (`miscellaneous.md`, `rc-client/src/misc.rs`): `set_entity_link` (leashes),
  `take_item_entity`, `player_combat_enter`/`end` (ignored), `post_effects`, `projectile_power`,
  `add_transient_block`, `move_minecart_along_track` (not tested: experimental mode).

## C. Screens and comfort

- In progress: `commands`, `command_suggestions`, `custom_chat_completions`, `delete_chat`
  (chat commands); `recipe_book_add`/`remove`/`settings`, `place_ghost_recipe`
  (creative inventory and recipe book).
- Spec done (`advancements-stats.md`), implementation to come: `update_advancements`,
  `select_advancements_tab`, `award_stats`.
- DONE: `server_data` (icon written to `servers.dat`), `low_disk_space_warning` (simple
  notification, not tested: dedicated server only).
- Remaining: `update_recipes` (stonecutter).

## D. Debugging and tests (to ignore without error)

`debug/*`, `debug_sample` (F3 tick graph: done), `tag_query`, `game_test_highlight_pos`
(done: `/test pos` marker, `special-screens.md` § 6.3), `test_instance_block_status`.

## E. Online connection — DONE (`authentication.md`)

Account provided by the launcher (arguments of the official game, including the Java line of the
official launcher), encryption, `join`, chat keys and signatures. Not tested with a real token.

## F. Pistons: entities not pushed yet (G5-F14, 22/10/2026)

The game pushes every entity whose piston reaction is "push" (the default). Rustcraft pushes
mobs, items, orbs, primed TNT and falling blocks on the server side; on the client side the local player
and the entities it simulates (items, orbs, TNT, falling blocks). Missing:

- projectiles (arrows, tridents, fireballs…) and vehicles (boats, minecarts), server and client;
- player on the server side (its client pushes it and sends back its position, like the game; the server does not
  replay the push for the movement check);
- interpolated entities on the client side (mobs, other players): the game adds the push to the
  prediction of their interpolation;
- effects of the blocks crossed by a pushed mob (cobweb, berries, magma): to wire up with
  the mobs' block effects (G5-F02);
- footstep sounds while the local player is pushed (the game emits them in the piston movement).
