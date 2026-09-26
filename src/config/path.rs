//! Config directory location — same semantics as the old `get_path.rs`:
//! root → `/root/.ssr`, otherwise `$HOME/.ssr`.

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
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Effective-uid check without pulling in `libc`/`nix`: `/proc/<pid>` is
/// owned by the process's effective uid, so `/proc/self` metadata gives us
/// euid 0 → root. Falls back to "not root" when `/proc` is unavailable.
fn is_effective_root() -> bool {
    fs::metadata("/proc/self")
        .map(|md| {
            use std::os::unix::fs::MetadataExt;
            md.uid() == 0
        })
        .unwrap_or(false)
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
}
