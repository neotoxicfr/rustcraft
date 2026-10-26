# Entities (26.3)

Written in a clean room from the decompiled client, in three parts: network and
client-side state (1), entity models and rendering (2), per-entity renders and animations (3).
Network decoder: `crates/rc-proto-v777/src/entity.rs`.

## Entities — network and client-side state (26.3, protocol 777)

Mission A. Written in a clean room from a reading of the decompiled 26.3 client; no line of
code copied. Packet ids taken from `reports/packets.json` (play phase, client-bound).

Reminders of primitives specific to this document:
- `vec3d` = three `f64` (x, y, z).
- `vec3lp` = "low precision" vector (velocities), described in §1.0.
- `angle` = `i8` read as a signed byte; degrees = value × 360 / 256 (−128..127 → −180..178.6).
  Server-side encoding: `floor(degrees × 256 / 360)` truncated to 8 bits.

---

#### 1. Client-bound packets concerning entities

#### 1.0 Auxiliary encodings

#### 1.0.1 Low-precision vector (`vec3lp`)
Used for all velocities (`add_entity`, `set_entity_motion`). Unit: blocks per tick.
- Read `o0 : u8`. If `o0 == 0` → zero vector, end (1 byte in total).
- Otherwise read `o1 : u8`, then `o2 : u32` (big-endian, unsigned).
  Assemble `B = (o2 << 16) | (o1 << 8) | o0` (48-bit integer).
- `scale = o0 & 3`. If bit 2 of `o0` (value 4) is set, additionally read a `varint` `v`
  (interpreted as unsigned 32 bits) and do `scale |= v << 2`.
- Each component takes 15 bits: x at bits 3..17, y at bits 18..32, z at bits 33..47.
  For a raw component `q = (B >> shift) & 0x7FFF`:
  `value = (min(q, 32766) × 2 / 32766 − 1) × scale`.
- Encoding (for information): NaN components → 0, clamped to ±1.7179869183e10; if max(|x|,|y|,|z|) <
  3.051944088384301e-5 → a single byte 0. Otherwise `scale = ceil(max)`, each component divided by
  the scale and quantized `round((c × 0.5 + 0.5) × 32766)`; if the scale fits in 2 bits, it goes
  into `o0 & 3` without bit 4, otherwise `o0` carries `(scale & 3) | 4` and the rest `scale >> 2` follows as a
  varint.

#### 1.0.2 Position delta (`move_entity_pos*`)
Relative moves use steps of 1/4096 of a block, **computed on the quantized positions**:
`new = (round(old_base × 4096) + delta) / 4096` for each axis whose delta is non-zero; an axis with
delta 0 keeps **exactly** the base value (no rounding). If the three deltas are zero, the base is
returned as is.
The "base" is the last known server position of the entity (see §4.1), not its
displayed interpolated position.

Since 26.x the delta can be **multi-step** ("stepped"): see §1.3.

#### 1.0.3 Position path (used by `entity_position_sync`)
- `kind : varint` — 0 = linear, 1 = stepped (any other id → 0).
- kind 0: `position : vec3d` (final position).
- kind 1: `n : varint` then `n` × { `position : vec3d`, `tick_offset : varint` }; the final
  position is that of the last step.

#### 1.0.4 Set of relative flags (`teleport_entity`)
`i32`, bit `1 << n`:
| bit | meaning |
|---|---|
| 0 | relative X |
| 1 | relative Y |
| 2 | relative Z |
| 3 | relative yaw |
| 4 | relative pitch |
| 5 | relative velocity X |
| 6 | relative velocity Y |
| 7 | relative velocity Z |
| 8 | ROTATE_DELTA (rotate the velocity by the rotation difference) |

#### 1.1 `add_entity` (0x01)
| field | type | note |
|---|---|---|
| id | varint | network id of the entity |
| uuid | uuid | |
| type | id(minecraft:entity_type) | raw varint, built-in registry (order of `registries.json`) |
| x, y, z | f64 ×3 | absolute position (feet) |
| velocity | vec3lp | blocks/tick |
| pitch | angle | |
| body yaw | angle | |
| head yaw | angle | |
| data | varint | meaning depends on the type (see §4.2) |

No metadata in this packet: it follows in a `set_entity_data` (often in the same
`bundle`). Equipment follows in `set_equipment`.

#### 1.2 `remove_entities` (0x4E)
| field | type |
|---|---|
| ids | list(varint) — `varint` count then that many `varint` |

#### 1.3 Relative moves
Common field `properties : varint`: bit 0 = on ground; `step_count = properties >>> 1` (logical
shift).

Delta body depending on `step_count`:
- `step_count == 0` → **linear**: `dx : i16`, `dy : i16`, `dz : i16` (units 1/4096 block).
- `step_count > 0` → **stepped**: `step_count` × { `ticks : varint`, `dx : i16`, `dy : i16`,
  `dz : i16` }. Each step is relative to the decoded position of the previous step (the first
  to the base); the client checks `step_count ≤ remaining_bytes / 7` otherwise decoding error.
  `ticks` = time offset of the step (in ticks) within the path.
  Decoded, this gives a stepped path (absolute positions + offsets); the codec's base
  is NOT permanently advanced by decoding (it is by processing, §4.1).
  An empty stepped path (`step_count` = 0 is impossible here, it would be linear) — not applicable.

#### `move_entity_pos` (0x36)
`id : varint`, `properties : varint`, delta.

#### `move_entity_pos_rot` (0x37)
`id : varint`, `properties : varint`, delta, `yaw : angle`, `pitch : angle`.

#### `move_entity_rot` (0x39)
`id : varint`, `on_ground : bool`, `yaw : angle`, `pitch : angle`.
(No properties field here: direct boolean.)

#### 1.4 `teleport_entity` (0x80)
| field | type |
|---|---|
| id | varint |
| position | vec3d |
| velocity | vec3d (f64 ×3, full precision) |
| yaw | f32 (degrees) |
| pitch | f32 (degrees) |
| relatives | i32 (set of relative flags, §1.0.4) |
| on_ground | bool |

Absolute computation: for each axis/rotation marked relative, add the entity's current value;
otherwise the packet's value replaces it. (Same rule as `player_position`, already known to the client.)

#### 1.5 `entity_position_sync` (0x23)
| field | type |
|---|---|
| id | varint |
| path | position path (§1.0.3) |
| yaw | f32 (degrees) |
| pitch | f32 (degrees) |
| on_ground | bool |

Periodic absolute resynchronization (corrects the drift of the deltas).

#### 1.6 `rotate_head` (0x55)
`id : varint`, `head_yaw : angle`.

#### 1.7 `set_entity_motion` (0x67)
`id : varint`, `velocity : vec3lp`.

#### 1.8 `set_entity_data` (0x65)
`id : varint`, then a metadata list (§2). No length prefix.

#### 1.9 `set_equipment` (0x68)
`id : varint`, then repeat:
- `slot : i8` — bits 0..6 = slot, bit 7 (0x80) = "another entry follows";
- `item : stack` (optional stack, already specified).
Stop after the entry whose bit 7 is zero (at least one entry).

Slots (ordinal order):
| id | slot |
|---|---|
| 0 | main hand (mainhand) |
| 1 | offhand (offhand) |
| 2 | feet (feet) |
| 3 | legs (legs) |
| 4 | torso (chest) |
| 5 | head (head) |
| 6 | body — animal armor (body) |
| 7 | saddle (saddle) |

#### 1.10 `entity_event` (0x22)
`id : i32` (**fixed 4-byte integer**, not varint), `event : i8`.

Values useful for display (full list of the 26.3 constants):
1 jump (rabbit…) · 2 kinetic hit · 3 death · 4 attack start · 5 attack end ·
6 taming failed · 7 taming succeeded · 8 shake off (wet wolf) · 9 item use
finished · 10 eats grass (sheep) · 11 offers a flower (golem) · 12 love hearts · 13 angry
villager · 14 happy villager · 15 witch magic · 16 zombie conversion · 17 firework
explosion · 18 "in love" hearts · 19 squid sync · 20 silverfish merge · 21 guardian
sound · 22/23 reduced/full debug · 24..28 permission level (local player) · 31 fishing
rod reel-in · 32 armor stand wobble · 34 flower offer end · 35 totem (protected from
death) · 38 dolphin seeks treasure · 39 ravager stunned · 40/41 trust failed/succeeded ·
42 villager sweating · 45 fox eats · 46 teleportation (particles) · 47..52 equipment
break main hand/offhand/head/chest/legs/feet · 53 honey slide · 54 honey jump · 55 hand
swap · 56 cancels shaking off · 58/59 charge start/end (goat) · 60 "poof" (death
particles) · 61 tendril shiver (warden) · 62 sonic charge · 63 sniffer digging sound ·
64 armadillo peek · 65 body armor break · 66 shake · 67 drowning particles ·
68 saddle break · 69 ravager roar · 70 TNT priming · 71 boat sinking ·
72 boat launch.

#### 1.11 `animate` (0x02)
`id : varint`, `action : u8`.
| action | meaning |
|---|---|
| 0 | wake up (leaving the bed) |
| 1 | critical hit (particles) |
| 2 | magic critical hit (particles) |

The other historical values (arm swing, damage) are no longer used by this
packet: the swing goes through `swing_animation` (§1.12). [not verified: client behaviour
on an unknown value — presumably ignored]

#### 1.12 `swing_animation` (0x7B) — new
| field | type | note |
|---|---|---|
| id | varint | |
| hand | varint | 0 = main, 1 = offhand |
| animation type | varint | 0 = none, 1 = "whack" (classic hit), 2 = "stab" (thrust) |
| duration | varint | ticks (game default: whack, 6) |

#### 1.13 `hurt_animation` (0x2B)
`id : varint`, `yaw : f32` (direction of the hit in degrees, for the tilt of the hurt player).

#### 1.14 `damage_event` (0x19)
| field | type | note |
|---|---|---|
| id | varint | entity hit |
| damage type | id(minecraft:damage_type) | raw varint; **synchronized** registry (order of the `registry_data` received during configuration) |
| cause | varint | entity id + 1; 0 = none |
| direct source | varint | entity id + 1; 0 = none |
| source position | option(vec3d) | `bool` then 3 × f64 |

#### 1.15 `set_passengers` (0x6D)
`vehicle : varint`, `passengers : varint` count then that many `varint` (ids). Ordered list:
the first one is the driver.

#### 1.16 `set_entity_link` (0x66)
`attached : i32`, `holder : i32` (fixed 4-byte integers). `holder = 0` → leash detached.
(Leash displayed between the attached entity and the holder.)

#### 1.17 `take_item_entity` (0x7F)
`item : varint` (id of the picked-up entity — item, arrow, xp orb), `collector : varint`,
`count : varint`. Animates the item flying toward the collector then removes it.

#### 1.18 `update_mob_effect` (0x87)
| field | type |
|---|---|
| id | varint |
| effect | id(minecraft:mob_effect) — raw varint, built-in registry |
| amplifier | varint |
| duration | varint (ticks; −1 = infinite) |
| flags | i8 — 1 ambient, 2 visible (particles), 4 icon shown, 8 blend (fade) |

#### 1.19 `remove_mob_effect` (0x4F)
`id : varint`, `effect : id(minecraft:mob_effect)`.

#### 1.20 Other entity packets useful for display
- `update_attributes` (0x86): `id : varint`, list (max 128) of { `attribute :
  id(minecraft:attribute)`, `base : f64`, modifiers: `varint` count × { `id : string`
  (identifier), `amount : f64`, `operation : varint` (0 add_value, 1 add_multiplied_base,
  2 add_multiplied_total) } }. Useful for `minecraft:scale` (rendered size of living entities).
- `move_minecart_along_track` (0x38): `id : varint`, list of { `position : vec3d`, `velocity :
  vec3d`, `yaw : angle`, `pitch : angle`, `weight : f32` } — minecart trajectory
  (experimental rule "new minecarts").
- `projectile_power` (0x8A): `id : varint`, `power : f64` (fireballs).
- `sound_entity` (0x76): `sound` (ref(minecraft:sound_event), 0 = direct), `source : varint`,
  `id : varint`, `volume : f32`, `pitch : f32`, `seed : i64`.
- `set_camera` (0x5F), `move_vehicle` (0x3A), `player_look_at` (0x48): not detailed here.
- `debug/entity_value` (0x1C): debugging, ignorable.
- `bundle_delimiter` (0x00): the server frames the "pairing" packets of an entity with two
  delimiters, sent in this order: `add_entity`, `set_entity_data` (if non-default values),
  `update_attributes` (living entities, synchronizable attributes), `set_equipment`
  (living entities, non-empty slots), `set_passengers` (if passengers), `set_passengers` of the
  vehicle (if it is a passenger), `set_entity_link` (if leashed). The client must
  apply the whole packet at once (same tick).

---

#### 2. Entity metadata (`set_entity_data`)

#### 2.1 Encoding of the list
Repeat:
- `index : u8`. If `index == 0xFF` → end of list.
- `type : varint` — serializer id (§2.2). Unknown type → decoding error.
- `value` encoded according to the serializer.

The client stores each value in the entity's slot `index`. The official game raises an error
if the received type differs from the type declared for that index (advice: ignore the entry and log
rather than crash). An index never received keeps its default value (§3).

#### 2.2 Serializer table (26.3, registration order = id)

| id | name | encoding of the value |
|---|---|---|
| 0 | byte | `i8` |
| 1 | int | `varint` |
| 2 | long | `varlong` |
| 3 | float | `f32` |
| 4 | string | `string(32767)` |
| 5 | component | `text` (component in network NBT, no "trusted" size limit) |
| 6 | optional component | `bool` then, if true, `text` |
| 7 | item stack | `stack` (optional stack: count 0 = empty) |
| 8 | boolean | `bool` |
| 9 | rotations | `f32` ×3 (x, y, z in degrees) |
| 10 | block pos | `position` |
| 11 | optional block pos | `bool` then `position` |
| 12 | direction | `varint`: 0 down, 1 up, 2 north, 3 south, 4 west, 5 east |
| 13 | optional living entity ref | `bool` then `uuid` |
| 14 | block state | `varint` (global block state id) |
| 15 | optional block state | `varint`; 0 = absent (so air is not representable), otherwise state id |
| 16 | particle | particle (§2.3) |
| 17 | particles | `varint` count then that many particles (§2.3) |
| 18 | villager data | `type : id(minecraft:villager_type)`, `profession : id(minecraft:villager_profession)`, `level : varint` |
| 19 | optional unsigned int | `varint` v; 0 = absent, otherwise value = v − 1 |
| 20 | pose | `varint` (table §2.4) |
| 21 | cat variant | `id(minecraft:cat_variant)` |
| 22 | cat sound variant | `id(minecraft:cat_sound_variant)` |
| 23 | cow variant | `id(minecraft:cow_variant)` |
| 24 | cow sound variant | `id(minecraft:cow_sound_variant)` |
| 25 | wolf variant | `id(minecraft:wolf_variant)` |
| 26 | wolf sound variant | `id(minecraft:wolf_sound_variant)` |
| 27 | frog variant | `id(minecraft:frog_variant)` |
| 28 | pig variant | `id(minecraft:pig_variant)` |
| 29 | pig sound variant | `id(minecraft:pig_sound_variant)` |
| 30 | chicken variant | `id(minecraft:chicken_variant)` |
| 31 | chicken sound variant | `id(minecraft:chicken_sound_variant)` |
| 32 | zombie nautilus variant | `id(minecraft:zombie_nautilus_variant)` |
| 33 | optional global pos | `bool` then { `dimension : string` (identifier), `position` } |
| 34 | painting variant | `ref(minecraft:painting_variant)`; if 0 → direct variant: `width : varint`, `height : varint`, `asset : string` (texture identifier), `title : option(text)`, `author : option(text)` |
| 35 | sniffer state | `varint`: 0 idle, 1 feeling happy, 2 scenting, 3 sniffing, 4 searching, 5 digging, 6 rising |
| 36 | armadillo state | `varint`: 0 idle, 1 rolling up, 2 scared (rolled up), 3 unrolling |
| 37 | copper golem state | `varint`: 0 idle, 1 takes an item, 2 finds nothing, 3 drops an item, 4 drops nothing |
| 38 | weathering copper state | `varint`: 0 unaffected, 1 exposed, 2 weathered, 3 oxidized (out of bounds → clamped) |
| 39 | vector3 | `f32` ×3 |
| 40 | quaternion | `f32` ×4 in the order x, y, z, w |
| 41 | resolvable profile | resolvable profile — same encoding as the item component `minecraft:profile` (see `item-components.md`) |
| 42 | humanoid arm | `varint`: 0 left, 1 right |
| 43 | dye color | `varint` 0..15: white, orange, magenta, light blue, yellow, lime, pink, gray, light gray, cyan, purple, blue, brown, green, red, black |

