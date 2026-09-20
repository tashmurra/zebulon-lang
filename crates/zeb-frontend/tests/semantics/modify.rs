#![forbid(unsafe_code)]
use zeb_frontend::{
    flow, llvm,
    parser::{self, Syntax},
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}

/// a modification takes over its target's name, and the original
/// definition becomes a private base class that `inherited` reaches.
#[test]
fn modifications_layer_over_objects_and_classes() {
    let tree = parse(
        "class Base: object rank() { return 1; } tag = 1; \
         item: Base rank() { return inherited() * 10 + 2; }; \
         modify Base rank() { return inherited() * 10 + 3; } tag = 4; \
         modify item extra = 5; \
         main(){ return item.rank() + item.tag + item.extra; }",
    );
    let named: Vec<&str> = tree
        .nodes
        .iter()
        .filter_map(|node| match &node.syntax {
            Syntax::Object { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    // Both layers keep the source names; the originals move to private bases.
    assert_eq!(named.iter().filter(|name| **name == "Base").count(), 1);
    assert_eq!(named.iter().filter(|name| **name == "item").count(), 1);
    assert_eq!(
        named
            .iter()
            .filter(|name| name.starts_with('$') && name.contains("$modified"))
            .count(),
        2
    );
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
}

#[test]
fn modification_of_an_undefined_target_is_rejected() {
    let source = Source::decode(b"modify Missing p = 1;".to_vec(), Encoding::Utf8).unwrap();
    let error = parser::parse_with(&source, parser::Model::Ownership).unwrap_err();
    assert_eq!(error.code, "parse-expected");
}
