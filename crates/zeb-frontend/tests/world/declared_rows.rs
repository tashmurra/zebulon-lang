#![forbid(unsafe_code)]
//! a declaration may state a relation row, written `relation(partner)`
//! so it cannot be confused with a field, and a reverse name writes the row the
//! way it reads it. Expectations are self-authored: TADS has no relations.
use zeb_frontend::{
    llvm::{self, Target},
    object_init,
    parser::{self, Model},
    source::{Encoding, Source},
};

fn tree(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        Model::Lifetimes,
    )
}

fn objects(text: &str) -> Result<String, zeb_frontend::Diagnostic> {
    object_init::emit(&tree(text)?)
}

const CONTAINS: &str =
    "relation contains(container: Entity, item: Entity) one_to_many reverse location;\n";
const EXITS: &str = "enum Direction: north, south;\nrelation exits(from: Entity, to: Entity, via: Direction) one_to_one;\n";

/// The relation set operation, as the boundary numbers it.
const SET: &str = "i32 131";

/// Every descriptor a declared row carries, in emission order.
fn descriptors(module: &str) -> Vec<u64> {
    module
        .lines()
        .filter(|line| line.contains(SET) && line.contains("%session"))
        .filter_map(|line| {
            line.split(", ")
                .find_map(|field| field.strip_prefix("i64 ")?.parse::<u64>().ok())
        })
        .collect()
}

#[test]
fn a_declaration_states_a_row_instead_of_writing_a_field() {
    let module = objects(&format!(
        "{CONTAINS}study: object; desk: object location(study);"
    ))
    .unwrap();
    assert_eq!(descriptors(&module).len(), 1, "{module}");
    // A row is a relation operation, not a property write.
    assert!(
        !module.contains("i32 8, i64 %session, i64 %object1"),
        "the row was also written as a field: {module}"
    );
}

/// The whole point of the parenthesised form: `=` keeps its ordinary meaning, so
/// a relation never takes a name away from a property. `location` and `owner` are
/// may also be ordinary properties.
#[test]
fn an_assignment_to_a_relation_name_is_still_an_ordinary_property() {
    let module = objects(&format!(
        "{CONTAINS}study: object; desk: object location = study;"
    ))
    .unwrap();
    assert!(
        descriptors(&module).is_empty(),
        "an assignment declared a row: {module}"
    );
    // And it is written as a field, which is an object-valued property write.
    assert!(module.contains("i32 8, i64 %session"), "{module}");
}

#[test]
fn a_reverse_name_declares_the_row_the_way_it_reads() {
    let forward = objects(&format!(
        "{CONTAINS}study: object; hall: object contains(study);"
    ))
    .unwrap();
    let reverse = objects(&format!(
        "{CONTAINS}study: object; desk: object location(study);"
    ))
    .unwrap();
    let (forward, reverse) = (descriptors(&forward)[0], descriptors(&reverse)[0]);
    assert_eq!(forward & 0xfff, 0, "the wrong relation");
    assert_eq!(forward >> 12 & 1, 0, "the forward name is not reversed");
    assert_eq!(reverse >> 12 & 1, 1, "the reverse name is");
    // one_to_many is cardinality 1, and neither is labelled.
    assert_eq!(forward >> 13 & 3, 1);
    assert_eq!(reverse >> 15 & 1, 0);
}

#[test]
fn a_labelled_row_names_its_label_as_well_as_its_partner() {
    let module = objects(&format!(
        "{EXITS}study: object exits(cellar, north); cellar: object exits(study, south);"
    ))
    .unwrap();
    let rows = descriptors(&module);
    assert_eq!(rows.len(), 2, "{module}");
    // Labelled, one_to_one, and the two labels are the two enumerator values.
    for row in &rows {
        assert_eq!(row >> 15 & 1, 1, "not labelled: {row}");
        assert_eq!(row >> 13 & 3, 0);
    }
    assert_eq!(rows[0] >> 16 & 0xfff, 0, "north is the first enumerator");
    assert_eq!(rows[1] >> 16 & 0xfff, 1, "south is the second");
}

#[test]
fn a_label_is_required_exactly_when_the_relation_has_one() {
    // Missing where the relation has a label column.
    assert_eq!(
        objects(&format!(
            "{EXITS}study: object; cellar: object exits(study);"
        ))
        .unwrap_err()
        .code,
        "object-init-relation"
    );
    // Supplied where it has none.
    assert_eq!(
        objects(&format!(
            "{CONTAINS}enum north;\nstudy: object; desk: object location(study, north);"
        ))
        .unwrap_err()
        .code,
        "object-init-relation"
    );
    // And a label that is not an enumerator.
    assert_eq!(
        objects(&format!(
            "{EXITS}study: object; cellar: object exits(study, study);"
        ))
        .unwrap_err()
        .code,
        "object-init-relation"
    );
}

#[test]
fn a_relation_row_names_an_entity() {
    assert_eq!(
        objects(&format!("{CONTAINS}desk: object location(5);"))
            .unwrap_err()
            .code,
        "parse-expected"
    );
    // A name that is not an object at all is caught by the ordinary initializer.
    assert_eq!(
        objects(&format!("{CONTAINS}desk: object location(nowhere);"))
            .unwrap_err()
            .code,
        "sem-name"
    );
}

