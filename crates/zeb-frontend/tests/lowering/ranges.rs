#![forbid(unsafe_code)]

use zeb_frontend::{
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
fn emit(source: &str, optimized: bool) -> String {
    let ast = parser::parse_with(
        &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    if optimized {
        llvm::emit_with_runtime_rooted(&ast, Target::MacX86_64, "_Rtest", &[1])
    } else {
        llvm::emit_with_runtime(&ast, Target::MacX86_64, "_Rtest")
    }
    .unwrap()
}
fn general(text: &str) -> &str {
    text.split("define internal %out @zfn0(")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap()
}
#[test]
fn bounded_integer_chains_remove_overflow_but_not_type_or_abi_checks() {
    let source = "f(x){local y=x&255;return (y+1)*2;}main(){return f(-1);}";
    let optimized = emit(source, true);
    let body = general(&optimized);
    assert_eq!(body.matches("optimized overflow-check").count(), 2);
    assert!(!body.contains("i32 2, i64"));
    assert!(body.contains("i32 1, i64"));
    assert!(body.contains("i32 5, i64"));
    assert!(!body.contains(" nsw "));
    let plain = emit(source, false);
    assert!(general(&plain).contains("i32 2, i64"));
    assert!(!plain.contains("overflow-check"));
}
#[test]
fn negation_and_signed_multiplication_use_both_endpoints() {
    for source in [
        "f(x){return -(x&127);}main(){return f(-1);}",
        "f(x){return ~(x&127)*-100;}main(){return f(-1);}",
        "f(x){local y=x&32767;return y*y;}main(){return f(-1);}",
    ] {
        let ir = emit(source, true);
        assert!(general(&ir).contains("optimized overflow-check"));
        assert!(!general(&ir).contains("i32 2, i64"));
    }
    let ir = emit(
        "f(x){local y=x&65535;return y*y;}main(){return f(-1);}",
        true,
    );
    assert!(general(&ir).contains("missed overflow-check"));
    assert!(general(&ir).contains("i32 2, i64"));
}
#[test]
fn loops_joins_unknown_calls_and_negative_masks_cannot_supply_bounds() {
    for source in [
        "f(x){local y=0;while(y<x){++y;}return y+1;}main(){return f(17);}",
        "f(x){local y;if(x)y=0;else y=2147483647;return y+1;}main(){return f(nil);}",
        "f(x){return (x&-1)+1;}main(){return f(2147483647);}",
        "f(x){return g(x)+1;}main(){return f(17);}g(x){return x;}",
    ] {
        let ir = emit(source, true);
        assert!(general(&ir).contains("missed overflow-check"));
        assert!(general(&ir).contains("i32 2, i64"));
    }
}
#[test]
fn bounded_result_does_not_discharge_domain_or_failed_prior_operations() {
    let ir = emit(
        "f(x){local y=x&0;return (y-2147483647-1)*-1;}main(){return f(0);}",
        true,
    );
    assert_eq!(general(&ir).matches("optimized overflow-check").count(), 2);
    assert!(general(&ir).contains("i32 2, i64"));
    for (source, code) in [
        ("f(x){return 1/(x&0);}main(){return f(0);}", 3),
        ("f(x){return 1<<(x&63);}main(){return f(32);}", 4),
    ] {
        let ir = emit(source, true);
        assert!(general(&ir).contains(&format!("i32 {code}, i64")));
    }
}
