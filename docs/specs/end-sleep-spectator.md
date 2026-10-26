# End of the game, sleep, spectator, spyglass (client 26.3)

Behaviour of the official client, described in our own words. Code: `win_screen.rs`, `bed.rs`,
`spectator.rs`, `camera.rs` (`UseFov`), `hud_more.rs` (fade, scope), `overlays.rs` (scope).

## 0. Game events (`game_event`)

Floating-point parameter `p`, integer `n = floor(p + 0.5)`.

| Type | Effect |
|---|---|
| 0 | System message `block.minecraft.spawn.not_valid` in the chat. |
| 1, 2, 7, 8 | Rain and thunderstorm (`rc-env`). |
| 3 | Local game mode = `n`. |
| 4 | End screen (§ 1); when it closes, `client_command` 0 (respawn). |
| 5 | Demo: 0 = welcome window (not done), 101–104 = hints in the chat (default keys). |
| 6 | Sound `entity.arrow.hit_player`, players category, volume 0.18, pitch 0.45, at the eyes. |
| 9 | Sound `entity.puffer_fish.sting`, neutral category, 1 / 1, at the feet. |
| 10 | Elder guardian appearance particle (not rendered); if `n == 1`, sound `entity.elder_guardian.curse` (hostile, 1 / 1). |
| 11 | Immediate respawn: death screen if `p == 0`. |
| 12 | Limited crafting (`p == 1`): remembered, no visible effect on the client. |
| 13 | Start of chunk loading (`flow.rs`). |

The server only sends event 4 the first time the player goes through the exit portal ("credits
seen" flag of the player); afterwards it brings the player straight back, and the client has nothing to do.

## 1. End screen

- Lines: poem (`texts/end.txt`), credits (`texts/credits.json`), post-credits
  (`texts/postcredits.txt`); poem and post-credits only when the screen comes from the End
  (always the case for event 4).
- Poem: each line of the file, `PLAYERNAME` replaced by the username; each `§f§k§a§b` token
  replaced by `§f§k` + 3 to 6 "X" (random draw `next_int(4) + 3` from a Java generator seeded
  8124371, one draw per token in order); line wrapped at 256 px, then an empty
  line; an empty line in the file yields only that empty line. 8 empty lines at the end of the
  file.
- Credits, per section: white centered `============`, yellow centered section name,
  `============`, 2 empty lines; per non-empty discipline: yellow centered name, 2 empty lines; per title:
  gray title (left-aligned), each white name preceded by 11 spaces, then 2 empty lines.
- Drawing: "End portal" background (15 layers, screen coordinates); vignette
  `misc/credits_vignette` in a darkening blend; logo + edition at `y = H + 50`, lines from
  `y + 100`, column `x = L/2 − 128`, line spacing 12, text with shadow; everything
  translated by `−scroll`. The last line stops at `H/2 − 6`. A line is drawn if
  `y' + 20 > 0` and `y' < H`.
- Scrolling: `+= Δticks × speed`, speed 0.5 (0.75 without the poem); Space held ×5,
  plus 15 per Ctrl key (left, right) held at the same time; up arrow held:
  reverse direction; never negative. Ends when scroll > `12 × lines + 2H + 24`, or Escape.
- Music `music.credits` (replaces the current music), stopped on close.

## 2. Sleep

- The local player sleeps as long as its entity data 14 (bed) is present.
- When that data arrives, the client places its feet at the center of the bed, at
  `y + height of the bed shape + 0.125`. Lying down: motionless, camera (first person) at the
  eye height of the lying pose (0.2) raised by 0.15, turned to `bed orientation − 180°`,
  pitch 0; hand hidden.
- Sleep counter: +1 per tick lying down (max 100); out of bed: +1 up to 110 then 0.
  Entity animation 0 (wake up) sets it to 100.
- Fade: flat `0x101020` of opacity `220 × c/100` (beyond 100: `1 − (c − 100)/10`),
  drawn even when the HUD is hidden, after the effects and the boss bars.
- Chat in bed: opened automatically when no screen is open; "Leave Bed" button
  200 × 20 at `(L/2 − 100, H − 40)`; Escape and the button send
  `player_command` 0 (get up) without closing. On waking: if the typed text is empty, the chat
  closes, otherwise it stays open as a normal chat.
- "You can only sleep at night" and other refusals: server action message.

## 3. Spectator menu

- Keys 1–9: menu closed → opens it (root category); open → picks the slot: first
  press = selection, second press on the same active slot = action.
- Middle button: opens the menu, or acts on the selected slot.
- Wheel, menu open: next active slot (opposite direction of the wheel); menu closed:
  flying speed ± 0.005 per notch, clamped to [0, 0.2] (local).
- Pages: slot 0 = previous page (pages ≥ 1), 7 = next page (active if entries
  remain), 8 = close; entry of slot `s` = `s + 6 × page`.
- Root: "teleport to a player" (listed players excluding spectators, sorted by UUID,
  halves compared as signed integers); "to a team member" (teams with a member
  known and not a spectator; icon: face of a randomly drawn member on a background of the team
  color). Lists frozen when the menu opens.
- Chosen player: `teleport_to_entity` (UUID).
- Drawing: `hud/hotbar` bar at `y = H − 22 × α`, selection, 16 × 16 icons (×0.25 if
  inactive), 12 × 12 faces at (2, 2), key number on the right; action text at `H − 35`
  (slot name, otherwise the category prompt), background if "text background everywhere".
  α = `clamp((5000 − ms since the last choice) / 2000, 0, 1)`; at 0 the menu closes.
- Player outlines (key "highlight players", no default key):
  not done (no glowing outline pass in the engine).

## 4. Spyglass and bow

- Target field-of-view factor: base (flying, speed); bow in use: `× (1 − min(t/20, 1)² × 0.15)`
  then effects scale; spyglass in use and first person: 0.1 directly.
- Scope: `misc/spyglass_scope` square of side `floor(min(L, H) × scale)` centered, black
  bands around; scale moved closer to 1.125 by `0.5 × Δticks` of the gap per frame, reset to
  0.5 when not in use; replaces the overlays of equipped items. Hand hidden.
- Mouse sensitivity divided by 8 while aiming.
- Sounds `item.spyglass.use` at the start (already done), `item.spyglass.stop_using` at the end (server).
