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

/// `delegated` evaluates the current property as another object
/// defines it, with `self` unchanged; the target need not be an ancestor.
#[test]
fn delegated_value_and_call_forms_lower() {
    let tree = parse(
        "class Mixin: object rank = (kind * 10 + 1) score(n) { return kind * 100 + n; }; \
         holder: object kind = 7 helper = Mixin rank = (delegated Mixin) \
         score(n) { return delegated helper(n); }; \
         main(){ return holder.rank + holder.score(3); }",
    );
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
}

#[test]
fn delegated_outside_a_method_is_rejected() {
    let tree = parse("other: object p = 1; main(){ return delegated other; }");
    assert_eq!(flow::check(&tree).unwrap_err().code, "sem-context");
}
