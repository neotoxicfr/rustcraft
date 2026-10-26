"""Performance A/B of the client on the bench (quiet machine mandatory).

    python tools/bench/ab.py [--passes 3] [--secs 25] [--from 10] [--host 127.0.0.1] [--port 25565]
                            [--name Bench] [--view "x y z yaw pitch"] [--env VAR=val]…
                            [--warmup 0|1] [--cold] VARIANT...

Each VARIANT is a list of environment variables separated by commas, or `base` for none:
`base RUSTCRAFT_SORTALL=1 RUSTCRAFT_SPLITDRAWS=1,RUSTCRAFT_ALLENTITIES=1`.
The variants are launched in rotation (A B C, then B C A…, order rotated on each pass) to
compensate for a slow drift; each launch reads the statistics lines from `--from` seconds on
(steady state) and keeps the median of the CPU, the GPU and the fps. The client quits by itself
(capture at the end of the measurement). An unmeasured warm-up launch precedes pass 1
(`--warmup 0` to skip it); a launch whose log says "cache to write" is left out (`--cold` keeps
it). Resource cache in `target/perf/cache/<exe>-<hash>` (as in `scenes.py`), logs in
`target/perf/ab`. The noise (spread of the passes) of each variant is shown with the difference
from the first: a difference within the noise does not conclude.
`--view` teleports the player over RCON (bench server folder in RUSTCRAFT_BENCH_SRV) for an
identical view from one launch to the next.
"""
import argparse
import os
import re
import statistics
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import scenes  # noqa: E402

LINE = re.compile(r"\[\s*([\d.]+) s\] (\d+) fps, CPU ([\d.]+) ms/frame, GPU ([\d.]+|\?) ms/frame")


def run(exe, env_extra, a, tmp):
    env = dict(os.environ)
    env["RUSTCRAFT_CACHE"] = scenes.seed_cache(scenes.exe_cache_dir(exe), scenes.cache_source(a))
    # Special dates frozen (comparable frames on Dec 24-26); a variant may force them.
    env.update({"RUSTCRAFT_HALLOWEEN": "0", "RUSTCRAFT_CHRISTMAS": "0"})
    for kv in a.env:
        k, v = kv.split("=", 1)
        env[k] = v
    env.update(env_extra)
    env["RUSTCRAFT_CAPTURE"] = f"{tmp}/ab.bmp;{a.secs}"
    log = os.path.join(tmp, "ab.log")
    with open(log, "w", encoding="utf-8") as out:
        p = subprocess.Popen([exe, a.host, str(a.port), a.name], env=env, stdout=out, stderr=subprocess.STDOUT)
        if a.view and os.environ.get("RUSTCRAFT_BENCH_SRV"):
            time.sleep(min(8, a.frm - 1))
            subprocess.run(
                [sys.executable, "tools/testmap/rcon.py", os.environ["RUSTCRAFT_BENCH_SRV"], f"tp {a.name} {a.view}"],
                capture_output=True,
            )
        p.wait()
    cpu, gpu, fps, cold = [], [], [], False
    with open(log, encoding="utf-8", errors="replace") as f:
        for line in f:
            c = scenes.COLD.search(line)
            if c:
                cold = c.group(1) == "to write"
            m = LINE.search(line)
            if m and float(m.group(1)) >= a.frm:
                fps.append(int(m.group(2)))
                cpu.append(float(m.group(3)))
                if m.group(4) != "?":
                    gpu.append(float(m.group(4)))
    med = lambda v: statistics.median(v) if v else float("nan")  # noqa: E731
    return med(cpu), med(gpu), med(fps), len(cpu), cold


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser()
    ap.add_argument("variants", nargs="+")
    ap.add_argument("--passes", type=int, default=3)
    ap.add_argument("--secs", type=int, default=25)
    ap.add_argument("--from", dest="frm", type=float, default=10)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=25565)
    ap.add_argument("--name", default="Bench")
    ap.add_argument("--view", default="")
    ap.add_argument("--exe", default="target/release/rustcraft.exe")
    ap.add_argument("--env", action="append", default=[], help="VAR=value added to every variant")
    ap.add_argument("--warmup", type=int, choices=[0, 1], default=1)
    ap.add_argument("--cold", action="store_true", help="keeps the cold-cache launches")
    ap.add_argument("--cache-source", default="")
    a = ap.parse_args()
    variants = []
    for v in a.variants:
        env = {} if v == "base" else dict(kv.split("=", 1) for kv in v.split(","))
        variants.append((v, env))
    results = {v: [] for v, _ in variants}
    tmp = os.path.join(scenes.PERF, "ab")
    os.makedirs(tmp, exist_ok=True)
    if a.warmup:
        r = run(a.exe, variants[0][1], a, tmp)
        print(f"warm-up: cache {'to write' if r[4] else 'valid'}", flush=True)
    for p in range(a.passes):
        for i in scenes.rotated(len(variants), p + 1):
            v, env = variants[i]
            r = run(a.exe, env, a, tmp)
            if r[4] and not a.cold:
                print(f"pass {p + 1} {v}: cold cache, left out", flush=True)
                continue
            results[v].append(r)
            print(f"pass {p + 1} {v}: CPU {r[0]:.3f} ms, GPU {r[1]:.3f} ms, {r[2]:.0f} fps ({r[3]} readings)", flush=True)
    print("\nvariant | CPU ms (min–max) | GPU ms (min–max) | fps (min–max) | fps noise | fps difference from the 1st")
    ref = [r[2] for r in results[variants[0][0]]]
    for v, _ in variants:
        rs = results[v]
        if not rs:
            print(f"{v} | no launch kept")
            continue
        col = lambda i, f: f"{min(r[i] for r in rs):{f}}–{max(r[i] for r in rs):{f}}"  # noqa: E731
        fps = [r[2] for r in rs]
        noise = scenes.noise_of(ref, fps)
        d = (statistics.median(fps) - statistics.median(ref)) / statistics.median(ref) * 100 if ref else float("nan")
        verdict = "noise unknown" if noise != noise else "within noise" if abs(d) <= noise else "above"
        print(f"{v} | {col(0, '.3f')} | {col(1, '.3f')} | {col(2, '.0f')} | {scenes.spread(fps):.1f} % | {d:+.1f} % ({verdict})")


if __name__ == "__main__":
    main()
