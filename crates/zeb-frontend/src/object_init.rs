//! Named object initialization lowered to native runtime calls, not interpreted.
//! General function/object-value lowering and inheritance remain separate work.
use crate::{
    Diagnostic,
    parser::{Ast, Id, Syntax},
    sema::{self, Scalar},
};
use std::{collections::HashMap, fmt::Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitialValue {
    Scalar(Scalar),
    Object(u32),
    String(u32),
    List(u32),
    Method(u32),
    Property(u32),
    Enumerator(u32),
    Function(u32),
}
#[derive(Debug)]
pub struct InitialProperty {
    pub id: u32,
    pub value: InitialValue,
    /// Set when the declaration stated a relation row rather than a field: the
    /// packed relation descriptor the row goes into.
    pub row: Option<u64>,
}
#[derive(Debug)]
pub struct InitialObject {
    pub name: String,
    pub is_class: bool,
    pub transient: bool,
    pub is_dictionary: bool,
    pub prototypes: Vec<u32>,
    pub properties: Vec<InitialProperty>,
}
#[derive(Debug)]
pub struct Plan {
    pub objects: Vec<InitialObject>,
    pub properties: Vec<String>,
    pub lists: Vec<Vec<InitialValue>>,
    /// Per AST node: module constant list index for an all-constant list
    /// literal in an ordinary function body.
    pub constant_lists: Vec<Option<u32>>,
}

fn push<T>(values: &mut Vec<T>, value: T) -> Result<(), Diagnostic> {
    values.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    values.push(value);
    Ok(())
}
fn copy(text: &str) -> Result<String, Diagnostic> {
    let mut out = String::new();
    out.try_reserve(text.len())
        .map_err(|_| Diagnostic::resource(0))?;
    out.push_str(text);
    Ok(out)
}

pub fn analyze(ast: &Ast) -> Result<Plan, Diagnostic> {
    if let Some(id) = ast.functions.first() {
        return Err(Diagnostic::new(
            "object-init-unavailable",
            "initialization-only lowering cannot omit source functions",
            ast.nodes.get(id.0).map_or(0, |n| n.start),
        ));
    }
    declarations(ast)
}

pub fn declarations(ast: &Ast) -> Result<Plan, Diagnostic> {
    sema::validate_ast(ast)?;
    let fail = |id: Id, code, message| Diagnostic::new(code, message, ast.nodes[id.0].start);
    let constants = sema::constants(ast)?;
    let enums = sema::enumerators(ast)?;
    let strings = crate::string_pool::collect(ast)?;
    let mut names = HashMap::new();
    names
        .try_reserve(ast.objects.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for (index, id) in ast.objects.iter().enumerate() {
        let Syntax::Object { name, .. } = &ast.nodes[id.0].syntax else {
            unreachable!()
        };
        let index = u32::try_from(index).map_err(|_| Diagnostic::resource(0))?;
        if names.insert(name.as_str(), index).is_some() {
            return Err(fail(*id, "sem-name", "duplicate object name"));
        }
    }
    let mut plan = Plan {
        objects: Vec::new(),
        properties: Vec::new(),
        lists: Vec::new(),
        constant_lists: Vec::new(),
    };
    let mut property_ids = HashMap::new();
    // Collect the complete property namespace before binding forward addresses.
    for node in &ast.nodes {
        let (declared, defined) = match &node.syntax {
            Syntax::Declaration(
                crate::parser::Declaration::Properties(names)
                | crate::parser::Declaration::VocabularyProperties(names),
            ) => (Some(names), None),
            Syntax::Object { properties, .. } => (None, Some(properties)),
            _ => continue,
        };
        for name in declared
            .into_iter()
            .flatten()
            .map(String::as_str)
            .chain(defined.into_iter().flatten().map(|(name, _)| name.as_str()))
        {
            if !property_ids.contains_key(name) {
                let index = u32::try_from(plan.properties.len())
                    .map_err(|_| Diagnostic::resource(node.start))?;
                property_ids
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(node.start))?;
                property_ids.insert(name, index);
                push(&mut plan.properties, copy(name)?)?;
            }
        }
    }
    // Grammar captures name properties even when no object defines them.
    for node in &ast.nodes {
        let Syntax::Declaration(crate::parser::Declaration::Grammar(rule)) = &node.syntax else {
            continue;
        };
        for item in &rule.items {
            if let crate::parser::GrammarItem::Capture(name) = item
                && !property_ids.contains_key(name.as_str())
            {
                let index = u32::try_from(plan.properties.len())
                    .map_err(|_| Diagnostic::resource(node.start))?;
                property_ids
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(node.start))?;
                property_ids.insert(name.as_str(), index);
                push(&mut plan.properties, copy(name)?)?;
            }
        }
    }

    let mut resolved = Vec::new();
    resolved
        .try_reserve_exact(ast.nodes.len())
        .map_err(|_| Diagnostic::resource(0))?;
    resolved.resize(ast.nodes.len(), None);
    for id in &ast.objects {
        let Syntax::Object {
            name,
            is_class,
            transient,
            is_dictionary,
            parents,
            properties,
            ..
        } = &ast.nodes[id.0].syntax
        else {
            unreachable!()
        };
        let mut object = InitialObject {
            name: copy(name)?,
            is_class: *is_class,
            transient: *transient,
            is_dictionary: *is_dictionary,
            prototypes: Vec::new(),
            properties: Vec::new(),
        };
        if parents.len() > 1 && parents.iter().any(|parent| parent == "object") {
            return Err(fail(
                *id,
                "sem-inheritance",
                "object must be the sole superclass",
            ));
        }
        for parent in parents {
            if parent != "object" {
                push(
                    &mut object.prototypes,
                    *names
                        .get(parent.as_str())
                        .ok_or_else(|| fail(*id, "sem-name", "unknown superclass"))?,
                )?;
            }
        }
        let mut seen: HashMap<&str, (bool, bool, Option<&str>)> = HashMap::new();
        seen.try_reserve(properties.len())
            .map_err(|_| Diagnostic::resource(0))?;
        for (name, expression) in properties {
            // a property is defined once — except a row of a
            // many_to_many relation, where several rows is the normal case and
            // the declaration is the only place that could not say so. For the
            // other two cardinalities a second row would displace the first, so
            // writing two is a mistake rather than a shorthand.
            //
            // **Unless the relation is labelled.** A labelled relation is a
            // family of tables, one per label, so two rows under
            // different labels are rows in different tables and neither
            // displaces the other.
            let row = ast.declared_rows[expression.0];
            let declared = row.and_then(|row| ast.relations.get(row.relation));
            // The label's **name**, not the node it was written at: two rows
            // both saying `north` are two different nodes and one table.
            let label =
                row.and_then(|row| row.label)
                    .and_then(|id| match &ast.nodes[id.0].syntax {
                        Syntax::Name(name) => Some(name.as_str()),
                        _ => None,
                    });
            let repeatable =
                declared.is_some_and(|relation| relation.cardinality == 2 || relation.labelled);
            match seen.insert(name.as_str(), (row.is_some(), repeatable, label)) {
                None => {}
                // Two rows of a labelled relation are only distinct if their
                // labels are: north twice is still a displacement.
                Some((_, true, before)) if repeatable && (before.is_none() || before != label) => {}
                Some((was_row, _, _)) => {
                    return Err(fail(
                        *expression,
                        "sem-property",
                        if was_row || row.is_some() {
                            // A second row of a one_to_one or one_to_many
                            // relation would displace the first, so writing two
                            // is a mistake rather than a shorthand — and so
                            // would a second row under a label already used.
                            "a relation states one row per declaration unless it is many_to_many or labelled"
                        } else {
                            "duplicate property definition"
                        },
                    ));
                }
            }
            let property = if let Some(id) = property_ids.get(name.as_str()) {
                *id
            } else {
                let index =
                    u32::try_from(plan.properties.len()).map_err(|_| Diagnostic::resource(0))?;
                property_ids
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(0))?;
                property_ids.insert(name.as_str(), index);
                push(&mut plan.properties, copy(name)?)?;
                index
            };
            let mut expression = *expression;
            while let Syntax::Group(inner) = ast.nodes[expression.0].syntax {
                expression = inner;
            }
            if matches!(&ast.nodes[expression.0].syntax, Syntax::String(literal) if literal.emitting)
            {
                return Err(fail(
                    expression,
                    "output-runtime-unavailable",
                    "emitting object properties require method dispatch",
                ));
            }
            let root = expression;
            let mut pending = Vec::new();
            push(&mut pending, (root, false))?;
            while let Some((expression, finish)) = pending.pop() {
                if resolved[expression.0].is_some() {
                    continue;
                }
                if !finish {
                    match &ast.nodes[expression.0].syntax {
                        Syntax::List(items) => {
                            push(&mut pending, (expression, true))?;
                            for child in items.iter().rev() {
                                push(&mut pending, (*child, false))?;
                            }
                            continue;
                        }
                        Syntax::Group(child) => {
                            push(&mut pending, (expression, true))?;
                            push(&mut pending, (*child, false))?;
                            continue;
                        }
                        _ => {}
                    }
                }
                if matches!(&ast.nodes[expression.0].syntax, Syntax::String(literal) if literal.emitting)
                {
                    return Err(fail(
                        expression,
                        "object-init-unavailable",
                        "emitting text is not a list constant",
                    ));
                }
                let value = if let Syntax::List(items) = &ast.nodes[expression.0].syntax {
                    let mut values = Vec::new();
                    for child in items {
                        push(
                            &mut values,
                            resolved[child.0].ok_or_else(|| {
                                fail(*child, "sem-ast", "unresolved list element")
                            })?,
                        )?;
                    }
                    let index =
                        u32::try_from(plan.lists.len()).map_err(|_| Diagnostic::resource(0))?;
                    push(&mut plan.lists, values)?;
                    InitialValue::List(index)
                } else if let Syntax::Group(child) = ast.nodes[expression.0].syntax {
                    resolved[child.0]
                        .ok_or_else(|| fail(expression, "sem-ast", "unresolved grouped constant"))?
                } else if matches!(ast.nodes[expression.0].syntax, Syntax::Function { .. }) {
                    InitialValue::Method(
                        u32::try_from(
                            ast.functions
                                .iter()
                                .position(|id| *id == expression)
                                .ok_or_else(|| {
                                    fail(expression, "sem-ast", "method has no function entry")
                                })?,
                        )
                        .map_err(|_| Diagnostic::resource(0))?,
                    )
                } else if let Syntax::FunctionReference(function) = &ast.nodes[expression.0].syntax
                {
                    InitialValue::Function(
                        u32::try_from(
                            ast.functions
                                .iter()
                                .position(|id| id == function)
                                .ok_or_else(|| fail(expression, "sem-ast", "missing callback"))?,
                        )
                        .map_err(|_| Diagnostic::resource(0))?,
                    )
                } else if let Syntax::PropertyAddress(name) = &ast.nodes[expression.0].syntax {
                    InitialValue::Property(*property_ids.get(name.as_str()).ok_or_else(|| {
                        fail(expression, "sem-property", "unknown property address")
                    })?)
                } else if let Some(literal) = strings.ids[expression.0] {
                    InitialValue::String(literal)
                } else if let Some(value) = constants[expression.0] {
                    InitialValue::Scalar(value)
                } else if let Syntax::Name(name) = &ast.nodes[expression.0].syntax {
                    if let Some(value) = enums.get(name.as_str()) {
                        InitialValue::Enumerator(*value)
                    } else if let Some(index) = ast.functions.iter().position(|id| matches!(&ast.nodes[id.0].syntax, Syntax::Function { name: function, .. } if function == name)) {
                        InitialValue::Function(u32::try_from(index).map_err(|_| Diagnostic::resource(0))?)
                    } else {
                        InitialValue::Object(*names.get(name.as_str()).ok_or_else(|| {
                            fail(expression, "sem-name", "unknown object initializer name")
                        })?)
                    }
                } else {
                    return Err(fail(
                        expression,
                        "object-init-unavailable",
                        "initializer requires a constant value, list, or named reference",
                    ));
                };
                resolved[expression.0] = Some(value);
            }
            let value =
                resolved[root.0].ok_or_else(|| fail(root, "sem-ast", "unresolved initializer"))?;
            // a relation row rather than a field, which the parser marked
            // on the value it was written with.
            let row = match declared_row(ast, &enums, root) {
                Some(Ok(descriptor)) => Some(descriptor),
                Some(Err((site, message))) => {
                    return Err(fail(site, "object-init-relation", message));
                }
                None => None,
            };
            if row.is_some() && !matches!(value, InitialValue::Object(_)) {
                return Err(fail(
                    root,
                    "object-init-relation",
                    "a declared relation row names an entity",
                ));
            }
            push(
                &mut object.properties,
                InitialProperty {
                    id: property,
                    value,
                    row,
                },
            )?;
        }
        push(&mut plan.objects, object)?;
    }
    let mut colors = Vec::new();
    colors
        .try_reserve(plan.objects.len())
        .map_err(|_| Diagnostic::resource(0))?;
    colors.resize(plan.objects.len(), 0u8);
    let mut stack = Vec::new();
    for index in 0..plan.objects.len() {
        if colors[index] == 2 {
            continue;
        }
        colors[index] = 1;
        push(&mut stack, (index, 0usize))?;
        while let Some((current, next)) = stack.pop() {
            if let Some(parent) = plan.objects[current].prototypes.get(next) {
                let parent = *parent as usize;
                if colors[parent] == 1 {
                    return Err(fail(
                        ast.objects[current],
                        "sem-inheritance",
                        "cyclic superclass graph",
                    ));
                }
                push(&mut stack, (current, next + 1))?;
                if colors[parent] == 0 {
                    colors[parent] = 1;
                    push(&mut stack, (parent, 0))?;
                }
            } else {
                colors[current] = 2;
            }
        }
    }
    if ast.nodes.iter().any(|node| {
        matches!(
            node.syntax,
            Syntax::Construction { .. }
                | Syntax::PublishedConstruction { .. }
                | Syntax::ExceptionConstruction { .. }
                | Syntax::LocalConstruction { .. }
                // parseTokens constructs grammar match objects.
                | Syntax::Declaration(crate::parser::Declaration::Grammar(_))
        )
    }) && !plan.properties.iter().any(|name| name == "construct")
    {
        push(&mut plan.properties, copy("construct")?)?;
    }
    if !plan.properties.iter().any(|name| name == "isClass") {
        push(&mut plan.properties, copy("isClass")?)?;
    }
    // Constant list literals in ordinary function bodies are materialized once as
    // module-owned constants. A literal with any
    // non-constant element keeps ordinary lowering.
    plan.constant_lists
        .try_reserve_exact(ast.nodes.len())
        .map_err(|_| Diagnostic::resource(0))?;
    plan.constant_lists.resize(ast.nodes.len(), None);
    let mut body_nodes = Vec::new();
    for function in &ast.functions {
        let Syntax::Function {
            is_static, body, ..
        } = &ast.nodes[function.0].syntax
        else {
            unreachable!()
        };
        if !*is_static {
            push(&mut body_nodes, *body)?;
        }
    }
    let mut failed = false;
    while let Some(node) = body_nodes.pop() {
        if let Syntax::List(_) = ast.nodes[node.0].syntax {
            let mut constant = true;
            let mut check = Vec::new();
            push(&mut check, node)?;
            while let Some(mut item) = check.pop() {
                while let Syntax::Group(inner) = ast.nodes[item.0].syntax {
                    item = inner;
                }
                match &ast.nodes[item.0].syntax {
                    Syntax::List(items) => {
                        check
                            .try_reserve(items.len())
                            .map_err(|_| Diagnostic::resource(0))?;
                        check.extend_from_slice(items);
                    }
                    Syntax::String(literal) if literal.emitting => constant = false,
                    Syntax::PropertyAddress(name) => {
                        constant &= property_ids.contains_key(name.as_str())
                    }
                    Syntax::Name(name) => {
                        constant &=
                            enums.contains_key(name.as_str()) || names.contains_key(name.as_str())
                    }
                    _ => constant &= strings.ids[item.0].is_some() || constants[item.0].is_some(),
                }
            }
            if constant {
                let mut pending = Vec::new();
                push(&mut pending, (node, false))?;
                while let Some((expression, finish)) = pending.pop() {
                    let mut inner = expression;
                    while let Syntax::Group(child) = ast.nodes[inner.0].syntax {
                        inner = child;
                    }
                    if let (Syntax::List(items), false) = (&ast.nodes[inner.0].syntax, finish) {
                        push(&mut pending, (expression, true))?;
                        for child in items.iter().rev() {
                            push(&mut pending, (*child, false))?;
                        }
                        continue;
                    }
                    let value = match &ast.nodes[inner.0].syntax {
                        Syntax::List(items) => {
                            let mut values = Vec::new();
                            values
                                .try_reserve_exact(items.len())
                                .map_err(|_| Diagnostic::resource(0))?;
                            for child in items {
                                values.push(resolved[child.0].ok_or_else(|| {
                                    fail(*child, "sem-ast", "unresolved list element")
                                })?);
                            }
                            let index = u32::try_from(plan.lists.len())
                                .map_err(|_| Diagnostic::resource(0))?;
                            push(&mut plan.lists, values)?;
                            InitialValue::List(index)
                        }
                        Syntax::PropertyAddress(name) => {
                            InitialValue::Property(property_ids[name.as_str()])
                        }
                        Syntax::Name(name) => match enums.get(name.as_str()) {
                            Some(value) => InitialValue::Enumerator(*value),
                            None => InitialValue::Object(names[name.as_str()]),
                        },
                        _ => match strings.ids[inner.0] {
                            Some(literal) => InitialValue::String(literal),
                            None => InitialValue::Scalar(
                                constants[inner.0].expect("checked constant element"),
                            ),
                        },
                    };
                    resolved[expression.0] = Some(value);
                }
                if let Some(InitialValue::List(index)) = resolved[node.0] {
                    plan.constant_lists[node.0] = Some(index);
                }
                continue;
            }
        }
        crate::sema::children(&ast.nodes[node.0].syntax, &mut |child| {
            if body_nodes.try_reserve(1).is_err() {
                failed = true;
            } else {
                body_nodes.push(child);
            }
        });
        if failed {
            return Err(Diagnostic::resource(0));
        }
    }
    Ok(plan)
}

