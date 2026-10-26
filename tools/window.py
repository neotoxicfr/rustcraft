"""Measurement window of a samply profile (shared by `cpu_profile.py` and `hot_map.py`).

`scenes.py run --samply` writes `window.json` ({"from": s, "to": s}, seconds since the client
started, like the `[perf]` readings) next to `profile.json.gz`. Samples outside that window (startup:
generation, first meshing, opening the regions) are discarded; `--whole-profile` (or a missing file)
keeps the whole profile.
"""
import json
import os


def load(profile_path, everything=False):
    """(start, end) in ms since the process started, or None (whole profile)."""
    if everything:
        return None
    p = os.path.join(os.path.dirname(os.path.abspath(profile_path)), 'window.json')
    if not os.path.exists(p):
        return None
    w = json.load(open(p, encoding='utf-8'))
    return float(w['from']) * 1e3, float(w['to']) * 1e3


def weights(thread, win):
    """(CPU) weight of each sample of the thread, set to 0 outside the window `win`."""
    s = thread['samples']
    deltas = s.get('threadCPUDelta') or [1] * s['length']
    if win is None:
        return deltas
    times = s.get('time')
    if times is None:  # time-delta format
        acc, times = 0.0, []
        for d in s.get('timeDeltas', []):
            acc += d
            times.append(acc)
    t0 = thread.get('processStartupTime', 0.0)
    lo, hi = win
    return [w if lo <= t - t0 <= hi else 0 for w, t in zip(deltas, times)]


def strip_flag(argv):
    """Removes `--whole-profile` from the arguments; returns (remaining arguments, flag present)."""
    rest = [a for a in argv if a != '--whole-profile']
    return rest, len(rest) != len(argv)
