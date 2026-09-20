#![forbid(unsafe_code)]
#[path = "lowering/cpu_attributes.rs"]
mod cpu_attributes;
#[path = "lowering/effect_attributes.rs"]
mod effect_attributes;
#[path = "lowering/effects.rs"]
mod effects;
#[path = "lowering/flow.rs"]
mod flow;
#[path = "lowering/grammar_lowering.rs"]
mod grammar_lowering;
#[path = "lowering/indirect_expansion.rs"]
mod indirect_expansion;
#[path = "lowering/ir.rs"]
mod ir;
#[path = "lowering/ir_verify.rs"]
mod ir_verify;
#[path = "lowering/local_reads.rs"]
mod local_reads;
#[path = "lowering/range_for.rs"]
mod range_for;
#[path = "lowering/ranges.rs"]
mod ranges;
#[path = "lowering/roots.rs"]
mod roots;
#[path = "lowering/specialize.rs"]
mod specialize;
#[path = "lowering/stack_candidate.rs"]
mod stack_candidate;
