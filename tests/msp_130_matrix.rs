use std::fs;

const START: &str = "<!-- msp-1.3.0-matrix:start -->";
const END: &str = "<!-- msp-1.3.0-matrix:end -->";

const METHODS: &[&str] = &[
    "goal/clear",
    "goal/edit",
    "goal/pause",
    "goal/resume",
    "goal/set",
    "item/readOutput",
    "session/rename",
    "session/setReasoningEffort",
    "skill/list",
    "task/background",
    "task/stop",
    "task/stopAll",
    "usage/read",
    "view/subscribe",
    "workflow/cancel",
    "workflow/childControl",
];

const NOTIFICATIONS: &[&str] = &[
    "session/modelRouteUnserved",
    "session/nameChanged",
    "session/reasoningEffortChanged",
    "session/statusChanged",
    "session/viewHealthChanged",
    "skill/changed",
    "usage/changed",
];

const ERRORS: &[&str] = &["skillNotFound", "outputUnavailable"];
const REQUESTS: &[&str] = &["approval/request", "userInput/request"];

fn matrix() -> String {
    let roadmap =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ROADMAP.md")).expect("ROADMAP.md");
    roadmap
        .split_once(START)
        .and_then(|(_, rest)| rest.split_once(END).map(|(body, _)| body.to_string()))
        .expect("Muse 1.3.0 matrix markers")
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
fn every_muse_130_addition_has_a_disposition() {
    let body = matrix();
    for name in METHODS
        .iter()
        .chain(NOTIFICATIONS)
        .chain(ERRORS)
        .chain(REQUESTS)
    {
        assert_disposition(&body, name);
    }
}

#[test]
fn the_muse_130_request_index_records_both_receipts() {
    let body = matrix();
    assert!(body.contains("top-level `requests` index"));
    for request in ["approval/request", "userInput/request"] {
        assert!(body.contains(&format!("| `{request}` | Mapped to ACP |")));
    }
    assert!(body.contains("empty `RequestReceipt`"));
}

#[test]
fn cross_references_do_not_count_as_disposition_rows() {
    let body = "| `skill/list` | Mapped to ACP | list skills |\n| `skill/changed` | Consumed | See `skill/list`. |";
    assert_disposition(body, "skill/list");
}

#[test]
#[should_panic(expected = "expected one matrix row")]
fn cross_reference_cannot_replace_a_missing_disposition() {
    assert_disposition(
        "| `skill/changed` | Consumed | See `skill/list`. |",
        "skill/list",
    );
}
