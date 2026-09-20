#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn constant_list_defaults_lower_nested_references_and_inheritance() {
    let ast = parse(include_str!(
        "../../../../tests/native/constant-list-defaults.t"
    ));
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}
#[test]
fn dynamic_elements_are_not_misrepresented_as_constants() {
    for source in [
        r#"a:object data=["emitting"]; main(){return nil;}"#,
        "a:object data=[f()]; f(){return 1;} main(){return nil;}",
        "a:object data=[new Vector(1)]; main(){return nil;}",
    ] {
        let source = Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap();
        let result = parser::parse_with(&source, parser::Model::Ownership)
            .and_then(|ast| flow::check(&ast).map(|_| ()));
        assert!(result.is_err());
    }
}
