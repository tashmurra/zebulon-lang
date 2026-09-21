#![forbid(unsafe_code)]

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zebc-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_zebc"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn capabilities_and_unavailable_compilation_are_honest() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("scalar.t"), "main() { return 42; }").unwrap();
    let cap = fixture.run(&["capabilities", "--format", "json"]);
    assert!(cap.status.success());
    assert!(
        String::from_utf8(cap.stdout)
            .unwrap()
            .contains("\"native_compilation\":true")
    );
    assert!(fixture.run(&["check", "scalar.t"]).status.success());
    {
        let out = fixture.run(&["build", "scalar.t"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        assert!(
            String::from_utf8(out.stderr)
                .unwrap()
                .contains("frontend-unavailable")
        );
    }
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[test]
fn lexical_inspection_and_precise_diagnostics() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("good.t"), "main() { return 42; }").unwrap();
    let out = fixture.run(&["inspect", "good.t", "--stage", "tokens"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("Integer(10)\t42")
    );
    // `@` is a template/containment symbol; `$` remains outside the lexicon.
    fs::write(fixture.0.join("bad.t"), "// comment\r\n  $").unwrap();
    let out = fixture.run(&["inspect", "bad.t", "--stage", "tokens"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("bad.t:2:3: lex-character")
    );
    let out = fixture.run(&[
        "inspect",
        "good.t",
        "--stage",
        "tokens",
        "--max-source-bytes",
        "4",
    ]);
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("source-limit")
    );
}

#[test]
fn unknown_and_duplicate_options_are_usage_errors() {
    let fixture = Fixture::new();
    for args in [
        vec!["wat"],
        vec!["check", "a.t", "--runtime-cache", "cache"],
        vec![
            "build",
            "a.t",
            "--runtime-cache",
            "cache",
            "--runtime-cache",
            "other",
        ],
        vec!["inspect", "a.t"],
        vec!["inspect", "a.t", "--stage", "machine"],
        vec!["check", "a.t", "--encoding", "utf-8", "--encoding", "ascii"],
        vec!["build", "a.t", "--opt", "invalid"],
    ] {
        assert_eq!(fixture.run(&args).status.code(), Some(2));
    }
}

#[test]
fn semantic_and_flow_errors_are_reported_before_build() {
    let fixture = Fixture::new();
    for (name, source, expected) in [
        ("overflow.t", "f(){return 2147483647+1;}", "sem-overflow"),
        ("name.t", "f(){return missing;}", "sem-name"),
        ("arity.t", "f(){return g();}g(x){return x;}", "sem-arity"),
        (
            "pending-flow.t",
            "f(){local x;return x;}",
            "flow-uninitialized",
        ),
    ] {
        fs::write(fixture.0.join(name), source).unwrap();
        let result = fixture.run(&["check", name]);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8(result.stderr).unwrap().contains(expected));
    }
    let ast = fixture.run(&["inspect", "overflow.t", "--stage", "ast"]);
    assert!(
        ast.status.success(),
        "syntax inspection must remain separate from semantics"
    );
}

#[test]
fn ir_inspection_can_show_flow_that_check_rejects() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("flow.t"), "f(x){local y;if(x)y=1;return y;}").unwrap();
    let result = fixture.run(&["inspect", "flow.t", "--stage", "ir"]);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("Branch"));
    assert!(output.contains("LogicalGuard"));
    assert!(output.contains("Propagate"));
    assert_eq!(fixture.run(&["check", "flow.t"]).status.code(), Some(1));
}

