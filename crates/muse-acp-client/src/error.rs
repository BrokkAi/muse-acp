use std::fmt;
use std::io;
use std::time::Duration;

use crate::protocol::RpcError;

/// A failure while connecting to or using an ACP agent.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClientError {
    /// The underlying stdio transport failed.
    Io(io::Error),
    /// A JSON value could not be encoded or decoded.
    Json(serde_json::Error),
    /// An inbound frame was not valid ACP JSON-RPC.
    Protocol(String),
    /// The agent answered a request with a JSON-RPC error.
    Rpc(RpcError),
    /// A request did not receive a response before its deadline.
    RequestTimeout {
        method: String,
        id: u64,
        timeout: Duration,
    },
    /// The agent used `$/cancel_request` to cancel one of this client's requests.
    RequestCancelled { id: u64 },
    /// No event arrived before an event-wait deadline.
    EventTimeout { timeout: Duration },
    /// The connection ended before a response arrived.
    ConnectionClosed { message: String },
    /// The local event receiver was dropped by its owner.
    EventStreamClosed,
    /// The client was used before a successful `initialize`.
    NotInitialized,
    /// `initialize` was called more than once on one connection.
    AlreadyInitialized,
    /// Another caller is already initializing this connection.
    InitializationInProgress,
    /// The agent returned an initialization result without `protocolVersion`.
    InvalidInitialize(String),
    /// A handler panicked while servicing an agent request.
    HandlerPanicked { method: String },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "ACP transport I/O failed: {error}"),
            Self::Json(error) => write!(f, "ACP JSON failed: {error}"),
            Self::Protocol(message) => write!(f, "ACP protocol error: {message}"),
            Self::Rpc(error) => write!(f, "ACP request failed: {error}"),
            Self::RequestTimeout {
                method,
                id,
                timeout,
            } => write!(
                f,
                "ACP request {id} ({method}) timed out after {}ms",
                timeout.as_millis()
            ),
            Self::RequestCancelled { id } => write!(f, "ACP request {id} was cancelled"),
            Self::EventTimeout { timeout } => write!(
                f,
                "timed out after {}ms waiting for an ACP event",
                timeout.as_millis()
            ),
            Self::ConnectionClosed { message } => {
                write!(f, "ACP connection closed: {message}")
            }
            Self::EventStreamClosed => f.write_str("ACP event stream is closed"),
            Self::NotInitialized => f.write_str("ACP client is not initialized"),
            Self::AlreadyInitialized => f.write_str("ACP client is already initialized"),
            Self::InitializationInProgress => {
                f.write_str("ACP client initialization is already in progress")
            }
            Self::InvalidInitialize(message) => {
                write!(f, "invalid ACP initialize response: {message}")
            }
            Self::HandlerPanicked { method } => {
                write!(f, "ACP client request handler panicked for {method}")
            }
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ClientError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
