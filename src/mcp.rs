//! Client MCP servers: ACP `mcpServers` to MSP `config.mcpServers`.
//!
//! ACP editors attach MCP servers to session setup (Zed's context servers,
//! the JetBrains IDE server). MSP 1.3.0 accepts them on `session/start` and
//! `session/resume` as a typed map keyed by server name, behind the
//! `sessionMcp` capability. Entries are translated one by one. An entry Muse
//! cannot run is dropped with a log line instead of failing the session,
//! because the host rejects the whole session for one malformed server. The
//! SDK has already refused a request whose servers do not fit the schema.
//!
//! Every server is sent as `optional`: ACP has no required servers, and a
//! required server that fails to start makes every turn fail. Log lines name
//! servers and transports only; commands, arguments, URLs, environment values
//! and headers can carry credentials and are never echoed.

use crate::json::esc;
use crate::request::McpServer;

/// The MSP side of one ACP `mcpServers` list.
#[derive(Debug, Default)]
pub struct Translation {
    /// JSON text of the MSP `mcpServers` object, or None when nothing is
    /// forwarded.
    pub servers: Option<String>,
    /// Names of the translated servers, in client order.
    pub forwarded: Vec<String>,
    /// One diagnostic per dropped entry.
    pub dropped: Vec<String>,
}

impl Translation {
    /// Whether the client supplied any server at all, forwarded or not.
    pub fn supplied(&self) -> bool {
        !self.forwarded.is_empty() || !self.dropped.is_empty()
    }

    /// `"a", "b"` for log lines.
    pub fn names(&self) -> String {
        self.forwarded
            .iter()
            .map(|name| esc(name))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Translate the client's `mcpServers`, which the SDK has already parsed
/// into their schema types. The first entry with a given name wins, since MSP
/// keys servers by name.
pub fn translate(servers: &[McpServer]) -> Translation {
    let mut translation = Translation::default();
    let mut members = Vec::new();
    for (index, entry) in servers.iter().enumerate() {
        match server(entry) {
            Ok((name, _)) if translation.forwarded.contains(&name) => {
                translation
                    .dropped
                    .push(format!("MCP server {} dropped: duplicate name", esc(&name)));
            }
            Ok((name, value)) => {
                members.push(format!("{}:{value}", esc(&name)));
                translation.forwarded.push(name);
            }
            Err(reason) => translation.dropped.push(format!(
                "MCP server {} dropped: {reason}",
                label(entry, index)
            )),
        }
    }
    if !members.is_empty() {
        translation.servers = Some(format!("{{{}}}", members.join(",")));
    }
    translation
}

fn label(entry: &McpServer, index: usize) -> String {
    match entry.name() {
        Some(name) if !name.is_empty() => esc(name),
        _ => format!("#{index}"),
    }
}

/// One ACP server entry as `(name, MSP SessionMcpServerConfig JSON)`.
fn server(entry: &McpServer) -> Result<(String, String), String> {
    let name = match entry.name() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => return Err("missing name".to_string()),
    };
    let value = match entry {
        McpServer::Stdio {
            command, args, env, ..
        } => stdio(command, args, env)?,
        McpServer::Http { url, headers, .. } => http(url, headers)?,
        McpServer::Sse { .. } => {
            return Err("the SSE transport is not supported by Muse".to_string());
        }
        McpServer::Other { kind, .. } => {
            return Err(format!("unsupported transport {}", esc(kind)));
        }
    };
    Ok((name, value))
}

fn stdio(command: &str, args: &[String], env: &[(String, String)]) -> Result<String, String> {
    if command.is_empty() {
        return Err("missing command".to_string());
    }
    let mut out = format!("{{\"transport\":\"stdio\",\"command\":{}", esc(command));
    if !args.is_empty() {
        let args: Vec<String> = args.iter().map(|arg| esc(arg)).collect();
        out.push_str(&format!(",\"args\":[{}]", args.join(",")));
    }
    if let Some(env) = pairs(env, "env")? {
        out.push_str(&format!(",\"env\":{env}"));
    }
    out.push_str(",\"mode\":\"optional\"}");
    Ok(out)
}

fn http(url: &str, headers: &[(String, String)]) -> Result<String, String> {
    if url.is_empty() {
        return Err("missing url".to_string());
    }
    let mut out = format!("{{\"transport\":\"streamableHttp\",\"url\":{}", esc(url));
    if let Some(headers) = pairs(headers, "headers")? {
        out.push_str(&format!(",\"headers\":{headers}"));
    }
    out.push_str(",\"mode\":\"optional\"}");
    Ok(out)
}

/// ACP `[{name, value}]` as an MSP string map, or None when empty. A later
/// entry replaces an earlier one with the same name, as in a process
/// environment.
fn pairs(items: &[(String, String)], key: &str) -> Result<Option<String>, String> {
    let mut map: Vec<(&str, &str)> = Vec::new();
    for (name, value) in items {
        if name.is_empty() {
            return Err(format!("{key} entries must have a name"));
        }
        match map
            .iter_mut()
            .find(|(existing, _)| *existing == name.as_str())
        {
            Some(slot) => slot.1 = value,
            None => map.push((name, value)),
        }
    }
    if map.is_empty() {
        return Ok(None);
    }
    let members = map
        .iter()
        .map(|(name, value)| format!("{}:{}", esc(name), esc(value)))
        .collect::<Vec<_>>();
    Ok(Some(format!("{{{}}}", members.join(","))))
}

#[cfg(test)]
mod tests {
    use super::translate;
    use agent_client_protocol::{v1, v2};

