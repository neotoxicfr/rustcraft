"""Game-folder fixtures of the off-world scenes of the bench (`scenes.py`): server list with fake
local status servers (favicons), extra worlds in the world list, resource packs. All of it is
written into the staged game folder; nothing listens outside 127.0.0.1.
"""
import base64
import json
import os
import shutil
import socket
import struct
import threading
import zipfile
import zlib

import nbt

STATUS_PORT = 25601
PROTOCOL = 777


# --- Images ---------------------------------------------------------------------------------


def png(width, height, pixel):
    """PNG (RGB, 8 bits) whose pixel (x, y) is `pixel(x, y)` -> (r, g, b)."""
    raw = b"".join(b"\0" + bytes(c for x in range(width) for c in pixel(x, y)) for y in range(height))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def icon(k):
    """64 × 64 icon number `k`: checkerboard of two colours drawn from k."""
    a = ((k * 53) % 256, (k * 97) % 256, (k * 31 + 80) % 256)
    b = (255 - a[0], 255 - a[1], 255 - a[2])
    return png(64, 64, lambda x, y: a if (x // 8 + y // 8 + k) % 2 else b)


# --- Server list and fake status servers -----------------------------------------------------


def write_servers(game, n, port=STATUS_PORT):
    """`servers.dat` (uncompressed, like the game) listing `n` servers on 127.0.0.1:port…port+n-1."""
    servers = [{"name": (nbt.STRING, f"Perf server {k + 1}"), "ip": (nbt.STRING, f"127.0.0.1:{port + k}")}
               for k in range(n)]
    nbt.write_raw(os.path.join(game, "servers.dat"), {"servers": nbt.List(nbt.COMPOUND, servers)})


def varint(v):
    out = bytearray()
    v &= 0xFFFFFFFF
    while True:
        b = v & 0x7F
        v >>= 7
        out.append(b | (0x80 if v else 0))
        if not v:
            return bytes(out)


def read_varint(conn):
    v, shift = 0, 0
    while True:
        b = conn.recv(1)
        if not b:
            raise ConnectionError("closed")
        v |= (b[0] & 0x7F) << shift
        if not b[0] & 0x80:
            return v
        shift += 7


def read_packet(conn):
    n = read_varint(conn)
    data = b""
    while len(data) < n:
        part = conn.recv(n - len(data))
        if not part:
            raise ConnectionError("closed")
        data += part
    return data


def packet(packet_id, body):
    data = varint(packet_id) + body
    return varint(len(data)) + data


def status_json(k):
    """Status of fake server `k`: version 26.3, 3 + k players of 20, MOTD, favicon."""
    return json.dumps({
        "version": {"name": "26.3", "protocol": PROTOCOL},
        "players": {"max": 20, "online": 3 + k, "sample": [{"name": f"Player{k}{i}", "id": f"00000000-0000-3000-8000-{k:06d}{i:06d}"}
                                                          for i in range(3)]},
        "description": {"text": f"Perf server {k + 1}", "color": "gold"},
        "favicon": "data:image/png;base64," + base64.b64encode(icon(k)).decode(),
    })


def serve_status(conn, k):
    """Handshake, status request, ping: answered like a server of the version."""
    conn.settimeout(10)
    read_packet(conn)  # handshake (protocol, address, port, intent)
    while True:
        data = read_packet(conn)
        if data[:1] == b"\x00":
            text = status_json(k).encode()
            conn.sendall(packet(0, varint(len(text)) + text))
        elif data[:1] == b"\x01":
            conn.sendall(packet(1, data[1:9]))
            return


class StatusServers:
    """`n` fake status servers on 127.0.0.1, port `port` + k, for the time of a `with`."""

    def __init__(self, n, port=STATUS_PORT):
        self.stop = threading.Event()
        self.socks = []
        for k in range(n):
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            s.bind(("127.0.0.1", port + k))
            s.listen(4)
            s.settimeout(0.5)
            self.socks.append(s)
        self.threads = [threading.Thread(target=self._loop, args=(s, k), daemon=True) for k, s in enumerate(self.socks)]

    def _loop(self, s, k):
        while not self.stop.is_set():
            try:
                conn, _ = s.accept()
            except OSError:
                continue
            with conn:
                try:
                    serve_status(conn, k)
                except (OSError, ConnectionError):
                    pass

    def __enter__(self):
        for t in self.threads:
            t.start()
        return self

    def __exit__(self, *_):
        self.stop.set()
        for t in self.threads:
            t.join(timeout=2)
        for s in self.socks:
            s.close()


# --- Worlds and resource packs ----------------------------------------------------------------


def write_worlds(game, model, n):
    """`n` extra worlds in the world list: the model's `level.dat` renamed, its own icon and
    play date each (no terrain: the list reads only the summary)."""
    level = nbt.read_dat(os.path.join(model, "level.dat"))
    for k in range(n):
        d = os.path.join(game, "saves", f"Perf world {k + 1:02d}")
        os.makedirs(d, exist_ok=True)
        level["Data"]["LevelName"] = (nbt.STRING, f"Perf world {k + 1:02d}")
        level["Data"]["LastPlayed"] = (nbt.LONG, 1_780_000_000_000 - k * 3_600_000)
        nbt.write_dat(os.path.join(d, "level.dat"), level)
        with open(os.path.join(d, "icon.png"), "wb") as f:
            f.write(icon(100 + k))


def write_resource_packs(game, n):
    """`n` zipped resource packs (description, icon; no assets), declared for every 26.x format."""
    d = os.path.join(game, "resourcepacks")
    if os.path.exists(d):
        shutil.rmtree(d)
    os.makedirs(d)
    for k in range(n):
        meta = {"pack": {"description": f"Perf pack {k + 1}", "min_format": 65, "max_format": 999}}
        with zipfile.ZipFile(os.path.join(d, f"perf-pack-{k + 1:02d}.zip"), "w", zipfile.ZIP_DEFLATED) as z:
            z.writestr("pack.mcmeta", json.dumps(meta))
            z.writestr("pack.png", icon(200 + k))
