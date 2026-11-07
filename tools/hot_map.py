"""Hot map: samply profiles of several scenes merged with the code, one sheet per crate.

    python tools/hot_map.py <profiles folder> <output folder> [.pdb or .map file] [--whole-profile]
                            [--breakdown <function>]...

Only each scene's measurement window counts (`window.json` written by `scenes.py` next to the
profile: startup, initial generation and the first meshing are left out); `--whole-profile` keeps
the whole profile (old behaviour).

The profiles folder holds one subfolder per scene (`plains12-1/profile.json.gz`, as written by
`scenes.py run --samply`). For each function of our crates and of the GPU stack under them (wgpu,
wgpu-core, wgpu-hal, and the GPU driver modules, whose functions keep samply's `fun_…` names but are
grouped under their module): self and inclusive time (in % of the whole process's CPU) per scene,
the thread that runs it, file:line of its definition, main callers and callees weighted by time.
Output: `INDEX.md` (crates and threads per scene) and one `<crate>.md` sheet per crate, functions
ranked by their worst inclusive share. Each `--breakdown <function>` (full cleaned name, as the
sheets print it) adds to `BREAKDOWN.md` the call paths under that function, three levels deep, in %
of the process per scene: where the time of a wgpu entry point goes (submission, encoder finish).

Names: samply leaves our functions unnamed (`fun_33ec0`) even with the matching .pdb next to the
exe, so pass the profiled exe's .pdb (the release profile writes one: `debug = "line-tables-only"`;
keep it with the exe when copying it) or the linker's `/MAP` map (v0 names read roughly).
"""
import bisect
import collections
import gzip
import json
import os
import re
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import window  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# GPU driver modules (user and kernel side) and the Windows graphics kernel: their functions have no
# names, so each frame of theirs is named `<module>::fun_…` and the module gets a sheet like a crate
# (which of our or wgpu's calls lead into the driver, and how much time they cost there).
GPU_MODULES = ('nvoglv64', 'nvwgf2umx', 'nvd3dumx', 'nvlddmkm', 'amdvlk64', 'amdxc64', 'atikmdag',
               'igvk64', 'igd12umd64', 'igdkmdn64', 'dxgkrnl', 'dxgmms2')
# Our crates, then the wgpu layers (longest names first: `wgpu_core::` is not `wgpu::`), then the
# driver modules.
OURS = re.compile(r'\b(rc_[a-z0-9_]+|rustcraft|wgpu_core|wgpu_hal|wgpu|' + '|'.join(GPU_MODULES) + r')::')
BREAKDOWN_DEPTH = 3


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


PROC_RECORDS = {0x110F, 0x1110, 0x1146, 0x1147}  # local/global procedure symbols, plain and _ID


