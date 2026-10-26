"""Text summary of a samply profile (`samply record --save-only --unstable-presymbolicate`):
CPU split by thread, then self time and inclusive time per function, on the busiest thread, or
`PROFIL_FIL=<index>`, `PROFIL_FIL=<start of name>` (all the threads with that name) or
`PROFIL_FIL=all` (the whole process).

    python tools/cpu_profile.py profile.json.gz [number of lines] [.map file] [--whole-profile]

Only the scene's measurement window counts (`window.json` written by `tools/bench/scenes.py` next
to the profile); `--whole-profile` keeps the startup too.
"""
import bisect
import collections
import gzip
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import window  # noqa: E402

argv, everything = window.strip_flag(sys.argv)
path = argv[1]
top = int(argv[2]) if len(argv) > 2 else 25
win = window.load(path, everything)
prof = json.load(gzip.open(path))
for t in prof['threads']:
    t['_w'] = window.weights(t, win)
syms_path = path.replace('.json.gz', '.json.syms.json')
# Symbols of an older recording with the same name: wrong names outside our code.
if os.path.getmtime(syms_path) < os.path.getmtime(path) - 60:
    sys.exit(f'{syms_path} is older than the profile: record with --unstable-presymbolicate')
syms = json.load(open(syms_path, encoding='utf-8'))
names = syms['string_table']
tables = {}
for lib in syms['data']:
    rows = sorted((r['rva'], r['size'], names[r['symbol']]) for r in lib['symbol_table'])
    tables[lib['debug_name']] = ([r[0] for r in rows], rows)


# .map file of the MSVC linker (`-C link-arg=/MAP:<file>`): names of our functions when the .pdb
# only gives addresses (rva = "Rva+Base" column minus the image base).
mapped = None
if len(argv) > 3:
    rows = []
    for line in open(argv[3], encoding='utf-8', errors='replace'):
        f = line.split()
        if len(f) >= 4 and ':' in f[0] and len(f[2]) == 16:
            try:
                rows.append((int(f[2], 16) - 0x140000000, f[1]))
            except ValueError:
                pass
    rows.sort()
    mapped = ([r[0] for r in rows], rows)


def demangle(name):
    """Approximate demangling of Rust's legacy scheme (_ZN…E)."""
    if not name.startswith('_ZN'):
        return name
    s, i, parts = name[3:], 0, []
    while i < len(s) and s[i].isdigit():
        j = i
        while s[j].isdigit():
            j += 1
        n = int(s[i:j])
        parts.append(s[j:j + n])
        i = j + n
    if parts and parts[-1].startswith('h') and len(parts[-1]) == 17:
        parts.pop()
    return '::'.join(parts).replace('$LT$', '<').replace('$GT$', '>').replace('$u20$', ' ').replace('..', '::')


def symbol(lib, addr):
    if mapped and not lib.startswith(('nt', 'kernel')):
        i = bisect.bisect_right(mapped[0], addr) - 1
        if i >= 0 and addr - mapped[0][i] < 0x100000:
            return demangle(mapped[1][i][1])
    t = tables.get(lib)
    if not t:
        return f'{lib}+{addr:#x}'
    i = bisect.bisect_right(t[0], addr) - 1
    if i >= 0 and addr < t[1][i][0] + max(t[1][i][1], 1):
        return t[1][i][2]
    return f'{lib}+{addr:#x}'


thread_sel = os.environ.get('PROFIL_FIL')  # thread index, `all` = every thread (otherwise the busiest)
def cpu_of(t):
    return sum(x or 0 for x in t['_w'])


libs = [l['debugName'] for l in prof['libs']]
total_cpu = sum(cpu_of(t) for t in prof['threads']) or 1
# CPU split by thread (the pool's threads grouped by name).
by_name = collections.Counter()
for t in prof['threads']:
    by_name[t['name'].rstrip('0123456789')] += cpu_of(t)
print('--- CPU per thread ---')
for name, c in by_name.most_common(12):
    print(f'{100 * c / total_cpu:5.1f} %  {name}')

if thread_sel == 'all':
    threads = prof['threads']
elif thread_sel and not thread_sel.isdigit():
    # Thread name (or start of a name): all the threads that carry it (meshing pool, server…).
    threads = [t for t in prof['threads'] if t['name'].startswith(thread_sel)]
else:
    # By default, the thread that used the most CPU (idle samples do not count).
    threads = [prof['threads'][int(thread_sel)] if thread_sel else max(prof['threads'], key=cpu_of)]

self_t, incl = collections.Counter(), collections.Counter()
n = 0
for th in threads:
    st, ft, fn, rt = th['stackTable'], th['frameTable'], th['funcTable'], th['resourceTable']
    frame_name = []
    for f in range(ft['length']):
        res = fn['resource'][ft['func'][f]]
        lib = libs[rt['lib'][res]] if res is not None and rt['lib'][res] is not None else '?'
        frame_name.append(symbol(lib, ft['address'][f]))
    for s, w in zip(th['samples']['stack'], th['_w']):
        w = w or 0
        if s is None or w == 0:
            continue
        n += w
        self_t[frame_name[st['frame'][s]]] += w
        seen = set()
        while s is not None:
            name = frame_name[st['frame'][s]]
            if name not in seen:
                incl[name] += w
                seen.add(name)
            s = st['prefix'][s]
n = n or 1
th = {'name': 'all threads' if thread_sel == 'all' else f"{threads[0]['name'] if threads else '?'} ({len(threads)} thread(s))"}

short = lambda x: (x[:110] + '…') if len(x) > 111 else x
print(f'{n} µs CPU ({th["name"]}); ' + (f'window {win[0] / 1e3:.0f}-{win[1] / 1e3:.0f} s' if win else 'whole profile'))
print('--- self time ---')
for name, c in self_t.most_common(top):
    print(f'{100 * c / n:5.1f} %  {short(name)}')
print('--- inclusive time ---')
for name, c in incl.most_common(top):
    print(f'{100 * c / n:5.1f} %  {short(name)}')
