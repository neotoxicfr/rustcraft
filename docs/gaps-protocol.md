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
mobs, minecarts, items, orbs, primed TNT, falling blocks, armor stands (markers ignored, a
weightless stand placed without collisions or cap) and mannequins on the server side, each with its
own cap of 0.51 block per axis and per tick (L10-ENT); what hangs on a block (frame, painting,
knot, cushion) is knocked off with its item by the first non-zero push, a fixed frame excepted. On
the client side the local player and the entities it simulates (items, orbs, TNT, falling blocks).
A pushed minecart applies its block effects twice like the game's (its own move, then the
piston's pass), both read at its final box instead of the path crossed. Missing:

- projectiles (arrows, tridents, fireballs…), server and client; boats on the client side (the
  integrated server pushes its boats like its minecarts since L16-BATEAUX);
- player on the server side (its client pushes it and sends back its position, like the game; the server does not
  replay the push for the movement check);
- interpolated entities on the client side (mobs, other players): the game adds the push to the
  prediction of their interpolation;
- effects of the blocks crossed by a pushed mob (cobweb, berries, magma): to wire up with
  the mobs' block effects (G5-F02);
- footstep sounds while the local player is pushed (the game emits them in the piston movement);
- interpolated armor stands, mannequins and minecarts on the client side (same as the mobs: the
  server's tracking carries their push).

The push of an opening shulker (G4-F08) moves the same entities, with an ordinary movement
(slowdown of the tick applied, no caution at the edge), minecarts included (their own move, block
effects after it; never the one the shulker rides, L10-ENT), and the decorations: what hangs falls
with its item (fixed frame excepted), armor stands and mannequins move with their collisions
(markers and weightless stands have no physics). Same missing cases (projectiles, boats on the
client side, player on the server side, interpolated entities on the client side). The server does not model the scale
attribute of the shulker (size 1 there; the client scales its push). Every armor stand and mannequin
runs its tracking at each tick, still ones included (L10-ENT): a pushed immovable mannequin reaches
the client.

## G. Vehicles and mob pushes (G4-F08, 02/11/2026)

A mob pushes the minecarts it touches (the minecart's own rule: it moves aside, the mob gets a
quarter; never its own rider), they count for the cramming, and a burning sulfur cube hurts them
(the one it rides included), which destroys one in about 5 ticks. A boat follows its own rule (only
a mob whose feet are not above the boat's bottom pushes it, then the general push between two
entities; a mob that touches a boat no player pilots boards it when a seat is free). The cramming draw uses every
pushed entity, the damage only those that ride nothing, like the game (L10-ENT).

`PortalCooldown`: read back and written for minecarts, primed TNT, falling blocks, mobs,
projectiles, dropped items and experience orbs. Items and orbs carry the game's common entity part
(`fall_distance`, `Fire`, `Air`, `OnGround`, `Invulnerable`, `PortalCooldown`), in its order, in the
save and in `/data` alike (one writer), read back at load, summon and `/data` (one reader); an
`Invulnerable` one only takes the damage of `bypasses_invulnerability` (L10-ENT). Still missing for
them: their UUID (derived from the identifier, never the world's), the optional fields
(`CustomName`, `Tags`, `data`, `Silent`, `NoGravity`, `Glowing`, `TicksFrozen`, `HasVisualFire`,
`invulnerable_time`, the item's `Owner`), the orb's random yaw, and a creative player's damage
through the invulnerability (no source carried down to them).
