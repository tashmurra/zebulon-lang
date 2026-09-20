//! Restore child-before-parent order after deferred anonymous body parsing.
use crate::{
    Diagnostic,
    parser::{Ast, Id, Node, Syntax},
};
fn remap(syntax: &mut Syntax, visit: &mut impl FnMut(&mut Id)) {
    match syntax {
        Syntax::Move(id)
        | Syntax::Delegated(id)
        | Syntax::OwnedCall(id)
        | Syntax::Group(id)
        | Syntax::ExpandedArgument(id)
        | Syntax::Unary(_, id, _)
        | Syntax::Expression(id) => visit(id),
        Syntax::Binary(_, a, b)
        | Syntax::While(a, b)
        | Syntax::DoWhile(a, b)
        | Syntax::LocalLookupSized(a, b)
        | Syntax::IndirectProperty(a, b)
        | Syntax::Index(a, b) => {
            visit(a);
            visit(b);
        }
        Syntax::StaticLookup { owner, property } => {
            visit(owner);
            visit(property);
        }
        Syntax::StaticVector {
            capacity,
            owner,
            property,
        } => {
            visit(capacity);
            visit(owner);
            visit(property);
        }
        Syntax::StaticRexPattern {
            text,
            owner,
            property,
        } => {
            visit(text);
            visit(owner);
            visit(property);
        }
        Syntax::Lookup(entries) => {
            for (key, value) in entries {
                if let Some(key) = key {
                    visit(key);
                }
                visit(value);
            }
        }
        Syntax::Membership {
            value, candidates, ..
        } => {
            visit(value);
            for candidate in candidates {
                visit(candidate);
            }
        }
        Syntax::PublishedConstruction {
            prototype,
            arguments,
        }
        | Syntax::LocalConstruction {
            prototype,
            arguments,
        }
        | Syntax::ExceptionConstruction {
            prototype,
            arguments,
        } => {
            visit(prototype);
            for argument in arguments {
                visit(argument);
            }
        }
        Syntax::Construction {
            prototype,
            arguments,
            owner,
            property,
        } => {
            visit(prototype);
            visit(owner);
            visit(property);
            for argument in arguments {
                visit(argument);
            }
        }
        Syntax::Label(_, body) => visit(body),
        Syntax::Apply(target, arguments) => {
            visit(target);
            visit(arguments);
        }
        Syntax::Finally(body, finalizer) => {
            visit(body);
            visit(finalizer);
        }
        Syntax::Try(body, catches) => {
            visit(body);
            for (class, declaration, handler) in catches {
                visit(class);
                visit(declaration);
                visit(handler);
            }
        }
        Syntax::ForEach(declaration, collection, body) => {
            visit(collection);
            visit(declaration);
            visit(body);
        }
        Syntax::For {
            init,
            condition,
            step,
            body,
        } => {
            for initializer in init {
                visit(initializer);
            }
            if let Some(condition) = condition {
                visit(condition);
            }
            if let Some(step) = step {
                visit(step);
            }
            visit(body);
        }
        Syntax::Switch(selector, arms) => {
            visit(selector);
            for (case, statements) in arms {
                if let Some(case) = case {
                    visit(case);
                }
                for statement in statements {
                    visit(statement);
                }
            }
        }
        Syntax::Conditional(a, b, c) => {
            visit(a);
            visit(b);
            visit(c);
        }
        Syntax::If(a, b, c) => {
            visit(a);
            visit(b);
            if let Some(c) = c {
                visit(c);
            }
        }
        Syntax::List(parts)
        | Syntax::ArgumentPack(parts)
        | Syntax::ArgumentPackTail(parts)
        | Syntax::Interpolation { parts, .. } => {
            for part in parts {
                visit(part);
            }
        }
        Syntax::Property(object, _) => visit(object),
        Syntax::Object { properties, .. } => {
            for (_, value) in properties {
                visit(value);
            }
        }
        Syntax::Call(a, args) => {
            visit(a);
            for arg in args {
                visit(arg);
            }
        }
        Syntax::Block(items) => {
            for item in items {
                visit(item);
            }
        }
        Syntax::Local(items) | Syntax::OwnedLocal(items) => {
            for (_, item) in items {
                if let Some(item) = item {
                    visit(item);
                }
            }
        }
        Syntax::Throw(id)
        | Syntax::Return(Some(id))
        | Syntax::OwnerScope(id)
        | Syntax::LocalVector(id) => visit(id),
        Syntax::Function { body, .. } => visit(body),
        Syntax::FunctionReference(id) => visit(id),
        Syntax::Declaration(_)
        | Syntax::Template(_)
        | Syntax::QualifiedInherited(_)
        | Syntax::Name(_)
        | Syntax::PropertyAddress(_)
        | Syntax::Integer(..)
        | Syntax::Decimal(_)
        | Syntax::String(_)
        | Syntax::LocalOwnedCollection
        | Syntax::LocalStringBuffer
        | Syntax::Nil
        | Syntax::True
        | Syntax::Empty
        | Syntax::Return(None)
        | Syntax::NamedBreak(_)
        | Syntax::NamedContinue(_)
        | Syntax::Goto(_)
        | Syntax::Break
        | Syntax::Continue => {}
    }
}

