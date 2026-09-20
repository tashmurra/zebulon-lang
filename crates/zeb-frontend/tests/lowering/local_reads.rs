#![forbid(unsafe_code)]
use zeb_frontend::{
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn emit(text: &str, opt: bool) -> String {
    let ast = parser::parse_with(
        &Source::decode(text.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    if opt {
        llvm::emit_optimized(&ast, Target::MacX86_64)
    } else {
        llvm::emit(&ast, Target::MacX86_64)
    }
    .unwrap()
}
fn first(ir: &str) -> &str {
    ir.split("define internal %out @zfn0(")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap()
}
#[test]
fn repeated_parameter_read_keeps_one_load() {
    let text = "f(x){return x+x;}";
    let plain = emit(text, false);
    let opt = emit(text, true);
    assert_eq!(first(&plain).matches("load i64, ptr %s").count(), 2);
    assert_eq!(first(&opt).matches("load i64, ptr %s").count(), 1);
    assert_eq!(first(&opt).matches("optimized local-read").count(), 1);
    assert!(!plain.contains("local-read"));
}
#[test]
fn stores_supply_values_without_removing_stores_or_checks() {
    let text = "f(x){local y=x;y=17;return y+y;}";
    let plain = emit(text, false);
    let opt = emit(text, true);
    assert_eq!(first(&opt).matches("load i64, ptr %s").count(), 1);
    assert_eq!(
        first(&opt).matches("store i64").count(),
        first(&plain).matches("store i64").count()
    );
    assert!(first(&opt).contains("optimized local-read"));
}
#[test]
fn call_boundary_retains_a_fresh_load() {
    let opt = emit("f(x){local y=x;g();return y;}g(){return 0;}", true);
    assert_eq!(first(&opt).matches("load i64, ptr %s").count(), 2);
    assert!(first(&opt).contains("call %iout @zsp1"));
}
#[test]
fn branch_join_and_loop_do_not_reuse_predecessor_values() {
    for text in [
        "f(x){local y=17;if(x)y=18;return y;}",
        "f(x){local y=0;while(y<x){++y;}return y;}",
    ] {
        let opt = emit(text, true);
        assert!(first(&opt).matches("missed local-read").count() >= 2);
        assert!(first(&opt).matches("load i64, ptr %s").count() >= 2);
    }
}