#[test]
fn llvm_inspection_targets_are_explicit_and_build_requires_output() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("native.t"), "f(x){return x+1;}").unwrap();
    for (target, triple) in [
        ("macos-x86_64", "x86_64-apple-macosx14.0.0"),
        ("macos-arm64", "arm64-apple-macosx14.0.0"),
        ("linux-x86_64", "x86_64-unknown-linux-gnu"),
        ("windows-x86_64", "x86_64-pc-windows-msvc"),
    ] {
        // The scalar profile belongs to the ownership model: under lifetimes
        // `+` is polymorphic and the runtime decides by tag, which the scalar
        // profile cannot express.
        let output = fixture.run(&[
            "inspect",
            "native.t",
            "--stage",
            "llvm",
            "--target",
            target,
            "--model",
            "ownership",
        ]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n");
        assert!(text.contains(triple));
        assert!(text.contains("define internal %out @zfn0"));
        assert!(text.contains("ret %out { i64 0, i32 2"));
    }
    assert_eq!(
        fixture
            .run(&[
                "inspect",
                "native.t",
                "--stage",
                "ast",
                "--target",
                "macos-arm64"
            ])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(fixture.run(&["build", "native.t"]).status.code(), Some(1));
}

#[test]
#[ignore = "requires LLVM 22.1.8 and host native prerequisites"]
fn universal_scalar_build_preserves_existing_outputs() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("game.t"),
        "main(){local x=0;while(x<17){++x;}return x;}",
    )
    .unwrap();
    let result = fixture.run(&["build", "game.t", "--out-dir", "bundle", "--opt", "O2"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let manifest = fs::read(fixture.0.join("bundle/manifest.json")).unwrap();
    assert!(
        String::from_utf8_lossy(&manifest)
            .contains(zeb_frontend::llvm::Target::host().unwrap().bundle_name())
    );
    let status = Command::new(
        fixture.0.join("bundle").join(
            zeb_frontend::llvm::Target::host()
                .unwrap()
                .executable("program"),
        ),
    )
    .current_dir(&fixture.0)
    .status()
    .unwrap();
    assert_eq!(status.code(), Some(17));
    let second = fixture.run(&["build", "game.t", "--out-dir", "bundle"]);
    assert_eq!(second.status.code(), Some(1));
    assert_eq!(
        fs::read(fixture.0.join("bundle/manifest.json")).unwrap(),
        manifest
    );
    fs::write(fixture.0.join("bad.t"), "main(){local x;return x;}").unwrap();
    assert_eq!(
        fixture
            .run(&["build", "bad.t", "--out-dir", "bad-bundle"])
            .status
            .code(),
        Some(1)
    );
    assert!(!fixture.0.join("bad-bundle").exists());
}

#[test]
#[ignore = "requires installed Rust/LLVM and Apple SDK"]
fn shared_scalar_bundle_runs_consumer_and_reports_source_error() {
    let fixture = Fixture::new();
    for (name, source, expected, status, opt) in [
        ("ok", "main(){local x=40;return x+2;}", "42\n", 0, "O2"),
        ("negative", "main(){local x=7;return -x;}", "-7\n", 0, "O0"),
        ("nil", "main(){return nil;}", "nil\n", 0, "O2"),
        ("true", "main(){return true;}", "true\n", 0, "O2"),
        (
            "minimum",
            "main(){return -2147483648;}",
            "-2147483648\n",
            0,
            "O2",
        ),
        (
            "type-error",
            "f(x){return x+1;}main(){return f(true);}",
            "error:1 byte:12\n",
            70,
            "O2",
        ),
        (
            "divisor-error",
            "f(x){return 1/x;}main(){return f(0);}",
            "error:3 byte:12\n",
            70,
            "O2",
        ),
        (
            "shift-error",
            "f(x){return 1<<x;}main(){return f(32);}",
            "error:4 byte:12\n",
            70,
            "O2",
        ),
        (
            "error",
            "f(x){return x+1;}main(){return f(2147483647);}",
            "error:2 byte:12\n",
            70,
            "O2",
        ),
    ] {
        fs::write(fixture.0.join(format!("{name}.t")), source).unwrap();
        let result = fixture.run(&[
            "build",
            &format!("{name}.t"),
            "--emit",
            "shared",
            "--out-dir",
            name,
            "--opt",
            opt,
        ]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = Command::new(
            fixture.0.join(name).join(
                zeb_frontend::llvm::Target::host()
                    .unwrap()
                    .executable("consumer"),
            ),
        )
        .current_dir(&fixture.0)
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(status));
        assert_eq!(
            String::from_utf8(if status == 0 {
                output.stdout
            } else {
                output.stderr
            })
            .unwrap(),
            expected
        );
        let manifest = fs::read_to_string(fixture.0.join(name).join("manifest.json")).unwrap();
        assert!(manifest.contains("scalar-shared-v1"));
        assert!(manifest.contains("\"adventure_game\":false"));
        assert_eq!(
            fixture
                .run(&[
                    "build",
                    &format!("{name}.t"),
                    "--emit",
                    "shared",
                    "--out-dir",
                    name
                ])
                .status
                .code(),
            Some(1)
        );
    }
}

#[test]
fn object_and_static_options_validate_source_before_reserving_outputs() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("bad.t"), "main(){return missing;}").unwrap();
    for mode in ["obj", "static"] {
        let output = fixture.run(&["build", "bad.t", "--emit", mode, "--out-dir", mode]);
        assert_eq!(output.status.code(), Some(1));
        assert!(!fixture.0.join(mode).exists());
        assert!(
            !String::from_utf8(output.stderr)
                .unwrap()
                .contains("expected --emit")
        );
    }
}