def load_pdb(path):
    """Function names and addresses read from the procedure symbols of every module of a .pdb
    (MSF container: superblock, stream directory, DBI stream 3, section headers stream)."""
    f = open(path, 'rb')
    block_size, _, _, dir_bytes, _, dir_map = struct.unpack_from('<IIIIII', f.read(56), 32)

    def block(n):
        f.seek(n * block_size)
        return f.read(block_size)

    dir_blocks = struct.unpack_from(f'<{-(-dir_bytes // block_size)}I', block(dir_map))
    directory = b''.join(block(n) for n in dir_blocks)[:dir_bytes]
    count = struct.unpack_from('<I', directory)[0]
    sizes = struct.unpack_from(f'<{count}I', directory, 4)
    pos, streams = 4 + 4 * count, []
    for size in sizes:
        n = 0 if size == 0xFFFFFFFF else -(-size // block_size)
        streams.append((struct.unpack_from(f'<{n}I', directory, pos), size))
        pos += 4 * n

    def stream(i):
        blocks, size = streams[i]
        return b''.join(block(n) for n in blocks)[:size]

    dbi = stream(3)
    mod_size, contrib_size, map_size, files_size, server_size, _, _, ec_size = struct.unpack_from('<iiiiiIii', dbi, 24)
    # Entry 5 of the optional debug header: the stream of section headers (address of each segment).
    debug_header = 64 + mod_size + contrib_size + map_size + files_size + server_size + ec_size
    headers = stream(struct.unpack_from('<H', dbi, debug_header + 10)[0])
    segment_rva = [struct.unpack_from('<I', headers, i * 40 + 12)[0] for i in range(len(headers) // 40)]
    rows, pos = [], 64
    while pos < 64 + mod_size:
        sym_stream, sym_bytes = struct.unpack_from('<HI', dbi, pos + 34)
        end = dbi.index(b'\0', dbi.index(b'\0', pos + 64) + 1) + 1  # module and object names
        pos = (end + 3) & ~3
        if sym_stream == 0xFFFF:
            continue
        syms, q = stream(sym_stream)[:sym_bytes], 4
        while q + 4 <= len(syms):
            length, kind = struct.unpack_from('<HH', syms, q)
            if kind in PROC_RECORDS:
                offset, segment = struct.unpack_from('<IH', syms, q + 32)
                name = syms[q + 39:syms.index(b'\0', q + 39)].decode('utf-8', 'replace')
                if 1 <= segment <= len(segment_rva):
                    rows.append((segment_rva[segment - 1] + offset, name))
            q += 2 + length
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


def gpu_module(lib):
    """Driver or graphics kernel module of a library name (`nvoglv64.pdb` -> `nvoglv64`), or None."""
    stem = os.path.splitext(os.path.basename(lib))[0].lower()
    return stem if stem in GPU_MODULES else None


def frame_name(lib, raw):
    """Cleaned name of a frame; a driver frame is prefixed with its module."""
    name = clean(raw)
    module = gpu_module(lib)
    return f'{module}::{name}' if module else name


def breakdown_path(chain, root):
    """Call path under `root` in a sample's chain (leaf first): the `BREAKDOWN_DEPTH` functions it
    calls, outermost first, repeated names (recursion) merged; `()` if the sample is in `root` itself.
    The outermost occurrence of `root` counts, so a recursive root is counted once per sample."""
    i = len(chain) - 1 - chain[::-1].index(root)
    path = []
    for name in reversed(chain[:i]):
        if not path or path[-1] != name:
            path.append(name)
        if len(path) == BREAKDOWN_DEPTH:
            break
    return tuple(path)


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


def scene_stats(path, mapped, everything=False, roots=()):
    prof, libs, symbol = load(path, mapped)
    win = window.load(path, everything)
    self_t, incl = collections.Counter(), collections.Counter()
    callers, callees = collections.defaultdict(collections.Counter), collections.defaultdict(collections.Counter)
    thread_of = collections.defaultdict(collections.Counter)
    by_thread = collections.Counter()
    # Per `--breakdown` root: weight of each call path under it.
    paths = {r: collections.Counter() for r in roots}
    total = 0
    for th in prof['threads']:
        tname = th['name'].rstrip('0123456789-_ ') or '?'
        st, ft, fn, rt = th['stackTable'], th['frameTable'], th['funcTable'], th['resourceTable']
        fname = []
        for f in range(ft['length']):
            res = fn['resource'][ft['func'][f]]
            lib = libs[rt['lib'][res]] if res is not None and rt['lib'][res] is not None else '?'
            fname.append(frame_name(lib, symbol(lib, ft['address'][f])))
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
            for r in roots:
                if r in seen:
                    paths[r][breakdown_path(chain, r)] += w
    return {'total': total or 1, 'self': self_t, 'incl': incl, 'callers': callers,
            'callees': callees, 'thread': thread_of, 'by_thread': by_thread, 'paths': paths}


_defs = {}


def locked_version(package):
    """Version of `package` in the repository's Cargo.lock, or None."""
    try:
        lock = open(os.path.join(ROOT, 'Cargo.lock'), encoding='utf-8').read()
    except OSError:
        return None
    m = re.search(r'^name = "' + re.escape(package) + r'"\nversion = "([^"]+)"', lock, re.M)
    return m.group(1) if m else None


def crate_source(crate):
    """Source folder of a crate: the client (`rustcraft`, apps/client) and our crates in the
    repository, the wgpu layers in the cargo registry at the version of Cargo.lock; None for a
    driver module (no source)."""
    if crate == 'rustcraft':
        return os.path.join(ROOT, 'apps', 'client', 'src')
    if crate.startswith('rc_'):
        return os.path.join(ROOT, 'crates', crate.replace('_', '-'), 'src')
    package = crate.replace('_', '-')
    version = locked_version(package) if crate.startswith('wgpu') else None
    if not version:
        return None
    registry = os.path.join(os.environ.get('CARGO_HOME') or os.path.join(os.path.expanduser('~'), '.cargo'),
                            'registry', 'src')
    for index in sorted(os.listdir(registry)) if os.path.isdir(registry) else []:
        src = os.path.join(registry, index, f'{package}-{version}', 'src')
        if os.path.isdir(src):
            return src
    return None


def definition(crate, name):
    """file:line of `fn <last segment>`, preferring the file of the module path (relative to the
    repository, or to the crate's registry folder for wgpu)."""
    parts = [p for p in name.split('::') if p]
    if crate not in _defs:
        idx = collections.defaultdict(list)
        src = crate_source(crate)
        # Registry crates: path from the registry index (`wgpu-core-30.0.1/src/…`).
        base = ROOT if src and src.startswith(ROOT) else os.path.dirname(os.path.dirname(src or ''))
        for d, _, files in os.walk(src) if src else []:
            for f in files:
                if f.endswith('.rs'):
                    p = os.path.join(d, f)
                    rel = os.path.relpath(p, base).replace('\\', '/')
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


def strip_breakdowns(argv):
    """Removes the `--breakdown <function>` pairs; returns (remaining arguments, functions)."""
    rest, roots, i = [], [], 0
    while i < len(argv):
        if argv[i] == '--breakdown' and i + 1 < len(argv):
            roots.append(argv[i + 1])
            i += 2
        else:
            rest.append(argv[i])
            i += 1
    return rest, roots


def breakdown_lines(scenes, names, roots, rows=30):
    """`BREAKDOWN.md`: for each root, its share of the process then the heaviest call paths under it."""
    lines = ['# Breakdown', '',
             f'Call paths under each function ({BREAKDOWN_DEPTH} levels, outermost call first), in % of the whole '
             'process\'s CPU per scene; `(self)`: samples in the function itself.', '']
    for r in roots:
        lines += [f'## `{r}`', '', '| path | ' + ' | '.join(names) + ' |', '|---|' + '---|' * len(names)]
        lines.append('| **total** | ' + ' | '.join(
            f"{pct(sum(scenes[n]['paths'][r].values()), scenes[n]['total']):.2f}" for n in names) + ' |')
        every = collections.Counter()
        for n in names:
            for p, w in scenes[n]['paths'][r].items():
                every[p] = max(every[p], pct(w, scenes[n]['total']))
        for p, _ in every.most_common(rows):
            label = ' > '.join(f'`{f}`' for f in p) or '(self)'
            lines.append(f'| {label} | ' + ' | '.join(
                f"{pct(scenes[n]['paths'][r][p], scenes[n]['total']):.2f}" for n in names) + ' |')
        lines.append('')
    return lines


def main():
    argv, everything = window.strip_flag(sys.argv)
    argv, roots = strip_breakdowns(argv)
    src, out = argv[1], argv[2]
    names = argv[3] if len(argv) > 3 else None
    mapped = (load_pdb if names.lower().endswith('.pdb') else load_map)(names) if names else None
    os.makedirs(out, exist_ok=True)
    scenes = {}
    for d in sorted(os.listdir(src)):
        p = os.path.join(src, d, 'profile.json.gz')
        if os.path.exists(p):
            print('profile', d, file=sys.stderr)
            scenes[d] = scene_stats(p, mapped, everything, roots)
    names = list(scenes)
    if roots:
        open(os.path.join(out, 'BREAKDOWN.md'), 'w', encoding='utf-8').write(
            '\n'.join(breakdown_lines(scenes, names, roots)) + '\n')

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
    idx += ['', '## Crates and GPU stack (summed self time of their functions)', '',
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
