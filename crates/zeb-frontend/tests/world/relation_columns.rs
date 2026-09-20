#![forbid(unsafe_code)]
//! Relation column annotations, reads across a labelled relation family,
//! and the narrow profile's arithmetic `+`.
//! Expectations are self-authored: none of these forms exists in TADS.
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser::{self, Model},
    source::{Encoding, Source},
};

fn tree(text: &str) -> Result<parser::Ast, zeb_frontend::Diagnostic> {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        Model::Lifetimes,
    )
}

fn check(text: &str) -> Result<(), zeb_frontend::Diagnostic> {
    let ast = tree(text)?;
    flow::check(&ast)?;
    llvm::emit_objects(&ast, Target::MacX86_64)?;
    Ok(())
}

const CONTAINS: &str =
    "relation contains(container: Entity, item: Entity) one_to_many reverse location;\n";
const EXITS: &str = "enum Direction: north, south;\nrelation exits(from: Entity, to: Entity, via: Direction) one_to_one;\n";

#[test]
fn an_unknown_column_annotation_is_refused() {
    // Column declarations are checked, not treated as annotations.
    assert_eq!(
        tree(
            "relation contains(container: Entitty, item: Entity) one_to_many; turn(t){return nil;}"
        )
        .unwrap_err()
        .code,
        "parse-expected"
    );
    // Entity and Text still parse where they are allowed.
    check(&format!(
        "{CONTAINS}a: object; b: object; turn(t){{ contains.set(a, b); return nil; }}"
    ))
    .unwrap();
}

#[test]
fn the_label_column_may_name_its_own_family() {
    // The annotation names a declared enum family.
    check(&format!(
        "{EXITS}study: object; hall: object;\n\
         turn(t){{ exits.set(study, hall, north); return exits.get(study, north); }}"
    ))
    .unwrap();
}

#[test]
fn an_entity_column_refuses_a_value_that_cannot_be_an_entity() {
    for argument in ["'desk'", "5", "[1, 2]", "true"] {
        let source =
            format!("{CONTAINS}a: object; turn(t){{ contains.set(a, {argument}); return nil; }}");
        assert_eq!(
            check(&source).unwrap_err().code,
            "sem-relation-column",
            "{argument} was accepted in an Entity column"
        );
    }
    // An enumerator is not an entity either, which is the mistake a labelled
    // relation invites: the label belongs last, not in a column.
    assert_eq!(
        check(&format!(
            "{EXITS}study: object; turn(t){{ exits.set(study, north, north); return nil; }}"
        ))
        .unwrap_err()
        .code,
        "sem-relation-column"
    );
}

#[test]
fn a_label_refuses_a_value_that_is_not_an_enumerator() {
    assert_eq!(
        check(&format!(
            "{EXITS}study: object; hall: object;\n\
             turn(t){{ exits.set(study, hall, 'north'); return nil; }}"
        ))
        .unwrap_err()
        .code,
        "sem-relation-column"
    );
}

#[test]
fn a_reverse_name_swaps_the_columns_it_checks() {
    // `location` reads contains backwards, so its right column is the container.
    assert_eq!(
        check(&format!(
            "{CONTAINS}a: object; turn(t){{ location.set('room', a); return nil; }}"
        ))
        .unwrap_err()
        .code,
        "sem-relation-column"
    );
    check(&format!(
        "{CONTAINS}a: object; b: object; turn(t){{ location.set(a, b); return nil; }}"
    ))
    .unwrap();
}

#[test]
fn a_vocabulary_column_takes_a_word() {
    const NAMES: &str = "enum token tokWord;\nproperty firstTokenIndex, lastTokenIndex, tokenList;\ndictionary cmdDict;\nrelation names(entity: Entity, word: Text) many_to_many;\n";
    assert_eq!(
        check(&format!(
            "{NAMES}coin: object names = 'coin'; turn(t){{ names.set(coin, 5); return nil; }}"
        ))
        .unwrap_err()
        .code,
        "sem-relation-column"
    );
}

#[test]
fn the_every_label_form_says_so_in_the_descriptor() {
    // The runtime cannot tell "every label" from "label 0" unless the descriptor
    // carries it, and both compile, so the bit is what this checks.
    let ast = tree(&format!(
        "{EXITS}study: object; turn(t){{ return exits.all(study); }}"
    ))
    .unwrap();
    let checked = flow::check(&ast).unwrap();
    let descriptors: Vec<u64> = checked
        .program
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match &instruction.operation {
            zeb_frontend::ir::Operation::Constant(zeb_frontend::sema::Scalar::Integer(value)) => {
                Some(*value as u32 as u64)
            }
            _ => None,
        })
        .collect();
    assert!(
        descriptors
            .iter()
            .any(|d| d >> 28 & 1 == 1 && d >> 15 & 1 == 1),
        "no every-label descriptor among {descriptors:?}"
    );

    // The labelled form does not claim it.
    let labelled = tree(&format!(
        "{EXITS}study: object; turn(t){{ return exits.all(study, north); }}"
    ))
    .unwrap();
    let checked = flow::check(&labelled).unwrap();
    assert!(
        !checked
            .program
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(
                &instruction.operation,
                zeb_frontend::ir::Operation::Constant(zeb_frontend::sema::Scalar::Integer(value))
                    if (*value as u32 as u64) >> 28 & 1 == 1
            )),
        "the labelled form set the every-label bit"
    );
}

