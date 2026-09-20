#![forbid(unsafe_code)]

use zeb_frontend::{
    flow,
    llvm::{self, Target},
    parser::{self, Ast},
    source::{Encoding, Source},
    specialize,
};
fn ast(s: &str) -> Ast {
    parser::parse_with(
        &Source::decode(s.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap()
}
fn plan(s: &str) -> Vec<specialize::Candidate> {
    let a = ast(s);
    let c = flow::check(&a).unwrap();
    specialize::plan(&a, &c.program, None).unwrap()
}
#[test]
fn integer_chains_and_recursion_have_unboxed_entries() {
    let a = ast(
        "id(x){return x;}f(x){return id(x);}fact(n){if(n==0)return 1;return n*fact(n-1);}main(){return f(fact(5));}",
    );
    let c = flow::check(&a).unwrap();
    assert!(
        specialize::plan(&a, &c.program, None)
            .unwrap()
            .iter()
            .all(|p| p.facts.is_some())
    );
    let text = llvm::emit_optimized(&a, Target::MacX86_64).unwrap();
    assert!(text.contains("define internal %iout @zsp0(i32 %arg0)"));
    assert!(text.contains("call %iout @zsp2(i32"));
    assert!(text.contains("define internal %out @zfn0(i64 %arg0)"));
    assert!(!llvm::emit(&a, Target::MacX86_64).unwrap().contains("@zsp"));
}
#[test]
fn logical_and_mixed_returns_fall_back_without_rejecting_source() {
    let p = plan(
        "logical(x){if(x)return 17;return 18;}mixed(x){if(x==0)return nil;return x;}wrap(x){return mixed(x);}",
    );
    assert!(p.iter().all(|p| p.facts.is_none()));
    let a = ast("f(x){return x+1;}main(){return f(true);}");
    let text = llvm::emit_optimized(&a, Target::MacX86_64).unwrap();
    let main = text
        .split("define internal %out @zfn1(")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    assert!(main.contains("call %out @zfn0(i64"));
}
#[test]
fn general_and_specialized_errors_use_their_own_outcome_types() {
    let a =
        ast("mixed(x){if(x==1)return nil;return 1/x;}f(x){return mixed(x)+1;}main(){return f(0);}");
    let text = llvm::emit_optimized(&a, Target::MacX86_64).unwrap();
    assert!(text.contains("ret %iout { i32 0, i32 1"));
    assert!(text.contains("insertvalue %out zeroinitializer, i32"));
    assert!(text.contains("insertvalue %iout zeroinitializer, i32"));
}
#[test]
fn budgets_keep_general_fallback_and_bound_clone_cost() {
    let mut source = String::new();
    for i in 0..100 {
        source += &format!("f{i}(x){{return x;}}");
    }
    let p = plan(&source);
    assert_eq!(
        p.iter().filter(|p| p.facts.is_some()).count(),
        specialize::MAX_CLONES
    );
    assert!(
        p.iter()
            .filter(|p| p.facts.is_some())
            .map(|p| p.cost)
            .sum::<usize>()
            <= specialize::MAX_TOTAL_COST
    );
    assert_eq!(p.last().unwrap().reason, "module budget");
    let body = "y+=1;".repeat(30);
    let mut many = String::new();
    for i in 0..50 {
        many += &format!("f{i}(x){{local y=x;{body}return y;}}");
    }
    let bounded = plan(&many);
    assert!(bounded.iter().filter(|p| p.facts.is_some()).count() < specialize::MAX_CLONES);
    assert!(
        bounded
            .iter()
            .filter(|p| p.facts.is_some())
            .map(|p| p.cost)
            .sum::<usize>()
            <= specialize::MAX_TOTAL_COST
    );
    assert!(bounded.iter().any(|p| p.reason == "module budget"));
    assert_eq!(
        plan(&format!(
            "large(x){{local y=x;{}return y;}}",
            "y+=1;".repeat(80)
        ))[0]
            .reason,
        "function budget"
    );
    let params = (0..17)
        .map(|i| format!("a{i}"))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        plan(&format!("f({params}){{return a0;}}"))[0].reason,
        "function budget"
    );
}
#[test]
fn pruning_a_callee_invalidates_dependent_integer_assumptions() {
    let p = plan("wrap(x){return mixed(x);}mixed(x){if(x==0)return nil;return 1;}");
    assert!(p.iter().all(|p| p.facts.is_none()));
}
#[test]
fn retained_inventory_and_malformed_ir_are_checked() {
    let a = ast("id(x){return x;}main(){return id(17);}");
    let mut c = flow::check(&a).unwrap();
    assert!(specialize::plan(&a, &c.program, Some(&[true])).is_err());
    let p = specialize::plan(&a, &c.program, Some(&[false, true])).unwrap();
    assert_eq!(p[0].reason, "not retained");
    assert!(p[0].facts.is_none());
    c.program.functions[0].blocks.clear();
    assert_eq!(
        specialize::plan(&a, &c.program, None).unwrap_err().code,
        "ir-invalid"
    );
}
