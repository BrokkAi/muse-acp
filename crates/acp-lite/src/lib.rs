//! A small, synchronous client for the Agent Client Protocol.
//!
//! `acp-lite` owns an ACP stdio agent process, correlates JSON-RPC
//! responses, streams notifications, and invokes a [`ClientHandler`] for
//! requests the agent sends back to the client. It has no async runtime and
//! does not depend on `muse-acp`.
//!
//! # Example
//!
//! ```no_run
//! use acp_lite::{
//!     AgentProcess, Client, ClientHandler, ClientOptions, ContentBlock,
//!     InitializeRequest, NewSessionRequest, PromptRequest,
//! };
//! use std::sync::Arc;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let agent = AgentProcess::new("muse-acp");
//! let client = Client::spawn(
//!     agent,
//!     Arc::new(acp_lite::DenyAllHandler),
//!     ClientOptions::default(),
//! )?;
//!
//! let initialized = client.initialize(InitializeRequest::v2())?;
//! let cwd = std::env::current_dir()?;
//! let session = client.new_session(NewSessionRequest::new(cwd))?;
//! let prompt = PromptRequest::new(
//!     session.session_id,
//!     vec![ContentBlock::text("Explain this repository")],
//! );
//! let _ = client.prompt(prompt)?;
//! # Ok(())
//! # }
//! ```

mod client;
mod error;
mod handler;
mod process;
mod protocol;

pub use client::{
    Client, ClientEvent, ClientHandle, ClientOptions, NewSessionResponse, PromptResponse,
};
pub use error::ClientError;
pub use handler::{ClientHandler, DenyAllHandler, IncomingRequest, RequestContext, RequestOutcome};
pub use process::AgentProcess;
pub use protocol::{
    ClientInfo, ContentBlock, InitializeRequest, InitializeResponse, NewSessionRequest,
    PromptRequest, ProtocolVersion, RpcError,
};
