//! The MSP `initialize` exchange (SS1.4).

use crate::config::{Capabilities, ClientInfo};
use crate::json::{J, esc};

/// The MSP envelope schema version this client speaks
/// (`InitializeResult.schema.version`). The default handshake check refuses
/// any other reported version.
pub const ENVELOPE_SCHEMA_VERSION: u64 = 1;

/// The facts a host reported in its `initialize` result. Every field is kept
/// as sent; nothing absent is filled in.
#[derive(Debug, Clone)]
pub struct Handshake {
    /// `serverInfo.name`.
    pub server_name: Option<String>,
    /// `serverInfo.version`, such as `1.4.11-R6667.1`.
    pub server_version: Option<String>,
    /// `schema.version`: the envelope schema version.
    pub schema_version: Option<u64>,
    /// `schema.fingerprint`: `sha256:<hex>` of the host's stable protocol
    /// surface. A fingerprint the caller does not know is a warning
    /// condition, not an error.
    pub fingerprint: Option<String>,
    /// `sessionDurability`, verbatim. Read it through
    /// [`Handshake::session_durability`].
    pub durability: Option<String>,
    /// `grantedCapabilities`: the requested capabilities the host granted,
    /// plus any it grants by policy. Fixed for the connection's lifetime.
    pub granted_capabilities: Vec<String>,
    /// `experimentalApi`: the effective experimental setting.
    pub experimental_api: Option<bool>,
    /// `sandboxAvailability`, verbatim (Muse 1.4.7+). Absent means unknown.
    pub sandbox_availability: Option<String>,
    /// `sandboxUnavailableReason`, verbatim. Only meaningful beside
    /// `unavailable`; never infer one when none is sent.
    pub sandbox_unavailable_reason: Option<String>,
    /// The complete `initialize` result, for fields this type does not name
    /// (`museHome`, `platformOs`, `userAgent`, and later additions).
    pub result: J,
}

/// Whether a host persists its sessions (MSP SS2.13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDurability {
    /// Sessions survive a host restart. Also the reading of an absent
    /// `sessionDurability`, per the schema's compatibility rule.
    Durable,
    /// Sessions live only as long as the host process.
    Ephemeral,
    /// A value this client does not know. It carries no recovery guarantee.
    Other(String),
}

impl SessionDurability {
    pub fn is_durable(&self) -> bool {
        matches!(self, SessionDurability::Durable)
    }
}

impl Handshake {
    /// Read the handshake facts from an `initialize` result.
    pub fn from_result(result: J) -> Handshake {
        let text = |value: Option<&J>| value.and_then(J::as_str).map(str::to_string);
        let schema = result.get("schema");
        let server = result.get("serverInfo");
        Handshake {
            server_name: text(server.and_then(|server| server.get("name"))),
            server_version: text(server.and_then(|server| server.get("version"))),
            schema_version: schema
                .and_then(|schema| schema.get("version"))
                .and_then(J::as_u64),
            fingerprint: text(schema.and_then(|schema| schema.get("fingerprint"))),
            durability: text(result.get("sessionDurability")),
            granted_capabilities: match result.get("grantedCapabilities") {
                Some(J::Arr(caps)) => caps
                    .iter()
                    .filter_map(J::as_str)
                    .map(str::to_string)
                    .collect(),
                _ => Vec::new(),
            },
            experimental_api: result.get("experimentalApi").and_then(J::as_bool),
            sandbox_availability: text(result.get("sandboxAvailability")),
            sandbox_unavailable_reason: text(result.get("sandboxUnavailableReason")),
            result,
        }
    }

    /// Whether the host granted this capability on this connection.
    pub fn granted(&self, capability: &str) -> bool {
        self.granted_capabilities
            .iter()
            .any(|granted| granted == capability)
    }

    /// The durability profile: absent reads as durable, an unknown value as
    /// [`SessionDurability::Other`].
    pub fn session_durability(&self) -> SessionDurability {
        match self.durability.as_deref() {
            None | Some("durable") => SessionDurability::Durable,
            Some("ephemeral") => SessionDurability::Ephemeral,
            Some(other) => SessionDurability::Other(other.to_string()),
        }
    }

    /// The leading `MAJOR.MINOR.PATCH` of `serverInfo.version`; see
    /// [`parse_release`].
    pub fn release(&self) -> Option<(u64, u64, u64)> {
        parse_release(self.server_version.as_deref()?)
    }

    /// Whether the host reports at least this release. An absent or
    /// unparseable version answers false.
    pub fn at_least(&self, major: u64, minor: u64, patch: u64) -> bool {
        self.release()
            .is_some_and(|release| release >= (major, minor, patch))
    }
}

/// The leading `MAJOR.MINOR.PATCH` of a Muse version string, ignoring any
/// suffix such as the `-R4684.1` build label. `None` when the version does
/// not start with three dot-separated decimal numbers.
pub fn parse_release(version: &str) -> Option<(u64, u64, u64)> {
    fn number(s: &str) -> Option<u64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    }
    let mut parts = version.splitn(3, '.');
    let major = number(parts.next()?)?;
    let minor = number(parts.next()?)?;
    let rest = parts.next()?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let patch = number(&rest[..end])?;
    Some((major, minor, patch))
}

