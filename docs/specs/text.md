# Text: fonts, components, layout and rendering (26.3)

Sources: reading of the official 26.3 client (kept outside the repository), notes in our own words. Complements `chunks.md`
(network NBT format). Unverified points are marked **[not verified]**.

Units: "px" = interface pixel before GUI scaling. Colours in 32-bit ARGB
(`0xAARRGGBB`). `ceil`/`floor` in the mathematical sense.

## 1. Resources to load

| Resource | Where | Role |
|---|---|---|
| `assets/minecraft/font/default.json`, `uniform.json`, `alt.json`, `illageralt.json` | jar | fonts named `minecraft:default`, `minecraft:uniform`, `minecraft:alt`, `minecraft:illageralt` |
| `assets/minecraft/font/include/space.json`, `include/default.json` | jar | blocks included by reference |
| `assets/minecraft/font/include/unifont.json` | **asset index 34** (the jar's file is an empty list, it is replaced) | two `unihex` providers |
| `minecraft/font/unifont.zip` (≈1.56 MB), `minecraft/font/unifont_jp.zip` (≈235 KB) | asset index 34 | unihex bitmaps |
| `minecraft/font/include/unifont_pua.json`, `unifont_pua.zip` | asset index 34 | not referenced by the vanilla fonts (useless unless a pack uses them) |
| `assets/minecraft/textures/font/ascii.png` (128×128), `accented.png` (144×900), `nonlatin_european.png` (128×536), `ascii_sga.png`, `asciillager.png` | jar | bitmap sheets (1-bit palettised PNGs, to be loaded as RGBA) |
| `assets/minecraft/lang/en_us.json`, `lang/deprecated.json` | jar | base translations, renamed/removed keys |
| `minecraft/lang/<code>.json` (e.g. `fr_fr.json`) | asset index 34 | other languages |
| `assets/minecraft/shaders/core/text.vsh`, `text.fsh` | jar | text shader |

The index's objects are in `%APPDATA%\.minecraft\assets\objects\<first 2 hex>\<sha1>`.
Index 34 is indeed that of version 26.3 (checked in the version JSON).

## 2. Fonts: definition files

A font = a JSON file `{"providers": [ … ]}`. Each provider has a `type` field and an
optional `filter` field (object `{option: boolean}`, see § 2.6).

### 2.1 Vanilla content

- `default`: `reference include/space`, then `reference include/default` with
  `filter {"uniform": false}`, then `reference include/unifont`.
- `uniform`: `reference include/space`, then `reference include/unifont`.
- `alt` (Standard Galactic Alphabet): `reference include/space`, then bitmap `font/ascii_sga.png`,
  ascent 7 (only the letters A–Z and a–z are mapped).
- `illageralt`: `reference include/space`, then bitmap `font/asciillager.png`, ascent 7.
- `include/space`: `space` with `" "` → 4 and U+200C → 0.
- `include/default`: three bitmaps in this order: `nonlatin_european.png` (ascent 7),
  `accented.png` (`height` 12, ascent 10), `ascii.png` (ascent 7).
- `include/unifont` (index 34): first `unihex` `font/unifont_jp.zip` with
  `filter {"jp": true}` and size overrides (U+3200–U+9FFF and U+F900–U+FAFF: left 0,
  right 15), then `unihex` `font/unifont.zip` with overrides: U+3001–U+30FF, U+3200–U+9FFF,
  U+1100–U+11FF, U+3130–U+318F, U+A960–U+A97F, U+D7B0–U+D7FF, U+F900–U+FAFF, U+FF01–U+FF5E
  → left 0, right 15; U+AC00–U+D7AF (hangul syllables) → left 1, right 15.
  The `__ranges` fields are comments, ignored.

### 2.2 `bitmap` type

Fields: `file` (identifier, prefixed `textures/` → `assets/<ns>/textures/<path>`),
`height` (default 8), `ascent` (required, error if `ascent > height`), `chars` (list of
strings; all must have the same number of code points, otherwise error; empty list =
error). Code points are read per code point (surrogate pairs grouped).

Loading:
1. Image loaded as 8-bit RGBA whatever the PNG format.
2. Cell: `cw = image_width / columns` and `ch = image_height / rows` (integer divisions).
3. `scale = height / ch` (float).
4. For each slot whose code point is not U+0000: real width `w` = index of the rightmost
   column containing at least one pixel with non-zero alpha, plus 1 (0 if the slot is empty).
   Scan from right to left, over the whole height of the cell.
5. Advance = `(int)(0.5 + w × scale) + 1` (truncation after +0.5).
6. If a code point appears twice in the same provider, the last wins (warning).

Glyph geometry (in px, origin = cursor position, y downward):
- left = 0; right = `cw × scale` (whole cell, empty columns included);
- top = `7 − ascent`; bottom = top + `ch × scale`.
- Examples: `ascii.png` → 8×8 cell, scale 1, top 0, bottom 8. `accented.png` →
  cell 12 high (900 / 75 rows), scale 1, top −3, bottom 9.

Shadow and bold offsets: 1 px.

### 2.3 `space` type

Field `advances`: object {single character → float advance}. Invisible glyph, given advance,
shadow/bold 1 (bold thus adds 1 to a space's advance).

### 2.4 `unihex` type

Fields: `hex_file` (identifier **without** prefix: `minecraft:font/unifont.zip` →
`assets/minecraft/font/unifont.zip`), `size_overrides` (optional list of
`{from, to, left, right}`, `from`/`to` = one character; error if `from >= to`).

Reading:
1. The file is a zip; each entry whose name ends in `.hex` is read (the others, including
   `LICENSE.txt`, ignored). Zip entry order; a later entry overwrites.
2. Line format: 4, 5 or 6 hexadecimal digits (code point), `:`, then 32, 64, 96 or 128
   hex digits, then `\n`. **Uppercase digits only** (`0-9A-F`), any other value =
   provider load error. Clean end of file = empty line without `:`.
3. The number of digits gives the matrix width: 32 → 8 columns, 64 → 16, 96 → 24,
   128 → 32; always 16 rows. Each pixel row is stored left-aligned in a 32-bit
   word (bit 31 = column 0).
4. Horizontal bounds: for the code points covered by an override, the override's `left`/`right`
   (columns inclusive, 0 = left). Otherwise automatic computation on the OR of the 16 rows:
   `left` = number of leading zeros, `right` = 31 − number of trailing zeros; if the matrix is
   entirely empty: `left = 0`, `right = matrix width` (quirk: one column more).
5. Width in texels `L = right − left + 1`. Bitmap sent: L × 16, lit pixel = opaque white
   (0xFFFFFFFF), unlit = 0; columns outside 0..31 → 0.

Metric: oversampling 2 → the glyph is displayed at `L/2 × 8` px; left 0, top 0
(baseline at 7 = implicit "ascent" 7), bottom 8. Advance = `floor(L / 2) + 1` (integer division).
Shadow and bold offsets: **0.5 px**.
Examples: overridden CJK glyph 0..15 → L 16, advance 9; hangul 1..15 → L 15, advance 8.

### 2.5 `reference` and `ttf` types

- `reference`: field `id` (font identifier, e.g. `minecraft:include/space` →
  `font/include/space.json`). Its providers are inserted at this point. Missing reference or
  cycle → the font that contains it fails entirely (warning).
- `ttf`: `file` (prefix `font/`), `size` (11), `oversample` (1), `shift` ([x, y], −512..512,
  default [0, 0]), `skip` (string or list of strings concatenated). FreeType, TrueType format
  required. **Unused by the vanilla fonts**; detailed rendering **[not verified]**, not to be
  implemented until a pack requires it.

### 2.6 Filters and options

Two options: `uniform` ("force Unicode font" setting) and `jp` ("Japanese glyph variants"
setting). A filter passes if, for each key present, (option active) == value.
No filter = always passes. A filtered reference merges its filter with that of each
included provider; on a common key, **the reference's filter wins**.
Vanilla consequences: "Unicode forced" removes the three bitmaps of `default` (everything goes through
unifont); "jp" activates `unifont_jp` in front of `unifont`. Changing an option reloads the glyph sets
(cache emptied, atlases rebuilt).

### 2.7 Character lookup order

- A font's provider list = JSON order, references expanded in place. With several
  resource packs: the providers of the highest-priority pack come first.
- Active providers = those whose filter passes, and which are the first provider of at least one
  code point (the others are removed, no visible effect).
- Lookup of a code point: first active provider that knows it. Result cached.
- So in `default`: space (advance 4) and U+200C come from `space`; then
  `nonlatin_european`, `accented`, `ascii`, then unifont.
- "Filtered" variant (restricted-use font, e.g. editable texts **[not verified: where it
  is used]**): a glyph is "suspicious" if its normal or bold advance is < 0 or > 32; we then
  take the first following non-suspicious provider, otherwise the missing glyph.
- Unknown font (absent id) → "missing" font: all characters = missing glyph.

### 2.8 Special glyphs

- Missing: 5×8 image, opaque white frame of 1 px (edges), transparent interior; advance 6;
  left 0, top 0 (baseline 7), bottom 8; shadow/bold 1. Visible on screen.
- White: 5×8 entirely opaque white, advance 6; used for effects (underline, strikethrough, background).
  Taken from the font `minecraft:default`.

## 3. Text components (network format)

### 3.1 Encoding

On the network: an anonymous NBT tag (`chunks.md` format). Decoding:
- **string** → literal text without style;
- **non-empty list** → copy of the first element, then each following element added as a
  child (`extra`) of this first one; the following ones thus **inherit the first one's style**;
  empty list = error. Compound elements `{"": x}` of a heterogeneous NBT list: unwrapped to `x`
  on reading (verified);
- **compound**: content + `extra` (non-empty list of components, optional) + style fields.

Content: if the `type` field is present, it chooses: `text`, `translatable`, `keybind`,
`score`, `selector`, `nbt`, `object`. Otherwise, detection by field present: `text` (string),
`translate`, `keybind`, `score`, `selector`, `nbt`, then `object`; first that decodes
**[not verified: exact order, no effect on a well-formed component]**.

| Content | Fields | Client-side display |
|---|---|---|
| text | `text` | the string |
| translation | `translate`, `fallback` (optional), `with` (optional list) | § 3.4 |
| key | `keybind` | name of the bound key **[not verified: labels]** |
| score | `score: {name, objective}` | nothing (must be resolved by the server) |
| selector | `selector`, `separator` | the raw selector string |
| nbt | `nbt`, `interpret`, `plain`, `separator`, source | nothing (server-resolved) |
| object | `object: "atlas"` + `atlas` (default block atlas) + `sprite`, or `object: "player"` + `player` + `hat` (default true); `fallback` | one U+FFFC character drawn with a sprite "font": 8×8 px square, advance 8, from y−1 to y+7 |

### 3.2 Style fields

`color` (string), `shadow_color` (ARGB integer, or list `[r, g, b, a]` of floats 0–1 converted
by `floor(v × 255)`), `bold`, `italic`, `underlined`, `strikethrough`, `obfuscated` (booleans,
NBT byte ≠ 0 = true), `click_event`, `hover_event`, `insertion` (string), `font` (font identifier,
e.g. `minecraft:alt`). Each field is **tri-state**: absent = inherited.

Named colours (RGB): black 000000, dark_blue 0000AA, dark_green 00AA00, dark_aqua 00AAAA,
dark_red AA0000, dark_purple AA00AA, gold FFAA00, gray AAAAAA, dark_gray 555555, blue 5555FF,
green 55FF55, aqua 55FFFF, red FF5555, light_purple FF55FF, yellow FFFF55, white FFFFFF.
`#` + hexadecimal: value read in base 16 (free length), must be within 0..0xFFFFFF, otherwise
error; unknown names = error (the whole component fails to decode).

### 3.3 Inheritance and flattening

Depth-first walk: the node's content, then its `extra` children in order. A node's effective style
= each field of the node if defined, otherwise that of the parent (root = empty style).
The result is a sequence of segments (style, string). An absent font is `minecraft:default`.

### 3.4 § codes (legacy)

Always interpreted at render time and in width computations, inside **each segment**:
- `§` + code (case-insensitive): `0`–`9`, `a`–`f` = colour (order of the table above,
  black = 0 … white = f) **and reset to false** of bold, italic, underlined, strikethrough, obfuscated;
  `k` obfuscated, `l` bold, `m` strikethrough, `n` underlined, `o` italic (cumulative); `r` = back
  to the segment's style (not to the empty style).
- unknown code: `§` and the following character are swallowed with no effect; trailing `§` ignored.
- the § state does not spill over onto the next segment.
- invalid surrogate pair → U+FFFD.
- Chat: if the "chat colours" setting is disabled, the codes `§[0-9a-fk-or]` are removed
  from the text before display.

### 3.5 Translation

- Table = `en_us.json`, then the chosen language on top (keys overwritten), taking
  packs into account. Then `deprecated.json`: `removed` keys deleted; for each `renamed` old →
  new, the old key's value is removed and put under the new one (if the old one
  does not exist, the new one is deleted too).
- On loading, any sequence `%[n$][digits/dots]d` or `…f` is rewritten as `%[n$]s`.
- Template = translation of the key; if absent: `fallback` if it exists, otherwise **the key itself**.
- Splitting: `%s` = next argument (sequential counter), `%n$s` = argument n (1-based, does not
  change the counter), `%%` = `%`. Any other `%` (non-`s` format, isolated `%`, trailing `%`),
  or argument index out of bounds → the whole template is displayed as is.
- Argument: component (displayed with its style inherited from the translation), otherwise number/boolean/string
  converted to text (`null` → "null"). A purely textual argument component without style is
  reduced to its string. NBT boolean received as a byte → "1"? **[not verified]**
- The template pieces are text to which the § codes apply.

### 3.6 Click, hover, insertion

These three fields are inherited like the others and survive § codes (colour, `r`).

- `click_event` = `{action, …}`. Actions accepted from a server: `open_url` (`url`, scheme
  `http`/`https` required), `run_command` and `suggest_command` (`command`, characters allowed in
  chat), `change_page` (`page` > 0), `copy_to_clipboard` (`value`), `show_dialog` (`dialog`:
  identifier or content), `custom` (`id`, optional NBT `payload`). `open_file` is refused.
- `hover_event` = `{action, …}`: `show_text` (`value`: component), `show_item` (`id`, `count`,
  `components`), `show_entity` (`id`, `uuid`: 4 integers or text, optional `name`).
- `insertion`: text inserted at the chat cursor by Shift + click.

Effects of a click (chat, book; the death screen only accepts links):
- link: nothing if "links" is disabled; otherwise confirmation screen if "confirm links"
  (title `chat.link.confirm`, address below wrapped at L − 50, warning
  `chat.link.warning` in `0xFFCCCC`, 100 px buttons spaced by 4: `gui.yes` opens the
  browser, `chat.copy` copies, `gui.no`; Escape = no; centred column, spacing 8, buttons
  16 px lower), otherwise direct opening; back to the original screen;
- command: parsed with the received tree; sent as is (`chat_command`) if it is
  complete and valid, without signable argument, and also valid without the restricted nodes; otherwise
  `multiplayer.confirm_command.title` screen with the message `…parse_errors`,
  `…signature_required` or `…permissions_required` (command in yellow) and two
  150 px buttons: "run" (or, for a signable command, "copy" from the chat and
  "suggest" from a book) and `gui.back`. After sending: the chat stays open, a book closes;
- suggestion: replaces the chat field's text; copy: system clipboard;
- dialog: opened as `show_dialog`; `custom`: `custom_click_action` packet (the internal identifier
  `minecraft:internal/expand_chat_queue` releases a message from the chat queue).

Hover (open chat, book, death cause): tooltip near the mouse; text wrapped at
`max(L/2, 200)`; item: name (rarity colour, italic if `custom_name`), `lore` in purple
italic, identifier in dark grey with advanced tooltips; entity: only with advanced
tooltips (name, `gui.entity_tooltip.type`, UUID).

## 4. Measuring and wrapping (HUD, chat)

- Line height: **9 px**. Height of a wrapped block = 9 × number of lines.
- Advance of a character = glyph advance (the style's font), + bold offset (1 or 0.5) if
  bold. § codes do not count. Obfuscated: advance of the real character.
- Width of a string = `ceil(sum of the float advances)`.
- GUI centring: `x − width / 2` (integer division).
- Head by width (truncation): characters are added as long as `remaining = max − Σadvances ≥ 0`.
- Ellipsis: head by width `max_width − width("...")`, then append "..." (three ASCII
  dots).

Wrapping into lines at width `M` (`M` brought to a minimum of 1), on the flattened text:
1. Advances are accumulated character by character, positions counted in the raw string (§ codes
   included).
2. `\n`: immediate break at its position.
3. A space stores "last space" (position and style) **before** adding its advance.
4. If the total exceeds `M` (strictly) and a character with non-zero advance has already been accepted
   on the line: break at the last space if there is one, otherwise just before the current character
   (word too long cut in the middle). The first visible character is always accepted.
5. If the character at the break is a space or `\n`, it is removed (a single one); otherwise nothing.
6. The remainder restarts with the style in force at the break (§ state included). A text ending in
   `\n` produces a final empty line.
7. Chat: width `M = floor(floor(width% × 280 + 40) / chat_scale)`; each line resulting from a
   wrap (not from a `\n`) is prefixed with a space (advance 4). Chat entry height:
   `(int)(9 × (line_spacing + 1))`.

## 5. Drawing a string

Inputs: position `(x, y)` (top-left corner of the line), base ARGB colour, shadow yes/no,
background colour (alpha 0 = none). In the GUI, a text with colour alpha 0 is not drawn at
all (trap: `0xFFFFFF` without alpha is invisible, white is `0xFFFFFFFF`), and the shadow is
enabled by default.

Logical → visual order (§ 5.1 below); no effect for Latin script. Characters are
grouped into **runs of identical style**;
a character's position is its index within its run.

For each character, cursor `cx` (start `x`), `cy = y`:
1. Glyph = lookup § 2.7. If obfuscated and character ≠ space (U+0020): glyph drawn at random
   (on each preparation, so every frame) among the font's code points whose
   `ceil(normal advance)` equals that of the real glyph (spaces and empty glyphs included, missing
   glyphs excluded); no candidate → missing glyph.
2. Colour: if the style has a colour, `(base alpha) | style RGB`, otherwise the base colour.
3. Shadow: if the style's `shadow_color` is defined: if the text alpha is 1, this colour as is,
   otherwise same RGB with alpha `floor(text α × shadow α × 255)`. Otherwise, if the shadow is
   enabled: each RGB channel × 0.25 truncated, alpha kept (white → 0x3F3F3F). Otherwise none.
   A shadow colour of 0 = no shadow.
4. Advance `a` = glyph advance (+ bold offset if bold); the glyph's shadow offset `d`
   (1, or 0.5 for unihex); bold offset `g` (same).
5. Glyph quad: x from `cx + left` to `cx + right`, y from `cy + top` to `cy + bottom`.
   - Italic: the top vertices are shifted in x by `1 − 0.25 × top`, the bottom ones by
     `1 − 0.25 × bottom` (8 px glyph: +1 at the top, −1 at the bottom).
   - Bold: the quad is enlarged by 0.1 px on each side and at top/bottom, and drawn twice:
     at `cx` and at `cx + g`.
   - Order of draws for a character: shadow (`+d, +d`), bold shadow (`+g+d, +d`), text,
     bold text (`+g`).
6. Strikethrough: rectangle from `x0` to `cx + a`, y from `cy + 3.5` to `cy + 4.5`; underline: y from `cy + 8`
   to `cy + 9`. `x0 = cx − 1` for the first character of a run, otherwise `cx`. Text colour,
   with shadow offset by `d` if shadow.
7. `cx += a`.

Background: rectangle from `x − 1, y − 1` to `max(cx)` and `y + 9`, background colour, drawn first.
Global order of a text: background, then all the glyphs (in order, each shadow then face), then
all the effects (strikethrough/underline, each shadow then face). No rounding of positions: float
coordinates (the GUI transform does the rest); only the bounding rectangle is rounded
(`floor` left/top, `ceil` right/bottom) for clipping (scissor).

8-direction outline (glowing text of signs): 8 copies offset by ±`d` in x and y, outline
colour, without effects **[usage not verified]**.

### 5.1 Bidirectional text and Arabic letters

"Bidirectional" language: field `bidirectional` of its declaration (`pack.mcmeta`, `language`
section); English is not. Component drawn (always, whatever the language):
1. the line's text flattened (§ codes applied), shaped by the Unicode library with only the
   "letters" option: each Arabic letter takes its presentation form (isolated, final,
   initial, medial) according to its neighbours, lam + alef become a ligature (the text gets shorter);
2. Unicode bidirectional algorithm, paragraph direction = first strong letter, otherwise right
   to left in a bidirectional language, left to right elsewhere;
3. runs laid out in visual order; a right-to-left run is read backwards, each
   character replaced by its mirror (`(` ↔ `)`, `[` ↔ `]`…).
Style of an output character = style of the input character of the same UTF-16 rank: after a
ligature, the following styles are shifted by one rank (quirk reproduced).
Plain string (not a component): same processing, but only in a bidirectional
language. Input field: typed text drawn as is, never reordered.
Width of a centred component: that of the visual line (forms and ligatures included).
Click and hover: style looked up in the visual line, at the x positions where each character is
drawn. Chat, continuation line: the line is reordered alone, then the indent (one space) is
put in front, hence always on the left. Sign editor, bidirectional language: the line is
replaced by its shaped and reordered string, centred on the width of that string,
cursor at the width of its first `cursor` characters; drawing this string
reorders it once more (quirk reproduced).

### 5.2 Objects in text

The character carrying an object is never replaced by its fallback text when drawing (the fallback
is only for plain text). Atlas sprite: one of the game's 12 atlases (banners, blocks, items,
chests, decorated pots, interface, map decorations, paintings, particles, shield patterns,
shulker boxes, celestial bodies), sprite looked up in the sources of `atlases/<atlas>.json`; sprite
absent: missing texture; unknown atlas: missing glyph (advance of that glyph). Whole image
stretched over 8 × 8 (no nine-slice). Head: face (8, 8, 8 × 8) of the profile's skin, then
hat (40, 8) on top if `hat`; default skin of the identifier (otherwise of the offline
identifier of the name) while waiting for the profile's skin. Top at `y − 1`, shadow offset by 1, advance 8
(+1 if bold, a single draw).
Head profile, read and rewritten like the game: bare username (16 UTF-16 units at most, characters
33 to 126), or compound `name`, `id` (4 integers; a string is refused), `properties` (list
`{name, value, signature}` or table `{name: [values]}`, 16 at most, rendered as a list), `texture`,
`cape`, `elytra` (identifiers), `model` (`wide`, `slim`). Known field invalid: component
refused; unknown fields removed; name and identifier present: `id` written first.
Plain text (creative and recipe book search, copy): the object's fallback text.

## 6. GPU rendering

- Atlas per font: 256×256 pages, added on demand, named after the font. RGBA8 format
  for all vanilla sources (bitmap loaded as RGBA, unihex white RGBA); a monochrome R8
  page only exists for TTF ("grey" pipeline: texture read `.rrrr`). Free placement
  (allocation tree, margin 1 texel); only the UV rule matters: `u0 = (x + 0.01)/256`,
  `u1 = (x − 0.01 + width_texels)/256`, same for v.
- Sampling: nearest neighbour, no mipmap; in the GUI clamped at the edge.
- GUI vertices: position, UV, colour (full light ignored); quads; z = 0 ("flat" mode).
- Shader: `colour = texture × vertex_colour × modulator (1,1,1,1)`; **discard if alpha < 0.1**;
  no fog in the GUI.
- GUI blend: `src·αs + dst·(1−αs)` for colour, `αs + αd·(1−αs)` for alpha; no depth test
  nor write. The glyphs of a GUI layer are drawn after the other elements
  of that layer, in order of addition (no sorting).
- Text in the world (names, signs): same shader with fog and light; variants
  "normal" (standard depth, shadow/text z offset by 0.03 and bold +0.001), "see-through"
  (no depth, no light) and "polygon offset"; details **[not verified]**,
  outside the HUD.
