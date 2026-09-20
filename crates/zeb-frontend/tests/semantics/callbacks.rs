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
fn parameters_globals_and_callback_result_calls_lower_to_native_dispatch() {
    let tree = ast(
        "inc(x){return x+1;} factory(){return ({x:x*2});} apply(fn,x){return fn(x);} main(){return apply({x:inc(x)},4)+factory()(5);}",
    );
    flow::check(&tree).unwrap();
    let text = llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    assert!(text.contains("@zinvoke1(i32"));
    assert!(text.contains("label %invalid"));
    assert_eq!(
        llvm::emit(&tree, Target::MacX86_64).unwrap_err().code,
        "native-profile"
    );
}
#[test]
fn unsupported_captures_cannot_silently_bind_to_globals() {
    for source in [
        "main(){local x=3;return ({:x});}",
        "o:object x=3 m(){return ({:x});};",
        "o:object m(){return ({:self});};",
        "enum x; main(){local x=3;return ({:x});}",
        "main(){local x=3;return ({: ({:x})});}",
    ] {
        let error = flow::check(&ast(source)).unwrap_err();
        assert_eq!(error.code, "sem-capture-unavailable", "{source}: {error:?}");
    }
    flow::check(&ast("main(){local x=3;return ({x:x+1});}")).unwrap();
}

#[test]
fn long_callbacks_have_statement_bodies_and_local_scopes() {
    let tree = ast(include_str!("../../../../tests/native/long-callbacks.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    for source in [
        "main(){local x=3;return function(){return x;};}",
        "enum x;main(){local x=3;return function(){ {local x=4;} return x; };}",
    ] {
        assert_eq!(
            flow::check(&ast(source)).unwrap_err().code,
            "sem-capture-unavailable"
        );
    }
    flow::check(&ast(
        "main(){local x=3;return function(){local x=4;return x;};}",
    ))
    .unwrap();
    assert!(
        parser::parse_with(
            &Source::decode(
                b"main(){return function(){return 3;".to_vec(),
                Encoding::Utf8
            )
            .unwrap(),
            parser::Model::Ownership
        )
        .is_err()
    );
}

#[test]
fn explicit_owned_environments_lower_to_native_calls() {
    let tree = ast(include_str!("../../../../tests/native/captured-callback.t"));
    flow::check(&tree).unwrap();
    llvm::emit_objects(&tree, Target::MacX86_64).unwrap();
    let rejected = ast(
        "f(env){return 1;} inputs:object modes=[]; main(){local cb=captureClosure(f,inputs.modes,inputs.modes);return nil;}",
    );
    assert_eq!(flow::check(&rejected).unwrap_err().code, "sem-owner");
}
