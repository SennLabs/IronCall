//! File and folder names for new workspace items (PLAN.md §4.3).
//!
//! Names are fixed when an item is created; renaming only changes `name:` in the file.
//! Slugs are safe on Windows and Linux: lowercase ASCII `[a-z0-9-]`, at most
//! [`MAX_SLUG_LEN`] characters, never a reserved Windows device name, and unique in
//! their folder ignoring case (Windows and macOS file systems are case-insensitive).

use std::collections::HashSet;
use std::path::Path;

use unicode_normalization::UnicodeNormalization;

/// Longest slug, before any `-2` style suffix.
pub const MAX_SLUG_LEN: usize = 60;

/// Windows' classic `MAX_PATH`. Longer paths break many Windows tools.
pub const MAX_PATH_LEN: usize = 260;

/// Warn when a full path gets this close to [`MAX_PATH_LEN`].
pub const PATH_WARN_LEN: usize = 240;

/// Shortest a slug is cut to when fitting a long path.
const MIN_SLUG_LEN: usize = 8;

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Turns a display name into a file-name slug.
///
/// Unicode is NFC-normalised and transliterated to ASCII (`Café` → `cafe`, `Ünïcödé` →
/// `unicode`); anything else becomes `-`. Empty results become `untitled`, and reserved
/// Windows names get a `-1` suffix (`CON` → `con-1`).
pub fn slugify(name: &str) -> String {
    let normalized: String = name.nfc().collect();
    let ascii = deunicode::deunicode(&normalized).to_ascii_lowercase();

    let mut slug = String::with_capacity(ascii.len());
    let mut pending_dash = false;
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(c);
        } else {
            pending_dash = true;
        }
    }
    truncate(&mut slug, MAX_SLUG_LEN);
    if slug.is_empty() {
        slug.push_str("untitled");
    }
    if is_reserved(&slug) {
        slug.push_str("-1");
    }
    slug
}

/// Whether `stem` is a reserved Windows device name (`con`, `nul`, `com1`, …).
pub fn is_reserved(stem: &str) -> bool {
    RESERVED.contains(&stem.to_ascii_lowercase().as_str())
}

/// A file or folder name that is free in `dir`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueName {
    /// The full name, including `extension` (e.g. `get-user-2.yaml`).
    pub name: String,
    /// Set if the full path is close to or over Windows' `MAX_PATH`.
    pub warning: Option<String>,
}

/// Picks a free name in `dir` for `slug` + `extension` (e.g. `".yaml"`, or `""` for a
/// folder), adding `-2`, `-3`, … on case-insensitive collisions.
///
/// `ignore` is a name to treat as free (the item's own current name, when renaming).
/// If the full path would be too long for Windows, the slug is shortened (down to a
/// minimum) and a warning is returned if it still doesn't fit.
pub fn unique_name(
    dir: &Path,
    slug: &str,
    extension: &str,
    ignore: Option<&str>,
) -> std::io::Result<UniqueName> {
    let mut taken = HashSet::new();
    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().to_lowercase();
            taken.insert(name);
        }
    }
    if let Some(ignore) = ignore {
        taken.remove(&ignore.to_lowercase());
    }

    // Leave room for "/", a "-NN" suffix and the extension.
    let dir_len = dir.to_string_lossy().chars().count();
    let budget = PATH_WARN_LEN.saturating_sub(dir_len + 1 + 4 + extension.len());
    let mut base = slug.to_string();
    if base.len() > budget {
        truncate(&mut base, budget.max(MIN_SLUG_LEN));
        if is_reserved(&base) {
            base.push_str("-1");
        }
    }

    let mut name = format!("{base}{extension}");
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        name = format!("{base}-{n}{extension}");
        n += 1;
    }

    let full_len = dir_len + 1 + name.chars().count();
    let warning = (full_len >= PATH_WARN_LEN).then(|| {
        format!(
            "The path is {full_len} characters long. Windows tools may fail on paths over \
             {MAX_PATH_LEN} characters; consider a shorter folder location."
        )
    });
    Ok(UniqueName { name, warning })
}

