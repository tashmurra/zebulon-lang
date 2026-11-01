# Development guide

The workspace has three crates: `zeb-frontend` owns preprocessing, parsing, checking and LLVM emission; `zeb-runtime` owns language state and session execution; `zebc` owns the CLI, native builds, manifests and tool discovery. Generic source helpers live in `libraries`. Application policy does not belong in these crates.

All first-party Rust forbids unsafe code, including tests. Generated native code and C callers have separate correctness obligations; safe Rust does not prove language lifetimes, source exceptions, ABI behavior or undo semantics. Preserve independent expected outcomes in tests.

## Native toolchain

Rust and external LLVM versions are pinned because emitted code, LLVM inspection and LTO are version-sensitive. Paths and the particular local installation are not pinned. Tool discovery resolves the selected executables once per build, validates versions and target libraries, and fingerprints relevant compiler/libraries/SDK metadata for cache identity.

Install Rust using rustup, then use the checked-in toolchain file. For native macOS builds, install both target libraries with `rustup target add x86_64-apple-darwin aarch64-apple-darwin`. Linux and Windows native builds only need their host Rust target. Obtain LLVM 22.1.8 from an installation of your choice or use the upstream archive bootstrap used by CI:

```sh
python3 tools/bootstrap_llvm.py build/llvm
```

The bootstrap downloads only official prebuilt LLVM 22.1.8 archives: `LLVM-22.1.8-macOS-ARM64.tar.xz`, `LLVM-22.1.8-Linux-X64.tar.xz`, or the full `clang+llvm-22.1.8-x86_64-pc-windows-msvc.tar.xz` archive (not the smaller Windows installer). SHA-256 values are pinned from the official GitHub release asset digests. It checks the checksum before extraction, then verifies the presence and pinned version of every required tool. Missing tools or host incompatibility stop setup with an explicit error; there is no source-build fallback.

CI caches only the extracted installation, with archive provenance recorded inside it. Archives and partial downloads are excluded from the cache. An Intel Mac must use an existing LLVM 22.1.8 installation selected via `ZEB_LLVM_CONFIG`; the bootstrap has no pinned Intel macOS binary. Windows requires an x64 Visual Studio developer environment and Windows SDK. The bootstrap installs only below the supplied directory, requires Python 3.12+, and never runs sudo or changes system developer-tool selection.

Do not replace version validation with a bypass to make a test pass. New compiler versions need their own native/LTO checks. SDK selection follows `SDKROOT` or the active Apple developer tools; paths containing spaces are supported.

### Separate LLVM and LLD installations on macOS

The macOS linker is selected from `ZEB_LD64_LLD` (an executable path or name), then the compatibility alias `ZEB_LLD`, then `ld64.lld` in `llvm-config --bindir`, then `ld64.lld` on `PATH` if the bundled linker is absent. If both variables are set, `ZEB_LD64_LLD` wins. An empty, missing, or wrong-version selection is an error; it does not fall back to the alias or automatic discovery. An invalid bundled linker is also an error. The selected linker must report LLVM's pinned version, 22.1.8, and its path and contents are included in `native-tool-lock.json` and build/cache identities. Full LTO uses this resolved executable.

Homebrew packages LLVM and LLD in separate kegs. With `llvm@22` and `lld@22` both at 22.1.8, configure them without copying or linking files into either keg:

```sh
export ZEB_LLVM_CONFIG="$(brew --prefix llvm@22)/bin/llvm-config"
export ZEB_LD64_LLD="$(brew --prefix lld@22)/bin/ld64.lld"
rustup target add x86_64-apple-darwin aarch64-apple-darwin
mkdir -p build
zebc build hello.t --emit shared --out-dir build/hello
./build/hello/consumer
lipo -archs build/hello/consumer
```

Existing Makefiles that export `ZEB_LLD="$(brew --prefix lld@22)/bin/ld64.lld"` work without adding the LLD keg to `PATH`, provided `ZEB_LD64_LLD` is unset. Both variables select the executable, not its containing directory.

Alternatively, put the matching LLD bin directory on `PATH`. The formula names alone do not guarantee the pinned patch version; the compiler validates the installed tools. The output directory must be new. Universal output is verified with separate `lipo -verify_arch` calls for `x86_64` and `arm64`, accommodating Apple lipo variants that interpret a second architecture as another input file. Both slices must pass verification before the bundle is marked complete.

