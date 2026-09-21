#![forbid(unsafe_code)]
//! End-to-end checks use independent inputs and expected outputs.
mod common;
use common::{TempDir, capture};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

fn build(dir: &Path, name: &str, source: &str, format: &str, opt: &str, lto: bool) -> PathBuf {
    let source_path = dir.join(format!("{name}.t"));
    fs::write(&source_path, source).unwrap();
    let bundle = dir.join(name);
    let mut args = vec![
        "build",
        source_path.to_str().unwrap(),
        "--out-dir",
        bundle.to_str().unwrap(),
        "--emit",
        format,
        "--opt",
        opt,
    ];
    if lto {
        args.extend(["--lto", "full"]);
    }
    let result = capture(
        dir,
        env!("CARGO_BIN_EXE_zebc"),
        &args,
        "",
        Duration::from_secs(180),
    );
    assert_eq!(result.0, 0, "{name}: {}{}", result.1, result.2);
    bundle
}
fn result(bundle: &Path, args: &[&str]) -> (i32, String, String) {
    capture(
        bundle,
        bundle.join(common::exe("consumer")).to_str().unwrap(),
        args,
        "",
        Duration::from_secs(15),
    )
}

#[test]
#[ignore = "requires Rust 1.98.1, LLVM 22.1.8 and host native prerequisites"]
fn link_modes_lto_and_independent_consumers() {
    let tmp = TempDir::new("link-modes");
    for format in ["obj", "static", "shared"] {
        let bundle = build(
            &tmp.0,
            format,
            "main(a,b){return a/b;}",
            format,
            "O2",
            false,
        );
        assert_eq!(result(&bundle, &["-7", "3"]), (0, "-2\n".into(), "".into()));
        assert_eq!(
            result(&bundle, &["1", "0"]),
            (70, "".into(), "error:3 byte:17\n".into())
        );
        assert_eq!(
            result(&bundle, &["-2147483648", "-1"]),
            (70, "".into(), "error:2 byte:17\n".into())
        );
    }
    for (name, source, args, expected) in [
        (
            "lto-division",
            "main(a,b){return a/b;}",
            vec!["-7", "3"],
            "-2\n",
        ),
        ("lto-constant", "main(){return 17;}", vec![], "17\n"),
        (
            "lto-recurrence",
            include_str!("../../../tests/native/runtime-input-recurrence.t"),
            vec!["5", "101", "1000"],
            "5\n",
        ),
    ] {
        let bundle = build(&tmp.0, name, source, "shared", "O2", true);
        assert_eq!(result(&bundle, &args), (0, expected.into(), "".into()));
        let manifest = fs::read_to_string(bundle.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"lto\":\"full\""));
        assert!(manifest.contains("linked-lto-object"));
        for arch in common::arches() {
            let slice = bundle.join(arch);
            let ir = fs::read_to_string(slice.join("lto-optimized.ll")).unwrap();
            assert!(
                !ir.lines().any(|l| l.contains("call i32")
                    && (l.contains("classify") || l.contains("call i32 %")))
            );
            assert!(
                !fs::read_to_string(slice.join("lto-dependencies.txt"))
                    .unwrap()
                    .contains("libzeb_runtime_")
            );
            for file in ["game-stack.su", "game-final.o"] {
                assert!(fs::metadata(slice.join(file)).unwrap().len() > 0);
            }
            if common::runs_on_host(arch) {
                assert_eq!(result(&slice, &args), (0, expected.into(), "".into()));
            }
        }
        for line in fs::read_to_string(bundle.join("SHA256SUMS"))
            .unwrap()
            .lines()
        {
            let (digest, name) = line.split_once("  ").unwrap();
            assert_eq!(zebc::digest::file(&bundle.join(name)).unwrap(), digest);
        }
    }
}

