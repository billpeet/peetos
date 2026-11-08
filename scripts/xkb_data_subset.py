#!/usr/bin/env python3
"""Copy the part of an installed xkeyboard-config tree needed for a set of
layouts and options into a smaller tree for the PeetOS ext2 image.

Starts from what the `evdev` rules pick for the pc105 model (keycodes evdev +
aliases, types/compat complete, symbols pc + inet + each layout) plus the
symbols named by the requested options in the rules file, then follows every
`include`/`augment`/`override`/`replace` statement transitively.

The result is verified separately by compiling each layout with libxkbcommon
(scripts/build_xkbcommon.sh); this script only computes the file closure.

Usage: xkb_data_subset.py --src /usr/share/X11/xkb --out DIR
                          --layouts us,gb,de [--options ctrl:nocaps,...]
"""
import argparse
import os
import re
import shutil
import sys

COMPONENT_DIRS = ("keycodes", "types", "compat", "symbols")
INCLUDE_RE = re.compile(r'\b(?:include|augment|override|replace)\s+"([^"]+)"')
# ext2 directories only load this many entries (see docs/vfs.md).
MAX_DIR_ENTRIES = 64


def split_spec(spec):
    """'pc+us(basic):2+inet(evdev)' -> ['pc', 'us', 'inet']"""
    names = []
    for token in re.split(r"[+|^]", spec):
        token = token.strip()
        if not token or token.startswith("%"):
            continue
        token = re.sub(r"\(.*?\)", "", token)
        token = token.split(":")[0]
        if token:
            names.append(token)
    return names


def rules_option_values(rules_text, options):
    """Map requested options to the component strings the rules give them."""
    found = {}
    section = None
    for raw in rules_text.splitlines():
        line = raw.split("//")[0].strip()
        if not line:
            continue
        if line.startswith("!"):
            parts = line[1:].split("=")
            lhs = parts[0].split()
            section = parts[1].strip() if len(parts) == 2 and lhs == ["option"] else None
            continue
        if section and "=" in line:
            key, value = [x.strip() for x in line.split("=", 1)]
            if key in options:
                found.setdefault(section, []).append(value)
    return found


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default="/usr/share/X11/xkb")
    ap.add_argument("--out", required=True)
    ap.add_argument("--layouts", default="us")
    ap.add_argument("--options", default="")
    args = ap.parse_args()

    layouts = [l.split(":")[0] for l in args.layouts.split(",") if l]
    options = [o for o in args.options.split(",") if o]
    rules_path = os.path.join(args.src, "rules", "evdev")
    if not os.path.isfile(rules_path):
        sys.exit("xkb rules not found: %s (install xkb-data / xkeyboard-config)" % rules_path)
    with open(rules_path, encoding="utf-8", errors="replace") as f:
        rules_text = f.read()

    pending = {
        "keycodes": {"evdev", "aliases"},
        "types": {"complete"},
        "compat": {"complete"},
        "symbols": {"pc", "inet"} | set(layouts),
    }
    for component, values in rules_option_values(rules_text, set(options)).items():
        if component in pending:
            for value in values:
                pending[component].update(split_spec(value))

    done = {c: set() for c in COMPONENT_DIRS}
    while any(pending.values()):
        for component in COMPONENT_DIRS:
            while pending[component]:
                name = pending[component].pop()
                if name in done[component]:
                    continue
                path = os.path.join(args.src, component, name)
                if not os.path.isfile(path):
                    sys.exit("xkb file referenced but missing: %s/%s" % (component, name))
                done[component].add(name)
                with open(path, encoding="utf-8", errors="replace") as f:
                    text = f.read()
                for spec in INCLUDE_RE.findall(text):
                    for inc in split_spec(spec):
                        if inc not in done[component]:
                            pending[component].add(inc)

    if os.path.exists(args.out):
        shutil.rmtree(args.out)
    os.makedirs(os.path.join(args.out, "rules"))
    shutil.copy2(rules_path, os.path.join(args.out, "rules", "evdev"))
    total = 1
    for component in COMPONENT_DIRS:
        for name in sorted(done[component]):
            dst = os.path.join(args.out, component, name)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy2(os.path.join(args.src, component, name), dst)
            total += 1

    for root, dirs, files in os.walk(args.out):
        if len(dirs) + len(files) > MAX_DIR_ENTRIES:
            sys.exit("%s has %d entries (limit %d)" % (root, len(dirs) + len(files), MAX_DIR_ENTRIES))
    print("[xkb-data] %d files for layouts=%s options=%s" % (total, ",".join(layouts), ",".join(options) or "-"))


if __name__ == "__main__":
    main()
