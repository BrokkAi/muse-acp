//! Remembers each session's read-only or plan mode, so a session loaded or
//! resumed later, even by a new adapter process, opens on the read-only host
//! again. Muse cannot store the mode: it is a launch flag of the host, not a
//! property of the session.
//!
//! The store is a JSON object from Muse session id to mode, kept in
//! `$XDG_STATE_HOME/muse-acp/session-modes.json` (Windows:
//! `%LOCALAPPDATA%\muse-acp\session-modes.json`). Default-mode sessions are
//! not listed. An unreadable store reads as empty.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::json::{J, esc, parse_json};

static LOCK: Mutex<()> = Mutex::new(());

fn store_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            } else {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            }
        })?;
    Some(state.join("muse-acp").join("session-modes.json"))
}

fn read_all() -> Vec<(String, String)> {
    let Some(path) = store_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match parse_json(&text) {
        Ok(J::Obj(fields)) => fields
            .into_iter()
            .filter_map(|(sid, mode)| {
                let mode = crate::acp::resolve_session_mode(mode.as_str()?)?;
                crate::acp::is_read_only_mode(mode).then(|| (sid, mode.to_string()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The remembered mode of a Muse session; `default` when none is stored.
pub fn load(msp_sid: &str) -> &'static str {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_all()
        .into_iter()
        .find(|(sid, _)| sid == msp_sid)
        .and_then(|(_, mode)| crate::acp::resolve_session_mode(&mode))
        .unwrap_or(crate::acp::DEFAULT_MODE)
}

/// Remembers a session's mode. A failure is logged: the mode still applies
/// for as long as this adapter runs.
pub fn save(msp_sid: &str, mode: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(path) = store_path() else {
        return;
    };
    let mut entries: Vec<(String, String)> = read_all()
        .into_iter()
        .filter(|(sid, _)| sid != msp_sid)
        .collect();
    if crate::acp::is_read_only_mode(mode) {
        entries.push((msp_sid.to_string(), mode.to_string()));
    }
    let body = entries
        .iter()
        .map(|(sid, mode)| format!("{}:{}", esc(sid), esc(mode)))
        .collect::<Vec<_>>()
        .join(",");
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let staging = path.with_extension("json.tmp");
        std::fs::write(&staging, format!("{{{body}}}\n"))?;
        std::fs::rename(&staging, &path)
    };
    if let Err(error) = write() {
        crate::msp::log(&format!(
            "could not remember the mode of session {msp_sid} in {}: {error}",
            path.display()
        ));
    }
}
