#![forbid(unsafe_code)]
use zeb_frontend::{
    effects::{self, Effect},
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
fn definition(text: &str, function: usize) -> &str {
    text.lines()
        .find(|l| l.starts_with(&format!("define internal %out @zfn{function}(")))
        .unwrap()
}
#[test]
fn scalar_outcomes_and_frame_writes_allow_native_attributes() {
    let tree = ast("f(x){local y=x;return y+1;}main(){return f(17);}");
    for target in [Target::MacX86_64, Target::MacArm64] {
        let ir = llvm::emit_optimized(&tree, target).unwrap();
        assert!(definition(&ir, 0).contains("memory(none) nounwind willreturn"));
        assert!(ir.contains("optimized llvm-effects source-byte 0"));
        // Source overflow remains an explicit checked outcome despite nounwind.
        assert!(ir.contains("icmp sgt i64"));
        for ir in [
            llvm::emit(&tree, target).unwrap(),
            llvm::emit_with_runtime_rooted(&tree, target, "_Rtest", &[1]).unwrap(),
        ] {
            assert!(!definition(&ir, 0).contains("memory(none)"));
            assert!(!definition(&ir, 0).contains("willreturn"));
        }
    }
}
#[test]
fn recursion_and_loop_divergence_reach_callers_without_willreturn() {
    for source in [
        "f(x){return f(x);}main(){return f(1);}",
        "f(x){while(true){}return x;}main(){return f(1);}",
    ] {
        let tree = ast(source);
        let ir = llvm::emit_optimized(&tree, Target::MacX86_64).unwrap();
        for i in 0..2 {
            assert!(definition(&ir, i).contains("memory(none) nounwind"));
            assert!(!definition(&ir, i).contains("willreturn"));
        }
    }
}
#[test]
fn unknown_effects_are_not_pure_and_source_throw_is_distinct() {
    assert!(!effects::Effects::unknown().scalar_only());
    let tree = ast("f(x){return 1/x;}");
    let checked = flow::check(&tree).unwrap();
    let summaries = effects::analyze(&tree, &checked.program).unwrap();
    assert!(summaries[0].effects.contains(Effect::Throw));
    assert!(summaries[0].effects.scalar_only());
    assert!(!summaries[0].effects.contains(Effect::Diverge));
}