Remarks:
- The variant registries (cat, cow, wolf, frog, pig, chicken, zombie nautilus, associated sounds,
  painting; villager type/profession are built in for the last two): the animal and painting
  variants are registries **synchronized** during configuration;
  the raw id is the index in the order of the entries received by `registry_data`. The registries
  `villager_type` and `villager_profession` are built in (order of `registries.json`).
- `id(...)` without "+1" here (pure "by registry" holder), except `painting variant` which is a
  `ref` (0 = direct, otherwise id + 1).

#### 2.3 Particle codec (serializers 16 and 17)
`type : id(minecraft:particle_type)` (built-in registry, order of `registries.json`), then the
options depending on the type. Types without options: nothing more. Types with options:

| type(s) | options |
|---|---|
| `block`, `block_marker`, `falling_dust`, `dust_pillar`, `block_crumble` | `state : varint` (block state id) |
| `dust` | `color : i32` (RGB), `scale : f32` |
| `dust_color_transition` | `from : i32` (RGB), `to : i32` (RGB), `scale : f32` |
| `entity_effect`, `tinted_leaves`, `flash` | `color : i32` (ARGB) |
| `effect`, `instant_effect` | `color : i32`, `power : f32` |
| `dragon_breath` | `power : f32` |
| `sculk_charge` | `roll : f32` |
| `shriek` | `delay : varint` |
| `item` | stack template (see `item-components.md`): `item : id(minecraft:item)`, `count : varint`, component patch |
| `vibration` | position source: `kind : id(minecraft:position_source_type)` (0 = `block` → `position`; 1 = `entity` → `entity_id : varint`, `y_offset : f32`), then `arrival_ticks : varint` |
| `trail` | `target : vec3d`, `color : i32`, `duration : varint` |
| `geyser`, `geyser_plume` | `water_blocks : i32` |
| `geyser_base`, `geyser_poof` | `water_blocks : i32`, `base_impulse : f32` |

For metadata, only index 10 of the "living entity" layer (list) and a few entities (lingering
effect cloud, etc.) use it; in practice potion effects mostly send
`entity_effect` with an ARGB color.

#### 2.4 Poses (serializer 20)
0 standing · 1 elytra flight · 2 sleeping · 3 swimming · 4 spin attack (trident) · 5 crouching ·
6 long jump · 7 dying · 8 croaking · 9 using tongue · 10 sitting · 11 roaring · 12 sniffing ·
13 emerging · 14 digging · 15 sliding · 16 shooting · 17 inhaling. Out of bounds → 0.

---

#### 3. Metadata indexes (26.3 inheritance chain)

Indexes are assigned per class in declaration order, following the last index of
the parent class. (Declaration order ≠ order in which the defaults are defined: rely on this
table.)

#### 3.1 Entity (all entities) — indexes 0 to 7
| idx | type | meaning | default |
|---|---|---|---|
| 0 | byte | shared flags (below) | 0 |
| 1 | int | remaining air (ticks) | 300 (max air) |
| 2 | optional component | custom name | absent |
| 3 | boolean | custom name always visible | false |
| 4 | boolean | silent | false |
| 5 | boolean | no gravity | false |
| 6 | pose | pose | 0 (standing) |
| 7 | int | freeze ticks (powder snow) | 0 |

Flags of index 0 (bit = `1 << n`):
| n | mask | meaning |
|---|---|---|
| 0 | 0x01 | on fire (the client shows it if not fire immune) |
| 1 | 0x02 | sneak key held |
| 2 | 0x04 | (unused) |
| 3 | 0x08 | sprinting |
| 4 | 0x10 | swimming |
| 5 | 0x20 | invisible |
| 6 | 0x40 | glowing |
| 7 | 0x80 | elytra flight |

Note: the crouching silhouette is rendered from the **pose** (5), not from bit 1.

#### 3.2 "Living entity" layer — indexes 8 to 14
| idx | type | meaning | default |
|---|---|---|---|
| 8 | byte | living flags: 0x01 using an item, 0x02 hand used = offhand (otherwise main), 0x04 spin attack (Riptide trident) | 0 |
| 9 | float | health | 1.0 |
| 10 | particles | effect particles (list, usually `entity_effect` ARGB) | empty list |
| 11 | boolean | "ambient" effects (beacon: more discreet particles) | false |
| 12 | int | arrows stuck in the body | 0 |
| 13 | int | bee stingers stuck | 0 |
| 14 | optional block pos | bed position (sleeping) | absent |

Invisibility by effect has no index of its own: the server sets bit 5 of index 0.

#### 3.3 Avatar (class common to players and the mannequin) — indexes 15 to 16
| idx | type | meaning | default |
|---|---|---|---|
| 15 | humanoid arm | main arm | 1 (right) |
| 16 | byte | visible skin parts: bit `1 << n`, n = 0 cape, 1 jacket, 2 left sleeve, 3 right sleeve, 4 left leg, 5 right leg, 6 hat | 0 |

#### 3.4 Player — indexes 17 to 20
| idx | type | meaning | default |
|---|---|---|---|
| 17 | float | absorption (yellow hearts) | 0 |
| 18 | int | score | 0 |
| 19 | optional unsigned int | left shoulder parrot: variant (0 red-blue, 1 blue, 2 green, 3 yellow-blue, 4 gray) | absent |
| 20 | optional unsigned int | right shoulder parrot (same values) | absent |

(The `mannequin` inherits from Avatar but not from Player: its own indexes start at 17.)

#### 3.5 Mob — index 15
(Mob inherits directly from the "living entity" layer; Avatar is not in its chain.)
| idx | type | meaning | default |
|---|---|---|---|
| 15 | byte | 0x01 no AI, 0x02 left-handed, 0x04 aggressive (raised arms of zombies, etc.) | 0 |

The "path-finding mob" layer adds no index.

#### 3.6 Ageable mob layer — indexes 16 to 17
| idx | type | meaning | default |
|---|---|---|---|
| 16 | boolean | baby | false |
| 17 | boolean | age locked | false |

---

#### 4. Client side: state, interpolation, local player

#### 4.1 State to keep per entity
- `id`, `uuid`, `type`, creation data (§4.2).
- **Position codec base** (`base`, vec3d): last known server position, used to
  decode the deltas (§1.0.2). Distinct from the displayed position.
- Current position `pos` and previous `pos_o`; yaw `yaw`/`yaw_o`, pitch `pitch`/`pitch_o`;
  for living entities: body yaw `body_yaw`/`body_yaw_o`, head yaw `head_yaw`/`head_yaw_o`.
- Velocity `velocity` (blocks/tick), `on_ground`.
- Indexed metadata array (§2-3), equipment (8 slots), passengers / vehicle,
  leash holder, active effects (id, amplifier, duration, flags), attributes.
- Interpolation state (§4.4).

Cycle per client tick (20 Hz at most: tick duration = max(50 ms, server ms per tick), so slower if `ticking_state` slows down, never faster; `protocol-supplements.md`): `pos_o ← pos`,
`rot_o ← rot`, then interpolation step, then the entity's own logic.
At render: `render_pos = lerp(partial_t, pos_o, pos)`; yaws in angular interpolation
(shortest path), pitch linear. The client "unwraps" `*_o` by ±360° so that the difference
with the current value stays within [−180, 180).

#### 4.2 On receiving `add_entity`
1. If the type is `player`: the player list entry (`player_info_update`) for this UUID must
   already exist, otherwise the packet is ignored (the game logs a warning). The profile
   (skin) comes from this entry.
2. Create the entity of the given type.
3. **All entities**: `base ← (x, y, z)`; position = (x, y, z) without interpolation, `pos_o =
   pos`; `yaw`, `pitch` from the packet (decoded angles); `rot_o = rot`; velocity = packet velocity.
4. **Living entities**: same, but `body_yaw = head_yaw = head yaw of the packet` (the body yaw is
   initialized from the **head**, not from `yaw`), identical `*_o`; pitch clamped to
   [−90, 90]; yaw taken modulo 360.
5. Remote players: in addition, `pos_o`/`rot_o` copied (no sliding on the first tick).
6. `data` field depending on the type:
   | type | meaning of `data` |
   |---|---|
   | `item_frame`, `glow_item_frame`, `painting` | attachment direction (0..5: down, up, north, south, west, east) |
   | `falling_block` | block state id |
   | projectiles (arrows, fireballs, snowballs, tridents…) | id of the owner entity (0 = none) |
   | `fishing_bobber` | id of the owner (the rod) |
   | `warden` | 1 = appears in "emerging" pose |
   | others | ignored (0) |
7. The metadata arrives right after (`set_entity_data`, same bundle); as long as it is
   missing, use the defaults of §3.

#### 4.3 Processing of movement packets (remote entity)
"Locally authoritative" = the entity is the local player, or a vehicle whose controlling passenger is
the local player. For these entities, movement packets **only update the base** and move nothing.

- `move_entity_pos` / `move_entity_pos_rot`: decode the delta with the current base → path;
  `base ← final position of the path`. If not authoritative: interpolate toward the path (with or without
  rotation depending on the packet), then `on_ground ← packet`.
- `move_entity_rot`: if not authoritative, interpolate the rotation alone (the position target is
  the current target, or the current position), `on_ground ← packet`.
- `entity_position_sync`: `base ← final position`. If not authoritative: if the squared distance between
  the current position and the target exceeds **4096** (64 blocks) or if the entity is not in a
  "ticked" zone, direct placement (`pos = pos_o = target`, direct rotations); otherwise
  interpolation. Then `on_ground`. If the entity does not interpolate and carries the local player,
  reposition the latter on its seat.
- `teleport_entity`: compute the absolute value (§1.4) from the **current interpolation
  target** if one exists (otherwise from the current state). The resulting pitch is clamped to [−90, 90].
  Velocity: relative component = current velocity (possibly rotated by the difference in
  rotation if ROTATE_DELTA) + packet; otherwise the packet's value.
  Interpolate if (ticked zone OR non-authoritative OR a relative position axis) AND distance² ≤ 4096;
  otherwise direct placement, and `pos_o`/`rot_o` recomputed by applying the same relative change to
  the old position (no trailing). This packet also applies to the authoritative entity.
  If the entity does not exist but was the vehicle of the local player just removed, apply
  the teleport to the local player.
- `rotate_head`: living entities → interpolation of the head toward the target over **3 ticks**:
  at each tick `head_yaw += wrap(target − head_yaw) / n` then `n −= 1`. Non-living entities: value
  set directly.
- `set_entity_motion`: velocity replaced immediately (including for the local player:
  knockback). Remote player: the velocity is approached over `interval + 1` ticks (interval = 2
  for players, so 3 ticks; 1 tick if the type has no interval), in equal steps.
- `remove_entities`: mark as removed and forget. If the entity was carrying the local player (even
  indirectly), remember its id (see `teleport_entity`); an `add_entity` of the same id erases
  this memory.
- `set_passengers`: empty the vehicle's passengers, then mount each known id, in
  order. Unknown vehicle → ignored. If the local player newly boards a boat:
  its yaws (current, old, head) take the boat's yaw; message "mount.onboard"
  (or "mount.sit" for `cushion`) in the action bar.
- `set_entity_link`: the holder is resolved lazily (the id may arrive before the
  entity). 0 = detach.
- `take_item_entity`: pickup sound (xp orb: volume 0.1; other: 0.2), pickup particle
  (copy of the appearance of the flying item toward the collector); for an item entity,
  reduce the stack by `count` and remove it if empty; arrows etc. removed; the xp orb is
  not removed by this packet.
