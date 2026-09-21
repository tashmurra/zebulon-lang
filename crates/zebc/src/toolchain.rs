//! Native tool discovery and per-build identity. No installation paths are compiled in.
use crate::{manifest::Json, process};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};
use zeb_frontend::llvm::Target;

pub const RUST_VERSION: &str = "1.98.1";
pub const LLVM_VERSION: &str = "22.1.8";
pub const TARGETS: [&str; 2] = ["x86_64-apple-darwin", "aarch64-apple-darwin"];
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub llvm_config: Option<String>,
    pub ld64_lld: Option<String>,
    pub rustc: Option<String>,
    pub sdk: Option<String>,
}
impl Options {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| env::var_os(name))
    }
    fn from_lookup(lookup: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<Self, String> {
        let first = |names: &[&str]| -> Result<Option<String>, String> {
            for name in names {
                if let Some(value) = lookup(name) {
                    let value = value
                        .into_string()
                        .map_err(|_| format!("{name} must be valid UTF-8"))?;
                    if value.is_empty() {
                        return Err(format!("{name} is set but empty"));
                    }
                    return Ok(Some(value));
                }
            }
            Ok(None)
        };
        Ok(Self {
            llvm_config: first(&["ZEB_LLVM_CONFIG", "LLVM_CONFIG"])?,
            ld64_lld: first(&["ZEB_LD64_LLD", "ZEB_LLD"])?,
            rustc: first(&["ZEB_RUSTC", "RUSTC"])?,
            sdk: first(&["SDKROOT"])?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    pub host: Target,
    pub bin: String,
    pub ld64_lld: String,
    pub sdk: String,
    pub rustc: String,
    pub lipo: String,
    pub otool: String,
    pub lock_json: String,
    pub profile_json: String,
}
fn text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("tool path must be UTF-8: {}", path.display()))
}
fn output(dir: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    process::read_stdout(dir, program, args, Duration::from_secs(60)).map(|s| s.trim().to_owned())
}
fn file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!(
            "required toolchain file is missing: {}",
            path.display()
        ));
    }
    Ok(())
}
fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn find_program(
    name: &str,
    dir: &Path,
    search: Option<&std::ffi::OsStr>,
) -> Result<String, String> {
    let expanded = if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let name = expanded.as_str();
    if name.is_empty() {
        return Err("configured executable is empty".into());
    }
    if Path::new(name).components().count() > 1 {
        let path = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            dir.join(name)
        };
        if executable(&path) {
            return text(&path);
        }
    } else if let Some(search) = search {
        for entry in env::split_paths(search) {
            let base = if entry.is_absolute() {
                entry
            } else {
                dir.join(entry)
            };
            let path = base.join(name);
            if executable(&path) {
                return text(&path);
            }
        }
    }
    Err(format!(
        "cannot find executable {name}; set its explicit override or add it to PATH"
    ))
}
fn check_version(label: &str, actual: &str, expected: &str) -> Result<(), String> {
    if actual
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .any(|word| word == expected)
    {
        Ok(())
    } else {
        Err(format!(
            "{label} requires version {expected}, found: {actual}"
        ))
    }
}
// LLD can be packaged separately from LLVM (for example, Homebrew's lld keg).
// An explicit selection, or an existing bundled linker, must validate; never
// silently replace an invalid selection with a different tool on PATH.
fn resolve_ld64_lld(
    dir: &Path,
    bin: &Path,
    selected: Option<&str>,
    search: Option<&std::ffi::OsStr>,
    inputs: &mut Vec<PathBuf>,
) -> Result<String, String> {
    let bundled = bin.join("ld64.lld");
    let linker = if let Some(selected) = selected {
        find_program(selected, dir, search)?
    } else if bundled.try_exists().map_err(|e| e.to_string())? {
        find_program(&text(&bundled)?, dir, search)?
    } else {
        find_program("ld64.lld", dir, search)
            .map_err(|e| format!("{e}; set ZEB_LD64_LLD to the matching LLD executable"))?
    };
    check_version(
        "ld64.lld",
        &output(dir, &linker, &["--version"])?,
        LLVM_VERSION,
    )?;
    inputs.push(PathBuf::from(&linker));
    Ok(linker)
}
#[cfg(test)]
fn host_target(os: &str, arch: &str) -> Result<&'static str, String> {
    Target::for_host(os, arch).map(Target::rust_triple)
}
fn directory_files(path: &Path, suffixes: &[&str]) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_file()
            && suffixes.iter().any(|s| {
                let name = path.to_string_lossy();
                name.ends_with(s) || (*s == ".so" && name.contains(".so."))
            })
        {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}
