//! Connection-wide escape hatch for shutdown, including a blocked ACP loop.
//! Normal cleanup runs first. The deadline thread never waits on I/O or locks;
//! terminal responses and diagnostics get a short, best-effort drain window.
use std::collections::HashMap;
use std::io::Write;
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use crate::{
    acp::StdoutShared,
    json::{J, esc, j_to_string},
    msp::MspHost,
};

static OUTPUT: OnceLock<StdoutShared> = OnceLock::new();
static REQUESTS: OnceLock<Mutex<HashMap<String, J>>> = OnceLock::new();
static HOST: OnceLock<Mutex<Weak<MspHost>>> = OnceLock::new();
static DISCONNECTED: AtomicBool = AtomicBool::new(false);
static EXPIRING: AtomicBool = AtomicBool::new(false);

pub fn initialize(stdout: &StdoutShared) {
    let _ = OUTPUT.set(stdout.clone());
}

pub fn register_host(host: &Arc<MspHost>) {
    *HOST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Arc::downgrade(host);
    if expiring() {
        host.force_stop();
    }
}

pub fn register_request(msg: &J) {
    if msg.get("method").and_then(J::as_str).is_some()
        && let Some(id) = msg.get("id")
    {
        REQUESTS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(j_to_string(id), id.clone());
    }
}

pub fn completed(id: &Option<J>) {
    if let Some(id) = id
        && let Some(requests) = REQUESTS.get()
    {
        requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&j_to_string(id));
    }
}

pub fn expiring() -> bool {
    EXPIRING.load(Ordering::SeqCst)
}

pub fn exit(code: i32) -> ! {
    std::process::exit(if expiring() { 1 } else { code });
}

fn timeout() -> Duration {
    Duration::from_millis(
        std::env::var("MUSE_SHUTDOWN_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| (100..=60_000).contains(v))
            .unwrap_or(8_000),
    )
}

/// Dropping a local guard cancels its timer after cleanup finishes.
pub struct Deadline(mpsc::Sender<()>);
impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

pub fn deadline(context: &'static str) -> Deadline {
    let (tx, rx) = mpsc::channel();
    if !expiring() {
        std::thread::spawn(move || {
            if matches!(
                rx.recv_timeout(timeout()),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                expire(context);
            }
        });
    }
    Deadline(tx)
}

/// Called on the stdin reader, before queueing EOF/shutdown. It also bounds a
/// main loop stuck waiting for a command or writing to an unread editor pipe.
pub fn disconnect() {
    if !DISCONNECTED.swap(true, Ordering::SeqCst) {
        std::thread::spawn(|| {
            std::thread::sleep(timeout());
            expire("editor disconnect");
        });
    }
}

/// Settle requests the main loop has not handled yet as well as active prompts.
/// Callers must keep a deadline armed: even a diagnostic pipe can be blocked.
pub fn settle(stdout: &StdoutShared, message: &str) {
    let _deadline = deadline("pending request settlement");
    let message = if expiring() {
        "adapter shutdown deadline expired"
    } else {
        message
    };
    let mut out = stdout.lock().unwrap_or_else(|p| p.into_inner());
    let Some(requests) = REQUESTS.get() else {
        return;
    };
    let pending: Vec<_> = requests
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .drain()
        .map(|(_, id)| id)
        .collect();
    for id in pending {
        let _ = writeln!(
            out,
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":-32603,\"message\":{}}}}}",
            j_to_string(&id),
            esc(message)
        );
    }
    let _ = out.flush();
}

fn expire(context: &'static str) -> ! {
    EXPIRING.store(true, Ordering::SeqCst);
    // Independent workers prevent a held output/child/stderr lock from
    // blocking the deadline itself. Delivery is impossible on a closed pipe.
    std::thread::spawn(move || {
        eprintln!(
            "[muse-acp] shutdown deadline expired ({context}); failing outstanding ACP requests; forcing exit"
        );
    });
    std::thread::spawn(|| {
        if let Some(host) = HOST
            .get()
            .and_then(|host| host.try_lock().ok())
            .and_then(|host| host.upgrade())
        {
            host.force_stop();
        }
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(stdout) = OUTPUT.get() {
            settle(stdout, "adapter shutdown deadline expired");
        }
        let _ = tx.send(());
    });
    // Give the kill/diagnostic workers time even when there are no requests.
    let started = std::time::Instant::now();
    let grace = Duration::from_millis(250);
    let _ = rx.recv_timeout(grace);
    std::thread::sleep(grace.saturating_sub(started.elapsed()));
    std::process::exit(1);
}
