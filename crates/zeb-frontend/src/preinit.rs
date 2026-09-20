#![forbid(unsafe_code)]
//! preinit is ordered at build time.
//!
//! adv3Lite runs a set of `PreinitObject`s before play begins, each with an
//! `execBeforeMe` list naming the ones that must run first, and it derives the
//! running order at every startup. That order is a property of the declarations
//! and nothing else, so an ahead-of-time compiler can settle it once: this
//! module topologically sorts the preinits while compiling, and the emitted
//! program runs a flat sequence with no sort in it.
//!
//! Two consequences follow, and both are improvements on the thing being
//! copied. A dependency cycle is a compile error rather than a startup
//! misorder, and a name that no preinit answers to is refused rather than
//! silently ignored.
//!
//! Expectations are self-authored. The spelling is adv3Lite's so that a
//! tutorial's `PreinitObject` pastes and works, but the mechanism is not, and
//! there is no reference to compare an ordering against.
use crate::Diagnostic;
use crate::parser::{Id, Node, Syntax};
use std::collections::HashMap;

/// The class an object derives from to become a preinit. adv3Lite's spelling,
/// kept so that source written against its tutorials compiles unchanged.
pub const PREINIT_CLASS: &str = "PreinitObject";

/// The property naming the preinits that must run before this one.
pub const BEFORE_ME: &str = "execBeforeMe";

/// The method a preinit runs.
pub const EXECUTE: &str = "execute";

struct Preinit<'a> {
    name: &'a str,
    /// Names that must run before this one, with the byte each was written at.
    before: Vec<(&'a str, usize)>,
    /// Where the declaration begins, so a refusal points at it.
    start: usize,
}

/// The parts of an object declaration this module reads.
struct Declared<'a> {
    name: &'a str,
    is_class: bool,
    parents: &'a [String],
    properties: &'a [(String, Id)],
}

fn object(nodes: &[Node], id: Id) -> Option<Declared<'_>> {
    match &nodes[id.0].syntax {
        Syntax::Object {
            name,
            is_class,
            parents,
            properties,
            ..
        } => Some(Declared {
            name: name.as_str(),
            is_class: *is_class,
            parents,
            properties,
        }),
        _ => None,
    }
}

/// Whether `name` reaches [`PREINIT_CLASS`] through its declared parents. The
/// walk is depth-bounded by the number of declarations, so a parent cycle in
/// malformed source terminates rather than spinning.
fn derives_from_preinit(parents_of: &HashMap<&str, &[String]>, name: &str, budget: usize) -> bool {
    if budget == 0 {
        return false;
    }
    match parents_of.get(name) {
        None => false,
        Some(parents) => parents.iter().any(|parent| {
            parent == PREINIT_CLASS || derives_from_preinit(parents_of, parent, budget - 1)
        }),
    }
}

/// The preinits of a program, in the order they must run.
///
/// Answers an empty list when the program declares no `PreinitObject`, which is
/// every program that does not use the library.
pub fn order(nodes: &[Node], objects: &[Id]) -> Result<Vec<String>, Diagnostic> {
    let mut parents_of: HashMap<&str, &[String]> = HashMap::new();
    for id in objects {
        if let Some(declared) = object(nodes, *id) {
            parents_of.insert(declared.name, declared.parents);
        }
    }
    if !parents_of.contains_key(PREINIT_CLASS) {
        return Ok(Vec::new());
    }
    let budget = objects.len().saturating_add(1);
    let mut preinits: Vec<Preinit> = Vec::new();
    for id in objects {
        let Some(declared) = object(nodes, *id) else {
            continue;
        };
        // A class is a declaration, not something to run. Only instances do.
        if declared.is_class || !derives_from_preinit(&parents_of, declared.name, budget) {
            continue;
        }
        let mut before = Vec::new();
        if let Some((_, value)) = declared
            .properties
            .iter()
            .find(|(property, _)| property == BEFORE_ME)
        {
            match &nodes[value.0].syntax {
                Syntax::List(items) => {
                    for item in items {
                        match &nodes[item.0].syntax {
                            Syntax::Name(named) => {
                                before.push((named.as_str(), nodes[item.0].start));
                            }
                            _ => {
                                return Err(Diagnostic::new(
                                    "sem-preinit",
                                    "execBeforeMe lists the preinits that run first, so every element names one",
                                    nodes[item.0].start,
                                ));
                            }
                        }
                    }
                }
                _ => {
                    return Err(Diagnostic::new(
                        "sem-preinit",
                        "execBeforeMe is a list of preinit names",
                        nodes[value.0].start,
                    ));
                }
            }
        }
        preinits.push(Preinit {
            name: declared.name,
            before,
            start: nodes[id.0].start,
        });
    }
    if preinits.is_empty() {
        return Ok(Vec::new());
    }
    let index: HashMap<&str, usize> = preinits
        .iter()
        .enumerate()
        .map(|(at, preinit)| (preinit.name, at))
        .collect();
    // A name nothing answers to is refused. adv3Lite ignores one, which turns a
    // typo in a dependency list into an ordering that is wrong and silent.
    for preinit in &preinits {
        for (named, byte) in &preinit.before {
            if !index.contains_key(named) {
                return Err(Diagnostic::new(
                    "sem-preinit",
                    "execBeforeMe names something that is not a preinit",
                    *byte,
                ));
            }
        }
    }
    // Depth-first postorder, which yields dependencies before their dependents.
    // Iterative rather than recursive: a program's preinit graph is author data
    // and must not decide how much stack the compiler uses.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Fresh,
        Active,
        Done,
    }
    let mut mark = vec![Mark::Fresh; preinits.len()];
    let mut ordered: Vec<String> = Vec::new();
    ordered
        .try_reserve_exact(preinits.len())
        .map_err(|_| Diagnostic::resource(preinits[0].start))?;
    for root in 0..preinits.len() {
        if mark[root] != Mark::Fresh {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        while let Some((at, step)) = stack.pop() {
            if step == 0 {
                if mark[at] == Mark::Done {
                    continue;
                }
                if mark[at] == Mark::Active {
                    return Err(Diagnostic::new(
                        "sem-preinit",
                        "execBeforeMe forms a cycle, so no order can run every preinit first",
                        preinits[at].start,
                    ));
                }
                mark[at] = Mark::Active;
            }
            if step < preinits[at].before.len() {
                stack.push((at, step + 1));
                let next = index[preinits[at].before[step].0];
                if mark[next] == Mark::Active {
                    return Err(Diagnostic::new(
                        "sem-preinit",
                        "execBeforeMe forms a cycle, so no order can run every preinit first",
                        preinits[next].start,
                    ));
                }
                stack.push((next, 0));
            } else {
                mark[at] = Mark::Done;
                ordered.push(preinits[at].name.to_owned());
            }
        }
    }
    Ok(ordered)
}
