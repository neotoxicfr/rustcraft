# Sounds and music (26.3, protocol 777)

Source: reading of the decompiled client (kept outside the repository), reworded. Scope: sound packets,
resources (`sounds.json`, `.ogg` files, asset index), engine (categories, volumes,
attenuation, channels), sounds produced by the client itself (steps, blocks, ambience, music,
interface), subtitles.

Notations: tick = 1/20 s; "random" = the linear congruential generator described in § 4.4 unless stated otherwise.

## 1. Packets (play, client-bound)

### 1.1 "Holder" sound event (common encoding)

A sound event is transmitted as a `varint` *k*:

- *k* > 0: reference to the built-in registry `minecraft:sound_event`, numeric id `k − 1`.
  The registry is **not** sent by the server (frozen registry of the game): 1991 entries in 26.3,
  ids in `reports/registries.json` of the data generator (e.g. `ambient.cave` = 7,
  `music.menu` = 1058, `block.stone.step` = 1608, `ui.button.click` = 1672).
- *k* = 0: inline-defined event: identifier (string ≤ 32767, form `namespace:path`),
  then optional fixed range (`bool` present, then `f32`).

The fixed range is only used on the server side (broadcast radius: fixed range if present, otherwise
16 × volume if volume > 1, otherwise 16). The client does not use it: only the identifier counts,
resolved in `sounds.json` (§ 2).

### 1.2 Categories (`varint` enumeration)

| id | name (options, `soundCategory_<name>`) |
|---|---|
| 0 | `master` (general) |
| 1 | `music` |
| 2 | `record` (jukebox, note blocks) |
| 3 | `weather` |
| 4 | `block` |
| 5 | `hostile` |
| 6 | `neutral` |
| 7 | `player` |
| 8 | `ambient` |
| 9 | `voice` |
| 10 | `ui` (interface) |

### 1.3 `sound`

Fields in order:

1. sound event (§ 1.1);
2. category (`varint`, § 1.2);
3. x, y, z: three `i32`, coordinates multiplied by 8 and truncated toward zero by the server;
   the client divides by 8 (precision 1/8 block);
4. volume (`f32`);
5. pitch (`f32`);
6. seed (`i64`).

Handling: one-shot sound, positioned at the received point, linear attenuation, non-relative, non-
looping, without delay (§ 3); the variant draw uses a generator initialized with the
seed (§ 4.4).

### 1.4 `sound_entity`

1. sound event (§ 1.1);
2. category (`varint`);
3. entity id (`varint`);
4. volume (`f32`); 5. pitch (`f32`); 6. seed (`i64`).

Handling: ignored if the entity is unknown. Otherwise its "attached to the entity" sound: initial
position = the entity's position, then copied at each tick (values converted to `f32`);
stopped as soon as the entity is removed from the world; not played if the entity is silent ("silent"
flag of the metadata) at start or during playback (it is then cut).
Variant drawn with the seed.

### 1.5 `stop_sound`

1. `u8` flags: bit 0 (value 1) = category present, bit 1 (value 2) = sound present;
2. if bit 0: category (`varint`);
3. if bit 1: event identifier (string ≤ 32767).

