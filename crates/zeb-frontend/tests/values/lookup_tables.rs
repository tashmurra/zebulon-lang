#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn ast(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn owned_tables_lower_entries_defaults_methods_and_indexed_assignment() {
    let tree = ast(include_str!("../../../../tests/native/lookup-tables.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn table_borrows_do_not_gain_ownership_and_keys_are_bound() {
    for source in [
        "main(){local table=[1 -> 2];return nil;}",
        "main(){local owned table=[missing -> 2];return nil;}",
        "main(){local owned table=[1 -> 2];return table;}",
        "main(){local owned table=[1 -> 2];table[1]+=3;return nil;}",
    ] {
        assert!(flow::check(&ast(source)).is_err(), "{source}");
    }
}
