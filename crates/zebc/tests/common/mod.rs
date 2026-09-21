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

pub fn arches() -> Vec<&'static str> {
    zeb_frontend::llvm::Target::host()
        .unwrap()
        .slices()
        .iter()
        .map(|t| t.arch())
        .collect()
}
pub fn exe(name: &str) -> String {
    zeb_frontend::llvm::Target::host().unwrap().executable(name)
}
pub fn capture(
    dir: &Path,
    program: &str,
    args: &[&str],
    input: &str,
    timeout: std::time::Duration,
) -> (i32, String, String) {
    let (code, out, err) = zebc::process::capture(dir, program, args, input, timeout).unwrap();
    (code, out.replace("\r\n", "\n"), err.replace("\r\n", "\n"))
}