### Native build duration and deadlines

Native compilation and linking have no wall-clock deadline by default. Large
optimized programs and cold runtime builds can legitimately take several minutes.
Set `ZEB_BUILD_TIMEOUT_SECS` to a positive integer to impose a per-command deadline
in automation; unset it or use `0` to disable that deadline. This covers Rust,
Clang, LLVM verification and scalar LTO build commands. It is not a total build
budget. Tool discovery, artifact inspection and native test execution retain
separate bounded deadlines. Output limits and process-tree cleanup still apply.
Deadline errors report the executable, arguments, working directory, elapsed time
and configured limit, so the failing architecture and stage can be identified.

Object/world builds compile the embedded Rust runtime and generated game code for
each host slice. macOS builds both x86-64 and arm64 sequentially. `--opt O2`
optimizes both runtime and game code; no Cargo incremental cache is used for these
embedded runtime compilations. Use the same explicit runtime cache directory
across builds to avoid recompiling an unchanged runtime:

```sh
zebc build examples/world.t --opt O2 --emit shared --runtime-cache build/runtime-cache --out-dir build/world-cached
```

The output directory must be new each time. The cache is opt-in and checks runtime
sources, toolchain, recipe and architecture; program-only changes can reuse it,
while compiler/runtime, optimization or toolchain changes may require a cold build.

## Tests

Use the commands in the README. `cargo test --workspace --locked` runs the ordinary suite. Frontend integration tests are grouped by syntax, semantics, values, world features and lowering. Runtime tests cover storage, identity, cleanup, grammar, history, save data and sessions. Compiler tests cover CLI diagnostics, manifests, data tables, tool resolution, caches and generated-code constraints.

Native tests are explicitly ignored by the ordinary Cargo run. Run all of them with:

```sh
cargo test -p zebc --locked -- --ignored --test-threads=1
```

This is a real native test request: a missing tool or target library fails it. Native tests compile the host target set (both slices on macOS), execute only the current host slice and use independent results. Temporary test directories are deleted on success and retained on panic for diagnosis. Tests may take several minutes because they exercise multiple output modes and LTO.

Add a focused regression at the layer where the defect occurs. Tests of compiler/runtime behavior must use repository-owned fixtures or inline programs, not external games or private downloads. Put shared native helpers in the existing test support module. Keep expected values independent of the emitter/runtime being tested. Do not delete unique assertions simply to reduce test-file count.

### Object enumeration performance

The runtime keeps a sorted vector of live record IDs alongside its object map.
`firstObj`/`nextObj` seek after the supplied logical slot and visit subsequent
records once, rather than repeatedly searching the entire hash table for the
next ID. Class and inheritance filters are evaluated against current records;
the index caches neither membership nor handles. Enumeration therefore retains
ascending slot order, current incarnation checks and visibility of prototype
changes between calls.

Creation reserves index capacity before publishing an object. Destruction removes
its ID, including recursive cleanup and Turn expiry. Restore reuses the validated,
sorted ID list from the existing save format, and failed restores leave the old
store intact. The index requires one `u64` per live record plus vector capacity;
inserting a reused slot and removing a record can shift O(n) IDs. This trades
mutation work for cheaper enumeration, so performance comparisons should include
allocation/cleanup workloads as well as repeated class walks. Sparse logical IDs
do not require scanning the gaps or allocating storage up to the highest ID.

Measure code changes at the same optimization level first, then compare native
`--opt O2` builds. Keep benchmark data and native fingerprints under ignored
`build/`; record the source revision, input world and command sequence, host
architecture, warmup, sample count and timing boundary. Native command completion
does not include an application's UI rendering or transport overhead.

## Clean-checkout checks and CI

The CI workflow runs formatting, workspace tests, Clippy, repository checks, release builds and the native suite on Ubuntu 24.04 x86-64, Windows Server 2025 x86-64, Apple Silicon macOS 15. The bootstrap’s `--config-out PATH` writes JSON containing `llvm_config` for shell-independent configuration. Extracted LLVM installation caches include host OS/architecture and a recipe revision. The macOS runner builds `zebc` for both `aarch64-apple-darwin` and `x86_64-apple-darwin` with deployment target 14.0, combines them with the resolved `llvm-lipo`, verifies both slices and uses the universal compiler to build and execute a sample. It uploads `target/universal/zebc` as `zebc-macos-universal`. Both the compiler and generated macOS output are universal, but CI executes only Apple Silicon; Intel execution is not covered. Linux and Windows upload their host-native x86-64 compilers. Ordinary `cargo build` and `cargo install` still produce a compiler for their selected Rust target.

