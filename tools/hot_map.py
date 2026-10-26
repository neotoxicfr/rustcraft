"""Hot map: samply profiles of several scenes merged with the code, one sheet per crate.

    python tools/hot_map.py <profiles folder> <output folder> [.map file] [--whole-profile]

Only each scene's measurement window counts (`window.json` written by `scenes.py` next to the
profile: startup, initial generation and the first meshing are left out); `--whole-profile` keeps
the whole profile (old behaviour).

The profiles folder holds one subfolder per scene (`plains12-1/profile.json.gz`, as written by
`scenes.py run --samply`). For each function of our crates: self and inclusive time (in % of the
whole process's CPU) per scene, the thread that runs it, file:line of its definition, main callers
and callees weighted by time. Output: `INDEX.md` (crates and threads per scene) and one `<crate>.md`
sheet per crate, functions ranked by their worst inclusive share.

Names: an executable linked with `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` gives full names
in the .pdb; otherwise pass the linker's `/MAP` map (v0 names read roughly).
"""
import bisect
import collections
import gzip
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import window  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATE_DIR = {'rustcraft': 'rc-app'}
OURS = re.compile(r'\b(rc_[a-z0-9_]+|rustcraft)::')


def load_map(path):
    rows = []
    for line in open(path, encoding='utf-8', errors='replace'):
        f = line.split()
        if len(f) >= 4 and ':' in f[0] and len(f[2]) == 16:
            try:
                rows.append((int(f[2], 16) - 0x140000000, f[1]))
            except ValueError:
                pass
    rows.sort()
    return [r[0] for r in rows], rows




def v0_rough(name):
    """Rough reading of a Rust v0 name (`_R…`): identifiers in order, without the generic
    arguments. Enough for the crate, the module and the function; the full .pdb is preferable."""
    s = re.sub(r'\.llvm\.\d+$', '', name)
    if '_R' not in s:
        return name
    s = s[s.index('_R'):]
    out, i = [], 2
    while i < len(s):
        c = s[i]
        if c == 'I' and out:  # start of the generic arguments: the main path is complete
            break
        if c == 'C' and i + 1 < len(s) and s[i + 1] == 's':  # crate root: Cs<hash>_
            if out:  # a second root opens a generic argument: the path is complete
                break
            j = s.find('_', i)
            i = j + 1 if j > 0 else i + 1
            continue
        if c == 's':  # disambiguator s<base62>_
            m = re.match(r's[0-9A-Za-z]*_', s[i:])
            if m:
                i += m.end()
                continue
        if c.isdigit():
            j = i
            while j < len(s) and s[j].isdigit():
                j += 1
            n = int(s[i:j])
            if j < len(s) and s[j] == '_':
                j += 1
            ident = s[j:j + n]
            if ident and (ident[0].isalpha() or ident[0] == '_'):
                out.append(ident)
            i = j + n
            continue
        i += 1
    return '::'.join(out) if out else name


def clean(name):
    name = v0_rough(name)
    name = re.sub(r'::\{\{closure\}\}|::closure\$\d+|::impl\$\d+|\$\w+', '', name)
    name = re.sub(r'<[^<>]*>', '', re.sub(r'<[^<>]*>', '', name))
    return name.strip('<> ')


def ours(name):
    m = OURS.search(name)
    return m.group(1) if m else None


def load(path, mapped):
    prof = json.load(gzip.open(path))
    syms = json.load(open(path.replace('.json.gz', '.json.syms.json'), encoding='utf-8'))
    names = syms['string_table']
    tables = {}
    for lib in syms['data']:
        rows = sorted((r['rva'], r['size'], names[r['symbol']]) for r in lib['symbol_table'])
        tables[lib['debug_name']] = ([r[0] for r in rows], rows)
    libs = [l['debugName'] for l in prof['libs']]

    def symbol(lib, addr):
        if mapped and lib.lower().startswith('rustcraft'):
            i = bisect.bisect_right(mapped[0], addr) - 1
            if i >= 0 and addr - mapped[0][i] < 0x100000:
                return mapped[1][i][1]
        t = tables.get(lib)
        if t:
            i = bisect.bisect_right(t[0], addr) - 1
            if i >= 0 and addr < t[1][i][0] + max(t[1][i][1], 1):
                return t[1][i][2]
        return f'{lib}+{addr:#x}'

    return prof, libs, symbol


def scene_stats(path, mapped, everything=False):
    prof, libs, symbol = load(path, mapped)
    win = window.load(path, everything)
    self_t, incl = collections.Counter(), collections.Counter()
    callers, callees = collections.defaultdict(collections.Counter), collections.defaultdict(collections.Counter)
    thread_of = collections.defaultdict(collections.Counter)
    by_thread = collections.Counter()
    total = 0
    for th in prof['threads']:
        tname = th['name'].rstrip('0123456789-_ ') or '?'
        st, ft, fn, rt = th['stackTable'], th['frameTable'], th['funcTable'], th['resourceTable']
        fname = []
        for f in range(ft['length']):
            res = fn['resource'][ft['func'][f]]
            lib = libs[rt['lib'][res]] if res is not None and rt['lib'][res] is not None else '?'
            fname.append(clean(symbol(lib, ft['address'][f])))
        for s, w in zip(th['samples']['stack'], window.weights(th, win)):
            w = w or 0
            if s is None or w == 0:
                continue
            total += w
            by_thread[tname] += w
            chain = []
            while s is not None:
                chain.append(fname[st['frame'][s]])
                s = st['prefix'][s]
            self_t[chain[0]] += w
            seen = set()
            prev = None
            for name in chain:  # from the leaf to the root
                if name == prev:
                    continue
                if name not in seen:
                    incl[name] += w
                    thread_of[name][tname] += w
                    seen.add(name)
                if prev is not None:
                    callers[prev][name] += w
                    callees[name][prev] += w
                prev = name
    return {'total': total or 1, 'self': self_t, 'incl': incl, 'callers': callers,
            'callees': callees, 'thread': thread_of, 'by_thread': by_thread}


