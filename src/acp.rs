use crate::gateway::{self, Gateway, TurnOutcome};
use crate::msp::{self, RpcError};
use agent_client_protocol::{
    Agent, ConnectTo, ConnectionTo, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, Stdio,
    on_receive_notification, on_receive_request,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ACP wire types. The runtime supplies framing, batches, cancellation, and
// request/response correlation; these payloads keep the gateway's wire surface
// explicit while remaining directly testable against the published schema.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "initialize", response = InitializeResponse)]
struct InitializeRequest {
    #[serde(rename = "protocolVersion")]
    protocol_version: u32,
    #[serde(default)]
    capabilities: Value,
    #[serde(default, rename = "clientCapabilities")]
    client_capabilities: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct InitializeResponse {
    #[serde(rename = "protocolVersion")]
    protocol_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    info: Option<Value>,
    #[serde(rename = "agentInfo", default, skip_serializing_if = "Option::is_none")]
    agent_info: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capabilities: Option<Value>,
    #[serde(rename = "agentCapabilities", skip_serializing_if = "Option::is_none")]
    agent_capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "authMethods")]
    auth_methods: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    _meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/new", response = NewSessionResponse)]
struct NewSessionRequest {
    cwd: String,
    #[serde(default, rename = "additionalDirectories")]
    additional_directories: Vec<String>,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct NewSessionResponse {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "configOptions"
    )]
    config_options: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/list", response = ListSessionsResponse)]
struct ListSessionsRequest {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct ListSessionsResponse {
    sessions: Vec<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "nextCursor"
    )]
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/resume", response = ResumeSessionResponse)]
struct ResumeSessionRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: String,
    #[serde(default, rename = "replayFrom")]
    replay_from: Option<Value>,
    #[serde(default, rename = "additionalDirectories")]
    additional_directories: Vec<String>,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/load", response = ResumeSessionResponse)]
struct LoadSessionRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: String,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonRpcResponse)]
struct ResumeSessionResponse {
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "configOptions"
    )]
    config_options: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/close", response = EmptyResponse)]
struct CloseSessionRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/delete", response = EmptyResponse)]
struct DeleteSessionRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/set_config_option", response = SetConfigResponse)]
struct SetConfigRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "configId")]
    config_id: String,
    value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct SetConfigResponse {
    #[serde(rename = "configOptions")]
    config_options: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/prompt", response = PromptResponse)]
struct PromptRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
    prompt: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "authenticate", response = EmptyResponse)]
struct AuthenticateRequest {
    #[serde(rename = "methodId")]
    method_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "logout", response = EmptyResponse)]
struct LogoutRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "auth/login", response = EmptyResponse)]
struct AuthLoginRequest {
    #[serde(rename = "methodId")]
    method_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "auth/logout", response = EmptyResponse)]
struct AuthLogoutRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct PromptResponse {
    #[serde(
        default,
        rename = "stopReason",
        skip_serializing_if = "Option::is_none"
    )]
    stop_reason: Option<String>,
}

/// The deployed ACP steering extension used by the Codex and Claude agents.
/// Steering is not part of the pinned ACP core schema, so the method is
/// underscore-prefixed as required by ACP's extension rules.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "_session/steering", response = SteeringResponse)]
struct SteeringRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
    prompt: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    _meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct SteeringResponse {
    outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct EmptyResponse {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "session/cancel")]
struct CancelNotification {
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "$/cancel_request")]
struct CancelClientRequestNotification {
    #[serde(rename = "requestId")]
    request_id: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "session/update")]
struct SessionUpdateNotification {
    #[serde(rename = "sessionId")]
    session_id: String,
    update: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "session/request_permission", response = PermissionResponse)]
struct PermissionRequest {
    #[serde(rename = "sessionId")]
    session_id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<Value>,
    options: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct PermissionResponse {
    outcome: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "elicitation/create", response = ElicitationResponse)]
