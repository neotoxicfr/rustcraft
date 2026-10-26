# Chat: network, display, input, interface scale (26.3)

Clean-room spec, written from a reading of the decompiled 26.3 client and server (kept outside the repository). The formats of a text component (NBT) and the splitting of a text into lines according to glyph width belong to `text.md`; here we only refer to it.

Conventions:
- Network types: `VarInt`, big-endian `i64`, `bool` (1 byte), `uuid` (2 × `i64`). "String(n)" = VarInt length in UTF-8 bytes then bytes, n = max length in characters (32767 if not specified).
- "Optional(X)" = `bool` present then X if true.
- "Component" = text component in **network NBT**: one unnamed root tag (type byte + payload). A plain string tag = literal text. Format details: `text.md`.
- "Signature" = **256 raw bytes**, without a length prefix.
- "GUI" coordinates = interface pixels after division by the interface scale (§ 5). Y axis downward, origin at the top left.
- "HUD tick" = integer counter incremented once per client tick (20/s) while the game is not paused (in multiplayer, never paused).

---

## 1. Packets received (play phase)

### 1.1 System message (`system_chat`)

| Field | Type |
|---|---|
| content | Component |
| overlay | `bool` |

- `overlay` true: the text goes to the **action bar** (§ 1.6), not to the chat.
- Otherwise: added to the chat with the "system" tag (§ 3.6), unless the chat visibility is "hidden".

Option `chatVisibility` (FULL by default): FULL = everything; SYSTEM ("commands only") = neither sending messages nor receiving player messages; HIDDEN = in addition, neither commands nor server system messages. Only messages produced by the client remain visible. Changing the option also refilters the history already displayed.

### 1.2 Player message (`player_chat`)

Exact field order:

| # | Field | Type |
|---|---|---|
| 1 | global index | VarInt |
| 2 | sender | `uuid` |
| 3 | index within the sender's session | VarInt |
| 4 | signature | Optional(Signature) |
| 5 | body: content | String(256) |
| 6 | body: timestamp | `i64`, milliseconds since the Unix epoch |
| 7 | body: salt | `i64` |
| 8 | body: last seen | VarInt n (≤ 20), then n × packed signature (below) |
| 9 | unsigned content | Optional(Component) |
| 10 | filter mask | VarInt type, then depending on the type (below) |
| 11 | chat type | `chat_type` registry reference (§ 1.3) |
| 12 | sender name | Component |
| 13 | target name | Optional(Component) |

**Packed signature**: VarInt `v`. If `v = 0`, 256 bytes of full signature follow. Otherwise it is index `v − 1` in the client's **signature cache** (§ 1.7).

**Filter mask**: type 0 = no filtering; 1 = fully filtered (nothing is displayed); 2 = partial, followed by a bit array (VarInt number of **bytes**, then the bytes; bit i = bit `i mod 8` of byte ⌊i/8⌋). In partial mode, each UTF-16 unit of the signed content whose bit is 1 becomes `#` (a character outside the basic plane counts as two), the runs of `#` being styled dark grey with the tooltip `chat.filtered`. Out-of-range type → 0.

**Client processing** (in this order):
1. Local counter `next_index` (0 at the start of the game session): if `global index ≠ next_index` → **disconnect** (missing or out-of-order message). Otherwise `next_index += 1`.
2. Unpack the last seen via the cache; an index that points to an empty slot → **disconnect** (invalid packet).
3. Push into the cache the last seen then the message's signature (if present) (§ 1.7).
4. Sender absent from the player list (`player_info_update`) → display the validation error (§ 1.2.1).
5. Validation: if the sender has **no chat session** (case of an offline server) and secure chat is not enforced, the message is **accepted without a signature**: the signature is forgotten, the message becomes "unsigned". If secure chat is enforced (and the client can verify keys), it is rejected → validation error. With a session, verification of the signature and of the chain (§ 1.8).
6. Displayed content = unsigned content if present, otherwise the literal text of the body. (Option "show only secure chat", false by default: ignores the unsigned content and hides unsafe messages.)
7. Decorated message = the chat type's decoration applied to the content (§ 1.3).
8. Mask: type 1 → nothing displayed; type 2 → the decoration is applied to the filtered text (step above) instead of the content.
9. Added to the chat with the trust tag (§ 3.6); ignored if the sender is blocked or if the player cannot receive player messages (visibility ≠ FULL).
10. Acknowledgement (§ 2.4) **only if the message keeps a signature** after step 5.

