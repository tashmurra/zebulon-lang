//! Host-native scalar executable driver. No Rust FFI or user-code execution.
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
    let host = Target::host()?;
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
    let modules = host.slices().iter().copied().map(emit).collect::<Vec<_>>();
    let modules = modules
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|d| format!("{} at byte {}: {}", d.code, d.byte, d.message))?;
    fs::create_dir(out).map_err(|e| format!("output directory must be new: {e}"))?;
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    let result = (|| {
        let tools = Toolchain::resolve(&out)?;
        tools.record(&out)?;
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
        let tool_hashes =
            zebc::digest::sums(&out, &[compiler, &tools.tool("opt"), &tools.tool("clang")])?;
        fs::write(out.join("TOOLS-SHA256SUMS"), tool_hashes).map_err(|e| e.to_string())?;
        for tool in ["opt", "clang"] {
            let text = run(&out, &tools.tool(tool), &["--version"], timeout)?;
            if !text.contains("version 22.1.8") {
                return Err(format!("{tool} is not LLVM 22.1.8"));
            }
            fs::write(out.join(format!("{tool}-version.txt")), text).map_err(|e| e.to_string())?;
        }
        for (index, mut module) in modules.into_iter().enumerate() {
            let target = host.slices()[index];
            let arch = target.arch();
            // Conventional process entry only; game functions retain their internal outcome ABI.
            let target_attributes = target.function_attributes();
            module += &format!(
                "\ndefine i32 @main() {target_attributes} {{\n  %r = call %out @zfn{entry}()\n  %v = extractvalue %out %r, 0\n  %err = extractvalue %out %r, 1\n  %failed = icmp ne i32 %err, 0\n  %bits = lshr i64 %v, 32\n  %int = trunc i64 %bits to i32\n  %tag = trunc i64 %v to i32\n  %isint = icmp eq i32 %tag, 2\n  %value = select i1 %isint, i32 %int, i32 %tag\n  %exit = select i1 %failed, i32 70, i32 %value\n  ret i32 %exit\n}}\n"
            );
            let file = format!("{arch}.ll");
            let exe = format!("{arch}.exe");
            let object = format!("{arch}.o");
            let usage = format!("{arch}.su");
            fs::write(out.join(&file), module).map_err(|e| e.to_string())?;
            let verification = run(
                &out,
                &tools.tool("opt"),
                &["-passes=verify", "-disable-output", &file],
                timeout,
            )?;
            fs::write(out.join(format!("{arch}-verify.txt")), verification)
                .map_err(|e| e.to_string())?;
            let compilation = tools.clang(
                &out,
                target,
                &[
                    if optimize { "-O2".into() } else { "-O0".into() },
                    "-fstack-usage".into(),
                    "-c".into(),
                    file.clone(),
                    "-o".into(),
                    object.clone(),
                ],
            )?;
            fs::write(out.join(format!("{arch}-compile.txt")), compilation)
                .map_err(|e| e.to_string())?;
            if !out.join(&usage).is_file() {
                return Err(format!("missing executable frame report: {usage}"));
            }
            // Link the exact object whose code-generation invocation emitted the report.
            let mut args = tools.linker_args(target);
            args.extend([object, "-o".into(), exe]);
            let linkage = tools.clang(&out, target, &args)?;
            fs::write(out.join(format!("{arch}-link.txt")), linkage).map_err(|e| e.to_string())?;
        }
        let program = host.executable("program");
        if host.is_macos() {
            run(
                &out,
                &tools.lipo,
                &["-create", "x86_64.exe", "arm64.exe", "-output", &program],
                timeout,
            )?;
            run(
                &out,
                &tools.lipo,
                &[&program, "-verify_arch", "x86_64", "arm64"],
                timeout,
            )?;
        } else {
            fs::copy(out.join("x86_64.exe"), out.join(&program)).map_err(|e| e.to_string())?;
        }
        let closure = tools.dependencies(&out, &[&program])?;
        fs::write(out.join("dependencies.txt"), closure).map_err(|e| e.to_string())?;
        let mut files: Vec<String> = [
            "source.t",
            "source-encoding.txt",
            "native-tool-lock.json",
            "target-profile.json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        files.push(program.clone());
        for target in host.slices() {
            for extension in ["ll", "o", "su", "exe"] {
                files.push(format!("{}.{extension}", target.arch()));
            }
        }
        let hashes =
            zebc::digest::sums(&out, &files.iter().map(String::as_str).collect::<Vec<_>>())?;
        fs::write(out.join("SHA256SUMS"), hashes).map_err(|e| e.to_string())?;
        let bundle_target = host.bundle_name();
        let deployment = if host.is_macos() { "14.0" } else { "host" };
        let manifest = format!(
            "{{\"schema\":1,\"status\":\"complete\",\"profile\":\"scalar-executable-v1\",\"target\":\"{bundle_target}\",\"deployment\":\"{deployment}\",\"optimization\":\"{}\",\"artifact\":\"{program}\",\"stack_report_scope\":\"linked-source-object\",\"qualified\":false,\"game_runtime\":false}}\n",
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
