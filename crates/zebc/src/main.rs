#![forbid(unsafe_code)]

mod native_build;
mod object_build;
mod shared_build;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use zeb_frontend::{
    Diagnostic, flow, ir, lexer, llvm, parser,
    parser::Model,
    source::{Encoding, Source},
};

const HELP: &str = "zebc — Zebulon compiler\n\
Usage:\n  zebc --version\n  zebc capabilities --format json\n  zebc inspect FILE --stage tokens|preprocessed|ast|ir|llvm|object-init-llvm|object-llvm [--encoding auto|utf-8|ascii|latin-1|utf-16le|utf-16be]\n  zebc check FILE\n  zebc build FILE --out-dir NEW_DIRECTORY [--opt O0|O2] [--emit obj|exe|static|shared] [--lto none|full] [--runtime-cache DIR]\n\
Optional: --include-dir DIR (repeatable); --max-source-bytes N (default 16777216)\n\
  --model ownership|lifetimes selects the memory model (default lifetimes); it applies to inspect, check and build\n\
check validates supported scalar/object source; build emits host-native bundles (universal on macOS; x86-64 on Linux/Windows).\n";

const CAPABILITIES_TEMPLATE: &str = r#"{"schema":1,"compiler":"zebc","compiler_version":"{compiler_version}","profile":"scalar-flow-v1","source_encodings":["utf-8","ascii","latin-1","utf-16le","utf-16be"],"token_inspection":true,"ast_inspection":true,"named_object_initialization_llvm":true,"scalar_ir_inspection":true,"llvm_inspection":true,"preliminary_semantic_checks":true,"semantic_check":true,"native_compilation":true,"native_profile":"scalar-executable-v1","shared_profile":"scalar-shared-v1","integer_argument_profiles":["scalar-shared-i32-v2","scalar-object-i32-v2","scalar-static-i32-v2"],"object_profile":"scalar-object-v1","static_profile":"scalar-static-v1","lto_modes":{"default":"none","full":"shared-O2-only"},"memory_models":["lifetimes","ownership"],"default_memory_model":"lifetimes","object_shared_profile":"{object_profile}","game_execution":true,"qualified_targets":[]}"#;

fn capabilities() -> String {
    let host = llvm::Target::host();
    let targets = host
        .as_ref()
        .map(|t| {
            t.slices()
                .iter()
                .map(|t| format!("\"{}\"", t.name()))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    CAPABILITIES_TEMPLATE
        .replace("{compiler_version}", env!("CARGO_PKG_VERSION"))
        .replace("{object_profile}", object_build::PROFILE)
        .replace(
            "\"native_compilation\":true",
            &format!("\"native_compilation\":{}", host.is_ok()),
        )
        .replace(
            "\"qualified_targets\":[]",
            &format!("\"build_targets\":[{targets}],\"qualified_targets\":[]"),
        )
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            let _ = writeln!(io::stderr().lock(), "{message}");
            ExitCode::from(code)
        }
    }
}

