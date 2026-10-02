//! Client-side auto-review ("approve on my behalf").
//!
//! In the `autoReview` approval mode the host runs `promptUnmatched`, and
//! the adapter answers the host's approval requests that only read or edit
//! ordinary files inside the session's workspace roots, instead of asking
//! the editor. It always picks the host's allow-once choice, so it never
//! saves a rule. Everything else still reaches the editor: any doubt about a
//! request means the user decides.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::json::J;

/// File access the adapter approves for the user. Deleting, moving, and
/// access kinds this list does not know still go to the editor.
const ELIGIBLE_ACCESS: [&str; 9] = [
    "read", "list", "stat", "search", "write", "create", "append", "edit", "modify",
];

/// The choice to send for an approval request the adapter may approve, or
/// why the request goes to the editor instead.
pub fn review(params: &J, roots: &[String]) -> Result<String, &'static str> {
    // Both flags are required on the wire. A missing one fails closed.
    if !matches!(params.get("protectedWrite"), Some(J::Bool(false))) {
        return Err("Muse marked it as a protected write");
    }
    if !matches!(params.get("judgeEscalated"), Some(J::Bool(false))) {
        return Err("Muse escalated it for review");
    }
    let subject = params.get("subject").ok_or("it has no subject")?;
    if subject.get("kind").and_then(|v| v.as_str()) != Some("fileAccess") {
        return Err("it is not a file access");
    }
    if matches!(subject.get("stages"), Some(J::Arr(stages)) if !stages.is_empty()) {
        return Err("it has approval stages");
    }
    let access = subject
        .get("access")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ELIGIBLE_ACCESS.contains(&access.as_str()) {
        return Err("it does not only read or edit");
    }
    let path = subject
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("it names no path")?;
    if !in_workspace(Path::new(path), roots) {
        return Err("its path is outside the workspace or hidden");
    }
    allow_once(params).ok_or("Muse offered no allow-once choice")
}

