#![forbid(unsafe_code)]
mod common;
use std::{fs, path::Path, time::Duration};
use zeb_frontend::{
    llvm::{Target, emit, emit_optimized},
    parser::{self, Model},
    source::{Encoding, Source},
};
fn integer(v: i32) -> u64 {
    (u64::from(v as u32) << 32) | 2
}
fn llvm(source: &str, target: Target) -> String {
    let emit = if std::env::var("ZEB_NATIVE_SOURCE_OPT").as_deref() == Ok("1") {
        emit_optimized
    } else {
        emit
    };
    emit(
        &parser::parse_with(
            &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            Model::Ownership,
        )
        .unwrap(),
        target,
    )
    .unwrap()
}
fn run(dir: &Path, program: &str, args: &[&str]) {
    zebc::process::run(dir, program, args, Duration::from_secs(30))
        .unwrap_or_else(|error| panic!("{program} {args:?}: {error}"));
}
#[test]
fn emission_keeps_checked_outcomes_and_source_coordinates() {
    let text = llvm("f(x){return x/2;}", Target::MacX86_64);
    assert!(text.contains("%out = type { i64, i32, i64 }"));
    assert!(text.contains("sdiv i32"));
    assert!(text.contains("source-byte 12"));
    assert!(!text.contains(" nsw "));
    assert!(!text.contains(" nuw "));
}
#[test]
#[ignore = "requires the installed LLVM 22 candidate, Apple sdk and native macOS host"]
fn native_scalar_cases_o0_o2_and_both_architectures() {
    assert_eq!(std::env::consts::OS, "macos");
    let temporary = common::TempDir::new("llvm_native");
    let dir = temporary.0.clone();
    let tools = common::tools(&dir);
    let bin = tools.bin.as_str();
    let sdk = tools.sdk.as_str();
    // Independent source programs and hand-derived expected outcomes. No VM or IR interpreter.
    let cases = [
        ("add", "f(x){return x+1;}", 0, integer(41), integer(42), 0),
        ("overflow", "f(x){return x+1;}", 0, integer(i32::MAX), 0, 2),
        ("type", "f(x){return x+1;}", 0, 1, 0, 1),
        ("negative", "f(x){return -x;}", 0, integer(i32::MIN), 0, 2),
        ("multiply", "f(x){return x*65536;}", 0, integer(65536), 0, 2),
        (
            "divide",
            "f(x){return x/2;}",
            0,
            integer(-7),
            integer(-3),
            0,
        ),
        ("divzero", "f(x){return 1/x;}", 0, integer(0), 0, 3),
        (
            "divoverflow",
            "f(x){return x/-1;}",
            0,
            integer(i32::MIN),
            0,
            2,
        ),
        (
            "remainder",
            "f(x){return x%-1;}",
            0,
            integer(i32::MIN),
            integer(0),
            0,
        ),
        (
            "signedrem",
            "f(x){return x%2;}",
            0,
            integer(-7),
            integer(-1),
            0,
        ),
        (
            "shift",
            "f(x){return 1<<x;}",
            0,
            integer(31),
            integer(i32::MIN),
            0,
        ),
        ("shiftbad", "f(x){return 1<<x;}", 0, integer(-1), 0, 4),
        ("shift32", "f(x){return 1<<x;}", 0, integer(32), 0, 4),
        (
            "unsigned",
            "f(x){return x>>>1;}",
            0,
            integer(-1),
            integer(i32::MAX),
            0,
        ),
        (
            "signed",
            "f(x){return x>>1;}",
            0,
            integer(-4),
            integer(-2),
            0,
        ),
        ("logical", "f(x){return !x;}", 0, 0, 1, 0),
        ("logicaltype", "f(x){return !x;}", 0, integer(0), 0, 1),
        ("equality", "f(x){return x==true;}", 0, integer(1), 0, 0),
        (
            "short",
            "f(x){return nil && (1/x==1);}",
            0,
            integer(0),
            0,
            0,
        ),
        (
            "conditional",
            "f(x){return x==0 ? 42 : 1/x;}",
            0,
            integer(0),
            integer(42),
            0,
        ),
        (
            "loop",
            "f(x){local sum=0;while(x>0){sum+=x;--x;}return sum;}",
            0,
            integer(10),
            integer(55),
            0,
        ),
        (
            "order",
            "g(a,b){return a*10+b;}f(x){return g(x=1,x=2);}",
            1,
            integer(0),
            integer(12),
            0,
        ),
        (
            "compound",
            "f(x){return x+=(x=2);}",
            0,
            integer(7),
            integer(9),
            0,
        ),
        (
            "postfix",
            "f(x){return x++ + ++x;}",
            0,
            integer(7),
            integer(16),
            0,
        ),
        (
            "propagate",
            "g(x){return x+1;}f(x){return g(x)+1;}",
            1,
            integer(i32::MAX),
            0,
            2,
        ),
        (
            "recursive",
            "f(x){if(x==0)return 1;return x*f(x-1);}",
            0,
            integer(5),
            integer(120),
            0,
        ),
        ("sub", "f(x){return x-3;}", 0, integer(7), integer(4), 0),
        (
            "suboverflow",
            "f(x){return x-1;}",
            0,
            integer(i32::MIN),
            0,
            2,
        ),
        ("negok", "f(x){return -x;}", 0, integer(-7), integer(7), 0),
        (
            "positive",
            "f(x){return +x;}",
            0,
            integer(-7),
            integer(-7),
            0,
        ),
        ("positivetype", "f(x){return +x;}", 0, 1, 0, 1),
        ("bitnot", "f(x){return ~x;}", 0, integer(0), integer(-1), 0),
        ("bitnottype", "f(x){return ~x;}", 0, 0, 0, 1),
        (
            "bitand",
            "f(x){return x&10;}",
            0,
            integer(12),
            integer(8),
            0,
        ),
        (
            "bitor",
            "f(x){return x|10;}",
            0,
            integer(12),
            integer(14),
            0,
        ),
        (
            "bitxor",
            "f(x){return x^10;}",
            0,
            integer(12),
            integer(6),
            0,
        ),
        ("less", "f(x){return x<0;}", 0, integer(-1), 1, 0),
        ("lessfalse", "f(x){return x<0;}", 0, integer(0), 0, 0),
        ("greater", "f(x){return x>0;}", 0, integer(1), 1, 0),
        ("greaterequal", "f(x){return x>=0;}", 0, integer(0), 1, 0),
        ("lessequal", "f(x){return x<=0;}", 0, integer(0), 1, 0),
        ("inequality", "f(x){return x!=true;}", 0, integer(1), 1, 0),
        ("equalequal", "f(x){return x==42;}", 0, integer(42), 1, 0),
        ("orderedtype", "f(x){return x<0;}", 0, 1, 0, 1),
        ("remzero", "f(x){return 1%x;}", 0, integer(0), 0, 3),
        ("rightshiftbad", "f(x){return 1>>x;}", 0, integer(32), 0, 4),
        (
            "unsignedshiftbad",
            "f(x){return 1>>>x;}",
            0,
            integer(-1),
            0,
            4,
        ),
        ("or", "f(x){return true || (1/x==1);}", 0, integer(0), 1, 0),
        ("orselected", "f(x){return nil || x;}", 0, 1, 1, 0),
        ("andselected", "f(x){return true && x;}", 0, 0, 0, 0),
        ("coalescenil", "f(x){return x??42;}", 0, 0, integer(42), 0),
        (
            "coalesceint",
            "f(x){return x??42;}",
            0,
            integer(0),
            integer(0),
            0,
        ),
        (
            "comma",
            "f(x){return (x=2,x+=3);}",
            0,
            integer(0),
            integer(5),
            0,
        ),
        (
            "postdec",
            "f(x){return x-- + x;}",
            0,
            integer(7),
            integer(13),
            0,
        ),
        (
            "predec",
            "f(x){return --x + x;}",
            0,
            integer(7),
            integer(12),
            0,
        ),
        (
            "incrementoverflow",
            "f(x){return ++x;}",
            0,
            integer(i32::MAX),
            0,
            2,
        ),
        (
            "decrementoverflow",
            "f(x){return --x;}",
            0,
            integer(i32::MIN),
            0,
            2,
        ),
        (
            "assignsub",
            "f(x){return x-=3;}",
            0,
            integer(7),
            integer(4),
            0,
        ),
        (
            "assignmul",
            "f(x){return x*=3;}",
            0,
            integer(7),
            integer(21),
            0,
        ),
        (
            "assigndiv",
            "f(x){return x/=3;}",
            0,
            integer(7),
            integer(2),
            0,
        ),
        (
            "assignrem",
            "f(x){return x%=3;}",
            0,
            integer(7),
            integer(1),
            0,
        ),
        (
            "assignand",
            "f(x){return x&=3;}",
            0,
            integer(7),
            integer(3),
            0,
        ),
        (
            "assignor",
            "f(x){return x|=8;}",
            0,
            integer(7),
            integer(15),
            0,
        ),
        (
            "assignxor",
            "f(x){return x^=3;}",
            0,
            integer(7),
            integer(4),
            0,
        ),
        (
            "assignshl",
            "f(x){return x<<=2;}",
            0,
            integer(7),
            integer(28),
            0,
        ),
        (
            "assignshr",
            "f(x){return x>>=2;}",
            0,
            integer(-8),
            integer(-2),
            0,
        ),
        (
            "assignushr",
            "f(x){return x>>>=2;}",
            0,
            integer(-1),
            integer(1073741823),
            0,
        ),
        (
            "nestedbreak",
            "f(x){local y=0;while(x>0){--x;while(true){++y;break;}if(x==2)continue;if(x==1)break;}return y;}",
            0,
            integer(5),
            integer(4),
            0,
        ),
        (
            "extra_greaterzero",
            "f(x){return x>0;}",
            0,
            integer(0),
            0,
            0,
        ),
        (
            "extra_greaternegative",
            "f(x){return x>0;}",
            0,
            integer(-1),
            0,
            0,
        ),
        ("extra_lefalse", "f(x){return x<=0;}", 0, integer(1), 0, 0),
        ("extra_letrue", "f(x){return x<=0;}", 0, integer(-1), 1, 0),
        ("extra_gefalse", "f(x){return x>=0;}", 0, integer(-1), 0, 0),
        ("extra_getrue", "f(x){return x>=0;}", 0, integer(1), 1, 0),
        ("extra_nefalse", "f(x){return x!=true;}", 0, 1, 0, 0),
        ("extra_eqfalse", "f(x){return x==42;}", 0, integer(41), 0, 0),
        ("extra_righttype", "f(x){return 1+x;}", 0, 0, 0, 1),
        (
            "extra_andtype",
            "f(x){return true&&x;}",
            0,
            integer(0),
            0,
            1,
        ),
        ("extra_ortype", "f(x){return nil||x;}", 0, integer(0), 0, 1),
        ("extra_condtype", "f(x){return x?1:2;}", 0, integer(0), 0, 1),
        ("extra_negtype", "f(x){return -x;}", 0, 1, 0, 1),
        ("extra_bitandtype", "f(x){return x&1;}", 0, 1, 0, 1),
        (
            "range_mask",
            "f(x){local y=x&255;return (y+1)*2;}",
            0,
            integer(-1),
            integer(512),
            0,
        ),
        (
            "range_negation",
            "f(x){return -(x&127);}",
            0,
            integer(-1),
            integer(-127),
            0,
        ),
        (
            "range_signed",
            "f(x){return ~(x&127)*-100;}",
            0,
            integer(-1),
            integer(12800),
            0,
        ),
        (
            "range_square",
            "f(x){local y=x&32767;return y*y;}",
            0,
            integer(-1),
            integer(1073676289),
            0,
        ),
        (
            "range_squareoverflow",
            "f(x){local y=x&65535;return y*y;}",
            0,
            integer(-1),
            0,
            2,
        ),
        (
            "range_minneg",
            "f(x){local y=x&0;y-=2147483647;y-=1;return y*-1;}",
            0,
            integer(0),
            0,
            2,
        ),
        (
            "range_loop",
            "f(x){local y=0;while(y<1){y=x;}return y+1;}",
            0,
            integer(i32::MAX),
            0,
            2,
        ),
        (
            "range_join",
            "f(x){local y;if(x)y=0;else y=2147483647;return y+1;}",
            0,
            0,
            0,
            2,
        ),
        ("range_type", "f(x){return x&255;}", 0, 1, 0, 1),
        (
            "range_call",
            "f(x){local y=x&255;return g(y)+2147483647;}g(x){return x;}",
            0,
            integer(1),
            0,
            2,
        ),
    ];
    let prefix = std::env::var("ZEB_NATIVE_CASE_PREFIX").unwrap_or_default();
    let specialized_only = std::env::var("ZEB_NATIVE_SPECIALIZED_ONLY").as_deref() == Ok("1");
    let general_only = std::env::var("ZEB_NATIVE_GENERAL_ONLY").as_deref() == Ok("1");
    assert!(
        !(general_only && specialized_only),
        "conflicting native case selections"
    );
    let integer_entry = |source: &str, entry: usize, arg: u64| {
        if arg as u32 != 2 {
            return false;
        }
        let ast = parser::parse_with(
            &Source::decode(source.as_bytes().to_vec(), Encoding::Utf8).unwrap(),
            Model::Ownership,
        )
        .unwrap();
        let checked = zeb_frontend::flow::check(&ast).unwrap();
        zeb_frontend::specialize::plan(&ast, &checked.program, None).unwrap()[entry]
            .facts
            .is_some()
    };
    if specialized_only {
        assert_eq!(std::env::var("ZEB_NATIVE_SOURCE_OPT").as_deref(), Ok("1"));
    }
    let cases: Vec<_> = cases
        .into_iter()
        .filter(|(name, ..)| name.starts_with(&prefix))
        .filter(|(_, source, entry, arg, ..)| {
            (!specialized_only || integer_entry(source, *entry, *arg))
                && (!general_only || !integer_entry(source, *entry, *arg))
        })
        .collect();
    assert!(!cases.is_empty(), "native case filter selected nothing");
    let count = cases.len();
    let compare: Vec<_> = std::env::var("ZEB_NATIVE_COMPARE_DIRS")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .collect();
    let check_existing = |file: &str, text: &str| {
        let existing = compare
            .iter()
            .map(|dir| dir.join(file))
            .find(|path| path.is_file())
            .expect("missing prior native input");
        assert_eq!(
            fs::read_to_string(existing).unwrap(),
            text,
            "generated LLVM changed: {file}"
        );
    };
    for (name, source, entry, arg, expected, error) in cases {
        for target in [Target::MacX86_64, Target::MacArm64] {
            let triple = target.triple();
            let mut text = llvm(source, target);
            let site = if error == 0 {
                0
            } else {
                source.find("return ").unwrap() + 7
            };
            let specialized_check = if specialized_only {
                let input = (arg >> 32) as u32 as i32;
                format!(
                    "  %sr = call %iout @zsp{entry}(i32 {input})\n  %sv = extractvalue %iout %sr, 0\n  %se = extractvalue %iout %sr, 1\n  %ss = extractvalue %iout %sr, 2\n  %sw = zext i32 %sv to i64\n  %sb = shl i64 %sw, 32\n  %sp = or i64 %sb, 2\n  %success = icmp eq i32 %se, 0\n  %normalized = select i1 %success, i64 %sp, i64 0\n  %svc = icmp eq i64 %normalized, {expected}\n  %sec = icmp eq i32 %se, {error}\n  %ssc = icmp eq i64 %ss, {site}\n  %spe = and i1 %svc, %sec\n  %sok = and i1 %spe, %ssc\n  %bothok = and i1 %ok, %sok\n"
                )
            } else {
                String::new()
            };
            let final_ok = if specialized_only { "%bothok" } else { "%ok" };
            // Test entry stays in the same LLVM module; this introduces no Rust FFI.
            text += &format!(
                "\ndefine i32 @main() {{\n  %r = call %out @zfn{entry}(i64 {arg})\n  %v = extractvalue %out %r, 0\n  %e = extractvalue %out %r, 1\n  %vc = icmp eq i64 %v, {expected}\n  %ec = icmp eq i32 %e, {error}\n  %valueok = and i1 %vc, %ec\n  %site = extractvalue %out %r, 2\n  %siteok = icmp eq i64 %site, {site}\n  %ok = and i1 %valueok, %siteok\n{specialized_check}  %exit = select i1 {final_ok}, i32 0, i32 1\n  ret i32 %exit\n}}\n"
            );
            let file = format!("{name}-{triple}.ll");
            if !compare.is_empty() {
                check_existing(&file, &text);
                continue;
            }
            fs::write(dir.join(&file), text).unwrap();
            run(
                &dir,
                &format!("{bin}/opt"),
                &["-passes=verify", "-disable-output", &file],
            );
            for optimization in ["-O0", "-O2"] {
                let host = matches!(
                    (std::env::consts::ARCH, target),
                    ("x86_64", Target::MacX86_64) | ("aarch64", Target::MacArm64)
                );
                let output = format!(
                    "{name}-{triple}{optimization}{}",
                    if host { "" } else { ".o" }
                );
                let mut args = vec![
                    "-target",
                    triple,
                    target.clang_cpu(),
                    "-isysroot",
                    sdk,
                    "-mmacosx-version-min=14.0",
                    optimization,
                    &file,
                    "-o",
                    &output,
                ];
                if !host {
                    args.push("-c");
                }
                run(&dir, &format!("{bin}/clang"), &args);
                if host {
                    run(&dir, &format!("./{output}"), &[]);
                }
            }
        }
    }
    let host_target = if std::env::consts::ARCH == "aarch64" {
        Target::MacArm64
    } else {
        Target::MacX86_64
    };
    let mut runaway = llvm("f(x){while(true){continue;}}", host_target);
    runaway += "\ndefine i32 @main() {\n  %r = call %out @zfn0(i64 0)\n  ret i32 0\n}\n";
    if !compare.is_empty() {
        check_existing("runaway.ll", &runaway);
        println!(
            "{} matrix LLVM modules and runaway LLVM byte-identical to prior executed inputs",
            count * 2
        );
        return;
    }
    fs::write(dir.join("runaway.ll"), runaway).unwrap();
    run(
        &dir,
        &format!("{bin}/opt"),
        &["-passes=verify", "-disable-output", "runaway.ll"],
    );
    run(
        &dir,
        &format!("{bin}/clang"),
        &[
            "-target",
            host_target.triple(),
            host_target.clang_cpu(),
            "-isysroot",
            sdk,
            "-mmacosx-version-min=14.0",
            "-O0",
            "runaway.ll",
            "-o",
            "runaway",
        ],
    );
    assert!(
        zebc::process::run(&dir, "./runaway", &[], Duration::from_millis(100))
            .unwrap_err()
            .contains("deadline")
    );
    if specialized_only {
        println!("{count} eligible cases directly execute both general and integer entries");
    }
    println!("native-scalar artifacts: {}", dir.display());
    println!(
        "{count} cases: {} host executions; {} other-architecture object builds; {} LLVM verification passes",
        count * 2,
        count * 2,
        count * 2
    );
}
