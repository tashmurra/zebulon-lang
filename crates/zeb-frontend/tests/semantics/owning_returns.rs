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
fn factories_transfer_local_ownership_through_native_calls() {
    let tree = ast(include_str!("../../../../tests/native/owning-factories.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn ownership_cannot_be_inferred_from_a_borrow_or_discarded() {
    for source in [
        "class C:object;owned f(x){return move x;}",
        "class C:object;owned f(){local x=new published C;return move x;}",
        "class C:object;f(){local owned x=new C;return move x;}",
        "class C:object;owned f(){local owned x=new C;return x;}",
        "class C:object;owned f(){local owned x=new C;return move x;}main(){f();return nil;}",
        "class C:object;f(){return C;}main(){local owned x=f();return nil;}",
        "class C:object;owned f(){local owned x=new C;return move x;}main(){return f;}",
        "class C:object;owned main(){local owned x=new C;return move x;}",
        "class C:object;owned f(n){if(n){local owned x=new C;return move x;}}",
        "class C:object;owned f(){try{local owned x=new C;return move x;}finally{}}",
    ] {
        assert!(
            flow::check(&ast(source)).is_err(),
            "unexpectedly accepted: {source}"
        );
    }
}