- `hurt_animation`: living entities → `hurt_time = hurt_duration = 10` ticks (red tint); players →
  in addition `hurt_dir = yaw` (tilt of the local player's camera).
- `damage_event`: living entities → same 10 ticks of red tint, walk animation speed
  forced to 1.5, hurt sound.
- `entity_event` 3 (death) on a non-player living entity: health ← 0 and start of the death animation.
- `swing_animation`: triggers the arm swing of the indicated hand with the type and
  duration received.
- `set_equipment`: applied only to living entities.

#### 4.4 Interpolation of positions and rotations

#### 4.4.1 Who interpolates, and over how many ticks
Each entity type has an "update interval" (ticks between two server sends):
| interval | types |
|---|---|
| 1 | `block_display`, `item_display`, `text_display` |
| 2 | `player`, `mannequin`, `allay`, `evoker_fangs` |
| 3 | all the others (default) |
| 4 | `eye_of_ender` |
| 5 | `fishing_bobber` |
| 10 | thrown projectiles (fireballs, egg, pearl, xp bottle, firework, llama spit, potions, snowball, primed TNT, wind charges, wither skull, dragon breath) |
| 20 | `arrow`, `spectral_arrow`, `trident`, `experience_orb`, `falling_block`, `item` |
| never | `area_effect_cloud`, `end_crystal`, item frames, `leash_knot`, `lightning_bolt`, `painting`, `cushion` |

Interpolation method depending on the class:
| entities | method |
|---|---|
| living entities (mobs, players, armor stand…) | **stepped**, default duration = the type's interval (3; 2 for players) |
| boats, minecarts (classic movement), xp orbs, fishing bobber | **linear**, 3 steps |
| display entities (`*_display`) | linear at 0 steps = direct placement |
| minecarts in experimental movement, shulker | none (direct placement; minecarts follow `move_minecart_along_track`) |
| all the others (dropped items, arrows, projectiles, falling blocks, TNT…) | none: direct placement; these entities simulate their physics client-side with the received velocity |

In all cases, if an interpolation is requested but the method does not accept it,
the position and rotation are set directly.

Shulker, client side: any position set outside a mount is re-centered (`floor(x) + 0.5`,
`floor(y + 0.5)`, `floor(z) + 0.5`), spawn included. After the first tick, a block change
resets locally the targeted opening (data 17) to 0 and, outside a mount and if the new
block is not the remembered old block, starts a render slide of 6 ticks
`−(block − old block) × ((remaining − pt) / 6)²` (old position = new). The body rotation
control does nothing: body at 0° on spawn and on dismount, head never brought back toward the
body. Mounting cancels the slide and sets the attachment to down;
dismounting remembers the current block as the old block. No shadow (radius 0).

#### 4.4.2 Linear interpolation (N steps)
- New target: if an interpolation is in progress toward exactly the same target (position,
  yaw, pitch), do nothing; otherwise target ← (pos, yaw, pitch), `remaining ← N`.
  If N = 0 → direct placement.
- Each tick, while `remaining > 0`: `α = 1 / remaining`;
  `pos ← lerp(α, pos, target)`; `yaw ← yaw + α × wrap(target_yaw − yaw)`;
  `pitch ← lerp(α, pitch, target_pitch)`; `remaining −= 1`.
  (So the target is reached exactly on the N-th tick.)

#### 4.4.3 Stepped interpolation (living entities)
State: queue of steps `(position, yaw, pitch, duration_ticks)`, start point
`(pos₀, yaw₀, pitch₀)`, counter `step_t` (float), `remaining_t` (sum of the unconsumed durations),
speed `v` (initially 1), N = default duration.

New target (path, yaw, pitch):
1. If no step is in progress: start point = current state, `step_t = 1`.
2. If the final position of the path equals the last recorded target: add one step
   (final position, yaw, pitch, N) — "rotation only" case.
3. Otherwise, **linear** path: add one step (position, yaw, pitch, N).
   **Stepped** path: if yaw and pitch equal the last target, add each step
   with these rotations and its own duration (`tick_offset`); otherwise distribute the rotation:
   `T = Σ offsets`, and for step k, `a = (Σ offsets up to k) / T`, rotation of the step =
   interpolation (angular for yaw, linear for pitch) between the last target and the
   new one with `a`.
4. `remaining_t += duration` of each added step; the last target becomes (end, yaw, pitch).

Each tick:
1. Computation of the displayed state: while the queue is not empty, take the first step of
   duration `d`; if `step_t < d`: `a = step_t / d`, state = interpolation between the start point
   and the step (linear position, angular yaw, linear pitch) — end; otherwise
   `step_t −= d`, the start point becomes the step, remove it and continue. Empty queue → state =
   last target.
2. Set position and rotations.
3. Advance: `r = tick_relative_speed` (1, or rate/20 if the server speeds up beyond
   20 tps); `v_target = max(remaining_t / N, 1)`; `v ← v + (v_target − v) / N`; if `r × v <
   remaining_t` then `step = r × v` otherwise `step = remaining_t` and `v ← 1`; `step_t += step`,
   `remaining_t −= step`.
   → In normal regime, a step of N ticks is traversed in N ticks; if packets pile up,
   `v` increases to catch up.

#### 4.4.4 Correction by local movement (both methods)
Before each interpolation step, if the entity has moved locally since the last step (push,
collision, physics simulated on both sides) by a squared difference > 1e-5, this difference is added to the target
and to all the remaining steps, **provided that** the collision box at the shifted target does not
hit anything. Local yaw/pitch differences are likewise carried onto the targets.

#### 4.5 Body and head (living entities)
- The yaw `yaw` received from movement packets is the "aim" orientation; `head_yaw` comes from
  `rotate_head`; `body_yaw` (rendered body) is **computed client-side** every tick.
- Non-mob living entities (remote players, armor stands excepted): after the tick logic,
  `dx, dz` = horizontal movement since the previous tick. Body target = current `body_yaw`;
  if `dx² + dz² > 0.0025`: walking direction `w = atan2(dz, dx) in degrees − 90`; if
  the difference between `wrap(yaw)` and `w` is between 95° and 265° (walking backward), target = `w − 180`,
  otherwise target = `w`. If an arm swing is in progress, target = `yaw`.
  Then `body_yaw += wrap(target − body_yaw) × 0.3`; if `|wrap(yaw − body_yaw)| > 50°`, the body is
  brought back to 50° from the aim yaw.
- Mobs: body rotation control:
  - if the mob moves (`dx² + dz² > 2.5e-7`): `body_yaw = yaw`, then the head is brought to at
    most 75° from the body; remember `head_yaw` as "stable head", counter = 0;
  - otherwise (and if it is not carrying a mob): if the head has turned by more than 15° since the stable head,
    counter = 0, new stable head, and the body is brought to at most 75° from the head;
    otherwise counter += 1 and, beyond 10 ticks, the body turns progressively toward the head:
    max allowed difference = `75 × (1 − clamp((counter − 10)/10, 0, 1))` (full alignment in
    10 additional ticks).
  - 75° = default value (some mobs redefine it [not verified: list]).
- Armor stand: `body_yaw = yaw` always.
- Remote players: the head interpolates over 3 ticks (`rotate_head`), the pitch is that
  of the movement packets.

#### 4.6 Local player
- The server never sends `add_entity` for the local player; its id comes from `login`
  (and stays valid after `respawn`).
- Packets targeting its own id to **apply**: `set_entity_data` (pose, flags, air, freeze,
  health, absorption, skin parts, main arm… the client reads them for third-person display
  and the HUD), `set_entity_motion` (knockback), `entity_event` (9 use finished, 22/23
  debug, 24..28 permission level, 35 totem — full-screen item animation —, 2 kinetic
  hit, etc.), `update_mob_effect` / `remove_mob_effect`, `update_attributes`,
  `hurt_animation`, `damage_event`, `animate` 0 (leaving the bed), `swing_animation`,
  `take_item_entity` (itself as collector), `set_passengers` (itself as passenger),
  `teleport_entity` (rare; the server normally uses `player_position`).
- To **ignore** for position: `move_entity_pos*`, `move_entity_rot`,
  `entity_position_sync` targeting the local player or the vehicle it drives — only update the
  codec base (§4.3). Its position comes from its own simulation and from `player_position`.
- `set_equipment`: the server does not normally send it for oneself (the inventory goes through the
  container packets); if it arrives, it applies (living entity).

---

### Uncertain points

1. `animate` with an action ≠ 0..2: the 26.3 client has no branch; assumed ignored. An
   `animate` 0 on a non-player entity would make the official client fail (forced conversion to
   player) — to be treated as ignored.
2. List of the mobs that redefine the max head/body angle (75° by default) and the 50° of
   non-mob living entities: not recorded.
3. Exact meaning of `tick_offset` in stepped paths: read as "duration of the step in
   ticks since the previous step" (this is how the interpolation uses it);
   server-side it is the number of ticks elapsed since the previous step.
4. Order of the ids of the synchronized registries (variants, damage types): it follows the order of the
   `registry_data` entries received during configuration [not verified on a capture].
5. The table of update intervals (§4.4.1) was extracted automatically from the
   declaration of the types; check `player` = 2 and the default 3 on a capture (living entities
   use this interval as interpolation duration).
6. `update_attributes`: list of the "synchronizable" attributes sent to the client not recorded
   (only `minecraft:scale` matters for display).
7. Entities without interpolation (dropped items, arrows, projectiles): the client simulates them with
   their velocity; the client physics of these entities is not described here (mission B/C or to be done).

## Entities — mission B: models (geometry) and rendering pipeline

Target version: Minecraft Java 26.3 (protocol 777). Sources read: decompiled game
(client geometry models, client rendering, low-level graphics library,
pearl rendering) and shaders `assets/minecraft/shaders/core/entity.*`,
`include/light.glsl`, `include/sample_lightmap.glsl`. Written in our own words, no code copied.

---

#### 1. Model system

#### 1.1 Units and frame

- Model unit = the **pixel** = 1/16 of a block. All pose and cube positions are in
  pixels; conversion to blocks (÷16) happens in two places: the pose translation of a
  part (÷16 when the pose is applied) and each vertex of a cube (÷16 when emitting).
  Scales and rotations are not converted.
- Model frame: **Y down**, X and Z "normal" (right-handed frame). The root of a biped/quadruped
  mob model has its "ground" at about y = +24 px (hence the constants 24 / 1.501
  of §4). The flip to the world (Y up) is done by the (−1, −1, 1) scale of the living-entity
  renderer (§4), not by the model.
- Rotations in **radians**.

#### 1.2 Model part (tree node)

A part carries:

| field | meaning |
|---|---|
| `x, y, z` | offset (pivot) in pixels, in the parent's frame |
| `rot_x, rot_y, rot_z` | rotations in radians |
| `scale_x, scale_y, scale_z` | scale, 1 by default |
| `visible` | false ⇒ neither the part nor its children are drawn |
| `no_draw` | true ⇒ the part's cubes are not emitted but the children are (the pose still applies) |
| cubes | ordered list (order of addition in the definition) |
| children | table name → part |
| initial pose | copy of the definition pose; resetting the pose reloads it (at the start of each animation setup) |

Definition pose: 9 floats (x, y, z, rot_x, rot_y, rot_z, scale_x, scale_y, scale_z);
the "offset", "rotation", "offset and rotation" factories set the scale to 1.

**Local matrix of a part** (applied to the right of the current matrix, so the last
operation listed acts first on the vertices):

`M_part = T(x/16, y/16, z/16) · R · S(scale_x, scale_y, scale_z)`

with `R = Rz(rot_z) · Ry(rot_y) · Rx(rot_x)` (quaternion "rotation in Z, Y, X order": on a vector, X first,
then Y, then Z). R is omitted if the three angles are zero, S if the three scales are 1
(optimization with no effect on the result).

**Recursive drawing** of a part with (matrix stack, color, light, overlay):
1. if `visible` is false → nothing;
2. if the part has neither cube nor children → nothing (not even a push);
3. push the matrix; multiply by `M_part`;
4. if `no_draw` is false → emit all the cubes of the part (list order);
5. draw each child recursively (order of the children table, see 1.6);
6. pop.

The root of a model is itself a part (no cube, pose ZERO except mesh transformation
§2.3), named "root" in name lookups.

#### 1.3 Cube (definition)

Fields of a cube definition:

| field | type | meaning |
|---|---|---|
| `texture_offset` (u, v) | integers (stored as float, **truncated to int** at baking) | top-left corner of the unfolded template in the texture, in texels |
| origin (x0, y0, z0) | floats, pixels | minimal corner of the cube, part frame |
| dimensions (w, h, d) | floats, pixels | width X, height Y, depth Z |
| inflation (gx, gy, gz) | floats, pixels | "cube deformation"; a single-value constructor sets the same on the 3 axes; can be negative |
| mirror | bool | inherited from the current state of the list builder at the time of adding (or forced by an overload) |
| texture scale (su, sv) | floats, 1 by default | multiplies the layer's texture size **for this cube only** |
| visible faces | set of directions | all by default; an overload allows omitting some |

Inflation changes **only the positions**, never the UVs (the template is computed with
non-inflated w, h, d). Usual inflation constants seen in the layer registry:
0.5 (inner armor), 1.0 (outer armor), 1.02 (piglin outer armor), 0.008
(tropical fish pattern), (−0.1, 0.5, 0.3) and (−0.1, 0.3, 0.3) (baby armors), 0.7 (baby
piglin armor).

#### 1.4 Exact generation of the 6 faces

Notation (pixels): `X0 = x0 − gx`, `X1 = x0 + w + gx`, `Y0 = y0 − gy`, `Y1 = y0 + h + gy`,
`Z0 = z0 − gz`, `Z1 = z0 + d + gz`. **If mirror: swap X0 and X1** (after inflation).

Eight corners:

| name | position |
|---|---|
| t0 | (X0, Y0, Z0) |
| t1 | (X1, Y0, Z0) |
| t2 | (X1, Y1, Z0) |
| t3 | (X0, Y1, Z0) |
| l0 | (X0, Y0, Z1) |
| l1 | (X1, Y0, Z1) |
| l2 | (X1, Y1, Z1) |
| l3 | (X0, Y1, Z1) |

Template bounds (texels, u and v = truncated texture_offset):
`u0 = u`, `u1 = u + d`, `u2 = u + d + w`, `u22 = u + d + w + w`, `u3 = u + d + w + d`,
`u4 = u + d + w + d + w`; `v0 = v`, `v1 = v + d`, `v2 = v + d + h`.
**Sums in f32 evaluated left to right, in this exact order**: with fractional dimensions
(e.g. depth 0.005 of some layers), writing `u + 2d + w` gives a UV that
differs in the last bit (observed on 6 faces).

> Verification (bench, probe of §2.4): these rules recomputed in Java for the 361 layers
> reproduce **to the bit** the 15,058 faces baked by the game (only exception: the layer
> `minecraft:boat#water_patch`, with texture size 0×0, whose UVs are NaN — it is a
> depth mask without a texture, see §3.9).

Faces, **in emission order** (a face absent from the "visible" set is skipped,
the order of the others is kept): each face = quad (a, b, c, d) + UV rectangle
(Ua, Va) → (Ub, Vb):

| # | direction (model normal) | vertices a, b, c, d | Ua | Va | Ub | Vb |
|---|---|---|---|---|---|---|
| 1 | DOWN (0, −1, 0) — face at Y0 | l1, l0, t0, t1 | u1 | v0 | u2 | v1 |
| 2 | UP (0, +1, 0) — face at Y1 | t2, t3, l3, l2 | u2 | v1 | u22 | v0 |
| 3 | WEST (−1, 0, 0) — face at X0 | t0, l0, l3, t3 | u0 | v1 | u1 | v2 |
| 4 | NORTH (0, 0, −1) — face at Z0 | t1, t0, t3, t2 | u1 | v1 | u2 | v2 |
| 5 | EAST (+1, 0, 0) — face at X1 | l1, t1, t2, l2 | u2 | v1 | u3 | v2 |
| 6 | SOUTH (0, 0, +1) — face at Z1 | l0, l1, l2, l3 | u3 | v1 | u4 | v2 |

Remarks: for UP, Va > Vb (rectangle read "upside down" in V); the names DOWN/UP are those
of the model frame (Y down): after the (−1,−1,1) scale of the rendering, the "DOWN" face (Y0,
template at the top of the unfolded layout) becomes the visual **top** of the box.

**UV assignment** on a quad (a, b, c, d) and a rectangle (Ua, Va, Ub, Vb), with
`TW = layerTextureWidth × su`, `TH = layerTextureHeight × sv`:
- a ← (Ub/TW, Va/TH)
- b ← (Ua/TW, Va/TH)
- c ← (Ua/TW, Vb/TH)
- d ← (Ub/TW, Vb/TH)

(No half-texel inset: the internal offset is 0.)

**Mirror**, in addition to the X0/X1 swap:
- the order of the 4 vertices is **reversed** (a,b,c,d → d,c,b,a) **after** the UV assignment
  (each vertex keeps its UV);
- the normal of the X-axis faces is **flipped** (WEST carries (+1,0,0), EAST (−1,0,0)); the other
  normals are unchanged.

Winding: without mirror, the four vertices are **counter-clockwise seen from outside**
(right-handed frame; checked on DOWN: cross product (b−a)×(c−a) collinear with the normal). The
mirror (determinant −1) reverses the geometric winding, and the order reversal restores it.
The (−1,−1,1) scale of the rendering has determinant +1 and keeps it. The faces are emitted as
**quads** (QUADS topology → two triangles a,b,c / a,c,d on the engine side).

**Vertex emission**: position = current pose matrix × (px/16, py/16, pz/16, 1);
normal = normal matrix of the pose × normal of the face (one normal per face, the same for
the 4 vertices); plus color, UV, overlay, light (common to the whole draw call, §3).
The bounding box of a baked cube (min/max) is the **non-inflated** box.

#### 1.5 Layer texture size

Each layer = mesh + "material" = (textureWidth, textureHeight)
integers, typically 64×64, 64×32, 128×128… These numbers only serve to normalize the UVs
(they are not necessarily the real size of the PNG: a PNG 2× or 4× larger remains correct
since the UVs are normalized).

#### 1.6 Order of children

During definition, the children of a part are stored in a hash table (iteration order
of the Java hash table on `String` keys); at baking, this iteration order is frozen into an
array-backed table. **The order only matters for translucent render types** (emission order in a
single buffer); to be exact, the dump (§2) must write the children in the actual iteration order of
the baked model part.
Redefining an existing child (same name) **replaces** its pose and cubes but **keeps** its
own children.

---

#### 2. Layer definitions and JSON dump

#### 2.1 Where the game creates them

- A static registry gives the **layer locations** (a location = identifier + layer name, displayed `minecraft:pig#main`). A simple location has the layer `main`; other named layers exist (`outer`, `inner`, `saddle`, `eyes`, `flag`, `water_patch`…); the
  **armor sets** register four layers `helmet`, `chestplate`, `leggings`, `boots` for
  the same model (23 sets). The known set is exposed by this registry.
- The public static function creating the layer roots (called by the extractor):
  builds the table **location → layer definition** by calling the static factories of each model
  (body, mesh, armor sets, baby version…, spread across the client's model package and a few
  block-entity renderers), possibly applies a mesh transformation, then **checks** that every known location has
  a definition (otherwise exception). Several locations share the same definition (e.g. all
  the boats of the same shape).
- The game's set of models wraps this table; baking a location gives a fresh
  tree of model parts. Each entity renderer bakes its layers in its constructor,
  then animates them every frame.
- **Mesh transformation**: only touches the **root pose**. Scaling
  by a factor `f` multiplies the offset and the scale of the root by `f`, then
  adds `24.016 × (1 − f)` to its y (keeps the feet on the ground). Dumped example:
  `minecraft:pillager#main` has the root (0, 1.501, 0) scale 0.9375 (the "villager" constant).
  The dump of §2.3 writes the already-transformed result: nothing to redo on the client side.

#### 2.2 Public calls, without a graphics context

Verified sequence (bench: Java probe launched like the existing client extractor, same
classpath client jar + libraries, **no window nor GPU**, 2.7 s, 0 errors):

1. detection of the game version (public static function of the shared constants)
2. bootstrap of the server registries (public static function)
3. creation of the layer roots → table location → layer definition
4. for each entry: baking of the root of the layer definition → tree of model parts (public)

None of these calls opens a graphics context: the render classes loaded along the way
only make descriptors in their static initializers; GPU accesses are in instance
methods never called here.

What the public API gives / does not give:

| data | public access | otherwise |
|---|---|---|
| current pose of a part | public fields `x y z rot_x rot_y rot_z scale_x scale_y scale_z visible no_draw` | — |
| initial pose | public accessor of the initial pose | — |
| named children | existence test and access by name only (no enumeration); a walk gives the paths but skips empty parts | reflection on the private children field (actual iteration order) |
| baked cubes | public fields `min_x..max_z`, `polygons`; vertices and normals through accessors | the list is private → reflection, or walk |
| raw cube definition (texture_offset, dims, inflation, mirror, UV scale, faces) | no | reflection on the private fields of the definitions (mesh, material and texture sizes, pose, cube: origin, dimensions, inflation, mirror, texture scale, visible faces) |
| definition tree | public accessor of the root (called by the extractor), then the named sub-parts | — |

The 26.3 jar is not obfuscated: reflection by field name is stable for a given
version (to be re-verified at each version; failure ⇒ clear message, no default value).

#### 2.3 Proposed dump format (`entity_models.json`)

Choice: write the **definition** (compact, ~340 KB for the whole game) and let the client
generate the faces according to §1.4 (rule verified to the bit). Default values omitted.

```json
{
  "format": 1,
  "version": "26.3",
  "layers": {
    "minecraft:pig#main": {
      "texture": [64, 64],
      "root": {
        "pose": [0,0,0, 0,0,0, 1,1,1],
        "children": {
          "head": {
            "pose": [0,12,-6, 0,0,0, 1,1,1],
            "cubes": [
              {"uv":[0,0],  "from":[-4,-4,-8], "size":[8,8,8]},
              {"uv":[16,16],"from":[-2,0,-9],  "size":[4,3,1]}
            ]
          },
          "left_front_leg": {
            "pose": [3,18,-5, 0,0,0, 1,1,1],
            "cubes": [{"uv":[0,16],"from":[-2,0,-2],"size":[4,6,4],"mirror":true}]
          }
        }
      }
    }
  }
}
```

Schema:

| key | type | default | meaning |
|---|---|---|---|
| `layers` | object | — | key = a model layer location as text (`namespace:model#layer`) |
| `texture` | [i32, i32] | — | texture size of the layer (UV normalization) |
| `root` | part | — | root part |
| part.`pose` | [9 × f32] | — | x, y, z (px), rot_x, rot_y, rot_z (rad), scale_x, scale_y, scale_z |
| part.`cubes` | list | empty | definition order (= emission order) |
| part.`children` | ordered object | empty | **actual iteration order** of the baked model part (the reader must keep the key order) |
| cube.`uv` | [i32, i32] | — | truncated texture_offset |
| cube.`from` | [3 × f32] | — | minimal corner (px) |
| cube.`size` | [3 × f32] | — | w, h, d (px) |
| cube.`grow` | [3 × f32] | [0,0,0] | inflation |
| cube.`mirror` | bool | false | mirror |
| cube.`tex_scale` | [2 × f32] | [1,1] | texture size multiplier for this cube |
| cube.`faces` | list of direction names (`down up north south west east`) | all | visible faces |

Floats are written as integers when they are integers, otherwise with the shortest decimal
representation of Java (float-to-text conversion) that reads back exactly as f32.

Measurements (bench, 26.3): **361 layers, 3,380 parts, 2,516 cubes, 15,064 faces**, file
339 KB, 2.7 s (3.8 s process included). Texture sizes encountered: 64×64 (143), 64×32
(119), 32×32 (34), 128×128 (30), 128×64 (18), 16×16 (6), 64×128 (3), 32×16 (2), 256×256 (2),
48×48, 48×32, 192×192, 0×0 (water_patch).

#### 2.4 Existing probe

Functional prototype (outside the repository, to be integrated into `tools/extractor/` and `rc-data/prepare.rs`
like `ClientExtractor.java`):
`ModelDump.java` (temporary working folder, untracked)
(+ `run.py` which builds the classpath from `versions/26.3/26.3.json`). It also contains a
check that recomputes all the faces according to §1.4 and compares them with the baked faces.

**Outside the dump** (to be handled by the "entity → renderer association" mission): which layer and
which texture each entity type uses is decided in the constructor of each renderer
(the game's entity renderer classes, registered in the entity renderer registry);
constructing these renderers requires a context (texture managers, items…) — not attempted here
[not verified whether constructible without a GPU]. Entities without a cube model (dropped item,
falling block, displays, paintings, experience orb…) do not go through these layers.

---

#### 3. Rendering

#### 3.1 Entity vertex format

A vertex = position (3 × f32, **relative to the camera**, view rotation excluded), color
(RGBA 8-bit), UV0 (2 × f32, texture), UV1 (2 × integers: **overlay** coordinates), UV2
(2 × integers: **lightmap** coordinates), normal (3 components). Topology: quads.
The camera's view rotation is in the "ModelView" matrix (uniform); the entities' matrix
stack starts from identity, with the camera-relative translation (§4).
Color, overlay and lightmap are **the same for all the vertices of a model draw call**
(one value per entity and per layer).

#### 3.2 Entity render types (26.3)

Type names as in game. Depth: the game uses an **inverted** depth (comparison
"≥", clear to 0); "test ≥" therefore means "passes if at least as close". "Write"
= depth write. Culling = of back faces. "Translucent" blend =
color `src·αs + dst·(1−αs)`, alpha `αs·1 + αd·(1−αs)`. Cutout threshold = fragment rejection
if the **texture's alpha** (before any multiplication) < 0.1.

| type (factory) | blend | cull | test | write | cutout | lightmap | overlay | diffuse | notes |
|---|---|---|---|---|---|---|---|---|---|
| `entity_solid` | none | yes | ≥ | yes | no | yes | yes | normal | |
| `entity_solid_z_offset_forward` | none | yes | ≥ | yes | no | yes | yes | normal | layering offset "forward" (§3.8) |
| `entity_cutout_cull` | none | yes | ≥ | yes | 0.1 | yes | yes | normal | |
| `entity_cutout` (**formerly "cutout_no_cull"**, default of the models) | none | **no** | ≥ | yes | 0.1 | yes | yes | per face | |
| `entity_cutout_z_offset` | none | no | ≥ | yes | 0.1 | yes | yes | per face | layering offset "backward" |
| `entity_cutout_dissolve` | none | no | ≥ | yes | 0.1 | yes | yes | per face | 2nd "mask" texture: reject if vertex_alpha < mask_alpha, then vertex_alpha := 1 |
| `entity_translucent` | translucent | no | ≥ | **yes** | 0.1 | yes | yes | per face | quads sorted on submission |
| `entity_translucent_cull_item_target` | translucent | yes | ≥ | yes | 0.1 | yes | yes | normal | quads sorted; also used for the "ghost" body (§3.5) |
| `entity_translucent_emissive` | translucent | no | ≥ | **no** | 0.1 | **no** | yes | per face | |
| `eyes` (`eyes`) | translucent | yes | ≥ | no | **no** | **no** | **no** | **none** | color = texture × vertex color, then fog |
| `armor_cutout_no_cull` | none | no | ≥ | yes | 0.1 | yes | **no** | per face | armors |
| `energy_swirl` | **additive** (1, 1) | no | ≥ | yes | 0.1 | no | no | none | texture matrix (u,v scrolling) |
| `entity_shadow` | translucent | yes | ≥ | no | no | (declared, not read) | no | none | §3.7 |
| `outline` (`outline`) | — | — | — | — | — | — | — | — | "glow" outline in a separate target |

"Normal" = diffuse lighting with the normal alone (§3.3); "per face" = two computed colors,
one for the front face (normal) and one for the back face (opposite normal), chosen at the
fragment depending on the orientation — indispensable without culling (the inside of a cube seen from
the inside is lit like a flipped face).
Default of the models: `entity_cutout`; other models choose `entity_cutout_cull`,
`entity_solid`, `entity_translucent` or `entity_cutout_z_offset` (choice made per model
class, outside the dump of §2).

Choices observed (26.3): culled cutout for the bat, the bee stinger, the baby
turtle, the chest, the arrow, the fishing bobber and the flames of an entity on fire; opaque
(`entity_solid`) for the banner (pole and flag), the bell, the book, the shield, the
trident, the cape, the conduit shell and the decorated pot; the eyes (`eyes`) and lightning are
culled too. Ours: "culled cutout", "opaque" and "culled translucent" passes
of `entity_render.rs`, cube faces winding counter-clockwise seen from the outside (test
`cube_faces_wind_outward`).

Enchantment glint ("glint" variants): computed IN the geometry's own shader. After
overlay and lightmap, the square of (strength × glint texel) is added, the alpha is raised to the
strength, then fog applies to the whole. Glint UV = texture matrix
T(−o0, o1) · Rz(+10°) · S(s) applied to the geometry UVs, s = 8 for items (UVs of
the atlas), 0.5 for entities (trident, shield), 0.16 for armors (texture
`enchanted_glint_armor`, otherwise `enchanted_glint_item`); textures read linear repeat;
ms = integer part of (time in ms × speed × 8), o0 = (ms mod 110,000) / 110,000,
o1 = (ms mod 30,000) / 30,000. Armor: glint on the first drawn layer only.
Separate pass (patterned shield, trimmed armor): equal depth, glint texture
alone (rejection under 0.1), output × strength × (1 − fog), blend that adds the square.

#### 3.3 Diffuse lighting ("cardinal")

Two light directions in **world coordinates** (normalized):
- normal world (`cardinal_light` = `default` of the dimension type):
  `L0 = normalize(0.2, 1, −0.7)`, `L1 = normalize(−0.2, 1, 0.7)`;
- `nether` type: `L0 = normalize(0.2, 1, −0.7)`, `L1 = normalize(−0.2, −1, 0.7)`.

(Field `cardinal_light` of the dimension type, default `default`.) The vertex normals
are transformed by the normal matrix of the pose (view rotation excluded), therefore in the
same world frame.

Per vertex: `d0 = max(0, L0·N)`, `d1 = max(0, L1·N)`,
`k = min(1, (d0 + d1) × 0.6 + 0.4)`; diffuse color = `(rgb × k, a)` of the vertex color.
"Per face" mode: `k_front` with N, `k_back` with −N; the fragment takes `k_front` if
the face is seen from the front, otherwise `k_back`.

#### 3.4 Entity lightmap (one value per entity)

- Probed position: the **interpolated eye position**: `(lerp(pt, xo, x), lerp(pt, yo, y) +
  eyeHeight, lerp(pt, zo, z))`, rounded to the containing block (floor). Exception: a
  "marker" armor stand looks for the brightest block of its small box
  [detail not followed].
- `block = 15` if the entity is on fire, otherwise the world's block light at that position;
  `sky` = the world's raw sky light (without darkening: that one is in the lightmap).
- Block light overrides observed: always 15 → allay, blaze, dragon fireball, magma cube,
  shulker bullet, vex, wither, wither skull; experience orb → `min(15,
  block + 7)`; glow squid → `max(glow, block)` (15 if glow = 15); glow item frame →
  `max(5, block)`; thrown "full bright" item → 15; displays → values imposed by
  the entity [detail not followed].
- Packing: `uv2 = (block × 16, sky × 16)` (integer `block << 4 | sky << 20`).
  Usual "full bright" value: 15728880 = (240, 240).
- Sampling: texture coordinate `uv2 / 256 + 1/32`, clamped to [1/32, 31/32] on the
  16×16 lightmap (specified in `lighting.md`) → center of the texel (block, sky).
- The result multiplies the color (§3.6). No per-vertex light nor ambient occlusion for
  entities.

#### 3.5 Vertex color, invisibility

- Color passed to the model: opaque white (−1), multiplied by the renderer's own tint
  (e.g. sheep/slimes [see mission C]).
- Entity invisible but **visible to the player** (spectator, teammate who sees
  invisible ones): type forced to `entity_translucent_cull_item_target` and color
  `0x26FFFFFF` (alpha 38/255 ≈ 0.149).
- Invisible and not visible: no body; only the outline if the entity "glows"
  (glowing effect, color = opaque team color) via the `outline` type.
- Only the body (main model) disappears: the layers remain except those that copy the
  model (clothing layers, wool, collars, markings…) and the cape; armor, held items, eyes
  and energy swirls are drawn (invisible armor stand = floating armor).
  No shadow for an invisible entity. Non-living entities ignore the flag (the item
  frame has its own rule). The flames of an entity on fire remain visible.

#### 3.6 "Damage" overlay and "white flash" (overlay texture)

Generated 16×16 texture (no file):
- rows v = 0…7: red `(255, 0, 0)` with alpha 178 (0xB2);
- rows v = 8…15: white with alpha `trunc((1 − u/15 × 0.75) × 255)` (255 at u = 0, 63 at u = 15).

Coordinates (read as an exact texel, no filtering): `u = trunc(whiteProgress × 15)`,
`v = 3` if the entity has the red overlay (`hurt_time > 0` or `death_time > 0`), otherwise `v = 10`.
"No overlay" = (0, 10) (white with alpha 255 → no effect). Red wins over white
(same row). White progress: 0 by default; creeper: `s = swell amount`, `0` if
`trunc(s × 10)` is even, otherwise `clamp(s, 0.5, 1)`; TNT minecart: u = 15 during the
blinking.

Application at the fragment: `rgb = mix(overlay.rgb, rgb, overlay.a)` (so `rgb·a + overlay·(1−a)`),
before the lightmap.

#### 3.7 Fragment computation order (pipeline `core/entity`)

1. `c = texture(UV0)` (with a texture matrix if the type has one);
2. cutout: reject if `c.a < 0.1` (cutout types);
3. `c *= diffuseColor(§3.3, front/back face) × ColorModulator` (global modulator, white
   except for effects);
4. overlay (§3.6) if the type uses it;
5. `c *= lightmap(§3.4)` if the type uses it (alpha included: the lightmap has an alpha of 1);
6. fog (§3.10);
7. blend depending on the type.

#### 3.8 Ground shadow

Radius `r` = radius of the renderer (constant per entity type, × `scale` for living entities,
× 0.5 more for a baby mob), capped at 32; strength `f` of the renderer (1 by default). "Entity shadows" option off or entity
invisible → no shadow.
1. `pow = (1 − camDist² / 256) × f` (camDist² = squared distance camera→entity position, not
   interpolated); if `pow ≤ 0` → no shadow (beyond 16 blocks).
2. Depth `depth = min(pow / 0.5 − 1, r)`.
3. For every block (x, y, z) with `x ∈ [⌊ex − r⌋, ⌊ex + r⌋]`, `z ∈ [⌊ez − r⌋, ⌊ez + r⌋]`,
   `y ∈ [⌊ey − depth⌋, ⌊ey⌋]` (ex, ey, ez = interpolated position):
   - the block **below** (y−1) must not have an invisible render shape;
   - max raw light of the block (x,y,z) = `max(block, sky − skyDarkening)` must be > 3;
   - the block below must have a full-block **collision** shape, and a non-empty (outline)
     shape;
   - alpha = `clamp((pow − (ey − y) × 0.5) × 0.5 × brightness(n), 0, 1)` with
     `brightness(n) = lerp(ambient_light of the dimension type, v/(4 − 3v), 1)`, `v = n/15`.
4. Horizontal quad at height `y + min_y(shape below)` (i.e. y for a full block), covering
   the XZ footprint of the shape below (`x + min_x … x + max_x`, same for z), coordinates relative to
   the entity; UV: `u = −Δx / (2r) + 0.5`, `v = −Δz / (2r) + 0.5` (Δ = corner relative to the entity),
   vertices in the order (x0,z0), (x0,z1), (x1,z1), (x1,z0); white color with alpha = alpha
   above; normal (0,1,0); full light (not read).
5. Rendering: texture `textures/misc/shadow.png`, UVs clamped to [0,1] at the fragment,
   `color = texture × vertex color`, fog, translucent blend, test ≥ without write,
   layering offset "backward" (§3.9) to avoid z-fighting.

#### 3.9 Layering offset ("layering")

In perspective, the type multiplies the ModelView matrix by a uniform scale
`1 − b/4096` with `b = +1` ("backward", shadows, cutout_z_offset) or `b = −1` ("forward").
(In orthographic: z translation of `b/512`.)
`boat#water_patch`: layer without a texture drawn in the "water mask" type (writes only
the depth, to hide the water inside the hull) [pipeline detail not followed].

#### 3.10 Fog

Same fog as the terrain (`environment.md` §5–7): factor
`max(lin(spherical_dist, envStart, envEnd), lin(cylindrical_dist, renderStart, renderEnd))`,
`spherical_dist = |p|`, `cylindrical_dist = max(|p.xz|, |p.y|)`, p = position relative to the
camera; `rgb = mix(rgb, fogColor, factor × fogAlpha)`; alpha unchanged.
Applied to all entity types, shadow included (per-vertex computation of the distances, interpolated).

#### 3.11 Draw order in the frame ("classic" transparency, "improved
transparency" option off = default)

1. opaque + cutout terrain;
2. **solid phase** of the elements: all the entity models whose type has **no blending**
   (or is forced solid), flames, leashes, opaque items…;
3. **translucent phase** of the elements, in this order: shadows → name tags → texts →
   shape outlines → **translucent moving items and blocks** (translucent faces of items,
   carried, falling or displayed blocks with a translucent quad: the whole block is then blended)
   **sorted among themselves from farthest to nearest** (key = squared distance to the camera of the
   pose origin for an item, of the cube center (0.5; 0.5; 0.5) for a block; tie → submission
   order) → **translucent models** (types with blending: `entity_translucent*`, the ghost
   of an invisible entity `entity_translucent_cull`, `eyes`, emissives…) sorted the same way
   among themselves (model pose origin) → free translucent geometry → translucent markers →
   translucent cracks, water mask. A moving block without a translucent quad and the non-
   translucent faces of an item go in the **solid phase**;
4. **translucent terrain** (water, stained glass…) — therefore **after** the translucent entities;
5. "after the terrain" elements (translucent particles), clouds, weather, world border;
6. outline pass (glowing) in a separate target, then "seen through" and
   "always in front" elements.

Within a single buffer of a "sorted on submission" type, the quads are sorted by decreasing squared distance
to the camera (stable sort) [quad reference point: not verified, probably its center].
With "improved transparency" (OIT) all the translucent phases go through a separate
OIT pipeline (not specified here).

#### 3.12 Visibility (culling)

An entity is extracted for the frame if:
1. distance: `d² = |pos − cam|²` (non-interpolated position) < `(size × 64 × view_scale)²` where
   `size` = average of the three sides of its collision box (1 if NaN) and
   `view_scale = clamp(effectiveRenderDistance / 8, 1, 2.5) × entityDistanceScale` (option,
   default 1);
2. frustum (if the rendering is subject to it): culling box = **interpolated** collision box,
   **inflated by 0.5** on each side; NaN or zero-size box → cube of ±2 around the
   position; if out of view but attached by a leash, visible if the holder's box or
   the union of the two boxes is. Living entities wearing a dragon head inflate the
   box by 0.5 more. Renderers that change the box or disable the test: minecarts,
   displays, happy ghast, illusioner, sniffer, trident, dragon, fishing hook, lightning;
   OR the entity (indirectly) carries the player;
3. AND the 16³ section that contains its block is outside the world height, or is compiled with a
   visibility (fade-in) ≥ 0.3;
4. AND it is not the camera entity (except detached camera — 3rd person — or sleeping camera);
   the local player is only rendered if it is the camera entity.
First frame of an entity (age 0): the old position is first set equal to the position.

---

#### 4. Matrices: pose of an entity in the world

Conventions: the stack starts from identity; every operation **right-multiplies** (the last one
listed applies first to the vertices); angles in degrees unless stated; `pt` = tick
fraction ("game time" fraction; a frozen value is requested if the entity is frozen by the
tick rate manager — which one: [not verified]).

#### 4.1 Common to all entities

1. `T(ex − cam_x + off.x, ey − cam_y + off.y, ez − cam_z + off.z)` with
   `e = lerp(pt, old, current)` (interpolated position, in f64 then relative f32) and `off` =
   passenger offset (zero except for a passenger of a "new behaviour" minecart that interpolates
   its own position: off = interpolated position of the minecart − lerp position of the minecart).
2. The specific rendering draws (§4.2 for a living entity), then, in the same pose:
   leashes, name tags (after popping the body), flame if the entity is burning (oriented
   facing the camera around Y).
3. Shadow: submitted with the pose of point 1 (passenger offset included, except for
   avatars/players where the offset is removed beforehand).

#### 4.2 Generic living entity (exact order)

After 4.1:
1. push;
2. if it sleeps and has a bed orientation: `T(−dx × (eyeH − 0.1), 0, −dz × (eyeH − 0.1))`
   (dx, dz = step of the bed direction; eyeH = standing eye height);
3. `S(s, s, s)` with `s` = attribute `scale` of the entity (babies are not in it: they have
   distinct `*_baby` layers);
4. rotations (body angle `b`):
   - if frozen: `b += cos(⌊age⌋ × 3.25) × π × 0.4` (added as is to the degrees);
   - if not sleeping: `Ry(180 − b)`;
   - then, exclusive in this order:
     - death (`death_time > 0`, death_time = ticks + pt): `Rz(min(1, √((death_time − 1) / 20 × 1.6)) × 90)`
       (90 = "tip angle", overridable);
     - spin attack (riptide trident): `Rx(−90 − pitch)` then `Ry(age × −75)`;
     - sleeping: `Ry(a)` with a = 90 (bed toward south), 0 (west), 270 (north), 180 (east), otherwise b;
       then `Rz(90)`, then `Ry(270)`;
     - upside down (name "Dinnerbone" or "Grumm"): `T(0, (boxHeight + 0.1) / s, 0)` then
       `Rz(180)`;
5. `S(−1, −1, 1)` (switch to the model frame, Y down);
6. renderer's own scale (`scale(...)`, empty by default; e.g. giant, slimes…);
7. `T(0, −1.501, 0)`: with Y flipped, places the model origin 1.501 blocks above the feet,
   so that a model point at y = 24 px (1.5) ends up 0.001 above the ground;
8. model: the animation setup (reset to the initial pose then animation) at draw time,
   then recursive drawing of the root (§1.2) with the render type, the light (§3.4), the overlay
   (§3.6) and the color (§3.5);
9. additional layers (armor, held items, eyes, saddle…): the animation setup redone, each
   drawn in the same pose (mission C);
10. pop; then leashes and name tags (4.1).

State angles (extracted per frame):
- `b` (body) = `lerpAngle(pt, prev_body_yaw, body_yaw)`; if the entity rides a living entity:
  `b0 = lerpAngle` of the mount's body, `δ = clamp(wrap(head − b0), −85, 85)`, `b = head − δ`,
  and if `|δ| > 50`, `b += 0.2 δ`;
- head = `lerpAngle(pt, prev_head_yaw, head_yaw)`; `relative_yaw` (relative, passed to the model) =
  `wrap(head − b)`; `render_pitch` = interpolated pitch; both are negated if "upside down".
- `lerpAngle(t, a, b) = a + t × wrapDegrees(b − a)`.

#### 4.3 Matrix recap (standing living entity, no special case)

`M = T(e − cam) · S(s) · Ry(180 − b) · S(−1, −1, 1) · S_render · T(0, −1.501, 0) · M_root · M_part… · (vertex/16)`

and the final position on screen = `Proj · ViewRot · M · vertex`.

---

### Uncertain points

1. **Order of children**: it is the iteration order of a Java hash table; the dump
   writes it as is (do not recompute it). Only affects the translucent types.
2. **Constructibility of the renderers without a GPU** (to also dump the association entity →
   layers, textures, render type, shadow radius): not attempted; otherwise hand-written table
   (mission C).
3. **Sorting of quads on submission** (`entity_translucent*`, `eyes`): key = decreasing squared distance,
   stable sort; the reference point (center of the quad?) was not followed.
4. **Lightmap alpha** assumed 1 (the multiplication of step 5 of §3.7 includes the alpha).
5. **Tick fraction of frozen entities** (§4) and light probe of the "marker" armor stand
   (§3.4): not followed.
6. **OIT pipeline** ("improved transparency" option) and "glowing" outline pass:
   only flagged.
7. The `entity_shadow` type declares the lightmap and the overlay but its shader does not read them:
   the shadow is therefore not darkened by light on display, only by its computed
   alpha (§3.8) — read in the shader, to be confirmed by eye.
8. The dimension type's `ambient_light` **is used** for the shadow alpha (§3.8, via the curve
   `lerp(ambient, v/(4−3v), 1)`): `lighting.md` says it is not used outside vignette/AI, to be completed.
9. Inverted depth: deduced from the default "≥" comparison and the clear to 0;
   a client with classic depth must use "≤".


## Entities — mission C: per-entity renders and animations (Minecraft Java 26.3, protocol 777)

Clean-room specification, written from a reading of the decompiled 26.3 client. No line of
source code; formulas, constants and resource names only. Angles of model parts in radians,
entity angles in degrees unless stated.

Model conventions (reminder, valid everywhere):
- A "part" has a pose (x, y, z in pixels of 1/16 block, rot_x, rot_y, rot_z in radians, scales
  scale_x/scale_y/scale_z). Application: translation (x/16, y/16, z/16), then quaternion rotation
  "ZYX" (first Z, then Y, then X on the local frame: matrix = Rz·Ry·Rx), then scale. Children
  inherit the parent's transformation.
- Before each animation setup, all parts take back their definition pose ("reset"). The formulas
  below therefore write either an absolute value (=), or an increment (+=) on the base pose.
- Model frame: Y down (the definitions are "upside down"), model origin 24 px above
  the feet; the engine corrects it with the scale (-1, -1, 1) and the translation of -1.501 block (§1.2).

---

#### 1. Generic rendering of a living entity

#### 1.1 State data extracted every frame (pt = partial tick fraction, 0..1)

| Quantity | Computation |
|---|---|
| position x, y, z | lerp(pt, old position of the previous tick, current position) |
| `age_ticks` | ticks_lived + pt (ticks_lived = number of client ticks lived by the entity) |
| `head_yaw_render` (head yaw, degrees) | lerpAngle(pt, prev_head_yaw, head_yaw) — lerpAngle = interpolation by the shortest path (difference brought into [-180, 180)) |
| `body_yaw_render` (body yaw) | without a living mount: lerpAngle(pt, prev_body_yaw, body_yaw). If the entity rides a living entity: b = lerpAngle(pt, body yaw of the mount); d = clamp(wrap(head_yaw_render − b), −85, 85); body_yaw_render = head_yaw_render − d; if \|d\| > 50, body_yaw_render += d × 0.2 |
| `relative_yaw` (relative head yaw) | the wrapped angle(head_yaw_render − body_yaw_render) |
| `pitch_render` (pitch) | lerp(pt, prev_pitch, pitch) |
| `upside_down` | custom name exactly "Dinnerbone" or "Grumm" (for the player: profile name, and only if the "cape" part is shown); then pitch_render and relative_yaw are negated |
| `walk_position` / `walk_speed` | §1.4; forced to 0 if the entity is a passenger or dead |
| `scale` | value of the attribute `minecraft:scale` (default 1, clamped [0.0625; 16]), received by the attribute update |
| `age_scale` | 0.5 if baby, otherwise 1 |
| `death_time` | if the integer death_time > 0: death_time + pt, otherwise 0 |
| `red_overlay` | hurt_time > 0 or death_time > 0 |
| invisibility test | entity flag "invisible" (bit 0x20 of byte 0 of the metadata) |
| `invisible_to_player` | invisible AND the local player does not "see" the entity (not a spectator; not a teammate in a team with "see invisible allies") |
| full freeze test | frozen ticks ≥ threshold (entity in powder snow) |
| light | sky/block levels at the block containing the "light probe" (approximate center of the entity); block = 15 if the entity is burning |

Per-tick update client side (remote entities included):
- Head: a "head rotation" packet triggers an interpolation over 3 ticks: at each tick,
  head_yaw = lerpAngle(1 / remaining_steps, head_yaw, target), then remaining_steps −= 1.
- Body (`body_yaw`) — recomputed locally, never transmitted directly for an ordinary mob:
  1. dx, dz = horizontal movement since the previous tick; if dx² + dz² > 0.0025000002:
     dir = atan2(dz, dx) in degrees − 90; if the difference \|wrap(yaw) − dir\| is in ]95; 265[
     (walking backward) target = dir − 180, otherwise target = dir. Otherwise target = current body_yaw.
  2. If a hit is in progress (swing animation > 0): target = yaw (the entity's yaw).
  3. (Non-mob living entities: players, armor stands…) body_yaw += wrap(target − body_yaw) × 0.3; then
     if \|wrap(yaw − body_yaw)\| > M (M = 50; player blocking with a shield: 15), the body is pushed
     so that the difference is exactly ±M.
  4. Mobs replace steps 1‑3 with the "body rotation controller" (client):
     - notation: constrain(base, target, m) = target − clamp(angularDifference(base → target), −m, m);
       head_m = 75° (default value for mobs);
     - moving (dx² + dz² > 2.5e‑7): body_yaw = yaw (the entity's yaw); head_yaw =
       constrain(head_yaw, body_yaw, 75); remember stable head = head_yaw; counter = 0;
     - still and without a passenger mob: if \|head_yaw − stable head\| > 15: counter = 0, stable head =
       head_yaw, body_yaw = constrain(body_yaw, head_yaw, 75); otherwise counter += 1 and, if it exceeds
       10: f = clamp((counter − 10) / 10, 0, 1); body_yaw = constrain(body_yaw, head_yaw,
       75 × (1 − f)) — the body catches up with the head in 10 ticks.
  5. The "O" (previous) values are brought to within 180° of the current value, so that the
     interpolations do not do a full turn.
- Position: interpolation of the movement packets (no "stepped" interpolation for
  living entities, linear for experience orbs) — out of scope for this mission (see network mission).

#### 1.2 Transformation stack (exact order)

Starting from the entity's world origin minus the camera:
1. Translation by the render offset (0 in general; sneaking player: (0, −2/16 × scale, 0);
   passenger of a "new behaviour" minecart: smoothing difference of the minecart).
2. (Sleeping body) if the pose is `sleeping` and the bed orientation is known: translation of
   (−dx × h, 0, −dz × h) where (dx, dz) = bed direction and h = standing eye height − 0.1.
3. Uniform scale `scale`.
4. The rotations setup (§1.3).
5. Scale (−1, −1, 1).
6. Renderer's own scale (player: 0.9375; see each entity).
7. Translation (0, −1.501, 0).
8. Drawing of the model, then of the layers ("layers") with the same stack. Every submitted model (body,
   armor, wool, eyes, charge…) runs its own animation setup on the SAME state: the layer models
   therefore reproduce exactly the pose of the body if they have the same parts.
9. Outside the stack: name tag (§1.8), flames if the entity is burning, shadow.

#### 1.3 Setting up the rotations

Input: body_yaw_render (degrees).
- Shaking (completely frozen entity): body_yaw_render += cos(floor(age_ticks) × 3.25) × π × 0.4
  (addition in DEGREES of a quantity of ±1.26: small shiver).
- If the pose is not `sleeping`: Y rotation of (180 − body_yaw_render) degrees.
- Then, by priority order (a single case):
  1. **Death** (death_time > 0): f = sqrt((death_time − 1) / 20 × 1.6), capped at 1; Z rotation of
     f × tipAngle (90° by default; spider 180°). The entity therefore tips over in ≈ 13.5 ticks and stays
     lying; the server removes it at tick 20 (entity event 60 → smoke cloud).
  2. **Spin attack** (Riptide trident, living flag 0x04): X rotation of (−90 − pitch), then
     Y rotation of (age_ticks × −75) degrees.
  3. **Sleeping**: Y rotation by an angle depending on the bed orientation (south 90, west 0, north 270,
     east 180; unknown → body_yaw_render), then Z rotation of tipAngle, then Y rotation of 270.
  4. **Upside down**: translation (0, (box height + 0.1) / scale, 0) then Z rotation of 180.
- **Player (and avatars)** in addition, after the generic case:
  - gliding (elytra): s = clamp(t² / 100, 0, 1) with t = flight ticks + pt; except spin attack,
    X rotation of s × (−90 − pitch); then if the horizontal velocity and the horizontal gaze
    are non-zero (square > 1e‑5): Y rotation (radians) of sign(mx·lz − mz·lx) × acos(min(1, \|m̂·l̂\|))
    where m = smoothed movement (lerp between the movement of the previous tick and the current one), l = gaze vector,
    both projected horizontally and normalized;
  - otherwise if swim_amount > 0: X rotation of lerp(swim_amount, 0, target) with target = −90 − pitch if
    in water, −90 otherwise; if the pose is "visual swimming", translation (0, −1, 0.3).
  - swim_amount: +0.09 per tick in visual swimming, −0.09 otherwise, clamped [0, 1], interpolated by pt.

#### 1.4 Limb swing

State per entity: prev_speed, speed, position, position_scale (1).
- Each client tick (in the "AI step", remote entities included):
  - distance = length of the movement since the previous tick (horizontal only, except for
    "omnidirectional" flying entities where the Y component counts);
  - if the entity is a passenger or dead: prev_speed = speed = position = 0;
  - otherwise: target = min(distance × 4, 1); prev_speed = speed; speed += (target − speed) × 0.4;
    position += speed; position_scale = 3 if baby, 1 otherwise.
- Hit received (damage event): speed = 1.5 (limb jolt).
- Render values:
  - `walk_speed` = min(lerp(pt, prev_speed, speed), 1)
  - `walk_position` = (position − speed × (1 − pt)) × position_scale
- Models then use cos(pos × 0.6662) × amplitude × speed (§2).

#### 1.5 Scale, baby

- Global scale = attribute `scale` (step 3 of §1.2); the shadow radius is multiplied by `scale`
  (and by `age_scale` for mobs).
- Babies: in 26.3, most mobs have a **distinct baby model** (separate model layer, own
  geometry) selected when the baby flag is true; it is no longer a simple reduction.
  For humanoids (zombie…) the baby model is the adult model transformed by the "baby
  humanoid" rule: head: translation (0, 16, 0) then scale 1.5/2 = 0.75; other parts: translation
  (0, 24, 0) then scale 1/2 (positions and scales of the parts multiplied). See §2 per entity.
- `walk_position` is tripled for babies (position_scale = 3): faster steps.

#### 1.6 "Hurt" state (overlay)

- hurt_time = 10 on receiving a damage event ("damage event" or "hurt animation" packet),
  decremented by 1 per tick. Red tint while hurt_time > 0 OR death_time > 0.
- 16×16 overlay texture (sampled per vertex): rows 0‑7 = pure red (A = 0xB3 ≈ 0.70);
  rows 8‑15 = white with alpha (1 − u/15 × 0.75). Coordinates: u = floor(whiteProgress × 15),
  v = 3 if red, 10 otherwise.
- Shader: color.rgb = mix(overlay.rgb, color.rgb, overlay.a), i.e. 30% pure red mixed in.
- White progress: 0 by default; creeper: see §2.6.

#### 1.7 Invisibility

- Visible body = not invisible.
- If invisible but NOT "invisible to the player" (spectator, or teammate who sees invisible ones):
  translucent render ("entity_translucent_cull" type) with multiplicative color 0x26FFFFFF
  (alpha 38/255 ≈ 0.15).
- If invisible to the player: nothing is drawn for the model, unless the entity "glows" (Glowing
  effect): outline only. The layers (armor, held item…) are still drawn normally
  (each layer decides; armor is displayed on an invisible mob).
- Normal render type: "entity_cutout" (alpha cutout, no sorting) for most models.

#### 1.8 Name tag

Decision (per frame):
- Squared camera–entity distance < (attribute `name_tag_distance`)², default 64 (bounds [0; 512]); for
  non-living entities: fixed 64.
- If the entity is crouching ("discreet" = sneaking): never beyond 32 blocks (distance² ≥ 1024).
- Team rules (if the entity has a team, and is not the local player): name visibility
  `always` → visible if not invisible to the player; `never` → never; `hideForOtherTeams` → if the
  local player has no team: visible if not invisible, otherwise only if same team (ally) and
  (sees invisible allies OR not invisible); `hideForOwnTeam` → if no local team: visible if
  not invisible, otherwise only if different team and not invisible.
- Without a team: interface not hidden (F1), entity ≠ camera entity, not invisible to the player, and
  without a passenger.
- Mobs (and the player, see below): additionally "name always visible" (metadata
  `custom_name_visible`) OR (custom name present AND entity targeted by the crosshair).
  Players: "show name" is always true → a player's name is shown as soon as the previous rules pass.
- "Below the name" line (score of the display objective `below_name`): if distance² <
  (attribute `below_name_distance`)², default 10.

Text: display name (for a player: team prefix/suffix + name, team color; for a mob:
custom name, otherwise translated name of the type).

Position and drawing:
- Attachment point "name_tag" of the entity; failing that (general case): (0, box height, 0) in the
  entity's world frame (not rotated, not scaled by §1.2). Translation (ax, ay + 0.5, az).
- Rotation = camera orientation (billboard facing the camera), then scale (0.025; −0.025; 0.025):
  1 font pixel = 0.025 block.
- Centered text: x = −width/2; y = offset (0; −10 for the player "deadmau5" with ears).
- Background: black with alpha = text background opacity of the options (if the option "background for chat
  only" is active, which is the default: 0.25). Text alpha = max((bgAlpha + 0.75) / 2, 0.5).
- If the entity is NOT crouching: two passes —
  1. "see-through" pass (no depth test): text with the alpha above + background;
  2. normal pass: opaque white text, no background, light raised to at least 2 in sky and block.
  If crouching: a single normal pass with background and semi-transparent text (not visible through walls).
- Line below the name: drawn first at the same point, then the name is shifted upward by
  9 × 1.15 × 0.025 = 0.25875 block.

#### 1.9 Shadow (brief reminder)

Radius (per renderer, × scale, × age_scale for mobs, capped at 32); strength = (1 − dist²/256) ×
intensity; no shadow if invisible or option disabled. Detail of the shadow tiles: out of scope.

#### 1.10 Attack swing ("attack_time", 26.3)

The "swing" is no longer a simple 6-tick counter: each swing has a **description**
(hand, type, duration).
- Packet `minecraft:swing_animation` (play, client-bound, id 123): `varint` entity id, `varint` hand
  (0 = main, 1 = offhand), `varint` type (0 `none`, 1 `whack`, 2 `stab`; out of bounds → 0),
  `varint` duration in ticks. The client starts the swing on the living entity (without re-emission).
  Default for an item without the component: `whack`, 6 ticks. (The old `animate` packet keeps the actions
  0 = wake up, 1 = critical hit, 2 = magic critical hit.)
- Start: refused if a swing has been in progress for more than 0 and at most duration/2 ticks; otherwise
  ticks = 0; if the description changes, animation and old animation restart from 0.
  Effective duration: duration − (1 + Haste amplifier) if the entity has Haste (Haste/Conduit);
  otherwise duration + 2 × (1 + amplifier) with Mining Fatigue; otherwise duration.
- Tick (at the start of the entity's base tick): old = animation; if in progress: animation =
  min(ticks / duration, 1) (if duration > 0); then ticks += 1 and, if the old ticks exceeded the duration, end
  (description cleared, animation = 0). Without a swing: animation = 0.
- Render value `swing_progress` = lerp(pt, old, animation) if animation ≥ old, otherwise
  lerp(pt, old, animation + 1) (passing through 1 at the wrap-around).
- The animated side is the swing's hand converted to an arm (main → the entity's main arm:
  player → metadata "main arm" (0 left, 1 right); mob → left-handed if bit
  0x02 of the mob flags byte is set, right-handed otherwise).

<!-- end §1 -->

---

#### 2. Animations of the common models

Notation: P = `walk_position`, V = `walk_speed`, A = `age_ticks`, yR = `relative_yaw` × π/180
(relative head), xR = `pitch_render` × π/180, s = `swing_progress`. Geometry of the parts: see the
"models" mission; only the useful pivots are recalled.

#### 2.1 Base humanoid (zombie, skeleton, player, armors)

Parts: `head` (child `hat`), `body`, `right_arm`, `left_arm`, `right_leg`, `left_leg`.
Adult pivots: head and body (0, 0, 0); right arm (−5, 2, 0), left arm (5, 2, 0);
right leg (−1.9, 12, 0), left leg (1.9, 12, 0) (skeleton: arms ±5, legs ±2).
Humanoid texture 64×64 (skeleton 64×32).

The animation setup, in this order:
1. Head: rot_x = xR; rot_y = yR; when gliding (elytra) rot_x = −π/4.
2. Walking (sv = "speed value": 1 in general; when gliding sv = max(1, (\|v\|²/0.2)³) where v =
   velocity of the entity in blocks/tick):
   - right arm rot_x = cos(P × 0.6662 + π) × 2 × V × 0.5 / sv
   - left arm rot_x = cos(P × 0.6662) × 2 × V × 0.5 / sv
   - right leg rot_x = cos(P × 0.6662) × 1.4 × V / sv
   - left leg rot_x = cos(P × 0.6662 + π) × 1.4 × V / sv
   - legs: rot_y = ±0.005 and rot_z = ±0.005 (right +, left −) — slight "waddle".
3. Passenger (seated): arms rot_x += −π/5; right leg (rot_x −1.4137167, rot_y π/10, rot_z 0.07853982);
   left leg (rot_x −1.4137167, rot_y −π/10, rot_z −0.07853982).
4. Arm poses (§2.1.1), applied first to the "priority" arm: if the entity is using an item,
   the arm of the hand used; otherwise the main arm if the offhand arm's pose is "two-handed",
   otherwise the offhand arm. The other arm is only posed if the first one's pose does not "affect
   the offhand".
5. Attack (§2.1.2).
6. Crouching (pose `crouching`): body rot_x = 0.5; arms rot_x += 0.4; legs z += 4; head y += 4.2;
   body y += 3.2; arms y += 3.2. (The crouching player is additionally lowered by 2/16 × scale, §1.2.)
7. Idle sway (except arms in spyglass pose): right arm with factor +1, left arm −1:
   rot_z += f × (cos(A × 0.09) × 0.05 + 0.05); rot_x += f × sin(A × 0.067) × 0.05.
8. Swimming (if swim_amount w > 0):
   - head rot_x = lerpAngleRad(w, rot_x, −π/4) except gliding;
   - p = P mod 26; excluded arms (arm_w = 0) if they hold a spear (pose `SPEAR`) or if they
     swing; if the entity is not using an item:
     - q(x) = −65x + x²;
     - p < 14: arm rot_x → 0, rot_y → π, rot_z left → π + 1.8707964 × q(p)/q(14), right →
       π − 1.8707964 × q(p)/q(14);
     - 14 ≤ p < 22: t = (p − 14)/8; rot_x → π/2 × t; rot_y → π; rot_z left → 5.012389 −
       1.8707964 t, right → 1.2707963 + 1.8707964 t;
     - 22 ≤ p < 26: t = (p − 22)/4; rot_x → π/2 − π/2 × t; rot_y → π; rot_z → π;
     ("→" = blend with factor arm_w: shortest-path angular interpolation for the left arm,
     linear for the right);
   - legs: left rot_x = lerp(w, rot_x, 0.3 cos(P/3 + π)), right = lerp(w, rot_x, 0.3 cos(P/3)).

#### 2.1.1 Arm poses (right arm; left = mirror of the signs of rot_y)

| Pose | Effect |
|---|---|
| `empty` | rot_y = 0 |
| `item` | rot_x = rot_x × 0.5 − π/10; rot_y = 0 |
| `block` (shield) | rot_x = rot_x × 0.5 − 0.9424779 + clamp(head rot_x, −4π/9, 0.43633232); rot_y = ∓30° (right −, left +) + clamp(head rot_y, −π/6, π/6) |
| `bow` (2 hands) | bow in the right hand: right rot_y = −0.1 + head rot_y; left rot_y = 0.1 + head rot_y + 0.4; both rot_x = −π/2 + head rot_x. Bow in the left: right rot_y = −0.1 + head rot_y − 0.4; left rot_y = 0.1 + head rot_y |
| `throw_trident` | rot_x = rot_x × 0.5 − π; rot_y = 0 |
| `crossbow_charge` (2 hands) | holding arm: rot_y = ∓0.8, rot_x = −0.97079635; pulling arm: t = clamp(use ticks, 0, charge duration)/duration; rot_y = lerp(t, 0.4, 0.85) × (±1); rot_x = lerp(t, −0.97079635, −π/2) |
| `crossbow_hold` (2 hands) | holding arm: rot_y = ∓0.3 + head rot_y, rot_x = −π/2 + head rot_x + 0.1; other arm: rot_y = ±0.6 + head rot_y, rot_x = −1.5 + head rot_x |
| `spyglass` | rot_x = clamp(head rot_x − 1.9198622 − (crouching ? π/12 : 0), −2.4, 3.3); rot_y = head rot_y ∓ π/12 |
| `horn` | rot_x = clamp(head rot_x, −1.2, 1.2) − 1.4835298; rot_y = head rot_y ∓ π/6 |
| `brush` | rot_x = rot_x × 0.5 − π/5; rot_y = 0 |
| `spear` | spear animation (out of scope of this batch) [not detailed] |

"Two-handed": bow, crossbow (charge and hold). "Affects the offhand": bow, trident,
crossbow (charge and hold).

Pose choice — player (for each hand; empty hand → `empty`):
- crossbow charged and no swing in progress → `crossbow_hold`;
- if this hand is in use (remaining use ticks > 0): depending on the item's use animation:
  block → `block`, bow → `bow`, trident → `throw_trident`, crossbow → `crossbow_charge`,
  spyglass → `spyglass`, toot_horn → `horn`, brush → `brush`, spear → `spear`;
- otherwise `spear` if the item is a spear (tag `spears`) or if a `stab` swing of this arm is
  in progress, otherwise `item`.
- If the main hand's pose is two-handed, the offhand becomes `item` (or `empty` if empty).
Generic humanoid mobs: `spear` (same conditions) otherwise `empty` — a zombie holding a sword therefore has
its arms in `EMPTY` pose (the item follows the arm).

#### 2.1.2 Generic attack (`whack`)

If s > 0 and a swing is described:
- body rot_y = sin(sqrt(s) × 2π) × 0.2, negated if the hitting arm is the left;
- arm positions (overwrite the pose): right z = sin(body rot_y) × 5 × age_scale,
  x = −cos(body rot_y) × 5 × age_scale; left z = −sin(body rot_y) × 5 × age_scale,
  x = cos(body rot_y) × 5 × age_scale;
- right and left arms rot_y += body rot_y; left arm rot_x += body rot_y;
- type `whack`: a = sin(easeOutQuart(s) × π) with easeOutQuart(x) = 1 − (1 − x)⁴;
  b = sin(s × π) × −(head rot_x − 0.7) × 0.75; hitting arm: rot_x −= a × 1.2 + b;
  rot_y += body rot_y × 2; rot_z += sin(s × π) × −0.4.
- type `none`: only the body and arm rotation above; `stab`: spear animation
  [not detailed].

#### 2.2 Zombie

- Renderer: humanoid model, shadow radius 0.5; layers: armor ("zombie" inner/outer models), worn
  head (skull/block), wings (elytra), held item.
- Textures: adult `textures/entity/zombie/zombie.png` (64×64); baby `textures/entity/zombie/zombie_baby.png`.
- Baby: **dedicated model** (`zombie_baby`): body 4×5×2 at pivot (0, 17.5, 0); head 6×6×6 at pivot
  (0, 15.25, 0) with a second box inflated by 0.25 (built-in hat layer); arms 2×5×2 at pivots
  (±3, 15.5, 0); legs 2×4×2 at pivots (±1, 20, 0); texture 64×64. Same animation formulas.
- Arms (after the generic attack, overwrites the arms) — unless an arm is in `SPEAR` pose:
  - raise = (adult) OR (baby with empty main hand);
  - droop = raise ? −π / (aggressive ? 1.5 : 2.25) : 0;
  - ay = (raise ? 1 : −1) × sin(s × π); ax = sin((1 − (1 − s)²) × π);
  - rot_x = droop + ay × 1.2 − ax × 0.4; rot_y = 0.1 − ay × 0.6;
  - right arm (except trident pose): rot_x, rot_y = raise ? −rot_y : rot_y, rot_z = 0;
  - left arm (same): rot_x, rot_y = raise ? rot_y : −rot_y, rot_z = 0.
  Then come crouching and idle sway (§2.1 steps 6‑7).
- "Aggressive" = bit 0x04 of the mob flags byte (mob metadata).
- Shaking (§1.3) also during conversion to drowned (metadata "underwater conversion").

#### 2.3 Skeleton

- Texture `textures/entity/skeleton/skeleton.png` (64×32); shadow 0.5; same set of layers as the zombie
  (armor model "skeleton").
- Thin limbs: arms 2×12×2 at pivots (±5, 2, 0), legs 2×12×2 at pivots (±2, 12, 0).
- Pose: main arm in `bow` if the skeleton is aggressive AND holds a bow in its main
  hand; otherwise generic pose (`EMPTY`, or `SPEAR`).
- After the full humanoid animation, if aggressive and NOT holding a bow (sword…):
  s2 = sin(s × π); s1 = sin((1 − (1 − s)²) × π); arms rot_z = 0; right arm rot_y = −(0.1 − s2 × 0.6);
  left rot_y = 0.1 − s2 × 0.6; arms rot_x = −π/2 − (s2 × 1.2 − s1 × 0.4); then idle sway
  (§2.1 step 7) applied a second time.
- Held item: before placing the item, the arm is temporarily shifted by x = +1 (right) or
  −1 (left) pixel (thin arms).
- Shaking: during conversion to stray (frozen skeleton).

<!-- end §2.3 -->

#### 2.4 Generic quadruped (cow, pig, sheep)

Parts: `head`, `body` (base pose rot_x = π/2: the slab is modeled standing then laid down),
`right_hind_leg`, `left_hind_leg`, `right_front_leg`, `left_front_leg`.
- head: rot_x = xR, rot_y = yR;
- right hind and left front leg: rot_x = cos(P × 0.6662) × 1.4 × V;
- left hind and right front leg: rot_x = cos(P × 0.6662 + π) × 1.4 × V (diagonal gait).
The baby models (dedicated geometry, see models mission) reuse these same formulas on their
parts of the same names.

Variants (cow, pig, chicken): dynamic registries synchronized at connection
(`minecraft:cow_variant`, `minecraft:pig_variant`, `minecraft:chicken_variant`), referenced by the
entity's variant metadata (network id in the registry). Each entry carries:
`asset_id` (adult texture), `baby_asset_id` (baby texture), `model` (model type, default `normal`).
Texture path: `minecraft:entity/cow/cow_cold` → `assets/minecraft/textures/entity/cow/cow_cold.png`
(prefix `textures/`, suffix `.png`). Variant absent (not yet received) → the entity is not drawn.

| Vanilla entry | Adult / baby texture | Model |
|---|---|---|
| cow `temperate` | `entity/cow/cow_temperate` / `cow_temperate_baby` | `normal` |
| cow `warm` | `entity/cow/cow_warm` / `cow_warm_baby` | `warm` |
| cow `cold` | `entity/cow/cow_cold` / `cow_cold_baby` | `cold` |
| pig `temperate` | `entity/pig/pig_temperate` / `pig_temperate_baby` | `normal` |
| pig `warm` | `entity/pig/pig_warm` / `pig_warm_baby` | `normal` |
| pig `cold` | `entity/pig/pig_cold` / `pig_cold_baby` | `cold` |
| chicken `temperate` | `entity/chicken/chicken_temperate` / `_baby` | `normal` |
| chicken `warm` | `entity/chicken/chicken_warm` / `_baby` | `normal` |
| chicken `cold` | `entity/chicken/chicken_cold` / `_baby` | `cold` |

Model layers selected (adult / baby): cow `cow`/`cow_baby`, `warm_cow`/`warm_cow_baby`,
`cold_cow`/`cold_cow_baby`; pig `pig`/`pig_baby`, `cold_pig`/`pig_baby`; chicken `chicken`/
`chicken_baby`, `cold_chicken`/`chicken_baby`.

#### Cow
- Shadow radius 0.7. Head at pivot (0, 4, −8) with horns; legs 4×12×4 at pivots (±4, 12, 7) and
  (±4, 12, −5). 64×64 texture. No animation of its own beyond the quadruped.
- "Mooshroom" variant: out of batch (textures `entity/cow/mooshroom_red|brown[_baby]`).

#### Pig
- Shadow radius 0.7. Quadruped with legs of 6: head at pivot (0, 12, −6), body (0, 11, 2), legs
  (±3, 18, 7) and (±3, 18, −5). 64×64 texture (baby 32×32).
- "Saddle" layer: equipment of the `saddle` slot rendered with the equipment layer
  `pig_saddle` (§3.5) on the `pig_saddle` model (same inflated geometry).

#### Sheep
- Shadow radius 0.7. Quadruped with legs of 12 (legs mirrored on the right); head 6×6×8 at pivot
  (0, 6, −8); body 8×16×6; texture `textures/entity/sheep/sheep.png` (64×32), baby
  `textures/entity/sheep/sheep_baby.png` (model `sheep_baby`).
- Grazing: entity event 10 sets a counter to 40, decremented by 1 per client tick (min 0). With
  c = counter, pt:
  - head offset ph = 0 if c ≤ 0; 1 if 4 ≤ c ≤ 36; (c − pt)/4 if c < 4; −(c − 40 − pt)/4 if c > 36;
  - head angle ah = π/5 + 0.21991149 × sin((c − 4 − pt)/32 × 28.7) if 4 < c ≤ 36; π/5 if c > 0
    (other cases); otherwise xR (normal pitch).
  - After the quadruped: head y += ph × 9 × age_scale; head rot_x = ah (replaces the pitch).
- Wool metadata (byte): color = bits 0‑3 (dye index 0..15), sheared = bit 0x10.
- "Undercoat" layer (adult only, not invisible, if color ≠ white OR "jeb_" sheep):
  model `sheep_wool_undercoat` (same geometry as the sheep body), texture
  `textures/entity/sheep/sheep_wool_undercoat.png`, tint = wool color.
- "Wool" layer (if not sheared): model `sheep_wool` (baby: `sheep_baby_wool`), texture
  `textures/entity/sheep/sheep_wool.png` (baby `sheep_wool_baby.png`), alpha cutout, tint =
  wool color; if the sheep is invisible: outline only if it glows, nothing otherwise. Same animation
  formulas (including grazing) as the body.
- Wool color (opaque ARGB): for each dye, diffuse color × 0.75 per component (default rounding,
  clamped 0..255); exception white → 0xE6E6E6. Diffuse colors (index: name = RGB):
  0 white F9FFFE, 1 orange F9801D, 2 magenta C74EBD, 3 light_blue 3AB3DA, 4 yellow FED83D, 5 lime
  80C71F, 6 pink F38BAA, 7 gray 474F52, 8 light_gray 9D9D97, 9 cyan 169C9C, 10 purple 8932B8, 11 blue
  3C44AA, 12 brown 835432, 13 green 5E7C16, 14 red B02E26, 15 black 1D1D21.
- "jeb_" (exact custom name): n = floor(A); i = n / 25 (integer division); colors c1 =
  dye (i mod 16), c2 = dye ((i + 1) mod 16) (index order); t = ((n mod 25) +
  frac(A)) / 25; each component = c1 + floor(t × (c2 − c1)).

#### 2.5 Chicken

- Shadow radius 0.3. Parts: `head` (children `beak`, `red_thing`), `body`, `right_leg`, `left_leg`,
  `right_wing`, `left_wing`; texture 64×32; head at pivot (0, 15, −4), legs at pivots
  (−2, 19, 1) and (1, 19, 1).
- Flapping (per client tick): prev_flap = flap; prev_flap_speed = flap_speed; flap_speed += (on ground ? −1 : 4)
  × 0.3, clamped [0, 1]; if in the air and flapping < 1: flapping = 1; flapping *= 0.9;
  flap += flapping × 2. ("on ground" = flag of the movement packets.)
- Render: f = lerp(pt, prev_flap, flap), fs = lerp(pt, prev_flap_speed, flap_speed); angle = (sin(f) + 1) × fs;
  right wing rot_z = angle, left wing rot_z = −angle.
- Legs: right rot_x = cos(P × 0.6662) × 1.4 × V; left cos(P × 0.6662 + π) × 1.4 × V.
- Head (adult only): rot_x = xR, rot_y = yR. The chick (`chicken_baby`) has no separate head part
  (body, legs, wings): no head rotation.

#### 2.6 Spider

- Texture `textures/entity/spider/spider.png` (64×32); shadow radius 0.8; death tip angle
  **180°** (it flips onto its back).
- Parts: `head` (pivot 0, 15, −3), `body0` (0, 15, 0), `body1` (0, 15, 9), and 8 legs 16×2×2 at
  pivots (±4, 15, z) with z = 2 (back), 1 (middle-back), 0 (middle-front), −1 (front). Base
  pose (rot_y; rot_z) right side: back (π/4; −π/4), middle-back (π/8; −0.58119464),
  middle-front (−π/8; −0.58119464), front (−π/4; −π/4); left side: opposite signs.
- Animation, with q = P × 0.6662:
  - head rot_y = yR, rot_x = xR;
  - horizontal sweep: gB = −cos(2q) × 0.4 × V; gMB = −cos(2q + π) × 0.4 × V;
    gMF = −cos(2q + π/2) × 0.4 × V; gF = −cos(2q + 3π/2) × 0.4 × V;
  - lift: lB = \|sin(q) × 0.4\| × V; lMB = \|sin(q + π) × 0.4\| × V; lMF = \|sin(q + π/2) × 0.4\| × V;
    lF = \|sin(q + 3π/2) × 0.4\| × V;
  - for each pair: right leg rot_y += g, rot_z += l; left leg rot_y −= g, rot_z −= l.
- "Eyes" layer: same model, texture `textures/entity/spider/spider_eyes.png`, emissive render:
  no lightmap (full brightness), no overlay, translucent blend, no depth write,
  drawn after the body (order 1). The layer does not test invisibility: the eyes stay visible
  on an invisible spider (official behaviour).
- Cave spider: same renderer, model scaled 0.7, texture `cave_spider.png`.

#### 2.7 Creeper

- Texture `textures/entity/creeper/creeper.png` (64×32); shadow radius 0.5. Parts: `head` (pivot
  0, 6, 0), `body` (0, 6, 0), 4 legs 4×6×4 at pivots (±2, 18, ±4).
- Animation: head rot_y = yR, rot_x = xR; legs like the quadruped (§2.4).
- Swell (client side, every tick while alive): old = swell; if "ignited" (metadata)
  direction = +1, otherwise direction = metadata "swell direction" (−1 default); swell +=
  direction, clamped [0, 30] (fuse 30 by default, not transmitted). g = lerp(pt, old, swell) / 28.
- Scale (step 6 of §1.2): w = 1 + sin(g × 100) × g × 0.01; h = clamp(g, 0, 1)⁴;
  scale (sxz, sy, sxz) with sxz = (1 + h × 0.4) × w and sy = (1 + h × 0.1) / w.
- White flash: progress = 0 if floor(g × 10) is even, otherwise clamp(g, 0.5, 1) → column u of
  the overlay (§1.6).
- Charge (metadata "charged"): model `creeper_armor` (same geometry inflated by 2 px), texture
  `textures/entity/creeper/creeper_armor.png`, type "energy_swirl": ADDITIVE blend, lightmap and
  overlay active, multiplicative color 0xFF808080 (50% gray), texture coordinates offset by
  (A × 0.01 mod 1, A × 0.01 mod 1); drawn after the body (order 1).

<!-- end §2 -->

---

#### 3. Player

#### 3.1 Renderer

- Two renderers: "wide" (`player`) and "slim" (`player_slim`), chosen according to the skin's model
  type. Shadow radius 0.5. Own scale 0.9375 (step 6 of §1.2). Crouching: render offset
  (0, −2/16 × scale, 0) (the shadow ignores it).
- Model render type: **translucent** (`entity_translucent`, with sorting), not "cutout": the
  semi-transparent pixels of the outer skin layer are blended.
- Spectator: only the head and the hat are drawn (body, arms, legs hidden) and NO
  layer (armor, items, cape…) is drawn.
- Layers, in order: armor (§3.5), held item (§3.4), stuck arrows, "deadmau5" ears
  (exact profile name, out of batch), cape (§3.3), worn head (skull/block on the head), elytra (equipment
  layer `wings`, out of batch), shoulder parrots (out of batch), Riptide effect, bee stingers.
- Name tag: rules of §1.8; a player's name is always a candidate. Vertical offset of the
  text −10 if deadmau5 ears.
- Body rotation: "non-mob living entity" rule of §1.1 (M = 50, 15 when blocking), with the swimming
  and gliding rotations of §1.3.

#### 3.2 Model, skin, outer layer

- Humanoid model (§2.1) + 5 "outer" sub-parts inflated by 0.25 px, children of the base parts:
  `jacket` (body, UV 16,32), `left_sleeve` (UV 48,48), `right_sleeve` (UV 40,32),
  `left_pants` (UV 0,48), `right_pants` (UV 0,32); plus `hat` (head, inflated by 0.5, UV 32,0).
  The left arm and leg have their own UVs (left arm 32,48; left leg 16,48): 64×64 texture.
- Slim: arms 3 px wide instead of 4 (right arm box from x −2 to 1, left from −1 to 2);
  for the held item, the arm is temporarily shifted by x = +0.5 (right) / −0.5 (left) pixel.
- Visibility of the outer parts = player's "displayed skin parts" metadata (byte,
  default 0 = nothing): bit 0 cape, bit 1 jacket (`jacket`), bit 2 left sleeve, bit 3 right sleeve,
  bit 4 left pants leg, bit 5 right, bit 6 hat (`hat`). A part is visible if its
  bit is set.
- Main hand: player's "main hand" metadata (0 left, 1 right).
- Skin:
  - source: `textures` property of the profile received in the player list update (base64
    value of a JSON containing `SKIN` {url, metadata.model}, `CAPE` {url}, possibly `ELYTRA`);
    `metadata.model` = `slim` → slim model, absent or other (`default`) → wide;
  - for any player other than the local player, the property must be **signed** (valid signature
    from the Mojang services); otherwise, or as long as the download is not finished, or if the player
    is not in the list: default skin determined by the UUID (rule already described in
    `docs/specs/items.md` §1.4.11: 18 skins `textures/entity/player/{slim,wide}/<name>.png`, index
    positive_mod(Java hash of the UUID, 18)), with its model type (the first 9 are slim);
  - processing of a downloaded skin (64×32 or 64×64 image, otherwise rejected):
    1. if 64×32 (legacy format): enlarge to 64×64, erase (transparent) the lower half
       (y 32..63), then copy mirrored horizontally (each copied rectangle is flipped left to
       right) the following areas — (x, y, w, h) source → offset (dx, dy):
       (4,16,4,4)→(16,32); (8,16,4,4)→(16,32); (0,20,4,12)→(24,32); (4,20,4,12)→(16,32);
       (8,20,4,12)→(8,32); (12,20,4,12)→(16,32); (44,16,4,4)→(−8,32); (48,16,4,4)→(−8,32);
       (40,20,4,12)→(0,32); (44,20,4,12)→(−8,32); (48,20,4,12)→(−16,32); (52,20,4,12)→(−8,32)
       (target = source + offset) — this makes the left leg and arm from the right ones;
    2. make opaque (alpha = 255) the rectangles x 0..31 × y 0..15 (head), x 0..63 × y 16..31 (body,
       right arm and leg base), x 16..47 × y 48..63 (left leg and arm base);
    3. legacy format only: if ALL the pixels of the area x 32..63 × y 0..31 (hat) have
       alpha ≥ 128, set the alpha of this whole area to 0 (hat opaque by mistake → erased).

#### 3.3 Cape [simplified]

- Shown if: not invisible, "cape" bit of the skin parts, skin with a cape texture, and no
  chest equipment with a `wings` layer (elytra). If the chest has a `humanoid` layer (chestplate):
  translation (0; −0.053125; 0.06875) before drawing.
- Model: box 10×16×1 (texture UV of 64×32: the cape is a 64×32 file read in a 64×64 frame with UV
  factors (1; 0.5)), child of the body at pivot (0, 0, 2), rotated by π around Y;
  the part therefore follows the body (crouching included). Solid render type (`entity_solid`).
- Animation: extra rotation (multiplied onto the pose) = Y(−π) · X((6 + lean/2 + flap)°) ·
  Z((lean2/2)°) · Y((180 − lean2/2)°).
- Variables (per tick, client side, for each player): "cape" point C following the position:
  for each axis, if \|pos − C\| ≤ 10: C += (pos − C) × 0.25, otherwise C = pos (and the old value too).
  Render: d = lerp(pt, C) − lerp(pt, position); forward = (sin b, −cos b) with b = smoothed body yaw;
  flap = clamp(d.y × 10, −6, 32); lean = clamp((d.x × forward.x + d.z × forward.z) × 100 × (1 −
  glideFactor), 0, 150); lean2 = clamp((d.x × forward.z − d.z × forward.x) × 100, −20, 20);
  flap += sin(walkedDistance × 6) × 32 × bob. bob: each tick, target = min(0.1, horizontal
  speed) if on the ground, alive and not swimming, otherwise 0; bob += (target − bob) × 0.4; reset to 0
  when mounted. The walked distance is only accumulated for the local player (0.6 × horizontal distance
  per tick); for the other players it stays 0, so the sinusoidal term is zero.

#### 3.4 Item held in hand (humanoid living entities)

For each arm (right then left), if the item of that arm is not empty:
1. Stack = the model's stack; apply the root transformation then that of the arm part
   (animated pose, §2.1) — for the skeleton and the slim player, with the temporary x offset of the arm.
2. X rotation of −90°, then Y rotation of 180°.
3. Translation (±1/16, 2/16, −10/16) (sign + for the right arm, − for the left); for a baby
   (except armor stand): (0, 1/16, −4.5/16).
4. (Spear / `stab` hit: extra transformations, out of batch.)
5. Drawing of the item in display context `thirdperson_righthand` (right arm) or
   `thirdperson_lefthand` (left arm): display transformation of the item model (format described
   in `docs/specs/items.md` §4: T(t) · R(rx, ry, rz) · S(s) · T(−0.5, −0.5, −0.5), translation
   in 1/16 and clamped ±5, scale clamped ±4); a model without `thirdperson_lefthand` reuses
   the right hand; **for the left hand** the transformation is applied mirrored: tx, ry and rz negated.
   Useful vanilla values: `item/generated` right hand rotation (0,0,0), translation (0,3,1),
   scale 0.55; `item/handheld` right rotation (0,−90,55), translation (0,4,0.5), scale 0.85 (its
   explicit left hand: (0,90,−55)); `block/block` right rotation (75,45,0), translation
   (0,2.5,0), scale 0.375.
6. Neutral overlay (no red tint on the item); entity light.
Player case: if it is using a spyglass in this arm without swinging, the item is drawn against the eye
(context `head`, head at pitch clamped [−π/6, π/2], offset ±2.5/16 in x and −1/16 in y) instead of the
hand.

#### 3.5 Armor and equipment (equipment layers)

Source: `equippable` component of the item (already specified in `item-components.md`): slot
`slot` and `asset_id` (e.g. `minecraft:iron`). Rendered only if `asset_id` is present AND if
the component's slot is the one where the item is worn.

Client file: `assets/<ns>/equipment/<path>.json`:
- `layers` (required, non-empty): table layer type → non-empty list of layers. Types:
  `humanoid`, `humanoid_leggings`, `humanoid_baby`, `wings`, `wolf_body`, `horse_body`, `llama_body`,
  `pig_saddle`, `strider_saddle`, `camel_saddle`, `camel_husk_saddle`, `horse_saddle`,
  `donkey_saddle`, `mule_saddle`, `zombie_horse_saddle`, `skeleton_horse_saddle`, `happy_ghast_body`,
  `nautilus_saddle`, `nautilus_body`.
- Layer: `texture` (identifier), `dyeable` (optional: object with optional RGB `color_when_undyed`),
  `use_player_texture` (boolean, default false; replaces the texture with the player's —
  profile cape/elytra).
- `trim_overrides` (optional): list of { `when` : { `material`?, `pattern`? } (at least one),
  `texture`? , `palette`? (at least one) }.
- Texture of a layer: `<ns>:textures/entity/equipment/<type>/<path>.png`
  (e.g. `iron` in `humanoid` → `textures/entity/equipment/humanoid/iron.png`; leggings →
  `.../humanoid_leggings/iron.png`).

Humanoids (armor layer, order: chest, legs, feet, head):
- Armor model = humanoid model of the same type as the wearer (same animations, same poses), of which
  only the parts of the slot are kept:
  | Slot | Parts | Inflation | Layer type |
  |---|---|---|---|
  | head | `head` (+ `hat`) | 1.0 (hat 1.5) | `humanoid` |
  | chest | `body`, `right_arm`, `left_arm` | 1.0 | `humanoid` |
  | legs | `body`, `right_leg`, `left_leg` | 0.5 (legs 0.4) | `humanoid_leggings` |
  | feet | `right_leg`, `left_leg` | 0.9 | `humanoid` |
  Baby (except armor stand): layer type `humanoid_baby`, own baby mesh (inflations
  (−0.1; 0.5; 0.3) outer, (−0.1; 0.3; 0.3) inner; parts `waist` for the legs,
  `left_foot`/`right_foot` for the feet) [geometry: models mission].
- For each layer of the list (in order): color = if `dyeable`: color of the component
  `dyed_color` (opaque) if present, otherwise `color_when_undyed` (opaque), otherwise 0 → **layer skipped**;
  without `dyeable`: white. Render "armor_cutout_no_cull" (cutout, back faces visible, lightmap,
  neutral overlay), multiplicative color. Leather: dyeable layer `leather` (default 0xA06540) then
  undyed `leather_overlay`.
- Enchanted glint: if the stack glints (enchanted or `enchantment_glint_override`) and WITHOUT trim:
  the first drawn layer carries the glint ("glint" variant). With trim: glint drawn
  after the trim.
- Trim (component `trim`, not on `humanoid_baby`):
  - pattern (registry `trim_pattern`): `asset_id`, `decal`; material (registry `trim_material`):
    `palette_id` (e.g. `minecraft:trim/iron`);
  - the first matching `trim_overrides` can replace the texture and/or the palette (an
    override without a palette = no palette); vanilla: iron worn on iron armor uses
    `trim/iron_darker` (same for gold, diamond, copper, netherite with their `_darker`);
  - base texture: `<ns>:textures/trims/entity/<type>/<asset_id>.png`; its `.mcmeta` metadata
    `palette.base_palette` (e.g. `trim_base`) designates the source palette;
  - palettes: `textures/palettes/<id>.png`, read as a list of pixels (row by row);
    source and target of the same size. Replacement: for each pixel with non-zero alpha of the texture, its
    RGB color (opaque) is looked up in the source palette; if found at index i, the color
    becomes that of the target palette at i, with alpha = textureAlpha × targetAlpha / 255; otherwise
    unchanged. The result is stored in a dynamic 512×512 atlas (free implementation detail);
  - render "armor_trim" (or "armor_trim_decal" if `decal`) with a slight depth offset toward
    the camera, white color.

<!-- end §3 -->

---

#### 4. Simple non-living entities

#### 4.1 Dropped item (`item`)

- Shadow: radius 0.15, intensity 0.75. Display context `ground` of the item model.
- `bob_phase`: uniform random number in [0, 2π) drawn at entity creation client side.
- Bounding box of the model: box of the model's vertices after the transformations of each layer
  (`ground` display + local transformation).
- Transformations (after translation to the entity's position):
  1. Y translation of bob + (−min_y + 0.0625), with bob = sin(A / 10 + bob_phase) × 0.1 + 0.1;
  2. Y rotation (radians) of A / 20 + bob_phase;
  3. copies (below).
- Number of copies n depending on the stack count: 1 → 1; 2‑16 → 2; 17‑32 → 3; 33‑48 → 4;
  49+ → 5.
- Randomness: classic 48-bit linear congruential generator (multiplier 0x5DEECE66D, increment 11),
  reset at each render with the seed (seed ⊕ 0x5DEECE66D); seed = raw id of the
  item in the `item` registry + damage (component `damage`, 0 otherwise); float = upper
  24 bits × 2⁻²⁴.
- If depth (Z size of the box) > 0.0625 (blocks, 3D items): copy 0 in place; copies 1..n−1
  translated by (r1, r2, r3) with ri = (random × 2 − 1) × 0.15.
- Otherwise (flat items): e = depth × 1.5; Z translation of −e × (n − 1)/2; copy 0; Z translation
  of +e; then for each following copy: local translation (r1, r2, 0) with ri = (random × 2 − 1) ×
  0.075, draw, return, Z translation of +e.
- Light: level at the entity's block (block = 15 if on fire). No "hurt" tint.

#### 4.2 Experience orb

- Shadow: radius 0.15, intensity 0.75. Block light = min(light + 7, 15).
- Icon depending on the value (integer metadata): ≥ 2477 → 10; ≥ 1237 → 9; ≥ 617 → 8; ≥ 307 → 7;
  ≥ 149 → 6; ≥ 73 → 5; ≥ 37 → 4; ≥ 17 → 3; ≥ 7 → 2; ≥ 3 → 1; otherwise 0.
- Texture `textures/entity/experience/experience_orb.png` (64×64, cells of 16): u0 = (icon mod 4) ×
  16/64, v0 = (icon div 4) × 16/64, u1 = u0 + 0.25, v1 = v0 + 0.25.
- Color per vertex: r = floor((sin(A/2) + 1) × 0.5 × 255), g = 255,
  b = floor((sin(A/2 + 4π/3) + 1) × 0.1 × 255), alpha 128.
- Geometry: translation (0, 0.1, 0), rotation = camera orientation (billboard facing the camera),
  scale 0.3; a quad from (−0.5, −0.25) to (0.5, 0.75) in the plane z = 0 (bottom-left UV (u0, v1),
  bottom-right (u1, v1), top-right (u1, v0), top-left (u0, v0)), normal (0, 1, 0).
  Translucent render (`entity_translucent`), neutral overlay.

#### 4.3 Arrow

- Textures: `textures/entity/projectiles/arrow.png`; tipped arrow (effect color > 0):
  `arrow_tipped.png`; spectral arrow: `arrow_spectral.png`. Render "entity_cutout" with back-face
  culling (cull).
- Model (32×32): `back` (square 0×5×5 at x = −11, rotated π/4 on X, scale 0.8), `cross_1` and
  `cross_2` (rectangle 16×4×0 from x = −12 to 4, UV scaled (1; 0.8), rotated by π/4 and 3π/4 on
  X); all poses multiplied by 0.9 (positions and scales).
- Transformations: Y rotation of (yaw − 90)°, then Z rotation of pitch° (yaw and pitch
  interpolated). No −1 flip nor −1.501 translation (unlike living entities).
- Shake on impact: shake_time = 7 when the "in ground" metadata becomes true (client side);
  −1 per tick; s = shake_time − pt; if s > 0: root rot_z += (−sin(3s) × s)°.
- Arrows stuck in a player: "arrows" layer (count = metadata), out of batch.

#### 4.4 Painting

- Registry `painting_variant` (synchronized): `width`, `height` (in blocks), `asset_id` → sprite of
  the paintings atlas (`textures/painting/<asset>.png`); back and edges: sprite `back`.
- Y rotation of (180 − d × 90)° with d = horizontal direction (south 0, west 1, north 2, east 3).
- Geometry per cell (sx, sy) of a centered width × height grid (x from −w/2 to w/2, y from −h/2 to h/2):
  front face at z = −1/32 (normal −Z) showing the matching portion of the front sprite (u
  decreasing as x increases; v decreasing as y increases); back face at z = +1/32 (whole `back`
  sprite); top/bottom/left/right edges on the perimeter (sprite `back`: strip of 1/16
  height for top/bottom, 1/16 width for the sides). Solid render with a slight depth offset
  toward the front.
- Light **per cell**: block containing the center of the cell (depending on direction: north x =
  floor(X + ox), west z = floor(Z − ox), south x = floor(X − ox), east z = floor(Z + ox); y = floor(Y +
  oy), where (ox, oy) = center of the cell relative to the center of the painting).
- Item frame (`item_frame`): not covered in this batch.

<!-- end §4 -->

---

### Uncertain points

1. **Mob body rotation controller**: described from the reading; the "non-mob living entity" rule
   (§1.1 steps 1‑3) does NOT apply to mobs. Value head_m = 75 for all the mobs
   read (some species may override it, not verified).
2. **Interpolated position of entities** (smoothing steps of the movement packets): out of scope
   here, to be taken from the network mission.
3. **Baby model geometry** (cow, pig, sheep, chicken, zombie): only the pivots of the baby zombie
   are given; the rest belongs to the "models" mission. The animation formulas are
   identical, except the chick without an animated head.
4. **Spear (`SPEAR`) and `stab` hit**: spear animations not detailed.
5. **Cape**: rotation formula read but not visually verified; the cape texture atlas
   (64×32 read with UV factors (1; 0.5)) is to be confirmed at implementation.
6. **Decoding of the `textures` property**: done by a library external to the game (publicly known
   base64 JSON format); signature verification not detailed (public key of the services).
7. **Exact order of the translucent passes** (spider eyes, creeper charge, "see-through" tag):
   relative orders 0/1 read; integration with OIT (order-independent transparency) is not followed.
8. **"Frozen" shaking**: the addition cos(…) × π × 0.4 is in degrees (≈ ±1.26°) according to the reading;
   the weak visual effect is consistent but not measured.
9. **Elytra, worn head (skulls), parrots, deadmau5 ears, stuck arrows, item
   frame**: flagged, not specified in this batch (see the addendum below, outside the frame).

## Addendum (wave W3b): special poses and living-entity layers

### Poses tracked client side (any living entity)
- **Swimming**: counter w ∈ [0, 1], +0.09 per tick if swimming is visible ("swimming" pose, or
  "gliding" pose without the gliding flag; drowned: "swimming" flag 0x10 outside a mount), otherwise
  −0.09; interpolated by pt.
- **Gliding**: flight ticks (flag 0x80), reset to 0 on the ground; opening = min(1, (t + pt)² / 100)
  — applied even on the ground (≤ 0.01), it also erases the forward lean of the cape (× (1 − o)).
- **Wings** (x, y, z) starting from 0, 30% of the way per tick toward: in flight, x = lerp(r, π/12, π/9),
  z = lerp(r, −π/12, −π/2) with r = 1 − (−vy/|v|)^1.5 if the entity is descending (1 otherwise); crouching
  (2π/9, 5°, −π/4); otherwise (π/12, 0, −π/12).
- **Use**: ticks since the start of use (living flag 1); crossbow charging:
  pulling arm y = lerp(k, 0.4, 0.85), x = lerp(k, −0.9708, −π/2), k = ticks / 25 clamped.

### Rotations (after the living entity's scale)
Order: shaking (body yaw + cos(⌊age⌋ × 3.25) × π × 0.4 **degrees**); yaw 180 − body
(except lying); then exclusive ones: death tip (180° spiders, endermite, silverfish),
spin attack (X −90 − pitch, Y age × −75), lying (Y depending on the bed direction — south 90,
west 0, north 270, east 180, otherwise the body —, Z 90/180, Y 270; model shifted before the scale
by −direction × (standing eyes − 0.1)), upside down (raised by (height + 0.1) / scale, Z 180;
pitch and relative head inverted). Squids, armor stand and dragon have none.
Shaking: full freeze (data 7 ≥ 140); zombie/drowned/husk (18), zombie villager (19),
skeleton alone (16, without the freeze), piglins outside the Nether (immunity 16, AI), hoglin (18), strider (19).
Avatars then: gliding → X o × (−90 − pitch) (not during the spin attack), then Y
of the signed angle between horizontal movement and gaze (if both > 1e-5); otherwise swimming → X
w × (−90 − pitch in water, −90 out of water), then (0; −1; 0.3) in visible swimming. Drowned: X
w × (−10 − pitch) around the middle of its box. Crouching avatar: render lowered by 2/16 × scale
(not the shadow). Upside down: name "Dinnerbone"/"Grumm" (avatar: only if cape displayed).

### Humanoid model
Gliding: head −π/4, limbs divided by max(1, (|v|²/0.2)³). Breaststroke (w > 0): head toward −π/4
(outside flight), 26-cycle on the walk (0..14 arc q(x) = x² − 65x, 14..22 return, 22..26 arms
brought back), left arm shortest-path interpolated, right linear, hitting arm excluded, nothing during
a use; legs 0.3 cos(walk / 3 [+ π]). Drowned: own swim (arms −4π/5 ± 0.35 sin(0.1 age),
z ∓0.15, legs ∓0.55 sin, head 0); raised "trident" arm if aggressive with a trident.

### Added layers
Elytra (wings model, 2/16 setback, player cape if the asset asks for it), worn heads (skulls
× 1.1875 animated by walking; other items in the "head" context × 0.625; villagers lowered,
piglins widened), enderman and phantom eyes, enderman block, golem poppy, snow golem pumpkin,
mooshroom mushrooms, charged creeper halo (scroll 0.01/tick),
golem cracks (health < 3/4, 1/2, 1/4), wolf (tamed/angry textures, armor + cracks
0.95/0.69/0.32, dyed collar, darkened while shaking), cat collar, villager profession/type/level
(type hat removed according to the `.mcmeta`), pig and strider saddles
(cold texture), warden emissive layers (without lightmap, pulsing alphas), creaking eyes,
breeze wind and eyes, shoulder parrots, stuck arrows and stingers (generator seeded by the
id), Riptide swirl, "deadmau5" ears, "jeb_" wool (16 colors × 25
ticks), held items of villagers/trader/witch (potion at the nose)/fox/panda/dolphin,
spyglass held at the eye, copper golem (oxidation, eyes, antenna block, worn head).
Remaining: armor trims (palettes), spear (`SPEAR`), silhouette alone of an invisible
glowing block layer, scared panda.


