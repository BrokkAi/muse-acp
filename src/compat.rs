//! Runtime MSP schema compatibility classification.
//!
//! MSP is a Developer Preview surface, so drift between the fingerprint this
//! adapter was built against and the one a live host reports is expected.
//! Every launch classifies the handshake facts once and logs them in a
//! machine-readable form; unknown fingerprints degrade loudly but do not
//! block, while an unsupported envelope schema version fails closed.

/// Envelope schema version this adapter understands (`InitializeResult.schema.version`).
pub const SUPPORTED_SCHEMA_VERSION: u64 = 1;

/// Fingerprint of the host release this adapter was originally validated
/// against (Muse host 1.0.2).
pub const TESTED_FINGERPRINT: &str =
    "sha256:03312c213efd14277a0e0a102f70adeae497a469ca4edf7242f479953ed758b7";

/// Fingerprint published by the Muse SDK manifest at revision `fbce769`
/// (schema version 1). Recorded because it is the shape source we track, but
/// it has not been verified against a live `muse serve` build yet.
pub const SDK_MANIFEST_FINGERPRINT: &str =
    "sha256:cfd31ee77d78fdada9febc4edccd29b0434ff8f6bf157c7c03fd0ecfcbc29f5a";

/// Fingerprint embedded in the SDK conformance-transcript fixtures. It is
/// deliberately distinct from every host fingerprint and must never be
/// classified as host compatibility.
pub const TRANSCRIPT_FIXTURE_FINGERPRINT: &str =
    "sha256:c8d1a2a1866814e220fd396d382a9a75861412feee884b5021b2ee359bd3dc59";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Validated against a live host by this project.
    Tested,
    /// Known shape, but not validated against a live host (or only partially).
    Degraded,
    /// Cannot be used safely with this adapter.
    Incompatible,
    /// Not a host fingerprint at all.
    Fixture,
    /// No compatibility decision recorded for this pair.
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Tested => "tested",
            Status::Degraded => "degraded",
            Status::Incompatible => "incompatible",
            Status::Fixture => "fixture",
            Status::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compatibility {
    pub schema_version: Option<u64>,
    pub fingerprint: String,
    pub status: Status,
    pub detail: &'static str,
}

impl Compatibility {
    /// True when the adapter must refuse to drive this host.
    pub fn is_fatal(&self) -> bool {
        self.status == Status::Incompatible
    }

    /// One machine-readable log line for support diagnostics.
    pub fn log_line(&self, adapter_version: &str, host: &str) -> String {
        format!(
            "schema-compat adapter={adapter_version} host={host} schema_version={} fingerprint={} status={} detail={}",
            self.schema_version
                .map(|v| v.to_string())
                .unwrap_or_else(|| "absent".to_string()),
            self.fingerprint,
            self.status.as_str(),
            self.detail
        )
    }
}

fn table_entry(fingerprint: &str) -> Option<(Status, &'static str)> {
    Some(match fingerprint {
        TESTED_FINGERPRINT => (Status::Tested, "validated against live host 1.0.2"),
        SDK_MANIFEST_FINGERPRINT => (
            Status::Degraded,
            "SDK manifest fbce769 schema v1; no live-host verification recorded",
        ),
        TRANSCRIPT_FIXTURE_FINGERPRINT => (
            Status::Fixture,
            "transcript fixture fingerprint; never a live-host result",
        ),
        _ => return None,
    })
}

/// Classify a handshake result. The envelope schema version is the hard gate;
/// fingerprint knowledge only adjusts the warning.
pub fn classify(schema_version: Option<u64>, fingerprint: &str) -> Compatibility {
    let fingerprint = if fingerprint.is_empty() {
        "absent".to_string()
    } else {
        fingerprint.to_string()
    };
    let (mut status, mut detail) = table_entry(&fingerprint)
        .unwrap_or((Status::Unknown, "no compatibility decision recorded"));

    match schema_version {
        Some(SUPPORTED_SCHEMA_VERSION) => {}
        Some(v) => {
            return Compatibility {
                schema_version: Some(v),
                fingerprint,
                status: Status::Incompatible,
                detail: "unsupported MSP envelope schema version (fatal)",
            };
        }
        None => {
            if status == Status::Tested {
                status = Status::Degraded;
                detail = "fingerprint known but schema version absent";
            } else if status == Status::Unknown {
                detail = "schema version absent and fingerprint unknown";
            }
        }
    }

    Compatibility {
        schema_version,
        fingerprint,
        status,
        detail,
    }
}

