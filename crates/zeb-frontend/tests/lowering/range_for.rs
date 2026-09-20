#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn ranges_lower_saved_bounds_and_ordered_updates() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/range-for.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let ast = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}
#[test]
fn range_locals_do_not_escape_the_loop() {
    let source = Source::decode(
        b"main(){for(local i in 1..3){} return i;}".to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let ast = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    assert!(flow::check(&ast).is_err());
}
