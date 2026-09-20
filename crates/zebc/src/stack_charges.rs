//! Candidate charge refinement from actual object reports; not a stack certificate.
use std::collections::{BTreeMap, BTreeSet};
use zeb_frontend::llvm::{StackCharge, Target};

pub struct Refinement {
    pub charges: Vec<StackCharge>,
    pub changed: bool,
    /// These final wrapper requirements must be covered by the host separately.
    pub wrappers: BTreeMap<String, u64>,
    /// Retained exports proved disconnected from generated-code calls.
    pub detached_exports: BTreeMap<String, u64>,
}

/// A stable candidate and the artifact measured in its final compilation round.
/// This binds iteration state only; callers still authenticate native artifacts,
/// validate call closure and establish physical host capacity before publication.
pub struct StableCandidate<A> {
    pub round: usize,
    pub refinement: Refinement,
    pub artifact: A,
}

/// Recompile after every charge change and return only a stable measured round.
/// Four attempts bound code-generation work; the caller also bounds each tool.
/// The callback must measure the artifact it returns, without publishing it.
pub fn refine_bounded<A>(
    initial: &[StackCharge],
    mut compile: impl FnMut(usize, &[StackCharge]) -> Result<(Refinement, A), String>,
) -> Result<StableCandidate<A>, String> {
    fn valid(charges: &[StackCharge]) -> bool {
        !charges.is_empty()
            && charges.iter().all(|c| {
                c.general > 0
                    && c.integer > 0
                    && c.general.is_multiple_of(16)
                    && c.integer.is_multiple_of(16)
            })
    }
    if !valid(initial) {
        return Err("invalid initial candidate charges".into());
    }
    let mut charges = initial.to_vec();
    for round in 0..4 {
        let (refinement, artifact) = compile(round, &charges)?;
        if refinement.charges.len() != charges.len() || !valid(&refinement.charges) {
            return Err("invalid remeasured candidate charges".into());
        }
        let mut changed = false;
        for (before, after) in charges.iter().zip(&refinement.charges) {
            if after.general < before.general || after.integer < before.integer {
                return Err("candidate charge refinement decreased a reservation".into());
            }
            changed |= before.general != after.general || before.integer != after.integer;
        }
        if changed != refinement.changed {
            return Err("candidate charge change flag disagrees with remeasurement".into());
        }
        if !changed {
            return Ok(StableCandidate {
                round,
                refinement,
                artifact,
            });
        }
        charges = refinement.charges;
    }
    Err("candidate charge refinement did not converge in four rounds".into())
}

fn number(text: &str) -> Result<u64, String> {
    if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
        return Err("invalid unsigned frame size".into());
    }
    text.parse().map_err(|_| "frame size overflow".into())
}

/// Local-frame floor plus native return-address/alignment reservation. This does
/// not include unreviewed runtime/system calls or prove host stack availability.
fn reservation(frame: u64, target: Target) -> Result<u64, String> {
    let call = match target {
        Target::MacX86_64 => 8,
        Target::MacArm64 => 0,
    };
    let bytes = frame
        .checked_add(call)
        .ok_or("frame reservation overflow")?
        .max(1);
    Ok(bytes.checked_add(15).ok_or("frame alignment overflow")? & !15)
}

/// `symbols` is llvm-nm --defined-only output for the exact object that produced
/// `usage`. The caller must enforce noredzone and authenticate all inputs.
pub fn refine(
    charges: &[StackCharge],
    usage: &str,
    symbols: &str,
    target: Target,
) -> Result<Refinement, String> {
    refine_with_export(charges, usage, symbols, target, None, 0)
}