#[test]
fn all_reads_a_labelled_relation_across_every_table() {
    // a room lists the ways out of it without naming each direction.
    check(&format!(
        "{EXITS}study: object; hall: object;\n\
         turn(t){{ exits.set(study, hall, north); return exits.all(study); }}"
    ))
    .unwrap();
    // The labelled form still works, and a third argument does not.
    check(&format!(
        "{EXITS}study: object; turn(t){{ return exits.all(study, north); }}"
    ))
    .unwrap();
    assert_eq!(
        check(&format!(
            "{EXITS}study: object; turn(t){{ return exits.all(study, north, north); }}"
        ))
        .unwrap_err()
        .code,
        "sem-arity"
    );
    // An unlabelled relation has no family to read across, so `all` still takes
    // exactly the entity.
    assert_eq!(
        check(&format!(
            "{CONTAINS}a: object; turn(t){{ return contains.all(a, a); }}"
        ))
        .unwrap_err()
        .code,
        "sem-arity"
    );
}

#[test]
fn a_scalar_program_adds_under_lifetimes() {
    // /: `+` in the narrow profile is arithmetic, so a scalar
    // program builds under the default memory model instead of requiring
    // `--model ownership`.
    let ast = tree("sum(a, b) { return a + b; } main() { return sum(2, 3); }").unwrap();
    assert!(ast.lifetimes, "the test means to exercise lifetimes");
    flow::check(&ast).unwrap();
    let scalar = llvm::emit(&ast, Target::MacX86_64).unwrap();
    assert!(
        scalar.contains("Zebulon scalar native IR"),
        "not the narrow profile: {scalar}"
    );
    // Arithmetic, not a tagged runtime call deciding by tag.
    assert!(
        scalar.contains("add i64"),
        "no scalar add emitted: {scalar}"
    );
    // The wide profile keeps the polymorphic `+`, which is what text needs.
    let wide = llvm::emit_objects(&ast, Target::MacX86_64).unwrap();
    assert!(!wide.is_empty());
}

#[test]
fn named_label_families_are_checked_in_calls_and_rows() {
    for use_ in [
        "turn(t){return exits.get(study, red);}",
        "hall: object exits(study, red); turn(t){return nil;}",
    ] {
        let e = check(&format!("{EXITS}enum Colour: red; study: object; {use_}")).unwrap_err();
        assert_eq!(e.code, "sem-enum-family");
        assert!(
            e.message.contains("Direction") && e.message.contains("Colour"),
            "{e:?}"
        );
    }
    assert_eq!(
        check("relation exits(a, b, way: Missing) one_to_one; turn(t){return nil;}")
            .unwrap_err()
            .code,
        "sem-enum-family"
    );
    for declaration in [
        "enum Direction: east;",
        "Direction: object;",
        "property Direction;",
        "Direction(){return nil;}",
        "enum Direction;",
    ] {
        assert_eq!(
            check(&format!("{EXITS}{declaration} turn(t){{return nil;}}"))
                .unwrap_err()
                .code,
            "sem-enum-family"
        );
    }
    check("enum a,b; relation links(x,y,label) one_to_one; x:object; y:object; turn(t){links.set(x,y,a);return links.get(x,a);}").unwrap();
}

#[test]
fn family_checks_respect_shadowing_and_the_packed_label_limit() {
    check(&format!("{EXITS}enum Colour:red; study:object; wrong(red){{return exits.get(study,red);}} turn(t){{local red=north;return exits.get(study,red);}}")).unwrap();
    let padding = (0..4096)
        .map(|n| format!("e{n}"))
        .collect::<Vec<_>>()
        .join(",");
    let error=check(&format!("enum {padding}; enum Direction:north; relation exits(a,b,via:Direction) one_to_one; turn(t){{return nil;}}")).unwrap_err();
    assert!(error.message.contains("12-bit"));
}

#[test]
fn reverse_outermost_names_the_documented_rule() {
    let e = check(&format!(
        "{CONTAINS}coin:object; turn(t){{return location.outermost(coin);}}"
    ))
    .unwrap_err();
    assert!(e.message.contains("Reverse relation operations"));
    for operation in ["get", "all", "descendants", "ancestors"] {
        check(&format!(
            "{CONTAINS}coin:object; turn(t){{return location.{operation}(coin);}}"
        ))
        .unwrap();
    }
}
