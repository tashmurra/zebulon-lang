//! Bounded subprocess supervision shared by the compiler and native test harness.
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
const LOG_LIMIT: usize = 1024 * 1024;
/// A symbol table is output we asked for rather than diagnostics that ran away,
/// and it grows with the runtime. The cap still bounds it.
pub const SYMBOL_LIMIT: usize = 64 * 1024 * 1024;

fn reader<R: Read + Send + 'static>(
    pipe: R,
    exceeded: Arc<AtomicBool>,
    limit: usize,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.take((limit + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            exceeded.store(true, Ordering::SeqCst);
        }
        Ok(bytes)
    })
}
#[cfg(not(windows))]
fn kill_group(child: &mut std::process::Child) {
    // The leader has not been reaped while its pipes are open, reserving its PID.
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}
#[cfg(windows)]
fn kill_group(child: &mut Box<dyn process_wrap::std::ChildWrapper>) {
    let _ = child.start_kill();
    let _ = child.wait();
}
pub fn run_env(
    dir: &Path,
    program: &str,
    args: &[&str],
    environment: &[(&str, &str)],
    timeout: Duration,
) -> Result<String, String> {
    run_inner(dir, program, args, timeout, LOG_LIMIT, false, environment)
}
pub fn run(dir: &Path, program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    run_bounded(dir, program, args, timeout, LOG_LIMIT)
}

/// Run a tool whose output is data rather than a log, with its own cap.
pub fn read_stdout(
    dir: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    run_inner(dir, program, args, timeout, LOG_LIMIT, true, &[])
}

pub fn run_bounded(
    dir: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
    limit: usize,
) -> Result<String, String> {
    run_inner(dir, program, args, timeout, limit, false, &[])
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    dir: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
    limit: usize,
    stdout_only: bool,
    environment: &[(&str, &str)],
) -> Result<String, String> {
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("TMPDIR", dir)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in [
        "HOME",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "RUSTUP_TOOLCHAIN",
        "DEVELOPER_DIR",
        "SystemRoot",
        "SYSTEMROOT",
        "WINDIR",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "VCToolsInstallDir",
        "WindowsSdkDir",
        "WindowsSDKVersion",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(environment.iter().copied());
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    let mut command = {
        use process_wrap::std::{CommandWrap, JobObject};
        let mut wrapped = CommandWrap::from(command);
        wrapped.wrap(JobObject);
        wrapped
    };
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    #[cfg(windows)]
    let (out, err) = (child.stdout().take(), child.stderr().take());
    #[cfg(not(windows))]
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = reader(out.expect("piped stdout"), exceeded.clone(), limit);
    let stderr = reader(err.expect("piped stderr"), exceeded.clone(), limit);
    let start = Instant::now();
    let status = loop {
        if exceeded.load(Ordering::SeqCst) || start.elapsed() >= timeout {
            kill_group(&mut child);
            // Killing the process group closes its pipes before joining readers.
            let _ = stdout.join();
            let _ = stderr.join();
            return Err(if exceeded.load(Ordering::SeqCst) {
                "tool output limit exceeded"
            } else {
                "tool deadline exceeded"
            }
            .to_owned());
        }
        if stdout.is_finished() && stderr.is_finished() {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    kill_group(&mut child);
                    return Err(e.to_string());
                }
            }
        }
        thread::sleep(Duration::from_millis(10));
    };
    let out = stdout
        .join()
        .map_err(|_| "stdout reader failed")?
        .map_err(|e| e.to_string())?;
    let err = stderr
        .join()
        .map_err(|_| "stderr reader failed")?
        .map_err(|e| e.to_string())?;
    if exceeded.load(Ordering::SeqCst) {
        return Err("tool output limit exceeded".to_owned());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&err)
    );
    if !status.success() {
        return Err(format!("{program} failed ({status}): {text}"));
    }
    Ok(if stdout_only {
        String::from_utf8_lossy(&out).into_owned()
    } else {
        text
    })
}

// These supervision tests exercise Unix process groups and system utilities.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn discovery_uses_stdout_without_mixing_successful_warnings() {
        let text = read_stdout(
            Path::new("/tmp"),
            "/bin/sh",
            &["-c", "printf /sdk; printf warning >&2"],
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(text, "/sdk");
    }
    #[test]
    fn process_deadline_and_output_limit_are_enforced() {
        let dir = Path::new("/tmp");
        assert!(
            run(dir, "/bin/sleep", &["5"], Duration::from_millis(30))
                .unwrap_err()
                .contains("deadline")
        );
        assert!(
            run(dir, "/usr/bin/yes", &[], Duration::from_secs(2))
                .unwrap_err()
                .contains("output limit")
        );
        assert!(
            run(
                dir,
                "/bin/sh",
                &["-c", "sleep 5 &"],
                Duration::from_millis(40)
            )
            .unwrap_err()
            .contains("deadline")
        );
        assert!(
            run(
                dir,
                "/bin/sh",
                &["-c", "echo expected-error >&2; exit 3"],
                Duration::from_secs(2)
            )
            .unwrap_err()
            .contains("expected-error")
        );
        assert_eq!(
            run(dir, "/bin/echo", &["bounded"], Duration::from_secs(2)).unwrap(),
            "bounded\n"
        );
    }
}

/// Supervise an executable with input and preserve nonzero language outcomes.
/// Files keep large stdin/stdout independent, while the same process-tree cleanup
/// as tool supervision enforces the deadline and output budget.
pub fn capture(
    dir: &Path,
    program: &str,
    args: &[&str],
    input: &str,
    timeout: Duration,
) -> Result<(i32, String, String), String> {
    use std::{fs, sync::atomic::AtomicU64};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let paths = ["stdin", "stdout", "stderr"]
        .map(|name| dir.join(format!("{name}-{}-{id}", std::process::id())));
    fs::write(&paths[0], input).map_err(|e| e.to_string())?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .stdin(fs::File::open(&paths[0]).map_err(|e| e.to_string())?)
        .stdout(fs::File::create(&paths[1]).map_err(|e| e.to_string())?)
        .stderr(fs::File::create(&paths[2]).map_err(|e| e.to_string())?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    let mut command = {
        use process_wrap::std::{CommandWrap, JobObject};
        let mut wrapper = CommandWrap::from(command);
        wrapper.wrap(JobObject);
        wrapper
    };
    let mut child = command.spawn().map_err(|e| format!("{program}: {e}"))?;
    let start = Instant::now();
    let result = loop {
        if start.elapsed() > timeout
            || paths[1..]
                .iter()
                .any(|p| fs::metadata(p).is_ok_and(|m| m.len() > 4 * 1024 * 1024))
        {
            kill_group(&mut child);
            break Err(format!("command exceeded time/output budget: {program}"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                kill_group(&mut child);
                break Err(e.to_string());
            }
        }
    };
    // Kill lingering descendants even if their parent completed normally.
    #[cfg(windows)]
    {
        let _ = child.start_kill();
    }
    let result = result.and_then(|status| {
        Ok((
            status.code().unwrap_or(-1),
            fs::read_to_string(&paths[1]).map_err(|e| e.to_string())?,
            fs::read_to_string(&paths[2]).map_err(|e| e.to_string())?,
        ))
    });
    for path in paths {
        let _ = fs::remove_file(path);
    }
    result
}
