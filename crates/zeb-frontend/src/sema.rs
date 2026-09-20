//! Preliminary binding and pure-constant checks. Flow/native checking is separate.
use crate::{
    Diagnostic,
    parser::{Ast, Id, Syntax},
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scalar {
    Integer(i32),
    Nil,
    True,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Binding {
    Local(usize),
    Function(usize),
    Object(u32),
    Property(u32),
    Enumerator(u32),
    SelfProperty(usize, u32),
    Inherited(usize, u32, u32),
    /// `delegated target`: self slot and the currently executing property.
    Delegated(usize, u32),
    /// A captured value of the enclosing scope, by capture index.
    Captured(usize),
    /// A property of a captured `self`, by capture index and property.
    CapturedSelfProperty(usize, u32),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Builtin {
    Invoke,
    Apply,
    ApplyMethod,
    ArgumentPack,
    /// The expanded list first, then fixed arguments.
    ArgumentPackTail,
    BeginScope,
    UnwindScope,
    PendingValue,
    PendingCode,
    PendingSite,
    PendingOwner,
    ClaimException,
    DetachException,
    RestorePending,
    EndScope,
    OfKind,
    PropDefined,
    FirstObj,
    NextObj,
    BeginIteration,
    AdvanceIteration,
    IterationValue,
    EndIteration,
    NewOwnedCollection,
    NewLocalVector,
    NewStaticVector,
    /// `static new RexPattern(text)`: a compiled pattern owned by a field.
    NewStaticRexPattern,
    NewStaticLookup,
    NewLocalLookup,
    NewLocalLookupSized,
    LookupBuckets,
    NewLocalStringBuffer,
    LookupSet,
    IndexSet,
    LookupDefault,
    LookupSetDefault,
    LookupLength,
    LookupContains,
    VectorAppend,
    VectorIndexOf,
    VectorRemoveRange,
    VectorRemoveAt,
    VectorInsert,
    /// `vector.sort(descending, comparator)`; expanded in IR.
    VectorSort,
    VectorSortBegin,
    VectorSortNext,
    VectorSortElement,
    ReserveInCollection,
    RemoveOwned,
    MoveOwnedTo,
    MoveFieldOwnerTo,
    OwnedLength,
    OwnedAt,
    BeginPublishedConstruction,
    ReserveConstruction,
    MoveLocalToField,
    MoveLocalToCollection,
    NewLocalOwnedCollection,
    FinishPublishedConstruction,
    FinishException,
    FinishLocalConstruction,
    MoveReturnOwner,
    BeginConstruction,
    FinishConstruction,
    DictionaryAdd,
    DictionaryRemove,
    DictionaryFind,
    DictionaryDefined,
    GrammarParse,
    GrammarBegin,
    GrammarFinish,
    ConstantList,
    List,
    ListIndex,
    Length,
    DataType,
    ToString,
    /// any value as text, which a value string's interpolation needs.
    /// `toString` converts a number; this converts whatever it is given.
    ValueToText,
    CaptureClosure,
    ClosureCapture,
    OwnedToString,
    OwnedToList,
    ApplyConstructor,
    CallQualified,
    /// Like CallQualified, but the target need not be an ancestor of self.
    CallDelegated,
    /// `rexMatch(pat, str, index?)`: match length at a position.
    RexMatch,
    /// `rexSearch(pat, str, index?)` and `rexGroup(n)`: owned result lists.
    RexSearch,
    RexGroup,
    /// `rexReplace(pat, str, replacement, flags?)`: owned replaced text.
    RexReplace,
    /// Callback list methods that allocate nothing; expanded in IR.
    IndexWhich,
    ValWhich,
    LastIndexWhich,
    CountWhich,
    ForEachItem,
    /// `subset` and `mapAll` build a new list, so they need an owned result.
    Subset,
    MapAll,
    /// t3vm functions with no state in an ahead-of-time build.
    PreinitMode,
    RunGc,
    DebugTrace,
    GlobalSymbols,
    GetTime,
    Savepoint,
    Undo,
    /// How many cycles can still be undone.
    UndoDepth,
    /// Remove an entity from the world, with every relation row naming it.
    Despawn,
    /// `event(id)` starts a semantic event, `eventSubject(entity)`
    /// names something it concerns.
    Event,
    EventSubject,
    EventValue,
    SnapshotToken,
    /// Ask the host to do something and wait for it to accept.
    HostAction,
    /// Ask the host something only it knows.
    EngineQuery,
    InputKey,
    InputLine,
    FlushOutput,
    OwnText,
    ReplaceOwner,
    RelationSet,
    RelationUnset,
    RelationGet,
    RelationAll,
    /// The words that name an entity, and whether one word does: a vocabulary
    /// relation read as data rather than through a grammar slot.
    VocabularyWords,
    VocabularyNames,
    RelationContains,
    RelationOutermost,
    RelationDescendants,
    RelationAncestors,
    UseLifetimes,
    BeginTurn,
    EndTurn,
    TurnJournalLength,
    StartsWith,
    EndsWith,
    FindText,
    ToLower,
    ToUpper,
    Htmlify,
    FindReplace,
    ApplyQualified,
    ToInteger,
    Substr,
    Add,
}
#[derive(Debug)]
pub struct Local {
    pub function: Id,
    pub declaration: Id,
    pub ordinal: usize,
    pub parameter: bool,
}
#[derive(Debug)]
pub struct Analysis {
    pub bindings: Vec<Option<Binding>>,
    /// For a relation call site, the packed relation descriptor: index in the
    /// low bits, then the direction, then the cardinality.
    pub relations: Vec<Option<u64>>,
    /// For a vocabulary relation's call site, the dictionary object its rows
    /// live in and the vocabulary property they are filed under, so the call
    /// lowers to a dictionary operation rather than a relation table.
    /// the third element is set when a label supplied the property,
    /// in which case the label argument has been consumed and must not also be
    /// lowered as an operand.
    pub vocabulary: Vec<Option<(u32, u32, bool)>>,
    pub constants: Vec<Option<Scalar>>,
    pub locals: Vec<Local>,
    pub calls: Vec<Option<usize>>,
    pub builtins: Vec<Option<Builtin>>,
    pub flow_complete: bool,
    pub properties: Vec<Option<u32>>,
    pub strings: Vec<Option<u32>>,
    /// Module constant list index for constant function-body list literals.
    pub constant_lists: Vec<Option<u32>>,
    /// Property ID of `construct`, called on grammar match objects.
    pub grammar_construct: Option<u32>,
    /// What each anonymous callback captures, by function index.
    /// A capturing callback takes its environment as a synthetic first parameter.
    pub captures: Vec<Vec<Capture>>,
}

/// One captured value of an anonymous callback. A captured `self` is simply the
/// enclosing method's `self`, and a callback nested in another captures through
/// that one's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// An enclosing local, by its slot in `locals`.
    Local(usize),
    /// A capture of the enclosing callback, by its index there.
    Outer(usize),
}

/// The value of one declared enumerator, for a caller outside the frontend that
/// has to name a token in a runtime call (the command cycle).
pub fn enumerator(ast: &Ast, name: &str) -> Option<u32> {
    enumerators(ast).ok()?.get(name).copied()
}

pub(crate) fn enumerators(ast: &Ast) -> Result<HashMap<&str, u32>, Diagnostic> {
    let mut symbols = HashMap::new();
    for node in &ast.nodes {
        if let Syntax::Declaration(crate::parser::Declaration::Enumerators { names, .. }) =
            &node.syntax
        {
            for name in names {
                let value =
                    u32::try_from(symbols.len()).map_err(|_| Diagnostic::resource(node.start))?;
                symbols
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(node.start))?;
                if symbols.insert(name.as_str(), value).is_some() {
                    return Err(Diagnostic::new(
                        "sem-name",
                        "duplicate enumerator",
                        node.start,
                    ));
                }
            }
        }
    }
    for node in &ast.nodes {
        let conflict = match &node.syntax {
            Syntax::Object {
                name, properties, ..
            } => {
                symbols.contains_key(name.as_str())
                    || properties
                        .iter()
                        .any(|(name, _)| symbols.contains_key(name.as_str()))
            }
            Syntax::Function { name, .. } => symbols.contains_key(name.as_str()),
            Syntax::Declaration(
                crate::parser::Declaration::Properties(names)
                | crate::parser::Declaration::VocabularyProperties(names),
            ) => names.iter().any(|name| symbols.contains_key(name.as_str())),
            Syntax::Declaration(crate::parser::Declaration::Intrinsic {
                class,
                signatures,
                ..
            }) => {
                class
                    .as_ref()
                    .is_some_and(|name| symbols.contains_key(name.as_str()))
                    || signatures
                        .iter()
                        .any(|signature| symbols.contains_key(signature.name.as_str()))
            }
            _ => false,
        };
        if conflict {
            return Err(Diagnostic::new(
                "sem-name",
                "enumerator conflicts with global symbol",
                node.start,
            ));
        }
    }
    check_enum_families(ast, &symbols)?;
    Ok(symbols)
}

/// Named enum families occupy contiguous values in this compilation unit.
pub(crate) fn enum_family_range(ast: &Ast, family: &str) -> Option<(u32, u32)> {
    let mut first = 0u32;
    for node in &ast.nodes {
        if let Syntax::Declaration(crate::parser::Declaration::Enumerators {
            names,
            family: declared,
            ..
        }) = &node.syntax
        {
            let end = first.checked_add(u32::try_from(names.len()).ok()?)?;
            if declared.as_deref() == Some(family) {
                return Some((first, end));
            }
            first = end;
        }
    }
    None
}
fn check_label_family(
    ast: &Ast,
    relation: &crate::parser::RelationDecl,
    argument: Id,
) -> Result<(), Diagnostic> {
    let Some(expected) = relation
        .label_family
        .as_deref()
        .filter(|_| !relation.vocabulary)
    else {
        return Ok(());
    };
    if let Syntax::Name(name) = &ast.nodes[ungroup(ast, argument).0].syntax {
        for node in &ast.nodes {
            if let Syntax::Declaration(crate::parser::Declaration::Enumerators {
                names,
                family,
                ..
            }) = &node.syntax
                && names.contains(name)
                && family.as_deref() != Some(expected)
            {
                let mut diagnostic = error(ast, argument, "sem-enum-family", "wrong enum family");
                diagnostic.message = format!(
                    "relation label requires enum family {expected}, but {name} belongs to {}",
                    family.as_deref().unwrap_or("<unnamed>")
                )
                .into();
                return Err(diagnostic);
            }
        }
    }
    Ok(())
}
fn check_enum_families(ast: &Ast, enums: &HashMap<&str, u32>) -> Result<(), Diagnostic> {
    let mut families = std::collections::HashSet::new();
    for node in &ast.nodes {
        if let Syntax::Declaration(crate::parser::Declaration::Enumerators {
            family: Some(family),
            ..
        }) = &node.syntax
            && (!families.insert(family.as_str()) || enums.contains_key(family.as_str()))
        {
            return Err(Diagnostic::new(
                "sem-enum-family",
                "enum family conflicts with another declaration",
                node.start,
            ));
        }
    }
    for node in &ast.nodes {
        let conflict = match &node.syntax {
            Syntax::Object {
                name, properties, ..
            } => {
                families.contains(name.as_str())
                    || properties
                        .iter()
                        .any(|(n, _)| families.contains(n.as_str()))
            }
            Syntax::Function { name, .. } => families.contains(name.as_str()),
            Syntax::Declaration(
                crate::parser::Declaration::Properties(names)
                | crate::parser::Declaration::VocabularyProperties(names),
            ) => names.iter().any(|n| families.contains(n.as_str())),
            Syntax::Declaration(crate::parser::Declaration::Intrinsic {
                class,
                signatures,
                ..
            }) => {
                class.as_deref().is_some_and(|n| families.contains(n))
                    || signatures
                        .iter()
                        .any(|s| families.contains(s.name.as_str()))
            }
            _ => false,
        };
        if conflict {
            return Err(Diagnostic::new(
                "sem-enum-family",
                "enum family conflicts with a global symbol",
                node.start,
            ));
        }
    }
    for relation in &ast.relations {
        if families.contains(relation.forward.as_str())
            || relation
                .reverse
                .as_deref()
                .is_some_and(|n| families.contains(n))
        {
            return Err(Diagnostic::new(
                "sem-enum-family",
                "enum family conflicts with a relation",
                0,
            ));
        }
        if let Some(family) = &relation.label_family {
            if relation.vocabulary {
                if family != "Property" && family != "Vocabulary" {
                    return Err(Diagnostic::new(
                        "sem-enum-family",
                        "vocabulary label annotation must be Property",
                        0,
                    ));
                }
            } else if let Some((_, end)) = enum_family_range(ast, family) {
                if end > 4096 {
                    return Err(Diagnostic::new(
                        "sem-enum-family",
                        "relation label family exceeds the 12-bit label range",
                        0,
                    ));
                }
            } else {
                let mut diagnostic = Diagnostic::new("sem-enum-family", "unknown enum family", 0);
                diagnostic.message =
                    format!("relation label names undeclared enum family {family}").into();
                return Err(diagnostic);
            }
        }
    }
    for (index, row) in ast.declared_rows.iter().enumerate() {
        if let Some(row) = row
            && let Some(label) = row.label
        {
            check_label_family(ast, &ast.relations[row.relation], label)?;
            if let Syntax::Name(name) = &ast.nodes[label.0].syntax
                && enums.get(name.as_str()).is_some_and(|v| *v > 4095)
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-enum-family",
                    "relation label exceeds the 12-bit label range",
                ));
            }
        }
    }
    Ok(())
}

fn error(ast: &Ast, id: Id, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        code,
        message,
        ast.nodes.get(id.0).map_or(0, |node| node.start),
    )
}
fn push<T>(values: &mut Vec<T>, value: T) -> Result<(), Diagnostic> {
    values.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    values.push(value);
    Ok(())
}
fn filled<T: Clone>(size: usize, value: T) -> Result<Vec<T>, Diagnostic> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| Diagnostic::resource(0))?;
    result.resize(size, value);
    Ok(result)
}

fn owned_declaration(ast: &Ast, mut id: Id) -> bool {
    while let Syntax::Label(_, body) = ast.nodes[id.0].syntax {
        id = body;
    }
    matches!(ast.nodes[id.0].syntax, Syntax::OwnedLocal(_))
}

