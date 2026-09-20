#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn ast(source: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn qualified_calls_keep_receiver_and_support_expansion() {
    let tree = ast(include_str!(
        "../../../../tests/native/qualified-inherited.t"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn qualified_calls_require_method_context_and_known_prototype() {
    for source in [
        "class Base:object; main(){inherited Base(); return nil;}",
        "class Base:object f(){inherited Missing();};",
    ] {
        assert!(flow::check(&ast(source)).is_err());
    }
}
#[test]
fn qualified_property_calls_start_at_the_named_class() {
    let tree = ast(include_str!(
        "../../../../tests/native/qualified-inherited-property.t"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
