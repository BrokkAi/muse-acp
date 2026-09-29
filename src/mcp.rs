//! Client MCP servers: ACP `mcpServers` to MSP `config.mcpServers`.
//!
//! ACP editors attach MCP servers to session setup (Zed's context servers,
//! the JetBrains IDE server). MSP 1.3.0 accepts them on `session/start` and
//! `session/resume` as a typed map keyed by server name, behind the
//! `sessionMcp` capability. Entries are translated one by one. An entry Muse
//! cannot run is dropped with a log line instead of failing the session,
//! because the host rejects the whole session for one malformed server.
//!
//! Every server is sent as `optional`: ACP has no required servers, and a
//! required server that fails to start makes every turn fail. Log lines name
//! servers and transports only; commands, arguments, URLs, environment values
//! and headers can carry credentials and are never echoed.

use crate::json::{J, esc};

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

/// Translate `params.mcpServers`. A missing, non-array or empty list
/// translates to nothing. The first entry with a given name wins, since MSP
/// keys servers by name.
pub fn translate(params: Option<&J>) -> Translation {
    let mut translation = Translation::default();
    let Some(J::Arr(entries)) = params.and_then(|p| p.get("mcpServers")) else {
        return translation;
    };
    let mut members = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
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

fn label(entry: &J, index: usize) -> String {
    match entry.get("name").and_then(J::as_str) {
        Some(name) if !name.is_empty() => esc(name),
        _ => format!("#{index}"),
    }
}

/// One ACP server entry as `(name, MSP SessionMcpServerConfig JSON)`.
fn server(entry: &J) -> Result<(String, String), String> {
    if !matches!(entry, J::Obj(_)) {
        return Err("not an object".to_string());
    }
    let name = match entry.get("name") {
        Some(J::Str(name)) if !name.is_empty() => name.clone(),
        _ => return Err("missing name".to_string()),
    };
    // ACP v1 stdio entries are untagged; v2 tags every transport.
    let value = match entry.get("type") {
        None | Some(J::Null) => stdio(entry)?,
        Some(J::Str(kind)) => match kind.as_str() {
            "stdio" => stdio(entry)?,
            "http" => http(entry)?,
            "sse" => return Err("the SSE transport is not supported by Muse".to_string()),
            other => return Err(format!("unsupported transport {}", esc(other))),
        },
        Some(_) => return Err("type is not a string".to_string()),
    };
    Ok((name, value))
}

fn stdio(entry: &J) -> Result<String, String> {
    let command = required_string(entry, "command")?;
    let mut out = format!("{{\"transport\":\"stdio\",\"command\":{}", esc(command));
    let args = match entry.get("args") {
        None | Some(J::Null) => Vec::new(),
        Some(J::Arr(values)) => values
            .iter()
            .map(|value| value.as_str().map(esc))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "args must be strings".to_string())?,
        Some(_) => return Err("args must be an array".to_string()),
    };
    if !args.is_empty() {
        out.push_str(&format!(",\"args\":[{}]", args.join(",")));
    }
    if let Some(env) = pairs(entry, "env")? {
        out.push_str(&format!(",\"env\":{env}"));
    }
    out.push_str(",\"mode\":\"optional\"}");
    Ok(out)
}

fn http(entry: &J) -> Result<String, String> {
    let url = required_string(entry, "url")?;
    let mut out = format!("{{\"transport\":\"streamableHttp\",\"url\":{}", esc(url));
    if let Some(headers) = pairs(entry, "headers")? {
        out.push_str(&format!(",\"headers\":{headers}"));
    }
    out.push_str(",\"mode\":\"optional\"}");
    Ok(out)
}

fn required_string<'a>(entry: &'a J, key: &str) -> Result<&'a str, String> {
    match entry.get(key) {
        Some(J::Str(value)) if !value.is_empty() => Ok(value),
        _ => Err(format!("missing {key}")),
    }
}

