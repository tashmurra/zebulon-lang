#![forbid(unsafe_code)]
use zeb_frontend::{
    flow, llvm,
    parser::{self, Syntax},
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}

/// TADS `for (x in collection)` iterates like foreach; ranges keep range-for.
#[test]
fn collection_for_in_lowers_as_foreach_and_ranges_stay_numeric() {
    let tree = parse(
        "f(){local t=0; for(local x in [1,2,3]) t+=x; local e; for(e in [4,5]) t+=e; \
         for(local i in 1..3) t+=i; return t;} main(){return f();}",
    );
    let kinds =
        |pattern: fn(&Syntax) -> bool| tree.nodes.iter().filter(|n| pattern(&n.syntax)).count();
    assert_eq!(kinds(|s| matches!(s, Syntax::ForEach(..))), 2);
    assert_eq!(kinds(|s| matches!(s, Syntax::For { .. })), 1);
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, llvm::Target::MacX86_64).unwrap();
}