fn run(args: Vec<OsString>) -> Result<(), (u8, String)> {
    let usage = |message: &str| (2, format!("zebc: {message}\n{HELP}"));
    let Some(command) = args.first().and_then(|value| value.to_str()) else {
        return Err(usage("missing command"));
    };
    let mut output = io::stdout().lock();
    let io_error = |error: io::Error| (1, format!("zebc: output-io: {error}"));
    if matches!(command, "--help" | "-h") && args.len() == 1 {
        return write!(output, "{HELP}").map_err(io_error);
    }
    if command == "--version" && args.len() == 1 {
        return writeln!(output, "zebc {}", env!("CARGO_PKG_VERSION")).map_err(io_error);
    }
    if command == "capabilities" {
        if args.len() != 3 || args[1] != "--format" || args[2] != "json" {
            return Err(usage("expected capabilities --format json"));
        }
        return writeln!(output, "{}", capabilities()).map_err(io_error);
    }
    if !matches!(command, "inspect" | "check" | "build") {
        return Err(usage("unknown command"));
    }
    let mut file = None;
    let mut encoding = None;
    let mut include_dirs = Vec::new();
    let mut stage = None;
    let mut limit = None;
    let mut target = None;
    let mut out_dir = None;
    let mut optimization = None;
    let mut emit = None;
    let mut lto = None;
    let mut model = None;
    let mut runtime_cache = None;
    let mut index = 1;
    let mut options = true;
    while index < args.len() {
        let arg = &args[index];
        if options && arg == "--" {
            options = false;
            index += 1;
            continue;
        }
        if options
            && matches!(
                arg.to_str(),
                Some(
                    "--encoding"
                        | "--include-dir"
                        | "--stage"
                        | "--max-source-bytes"
                        | "--target"
                        | "--out-dir"
                        | "--opt"
                        | "--emit"
                        | "--lto"
                        | "--model"
                        | "--runtime-cache"
                )
            )
        {
            let value = args
                .get(index + 1)
                .and_then(|item| item.to_str())
                .ok_or_else(|| usage("option needs a value"))?;
            match arg.to_str() {
                Some("--runtime-cache") if runtime_cache.is_none() => {
                    runtime_cache = Some(PathBuf::from(value));
                }
                Some("--include-dir") => include_dirs.push(PathBuf::from(value)),
                Some("--encoding") if encoding.is_none() => {
                    encoding =
                        Some(Encoding::parse(value).ok_or_else(|| usage("unknown encoding"))?)
                }
                Some("--stage") if stage.is_none() => stage = Some(value),
                Some("--out-dir") if out_dir.is_none() => out_dir = Some(PathBuf::from(value)),
                Some("--emit") if emit.is_none() => {
                    emit = Some(match value {
                        "exe" | "shared" | "obj" | "static" => value,
                        _ => return Err(usage("expected --emit obj|exe|static|shared")),
                    })
                }
                Some("--model") if model.is_none() => {
                    model = Some(match value {
                        "lifetimes" => Model::Lifetimes,
                        "ownership" => Model::Ownership,
                        _ => return Err(usage("expected --model ownership|lifetimes")),
                    })
                }
                Some("--lto") if lto.is_none() => {
                    lto = Some(match value {
                        "none" => false,
                        "full" => true,
                        _ => return Err(usage("expected --lto none|full")),
                    })
                }
                Some("--opt") if optimization.is_none() => {
                    optimization = Some(match value {
                        "O0" => false,
                        "O2" => true,
                        _ => return Err(usage("expected --opt O0|O2")),
                    })
                }
                Some("--target") if target.is_none() => {
                    target = Some(match value {
                        "macos-x86_64" => llvm::Target::MacX86_64,
                        "macos-arm64" => llvm::Target::MacArm64,
                        "linux-x86_64" => llvm::Target::LinuxX86_64,
                        "windows-x86_64" => llvm::Target::WindowsX86_64,
                        _ => return Err(usage("unknown scalar LLVM target")),
                    })
                }
                Some("--max-source-bytes") if limit.is_none() => {
                    limit = Some(
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|value| *value > 0)
                            .ok_or_else(|| usage("invalid source byte budget"))?,
                    )
                }
                _ => return Err(usage("duplicate option")),
            }
            index += 2;
        } else if options && arg.to_str().is_some_and(|value| value.starts_with('-')) {
            return Err(usage("unknown option"));
        } else {
            if file.is_some() {
                return Err(usage("expected one source file"));
            }
            file = Some(PathBuf::from(arg));
            index += 1;
        }
    }
    if (command == "inspect"
        && !matches!(
            stage,
            Some(
                "tokens"
                    | "preprocessed"
                    | "ast"
                    | "ir"
                    | "llvm"
                    | "object-init-llvm"
                    | "object-llvm"
            )
        ))
        || (command != "inspect" && stage.is_some())
    {
        return Err(usage(
            "only inspect --stage tokens|preprocessed|ast|ir|llvm|object-init-llvm|object-llvm is implemented",
        ));
    }
    if target.is_some() && !(command == "inspect" && matches!(stage, Some("llvm" | "object-llvm")))
    {
        return Err(usage("--target currently requires inspect --stage llvm"));
    }
    if command != "build"
        && (out_dir.is_some()
            || optimization.is_some()
            || emit.is_some()
            || lto.is_some()
            || runtime_cache.is_some())
    {
        return Err(usage("build options require build"));
    }
    let path = file.ok_or_else(|| usage("missing source file"))?;
    let source = Source::read_declared(
        &path,
        encoding.unwrap_or(Encoding::Auto),
        limit.unwrap_or(16 * 1024 * 1024),
    )
    .map_err(|error| {
        (
            1,
            format!(
                "{}:byte {}: {}: {}",
                path.display(),
                error.byte,
                error.code,
                error.message
            ),
        )
    })?;
    let preprocessed = if stage == Some("preprocessed")
        || !include_dirs.is_empty()
        || zeb_frontend::preprocess::has_directives(&source)
    {
        Some(
            zeb_frontend::preprocess::read(
                &path,
                &include_dirs,
                encoding.unwrap_or(Encoding::Auto),
                limit.unwrap_or(16 * 1024 * 1024),
            )
            .map_err(|e| {
                if e.line == 0 {
                    return (
                        1,
                        format!(
                            "{}:byte {}: {}: {}",
                            e.path.display(),
                            e.column,
                            e.diagnostic.code,
                            e.diagnostic.message
                        ),
                    );
                }
                (
                    1,
                    format!(
                        "{}:{}:{}: {}: {}",
                        e.path.display(),
                        e.line,
                        e.column,
                        e.diagnostic.code,
                        e.diagnostic.message
                    ),
                )
            })?,
        )
    } else {
        None
    };
    let source = preprocessed.as_ref().map_or(&source, |p| &p.source);
    if stage == Some("preprocessed") {
        return output.write_all(source.original_bytes()).map_err(io_error);
    }
    let diagnostic = |error: Diagnostic| {
        if let Some(p) = &preprocessed {
            let (path, line, column) = p.location(error.byte);
            return (
                1,
                format!(
                    "{}:{line}:{column}: {}: {}",
                    path.display(),
                    error.code,
                    error.message
                ),
            );
        }

        let (line, column) = source.location(error.byte);
        (
            1,
            format!(
                "{}:{line}:{column}: {}: {}",
                path.display(),
                error.code,
                error.message
            ),
        )
    };
    if command != "inspect"
        || matches!(
            stage,
            Some("ast" | "ir" | "llvm" | "object-init-llvm" | "object-llvm")
        )
    {
        let ast = parser::parse_with(source, model.unwrap_or_default()).map_err(diagnostic)?;
        if stage == Some("object-llvm") {
            let text = llvm::emit_objects(
                &ast,
                match target {
                    Some(t) => t,
                    None => llvm::Target::host().map_err(|e| (1, e))?,
                },
            )
            .map_err(diagnostic)?;
            return write!(output, "{text}").map_err(io_error);
        }
        if stage == Some("object-init-llvm") {
            let text = zeb_frontend::object_init::emit(&ast).map_err(diagnostic)?;
            return write!(output, "{text}").map_err(io_error);
        }
        if command != "inspect" {
            let _checked = flow::check(&ast).map_err(diagnostic)?;
            if command == "check" {
                return Ok(());
            }
            if let Some(out_dir) = out_dir {
                let mode = emit.unwrap_or("exe");
                if lto == Some(true) && (mode != "shared" || optimization != Some(true)) {
                    return Err(usage("full LTO requires --emit shared --opt O2"));
                }
                let object_program = !ast.objects.is_empty()
                    || ast.nodes.iter().any(|n| {
                        matches!(
                            n.syntax,
                            parser::Syntax::Property(..)
                                | parser::Syntax::String(_)
                                | parser::Syntax::Apply(..)
                                | parser::Syntax::LocalVector(_)
                                | parser::Syntax::Function { rest: true, .. }
                        )
                    });
                if runtime_cache.is_some() && !object_program {
                    return Err(usage("--runtime-cache requires an object shared bundle"));
                }
                let result = if object_program {
                    if mode != "shared" || lto == Some(true) {
                        return Err(usage(
                            "object programs currently require --emit shared --lto none",
                        ));
                    }
                    object_build::build(
                        &ast,
                        source,
                        &out_dir,
                        optimization.unwrap_or(false),
                        runtime_cache.as_deref(),
                    )
                } else if mode == "exe" {
                    native_build::build(&ast, source, &out_dir, optimization.unwrap_or(false))
                } else {
                    let format = match mode {
                        "obj" => shared_build::Format::Object,
                        "static" => shared_build::Format::Static,
                        _ => shared_build::Format::Shared,
                    };
                    shared_build::build(
                        &ast,
                        source,
                        &out_dir,
                        optimization.unwrap_or(false),
                        format,
                        lto.unwrap_or(false),
                    )
                };
                return result.map_err(|error| (1, format!("zebc: native-build: {error}")));
            }
            return Err(diagnostic(Diagnostic::new(
                "frontend-unavailable",
                "build requires --out-dir NEW_DIRECTORY (native bundle)",
                0,
            )));
        }
        if stage == Some("llvm") {
            let target = match target {
                Some(t) => t,
                None => llvm::Target::host().map_err(|e| (1, e))?,
            };
            let module = llvm::emit(&ast, target).map_err(diagnostic)?;
            write!(output, "{module}").map_err(io_error)?;
            return Ok(());
        }
        if stage == Some("ir") {
            let program = ir::lower(&ast).map_err(diagnostic)?;
            for (index, function) in program.functions.iter().enumerate() {
                writeln!(
                    output,
                    "function {index} source={:?} slots={} values={}",
                    function.source,
                    function.slots.len(),
                    function.values
                )
                .map_err(io_error)?;
                for (block_index, block) in function.blocks.iter().enumerate() {
                    writeln!(output, "  block {block_index} source={:?}", block.site)
                        .map_err(io_error)?;
                    for instruction in &block.instructions {
                        writeln!(output, "    {instruction:?}").map_err(io_error)?;
                    }
                    writeln!(output, "    {:?}", block.terminator).map_err(io_error)?;
                }
            }
            return Ok(());
        }
        for (index, node) in ast.nodes.iter().enumerate() {
            writeln!(
                output,
                "{index}\t{}..{}\t{:?}",
                node.start, node.end, node.syntax
            )
            .map_err(io_error)?;
        }
        return Ok(());
    }
    let tokens = lexer::lex(source).map_err(diagnostic)?;
    for token in tokens {
        let (line, column) = source.location(token.byte_start);
        write!(
            output,
            "{line}:{column}\t{}..{}\t{:?}\t",
            token.byte_start, token.byte_end, token.kind
        )
        .map_err(io_error)?;
        for ch in token.spelling(source) {
            write!(output, "{ch}").map_err(io_error)?;
        }
        writeln!(output).map_err(io_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod metadata_tests {
    #[test]
    fn capabilities_use_the_emitted_object_profile_without_claiming_qualification() {
        let text = super::capabilities();
        assert!(text.contains(&format!(
            "\"object_shared_profile\":\"{}\"",
            super::object_build::PROFILE
        )));
        assert!(text.contains("\"game_execution\":true"));
        assert!(text.contains("\"qualified_targets\":[]"));
    }
}
