#![forbid(unsafe_code)]

use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser::{self, Ast},
    roots,
    source::{Encoding, Source},
};
fn ast(text: &str) -> Ast {
    parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
fn retained(text: &str, entries: &[usize]) -> Vec<bool> {
    let a = ast(text);
    let checked = flow::check(&a).unwrap();
    roots::retained(&a, &checked.program, entries).unwrap()
}
#[test]
fn keeps_transitive_calls_and_prunes_unrooted_recursive_island() {
    assert_eq!(
        retained(
            "leaf(){return 17;}unused(){return unused();}main(){return leaf();}",
            &[2]
        ),
        vec![true, false, true]
    );
}
#[test]
fn multiple_exports_preserve_mutual_recursion_and_original_ids() {
    let source = "a(){return b();}b(){return a();}unused(){return 5;}main(){return 17;}";
    assert_eq!(retained(source, &[3, 0, 3]), vec![true, true, false, true]);
    let text = llvm::emit_rooted(&ast(source), Target::MacX86_64, &[3, 0]).unwrap();
    assert!(text.contains("define internal %out @zfn3("));
    assert!(!text.contains("define internal %out @zfn2("));
    assert!(text.contains("call %iout @zsp1("));
    assert!(text.contains("call %iout @zsp0("));
}
#[test]
fn calls_after_return_are_conservatively_retained() {
    assert_eq!(
        retained("helper(){return 17;}main(){return 0;helper();}", &[1]),
        vec![true, true]
    );
}
#[test]
fn invalid_roots_and_malformed_ir_are_rejected() {
    let a = ast("main(){return 0;}");
    let mut checked = flow::check(&a).unwrap();
    for entry in [vec![], vec![1], vec![usize::MAX]] {
        assert_eq!(
            roots::retained(&a, &checked.program, &entry)
                .unwrap_err()
                .code,
            "native-roots"
        );
    }
    checked.program.functions[0].blocks.clear();
    assert_eq!(
        roots::retained(&a, &checked.program, &[0])
            .unwrap_err()
            .code,
        "ir-invalid"
    );
}
#[test]
fn removal_never_suppresses_source_errors_and_runtime_checks_stay() {
    let a = ast("unused(){return 1/0;}main(){return 0;}");
    assert!(llvm::emit_rooted(&a, Target::MacX86_64, &[1]).is_err());
    let a = ast("unused(){return 42;}id(x){return x;}main(){return id(17);}");
    let plain = llvm::emit(&a, Target::MacX86_64).unwrap();
    let rooted = llvm::emit_with_runtime_rooted(&a, Target::MacX86_64, "_Rtest", &[2]).unwrap();
    assert!(plain.contains("define internal %out @zfn0("));
    assert!(!rooted.contains("define internal %out @zfn0("));
    assert!(rooted.contains("label %callerr"));
    assert!(rooted.contains("optimized function-removal source-byte"));
    assert_eq!(
        llvm::emit_with_runtime_rooted(&a, Target::MacX86_64, "bad symbol", &[2])
            .unwrap_err()
            .code,
        "runtime-symbol"
    );
}
#[test]
fn deep_dependency_graph_does_not_recurse_in_the_compiler() {
    let mut source = String::new();
    for i in 0..2000 {
        source += &format!("f{i}(){{return f{}();}}", i + 1);
    }
    source += "f2000(){return 17;}";
    assert!(retained(&source, &[0]).into_iter().all(|keep| keep));
}
