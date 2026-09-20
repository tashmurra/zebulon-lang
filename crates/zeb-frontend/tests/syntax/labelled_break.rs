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
fn a_labelled_statement_can_be_left_with_a_named_break() {
    let tree = ast(include_str!("../../../../tests/native/labelled-break.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn named_continue_still_requires_a_loop_and_labels_stay_unique() {
    for source in [
        // `continue` needs somewhere to go back to, which a block is not.
        "main(){local n=0; here: {n=1; continue here;} return n;}",
        // A break still needs the label to name an enclosing statement.
        "main(){local n=0; here: {n=1;} break here; return n;}",
        "main(){ here: { here: { break here; } } return nil;}",
    ] {
        assert!(flow::check(&ast(source)).is_err(), "{source}");
    }
}
