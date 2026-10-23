//! Named sessions (`haunt attach NAME`): the tabs, layout and pane text saved to
//! disk so they can be restored after haunt exits or the machine crashes.
//!
//! Files live in `$XDG_STATE_HOME/haunt/sessions` (default
//! `~/.local/state/haunt/sessions`): `NAME.json` holds the session,
//! `NAME.lock` holds the pid of the server running it, and `NAME.log` gets
//! that server's error output. The server's socket is in `runtime_dir()`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::layout::Node;
use crate::pane::PaneId;

/// Bump when the file format changes incompatibly.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Session {
    pub version: u32,
    pub active: usize,
    pub tabs: Vec<SavedTab>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedTab {
    pub name: String,
    pub collapsed: bool,
    pub focused: PaneId,
    /// Pane ids in here refer to `panes`; they're reused on restore.
    pub layout: Node,
    pub panes: Vec<SavedPane>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedPane {
    pub id: PaneId,
    /// The shell's working directory when saved.
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
    /// Scrollback + screen as text with ANSI colors (see `snapshot`).
    pub history: String,
}

impl Session {
    pub fn new(active: usize, tabs: Vec<SavedTab>) -> Session {
        Session {
            version: VERSION,
            active,
            tabs,
        }
    }
}

/// Session names become file names, so keep them to a safe character set.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local/state")
        });
    state.join("haunt/sessions")
}

pub fn path(name: &str) -> PathBuf {
    dir().join(format!("{name}.json"))
}

pub fn log_path(name: &str) -> PathBuf {
    dir().join(format!("{name}.log"))
}

/// Where running sessions' sockets live: `$XDG_RUNTIME_DIR/haunt`, or
/// `/tmp/haunt-UID` like tmux. Kept short because socket paths are limited
/// to about 100 bytes, and cleared on reboot along with the servers.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("haunt"),
        None => PathBuf::from(format!("/tmp/haunt-{}", unsafe { libc::getuid() })),
    }
}

/// Create `runtime_dir()` if needed, and make sure only we can use it:
/// anyone who can reach a session's socket can type into its shells.
pub fn private_runtime_dir() -> Result<PathBuf> {
    let dir = runtime_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // symlink_metadata: refuse a symlink someone planted in /tmp.
    let meta = fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        bail!("{} is not a directory owned by you", dir.display());
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

pub fn socket_path(name: &str) -> PathBuf {
    runtime_dir().join(format!("{name}.sock"))
}

/// Load a saved session; `Ok(None)` if there isn't one yet.
pub fn load(path: &Path) -> Result<Option<Session>> {
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let session: Session = serde_json::from_slice(&data)
        .with_context(|| format!("{} is not a valid haunt session", path.display()))?;
    if session.version != VERSION {
        bail!(
            "{} was saved by an incompatible haunt version (format {}, expected {VERSION})",
            path.display(),
            session.version
        );
    }
    Ok(Some(session))
}

/// Write the session atomically: a crash mid-save leaves the previous file
/// intact instead of a half-written one.
pub fn save(path: &Path, session: &Session) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = fs::File::create(&tmp)?;
    serde_json::to_writer(&mut file, session)?;
    file.flush()?;
    // Make sure the data is on disk before the rename makes it visible,
    // otherwise a power loss can leave an empty file behind.
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn delete(path: &Path) {
    let _ = fs::remove_file(path);
}

pub struct Listed {
    pub name: String,
    /// Pid of the server, if the session is running.
    pub running: Option<i32>,
    /// When it was last saved, if it has been.
    pub saved: Option<SystemTime>,
}

/// Running and saved sessions: running first, then most recently saved.
pub fn list() -> Vec<Listed> {
    let Ok(entries) = fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?;
            if ext != "json" && ext != "lock" {
                return None;
            }
            Some(path.file_stem()?.to_str()?.to_string())
        })
        .collect();
    names.sort();
    names.dedup();
    let mut out: Vec<Listed> = names
        .into_iter()
        .map(|name| Listed {
            running: running(&name),
            saved: fs::metadata(path(&name)).and_then(|m| m.modified()).ok(),
            name,
        })
        .filter(|l| l.running.is_some() || l.saved.is_some())
        .collect();
    out.sort_by_key(|l| (l.running.is_none(), std::cmp::Reverse(l.saved)));
    out
}

/// The pid of the server running session `name`, if there is one.
pub fn running(name: &str) -> Option<i32> {
    let text = fs::read_to_string(dir().join(format!("{name}.lock"))).ok()?;
    let pid = text.trim().parse::<i32>().ok()?;
    process_alive(pid).then_some(pid)
}

/// "5m ago"-style age for `haunt ls`.
pub fn age(time: SystemTime) -> String {
    let secs = time.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// Held by a session's server, so two servers can't run (and save) the same
/// session. A lock left behind by a crash is ignored because its pid is dead.
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    pub fn acquire(name: &str) -> Result<Lock> {
        let path = dir().join(format!("{name}.lock"));
        if let Some(pid) = running(name)
            && pid != std::process::id() as i32
        {
            bail!("session '{name}' is already running (pid {pid})");
        }
        fs::create_dir_all(dir())?;
        fs::write(&path, std::process::id().to_string())?;
        Ok(Lock { path })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn process_alive(pid: i32) -> bool {
    // Signal 0 checks for existence without sending anything. EPERM means
    // the process exists but belongs to someone else.
    let ok = unsafe { libc::kill(pid, 0) } == 0;
    ok || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Split;

    #[test]
    fn names() {
        assert!(valid_name("work"));
        assert!(valid_name("my-project_2.0"));
        assert!(!valid_name(""));
        assert!(!valid_name("../etc"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("has space"));
        assert!(!valid_name(&"x".repeat(65)));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("haunt-test-{}", std::process::id()));
        let path = dir.join("s.json");
        let session = Session::new(
            1,
            vec![SavedTab {
                name: "dev".into(),
                collapsed: true,
                focused: 3,
                layout: Node::Split {
                    split: Split::Vertical,
                    first: Box::new(Node::Pane(2)),
                    second: Box::new(Node::Pane(3)),
                },
                panes: vec![
                    SavedPane {
                        id: 2,
                        cwd: Some("/tmp".into()),
                        title: None,
                        history: "\x1b[0;31mred\x1b[0m\r\nline".into(),
                    },
                    SavedPane {
                        id: 3,
                        cwd: None,
                        title: Some("vim".into()),
                        history: String::new(),
                    },
                ],
            }],
        );
        assert_eq!(load(&path).unwrap(), None);
        save(&path, &session).unwrap();
        assert_eq!(load(&path).unwrap(), Some(session));
        assert!(!path.with_extension("json.tmp").exists());

        fs::write(&path, "not json").unwrap();
        assert!(load(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dead_pid_is_not_alive() {
        assert!(process_alive(std::process::id() as i32));
        // Pids are far below this on macOS and Linux.
        assert!(!process_alive(999_999_999));
    }
}
