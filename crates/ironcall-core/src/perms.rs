//! User-only folders for private data (PLAN.md §6.3): the app data dir (history,
//! spool, cookies, settings), the logs, and a workspace's `.ironcall` folder when it
//! holds a secrets file.
//!
//! Unix: mode 0700 on the folder (files are also created 0600).
//! Windows: the folder's ACL is replaced by one that grants only the current user and
//! SYSTEM, with inheritance from above turned off; files created inside inherit it.
//! This uses `icacls`, because the workspace forbids `unsafe` and the Win32 security
//! APIs need it. It runs once per folder, not per file.

use std::io;
use std::path::Path;

/// Well-known SID of the local SYSTEM account (backup and antivirus run as SYSTEM).
pub const SYSTEM_SID: &str = "S-1-5-18";

/// Groups whose access is removed explicitly, in case a folder had direct grants:
/// Everyone, Authenticated Users, Users.
pub const BROAD_SIDS: &[&str] = &["S-1-1-0", "S-1-5-11", "S-1-5-32-545"];

/// Makes `dir` (created if missing) accessible only to the current user.
pub fn restrict_to_user(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        windows::restrict(dir)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(())
    }
}

/// `icacls` arguments that leave `dir` with full control for `user_sid` and SYSTEM only.
pub fn icacls_args(dir: &Path, user_sid: &str) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![dir.as_os_str().to_owned(), "/inheritance:r".into()];
    for sid in [user_sid, SYSTEM_SID] {
        args.push("/grant:r".into());
        // (OI)(CI): files and subfolders created inside inherit the grant.
        args.push(format!("*{sid}:(OI)(CI)F").into());
    }
    args.push("/remove:g".into());
    args.extend(BROAD_SIDS.iter().map(|s| format!("*{s}").into()));
    args.push("/Q".into());
    args
}

/// The SID in `whoami /user` output, in table or CSV form. Headers are localized, so
/// this looks for the first `S-1-…` token rather than a column.
pub fn parse_whoami_sid(output: &str) -> Option<String> {
    output
        .split(|c: char| c.is_whitespace() || c == ',' || c == '"')
        .find(|t| {
            t.starts_with("S-1-") && t.len() > 6 && t[4..].chars().all(|c| c.is_ascii_digit() || c == '-')
        })
        .map(str::to_string)
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;

    /// Don't flash a console window from a GUI app.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn run(program: &str, args: &[std::ffi::OsString]) -> io::Result<String> {
        let out = Command::new(program)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "{program} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn user_sid() -> io::Result<String> {
        static SID: OnceLock<Option<String>> = OnceLock::new();
        SID.get_or_init(|| {
            run("whoami", &["/user".into()])
                .ok()
                .and_then(|out| super::parse_whoami_sid(&out))
        })
        .clone()
        .ok_or_else(|| io::Error::other("couldn't find the current user's SID"))
    }

    pub(super) fn restrict(dir: &Path) -> io::Result<()> {
        let sid = user_sid()?;
        run("icacls", &super::icacls_args(dir, &sid)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icacls_arguments_grant_only_the_user_and_system() {
        let args: Vec<String> = icacls_args(
            Path::new(r"C:\Users\ada\AppData\Roaming\x"),
            "S-1-5-21-1-2-3-1001",
        )
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect();
        assert_eq!(
            args,
            [
                r"C:\Users\ada\AppData\Roaming\x",
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-21-1-2-3-1001:(OI)(CI)F",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F",
                "/remove:g",
                "*S-1-1-0",
                "*S-1-5-11",
                "*S-1-5-32-545",
                "/Q",
            ]
        );
    }

    #[test]
    fn whoami_output_parsing() {
        assert_eq!(
            parse_whoami_sid("\"desktop-1\\\\ada\",\"S-1-5-21-111-222-333-1001\"\r\n").as_deref(),
            Some("S-1-5-21-111-222-333-1001")
        );
        let table = "USER INFORMATION\r\n----------------\r\n\r\nUser Name   SID\r\n=========== ===\r\ndesk\\ada   S-1-5-21-0-0-0-1000\r\n";
        assert_eq!(parse_whoami_sid(table).as_deref(), Some("S-1-5-21-0-0-0-1000"));
        assert_eq!(parse_whoami_sid("garbage"), None);
        assert_eq!(parse_whoami_sid(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_folders_become_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("private");
        restrict_to_user(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