#[test]
fn linkable_entry_rejects_unsupported_arity_before_output() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("four.t"), "main(a,b,c,d){return a;}").unwrap();
    for mode in ["obj", "static", "shared"] {
        let output = fixture.run(&["build", "four.t", "--emit", mode, "--out-dir", mode]);
        assert_eq!(output.status.code(), Some(1));
        assert!(!fixture.0.join(mode).exists());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("zero to three parameters")
        );
    }
}

#[test]
fn full_lto_requires_an_optimized_shared_build_before_output_creation() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("scalar.t"), "main(){return 17;}").unwrap();
    for args in [
        vec!["build", "scalar.t", "--out-dir", "out", "--lto", "full"],
        vec![
            "build",
            "scalar.t",
            "--out-dir",
            "out",
            "--emit",
            "shared",
            "--lto",
            "full",
        ],
        vec![
            "build",
            "scalar.t",
            "--out-dir",
            "out",
            "--emit",
            "static",
            "--opt",
            "O2",
            "--lto",
            "full",
        ],
        vec!["check", "scalar.t", "--lto", "none"],
        vec!["build", "scalar.t", "--out-dir", "out", "--lto", "thin"],
        vec![
            "build",
            "scalar.t",
            "--out-dir",
            "out",
            "--lto",
            "none",
            "--lto",
            "full",
        ],
    ] {
        assert!(!fixture.run(&args).status.success());
        assert!(!fixture.0.join("out").exists());
    }
}

#[test]
fn object_bundle_options_and_entry_errors_preserve_outputs() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("objects.t"),
        "item: object count=1; main(){return item.count;}",
    )
    .unwrap();
    assert!(fixture.run(&["check", "objects.t"]).status.success());
    for args in [
        vec!["build", "objects.t", "--emit", "exe", "--out-dir", "bundle"],
        vec![
            "build",
            "objects.t",
            "--emit",
            "shared",
            "--opt",
            "O2",
            "--lto",
            "full",
            "--out-dir",
            "bundle",
        ],
    ] {
        assert!(!fixture.run(&args).status.success());
        assert!(!fixture.0.join("bundle").exists());
    }
    fs::write(
        fixture.0.join("objects.t"),
        "item: object; main(arg){return item;}",
    )
    .unwrap();
    assert!(
        !fixture
            .run(&[
                "build",
                "objects.t",
                "--emit",
                "shared",
                "--out-dir",
                "bundle"
            ])
            .status
            .success()
    );
    assert!(!fixture.0.join("bundle").exists());
}

/// A world requires at least one entry; text and action entry points are independent.
#[test]
fn a_lifetimes_bundle_takes_lines_or_actions_or_both() {
    let fixture = Fixture::new();
    let world = "enum token tokWord;\nclass Thing: object name = 'thing';\n";
    // Neither entry point: nothing can ever reach the game.
    fs::write(
        fixture.0.join("mute.t"),
        format!("{world}startup() {{ return nil; }}"),
    )
    .unwrap();
    let mute = fixture.run(&["build", "mute.t", "--out-dir", "mute", "--emit", "shared"]);
    assert_eq!(mute.status.code(), Some(1));
    let said = String::from_utf8(mute.stderr).unwrap();
    assert!(
        said.contains("requires turn(tokens) or act(verb, subjects)"),
        "the refusal should name both entry points: {said}"
    );
    // Entry validation accepts an action-only program before tool discovery.
    fs::write(
        fixture.0.join("acts.t"),
        format!("{world}act(verb, subjects) {{ return nil; }}"),
    )
    .unwrap();
    let acts = fixture.run(&["build", "acts.t", "--out-dir", "acts", "--emit", "shared"]);
    assert!(
        !String::from_utf8(acts.stderr)
            .unwrap()
            .contains("requires turn(tokens)"),
        "a game that only takes actions is a game"
    );
}

#[test]
fn runtime_cache_requires_an_object_shared_build() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("scalar.t"), "main(){return 42;}").unwrap();
    let output = fixture.run(&[
        "build",
        "scalar.t",
        "--emit",
        "shared",
        "--out-dir",
        "bundle",
        "--runtime-cache",
        "cache",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--runtime-cache requires an object shared bundle")
    );
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("bundle").exists());
}
