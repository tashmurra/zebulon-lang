#![forbid(unsafe_code)]
//! a bundle describes its own surface.
//!
//! `zebc build --emit shared` already writes a C header and a working consumer.
//! Both are written for a person reading them, and neither says what the bundle
//! actually contains — which relations exist, what their columns hold, which
//! entry points the game declared, what an inspection kind answers. An engine's
//! import step and a binding generator read none of it.
//!
//! This is that description, and it is the **source** rather than a summary:
//! the header and the consumer are rendered from the same table the manifest
//! is, so the two cannot drift apart. A name that appears in one appears in all
//! three or in none.
//!
//! No JSON dependency: the workspace has none at all, and a manifest is not a
//! reason to acquire one. `Json` below is a value tree with a renderer, and
//! `Manifest::validate` is the schema, checked before anything is written.

use std::collections::BTreeMap;
use zeb_frontend::parser::{Ast, Syntax};

/// The manifest format. Raised when a consumer would have to be changed; a new
/// field that an old reader can ignore does not raise it.
pub const SCHEMA: u32 = 1;

/* ------------------------------------------------------------------- json */

/// A JSON value. Object keys keep insertion order, so a manifest built twice
/// from the same bundle is byte-identical — which is what lets it go in the
/// identity digest and what lets a test diff two builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Str(String),
    Num(i64),
    Bool(bool),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn str(text: impl Into<String>) -> Self {
        Json::Str(text.into())
    }

    /// Look a key up in an object, for validation and for tests.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Render with two-space indentation. Arrays of scalars stay on one line;
    /// arrays of objects get a line each, because that is the shape a person
    /// ends up reading in a diff.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        let inner = "  ".repeat(depth + 1);
        match self {
            Json::Str(text) => {
                out.push('"');
                for ch in text.chars() {
                    match ch {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            out.push_str(&format!("\\u{:04x}", c as u32));
                        }
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            Json::Num(value) => out.push_str(&value.to_string()),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Arr(items) if items.is_empty() => out.push_str("[]"),
            Json::Arr(items) => {
                let scalars = items
                    .iter()
                    .all(|item| matches!(item, Json::Str(_) | Json::Num(_) | Json::Bool(_)));
                if scalars {
                    out.push('[');
                    for (at, item) in items.iter().enumerate() {
                        if at > 0 {
                            out.push_str(", ");
                        }
                        item.write(out, 0);
                    }
                    out.push(']');
                } else {
                    out.push_str("[\n");
                    for (at, item) in items.iter().enumerate() {
                        out.push_str(&inner);
                        item.write(out, depth + 1);
                        if at + 1 < items.len() {
                            out.push(',');
                        }
                        out.push('\n');
                    }
                    out.push_str(&pad);
                    out.push(']');
                }
            }
            Json::Obj(fields) if fields.is_empty() => out.push_str("{}"),
            // A small object of scalars stays on one line. A relation, a
            // parameter or an entity is one row of a table conceptually, and
            // five lines each turns the file into something nobody reads.
            Json::Obj(fields)
                if fields.iter().all(|(_, value)| {
                    matches!(value, Json::Str(_) | Json::Num(_) | Json::Bool(_))
                        || matches!(value, Json::Arr(items)
                            if items.iter().all(|i| matches!(i, Json::Str(_) | Json::Num(_) | Json::Bool(_))))
                }) =>
            {
                let mut line = String::from("{ ");
                for (at, (key, value)) in fields.iter().enumerate() {
                    if at > 0 {
                        line.push_str(", ");
                    }
                    Json::Str(key.clone()).write(&mut line, 0);
                    line.push_str(": ");
                    value.write(&mut line, 0);
                }
                line.push_str(" }");
                if line.chars().count() + pad.chars().count() <= 118 {
                    out.push_str(&line);
                    return;
                }
                self.write_block(out, depth);
            }
            Json::Obj(_) => self.write_block(out, depth),
        }
    }

    /// An object, a field per line.
    fn write_block(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        let inner = "  ".repeat(depth + 1);
        match self {
            Json::Obj(fields) => {
                out.push_str("{\n");
                for (at, (key, value)) in fields.iter().enumerate() {
                    out.push_str(&inner);
                    Json::Str(key.clone()).write(out, depth + 1);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                    if at + 1 < fields.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push('}');
            }
            other => other.write(out, depth),
        }
    }
}

/* ---------------------------------------------------------------- exports */

/// One exported C function.
///
/// The suffix is what follows the bundle symbol; the empty one is the entry
/// point itself. Held as data rather than written into a header template so
/// that the header, the manifest and the symbol check all read the same list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Export {
    pub suffix: &'static str,
    pub returns: &'static str,
    pub parameters: &'static [(&'static str, &'static str)],
    /// What the call is for, so a binding generator can group and a reader can
    /// find: `session`, `output`, `reply`, `inspect` or `literal`.
    pub group: &'static str,
}

