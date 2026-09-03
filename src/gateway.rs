use crate::msp::{self, MuseHost, RpcError};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub enum AcpEvent {
    Update {
        session_id: String,
        update: Value,
    },
    Permission {
        session_id: String,
        approval: Value,
    },
    Elicitation {
        session_id: String,
        user_input: Value,
    },
    HostClosed,
}

#[derive(Debug, Clone)]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider_id: Option<String>,
    pub profile_id: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
    Ultra,
}

impl ReasoningEffort {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Ultra,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Ultra => "ultra",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Xhigh => "Extra high",
            Self::Ultra => "Ultra",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::None => "Request no explicit reasoning",
            Self::Minimal => "Request the smallest amount of reasoning",
            Self::Low => "Request low reasoning effort",
            Self::Medium => "Request balanced reasoning effort",
            Self::High => "Request high reasoning effort",
            Self::Xhigh => "Request extra-high reasoning effort",
            Self::Ultra => "Request maximum reasoning effort",
        }
    }

    pub fn parse(value: &str) -> Result<Self, RpcError> {
        Ok(match value {
            "none" => Self::None,
            "minimal" => Self::Minimal,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "xhigh" => Self::Xhigh,
            "ultra" => Self::Ultra,
            _ => {
                return Err(RpcError::invalid_params(
                    "invalid muse.reasoningEffort value",
                ));
            }
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub cwd: String,
    pub active_turn: Option<String>,
    pub model: ModelSelection,
    pub reasoning_effort: ReasoningEffort,
}

type TurnKey = (String, String);
type TurnWatchers = HashMap<TurnKey, Vec<oneshot::Sender<TurnOutcome>>>;

pub struct Gateway {
    pub host: Arc<MuseHost>,
    muse_home: OnceLock<String>,
    sessions: Mutex<HashMap<String, SessionState>>,
    item_kinds: Mutex<HashMap<String, HashMap<String, String>>>,
    synthetic_user_commands: Mutex<HashMap<String, HashSet<String>>>,
    turn_watchers: Mutex<TurnWatchers>,
    finished_turns: Mutex<HashMap<TurnKey, TurnOutcome>>,
    pending_approvals: Mutex<HashMap<(String, String), Value>>,
    pending_user_inputs: Mutex<HashMap<(String, String), Value>>,
    pending_approval_requests: Mutex<HashMap<(String, String), Value>>,
    pending_user_input_requests: Mutex<HashMap<(String, String), Value>>,
    acp_tx: mpsc::UnboundedSender<AcpEvent>,
    acp_rx: Mutex<Option<mpsc::UnboundedReceiver<AcpEvent>>>,
    pub form_elicitation: AtomicBool,
    pub protocol_version: AtomicU32,
    closed: AtomicBool,
}

impl Gateway {
    pub async fn new() -> Result<Arc<Self>, String> {
        let (host, stdout) = MuseHost::spawn().await?;
        let (acp_tx, acp_rx) = mpsc::unbounded_channel();
        let gateway_slot: Arc<OnceLock<Arc<Self>>> = Arc::new(OnceLock::new());
        let callback_slot = gateway_slot.clone();
        let close_slot = gateway_slot.clone();
        tokio::spawn(msp::read_host(
            host.clone(),
            stdout,
            move |method, params| {
                if let Some(gateway) = callback_slot.get() {
                    gateway.handle_notification(method, params);
                }
            },
            move || {
                if let Some(gateway) = close_slot.get() {
                    gateway.host_closed();
                }
            },
        ));

        let initialize_result = match host.initialize().await {
            Ok(result) => result,
            Err(error) => {
                host.shutdown().await;
                return Err(format!("failed to initialize MSP host: {error}"));
            }
        };
        let muse_home = initialize_result
            .get("museHome")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| "MSP initialize returned no museHome".to_string())?;

        let gateway = Arc::new(Self {
            host,
            muse_home: OnceLock::new(),
            sessions: Mutex::new(HashMap::new()),
            item_kinds: Mutex::new(HashMap::new()),
            synthetic_user_commands: Mutex::new(HashMap::new()),
            turn_watchers: Mutex::new(HashMap::new()),
            finished_turns: Mutex::new(HashMap::new()),
            pending_approvals: Mutex::new(HashMap::new()),
            pending_user_inputs: Mutex::new(HashMap::new()),
            pending_approval_requests: Mutex::new(HashMap::new()),
            pending_user_input_requests: Mutex::new(HashMap::new()),
            acp_tx,
            acp_rx: Mutex::new(Some(acp_rx)),
            form_elicitation: AtomicBool::new(false),
            protocol_version: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        });
        let _ = gateway_slot.set(gateway.clone());
        let _ = gateway.muse_home.set(muse_home);

        Ok(gateway)
    }

    pub fn take_acp_receiver(&self) -> Option<mpsc::UnboundedReceiver<AcpEvent>> {
        self.acp_rx.lock().unwrap().take()
    }

    pub fn emit_update(&self, session_id: &str, update: Value) {
        let _ = self.acp_tx.send(AcpEvent::Update {
            session_id: session_id.to_string(),
            update,
        });
    }

    pub fn emit_permission(&self, session_id: &str, approval: Value) {
        let _ = self.acp_tx.send(AcpEvent::Permission {
            session_id: session_id.to_string(),
            approval,
        });
    }

    pub fn emit_elicitation(&self, session_id: &str, user_input: Value) {
        let _ = self.acp_tx.send(AcpEvent::Elicitation {
            session_id: session_id.to_string(),
            user_input,
        });
    }

    pub fn mark_synthetic_user_command(&self, session_id: &str, command_id: &str) {
        self.synthetic_user_commands
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default()
            .insert(command_id.to_string());
    }

    pub fn clear_synthetic_user_command(&self, session_id: &str, command_id: &str) {
        let mut commands = self.synthetic_user_commands.lock().unwrap();
        if let Some(session_commands) = commands.get_mut(session_id) {
            session_commands.remove(command_id);
            if session_commands.is_empty() {
                commands.remove(session_id);
            }
        }
    }

    fn is_synthetic_user_command(&self, session_id: &str, command_id: &str) -> bool {
        self.synthetic_user_commands
            .lock()
            .unwrap()
            .get(session_id)
            .is_some_and(|commands| commands.contains(command_id))
    }

    pub fn track_approval_request(&self, session_id: &str, approval_id: &str, request_id: Value) {
        self.pending_approval_requests.lock().unwrap().insert(
            (session_id.to_string(), approval_id.to_string()),
            request_id,
        );
    }

    pub fn untrack_approval_request(&self, session_id: &str, approval_id: &str) {
        self.pending_approval_requests
            .lock()
            .unwrap()
            .remove(&(session_id.to_string(), approval_id.to_string()));
    }

    pub fn track_user_input_request(
        &self,
        session_id: &str,
        user_input_id: &str,
        request_id: Value,
    ) {
        self.pending_user_input_requests.lock().unwrap().insert(
            (session_id.to_string(), user_input_id.to_string()),
            request_id,
        );
    }

    pub fn untrack_user_input_request(&self, session_id: &str, user_input_id: &str) {
        self.pending_user_input_requests
            .lock()
            .unwrap()
            .remove(&(session_id.to_string(), user_input_id.to_string()));
    }

    pub fn pending_interaction_request_ids(&self, session_id: &str) -> Vec<Value> {
        let approvals = self.pending_approval_requests.lock().unwrap();
        let user_inputs = self.pending_user_input_requests.lock().unwrap();
        approvals
            .keys()
            .chain(user_inputs.keys())
            .filter(|(session, _)| session == session_id)
            .filter_map(|(_, interaction_id)| {
                approvals
                    .get(&(session_id.to_string(), interaction_id.clone()))
                    .or_else(|| user_inputs.get(&(session_id.to_string(), interaction_id.clone())))
                    .cloned()
            })
            .collect()
    }

    pub fn set_client_capabilities(&self, capabilities: &Value) {
        let form = capabilities
            .get("elicitation")
            .and_then(|value| value.get("form"))
            .is_some_and(|value| !value.is_null());
        self.form_elicitation.store(form, Ordering::SeqCst);
    }

    pub fn session_exists(&self, session_id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(session_id)
    }

    /// Delete a durable Muse session through its compatibility-guarded index.
    ///
    /// MSP v1 has no `session/delete` method. Muse's installed session index is
    /// the only supported discovery source, so this adapter updates that index
    /// and its two owned storage trees while holding an immediate transaction.
    /// The index schema is checked explicitly rather than applying an unguarded
    /// SQL delete to a future database format.
    pub fn delete_durable_session(&self, session_id: &str) -> Result<(), RpcError> {
        uuid::Uuid::parse_str(session_id)
            .map_err(|_| RpcError::invalid_params("sessionId must be a Muse UUID"))?;
        let muse_home = self
            .muse_home
            .get()
            .map(String::as_str)
            .ok_or_else(|| RpcError::internal("Muse home is not initialized"))?;
        let database = Path::new(muse_home).join("session-index.db");
        let mut connection =
            Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|error| RpcError::internal(format!("open Muse session index: {error}")))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| RpcError::internal(format!("lock Muse session index: {error}")))?;

        let schema_version: String = connection
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| {
                RpcError::internal(format!("read Muse session index schema: {error}"))
            })?;
        let schema_version: i64 = schema_version
            .parse()
            .map_err(|_| RpcError::internal("Muse session index has an invalid schema version"))?;
        if schema_version != 1 {
            return Err(RpcError::internal(format!(
                "unsupported Muse session index schema v{schema_version}"
            )));
        }

        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| RpcError::internal(format!("begin session delete: {error}")))?;
        let (session_dir, session_stream_id): (String, String) = transaction
            .query_row(
                "SELECT session_dir, session_stream_id FROM sessions WHERE session_id = ?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| RpcError::invalid_params("session is not in Muse's durable index"))?;

        uuid::Uuid::parse_str(&session_stream_id).map_err(|_| {
            RpcError::internal("Muse session index returned an invalid session stream ID")
        })?;
        let sessions_root = Path::new(muse_home).join("sessions");
        let canonical_sessions_root = sessions_root.canonicalize().map_err(|error| {
            RpcError::internal(format!("resolve Muse sessions directory: {error}"))
        })?;
        let canonical_session_dir = Path::new(&session_dir).canonicalize().map_err(|error| {
            RpcError::internal(format!("resolve Muse session directory: {error}"))
        })?;
        if canonical_session_dir
            .file_name()
            .and_then(|name| name.to_str())
            != Some(session_id)
            || !canonical_session_dir.starts_with(&canonical_sessions_root)
        {
            return Err(RpcError::internal(
                "Muse session index returned an unsafe session directory",
            ));
        }

        if canonical_session_dir.exists()
            && std::fs::remove_dir_all(&canonical_session_dir).is_err()
        {
            return Err(RpcError::internal(
                "failed to remove Muse session directory",
            ));
        }
        let view_dir = canonical_sessions_root
            .join(".msp-view-v1")
            .join(&session_stream_id);
        if view_dir.exists() && std::fs::remove_dir_all(&view_dir).is_err() {
            return Err(RpcError::internal(
                "failed to remove Muse session view cache",
            ));
        }

        transaction
            .execute("DELETE FROM sessions WHERE session_id = ?1", [session_id])
            .and_then(|changed| {
                if changed == 1 {
                    Ok(())
                } else {
                    Err(rusqlite::Error::QueryReturnedNoRows)
                }
            })
            .map_err(|_| RpcError::internal("Muse session disappeared during delete"))?;
        transaction
            .commit()
            .map_err(|error| RpcError::internal(format!("commit session delete: {error}")))?;
        self.remove_session(session_id);
        Ok(())
    }

    pub fn approval_active(&self, session_id: &str, approval_id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(session_id)
            && self
                .pending_approvals
                .lock()
                .unwrap()
                .contains_key(&(session_id.to_string(), approval_id.to_string()))
    }

    pub fn user_input_active(&self, session_id: &str, user_input_id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(session_id)
            && self
                .pending_user_inputs
                .lock()
                .unwrap()
                .contains_key(&(session_id.to_string(), user_input_id.to_string()))
    }

    pub fn insert_session(&self, session_id: String, cwd: String, model: ModelSelection) {
        let reasoning_effort = self
            .sessions
            .lock()
            .unwrap()
            .get(&session_id)
            .map_or_else(ReasoningEffort::default, |state| state.reasoning_effort);
        self.sessions.lock().unwrap().insert(
            session_id,
            SessionState {
                cwd,
                active_turn: None,
                model,
                reasoning_effort,
            },
        );
    }

    pub fn remove_session(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
        self.item_kinds.lock().unwrap().remove(session_id);
        self.synthetic_user_commands
            .lock()
            .unwrap()
            .remove(session_id);
        self.turn_watchers
            .lock()
            .unwrap()
            .retain(|(session, _), _| session != session_id);
        self.finished_turns
            .lock()
            .unwrap()
            .retain(|(session, _), _| session != session_id);
        self.pending_approvals
            .lock()
            .unwrap()
            .retain(|(session, _), _| session != session_id);
        self.pending_user_inputs
            .lock()
            .unwrap()
            .retain(|(session, _), _| session != session_id);
        self.pending_approval_requests
            .lock()
            .unwrap()
            .retain(|(session, _), _| session != session_id);
        self.pending_user_input_requests
            .lock()
            .unwrap()
            .retain(|(session, _), _| session != session_id);
    }

    pub fn session_cwd(&self, session_id: &str) -> Result<String, RpcError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|session| session.cwd.clone())
            .ok_or_else(|| RpcError::invalid_params("unknown sessionId"))
    }

    pub fn update_model(&self, session_id: &str, model: ModelSelection) {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(session_id) {
            session.model = model;
        }
    }

    pub fn update_reasoning_effort(
        &self,
        session_id: &str,
        reasoning_effort: ReasoningEffort,
    ) -> Result<(), RpcError> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions
            .get_mut(session_id)
            .map(|session| session.reasoning_effort = reasoning_effort)
            .ok_or_else(|| RpcError::invalid_params("unknown sessionId"))
    }

    pub fn reasoning_effort(&self, session_id: &str) -> Result<ReasoningEffort, RpcError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|session| session.reasoning_effort)
            .ok_or_else(|| RpcError::invalid_params("unknown sessionId"))
    }

    pub fn set_active_turn(&self, session_id: &str, turn_id: Option<String>) {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(session_id) {
            session.active_turn = turn_id;
        }
    }

    pub fn active_turn(&self, session_id: &str) -> Option<String> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|session| session.active_turn.clone())
    }

    pub fn watch_turn(&self, session_id: &str, turn_id: &str) -> oneshot::Receiver<TurnOutcome> {
        let (tx, rx) = oneshot::channel();
        let key = (session_id.to_string(), turn_id.to_string());
        {
            // Keep the finished-turn lock across watcher registration so a
            // concurrent completion cannot fall between the two maps.
            let mut finished = self.finished_turns.lock().unwrap();
            if let Some(outcome) = finished.remove(&key) {
                let _ = tx.send(outcome);
                return rx;
            }
            self.turn_watchers
                .lock()
                .unwrap()
                .entry(key)
                .or_default()
                .push(tx);
        }
        rx
    }

    fn finish_turn(&self, session_id: &str, turn_id: &str, outcome: TurnOutcome) {
        let key = (session_id.to_string(), turn_id.to_string());
        let mut finished = self.finished_turns.lock().unwrap();
        {
            let watchers = self
                .turn_watchers
                .lock()
                .unwrap()
                .remove(&key)
                .unwrap_or_default();
            let had_watchers = !watchers.is_empty();
            for watcher in watchers {
                let _ = watcher.send(outcome.clone());
            }
            if !had_watchers && self.protocol_version.load(Ordering::SeqCst) == 1 {
                finished.insert(key, outcome);
            }
        }
    }

    pub fn host_closed(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let sessions: Vec<String> = self.sessions.lock().unwrap().keys().cloned().collect();
        for session_id in sessions {
            self.emit_update(
                &session_id,
                json!({
                    "sessionUpdate": "state_update",
                    "state": "idle",
                    "stopReason": "_msp_host_closed"
                }),
            );
            if let Some(turn_id) = self.active_turn(&session_id) {
                self.finish_turn(
                    &session_id,
                    &turn_id,
                    TurnOutcome::Failed("MSP host closed".into()),
                );
            }
        }
        let _ = self.acp_tx.send(AcpEvent::HostClosed);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn handle_notification(&self, method: String, params: Value) {
        if self.is_closed() {
            return;
        }
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        match method.as_str() {
            "turn/started" => {
                let turn_id = params
                    .get("turnId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.set_active_turn(&session_id, Some(turn_id.to_string()));
                self.emit_update(&session_id, state_update("running", None));
            }
            "item/started" | "item/updated" | "item/completed" => {
                if let Some(item) = params.get("item") {
                    if let (Some(item_id), Some(kind)) = (
                        item.get("itemId").and_then(Value::as_str),
                        item.get("kind").and_then(Value::as_str),
                    ) {
                        self.item_kinds
                            .lock()
                            .unwrap()
                            .entry(session_id.clone())
                            .or_default()
                            .insert(item_id.to_string(), kind.to_string());
                    }
                    if item.get("kind").and_then(Value::as_str) == Some("userMessage")
                        && let Some(command_id) = item.get("commandId").and_then(Value::as_str)
                        && self.is_synthetic_user_command(&session_id, command_id)
                    {
                        if method == "item/completed" {
                            self.clear_synthetic_user_command(&session_id, command_id);
                        }
                        return;
                    }
                    if let Some(update) = item_update(item) {
                        self.emit_update(&session_id, update);
                    }
                }
            }
            "item/delta" => {
                let item_id = params
                    .get("itemId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let field = params
                    .get("field")
                    .and_then(Value::as_str)
                    .unwrap_or("text");
                let delta = params
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let kind = self
                    .item_kinds
                    .lock()
                    .unwrap()
                    .get(&session_id)
                    .and_then(|items| items.get(item_id))
                    .cloned()
                    .unwrap_or_default();
                if let Some(update) = item_delta_update(&kind, item_id, field, delta) {
                    self.emit_update(&session_id, update);
                }
            }
            "approval/requested" | "approval/updated" => {
                if let Some(approval_id) = params.get("approvalId").and_then(Value::as_str) {
                    self.pending_approvals.lock().unwrap().insert(
                        (session_id.clone(), approval_id.to_string()),
                        params.clone(),
                    );
                }
                self.emit_update(&session_id, state_update("requires_action", None));
                let _ = self.acp_tx.send(AcpEvent::Permission {
                    session_id,
                    approval: params,
                });
            }
            "userInput/requested" => {
                if let Some(user_input_id) = params.get("userInputId").and_then(Value::as_str) {
                    self.pending_user_inputs.lock().unwrap().insert(
                        (session_id.clone(), user_input_id.to_string()),
                        params.clone(),
                    );
                }
                self.emit_update(&session_id, state_update("requires_action", None));
                let _ = self.acp_tx.send(AcpEvent::Elicitation {
                    session_id,
                    user_input: params,
                });
            }
            "approval/resolved" | "userInput/settled" => {
                if let Some(approval_id) = params.get("approvalId").and_then(Value::as_str) {
                    self.pending_approvals
                        .lock()
                        .unwrap()
                        .remove(&(session_id.clone(), approval_id.to_string()));
                }
                if let Some(user_input_id) = params.get("userInputId").and_then(Value::as_str) {
                    self.pending_user_inputs
                        .lock()
                        .unwrap()
                        .remove(&(session_id.clone(), user_input_id.to_string()));
                }
                self.emit_update(&session_id, state_update("running", None));
            }
            "session/todoListChanged" => {
                self.emit_update(&session_id, todo_update(&params));
            }
            "session/contextUsage" => {
                self.emit_update(&session_id, usage_update(&params));
            }
            "session/modelChanged" => {
                self.update_model(
                    &session_id,
                    ModelSelection {
                        provider_id: params
                            .get("providerId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        profile_id: params
                            .get("profileId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        model_id: params
                            .get("modelId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    },
                );
                self.emit_update(
                    &session_id,
                    json!({
                        "sessionUpdate": "config_option_update",
                        "configOptions": self.current_config_options(&session_id)
                    }),
                );
            }
            "turn/completed" => {
                let turn_id = params
                    .get("turnId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(command_id) = params.get("commandId").and_then(Value::as_str) {
                    self.clear_synthetic_user_command(&session_id, command_id);
                }
                let terminal = params
                    .get("terminal")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                let (stop_reason, outcome) = match terminal {
                    "completed" => ("end_turn", TurnOutcome::Completed),
                    "cancelled" => ("cancelled", TurnOutcome::Cancelled),
                    _ => (
                        "_msp_failed",
                        TurnOutcome::Failed(
                            params
                                .get("error")
                                .and_then(|error| error.get("message"))
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| "MSP turn failed".to_string()),
                        ),
                    ),
                };
                self.set_active_turn(&session_id, None);
                self.emit_update(&session_id, state_update("idle", Some(stop_reason)));
                self.finish_turn(&session_id, turn_id, outcome);
            }
            _ => {}
        }
    }

    pub fn current_config_options(&self, session_id: &str) -> Vec<Value> {
        let Some(state) = self.sessions.lock().unwrap().get(session_id).cloned() else {
            return Vec::new();
        };
        let version = self.protocol_version.load(Ordering::SeqCst);
        vec![
            normalize_config_option(config_option(state.model, Vec::new()), version),
            normalize_config_option(
                reasoning_effort_config_option(state.reasoning_effort),
                version,
            ),
        ]
    }

    pub async fn cancel_pending_interactions(&self, session_id: &str) {
        let approvals: Vec<(String, Value)> = {
            let pending = self.pending_approvals.lock().unwrap();
            pending
                .iter()
                .filter(|((pending_session, _), _)| pending_session == session_id)
                .map(|((_, approval_id), approval)| (approval_id.clone(), approval.clone()))
                .collect()
        };

        for (approval_id, approval) in approvals {
            let choice = approval
                .get("availableChoices")
                .and_then(Value::as_array)
                .and_then(|choices| {
                    choices.iter().find(|choice| {
                        matches!(
                            choice.get("decision").and_then(Value::as_str),
                            Some("abort") | Some("denied") | Some("timedOut")
                        )
                    })
                })
                .and_then(|choice| choice.get("choiceId"))
                .cloned();
            let Some(choice_id) = choice.as_ref().and_then(Value::as_str) else {
                self.pending_approvals
                    .lock()
                    .unwrap()
                    .remove(&(session_id.to_string(), approval_id.clone()));
                self.untrack_approval_request(session_id, &approval_id);
                continue;
            };

            let _ = self
                .host
                .request(
                    "approval/decide",
                    json!({
                        "commandId": msp::uuid_v7(),
                        "sessionId": session_id,
                        "approvalId": approval_id,
                        "choiceId": choice_id,
                        "requirementId": approval
                            .get("currentRequirementId")
                            .cloned()
                            .unwrap_or(Value::Null)
                    }),
                )
                .await;
            self.pending_approvals
                .lock()
                .unwrap()
                .remove(&(session_id.to_string(), approval_id.clone()));
            self.untrack_approval_request(session_id, &approval_id);
        }

        let user_inputs: Vec<(String, Value)> = {
            let pending = self.pending_user_inputs.lock().unwrap();
            pending
                .iter()
                .filter(|((pending_session, _), _)| pending_session == session_id)
                .map(|((_, user_input_id), user_input)| (user_input_id.clone(), user_input.clone()))
                .collect()
        };

        for (user_input_id, _) in user_inputs {
            let _ = self
                .host
                .request(
                    "userInput/cancel",
                    json!({
                        "commandId": msp::uuid_v7(),
                        "sessionId": session_id,
                        "userInputId": user_input_id,
                        "reason": "session cancelled"
                    }),
                )
                .await;
            self.pending_user_inputs
                .lock()
                .unwrap()
                .remove(&(session_id.to_string(), user_input_id.clone()));
            self.untrack_user_input_request(session_id, &user_input_id);
        }
    }
    pub async fn config_options(&self, session_id: &str) -> Result<Vec<Value>, RpcError> {
        let state = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RpcError::invalid_params("unknown sessionId"))?;
        let catalog = self
            .host
            .request("model/list", json!({ "sessionId": session_id }))
            .await?;
        let models = catalog
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let version = self.protocol_version.load(Ordering::SeqCst);
        Ok(vec![
            normalize_config_option(config_option(state.model, models), version),
            normalize_config_option(
                reasoning_effort_config_option(state.reasoning_effort),
                version,
            ),
        ])
    }
}

fn normalize_config_option(mut option: Value, protocol_version: u32) -> Value {
    let id = option.get("configId").cloned().unwrap_or(Value::Null);
    if protocol_version == 1 {
        option.as_object_mut().unwrap().remove("configId");
        option["id"] = id;
    }
    option
}

pub fn state_update(state: &str, stop_reason: Option<&str>) -> Value {
    let mut update = json!({
        "sessionUpdate": "state_update",
        "state": state
    });
    if let Some(stop_reason) = stop_reason {
        update["stopReason"] = json!(stop_reason);
    }
    update
}

pub fn config_option(current: ModelSelection, mut models: Vec<Value>) -> Value {
    let mut current_value = model_value(&current);
    if !current_value.is_empty()
        && !models.iter().any(|model| {
            model
                .get("modelId")
                .and_then(Value::as_str)
                .map(str::to_string)
                == current.model_id.clone()
        })
    {
        models.push(json!({
            "providerId": current.provider_id.clone().unwrap_or_default(),
            "profileId": current.profile_id.clone(),
            "modelId": current.model_id.clone().unwrap_or_default(),
            "displayLabel": current.model_id.clone().unwrap_or_else(|| "current model".to_string()),
            "isActive": true,
            "isDefault": false
        }));
    }

    let options: Vec<Value> = models
        .iter()
        .map(|model| {
            let value = model_value(&ModelSelection {
                provider_id: model.get("providerId").and_then(Value::as_str).map(str::to_string),
                profile_id: model.get("profileId").and_then(Value::as_str).map(str::to_string),
                model_id: model.get("modelId").and_then(Value::as_str).map(str::to_string),
            });
            json!({
                "value": value,
                "name": model.get("displayLabel").cloned().unwrap_or_else(|| json!("Unknown model")),
                "description": model.get("description").cloned().unwrap_or(Value::Null)
            })
        })
        .collect();

    if current_value.is_empty() {
        current_value = options
            .first()
            .and_then(|option| option.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }

    json!({
        "configId": "muse.model",
        "name": "Muse model",
        "description": "Model used by this Muse session",
        "category": "model_config",
        "type": "select",
        "currentValue": current_value,
        "options": options
    })
}

pub fn reasoning_effort_config_option(current: ReasoningEffort) -> Value {
    json!({
        "configId": "muse.reasoningEffort",
        "name": "Muse reasoning effort",
        "description": "Reasoning effort sent with every prompt for this session",
        "category": "thought_level",
        "type": "select",
        "currentValue": current.as_str(),
        "options": ReasoningEffort::ALL
            .into_iter()
            .map(|effort| {
                json!({
                    "value": effort.as_str(),
                    "name": effort.label(),
                    "description": effort.description()
                })
            })
            .collect::<Vec<_>>()
    })
}

pub fn model_value(model: &ModelSelection) -> String {
    if model.model_id.is_none() {
        return String::new();
    }
    format!(
        "{}:{}:{}",
        hex(model.provider_id.as_deref().unwrap_or("")),
        hex(model.profile_id.as_deref().unwrap_or("")),
        hex(model.model_id.as_deref().unwrap_or(""))
    )
}

pub fn decode_model_value(value: &str) -> Result<ModelSelection, RpcError> {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 3 {
        return Err(RpcError::invalid_params("invalid muse.model value"));
    }
    let decode =
        |part: &str| unhex(part).map_err(|_| RpcError::invalid_params("invalid muse.model value"));
    let nonempty = |value: String| if value.is_empty() { None } else { Some(value) };
    Ok(ModelSelection {
        provider_id: nonempty(decode(parts[0])?),
        profile_id: nonempty(decode(parts[1])?),
        model_id: nonempty(decode(parts[2])?),
    })
}

fn hex(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unhex(value: &str) -> Result<String, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd-length hex".to_string());
    }
    let mut out = Vec::with_capacity(value.len() / 2);
    let bytes = value.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16).ok_or("invalid hex")?;
        let low = (pair[1] as char).to_digit(16).ok_or("invalid hex")?;
        out.push((high * 16 + low) as u8);
    }
    String::from_utf8(out).map_err(|_| "invalid UTF-8".to_string())
}

pub fn selection_from_session(session: &Value) -> ModelSelection {
    ModelSelection {
        provider_id: session
            .get("providerId")
            .and_then(Value::as_str)
            .map(str::to_string),
        profile_id: None,
        model_id: session
            .get("modelId")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

pub fn convert_prompt(prompt: &Value, cwd: &str) -> Result<Vec<Value>, RpcError> {
    let blocks = prompt
        .as_array()
        .ok_or_else(|| RpcError::invalid_params("prompt must be an array"))?;
    let mut parts = Vec::new();
    for block in blocks {
        match block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text" => {
                let text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::invalid_params("text block requires text"))?;
                parts.push(json!({ "type": "text", "text": text }));
            }
            "image" => {
                let data = block
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::invalid_params("image block requires data"))?;
                let media_type = block
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::invalid_params("image block requires mimeType"))?;
                parts.push(json!({
                    "type": "image",
                    "mediaType": media_type,
                    "base64Data": data
                }));
            }
            "resource_link" => {
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("resource");
                let uri = block
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::invalid_params("resource_link requires uri"))?;
                parts.push(json!({
                    "type": "text",
                    "text": resource_mention(name, uri, cwd)
                }));
            }
            other => {
                return Err(RpcError::invalid_params(format!(
                    "unsupported prompt content type '{other}'"
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(RpcError::invalid_params("prompt must not be empty"));
    }
    Ok(parts)
}

fn resource_mention(name: &str, uri: &str, cwd: &str) -> String {
    if let Some(path) = uri.strip_prefix("file://") {
        let path = percent_decode(path);
        let cwd = cwd.trim_end_matches('/');
        if let Some(relative) = path.strip_prefix(&format!("{cwd}/")) {
            return format!("@{relative}");
        }
    }
    format!("{name}: {uri}")
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn item_update(item: &Value) -> Option<Value> {
    let item_id = item.get("itemId")?.as_str()?;
    let kind = item.get("kind")?.as_str()?;
    let text = item
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            item.get("summary")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        });

    match kind {
        "userMessage" => Some(json!({
            "sessionUpdate": "user_message",
            "messageId": item_id,
            "content": [{"type": "text", "text": text}]
        })),
        "agentMessage" => Some(json!({
            "sessionUpdate": "agent_message",
            "messageId": item_id,
            "content": [{"type": "text", "text": text}]
        })),
        "reasoning" => Some(json!({
            "sessionUpdate": "agent_thought",
            "messageId": item_id,
            "content": [{"type": "text", "text": text}]
        })),
        "toolCall" | "userShell" | "subagent" | "workflow" | "reminderChild" => {
            Some(tool_update(item_id, item))
        }
        _ => None,
    }
}

pub fn item_delta_update(kind: &str, item_id: &str, field: &str, delta: &str) -> Option<Value> {
    if delta.is_empty() {
        return None;
    }
    match kind {
        "userMessage" => Some(json!({
            "sessionUpdate": "user_message_chunk",
            "messageId": item_id,
            "content": {"type": "text", "text": delta}
        })),
        "agentMessage" => Some(json!({
            "sessionUpdate": "agent_message_chunk",
            "messageId": item_id,
            "content": {"type": "text", "text": delta}
        })),
        "reasoning" if field == "text" || field.starts_with("summary.") => Some(json!({
            "sessionUpdate": "agent_thought_chunk",
            "messageId": item_id,
            "content": {"type": "text", "text": delta}
        })),
        "toolCall" | "userShell" | "subagent" | "workflow" | "reminderChild"
            if field == "output" || field == "visibleOutput" =>
        {
            Some(json!({
                "sessionUpdate": "tool_call_content_chunk",
                "toolCallId": item_id,
                "content": {
                    "type": "content",
                    "content": {"type": "text", "text": delta}
                }
            }))
        }
        _ => None,
    }
}

fn tool_update(item_id: &str, item: &Value) -> Value {
    let tool_name = item
        .get("tool")
        .and_then(Value::as_str)
        .or_else(|| item.get("commandText").and_then(Value::as_str))
        .unwrap_or("Muse activity");
    let status = match item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("inProgress")
    {
        "inProgress" => "in_progress",
        "completed" => "completed",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    let output = item
        .get("visibleOutput")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let raw_input = item
        .get("args")
        .and_then(Value::as_str)
        .map(|value| serde_json::from_str::<Value>(value).unwrap_or_else(|_| json!(value)))
        .unwrap_or(Value::Null);

    json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": item_id,
        "title": tool_name,
        "kind": tool_kind(tool_name),
        "status": status,
        "rawInput": raw_input,
        "rawOutput": output,
        "content": [{
            "type": "content",
            "content": {"type": "text", "text": output}
        }]
    })
}

fn tool_kind(tool: &str) -> &'static str {
    let tool = tool.to_ascii_lowercase();
    if tool.contains("read") || tool.contains("view") {
        "read"
    } else if tool.contains("edit") || tool.contains("write") || tool.contains("patch") {
        "edit"
    } else if tool.contains("delete") || tool.contains("remove") {
        "delete"
    } else if tool.contains("move") || tool.contains("rename") {
        "move"
    } else if tool.contains("search") || tool.contains("grep") || tool.contains("find") {
        "search"
    } else if tool.contains("shell") || tool.contains("exec") || tool.contains("bash") {
        "execute"
    } else if tool.contains("fetch") || tool.contains("http") || tool.contains("web") {
        "fetch"
    } else if tool.contains("think") || tool.contains("plan") {
        "think"
    } else {
        "other"
    }
}