/// Safe goto. Labels are unique per function. A goto may target a label on
/// a statement of a sequence that encloses it: a block, or a whole switch body (one
/// lexical scope). It never enters a nested block, loop or switch, and it
/// may not cross a `local owned` declaration in that sequence, so no acquisition is
/// skipped or repeated inside a still-active scope. Leaving nested scopes runs their
/// ordinary cleanup during lowering.
fn validate_gotos(ast: &Ast, body: Id) -> Result<(), Diagnostic> {
    let fail = |id: Id, message| Diagnostic::new("sem-goto", message, ast.nodes[id.0].start);
    let mut parents: HashMap<usize, Id> = HashMap::new();
    let mut labels: HashMap<&str, Id> = HashMap::new();
    let mut gotos: Vec<(Id, &str)> = Vec::new();
    let mut pending = Vec::new();
    push(&mut pending, body)?;
    let mut failed = false;
    while let Some(id) = pending.pop() {
        match &ast.nodes[id.0].syntax {
            Syntax::Function { .. } if id != body => continue,
            Syntax::Label(name, _) => {
                labels.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
                if labels.insert(name.as_str(), id).is_some() {
                    return Err(Diagnostic::new(
                        "sem-label",
                        "duplicate label in function",
                        ast.nodes[id.0].start,
                    ));
                }
            }
            Syntax::Goto(name) => push(&mut gotos, (id, name.as_str()))?,
            _ => {}
        }
        children(&ast.nodes[id.0].syntax, &mut |child| {
            if parents.try_reserve(1).is_err() || pending.try_reserve(1).is_err() {
                failed = true;
                return;
            }
            parents.insert(child.0, id);
            pending.push(child);
        });
        if failed {
            return Err(Diagnostic::resource(0));
        }
    }
    for (goto, name) in gotos {
        let label = *labels
            .get(name)
            .ok_or_else(|| fail(goto, "goto label is not defined in this function"))?;
        let mut statement = label;
        while let Some(parent) = parents.get(&statement.0)
            && matches!(ast.nodes[parent.0].syntax, Syntax::Label(..))
        {
            statement = *parent;
        }
        let container = *parents
            .get(&statement.0)
            .ok_or_else(|| fail(goto, "goto target must be a statement in a block"))?;
        let mut sequence: Vec<Id> = Vec::new();
        match &ast.nodes[container.0].syntax {
            Syntax::Block(items) => {
                sequence
                    .try_reserve(items.len())
                    .map_err(|_| Diagnostic::resource(0))?;
                sequence.extend_from_slice(items);
            }
            Syntax::Switch(_, arms) => {
                for (_, statements) in arms {
                    sequence
                        .try_reserve(statements.len())
                        .map_err(|_| Diagnostic::resource(0))?;
                    sequence.extend_from_slice(statements);
                }
            }
            _ => {
                return Err(fail(
                    goto,
                    "goto target must be a statement in a block or switch body",
                ));
            }
        }
        let mut current = goto;
        let from = loop {
            let parent = *parents
                .get(&current.0)
                .ok_or_else(|| fail(goto, "goto cannot enter a nested block, loop or switch"))?;
            if parent == container {
                break current;
            }
            current = parent;
        };
        let (Some(target), Some(source)) = (
            sequence.iter().position(|item| *item == statement),
            sequence.iter().position(|item| *item == from),
        ) else {
            return Err(fail(
                goto,
                "goto cannot enter a nested block, loop or switch",
            ));
        };
        let crossed = if target > source {
            source + 1..target
        } else {
            target..source
        };
        if sequence[crossed]
            .iter()
            .any(|item| owned_declaration(ast, *item))
        {
            return Err(fail(
                goto,
                "goto cannot cross a local owned declaration; enclose that region in a block",
            ));
        }
    }
    Ok(())
}

pub(crate) fn children(syntax: &Syntax, visit: &mut impl FnMut(Id)) {
    match syntax {
        Syntax::Move(id)
        | Syntax::Delegated(id)
        | Syntax::OwnedCall(id)
        | Syntax::Group(id)
        | Syntax::ExpandedArgument(id)
        | Syntax::Unary(_, id, _)
        | Syntax::Expression(id) => visit(*id),
        Syntax::Binary(_, a, b)
        | Syntax::While(a, b)
        | Syntax::DoWhile(a, b)
        | Syntax::LocalLookupSized(a, b)
        | Syntax::IndirectProperty(a, b)
        | Syntax::Index(a, b) => {
            visit(*a);
            visit(*b);
        }
        Syntax::StaticLookup { owner, property } => {
            visit(*owner);
            visit(*property);
        }
        Syntax::StaticVector {
            capacity,
            owner,
            property,
        } => {
            visit(*capacity);
            visit(*owner);
            visit(*property);
        }
        Syntax::StaticRexPattern {
            text,
            owner,
            property,
        } => {
            visit(*text);
            visit(*owner);
            visit(*property);
        }
        Syntax::Lookup(entries) => {
            for (key, value) in entries {
                if let Some(key) = key {
                    visit(*key);
                }
                visit(*value);
            }
        }
        Syntax::Membership {
            value, candidates, ..
        } => {
            visit(*value);
            for candidate in candidates {
                visit(*candidate);
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
            visit(*prototype);
            for argument in arguments {
                visit(*argument);
            }
        }
        Syntax::Construction {
            prototype,
            arguments,
            owner,
            property,
        } => {
            visit(*prototype);
            visit(*owner);
            visit(*property);
            for argument in arguments {
                visit(*argument);
            }
        }
        Syntax::Label(_, body) => visit(*body),
        Syntax::Apply(target, arguments) => {
            visit(*target);
            visit(*arguments);
        }
        Syntax::Finally(body, finalizer) => {
            visit(*body);
            visit(*finalizer);
        }
        Syntax::Try(body, catches) => {
            visit(*body);
            for (class, declaration, handler) in catches {
                visit(*class);
                visit(*declaration);
                visit(*handler);
            }
        }
        Syntax::ForEach(declaration, collection, body) => {
            visit(*collection);
            visit(*declaration);
            visit(*body);
        }
        Syntax::For {
            init,
            condition,
            step,
            body,
        } => {
            for initializer in init {
                visit(*initializer);
            }
            if let Some(condition) = condition {
                visit(*condition);
            }
            if let Some(step) = step {
                visit(*step);
            }
            visit(*body);
        }
        Syntax::Switch(selector, arms) => {
            visit(*selector);
            for (case, statements) in arms {
                if let Some(case) = case {
                    visit(*case);
                }
                for statement in statements {
                    visit(*statement);
                }
            }
        }
        Syntax::Conditional(a, b, c) => {
            visit(*a);
            visit(*b);
            visit(*c);
        }
        Syntax::If(a, b, c) => {
            visit(*a);
            visit(*b);
            if let Some(c) = c {
                visit(*c);
            }
        }
        Syntax::List(parts)
        | Syntax::ArgumentPack(parts)
        | Syntax::ArgumentPackTail(parts)
        | Syntax::Interpolation { parts, .. } => {
            for part in parts {
                visit(*part);
            }
        }
        Syntax::Property(object, _) => visit(*object),
        Syntax::Object { properties, .. } => {
            for (_, value) in properties {
                visit(*value);
            }
        }
        Syntax::Call(a, args) => {
            visit(*a);
            for arg in args {
                visit(*arg);
            }
        }
        Syntax::Block(items) => {
            for item in items {
                visit(*item);
            }
        }
        Syntax::Local(items) | Syntax::OwnedLocal(items) => {
            for (_, item) in items {
                if let Some(item) = item {
                    visit(*item);
                }
            }
        }
        Syntax::Throw(id)
        | Syntax::Return(Some(id))
        | Syntax::OwnerScope(id)
        | Syntax::LocalVector(id) => visit(*id),
        Syntax::Function { body, .. } => visit(*body),
        _ => {}
    }
}
pub(crate) fn validate_ast(ast: &Ast) -> Result<(), Diagnostic> {
    for (index, node) in ast.nodes.iter().enumerate() {
        let mut invalid = node.start > node.end;
        if let Syntax::Function {
            rest: true,
            parameters,
            ..
        } = &node.syntax
        {
            invalid |= parameters.is_empty();
        }
        if let Syntax::Function {
            optional,
            rest,
            parameters,
            ..
        } = &node.syntax
        {
            invalid |= *optional > parameters.len().saturating_sub(usize::from(*rest));
        }
        if let Syntax::Integer(_, radix) = node.syntax {
            invalid |= !matches!(radix, 8 | 10 | 16);
        }
        children(&node.syntax, &mut |id| {
            invalid |= id.0 >= index;
        });
        if invalid {
            return Err(error(
                ast,
                Id(index),
                "sem-ast",
                "invalid or non-topological AST",
            ));
        }
    }
    let mut scoped = Vec::new();
    scoped
        .try_reserve_exact(ast.nodes.len())
        .map_err(|_| Diagnostic::resource(0))?;
    scoped.resize(ast.nodes.len(), false);
    for node in &ast.nodes {
        if let Syntax::OwnerScope(body) = node.syntax {
            let Syntax::Block(items) = &ast.nodes[body.0].syntax else {
                return Err(error(
                    ast,
                    body,
                    "sem-owner",
                    "owner scope requires a block",
                ));
            };
            for id in items {
                scoped[id.0] = true;
            }
        }
    }
    for (index, node) in ast.nodes.iter().enumerate() {
        if let Syntax::OwnedLocal(items) = &node.syntax {
            if !scoped[index] {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-owner",
                    "local owned requires an explicit block",
                ));
            }
            for (_, init) in items {
                if !init.is_some_and(|id| {
                    matches!(
                        ast.nodes[id.0].syntax,
                        Syntax::LocalVector(_)
                            | Syntax::LocalOwnedCollection
                            | Syntax::LocalStringBuffer
                            | Syntax::LocalLookupSized(..)
                            | Syntax::Lookup(_)
                            | Syntax::LocalConstruction { .. }
                            | Syntax::OwnedCall(_)
                    )
                }) {
                    return Err(error(
                        ast,
                        Id(index),
                        "sem-owner",
                        "owned local requires an owned constructor",
                    ));
                }
            }
        }
    }
    for id in &ast.functions {
        if !matches!(
            ast.nodes.get(id.0).map(|n| &n.syntax),
            Some(Syntax::Function { .. })
        ) {
            return Err(error(ast, *id, "sem-ast", "invalid function root"));
        }
    }
    for id in &ast.objects {
        if !matches!(
            ast.nodes.get(id.0).map(|n| &n.syntax),
            Some(Syntax::Object { .. })
        ) {
            return Err(error(ast, *id, "sem-ast", "invalid object root"));
        }
    }
    Ok(())
}
fn ungroup(ast: &Ast, mut id: Id) -> Id {
    while let Syntax::Group(inner) = ast.nodes[id.0].syntax {
        id = inner;
    }
    id
}

fn inherited_destination(ast: &Ast, id: Id) -> bool {
    match &ast.nodes[ungroup(ast, id).0].syntax {
        Syntax::Name(name) => name == "inherited",
        Syntax::Property(receiver, _) => {
            matches!(&ast.nodes[ungroup(ast, *receiver).0].syntax, Syntax::Name(name) if name == "inherited")
        }
        _ => false,
    }
}

fn literal(text: &str, radix: u32) -> Option<u32> {
    let digits = if radix == 16 { text.get(2..)? } else { text };
    if digits.is_empty() {
        return None;
    }
    let mut value = 0u32;
    for ch in digits.chars() {
        value = value.checked_mul(radix)?.checked_add(ch.to_digit(radix)?)?;
    }
    Some(value)
}
fn logical(value: Scalar) -> Result<bool, &'static str> {
    match value {
        Scalar::Nil => Ok(false),
        Scalar::True => Ok(true),
        _ => Err("sem-type"),
    }
}
fn boolean(value: bool) -> Scalar {
    if value { Scalar::True } else { Scalar::Nil }
}
fn integer(value: Scalar) -> Result<i32, &'static str> {
    if let Scalar::Integer(value) = value {
        Ok(value)
    } else {
        Err("sem-type")
    }
}
fn unary(op: &str, value: Scalar) -> Result<Scalar, &'static str> {
    if op == "!" {
        return Ok(boolean(!logical(value)?));
    }
    let x = integer(value)?;
    Ok(Scalar::Integer(match op {
        "+" => x,
        "-" => x.checked_neg().ok_or("sem-overflow")?,
        "~" => !x,
        _ => return Err("sem-unavailable"),
    }))
}
fn binary(op: &str, left: Scalar, right: Scalar) -> Result<Scalar, &'static str> {
    match op {
        "==" => return Ok(boolean(left == right)),
        "!=" => return Ok(boolean(left != right)),
        "&&" => return Ok(boolean(logical(left)? & logical(right)?)),
        "||" => return Ok(boolean(logical(left)? | logical(right)?)),
        "??" => return Ok(if left == Scalar::Nil { right } else { left }),
        "," => return Ok(right),
        _ => {}
    }
    let a = integer(left)?;
    let b = integer(right)?;
    let value = match op {
        "+" => a.checked_add(b).ok_or("sem-overflow")?,
        "-" => a.checked_sub(b).ok_or("sem-overflow")?,
        "*" => a.checked_mul(b).ok_or("sem-overflow")?,
        "/" => {
            if b == 0 {
                return Err("sem-divisor");
            }
            a.checked_div(b).ok_or("sem-overflow")?
        }
        "%" => {
            if b == 0 {
                return Err("sem-divisor");
            }
            if a == i32::MIN && b == -1 { 0 } else { a % b }
        }
        "&" => a & b,
        "|" => a | b,
        "^" => a ^ b,
        "<<" | ">>" | ">>>" => {
            let shift = u32::try_from(b)
                .ok()
                .filter(|n| *n < 32)
                .ok_or("sem-shift")?;
            match op {
                "<<" => a.wrapping_shl(shift),
                ">>" => a >> shift,
                _ => {
                    i32::from_ne_bytes((u32::from_ne_bytes(a.to_ne_bytes()) >> shift).to_ne_bytes())
                }
            }
        }
        "<" => return Ok(boolean(a < b)),
        ">" => return Ok(boolean(a > b)),
        "<=" => return Ok(boolean(a <= b)),
        ">=" => return Ok(boolean(a >= b)),
        _ => return Err("sem-unavailable"),
    };
    Ok(Scalar::Integer(value))
}
/// The packed descriptor for a name that reads a relation, or nothing when the
/// name is something else.
fn relation_descriptor(ast: &Ast, receiver: Id) -> Option<u64> {
    let Syntax::Name(name) = &ast.nodes[ungroup(ast, receiver).0].syntax else {
        return None;
    };
    ast.relations
        .iter()
        .enumerate()
        .find_map(|(index, relation)| {
            let reversed = if relation.forward == *name {
                0
            } else if relation.reverse.as_deref() == Some(name.as_str()) {
                1
            } else {
                return None;
            };
            // index | direction | cardinality | labelled, packed small enough to
            // travel as a constant operand. A labelled call ors its label in at
            // bit 16.
            Some(
                index as u64
                    | (reversed << 12)
                    | (relation.cardinality << 13)
                    | (u64::from(relation.labelled) << 15),
            )
        })
}

/// What a relation column holds. The annotation on the declaration
/// says which, and these are the shapes a source expression can be seen to have
/// without inferring a type: a literal of the wrong kind is a mistake whatever
/// the rest of the program does. The run-time tag guards remain the authority
/// for everything an expression only computes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Entity,
    Word,
    Label,
}

/// Whether `argument` cannot possibly hold what `column` requires.
fn column_mismatch(
    ast: &Ast,
    enums: &HashMap<&str, u32>,
    argument: Id,
    column: Column,
) -> Option<&'static str> {
    let leaf = ungroup(ast, argument);
    let named_enumerator = match &ast.nodes[leaf.0].syntax {
        Syntax::Name(name) => enums.contains_key(name.as_str()),
        _ => false,
    };
    let syntax = &ast.nodes[leaf.0].syntax;
    match column {
        Column::Entity => match syntax {
            Syntax::String(_) | Syntax::Interpolation { .. } => {
                Some("a relation's Entity column takes an entity, not text")
            }
            Syntax::Integer(..) | Syntax::Decimal(_) | Syntax::True => {
                Some("a relation's Entity column takes an entity, not a number or true")
            }
            Syntax::List(_) => Some("a relation's Entity column takes an entity, not a list"),
            _ if named_enumerator => {
                Some("a relation's Entity column takes an entity, not an enumerator")
            }
            _ => None,
        },
        Column::Word => match syntax {
            Syntax::Integer(..) | Syntax::Decimal(_) | Syntax::True => {
                Some("a vocabulary relation's Text column takes a word, not a number or true")
            }
            Syntax::List(_) => Some("a vocabulary relation's Text column takes a word, not a list"),
            _ if named_enumerator => {
                Some("a vocabulary relation's Text column takes a word, not an enumerator")
            }
            _ => None,
        },
        // A relation-family label requires an enumerator; the operation names its
        // table with a literal one often enough to be worth saying early. A
        // vocabulary relation's label is a property address instead,
        // and is checked where the dictionary property is resolved.
        Column::Label => match syntax {
            Syntax::String(_) | Syntax::Interpolation { .. } => {
                Some("a relation's label is an enumerator, not text")
            }
            Syntax::Integer(..) | Syntax::Decimal(_) | Syntax::True | Syntax::Nil => {
                Some("a relation's label is an enumerator, not a number, true or nil")
            }
            Syntax::List(_) => Some("a relation's label is an enumerator, not a list"),
            _ => None,
        },
    }
}

