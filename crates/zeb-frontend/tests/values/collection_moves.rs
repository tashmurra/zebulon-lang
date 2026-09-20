#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn local_collections_keep_fresh_results_and_borrow_existing_results() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/resolve-owners.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let ast = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}

#[test]
fn borrowed_receivers_cannot_move_into_collections() {
    let source = Source::decode(b"f(borrow, recipients){borrow.moveToCollection(recipients);return nil;}main(){return nil;}".to_vec(), Encoding::Utf8).unwrap();
    let ast = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    assert!(flow::check(&ast).is_err());
}
