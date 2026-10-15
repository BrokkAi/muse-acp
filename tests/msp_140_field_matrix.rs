//! Guards the Muse 1.4.x field-level and vocabulary disposition matrix.
//!
//! Issue #224 closed the gap between the pinned 1.4.6 stable surface and the
//! adapter by deciding, per field, whether it earns an ACP mapping or an
//! explicit "Intentionally ignored" disposition. This test keeps that matrix
//! honest in two ways: every addition has exactly one disposition row in
//! `ROADMAP.md` section 9, and every addition is still present (and, where it
//! matters, still nullable) in the vendored schema bundle — so a schema
//! advance cannot silently drop a field the adapter claims to tolerate.

use std::fs;

const START: &str = "<!-- msp-1.4.x-field-matrix:start -->";
const END: &str = "<!-- msp-1.4.x-field-matrix:end -->";

/// Every field or vocabulary addition this matrix covers, as the label used in
/// the ROADMAP row (the second table cell). Kept in the issue's order.
const ADDITIONS: &[&str] = &[
    "ItemKind.webSearch",
    "TurnErrorKind.mcpLinkError",
    "TurnError.quota",
    "TurnRetryScheduledParams.errorKind",
    "TurnRetryScheduledParams.httpStatus",
    "TurnRetryScheduledParams.system",
    "TurnRetractedParams.outcome",
    "SessionStartParams.permissionProfile",
    "TurnStartParams.userShell",
    "Session.sideFrom",
    "SessionBranchChangedParams.branch",
    "SessionModelChangedParams.providerId",
    "SessionModelRouteUnservedParams.providerId",
    "SessionTokenUsageParams.modelId",
    "SessionGoalChangedParams.goal",
];

fn matrix() -> String {
    let roadmap =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ROADMAP.md")).expect("ROADMAP.md");
    roadmap
        .split_once(START)
        .and_then(|(_, rest)| rest.split_once(END).map(|(body, _)| body.to_string()))
        .expect("Muse 1.4.x field matrix markers")
}

fn assert_disposition(body: &str, name: &str) {
    let rows: Vec<&str> = body
        .lines()
        .filter(|line| {
            line.starts_with('|')
                && line.split('|').nth(1).map(str::trim) == Some(format!("`{name}`").as_str())
        })
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "expected one matrix row for {name}: {rows:?}"
    );
    let cells: Vec<&str> = rows[0].split('|').map(str::trim).collect();
    assert!(
        cells.len() >= 4,
        "malformed matrix row for {name}: {}",
        rows[0]
    );
    let valid = [
        "Consumed",
        "Mapped to ACP",
        "Internally tracked",
        "Intentionally ignored",
        "Unsupported pending protocol decision",
    ];
    assert!(
        valid.contains(&cells[2]),
        "invalid disposition for {name}: {}",
        cells[2]
    );
    if cells[2] == "Unsupported pending protocol decision" {
        assert!(
            cells[3].contains("https://github.com/BrokkAi/muse-acp/issues/"),
            "unsupported {name} needs a tracking issue: {}",
            cells[3]
        );
    }
}

#[test]
fn every_muse_140_field_addition_has_a_disposition() {
    let body = matrix();
    for name in ADDITIONS {
        assert_disposition(&body, name);
    }
}

#[test]
fn the_field_matrix_covers_no_extra_rows() {
    let body = matrix();
    let rows: Vec<String> = body
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            line.split('|')
                .nth(1)
                .map(str::trim)
                .unwrap_or("")
                .trim_matches('`')
                .to_string()
        })
        .collect();
    assert_eq!(
        rows.len(),
        ADDITIONS.len(),
        "the matrix must classify every and only its additions: {rows:?}"
    );
    for name in ADDITIONS {
        assert!(
            rows.contains(&name.to_string()),
            "missing matrix row {name}"
        );
    }
}

fn schema() -> serde_json::Value {
    let text = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/protocol/stable/msp.schema.json"
    ))
    .expect("vendored schema bundle");
    serde_json::from_str(&text).expect("schema bundle JSON")
}

fn has_property(defs: &serde_json::Value, def: &str, member: &str) -> bool {
    defs.get(def)
        .and_then(|d| d.get("properties"))
        .and_then(|p| p.get(member))
        .is_some()
}

fn null_tolerant(defs: &serde_json::Value, def: &str, member: &str) -> bool {
    let Some(member) = defs
        .get(def)
        .and_then(|d| d.get("properties"))
        .and_then(|p| p.get(member))
    else {
        return false;
    };
    if member
        .get("type")
        .and_then(|t| t.as_array())
        .is_some_and(|kinds| kinds.iter().any(|k| k == "null"))
    {
        return true;
    }
    member
        .get("anyOf")
        .and_then(|a| a.as_array())
        .is_some_and(|arms| {
            arms.iter()
                .any(|arm| arm.get("type").and_then(|t| t.as_str()) == Some("null"))
        })
}

#[test]
fn every_field_addition_is_still_in_the_vendored_bundle() {
    let schema = schema();
    let defs = schema.get("$defs").expect("$defs");
    let enum_has = |def: &str, value: &str| {
        defs.get(def)
            .and_then(|d| d.get("enum"))
            .and_then(|e| e.as_array())
            .is_some_and(|values| values.iter().any(|v| v == value))
    };
    assert!(
        enum_has("ItemKind", "webSearch"),
        "ItemKind.webSearch left the stable surface"
    );
    assert!(
        enum_has("TurnErrorKind", "mcpLinkError"),
        "TurnErrorKind.mcpLinkError left the stable surface"
    );
    for (def, member) in [
        ("TurnError", "quota"),
        ("TurnRetryScheduledParams", "errorKind"),
        ("TurnRetryScheduledParams", "httpStatus"),
        ("TurnRetryScheduledParams", "system"),
        ("TurnRetractedParams", "outcome"),
        ("SessionStartParams", "permissionProfile"),
        ("TurnStartParams", "userShell"),
        ("Session", "sideFrom"),
        ("SessionBranchChangedParams", "branch"),
        ("SessionModelChangedParams", "providerId"),
        ("SessionModelRouteUnservedParams", "providerId"),
        ("SessionTokenUsageParams", "modelId"),
        ("SessionGoalChangedParams", "goal"),
    ] {
        assert!(
            has_property(defs, def, member),
            "{def}.{member} left the stable surface"
        );
    }
}

#[test]
fn the_required_nullable_additions_keep_their_null_arm() {
    let schema = schema();
    let defs = schema.get("$defs").expect("$defs");
    for (def, member) in [
        ("Session", "sideFrom"),
        ("SessionBranchChangedParams", "branch"),
        ("SessionModelChangedParams", "providerId"),
        ("SessionModelRouteUnservedParams", "providerId"),
        ("SessionTokenUsageParams", "modelId"),
        ("SessionGoalChangedParams", "goal"),
    ] {
        assert!(
            null_tolerant(defs, def, member),
            "{def}.{member} must stay null-tolerant"
        );
    }
}

#[test]
#[should_panic(expected = "expected one matrix row")]
fn a_missing_row_cannot_be_masked_by_another_addition() {
    assert_disposition(
        "| `TurnError.quota` | Mapped to ACP | see the quota row |",
        "ItemKind.webSearch",
    );
}
