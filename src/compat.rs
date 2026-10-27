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

/// Stable-surface fingerprint of the Muse 1.2.1 host (schema version 1).
/// The `initialize` reply carries this fingerprint (it is the stable-surface
/// digest, not a build hash), and it matches the binary's own `muse schema`
/// export, which was diffed against the vendored bundle (purely additive
/// surface growth). Validated against a live `muse serve` 1.2.1: handshake,
/// session lifecycle, and a full turn (`end_turn`, `terminal=completed`).
pub const HOST_121_FINGERPRINT: &str =
    "sha256:c7ff6c5d1e89cd42f803aea1f05b8e72082f2099685802473eb726903484713b";

/// Stable-surface fingerprint of the Muse 1.3.0 host (schema version 1).
/// The live-host smoke test verified the handshake and session lifecycle, and
/// the stable surface was confirmed additive against the vendored bundle.
pub const HOST_130_FINGERPRINT: &str =
    "sha256:ab69549a7ebb423fce94068762da0b5ff3cdec1f8fc263dcc17248eda117f852";

/// Stable-surface fingerprint reported by the Muse 1.3.0-R3401.1 host build.
/// This build was also verified through a live handshake, session lifecycle,
/// and full turn (`end_turn`, `terminal=completed`).
pub const HOST_130_R3401_1_FINGERPRINT: &str =
    "sha256:7469c9e352e67def4a59df7e439984d7194fa351e1c8b7abb34060fd977ced81";

/// Stable-surface fingerprint of the Muse 1.4.1 host (schema version 1),
/// also carried by the binary's own `muse schema generate-json-schema`
/// export. That export is purely additive over the 1.3.0-R3401.1 surface:
/// `session/delete`, the `session/started`, `session/closed` and
/// `session/deleteCompleted` notifications, `workspaceRoots` on
/// `session/start` and `turn/start`, and per-model reasoning tiers in the
/// catalog, with nothing removed or newly required. Validated by the
/// live-host smoke test (handshake, session lifecycle, full turn) and live
/// session MCP turns.
pub const HOST_141_FINGERPRINT: &str =
    "sha256:e0e163db6ccf00dbe68402ce55d6319b3edc33c421f31e9583b587b2de8a118f";

/// Stable-surface fingerprint of the Muse 1.4.2 host (schema version 1). Its
/// schema export is purely additive over 1.4.1: `feedback/submit` and the
/// `feedback` capability, `session/list` filters (`filter`,
/// `appliedFilter`), and cache and cost fields on cumulative usage, with
/// nothing removed or newly required and the notification, request, and
/// error tables unchanged. Validated by the loopback live-host suite
/// (`tests/live_loopback.rs`): turns, shell approvals, cancel, reload,
/// session MCP, and the auto-review settings view.
pub const HOST_142_FINGERPRINT: &str =
    "sha256:61afea3112e0906e9dc3a536144278a74cb4b36fc6e20901a91d4432ba3568e2";

/// Stable-surface fingerprint of the Muse 1.4.3 host (schema version 1). Its
/// schema export removes `session/delete` and `session/deleteCompleted`
/// (the host answers the method with `methodNotFound`, so the adapter stops
/// offering delete). Everything else is additive over 1.4.2: `hook/list`,
/// `plugin/list`, `userInput/interrupt`, the `turn/foregroundCompleted` and
/// `userInput/engaged` notifications, the `hookRun` item kind, tool
/// receipts, `lastTurn` on resume and read, and session `kind` and
/// `parentSessionId`. `userInput/settled` now requires a nullable
/// `decidedAt` and `feedback/submit` no longer requires `sessionId`.
/// Validated by the loopback live-host suite (`tests/live_loopback.rs`).
pub const HOST_143_FINGERPRINT: &str =
    "sha256:4cb671082574037fc1070a136db7c0c8dc24e65e78cf3d209a10944ff9f9845f";

