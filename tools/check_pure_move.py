#!/usr/bin/env python3
"""Checks that a refactoring only MOVED Rust items between files.

    python -I tools/check_pure_move.py <base-rev> <old-file> <new-file>... [--rename old=new] [--ignore name]

`old-file` is read at `base-rev` (git show); the new files are read from the working tree
(the old file itself, slimmed down, belongs in that list). Every item of the old file (function,
struct, constant, impl method, test...) must exist exactly as many times in the new files with the
same code: visibility keywords (`pub(crate)`) and doc comments (`///`) are ignored, everything else
(statement order included) must be identical. Exit status 1 if an item is missing or changed.

`--rename old=new` accepts an item that kept its body under another name (for example `main=run`).
`--ignore name` skips an item that was restructured on purpose (it is listed, to be read by eye).
Items only present in the new files are listed (wrappers, new tests...) for a human to read.
"""

import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

CLOSERS = {"}", "};", "];", ");"}
VISIBILITY = r"(?:pub(?:\([a-z]+\))?\s+)?"
KEYWORDS = VISIBILITY + r"(?:unsafe\s+)?(?:async\s+)?(?:fn|struct|enum|trait|type|const|static|mod|impl|use)\b"


def indent_of(line):
    return len(line) - len(line.lstrip(" "))


def normalize(lines):
    """Item text without doc comments and visibility keywords."""
    out = []
    for ln in lines:
        if ln.lstrip().startswith("///"):
            continue
        out.append(re.sub(r"^(\s*)pub\((?:crate|super)\) ", r"\1", ln))
    return "\n".join(out)


def item_name(sig):
    m = re.search(r"\b(?:fn|struct|enum|trait|type|const|static|mod)\s+(\w+)", sig)
    return m.group(1) if m else sig.strip()


def parse(lines, indent=0, container=""):
    """Items of a block of lines: (container, name, kind, text) for every item and every child of an
    `impl` or inline `mod` block (the block wrapper itself is not an item)."""
    items = []
    i, n = 0, len(lines)
    while i < n:
        ln = lines[i]
        if not ln.strip() or indent_of(ln) != indent:
            i += 1
            continue
        head = i
        while i < n and (lines[i].startswith(" " * indent + "///") or lines[i].startswith(" " * indent + "#[")):
            i += 1
        if i >= n:
            break
        if not re.match(r" *" + KEYWORDS, lines[i]):
            i += 1  # plain comment, statement...
            continue
        sig = lines[i]
        j = i
        if not (sig.rstrip().endswith(";") or sig.rstrip().endswith("}")):
            j = next(k for k in range(i + 1, n) if indent_of(lines[k]) == indent and lines[k].strip() in CLOSERS)
        text = lines[head : j + 1]
        stripped = sig.strip()
        kind = re.match(VISIBILITY + r"(?:unsafe\s+)?(?:async\s+)?(\w+)", stripped).group(1)
        if kind in ("impl", "mod") and sig.rstrip().endswith("{"):
            name = stripped.rstrip("{ ").removeprefix("pub ") if kind == "impl" else "mod"
            items += parse(lines[i + 1 : j], indent + 4, name)
        else:
            items.append((container, item_name(sig), kind, normalize(text)))
        i = j + 1
    return items


def collect(texts):
    found = defaultdict(list)
    for text in texts:
        for container, name, kind, body in parse(text.split("\n")):
            found[(container, name, kind)].append(body)
    return found


def main(argv):
    renames, ignored = {}, set()
    args = []
    k = 0
    while k < len(argv):
        if argv[k] == "--rename":
            old_name, new_name = argv[k + 1].split("=")
            renames[old_name] = new_name
            k += 2
        elif argv[k] == "--ignore":  # restructured by hand: the reviewer reads it
            ignored.add(argv[k + 1])
            k += 2
        else:
            args.append(argv[k])
            k += 1
    base, old_path, new_paths = args[0], args[1], args[2:]
    old_text = subprocess.run(
        ["git", "show", f"{base}:{old_path}"], capture_output=True, text=True, encoding="utf-8", check=True
    ).stdout
    old = collect([old_text])
    new = collect([Path(p).read_text(encoding="utf-8") for p in new_paths])
    plumbing = ("use", "mod")  # imports and module declarations legitimately change
    failures, same = [], 0
    for (container, name, kind), bodies in old.items():
        if kind in plumbing or name in ignored:
            continue
        new_name = renames.get(name, name) if container == "" else name
        got = new.get((container, new_name, kind), [])
        want = bodies
        if new_name != name:
            want = [b.replace(f"fn {name}(", f"fn {new_name}(", 1) for b in bodies]
        if Counter(got) == Counter(want):
            same += 1
        else:
            failures.append((container, name, len(want), len(got)))
    added = [key for key in new if key not in {(c, renames.get(nm, nm) if c == "" else nm, kd) for c, nm, kd in old}]
    print(f"{same} items moved unchanged; {len(failures)} missing or changed")
    for container, name, want, got in failures:
        why = "the code differs" if want == got else f"expected {want} copy(ies), found {got}"
        print(f"  FAIL {container or '<root>'}::{name}: {why}")
    for container, name, kind in sorted(added):
        if kind not in plumbing:
            print(f"  new item {container or '<root>'}::{name} ({kind})")
    for name in sorted(ignored):
        print(f"  ignored (check by eye): {name}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
