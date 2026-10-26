# `rc-capture` capture format (`.rccap`)

Files are produced outside the repository (`%LOCALAPPDATA%\rustcraft-re\captures\`) and never committed.

- Header: 8 bytes `RCCAP\0\0\x01` (version 1).
- Then a sequence of records, one per decoded packet:
  - little-endian `u64`: nanoseconds since the connection was opened;
  - `u8` direction: 0 = server → client, 1 = client → server;
  - `u8` phase at the time of the packet: 0 handshake, 1 status, 2 login, 3 configuration, 4 play;
  - VarInt: packet length;
  - the decompressed, decrypted packet: VarInt id + body.

The proxy relays the bytes unmodified; it updates the phase and compression before forwarding, which guarantees that the other end cannot reply before the state change. Offline only (online, the traffic is encrypted end to end).