#[test]
#[ignore = "requires Rust 1.98.1, LLVM 22.1.8 and host native prerequisites"]
fn world_rollback_and_process_boundary_persistence() {
    let tmp = TempDir::new("world-state");
    let bundle = build(
        &tmp.0,
        "world",
        include_str!("../../../tests/native/world-state.t"),
        "shared",
        "O2",
        false,
    );
    let play = |commands: &str| {
        let answer = capture(
            &bundle,
            bundle.join(common::exe("consumer")).to_str().unwrap(),
            &[],
            commands,
            Duration::from_secs(30),
        );
        assert_eq!(answer.0, 0, "{}", answer.2);
        assert!(answer.2.is_empty(), "{}", answer.2);
        answer.1
    };
    let states = |text: &str| -> Vec<String> {
        text.split("[world ")
            .skip(1)
            .map(|s| s.split(" undo=").next().unwrap().to_owned())
            .collect()
    };
    let baseline = "count=0 box=0 in=1 exits=1/1 ghost=2 vocab=0 vec=1/1 lookup=2 bytes=4";
    let changed = "count=2 box=0 in=1 exits=1/1 ghost=2 vocab=1 vec=9/2 lookup=7 bytes=5";
    let text = play("!0\n!1\n!2\n!3\n!0\n!5\n!2\n!2\n!6\n!0\n");
    assert_eq!(
        states(&text),
        [
            baseline,
            changed,
            baseline,
            baseline,
            baseline,
            &changed.replace("count=2", "count=99"),
            changed,
            baseline,
            baseline,
            baseline
        ]
    );
    assert_eq!(text.matches("[recovered]").count(), 2);
    let save = tmp.0.join("world.zsave");
    let text = play(&format!("!1\n!4\n:save {}\n!0\n", save.display()));
    assert!(text.contains("[save ok code=0]"));
    let saved = changed.replace("ghost=2", "ghost=1");
    assert_eq!(states(&text).last(), Some(&saved));
    let restored = tmp.0.join("restored.zsave");
    let text = play(&format!(
        ":restore {}\n!0\n!2\n!0\n:save {}\n",
        save.display(),
        restored.display()
    ));
    assert!(text.contains("[restore ok code=0]"));
    assert_eq!(states(&text), vec![saved; 3]);
    assert_eq!(text.matches("undo=0]").count(), 3);
    assert_eq!(
        fs::metadata(&save).unwrap().len(),
        fs::metadata(restored).unwrap().len()
    );
    let bytes = fs::read(&save).unwrap();
    let mut incompatible = bytes.clone();
    incompatible[16..48].fill(0);
    for (name, data) in [
        ("truncated", bytes[..bytes.len() - 1].to_vec()),
        ("incompatible", incompatible),
    ] {
        let path = tmp.0.join(name);
        fs::write(&path, data).unwrap();
        let text = play(&format!(":restore {}\n!0\n", path.display()));
        assert!(text.contains("[restore failed code="));
        assert!(text.contains(&format!("{baseline} undo=0]")));
    }
}

#[test]
#[ignore = "requires Rust 1.98.1, LLVM 22.1.8 and host native prerequisites"]
fn scalar_specialization_preserves_values_errors_and_fallbacks() {
    let tmp = TempDir::new("specialization");
    let cases: &[(&str, &str, &str, i32, &[&str])] = &[
        (
            "chain",
            "id(x){return x;}f(x){return id(x);}main(){return f(17);}",
            "17\n",
            0,
            &["call %iout @zsp0(i32", "call %iout @zsp1(i32"],
        ),
        (
            "logical",
            "f(x){if(x)return 17;return 18;}main(){return f(true);}",
            "17\n",
            0,
            &["call %out @zfn0(i64"],
        ),
        (
            "recursive",
            "f(x){if(x==0)return 1;return x*f(x-1);}main(){return f(5);}",
            "120\n",
            0,
            &["call %iout @zsp0(i32"],
        ),
        (
            "special-error",
            "bad(x){return 1/x;}main(){return bad(0);}",
            "error:3 byte:14\n",
            70,
            &["call %iout @zsp0(i32"],
        ),
        (
            "general-error",
            "g(x){if(x==1)return nil;return 1/x;}f(x){return g(x)+1;}main(){return f(0);}",
            "error:3 byte:31\n",
            70,
            &["call %out @zfn0(i64", "call %iout @zsp1(i32"],
        ),
        (
            "overflow",
            "id(x){return x;}f(x){return id(x)+1;}main(){return f(2147483647);}",
            "error:2 byte:28\n",
            70,
            &["call %iout @zsp1(i32"],
        ),
        (
            "type-fallback",
            "id(x){return x;}main(){return id(true)+1;}",
            "error:1 byte:30\n",
            70,
            &["call %out @zfn0(i64"],
        ),
        (
            "nil-fallback",
            "f(x){if(x==0)return nil;return 17;}main(){return f(0);}",
            "nil\n",
            0,
            &["call %out @zfn0(i64"],
        ),
        (
            "range-mask",
            "f(x){local y=x&255;return (y+1)*2;}main(){return f(-1);}",
            "512\n",
            0,
            &["optimized overflow-check"],
        ),
        (
            "range-overflow",
            "f(x){local y=x&65535;return y*y;}main(){return f(-1);}",
            "error:2 byte:28\n",
            70,
            &["missed overflow-check"],
        ),
        (
            "range-type",
            "f(x){return (x&255)+1;}main(){return f(true);}",
            "error:1 byte:13\n",
            70,
            &["optimized overflow-check", "call %out @zfn0(i64"],
        ),
    ];
    for &(name, source, expected, status, markers) in cases {
        let bundle = build(&tmp.0, name, source, "shared", "O2", false);
        let actual = result(&bundle, &[]);
        assert_eq!(actual.0, status, "{name}: {actual:?}");
        assert_eq!(
            if status == 0 { actual.1 } else { actual.2 },
            expected,
            "{name}"
        );
        for arch in common::arches() {
            let ir = fs::read_to_string(bundle.join(arch).join("game.ll")).unwrap();
            for marker in markers {
                assert!(ir.contains(marker), "{name} {arch}: missing {marker}");
            }
        }
    }
}