Handling (the comparison is on the **event** identifier, not the file's):

- category given: cuts the sounds of this category (all of them, or only those of the event
  if the sound is given);
- neither: **total stop** of the engine (all channels, queue, periodic
  sounds; the internal category gains go back to 1, which also cuts the music);
- sound alone: cuts this sound in all categories.

### 1.6 `level_event`

1. type (`i32`); 2. block position (packed `i64`); 3. data (`i32`); 4. global (`bool`).

"Block position" below = block center (+0.5 on each axis). "r" = client world
random, `f` = new float [0, 1). Common formulas:
`H1 = 1 + (f − f)·0.2` (two successive draws), `H2 = 0.9 + f·0.1`, `H3 = 2.6 + (f − f)·0.8`.

**Global events** (global bool = true; only 1023, 1028, 1038 do anything): the sound
is placed 2 blocks from the camera in the direction of the received block's center (normalized), hence
always audible:

| type | sound | category | volume | pitch |
|---|---|---|---|---|
| 1023 | `entity.wither.spawn` | hostile | 1 | 1 |
| 1028 | `entity.ender_dragon.death` | hostile | 5 | 1 |
| 1038 | `block.end_portal.spawn` | hostile | 1 | 1 |

**Local events** that play a sound (the others only do particles):

| type | sound | cat. | volume | pitch |
|---|---|---|---|---|
| 1000 | `block.dispenser.dispense` | block | 1 | 1 |
| 1001 | `block.dispenser.fail` | block | 1 | 1.2 |
| 1002 | `block.dispenser.launch` | block | 1 | 1.2 |
| 1004 | `entity.firework_rocket.shoot` | neutral | 1 | 1.2 |
| 1009 | data 0: `block.fire.extinguish`; data 1: `entity.generic.extinguish_fire` | block | 0.5 / 0.7 | H3 / `1.6 + (f−f)·0.4` |
| 1010 | jukebox disc: data = id in the synchronized registry `jukebox_song`; sound of that track, record category, volume 4, pitch 1, linear, at the block's center; remembered by position | record | 4 | 1 |
| 1011 | stops the track remembered at this position | | | |
| 1015 | `entity.ghast.warn` | hostile | 10 | H1 |
| 1016 | `entity.ghast.shoot` | hostile | 10 | H1 |
| 1017 | `entity.ender_dragon.shoot` | hostile | 10 | H1 |
| 1018 | `entity.blaze.shoot` | hostile | 2 | H1 |
| 1019 | `entity.zombie.attack_wooden_door` | hostile | 2 | H1 |
| 1020 | `entity.zombie.attack_iron_door` | hostile | 2 | H1 |
| 1021 | `entity.zombie.break_wooden_door` | hostile | 2 | H1 |
| 1022 | `entity.wither.break_block` | hostile | 2 | H1 |
| 1024 | `entity.wither.shoot` | hostile | 2 | H1 |
| 1025 | `entity.bat.takeoff` | neutral | 0.05 | H1 |
| 1026 | `entity.zombie.infect` | hostile | 2 | H1 |
| 1027 | `entity.zombie_villager.converted` | hostile | 2 | H1 |
| 1029 | `block.anvil.destroy` | block | 1 | H2 |
| 1030 | `block.anvil.use` | block | 1 | H2 |
| 1031 | `block.anvil.land` | block | 0.3 | H2 |
| 1032 | `block.portal.travel`: local ambient sound (relative, no attenuation, ambient category), volume 0.25, pitch `0.8 + f·0.4` | ambient | 0.25 | |
| 1033 | `block.chorus_flower.grow` | block | 1 | 1 |
| 1034 | `block.chorus_flower.death` | block | 1 | 1 |
| 1035 | `block.brewing_stand.brew` (bottle / brewing stand) | block | 1 | 1 |
| 1039 | `entity.phantom.bite` | hostile | 0.3 | H2 |
| 1040 | `entity.zombie.converted_to_drowned` | hostile | 2 | H1 |
| 1041 | `entity.husk.converted_to_zombie` | hostile | 2 | H1 |
| 1042 | `block.grindstone.use` | block | 1 | H2 |
| 1043 | `item.book.page_turn` | block | 1 | H2 |
| 1044 | `block.smithing_table.use` | block | 1 | H2 |
| 1045 | `block.pointed_dripstone.land` | block | 2 | H2 |
| 1046 | `block.pointed_dripstone.drip_lava_into_cauldron` | block | 2 | H2 |
| 1047 | `block.pointed_dripstone.drip_water_into_cauldron` | block | 2 | H2 |
| 1048 | `entity.skeleton.converted_to_stray` | hostile | 2 | H1 |
| 1049 | `block.crafter.craft` | block | 1 | 1 |
| 1050 | `block.crafter.fail` | block | 1 | 1 |
| 1051 | `entity.wind_charge.throw` | block | 0.5 | `0.4 / (0.8 + f·0.4)` |
| 1052 | `block.sulfur_spike.land` | block | 2 | H2 |
| 1053, 1054 | `entity.splash_potion.break` (thrown bottle) | neutral | 1 | H2 |
| 1500 | data > 0: `block.composter.fill_success`, otherwise `block.composter.fill` | block | 1 | 1 |
| 1501 | `block.lava.extinguish` | block | 0.5 | H3 |
| 1502 | `block.redstone_torch.burnout` | block | 0.5 | H3 |
| 1503 | `block.end_portal_frame.fill` | block | 1 | 1 |
| 1505 | `item.bone_meal.use` | block | 1 | 1 |
| 2001 | block broken: data = block state id; if non-air, the "broken" sound of the block's sound set (§ 5.2) | block | `(v + 1)/2` | `h · 0.8` |
| 2006 | `entity.dragon_fireball.explode` | hostile | 1 | H2 |
| 2020 | mining hit: "hit" sound of the sound set of the block present (§ 5.2); 2019 = same particles without sound | block | `(v + 1)/8` | `h · 0.5` |
| 3000 | `block.end_gateway.spawn` | block | 10 | `(1 + (f−f)·0.2)·0.7` |
| 3001 | `entity.ender_dragon.growl` | hostile | 64 | `0.8 + f·0.3` |
| 3006 | `block.sculk.charge` (see note) | block | | |
| 3007 | `block.sculk_shrieker.shriek` at (x+0.5, y + height of the top of the shrieker, z+0.5), unless the block is submerged | block | 2 | `0.6 + f·0.4` |
| 3008 | "brushing completed" sound of the brushable block whose state is in the data | player | 1 | 1 |
| 3012 | `block.trial_spawner.spawn_mob` (distance delay) | block | 1 | H1 |
| 3013, 3019 | `block.trial_spawner.detect_player` (delay) | block | 1 | H1 |
| 3014 | `block.trial_spawner.eject_item` (delay) | block | 1 | H1 |
| 3015 | `block.vault.activate` (delay) | block | 1 | H1 |
| 3016 | `block.vault.deactivate` (delay) | block | 1 | H1 |
| 3018 | `block.cobweb.place` (delay) | block | 1 | H1 |
| 3020 | `block.trial_spawner.ominous_activate` (delay) | block | data 0: 0.3, otherwise 1 | H1 |
| 3021 | `block.trial_spawner.spawn_item` (delay) | block | 1 | H1 |

`v`, `h`: volume and pitch of the block's sound set (§ 5.2).

Note 3006: data = number of charges `n`; if `n` > 0, with probability `0.3 + 0.1·n`, a sound
of volume `0.15 + 0.02·n²·f` and pitch `0.4 + 0.3·n·f`; if `n` = 0, a sound of volume 1,
pitch 1 (to check in detail if sculk is implemented).

"Distance delay": if the camera is more than 10 blocks away (distance² > 100), the sound is queued
with a delay of `⌊(distance / 40) · 20⌋` ticks (sound at 40 blocks/s); otherwise immediate.
All the other local sounds are without delay.

Doors, trapdoors, fence gates, chests, etc. do **not** go through `level_event` in 26.3:
the server sends an ordinary `sound` packet, except to the player who acted (§ 5.1).

## 2. Resources

### 2.1 Where the files are in the official installation

The sounds are not in the client jar. They are in the launcher's asset store:

- `<.minecraft>/versions/26.3/26.3.json` gives the asset index: field `assets` = `"34"`.
- `<.minecraft>/assets/indexes/34.json`: object `objects` where each key is a path
  "namespace/path" (e.g. `minecraft/sounds.json`, `minecraft/sounds/random/click.ogg`,
  `minecraft/lang/fr_fr.json`) and the value `{ "hash": <sha1 hex>, "size": <bytes> }`.
- Actual file: `<.minecraft>/assets/objects/<first 2 characters of the hash>/<hash>`.
  (index 34: 5147 objects, of which 4961 `.ogg`.)

The official client builds, at startup, a virtual file system from the index:
each key becomes a path, which points to the object file. This system serves as the "assets"
root of the base resource pack (on a par with the jar's `assets/`). Resolution
of a sound name: name `namespace:path` (default namespace `minecraft`) → resource
`<namespace>/sounds/<path>.ogg` → index key → `objects/xx/<hash>`. User
resource packs, higher priority, may provide the same paths under `assets/`.

### 2.2 `sounds.json`

One file per namespace (`<namespace>/sounds.json`), read in **each** active resource pack,
from lowest to highest priority. Root: object *event* → *entry*. The event key
is a path, the namespace being that of the file (`block.stone.step` in
`minecraft/sounds.json` → `minecraft:block.stone.step`). 1991 events in 26.3.

Entry:

| field | type | default | role |
|---|---|---|---|
| `replace` | boolean | false | true: replaces the list already accumulated for this event; false: appends |
| `subtitle` | string | absent | translation key of the subtitle (the last entry that recreates the event sets it) |
| `sounds` | array | empty | variants |

A variant is either a **string** (file name; all default values), or an
**object**:

| field | type | default | constraint |
|---|---|---|---|
| `name` | string | required | file name `namespace:path` (without `sounds/` nor `.ogg`), or event name if `type` = `event` |
| `type` | `file` or `event` | `file` | other value → invalid entry |
| `volume` | float | 1 | > 0 |
| `pitch` | float | 1 | > 0 |
| `weight` | integer | 1 | > 0 |
| `stream` | boolean | false | streamed playback (music, long files) |
| `preload` | boolean | false | decoded when the engine loads |
| `attenuation_distance` | integer | 16 | base attenuation distance |

No vanilla variant uses `preload` nor `replace`. Vanilla 26.3: 219 variants with
`attenuation_distance` (e.g. amethyst resonance 48), 61 of type `event`, 344 in `stream`.

A `file` variant whose `.ogg` file exists in no pack is **discarded** at
loading (warning). An invalid `sounds.json` file makes that whole file be ignored for
that pack.

`type: event` variant: redirect to another event, resolved at draw time. Its weight
in the draw = **sum of the weights** of the targeted event (not its own `weight`); 0 if
the targeted event does not exist. When it is drawn: a variant is drawn from the targeted event
(same generator), then composed: volume = targeted volume × redirect's volume, pitch = targeted
pitch × redirect's pitch, stream = targeted stream OR redirect's stream, preload and attenuation
distance = those of the targeted variant.

### 2.3 Subtitles

Key `subtitle` → translatable component (language files: `en_us` in the jar, the others in
the index, e.g. `minecraft/lang/fr_fr.json`). 1889 vanilla events have one. Display: § 7.

### 2.4 Decoding

Ogg Vorbis files, decoded to signed 16-bit little-endian PCM, with the file's sample rate and number of channels.
Non-stream sounds: decoded entirely once, then cached by path (cache cleared
on resource reload). Stream sounds: progressive decoding, buffers of about 1 s of
PCM, 4 buffers in the queue; a "looping" stream reopens the file at the end.

## 3. Engine

### 3.1 Final volume of a sound

```
sound_volume = requested_volume × variant_volume          (variant_volume: sounds.json)
gain         = clamp(sound_volume, 0, 1)
             × clamp(option(category) × option(master), 0, 1)   (master alone if category = master)
             × internal_gain(category)                          (default 1, clamped to [0, 1])
```

Options: all categories are **1.0** by default (sliders 0 – 100%). The internal gain
is only used for the music fade (§ 6.3). Moving a slider reapplies the gain to all the
sounds of that category in progress (or of all if `master`).

If `gain` = 0 at start, the sound is **not** launched, except category `music` or sounds allowed
to start muted (minecart, § 5.6), launched silent.

### 3.2 Pitch

`pitch = clamp(requested_pitch × variant_pitch, 0.5, 2.0)` (playback speed factor).

### 3.3 Attenuation and distance

Three properties per sound: attenuation (`linear` or `none`), relative (yes/no), position.

- Effective attenuation distance: `D = max(sound_volume, 1) × attenuation_distance`
  (`sound_volume` unclamped, before clamp: a volume 4 quadruples the range).
- Linear: "clamped linear" model per source, reference distance 0, factor 1, max
  distance `D` → gain factor `clamp(1 − d / D, 0, 1)`, `d` = source–listener distance.
- None: no attenuation (factor 1).
- Relative: position expressed in the listener's frame (`(0, 0, 0)` = on the listener).
  Interface, music and local ambience sounds are relative, without attenuation, at `(0,0,0)`.
- Stereo spatialization: by the audio API (OpenAL), according to the source's direction in the
  listener's frame. Option "directional audio" (default: no) = enables HRTF if the
  device offers it; the output limiter is always requested.

### 3.4 Listener

Updated **at each frame** (before rendering) from the camera: position = camera
position (interpolated, including in 3rd person), orientation = camera forward vector + camera
up vector. Listener gain unchanged (1). Reset to the origin (forward = −Z, up = +Y,
position (0, 0, 0)) at engine initialization.

### 3.5 Simultaneous channels

Two source pools:

- total `N` = number of mono sources declared by the device; failing that, 30;
- streams: `clamp(⌊√N⌋, 2, 8)`; normal: `clamp(N − streams, 8, 255)`.
  (OpenAL Soft default: N = 255 → 8 streams, 247 normal.)

Pool full → the new sound is simply **dropped** (no eviction). A finished sound
keeps its entry at least 20 ticks after its start before being cleaned up.

### 3.6 Variant choice

At playback, the event is resolved in the table coming from `sounds.json`:

- unknown event → nothing (warning only once);
- `minecraft:intentionally_empty` → nothing, without warning;
- event without a valid variant → nothing.

Weighted draw: `P` = sum of the weights (§ 2.2); `i = bounded_int(P)` (§ 4.4); walk the
variants in order of appearance (pack stacking order then array order) subtracting
each weight; the first one that makes `i < 0` is chosen.

Generator: for `sound` and `sound_entity`, initialized with the packet's seed; for local
sounds (§ 5), seed drawn from the client world's random, or uninitialized generator (random
seed) for music, interface, ambience. The same generator is then used to sample
the variant's volume and pitch, which are constant in vanilla (no draw consumed).

### 3.7 Looping, periodic, deferred sounds

- "Looping" sound without delay: native loop (full buffer) or stream reopened in a loop.
- Looping sound with delay > 0: at the end, put back in the queue for `delay` ticks then relaunched.
- "Periodic" sound (updated at each tick): before the update, if it can no longer play
  (silent entity…) it is cut; otherwise its own update runs, then volume,
  pitch and position are reapplied to the channel (formulas § 3.1–3.2). A sound marked "stopped"
  is cut. For an audible looping sound, the subtitle is refreshed every 10 ticks.
- Deferred queue: sounds launched when the engine's tick counter reaches the deadline.
- Pause (integrated singleplayer only, so never for our multiplayer client): everything is paused
  except `music` and `ui`.

### 3.8 Order in the client tick

Music manager (§ 6) then sound engine: audio device checked (change or
loss → full reload), queued sounds, periodic sounds, cleanup, deferred queue.

## 4. Coding details

### 4.1 Position of a block sound

Block center: `(x + 0.5, y + 0.5, z + 0.5)`.

### 4.2 Sound "attached to the entity"

See § 1.4. Also used for local sounds attached to an entity.

### 4.3 Sound "of the local player" (prediction)

A sound that the local player triggers through a predicted action (step, block placement, etc.) is played
immediately by the client; the server sends it to all the other players **except** them. The
client therefore never plays a sound for another entity without a packet.

### 4.4 Pseudo-random generator (identical to the classic Java generator)

- initialization: `s = (seed XOR 0x5DEECE66D) & (2⁴⁸ − 1)`;
- `next(b)`: `s = (s × 0x5DEECE66D + 0xB) & (2⁴⁸ − 1)`, returns `s >> (48 − b)` as a signed
  32-bit integer;
- `bounded_int(n)`: if `n` is a power of 2, `(n × next(31)) >> 31`; otherwise repeat
  `x = next(31)`, `m = x mod n` while `x − m + (n − 1)` overflows (negative in signed 32 bits);
  return `m`;
- float: `next(24) / 2²⁴`.

## 5. Sounds played by the client without a packet

### 5.1 Local player's steps

Only the local player produces their own steps (§ 4.3). At each movement applied:

1. No emission if the player is flying ("flying" ability) or is sneaking on the ground, nor if they
   are a passenger of a vehicle.
2. Support block `A` = block under the feet (main supporting block), effect block `E` = block
   "walked on" with offset 0.2 (see `movement.md`, list of offsets).
3. `walkDist += 0.6 × (movement length if A is climbable, otherwise its horizontal
   length)`.
4. If `walkDist > nextStep` and `A` is not air:
   - if the player is on the ground, or on a climbable block, or sneaking with no vertical movement, and
     not swimming: **step sound** on `E` (below), then `nextStep = ⌊walkDist⌋ + 1`;
   - otherwise, if in water: `nextStep = ⌊walkDist⌋ + 1` and swim sound.
   Initial value: `nextStep = 1`. So one step per block walked (0.6 × distance).
5. Step sound on a state `S`:
   - in water: swim sound + muffled step of `S` (volume `v·0.05`, pitch `h·0.8`);
   - otherwise, if the block above `S` carries the tag `inside_step_sound_blocks` (snow
     layer, vegetation…) or `combination_step_sound_blocks`: this block above is used; if
     it is a "combination" block (carpet, snow…), its step is played (volume `v·0.15`, pitch
     `h`) **and** the muffled step of the block below; otherwise its step alone;
   - otherwise step of `S`: volume `v·0.15`, pitch `h`.
   - `crystal_sound_blocks` blocks (amethyst): in addition, at most once per 20 ticks,
     `block.amethyst_block.chime`: intensity `I = min(1, I·0.997^(ticks elapsed) + 0.07)`,
     pitch `0.5 + I·f·1.2`, volume `0.1 + I·1.2`.
6. Swim sound: `entity.player.swim`; speed `= min(1, √(vx²·0.2 + vy² + vz²·0.2) × 0.35)`
   (0.4 for a driven vehicle, its speed) = volume; pitch `1 + (f − f)·0.4`.

Category `player`, position = the player's position (feet), linear, without delay.

### 5.2 Block sound sets

Each block (sometimes each state) has a sound set: volume `v`, pitch `h`, and five
events: broken, step, placed, hit, fall. 131 sets in 26.3; almost all `v = h = 1`;
exceptions: metal (`h` = 1.5), anvil (`v` = 0.3), twisting vines (`h` = 0.5). Example stone:
`block.stone.break/step/place/hit/fall`. The block → set table is not in the data
generator's reports: to extract (see Uncertain points).

### 5.3 Blocks broken and placed by the local player

- Start of mining (non-instant): "hit" sound of the block, category block, volume `(v+1)/8`,
  pitch `h·0.5`, at the block's center, random generator.
- During mining: the following hits come from the server (`level_event` 2020 every
  4 ticks, sent to everyone including the miner).
- Block broken (mining finished, or instant, or creative): the client locally executes
  event 2001 ("broken" sound, volume `(v+1)/2`, pitch `h·0.8` + particles); the server
  sends it to the others.
- Block placed: "placed" sound of the new state, category block, volume `(v+1)/2`, pitch `h·0.8`,
  at the block's center.

### 5.4 Biome ambience

Source: environment attribute `minecraft:audio/ambient_sounds` (discrete, stacking
dimension then biome as in `environment.md` § 1.5), evaluated **at the player's position**
each tick. JSON/NBT format:

```
{ "loop": <sound>, "mood": { "sound": <sound>, "tick_delay": int, "block_search_extent": int,
  "offset": double }, "additions": <setting or list> }
addition setting = { "sound": <sound>, "tick_chance": double }
<sound> = event identifier, or object { "sound_id": id, "range": optional float }
```

Vanilla values: dimension types overworld, overworld_caves, the_end → mood
`ambient.cave` (6000, 8, 2.0); Nether: nothing at dimension level; five Nether biomes
(basalt_deltas, crimson_forest, nether_wastes, soul_sand_valley, warped_forest): loop
`ambient.<biome>.loop`, additions `ambient.<biome>.additions` with `tick_chance` 0.0111, mood
`ambient.<biome>.mood` (6000, 8, 2.0).

Each tick (local player loaded):

1. **Loop**: if the requested loop changes (including to "none"), all the loops
   in progress start fading out; the new one (created and launched if absent) fades in.
   Loop instance: category ambient, looping, relative, no attenuation;
   `fade` ∈ [−1, 40], `volume = clamp(fade / 40, 0, 1)`; fading in: `fade = max(0, fade)`,
   +1 per tick; fading out: `fade = min(fade, 40)`, −1 per tick; stop when `fade < 0`.
2. **Additions**: for each setting, if `world_random.double() < tick_chance`, plays the sound
   (ambient, relative, no attenuation, volume 1, pitch 1).
3. **Mood**:
   - `e = block_search_extent`, `n = 2e + 1`; drawn point
     `P = block(x + bounded_int(n) − e, eyes + bounded_int(n) − e, z + bounded_int(n) − e)`;
   - if sky light at `P` = `c` > 0: `mood −= c / 15 × 0.001`;
     otherwise `mood −= (block_light(P) − 1) / tick_delay` (in total darkness: +1/6000 per tick);
   - if `mood ≥ 1`: direction `u` = (center of `P`) − (x, eyes, z), distance `d = |u|`;
     sound at `(x, eyes, z) + u / d × (d + offset)`, category ambient, linear, volume 1,
     pitch 1, variant drawn with the world's random; then `mood = 0`;
   - otherwise `mood = max(mood, 0)`.
   (The mood displayed in the debug screen is this value.)

### 5.5 Underwater and bubble columns

- Entering underwater (eyes in water, not in spectator): `ambient.underwater.enter`
  (ambient, at the player's position) + loop `ambient.underwater.loop` (relative, looping,
  `fade` +1 per tick underwater, −2 otherwise, capped at 40, volume `fade/40`, stop when
  `fade < 0`). Exit: `ambient.underwater.exit`.
- Each tick underwater, draw `f`: < 0.0001 → `ambient.underwater.loop.additions.ultra_rare`,
  < 0.001 → `….additions.rare`, < 0.01 → `….additions` (relative, cut on leaving the water).
- Bubble column: on entry (player's box reduced by 0.4 in height), except on the very
  first tick and in spectator: `block.bubble_column.whirlpool_inside` (downward column)
  or `block.bubble_column.upwards_inside`, category player, volume 1, pitch 1.

### 5.6 Periodic entity sounds

When an entity appears on the client side:

- minecart (any type): `entity.minecart.riding`, category neutral, looping, linear,
  may start muted; each tick: position = minecart; horizontal speed `s`; if
  `s ≥ 0.01`, world not frozen and (old behavior or on rails): volume
  `0.7 × clamp(s, 0, 0.5)`; otherwise volume 0; playback pitch 1 (see Uncertain points);
- bee: `entity.bee.loop` (or `entity.bee.loop_aggressive` if already angry), launched on the
  next engine tick, neutral, looping, linear, may start muted; each tick: if the
  anger changes (synchronized end of anger > world time), the other loop is queued
  and this one stops; otherwise horizontal speed `s` (components < 0.003 set to 0): if
  `s ≥ 0.01`, volume `1.2 × clamp(s, 0, 0.5)` and pitch `a + clamp(s, a, b) × (b − a)`
  (adult a = 0.7, b = 1.1; baby 1.1 and 1.5), otherwise volume and pitch 0.
These sounds are only created at appearance: an entity that is silent at that moment, or that
becomes so, has no sound anymore.
Other periodic sounds exist (minecart passenger, elytra, guardian): out of scope.

### 5.7 Rain

At each tick, if the rain level > 0: among the columns drawn around the camera for
the drops (weather radius of the options), the last valid rain position is kept
(ground between camera −10 and +10, "rain" precipitation); if it exists and
`bounded_int(3) < rainCounter++` (then `rainCounter = 0`): if this ground is more than one block
above the camera and the camera is under a roof → `weather.rain.above` (volume 0.1,
pitch 0.5), otherwise `weather.rain` (volume 0.2, pitch 1); category weather, block center.
Tick-specific generator: seed `gameTime × 312987231`.

### 5.8 Interface

- Button click (buttons, tabs, checkboxes, world lists, option toggles; not sliders
  nor text fields): `ui.button.click`, category ui, volume **0.25**,
  pitch 1, relative, no attenuation.
- Notifications (toasts): their sound, volume 1, pitch 1, category ui.
- Volume slider moved outside a game: preview of the category (record: note block
  guitar, weather: thunder, block: grass placed, hostile: zombie, neutral: cow,
  player: eating, ambient: `ambient.cave`, ui: click), category ui, volume = the slider's
  value; no new preview as long as the previous one's channel is not released (at least
  20 ticks after its launch); changing slider cuts the preview in progress; general, music
  and voice: no sound.
- Various screens: book page, loom pattern selection, stonecutter recipe
  (same form: category ui, volume 0.25).

## 6. Background music

### 6.1 Tracks and delays

A "track" = sound event + min delay + max delay (ticks) + "replaces the music in
progress". JSON/NBT format: `{ "sound": <sound>, "min_delay": int ≥ 0, "max_delay": int ≥ 0,
"replace_current_music": bool (default false) }`.

| situation | event | min | max | replaces |
|---|---|---|---|---|
| menu (no player) | `music.menu` | 20 | 600 | yes |
| creative | `music.creative` | 12000 | 24000 | no |
| game (default) | `music.game` | 12000 | 24000 | no |
| underwater | `music.under_water` | 12000 | 24000 | no |
| End | `music.end` | 6000 | 24000 | yes |
| End fight | `music.dragon` | 0 | 0 | yes |
| credits | `music.credits` | 0 | 0 | yes |
| biomes | `music.overworld.<…>`, `music.nether.<…>` | 12000 | 24000 | no |

### 6.2 Music choice ("situation music")

1. Open screen that imposes a music (only the end credits) → that one.
2. No player → menu.
3. End dimension and boss bar requesting the music (boss bar flag) →
   End fight.
4. Otherwise attribute `minecraft:audio/background_music` read by the camera probe:
   `{ "default": track?, "creative": track?, "underwater": track? }`; if the player has
   eyes underwater and `underwater` exists → it; otherwise if creative (instant build and
   right to fly) and `creative` exists → it; otherwise `default` (may be absent → no
   music).

Vanilla data: overworld and overworld_caves → default `music.game`, creative
`music.creative`; the_end → default `music.end` (6000–24000, replaces); Nether: nothing.
Biomes: oceans, rivers and frozen river → default `music.game`, creative `music.creative`,
underwater `music.under_water`; most surface biomes have their own
`music.overworld.<name>` (forests → `forest`, badlands ×3 → `badlands`, swamps ×2 → `swamp`,
old growth taigas → `old_growth_taiga`, etc.); the five Nether biomes have
`music.nether.<biome>`; `pale_garden` defines an empty object (no music) and
`minecraft:audio/music_volume` = 0. Biomes without the attribute inherit from the dimension.

### 6.3 Manager (each client tick)

Option "music frequency": default 20 min (24000 ticks), frequent 10 min, constant.
Initial `nextDelay` = 100.

1. Target volume `V` = 1 if a screen imposes its music, otherwise attribute
   `minecraft:audio/music_volume` (camera probe, default 1). If a track plays and the current
   gain `g ≠ V`: rise `g += clamp(g, 0.0005, 0.005)` (without exceeding `V`); fall
   `g = 0.03·V + 0.97·g` (`g = V` if the gap < 0.0001 or if we go below); `g` clamped to
   [0, 1]; if `g ≤ 0.0001`: track stop (stop rule below) and end of tick; otherwise
   `g` becomes the internal gain of the music category (§ 3.1).
2. Situation music `M` (§ 6.2). If none: `nextDelay = max(nextDelay, 100)`.
3. Otherwise:
   - if a track plays, `M` replaces and its event differs from the current one: it is
     cut and `nextDelay = random(0, min(M) / 2)`;
   - if the current track no longer plays: it is forgotten and
     `nextDelay = min(nextDelay, nextDelayFor(M))`;
   - `nextDelay = min(nextDelay, max(M))`;
   - if no track, no world loading screen, and `--nextDelay ≤ 0`: launch `M`
     (music sound: category music, relative, no attenuation, volume 1, pitch 1; the
     variant is drawn in `sounds.json`), show the "now playing" toast if it
     started audible, then `nextDelay = ∞`.

`nextDelayFor(M)`: constant → 100; otherwise `random(min(min(M), F), min(max(M), F))` where `F` =
frequency in ticks; without music → `F`. `random(a, b)` = inclusive uniform integer (a if a ≥ b).
Track stop: cuts, hides the toast, `nextDelay = nextDelayFor(situation) + 100`.

Details: when a track is launched the current gain `g` is kept (after a fade-out
it is 0 and the next track rises by +0.0005, +0.001, +0.002… per tick; the category's
internal gain stays at its last value until then). "No longer plays" = channel released by the
engine, which only happens at the engine tick following the stop and never less than 20 ticks after
launch: a replaced track therefore stays "in progress" for a tick or more, and the
replacement delay is redrawn at each tick until then. Delay generator: random seed.

## 7. Subtitles (option, default: no)

Subscription to the engine: each sound launched (and each audible looping sound, every 10 ticks)
transmits its subtitle, position and range (`D` of § 3.3, or infinite if relative or without
attenuation). A subtitle already present (same text) is refreshed: the instance
at the same position is replaced and (position, now) is added.

Display (in interface units), last element of the HUD, on a separate layer (and after
the open screen if there is no screen or if it is an "in game" screen):

- instances older than `3000 ms × notification display duration` (option, default 1)
  removed; a subtitle is visible if it has an instance left and the camera is within
  range of the best instance (infinite range: always);
- best instance: the only one if there is only one, otherwise the most recent (then the farthest)
  among those in range;
- common width `W` = max of the text widths + widths of `<`, ` `, `>`, ` `;
  `w = W / 2` (integer division), height 9, `h = 4`;
- line *i* (0 = the oldest, at the bottom): origin `(guiWidth − w − 2, guiHeight − 35 − i·10)`;
  background `[−w − 1, −h − 1] → [w + 1, h + 1]` black with alpha = text background opacity (option),
  or **0.8** (0xCC000000) if the background is reserved for chat (default);
- brightness `L = ⌊clamped_lerp(t, 255, 75)⌋` with `t = max(0, now − instant − p) /
  ((3000 − p) × duration)`, `p` = 500 ms for a looping sound, 0 otherwise; color `ARGB(255, L, L, L)`,
  text without shadow;
- direction: `u` = unit vector camera → instance; if `forward·u ≤ 0.5` (outside the front
  field): `>` at `x = w − width(">")` if `right·u > 0`, `<` at `x = −w` if `right·u < 0`;
  text centered at `x = −width/2`, `y = −h`.

## 8. Uncertain points

- Block → sound set table (131 sets, 5 events each): to extract from the game's
  block definition (no data generator report gives it); same for the tags
  `inside_step_sound_blocks`, `combination_step_sound_blocks`, `crystal_sound_blocks`
  (received by the tags packet, hence available on the client side).
- Stereo files: with OpenAL, a stereo source is neither spatialized nor attenuated (music,
  some ambiences). To reproduce with our audio library: stereo = played as is.
- `N` (mono sources of the device) depends on the OpenAL implementation; take 255 → 8 streams,
  247 normal, or 30 by default if it cannot be queried.
- Minecart: the pitch incremented per tick (+0.0025, clamped to 1) is stored in a field
  of its own that is not the one read by the engine; the effective pitch therefore stays 1 (reading of the
  decompiled code, to confirm by ear).
- Random block animation sounds (fire, lava, torches, embers, portals, drips) played
  by the client in the tick that animates nearby blocks: not covered here, to handle with
  the particles.
- Event 3006 (sculk charge) and the "minecart passenger", elytra, guardian, thunder sounds
  on the client side: not detailed.
- Exact sequence of the world random draws (local sounds): irrelevant for the network, but
  the mood and additions random shares the client world's generator.
