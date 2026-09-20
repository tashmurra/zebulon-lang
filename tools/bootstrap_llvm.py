#!/usr/bin/env python3
"""Install pinned upstream LLVM below a user-selected directory, without sudo."""
import argparse
import hashlib
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import urllib.request

VERSION = "22.1.8"
BASE = f"https://github.com/llvm/llvm-project/releases/download/llvmorg-{VERSION}/"
ARCHIVES = {
    "arm64": (f"LLVM-{VERSION}-macOS-ARM64.tar.xz", "f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f"),
    "x86_64": (f"llvm-project-{VERSION}.src.tar.xz", "922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888"),
}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    if sys.version_info < (3, 12):
        parser.error("Python 3.12+ is required for safe archive extraction")
    arch = platform.machine()
    if platform.system() != "Darwin" or arch not in ARCHIVES:
        parser.error("this bootstrap supports Intel and Apple Silicon macOS")
    root = args.directory.resolve()
    root.mkdir(parents=True, exist_ok=True)
    name, expected = ARCHIVES[arch]
    archive = root / name
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        with urllib.request.urlopen(BASE + name, timeout=60) as response, partial.open("wb") as out:
            shutil.copyfileobj(response, out)
        partial.replace(archive)
    with archive.open("rb") as stream:
        actual = hashlib.file_digest(stream, "sha256").hexdigest()
    if actual != expected:
        raise SystemExit(f"checksum mismatch for {archive}; remove it and retry")
    source = root / name.removesuffix(".tar.xz")
    if not source.exists():
        with tarfile.open(archive) as package:
            package.extractall(root, filter="data")
    if arch == "arm64":
        config = source / "bin/llvm-config"
    else:
        build = root / "build"
        subprocess.run([
            "cmake", "-G", "Ninja", "-S", str(source / "llvm"), "-B", str(build),
            "-DCMAKE_BUILD_TYPE=Release", "-DLLVM_ENABLE_PROJECTS=clang;lld",
            "-DLLVM_TARGETS_TO_BUILD=X86;AArch64", "-DLLVM_INCLUDE_TESTS=OFF",
            "-DLLVM_INCLUDE_BENCHMARKS=OFF", "-DLLVM_INCLUDE_EXAMPLES=OFF",
            "-DLLVM_ENABLE_ASSERTIONS=OFF", "-DLLVM_PARALLEL_LINK_JOBS=1",
        ], cwd=root, check=True)
        subprocess.run([
            "cmake", "--build", str(build), "--parallel", "3", "--target",
            "llvm-config", "opt", "clang", "llvm-ar", "llvm-nm", "llvm-dis",
            "llvm-objdump", "llvm-lipo", "lld",
        ], cwd=root, check=True)
        config = build / "bin/llvm-config"
    found = subprocess.check_output([str(config), "--version"], cwd=root, text=True).strip()
    if found != VERSION:
        raise SystemExit(f"expected LLVM {VERSION}, found {found}")
    import shlex
    print("export ZEB_LLVM_CONFIG=" + shlex.quote(str(config)))

if __name__ == "__main__":
    main()