/// Machine-readable rows for `--selftest`; host facts require a live host.
pub fn selftest_lines(adapter_version: &str) -> Vec<String> {
    let mut lines = vec![format!(
        "selftest adapter={adapter_version} supported_schema_version={SUPPORTED_SCHEMA_VERSION} host=offline"
    )];
    for (fp, kind) in [
        (TESTED_FINGERPRINT, "tested-host"),
        (SDK_MANIFEST_FINGERPRINT, "sdk-manifest"),
        (TRANSCRIPT_FIXTURE_FINGERPRINT, "transcript-fixture"),
    ] {
        let c = classify(Some(SUPPORTED_SCHEMA_VERSION), fp);
        lines.push(format!(
            "schema-compat kind={kind} schema_version={} fingerprint={fp} status={} detail={}",
            c.schema_version.unwrap_or_default(),
            c.status.as_str(),
            c.detail
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_validated_host_fingerprint_as_tested() {
        let c = classify(Some(1), TESTED_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert!(!c.is_fatal());
    }

    #[test]
    fn sdk_manifest_is_degraded_not_tested() {
        let c = classify(Some(1), SDK_MANIFEST_FINGERPRINT);
        assert_eq!(c.status, Status::Degraded);
        assert!(!c.is_fatal());
    }

    #[test]
    fn transcript_fixture_fingerprint_is_never_host_compatibility() {
        let c = classify(Some(1), TRANSCRIPT_FIXTURE_FINGERPRINT);
        assert_eq!(c.status, Status::Fixture);
        assert!(!c.is_fatal());
    }

    #[test]
    fn unknown_fingerprint_degrades_but_does_not_block() {
        let c = classify(Some(1), "sha256:deadbeef");
        assert_eq!(c.status, Status::Unknown);
        assert!(!c.is_fatal());
    }

    #[test]
    fn missing_schema_version_degrades_known_fingerprints() {
        let c = classify(None, TESTED_FINGERPRINT);
        assert_eq!(c.status, Status::Degraded);
    }

    #[test]
    fn unsupported_envelope_schema_version_is_fatal() {
        let c = classify(Some(2), TESTED_FINGERPRINT);
        assert_eq!(c.status, Status::Incompatible);
        assert!(c.is_fatal());
    }

    #[test]
    fn log_and_selftest_lines_are_machine_readable() {
        let c = classify(Some(1), TESTED_FINGERPRINT);
        let line = c.log_line("0.2.5", "muse-session-server/1.0.2");
        for token in [
            "schema-compat adapter=0.2.5",
            "host=muse-session-server/1.0.2",
            "schema_version=1",
            "status=tested",
        ] {
            assert!(line.contains(token), "missing {token} in {line}");
        }

        let lines = selftest_lines("0.2.5");
        assert!(lines[0].contains("adapter=0.2.5"));
        assert!(lines[0].contains("host=offline"));
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().any(|l| l.contains("kind=sdk-manifest")));
        assert!(lines.iter().any(|l| l.contains("kind=transcript-fixture")));
    }
}

#[cfg(test)]
mod corpus_tests {
    use super::{SDK_MANIFEST_FINGERPRINT, SUPPORTED_SCHEMA_VERSION};
    use crate::json::parse_json;

    fn protocol_path(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/protocol")
            .join(rel)
    }

    #[test]
    fn vendored_sdk_manifest_matches_the_compatibility_table() {
        // A schema advance that forgets to re-pin must fail here, not at a
        // user's first prompt.
        let text = std::fs::read_to_string(protocol_path("stable/manifest.json"))
            .expect("vendored manifest");
        let manifest = parse_json(&text).expect("manifest JSON");
        let fingerprint = manifest
            .get("fingerprint")
            .and_then(|v| v.as_str())
            .expect("manifest fingerprint");
        assert_eq!(fingerprint, SDK_MANIFEST_FINGERPRINT);
        let version = manifest
            .get("schemaVersion")
            .and_then(|v| v.as_u64())
            .expect("manifest schemaVersion");
        assert_eq!(version, SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn vendored_schema_bundle_is_well_formed() {
        let text = std::fs::read_to_string(protocol_path("stable/msp.schema.json"))
            .expect("vendored schema bundle");
        let schema = parse_json(&text).expect("schema bundle JSON");
        let defs = schema.get("$defs").expect("$defs");
        for required in ["Item", "ApprovalRequestParams", "UserInputRequestParams"] {
            assert!(defs.get(required).is_some(), "missing $defs.{required}");
        }
    }
}
