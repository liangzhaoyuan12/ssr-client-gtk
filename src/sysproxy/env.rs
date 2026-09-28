//! Environment-variable backend: writes a managed block of `http_proxy` /
//! `https_proxy` / `all_proxy` exports into the user's shell rc file.
//!
//! This is the option for desktops that are neither KDE nor GNOME-family —
//! instead of asking the desktop for a system proxy it may not have, we
//! configure **new shells** (GOAL §4.1 系统代理的第三种后端).
//!
//! Design rules:
//! - **Managed block** between `BEGIN`/`END` markers: enabling re-renders it
//!   (self-heal after a crash), disabling removes exactly that block, so
//!   everything the user wrote stays byte-identical.
//! - The shell is chosen from `$SHELL` first (a menu-launched GUI knows the
//!   login shell), then by walking the parent-process chain (launched from
//!   a terminal); no shell ⇒ a clear error instead of guessing.
//! - Which rc file is purely a function of the shell: `.bashrc` / `.zshrc` /
//!   `conf.d/ssr-client-gtk.fish` (fish gets its own file, never
//!   `config.fish`).

use std::path::{Path, PathBuf};

use crate::config::path::home_dir;
use crate::error::AppResult;

use super::{Setting, Snapshot, SysProxy};

/// Backend id stored in [`Snapshot::backend`].
pub(crate) const BACKEND: &str = "env";

/// Shells we know how to configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// `$HOME/.bashrc`
    Bash,
    /// `$HOME/.zshrc`
    Zsh,
    /// `$XDG_CONFIG_HOME/fish/conf.d/ssr-client-gtk.fish` (our own file)
    Fish,
}

impl Shell {
    /// Map a shell executable — `/bin/zsh`, `zsh`, or a login shell's
    /// `-zsh` — to a supported shell.
    pub fn from_exe(exe: &str) -> Option<Shell> {
        let name = Path::new(exe)
            .file_name()
            .and_then(|n| n.to_str())?
            .trim_start_matches('-');
        match name {
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            "fish" => Some(Shell::Fish),
            _ => None,
        }
    }

    /// The rc file we manage for this shell.
    pub fn rc_path(&self) -> PathBuf {
        let home = home_dir();
        match self {
            Shell::Bash => home.join(".bashrc"),
            Shell::Zsh => home.join(".zshrc"),
            Shell::Fish => home
                .join(".config")
                .join("fish")
                .join("conf.d")
                .join("ssr-client-gtk.fish"),
        }
    }

    /// `export k="v"` for POSIX shells, `set -gx k v` for fish.
    fn assign(&self, key: &str, value: &str) -> String {
        match self {
            Shell::Fish => format!("set -gx {key} {value}"),
            _ => format!("export {key}=\"{value}\""),
        }
    }
}

/// Which shell's rc file should be edited (see module docs).
///
/// Errors carry a message fit for a toast.
pub fn detect() -> Result<Shell, String> {
    // 1) `$SHELL` — the login shell, available even when the GUI was
    //    started from a menu (no terminal in the process tree at all).
    if let Ok(exe) = std::env::var("SHELL")
        && let Some(shell) = Shell::from_exe(&exe)
    {
        return Ok(shell);
    }
    // 2) Walk the parent-process chain: launched from a terminal, some
    //    ancestor really is the shell that will read the rc file.
    //    Unix-only: the walk reads `/proc/<pid>/{status,cmdline,comm}`
    //    (GOAL §11 A5). Windows has no rc files to configure at all, so it
    //    only ever reaches the error below there.
    #[cfg(unix)]
    {
        let mut pid = std::process::id();
        for _ in 0..64 {
            let Some(ppid) = parent_of(pid) else { break };
            if ppid <= 1 {
                break;
            }
            if let Some(exe) = cmdline_of(ppid)
                && let Some(shell) = Shell::from_exe(&exe)
            {
                return Ok(shell);
            }
            pid = ppid;
        }
    }
    let shell = std::env::var("SHELL").unwrap_or_default();
    Err(format!(
        "cannot work out which shell to configure (SHELL={shell:?}, no shell in the process tree); supported: bash, zsh, fish"
    ))
}

