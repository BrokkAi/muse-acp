//! Process-local compatibility for Muse's TUI-only auto-review profile.
//!
//! `muse serve` has no permission-profile flag or settings-file override. Use
//! a private XDG config view only when the saved built-in profile needs the
//! unavailable reviewer. Everything except settings is linked, not copied.
//! Windows links folders with junctions and, without the symbolic-link
//! privilege, files with hard links; neither needs Developer Mode.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::json::{J, j_to_string, parse_json};

pub struct HostConfig {
    root: PathBuf,
    #[cfg(windows)]
    hard_links: Vec<windows::HardLink>,
}

impl Drop for HostConfig {
    fn drop(&mut self) {
        #[cfg(windows)]
        for link in &self.hard_links {
            link.restore();
        }
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
            Ok(()) => {
                return Ok(HostConfig {
                    root,
                    #[cfg(windows)]
                    hard_links: Vec::new(),
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

/// Symbolic links need Developer Mode or elevation on Windows. Junctions and
/// hard links do not, so they are the fallback for a standard account.
#[cfg(windows)]
fn link_entry(config: &mut HostConfig, source: &Path, target: &Path) -> io::Result<()> {
    if source.is_dir() {
        return match windows::junction(source, target) {
            Ok(()) => Ok(()),
            // Junctions cannot point at network shares.
            Err(error) => std::os::windows::fs::symlink_dir(source, target).map_err(|_| error),
        };
    }
    if std::os::windows::fs::symlink_file(source, target).is_ok() {
        return Ok(());
    }
    config.hard_links.push(windows::hard_link(source, target)?);
    Ok(())
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
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if entry.file_name() != "muse" {
            let target = config.root.join(entry.file_name());
            link_entry(&mut config, &entry.path(), &target)?;
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
    let config = match prepare(source) {
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

/// Unprivileged Windows links, declared here so the crate keeps zero
/// dependencies.
#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::fs;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Path, PathBuf, Prefix};

    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const MAXIMUM_REPARSE_DATA_BUFFER_SIZE: usize = 16 * 1024;

    /// BY_HANDLE_FILE_INFORMATION; only the identity fields are read.
    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation_time: [u32; 2],
        last_access_time: [u32; 2],
        last_write_time: [u32; 2],
        volume_serial_number: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn DeviceIoControl(
            device: *mut c_void,
            code: u32,
            input: *const c_void,
            input_size: u32,
            output: *mut c_void,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn GetFileInformationByHandle(
            file: *mut c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    /// Creates an NTFS directory junction at `target` that points to
    /// `source`. Like a directory symbolic link it resolves by path, so
    /// changes made inside it reach the source folder, but it needs no
    /// privilege.
    pub fn junction(source: &Path, target: &Path) -> io::Result<()> {
        let print = drive_path(source)?;
        let mut substitute: Vec<u16> = r"\??\".encode_utf16().collect();
        substitute.extend_from_slice(&print);
        let buffer = mount_point(&substitute, &print)?;
        fs::create_dir(target)?;
        let result = set_reparse_point(target, &buffer);
        if result.is_err() {
            let _ = fs::remove_dir(target);
        }
        result
    }

    /// The UTF-16 `C:\...` form of an absolute local path. Junctions can
    /// only point at local volumes, so UNC and device paths are refused.
    fn drive_path(path: &Path) -> io::Result<Vec<u16>> {
        let verbatim = match path.components().next() {
            Some(Component::Prefix(prefix)) if path.has_root() => match prefix.kind() {
                Prefix::Disk(_) => false,
                Prefix::VerbatimDisk(_) => true,
                _ => return Err(unsupported(path)),
            },
            _ => return Err(unsupported(path)),
        };
        let wide = path.as_os_str().encode_wide();
        Ok(if verbatim {
            // Drop the `\\?\` prefix.
            wide.skip(4).collect()
        } else {
            wide.map(|unit| {
                if unit == u16::from(b'/') {
                    u16::from(b'\\')
                } else {
                    unit
                }
            })
            .collect()
        })
    }

    fn unsupported(path: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("cannot create a junction to {}", path.display()),
        )
    }

    /// A mount-point REPARSE_DATA_BUFFER: an eight-byte header, four name
    /// offsets and lengths, then the NUL-terminated substitute and print
    /// names.
    fn mount_point(substitute: &[u16], print: &[u16]) -> io::Result<Vec<u8>> {
        let substitute_bytes = substitute.len() * 2;
        let print_bytes = print.len() * 2;
        let data = 8 + substitute_bytes + 2 + print_bytes + 2;
        if 8 + data > MAXIMUM_REPARSE_DATA_BUFFER_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "junction target path is too long",
            ));
        }
        let mut buffer = Vec::with_capacity(8 + data);
        buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        // Every value fits: the whole buffer is at most 16 KiB.
        for field in [
            data,
            0,
            0,
            substitute_bytes,
            substitute_bytes + 2,
            print_bytes,
        ] {
            buffer.extend_from_slice(&(field as u16).to_le_bytes());
        }
        for unit in substitute.iter().chain(&[0]).chain(print).chain(&[0]) {
            buffer.extend_from_slice(&unit.to_le_bytes());
        }
        Ok(buffer)
    }

    fn set_reparse_point(dir: &Path, buffer: &[u8]) -> io::Result<()> {
        let dir = fs::OpenOptions::new()
            .access_mode(GENERIC_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)?;
        let mut returned = 0;
        // SAFETY: the handle stays open for the call, and the input is a
        // complete reparse buffer of the stated length with no output.
        let ok = unsafe {
            DeviceIoControl(
                dir.as_raw_handle(),
                FSCTL_SET_REPARSE_POINT,
                buffer.as_ptr().cast(),
                buffer.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct FileId {
        volume: u32,
        index: u64,
    }

    fn file_id(path: &Path) -> io::Result<FileId> {
        let file = fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let mut info = ByHandleFileInformation::default();
        // SAFETY: the handle stays open for the call, and `info` is a
        // writable BY_HANDLE_FILE_INFORMATION.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(FileId {
            volume: info.volume_serial_number,
            index: (u64::from(info.index_high) << 32) | u64::from(info.index_low),
        })
    }

    /// A hard link to a file in the settings view. Muse saves some files,
    /// credentials included, by renaming a new file over the old one, which
    /// through a hard link leaves the new file in the view.
    pub struct HardLink {
        source: PathBuf,
        target: PathBuf,
        id: FileId,
    }

    /// Hard links need no privilege, but the view and the source must share
    /// a volume.
    pub fn hard_link(source: &Path, target: &Path) -> io::Result<HardLink> {
        fs::hard_link(source, target)?;
        Ok(HardLink {
            source: source.to_path_buf(),
            target: target.to_path_buf(),
            id: file_id(target)?,
        })
    }

    impl HardLink {
        /// Moves a file Muse replaced in the view back over its source, as
        /// Muse would have done on the real path. The file is moved, never
        /// copied. A source that also changed since launch is newer, so it is
        /// kept and the view's file is removed with the view.
        pub fn restore(&self) {
            match file_id(&self.target) {
                Ok(current) if current != self.id => {}
                // Unchanged, rewritten in place (already in the source), or
                // removed (a removed link never removes its source).
                _ => return,
            }
            let name = self.source.display();
            if file_id(&self.source).ok() != Some(self.id) {
                crate::msp::log(&format!(
                    "kept {name}: it changed outside muse serve while the host ran, so the host's change to it was discarded"
                ));
                return;
            }
            if let Err(error) = fs::rename(&self.target, &self.source) {
                crate::msp::log(&format!(
                    "could not save muse serve's change to {name}: {error}"
                ));
            }
        }
    }
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

    #[cfg(windows)]
    #[test]
    fn overlay_links_other_config_and_cleans_up_without_changing_sources() {
        let source = private_temp_dir().unwrap();
        let muse = source.root.join("muse");
        fs::create_dir(&muse).unwrap();
        fs::create_dir(muse.join("skills")).unwrap();
        fs::write(muse.join("skills/SKILL.md"), "fixture skill").unwrap();
        fs::create_dir(source.root.join("other-app")).unwrap();
        fs::write(source.root.join("other-app/config"), "fixture other").unwrap();
        let original = r#"{"permissions":{"default_profile":":auto-review"},"model":"chosen"}"#;
        fs::write(muse.join("settings.json"), original).unwrap();
        fs::write(muse.join("auth.json"), "fixture credential").unwrap();
        fs::write(muse.join("trust.json"), "fixture trust").unwrap();
        fs::write(muse.join(".settings.json.lock"), "original lock").unwrap();
        let overlay = prepare(&source.root).unwrap().unwrap();
        let root = overlay.root.clone();
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
        drop(overlay);
        assert!(!root.exists());
        assert_eq!(
            fs::read_to_string(muse.join("settings.json")).unwrap(),
            original
        );
        assert_eq!(
            fs::read_to_string(muse.join("auth.json")).unwrap(),
            "fixture credential"
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
    fn junctions_need_a_local_absolute_target() {
        let view = private_temp_dir().unwrap();
        for target in [r"\\server\share\config", r"relative\config", r"C:relative"] {
            let error = windows::junction(Path::new(target), &view.root.join("link")).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported, "{target}");
            assert!(!view.root.join("link").exists());
        }
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
        // How Muse saves: write a new file, then rename it over the old one.
        let replace = |path: &Path, text: &str| {
            let temp = path.with_extension("tmp");
            fs::write(&temp, text).unwrap();
            fs::rename(&temp, path).unwrap();
        };

        // Dropping the view moves the replacement back, then removes the view.
        fs::write(&source, "one").unwrap();
        let mut view = private_temp_dir().unwrap();
        let view_auth = view.root.join("auth.json");
        let link = windows::hard_link(&source, &view_auth).unwrap();
        view.hard_links.push(link);
        replace(&view_auth, "two");
        assert_eq!(fs::read_to_string(&source).unwrap(), "one");
        let root = view.root.clone();
        drop(view);
        assert!(!root.exists());
        assert_eq!(fs::read_to_string(&source).unwrap(), "two");

        // In-place writes already reach the source.
        let link = windows::hard_link(&source, &target).unwrap();
        fs::write(&target, "three").unwrap();
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "three");

        // A removed link never removes its source.
        fs::remove_file(&target).unwrap();
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "three");

        // A source replaced outside the host is newer, so it is kept.
        let link = windows::hard_link(&source, &target).unwrap();
        replace(&target, "host change");
        replace(&source, "outside change");
        link.restore();
        assert_eq!(fs::read_to_string(&source).unwrap(), "outside change");
        assert_eq!(fs::read_to_string(&target).unwrap(), "host change");
    }
}
