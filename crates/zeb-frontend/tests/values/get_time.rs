#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn ast(source: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn the_tick_counter_compiles_as_an_integer_intrinsic() {
    // GetTimeTicks is 2 in tadsgen.h; the fixture reaches this through #define.
    let tree = ast("main(){local t = getTime(2); return t - getTime(2);}");
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn a_declaration_that_does_not_match_the_contract_is_refused() {
    for source in [
        "intrinsic 'tads-gen/030008' { getTime(a, b); } main(){return getTime(2);}",
        "intrinsic 'tads-gen/030007' { getTime(a?); } main(){return getTime(2);}",
    ] {
        assert!(flow::check(&ast(source)).is_err(), "{source}");
    }
}
