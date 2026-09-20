#![forbid(unsafe_code)]
use zeb_frontend::{
    flow, llvm,
    parser::{self, Syntax},
    source::{Encoding, Source},
};
fn decode(text: &str) -> Source {
    Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap()
}

/// Fixed arguments may follow a single expansion: `f(list..., a)` and
/// `f(a, list..., b)`; a second expansion is rejected.
#[test]
fn expansion_followed_by_fixed_arguments_lowers() {
    let tree = parser::parse_with(
        &decode(
            "target: object three(a,b,c){ return a+b+c; } ; \
         main(){ local pair = [1, 2]; local single = [7]; \
         return target.three(pair..., 3) + target.three(9, single..., 5); }",
        ),
        parser::Model::Ownership,
    )
    .unwrap();
    let tails = tree
        .nodes
        .iter()
        .filter(|n| matches!(n.syntax, Syntax::ArgumentPackTail(_)))
        .count();
    assert_eq!(tails, 2);
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
    assert!(
        parser::parse_with(
            &decode("main(){ local a = [1]; return f(a..., a..., 1); }"),
            parser::Model::Ownership
        )
        .is_err()
    );
}
