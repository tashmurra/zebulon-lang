#![forbid(unsafe_code)]
use zeb_frontend::{
    flow, llvm, parser,
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}

/// TADS sources use `owned` as an ordinary name (Adv3 `disambig.t`), so it only
/// introduces an owned declaration when a name follows it.
#[test]
fn owned_is_both_a_declaration_keyword_and_a_variable_name() {
    for text in [
        "main(){ local owned = 3; owned += 1; return owned; }",
        "main(){ local owned, other = 2; owned = other; return owned; }",
        "main(){ local owned v = new Vector(2); v.append(1); return v.length(); }",
    ] {
        let tree = parse(text);
        flow::check(&tree).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        llvm::emit_objects(&tree, llvm::Target::MacX86_64)
            .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    }
}
