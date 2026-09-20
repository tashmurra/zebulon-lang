#![forbid(unsafe_code)]
use zeb_frontend::llvm::Target;
use zebc::stack_machine::validate;
const SYMBOLS: &str = "10 t __Rtest8classify\n20 T _zeb_stack_candidate_test\n30 t _zfn0\n";
const X86: &str = "game: file format mach-o 64-bit x86-64\nDisassembly of section __TEXT,__text:\n10 <__Rtest8classify>:\n10: retq\n20 <_zeb_stack_candidate_test>:\n20: callq 0x30 <_zfn0>\n25: retq\n30 <_zfn0>:\n30: callq 0x30 <_zfn0>\n35: retq\n";
const ARM: &str = "game: file format mach-o arm64\nDisassembly of section __TEXT,__text:\n10 <__Rtest8classify>:\n10: ret\n20 <_zeb_stack_candidate_test>:\n20: bl 0x30 <_zfn0>\n24: ret\n30 <_zfn0>:\n30: bl 0x30 <_zfn0>\n34: ret\n";

#[test]
fn admits_direct_recursive_calls_in_both_slices() {
    for (text, target) in [(X86, Target::MacX86_64), (ARM, Target::MacArm64)] {
        let calls = validate(text, SYMBOLS, "_Rtest8classify", target).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].caller, "zeb_stack_candidate_test");
        assert_eq!(calls[0].callee, "zfn0");
        assert_eq!(calls[0].address, 0x20);
        assert_eq!(calls[1].caller, "zfn0");
    }
}
#[test]
fn rejects_uncovered_control_transfers() {
    for replacement in [
        "callq *%rax",
        "callq 0x10",
        "callq 0x31",
        "callq 0x99",
        "jmp 0x30",
        "je 0x99",
        "jmpq *%rax",
        "syscall",
        "ud2",
    ] {
        let text = X86.replacen("callq 0x30 <_zfn0>", replacement, 1);
        assert!(
            validate(&text, SYMBOLS, "_Rtest8classify", Target::MacX86_64).is_err(),
            "accepted {replacement}"
        );
    }
    for replacement in [
        "blr x9",
        "br x9",
        "b 0x30",
        "cbz w0, 0x99",
        "bl 0x10",
        "svc #0",
    ] {
        let text = ARM.replacen("bl 0x30 <_zfn0>", replacement, 1);
        assert!(
            validate(&text, SYMBOLS, "_Rtest8classify", Target::MacArm64).is_err(),
            "accepted {replacement}"
        );
    }
}
#[test]
fn rejects_incomplete_or_disagreeing_reports() {
    for text in [
        X86.replace("10: retq\n", ""),
        X86.replace(
            "20 <_zeb_stack_candidate_test>",
            "21 <_zeb_stack_candidate_test>",
        ),
        X86.replace("25: retq", "20: retq"),
        X86.replace("35: retq", "35: mystery"),
        X86.replace("x86-64", "arm64"),
        X86.replace("__TEXT,__text", "__TEXT,__stubs"),
    ] {
        assert!(validate(&text, SYMBOLS, "_Rtest8classify", Target::MacX86_64).is_err());
    }
    assert!(
        validate(
            X86,
            &SYMBOLS.replace("30 t", "40 t"),
            "_Rtest8classify",
            Target::MacX86_64
        )
        .is_err()
    );
}
#[test]
fn rejects_leaf_calls_and_reachable_fallthrough() {
    for text in [
        X86.replace("10: retq", "10: callq 0x30"),
        X86.replace("25: retq", "25: movq %rax, %rax"),
        X86.replace("35: retq", "35: movq %rax, %rax"),
    ] {
        assert!(validate(&text, SYMBOLS, "_Rtest8classify", Target::MacX86_64).is_err());
    }
}
#[test]
fn allows_internal_branch_and_unreachable_padding() {
    let text = X86.replace(
        "25: retq",
        "25: je 0x29\n27: retq\n28: nopw (%rax)\n29: retq",
    );
    assert!(validate(&text, SYMBOLS, "_Rtest8classify", Target::MacX86_64).is_ok());
    let text = ARM.replace("24: ret", "24: cbz w0, 0x2c\n28: ret\n2c: ret");
    assert!(validate(&text, SYMBOLS, "_Rtest8classify", Target::MacArm64).is_ok());
}