/// The v8 bundle ABI, in the order the header has always listed it.
pub const EXPORTS: &[Export] = &[
    Export {
        suffix: "_persistence",
        returns: "uint64_t",
        parameters: &[
            ("slot", "uint64_t"),
            ("op", "uint32_t"),
            ("value", "uint64_t"),
        ],
        group: "save",
    },
    Export {
        suffix: "",
        returns: "uint64_t",
        parameters: &[
            ("abi", "uint32_t"),
            ("slot", "uint64_t"),
            ("objects", "uint64_t"),
            ("properties", "uint64_t"),
            ("output_limit", "uint64_t"),
        ],
        group: "session",
    },
    Export {
        suffix: "_reset",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t")],
        group: "session",
    },
    Export {
        suffix: "_discard",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t")],
        group: "session",
    },
    Export {
        suffix: "_poll",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t")],
        group: "session",
    },
    Export {
        suffix: "_output_byte",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t"), ("offset", "uint64_t")],
        group: "output",
    },
    Export {
        suffix: "_output_len",
        returns: "uint64_t",
        parameters: &[("slot", "uint64_t")],
        group: "output",
    },
    Export {
        suffix: "_output_word",
        returns: "uint64_t",
        parameters: &[("slot", "uint64_t"), ("offset", "uint64_t")],
        group: "output",
    },
    Export {
        suffix: "_drained",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t")],
        group: "output",
    },
    // the semantic events a turn produced, beside its text.
    Export {
        suffix: "_event_len",
        returns: "uint64_t",
        parameters: &[("slot", "uint64_t")],
        group: "events",
    },
    Export {
        suffix: "_event_word",
        returns: "uint64_t",
        parameters: &[("slot", "uint64_t"), ("offset", "uint64_t")],
        group: "events",
    },
    Export {
        suffix: "_reply_byte",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t"), ("byte", "uint32_t")],
        group: "reply",
    },
    Export {
        suffix: "_reply_action",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t"), ("verb", "uint32_t")],
        group: "reply",
    },
    Export {
        suffix: "_reply_subject",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t"), ("handle", "uint64_t")],
        group: "reply",
    },
    Export {
        suffix: "_reply_value",
        returns: "uint32_t",
        parameters: &[
            ("slot", "uint64_t"),
            ("tag", "uint32_t"),
            ("payload", "uint64_t"),
        ],
        group: "reply",
    },
    Export {
        suffix: "_resume",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t")],
        group: "reply",
    },
    Export {
        suffix: "_close",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t")],
        group: "session",
    },
    Export {
        suffix: "_finish",
        returns: "uint32_t",
        parameters: &[("slot", "uint64_t"), ("outcome", "uint64_t")],
        group: "session",
    },
    Export {
        suffix: "_outcome",
        returns: "uint64_t",
        parameters: &[("slot", "uint64_t")],
        group: "session",
    },
    Export {
        suffix: "_inspect",
        returns: "uint32_t",
        parameters: &[
            ("slot", "uint64_t"),
            ("kind", "uint32_t"),
            ("a", "uint64_t"),
            ("b", "uint64_t"),
        ],
        group: "inspect",
    },
    Export {
        suffix: "_result_len",
        returns: "uint64_t",
        parameters: &[("slot", "uint64_t")],
        group: "inspect",
    },
    Export {
        suffix: "_result",
        returns: "uint64_t",
        parameters: &[("slot", "uint64_t"), ("index", "uint64_t")],
        group: "inspect",
    },
    Export {
        suffix: "_text_byte",
        returns: "uint32_t",
        parameters: &[("literal", "uint32_t"), ("offset", "uint64_t")],
        group: "literal",
    },
];

