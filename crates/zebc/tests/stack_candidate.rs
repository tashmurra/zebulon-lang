#![forbid(unsafe_code)]
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
#[ignore = "requires admitted LLVM and macOS sdk; candidate charges are not physical stack certificates"]
fn guarded_candidates_preserve_native_results_errors_and_order() {
    let temporary = common::TempDir::new("stack_candidate");
    let dir = temporary.0.clone();
    let tools = common::tools(&dir);
    let bin = tools.bin.as_str();
    let sdk = tools.sdk.as_str();
    // budget, O0 value/code/site-text, O2 value/code/site-text; charges 64/32.
    let cases = [
        (
            "exact",
            "f(){return 7;}main(){return f();}",
            64,
            7,
            0,
            "",
            7,
            0,
            "",
        ),
        (
            "special",
            "f(){return 7;}main(){return f();}",
            32,
            0,
            6,
            "f();}",
            7,
            0,
            "",
        ),
        (
            "below",
            "f(){return 7;}main(){return f();}",
            31,
            0,
            6,
            "f();}",
            0,
            6,
            "f();}",
        ),
        (
            "reuse",
            "f(){return 7;}main(){return f()+f();}",
            64,
            14,
            0,
            "",
            14,
            0,
            "",
        ),
        (
            "nested",
            "f(){return 7;}g(){return f();}main(){return g();}",
            127,
            0,
            6,
            "f();}",
            7,
            0,
            "",
        ),
        (
            "propagate",
            "f(){return 7;}g(){return f();}main(){return g();}",
            63,
            0,
            6,
            "g();}",
            0,
            6,
            "f();}",
        ),
        (
            "recursion",
            "f(n){if(n==0)return 7;return f(n-1);}main(){return f(1000);}",
            512,
            0,
            6,
            "f(n-1)",
            0,
            6,
            "f(n-1)",
        ),
        (
            "argument",
            "boom(x){return 1/x;}sink(x){return x;}main(){return sink(boom(0));}",
            64,
            0,
            3,
            "1/x",
            0,
            3,
            "1/x",
        ),
        (
            "general",
            "f(x){return x;}main(){return f(nil);}",
            63,
            0,
            6,
            "f(nil)",
            0,
            6,
            "f(nil)",
        ),
        (
            "unsigned-max",
            "f(){return 7;}main(){return f();}",
            u64::MAX,
            7,
            0,
            "",
            7,
            0,
            "",
        ),
    ];
    for (name, src, budget, v0, c0, s0, v2, c2, s2) in cases {
        let ast = parser::parse_with(
            &Source::decode(src.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership,
        )
        .unwrap();
        let mut charges = vec![
            StackCharge {
                general: 64,
                integer: 32
            };
            ast.functions.len()
        ];
        if name == "argument" {
            // The pending sink call would fail admission; boom must fail first.
            charges[1] = StackCharge {
                general: 128,
                integer: 128,
            };
        }
        let root = ast.functions.len() - 1;
        for optimize in [false, true] {
            let (value, code, site) = if optimize { (v2, c2, s2) } else { (v0, c0, s0) };
            let offset = if code == 0 {
                0
            } else {
                src.rfind(site).unwrap()
            };
            let word = if code == 0 {
                ((value as u64) << 32) | 2
            } else {
                0
            };
            let opt = if optimize { "O2" } else { "O0" };
            for (arch, target) in [("x86_64", Target::MacX86_64), ("arm64", Target::MacArm64)] {
                let mut text =
                    llvm::emit_stack_candidate(&ast, target, optimize, &charges).unwrap();
                text += &format!(
                    "\ndefine i32 @main() {{\n %r = call %out @zfn{root}(i64 {budget})\n %v = extractvalue %out %r, 0\n %c = extractvalue %out %r, 1\n %s = extractvalue %out %r, 2\n %a = icmp eq i64 %v, {word}\n %b = icmp eq i32 %c, {code}\n %d = icmp eq i64 %s, {offset}\n %e = and i1 %a, %b\n %ok = and i1 %e, %d\n %exit = select i1 %ok, i32 0, i32 1\n ret i32 %exit\n}}\n"
                );
                let stem = format!("{name}-{opt}-{arch}");
                let file = format!("{stem}.ll");
                fs::write(dir.join(&file), text).unwrap();
                run(
                    &dir,
                    &format!("{bin}/opt"),
                    &["-passes=verify", "-disable-output", &file],
                );
                let output = format!("{stem}.o");
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
                        &file,
                        "-o",
                        &output,
                    ],
                );
                if common::runs_on_host(arch) {
                    let exe = format!("{stem}.exe");
                    run(
                        &dir,
                        &format!("{bin}/clang"),
                        &[
                            "-target",
                            target.triple(),
                            target.clang_cpu(),
                            "-isysroot",
                            sdk,
                            &output,
                            "-o",
                            &exe,
                        ],
                    );
                    run(&dir, dir.join(&exe).to_str().unwrap(), &[]);
                }
            }
        }
    }
    println!("candidate artifacts: {}", dir.display());
}