/// a labelled relation's tables are allocated after the ones in use, so
/// every declared relation's table is reserved before any row is written. The
/// fixture declares travel before containment to exercise this ordering.
#[test]
fn every_declared_relation_reserves_its_table_first() {
    let module = objects(&format!(
        "{EXITS}{CONTAINS}study: object exits(cellar, north); cellar: object;          desk: object location(study);"
    ))
    .unwrap();
    let reserve = module
        .lines()
        .filter(|line| line.contains("i32 129"))
        .count();
    assert_eq!(reserve, 2, "one per declared relation: {module}");
    // Reserved before the first row, not after it.
    let first_reserve = module.find("i32 129").expect("a reservation");
    let first_row = module.find(SET).expect("a row");
    assert!(first_reserve < first_row, "{module}");

    // A vocabulary relation has no table to reserve.
    let ast = tree(
        "enum token tokWord;\n\
         property firstTokenIndex, lastTokenIndex, tokenList;\n\
         dictionary cmdDict;\n\
         relation names(entity: Entity, word: Text) many_to_many;\n\
         coin: object names = 'coin';\n\
         turn(toks){ return nil; }",
    )
    .unwrap();
    let module = llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
    assert!(!module.contains("i32 129"), "{module}");
}

#[test]
fn a_vocabulary_relation_still_writes_words_as_a_property() {
    let ast = tree(
        "enum token tokWord;\n\
         property firstTokenIndex, lastTokenIndex, tokenList;\n\
         dictionary cmdDict;\n\
         relation names(entity: Entity, word: Text) many_to_many;\n\
         coin: object names = 'coin';\n\
         turn(toks){ return nil; }",
    )
    .unwrap();
    let module = llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
    assert!(
        descriptors(&module).is_empty(),
        "a word became a relation row: {module}"
    );
}

// a containment prefix states a row. `+` has one meaning in TADS, so
// unlike `=` there is no second reading for it to protect.

#[test]
fn a_containment_prefix_states_a_row_per_level() {
    let module = objects(&format!(
        "{CONTAINS}+ property location;\nstudy: object;\n+ desk: object;\n++ drawer: object;\n+++ tin: object;"
    ))
    .unwrap();
    // One row per nesting level: desk in study, drawer in desk, tin in drawer.
    assert_eq!(descriptors(&module).len(), 3, "{module}");
}

/// A containment prefix must create a relation row for the nested entity.
#[test]
fn a_containment_prefix_is_not_written_as_a_field() {
    let module = objects(&format!(
        "{CONTAINS}+ property location;\nstudy: object;\n+ desk: object;"
    ))
    .unwrap();
    assert_eq!(descriptors(&module).len(), 1, "{module}");
    assert!(
        !module.contains("i32 8, i64 %session, i64 %object1"),
        "the prefix was also written as a field: {module}"
    );
}

/// `+ desk` under `study` puts the desk in the study, because `location` is the
/// reverse name and a row goes in the way its name reads.
#[test]
fn a_containment_prefix_follows_the_direction_of_the_name_it_was_declared_with() {
    let reverse = objects(&format!(
        "{CONTAINS}+ property location;\nstudy: object;\n+ desk: object;"
    ))
    .unwrap();
    let forward = objects(&format!(
        "{CONTAINS}+ property contains;\nstudy: object;\n+ desk: object;"
    ))
    .unwrap();
    let (reverse, forward) = (descriptors(&reverse)[0], descriptors(&forward)[0]);
    assert_eq!(reverse >> 12 & 1, 1, "the reverse name is reversed");
    assert_eq!(forward >> 12 & 1, 0, "the forward name is not");
}

/// A containment property that names no relation keeps writing a field, so a
/// program with no relations behaves exactly as it did.
#[test]
fn a_containment_prefix_naming_no_relation_still_writes_a_field() {
    let module = objects("+ property parent;\nbox: object;\n+ lid: object;").unwrap();
    assert!(
        descriptors(&module).is_empty(),
        "a non-relation prefix declared a row: {module}"
    );
    assert!(module.contains("i32 8, i64 %session"), "{module}");
}

/// A labelled relation needs a third column, and a prefix has nowhere to name
/// it. Refused rather than filed under whichever table comes first.
#[test]
fn a_containment_prefix_refuses_a_labelled_relation() {
    let error = tree(&format!(
        "{EXITS}+ property exits;\nstudy: object;\n+ cellar: object;"
    ))
    .unwrap_err();
    assert!(
        format!("{error:?}").contains("label"),
        "expected a label diagnostic, got {error:?}"
    );
}

/// A vocabulary relation's rows are words, so nesting has no meaning for it.
/// Ignoring it wrote the container into a vocabulary property and did nothing —
/// a silent no-op rather than a relation update.
#[test]
fn a_containment_prefix_refuses_a_vocabulary_relation() {
    let error = tree(
        "dictionary cmdDict;\nrelation names(entity: Entity, word: Text) many_to_many;\n+ property names;\nbox: object;\n+ lid: object;",
    )
    .unwrap_err();
    assert!(
        format!("{error:?}").contains("vocabulary"),
        "expected a vocabulary diagnostic, got {error:?}"
    );
}

