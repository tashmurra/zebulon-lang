#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn sized_owned_tables_lower_capacity_and_bucket_queries() {
    let source = Source::decode(
        include_bytes!("../../../../tests/native/sized-tables.t").to_vec(),
        Encoding::Utf8,
    )
    .unwrap();
    let ast = parser::parse_with(&source, parser::Model::Ownership).unwrap();
    flow::check(&ast).unwrap();
    llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
}
