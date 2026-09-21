# Zebulon

Zebulon is a world-modelling language with a Rust compiler and embeddable runtime. It compiles source through LLVM into native code. Worlds use entities, relations and deterministic Turn/World lifetimes; a host can drive them with text or structured actions.

The `zebc` compiler, frontend, runtime and generic collection helpers are included. An adventure framework is not required. The project is experimental: APIs and supported language subsets may change. See the [language guide](docs/language.md), [embedding guide](docs/embedding.md) and [development guide](docs/development.md).

## Requirements

- Rust **1.98.1**. The checked-in rustup configuration selects it for compiler/runtime builds on macOS, Linux and Windows.
- For native builds: **LLVM 22.1.8** with Clang, LLD, `opt`, `llvm-config`, `llvm-ar`, `llvm-nm`, `llvm-dis`, `llvm-objdump` and `llvm-readobj`.
- macOS additionally requires an Apple macOS SDK, `llvm-lipo`, and both Rust targets: `rustup target add x86_64-apple-darwin aarch64-apple-darwin`.
- Linux x86-64 requires the host C/C++ development toolchain (including libc headers and startup libraries).
- Windows x86-64 requires Visual Studio C++ Build Tools and a Windows SDK. Run from an **x64 Developer PowerShell** so `INCLUDE` and `LIB` are configured.
- Python 3 for repository checks and embedding examples; Python 3.12+, CMake and Ninja for the optional LLVM bootstrap.

Each compiler builds for its host OS. macOS output is universal arm64/x86-64 with deployment target 14.0; Linux output uses the x86-64 GNU/Linux ABI; Windows output uses the x86-64 MSVC ABI. Cross-OS builds, Linux/Windows ARM64, musl, MinGW and iOS are not supported. Build success alone does not establish execution on another architecture or compatibility with older operating systems.

LLVM may be installed anywhere. Put the required `llvm-config` on `PATH`, or set `ZEB_LLVM_CONFIG` to its executable. The compiler obtains all LLVM tools from that installation. There is no dependency on a particular package manager.

| Setting | Selection |
| --- | --- |
| LLVM | `ZEB_LLVM_CONFIG`, otherwise `LLVM_CONFIG`, otherwise `llvm-config` on `PATH` |
| Rust | `ZEB_RUSTC`, otherwise `RUSTC`, otherwise `rustc` on `PATH` |
| macOS SDK | `SDKROOT`, otherwise `xcrun --sdk macosx --show-sdk-path` |
| Apple tools | `DEVELOPER_DIR` when set, otherwise the developer tools selected by the system |

An invalid explicit setting is an error. Tool versions and the required Rust target libraries are checked before native compilation. Ordinary source checking and Rust unit tests do not require LLVM. [Toolchain setup and CI](docs/development.md#native-toolchain) explains how to obtain the pinned LLVM version.

## Build and install

CI builds and tests the Rust workspace and generated native programs on Linux x86-64, Windows x86-64 and Apple Silicon macOS. The macOS job builds both universal slices but executes only Apple Silicon; Intel execution is not covered by CI. Successful jobs provide host-specific release `zebc` binaries as workflow artifacts. Download them from the successful run’s Artifacts section in GitHub Actions; these compiler executables are not universal. Native builds require the external prerequisites above.

From the repository root:

```sh
cargo build --workspace --locked
cargo run --locked -p zebc -- --help
cargo install --path crates/zebc --locked
```

The installed compiler embeds the runtime source it needs; it does not need this checkout when building another project. If Rust was installed using rustup, ensure its binaries are on `PATH`. Native builds also need the external tools listed above.

## First program

The supplied `examples/scalar.t` is:

```text
main(value) { return value * 2 + 1; }
```

Build a shared bundle and run its generated C consumer:

```sh
cargo run --locked -p zebc -- check examples/scalar.t --model ownership
mkdir -p build
cargo run --locked -p zebc -- build examples/scalar.t --model ownership --emit shared --out-dir build/scalar
./build/scalar/consumer 20
```

On Windows, use `build/scalar/consumer.exe 20`. The consumer prints `41`. Output directories must be new: choose another output name to repeat a build. The compiler refuses to overwrite an existing bundle. A shared bundle contains native libraries, a generated header, a manifest and a working C consumer.

## A host-driven world

`examples/world.t` models a probe on a bench. Action 1 increments its reading; action 2 undoes the last completed turn.

```sh
cargo run --locked -p zebc -- check examples/world.t
cargo run --locked -p zebc -- build examples/world.t --emit shared --out-dir build/world
printf '!1\n!1\n!2\n' | ./build/world/consumer
```

The readings are `1`, `2` and `1`. The consumer also accepts plain text, `:save PATH` and `:restore PATH`. It is an example host; applications should implement their own presentation and filesystem policies.

`examples/embedding.c` shows a minimal independent C caller for the scalar bundle:

```sh
python3 examples/run_embedding.py build/scalar
```

See the [embedding guide](docs/embedding.md) before integrating a world bundle.

## Tests

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 tools/check_repository.py
```

With the native prerequisites configured, explicitly run the native suite:

```sh
cargo test -p zebc --locked -- --ignored --test-threads=1
```

Native tests build the host target set and execute the host slice; macOS builds both architecture slices. They fail on missing prerequisites. They are excluded from the default suite because they compile and link native programs, not because their results are optional for native changes. Test output and build identities remain local.

## Licence

Copyright (c) 2026 John Cunningham. Distributed under the [MIT licence](LICENSE). Apple SDKs, Rust and LLVM are external tools with their own licences; this repository does not redistribute them.