/// What each inspection kind answers. A host cannot discover
/// this by trying: legacy queries may answer nothing; framed queries publish
/// their status and limits explicitly.
const INSPECT_KINDS: &[(i64, &str, &str)] = &[
    (
        0,
        "relation",
        "partners of `b` in the relation `a` names; bit 12 reads it the other way, and bit 13 says bits 16-27 carry a label",
    ),
    (
        1,
        "properties",
        "the property ids `b` defines itself, not inherited",
    ),
    (2, "prototypes", "what `b` derives from"),
    (3, "entities", "every world entity, bounded"),
    (
        5,
        "declared",
        "handles at declared indices a..a+b (b=0 means one); absent indices omitted",
    ),
    (
        4,
        "presentation",
        "triples of property id, value tag and payload for the properties `b` lists under the property id `a`; nothing is evaluated",
    ),
    (
        6,
        "scene",
        "a=(presentation_property<<32)|containment_relation. Optional a bit24 enables membership relation in bits12..23, reversed by bit25. b=root handle. Header [1,status,records]; record [words,handle,parent,resident,property_count,property/tag/payload triples]. Includes containment descendants and presented references. No evaluation. status=0 complete; nonzero means no records. Limits: 4096 entities, 65536 words, 256 properties/entity",
    ),
    (
        7,
        "presentation_range",
        "a=presentation property, b=(count<<32)|first declared index, count=0 means one. Header [1,status,records]; record [words,index,handle,property/tag/payload triples]. Absent index has handle 0. Limits: 4096 indices, 65536 words, 256 properties/entity",
    ),
    (
        8,
        "relation_range",
        "a=kind-0 descriptor, b=(count<<32)|first declared index, count=0 means one. Header [1,status,records]; record [words,index,handle,partner handles]. Absent index has handle 0. Limits: 4096 indices, 65536 words",
    ),
];

/// What a value tag means in a presentation answer. Text built at run
/// time and anything that would have to be evaluated are named, not answered.
const VALUE_TAGS: &[(i64, &str, &str)] = &[
    (0, "nil", "no value"),
    (1, "bool", "the payload is 0 or 1"),
    (
        2,
        "int",
        "the payload is the value as a 32-bit two's complement",
    ),
    (3, "entity", "the payload is a handle"),
    (
        4,
        "literal",
        "the payload is a literal index; read it with `text_byte`",
    ),
    (
        5,
        "enumerator",
        "the payload is the enumerator's value, which `enumerators` names",
    ),
    (
        7,
        "text",
        "built at run time; not readable without evaluating",
    ),
    (9, "list", "not readable"),
    (
        15,
        "computed",
        "a method or a function; reading it would run the game",
    ),
];

/* --------------------------------------------------------------- manifest */

/// Which entry points a game declared, and the tokenizer's word enumerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub startup: bool,
    /// A line intent reaches this; a game that never parses has none.
    pub turn: bool,
    /// An action intent reaches this.
    pub act: bool,
    pub recover: bool,
    pub token: Option<u32>,
}

/// One relation, and how to name it across the inspection boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub index: usize,
    pub forward: String,
    pub reverse: Option<String>,
    pub cardinality: &'static str,
    pub labelled: bool,
    pub vocabulary: bool,
}

/// One declared object, by the name its author gave it.
///
/// This is the only thing about an entity that is the same in the next session
/// and was visible to whoever built the engine project, which is why a stable
/// entity id has to be built out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    pub name: String,
    pub is_class: bool,
    pub parents: Vec<String>,
    /// The declaration index the world is built with. Inspection kind
    /// 5 answers the handle bound to it, which is how a name an engine project
    /// was built around joins a session where everything is a fresh handle.
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub identity: String,
    pub symbol: String,
    pub profile: String,
    pub abi: u32,
    pub optimize: bool,
    pub model: &'static str,
    pub game: String,
    pub runtime: String,
    pub targets: Vec<&'static str>,
    pub entry: Entry,
    pub relations: Vec<Relation>,
    pub enumerators: BTreeMap<String, u32>,
    /// Every property name and the id it is known by across the boundary.
    /// A presentation answer names properties by id, so without this
    /// a host would be reading numbers.
    pub properties: BTreeMap<String, u32>,
    pub entities: Vec<Entity>,
}

