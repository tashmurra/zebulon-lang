#!/usr/bin/env python3
"""Compile and run the independent C host for a scalar.t shared bundle."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    args = parser.parse_args()
    bundle = args.bundle.resolve()
    manifest = json.loads((bundle / "manifest.json").read_text())
    config = os.environ.get("ZEB_LLVM_CONFIG", os.environ.get("LLVM_CONFIG", "llvm-config"))
    found = shutil.which(config)
    if found is None:
        raise SystemExit("cannot find configured llvm-config executable")
    config = str(Path(found).absolute())
    binary = subprocess.check_output([config, "--bindir"], cwd=bundle, text=True).strip()
    sdk = os.environ.get("SDKROOT")
    if sdk is None:
        sdk = subprocess.check_output(["xcrun", "--sdk", "macosx", "--show-sdk-path"], cwd=bundle, text=True).strip()
    # The manifest/header are trusted outputs of this compiler, not downloaded input.
    source = Path(__file__).resolve().with_name("embedding.c")
    symbol = manifest["game_entry"]
    import re
    if not isinstance(symbol, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", symbol):
        raise SystemExit("invalid manifest entry symbol")
    libraries = []
    for key in ("game", "runtime"):
        name = manifest[key]
        if Path(name).name != name:
            raise SystemExit("manifest library must be a filename")
        path = bundle / name
        if path not in libraries:
            libraries.append(path)
    output = bundle / "embedding-example"
    subprocess.run([
        str(Path(binary) / "clang"), "-std=c11", "-Wall", "-Wextra", "-Werror",
        "-isysroot", sdk, "-mmacosx-version-min=14.0", "-I", str(bundle),
        "-DZEB_ENTRY=" + symbol, str(source), *map(str, libraries),
        "-Wl,-rpath,@loader_path", "-o", str(output),
    ], cwd=bundle, check=True)
    subprocess.run([str(output)], cwd=bundle, check=True)

if __name__ == "__main__":
    main()