#### 1.2.1 Validation error
Line added = the chat type's decoration applied to the text `chat.validation_error` (red, italic), "error" tag. If a signature was present, it is counted as processed but **not displayed** (§ 2.4).

### 1.3 `chat_type` registry and decoration

Registry reference (field 11): VarInt `v`. `v ≥ 1` → entry at index `v − 1` of the `chat_type` registry **in the order received** during configuration (`registry_data`). `v = 0` → **inline** type: the full type follows ("chat" decoration then "narration" decoration).

A decoration (network, inline): String translation key; VarInt n then n × VarInt parameter (0 = `sender`, 1 = `target`, 2 = `content`; out of range → 0); style (same NBT encoding as a component, § `text.md`).

Decorating = translatable component (key, arguments = parameters resolved in list order), to which the decoration's style is applied. Resolution: `sender` → sender name (field 12); `target` → target name (field 13), or empty component if absent; `content` → the content.

Vanilla types (data to load from the received registry; default values for a bench):

| Key | Translation key (chat) | Parameters | Style |
|---|---|---|---|
| `chat` | `chat.type.text` (`<%s> %s`) | sender, content | — |
| `say_command` | `chat.type.announcement` (`[%s] %s`) | sender, content | — |
| `msg_command_incoming` | `commands.message.display.incoming` | sender, content | grey, italic |
| `msg_command_outgoing` | `commands.message.display.outgoing` | target, content | grey, italic |
| `team_msg_command_incoming` | `chat.type.team.text` (`%s <%s> %s`) | target, sender, content | — |
| `team_msg_command_outgoing` | `chat.type.team.sent` (`-> %s <%s> %s`) | target, sender, content | — |
| `emote_command` | `chat.type.emote` (`* %s %s`) | sender, content | — |

Narration: `chat.type.text.narrate` (sender, content) for all except `emote_command` (`chat.type.emote`). The templates come from `assets/minecraft/lang/<language>.json`.

### 1.4 Disguised message (`disguised_chat`)

| Field | Type |
|---|---|
| message | Component |
| chat type | `chat_type` reference (§ 1.3) |
| sender name | Component |
| target name | Optional(Component) |

Displayed as is after decoration, "system" tag (§ 3.6), no signature, no acknowledgement. Ignored if the player cannot receive player messages.

### 1.5 Deletion (`delete_chat`)

Single field: packed signature (§ 1.2). Unknown index → **disconnect**. Otherwise:
1. If the signature is still "pending" in the last-seen tracking (§ 2.4), its slot is emptied (it will not be acknowledged).
2. If the message is in the deferred queue (§ 3.8), it is removed from it; otherwise, in the chat's message history, the message bearing this signature is replaced by the marker `chat.deleted_marker` (grey, italic, "system" tag, same add time) — but only if at least **60 HUD ticks** have elapsed since it was added; otherwise the deletion is postponed and retried on every chat tick until the deadline.

### 1.6 Action bar and titles

- `set_action_bar_text`: one Component; same effect as an overlay `system_chat`.
- Action bar (display): text kept **60 ticks** (decremented per HUD tick). 8-bit alpha = `⌊(remaining − partial_tick) × 255 / 20⌋`, capped at 255, nothing if ≤ 0 (so full opacity for 40 ticks, linear fade over the last 20). Horizontally centred: origin `(guiWidth / 2, guiHeight − 68)`, text at `x = −text_width / 2`, `y = −4`, white with this alpha, shadowed. Rainbow colour (record "now playing"): hue `t / 50`, saturation 0.7, value 0.6 — only for the local "now playing" message, never for the network one.
- "Backdrop" (action bar, titles): only if the "text background everywhere" option is chosen (default: chat only → **no backdrop**). Otherwise rectangle `[x−2, y−2] → [x+w+2, y+9+2]` black with alpha = text background opacity × text alpha.
- Titles: `set_title_text` (Component), `set_subtitle_text` (Component), `set_titles_animation` (3 × `i32`: fade in, stay, fade out; a value < 0 leaves the current duration unchanged), `clear_titles` (`bool`: also reset the default durations). Default durations 10 / 70 / 20 ticks. Receiving a title starts the timer at fade in + stay + fade out; the subtitle alone starts nothing. At 0: title and subtitle cleared. Alpha: linear rise during fade in, 255 during stay, linear fall during fade out (with partial tick), clamped 0–255. Drawing: origin at the centre of the screen; title at ×4 scale, `x = −w/2`, `y = −10`; subtitle at ×2 scale, `x = −w/2`, `y = 5`; white, shadowed.
- HUD drawing order: … scoreboard, action bar, titles, **chat**, player list …; none of this if the interface is hidden (F1).

