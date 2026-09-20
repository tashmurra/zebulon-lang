//! One scalar control-flow representation for flow checking and native lowering.
use crate::{
    Diagnostic,
    parser::{Ast, Id, Syntax},
    sema::{self, Analysis, Binding, Scalar},
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockId(pub usize);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Value(pub usize);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Slot(pub usize);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    None,
    Propagate,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchMode {
    Logical,
    IsNil,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unary {
    Positive,
    Negative,
    Not,
    BitNot,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    And,
    Or,
    Xor,
    ShiftLeft,
    ShiftRight,
    ShiftUnsigned,
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
}
#[derive(Clone, Copy, Debug)]
pub enum PropertyKey {
    Named(u32),
    Computed(Value),
}
impl From<u32> for PropertyKey {
    fn from(value: u32) -> Self {
        Self::Named(value)
    }
}
#[derive(Debug)]
pub enum Operation {
    Constant(Scalar),
    NamedObject(u32),
    PropertyValue(u32),
    FunctionValue(u32),
    Enumerator(u32),
    StringLiteral(u32),
    EmitLiteral(u32),
    EmitValue(Value),
    GetProperty(Value, PropertyKey),
    BeginStatic(Value, u32),
    EndStatic,
    Builtin {
        kind: sema::Builtin,
        arguments: Vec<Value>,
    },
    SetProperty(Value, PropertyKey, Value),
    Load(Slot),
    Store(Slot, Value),
    Reset(Slot),
    Unary(Unary, Value),
    Binary(Binary, Value, Value),
    LogicalGuard(Value),
    CallMethod {
        receiver: Value,
        property: PropertyKey,
        after: Option<u32>,
        arguments: Vec<Value>,
    },
    Call {
        function: usize,
        arguments: Vec<Value>,
    },
}
impl Operation {
    pub fn computed_property(&self) -> Option<Value> {
        match self {
            Self::GetProperty(_, PropertyKey::Computed(value))
            | Self::SetProperty(_, PropertyKey::Computed(value), _)
            | Self::CallMethod {
                property: PropertyKey::Computed(value),
                ..
            } => Some(*value),
            _ => None,
        }
    }
}
#[derive(Debug)]
pub struct Instruction {
    pub result: Option<Value>,
    pub operation: Operation,
    pub site: Id,
    pub failure: Failure,
}
#[derive(Clone, Copy, Debug)]
pub struct ExceptionEdge {
    pub target: BlockId,
    pub slot: Slot,
}

#[derive(Debug)]
pub enum Terminator {
    Invoke {
        normal: BlockId,
        exception: ExceptionEdge,
    },
    Resume(Option<ExceptionEdge>),
    Jump(BlockId),
    Branch {
        condition: Value,
        mode: BranchMode,
        yes: BlockId,
        no: BlockId,
    },
    Return(Value),
    Throw(Value, Id, Option<ExceptionEdge>),
    Fallthrough,
}
impl Terminator {
    pub fn exception(&self) -> Option<ExceptionEdge> {
        match *self {
            Self::Invoke { exception, .. } => Some(exception),
            Self::Throw(_, _, edge) | Self::Resume(edge) => edge,
            _ => None,
        }
    }
}
#[derive(Debug)]
pub struct Block {
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
    pub site: Id,
}
#[derive(Debug)]
pub struct LocalSlot {
    pub declaration: Option<(Id, usize)>,
    pub parameter: bool,
}
#[derive(Debug)]
pub struct Function {
    pub source: Id,
    pub blocks: Vec<Block>,
    pub slots: Vec<LocalSlot>,
    pub values: usize,
}
#[derive(Debug)]
pub struct Program {
    pub functions: Vec<Function>,
}

fn push<T>(values: &mut Vec<T>, value: T) -> Result<(), Diagnostic> {
    values.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    values.push(value);
    Ok(())
}
fn unary(op: &str) -> Option<Unary> {
    Some(match op {
        "+" => Unary::Positive,
        "-" => Unary::Negative,
        "!" => Unary::Not,
        "~" => Unary::BitNot,
        _ => return None,
    })
}
fn binary(op: &str) -> Option<Binary> {
    Some(match op {
        "+" => Binary::Add,
        "-" => Binary::Subtract,
        "*" => Binary::Multiply,
        "/" => Binary::Divide,
        "%" => Binary::Remainder,
        "&" => Binary::And,
        "|" => Binary::Or,
        "^" => Binary::Xor,
        "<<" => Binary::ShiftLeft,
        ">>" => Binary::ShiftRight,
        ">>>" => Binary::ShiftUnsigned,
        "==" => Binary::Equal,
        "!=" => Binary::NotEqual,
        "<" => Binary::Less,
        ">" => Binary::Greater,
        "<=" => Binary::LessEqual,
        ">=" => Binary::GreaterEqual,
        _ => return None,
    })
}
fn assigned(op: &str) -> bool {
    matches!(
        op,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" | ">>>="
    )
}

struct Builder<'a> {
    ast: &'a Ast,
    analysis: &'a Analysis,
    /// `+` nodes that initialize an owned local, so they may build text.
    concatenations: &'a [bool],
    function: Function,
    current: BlockId,
    locals: HashMap<usize, Slot>,
    declarations: HashMap<(usize, usize), Slot>,
    iterations: usize,
    handlers: Vec<Handler>,
    finalizers: Vec<Finalizer>,
    active_finalizers: Vec<usize>,
    labels: Vec<(Id, Option<Loop>)>,
    /// Labels of this function body by name.
    goto_labels: HashMap<&'a str, Id>,
    /// Per label node: its IR block and the (handler, finally, iteration) depths of
    /// its enclosing statement sequence, recorded when that sequence is entered.
    goto_targets: HashMap<usize, GotoTarget>,
}
type GotoTarget = (BlockId, Option<(usize, usize, usize)>);
#[derive(Clone, Copy)]
struct Handler {
    iterations: usize,
    edge: ExceptionEdge,
    checkpoint: Value,
}

#[derive(Clone, Copy)]
enum Action {
    Return,
    Jump {
        target: BlockId,
        handler_depth: usize,
        finally_depth: usize,
        iteration_depth: usize,
    },
}
struct Finalizer {
    entry: BlockId,
    end: BlockId,
    handler: Handler,
    reason: Slot,
    value: Slot,
    packet: [Slot; 4],
    actions: Vec<Action>,
    normal: bool,
}

#[derive(Clone, Copy)]
enum Receiver {
    Expression(Id),
    SelfSlot(usize, Id),
    /// A captured `self`, by capture index.
    Captured(usize, Id),
}
#[derive(Clone, Copy)]
enum CallTarget {
    TemporaryConstruction(Id, sema::Builtin, bool),
    Qualified(usize, u32, u32, bool),
    /// `delegated target`: self slot and property; the target object is the
    /// first evaluated argument.
    Delegated(usize, u32),
    ExpandedProperty(u32),
    /// `obj.(expr)(args...)`: receiver, then the property value, then the list.
    ExpandedIndirect,
    Construction(Id, Id, Id),
    Direct(usize),
    Builtin(sema::Builtin),
    Property(Receiver, u32, Option<u32>),
    Indirect(Receiver, Id),
}
enum IndirectAction {
    Read,
    Write(Value),
    Compound(Binary, Id),
    Call(Vec<Value>),
}
enum ExpressionTask {
    IndexRhs(Id, Id, Id),
    IndexSetReceiver(Id, Id, Value),
    IndexSetKey(Id, Value, Value),
    LookupNext(Id, Vec<(Option<Value>, Value)>),
    LookupKey(Id, Vec<(Option<Value>, Value)>, Value),
    LookupValue(Id, Vec<(Option<Value>, Value)>, usize),
    MembershipLeft(Id),
    MembershipItem(Id, Value, usize, Slot, BlockId),
    MoveOwner(Id),
    IndexReceiver(Id, Id),
    IndexValue(Id, Value),
    IndirectRhs(Id, Id, Id),
    IndirectReceiver(Id, Id, IndirectAction),
    IndirectKey(Id, Value, IndirectAction),
    InterpolationNext(Id, usize),
    Emit(Id),
    Receiver(Receiver),
    MethodReceiver(Id, u32, Option<u32>, Vec<Value>),
    Property(Id, u32),
    PropertyRhs(Id, Receiver, u32),
    PropertyWrite(Id, u32, Value),
    PropertyCompound(Id, u32, Binary, Id),
    PropertyFinish(Id, PropertyKey, Value, Binary, Value),
    PropertyIncrement(Id, u32, Binary, bool),
    Eval(Id),
    Unary(Id, Unary),
    Left(Id, &'static str, Id),
    Right(Id, &'static str, Value),
    Assign(Id, Slot, Option<(Binary, Value)>, bool),
    OwnText(Id),
    CallNext(Id, CallTarget, Vec<Id>, Vec<Value>),
    CallArg(Id, CallTarget, Vec<Id>, Vec<Value>),
    Choose(Id, Id, Id),
    Then(Id, Id, BlockId, BlockId, Slot),
    Else(Id, BlockId, Slot),
    Short(Id, &'static str, Id),
    ShortEnd(Id, BlockId, Slot, bool),
}
#[derive(Clone, Copy)]
struct Loop {
    iterations: usize,
    test: Option<BlockId>,
    exit: BlockId,
    break_depth: usize,
    continue_depth: usize,
    break_finally: usize,
    continue_finally: usize,
}
enum StatementTask {
    EndLabel(BlockId),
    AfterProtected(Id, usize, Option<Loop>),
    AfterFinally(Id, usize),
    AfterTry(Id, Handler, BlockId, Option<Loop>),
    CatchStart(BlockId, Id, Value),
    AfterCatch(Id, BlockId),
    EndIteration(Id, BlockId, BlockId),
    ForStart(Id, Option<Id>, Option<Id>, Id),
    ForEnd(Id, Option<Id>, BlockId, BlockId, BlockId),
    Begin(BlockId),
    Visit(Id, Option<Loop>),
    AfterThen(Id, Option<Id>, Option<Loop>, BlockId, BlockId),
    AfterElse(BlockId),
    AfterWhile(BlockId, BlockId),
    AfterDo(Id, Id, BlockId, BlockId, BlockId),
}
impl Builder<'_> {
    fn loop_context(&mut self, id: Id, context: Loop) -> Loop {
        for (label, target) in &mut self.labels {
            if matches!(self.ast.nodes[label.0].syntax, Syntax::Label(_, body) if body == id) {
                *target = Some(context);
            }
        }
        context
    }
    /// Whether a name resolves to a local declared with `local owned`.
    fn owned_local(&self, id: Id) -> bool {
        matches!(self.analysis.bindings[id.0], Some(sema::Binding::Local(local))
        if matches!(
            self.ast.nodes[self.analysis.locals[local].declaration.0].syntax,
            Syntax::OwnedLocal(_)
        ))
    }
    fn error(&self, id: Id, message: &'static str) -> Diagnostic {
        Diagnostic::new("ir-lowering", message, self.ast.nodes[id.0].start)
    }
    fn block(&mut self, site: Id) -> Result<BlockId, Diagnostic> {
        let id = BlockId(self.function.blocks.len());
        push(
            &mut self.function.blocks,
            Block {
                instructions: Vec::new(),
                terminator: Terminator::Fallthrough,
                site,
            },
        )?;
        Ok(id)
    }
    fn terminate(&mut self, term: Terminator) {
        self.function.blocks[self.current.0].terminator = term;
    }
    fn effect(&mut self, op: Operation, site: Id) -> Result<(), Diagnostic> {
        self.instruction(op, site, false)?;
        Ok(())
    }
    fn value(&mut self, op: Operation, site: Id) -> Result<Value, Diagnostic> {
        self.instruction(op, site, true)?
            .ok_or(self.error(site, "missing IR value"))
    }
    fn instruction(
        &mut self,
        operation: Operation,
        site: Id,
        produces: bool,
    ) -> Result<Option<Value>, Diagnostic> {
        let failure = if matches!(
            operation,
            Operation::Unary(..)
                | Operation::Binary(..)
                | Operation::LogicalGuard(..)
                | Operation::Call { .. }
                | Operation::CallMethod { .. }
                | Operation::NamedObject(_)
                | Operation::StringLiteral(_)
                | Operation::EmitLiteral(_)
                | Operation::EmitValue(_)
                | Operation::GetProperty(..)
                | Operation::SetProperty(..)
                | Operation::BeginStatic(..)
                | Operation::EndStatic
                | Operation::Builtin { .. }
        ) {
            Failure::Propagate
        } else {
            Failure::None
        };
        let result = if produces {
            let id = Value(self.function.values);
            self.function.values = self
                .function
                .values
                .checked_add(1)
                .ok_or(self.error(site, "value identity exhausted"))?;
            Some(id)
        } else {
            None
        };
        let invokes = matches!(
            operation,
            Operation::Call { .. }
                | Operation::CallMethod { .. }
                | Operation::GetProperty(..)
                | Operation::Builtin {
                    kind: sema::Builtin::Invoke
                        | sema::Builtin::Apply
                        | sema::Builtin::ApplyMethod
                        | sema::Builtin::ApplyConstructor
                        | sema::Builtin::CallQualified
                        | sema::Builtin::CallDelegated
                        | sema::Builtin::ApplyQualified,
                    ..
                }
        );
        push(
            &mut self.function.blocks[self.current.0].instructions,
            Instruction {
                result,
                operation,
                site,
                failure,
            },
        )?;
        if let Some(handler) = self.handlers.last().copied()
            && invokes
        {
            let normal = self.block(site)?;
            self.terminate(Terminator::Invoke {
                normal,
                exception: handler.edge,
            });
            self.current = normal;
        }
        Ok(result)
    }
    /// Read capture `index` from the environment parameter of a capturing
    /// callback, which sema declares as its first parameter.
    fn captured(&mut self, index: usize, site: Id) -> Result<Value, Diagnostic> {
        let slot = *self
            .declarations
            .get(&(self.function.source.0, 0))
            .ok_or(self.error(site, "missing capture environment"))?;
        let environment = self.value(Operation::Load(slot), site)?;
        let index = i32::try_from(index).map_err(|_| self.error(site, "capture index"))?;
        let index = self.value(Operation::Constant(Scalar::Integer(index)), site)?;
        self.value(
            Operation::Builtin {
                kind: sema::Builtin::ClosureCapture,
                arguments: vec![environment, index],
            },
            site,
        )
    }

    /// True when this `+` initializes an owned local, so it may build new text
    /// belonging to the scope rather than only adding numbers.
    fn owned_concat(&self, id: Id) -> bool {
        self.concatenations.get(id.0).copied().unwrap_or(false)
    }

    fn temp(&mut self) -> Result<Slot, Diagnostic> {
        let slot = Slot(self.function.slots.len());
        push(
            &mut self.function.slots,
            LocalSlot {
                declaration: None,
                parameter: false,
            },
        )?;
        Ok(slot)
    }
    fn local(&self, mut id: Id) -> Result<Slot, Diagnostic> {
        while let Syntax::Group(child) = self.ast.nodes[id.0].syntax {
            id = child;
        }
        match self.analysis.bindings[id.0] {
            Some(Binding::Local(slot)) => self
                .locals
                .get(&slot)
                .copied()
                .ok_or(self.error(id, "local belongs to another function")),
            _ => Err(self.error(id, "expected a bound local")),
        }
    }
    fn take(&self, result: &mut Option<Value>, site: Id) -> Result<Value, Diagnostic> {
        result
            .take()
            .ok_or(self.error(site, "missing expression result"))
    }
    fn property_destination(&self, mut id: Id) -> Option<(Receiver, u32)> {
        while let Syntax::Group(inner) = self.ast.nodes[id.0].syntax {
            id = inner;
        }
        if let Syntax::Property(receiver, _) = self.ast.nodes[id.0].syntax {
            self.analysis.properties[id.0]
                .map(|property| (Receiver::Expression(receiver), property))
        } else if let Some(Binding::SelfProperty(slot, property)) = self.analysis.bindings[id.0] {
            Some((Receiver::SelfSlot(slot, id), property))
        } else if let Some(Binding::CapturedSelfProperty(index, property)) =
            self.analysis.bindings[id.0]
        {
            Some((Receiver::Captured(index, id), property))
        } else {
            None
        }
    }
    fn expression(&mut self, root: Id) -> Result<Value, Diagnostic> {
        let mut tasks = Vec::new();
        push(&mut tasks, ExpressionTask::Eval(root))?;
        let mut result = None;
        while let Some(task) = tasks.pop() {
            match task {
                ExpressionTask::IndexRhs(id, receiver, key) => {
                    let rhs = self.take(&mut result, id)?;
                    push(&mut tasks, ExpressionTask::IndexSetReceiver(id, key, rhs))?;
                    push(&mut tasks, ExpressionTask::Eval(receiver))?;
                }
                ExpressionTask::IndexSetReceiver(id, key, rhs) => {
                    let receiver = self.take(&mut result, id)?;
                    push(&mut tasks, ExpressionTask::IndexSetKey(id, receiver, rhs))?;
                    push(&mut tasks, ExpressionTask::Eval(key))?;
                }
                ExpressionTask::IndexSetKey(id, receiver, rhs) => {
                    let key = self.take(&mut result, id)?;
                    let mut arguments = Vec::new();
                    for value in [receiver, key, rhs] {
                        push(&mut arguments, value)?;
                    }
                    result = Some(self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::IndexSet,
                            arguments,
                        },
                        id,
                    )?);
                }
                ExpressionTask::LookupNext(id, entries) => {
                    let Syntax::Lookup(source) = &self.ast.nodes[id.0].syntax else {
                        unreachable!()
                    };
                    if entries.len() < source.len() {
                        let index = source.len() - entries.len() - 1;
                        push(&mut tasks, ExpressionTask::LookupValue(id, entries, index))?;
                        push(&mut tasks, ExpressionTask::Eval(source[index].1))?;
                    } else {
                        let table = self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::NewLocalLookup,
                                arguments: Vec::new(),
                            },
                            id,
                        )?;
                        for (key, value) in entries.into_iter().rev() {
                            let mut arguments = Vec::new();
                            push(&mut arguments, table)?;
                            if let Some(key) = key {
                                push(&mut arguments, key)?;
                            }
                            push(&mut arguments, value)?;
                            self.value(
                                Operation::Builtin {
                                    kind: if key.is_some() {
                                        sema::Builtin::LookupSet
                                    } else {
                                        sema::Builtin::LookupSetDefault
                                    },
                                    arguments,
                                },
                                id,
                            )?;
                        }
                        result = Some(table);
                    }
                }
                ExpressionTask::LookupKey(id, mut entries, value) => {
                    let key = self.take(&mut result, id)?;
                    push(&mut entries, (Some(key), value))?;
                    push(&mut tasks, ExpressionTask::LookupNext(id, entries))?;
                }
                ExpressionTask::LookupValue(id, mut entries, index) => {
                    let value = self.take(&mut result, id)?;
                    let Syntax::Lookup(source) = &self.ast.nodes[id.0].syntax else {
                        unreachable!()
                    };
                    if let Some(key) = source[index].0 {
                        push(&mut tasks, ExpressionTask::LookupKey(id, entries, value))?;
                        push(&mut tasks, ExpressionTask::Eval(key))?;
                    } else {
                        push(&mut entries, (None, value))?;
                        push(&mut tasks, ExpressionTask::LookupNext(id, entries))?;
                    }
                }
                ExpressionTask::MembershipLeft(id) => {
                    let left = self.take(&mut result, id)?;
                    let Syntax::Membership { candidates, .. } = &self.ast.nodes[id.0].syntax else {
                        unreachable!()
                    };
                    let first = candidates[0];
                    let slot = self.temp()?;
                    self.effect(Operation::Reset(slot), id)?;
                    let join = self.block(id)?;
                    push(
                        &mut tasks,
                        ExpressionTask::MembershipItem(id, left, 0, slot, join),
                    )?;
                    push(&mut tasks, ExpressionTask::Eval(first))?;
                }
                ExpressionTask::MembershipItem(id, left, index, slot, join) => {
                    let right = self.take(&mut result, id)?;
                    let Syntax::Membership {
                        candidates,
                        negated,
                        ..
                    } = &self.ast.nodes[id.0].syntax
                    else {
                        unreachable!()
                    };
                    let next = candidates.get(index + 1).copied();
                    let negated = *negated;
                    let equal = self.value(Operation::Binary(Binary::Equal, left, right), id)?;
                    let answer = if negated {
                        self.value(Operation::Unary(Unary::Not, equal), id)?
                    } else {
                        equal
                    };
                    self.effect(Operation::Store(slot, answer), id)?;
                    if let Some(next) = next {
                        self.effect(Operation::LogicalGuard(equal), id)?;
                        let next_block = self.block(id)?;
                        self.terminate(Terminator::Branch {
                            condition: equal,
                            mode: BranchMode::Logical,
                            yes: join,
                            no: next_block,
                        });
                        self.current = next_block;
                        push(
                            &mut tasks,
                            ExpressionTask::MembershipItem(id, left, index + 1, slot, join),
                        )?;
                        push(&mut tasks, ExpressionTask::Eval(next))?;
                    } else {
                        self.terminate(Terminator::Jump(join));
                        self.current = join;
                        result = Some(self.value(Operation::Load(slot), id)?);
                    }
                }
                ExpressionTask::MoveOwner(id) => {
                    let value = result
                        .take()
                        .ok_or(self.error(id, "missing move operand"))?;
                    let mut arguments = Vec::new();
                    push(&mut arguments, value)?;
                    result = Some(self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::MoveReturnOwner,
                            arguments,
                        },
                        id,
                    )?);
                }
                ExpressionTask::Receiver(receiver) => match receiver {
                    Receiver::Expression(id) => push(&mut tasks, ExpressionTask::Eval(id))?,
                    Receiver::SelfSlot(slot, id) => {
                        let slot = *self
                            .locals
                            .get(&slot)
                            .ok_or(self.error(id, "self belongs to another function"))?;
                        result = Some(self.value(Operation::Load(slot), id)?);
                    }
                    Receiver::Captured(index, id) => {
                        result = Some(self.captured(index, id)?);
                    }
                },
                ExpressionTask::InterpolationNext(id, index) => {
                    let Syntax::Interpolation { parts, .. } = &self.ast.nodes[id.0].syntax else {
                        unreachable!()
                    };
                    if let Some(part) = parts.get(index).copied() {
                        push(&mut tasks, ExpressionTask::InterpolationNext(id, index + 1))?;
                        if index % 2 == 0 {
                            let literal = self.analysis.strings[part.0]
                                .ok_or(self.error(part, "missing interpolation text"))?;
                            self.effect(Operation::EmitLiteral(literal), part)?;
                        } else {
                            push(&mut tasks, ExpressionTask::Emit(part))?;
                            push(&mut tasks, ExpressionTask::Eval(part))?;
                        }
                    } else {
                        result = Some(self.value(Operation::Constant(Scalar::Nil), id)?);
                    }
                }
                ExpressionTask::Emit(id) => {
                    let value = self.take(&mut result, id)?;
                    self.effect(Operation::EmitValue(value), id)?;
                }
                ExpressionTask::IndexReceiver(id, index) => {
                    let receiver = self.take(&mut result, id)?;
                    push(&mut tasks, ExpressionTask::IndexValue(id, receiver))?;
                    push(&mut tasks, ExpressionTask::Eval(index))?;
                }
                ExpressionTask::IndexValue(id, receiver) => {
                    let index = self.take(&mut result, id)?;
                    let mut arguments = Vec::new();
                    push(&mut arguments, receiver)?;
                    push(&mut arguments, index)?;
                    result = Some(self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::ListIndex,
                            arguments,
                        },
                        id,
                    )?);
                }
                ExpressionTask::IndirectRhs(id, receiver, key) => {
                    let rhs = self.take(&mut result, id)?;
                    push(
                        &mut tasks,
                        ExpressionTask::IndirectReceiver(id, key, IndirectAction::Write(rhs)),
                    )?;
                    push(&mut tasks, ExpressionTask::Eval(receiver))?;
                }
                ExpressionTask::IndirectReceiver(id, key, action) => {
                    let receiver = self.take(&mut result, id)?;
                    push(
                        &mut tasks,
                        ExpressionTask::IndirectKey(id, receiver, action),
                    )?;
                    push(&mut tasks, ExpressionTask::Eval(key))?;
                }
                ExpressionTask::IndirectKey(id, receiver, action) => {
                    let property = PropertyKey::Computed(self.take(&mut result, id)?);
                    match action {
                        IndirectAction::Read => {
                            result =
                                Some(self.value(Operation::GetProperty(receiver, property), id)?)
                        }
                        IndirectAction::Write(rhs) => {
                            self.effect(Operation::SetProperty(receiver, property, rhs), id)?;
                            result = Some(rhs);
                        }
                        IndirectAction::Compound(op, rhs) => {
                            let old = self.value(Operation::GetProperty(receiver, property), id)?;
                            push(
                                &mut tasks,
                                ExpressionTask::PropertyFinish(id, property, receiver, op, old),
                            )?;
                            push(&mut tasks, ExpressionTask::Eval(rhs))?;
                        }
                        IndirectAction::Call(arguments) => {
                            result = Some(self.value(
                                Operation::CallMethod {
                                    receiver,
                                    property,
                                    after: None,
                                    arguments,
                                },
                                id,
                            )?)
                        }
                    }
                }
                ExpressionTask::MethodReceiver(id, property, after, arguments) => {
                    let receiver = self.take(&mut result, id)?;
                    result = Some(self.value(
                        Operation::CallMethod {
                            receiver,
                            property: property.into(),
                            after,
                            arguments,
                        },
                        id,
                    )?);
                }
                ExpressionTask::PropertyRhs(id, receiver, property) => {
                    let rhs = self.take(&mut result, id)?;
                    push(&mut tasks, ExpressionTask::PropertyWrite(id, property, rhs))?;
                    push(&mut tasks, ExpressionTask::Receiver(receiver))?;
                }
                ExpressionTask::PropertyWrite(id, property, rhs) => {
                    let receiver = self.take(&mut result, id)?;
                    self.effect(Operation::SetProperty(receiver, property.into(), rhs), id)?;
                    result = Some(rhs);
                }
                ExpressionTask::PropertyCompound(id, property, op, rhs) => {
                    let receiver = self.take(&mut result, id)?;
                    let old = self.value(Operation::GetProperty(receiver, property.into()), id)?;
                    push(
                        &mut tasks,
                        ExpressionTask::PropertyFinish(id, property.into(), receiver, op, old),
                    )?;
                    push(&mut tasks, ExpressionTask::Eval(rhs))?;
                }
                ExpressionTask::PropertyFinish(id, property, receiver, op, old) => {
                    let rhs = self.take(&mut result, id)?;
                    let value = self.value(Operation::Binary(op, old, rhs), id)?;
                    self.effect(Operation::SetProperty(receiver, property, value), id)?;
                    result = Some(value);
                }
                ExpressionTask::PropertyIncrement(id, property, op, postfix) => {
                    let receiver = self.take(&mut result, id)?;
                    let old = self.value(Operation::GetProperty(receiver, property.into()), id)?;
                    let one = self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                    let value = self.value(Operation::Binary(op, old, one), id)?;
                    self.effect(Operation::SetProperty(receiver, property.into(), value), id)?;
                    result = Some(if postfix { old } else { value });
                }
                ExpressionTask::Property(id, property) => {
                    let receiver = self.take(&mut result, id)?;
                    result =
                        Some(self.value(Operation::GetProperty(receiver, property.into()), id)?);
                }
                ExpressionTask::Eval(id) => {
                    if let Some(value) = self.analysis.constants[id.0] {
                        result = Some(self.value(Operation::Constant(value), id)?);
                        continue;
                    }
                    if let Some(literal) = self.analysis.strings[id.0] {
                        if matches!(&self.ast.nodes[id.0].syntax,Syntax::String(lit) if lit.emitting)
                        {
                            self.effect(Operation::EmitLiteral(literal), id)?;
                            result = Some(self.value(Operation::Constant(Scalar::Nil), id)?);
                        } else {
                            result = Some(self.value(Operation::StringLiteral(literal), id)?);
                        }
                        continue;
                    }
                    if let Some(Binding::Function(value)) = self.analysis.bindings[id.0] {
                        let body = self.value(
                            Operation::FunctionValue(
                                u32::try_from(value)
                                    .map_err(|_| self.error(id, "function identity exhausted"))?,
                            ),
                            id,
                        )?;
                        let captures = self
                            .analysis
                            .captures
                            .get(value)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        result = Some(if captures.is_empty() {
                            body
                        } else {
                            // build the callback's captured environment,
                            // owned by the enclosing scope like any local allocation.
                            let count = i32::try_from(captures.len())
                                .map_err(|_| self.error(id, "too many captures"))?;
                            let size =
                                self.value(Operation::Constant(Scalar::Integer(count)), id)?;
                            let modes = self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::NewLocalVector,
                                    arguments: vec![size],
                                },
                                id,
                            )?;
                            let values = self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::NewLocalVector,
                                    arguments: vec![size],
                                },
                                id,
                            )?;
                            // mode 2 stores scalars by copy and references by reference
                            let mode = self.value(Operation::Constant(Scalar::Integer(2)), id)?;
                            let captures: Vec<sema::Capture> = captures.to_vec();
                            for capture in captures {
                                let value = match capture {
                                    sema::Capture::Local(local) => {
                                        let slot = *self.locals.get(&local).ok_or(
                                            self.error(id, "captured local is out of scope"),
                                        )?;
                                        self.value(Operation::Load(slot), id)?
                                    }
                                    // a nested callback reads this one's environment
                                    sema::Capture::Outer(index) => self.captured(index, id)?,
                                };
                                for (vector, item) in [(modes, mode), (values, value)] {
                                    self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::VectorAppend,
                                            arguments: vec![vector, item],
                                        },
                                        id,
                                    )?;
                                }
                            }
                            let mut lists = Vec::new();
                            for vector in [modes, values] {
                                push(
                                    &mut lists,
                                    self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::OwnedToList,
                                            arguments: vec![vector],
                                        },
                                        id,
                                    )?,
                                )?;
                            }
                            self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::CaptureClosure,
                                    arguments: vec![body, lists[0], lists[1]],
                                },
                                id,
                            )?
                        });
                        continue;
                    }
                    if let Some(Binding::Captured(index)) = self.analysis.bindings[id.0] {
                        result = Some(self.captured(index, id)?);
                        continue;
                    }
                    if let Some(Binding::Enumerator(value)) = self.analysis.bindings[id.0] {
                        result = Some(self.value(Operation::Enumerator(value), id)?);
                        continue;
                    }
                    if let Some(Binding::Property(property)) = self.analysis.bindings[id.0] {
                        result = Some(self.value(Operation::PropertyValue(property), id)?);
                        continue;
                    }
                    if let Some(Binding::Object(object)) = self.analysis.bindings[id.0] {
                        result = Some(self.value(Operation::NamedObject(object), id)?);
                        continue;
                    }
                    if let Some(Binding::Inherited(slot, property, object)) =
                        self.analysis.bindings[id.0]
                    {
                        push(
                            &mut tasks,
                            ExpressionTask::MethodReceiver(id, property, Some(object), Vec::new()),
                        )?;
                        push(
                            &mut tasks,
                            ExpressionTask::Receiver(Receiver::SelfSlot(slot, id)),
                        )?;
                        continue;
                    }
                    if let Some(Binding::SelfProperty(slot, property)) =
                        self.analysis.bindings[id.0]
                    {
                        push(&mut tasks, ExpressionTask::Property(id, property))?;
                        push(
                            &mut tasks,
                            ExpressionTask::Receiver(Receiver::SelfSlot(slot, id)),
                        )?;
                        continue;
                    }
                    if let Some(Binding::CapturedSelfProperty(index, property)) =
                        self.analysis.bindings[id.0]
                    {
                        push(&mut tasks, ExpressionTask::Property(id, property))?;
                        push(
                            &mut tasks,
                            ExpressionTask::Receiver(Receiver::Captured(index, id)),
                        )?;
                        continue;
                    }
                    match &self.ast.nodes[id.0].syntax {
                        Syntax::Interpolation { .. } => {
                            push(&mut tasks, ExpressionTask::InterpolationNext(id, 0))?
                        }
                        Syntax::Apply(target, arguments) => {
                            let mut qualified_target = *target;
                            while let Syntax::Group(inner) =
                                self.ast.nodes[qualified_target.0].syntax
                            {
                                qualified_target = inner;
                            }
                            if let Syntax::Property(receiver, _) =
                                self.ast.nodes[qualified_target.0].syntax
                            {
                                if matches!(
                                    self.analysis.bindings[qualified_target.0],
                                    Some(Binding::Inherited(..))
                                ) {
                                    return Err(self.error(
                                        id,
                                        "expanded inherited property call is not implemented",
                                    ));
                                }
                                let property = self.analysis.properties[qualified_target.0]
                                    .ok_or(self.error(id, "unbound expanded method"))?;
                                let mut remaining = Vec::new();
                                push(&mut remaining, receiver)?;
                                push(&mut remaining, *arguments)?;
                                push(
                                    &mut tasks,
                                    ExpressionTask::CallNext(
                                        id,
                                        CallTarget::ExpandedProperty(property),
                                        remaining,
                                        Vec::new(),
                                    ),
                                )?;
                                continue;
                            }
                            if let Syntax::IndirectProperty(receiver, key) =
                                self.ast.nodes[qualified_target.0].syntax
                            {
                                // The property is a value here, but the dispatch is
                                // the one `obj.prop(args...)` uses.
                                let mut remaining = Vec::new();
                                for child in [receiver, key, *arguments] {
                                    push(&mut remaining, child)?;
                                }
                                push(
                                    &mut tasks,
                                    ExpressionTask::CallNext(
                                        id,
                                        CallTarget::ExpandedIndirect,
                                        remaining,
                                        Vec::new(),
                                    ),
                                )?;
                                continue;
                            }
                            let function = if matches!(
                                self.ast.nodes[qualified_target.0].syntax,
                                Syntax::QualifiedInherited(_)
                            ) {
                                let Some(Binding::Inherited(slot, property, object)) =
                                    self.analysis.bindings[qualified_target.0]
                                else {
                                    return Err(self.error(id, "unbound inherited target"));
                                };
                                CallTarget::Qualified(slot, property, object, true)
                            } else {
                                CallTarget::Builtin(sema::Builtin::Apply)
                            };
                            let mut remaining = Vec::new();
                            if !matches!(function, CallTarget::Qualified(..)) {
                                push(&mut remaining, *target)?;
                            }
                            push(&mut remaining, *arguments)?;
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(id, function, remaining, Vec::new()),
                            )?;
                        }
                        Syntax::LocalLookupSized(buckets, capacity) => {
                            let mut arguments = Vec::new();
                            for argument in [*buckets, *capacity] {
                                push(&mut arguments, argument)?;
                            }
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Builtin(sema::Builtin::NewLocalLookupSized),
                                    arguments,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::Lookup(_) => {
                            push(&mut tasks, ExpressionTask::LookupNext(id, Vec::new()))?;
                        }
                        Syntax::LocalOwnedCollection => {
                            result = Some(self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::NewLocalOwnedCollection,
                                    arguments: Vec::new(),
                                },
                                id,
                            )?);
                        }
                        Syntax::LocalStringBuffer => {
                            result = Some(self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::NewLocalStringBuffer,
                                    arguments: Vec::new(),
                                },
                                id,
                            )?);
                        }
                        Syntax::StaticLookup { owner, property } => {
                            let mut arguments = Vec::new();
                            for argument in [*owner, *property] {
                                push(&mut arguments, argument)?;
                            }
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Builtin(sema::Builtin::NewStaticLookup),
                                    arguments,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::StaticRexPattern {
                            text,
                            owner,
                            property,
                        } => {
                            let mut arguments = Vec::new();
                            for argument in [*owner, *property, *text] {
                                push(&mut arguments, argument)?;
                            }
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Builtin(sema::Builtin::NewStaticRexPattern),
                                    arguments,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::StaticVector {
                            capacity,
                            owner,
                            property,
                        } => {
                            let mut arguments = Vec::new();
                            for argument in [*owner, *property, *capacity] {
                                push(&mut arguments, argument)?;
                            }
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Builtin(sema::Builtin::NewStaticVector),
                                    arguments,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::LocalVector(capacity) => {
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Builtin(sema::Builtin::NewLocalVector),
                                    vec![*capacity],
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::List(_) if self.analysis.constant_lists[id.0].is_some() => {
                            let index = i32::try_from(
                                self.analysis.constant_lists[id.0].expect("checked constant list"),
                            )
                            .map_err(|_| Diagnostic::resource(self.ast.nodes[id.0].start))?;
                            let index =
                                self.value(Operation::Constant(Scalar::Integer(index)), id)?;
                            result = Some(self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::ConstantList,
                                    arguments: vec![index],
                                },
                                id,
                            )?);
                        }
                        Syntax::List(items)
                        | Syntax::ArgumentPack(items)
                        | Syntax::ArgumentPackTail(items) => {
                            let mut remaining = Vec::new();
                            remaining
                                .try_reserve(items.len())
                                .map_err(|_| Diagnostic::resource(self.ast.nodes[id.0].start))?;
                            remaining.extend_from_slice(items);
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Builtin(match self.ast.nodes[id.0].syntax {
                                        Syntax::ArgumentPack(_) => sema::Builtin::ArgumentPack,
                                        Syntax::ArgumentPackTail(_) => {
                                            sema::Builtin::ArgumentPackTail
                                        }
                                        _ => sema::Builtin::List,
                                    }),
                                    remaining,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::Index(receiver, index) => {
                            push(&mut tasks, ExpressionTask::IndexReceiver(id, *index))?;
                            push(&mut tasks, ExpressionTask::Eval(*receiver))?;
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
                            let mut remaining = Vec::new();
                            remaining
                                .try_reserve(arguments.len())
                                .map_err(|_| Diagnostic::resource(self.ast.nodes[id.0].start))?;
                            remaining.extend_from_slice(arguments);
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::TemporaryConstruction(
                                        *prototype,
                                        match self.ast.nodes[id.0].syntax {
                                            Syntax::ExceptionConstruction { .. } => {
                                                sema::Builtin::FinishException
                                            }
                                            Syntax::LocalConstruction { .. } => {
                                                sema::Builtin::FinishLocalConstruction
                                            }
                                            _ => sema::Builtin::FinishPublishedConstruction,
                                        },
                                        arguments.first().is_some_and(|argument| {
                                            matches!(
                                                self.ast.nodes[argument.0].syntax,
                                                Syntax::ExpandedArgument(_)
                                            )
                                        }),
                                    ),
                                    remaining,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::Construction {
                            prototype,
                            arguments,
                            owner,
                            property,
                        } => {
                            let mut remaining = Vec::new();
                            remaining
                                .try_reserve(arguments.len())
                                .map_err(|_| Diagnostic::resource(self.ast.nodes[id.0].start))?;
                            remaining.extend_from_slice(arguments);
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Construction(*prototype, *owner, *property),
                                    remaining,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::IndirectProperty(receiver, key) => {
                            push(
                                &mut tasks,
                                ExpressionTask::IndirectReceiver(id, *key, IndirectAction::Read),
                            )?;
                            push(&mut tasks, ExpressionTask::Eval(*receiver))?;
                        }
                        Syntax::Property(receiver, _) => {
                            let property = self.analysis.properties[id.0]
                                .ok_or(self.error(id, "unbound property"))?;
                            push(&mut tasks, ExpressionTask::Property(id, property))?;
                            push(&mut tasks, ExpressionTask::Eval(*receiver))?;
                        }
                        Syntax::Name(_) => {
                            result = Some(self.value(Operation::Load(self.local(id)?), id)?)
                        }
                        Syntax::Membership { value, .. } => {
                            push(&mut tasks, ExpressionTask::MembershipLeft(id))?;
                            push(&mut tasks, ExpressionTask::Eval(*value))?;
                        }
                        Syntax::Move(child) => {
                            push(&mut tasks, ExpressionTask::MoveOwner(id))?;
                            push(&mut tasks, ExpressionTask::Eval(*child))?;
                        }
                        Syntax::Delegated(object) => {
                            let Some(Binding::Delegated(slot, property)) =
                                self.analysis.bindings[id.0]
                            else {
                                return Err(self.error(id, "unbound delegated target"));
                            };
                            let mut remaining = Vec::new();
                            push(&mut remaining, *object)?;
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(
                                    id,
                                    CallTarget::Delegated(slot, property),
                                    remaining,
                                    Vec::new(),
                                ),
                            )?;
                        }
                        Syntax::OwnedCall(child)
                            if matches!(self.ast.nodes[child.0].syntax, Syntax::String(_)) =>
                        {
                            // `local owned t = ''` owns a copy, so the
                            // slot has an owner entry to replace as it grows.
                            push(&mut tasks, ExpressionTask::OwnText(id))?;
                            push(&mut tasks, ExpressionTask::Eval(*child))?;
                        }
                        Syntax::OwnedCall(child)
                        | Syntax::Group(child)
                        | Syntax::ExpandedArgument(child) => {
                            push(&mut tasks, ExpressionTask::Eval(*child))?
                        }
                        Syntax::Unary(op, child, postfix) if matches!(*op, "++" | "--") => {
                            if let Some((receiver, property)) = self.property_destination(*child) {
                                push(
                                    &mut tasks,
                                    ExpressionTask::PropertyIncrement(
                                        id,
                                        property,
                                        if *op == "++" {
                                            Binary::Add
                                        } else {
                                            Binary::Subtract
                                        },
                                        *postfix,
                                    ),
                                )?;
                                push(&mut tasks, ExpressionTask::Receiver(receiver))?;
                                continue;
                            }
                            let slot = self.local(*child)?;
                            let old = self.value(Operation::Load(slot), id)?;
                            let one = self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                            let new = self.value(
                                Operation::Binary(
                                    if *op == "++" {
                                        Binary::Add
                                    } else {
                                        Binary::Subtract
                                    },
                                    old,
                                    one,
                                ),
                                id,
                            )?;
                            self.effect(Operation::Store(slot, new), id)?;
                            result = Some(if *postfix { old } else { new });
                        }
                        Syntax::Unary(op, child, false) => {
                            push(
                                &mut tasks,
                                ExpressionTask::Unary(
                                    id,
                                    unary(op).ok_or(self.error(id, "unknown unary operation"))?,
                                ),
                            )?;
                            push(&mut tasks, ExpressionTask::Eval(*child))?;
                        }
                        Syntax::Binary(op, left, right) if assigned(op) => {
                            let mut target = *left;
                            while let Syntax::Group(inner) = self.ast.nodes[target.0].syntax {
                                target = inner;
                            }
                            if let Syntax::Index(receiver, key) = self.ast.nodes[target.0].syntax {
                                push(&mut tasks, ExpressionTask::IndexRhs(id, receiver, key))?;
                                push(&mut tasks, ExpressionTask::Eval(*right))?;
                                continue;
                            }
                            if let Syntax::IndirectProperty(receiver, key) =
                                self.ast.nodes[target.0].syntax
                            {
                                if *op == "=" {
                                    push(
                                        &mut tasks,
                                        ExpressionTask::IndirectRhs(id, receiver, key),
                                    )?;
                                    push(&mut tasks, ExpressionTask::Eval(*right))?;
                                } else {
                                    let operation = binary(op.strip_suffix('=').unwrap_or(op))
                                        .ok_or(self.error(id, "unknown compound assignment"))?;
                                    push(
                                        &mut tasks,
                                        ExpressionTask::IndirectReceiver(
                                            id,
                                            key,
                                            IndirectAction::Compound(operation, *right),
                                        ),
                                    )?;
                                    push(&mut tasks, ExpressionTask::Eval(receiver))?;
                                }
                                continue;
                            }

                            if let Some((receiver, property)) = self.property_destination(*left) {
                                if *op == "=" {
                                    push(
                                        &mut tasks,
                                        ExpressionTask::PropertyRhs(id, receiver, property),
                                    )?;
                                    push(&mut tasks, ExpressionTask::Eval(*right))?;
                                } else {
                                    let operation = binary(op.strip_suffix('=').unwrap_or(op))
                                        .ok_or(self.error(id, "unknown compound assignment"))?;
                                    push(
                                        &mut tasks,
                                        ExpressionTask::PropertyCompound(
                                            id, property, operation, *right,
                                        ),
                                    )?;
                                    push(&mut tasks, ExpressionTask::Receiver(receiver))?;
                                }
                                continue;
                            }
                            let slot = self.local(*left)?;
                            let old = if *op == "=" {
                                None
                            } else {
                                Some((
                                    binary(op.strip_suffix('=').unwrap_or(op))
                                        .ok_or(self.error(id, "unknown compound assignment"))?,
                                    self.value(Operation::Load(slot), id)?,
                                ))
                            };
                            // appending to an owned local builds text and
                            // hands the slot's owner entry to the result.
                            let accumulate = *op == "+=" && self.owned_local(*left);
                            push(
                                &mut tasks,
                                ExpressionTask::Assign(id, slot, old, accumulate),
                            )?;
                            push(&mut tasks, ExpressionTask::Eval(*right))?;
                        }
                        Syntax::Binary(op, left, right) if matches!(*op, "&&" | "||" | "??") => {
                            push(&mut tasks, ExpressionTask::Short(id, op, *right))?;
                            push(&mut tasks, ExpressionTask::Eval(*left))?;
                        }
                        Syntax::Binary(op, left, right) => {
                            push(&mut tasks, ExpressionTask::Left(id, op, *right))?;
                            push(&mut tasks, ExpressionTask::Eval(*left))?;
                        }
                        Syntax::Conditional(condition, yes, no) => {
                            push(&mut tasks, ExpressionTask::Choose(id, *yes, *no))?;
                            push(&mut tasks, ExpressionTask::Eval(*condition))?;
                        }
                        Syntax::Call(target, arguments) => {
                            let mut target = *target;
                            while let Syntax::Group(inner) = self.ast.nodes[target.0].syntax {
                                target = inner;
                            }
                            let function = if let Syntax::IndirectProperty(receiver, key) =
                                self.ast.nodes[target.0].syntax
                            {
                                CallTarget::Indirect(Receiver::Expression(receiver), key)
                            } else if let Some(kind) = self.analysis.builtins[id.0] {
                                CallTarget::Builtin(kind)
                            } else if let Some(Binding::Delegated(slot, property)) =
                                self.analysis.bindings[target.0]
                            {
                                CallTarget::Delegated(slot, property)
                            } else if let Some(Binding::Inherited(slot, property, object)) =
                                self.analysis.bindings[target.0]
                            {
                                // `inherited Class(...)` and `inherited Class.prop(...)`
                                // both run the definition that class provides, while
                                // plain `inherited.prop(...)` continues past this one.
                                let qualified = match self.ast.nodes[target.0].syntax {
                                    Syntax::QualifiedInherited(_) => true,
                                    Syntax::Property(receiver, _) => matches!(
                                        self.ast.nodes[receiver.0].syntax,
                                        Syntax::QualifiedInherited(_)
                                    ),
                                    _ => false,
                                };
                                if qualified {
                                    CallTarget::Qualified(slot, property, object, false)
                                } else {
                                    CallTarget::Property(
                                        Receiver::SelfSlot(slot, target),
                                        property,
                                        Some(object),
                                    )
                                }
                            } else if let Syntax::Property(receiver, _) =
                                self.ast.nodes[target.0].syntax
                            {
                                CallTarget::Property(
                                    Receiver::Expression(receiver),
                                    self.analysis.properties[target.0]
                                        .ok_or(self.error(target, "unbound method property"))?,
                                    None,
                                )
                            } else if let Some(Binding::SelfProperty(slot, property)) =
                                self.analysis.bindings[target.0]
                            {
                                CallTarget::Property(
                                    Receiver::SelfSlot(slot, target),
                                    property,
                                    None,
                                )
                            } else if let Some(Binding::CapturedSelfProperty(index, property)) =
                                self.analysis.bindings[target.0]
                            {
                                CallTarget::Property(
                                    Receiver::Captured(index, target),
                                    property,
                                    None,
                                )
                            } else {
                                CallTarget::Direct(
                                    self.analysis.calls[id.0]
                                        .ok_or(self.error(id, "unresolved direct call"))?,
                                )
                            };
                            let mut remaining = Vec::new();
                            remaining
                                .try_reserve(arguments.len() + 1)
                                .map_err(|_| Diagnostic::resource(self.ast.nodes[id.0].start))?;
                            if matches!(function, CallTarget::Delegated(..)) {
                                let Syntax::Delegated(object) = self.ast.nodes[target.0].syntax
                                else {
                                    return Err(self.error(id, "unbound delegated target"));
                                };
                                push(&mut remaining, object)?;
                            }
                            // a vocabulary relation's call has no receiver
                            // to take: its dictionary is supplied by the analysis,
                            // not written at the call site.
                            if self.analysis.vocabulary[id.0].is_none()
                                && matches!(
                                    function,
                                    CallTarget::Builtin(
                                        sema::Builtin::OwnedToList
                                            | sema::Builtin::MoveLocalToCollection
                                            | sema::Builtin::MoveLocalToField
                                            | sema::Builtin::ReserveConstruction
                                            | sema::Builtin::ReserveInCollection
                                            | sema::Builtin::RemoveOwned
                                            | sema::Builtin::MoveOwnedTo
                                            | sema::Builtin::MoveFieldOwnerTo
                                            | sema::Builtin::LookupDefault
                                            | sema::Builtin::LookupSetDefault
                                            | sema::Builtin::LookupBuckets
                                            | sema::Builtin::LookupLength
                                            | sema::Builtin::LookupContains
                                            | sema::Builtin::VectorAppend
                                            | sema::Builtin::VectorIndexOf
                                            | sema::Builtin::VectorRemoveAt
                                            | sema::Builtin::VectorRemoveRange
                                            | sema::Builtin::VectorInsert
                                            | sema::Builtin::VectorSort
                                            | sema::Builtin::IndexWhich
                                            | sema::Builtin::ValWhich
                                            | sema::Builtin::LastIndexWhich
                                            | sema::Builtin::CountWhich
                                            | sema::Builtin::ForEachItem
                                            | sema::Builtin::Subset
                                            | sema::Builtin::MapAll
                                            | sema::Builtin::OwnedLength
                                            | sema::Builtin::OwnedAt
                                            | sema::Builtin::PropDefined
                                            | sema::Builtin::OfKind
                                            | sema::Builtin::Substr
                                            | sema::Builtin::StartsWith
                                            | sema::Builtin::EndsWith
                                            | sema::Builtin::FindText
                                            | sema::Builtin::ToLower
                                            | sema::Builtin::ToUpper
                                            | sema::Builtin::Htmlify
                                            | sema::Builtin::FindReplace
                                            | sema::Builtin::Length
                                            | sema::Builtin::DictionaryAdd
                                            | sema::Builtin::DictionaryRemove
                                            | sema::Builtin::DictionaryFind
                                            | sema::Builtin::DictionaryDefined
                                            | sema::Builtin::GrammarParse
                                    )
                                )
                            {
                                // A bare intrinsic method call takes `self` as its
                                // receiver, which the analysis bound on the target.
                                match self.ast.nodes[target.0].syntax {
                                    Syntax::Property(receiver, _) => remaining.push(receiver),
                                    _ => remaining.push(target),
                                }
                            }
                            if matches!(function, CallTarget::Builtin(sema::Builtin::Invoke)) {
                                remaining.push(target);
                            }
                            remaining.extend_from_slice(arguments);
                            push(
                                &mut tasks,
                                ExpressionTask::CallNext(id, function, remaining, Vec::new()),
                            )?;
                        }
                        _ => return Err(self.error(id, "unsupported expression in scalar IR")),
                    }
                }
                ExpressionTask::Unary(id, op) => {
                    let value = self.take(&mut result, id)?;
                    result = Some(self.value(Operation::Unary(op, value), id)?);
                }
                ExpressionTask::Left(id, op, right) => {
                    let left = self.take(&mut result, id)?;
                    push(&mut tasks, ExpressionTask::Right(id, op, left))?;
                    push(&mut tasks, ExpressionTask::Eval(right))?;
                }
                ExpressionTask::Right(id, op, left) => {
                    let right = self.take(&mut result, id)?;
                    result = Some(if op == "," {
                        right
                    } else if op == "+"
                        && (matches!(
                            self.ast.nodes[self.function.source.0].syntax,
                            Syntax::Function {
                                is_static: true,
                                ..
                            }
                        ) || self.owned_concat(id))
                    {
                        let mut arguments = Vec::new();
                        push(&mut arguments, left)?;
                        push(&mut arguments, right)?;
                        self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::Add,
                                arguments,
                            },
                            id,
                        )?
                    } else {
                        self.value(
                            Operation::Binary(
                                binary(op).ok_or(self.error(id, "unknown binary operation"))?,
                                left,
                                right,
                            ),
                            id,
                        )?
                    });
                }
                ExpressionTask::OwnText(id) => {
                    let value = self.take(&mut result, id)?;
                    result = Some(self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::OwnText,
                            arguments: vec![value],
                        },
                        id,
                    )?);
                }
                ExpressionTask::Assign(id, slot, old, accumulate) => {
                    let rhs = self.take(&mut result, id)?;
                    let value = match old {
                        Some((_, old)) if accumulate => {
                            // The append builds new text, which then takes over the
                            // slot's owner entry and releases what it held.
                            let built = self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::Add,
                                    arguments: vec![old, rhs],
                                },
                                id,
                            )?;
                            self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::ReplaceOwner,
                                    arguments: vec![old, built],
                                },
                                id,
                            )?
                        }
                        Some((op, old)) => self.value(Operation::Binary(op, old, rhs), id)?,
                        None => rhs,
                    };
                    self.effect(Operation::Store(slot, value), id)?;
                    result = Some(value);
                }
                ExpressionTask::CallNext(id, function, mut remaining, mut arguments) => {
                    if let Some(next) = remaining.pop() {
                        push(
                            &mut tasks,
                            ExpressionTask::CallArg(id, function, remaining, arguments),
                        )?;
                        push(&mut tasks, ExpressionTask::Eval(next))?;
                    } else {
                        arguments.reverse();
                        match function {
                            CallTarget::ExpandedProperty(property) => {
                                let property =
                                    self.value(Operation::PropertyValue(property), id)?;
                                let mut values = Vec::new();
                                for value in [arguments[0], arguments[0], property, arguments[1]] {
                                    push(&mut values, value)?;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::ApplyMethod,
                                        arguments: values,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::ExpandedIndirect => {
                                let mut values = Vec::new();
                                for value in
                                    [arguments[0], arguments[0], arguments[1], arguments[2]]
                                {
                                    push(&mut values, value)?;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::ApplyMethod,
                                        arguments: values,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Delegated(slot, property) => {
                                // The current property, as the target object defines
                                // it, with `self` unchanged.
                                let slot = *self
                                    .locals
                                    .get(&slot)
                                    .ok_or(self.error(id, "missing self slot"))?;
                                let receiver = self.value(Operation::Load(slot), id)?;
                                let mut values = Vec::new();
                                let (object, rest) = arguments
                                    .split_first()
                                    .ok_or(self.error(id, "missing delegated target"))?;
                                let property =
                                    self.value(Operation::PropertyValue(property), id)?;
                                for value in [receiver, *object, property] {
                                    push(&mut values, value)?;
                                }
                                for value in rest {
                                    push(&mut values, *value)?;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::CallDelegated,
                                        arguments: values,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Qualified(slot, property, object, expanded) => {
                                let slot = *self
                                    .locals
                                    .get(&slot)
                                    .ok_or(self.error(id, "missing self slot"))?;
                                let receiver = self.value(Operation::Load(slot), id)?;
                                let object = self.value(Operation::NamedObject(object), id)?;
                                let property =
                                    self.value(Operation::PropertyValue(property), id)?;
                                let mut values = Vec::new();
                                for value in [receiver, object, property] {
                                    push(&mut values, value)?;
                                }
                                for value in arguments {
                                    push(&mut values, value)?;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: if expanded {
                                            sema::Builtin::ApplyQualified
                                        } else {
                                            sema::Builtin::CallQualified
                                        },
                                        arguments: values,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::TemporaryConstruction(prototype, finish, expanded) => {
                                let prototype = self.expression(prototype)?;
                                let mut begin_args = Vec::new();
                                push(&mut begin_args, prototype)?;
                                let object = self.value(
                                    Operation::Builtin {
                                        kind: if finish
                                            == sema::Builtin::FinishPublishedConstruction
                                        {
                                            sema::Builtin::BeginPublishedConstruction
                                        } else {
                                            sema::Builtin::BeginConstruction
                                        },
                                        arguments: begin_args,
                                    },
                                    id,
                                )?;
                                let construct = self.analysis.properties[id.0]
                                    .ok_or(self.error(id, "missing constructor property"))?;
                                if expanded {
                                    let property =
                                        self.value(Operation::PropertyValue(construct), id)?;
                                    let mut expanded_arguments = Vec::new();
                                    for argument in [object, property, arguments[0]] {
                                        push(&mut expanded_arguments, argument)?;
                                    }
                                    self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::ApplyConstructor,
                                            arguments: expanded_arguments,
                                        },
                                        id,
                                    )?;
                                } else {
                                    self.value(
                                        Operation::CallMethod {
                                            receiver: object,
                                            property: construct.into(),
                                            after: None,
                                            arguments,
                                        },
                                        id,
                                    )?;
                                }
                                let mut finish_args = Vec::new();
                                push(&mut finish_args, object)?;
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: finish,
                                        arguments: finish_args,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Construction(prototype, owner, property) => {
                                // Parser-owned operands are atomic bound names/addresses, never recursive expressions.
                                let prototype = self.expression(prototype)?;
                                let owner = self.expression(owner)?;
                                let property = self.expression(property)?;
                                let mut begin_args = Vec::new();
                                push(&mut begin_args, prototype)?;
                                let object = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::BeginConstruction,
                                        arguments: begin_args,
                                    },
                                    id,
                                )?;
                                let construct = self.analysis.properties[id.0]
                                    .ok_or(self.error(id, "missing constructor property"))?;
                                self.value(
                                    Operation::CallMethod {
                                        receiver: object,
                                        property: construct.into(),
                                        after: None,
                                        arguments,
                                    },
                                    id,
                                )?;
                                let mut finish_args = Vec::new();
                                for value in [object, owner, property] {
                                    push(&mut finish_args, value)?;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::FinishConstruction,
                                        arguments: finish_args,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Builtin(sema::Builtin::VectorSort) => {
                                // the runtime quicksort suspends at each
                                // comparison; the comparator runs here with ordinary
                                // call, exception and scope handling.
                                let [vector, descending, comparator] = arguments[..] else {
                                    return Err(self.error(id, "sort requires a comparator"));
                                };
                                self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::VectorSortBegin,
                                        arguments: vec![vector, descending],
                                    },
                                    id,
                                )?;
                                let nil = self.value(Operation::Constant(Scalar::Nil), id)?;
                                let outcome = self.temp()?;
                                self.effect(Operation::Reset(outcome), id)?;
                                self.effect(Operation::Store(outcome, nil), id)?;
                                let test = self.block(id)?;
                                let body = self.block(id)?;
                                let exit = self.block(id)?;
                                self.terminate(Terminator::Jump(test));
                                self.current = test;
                                let last = self.value(Operation::Load(outcome), id)?;
                                let more = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::VectorSortNext,
                                        arguments: vec![vector, last],
                                    },
                                    id,
                                )?;
                                self.effect(Operation::LogicalGuard(more), id)?;
                                self.terminate(Terminator::Branch {
                                    condition: more,
                                    mode: BranchMode::Logical,
                                    yes: body,
                                    no: exit,
                                });
                                self.current = body;
                                let mut pair = Vec::new();
                                for which in 0..2 {
                                    let which = self
                                        .value(Operation::Constant(Scalar::Integer(which)), id)?;
                                    pair.push(self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::VectorSortElement,
                                            arguments: vec![vector, which],
                                        },
                                        id,
                                    )?);
                                }
                                let order = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::Invoke,
                                        arguments: vec![comparator, pair[0], pair[1]],
                                    },
                                    id,
                                )?;
                                self.effect(Operation::Store(outcome, order), id)?;
                                self.terminate(Terminator::Jump(test));
                                self.current = exit;
                                result = Some(vector);
                            }
                            CallTarget::Builtin(sema::Builtin::GrammarParse) => {
                                // A parse with no recipient hands its trees to the
                                // turn, so the missing argument reads as nil.
                                let mut arguments = arguments;
                                if arguments.len() == 3 {
                                    let none = self.value(Operation::Constant(Scalar::Nil), id)?;
                                    push(&mut arguments, none)?;
                                }
                                // bare match objects, then construct on each in
                                // pre-order where defined, then property assignment.
                                let order = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::GrammarBegin,
                                        arguments,
                                    },
                                    id,
                                )?;
                                if let Some(construct) = self.analysis.grammar_construct {
                                    let count = self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::Length,
                                            arguments: vec![order],
                                        },
                                        id,
                                    )?;
                                    let one =
                                        self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                                    let index = self.temp()?;
                                    self.effect(Operation::Reset(index), id)?;
                                    self.effect(Operation::Store(index, one), id)?;
                                    let test = self.block(id)?;
                                    let body = self.block(id)?;
                                    let call = self.block(id)?;
                                    let next = self.block(id)?;
                                    let exit = self.block(id)?;
                                    self.terminate(Terminator::Jump(test));
                                    self.current = test;
                                    let current = self.value(Operation::Load(index), id)?;
                                    let more = self.value(
                                        Operation::Binary(Binary::LessEqual, current, count),
                                        id,
                                    )?;
                                    self.effect(Operation::LogicalGuard(more), id)?;
                                    self.terminate(Terminator::Branch {
                                        condition: more,
                                        mode: BranchMode::Logical,
                                        yes: body,
                                        no: exit,
                                    });
                                    self.current = body;
                                    let element = self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::ListIndex,
                                            arguments: vec![order, current],
                                        },
                                        id,
                                    )?;
                                    let property =
                                        self.value(Operation::PropertyValue(construct), id)?;
                                    let defined = self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::PropDefined,
                                            arguments: vec![element, property],
                                        },
                                        id,
                                    )?;
                                    self.effect(Operation::LogicalGuard(defined), id)?;
                                    self.terminate(Terminator::Branch {
                                        condition: defined,
                                        mode: BranchMode::Logical,
                                        yes: call,
                                        no: next,
                                    });
                                    self.current = call;
                                    self.value(
                                        Operation::CallMethod {
                                            receiver: element,
                                            property: construct.into(),
                                            after: None,
                                            arguments: Vec::new(),
                                        },
                                        id,
                                    )?;
                                    self.terminate(Terminator::Jump(next));
                                    self.current = next;
                                    let step = self
                                        .value(Operation::Binary(Binary::Add, current, one), id)?;
                                    self.effect(Operation::Store(index, step), id)?;
                                    self.terminate(Terminator::Jump(test));
                                    self.current = exit;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::GrammarFinish,
                                        arguments: vec![order],
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Builtin(
                                kind @ (sema::Builtin::Subset | sema::Builtin::MapAll),
                            ) => {
                                // build the new list in a scope-owned
                                // vector, then snapshot it like any owned result.
                                let [collection, callback] = arguments[..] else {
                                    return Err(self.error(id, "list method takes one callback"));
                                };
                                let count = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::Length,
                                        arguments: vec![collection],
                                    },
                                    id,
                                )?;
                                let buffer = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::NewLocalVector,
                                        arguments: vec![count],
                                    },
                                    id,
                                )?;
                                let one =
                                    self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                                let index = self.temp()?;
                                self.effect(Operation::Reset(index), id)?;
                                self.effect(Operation::Store(index, one), id)?;
                                let test = self.block(id)?;
                                let body = self.block(id)?;
                                let keep = self.block(id)?;
                                let next = self.block(id)?;
                                let exit = self.block(id)?;
                                self.terminate(Terminator::Jump(test));
                                self.current = test;
                                let current = self.value(Operation::Load(index), id)?;
                                let more = self.value(
                                    Operation::Binary(Binary::LessEqual, current, count),
                                    id,
                                )?;
                                self.effect(Operation::LogicalGuard(more), id)?;
                                self.terminate(Terminator::Branch {
                                    condition: more,
                                    mode: BranchMode::Logical,
                                    yes: body,
                                    no: exit,
                                });
                                self.current = body;
                                let element = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::ListIndex,
                                        arguments: vec![collection, current],
                                    },
                                    id,
                                )?;
                                let mapped = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::Invoke,
                                        arguments: vec![callback, element],
                                    },
                                    id,
                                )?;
                                if kind == sema::Builtin::MapAll {
                                    self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::VectorAppend,
                                            arguments: vec![buffer, mapped],
                                        },
                                        id,
                                    )?;
                                    self.terminate(Terminator::Jump(next));
                                } else {
                                    self.effect(Operation::LogicalGuard(mapped), id)?;
                                    self.terminate(Terminator::Branch {
                                        condition: mapped,
                                        mode: BranchMode::Logical,
                                        yes: keep,
                                        no: next,
                                    });
                                    self.current = keep;
                                    self.value(
                                        Operation::Builtin {
                                            kind: sema::Builtin::VectorAppend,
                                            arguments: vec![buffer, element],
                                        },
                                        id,
                                    )?;
                                    self.terminate(Terminator::Jump(next));
                                }
                                self.current = next;
                                let step =
                                    self.value(Operation::Binary(Binary::Add, current, one), id)?;
                                self.effect(Operation::Store(index, step), id)?;
                                self.terminate(Terminator::Jump(test));
                                self.current = exit;
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::OwnedToList,
                                        arguments: vec![buffer],
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Builtin(
                                kind @ (sema::Builtin::IndexWhich
                                | sema::Builtin::ValWhich
                                | sema::Builtin::LastIndexWhich
                                | sema::Builtin::CountWhich
                                | sema::Builtin::ForEachItem),
                            ) => {
                                // walk the collection and call the
                                // callback per element, as the reference does.
                                let [collection, callback] = arguments[..] else {
                                    return Err(self.error(id, "list method takes one callback"));
                                };
                                let count = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::Length,
                                        arguments: vec![collection],
                                    },
                                    id,
                                )?;
                                let one =
                                    self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                                let nil = self.value(Operation::Constant(Scalar::Nil), id)?;
                                let zero =
                                    self.value(Operation::Constant(Scalar::Integer(0)), id)?;
                                let last = kind == sema::Builtin::LastIndexWhich;
                                let index = self.temp()?;
                                let outcome = self.temp()?;
                                self.effect(Operation::Reset(index), id)?;
                                self.effect(Operation::Reset(outcome), id)?;
                                let start = if last { count } else { one };
                                self.effect(Operation::Store(index, start), id)?;
                                let initial = if kind == sema::Builtin::CountWhich {
                                    zero
                                } else {
                                    nil
                                };
                                self.effect(Operation::Store(outcome, initial), id)?;
                                let test = self.block(id)?;
                                let body = self.block(id)?;
                                let keep = self.block(id)?;
                                let next = self.block(id)?;
                                let exit = self.block(id)?;
                                self.terminate(Terminator::Jump(test));
                                self.current = test;
                                let current = self.value(Operation::Load(index), id)?;
                                let more = self.value(
                                    Operation::Binary(
                                        if last {
                                            Binary::GreaterEqual
                                        } else {
                                            Binary::LessEqual
                                        },
                                        current,
                                        if last { one } else { count },
                                    ),
                                    id,
                                )?;
                                self.effect(Operation::LogicalGuard(more), id)?;
                                self.terminate(Terminator::Branch {
                                    condition: more,
                                    mode: BranchMode::Logical,
                                    yes: body,
                                    no: exit,
                                });
                                self.current = body;
                                let element = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::ListIndex,
                                        arguments: vec![collection, current],
                                    },
                                    id,
                                )?;
                                let verdict = self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::Invoke,
                                        arguments: vec![callback, element],
                                    },
                                    id,
                                )?;
                                if kind == sema::Builtin::ForEachItem {
                                    self.terminate(Terminator::Jump(next));
                                } else {
                                    self.effect(Operation::LogicalGuard(verdict), id)?;
                                    self.terminate(Terminator::Branch {
                                        condition: verdict,
                                        mode: BranchMode::Logical,
                                        yes: keep,
                                        no: next,
                                    });
                                    self.current = keep;
                                    match kind {
                                        sema::Builtin::CountWhich => {
                                            let total = self.value(Operation::Load(outcome), id)?;
                                            let total = self.value(
                                                Operation::Binary(Binary::Add, total, one),
                                                id,
                                            )?;
                                            self.effect(Operation::Store(outcome, total), id)?;
                                            self.terminate(Terminator::Jump(next));
                                        }
                                        sema::Builtin::ValWhich => {
                                            self.effect(Operation::Store(outcome, element), id)?;
                                            self.terminate(Terminator::Jump(exit));
                                        }
                                        _ => {
                                            self.effect(Operation::Store(outcome, current), id)?;
                                            self.terminate(Terminator::Jump(exit));
                                        }
                                    }
                                }
                                self.current = next;
                                let step = self.value(
                                    Operation::Binary(
                                        if last { Binary::Subtract } else { Binary::Add },
                                        current,
                                        one,
                                    ),
                                    id,
                                )?;
                                self.effect(Operation::Store(index, step), id)?;
                                self.terminate(Terminator::Jump(test));
                                self.current = exit;
                                result = Some(self.value(Operation::Load(outcome), id)?);
                            }
                            CallTarget::Builtin(
                                sema::Builtin::PreinitMode
                                | sema::Builtin::RunGc
                                | sema::Builtin::DebugTrace
                                | sema::Builtin::GlobalSymbols,
                            ) => {
                                // an ahead-of-time build has no preinit
                                // mode, collector or symbol table. Arguments still
                                // evaluate; their values are unused.
                                result = Some(self.value(Operation::Constant(Scalar::Nil), id)?);
                            }
                            // a vocabulary relation's operations are
                            // dictionary operations. The dictionary and the
                            // vocabulary property are not written at the call site,
                            // so the analysis supplies both and they are passed in
                            // the operands `addWord` uses.
                            CallTarget::Builtin(kind)
                                if self.analysis.vocabulary[id.0].is_some() =>
                            {
                                let (dictionary, property, from_label) = self.analysis.vocabulary
                                    [id.0]
                                    .ok_or(self.error(id, "missing vocabulary relation"))?;
                                let dictionary =
                                    self.value(Operation::NamedObject(dictionary), id)?;
                                let property =
                                    self.value(Operation::PropertyValue(property), id)?;
                                let mut values = Vec::new();
                                push(&mut values, dictionary)?;
                                // the label became the property, so it
                                // is not also an operand. Every other argument
                                // stands where it did.
                                let carried = if from_label {
                                    arguments.len().saturating_sub(1)
                                } else {
                                    arguments.len()
                                };
                                for value in arguments.into_iter().take(carried) {
                                    push(&mut values, value)?;
                                }
                                push(&mut values, property)?;
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind,
                                        arguments: values,
                                    },
                                    id,
                                )?);
                            }
                            // a relation call carries the descriptor the
                            // analysis resolved from the name it was written with.
                            CallTarget::Builtin(kind)
                                if self.analysis.relations[id.0].is_some() =>
                            {
                                let descriptor = self.analysis.relations[id.0]
                                    .ok_or(self.error(id, "missing relation"))?;
                                let descriptor = self.value(
                                    Operation::Constant(Scalar::Integer(
                                        i32::try_from(descriptor).map_err(|_| {
                                            self.error(id, "relation is out of range")
                                        })?,
                                    )),
                                    id,
                                )?;
                                let mut values = Vec::new();
                                push(&mut values, descriptor)?;
                                for value in arguments {
                                    push(&mut values, value)?;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind,
                                        arguments: values,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Builtin(sema::Builtin::GetTime) => {
                                // The reference defaults to the calendar mode when
                                // the argument is omitted (tadsgen.h).
                                if arguments.is_empty() {
                                    let mode =
                                        self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                                    push(&mut arguments, mode)?;
                                }
                                result = Some(self.value(
                                    Operation::Builtin {
                                        kind: sema::Builtin::GetTime,
                                        arguments,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Builtin(kind) => {
                                result =
                                    Some(self.value(Operation::Builtin { kind, arguments }, id)?);
                            }
                            CallTarget::Direct(function) => {
                                let Syntax::Function {
                                    parameters, rest, ..
                                } = &self.ast.nodes[self.ast.functions[function].0].syntax
                                else {
                                    unreachable!()
                                };
                                let fixed = parameters.len() - usize::from(*rest);
                                while !rest && arguments.len() < fixed {
                                    let nil = self.value(Operation::Constant(Scalar::Nil), id)?;
                                    push(&mut arguments, nil)?;
                                }
                                result = Some(self.value(
                                    Operation::Call {
                                        function,
                                        arguments,
                                    },
                                    id,
                                )?);
                            }
                            CallTarget::Indirect(receiver, key) => {
                                push(
                                    &mut tasks,
                                    ExpressionTask::IndirectReceiver(
                                        id,
                                        key,
                                        IndirectAction::Call(arguments),
                                    ),
                                )?;
                                push(&mut tasks, ExpressionTask::Receiver(receiver))?;
                            }
                            CallTarget::Property(receiver, property, after) => {
                                push(
                                    &mut tasks,
                                    ExpressionTask::MethodReceiver(id, property, after, arguments),
                                )?;
                                push(&mut tasks, ExpressionTask::Receiver(receiver))?;
                            }
                        }
                    }
                }
                ExpressionTask::CallArg(id, function, remaining, mut arguments) => {
                    push(&mut arguments, self.take(&mut result, id)?)?;
                    push(
                        &mut tasks,
                        ExpressionTask::CallNext(id, function, remaining, arguments),
                    )?;
                }
                ExpressionTask::Choose(id, yes, no) => {
                    let condition = self.take(&mut result, id)?;
                    self.effect(Operation::LogicalGuard(condition), id)?;
                    let slot = self.temp()?;
                    self.effect(Operation::Reset(slot), id)?;
                    let yes_block = self.block(id)?;
                    let no_block = self.block(id)?;
                    let join = self.block(id)?;
                    self.terminate(Terminator::Branch {
                        condition,
                        mode: BranchMode::Logical,
                        yes: yes_block,
                        no: no_block,
                    });
                    self.current = yes_block;
                    push(
                        &mut tasks,
                        ExpressionTask::Then(id, no, no_block, join, slot),
                    )?;
                    push(&mut tasks, ExpressionTask::Eval(yes))?;
                }
                ExpressionTask::Then(id, no, no_block, join, slot) => {
                    let yes = self.take(&mut result, id)?;
                    self.effect(Operation::Store(slot, yes), id)?;
                    self.terminate(Terminator::Jump(join));
                    self.current = no_block;
                    push(&mut tasks, ExpressionTask::Else(id, join, slot))?;
                    push(&mut tasks, ExpressionTask::Eval(no))?;
                }
                ExpressionTask::Else(id, join, slot) => {
                    let no = self.take(&mut result, id)?;
                    self.effect(Operation::Store(slot, no), id)?;
                    self.terminate(Terminator::Jump(join));
                    self.current = join;
                    result = Some(self.value(Operation::Load(slot), id)?);
                }
                ExpressionTask::Short(id, op, right) => {
                    let left = self.take(&mut result, id)?;
                    if op != "??" {
                        self.effect(Operation::LogicalGuard(left), id)?;
                    }
                    let slot = self.temp()?;
                    self.effect(Operation::Store(slot, left), id)?;
                    let rhs = self.block(id)?;
                    let join = self.block(id)?;
                    let (mode, yes, no) = match op {
                        "&&" => (BranchMode::Logical, rhs, join),
                        "||" => (BranchMode::Logical, join, rhs),
                        _ => (BranchMode::IsNil, rhs, join),
                    };
                    self.terminate(Terminator::Branch {
                        condition: left,
                        mode,
                        yes,
                        no,
                    });
                    self.current = rhs;
                    push(
                        &mut tasks,
                        ExpressionTask::ShortEnd(id, join, slot, op != "??"),
                    )?;
                    push(&mut tasks, ExpressionTask::Eval(right))?;
                }
                ExpressionTask::ShortEnd(id, join, slot, logical) => {
                    let right = self.take(&mut result, id)?;
                    if logical {
                        self.effect(Operation::LogicalGuard(right), id)?;
                    }
                    self.effect(Operation::Store(slot, right), id)?;
                    self.terminate(Terminator::Jump(join));
                    self.current = join;
                    result = Some(self.value(Operation::Load(slot), id)?);
                }
            }
        }
        self.take(&mut result, root)
    }
    // Finalizer dispatch must not invent normal fallthrough when every protected
    // path returns or throws. Pending transfers have already left this block.
    fn current_reachable(&self) -> Result<bool, Diagnostic> {
        let mut seen = Vec::new();
        seen.try_reserve_exact(self.function.blocks.len())
            .map_err(|_| Diagnostic::resource(0))?;
        seen.resize(self.function.blocks.len(), false);
        let mut pending = Vec::new();
        push(&mut pending, BlockId(0))?;
        while let Some(block) = pending.pop() {
            if block == self.current {
                return Ok(true);
            }
            if seen[block.0] {
                continue;
            }
            seen[block.0] = true;
            let term = &self.function.blocks[block.0].terminator;
            if let Some(edge) = term.exception() {
                push(&mut pending, edge.target)?;
            }
            match term {
                Terminator::Jump(target) | Terminator::Invoke { normal: target, .. } => {
                    push(&mut pending, *target)?
                }
                Terminator::Branch { yes, no, .. } => {
                    push(&mut pending, *yes)?;
                    push(&mut pending, *no)?;
                }
                _ => {}
            }
        }
        Ok(false)
    }
    fn route_action(
        &mut self,
        action: Action,
        value: Option<Value>,
        site: Id,
    ) -> Result<(), Diagnostic> {
        let depth = match action {
            Action::Return => 0,
            Action::Jump { finally_depth, .. } => finally_depth,
        };
        if self.active_finalizers.len() > depth {
            let index = *self
                .active_finalizers
                .last()
                .ok_or(self.error(site, "missing finalizer"))?;
            let (slot, reason, entry) = {
                let f = &self.finalizers[index];
                (f.value, f.reason, f.entry)
            };
            if let Some(value) = value {
                self.effect(Operation::Store(slot, value), site)?;
            }
            let code = i32::try_from(self.finalizers[index].actions.len())
                .map_err(|_| Diagnostic::resource(self.ast.nodes[site.0].start))?
                .checked_add(3)
                .ok_or(Diagnostic::resource(0))?;
            push(&mut self.finalizers[index].actions, action)?;
            let code = self.value(Operation::Constant(Scalar::Integer(code)), site)?;
            self.effect(Operation::Store(reason, code), site)?;
            self.terminate(Terminator::Jump(entry));
        } else {
            match action {
                Action::Return => {
                    if matches!(
                        self.ast.nodes[self.function.source.0].syntax,
                        Syntax::Function {
                            is_static: true,
                            ..
                        }
                    ) {
                        self.effect(Operation::EndStatic, site)?;
                    }
                    for _ in 0..self.iterations {
                        self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::EndIteration,
                                arguments: Vec::new(),
                            },
                            site,
                        )?;
                    }
                    self.terminate(Terminator::Return(
                        value.ok_or(self.error(site, "missing return value"))?,
                    ));
                }
                Action::Jump {
                    target,
                    handler_depth,
                    iteration_depth,
                    ..
                } => {
                    let mut remaining_iterations = self.iterations;
                    for index in (handler_depth..self.handlers.len()).rev() {
                        remaining_iterations =
                            remaining_iterations.min(self.handlers[index].iterations);
                        let checkpoint = self.handlers[index].checkpoint;
                        self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::EndScope,
                                arguments: vec![checkpoint],
                            },
                            site,
                        )?;
                    }
                    for _ in iteration_depth..remaining_iterations {
                        self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::EndIteration,
                                arguments: Vec::new(),
                            },
                            site,
                        )?;
                    }
                    self.terminate(Terminator::Jump(target));
                }
            }
        }
        self.current = self.block(site)?;
        Ok(())
    }
    /// A goto label's IR block, created on first use.
    fn goto_target(&mut self, label: Id) -> Result<BlockId, Diagnostic> {
        if let Some((block, _)) = self.goto_targets.get(&label.0) {
            return Ok(*block);
        }
        let block = self.block(label)?;
        self.goto_targets
            .try_reserve(1)
            .map_err(|_| Diagnostic::resource(0))?;
        self.goto_targets.insert(label.0, (block, None));
        Ok(block)
    }
    /// Record exit depths for labels on the direct statements of an entered block or
    /// switch body. Statements before a label cannot change these depths.
    fn enter_goto_sequence(&mut self, items: impl Iterator<Item = Id>) -> Result<(), Diagnostic> {
        let depths = (
            self.handlers.len(),
            self.active_finalizers.len(),
            self.iterations,
        );
        for mut item in items {
            while let Syntax::Label(_, body) = self.ast.nodes[item.0].syntax {
                self.goto_target(item)?;
                if let Some(entry) = self.goto_targets.get_mut(&item.0) {
                    entry.1 = Some(depths);
                }
                item = body;
            }
        }
        Ok(())
    }
    fn statements(&mut self, root: Id) -> Result<(), Diagnostic> {
        let mut tasks = Vec::new();
        push(&mut tasks, StatementTask::Visit(root, None))?;
        // Index this body's goto labels; nested function bodies lower separately.
        let ast = self.ast;
        let mut pending = Vec::new();
        push(&mut pending, root)?;
        let mut failed = false;
        while let Some(node) = pending.pop() {
            match &ast.nodes[node.0].syntax {
                Syntax::Function { .. } if node != root => continue,
                Syntax::Label(name, _) => {
                    self.goto_labels
                        .try_reserve(1)
                        .map_err(|_| Diagnostic::resource(0))?;
                    self.goto_labels.insert(name.as_str(), node);
                }
                _ => {}
            }
            crate::sema::children(&ast.nodes[node.0].syntax, &mut |child| {
                if pending.try_reserve(1).is_err() {
                    failed = true;
                } else {
                    pending.push(child);
                }
            });
            if failed {
                return Err(Diagnostic::resource(0));
            }
        }
        while let Some(task) = tasks.pop() {
            match task {
                StatementTask::EndLabel(exit) => {
                    self.labels.pop();
                    self.terminate(Terminator::Jump(exit));
                    self.current = exit;
                }
                StatementTask::Visit(id, context) => match &self.ast.nodes[id.0].syntax {
                    Syntax::Label(_, body) => {
                        // Every label starts a block that a goto may target.
                        let target = self.goto_target(id)?;
                        self.terminate(Terminator::Jump(target));
                        self.current = target;
                        // `break label` leaves the labelled statement. A
                        // labelled loop replaces this with its own context, so its
                        // break and continue keep the loop's targets.
                        let exit = self.block(id)?;
                        push(
                            &mut self.labels,
                            (
                                id,
                                Some(Loop {
                                    iterations: self.iterations,
                                    test: None,
                                    exit,
                                    break_depth: self.handlers.len(),
                                    continue_depth: context
                                        .map_or(self.handlers.len(), |c| c.continue_depth),
                                    break_finally: self.active_finalizers.len(),
                                    continue_finally: context
                                        .map_or(self.active_finalizers.len(), |c| {
                                            c.continue_finally
                                        }),
                                }),
                            ),
                        )?;
                        push(&mut tasks, StatementTask::EndLabel(exit))?;
                        push(&mut tasks, StatementTask::Visit(*body, context))?;
                    }
                    Syntax::Block(items) => {
                        self.enter_goto_sequence(items.iter().copied())?;
                        for item in items.iter().rev() {
                            push(&mut tasks, StatementTask::Visit(*item, context))?;
                        }
                    }
                    Syntax::Empty => {}
                    Syntax::Expression(expr) => {
                        let mut value = *expr;
                        while let Syntax::Group(inner) = self.ast.nodes[value.0].syntax {
                            value = inner;
                        }
                        if matches!(&self.ast.nodes[value.0].syntax, Syntax::String(literal) if literal.emitting)
                        {
                            let literal = self.analysis.strings[value.0]
                                .ok_or(self.error(value, "unbound output literal"))?;
                            self.effect(Operation::EmitLiteral(literal), value)?;
                        } else {
                            self.expression(*expr)?;
                        }
                    }
                    Syntax::Local(items) | Syntax::OwnedLocal(items) => {
                        for (ordinal, (_, initializer)) in items.iter().enumerate() {
                            let slot = *self
                                .declarations
                                .get(&(id.0, ordinal))
                                .ok_or(self.error(id, "missing local declaration"))?;
                            self.effect(Operation::Reset(slot), id)?;
                            if let Some(expr) = initializer {
                                let value = self.expression(*expr)?;
                                self.effect(Operation::Store(slot, value), id)?;
                            }
                        }
                    }
                    Syntax::Throw(expr) => {
                        let value = self.expression(*expr)?;
                        for _ in 0..if self.handlers.is_empty() {
                            self.iterations
                        } else {
                            0
                        } {
                            self.value(
                                Operation::Builtin {
                                    kind: sema::Builtin::EndIteration,
                                    arguments: Vec::new(),
                                },
                                id,
                            )?;
                        }
                        self.terminate(Terminator::Throw(
                            value,
                            id,
                            self.handlers.last().map(|handler| handler.edge),
                        ));
                        self.current = self.block(id)?;
                    }
                    Syntax::Return(expr) => {
                        let value = if let Some(expr) = expr {
                            self.expression(*expr)?
                        } else {
                            self.value(Operation::Constant(Scalar::Nil), id)?
                        };
                        self.route_action(Action::Return, Some(value), id)?;
                    }
                    Syntax::If(condition, yes, no) => {
                        let value = self.expression(*condition)?;
                        self.effect(Operation::LogicalGuard(value), id)?;
                        let yes_block = self.block(id)?;
                        let no_block = self.block(id)?;
                        let join = self.block(id)?;
                        self.terminate(Terminator::Branch {
                            condition: value,
                            mode: BranchMode::Logical,
                            yes: yes_block,
                            no: no_block,
                        });
                        self.current = yes_block;
                        push(
                            &mut tasks,
                            StatementTask::AfterThen(id, *no, context, no_block, join),
                        )?;
                        push(&mut tasks, StatementTask::Visit(*yes, context))?;
                    }
                    Syntax::Switch(selector, arms) => {
                        self.enter_goto_sequence(
                            arms.iter()
                                .flat_map(|(_, statements)| statements.iter().copied()),
                        )?;
                        let selected = self.expression(*selector)?;
                        let exit = self.block(id)?;
                        let mut bodies = Vec::new();
                        for _ in arms {
                            let block = self.block(id)?;
                            push(&mut bodies, block)?;
                        }
                        let mut default = exit;
                        for ((case, _), body) in arms.iter().zip(&bodies) {
                            if let Some(case) = case {
                                let value = self.expression(*case)?;
                                let equal = self.value(
                                    Operation::Binary(Binary::Equal, selected, value),
                                    *case,
                                )?;
                                self.effect(Operation::LogicalGuard(equal), *case)?;
                                let next = self.block(id)?;
                                self.terminate(Terminator::Branch {
                                    condition: equal,
                                    mode: BranchMode::Logical,
                                    yes: *body,
                                    no: next,
                                });
                                self.current = next;
                            } else {
                                default = *body;
                            }
                        }
                        self.terminate(Terminator::Jump(default));
                        let inner = Some(self.loop_context(
                            id,
                            Loop {
                                iterations: self.iterations,
                                test: context.and_then(|context| context.test),
                                exit,
                                break_depth: self.handlers.len(),
                                continue_depth:
                                    context.map_or(self.handlers.len(), |context| {
                                        context.continue_depth
                                    }),
                                break_finally: self.active_finalizers.len(),
                                continue_finally:
                                    context.map_or(self.active_finalizers.len(), |context| {
                                        context.continue_finally
                                    }),
                            },
                        ));
                        if arms.is_empty() {
                            self.current = exit;
                        }
                        for (index, (_, statements)) in arms.iter().enumerate().rev() {
                            let next = bodies.get(index + 1).copied().unwrap_or(exit);
                            push(&mut tasks, StatementTask::AfterElse(next))?;
                            for statement in statements.iter().rev() {
                                push(&mut tasks, StatementTask::Visit(*statement, inner))?;
                            }
                            push(&mut tasks, StatementTask::Begin(bodies[index]))?;
                        }
                    }
                    Syntax::Finally(body, _) | Syntax::OwnerScope(body) => {
                        let checkpoint = self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::BeginScope,
                                arguments: Vec::new(),
                            },
                            id,
                        )?;
                        let slot = self.temp()?;
                        let target = self.block(id)?;
                        let entry = self.block(id)?;
                        let end = self.block(id)?;
                        let handler = Handler {
                            iterations: self.iterations,
                            edge: ExceptionEdge { target, slot },
                            checkpoint,
                        };
                        let reason = self.temp()?;
                        let value = self.temp()?;
                        let packet = [self.temp()?, self.temp()?, self.temp()?, self.temp()?];
                        let nil = self.value(Operation::Constant(Scalar::Nil), id)?;
                        for slot in [reason, value, packet[0], packet[1], packet[2], packet[3]] {
                            self.effect(Operation::Store(slot, nil), id)?;
                        }
                        let index = self.finalizers.len();
                        push(
                            &mut self.finalizers,
                            Finalizer {
                                entry,
                                end,
                                handler,
                                reason,
                                value,
                                packet,
                                actions: Vec::new(),
                                normal: false,
                            },
                        )?;
                        push(&mut self.active_finalizers, index)?;
                        // A block has one exception destination. Keep calls before
                        // this protected scope outside its handler.
                        let protected = self.block(id)?;
                        self.terminate(Terminator::Jump(protected));
                        self.current = protected;
                        push(&mut self.handlers, handler)?;
                        push(
                            &mut tasks,
                            StatementTask::AfterProtected(id, index, context),
                        )?;
                        push(&mut tasks, StatementTask::Visit(*body, context))?;
                    }
                    Syntax::Try(body, _) => {
                        let checkpoint = self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::BeginScope,
                                arguments: Vec::new(),
                            },
                            id,
                        )?;
                        let slot = self.temp()?;
                        let target = self.block(id)?;
                        let end = self.block(id)?;
                        let handler = Handler {
                            iterations: self.iterations,
                            edge: ExceptionEdge { target, slot },
                            checkpoint,
                        };
                        // A block has one exception destination. Keep calls before
                        // this protected scope outside its handler.
                        let protected = self.block(id)?;
                        self.terminate(Terminator::Jump(protected));
                        self.current = protected;
                        push(&mut self.handlers, handler)?;
                        push(
                            &mut tasks,
                            StatementTask::AfterTry(id, handler, end, context),
                        )?;
                        push(&mut tasks, StatementTask::Visit(*body, context))?;
                    }
                    Syntax::ForEach(declaration, collection, body) => {
                        let source = self.expression(*collection)?;
                        self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::BeginIteration,
                                arguments: vec![source],
                            },
                            id,
                        )?;
                        self.iterations += 1;
                        let test = self.block(id)?;
                        let body_block = self.block(id)?;
                        let exit = self.block(id)?;
                        self.terminate(Terminator::Jump(test));
                        self.current = test;
                        let next = self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::AdvanceIteration,
                                arguments: Vec::new(),
                            },
                            id,
                        )?;
                        self.effect(Operation::LogicalGuard(next), id)?;
                        self.terminate(Terminator::Branch {
                            condition: next,
                            mode: BranchMode::Logical,
                            yes: body_block,
                            no: exit,
                        });
                        self.current = body_block;
                        let value = self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::IterationValue,
                                arguments: Vec::new(),
                            },
                            id,
                        )?;
                        let slot = *self
                            .declarations
                            .get(&(declaration.0, 0))
                            .ok_or(self.error(id, "missing iteration binding"))?;
                        self.effect(Operation::Reset(slot), id)?;
                        self.effect(Operation::Store(slot, value), id)?;
                        push(&mut tasks, StatementTask::EndIteration(id, test, exit))?;
                        push(
                            &mut tasks,
                            StatementTask::Visit(
                                *body,
                                Some(self.loop_context(
                                    id,
                                    Loop {
                                        iterations: self.iterations,
                                        test: Some(test),
                                        exit,
                                        break_depth: self.handlers.len(),
                                        continue_depth: self.handlers.len(),
                                        break_finally: self.active_finalizers.len(),
                                        continue_finally: self.active_finalizers.len(),
                                    },
                                )),
                            ),
                        )?;
                    }
                    Syntax::For {
                        init,
                        condition,
                        step,
                        body,
                    } => {
                        push(
                            &mut tasks,
                            StatementTask::ForStart(id, *condition, *step, *body),
                        )?;
                        for initializer in init.iter().rev() {
                            push(&mut tasks, StatementTask::Visit(*initializer, context))?;
                        }
                    }
                    Syntax::DoWhile(condition, body) => {
                        let body_block = self.block(id)?;
                        let test = self.block(id)?;
                        let exit = self.block(id)?;
                        self.terminate(Terminator::Jump(body_block));
                        self.current = body_block;
                        push(
                            &mut tasks,
                            StatementTask::AfterDo(id, *condition, test, body_block, exit),
                        )?;
                        push(
                            &mut tasks,
                            StatementTask::Visit(
                                *body,
                                Some(self.loop_context(
                                    id,
                                    Loop {
                                        iterations: self.iterations,
                                        test: Some(test),
                                        exit,
                                        break_depth: self.handlers.len(),
                                        continue_depth: self.handlers.len(),
                                        break_finally: self.active_finalizers.len(),
                                        continue_finally: self.active_finalizers.len(),
                                    },
                                )),
                            ),
                        )?;
                    }
                    Syntax::While(condition, body) => {
                        let test = self.block(id)?;
                        let body_block = self.block(id)?;
                        let exit = self.block(id)?;
                        self.terminate(Terminator::Jump(test));
                        self.current = test;
                        let value = self.expression(*condition)?;
                        self.effect(Operation::LogicalGuard(value), id)?;
                        self.terminate(Terminator::Branch {
                            condition: value,
                            mode: BranchMode::Logical,
                            yes: body_block,
                            no: exit,
                        });
                        self.current = body_block;
                        push(&mut tasks, StatementTask::AfterWhile(test, exit))?;
                        push(
                            &mut tasks,
                            StatementTask::Visit(
                                *body,
                                Some(self.loop_context(
                                    id,
                                    Loop {
                                        iterations: self.iterations,
                                        test: Some(test),
                                        exit,
                                        break_depth: self.handlers.len(),
                                        continue_depth: self.handlers.len(),
                                        break_finally: self.active_finalizers.len(),
                                        continue_finally: self.active_finalizers.len(),
                                    },
                                )),
                            ),
                        )?;
                    }
                    Syntax::Goto(name) => {
                        // Exits nested scopes, iterations and finalizers like break does.
                        let label = *self
                            .goto_labels
                            .get(name.as_str())
                            .ok_or(self.error(id, "missing goto label"))?;
                        let target = self.goto_target(label)?;
                        let (handler_depth, finally_depth, iteration_depth) = self
                            .goto_targets
                            .get(&label.0)
                            .and_then(|(_, depths)| *depths)
                            .ok_or(self.error(id, "goto target sequence does not enclose goto"))?;
                        self.route_action(
                            Action::Jump {
                                target,
                                handler_depth,
                                finally_depth,
                                iteration_depth,
                            },
                            None,
                            id,
                        )?;
                    }
                    Syntax::Break
                    | Syntax::Continue
                    | Syntax::NamedBreak(_)
                    | Syntax::NamedContinue(_) => {
                        let context = match &self.ast.nodes[id.0].syntax {
                            Syntax::NamedBreak(name) | Syntax::NamedContinue(name) => self.labels.iter().rev().find_map(|(label, target)| {
                                if matches!(&self.ast.nodes[label.0].syntax, Syntax::Label(candidate, _) if candidate == name) { *target } else { None }
                            }),
                            _ => context,
                        }.ok_or(self.error(id, "missing loop target"))?;
                        let is_break = matches!(
                            self.ast.nodes[id.0].syntax,
                            Syntax::Break | Syntax::NamedBreak(_)
                        );
                        let target = if is_break {
                            context.exit
                        } else {
                            context
                                .test
                                .ok_or(self.error(id, "missing continue target"))?
                        };
                        self.route_action(
                            Action::Jump {
                                iteration_depth: context.iterations,
                                target,
                                handler_depth: if is_break {
                                    context.break_depth
                                } else {
                                    context.continue_depth
                                },
                                finally_depth: if is_break {
                                    context.break_finally
                                } else {
                                    context.continue_finally
                                },
                            },
                            None,
                            id,
                        )?;
                    }
                    _ => return Err(self.error(id, "expected statement in scalar IR")),
                },
                StatementTask::AfterProtected(id, index, context) => {
                    self.finalizers[index].normal = self.current_reachable()?;
                    let (entry, handler, reason, packet) = {
                        let f = &self.finalizers[index];
                        (f.entry, f.handler, f.reason, f.packet)
                    };
                    self.handlers
                        .pop()
                        .ok_or(self.error(id, "missing finally handler"))?;
                    self.active_finalizers
                        .pop()
                        .ok_or(self.error(id, "missing finally scope"))?;
                    let normal = self.value(Operation::Constant(Scalar::Integer(0)), id)?;
                    self.effect(Operation::Store(reason, normal), id)?;
                    self.terminate(Terminator::Jump(entry));
                    self.current = handler.edge.target;
                    for (slot, kind) in packet.into_iter().zip([
                        sema::Builtin::PendingValue,
                        sema::Builtin::PendingCode,
                        sema::Builtin::PendingSite,
                        sema::Builtin::PendingOwner,
                    ]) {
                        let value = self.value(
                            Operation::Builtin {
                                kind,
                                arguments: Vec::new(),
                            },
                            id,
                        )?;
                        self.effect(Operation::Store(slot, value), id)?;
                    }
                    let exceptional = self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                    self.effect(Operation::Store(reason, exceptional), id)?;
                    self.terminate(Terminator::Jump(entry));
                    self.current = entry;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::UnwindScope,
                            arguments: vec![handler.checkpoint],
                        },
                        id,
                    )?;
                    let checkpoint = self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::BeginScope,
                            arguments: Vec::new(),
                        },
                        id,
                    )?;
                    let owner = self.value(Operation::Load(packet[3]), id)?;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::ClaimException,
                            arguments: vec![owner],
                        },
                        id,
                    )?;
                    let failure = self.block(id)?;
                    let slot = self.temp()?;
                    push(
                        &mut self.handlers,
                        Handler {
                            iterations: self.iterations,
                            edge: ExceptionEdge {
                                target: failure,
                                slot,
                            },
                            checkpoint,
                        },
                    )?;
                    push(&mut tasks, StatementTask::AfterFinally(id, index))?;
                    if let Syntax::Finally(_, finalizer) = self.ast.nodes[id.0].syntax {
                        push(&mut tasks, StatementTask::Visit(finalizer, context))?;
                    }
                }
                StatementTask::AfterFinally(id, index) => {
                    let cleanup = self
                        .handlers
                        .pop()
                        .ok_or(self.error(id, "missing finalizer cleanup scope"))?;
                    let completed = self.current;
                    self.current = cleanup.edge.target;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndScope,
                            arguments: vec![cleanup.checkpoint],
                        },
                        id,
                    )?;
                    self.terminate(Terminator::Resume(
                        self.handlers.last().map(|handler| handler.edge),
                    ));
                    self.current = completed;
                    let (end, reason, value, packet) = {
                        let f = &self.finalizers[index];
                        (f.end, f.reason, f.value, f.packet)
                    };
                    let reason = self.value(Operation::Load(reason), id)?;
                    let exceptional = self.value(Operation::Constant(Scalar::Integer(1)), id)?;
                    let matches =
                        self.value(Operation::Binary(Binary::Equal, reason, exceptional), id)?;
                    self.effect(Operation::LogicalGuard(matches), id)?;
                    let resume = self.block(id)?;
                    let next = self.block(id)?;
                    self.terminate(Terminator::Branch {
                        condition: matches,
                        mode: BranchMode::Logical,
                        yes: resume,
                        no: next,
                    });
                    self.current = resume;
                    let owner = self.value(Operation::Load(packet[3]), id)?;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::DetachException,
                            arguments: vec![owner],
                        },
                        id,
                    )?;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndScope,
                            arguments: vec![cleanup.checkpoint],
                        },
                        id,
                    )?;
                    let mut arguments = Vec::new();
                    for slot in packet {
                        let v = self.value(Operation::Load(slot), id)?;
                        push(&mut arguments, v)?;
                    }
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::RestorePending,
                            arguments,
                        },
                        id,
                    )?;
                    self.terminate(Terminator::Resume(self.handlers.last().map(|h| h.edge)));
                    self.current = next;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndScope,
                            arguments: vec![cleanup.checkpoint],
                        },
                        id,
                    )?;
                    for ordinal in 0..self.finalizers[index].actions.len() {
                        let action = self.finalizers[index].actions[ordinal];
                        let code = self.value(
                            Operation::Constant(Scalar::Integer(
                                i32::try_from(ordinal).map_err(|_| Diagnostic::resource(0))? + 3,
                            )),
                            id,
                        )?;
                        let matches =
                            self.value(Operation::Binary(Binary::Equal, reason, code), id)?;
                        self.effect(Operation::LogicalGuard(matches), id)?;
                        let target = self.block(id)?;
                        let next = self.block(id)?;
                        self.terminate(Terminator::Branch {
                            condition: matches,
                            mode: BranchMode::Logical,
                            yes: target,
                            no: next,
                        });
                        self.current = target;
                        let v = self.value(Operation::Load(value), id)?;
                        self.route_action(action, Some(v), id)?;
                        self.current = next;
                    }
                    self.terminate(Terminator::Jump(if self.finalizers[index].normal {
                        end
                    } else {
                        resume
                    }));
                    self.current = end;
                }
                StatementTask::AfterTry(id, handler, end, context) => {
                    self.handlers
                        .pop()
                        .ok_or(self.error(id, "missing protected scope"))?;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndScope,
                            arguments: vec![handler.checkpoint],
                        },
                        id,
                    )?;
                    self.terminate(Terminator::Jump(end));
                    self.current = handler.edge.target;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndScope,
                            arguments: vec![handler.checkpoint],
                        },
                        id,
                    )?;
                    let exception = self.value(Operation::Load(handler.edge.slot), id)?;
                    let Syntax::Try(_, catches) = &self.ast.nodes[id.0].syntax else {
                        unreachable!()
                    };
                    let mut bodies = Vec::new();
                    for (class, declaration, body) in catches {
                        let class = self.expression(*class)?;
                        let matches = self.value(
                            Operation::Builtin {
                                kind: sema::Builtin::OfKind,
                                arguments: vec![exception, class],
                            },
                            id,
                        )?;
                        self.effect(Operation::LogicalGuard(matches), id)?;
                        let target = self.block(id)?;
                        let next = self.block(id)?;
                        self.terminate(Terminator::Branch {
                            condition: matches,
                            mode: BranchMode::Logical,
                            yes: target,
                            no: next,
                        });
                        push(&mut bodies, (target, *declaration, *body))?;
                        self.current = next;
                    }
                    self.terminate(Terminator::Resume(
                        self.handlers.last().map(|handler| handler.edge),
                    ));
                    push(&mut tasks, StatementTask::Begin(end))?;
                    for (target, declaration, body) in bodies.into_iter().rev() {
                        push(&mut tasks, StatementTask::AfterCatch(declaration, end))?;
                        push(&mut tasks, StatementTask::Visit(body, context))?;
                        push(
                            &mut tasks,
                            StatementTask::CatchStart(target, declaration, exception),
                        )?;
                    }
                }
                StatementTask::AfterCatch(id, end) => {
                    let handler = self
                        .handlers
                        .pop()
                        .ok_or(self.error(id, "missing catch cleanup scope"))?;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndScope,
                            arguments: vec![handler.checkpoint],
                        },
                        id,
                    )?;
                    self.terminate(Terminator::Jump(end));
                    self.current = handler.edge.target;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndScope,
                            arguments: vec![handler.checkpoint],
                        },
                        id,
                    )?;
                    self.terminate(Terminator::Resume(
                        self.handlers.last().map(|handler| handler.edge),
                    ));
                }
                StatementTask::CatchStart(target, declaration, exception) => {
                    self.current = target;
                    let owner = self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::PendingOwner,
                            arguments: Vec::new(),
                        },
                        declaration,
                    )?;
                    let checkpoint = self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::BeginScope,
                            arguments: Vec::new(),
                        },
                        declaration,
                    )?;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::ClaimException,
                            arguments: vec![owner],
                        },
                        declaration,
                    )?;
                    let failure = self.block(declaration)?;
                    let slot = self.temp()?;
                    push(
                        &mut self.handlers,
                        Handler {
                            iterations: self.iterations,
                            edge: ExceptionEdge {
                                target: failure,
                                slot,
                            },
                            checkpoint,
                        },
                    )?;
                    let slot = *self
                        .declarations
                        .get(&(declaration.0, 0))
                        .ok_or(self.error(declaration, "missing catch local"))?;
                    self.effect(Operation::Reset(slot), declaration)?;
                    self.effect(Operation::Store(slot, exception), declaration)?;
                }
                StatementTask::EndIteration(id, test, exit) => {
                    self.terminate(Terminator::Jump(test));
                    self.current = exit;
                    self.value(
                        Operation::Builtin {
                            kind: sema::Builtin::EndIteration,
                            arguments: Vec::new(),
                        },
                        id,
                    )?;
                    self.iterations -= 1;
                }
                StatementTask::ForStart(id, condition, step, body) => {
                    let test = self.block(id)?;
                    let body_block = self.block(id)?;
                    let increment = self.block(id)?;
                    let exit = self.block(id)?;
                    self.terminate(Terminator::Jump(test));
                    self.current = test;
                    if let Some(condition) = condition {
                        let value = self.expression(condition)?;
                        self.effect(Operation::LogicalGuard(value), id)?;
                        self.terminate(Terminator::Branch {
                            condition: value,
                            mode: BranchMode::Logical,
                            yes: body_block,
                            no: exit,
                        });
                    } else {
                        self.terminate(Terminator::Jump(body_block));
                    }
                    self.current = body_block;
                    push(
                        &mut tasks,
                        StatementTask::ForEnd(id, step, increment, test, exit),
                    )?;
                    push(
                        &mut tasks,
                        StatementTask::Visit(
                            body,
                            Some(self.loop_context(
                                id,
                                Loop {
                                    iterations: self.iterations,
                                    test: Some(increment),
                                    exit,
                                    break_depth: self.handlers.len(),
                                    continue_depth: self.handlers.len(),
                                    break_finally: self.active_finalizers.len(),
                                    continue_finally: self.active_finalizers.len(),
                                },
                            )),
                        ),
                    )?;
                }
                StatementTask::ForEnd(_id, step, increment, test, exit) => {
                    self.terminate(Terminator::Jump(increment));
                    self.current = increment;
                    if let Some(step) = step {
                        self.expression(step)?;
                    }
                    self.terminate(Terminator::Jump(test));
                    self.current = exit;
                }
                StatementTask::Begin(block) => {
                    self.current = block;
                }
                StatementTask::AfterThen(id, no, context, no_block, join) => {
                    self.terminate(Terminator::Jump(join));
                    self.current = no_block;
                    push(&mut tasks, StatementTask::AfterElse(join))?;
                    if let Some(no) = no {
                        push(&mut tasks, StatementTask::Visit(no, context))?;
                    } else {
                        self.function.blocks[no_block.0].site = id;
                    }
                }
                StatementTask::AfterElse(join) => {
                    self.terminate(Terminator::Jump(join));
                    self.current = join;
                }
                StatementTask::AfterDo(id, condition, test, body, exit) => {
                    self.terminate(Terminator::Jump(test));
                    self.current = test;
                    let value = self.expression(condition)?;
                    self.effect(Operation::LogicalGuard(value), id)?;
                    self.terminate(Terminator::Branch {
                        condition: value,
                        mode: BranchMode::Logical,
                        yes: body,
                        no: exit,
                    });
                    self.current = exit;
                }
                StatementTask::AfterWhile(test, exit) => {
                    self.terminate(Terminator::Jump(test));
                    self.current = exit;
                }
            }
        }
        Ok(())
    }
}

