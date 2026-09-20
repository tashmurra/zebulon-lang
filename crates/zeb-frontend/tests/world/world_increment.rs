#![forbid(unsafe_code)]
//! Compile the world fixtures covering undo, rollback, despawn, labelled
//! relations, handle incarnation, intents and inspection.
//!
//! Expectations are self-authored: this model deliberately diverges from TADS,
//! so none of these programs has a reference counterpart.
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser::{self, Model},
    source::{Encoding, Source},
};

fn compile(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    let ast = parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        Model::Lifetimes,
    )?;
    flow::check(&ast)?;
    llvm::emit_objects(&ast, Target::MacX86_64)?;
    Ok(ast)
}

#[test]
fn undo_cycles_compiles() {
    compile(include_str!("../../../../tests/native/undo-cycles.t")).unwrap();
}

#[test]
fn turn_rollback_compiles() {
    compile(include_str!("../../../../tests/native/turn-rollback.t")).unwrap();
}

#[test]
fn despawn_compiles() {
    compile(include_str!("../../../../tests/native/despawn.t")).unwrap();
}

#[test]
fn labelled_relations_compile() {
    let ast = compile(include_str!(
        "../../../../tests/native/labelled-relations.t"
    ))
    .unwrap();
    // The label column is what makes travel a table rather than a field, so the
    // declaration has to carry it.
    assert!(
        ast.relations.iter().any(|relation| relation.labelled),
        "no labelled relation survived parsing"
    );
}

#[test]
fn intent_and_inspection_compiles() {
    compile(include_str!(
        "../../../../tests/native/intent-and-inspection.t"
    ))
    .unwrap();
}

#[test]
fn semantic_events_compile() {
    // `event(id)` and `eventSubject(entity)` are the two halves of a
    // semantic event, because the operation boundary has three payload slots
    // and an id costs two of them.
    compile(include_str!("../../../../tests/native/semantic-events.t")).unwrap();
}

#[test]
fn vocabulary_data_compiles() {
    compile(include_str!("../../../../tests/native/vocabulary-data.t")).unwrap();
}

#[test]
fn the_two_room_world_compiles() {
    compile(include_str!("../../../../tests/native/two-rooms.t")).unwrap();
}

#[test]
fn declared_nesting_compiles() {
    // `+`/`++` states a containment row. Four levels deep, then back
    // out to the top, which is the case a prefix that drops a level gets wrong.
    compile(include_str!("../../../../tests/native/declared-nesting.t")).unwrap();
}

#[test]
fn preinit_order_compiles() {
    // a preinit phase between object initialization and startup,
    // ordered while compiling. Three preinits, a subclass, and rows read from
    // declarations rather than assembled.
    compile(include_str!("../../../../tests/native/preinit-order.t")).unwrap();
}

#[test]
fn vocabulary_parts_compiles() {
    // one entity with words in two parts of speech, told apart by the
    // vocabulary property label.
    compile(include_str!("../../../../tests/native/vocabulary-parts.t")).unwrap();
}

#[test]
fn vocabulary_string_compiles() {
    // an adv3Lite vocab string split into rows while compiling.
    compile(include_str!("../../../../tests/native/vocabulary-string.t")).unwrap();
}

#[test]
fn vocabulary_slot_compiles() {
    // a grammar slot naming one part of speech.
    compile(include_str!("../../../../tests/native/vocabulary-slot.t")).unwrap();
}

#[test]
fn modify_rows_compiles() {
    // a declared row survives modify, and a modification may state
    // its own.
    compile(include_str!("../../../../tests/native/modify-rows.t")).unwrap();
}

#[test]
fn list_index_of_compiles() {
    // indexOf reads a list as well as a vector.
    compile(include_str!("../../../../tests/native/list-index-of.t")).unwrap();
}

#[test]
fn value_interpolation_compiles() {
    // a value string interpolates, and a property holding one
    // becomes a method.
    compile(include_str!(
        "../../../../tests/native/value-interpolation.t"
    ))
    .unwrap();
}

#[test]
fn multi_row_declaration_compiles() {
    // several rows of a many_to_many relation in one declaration.
    compile(include_str!(
        "../../../../tests/native/multi-row-declaration.t"
    ))
    .unwrap();
}

#[test]
fn undo_depth_is_reachable_from_source() {
    // The runtime, the lowering and the verifier all carried undoDepth before
    // any source name did, so a game could not ask how much undo it had left.
    compile("turn(toks){ if (undoDepth() > 0) { \"some\\n\"; } return nil; }").unwrap();
}

#[test]
fn undo_depth_takes_no_arguments() {
    assert_eq!(
        compile("turn(toks){ return undoDepth(1); }")
            .unwrap_err()
            .code,
        "sem-arity"
    );
}