/// The `initialize` params. Members appear in a fixed order and absent
/// options are omitted, so the same configuration always sends the same
/// bytes.
pub(crate) fn initialize_params(info: &ClientInfo, capabilities: &Capabilities) -> String {
    let title = info
        .title
        .as_deref()
        .map(|title| format!(",\"title\":{}", esc(title)))
        .unwrap_or_default();
    let mut members = Vec::new();
    if let Some(experimental) = capabilities.experimental_api {
        members.push(format!("\"experimentalApi\":{experimental}"));
    }
    if let Some(dialogs) = capabilities.user_input_dialogs {
        members.push(format!("\"userInputDialogs\":{dialogs}"));
    }
    let list = |names: &[String]| {
        names
            .iter()
            .map(|name| esc(name))
            .collect::<Vec<_>>()
            .join(",")
    };
    if !capabilities.requested.is_empty() {
        members.push(format!(
            "\"requestedCapabilities\":[{}]",
            list(&capabilities.requested)
        ));
    }
    if !capabilities.opt_out_notification_methods.is_empty() {
        members.push(format!(
            "\"optOutNotificationMethods\":[{}]",
            list(&capabilities.opt_out_notification_methods)
        ));
    }
    format!(
        "{{\"clientInfo\":{{\"name\":{},\"version\":{}{title}}},\"capabilities\":{{{}}}}}",
        esc(&info.name),
        esc(&info.version),
        members.join(",")
    )
}

/// Refuse an envelope schema version this client does not speak. An absent
/// version is accepted: it predates the field, and the fingerprint still
/// identifies the surface.
pub(crate) fn default_check(handshake: &Handshake) -> Result<(), String> {
    match handshake.schema_version {
        Some(version) if version != ENVELOPE_SCHEMA_VERSION => Err(format!(
            "incompatible host schema: version={version}; this client speaks version {ENVELOPE_SCHEMA_VERSION}"
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_json;

    #[test]
    fn initialize_params_keep_a_stable_member_order() {
        let info = ClientInfo {
            name: "muse_acp".into(),
            version: "0.15.0".into(),
            title: None,
        };
        let capabilities = Capabilities {
            experimental_api: Some(true),
            user_input_dialogs: Some(false),
            requested: vec!["sessionListStream".into(), "sessionMcp".into()],
            opt_out_notification_methods: Vec::new(),
        };
        assert_eq!(
            initialize_params(&info, &capabilities),
            r#"{"clientInfo":{"name":"muse_acp","version":"0.15.0"},"capabilities":{"experimentalApi":true,"userInputDialogs":false,"requestedCapabilities":["sessionListStream","sessionMcp"]}}"#
        );
        let titled = ClientInfo {
            title: Some("Tool \"x\"".into()),
            ..info
        };
        let quiet = Capabilities {
            opt_out_notification_methods: vec!["usage/changed".into()],
            ..Capabilities::default()
        };
        let params = initialize_params(&titled, &quiet);
        assert_eq!(
            params,
            r#"{"clientInfo":{"name":"muse_acp","version":"0.15.0","title":"Tool \"x\""},"capabilities":{"optOutNotificationMethods":["usage/changed"]}}"#
        );
        assert!(parse_json(&params).is_ok());
    }

    #[test]
    fn handshake_facts_are_read_without_filling_gaps() {
        let full = Handshake::from_result(
            parse_json(
                r#"{"serverInfo":{"name":"muse","version":"1.4.11-R6667.1"},"schema":{"version":1,"fingerprint":"sha256:ab"},"grantedCapabilities":["sessionMcp",7,"feedback"],"experimentalApi":true,"sessionDurability":"ephemeral","sandboxAvailability":"unavailable","sandboxUnavailableReason":"userNamespacesDenied","museHome":"/h"}"#,
            )
            .unwrap(),
        );
        assert_eq!(full.server_name.as_deref(), Some("muse"));
        assert_eq!(full.schema_version, Some(1));
        assert_eq!(full.fingerprint.as_deref(), Some("sha256:ab"));
        assert_eq!(full.granted_capabilities, ["sessionMcp", "feedback"]);
        assert!(full.granted("feedback") && !full.granted("userShell"));
        assert_eq!(full.session_durability(), SessionDurability::Ephemeral);
        assert_eq!(full.release(), Some((1, 4, 11)));
        assert!(full.at_least(1, 4, 8) && !full.at_least(1, 5, 0));
        assert_eq!(full.result.get("museHome").and_then(J::as_str), Some("/h"));

        let empty = Handshake::from_result(parse_json("{}").unwrap());
        assert_eq!(empty.server_name, None);
        assert_eq!(empty.schema_version, None);
        assert!(empty.granted_capabilities.is_empty());
        // Absent durability is durable per the schema; unknown is not.
        assert!(empty.session_durability().is_durable());
        assert!(!empty.at_least(0, 0, 0));
        let odd = Handshake {
            durability: Some("future".into()),
            ..empty
        };
        assert_eq!(
            odd.session_durability(),
            SessionDurability::Other("future".into())
        );
    }

    #[test]
    fn release_parsing_reads_the_leading_version_only() {
        assert_eq!(parse_release("1.4.2-R4684.1"), Some((1, 4, 2)));
        assert_eq!(parse_release("1.10.0"), Some((1, 10, 0)));
        for unknown in ["", "garbage", "1.4", "v1.4.2", "+1.4.2", "1..4.2", "1.4.-2"] {
            assert_eq!(parse_release(unknown), None, "{unknown:?}");
        }
    }

    #[test]
    fn the_default_check_refuses_only_an_unsupported_envelope() {
        let with = |version: Option<u64>| Handshake {
            schema_version: version,
            ..Handshake::from_result(J::Null)
        };
        assert!(default_check(&with(Some(1))).is_ok());
        assert!(default_check(&with(None)).is_ok());
        let refused = default_check(&with(Some(2))).unwrap_err();
        assert!(refused.contains("version=2"), "{refused}");
    }
}
