#![forbid(unsafe_code)]
#![cfg(target_os = "macos")]
// Experimental stack qualification remains Mach-O-specific.
mod common;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
use zeb_frontend::{
    llvm::{self, StackCharge, Target},
    parser,
    source::{Encoding, Source},
};
fn run(dir: &Path, tool: &str, args: &[&str], start: Instant) -> String {
    let remaining = Duration::from_secs(120)
        .checked_sub(start.elapsed())
        .expect("refinement budget expired");
    zebc::process::run(dir, tool, args, remaining.min(Duration::from_secs(30))).unwrap()
}
#[test]
#[ignore = "requires admitted native tools; local frame refinement is not host/closure certification"]
fn rebuilds_and_rechecks_actual_charged_objects() {
    let start = Instant::now();
    let temporary = common::TempDir::new("stack_refinement");
    let dir = temporary.0.clone();
    let tools = common::tools(&dir);
    let bin = tools.bin.as_str();
    let sdk = tools.sdk.as_str();
    let locals = (0..32)
        .map(|i| format!("local v{i}=x+{i};"))
        .collect::<String>();
    let sum = (0..32)
        .map(|i| format!("v{i}"))
        .collect::<Vec<_>>()
        .join("+");
    let source = format!(
        "leaf(x){{{locals}return {sum};}}recur(n){{if(n==0)return 1;return n+recur(n-1);}}main(seed,depth,unused){{return recur(depth)+leaf(seed);}}"
    );
    fs::write(dir.join("source.t"), &source).unwrap();
    let recursive_byte = source.find("recur(n-1)").unwrap() as u64;
    let ast = parser::parse_with(
        &Source::decode(source.into_bytes(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    let root_byte = ast.nodes[ast.functions[2].0].start as u64;

    let mut summaries = String::new();
    let mut changed_profiles = 0;
    for optimize in [false, true] {
        for (arch, target) in [("x86_64", Target::MacX86_64), ("arm64", Target::MacArm64)] {
            let opt = if optimize { "O2" } else { "O0" };
            let mut charges = vec![
                StackCharge {
                    general: 64,
                    integer: 32
                };
                3
            ];
            let mut accepted = false;
            for round in 0..4 {
                let stem = format!("{opt}-{arch}-{round}");
                let ll = format!("{stem}.ll");
                let obj = format!("{stem}.o");
                let su = format!("{stem}.su");
                fs::write(
                    dir.join(&ll),
                    llvm::emit_stack_entry_candidate(
                        &ast,
                        target,
                        optimize,
                        &charges,
                        2,
                        "zeb_stack_candidate_test",
                    )
                    .unwrap(),
                )
                .unwrap();
                let emitted = fs::read_to_string(dir.join(&ll)).unwrap();
                zebc::stack_charges::verify_emitted_charges(&charges, &emitted).unwrap();
                let stale: Vec<_> = charges
                    .iter()
                    .map(|c| StackCharge {
                        general: c.general + 16,
                        integer: c.integer,
                    })
                    .collect();
                assert!(zebc::stack_charges::verify_emitted_charges(&stale, &emitted).is_err());
                run(
                    &dir,
                    &format!("{bin}/opt"),
                    &["-passes=verify", "-disable-output", &ll],
                    start,
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
                        &obj,
                    ],
                    start,
                );
                let symbols = run(
                    &dir,
                    &format!("{bin}/llvm-nm"),
                    &["--defined-only", &obj],
                    start,
                );
                fs::write(dir.join(format!("{stem}-symbols.txt")), &symbols).unwrap();
                let r = zebc::stack_charges::refine(
                    &charges,
                    &fs::read_to_string(dir.join(&su)).unwrap(),
                    &symbols,
                    target,
                )
                .unwrap();
                summaries += &format!(
                    "{stem} changed={} input={:?} next={:?} wrapper={:?}\n",
                    r.changed,
                    charges
                        .iter()
                        .map(|c| (c.general, c.integer))
                        .collect::<Vec<_>>(),
                    r.charges
                        .iter()
                        .map(|c| (c.general, c.integer))
                        .collect::<Vec<_>>(),
                    r.wrappers
                );
                fs::write(dir.join("rounds.txt"), &summaries).unwrap();
                if r.changed {
                    if round == 0 {
                        changed_profiles += 1;
                    }
                    charges = r.charges;
                    continue;
                }
                let c = format!("{stem}.c");
                let exe = format!("{stem}.exe");
                let consumer = format!(
                    "#include <stdint.h>\nextern uint64_t zeb_stack_candidate_test(uint32_t,uint64_t,int32_t,int32_t,int32_t);\nint main(void){{\n if(zeb_stack_candidate_test(3,1048576,3,8,0)!=((UINT64_C(629)<<32)|2))return 1;\n if(zeb_stack_candidate_test(3,{},3,8,0)!=UINT64_C({}))return 2;\n if(zeb_stack_candidate_test(3,{},3,8,0)!=UINT64_C({}))return 3;\n return 0;\n}}\n",
                    charges[2].general - 1,
                    (root_byte << 32) | 8,
                    charges[2].general + charges[1].general,
                    (recursive_byte << 32) | 8
                );
                fs::write(dir.join(&c), consumer).unwrap();
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
                        &c,
                        &obj,
                        "-o",
                        &exe,
                    ],
                    start,
                );
                if common::runs_on_host(arch) {
                    run(&dir, dir.join(&exe).to_str().unwrap(), &[], start);
                }
                accepted = true;
                break;
            }
            fs::write(dir.join("rounds.txt"), &summaries).unwrap();
            assert!(accepted, "refinement did not converge within four rounds");
        }
    }
    assert!(
        changed_profiles >= 2,
        "fixture must require refinement on both O0 slices"
    );
    println!("refinement artifacts: {}", dir.display());
}
