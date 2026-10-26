# Chat commands: tree, parsing, suggestions, display (26.3)

Clean-room spec, written from a reading of the decompiled 26.3 client and server (kept outside the repository) and from the known behaviour of the command-parsing library embedded in the client (free library, version 1.3.11, also decompiled outside the repository to verify § 2.2, 2.3, 3.1 and 4.4). Conventions for network types, Components and GUI coordinates: those of `chat.md` (header). Input, history, sending of `chat_command` / `chat_command_signed`, last seen: `chat.md` § 2.2, § 2.4 and § 4; here we only refer to them.

Vocabulary:
- **Reader**: cursor over the complete typed string (the leading `/` included: all positions count the `/`, it is simply skipped before parsing).
- **Node**: root, literal (fixed word) or argument (name + type). A node may have children, a **redirect** (to another node whose children take over) and be **executable** (the command may stop there).
- "Unquoted word": a run of the characters `0-9 A-Z a-z _ - . +`.

---

## 1. `commands` packet (received, play phase)

### 1.1 Encoding

| Field | Type |
|---|---|
| nodes | VarInt n, then n × Node |
| root index | VarInt |

Node:

| Field | Type | Presence |
|---|---|---|
| flags | `u8` | always |
| children | VarInt k, then k × VarInt (indices) | always |
| redirect target | VarInt (index) | if flag `0x08` |
| name | String(32767) | literal and argument |
| argument type | VarInt = index in the `command_argument_type` registry (§ 1.3) | argument |
| type properties | depending on the type (§ 1.3) | argument |
| suggestions identifier | Identifier (String) | argument with flag `0x10` |

Flags:

| Bits | Meaning |
|---|---|
| `0x03` | kind: 0 root, 1 literal, 2 argument (3: treated as root) |
| `0x04` | executable |
| `0x08` | redirect present |
| `0x10` | custom suggestions (identifier present) |
| `0x20` | **restricted**: the node requires a permission that a player with no rights at all does not have (§ 4.5) |

Node order on the server side (informative): breadth-first traversal from the root (children, then redirect target), a node already numbered is not taken again. The client does not depend on it.

### 1.2 Building the tree on the client

- Validation: all nodes must be buildable, resolving redirects first (a node whose target is not yet built waits); if a full pass makes no more progress → "impossible" tree, decoding error (disconnect).
- Recursive resolution from the root with memoisation (cycles through redirects are allowed). A child that resolves to the **root** is not added as a child.
- Argument type with an unknown index: the node becomes a root… but its properties are not read, so the rest of the packet is misaligned (in practice: decoding error). A client must know all 62 types.
- Each reception **replaces** the whole tree. The server sends it on entering the game, on every operator level change (§ 4.5), after `/reload` and a few other permission changes.
- The server only sends the nodes the player **can use** (recursive filtering by permission): a non-operator player receives a reduced tree (`help`, `list`, `me`, `msg`, `teammsg`, `tell`, `tm`, `trigger`, `w`, `random`… depending on the server).

### 1.3 `command_argument_type` registry (26.3, 62 entries)

Indices are fixed (built-in registry, registration order). Properties encoded after the index:

| # | Identifier | Properties |
|---|---|---|
| 0 | `brigadier:bool` | — |
| 1 | `brigadier:float` | `u8` flags (`0x01` min, `0x02` max), then `f32` min if `0x01`, `f32` max if `0x02` (defaults ∓ largest finite 32-bit real) |
| 2 | `brigadier:double` | same as `f64` (defaults ∓ largest finite `f64`) |
| 3 | `brigadier:integer` | same as `i32` (defaults `i32::MIN` / `i32::MAX`) |
| 4 | `brigadier:long` | same as `i64` (defaults `i64::MIN` / `i64::MAX`) |
| 5 | `brigadier:string` | VarInt: 0 = one word, 1 = phrase (word or quoted string), 2 = greedy (the whole rest) |
| 6 | `entity` | `u8`: `0x01` single entity only, `0x02` players only |
| 7 | `game_profile` | — |
| 8 | `block_pos` | — |
| 9 | `column_pos` | — |
| 10 | `vec3` | — |
| 11 | `vec2` | — |
| 12 | `block_state` | — |
| 13 | `block_predicate` | — |
| 14 | `item_stack` | — |
| 15 | `item_predicate` | — |
| 16 | `team_color` | — |
| 17 | `hex_color` | — |
| 18 | `component` | — |
| 19 | `style` | — |
| 20 | `message` | — |
| 21 | `nbt_compound_tag` | — |
| 22 | `nbt_tag` | — |
| 23 | `nbt_path` | — |
| 24 | `objective` | — |
| 25 | `objective_criteria` | — |
| 26 | `operation` | — |
| 27 | `particle` | — |
| 28 | `angle` | — |
| 29 | `rotation` | — |
| 30 | `scoreboard_slot` | — |
| 31 | `score_holder` | `u8`: `0x01` multiple allowed |
| 32 | `swizzle` | — |
| 33 | `team` | — |
| 34 | `item_slot` | — |
| 35 | `item_slots` | — |
| 36 | `resource_location` | — |
| 37 | `function` | — |
| 38 | `entity_anchor` | — |
| 39 | `int_range` | — |
| 40 | `float_range` | — |
| 41 | `dimension` | — |
| 42 | `gamemode` | — |
| 43 | `time` | `i32` minimum (in ticks) |
| 44 | `resource_or_tag` | Identifier of the target registry |
| 45 | `resource_or_tag_key` | Identifier of the target registry |
| 46 | `resource` | Identifier of the target registry |
| 47 | `resource_key` | Identifier of the target registry |
| 48 | `resource_selector` | Identifier of the target registry |
| 49 | `template_mirror` | — |
| 50 | `template_rotation` | — |
| 51 | `heightmap` | — |
| 52 | `loot_table` | — |
| 53 | `loot_predicate` | — |
| 54 | `loot_modifier` | — |
| 55 | `context_float_provider` | — |
| 56 | `context_int_provider` | — |
| 57 | `slot_source` | — |
| 58 | `dialog` | — |
| 59 | `feature` | — |
| 60 | `swing_animation` | — |
| 61 | `uuid` | — |

