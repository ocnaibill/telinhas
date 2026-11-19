//! A small log file of what the connections did, for a person to send when
//! something does not work. It lives next to the app's other data and is
//! started afresh when it grows past a megabyte.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1_000_000;

fn directory() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
    }?;
    Some(base.join("Telinhas"))
}

/// Where the log is written, if there is a place for it.
pub fn path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = directory()?;
        std::fs::create_dir_all(&dir).ok()?;
        let file = dir.join("telinhas.log");
        if std::fs::metadata(&file).is_ok_and(|m| m.len() > MAX_BYTES) {
            let _ = std::fs::remove_file(&file);
        }
        Some(file)
    })
    .clone()
}

/// `HH:MM:SS` in UTC, which is enough to line up two people's logs.
fn clock() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let day = seconds % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, day % 3600 / 60, day % 60)
}

/// Appends one line. Failing to write is not worth bothering anyone about.
pub(crate) fn line(text: &str) {
    let Some(path) = path() else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{} {text}", clock());
    }
}
