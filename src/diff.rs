//! ACP v2 `diff` tool-call content from Muse's stored tool patches.
//!
//! An edit-family `toolCall` item carries `patchRef` (`kind: "tool_patch"`,
//! `mediaType: "application/json"`); `item/readOutput` serves the stored
//! document, which a live Muse 1.4.11 writes as
//!
//! ```text
//! {"files":[{"path":"/abs/file","hunks":[{"oldStart":1,"oldLines":3,
//!   "newStart":1,"newLines":4,"lines":[" context","-old","+new"]}]}]}
//! ```
//!
//! Paths are absolute and each line keeps its unified-diff prefix. The
//! document records neither file modes nor whether a file ends with a
//! newline, so the patch text carries no mode lines and no `\ No newline at
//! end of file` markers. A file whose hunks hold no old lines was empty or
//! absent before the call; only a whole-file write (`write_file`,
//! `create_file`) reports that as an added file, and an edit reports a
//! modification. Anything that does not have this shape yields no diff, so
//! the card keeps its text instead of a guessed one.

use crate::json::{J, esc};

/// Native tools that write a whole file: a file with no prior lines is one
/// they created.
const WHOLE_FILE_WRITERS: [&str; 2] = ["write_file", "create_file"];

struct Hunk {
    old_start: u64,
    old_lines: u64,
    new_start: u64,
    new_lines: u64,
    lines: Vec<String>,
}

fn hunk(value: &J) -> Option<Hunk> {
    let number = |key: &str| value.get(key).and_then(J::as_u64);
    let J::Arr(raw) = value.get("lines")? else {
        return None;
    };
    let mut lines = Vec::with_capacity(raw.len());
    for line in raw {
        let line = line.as_str()?;
        if !matches!(line.as_bytes().first(), Some(b' ' | b'+' | b'-')) || line.contains('\n') {
            return None;
        }
        lines.push(line.to_string());
    }
    let hunk = Hunk {
        old_start: number("oldStart")?,
        old_lines: number("oldLines")?,
        new_start: number("newStart")?,
        new_lines: number("newLines")?,
        lines,
    };
    // The counts must describe the lines, or the patch would not render
    // what Muse changed.
    let old = hunk.lines.iter().filter(|l| !l.starts_with('+')).count() as u64;
    let new = hunk.lines.iter().filter(|l| !l.starts_with('-')).count() as u64;
    (old == hunk.old_lines && new == hunk.new_lines).then_some(hunk)
}

/// Git's C-style quoting for a path with unusual characters.
fn git_path(path: &str) -> String {
    let unusual = path
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || b >= 0x80 || b == b'"' || b == b'\\');
    if !unusual {
        return path.to_string();
    }
    let mut quoted = String::from("\"");
    for b in path.bytes() {
        match b {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b'\t' => quoted.push_str("\\t"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            0x20..=0x7e => quoted.push(b as char),
            _ => quoted.push_str(&format!("\\{b:03o}")),
        }
    }
    quoted.push('"');
    quoted
}

/// A unified hunk range. Git names the line before an empty range, which is
/// one less than the 1-based start Muse records for it.
fn range(start: u64, lines: u64) -> String {
    let start = if lines == 0 {
        start.saturating_sub(1)
    } else {
        start
    };
    format!("{start},{lines}")
}

