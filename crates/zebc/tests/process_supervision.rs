#![forbid(unsafe_code)]
mod common;
use std::{io::Write, process::Command, time::Duration};

// Executed as a subprocess by the test below. Running the ignored suite without
// the explicit fixture environment is harmless.
#[test]
#[ignore = "subprocess fixture"]
#[allow(clippy::zombie_processes)] // Deliberately orphaned; the supervisor must kill it.
fn process_fixture() {
    match std::env::var("ZEB_PROCESS_FIXTURE").as_deref() {
        Ok("sleep") => std::thread::sleep(Duration::from_secs(5)),
        Ok("flood") => {
            let mut out = std::io::stdout().lock();
            for _ in 0..4096 {
                if out.write_all(&[b'x'; 1024]).is_err() {
                    break;
                }
            }
        }
        Ok("descendant") => {
            let _child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "process_fixture", "--ignored", "--nocapture"])
                .env("ZEB_PROCESS_FIXTURE", "sleep")
                .spawn()
                .unwrap();
            // Exit before the descendant: it retains both inherited pipes.
        }
        Ok("error") => {
            eprintln!("fixture error");
            std::process::exit(3);
        }
        _ => {}
    }
}

#[test]
fn native_process_trees_are_bounded_on_every_host() {
    let temp = common::TempDir::new("process paths with spaces");
    let exe = std::env::current_exe().unwrap();
    for (mode, expected) in [
        ("sleep", "deadline"),
        ("descendant", "deadline"),
        ("flood", "output limit"),
        ("error", "fixture error"),
    ] {
        let error = zebc::process::run_env(
            &temp.0,
            exe.to_str().unwrap(),
            &["--exact", "process_fixture", "--ignored", "--nocapture"],
            &[("ZEB_PROCESS_FIXTURE", mode)],
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(error.contains(expected), "{mode}: {error}");
    }
}
