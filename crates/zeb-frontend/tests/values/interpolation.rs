use zeb_frontend::{
    flow, parser,
    source::{Encoding, Source},
};
fn source(text: &str) -> Source {
    Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()
}
#[test]
fn output_embeddings_keep_native_order_and_nested_expression_structure() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/native/interpolation.t");
    let input = zeb_frontend::preprocess::read(&path, &[], Encoding::Utf8, 1024 * 1024).unwrap();
    let ast = parser::parse_with(&input.source, parser::Model::Ownership).unwrap();
    let checked = flow::check(&ast).unwrap();
    assert!(
        checked
            .program
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.operation, zeb_frontend::ir::Operation::EmitValue(_)))
    );
    zeb_frontend::llvm::emit_objects(&ast, zeb_frontend::llvm::Target::MacArm64).unwrap();
}
#[test]
fn value_interpolation_stays_unavailable_until_owned_results_are_lowered() {
    let ast = parser::parse_with(
        &source("main(){return 'value <<42>>';}"),
        parser::Model::Ownership,
    )
    .unwrap();
    assert_eq!(flow::check(&ast).unwrap_err().code, "sem-unavailable");
}
#[test]
fn interpolation_is_iterative_and_preserves_encoded_locations() {
    let mut text = String::from("main(){");
    for _ in 0..200 {
        text.push_str("\"a<<");
    }
    text.push_str("42");
    for _ in 0..200 {
        text.push_str(">>b\"");
    }
    text.push_str(";return nil;}");
    let ast = parser::parse_with(&source(&text), parser::Model::Ownership).unwrap();
    flow::check(&ast).unwrap();
    let s = Source::decode(b"main(){\"\xe9<<missing>>\";}".to_vec(), Encoding::Latin1).unwrap();
    let ast = parser::parse_with(&s, parser::Model::Ownership).unwrap();
    assert_eq!(flow::check(&ast).unwrap_err().byte, 11);
}
