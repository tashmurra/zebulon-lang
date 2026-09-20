//! Positional object properties, resolved before normal property lowering.
use crate::{
    Diagnostic,
    parser::{Id, Node, Syntax},
};
use std::collections::HashSet;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    String(bool),
    Marker(&'static str),
    List,
    Inherited,
}
#[derive(Debug)]
pub struct Item {
    pub kind: Kind,
    pub property: String,
}
#[derive(Debug)]
pub struct Group {
    pub alternatives: Vec<Item>,
    pub optional: bool,
}
#[derive(Debug)]
pub struct Template {
    pub owner: String,
    pub groups: Vec<Group>,
}
fn push<T>(v: &mut Vec<T>, value: T) -> Result<(), Diagnostic> {
    v.try_reserve(1).map_err(|_| Diagnostic::resource(0))?;
    v.push(value);
    Ok(())
}
fn copy(s: &str) -> Result<String, Diagnostic> {
    let mut out = String::new();
    out.try_reserve(s.len())
        .map_err(|_| Diagnostic::resource(0))?;
    out.push_str(s);
    Ok(out)
}
pub fn select(
    nodes: &[Node],
    objects: &[Id],
    parents: &[String],
    templates: &[Id],
    arguments: &[(Kind, Id)],
    site: usize,
) -> Result<Vec<(String, Id)>, Diagnostic> {
    // Same last-occurrence superclass order used for runtime lookup. Object
    // definitions already parsed supply ancestor relationships.
    let mut pending = Vec::new();
    for parent in parents {
        push(&mut pending, (parent.as_str(), false))?;
    }
    let mut visited = HashSet::new();
    let mut order = Vec::new();
    let mut unresolved = Vec::new();
    while let Some((name, finish)) = pending.pop() {
        if finish {
            push(&mut order, name)?;
            continue;
        }
        if visited.contains(name) {
            continue;
        }
        visited
            .try_reserve(1)
            .map_err(|_| Diagnostic::resource(site))?;
        visited.insert(name);
        push(&mut pending, (name, true))?;
        if let Some(node) = objects
            .iter()
            .map(|id| &nodes[id.0])
            .find(|n| matches!(&n.syntax,Syntax::Object{name:n,..} if n==name))
        {
            let Syntax::Object { parents, .. } = &node.syntax else {
                unreachable!()
            };
            for parent in parents {
                push(&mut pending, (parent.as_str(), false))?;
            }
        } else if name != "object" {
            push(&mut unresolved, name)?;
        }
    }
    order.reverse();
    if !order.contains(&"object") {
        push(&mut order, "object")?;
    }
    for owner in order {
        for id in templates {
            let Syntax::Template(template) = &nodes[id.0].syntax else {
                unreachable!()
            };
            if template.owner != owner {
                continue;
            }
            if template
                .groups
                .iter()
                .flat_map(|g| &g.alternatives)
                .any(|a| a.kind == Kind::Inherited)
            {
                return Err(Diagnostic::new(
                    "template-unavailable",
                    "inherited template expansion is not implemented",
                    site,
                ));
            }
            let mut states: Vec<(usize, usize, Option<usize>)> = Vec::new();
            let mut paths: Vec<(Option<usize>, usize, usize, usize)> = Vec::new();
            let mut seen = HashSet::new();
            push(&mut states, (0usize, 0usize, None))?;
            while let Some((group, arg, path)) = states.pop() {
                if seen.contains(&(group, arg)) {
                    continue;
                }
                seen.try_reserve(1)
                    .map_err(|_| Diagnostic::resource(site))?;
                seen.insert((group, arg));
                if group == template.groups.len() {
                    if arg != arguments.len() {
                        continue;
                    }
                    let mut result = Vec::new();
                    let mut at = path;
                    while let Some(index) = at {
                        let (previous, g, a, v) = paths[index];
                        push(
                            &mut result,
                            (
                                copy(&template.groups[g].alternatives[a].property)?,
                                arguments[v].1,
                            ),
                        )?;
                        at = previous;
                    }
                    result.reverse();
                    return Ok(result);
                }
                let item = &template.groups[group];
                if item.optional {
                    push(&mut states, (group + 1, arg, path))?;
                }
                if let Some((kind, _)) = arguments.get(arg) {
                    for (alternative, entry) in item.alternatives.iter().enumerate().rev() {
                        if entry.kind == *kind {
                            let index = paths.len();
                            push(&mut paths, (path, group, alternative, arg))?;
                            push(&mut states, (group + 1, arg + 1, Some(index)))?;
                        }
                    }
                }
            }
        }
        if unresolved.contains(&owner) {
            return Err(Diagnostic::new(
                "template-unavailable",
                "template ancestry needs this superclass definition before use",
                site,
            ));
        }
    }
    Err(Diagnostic::new(
        "template-match",
        "no supported template matches these object values",
        site,
    ))
}
