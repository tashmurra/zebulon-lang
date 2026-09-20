#![forbid(unsafe_code)]
mod common;
use std::{fs, path::Path, time::Duration};
use zeb_frontend::{
    llvm::{self, Target},
    parser::{self, Model},
    source::{Encoding, Source},
};
fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    zebc::process::run(dir, program, args, Duration::from_secs(60))
        .unwrap_or_else(|e| panic!("{program} {args:?}: {e}"))
}
#[test]
fn invalid_runtime_symbols_cannot_enter_llvm() {
    let ast = parser::parse_with(
        &Source::decode(b"f(x){return x+1;}".to_vec(), Encoding::Utf8).unwrap(),
        Model::Ownership,
    )
    .unwrap();
    for symbol in ["bad", "_Rbad\n}", "_R\"quoted"] {
        assert_eq!(
            llvm::emit_with_runtime(&ast, Target::MacX86_64, symbol)
                .unwrap_err()
                .code,
            "runtime-symbol"
        );
    }
    let text = llvm::emit_with_runtime(&ast, Target::MacX86_64, "_Rtest").unwrap();
    assert!(text.contains("external constant %scalar_api"));
    assert!(text.contains("i32 5, i64 0"));
}
#[test]
#[ignore = "requires installed Rust 1.98.1, LLVM 22.1.8 and the macOS SDK"]
fn safe_runtime_dylib_links_from_generated_llvm_and_c() {
    let root = common::root();
    let temporary = common::TempDir::new("runtime-link");
    let dir = temporary.0.clone();
    let tools = common::tools(&dir);
    let bin = tools.bin.as_str();
    let sdk = tools.sdk.as_str();
    let rustc = &tools.rustc;
    let source = root.join("crates/zeb-runtime/src/lib.rs");
    let ast = parser::parse_with(
        &Source::decode(
            b"f(x){return x+1;} g(x){return !x;}".to_vec(),
            Encoding::Utf8,
        )
        .unwrap(),
        Model::Ownership,
    )
    .unwrap();
    for (arch, target, rust_target) in [
        ("x86_64", Target::MacX86_64, "x86_64-apple-darwin"),
        ("arm64", Target::MacArm64, "aarch64-apple-darwin"),
    ] {
        let folder = dir.join(arch);
        fs::create_dir(&folder).unwrap();
        run(
            &folder,
            "/usr/bin/env",
            &[
                "MACOSX_DEPLOYMENT_TARGET=14.0",
                &format!("SDKROOT={sdk}"),
                rustc,
                "--edition=2024",
                "--crate-name",
                "zeb_runtime",
                "--crate-type=dylib",
                "--target",
                rust_target,
                "-C",
                &format!("target-cpu={}", target.cpu()),
                "-C",
                "opt-level=2",
                "-C",
                "link-arg=-Wl,-install_name,@rpath/libzeb_runtime.dylib",
                source.to_str().unwrap(),
                "-o",
                "libzeb_runtime.dylib",
            ],
        );
        let symbols = run(
            &folder,
            &format!("{bin}/llvm-nm"),
            &["--extern-only", "--defined-only", "libzeb_runtime.dylib"],
        );
        fs::write(folder.join("symbols.txt"), &symbols).unwrap();
        let candidates: Vec<_> = symbols
            .lines()
            .filter_map(|line| line.split_whitespace().last())
            .filter(|s| s.starts_with("__R") && s.ends_with("13SCALAR_API_V1"))
            .collect();
        assert_eq!(candidates.len(), 1, "{symbols}");
        let symbol = &candidates[0][1..];
        fs::write(folder.join("symbol.txt"), symbol).unwrap();
        let consumer = format!(
            "#include <stdint.h>\n#include <stddef.h>\nstruct scalar_api {{ uint32_t version; uint32_t size; uint32_t (*classify)(uint64_t); }};\n_Static_assert(sizeof(struct scalar_api)==16,\"size\");\n_Static_assert(offsetof(struct scalar_api,classify)==8,\"offset\");\nextern const struct scalar_api {symbol};\nint main(void) {{ const struct scalar_api *api=&{symbol}; if(api->version!=1 || api->size!=16)return 1; const uint64_t values[]={{0,1,2,UINT64_C(0xffffffff00000002),UINT64_C(0x8000000000000002),3,UINT64_MAX,UINT64_C(0x100000000),UINT64_C(0x100000001)}}; const uint32_t expected[]={{0,1,2,2,2,255,255,255,255}}; for(unsigned i=0;i<9;i++)if(api->classify(values[i])!=expected[i])return 2; return 0; }}\n"
        );
        fs::write(folder.join("consumer.c"), consumer).unwrap();
        run(
            &folder,
            &format!("{bin}/clang"),
            &[
                "-target",
                target.triple(),
                target.clang_cpu(),
                "-isysroot",
                sdk,
                "-mmacosx-version-min=14.0",
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "consumer.c",
                "-L.",
                "-lzeb_runtime",
                "-Wl,-rpath,@loader_path",
                "-o",
                "consumer",
            ],
        );
        let mut game = llvm::emit_with_runtime(&ast, target, symbol).unwrap();
        game += "\ndefine i32 @main() {\n  %a = call %out @zfn0(i64 176093659138)\n  %av = extractvalue %out %a, 0\n  %ae = extractvalue %out %a, 1\n  %avok = icmp eq i64 %av, 180388626434\n  %aeok = icmp eq i32 %ae, 0\n  %aok = and i1 %avok, %aeok\n  %b = call %out @zfn0(i64 1)\n  %be = extractvalue %out %b, 1\n  %bs = extractvalue %out %b, 2\n  %beok = icmp eq i32 %be, 1\n  %bsok = icmp eq i64 %bs, 12\n  %bok = and i1 %beok, %bsok\n  %c = call %out @zfn1(i64 4294967297)\n  %ce = extractvalue %out %c, 1\n  %cok = icmp eq i32 %ce, 1\n  %abok = and i1 %aok, %bok\n  %allok = and i1 %abok, %cok\n  %exit = select i1 %allok, i32 0, i32 1\n  ret i32 %exit\n}\n";
        fs::write(folder.join("game.ll"), game).unwrap();
        run(
            &folder,
            &format!("{bin}/opt"),
            &["-passes=verify", "-disable-output", "game.ll"],
        );
        for optimization in ["-O0", "-O2"] {
            run(
                &folder,
                &format!("{bin}/clang"),
                &[
                    "-target",
                    target.triple(),
                    target.clang_cpu(),
                    "-isysroot",
                    sdk,
                    "-mmacosx-version-min=14.0",
                    optimization,
                    "game.ll",
                    "-L.",
                    "-lzeb_runtime",
                    "-Wl,-rpath,@loader_path",
                    "-o",
                    &format!("game{optimization}"),
                ],
            );
            if common::runs_on_host(arch) {
                run(&folder, &format!("./game{optimization}"), &[]);
            }
        }
        let mut bad = llvm::emit_with_runtime(&ast, target, symbol).unwrap();
        bad += "\ndefine i32 @main() {\n  %r = call %out @zfn0(i64 2)\n  %e = extractvalue %out %r, 1\n  %ok = icmp eq i32 %e, 5\n  %exit = select i1 %ok, i32 0, i32 1\n  ret i32 %exit\n}\n";
        fs::write(folder.join("bad-api.ll"), bad).unwrap();
        for (name, version, size) in [("bad-version", 2, 16), ("bad-size", 1, 0)] {
            let fake = format!(
                "#include <stdint.h>\nstruct scalar_api {{ uint32_t version,size; uint32_t (*classify)(uint64_t); }};\nconst struct scalar_api {symbol} = {{ {version}, {size}, 0 }};\n"
            );
            fs::write(folder.join(format!("{name}.c")), fake).unwrap();
            run(
                &folder,
                &format!("{bin}/clang"),
                &[
                    "-target",
                    target.triple(),
                    target.clang_cpu(),
                    "-isysroot",
                    sdk,
                    "-mmacosx-version-min=14.0",
                    "-O2",
                    "bad-api.ll",
                    &format!("{name}.c"),
                    "-o",
                    name,
                ],
            );
            if common::runs_on_host(arch) {
                run(&folder, &format!("./{name}"), &[]);
            }
        }
        let closure = run(
            &folder,
            &tools.otool,
            &["-L", "libzeb_runtime.dylib", "consumer", "game-O2"],
        );
        assert!(!closure.contains("/opt/local/") && !closure.contains("/usr/local/"));
        fs::write(folder.join("dependencies.txt"), closure).unwrap();
        if common::runs_on_host(arch) {
            run(&folder, "./consumer", &[]);
        }
    }
    for artifact in ["libzeb_runtime.dylib", "consumer", "game-O0", "game-O2"] {
        run(
            &dir,
            &tools.lipo,
            &[
                "-create",
                &format!("x86_64/{artifact}"),
                &format!("arm64/{artifact}"),
                "-output",
                artifact,
            ],
        );
        run(
            &dir,
            &tools.lipo,
            &[artifact, "-verify_arch", "x86_64", "arm64"],
        );
    }
    for artifact in ["consumer", "game-O0", "game-O2"] {
        run(&dir, &format!("./{artifact}"), &[]);
    }
    println!("safe runtime linked artifacts: {}", dir.display());
}