/// Stable-surface fingerprint of the Muse 1.4.4 host (schema version 1).
/// `session/delete` stays removed; the additions over 1.4.3 are
/// `computerUseSettings/read`, `computerUseSettings/update`, `media/upload`,
/// `session/sideChat`, `skill/setActivation`, `workflow/pause`, the task-list
/// and status vocabulary, and `SideProvenance`. Validated
/// against a live `muse serve --provider echo` 1.4.4 handshake (server
/// `muse/1.4.4`, durable, all requested capabilities granted) and by diffing
/// the vendored bundle against the binary's own
/// `muse schema generate-json-schema` export, which is identical.
pub const HOST_144_FINGERPRINT: &str =
    "sha256:7c94f153c41659cb3f1bd3c3e04438be254644cb2a97d65d48edc7449b74858a";

/// Stable-surface fingerprint of the Muse 1.4.6 host (schema version 1),
/// read from that release's own published download manifest. It is the same
/// surface the SDK's `ea88c822` (1.4.6) pin published, and the pinned 1.4.6
/// build in the live-host matrix (`tests/live_loopback.rs`) validates it.
pub const HOST_146_FINGERPRINT: &str =
    "sha256:eae467ab60cb9a5d5af4de616830f065e59375c987fd59269c8cc33ed2a1137f";

/// Stable-surface fingerprint of the Muse 1.4.7 host (schema version 1),
/// read from that release's published download manifest and reported
/// byte-for-byte by a live `muse serve` 1.4.7. The SDK's retired `a0c8611`
/// pin published the same surface, so the pinned 1.4.7 build in the
/// live-host matrix (`tests/live_loopback.rs`) validates it; the vendored
/// corpus has since moved to the 1.4.13 surface.
pub const HOST_147_FINGERPRINT: &str =
    "sha256:6de32db08bbbe0deb478d2bfcaba001e9fb490986bc80db3aceb4d996abf7df9";

/// Stable-surface fingerprint of the Muse 1.4.8 host (schema version 1),
/// read from that release's published download manifest and reported
/// byte-for-byte by a live `muse serve` 1.4.8. Its `muse schema
/// generate-json-schema` export is byte-identical to the then-vendored
/// `838d36a` bundle, so the pinned 1.4.8 build in the live-host matrix
/// (`tests/live_loopback.rs`) validated that corpus; the corpus has since
/// moved to the 1.4.13 surface. Over
/// 1.4.7 the surface is purely additive (`turn/modelRetryScheduled`, the
/// computer-use app and app-patch vocabulary, `Item.messagePhase` and the
/// `monitorFeed` fields, `Session.generatedName`, `permissionProfile` on
/// resume, the skill's declared title, and the `sessionRename` capability),
/// with nothing removed or newly required.
pub const HOST_148_FINGERPRINT: &str =
    "sha256:4e3200c3c9dc0cd7919d11022630d43f9bc1c5cdda375381e90b0183b5665069";

/// Stable-surface fingerprint of the Muse 1.4.9 host (schema version 1),
/// carried unchanged by Muse 1.4.10. The SDK's `ae46934` pin (2026-10-16,
/// "remirror: 1.4.10") publishes this surface: its stable bundle is
/// byte-identical to what an installed Muse 1.4.10 exports with `muse schema
/// generate-json-schema`, and a live 1.4.10 host reports exactly this digest.
/// Over 1.4.8 the surface is purely additive (`Item.displayLabel` with the
/// `ToolDisplayLabel` triple, and the `approval/reviewing` notification),
/// with nothing removed or newly required. It classifies as degraded until a
/// pinned 1.4.9 or 1.4.10 build joins the live-host matrix.
pub const HOST_149_FINGERPRINT: &str =
    "sha256:5d3b43e5ccdb7dc4f5a32c380f5f2cde0f10e3059b93610afb2e73f73b1d914c";