/// Conservatively reserve a measured runtime floor in every surviving source
/// function and wrapper, including specialized functions whose calls disappeared.
/// The undefined-symbol report must name only the exact runtime table. The caller
/// must separately establish runtime target/closure and artifact identity.
/// This arithmetic alone does not qualify indirect calls or physical host capacity.
pub fn refine_with_runtime_floor(
    charges: &[StackCharge],
    usage: &str,
    symbols: &str,
    target: Target,
    runtime_floor: u64,
    undefined: &str,
    runtime_api: &str,
) -> Result<Refinement, String> {
    if !runtime_api.starts_with("_R")
        || !runtime_api.ends_with("13SCALAR_API_V1")
        || !runtime_api
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid runtime API identity".into());
    }
    // Pinned llvm-nm --undefined-only emits one bare Mach-O symbol per line.
    let imports: Vec<_> = undefined
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if imports != [format!("_{runtime_api}")] {
        return Err("expected only the separate runtime API import".into());
    }
    if runtime_floor == 0 || !runtime_floor.is_multiple_of(16) {
        return Err("positive aligned runtime floor required".into());
    }
    refine_with_export(charges, usage, symbols, target, None, runtime_floor)
}

/// Full-LTO candidate local-frame refinement. The exact retained Rust classifier
/// must be isolated in the final LLVM graph; unknown/indirect calls and native
/// undefined symbols are rejected. This is not final machine/host certification.
pub fn refine_full_lto(
    charges: &[StackCharge],
    usage: &str,
    symbols: &str,
    undefined: &str,
    callgraph: &str,
    runtime_leaf: &str,
    target: Target,
) -> Result<Refinement, String> {
    if !undefined.trim().is_empty() {
        return Err("uncovered final-object dependency".into());
    }
    if !runtime_leaf.starts_with("_R")
        || !runtime_leaf.ends_with("8classify")
        || !runtime_leaf
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid runtime classifier identity".into());
    }
    let functions = text_functions(symbols)?;
    crate::stack_callgraph::validate(callgraph, &functions, runtime_leaf)?;
    refine_with_export(charges, usage, symbols, target, Some(runtime_leaf), 0)
}

/// Measure the separate runtime leaf's local frame/ABI floor. This does not
/// resolve a game's indirect API-table calls or reserve caller/host capacity.
pub fn runtime_leaf_floor(
    usage: &str,
    symbols: &str,
    undefined: &str,
    graph: &str,
    leaf: &str,
    target: Target,
) -> Result<u64, String> {
    if !undefined.trim().is_empty() {
        return Err("uncovered runtime dependency".into());
    }
    if !leaf.starts_with("_R")
        || !leaf.ends_with("8classify")
        || !leaf.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid runtime classifier identity".into());
    }
    let functions = text_functions(symbols)?;
    crate::stack_callgraph::validate_leaf(graph, &functions, leaf)?;
    let rows: Vec<_> = usage.lines().collect();
    if rows.len() != 1 {
        return Err("single runtime frame required".into());
    }
    let fields: Vec<_> = rows[0].split('\t').collect();
    if fields.len() != 3 || fields[2] != "static" || fields[0].rsplit(':').next() != Some(leaf) {
        return Err("runtime leaf frame mismatch".into());
    }
    reservation(number(fields[1])?, target)
}

pub(crate) fn text_functions(symbols: &str) -> Result<BTreeSet<&str>, String> {
    let mut functions = BTreeSet::new();
    let mut names = BTreeSet::new();
    for line in symbols.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3
            || fields[0].is_empty()
            || !fields[0].bytes().all(|b| b.is_ascii_hexdigit())
            || u64::from_str_radix(fields[0], 16).is_err()
            || !matches!(
                fields[1],
                "a" | "A" | "b" | "B" | "d" | "D" | "r" | "R" | "s" | "S" | "t" | "T"
            )
        {
            return Err("malformed or unsupported defined-symbol report".into());
        }
        if !names.insert(fields[2]) {
            return Err("duplicate object symbol".into());
        }
        if matches!(fields[1], "t" | "T") {
            if let Some(name) = fields[2].strip_prefix('_') {
                if name.is_empty() || !functions.insert(name) {
                    return Err("invalid or duplicate object function".into());
                }
            } else if !fields[2].strip_prefix("ltmp").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
            }) {
                return Err("unrecognized text symbol".into());
            }
        }
    }
    if functions.is_empty() {
        return Err("missing object functions".into());
    }
    Ok(functions)
}

