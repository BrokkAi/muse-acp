//! A small, synchronous client for the Muse Session Protocol (MSP).
//!
//! `muse-msp-client` launches one `muse serve` host (or connects over any
//! pair of streams), performs the `initialize` handshake, correlates
//! JSON-RPC responses with per-method deadlines, serializes concurrent
//! writes, delivers notifications and server-initiated requests as events,
//! classifies how the host exited, and shuts it down within a bounded time.
//! It has no dependencies and no async runtime, and it does not depend on
//! `muse-acp`. Nothing is read from the environment: the executable,
//! arguments, variables, and capabilities are all explicit.
//!
//! # Example
//!
//! ```no_run
//! use muse_msp_client::json::{J, esc};
//! use muse_msp_client::{
//!     ClientOptions, LaunchConfig, MspClient, MspEvent, Params, ServerRequestMode,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut config = LaunchConfig::new("/home/me/.local/bin/muse");
//! config.options(
//!     ClientOptions::default().server_request("approval/request", ServerRequestMode::Acknowledge),
//! );
//! let (client, events) = MspClient::launch(config)?;
//! println!("connected to {:?}", client.handshake().and_then(|h| h.server_version));
//!
//! let started = client.request(
//!     "session/start",
//!     Params::raw(format!(
//!         "{{\"commandId\":{},\"workspaceRoot\":{}}}",
//!         esc(&client.mint_command_id()),
//!         esc("/home/me/project")
//!     )),
//! )?;
//! let session = started.get("session").and_then(|s| s.get("sessionId")).and_then(J::as_str);
//! println!("session {session:?}");
//!
//! while let Ok(event) = events.recv() {
//!     match event {
//!         MspEvent::Notification { method, .. } => println!("{method}"),
//!         MspEvent::Closed { reason } => {
//!             println!("host closed: {reason}");
//!             break;
//!         }
//!         _ => {}
//!     }
//! }
//! client.close();
//! # Ok(())
//! # }
//! ```

mod client;
mod config;
mod error;
mod exit;
mod handshake;
pub mod json;
mod process;

pub use client::{MspClient, MspEvent, Params, PendingRequest, RequestHandle, default_timeout};
pub use config::{
    Capabilities, ClientInfo, ClientOptions, HostFlags, LaunchConfig, ServerRequestMode,
    ShutdownPolicy, Transport,
};
pub use error::{Diagnostic, HandshakeError, LaunchError, MspError};
pub use exit::{ExitClassification, ExitKind, classify_exit_parts};
pub use handshake::{ENVELOPE_SCHEMA_VERSION, Handshake, SessionDurability, parse_release};
pub use process::probe_exit;