Registry identifier = String `namespace:path` (e.g. `minecraft:enchantment`, `minecraft:worldgen/structure`).

### 1.4 Suggestion identifiers

| Identifier | Computation on the client |
|---|---|
| `minecraft:ask_server` | request to the server (§ 3.2) |
| `minecraft:available_sounds` | identifiers of the sound events known to the client (`sounds.json` loaded), "resource" filtering (§ 3.4) |
| `minecraft:summonable_entities` | entity types of the registry that are enabled and summonable, with tooltip = translated name of the type |
| `minecraft:post_effects` | post-processing effects known to the client |
| any other identifier | treated as `ask_server` |

In practice, a vanilla server puts `ask_server` on every argument whose suggestions depend on server state (functions, objectives, score holders, attributes, bossbars, data sets…).

---

## 2. Client-side parsing

### 2.1 When and how

- On every modification of the input field, if the text starts with `/` (or if the screen is in "commands only" mode), the text is parsed with the received tree. The result (recognised nodes, arguments and their ranges, per-node errors, reader position) is used for: colouring (§ 4.3), suggestions (§ 3), the usage or error message (§ 4.4), and, at send time, for choosing the packet (§ 2.6).
- A command is **never refused** on the client for a parse error: Enter always sends the command as is (`chat.md` § 4); the server itself answers with an error message.

### 2.2 Tree traversal (library, verified)

From a node, at the current cursor:
1. **Relevant** children: if the next word (up to the space) is exactly the name of a literal child, only that literal is tried; otherwise all argument children (non-matching literals fail with `argument.literal.incorrect`).
2. A child is only tried if the source can use it (permission requirements: only the "restricted" flag sets one on the client, § 4.5).
3. Literal: **exact, case-sensitive** match, followed by a space or the end.
4. Argument: the type's parser (§ 2.4) is run from the cursor; if it succeeds, it must be followed by a space or the end, otherwise error `command.expected.separator`.
5. After a space, we continue into the node's children; if it has a redirect, we open a **child context** and continue into the children of the target (e.g. `execute … run` redirects to the root). We only continue if at least **2** characters remain (the space and another one; **1** is enough for a redirect); otherwise the interpretation stops there. A redirect returns its result immediately (the other children are no longer compared).
6. Several possible interpretations: stable sort, first those that read the whole input, then those without errors; the first one wins (on a tie: the order of the children). The errors of failed children are forgotten as soon as an interpretation exists.
7. An argument error is remembered per node (several possible: one per branch tried).

Valid command = input entirely read, no error, and last node executable.

### 2.3 Basic reading (library, verified)