### 1.7 Signature cache (client)

128 slots, empty at the start, reset at each game session. "Pushing" a list L (the body's last seen, then the message's signature): queue = L; for i = 0, 1, … while the queue is not empty and i < 128: the old value of slot i is replaced by the **last** element of the queue (removed); the old value, if non-null and absent from L, is put back at the **head** of the queue. Effect: the entries of L occupy the first slots (the most recent at 0) and the rest slides.

Offline server: messages arrive without a signature and with empty last seen; the cache stays empty and is of no use, but it must still be implemented if a signing server is to be accepted.

### 1.8 Players' chat sessions and verification (client)

**Services keys**: at launch, the game reads the services' public key set (address `authentication/getPublicKeys` of the discovery document, fallback `https://api.minecraftservices.com/publickeys`, no token). Only the `playerCertificateKeys` keys (RSA 4,096 bits, X.509 in base 64) are used here. Empty set (services unreachable): "the client cannot verify".

**Secure chat enforced** = the client can verify **and** the `login` packet announces it.

**Per player** (created on addition in `player_info_update`, erased on `player_info_remove`), a validator:
- without a chat session: "accept unsigned" (signature removed → not secure), or "refuse everything" if secure chat is enforced;
- action "initialise chat": if the client cannot verify, or session absent → back to the no-session case. Otherwise the key's signature is verified with **SHA1withRSA** by one of the services keys, over `player uuid (16 bytes) ‖ expiry in ms (i64) ‖ DER X.509 public key`; failure → back to the no-session case; success → keyed validator.
- A public key that is not a readable X.509 RSA key makes the whole packet refused (disconnect).

