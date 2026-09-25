//! Process-local compatibility for Muse's TUI-only auto-review profile.
//!
//! `muse serve` has no permission-profile flag or settings-file override. Use
//! a private XDG config view only when the saved built-in profile needs the
//! unavailable reviewer. Everything except settings is linked, not copied.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::json::{J, j_to_string, parse_json};

pub struct HostConfig {
    root: PathBuf,
}

impl Drop for HostConfig {
    fn drop(&mut self) {
        // remove_dir_all removes the links themselves, never their targets.
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn config_root(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|value| !value.is_empty())
                .map(|value| PathBuf::from(value).join(".config"))
        })
}

/// Returns a replacement only for the built-in auto-review profile. Other
/// profiles (including restrictive and user-defined ones) stay host-owned.
fn host_settings(text: &str) -> Option<String> {
    let mut settings = parse_json(text).ok()?;
    let J::Obj(fields) = &mut settings else {
        return None;
    };
    let (_, J::Obj(permissions)) = fields.iter_mut().find(|(key, _)| key == "permissions")? else {
        return None;
    };
    let (_, profile) = permissions
        .iter_mut()
        .find(|(key, _)| key == "default_profile")?;
    if profile.as_str() != Some(":auto-review") {
        return None;
    }
    *profile = J::Str(":ask-me".into());
    Some(j_to_string(&settings))
}

fn private_temp_dir() -> io::Result<HostConfig> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    loop {
        let root = std::env::temp_dir().join(format!(
            "muse-acp-config-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        #[cfg(unix)]
        let mut builder = fs::DirBuilder::new();
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&root) {
            Ok(()) => return Ok(HostConfig { root }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn link_entry(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)
    }
    #[cfg(windows)]
    {
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(source, target)
        } else {
            std::os::windows::fs::symlink_file(source, target)
        }
    }
}

fn prepare(source: &Path) -> io::Result<Option<HostConfig>> {
    let source = std::path::absolute(source)?;
    let text = match fs::read_to_string(source.join("muse/settings.json")) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // Let Muse diagnose invalid settings itself, without changing them.
    let Some(settings) = host_settings(&text) else {
        return Ok(None);
    };
    let config = private_temp_dir()?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if entry.file_name() != "muse" {
            link_entry(&entry.path(), &config.root.join(entry.file_name()))?;
        }
    }
    let muse = config.root.join("muse");
    fs::create_dir(&muse)?;
    for entry in fs::read_dir(source.join("muse"))? {
        let entry = entry?;
        if entry.file_name() != "settings.json" && entry.file_name() != ".settings.json.lock" {
            link_entry(&entry.path(), &muse.join(entry.file_name()))?;
        }
    }
    fs::write(muse.join("settings.json"), settings)?;
    Ok(Some(config))
}

pub fn configure(cmd: &mut Command) -> Result<Option<HostConfig>, String> {
    let Some(source) = config_root(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
    ) else {
        return Ok(None);
    };
    let config = prepare(&source)
        .map_err(|error| format!("prepare Muse host settings for human approvals: {error}"))?;
    if let Some(config) = &config {
        cmd.env("XDG_CONFIG_HOME", &config.root);
        crate::msp::log(
            "using :ask-me for muse serve; saved :auto-review settings remain unchanged",
        );
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_auto_review_changes_and_other_settings_survive() {
        let text = r#"{"schema_version":1,"model":"chosen-model","reasoning_effort":"xhigh","permissions":{"schema_version":1,"default_profile":":auto-review","profiles":[{"id":"custom"}]},"unknown":{"number":12345678901234567890}}"#;
        let actual = host_settings(text).unwrap();
        assert_eq!(actual, text.replace(":auto-review", ":ask-me"));
        for profile in [":ask-me", ":read-only", ":unrestricted", "custom"] {
            assert!(host_settings(&text.replace(":auto-review", profile)).is_none());
        }
        for text in ["{}", "null", "malformed", r#"{"permissions":null}"#] {
            assert!(host_settings(text).is_none());
        }
    }

    #[test]
    fn xdg_root_takes_precedence_over_home() {
        assert_eq!(
            config_root(Some("custom-config".into()), Some("user-home".into())),
            Some(PathBuf::from("custom-config"))
        );
        assert_eq!(
            config_root(Some("".into()), Some("user-home".into())),
            Some(PathBuf::from("user-home").join(".config"))
        );
        assert_eq!(config_root(None, None), None);
    }

    #[test]
    fn absent_settings_need_no_overlay() {
        let source = private_temp_dir().unwrap();
        assert!(prepare(&source.root).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn overlay_links_other_config_and_cleans_up_without_changing_sources() {
        use std::os::unix::fs::PermissionsExt;

        let source = private_temp_dir().unwrap();
        let muse = source.root.join("muse");
        fs::create_dir(&muse).unwrap();
        fs::create_dir(muse.join("skills")).unwrap();
        fs::create_dir(source.root.join("other-app")).unwrap();
        let original = r#"{"permissions":{"default_profile":":auto-review"},"model":"chosen"}"#;
        fs::write(muse.join("settings.json"), original).unwrap();
        fs::write(muse.join("auth.json"), "fixture credential").unwrap();
        fs::write(muse.join("trust.json"), "fixture trust").unwrap();
        fs::write(muse.join(".settings.json.lock"), "original lock").unwrap();
        let overlay = prepare(&source.root).unwrap().unwrap();
        let root = overlay.root.clone();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in [
            "muse/auth.json",
            "muse/trust.json",
            "muse/skills",
            "other-app",
        ] {
            assert_eq!(
                fs::read_link(root.join(name)).unwrap(),
                source.root.join(name)
            );
        }
        assert!(!root.join("muse/.settings.json.lock").exists());
        assert_eq!(
            fs::read_to_string(root.join("muse/settings.json")).unwrap(),
            original.replace(":auto-review", ":ask-me")
        );
        drop(overlay);
        assert!(!root.exists());
        assert_eq!(
            fs::read_to_string(muse.join("settings.json")).unwrap(),
            original
        );
        assert!(muse.join("auth.json").exists());
        assert!(muse.join("skills").is_dir());
        assert!(source.root.join("other-app").is_dir());
    }
}
