#![forbid(unsafe_code)]

use zeb_frontend::{
    effects::{self, Effect, Effects},
    flow,
    llvm::{self, Target},
    parser::{self, Ast},
    source::{Encoding, Source},
};
fn ast(text: &str) -> Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
fn summary(text: &str) -> Vec<effects::Summary> {
    let ast = ast(text);
    effects::analyze(&ast, &flow::check(&ast).unwrap().program).unwrap()
}
#[test]
fn frame_access_is_not_external_state() {
    let s = summary("f(x){local y=x;return y;}main(){return f(7);}");
    assert!(s[0].reads_frame && s[0].writes_frame);
    assert!(!s[1].reads_frame && !s[1].writes_frame);
    for f in s {
        assert_eq!(f.effects, Effects::default());
    }
}
#[test]
fn source_errors_propagate_through_recursive_call_graph() {
    let s = summary(
        "a(x){return b(x);}b(x){if(x==0)return 1/x;return a(x);}main(){return a(0);}unrelated(){return 3;}",
    );
    for f in &s[..3] {
        assert!(f.effects.contains(Effect::Throw));
        assert!(f.effects.contains(Effect::Diverge));
    }
    assert_eq!(s[3].effects, Effects::default());
}
#[test]
fn recursion_without_source_error_is_not_termination() {
    let s = summary("a(){return b();}b(){return a();}main(){return a();}");
    for f in s {
        assert!(f.effects.contains(Effect::Diverge));
        assert!(!f.effects.contains(Effect::Throw));
    }
}
#[test]
fn cfg_cycles_propagate_but_disconnected_code_does_not() {
    let s = summary(
        "spin(){while(true){}return nil;}caller(){return spin();}dead(x){return 0;local y=1/x;}",
    );
    assert!(s[0].effects.contains(Effect::Diverge));
    assert!(s[1].effects.contains(Effect::Diverge));
    assert_eq!(s[2].effects, Effects::default());
}
#[test]
fn unknown_behavior_includes_every_observable_effect() {
    for effect in [
        Effect::ReadState,
        Effect::WriteState,
        Effect::Allocate,
        Effect::Transfer,
        Effect::Destroy,
        Effect::Throw,
        Effect::Callback,
        Effect::Io,
        Effect::Suspend,
        Effect::Diverge,
    ] {
        assert!(Effects::unknown().contains(effect));
    }
}
#[test]
fn error_check_elision_requires_proof_and_retains_calls() {
    let a = ast("id(x){return x;}bad(x){return 1/x;}main(){local x=id(0);return bad(x);}");
    let plain = llvm::emit(&a, Target::MacX86_64).unwrap();
    let optimized = llvm::emit_optimized(&a, Target::MacX86_64).unwrap();
    assert_eq!(plain.matches("label %callerr").count(), 2);
    let main = optimized
        .split("define internal %out @zfn2(")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    assert_eq!(main.matches("label %callerr").count(), 1);
    assert!(main.contains("call %iout @zsp0"));
    assert!(optimized.contains("optimized call-error-check source-byte"));
    assert!(optimized.contains("missed call-error-check source-byte"));
    let runtime = llvm::emit_with_runtime(&a, Target::MacX86_64, "_Rtest").unwrap();
    assert_eq!(runtime.matches("label %callerr").count(), 2);
}
#[test]
fn deep_call_graph_uses_worklists_and_rejects_invalid_ir() {
    let mut source = String::new();
    for i in 0..2000 {
        source += &format!("f{i}(x){{return f{}(x);}}", i + 1);
    }
    source += "f2000(x){return 1/x;}";
    let a = ast(&source);
    let mut checked = flow::check(&a).unwrap();
    assert!(
        effects::analyze(&a, &checked.program)
            .unwrap()
            .iter()
            .all(|s| s.effects.contains(Effect::Throw))
    );
    checked.program.functions[0].blocks.clear();
    assert_eq!(
        effects::analyze(&a, &checked.program).unwrap_err().code,
        "ir-invalid"
    );
}
