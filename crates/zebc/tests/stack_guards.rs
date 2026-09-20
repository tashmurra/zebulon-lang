#![forbid(unsafe_code)]
use zeb_frontend::{
    llvm::{self, StackCharge, Target},
    parser,
    source::{Encoding, Source},
};
use zebc::{stack_charges::verify_emitted_charges, stack_guards::verify_guarded_calls};
fn fixture(optimize: bool, target: Target) -> (Vec<StackCharge>, String) {
    let source = Source::decode(
        b"f(n){if(n==0)return 1;return f(n-1)+n;}main(n){return f(n);}".to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let ast = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    let charges = vec![
        StackCharge {
            general: 64,
            integer: 32
        };
        2
    ];
    let module = llvm::emit_stack_entry_with_runtime_candidate(
        &ast,
        target,
        optimize,
        &charges,
        1,
        "zeb_stack_candidate_test",
        None,
    )
    .unwrap();
    (charges, module)
}
#[test]
fn checks_general_specialized_and_root_calls() {
    for target in [Target::MacX86_64, Target::MacArm64] {
        for optimized in [false, true] {
            let (charges, module) = fixture(optimized, target);
            let count = verify_guarded_calls(&charges, &module).unwrap();
            assert!(count >= 3);
            if optimized {
                assert!(module.contains("call %iout @zsp"));
            }
        }
    }
}
#[test]
fn rejects_instruction_mutations_with_intact_charge_comments() {
    let (charges, module) = fixture(false, Target::MacX86_64);
    let mutations = [
        module.replacen(
            "icmp ult i64 %stack_remaining, 64",
            "icmp slt i64 %stack_remaining, 64",
            1,
        ),
        module.replacen(
            "icmp ult i64 %stack_remaining, 64",
            "icmp ult i64 %stack_remaining, 16",
            1,
        ),
        module.replacen(
            "sub i64 %stack_remaining, 64",
            "sub i64 %stack_remaining, 16",
            1,
        ),
        module.replacen(
            "sub i64 %stack_remaining, 64",
            "add i64 %stack_remaining, 64",
            1,
        ),
        module.replacen("i32 6, i64", "i32 0, i64", 1),
        module.replacen("icmp ult i64 %budget, 64", "icmp ult i64 %budget, 1", 1),
        module.replacen("sub i64 %budget, 64", "sub i64 %budget, 1", 1),
        module.replacen(
            "label %exhausted, label %invoke",
            "label %invoke, label %exhausted",
            1,
        ),
        module.replacen(
            "br i1 %small, label %exhausted, label %invoke",
            "br label %invoke",
            1,
        ),
        module.replacen(
            "br i1 %version, label %admit, label %mismatch",
            "br i1 %version, label %admit, label %invoke",
            1,
        ),
        module.replacen("i64 %remaining,", "i64 %budget,", 1),
    ];
    for mutant in mutations {
        assert_ne!(mutant, module);
        verify_emitted_charges(&charges, &mutant).unwrap();
        assert!(verify_guarded_calls(&charges, &mutant).is_err());
    }
}
#[test]
fn rejects_indirect_or_unassigned_source_calls() {
    let (charges, module) = fixture(false, Target::MacX86_64);
    let call = module
        .lines()
        .find(|l| l.contains(" = call %out @zfn"))
        .unwrap();
    let rhs = call.split_once(" = ").unwrap().1;
    for mutant in [
        module.replacen(call, rhs, 1),
        module.replacen("call %out @zfn0", "call %out %pointer", 1),
    ] {
        verify_emitted_charges(&charges, &mutant).unwrap();
        assert!(verify_guarded_calls(&charges, &mutant).is_err());
    }
}
