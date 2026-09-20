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
fn owning_collection_operations_compile_with_native_constructors() {
    let tree = ast(include_str!("../../../../tests/native/owned-events.t"));
    flow::check(&tree).unwrap();
    assert_eq!(
        llvm::emit(&ast("main(c){return c.ownedLength();}"), Target::MacX86_64)
            .unwrap_err()
            .code,
        "native-profile"
    );
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    let self_removal = ast(include_str!(
        "../../../../tests/native/event-self-removal.t"
    ));
    llvm::emit_objects(&self_removal, Target::MacX86_64).unwrap();
}
#[test]
fn ownership_and_argument_contracts_are_not_silently_relaxed() {
    for (source, code) in [
        ("main(){return newOwnedCollection();}", "sem-unavailable"),
        ("r:object c=static newOwnedCollection(1);", "sem-arity"),
        ("main(x){return x.ownedAt();}", "sem-arity"),
        ("main(x){return x.reserveInCollection();}", "sem-arity"),
    ] {
        assert_eq!(flow::check(&ast(source)).unwrap_err().code, code);
    }
}
