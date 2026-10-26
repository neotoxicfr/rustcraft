# Offline connection up to play (26.3)

Sources: community wiki, reading of the official game, reference capture.

## Handshake and login
1. Handshake: protocol (VarInt), address (string ≤ 255), port (`u16`), intent (VarInt: 1 status, 2 login, 3 transfer).
2. Client `hello`: username (string ≤ 16), UUID (128 bits). Offline, the official client sends the UUID of its profile; we send the standard offline UUID (version 3, MD5 of `OfflinePlayer:<username>`).
3. Server: optionally a compression threshold (VarInt, applied from the next packet on, in both directions), then login finished (profile: UUID, username, properties; then a session UUID).
4. Client: login-finished acknowledgement → configuration phase.

## Configuration
- The client sends its information: language (≤ 16), view distance (byte), chat visibility (VarInt), colors (boolean), skin parts (byte), main hand (VarInt), filtering (boolean), listed in the player list (boolean), particles (VarInt). Without this packet, the server keeps a default view distance of **2**.
- Known packs: the client replies with the list of packs it knows (list ≤ 64). Empty list → the server sends the full content of the registries.
- Keep-alive (`i64` echoed back unchanged), ping (`i32` echoed back as pong), code of conduct (accept), finish configuration (reply with finish configuration → play phase).

## Play
- Player position: id (VarInt), position and movement (3 doubles + 3 doubles), yaw and pitch (`f32`), relative flags (`i32`: bits 0 x, 1 y, 2 z, 3 yaw, 4 pitch, 5–7 movement x/y/z, 8 movement rotation). The client replies with the acceptance: id + final position (3 doubles) + yaw + pitch.
- Chunks in batches: batch start (empty), chunks, batch finished (VarInt size). The client **must** acknowledge each batch with the desired number of chunks per tick (`f32`), otherwise the server stops sending (at most 10 unacknowledged batches). Official computation: 7,000,000 ns ÷ average processing time of a chunk (weighted average, sample clamped to ×3 / ÷3, weight ≤ 49, start 2 ms); the server clamps to [0.01, 64].
- Chunk forget: position packed in 64 bits (x in the low bits, z in the high bits).
- Cache center (VarInt x, z), cache radius (VarInt).
- Block change: packed block position + state (VarInt).
- Changes within a section: section position (`i64`: x on 22 bits in the high bits, z on 22 bits, y on 20 bits in the low bits), VarInt count, then for each one a VarLong = `state << 12 | (x << 8 | z << 4 | y)` local.
- After the first accepted position, the client signals that it has loaded the world (empty packet).

## Client tick and packets emitted by the game logic
Order of a tick (20 per second): inputs → entity physics (including the local player) →
player synchronization → rest of the world → **end of tick** (empty packet, sent every tick
while not paused; observed in the capture: ≈ 20 per second).
- Player synchronization (once the world is loaded): if the keys changed since the last
  send, an input packet (byte: forward 1, backward 2, left 4, right 8, jump 16, sneak 32,
  sprint 64); then the position (see "position sending" in `movement.md`).
- Movement: x, y, z (`f64`), then depending on the packet yaw and pitch (`f32`), then a flags
  byte (on ground 1, horizontal collision 2).
- Player command: entity id (VarInt), action (VarInt: stop sleeping 0, start
  sprinting 1, stop sprinting 2, …), data (VarInt, 0).
