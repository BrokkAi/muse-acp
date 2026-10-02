//! The Muse hosts this adapter runs: the main `muse serve`, and a read-only
//! one launched with `--disable-write --disable-shell` the first time a
//! read-only or plan session needs it. Muse itself refuses writes and shell
//! commands on the read-only host.
//!
//! A session can be loaded by only one host at a time, and a host releases a
//! session only when it shuts down. `Hosts` therefore records which host owns
//! each Muse session (including subagent child sessions, learned from the
//! events each host emits) and sends every command to the owner named by its
//! `sessionId`. Commands without one go to the main host.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use crate::json::{J, parse_json};
use crate::msp::{HandshakeInfo, LaunchError, MspEvent, MspHost};

/// Flags that make Muse refuse workspace writes and shell commands.
pub const READ_ONLY_ARGS: [&str; 2] = ["--disable-write", "--disable-shell"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Main,
    ReadOnly,
}

impl HostKind {
    pub fn name(self) -> &'static str {
        match self {
            HostKind::Main => "main",
            HostKind::ReadOnly => "read-only",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            HostKind::Main => &[],
            HostKind::ReadOnly => &READ_ONLY_ARGS,
        }
    }
}

/// Which process an event came from. Events from a process that has since
/// been replaced, such as one shut down to release a session, are stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostTag {
    pub kind: HostKind,
    pub generation: u64,
}

/// Starts forwarding one process's events into the adapter's event loop.
pub type Forward = Box<dyn Fn(HostTag, Receiver<MspEvent>) + Send + Sync>;

struct Current {
    host: Arc<MspHost>,
    generation: u64,
}

pub struct Hosts {
    main: Mutex<Current>,
    read_only: Mutex<Option<Current>>,
    owners: Mutex<HashMap<String, HostKind>>,
    user_input_dialogs: bool,
    user_shell: bool,
    forward: Forward,
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

impl Hosts {
    /// Launches the main host.
    pub fn launch(
        user_input_dialogs: bool,
        user_shell: bool,
        forward: Forward,
    ) -> Result<Arc<Hosts>, LaunchError> {
        let (host, generation) =
            launch_process(HostKind::Main, user_input_dialogs, user_shell, &forward)?;
        Ok(Arc::new(Hosts {
            main: Mutex::new(Current { host, generation }),
            read_only: Mutex::new(None),
            owners: Mutex::new(HashMap::new()),
            user_input_dialogs,
            user_shell,
            forward,
        }))
    }

    /// Launches a new process of this kind, forwarding its events, without
    /// making it current; see [`Hosts::replace`].
    pub fn launch_kind(&self, kind: HostKind) -> Result<(Arc<MspHost>, u64), LaunchError> {
        launch_process(
            kind,
            self.user_input_dialogs,
            self.user_shell,
            &self.forward,
        )
    }

    /// Makes `host` the current process of its kind.
    pub fn replace(&self, kind: HostKind, host: Arc<MspHost>, generation: u64) {
        let current = Current { host, generation };
        match kind {
            HostKind::Main => *lock(&self.main) = current,
            HostKind::ReadOnly => *lock(&self.read_only) = Some(current),
        }
    }

    /// Forgets the read-only process, which exited or was shut down with no
    /// sessions left; the next read-only session launches a new one.
    pub fn clear_read_only(&self) {
        lock(&self.read_only).take();
    }

    /// The current process of this kind, if one is running.
    pub fn host(&self, kind: HostKind) -> Option<Arc<MspHost>> {
        match kind {
            HostKind::Main => Some(lock(&self.main).host.clone()),
            HostKind::ReadOnly => lock(&self.read_only).as_ref().map(|c| c.host.clone()),
        }
    }

    pub fn main_host(&self) -> Arc<MspHost> {
        lock(&self.main).host.clone()
    }

    /// The read-only process, launched on first use.
    pub fn read_only_host(&self) -> Result<Arc<MspHost>, String> {
        if let Some(host) = self.host(HostKind::ReadOnly) {
            return Ok(host);
        }
        let (host, generation) = self
            .launch_kind(HostKind::ReadOnly)
            .map_err(|e| format!("could not start the read-only Muse host: {e}"))?;
        crate::msp::log("read-only Muse host started (--disable-write --disable-shell)");
        self.replace(HostKind::ReadOnly, host.clone(), generation);
        Ok(host)
    }

    /// Whether events with this tag come from a current process.
    pub fn is_current(&self, tag: HostTag) -> bool {
        match tag.kind {
            HostKind::Main => lock(&self.main).generation == tag.generation,
            HostKind::ReadOnly => lock(&self.read_only)
                .as_ref()
                .is_some_and(|c| c.generation == tag.generation),
        }
    }

    /// Records the host that loaded a session. An explicit start, resume,
    /// fork, or move decides; events only teach sessions not yet known, such
    /// as subagent children.
    pub fn note_owner(&self, msp_sid: &str, kind: HostKind) {
        if !msp_sid.is_empty() {
            lock(&self.owners).insert(msp_sid.to_string(), kind);
        }
    }

    pub fn learn_owner(&self, msp_sid: &str, kind: HostKind) {
        if !msp_sid.is_empty() {
            lock(&self.owners)
                .entry(msp_sid.to_string())
                .or_insert(kind);
        }
    }

    /// The host recorded for a session, if any.
    pub fn known_owner(&self, msp_sid: &str) -> Option<HostKind> {
        lock(&self.owners).get(msp_sid).copied()
    }

    pub fn owner(&self, msp_sid: &str) -> HostKind {
        lock(&self.owners)
            .get(msp_sid)
            .copied()
            .unwrap_or(HostKind::Main)
    }

    /// The process that owns the params' `sessionId`, else the main host.
    fn route(&self, params_json: &str) -> Arc<MspHost> {
        let owner = parse_json(params_json)
            .ok()
            .and_then(|params| {
                params
                    .get("sessionId")
                    .and_then(J::as_str)
                    .map(str::to_string)
            })
            .map(|sid| self.owner(&sid))
            .unwrap_or(HostKind::Main);
        self.host(owner).unwrap_or_else(|| self.main_host())
    }

    pub fn command(&self, method: &str, params_json: &str) -> Result<J, J> {
        self.route(params_json).command(method, params_json)
    }

    pub fn mint_cmd(&self, prefix: &str) -> String {
        self.main_host().mint_cmd(prefix)
    }

    pub fn handshake(&self) -> HandshakeInfo {
        self.main_host().handshake()
    }

    pub fn logged_out(&self) -> bool {
        self.main_host().logged_out()
    }

    pub fn refresh_config(&self) {
        for host in self.all() {
            host.refresh_config();
        }
    }

    pub fn shutdown(&self) {
        for host in self.all() {
            host.shutdown();
        }
    }

    pub fn force_stop(&self) {
        for host in self.all() {
            host.force_stop();
        }
    }

    fn all(&self) -> Vec<Arc<MspHost>> {
        let mut hosts = vec![self.main_host()];
        hosts.extend(self.host(HostKind::ReadOnly));
        hosts
    }
}

fn launch_process(
    kind: HostKind,
    user_input_dialogs: bool,
    user_shell: bool,
    forward: &Forward,
) -> Result<(Arc<MspHost>, u64), LaunchError> {
    let (host, events) = MspHost::launch(user_input_dialogs, user_shell, kind.args())?;
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    forward(HostTag { kind, generation }, events);
    Ok((host, generation))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}