| Reading | Rule | Errors |
|---|---|---|
| spaces | Unicode whitespace characters skipped where the grammar allows it (selector options, properties…), **never** between two arguments (a single `' '` separator) | |
| integer / long | run of `0-9 - .` then conversion | `parsing.int.expected` / `parsing.int.invalid` (same for `long`) |
| 32 / 64-bit real | run of `0-9 - .` (no exponent) | `parsing.float.*`, `parsing.double.*` |
| boolean | string `true` or `false` (case-sensitive) | `parsing.bool.expected`, `parsing.bool.invalid` |
| quoted string | opens with `"` or `'`, escapes `\\` and `\` + the same quote | `parsing.quote.expected.start`, `parsing.quote.expected.end`, `parsing.quote.escape` |
| string | quoted if it starts with a quote, otherwise unquoted word | |
| expected character | | `parsing.expected` |

Numeric bounds (`brigadier:*`): `argument.integer.low` / `.big`, `argument.long.*`, `argument.float.*`, `argument.double.*` (parameters: bound, value found). The cursor goes back to the start of the number.

Boolean: suggestions `true`, `false` filtered by prefix.
String: one word = unquoted word; phrase = string; greedy = the whole rest of the input.

### 2.4 Parsing of each type

Identifier (used everywhere): greedy run of `0-9 a-z _ : / . -`; default namespace `minecraft`; namespace without `/`. Invalid → `argument.id.invalid`, cursor put back at the start.

| Type | Syntax and rules | Errors (keys) |
|---|---|---|
| `entity` | see § 2.5. Then: if "single" and the selector can return several → error (cursor put back at **0**); if "players only" and the selector includes non-players (except `@s`) → error | `argument.entity.toomany`, `argument.player.toomany`, `argument.player.entities` |
| `game_profile` | selector (must target players only) or raw name up to the space | `argument.player.entities` |
| `score_holder` | `@…` selector (a single entity if not "multiple"), otherwise raw name up to the space; `*` = all | `argument.entity.toomany` |
| `block_pos` | 3 components; if the first starts with `^` → local; otherwise each is `~`, `~n` or integer (after `~`, a real is allowed) | `argument.pos.missing.int`, `argument.pos.mixed`, `argument.pos3d.incomplete` |
| `vec3` | 3 components, reals; `^` local; a number **without a point** and not relative gets `+0.5` in X and Z (not in Y) | `argument.pos.missing.double`, `argument.pos.mixed`, `argument.pos3d.incomplete` |
| `column_pos` | 2 integer components X Z (`~` allowed, `^` refused) | `argument.pos2d.incomplete`, `argument.pos.mixed` |
| `vec2` | 2 reals X Z, same `+0.5` rule | `argument.pos2d.incomplete` |
| `rotation` | 2 components `yaw pitch`, `~` allowed, no `+0.5` | `argument.rotation.incomplete` |
| `angle` | one value `~` / `~n` / real, NaN and infinity refused | `argument.angle.incomplete`, `argument.angle.invalid` |
| `swizzle` | letters `x y z` without repetition, up to the space | `arguments.swizzle.invalid` |
| `block_state` | block identifier, then optional `[key=value,…]` (known properties, valid values, no duplicates; spaces allowed), then optional `{SNBT}`; **no** `#` tag | `argument.block.id.invalid`, `argument.block.property.unknown`, `.duplicate`, `.invalid`, `.novalue`, `.unclosed`, `argument.block.tag.disallowed` |
| `block_predicate` | as above, but `#tag` allowed; with a tag, the properties are "vague" (names not checked) | `arguments.block.tag.unknown` in addition |
| `item_stack` | item identifier, then optional `[component=SNBT, !component, …]` (`!` = removed component; no duplicates; a comma must be followed by a component) | `argument.item.id.invalid`, `arguments.item.component.unknown`, `.malformed`, `.expected`, `.repeated` |
| `item_predicate` | `item` / `#tag` / `*`, then `[conditions]`: conditions separated by `,` (all required), alternatives by `|`, negation `!`; a condition = `component` (presence), `component=SNBT` (equality) or `predicate~SNBT` (sub-predicate) | `arguments.item.tag.unknown`, `arguments.item.predicate.unknown`, `.malformed` |
| `team_color` | word: colour name (the 16 colours `black` … `white`, without `reset`) | `argument.color.invalid` |
| `hex_color` | word of 3 or 6 hexadecimal digits (`F00` → each digit ×17) | `argument.hexcolor.invalid` |
| `component` | SNBT (§ 2.4.1) decoded as a Component (a lone string = text) | `argument.component.invalid` |
| `style` | compound SNBT decoded as a style (`{bold:true}`, `{color:'red'}`) | `argument.style.invalid` |
| `message` | the whole rest of the input; more than **256** remaining characters → error. If selectors are allowed (§ 4.5), each `@…` is read as a selector (an `@` that does not form one stays text) | `argument.message.too_long` (length, 256) |
| `nbt_compound_tag` | SNBT that must be a compound `{…}` | `argument.nbt.expected.compound`, `snbt.parser.*` |
| `nbt_tag` | any SNBT value | `snbt.parser.*` |
| `nbt_path` | sequence of nodes: `name` (without `space " ' [ ] . { }`) or `"name"`; `[n]` (index, negative allowed), `[]` (all), `[{…}]` (matching elements), `{…}` (filter); separated by `.`; max depth 512 | `arguments.nbtpath.node.invalid`, `arguments.nbtpath.too_deep` |
| `objective`, `team` | unquoted word | — |
| `objective_criteria` | everything up to the space; must be a known criterion (`dummy`, `trigger`, `health`, `minecraft.type:item`…) | `argument.criteria.invalid` |
| `operation` | everything up to the space among `= += -= *= /= %= < > ><` | `arguments.operation.invalid` |
| `particle` | particle type identifier then, if the type has options, `{SNBT}` | `particle.notFound`, `particle.invalidOptions` |
| `scoreboard_slot` | word: `list`, `sidebar`, `below_name`, `sidebar.team.<colour>` | `argument.scoreboardDisplaySlot.invalid` |
| `item_slot` | name up to the space designating **one** slot (`container.5`, `weapon.mainhand`…) | `slot.unknown`, `slot.only_single_allowed` |
| `item_slots` | name or range (`container.*`) | `slot.unknown` |
| `resource_location`, `dimension` | identifier | `argument.id.invalid` |
| `function` | `#identifier` (tag) or identifier | (checked by the server) |
| `entity_anchor` | `eyes` or `feet` | `argument.anchor.invalid` |
| `int_range` | `n`, `n..`, `..m`, `n..m`; integer numbers (a lone `.` is read into the number → invalid integer); min > max refused | `argument.range.empty`, `parsing.int.invalid`, `argument.range.swapped` |
| `float_range` | same with reals | `argument.range.empty`, `parsing.double.invalid`, `argument.range.swapped` |
| `gamemode` | `survival`, `creative`, `adventure`, `spectator` | `argument.gamemode.invalid` |
| `time` | real then optional unit: `d` (×24000), `s` (×20), `t` or nothing (×1); rounded to nearest; < received minimum → error | `argument.time.invalid_unit`, `argument.time.tick_count_too_low` |
| `resource` | identifier that must exist in the target registry (registries known to the client) | `argument.resource.not_found`, `argument.resource.invalid_type` |
| `resource_key` | identifier, existence not checked | — |
| `resource_or_tag` / `_key` | `#identifier` (registry tag) or identifier | `argument.resource_tag.not_found`, `argument.resource_tag.invalid_type` |
| `resource_selector` | pattern made of identifier characters plus `*` and `?`; `minecraft:` added if there is no `:`; must match at least one entry | `argument.resource_selector.not_found` |
| `template_mirror` | `none`, `left_right`, `front_back` | `argument.enum.invalid` |
| `template_rotation` | `none`, `clockwise_90`, `180`, `counterclockwise_90` | `argument.enum.invalid` |
| `heightmap` | `world_surface`, `motion_blocking`, `motion_blocking_no_leaves`, `ocean_floor` ("real" forms only) | `argument.enum.invalid` |
| `loot_table`, `loot_predicate`, `loot_modifier`, `context_float_provider`, `context_int_provider`, `dialog`, `feature` | identifier of a registry element, **or** inline value in SNBT (`{…}`, number, boolean…) decoded according to the type | `argument.resource_or_id.no_such_element`, `argument.resource_or_id.failed_to_parse` |
| `slot_source` | inline value/identifier (as above) or slot name | same |
| `swing_animation` | `none`, `whack`, `stab` | `argument.swing_animation.invalid` |
| `uuid` | longest leading run of `0-9 a-f A-F -`, which must form a UUID | `argument.uuid.invalid` |

