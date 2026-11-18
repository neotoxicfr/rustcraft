"""Compares an assembly of blocks between the Rust singleplayer and the official server, tick by tick.

Three steps (the test server is shared and may lag, nothing is frozen):

1. `python blocks_probe.py vanilla <server-folder> <script> <singleplayer-readout> <output>`:
   replays the script on the server over RCON. Each group of commands between two
   `tick`/`wait` lines is sent as one batch, framed by two `time query gametime`; the real time of
   the group is noted. During a `tick N`, each position that changes in the singleplayer readout
   (nominal) is read by trying the states it takes in singleplayer (`execute if block x y z <state>`),
   each reading framed by two time reads (same value = snapshot of a single tick). Writes
   `<output>.trace` (relative tick, position, state or `?`) and `<output>.txt`: the script with
   the real gaps between groups, to be replayed in singleplayer.
2. Replay `<output>.txt` with the singleplayer probe (`RUSTCRAFT_PROBE`).
3. `python blocks_probe.py compare <realigned-singleplayer-readout> <output>.trace`: differences.

Touches neither the time nor the server's rules. A player must stay connected (the test server
pauses without players). Reference scenarios: `scenarios-blocks/*.txt` (zone 4000..4015;
`kill` of dropped items at the top: no effect in singleplayer). A final summary (`# end`) compares
the whole region, including the cells that singleplayer never changed.
"""
import socket
import struct
import sys
import time

sys.path.insert(0, __file__.rsplit('\\', 1)[0].rsplit('/', 1)[0])
from rcon import props  # noqa: E402


class Rcon:
    def __init__(self, folder):
        p = props(folder)
        self.sock = socket.create_connection(('127.0.0.1', int(p['rcon.port'])))
        self.req = 0
        self.call(p['rcon.password'], 3)

    def call(self, body, kind=2):
        self.req += 1
        data = struct.pack('<ii', self.req, kind) + body.encode('utf-8') + b'\0\0'
        self.sock.sendall(struct.pack('<i', len(data)) + data)
        size = struct.unpack('<i', self._recv(4))[0]
        self._recv(8)
        return self._recv(size - 8)[:-2].decode('utf-8', 'replace')

    def _recv(self, n):
        buf = b''
        while len(buf) < n:
            part = self.sock.recv(n - len(buf))
            if not part:
                raise ConnectionError('RCON closed')
            buf += part
        return buf

    def gametime(self):
        # An empty reply sometimes arrives (shared server): ask again.
        for _ in range(20):
            t = self.call('time query gametime')
            if 'game time is' in t:
                return int(''.join(c for c in t if c.isdigit()))
        raise ConnectionError(f'unreadable time: {t!r}')


def load_trace(path):
    """Readout: {tick: [(pos, state)]} (deltas)."""
    deltas = {}
    for line in open(path, encoding='utf-8'):
        if line.startswith('#') or not line.strip():
            continue
        t, x, y, z, st = line.split(maxsplit=4)
        deltas.setdefault(int(t), []).append(((int(x), int(y), int(z)), st.strip()))
    return deltas


def states_at(deltas, upto):
    cur = {}
    for t in sorted(deltas):
        if t > upto:
            break
        for p, st in deltas[t]:
            cur[p] = st
    return cur


def vanilla(folder, script, solo, out):
    deltas = load_trace(solo)
    history = {}
    moving = {p for t, lst in deltas.items() if t > 0 for p, _ in lst}
    for t in sorted(deltas):
        for p, st in deltas[t]:
            if p not in moving:
                continue
            history.setdefault(p, [])
            if st not in history[p]:
                history[p].append(st)
    watched = sorted(history)
    r = Rcon(folder)
    batch = []
    real = []  # lines of the realigned script
    trace = []
    notes = []
    t_solo = 0  # singleplayer tick of the last group
    g_last = None
    last_seen = {}

    def run_batch():
        g0 = r.gametime()
        for c in batch:
            r.call(c)
        g1 = r.gametime()
        return g0, g1

    for line in open(script, encoding='utf-8'):
        line = line.rstrip('\n')
        s = line.strip()
        if not s or s.startswith('#') or s.startswith('region') or s.startswith('mark'):
            real.append(line)
            continue
        w = s.split()
        if w[0] not in ('tick', 'wait'):
            batch.append(s)
            continue
        n = int(w[1])
        g0, g1 = run_batch()
        if g0 != g1:
            notes.append(f'# group before tick {t_solo} straddles {g1 - g0 + 1} ticks: uncertain results')
        if g_last is not None:
            gap = g1 - g_last
            # The previous group is followed by `gap` real ticks.
            real.append(f'tick {gap}')
            t_solo += gap
        real.extend(batch)
        batch.clear()
        g_last = g1
        seen = set()
        end = g1 + n
        while True:
            a = r.gametime()
            if a >= end:
                break
            if w[0] != 'tick' or a - g1 < 1 or a in seen:
                continue
            res = {}
            for p in watched:
                got = '?'
                # Last seen state first: a single try most of the time.
                order = history[p]
                if p in last_seen:
                    order = [last_seen[p]] + [c for c in order if c != last_seen[p]]
                for cand in order:
                    if 'passed' in r.call(f'execute if block {p[0]} {p[1]} {p[2]} {cand}'):
                        got = cand
                        break
                res[p] = got
                last_seen[p] = got
            b = r.gametime()
            if a != b:
                continue
            seen.add(a)
            for p, st in res.items():
                trace.append(f'{t_solo + a - g1} {p[0]} {p[1]} {p[2]} {st}')
        notes.append(f'# block after tick {t_solo}: {len(seen)} ticks read out of {n}')
    real.append(f'tick {n + 2}')
    # Final summary over the whole region: catches a block that singleplayer never changed.
    final = states_at(deltas, max(deltas) if deltas else 0)
    for p, st in sorted(final.items()):
        if 'passed' not in r.call(f'execute if block {p[0]} {p[1]} {p[2]} {st}'):
            notes.append(f'# end: {p} singleplayer {st} official differs')
    open(out + '.trace', 'w', encoding='utf-8', newline='\n').write('\n'.join(notes + trace) + '\n')
    open(out + '.txt', 'w', encoding='utf-8', newline='\n').write('\n'.join(real) + '\n')
    print('\n'.join(notes))


def compare(solo, van):
    deltas = load_trace(solo)
    bad = 0
    n = 0
    for line in open(van, encoding='utf-8'):
        if line.startswith('#') or not line.strip():
            continue
        t, x, y, z, st = line.split(maxsplit=4)
        t, p, st = int(t), (int(x), int(y), int(z)), st.strip()
        exp = states_at(deltas, t).get(p, 'minecraft:air')
        n += 1
        if exp != st:
            bad += 1
            if bad <= 40:
                print(f'tick {t} {p} singleplayer {exp} official {st}')
    print(f'{n} comparisons, {bad} differences')


if __name__ == '__main__':
    if sys.argv[1] == 'vanilla':
        vanilla(*sys.argv[2:6])
    else:
        compare(*sys.argv[2:4])
