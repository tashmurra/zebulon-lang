#!/usr/bin/env python3
"""Install a checksum-pinned official LLVM binary archive; never build from source."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.parse
import urllib.request

VERSION = "22.1.8"
BASE = f"https://github.com/llvm/llvm-project/releases/download/llvmorg-{VERSION}/"
# Published SHA-256 digests from the official llvmorg-22.1.8 release assets:
# https://api.github.com/repos/llvm/llvm-project/releases/tags/llvmorg-22.1.8
ARCHIVES = {
    ("Darwin", "arm64"): (f"LLVM-{VERSION}-macOS-ARM64.tar.xz", "f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f"),
    ("Linux", "x86_64"): (f"LLVM-{VERSION}-Linux-X64.tar.xz", "df0e1ecf16caf3489a272a5eea4eec9b0d82878f6477fa309504f918a0006384"),
    ("Windows", "x86_64"): (f"clang+llvm-{VERSION}-x86_64-pc-windows-msvc.tar.xz", "d96c2cc1736f4eb7fa43cb9bbdf56d93551a9ae0a9aadb9c99c3c3b2b712a234"),
}
COMMON_TOOLS = ("llvm-config", "opt", "clang", "llvm-ar", "llvm-nm", "llvm-dis", "llvm-objdump", "llvm-readobj")


def required_tools(system):
    linker = {"Darwin": ("llvm-lipo", "ld64.lld"), "Linux": ("ld.lld",), "Windows": ("lld-link",)}[system]
    suffix = ".exe" if system == "Windows" else ""
    return [name + suffix for name in COMMON_TOOLS + linker]


def validate(installation, system):
    for tool in required_tools(system):
        path = installation / "bin" / tool
        if not path.is_file():
            raise SystemExit(f"unsuitable LLVM archive: missing required tool bin/{tool}; no source fallback")
        try:
            result = subprocess.run([str(path), "--version"], text=True, capture_output=True, timeout=60, check=True)
        except (OSError, subprocess.SubprocessError) as error:
            detail = getattr(error, "stderr", None) or str(error)
            raise SystemExit(f"LLVM tool {tool} cannot run on this host: {detail}; no source fallback") from error
        if VERSION not in re.split(r"[^A-Za-z0-9.\-]+", result.stdout + result.stderr):
            raise SystemExit(f"LLVM tool {tool}: expected {VERSION}, found {result.stdout}{result.stderr}")
    return installation / "bin" / ("llvm-config.exe" if system == "Windows" else "llvm-config")


def install(root, system, name, expected):
    installation = root / "installation"
    marker = installation / "zeb-archive.json"
    provenance = {"archive": name, "sha256": expected}
    if installation.exists():
        if not marker.is_file() or json.loads(marker.read_text(encoding="utf-8")) != provenance:
            raise SystemExit(f"unrecognized cached LLVM installation: {installation}; remove it and retry")
        return validate(installation, system)
    archive = root / name
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        print(f"Downloading {name}", flush=True)
        with urllib.request.urlopen(BASE + urllib.parse.quote(name), timeout=60) as response, partial.open("wb") as out:
            shutil.copyfileobj(response, out)
        partial.replace(archive)
    with archive.open("rb") as stream:
        actual = hashlib.file_digest(stream, "sha256").hexdigest()
    if actual != expected:
        raise SystemExit(f"checksum mismatch for {archive}: expected {expected}, found {actual}; remove it and retry")
    print(f"Verified SHA-256: {actual}", flush=True)
    with tempfile.TemporaryDirectory(prefix="extract-", dir=root) as temporary:
        staging = Path(temporary)
        with tarfile.open(archive) as package:
            package.extractall(staging, filter="data")
        candidates = list(staging.glob("*/bin/llvm-config*"))
        candidates = [p for p in candidates if p.name in ("llvm-config", "llvm-config.exe")]
        if len(candidates) != 1:
            raise SystemExit("unsuitable LLVM archive: expected exactly one bin/llvm-config executable; no source fallback")
        extracted = candidates[0].parent.parent
        validate(extracted, system)
        (extracted / "zeb-archive.json").write_text(json.dumps(provenance) + "\n", encoding="utf-8")
        extracted.rename(installation)
    archive.unlink()
    print(f"Validated all {len(required_tools(system))} required tools", flush=True)
    return validate(installation, system)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--config-out", type=Path, help="write resolved tools as JSON")
    args = parser.parse_args()
    if sys.version_info < (3, 12):
        parser.error("Python 3.12+ is required for safe archive extraction")
    system, arch = platform.system(), platform.machine().lower()
    if arch == "amd64":
        arch = "x86_64"
    if (system, arch) not in ARCHIVES:
        parser.error("no pinned prebuilt archive for this host; configure an existing LLVM 22.1.8 installation with ZEB_LLVM_CONFIG; no source fallback")
    root = args.directory.resolve()
    root.mkdir(parents=True, exist_ok=True)
    config = install(root, system, *ARCHIVES[(system, arch)])
    if args.config_out:
        args.config_out.write_text(json.dumps({"llvm_config": str(config)}) + "\n", encoding="utf-8")
    if system == "Windows":
        print("$env:ZEB_LLVM_CONFIG = '" + str(config).replace("'", "''") + "'")
    else:
        print("export ZEB_LLVM_CONFIG=" + shlex.quote(str(config)))


if __name__ == "__main__":
    main()
