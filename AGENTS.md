# Working on Zebulon

Zebulon is a Rust compiler and runtime for a world-modelling language. The compiler command is `zebc`; `zeb` is reserved for tooling.

- Keep compiler, runtime, standard helpers and application policy separate.
- Preserve safe Rust: crate roots forbid unsafe code. Do not add unsafe FFI, generated unsafe code or local shims that bypass this policy.
- Preserve source-language semantics independently of Rust implementation ownership. Changes to lifetimes, relations, undo, persistence or embedding require focused regression tests and documentation.
- Read the README and development guide before changing build or test infrastructure.
- Use the shared toolchain resolver; do not encode local installation paths.
- Keep tests independent of private files, third-party games and network services.
- Run focused checks during development and the documented workspace/native suites for affected behavior. Do not claim execution on architectures that only cross-compiled.
- Keep generated build output and local tool fingerprints out of version control.
- Use `codex/` for new development branches. Do not publish or push without authorization.
