#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn sequential_updates_can_keep_previous_field_owners_alive() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/resolve-update-lists.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let tree = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