/// The columns a relation operation's arguments stand in, after the descriptor.
/// Whether this operation's arguments stand in relation columns.
fn relation_builtin_columns(builtin: Builtin) -> bool {
    matches!(
        builtin,
        Builtin::RelationSet
            | Builtin::RelationUnset
            | Builtin::RelationGet
            | Builtin::RelationAll
            | Builtin::RelationContains
            | Builtin::RelationOutermost
            | Builtin::RelationDescendants
            | Builtin::RelationAncestors
    )
}

/// `set`, `unset` and `contains` name both sides; the rest name the left side
/// only, and a labelled relation adds the label last.
fn relation_columns(builtin: Builtin, vocabulary: bool, labelled: bool) -> Vec<Column> {
    let right = if vocabulary {
        Column::Word
    } else {
        Column::Entity
    };
    let mut columns = match builtin {
        Builtin::RelationSet | Builtin::RelationUnset | Builtin::RelationContains => {
            vec![Column::Entity, right]
        }
        _ => vec![Column::Entity],
    };
    if labelled {
        columns.push(Column::Label);
    }
    columns
}

fn relation_builtin(name: &str) -> Option<Builtin> {
    Some(match name {
        "set" => Builtin::RelationSet,
        "unset" => Builtin::RelationUnset,
        "get" => Builtin::RelationGet,
        "all" => Builtin::RelationAll,
        "contains" => Builtin::RelationContains,
        "outermost" => Builtin::RelationOutermost,
        "descendants" => Builtin::RelationDescendants,
        "ancestors" => Builtin::RelationAncestors,
        _ => return None,
    })
}

fn assignment(op: &str) -> bool {
    matches!(
        op,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" | ">>>="
    )
}

pub(crate) fn constants(ast: &Ast) -> Result<Vec<Option<Scalar>>, Diagnostic> {
    let mut values = filled(ast.nodes.len(), None)?;
    let mut minimum = filled(ast.nodes.len(), false)?;
    for node in &ast.nodes {
        if let Syntax::Unary("-", operand, false) = node.syntax {
            let leaf = ungroup(ast, operand);
            if matches!(&ast.nodes[leaf.0].syntax,Syntax::Integer(text,10) if literal(text,10)==Some(2147483648))
            {
                minimum[leaf.0] = true;
            }
        }
    }
    for (index, node) in ast.nodes.iter().enumerate() {
        let id = Id(index);
        let fail = |code| {
            error(
                ast,
                id,
                code,
                "invalid constant expression for the scalar profile",
            )
        };
        let result = match &node.syntax {
            Syntax::Integer(text, radix) => {
                let value = literal(text, *radix).ok_or_else(|| fail("sem-literal-range"))?;
                if *radix == 10 && value > i32::MAX as u32 {
                    if value == 2147483648 && minimum[index] {
                        None
                    } else {
                        return Err(fail("sem-literal-range"));
                    }
                } else {
                    Some(Scalar::Integer(i32::from_ne_bytes(value.to_ne_bytes())))
                }
            }
            Syntax::Decimal(_) => return Err(fail("sem-unavailable")),
            Syntax::String(_) => None,
            Syntax::Nil => Some(Scalar::Nil),
            Syntax::True => Some(Scalar::True),
            Syntax::Group(child) => values[child.0],
            Syntax::Unary(op, child, postfix) => {
                if *postfix || matches!(*op, "++" | "--") {
                    None
                } else if *op == "-" && minimum[ungroup(ast, *child).0] {
                    Some(Scalar::Integer(i32::MIN))
                } else {
                    values[child.0]
                        .map(|value| unary(op, value))
                        .transpose()
                        .map_err(fail)?
                }
            }
            Syntax::Binary(op, a, b) if !assignment(op) => {
                // Validate either known operand even when the other is unknown.
                if !matches!(*op, "==" | "!=" | "??" | ",") {
                    for value in [values[a.0], values[b.0]].into_iter().flatten() {
                        if matches!(*op, "&&" | "||") {
                            logical(value).map_err(fail)?;
                        } else {
                            integer(value).map_err(fail)?;
                        }
                    }
                }
                match (values[a.0], values[b.0]) {
                    (Some(a), Some(b)) => Some(binary(op, a, b).map_err(fail)?),
                    _ => None,
                }
            }
            Syntax::Membership {
                value,
                candidates,
                negated,
            } => {
                if let Some(left) = values[value.0] {
                    let mut answer = Some(boolean(*negated));
                    for candidate in candidates {
                        match values[candidate.0] {
                            Some(right) if left == right => {
                                answer = Some(boolean(!negated));
                                break;
                            }
                            Some(_) => (),
                            None => {
                                answer = None;
                                break;
                            }
                        }
                    }
                    answer
                } else {
                    None
                }
            }
            Syntax::Conditional(condition, yes, no) => match values[condition.0] {
                Some(value) => {
                    if logical(value).map_err(fail)? {
                        values[yes.0]
                    } else {
                        values[no.0]
                    }
                }
                None => None,
            },
            Syntax::If(condition, ..)
            | Syntax::While(condition, ..)
            | Syntax::DoWhile(condition, ..)
            | Syntax::For {
                condition: Some(condition),
                ..
            } => {
                if let Some(value) = values[condition.0] {
                    logical(value).map_err(fail)?;
                }
                None
            }
            _ => None,
        };
        values[index] = result;
    }
    Ok(values)
}

#[derive(Clone, Copy, Default)]
struct Control {
    in_loop: bool,
    can_break: bool,
}

#[derive(PartialEq)]
enum CaseKey<'a> {
    Scalar(Scalar),
    String(&'a str),
    Enum(u32),
    Object(&'a str),
    Property(&'a str),
}

fn case_key<'a>(
    ast: &'a Ast,
    id: Id,
    values: &[Option<Scalar>],
    enums: &HashMap<&str, u32>,
) -> Result<CaseKey<'a>, Diagnostic> {
    let id = ungroup(ast, id);
    if let Some(value) = values[id.0] {
        return Ok(CaseKey::Scalar(value));
    }
    match &ast.nodes[id.0].syntax {
        Syntax::String(value) if !value.emitting => Ok(CaseKey::String(&value.text)),
        Syntax::Name(name) if enums.contains_key(name.as_str()) => {
            Ok(CaseKey::Enum(enums[name.as_str()]))
        }
        Syntax::Name(name) if ast.objects.iter().any(|id| matches!(&ast.nodes[id.0].syntax, Syntax::Object { name: object, .. } if object == name)) => Ok(CaseKey::Object(name)),
        Syntax::PropertyAddress(name) => Ok(CaseKey::Property(name)),
        _ => Err(error(
            ast,
            id,
            "sem-switch",
            "case label requires a supported constant",
        )),
    }
}

enum Task {
    LabelExit,
    Catch(Id, Id, Id, Control),
    Visit(Id, bool, Control),
    Exit,
    Declare(Id, usize, Control),
}

