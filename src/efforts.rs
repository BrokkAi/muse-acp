//! Remembers the last explicitly chosen reasoning tier per model, so a new
//! session starts where the user left off — the way a new session already
//! inherits the host's last-used model. Muse keeps the same memory in its own
//! settings (`model_reasoning_efforts`), but MSP has no global-default setter:
//! `session/setReasoningEffort` only sets one session's standing default. The
//! adapter therefore applies the remembered tier with
//! `session/setReasoningEffort` right after `session/start`.
//!
//! The store is a JSON object from model id to tier, kept in
//! `$XDG_STATE_HOME/muse-acp/reasoning-efforts.json` (Windows:
//! `%LOCALAPPDATA%\muse-acp\reasoning-efforts.json`). Models without a
//! remembered tier, and the `default` choice, are not listed. An unreadable
//! store reads as empty.

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
    Some(state.join("muse-acp").join("reasoning-efforts.json"))
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
            .filter_map(|(model, effort)| {
                let effort = effort.as_str()?;
                crate::acp::is_reasoning_effort(effort).then(|| (model, effort.to_string()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The last tier chosen for this model, if one was remembered.
pub fn load(model: &str) -> Option<String> {
    if model.is_empty() {
        return None;
    }
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    read_all()
        .into_iter()
        .find(|(id, _)| id == model)
        .map(|(_, effort)| effort)
}

/// Remembers `effort` as the last tier chosen for `model`. A failure is
/// logged: the session that chose it still applies it for as long as this
/// adapter runs.
pub fn save(model: &str, effort: &str) {
    if model.is_empty() || !crate::acp::is_reasoning_effort(effort) {
        return;
    }
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut entries: Vec<(String, String)> = read_all()
        .into_iter()
        .filter(|(id, _)| id != model)
        .collect();
    entries.push((model.to_string(), effort.to_string()));
    write_all(&entries);
}

/// Forgets the remembered tier for `model`, so new sessions start at the Muse
/// default again. Used when the user picks the `default` selector value.
pub fn clear(model: &str) {
    if model.is_empty() {
        return;
    }
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let entries: Vec<(String, String)> = read_all()
        .into_iter()
        .filter(|(id, _)| id != model)
        .collect();
    write_all(&entries);
}

fn write_all(entries: &[(String, String)]) {
    let Some(path) = store_path() else {
        return;
    };
    let body = entries
        .iter()
        .map(|(model, effort)| format!("{}:{}", esc(model), esc(effort)))
        .collect::<Vec<_>>()
        .join(",");
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Each process stages its own file, so two adapters saving at once
        // never rename a half-written store into place. The later save
        // wins; a tier saved at the same moment by another adapter can be
        // lost, which only means a new session starts at the Muse default.
        let staging = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&staging, format!("{{{body}}}\n"))?;
        std::fs::rename(&staging, &path)
    };
    if let Err(error) = write() {
        crate::msp::log(&format!(
            "could not update the reasoning effort store in {}: {error}",
            path.display()
        ));
    }
}
