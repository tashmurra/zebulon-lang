#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn parse(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn explicit_local_owner_transfer_lowers_to_checked_native_fields() {
    let ast = parse(include_str!("../../../../tests/native/field-moves.t"));
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}
#[test]
fn ordinary_borrows_cannot_consume_an_owner() {
    for source in [
        "holder:object x=nil; f(borrow){borrow.moveToField(holder,&x);} main(){return nil;}",
        "holder:object x=nil; main(){local owned v=new LookupTable();local alias=v;alias.moveToField(holder,&x);return nil;}",
    ] {
        assert!(flow::check(&parse(source)).is_err());
    }
}
