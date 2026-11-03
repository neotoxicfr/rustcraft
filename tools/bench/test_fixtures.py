"""Tests of the off-world fixtures of the bench (`python -I tools/bench/test_fixtures.py`): server
list, fake status server queried like the client does (127.0.0.1 only), extra worlds,
resource packs, icons."""
import base64
import json
import os
import socket
import struct
import sys
import tempfile
import unittest
import zipfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import fixtures  # noqa: E402
import nbt  # noqa: E402
import scenes  # noqa: E402

TEST_PORT = 46101


def scratch():
    base = os.path.join(scenes.ROOT, "target", "bench-tests")
    os.makedirs(base, exist_ok=True)
    return tempfile.TemporaryDirectory(dir=base)


def png_size(data):
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    w, h = struct.unpack(">II", data[16:24])
    return w, h


class Images(unittest.TestCase):
    def test_png_decodes(self):
        data = fixtures.icon(3)
        self.assertEqual(png_size(data), (64, 64))
        idat = data[data.index(b"IDAT") + 4:data.index(b"IEND") - 8]
        raw = zlib.decompress(idat)
        self.assertEqual(len(raw), 64 * (1 + 64 * 3))
        self.assertNotEqual(fixtures.icon(3), fixtures.icon(4))


class Servers(unittest.TestCase):
    def test_server_list_uncompressed(self):
        with scratch() as t:
            fixtures.write_servers(t, 6, TEST_PORT)
            with open(os.path.join(t, "servers.dat"), "rb") as f:
                self.assertEqual(f.read(1), b"\x0a")  # raw compound, not gzip
            root = nbt.read_dat(os.path.join(t, "servers.dat"))
        ips = [s["ip"][1] for s in root["servers"].items]
        self.assertEqual(ips, [f"127.0.0.1:{TEST_PORT + k}" for k in range(6)])

    def query(self, port):
        """Handshake (intent 1), status request, ping: what the multiplayer screen sends."""
        with socket.create_connection(("127.0.0.1", port), timeout=5) as s:
            host = b"127.0.0.1"
            hs = fixtures.varint(0) + fixtures.varint(777) + fixtures.varint(len(host)) + host + struct.pack(">H", port) + fixtures.varint(1)
            s.sendall(fixtures.varint(len(hs)) + hs + fixtures.packet(0, b""))
            body = fixtures.read_packet(s)
            self.assertEqual(body[:1], b"\x00")
            r = _Reader(body[1:])
            status = json.loads(r.take(r.varint()).decode())
            s.sendall(fixtures.packet(1, struct.pack(">q", 1234567)))
            pong = fixtures.read_packet(s)
        self.assertEqual(pong, b"\x01" + struct.pack(">q", 1234567))
        return status

    def test_status_answered(self):
        with fixtures.StatusServers(2, TEST_PORT):
            first, second = self.query(TEST_PORT), self.query(TEST_PORT + 1)
            again = self.query(TEST_PORT)
        self.assertEqual(first["version"]["protocol"], 777)
        self.assertEqual((first["players"]["online"], second["players"]["online"]), (3, 4))
        self.assertEqual(first, again)
        icon = base64.b64decode(first["favicon"].split(",", 1)[1])
        self.assertEqual(png_size(icon), (64, 64))
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", TEST_PORT), timeout=1).close()


class _Reader:
    def __init__(self, b):
        self.b, self.i = b, 0

    def take(self, n):
        v = self.b[self.i:self.i + n]
        self.i += n
        return v

    def varint(self):
        v, shift = 0, 0
        while True:
            c = self.take(1)[0]
            v |= (c & 0x7F) << shift
            if not c & 0x80:
                return v
            shift += 7


class Worlds(unittest.TestCase):
    def test_world_list_and_packs(self):
        with scratch() as t:
            model, game = os.path.join(t, "model"), os.path.join(t, "game")
            os.makedirs(model)
            nbt.write_dat(os.path.join(model, "level.dat"), {"Data": {"LevelName": (nbt.STRING, "perf")}})
            fixtures.write_worlds(game, model, 20)
            fixtures.write_resource_packs(game, 20)
            worlds = sorted(os.listdir(os.path.join(game, "saves")))
            self.assertEqual(len(worlds), 20)
            level = nbt.read_dat(os.path.join(game, "saves", worlds[19], "level.dat"))
            self.assertEqual(level["Data"]["LevelName"][1], "Perf world 20")
            with open(os.path.join(game, "saves", worlds[0], "icon.png"), "rb") as f:
                self.assertEqual(png_size(f.read()), (64, 64))
            packs = sorted(os.listdir(os.path.join(game, "resourcepacks")))
            self.assertEqual(len(packs), 20)
            with zipfile.ZipFile(os.path.join(game, "resourcepacks", packs[0])) as z:
                meta = json.loads(z.read("pack.mcmeta"))
                self.assertEqual(png_size(z.read("pack.png")), (64, 64))
        self.assertEqual((meta["pack"]["min_format"], meta["pack"]["max_format"]), (65, 999))


if __name__ == "__main__":
    unittest.main()