fn refine_with_export(
    charges: &[StackCharge],
    usage: &str,
    symbols: &str,
    target: Target,
    detached_export: Option<&str>,
    runtime_floor: u64,
) -> Result<Refinement, String> {
    if charges.is_empty() || charges.iter().any(|c| c.general == 0 || c.integer == 0) {
        return Err("positive charge inventory required".into());
    }
    let functions = text_functions(symbols)?;
    let mut next = Vec::new();
    next.try_reserve_exact(charges.len())
        .map_err(|_| "charge allocation failed")?;
    next.extend_from_slice(charges);
    let mut seen = BTreeSet::new();
    let mut wrappers = BTreeMap::new();
    let mut detached_exports = BTreeMap::new();
    let mut changed = false;
    for line in usage.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 || fields[2] != "static" {
            return Err("static frame report required".into());
        }
        let name = fields[0].rsplit(':').next().ok_or("missing function")?;
        if !functions.contains(name) || !seen.insert(name) {
            return Err("unknown or duplicate frame function".into());
        }
        let required = reservation(number(fields[1])?, target)?
            .checked_add(runtime_floor)
            .ok_or("runtime reservation overflow")?;
        let entry = if let Some(id) = name.strip_prefix("zfn") {
            Some((id, false))
        } else {
            name.strip_prefix("zsp").map(|id| (id, true))
        };
        if let Some((id, integer)) = entry {
            let index = usize::try_from(number(id)?).map_err(|_| "function id overflow")?;
            let charge = next.get_mut(index).ok_or("unknown source function id")?;
            let selected = if integer {
                &mut charge.integer
            } else {
                &mut charge.general
            };
            if required > *selected {
                *selected = required;
                changed = true;
            }
        } else if detached_export == Some(name) {
            detached_exports.insert(name.to_owned(), required);
        } else if name.starts_with("zeb_stack_candidate_") {
            wrappers.insert(name.to_owned(), required);
        } else {
            return Err("uncovered native function".into());
        }
    }
    if seen != functions {
        return Err("incomplete frame coverage".into());
    }
    Ok(Refinement {
        charges: next,
        changed,
        wrappers,
        detached_exports,
    })
}

