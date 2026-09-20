#![forbid(unsafe_code)]
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
    let remaining = Duration::from_secs(180)
        .checked_sub(start.elapsed())
        .expect("runtime candidate budget expired");
    let result = zebc::process::run(dir, tool, args, remaining.min(Duration::from_secs(30)));
    use std::io::Write;
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("commands.txt"))
        .unwrap();
    writeln!(
        log,
        "cwd={}; tool={tool:?}; args={args:?}; result={result:?}",
        dir.display()
    )
    .unwrap();
    result.unwrap()
}

fn machine_closure(
    folder: &Path,
    dylib: &str,
    stem: &str,
    leaf: &str,
    target: Target,
    start: Instant,
) {
    let tools = common::tools(folder);
    let bin = tools.bin.as_str();
    let symbols = run(
        folder,
        &format!("{bin}/llvm-nm"),
        &["--defined-only", dylib],
        start,
    );
    let disassembly = run(
        folder,
        &format!("{bin}/llvm-objdump"),
        &["--disassemble", "--no-show-raw-insn", dylib],
        start,
    );
    fs::write(folder.join(format!("{stem}-machine-symbols.txt")), &symbols).unwrap();
    fs::write(
        folder.join(format!("{stem}-machine-disassembly.txt")),
        &disassembly,
    )
    .unwrap();
    let calls = zebc::stack_machine::validate(&disassembly, &symbols, leaf, target).unwrap();
    fs::write(
        folder.join(format!("{stem}-machine-calls.txt")),
        format!("{calls:#?}\n"),
    )
    .unwrap();
    assert!(!calls.is_empty(), "fixture must retain native calls");
}

