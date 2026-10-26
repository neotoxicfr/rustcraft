# Rebindable keys and input — behaviour specification

Code: `apps/client/src/keys.rs`, `screenshot.rs`; settings in `options.rs` (`KEYS`).

## 1. Bindings

### 1.1 Table

Each action has a key read from `options.txt` (`key_<action>:<name>`), keyboard
(`key.keyboard.*`) or mouse (`key.mouse.left|right|middle`, then `key.mouse.4`, `.5`… for the
side buttons). `key.keyboard.unknown` = action without a key. The keys screen writes into
the settings; the table is re-read every tick (hot reload).

### 1.2 Press, repeat, release

- One key can serve several actions (conflict shown in red in the screen): on
  press, **all** of them go to the pressed state and each fires once.
- In game (no screen), the press AND every keyboard repeat set the action to "pressed"
  and fire it (holding "drop" drops continuously). Screens do not receive
  repeats.
- Releasing sets the action back to "released".
- Toggles (settings `toggleCrouch`, `toggleSprint`, `toggleAttack`, `toggleUse`): for
  sneak, sprint, attack and use, each press (repeats included) flips the state;
  releasing changes nothing.

### 1.3 Screens

- Opening a screen (or releasing the mouse) releases all actions. A toggle active at that
  moment remembers it.
- Returning to the game: the sneak, sprint and attack toggles bound to the keyboard that
  remembered it become active again (not "use"); simple actions bound to the keyboard
  take back the real state of their key (on Windows and Linux).
- Respawn: all toggles are reset.

### 1.4 In-game triggers

Perspective, cinematic camera, hide the interface, spectator effects, slots 1–9 (in
creative, with "load toolbar" or "save toolbar" held: load or
save), inventory (or the mount's inventory, requested from the server), quick actions
(dialog of the `quick_actions` tag if it is alone, otherwise the `quick_actions` dialog), swap
hands, drop (Ctrl: whole stack), attack, use, pick block, chat, command
(prefilled with `/`). The player list is shown while its key is held. Escape stays
fixed (pause menu).

### 1.5 Container screens

Escape or the inventory key close; the "swap hands" key targets the
off-hand (slot 40); the keys of slots 1–9 swap; "drop" drops the hovered slot
(Ctrl: whole stack). Same in the creative inventory outside search; the chat
key opens the search tab there.

## 2. Global keys

On press (not on repeats), before the screens, unless the debug modifier (F3)
is held:

- **Fullscreen** (F11): flips the `fullscreen` setting, saves it and applies it.
- **Screenshot** (F2): image of the window (interface included) as an opaque PNG in
  `<game folder>/screenshots/yyyy-MM-dd_HH.mm.ss.png` (local time; `_2`, `_3`… if the name
  exists), written off the render thread. Message in the chat: "screenshot.success" with
  the underlined file name, whose click opens the file; on failure
  "screenshot.failure" with the error.

Window close: refused if the `quitShortcuts` setting is disabled and it comes
from Alt+F4 (Alt held and F4 pressed).

## 3. Mouse

- **Rotation** (once per frame): s = sensitivity × 0.6 + 0.2; factor = s³ × 8 (s³ alone when
  aiming a spyglass in first person). The movement accumulated since the previous frame ×
  factor, inverted per axis according to `invertXMouse` / `invertYMouse`, then × 0.15° (pitch clamped to
  ±90°).
- **Cinematic camera** (key without a default, not saved): per axis, a smoother keeps an
  accumulated target, a part already travelled and the last velocity. Each frame: target +=
  movement × factor; gap = target − travelled; new velocity = average of the old one and
  of the gap; if the gap exceeds (in absolute value, same sign) the old velocity, the
  new velocity is taken instead; rotation = gap × frame duration × factor, added to the
  travelled part. Outside cinematic mode, the smoothers are reset.
- **Wheel**: in discrete mode (`discrete_mouse_scroll`), only the sign counts; × the
  wheel sensitivity. Accumulated per axis (reset on a change of sign), whole notches
  consumed. In game: vertical notch (otherwise the opposite of the horizontal one) → previous slot (upwards)
  or next; as a spectator, the flying speed changes by 0.005 per vertical notch (clamped
  0 to 0.2).

## 4. Window

- **Focus loss**: if `pauseOnLostFocus`, pause menu after 500 ms without focus (with no screen
  open).
- **Frame rate**: limit = `maxFps` (260 = unlimited). Minimized window: 10. If
  `inactivityFpsLimit` = `afk`: more than 10 min without input → 10; more than a minute → min(limit,
  30). Outside a world (title screen, menus): 60. Any input (key, button, wheel, mouse)
  resets the inactivity counter.