#### 2.4.1 SNBT (NBT text of commands)
- Compound `{key:value, …}` (key = unquoted word or string; trailing comma tolerated), list `[v, …]`, typed arrays `[B; …]`, `[I; …]`, `[L; …]`.
- Numbers: decimal, `0x` hexadecimal, `0b` binary, `_` separators (not at the start/end), sign `+`/`-`; integer suffixes `b s i l` (any case) optionally preceded by `u` (unsigned) or `s` (signed); reals with `.`, exponent `e±n`, suffixes `f d`; infinity refused; insignificant leading zero refused.
- Strings `"…"` or `'…'` with escapes `\b \s \t \n \f \r \\ \' \"`, `\xHH`, `\uHHHH`, `\UHHHHHHHH`, `\N{name}`. Unquoted word: `true`/`false` (any case) → byte 1/0, otherwise string. Functions `bool(x)` and `uuid("…")`.
- Errors: `snbt.parser.*` (`expected_decimal_numeral`, `leading_zero_not_allowed`, `underscore_not_allowed`, `invalid_character_name`, `invalid_array_element_type`, `infinity_not_allowed`…), extra data `argument.nbt.trailing`.

### 2.5 Entities and selectors

- Leading `@`: if selectors are not allowed (§ 4.5) → `argument.entity.selector.not_allowed`. Otherwise a letter:

| Selector | Max | Order | Players only | Living only |
|---|---|---|---|---|
| `@p` | 1 | nearest | yes | no |
| `@a` | ∞ | any | yes | no |
| `@r` | 1 | random | yes | no |
| `@s` | 1 | — (self) | no (but tolerated for "players only") | no |
| `@e` | ∞ | any | no | yes |
| `@n` | 1 | nearest | no | yes |

  Nothing after `@` → `argument.entity.selector.missing`; another letter → `argument.entity.selector.unknown`.
- Without `@`: string (word or quotes); if it is a UUID → target by UUID (may designate an entity); otherwise player name of 1 to 16 characters, else `argument.entity.invalid`.
- Options `[key=value, …]` (spaces allowed around): unknown key → `argument.entity.options.unknown`; not applicable → `argument.entity.options.inapplicable`; no `=` → `argument.entity.options.valueless`; neither `,` nor `]` → `argument.entity.options.unterminated`. `!` = negation for the options that accept it.

| Option | Value | Applicable if |
|---|---|---|
| `name` | `[!]string` | a single positive name; negations repeatable |
| `distance` | real range ≥ 0 | not yet given (`argument.entity.options.distance.negative`) |
| `level` | integer range ≥ 0 (restricted to players) | not yet given (`…level.negative`) |
| `x` `y` `z` `dx` `dy` `dz` | real | each once |
| `x_rotation` `y_rotation` | angle range (wrapped to ±180°, min > max = interval crossing ±180) | once |
| `limit` | integer ≥ 1 (`…limit.toosmall`) | not with `@s`, once |
| `sort` | `nearest` `furthest` `random` `arbitrary` (`…sort.irreversible` otherwise) | not with `@s`, once |
| `gamemode` | `[!]mode` (players) | one positive, or negatives (`…mode.invalid`) |
| `team` | `[!]word` (empty = no team) | same rule |
| `type` | `[!]identifier` or `[!]#tag` | one positive; negatives and tags repeatable (not the same tag twice); `…type.invalid` |
| `tag` | `[!]word` (empty = no tag) | always |
| `nbt` | `[!]{SNBT}` | always |
| `scores` | `{objective=integer range, …}` | once |
| `advancements` | `{id=boolean, id={criterion=boolean, …}, …}` | once |
| `predicate` | `[!]identifier` | always |

These options only take effect on the server; the client only checks the syntax, colours and suggests.

### 2.6 Signable arguments and packet choice

- Only the `message` type (no. 20) is **signable**.
- On sending (`chat.md` § 2.2): the command is parsed again with the normal source and the `message` arguments actually read are listed, **in the main context and the child contexts, but stopping at a redirect to the root** (so `/execute run say …` → no signable argument → `chat_command`). For each: (node name, exact text of its range in the command).
- Empty list → `chat_command`. Otherwise → `chat_command_signed`, one entry (name ≤ 16, signature) per argument, 8 at most; without a chat session the list is empty (`chat.md` § 2.2). The last-seen messages are taken **only** in this case.

---

## 3. Suggestions

### 3.1 Computation

- Triggered after each parse, if the cursor is ≥ 1 (in chat mode, a command; the lone `/` is enough).
- We look for the **parent node** at the cursor: the last node whose parse ends before the cursor (child context included); the start position = start of the next word. For each child of the parent we ask for its suggestions on the text **truncated at the cursor**:
  - literal: its name if, in lowercase, it starts with the typed text in lowercase;
  - argument with a suggestion identifier: the provider of § 1.4;
  - otherwise the type's own suggestions (§ 3.3).
- Merge: the common range covers all ranges, each suggestion is extended to that range (copying the typed text around it), duplicates removed, **case-insensitive** alphabetical sort. A single source: returned as is (already sorted by its builder). Any source ignores a suggestion **equal** to the text already typed since the start of the range (not numbers).
- The result is only used once **all** sources have answered (the server included). The list is then opened automatically if the "automatic suggestions" option (`autoSuggestCommands`, default true) is active and if suggestions are allowed (see § 4.1).
- Re-sort at display time: first the suggestions that start with the last typed word (text before the cursor after the last run of whitespace, in lowercase), or with `minecraft:` + that word; then the others; order kept within each group.
- Text without `/` (message): suggestions = names of online players (player list) + custom completions (§ 3.5), filtered by the last word ("substring" rule § 3.4).

