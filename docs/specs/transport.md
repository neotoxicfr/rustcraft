# Transport: frames, compression, encryption (26.3)

Sources: community wiki "Java Edition protocol", reading of the official game (`decompilation` procedure), to be confirmed by capture.

## Frame
- Prefix: content length as a VarInt, **at most 3 bytes** (length ≤ 2,097,151). A wider prefix is a fatal error.
- A zero length is a fatal error.
- The sender refuses to send content whose prefix would exceed 3 bytes.

## Compression (after the compression-enabling packet of the login phase)
- Frame content = VarInt "decompressed size" followed by the data.
- Decompressed size = 0: data sent as is.
- Otherwise: zlib data; the announced size must be **≥ threshold** and **≤ 8,388,608** (8 MiB), otherwise fatal error ("badly compressed packet"). The minimum check is active in the official client.
- Sending: compress if the size ≥ threshold, otherwise size 0 + raw data. Refuse to send beyond 8 MiB decompressed.
- Negative threshold received: compression disabled.

## Encryption (after the key exchange)
- AES-128 in CFB8 mode, no padding, applied to the whole byte stream (prefixes included), in both directions.
- Key = initialization vector = shared secret of 16 bytes. One state per direction, kept throughout the connection.
- Order when sending: content → compression → length prefix → encryption. Reverse when receiving.
