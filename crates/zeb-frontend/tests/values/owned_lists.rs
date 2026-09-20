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
fn owned_snapshots_return_and_move_into_fields() {
    let tree = ast(include_str!("../../../../tests/native/owned-list.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn snapshots_require_an_explicit_owner_and_supported_arguments() {
    for text in [
        "main(){local owned v=new Vector(1); local l=v.toList(); return nil;}",
        "main(){local owned v=new Vector(1); local owned l=v.toList(1); return nil;}",
    ] {
        assert!(flow::check(&ast(text)).is_err(), "{text}");
    }
}
