use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::time::{Duration, timeout};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

pub struct MuseHost {
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Pending,
    next_id: AtomicI64,
}

impl MuseHost {
    pub async fn spawn() -> Result<(Arc<Self>, ChildStdout), String> {
        let binary = std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".to_string());
        let extra = std::env::var("MUSE_SERVE_EXTRA_ARGS").unwrap_or_default();
        let mut command = Command::new(&binary);
        command.arg("serve");
        command.args(extra.split_whitespace().map(str::to_string));
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to spawn '{binary} serve': {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "MSP host had no stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "MSP host had no stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "MSP host had no stderr".to_string())?;

        let host = Arc::new(Self {
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicI64::new(1),
        });

        tokio::spawn(read_stderr(stderr));
        Ok((host, stdout))
    }

    /// Close MSP stdin and reap the child process.
    ///
    /// The stdout reader intentionally keeps its `Arc<MuseHost>` alive, so
    /// closing stdin here ensures that reader reaches EOF rather than leaving
    /// a detached `muse serve` process after the ACP client disconnects.
    pub async fn shutdown(&self) {
        self.stdin.lock().await.take();
        let Some(mut child) = self.child.lock().await.take() else {
            return;
        };

        match timeout(Duration::from_secs(2), child.wait()).await {
            Ok(Ok(status)) => {
                if !status.success() {
                    eprintln!("[muse-acp] MSP host exited with {status} during shutdown");
                }
            }
            Ok(Err(error)) => {
                eprintln!("[muse-acp] failed to wait for MSP host shutdown: {error}");
            }
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }
    }

    pub async fn initialize(&self) -> Result<Value, RpcError> {
        let result = self
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "muse_acp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;
        self.notification("initialized", json!({})).await?;
        Ok(result)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        if let Err(error) = self.write(message).await {
            self.pending.lock().await.remove(&id);
            return Err(RpcError::internal(format!(
                "failed to write MSP request: {error}"
            )));
        }

        match rx.await {
            Ok(result) => result,
            Err(_) => Err(RpcError::internal("MSP response channel closed")),
        }
    }

    pub async fn notification(&self, method: &str, params: Value) -> Result<(), RpcError> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.write(message).await.map_err(|error| {
            RpcError::internal(format!("failed to write MSP notification: {error}"))
        })
    }

    async fn write(&self, message: Value) -> Result<(), tokio::io::Error> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        let Some(stdin) = stdin.as_mut() else {
            return Err(tokio::io::Error::other("MSP host is shutting down"));
        };
        stdin.write_all(&bytes).await?;
        stdin.flush().await
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WireMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<WireError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireError {
    code: i64,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

pub async fn read_host(
    host: Arc<MuseHost>,
    stdout: tokio::process::ChildStdout,
    on_notification: impl Fn(String, Value) + Send + Sync + 'static,
    on_close: impl FnOnce() + Send + 'static,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<WireMessage>(&line) else {
            eprintln!("[muse-acp] ignoring malformed MSP line");
            continue;
        };

        if let Some(id) = message.id {
            let Some(id) = id.as_i64() else {
                continue;
            };
            let sender = host.pending.lock().await.remove(&id);
            if let Some(sender) = sender {
                let reply = if let Some(error) = message.error {
                    Err(RpcError {
                        code: error.code,
                        message: error.message,
                        data: error.data,
                    })
                } else {
                    Ok(message.result.unwrap_or(Value::Null))
                };
                let _ = sender.send(reply);
            }
        } else if let Some(method) = message.method {
            on_notification(method, message.params.unwrap_or_else(|| json!({})));
        }
    }

    let pending = std::mem::take(&mut *host.pending.lock().await);
    for (_, sender) in pending {
        let _ = sender.send(Err(RpcError::internal("MSP host closed")));
    }
    on_close();
}

async fn read_stderr(stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        eprintln!("[muse] {line}");
    }
}

pub fn uuid_v7() -> String {
    Uuid::now_v7().to_string()
}