### 3.2 Request to the server

- `command_suggestion` (sent): VarInt transaction identifier, String(32500) = the input **truncated at the cursor**, `/` included (verified: the suggestion context is built on the truncated input).
- Identifier: client counter initialised to −1, **pre-incremented** on each request; a new request cancels the previous one (its answer will be ignored); an accepted answer resets the counter to −1 (the next request therefore starts again at 0).
- `command_suggestions` (received): VarInt identifier, VarInt start, VarInt length (range in the sent string, `/` included), VarInt n, n × (String text, Optional(Component) tooltip). Accepted only if the identifier is the pending one.
- Server: handles at most one request per tick (the most recent); 1000 suggestions at most; if the player does not have command-block rights, a request of more than **256** characters is silently ignored (no kick), otherwise the limit is 32500.

### 3.3 Built-in suggestions per type (computed by the client)

| Type | Suggestions |
|---|---|
| `entity` | names of online players; + UUID of the entity targeted by the crosshair (except "players only"); + `@p @a @r @s @e @n` (tooltips `argument.entity.selector.nearestPlayer`, `allPlayers`, `randomPlayer`, `self`, `allEntities`, `nearestEntity`) if selectors are allowed. After `@x`: `[`; inside the options: `]` then `key=` (applicable options, prefix, tooltip `argument.entity.options.<key>.description`); after a value: `,` and `]`; values of `sort`; `gamemode` (with `!` variants); `type` (identifiers, `!id`, `#tag`, `!#tag`) |
| `game_profile`, `score_holder` | as `entity` (online players, selectors); often replaced by `ask_server` |
| `block_pos`, `column_pos` | integer coordinates of the **block targeted** by the crosshair, otherwise `~ ~ ~`; if the text starts with `^`: `^ ^ ^` |
| `vec3`, `vec2` | exact targeted point (reals with 2 decimals) if a block is targeted, otherwise `~ ~ ~`; `^` likewise |
| (coordinates, common rule) | empty text → `x`, `x y`, `x y z`; one component typed → `typed y`, `typed y z`; two → `typed typed z`; only proposals that parse without error are kept (2D: `x`, `x z` then `typed z`) |
| `block_state`, `block_predicate` | block identifiers (and `#tags` for the predicate); then `[` / `{`; property names, `=`, possible values, `,`, `]` |
| `item_stack`, `item_predicate` | item identifiers (`#`, `*` for the predicate); `[`, component names (and `!`), `=`, `,`, `]` |
| `team_color`, `gamemode`, `entity_anchor`, `heightmap`, `template_*`, `swing_animation`, `scoreboard_slot`, `operation`, `item_slot(s)` | the allowed values |
| `hex_color` | the examples `F00`, `FF0000` |
| `team` | teams known to the client (received scoreboard) |
| `objective` | **request to the server** even without a suggestion identifier |
| `dimension` | known dimensions (list received on connection) |
| `particle` | identifiers of the particle registry |
| `time` | after a valid number: units `d`, `s`, `t` |
| `resource`, `resource_key`, `resource_or_tag(_key)` | elements (and `#tags` depending on the type) of the target registry if known to the client; **otherwise request to the server** |
| `bool` | `true`, `false` |
| others (numbers, strings, `message`, SNBT, `uuid`…) | nothing |

### 3.4 Filtering rules
- "Substring": the typed text (lowercase) must match the **start** of the value (lowercase) or the start of a segment located after `.`, `_` or `/` (e.g. `stone` finds `smooth_stone` through the segment after `_`; `ne` does not find `stone`).
- "Resource": if the text contains `:`, substring rule on the full identifier; otherwise on the namespace **or** on the path. With a prefix (`#`, `!`, `!#`): the text must start with that prefix (or be a beginning of it), the rest is filtered the same way, and the suggestion carries the prefix.

### 3.5 `custom_chat_completions` (received)
VarInt action (0 add, 1 remove, 2 set = clear then add), VarInt n, n × String. Set without duplicates, only used for **messages** without `/` (§ 3.1), in addition to the names of online players.

---

## 4. Interface

These rules apply to the chat screen (`chat.md` § 4). The field there is borderless, `x = 4`.

### 4.1 States
- On opening: suggestions **not** allowed (the list does not open by itself) but parsing, colouring and usage are active. The first modification of the text allows suggestions. Browsing the history (arrows with no open list) forbids them again.
- The list can always be opened with **Tab** (it does not hide itself: no "hideable" mode in the chat).
- When the list is open, the usage message is not drawn.

### 4.2 Suggestion list (above the field)
- Text width `W` = max width of the suggestion texts. Visible lines `n = min(count, 10)`.
- `x = clamp(Xchar(range start), 0, Xchar(0) + inner_width − W) − 1` where `Xchar(i)` = screen abscissa of character i of the field (horizontal scrolling included); `y = gui_height − 12 − 3 − 12·n`. Rectangle `W + 1` × `12·n`.
- Each line i: background `[x, y+12i] → [x+W+1, y+12i+12]` colour `0xD0000000`; text at `(x+1, y+2+12i)`, **yellow `0xFFFFFF00`** if selected, otherwise grey `0xFFAAAAAA` (no shadow… see Uncertain points).
- More than 10 suggestions: 1 px band above (`y−1`) and below (`y+12n`) in the background colour; if there are some before (offset > 0), white dots `0xFFFFFFFF` on the top band (every other pixel, even relative x); if some remain after, same at the bottom.
- Hover: the hovered line is selected **only if the mouse moved** since the previous frame; tooltip = tooltip of the selected suggestion (if present) while the mouse is over a line; "hand" cursor.
- Click inside the rectangle: selects and applies the line. Wheel inside the rectangle: offset −= sign (clamped to `[0, count − 10]`).
- Selection (circular index): the ghost text of the field = the part of the completed text that goes beyond the current input (if the completed text starts with the input), drawn in grey `0xFF808080` right after the cursor when it is at the end of the text.
- Keys, list open:
  - Up / Down: selection −1 / +1 (circular), scrolls to keep it visible (when going down, it is placed on the last visible line), disables the "cycle".
  - Tab: if the cycle is active, selection +1 (Shift+Tab: −1); then **applies** the selection: the range is replaced by the text, cursor placed at `start + length`, the list stays open and the cycle becomes active (next Tab = next suggestion).
  - Escape: closes the list and clears the ghost text, **without** closing the screen.
  - Any other key: normal behaviour of the field; a modification restarts parsing and closes the list (it reopens according to § 3.1).

