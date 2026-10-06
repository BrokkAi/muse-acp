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
use crate::msp::{HandshakeInfo, LaunchError, MspEvent, MspHost, mk_err};

/// Flags that make Muse refuse workspace writes and shell commands.
pub const READ_ONLY_ARGS: [&str; 2] = ["--disable-write", "--disable-shell"];
/// The reviewer host is read-only and memory-only: `--no-session-log` keeps
/// every review out of the user's saved session list.
pub const REVIEWER_ARGS: [&str; 3] = ["--no-session-log", "--disable-write", "--disable-shell"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Main,
    ReadOnly,
    Reviewer,
}

impl HostKind {
    pub fn name(self) -> &'static str {
        match self {
            HostKind::Main => "main",
            HostKind::ReadOnly => "read-only",
            HostKind::Reviewer => "reviewer",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            HostKind::Main => &[],
            HostKind::ReadOnly => &READ_ONLY_ARGS,
            HostKind::Reviewer => &REVIEWER_ARGS,
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
    reviewer: Mutex<Option<Current>>,
    owners: Mutex<HashMap<String, HostKind>>,
    /// Held while the read-only host launches, so two callers never start two.
    launching: Mutex<()>,
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
            reviewer: Mutex::new(None),
            owners: Mutex::new(HashMap::new()),
            launching: Mutex::new(()),
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
            HostKind::Reviewer => *lock(&self.reviewer) = Some(current),
        }
    }

    /// Forgets the read-only process, which exited or was shut down with no
    /// sessions left; the next read-only session launches a new one.
    pub fn clear_read_only(&self) {
        lock(&self.read_only).take();
    }

    /// Forgets the reviewer process; the next review launches a new one.
    pub fn clear_reviewer(&self) {
        lock(&self.reviewer).take();
    }

    /// The current process of this kind, if one is running.
    pub fn host(&self, kind: HostKind) -> Option<Arc<MspHost>> {
        match kind {
            HostKind::Main => Some(lock(&self.main).host.clone()),
            HostKind::ReadOnly => lock(&self.read_only).as_ref().map(|c| c.host.clone()),
            HostKind::Reviewer => lock(&self.reviewer).as_ref().map(|c| c.host.clone()),
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
        let _launching = lock(&self.launching);
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

    /// The memory-only, read-only reviewer process, launched on first use.
    pub fn reviewer_host(&self) -> Result<Arc<MspHost>, String> {
        if let Some(host) = self.host(HostKind::Reviewer) {
            return Ok(host);
        }
        let _launching = lock(&self.launching);
        if let Some(host) = self.host(HostKind::Reviewer) {
            return Ok(host);
        }
        let (host, generation) = self
            .launch_kind(HostKind::Reviewer)
            .map_err(|e| format!("could not start the reviewer Muse host: {e}"))?;
        crate::msp::log(
            "reviewer Muse host started (--no-session-log --disable-write --disable-shell)",
        );
        self.replace(HostKind::Reviewer, host.clone(), generation);
        Ok(host)
    }

    /// Whether events with this tag come from a current process.
    pub fn is_current(&self, tag: HostTag) -> bool {
        match tag.kind {
            HostKind::Main => lock(&self.main).generation == tag.generation,
            HostKind::ReadOnly => lock(&self.read_only)
                .as_ref()
                .is_some_and(|c| c.generation == tag.generation),
            HostKind::Reviewer => lock(&self.reviewer)
                .as_ref()
                .is_some_and(|c| c.generation == tag.generation),
        }
    }

    /// Records the host that holds a session: the one that started, resumed,
    /// forked, or moved it, or the one running a subagent child session.
    pub fn note_owner(&self, msp_sid: &str, kind: HostKind) {
        if !msp_sid.is_empty() {
            lock(&self.owners).insert(msp_sid.to_string(), kind);
        }
    }

    /// Records an owner only for a session not known yet.
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

    /// Forgets a session Muse deleted, so no later command routes to the
    /// host that used to hold it.
    pub fn forget_owner(&self, msp_sid: &str) {
        lock(&self.owners).remove(msp_sid);
    }

    pub fn owner(&self, msp_sid: &str) -> HostKind {
        lock(&self.owners)
            .get(msp_sid)
            .copied()
            .unwrap_or(HostKind::Main)
    }

    /// The process that owns the params' `sessionId`, else the main host. A
    /// read-only session never falls back to the main host: its host is
    /// started again if it is not running.
    fn route(&self, params_json: &str) -> Result<Arc<MspHost>, J> {
        // Most adapters never run a read-only session; skip parsing then.
        let any_read_only = lock(&self.owners)
            .values()
            .any(|kind| matches!(*kind, HostKind::ReadOnly | HostKind::Reviewer));
        if !any_read_only {
            return Ok(self.main_host());
        }
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
        match owner {
            HostKind::Main => Ok(self.main_host()),
            HostKind::ReadOnly => self
                .read_only_host()
                .map_err(|message| mk_err(-32603, &message)),
            HostKind::Reviewer => self
                .reviewer_host()
                .map_err(|message| mk_err(-32603, &message)),
        }
    }

    pub fn command(&self, method: &str, params_json: &str) -> Result<J, J> {
        self.route(params_json)?.command(method, params_json)
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
        hosts.extend(self.host(HostKind::Reviewer));
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
