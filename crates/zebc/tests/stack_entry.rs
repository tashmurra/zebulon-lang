#![forbid(unsafe_code)]
#![cfg(target_os = "macos")]
// Experimental stack qualification remains Mach-O-specific.
mod common;
use std::{fs, path::Path, time::Duration};
use zeb_frontend::{
    llvm::{self, StackCharge, Target},
    parser,
    source::{Encoding, Source},
};
fn run(dir: &Path, tool: &str, args: &[&str]) {
    zebc::process::run(dir, tool, args, Duration::from_secs(30)).unwrap();
}
#[test]
#[ignore = "candidate ABI native test requires admitted LLVM/macOS; no physical stack certificate"]
fn guarded_c_entry_checks_root_versions_arguments_and_error_words() {
    let temporary = common::TempDir::new("stack_entry");
    let dir = temporary.0.clone();
    let tools = common::tools(&dir);
    let bin = tools.bin.as_str();
    let sdk = tools.sdk.as_str();
    let cases = [
        (
            "zero",
            "main(){return 42;}",
            0,
            "",
            42u64 << 32 | 2,
            "",
            0u64,
        ),
        (
            "one",
            "f(x){return x+1;}main(x){return f(x);}",
            1,
            ", 41",
            42u64 << 32 | 2,
            ", 2147483647",
            4u64,
        ),
        (
            "two",
            "main(a,b){return a/b;}",
            2,
            ", -7, 3",
            ((-2i32 as u32 as u64) << 32) | 2,
            ", 1, 0",
            5u64,
        ),
        (
            "three",
            "f(n){if(n<=0)return 7;return f(n-1);}main(seed,n,unused){return f(n)+seed;}",
            3,
            ", 5, 2, 0",
            12u64 << 32 | 2,
            ", 5, 1000, 0",
            8u64,
        ),
    ];
    for (name, src, arity, inputs, want, bad_inputs, error_kind) in cases {
        let ast = parser::parse_with(
            &Source::decode(src.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership,
        )
        .unwrap();
        let root = ast.functions.len() - 1;
        let root_byte = ast.nodes[ast.functions[root].0].start;
        let charges = vec![
            StackCharge {
                general: 64,
                integer: 32
            };
            ast.functions.len()
        ];
        let error_site = match name {
            "one" => src.find("x+1").unwrap(),
            "two" => src.find("a/b").unwrap(),
            "three" => src.find("f(n-1)").unwrap(),
            _ => 0,
        };
        let params = ", int32_t".repeat(arity);
        let bad_check = if error_kind != 0 {
            format!(
                "if(zeb_stack_candidate_test(3,{}, {}) != UINT64_C({})) return 4;",
                if name == "three" { 128 } else { 512 },
                bad_inputs.trim_start_matches(", "),
                ((error_site as u64) << 32) | error_kind
            )
        } else {
            String::new()
        };
        let c = format!(
            "#include <stdint.h>\nextern uint64_t zeb_stack_candidate_test(uint32_t,uint64_t{params});\nint main(void){{\n const uint32_t versions[]={{0,1,2,4,UINT32_MAX}};\n for(unsigned i=0;i<5;++i)if(zeb_stack_candidate_test(versions[i],0{inputs})!=7)return 1;\n if(zeb_stack_candidate_test(3,63{inputs})!=UINT64_C({}))return 2;\n if(zeb_stack_candidate_test(3,512{inputs})!=UINT64_C({want}))return 3;\n {bad_check}\n return 0;\n}}\n",
            ((root_byte as u64) << 32) | 8
        );
        for optimize in [false, true] {
            let opt = if optimize { "O2" } else { "O0" };
            for (arch, target) in [("x86_64", Target::MacX86_64), ("arm64", Target::MacArm64)] {
                let stem = format!("{name}-{opt}-{arch}");
                let ll = format!("{stem}.ll");
                let object = format!("{stem}.o");
                let file = format!("{stem}.c");
                let exe = format!("{stem}.exe");
                fs::write(
                    dir.join(&ll),
                    llvm::emit_stack_entry_candidate(
                        &ast,
                        target,
                        optimize,
                        &charges,
                        root,
                        "zeb_stack_candidate_test",
                    )
                    .unwrap(),
                )
                .unwrap();
                fs::write(dir.join(&file), &c).unwrap();
                run(
                    &dir,
                    &format!("{bin}/opt"),
                    &["-passes=verify", "-disable-output", &ll],
                );
                run(
                    &dir,
                    &format!("{bin}/clang"),
                    &[
                        "-target",
                        target.triple(),
                        target.clang_cpu(),
                        "-isysroot",
                        sdk,
                        &format!("-{opt}"),
                        "-fstack-usage",
                        "-c",
                        &ll,
                        "-o",
                        &object,
                    ],
                );
                run(
                    &dir,
                    &format!("{bin}/clang"),
                    &[
                        "-target",
                        target.triple(),
                        target.clang_cpu(),
                        "-isysroot",
                        sdk,
                        "-O2",
                        "-Wall",
                        "-Wextra",
                        "-Werror",
                        &file,
                        &object,
                        "-o",
                        &exe,
                    ],
                );
                if common::runs_on_host(arch) {
                    run(&dir, dir.join(exe).to_str().unwrap(), &[]);
                }
            }
        }
    }
    println!("entry artifacts: {}", dir.display());
}
