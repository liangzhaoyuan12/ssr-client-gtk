//! Config directory location — same semantics as the old `get_path.rs`:
//! root → `/root/.ssr`, otherwise `$HOME/.ssr`.

/// `fs` is only touched by the `#[cfg(unix)]` root probe; leaving it
/// unqualified makes it an unused import on Windows, which the packaging
/// scripts' `clippy -- -D warnings` gate turns into a failure there.
#[cfg(unix)]
use std::fs;
use std::path::PathBuf;

/// Directory holding `<cfg_name>.json` files.
pub fn config_dir() -> PathBuf {
    if is_effective_root() {
        PathBuf::from("/root/.ssr")
    } else {
        home_dir().join(".ssr")
    }
}

/// `$HOME` of the current user (mirrors the old `dirs::home_dir` fallback).
///
/// Cross-platform per GOAL §11 B7/D12: `HOME` on Unix, `USERPROFILE` (then
/// `HOMEDRIVE`+`HOMEPATH`) on Windows — the config dir stays `~/.ssr`
/// everywhere. Nothing found → the temp dir (was a hard-coded `/tmp`).
pub fn home_dir() -> PathBuf {
    home_dir_from(|key| std::env::var_os(key))
}

/// Pure lookup so the fallback chain is testable without mutating the real
/// process environment (Phase 8.2 verification).
fn home_dir_from(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(v) = get(key)
            && !v.is_empty()
        {
            return PathBuf::from(v);
        }
    }
    if let (Some(drive), Some(rest)) = (get("HOMEDRIVE"), get("HOMEPATH"))
        && !drive.is_empty()
    {
        let mut p = PathBuf::from(drive);
        p.push(rest);
        return p;
    }
    std::env::temp_dir()
}

/// Effective-uid check without pulling in `libc`/`nix`: `/proc/<pid>` is
/// owned by the process's effective uid, so `/proc/self` metadata gives us
/// euid 0 → root. Falls back to "not root" when `/proc` is unavailable.
///
/// `#[cfg(unix)]` because `std::os::unix::fs::MetadataExt` does not exist on
/// Windows (GOAL §11 A4); macOS has no `/proc`, so the metadata lookup fails
/// and the answer is the same `false`.
#[cfg(unix)]
fn is_effective_root() -> bool {
    fs::metadata("/proc/self")
        .map(|md| {
            use std::os::unix::fs::MetadataExt;
            md.uid() == 0
        })
        .unwrap_or(false)
}

/// No `/proc` and no privilege model to mirror → never "root".
#[cfg(not(unix))]
fn is_effective_root() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_ends_with_dot_ssr() {
        let dir = config_dir();
        assert_eq!(dir.file_name().unwrap(), ".ssr");
        // Non-root test runner → under $HOME, never /root.
        if !is_effective_root() {
            assert!(dir.starts_with(home_dir()));
        }
    }

    /// GOAL §11 B7/D12 (Phase 8.2): HOME wins; a Windows box with only
    /// `USERPROFILE` still lands in a home directory; `HOMEDRIVE`+`HOMEPATH`
    /// is the last resort; nothing set → the temp dir (not a hard `/tmp`).
    #[test]
    fn home_dir_falls_back_across_platforms() {
        let s = |v: &str| Some(std::ffi::OsString::from(v));
        let none = |_: &str| -> Option<std::ffi::OsString> { None };

        assert_eq!(
            home_dir_from(|k| if k == "HOME" { s("/home/me") } else { none(k) }),
            PathBuf::from("/home/me"),
            "HOME wins on Unix"
        );
        assert_eq!(
            home_dir_from(|k| if k == "USERPROFILE" {
                s("C:\\Users\\me")
            } else {
                none(k)
            }),
            PathBuf::from("C:\\Users\\me"),
            "Windows without HOME uses USERPROFILE"
        );
        let drive = home_dir_from(|k| match k {
            "HOMEDRIVE" => s("C:"),
            "HOMEPATH" => s("\\Users\\me"),
            _ => none(k),
        });
        assert!(
            drive.starts_with("C:"),
            "HOMEDRIVE+HOMEPATH must stay on the drive: {drive:?}"
        );
        assert_eq!(home_dir_from(none), std::env::temp_dir());
        // An empty value must not count as "found".
        assert_eq!(
            home_dir_from(|k| if k == "HOME" { s("") } else { none(k) }),
            std::env::temp_dir()
        );
    }
}
