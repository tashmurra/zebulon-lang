//! Structural validation before analysis or native emission trusts scalar IR.
use crate::{
    Diagnostic,
    ir::{Block, BranchMode, Failure, Operation, Program, Terminator, Value},
    parser::{Ast, Id, Syntax},
};

fn error(ast: &Ast, site: Id, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        "ir-invalid",
        message,
        ast.nodes.get(site.0).map_or(0, |node| node.start),
    )
}
fn filled<T: Clone>(n: usize, value: T) -> Result<Vec<T>, Diagnostic> {
    let mut v = Vec::new();
    v.try_reserve_exact(n)
        .map_err(|_| Diagnostic::resource(0))?;
    v.resize(n, value);
    Ok(v)
}
fn push<T>(v: &mut Vec<T>, item: T) -> Result<(), Diagnostic> {
    v.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    v.push(item);
    Ok(())
}
fn successors(block: &Block) -> [Option<usize>; 2] {
    match block.terminator {
        Terminator::Invoke { normal, exception } => [Some(normal.0), Some(exception.target.0)],
        Terminator::Throw(_, _, Some(edge)) | Terminator::Resume(Some(edge)) => {
            [Some(edge.target.0), None]
        }
        Terminator::Jump(b) => [Some(b.0), None],
        Terminator::Branch { yes, no, .. } => [Some(yes.0), Some(no.0)],
        _ => [None, None],
    }
}
fn operands(
    op: &Operation,
    mut visit: impl FnMut(Value) -> Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
    if let Some(value) = op.computed_property() {
        visit(value)?;
    }
    match op {
        Operation::EmitValue(v)
        | Operation::BeginStatic(v, _)
        | Operation::GetProperty(v, _)
        | Operation::Store(_, v)
        | Operation::Unary(_, v)
        | Operation::LogicalGuard(v) => visit(*v)?,
        Operation::Binary(_, a, b) | Operation::SetProperty(a, _, b) => {
            visit(*a)?;
            visit(*b)?;
        }
        Operation::CallMethod {
            receiver,
            arguments,
            ..
        } => {
            visit(*receiver)?;
            for v in arguments {
                visit(*v)?;
            }
        }
        Operation::Builtin { arguments, .. } | Operation::Call { arguments, .. } => {
            for v in arguments {
                visit(*v)?;
            }
        }
        _ => {}
    }
    Ok(())
}
// Reverse postorder and immediate dominators use linear storage; no block-squared matrix.
fn dominators(blocks: &[Block]) -> Result<Vec<Option<usize>>, Diagnostic> {
    let mut seen = filled(blocks.len(), false)?;
    let mut post = Vec::new();
    let mut stack = Vec::new();
    push(&mut stack, (0usize, false))?;
    while let Some((b, finish)) = stack.pop() {
        if finish {
            push(&mut post, b)?;
            continue;
        }
        if seen[b] {
            continue;
        }
        seen[b] = true;
        push(&mut stack, (b, true))?;
        for next in successors(&blocks[b]).into_iter().flatten() {
            if !seen[next] {
                push(&mut stack, (next, false))?;
            }
        }
    }
    post.reverse();
    let mut rank = filled(blocks.len(), usize::MAX)?;
    for (i, b) in post.iter().enumerate() {
        rank[*b] = i;
    }
    let mut predecessors = filled(blocks.len(), Vec::new())?;
    for b in &post {
        for next in successors(&blocks[*b]).into_iter().flatten() {
            push(&mut predecessors[next], *b)?;
        }
    }
    let mut parent: Vec<Option<usize>> = filled(blocks.len(), None)?;
    parent[0] = Some(0);
    loop {
        let mut changed = false;
        for b in post.iter().skip(1) {
            let mut candidate: Option<usize> = None;
            for pred in &predecessors[*b] {
                if parent[*pred].is_none() {
                    continue;
                }
                candidate = Some(match candidate {
                    None => *pred,
                    Some(mut a) => {
                        let mut c = *pred;
                        while a != c {
                            if rank[a] > rank[c] {
                                a = parent[a].expect("known dominator chain");
                            } else {
                                c = parent[c].expect("known dominator chain");
                            }
                        }
                        a
                    }
                });
            }
            if parent[*b] != candidate {
                parent[*b] = candidate;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(parent)
}

// Derive source membership from AST edges, never from mutable IR annotations.
fn source_owners(ast: &Ast) -> Result<Vec<Option<usize>>, Diagnostic> {
    let mut owners = filled(ast.nodes.len(), None)?;
    let mut pending = Vec::new();
    for (function, root) in ast.functions.iter().enumerate() {
        push(&mut pending, *root)?;
        while let Some(site) = pending.pop() {
            let Some(node) = ast.nodes.get(site.0) else {
                return Err(error(ast, site, "invalid function source tree"));
            };
            if let Some(owner) = owners[site.0] {
                if owner != function {
                    return Err(error(ast, site, "source node shared by another function"));
                }
                continue;
            }
            owners[site.0] = Some(function);
            let mut result = Ok(());
            crate::sema::children(&node.syntax, &mut |child| {
                if result.is_ok() {
                    result = if child.0 >= site.0 {
                        Err(error(ast, site, "invalid non-topological source tree"))
                    } else {
                        push(&mut pending, child)
                    };
                }
            });
            result?;
        }
    }
    Ok(owners)
}

pub fn verify(ast: &Ast, program: &Program) -> Result<(), Diagnostic> {
    let literal_count = crate::string_pool::collect(ast)?.text.len();
    if program.functions.len() != ast.functions.len() {
        return Err(error(ast, Id(0), "function inventory differs from source"));
    }
    let owners = source_owners(ast)?;
    for (fi, f) in program.functions.iter().enumerate() {
        let fail = |message| error(ast, f.source, message);
        if ast.functions[fi] != f.source {
            return Err(fail("function source identity mismatch"));
        }
        let Some(Syntax::Function { parameters, .. }) =
            ast.nodes.get(f.source.0).map(|n| &n.syntax)
        else {
            return Err(fail("invalid function source"));
        };
        if f.blocks.is_empty() {
            return Err(fail("missing function entry"));
        }
        let parameter_count = f.slots.iter().filter(|s| s.parameter).count();
        // A capturing callback takes its environment as a synthetic first
        // parameter, declared by the analysis ; only callbacks may.
        let Some(Syntax::Function { name, .. }) = ast.nodes.get(f.source.0).map(|n| &n.syntax)
        else {
            return Err(fail("invalid function source"));
        };
        let callback = name.starts_with("$callback");
        if !(parameter_count == parameters.len()
            || callback && parameter_count == parameters.len() + 1)
            || f.slots.iter().take(parameter_count).any(|s| !s.parameter)
        {
            return Err(fail("parameter slots must form the declared prefix"));
        }
        for (ordinal, slot) in f.slots.iter().take(parameter_count).enumerate() {
            if slot.declaration != Some((f.source, ordinal)) {
                return Err(fail("parameter source order mismatch"));
            }
        }
        let mut declarations = std::collections::HashSet::new();
        declarations
            .try_reserve(f.slots.len())
            .map_err(|_| Diagnostic::resource(0))?;
        for slot in &f.slots {
            if let Some((site, ordinal)) = slot.declaration {
                if !declarations.insert((site.0, ordinal)) {
                    return Err(fail("duplicate source slot declaration"));
                }
                let valid = if slot.parameter {
                    // The environment parameter of a capturing callback has no
                    // source parameter of its own.
                    site == f.source && ordinal < parameters.len() + usize::from(callback)
                } else {
                    matches!(ast.nodes.get(site.0).map(|n|&n.syntax),Some(Syntax::Local(items) | Syntax::OwnedLocal(items)) if ordinal<items.len())
                };
                if valid && owners[site.0] != Some(fi) {
                    return Err(fail("slot source belongs to another function"));
                }
                if !valid {
                    return Err(fail("invalid slot source declaration"));
                }
            } else if slot.parameter {
                return Err(fail("parameter lacks a source declaration"));
            }
        }
        let mut definitions = filled(f.values, None)?;
        for (bi, block) in f.blocks.iter().enumerate() {
            if block.site.0 >= ast.nodes.len() {
                return Err(fail("invalid block source site"));
            }
            if owners[block.site.0] != Some(fi) {
                return Err(fail("block source belongs to another function"));
            }
            if let Some(edge) = block.terminator.exception()
                && edge.slot.0 >= f.slots.len()
            {
                return Err(fail("exception slot is out of range"));
            }
            for target in successors(block).into_iter().flatten() {
                if target >= f.blocks.len() {
                    return Err(fail("block target is out of range"));
                }
            }
            for (ii, i) in block.instructions.iter().enumerate() {
                let fail = |message| error(ast, i.site, message);
                if i.site.0 >= ast.nodes.len() {
                    return Err(fail("invalid instruction source site"));
                }
                if owners[i.site.0] != Some(fi) {
                    return Err(fail("instruction source belongs to another function"));
                }
                let produces = !matches!(
                    i.operation,
                    Operation::Store(..)
                        | Operation::Reset(..)
                        | Operation::LogicalGuard(..)
                        | Operation::SetProperty(..)
                        | Operation::BeginStatic(..)
                        | Operation::EndStatic
                        | Operation::EmitLiteral(_)
                        | Operation::EmitValue(_)
                );
                if produces != i.result.is_some() {
                    return Err(fail("operation result shape mismatch"));
                }
                let checked = matches!(
                    i.operation,
                    Operation::Unary(..)
                        | Operation::Binary(..)
                        | Operation::LogicalGuard(..)
                        | Operation::Call { .. }
                        | Operation::Builtin { .. }
                        | Operation::CallMethod { .. }
                        | Operation::NamedObject(_)
                        | Operation::StringLiteral(_)
                        | Operation::GetProperty(..)
                        | Operation::SetProperty(..)
                        | Operation::BeginStatic(..)
                        | Operation::EndStatic
                        | Operation::EmitLiteral(_)
                        | Operation::EmitValue(_)
                );
                if (i.failure == Failure::Propagate) != checked {
                    return Err(fail("operation failure edge mismatch"));
                }
                if let Some(v) = i.result {
                    let Some(definition) = definitions.get_mut(v.0) else {
                        return Err(fail("value definition is out of range"));
                    };
                    if definition.replace((bi, ii)).is_some() {
                        return Err(fail("duplicate value definition"));
                    }
                }
                match &i.operation {
                    Operation::Builtin { kind, arguments }
                        if !match kind {
                            crate::sema::Builtin::NewOwnedCollection
                            | crate::sema::Builtin::NewLocalStringBuffer
                            | crate::sema::Builtin::NewLocalLookup => arguments.is_empty(),
                            crate::sema::Builtin::LookupSet | crate::sema::Builtin::IndexSet => {
                                arguments.len() == 3
                            }
                            crate::sema::Builtin::LookupDefault
                            | crate::sema::Builtin::LookupBuckets
                            | crate::sema::Builtin::LookupLength => arguments.len() == 1,
                            crate::sema::Builtin::LookupContains
                            | crate::sema::Builtin::LookupSetDefault => arguments.len() == 2,
                            crate::sema::Builtin::NewLocalOwnedCollection => arguments.is_empty(),
                            crate::sema::Builtin::VectorInsert => arguments.len() >= 3,
                            crate::sema::Builtin::VectorRemoveRange => arguments.len() == 3,
                            crate::sema::Builtin::VectorSort => arguments.len() == 3,
                            crate::sema::Builtin::VectorSortBegin
                            | crate::sema::Builtin::VectorSortNext
                            | crate::sema::Builtin::VectorSortElement => arguments.len() == 2,
                            crate::sema::Builtin::CallQualified
                            | crate::sema::Builtin::CallDelegated => arguments.len() >= 3,
                            crate::sema::Builtin::ArgumentPack
                            | crate::sema::Builtin::ArgumentPackTail => arguments.len() >= 2,
                            crate::sema::Builtin::ApplyQualified
                            | crate::sema::Builtin::ApplyMethod => arguments.len() == 4,
                            crate::sema::Builtin::ApplyConstructor => arguments.len() == 3,
                            crate::sema::Builtin::MoveOwnedTo => arguments.len() == 3,
                            crate::sema::Builtin::MoveFieldOwnerTo => arguments.len() == 4,
                            crate::sema::Builtin::MoveLocalToCollection => arguments.len() == 2,
                            crate::sema::Builtin::NewLocalLookupSized => arguments.len() == 2,
                            crate::sema::Builtin::NewStaticLookup => arguments.len() == 2,
                            crate::sema::Builtin::NewStaticVector
                            | crate::sema::Builtin::NewStaticRexPattern => arguments.len() == 3,
                            crate::sema::Builtin::OwnedToString
                            | crate::sema::Builtin::OwnedToList => arguments.len() == 1,
                            crate::sema::Builtin::NewLocalVector
                            | crate::sema::Builtin::OwnedLength => arguments.len() == 1,
                            crate::sema::Builtin::VectorAppend
                            | crate::sema::Builtin::VectorIndexOf
                            | crate::sema::Builtin::VectorRemoveAt
                            | crate::sema::Builtin::ReserveInCollection
                            | crate::sema::Builtin::RemoveOwned
                            | crate::sema::Builtin::OwnedAt
                            | crate::sema::Builtin::OfKind => arguments.len() == 2,
                            // the enumeration flag is optional and
                            // means instances when left off.
                            crate::sema::Builtin::FirstObj => (1..=2).contains(&arguments.len()),
                            crate::sema::Builtin::NextObj => (2..=3).contains(&arguments.len()),
                            crate::sema::Builtin::PropDefined => (2..=3).contains(&arguments.len()),
                            crate::sema::Builtin::BeginIteration => arguments.len() == 1,
                            crate::sema::Builtin::AdvanceIteration
                            | crate::sema::Builtin::IterationValue
                            | crate::sema::Builtin::EndIteration => arguments.is_empty(),
                            crate::sema::Builtin::BeginScope
                            | crate::sema::Builtin::PendingValue
                            | crate::sema::Builtin::PendingCode
                            | crate::sema::Builtin::PendingOwner
                            | crate::sema::Builtin::PendingSite => arguments.is_empty(),
                            crate::sema::Builtin::EndScope | crate::sema::Builtin::UnwindScope => {
                                arguments.len() == 1
                            }
                            crate::sema::Builtin::RestorePending => arguments.len() == 4,
                            crate::sema::Builtin::ClaimException
                            | crate::sema::Builtin::DetachException => arguments.len() == 1,
                            crate::sema::Builtin::Invoke => !arguments.is_empty(),
                            crate::sema::Builtin::Apply => arguments.len() == 2,
                            crate::sema::Builtin::BeginPublishedConstruction
                            | crate::sema::Builtin::FinishLocalConstruction
                            | crate::sema::Builtin::FinishException
                            | crate::sema::Builtin::FinishPublishedConstruction
                            | crate::sema::Builtin::BeginConstruction => arguments.len() == 1,
                            crate::sema::Builtin::MoveLocalToField
                            | crate::sema::Builtin::ReserveConstruction
                            | crate::sema::Builtin::FinishConstruction => arguments.len() == 3,
                            crate::sema::Builtin::DictionaryAdd
                            | crate::sema::Builtin::DictionaryRemove => arguments.len() == 4,
                            crate::sema::Builtin::DictionaryFind => {
                                (2..=3).contains(&arguments.len())
                            }
                            crate::sema::Builtin::DictionaryDefined => arguments.len() == 2,
                            crate::sema::Builtin::GrammarParse
                            | crate::sema::Builtin::GrammarBegin => arguments.len() == 4,
                            // dictionary, entity, property; and the same
                            // with the word between them.
                            crate::sema::Builtin::VocabularyWords => arguments.len() == 3,
                            crate::sema::Builtin::VocabularyNames => arguments.len() == 4,
                            crate::sema::Builtin::GrammarFinish => arguments.len() == 1,
                            crate::sema::Builtin::ConstantList => arguments.len() == 1,
                            crate::sema::Builtin::List => true,
                            crate::sema::Builtin::Length => arguments.len() == 1,
                            crate::sema::Builtin::ListIndex => arguments.len() == 2,
                            crate::sema::Builtin::CaptureClosure => arguments.len() == 3,
                            crate::sema::Builtin::ClosureCapture => arguments.len() == 2,
                            crate::sema::Builtin::DataType | crate::sema::Builtin::GetTime => {
                                arguments.len() == 1
                            }
                            crate::sema::Builtin::InputKey
                            | crate::sema::Builtin::InputLine
                            | crate::sema::Builtin::FlushOutput => arguments.is_empty(),
                            crate::sema::Builtin::OwnText => arguments.len() == 1,
                            crate::sema::Builtin::UseLifetimes
                            | crate::sema::Builtin::Savepoint
                            | crate::sema::Builtin::BeginTurn
                            | crate::sema::Builtin::EndTurn
                            | crate::sema::Builtin::TurnJournalLength
                            | crate::sema::Builtin::Undo
                            | crate::sema::Builtin::UndoDepth => arguments.is_empty(),
                            crate::sema::Builtin::Despawn
                            | crate::sema::Builtin::Event
                            | crate::sema::Builtin::EventValue
                            | crate::sema::Builtin::SnapshotToken
                            | crate::sema::Builtin::EventSubject
                            | crate::sema::Builtin::HostAction
                            | crate::sema::Builtin::EngineQuery => arguments.len() == 1,
                            crate::sema::Builtin::RelationGet
                            | crate::sema::Builtin::RelationAll
                            | crate::sema::Builtin::RelationOutermost
                            | crate::sema::Builtin::RelationDescendants
                            | crate::sema::Builtin::RelationAncestors => {
                                (2..=3).contains(&arguments.len())
                            }
                            // a labelled relation carries one more
                            // argument, naming which table the row is in.
                            crate::sema::Builtin::RelationSet
                            | crate::sema::Builtin::RelationUnset
                            | crate::sema::Builtin::RelationContains => {
                                (3..=4).contains(&arguments.len())
                            }
                            crate::sema::Builtin::ReplaceOwner => arguments.len() == 2,
                            crate::sema::Builtin::ToLower | crate::sema::Builtin::ToUpper => {
                                arguments.len() == 1
                            }
                            crate::sema::Builtin::StartsWith | crate::sema::Builtin::EndsWith => {
                                arguments.len() == 2
                            }
                            crate::sema::Builtin::Htmlify => (1..=2).contains(&arguments.len()),
                            crate::sema::Builtin::FindText => (2..=3).contains(&arguments.len()),
                            crate::sema::Builtin::FindReplace => (3..=6).contains(&arguments.len()),
                            crate::sema::Builtin::Add => arguments.len() == 2,
                            crate::sema::Builtin::RexGroup => arguments.len() == 1,
                            crate::sema::Builtin::RexReplace => (3..=4).contains(&arguments.len()),
                            crate::sema::Builtin::Substr
                            | crate::sema::Builtin::RexMatch
                            | crate::sema::Builtin::RexSearch => (2..=3).contains(&arguments.len()),
                            _ => (1..=2).contains(&arguments.len()),
                        } =>
                    {
                        return Err(fail("invalid string intrinsic arity"));
                    }
                    Operation::StringLiteral(index) | Operation::EmitLiteral(index)
                        if *index as usize >= literal_count =>
                    {
                        return Err(fail("string literal is out of range"));
                    }
                    Operation::NamedObject(index) if *index as usize >= ast.objects.len() => {
                        return Err(fail("named object is out of range"));
                    }
                    Operation::Load(slot) | Operation::Store(slot, _) | Operation::Reset(slot)
                        if slot.0 >= f.slots.len() =>
                    {
                        return Err(fail("slot is out of range"));
                    }
                    Operation::Call {
                        function,
                        arguments,
                    } => {
                        let Some(callee) = program.functions.get(*function) else {
                            return Err(fail("call target is out of range"));
                        };
                        let Some(Syntax::Function {
                            parameters,
                            rest,
                            optional,
                            ..
                        }) = ast.nodes.get(callee.source.0).map(|n| &n.syntax)
                        else {
                            return Err(fail("invalid callee source"));
                        };
                        if if *rest {
                            arguments.len() < parameters.len().saturating_sub(1 + optional)
                        } else {
                            parameters.len() != arguments.len()
                        } {
                            return Err(fail("call arity mismatch"));
                        }
                    }
                    _ => {}
                }
            }
        }
        if definitions.iter().any(Option::is_none) {
            return Err(fail("declared value has no definition"));
        }
        let parents = dominators(&f.blocks)?;
        for (bi, block) in f.blocks.iter().enumerate() {
            let use_value = |v: Value, ii: usize, site: Id| -> Result<(), Diagnostic> {
                let Some(Some((definition, position))) = definitions.get(v.0) else {
                    return Err(error(ast, site, "operand has no definition"));
                };
                if *definition == bi {
                    if *position >= ii {
                        return Err(error(ast, site, "value used before definition"));
                    }
                } else if parents[bi].is_some() {
                    let mut current = bi;
                    while current != *definition {
                        let parent = parents[current].expect("reachable dominator chain");
                        if parent == current {
                            return Err(error(ast, site, "definition does not dominate use"));
                        }
                        current = parent;
                    }
                }
                Ok(())
            };
            for (ii, i) in block.instructions.iter().enumerate() {
                operands(&i.operation, |v| use_value(v, ii, i.site))?;
            }
            match block.terminator {
                Terminator::Throw(v, _, _) | Terminator::Return(v) => {
                    use_value(v, block.instructions.len(), block.site)?
                }
                Terminator::Branch {
                    condition, mode, ..
                } => {
                    use_value(condition, block.instructions.len(), block.site)?;
                    if mode == BranchMode::Logical
                        && !block.instructions.iter().any(
                            |i| matches!(i.operation,Operation::LogicalGuard(v) if v==condition),
                        )
                    {
                        return Err(error(ast, block.site, "logical branch lacks its guard"));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
