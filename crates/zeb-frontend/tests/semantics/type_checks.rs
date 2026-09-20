#![forbid(unsafe_code)]

use zeb_frontend::{
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn emit(source: &str, opt: bool, runtime: bool) -> String {
    let ast = parser::parse_with(
        &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    match (opt, runtime) {
        (false, false) => llvm::emit(&ast, Target::MacX86_64),
        (true, false) => llvm::emit_optimized(&ast, Target::MacX86_64),
        (false, true) => llvm::emit_with_runtime(&ast, Target::MacX86_64, "_Rtest"),
        (true, true) => llvm::emit_with_runtime_rooted(&ast, Target::MacX86_64, "_Rtest", &[1]),
    }
    .unwrap()
}
fn body<'a>(module: &'a str, name: &str) -> &'a str {
    module
        .split(&format!("@{name}("))
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap()
}
#[test]
fn specialized_integers_skip_type_checks_but_keep_overflow_and_abi_checks() {
    let source = "f(x){return x+1;}main(){return f(17);}";
    let ir = emit(source, true, true);
    let b = body(&ir, "zsp0");
    assert!(b.contains("optimized integer-check"));
    assert!(!b.contains("call i32 %"));
    assert!(b.contains("i32 2, i64"));
    assert!(b.contains("i32 5, i64"));
    let general = body(&ir, "zfn0");
    assert_eq!(general.matches("call i32 %").count(), 1);
    assert!(general.contains("i32 1, i64"));
    let plain = emit(source, false, true);
    assert!(!plain.contains("optimized integer-check"));
}
#[test]
fn logical_facts_remove_only_proven_logical_checks() {
    let ir = emit(
        "f(x){return !x;}main(){local a=true;while(a){a=nil;}return !a;}",
        true,
        false,
    );
    assert!(body(&ir, "zfn1").contains("optimized logical-check"));
    assert!(!body(&ir, "zfn1").contains("i32 1, i64"));
    assert!(body(&ir, "zfn0").contains("i32 1, i64"));
}
#[test]
fn mixed_joins_and_general_call_results_keep_checks() {
    let ir = emit(
        "f(x){local y;if(x)y=17;else y=nil;return y+1;}main(){return f(true);}",
        true,
        true,
    );
    assert_eq!(body(&ir, "zfn0").matches("call i32 %").count(), 2);
    let ir = emit("id(x){return x;}main(){return id(true)+1;}", true, true);
    assert!(body(&ir, "zfn1").contains("i32 1, i64"));
}
#[test]
fn integer_proof_does_not_remove_division_or_shift_domain_checks() {
    for (source, code) in [
        ("f(x){return 1/x;}main(){return f(0);}", 3),
        ("f(x){return 1<<x;}main(){return f(32);}", 4),
    ] {
        let ir = emit(source, true, true);
        let b = body(&ir, "zsp0");
        assert!(!b.contains("call i32 %"));
        assert!(b.contains(&format!("i32 {code}, i64")));
    }
}
