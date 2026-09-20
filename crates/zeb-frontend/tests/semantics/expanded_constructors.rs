#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn expanded_owned_constructors_lower_with_argument_lists() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/expanded-constructors.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let tree = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}

#[test]
fn fixed_arguments_and_trailing_expansion_lower_to_native_packs() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/mixed-expanded-arguments.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let tree = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}

#[test]
fn expanded_named_methods_use_checked_native_dispatch() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/expanded-methods.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let tree = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