/// ACP `[{name, value}]` as an MSP string map, or None when empty. A later
/// entry replaces an earlier one with the same name, as in a process
/// environment.
fn pairs(entry: &J, key: &str) -> Result<Option<String>, String> {
    let items = match entry.get(key) {
        None | Some(J::Null) => return Ok(None),
        Some(J::Arr(items)) => items,
        Some(_) => return Err(format!("{key} must be an array")),
    };
    let mut map: Vec<(&str, &str)> = Vec::new();
    for item in items {
        let (Some(J::Str(name)), Some(J::Str(value))) = (item.get("name"), item.get("value"))
        else {
            return Err(format!("{key} entries must have string name and value"));
        };
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
    use crate::json::parse_json;

    fn run(servers: &str) -> super::Translation {
        let params = parse_json(&format!("{{\"mcpServers\":{servers}}}")).expect("test JSON");
        translate(Some(&params))
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
        let t = run(
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
    fn empty_or_missing_lists_translate_to_nothing() {
        for servers in ["[]", "null", "{}", "\"x\""] {
            let t = run(servers);
            assert!(t.servers.is_none() && !t.supplied(), "{servers}");
        }
        assert!(!translate(None).supplied());
    }

    #[test]
    fn unsupported_and_malformed_entries_are_dropped_without_failing_the_rest() {
        let t = run(r#"[
                {"type":"sse","name":"legacy","url":"https://sse.example/sse"},
                {"type":"acp","name":"inline"},
                {"type":7,"name":"typed"},
                {"command":"/bin/x"},
                {"name":"","command":"/bin/x"},
                {"name":"nocmd"},
                {"type":"http","name":"nourl"},
                {"name":"badargs","command":"/bin/x","args":[1]},
                {"name":"badenv","command":"/bin/x","env":{"K":"V"}},
                {"name":"badpair","command":"/bin/x","env":[{"name":"K"}]},
                "not an object",
                {"name":"ok","command":"/bin/ok"}
            ]"#);
        assert_eq!(t.forwarded, ["ok"]);
        assert_eq!(
            t.servers.as_deref(),
            Some(r#"{"ok":{"transport":"stdio","command":"/bin/ok","mode":"optional"}}"#)
        );
        assert_eq!(t.dropped.len(), 11, "{:?}", t.dropped);
        assert!(t.dropped[0].contains("\"legacy\"") && t.dropped[0].contains("SSE"));
        assert!(t.dropped[1].contains("unsupported transport \"acp\""));
        assert!(t.dropped[3].contains("#3") && t.dropped[3].contains("missing name"));
        assert!(t.dropped[5].contains("missing command"));
        assert!(t.dropped[6].contains("missing url"));
        assert!(t.dropped[10].contains("not an object"));
    }

    #[test]
    fn the_first_server_with_a_name_wins() {
        let t = run(
            r#"[{"name":"dup","command":"/bin/first"},{"type":"http","name":"dup","url":"https://second"}]"#,
        );
        assert_eq!(t.forwarded, ["dup"]);
        assert!(t.servers.as_deref().unwrap().contains("/bin/first"));
        assert!(!t.servers.as_deref().unwrap().contains("second"));
        assert_eq!(t.dropped, ["MCP server \"dup\" dropped: duplicate name"]);
    }

    #[test]
    fn a_later_environment_entry_replaces_an_earlier_one() {
        let t = run(
            r#"[{"name":"e","command":"/bin/e","env":[{"name":"A","value":"1"},{"name":"B","value":""},{"name":"A","value":"2"}]}]"#,
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
                {"name":"a","command":"/secret/cmd","args":["--token","SECRET-ARG",3]},
                {"type":"sse","name":"b","url":"https://SECRET-URL"},
                {"type":"http","name":"c","url":"https://SECRET-URL","headers":[{"name":"Authorization"}]},
                {"name":"d","command":"/secret/cmd","env":[{"name":"TOKEN","value":"SECRET-ENV"},{"value":"x"}]},
                {"name":"e","command":"/secret/cmd","env":[{"name":"TOKEN","value":"SECRET-ENV"}]},
                {"name":"e","command":"/secret/cmd"}
            ]"#);
        assert_eq!(t.dropped.len(), 5, "{:?}", t.dropped);
        for line in &t.dropped {
            for secret in ["SECRET", "/secret/cmd", "Authorization", "TOKEN"] {
                assert!(!line.contains(secret), "{line}");
            }
        }
        assert!(!t.names().contains("SECRET"));
    }
}
