#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
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
fn membership_binds_names_and_lowers_lazy_candidates() {
    let tree = parse(include_str!("../../../../tests/native/membership.t")).unwrap();
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert!(flow::check(&parse("main(){return 1 is in (missing);}").unwrap()).is_err());
}
#[test]
fn membership_requires_a_nonempty_parenthesized_set() {
    for expression in [
        "1 is in ()",
        "1 not in ()",
        "1 is in 2",
        "1 is in (2,)",
        "1 not (2)",
    ] {
        assert!(
            parse(&format!("main(){{return {expression};}}")).is_err(),
            "{expression}"
        );
    }
}

#[test]
fn implicit_export_name_preserves_declaration_metadata() {
    let tree = parse("export RuntimeError; export p 'external.p';").unwrap();
    let exported: Vec<_> = tree
        .nodes
        .iter()
        .filter_map(|node| match &node.syntax {
            parser::Syntax::Declaration(parser::Declaration::Export { name, external }) => {
                Some((name.as_str(), external.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        exported,
        [("RuntimeError", "RuntimeError"), ("p", "external.p")]
    );
}
