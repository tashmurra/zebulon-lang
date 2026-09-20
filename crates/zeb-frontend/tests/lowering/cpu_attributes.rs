#![forbid(unsafe_code)]
use zeb_frontend::{
    llvm::{self, Target},
    parser,
    source::{Encoding, Source},
};
#[test]
fn every_source_entry_carries_the_selected_baseline_cpu() {
    let ast = parser::parse_with(
        &Source::decode(
            b"f(x){return x+x;}main(){return f(17);}".to_vec(),
            Encoding::Utf8,
        )
        .unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    for (target, cpu) in [(Target::MacX86_64, "x86-64"), (Target::MacArm64, "generic")] {
        for text in [
            llvm::emit(&ast, target).unwrap(),
            llvm::emit_optimized(&ast, target).unwrap(),
            llvm::emit_with_runtime_rooted(&ast, target, "_Rtest", &[1]).unwrap(),
        ] {
            let definitions: Vec<_> = text
                .lines()
                .filter(|line| line.starts_with("define "))
                .collect();
            assert!(definitions.len() >= 2);
            for line in definitions {
                assert!(
                    line.contains(&format!("\"target-cpu\"=\"{cpu}\"")),
                    "{line}"
                );
            }
            assert!(!text.contains("penryn"));
            assert!(!text.contains("\"target-cpu\"=\"native\""));
        }
    }
}
