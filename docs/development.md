# Development guide

The workspace has three crates: `zeb-frontend` owns preprocessing, parsing, checking and LLVM emission; `zeb-runtime` owns language state and session execution; `zebc` owns the CLI, native builds, manifests and tool discovery. Generic source helpers live in `libraries`. Application policy does not belong in these crates.

All first-party Rust forbids unsafe code, including tests. Generated native code and C callers have separate correctness obligations; safe Rust does not prove language lifetimes, source exceptions, ABI behavior or undo semantics. Preserve independent expected outcomes in tests.

## Native toolchain

Rust and external LLVM versions are pinned because emitted code, LLVM inspection and LTO are version-sensitive. Paths and the particular local installation are not pinned. Tool discovery resolves the selected executables once per build, validates versions and target libraries, and fingerprints relevant compiler/libraries/SDK metadata for cache identity.

Install Rust using rustup, then use the checked-in toolchain file. Obtain LLVM 22.1.8 from an installation of your choice or use the upstream archive bootstrap used by CI:

```sh
python3 tools/bootstrap_llvm.py build/llvm
```

The bootstrap verifies pinned upstream archive SHA-256 values. Apple Silicon uses the upstream binary archive; Intel builds the required tools from the upstream source archive and needs CMake and Ninja. The Intel build can take substantially longer than project tests. It installs only below the directory you supply and prints the `ZEB_LLVM_CONFIG` assignment to use. It does not run sudo or change the system developer-tool selection.

Do not replace version validation with a bypass to make a test pass. New compiler versions need their own native/LTO checks. SDK selection follows `SDKROOT` or the active Apple developer tools; paths containing spaces are supported.

## Tests

Use the commands in the README. `cargo test --workspace --locked` runs the ordinary suite. Frontend integration tests are grouped by syntax, semantics, values, world features and lowering. Runtime tests cover storage, identity, cleanup, grammar, history, save data and sessions. Compiler tests cover CLI diagnostics, manifests, data tables, tool resolution, caches and generated-code constraints.

Native tests are explicitly ignored by the ordinary Cargo run. Run all of them with:

```sh
cargo test -p zebc --locked -- --ignored --test-threads=1
```

This is a real native test request: a missing tool or target library fails it. Native tests compile both architecture slices, execute only the current host slice and use independent results. Temporary test directories are deleted on success and retained on panic for diagnosis. Tests may take several minutes because they exercise multiple output modes and LTO.

Add a focused regression at the layer where the defect occurs. Tests of compiler/runtime behavior must use repository-owned fixtures or inline programs, not external games or private downloads. Put shared native helpers in the existing test support module. Keep expected values independent of the emitter/runtime being tested. Do not delete unique assertions simply to reduce test-file count.

## Clean-checkout checks and CI

The CI workflow configures Intel and Apple Silicon macOS jobs, pins prerequisites and invokes the same workspace and native commands. Both architectures must execute successfully before reporting dual-host execution validation. A generated workflow is not a successful CI run.

For a portability check, copy only tracked source into a new directory, use an empty `CARGO_TARGET_DIR` and runtime cache, and repeat the documented checks. Build and run an installed/copied `zebc` from outside the checkout. No step should read another repository or an ignored header download.

`python3 tools/check_repository.py` checks public-tree hygiene, documentation links and external fixture dependencies. A release review should additionally run a local credential scanner over the exact source export and inspect retained binary fixtures. Do not upload private source to a scanning service.

## Changes and scope

Keep language behavior stable during packaging refactors. Changes to source semantics or the embedding ABI need explicit documentation and tests. Do not add first-party unsafe code. Do not expose internal Rust layouts as public C structures or portable save data.

Keep build artifacts, native fingerprints and logs ignored. Never claim performance, minimum-OS compatibility, or execution on a different architecture from build success alone. This distribution's native implementation targets macOS; new target backends are separate work.
