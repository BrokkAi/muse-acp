use std::fmt;
use std::io;
use std::time::Duration;

use crate::exit::ExitClassification;
use crate::json::J;

/// Why a request, notification, or response did not complete.
#[derive(Debug)]
#[non_exhaustive]
pub enum MspError {
    /// The host answered with a JSON-RPC error object, kept verbatim
    /// (`code`, `message`, and the typed `data` such as `data.kind`).
    Rpc(J),
    /// No response arrived within the deadline. The waiter is removed, so a
    /// late response is ignored. A command that carries a `commandId` may be
    /// retried with the same id; never mint a fresh one for a retry.
    Timeout {
        method: String,
        id: u64,
        timeout: Duration,
    },
    /// The caller cancelled the request through its [`crate::RequestHandle`].
    /// The frame was already written; the host may still act on it.
    Cancelled { method: String, id: u64 },
    /// The host closed its stdout before answering.
    HostClosed,
    /// The client failed the request while closing the connection, for
    /// example "serve shutting down" or "serve stdin write timed out; host
    /// terminated".
    Closed(String),
    /// The frame could not be written. A write that times out terminates the
    /// transport and reports [`io::ErrorKind::TimedOut`].
    Write(io::Error),
}

impl MspError {
    /// The JSON-RPC error code of a host error.
    pub fn code(&self) -> Option<i64> {
        match self {
            MspError::Rpc(error) => match error.get("code") {
                Some(J::Num(code)) => code.parse().ok(),
                _ => None,
            },
            _ => None,
        }
    }

    /// The `message` of a host error.
    pub fn message(&self) -> Option<&str> {
        match self {
            MspError::Rpc(error) => error.get("message").and_then(J::as_str),
            _ => None,
        }
    }

    /// The typed `data.kind` of a host error, such as `commandRejected`.
    pub fn kind(&self) -> Option<&str> {
        match self {
            MspError::Rpc(error) => error
                .get("data")
                .and_then(|data| data.get("kind"))
                .and_then(J::as_str),
            _ => None,
        }
    }
}

impl fmt::Display for MspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MspError::Rpc(_) => match self.code() {
                Some(code) => write!(
                    f,
                    "host error {code}: {}",
                    self.message().unwrap_or("unknown error")
                ),
                None => write!(
                    f,
                    "host error: {}",
                    self.message().unwrap_or("unknown error")
                ),
            },
            MspError::Timeout {
                method,
                id,
                timeout,
            } => write!(
                f,
                "serve command timed out after {}ms method={method} id={id}",
                timeout.as_millis()
            ),
            MspError::Cancelled { method, id } => {
                write!(f, "request cancelled method={method} id={id}")
            }
            MspError::HostClosed => f.write_str("serve host closed the connection"),
            MspError::Closed(reason) => f.write_str(reason),
            MspError::Write(error) => write!(f, "serve write failed: {error}"),
        }
    }
}

impl std::error::Error for MspError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MspError::Write(error) => Some(error),
            _ => None,
        }
    }
}

/// Why the MSP handshake did not complete.
#[derive(Debug)]
pub enum HandshakeError {
    /// The `initialize` request failed.
    Request(MspError),
    /// The handshake check refused the negotiated handshake. The
    /// `initialized` notification was not sent.
    Rejected(String),
    /// The `initialized` notification could not be sent.
    Initialized(MspError),
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandshakeError::Request(error) => write!(f, "serve initialize failed: {error}"),
            HandshakeError::Rejected(reason) => f.write_str(reason),
            HandshakeError::Initialized(MspError::Write(error)) => {
                write!(f, "serve notify failed: {error}")
            }
            HandshakeError::Initialized(error) => write!(f, "serve notify failed: {error}"),
        }
    }
}

impl std::error::Error for HandshakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HandshakeError::Request(error) | HandshakeError::Initialized(error) => Some(error),
            HandshakeError::Rejected(_) => None,
        }
    }
}

/// Why a host could not be started.
#[derive(Debug)]
pub enum LaunchError {
    /// The executable could not be started. [`io::ErrorKind::NotFound`] means
    /// there is no executable at that path.
    Spawn { program: String, error: io::Error },
    /// The process started without a usable pipe, or a probe could not watch
    /// it.
    Startup(String),
    /// The host exited during the handshake.
    HostExit(ExitClassification),
    /// The handshake failed while the host kept running; the client closed it.
    Handshake(HandshakeError),
}

impl LaunchError {
    /// Whether launching again without other changes can reasonably help.
    pub fn retryable(&self) -> bool {
        match self {
            LaunchError::Spawn { .. } => false,
            LaunchError::Startup(_) => true,
            LaunchError::HostExit(exit) => exit.retryable(),
            LaunchError::Handshake(error) => !matches!(error, HandshakeError::Rejected(_)),
        }
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::Spawn { program, error } => {
                write!(f, "failed to spawn '{program} serve': {error}")
            }
            LaunchError::Startup(message) => f.write_str(message),
            LaunchError::HostExit(exit) => write!(f, "{exit}"),
            LaunchError::Handshake(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LaunchError::Spawn { error, .. } => Some(error),
            LaunchError::Handshake(error) => Some(error),
            LaunchError::Startup(_) | LaunchError::HostExit(_) => None,
        }
    }
}

