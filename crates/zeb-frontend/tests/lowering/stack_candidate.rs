#![forbid(unsafe_code)]
use zeb_frontend::{
    llvm::{self, StackCharge, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn rejects_incomplete_or_zero_entry_charges() {
    let ast = parser::parse_with(
        &Source::decode(b"main(){return 1;}".to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    for charges in [
        vec![],
        vec![StackCharge {
            general: 0,
            integer: 1,
        }],
        vec![StackCharge {
            general: 1,
            integer: 0,
        }],
        vec![
            StackCharge {
                general: 1,
                integer: 1
            };
            2
        ],
    ] {
        assert_eq!(
            llvm::emit_stack_candidate(&ast, Target::MacX86_64, true, &charges)
                .unwrap_err()
                .code,
            "stack-charges"
        );
    }
}

#[test]
fn candidate_entry_rejects_bad_roots_signatures_and_symbols() {
    for (source, root, symbol) in [
        ("main(){return 1;}", 1, "zeb_stack_candidate_test"),
        ("main(){return 1;}", 0, "zeb_stack_candidate_bad\nname"),
        ("main(){return 1;}", 0, "zfn0"),
        ("main(a,b,c,d){return a;}", 0, "zeb_stack_candidate_test"),
    ] {
        let ast = parser::parse_with(
            &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership,
        )
        .unwrap();
        let charges = vec![
            StackCharge {
                general: 64,
                integer: 32
            };
            ast.functions.len()
        ];
        assert_eq!(
            llvm::emit_stack_entry_candidate(&ast, Target::MacX86_64, true, &charges, root, symbol)
                .unwrap_err()
                .code,
            "stack-entry"
        );
    }
}

#[test]
fn candidate_entry_rejects_unrepresentable_source_starts() {
    let mut ast = parser::parse_with(
        &Source::decode(b"main(){return 1;}".to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    ast.nodes[ast.functions[0].0].start = usize::MAX;
    let charges = [StackCharge {
        general: 64,
        integer: 32,
    }];
    assert_eq!(
        llvm::emit_stack_entry_candidate(
            &ast,
            Target::MacX86_64,
            true,
            &charges,
            0,
            "zeb_stack_candidate_test"
        )
        .unwrap_err()
        .code,
        "stack-entry"
    );
}

#[test]
fn runtime_candidate_rejects_untrusted_symbol_and_missing_charge_inventory() {
    let ast = parser::parse_with(
        &Source::decode(b"main(x){return x+1;}".to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    let charges = [StackCharge {
        general: 256,
        integer: 128,
    }];
    for (runtime, charges, code) in [
        (Some("_Rbad\nname"), &charges[..], "runtime-symbol"),
        (Some("_Rmatching"), &[][..], "stack-charges"),
    ] {
        assert_eq!(
            llvm::emit_stack_entry_with_runtime_candidate(
                &ast,
                Target::MacX86_64,
                true,
                charges,
                0,
                "zeb_stack_candidate_test",
                runtime,
            )
            .unwrap_err()
            .code,
            code
        );
    }
}