pub fn analyze(ast: &Ast) -> Result<Analysis, Diagnostic> {
    validate_ast(ast)?;
    // Validate rule structure early; native rule tables are emitted by object builds.
    crate::grammar::lower(ast)?;
    let mut intrinsic_functions = HashMap::new();
    for node in &ast.nodes {
        if let Syntax::Declaration(crate::parser::Declaration::Intrinsic {
            class: None,
            version,
            signatures,
            ..
        }) = &node.syntax
        {
            for signature in signatures {
                intrinsic_functions
                    .try_reserve(1)
                    .map_err(|_| Diagnostic::resource(node.start))?;
                if intrinsic_functions
                    .insert(signature.name.as_str(), (version.as_str(), signature))
                    .is_some()
                {
                    return Err(Diagnostic::new(
                        "sem-intrinsic",
                        "duplicate intrinsic function declaration",
                        node.start,
                    ));
                }
            }
        }
    }

    let mut output_context = filled(ast.nodes.len(), false)?;
    let mut embedded = Vec::new();
    for node in &ast.nodes {
        if let Syntax::Expression(expr) = node.syntax {
            output_context[ungroup(ast, expr).0] = true;
        }
        if let Syntax::Interpolation { parts, .. } = &node.syntax {
            for part in parts.iter().skip(1).step_by(2) {
                push(&mut embedded, *part)?;
            }
        }
    }
    while let Some(id) = embedded.pop() {
        if output_context[id.0] {
            continue;
        }
        output_context[id.0] = true;
        let mut failure = None;
        children(&ast.nodes[id.0].syntax, &mut |child| {
            if let Err(error) = push(&mut embedded, child) {
                failure = Some(error);
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
    }
    for (index, node) in ast.nodes.iter().enumerate() {
        let emits = matches!(&node.syntax,Syntax::String(literal) if literal.emitting)
            || matches!(node.syntax, Syntax::Interpolation { emitting: true, .. });
        if emits && !output_context[index] {
            return Err(error(
                ast,
                Id(index),
                "sem-output-context",
                "output strings require a statement or embedded expression",
            ));
        }
    }
    let plan = crate::object_init::declarations(ast)?;
    let enums = enumerators(ast)?;
    let case_constants = constants(ast)?;
    let mut property_names = HashMap::new();
    property_names
        .try_reserve(plan.properties.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for (index, name) in plan.properties.iter().enumerate() {
        property_names.insert(name.as_str(), index as u32);
    }
    let mut objects = HashMap::new();
    objects
        .try_reserve(ast.objects.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for (index, id) in ast.objects.iter().enumerate() {
        let Syntax::Object { name, .. } = &ast.nodes[id.0].syntax else {
            unreachable!()
        };
        objects.insert(name.as_str(), index as u32);
    }
    let mut functions = HashMap::new();
    functions
        .try_reserve(ast.functions.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for (index, id) in ast.functions.iter().enumerate() {
        let Syntax::Function { name, .. } = &ast.nodes[id.0].syntax else {
            unreachable!()
        };
        if objects.contains_key(name.as_str()) || functions.insert(name.as_str(), index).is_some() {
            return Err(error(ast, *id, "sem-name", "duplicate function name"));
        }
    }
    let mut external_functions = HashMap::new();
    for (index, node) in ast.nodes.iter().enumerate() {
        if let Syntax::Declaration(crate::parser::Declaration::ExternalFunction(name, arity)) =
            &node.syntax
        {
            if objects.contains_key(name.as_str())
                || enums.contains_key(name.as_str())
                || property_names.contains_key(name.as_str())
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-external",
                    "external function conflicts with a non-function declaration",
                ));
            }
            if external_functions
                .get(name.as_str())
                .is_some_and(|previous| previous != arity)
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-external",
                    "inconsistent external function arity",
                ));
            }
            if let Some(function) = functions.get(name.as_str())
                && let Syntax::Function {
                    parameters,
                    rest,
                    optional,
                    ..
                } = &ast.nodes[ast.functions[*function].0].syntax
                && (*rest || *optional != 0 || parameters.len() != *arity)
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-external",
                    "external declaration does not match the function definition",
                ));
            }
            external_functions
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(node.start))?;
            external_functions.insert(name.as_str(), *arity);
        }
    }
    let mut capture_candidates = filled(ast.nodes.len(), false)?;
    // What each capturing callback takes from its enclosing scope, in order.
    // Enclosing functions are analysed before the callbacks they reference.
    let mut capture_scopes: HashMap<usize, Vec<(String, Capture)>> = HashMap::new();
    // Where each local was first captured, so a later assignment can be refused:
    // a capture takes the value, while the reference shares the variable.
    let mut capture_sites: HashMap<usize, usize> = HashMap::new();
    let mut result = Analysis {
        bindings: filled(ast.nodes.len(), None)?,
        relations: filled(ast.nodes.len(), None)?,
        vocabulary: filled(ast.nodes.len(), None)?,
        constants: Vec::new(),
        locals: Vec::new(),
        calls: filled(ast.nodes.len(), None)?,
        builtins: filled(ast.nodes.len(), None)?,
        flow_complete: false,
        properties: filled(ast.nodes.len(), None)?,
        strings: crate::string_pool::collect(ast)?.ids,
        constant_lists: {
            let mut lists = Vec::new();
            lists
                .try_reserve_exact(plan.constant_lists.len())
                .map_err(|_| Diagnostic::resource(0))?;
            lists.extend_from_slice(&plan.constant_lists);
            lists
        },
        grammar_construct: plan
            .properties
            .iter()
            .position(|name| name == "construct")
            .and_then(|index| u32::try_from(index).ok()),
        captures: filled(ast.functions.len(), Vec::new())?,
    };
    let owned_calls = owned_call_sites(ast)?;
    // Children precede parents: propagate protected-return context in one pass.
    let mut inside_finally = filled(ast.nodes.len(), false)?;
    for index in (0..ast.nodes.len()).rev() {
        if inside_finally[index] || matches!(ast.nodes[index].syntax, Syntax::Finally(..)) {
            children(&ast.nodes[index].syntax, &mut |child| {
                inside_finally[child.0] = true
            });
        }
    }
    // Static field initializers build module-owned values, so the scope rules
    // for run-time text and callbacks do not apply inside them.
    let mut inside_static = filled(ast.nodes.len(), false)?;
    for function in &ast.functions {
        if let Syntax::Function {
            body,
            is_static: true,
            ..
        } = ast.nodes[function.0].syntax
        {
            inside_static[body.0] = true;
        }
    }
    for index in (0..ast.nodes.len()).rev() {
        if inside_static[index] {
            children(&ast.nodes[index].syntax, &mut |child| {
                inside_static[child.0] = true
            });
        }
    }
    let mut owned_initializers = filled(ast.nodes.len(), false)?;
    for node in &ast.nodes {
        if let Syntax::OwnedLocal(items) = &node.syntax {
            for (_, initializer) in items {
                if let Some(initializer) = initializer {
                    owned_initializers[initializer.0] = true;
                }
            }
        }
    }
    // A callback's captures are discovered while analysing the function that
    // creates it, so enclosing functions must be analysed first.
    let mut enclosing = filled(ast.functions.len(), None)?;
    for (index, function) in ast.functions.iter().enumerate() {
        let Syntax::Function { body, .. } = ast.nodes[function.0].syntax else {
            continue;
        };
        let mut pending = Vec::new();
        push(&mut pending, body)?;
        while let Some(node) = pending.pop() {
            if let Syntax::FunctionReference(inner) = ast.nodes[node.0].syntax
                && let Some(inner) = ast.functions.iter().position(|id| *id == inner)
            {
                enclosing[inner] = Some(index);
            }
            let mut failure = None;
            children(&ast.nodes[node.0].syntax, &mut |child| {
                if let Err(error) = push(&mut pending, child) {
                    failure = Some(error);
                }
            });
            if let Some(error) = failure {
                return Err(error);
            }
        }
    }
    let mut order = Vec::new();
    order
        .try_reserve_exact(ast.functions.len())
        .map_err(|_| Diagnostic::resource(0))?;
    let mut placed = filled(ast.functions.len(), false)?;
    for index in 0..ast.functions.len() {
        // place each function after every function that encloses it
        let mut chain = Vec::new();
        let mut at = Some(index);
        while let Some(current) = at {
            if placed[current] {
                break;
            }
            push(&mut chain, current)?;
            at = enclosing[current];
            if chain.len() > ast.functions.len() {
                return Err(Diagnostic::new("sem-ast", "cyclic callback nesting", 0));
            }
        }
        for current in chain.into_iter().rev() {
            if !placed[current] {
                placed[current] = true;
                push(&mut order, current)?;
            }
        }
    }
    for function_index in order {
        let function = &ast.functions[function_index];
        let Syntax::Function {
            returns_owned,
            name: function_name,
            is_static,
            parameters,
            body,
            ..
        } = &ast.nodes[function.0].syntax
        else {
            unreachable!()
        };
        validate_gotos(ast, *body)?;
        if *returns_owned && function_name == "main" {
            return Err(error(
                ast,
                *function,
                "sem-owner",
                "main cannot return ownership",
            ));
        }
        let self_slot = (function_name.starts_with('$') && function_name.contains('.'))
            .then_some(result.locals.len());
        let method_context = function_name
            .strip_prefix('$')
            .and_then(|name| name.split_once('.'))
            .and_then(|(object, property)| {
                Some((*objects.get(object)?, *property_names.get(property)?))
            });
        let mut visible: HashMap<&str, usize> = HashMap::new();
        visible
            .try_reserve(parameters.len())
            .map_err(|_| Diagnostic::resource(0))?;
        let mut scopes: Vec<Vec<&str>> = Vec::new();
        let mut labels: Vec<(&str, bool)> = Vec::new();
        push(&mut scopes, Vec::new())?;
        // a capturing callback takes its environment as a synthetic
        // first parameter, and its captured names resolve through that.
        let captured = capture_scopes.get(&function_index).cloned();
        let mut captures_in_scope: HashMap<&str, usize> = HashMap::new();
        if let Some(captured) = captured.as_ref() {
            push(
                &mut result.locals,
                Local {
                    function: *function,
                    declaration: *function,
                    ordinal: 0,
                    parameter: true,
                },
            )?;
            captures_in_scope
                .try_reserve(captured.len())
                .map_err(|_| Diagnostic::resource(0))?;
            for (index, (name, _)) in captured.iter().enumerate() {
                captures_in_scope.insert(name.as_str(), index);
            }
        }
        let parameter_offset = usize::from(captured.is_some());
        for (ordinal, name) in parameters.iter().enumerate() {
            let ordinal = ordinal + parameter_offset;
            // A parameter may shadow a global function or object, as the
            // reference allows; name resolution already prefers the local.
            // Another parameter or an enclosing local may not be
            // shadowed, and `inherited` stays reserved.
            if name == "inherited" || visible.contains_key(name.as_str()) {
                return Err(error(
                    ast,
                    *function,
                    "sem-shadowing",
                    "duplicate or shadowing parameter",
                ));
            }
            let slot = result.locals.len();
            push(
                &mut result.locals,
                Local {
                    function: *function,
                    declaration: *function,
                    ordinal,
                    parameter: true,
                },
            )?;
            visible.insert(name, slot);
        }
        let mut tasks = Vec::new();
        push(&mut tasks, Task::Visit(*body, false, Control::default()))?;
        while let Some(task) = tasks.pop() {
            match task {
                Task::LabelExit => {
                    labels.pop();
                }
                Task::Exit => {
                    for name in
                        scopes
                            .pop()
                            .ok_or(error(ast, *function, "sem-ast", "missing scope"))?
                    {
                        visible.remove(name);
                    }
                }
                Task::Declare(id, ordinal, depth) => {
                    let (Syntax::Local(items) | Syntax::OwnedLocal(items)) =
                        &ast.nodes[id.0].syntax
                    else {
                        return Err(error(ast, id, "sem-ast", "invalid declaration"));
                    };
                    let (name, init) = &items[ordinal];
                    // A local may shadow a global function or object.
                    if name == "inherited" || visible.contains_key(name.as_str()) {
                        return Err(error(
                            ast,
                            id,
                            "sem-shadowing",
                            "duplicate or shadowing local",
                        ));
                    }
                    let slot = result.locals.len();
                    push(
                        &mut result.locals,
                        Local {
                            function: *function,
                            declaration: id,
                            ordinal,
                            parameter: false,
                        },
                    )?;
                    visible
                        .try_reserve(1)
                        .map_err(|_| Diagnostic::resource(0))?;
                    visible.insert(name, slot);
                    push(
                        scopes.last_mut().ok_or(error(
                            ast,
                            id,
                            "sem-ast",
                            "missing declaration scope",
                        ))?,
                        name,
                    )?;
                    if let Some(init) = init {
                        push(&mut tasks, Task::Visit(*init, false, depth))?;
                    }
                }
                Task::Catch(class, declaration, body, depth) => {
                    push(&mut scopes, Vec::new())?;
                    push(&mut tasks, Task::Exit)?;
                    push(&mut tasks, Task::Visit(body, false, depth))?;
                    push(&mut tasks, Task::Visit(declaration, false, depth))?;
                    push(&mut tasks, Task::Visit(class, false, depth))?;
                }
                Task::Visit(id, callee, depth) => {
                    let fail = |code, message| error(ast, id, code, message);
                    match &ast.nodes[id.0].syntax {
                        Syntax::FunctionReference(function) => {
                            let index = ast
                                .functions
                                .iter()
                                .position(|id| id == function)
                                .ok_or_else(|| fail("sem-ast", "missing callback function"))?;
                            let Syntax::Function {
                                parameters, body, ..
                            } = &ast.nodes[function.0].syntax
                            else {
                                return Err(fail("sem-ast", "invalid callback target"));
                            };
                            let mut pending = Vec::new();
                            push(&mut pending, *body)?;
                            // Names the callback declares itself are not captures.
                            // A declaration covers the rest of its block, so record
                            // each one with the span it shadows.
                            let mut declared: Vec<(&str, usize, usize)> = Vec::new();
                            let mut blocks = Vec::new();
                            push(&mut blocks, (*body, ast.nodes[body.0].end))?;
                            while let Some((node, block_end)) = blocks.pop() {
                                let block_end = match ast.nodes[node.0].syntax {
                                    Syntax::Block(_) => ast.nodes[node.0].end,
                                    _ => block_end,
                                };
                                if let Syntax::Local(items) | Syntax::OwnedLocal(items) =
                                    &ast.nodes[node.0].syntax
                                {
                                    for (name, _) in items {
                                        push(
                                            &mut declared,
                                            (name.as_str(), ast.nodes[node.0].start, block_end),
                                        )?;
                                    }
                                }
                                if let Syntax::FunctionReference(nested) = ast.nodes[node.0].syntax
                                    && let Syntax::Function { body, .. } =
                                        ast.nodes[nested.0].syntax
                                {
                                    push(&mut blocks, (body, ast.nodes[body.0].end))?;
                                }
                                let mut failure = None;
                                children(&ast.nodes[node.0].syntax, &mut |child| {
                                    if let Err(error) = push(&mut blocks, (child, block_end)) {
                                        failure = Some(error);
                                    }
                                });
                                if let Some(error) = failure {
                                    return Err(error);
                                }
                            }
                            push(&mut pending, *body)?;
                            // collect what the callback takes from this
                            // scope. A bare property name needs the enclosing `self`.
                            let mut captured: Vec<(String, Capture)> = Vec::new();
                            let mut capture =
                                |name: &str, source: Capture| -> Result<usize, Diagnostic> {
                                    if let Some(at) =
                                        captured.iter().position(|(known, _)| known == name)
                                    {
                                        return Ok(at);
                                    }
                                    push(&mut captured, (name.to_owned(), source))?;
                                    Ok(captured.len() - 1)
                                };
                            while let Some(node) = pending.pop() {
                                if let Syntax::Name(name) = &ast.nodes[node.0].syntax
                                    && !parameters.contains(name)
                                    && !declared.iter().any(|(declared, from, to)| {
                                        *declared == name.as_str()
                                            && *from <= ast.nodes[node.0].start
                                            && ast.nodes[node.0].start < *to
                                    })
                                {
                                    if let Some(slot) = visible.get(name.as_str()) {
                                        capture(name, Capture::Local(*slot))?;
                                        capture_candidates[node.0] = true;
                                    } else if let Some(outer) = captures_in_scope.get(name.as_str())
                                    {
                                        // a nested callback captures through this
                                        // callback's own environment
                                        capture(name, Capture::Outer(*outer))?;
                                        capture_candidates[node.0] = true;
                                    } else if property_names.contains_key(name.as_str())
                                        && !functions.contains_key(name.as_str())
                                        && !objects.contains_key(name.as_str())
                                        && !enums.contains_key(name.as_str())
                                    {
                                        let source = match (
                                            visible.get("self"),
                                            captures_in_scope.get("self"),
                                        ) {
                                            (Some(slot), _) => Capture::Local(*slot),
                                            (None, Some(outer)) => Capture::Outer(*outer),
                                            _ => {
                                                return Err(fail(
                                                    "sem-capture-unavailable",
                                                    "callback property access requires an enclosing self",
                                                ));
                                            }
                                        };
                                        capture("self", source)?;
                                        capture_candidates[node.0] = true;
                                    }
                                }
                                // a nested callback's body is part of this scan:
                                // what it uses, this callback must capture first
                                if let Syntax::FunctionReference(nested) = ast.nodes[node.0].syntax
                                    && let Syntax::Function { body, .. } =
                                        ast.nodes[nested.0].syntax
                                {
                                    push(&mut pending, body)?;
                                }
                                let mut failure = None;
                                children(&ast.nodes[node.0].syntax, &mut |child| {
                                    if let Err(error) = push(&mut pending, child) {
                                        failure = Some(error);
                                    }
                                });
                                if let Some(error) = failure {
                                    return Err(error);
                                }
                            }
                            if !captured.is_empty() {
                                if *is_static {
                                    return Err(fail(
                                        "sem-capture-unavailable",
                                        "captured callbacks require a function scope",
                                    ));
                                }
                                let mut list = Vec::new();
                                for (_, capture) in &captured {
                                    push(&mut list, *capture)?;
                                }
                                for (_, source) in &captured {
                                    if let Capture::Local(slot) = source {
                                        capture_sites
                                            .try_reserve(1)
                                            .map_err(|_| Diagnostic::resource(0))?;
                                        capture_sites.entry(*slot).or_insert(id.0);
                                    }
                                }
                                result.captures[index] = list;
                                capture_scopes
                                    .try_reserve(1)
                                    .map_err(|_| Diagnostic::resource(0))?;
                                capture_scopes.insert(index, captured);
                            }
                            result.bindings[id.0] = Some(Binding::Function(index));
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
                            if !matches!(ast.nodes[id.0].syntax, Syntax::LocalConstruction { .. })
                                && !matches!(&ast.nodes[prototype.0].syntax, Syntax::Name(name) if objects.contains_key(name.as_str()))
                            {
                                return Err(fail(
                                    "sem-name",
                                    "construction requires a named object prototype",
                                ));
                            }
                            result.properties[id.0] = property_names.get("construct").copied();
                            push(&mut tasks, Task::Visit(*prototype, false, depth))?;
                            for argument in arguments.iter().rev() {
                                push(&mut tasks, Task::Visit(*argument, false, depth))?;
                            }
                        }
                        Syntax::StaticLookup { owner, property } => {
                            if !is_static {
                                return Err(fail(
                                    "sem-owner",
                                    "static table requires a field initializer",
                                ));
                            }
                            for child in [*owner, *property] {
                                push(&mut tasks, Task::Visit(child, false, depth))?;
                            }
                        }
                        Syntax::StaticRexPattern {
                            text,
                            owner,
                            property,
                        } => {
                            if !is_static {
                                return Err(fail(
                                    "sem-owner",
                                    "static pattern requires a field initializer",
                                ));
                            }
                            for child in [*text, *owner, *property] {
                                push(&mut tasks, Task::Visit(child, false, depth))?;
                            }
                        }
                        Syntax::StaticVector {
                            capacity,
                            owner,
                            property,
                        } => {
                            if !is_static {
                                return Err(fail(
                                    "sem-owner",
                                    "static vector requires a field initializer",
                                ));
                            }
                            for child in [*capacity, *owner, *property] {
                                push(&mut tasks, Task::Visit(child, false, depth))?;
                            }
                        }
                        Syntax::Construction {
                            prototype,
                            arguments,
                            owner,
                            property,
                        } => {
                            if !is_static {
                                return Err(fail(
                                    "sem-unavailable",
                                    "owned construction requires static field ownership",
                                ));
                            }
                            if !matches!(&ast.nodes[prototype.0].syntax, Syntax::Name(name) if objects.contains_key(name.as_str()))
                            {
                                return Err(fail(
                                    "sem-name",
                                    "construction requires a named object prototype",
                                ));
                            }
                            if !matches!(&ast.nodes[owner.0].syntax, Syntax::Name(name) if name == "self")
                                || !matches!(
                                    ast.nodes[property.0].syntax,
                                    Syntax::PropertyAddress(_)
                                )
                            {
                                return Err(fail(
                                    "sem-ast",
                                    "construction recipient must be the initializer field",
                                ));
                            }
                            result.properties[id.0] = property_names.get("construct").copied();
                            for child in [*prototype, *owner, *property] {
                                push(&mut tasks, Task::Visit(child, false, depth))?;
                            }
                            for argument in arguments.iter().rev() {
                                push(&mut tasks, Task::Visit(*argument, false, depth))?;
                            }
                        }
                        Syntax::LocalLookupSized(buckets, capacity) => {
                            // Under lifetimes the table belongs to the turn, so
                            // it needs no owning local.
                            if !ast.lifetimes && !owned_initializers[id.0] {
                                return Err(fail("sem-owner", "sized table requires local owned"));
                            }
                            push(&mut tasks, Task::Visit(*buckets, false, depth))?;
                            push(&mut tasks, Task::Visit(*capacity, false, depth))?;
                        }
                        Syntax::Lookup(entries) => {
                            if !ast.lifetimes && !owned_initializers[id.0] {
                                return Err(fail(
                                    "sem-owner",
                                    "lookup literal requires local owned",
                                ));
                            }
                            for (key, value) in entries.iter().rev() {
                                push(&mut tasks, Task::Visit(*value, false, depth))?;
                                if let Some(key) = key {
                                    push(&mut tasks, Task::Visit(*key, false, depth))?;
                                }
                            }
                        }
                        Syntax::ArgumentPack(items) | Syntax::ArgumentPackTail(items) => {
                            for item in items.iter().rev() {
                                push(&mut tasks, Task::Visit(*item, false, depth))?;
                            }
                        }
                        Syntax::List(items) => {
                            // Constant literals borrow module-owned storage.
                            if !is_static && plan.constant_lists[id.0].is_some() {
                                continue;
                            }
                            for item in items.iter().rev() {
                                push(&mut tasks, Task::Visit(*item, false, depth))?;
                            }
                        }
                        Syntax::Index(receiver, index) => {
                            push(&mut tasks, Task::Visit(*index, false, depth))?;
                            push(&mut tasks, Task::Visit(*receiver, false, depth))?;
                        }
                        Syntax::PropertyAddress(name) => {
                            // `&name` on a function is a function pointer, as in the
                            // reference compiler; a symbol is never both.
                            if !property_names.contains_key(name.as_str())
                                && let Some(index) = functions.get(name.as_str())
                            {
                                if callee {
                                    return Err(fail(
                                        "sem-unavailable",
                                        "calls through property values require a receiver",
                                    ));
                                }
                                result.bindings[id.0] = Some(Binding::Function(*index));
                                continue;
                            }
                            let property = *property_names
                                .get(name.as_str())
                                .ok_or_else(|| fail("sem-property", "unknown property address"))?;
                            if callee {
                                return Err(fail(
                                    "sem-unavailable",
                                    "calls through property values require a receiver",
                                ));
                            }
                            result.bindings[id.0] = Some(Binding::Property(property));
                        }
                        Syntax::Delegated(target) => {
                            let (_, property) = method_context.ok_or_else(|| {
                                fail("sem-context", "delegated requires a method")
                            })?;
                            result.bindings[id.0] = Some(Binding::Delegated(
                                self_slot.ok_or_else(|| fail("sem-context", "missing self"))?,
                                property,
                            ));
                            push(&mut tasks, Task::Visit(*target, false, depth))?;
                        }
                        Syntax::QualifiedInherited(name) => {
                            if !callee {
                                return Err(fail(
                                    "sem-context",
                                    "qualified inherited requires a call",
                                ));
                            }
                            let (_, property) = method_context.ok_or_else(|| {
                                fail("sem-context", "inherited requires a method")
                            })?;
                            let object = *objects.get(name.as_str()).ok_or_else(|| {
                                fail("sem-name", "qualified inherited requires a named prototype")
                            })?;
                            result.bindings[id.0] = Some(Binding::Inherited(
                                self_slot.ok_or_else(|| fail("sem-context", "missing self"))?,
                                property,
                                object,
                            ));
                        }
                        Syntax::Name(name) => {
                            let binding = if name == "inherited" {
                                let (object, property) = method_context.ok_or_else(|| {
                                    fail("sem-context", "inherited requires a method")
                                })?;
                                Binding::Inherited(
                                    self_slot.ok_or_else(|| fail("sem-context", "missing self"))?,
                                    property,
                                    object,
                                )
                            } else if let Some(slot) = visible.get(name.as_str()) {
                                Binding::Local(*slot)
                            } else if let Some(index) = captures_in_scope.get(name.as_str()) {
                                Binding::Captured(*index)
                            } else if let Some(function) = functions.get(name.as_str()) {
                                Binding::Function(*function)
                            } else if let Some(value) = enums.get(name.as_str()) {
                                Binding::Enumerator(*value)
                            } else if let Some(object) = objects.get(name.as_str()) {
                                Binding::Object(*object)
                            } else if let (Some(slot), Some(property)) =
                                (self_slot, property_names.get(name.as_str()))
                            {
                                Binding::SelfProperty(slot, *property)
                            } else if let (Some(index), Some(property)) = (
                                captures_in_scope.get("self"),
                                property_names.get(name.as_str()),
                            ) {
                                Binding::CapturedSelfProperty(*index, *property)
                            } else if capture_candidates[id.0] {
                                return Err(fail(
                                    "sem-capture-unavailable",
                                    "callback capture requires an explicit ownership contract",
                                ));
                            } else {
                                return Err(if external_functions.contains_key(name.as_str()) {
                                    fail(
                                        "sem-unresolved-external",
                                        "external function has no definition in this program",
                                    )
                                } else {
                                    fail("sem-name", "unknown name")
                                });
                            };
                            if callee
                                && matches!(
                                    binding,
                                    Binding::Local(_) | Binding::Object(_) | Binding::Enumerator(_)
                                )
                            {
                                return Err(fail(
                                    "sem-unavailable",
                                    "calls through values are not implemented",
                                ));
                            }
                            if !callee
                                && let Binding::Function(index) = binding
                                && matches!(
                                    ast.nodes[ast.functions[index].0].syntax,
                                    Syntax::Function {
                                        returns_owned: true,
                                        ..
                                    }
                                )
                            {
                                return Err(fail(
                                    "sem-unavailable",
                                    "owning function values are not implemented",
                                ));
                            }
                            result.bindings[id.0] = Some(binding);
                        }
                        Syntax::Membership {
                            value, candidates, ..
                        } => {
                            if candidates.is_empty() {
                                return Err(fail("sem-ast", "membership requires candidates"));
                            }
                            for candidate in candidates.iter().rev() {
                                push(&mut tasks, Task::Visit(*candidate, false, depth))?;
                            }
                            push(&mut tasks, Task::Visit(*value, false, depth))?;
                        }
                        Syntax::Move(child) => {
                            if !returns_owned {
                                return Err(fail(
                                    "sem-owner",
                                    "return move requires an owning function",
                                ));
                            }
                            push(&mut tasks, Task::Visit(*child, false, depth))?;
                        }
                        Syntax::OwnedCall(child) => {
                            push(&mut tasks, Task::Visit(*child, false, depth))?;
                        }
                        Syntax::Return(value) => {
                            if *returns_owned && inside_finally[id.0] {
                                return Err(fail(
                                    "sem-unavailable",
                                    "owning returns through author finalizers are not implemented",
                                ));
                            }
                            if *returns_owned
                                && !value.is_some_and(|v| {
                                    matches!(ast.nodes[v.0].syntax, Syntax::Move(_))
                                })
                            {
                                return Err(fail(
                                    "sem-owner",
                                    "owning function requires return move",
                                ));
                            }
                            if let Some(child) = value {
                                push(&mut tasks, Task::Visit(*child, false, depth))?;
                            }
                        }
                        Syntax::Group(child) | Syntax::ExpandedArgument(child) => {
                            push(&mut tasks, Task::Visit(*child, callee, depth))?
                        }
                        Syntax::Apply(target, arguments) => {
                            push(
                                &mut tasks,
                                Task::Visit(
                                    *target,
                                    matches!(
                                        ast.nodes[ungroup(ast, *target).0].syntax,
                                        Syntax::QualifiedInherited(_)
                                    ),
                                    depth,
                                ),
                            )?;
                            push(&mut tasks, Task::Visit(*arguments, false, depth))?;
                        }
                        Syntax::Call(target, args) => {
                            let target = ungroup(ast, *target);
                            if matches!(ast.nodes[target.0].syntax, Syntax::QualifiedInherited(_)) {
                                push(&mut tasks, Task::Visit(target, true, depth))?;
                                for arg in args.iter().rev() {
                                    push(&mut tasks, Task::Visit(*arg, false, depth))?;
                                }
                                continue;
                            }
                            let intrinsic = match &ast.nodes[target.0].syntax {
                                Syntax::Property(_, name)
                                    if !property_names.contains_key(name.as_str())
                                        && matches!(
                                            name.as_str(),
                                            "getDefaultValue"
                                                | "setDefaultValue"
                                                | "getEntryCount"
                                                | "getBucketCount"
                                                | "isKeyPresent"
                                        ) =>
                                {
                                    Some(match name.as_str() {
                                        "getDefaultValue" => Builtin::LookupDefault,
                                        "setDefaultValue" => Builtin::LookupSetDefault,
                                        "getEntryCount" => Builtin::LookupLength,
                                        "getBucketCount" => Builtin::LookupBuckets,
                                        _ => Builtin::LookupContains,
                                    })
                                }
                                Syntax::Property(_, name)
                                    if matches!(
                                        name.as_str(),
                                        "append"
                                            | "indexOf"
                                            | "removeRange"
                                            | "removeElementAt"
                                            | "insertAt"
                                    ) && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(if name == "insertAt" {
                                        Builtin::VectorInsert
                                    } else if name == "removeRange" {
                                        Builtin::VectorRemoveRange
                                    } else if name == "removeElementAt" {
                                        Builtin::VectorRemoveAt
                                    } else if name == "indexOf" {
                                        Builtin::VectorIndexOf
                                    } else {
                                        Builtin::VectorAppend
                                    })
                                }
                                Syntax::Property(_, name)
                                    if name == "sort"
                                        && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::VectorSort)
                                }
                                Syntax::Property(_, name)
                                    if matches!(
                                        name.as_str(),
                                        "indexWhich"
                                            | "valWhich"
                                            | "lastIndexWhich"
                                            | "countWhich"
                                            | "subset"
                                            | "mapAll"
                                            | "forEach"
                                    ) && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(match name.as_str() {
                                        "indexWhich" => Builtin::IndexWhich,
                                        "valWhich" => Builtin::ValWhich,
                                        "lastIndexWhich" => Builtin::LastIndexWhich,
                                        "countWhich" => Builtin::CountWhich,
                                        "subset" => Builtin::Subset,
                                        "mapAll" => Builtin::MapAll,
                                        _ => Builtin::ForEachItem,
                                    })
                                }
                                // A bare intrinsic method call runs on `self`,
                                // which a callback reaches through its capture.
                                Syntax::Name(name)
                                    if matches!(name.as_str(), "ofKind" | "propDefined")
                                        && !property_names.contains_key(name.as_str())
                                        && !functions.contains_key(name.as_str())
                                        && (self_slot.is_some()
                                            || captures_in_scope.contains_key("self")) =>
                                {
                                    result.bindings[target.0] = Some(match self_slot {
                                        Some(slot) => Binding::Local(slot),
                                        None => Binding::Captured(captures_in_scope["self"]),
                                    });
                                    Some(if name == "ofKind" {
                                        Builtin::OfKind
                                    } else {
                                        Builtin::PropDefined
                                    })
                                }
                                Syntax::Property(_, name)
                                    if matches!(name.as_str(), "ofKind" | "propDefined")
                                        && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(if name == "ofKind" {
                                        Builtin::OfKind
                                    } else {
                                        Builtin::PropDefined
                                    })
                                }
                                Syntax::Name(name)
                                    if matches!(
                                        name.as_str(),
                                        "captureClosure" | "closureCapture"
                                    ) && !functions.contains_key(name.as_str())
                                        && !visible.contains_key(name.as_str()) =>
                                {
                                    if name == "captureClosure" && !owned_calls[id.0] {
                                        return Err(fail(
                                            "sem-owner",
                                            "captured callback requires an owning initializer",
                                        ));
                                    }
                                    Some(if name == "captureClosure" {
                                        Builtin::CaptureClosure
                                    } else {
                                        Builtin::ClosureCapture
                                    })
                                }
                                // the parser writes this in place of a
                                // value string's interpolation; no author types it.
                                Syntax::Name(name) if name == "$valueText" => {
                                    Some(Builtin::ValueToText)
                                }
                                Syntax::Name(name)
                                    if matches!(name.as_str(), "firstObj" | "nextObj")
                                        && !functions.contains_key(name.as_str())
                                        && !visible.contains_key(name.as_str()) =>
                                {
                                    Some(if name == "firstObj" {
                                        Builtin::FirstObj
                                    } else {
                                        Builtin::NextObj
                                    })
                                }
                                Syntax::Name(name)
                                    if name == "newOwnedCollection"
                                        && !functions.contains_key(name.as_str())
                                        && !visible.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::NewOwnedCollection)
                                }
                                Syntax::Property(_, name)
                                    if !property_names.contains_key(name.as_str())
                                        && matches!(
                                            name.as_str(),
                                            "reserveInCollection"
                                                | "removeOwned"
                                                | "moveOwnedTo"
                                                | "moveFieldOwnerTo"
                                                | "ownedLength"
                                                | "ownedAt"
                                        ) =>
                                {
                                    Some(match name.as_str() {
                                        "reserveInCollection" => Builtin::ReserveInCollection,
                                        "removeOwned" => Builtin::RemoveOwned,
                                        "moveOwnedTo" => Builtin::MoveOwnedTo,
                                        "moveFieldOwnerTo" => Builtin::MoveFieldOwnerTo,
                                        "ownedLength" => Builtin::OwnedLength,
                                        _ => Builtin::OwnedAt,
                                    })
                                }
                                Syntax::Property(_, name)
                                    if name == "moveToCollection"
                                        && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::MoveLocalToCollection)
                                }
                                Syntax::Property(_, name)
                                    if name == "toList"
                                        && !property_names.contains_key(name.as_str()) =>
                                {
                                    if !owned_calls[id.0] {
                                        return Err(fail(
                                            "sem-owner",
                                            "toList snapshot requires an owned result",
                                        ));
                                    }
                                    Some(Builtin::OwnedToList)
                                }
                                Syntax::Property(_, name)
                                    if name == "moveToField"
                                        && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::MoveLocalToField)
                                }
                                Syntax::Property(_, name)
                                    if name == "reserveConstruction"
                                        && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::ReserveConstruction)
                                }
                                Syntax::Name(name)
                                    if !functions.contains_key(name.as_str())
                                        && !visible.contains_key(name.as_str())
                                        && !property_names.contains_key(name.as_str())
                                        && !enums.contains_key(name.as_str()) =>
                                {
                                    match name.as_str() {
                                        "dataType" => Some(Builtin::DataType),
                                        "toString" => Some(if owned_calls[id.0] {
                                            Builtin::OwnedToString
                                        } else {
                                            Builtin::ToString
                                        }),
                                        "toInteger" => Some(Builtin::ToInteger),
                                        "rexMatch" => Some(Builtin::RexMatch),
                                        "rexSearch" => Some(Builtin::RexSearch),
                                        "rexGroup" => Some(Builtin::RexGroup),
                                        "rexReplace" => Some(Builtin::RexReplace),
                                        "t3GetVMPreinitMode" => Some(Builtin::PreinitMode),
                                        "t3RunGC" => Some(Builtin::RunGc),
                                        "t3DebugTrace" => Some(Builtin::DebugTrace),
                                        "t3GetGlobalSymbols" => Some(Builtin::GlobalSymbols),
                                        "getTime" => Some(Builtin::GetTime),
                                        "savepoint" => Some(Builtin::Savepoint),
                                        "undo" => Some(Builtin::Undo),
                                        "undoDepth" => Some(Builtin::UndoDepth),
                                        "despawn" => Some(Builtin::Despawn),
                                        "event" => Some(Builtin::Event),
                                        "eventSubject" => Some(Builtin::EventSubject),
                                        "eventValue" => Some(Builtin::EventValue),
                                        "snapshotToken" => Some(Builtin::SnapshotToken),
                                        "hostAction" => Some(Builtin::HostAction),
                                        "engineQuery" => Some(Builtin::EngineQuery),
                                        "inputKey" => Some(Builtin::InputKey),
                                        "inputLine" => Some(Builtin::InputLine),
                                        "flushOutput" => Some(Builtin::FlushOutput),
                                        "beginTurn" => Some(Builtin::BeginTurn),
                                        "endTurn" => Some(Builtin::EndTurn),
                                        "turnJournalLength" => Some(Builtin::TurnJournalLength),
                                        _ => None,
                                    }
                                }
                                Syntax::Property(_, name)
                                    if !property_names.contains_key(name.as_str())
                                        && matches!(
                                            name.as_str(),
                                            "addWord" | "removeWord" | "findWord" | "isWordDefined"
                                        ) =>
                                {
                                    Some(match name.as_str() {
                                        "addWord" => Builtin::DictionaryAdd,
                                        "removeWord" => Builtin::DictionaryRemove,
                                        "findWord" => Builtin::DictionaryFind,
                                        _ => Builtin::DictionaryDefined,
                                    })
                                }
                                // `contains.set(a, b)` and the rest read
                                // the table the relation's name stands for.
                                Syntax::Property(receiver, name)
                                    if relation_descriptor(ast, *receiver).is_some()
                                        && relation_builtin(name).is_some() =>
                                {
                                    result.relations[id.0] = relation_descriptor(ast, *receiver);
                                    relation_builtin(name)
                                }
                                Syntax::Property(_, name)
                                    if name == "parseTokens"
                                        && !property_names.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::GrammarParse)
                                }
                                Syntax::Property(_, name)
                                    if name == "length"
                                        && !property_names.contains_key(name.as_str())
                                        && !enums.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::Length)
                                }
                                Syntax::Property(_, name)
                                    if name == "substr"
                                        && !property_names.contains_key(name.as_str())
                                        && !enums.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::Substr)
                                }
                                // systype.h String methods.
                                Syntax::Property(_, name)
                                    if matches!(
                                        name.as_str(),
                                        "startsWith"
                                            | "endsWith"
                                            | "toLower"
                                            | "toUpper"
                                            | "htmlify"
                                            | "findReplace"
                                    ) && !property_names.contains_key(name.as_str())
                                        && !enums.contains_key(name.as_str()) =>
                                {
                                    Some(match name.as_str() {
                                        "startsWith" => Builtin::StartsWith,
                                        "endsWith" => Builtin::EndsWith,
                                        "toLower" => Builtin::ToLower,
                                        "toUpper" => Builtin::ToUpper,
                                        "htmlify" => Builtin::Htmlify,
                                        _ => Builtin::FindReplace,
                                    })
                                }
                                Syntax::Property(_, name)
                                    if name == "find"
                                        && !property_names.contains_key(name.as_str())
                                        && !enums.contains_key(name.as_str()) =>
                                {
                                    Some(Builtin::FindText)
                                }
                                _ => None,
                            };
                            if let Syntax::Name(name) = &ast.nodes[target.0].syntax
                                && let Some((version, signature)) =
                                    intrinsic_functions.get(name.as_str())
                            {
                                if functions.contains_key(name.as_str())
                                    || visible.contains_key(name.as_str())
                                    || property_names.contains_key(name.as_str())
                                {
                                    return Err(fail(
                                        "sem-intrinsic",
                                        "intrinsic function name conflicts with a source binding",
                                    ));
                                }
                                let expected = match intrinsic {
                                    Some(
                                        Builtin::PreinitMode
                                        | Builtin::RunGc
                                        | Builtin::DebugTrace
                                        | Builtin::GlobalSymbols,
                                    ) => "t3vm/010006",
                                    Some(
                                        Builtin::InputKey
                                        | Builtin::InputLine
                                        | Builtin::FlushOutput,
                                    ) => "tads-io/030007",
                                    _ => "tads-gen/030008",
                                };
                                if intrinsic.is_none() || *version != expected {
                                    return Err(fail(
                                        "sem-intrinsic-unavailable",
                                        "declared intrinsic operation or version is not implemented",
                                    ));
                                }
                                let required = signature
                                    .parameters
                                    .iter()
                                    .filter(|(_, optional)| !optional)
                                    .count();
                                if args.len() < required
                                    || !signature.variadic
                                        && args.len() > signature.parameters.len()
                                {
                                    return Err(fail(
                                        "sem-arity",
                                        "wrong declared intrinsic argument count",
                                    ));
                                }
                                let valid_signature = match intrinsic {
                                    Some(Builtin::FirstObj | Builtin::NextObj) => {
                                        let next = intrinsic == Some(Builtin::NextObj);
                                        required == usize::from(next)
                                            && signature.parameters.len() == 2 + usize::from(next)
                                            && !signature.variadic
                                    }
                                    Some(Builtin::DataType) => {
                                        required == 1
                                            && signature.parameters.len() == 1
                                            && !signature.variadic
                                    }
                                    Some(Builtin::ToInteger) => {
                                        required == 1
                                            && signature.parameters.len() == 2
                                            && !signature.variadic
                                    }
                                    Some(Builtin::ToString | Builtin::OwnedToString) => {
                                        required == 1
                                            && signature.parameters.len() == 3
                                            && !signature.variadic
                                    }
                                    // t3.h: t3RunGC; t3GetVMPreinitMode;
                                    // t3DebugTrace(mode,...); t3GetGlobalSymbols(which?);
                                    // t3GetStackTrace(level?, flags?)
                                    // tadsgen.h: rexReplace(pat, str, replacement,
                                    // flags?, index?, limit?); index and limit are
                                    // not implemented, so calls may pass four.
                                    Some(Builtin::RexReplace) => {
                                        required == 3
                                            && signature.parameters.len() == 6
                                            && !signature.variadic
                                    }
                                    // tadsgen.h: rexGroup(groupNum)
                                    Some(Builtin::RexGroup) => {
                                        required == 1
                                            && signature.parameters.len() == 1
                                            && !signature.variadic
                                    }
                                    // tadsgen.h: rexMatch/rexSearch(pat, str, index?)
                                    Some(Builtin::RexMatch | Builtin::RexSearch) => {
                                        required == 2
                                            && signature.parameters.len() == 3
                                            && !signature.variadic
                                    }
                                    // tadsgen.h: getTime(timeType?)
                                    Some(Builtin::GetTime) => {
                                        required == 0
                                            && signature.parameters.len() == 1
                                            && !signature.variadic
                                    }
                                    // tadsgen.h: savepoint, undo; tadsio.h: inputKey
                                    Some(
                                        Builtin::PreinitMode
                                        | Builtin::RunGc
                                        | Builtin::Savepoint
                                        | Builtin::Undo
                                        | Builtin::UndoDepth
                                        | Builtin::InputKey
                                        | Builtin::InputLine
                                        | Builtin::FlushOutput,
                                    ) => signature.parameters.is_empty() && !signature.variadic,
                                    // despawn(entity), hostAction(code)
                                    // and engineQuery(code) each take
                                    // exactly one argument, which is what the call
                                    // site below requires of them too.
                                    Some(
                                        Builtin::Despawn
                                        | Builtin::HostAction
                                        | Builtin::EngineQuery
                                        | Builtin::Event
                                        | Builtin::EventValue
                                        | Builtin::SnapshotToken
                                        | Builtin::EventSubject,
                                    ) => {
                                        required == 1
                                            && signature.parameters.len() == 1
                                            && !signature.variadic
                                    }
                                    Some(Builtin::DebugTrace) => {
                                        required == 1 && signature.variadic
                                    }
                                    Some(Builtin::GlobalSymbols) => {
                                        required == 0
                                            && signature.parameters.len() == 1
                                            && !signature.variadic
                                    }
                                    _ => false,
                                };
                                if !valid_signature {
                                    return Err(fail(
                                        "sem-intrinsic",
                                        "intrinsic declaration does not match the supported contract",
                                    ));
                                }
                            }
                            if !ast.lifetimes
                                && (matches!(
                                    intrinsic,
                                    Some(
                                        Builtin::RexSearch
                                            | Builtin::RexGroup
                                            | Builtin::RexReplace
                                            | Builtin::Subset
                                            | Builtin::MapAll
                                            | Builtin::InputKey
                                            | Builtin::InputLine
                                            | Builtin::ToLower
                                            | Builtin::ToUpper
                                            | Builtin::Htmlify
                                            | Builtin::FindReplace
                                    )
                                ) || !is_static
                                    && matches!(intrinsic, Some(Builtin::DictionaryFind)))
                                && !owned_calls[id.0]
                            {
                                return Err(fail(
                                    "sem-owner",
                                    "a new list or text result requires an owned local",
                                ));
                            }
                            if let Some(intrinsic) = intrinsic {
                                if !ast.lifetimes
                                    && !is_static
                                    && !matches!(
                                        intrinsic,
                                        // text built at run time belongs to the
                                        // calling scope
                                        Builtin::ToString
                                            | Builtin::Substr
                                            | Builtin::RexMatch
                                            | Builtin::RexSearch
                                            | Builtin::RexGroup
                                            | Builtin::RexReplace
                                            | Builtin::PreinitMode
                                            | Builtin::RunGc
                                            | Builtin::DebugTrace
                                            | Builtin::GlobalSymbols
                                            | Builtin::CaptureClosure
                                            | Builtin::ClosureCapture
                                            | Builtin::OwnedToString
                                            | Builtin::OwnedToList
                                            | Builtin::LookupDefault
                                            | Builtin::LookupSetDefault
                                            | Builtin::LookupBuckets
                                            | Builtin::LookupLength
                                            | Builtin::LookupContains
                                            | Builtin::VectorAppend
                                            | Builtin::VectorIndexOf
                                            | Builtin::VectorRemoveAt
                                            | Builtin::VectorRemoveRange
                                            | Builtin::VectorInsert
                                            | Builtin::VectorSort
                                            | Builtin::IndexWhich
                                            | Builtin::ValWhich
                                            | Builtin::LastIndexWhich
                                            | Builtin::CountWhich
                                            | Builtin::ForEachItem
                                            | Builtin::Subset
                                            | Builtin::MapAll
                                            | Builtin::DataType
                                            | Builtin::DictionaryFind
                                            | Builtin::GetTime
                                            | Builtin::ToInteger
                                            | Builtin::InputKey
                                            | Builtin::InputLine
                                            | Builtin::FlushOutput
                                            | Builtin::BeginTurn
                                            | Builtin::EndTurn
                                            | Builtin::TurnJournalLength
                                            | Builtin::RelationSet
                                            | Builtin::RelationUnset
                                            | Builtin::RelationGet
                                            | Builtin::RelationAll
                                            // a vocabulary relation read
                                            // as data, which allocates exactly as
                                            // RelationAll does.
                                            | Builtin::VocabularyWords
                                            | Builtin::VocabularyNames
                                            | Builtin::RelationContains
                                            | Builtin::RelationOutermost
                                            | Builtin::RelationDescendants
                                            | Builtin::RelationAncestors
                                            | Builtin::StartsWith
                                            | Builtin::EndsWith
                                            | Builtin::FindText
                                            | Builtin::ToLower
                                            | Builtin::ToUpper
                                            | Builtin::Htmlify
                                            | Builtin::FindReplace
                                            | Builtin::Savepoint
                                            | Builtin::Despawn
                                            | Builtin::Event
                                            | Builtin::EventValue
                                        | Builtin::SnapshotToken
                                        | Builtin::EventSubject
                                            | Builtin::HostAction
                                            | Builtin::EngineQuery
                                            | Builtin::Undo
                                            | Builtin::UndoDepth
                                            | Builtin::Length
                                            | Builtin::DictionaryAdd
                                            | Builtin::DictionaryRemove
                                            | Builtin::DictionaryDefined
                                            | Builtin::GrammarParse
                                            | Builtin::MoveLocalToCollection
                                            | Builtin::MoveLocalToField
                                            | Builtin::ReserveConstruction
                                            | Builtin::ReserveInCollection
                                            | Builtin::RemoveOwned
                                            | Builtin::MoveOwnedTo
                                            | Builtin::MoveFieldOwnerTo
                                            | Builtin::OwnedLength
                                            | Builtin::OwnedAt
                                            | Builtin::FirstObj
                                            | Builtin::NextObj
                                            | Builtin::PropDefined
                                            | Builtin::OfKind
                                    )
                                {
                                    return Err(fail(
                                        "sem-unavailable",
                                        "heap-producing intrinsic outside static initializer ownership is not implemented",
                                    ));
                                }
                                // a labelled relation's operations take
                                // one more argument, naming the table the row is in.
                                let labelled = result.relations[id.0]
                                    .is_some_and(|descriptor| (descriptor >> 15) & 1 != 0);
                                let arity_ok = match intrinsic {
                                    Builtin::CaptureClosure => args.len() == 3,
                                    Builtin::ClosureCapture => args.len() == 2,
                                    // the flag is optional and means
                                    // instances when left off, as the reference has it.
                                    Builtin::ValueToText => args.len() == 1,
                                    Builtin::FirstObj => (1..=2).contains(&args.len()),
                                    Builtin::NextObj => (2..=3).contains(&args.len()),
                                    Builtin::OwnedToString => args.len() == 1,
                                    Builtin::OwnedToList => args.is_empty(),
                                    Builtin::NewOwnedCollection | Builtin::OwnedLength => {
                                        args.is_empty()
                                    }
                                    Builtin::LookupDefault
                                    | Builtin::LookupLength
                                    | Builtin::LookupBuckets => args.is_empty(),
                                    Builtin::LookupSetDefault
                                    | Builtin::LookupContains
                                    | Builtin::VectorAppend
                                    | Builtin::VectorIndexOf
                                    | Builtin::VectorRemoveAt
                                    | Builtin::MoveLocalToCollection
                                    | Builtin::ReserveInCollection
                                    | Builtin::RemoveOwned
                                    | Builtin::OwnedAt
                                    | Builtin::OfKind => args.len() == 1,
                                    // propDefined(prop, PropDefXxx?)
                                    Builtin::PropDefined => (1..=2).contains(&args.len()),
                                    Builtin::VectorRemoveRange => args.len() == 2,
                                    Builtin::VectorInsert => args.len() >= 2,
                                    Builtin::PreinitMode
                                    | Builtin::RunGc
                                    | Builtin::GlobalSymbols
                                    | Builtin::Savepoint
                                    | Builtin::Undo
                                    | Builtin::UndoDepth
                                    | Builtin::InputKey
                                    | Builtin::InputLine
                                    | Builtin::FlushOutput
                                    | Builtin::BeginTurn
                                    | Builtin::EndTurn
                                    | Builtin::TurnJournalLength => args.is_empty(),
                                    Builtin::Despawn
                                    | Builtin::HostAction
                                    | Builtin::EngineQuery
                                    | Builtin::EventValue
                                    | Builtin::SnapshotToken => args.len() == 1,
                                    // A labelled relation takes one more: which
                                    // of its tables the row belongs to.
                                    Builtin::RelationSet
                                    | Builtin::RelationUnset
                                    | Builtin::RelationContains => {
                                        args.len() == 2 + usize::from(labelled)
                                    }
                                    // leaving a labelled relation's label
                                    // off `all` answers across every table, which is
                                    // how a room lists the ways out of it.
                                    Builtin::RelationAll if labelled => {
                                        (1..=2).contains(&args.len())
                                    }
                                    Builtin::RelationGet
                                    | Builtin::RelationAll
                                    | Builtin::RelationOutermost
                                    | Builtin::RelationDescendants
                                    | Builtin::RelationAncestors => {
                                        args.len() == 1 + usize::from(labelled)
                                    }
                                    Builtin::GetTime => (0..=1).contains(&args.len()),
                                    Builtin::DebugTrace => args.len() == 1,
                                    Builtin::RexMatch | Builtin::RexSearch => {
                                        (2..=3).contains(&args.len())
                                    }
                                    Builtin::RexGroup => args.len() == 1,
                                    Builtin::RexReplace => (3..=4).contains(&args.len()),
                                    // In-place Vector sort; the comparator form only.
                                    Builtin::VectorSort => args.len() == 2,
                                    Builtin::IndexWhich
                                    | Builtin::ValWhich
                                    | Builtin::LastIndexWhich
                                    | Builtin::CountWhich
                                    | Builtin::ForEachItem
                                    | Builtin::Subset
                                    | Builtin::MapAll => args.len() == 1,
                                    Builtin::MoveFieldOwnerTo => args.len() == 3,
                                    Builtin::MoveOwnedTo
                                    | Builtin::ReserveConstruction
                                    | Builtin::MoveLocalToField => args.len() == 2,
                                    Builtin::Length | Builtin::ToLower | Builtin::ToUpper => {
                                        args.is_empty()
                                    }
                                    Builtin::StartsWith | Builtin::EndsWith => args.len() == 1,
                                    Builtin::Htmlify => args.len() <= 1,
                                    Builtin::FindText => (1..=2).contains(&args.len()),
                                    Builtin::FindReplace => (2..=5).contains(&args.len()),
                                    Builtin::DataType | Builtin::DictionaryDefined => {
                                        args.len() == 1
                                    }
                                    // parseTokens(tokens, dict, recipient): the explicit
                                    // recipient owns match trees (Zebulon profile change).
                                    Builtin::DictionaryAdd
                                    | Builtin::DictionaryRemove
                                    | Builtin::GrammarParse => {
                                        // without a recipient the match
                                        // trees belong to the turn.
                                        (2..=3).contains(&args.len())
                                    }
                                    _ => (1..=2).contains(&args.len()),
                                };
                                if !arity_ok {
                                    return Err(fail(
                                        "sem-arity",
                                        "wrong intrinsic argument count",
                                    ));
                                }
                                // `all` on a labelled relation without a
                                // label reads every table of the family. The
                                // boundary has no room for a fourth operand, so the
                                // descriptor says so instead.
                                if intrinsic == Builtin::RelationAll
                                    && labelled
                                    && args.len() == 1
                                    && let Some(descriptor) = result.relations[id.0]
                                {
                                    result.relations[id.0] = Some(descriptor | 1 << 28);
                                }
                                // the column annotations decide what each
                                // argument may be, so an argument that plainly is
                                // not that is refused here rather than at the
                                // run-time tag guard.
                                if let Some(descriptor) = result.relations[id.0]
                                    && relation_builtin_columns(intrinsic)
                                {
                                    let index =
                                        usize::try_from(descriptor & 0xfff).map_err(|_| {
                                            fail("sem-relation", "relation is not declared")
                                        })?;
                                    let declared = ast.relations.get(index).ok_or_else(|| {
                                        fail("sem-relation", "relation is not declared")
                                    })?;
                                    let reversed = (descriptor >> 12) & 1 != 0;
                                    let mut columns =
                                        relation_columns(intrinsic, declared.vocabulary, labelled);
                                    // A reverse name reads the table the other way,
                                    // so its two sides swap.
                                    if reversed
                                        && matches!(
                                            intrinsic,
                                            Builtin::RelationSet
                                                | Builtin::RelationUnset
                                                | Builtin::RelationContains
                                        )
                                    {
                                        columns.swap(0, 1);
                                    }
                                    if reversed && intrinsic == Builtin::RelationOutermost {
                                        return Err(fail(
                                            "sem-relation",
                                            "outermost uses the forward relation name; see language-contract.md, Reverse relation operations",
                                        ));
                                    }
                                    for (argument, column) in args.iter().zip(&columns) {
                                        // A lexical binding can shadow a global enumerator;
                                        // its computed value is checked at runtime.
                                        if matches!(&ast.nodes[ungroup(ast,*argument).0].syntax, Syntax::Name(name) if visible.contains_key(name.as_str()))
                                        {
                                            continue;
                                        }
                                        if *column == Column::Label {
                                            check_label_family(ast, declared, *argument)?;
                                        }
                                        if let Some(message) =
                                            column_mismatch(ast, &enums, *argument, *column)
                                        {
                                            return Err(fail("sem-relation-column", message));
                                        }
                                    }
                                }
                                // a vocabulary relation's rows are in the
                                // dictionary it was declared under, not in a
                                // relation table, so its operations are dictionary
                                // operations.
                                let mut intrinsic = intrinsic;
                                if let Some(descriptor) = result.relations[id.0]
                                    && relation_builtin_columns(intrinsic)
                                {
                                    let index =
                                        usize::try_from(descriptor & 0xfff).map_err(|_| {
                                            fail("sem-relation", "relation is not declared")
                                        })?;
                                    let declared = ast.relations.get(index).ok_or_else(|| {
                                        fail("sem-relation", "relation is not declared")
                                    })?;
                                    if declared.vocabulary {
                                        if (descriptor >> 12) & 1 != 0 {
                                            return Err(fail(
                                                "sem-vocabulary",
                                                "a vocabulary relation is read backwards by a grammar slot, not as a table",
                                            ));
                                        }
                                        let dictionary = declared
                                            .dictionary
                                            .as_deref()
                                            .and_then(|name| objects.get(name))
                                            .ok_or_else(|| {
                                                fail(
                                                    "sem-vocabulary",
                                                    "the vocabulary relation's dictionary is not declared",
                                                )
                                            })?;
                                        // a labelled vocabulary
                                        // relation's label is its part of
                                        // speech. A dictionary entry is (word,
                                        // object, property), so the label names
                                        // the property the row is filed under,
                                        // where an unlabelled relation uses its
                                        // forward name for every word.
                                        let labelled = declared.labelled;
                                        let named = if labelled {
                                            // The part of speech is the last
                                            // argument, so a call that omits it
                                            // is short rather than wrong: say
                                            // that, instead of complaining about
                                            // the shape of an argument that was
                                            // never meant to be the label.
                                            let wanted =
                                                relation_columns(intrinsic, true, true).len();
                                            if args.len() != wanted {
                                                return Err(fail(
                                                    "sem-vocabulary",
                                                    "a labelled vocabulary relation names a part of speech last, written &noun",
                                                ));
                                            }
                                            let label = args.last().ok_or_else(|| {
                                                fail(
                                                    "sem-vocabulary",
                                                    "a labelled vocabulary relation names a part of speech",
                                                )
                                            })?;
                                            match &ast.nodes[label.0].syntax {
                                                // `&noun` is how TADS names a
                                                // property, and a part of speech
                                                // is a property. An enumerator
                                                // would select a relation-family
                                                // label, not a vocabulary property.
                                                Syntax::PropertyAddress(name) => name.as_str(),
                                                _ => {
                                                    return Err(fail(
                                                        "sem-vocabulary",
                                                        "a vocabulary relation's part of speech is a property address, written &noun",
                                                    ));
                                                }
                                            }
                                        } else {
                                            declared.forward.as_str()
                                        };
                                        let property =
                                            property_names.get(named).ok_or_else(|| {
                                                fail(
                                                    "sem-vocabulary",
                                                    "the vocabulary relation's property is not declared",
                                                )
                                            })?;
                                        intrinsic = match intrinsic {
                                            Builtin::RelationSet => Builtin::DictionaryAdd,
                                            Builtin::RelationUnset => Builtin::DictionaryRemove,
                                            Builtin::RelationContains => Builtin::VocabularyNames,
                                            Builtin::RelationAll => Builtin::VocabularyWords,
                                            // A word is not a container, and a
                                            // word set has no single partner.
                                            _ => {
                                                return Err(fail(
                                                    "sem-vocabulary",
                                                    "a vocabulary relation supports set, unset, all and contains",
                                                ));
                                            }
                                        };
                                        result.vocabulary[id.0] =
                                            Some((*dictionary, *property, labelled));
                                        result.relations[id.0] = None;
                                    }
                                }
                                result.builtins[id.0] = Some(intrinsic);
                                // A relation's name stands for a table, not a
                                // value, so there is no receiver to evaluate.
                                // A vocabulary relation's name is a table too, even
                                // though its call lowered to a dictionary
                                // operation.
                                if let Syntax::Property(receiver, _) = ast.nodes[target.0].syntax
                                    && result.relations[id.0].is_none()
                                    && result.vocabulary[id.0].is_none()
                                {
                                    push(&mut tasks, Task::Visit(receiver, false, depth))?;
                                }
                                for arg in args.iter().rev() {
                                    push(&mut tasks, Task::Visit(*arg, false, depth))?;
                                }
                                continue;
                            }
                            if matches!(
                                ast.nodes[target.0].syntax,
                                Syntax::Property(..) | Syntax::IndirectProperty(..)
                            ) {
                                push(&mut tasks, Task::Visit(target, false, depth))?;
                                for arg in args.iter().rev() {
                                    push(&mut tasks, Task::Visit(*arg, false, depth))?;
                                }
                                continue;
                            }
                            // `delegated target(args)` dispatches the current
                            // property, not the value of the target expression.
                            if matches!(ast.nodes[target.0].syntax, Syntax::Delegated(_)) {
                                push(&mut tasks, Task::Visit(target, true, depth))?;
                                for arg in args.iter().rev() {
                                    push(&mut tasks, Task::Visit(*arg, false, depth))?;
                                }
                                continue;
                            }
                            // Calls through a local, or through a captured value,
                            // dispatch on the value.
                            if !matches!(&ast.nodes[target.0].syntax, Syntax::Name(name)
                                if !visible.contains_key(name.as_str())
                                    && !captures_in_scope.contains_key(name.as_str()))
                            {
                                result.builtins[id.0] = Some(Builtin::Invoke);
                                push(&mut tasks, Task::Visit(target, false, depth))?;
                                for arg in args.iter().rev() {
                                    push(&mut tasks, Task::Visit(*arg, false, depth))?;
                                }
                                continue;
                            }
                            let Syntax::Name(name) = &ast.nodes[target.0].syntax else {
                                unreachable!()
                            };
                            if name == "inherited" && method_context.is_some() {
                                push(&mut tasks, Task::Visit(target, true, depth))?;
                                for arg in args.iter().rev() {
                                    push(&mut tasks, Task::Visit(*arg, false, depth))?;
                                }
                                continue;
                            }
                            // A bare method call resolves against `self`, which a
                            // callback reaches through its capture.
                            if (self_slot.is_some() || captures_in_scope.contains_key("self"))
                                && property_names.contains_key(name.as_str())
                                && !visible.contains_key(name.as_str())
                                && !captures_in_scope.contains_key(name.as_str())
                                && !functions.contains_key(name.as_str())
                                && !objects.contains_key(name.as_str())
                            {
                                push(&mut tasks, Task::Visit(target, true, depth))?;
                                for arg in args.iter().rev() {
                                    push(&mut tasks, Task::Visit(*arg, false, depth))?;
                                }
                                continue;
                            }
                            if visible.contains_key(name.as_str()) {
                                return Err(fail(
                                    "sem-unavailable",
                                    "calls through values are not implemented",
                                ));
                            }
                            let function = functions.get(name.as_str()).ok_or_else(|| {
                                if external_functions.contains_key(name.as_str()) {
                                    fail(
                                        "sem-unresolved-external",
                                        "external function has no definition in this program",
                                    )
                                } else {
                                    fail("sem-name", "unknown function")
                                }
                            })?;
                            let Syntax::Function {
                                parameters,
                                rest,
                                optional,
                                ..
                            } = &ast.nodes[ast.functions[*function].0].syntax
                            else {
                                unreachable!()
                            };
                            let fixed = parameters.len() - usize::from(*rest);
                            if args.len() < fixed - optional || (!rest && args.len() > fixed) {
                                return Err(fail("sem-arity", "wrong positional argument count"));
                            }
                            let owns = matches!(
                                ast.nodes[ast.functions[*function].0].syntax,
                                Syntax::Function {
                                    returns_owned: true,
                                    ..
                                }
                            );
                            if owns != owned_calls[id.0] {
                                return Err(fail(
                                    "sem-owner",
                                    "owning function results require a local owned initializer",
                                ));
                            }
                            result.calls[id.0] = Some(*function);
                            push(&mut tasks, Task::Visit(target, true, depth))?;
                            for arg in args.iter().rev() {
                                push(&mut tasks, Task::Visit(*arg, false, depth))?;
                            }
                        }
                        Syntax::Label(name, body) => {
                            // Any statement may carry a goto label and be
                            // left with `break label` ; `continue label`
                            // still requires the label to name a loop.
                            let loops = matches!(
                                ast.nodes[body.0].syntax,
                                Syntax::ForEach(..)
                                    | Syntax::For { .. }
                                    | Syntax::While(..)
                                    | Syntax::DoWhile(..)
                            );
                            if labels.iter().any(|(label, _)| *label == name.as_str()) {
                                return Err(fail("sem-label", "duplicate enclosing label"));
                            }
                            push(&mut labels, (name.as_str(), loops))?;
                            push(&mut tasks, Task::LabelExit)?;
                            push(&mut tasks, Task::Visit(*body, false, depth))?;
                        }
                        // Target validity is checked per function by validate_gotos.
                        Syntax::Goto(_) => {}
                        Syntax::NamedBreak(name) => {
                            if !labels.iter().any(|(label, _)| *label == name.as_str()) {
                                return Err(fail(
                                    "sem-label",
                                    "label is not an enclosing statement",
                                ));
                            }
                        }
                        Syntax::NamedContinue(name) => {
                            if !labels
                                .iter()
                                .any(|(label, loops)| *label == name.as_str() && *loops)
                            {
                                return Err(fail("sem-label", "label is not an enclosing loop"));
                            }
                        }
                        Syntax::Block(items) => {
                            push(&mut scopes, Vec::new())?;
                            push(&mut tasks, Task::Exit)?;
                            for item in items.iter().rev() {
                                push(&mut tasks, Task::Visit(*item, false, depth))?;
                            }
                        }
                        Syntax::Local(items) | Syntax::OwnedLocal(items) => {
                            for ordinal in (0..items.len()).rev() {
                                push(&mut tasks, Task::Declare(id, ordinal, depth))?;
                            }
                        }
                        Syntax::OwnerScope(body) | Syntax::LocalVector(body) => {
                            push(&mut tasks, Task::Visit(*body, false, depth))?;
                        }
                        Syntax::Finally(body, finalizer) => {
                            push(&mut tasks, Task::Visit(*finalizer, false, depth))?;
                            push(&mut tasks, Task::Visit(*body, false, depth))?;
                        }
                        Syntax::Try(body, catches) => {
                            for (class, declaration, handler) in catches.iter().rev() {
                                push(
                                    &mut tasks,
                                    Task::Catch(*class, *declaration, *handler, depth),
                                )?;
                            }
                            push(&mut tasks, Task::Visit(*body, false, depth))?;
                        }
                        Syntax::ForEach(declaration, collection, body) => {
                            push(&mut scopes, Vec::new())?;
                            push(&mut tasks, Task::Exit)?;
                            push(
                                &mut tasks,
                                Task::Visit(
                                    *body,
                                    false,
                                    Control {
                                        in_loop: true,
                                        can_break: true,
                                    },
                                ),
                            )?;
                            push(&mut tasks, Task::Visit(*declaration, false, depth))?;
                            push(&mut tasks, Task::Visit(*collection, false, depth))?;
                        }
                        Syntax::For {
                            init,
                            condition,
                            step,
                            body,
                        } => {
                            push(&mut scopes, Vec::new())?;
                            push(&mut tasks, Task::Exit)?;
                            push(
                                &mut tasks,
                                Task::Visit(
                                    *body,
                                    false,
                                    Control {
                                        in_loop: true,
                                        can_break: true,
                                    },
                                ),
                            )?;
                            if let Some(step) = step {
                                push(&mut tasks, Task::Visit(*step, false, depth))?;
                            }
                            if let Some(condition) = condition {
                                push(&mut tasks, Task::Visit(*condition, false, depth))?;
                            }
                            for initializer in init.iter().rev() {
                                push(&mut tasks, Task::Visit(*initializer, false, depth))?;
                            }
                        }
                        Syntax::DoWhile(condition, body) => {
                            push(&mut tasks, Task::Visit(*condition, false, depth))?;
                            push(
                                &mut tasks,
                                Task::Visit(
                                    *body,
                                    false,
                                    Control {
                                        in_loop: true,
                                        can_break: true,
                                    },
                                ),
                            )?;
                        }
                        Syntax::While(condition, body) => {
                            push(
                                &mut tasks,
                                Task::Visit(
                                    *body,
                                    false,
                                    Control {
                                        in_loop: true,
                                        can_break: true,
                                    },
                                ),
                            )?;
                            push(&mut tasks, Task::Visit(*condition, false, depth))?;
                        }
                        Syntax::Switch(selector, arms) => {
                            let mut cases = Vec::new();
                            for (case, _) in arms {
                                if let Some(case) = case {
                                    let key = case_key(ast, *case, &case_constants, &enums)?;
                                    if cases.contains(&key) {
                                        return Err(fail("sem-switch", "duplicate case value"));
                                    }
                                    push(&mut cases, key)?;
                                }
                            }
                            push(&mut scopes, Vec::new())?;
                            push(&mut tasks, Task::Exit)?;
                            let inner = Control {
                                in_loop: depth.in_loop,
                                can_break: true,
                            };
                            for (case, statements) in arms.iter().rev() {
                                for statement in statements.iter().rev() {
                                    push(&mut tasks, Task::Visit(*statement, false, inner))?;
                                }
                                if let Some(case) = case {
                                    push(&mut tasks, Task::Visit(*case, false, depth))?;
                                }
                            }
                            push(&mut tasks, Task::Visit(*selector, false, depth))?;
                        }
                        Syntax::Break if !depth.can_break => {
                            return Err(fail(
                                "sem-control-target",
                                "break is outside a loop or switch",
                            ));
                        }
                        Syntax::Continue if !depth.in_loop => {
                            return Err(fail(
                                "sem-control-target",
                                "loop control is outside a loop",
                            ));
                        }
                        Syntax::Binary(op, left, right) => {
                            if assignment(op) {
                                let target = ungroup(ast, *left);
                                if inherited_destination(ast, target) {
                                    return Err(fail(
                                        "sem-destination",
                                        "inherited access cannot be assigned",
                                    ));
                                }
                                if matches!(ast.nodes[target.0].syntax, Syntax::Index(..))
                                    && *op != "="
                                {
                                    return Err(fail(
                                        "sem-unavailable",
                                        "compound indexed assignment is not implemented",
                                    ));
                                }
                                if !matches!(&ast.nodes[target.0].syntax,Syntax::Name(name) if visible.contains_key(name.as_str()) || (self_slot.is_some() && property_names.contains_key(name.as_str()) && !functions.contains_key(name.as_str()) && !objects.contains_key(name.as_str())))
                                    && !matches!(
                                        ast.nodes[target.0].syntax,
                                        Syntax::Property(..)
                                            | Syntax::IndirectProperty(..)
                                            | Syntax::Index(..)
                                    )
                                {
                                    return Err(fail(
                                        "sem-destination",
                                        "assignment needs a local, parameter or property",
                                    ));
                                }
                            }
                            push(&mut tasks, Task::Visit(*right, false, depth))?;
                            push(&mut tasks, Task::Visit(*left, false, depth))?;
                        }
                        Syntax::Unary(op, child, _) => {
                            if matches!(*op, "++" | "--") {
                                let target = ungroup(ast, *child);
                                if inherited_destination(ast, target) {
                                    return Err(fail(
                                        "sem-destination",
                                        "inherited access cannot be incremented",
                                    ));
                                }
                                if matches!(
                                    ast.nodes[target.0].syntax,
                                    Syntax::IndirectProperty(..)
                                ) {
                                    return Err(fail(
                                        "sem-unavailable",
                                        "indirect property increments are not implemented",
                                    ));
                                }
                                if !matches!(&ast.nodes[target.0].syntax,Syntax::Name(name) if visible.contains_key(name.as_str()) || (self_slot.is_some() && property_names.contains_key(name.as_str()) && !functions.contains_key(name.as_str()) && !objects.contains_key(name.as_str())))
                                    && !matches!(ast.nodes[target.0].syntax, Syntax::Property(..))
                                {
                                    return Err(fail(
                                        "sem-destination",
                                        "increment needs a local, parameter or property",
                                    ));
                                }
                            }
                            push(&mut tasks, Task::Visit(*child, false, depth))?;
                        }
                        Syntax::IndirectProperty(receiver, key) => {
                            if inherited_destination(ast, *receiver) {
                                return Err(fail(
                                    "sem-unavailable",
                                    "indirect inherited selection is not implemented",
                                ));
                            }
                            push(&mut tasks, Task::Visit(*key, false, depth))?;
                            push(&mut tasks, Task::Visit(*receiver, false, depth))?;
                        }
                        Syntax::Property(receiver, name) => {
                            if callee {
                                return Err(fail(
                                    "sem-unavailable",
                                    "method dispatch is not implemented",
                                ));
                            }
                            let property = if let Some(index) = property_names.get(name.as_str()) {
                                *index
                            } else {
                                let index = u32::try_from(property_names.len())
                                    .map_err(|_| Diagnostic::resource(0))?;
                                property_names
                                    .try_reserve(1)
                                    .map_err(|_| Diagnostic::resource(0))?;
                                property_names.insert(name.as_str(), index);
                                index
                            };
                            result.properties[id.0] = Some(property);
                            // `inherited Class.prop(...)` names both the class to
                            // inherit from and the property.
                            if let Syntax::QualifiedInherited(class) =
                                &ast.nodes[ungroup(ast, *receiver).0].syntax
                            {
                                let object = *objects.get(class.as_str()).ok_or_else(|| {
                                    fail(
                                        "sem-name",
                                        "qualified inherited requires a named prototype",
                                    )
                                })?;
                                result.bindings[id.0] = Some(Binding::Inherited(
                                    self_slot.ok_or_else(|| fail("sem-context", "missing self"))?,
                                    property,
                                    object,
                                ));
                            } else if matches!(&ast.nodes[ungroup(ast, *receiver).0].syntax, Syntax::Name(name) if name == "inherited")
                            {
                                let (object, _) = method_context.ok_or_else(|| {
                                    fail("sem-context", "inherited requires a method")
                                })?;
                                result.bindings[id.0] = Some(Binding::Inherited(
                                    self_slot.ok_or_else(|| fail("sem-context", "missing self"))?,
                                    property,
                                    object,
                                ));
                            } else {
                                push(&mut tasks, Task::Visit(*receiver, false, depth))?;
                            }
                        }
                        Syntax::Interpolation { parts, .. } => {
                            // an emitting interpolation writes its
                            // parts out; a value one joins them into text. Both
                            // evaluate the same expressions.
                            for part in parts.iter().rev() {
                                push(&mut tasks, Task::Visit(*part, false, depth))?;
                            }
                        }
                        Syntax::Object { .. } => {
                            return Err(fail(
                                "sem-unavailable",
                                "object expressions are not yet lowered",
                            ));
                        }
                        Syntax::Function { .. } => {
                            return Err(fail("sem-ast", "nested function node in scalar body"));
                        }
                        syntax => {
                            let nested = match syntax {
                                Syntax::Conditional(a, b, c) => [Some(*a), Some(*b), Some(*c)],
                                Syntax::If(a, b, c) => [Some(*a), Some(*b), *c],
                                Syntax::Throw(child) => [Some(*child), None, None],
                                Syntax::Return(child) => [*child, None, None],
                                Syntax::Expression(child) => [Some(*child), None, None],
                                _ => [None; 3],
                            };
                            for child in nested.into_iter().rev().flatten() {
                                push(&mut tasks, Task::Visit(child, false, depth))?;
                            }
                        }
                    }
                }
            }
        }
    }
    for node in &ast.nodes {
        if let Syntax::Switch(_, arms) = &node.syntax {
            for (case, _) in arms {
                if let Some(case) = case {
                    let case = ungroup(ast, *case);
                    if matches!(
                        ast.nodes[case.0].syntax,
                        Syntax::QualifiedInherited(_) | Syntax::Name(_)
                    ) && !matches!(
                        result.bindings[case.0],
                        Some(Binding::Enumerator(_) | Binding::Object(_))
                    ) {
                        return Err(error(
                            ast,
                            case,
                            "sem-switch",
                            "case label is shadowed or is not a constant",
                        ));
                    }
                }
            }
        }
    }
    for (index, node) in ast.nodes.iter().enumerate() {
        if matches!(
            result.builtins[index],
            Some(Builtin::MoveLocalToField | Builtin::MoveLocalToCollection)
        ) {
            let Syntax::Call(target, _) = &node.syntax else {
                unreachable!()
            };
            let Syntax::Property(receiver, _) = ast.nodes[ungroup(ast, *target).0].syntax else {
                unreachable!()
            };
            let receiver = ungroup(ast, receiver);
            if !matches!(result.bindings[receiver.0], Some(Binding::Local(local)) if matches!(ast.nodes[result.locals[local].declaration.0].syntax, Syntax::OwnedLocal(_)))
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-owner",
                    "owner transfer requires a local owned receiver",
                ));
            }
        }
        if let Syntax::Move(child) = node.syntax {
            let child = ungroup(ast, child);
            if !matches!(result.bindings[child.0], Some(Binding::Local(local)) if matches!(ast.nodes[result.locals[local].declaration.0].syntax, Syntax::OwnedLocal(_)))
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-owner",
                    "move requires a local owned binding",
                ));
            }
        }
        // A captured environment belongs to the enclosing scope, so a capturing
        // callback may be a temporary or an owned local, but must not escape it.
        let escaping = match &node.syntax {
            Syntax::Return(Some(value)) => Some(*value),
            Syntax::Binary(op, target, value) if assignment(op) => {
                let target = ungroup(ast, *target);
                let property = matches!(
                    ast.nodes[target.0].syntax,
                    Syntax::Property(..) | Syntax::IndirectProperty(..)
                ) || matches!(
                    result.bindings[target.0],
                    Some(Binding::SelfProperty(..) | Binding::CapturedSelfProperty(..))
                );
                property.then_some(*value)
            }
            _ => None,
        };
        if let Some(value) = escaping
            && !inside_static[index]
        {
            let mut value = ungroup(ast, value);
            if let Syntax::OwnedCall(inner) | Syntax::Move(inner) = ast.nodes[value.0].syntax {
                value = ungroup(ast, inner);
            }
            if let Syntax::FunctionReference(function) = ast.nodes[value.0].syntax
                && let Some(callback) = ast.functions.iter().position(|id| *id == function)
                && !result.captures[callback].is_empty()
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-capture-unavailable",
                    "a captured callback cannot outlive the scope that captured it",
                ));
            }
            // Text built at run time belongs to this scope too , so it
            // leaves only through an owning return of an owned local. With
            // lifetimes there is no scope to outlive.
            if !ast.lifetimes
                && (matches!(
                    result.builtins[value.0],
                    Some(Builtin::ToString | Builtin::OwnedToString | Builtin::Substr)
                ) || matches!(ast.nodes[value.0].syntax, Syntax::List(_))
                    && plan.constant_lists[value.0].is_none())
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-owner",
                    "a value built here cannot outlive its scope; bind an owned local and move it",
                ));
            }
        }
        // A capture takes the value, so a later assignment would not be seen by
        // the callback, unlike the reference's shared variable.
        let assigned = match &node.syntax {
            Syntax::Binary(op, target, _) if assignment(op) => Some(*target),
            Syntax::Unary("++" | "--", target, _) => Some(*target),
            _ => None,
        };
        if let Some(target) = assigned
            && let Some(Binding::Local(slot)) = result.bindings[ungroup(ast, target).0]
            && capture_sites.get(&slot).is_some_and(|site| *site < index)
        {
            return Err(error(
                ast,
                Id(index),
                "sem-capture-unavailable",
                "assignment to a captured local after its capture is not implemented",
            ));
        }
        // Only a capturing callback allocates, so only it may own an initializer.
        if let Syntax::OwnedCall(child) = node.syntax
            && let Syntax::FunctionReference(function) = ast.nodes[child.0].syntax
        {
            let index = ast
                .functions
                .iter()
                .position(|id| *id == function)
                .ok_or_else(|| error(ast, Id(index), "sem-ast", "missing callback function"))?;
            if result.captures[index].is_empty() {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-owner",
                    "owned initializer requires a capturing callback",
                ));
            }
        }
        if let Syntax::OwnedCall(child) = node.syntax
            && !matches!(
                ast.nodes[child.0].syntax,
                Syntax::FunctionReference(_)
                    | Syntax::Binary("+", _, _)
                    | Syntax::List(_)
                    | Syntax::String(_)
            )
            && result.calls[child.0].is_none()
            && !matches!(
                result.builtins[child.0],
                Some(
                    Builtin::OwnedToString
                        | Builtin::OwnedToList
                        | Builtin::CaptureClosure
                        | Builtin::RexSearch
                        | Builtin::RexGroup
                        | Builtin::RexReplace
                        | Builtin::Subset
                        | Builtin::MapAll
                        | Builtin::Substr
                        | Builtin::DictionaryFind
                        | Builtin::InputKey
                        | Builtin::InputLine
                        | Builtin::ToLower
                        | Builtin::ToUpper
                        | Builtin::Htmlify
                        | Builtin::FindReplace
                )
            )
        {
            let Syntax::Call(target, _) = ast.nodes[child.0].syntax else {
                unreachable!()
            };
            let target = ungroup(ast, target);
            if result.builtins[child.0].is_some()
                || !matches!(
                    ast.nodes[target.0].syntax,
                    Syntax::Property(..) | Syntax::IndirectProperty(..)
                )
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-unavailable",
                    "owning call requires a named function or explicit method",
                ));
            }
        }
        let target = match &node.syntax {
            // `t += x` keeps the slot's ownership, handing its owner
            // entry to the text the append builds, so it is a transfer rather
            // than a reassignment. Every other form still needs an explicit one.
            Syntax::Binary("+=", _, _) => None,
            Syntax::Binary(op, left, _) if assignment(op) => Some(*left),
            Syntax::Unary("++" | "--", child, _) | Syntax::Return(Some(child)) => Some(*child),
            _ => None,
        };
        if let Some(target) = target {
            let target = ungroup(ast, target);
            if let Some(Binding::Local(local)) = result.bindings[target.0]
                && matches!(
                    ast.nodes[result.locals[local].declaration.0].syntax,
                    Syntax::OwnedLocal(_)
                )
            {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-owner",
                    "owned locals cannot be reassigned or returned without an explicit ownership transfer",
                ));
            }
        }
    }
    result.constants = case_constants;
    Ok(result)
}

