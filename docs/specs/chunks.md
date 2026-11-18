# Chunks, light, registries, joining the game (26.3)

Sources: community wiki, reading of the official game, verification against the reference capture.

## Network NBT
- A tag = type byte, then payload, **no name** at the root. Type 0 = absent (empty optional field).
- NBT strings: `u16` length then modified UTF-8 (NUL encoded on 2 bytes, characters outside the basic plane as surrogate pairs).

## Registries (configuration phase)
- Registry id, then a (VarInt) list of entries: id + optional NBT (boolean + tag).
- The order of the entries gives the numeric id used afterwards in play.
- If the client announces no known pack, the server sends the full content of every entry.
- Dimension type: `min_y` (multiple of 16) and `height` determine the number of sections (`height / 16`).

## Joining the game
Integer (player id), boolean (hardcore), list of dimension keys, VarInt × 3 (max players, view distance, simulation distance), booleans × 3, then spawn information: dimension type (VarInt, index in the received registry), dimension key, hashed seed (`i64`), game mode (VarInt), previous mode (VarInt encoded + 1, 0 = none), booleans × 2 (debug, flat), optional last death (dimension + position), portal cooldown, sea level (VarInt); finally two booleans: online mode, enforced secure chat.

## Chunk packet with light
1. `x`, `z`: 32-bit integers.
2. Heightmaps: VarInt count, then for each one a type (VarInt) and a prefixed (VarInt) array of `i64`.
3. Section buffer: VarInt length (≤ 2 MiB) then, for each section from bottom to top:
   - number of non-empty blocks (`i16`), number of fluids (`i16`);
   - block state container (4096 entries, index `(y·16 + z)·16 + x`);
   - biome container (64 entries, index `(y·4 + z)·4 + x`).
4. Block entities: VarInt count; each one: packed `xz` (byte, x in the high bits), `y` (`i16`), type (VarInt), optional NBT.
5. Light: four bit sets, each a prefixed **byte array** (VarInt length; bit 0 = low bit of the first byte — not `i64` words as in older versions) — sky present, block present, sky empty, block empty —, then two lists of 2048-byte arrays (VarInt length ≤ 2048 each). Mask index 0 = the section just below the world; there are `sections + 2` positions.

## Paletted container
- `bits` byte, palette, then an `i64` array **with no** length prefix, of fixed length `ceil(entries / (64 / width))`. Values never straddle two words; the first one is in the low bits.
- The **read width depends on the local configuration** and not only on the received byte:

| Received byte | Blocks (4096) | Biomes (64) |
|---|---|---|
| 0 | single value: 1 VarInt, no words | same |
| 1–3 | linear palette, **width 4** | linear palette, width = byte |
| 4 | linear palette, width 4 | direct, local global width |
| 5–8 | palette (table), width = byte | direct, local global width |
| ≥ 9 | direct, **width = bits of the local global palette** (16 in 26.3) | direct |

- Palette: VarInt size then global ids (VarInt). Direct: no palette.
- Local global width = `ceil(log2(number of entries))` of the registry (block states; biomes received during configuration).
