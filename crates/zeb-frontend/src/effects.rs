//! Conservative source effects for verified scalar IR. Local frames are not shared state.
use crate::{
    Diagnostic,
    ir::{Failure, Operation, Program, Terminator},
    ir_verify,
    parser::Ast,
};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum Effect {
    ReadState = 1,
    WriteState = 2,
    Allocate = 4,
    Transfer = 8,
    Destroy = 16,
    Throw = 32,
    Callback = 64,
    Io = 128,
    Suspend = 256,
    Diverge = 512,
    SourceException = 1024,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Effects(u16);
impl Effects {
    /// Unknown external behavior must never default to pure.
    pub const fn unknown() -> Self {
        Self(2047)
    }
    pub const fn contains(self, effect: Effect) -> bool {
        self.0 & effect as u16 != 0
    }
    /// Only explicit scalar error outcomes and possible nontermination remain.
    /// Unknown or future effect bits deny native purity assumptions.
    pub const fn scalar_only(self) -> bool {
        self.0 & !(Effect::Throw as u16 | Effect::Diverge as u16) == 0
    }
    fn insert(&mut self, effect: Effect) {
        self.0 |= effect as u16;
    }
    fn merge(&mut self, other: Self) -> bool {
        let before = *self;
        self.0 |= other.0;
        *self != before
    }
}
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub effects: Effects,
    pub reads_frame: bool,
    pub writes_frame: bool,
}
fn filled<T: Clone>(n: usize, value: T) -> Result<Vec<T>, Diagnostic> {
    let mut out = Vec::new();
    out.try_reserve(n).map_err(|_| Diagnostic::resource(0))?;
    out.resize(n, value);
    Ok(out)
}
fn push<T>(out: &mut Vec<T>, value: T) -> Result<(), Diagnostic> {
    out.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    out.push(value);
    Ok(())
}
fn reverse(edges: &[Vec<usize>]) -> Result<Vec<Vec<usize>>, Diagnostic> {
    let mut callers = filled(edges.len(), Vec::new())?;
    for (from, targets) in edges.iter().enumerate() {
        for &to in targets {
            push(&mut callers[to], from)?;
        }
    }
    Ok(callers)
}
// Repeatedly remove sinks. Survivors can reach a cycle, including its callers.
fn may_loop(edges: &[Vec<usize>]) -> Result<Vec<bool>, Diagnostic> {
    let incoming = reverse(edges)?;
    let mut remaining = filled(edges.len(), 0usize)?;
    let mut queue = Vec::new();
    for (node, targets) in edges.iter().enumerate() {
        remaining[node] = targets.len();
        if targets.is_empty() {
            push(&mut queue, node)?;
        }
    }
    while let Some(node) = queue.pop() {
        for &caller in &incoming[node] {
            remaining[caller] -= 1;
            if remaining[caller] == 0 {
                push(&mut queue, caller)?;
            }
        }
    }
    let mut result = filled(edges.len(), false)?;
    for (i, degree) in remaining.into_iter().enumerate() {
        result[i] = degree != 0;
    }
    Ok(result)
}

