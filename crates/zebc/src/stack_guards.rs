//! Structural guard check for the compiler's pre-optimization LLVM dialect.
//! LLVM verification, final native accounting and host admission remain separate.
use std::collections::BTreeMap;
use zeb_frontend::llvm::StackCharge;

#[derive(Default)]
struct Function<'a> {
    name: &'a str,
    blocks: BTreeMap<&'a str, Vec<&'a str>>,
    definitions: BTreeMap<&'a str, (&'a str, usize, &'a str)>,
}

fn charge(name: &str, charges: &[StackCharge]) -> Result<u64, String> {
    let (id, integer) = if let Some(id) = name.strip_prefix("zfn") {
        (id, false)
    } else {
        (
            name.strip_prefix("zsp").ok_or("unknown guarded callee")?,
            true,
        )
    };
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid guarded callee id".into());
    }
    let c = charges
        .get(
            id.parse::<usize>()
                .map_err(|_| "guarded callee id overflow")?,
        )
        .ok_or("unknown guarded callee id")?;
    Ok(if integer { c.integer } else { c.general })
}
fn branch(line: &str) -> Result<Option<(&str, &str, &str)>, String> {
    if let Some(dest) = line.strip_prefix("br label %") {
        return Ok(Some(("", dest, "")));
    }
    if let Some(rest) = line.strip_prefix("br i1 ") {
        let (condition, rest) = rest
            .split_once(", label %")
            .ok_or("malformed conditional branch")?;
        let (yes, no) = rest
            .split_once(", label %")
            .ok_or("malformed conditional branch")?;
        return Ok(Some((condition, yes, no)));
    }
    if line.starts_with("ret ") {
        return Ok(None);
    }
    Err("unsupported or missing candidate block terminator".into())
}
fn check(f: &Function<'_>, charges: &[StackCharge]) -> Result<usize, String> {
    let mut predecessors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (&name, body) in &f.blocks {
        for line in body.iter().take(body.len().saturating_sub(1)) {
            if line.starts_with("br ") || line.starts_with("ret ") {
                return Err("instruction follows a terminator".into());
            }
        }
        if let Some((_, yes, no)) = branch(body.last().ok_or("empty candidate block")?)? {
            for dest in [yes, no].into_iter().filter(|s| !s.is_empty()) {
                if !f.blocks.contains_key(dest) {
                    return Err("unknown candidate branch destination".into());
                }
                predecessors.entry(dest).or_default().push(name);
            }
        }
    }
    if !f.blocks.contains_key("entry") || predecessors.contains_key("entry") {
        return Err("invalid candidate entry block".into());
    }
    let wrapper = f.name.starts_with("zeb_stack_candidate_");
    let incoming = if wrapper {
        "%budget"
    } else {
        "%stack_remaining"
    };
    let mut calls = 0;
    for (&block, body) in &f.blocks {
        for (index, line) in body.iter().enumerate() {
            let Some((_, rhs)) = line.split_once(" = ") else {
                if line.contains("call ") {
                    return Err("unsupported unassigned call".into());
                }
                continue;
            };
            let Some(rest) = rhs
                .strip_prefix("call %out @")
                .or_else(|| rhs.strip_prefix("call %iout @"))
            else {
                if rhs.contains("call ")
                    && (rhs.contains("@zfn")
                        || rhs.contains("@zsp")
                        || rhs.contains("call %out ")
                        || rhs.contains("call %iout "))
                {
                    return Err("unsupported source call form".into());
                }
                continue;
            };
            let (callee, arguments) = rest.split_once('(').ok_or("invalid guarded call")?;
            let amount = charge(callee, charges)?;
            let budget = arguments
                .strip_prefix("i64 ")
                .and_then(|s| s.split([',', ')']).next())
                .ok_or("missing call budget")?;
            let &(sub_block, sub_index, subtraction) = f
                .definitions
                .get(budget)
                .ok_or("call lacks budget subtraction")?;
            if sub_block != block
                || sub_index >= index
                || subtraction != format!("sub i64 {incoming}, {amount}")
            {
                return Err("call budget is not the current checked subtraction".into());
            }
            let preds = predecessors.get(block).ok_or("unguarded call block")?;
            if preds.len() != 1 {
                return Err("guarded call block has a bypass predecessor".into());
            }
            let guard_block = preds[0];
            let guard_body = &f.blocks[guard_block];
            let (condition, failure, success) =
                branch(guard_body.last().unwrap())?.ok_or("missing admission branch")?;
            if success != block || failure == block {
                return Err("admission branch polarity is wrong".into());
            }
            let &(cmp_block, _, comparison) = f
                .definitions
                .get(condition)
                .ok_or("missing admission comparison")?;
            if cmp_block != guard_block
                || comparison != format!("icmp ult i64 {incoming}, {amount}")
            {
                return Err("admission does not compare the exact unsigned charge".into());
            }
            let failure_body = &f.blocks[failure];
            if failure_body.len() != 1 {
                return Err("admission failure does not return immediately".into());
            }
            let ret = failure_body[0];
            let resource = if wrapper {
                ret.strip_prefix("ret i64 ")
                    .and_then(|s| s.parse::<u64>().ok())
                    .is_some_and(|n| n as u32 == 8)
            } else {
                ret.strip_prefix("ret %out { i64 0, i32 6, i64 ")
                    .or_else(|| ret.strip_prefix("ret %iout { i32 0, i32 6, i64 "))
                    .and_then(|s| s.strip_suffix(" }"))
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some()
            };
            if !resource {
                return Err("admission failure is not a stack-resource outcome".into());
            }
            calls += 1;
        }
    }
    Ok(calls)
}

/// Check every original source/root call against its table entry, actual SSA
/// subtraction and sole incoming success edge. Does not use guard comments as
/// evidence and does not validate post-LTO rewritten code or source-byte truth.
pub fn verify_guarded_calls(charges: &[StackCharge], module: &str) -> Result<usize, String> {
    crate::stack_charges::verify_emitted_charges(charges, module)?;
    let mut current: Option<Function<'_>> = None;
    let mut block = None;
    let mut count = 0;
    for line in module
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with(';'))
    {
        if line.starts_with("define ") {
            if current.is_some() {
                return Err("nested candidate definition".into());
            }
            let name = line
                .split_once('@')
                .and_then(|(_, s)| s.split_once('('))
                .ok_or("invalid candidate function")?
                .0;
            current = Some(Function {
                name,
                ..Function::default()
            });
            block = None;
        } else if line == "}" {
            let f = current.take().ok_or("orphan candidate function end")?;
            count += check(&f, charges)?;
            block = None;
        } else if let Some(f) = current.as_mut() {
            if let Some(label) = line.strip_suffix(':') {
                if label.is_empty() || f.blocks.insert(label, Vec::new()).is_some() {
                    return Err("duplicate candidate block".into());
                }
                block = Some(label);
            } else {
                let label = block.ok_or("candidate instruction outside block")?;
                let body = f.blocks.get_mut(label).unwrap();
                if let Some((lhs, rhs)) = line.split_once(" = ")
                    && (matches!(lhs, "%budget" | "%stack_remaining")
                        || f.definitions
                            .insert(lhs, (label, body.len(), rhs))
                            .is_some())
                {
                    return Err("duplicate candidate SSA definition".into());
                }
                body.push(line);
            }
        }
    }
    if current.is_some() {
        return Err("unterminated candidate function".into());
    }
    Ok(count)
}
