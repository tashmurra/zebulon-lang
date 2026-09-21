//! Pinned LLVM linked-disassembly reader for candidate call closure only.
//! Does not prove instruction semantics, frame sizes, or budget-check dominance.
use std::collections::{BTreeMap, BTreeSet};
use zeb_frontend::llvm::Target;

#[derive(Debug)]
pub struct NativeCall {
    pub caller: String,
    pub callee: String,
    pub address: u64,
}

fn hex(s: &str) -> Result<u64, String> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid disassembly address".into());
    }
    u64::from_str_radix(s, 16).map_err(|_| "disassembly address overflow".into())
}
fn source(name: &str) -> bool {
    name.strip_prefix("zfn")
        .or_else(|| name.strip_prefix("zsp"))
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}
struct Instruction<'a> {
    owner: &'a str,
    branch: Option<u64>,
    falls_through: bool,
}

/// Reports must come from the same authenticated linked dylib: llvm-nm
/// --defined-only and llvm-objdump --disassemble --no-show-raw-insn. This
/// deliberately excludes non-LTO indirect runtime dispatch and tail transfers.
pub fn validate(
    disassembly: &str,
    symbols: &str,
    runtime_leaf: &str,
    target: Target,
) -> Result<Vec<NativeCall>, String> {
    let expected = crate::stack_charges::text_functions(symbols)?;
    let mut entries = BTreeMap::new();
    for line in symbols.lines().filter(|s| !s.trim().is_empty()) {
        let f: Vec<_> = line.split_whitespace().collect();
        if matches!(f[1], "t" | "T")
            && let Some(name) = f[2].strip_prefix('_')
            && entries.insert(hex(f[0])?, name).is_some()
        {
            return Err("aliased native entries are not admitted".into());
        }
    }
    if !expected.contains(runtime_leaf)
        || expected
            .iter()
            .any(|n| *n != runtime_leaf && !source(n) && !n.starts_with("zeb_stack_candidate_"))
    {
        return Err("unexpected native function inventory".into());
    }
    let format = match target {
        Target::LinuxX86_64 | Target::WindowsX86_64 => {
            return Err("stack qualification is not validated for this target".into());
        }
        Target::MacX86_64 => "file format mach-o 64-bit x86-64",
        Target::MacArm64 => "file format mach-o arm64",
    };
    let (mut header, mut section) = (false, false);
    let mut functions = BTreeSet::new();
    let mut instructions = BTreeMap::new();
    let mut calls = Vec::new();
    let mut current = None;
    let mut first = None;
    for line in disassembly.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if !header {
            if !line.ends_with(format) {
                return Err("unexpected native disassembly format".into());
            }
            header = true;
            continue;
        }
        if !section {
            if line != "Disassembly of section __TEXT,__text:" {
                return Err("unexpected native code section".into());
            }
            section = true;
            continue;
        }
        if let Some((address, name)) = line.split_once(" <")
            && let Some(name) = name.strip_suffix(">:").and_then(|s| s.strip_prefix('_'))
        {
            let address = hex(address)?;
            if first.is_some() || entries.get(&address) != Some(&name) || !functions.insert(name) {
                return Err("native function header disagrees with symbols".into());
            }
            current = Some(name);
            first = Some(address);
            continue;
        }
        let owner = current.ok_or("instruction outside native function")?;
        let (address, text) = line.split_once(':').ok_or("malformed native instruction")?;
        let address = hex(address)?;
        if let Some(start) = first.take()
            && address != start
        {
            return Err("missing function entry instruction".into());
        }
        if instructions
            .last_key_value()
            .is_some_and(|(previous, _)| *previous >= address)
        {
            return Err("unordered or duplicate native instruction".into());
        }
        let mut words = text.split_whitespace();
        let op = words.next().ok_or("missing native opcode")?;
        let operands = text.trim().strip_prefix(op).unwrap().trim();
        let (call, branch, unconditional, ret, ordinary) = match target {
            Target::LinuxX86_64 | Target::WindowsX86_64 => {
                return Err("stack qualification is not validated for this target".into());
            }
            Target::MacX86_64 => (
                op == "callq",
                matches!(op, "jae" | "jb" | "je" | "jge" | "jmp" | "jne" | "jno"),
                op == "jmp",
                op == "retq",
                matches!(
                    op,
                    "addl"
                        | "addq"
                        | "andq"
                        | "cltd"
                        | "cmovel"
                        | "cmoveq"
                        | "cmovneq"
                        | "cmpl"
                        | "cmpq"
                        | "idivl"
                        | "leal"
                        | "leaq"
                        | "movabsq"
                        | "movl"
                        | "movq"
                        | "negl"
                        | "nopl"
                        | "nopw"
                        | "orb"
                        | "orq"
                        | "popq"
                        | "pushq"
                        | "sarq"
                        | "setb"
                        | "setne"
                        | "shlq"
                        | "shrq"
                        | "testl"
                        | "testq"
                        | "xorl"
                ),
            ),
            Target::MacArm64 => (
                op == "bl",
                matches!(
                    op,
                    "b" | "b.eq" | "b.ge" | "b.hs" | "b.lo" | "b.ne" | "cbnz" | "cbz"
                ),
                op == "b",
                op == "ret",
                matches!(
                    op,
                    "add"
                        | "and"
                        | "asr"
                        | "bfi"
                        | "cmn"
                        | "cmp"
                        | "csel"
                        | "csinc"
                        | "ldp"
                        | "lsl"
                        | "lsr"
                        | "mov"
                        | "movk"
                        | "orr"
                        | "sdiv"
                        | "stp"
                        | "sub"
                        | "subs"
                ),
            ),
        };
        if !(call || branch || ret || ordinary) || (ret && !operands.is_empty()) {
            return Err(format!("unsupported native instruction {op}"));
        }
        let mut branch_target = None;
        if call || branch {
            let operand = if matches!(op, "cbz" | "cbnz") {
                operands
                    .split_once(',')
                    .ok_or("malformed conditional branch")?
                    .1
                    .trim()
            } else {
                operands
            };
            let destination = operand
                .split_whitespace()
                .next()
                .and_then(|s| s.strip_prefix("0x"))
                .ok_or("indirect or malformed native transfer")?;
            let destination = hex(destination)?;
            if call {
                let callee = *entries
                    .get(&destination)
                    .ok_or("call does not target a native entry")?;
                if owner == runtime_leaf || !source(callee) {
                    return Err("uncovered native call target".into());
                }
                calls.push(NativeCall {
                    caller: owner.into(),
                    callee: callee.into(),
                    address,
                });
            } else {
                branch_target = Some(destination);
            }
        }
        instructions.insert(
            address,
            Instruction {
                owner,
                branch: branch_target,
                falls_through: !(ret || unconditional),
            },
        );
    }
    if !section || first.is_some() || functions != expected {
        return Err("incomplete native function coverage".into());
    }
    for instruction in instructions.values() {
        if let Some(destination) = instruction.branch
            && instructions.get(&destination).map(|i| i.owner) != Some(instruction.owner)
        {
            return Err("branch escapes its function or targets a non-instruction".into());
        }
    }
    // Check reachable fallthrough as well as explicit branches. Padding after a
    // return can be unreachable; it must not hide a reachable entry crossover.
    for (&entry, &owner) in &entries {
        let mut pending = vec![entry];
        let mut seen = BTreeSet::new();
        while let Some(address) = pending.pop() {
            if !seen.insert(address) {
                continue;
            }
            let instruction = instructions
                .get(&address)
                .ok_or("missing reachable instruction")?;
            if instruction.owner != owner {
                return Err("native fallthrough escapes function".into());
            }
            if let Some(destination) = instruction.branch {
                pending.push(destination);
            }
            if instruction.falls_through {
                let next = instructions
                    .range((
                        std::ops::Bound::Excluded(address),
                        std::ops::Bound::Unbounded,
                    ))
                    .next()
                    .map(|(&a, _)| a)
                    .ok_or("native fallthrough leaves code")?;
                pending.push(next);
            }
        }
    }
    Ok(calls)
}