### 4.3 Input colouring
On the last (deepest) context of the parse, for each argument read in order: cyclic colours **aqua `0x55FFFF`, yellow `0xFFFF55`, green `0x55FF55`, pink `0xFF55FF`, gold `0xFFAA00`**; everything else (the `/`, the literals, the spaces) in grey `0xAAAAAA`. If the parse stopped before the end, the unread part is in red `0xFF5555`. The text of a message without `/` is not coloured (off-white of the field, `chat.md` § 4). Draft: grey italic (`chat.md` § 4).

### 4.4 Usage and errors (lines above the field)
Computed when the suggestions are ready:
1. If the cursor is at the end of the text, there is **no** suggestion and there are parse errors: one line per error that is not "incorrect literal"; if there was at least one "incorrect literal" error, one extra `command.unknown.argument` line. Otherwise, if the input was not read to the end → "extra characters".
2. If there is no line: usage of the **argument** children of the parent node at the cursor (literals are omitted), in grey `0xAAAAAA`. If this usage is empty and there are extra characters: a general error line (unknown command `command.unknown.command`, or the error of the first failed node).
3. Format of an error: if it has a context, `command.context.parse_error` = "%message at position %cursor: %context" where context = `...` if truncated + up to 10 characters before the cursor + `<--[HERE]`; otherwise the message alone.
4. Chat restrictions (option "chat visibility" or account): `chat_screen.commands_not_allowed` (red) for a command if commands are forbidden (visibility "hidden"), `chat_screen.messages_not_allowed` (red) for a message if messages are; Enter then has no effect.

"Smart" usage of a node (library, verified): literal = its name, argument = `<name>`; in brackets `[…]` if the parent is executable; redirect → `-> target` or `...` if it is the root; a single child → its usage chained; several → `(a|b|c)` (or `[a|b]` if executable), or the common usage if identical.

