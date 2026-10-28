//! Small file helpers: atomic, user-only writes.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// Writes `bytes` to `path` atomically (temp file in the same directory, fsync,
/// rename), readable only by the current user: the file is 0600 on unix, and the parent
/// directory (created if needed) is made user-only on every platform, because a
/// workspace's `.ironcall` folder can live anywhere, e.g. on a shared drive.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    if let Err(e) = ironcall_core::perms::restrict_to_user(dir) {
        // The file itself is still 0600 on unix; on Windows it inherits the folder's ACL.
        tracing::warn!(dir = %dir.display(), error = %e, "couldn't make the secrets folder user-only");
    }
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("secrets");
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!(".{file_name}.tmp-{}", u64::from_le_bytes(nonce)));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `rename`, retried briefly on Windows where antivirus or indexers can hold the target.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if cfg!(windows) && attempt < 5 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * attempt));
            }
            Err(e) => return Err(e),
        }
    }
}