    /// The servers of an ACP v1 list, as the SDK parses them.
    fn run(servers: &str) -> super::Translation {
        let servers: Vec<v1::schema::McpServer> =
            serde_json::from_str(servers).expect("v1 servers");
        let servers: Vec<_> = servers.into_iter().map(crate::request::v1_mcp).collect();
        translate(&servers)
    }

    /// The servers of an ACP v2 list, as the SDK parses them.
    fn run_v2(servers: &str) -> super::Translation {
        let servers: Vec<v2::schema::McpServer> =
            serde_json::from_str(servers).expect("v2 servers");
        let servers: Vec<_> = servers.into_iter().map(crate::request::v2_mcp).collect();
        translate(&servers)
    }

    #[test]
    fn v1_untagged_stdio_becomes_an_optional_stdio_server() {
        let t = run(
            r#"[{"name":"intellij","command":"/opt/idea/bin/java","args":["-cp","x","Main"],"env":[{"name":"IJ_PORT","value":"64342"}]}]"#,
        );
        assert_eq!(
            t.servers.as_deref(),
            Some(
                r#"{"intellij":{"transport":"stdio","command":"/opt/idea/bin/java","args":["-cp","x","Main"],"env":{"IJ_PORT":"64342"},"mode":"optional"}}"#
            )
        );
        assert_eq!(t.forwarded, ["intellij"]);
        assert!(t.dropped.is_empty());
    }

    #[test]
    fn v2_tagged_stdio_and_http_are_both_forwarded() {
        let t = run_v2(
            r#"[{"type":"stdio","name":"files","command":"/bin/mcp"},{"type":"http","name":"docs","url":"https://mcp.example/mcp","headers":[{"name":"Authorization","value":"Bearer t"}]}]"#,
        );
        assert_eq!(
            t.servers.as_deref(),
            Some(
                r#"{"files":{"transport":"stdio","command":"/bin/mcp","mode":"optional"},"docs":{"transport":"streamableHttp","url":"https://mcp.example/mcp","headers":{"Authorization":"Bearer t"},"mode":"optional"}}"#
            )
        );
        assert_eq!(t.names(), r#""files", "docs""#);
    }

    #[test]
    fn an_empty_list_translates_to_nothing() {
        let t = run("[]");
        assert!(t.servers.is_none() && !t.supplied());
    }

    #[test]
    fn unsupported_entries_are_dropped_without_failing_the_rest() {
        let t = run(r#"[
                {"type":"sse","name":"legacy","url":"https://sse.example/sse","headers":[]},
                {"name":"","command":"/bin/x","args":[],"env":[]},
                {"name":"nocmd","command":"","args":[],"env":[]},
                {"type":"http","name":"nourl","url":"","headers":[]},
                {"name":"badpair","command":"/bin/x","args":[],"env":[{"name":"","value":"v"}]},
                {"name":"ok","command":"/bin/ok","args":[],"env":[]}
            ]"#);
        assert_eq!(t.forwarded, ["ok"]);
        assert_eq!(
            t.servers.as_deref(),
            Some(r#"{"ok":{"transport":"stdio","command":"/bin/ok","mode":"optional"}}"#)
        );
        assert_eq!(t.dropped.len(), 5, "{:?}", t.dropped);
        assert!(t.dropped[0].contains("\"legacy\"") && t.dropped[0].contains("SSE"));
        assert!(t.dropped[1].contains("#1") && t.dropped[1].contains("missing name"));
        assert!(t.dropped[2].contains("missing command"));
        assert!(t.dropped[3].contains("missing url"));
        assert!(t.dropped[4].contains("env entries must have a name"));
        let t = run_v2(r#"[{"type":"websocket","name":"future","url":"wss://x"}]"#);
        assert!(t.dropped[0].contains("unsupported transport \"websocket\""));
    }

    #[test]
    fn the_first_server_with_a_name_wins() {
        let t = run(
            r#"[{"name":"dup","command":"/bin/first","args":[],"env":[]},{"type":"http","name":"dup","url":"https://second","headers":[]}]"#,
        );
        assert_eq!(t.forwarded, ["dup"]);
        assert!(t.servers.as_deref().unwrap().contains("/bin/first"));
        assert!(!t.servers.as_deref().unwrap().contains("second"));
        assert_eq!(t.dropped, ["MCP server \"dup\" dropped: duplicate name"]);
    }

    #[test]
    fn a_later_environment_entry_replaces_an_earlier_one() {
        let t = run(
            r#"[{"name":"e","command":"/bin/e","args":[],"env":[{"name":"A","value":"1"},{"name":"B","value":""},{"name":"A","value":"2"}]}]"#,
        );
        assert!(
            t.servers
                .as_deref()
                .unwrap()
                .contains(r#""env":{"A":"2","B":""}"#),
            "{:?}",
            t.servers
        );
    }

    #[test]
    fn diagnostics_never_echo_commands_urls_or_secret_values() {
        let t = run(r#"[
                {"type":"sse","name":"b","url":"https://SECRET-URL","headers":[]},
                {"type":"http","name":"c","url":"https://SECRET-URL","headers":[{"name":"","value":"SECRET"}]},
                {"name":"d","command":"/secret/cmd","args":["--token","SECRET-ARG"],"env":[{"name":"TOKEN","value":"SECRET-ENV"},{"name":"","value":"x"}]},
                {"name":"e","command":"/secret/cmd","args":[],"env":[{"name":"TOKEN","value":"SECRET-ENV"}]},
                {"name":"e","command":"/secret/cmd","args":[],"env":[]}
            ]"#);
        assert_eq!(t.dropped.len(), 4, "{:?}", t.dropped);
        for line in &t.dropped {
            for secret in ["SECRET", "/secret/cmd", "Authorization", "TOKEN"] {
                assert!(!line.contains(secret), "{line}");
            }
        }
        assert!(!t.names().contains("SECRET"));
    }
}
