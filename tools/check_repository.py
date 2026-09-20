#!/usr/bin/env python3
"""Check the tracked public source tree for stale links and private dependencies."""
from pathlib import Path
import re
import sys
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]

def main():
    # Include newly authored files before the initial commit; exclude generated trees.
    ignored = {".git", "target", "build", ".local", "__pycache__"}
    paths = [p for p in ROOT.rglob("*") if p.is_file() and not any(x in ignored for x in p.relative_to(ROOT).parts)]
    errors = []
    private = re.compile(r"/(?:Users|home)/[^/\s]+|Z-(?:DEC|TD|[A-Z])\d+|/opt/pkg|/Applications/Xcode\.app")
    for path in paths:
        relative = path.relative_to(ROOT)
        if path.is_symlink():
            errors.append(f"{relative}: review symlink before release")
        try:
            text = path.read_text()
        except UnicodeError:
            if path.suffix != ".bin": errors.append(f"{relative}: unreviewed binary")
            continue
        if path != Path(__file__).resolve() and private.search(text):
            errors.append(f"{relative}: historical or machine-specific reference")
        if re.search(r'^\s*#include\s*[<"](?:tads|dict)\.h[>"]', text, re.M):
            errors.append(f"{relative}: external fixture header")
        if path.suffix == ".md":
            for link in re.findall(r'\[[^]]*\]\(([^)]+)\)', text):
                target = unquote(link.split("#", 1)[0])
                if not target or "://" in target or target.startswith("mailto:"): continue
                if not (path.parent / target).exists(): errors.append(f"{relative}: missing link {target}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Checked {len(paths)} source files, documentation links and fixture dependencies.")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