/// Emits fixed-width C-signature helper calls. Link names are resolved by the
/// matching native bundle; source names never become external linker symbols.
/// The packed descriptor for a relation row stated in a declaration ,
/// or nothing when the property is an ordinary field. The parser marked the row,
/// so `location = study` is a field and only `location(study)` is a row: a
/// relation never takes a name away from a property.
fn declared_row(
    ast: &Ast,
    enums: &std::collections::HashMap<&str, u32>,
    value: Id,
) -> Option<Result<u64, (Id, &'static str)>> {
    let row = ast.declared_rows[value.0]?;
    let relation = ast.relations.get(row.relation)?;
    let mut descriptor = row.relation as u64 | (relation.cardinality << 13);
    match (relation.labelled, row.label) {
        (true, Some(label)) => {
            let Syntax::Name(name) = &ast.nodes[label.0].syntax else {
                return Some(Err((label, "a relation label is an enumerator")));
            };
            let Some(value) = enums.get(name.as_str()) else {
                return Some(Err((label, "a relation label is an enumerator")));
            };
            descriptor |= 1 << 15 | u64::from(*value) << 16;
        }
        (true, None) => {
            return Some(Err((
                value,
                "a labelled relation's row names its label as well as its partner",
            )));
        }
        (false, Some(_)) => {
            return Some(Err((value, "this relation has no label column")));
        }
        (false, None) => {}
    }
    Some(Ok(descriptor | u64::from(row.reversed) << 12))
}