/// Read a bundle's surface out of what was compiled.
pub fn describe(
    ast: &Ast,
    identity: &str,
    runtime_identity: &str,
    optimize: bool,
    entry: Entry,
    profile: &str,
    abi: u32,
) -> Manifest {
    let relations = ast
        .relations
        .iter()
        .enumerate()
        .map(|(index, declared)| Relation {
            index,
            forward: declared.forward.clone(),
            reverse: declared.reverse.clone(),
            cardinality: match declared.cardinality {
                0 => "one_to_one",
                1 => "one_to_many",
                _ => "many_to_many",
            },
            labelled: declared.labelled,
            vocabulary: declared.vocabulary,
        })
        .collect();
    let mut enumerators = BTreeMap::new();
    let mut next = 0u32;
    for node in &ast.nodes {
        if let Syntax::Declaration(zeb_frontend::parser::Declaration::Enumerators {
            names, ..
        }) = &node.syntax
        {
            for name in names {
                enumerators.entry(name.clone()).or_insert(next);
                next += 1;
            }
        }
    }
    let entities = declared_entities(ast);
    let mut properties = BTreeMap::new();
    if let Ok(plan) = zeb_frontend::object_init::declarations(ast) {
        for (id, name) in plan.properties.iter().enumerate() {
            if let Ok(id) = u32::try_from(id) {
                properties.insert(name.clone(), id);
            }
        }
    }
    Manifest {
        identity: identity.to_owned(),
        symbol: format!("zeb_game_{identity}_v{abi}"),
        profile: profile.to_owned(),
        abi,
        optimize,
        model: if ast.lifetimes {
            "lifetimes"
        } else {
            "ownership"
        },
        game: format!("libzeb_game_{identity}.dylib"),
        // the runtime's **own** identity, not the bundle's. It holds
        // nothing of the game any more, so two programs built with the same
        // tools and options name — and are — the same library.
        runtime: format!("libzeb_runtime_{runtime_identity}.dylib"),
        targets: vec!["macos-x86_64", "macos-arm64"],
        entry,
        relations,
        enumerators,
        properties,
        entities,
    }
}

/// A name the compiler made up rather than one an author wrote.
///
/// `modify keeper` renames the original object to `$keeper$modified305` and
/// gives its name to the modification, whose parent is that private base. Both
/// are objects, and only one of them was declared by anybody. A manifest that
/// published the other would be handing an engine a symbol that changes when an
/// unrelated declaration moves — and calling it an entity id.
fn is_synthesised(name: &str) -> bool {
    name.starts_with('$')
}

/// Declared objects, with the parents their author wrote and the index the
/// running world binds them by.
///
/// A modified object's immediate parent is the private base `modify` made, so
/// the chain is followed until it reaches names that were actually written.
/// Modifying something twice layers twice, hence the loop.
fn declared_entities(ast: &Ast) -> Vec<Entity> {
    let mut parents_of: BTreeMap<&str, &Vec<String>> = BTreeMap::new();
    for id in &ast.objects {
        if let Syntax::Object { name, parents, .. } = &ast.nodes[id.0].syntax {
            parents_of.insert(name.as_str(), parents);
        }
    }
    let resolve = |parents: &Vec<String>| -> Vec<String> {
        let mut settled: Vec<String> = Vec::new();
        let mut pending: Vec<String> = parents.clone();
        // A chain is as long as the modifications of one object, which is small;
        // the bound is here so a cycle cannot spin rather than because one can.
        for _ in 0..64 {
            if pending.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for parent in pending {
                match parents_of.get(parent.as_str()) {
                    Some(grandparents) if is_synthesised(&parent) => {
                        next.extend(grandparents.iter().cloned());
                    }
                    _ if is_synthesised(&parent) => {}
                    _ => settled.push(parent),
                }
            }
            pending = next;
        }
        settled
    };
    // The index is the position in the declaration plan, which is what the
    // emitted initializer binds each object by — not the position in this
    // filtered list, which skips dictionaries and the private bases `modify`
    // makes.
    let planned: Vec<String> = zeb_frontend::object_init::declarations(ast)
        .map(|plan| plan.objects.iter().map(|o| o.name.clone()).collect())
        .unwrap_or_default();
    ast.objects
        .iter()
        .filter_map(|id| match &ast.nodes[id.0].syntax {
            Syntax::Object {
                name,
                is_class,
                parents,
                is_dictionary,
                ..
            } if !is_dictionary && !is_synthesised(name) => Some(Entity {
                name: name.clone(),
                is_class: *is_class,
                parents: resolve(parents),
                index: u32::try_from(
                    planned
                        .iter()
                        .position(|planned| planned == name)
                        .unwrap_or(0),
                )
                .unwrap_or(0),
            }),
            _ => None,
        })
        .collect()
}

impl Manifest {
    /// Every C name this bundle is supposed to define.
    pub fn exported_names(&self) -> Vec<String> {
        EXPORTS
            .iter()
            .map(|export| format!("{}{}", self.symbol, export.suffix))
            .collect()
    }

