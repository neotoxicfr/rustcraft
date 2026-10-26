"""Minimal NBT (big-endian, gzip-compressed) to prepare the scene bench worlds.

Values: dict (compound, order kept), `List(element_type, [values])`, and for scalars a
tuple (type, value): (1, byte), (2, short), (3, int), (4, long), (5, float), (6, double),
(8, string), (7|11|12, array). `read_dat` / `write_dat` read and write a `.dat` file.
"""
import gzip
import struct

END, BYTE, SHORT, INT, LONG, FLOAT, DOUBLE, BYTES, STRING, LIST, COMPOUND, INTS, LONGS = range(13)
_FMT = {BYTE: ">b", SHORT: ">h", INT: ">i", LONG: ">q", FLOAT: ">f", DOUBLE: ">d"}


class _R:
    def __init__(self, b):
        self.b, self.i = b, 0

    def take(self, n):
        v = self.b[self.i:self.i + n]
        self.i += n
        return v

    def un(self, fmt):
        n = struct.calcsize(fmt)
        return struct.unpack(fmt, self.take(n))[0]

    def string(self):
        return self.take(self.un(">H")).decode("utf-8", "replace")


def _payload(r, t):
    if t in _FMT:
        return r.un(_FMT[t])
    if t == STRING:
        return r.string()
    if t == BYTES:
        return r.take(r.un(">i"))
    if t == INTS:
        n = r.un(">i")
        return list(struct.unpack(f">{n}i", r.take(4 * n)))
    if t == LONGS:
        n = r.un(">i")
        return list(struct.unpack(f">{n}q", r.take(8 * n)))
    if t == LIST:
        et, n = r.un(">b"), r.un(">i")
        return List(et, [_wrap(et, _payload(r, et)) for _ in range(n)])
    if t == COMPOUND:
        out = {}
        while True:
            ct = r.un(">b")
            if ct == END:
                return out
            name = r.string()
            out[name] = _wrap(ct, _payload(r, ct))
    raise ValueError(f"unknown NBT type {t}")


def _wrap(t, v):
    return v if t in (LIST, COMPOUND) else (t, v)


class List:
    """NBT list: element type and values (wrapped as elsewhere)."""

    def __init__(self, et, items):
        self.et, self.items = et, items


def _type(v):
    if isinstance(v, dict):
        return COMPOUND
    if isinstance(v, List):
        return LIST
    return v[0]


def _put(out, t, v):
    if t in _FMT:
        out += struct.pack(_FMT[t], v[1])
    elif t == STRING:
        b = v[1].encode("utf-8")
        out += struct.pack(">H", len(b)) + b
    elif t == BYTES:
        out += struct.pack(">i", len(v[1])) + bytes(v[1])
    elif t == INTS:
        out += struct.pack(f">i{len(v[1])}i", len(v[1]), *v[1])
    elif t == LONGS:
        out += struct.pack(f">i{len(v[1])}q", len(v[1]), *v[1])
    elif t == LIST:
        et, items = v.et, v.items
        out += struct.pack(">bi", et if items else END, len(items))
        for it in items:
            _put(out, et, it)
    elif t == COMPOUND:
        for k, c in v.items():
            ct = _type(c)
            out += struct.pack(">b", ct)
            b = k.encode("utf-8")
            out += struct.pack(">H", len(b)) + b
            _put(out, ct, c)
        out += b"\0"


def read_dat(path):
    raw = open(path, "rb").read()
    if raw[:2] == b"\x1f\x8b":
        raw = gzip.decompress(raw)
    r = _R(raw)
    assert r.un(">b") == COMPOUND
    r.string()
    return _payload(r, COMPOUND)


def write_dat(path, root):
    out = bytearray(b"\x0a\0\0")
    _put(out, COMPOUND, root)
    with open(path, "wb") as f:
        f.write(gzip.compress(bytes(out)))
