#![allow(dead_code)]
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use zebc::toolchain::Toolchain;
static NEXT: AtomicU64 = AtomicU64::new(0);
static TOOLS: OnceLock<Toolchain> = OnceLock::new();
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
pub struct TempDir(pub PathBuf);
impl TempDir {
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "zeb-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
pub fn tools(dir: &Path) -> &'static Toolchain {
    let tools = TOOLS.get_or_init(|| {
        Toolchain::resolve(dir).unwrap_or_else(|e| panic!("native prerequisites: {e}"))
    });
    tools.record(dir).unwrap();
    tools
}
pub fn runs_on_host(arch: &str) -> bool {
    arch == if std::env::consts::ARCH == "aarch64" {
        "arm64"
    } else {
        "x86_64"
    }
}

/// Supervise a test process using files so output cannot deadlock a pipe.
pub fn capture(
    dir: &Path,
    program: &str,
    args: &[&str],
    input: &str,
    timeout: std::time::Duration,
) -> (i32, String, String) {
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let stdin = dir.join(format!("stdin-{id}"));
    let stdout = dir.join(format!("stdout-{id}"));
    let stderr = dir.join(format!("stderr-{id}"));
    fs::write(&stdin, input).unwrap();
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .stdin(fs::File::open(&stdin).unwrap())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().unwrap_or_else(|e| panic!("{program}: {e}"));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > timeout
            || [&stdout, &stderr]
                .iter()
                .any(|p| fs::metadata(p).unwrap().len() > 4 * 1024 * 1024)
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
            panic!("native command exceeded time/output budget: {program} {args:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    (
        status.code().unwrap_or(-1),
        fs::read_to_string(stdout).unwrap(),
        fs::read_to_string(stderr).unwrap(),
    )
}
