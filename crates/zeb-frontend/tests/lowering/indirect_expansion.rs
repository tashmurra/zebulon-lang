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
fn arguments_expand_into_an_indirect_property_call() {
    let tree = ast(include_str!(
        "../../../../tests/native/indirect-expansion.t"
    ));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn indirect_inherited_selection_is_still_refused() {
    let source = "class Base:object f(a){return a;} ; class Sub:Base f(a){local p=&f; return inherited.(p)(a);} ; main(){return nil;}";
    assert!(flow::check(&ast(source)).is_err());
}