pub fn emit(ast: &Ast) -> Result<String, Diagnostic> {
    analyze(ast)?;
    emit_declarations(ast)
}

pub(crate) fn emit_declarations(ast: &Ast) -> Result<String, Diagnostic> {
    let plan = declarations(ast)?;
    let mut out = String::new();
    // Text emission grows fallibly; source-scale data is never recursive.
    macro_rules! line {
        ($($arg:tt)*) => {{
            let args = format_args!($($arg)*);
            // Write into a fallible adapter rather than String's infallible growth.
            struct Sink<'a>(&'a mut String);
            impl Write for Sink<'_> {
                fn write_str(&mut self, text: &str) -> std::fmt::Result {
                    self.0.try_reserve(text.len()).map_err(|_| std::fmt::Error)?;
                    self.0.push_str(text); Ok(())
                }
            }
            Sink(&mut out).write_fmt(args).map_err(|_| Diagnostic::resource(0))?;
        }};
    }
    line!(
        "declare i64 @zeb_objects_call(i32, i64, i64, i64, i64)\ndeclare i32 @zeb_objects_status()\n\ndefine i32 @zeb_initialize_objects(i64 %session) {{\nentry:\n"
    );
    let mut step = 0usize;
    macro_rules! check_status {
        () => {{
            line!("  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n");
            step += 1;
        }};
    }

    if let Some(property) = plan.properties.iter().position(|name| name == "isClass") {
        line!(
            "  %classProperty = call i64 @zeb_objects_call(i32 19, i64 %session, i64 {property}, i64 0, i64 0)\n"
        );
        line!(
            "  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n"
        );
        step += 1;
    }
    // reserve every declared relation's table before anything writes a
    // row. A labelled relation's tables are allocated after the ones in use, so
    // without this a labelled row written first could be handed the index another
    // relation was going to claim, and the second one would find the wrong
    // cardinality there.
    for (index, relation) in ast.relations.iter().enumerate() {
        if relation.vocabulary {
            // Its rows are dictionary entries, not a table.
            continue;
        }
        let cardinality = relation.cardinality;
        let family = relation
            .label_family
            .as_deref()
            .and_then(|name| crate::sema::enum_family_range(ast, name))
            .map_or(0, |(start, end)| {
                1 | (u64::from(start) << 1) | (u64::from(end) << 14)
            });
        line!(
            "  %relation{index} = call i64 @zeb_objects_call(i32 129, i64 %session, i64 {index}, i64 {cardinality}, i64 {family})\n"
        );
        check_status!();
    }
    // Allocate and bind every named object before assigning any references.
    for (index, object) in plan.objects.iter().enumerate() {
        let create_op = if object.is_dictionary { 34 } else { 1 };
        line!(
            "  %object{index} = call i64 @zeb_objects_call(i32 {create_op}, i64 %session, i64 0, i64 0, i64 0)\n"
        );
        line!(
            "  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n"
        );
        step += 1;
        line!(
            "  %bind{index} = call i64 @zeb_objects_call(i32 10, i64 %session, i64 {index}, i64 %object{index}, i64 0)\n"
        );
        line!(
            "  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n"
        );
        step += 1;
    }
    for (index, items) in plan.lists.iter().enumerate() {
        line!(
            "  %list{index} = call i64 @zeb_objects_call(i32 93, i64 %session, i64 {}, i64 0, i64 0)\n",
            items.len()
        );
        check_status!();
        for value in items {
            let (kind, bits, prefix) = match *value {
                InitialValue::Scalar(Scalar::Nil) => (0, 0, ""),
                InitialValue::Scalar(Scalar::True) => (1, 0, ""),
                InitialValue::Scalar(Scalar::Integer(n)) => (2, n as u32, ""),
                InitialValue::Object(n) => (3, n, "%object"),
                InitialValue::String(n) => (4, n, ""),
                InitialValue::List(n) => (9, n, "%list"),
                InitialValue::Property(n) => (8, n, ""),
                InitialValue::Enumerator(n) => (10, n, ""),
                InitialValue::Function(n) => (11, n, ""),
                InitialValue::Method(_) => {
                    return Err(Diagnostic::new(
                        "object-init-unavailable",
                        "method body is not a list constant",
                        0,
                    ));
                }
            };
            line!(
                "  %append{step} = call i64 @zeb_objects_call(i32 94, i64 %session, i64 %list{index}, i64 {kind}, i64 {prefix}{bits})\n"
            );
            check_status!();
        }
        line!(
            "  %finish{step} = call i64 @zeb_objects_call(i32 38, i64 %session, i64 %list{index}, i64 0, i64 0)\n"
        );
        check_status!();
    }
    for (index, object) in plan.objects.iter().enumerate() {
        if object.transient {
            line!(
                "  %transient{index} = call i64 @zeb_objects_call(i32 90, i64 %session, i64 %object{index}, i64 1, i64 0)\n"
            );
            line!(
                "  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n"
            );
            step += 1;
        }
        if object.is_class {
            line!(
                "  %class{index} = call i64 @zeb_objects_call(i32 18, i64 %session, i64 %object{index}, i64 0, i64 0)\n"
            );
            line!(
                "  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n"
            );
            step += 1;
        }
        for parent in &object.prototypes {
            line!(
                "  %prototype{step} = call i64 @zeb_objects_call(i32 16, i64 %session, i64 %object{index}, i64 %object{parent}, i64 0)\n"
            );
            line!(
                "  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n"
            );
            step += 1;
        }
        for property in &object.properties {
            // a declared relation row, not a field. The row runs the way
            // the name reads it, and the runtime reads the direction out of the
            // descriptor.
            if let Some(descriptor) = property.row {
                let InitialValue::Object(target) = property.value else {
                    return Err(Diagnostic::new(
                        "object-init-relation",
                        "a declared relation row names an entity",
                        0,
                    ));
                };
                line!(
                    "  %row{step} = call i64 @zeb_objects_call(i32 131, i64 %session, i64 {descriptor}, i64 %object{index}, i64 %object{target})\n"
                );
                check_status!();
                continue;
            }
            if let InitialValue::List(list) = property.value {
                line!(
                    "  %write{step} = call i64 @zeb_objects_call(i32 41, i64 %session, i64 %object{index}, i64 {}, i64 %list{list})\n",
                    property.id
                );
                check_status!();
                continue;
            }
            let (op, bits, reference) = match property.value {
                InitialValue::Scalar(Scalar::Nil) => (5, 0, None),
                InitialValue::Scalar(Scalar::True) => (6, 0, None),
                InitialValue::Scalar(Scalar::Integer(value)) => (7, u64::from(value as u32), None),
                InitialValue::Object(object) => (8, 0, Some(object)),
                InitialValue::String(literal) => (13, u64::from(literal), None),
                InitialValue::List(_) => unreachable!(),
                InitialValue::Method(method) => (15, u64::from(method), None),
                InitialValue::Property(property) => (35, u64::from(property), None),
                InitialValue::Enumerator(value) => (42, u64::from(value), None),
                InitialValue::Function(value) => (49, u64::from(value), None),
            };
            if let Some(object) = reference {
                line!(
                    "  %write{step} = call i64 @zeb_objects_call(i32 {op}, i64 %session, i64 %object{index}, i64 {}, i64 %object{object})\n",
                    property.id
                );
            } else {
                line!(
                    "  %write{step} = call i64 @zeb_objects_call(i32 {op}, i64 %session, i64 %object{index}, i64 {}, i64 {bits})\n",
                    property.id
                );
            }
            line!(
                "  %status{step} = call i32 @zeb_objects_status()\n  %ok{step} = icmp eq i32 %status{step}, 0\n  br i1 %ok{step}, label %next{step}, label %fail{step}\nfail{step}:\n  %close{step} = call i64 @zeb_objects_call(i32 9, i64 %session, i64 0, i64 0, i64 0)\n  ret i32 %status{step}\nnext{step}:\n"
            );
            step += 1;
        }
    }
    line!("  ret i32 0\n}}\n");
    Ok(out)
}