/// Stable-surface fingerprint of the Muse 1.4.11 host (schema version 1).
/// The SDK's `22a8a56` pin (2026-10-19, "remirror: 1.4.11") publishes this
/// surface: its stable bundle is byte-identical to what an installed Muse
/// 1.4.11 exports with `muse schema generate-json-schema`, and a live
/// 1.4.11 host reports exactly this digest. Over 1.4.9/1.4.10 the surface
/// is purely additive (the `session/leave` command with its
/// `session/leaveCompleted` terminal, the `leave` reason on
/// `session/closed`, the monitor-inbox trio, the `damagedRange` and
/// `scheduledFire` item kinds, goal timestamps, recovery observations and
/// modes, and the skill-invocation source), with nothing removed or newly
/// required. The pinned 1.4.11-R6667.1 build in the nightly live-host matrix
/// reports this digest, so it classifies as tested; the vendored corpus has
/// since moved to the 1.4.13 surface.
pub const HOST_1411_FINGERPRINT: &str =
    "sha256:f282ad16e21520d0cda68cb190149b86e2b062eef1dd282749360c05aa3d1082";

/// Stable-surface fingerprint of the Muse 1.4.12 host (schema version 1).
/// The SDK's `09d67ec` pin (2026-10-22, "remirror: 1.4.12") publishes this
/// surface: its stable bundle is byte-identical to what an installed Muse
/// 1.4.12 exports with `muse schema generate-json-schema`, and a live
/// 1.4.12 host reports exactly this digest. Over 1.4.11 the surface is
/// purely additive (computer-use approval card facts, the
/// `backgroundTerminalCompletion` and `overdueNotice` item kinds, the
/// `blockedPrompt` recall seed on read and resume, the history
/// `checkpointBoundary`, and the task-list `command` and `executionMode`),
/// with nothing removed or newly required. The pinned 1.4.12-R6909.1 build in
/// the nightly live-host matrix reports this digest, so it classifies as
/// tested; the vendored corpus has since moved to the 1.4.13 surface.
pub const HOST_1412_FINGERPRINT: &str =
    "sha256:cfdb6d2525601b9bac76584ccb0610ab3a5539b5e4f51141f079b99b39fcb4e0";

/// Stable-surface fingerprint of the Muse 1.4.13 surface (schema version 1),
/// published by the vendored SDK manifest.
/// The SDK's `3dce908` pin (2026-10-24, "remirror: 1.4.13") publishes this
/// surface. Over 1.4.12 it is purely additive, with the method,
/// notification, request, error, and capability tables unchanged:
/// computer-use approval data facts (`cuaCardFacts.data` with its `mask`),
/// the monitor tool-call replay hint and terminal-row anchor
/// (`Item.replayHint`, `Item.terminalRowAfterItemId`), the ManagedBash
/// `Item.workId`, and the queued inputs a `session/read` still holds
/// (`SessionReadResult.queuedInputs`), with nothing removed or newly
/// required. The pinned 1.4.13-R6996.1 build in the nightly live-host matrix
/// reports this digest, and its `muse schema generate-json-schema` output is
/// byte-identical to the vendored bundle, so it classifies as tested.
pub const HOST_1413_FINGERPRINT: &str =
    "sha256:7fddaf646dbc317e5fcac0785e135eb04df14fbb0db1863089dfd6da21730e5b";