/// `PPid:` from `/proc/<pid>/status`.
#[cfg(unix)]
fn parent_of(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find(|l| l.starts_with("PPid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// argv0 of `/proc/<pid>/cmdline`, falling back to `comm`.
#[cfg(unix)]
fn cmdline_of(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv0 = raw.split(|b| *b == 0).next().filter(|s| !s.is_empty())?;
    let text = String::from_utf8_lossy(argv0).into_owned();
    if !text.is_empty() {
        return Some(text);
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_string())
}

/// Marker lines wrapping the block we own.
pub const BEGIN: &str = "# >>> ssr-client-gtk proxy >>>";
pub const END: &str = "# <<< ssr-client-gtk proxy <<<";

/// `socks5h://` so the **hostname** travels to our SOCKS5 front-end and the
/// routing rules (domain ACL entries, CN lookup) get a chance to decide.
const PROXY_URL: &str = "socks5h://127.0.0.1";
/// Targets that must never be proxied (loopback — including our own SOCKS
/// port — and link-local mDNS).
const NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// Variables exported for both cases: many tools read only the upper-case
/// spelling (`HTTP_PROXY`), most only the lower-case one.
const VARS: [&str; 8] = [
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
];

/// Render the managed block (leading newline included, so removing it puts
/// the file back byte for byte).
fn render(shell: Shell, port: u16) -> String {
    let mut out = format!("\n{BEGIN}\n");
    for var in VARS {
        let value = if var.eq_ignore_ascii_case("no_proxy") {
            NO_PROXY.to_string()
        } else {
            format!("{PROXY_URL}:{port}")
        };
        out.push_str(&shell.assign(var, &value));
        out.push('\n');
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// Remove our block, wherever it sits, leaving every other byte alone.
fn remove_block(text: &str) -> String {
    let Some(begin_at) = text.find(BEGIN) else {
        return text.to_string();
    };
    // Consume the newline that introduced the block, so an append at the
    // end of a file is reversed exactly.
    let start = if begin_at > 0 && text.as_bytes()[begin_at - 1] == b'\n' {
        begin_at - 1
    } else {
        begin_at
    };
    let Some(end_at) = text[begin_at..].find(END) else {
        // Unbalanced markers (hand-edited): leave the text untouched
        // rather than delete something we cannot identify.
        return text.to_string();
    };
    let end_abs = begin_at + end_at;
    let tail = &text[end_abs + END.len()..];
    let end = match tail.find('\n') {
        Some(n) => end_abs + END.len() + n + 1,
        None => text.len(),
    };
    format!("{}{}", &text[..start], &text[end..])
}

/// Write the block into `path`, returning the snapshot that undoes it.
///
/// `path` is a parameter so tests never touch the real rc files.
pub(crate) fn apply(shell: Shell, path: &Path, port: u16) -> AppResult<Snapshot> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let existed = path.exists() && !existing.is_empty();
    let cleaned = remove_block(&existing);
    let content = format!("{cleaned}{}", render(shell, port));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, content)?;
    Ok(Snapshot {
        backend: BACKEND.into(),
        settings: vec![Setting {
            group: BACKEND.into(),
            key: path.display().to_string(),
            // `None` = the file was not there before ⇒ remove it on
            // restore (same convention as the KDE/GNOME backends).
            value: if existed && !cleaned.is_empty() {
                Some(cleaned)
            } else {
                None
            },
        }],
    })
}

/// Detect the shell and write its rc file.
pub(crate) fn enable(sp: &SysProxy, port: u16) -> AppResult<Snapshot> {
    let shell = detect().map_err(crate::error::AppError::SysProxy)?;
    #[cfg(test)]
    if let Some(path) = &sp.env_rc {
        return apply(shell, path, port);
    }
    let _ = sp; // only the test build reads the rc override
    apply(shell, &shell.rc_path(), port)
}

/// Strip our block from every file in the snapshot (see module docs).
pub(crate) fn restore(_sp: &SysProxy, snap: &Snapshot) -> AppResult<()> {
    for setting in &snap.settings {
        let path = Path::new(&setting.key);
        let current = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(crate::error::AppError::SysProxy(format!(
                    "{}: {e}",
                    path.display()
                )));
            }
        };
        if let Some(original) = &setting.value
            && current.starts_with(original)
            && remove_block(&current[original.len()..]).is_empty()
        {
            // Nothing but our block was added → put the original back
            // byte for byte (GOAL §8-9 精确还原).
            if &current != original {
                std::fs::write(path, original)?;
            }
            continue;
        }
        let cleaned = remove_block(&current);
        if cleaned == current {
            continue; // block already gone (hand-removed or never written)
        }
        if setting.value.is_none() && cleaned.trim().is_empty() {
            // We created the file and the user added nothing to it.
            std::fs::remove_file(path)?;
            continue;
        }
        // Otherwise: drop only our block. When nothing else changed this
        // reproduces the original file byte for byte; when the user edited
        // their rc while the proxy was on, their edits survive.
        std::fs::write(path, cleaned)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpfile(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ssr-env-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn shell_mapping_and_rc_paths() {
        assert_eq!(Shell::from_exe("/bin/bash"), Some(Shell::Bash));
        assert_eq!(Shell::from_exe("zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::from_exe("-zsh"), Some(Shell::Zsh)); // login shell
        assert_eq!(Shell::from_exe("/usr/bin/fish"), Some(Shell::Fish));
        assert_eq!(Shell::from_exe("/usr/bin/tcsh"), None);
        assert_eq!(Shell::from_exe(""), None);

        assert!(Shell::Bash.rc_path().ends_with(".bashrc"));
        assert!(Shell::Zsh.rc_path().ends_with(".zshrc"));
        assert!(
            Shell::Fish
                .rc_path()
                .to_string_lossy()
                .ends_with("conf.d/ssr-client-gtk.fish")
        );
    }

    #[test]
    fn restore_is_byte_identical_on_an_existing_rc() {
        let path = tmpfile("bashrc-existing");
        let original = "# my rc\nalias ll='ls -l'\n";
        fs::write(&path, original).unwrap();

        let snap = apply(Shell::Bash, &path, 1080).unwrap();
        let written = read(&path);
        assert!(written.starts_with(original), "original part preserved");
        assert!(written.contains(BEGIN) && written.contains(END));
        assert!(
            written.contains("export http_proxy=\"socks5h://127.0.0.1:1080\""),
            "exports present: {written}"
        );
        assert!(written.contains("export NO_PROXY=\"localhost,127.0.0.1,::1\""));
        assert_eq!(snap.backend, BACKEND);
        assert!(snap.settings[0].value.as_deref() == Some(original));

        restore(&SysProxy::system(), &snap).unwrap();
        assert_eq!(read(&path), original, "byte-identical restore");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn creates_a_missing_rc_and_removes_it_again() {
        let path = tmpfile("bashrc-missing");
        let _ = fs::remove_file(&path);

        let snap = apply(Shell::Bash, &path, 1080).unwrap();
        assert!(path.exists(), "rc file created");
        assert!(snap.settings[0].value.is_none(), "recorded as absent");

        restore(&SysProxy::system(), &snap).unwrap();
        assert!(!path.exists(), "file we created is removed again");
    }

    #[test]
    fn enabling_twice_does_not_duplicate_the_block() {
        let path = tmpfile("bashrc-twice");
        fs::write(&path, "# rc\n").unwrap();
        apply(Shell::Bash, &path, 1080).unwrap();
        let snap = apply(Shell::Bash, &path, 1080).unwrap();

        let text = read(&path);
        assert_eq!(text.matches(BEGIN).count(), 1, "single block: {text}");
        assert_eq!(text.matches(END).count(), 1);

        restore(&SysProxy::system(), &snap).unwrap();
        assert_eq!(read(&path), "# rc\n");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn user_edits_made_while_the_proxy_was_on_survive() {
        let path = tmpfile("bashrc-edited");
        fs::write(&path, "# rc\n").unwrap();
        let snap = apply(Shell::Bash, &path, 1080).unwrap();

        // The user appends their own line *after* our block.
        let mut text = read(&path);
        text.push_str("export EDITOR=vim\n");
        fs::write(&path, text).unwrap();

        restore(&SysProxy::system(), &snap).unwrap();
        assert_eq!(read(&path), "# rc\nexport EDITOR=vim\n");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn fish_uses_fish_syntax_in_its_own_file() {
        let path = tmpfile("config.fish");
        let snap = apply(Shell::Fish, &path, 1080).unwrap();
        let text = read(&path);
        assert!(
            text.contains("set -gx http_proxy socks5h://127.0.0.1:1080"),
            "{text}"
        );
        assert!(!text.contains("export "), "no bash syntax for fish: {text}");
        restore(&SysProxy::system(), &snap).unwrap();
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn remove_block_alone_never_touches_other_lines() {
        // `apply` renders the block with its own leading newline, so a file
        // it produced looks like "a\n\nBEGIN…".
        let text = format!("a\n\n{BEGIN}\nset -gx x 1\n{END}\nb\n");
        assert_eq!(remove_block(&text), "a\nb\n");
        // …and a hand-tightened block (one newline) is still removed without
        // touching anything after it.
        let tight = format!("a\n{BEGIN}\nset -gx x 1\n{END}\nb\n");
        assert_eq!(remove_block(&tight), "ab\n");
        // No markers at all → untouched.
        assert_eq!(remove_block("plain\n"), "plain\n");
        // Unbalanced markers → untouched (never delete blindly).
        assert_eq!(
            remove_block(&format!("x\n{BEGIN}\ny\n")),
            format!("x\n{BEGIN}\ny\n")
        );
        // Block at the very start, no preceding newline.
        assert_eq!(
            remove_block(&format!("{BEGIN}\nx\n{END}\ntail\n")),
            "tail\n"
        );
    }
}
