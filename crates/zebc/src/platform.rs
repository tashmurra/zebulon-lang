//! Native format and linker recipes shared by all build drivers.
use crate::{process, toolchain::Toolchain};
use std::{fs, path::Path, time::Duration};
use zeb_frontend::llvm::Target;

impl Toolchain {
    pub fn tool(&self, name: &str) -> String {
        Path::new(&self.bin)
            .join(self.host.executable(name))
            .to_string_lossy()
            .into_owned()
    }
    pub fn clang_args(&self, target: Target) -> Vec<String> {
        let mut args = vec![
            "-target".into(),
            target.triple().into(),
            target.clang_cpu().into(),
        ];
        if target.is_macos() {
            args.extend([
                "-isysroot".into(),
                self.sdk.clone(),
                "-mmacosx-version-min=14.0".into(),
            ]);
        } else if !target.is_windows() {
            args.push("-fPIC".into());
        } else {
            args.extend([
                "-D_CRT_SECURE_NO_WARNINGS".into(),
                "-D_CRT_NONSTDC_NO_WARNINGS".into(),
            ]);
        }
        args
    }
    pub fn linker_args(&self, target: Target) -> Vec<String> {
        if target.is_macos() {
            vec![]
        } else if target.is_windows() {
            // Clang's MSVC driver resolves a linker name through its program
            // search paths; unlike its ELF driver it does not use --ld-path.
            vec!["-fuse-ld=lld".into(), format!("-B{}", self.bin)]
        } else {
            vec![format!("--ld-path={}", self.tool("ld.lld"))]
        }
    }

    pub fn loader_args(&self, target: Target) -> Vec<String> {
        if target.is_macos() {
            vec!["-Wl,-rpath,@loader_path".into()]
        } else if target.is_windows() {
            vec![]
        } else {
            vec!["-Wl,-rpath,$ORIGIN".into()]
        }
    }
    pub fn shared_args(&self, target: Target, name: &str) -> Vec<String> {
        let mut args = self.linker_args(target);
        if target.is_macos() {
            args.extend([
                "-dynamiclib".into(),
                format!("-Wl,-install_name,@rpath/{name}"),
            ]);
        } else {
            args.push("-shared".into());
            if target.is_windows() {
                args.push(format!("-Wl,/implib:{}", self.import_library(name)));
            } else {
                args.push(format!("-Wl,-soname,{name}"));
            }
        }
        args.extend(self.loader_args(target));
        args
    }
    pub fn import_library(&self, name: &str) -> String {
        if self.host.is_windows() && name.ends_with(".dll") {
            format!("{name}.lib")
        } else {
            name.into()
        }
    }
    pub fn rust_args(&self, target: Target, runtime: &str) -> Vec<String> {
        let linker = if target.is_windows() {
            self.tool("lld-link")
        } else {
            self.tool("clang")
        };
        let mut args = vec!["-C".into(), format!("linker={linker}")];
        if target.is_windows() {
            args.extend([
                "-C".into(),
                format!("link-arg=/IMPLIB:{}", self.import_library(runtime)),
            ]);
        } else {
            for flag in self
                .shared_args(target, runtime)
                .into_iter()
                .filter(|x| x != "-dynamiclib" && x != "-shared")
            {
                args.extend(["-C".into(), format!("link-arg={flag}")]);
            }
        }
        args
    }
    pub fn run(&self, dir: &Path, program: &str, args: &[String]) -> Result<String, String> {
        let environment: Vec<(&str, &str)> = if self.host.is_macos() {
            vec![("SDKROOT", &self.sdk), ("MACOSX_DEPLOYMENT_TARGET", "14.0")]
        } else {
            vec![]
        };
        process::run_env(
            dir,
            program,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            &environment,
            Duration::from_secs(120),
        )
    }
    pub fn clang(&self, dir: &Path, target: Target, args: &[String]) -> Result<String, String> {
        let mut full = self.clang_args(target);
        full.extend_from_slice(args);
        self.run(dir, &self.tool("clang"), &full)
    }
    pub fn symbols(&self, dir: &Path, file: &str, demangle: bool) -> Result<String, String> {
        // PE DLLs usually have no COFF symbol table; inspect their import library.
        let file = self.import_library(file);
        let mut args = vec!["--defined-only", "--extern-only"];
        if demangle {
            args.push("--demangle");
        }
        args.push(&file);
        process::run_bounded(
            dir,
            &self.tool("llvm-nm"),
            &args,
            Duration::from_secs(60),
            process::SYMBOL_LIMIT,
        )
    }
    pub fn exports(&self, dir: &Path, file: &str) -> Result<String, String> {
        if self.host.is_windows() && file.ends_with(".dll") {
            let output = process::run_bounded(
                dir,
                &self.tool("llvm-readobj"),
                &["--coff-exports", file],
                Duration::from_secs(60),
                process::SYMBOL_LIMIT,
            )?;
            Ok(output
                .lines()
                .filter_map(|s| s.trim().strip_prefix("Name: "))
                .collect::<Vec<_>>()
                .join("\n"))
        } else {
            let mut args = vec!["--defined-only", "--extern-only"];
            if self.host == Target::LinuxX86_64 && file.ends_with(".so") {
                args.push("--dynamic");
            }
            args.push(file);
            process::run_bounded(
                dir,
                &self.tool("llvm-nm"),
                &args,
                Duration::from_secs(60),
                process::SYMBOL_LIMIT,
            )
        }
    }
    pub fn dependencies(&self, dir: &Path, files: &[&str]) -> Result<String, String> {
        let mut args = if self.host.is_macos() {
            vec!["-L"]
        } else if self.host.is_windows() {
            vec!["--coff-imports"]
        } else {
            vec!["--needed-libs", "--dynamic-table"]
        };
        args.extend_from_slice(files);
        let program = if self.host.is_macos() {
            self.otool.clone()
        } else {
            self.tool("llvm-readobj")
        };
        process::run(dir, &program, &args, Duration::from_secs(60))
    }
    pub fn assemble(&self, out: &Path, files: &[&str]) -> Result<(), String> {
        for file in files {
            if self.host.is_macos() {
                process::run(
                    out,
                    &self.lipo,
                    &[
                        "-create",
                        &format!("x86_64/{file}"),
                        &format!("arm64/{file}"),
                        "-output",
                        file,
                    ],
                    Duration::from_secs(60),
                )?;
                process::run(
                    out,
                    &self.lipo,
                    &[file, "-verify_arch", "x86_64", "arm64"],
                    Duration::from_secs(30),
                )?;
            } else {
                fs::copy(out.join(self.host.arch()).join(file), out.join(file))
                    .map_err(|e| e.to_string())?;
                if self.host.is_windows() && file.ends_with(".dll") {
                    let import = self.import_library(file);
                    fs::copy(out.join(self.host.arch()).join(&import), out.join(import))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }
}

/// Windows exports are explicit; ELF/Mach-O preserve their existing visibility.
pub fn export_ir(mut module: String, target: Target) -> String {
    if target.is_windows() {
        for ty in ["i64", "i32", "void"] {
            module = module.replace(
                &format!("define {ty} @"),
                &format!("define dllexport {ty} @"),
            );
        }
    }
    module
}