/// Fingerprint published by the vendored Muse SDK manifest at revision
/// `3dce908` (schema version 1), which describes the Muse 1.4.13 stable
/// surface. The vendored bundle is the SDK's own stable export for that
/// release, so the constant aliases [`HOST_1413_FINGERPRINT`]: one table
/// entry and one match arm cover both the manifest check in `corpus_tests`
/// and the `sdk-manifest` row in [`selftest_lines`].
pub const SDK_MANIFEST_FINGERPRINT: &str = HOST_1413_FINGERPRINT;

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
        HOST_121_FINGERPRINT => (Status::Tested, "validated against live host 1.2.1"),
        HOST_130_FINGERPRINT => (Status::Tested, "validated against live host 1.3.0"),
        HOST_130_R3401_1_FINGERPRINT => {
            (Status::Tested, "validated against live host 1.3.0-R3401.1")
        }
        HOST_141_FINGERPRINT => (Status::Tested, "validated against live host 1.4.1"),
        HOST_142_FINGERPRINT => (Status::Tested, "validated against live host 1.4.2"),
        HOST_143_FINGERPRINT => (Status::Tested, "validated against live host 1.4.3"),
        HOST_144_FINGERPRINT => (Status::Tested, "validated against live host 1.4.4"),
        HOST_146_FINGERPRINT => (Status::Tested, "validated against live host 1.4.6"),
        HOST_147_FINGERPRINT => (Status::Tested, "validated against live host 1.4.7"),
        HOST_148_FINGERPRINT => (
            Status::Tested,
            "validated against live host 1.4.8; the previously vendored SDK 838d36a surface",
        ),
        HOST_149_FINGERPRINT => (
            Status::Degraded,
            "Muse 1.4.9/1.4.10 surface; byte-identical to the installed 1.4.10 export, pending a pinned CI build",
        ),
        HOST_1411_FINGERPRINT => (
            Status::Tested,
            "validated against live host 1.4.11-R6667.1; the previously vendored SDK 22a8a56 surface",
        ),
        HOST_1412_FINGERPRINT => (
            Status::Tested,
            "validated against live host 1.4.12-R6909.1; the previously vendored SDK 09d67ec surface",
        ),
        HOST_1413_FINGERPRINT => (
            Status::Tested,
            "validated against live host 1.4.13-R6996.1; the vendored SDK 3dce908 surface",
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
        (HOST_121_FINGERPRINT, "host-1.2.1"),
        (HOST_130_FINGERPRINT, "host-1.3.0"),
        (HOST_130_R3401_1_FINGERPRINT, "host-1.3.0-r3401.1"),
        (HOST_141_FINGERPRINT, "host-1.4.1"),
        (HOST_142_FINGERPRINT, "host-1.4.2"),
        (HOST_143_FINGERPRINT, "host-1.4.3"),
        (HOST_144_FINGERPRINT, "host-1.4.4"),
        (HOST_146_FINGERPRINT, "host-1.4.6"),
        (HOST_147_FINGERPRINT, "host-1.4.7"),
        (HOST_148_FINGERPRINT, "host-1.4.8"),
        (HOST_149_FINGERPRINT, "host-1.4.9"),
        (HOST_1411_FINGERPRINT, "host-1.4.11"),
        (HOST_1412_FINGERPRINT, "host-1.4.12"),
        (HOST_1413_FINGERPRINT, "host-1.4.13"),
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
    fn sdk_manifest_surface_is_the_1_4_13_surface() {
        // The vendored SDK manifest (3dce908) publishes the Muse 1.4.13
        // surface, and the pinned 1.4.13-R6996.1 build in the live-host
        // matrix reports this digest, so the surface classifies as tested.
        assert_eq!(
            SDK_MANIFEST_FINGERPRINT,
            "sha256:7fddaf646dbc317e5fcac0785e135eb04df14fbb0db1863089dfd6da21730e5b"
        );
        assert_eq!(SDK_MANIFEST_FINGERPRINT, HOST_1413_FINGERPRINT);
        assert_ne!(SDK_MANIFEST_FINGERPRINT, HOST_1412_FINGERPRINT);
        let c = classify(Some(1), SDK_MANIFEST_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_121_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_121_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_130_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_130_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_130_r3401_1_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_130_R3401_1_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_141_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_141_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(c.detail, "validated against live host 1.4.1");
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_142_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_142_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(c.detail, "validated against live host 1.4.2");
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_143_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_143_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(c.detail, "validated against live host 1.4.3");
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_144_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_144_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(c.detail, "validated against live host 1.4.4");
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_146_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_146_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(c.detail, "validated against live host 1.4.6");
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_147_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_147_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(c.detail, "validated against live host 1.4.7");
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_148_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_148_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(
            c.detail,
            "validated against live host 1.4.8; the previously vendored SDK 838d36a surface"
        );
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_149_fingerprint_is_degraded_pending_a_ci_pin() {
        let c = classify(Some(1), HOST_149_FINGERPRINT);
        assert_eq!(c.status, Status::Degraded);
        assert_eq!(
            c.detail,
            "Muse 1.4.9/1.4.10 surface; byte-identical to the installed 1.4.10 export, pending a pinned CI build"
        );
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_1411_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_1411_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(
            c.detail,
            "validated against live host 1.4.11-R6667.1; the previously vendored SDK 22a8a56 surface"
        );
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_1412_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_1412_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(
            c.detail,
            "validated against live host 1.4.12-R6909.1; the previously vendored SDK 09d67ec surface"
        );
        assert!(!c.is_fatal());
    }

    #[test]
    fn host_1413_fingerprint_is_tested() {
        let c = classify(Some(1), HOST_1413_FINGERPRINT);
        assert_eq!(c.status, Status::Tested);
        assert_eq!(
            c.detail,
            "validated against live host 1.4.13-R6996.1; the vendored SDK 3dce908 surface"
        );
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
        assert_eq!(lines.len(), 18);
        assert!(lines.iter().any(|l| l.contains("kind=sdk-manifest")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.2.1")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.3.0")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.3.0-r3401.1")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.1")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.2")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.3")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.4")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.6")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.7")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.8")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.9")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.11")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.12")));
        assert!(lines.iter().any(|l| l.contains("kind=host-1.4.13")));
        assert!(lines.iter().any(|l| l.contains("kind=transcript-fixture")));
    }
}