/// Check the emitter's pre-optimization inventory against the table being refined.
/// This detects stale build inputs; it does not authenticate objects or independently
/// prove LLVM instruction semantics. Pass the exact module consumed by code generation.
pub fn verify_emitted_charges(charges: &[StackCharge], module: &str) -> Result<(), String> {
    if charges.is_empty() || charges.iter().any(|c| c.general == 0 || c.integer == 0) {
        return Err("positive charge inventory required".into());
    }
    let mut metadata = BTreeMap::new();
    let mut definitions = BTreeSet::new();
    let mut wrapper_count = 0;
    for line in module.lines() {
        if let Some(rest) = line.strip_prefix("; candidate-stack-charge ") {
            let (name, amount) = rest
                .split_once(' ')
                .ok_or("invalid emitted charge record")?;
            let amount = number(amount)?;
            let (id, integer) = if let Some(id) = name.strip_prefix("zfn") {
                (id, false)
            } else if let Some(id) = name.strip_prefix("zsp") {
                (id, true)
            } else {
                return Err("unknown emitted charge entry".into());
            };
            let id = usize::try_from(number(id)?).map_err(|_| "emitted charge id overflow")?;
            let charge = charges.get(id).ok_or("unknown emitted source function")?;
            let expected = if integer {
                charge.integer
            } else {
                charge.general
            };
            if amount != expected || metadata.insert(name, amount).is_some() {
                return Err("stale or duplicate emitted charge".into());
            }
        } else if line.starts_with("define ") {
            let name = line
                .split_once('@')
                .and_then(|(_, s)| s.split_once('('))
                .map(|(n, _)| n)
                .ok_or("invalid candidate definition")?;
            if !line.split_whitespace().any(|s| s == "noredzone") {
                return Err("candidate definition permits red-zone use".into());
            }
            if name.starts_with("zeb_stack_candidate_") {
                wrapper_count += 1;
            } else if !definitions.insert(name) {
                return Err("duplicate candidate definition".into());
            }
        }
    }
    if wrapper_count > 1 || definitions != metadata.keys().copied().collect::<BTreeSet<_>>() {
        return Err("incomplete emitted charge coverage".into());
    }
    for id in 0..charges.len() {
        if !metadata.contains_key(format!("zfn{id}").as_str()) {
            return Err("missing general emitted charge".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_runtime_requires_one_closed_leaf_and_matching_frame() {
        let graph = "Call graph node <<null function>><<0x1>>  #uses=0\n  CS<None> calls function '_Rtest8classify'\nCall graph node for function: '_Rtest8classify'<<0x2>>  #uses=1\n";
        let usage = "runtime:_Rtest8classify\t8\tstatic";
        let symbols = "0 t __Rtest8classify\n10 S _API";
        assert_eq!(
            runtime_leaf_floor(
                usage,
                symbols,
                "",
                graph,
                "_Rtest8classify",
                Target::MacX86_64
            )
            .unwrap(),
            16
        );
        for (u, s, dep, g) in [
            (
                usage.replace("static", "dynamic"),
                symbols.to_owned(),
                "",
                graph.to_owned(),
            ),
            (
                format!("{usage}\n{usage}"),
                symbols.to_owned(),
                "",
                graph.to_owned(),
            ),
            (
                usage.replace("8classify", "8unknown"),
                symbols.to_owned(),
                "",
                graph.to_owned(),
            ),
            (
                usage.to_owned(),
                format!("{symbols}\n20 t _extra"),
                "",
                graph.to_owned(),
            ),
            (
                usage.to_owned(),
                symbols.to_owned(),
                "U _malloc",
                graph.to_owned(),
            ),
            (
                usage.to_owned(),
                symbols.to_owned(),
                "",
                format!("{graph}  CS<0x3> calls function '_Rtest8classify'\n"),
            ),
        ] {
            assert!(
                runtime_leaf_floor(&u, &s, dep, &g, "_Rtest8classify", Target::MacX86_64).is_err()
            );
        }
    }

    #[test]
    fn runtime_floor_grows_source_and_wrapper_without_accumulating_on_recheck() {
        let c = [StackCharge {
            general: 48,
            integer: 16,
        }];
        let usage = "x:zfn0\t40\tstatic\nx:zsp0\t8\tstatic\nx:zeb_stack_candidate_test\t88\tstatic";
        let symbols = "0 t _zfn0\n40 t _zsp0\n80 T _zeb_stack_candidate_test";
        let r = refine_with_runtime_floor(
            &c,
            usage,
            symbols,
            Target::MacX86_64,
            16,
            "__Rtest13SCALAR_API_V1",
            "_Rtest13SCALAR_API_V1",
        )
        .unwrap();
        assert!(r.changed);
        assert_eq!(r.charges[0].general, 64);
        assert_eq!(r.charges[0].integer, 32);
        assert_eq!(r.wrappers["zeb_stack_candidate_test"], 112);
        let stable = refine_with_runtime_floor(
            &r.charges,
            usage,
            symbols,
            Target::MacX86_64,
            16,
            "__Rtest13SCALAR_API_V1",
            "_Rtest13SCALAR_API_V1",
        )
        .unwrap();
        assert!(!stable.changed);
        assert_eq!(stable.wrappers, r.wrappers);
        for floor in [0, 1, 17, u64::MAX - 15] {
            assert!(
                refine_with_runtime_floor(
                    &c,
                    usage,
                    symbols,
                    Target::MacX86_64,
                    floor,
                    "__Rtest13SCALAR_API_V1",
                    "_Rtest13SCALAR_API_V1"
                )
                .is_err()
            );
        }
        let arm = refine_with_runtime_floor(
            &c,
            usage,
            symbols,
            Target::MacArm64,
            16,
            "__Rtest13SCALAR_API_V1",
            "_Rtest13SCALAR_API_V1",
        )
        .unwrap();
        assert_eq!(arm.charges[0].general, 64);
        assert_eq!(arm.wrappers["zeb_stack_candidate_test"], 112);
    }

    #[test]
    fn separate_runtime_rejects_extra_missing_or_malformed_imports() {
        let charges = [StackCharge {
            general: 64,
            integer: 32,
        }];
        for imports in [
            "",
            "_malloc",
            "__Rother13SCALAR_API_V1",
            "__Rtest13SCALAR_API_V1\n_malloc",
            "__Rtest13SCALAR_API_V1\n__Rtest13SCALAR_API_V1",
            "U __Rtest13SCALAR_API_V1",
            "__Rtest13SCALAR_API_V1 extra",
        ] {
            assert!(
                refine_with_runtime_floor(
                    &charges,
                    "x:zfn0\t8\tstatic",
                    "0 t _zfn0",
                    Target::MacX86_64,
                    16,
                    imports,
                    "_Rtest13SCALAR_API_V1",
                )
                .is_err(),
                "accepted {imports}"
            );
        }
        assert!(
            refine_with_runtime_floor(
                &charges,
                "x:zfn0\t8\tstatic",
                "0 t _zfn0",
                Target::MacX86_64,
                16,
                "_malloc",
                "malloc",
            )
            .is_err()
        );
    }

    #[test]
    fn grows_only_insufficient_entries_and_is_stable_on_recheck() {
        let c = [StackCharge {
            general: 64,
            integer: 128,
        }];
        let report =
            "x:zfn0\t88\tstatic\nx:zsp0\t16\tstatic\nx:zeb_stack_candidate_test\t24\tstatic";
        let symbols = "0 t _zfn0\n40 t _zsp0\n80 T _zeb_stack_candidate_test";
        let r = refine(&c, report, symbols, Target::MacX86_64).unwrap();
        assert!(r.changed);
        assert_eq!(r.charges[0].general, 96);
        assert_eq!(r.charges[0].integer, 128);
        assert_eq!(r.wrappers["zeb_stack_candidate_test"], 32);
        assert!(
            !refine(&r.charges, report, symbols, Target::MacX86_64)
                .unwrap()
                .changed
        );
        assert_eq!(reservation(0, Target::MacArm64).unwrap(), 16);
        assert_eq!(reservation(32, Target::MacArm64).unwrap(), 32);
        assert_eq!(reservation(32, Target::MacX86_64).unwrap(), 48);
    }
    #[test]
    fn rejects_malformed_symbol_lines_instead_of_hiding_functions() {
        let charges = [StackCharge {
            general: 64,
            integer: 32,
        }];
        for extra in [
            "20 t _hidden extra",
            "t _hidden",
            "nothex t _hidden",
            "20 ? _hidden",
            "20 U _external",
            "20 tt _hidden",
            "20 t ltmp_hidden",
            "20 t ltmp",
            "10000000000000000 t _hidden",
            "warning: incomplete symbol report",
        ] {
            let symbols = format!("0 t _zfn0\n{extra}");
            assert!(
                refine(&charges, "x:zfn0\t8\tstatic", &symbols, Target::MacX86_64).is_err(),
                "accepted {extra}"
            );
        }
    }

    #[test]
    fn rejects_missing_dynamic_unknown_duplicate_and_overflowing_reports() {
        let c = [StackCharge {
            general: 64,
            integer: 32,
        }];
        for report in [
            "",
            "x:zfn0\t8\tdynamic",
            "x:zfn1\t8\tstatic",
            "x:zfn0\t8\tstatic\nx:zfn0\t8\tstatic",
            "x:zfn0\t18446744073709551615\tstatic",
            "x:zfn0\t+8\tstatic",
        ] {
            assert!(refine(&c, report, "0 t _zfn0", Target::MacX86_64).is_err());
        }
    }
    #[test]
    fn emitted_inventory_rejects_a_larger_table_that_frame_floors_accept() {
        let small = [StackCharge {
            general: 16,
            integer: 16,
        }];
        let large = [StackCharge {
            general: 64,
            integer: 32,
        }];
        let module = "; candidate-stack-charge zfn0 16\ndefine internal %out @zfn0(i64 %stack_remaining) noredzone {\n}\n";
        assert!(
            !refine(&large, "x:zfn0\t24\tstatic", "0 t _zfn0", Target::MacX86_64)
                .unwrap()
                .changed
        );
        assert!(verify_emitted_charges(&large, module).is_err());
        verify_emitted_charges(&small, module).unwrap();
        for bad in [
            module.replace(" 16\n", " 0\n"),
            module.replace("noredzone ", ""),
            module.replace("zfn0 16", "zfn1 16"),
            module.replace("zfn0(i64", "zfn1(i64"),
            format!("{module}; candidate-stack-charge zfn0 16\n"),
            format!("{module}; candidate-stack-charge zsp0 16\n"),
            String::new(),
        ] {
            assert!(
                verify_emitted_charges(&small, &bad).is_err(),
                "accepted {bad}"
            );
        }
    }

    const GRAPH: &str = "Call graph node <<null function>><<0x1>>  #uses=0
  CS<None> calls function '_Rtest8classify'
  CS<None> calls function 'zeb_stack_candidate_test'

Call graph node for function: '_Rtest8classify'<<0x2>>  #uses=1

Call graph node for function: 'zeb_stack_candidate_test'<<0x3>>  #uses=1
  CS<0x4> calls function 'zfn0'

Call graph node for function: 'zfn0'<<0x5>>  #uses=2
  CS<0x6> calls function 'zfn0'
";
    const SYMBOLS: &str = "0 T __Rtest8classify\n20 T _zeb_stack_candidate_test\n40 t _zfn0";
    const USAGE: &str =
        "x:_Rtest8classify\t8\tstatic\nx:zeb_stack_candidate_test\t24\tstatic\nx:zfn0\t88\tstatic";

    #[test]
    fn full_lto_refines_source_and_records_isolated_runtime_export() {
        let charges = [StackCharge {
            general: 64,
            integer: 32,
        }];
        let r = refine_full_lto(
            &charges,
            USAGE,
            SYMBOLS,
            "",
            GRAPH,
            "_Rtest8classify",
            Target::MacX86_64,
        )
        .unwrap();
        assert!(r.changed);
        assert_eq!(r.charges[0].general, 96);
        assert_eq!(r.detached_exports["_Rtest8classify"], 16);
        assert_eq!(r.wrappers["zeb_stack_candidate_test"], 32);
        assert!(
            !refine_full_lto(
                &r.charges,
                USAGE,
                SYMBOLS,
                "",
                GRAPH,
                "_Rtest8classify",
                Target::MacX86_64
            )
            .unwrap()
            .changed
        );
        assert!(refine(&charges, USAGE, SYMBOLS, Target::MacX86_64).is_err());
    }

    #[test]
    fn full_lto_rejects_unknown_indirect_connected_and_incomplete_graphs() {
        let charges = [StackCharge {
            general: 64,
            integer: 32,
        }];
        let mut bad = vec![
            String::new(),
            GRAPH.replace("<<0x2>>", "<<broken>>"),
            GRAPH.replace("  CS<None> calls function '_Rtest8classify'\n", ""),
        ];
        for edge in [
            "  CS<0x7> calls external node\n",
            "  CS<0x7> calls function '_Rtest8classify'\n",
            "  CS<0x7> calls function 'zeb_stack_candidate_test'\n",
            "  CS<0x7> calls function 'unknown'\n",
            "  CS<None> calls function 'zfn0'\n",
            "unrecognized diagnostic\n",
            "Call graph node for function: 'zfn0'<<0x9>>  #uses=0\n",
        ] {
            bad.push(format!("{GRAPH}{edge}"));
        }
        bad.push(GRAPH.replace("Call graph node for function: '_Rtest8classify'<<0x2>>  #uses=1", "Call graph node for function: '_Rtest8classify'<<0x2>>  #uses=1\n  CS<0x9> calls function 'zfn0'"));
        bad.push(GRAPH.replace(
            "Call graph node for function: '_Rtest8classify'<<0x2>>  #uses=1\n",
            "",
        ));
        for graph in bad {
            assert!(
                refine_full_lto(
                    &charges,
                    USAGE,
                    SYMBOLS,
                    "",
                    &graph,
                    "_Rtest8classify",
                    Target::MacX86_64
                )
                .is_err(),
                "accepted {graph}"
            );
        }
        assert!(
            refine_full_lto(
                &charges,
                USAGE,
                SYMBOLS,
                "U _memcpy",
                GRAPH,
                "_Rtest8classify",
                Target::MacX86_64
            )
            .is_err()
        );
        assert!(
            refine_full_lto(
                &charges,
                USAGE,
                SYMBOLS,
                "",
                GRAPH,
                "_Rwrong8classify",
                Target::MacX86_64
            )
            .is_err()
        );
    }
}
