//! Process-local compatibility for Muse's TUI-only auto-review profile.
//!
//! `muse serve` has no permission-profile flag or settings-file override. Use
//! a private XDG config view only when the saved built-in profile needs the
//! unavailable reviewer. Everything except settings is linked, not copied.
//! Windows links folders with junctions and, without the symbolic-link
//! privilege, files with hard links; neither needs Developer Mode.

#[cfg(windows)]
mod windows;

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::json::{J, j_to_string, parse_json};

pub struct HostConfig {
    root: PathBuf,
    #[cfg(windows)]
    links: windows::Links,
}

impl HostConfig {
    /// Brings files that changed in the real Muse folder while the host
    /// runs, such as credentials from `muse login` in a terminal, into the
    /// view. Only Windows hard links need this; links elsewhere resolve by
    /// path.
    pub fn refresh(&mut self) {
        #[cfg(windows)]
        self.links.refresh();
    }
}

impl Drop for HostConfig {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.links.restore();
        // remove_dir_all removes the links themselves, never their targets.
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Files Muse keeps next to its settings that a session can create: the
/// credential a login writes, workspace trust, and their locks. One that does
/// not exist yet still gets an entry in the view, so Muse creates it in the
/// real folder instead of in the view, where it would be lost.
const MUSE_FILES: [&str; 4] = [
    "auth.json",
    ".auth.json.lock",
    "trust.json",
    ".trust.json.lock",
];

/// Why the latest launch ran without a view, for the editor-facing hint if
/// Muse then refuses the saved profile.
static VIEW_FAILURE: Mutex<Option<String>> = Mutex::new(None);

pub fn view_failure() -> Option<String> {
    VIEW_FAILURE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
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
            Ok(()) => {
                return Ok(HostConfig {
                    root,
                    #[cfg(windows)]
                    links: windows::Links::default(),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
fn link_entry(_config: &mut HostConfig, source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn link_entry(config: &mut HostConfig, source: &Path, target: &Path) -> io::Result<()> {
    if source.is_dir() {
        windows::link_dir(source, target)
    } else {
        config.links.link_file(source, target)
    }
}

/// Links a Muse file that does not exist yet. A dangling symbolic link
/// works: Muse follows it and creates the real file.
#[cfg(unix)]
fn link_missing(_config: &mut HostConfig, source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn link_missing(config: &mut HostConfig, source: &Path, target: &Path) -> io::Result<()> {
    config.links.link_missing(source, target)
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
    let mut config = private_temp_dir()?;
    #[cfg(windows)]
    config.links.claim(&config.root)?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if entry.file_name() != "muse" {
            // Other apps' settings only matter to tools muse serve runs, so
            // one that cannot be linked is left out instead of failing.
            let target = config.root.join(entry.file_name());
            if let Err(error) = link_entry(&mut config, &entry.path(), &target) {
                crate::msp::log(&format!(
                    "left {} out of the Muse settings view: {error}",
                    entry.path().display()
                ));
            }
        }
    }
    let muse = config.root.join("muse");
    fs::create_dir(&muse)?;
    for entry in fs::read_dir(source.join("muse"))? {
        let entry = entry?;
        if entry.file_name() != "settings.json" && entry.file_name() != ".settings.json.lock" {
            link_entry(&mut config, &entry.path(), &muse.join(entry.file_name()))?;
        }
    }
    for name in MUSE_FILES {
        let file = source.join("muse").join(name);
        let missing = matches!(
            fs::symlink_metadata(&file),
            Err(error) if error.kind() == io::ErrorKind::NotFound
        );
        if missing && let Err(error) = link_missing(&mut config, &file, &muse.join(name)) {
            crate::msp::log(&format!(
                "a new {name} would stay in the Muse settings view: {error}"
            ));
        }
    }
    fs::write(muse.join("settings.json"), settings)?;
    Ok(Some(config))
}

/// Never fails the launch: without a view, `muse serve` reads the saved
/// settings itself, and a refused profile reaches the editor with guidance.
pub fn configure(cmd: &mut Command) -> Option<HostConfig> {
    let source = config_root(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
    )?;
    configure_from(cmd, &source)
}

fn configure_from(cmd: &mut Command, source: &Path) -> Option<HostConfig> {
    #[cfg(windows)]
    windows::sweep();
    let prepared = prepare(source);
    *VIEW_FAILURE.lock().unwrap_or_else(|p| p.into_inner()) =
        prepared.as_ref().err().map(ToString::to_string);
    let config = match prepared {
        Ok(config) => config?,
        Err(error) => {
            crate::msp::log(&format!(
                "could not prepare the settings view for human approvals ({error}); muse serve uses the saved Muse settings, which may refuse a saved :auto-review profile"
            ));
            return None;
        }
    };
    cmd.env("XDG_CONFIG_HOME", &config.root);
    crate::msp::log("using :ask-me for muse serve; saved :auto-review settings remain unchanged");
    Some(config)
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

    #[cfg(unix)]
    #[test]
    fn files_muse_creates_later_land_in_the_real_folder() {
        let source = private_temp_dir().unwrap();
        let muse = source.root.join("muse");
        fs::create_dir(&muse).unwrap();
        fs::write(
            muse.join("settings.json"),
            r#"{"permissions":{"default_profile":":auto-review"}}"#,
        )
        .unwrap();
        let overlay = prepare(&source.root).unwrap().unwrap();
        let view = overlay.root.join("muse");
        for name in MUSE_FILES {
            assert_eq!(
                fs::read_link(view.join(name)).unwrap(),
                muse.join(name),
                "{name}"
            );
            assert!(!muse.join(name).exists(), "{name}");
        }
        // Muse trusts a workspace for the first time.
        fs::write(view.join("trust.json"), "first trust").unwrap();
        // The user logs in from a terminal while the host runs.
        fs::write(muse.join("auth.json"), "fresh credential").unwrap();
        assert_eq!(
            fs::read_to_string(view.join("auth.json")).unwrap(),
            "fresh credential"
        );
        drop(overlay);
        assert_eq!(
            fs::read_to_string(muse.join("trust.json")).unwrap(),
            "first trust"
        );
        assert_eq!(
            fs::read_to_string(muse.join("auth.json")).unwrap(),
            "fresh credential"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unprepared_view_launches_with_saved_settings() {
        use std::os::unix::fs::PermissionsExt;

        let source = private_temp_dir().unwrap();
        let muse = source.root.join("muse");
        fs::create_dir(&muse).unwrap();
        fs::write(
            muse.join("settings.json"),
            r#"{"permissions":{"default_profile":":auto-review"}}"#,
        )
        .unwrap();
        // The settings stay readable, but the folder cannot be listed.
        fs::set_permissions(&muse, fs::Permissions::from_mode(0o100)).unwrap();
        let listable = fs::read_dir(&muse).is_ok();
        let mut cmd = Command::new("muse");
        let config = configure_from(&mut cmd, &source.root);
        fs::set_permissions(&muse, fs::Permissions::from_mode(0o700)).unwrap();
        if listable {
            // Permissions do not bind this user (for example, root).
            return;
        }
        assert!(config.is_none());
        assert!(cmd.get_envs().all(|(key, _)| key != "XDG_CONFIG_HOME"));
    }

    /// How Muse saves a file: write a new one, then rename it over the old.
    #[cfg(windows)]
    fn muse_save(path: &Path, text: &str) {
        let temp = path.with_extension("tmp");
        fs::write(&temp, text).unwrap();
        fs::rename(&temp, path).unwrap();
    }

    /// A source config root and the view prepared from it, with files
    /// hard-linked as on a standard Windows account (CI runs elevated).
    #[cfg(windows)]
    fn windows_overlay(original: &str) -> (HostConfig, HostConfig) {
        windows::FILE_SYMLINKS_DENIED.store(true, Ordering::Relaxed);
        let source = private_temp_dir().unwrap();
        let muse = source.root.join("muse");
        fs::create_dir(&muse).unwrap();
        fs::create_dir(muse.join("skills")).unwrap();
        fs::write(muse.join("skills/SKILL.md"), "fixture skill").unwrap();
        fs::create_dir(source.root.join("other-app")).unwrap();
        fs::write(source.root.join("other-app/config"), "fixture other").unwrap();
        fs::write(muse.join("settings.json"), original).unwrap();
        fs::write(muse.join("auth.json"), "fixture credential").unwrap();
        fs::write(muse.join("trust.json"), "fixture trust").unwrap();
        fs::write(muse.join(".settings.json.lock"), "original lock").unwrap();
        let overlay = prepare(&source.root).unwrap().unwrap();
        (source, overlay)
    }

    #[cfg(windows)]
    #[test]
    fn overlay_links_other_config_and_cleans_up_without_changing_sources() {
        let original = r#"{"permissions":{"default_profile":":auto-review"},"model":"chosen"}"#;
        let (source, overlay) = windows_overlay(original);
        let muse = source.root.join("muse");
        let root = overlay.root.clone();
        assert!(overlay.links.hard.len() >= 2, "files must be hard links");
        for (name, text) in [
            ("muse/auth.json", "fixture credential"),
            ("muse/trust.json", "fixture trust"),
            ("muse/skills/SKILL.md", "fixture skill"),
            ("other-app/config", "fixture other"),
        ] {
            assert_eq!(fs::read_to_string(root.join(name)).unwrap(), text, "{name}");
        }
        for name in ["muse/skills", "other-app"] {
            let kind = fs::symlink_metadata(root.join(name)).unwrap().file_type();
            assert!(kind.is_symlink(), "{name} must be a link, not a copy");
        }
        fs::write(root.join("other-app/new"), "through the view").unwrap();
        assert_eq!(
            fs::read_to_string(source.root.join("other-app/new")).unwrap(),
            "through the view"
        );
        assert!(!root.join("muse/.settings.json.lock").exists());
        assert_eq!(
            fs::read_to_string(root.join("muse/settings.json")).unwrap(),
            original.replace(":auto-review", ":ask-me")
        );
        // Muse refreshes a credential while the host runs.
        muse_save(&root.join("muse/auth.json"), "refreshed credential");
        assert_eq!(
            fs::read_to_string(muse.join("auth.json")).unwrap(),
            "fixture credential"
        );
        drop(overlay);
        assert!(!root.exists());
        assert_eq!(
            fs::read_to_string(muse.join("settings.json")).unwrap(),
            original
        );
        assert_eq!(
            fs::read_to_string(muse.join("auth.json")).unwrap(),
            "refreshed credential"
        );
        assert_eq!(
            fs::read_to_string(muse.join("trust.json")).unwrap(),
            "fixture trust"
        );
        assert_eq!(
            fs::read_to_string(muse.join("skills/SKILL.md")).unwrap(),
            "fixture skill"
        );
        assert!(source.root.join("other-app/config").exists());
        assert!(source.root.join("other-app/new").exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_later_launch_restores_a_view_whose_adapter_was_terminated() {
        let original = r#"{"permissions":{"default_profile":":auto-review"}}"#;
        let (source, mut overlay) = windows_overlay(original);
        let root = overlay.root.clone();
        muse_save(&root.join("muse/auth.json"), "refreshed credential");
        // Live views are left alone.
        windows::sweep();
        assert!(root.exists());
        // Terminated: the owner file closes, and Drop never runs.
        overlay.links.abandon();
        std::mem::forget(overlay);
        windows::sweep();
        assert!(!root.exists());
        assert_eq!(
            fs::read_to_string(source.root.join("muse/auth.json")).unwrap(),
            "refreshed credential"
        );
    }

    /// A config root with only auto-review settings, prepared as on a
    /// standard Windows account.
    #[cfg(windows)]
    fn bare_windows_overlay() -> (HostConfig, HostConfig) {
        windows::FILE_SYMLINKS_DENIED.store(true, Ordering::Relaxed);
        let source = private_temp_dir().unwrap();
        fs::create_dir(source.root.join("muse")).unwrap();
        fs::write(
            source.root.join("muse/settings.json"),
            r#"{"permissions":{"default_profile":":auto-review"}}"#,
        )
        .unwrap();
        let overlay = prepare(&source.root).unwrap().unwrap();
        (source, overlay)
    }

    #[cfg(windows)]
    #[test]
    fn without_symbolic_links_new_files_are_kept_and_a_login_reaches_the_host() {
        let (source, mut overlay) = bare_windows_overlay();
        let muse = source.root.join("muse");
        let view = overlay.root.join("muse");
        // Muse trusts a workspace for the first time, inside the view.
        fs::write(view.join("trust.json"), "first trust").unwrap();
        // The user logs in from a terminal; the editor then authenticates,
        // which refreshes the view.
        fs::write(muse.join("auth.json"), "first credential").unwrap();
        assert!(!view.join("auth.json").exists());
        overlay.refresh();
        assert_eq!(
            fs::read_to_string(view.join("auth.json")).unwrap(),
            "first credential"
        );
        // A second login replaces the credential.
        muse_save(&muse.join("auth.json"), "second credential");
        assert_eq!(
            fs::read_to_string(view.join("auth.json")).unwrap(),
            "first credential"
        );
        overlay.refresh();
        assert_eq!(
            fs::read_to_string(view.join("auth.json")).unwrap(),
            "second credential"
        );
        // Muse removes the view's entry; the next refresh links it again.
        fs::remove_file(view.join("auth.json")).unwrap();
        overlay.refresh();
        assert_eq!(
            fs::read_to_string(view.join("auth.json")).unwrap(),
            "second credential"
        );
        drop(overlay);
        assert_eq!(
            fs::read_to_string(muse.join("trust.json")).unwrap(),
            "first trust"
        );
        assert_eq!(
            fs::read_to_string(muse.join("auth.json")).unwrap(),
            "second credential"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_logout_is_not_undone_by_a_later_launch() {
        let (source, mut overlay) = bare_windows_overlay();
        let muse = source.root.join("muse");
        let root = overlay.root.clone();
        fs::write(muse.join("auth.json"), "credential").unwrap();
        overlay.refresh();
        assert!(root.join("muse/auth.json").exists());
        // `muse logout` in a terminal, then the editor terminates the agent.
        fs::remove_file(muse.join("auth.json")).unwrap();
        overlay.links.abandon();
        std::mem::forget(overlay);
        windows::sweep();
        assert!(!root.exists());
        assert!(!muse.join("auth.json").exists(), "the logout must stand");
    }

    #[cfg(windows)]
    #[test]
    fn a_later_launch_keeps_the_new_files_of_a_terminated_adapter() {
        let (source, mut overlay) = bare_windows_overlay();
        let root = overlay.root.clone();
        fs::write(root.join("muse/trust.json"), "first trust").unwrap();
        // A file that also appeared in the real folder is never replaced.
        fs::write(root.join("muse/auth.json"), "view credential").unwrap();
        fs::write(source.root.join("muse/auth.json"), "real credential").unwrap();
        overlay.links.abandon();
        std::mem::forget(overlay);
        windows::sweep();
        assert!(!root.exists());
        assert_eq!(
            fs::read_to_string(source.root.join("muse/trust.json")).unwrap(),
            "first trust"
        );
        assert_eq!(
            fs::read_to_string(source.root.join("muse/auth.json")).unwrap(),
            "real credential"
        );
    }

    #[cfg(windows)]
    #[test]
    fn junctions_need_a_local_absolute_target() {
        let view = private_temp_dir().unwrap();
        for target in [r"\\server\share\config", r"relative\config", r"C:relative"] {
            let error = windows::junction(Path::new(target), &view.root.join("link")).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported, "{target}");
            assert!(!view.root.join("link").exists());
        }
        // A target that does not resolve leaves no junction behind.
        let missing = view.root.join("missing");
        assert!(windows::junction(&missing, &view.root.join("dangling")).is_err());
        assert!(!view.root.join("dangling").exists());
        let source = private_temp_dir().unwrap();
        fs::write(source.root.join("kept.txt"), "verbatim").unwrap();
        let verbatim = PathBuf::from(format!(r"\\?\{}", source.root.display()));
        windows::junction(&verbatim, &view.root.join("verbatim")).unwrap();
        assert_eq!(
            fs::read_to_string(view.root.join("verbatim/kept.txt")).unwrap(),
            "verbatim"
        );
        drop(view);
        assert!(source.root.join("kept.txt").exists());
    }

    #[cfg(windows)]
    #[test]
    fn hard_link_replacements_move_back_only_over_an_unchanged_source() {
        let dir = private_temp_dir().unwrap();
        let source = dir.root.join("auth.json");
        let target = dir.root.join("view-auth.json");

        // Muse's save moves back.
        fs::write(&source, "one").unwrap();
        let link = windows::HardLink::create(&source, &target).unwrap();
        muse_save(&target, "two");
        assert_eq!(fs::read_to_string(&source).unwrap(), "one");
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "two");
        assert!(!target.exists(), "the file is moved, not copied");

        // In-place writes already reach the source.
        let link = windows::HardLink::create(&source, &target).unwrap();
        fs::write(&target, "three").unwrap();
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "three");

        // A removed link never removes its source.
        fs::remove_file(&target).unwrap();
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "three");

        // A source replaced outside the host is newer, so it is kept.
        let link = windows::HardLink::create(&source, &target).unwrap();
        muse_save(&target, "host change");
        muse_save(&source, "outside change");
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "outside change");
        fs::remove_file(&target).unwrap();

        // So is a source rewritten in place.
        let link = windows::HardLink::create(&source, &target).unwrap();
        muse_save(&target, "host change");
        fs::write(&source, "outside edit, longer").unwrap();
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "outside edit, longer");
    }
}