pub(super) fn restore(mut ast: Ast) -> Result<Ast, Diagnostic> {
    let count = ast.nodes.len();
    let mut states = Vec::new();
    states
        .try_reserve_exact(count)
        .map_err(|_| Diagnostic::resource(0))?;
    states.resize(count, 0u8);
    let mut order = Vec::new();
    order
        .try_reserve_exact(count)
        .map_err(|_| Diagnostic::resource(0))?;
    let mut stack = Vec::new();
    for root in 0..count {
        super::push(&mut stack, (Id(root), false), 0)?;
        while let Some((id, after)) = stack.pop() {
            if states[id.0] == 2 {
                continue;
            }
            if after {
                states[id.0] = 2;
                order.push(id);
                continue;
            }
            if states[id.0] == 1 {
                return Err(Diagnostic::new(
                    "parse-ast",
                    "cyclic anonymous function body",
                    ast.nodes[id.0].start,
                ));
            }
            states[id.0] = 1;
            super::push(&mut stack, (id, true), 0)?;
            let mut failure = None;
            crate::sema::children(&ast.nodes[id.0].syntax, &mut |child| {
                if let Err(error) = super::push(&mut stack, (child, false), 0) {
                    failure = Some(error);
                }
            });
            if let Some(error) = failure {
                return Err(error);
            }
            if let Syntax::FunctionReference(child) = ast.nodes[id.0].syntax {
                super::push(&mut stack, (child, false), 0)?;
            }
        }
    }
    let mut mapping = Vec::new();
    mapping
        .try_reserve_exact(count)
        .map_err(|_| Diagnostic::resource(0))?;
    mapping.resize(count, Id(0));
    for (new, old) in order.iter().enumerate() {
        mapping[old.0] = Id(new);
    }
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(count)
        .map_err(|_| Diagnostic::resource(0))?;
    for node in ast.nodes {
        slots.push(Some(node));
    }
    let mut nodes: Vec<Node> = Vec::new();
    nodes
        .try_reserve_exact(count)
        .map_err(|_| Diagnostic::resource(0))?;
    for old in order {
        let mut node = slots[old.0].take().expect("unique syntax order");
        remap(&mut node.syntax, &mut |id| *id = mapping[id.0]);
        nodes.push(node);
    }
    for id in ast.functions.iter_mut().chain(ast.objects.iter_mut()) {
        *id = mapping[id.0];
    }
    // the declared-row table is keyed by node, so it moves with them.
    let mut rows: Vec<Option<crate::parser::DeclaredRow>> = Vec::new();
    rows.try_reserve_exact(count)
        .map_err(|_| Diagnostic::resource(0))?;
    rows.resize(count, None);
    for (old, row) in ast.declared_rows.iter().enumerate() {
        if let Some(row) = row {
            let mut row = *row;
            if let Some(label) = row.label.as_mut() {
                *label = mapping[label.0];
            }
            rows[mapping[old].0] = Some(row);
        }
    }
    ast.declared_rows = rows;
    ast.nodes = nodes;
    Ok(ast)
}
