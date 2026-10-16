//! Device keys: the shared core's implementation
//! ([`tallycam_core::device_key`], the same code the app uses), plus key
//! generation for test devices, which needs an entropy source the pure core
//! does without.

use std::path::Path;

pub use tallycam_core::device_key::*;

/// A new random software key.
pub fn generate() -> SoftwareKey {
    loop {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).expect("no system random source");
        // Zero or a value above the curve order is not a key (about 2^-32
        // of the time); draw again.
        if let Some(key) = SoftwareKey::from_secret(&secret) {
            return key;
        }
    }
}

/// The software key in `path` (hex secret), or a new one written there when
/// the file does not exist.
pub fn load_or_create(path: &Path) -> Result<SoftwareKey, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => SoftwareKey::from_hex(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = generate();
            write_secret(path, &(key.to_hex() + "\n"))
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            Ok(key)
        }
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Creates `path` readable by its owner only (0600 on Unix), so other users
/// cannot read the secret and pass as the device. Fails if the file appeared
/// meanwhile rather than overwrite someone else's key.
fn write_secret(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_differ_and_persist() {
        assert_ne!(generate().public_key(), generate().public_key());
        let dir = std::env::temp_dir().join(format!("tallycam-hub-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device.key");
        let _ = std::fs::remove_file(&path);
        let first = load_or_create(&path).unwrap();
        let again = load_or_create(&path).unwrap();
        assert_eq!(first.public_key(), again.public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