/// The host's approve-once choice. Session-wide and persistent approvals
/// would outlive this request, so they are never chosen.
fn allow_once(params: &J) -> Option<String> {
    let J::Arr(choices) = params.get("availableChoices")? else {
        return None;
    };
    choices
        .iter()
        .find(|choice| {
            choice.get("decision").and_then(|v| v.as_str()) == Some("approved")
                && choice.get("scope").and_then(|v| v.as_str()) == Some("once")
        })?
        .get("choiceId")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Whether `path` resolves inside one of `roots` without passing through a
/// hidden file or folder below the root, such as `.git`, `.github`, `.env`,
/// or `.vscode`: those hold credentials and settings that run code.
fn in_workspace(path: &Path, roots: &[String]) -> bool {
    let Some(resolved) = resolve(path) else {
        return false;
    };
    roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .any(|root| {
            resolved.strip_prefix(&root).is_ok_and(|rest| {
                rest.components().all(|component| {
                    matches!(component, Component::Normal(name)
                        if !name.to_string_lossy().starts_with('.'))
                })
            })
        })
}

/// The canonical form of an absolute path whose last components may not
/// exist yet, as when a file is created. Only names that do not exist at all
/// are appended unresolved: a dangling symbolic link, or a name that cannot
/// be resolved, could lead outside the workspace, so it yields `None`.
fn resolve(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return None;
    }
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(mut resolved) => {
                resolved.extend(missing.iter().rev());
                return Some(resolved);
            }
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && std::fs::symlink_metadata(existing)
                        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                missing.push(existing.file_name()?);
                existing = existing.parent()?;
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_json;

    struct Workspace(PathBuf);

    impl Workspace {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "muse-acp-auto-review-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src/lib.rs"), "fixture").unwrap();
            Workspace(root)
        }

        fn path(&self, rest: &str) -> String {
            self.0.join(rest).to_string_lossy().into_owned()
        }

        fn roots(&self) -> Vec<String> {
            vec![self.0.to_string_lossy().into_owned()]
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn request(path: &str, access: &str) -> J {
        let mut subject = format!(
            r#"{{"kind":"fileAccess","toolName":"write_file","path":{},"access":{}}}"#,
            crate::json::esc(path),
            crate::json::esc(access)
        );
        if access.is_empty() {
            subject = format!(
                r#"{{"kind":"fileAccess","path":{}}}"#,
                crate::json::esc(path)
            );
        }
        parse_json(&format!(
            r#"{{"approvalId":"a1","subject":{subject},"availableChoices":[{{"choiceId":"allow_session","decision":"approvedForSession","scope":"session"}},{{"choiceId":"allow_once","decision":"approved","scope":"once"}},{{"choiceId":"abort","decision":"abort","scope":"once"}}],"protectedWrite":false,"judgeEscalated":false}}"#
        ))
        .unwrap()
    }

    fn with(params: &J, key: &str, value: J) -> J {
        let J::Obj(mut fields) = params.clone() else {
            unreachable!()
        };
        fields.retain(|(name, _)| name != key);
        fields.push((key.to_string(), value));
        J::Obj(fields)
    }

    #[test]
    fn workspace_reads_and_edits_take_the_allow_once_choice() {
        let ws = Workspace::new("eligible");
        for access in ["read", "write", "Edit", "create", "list", "search"] {
            assert_eq!(
                review(&request(&ws.path("src/lib.rs"), access), &ws.roots()),
                Ok("allow_once".to_string()),
                "{access}"
            );
        }
        // A file that does not exist yet, in a folder that does not either.
        assert_eq!(
            review(&request(&ws.path("src/new/mod.rs"), "create"), &ws.roots()),
            Ok("allow_once".to_string())
        );
    }

    #[test]
    fn anything_else_goes_to_the_editor() {
        let ws = Workspace::new("ineligible");
        let roots = ws.roots();
        let file = ws.path("src/lib.rs");
        for access in ["delete", "move", "rename", "chmod", ""] {
            assert!(review(&request(&file, access), &roots).is_err(), "{access}");
        }
        let eligible = request(&file, "write");
        for (key, value) in [
            ("protectedWrite", J::Bool(true)),
            ("judgeEscalated", J::Bool(true)),
            ("protectedWrite", J::Null),
            ("availableChoices", parse_json(r#"[{"choiceId":"allow_session","decision":"approvedForSession","scope":"session"}]"#).unwrap()),
        ] {
            assert!(review(&with(&eligible, key, value), &roots).is_err(), "{key}");
        }
        let without_flag = match eligible.clone() {
            J::Obj(fields) => J::Obj(
                fields
                    .into_iter()
                    .filter(|(name, _)| name != "judgeEscalated")
                    .collect(),
            ),
            _ => unreachable!(),
        };
        assert!(review(&without_flag, &roots).is_err());
        for subject in [
            r#"{"kind":"shell","command":"cargo test"}"#,
            r#"{"kind":"network","target":"example.com"}"#,
            r#"{"kind":"tool","toolName":"mcp__probe__tool"}"#,
            r#"{"kind":"process","command":"make"}"#,
            r#"{"kind":"somethingNew","path":"/"}"#,
        ] {
            let params = with(&eligible, "subject", parse_json(subject).unwrap());
            assert!(review(&params, &roots).is_err(), "{subject}");
        }
        let staged = with(
            &eligible,
            "subject",
            parse_json(&format!(
                r#"{{"kind":"fileAccess","access":"write","path":{},"stages":[{{}}]}}"#,
                crate::json::esc(&file)
            ))
            .unwrap(),
        );
        assert!(review(&staged, &roots).is_err());
    }

    #[test]
    fn paths_must_stay_inside_the_workspace_and_out_of_hidden_folders() {
        let ws = Workspace::new("paths");
        let outside = Workspace::new("paths-outside");
        std::fs::create_dir_all(ws.0.join(".git/hooks")).unwrap();
        let roots = ws.roots();
        for path in [
            outside.path("src/lib.rs"),
            ws.path(".git/hooks/pre-commit"),
            ws.path(".env"),
            ws.path(".github/workflows/ci.yml"),
            ws.path("src/../../escape"),
            "relative/src/lib.rs".to_string(),
        ] {
            assert!(review(&request(&path, "write"), &roots).is_err(), "{path}");
        }
        // The workspace root itself may be hidden; only names below it count.
        let hidden_root = Workspace::new("paths-hidden");
        let dotted = hidden_root.0.join(".config");
        std::fs::create_dir_all(&dotted).unwrap();
        assert!(
            review(
                &request(&dotted.join("notes.md").to_string_lossy(), "write"),
                &[dotted.to_string_lossy().into_owned()]
            )
            .is_ok()
        );
        assert!(review(&request(&ws.path("src/lib.rs"), "write"), &[]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_cannot_lead_outside_the_workspace() {
        let ws = Workspace::new("links");
        let outside = Workspace::new("links-outside");
        let roots = ws.roots();
        std::os::unix::fs::symlink(&outside.0, ws.0.join("linked")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("missing"), ws.0.join("dangling")).unwrap();
        std::os::unix::fs::symlink(ws.0.join(".git"), ws.0.join("visible")).unwrap();
        std::fs::create_dir_all(ws.0.join(".git")).unwrap();
        for path in [
            ws.path("linked/src/lib.rs"),
            ws.path("linked/new.rs"),
            ws.path("dangling"),
            ws.path("visible/config"),
        ] {
            assert!(review(&request(&path, "write"), &roots).is_err(), "{path}");
        }
        // A link that stays inside the workspace is fine.
        std::os::unix::fs::symlink(ws.0.join("src"), ws.0.join("source")).unwrap();
        assert!(review(&request(&ws.path("source/lib.rs"), "write"), &roots).is_ok());
    }
}
