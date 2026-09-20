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

/// comparator sorts lower to the resumable runtime quicksort, including
/// inside handlers; the comparator-free form is not implemented yet.
#[test]
fn comparator_vector_sort_lowers_and_bare_sort_is_rejected() {
    for text in [
        "main(){ local owned v = new Vector(3); v.append(2); v.append(1); \
         local same = v.sort(nil, {a, b: a - b}); return same == v; }",
        "main(){ local owned v = new Vector(2); v.append(1); \
         try { v.sort(true, {a, b: a - b}); } catch (Exception e) { return 0; } return 1; } \
         class Exception: object;",
    ] {
        let tree = parse(text);
        flow::check(&tree).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        llvm::emit_objects(&tree, llvm::Target::MacX86_64)
            .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    }
    let bare = parse("main(){ local owned v = new Vector(2); v.sort(); return nil; }");
    assert_eq!(flow::check(&bare).unwrap_err().code, "sem-arity");
}
