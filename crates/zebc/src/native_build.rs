//! macOS scalar executable driver. No Rust FFI or user-code execution.
use std::{fs, io::Write, path::Path, time::Duration};
use zeb_frontend::{
    llvm::{self, Target},
    parser::{Ast, Syntax},
};
use zebc::process::run;
use zebc::toolchain::Toolchain;

pub fn build(
    ast: &Ast,
    source: &zeb_frontend::source::Source,
    out: &Path,
    optimize: bool,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("native build currently requires macOS".to_owned());
    }
    let entry = ast
        .functions
        .iter()
        .enumerate()
        .find_map(|(i, id)| match &ast.nodes[id.0].syntax {
            Syntax::Function {
                name, parameters, ..
            } if name == "main" && parameters.is_empty() => Some(i),
            _ => None,
        })
        .ok_or("scalar executable requires main() with no parameters")?;
    // Complete source checking/emission before reserving the destination.
    let emit = |target| {
        if optimize {
            llvm::emit_rooted(ast, target, &[entry])
        } else {
            llvm::emit(ast, target)
        }
    };
    let modules = [emit(Target::MacX86_64), emit(Target::MacArm64)];
    let modules = modules
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|d| format!("{} at byte {}: {}", d.code, d.byte, d.message))?;
    fs::create_dir(out).map_err(|e| format!("output directory must be new: {e}"))?;
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    let result = (|| {
        let tools = Toolchain::resolve(&out)?;
        tools.record(&out)?;
        let bin = tools.bin.as_str();
        let sdk = tools.sdk.as_str();
        let timeout = Duration::from_secs(30);
        fs::write(out.join("source.t"), source.original_bytes()).map_err(|e| e.to_string())?;
        fs::write(
            out.join("source-encoding.txt"),
            format!("{:?}\n", source.encoding),
        )
        .map_err(|e| e.to_string())?;
        let compiler = std::env::current_exe().map_err(|e| e.to_string())?;
        let compiler = compiler
            .to_str()
            .ok_or("compiler path must be UTF-8 for this initial driver")?;
        let tool_hashes = run(
            &out,
            "/usr/bin/shasum",
            &[
                "-a",
                "256",
                compiler,
                &format!("{bin}/opt"),
                &format!("{bin}/clang"),
            ],
            timeout,
        )?;
        fs::write(out.join("TOOLS-SHA256SUMS"), tool_hashes).map_err(|e| e.to_string())?;
        for tool in ["opt", "clang"] {
            let text = run(&out, &format!("{bin}/{tool}"), &["--version"], timeout)?;
            if !text.contains("version 22.1.8") {
                return Err(format!("{tool} is not LLVM 22.1.8"));
            }
            fs::write(out.join(format!("{tool}-version.txt")), text).map_err(|e| e.to_string())?;
        }
        for (index, mut module) in modules.into_iter().enumerate() {
            let (arch, target) = if index == 0 {
                ("x86_64", Target::MacX86_64)
            } else {
                ("arm64", Target::MacArm64)
            };
            // Conventional process entry only; game functions retain their internal outcome ABI.
            let cpu = target.cpu();
            module += &format!(
                "\ndefine i32 @main() \"target-cpu\"=\"{cpu}\" {{\n  %r = call %out @zfn{entry}()\n  %v = extractvalue %out %r, 0\n  %err = extractvalue %out %r, 1\n  %failed = icmp ne i32 %err, 0\n  %bits = lshr i64 %v, 32\n  %int = trunc i64 %bits to i32\n  %tag = trunc i64 %v to i32\n  %isint = icmp eq i32 %tag, 2\n  %value = select i1 %isint, i32 %int, i32 %tag\n  %exit = select i1 %failed, i32 70, i32 %value\n  ret i32 %exit\n}}\n"
            );
            let file = format!("{arch}.ll");
            let exe = format!("{arch}.exe");
            let object = format!("{arch}.o");
            let usage = format!("{arch}.su");
            fs::write(out.join(&file), module).map_err(|e| e.to_string())?;
            let verification = run(
                &out,
                &format!("{bin}/opt"),
                &["-passes=verify", "-disable-output", &file],
                timeout,
            )?;
            fs::write(out.join(format!("{arch}-verify.txt")), verification)
                .map_err(|e| e.to_string())?;
            let compilation = run(
                &out,
                &format!("{bin}/clang"),
                &[
                    "-target",
                    target.triple(),
                    target.clang_cpu(),
                    "-isysroot",
                    sdk,
                    "-mmacosx-version-min=14.0",
                    if optimize { "-O2" } else { "-O0" },
                    "-fstack-usage",
                    "-c",
                    &file,
                    "-o",
                    &object,
                ],
                timeout,
            )?;
            fs::write(out.join(format!("{arch}-compile.txt")), compilation)
                .map_err(|e| e.to_string())?;
            if !out.join(&usage).is_file() {
                return Err(format!("missing executable frame report: {usage}"));
            }
            // Link the exact object whose code-generation invocation emitted the report.
            let linkage = run(
                &out,
                &format!("{bin}/clang"),
                &[
                    "-target",
                    target.triple(),
                    target.clang_cpu(),
                    "-isysroot",
                    sdk,
                    "-mmacosx-version-min=14.0",
                    &object,
                    "-o",
                    &exe,
                ],
                timeout,
            )?;
            fs::write(out.join(format!("{arch}-link.txt")), linkage).map_err(|e| e.to_string())?;
        }
        run(
            &out,
            &tools.lipo,
            &["-create", "x86_64.exe", "arm64.exe", "-output", "program"],
            timeout,
        )?;
        run(
            &out,
            &tools.lipo,
            &["program", "-verify_arch", "x86_64", "arm64"],
            timeout,
        )?;
        let closure = run(&out, &tools.otool, &["-L", "program"], timeout)?;
        fs::write(out.join("dependencies.txt"), closure).map_err(|e| e.to_string())?;
        let hashes = run(
            &out,
            "/usr/bin/shasum",
            &[
                "-a",
                "256",
                "source.t",
                "source-encoding.txt",
                "native-tool-lock.json",
                "target-profile.json",
                "x86_64.ll",
                "arm64.ll",
                "x86_64.o",
                "arm64.o",
                "x86_64.su",
                "arm64.su",
                "x86_64.exe",
                "arm64.exe",
                "program",
            ],
            timeout,
        )?;
        fs::write(out.join("SHA256SUMS"), hashes).map_err(|e| e.to_string())?;
        let manifest = format!(
            "{{\"schema\":1,\"status\":\"complete\",\"profile\":\"scalar-executable-v1\",\"target\":\"macos-universal\",\"deployment\":\"14.0\",\"optimization\":\"{}\",\"artifact\":\"program\",\"stack_report_scope\":\"linked-source-object\",\"qualified\":false,\"game_runtime\":false}}\n",
            if optimize { "O2" } else { "O0" }
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
        let _ = fs::write(out.join("FAILED.txt"), error);
    }
    result
}
