//! Paths of files referenced by requests (multipart file parts, binary bodies).
//!
//! Request files store paths relative to the workspace root with `/` separators, or
//! absolute paths for files outside it. Everything here is lexical: nothing touches the
//! disk, so it can't be fooled into following symlinks, and it works for files that don't
//! exist yet. The app still checks real access before reading (PLAN.md §9).

use std::path::{Component, Path, PathBuf};

/// Removes `.` and resolves `..` lexically. `..` never goes above the root or prefix.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop normal components; never climb above `/` or `C:\`.
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Resolves a path as written in a request file: absolute paths as they are, relative
/// ones (with `/` separators) against `root`. Returns `None` for a relative path when
/// there is no workspace root (e.g. a scratch request).
pub fn resolve(root: Option<&Path>, written: &str) -> Option<PathBuf> {
    let written_path = Path::new(written);
    if written_path.is_absolute() {
        return Some(normalize(written_path));
    }
    let mut path = root?.to_path_buf();
    for part in written.split(['/', '\\']).filter(|p| !p.is_empty()) {
        path.push(part);
    }
    Some(normalize(&path))
}

/// True if `path` is `root` or inside it, after lexical normalisation. Comparison is
/// case-insensitive on Windows.
pub fn is_inside(root: &Path, path: &Path) -> bool {
    let root = normalize(root);
    let path = normalize(path);
    if cfg!(windows) {
        let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
        lower(&path).starts_with(lower(&root))
    } else {
        path.starts_with(&root)
    }
}

/// How to store a picked file in a request: relative to `root` with `/` if inside it,
/// otherwise absolute.
pub fn to_written(root: Option<&Path>, path: &Path) -> String {
    let path = normalize(path);
    if let Some(root) = root.map(normalize)
        && is_inside(&root, &path)
        && let Ok(rel) = path.strip_prefix(&root)
    {
        return rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
    }
    path.to_string_lossy().into_owned()
}

/// Guesses a MIME type from a file extension. `None` if unknown (callers fall back to
/// `application/octet-stream`).
pub fn guess_content_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "md" => "text/markdown",
        "yaml" | "yml" => "application/yaml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wasm" => "application/wasm",
        "dcm" => "application/dicom",
        "bin" => "application/octet-stream",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    const ROOT: &str = "/ws/project";
    #[cfg(windows)]
    const ROOT: &str = r"C:\ws\project";

    #[test]
    fn normalize_resolves_dots_without_climbing_above_root() {
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
        assert_eq!(normalize(Path::new("../a")), PathBuf::from("../a"));
        #[cfg(unix)]
        {
            assert_eq!(normalize(Path::new("/a/../../b")), PathBuf::from("/b"));
            assert_eq!(
                normalize(Path::new("/ws/project/../../etc/passwd")),
                PathBuf::from("/etc/passwd")
            );
        }
    }

    #[test]
    fn resolve_relative_and_absolute() {
        let root = Path::new(ROOT);
        assert_eq!(
            resolve(Some(root), "files/a.png"),
            Some(root.join("files").join("a.png"))
        );
        assert_eq!(resolve(Some(root), "./files/../b.bin"), Some(root.join("b.bin")));
        assert_eq!(resolve(None, "files/a.png"), None);
        #[cfg(unix)]
        assert_eq!(resolve(None, "/tmp/x.bin"), Some(PathBuf::from("/tmp/x.bin")));
    }

    #[test]
    fn is_inside_rejects_traversal() {
        let root = Path::new(ROOT);
        assert!(is_inside(root, &root.join("a/b.txt")));
        assert!(is_inside(root, root));
        assert!(!is_inside(root, &resolve(Some(root), "../other/x").unwrap()));
        assert!(!is_inside(root, &resolve(Some(root), "a/../../x").unwrap()));
        // A sibling directory sharing a name prefix is outside.
        assert!(!is_inside(root, Path::new(&format!("{ROOT}-evil/x"))));
    }

    #[test]
    fn to_written_is_relative_inside_and_absolute_outside() {
        let root = Path::new(ROOT);
        assert_eq!(
            to_written(Some(root), &root.join("files").join("a.png")),
            "files/a.png"
        );
        #[cfg(unix)]
        {
            assert_eq!(to_written(Some(root), Path::new("/tmp/x.bin")), "/tmp/x.bin");
            assert_eq!(to_written(None, Path::new("/tmp/x.bin")), "/tmp/x.bin");
        }
    }

    #[test]
    fn guesses_common_types() {
        assert_eq!(guess_content_type(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(
            guess_content_type(Path::new("data.json")),
            Some("application/json")
        );
        assert_eq!(guess_content_type(Path::new("noext")), None);
        assert_eq!(guess_content_type(Path::new("a.unknownext")), None);
    }
}