**Keyed validator** (one per session): for each message, failure if the key expired more than 8 h ago, if the signature is missing or wrong (**SHA256withRSA** of the player's key over the signed content of the authentication § 3.4: version 1, player, session, session index, salt, timestamp in seconds, content, unpacked last seen), or if the session index is not strictly greater than that of the last accepted message (a message identical to the last one passes). The first failure invalidates the chain: all later messages of this session are refused (validation error, § 1.2.1).

**Trust level** (at display time): not secure if the message no longer has a signature or if it is received more than 7 min after its timestamp; modified if the decorated message's text does not contain the signed content, or if the unsigned content uses a font other than the default; otherwise secure (no tag). "Show only secure chat": the unsigned content is ignored for decoration, and non-secure messages are hidden.

---

## 2. Packets sent

### 2.1 Message (`chat`)

| Field | Type |
|---|---|
| message | String(256) |
| timestamp | `i64` Unix ms (current time) |
| salt | `i64` random (secure) |
| signature | Optional(Signature) — **absent** without a chat session |
| last seen: offset | VarInt |
| last seen: acknowledgements | fixed **3 bytes** = 20-bit field (bit i = byte ⌊i/8⌋, bit i mod 8) |
| last seen: checksum | 1 byte |

### 2.2 Command (`chat_command` and `chat_command_signed`)

- The text is sent **without the leading `/`**.
- `chat_command`: one String (32767). Used when the command, parsed by the received command tree (`commands`), has **no signable argument** (arguments of type "message").
- `chat_command_signed`: String command, `i64` timestamp ms, `i64` salt, VarInt n (≤ 8) then n × (String(16) argument name + Signature), then the same 3 last-seen fields as § 2.1. Without a chat session, the signature list is **empty** (n = 0) but the signed packet is still used if the command has signable arguments, and the offline server accepts it.
- Shortcut for a client without a command parser: **always** send `chat_command`. Verified on the server side: an unsigned command that contains signable arguments is refused (message `chat.disabled.invalid_command_signature`) **only** if the server enforces the secure profile; offline it is executed. The signed packet is only indispensable against a server that enforces signing.

### 2.3 Server checks (to avoid being kicked)

- Forbidden characters in a message or command: `§` (U+00A7), < 32, and 127 → **kick** ("illegal characters"). The client already filters input (§ 4).
- Chat visibility "hidden": the message (not the command) is refused by a red `system_chat` `chat.disabled.options`.
- Anti-spam: two separate counters (broadcast messages, commands), +20 per send, −1 per server tick; if the counter is ≥ `20 × threshold` after the addition → kick `disconnect.spam` (except operator or owner in singleplayer). Threshold = `chat-spam-threshold-seconds`, 10 by default (200); likewise for commands with their own property **[its default not verified]**. In practice: no more than one message per second on average, burst of 9 at most.
- Inconsistent last seen (offset greater than the pending messages, bit acknowledging an empty or ignored slot, already-acknowledged slot left at 0, bits beyond 20) → kick `chat_validation_failed`. Checksum 0 = not verified.
- More than **4096** pending signed messages on the server side → kick `too_many_pending_chats`. Concerns only **signed** messages: a client on an offline server is not exposed to it.

### 2.4 Last-seen tracking and acknowledgement (`chat_ack`)

Client state: circular array of 20 slots (empty), index `queue`, counter `offset`, last tracked signature. Reset at each game session.

- **Signed message processed** (displayed or not): if its signature equals the last tracked one, nothing. Otherwise it becomes the last tracked; slot `queue` ← (signature, "pending") if the message was **displayed**, otherwise empty slot; `queue = (queue + 1) mod 20`; `offset += 1`. Then if `offset > 64` → send `chat_ack`.
- `chat_ack`: one VarInt = current `offset`, then `offset = 0`. Only sent if `offset > 0`. Also sent when the server goes back to configuration (`start_configuration`).
- **Before each `chat` / `chat_command_signed`**: take `offset` (then set it to 0); walk i = 0..19 over slot `(queue + i) mod 20` (from oldest to most recent); each non-empty slot → bit i to 1, its signature added to the "last seen" list and the slot marked "acknowledged". Checksum of the list: `h = 1 ; for each signature: h = 31·h + hash(sig)` in signed 32-bit integer, `hash(sig) = 1 ; for each byte b (signed): 31·hash + b`; byte = low 8 bits of h; if 0 → 1.
- `chat_command` (unsigned) sends no last seen.

**Offline client (server `online-mode=false`, `enforce-secure-profile=false`)**: no chat session (no key pair, no `chat_session_update`), signature always absent. Since received messages are unsigned, none enters the tracking: `offset = 0`, acknowledgements = 3 null bytes, checksum = **1** (empty list), no `chat_ack` to send. Implementing the tracking anyway makes the client correct against a signing server.

---

## 3. Chat display (HUD)

### 3.1 Settings (defaults)

| Setting | Default | Effect |
|---|---|---|
| chat scale `s` | 1.0 (0–1) | drawing factor |
| width | 1.0 → `⌊p·280 + 40⌋` = **320** GUI px | 40–320 |
| height closed | `70/160 = 0.4375` → `⌊p·160 + 20⌋` = **90** px | 20–180 |
| height open | 1.0 → **180** px | |
| line spacing `sp` | 0.0 (0–1) | |
| text opacity `o` | 1.0 → effective opacity `o·0.9 + 0.1` = 1.0 | |
| background opacity `f` | 0.5 | line background |
| text background "chat only" | true | |
| message delay | 0 s (0–6 s by 0.1) | § 3.8 |
| chat colours | true | false = `§` codes removed before splitting |

### 3.2 Geometry

All the quantities below are in **chat units**; one chat unit = `s` GUI px. Drawing applies a scale `s`, then a translation `(+4, 0)` (in chat units).

- `L = ⌈width / s⌉` (max text width for drawing); for **splitting** into lines we take `⌊width / s⌋` (sic: two different roundings).
- `bottom = ⌊(guiHeight − 40) / s⌋`.
- Entry height `h = ⌊9·(sp + 1)⌋` (9 by default).
- Text offset `d = round(8·(sp + 1) − 4·sp)` (8 by default).
- Lines per page `N = ⌊height / h⌋`: **10** closed, **20** open (defaults).
- Line with visual index `i` (0 = the lowest): `bottom_i = bottom − i·h`, `top_i = bottom_i − h`, text at `(0, bottom_i − d)`.
- Line background: rectangle `x ∈ [−4, L + 8]`, `y ∈ [top_i, bottom_i]`, black with 8-bit alpha `⌊α·f·255⌋` (α = the line's alpha, § 3.3).
- Text: white with alpha `⌊α·(o·0.9 + 0.1)·255⌋`, **shadowed**, not drawn if this alpha is 0.
- In GUI px (defaults, `s = 1`): background from `x = 0` to `x = 332`, text at `x = 4`, bottom of the lowest line at `guiHeight − 40`, above the hotbar.

### 3.3 Fade (chat closed)

Age `a = hudTick − add_tick` (integers, no partial tick). `t = clamp((1 − a/200)·10, 0, 1)`; `α = t²`. So full opacity up to 180 ticks, then quadratic decrease down to 0 at 200 ticks (10 s). A line with `α ≤ 1e-5` is not drawn but **keeps its place** (indices do not close up). Chat open: `α = 1` for all lines.

Walk: draw lines `i = min(line_count − scroll, N) − 1` down to 0, stored line with index `i + scroll` (0 = the most recent). Two passes: first all backgrounds, then texts and indicators.

### 3.4 Messages and lines

- Message history: **100** at most (the oldest drops out). Split lines: **100** at most as well (independent).
- Each message is split at width `⌊width/s⌋` (minus 9 + 4 + 2 = 15 if its tag has an icon); each continuation line is prefixed with **one space** (indentation). Breaking algorithm: `text.md`. Empty message → one empty line.
- Lines are inserted at the head (the last line of the message is the lowest); the last line of a message is marked "end of entry".
- Changing width, scale, opacities: scrolling reset and re-splitting of the whole history.

### 3.5 Scrolling (chat open)

- Wheel: step = ±1 (clamped), ×7 except with Shift. Page Up: `+(N − 1)`; Page Down: `−(N − 1)`.
- `scroll` clamped to `[0, line_count − N]`; returning to 0 clears "new message".
- Message received with chat open and `scroll > 0`: `scroll += 1` per line added, "new message" flag raised.
- Scrollbar (if `line_count·h ≠ c·h`, `c` = lines drawn): `H = c·h`, `V = line_count·h`, `y = scroll·H / line_count − bottom`, height `H² / V`; two rectangles from `−y` to `−y − height`: `x ∈ [L+4, L+6]` colour `0xCC3333` if new message otherwise `0x3333AA`, then `x ∈ [L+5, L+6]` colour `0xCCCCCC`; alpha 170 if `y > 0`, otherwise 96.

### 3.6 Side indicator (tags)

Bar `x ∈ [−4, −2]`, `y ∈ [top_i, bottom_i]` on each line whose message carries a tag, tag colour with the text's alpha.

| Tag | Colour | When |
|---|---|---|
| system | `0xD0D0D0` | `system_chat`, `disguised_chat`, deletion marker |
| not secure | `0xD0D0D0` | player message without a valid signature (**all** messages on an offline server) |
| modified | `0x606060` + icon `icon/chat_modified` 9×9 | signed message whose decorated text does not contain the signed content, or whose unsigned content changes font |
| error | `0xFF5555` | § 1.2.1 |
| none | — | intact signed message (secure) |

Chat open: hovering the bar → tooltip (width ≤ 210) with `chat.tag.*`; the "modified" icon only appears on hovering the line or the icon, at `x = line_text_width + 4`, bottom = bottom of the text, top = bottom − 10.

### 3.7 Particularities
- The HUD chat is not drawn when the chat screen is open (the screen draws it, § 4).
- Chat restrictions (account): extra red underlined line `chat_screen.restricted` above the messages **[out of scope offline]**.

### 3.8 Deferred queue
If the delay > 0, messages are queued and released one at a time when `now ≥ last_display + delay`. Line `chat.queue` ("[+n pending line(s)]") below the chat, opacity ×0.5, on a background `x ∈ [−2, L+4]`, `y ∈ [bottom, bottom+9]`. Delay 0 by default: immediate display.

---

## 4. Input

- Keys: **T** (`key.chat`) opens in message mode; **/** (`key.command`, only if no screen is open) opens with `/` already typed. (Codes = SDL scancodes 23 and 56.) Draft: if the screen was closed by interruption (not by Escape / Enter), its text is restored on reopening (command mode: only if the draft was a command) and displayed in grey italic; Backspace on a draft clears it entirely.
- Field background: rectangle `[2, guiHeight − 14] → [guiWidth − 2, guiHeight − 2]`, colour `0x80000000` (or text background opacity if "everywhere").
- Borderless field: `x = 4`, `y = guiHeight − 12`, width `guiWidth − 4`, height 12; text at `(4, guiHeight − 12)`, colour `0xE0E0E0`, shadowed; horizontal text scrolling when it overflows.
- Max length **256** characters. Characters refused on input: `§`, < 32, 127.
- Cursor: visible if `⌊(ms since focus gained) / 300⌋` is even (blinks every 300 ms). At the end of the text (and length < max): character `_` drawn right after the text (x + width + 1); elsewhere or at max length: 1 px vertical bar, `[x−1, y−1] → [x, y+10]`, in the text colour.
- Send history: 100 entries max (no consecutive duplicate); up arrow = older, down = more recent, the "past the end" position gives back the text being edited. Commands are also kept on disk (`command_history.txt` in the game folder) and reloaded at startup.
- Enter: text = `trim`, consecutive spaces reduced to one, truncated to 256. Empty → nothing is sent. Otherwise added to the history; starts with `/` → command without the `/` (§ 2.2); otherwise `chat` (§ 2.1). The screen closes and scrolling returns to 0.
- Escape: closes without sending. Click on a chat text: click events (`text.md` § 3.6); Shift+click inserts the "insertion" text. Hover: tooltip; hover over the side bar: the tag's text (`chat.tag.*`).
- Field editing: Backspace / Delete (Ctrl: to the start / end of the word, spaces included), Home / End, arrows (Ctrl: by word; Shift: extends the selection), Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V (system clipboard; pasting replaces the selection and stops at 256). Mouse: click = cursor (Shift: extends), double click = word, drag = selection. Selection drawn inverted. IME input: composition displayed at the cursor, committed text inserted.
- Draft: Escape keeps it only if "save drafts"; a send discards it; a forced close (another screen) keeps it.
- Command suggestions (Tab, list above the field, background `0xD0000000`): see `commands.md` (tree, parsing, suggestions, colouring).

---

## 5. Interface scale (GUI scale)

Setting `guiScale`: 0 = automatic (default), otherwise a fixed value (capped by the automatic maximum).

```
s = 1
while s ≠ setting and s < fb_width and s < fb_height
      and fb_width / (s+1) ≥ 320 and fb_height / (s+1) ≥ 240 :   (integer divisions)
    s = s + 1
if Unicode font forced and s odd : s = s + 1
```
(In automatic mode the setting is 0, never reached: we go up as long as it fits.) `fb_width`, `fb_height` = size of the **framebuffer** in physical pixels. Then `guiWidth = ⌈fb_width / s⌉`, `guiHeight = ⌈fb_height / s⌉`. Examples: 854×480 → 2; 1920×1080 → 4; 2560×1440 → 6. Recomputed on every resize.

---

## 6. Resources to load

- `assets/minecraft/lang/en_us.json` (and the chosen language): `chat.type.*`, `commands.message.display.*`, `chat.deleted_marker`, `chat.validation_error`, `chat.filtered`, `chat.queue`, `chat.tag.*`, `chat.disabled.options`.
- Interface sprite `icon/chat_modified` (9×9).
- `chat_type` registry received in configuration (order = network ids).
