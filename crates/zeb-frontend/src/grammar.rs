//! Resolve source productions and lower groups without expanding their Cartesian product.
use crate::{
    Diagnostic,
    parser::{Ast, Declaration, GrammarItem, Syntax},
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductionId(pub usize);
#[derive(Debug, PartialEq, Eq)]
pub enum Term<'a> {
    Production(ProductionId),
    Literal(&'a str),
    /// A token enumerator or dictionary part of speech, classified at emission.
    Symbol(&'a str),
    Wildcard,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Element<'a> {
    pub term: Term<'a>,
    pub capture: Option<&'a str>,
}
#[derive(Debug)]
pub struct Rule<'a> {
    pub production: ProductionId,
    pub elements: Vec<Element<'a>>,
    /// None marks a grouping production, which must not create a game match object.
    pub match_name: Option<&'a str>,
    /// Retained for command ranking; matching itself ignores it.
    pub badness: i32,
    pub byte: usize,
}
#[derive(Debug)]
pub struct Grammar<'a> {
    pub productions: Vec<Option<&'a str>>,
    pub rules: Vec<Rule<'a>>,
}
fn push<T>(v: &mut Vec<T>, item: T, byte: usize) -> Result<(), Diagnostic> {
    v.try_reserve(1).map_err(|_| Diagnostic::resource(byte))?;
    v.push(item);
    Ok(())
}
impl<'a> Grammar<'a> {
    fn production(
        &mut self,
        name: Option<&'a str>,
        byte: usize,
    ) -> Result<ProductionId, Diagnostic> {
        let id = ProductionId(self.productions.len());
        push(&mut self.productions, name, byte)?;
        Ok(id)
    }
}
/// Preserves source order, duplicate alternatives, empty alternatives and capture positions.
/// Non-production symbols stay unresolved here; `rust_data` classifies them against
/// token enumerators and dictionary properties rather than guessing from spelling.
pub fn lower(ast: &Ast) -> Result<Grammar<'_>, Diagnostic> {
    let mut grammar = Grammar {
        productions: Vec::new(),
        rules: Vec::new(),
    };
    let mut names = HashMap::new();
    let mut matches = HashMap::new();
    for node in &ast.nodes {
        let Syntax::Declaration(Declaration::Grammar(rule)) = &node.syntax else {
            continue;
        };
        if !names.contains_key(rule.production.as_str()) {
            names
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(node.start))?;
            names.insert(
                rule.production.as_str(),
                grammar.production(Some(&rule.production), node.start)?,
            );
        }
        if let Some(name) = &rule.match_name {
            matches
                .try_reserve(1)
                .map_err(|_| Diagnostic::resource(node.start))?;
            if matches.insert(name.as_str(), node.start).is_some() {
                return Err(Diagnostic::new(
                    "grammar-duplicate",
                    "duplicate grammar match-object name",
                    node.start,
                ));
            }
        }
    }
    for node in &ast.nodes {
        let Syntax::Declaration(Declaration::Grammar(source)) = &node.syntax else {
            continue;
        };
        let Some(name) = source.match_name.as_deref() else {
            continue;
        };
        let mut stack = Vec::new();
        push(
            &mut stack,
            Rule {
                production: names[source.production.as_str()],
                elements: Vec::new(),
                match_name: Some(name),
                badness: 0,
                byte: node.start,
            },
            node.start,
        )?;
        for item in &source.items {
            if matches!(item, GrammarItem::Open) {
                let id = grammar.production(None, node.start)?;
                let current = stack.last_mut().ok_or_else(|| {
                    Diagnostic::new("grammar-shape", "missing grammar sequence", node.start)
                })?;
                push(
                    &mut current.elements,
                    Element {
                        term: Term::Production(id),
                        capture: None,
                    },
                    node.start,
                )?;
                push(
                    &mut stack,
                    Rule {
                        production: id,
                        elements: Vec::new(),
                        match_name: None,
                        badness: 0,
                        byte: node.start,
                    },
                    node.start,
                )?;
                continue;
            }
            if matches!(item, GrammarItem::Close) {
                if stack.len() <= 1 {
                    return Err(Diagnostic::new(
                        "grammar-shape",
                        "unmatched grammar group",
                        node.start,
                    ));
                }
                push(
                    &mut grammar.rules,
                    stack.pop().expect("checked group"),
                    node.start,
                )?;
                continue;
            }
            let depth = stack.len();
            let current = stack.last_mut().ok_or_else(|| {
                Diagnostic::new("grammar-shape", "missing grammar sequence", node.start)
            })?;
            if matches!(item, GrammarItem::Alternative) {
                let next = Rule {
                    production: current.production,
                    elements: Vec::new(),
                    match_name: current.match_name,
                    badness: 0,
                    byte: node.start,
                };
                push(
                    &mut grammar.rules,
                    std::mem::replace(current, next),
                    node.start,
                )?;
                continue;
            }
            if let GrammarItem::Badness(value) = item {
                if depth != 1 || !current.elements.is_empty() {
                    return Err(Diagnostic::new(
                        "grammar-shape",
                        "badness must begin a top-level alternative",
                        node.start,
                    ));
                }
                current.badness = *value;
                continue;
            }
            if let GrammarItem::Capture(property) = item {
                let element = current.elements.last_mut().ok_or_else(|| {
                    Diagnostic::new("grammar-shape", "capture has no preceding term", node.start)
                })?;
                if element.capture.replace(property).is_some() {
                    return Err(Diagnostic::new(
                        "grammar-shape",
                        "duplicate term capture",
                        node.start,
                    ));
                }
                continue;
            }
            let term = match item {
                GrammarItem::Symbol(symbol) => names
                    .get(symbol.as_str())
                    .map_or(Term::Symbol(symbol), |id| Term::Production(*id)),
                GrammarItem::Literal(value) => Term::Literal(value),
                GrammarItem::Wildcard => Term::Wildcard,
                _ => unreachable!("structural items handled above"),
            };
            push(
                &mut current.elements,
                Element {
                    term,
                    capture: None,
                },
                node.start,
            )?;
        }
        if stack.len() != 1 {
            return Err(Diagnostic::new(
                "grammar-shape",
                "unclosed grammar group",
                node.start,
            ));
        }
        push(
            &mut grammar.rules,
            stack.pop().expect("checked root"),
            node.start,
        )?;
    }
    Ok(grammar)
}

