#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};

fn ast(text: &str) -> parser::Ast {
    parser::parse(&Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()).unwrap()
}

#[test]
fn copied_values_and_snapshot_results_lower_on_both_targets() {
    let tree = ast(
        "main(){event('choices'); eventValue(-17); eventValue(true); \
        eventValue(nil); eventValue('λ'); local t = snapshotToken(0); \
        return snapshotToken(t);}",
    );
    flow::check(&tree).unwrap();
    for target in [Target::MacX86_64, Target::MacArm64] {
        llvm::emit_objects(&tree, target).unwrap();
    }
}

#[test]
fn scalar_intrinsics_require_one_argument() {
    for expression in [
        "eventValue()",
        "eventValue(1, 2)",
        "snapshotToken()",
        "snapshotToken(0, 1)",
    ] {
        assert!(
            flow::check(&ast(&format!("main(){{ {expression}; return nil; }}"))).is_err(),
            "{expression}"
        );
    }
}
