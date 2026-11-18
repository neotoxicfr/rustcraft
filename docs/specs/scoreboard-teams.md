# Scoreboard and teams (26.3, protocol 777)

Source: reading of the decompiled client (kept outside the repository), reworded. Related: `player-list.md` (Tab),
`entities.md` part C § 1.8 (name tags), `text.md` (named colors, components),
`hud.md` (HUD layers).

Encodings: string = `varint` length then UTF-8, ≤ 32767 unless stated otherwise; text component =
network NBT (tag type then payload, no name); "optional" = `bool` present then the value;
enumeration = `varint` id, an out-of-range value falls back to id 0.

## 1. Client-side model

One scoreboard per connection (it survives dimension changes and respawns). It contains:
- **objectives** by name: display name (component), render type (`integer` or `hearts`),
  optional number format;
- **scores** indexed by (holder, objective). The holder is a string: profile name for a
  player, UUID as text (lowercase, with dashes) for any other entity. A score carries: value
  (`i32`), optional display name (replaces the holder's name), optional number format
  (replaces the objective's);
- 19 **display slots**, each empty or pointing to an objective;
- **teams** by name, and the holder → team membership (at most one team per holder).

Every processing error listed as "→ disconnect" below causes a disconnect with the
translatable message `disconnect.packetError`.

## 2. Number formats

Encoding: type id (`varint`, fixed registry), then the type's data.

| id | name | data | rendering of the value v |
|---|---|---|---|
| 0 | `blank` | nothing | empty text (width 0) |
| 1 | `styled` | style (NBT: compound of only the style fields of a component — color, bold, font…) | v in decimal (sign `-` if negative), with that style |
| 2 | `fixed` | text component | that component as is, v ignored |

Effective format of a score = the score's own format, otherwise the objective's format, otherwise the
default of the display place: sidebar → `styled` red (`red`); Tab list → `styled` yellow
(`yellow`); below the name → `styled` with no style.

## 3. Packets

### 3.1 `set_objective` (0x6C)

- objective name (string);
- mode (`i8`): 0 create, 1 remove, 2 update;
- only if mode is 0 or 2: display name (component), render type (`varint`: 0 `integer`,
  1 `hearts`), optional number format (§ 2).

Processing:
- 0: creates the objective. Name already exists → disconnect.
- 1: unknown objective → ignored. Otherwise: removed; every slot that pointed to it becomes empty;
  all its scores are deleted.
- 2: unknown objective → ignored. Otherwise replaces render type, display name and format (absent →
  no own format any more).
- Any other value: no extra field is read, nothing happens.

### 3.2 `set_display_objective` (0x64)

- slot (`varint`, table below; out of range → 0);
- objective name (string); empty string = clear the slot.

Unknown name → the slot is cleared (no error).

| id | slot | id | slot |
|---|---|---|---|
| 0 | `list` (Tab list) | 10 | `sidebar.team.gray` |
| 1 | `sidebar` | 11 | `sidebar.team.dark_gray` |
| 2 | `below_name` | 12 | `sidebar.team.blue` |
| 3 | `sidebar.team.black` | 13 | `sidebar.team.green` |
| 4 | `sidebar.team.dark_blue` | 14 | `sidebar.team.aqua` |
| 5 | `sidebar.team.dark_green` | 15 | `sidebar.team.red` |
| 6 | `sidebar.team.dark_aqua` | 16 | `sidebar.team.light_purple` |
| 7 | `sidebar.team.dark_red` | 17 | `sidebar.team.yellow` |
| 8 | `sidebar.team.dark_purple` | 18 | `sidebar.team.white` |
| 9 | `sidebar.team.gold` | | |

Sidebar slot of a team color = 3 + color id (§ 3.5).

### 3.3 `set_score` (0x70)

- holder (string);
- objective name (string);
- value (`varint`, interpreted as signed `i32`);
- optional display name (component);
- optional number format (§ 2).

Unknown objective → ignored (warning). Otherwise creates the score if needed and replaces value,
display name and format (an absent field erases the score's previous value).

### 3.4 `reset_score` (0x50)

- holder (string);
- objective name: optional (string).

- Absent: all of the holder's scores are deleted.
- Present, unknown objective → ignored. Otherwise the score (holder, objective) is deleted (no effect
  if it does not exist).

### 3.5 `set_player_team` (0x6F)

- team name (string);
- mode (`i8`): 0 create, 1 remove, 2 update the options, 3 add members,
  4 remove members;
- if mode is 0 or 2: parameters, in this order:
  1. display name (component);
  2. member prefix (component);
  3. member suffix (component);
  4. name tag visibility (`varint`): 0 `always`, 1 `never`, 2 `hideForOtherTeams`,
     3 `hideForOwnTeam`;
  5. collision rule (`varint`): 0 `always`, 1 `never`, 2 `pushOtherTeams`, 3 `pushOwnTeam`;
  6. color: optional, then `varint` 0..15 (table below; out of range → 0); absent =
     team without a color;
  7. options (`i8`): bit 0 (0x01) friendly fire allowed, bit 1 (0x02) sees invisible teammates;
- if mode is 0, 3 or 4: members = `varint` n then n strings (holder names: profile name or UUID
  as text).

Colors (id, name, RGB in `text.md`): 0 black, 1 dark_blue, 2 dark_green, 3 dark_aqua,
4 dark_red, 5 dark_purple, 6 gold, 7 gray, 8 dark_gray, 9 blue, 10 green, 11 aqua, 12 red,
13 light_purple, 14 yellow, 15 white. (In 26.3 the color is an optional 0..15; there is no longer a
"reset" value. The visibility of death messages is no longer transmitted.)

Default values of a new team: display name = literal team name, empty prefix and
suffix, visibility `always`, collision `always`, no color, options 0.

Processing, in this order:
1. Mode 0: creates the team; if it already exists, reuses it (warning, no error).
   Other modes: unknown team → the whole packet is ignored.
2. Parameters present: apply display name, color, options, visibility, collision, prefix,
   suffix.
3. Mode 0 or 3: for each member, remove it from its current team if it has one, then add it
   to this one.
   Mode 4: for each member, if it is not in THIS team → disconnect; otherwise removed.
4. Mode 1: the team disappears and all its members become teamless.
Any other mode (≥ 5): neither parameters nor members are read; nothing changes.

## 4. Name formatting by the team

Formatted name = empty component carrying the team's color (if it has one), with three
children: prefix, name, suffix. The children inherit the color unless they set their own
(a colored prefix keeps its color). Without a team: the name as is.

Uses on the client side:
- Tab list: profile name formatted by the team of the profile name, unless the entry has its own
  display name (which is not formatted); italic added for a spectator;
- a player's name tag: formatted profile name; for another entity: name (custom or
  translated) formatted by the team of its UUID;
- sidebar: name of each line (display name of the score, otherwise the holder as a literal) formatted by
  the holder's team.
- Chat: names already arrive formatted by the server (in the chat type's parameters); the client
  reformats nothing.
- Glowing outline (glowing effect): RGB color of the team made opaque, white without a color
  (already in `entities.md`).

Formatted team display name (used in server messages, not by client rendering): display
name in square brackets, colored with the team color.

## 5. Sidebar

### 5.1 Choosing the objective

If the local player (by profile name) has a team with a color and the slot
`sidebar.team.<color>` points to an objective, that one is used; otherwise the `sidebar` slot. None →
nothing is drawn.

### 5.2 Draw order

In the HUD (HUD not hidden by F1), after the demo overlay and before the action bar, the
titles, the chat and the Tab list; in a new layer.

### 5.3 Lines

- Scores of the objective whose holder does not start with `#`.
- Sort: value descending, then holder in case-insensitive alphabetical order.
- Only the first 15. The first (highest value) is at the top.
- For each line: name = § 4; score text = effective format (§ 2, default red);
  score width `ls` (0 for `blank`).

### 5.4 Layout (interface units, integer divisions)

- `lt` = width of the objective's display name; `sep` = width of `": "`.
- `L` = max(`lt`, for each line: name width + (`ls` > 0 ? `sep` + `ls` : 0)).
- `n` = number of lines; `h` = n × 9.
- `bottom` = guiHeight / 2 + h / 3.
- `left` = guiWidth − L − 3; `right` = guiWidth − 1.
- `titleY` = bottom − n × 9 (top of the first line).
- Title background: rectangle (left − 2, titleY − 10) → (right, titleY − 1).
- Lines background: rectangle (left − 2, titleY − 1) → (right, bottom).
- Title: x = left + L / 2 − lt / 2, y = titleY − 9.
- Line i (0 at the top): y = bottom − (n − i) × 9; name at x = left; score at x = right − `ls`
  (right-aligned).
- All texts: base color white 0xFFFFFFFF, **no shadow**.
- No line: only the title background (and a lines background of height 1) and the title, vertically
  centered at guiHeight / 2.

### 5.5 Background colors

Black whose alpha depends on the "text background" option:
- "chat only" (default): title alpha ⌊0.4 × 255⌋ = 102 → **0x66000000**; lines
  ⌊0.3 × 255⌋ = 76 → **0x4C000000**;
- "everywhere": title and lines = text background opacity (default 0.5 → 0x7F000000).

## 6. Score in the Tab list (slot `list`)

Completes `player-list.md` § 3 (the score width is already included there in the cell width).

- A player's score: score (profile name, objective); absent → no text (and 0 for hearts).
- `integer` objective: text = effective format (default yellow); score column width =
  max over the players of (text width > 0 ? width of `" "` + text width : 0).
- `hearts` objective: column width = 90.
- In a cell, after the optional head: `g` = x of the name + widest name + 1, `d` = g + column
  width. Nothing if the player is a spectator or if d − g ≤ 5.
- `integer`: text at x = d − width, with shadow, white base.

### 6.1 Hearts (`hearts`)

State per player UUID (created with the current value, erased when the player leaves the list):
last value, displayed value, time of last change, end of blinking. Time in
interface ticks (HUD counter).
- On each draw with value v: if v ≠ last value → end of blinking = t + 20 if v
  decreases, t + 10 if v increases; last value = v; last change = t. Then if
  t − last change > 20 → displayed value = v.
- Blinks if end > t and (end − t) mod 6 ≥ 3.
- `full` = ⌈max(v, displayed) / 2⌉; `total` = max(v, displayed, 20) / 2 (integer division).
  If full = 0: nothing.
- `step` = ⌊min((d − g − 4) / total, 9)⌋ (floating-point division).
- If step ≤ 3: text in place of the hearts. p = clamp(v / 20, 0, 1); RGB color =
  (⌊(1 − p) × 255⌋, ⌊p × 255⌋, 0), opaque, with shadow. Text = translation
  `multiplayer.player.list.hp` ("%shp") with v / 2 as a float ("10.0hp") if it fits between g and
  d, otherwise the number alone ("10.0"). Centered: x = (d + g − width) / 2.
- Otherwise, 9 × 9 sprites at x = g + k × step, y of the line:
  - for k from full to total − 1: container (`hud/heart/container`, or
    `hud/heart/container_blinking` while blinking);
  - for k from 0 to full − 1: container; then if blinking: `hud/heart/full_blinking` if
    2k + 1 < displayed, `hud/heart/half_blinking` if 2k + 1 = displayed; then
    `hud/heart/full` if 2k + 1 < v, `hud/heart/half` if 2k + 1 = v — from the 11th heart
    (k ≥ 10) these last two become `hud/heart/absorbing_full_blinking` and
    `hud/heart/absorbing_half_blinking`.

## 7. "Below the name" line (slot `below_name`)

- Text = formatted value (effective format, default no style) + `" "` + the objective's display name.
  With the `blank` format the text therefore starts with a space.
- Holder = profile name for a player, UUID as text for another entity (any entity can have
  one, not only players). No score → no line.
- Camera–entity distance² < (attribute `below_name_distance`)² (default 10; fixed 10 for a
  non-living entity).
- Only drawn if the name tag itself is displayed (the attachment point is only computed in that
  case). Drawing: `entities.md` § 1.8 (line first, name shifted by 0.25875 block).
- Special case: the mannequin shows its description in this place instead of a score.

## 8. Team rules for name tags (check of `entities.md` § 1.8)

The summary in `entities.md` is correct. Details:
- "Ally" = exactly the same team.
- "Invisible to the local player": false if the local player is a spectator; also false if the
  entity and the local player are in the same team and the team has the "sees invisible
  teammates" option; otherwise = the entity is invisible.
- Team rules apply only if the entity has a team AND is not the local player.
  In that case the result is final: the conditions "HUD not hidden (F1)", "entity ≠ camera
  entity" and "no passenger" are NOT checked (an entity of an `always` team keeps its
  name tag in F1 and even when loaded with passengers). The 32-block limit when sneaking and the
  `name_tag_distance` distance are still checked beforehand.
- `hideForOtherTeams` with a local team: visible only to an ally, and even then invisible
  if the "sees invisible teammates" option is active.

## 9. Collision rule (pushing between entities)

Predicate "B can be pushed by A" (`ruleA`, `ruleB` = rule of their team, `always` without a
team; `same` = A has a team and B has the same one):
- ruleA = `never` → never.
- B spectator or not pushable → no.
- On the client side, only the local player can be pushed (any other B → no).
- ruleB = `never` → no.
- (ruleA or ruleB = `pushOwnTeam`) and `same` → no.
- Otherwise yes if neither ruleA nor ruleB is `pushOtherTeams`, or if `same`.
The local player is therefore pushed by the living entities simulated around them, boats and
minecarts, according to this predicate.

## Uncertain points

- Behaviour of an unknown mode of `set_objective`/`set_player_team`: deduced from the decoding (no
  field read); a conforming server does not send one.
- Name `pushOwnTeam`: the code forbids pushing between members of the same team in this case (the
  name suggests the opposite); we follow the code.
- Lines background with 0 lines: rectangle of height 1 (titleY − 1 → bottom) deduced from the formulas,
  not checked in game.
- Rounding of the 0.3/0.4 alphas in single-precision float: 76 and 102 computed, to be confirmed by a
  screenshot.
- Integration of the push predicate: the local simulation of remote entities (which calls the
  push) belongs to the physics/entities spec, not re-read here.
