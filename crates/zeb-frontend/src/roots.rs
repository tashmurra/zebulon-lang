//! Explicit source entry roots for the closed, direct-call scalar profile.
use crate::{
    Diagnostic,
    ir::{Operation, Program},
    ir_verify,
    parser::Ast,
};

pub fn retained(ast: &Ast, program: &Program, roots: &[usize]) -> Result<Vec<bool>, Diagnostic> {
    ir_verify::verify(ast, program)?;
    if roots.is_empty() || roots.iter().any(|&root| root >= program.functions.len()) {
        return Err(Diagnostic::new(
            "native-roots",
            "native roots must be nonempty valid function indices",
            0,
        ));
    }
    let mut retained = Vec::new();
    retained
        .try_reserve(program.functions.len())
        .map_err(|_| Diagnostic::resource(0))?;
    retained.resize(program.functions.len(), false);
    let mut pending = Vec::new();
    pending
        .try_reserve(program.functions.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for &root in roots {
        if !retained[root] {
            retained[root] = true;
            pending.push(root);
        }
    }
    while let Some(function) = pending.pop() {
        // Include even flow-infeasible/disconnected calls. This deliberately over-retains;
        // function removal does not depend on a second control-flow proof.
        for block in &program.functions[function].blocks {
            for instruction in &block.instructions {
                match &instruction.operation {
                    Operation::Call {
                        function: callee, ..
                    } => {
                        if !retained[*callee] {
                            retained[*callee] = true;
                            pending.push(*callee);
                        }
                    }
                    Operation::CallMethod { .. } | Operation::GetProperty(..) => {
                        for (callee, live) in retained.iter_mut().enumerate() {
                            if !*live {
                                *live = true;
                                pending.push(callee);
                            }
                        }
                    }
                    Operation::Builtin { .. }
                    | Operation::EmitLiteral(_)
                    | Operation::EmitValue(_)
                    | Operation::PropertyValue(_)
                    | Operation::FunctionValue(_)
                    | Operation::Enumerator(_)
                    | Operation::StringLiteral(_)
                    | Operation::NamedObject(_)
                    | Operation::SetProperty(..)
                    | Operation::BeginStatic(..)
                    | Operation::EndStatic
                    | Operation::Constant(_)
                    | Operation::Load(_)
                    | Operation::Store(..)
                    | Operation::Reset(_)
                    | Operation::Unary(..)
                    | Operation::Binary(..)
                    | Operation::LogicalGuard(_) => {}
                }
            }
        }
    }
    Ok(retained)
}
