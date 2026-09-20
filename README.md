# Zebulon

Zebulon is a world-modelling language with a Rust compiler and embeddable runtime. It compiles source through LLVM into native code. Worlds use entities, relations and deterministic Turn/World lifetimes; a host can drive them with text or structured actions.

The `zebc` compiler, frontend, runtime and generic collection helpers are included. An adventure framework is not required. The project is experimental: APIs and supported language subsets may change. See the [language guide](docs/language.md), [embedding guide](docs/embedding.md) and [development guide](docs/development.md).

## Requirements

- Rust **1.98.1**, including the `x86_64-apple-darwin` and `aarch64-apple-darwin` standard libraries. The checked-in rustup configuration selects them.
- For native builds: Intel or Apple Silicon macOS, an Apple macOS SDK selected with Xcode's developer tools, and **LLVM 22.1.8** with Clang, LLD, `opt`, `llvm-config`, `llvm-ar`, `llvm-nm`, `llvm-dis`, `llvm-objdump` and `llvm-lipo`.
- Python 3 for the embedding example and repository checks; Python 3.12+ plus CMake/Ninja for the optional CI LLVM bootstrap.

Native output is universal macOS arm64/x86-64 with a macOS 14.0 deployment target. Building a slice does not establish that it ran on that architecture or on the minimum OS. Local validation has run on an Intel Mac; the Apple Silicon CI workflow must pass before treating that host as execution-verified. Windows, Linux and iOS native output are not provided by this distribution.

LLVM may be installed anywhere. Put the required `llvm-config` on `PATH`, or set `ZEB_LLVM_CONFIG` to its executable. The compiler obtains all LLVM tools from that installation. There is no dependency on a particular package manager.

| Setting | Selection |
| --- | --- |
| LLVM | `ZEB_LLVM_CONFIG`, otherwise `LLVM_CONFIG`, otherwise `llvm-config` on `PATH` |
| Rust | `ZEB_RUSTC`, otherwise `RUSTC`, otherwise `rustc` on `PATH` |
| macOS SDK | `SDKROOT`, otherwise `xcrun --sdk macosx --show-sdk-path` |
| Apple tools | `DEVELOPER_DIR` when set, otherwise the developer tools selected by the system |

An invalid explicit setting is an error. Tool versions and both Rust target libraries are checked before native compilation. Ordinary source checking and Rust unit tests do not require LLVM. [Toolchain setup and CI](docs/development.md#native-toolchain) explains how to obtain the pinned LLVM version.

## Build and install

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

The consumer prints `41`. Output directories must be new: choose another output name to repeat a build. The compiler refuses to overwrite an existing bundle. A shared bundle contains native libraries, a generated header, a manifest and a working C consumer.

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

Native tests build both architecture slices and execute the host slice. They fail on missing prerequisites. They are excluded from the default suite because they compile and link native programs, not because their results are optional for native changes. Test output and build identities remain local.

## Licence

Copyright (c) 2026 John Cunningham. Distributed under the [MIT licence](LICENSE). Apple SDKs, Rust and LLVM are external tools with their own licences; this repository does not redistribute them.