/// Cuts an ASCII slug to `max` characters without leaving a trailing `-`.
fn truncate(slug: &mut String, max: usize) {
    if slug.len() > max {
        slug.truncate(max);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_names() {
        assert_eq!(slugify("Create user"), "create-user");
        assert_eq!(slugify("  GET /users/:id  "), "get-users-id");
        assert_eq!(slugify("List   users -- all"), "list-users-all");
        assert_eq!(slugify("v2.0 API"), "v2-0-api");
    }

    #[test]
    fn unicode_is_transliterated_after_nfc() {
        assert_eq!(slugify("Café"), "cafe");
        // Decomposed "é" (e + combining acute) gives the same slug as the composed form.
        assert_eq!(slugify("Cafe\u{301}"), "cafe");
        assert_eq!(slugify("Ünïcödé"), "unicode");
        assert_eq!(slugify("Привет мир"), "privet-mir");
        assert_eq!(slugify("東京"), "dong-jing");
    }

    #[test]
    fn empty_or_symbol_only_becomes_untitled() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("!!! ???"), "untitled");
        assert_eq!(slugify("🚀"), "rocket");
    }

    #[test]
    fn reserved_windows_names_are_blocked() {
        for name in ["CON", "nul", "Com1", "LPT9", "aux", "prn"] {
            let slug = slugify(name);
            assert!(!is_reserved(&slug), "{name} -> {slug}");
            assert!(slug.ends_with("-1"), "{name} -> {slug}");
        }
        assert_eq!(slugify("console"), "console");
        assert_eq!(slugify("com10"), "com10");
    }

    #[test]
    fn length_is_capped_without_trailing_dash() {
        let slug = slugify(&"word ".repeat(40));
        assert!(slug.len() <= MAX_SLUG_LEN);
        assert!(!slug.ends_with('-'));
        let slug = slugify(&format!("{} b", "a".repeat(59)));
        assert_eq!(slug, "a".repeat(59));
    }

    #[test]
    fn no_trailing_dots_or_spaces() {
        for name in ["name.", "name ", "name. .", "..."] {
            let slug = slugify(name);
            assert!(!slug.ends_with('.') && !slug.ends_with(' '), "{slug}");
        }
    }

    #[test]
    fn case_insensitive_collisions_get_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Get-User.yaml"), "").unwrap();
        std::fs::write(dir.path().join("get-user-2.yaml"), "").unwrap();
        let name = unique_name(dir.path(), "get-user", ".yaml", None).unwrap();
        assert_eq!(name.name, "get-user-3.yaml");
        assert!(name.warning.is_none());
    }

    #[test]
    fn folder_names_collide_with_existing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("users")).unwrap();
        assert_eq!(
            unique_name(dir.path(), "users", "", None).unwrap().name,
            "users-2"
        );
    }

    #[test]
    fn own_name_can_be_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.yaml"), "").unwrap();
        assert_eq!(
            unique_name(dir.path(), "a", ".yaml", Some("a.yaml"))
                .unwrap()
                .name,
            "a.yaml"
        );
    }

    #[test]
    fn long_paths_shorten_the_slug_then_warn() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("d".repeat(100)).join("e".repeat(100));
        std::fs::create_dir_all(&deep).unwrap();
        let slug = slugify(&"long name ".repeat(10));
        let name = unique_name(&deep, &slug, ".yaml", None).unwrap();
        assert!(name.name.len() < slug.len() + 5, "slug shortened: {}", name.name);
        assert!(name.name.len() >= MIN_SLUG_LEN);

        let deeper = deep.join("f".repeat(30));
        std::fs::create_dir_all(&deeper).unwrap();
        let name = unique_name(&deeper, &slug, ".yaml", None).unwrap();
        assert!(name.warning.is_some(), "{name:?}");
    }
}
