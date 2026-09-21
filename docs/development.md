# Development guide

The workspace has three crates: `zeb-frontend` owns preprocessing, parsing, checking and LLVM emission; `zeb-runtime` owns language state and session execution; `zebc` owns the CLI, native builds, manifests and tool discovery. Generic source helpers live in `libraries`. Application policy does not belong in these crates.

All first-party Rust forbids unsafe code, including tests. Generated native code and C callers have separate correctness obligations; safe Rust does not prove language lifetimes, source exceptions, ABI behavior or undo semantics. Preserve independent expected outcomes in tests.

## Native toolchain

Rust and external LLVM versions are pinned because emitted code, LLVM inspection and LTO are version-sensitive. Paths and the particular local installation are not pinned. Tool discovery resolves the selected executables once per build, validates versions and target libraries, and fingerprints relevant compiler/libraries/SDK metadata for cache identity.

Install Rust using rustup, then use the checked-in toolchain file. For native macOS builds, install both target libraries with `rustup target add x86_64-apple-darwin aarch64-apple-darwin`. Linux and Windows native builds only need their host Rust target. Obtain LLVM 22.1.8 from an installation of your choice or use the upstream archive bootstrap used by CI:

```sh
python3 tools/bootstrap_llvm.py build/llvm
```

The bootstrap verifies pinned upstream archive SHA-256 values. Apple Silicon macOS uses the upstream binary archive; Intel macOS, Linux and Windows build the required tools from the pinned upstream source archive using CMake and Ninja. Source builds can take substantially longer than project tests. Windows bootstrap must run in an x64 Visual Studio developer environment. It installs only below the directory you supply and prints the `ZEB_LLVM_CONFIG` assignment to use. It does not run sudo or change the system developer-tool selection.

Do not replace version validation with a bypass to make a test pass. New compiler versions need their own native/LTO checks. SDK selection follows `SDKROOT` or the active Apple developer tools; paths containing spaces are supported.

## Tests

Use the commands in the README. `cargo test --workspace --locked` runs the ordinary suite. Frontend integration tests are grouped by syntax, semantics, values, world features and lowering. Runtime tests cover storage, identity, cleanup, grammar, history, save data and sessions. Compiler tests cover CLI diagnostics, manifests, data tables, tool resolution, caches and generated-code constraints.

Native tests are explicitly ignored by the ordinary Cargo run. Run all of them with:

```sh
cargo test -p zebc --locked -- --ignored --test-threads=1
```

This is a real native test request: a missing tool or target library fails it. Native tests compile the host target set (both slices on macOS), execute only the current host slice and use independent results. Temporary test directories are deleted on success and retained on panic for diagnosis. Tests may take several minutes because they exercise multiple output modes and LTO.

Add a focused regression at the layer where the defect occurs. Tests of compiler/runtime behavior must use repository-owned fixtures or inline programs, not external games or private downloads. Put shared native helpers in the existing test support module. Keep expected values independent of the emitter/runtime being tested. Do not delete unique assertions simply to reduce test-file count.

## Clean-checkout checks and CI

The CI workflow runs formatting, workspace tests, Clippy, repository checks, release builds and the native suite on Ubuntu 24.04 x86-64, Windows Server 2025 x86-64, Apple Silicon macOS 15. The bootstrap’s `--config-out PATH` writes JSON containing `llvm_config` for shell-independent configuration. LLVM build caches include host OS/architecture and a recipe revision. The macOS runner builds both architecture slices but executes only Apple Silicon; Intel execution is not covered by CI. Successful jobs upload host-specific compiler artifacts.

Experimental physical-stack qualification remains macOS-specific and rejects new targets explicitly; production executable, object, static/shared and world modes are tested on all supported hosts. Full LTO remains restricted to optimized scalar shared bundles. Every host must execute successfully before reporting execution validation for it. A generated workflow or cross-target check is not a successful native CI run.

For a portability check, copy only tracked source into a new directory, use an empty `CARGO_TARGET_DIR` and runtime cache, and repeat the documented checks. Build and run an installed/copied `zebc` from outside the checkout. No step should read another repository or an ignored header download.

`python3 tools/check_repository.py` checks public-tree hygiene, documentation links and external fixture dependencies. A release review should additionally run a local credential scanner over the exact source export and inspect retained binary fixtures. Do not upload private source to a scanning service.

## Changes and scope

Keep language behavior stable during packaging refactors. Changes to source semantics or the embedding ABI need explicit documentation and tests. Do not add first-party unsafe code. Do not expose internal Rust layouts as public C structures or portable save data.

Keep build artifacts, native fingerprints and logs ignored. Never claim performance, minimum-OS compatibility, or execution on a different architecture from build success alone. Native compilation targets the current supported host OS. Adding architectures or cross-compilation requires separate toolchain and execution coverage.
