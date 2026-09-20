#![forbid(unsafe_code)]
use zeb_frontend::{
    object_init::{self, InitialValue},
    parser::{self, Syntax},
    sema::{self, Scalar},
    source::{Encoding, Source},
};
fn ast(text: &str) -> parser::Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
#[test]
fn named_graph_preserves_order_forward_refs_and_shared_property_ids() {
    let tree = ast(include_str!(
        "../../../../tests/native/object-initialization.t"
    ));
    let plan = object_init::analyze(&tree).unwrap();
    assert_eq!(
        plan.objects
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["stack", "door"]
    );
    assert_eq!(
        plan.properties,
        ["count", "ready", "peer", "locked", "isClass"]
    );
    assert_eq!(
        plan.objects[0].properties[0].value,
        InitialValue::Scalar(Scalar::Integer(42))
    );
    assert_eq!(plan.objects[0].properties[2].value, InitialValue::Object(1));
    assert_eq!(plan.objects[1].properties[1].value, InitialValue::Object(0));
    assert_eq!(plan.objects[1].properties[1].id, 2);
    assert!(
        object_init::emit(&tree)
            .unwrap()
            .contains("define i32 @zeb_initialize_objects")
    );
    assert!(sema::analyze(&tree).is_ok());
}
#[test]
fn invalid_or_unimplemented_initializers_do_not_disappear() {
    for (source, code) in [
        ("a: object x=missing;", "sem-name"),
        ("a: object x=1 x=2;", "sem-property"),
        ("a: object; a: object;", "sem-name"),
        ("a: Parent;", "sem-name"),
        (
            "a: object x=other.x; other: object x=1;",
            "object-init-unavailable",
        ),
        ("a: object; main(){return 1;}", "object-init-unavailable"),
    ] {
        assert_eq!(
            object_init::analyze(&ast(source)).unwrap_err().code,
            code,
            "{source}"
        );
    }
}
#[test]
fn property_postfix_syntax_preserves_receiver_and_chain() {
    let tree = ast("main(a){return a.peer.count;}");
    let properties = tree
        .nodes
        .iter()
        .filter_map(|n| {
            if let Syntax::Property(_, name) = &n.syntax {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(properties, ["peer", "count"]);
    assert!(sema::analyze(&tree).is_ok());
}

#[test]
fn inheritance_resolves_forward_names_and_rejects_cycles() {
    let plan = object_init::analyze(&ast("leaf: middle; middle: base; base: object;")).unwrap();
    assert_eq!(
        plan.objects
            .iter()
            .map(|o| o.prototypes.clone())
            .collect::<Vec<_>>(),
        [vec![1], vec![2], vec![]]
    );
    for text in ["a: a;", "a: b; b: a;", "a: b; b: c; c: b;"] {
        assert_eq!(
            object_init::analyze(&ast(text)).unwrap_err().code,
            "sem-inheritance"
        );
    }
    let plan = object_init::analyze(&ast("a: object; b: object; c: a,b;")).unwrap();
    assert_eq!(plan.objects[2].prototypes, [0, 1]);
    assert_eq!(
        object_init::analyze(&ast("a: object; b: object,a;"))
            .unwrap_err()
            .code,
        "sem-inheritance"
    );
}

#[test]
fn containment_uses_references_and_skips_classes() {
    let tree = ast(
        "+ property location; class Item: object; room: object; + Item; ++ note: Item; class Other: object; + chair: Item; elsewhere: object; + moved: Item location=room;",
    );
    let plan = object_init::analyze(&tree).unwrap();
    let location = plan
        .properties
        .iter()
        .position(|p| p == "location")
        .unwrap() as u32;
    let container = |index: usize| {
        &plan.objects[index]
            .properties
            .iter()
            .find(|p| p.id == location)
            .unwrap()
            .value
    };
    assert_eq!(container(2), &InitialValue::Object(1));
    assert_eq!(container(3), &InitialValue::Object(2));
    assert_eq!(container(5), &InitialValue::Object(1));
    assert_eq!(container(7), &InitialValue::Object(1));
    assert!(plan.objects[2].name.starts_with("$anonymous"));
    for source in [
        "+ a: object;",
        "+ property location; + a: object;",
        "+ property location; a: object; ++ b: object;",
        "+ main(){return nil;}",
        "+ enum red;",
        "a: object { p=1;",
    ] {
        assert!(
            parser::parse_with(
                &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
                parser::Model::Ownership
            )
            .is_err(),
            "{source}"
        );
    }
}