/// Which anonymous callbacks capture their enclosing scope, by function index.
/// A capturing callback takes its environment as a synthetic first parameter,
/// so native dispatch must expect one more argument than the source declares.
pub fn capturing_callbacks(ast: &Ast) -> Result<Vec<bool>, Diagnostic> {
    let analysis = analyze(ast)?;
    let mut flags = Vec::new();
    flags
        .try_reserve_exact(analysis.captures.len())
        .map_err(|_| Diagnostic::resource(0))?;
    flags.extend(
        analysis
            .captures
            .iter()
            .map(|captures| !captures.is_empty()),
    );
    Ok(flags)
}

/// Native calls whose successful result must be adopted into the caller scope.
pub(crate) fn owned_call_sites(ast: &Ast) -> Result<Vec<bool>, Diagnostic> {
    let mut sites = Vec::new();
    sites
        .try_reserve_exact(ast.nodes.len())
        .map_err(|_| Diagnostic::resource(0))?;
    sites.resize(ast.nodes.len(), false);
    for (index, node) in ast.nodes.iter().enumerate() {
        if let Syntax::OwnedCall(call) = node.syntax {
            // A callback that captures its scope allocates an environment, so it
            // is an owning initializer like a factory call , and so is
            // a concatenation, which builds new text.
            if matches!(
                ast.nodes[call.0].syntax,
                Syntax::FunctionReference(_) | Syntax::Binary("+", _, _) | Syntax::List(_)
            ) {
                sites[call.0] = true;
                continue;
            }
            // `local owned t = ''` starts an accumulator. The slot owns
            // a copy of the text, so later appends replace it in place.
            if matches!(ast.nodes[call.0].syntax, Syntax::String(_)) {
                sites[call.0] = true;
                continue;
            }
            if !matches!(ast.nodes[call.0].syntax, Syntax::Call(_, _)) {
                return Err(error(
                    ast,
                    Id(index),
                    "sem-owner",
                    "owned initializer requires a factory call",
                ));
            }
            sites[call.0] = true;
        }
    }
    Ok(sites)
}
