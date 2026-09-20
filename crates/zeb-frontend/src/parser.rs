//! Flat syntax trees and explicit parse worklists; no input-proportional recursion.
#[path = "grammar_syntax.rs"]
mod grammar_syntax;
#[path = "syntax_order.rs"]
mod syntax_order;
#[path = "vocabulary.rs"]
mod vocabulary;
pub use grammar_syntax::{GrammarItem, GrammarRule};

use crate::{
    Diagnostic,
    lexer::{self, Kind, Token},
    source::Source,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Id(pub usize);

#[derive(Debug)]
pub enum Syntax {
    Declaration(Declaration),
    Template(crate::templates::Template),
    Name(String),
    PropertyAddress(String),
    FunctionReference(Id),
    Membership {
        value: Id,
        candidates: Vec<Id>,
        negated: bool,
    },
    LocalConstruction {
        prototype: Id,
        arguments: Vec<Id>,
    },
    ExceptionConstruction {
        prototype: Id,
        arguments: Vec<Id>,
    },
    PublishedConstruction {
        prototype: Id,
        arguments: Vec<Id>,
    },
    Construction {
        prototype: Id,
        arguments: Vec<Id>,
        owner: Id,
        property: Id,
    },
    List(Vec<Id>),
    Index(Id, Id),
    Integer(String, u32),
    Decimal(String),
    String(crate::strings::Literal),
    Interpolation {
        emitting: bool,
        parts: Vec<Id>,
    },
    Nil,
    True,
    Group(Id),
    Unary(&'static str, Id, bool),
    Binary(&'static str, Id, Id),
    Conditional(Id, Id, Id),
    Call(Id, Vec<Id>),
    Property(Id, String),
    IndirectProperty(Id, Id),
    Object {
        name: String,
        is_class: bool,
        transient: bool,
        is_dictionary: bool,
        parents: Vec<String>,
        properties: Vec<(String, Id)>,
    },
    Block(Vec<Id>),
    Empty,
    Expression(Id),
    Local(Vec<(String, Option<Id>)>),
    ForEach(Id, Id, Id),
    Return(Option<Id>),
    Throw(Id),
    Try(Id, Vec<(Id, Id, Id)>),
    Finally(Id, Id),
    Label(String, Id),
    Apply(Id, Id),
    ArgumentPack(Vec<Id>),
    /// An expanded list followed by fixed arguments: `f(list..., a, b)`.
    ArgumentPackTail(Vec<Id>),
    ExpandedArgument(Id),
    QualifiedInherited(String),
    /// `delegated target`: the currently executing property as another object
    /// defines it, with `self` unchanged.
    Delegated(Id),
    NamedBreak(String),
    NamedContinue(String),
    /// `goto label;` within the enclosing-sequence profile.
    Goto(String),
    OwnerScope(Id),
    LocalVector(Id),
    StaticLookup {
        owner: Id,
        property: Id,
    },
    /// `static new RexPattern(text)` in a field: a compiled pattern object.
    StaticRexPattern {
        text: Id,
        owner: Id,
        property: Id,
    },
    StaticVector {
        capacity: Id,
        owner: Id,
        property: Id,
    },
    LocalStringBuffer,
    LocalOwnedCollection,
    LocalLookupSized(Id, Id),
    Lookup(Vec<(Option<Id>, Id)>),
    Move(Id),
    OwnedCall(Id),
    OwnedLocal(Vec<(String, Option<Id>)>),
    If(Id, Id, Option<Id>),
    While(Id, Id),
    DoWhile(Id, Id),
    For {
        init: Vec<Id>,
        condition: Option<Id>,
        step: Option<Id>,
        body: Id,
    },
    Switch(Id, Vec<(Option<Id>, Vec<Id>)>),
    Break,
    Continue,
    Function {
        returns_owned: bool,
        optional: usize,
        rest: bool,
        name: String,
        is_static: bool,
        parameters: Vec<String>,
        body: Id,
    },
}

/// Header contracts are metadata, never executable stand-ins for intrinsics.
#[derive(Debug)]
pub enum Declaration {
    Grammar(GrammarRule),
    /// `relation contains(container, item) one_to_many reverse location;`.
    /// The index is the relation's place in declaration order.
    Relation(usize),
    ExternalFunction(String, usize),
    Intrinsic {
        class: Option<String>,
        version: String,
        parent: Option<String>,
        signatures: Vec<Signature>,
    },
    Enumerators {
        names: Vec<String>,
        token: bool,
        family: Option<String>,
    },
    Properties(Vec<String>),
    VocabularyProperties(Vec<String>),
    Export {
        name: String,
        external: String,
    },
}
#[derive(Debug)]
pub struct Signature {
    pub name: String,
    pub is_static: bool,
    pub parameters: Vec<(String, bool)>,
    pub variadic: bool,
}

#[derive(Debug)]
pub struct Node {
    pub syntax: Syntax,
    pub start: usize,
    pub end: usize,
}

/// A relation row stated in a declaration: which relation, and the
/// label when it has a third column. The partner is the property's own value, so
/// the ordinary initializer path resolves it.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredRow {
    pub relation: usize,
    /// Whether the reverse name was the one written: `location(study)` reads the
    /// containment table right to left, so the row goes in that way round.
    pub reversed: bool,
    pub label: Option<Id>,
}

#[derive(Debug)]
pub struct Ast {
    pub nodes: Vec<Node>,
    pub functions: Vec<Id>,
    pub objects: Vec<Id>,
    /// Relations in declaration order; a call site names one by its index.
    pub relations: Vec<RelationDecl>,
    /// Per node: set on the value of an object property written as a relation row,
    /// `desk: Thing location(study)`, rather than as a field.
    pub declared_rows: Vec<Option<DeclaredRow>>,
    /// Whether this compilation uses World/Turn lifetimes instead of
    /// scope ownership. Selected by the build, not by the source.
    pub lifetimes: bool,
}

/// A relation and the two names that read it. The forward name
/// reads left to right, the reverse name right to left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationDecl {
    pub forward: String,
    pub reverse: Option<String>,
    /// 0 one_to_one, 1 one_to_many, 2 many_to_many.
    pub cardinality: u64,
    /// Set when the right side is annotated `Text`: the pairs live in the
    /// dictionary rather than in a relation table, and the forward name is a
    /// vocabulary property.
    pub vocabulary: bool,
    /// Set when the relation has a third column naming which of a family of
    /// tables a row belongs to, such as a direction.
    pub labelled: bool,
    pub label_family: Option<String>,
    /// For a vocabulary relation, the dictionary in force where it was declared:
    /// its rows live there, so that is the one it reads and writes.
    /// An object's vocabulary properties already follow this rule.
    pub dictionary: Option<String>,
}

fn push<T>(items: &mut Vec<T>, item: T, byte: usize) -> Result<(), Diagnostic> {
    items
        .try_reserve(1)
        .map_err(|_| Diagnostic::resource(byte))?;
    items.push(item);
    Ok(())
}

enum ExprTask {
    LookupKey(usize, Vec<(Option<Id>, Id)>, u8, bool),
    LookupValue(usize, Vec<(Option<Id>, Id)>, Option<Id>, u8, bool),
    Membership(Id, Vec<Id>, bool, u8, bool),
    Anonymous(usize, Vec<String>, u8, bool),
    Published(usize, u8, bool),
    List(usize, Vec<Id>, u8, bool),
    Index(Id, u8, bool),
    Indirect(Id, u8, bool),
    Interpolation(usize, bool, Vec<Id>, u8, bool),
    Parse(u8, bool),
    Rest(Id, u8, bool),
    Prefix(&'static str, usize, u8, bool),
    Group(usize, u8, bool),
    Binary(&'static str, Id, u8, bool),
    CallNext(Id, Vec<Id>, u8, bool),
    CallArg(Id, Vec<Id>, u8, bool),
    TailNext(Id, Vec<Id>, u8, bool),
    TailArg(Id, Vec<Id>, u8, bool),
    Middle(Id, u8, bool),
    Conditional(Id, Id, u8, bool),
}

enum StatementTask {
    Label(usize, String),
    FinallyBody(usize, Id),
    TryBody(usize),
    TryNext(usize, Id, Vec<(Id, Id, Id)>),
    CatchBody(usize, Id, Vec<(Id, Id, Id)>, Id, Id),
    ForEach(usize, Id, Id, Option<Id>),
    Switch(usize, Id, Vec<(Option<Id>, Vec<Id>)>),
    SwitchChild(usize, Id, Vec<(Option<Id>, Vec<Id>)>),
    Parse,
    /// Start byte, the anonymous-callback count on entry, and the statements.
    Block(usize, (usize, usize), Vec<Id>),
    BlockChild(usize, (usize, usize), Vec<Id>),
    Then(usize, Id),
    Else(usize, Id, Id),
    While(usize, Id),
    DoWhile(usize),
    For(usize, Vec<Id>, Option<Id>, Option<Id>),
}

struct Parser<'a> {
    /// nodes built from a value string's interpolation.
    interpolated: Vec<Id>,
    source: &'a Source,
    tokens: Vec<Token>,
    cursor: usize,
    nodes: Vec<Node>,
    anonymous: Vec<Id>,
    /// Calls that build text at run time; their result belongs to a scope.
    text_builders: usize,
    /// Set by the selected memory model: under lifetimes a block owns no scope,
    /// because a value's class says when it goes.
    lifetimes: bool,
    deferred_bodies: Vec<(Id, usize, usize)>,
    braces: std::collections::HashMap<usize, usize>,
}

impl Parser<'_> {
    /// `'a<<x>>b'` as `'a' + $valueText(x) + 'b'`.
    ///
    /// The parts alternate literal text and expressions, starting and ending
    /// with text. Empty literals are dropped, because joining them would cost a
    /// concatenation that says nothing.
    fn joined_interpolation(
        &mut self,
        parts: &[Id],
        start: usize,
        end: usize,
    ) -> Result<Id, Diagnostic> {
        let joined = self.joined_interpolation_parts(parts, start, end)?;
        // Remembered so that an object property holding one becomes a method:
        // a value string with an expression in it is evaluated when it is read,
        // which is what TADS does with it too.
        self.interpolated
            .try_reserve(1)
            .map_err(|_| Diagnostic::resource(start))?;
        self.interpolated.push(joined);
        Ok(joined)
    }

    fn joined_interpolation_parts(
        &mut self,
        parts: &[Id],
        start: usize,
        end: usize,
    ) -> Result<Id, Diagnostic> {
        let mut joined: Option<Id> = None;
        for (index, part) in parts.iter().copied().enumerate() {
            let piece = if index % 2 == 0 {
                if matches!(&self.nodes[part.0].syntax, Syntax::String(l) if l.text.is_empty()) {
                    continue;
                }
                part
            } else {
                // Whatever it is, as text. `toString` converts a number only.
                let name = self.node(Syntax::Name("$valueText".to_owned()), start, end)?;
                let mut arguments = Vec::new();
                push(&mut arguments, part, start)?;
                self.node(Syntax::Call(name, arguments), start, end)?
            };
            joined = Some(match joined {
                None => piece,
                Some(left) => self.node(Syntax::Binary("+", left, piece), start, end)?,
            });
        }
        match joined {
            Some(id) => Ok(id),
            // `''` with nothing in it at all: the empty text.
            None => {
                let literal = crate::strings::Literal {
                    text: String::new(),
                    emitting: false,
                    triple: false,
                };
                self.node(Syntax::String(literal), start, end)
            }
        }
    }

    fn token(&self) -> Token {
        self.tokens[self.cursor]
    }
    fn byte(&self) -> usize {
        self.token().byte_start
    }
    fn error(&self, message: &'static str) -> Diagnostic {
        Diagnostic::new("parse-expected", message, self.byte())
    }
    fn symbol(&self, symbol: &str) -> bool {
        matches!(self.token().kind, Kind::Symbol(value) if value == symbol)
    }
    fn take(&mut self, symbol: &str) -> bool {
        if self.symbol(symbol) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, symbol: &str) -> Result<(), Diagnostic> {
        if self.take(symbol) {
            Ok(())
        } else {
            Err(self.error("missing expected punctuation"))
        }
    }
    fn word(&self, word: &str) -> bool {
        self.token().kind == Kind::Identifier && self.token().spelling(self.source).eq(word.chars())
    }
    fn take_word(&mut self, word: &str) -> bool {
        if self.word(word) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    /// `owned` is contextual: TADS sources may also use it as a variable name,
    /// so it only introduces an owned declaration when a name follows it.
    /// Note a call that builds text at run time, so its block opens a scope.
    fn note_text_builder(&mut self, callee: Id) {
        let builds = match &self.nodes[callee.0].syntax {
            Syntax::Name(name) => matches!(name.as_str(), "toString" | "rexReplace"),
            Syntax::Property(_, name) => {
                matches!(name.as_str(), "substr" | "toString" | "rexReplace")
            }
            _ => false,
        };
        if builds {
            self.text_builders += 1;
        }
    }

    /// `[for NAME in COLLECTION : EXPRESSION]`, with `[` already taken. The
    /// list a query derives is the shape a relation-based world model asks for
    /// constantly, and writing it as an accumulator loop is worse in every
    /// reading. It lowers to the existing `mapAll` path.
    fn comprehension(&mut self, start: usize) -> Result<Id, Diagnostic> {
        let form = "comprehension reads [for NAME in COLLECTION : EXPRESSION]";
        if !self.take_word("for") {
            return Err(self.error(form));
        }
        let name = self.name()?;
        if !self.take_word("in") {
            return Err(self.error(form));
        }
        let collection = self.expression(2, false)?;
        self.expect(":")?;
        let value = self.expression(2, false)?;
        // An optional filter keeps both halves of a query in one place
        // `[for x in c : f(x) if p(x)]` selects, then maps.
        let condition = if self.take_word("if") {
            Some(self.expression(2, false)?)
        } else {
            None
        };
        self.expect("]")?;
        let end = self.consumed_end();
        let mut source = collection;
        if let Some(condition) = condition {
            let keep = self.callback(&name, condition, start, end)?;
            let subset = self.node(Syntax::Property(source, "subset".to_owned()), start, end)?;
            let mut arguments = Vec::new();
            push(&mut arguments, keep, start)?;
            source = self.node(Syntax::Call(subset, arguments), start, end)?;
        }
        let map = self.callback(&name, value, start, end)?;
        let access = self.node(Syntax::Property(source, "mapAll".to_owned()), start, end)?;
        let mut arguments = Vec::new();
        push(&mut arguments, map, start)?;
        self.node(Syntax::Call(access, arguments), start, end)
    }

    /// One anonymous `{ name: expression }` callback, as a value.
    fn callback(
        &mut self,
        name: &str,
        value: Id,
        start: usize,
        end: usize,
    ) -> Result<Id, Diagnostic> {
        let body = self.node(Syntax::Return(Some(value)), start, end)?;
        let mut parameters = Vec::new();
        push(&mut parameters, name.to_owned(), start)?;
        let function = self.node(
            Syntax::Function {
                returns_owned: false,
                optional: 0,
                rest: false,
                name: format!("$callback{}", self.anonymous.len()),
                is_static: false,
                parameters,
                body,
            },
            start,
            end,
        )?;
        push(&mut self.anonymous, function, start)?;
        self.node(Syntax::FunctionReference(function), start, end)
    }

    /// One of the intrinsic collection constructors, with `new` already taken.
    /// `None` means the prototype names something else and nothing was consumed
    /// beyond `new`, so the caller may resume where it left off.
    fn intrinsic_collection(&mut self, start: usize) -> Result<Option<Id>, Diagnostic> {
        let syntax = if self.take_word("OwnedCollection") {
            self.expect("(")?;
            self.expect(")")?;
            Syntax::LocalOwnedCollection
        } else if self.take_word("StringBuffer") {
            self.expect("(")?;
            self.expect(")")?;
            Syntax::LocalStringBuffer
        } else if self.take_word("LookupTable") {
            self.expect("(")?;
            if self.take(")") {
                Syntax::Lookup(Vec::new())
            } else {
                let buckets = self.expression(2, false)?;
                self.expect(",")?;
                let capacity = self.expression(2, false)?;
                self.expect(")")?;
                Syntax::LocalLookupSized(buckets, capacity)
            }
        } else if self.take_word("Vector") {
            self.expect("(")?;
            let capacity = self.expression(2, false)?;
            self.expect(")")?;
            Syntax::LocalVector(capacity)
        } else {
            return Ok(None);
        };
        Ok(Some(self.node(syntax, start, self.consumed_end())?))
    }

    fn take_word_before_name(&mut self, word: &str) -> bool {
        if self.word(word)
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|token| token.kind == Kind::Identifier)
        {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn declaration_string(&mut self) -> Result<String, Diagnostic> {
        if !matches!(
            self.token().kind,
            Kind::Quoted {
                emitting: false,
                ..
            }
        ) {
            return Err(self.error("expected a constant single-quoted declaration string"));
        }
        let value = self.token().literal(self.source)?.text;
        self.cursor += 1;
        Ok(value)
    }
    fn intrinsic(&mut self) -> Result<Declaration, Diagnostic> {
        let class = if self.take_word("class") {
            Some(self.name()?)
        } else {
            None
        };
        let version = self.declaration_string()?;
        let parent = if class.is_some() && self.take(":") {
            Some(self.name()?)
        } else {
            None
        };
        self.expect("{")?;
        let mut signatures = Vec::new();
        while !self.take("}") {
            let is_static = self.take_word("static");
            if is_static && class.is_none() {
                return Err(self.error("static requires an intrinsic class"));
            }
            let name = self.name()?;
            if signatures.iter().any(|s: &Signature| s.name == name) {
                return Err(self.error("duplicate intrinsic signature"));
            }
            self.expect("(")?;
            let mut parameters = Vec::new();
            let mut optional = false;
            let mut variadic = false;
            if !self.take(")") {
                loop {
                    if self.take("...") {
                        variadic = true;
                        self.expect(")")?;
                        break;
                    }
                    let name = self.name()?;
                    let is_optional = self.take("?");
                    if optional && !is_optional {
                        return Err(self.error("required parameter follows optional parameter"));
                    }
                    if parameters.iter().any(|(p, _)| p == &name) {
                        return Err(self.error("duplicate intrinsic parameter"));
                    }
                    optional |= is_optional;
                    push(&mut parameters, (name, is_optional), self.byte())?;
                    if self.take(")") {
                        break;
                    }
                    self.expect(",")?;
                }
            }
            self.expect(";")?;
            push(
                &mut signatures,
                Signature {
                    name,
                    is_static,
                    parameters,
                    variadic,
                },
                self.byte(),
            )?;
        }
        Ok(Declaration::Intrinsic {
            class,
            version,
            parent,
            signatures,
        })
    }
    fn text(&self, token: Token) -> Result<String, Diagnostic> {
        let mut result = String::new();
        result
            .try_reserve(token.byte_end - token.byte_start)
            .map_err(|_| Diagnostic::resource(token.byte_start))?;
        for ch in token.spelling(self.source) {
            result.push(ch);
        }
        Ok(result)
    }
    fn name(&mut self) -> Result<String, Diagnostic> {
        if self.token().kind != Kind::Identifier {
            return Err(self.error("expected an identifier"));
        }
        let text = self.text(self.token())?;
        if matches!(
            text.as_str(),
            "nil"
                | "true"
                | "return"
                | "local"
                | "if"
                | "else"
                | "while"
                | "do"
                | "break"
                | "continue"
                | "function"
        ) {
            return Err(self.error("keyword cannot be used as a name"));
        }
        self.cursor += 1;
        Ok(text)
    }
    fn consumed_end(&self) -> usize {
        self.tokens[self.cursor.saturating_sub(1)].byte_end
    }
    fn node(&mut self, syntax: Syntax, start: usize, end: usize) -> Result<Id, Diagnostic> {
        let id = Id(self.nodes.len());
        push(&mut self.nodes, Node { syntax, start, end }, start)?;
        Ok(id)
    }
    fn result(&self, value: &mut Option<Id>) -> Result<Id, Diagnostic> {
        value.take().ok_or(Diagnostic::new(
            "parser-internal",
            "missing parse result",
            self.byte(),
        ))
    }

    fn emitting_property(
        &mut self,
        object: &str,
        property: &str,
        value: Id,
        functions: &mut Vec<Id>,
    ) -> Result<Id, Diagnostic> {
        // `p = (expr)` is a method that evaluates expr on each access, and an
        // emitting string property is a method that prints it.
        let computed = matches!(self.nodes[value.0].syntax, Syntax::Group(_));
        if !computed
            && !matches!(&self.nodes[value.0].syntax,Syntax::String(literal) if literal.emitting)
            && !matches!(
                self.nodes[value.0].syntax,
                Syntax::Interpolation { emitting: true, .. }
            )
        {
            return Ok(value);
        }
        let start = self.nodes[value.0].start;
        let end = self.nodes[value.0].end;
        let body = if computed {
            let Syntax::Group(inner) = self.nodes[value.0].syntax else {
                unreachable!()
            };
            let ret = self.node(Syntax::Return(Some(inner)), start, end)?;
            let mut statements = Vec::new();
            push(&mut statements, ret, start)?;
            self.node(Syntax::Block(statements), start, end)?
        } else {
            let statement = self.node(Syntax::Expression(value), start, end)?;
            let ret = self.node(Syntax::Return(None), start, end)?;
            let mut statements = Vec::new();
            push(&mut statements, statement, start)?;
            push(&mut statements, ret, start)?;
            self.node(Syntax::Block(statements), start, end)?
        };
        let mut parameters = Vec::new();
        push(&mut parameters, "self".to_owned(), start)?;
        let function = self.node(
            Syntax::Function {
                returns_owned: false,
                optional: 0,
                rest: false,
                name: format!("${object}.{property}"),
                is_static: false,
                parameters,
                body,
            },
            start,
            end,
        )?;
        push(functions, function, start)?;
        Ok(function)
    }
    fn expression(&mut self, min: u8, commas: bool) -> Result<Id, Diagnostic> {
        let mut tasks = Vec::new();
        push(&mut tasks, ExprTask::Parse(min, commas), self.byte())?;
        let mut value = None;
        while let Some(task) = tasks.pop() {
            match task {
                ExprTask::Parse(min, commas) => {
                    let token = self.token();
                    let start = self.byte();
                    if let Kind::StringPart {
                        emitting,
                        head: true,
                        ..
                    } = token.kind
                    {
                        let mut literal = token.literal(self.source)?;
                        literal.emitting = false;
                        self.cursor += 1;
                        let first = self.node(Syntax::String(literal), start, token.byte_end)?;
                        let mut parts = Vec::new();
                        push(&mut parts, first, start)?;
                        push(
                            &mut tasks,
                            ExprTask::Interpolation(start, emitting, parts, min, commas),
                            start,
                        )?;
                        push(&mut tasks, ExprTask::Parse(1, true), start)?;
                        continue;
                    }
                    if self.take("[") {
                        // [for x in c : e] builds a list from a query.
                        // It desugars to c.mapAll({x: e}).
                        if self.lifetimes && self.word("for") {
                            let id = self.comprehension(start)?;
                            push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                            continue;
                        }
                        if self.take("*") {
                            self.expect("->")?;
                            push(
                                &mut tasks,
                                ExprTask::LookupValue(start, Vec::new(), None, min, commas),
                                start,
                            )?;
                            push(&mut tasks, ExprTask::Parse(2, false), start)?;
                        } else if self.take("]") {
                            let id =
                                self.node(Syntax::List(Vec::new()), start, self.consumed_end())?;
                            push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                        } else {
                            push(
                                &mut tasks,
                                ExprTask::List(start, Vec::new(), min, commas),
                                start,
                            )?;
                            push(&mut tasks, ExprTask::Parse(2, false), start)?;
                        }
                        continue;
                    }
                    if self.take_word("function") {
                        let (parameters, optional, rest) = self.formal_parameters(false)?;
                        let open = self.cursor;
                        if !self.symbol("{") {
                            return Err(self.error("anonymous function requires a braced body"));
                        }
                        let close =
                            self.braces.get(&open).copied().ok_or_else(|| {
                                self.error("unterminated anonymous function body")
                            })?;
                        self.cursor = close + 1;
                        let end = self.consumed_end();
                        let body = self.node(Syntax::Block(Vec::new()), start, end)?;
                        let function = self.node(
                            Syntax::Function {
                                returns_owned: false,
                                optional,
                                rest,
                                name: format!("$callback{}", self.anonymous.len()),
                                is_static: false,
                                parameters,
                                body,
                            },
                            start,
                            end,
                        )?;
                        push(&mut self.anonymous, function, start)?;
                        push(
                            &mut self.deferred_bodies,
                            (function, open, close + 1),
                            start,
                        )?;
                        let reference =
                            self.node(Syntax::FunctionReference(function), start, end)?;
                        push(&mut tasks, ExprTask::Rest(reference, min, commas), start)?;
                        continue;
                    }
                    if self.take("{") {
                        let mut parameters = Vec::new();
                        if !self.take(":") {
                            loop {
                                let name = self.name()?;
                                push(&mut parameters, name, start)?;
                                if self.take(":") {
                                    break;
                                }
                                self.expect(",")?;
                            }
                        }
                        push(
                            &mut tasks,
                            ExprTask::Anonymous(start, parameters, min, commas),
                            start,
                        )?;
                        push(&mut tasks, ExprTask::Parse(1, true), start)?;
                        continue;
                    }
                    if self.take_word("new") {
                        // With the two lifetimes there is no
                        // publication question: a value built here belongs to
                        // the turn until something in world state keeps it.
                        // Under scope ownership the form is still required.
                        if !self.take_word("published") && !self.lifetimes {
                            return Err(self.error("new requires an implemented ownership form: new published or an owned static field"));
                        }
                        push(&mut tasks, ExprTask::Published(start, min, commas), start)?;
                        push(&mut tasks, ExprTask::Parse(15, false), start)?;
                        continue;
                    }
                    if self.take("(") {
                        push(&mut tasks, ExprTask::Group(start, min, commas), start)?;
                        push(&mut tasks, ExprTask::Parse(1, true), start)?;
                        continue;
                    }
                    if self.take("&") {
                        let name = self.name()?;
                        let id =
                            self.node(Syntax::PropertyAddress(name), start, self.consumed_end())?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                        continue;
                    }
                    if let Kind::Symbol(op @ ("+" | "-" | "!" | "~" | "++" | "--")) = token.kind {
                        self.cursor += 1;
                        push(&mut tasks, ExprTask::Prefix(op, start, min, commas), start)?;
                        push(&mut tasks, ExprTask::Parse(15, commas), start)?;
                        continue;
                    }
                    let syntax = match token.kind {
                        Kind::Integer(radix) => Syntax::Integer(self.text(token)?, radix),
                        Kind::BigNumber => Syntax::Decimal(self.text(token)?),
                        Kind::Quoted { .. } => Syntax::String(token.literal(self.source)?),
                        Kind::Identifier if self.word("nil") => Syntax::Nil,
                        Kind::Identifier if self.word("true") => Syntax::True,
                        Kind::Identifier => {
                            let name = self.name()?;
                            let syntax = if name == "inherited"
                                && matches!(self.token().kind, Kind::Identifier)
                            {
                                Syntax::QualifiedInherited(self.name()?)
                            } else if name == "delegated"
                                && matches!(self.token().kind, Kind::Identifier)
                            {
                                let target = self.name()?;
                                let end = self.consumed_end();
                                Syntax::Delegated(self.node(Syntax::Name(target), start, end)?)
                            } else {
                                Syntax::Name(name)
                            };
                            let id = self.node(syntax, start, self.consumed_end())?;
                            push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                            continue;
                        }
                        _ => return Err(self.error("expected an expression")),
                    };
                    self.cursor += 1;
                    let id = self.node(syntax, start, token.byte_end)?;
                    push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                }
                ExprTask::Interpolation(start, emitting, mut parts, min, commas) => {
                    let expr = self.result(&mut value)?;
                    push(&mut parts, expr, start)?;
                    let token = self.token();
                    let Kind::StringPart {
                        head: false, last, ..
                    } = token.kind
                    else {
                        return Err(self.error("expected end of embedded expression"));
                    };
                    let mut literal = token.literal(self.source)?;
                    literal.emitting = false;
                    self.cursor += 1;
                    let part =
                        self.node(Syntax::String(literal), token.byte_start, token.byte_end)?;
                    push(&mut parts, part, start)?;
                    if last {
                        let id = if emitting {
                            self.node(
                                Syntax::Interpolation { emitting, parts },
                                start,
                                token.byte_end,
                            )?
                        } else {
                            // a value string's interpolation is
                            // concatenation. Written out as one here rather than
                            // lowered specially, so the ownership rules, the
                            // text building and the two memory models all apply
                            // to it exactly as they do to a `+` an author wrote.
                            self.joined_interpolation(&parts, start, token.byte_end)?
                        };
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                    } else {
                        push(
                            &mut tasks,
                            ExprTask::Interpolation(start, emitting, parts, min, commas),
                            start,
                        )?;
                        push(&mut tasks, ExprTask::Parse(1, true), start)?;
                    }
                }
                ExprTask::List(start, mut items, min, commas) => {
                    let item = self.result(&mut value)?;
                    if items.is_empty() && self.take("->") {
                        push(
                            &mut tasks,
                            ExprTask::LookupValue(start, Vec::new(), Some(item), min, commas),
                            start,
                        )?;
                        push(&mut tasks, ExprTask::Parse(2, false), start)?;
                        continue;
                    }
                    push(&mut items, item, start)?;
                    if self.take("]") {
                        let id = self.node(Syntax::List(items), start, self.consumed_end())?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                    } else {
                        self.expect(",")?;
                        push(&mut tasks, ExprTask::List(start, items, min, commas), start)?;
                        push(&mut tasks, ExprTask::Parse(2, false), start)?;
                    }
                }
                ExprTask::LookupKey(start, entries, min, commas) => {
                    let key = self.result(&mut value)?;
                    self.expect("->")?;
                    push(
                        &mut tasks,
                        ExprTask::LookupValue(start, entries, Some(key), min, commas),
                        start,
                    )?;
                    push(&mut tasks, ExprTask::Parse(2, false), start)?;
                }
                ExprTask::LookupValue(start, mut entries, key, min, commas) => {
                    let entry = self.result(&mut value)?;
                    if key.is_none() && entries.iter().any(|(key, _)| key.is_none()) {
                        return Err(self.error("duplicate lookup default"));
                    }
                    push(&mut entries, (key, entry), start)?;
                    if self.take("]") {
                        let id = self.node(Syntax::Lookup(entries), start, self.consumed_end())?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                    } else {
                        self.expect(",")?;
                        if self.take("*") {
                            self.expect("->")?;
                            push(
                                &mut tasks,
                                ExprTask::LookupValue(start, entries, None, min, commas),
                                start,
                            )?;
                        } else {
                            push(
                                &mut tasks,
                                ExprTask::LookupKey(start, entries, min, commas),
                                start,
                            )?;
                        }
                        push(&mut tasks, ExprTask::Parse(2, false), start)?;
                    }
                }
                ExprTask::Index(receiver, min, commas) => {
                    let index = self.result(&mut value)?;
                    self.expect("]")?;
                    let start = self.nodes[receiver.0].start;
                    let id =
                        self.node(Syntax::Index(receiver, index), start, self.consumed_end())?;
                    push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                }
                ExprTask::Indirect(receiver, min, commas) => {
                    let key = self.result(&mut value)?;
                    self.expect(")")?;
                    let start = self.nodes[receiver.0].start;
                    let id = self.node(
                        Syntax::IndirectProperty(receiver, key),
                        start,
                        self.consumed_end(),
                    )?;
                    push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                }
                ExprTask::Membership(left, mut candidates, negated, min, commas) => {
                    let candidate = self.result(&mut value)?;
                    let start = self.nodes[left.0].start;
                    push(&mut candidates, candidate, start)?;
                    if self.take(")") {
                        let id = self.node(
                            Syntax::Membership {
                                value: left,
                                candidates,
                                negated,
                            },
                            start,
                            self.consumed_end(),
                        )?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                    } else {
                        self.expect(",")?;
                        push(
                            &mut tasks,
                            ExprTask::Membership(left, candidates, negated, min, commas),
                            start,
                        )?;
                        push(&mut tasks, ExprTask::Parse(2, false), start)?;
                    }
                }
                ExprTask::Rest(left, min, commas) => {
                    let start = self.nodes[left.0].start;
                    if self.take("[") {
                        push(&mut tasks, ExprTask::Index(left, min, commas), start)?;
                        push(&mut tasks, ExprTask::Parse(1, true), start)?;
                        continue;
                    }
                    if self.take(".") {
                        if self.take("(") {
                            push(&mut tasks, ExprTask::Indirect(left, min, commas), start)?;
                            push(&mut tasks, ExprTask::Parse(1, true), start)?;
                            continue;
                        }
                        let name = self.name()?;
                        let id =
                            self.node(Syntax::Property(left, name), start, self.consumed_end())?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                        continue;
                    }
                    if self.take("(") {
                        if self.take(")") {
                            let id = self.node(
                                Syntax::Call(left, Vec::new()),
                                start,
                                self.consumed_end(),
                            )?;
                            push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                        } else {
                            push(
                                &mut tasks,
                                ExprTask::CallNext(left, Vec::new(), min, commas),
                                start,
                            )?;
                        }
                        continue;
                    }
                    if let Kind::Symbol(op @ ("++" | "--")) = self.token().kind {
                        self.cursor += 1;
                        let id =
                            self.node(Syntax::Unary(op, left, true), start, self.consumed_end())?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                        continue;
                    }
                    if min <= 10 && (self.word("is") || self.word("not")) {
                        let negated = self.take_word("not");
                        if !negated {
                            self.take_word("is");
                        }
                        if !self.take_word("in") {
                            return Err(self.error("expected in after membership operator"));
                        }
                        self.expect("(")?;
                        push(
                            &mut tasks,
                            ExprTask::Membership(left, Vec::new(), negated, min, commas),
                            start,
                        )?;
                        push(&mut tasks, ExprTask::Parse(2, false), start)?;
                        continue;
                    }
                    if self.symbol("?") && min <= 3 {
                        self.cursor += 1;
                        push(&mut tasks, ExprTask::Middle(left, min, commas), start)?;
                        push(
                            &mut tasks,
                            ExprTask::Parse(if commas { 1 } else { 2 }, commas),
                            start,
                        )?;
                        continue;
                    }
                    if let Kind::Symbol(op) = self.token().kind
                        && let Some((left_bp, right_bp)) = binding(op)
                        && left_bp >= min
                        && (commas || op != ",")
                    {
                        self.cursor += 1;
                        push(&mut tasks, ExprTask::Binary(op, left, min, commas), start)?;
                        push(&mut tasks, ExprTask::Parse(right_bp, commas), start)?;
                        continue;
                    }
                    value = Some(left);
                }
                ExprTask::Prefix(op, start, min, commas) => {
                    let operand = self.result(&mut value)?;
                    let id = self.node(
                        Syntax::Unary(op, operand, false),
                        start,
                        self.nodes[operand.0].end,
                    )?;
                    push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                }
                ExprTask::Group(start, min, commas) => {
                    let child = self.result(&mut value)?;
                    self.expect(")")?;
                    let id = self.node(Syntax::Group(child), start, self.consumed_end())?;
                    push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                }
                ExprTask::Binary(op, left, min, commas) => {
                    let right = self.result(&mut value)?;
                    let start = self.nodes[left.0].start;
                    let id = self.node(
                        Syntax::Binary(op, left, right),
                        start,
                        self.nodes[right.0].end,
                    )?;
                    push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                }
                ExprTask::CallNext(callee, arguments, min, commas) => {
                    push(
                        &mut tasks,
                        ExprTask::CallArg(callee, arguments, min, commas),
                        self.byte(),
                    )?;
                    push(&mut tasks, ExprTask::Parse(2, false), self.byte())?;
                }
                ExprTask::CallArg(callee, mut arguments, min, commas) => {
                    let argument = self.result(&mut value)?;
                    if self.take("...") {
                        let argument = if arguments.is_empty() {
                            argument
                        } else {
                            push(&mut arguments, argument, self.byte())?;
                            self.node(
                                Syntax::ArgumentPack(arguments),
                                self.nodes[callee.0].start,
                                self.consumed_end(),
                            )?
                        };
                        if self.take(",") {
                            let mut parts = Vec::new();
                            push(&mut parts, argument, self.byte())?;
                            push(
                                &mut tasks,
                                ExprTask::TailNext(callee, parts, min, commas),
                                self.byte(),
                            )?;
                            continue;
                        }
                        self.expect(")")?;
                        let start = self.nodes[callee.0].start;
                        let id =
                            self.node(Syntax::Apply(callee, argument), start, self.consumed_end())?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                        continue;
                    }
                    push(&mut arguments, argument, self.byte())?;
                    if self.take(",") {
                        push(
                            &mut tasks,
                            ExprTask::CallNext(callee, arguments, min, commas),
                            self.byte(),
                        )?;
                    } else {
                        self.expect(")")?;
                        let start = self.nodes[callee.0].start;
                        let id =
                            self.node(Syntax::Call(callee, arguments), start, self.consumed_end())?;
                        self.note_text_builder(callee);
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                    }
                }
                ExprTask::TailNext(callee, parts, min, commas) => {
                    push(
                        &mut tasks,
                        ExprTask::TailArg(callee, parts, min, commas),
                        self.byte(),
                    )?;
                    push(&mut tasks, ExprTask::Parse(2, false), self.byte())?;
                }
                ExprTask::TailArg(callee, mut parts, min, commas) => {
                    let argument = self.result(&mut value)?;
                    if self.take("...") {
                        return Err(self.error("only one argument expansion is supported"));
                    }
                    push(&mut parts, argument, self.byte())?;
                    if self.take(",") {
                        push(
                            &mut tasks,
                            ExprTask::TailNext(callee, parts, min, commas),
                            self.byte(),
                        )?;
                    } else {
                        self.expect(")")?;
                        let start = self.nodes[callee.0].start;
                        let pack =
                            self.node(Syntax::ArgumentPackTail(parts), start, self.consumed_end())?;
                        let id =
                            self.node(Syntax::Apply(callee, pack), start, self.consumed_end())?;
                        push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                    }
                }
                ExprTask::Anonymous(start, parameters, min, commas) => {
                    let expression = self.result(&mut value)?;
                    self.expect("}")?;
                    let end = self.consumed_end();
                    let body = if matches!(&self.nodes[expression.0].syntax, Syntax::String(value) if value.emitting)
                        || matches!(
                            self.nodes[expression.0].syntax,
                            Syntax::Interpolation { emitting: true, .. }
                        ) {
                        let statement = self.node(Syntax::Expression(expression), start, end)?;
                        let ret = self.node(Syntax::Return(None), start, end)?;
                        let mut statements = Vec::new();
                        push(&mut statements, statement, start)?;
                        push(&mut statements, ret, start)?;
                        self.node(Syntax::Block(statements), start, end)?
                    } else {
                        self.node(Syntax::Return(Some(expression)), start, end)?
                    };
                    let function = self.node(
                        Syntax::Function {
                            returns_owned: false,
                            optional: 0,
                            rest: false,
                            name: format!("$callback{}", self.anonymous.len()),
                            is_static: false,
                            parameters,
                            body,
                        },
                        start,
                        end,
                    )?;
                    push(&mut self.anonymous, function, start)?;
                    let reference = self.node(Syntax::FunctionReference(function), start, end)?;
                    push(&mut tasks, ExprTask::Rest(reference, min, commas), start)?;
                }
                ExprTask::Published(start, min, commas) => {
                    let expression = self.result(&mut value)?;
                    let (prototype, arguments) = match &self.nodes[expression.0].syntax {
                        Syntax::Name(_) => (expression, Vec::new()),
                        Syntax::Call(prototype, arguments)
                            if matches!(self.nodes[prototype.0].syntax, Syntax::Name(_)) =>
                        {
                            let mut copied = Vec::new();
                            copied
                                .try_reserve(arguments.len())
                                .map_err(|_| Diagnostic::resource(start))?;
                            copied.extend_from_slice(arguments);
                            (*prototype, copied)
                        }
                        _ => {
                            return Err(
                                self.error("published construction requires a named prototype")
                            );
                        }
                    };
                    let node = self.node(
                        Syntax::PublishedConstruction {
                            prototype,
                            arguments,
                        },
                        start,
                        self.consumed_end(),
                    )?;
                    push(&mut tasks, ExprTask::Rest(node, min, commas), start)?;
                }
                ExprTask::Middle(condition, min, commas) => {
                    let yes = self.result(&mut value)?;
                    self.expect(":")?;
                    push(
                        &mut tasks,
                        ExprTask::Conditional(condition, yes, min, commas),
                        self.byte(),
                    )?;
                    push(&mut tasks, ExprTask::Parse(2, commas), self.byte())?;
                }
                ExprTask::Conditional(condition, yes, min, commas) => {
                    let no = self.result(&mut value)?;
                    let start = self.nodes[condition.0].start;
                    let id = self.node(
                        Syntax::Conditional(condition, yes, no),
                        start,
                        self.nodes[no.0].end,
                    )?;
                    push(&mut tasks, ExprTask::Rest(id, min, commas), start)?;
                }
            }
        }
        self.result(&mut value)
    }

    fn formal_parameters(
        &mut self,
        receiver: bool,
    ) -> Result<(Vec<String>, usize, bool), Diagnostic> {
        self.expect("(")?;
        let mut parameters = Vec::new();
        if receiver {
            push(&mut parameters, "self".to_owned(), self.byte())?;
        }
        let mut optional = 0;
        let mut rest = false;
        if !self.take(")") {
            loop {
                if self.take("...") {
                    rest = true;
                    push(&mut parameters, "$rest".to_owned(), self.byte())?;
                    self.expect(")")?;
                    break;
                }
                rest = self.take("[");
                let name = self.name()?;
                push(&mut parameters, name, self.byte())?;
                if rest {
                    self.expect("]")?;
                    self.expect(")")?;
                    break;
                }
                if self.take("?") {
                    optional += 1;
                } else if optional != 0 {
                    return Err(self.error("required parameter cannot follow optional parameters"));
                }
                if self.take(")") {
                    break;
                }
                self.expect(",")?;
            }
        }
        Ok((parameters, optional, rest))
    }

    // Expand range initializers into scoped locals and ordinary guarded operations.
    // Synthetic names contain '$', which source identifiers cannot contain.
    /// Parse `[local] name in collection)` for foreach and collection for..in.
    fn foreach_header(&mut self, start: usize) -> Result<(Id, Id, Option<Id>), Diagnostic> {
        let local = self.take_word("local");
        let destination = self.name()?;
        let name = if local {
            destination.clone()
        } else {
            format!("$foreach{}", self.nodes.len())
        };
        let assignment = if local {
            None
        } else {
            let target = self.node(Syntax::Name(destination), start, self.consumed_end())?;
            let source = self.node(Syntax::Name(name.clone()), start, self.consumed_end())?;
            let assign = self.node(
                Syntax::Binary("=", target, source),
                start,
                self.consumed_end(),
            )?;
            Some(self.node(Syntax::Expression(assign), start, self.consumed_end())?)
        };
        if !self.take_word("in") {
            return Err(Diagnostic::new(
                "parse-expected",
                "expected in",
                self.byte(),
            ));
        }
        let collection = self.expression(1, true)?;
        self.expect(")")?;
        let mut declarations = Vec::new();
        push(&mut declarations, (name, None), start)?;
        let declaration = self.node(Syntax::Local(declarations), start, self.consumed_end())?;
        Ok((declaration, collection, assignment))
    }

    /// Token-only lookahead after `for (`: `[local] name in expr)` with no `..`,
    /// `,` or `;` at depth zero is a collection loop. Creates no AST nodes.
    fn for_in_collection(&self) -> bool {
        let spelled = |index: usize, word: &str| {
            self.tokens.get(index).is_some_and(|token| {
                token.kind == Kind::Identifier && token.spelling(self.source).eq(word.chars())
            })
        };
        let mut index = self.cursor;
        if spelled(index, "local") {
            index += 1;
        }
        if !self
            .tokens
            .get(index)
            .is_some_and(|token| token.kind == Kind::Identifier)
            || !spelled(index + 1, "in")
        {
            return false;
        }
        let mut depth = 0usize;
        for token in self.tokens.iter().skip(index + 2) {
            match token.kind {
                Kind::Symbol("(" | "[" | "{") => depth += 1,
                Kind::Symbol(")" | "]" | "}") => {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                }
                Kind::Symbol(".." | "," | ";") if depth == 0 => return false,
                Kind::End => return false,
                _ => {}
            }
        }
        false
    }

    fn range_initializer(
        &mut self,
        name: String,
        local: bool,
        start: usize,
        init: &mut Vec<Id>,
    ) -> Result<(Id, Id), Diagnostic> {
        let from = self.expression(2, false)?;
        self.expect("..")?;
        let to = self.expression(2, false)?;
        let step = if self.take_word("step") {
            self.expression(2, false)?
        } else {
            self.node(
                Syntax::Integer("1".to_owned(), 10),
                start,
                self.consumed_end(),
            )?
        };
        let end = self.consumed_end();
        let target = self.node(Syntax::Name(name.clone()), start, end)?;
        let first = if local {
            let mut declarations = Vec::new();
            push(&mut declarations, (name, Some(from)), start)?;
            self.node(Syntax::Local(declarations), start, end)?
        } else {
            let assignment = self.node(Syntax::Binary("=", target, from), start, end)?;
            self.node(Syntax::Expression(assignment), start, end)?
        };
        push(init, first, start)?;
        let mut saved = Vec::new();
        for (label, value) in [("end", to), ("step", step)] {
            let name = format!("$range{}.{}", self.nodes.len(), label);
            let mut declarations = Vec::new();
            push(&mut declarations, (name.clone(), Some(value)), start)?;
            let declaration = self.node(Syntax::Local(declarations), start, end)?;
            push(init, declaration, start)?;
            let reference = self.node(Syntax::Name(name), start, end)?;
            push(&mut saved, reference, start)?;
        }
        let zero = self.node(Syntax::Integer("0".to_owned(), 10), start, end)?;
        let descending = self.node(Syntax::Binary("<", saved[1], zero), start, end)?;
        let down = self.node(Syntax::Binary(">=", target, saved[0]), start, end)?;
        let up = self.node(Syntax::Binary("<=", target, saved[0]), start, end)?;
        let condition = self.node(Syntax::Conditional(descending, down, up), start, end)?;
        let update = self.node(Syntax::Binary("+=", target, saved[1]), start, end)?;
        Ok((condition, update))
    }

    fn statement(&mut self) -> Result<Id, Diagnostic> {
        let mut tasks = Vec::new();
        push(&mut tasks, StatementTask::Parse, self.byte())?;
        let mut value = None;
        while let Some(task) = tasks.pop() {
            match task {
                StatementTask::Parse => {
                    let start = self.byte();
                    if self.token().kind == Kind::Identifier
                        && self
                            .tokens
                            .get(self.cursor + 1)
                            .is_some_and(|token| token.kind == Kind::Symbol(":"))
                    {
                        let name = self.name()?;
                        self.expect(":")?;
                        push(&mut tasks, StatementTask::Label(start, name), start)?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else if self.take("{") {
                        push(
                            &mut tasks,
                            StatementTask::Block(
                                start,
                                (self.anonymous.len(), self.text_builders),
                                Vec::new(),
                            ),
                            start,
                        )?;
                    } else if self.take(";") {
                        value = Some(self.node(Syntax::Empty, start, self.consumed_end())?);
                    } else if self.take_word("if") || self.take_word("while") {
                        let is_if = self.tokens[self.cursor - 1]
                            .spelling(self.source)
                            .eq("if".chars());
                        self.expect("(")?;
                        let condition = self.expression(1, true)?;
                        self.expect(")")?;
                        let after = if is_if {
                            StatementTask::Then(start, condition)
                        } else {
                            StatementTask::While(start, condition)
                        };
                        push(&mut tasks, after, start)?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else if self.take_word("do") {
                        push(&mut tasks, StatementTask::DoWhile(start), start)?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else if self.take_word("foreach") {
                        self.expect("(")?;
                        let (declaration, collection, assignment) = self.foreach_header(start)?;
                        push(
                            &mut tasks,
                            StatementTask::ForEach(start, declaration, collection, assignment),
                            start,
                        )?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else if self.take_word("for") {
                        self.expect("(")?;
                        if self.for_in_collection() {
                            // TADS `for (x in collection)` iterates like foreach
                            // (external for-in-reference-001); ranges keep range-for.
                            let (declaration, collection, assignment) =
                                self.foreach_header(start)?;
                            push(
                                &mut tasks,
                                StatementTask::ForEach(start, declaration, collection, assignment),
                                start,
                            )?;
                            push(&mut tasks, StatementTask::Parse, start)?;
                            continue;
                        }
                        let mut init = Vec::new();
                        let mut range_conditions = Vec::new();
                        let mut range_updates = Vec::new();
                        while !self.symbol(";") && !self.symbol(")") {
                            let local = self.take_word("local");
                            let range = self.token().kind == Kind::Identifier
                                && self.tokens.get(self.cursor + 1).is_some_and(|token| {
                                    token.spelling(self.source).eq("in".chars())
                                });
                            if range {
                                let name = self.name()?;
                                self.take_word("in");
                                let (condition, update) =
                                    self.range_initializer(name, local, start, &mut init)?;
                                push(&mut range_conditions, condition, start)?;
                                push(&mut range_updates, update, start)?;
                            } else {
                                let statement = if local {
                                    let name = self.name()?;
                                    self.expect("=")?;
                                    let value = self.expression(2, false)?;
                                    let mut declarations = Vec::new();
                                    push(&mut declarations, (name, Some(value)), start)?;
                                    self.node(
                                        Syntax::Local(declarations),
                                        start,
                                        self.consumed_end(),
                                    )?
                                } else {
                                    let expression = self.expression(2, false)?;
                                    self.node(
                                        Syntax::Expression(expression),
                                        start,
                                        self.consumed_end(),
                                    )?
                                };
                                push(&mut init, statement, start)?;
                            }
                            if !self.take(",") {
                                break;
                            }
                            if self.symbol(";") || self.symbol(")") {
                                return Err(self.error("missing for initializer after comma"));
                            }
                        }
                        let (explicit_condition, mut step) =
                            if !range_conditions.is_empty() && self.symbol(")") {
                                (None, None)
                            } else {
                                self.expect(";")?;
                                let condition = if self.symbol(";") {
                                    None
                                } else {
                                    Some(self.expression(1, true)?)
                                };
                                self.expect(";")?;
                                let step = if self.symbol(")") {
                                    None
                                } else {
                                    Some(self.expression(1, true)?)
                                };
                                (condition, step)
                            };
                        self.expect(")")?;
                        let mut condition = None;
                        for next in range_conditions.into_iter().chain(explicit_condition) {
                            condition = Some(if let Some(previous) = condition {
                                self.node(
                                    Syntax::Binary("&&", previous, next),
                                    start,
                                    self.consumed_end(),
                                )?
                            } else {
                                next
                            });
                        }
                        for next in range_updates {
                            step = Some(if let Some(previous) = step {
                                self.node(
                                    Syntax::Binary(",", previous, next),
                                    start,
                                    self.consumed_end(),
                                )?
                            } else {
                                next
                            });
                        }
                        push(
                            &mut tasks,
                            StatementTask::For(start, init, condition, step),
                            start,
                        )?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else if self.take_word("switch") {
                        self.expect("(")?;
                        let selector = self.expression(1, true)?;
                        self.expect(")")?;
                        self.expect("{")?;
                        push(
                            &mut tasks,
                            StatementTask::Switch(start, selector, Vec::new()),
                            start,
                        )?;
                    } else if self.take_word("try") {
                        push(&mut tasks, StatementTask::TryBody(start), start)?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else if self.take_word("throw") {
                        let expression = if self.take_word("new") {
                            let published = self.take_word("published");
                            let expression = self.expression(15, false)?;
                            let (prototype, arguments) = match &self.nodes[expression.0].syntax {
                                Syntax::Name(_) => (expression, Vec::new()),
                                Syntax::Call(prototype, arguments)
                                    if matches!(
                                        self.nodes[prototype.0].syntax,
                                        Syntax::Name(_)
                                    ) =>
                                {
                                    let mut copied = Vec::new();
                                    copied
                                        .try_reserve(arguments.len())
                                        .map_err(|_| Diagnostic::resource(start))?;
                                    copied.extend_from_slice(arguments);
                                    (*prototype, copied)
                                }
                                _ => {
                                    return Err(self.error(
                                        "exception construction requires a named prototype",
                                    ));
                                }
                            };
                            self.node(
                                if published {
                                    Syntax::PublishedConstruction {
                                        prototype,
                                        arguments,
                                    }
                                } else {
                                    Syntax::ExceptionConstruction {
                                        prototype,
                                        arguments,
                                    }
                                },
                                start,
                                self.consumed_end(),
                            )?
                        } else {
                            self.expression(1, true)?
                        };
                        self.expect(";")?;
                        value = Some(self.node(
                            Syntax::Throw(expression),
                            start,
                            self.consumed_end(),
                        )?);
                    } else if self.take_word("return") {
                        let expression = if self.symbol(";") {
                            None
                        } else {
                            let moved = self.take_word("move");
                            let expression = self.expression(1, true)?;
                            Some(if moved {
                                self.node(Syntax::Move(expression), start, self.consumed_end())?
                            } else {
                                expression
                            })
                        };
                        self.expect(";")?;
                        value = Some(self.node(
                            Syntax::Return(expression),
                            start,
                            self.consumed_end(),
                        )?);
                    } else if self.take_word("local") {
                        let owned = self.take_word_before_name("owned");
                        let mut declarations = Vec::new();
                        loop {
                            let name = self.name()?;
                            let init = if owned {
                                self.expect("=")?;
                                if !self.take_word("new") {
                                    let call = self.expression(2, false)?;
                                    Some(
                                        if matches!(self.nodes[call.0].syntax, Syntax::Lookup(_)) {
                                            call
                                        } else {
                                            self.node(
                                                Syntax::OwnedCall(call),
                                                start,
                                                self.consumed_end(),
                                            )?
                                        },
                                    )
                                } else if let Some(node) = self.intrinsic_collection(start)? {
                                    Some(node)
                                } else {
                                    let expression = self.expression(15, false)?;
                                    let (prototype, arguments) =
                                        match &self.nodes[expression.0].syntax {
                                            Syntax::Name(_) => (expression, Vec::new()),
                                            Syntax::Apply(prototype, argument)
                                                if matches!(
                                                    self.nodes[prototype.0].syntax,
                                                    Syntax::Name(_)
                                                ) =>
                                            {
                                                let prototype = *prototype;
                                                let argument = *argument;
                                                let expanded = self.node(
                                                    Syntax::ExpandedArgument(argument),
                                                    start,
                                                    self.consumed_end(),
                                                )?;
                                                {
                                                    let mut arguments = Vec::new();
                                                    push(&mut arguments, expanded, start)?;
                                                    (prototype, arguments)
                                                }
                                            }
                                            Syntax::Call(prototype, arguments)
                                                if matches!(
                                                    self.nodes[prototype.0].syntax,
                                                    Syntax::Name(_)
                                                ) =>
                                            {
                                                let mut copied = Vec::new();
                                                copied
                                                    .try_reserve(arguments.len())
                                                    .map_err(|_| Diagnostic::resource(start))?;
                                                copied.extend_from_slice(arguments);
                                                (*prototype, copied)
                                            }
                                            _ => {
                                                return Err(self.error(
                                                    "local construction requires a named prototype",
                                                ));
                                            }
                                        };
                                    Some(self.node(
                                        Syntax::LocalConstruction {
                                            prototype,
                                            arguments,
                                        },
                                        start,
                                        self.consumed_end(),
                                    )?)
                                }
                            } else if self.take("=") {
                                // Under lifetimes a growable collection needs no
                                // owner, so a plain local reaches the intrinsic
                                // constructors too.
                                let resumption = self.cursor;
                                let intrinsic = if self.lifetimes && self.take_word("new") {
                                    self.intrinsic_collection(start)?
                                } else {
                                    None
                                };
                                match intrinsic {
                                    Some(node) => Some(node),
                                    None => {
                                        self.cursor = resumption;
                                        Some(self.expression(2, false)?)
                                    }
                                }
                            } else {
                                None
                            };
                            push(&mut declarations, (name, init), start)?;
                            if !self.take(",") {
                                break;
                            }
                        }
                        self.expect(";")?;
                        value = Some(self.node(
                            if owned {
                                Syntax::OwnedLocal(declarations)
                            } else {
                                Syntax::Local(declarations)
                            },
                            start,
                            self.consumed_end(),
                        )?);
                    } else if self.take_word("break") {
                        let syntax = if self.token().kind == Kind::Identifier {
                            Syntax::NamedBreak(self.name()?)
                        } else {
                            Syntax::Break
                        };
                        self.expect(";")?;
                        value = Some(self.node(syntax, start, self.consumed_end())?);
                    } else if self.take_word("continue") {
                        let syntax = if self.token().kind == Kind::Identifier {
                            Syntax::NamedContinue(self.name()?)
                        } else {
                            Syntax::Continue
                        };
                        self.expect(";")?;
                        value = Some(self.node(syntax, start, self.consumed_end())?);
                    } else if self.take_word("goto") {
                        let name = self.name()?;
                        self.expect(";")?;
                        value = Some(self.node(Syntax::Goto(name), start, self.consumed_end())?);
                    } else if ["for", "do", "switch", "try", "throw", "foreach"]
                        .iter()
                        .any(|word| self.word(word))
                    {
                        return Err(Diagnostic::new(
                            "parse-unavailable",
                            "statement is outside this syntax slice",
                            start,
                        ));
                    } else {
                        let expression = self.expression(1, true)?;
                        self.expect(";")?;
                        value = Some(self.node(
                            Syntax::Expression(expression),
                            start,
                            self.consumed_end(),
                        )?);
                    }
                }
                StatementTask::Switch(start, selector, mut arms) => {
                    if self.take("}") {
                        value = Some(self.node(
                            Syntax::Switch(selector, arms),
                            start,
                            self.consumed_end(),
                        )?);
                    } else if self.word("case") || self.word("default") {
                        let case = if self.take_word("case") {
                            Some(self.expression(2, false)?)
                        } else {
                            self.take_word("default");
                            if arms.iter().any(|(case, _)| case.is_none()) {
                                return Err(self.error("duplicate default label"));
                            }
                            None
                        };
                        self.expect(":")?;
                        push(&mut arms, (case, Vec::new()), start)?;
                        push(
                            &mut tasks,
                            StatementTask::Switch(start, selector, arms),
                            start,
                        )?;
                    } else {
                        if arms.is_empty() {
                            return Err(
                                self.error("switch statement requires a case or default label")
                            );
                        }
                        if self.token().kind == Kind::End {
                            return Err(self.error("unterminated switch"));
                        }
                        push(
                            &mut tasks,
                            StatementTask::SwitchChild(start, selector, arms),
                            start,
                        )?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    }
                }
                StatementTask::SwitchChild(start, selector, mut arms) => {
                    let child = self.result(&mut value)?;
                    push(&mut arms.last_mut().unwrap().1, child, start)?;
                    push(
                        &mut tasks,
                        StatementTask::Switch(start, selector, arms),
                        start,
                    )?;
                }
                StatementTask::Label(start, name) => {
                    let body = self.result(&mut value)?;
                    value =
                        Some(self.node(Syntax::Label(name, body), start, self.consumed_end())?);
                }
                StatementTask::Block(start, callbacks, children) => {
                    if self.take("}") {
                        // A block owns a scope when it declares an owned local, and
                        // also when it creates a callback, whose captured
                        // environment belongs to that scope.
                        let owns = !self.lifetimes
                            && (self.anonymous.len() > callbacks.0
                                || self.text_builders > callbacks.1
                                || children.iter().any(|id| {
                                    matches!(self.nodes[id.0].syntax, Syntax::OwnedLocal(_))
                                }));
                        let block =
                            self.node(Syntax::Block(children), start, self.consumed_end())?;
                        value = Some(if owns {
                            self.node(Syntax::OwnerScope(block), start, self.consumed_end())?
                        } else {
                            block
                        });
                    } else {
                        if self.token().kind == Kind::End {
                            return Err(self.error("unterminated block"));
                        }
                        push(
                            &mut tasks,
                            StatementTask::BlockChild(start, callbacks, children),
                            start,
                        )?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    }
                }
                StatementTask::BlockChild(start, callbacks, mut children) => {
                    let child = self.result(&mut value)?;
                    push(&mut children, child, start)?;
                    push(
                        &mut tasks,
                        StatementTask::Block(start, callbacks, children),
                        start,
                    )?;
                }
                StatementTask::Then(start, condition) => {
                    let yes = self.result(&mut value)?;
                    if self.take_word("else") {
                        push(
                            &mut tasks,
                            StatementTask::Else(start, condition, yes),
                            start,
                        )?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else {
                        value = Some(self.node(
                            Syntax::If(condition, yes, None),
                            start,
                            self.consumed_end(),
                        )?);
                    }
                }
                StatementTask::Else(start, condition, yes) => {
                    let no = self.result(&mut value)?;
                    value = Some(self.node(
                        Syntax::If(condition, yes, Some(no)),
                        start,
                        self.consumed_end(),
                    )?);
                }
                StatementTask::TryBody(start) => {
                    let body = self.result(&mut value)?;
                    push(
                        &mut tasks,
                        StatementTask::TryNext(start, body, Vec::new()),
                        start,
                    )?;
                }
                StatementTask::TryNext(start, body, catches) => {
                    if self.take_word("catch") {
                        self.expect("(")?;
                        let class_start = self.byte();
                        let name = self.name()?;
                        let class =
                            self.node(Syntax::Name(name), class_start, self.consumed_end())?;
                        let name = self.name()?;
                        self.expect(")")?;
                        let mut locals = Vec::new();
                        push(&mut locals, (name, None), start)?;
                        let declaration =
                            self.node(Syntax::Local(locals), class_start, self.consumed_end())?;
                        push(
                            &mut tasks,
                            StatementTask::CatchBody(start, body, catches, class, declaration),
                            start,
                        )?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else if self.take_word("finally") {
                        let protected = if catches.is_empty() {
                            body
                        } else {
                            self.node(Syntax::Try(body, catches), start, self.consumed_end())?
                        };
                        push(
                            &mut tasks,
                            StatementTask::FinallyBody(start, protected),
                            start,
                        )?;
                        push(&mut tasks, StatementTask::Parse, start)?;
                    } else {
                        if catches.is_empty() {
                            return Err(Diagnostic::new(
                                "parse-expected",
                                "try requires catch or finally",
                                self.byte(),
                            ));
                        }
                        value = Some(self.node(
                            Syntax::Try(body, catches),
                            start,
                            self.consumed_end(),
                        )?);
                    }
                }
                StatementTask::FinallyBody(start, protected) => {
                    let finalizer = self.result(&mut value)?;
                    value = Some(self.node(
                        Syntax::Finally(protected, finalizer),
                        start,
                        self.consumed_end(),
                    )?);
                }
                StatementTask::CatchBody(start, body, mut catches, class, declaration) => {
                    let handler = self.result(&mut value)?;
                    push(&mut catches, (class, declaration, handler), start)?;
                    push(
                        &mut tasks,
                        StatementTask::TryNext(start, body, catches),
                        start,
                    )?;
                }
                StatementTask::ForEach(start, declaration, collection, assignment) => {
                    let mut body = self.result(&mut value)?;
                    if let Some(assignment) = assignment {
                        let mut statements = Vec::new();
                        push(&mut statements, assignment, start)?;
                        push(&mut statements, body, start)?;
                        body = self.node(Syntax::Block(statements), start, self.consumed_end())?;
                    }
                    value = Some(self.node(
                        Syntax::ForEach(declaration, collection, body),
                        start,
                        self.consumed_end(),
                    )?);
                }
                StatementTask::For(start, init, condition, step) => {
                    let body = self.result(&mut value)?;
                    value = Some(self.node(
                        Syntax::For {
                            init,
                            condition,
                            step,
                            body,
                        },
                        start,
                        self.consumed_end(),
                    )?);
                }
                StatementTask::DoWhile(start) => {
                    let body = self.result(&mut value)?;
                    if !self.take_word("while") {
                        return Err(self.error("expected while after do body"));
                    }
                    self.expect("(")?;
                    let condition = self.expression(1, true)?;
                    self.expect(")")?;
                    self.expect(";")?;
                    value = Some(self.node(
                        Syntax::DoWhile(condition, body),
                        start,
                        self.consumed_end(),
                    )?);
                }
                StatementTask::While(start, condition) => {
                    let body = self.result(&mut value)?;
                    value = Some(self.node(
                        Syntax::While(condition, body),
                        start,
                        self.consumed_end(),
                    )?);
                }
            }
        }
        self.result(&mut value)
    }
}

fn binding(op: &str) -> Option<(u8, u8)> {
    let precedence = match op {
        "," => 1,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" | ">>>=" => {
            return Some((2, 2));
        }
        "??" => 4,
        "||" => 5,
        "&&" => 6,
        "|" => 7,
        "^" => 8,
        "&" => 9,
        "==" | "!=" => 10,
        "<" | ">" | "<=" | ">=" => 11,
        "<<" | ">>" | ">>>" => 12,
        "+" | "-" => 13,
        "*" | "/" | "%" => 14,
        _ => return None,
    };
    Some((precedence, precedence + 1))
}

/// The memory model a compilation uses. The build selects it; no
/// source declaration names it, so a file carries no mode word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Model {
    /// World/Turn lifetimes with automatic promotion.
    #[default]
    Lifetimes,
    /// Scope-bound ownership: owned locals, recipients and owning returns.
    Ownership,
}

/// Parse under the default model. `parse_with` names the model explicitly.
pub fn parse(source: &Source) -> Result<Ast, Diagnostic> {
    parse_with(source, Model::default())
}

pub fn parse_with(source: &Source, model: Model) -> Result<Ast, Diagnostic> {
    let tokens = lexer::lex(source)?;
    let mut braces = std::collections::HashMap::new();
    let mut opens = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind == Kind::Symbol("{") {
            push(&mut opens, index, token.byte_start)?;
        } else if token.kind == Kind::Symbol("}")
            && let Some(open) = opens.pop()
        {
            braces
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(token.byte_start))?;
            braces.insert(open, index);
        }
    }
    let mut parser = Parser {
        interpolated: Vec::new(),
        source,
        tokens,
        cursor: 0,
        nodes: Vec::new(),
        anonymous: Vec::new(),
        text_builders: 0,
        lifetimes: matches!(model, Model::Lifetimes),
        deferred_bodies: Vec::new(),
        braces,
    };
    let mut functions = Vec::new();
    let mut objects = Vec::new();
    let mut templates = Vec::new();
    let mut containment_property = None;
    let mut containers: Vec<String> = Vec::new();
    let mut active_dictionary = None;
    let mut relations: Vec<RelationDecl> = Vec::new();
    // property values that are relation rows, by the value's node.
    let mut declared_rows: Vec<(Id, DeclaredRow)> = Vec::new();
    let lifetimes = matches!(model, Model::Lifetimes);
    let mut object_dictionaries = Vec::new();
    // (layer, target) object indices for `modify` declarations, in source order.
    let mut modifications: Vec<(usize, usize)> = Vec::new();
    let mut vocabulary_properties = Vec::new();
    let mut vocabulary_values = Vec::new();
    while parser.token().kind != Kind::End {
        if parser.take(";") {
            continue;
        }
        let start = parser.byte();
        let mut containment_depth = 0usize;
        loop {
            let increment = if parser.take("++") {
                2
            } else if parser.take("+") {
                1
            } else {
                break;
            };
            containment_depth = containment_depth
                .checked_add(increment)
                .ok_or_else(|| Diagnostic::resource(start))?;
        }
        if containment_depth == 1 && parser.take_word("property") {
            let name = parser.name()?;
            parser.expect(";")?;
            containment_property = Some(name.clone());
            let mut names = Vec::new();
            push(&mut names, name, start)?;
            parser.node(
                Syntax::Declaration(Declaration::Properties(names)),
                start,
                parser.consumed_end(),
            )?;
            continue;
        }
        if containment_depth > 0
            && (parser.word("dictionary")
                || parser.word("enum")
                || parser.word("property")
                || parser.word("export")
                || parser.word("intrinsic"))
        {
            return Err(parser.error("containment prefix requires an object"));
        }
        if parser.take_word("relation") {
            // relation NAME(left, right) CARDINALITY [reverse NAME];
            let forward = parser.name()?;
            parser.expect("(")?;
            // A side may carry a type annotation. `Text` on the right side is
            // the one that changes anything: it says the pairs are vocabulary,
            // which the dictionary already stores.
            let mut vocabulary = false;
            let mut label_family = None;
            let mut sides = 0;
            loop {
                parser.name()?;
                sides += 1;
                if parser.take(":") {
                    let annotation = parser.name()?;
                    if annotation == "Text" {
                        if sides != 2 {
                            return Err(parser.error("only a relation's right side may be Text"));
                        }
                        vocabulary = true;
                    } else if sides <= 2 && annotation != "Entity" {
                        // the annotation decides what the column holds,
                        // so a name nothing means is a mistake rather than a
                        // comment. The label column is resolved against named
                        // enum families by semantic analysis.
                        return Err(
                            parser.error("a relation column is Entity, or Text on the right")
                        );
                    }
                    if sides == 3 {
                        label_family = Some(annotation);
                    }
                }
                if !parser.take(",") {
                    break;
                }
            }
            parser.expect(")")?;
            // A third column labels the row, so the relation becomes a family
            // of tables, one per label. That is what travel needs.
            if sides > 3 {
                return Err(parser.error("a relation has two columns, or three with a label"));
            }
            // a vocabulary relation's label is its part of speech. A
            // TADS dictionary entry is (word, object, property), so the label
            // names the vocabulary property the row is filed under rather than
            // one table of a family. Unlike a relation-family label it is a property,
            // not an enumerator, and it rides in the operand the vocabulary
            // path already carries rather than in the relation descriptor.
            let labelled = sides == 3;
            let cardinality = if parser.take_word("one_to_one") {
                0
            } else if parser.take_word("one_to_many") {
                1
            } else if parser.take_word("many_to_many") {
                2
            } else {
                return Err(parser.error("relation needs one_to_one, one_to_many or many_to_many"));
            };
            let reverse = if parser.take_word("reverse") {
                Some(parser.name()?)
            } else {
                None
            };
            parser.expect(";")?;
            if relations.iter().any(|existing: &RelationDecl| {
                existing.forward == forward
                    || existing.reverse == reverse.clone().or(Some(forward.clone()))
            }) {
                return Err(parser.error("relation name is already declared"));
            }
            if vocabulary && active_dictionary.is_none() {
                // Its rows would have nowhere to live, and nowhere to be read
                // from.
                return Err(
                    parser.error("a vocabulary relation needs a dictionary declared before it")
                );
            }
            if vocabulary {
                // The forward name is also the part of speech the dictionary
                // files these words under, so `desk: Thing names = 'desk'`
                // populates it through the existing vocabulary path.
                push(&mut vocabulary_properties, forward.clone(), start)?;
                let mut names = Vec::new();
                push(&mut names, forward.clone(), start)?;
                parser.node(
                    Syntax::Declaration(Declaration::VocabularyProperties(names)),
                    start,
                    parser.consumed_end(),
                )?;
            }
            let index = relations.len();
            push(
                &mut relations,
                RelationDecl {
                    forward: forward.clone(),
                    reverse,
                    cardinality,
                    vocabulary,
                    labelled,
                    label_family,
                    dictionary: vocabulary.then(|| active_dictionary.clone()).flatten(),
                },
                start,
            )?;
            parser.node(
                Syntax::Declaration(Declaration::Relation(index)),
                start,
                parser.consumed_end(),
            )?;
            continue;
        }
        if parser.take_word("dictionary") {
            if parser.take_word("property") {
                let mut names = Vec::new();
                loop {
                    let name = parser.name()?;
                    push(&mut names, name, start)?;
                    if parser.take(";") {
                        break;
                    }
                    parser.expect(",")?;
                }
                for name in &names {
                    if !vocabulary_properties.contains(name) {
                        push(&mut vocabulary_properties, name.clone(), start)?;
                    }
                }
                parser.node(
                    Syntax::Declaration(Declaration::VocabularyProperties(names)),
                    start,
                    parser.consumed_end(),
                )?;
                continue;
            }
            let name = parser.name()?;
            parser.expect(";")?;
            active_dictionary = Some(name.clone());
            if objects.iter().any(|id: &Id| matches!(&parser.nodes[id.0].syntax, Syntax::Object { name: old, is_dictionary: true, .. } if old == &name)) {
                continue;
            }
            let node = parser.node(
                Syntax::Object {
                    name,
                    is_class: false,
                    transient: false,
                    is_dictionary: true,
                    parents: Vec::new(),
                    properties: Vec::new(),
                },
                start,
                parser.consumed_end(),
            )?;
            push(&mut objects, node, start)?;
            push(&mut object_dictionaries, None, start)?;
            continue;
        }
        let declaration = if parser.take_word("extern") {
            if !parser.take_word("function") {
                return Err(parser.error("only extern function declarations are implemented"));
            }
            let name = parser.name()?;
            let mut arity = 0usize;
            if parser.take("(") && !parser.take(")") {
                loop {
                    parser.name()?;
                    arity = arity.checked_add(1).ok_or(Diagnostic::resource(start))?;
                    if parser.take(")") {
                        break;
                    }
                    parser.expect(",")?;
                }
            }
            parser.expect(";")?;
            Some(Declaration::ExternalFunction(name, arity))
        } else if parser.take_word("intrinsic") {
            Some(parser.intrinsic()?)
        } else if parser.take_word("enum") {
            let token = parser.take_word("token");
            let first = parser.name()?;
            let family = if parser.take(":") {
                Some(first.clone())
            } else {
                None
            };
            let mut names = vec![if family.is_some() {
                parser.name()?
            } else {
                first
            }];
            while !parser.take(";") {
                parser.expect(",")?;
                let name = parser.name()?;
                push(&mut names, name, start)?;
            }
            Some(Declaration::Enumerators {
                names,
                token,
                family,
            })
        } else if parser.take_word("property") {
            let mut names = Vec::new();
            loop {
                let name = parser.name()?;
                push(&mut names, name, start)?;
                if parser.take(";") {
                    break;
                }
                parser.expect(",")?;
            }
            Some(Declaration::Properties(names))
        } else if parser.take_word("export") {
            let name = parser.name()?;
            let external = if parser.symbol(";") {
                name.clone()
            } else {
                parser.declaration_string()?
            };
            parser.expect(";")?;
            Some(Declaration::Export { name, external })
        } else {
            None
        };
        if let Some(declaration) = declaration {
            parser.node(Syntax::Declaration(declaration), start, parser.byte())?;
            continue;
        }
        let grammar = if parser.take_word("grammar") {
            let rule = parser.grammar_rule()?;
            let name = rule.match_name.clone();
            let production = rule.production.clone();
            parser.node(
                Syntax::Declaration(Declaration::Grammar(rule)),
                start,
                parser.byte(),
            )?;
            // Each production is one static object, the receiver of parseTokens.
            if !objects.iter().any(|id: &Id| {
                matches!(&parser.nodes[id.0].syntax, Syntax::Object { name: existing, .. } if existing == &production)
            }) {
                let mut parents = Vec::new();
                push(&mut parents, "object".to_owned(), start)?;
                let node = parser.node(
                    Syntax::Object {
                        name: production,
                        is_class: false,
                        transient: false,
                        is_dictionary: false,
                        parents,
                        properties: Vec::new(),
                    },
                    start,
                    parser.byte(),
                )?;
                push(&mut objects, node, start)?;
                push(&mut object_dictionaries, None, start)?;
            }
            if name.is_none() {
                continue;
            }
            name
        } else {
            None
        };
        let transient = parser.take_word("transient");
        let is_class = parser.take_word("class");
        if transient && is_class {
            return Err(parser.error("transient class declarations are not implemented"));
        }
        // `modify X...` layers new definitions over an existing object or class.
        let modification = parser.take_word("modify");
        if ["modify", "replace", "grammar", "intrinsic", "enum"]
            .iter()
            .any(|word| parser.word(word))
        {
            return Err(Diagnostic::new(
                "parse-unavailable",
                "declaration is outside this syntax slice",
                start,
            ));
        }
        let returns_owned = parser.take_word("owned");
        let explicit_function = parser.take_word("function");
        let mut name = if let Some(name) = grammar.as_ref() {
            name.clone()
        } else {
            parser.name()?
        };
        if returns_owned && !parser.symbol("(") {
            return Err(parser.error("owning returns require a named function"));
        }
        if parser.take_word("template") {
            if transient {
                return Err(parser.error("transient requires an object declaration"));
            }
            if containment_depth > 0 {
                return Err(parser.error("template cannot have a containment prefix"));
            }
            if is_class || explicit_function {
                return Err(parser.error("invalid template declaration"));
            }
            let mut groups: Vec<crate::templates::Group> = Vec::new();
            let mut alternative = false;
            while !parser.take(";") {
                let (kind, property) = if let Kind::Quoted { emitting, .. } = parser.token().kind {
                    let literal = parser.token().literal(source)?;
                    parser.cursor += 1;
                    (crate::templates::Kind::String(emitting), literal.text)
                } else if parser.take("[") {
                    let property = parser.name()?;
                    parser.expect("]")?;
                    (crate::templates::Kind::List, property)
                } else if parser.take_word("inherited") {
                    (crate::templates::Kind::Inherited, String::new())
                } else if let Kind::Symbol(
                    marker @ ("@" | "+" | "-" | "*" | "/" | "%" | "->" | "&" | "!" | "~" | ","),
                ) = parser.token().kind
                {
                    parser.cursor += 1;
                    (crate::templates::Kind::Marker(marker), parser.name()?)
                } else {
                    return Err(parser.error("expected a template property item"));
                };
                if kind != crate::templates::Kind::Inherited
                    && (property.is_empty()
                        || !property.chars().enumerate().all(|(i, c)| {
                            c == '_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit()
                        }))
                {
                    return Err(parser.error("template property must be an identifier"));
                }
                let optional = parser.take("?");
                let item = crate::templates::Item { kind, property };
                if alternative {
                    let group = groups
                        .last_mut()
                        .ok_or(parser.error("missing template alternative"))?;
                    group.optional |= optional;
                    push(&mut group.alternatives, item, start)?;
                } else {
                    let mut alternatives = Vec::new();
                    push(&mut alternatives, item, start)?;
                    push(
                        &mut groups,
                        crate::templates::Group {
                            alternatives,
                            optional,
                        },
                        start,
                    )?;
                }
                alternative = parser.take("|");
                if alternative && parser.symbol(";") {
                    return Err(parser.error("missing template alternative"));
                }
            }
            let node = parser.node(
                Syntax::Template(crate::templates::Template {
                    owner: name,
                    groups,
                }),
                start,
                parser.consumed_end(),
            )?;
            push(&mut templates, node, start)?;
            continue;
        }
        let named_object =
            grammar.is_some() || modification || (!explicit_function && parser.take(":"));
        if !explicit_function && (named_object || !parser.symbol("(")) {
            if is_class && containment_depth > 0 {
                return Err(parser.error("class cannot have a containment prefix"));
            }
            let mut parents = Vec::new();
            let modified = if modification {
                let target = objects
                    .iter()
                    .rposition(|id: &Id| {
                        matches!(&parser.nodes[id.0].syntax,
                            Syntax::Object { name: existing, is_dictionary: false, .. }
                            if existing == &name)
                    })
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "parse-expected",
                            "modify requires a defined object or class",
                            start,
                        )
                    })?;
                let Syntax::Object {
                    parents: existing, ..
                } = &parser.nodes[objects[target].0].syntax
                else {
                    unreachable!()
                };
                // Templates in the modification match the target's own parents.
                for parent in existing.clone() {
                    push(&mut parents, parent, start)?;
                }
                Some(target)
            } else {
                None
            };
            if !named_object {
                if is_class {
                    return Err(parser.error("class requires a name and colon"));
                }
                push(&mut parents, name, start)?;
                name = format!("$anonymous{}", objects.len());
            }
            if (named_object && !modification) || parser.take(",") {
                loop {
                    let parent = parser.name()?;
                    push(&mut parents, parent, start)?;
                    if !parser.take(",") {
                        break;
                    }
                }
            }
            let implicit_container = if containment_depth > 0 {
                let property = containment_property
                    .as_ref()
                    .ok_or_else(|| parser.error("containment requires a + property declaration"))?;
                let container = containers
                    .get(containment_depth - 1)
                    .ok_or_else(|| parser.error("containment level has no preceding container"))?;
                Some((property.clone(), container.clone()))
            } else {
                None
            };
            let mut braced = parser.take("{");
            let mut properties = Vec::new();
            let mut arguments = Vec::new();
            loop {
                let kind = match parser.token().kind {
                    Kind::Quoted { emitting, .. }
                    | Kind::StringPart {
                        emitting,
                        head: true,
                        ..
                    } => crate::templates::Kind::String(emitting),
                    Kind::Symbol(
                        marker @ ("@" | "+" | "-" | "*" | "/" | "%" | "->" | "&" | "!" | "~" | ","),
                    ) => {
                        parser.cursor += 1;
                        crate::templates::Kind::Marker(marker)
                    }
                    Kind::Symbol("[") => {
                        return Err(parser.error("list template values are not implemented"));
                    }
                    _ => break,
                };
                let value = parser.expression(15, false)?;
                push(&mut arguments, (kind, value), start)?;
            }
            if !arguments.is_empty() {
                let matched = crate::templates::select(
                    &parser.nodes,
                    &objects,
                    &parents,
                    &templates,
                    &arguments,
                    start,
                )?;
                for (property, value) in matched {
                    if vocabulary_properties.contains(&property) {
                        return Err(parser.error("template vocabulary values are not implemented"));
                    }
                    let value =
                        parser.emitting_property(&name, &property, value, &mut functions)?;
                    push(&mut properties, (property, value), start)?;
                }
            }
            if !braced {
                braced = parser.take("{");
            }
            let mut propertysets: Vec<String> = Vec::new();
            loop {
                if !propertysets.is_empty() && parser.take("}") {
                    propertysets.pop();
                    continue;
                }
                if parser.take_word("propertyset") {
                    let pattern = parser.declaration_string()?;
                    if pattern.chars().filter(|&c| c == '*').count() != 1 {
                        return Err(parser.error("propertyset pattern requires one wildcard"));
                    }
                    if parser.symbol("(") {
                        return Err(
                            parser.error("propertyset common parameters are not implemented")
                        );
                    }
                    parser.expect("{")?;
                    push(&mut propertysets, pattern, start)?;
                    continue;
                }
                if braced && propertysets.is_empty() {
                    if parser.take("}") {
                        parser.take(";");
                        break;
                    }
                } else if propertysets.is_empty() && parser.take(";") {
                    break;
                }
                let owned = parser.take_word("owned");
                let mut property = parser.name()?;
                for pattern in propertysets.iter().rev() {
                    let (prefix, suffix) = pattern.split_once('*').expect("checked pattern");
                    let length = prefix
                        .len()
                        .checked_add(property.len())
                        .and_then(|n| n.checked_add(suffix.len()))
                        .ok_or_else(|| Diagnostic::resource(start))?;
                    let mut expanded = String::new();
                    expanded
                        .try_reserve_exact(length)
                        .map_err(|_| Diagnostic::resource(start))?;
                    expanded.push_str(prefix);
                    expanded.push_str(&property);
                    expanded.push_str(suffix);
                    property = expanded;
                }
                if !propertysets.is_empty()
                    && (!property.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                        || !property
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_'))
                {
                    return Err(parser.error("propertyset expansion must be an identifier"));
                }
                let member_start = parser.byte();
                // `p: Class { q = v... }` defines a nested anonymous object and
                // stores a reference to it, as the reference compiler does.
                if parser.symbol(":") {
                    parser.cursor += 1;
                    let mut nested_parents = Vec::new();
                    loop {
                        push(&mut nested_parents, parser.name()?, member_start)?;
                        if !parser.take(",") {
                            break;
                        }
                    }
                    parser.expect("{")?;
                    let mut nested_properties = Vec::new();
                    while !parser.take("}") {
                        let nested = parser.name()?;
                        if parser.symbol("(") || parser.symbol("{") {
                            return Err(parser.error("nested object methods are not implemented"));
                        }
                        parser.expect("=")?;
                        let value = parser.expression(2, false)?;
                        parser.take(",");
                        push(&mut nested_properties, (nested, value), member_start)?;
                    }
                    let nested_name = format!("$nested{}", objects.len());
                    let node = parser.node(
                        Syntax::Object {
                            name: nested_name.clone(),
                            is_class: false,
                            transient: false,
                            is_dictionary: false,
                            parents: nested_parents,
                            properties: nested_properties,
                        },
                        member_start,
                        parser.consumed_end(),
                    )?;
                    push(&mut objects, node, member_start)?;
                    push(
                        &mut object_dictionaries,
                        active_dictionary.clone(),
                        member_start,
                    )?;
                    let value = parser.node(
                        Syntax::Name(nested_name),
                        member_start,
                        parser.consumed_end(),
                    )?;
                    push(&mut properties, (property, value), start)?;
                    continue;
                }
                // `desk: Thing location(study)` states a relation row.
                // The relation's own name says which table, and the parenthesised
                // form cannot be mistaken for a field: `location = study` stays an
                // ordinary property, so a relation never takes a name away from
                // one. A vocabulary relation is excluded because its forward name
                // is already how words are written.
                let row = relations.iter().enumerate().find_map(|(index, relation)| {
                    if relation.vocabulary {
                        return None;
                    }
                    if relation.forward == property {
                        Some((index, false))
                    } else if relation.reverse.as_deref() == Some(property.as_str()) {
                        Some((index, true))
                    } else {
                        None
                    }
                });
                if let Some((relation, reversed)) = row
                    && parser.symbol("(")
                {
                    parser.cursor += 1;
                    let partner = parser.name()?;
                    let partner =
                        parser.node(Syntax::Name(partner), member_start, parser.consumed_end())?;
                    let label = if parser.take(",") {
                        let name = parser.name()?;
                        Some(parser.node(
                            Syntax::Name(name),
                            member_start,
                            parser.consumed_end(),
                        )?)
                    } else {
                        None
                    };
                    if !parser.take(")") {
                        return Err(parser.error("a declared relation row takes a partner, and a label when the relation has one"));
                    }
                    push(
                        &mut declared_rows,
                        (
                            partner,
                            DeclaredRow {
                                relation,
                                reversed,
                                label,
                            },
                        ),
                        member_start,
                    )?;
                    push(&mut properties, (property, partner), start)?;
                    continue;
                }
                let mut parameters = Vec::new();
                let mut rest = false;
                let method = parser.symbol("(") || parser.symbol("{");
                if method && vocabulary_properties.contains(&property) {
                    return Err(parser.error("vocabulary properties require literal words"));
                }
                if owned && method && property == "construct" {
                    return Err(parser.error("constructors cannot return ownership"));
                }
                let value = if method {
                    let optional;
                    if parser.symbol("(") {
                        (parameters, optional, rest) = parser.formal_parameters(true)?;
                    } else {
                        optional = 0;
                        push(&mut parameters, "self".to_owned(), member_start)?;
                    }
                    if !parser.symbol("{") {
                        return Err(parser.error("method requires a braced body"));
                    }
                    let body = parser.statement()?;
                    let body = if owned {
                        body
                    } else {
                        let ret = parser.node(
                            Syntax::Return(None),
                            member_start,
                            parser.consumed_end(),
                        )?;
                        let mut statements = Vec::new();
                        push(&mut statements, body, member_start)?;
                        push(&mut statements, ret, member_start)?;
                        parser.node(
                            Syntax::Block(statements),
                            member_start,
                            parser.consumed_end(),
                        )?
                    };
                    let function = parser.node(
                        Syntax::Function {
                            returns_owned: owned,
                            optional,
                            rest,
                            is_static: false,
                            name: format!("${name}.{property}"),
                            parameters,
                            body,
                        },
                        member_start,
                        parser.consumed_end(),
                    )?;
                    push(&mut functions, function, member_start)?;
                    function
                } else {
                    parser.expect("=")?;
                    let interpolated_before = parser.interpolated.len();
                    let is_static = parser.take_word("static");
                    let value = if owned {
                        if !is_static || !parser.take_word("new") {
                            return Err(parser
                                .error("owned fields currently require static new construction"));
                        }
                        let expression = parser.expression(15, false)?;
                        let (prototype, arguments) = match &parser.nodes[expression.0].syntax {
                            Syntax::Name(_) => (expression, Vec::new()),
                            Syntax::Call(prototype, arguments)
                                if matches!(parser.nodes[prototype.0].syntax, Syntax::Name(_)) =>
                            {
                                let mut copied = Vec::new();
                                copied
                                    .try_reserve(arguments.len())
                                    .map_err(|_| Diagnostic::resource(member_start))?;
                                copied.extend_from_slice(arguments);
                                (*prototype, copied)
                            }
                            _ => {
                                return Err(parser.error("construction requires a named prototype"));
                            }
                        };
                        let end = parser.consumed_end();
                        let owner =
                            parser.node(Syntax::Name("self".to_owned()), member_start, end)?;
                        let key = parser.node(
                            Syntax::PropertyAddress(property.clone()),
                            member_start,
                            end,
                        )?;
                        if matches!(&parser.nodes[prototype.0].syntax, Syntax::Name(name) if name == "LookupTable")
                        {
                            if !arguments.is_empty() {
                                return Err(parser.error(
                                    "static LookupTable currently requires zero arguments",
                                ));
                            }
                            parser.node(
                                Syntax::StaticLookup {
                                    owner,
                                    property: key,
                                },
                                member_start,
                                end,
                            )?
                        } else if matches!(&parser.nodes[prototype.0].syntax, Syntax::Name(name) if name == "RexPattern")
                        {
                            let [text] = arguments[..] else {
                                return Err(
                                    parser.error("static RexPattern requires one pattern argument")
                                );
                            };
                            parser.node(
                                Syntax::StaticRexPattern {
                                    text,
                                    owner,
                                    property: key,
                                },
                                member_start,
                                end,
                            )?
                        } else if matches!(&parser.nodes[prototype.0].syntax, Syntax::Name(name) if name == "Vector")
                        {
                            if arguments.len() > 1 {
                                return Err(parser.error(
                                    "static Vector supports zero or one capacity argument",
                                ));
                            }
                            let capacity = if let Some(value) = arguments.first() {
                                *value
                            } else {
                                parser.node(
                                    Syntax::Integer("0".to_owned(), 10),
                                    member_start,
                                    end,
                                )?
                            };
                            parser.node(
                                Syntax::StaticVector {
                                    capacity,
                                    owner,
                                    property: key,
                                },
                                member_start,
                                end,
                            )?
                        } else {
                            parser.node(
                                Syntax::Construction {
                                    prototype,
                                    arguments,
                                    owner,
                                    property: key,
                                },
                                member_start,
                                end,
                            )?
                        }
                    } else if vocabulary_properties.contains(&property) {
                        if is_static {
                            return Err(parser.error("vocabulary properties require literal words"));
                        }
                        let mut words = Vec::new();
                        while matches!(
                            parser.token().kind,
                            Kind::Quoted {
                                emitting: false,
                                ..
                            }
                        ) {
                            let word = parser.expression(15, false)?;
                            if !matches!(parser.nodes[word.0].syntax, Syntax::String(_)) {
                                return Err(
                                    parser.error("vocabulary properties require literal words")
                                );
                            }
                            push(&mut words, word, member_start)?;
                        }
                        if words.is_empty() {
                            return Err(parser.error("vocabulary properties require literal words"));
                        }
                        // a labelled vocabulary relation's forward name
                        // takes an adv3Lite vocab string — short name; adjectives;
                        // nouns — and the sections are parts of speech, which the
                        // label gave somewhere to go. The split is a
                        // pure function of the declaration, so it happens here and
                        // costs the built program nothing.
                        let sectioned = relations.iter().any(|relation| {
                            relation.vocabulary && relation.labelled && relation.forward == property
                        });
                        if sectioned {
                            if words.len() != 1 {
                                return Err(parser.error(
                                    "a vocab string is one literal; its sections are separated by semicolons",
                                ));
                            }
                            let Syntax::String(literal) = &parser.nodes[words[0].0].syntax else {
                                return Err(parser.error("a vocab string is a literal"));
                            };
                            let text = literal.text.clone();
                            let split = vocabulary::sections(&text, member_start)?;
                            let end = parser.consumed_end();
                            // One declaration becomes one property per part of
                            // speech, which is the shape the dictionary lowering
                            // already merges through parents.
                            let mut grouped: Vec<(&'static str, Vec<Id>)> = Vec::new();
                            for (part, word) in &split.words {
                                let node = parser.string_word(word, member_start, end)?;
                                match grouped.iter_mut().find(|(name, _)| name == &*part) {
                                    Some((_, list)) => push(list, node, member_start)?,
                                    None => {
                                        let mut list = Vec::new();
                                        push(&mut list, node, member_start)?;
                                        push(&mut grouped, (part, list), member_start)?;
                                    }
                                }
                            }
                            let mut first = None;
                            for (part, list) in grouped {
                                let node = parser.node(Syntax::List(list), member_start, end)?;
                                push(&mut vocabulary_values, node, member_start)?;
                                if !vocabulary_properties.contains(&part.to_owned()) {
                                    push(
                                        &mut vocabulary_properties,
                                        part.to_owned(),
                                        member_start,
                                    )?;
                                }
                                push(&mut properties, (part.to_owned(), node), start)?;
                                first.get_or_insert(node);
                            }
                            // The short name is what the object is called, unless
                            // the declaration says otherwise itself.
                            if !split.name.is_empty()
                                && !properties.iter().any(|(existing, _)| existing == "name")
                            {
                                let node = parser.string_word(&split.name, member_start, end)?;
                                push(&mut properties, ("name".to_owned(), node), start)?;
                            }
                            continue;
                        }
                        let list = parser.node(
                            Syntax::List(words),
                            member_start,
                            parser.consumed_end(),
                        )?;
                        push(&mut vocabulary_values, list, member_start)?;
                        list
                    } else {
                        parser.expression(2, false)?
                    };
                    // a value string with an expression in it is read
                    // when the property is read, not once when the object is
                    // built, so it becomes a method. TADS treats one the same
                    // way, and an initializer has to be a constant here.
                    let value = if !is_static
                        && parser.interpolated.len() > interpolated_before
                        && parser.interpolated[interpolated_before..].contains(&value)
                    {
                        let end = parser.consumed_end();
                        let ret = parser.node(Syntax::Return(Some(value)), member_start, end)?;
                        let mut statements = Vec::new();
                        push(&mut statements, ret, member_start)?;
                        let body = parser.node(Syntax::Block(statements), member_start, end)?;
                        let mut parameters = Vec::new();
                        push(&mut parameters, "self".to_owned(), member_start)?;
                        let function = parser.node(
                            Syntax::Function {
                                returns_owned: false,
                                optional: 0,
                                rest: false,
                                is_static: false,
                                name: format!("${name}.{property}"),
                                parameters,
                                body,
                            },
                            member_start,
                            end,
                        )?;
                        push(&mut functions, function, member_start)?;
                        function
                    } else {
                        value
                    };
                    if is_static {
                        let end = parser.consumed_end();
                        let receiver =
                            parser.node(Syntax::Name("self".to_owned()), member_start, end)?;
                        let target = parser.node(
                            Syntax::Property(receiver, property.clone()),
                            member_start,
                            end,
                        )?;
                        let assign = if owned {
                            value
                        } else {
                            parser.node(Syntax::Binary("=", target, value), member_start, end)?
                        };
                        let body = parser.node(Syntax::Return(Some(assign)), member_start, end)?;
                        push(&mut parameters, "self".to_owned(), member_start)?;
                        let function = parser.node(
                            Syntax::Function {
                                returns_owned: false,
                                optional: 0,
                                rest: false,
                                name: format!("${name}.{property}"),
                                is_static: true,
                                parameters,
                                body,
                            },
                            member_start,
                            end,
                        )?;
                        push(&mut functions, function, member_start)?;
                        function
                    } else {
                        parser.emitting_property(&name, &property, value, &mut functions)?
                    }
                };
                push(&mut properties, (property, value), start)?;
            }
            if let Some((property, container)) = implicit_container
                && !properties.iter().any(|(existing, _)| existing == &property)
            {
                let value = parser.node(Syntax::Name(container), start, parser.consumed_end())?;
                // `+` means containment and nothing else, so when the
                // declared containment property names a relation the prefix
                // states a row rather than writing a field. Unlike
                // `location = study`, there is no second reading for `=` to
                // protect here: a prefix that wrote a field left the relation
                // table not knowing the entity existed, with no diagnostic.
                // A vocabulary relation is refused rather than ignored. Its
                // rows are words, so nesting has no meaning for it, and writing
                // the container into a vocabulary property is a silent no-op
                // that leaves an entity where the language expects a word. This
                // refusal is on the prefix only: `names = 'coin'` is untouched.
                if relations.iter().any(|relation| {
                    relation.vocabulary
                        && (relation.forward == property
                            || relation.reverse.as_deref() == Some(property.as_str()))
                }) {
                    return Err(parser.error(
                        "a vocabulary relation cannot be a containment property, because its rows are words rather than entities",
                    ));
                }
                let row = relations.iter().enumerate().find_map(|(index, relation)| {
                    if relation.vocabulary {
                        return None;
                    }
                    if relation.forward == property {
                        Some((index, false))
                    } else if relation.reverse.as_deref() == Some(property.as_str()) {
                        Some((index, true))
                    } else {
                        None
                    }
                });
                if let Some((relation, reversed)) = row {
                    // A labelled relation needs a third column the prefix has
                    // nowhere to put, so it is refused rather than filed under
                    // whichever table happens to be first.
                    if relations[relation].labelled {
                        return Err(parser.error(
                            "a containment prefix cannot state a labelled relation's row, because it has nowhere to name the label",
                        ));
                    }
                    push(
                        &mut declared_rows,
                        (
                            value,
                            DeclaredRow {
                                relation,
                                reversed,
                                label: None,
                            },
                        ),
                        start,
                    )?;
                }
                push(&mut properties, (property, value), start)?;
            }
            if !is_class && !modification {
                containers.truncate(containment_depth);
                push(&mut containers, name.clone(), start)?;
            }
            let node = parser.node(
                Syntax::Object {
                    name,
                    is_class,
                    transient,
                    is_dictionary: false,
                    parents,
                    properties,
                },
                start,
                parser.consumed_end(),
            )?;
            push(&mut objects, node, start)?;
            push(&mut object_dictionaries, active_dictionary.clone(), start)?;
            if let Some(target) = modified {
                push(&mut modifications, (objects.len() - 1, target), start)?;
            }
            continue;
        }
        if transient {
            return Err(parser.error("transient requires an object declaration"));
        }
        if containment_depth > 0 {
            return Err(parser.error("containment prefix requires an object"));
        }
        if is_class {
            return Err(parser.error("class requires an object definition"));
        }
        let (parameters, optional, rest) = parser.formal_parameters(false)?;
        if !parser.symbol("{") {
            return Err(parser.error("function requires a braced body"));
        }
        let body = parser.statement()?;
        let node = parser.node(
            Syntax::Function {
                returns_owned,
                optional,
                rest,
                is_static: false,
                name,
                parameters,
                body,
            },
            start,
            parser.consumed_end(),
        )?;
        push(&mut functions, node, start)?;
    }
    let end_cursor = parser.cursor;
    let mut next_body = 0;
    while next_body < parser.deferred_bodies.len() {
        let (function, start, end) = parser.deferred_bodies[next_body];
        next_body += 1;
        parser.cursor = start;
        let parsed = parser.statement()?;
        if parser.cursor != end {
            return Err(parser.error("anonymous function body boundary mismatch"));
        }
        let end_byte = parser.consumed_end();
        let ret = parser.node(Syntax::Return(None), end_byte, end_byte)?;
        let mut statements = Vec::new();
        push(&mut statements, parsed, end_byte)?;
        push(&mut statements, ret, end_byte)?;
        let completed = parser.node(
            Syntax::Block(statements),
            parser.tokens[start].byte_start,
            end_byte,
        )?;
        let Syntax::Function { body, .. } = &mut parser.nodes[function.0].syntax else {
            unreachable!()
        };
        *body = completed;
    }
    parser.cursor = end_cursor;
    for function in parser.anonymous.drain(..) {
        push(&mut functions, function, 0)?;
    }
    // Reference layering: the target's original definition becomes a private base
    // class and the modification takes over its name, so `inherited` in the
    // modification reaches the original definitions and existing references,
    // including subclasses declared earlier, see the modified behaviour.
    for (layer, target) in modifications {
        let Syntax::Object {
            name,
            is_class,
            transient,
            ..
        } = &parser.nodes[objects[target].0].syntax
        else {
            unreachable!()
        };
        let base = format!("${name}$modified{target}");
        let (was_class, was_transient) = (*is_class, *transient);
        // Method bodies carry `$object.property` names, so they move with the target.
        let Syntax::Object {
            name, properties, ..
        } = &parser.nodes[objects[target].0].syntax
        else {
            unreachable!()
        };
        let old_name = name.clone();
        let mut renames = Vec::new();
        for (property, value) in properties {
            if matches!(&parser.nodes[value.0].syntax,
                Syntax::Function { name, .. } if name == &format!("${old_name}.{property}"))
            {
                push(&mut renames, (*value, format!("${base}.{property}")), 0)?;
            }
        }
        for (value, renamed) in renames {
            let Syntax::Function { name, .. } = &mut parser.nodes[value.0].syntax else {
                unreachable!()
            };
            *name = renamed;
        }
        // a declared relation row says where **this object** is, so it
        // moves with the name. The target is about to become a private base
        // class, and a row naming a class is a row naming something that is not
        // there: `contains.all` would find it by scanning while `get` and
        // `contains` looked under the name and found nothing.
        let mut carried: Vec<(String, Id)> = Vec::new();
        let Syntax::Object { properties, .. } = &parser.nodes[objects[target].0].syntax else {
            unreachable!()
        };
        for (property, value) in properties {
            if declared_rows.iter().any(|(node, _)| *node == *value) {
                push(&mut carried, (property.clone(), *value), 0)?;
            }
        }
        if !carried.is_empty() {
            let Syntax::Object { properties, .. } = &mut parser.nodes[objects[target].0].syntax
            else {
                unreachable!()
            };
            properties.retain(|(_, value)| !carried.iter().any(|(_, moved)| moved == value));
            let Syntax::Object { properties, .. } = &mut parser.nodes[objects[layer].0].syntax
            else {
                unreachable!()
            };
            for (property, value) in carried {
                // A modification that states its own row wins; it is the later
                // word on where the object is.
                if !properties.iter().any(|(existing, _)| *existing == property) {
                    properties
                        .try_reserve(1)
                        .map_err(|_| Diagnostic::resource(0))?;
                    properties.push((property, value));
                }
            }
        }
        let Syntax::Object { name, is_class, .. } = &mut parser.nodes[objects[target].0].syntax
        else {
            unreachable!()
        };
        *name = base.clone();
        *is_class = true;
        let Syntax::Object {
            is_class,
            transient,
            parents,
            ..
        } = &mut parser.nodes[objects[layer].0].syntax
        else {
            unreachable!()
        };
        *is_class = was_class;
        *transient = was_transient;
        parents.clear();
        push(parents, base, 0)?;
    }
    let mut vocabulary_statements = parser.lower_vocabulary(
        &mut objects,
        &object_dictionaries,
        &vocabulary_values,
        &mut functions,
    )?;
    let mut initializers = Vec::new();
    for object in &objects {
        let Syntax::Object {
            name, properties, ..
        } = &parser.nodes[object.0].syntax
        else {
            unreachable!()
        };
        for (property, value) in properties {
            if matches!(
                parser.nodes[value.0].syntax,
                Syntax::Function {
                    returns_owned: false,
                    optional: 0,
                    rest: false,
                    is_static: true,
                    ..
                }
            ) {
                push(
                    &mut initializers,
                    (
                        name.clone(),
                        property.clone(),
                        parser.nodes[value.0].start,
                        parser.nodes[value.0].end,
                    ),
                    0,
                )?;
            }
        }
    }
    if !initializers.is_empty() || !vocabulary_statements.is_empty() {
        let mut statements = Vec::new();
        statements.append(&mut vocabulary_statements);
        for (name, property, start, end) in initializers {
            let receiver = parser.node(Syntax::Name(name), start, end)?;
            let access = parser.node(Syntax::Property(receiver, property), start, end)?;
            let statement = parser.node(Syntax::Expression(access), start, end)?;
            push(&mut statements, statement, start)?;
        }
        let end = parser.byte();
        let ret = parser.node(Syntax::Return(None), end, end)?;
        push(&mut statements, ret, end)?;
        let body = parser.node(Syntax::Block(statements), end, end)?;
        let function = parser.node(
            Syntax::Function {
                returns_owned: false,
                optional: 0,
                rest: false,
                name: "$initializers".to_owned(),
                is_static: false,
                parameters: Vec::new(),
                body,
            },
            end,
            end,
        )?;
        push(&mut functions, function, end)?;
    }
    // the preinits run in an order settled here, not at startup. The
    // synthesized body is a flat sequence of `name.execute` calls; a cycle or a
    // name nothing answers to is refused before any of it is built.
    let preinit_order = crate::preinit::order(&parser.nodes, &objects)?;
    if !preinit_order.is_empty() {
        let end = parser.byte();
        let mut statements = Vec::new();
        for name in preinit_order {
            let receiver = parser.node(Syntax::Name(name), end, end)?;
            let access = parser.node(
                Syntax::Property(receiver, crate::preinit::EXECUTE.to_owned()),
                end,
                end,
            )?;
            let statement = parser.node(Syntax::Expression(access), end, end)?;
            push(&mut statements, statement, end)?;
        }
        let ret = parser.node(Syntax::Return(None), end, end)?;
        push(&mut statements, ret, end)?;
        let body = parser.node(Syntax::Block(statements), end, end)?;
        let function = parser.node(
            Syntax::Function {
                returns_owned: false,
                optional: 0,
                rest: false,
                name: "$preinit".to_owned(),
                is_static: false,
                parameters: Vec::new(),
                body,
            },
            end,
            end,
        )?;
        push(&mut functions, function, end)?;
    }
    let deferred = !parser.deferred_bodies.is_empty();
    let mut rows: Vec<Option<DeclaredRow>> = Vec::new();
    rows.try_reserve_exact(parser.nodes.len())
        .map_err(|_| Diagnostic::resource(0))?;
    rows.resize(parser.nodes.len(), None);
    for (value, row) in declared_rows {
        rows[value.0] = Some(row);
    }
    let ast = Ast {
        nodes: parser.nodes,
        functions,
        objects,
        relations,
        declared_rows: rows,
        lifetimes,
    };
    if deferred {
        syntax_order::restore(ast)
    } else {
        Ok(ast)
    }
}
