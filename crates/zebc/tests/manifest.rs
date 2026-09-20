#![forbid(unsafe_code)]
//! Bundle descriptions are derived from independent source declarations.
use std::path::{Path, PathBuf};
use zeb_frontend::{
    parser::{self, Model},
    preprocess,
    source::Encoding,
};
use zebc::manifest::{self, Entry, Json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/zebc has two ancestors")
        .to_path_buf()
}

fn describe(program: &str) -> manifest::Manifest {
    let path = root().join("tests/native/manifest").join(program);
    let includes = vec![];
    let processed = preprocess::read(&path, &includes, Encoding::Utf8, 16 * 1024 * 1024)
        .unwrap_or_else(|e| panic!("{program}: {}", e.diagnostic.message));
    let ast = parser::parse_with(&processed.source, Model::Lifetimes)
        .unwrap_or_else(|d| panic!("{program}: {}", d.message));
    let declared = |name: &str, arity: usize| {
        ast.functions.iter().any(|id| {
            matches!(&ast.nodes[id.0].syntax,
                zeb_frontend::parser::Syntax::Function { name: found, parameters, .. }
                    if found == name && parameters.len() == arity)
        })
    };
    let entry = Entry {
        startup: declared("startup", 0),
        turn: declared("turn", 1),
        act: declared("act", 2),
        recover: declared("recover", 1),
        token: zeb_frontend::sema::enumerator(&ast, "tokWord"),
    };
    manifest::describe(
        &ast,
        &"0".repeat(64),
        &"1".repeat(64),
        false,
        entry,
        "object-shared-v12",
        12,
    )
}

#[test]
fn every_program_describes_itself_within_the_schema() {
    for program in ["text.t", "action.t", "combined.t"] {
        let described = describe(program);
        manifest::validate(&described)
            .unwrap_or_else(|e| panic!("{program} does not validate: {e}"));
        assert!(
            described.json().render().ends_with("}\n"),
            "{program} renders a whole document"
        );
    }
}

#[test]
fn the_manifest_says_which_entry_points_a_game_declared() {
    let parses = describe("text.t");
    assert!(parses.entry.turn, "text.t takes a line");
    assert!(!parses.entry.act, "and does not take an action");

    let clicks = describe("action.t");
    assert!(clicks.entry.act, "action.t takes an action");
    assert!(!clicks.entry.turn, "and cannot be typed at");
}

#[test]
fn bundle_describes_declared_relations() {
    let described = describe("text.t");
    let named = |name: &str| described.relations.iter().find(|r| r.forward == name);
    let contains = named("contains").expect("source declares contains");
    assert_eq!(contains.reverse.as_deref(), Some("location"));
    assert_eq!(contains.cardinality, "one_to_many");
    assert!(!contains.labelled);

    let exits = named("exits").expect("source declares exits");
    assert!(exits.labelled, "travel is a family of tables ()");

    let vocab = named("vocab").expect("source declares vocab");
    assert!(vocab.vocabulary, "vocabulary lives in the dictionary");

    // Directions are enumerators, and a host reading a label needs their values.
    for way in ["north", "south", "east", "west", "up", "down"] {
        assert!(
            described.enumerators.contains_key(way),
            "{way} is not in the manifest's enumerators"
        );
    }
}

#[test]
fn a_games_own_entities_are_named_with_their_classes() {
    let described = describe("combined.t");
    let entity = |name: &str| described.entities.iter().find(|e| e.name == name);
    let meter = entity("meter").expect("the meter is declared");
    assert!(!meter.is_class);
    assert_eq!(meter.parents, vec!["Sensor".to_owned()]);
    let room = entity("station").expect("the station is declared");
    assert_eq!(room.parents, vec!["Station".to_owned()]);
    assert!(
        entity("Thing").is_some_and(|thing| thing.is_class),
        "a class is marked as one"
    );
}

#[test]
fn a_modified_object_keeps_the_parents_its_author_wrote() {
    let described = describe("combined.t");
    assert!(
        !described.entities.iter().any(|e| e.name.starts_with('$')),
        "a synthesised object reached the manifest"
    );
    assert!(
        !described
            .entities
            .iter()
            .any(|e| e.parents.iter().any(|p| p.starts_with('$'))),
        "a synthesised name reached the manifest as a parent"
    );
    let meter = described
        .entities
        .iter()
        .find(|e| e.name == "meter")
        .expect("the meter is declared");
    assert_eq!(meter.parents, vec!["Sensor".to_owned()]);
}

#[test]
fn the_manifest_and_the_header_agree() {
    let described = describe("text.t");
    let header = described.header();
    let Some(Json::Arr(exports)) = described.json().get("exports").cloned() else {
        panic!("the manifest has no exports array");
    };
    assert_eq!(exports.len(), manifest::EXPORTS.len());
    for export in &exports {
        let Some(Json::Str(name)) = export.get("name") else {
            panic!("an export has no name");
        };
        assert!(
            header.contains(&format!("{name}(")),
            "the header omits {name}"
        );
        assert!(export.get("returns").is_some() && export.get("parameters").is_some());
    }
}
