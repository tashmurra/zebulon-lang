//! Strict reader for the pinned LLVM print-callgraph diagnostic, used only by
//! candidate frame admission. Artifact identity must be checked by its caller.
use std::collections::{BTreeMap, BTreeSet};

fn address(s: &str) -> bool {
    s.strip_prefix("0x")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_hexdigit()))
}
fn header_tail(s: &str) -> bool {
    s.strip_prefix("<<")
        .and_then(|s| s.split_once(">>  #uses="))
        .is_some_and(|(a, n)| address(a) && !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// Require a closed graph matching all native text definitions. The single
/// retained Rust classifier is an isolated leaf, never a generated-code callee.
/// This checks LLVM call closure, not machine instructions or stack availability.
pub(crate) fn validate(
    report: &str,
    definitions: &BTreeSet<&str>,
    runtime_leaf: &str,
) -> Result<(), String> {
    let (nodes, external) = parse(report, definitions)?;
    let exports: BTreeSet<_> = definitions
        .iter()
        .copied()
        .filter(|n| *n == runtime_leaf || n.starts_with("zeb_stack_candidate_"))
        .collect();
    if exports.len() != 2 || external != exports {
        return Err("unexpected candidate external entry set".into());
    }
    if !nodes.get(runtime_leaf).is_some_and(BTreeSet::is_empty) {
        return Err("runtime export is not a leaf".into());
    }
    for callees in nodes.values() {
        if callees
            .iter()
            .any(|n| !(n.starts_with("zfn") || n.starts_with("zsp")))
        {
            return Err("uncovered runtime or wrapper call".into());
        }
    }
    Ok(())
}

type Nodes<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

fn parse<'a>(
    report: &'a str,
    definitions: &BTreeSet<&str>,
) -> Result<(Nodes<'a>, BTreeSet<&'a str>), String> {
    let mut nodes: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut external = BTreeSet::new();
    let mut current = None;
    let mut external_seen = false;
    let mut section_seen = false;
    for line in report.lines().filter(|s| !s.is_empty()) {
        if let Some(tail) = line.strip_prefix("Call graph node <<null function>>") {
            if external_seen || !header_tail(tail) {
                return Err("invalid external callgraph node".into());
            }
            external_seen = true;
            section_seen = true;
            current = None;
        } else if let Some(rest) = line.strip_prefix("Call graph node for function: '") {
            let (name, tail) = rest.split_once('\'').ok_or("invalid callgraph function")?;
            if !definitions.contains(name)
                || !header_tail(tail)
                || nodes.insert(name, BTreeSet::new()).is_some()
            {
                return Err("unknown or duplicate callgraph function".into());
            }
            section_seen = true;
            current = Some(name);
        } else if let Some(rest) = line.strip_prefix("  CS<") {
            let (site, callee) = rest
                .split_once("> calls function '")
                .ok_or("unresolved or malformed callgraph edge")?;
            let callee = callee
                .strip_suffix('\'')
                .ok_or("invalid callgraph callee")?;
            if !section_seen || !definitions.contains(callee) {
                return Err("unknown callgraph callee".into());
            }
            if let Some(caller) = current {
                if !address(site) {
                    return Err("invalid native callgraph site".into());
                }
                nodes
                    .get_mut(caller)
                    .ok_or("missing callgraph caller")?
                    .insert(callee);
            } else {
                if site != "None" {
                    return Err("invalid external callgraph site".into());
                }
                external.insert(callee);
            }
        } else {
            return Err("unrecognized callgraph report line".into());
        }
    }
    if !external_seen || nodes.keys().copied().collect::<BTreeSet<_>>() != *definitions {
        return Err("incomplete callgraph coverage".into());
    }
    Ok((nodes, external))
}

/// A separate runtime object may currently contain only the classifier leaf.
pub(crate) fn validate_leaf(
    report: &str,
    definitions: &BTreeSet<&str>,
    leaf: &str,
) -> Result<(), String> {
    let (nodes, external) = parse(report, definitions)?;
    if *definitions != BTreeSet::from([leaf])
        || external != BTreeSet::from([leaf])
        || !nodes.get(leaf).is_some_and(BTreeSet::is_empty)
    {
        return Err("runtime classifier is not an isolated single leaf".into());
    }
    Ok(())
}