_defs = {}


def definition(crate, name):
    """file:line of `fn <last segment>`, preferring the file of the module path."""
    parts = [p for p in name.split('::') if p]
    if crate not in _defs:
        idx = collections.defaultdict(list)
        src = os.path.join(ROOT, 'crates', CRATE_DIR.get(crate, crate.replace('_', '-')), 'src')
        for d, _, files in os.walk(src):
            for f in files:
                if f.endswith('.rs'):
                    p = os.path.join(d, f)
                    rel = os.path.relpath(p, ROOT).replace('\\', '/')
                    for i, line in enumerate(open(p, encoding='utf-8', errors='replace'), 1):
                        m = re.match(r'\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)', line)
                        if m:
                            idx[m.group(1)].append(f'{rel}:{i}')
        _defs[crate] = idx
    cands = _defs[crate].get(parts[-1], []) if parts else []
    if len(cands) > 1:
        mods = [p for p in parts[1:-1] if p.islower()]
        best = [c for c in cands if all(m in c for m in mods[-1:])]
        cands = best or cands
    return cands[0] + (f' (+{len(cands) - 1})' if len(cands) > 1 else '') if cands else '?'


def pct(x, total):
    return 100.0 * x / total


def main():
    argv, everything = window.strip_flag(sys.argv)
    src, out = argv[1], argv[2]
    mapped = load_map(argv[3]) if len(argv) > 3 else None
    os.makedirs(out, exist_ok=True)
    scenes = {}
    for d in sorted(os.listdir(src)):
        p = os.path.join(src, d, 'profile.json.gz')
        if os.path.exists(p):
            print('profile', d, file=sys.stderr)
            scenes[d] = scene_stats(p, mapped, everything)
    names = list(scenes)

    crates = collections.defaultdict(set)
    for sc in scenes.values():
        for f in sc['incl']:
            c = ours(f)
            if c:
                crates[c].add(f)

    idx = ['# Hot map', '', f'Scenes: {", ".join(names)}. Shares in % of the whole process\'s CPU.', '',
           '## Threads', '', '| thread | ' + ' | '.join(names) + ' |', '|---|' + '---|' * len(names)]
    threads = collections.Counter()
    for sc in scenes.values():
        for t, w in sc['by_thread'].items():
            threads[t] += pct(w, sc['total'])
    for t, _ in threads.most_common(15):
        idx.append(f'| {t} | ' + ' | '.join(f"{pct(scenes[n]['by_thread'][t], scenes[n]['total']):.1f}" for n in names) + ' |')
    idx += ['', '## Crates (summed self time of their functions)', '',
            '| crate | ' + ' | '.join(names) + ' | sheet |', '|---|' + '---|' * (len(names) + 1)]
    rows = []
    for c, fs in crates.items():
        vals = [sum(pct(scenes[n]['self'][f], scenes[n]['total']) for f in fs) for n in names]
        rows.append((max(vals), c, vals))
    for _, c, vals in sorted(rows, reverse=True):
        idx.append(f'| {c} | ' + ' | '.join(f'{v:.1f}' for v in vals) + f' | [{c}.md]({c}.md) |')
    open(os.path.join(out, 'INDEX.md'), 'w', encoding='utf-8').write('\n'.join(idx) + '\n')

    for c, fs in crates.items():
        ranked = sorted(fs, key=lambda f: -max(pct(scenes[n]['incl'][f], scenes[n]['total']) for n in names))
        lines = [f'# {c}: hot functions', '',
                 'Per scene: self / inclusive (% of the process\'s CPU). Callers and callees: share of the function\'s inclusive time.', '']
        for f in ranked:
            worst = max(pct(scenes[n]['incl'][f], scenes[n]['total']) for n in names)
            if worst < 0.05:
                break
            thr = collections.Counter()
            cal, cee = collections.Counter(), collections.Counter()
            tot = 0
            for n in names:
                sc = scenes[n]
                thr.update(sc['thread'][f])
                cal.update(sc['callers'][f])
                cee.update(sc['callees'][f])
                tot += sc['incl'][f]
            tot = tot or 1
            cells = ', '.join(f"{n} {pct(scenes[n]['self'][f], scenes[n]['total']):.2f}/{pct(scenes[n]['incl'][f], scenes[n]['total']):.2f}" for n in names)
            lines.append(f'## `{f}`')
            lines.append(f'- definition: {definition(c, f)}')
            lines.append(f'- threads: ' + ', '.join(f'{t} {100 * w / sum(thr.values()):.0f} %' for t, w in thr.most_common(3)))
            lines.append(f'- scenes: {cells}')
            lines.append('- called by: ' + '; '.join(f'`{k}` {100 * w / tot:.0f} %' for k, w in cal.most_common(3)))
            lines.append('- calls: ' + '; '.join(f'`{k}` {100 * w / tot:.0f} %' for k, w in cee.most_common(4)))
            lines.append('')
        open(os.path.join(out, f'{c}.md'), 'w', encoding='utf-8').write('\n'.join(lines))
    print(f'{len(crates)} crates -> {out}', file=sys.stderr)


if __name__ == '__main__':
    main()