Release builds on macOS use only the two explicit target builds above; CI does
not first build another host release compiler. Debug/test-profile builds remain
separate because they validate the workspace and native suite.

Each PR revision builds and tests its explicit head commit on all three
platforms. On a push to `main`, a small selection job looks for a successful PR
run with an identical Git tree (all tracked files, including toolchain pins and
the workflow). It verifies that all three native jobs passed and all three
compiler artifacts are still available with SHA-256 digests. When they match,
the native matrix is skipped and the merge run's summary links to the original
PR artifacts. No compiler is rebuilt or copied merely because its source now
has a merge commit hash.

A changed merge tree, unavailable artifacts or a failed lookup runs the normal
build and test matrix. Selection checks the latest 100 successful PR runs of
this workflow and does not follow other reuse-only runs. PR checks cover the PR
head; integration changes introduced by merging are validated by the fallback
main build. Manual `workflow_dispatch` runs always build and test, providing an
explicit way to regenerate artifacts. Tag pushes do not trigger this workflow.
Run `python3 tools/test_ci_reuse.py` for the offline artifact-selection tests.

### Release from tested CI artifacts

Include the intended package version and lockfile update in the implementation
PR, before its CI build. After merging, wait for the main-branch CI run at the
release commit to succeed. If it reused a PR build, follow the original run link
in its summary; otherwise use the main run's own artifacts. Download
`zebc-linux-x86_64`, `zebc-windows-x86_64` and `zebc-macos-universal`, verify their
downloaded ZIP checksums against GitHub's artifact digests, package the binaries
with `LICENSE`, and generate archive SHA-256 checksums.

Before publishing, independently verify that the source run succeeded, all three
native jobs passed, artifacts are unexpired, and the built commit's Git tree is
identical to the release commit's tree. Verify binary architectures and the
reported compiler version. Restore executable permissions when unpacking Actions
artifacts on macOS/Linux. Record the original CI run URL, built commit, source
tree and release commit in the release notes. Tag the release commit on `main`.
When the merge reused artifacts, the original PR commit and release commit will
usually differ; their source trees must not.

Packaging and publishing do not need another compiler build. Do not substitute
local binaries or artifacts from a different source tree. If the version,
compiler source or build recipe changes, obtain artifacts from a successful CI
run of that new tree. A normal PR, unchanged merge and release therefore use one
native build/test matrix, without a separate version-only PR/build cycle.

### Platform validation

Experimental physical-stack qualification remains macOS-specific and rejects new targets explicitly; production executable, object, static/shared and world modes are tested on all supported hosts. Full LTO remains restricted to optimized scalar shared bundles. Every host must execute successfully before reporting execution validation for it. A generated workflow or cross-target check is not a successful native CI run.

For a portability check, copy only tracked source into a new directory, use an empty `CARGO_TARGET_DIR` and runtime cache, and repeat the documented checks. Build and run an installed/copied `zebc` from outside the checkout. No step should read another repository or an ignored header download.

`python3 tools/check_repository.py` checks public-tree hygiene, documentation links and external fixture dependencies. A release review should additionally run a local credential scanner over the exact source export and inspect retained binary fixtures. Do not upload private source to a scanning service.

## Changes and scope

Keep language behavior stable during packaging refactors. Changes to source semantics or the embedding ABI need explicit documentation and tests. Do not add first-party unsafe code. Do not expose internal Rust layouts as public C structures or portable save data.

Keep build artifacts, native fingerprints and logs ignored. Never claim performance, minimum-OS compatibility, or execution on a different architecture from build success alone. Native compilation targets the current supported host OS. Adding architectures or cross-compilation requires separate toolchain and execution coverage.

Windows generated IR uses the pinned Rust MSVC target’s x86-64 baseline (SSE3, CMPXCHG16B and LAHF/SAHF, in addition to SSE2). Matching runtime and game function attributes allows full LTO to inline across the runtime boundary. This does not qualify execution on an older Windows release or a different architecture.