#[cfg(test)]
mod corpus_tests {
    use super::{SDK_MANIFEST_FINGERPRINT, SUPPORTED_SCHEMA_VERSION};
    use crate::json::parse_json;
    use std::collections::BTreeSet;

    fn protocol_path(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/protocol")
            .join(rel)
    }

    fn repository_path(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
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

    #[test]
    fn every_schema_notification_has_an_explicit_matrix_disposition() {
        let schema_text = std::fs::read_to_string(protocol_path("stable/msp.schema.json"))
            .expect("vendored schema bundle");
        let schema = parse_json(&schema_text).expect("schema bundle JSON");
        let schema_notifications = match schema.get("notifications").expect("notifications") {
            crate::json::J::Obj(entries) => entries
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<BTreeSet<_>>(),
            _ => panic!("schema notifications must be an object"),
        };

        let matrix = std::fs::read_to_string(repository_path("docs/event-compatibility.md"))
            .expect("MSP event compatibility matrix");
        let rows = matrix
            .split("<!-- schema-notifications:start -->")
            .nth(1)
            .and_then(|tail| tail.split("<!-- schema-notifications:end -->").next())
            .expect("schema notification matrix markers");
        let dispositions = [
            "Mapped to ACP",
            "Internally tracked",
            "Consumed",
            "Intentionally ignored",
            "Unsupported pending protocol decision",
        ];
        let mut documented = BTreeSet::new();
        for line in rows.lines().filter(|line| line.starts_with("| `")) {
            let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
            assert!(cells.len() >= 5, "malformed matrix row: {line}");
            let method = cells[1]
                .strip_prefix('`')
                .and_then(|value| value.strip_suffix('`'))
                .expect("backtick-wrapped notification method");
            assert!(
                dispositions.contains(&cells[2]),
                "invalid disposition for {method}: {}",
                cells[2]
            );
            assert!(
                documented.insert(method),
                "duplicate notification matrix row: {method}"
            );
        }

        assert_eq!(
            documented, schema_notifications,
            "the compatibility matrix must classify every and only published schema notification"
        );
    }
}
