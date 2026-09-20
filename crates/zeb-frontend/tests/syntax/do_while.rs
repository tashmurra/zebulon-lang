#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn post_test_loops_lower_control_cleanup_and_labels() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/do-while.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let ast = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}
#[test]
fn post_test_loop_requires_condition_and_terminator() {
    for body in [
        "do {}",
        "do {} while (true)",
        "do {} until (true);",
        "do {} while ();",
        "local do = 1;",
    ] {
        let source =
            Source::decode(format!("main() {{ {body} }}").into_bytes(), Encoding::Utf8).unwrap();
        assert!(
            parser::parse_with(&source, parser::Model::Ownership).is_err(),
            "{body}"
        );
    }
}
