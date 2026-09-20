#![forbid(unsafe_code)]
use zeb_frontend::{
    parser,
    source::{Encoding, Source},
};
fn parse(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
}
#[test]
fn nested_patterns_expand_declarations_without_rewriting_bodies() {
    let ast = parse("item:object { propertyset 'pre*' { propertyset '*Post' { value=7 calc(){return value;} } } value=9 }; main(){return item.precalcPost();}").unwrap();
    zeb_frontend::flow::check(&ast).unwrap();
    zeb_frontend::llvm::emit_objects(&ast, zeb_frontend::llvm::Target::MacX86_64).unwrap();
}
#[test]
fn invalid_patterns_and_unimplemented_common_parameters_are_rejected() {
    for source in [
        "item:object propertyset 'missing' {value=1};",
        "item:object propertyset '**' {value=1};",
        "item:object propertyset 'bad-*' {value=1};",
        "item:object propertyset '*' (a,*) { f(){} };",
        "item:object propertyset '*' {value=1;",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}
