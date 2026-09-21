#![forbid(unsafe_code)]

//! Host compiler tooling, separate from the game runtime.
pub mod digest;
pub mod manifest;
pub mod platform;
pub mod process;
pub mod runtime_cache;
mod stack_callgraph;
pub mod stack_charges;
pub mod stack_guards;
pub mod stack_machine;
pub mod toolchain;
