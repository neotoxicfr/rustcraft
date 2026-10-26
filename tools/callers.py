"""Callers of a function in a samply profile, weighted by CPU time:

    python tools/callers.py profile.json.gz pattern [.map file] [depth]

For the samples whose stack contains a function whose name contains `pattern`, prints the most
frequent caller chains (from the nearest to the farthest).
"""
import collections
import os
import sys

sys.argv, args = [sys.argv[0], sys.argv[1], '1'] + sys.argv[3:4], sys.argv
# Reuses the loading and symbolication of cpu_profile.py.
src = open(os.path.join(os.path.dirname(__file__), 'cpu_profile.py'), encoding='utf-8').read()
ns = {'__file__': __file__}
exec(src.split('self_t, incl = ')[0], ns)
pattern = args[2]
depth = int(args[4]) if len(args) > 4 else 6
st, frame_name, th = ns['st'], ns['frame_name'], ns['th']
deltas = th['samples'].get('threadCPUDelta') or [1] * th['samples']['length']
chains = collections.Counter()
total = 0
for s, w in zip(th['samples']['stack'], deltas):
    w = w or 0
    if s is None or w == 0:
        continue
    names = []
    while s is not None:
        names.append(frame_name[st['frame'][s]])
        s = st['prefix'][s]
    for i, n in enumerate(names):
        if pattern in n:
            chains[' ← '.join(x[:60] for x in names[i:i + depth])] += w
            total += w
            break
for c, w in chains.most_common(8):
    print(f'{w / 1000:8.1f} ms  {c}')
print(f'total {total / 1000:.1f} ms')
