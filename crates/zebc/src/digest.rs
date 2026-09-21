//! Streaming checksums, independent of host command-line utilities.
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub fn file(path: &Path) -> Result<String, String> {
    let mut input = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn sums(dir: &Path, names: &[&str]) -> Result<String, String> {
    let mut result = String::new();
    for name in names {
        result.push_str(&format!("{}  {name}\n", file(&dir.join(name))?));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_digest() {
        use sha2::{Digest, Sha256};
        assert_eq!(
            format!("{:x}", Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