Drawing: lines stacked upwards, line i at `y = gui_height − 27 − 12i`; `x = clamp(Xchar(start of the word at the cursor), 0, Xchar(0) + inner_width − Wu)` (`Wu` = max width of the lines; 0 if there is no line); background `[x−1, y] → [x+Wu+1, y+12]` colour `0xD0000000`; text at `(x, y+2)`, white by default (the component's colours take precedence), shadowed.

### 4.5 Permissions
- Operator level received through `entity_event` on the local player: **24 → 0, 25 → 1, 26 → 2, 27 → 3, 28 → 4**. The server then sends `commands` again: it is this new tree that changes the visible commands.
- `@…` selectors allowed if level ≥ **2** (and in the arguments `entity`, `game_profile`, `score_holder`, `message`): otherwise error `argument.entity.selector.not_allowed` and no `@` suggestion. For `message`, without this right, the text is taken as is (no error).
- "Restricted" flag (`0x20`): for input in the chat, the client gives itself the corresponding permission → these nodes are visible and usable normally.
- Commands launched **without typing** (click on a `run_command` text, dialog boxes): the client checks before sending `chat_command`:
  1. parse with the player's rights invalid → confirmation screen `multiplayer.confirm_command.parse_errors`;
  2. signable arguments → `multiplayer.confirm_command.signature_required` (button `multiplayer.confirm_command.suggest_command` which opens the chat prefilled if commands are allowed; otherwise just back);
  3. parse **with no rights at all** (restricted nodes and selectors refused) invalid → `multiplayer.confirm_command.permissions_required`;
  4. otherwise direct send. Screen: title `multiplayer.confirm_command.title`, command in yellow, buttons `multiplayer.confirm_command.run_command` (or copy) and Back/Cancel.

---

## 5. Deleting a signed message (`delete_chat`)

Rules and rendering: `chat.md` § 1.5 (compacted signature; removal from the deferred queue; otherwise replacement after at least 60 HUD ticks by `chat.deleted_marker` in grey italic, "system" tag bar `0xD0D0D0`, same added time; unknown index → disconnect). Rendering: the message is replaced **at its rank** in the message history (same added time, hence same fade), then **all** displayed lines are rebuilt from the history (messages filtered by the reception rights, split as on arrival, `chat.md` § 3.4): the lines of the deleted message give way to the line(s) of the marker (one line at the default width). Scrolling is not explicitly reset. No visible effect on an offline server (no signed message).

---

## 6. Integrated server side (singleplayer, `rc-sim/src/commands/`)

### 6.1 Tree
- The server of a singleplayer game registers the commands with the "integrated" selection: no `ban`, `ban-ip`, `banlist`, `deop`, `op`, `pardon`, `pardon-ip`, `perf`, `save-*`, `setidletimeout`, `stop`, `transfer`, `whitelist`; `publish` and `unpublish` in addition; `seed` and `version` without permission; `jfr` if the JVM profiler is available.
- Named permission levels: 0 everyone, 1 moderators, 2 game masters, 3 administrators, 4 owners. Host player: 4 if cheats are enabled (`allowCommands` in `level.dat`), otherwise 0; opening to the local network later.
- The tree sent is recursively filtered by the requirements of each node (a refused node disappears with its children, redirects point to the copies). A node's "restricted" flag = its requirement is not met by a source without rights.
- The generator's `commands.json` report sorts the keys (order lost) and omits the suggestion providers: the tree therefore comes from the `CommandsExtractor.java` extractor, which builds the tree like the game and then encodes it in the exact packet format for each of the 5 levels (`<cache>/<version>/commands.bin`). The order of the `gamerule` rules varies from one game launch to another (hash table).

### 6.2 Execution and feedback
- The input is parsed with the tree of the source's level (same algorithm as the client, § 2.2). Parse error: red message, then grey context line (`...` if cut, 10 characters before the cursor, rest of the input in underlined red, `command.context.here` in red italic; click = suggest `/command`). Input entirely read but last node not executable → `command.unknown.command` at the end cursor. Several errors on the last branch: `command.unknown.command` if nothing was read, otherwise `command.unknown.argument`.
- A redirect splits the input into segments; every segment except the last is an `execute` sub-command (or an alias: `tp`, `xp`, `tell`, `w`, `tm`) that transforms the list of sources; the final command runs for each source. If a "forking" sub-command (as, at, positioned as, rotated as, facing entity, if, unless, on) is present, the errors of the sources are silenced.
- Success: to the player if the rule `send_command_feedback` is true; from a command block: `LastOutput` = "[HH:mm:ss]" + message if the output is tracked and the rule `command_block_output` is true, and broadcast to operators "chat.type.admin" in grey italic. Failure: red, always (except for a silent source).
- "One / several" answers: a tracker counts the elements touched (and those with a non-zero value); "single" message if only one, otherwise "multiple" with the count; some commands count all elements (scores add/remove/operation), the others only the non-zero ones (side effect of the game reproduced: `tag @s list` without a tag → "multiple" message with 0).
- Arguments: coordinates (`~` relative to the source, `^` local through the rotation and the anchor, +0.5 in X and Z for an absolute integer in `vec3`), selectors (sort: `@p` and `@n` nearest, `@r` random, `@a` `@e` in the order entities were added), messages (selectors replaced by names if level ≥ 2), components (`selector` and `score` contents resolved).
- Numbers in translation parameters in Java format (`1.0`, `1.0E7`); teleport coordinates in `%f` (6 decimals).

### 6.2.1 Execution context, functions and returns (W11)
- Every top-level execution (input, command block, function launched by the server: `tick`/`load` tags, `schedule`, advancement reward — one per function) opens a context: command quota = rule `max_command_sequence_length` (at least 1), source limit = `max_command_forks`. Costs: one sub-command step (except pure `execute run` redirects and aliases), one final command per source (except `function` and `return`, run directly), one function call. Quota exhausted: everything stops, log line "Command execution stopped due to limit (executed N commands)", no message. Source list ≥ limit: error `command.forkLimit` on the original source (silenced in forked mode). No depth limit in the game; ours is a safety net of 65,536 nested calls (singleplayer thread with a reserved 1 GiB stack, ≈ 6 KB per level).
- Error of a sub-command outside forked mode: the command stops (the other sources are not tried).
- `return <value>`: result of the function; `return fail`: failure without a message; `return run <command>`: the function stops at this step, the rest runs for the first source only and its result (success or failure) becomes the function's; no source left → failure. `return run function f` returns the result of `f` (the first function of a tag that returns something stops the following ones; none → failure).
- Function without `return`: no result; `execute store` touches nothing. Several functions (tag): sum of the results returned. Each result returned is announced `commands.function.result` to a non-silent source.
- Loading: lines joined by a trailing `\` (edge spaces removed like Java), `#` comments and empty lines ignored; a line starting with `/` (or `//`), continuation at end of file, macro without a variable or invalid variable name (letters, digits, `_`), command that does not parse at level 2 → whole function refused (log "Failed to load function").
- Macros: line `$…$(name)…`; call without arguments → `commands.function.instantiationFailure` (`commands.function.error.missing_arguments`); missing key → `…missing_argument` (first missing parameter in order of appearance); invalid substituted line → `…error.parse` (command, error text "… at position N: …<--[HERE]"). Arguments: SNBT compound, or `with block|entity|storage <source> [path]` (single path, compound otherwise `commands.function.error.argument_not_compound` with the type name `TAG_…`). Values: integers without suffix, floats in decimal without exponent (15 decimals at most, rounded half to even from the shortest representation), raw strings, the rest in SNBT.
- `execute if|unless function`: the first function that returns a result decides (≠ 0); without `return` → 0. `execute on vehicle|passengers|controller|origin|attacker|target|owner` (leash: not simulated by singleplayer, no entity).
- `scoreboard players operation`: `/=` and `%=` by 0 → `arguments.operation.div0` (previous targets already modified); floor division and remainder without overflow (`MIN %= -1` = 0).

### 6.2.2 Teleportation, facing, particles, experience (W11)
- `tp` to a position: relative flags sent to the player (X/Y/Z + DELTA velocity per `~` axis, all three for `^`, absent rotation = relative +0, rotation given relative per `~` component); difference transmitted for a relative axis. Then vertical velocity zero and on the ground (except gliding), navigation stopped. To an entity: all absolute. Limits: −30,000,000 ≤ x, z < 30,000,000, −20,000,000 ≤ y < 20,000,000.
- `facing` (tp, rotate): look direction computed from the SOURCE's anchor applied to the target (feet, or eyes after `execute anchored eyes`), look packet with this anchor (and the targeted entity). `rotate <target> <rotation>`: rotation resolved against the source, rotation-only packet for the player, pitch clamped to ±90.
- `particle`: options decoded like the game's codecs (integer colour or floats 0–1 → `floor(x×255)`, ARGB for `entity_effect`/`tinted_leaves`/`flash`, block state compound `Name`/`Properties` or identifier, item, scale 0.01–4, power, `roll`, `delay`); `delta` resolved against the source without recentring; visibility measured from the centre of the player's block (32, 512 forced).
- Experience: levels added saturated; below 0 → level, progress and total to 0; sound `entity.player.levelup` (volume 0.75 × min(level/30, 1)) when going up past a multiple of 5, at most every 100 ticks of the player (orbs, rewards, command). `xp set … points`: progress clamped to (needed − 1)/needed, total unchanged.
- `clear`: inventory in its internal order (hotbar, main storage, armour feet → head, off-hand), then the crafting grid, then the cursor; remainder to remove = maximum − already removed. `give`: stack entirely stored → dummy item (never pickable, age 5999) in front of the player; remainder (even after partial storage) dropped without delay, with no thrower. `item … with <item> <n>` beyond the maximum stack → `arguments.item.overstacked`.
- `gamerule`: value compared after reading (`03` = 3), short name in `commands.gamerule.not_set`. `setworldspawn`: yaw wrapped, pitch clamped. `time query <timeline> repetition` without a period → 0. `swing`: living entities only (`commands.swing.failed.notliving`), messages `commands.swing.success.*`, type (`none`/`whack`/`stab`) and duration transmitted.

### 6.3 Command blocks
- Impulse: rising edge of the power (if not automatic) → condition recorded, tick in 1. Repeating: tick on every tick while powered or automatic. Chain: executed by the previous block (orientation), if powered or automatic, chain stops if already executed this tick.
- Conditional: succeeds if the block behind (opposite to the orientation) is a command block with `SuccessCount` > 0; otherwise `SuccessCount` = 0.
- Source: block centre, rotation (0, yaw of the orientation), level 2, name "@" or custom name. `SuccessCount` = number of successful executions; comparator output = `SuccessCount`.
- "Chain" block automatic from its creation; placement by the player: output tracking = rule `send_command_feedback`, power checked. Use by an operator in creative: block-entity data sent (the screen is opened by the client), settings received through `set_command_block` (`advMode.setCommand.success`).

### 6.4 Commands added by S9 (`cmds_s9a.rs` … `cmds_s9d.rs`, `lan/mod.rs`)
- `advancement grant|revoke <targets> only|from|until|through|everything` (+ criterion for `only`): same "one / several" messages as the game (`commands.advancement.<action>.<mode>.to.<one|many>.<success|failure>`); `from` = the advancement and its descendants, `until` = its ancestors and itself, `through` = both.
- `stopwatch create|query|restart|remove`: real milliseconds; file `data/minecraft/stopwatches.dat`. `dialog show|clear`, `waypoint list|modify` (colour, style), `ride mount|dismount`, `spectate`, `spreadplayers` (random attempts like the game, teams together), `fillbiome` (chunks resent), `datapack list|enable|disable|create` (pack format 121), `reload`.
- `item replace|modify` (item modifiers from the jar and the world), `loot` all sources (`loot`, `kill`, `mine`, `fish`) and targets (`give`, `spawn`, `insert`, `replace block|entity`), `attribute … base get|set|reset` and `modifier add|remove|value get`.
- `place feature` (decoration placed over a region of 3×3 chunks, `rc-worldgen`), `place template` (templates from the jar and the world, rotation, mirror, integrity). `place structure` and `place jigsaw`: clean failure `commands.place.structure.failed` / `commands.place.jigsaw.failed` (structure generation outside world generation not yet wired).
- `execute if|unless predicate`, selector option `advancements={id=bool,id={criterion=bool}}`, command block minecart (command run every 4 ticks on a powered activator rail, source = the minecart).
- `publish [allowCommands] [port]`: reserved to the host (level 4); free port chosen by the system if absent; success `commands.publish.started.lan` (copyable port); already open → `commands.publish.alreadyPublished.lan`; listen failure → `commands.publish.failed`. `unpublish` always answers `commands.publish.failed` (closing is done through the world options).

## 7. Uncertain points

- (Resolved) Parsing library decompiled: § 2.2, 2.3, 3.1, 4.4 and the text sent in `command_suggestion` (input truncated at the cursor, `/` included) are verified.
- Item predicates, SNBT, `resource_or_id`: the game reads them with a backtracking grammar that keeps the errors and suggestions of the **furthest** failure point; our top-down reading returns the first dead end (identical in common cases, message sometimes different on a very invalid input).
- Values of item components, text components, styles, particle options: the game decodes them with their full formats (`arguments.item.component.malformed`, `argument.component.invalid`…); we only check the SNBT syntax.
- Shadow of the text of the suggestion list and of the usage lines: the drawing call used shadows by default in the other screens; not verified here.
- `hex_color`: a word of 3 or 6 **non-hexadecimal** characters (e.g. `ggg`) causes, on the client, a conversion error not anticipated by the parser (not a syntax error); exact effect (exception propagated, parse interrupted) not verified. Our client: treat as `argument.hexcolor.invalid`.
- Registry targeted by `resource*` absent on the client (unsynchronised registry): building the tree might fail; vanilla only sends known registries. Our client: treat the argument as an identifier string and ask the server for suggestions.
- `delete_chat` with the chat open and scrolled: the rebuild of the lines goes through the normal add, which may shift the scrolling (`chat.md` § 3.5); not verified.
- Exact list of commands of a non-operator: depends on the server (Paper adds its own); assume nothing, everything comes from the packet.
- Mouse click on the list: the hover selection only happens if the mouse moved; a click with no prior movement applies the line under the pointer (direct computation), so no visible difference.
