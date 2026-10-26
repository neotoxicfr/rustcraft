# World interactions (26.3)

Source: reading of the decompiled client and server (kept outside the repository), reworded. **Client** side:
what the client computes, predicts, sends, and when. The server is only described where its answer
conditions the client.

Cross-references: aiming and ranges → `targeting.md`; sending `set_carried_item` → `hud.md` § 6.2;
attack indicator → `hud.md` § 3; arm swing (`swing_animation`, duration, Haste /
Mining Fatigue) → `entities.md` § 1.10 (rendering); first-person hand, swap counter, "bounce"
of the hand after use → `hand-camera.md` § 2.1; block sounds (hit, broken, placed) →
`sounds.md` § 5.2–5.3; mining particles (events 2019/2020, helper F) and 2001 →
`particles.md` § 1.4; encoding of the components (`tool`, `can_break`, `consumable`,
`blocks_attacks`, `attack_range`…) → `item-components.md`.

Vocabulary: "tick" = client tick (20/s). "Targeted block", "targeted entity" = aiming result
of the tick (`targeting.md` § 1, recomputed at the start of each client tick before the keys).

---

## 0. Packets involved

Server-bound (fields in encoding order):

| Packet | Fields |
|---|---|
| `player_action` | `action` varint (0 start mining, 1 face change, 2 abort, 3 finish mining, 4 drop the stack, 5 drop one item, 6 release use, 7 swap hands, 8 jab); `pos` packed position; `face` unsigned byte (0 down, 1 up, 2 north, 3 south, 4 west, 5 east); `sequence` varint |
| `use_item_on` | `hand` varint (0 main, 1 offhand); `pos` position; `face` varint (same numbers); `cx, cy, cz` 3 × f32 = hit point − block corner; `inside` bool; `world_border` bool; `sequence` varint |
| `use_item` | `hand` varint; `sequence` varint; `yaw` f32; `pitch` f32 (the player's current rotation) |
| `attack` | `entity_id` varint (**new in 26.3**: the attack is no longer a variant of `interact`) |
| `interact` | `entity_id` varint; `hand` varint; `point` `vec3lp` (`entities.md` § 1.0.1) = aimed point − entity position (feet); `sneaking` bool (sneak key held) |
| `punch` | no fields. Replaces the old server-bound swing packet: it signals a **left click**; the server makes the arm swing (seen by others) and resets the attack cooldown |
| `spectator_action` | optional entity id, encoded as a varint `id + 1` (0 = no entity) |
| `pick_item_from_block` | `pos` position; `with_data` bool |
| `pick_item_from_entity` | `entity_id` varint; `with_data` bool |
| `set_carried_item` | slot `i16` (`hud.md` § 6.2) |

There is **no** server-bound swing packet for the right click: when the server
accepts a use, it makes the arm swing itself (broadcast to the others; also sent back to the player
if the client had not predicted the swing, see § 3.4).

Client-bound: `block_changed_ack` (`sequence` varint), `block_update`, `section_blocks_update`,
`block_destruction` (`id` varint, `pos`, `stage` unsigned byte), `level_event`,
`swing_animation`, `set_held_slot`, attribute updates, entity data (use
flags).

### 0.1 Sequence numbers and prediction (common to § 1 and § 3)

- The client keeps a sequence counter per world (starts at 0 when the client world is created,
  so the first one sent is 1).
- **Predicted action**: the counter is incremented, "prediction" mode is opened, the local
  action is executed, the packet is built **with this number** and sent, then the mode is
  closed. Predicted actions: `player_action` start / finish mining (survival and creative),
  `use_item_on`, `use_item`. The other `player_action`s (abort, face change, drop,
  release, swap, jab) go out with sequence 0 and do not touch the counter.
- During prediction mode, **each locally changed block** is remembered:
  `pos → (current sequence, old state, player position at the time of the change)`. If there is
  already an entry for this position, only its sequence is updated (the original server state
  is kept).
- `block_update` / `section_blocks_update`: if an entry exists for the position, the received state
  replaces the entry's **remembered state**, **without** touching the displayed world; otherwise it is applied
  normally.
- `block_changed_ack(s)`: for each entry with sequence ≤ s: it is removed, and if the world's state
  differs from the remembered state, the world takes the remembered state (= last known server state).
  If in addition the local player then collides with this new state **and** no
  teleport (`player_position`) was accepted after this sequence, the player is moved back
  to the remembered position.
- The server sends only one acknowledgement per tick (the largest number received during the tick, sent at
  the start of its next tick), after any correcting `block_update`s. A server refusal
  therefore always translates to: `block_update` (true state) then `block_changed_ack`.

---

## 1. Breaking a block

### 1.1 Client mining state

`active` (bool), `mining_pos` (start (−1, −1, −1)), `mining_item` (copy of the main-hand stack
at the start), `progress` (f32), `mining_ticks` (f32, informational), `delay` (int, ticks),
`mining_face`. The **same item** = same item and same components (damage included: a stack that
wears out changes components, see Uncertain points).

`stage()` = `progress > 0 ? floor(progress × 10) : −1`.

### 1.2 Mining speed (per tick)

`gain(state)` = 0 if the state's hardness is −1 (unbreakable); otherwise
`player_speed(state) / hardness / (right_tool ? 30 : 100)`, where `right_tool` = the state does not require
a tool for its drops **or** the main-hand item is "correct" (below). Hardness 0 →
division by zero → +∞ (instant break); reproduce with IEEE floats.

`player_speed(state)` (stack = selected hotbar slot):

1. `v` = tool speed: if the stack has a `tool` component, first rule (in order) that
   **has** a speed and whose block set contains the block → that speed; otherwise
   `default_mining_speed`. Without a `tool` component: 1.
2. If `v > 1`: `v += attribute mining_efficiency` (Efficiency gives `level² + 1` through an
   attribute modifier carried by the item).
3. Haste or Conduit Power present: `a = max(Haste amplifier, Conduit
   amplifier)` (0 for an absent effect); `v ×= 1 + (a + 1) × 0.2`.
4. Mining Fatigue of amplifier `f`: `v ×= 0.3^(f + 1)` (computed in double then f32).
5. `v ×= attribute block_break_speed` (default 1).
6. If the **eyes** are in water: `v ×= attribute submerged_mining_speed` (default 0.2;
   Aqua Affinity adds +400% of the total → 1.0: this is the enchantment's only path).
7. If the player is **not on the ground**: `v /= 5`.

"Correct": `tool` component present and first rule (in order) that **has** a
`correct_for_drops` and contains the block → that value; none → false. Without `tool`: false.

Data needed: hardness and "requires a tool" per block (to extract from the jar, not yet in
`rc-data`), block sets of the rules (synced tags or direct lists of the component).

Attributes: the client uses the values received from the server (attribute updates); it does **not**
recompute equipment modifiers itself. After a slot change, the speed
therefore uses the old value of `mining_efficiency` until the server update (vanilla
behavior, to reproduce).

### 1.3 Per-tick loop (survival and adventure)

Order in the client tick (screen closed, no overlay): aiming → keys (drop, swap,
clicks… § 6) → **continuous mining** → decrement of `miss_delay`.

**Left click (each press consumed)**, refused if `miss_delay > 0`; if the hands are busy
(piloting a boat) nothing is done; spectator → § 1.7. Otherwise, if the held item has a
`minimum_attack_charge` not reached (`(attack_counter + 0) / attack_delay < threshold`, see § 4.1)
→ nothing. If the item has `piercing_weapon` → jab (§ 4.4), end. Otherwise depending on the aim:
- **entity** → attack (§ 4.1);
- **non-air block** → `start_mining(pos, face)`; if the block is air right afterwards (instant
  break), this tick's continuous mining is called with "button released";
- **air block or nothing** → `miss_delay = 10` (outside creative) and reset of the attack
  and swap counters.
Then, in all these cases: local swing of the main hand (the item's `attack_animation`
animation, default `whack` 6 ticks) and sending of `punch`.

**Continuous mining** `continue(held)` with `held` = no screen ∧ no instant break
this tick ∧ attack button held ∧ mouse captured:
- if `¬held`: `miss_delay = 0`;
- if `miss_delay ≤ 0` and the player is not using an item, and the held item does not have
  `piercing_weapon`:
  - if `held` and the aim is a non-air block: `continue_mining(pos, face)`; if it returns
    true → local swing (attack animation) **and** `punch` (so one `punch` per tick as long as
    we mine, delays included);
  - otherwise (`¬held`, or aim at entity / nothing) → `stop_mining()`. (Aim at an air block
    with the button held: nothing.)

**`start_mining(pos, face)`** (mining side):
1. Refusal (returns false, nothing sent) if restricted action (§ 1.6) or `pos` outside the world
   border.
2. Creative → § 1.5.
3. Otherwise, if `¬active` or different target (`pos ≠ mining_pos` or item different from
   `mining_item`):
   - if `active`: send `player_action` **abort** (pos = old `mining_pos`, face = the
     new face, sequence 0);
   - predicted action (§ 0.1): if the block is not air and `progress == 0`, apply the block's
     "hit" effect on the client side (no effect for almost all blocks; redstone ore:
     lights up and emits its particles); if the block is not air and `gain ≥ 1` → **local break**
     (§ 1.4); otherwise `active = true`, `mining_pos = pos`, `mining_face = face`,
     `mining_item = stack in hand`, `progress = 0`, `mining_ticks = 0`, local crack (§ 2) with
     `stage()` (= −1 here, so nothing displayed), then **hit effects**: the block's "hit" sound
     (volume `(v+1)/8`, pitch `h × 0.5`, `sounds.md` § 5.3) and a mining particle on the face
     (helper F of `particles.md`); packet `player_action` **start** (pos, face, sequence).
   - Note: the instant break in survival **does not set** a `delay` → with a gain ≥ 1
     held, one block per tick.
4. Returns true (even if nothing was done because it is already the current target).

**`continue_mining(pos, face)`**:
1. Optional sending of `set_carried_item` (`hud.md` § 6.2).
2. If `delay > 0`: `delay −= 1`, returns true.
3. Creative (and `pos` inside the border) → § 1.5.
4. If same target (pos and item):
   - block became air → `active = false`, returns false;
   - `progress += gain(state)` (recomputed each tick: effects, ground, water…), `mining_ticks += 1`;
   - if `progress ≥ 1`: `active = false`; predicted action: local break (§ 1.4) + `player_action`
     **finish** (pos, face, sequence); then `mining_face = none`, `progress = 0`, `mining_ticks = 0`,
     **`delay = 5`**;
   - otherwise, if `face ≠ mining_face`: `mining_face = face`, send `player_action` **face
     change** (sequence 0);
   - in both cases: local crack with `stage()` (after a break: −1 → removed); returns
     true.
5. Otherwise → `start_mining(pos, face)`.

No periodic hit sound on the client side: the "hit" sound every 4 ticks and the mining particles of the
following ticks come from the server (`level_event` 2020 on mining ticks that are multiples
of 4, 2019 otherwise, sent to everyone including the miner, see `sounds.md` § 5.3 and `particles.md`).

**`stop_mining()`**: if `active`: `player_action` **abort** (pos = `mining_pos`, face =
down, sequence 0), `active = false`, `progress = 0`, local crack removed (−1), attack
and swap counters reset.

Delay between two blocks: 5 ticks after a break completed in survival, during which
`continue_mining` only counts down (and the arm swings, `punch` goes out). A break by gain
≥ 1 at the start imposes no delay.

Interaction with the right click: a right click is ignored as long as `active` is true (§ 3.1).

### 1.4 Local (predicted) break

Refused (nothing changes, the packet goes out anyway) if: restricted action (§ 1.6); the item in
hand cannot break in creative (§ 1.5); "game master" block (command block, structure block,
jigsaw, test block…) without sufficient operator rights; air block.
Otherwise: "the player is about to break" effects of the block on the client side — at minimum event 2001 played
locally (the "broken" sound + particles, `sounds.md` § 5.3 / `particles.md`); then the block is
replaced by the "fluid only" state of its fluid (air, or source water for a waterlogged block), with
update of the neighbors; this change is remembered by the prediction (§ 0.1).

### 1.5 Creative

- `start_mining` in creative: predicted action = local break (§ 1.4) + `player_action` **start**
  (sequence); then `delay = 5`. No crack, no hit effect.
- `continue_mining` in creative (after the `delay` countdown): same (break + **start**),
  `delay = 5`. Effective rhythm when holding the button: one break every **6** ticks (the break
  tick + 5 ticks of countdown).
- Items that do not break in creative: `tool` component with `can_destroy_blocks_in_creative =
  false` (vanilla: all swords, the mace, the trident, component value sent by the
  server); and the debug stick (rule specific to the item). The local break is refused but the
  **start** packet is still sent (the server will refuse and send the block back); the arm
  swings and `punch` goes out.
- Attacking an entity in creative sets `delay = 5` (no break right after a hit).
- No `miss_delay` on a click into the void in creative.

### 1.6 Adventure, spectator: restricted action

`restricted(pos)`: false in survival and creative; true in spectator; in adventure: false if the
player has the "may build" ability (rare), otherwise true if the main hand is empty or if
the stack has no `can_break` that accepts the block at `pos` (predicates of `item-components.md`,
"at least one predicate verified").
Consequence: in adventure without a suitable `can_break`, `start_mining` returns false without sending anything
(but the arm swings and `punch` goes out, § 1.3).

### 1.7 Spectator

Left click: targeted entity → `spectator_action` with its id (the server moves the camera into
the entity if it is selectable and in range); otherwise empty `spectator_action`. No mining,
no swing, no `punch`.

### 1.8 What the server does (to understand the replies)

- Start: "too far" refusal (block box → eyes distance ≥ block range + 1) **without**
  correction nor explicit refusal ack (the ack goes out anyway); above the world, spawn point
  protection, interaction forbidden, restricted action → `block_update` of the true state.
  Creative → break. Survival: gain ≥ 1 → break; otherwise start of its own countdown (progression =
  gain × (ticks elapsed + 1)).
- Finish: accepted if the server progression ≥ **0.7**; otherwise the server finishes later
  on its own ("deferred break") as soon as it reaches 1, without a new packet from the client. If the server
  break fails: `block_update`.
- The server sends `block_destruction` (id = the miner's entity id) to the **other** players within
  32 blocks (distance² < 1024 between the block and the feet), never to the miner themselves.

---

## 2. Cracks (destruction stages)

### 2.1 Storage

- Table `id → (pos, stage, update tick)`, plus index `pos → sorted set` (sorted by stage
  then id). Local entry: id = the local player's entity id; other players: id received in
  `block_destruction` (the miner's entity id).
- Update `(id, pos, stage)`: if `0 ≤ stage < 10`: remove this id's old entry from the
  index; if it does not exist or the pos differs, create a new one; set the stage and
  the update tick = current game time; reinsert into the index. Otherwise (−1, ≥ 10, or
  255): remove this id's entry.
- Cleanup: at each client world tick where `game_time % 20 == 0`, remove the entries not
  updated for more than **400** ticks (case of a miner who goes out of range without aborting).

### 2.2 Rendering

- Per position, only the **highest** stage of the set is drawn (last sorted element).
- Ignored if the distance² between the block's center and the camera > 1024 (32 blocks).
- Only for states whose render type is "model" (not invisible blocks nor those
  drawn only as a block entity; the latter receive the crack in their own block-entity
  rendering with the same stage, see Uncertain points).
- Geometry: the quads of the **block model** (same parts, same random variant draw:
  block position seed), translated to the block position + the state's random offset
  (tall grass, etc.).
- Texture: `block/destroy_stage_<stage>` (10 textures of 16×16). **Projected** UVs, not those
  of the model: for each vertex, block-local position `p` (after the offset), face = direction
  closest to the normal; `p` rotated 180° around Y, then −90° around X, then by the
  face's own rotation; `uv = (−p.x, −p.y)` (texture repeated). White color, full light,
  no tint.
- "Crumbling" pipeline: multiplicative blend `color = src × dst + dst × src` (source
  factors = destination color, destination = source color; alpha: 1/0); "closer or equal"
  depth test **without writing**; depth bias (slope 1, constant 10) to get in front of the
  face; fragments with alpha < 0.1 discarded; fog applied. Opaque block: with the solid
  pass; translucent block: in the overlay pass after the translucent one.

---

## 3. Placing / using (right click)

### 3.1 Trigger

- Each press consumed → `use()`. In addition, at each tick, if the button is held,
  `right_delay == 0` and the player is not using an item → `use()`.
- `right_delay`: decremented at the **start** of each client tick (before everything else); set to 4
  by `use()`. Repeat when holding: one use every 4 ticks.
- During a prolonged use (§ 5), left, right and middle clicks are **flushed**
  with no effect.
- Screen open: `miss_delay = 10000` at each tick (mining resumes only on release).

`use()`:
1. If `active` (survival mining in progress) → nothing (and `right_delay` is not set).
2. `right_delay = 4`. Hands busy (boat) → nothing.
3. For **main hand then offhand**: let `stack` be that hand's item; if the item is not
   enabled by the world's features → full stop. `anim` = the stack's `interact_animation`
   (default `whack` 6).
   - **Entity** aim: if the entity is outside the world border → full stop. If it is within
     interaction range (distance² box → eyes < entity range²): send `interact` (§ 4.2);
     if the local result is a **success**: local swing if "predicted" success, then full
     stop. Otherwise we continue (no block branch).
   - **Block** aim: `n0` = stack quantity; `r = use_item_on(hand, aim)` (§ 3.2); if
     success: if "predicted", local swing of this hand and, if the stack is not empty and
     its quantity changed (or player with infinite materials), "bounce" of the hand
     (`hand-camera.md`); full stop. If failure → full stop. If "pass" → we continue.
   - If the stack is not empty: `r = use_item(hand)` (§ 3.3); if success: local swing if
     "predicted", "bounce" of the hand in all success cases; full stop.
   - Otherwise next hand.

Interaction results: *success* (with swing source: "predicted" = the client swings
and the server only sends the swing to the others; "server only" = the client does not swing,
the server sends the swing back to everyone including the player; "none" = nobody swings),
*failure*, *pass*, *try with empty hand*.

### 3.2 `use_item_on` (block aim)

1. Optional sending of `set_carried_item`.
2. Targeted block outside the world border → **failure** without a packet.
3. Predicted action (§ 0.1), the packet `use_item_on(hand, aim, sequence)` goes out **after**
   the local execution; the `world_border` field is false for a normal shot, `inside` true if the
   aim started inside the shape (`targeting.md` § 2).
4. Local execution:
   - Spectator → result "success without swing" (the packet goes out; the server opens the menus
     of blocks with a menu).
   - `bypass` = sneaking **and** at least one non-empty hand. If `¬bypass`:
     - block not enabled by the features → **failure**;
     - block's reaction to the item (`block × item`, e.g. book on a chiseled bookshelf,
       item in a pot, bucket on a cauldron…); if it consumes the action → result;
     - if it asks to "try with empty hand" and it is the main hand: block reaction
       without an item (open a door, a chest, a lever, a bed…); if it consumes → result.
   - If the stack is not empty and not on cooldown (`use_cooldown`, cooldown group): use of
     the item on the block. Adventure: if the player cannot build and the stack has no
     `can_place_on` accepting the **clicked** block → "pass". Player with infinite materials: the
     quantity is restored after use. Otherwise, if success with transformed stack → the hand receives
     the new stack. Empty stack or on cooldown → "pass".
5. Main hand / offhand order: the offhand is only tried if the main hand
   gave "pass" (e.g.: torch in offhand, pickaxe in main hand on a block with no reaction).

**Placing a block (block item)**, entirely predicted by the vanilla client:
- Position: if the clicked state is **replaceable** by this placement (tall grass, snow layer, water…)
  → the clicked position itself, otherwise the neighboring one on the face side. If this neighbor is
  not replaceable → failure.
- Placed state = the block's own placement rule (orientation according to the look direction, the face, the point
  `cy` for slabs / stairs, waterlogging…); then survival check (does the block hold
  here) and obstruction check (no blocking entity, the player included, depending on the placement
  context). Failure if either is missing.
- Local placement (remembered by the prediction), application of the stack's `block_state` component,
  "placed" sound played locally (`sounds.md` § 5.3), quantity −1 (except infinite materials) → "predicted"
  success. The stack's block-entity data is only applied server side.
- The server always answers with `block_update` of the clicked position **and** of its neighbor on the
  face side, then the ack: a wrong prediction is corrected at the latest at the ack (§ 0.1).
- Server checks useful to the client: hit point within 1.0000001 of the block's center on
  each axis, block in range (block range + 1), height within the world's limits (otherwise build
  limit message in the action bar).

Other items predicted by the vanilla client: any item whose use on a block is executed
locally — buckets (empty: pick up a source, full: place the fluid, fill /
empty sounds played locally), flint and steel / fire charge, hoe, shovel (path), axe (stripping,
scraping oxidation, scraping wax), bone (bone meal), seeds and plantings, frames and paintings (the entity
is created by the server only), signs (the editing screen is opened by the server), etc.
Their effects on blocks go through the same prediction mechanism.

### 3.3 `use_item` (no block / entity target, or after "pass")

- Spectator → "pass" without a packet.
- Optional sending of `set_carried_item`; predicted action: the packet `use_item(hand, sequence,
  yaw, pitch)` is built first, then: stack on cooldown → "pass"; otherwise local use of
  the item; if success with transformed stack, the hand receives the new stack.
- Main local uses: consumable (§ 5.2), swappable equipment (armor, elytra…
  immediate swap with the equipment slot), `blocks_attacks` (shield), `kinetic_weapon`
  (spear: start + sound), bow, crossbow, trident, spyglass (§ 5), buckets (raycast on
  fluids within block range), thrown items (snowball, egg, pearl…: projectile created by the
  server only; the throw sound is broadcast by the server).
- Instant-use item (use duration ≤ 0) that succeeds: "after use" effects of the stack
  (`use_cooldown` cooldown set locally, `use_remainder` kept).

### 3.4 Swing and "bounce"

- The client swings the hand concerned locally only for a "predicted" success (§ 3.1).
- The server, for any success that swings: swing broadcast to the others, and sent back to the player
  themselves if the source was not "predicted".
- The hand's "bounce" (`hand-camera.md` § 2.1) follows the rules of § 3.1.

### 3.5 Items with their own ray (boats, buckets, bottle, lily pad, eggs)

These items do not rely on the crosshair: on use (`use_item`), the client AND the server cast
a ray again from the player's eyes (position + eye height, not the camera), along the
view (pitch, yaw; sine and cosine from the game's table), over the block interaction range
(attribute, 4.5 by default). For each block crossed we compare the block's outline and the
shape of its fluid (box of height = amount / 9, or 1 if the same fluid is above); the
closest wins, the block on a tie. Fluid modes:

| Item | Fluids hit | Hitting nothing | On a block hit |
|---|---|---|---|
| boat, raft (with or without chest) | all | pass | nobody must enclose the eyes (otherwise pass); 1.375 × 0.5625 box at the hit point, player's yaw, without collision otherwise failure: blocks, vehicles, and any pushable living creature including the local player (except bats, armor stands, mounts already ridden, spectators, player on a ladder); the server creates the entity and consumes the item; the client swings the hand |
| empty bucket | sources only | pass | water or lava source, waterlogged block, bubble column (water bucket, water sound), powder snow picked up (block emptied, fill sound played by the client); otherwise failure |
| water, lava bucket | none | pass | placed in the liquid container hit (water: waterloggable, seagrass, kelp), otherwise in front of the face hit; container already waterlogged or refusing (seagrass): success without effect, the bucket empties, nothing changes; air block, replaceable or non-"solid" (in the historical sense) replaced, second attempt in front of the face if the block does not accept (or player sneaking); water evaporated in the Nether (extinguish sound, smoke) |
| empty bottle | sources only | pass | water → water bottle, sound played by the client |
| lily pad, frogspawn | sources only | placement attempted above the ray's arrival block (fails in practice) | ordinary block-item placement above the block hit (the client predicts block, placement sound, stack decremented, and refuses if the player or an entity blocking construction occupies the cell), without adventure mode check nor "used" statistic; on a block aimed at with the crosshair (`use_item_on`) the item does NOTHING (pass) |
| spawn egg | sources only | pass | client: success as soon as a block is hit; server: only if the block hit is a liquid block, mob placed in this block without vertical offset |

- The "except the author" sounds (filling, emptying) are not sent back to the acting player: their
  client plays them itself, with the predicted blocks (§ 0.1).
- A failure stops the hand's attempt (no swing) but lets the next hand try.
- Quirk: the game caches, per fluid state, the first shape computed; depending on what was
  asked first, a source can therefore stay 1 high even without water above. Not
  reproducible (depends on the process history): we compute the exact shape.

---

## 4. Entities

### 4.1 Attack (left click on an entity)

Cooldown: `attack_delay = 20 / attribute attack_speed` ticks (`hud.md`, `hand-camera.md`);
`attack_counter` +1 per player tick; `strength(pt) = clamp((attack_counter + pt) /
attack_delay, 0, 1)`. Reset: change of **item** in the main hand, attack, click
into the void, mining abort, and on receiving a `punch` on the server side.

Sequence (after the left-click checks, § 1.3):
1. If the item has `attack_range`: the attack is only sent if the aimed point is at an eye
   distance within `[min − margin, max + margin]` (creative min/max in creative; margin =
   `hitbox_margin`). Out of range: no packet (but swing + `punch`).
2. Optional sending of `set_carried_item`, then `attack(entity_id)`.
3. Local simulation of the attack: if the entity is not attackable or ignores the attack, nothing.
   Otherwise strength `s = strength(0.5)`; most entities **do not simulate** damage on the client side.
   Exceptions that "take it" on the client side (if not invulnerable to the source): boats, rafts,
   minecarts, frames (except fixed), paintings, leash knots, End crystals (except dragon
   source), experience orbs, dropped items, shulker bullets, cushions. For these only:
   critical hit (s > 0.9, falling, not on the ground, not on a ladder, not in water, not
   passenger, living target, not sprinting) → local `crit` particles following the entity;
   knockback: if sprinting and s > 0.9, knockback +0.5 → the non-living target receives a push
   `(−sin(yaw)·k, 0.1, cos(yaw)·k)`, then the local player's velocity is multiplied by
   (0.6; 1; 0.6) and the sprint cut.
   Attack sounds (strong, weak, critical, knockback, sweep, no damage): **all broadcast by the
   server** (none played locally). `sweep_attack` sweep particles and damage indicators: sent
   by the server. Critical particles on mobs: `animate` packet from the server
   (`particles.md` § 1.5).
4. Reset of the attack and swap counters; in creative `delay = 5` (§ 1.5).
5. Local swing (attack animation) + `punch` (§ 1.3).

Server checks (for the record): box → eyes distance within the item's attack range
with 3 of tolerance; attacking a dropped item, an orb, oneself or a non-attackable arrow
**disconnects** the client: never send `attack` on these entities (they are not
selectable by the aim, except stuck arrows: already excluded, `targeting.md` § 3).

### 4.2 Interaction (right click on an entity)

- `interact(entity_id, hand, point − entity feet, sneaking)` then, outside spectator, the local
  reaction: entity reaction (mount, swap, milk, shear…); if it does not consume
  the action and the stack is not empty and the target living: item reaction on the entity
  (equip via `equippable.equip_on_interact`, name tag, saddle, bucket on a fish…). With
  infinite materials the quantity is restored. Spectator: "pass" (the server opens the menus
  of entities with a menu: chest minecart…).
- Only one attempt per hand; the offhand is only tried if the main hand did not
  succeed (and the main hand also tries `use_item` before passing to the offhand, § 3.1).

### 4.3 Range and block / entity choice

Ranges: `block_interaction_range` (4.5) and `entity_interaction_range` (3); the choice between targeted
block and entity is that of `targeting.md` § 1. Additional client checks: entity interaction
only if distance² (box → eyes) < entity range²; attack only if
`attack_range` allows it (§ 4.1). The server tolerates +1 (block) and +3 (entity).

### 4.4 Jab (item `piercing_weapon`, spears)

Left click (and no continuous mining): optional sending of `set_carried_item`, `player_action`
**jab** (pos 0, face down, sequence 0); reset of the attack counter; local swing
with the item's attack animation; the component's `sound` played locally. No `punch`.
Aiming and damage: server (`attack_range` range). Spear charge (right click,
`kinetic_weapon`): `hand-camera.md` § 2.8 for the animation.

---

## 5. Prolonged use

### 5.1 Common mechanics

- Start (successful local use of an item with a duration): `used_item = stack`, `remaining` = the stack's
  use duration, local "using" flag true, hand remembered. The server sets on its side
  bits 1 (using) and 2 (offhand) of the living-entity flags byte.
- Entity data received for the local player: if the server says "using" and the client does not
  → local start on the indicated hand; if the server says "not using" and the
  client is using → local stop (this is how a meal ends: the client does **not** end it
  itself).
- Each player tick: if the stack of the hand in use is still the same **item** (components
  ignored), it replaces `used_item` and the use tick is run (§ 5.2), then `remaining −= 1`
  (at 0: only the server finishes); otherwise local stop.
- Release: in key processing, if the player is using an item and the use button is
  **no longer** held: optional sending of `set_carried_item`, `player_action`
  **release** (pos 0, face down, sequence 0), then local release (the item's release effect
  if it is still the same item, then stop).
- Slowdown while using: `use_effects` (`speed_multiplier`, default 0.2; `can_sprint`)
  applied to the movement input (except passenger) — `movement.md`.
- Hand swap (F): the server stops the use.

### 5.2 Eating / drinking (`consumable`)

- Start: refusal if the item is food and the player cannot eat (hunger
  full, except `can_always_eat`, creative or invulnerable) → failure. Duration = `floor(consume_seconds ×
  20)`; 0 → immediate consumption (local effects then stack −1).
- Use tick: if `elapsed = duration − remaining > floor(duration × 0.21875)` **and** `remaining % 4 == 0`
  → 5 item particles (if `has_consume_particles`) + the component's sound.
- Particles (`item` with the stack): velocity `(r−0.5)×0.1, r×0.1+0.1, 0` rotated by −pitch
  around X then −yaw around Y, then +0.05 in Y; position `((r−0.5)×0.3, −r×0.6−0.3, 0.6)`
  rotated the same way, + the player's (x, eyes, z).
- Sound: eating → volume 0.5 or 1 (boolean draw), triangular pitch (center 1, spread 0.2);
  drinking → volume 0.5, uniform pitch [0.9; 1]. Local player: played locally (players
  category); other players: sent by the server (the client emits nothing for them), particles on the
  other hand computed locally for everyone.
- End: server (16 particles + sound, effects, stack); the client receives the new stack and the end
  via the flags.

### 5.3 Bow, crossbow, trident, spyglass, shield

| Item | Local start | During | Release |
|---|---|---|---|
| Bow (duration 72000) | failure without ammunition unless infinite materials | model `pull` (`items.md`) | shot on the server side if power ≥ 0.1; shot sound broadcast by the server |
| Crossbow (72000) | loaded → shot (server; the client does not empty the charge), success without start; otherwise start if ammunition, otherwise failure | charging sounds: server | nothing (charging finished on the server side at charge time `floor(1.25 s modified × 20)`) |
| Trident (72000) | failure if the next damage breaks it, or Riptide without water or rain | — | nothing if held < 10 ticks; throw / riptide on the server side |
| Spyglass (1200) | sound `item.spyglass.use` played locally; immediate start | zoom (`hand-camera.md`) | local sound `item.spyglass.stop_using` |
| Shield (`blocks_attacks`) | start | "blocks" when `elapsed ≥ block_delay_seconds × 20` (5 vanilla ticks) | — |
| Spear (`kinetic_weapon`) | start + the component's sound | damage: server | — |

---

## 6. Other actions

Order of key processing in the tick (no screen nor overlay): view change,
smooth camera, HUD keys, spectator effect, **slots 1–9**, inventory, quick actions,
**swap hands (F)**, **drop (Q)**, then prolonged use / clicks (left, right,
middle), right-click repeat, continuous mining.

- **Slots 1–9**: spectator → spectator menu; creative with the hotbar save or
  load key held → saved hotbars; otherwise the selected slot changes
  locally. No immediate sending: `set_carried_item` goes out at the next tick (`hud.md` § 6.2).
- **Scroll wheel** (no screen): accumulation per axis (reset if the sign changes), multiplied
  by the sensitivity (and reduced to the sign if "discrete scrolling"); integer part consumed;
  `notch = (y ≠ 0) ? y : −x`; spectator → menu or flying speed `clamp(v + y × 0.005, 0, 0.2)`;
  otherwise `slot = (slot − sign(notch)) mod 9` (a single notch per event, whatever the
  amplitude).
- **Drop (Q / Ctrl+Q)** (outside spectator): **predicted** removal of 1 item (or of the whole stack
  with Ctrl) from the selected slot; optional sending of `set_carried_item`; `player_action`
  **drop one item** (5) or **drop the stack** (4); if something was removed, local swing
  of the main hand (default animation). The item entity is created by the server.
  Q in an inventory screen: `inventories.md`.
- **Swap hands (F)** (outside spectator): `player_action` **swap** alone; no
  prediction (the server swaps and sends the slots back; it also stops the use in progress).
- **Pick block (middle click)**: if the aim is not "nothing": `with_data` = Ctrl
  held; block → `pick_item_from_block(pos, with_data)`, entity →
  `pick_item_from_entity(id, with_data)`. No local action. Server: in range (block +1,
  entity +3); if the item already exists in the inventory → selection (hotbar) or swap with the selected
  slot (outside the hotbar); otherwise, in creative only, add then select; reply
  `set_held_slot` + modified slots. Block data included only in creative.
- **Arm swing**: local only (§ 1.3, 3.1, 4, 6) + `punch` for the left click;
  duration and rendering: `entities.md` § 1.10.

---

## 7. Sounds and particles played by the client itself

| Action | Local sound | Local particles |
|---|---|---|
| Start of non-instant mining (survival) | the block's "hit" (`sounds.md` § 5.3) | 1 mining particle on the face (helper F) |
| Mining in progress | — (server: 2020 / 2019) | — (server) |
| Local break (survival, creative) | 2001: "broken" | 2001: block pieces |
| Placing a block | the new state's "placed" | — |
| Bucket filled / emptied | fluid pickup or emptying sound (emptying into forbidden water: extinguish, volume 0.5) | evaporation smoke (Nether) |
| Eating / drinking (local player) | the consumable's sound (§ 5.2) | item particles (all players) |
| Spyglass | start and end of use | — |
| Jab (spear) | `piercing_weapon`'s `sound` | — |
| Attack on a vehicle, frame… (§ 4.1) | — | `crit` if critical |
| Attack on a mob | — (server sounds) | — (server `animate` packet / particles) |

---

## 8. Uncertain points

1. **Predicted placement**: vanilla executes each block's placement rule on the client side. Reproducing
   them all is a big job; a first version may only predict simple full
   blocks (and predict nothing otherwise, the server `block_update` following closely). To be
   decided; the visible difference is a one-round-trip delay on the unpredicted blocks.
2. **"Different" item during mining**: the comparison includes the components; a tool
   whose `damage` component changes during mining (rare case: damage from another source)
   restarts the mining from zero. Reproduced as read.
3. The **packet numbers** of the server-bound packets (`attack`, `punch`, `interact`, `use_item_on`…) are
   not collected here: take them from the protocol 777 registry (`rc-proto-v777`).
4. **Cracks on block entities** (chests, signs, beds…): passed to the block-entity
   rendering with the highest stage; the detail (same projected UVs on the parts of the
   entity model) is not described here.
5. "The player is about to break" effects on the client side other than 2001 (doors / beds that break their
   other half, beehive…): not collected block by block.
6. **Hardness and "requires a tool"** per block: not yet extracted (`rc-data`); needed by
   § 1.2.
7. `use_cooldown` cooldown set locally after an instant use: assumed, call seen but
   client path not followed to the end.