/// The refusal is on the prefix only. Declaring vocabulary the ordinary way is
/// untouched, which is what makes refusing the prefix cheap.
#[test]
fn refusing_the_prefix_does_not_disturb_ordinary_vocabulary() {
    tree("dictionary cmdDict;\nrelation names(entity: Entity, word: Text) many_to_many;\ncoin: object names = 'coin';")
        .unwrap();
}

// a declared row moves with the name when an object is modified.

/// `modify` renames the target to a private base class and gives its name to
/// the modification. A row left on the base names something that is not the
/// object any more — and is a class besides, which has no business in a
/// containment table.
#[test]
fn a_declared_row_survives_modify() {
    let module = objects(&format!(
        "{CONTAINS}study: object;\ndesk: object location(study);\nmodify desk poked = true;"
    ))
    .unwrap();
    assert_eq!(descriptors(&module).len(), 1, "the row was lost: {module}");
}

/// The row has to name the object that kept the name, not the base the modify
/// left behind. Scans and keyed reads must agree on the entity identity.
#[test]
fn a_modified_objects_row_names_the_object_that_kept_the_name() {
    let module = objects(&format!(
        "{CONTAINS}study: object;\ndesk: object location(study);\nmodify desk poked = true;"
    ))
    .unwrap();
    let plain = objects(&format!(
        "{CONTAINS}study: object;\ndesk: object location(study);"
    ))
    .unwrap();
    // The same row, against the same pair, whether or not the object was modified.
    assert_eq!(descriptors(&module), descriptors(&plain), "{module}");
}

/// A modification that states its own row is the later word on where the
/// object is, so it wins rather than colliding with the one it replaces.
#[test]
fn a_modifications_own_row_replaces_the_targets() {
    let module = objects(&format!(
        "{CONTAINS}study: object;\ncellar: object;\ndesk: object location(study);\nmodify desk location(cellar);"
    ))
    .unwrap();
    assert_eq!(
        descriptors(&module).len(),
        1,
        "two rows were emitted: {module}"
    );
}

#[test]
fn a_labelled_row_also_survives_modify() {
    let module = objects(&format!(
        "{EXITS}study: object;\ncellar: object;\nhall: object exits(cellar, north);\nmodify hall poked = true;"
    ))
    .unwrap();
    assert_eq!(
        descriptors(&module).len(),
        1,
        "the labelled row was lost: {module}"
    );
}

// several rows of a many_to_many relation in one declaration.

const CONNECTS: &str = "relation connects(door: Entity, room: Entity) many_to_many;\n";

/// A door is one entity in two rooms, and its declaration can say so. adv3Lite
/// needs two objects naming each other.
#[test]
fn a_many_to_many_relation_states_several_rows() {
    let module = objects(&format!(
        "{CONNECTS}hall: object;\ndrive: object;\nfrontDoor: object connects(hall) connects(drive);"
    ))
    .unwrap();
    assert_eq!(descriptors(&module).len(), 2, "{module}");
}

#[test]
fn a_many_to_many_relation_states_as_many_as_it_likes() {
    let module = objects(&format!(
        "{CONNECTS}a: object;\nb: object;\nc: object;\nj: object connects(a) connects(b) connects(c);"
    ))
    .unwrap();
    assert_eq!(descriptors(&module).len(), 3, "{module}");
}

/// For the other two cardinalities a second row would displace the first, so
/// writing two is a mistake — and the message says which rule was broken.
#[test]
fn a_one_to_many_relation_still_states_one_row() {
    let error = objects(&format!(
        "{CONTAINS}a: object;\nb: object;\nc: object location(a) location(b);"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-property");
    assert!(error.message.contains("many_to_many"), "{error:?}");
}

/// A **labelled** relation states one row per label, not one row.
///
/// A labelled relation is a family of tables, one per label, so two
/// rows under different labels go in different tables and neither displaces the
/// other. This was refused until the author game asked for a room with a way
/// north and a way west, which no adventure survives without.
#[test]
fn a_labelled_relation_states_one_row_per_label() {
    let module = objects(&format!(
        "{EXITS}a: object;\nb: object;\nc: object exits(a, north) exits(b, south);"
    ))
    .expect("two directions are two tables");
    assert_eq!(descriptors(&module).len(), 2, "{module}");

    // The same label twice is still a displacement, and still a mistake.
    let error = objects(&format!(
        "{EXITS}a: object;\nb: object;\nc: object exits(a, north) exits(b, north);"
    ))
    .unwrap_err();
    assert_eq!(error.code, "sem-property");
    assert!(error.message.contains("labelled"), "{error:?}");
}

/// An ordinary property is still defined once, with the message it always had.
#[test]
fn an_ordinary_property_is_still_defined_once() {
    let error = objects("a: object n = 1 n = 2;").unwrap_err();
    assert_eq!(error.code, "sem-property");
    assert!(error.message.contains("duplicate"), "{error:?}");
}
