//! Bounded all-integer call profiles. The general checked entry is never replaced.
use crate::{
    Diagnostic,
    flow::{self, FunctionFacts},
    ir::{Program, Terminator},
    parser::Ast,
};

pub const MAX_PARAMETERS: usize = 16;
pub const MAX_FUNCTION_COST: usize = 256;
pub const MAX_CLONES: usize = 64;
pub const MAX_TOTAL_COST: usize = 4096;
#[derive(Debug)]
pub struct Candidate {
    pub facts: Option<FunctionFacts>,
    pub reason: &'static str,
    pub cost: usize,
}
fn filled<T: Clone>(n: usize, value: T) -> Result<Vec<T>, Diagnostic> {
    let mut out = Vec::new();
    out.try_reserve(n).map_err(|_| Diagnostic::resource(0))?;
    out.resize(n, value);
    Ok(out)
}
/// Infer profiles under mutual all-integer assumptions, then monotonically discard
/// invalid/unaffordable candidates and recheck callers. No invalid assumption survives.
pub fn plan(
    ast: &Ast,
    program: &Program,
    retained: Option<&[bool]>,
) -> Result<Vec<Candidate>, Diagnostic> {
    crate::ir_verify::verify(ast, program)?;
    if retained.is_some_and(|r| r.len() != program.functions.len()) {
        return Err(Diagnostic::new(
            "specialization-profile",
            "wrong retention inventory",
            0,
        ));
    }
    let effects = crate::effects::analyze(ast, program)?;
    let mut active = filled(program.functions.len(), true)?;
    let mut reasons = filled(program.functions.len(), "all-integer profile")?;
    let mut costs = filled(program.functions.len(), 0usize)?;
    let mut count = 0;
    let mut total = 0;
    for (i, f) in program.functions.iter().enumerate() {
        let params = f.slots.iter().filter(|s| s.parameter).count();
        let cost = f
            .blocks
            .iter()
            .try_fold(params, |n, b| {
                n.checked_add(b.instructions.len())
                    .and_then(|x| x.checked_add(1))
            })
            .ok_or(Diagnostic::resource(0))?;
        costs[i] = cost;
        let reason = if retained.is_some_and(|r| !r[i]) {
            Some("not retained")
        } else if effects[i]
            .effects
            .contains(crate::effects::Effect::SourceException)
        {
            Some("source exception value requires general outcome")
        } else if effects[i]
            .effects
            .contains(crate::effects::Effect::Allocate)
        {
            Some("owned argument or runtime storage requires general values")
        } else if params > MAX_PARAMETERS || cost > MAX_FUNCTION_COST {
            Some("function budget")
        } else if count == MAX_CLONES || total + cost > MAX_TOTAL_COST {
            Some("module budget")
        } else {
            None
        };
        if let Some(reason) = reason {
            active[i] = false;
            reasons[i] = reason;
        } else {
            count += 1;
            total += cost;
        }
    }
    loop {
        let mut removed = false;
        for (i, f) in program.functions.iter().enumerate() {
            if !active[i] {
                continue;
            }
            match flow::check_function(ast, f, true, &active) {
                Ok(facts) => {
                    let mut returns = false;
                    let valid = f.blocks.iter().enumerate().all(|(b, block)| {
                        if !facts.reachable[b] {
                            return true;
                        }
                        match block.terminator {
                            Terminator::Return(v) => {
                                returns = true;
                                facts.value_types[v.0] == 1
                            }
                            _ => true,
                        }
                    });
                    if !valid || !returns {
                        active[i] = false;
                        reasons[i] = "return is not exclusively integer";
                        removed = true;
                    }
                }
                Err(error) if error.code == "resource-exhausted" => return Err(error),
                Err(_) => {
                    active[i] = false;
                    reasons[i] = "integer parameters do not satisfy body";
                    removed = true;
                }
            }
        }
        if !removed {
            break;
        }
    }
    let mut result = Vec::new();
    result
        .try_reserve(program.functions.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for (i, f) in program.functions.iter().enumerate() {
        let facts = if active[i] {
            Some(flow::check_function(ast, f, true, &active)?)
        } else {
            None
        };
        result.push(Candidate {
            facts,
            reason: reasons[i],
            cost: costs[i],
        });
    }
    Ok(result)
}