pub fn lower(ast: &Ast) -> Result<Program, Diagnostic> {
    lower_with(ast, ast.lifetimes)
}

/// Lower with `+` polymorphic or not. Under lifetimes a `+` may concatenate and
/// the runtime decides by tag, which needs the wide tagged word. A profile that
/// has no text at all cannot concatenate, so it asks for the arithmetic `+`
/// instead and stays buildable under either memory model.
pub fn lower_with(ast: &Ast, polymorphic_plus: bool) -> Result<Program, Diagnostic> {
    let analysis = sema::analyze(ast)?;
    // a `+` initializing an owned local may build text, and so may the
    // additions nested inside it, as in `a + '-' + b`.
    let mut concatenations = Vec::new();
    concatenations
        .try_reserve_exact(ast.nodes.len())
        .map_err(|_| Diagnostic::resource(0))?;
    concatenations.resize(ast.nodes.len(), false);
    // with lifetimes there is no owning-initializer to key on, so `+`
    // is simply polymorphic and the runtime decides by tag.
    if polymorphic_plus {
        for (index, node) in ast.nodes.iter().enumerate() {
            if matches!(node.syntax, Syntax::Binary("+", _, _)) {
                concatenations[index] = true;
            }
        }
    }
    for node in &ast.nodes {
        let Syntax::OwnedCall(child) = node.syntax else {
            continue;
        };
        let mut pending = Vec::new();
        push(&mut pending, child)?;
        while let Some(id) = pending.pop() {
            if let Syntax::Binary("+", left, right) = ast.nodes[id.0].syntax {
                concatenations[id.0] = true;
                push(&mut pending, left)?;
                push(&mut pending, right)?;
            }
        }
    }
    let mut functions = Vec::new();
    for source in &ast.functions {
        let Syntax::Function {
            body, is_static, ..
        } = ast.nodes[source.0].syntax
        else {
            unreachable!()
        };
        let mut builder = Builder {
            iterations: 0,
            handlers: Vec::new(),
            finalizers: Vec::new(),
            active_finalizers: Vec::new(),
            labels: Vec::new(),
            goto_labels: HashMap::new(),
            goto_targets: HashMap::new(),
            ast,
            analysis: &analysis,
            concatenations: &concatenations,
            function: Function {
                source: *source,
                blocks: Vec::new(),
                slots: Vec::new(),
                values: 0,
            },
            current: BlockId(0),
            locals: HashMap::new(),
            declarations: HashMap::new(),
        };
        for (index, local) in analysis
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.function == *source)
        {
            let slot = Slot(builder.function.slots.len());
            push(
                &mut builder.function.slots,
                LocalSlot {
                    declaration: Some((local.declaration, local.ordinal)),
                    parameter: local.parameter,
                },
            )?;
            builder
                .locals
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(0))?;
            builder.locals.insert(index, slot);
            builder
                .declarations
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(0))?;
            builder
                .declarations
                .insert((local.declaration.0, local.ordinal), slot);
        }
        builder.block(*source)?;
        if is_static {
            let Syntax::Return(Some(assign)) = ast.nodes[body.0].syntax else {
                return Err(builder.error(body, "invalid static body"));
            };
            let property = match ast.nodes[assign.0].syntax {
                Syntax::Binary("=", target, _) => analysis.properties[target.0]
                    .ok_or(builder.error(target, "missing static property"))?,
                Syntax::StaticVector { property, .. }
                | Syntax::StaticRexPattern { property, .. }
                | Syntax::StaticLookup { property, .. }
                | Syntax::Construction { property, .. } => match analysis.bindings[property.0] {
                    Some(Binding::Property(property)) => property,
                    _ => {
                        return Err(
                            builder.error(property, "missing construction recipient property")
                        );
                    }
                },
                _ => return Err(builder.error(body, "invalid static assignment")),
            };
            let receiver = builder.value(Operation::Load(Slot(0)), *source)?;
            builder.effect(Operation::BeginStatic(receiver, property), *source)?;
        }
        builder.statements(body)?;
        push(&mut functions, builder.function)?;
    }
    Ok(Program { functions })
}