fn text(out: &mut String, value: &str, byte: usize) -> Result<(), Diagnostic> {
    out.try_reserve(value.len())
        .map_err(|_| Diagnostic::resource(byte))?;
    out.push_str(value);
    Ok(())
}
fn option(value: Option<u32>) -> String {
    value.map_or_else(|| "None".to_owned(), |v| format!("Some({v})"))
}

/// Generated immutable runtime tables. Productions and match objects use their
/// static object slots; captures and token properties use object property IDs.
pub fn rust_data(ast: &Ast) -> Result<String, Diagnostic> {
    let grammar = lower(ast)?;
    let plan = crate::object_init::declarations(ast)?;
    let enums = crate::sema::enumerators(ast)?;
    let property = |name: &str| {
        plan.properties
            .iter()
            .position(|p| p == name)
            .and_then(|i| u32::try_from(i).ok())
    };
    let mut slots = HashMap::new();
    slots
        .try_reserve(ast.objects.len())
        .map_err(|_| Diagnostic::resource(0))?;
    for (index, id) in ast.objects.iter().enumerate() {
        if let Syntax::Object { name, .. } = &ast.nodes[id.0].syntax {
            let slot = u32::try_from(index).map_err(|_| Diagnostic::resource(0))?;
            slots.insert(name.as_str(), slot);
        }
    }
    let mut vocabulary = HashSet::new();
    let mut tokens = HashSet::new();
    // a vocabulary relation's slot binds the entities a word names,
    // not the word. The dictionary is the index either way.
    let relations: HashSet<&str> = ast
        .relations
        .iter()
        .filter(|relation| relation.vocabulary && !relation.labelled)
        .map(|relation| relation.forward.as_str())
        .collect();
    // a labelled vocabulary relation files its words under the part of
    // speech its label names, so its forward name indexes nothing. A slot has
    // nowhere to say which part it wants, so one written against it is refused
    // rather than compiled into a match that silently never fires.
    let labelled_vocabulary: HashSet<&str> = ast
        .relations
        .iter()
        .filter(|relation| relation.vocabulary && relation.labelled)
        .map(|relation| relation.forward.as_str())
        .collect();
    for node in &ast.nodes {
        match &node.syntax {
            Syntax::Declaration(Declaration::VocabularyProperties(names)) => {
                vocabulary
                    .try_reserve(names.len())
                    .map_err(|_| Diagnostic::resource(node.start))?;
                vocabulary.extend(names.iter().map(String::as_str));
            }
            Syntax::Declaration(Declaration::Enumerators {
                names, token: true, ..
            }) => {
                tokens
                    .try_reserve(names.len())
                    .map_err(|_| Diagnostic::resource(node.start))?;
                tokens.extend(names.iter().map(String::as_str));
            }
            _ => {}
        }
    }
    let mut out = String::new();
    text(
        &mut out,
        "pub static GRAMMAR: crate::grammar::StaticGrammar = crate::grammar::StaticGrammar {\n    productions: &[",
        0,
    )?;
    for name in &grammar.productions {
        let slot = match name {
            Some(name) => Some(*slots.get(name).ok_or_else(|| {
                Diagnostic::new(
                    "grammar-production-object",
                    "grammar production has no static object",
                    0,
                )
            })?),
            None => None,
        };
        text(&mut out, &option(slot), 0)?;
        text(&mut out, ", ", 0)?;
    }
    text(&mut out, "],\n    rules: &[\n", 0)?;
    for rule in &grammar.rules {
        let match_slot = match rule.match_name {
            Some(name) => Some(*slots.get(name).ok_or_else(|| {
                Diagnostic::new(
                    "grammar-match-object",
                    "grammar rule has no match object",
                    rule.byte,
                )
            })?),
            None => None,
        };
        text(
            &mut out,
            &format!(
                "        crate::grammar::StaticRule {{ production: {}, match_slot: {}, badness: {}, elements: &[",
                rule.production.0,
                option(match_slot),
                rule.badness
            ),
            rule.byte,
        )?;
        for element in &rule.elements {
            let term = match element.term {
                Term::Production(id) => format!("Production({})", id.0),
                Term::Wildcard => "Star".to_owned(),
                Term::Literal(value) => {
                    let mut escaped = String::new();
                    for ch in value.chars().flat_map(char::escape_default) {
                        escaped
                            .try_reserve(ch.len_utf8())
                            .map_err(|_| Diagnostic::resource(rule.byte))?;
                        escaped.push(ch);
                    }
                    format!("Literal(\"{escaped}\")")
                }
                Term::Symbol(name) if tokens.contains(name) => {
                    format!("Token({})", enums[name])
                }
                Term::Symbol(name) if relations.contains(name) => {
                    format!(
                        "Vocabulary({})",
                        property(name).expect("declared vocabulary")
                    )
                }
                // `vocab.noun` names one part of speech of a labelled
                // vocabulary relation. It matches like any other vocabulary slot;
                // only the property it looks under is chosen by the author.
                Term::Symbol(name)
                    if name
                        .split_once('.')
                        .is_some_and(|(head, _)| labelled_vocabulary.contains(head)) =>
                {
                    let (_, part) = name.split_once('.').expect("checked above");
                    let identity = property(part).ok_or_else(|| {
                        Diagnostic::new(
                            "sem-vocabulary",
                            "a grammar slot's part of speech is a declared property",
                            rule.byte,
                        )
                    })?;
                    format!("Vocabulary({identity})")
                }
                // The bare name has no part of speech, and a slot that matched
                // every part would bind an entity by its adjective — `take oak`
                // would answer the desk, which is what parts of speech exist to
                // prevent. So it names one or it is refused.
                Term::Symbol(name) if labelled_vocabulary.contains(name) => {
                    return Err(Diagnostic::new(
                        "sem-vocabulary",
                        "a grammar slot names one part of speech of a labelled vocabulary relation, written vocab.noun",
                        rule.byte,
                    ));
                }
                Term::Symbol(name) if vocabulary.contains(name) => {
                    format!("Speech({})", property(name).expect("declared vocabulary"))
                }
                Term::Symbol(_) => {
                    return Err(Diagnostic::new(
                        "grammar-symbol-unavailable",
                        "grammar symbol is not a production, token enumerator or dictionary property",
                        rule.byte,
                    ));
                }
            };
            let capture = match element.capture {
                Some(name) => Some(property(name).ok_or_else(|| {
                    Diagnostic::new(
                        "grammar-capture",
                        "grammar capture has no property identity",
                        rule.byte,
                    )
                })?),
                None => None,
            };
            text(
                &mut out,
                &format!(
                    "crate::grammar::StaticElement {{ term: crate::grammar::StaticTerm::{term}, capture: {} }}, ",
                    option(capture)
                ),
                rule.byte,
            )?;
        }
        text(&mut out, "] },\n", rule.byte)?;
    }
    text(
        &mut out,
        &format!(
            "    ],\n    badness: {},\n    first_token_index: {},\n    last_token_index: {},\n    token_list: {},\n}};\n",
            option(property("badness")),
            option(property("firstTokenIndex")),
            option(property("lastTokenIndex")),
            option(property("tokenList"))
        ),
        0,
    )?;
    Ok(out)
}