    /// The C header, rendered from the same export table the manifest is, so a
    /// name cannot appear in one and not the other.
    pub fn header(&self) -> String {
        let mut out = String::from(&format!(
            "#pragma once\n#include <stdint.h>\n/* Private exact-bundle ABI v{}. Pass it as the entry's first argument; the\n   bundle refuses any other. The host drives the cycle: reset a slot, run the\n   game on a worker thread, then poll/drain/reply/resume until the session ends.\n   Every call names its session slot, so one process may run several. */\n",
            self.abi
        ));
        for export in EXPORTS {
            let parameters = export
                .parameters
                .iter()
                .map(|(name, kind)| format!("{kind} {name}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "{} {}{}({parameters});\n",
                export.returns, self.symbol, export.suffix
            ));
        }
        out
    }

    /// The manifest as a value tree. Rendered by `Json::render`.
    pub fn json(&self) -> Json {
        let entry = &self.entry;
        Json::Obj(vec![
            ("schema".into(), Json::Num(i64::from(SCHEMA))),
            (
                "scalar_transport".into(),
                Json::Obj(vec![
                    ("version".into(), Json::Num(1)),
                    ("packet_limit".into(), Json::Num(65536)),
                    (
                        "tags".into(),
                        Json::Arr(vec![
                            Json::str("nil=0"),
                            Json::str("true=1"),
                            Json::str("i32=2"),
                            Json::str("entity=3"),
                            Json::str("utf8=4"),
                        ]),
                    ),
                    (
                        "event_tail".into(),
                        Json::str(
                            "optional u64 count; repeated u64 tag, u64 byte_length, payload padded to 8 bytes; little-endian",
                        ),
                    ),
                    (
                        "reply_text".into(),
                        Json::str("reply_byte UTF-8 bytes then reply_value tag=4 payload=0"),
                    ),
                ]),
            ),
            (
                "bundle".into(),
                Json::Obj(vec![
                    ("profile".into(), Json::str(&self.profile)),
                    ("abi".into(), Json::Num(i64::from(self.abi))),
                    ("identity".into(), Json::str(&self.identity)),
                    ("symbol".into(), Json::str(&self.symbol)),
                    ("game".into(), Json::str(&self.game)),
                    ("runtime".into(), Json::str(&self.runtime)),
                    ("optimize".into(), Json::Bool(self.optimize)),
                    (
                        "targets".into(),
                        Json::Arr(self.targets.iter().map(|t| Json::str(*t)).collect()),
                    ),
                ]),
            ),
            (
                "entry".into(),
                Json::Obj(vec![
                    ("model".into(), Json::str(self.model)),
                    ("startup".into(), Json::Bool(entry.startup)),
                    ("turn".into(), Json::Bool(entry.turn)),
                    ("act".into(), Json::Bool(entry.act)),
                    ("recover".into(), Json::Bool(entry.recover)),
                    (
                        "token".into(),
                        entry
                            .token
                            .map_or(Json::Bool(false), |value| Json::Num(i64::from(value))),
                    ),
                ]),
            ),
            (
                "exports".into(),
                Json::Arr(
                    EXPORTS
                        .iter()
                        .map(|export| {
                            Json::Obj(vec![
                                (
                                    "name".into(),
                                    Json::str(format!("{}{}", self.symbol, export.suffix)),
                                ),
                                ("group".into(), Json::str(export.group)),
                                ("returns".into(), Json::str(export.returns)),
                                (
                                    "parameters".into(),
                                    Json::Arr(
                                        export
                                            .parameters
                                            .iter()
                                            .map(|(name, kind)| {
                                                Json::Obj(vec![
                                                    ("name".into(), Json::str(*name)),
                                                    ("type".into(), Json::str(*kind)),
                                                ])
                                            })
                                            .collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "relations".into(),
                Json::Arr(
                    self.relations
                        .iter()
                        .map(|relation| {
                            let mut fields = vec![
                                ("index".into(), Json::Num(relation.index as i64)),
                                ("forward".into(), Json::str(&relation.forward)),
                                ("cardinality".into(), Json::str(relation.cardinality)),
                                ("labelled".into(), Json::Bool(relation.labelled)),
                                ("vocabulary".into(), Json::Bool(relation.vocabulary)),
                            ];
                            if let Some(reverse) = &relation.reverse {
                                fields.insert(2, ("reverse".into(), Json::str(reverse)));
                            }
                            // What to pass as `a` to inspect kind 0. A labelled
                            // relation has no answer yet: the descriptor has no
                            // room for the label.
                            if !relation.vocabulary && !relation.labelled {
                                fields.push((
                                    "descriptor".into(),
                                    Json::Obj(vec![
                                        ("forward".into(), Json::Num(relation.index as i64)),
                                        ("reverse".into(), Json::Num(relation.index as i64 + 4096)),
                                    ]),
                                ));
                            }
                            Json::Obj(fields)
                        })
                        .collect(),
                ),
            ),
            (
                "enumerators".into(),
                Json::Obj(
                    self.enumerators
                        .iter()
                        .map(|(name, value)| (name.clone(), Json::Num(i64::from(*value))))
                        .collect(),
                ),
            ),
            (
                "values".into(),
                Json::Arr(
                    VALUE_TAGS
                        .iter()
                        .map(|(tag, name, means)| {
                            Json::Obj(vec![
                                ("tag".into(), Json::Num(*tag)),
                                ("name".into(), Json::str(*name)),
                                ("means".into(), Json::str(*means)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "properties".into(),
                Json::Obj(
                    self.properties
                        .iter()
                        .map(|(name, id)| (name.clone(), Json::Num(i64::from(*id))))
                        .collect(),
                ),
            ),
            (
                "inspect".into(),
                Json::Arr(
                    INSPECT_KINDS
                        .iter()
                        .map(|(kind, name, answers)| {
                            Json::Obj(vec![
                                ("kind".into(), Json::Num(*kind)),
                                ("name".into(), Json::str(*name)),
                                ("answers".into(), Json::str(*answers)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "entities".into(),
                Json::Arr(
                    self.entities
                        .iter()
                        .map(|entity| {
                            Json::Obj(vec![
                                ("name".into(), Json::str(&entity.name)),
                                ("index".into(), Json::Num(i64::from(entity.index))),
                                ("class".into(), Json::Bool(entity.is_class)),
                                (
                                    "parents".into(),
                                    Json::Arr(entity.parents.iter().map(Json::str).collect()),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

/* -------------------------------------------------------------- the schema */

/// The schema, checked before anything is written.
///
/// A manifest is read by a binding generator and by an engine's import step, so
/// a bundle that would ship a malformed one should fail the build rather than
/// ship it. This is why the check is here and not only in a test: a test proves
/// the cases it was given, and this proves every build.
pub fn validate(manifest: &Manifest) -> Result<(), String> {
    let json = manifest.json();
    let require = |key: &str| -> Result<&Json, String> {
        json.get(key)
            .ok_or_else(|| format!("manifest has no {key}"))
    };
    if require("schema")? != &Json::Num(i64::from(SCHEMA)) {
        return Err("manifest schema is not the current one".to_owned());
    }
    for key in [
        "bundle",
        "entry",
        "exports",
        "relations",
        "enumerators",
        "inspect",
        "entities",
    ] {
        require(key)?;
    }
    if manifest.identity.len() != 64 || !manifest.identity.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("manifest identity is not a digest".to_owned());
    }
    if !manifest.symbol.starts_with("zeb_game_")
        || !manifest.symbol.ends_with(&format!("_v{}", manifest.abi))
    {
        return Err(format!(
            "manifest symbol {} is not shaped like one",
            manifest.symbol
        ));
    }
    // A host-driven world needs at least one text or structured entry point.
    if manifest.model == "lifetimes" && !manifest.entry.turn && !manifest.entry.act {
        return Err("manifest declares no way into the game".to_owned());
    }
    // Two relations answering to one name would make an inspection ambiguous.
    let mut names = Vec::new();
    for relation in &manifest.relations {
        for name in std::iter::once(&relation.forward).chain(relation.reverse.iter()) {
            if names.contains(&name.as_str()) {
                return Err(format!("two relations answer to {name}"));
            }
            names.push(name);
        }
        if relation.index >= 4096 {
            return Err("a relation index does not fit an inspection descriptor".to_owned());
        }
    }
    // An entity id has to be unique or it cannot be an id.
    let mut declared: Vec<&str> = manifest.entities.iter().map(|e| e.name.as_str()).collect();
    declared.sort_unstable();
    if let Some(pair) = declared.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("two entities are declared as {}", pair[0]));
    }
    Ok(())
}

/// Which of the manifest's exports the built library does not actually define.
///
/// A description that is merely plausible is worse than none: a binding
/// generator reads it and emits code that links against nothing. `symbols` is
/// `llvm-nm --defined-only --extern-only` output, whose last column is the
/// name. Mach-O decorates a C symbol with a leading underscore, so both
/// spellings count as defined.
pub fn missing_exports(manifest: &Manifest, symbols: &str) -> Vec<String> {
    let defined: Vec<&str> = symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .collect();
    manifest
        .exported_names()
        .into_iter()
        .filter(|name| {
            !defined.contains(&name.as_str()) && !defined.contains(&format!("_{name}").as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeb_frontend::{
        parser,
        source::{Encoding, Source},
    };

    fn manifest_of(source: &str) -> Manifest {
        let source = Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap();
        let ast = parser::parse_with(&source, parser::Model::Lifetimes).unwrap();
        describe(
            &ast,
            &"a".repeat(64),
            &"b".repeat(64),
            false,
            Entry {
                startup: true,
                turn: true,
                act: false,
                recover: false,
                token: Some(0),
            },
            "object-shared-v9",
            8,
        )
    }

    const WORLD: &str = "\
enum token tokWord;
enum Direction: north, south;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
relation exits(from: Entity, to: Entity, way: Direction) one_to_one;
class Thing: object name = 'thing';
hall: Thing name = 'hall';
";

    /// The point of the manifest: what a bundle contains, read from what was
    /// compiled rather than from a hand-maintained list.
    #[test]
    fn a_manifest_describes_what_was_compiled() {
        let manifest = manifest_of(WORLD);
        validate(&manifest).unwrap();
        assert_eq!(manifest.relations.len(), 2);
        assert_eq!(manifest.relations[0].forward, "contains");
        assert_eq!(manifest.relations[0].reverse.as_deref(), Some("location"));
        assert_eq!(manifest.relations[0].cardinality, "one_to_many");
        assert!(manifest.relations[1].labelled, "exits has a third column");
        assert_eq!(manifest.enumerators.get("north"), Some(&1));
        let names: Vec<&str> = manifest.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"hall"),
            "a declared object is an entity: {names:?}"
        );
        assert!(names.contains(&"Thing"), "so is a class, marked as one");
        assert!(
            manifest
                .entities
                .iter()
                .any(|e| e.name == "Thing" && e.is_class)
        );
    }

    /// A labelled relation deliberately gets no descriptor: inspection's `a`
    /// carries a relation and a direction and no label, so answering one would
    /// be a promise the boundary cannot keep.
    #[test]
    fn only_an_answerable_relation_gets_a_descriptor() {
        let manifest = manifest_of(WORLD);
        let relations = manifest.json();
        let relations = relations.get("relations").unwrap();
        let Json::Arr(items) = relations else {
            panic!("relations is not an array")
        };
        assert!(
            items[0].get("descriptor").is_some(),
            "contains is answerable"
        );
        assert!(
            items[1].get("descriptor").is_none(),
            "exits is labelled, and inspection cannot ask for a label yet"
        );
    }

    /// The header is rendered from the export table, so the manifest and the
    /// header cannot disagree about what exists.
    #[test]
    fn the_header_and_the_manifest_name_the_same_functions() {
        let manifest = manifest_of(WORLD);
        let header = manifest.header();
        for name in manifest.exported_names() {
            assert!(
                header.contains(&format!("{name}(")),
                "header is missing {name}"
            );
        }
        assert_eq!(
            header.matches(&manifest.symbol).count(),
            EXPORTS.len(),
            "the header declares exactly the exported names"
        );
    }

    /// Rendering is deterministic, which is what lets a manifest be diffed and
    /// what would let it join the identity digest.
    #[test]
    fn rendering_is_stable() {
        let first = manifest_of(WORLD).json().render();
        let second = manifest_of(WORLD).json().render();
        assert_eq!(first, second);
        assert!(first.ends_with("}\n"));
    }

    #[test]
    fn the_schema_refuses_a_bundle_with_no_way_in() {
        let mut manifest = manifest_of(WORLD);
        manifest.entry.turn = false;
        manifest.entry.act = false;
        assert!(validate(&manifest).unwrap_err().contains("no way into"));
    }

    #[test]
    fn the_schema_refuses_a_duplicated_entity_id() {
        let mut manifest = manifest_of(WORLD);
        let first = manifest.entities[0].clone();
        manifest.entities.push(first);
        assert!(validate(&manifest).unwrap_err().contains("two entities"));
    }

    /// The manifest may not name a function the bundle does not define. This is
    /// the check that makes the description evidence rather than a claim.
    #[test]
    fn an_export_the_library_does_not_define_is_found() {
        let manifest = manifest_of(WORLD);
        let names = manifest.exported_names();
        // Mach-O's leading underscore counts as the same symbol.
        let complete = names
            .iter()
            .map(|name| format!("0000000000001234 T _{name}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(missing_exports(&manifest, &complete).is_empty());
        // An unadorned name counts too, for a platform that does not decorate.
        let plain = names.join("\n");
        assert!(missing_exports(&manifest, &plain).is_empty());
        // One dropped is one reported, by name.
        let short = complete
            .lines()
            .filter(|line| !line.ends_with("_inspect"))
            .collect::<Vec<_>>()
            .join("\n");
        let missing = missing_exports(&manifest, &short);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].ends_with("_inspect"), "{missing:?}");
        // And an empty symbol table means every one of them.
        assert_eq!(missing_exports(&manifest, "").len(), EXPORTS.len());
    }

    /// Text in a manifest is JSON text. A game's object names come from source
    /// and a description comes from a table, so both have to survive escaping.
    #[test]
    fn strings_are_escaped() {
        let awkward = Json::str("a \"quoted\" \\ path\nand a tab\t");
        assert_eq!(
            awkward.render().trim_end(),
            "\"a \\\"quoted\\\" \\\\ path\\nand a tab\\t\""
        );
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;
    use zeb_frontend::{
        parser,
        source::{Encoding, Source},
    };

    fn described(source: &str) -> Manifest {
        let source = Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap();
        let ast = parser::parse_with(&source, parser::Model::Lifetimes).unwrap();
        describe(
            &ast,
            &"b".repeat(64),
            &"c".repeat(64),
            false,
            Entry {
                startup: true,
                turn: true,
                act: false,
                recover: false,
                token: Some(0),
            },
            "object-shared-v10",
            10,
        )
    }

    const WORLD: &str = "\
enum token tokWord;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
class Thing: object name = 'thing' isOpen = nil presentation = [&name, &isOpen];
hall: Thing name = 'hall';
chest: Thing name = 'chest' isOpen = true;
";

    /// an entity's index is the one the running world binds it by, not
    /// its position in the published list — which skips dictionaries and the
    /// private bases `modify` makes, and so would be off by however many of
    /// those came before it.
    #[test]
    fn an_entitys_index_is_the_one_the_world_is_built_with() {
        let manifest = described(WORLD);
        let planned = zeb_frontend::object_init::declarations(
            &parser::parse_with(
                &Source::decode(WORLD.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
                parser::Model::Lifetimes,
            )
            .unwrap(),
        )
        .unwrap();
        for entity in &manifest.entities {
            let at = usize::try_from(entity.index).unwrap();
            assert_eq!(
                planned.objects[at].name, entity.name,
                "{} is published at index {} but the world builds {} there",
                entity.name, entity.index, planned.objects[at].name
            );
        }
    }

    /// a host reading a presentation answer gets property ids, so the
    /// manifest has to say what they are called.
    #[test]
    fn property_ids_are_published_by_name() {
        let manifest = described(WORLD);
        for wanted in ["name", "isOpen", "presentation"] {
            assert!(
                manifest.properties.contains_key(wanted),
                "the manifest does not name the property {wanted}"
            );
        }
        // Ids are distinct, or a host would decode two properties as one.
        let mut ids: Vec<u32> = manifest.properties.values().copied().collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "two properties share an id");
    }

    /// The new inspection kinds and the value tags are published, because a host
    /// cannot discover either by trying: an unanswerable inspection answers
    /// nothing rather than failing.
    #[test]
    fn the_manifest_says_what_can_be_inspected() {
        let json = described(WORLD).json();
        let Some(Json::Arr(kinds)) = json.get("inspect").cloned() else {
            panic!("no inspect list");
        };
        let numbers: Vec<i64> = kinds
            .iter()
            .filter_map(|kind| match kind.get("kind") {
                Some(Json::Num(value)) => Some(*value),
                _ => None,
            })
            .collect();
        for wanted in [0, 1, 2, 3, 4, 5] {
            assert!(
                numbers.contains(&wanted),
                "inspection kind {wanted} is not published"
            );
        }
        assert!(
            json.get("values").is_some(),
            "the value tags are not published"
        );
        assert!(
            json.get("properties").is_some(),
            "the property ids are not published"
        );
    }
}
