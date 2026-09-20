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
fn owned_buffers_compile_and_cannot_escape_as_ordinary_returns() {
    let tree = ast(include_str!("../../../../tests/native/string-buffers.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert!(flow::check(&ast("f(){local owned b=new StringBuffer();return b;}")).is_err());
}