#[test]
#[ignore = "admitted native tools; candidate runtime/LTO behavior, not a physical stack certificate"]
fn guarded_runtime_preserves_mutual_recursion_and_source_errors_through_lto() {
    let temporary = common::TempDir::new("stack_runtime");
    let dir = temporary.0.clone();
    let tools = common::tools(&dir);
    let bin = tools.bin.as_str();
    let sdk = tools.sdk.as_str();
    let start = Instant::now();
    fs::write(
        dir.join("runtime.rs"),
        include_str!("../../zeb-runtime/src/lib.rs"),
    )
    .unwrap();
    let source = "even(n){if(n==0)return 1;return odd(n-1);}odd(n){if(n==0)return 2;return even(n-1);}main(n,divisor){return even(n)/divisor;}";
    fs::write(dir.join("source.t"), source).unwrap();
    let ast = parser::parse_with(
        &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
        parser::Model::Ownership,
    )
    .unwrap();
    let charges = [StackCharge {
        general: 256,
        integer: 128,
    }; 3];
    let root_byte = source.find("main(").unwrap() as u64;
    // Division sites refer to the complete source expression, starting at even(n).
    let division_byte = source.find("even(n)/").unwrap() as u64;
    for (arch, target, rust_target) in [
        ("x86_64", Target::MacX86_64, "x86_64-apple-darwin"),
        ("arm64", Target::MacArm64, "aarch64-apple-darwin"),
    ] {
        let folder = dir.join(arch);
        fs::create_dir(&folder).unwrap();
        run(
            &folder,
            &tools.rustc,
            &[
                "--edition=2024",
                "--crate-name",
                "zeb_runtime",
                "--crate-type=lib",
                "--emit=llvm-bc",
                "--target",
                rust_target,
                "-C",
                "opt-level=2",
                "-C",
                "no-redzone=yes",
                "-C",
                &format!("target-cpu={}", target.cpu()),
                "../runtime.rs",
                "-o",
                "runtime.bc",
            ],
            start,
        );
        let symbols = run(
            &folder,
            &format!("{bin}/llvm-nm"),
            &["--defined-only", "runtime.bc"],
            start,
        );
        let names: Vec<_> = symbols
            .lines()
            .filter_map(|l| l.split_whitespace().last())
            .filter(|s| s.contains("SCALAR_API_V1"))
            .collect();
        assert_eq!(names.len(), 1);
        let runtime = names[0]
            .strip_prefix("__R")
            .map(|s| format!("_R{s}"))
            .unwrap_or_else(|| names[0].to_owned());
        let leaves: Vec<_> = symbols
            .lines()
            .filter_map(|l| l.split_whitespace().last())
            .filter(|n| n.ends_with("8classify"))
            .collect();
        assert_eq!(leaves.len(), 1);
        let runtime_leaf = leaves[0]
            .strip_prefix("__R")
            .map(|n| format!("_R{n}"))
            .unwrap_or_else(|| leaves[0].to_owned());
        let common = [
            "-target",
            target.triple(),
            target.clang_cpu(),
            "-isysroot",
            sdk,
            "-O2",
        ];
        let mut args = common.to_vec();
        args.extend(["-fstack-usage", "-c", "runtime.bc", "-o", "runtime.o"]);
        run(&folder, &format!("{bin}/clang"), &args, start);
        let runtime_definitions = run(
            &folder,
            &format!("{bin}/llvm-nm"),
            &["--defined-only", "runtime.o"],
            start,
        );
        let runtime_undefined = run(
            &folder,
            &format!("{bin}/llvm-nm"),
            &["--undefined-only", "runtime.o"],
            start,
        );
        let runtime_graph = run(
            &folder,
            &format!("{bin}/opt"),
            &["-passes=print-callgraph", "-disable-output", "runtime.bc"],
            start,
        );
        let runtime_usage = fs::read_to_string(folder.join("runtime.su")).unwrap();
        let floor = zebc::stack_charges::runtime_leaf_floor(
            &runtime_usage,
            &runtime_definitions,
            &runtime_undefined,
            &runtime_graph,
            &runtime_leaf,
            target,
        )
        .unwrap();
        fs::write(folder.join("runtime-symbols.txt"), &runtime_definitions).unwrap();
        fs::write(folder.join("runtime-undefined.txt"), &runtime_undefined).unwrap();
        fs::write(folder.join("runtime-callgraph.txt"), &runtime_graph).unwrap();
        fs::write(folder.join("runtime-floor.txt"), format!("{floor}\n")).unwrap();
        assert_eq!(floor, 16, "changed runtime floor requires review");
        // These charges fit the source-only local frames, but not their runtime
        // calls. Demonstrate that runtime accounting causes real re-emission,
        // rather than merely changing a table beside an already built object.
        let small = [StackCharge {
            general: 48,
            integer: 16,
        }; 3];
        let stable = zebc::stack_charges::refine_bounded(&small, |round, small| {
            let stem = format!("refine-{round}");
            let ll = format!("{stem}.ll");
            let obj = format!("{stem}.o");
            let emitted = llvm::emit_stack_entry_with_runtime_candidate(
                &ast,
                target,
                false,
                small,
                2,
                "zeb_stack_candidate_test",
                Some(&runtime),
            )
            .unwrap();
            zebc::stack_charges::verify_emitted_charges(small, &emitted).unwrap();
            assert!(zebc::stack_guards::verify_guarded_calls(small, &emitted).unwrap() > 0);
            fs::write(folder.join(&ll), &emitted).unwrap();
            run(
                &folder,
                &format!("{bin}/opt"),
                &["-passes=verify", "-disable-output", &ll],
                start,
            );
            let mut a = common.to_vec();
            a.extend(["-fstack-usage", "-c", &ll, "-o", &obj]);
            run(&folder, &format!("{bin}/clang"), &a, start);
            let symbols = run(
                &folder,
                &format!("{bin}/llvm-nm"),
                &["--defined-only", &obj],
                start,
            );
            let undefined = run(
                &folder,
                &format!("{bin}/llvm-nm"),
                &["--undefined-only", &obj],
                start,
            );
            let usage = fs::read_to_string(folder.join(format!("{stem}.su"))).unwrap();
            fs::write(folder.join(format!("{stem}-symbols.txt")), &symbols).unwrap();
            fs::write(folder.join(format!("{stem}-undefined.txt")), &undefined).unwrap();
            let refined = zebc::stack_charges::refine_with_runtime_floor(
                small, &usage, &symbols, target, floor, &undefined, &runtime,
            )
            .unwrap();
            fs::write(
                folder.join(format!("{stem}-refinement.txt")),
                format!(
                    "input={:?}; next={:?}; changed={}; wrappers={:?}\n",
                    small
                        .iter()
                        .map(|c| (c.general, c.integer))
                        .collect::<Vec<_>>(),
                    refined
                        .charges
                        .iter()
                        .map(|c| (c.general, c.integer))
                        .collect::<Vec<_>>(),
                    refined.changed,
                    refined.wrappers,
                ),
            )
            .unwrap();
            if round == 0 {
                assert!(
                    !zebc::stack_charges::refine(small, &usage, &symbols, target)
                        .unwrap()
                        .changed,
                    "fixture must isolate runtime reserve from local frame growth"
                );
                assert!(
                    refined.changed,
                    "fixture must require runtime-inclusive growth"
                );
            }
            if refined.changed {
                assert!(
                    zebc::stack_charges::verify_emitted_charges(&refined.charges, &emitted)
                        .is_err()
                );
            }
            Ok((refined, (stem, obj)))
        })
        .unwrap();
        assert_eq!(stable.round, 1, "fixture must stabilize after rebuilding");
        let small = &stable.refinement.charges;
        let (stem, obj) = stable.artifact;
        {
            let dylib = folder.join(format!("{stem}.dylib"));
            let dylib = dylib.to_str().unwrap();
            let mut a = common.to_vec();
            a.extend(["-dynamiclib", &obj, "runtime.o", "-o", dylib]);
            run(&folder, &format!("{bin}/clang"), &a, start);
            let undefined = run(
                &folder,
                &format!("{bin}/llvm-nm"),
                &["--undefined-only", dylib],
                start,
            );
            fs::write(
                folder.join(format!("{stem}-linked-undefined.txt")),
                &undefined,
            )
            .unwrap();
            assert!(undefined.trim().is_empty());
            let c = format!("{stem}.c");
            let exe = format!("{stem}.exe");
            fs::write(folder.join(&c), format!(
                "#include <stdint.h>\nextern uint64_t zeb_stack_candidate_test(uint32_t,uint64_t,int32_t,int32_t);\nint main(void){{\nif(zeb_stack_candidate_test(3,8192,8,1)!=((UINT64_C(1)<<32)|2))return 1;\nif(zeb_stack_candidate_test(3,8192,9,1)!=((UINT64_C(2)<<32)|2))return 2;\nif(zeb_stack_candidate_test(3,{},8,1)!=UINT64_C({}))return 3;\nif(zeb_stack_candidate_test(2,0,8,1)!=7)return 4;\nreturn 0;\n}}\n",
                small[2].general - 1, (root_byte << 32) | 8,
            )).unwrap();
            let mut a = common.to_vec();
            a.extend(["-Wall", "-Wextra", "-Werror", &c, dylib, "-o", &exe]);
            run(&folder, &format!("{bin}/clang"), &a, start);
            if common::runs_on_host(arch) {
                run(&folder, folder.join(&exe).to_str().unwrap(), &[], start);
            }
        }
        let compile_full = |ll: &str, stem: &str, charges: &[StackCharge]| {
            let obj = format!("{stem}.o");
            let runtime_lto = format!("{stem}-runtime.lto.o");
            let game_lto = format!("{stem}-game.lto.o");
            for (input, output) in [
                ("runtime.bc", runtime_lto.as_str()),
                (ll, game_lto.as_str()),
            ] {
                let mut a = common.to_vec();
                a.extend(["-flto=full", "-c", input, "-o", output]);
                run(&folder, &format!("{bin}/clang"), &a, start);
            }
            let bc = format!("{stem}.bc");
            let optimized_ll = format!("{stem}.ll");
            let linker = format!("-fuse-ld={bin}/ld64.lld");
            let mut a = common.to_vec();
            a.extend([
                "-flto=full",
                &linker,
                "-dynamiclib",
                &runtime_lto,
                &game_lto,
                "-Wl,--lto-emit-llvm",
                "-o",
                &bc,
            ]);
            run(&folder, &format!("{bin}/clang"), &a, start);
            run(
                &folder,
                &format!("{bin}/opt"),
                &["-passes=verify", "-disable-output", &bc],
                start,
            );
            run(
                &folder,
                &format!("{bin}/llvm-dis"),
                &[&bc, "-o", &optimized_ll],
                start,
            );
            let text = fs::read_to_string(folder.join(&optimized_ll)).unwrap();
            assert!(
                !text
                    .lines()
                    .any(|l| l.contains("call ") && (l.contains("classify") || !l.contains('@'))),
                "unresolved runtime/indirect call remains"
            );
            let mut a = common.to_vec();
            a.extend(["-fstack-usage", "-c", &bc, "-o", &obj]);
            run(&folder, &format!("{bin}/clang"), &a, start);
            let undefined = run(
                &folder,
                &format!("{bin}/llvm-nm"),
                &["--undefined-only", &obj],
                start,
            );
            assert!(
                undefined.trim().is_empty(),
                "unexpected combined-object dependency"
            );
            let definitions = run(
                &folder,
                &format!("{bin}/llvm-nm"),
                &["--defined-only", &obj],
                start,
            );
            let graph = run(
                &folder,
                &format!("{bin}/opt"),
                &["-passes=print-callgraph", "-disable-output", &bc],
                start,
            );
            fs::write(folder.join(format!("{stem}-callgraph.txt")), &graph).unwrap();
            fs::write(folder.join(format!("{stem}-symbols.txt")), &definitions).unwrap();
            let usage = fs::read_to_string(folder.join(format!("{stem}.su"))).unwrap();
            let refined = zebc::stack_charges::refine_full_lto(
                charges,
                &usage,
                &definitions,
                &undefined,
                &graph,
                &runtime_leaf,
                target,
            )
            .unwrap();
            assert_eq!(refined.detached_exports.len(), 1);
            fs::write(
                folder.join(format!("{stem}-refinement.txt")),
                format!(
                    "changed={}; charges={:?}; wrappers={:?}; detached exports={:?}\n",
                    refined.changed,
                    refined
                        .charges
                        .iter()
                        .map(|c| (c.general, c.integer))
                        .collect::<Vec<_>>(),
                    refined.wrappers,
                    refined.detached_exports
                ),
            )
            .unwrap();
            refined
        };
        // LTO can change frames and charge-dependent branches together. Start
        // below the measured floor and regenerate the combined module each time.
        let lto_source = "even(n){if(n==0)return 1;return odd(n-1)+n;}odd(n){if(n==0)return 2;return even(n-1)+n;}main(n,divisor){return even(n)/divisor;}";
        let lto_ast = parser::parse_with(
            &Source::decode(lto_source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            parser::Model::Ownership,
        )
        .unwrap();
        let lto_division_byte = lto_source.find("even(n)/").unwrap() as u64;
        let lto_root_byte = lto_source.find("main(").unwrap() as u64;
        fs::write(folder.join("lto-rebuild-source.t"), lto_source).unwrap();
        let low = [StackCharge {
            general: 16,
            integer: 16,
        }; 3];
        let lto_stable = zebc::stack_charges::refine_bounded(&low, |round, current| {
            let stem = format!("lto-refine-{round}");
            let ll = format!("{stem}-input.ll");
            let emitted = llvm::emit_stack_entry_with_runtime_candidate(
                &lto_ast,
                target,
                false,
                current,
                2,
                "zeb_stack_candidate_test",
                Some(&runtime),
            )
            .unwrap();
            zebc::stack_charges::verify_emitted_charges(current, &emitted).unwrap();
            assert!(zebc::stack_guards::verify_guarded_calls(current, &emitted).unwrap() > 0);
            fs::write(folder.join(&ll), &emitted).unwrap();
            let refined = compile_full(&ll, &stem, current);
            if round == 0 {
                assert!(refined.changed, "LTO fixture must require rebuilding");
            }
            if refined.changed {
                assert!(
                    zebc::stack_charges::verify_emitted_charges(&refined.charges, &emitted)
                        .is_err()
                );
            }
            Ok((refined, stem))
        })
        .unwrap();
        assert!(lto_stable.round > 0);
        let stem = &lto_stable.artifact;
        let obj = format!("{stem}.o");
        let dylib_path = folder.join(format!("{stem}.dylib"));
        let dylib = dylib_path.to_str().unwrap();
        let mut a = common.to_vec();
        a.extend(["-dynamiclib", &obj, "-o", dylib]);
        run(&folder, &format!("{bin}/clang"), &a, start);
        let undefined = run(
            &folder,
            &format!("{bin}/llvm-nm"),
            &["--undefined-only", dylib],
            start,
        );
        fs::write(
            folder.join(format!("{stem}-linked-undefined.txt")),
            &undefined,
        )
        .unwrap();
        assert!(undefined.trim().is_empty());
        machine_closure(&folder, dylib, stem, &runtime_leaf, target, start);
        let c = format!("{stem}.c");
        let exe = format!("{stem}.exe");
        fs::write(folder.join(&c), format!(
            "#include <stdint.h>\nextern uint64_t zeb_stack_candidate_test(uint32_t,uint64_t,int32_t,int32_t);\nint main(void){{\nif(zeb_stack_candidate_test(3,8192,8,1)!=((UINT64_C(37)<<32)|2))return 1;\nif(zeb_stack_candidate_test(3,8192,9,1)!=((UINT64_C(47)<<32)|2))return 2;\nif(zeb_stack_candidate_test(3,8192,8,0)!=UINT64_C({}))return 3;\nif(zeb_stack_candidate_test(3,0,8,1)!=UINT64_C({}))return 4;\nif((uint32_t)zeb_stack_candidate_test(3,8192,1000,1)!=8)return 5;\nreturn 0;\n}}\n", (lto_division_byte<<32)|5, (lto_root_byte<<32)|8
        )).unwrap();
        let mut a = common.to_vec();
        a.extend(["-Wall", "-Wextra", "-Werror", &c, dylib, "-o", &exe]);
        run(&folder, &format!("{bin}/clang"), &a, start);
        if common::runs_on_host(arch) {
            run(&folder, folder.join(&exe).to_str().unwrap(), &[], start);
        }
        for optimize in [false, true] {
            let profile = if optimize { "specialized" } else { "general" };
            let ll = format!("{profile}.ll");
            let emitted = llvm::emit_stack_entry_with_runtime_candidate(
                &ast,
                target,
                optimize,
                &charges,
                2,
                "zeb_stack_candidate_test",
                Some(&runtime),
            )
            .unwrap();
            assert!(
                emitted.contains("call i32 %"),
                "fixture must exercise actual runtime calls before LTO"
            );
            zebc::stack_charges::verify_emitted_charges(&charges, &emitted).unwrap();
            assert!(zebc::stack_guards::verify_guarded_calls(&charges, &emitted).unwrap() > 0);
            let stale = [StackCharge {
                general: 512,
                integer: 256,
            }; 3];
            assert!(zebc::stack_charges::verify_emitted_charges(&stale, &emitted).is_err());
            fs::write(folder.join(&ll), emitted).unwrap();
            run(
                &folder,
                &format!("{bin}/opt"),
                &["-passes=verify", "-disable-output", &ll],
                start,
            );
            // The C root always enters general main. Its unknown parameter first
            // calls general even (256). General recursion then reserves 256 each;
            // optimized recursion switches to integer entries (128 each).
            // At 4096, 15 or 29 recursive activations fit, respectively. Both
            // last admitted activations are even, rejecting their call to odd.
            let failure_call = "odd(n-1)";
            let recursive_byte = source.find(failure_call).unwrap() as u64;
            let c = format!("{profile}.c");
            fs::write(folder.join(&c),format!("#include <stdint.h>\nextern uint64_t zeb_stack_candidate_test(uint32_t,uint64_t,int32_t,int32_t);\nint main(void){{\n if(zeb_stack_candidate_test(3,8192,8,1)!=((UINT64_C(1)<<32)|2))return 1;\n if(zeb_stack_candidate_test(3,8192,9,1)!=((UINT64_C(2)<<32)|2))return 2;\n if(zeb_stack_candidate_test(3,8192,8,0)!=UINT64_C({}))return 3;\n if(zeb_stack_candidate_test(3,4096,100,1)!=UINT64_C({}))return 4;\n if(zeb_stack_candidate_test(3,255,8,1)!=UINT64_C({}))return 5;\n if(zeb_stack_candidate_test(2,0,8,1)!=7)return 6;\n return 0;\n}}\n",(division_byte<<32)|5,(recursive_byte<<32)|8,(root_byte<<32)|8)).unwrap();
            for full in [false, true] {
                let stem = format!("{profile}-{}", if full { "full" } else { "none" });
                let obj = format!("{stem}.o");
                let exe = format!("{stem}.exe");
                if full {
                    let refined = compile_full(&ll, &stem, &charges);
                    assert!(!refined.changed, "candidate charges require rebuilding");
                } else {
                    let mut a = common.to_vec();
                    a.extend(["-fstack-usage", "-c", &ll, "-o", &obj]);
                    run(&folder, &format!("{bin}/clang"), &a, start);
                    let definitions = run(
                        &folder,
                        &format!("{bin}/llvm-nm"),
                        &["--defined-only", &obj],
                        start,
                    );
                    let usage = fs::read_to_string(folder.join(format!("{stem}.su"))).unwrap();
                    let undefined = run(
                        &folder,
                        &format!("{bin}/llvm-nm"),
                        &["--undefined-only", &obj],
                        start,
                    );
                    fs::write(folder.join(format!("{stem}-undefined.txt")), &undefined).unwrap();
                    let refined = zebc::stack_charges::refine_with_runtime_floor(
                        &charges,
                        &usage,
                        &definitions,
                        target,
                        floor,
                        &undefined,
                        &runtime,
                    )
                    .unwrap();
                    // Never link an object whose measured requirements would
                    // change its supplied source charges. A changed table needs
                    // fresh emission/code generation and a new frame report.
                    assert!(
                        !refined.changed,
                        "runtime-inclusive charges require rebuilding"
                    );
                    assert_eq!(refined.wrappers.len(), 1);
                    assert!(refined.detached_exports.is_empty());
                    fs::write(folder.join(format!("{stem}-symbols.txt")), &definitions).unwrap();
                    fs::write(folder.join(format!("{stem}-refinement.txt")), format!(
                        "source charges stable; runtime floor={floor}; wrappers={:?}; host admission pending\n",
                        refined.wrappers,
                    )).unwrap();
                }
                // Link the exact measured object(s) into a real dylib, then use
                // that dylib from the independently compiled consumer.
                let dylib = folder.join(format!("{stem}.dylib"));
                let dylib = dylib.to_str().unwrap();
                let mut a = common.to_vec();
                a.extend(["-dynamiclib", &obj]);
                if !full {
                    a.push("runtime.o");
                }
                a.extend(["-o", dylib]);
                run(&folder, &format!("{bin}/clang"), &a, start);
                let unresolved = run(
                    &folder,
                    &format!("{bin}/llvm-nm"),
                    &["--undefined-only", dylib],
                    start,
                );
                fs::write(
                    folder.join(format!("{stem}-linked-undefined.txt")),
                    &unresolved,
                )
                .unwrap();
                assert!(unresolved.trim().is_empty(), "uncovered linked dependency");
                if full {
                    machine_closure(&folder, dylib, &stem, &runtime_leaf, target, start);
                }
                let mut a = common.to_vec();
                a.extend(["-Wall", "-Wextra", "-Werror", &c, dylib, "-o", &exe]);
                run(&folder, &format!("{bin}/clang"), &a, start);
                if common::runs_on_host(arch) {
                    run(&folder, folder.join(&exe).to_str().unwrap(), &[], start);
                }
            }
        }
    }
    println!(
        "guarded runtime artifacts: {}; elapsed {:.3}s",
        dir.display(),
        start.elapsed().as_secs_f64()
    );
}
