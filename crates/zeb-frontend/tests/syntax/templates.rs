use zeb_frontend::{
    flow, parser,
    source::{Encoding, Source},
};
fn parse(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
}
#[test]
fn templates_lower_to_existing_properties_and_native_methods() {
    let ast = parse(include_str!("../../../../tests/native/object-templates.t")).unwrap();
    flow::check(&ast).unwrap();
    assert!(
        ast.nodes
            .iter()
            .any(|n| matches!(n.syntax, parser::Syntax::Template(_)))
    );
    zeb_frontend::llvm::emit_objects(&ast, zeb_frontend::llvm::Target::MacArm64).unwrap();
}
#[test]
fn unmatched_and_unimplemented_shapes_do_not_drop_data() {
    for source in [
        "class A: object; A template 'x'; a:A +3;",
        "class A:object; A template inherited; a:A 'x';",
        "object template [items]; a:object [1,2];",
        "object template 'x' | ;",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}
