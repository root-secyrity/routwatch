//! Files that leave the machine carry identity; create them like credentials.
//!
//! A dump can contain the serial, the IMEI, client names and Wi-Fi keys. It is
//! written for the owner's eyes, so the file is created 0600 inside a 0700
//! directory and an existing file is never replaced without an explicit
//! `force`. Both promises are tested here, not in the command layer.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Write `bytes` at mode 0600, creating the parent directory at 0700.
/// `force` is the only way to replace an existing file.
pub fn write_private(path: &Path, bytes: &[u8], force: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let mut b = std::fs::DirBuilder::new();
            b.recursive(true).mode(0o700);
            b.create(dir)?;
        }
    }
    let mut opts = OpenOptions::new();
    opts.write(true).mode(0o600);
    if force {
        opts.create(true).truncate(true);
    } else {
        opts.create_new(true);
    }
    let mut file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::AlreadyExists && !force => {
            return Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                "refusing to overwrite an existing file (pass --force)",
            ));
        }
        Err(e) => return Err(e),
    };
    // `mode` applies only on creation; an existing file keeps its bits.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("routwatch-out-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn new_files_are_0600_and_existing_ones_are_refused() {
        let p = tmp("a.txt");
        let _ = std::fs::remove_file(&p);
        write_private(&p, b"one", false).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let err = write_private(&p, b"two", false).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("refusing to overwrite"));
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
        write_private(&p, b"two", true).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn an_existing_0644_file_is_tightened_on_rewrite() {
        let p = tmp("b.txt");
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, b"old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&p, b"new", true).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_missing_parent_is_created_at_0700() {
        let d = tmp("nested");
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("deep").join("c.txt");
        write_private(&p, b"x", false).unwrap();
        assert_eq!(
            std::fs::metadata(d.join("deep"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
