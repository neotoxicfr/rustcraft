"""Summary of a RUSTCRAFT_TRACE trace, segment by segment (one segment per teleport):
heights, mean horizontal speed, vertical speed, successive peaks (bounces)."""
import math
import sys

segs, cur = [], None
for line in open(sys.argv[1], encoding='utf-8'):
    f = line.split()
    if f and f[0] == 'tp':
        cur = {'tp': tuple(map(float, f[1:4])), 'ticks': []}
        segs.append(cur)
    elif cur is not None and len(f) == 7:  # the last line may be truncated
        cur['ticks'].append(tuple(map(float, f)))

for s in segs:
    t = s['ticks']
    if not t:
        continue
    hs = sum(math.hypot(a[3], a[5]) for a in t) / len(t) * 20
    ys = [a[1] for a in t]
    print(f"tp {s['tp']}  {len(t)} ticks  y {min(ys):.3f}..{max(ys):.3f}  v_h avg {hs:.2f} m/s  "
          f"vy {min(a[4] for a in t):.3f}..{max(a[4] for a in t):.3f}")
    tops = [round(b[1], 3) for a, b in zip(t, t[1:]) if a[4] > 0 >= b[4]]
    if tops:
        print('   peaks', tops[:8])