fn target_libraries(dir: &Path, rustc: &str, target: &str) -> Result<Vec<PathBuf>, String> {
    let path = output(
        dir,
        rustc,
        &["--print", "target-libdir", "--target", target],
    )?;
    let files = directory_files(
        Path::new(&path),
        &[".rlib", ".dylib", ".so", ".dll", ".lib"],
    )
    .map_err(|e| {
        format!(
            "Rust target {target} is unavailable: {e}; install it with rustup target add {target}"
        )
    })?;
    if !files.iter().any(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("libstd-"))
    }) {
        return Err(format!(
            "Rust target {target} has no standard library; run rustup target add {target}"
        ));
    }
    Ok(files)
}
fn fingerprints(_dir: &Path, paths: &[PathBuf]) -> Result<Json, String> {
    let mut result = BTreeMap::new();
    for path in paths {
        result.insert(text(path)?, Json::Str(crate::digest::file(path)?));
    }
    Ok(Json::Obj(result.into_iter().collect()))
}
impl Toolchain {
    pub fn resolve(dir: &Path) -> Result<Self, String> {
        Self::resolve_with(dir, &Options::from_env()?)
    }
    pub fn resolve_with(dir: &Path, options: &Options) -> Result<Self, String> {
        let host = Target::host()?;
        let dir = dir.canonicalize().map_err(|e| e.to_string())?;
        let path = env::var_os("PATH");
        let caller_dir =
            env::current_dir().map_err(|e| format!("cannot resolve working directory: {e}"))?;
        let llvm = find_program(
            options.llvm_config.as_deref().unwrap_or("llvm-config"),
            &caller_dir,
            path.as_deref(),
        )?;
        check_version("LLVM", &output(&dir, &llvm, &["--version"])?, LLVM_VERSION)?;
        let bin = output(&dir, &llvm, &["--bindir"])?;
        if !Path::new(&bin).is_absolute() {
            return Err("llvm-config --bindir must return an absolute path".into());
        }
        let mut inputs = vec![PathBuf::from(&llvm)];
        let mut required = vec![
            "opt",
            "clang",
            "llvm-ar",
            "llvm-nm",
            "llvm-dis",
            "llvm-objdump",
            "llvm-readobj",
        ];
        required.extend(if host.is_macos() {
            vec!["llvm-lipo"]
        } else if host.is_windows() {
            vec!["lld-link"]
        } else {
            vec!["ld.lld"]
        });
        for tool in required {
            let full = Path::new(&bin).join(host.executable(tool));
            if !executable(&full) {
                return Err(format!(
                    "LLVM installation is missing executable {tool}: {}",
                    full.display()
                ));
            }
            check_version(
                tool,
                &output(&dir, &text(&full)?, &["--version"])?,
                LLVM_VERSION,
            )?;
            inputs.push(full);
        }
        let ld64_lld = if host.is_macos() {
            resolve_ld64_lld(
                &caller_dir,
                Path::new(&bin),
                options.ld64_lld.as_deref(),
                path.as_deref(),
                &mut inputs,
            )?
        } else {
            String::new()
        };
        if host.is_windows() {
            inputs.extend(directory_files(Path::new(&bin), &[".dll"])?);
        }
        let libdir = output(&dir, &llvm, &["--libdir"])?;
        inputs.extend(directory_files(
            Path::new(&libdir),
            &[".dylib", ".so", ".dll"],
        )?);
        let selected_rustc = find_program(
            options.rustc.as_deref().unwrap_or("rustc"),
            &caller_dir,
            path.as_deref(),
        )?;
        let rust_version = output(&dir, &selected_rustc, &["-vV"])?;
        check_version(
            "rustc",
            rust_version.lines().next().unwrap_or(""),
            RUST_VERSION,
        )?;
        let sysroot = output(&dir, &selected_rustc, &["--print", "sysroot"])?;
        // Use the selected compiler's real binary rather than a rustup proxy whose
        // selection could change when a build executes in another directory.
        let rustc = text(
            &Path::new(&sysroot)
                .join("bin")
                .join(host.executable("rustc")),
        )?;
        check_version(
            "resolved rustc",
            &output(&dir, &rustc, &["--version"])?,
            RUST_VERSION,
        )?;
        inputs.extend([PathBuf::from(&selected_rustc), PathBuf::from(&rustc)]);
        inputs.extend(directory_files(
            &Path::new(&sysroot).join("lib"),
            &[".dylib", ".so", ".dll"],
        )?);
        if host.is_windows() {
            inputs.extend(directory_files(
                &Path::new(&sysroot).join("bin"),
                &[".dll"],
            )?);
        }
        for target in host.slices() {
            inputs.extend(target_libraries(&dir, &rustc, target.rust_triple())?);
        }
        let sdk = if host.is_macos() {
            match &options.sdk {
                Some(path) => text(
                    &Path::new(path)
                        .canonicalize()
                        .map_err(|e| format!("invalid SDKROOT {path}: {e}"))?,
                )?,
                None => output(
                    &dir,
                    "/usr/bin/xcrun",
                    &["--sdk", "macosx", "--show-sdk-path"],
                )?,
            }
        } else {
            String::new()
        };
        if host.is_macos() {
            for name in ["SDKSettings.json", "usr/lib/libSystem.tbd"] {
                let path = Path::new(&sdk).join(name);
                file(&path)?;
                inputs.push(path);
            }
        }
        let lipo = if host.is_macos() {
            output(&dir, "/usr/bin/xcrun", &["--find", "lipo"])?
        } else {
            String::new()
        };
        let otool = if host.is_macos() {
            output(&dir, "/usr/bin/xcrun", &["--find", "otool"])?
        } else {
            String::new()
        };
        if host.is_macos() {
            inputs.extend([PathBuf::from(&lipo), PathBuf::from(&otool)]);
        }
        let clang = text(&Path::new(&bin).join(host.executable("clang")))?;
        if host == Target::LinuxX86_64 {
            for name in ["crt1.o", "crti.o", "crtn.o", "libc.so", "libgcc_s.so.1"] {
                let path = output(&dir, &clang, &[&format!("--print-file-name={name}")])?;
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(format!(
                        "missing Linux development library {name}; install the host C/C++ development toolchain"
                    ));
                }
                file(&path)?;
                inputs.push(path);
            }
        }
        if host.is_windows() {
            for variable in ["INCLUDE", "LIB"] {
                let value = env::var_os(variable).ok_or_else(|| format!("{variable} is missing; run from an x64 Visual Studio Developer PowerShell with the Windows SDK installed"))?;
                for directory in env::split_paths(&value) {
                    inputs.extend(directory_files(
                        &directory,
                        if variable == "LIB" {
                            &[".lib"]
                        } else {
                            &[".h"]
                        },
                    )?);
                }
            }
        }
        inputs.sort();
        inputs.dedup();
        let lock_json = Json::Obj(vec![
            ("schema".into(), Json::Num(1)),
            ("rustc".into(), Json::Str(rust_version)),
            ("llvm".into(), Json::Str(LLVM_VERSION.into())),
            ("files".into(), fingerprints(&dir, &inputs)?),
        ])
        .render();
        let profile_json = Json::Obj(vec![
            ("host".into(), Json::Str(host.rust_triple().into())),
            ("sdk".into(), Json::Str(sdk.clone())),
            (
                "deployment".into(),
                Json::Str(if host.is_macos() { "14.0" } else { "host" }.into()),
            ),
            (
                "targets".into(),
                Json::Arr(
                    host.slices()
                        .iter()
                        .map(|t| Json::Str(t.rust_triple().into()))
                        .collect(),
                ),
            ),
        ])
        .render();
        Ok(Self {
            host,
            bin,
            ld64_lld,
            sdk,
            rustc,
            lipo,
            otool,
            lock_json,
            profile_json,
        })
    }
    pub fn record(&self, dir: &Path) -> Result<(), String> {
        fs::write(dir.join("native-tool-lock.json"), &self.lock_json).map_err(|e| e.to_string())?;
        fs::write(dir.join("target-profile.json"), &self.profile_json).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lld_environment_alias_has_explicit_precedence() {
        let options = |primary: Option<&str>, alias: Option<&str>| {
            Options::from_lookup(|name| match name {
                "ZEB_LD64_LLD" => primary.map(Into::into),
                "ZEB_LLD" => alias.map(Into::into),
                _ => None,
            })
        };
        assert_eq!(
            options(None, Some("separate/ld64.lld"))
                .unwrap()
                .ld64_lld
                .as_deref(),
            Some("separate/ld64.lld")
        );
        assert_eq!(
            options(Some("primary/ld64.lld"), Some("alias/ld64.lld"))
                .unwrap()
                .ld64_lld
                .as_deref(),
            Some("primary/ld64.lld")
        );
        assert_eq!(
            options(Some("primary/ld64.lld"), Some(""))
                .unwrap()
                .ld64_lld
                .as_deref(),
            Some("primary/ld64.lld")
        );
        assert!(options(None, None).unwrap().ld64_lld.is_none());
        assert_eq!(
            options(Some(""), Some("alias/ld64.lld")).unwrap_err(),
            "ZEB_LD64_LLD is set but empty"
        );
        assert_eq!(
            options(None, Some("")).unwrap_err(),
            "ZEB_LLD is set but empty"
        );
    }
    #[test]
    #[cfg(unix)]
    fn separate_lld_selection_validation_and_fingerprints() {
        use std::os::unix::fs::PermissionsExt;
        let dir = env::temp_dir().join(format!("zeb separate lld {}", std::process::id()));
        let bin = dir.join("llvm bin");
        let lld_bin = dir.join("lld bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&lld_bin).unwrap();
        let bundled = bin.join("ld64.lld");
        let separate = lld_bin.join("ld64.lld");
        let script = |path: &Path, version: &str| {
            fs::write(path, format!("#!/bin/sh\nprintf 'LLD {version}\\n'\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        };
        script(&separate, LLVM_VERSION);
        let options = Options::from_lookup(|name| {
            (name == "ZEB_LLD").then(|| separate.as_os_str().to_owned())
        })
        .unwrap();
        // The configured alias must work with neither a bundled linker nor PATH.
        let mut alias_inputs = Vec::new();
        assert_eq!(
            resolve_ld64_lld(
                &dir,
                &bin,
                options.ld64_lld.as_deref(),
                None,
                &mut alias_inputs
            )
            .unwrap(),
            text(&separate).unwrap()
        );
        assert_eq!(alias_inputs, std::slice::from_ref(&separate));
        let search = env::join_paths([&lld_bin]).unwrap();
        let mut inputs = Vec::new();
        let found = resolve_ld64_lld(&dir, &bin, None, Some(&search), &mut inputs).unwrap();
        assert_eq!(found, text(&separate).unwrap());
        assert_eq!(inputs, std::slice::from_ref(&separate));
        let before = fingerprints(&dir, &inputs).unwrap().render();
        script(&separate, "22.1.8 (rebuilt)");
        assert_ne!(before, fingerprints(&dir, &inputs).unwrap().render());

        // A bundled tool wins over PATH, but the explicit override wins over both.
        script(&bundled, LLVM_VERSION);
        assert_eq!(
            resolve_ld64_lld(&dir, &bin, None, Some(&search), &mut inputs).unwrap(),
            text(&bundled).unwrap()
        );
        assert_eq!(
            resolve_ld64_lld(&dir, &bin, Some("lld bin/ld64.lld"), None, &mut inputs).unwrap(),
            text(&separate).unwrap()
        );
        for selected in [Some("missing/ld64.lld"), Some("")] {
            assert!(resolve_ld64_lld(&dir, &bin, selected, Some(&search), &mut inputs).is_err());
        }
        script(&separate, "22.1.80");
        let error = resolve_ld64_lld(
            &dir,
            &bin,
            Some("lld bin/ld64.lld"),
            Some(&search),
            &mut inputs,
        )
        .unwrap_err();
        assert!(error.contains("requires version 22.1.8"), "{error}");
        script(&bundled, "21.1.8");
        script(&separate, LLVM_VERSION);
        assert!(resolve_ld64_lld(&dir, &bin, None, Some(&search), &mut inputs).is_err());
        fs::remove_file(&bundled).unwrap();
        script(&separate, "21.1.8");
        assert!(resolve_ld64_lld(&dir, &bin, None, Some(&search), &mut inputs).is_err());
        fs::remove_file(&separate).unwrap();
        let error = resolve_ld64_lld(&dir, &bin, None, Some(&search), &mut inputs).unwrap_err();
        assert!(error.contains("ZEB_LD64_LLD"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn exact_versions_only() {
        assert!(check_version("clang", "clang version 22.1.8 (build)", LLVM_VERSION).is_ok());
        for value in ["22.1.80", "22.1.8-dev", "21.1.8", ""] {
            assert!(check_version("clang", value, LLVM_VERSION).is_err());
        }
    }
    #[test]
    fn supported_hosts_have_matching_rust_targets() {
        assert_eq!(host_target("macos", "x86_64").unwrap(), TARGETS[0]);
        assert_eq!(host_target("macos", "aarch64").unwrap(), TARGETS[1]);
        assert_eq!(
            host_target("linux", "x86_64").unwrap(),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            host_target("windows", "x86_64").unwrap(),
            "x86_64-pc-windows-msvc"
        );
        assert!(host_target("linux", "aarch64").is_err());
    }
    #[test]
    fn explicit_missing_program_does_not_fall_back() {
        assert!(
            find_program(
                "/missing/zeb-tool",
                Path::new("/"),
                env::var_os("PATH").as_deref()
            )
            .is_err()
        );
        assert!(find_program("", Path::new("/"), env::var_os("PATH").as_deref()).is_err());
    }
    #[test]
    #[cfg(unix)]
    fn executable_paths_with_spaces_are_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let dir = env::temp_dir().join(format!("zeb tools {}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let program = dir.join("tool name");
        fs::write(&program, "#!/bin/sh\nprintf '22.1.8\\n'\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let found = find_program(program.to_str().unwrap(), &dir, None).unwrap();
        assert_eq!(output(&dir, &found, &[]).unwrap(), "22.1.8");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn replacing_an_input_changes_its_fingerprint() {
        let dir = env::temp_dir().join(format!("zeb-tool-digest-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input");
        fs::write(&path, "one").unwrap();
        let first = fingerprints(&dir, std::slice::from_ref(&path))
            .unwrap()
            .render();
        fs::write(&path, "two").unwrap();
        assert_ne!(first, fingerprints(&dir, &[path]).unwrap().render());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn missing_target_libraries_have_installation_guidance() {
        use std::os::unix::fs::PermissionsExt;
        let dir = env::temp_dir().join(format!("zeb-missing-stdlib-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let compiler = dir.join("rustc");
        fs::write(&compiler, "#!/bin/sh\nprintf '%s/missing\\n' \"${0%/*}\"\n").unwrap();
        fs::set_permissions(&compiler, fs::Permissions::from_mode(0o700)).unwrap();
        let error = target_libraries(&dir, compiler.to_str().unwrap(), TARGETS[1]).unwrap_err();
        assert!(
            error.contains("rustup target add aarch64-apple-darwin"),
            "{error}"
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn explicit_invalid_llvm_is_an_error() {
        let options = Options {
            llvm_config: Some("/missing/explicit-llvm-config".into()),
            ..Options::default()
        };
        let error = Toolchain::resolve_with(&env::temp_dir(), &options).unwrap_err();
        assert!(error.contains("/missing/explicit-llvm-config"));
    }
}
