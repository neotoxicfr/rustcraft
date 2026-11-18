# Player list (26.3, protocol 777)

Source: reading of the decompiled client (outside the repository), reworded.

## 1. `player_info_update` (play, client-bound)

- `u8`: set of actions (bit *i* = action with id *i*, 8 actions → 1 byte).
- `varint`: number of entries, then for each entry: UUID (`u128`), then, in id
  order, the fields of the present actions only:

| id | action | fields |
|---|---|---|
| 0 | add player | name (string ≤ 16), properties: `varint` n ≤ 16, then n × { name (≤ 64), value (≤ 32767), optional signature (`bool` + string ≤ 1024) } |
| 1 | chat session | `bool` present, then session UUID, expiry (`i64`, ms), public key (array ≤ 512), key signature (array ≤ 4096) |
| 2 | game mode | `varint` (0 survival, 1 creative, 2 adventure, 3 spectator) |
| 3 | listed | `bool` |
| 4 | latency | `varint` (ms) |
| 5 | display name | `bool` present, then text component (NBT) |
| 6 | list order | `varint` |
| 7 | hat visible | `bool` |

Byte arrays: `varint` length then bytes.

Client side: the add creates the entry (profile = UUID, name, properties); the other actions
modify an existing entry and are ignored for an unknown UUID.

## 2. `player_info_remove`

`varint` n then n UUIDs: entries removed.

## 3. Usage

- `player` entity: its name is that of the profile of the entry with the same UUID (name tag,
  `entities.md` part 2 § 1.8; team prefix and suffix as well once teams are
  tracked).
- `textures` property: base 64 JSON, `textures.SKIN.url` (and `metadata.model` = `slim`
  for slim arms), `textures.CAPE.url`; without the property, default appearance drawn from
  the UUID.