pub fn analyze(ast: &Ast, program: &Program) -> Result<Vec<Summary>, Diagnostic> {
    ir_verify::verify(ast, program)?;
    let mut summaries = filled(program.functions.len(), Summary::default())?;
    let mut calls = filled(program.functions.len(), Vec::new())?;
    for (index, function) in program.functions.iter().enumerate() {
        if matches!(
            ast.nodes[function.source.0].syntax,
            crate::parser::Syntax::Function { rest: true, .. }
                | crate::parser::Syntax::Function {
                    returns_owned: true,
                    ..
                }
        ) {
            for effect in [Effect::Allocate, Effect::Destroy, Effect::Throw] {
                summaries[index].effects.insert(effect);
            }
        }
        let mut edges = filled(function.blocks.len(), Vec::new())?;
        for (bi, block) in function.blocks.iter().enumerate() {
            match block.terminator {
                Terminator::Invoke { normal, .. } => push(&mut edges[bi], normal.0)?,
                Terminator::Resume(_) => {
                    summaries[index].effects.insert(Effect::Throw);
                    summaries[index].effects.insert(Effect::SourceException);
                }
                Terminator::Jump(to) => push(&mut edges[bi], to.0)?,
                Terminator::Branch { yes, no, .. } => {
                    push(&mut edges[bi], yes.0)?;
                    push(&mut edges[bi], no.0)?;
                }
                Terminator::Throw(..) => {
                    summaries[index].effects.insert(Effect::Throw);
                    summaries[index].effects.insert(Effect::SourceException);
                }
                Terminator::Return(_) | Terminator::Fallthrough => {}
            }
            if let Some(edge) = block.terminator.exception() {
                push(&mut edges[bi], edge.target.0)?;
            }
        }
        if may_loop(&edges)?[0] {
            summaries[index].effects.insert(Effect::Diverge);
        }
        let mut reached = filled(function.blocks.len(), false)?;
        let mut pending = Vec::new();
        push(&mut pending, 0)?;
        while let Some(bi) = pending.pop() {
            if reached[bi] {
                continue;
            }
            reached[bi] = true;
            for &to in &edges[bi] {
                push(&mut pending, to)?;
            }
            for instruction in &function.blocks[bi].instructions {
                match &instruction.operation {
                    Operation::Load(_) => summaries[index].reads_frame = true,
                    Operation::Store(..) | Operation::Reset(_) => {
                        summaries[index].writes_frame = true
                    }
                    Operation::Call { function, .. } => push(&mut calls[index], *function)?,
                    Operation::CallMethod { .. } | Operation::GetProperty(..) => {
                        summaries[index].effects = Effects::unknown()
                    }
                    Operation::Constant(_)
                    | Operation::PropertyValue(_)
                    | Operation::FunctionValue(_)
                    | Operation::Enumerator(_) => {}
                    Operation::Builtin { .. } => summaries[index].effects = Effects::unknown(),
                    Operation::EmitLiteral(_) | Operation::EmitValue(_) => {
                        summaries[index].effects.insert(Effect::Io);
                        summaries[index].effects.insert(Effect::Allocate);
                        summaries[index].effects.insert(Effect::Throw);
                    }
                    Operation::SetProperty(..)
                    | Operation::BeginStatic(..)
                    | Operation::EndStatic => {
                        summaries[index].effects.insert(Effect::WriteState);
                        summaries[index].effects.insert(Effect::Allocate);
                        summaries[index].effects.insert(Effect::Throw);
                    }
                    Operation::StringLiteral(_) | Operation::NamedObject(_) => {
                        summaries[index].effects.insert(Effect::ReadState);
                        summaries[index].effects.insert(Effect::Throw);
                    }
                    Operation::Unary(..) | Operation::Binary(..) | Operation::LogicalGuard(_) => {
                        if instruction.failure == Failure::Propagate {
                            summaries[index].effects.insert(Effect::Throw);
                        }
                    }
                }
            }
        }
    }
    for (i, cycle) in may_loop(&calls)?.into_iter().enumerate() {
        if cycle {
            summaries[i].effects.insert(Effect::Diverge);
        }
    }
    let callers = reverse(&calls)?;
    let mut queue = VecDeque::new();
    queue
        .try_reserve(summaries.len())
        .map_err(|_| Diagnostic::resource(0))?;
    queue.extend(0..summaries.len());
    let mut queued = filled(summaries.len(), true)?;
    while let Some(callee) = queue.pop_front() {
        queued[callee] = false;
        let effects = summaries[callee].effects;
        for &caller in &callers[callee] {
            if summaries[caller].effects.merge(effects) && !queued[caller] {
                queue.push_back(caller);
                queued[caller] = true;
            }
        }
    }
    Ok(summaries)
}