#[test]
#[ignore = "requires Rust 1.98.1, LLVM 22.1.8 and host native prerequisites"]
fn root_pruning_preserves_recursive_and_error_outcomes() {
    let tmp = TempDir::new("roots");
    type RootCase = (
        &'static str,
        &'static str,
        &'static [usize],
        usize,
        Option<i32>,
    );
    let cases: &[RootCase] = &[
        (
            "island",
            "leaf(){return 17;}orphan(){return orphan();}main(){return leaf();}",
            &[0, 2],
            3,
            Some(17),
        ),
        (
            "mutual",
            "even(n){if(n==0)return 17;return odd(n-1);}odd(n){if(n==0)return 17;return even(n-1);}unused(){return -7;}main(){return even(4);}",
            &[0, 1, 3],
            4,
            Some(17),
        ),
        (
            "error",
            "unused(){return 42;}bad(x){return 1/x;}main(){return bad(0);}",
            &[1, 2],
            3,
            None,
        ),
    ];
    for &(name, source, keep, count, value) in cases {
        for format in ["exe", "shared"] {
            for opt in ["O0", "O2"] {
                let bundle = build(
                    &tmp.0,
                    &format!("{name}-{format}-{opt}"),
                    source,
                    format,
                    opt,
                    false,
                );
                for arch in common::arches() {
                    let path = if format == "exe" {
                        bundle.join(format!("{arch}.ll"))
                    } else {
                        bundle.join(arch).join("game.ll")
                    };
                    let ir = fs::read_to_string(path).unwrap();
                    let actual: Vec<usize> = ir
                        .lines()
                        .filter_map(|line| {
                            line.strip_prefix("define internal %out @zfn")?
                                .split_once('(')?
                                .0
                                .parse()
                                .ok()
                        })
                        .collect();
                    let expected = if opt == "O2" {
                        keep.to_vec()
                    } else {
                        (0..count).collect()
                    };
                    assert_eq!(actual, expected, "{name} {format} {opt} {arch}");
                }
                let program = bundle.join(common::exe(if format == "exe" {
                    "program"
                } else {
                    "consumer"
                }));
                let answer = capture(
                    &bundle,
                    program.to_str().unwrap(),
                    &[],
                    "",
                    Duration::from_secs(10),
                );
                assert_eq!(
                    answer.0,
                    value.map_or(70, |n| if format == "exe" { n } else { 0 })
                );
                if format == "shared" {
                    if let Some(n) = value {
                        assert_eq!(answer.1, format!("{n}\n"));
                    } else {
                        assert_eq!(
                            answer.2,
                            format!("error:3 byte:{}\n", source.find("1/x").unwrap())
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "requires Rust 1.98.1, LLVM 22.1.8 and host native prerequisites"]
fn effect_optimizations_preserve_error_checks_and_native_outcomes() {
    let tmp = TempDir::new("effects");
    let cases = [
        (
            "identity",
            "id(x){return x;}main(){return id(17);}",
            17,
            1,
            0,
        ),
        (
            "frame-write",
            "f(x){local y=x;y=17;return y;}main(){return f(42);}",
            17,
            1,
            0,
        ),
        (
            "argument-error",
            "id(x){return x;}bad(x){return 1/x;}main(){return id(bad(0));}",
            70,
            1,
            1,
        ),
        (
            "caller-error",
            "id(x){return x;}main(){return id(true)+1;}",
            70,
            1,
            0,
        ),
        (
            "transitive-error",
            "bad(x){return 1/x;}f(x){return bad(x);}main(){return f(0);}",
            70,
            0,
            2,
        ),
    ];
    for (name, source, status, removed, retained) in cases {
        for opt in ["O0", "O2"] {
            let bundle = build(&tmp.0, &format!("{name}-{opt}"), source, "exe", opt, false);
            for arch in common::arches() {
                let ir = fs::read_to_string(bundle.join(format!("{arch}.ll"))).unwrap();
                let bodies = ir
                    .split("define internal %out @zfn")
                    .skip(1)
                    .map(|body| body.split("\n}").next().unwrap())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert_eq!(
                    bodies.matches("; optimized call-error-check").count(),
                    if opt == "O2" { removed } else { 0 }
                );
                assert_eq!(
                    bodies.matches("label %callerr").count(),
                    if opt == "O2" {
                        retained
                    } else {
                        retained + removed
                    }
                );
            }
            let answer = capture(
                &bundle,
                bundle.join(common::exe("program")).to_str().unwrap(),
                &[],
                "",
                Duration::from_secs(10),
            );
            assert_eq!(answer.0, status, "{name} {opt}: {answer:?}");
        }
    }
}

#[test]
#[ignore = "requires Rust 1.98.1, LLVM 22.1.8, Python 3 and host native prerequisites"]
fn published_examples_work_from_an_independent_host_directory() {
    let tmp = TempDir::new("examples");
    let scalar = build(
        &tmp.0,
        "scalar",
        &fs::read_to_string(common::root().join("examples/scalar.t")).unwrap(),
        "shared",
        "O2",
        false,
    );
    assert_eq!(result(&scalar, &["20"]), (0, "41\n".into(), "".into()));
    let python = std::env::var("PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    });
    let answer = capture(
        &tmp.0,
        &python,
        &[
            common::root()
                .join("examples/run_embedding.py")
                .to_str()
                .unwrap(),
            scalar.to_str().unwrap(),
        ],
        "",
        Duration::from_secs(60),
    );
    assert_eq!(answer.0, 0, "{}{}", answer.1, answer.2);
    assert!(answer.1.ends_with("41\n"));
    let world = build(
        &tmp.0,
        "world",
        &fs::read_to_string(common::root().join("examples/world.t")).unwrap(),
        "shared",
        "O2",
        false,
    );
    let answer = capture(
        &world,
        world.join(common::exe("consumer")).to_str().unwrap(),
        &[],
        "!1\n!1\n!2\n",
        Duration::from_secs(15),
    );
    assert_eq!(answer.0, 0, "{}", answer.2);
    let readings: Vec<_> = answer
        .1
        .lines()
        .filter(|line| line.starts_with("reading="))
        .collect();
    assert_eq!(
        readings,
        [
            "reading=1 onBench=1",
            "reading=2 onBench=1",
            "reading=1 onBench=1"
        ]
    );
}

#[test]
#[ignore = "requires pinned native tools and host development libraries"]
fn copied_compiler_and_relocated_world_bundle_keep_working() {
    let tmp = TempDir::new("portable world paths with spaces");
    let compiler = tmp.0.join(common::exe("copied-zebc"));
    fs::copy(env!("CARGO_BIN_EXE_zebc"), &compiler).unwrap();
    let source = tmp.0.join("world.t");
    fs::write(&source, include_str!("../../../examples/world.t")).unwrap();
    let cache = tmp.0.join("runtime cache");
    for name in ["first", "second"] {
        let output = tmp.0.join(name);
        let result = capture(
            &tmp.0,
            compiler.to_str().unwrap(),
            &[
                "build",
                source.to_str().unwrap(),
                "--emit",
                "shared",
                "--out-dir",
                output.to_str().unwrap(),
                "--runtime-cache",
                cache.to_str().unwrap(),
            ],
            "",
            Duration::from_secs(240),
        );
        assert_eq!(result.0, 0, "{}{}", result.1, result.2);
        assert!(
            result
                .2
                .contains(if name == "first" { "miss" } else { "hit" }),
            "{}",
            result.2
        );
    }
    fs::remove_dir_all(cache).unwrap();
    fs::remove_dir_all(tmp.0.join("first")).unwrap();
    let relocated = tmp.0.join("relocated bundle");
    fs::rename(tmp.0.join("second"), &relocated).unwrap();
    let result = capture(
        &tmp.0,
        relocated.join(common::exe("consumer")).to_str().unwrap(),
        &[],
        "!1\n!1\n!2\n",
        Duration::from_secs(30),
    );
    assert_eq!(result, (0, "Ready. Actions: 1 increments, 2 undoes.\nreading=1 onBench=1\nreading=2 onBench=1\nreading=1 onBench=1\n".into(), "".into()));
    let manifest = fs::read_to_string(relocated.join("manifest.json")).unwrap();
    assert!(manifest.contains(zeb_frontend::llvm::Target::host().unwrap().name()));
    if cfg!(windows) {
        let dlls = fs::read_dir(&relocated)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|s| s == "dll"))
            .collect::<Vec<_>>();
        assert_eq!(dlls.len(), 2);
        for dll in dlls {
            assert!(dll.with_extension("dll.lib").is_file());
        }
    }
}
