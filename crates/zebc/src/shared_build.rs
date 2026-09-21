//! Private scalar linkable bundles. Runtime Rust exports keep their ordinary mangling.
use std::{fs, io::Write, path::Path, time::Duration};
use zeb_frontend::{
    llvm::{self, Target},
    parser::{Ast, Syntax},
    source::Source,
};
use zebc::process::run;
use zebc::toolchain::Toolchain;
const RUNTIME: &str = include_str!("../../zeb-runtime/src/lib.rs");
fn write(dir: &Path, name: &str, contents: impl AsRef<[u8]>) -> Result<(), String> {
    fs::write(dir.join(name), contents).map_err(|e| e.to_string())
}
fn command(dir: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    run(dir, program, args, Duration::from_secs(60))
}
fn hash(dir: &Path, file: &str) -> Result<String, String> {
    zebc::digest::file(&dir.join(file))
}
fn entry_wrapper(entry: usize, symbol: &str, target: Target, arity: usize) -> String {
    let version = if arity == 0 { 1 } else { 2 };
    let parameters = (0..arity)
        .map(|i| format!(", i32 %arg{i}"))
        .collect::<String>();
    let packing = (0..arity).map(|i| format!("  %wide{i} = zext i32 %arg{i} to i64\n  %bits{i} = shl i64 %wide{i}, 32\n  %packed{i} = or i64 %bits{i}, 2\n")).collect::<String>();
    let arguments = (0..arity)
        .map(|i| format!("i64 %packed{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let cpu = target.cpu();
    format!(
        "\ndefine i64 @{symbol}(i32 %abi{parameters}) \"target-cpu\"=\"{cpu}\" {{\n  %matches = icmp eq i32 %abi, {version}\n  br i1 %matches, label %invoke, label %mismatch\nmismatch:\n  ret i64 7\ninvoke:\n{packing}  %r = call %out @zfn{entry}({arguments})\n  %value = extractvalue %out %r, 0\n  %error = extractvalue %out %r, 1\n  %site = extractvalue %out %r, 2\n  %failed = icmp ne i32 %error, 0\n  %kind = add i32 %error, 2\n  %tag = zext i32 %kind to i64\n  %payload = shl i64 %site, 32\n  %failure = or i64 %payload, %tag\n  %word = select i1 %failed, i64 %failure, i64 %value\n  ret i64 %word\n}}\n"
    )
}
fn consumer(symbol: &str, arity: usize) -> String {
    let version = if arity == 0 { 1 } else { 2 };
    let (helper, main, parse) = if arity == 0 {
        (String::new(), "int main(void)".to_owned(), String::new())
    } else {
        let helper = r#"#include <stdlib.h>
#include <errno.h>
static int parse_i32(const char *text, int32_t *value) {
  const char *digits = text;
  if (*digits == '+' || *digits == '-') ++digits;
  if (!*digits) return 0;
  for (const char *p = digits; *p; ++p) if (*p < '0' || *p > '9') return 0;
  errno = 0;
  char *end;
  intmax_t parsed = strtoimax(text, &end, 10);
  if (errno || *end || parsed < INT32_MIN || parsed > INT32_MAX) return 0;
  *value = (int32_t)parsed;
  return 1;
}
"#
        .to_owned();
        let mut parse = format!(
            "  if (argc != {}) {{ fputs(\"expected {arity} signed integer arguments\\n\", stderr); return 64; }}\n",
            arity + 1
        );
        for i in 0..arity {
            parse += &format!(
                "  int32_t arg{i};\n  if (!parse_i32(argv[{}], &arg{i})) {{ fputs(\"invalid signed integer argument\\n\", stderr); return 64; }}\n",
                i + 1
            );
        }
        (helper, "int main(int argc, char **argv)".to_owned(), parse)
    };
    let arguments = (0..arity).map(|i| format!(", arg{i}")).collect::<String>();
    let mismatch_version = if arity == 0 { 0 } else { 1 };
    format!(
        "#include <stdint.h>\n#include <inttypes.h>\n#include <stdio.h>\n#include \"zeb_game.h\"\n{helper}{main} {{\n{parse}  if ({symbol}({mismatch_version}{arguments}) != UINT64_C(7)) {{ fputs(\"ABI mismatch\\n\", stderr); return 71; }}\n  uint64_t word = {symbol}({version}{arguments});\n  uint32_t kind=(uint32_t)word, payload=(uint32_t)(word>>32);\n  if(kind==0 && payload==0) {{ puts(\"nil\"); return 0; }}\n  if(kind==1 && payload==0) {{ puts(\"true\"); return 0; }}\n  if(kind==2) {{ int64_t value=payload<=INT32_MAX ? (int64_t)payload : (int64_t)payload-INT64_C(4294967296); printf(\"%\" PRId64 \"\\n\",value); return 0; }}\n  if(kind>=3 && kind<=7) {{ fprintf(stderr,\"error:%\" PRIu32 \" byte:%\" PRIu32 \"\\n\",kind-2,payload); return 70; }}\n  fputs(\"invalid scalar result\\n\",stderr); return 71;\n}}\n"
    )
}
#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Shared,
    Object,
    Static,
}
impl Format {
    fn profile(self, arity: usize) -> &'static str {
        match (self, arity == 0) {
            (Self::Shared, true) => "scalar-shared-v1",
            (Self::Object, true) => "scalar-object-v1",
            (Self::Static, true) => "scalar-static-v1",
            (Self::Shared, false) => "scalar-shared-i32-v2",
            (Self::Object, false) => "scalar-object-i32-v2",
            (Self::Static, false) => "scalar-static-i32-v2",
        }
    }
}
pub fn build(
    ast: &Ast,
    source: &Source,
    out: &Path,
    optimize: bool,
    format: Format,
    full_lto: bool,
) -> Result<(), String> {
    if full_lto && (format != Format::Shared || !optimize) {
        return Err("full LTO requires optimized shared output".to_owned());
    }
    let host = Target::host()?;
    if source.byte_len() > u32::MAX as usize {
        return Err("scalar native ABI requires source offsets within u32".to_owned());
    }
    let (entry, arity) = ast
        .functions
        .iter()
        .enumerate()
        .find_map(|(i, id)| match &ast.nodes[id.0].syntax {
            Syntax::Function {
                name, parameters, ..
            } if name == "main" && parameters.len() <= 3 => Some((i, parameters.len())),
            _ => None,
        })
        .ok_or("scalar linkable entry requires main with zero to three parameters")?;
    zeb_frontend::flow::check(ast).map_err(|d| format!("{} at {}", d.code, d.byte))?;
    fs::create_dir(out).map_err(|e| format!("output directory must be new: {e}"))?;
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    let result = (|| {
        let tools = Toolchain::resolve(&out)?;
        tools.record(&out)?;

        let rustc = tools.rustc.clone();
        let compiler = std::env::current_exe().map_err(|e| e.to_string())?;
        let compiler = compiler
            .to_str()
            .ok_or("compiler path must be UTF-8 for this initial driver")?;
        for (name, program, expected) in [
            ("rustc", rustc.clone(), "rustc 1.98.1"),
            ("opt", tools.tool("opt"), "version 22.1.8"),
            ("clang", tools.tool("clang"), "version 22.1.8"),
        ] {
            let text = command(&out, &program, &["--version"])?;
            if !text.contains(expected) {
                return Err(format!("wrong {name} version"));
            }
            write(&out, &format!("{name}-version.txt"), text)?;
        }
        write(&out, "source.t", source.original_bytes())?;
        write(&out, "runtime.rs", RUNTIME)?;
        write(
            &out,
            "source-encoding.txt",
            format!("{:?}\n", source.encoding),
        )?;
        let tool_hashes = zebc::digest::sums(
            &out,
            &[
                compiler,
                &rustc,
                &tools.tool("opt"),
                &tools.tool("clang"),
                &tools.tool("llvm-ar"),
            ],
        )?;
        write(&out, "TOOLS-SHA256SUMS", &tool_hashes)?;
        let profile = format.profile(arity);
        let version = if arity == 0 { 1 } else { 2 };
        let source_hash = hash(&out, "source.t")?;
        let runtime_hash = hash(&out, "runtime.rs")?;
        write(
            &out,
            "identity.txt",
            format!(
                "{profile}\n{source_hash}\n{runtime_hash}\n{tool_hashes}\n{:?}\n{optimize}\nlto-full={full_lto}\n",
                source.encoding
            ),
        )?;
        let mut identity_file = fs::OpenOptions::new()
            .append(true)
            .open(out.join("identity.txt"))
            .map_err(|e| e.to_string())?;
        writeln!(
            identity_file,
            "{}\n{}\n{}",
            tools.lock_json,
            tools.profile_json,
            include_str!("shared_build.rs")
        )
        .map_err(|e| e.to_string())?;
        let identity = hash(&out, "identity.txt")?;
        let symbol = format!("zeb_game_{identity}_v{version}");
        let game_ext = match format {
            Format::Shared => host.shared_ext(),
            Format::Object => host.object_ext(),
            Format::Static => host.archive_ext(),
        };
        let runtime_ext = if format == Format::Shared {
            host.shared_ext()
        } else {
            host.object_ext()
        };
        let game_lib = format!("libzeb_game_{identity}.{game_ext}");
        let runtime_lib = format!("libzeb_runtime_{identity}.{runtime_ext}");
        let arguments = (0..arity)
            .map(|i| format!(", int32_t arg{i}"))
            .collect::<String>();
        write(
            &out,
            "zeb_game.h",
            format!(
                "#ifndef ZEB_GAME_{identity}_H\n#define ZEB_GAME_{identity}_H\n#include <stdint.h>\n/* Private scalar ABI v{version}; exact matching bundle required. */\nuint64_t {symbol}(uint32_t abi_version{arguments});\n#endif\n"
            ),
        )?;
        write(&out, "consumer.c", consumer(&symbol, arity))?;
        let mut slices = Vec::new();
        let consumer_name = host.executable("consumer");
        for &target in host.slices() {
            let arch = target.arch();
            let rust_target = target.rust_triple();
            let folder = out.join(arch);
            fs::create_dir(&folder).map_err(|e| e.to_string())?;
            let text = if full_lto {
                compile_runtime_bitcode(&folder, target, rust_target, &rustc, &tools)?;
                String::new()
            } else {
                let mut args: Vec<String> = [
                    "--edition=2024",
                    "--crate-name=zeb_runtime",
                    "--target",
                    rust_target,
                    "-C",
                    "opt-level=2",
                    "../runtime.rs",
                    "-o",
                    &runtime_lib,
                ]
                .into_iter()
                .map(str::to_owned)
                .collect();
                args.extend(["-C".into(), format!("target-cpu={}", target.cpu())]);
                if format == Format::Shared {
                    args.push("--crate-type=dylib".into());
                    args.extend(tools.rust_args(target, &runtime_lib));
                } else {
                    args.extend(["--crate-type=lib".into(), "--emit=obj".into()]);
                }
                tools.run(&folder, &rustc, &args)?
            };
            write(&folder, "rust-build.txt", text)?;
            let symbols = if target.is_windows() && format == Format::Shared && !full_lto {
                tools.exports(&folder, &runtime_lib)?
            } else {
                tools.symbols(
                    &folder,
                    if full_lto { "runtime.bc" } else { &runtime_lib },
                    false,
                )?
            };
            let candidates: Vec<_> = symbols
                .lines()
                .filter_map(|line| line.split_whitespace().last())
                .filter(|s| {
                    target.ir_symbol(s).starts_with("_R")
                        && s.ends_with("13SCALAR_API_V1")
                        && !s.starts_with("__imp_")
                })
                .collect();
            if candidates.len() != 1 {
                return Err("runtime API symbol is missing or ambiguous".to_owned());
            }
            let runtime_symbol = target.ir_symbol(candidates[0]);
            write(&folder, "runtime-symbol.txt", runtime_symbol)?;
            let mut module = if optimize {
                llvm::emit_with_runtime_rooted(ast, target, runtime_symbol, &[entry])
            } else {
                llvm::emit_with_runtime(ast, target, runtime_symbol)
            }
            .map_err(|d| format!("{}: {}", d.code, d.message))?;
            module += &entry_wrapper(entry, &symbol, target, arity);
            if target.is_windows() && format == Format::Shared && !full_lto {
                module = module.replace(
                    "external constant %scalar_api",
                    "external dllimport constant %scalar_api",
                );
            }
            let module = zebc::platform::export_ir(module, target);
            write(&folder, "game.ll", module)?;
            command(
                &folder,
                &tools.tool("opt"),
                &["-passes=verify", "-disable-output", "game.ll"],
            )?;
            // Retain the actual object used by every link/archive, together with
            // Clang's frame report for that same code generation invocation.
            let object = if format == Format::Object {
                game_lib.as_str()
            } else {
                "game.o"
            };
            if !full_lto {
                let text = tools.clang(
                    &folder,
                    target,
                    &[
                        if optimize { "-O2".into() } else { "-O0".into() },
                        "-fstack-usage".into(),
                        "-c".into(),
                        "game.ll".into(),
                        "-o".into(),
                        object.into(),
                    ],
                )?;
                write(&folder, "game-build.txt", text)?;
                let stack_path = folder.join(object).with_extension("su");
                let stack =
                    fs::read(&stack_path).map_err(|e| format!("missing stack report: {e}"))?;
                if stack.is_empty() {
                    return Err("empty stack report".to_owned());
                }
                fs::rename(stack_path, folder.join("game-stack.su")).map_err(|e| e.to_string())?;
                if format == Format::Shared {
                    let mut args = tools.shared_args(target, &game_lib);
                    args.extend([
                        object.into(),
                        tools.import_library(&runtime_lib),
                        "-o".into(),
                        game_lib.clone(),
                    ]);
                    let text = tools.clang(&folder, target, &args)?;
                    write(&folder, "game-link.txt", text)?;
                } else if format == Format::Static {
                    let text = command(
                        &folder,
                        &tools.tool("llvm-ar"),
                        &["rcsD", &game_lib, object, &runtime_lib],
                    )?;
                    write(&folder, "archive-build.txt", text)?;
                }
            }
            if full_lto {
                full_link(&folder, target, &game_lib, &tools)?;
            }
            let exports = tools.exports(&folder, &game_lib)?;
            if !exports.lines().any(|line| {
                line.split_whitespace()
                    .last()
                    .is_some_and(|s| target.ir_symbol(s) == symbol)
            }) {
                return Err("game entry export missing".to_owned());
            }
            write(&folder, "game-symbols.txt", exports)?;
            let mut consumer_args: Vec<String> =
                ["-std=c11", "-Wall", "-Wextra", "-Werror", "../consumer.c"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
            consumer_args.push(tools.import_library(&game_lib));
            if format == Format::Object {
                consumer_args.push(runtime_lib.clone());
            }
            consumer_args.extend(tools.linker_args(target));
            consumer_args.extend(tools.loader_args(target));
            consumer_args.extend(["-o".into(), consumer_name.clone()]);
            tools.clang(&folder, target, &consumer_args)?;
            let mut dependencies = vec![consumer_name.as_str()];
            if format == Format::Shared {
                dependencies.push(&game_lib);
                if !full_lto {
                    dependencies.push(&runtime_lib);
                }
            }
            let closure = tools.dependencies(&folder, &dependencies)?;
            write(&folder, "dependencies.txt", closure)?;
            slices.push(format!(
                "{{\"architecture\":\"{arch}\",\"runtime_symbol\":\"{runtime_symbol}\"}}"
            ));
        }
        let artifacts = if full_lto {
            vec![game_lib.as_str(), consumer_name.as_str()]
        } else {
            vec![
                runtime_lib.as_str(),
                game_lib.as_str(),
                consumer_name.as_str(),
            ]
        };
        tools.assemble(&out, &artifacts)?;
        let mut hash_inputs = vec![
            "source.t".to_owned(),
            "source-encoding.txt".to_owned(),
            "native-tool-lock.json".to_owned(),
            "target-profile.json".to_owned(),
            "runtime.rs".to_owned(),
            "identity.txt".to_owned(),
            "zeb_game.h".to_owned(),
            "consumer.c".to_owned(),
            game_lib.clone(),
            consumer_name.clone(),
        ];
        if !full_lto {
            hash_inputs.push(runtime_lib.clone());
        }
        for target in host.slices() {
            let arch = target.arch();
            if full_lto {
                for name in [
                    "game.ll",
                    "game-final.o",
                    "game-stack.su",
                    "lto-sections.txt",
                    "lto-optimized.bc",
                ] {
                    hash_inputs.push(format!("{arch}/{name}"));
                }
                continue;
            }
            let object = if format == Format::Object {
                game_lib.as_str()
            } else {
                "game.o"
            };
            for name in ["game.ll", "game-stack.su", "rust-build.txt", object] {
                hash_inputs.push(format!("{arch}/{name}"));
            }
        }
        if full_lto {
            for target in host.slices() {
                let arch = target.arch();
                for name in [
                    "runtime.bc",
                    "runtime.lto.o",
                    "game.lto.o",
                    "lto-optimized.ll",
                ] {
                    hash_inputs.push(format!("{arch}/{name}"));
                }
            }
        }
        if host.is_windows() && format == Format::Shared {
            hash_inputs.push(tools.import_library(&game_lib));
            if !full_lto {
                hash_inputs.push(tools.import_library(&runtime_lib));
            }
        }
        let hashes = zebc::digest::sums(
            &out,
            &hash_inputs.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
        write(&out, "SHA256SUMS", hashes)?;
        let lto_mode = if full_lto { "full" } else { "none" };
        let stack_scope = if full_lto {
            "linked-lto-object"
        } else {
            "linked-object"
        };
        let runtime_artifact = if full_lto { &game_lib } else { &runtime_lib };
        let bundle_target = host.bundle_name();
        let deployment = if host.is_macos() { "14.0" } else { "host" };
        let imports = if host.is_windows() && format == Format::Shared {
            format!(
                ",\"game_import\":\"{}\",\"runtime_import\":\"{}\"",
                tools.import_library(&game_lib),
                tools.import_library(runtime_artifact)
            )
        } else {
            String::new()
        };
        let manifest = format!(
            "{{\"schema\":1,\"status\":\"complete\",\"profile\":\"{profile}\",\"target\":\"{bundle_target}\",\"deployment\":\"{deployment}\",\"optimization\":\"{}\",\"identity\":\"{identity}\",\"game_entry\":\"{symbol}\",\"abi_version\":{version},\"argument_count\":{arity},\"runtime\":\"{runtime_artifact}\",\"game\":\"{game_lib}\",\"consumer\":\"{consumer_name}\"{imports},\"slices\":[{}],\"lto\":\"{lto_mode}\",\"stack_report_scope\":\"{stack_scope}\",\"qualified\":false,\"adventure_game\":false}}\n",
            if optimize { "O2" } else { "O0" },
            slices.join(",")
        );
        let mut file = fs::File::create(out.join("manifest.pending")).map_err(|e| e.to_string())?;
        file.write_all(manifest.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(out.join("manifest.pending"), out.join("manifest.json"))
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = write(&out, "FAILED.txt", error);
    }
    result
}

/// Opt-in full LTO: actual runtime bitcode and generated game code; consumer stays separate.
fn compile_runtime_bitcode(
    folder: &Path,
    target: Target,
    rust_target: &str,
    rustc: &str,
    tools: &Toolchain,
) -> Result<(), String> {
    command(
        folder,
        rustc,
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
            &format!("target-cpu={}", target.cpu()),
            "../runtime.rs",
            "-o",
            "runtime.bc",
        ],
    )?;
    command(
        folder,
        &tools.tool("opt"),
        &["-passes=verify", "-disable-output", "runtime.bc"],
    )?;
    Ok(())
}

fn full_link(folder: &Path, target: Target, game: &str, tools: &Toolchain) -> Result<(), String> {
    if !target.is_macos() {
        return portable_full_link(folder, target, game, tools);
    }
    let bin = &tools.bin;
    let sdk = tools.sdk.as_str();
    let common = [
        "-target",
        target.triple(),
        target.clang_cpu(),
        "-isysroot",
        sdk,
        "-mmacosx-version-min=14.0",
        "-O2",
        "-flto=full",
    ];
    for (input, output) in [("runtime.bc", "runtime.lto.o"), ("game.ll", "game.lto.o")] {
        let mut args = common.to_vec();
        args.extend(["-c", input, "-o", output]);
        command(folder, &tools.tool("clang"), &args)?;
    }
    let linker = format!("-fuse-ld={bin}/ld64.lld");
    let install_name = format!("-Wl,-install_name,@rpath/{game}");
    let mut args = common.to_vec();
    args.extend([
        &linker,
        "-dynamiclib",
        "game.lto.o",
        "runtime.lto.o",
        &install_name,
        "-Wl,--lto-emit-llvm",
        "-o",
        "lto-optimized.bc",
    ]);
    command(folder, &tools.tool("clang"), &args)?;
    let optimized = "lto-optimized.bc";
    command(
        folder,
        &tools.tool("opt"),
        &["-passes=verify", "-disable-output", optimized],
    )?;
    command(
        folder,
        &format!("{bin}/llvm-dis"),
        &[optimized, "-o", "lto-optimized.ll"],
    )?;
    // Generate the object and frame report together from the optimized combined
    // module. The final native link consumes this exact object, without LTO.
    let mut native = common[..common.len() - 1].to_vec();
    native.extend(["-fstack-usage", "-c", optimized, "-o", "game-final.o"]);
    command(folder, &tools.tool("clang"), &native)?;
    let stack = fs::read(folder.join("game-final.su"))
        .map_err(|e| format!("missing final frame report: {e}"))?;
    if stack.is_empty() {
        return Err("empty final frame report".to_owned());
    }
    fs::rename(folder.join("game-final.su"), folder.join("game-stack.su"))
        .map_err(|e| e.to_string())?;
    let mut native_link = common[..common.len() - 1].to_vec();
    native_link.extend([
        &linker,
        "-dynamiclib",
        "game-final.o",
        &install_name,
        "-o",
        game,
    ]);
    command(folder, &tools.tool("clang"), &native_link)?;
    let sections = command(folder, &tools.otool, &["-l", "game-final.o", game])?;
    write(folder, "lto-sections.txt", sections)?;
    let closure = command(folder, &tools.otool, &["-L", game])?;
    for line in closure.lines().filter(|line| line.starts_with('\t')) {
        let dependency = line.trim().split(" (").next().ok_or("missing dependency")?;
        if dependency != format!("@rpath/{game}")
            && !dependency.starts_with("/usr/lib/")
            && !dependency.starts_with("/System/Library/")
        {
            return Err(format!(
                "unexpected combined-library dependency: {dependency}"
            ));
        }
    }
    write(folder, "lto-dependencies.txt", closure)
}

fn portable_full_link(
    folder: &Path,
    target: Target,
    game: &str,
    tools: &Toolchain,
) -> Result<(), String> {
    for (input, output) in [("runtime.bc", "runtime.lto.o"), ("game.ll", "game.lto.o")] {
        tools.clang(
            folder,
            target,
            &[
                "-O2".into(),
                "-flto=full".into(),
                "-c".into(),
                input.into(),
                "-o".into(),
                output.into(),
            ],
        )?;
    }
    let mut args = tools.shared_args(target, game);
    args.extend([
        "-O2".into(),
        "-flto=full".into(),
        "game.lto.o".into(),
        "runtime.lto.o".into(),
        if target.is_windows() {
            "-Wl,/lldsavetemps".into()
        } else {
            "-Wl,--save-temps".into()
        },
        "-o".into(),
        game.into(),
    ]);
    tools.clang(folder, target, &args)?;
    let candidates = fs::read_dir(folder)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".opt.bc"))
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err(format!(
            "expected one full-LTO optimized module, found {}",
            candidates.len()
        ));
    }
    fs::copy(&candidates[0], folder.join("lto-optimized.bc")).map_err(|e| e.to_string())?;
    command(
        folder,
        &tools.tool("opt"),
        &["-passes=verify", "-disable-output", "lto-optimized.bc"],
    )?;
    command(
        folder,
        &tools.tool("llvm-dis"),
        &["lto-optimized.bc", "-o", "lto-optimized.ll"],
    )?;
    tools.clang(
        folder,
        target,
        &[
            "-O2".into(),
            "-fstack-usage".into(),
            "-c".into(),
            "lto-optimized.bc".into(),
            "-o".into(),
            "game-final.o".into(),
        ],
    )?;
    let stack = fs::read(folder.join("game-final.su"))
        .map_err(|e| format!("missing final frame report: {e}"))?;
    if stack.is_empty() {
        return Err("empty final frame report".into());
    }
    fs::rename(folder.join("game-final.su"), folder.join("game-stack.su"))
        .map_err(|e| e.to_string())?;
    let mut native = tools.shared_args(target, game);
    native.extend(["game-final.o".into(), "-o".into(), game.into()]);
    tools.clang(folder, target, &native)?;
    let sections = command(
        folder,
        &tools.tool("llvm-readobj"),
        &["--sections", "game-final.o", game],
    )?;
    write(folder, "lto-sections.txt", sections)?;
    write(
        folder,
        "lto-dependencies.txt",
        tools.dependencies(folder, &[game])?,
    )
}
