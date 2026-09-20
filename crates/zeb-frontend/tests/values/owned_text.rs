#![forbid(unsafe_code)]
use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser,
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
fn text_snapshot_can_return_after_finalizer_completion() {
    let tree = ast(include_str!("../../../../tests/native/owned-text.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
}
#[test]
fn finalizer_replacement_and_borrowed_text_return_remain_rejected() {
    for source in [
        "owned f(){local owned b=new StringBuffer();local owned t=toString(b);try{return move t;}finally{}}",
        "owned f(){local owned b=new StringBuffer();local owned t=toString(b);try{}finally{return move t;}}",
        "f(){local owned b=new StringBuffer();local owned t=toString(b);return t;}",
        "main(){local owned b=new StringBuffer();local owned t=toString(b,10);return nil;}",
    ] {
        assert!(flow::check(&ast(source)).is_err(), "{source}");
    }
}

#[test]
fn transient_declaration_survives_object_planning() {
    let tree = ast("transient state:object value=7;");
    assert!(tree.nodes.iter().any(|node| matches!(
        node.syntax,
        parser::Syntax::Object {
            transient: true,
            ..
        }
    )));
    flow::check(&tree).unwrap();
    let plan = zeb_frontend::object_init::analyze(&tree).unwrap();
    assert!(plan.objects[0].transient);
    let initializer = zeb_frontend::object_init::emit(&tree).unwrap();
    assert!(initializer.contains("@zeb_objects_call(i32 90,"));
}