pub fn todo_update(params: &Value) -> Value {
    let entries = params
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let status = match item.get("status").and_then(Value::as_str) {
                        Some("inProgress") => "in_progress",
                        Some("completed") => "completed",
                        Some("cancelled") => "cancelled",
                        _ => "pending",
                    };
                    json!({
                        "content": item.get("activeForm").cloned().unwrap_or_else(|| item.get("text").cloned().unwrap_or(json!(""))),
                        "priority": "medium",
                        "status": status
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "sessionUpdate": "plan_update",
        "plan": {
            "type": "items",
            "planId": "muse.todos",
            "entries": entries
        }
    })
}

pub fn usage_update(params: &Value) -> Value {
    json!({
        "sessionUpdate": "usage_update",
        "used": params.get("usedTokens").cloned().unwrap_or(json!(0)),
        "size": params.get("windowTokens").cloned().unwrap_or(json!(0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_values_round_trip_through_colons_and_unicode() {
        let model = ModelSelection {
            provider_id: Some("provider:one".into()),
            profile_id: Some("profilé".into()),
            model_id: Some("model:two".into()),
        };
        let encoded = model_value(&model);
        assert_eq!(decode_model_value(&encoded).unwrap(), model);
        assert!(decode_model_value("not-a-model-value").is_err());
        assert!(decode_model_value("zz:zz:zz").is_err());
    }

    #[test]
    fn reasoning_effort_config_is_always_selected() {
        assert_eq!(ReasoningEffort::default(), ReasoningEffort::Medium);
        for effort in ReasoningEffort::ALL {
            assert_eq!(ReasoningEffort::parse(effort.as_str()).unwrap(), effort);
        }
        assert!(ReasoningEffort::parse("extreme").is_err());

        let option = reasoning_effort_config_option(ReasoningEffort::High);
        assert_eq!(option["configId"], "muse.reasoningEffort");
        assert_eq!(option["category"], "thought_level");
        assert_eq!(option["currentValue"], "high");
        assert_eq!(option["options"].as_array().unwrap().len(), 7);
    }

    #[test]
    fn converts_prompt_content_to_msp_turn_input() {
        let prompt = json!([
            {"type": "text", "text": "look at"},
            {
                "type": "image",
                "data": "aGVsbG8=",
                "mimeType": "image/png"
            },
            {
                "type": "resource_link",
                "name": "notes",
                "uri": "file:///workspace/src/notes%20.md"
            }
        ]);
        let parts = convert_prompt(&prompt, "/workspace").unwrap();
        assert_eq!(parts[0], json!({"type": "text", "text": "look at"}));
        assert_eq!(
            parts[1],
            json!({
                "type": "image",
                "mediaType": "image/png",
                "base64Data": "aGVsbG8="
            })
        );
        assert_eq!(parts[2]["text"], "@src/notes .md");

        assert!(convert_prompt(&json!([]), "/tmp").is_err());
        assert!(
            convert_prompt(
                &json!([{"type": "audio", "data": "", "mimeType": ""}]),
                "/tmp"
            )
            .is_err()
        );
    }

    #[test]
    fn maps_message_and_tool_items_to_acp_updates() {
        let message = item_update(&json!({
            "itemId": "message-1",
            "kind": "agentMessage",
            "status": "completed",
            "text": "done"
        }))
        .unwrap();
        assert_eq!(message["sessionUpdate"], "agent_message");
        assert_eq!(message["messageId"], "message-1");
        assert_eq!(message["content"][0]["text"], "done");

        let tool = item_update(&json!({
            "itemId": "tool-1",
            "kind": "toolCall",
            "status": "failed",
            "tool": "read_file",
            "args": "{\"path\":\"src/main.rs\"}",
            "visibleOutput": "not found"
        }))
        .unwrap();
        assert_eq!(tool["sessionUpdate"], "tool_call_update");
        assert_eq!(tool["kind"], "read");
        assert_eq!(tool["status"], "failed");
        assert_eq!(tool["rawInput"]["path"], "src/main.rs");
    }

    #[test]
    fn maps_item_deltas_to_acp_chunks() {
        let update = item_delta_update("agentMessage", "message-1", "text", "chunk").unwrap();
        assert_eq!(update["sessionUpdate"], "agent_message_chunk");
        assert_eq!(update["content"]["text"], "chunk");

        let update = item_delta_update("toolCall", "tool-1", "output", "output").unwrap();
        assert_eq!(update["sessionUpdate"], "tool_call_content_chunk");
        assert_eq!(update["content"]["content"]["text"], "output");

        assert!(item_delta_update("agentMessage", "message-1", "text", "").is_none());
        assert!(item_delta_update("compaction", "item-1", "text", "ignored").is_none());
    }

    #[test]
    fn maps_todo_and_context_usage() {
        let todo = todo_update(&json!({
            "items": [
                {"text": "first", "status": "completed"},
                {"text": "second", "activeForm": "running second", "status": "inProgress"}
            ]
        }));
        assert_eq!(todo["sessionUpdate"], "plan_update");
        assert_eq!(todo["plan"]["entries"][0]["status"], "completed");
        assert_eq!(todo["plan"]["entries"][1]["content"], "running second");
        assert_eq!(todo["plan"]["entries"][1]["status"], "in_progress");

        let usage = usage_update(&json!({
            "usedTokens": 12,
            "windowTokens": 100
        }));
        assert_eq!(usage["sessionUpdate"], "usage_update");
        assert_eq!(usage["used"], 12);
        assert_eq!(usage["size"], 100);
    }
}
