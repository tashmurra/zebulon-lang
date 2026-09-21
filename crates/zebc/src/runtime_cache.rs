//! Explicit, content-addressed native runtime cache. No cache is used by default.
//! std filesystem operations suffice: complete entries are published by rename,
//! and copies (never hard links) keep bundles independent of cache eviction.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// Length framing prevents two different component lists sharing an identity.
pub fn identity(parts: &[&str]) -> String {
    let mut result = String::from("zeb-runtime-cache-v1\n");
    for part in parts {
        result.push_str(&format!("{}:{part}\n", part.len()));
    }
    result
}

fn checksum(dir: &Path, runtime: &str) -> Result<String, String> {
    let mut names = vec![runtime, "rust-build.txt"];
    let import = format!("{runtime}.lib");
    if runtime.ends_with(".dll") {
        names.push(&import);
    }
    crate::digest::sums(dir, &names)
}

/// An absent entry is a miss. A damaged published entry is an explicit error,
/// never a library handed to the linker. An abandoned temporary entry is ignored.
pub fn restore(entry: &Path, out: &Path, runtime: &str) -> Result<bool, String> {
    match fs::metadata(entry) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("runtime cache {}: {e}", entry.display())),
        Ok(_) => (),
    }
    let read = || -> Result<(), String> {
        let expected = fs::read_to_string(entry.join("SHA256SUMS")).map_err(|e| e.to_string())?;
        if checksum(entry, runtime)? != expected {
            return Err("checksum mismatch".into());
        }
        fs::copy(entry.join(runtime), out.join(runtime)).map_err(|e| e.to_string())?;
        fs::copy(entry.join("rust-build.txt"), out.join("rust-build.txt"))
            .map_err(|e| e.to_string())?;
        if runtime.ends_with(".dll") {
            let import = format!("{runtime}.lib");
            fs::copy(entry.join(&import), out.join(&import)).map_err(|e| e.to_string())?;
        }
        // Validate the copies as well, before the caller links them.
        if checksum(out, runtime)? != expected {
            return Err("copied entry checksum mismatch".into());
        }
        Ok(())
    };
    read().map_err(|e| {
        format!(
            "runtime cache {}: {e}; remove the damaged entry and rebuild",
            entry.display()
        )
    })?;
    Ok(true)
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn publish(entry: &Path, out: &Path, runtime: &str) -> Result<(), String> {
    let parent = entry.parent().ok_or("cache entry needs a parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("runtime cache: {e}"))?;
    let temp = loop {
        let candidate = parent.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => break Temporary(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("runtime cache: {e}")),
        }
    };
    for file in [runtime, "rust-build.txt"] {
        fs::copy(out.join(file), temp.0.join(file)).map_err(|e| e.to_string())?;
    }
    if runtime.ends_with(".dll") {
        let import = format!("{runtime}.lib");
        fs::copy(out.join(&import), temp.0.join(&import)).map_err(|e| e.to_string())?;
    }
    let sums = checksum(&temp.0, runtime)?;
    fs::write(temp.0.join("SHA256SUMS"), &sums).map_err(|e| e.to_string())?;
    // A competing builder may have published the same complete entry. Accept
    // it only if it contains exactly the files this build produced.
    if let Err(error) = fs::rename(&temp.0, entry) {
        let existing = fs::read_to_string(entry.join("SHA256SUMS"));
        if existing.as_deref().ok() != Some(sums.as_str()) || checksum(entry, runtime)? != sums {
            return Err(format!(
                "runtime cache publication {}: {error}",
                entry.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Temporary {
        let path = std::env::temp_dir().join(format!(
            "zeb-cache-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Temporary(path)
    }

    #[test]
    fn cached_artifacts_survive_eviction_and_corruption_is_refused() {
        let root = fixture();
        let out = root.0.join("out");
        let warm = root.0.join("warm");
        let entry = root.0.join("cache/key/x86_64");
        fs::create_dir(&out).unwrap();
        fs::create_dir(&warm).unwrap();
        fs::write(out.join("runtime.dylib"), b"complete library").unwrap();
        fs::write(out.join("rust-build.txt"), b"compiler log").unwrap();
        assert!(!restore(&entry, &warm, "runtime.dylib").unwrap());
        publish(&entry, &out, "runtime.dylib").unwrap();
        publish(&entry, &out, "runtime.dylib").unwrap(); // competing identical writer
        assert!(restore(&entry, &warm, "runtime.dylib").unwrap());
        for file in ["runtime.dylib", "rust-build.txt"] {
            assert_eq!(
                fs::read(out.join(file)).unwrap(),
                fs::read(warm.join(file)).unwrap()
            );
        }
        fs::write(entry.join("runtime.dylib"), b"partial").unwrap();
        assert!(
            restore(&entry, &warm, "runtime.dylib")
                .unwrap_err()
                .contains("checksum mismatch")
        );
        fs::remove_dir_all(&entry).unwrap();
        assert_eq!(
            fs::read(warm.join("runtime.dylib")).unwrap(),
            b"complete library"
        );
        fs::create_dir(entry.parent().unwrap().join(".tmp-interrupted")).unwrap();
        assert!(!restore(&entry, &warm, "runtime.dylib").unwrap());
    }

    #[test]
    fn changed_sources_tools_recipe_and_architecture_miss() {
        let root = fixture();
        let out = root.0.join("out");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("runtime.dylib"), b"library").unwrap();
        fs::write(out.join("rust-build.txt"), b"").unwrap();
        let key = |parts: &[&str]| {
            fs::write(out.join("identity"), identity(parts)).unwrap();
            crate::digest::file(&out.join("identity")).unwrap()
        };
        let original = key(&["source", "tool hash", "recipe O0"]);
        let entry = root.0.join("cache").join(&original).join("x86_64");
        publish(&entry, &out, "runtime.dylib").unwrap();
        assert!(restore(&entry, &out, "runtime.dylib").unwrap());
        for parts in [
            ["changed source", "tool hash", "recipe O0"],
            ["source", "changed tool hash", "recipe O0"],
            ["source", "tool hash", "recipe O2"],
        ] {
            let changed = key(&parts);
            assert_ne!(original, changed);
            assert!(
                !restore(
                    &root.0.join("cache").join(changed).join("x86_64"),
                    &out,
                    "runtime.dylib"
                )
                .unwrap()
            );
        }
        assert!(!restore(&entry.with_file_name("arm64"), &out, "runtime.dylib").unwrap());
        assert_ne!(identity(&["ab", "c"]), identity(&["a", "bc"]));
    }
}