/// The ACP v2 `diff` content item for one tool's stored patch document, or
/// `None` when the document is not a recognizable, non-empty patch.
pub fn acp_v2_diff(tool: &str, document: &J) -> Option<String> {
    let J::Arr(files) = document.get("files")? else {
        return None;
    };
    let creates = WHOLE_FILE_WRITERS.contains(&tool.to_ascii_lowercase().as_str());
    let mut changes = Vec::new();
    let mut patch = String::new();
    for file in files {
        let path = file.get("path").and_then(J::as_str)?;
        if !std::path::Path::new(path).is_absolute() || path.contains('\n') {
            return None;
        }
        let J::Arr(raw_hunks) = file.get("hunks")? else {
            return None;
        };
        let hunks = raw_hunks.iter().map(hunk).collect::<Option<Vec<_>>>()?;
        if hunks.is_empty() {
            continue;
        }
        let added = creates && hunks.iter().all(|h| h.old_lines == 0);
        changes.push(format!(
            "{{\"operation\":{},\"path\":{}}}",
            esc(if added { "add" } else { "modify" }),
            esc(path)
        ));
        let shown = git_path(path);
        patch.push_str(&format!("diff --git {shown} {shown}\n"));
        if added {
            patch.push_str(&format!("--- /dev/null\n+++ {shown}\n"));
        } else {
            patch.push_str(&format!("--- {shown}\n+++ {shown}\n"));
        }
        for h in &hunks {
            patch.push_str(&format!(
                "@@ -{} +{} @@\n",
                range(h.old_start, h.old_lines),
                range(h.new_start, h.new_lines)
            ));
            for line in &h.lines {
                patch.push_str(line);
                patch.push('\n');
            }
        }
    }
    if changes.is_empty() {
        return None;
    }
    Some(format!(
        "{{\"type\":\"diff\",\"changes\":[{}],\"patch\":{{\"format\":\"git_patch\",\"text\":{}}}}}",
        changes.join(","),
        esc(&patch)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_json;

    fn diff(tool: &str, document: &str) -> Option<J> {
        acp_v2_diff(tool, &parse_json(document).unwrap()).map(|d| parse_json(&d).unwrap())
    }

    #[cfg(unix)]
    fn text(diff: &J) -> &str {
        diff.get("patch")
            .and_then(|p| p.get("text"))
            .and_then(J::as_str)
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn an_edit_becomes_a_modify_with_its_git_patch() {
        // Recorded from a live Muse 1.4.11 `edit_file`.
        let d = diff(
            "edit_file",
            r#"{"files":[{"hunks":[{"lines":[" line one","-hello notes","+bye notes","+extra"," line three"],"newLines":4,"newStart":1,"oldLines":3,"oldStart":1}],"path":"/ws/notes.txt"}]}"#,
        )
        .expect("diff");
        assert_eq!(
            crate::json::j_to_string(d.get("changes").unwrap()),
            r#"[{"operation":"modify","path":"/ws/notes.txt"}]"#
        );
        assert_eq!(
            text(&d),
            "diff --git /ws/notes.txt /ws/notes.txt\n--- /ws/notes.txt\n+++ /ws/notes.txt\n@@ -1,3 +1,4 @@\n line one\n-hello notes\n+bye notes\n+extra\n line three\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_whole_file_write_with_no_prior_lines_is_an_add() {
        // Recorded from a live Muse 1.4.11 `write_file` of a new file.
        let new_file = r#"{"files":[{"hunks":[{"lines":["+a","+b"],"newLines":2,"newStart":1,"oldLines":0,"oldStart":1}],"path":"/ws/new.txt"}]}"#;
        let d = diff("write_file", new_file).expect("diff");
        assert_eq!(
            crate::json::j_to_string(d.get("changes").unwrap()),
            r#"[{"operation":"add","path":"/ws/new.txt"}]"#
        );
        assert_eq!(
            text(&d),
            "diff --git /ws/new.txt /ws/new.txt\n--- /dev/null\n+++ /ws/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n"
        );
        // An edit never adds a file, even with no prior lines.
        let d = diff("edit_file", new_file).expect("diff");
        assert!(text(&d).contains("--- /ws/new.txt\n"));
        // Emptying a file names the line before its empty new range.
        let d = diff(
            "write_file",
            r#"{"files":[{"hunks":[{"lines":["-x"],"newLines":0,"newStart":1,"oldLines":1,"oldStart":1}],"path":"/ws/x"}]}"#,
        )
        .expect("diff");
        assert!(text(&d).contains("@@ -1,1 +0,0 @@\n-x\n"), "{}", text(&d));
    }

    #[cfg(unix)]
    #[test]
    fn unusual_paths_are_quoted_as_git_quotes_them() {
        let d = diff(
            "edit_file",
            r#"{"files":[{"hunks":[{"lines":["-a","+b"],"newLines":1,"newStart":1,"oldLines":1,"oldStart":1}],"path":"/ws/café \"q\".txt"}]}"#,
        )
        .expect("diff");
        assert!(
            text(&d).starts_with("diff --git \"/ws/caf\\303\\251 \\\"q\\\".txt\" "),
            "{}",
            text(&d)
        );
    }

    #[test]
    fn unrecognized_documents_yield_no_diff() {
        for document in [
            r#"{}"#,
            r#"{"files":[]}"#,
            r#"{"files":[{"path":"relative.txt","hunks":[{"lines":["+a"],"newLines":1,"newStart":1,"oldLines":0,"oldStart":1}]}]}"#,
            r#"{"files":[{"path":"/ws/a","hunks":[]}]}"#,
            // Counts that do not match the lines.
            r#"{"files":[{"path":"/ws/a","hunks":[{"lines":["+a"],"newLines":2,"newStart":1,"oldLines":0,"oldStart":1}]}]}"#,
            // A line without a unified-diff prefix.
            r#"{"files":[{"path":"/ws/a","hunks":[{"lines":["a"],"newLines":1,"newStart":1,"oldLines":1,"oldStart":1}]}]}"#,
        ] {
            assert!(diff("edit_file", document).is_none(), "{document}");
        }
    }
}
