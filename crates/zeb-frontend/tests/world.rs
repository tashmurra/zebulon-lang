#![forbid(unsafe_code)]
#[path = "world/declared_rows.rs"]
mod declared_rows;
#[path = "world/grammar_declarations.rs"]
mod grammar_declarations;
#[path = "world/host_scalars.rs"]
mod host_scalars;
#[path = "world/lifetime_collections.rs"]
mod lifetime_collections;
#[path = "world/preinit.rs"]
mod preinit;
#[path = "world/relation_columns.rs"]
mod relation_columns;
#[path = "world/vocabulary_relation.rs"]
mod vocabulary_relation;
#[path = "world/world_increment.rs"]
mod world_increment;

#[test]
fn original_examples_pass_the_frontend() {
    use zeb_frontend::{
        flow, llvm,
        parser::{self, Model},
        preprocess,
        source::Encoding,
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    for (name, model) in [
        ("scalar.t", Model::Ownership),
        ("world.t", Model::Lifetimes),
    ] {
        let input = preprocess::read(
            &root.join("examples").join(name),
            &[],
            Encoding::Utf8,
            16 * 1024 * 1024,
        )
        .unwrap();
        let ast = parser::parse_with(&input.source, model).unwrap();
        flow::check(&ast).unwrap();
        for target in [llvm::Target::MacX86_64, llvm::Target::MacArm64] {
            if name == "scalar.t" {
                llvm::emit(&ast, target).unwrap();
            } else {
                llvm::emit_objects(&ast, target).unwrap();
            }
        }
    }
}
