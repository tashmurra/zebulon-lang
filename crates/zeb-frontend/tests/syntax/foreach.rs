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
fn iteration_bindings_and_control_flow_compile() {
    for source in [
        include_str!("../../../../tests/native/foreach.t"),
        include_str!("../../../../tests/native/foreach-events.t"),
        include_str!("../../../../tests/native/foreach-existing.t"),
    ] {
        let tree = ast(source);
        flow::check(&tree).unwrap();
        llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    }
}
#[test]
fn binding_scope_and_scalar_profile_remain_checked() {
    assert_eq!(
        flow::check(&ast("f(xs){foreach(local x in xs){} return x;}"))
            .unwrap_err()
            .code,
        "sem-name"
    );
    assert_eq!(
        flow::check(&ast("f(xs){local x=1;foreach(local x in xs){} return x;}"))
            .unwrap_err()
            .code,
        "sem-shadowing"
    );
    assert_eq!(
        llvm::emit(
            &ast("f(xs){foreach(local x in xs){return x;}return nil;}"),
            Target::MacX86_64
        )
        .unwrap_err()
        .code,
        "native-profile"
    );
}

#[test]
fn existing_binding_requires_a_writable_destination() {
    assert_eq!(
        flow::check(&ast("f(xs){foreach(missing in xs){} return nil;}"))
            .unwrap_err()
            .code,
        "sem-destination"
    );
    assert!(
        flow::check(&ast(
            "f(xs){local owned value=new object(); foreach(value in xs){} return nil;}"
        ))
        .is_err()
    );
}