/// Something the client observed that a caller may want to log. Diagnostics
/// never carry frame payloads or stderr text, so they cannot leak file
/// contents or credentials. Install a sink with
/// [`crate::ClientOptions::diagnostics`]; without one they are dropped.
#[derive(Debug)]
#[non_exhaustive]
pub enum Diagnostic<'a> {
    /// A frame that is not UTF-8 was skipped.
    InvalidUtf8Frame,
    /// A stdout frame exceeded the reader's memory bound.
    StdoutFrameTooLarge { max_bytes: usize },
    /// A frame that is not valid JSON was skipped.
    MalformedFrame { error: &'a str },
    /// A server request for a method without a configured
    /// [`crate::ServerRequestMode`] was answered with `methodNotFound`.
    UnsupportedServerRequest { method: &'a str, id: &'a J },
    /// A response arrived for no pending request: it came after its request
    /// timed out or was cancelled, or carried an id this client never sent.
    UnknownResponse { id: &'a str },
    /// A notification arrived. Only the method name is reported.
    Notification { method: &'a str },
    /// `close` gave up waiting for the writer lock.
    ShutdownWriterLockTimeout,
    /// `close` gave up waiting for the child lock.
    ShutdownChildLockTimeout,
    /// `close` gave up waiting for the child to exit.
    ShutdownChildExitTimeout,
    /// `close` could not query the child's status.
    ShutdownWaitFailed { error: &'a io::Error },
    /// `close` finished with this many bytes of stderr captured.
    StderrCaptured { bytes: usize },
}

impl fmt::Display for Diagnostic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Diagnostic::InvalidUtf8Frame => f.write_str("serve parse error: invalid UTF-8 frame"),
            Diagnostic::StdoutFrameTooLarge { max_bytes } => {
                write!(
                    f,
                    "serve protocol error: stdout frame exceeds {max_bytes} bytes"
                )
            }
            Diagnostic::MalformedFrame { error } => write!(f, "serve parse error: {error}"),
            Diagnostic::UnsupportedServerRequest { method, id } => write!(
                f,
                "unsupported MSP server request: {method} id={}",
                crate::json::j_to_string(id)
            ),
            Diagnostic::UnknownResponse { id } => write!(f, "serve response for unknown id {id}"),
            Diagnostic::Notification { method } => write!(f, "msp<-host method={method}"),
            Diagnostic::ShutdownWriterLockTimeout => {
                f.write_str("serve shutdown timed out waiting for writer lock")
            }
            Diagnostic::ShutdownChildLockTimeout => {
                f.write_str("serve shutdown timed out waiting for child lock")
            }
            Diagnostic::ShutdownChildExitTimeout => {
                f.write_str("serve shutdown timed out waiting for child exit")
            }
            Diagnostic::ShutdownWaitFailed { error } => {
                write!(f, "serve shutdown wait failed: {error}")
            }
            Diagnostic::StderrCaptured { bytes } => {
                write!(f, "serve stderr captured {bytes} bytes")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_json;

    /// `muse-acp` logs these lines verbatim, so a wording change is a
    /// change to its diagnostic contract.
    #[test]
    fn diagnostic_lines_are_stable() {
        let id = parse_json(r#""s-1""#).unwrap();
        let wait = io::Error::other("busy");
        for (diagnostic, line) in [
            (
                Diagnostic::InvalidUtf8Frame,
                "serve parse error: invalid UTF-8 frame",
            ),
            (
                Diagnostic::MalformedFrame {
                    error: "trailing characters",
                },
                "serve parse error: trailing characters",
            ),
            (
                Diagnostic::UnsupportedServerRequest {
                    method: "future/request",
                    id: &id,
                },
                "unsupported MSP server request: future/request id=\"s-1\"",
            ),
            (
                Diagnostic::UnknownResponse { id: "7" },
                "serve response for unknown id 7",
            ),
            (
                Diagnostic::Notification {
                    method: "turn/started",
                },
                "msp<-host method=turn/started",
            ),
            (
                Diagnostic::ShutdownWriterLockTimeout,
                "serve shutdown timed out waiting for writer lock",
            ),
            (
                Diagnostic::ShutdownChildLockTimeout,
                "serve shutdown timed out waiting for child lock",
            ),
            (
                Diagnostic::ShutdownChildExitTimeout,
                "serve shutdown timed out waiting for child exit",
            ),
            (
                Diagnostic::ShutdownWaitFailed { error: &wait },
                "serve shutdown wait failed: busy",
            ),
            (
                Diagnostic::StderrCaptured { bytes: 12 },
                "serve stderr captured 12 bytes",
            ),
        ] {
            assert_eq!(diagnostic.to_string(), line);
        }
    }

    #[test]
    fn host_errors_expose_their_typed_fields() {
        let error = MspError::Rpc(
            parse_json(r#"{"code":-32030,"message":"rejected","data":{"kind":"commandRejected"}}"#)
                .unwrap(),
        );
        assert_eq!(error.code(), Some(-32030));
        assert_eq!(error.message(), Some("rejected"));
        assert_eq!(error.kind(), Some("commandRejected"));
        assert_eq!(error.to_string(), "host error -32030: rejected");
        assert_eq!(MspError::HostClosed.code(), None);
        assert_eq!(
            HandshakeError::Initialized(MspError::Write(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "serve stdin closed"
            )))
            .to_string(),
            "serve notify failed: serve stdin closed"
        );
    }
}