struct ElicitationRequest {
    message: String,
    mode: Value,
    #[serde(rename = "requestedSchema")]
    requested_schema: Value,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "toolCallId", skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct ElicitationResponse {
    action: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
}

type ClientConnection = ConnectionTo<agent_client_protocol::Client>;

pub async fn serve(gateway: Arc<Gateway>) -> Result<(), agent_client_protocol::Error> {
    let router = Agent
        .protocol_router()
        .with_v1(agent_builder(1, gateway.clone()))
        .with_v2(agent_builder(2, gateway.clone()));
    router.connect_to(Stdio::new()).await
}

fn agent_builder(
    version: u32,
    gateway: Arc<Gateway>,
) -> impl agent_client_protocol::ConnectTo<agent_client_protocol::Client> {
    let initialize_gateway = gateway.clone();
    let new_gateway = gateway.clone();
    let prompt_gateway = gateway.clone();
    let cancel_gateway = gateway.clone();
    let list_gateway = gateway.clone();
    let resume_gateway = gateway.clone();
    let load_gateway = gateway.clone();
    let close_gateway = gateway.clone();
    let config_gateway = gateway.clone();
    let background_gateway = gateway.clone();
    let steering_gateway = gateway.clone();
    let delete_gateway = gateway.clone();
    let authenticate_gateway = gateway.clone();
    let logout_gateway = gateway.clone();
    let auth_login_gateway = gateway.clone();
    let auth_logout_gateway = gateway.clone();

    let builder = if version == 2 {
        Agent.v2()
    } else {
        Agent.builder()
    };

    builder
        .on_receive_request(
            async move |request: InitializeRequest, responder, _| {
                let response =
                    initialize(initialize_gateway.clone(), version, request).map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, _| {
                let response = new_session(new_gateway.clone(), request)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, cx| {
                cancel_session(cancel_gateway.clone(), notification.session_id, &cx)
                    .await
                    .map_err(acp_error)?;
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, cx| {
                let response = prompt(prompt_gateway.clone(), request, &cx)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SteeringRequest, responder, cx| {
                let response = steer(steering_gateway.clone(), request, &cx)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoadSessionRequest, responder, _| {
                let response = load_session(load_gateway.clone(), request)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ListSessionsRequest, responder, _| {
                let response = list_sessions(list_gateway.clone(), request)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ResumeSessionRequest, responder, cx| {
                let response = resume_session(resume_gateway.clone(), request, &cx)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CloseSessionRequest, responder, cx| {
                let response = close_session(close_gateway.clone(), request, &cx)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeleteSessionRequest, responder, cx| {
                let response = delete_session(delete_gateway.clone(), request, &cx)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetConfigRequest, responder, _| {
                let response = set_config(config_gateway.clone(), request)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: AuthenticateRequest, responder, _| {
                let response = authenticate(authenticate_gateway.clone(), request)
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: LogoutRequest, responder, _| {
                let response = logout(logout_gateway.clone()).await.map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: AuthLoginRequest, responder, _| {
                let response = authenticate(
                    auth_login_gateway.clone(),
                    AuthenticateRequest {
                        method_id: request.method_id,
                    },
                )
                .await
                .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: AuthLogoutRequest, responder, _| {
                let response = logout(auth_logout_gateway.clone())
                    .await
                    .map_err(acp_error)?;
                responder.respond(response)
            },
            on_receive_request!(),
        )
        .with_spawned(move |cx| async move {
            let Some(mut receiver) = background_gateway.take_acp_receiver() else {
                return Ok(());
            };
            while let Some(event) = receiver.recv().await {
                match event {
                    gateway::AcpEvent::Update { session_id, update } => {
                        send_update(&cx, &session_id, update)?;
                    }
                    gateway::AcpEvent::Permission {
                        session_id,
                        approval,
                    } => {
                        handle_permission(background_gateway.clone(), &cx, &session_id, &approval)
                            .await?;
                    }
                    gateway::AcpEvent::Elicitation {
                        session_id,
                        user_input,
                    } => {
                        handle_elicitation(
                            background_gateway.clone(),
                            &cx,
                            &session_id,
                            &user_input,
                        )
                        .await?;
                    }
                    gateway::AcpEvent::HostClosed => {
                        return Err(acp_error(msp::RpcError::internal(
                            "MSP host closed unexpectedly",
                        )));
                    }
                }
            }
            Ok::<(), agent_client_protocol::Error>(())
        })
}

fn initialize(
    gateway: Arc<Gateway>,
    version: u32,
    request: InitializeRequest,
) -> Result<InitializeResponse, RpcError> {
    if request.protocol_version != version {
        return Err(RpcError::invalid_params(format!(
            "protocol router selected ACP v{version} for request v{}",
            request.protocol_version
        )));
    }
    let version = request.protocol_version;
    gateway
        .protocol_version
        .store(version, std::sync::atomic::Ordering::SeqCst);
    let capabilities =
        if request.client_capabilities.is_null() || request.client_capabilities == json!({}) {
            request.capabilities
        } else {
            request.client_capabilities
        };
    gateway.set_client_capabilities(&capabilities);

    let info = json!({
        "name": "muse-acp",
        "title": "Muse ACP gateway",
        "version": env!("CARGO_PKG_VERSION")
    });
    let terminal_auth = client_supports_terminal_auth(&capabilities);
    let mut auth_methods = Vec::new();
    if terminal_auth {
        auth_methods.push(if version == 1 {
            json!({
                "id": "muse-login",
                "type": "terminal",
                "name": "Muse account",
                "description": "Run muse login in an interactive terminal",
                "args": ["login"]
            })
        } else {
            json!({
                "methodId": "muse-login",
                "type": "terminal",
                "name": "Muse account",
                "description": "Run muse login in an interactive terminal",
                "args": ["login"]
            })
        });
    }

    if version == 1 {
        Ok(InitializeResponse {
            protocol_version: 1,
            info: Some(info),
            agent_info: None,
            capabilities: Some(json!({
                "loadSession": true,
                "promptCapabilities": {
                    "text": true,
                    "image": true,
                    "audio": false,
                    "embeddedContext": false
                },
                "mcpCapabilities": {
                    "http": false,
                    "sse": false
                },
                "sessionCapabilities": {
                    "list": {},
                    "close": {},
                    "delete": {}
                },
                "auth": {
                    "logout": if terminal_auth { json!({}) } else { Value::Null }
                }
            })),
            agent_capabilities: None,
            auth_methods,
            _meta: Some(json!({
                "steering": {
                    "supported": true
                }
            })),
        })
    } else {
        Ok(InitializeResponse {
            protocol_version: 2,
            info: None,
            agent_info: Some(info),
            agent_capabilities: Some(json!({
                "session": {
                    "prompt": {
                        "image": {}
                    },
                    "list": {},
                    "resume": {},
                    "close": {},
                    "delete": {}
                }
            })),
            capabilities: None,
            auth_methods,
            _meta: Some(json!({
                "steering": {
                    "supported": true
                }
            })),
        })
    }
}

fn client_supports_terminal_auth(capabilities: &Value) -> bool {
    capabilities
        .get("auth")
        .and_then(|auth| auth.get("terminal"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || capabilities
            .get("_meta")
            .and_then(|meta| meta.get("terminal-auth"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

async fn new_session(
    gateway: Arc<Gateway>,
    request: NewSessionRequest,
) -> Result<NewSessionResponse, RpcError> {
    validate_workspace_request(
        &request.cwd,
        &request.additional_directories,
        &request.mcp_servers,
    )?;
    let mut start = json!({
        "commandId": msp::uuid_v7(),
        "workspaceRoot": request.cwd
    });
    if let Ok(provider) = std::env::var("MUSE_PROVIDER") {
        start["providerId"] = json!(provider);
    }
    if let Ok(model) = std::env::var("MUSE_MODEL") {
        start["modelId"] = json!(model);
    }
    let result = gateway.host.request("session/start", start).await?;
    let Some(session) = result.get("session") else {
        return Err(RpcError::internal("MSP session/start returned no session"));
    };
    let Some(session_id) = session.get("sessionId").and_then(Value::as_str) else {
        return Err(RpcError::internal("MSP session has no sessionId"));
    };
    let model = gateway::selection_from_session(session);
    gateway.insert_session(session_id.to_string(), request.cwd, model);
    let config = gateway
        .config_options(session_id)
        .await
        .unwrap_or_else(|_| gateway.current_config_options(session_id));
    Ok(NewSessionResponse {
        session_id: session_id.to_string(),
        config_options: config,
    })
}

async fn prompt(
    gateway: Arc<Gateway>,
    request: PromptRequest,
    cx: &ClientConnection,
) -> Result<PromptResponse, RpcError> {
    let cwd = gateway.session_cwd(&request.session_id)?;
    let input = gateway::convert_prompt(&request.prompt, &cwd)?;
    let reasoning_effort = gateway.reasoning_effort(&request.session_id)?;
    let command_id = msp::uuid_v7();
    gateway.mark_synthetic_user_command(&request.session_id, &command_id);
    let result = match gateway
        .host
        .request(
            "turn/start",
            json!({
                "commandId": command_id,
                "sessionId": request.session_id,
                "input": input,
                "reasoningEffort": reasoning_effort.as_str()
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            gateway.clear_synthetic_user_command(&request.session_id, &command_id);
            return Err(error);
        }
    };
    let Some(turn_id) = result.get("turnId").and_then(Value::as_str) else {
        gateway.clear_synthetic_user_command(&request.session_id, &command_id);
        return Err(RpcError::internal("MSP turn/start returned no turnId"));
    };
    if result.get("disposition").and_then(Value::as_str) == Some("started") {
        gateway.set_active_turn(&request.session_id, Some(turn_id.to_string()));
    }
    send_update(
        cx,
        &request.session_id,
        json!({
            "sessionUpdate": "user_message",
            "messageId": format!("user-{turn_id}"),
            "content": request.prompt
        }),
    )
    .map_err(|error| {
        gateway.clear_synthetic_user_command(&request.session_id, &command_id);
        RpcError::internal(error.to_string())
    })?;
    if gateway
        .protocol_version
        .load(std::sync::atomic::Ordering::SeqCst)
        == 1
    {
        let mut watcher = gateway.watch_turn(&request.session_id, turn_id);
        return match (&mut watcher).await {
            Ok(TurnOutcome::Completed) => Ok(PromptResponse {
                stop_reason: Some("end_turn".to_string()),
            }),
            Ok(TurnOutcome::Cancelled) => Ok(PromptResponse {
                stop_reason: Some("cancelled".to_string()),
            }),
            Ok(TurnOutcome::Failed(message)) => Err(RpcError::internal(message)),
            Err(_) => Err(RpcError::internal("turn watcher closed")),
        };
    }
    Ok(PromptResponse { stop_reason: None })
}

async fn steer(
    gateway: Arc<Gateway>,
    request: SteeringRequest,
    cx: &ClientConnection,
) -> Result<SteeringResponse, RpcError> {
    let cwd = gateway.session_cwd(&request.session_id)?;
    let input = gateway::convert_prompt(&request.prompt, &cwd)?;
    let reasoning_effort = gateway.reasoning_effort(&request.session_id)?;
    let prompt_required = steering_requests_prompt_required(&request._meta)?;

    let active_turn_id = gateway.active_turn(&request.session_id);
    if active_turn_id.is_none() && prompt_required {
        return Ok(SteeringResponse {
            outcome: "promptRequired".to_string(),
            reason: Some("noRunningTurn".to_string()),
        });
    }

    let command_id = msp::uuid_v7();
    gateway.mark_synthetic_user_command(&request.session_id, &command_id);

    let result = if let Some(expected_turn_id) = active_turn_id {
        match gateway
            .host
            .request(
                "turn/steer",
                json!({
                    "commandId": command_id,
                    "sessionId": request.session_id,
                    "expectedTurnId": expected_turn_id,
                    "input": input,
                    "reasoningEffort": reasoning_effort.as_str()
                }),
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                gateway.clear_synthetic_user_command(&request.session_id, &command_id);
                return Err(error);
            }
        }
    } else {
        // Preserve the deployed extension's compatibility fallback. `steer`
        // asks MSP to steer if a turn begins after the idle check, avoiding a
        // check-then-start race without silently queueing a fresh prompt.
        match gateway
            .host
            .request(
                "turn/start",
                json!({
                    "commandId": command_id,
                    "sessionId": request.session_id,
                    "input": input,
                    "ifBusy": "steer",
                    "reasoningEffort": reasoning_effort.as_str()
                }),
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                gateway.clear_synthetic_user_command(&request.session_id, &command_id);
                return Err(error);
            }
        }
    };

    let Some(turn_id) = result.get("turnId").and_then(Value::as_str) else {
        gateway.clear_synthetic_user_command(&request.session_id, &command_id);
        return Err(RpcError::internal(
            "MSP steering response returned no turnId",
        ));
    };
    let disposition = result.get("disposition").and_then(Value::as_str);
    let outcome = if disposition == Some("steered") || disposition.is_none() {
        "injected"
    } else if disposition == Some("started") {
        "startedNewTurn"
    } else {
        gateway.clear_synthetic_user_command(&request.session_id, &command_id);
        return Err(RpcError::internal(format!(
            "MSP returned unexpected steering disposition '{}'",
            disposition.unwrap_or_default()
        )));
    };
    if disposition == Some("started") {
        gateway.set_active_turn(&request.session_id, Some(turn_id.to_string()));
    }
    send_update(
        cx,
        &request.session_id,
        json!({
            "sessionUpdate": "user_message",
            "messageId": format!("user-{command_id}"),
            "content": request.prompt
        }),
    )
    .map_err(|error| RpcError::internal(error.to_string()))?;

    Ok(SteeringResponse {
        outcome: outcome.to_string(),
        reason: None,
    })
}

fn steering_requests_prompt_required(meta: &Option<Value>) -> Result<bool, RpcError> {
    let Some(meta) = meta else {
        return Ok(false);
    };
    let Some(meta) = meta.as_object() else {
        return Err(RpcError::invalid_params("steering _meta must be an object"));
    };
    let Some(steering) = meta.get("steering") else {
        return Ok(false);
    };
    let Some(steering) = steering.as_object() else {
        return Err(RpcError::invalid_params(
            "steering _meta.steering must be an object",
        ));
    };
    match steering.get("idleBehavior").and_then(Value::as_str) {
        None => Ok(false),
        Some("promptRequired") => Ok(true),
        Some(_) => Err(RpcError::invalid_params(
            "unsupported steering idleBehavior",
        )),
    }
}

async fn list_sessions(
    gateway: Arc<Gateway>,
    request: ListSessionsRequest,
) -> Result<ListSessionsResponse, RpcError> {
    let mut params = json!({ "limit": 200 });
    if let Some(cwd) = request.cwd {
        params["workspaceRoot"] = json!(cwd);
    }
    if let Some(cursor) = request.cursor {
        params["cursor"] = json!(cursor);
    }
    let result = gateway.host.request("session/list", params).await?;
    let sessions = result
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|session| {
            json!({
                "sessionId": session.get("sessionId").cloned().unwrap_or(Value::Null),
                "cwd": session.get("workspaceRoot").cloned().unwrap_or(Value::Null),
                "updatedAt": session.get("updatedAt").cloned().unwrap_or(Value::Null)
            })
        })
        .collect();
    Ok(ListSessionsResponse {
        sessions,
        next_cursor: result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

async fn resume_session(
    gateway: Arc<Gateway>,
    request: ResumeSessionRequest,
    cx: &ClientConnection,
) -> Result<ResumeSessionResponse, RpcError> {
    validate_workspace_request(
        &request.cwd,
        &request.additional_directories,
        &request.mcp_servers,
    )?;
    let replay = request
        .replay_from
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !replay.is_empty() && replay != "start" {
        return Err(RpcError::invalid_params("unsupported replayFrom cursor"));
    }

    let result = gateway
        .host
        .request(
            "session/resume",
            json!({
                "commandId": msp::uuid_v7(),
                "sessionId": request.session_id,
                "history": "inline"
            }),
        )
        .await?;
    let Some(session) = result.get("session") else {
        return Err(RpcError::internal("MSP session/resume returned no session"));
    };
    let stored_cwd = session
        .get("workspaceRoot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if stored_cwd != request.cwd {
        return Err(RpcError::invalid_params(
            "cwd does not match stored session workspace",
        ));
    }
    gateway.insert_session(
        request.session_id.clone(),
        request.cwd,
        gateway::selection_from_session(session),
    );

    if replay == "start" {
        let history = result.get("history").cloned().unwrap_or(json!({}));
        let inline_items = history.get("items").and_then(Value::as_array).cloned();
        let snapshot_items = history
            .get("snapshot")
            .and_then(|snapshot| snapshot.get("state"))
            .and_then(|state| state.get("items"))
            .and_then(Value::as_array)
            .cloned();
        if let Some(items) = inline_items.or(snapshot_items) {
            for item in items {
                if let Some(update) = gateway::item_update(&item) {
                    send_update(cx, &request.session_id, update)
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                }
            }
        } else {
            replay_paged(gateway.clone(), &request.session_id, cx).await?;
        }
        rehydrate_pending(gateway.clone(), &request.session_id).await?;
    }

    let config = gateway
        .config_options(&request.session_id)
        .await
        .unwrap_or_default();
    Ok(ResumeSessionResponse {
        config_options: config,
    })
}

async fn load_session(
    gateway: Arc<Gateway>,
    request: LoadSessionRequest,
) -> Result<ResumeSessionResponse, RpcError> {
    if !request.mcp_servers.is_empty() {
        return Err(RpcError::invalid_params("MCP servers are not supported"));
    }
    let result = gateway
        .host
        .request(
            "session/resume",
            json!({
                "commandId": msp::uuid_v7(),
                "sessionId": request.session_id,
                "history": "inline"
            }),
        )
        .await?;
    let Some(session) = result.get("session") else {
        return Err(RpcError::internal("MSP session/resume returned no session"));
    };
    let stored_cwd = session
        .get("workspaceRoot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if stored_cwd != request.cwd {
        return Err(RpcError::invalid_params(
            "cwd does not match stored session workspace",
        ));
    }
    gateway.insert_session(
        request.session_id.clone(),
        request.cwd,
        gateway::selection_from_session(session),
    );
    let config = gateway
        .config_options(&request.session_id)
        .await
        .unwrap_or_else(|_| gateway.current_config_options(&request.session_id));
    Ok(ResumeSessionResponse {
        config_options: config,
    })
}

async fn replay_paged(
    gateway: Arc<Gateway>,
    session_id: &str,
    cx: &ClientConnection,
) -> Result<(), RpcError> {
    let mut cursor: Option<String> = None;
    loop {
        let mut params = json!({
            "sessionId": session_id,
            "limit": 1000,
            "direction": "forward"
        });
        if let Some(value) = cursor {
            params["cursor"] = json!(value);
        }
        let result = gateway.host.request("view/page", params).await?;
        let events = result
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for event in events {
            if event.get("method").and_then(Value::as_str) == Some("item/completed")
                && let Some(item) = event.get("params").and_then(|params| params.get("item"))
                && let Some(update) = gateway::item_update(item)
            {
                send_update(cx, session_id, update)
                    .map_err(|error| RpcError::internal(error.to_string()))?;
            }
        }
        let next = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next.is_none() {
            break;
        }
        cursor = next;
    }
    Ok(())
}

async fn rehydrate_pending(gateway: Arc<Gateway>, session_id: &str) -> Result<(), RpcError> {
    let pending = gateway
        .host
        .request("approval/listPending", json!({ "sessionId": session_id }))
        .await?;
    for approval in pending
        .get("approvals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        gateway.emit_permission(session_id, approval);
    }
    for user_input in pending
        .get("userInputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        gateway.emit_elicitation(session_id, user_input);
    }
    Ok(())
}

async fn close_session(
    gateway: Arc<Gateway>,
    request: CloseSessionRequest,
    cx: &ClientConnection,
) -> Result<EmptyResponse, RpcError> {
    if !gateway.session_exists(&request.session_id) {
        return Err(RpcError::invalid_params("unknown sessionId"));
    }
    cancel_pending_interactions(&gateway, &request.session_id, cx).await;
    cancel_msp_turn(gateway.clone(), &request.session_id).await?;
    gateway
        .host
        .request(
            "view/unsubscribe",
            json!({
                "sessionId": request.session_id
            }),
        )
        .await?;
    gateway.remove_session(&request.session_id);
    Ok(EmptyResponse {})
}

async fn delete_session(
    gateway: Arc<Gateway>,
    request: DeleteSessionRequest,
    cx: &ClientConnection,
) -> Result<EmptyResponse, RpcError> {
    if gateway.session_exists(&request.session_id) {
        cancel_pending_interactions(&gateway, &request.session_id, cx).await;
        cancel_msp_turn(gateway.clone(), &request.session_id).await?;
        gateway
            .host
            .request(
                "view/unsubscribe",
                json!({
                    "sessionId": request.session_id
                }),
            )
            .await?;
        gateway.remove_session(&request.session_id);
    }

    tokio::task::spawn_blocking(move || gateway.delete_durable_session(&request.session_id))
        .await
        .map_err(|error| RpcError::internal(format!("join session delete: {error}")))??;
    Ok(EmptyResponse {})
}

async fn authenticate(
    _gateway: Arc<Gateway>,
    request: AuthenticateRequest,
) -> Result<EmptyResponse, RpcError> {
    if request.method_id != "muse-login" {
        return Err(RpcError::invalid_params("unknown authentication method"));
    }
    // Terminal-capable clients execute the advertised `muse-acp login`
    // command before making this request. The CLI owns browser/device-flow
    // interaction; this method only acknowledges the completed login.
    Ok(EmptyResponse {})
}

async fn logout(_gateway: Arc<Gateway>) -> Result<EmptyResponse, RpcError> {
    let binary = std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".to_string());
    let output = tokio::process::Command::new(&binary)
        .arg("logout")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|error| RpcError::internal(format!("run {binary} logout: {error}")))?;
    if !output.stderr.is_empty() {
        eprintln!(
            "[muse] {}",
            String::from_utf8_lossy(&output.stderr).trim_end()
        );
    }
    if !output.status.success() {
        return Err(RpcError::internal(format!(
            "{binary} logout exited with {}",
            output.status.code().unwrap_or(-1)
        )));
    }
    Ok(EmptyResponse {})
}

async fn set_config(
    gateway: Arc<Gateway>,
    request: SetConfigRequest,
) -> Result<SetConfigResponse, RpcError> {
    if !gateway.session_exists(&request.session_id) {
        return Err(RpcError::invalid_params("unknown sessionId"));
    }
    if request.config_id != "muse.model" && request.config_id != "muse.reasoningEffort" {
        return Err(RpcError::invalid_params("unknown configId"));
    }
    let Some(value) = request.value.get("value").and_then(Value::as_str) else {
        return Err(RpcError::invalid_params(format!(
            "{} requires an id value",
            request.config_id
        )));
    };

    match request.config_id.as_str() {
        "muse.model" => {
            let model = gateway::decode_model_value(value)?;
            let Some(model_id) = model.model_id.clone() else {
                return Err(RpcError::invalid_params("model id is required"));
            };
            let mut selection = json!({ "modelId": model_id });
            if let Some(provider) = model.provider_id.clone() {
                selection["providerId"] = json!(provider);
            }
            if let Some(profile) = model.profile_id.clone() {
                selection["profileId"] = json!(profile);
            }
            gateway
                .host
                .request(
                    "session/setModel",
                    json!({
                        "commandId": msp::uuid_v7(),
                        "sessionId": request.session_id,
                        "model": selection
                    }),
                )
                .await?;
            gateway.update_model(&request.session_id, model);
        }
        "muse.reasoningEffort" => {
            let effort = gateway::ReasoningEffort::parse(value)?;
            gateway.update_reasoning_effort(&request.session_id, effort)?;
        }
        _ => unreachable!("supported config ID was checked above"),
    }

    let config = gateway
        .config_options(&request.session_id)
        .await
        .unwrap_or_else(|_| gateway.current_config_options(&request.session_id));
    Ok(SetConfigResponse {
        config_options: config,
    })
}

async fn cancel_session(
    gateway: Arc<Gateway>,
    session_id: String,
    cx: &ClientConnection,
) -> Result<(), RpcError> {
    if !gateway.session_exists(&session_id) {
        return Ok(());
    }
    cancel_pending_interactions(&gateway, &session_id, cx).await;
    cancel_msp_turn(gateway, &session_id).await
}

async fn cancel_pending_interactions(gateway: &Gateway, session_id: &str, cx: &ClientConnection) {
    let request_ids = gateway.pending_interaction_request_ids(session_id);
    for request_id in request_ids {
        let _ = cx.send_notification(CancelClientRequestNotification { request_id });
    }
    gateway.cancel_pending_interactions(session_id).await;
}

async fn cancel_msp_turn(gateway: Arc<Gateway>, session_id: &str) -> Result<(), RpcError> {
    let turn_id = gateway.active_turn(session_id);
    let Some(turn_id) = turn_id else {
        return Ok(());
    };
    let mut params = json!({
        "commandId": msp::uuid_v7(),
        "sessionId": session_id
    });
    params["turnId"] = json!(turn_id);
    gateway
        .host
        .request("turn/cancel", params)
        .await
        .map(|_| ())
}

fn validate_workspace_request(
    cwd: &str,
    additional: &[String],
    mcp: &[Value],
) -> Result<(), RpcError> {
    if !std::path::Path::new(cwd).is_absolute() {
        return Err(RpcError::invalid_params("cwd must be absolute"));
    }
    if !additional.is_empty() {
        return Err(RpcError::invalid_params(
            "additionalDirectories are not supported",
        ));
    }
    if !mcp.is_empty() {
        return Err(RpcError::invalid_params("MCP servers are not supported"));
    }
    Ok(())
}

fn send_update(
    cx: &ClientConnection,
    session_id: &str,
    update: Value,
) -> Result<(), agent_client_protocol::Error> {
    cx.send_notification(SessionUpdateNotification {
        session_id: session_id.to_string(),
        update,
    })
}

async fn handle_permission(
    gateway: Arc<Gateway>,
    cx: &ClientConnection,
    session_id: &str,
    approval: &Value,
) -> Result<(), agent_client_protocol::Error> {
    let choices = approval
        .get("availableChoices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if choices.is_empty() {
        return Ok(());
    }
    let approval_id = approval
        .get("approvalId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let tool_name = approval
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("Muse operation");
    let subject_kind = approval
        .get("subject")
        .and_then(|subject| subject.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let raw_args = approval
        .get("rawArgs")
        .and_then(Value::as_str)
        .unwrap_or("");
    let cwd = gateway.session_cwd(session_id).unwrap_or_default();
    let options: Vec<Value> = choices
        .iter()
        .map(|choice| {
            let decision = choice
                .get("decision")
                .and_then(Value::as_str)
                .unwrap_or("denied");
            let scope = choice
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("once");
            let allow = decision.contains("approved");
            let kind = if allow && scope == "once" || !allow && scope == "once" {
                if allow { "allow_once" } else { "reject_once" }
            } else if allow {
                "allow_always"
            } else {
                "reject_always"
            };
            json!({
                "optionId": choice.get("choiceId").cloned().unwrap_or(json!("")),
                "name": choice.get("label").cloned().unwrap_or_else(|| json!(decision)),
                "kind": kind
            })
        })
        .collect();

    let subject = if subject_kind == "shell" {
        json!({
            "type": "command",
            "command": approval
                .get("subject")
                .and_then(|subject| subject.get("command"))
                .and_then(Value::as_str)
                .unwrap_or(raw_args),
            "cwd": cwd
        })
    } else {
        json!({
            "type": "tool_call",
            "toolCall": {
                "toolCallId": approval
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .unwrap_or(&approval_id),
                "title": tool_name,
                "status": "pending"
            }
        })
    };
    let request = PermissionRequest {
        session_id: session_id.to_string(),
        title: format!("Muse requests {subject_kind} approval"),
        description: Some(if raw_args.is_empty() {
            format!("Tool: {tool_name}")
        } else {
            format!("Tool: {tool_name}\n\n{raw_args}")
        }),
        subject: Some(subject),
        options,
    };

    let sent = cx.send_request(request);
    let request_id = serde_json::to_value(sent.id())
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    gateway.track_approval_request(session_id, &approval_id, request_id);
    let response = match sent.block_task().await {
        Ok(response) => response,
        Err(error) => {
            gateway.untrack_approval_request(session_id, &approval_id);
            return Err(error);
        }
    };
    gateway.untrack_approval_request(session_id, &approval_id);
    let outcome = response.outcome;
    if !gateway.approval_active(session_id, &approval_id) {
        return Ok(());
    }
    let selected = outcome
        .get("optionId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            outcome
                .get("outcome")
                .and_then(Value::as_str)
                .and_then(|value| {
                    choices
                        .iter()
                        .find(|choice| {
                            matches!(
                                choice.get("decision").and_then(Value::as_str),
                                Some("abort") | Some("denied") | Some("timedOut")
                            ) && value == "cancelled"
                        })
                        .and_then(|choice| choice.get("choiceId"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_else(|| {
            choices
                .iter()
                .find(|choice| {
                    matches!(
                        choice.get("decision").and_then(Value::as_str),
                        Some("denied") | Some("abort") | Some("timedOut")
                    )
                })
                .and_then(|choice| choice.get("choiceId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });

    let mut params = json!({
        "commandId": msp::uuid_v7(),
        "sessionId": session_id,
                "approvalId": approval_id,
        "choiceId": selected,
        "requirementId": approval.get("currentRequirementId").cloned().unwrap_or(Value::Null)
    });
    if let Some(feedback) = outcome.get("feedback").and_then(Value::as_str) {
        params["feedback"] = json!(feedback);
    }
    gateway
        .host
        .request("approval/decide", params)
        .await
        .map_err(acp_error)?;
    Ok(())
}

async fn handle_elicitation(
    gateway: Arc<Gateway>,
    cx: &ClientConnection,
    session_id: &str,
    user_input: &Value,
) -> Result<(), agent_client_protocol::Error> {
    let user_input_id = user_input
        .get("userInputId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let questions = user_input
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !gateway
        .form_elicitation
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        let _ = gateway
            .host
            .request(
                "userInput/cancel",
                json!({
                    "commandId": msp::uuid_v7(),
                    "sessionId": session_id,
                    "userInputId": user_input_id,
                    "reason": "ACP client does not support form elicitation"
                }),
            )
            .await;
        return Ok(());
    }

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for question in &questions {
        let id = question
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        required.push(json!(id));
        let selection = question.get("selection");
        let mode = selection
            .and_then(|value| value.get("mode"))
            .and_then(Value::as_str)
            .unwrap_or("single");
        let labels: Vec<String> = question
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| option.get("label").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut schema = if mode == "multiple" {
            let mut value = json!({
                "type": "array",
                "items": {"type": "string", "enum": labels}
            });
            if let Some(min) = selection
                .and_then(|value| value.get("minSelections"))
                .and_then(Value::as_i64)
            {
                value["minItems"] = json!(min);
            }
            if let Some(max) = selection
                .and_then(|value| value.get("maxSelections"))
                .and_then(Value::as_i64)
            {
                value["maxItems"] = json!(max);
            }
            value
        } else if labels.is_empty() {
            json!({"type": "string"})
        } else {
            json!({"type": "string", "enum": labels})
        };
        schema["title"] = question.get("header").cloned().unwrap_or_else(|| json!(id));
        schema["description"] = question.get("question").cloned().unwrap_or(Value::Null);
        properties.insert(id.to_string(), schema);
    }

    let message = questions
        .iter()
        .filter_map(|question| question.get("question").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n");
    let request = ElicitationRequest {
        message,
        mode: json!("form"),
        requested_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required
        }),
        session_id: session_id.to_string(),
        tool_call_id: user_input
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    let sent = cx.send_request(request);
    let request_id = serde_json::to_value(sent.id())
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    gateway.track_user_input_request(session_id, user_input_id, request_id);
    let response = match sent.block_task().await {
        Ok(response) => response,
        Err(error) => {
            gateway.untrack_user_input_request(session_id, user_input_id);
            return Err(error);
        }
    };
    gateway.untrack_user_input_request(session_id, user_input_id);
    if !gateway.user_input_active(session_id, user_input_id) {
        return Ok(());
    }
    match response.action.as_str() {
        Some("accept") => {
            let content = response.content.unwrap_or_else(|| json!({}));
            let mut answers = Vec::new();
            for question in &questions {
                let id = question
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mode = question
                    .get("selection")
                    .and_then(|value| value.get("mode"))
                    .and_then(Value::as_str)
                    .unwrap_or("single");
                let value = content.get(id).cloned().unwrap_or(Value::Null);
                let answer = if mode == "multiple" {
                    json!({
                        "questionId": id,
                        "selectedLabels": value
                    })
                } else if question
                    .get("options")
                    .and_then(Value::as_array)
                    .is_some_and(|options| !options.is_empty())
                {
                    json!({
                        "questionId": id,
                        "selectedLabel": value
                    })
                } else {
                    json!({
                        "questionId": id,
                        "freeText": value
                    })
                };
                answers.push(answer);
            }
            gateway
                .host
                .request(
                    "userInput/answer",
                    json!({
                        "commandId": msp::uuid_v7(),
                        "sessionId": session_id,
                        "userInputId": user_input_id,
                        "answers": answers
                    }),
                )
                .await
                .map_err(acp_error)?;
        }
        _ => {
            gateway
                .host
                .request(
                    "userInput/cancel",
                    json!({
                        "commandId": msp::uuid_v7(),
                        "sessionId": session_id,
                        "userInputId": user_input_id,
                        "reason": "user declined elicitation"
                    }),
                )
                .await
                .map_err(acp_error)?;
        }
    }
    Ok(())
}

fn acp_error(error: RpcError) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(error.code as i32, error.message)
        .data(json!({ "msp": { "code": error.code, "data": error.data } }))
}
