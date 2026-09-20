//! Fixed-point scalar initialization and type facts over the shared IR.
use crate::{
    Diagnostic,
    ir::{self, Binary, BranchMode, Failure, Function, Operation, Terminator, Unary},
    parser::{Ast, Id},
    sema::Scalar,
};

const INTEGER: u16 = 1;
const NIL: u16 = 2;
const TRUE: u16 = 4;
const LOGICAL: u16 = NIL | TRUE;
const OBJECT: u16 = 8;
const STRING: u16 = 16;
const PROPERTY: u16 = 32;
const LIST: u16 = 64;
const ENUM: u16 = 128;
const FUNCTION: u16 = 256;
const UNKNOWN: u16 = INTEGER | LOGICAL | OBJECT | STRING | PROPERTY | LIST | ENUM | FUNCTION;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fact {
    initialized: bool,
    types: u16,
}
impl Fact {
    const EMPTY: Self = Self {
        initialized: false,
        types: 0,
    };
    const PARAMETER: Self = Self {
        initialized: true,
        types: UNKNOWN,
    };
    fn merge(&mut self, other: Self) -> bool {
        let old = *self;
        self.initialized &= other.initialized;
        self.types |= other.types;
        *self != old
    }
}
#[derive(Debug)]
pub struct FunctionFacts {
    pub reachable: Vec<bool>,
    /// Bit set: integer=1, nil=2, true=4, object=8, string=16, property=32, list=64, enum=128. Zero means no reachable definition.
    pub value_types: Vec<u16>,
    /// Operations that native emission must still implement with source outcomes.
    pub runtime_checks: Vec<Id>,
}
#[derive(Debug)]
pub struct Checked {
    pub program: ir::Program,
    pub facts: Vec<FunctionFacts>,
}
fn filled<T: Clone>(size: usize, value: T) -> Result<Vec<T>, Diagnostic> {
    let mut v = Vec::new();
    v.try_reserve_exact(size)
        .map_err(|_| Diagnostic::resource(0))?;
    v.resize(size, value);
    Ok(v)
}
fn copy<T: Clone>(v: &[T]) -> Result<Vec<T>, Diagnostic> {
    let mut out = Vec::new();
    out.try_reserve_exact(v.len())
        .map_err(|_| Diagnostic::resource(0))?;
    out.extend_from_slice(v);
    Ok(out)
}
fn push<T>(v: &mut Vec<T>, item: T) -> Result<(), Diagnostic> {
    v.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    v.push(item);
    Ok(())
}
fn error(ast: &Ast, site: Id, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(code, message, ast.nodes[site.0].start)
}
fn scalar(value: Scalar) -> u16 {
    match value {
        Scalar::Integer(_) => INTEGER,
        Scalar::Nil => NIL,
        Scalar::True => TRUE,
    }
}
fn binary_result(op: Binary) -> u16 {
    match op {
        Binary::Equal
        | Binary::NotEqual
        | Binary::Less
        | Binary::Greater
        | Binary::LessEqual
        | Binary::GreaterEqual => LOGICAL,
        _ => INTEGER,
    }
}
fn targets(term: &Terminator, values: &[u16]) -> [Option<usize>; 2] {
    match *term {
        Terminator::Invoke { normal, exception } => [Some(normal.0), Some(exception.target.0)],
        Terminator::Throw(_, _, Some(edge)) | Terminator::Resume(Some(edge)) => {
            [Some(edge.target.0), None]
        }
        Terminator::Jump(target) => [Some(target.0), None],
        Terminator::Branch {
            condition,
            mode,
            yes,
            no,
        } => {
            let types = values[condition.0];
            if types == 0 {
                return [None, None];
            }
            let yes_only = match mode {
                BranchMode::Logical => types == TRUE,
                BranchMode::IsNil => types == NIL,
            };
            let no_only = match mode {
                BranchMode::Logical => types == NIL,
                BranchMode::IsNil => types & NIL == 0,
            };
            [(!no_only).then_some(yes.0), (!yes_only).then_some(no.0)]
        }
        _ => [None, None],
    }
}
fn transfer(
    f: &Function,
    index: usize,
    slots: &mut [Fact],
    values: &mut [u16],
    integer_callees: &[bool],
) -> (bool, bool) {
    let mut changed = false;
    for i in &f.blocks[index].instructions {
        if i.operation
            .computed_property()
            .is_some_and(|value| values[value.0] == 0)
        {
            return (changed, false);
        }
        let types = match &i.operation {
            Operation::Constant(v) => scalar(*v),
            Operation::NamedObject(_) => OBJECT,
            Operation::PropertyValue(_) => PROPERTY,
            Operation::Enumerator(_) => ENUM,
            Operation::FunctionValue(_) => FUNCTION,
            Operation::StringLiteral(_) => STRING,
            Operation::EmitLiteral(_) | Operation::EndStatic => 0,
            Operation::SetProperty(receiver, _, value) => {
                if values[receiver.0] == 0 || values[value.0] == 0 {
                    return (changed, false);
                }
                0
            }
            Operation::EmitValue(receiver) | Operation::BeginStatic(receiver, _) => {
                if values[receiver.0] == 0 {
                    return (changed, false);
                }
                0
            }
            Operation::GetProperty(receiver, _) => {
                if values[receiver.0] == 0 {
                    return (changed, false);
                }
                UNKNOWN
            }
            Operation::Load(slot) => {
                let fact = slots[slot.0];
                if fact.initialized {
                    fact.types
                } else {
                    UNKNOWN
                }
            }
            Operation::Reset(slot) => {
                slots[slot.0] = Fact::EMPTY;
                0
            }
            Operation::Store(slot, value) => {
                if values[value.0] == 0 {
                    return (changed, false);
                }
                slots[slot.0] = Fact {
                    initialized: true,
                    types: values[value.0],
                };
                0
            }
            Operation::Unary(op, value) => {
                if values[value.0] == 0 {
                    return (changed, false);
                }
                if *op == Unary::Not { LOGICAL } else { INTEGER }
            }
            Operation::Binary(op, left, right) => {
                if values[left.0] == 0 || values[right.0] == 0 {
                    return (changed, false);
                }
                binary_result(*op)
            }
            Operation::LogicalGuard(value) => {
                if values[value.0] == 0 {
                    return (changed, false);
                }
                0
            }
            Operation::Builtin { kind, arguments } => {
                if arguments.iter().any(|v| values[v.0] == 0) {
                    return (changed, false);
                }
                match kind {
                    crate::sema::Builtin::NewLocalStringBuffer
                    | crate::sema::Builtin::NewLocalOwnedCollection
                    | crate::sema::Builtin::NewLocalLookupSized
                    | crate::sema::Builtin::NewLocalLookup
                    | crate::sema::Builtin::VectorAppend
                    | crate::sema::Builtin::VectorRemoveAt
                    | crate::sema::Builtin::VectorRemoveRange
                    | crate::sema::Builtin::VectorInsert
                    | crate::sema::Builtin::NewStaticLookup
                    | crate::sema::Builtin::NewStaticVector
                    | crate::sema::Builtin::NewStaticRexPattern
                    | crate::sema::Builtin::NewLocalVector
                    | crate::sema::Builtin::NewOwnedCollection
                    | crate::sema::Builtin::OwnedAt
                    | crate::sema::Builtin::BeginPublishedConstruction
                    | crate::sema::Builtin::FinishLocalConstruction
                    | crate::sema::Builtin::FinishException
                    | crate::sema::Builtin::FinishPublishedConstruction
                    | crate::sema::Builtin::BeginConstruction
                    | crate::sema::Builtin::FinishConstruction => OBJECT,
                    crate::sema::Builtin::FirstObj | crate::sema::Builtin::NextObj => OBJECT | NIL,
                    crate::sema::Builtin::CaptureClosure => OBJECT,
                    crate::sema::Builtin::ClosureCapture => UNKNOWN,
                    crate::sema::Builtin::MoveReturnOwner => OBJECT | STRING | LIST,
                    crate::sema::Builtin::OwnedToList
                    | crate::sema::Builtin::ArgumentPack
                    | crate::sema::Builtin::ArgumentPackTail => LIST,
                    crate::sema::Builtin::ApplyConstructor => NIL,
                    crate::sema::Builtin::CallQualified | crate::sema::Builtin::CallDelegated => {
                        UNKNOWN
                    }
                    crate::sema::Builtin::ApplyQualified | crate::sema::Builtin::ApplyMethod => {
                        UNKNOWN
                    }
                    crate::sema::Builtin::VectorIndexOf | crate::sema::Builtin::RexMatch => {
                        INTEGER | NIL
                    }
                    crate::sema::Builtin::VectorSort => OBJECT,
                    crate::sema::Builtin::VectorSortBegin => NIL,
                    crate::sema::Builtin::VectorSortNext => LOGICAL,
                    crate::sema::Builtin::VectorSortElement => UNKNOWN,
                    crate::sema::Builtin::OwnedToString => STRING,
                    crate::sema::Builtin::BeginScope => INTEGER,
                    crate::sema::Builtin::EndScope
                    | crate::sema::Builtin::UnwindScope
                    | crate::sema::Builtin::RestorePending => NIL,
                    crate::sema::Builtin::PendingValue
                    | crate::sema::Builtin::PendingCode
                    | crate::sema::Builtin::ClaimException
                    | crate::sema::Builtin::DetachException
                    | crate::sema::Builtin::PendingOwner
                    | crate::sema::Builtin::PendingSite => UNKNOWN,
                    crate::sema::Builtin::Apply
                    | crate::sema::Builtin::Invoke
                    | crate::sema::Builtin::IterationValue => UNKNOWN,
                    crate::sema::Builtin::BeginIteration | crate::sema::Builtin::EndIteration => {
                        NIL
                    }
                    crate::sema::Builtin::AdvanceIteration
                    | crate::sema::Builtin::PropDefined
                    | crate::sema::Builtin::OfKind => LOGICAL,
                    crate::sema::Builtin::LookupBuckets
                    | crate::sema::Builtin::LookupLength
                    | crate::sema::Builtin::OwnedLength
                    | crate::sema::Builtin::ToInteger
                    | crate::sema::Builtin::DataType
                    | crate::sema::Builtin::GetTime
                    | crate::sema::Builtin::Length => INTEGER,
                    crate::sema::Builtin::InputKey
                    | crate::sema::Builtin::ToLower
                    | crate::sema::Builtin::ToUpper
                    | crate::sema::Builtin::Htmlify
                    | crate::sema::Builtin::FindReplace => STRING,
                    // inputLine reports the end of input as nil.
                    crate::sema::Builtin::InputLine => STRING | NIL,
                    crate::sema::Builtin::FlushOutput => NIL,
                    crate::sema::Builtin::OwnText | crate::sema::Builtin::ReplaceOwner => STRING,
                    crate::sema::Builtin::RelationSet
                    | crate::sema::Builtin::RelationUnset
                    | crate::sema::Builtin::Savepoint
                    | crate::sema::Builtin::BeginTurn
                    | crate::sema::Builtin::UseLifetimes => NIL,
                    crate::sema::Builtin::RelationContains => LOGICAL,
                    // true when a cycle was put back, nil when none was.
                    crate::sema::Builtin::Undo => LOGICAL | NIL,
                    crate::sema::Builtin::UndoDepth => INTEGER,
                    crate::sema::Builtin::Despawn
                    | crate::sema::Builtin::Event
                    | crate::sema::Builtin::EventSubject
                    | crate::sema::Builtin::EventValue => NIL,
                    crate::sema::Builtin::SnapshotToken => INTEGER | LOGICAL | NIL,
                    crate::sema::Builtin::HostAction => LOGICAL | NIL,
                    crate::sema::Builtin::EngineQuery => INTEGER | NIL,
                    crate::sema::Builtin::EndTurn | crate::sema::Builtin::TurnJournalLength => {
                        INTEGER
                    }
                    crate::sema::Builtin::RelationGet => OBJECT | NIL,
                    crate::sema::Builtin::RelationOutermost => OBJECT,
                    crate::sema::Builtin::RelationAll
                    | crate::sema::Builtin::RelationDescendants
                    | crate::sema::Builtin::RelationAncestors
                    // the words that name an entity, as a list of text.
                    | crate::sema::Builtin::VocabularyWords => LIST,
                    crate::sema::Builtin::ValueToText => STRING,
                    crate::sema::Builtin::VocabularyNames => LOGICAL,
                    crate::sema::Builtin::StartsWith | crate::sema::Builtin::EndsWith => LOGICAL,
                    crate::sema::Builtin::FindText => INTEGER | NIL,
                    crate::sema::Builtin::RexSearch | crate::sema::Builtin::RexGroup => LIST | NIL,
                    crate::sema::Builtin::List
                    | crate::sema::Builtin::DictionaryFind
                    | crate::sema::Builtin::GrammarParse
                    | crate::sema::Builtin::GrammarBegin
                    | crate::sema::Builtin::GrammarFinish
                    | crate::sema::Builtin::ConstantList => LIST,
                    crate::sema::Builtin::MoveLocalToCollection
                    | crate::sema::Builtin::MoveLocalToField
                    | crate::sema::Builtin::ReserveInCollection
                    | crate::sema::Builtin::ReserveConstruction
                    | crate::sema::Builtin::DictionaryAdd
                    | crate::sema::Builtin::DictionaryRemove => NIL,
                    crate::sema::Builtin::LookupContains
                    | crate::sema::Builtin::RemoveOwned
                    | crate::sema::Builtin::MoveOwnedTo
                    | crate::sema::Builtin::MoveFieldOwnerTo
                    | crate::sema::Builtin::DictionaryDefined => LOGICAL,
                    crate::sema::Builtin::LookupDefault
                    | crate::sema::Builtin::LookupSetDefault
                    | crate::sema::Builtin::LookupSet
                    | crate::sema::Builtin::IndexSet
                    | crate::sema::Builtin::ListIndex => UNKNOWN,
                    crate::sema::Builtin::Add => INTEGER | STRING | LIST,
                    _ => STRING,
                }
            }
            Operation::CallMethod {
                receiver,
                arguments,
                ..
            } => {
                if values[receiver.0] == 0 || arguments.iter().any(|v| values[v.0] == 0) {
                    return (changed, false);
                }
                UNKNOWN
            }
            Operation::Call {
                function,
                arguments,
            } => {
                if arguments.iter().any(|v| values[v.0] == 0) {
                    return (changed, false);
                }
                if integer_callees.get(*function) == Some(&true)
                    && arguments.iter().all(|v| values[v.0] == INTEGER)
                {
                    INTEGER
                } else {
                    UNKNOWN
                }
            }
        };
        if let Some(value) = i.result {
            let old = values[value.0];
            values[value.0] |= types;
            changed |= old != values[value.0];
        }
    }
    (changed, true)
}
pub(crate) fn check_function(
    ast: &Ast,
    f: &Function,
    integer_parameters: bool,
    integer_callees: &[bool],
) -> Result<FunctionFacts, Diagnostic> {
    let mut incoming: Vec<Option<Vec<Fact>>> = filled(f.blocks.len(), None)?;
    let mut entry = filled(f.slots.len(), Fact::EMPTY)?;
    for (slot, description) in entry.iter_mut().zip(&f.slots) {
        if description.parameter {
            *slot = if integer_parameters {
                Fact {
                    initialized: true,
                    types: INTEGER,
                }
            } else {
                Fact::PARAMETER
            };
        }
    }
    incoming[0] = Some(entry);
    let mut values = filled(f.values, 0u16)?;
    loop {
        let mut changed = false;
        for index in 0..f.blocks.len() {
            let Some(input) = &incoming[index] else {
                continue;
            };
            let mut slots = copy(input)?;
            let (value_change, ready) =
                transfer(f, index, &mut slots, &mut values, integer_callees);
            changed |= value_change;
            if !ready {
                continue;
            }
            for target in targets(&f.blocks[index].terminator, &values)
                .into_iter()
                .flatten()
            {
                let mut edge_slots = copy(&slots)?;
                if let Some(edge) = f.blocks[index].terminator.exception()
                    && edge.target.0 == target
                {
                    edge_slots[edge.slot.0] = Fact {
                        initialized: true,
                        types: OBJECT,
                    };
                }
                match &mut incoming[target] {
                    Some(previous) => {
                        for (a, b) in previous.iter_mut().zip(&edge_slots) {
                            changed |= a.merge(*b);
                        }
                    }
                    entry @ None => {
                        *entry = Some(edge_slots);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut runtime_checks = Vec::new();
    for (index, block) in f.blocks.iter().enumerate() {
        let Some(input) = &incoming[index] else {
            continue;
        };
        let mut slots = copy(input)?;
        for i in &block.instructions {
            let require = |value: ir::Value, allowed: u16| -> Result<(), Diagnostic> {
                if values[value.0] & allowed == 0 {
                    Err(error(
                        ast,
                        i.site,
                        "flow-type",
                        "scalar operation has no valid operand type",
                    ))
                } else {
                    Ok(())
                }
            };
            if let Some(value) = i.operation.computed_property() {
                require(value, PROPERTY)?;
            }
            match i.operation {
                Operation::Load(slot) if !slots[slot.0].initialized => {
                    return Err(error(
                        ast,
                        i.site,
                        "flow-uninitialized",
                        "local may be read before initialization",
                    ));
                }
                Operation::Store(slot, value) => {
                    slots[slot.0] = Fact {
                        initialized: true,
                        types: values[value.0],
                    }
                }
                Operation::Reset(slot) => slots[slot.0] = Fact::EMPTY,
                Operation::Unary(op, value) => {
                    require(value, if op == Unary::Not { LOGICAL } else { INTEGER })?
                }
                Operation::Binary(op, left, right)
                    if !matches!(op, Binary::Equal | Binary::NotEqual) =>
                {
                    require(left, INTEGER)?;
                    require(right, INTEGER)?;
                }
                Operation::Builtin {
                    kind: crate::sema::Builtin::ListIndex,
                    ref arguments,
                } => {
                    require(arguments[0], LIST | OBJECT)?;
                    if values[arguments[0].0] & OBJECT == 0 {
                        require(arguments[1], INTEGER)?;
                    }
                }
                Operation::Builtin {
                    kind: crate::sema::Builtin::Length,
                    ref arguments,
                } => require(arguments[0], STRING | LIST | OBJECT)?,
                Operation::LogicalGuard(value) => require(value, LOGICAL)?,
                Operation::BeginStatic(value, _)
                | Operation::GetProperty(value, _)
                | Operation::SetProperty(value, _, _)
                | Operation::CallMethod {
                    receiver: value, ..
                } => require(value, OBJECT)?,
                _ => {}
            }
            if i.failure == Failure::Propagate {
                push(&mut runtime_checks, i.site)?;
            }
        }
        if matches!(block.terminator, Terminator::Fallthrough) {
            return Err(error(
                ast,
                block.site,
                "flow-return",
                "reachable function end requires an explicit return",
            ));
        }
    }
    let mut reachable = Vec::new();
    for input in incoming {
        push(&mut reachable, input.is_some())?;
    }
    Ok(FunctionFacts {
        reachable,
        value_types: values,
        runtime_checks,
    })
}
/// Accept only source ASTs; the IR is built here, never trusted external input.
pub fn check(ast: &Ast) -> Result<Checked, Diagnostic> {
    check_with(ast, ast.lifetimes)
}

/// Check the program a given `+` lowering produces. The emitter that will build
/// the narrow profile checks the same IR it emits.
pub fn check_with(ast: &Ast, polymorphic_plus: bool) -> Result<Checked, Diagnostic> {
    let program = ir::lower_with(ast, polymorphic_plus)?;
    crate::ir_verify::verify(ast, &program)?;
    let mut facts = Vec::new();
    for function in &program.functions {
        push(&mut facts, check_function(ast, function, false, &[])?)?;
    }
    Ok(Checked { program, facts })
}
