//! Unprivileged Windows links for the settings view, declared here so the
//! crate keeps zero dependencies.
//!
//! Symbolic links need Developer Mode or elevation. Folders are linked with
//! directory junctions, which need neither. Files use symbolic links when
//! Windows allows them, and hard links otherwise. A hard link shares the file,
//! not the path, so when Muse saves a file by renaming a new one over it the
//! new file stays in the view. `Links` moves such files back over their
//! originals. Editors usually stop agents by terminating them, which skips
//! `Drop`, so every view records its hard links in a manifest and holds an
//! owner file open; the next launch finishes the restore for any view whose
//! owner is gone (`sweep`).

use std::ffi::c_void;
use std::fs;
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::json::{J, esc, parse_json};

const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const MAXIMUM_REPARSE_DATA_BUFFER_SIZE: usize = 16 * 1024;
/// `FILE_INFO_BY_HANDLE_CLASS::FileIdInfo`.
const FILE_ID_INFO: i32 = 18;
const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;

/// Held open, without delete sharing, for as long as the view's owner runs.
const OWNER: &str = ".muse-acp-owner";
/// One JSON line per hard link: what to restore if the owner never does.
const MANIFEST: &str = ".muse-acp-links";
const VIEW_PREFIX: &str = "muse-acp-config-";

/// Set after the first refused symbolic link, so later files go straight to
/// hard links. Tests set it to exercise the hard-link path on elevated CI.
pub(super) static FILE_SYMLINKS_DENIED: AtomicBool = AtomicBool::new(false);

/// FILE_ID_INFO: a 64-bit volume serial number and a 128-bit file id, which
/// stay unique on ReFS, unlike BY_HANDLE_FILE_INFORMATION's 64-bit index.
#[repr(C)]
#[derive(Default)]
struct FileIdInfo {
    volume_serial_number: u64,
    file_id: [u8; 16],
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
    fn GetFileInformationByHandleEx(
        file: *mut c_void,
        class: i32,
        information: *mut c_void,
        size: u32,
    ) -> i32;
}

/// Logs without panicking: restores run in `Drop`, possibly after the editor
/// closed the adapter's stderr.
fn log(msg: &str) {
    let _ = writeln!(io::stderr(), "[muse-acp] {msg}");
}

/// Links a folder with a junction, or with a directory symbolic link when a
/// junction cannot reach it (network locations).
pub fn link_dir(source: &Path, target: &Path) -> io::Result<()> {
    match junction(source, target) {
        Ok(()) => Ok(()),
        Err(junction_error) => {
            std::os::windows::fs::symlink_dir(source, target).map_err(|symlink_error| {
                io::Error::new(
                    junction_error.kind(),
                    format!("junction: {junction_error}; symbolic link: {symlink_error}"),
                )
            })
        }
    }
}

/// Creates an NTFS directory junction at `target` that points to `source`.
/// Like a directory symbolic link it resolves by path, so changes made
/// inside it reach the source folder, but it needs no privilege.
pub fn junction(source: &Path, target: &Path) -> io::Result<()> {
    let print = drive_path(source)?;
    let mut substitute: Vec<u16> = r"\??\".encode_utf16().collect();
    substitute.extend_from_slice(&print);
    let buffer = mount_point(&substitute, &print)?;
    fs::create_dir(target)?;
    // A junction to a mapped network drive is created but never resolves.
    let result = set_reparse_point(target, &buffer).and_then(|()| fs::metadata(target).map(drop));
    if result.is_err() {
        let _ = fs::remove_dir(target);
    }
    result
}

/// The UTF-16 `C:\...` form of an absolute local path. Junctions can only
/// point at local volumes, so UNC and device paths are refused.
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
/// offsets and lengths, then the NUL-terminated substitute and print names.
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

/// Which file a path names, and the content stamp it had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileState {
    volume: u64,
    id: u128,
    written: u64,
    size: u64,
}

impl FileState {
    fn read(path: &Path) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let mut info = FileIdInfo::default();
        // SAFETY: the handle stays open for the call, and `info` is a
        // writable FILE_ID_INFO of the stated size.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FILE_ID_INFO,
                (&mut info as *mut FileIdInfo).cast(),
                std::mem::size_of::<FileIdInfo>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let metadata = file.metadata()?;
        Ok(FileState {
            volume: info.volume_serial_number,
            id: u128::from_le_bytes(info.file_id),
            written: metadata.last_write_time(),
            size: metadata.file_size(),
        })
    }

    fn same_file(&self, other: &FileState) -> bool {
        self.volume == other.volume && self.id == other.id
    }
}

/// A hard link from the view to an original file.
pub struct HardLink {
    source: PathBuf,
    target: PathBuf,
    state: FileState,
}

impl HardLink {
    /// Hard links need no privilege, but the view and the original must
    /// share a volume.
    pub fn create(source: &Path, target: &Path) -> io::Result<Self> {
        fs::hard_link(source, target)?;
        Ok(HardLink {
            source: source.to_path_buf(),
            target: target.to_path_buf(),
            state: FileState::read(target)?,
        })
    }

    /// Moves a file Muse replaced in the view back over its original, as
    /// Muse would have done on the real path. The file is moved, never
    /// copied. An original that also changed since launch is newer, so it is
    /// kept and the view's file is removed with the view.
    pub fn restore(&self) {
        match FileState::read(&self.target) {
            Ok(current) if !current.same_file(&self.state) => {}
            // Unchanged, rewritten in place (already in the original), or
            // removed (a removed link never removes its original).
            _ => return,
        }
        // Replace the file a symbolic link names, never the link itself.
        let destination = fs::canonicalize(&self.source).unwrap_or_else(|_| self.source.clone());
        let name = self.source.display();
        if FileState::read(&destination).ok() != Some(self.state) {
            log(&format!(
                "kept {name}: it changed outside muse serve while the host ran, so the host's change to it was discarded"
            ));
            return;
        }
        if let Err(error) = fs::rename(&self.target, &destination) {
            log(&format!(
                "could not save muse serve's change to {name}: {error}"
            ));
        }
    }

    /// The original was replaced outside the view, for example by `muse
    /// login` in a terminal, while the view still links the old file.
    fn stale(&self) -> bool {
        let view_unchanged =
            FileState::read(&self.target).is_ok_and(|current| current.same_file(&self.state));
        let original_replaced =
            FileState::read(&self.source).is_ok_and(|current| !current.same_file(&self.state));
        view_unchanged && original_replaced
    }

    fn to_json(&self) -> String {
        format!(
            "{{\"source\":{},\"target\":{},\"volume\":\"{}\",\"id\":\"{}\",\"written\":\"{}\",\"size\":\"{}\"}}",
            esc(&self.source.to_string_lossy()),
            esc(&self.target.to_string_lossy()),
            self.state.volume,
            self.state.id,
            self.state.written,
            self.state.size,
        )
    }

    fn from_json(line: &str) -> Option<Self> {
        let value = parse_json(line).ok()?;
        let text = |key: &str| value.get(key).and_then(J::as_str);
        Some(HardLink {
            source: PathBuf::from(text("source")?),
            target: PathBuf::from(text("target")?),
            state: FileState {
                volume: text("volume")?.parse().ok()?,
                id: text("id")?.parse().ok()?,
                written: text("written")?.parse().ok()?,
                size: text("size")?.parse().ok()?,
            },
        })
    }
}

/// A Muse file that did not exist at launch, on an account without symbolic
/// links. Muse would create it in the view, so it is moved to the real
/// folder when the view goes away, unless the real folder has one by then.
pub struct Pending {
    source: PathBuf,
    target: PathBuf,
}

impl Pending {
    fn keep(&self) {
        if fs::symlink_metadata(&self.target).is_err() || fs::symlink_metadata(&self.source).is_ok()
        {
            return;
        }
        if let Err(error) = move_new(&self.target, &self.source) {
            log(&format!(
                "could not keep the new {}: {error}",
                self.source.display()
            ));
        }
    }

    fn to_json(&self) -> String {
        format!(
            "{{\"pending\":true,\"source\":{},\"target\":{}}}",
            esc(&self.source.to_string_lossy()),
            esc(&self.target.to_string_lossy()),
        )
    }

    fn from_json(line: &str) -> Option<Self> {
        let value = parse_json(line).ok()?;
        let text = |key: &str| value.get(key).and_then(J::as_str);
        matches!(value.get("pending"), Some(J::Bool(true))).then_some(())?;
        Some(Pending {
            source: PathBuf::from(text("source")?),
            target: PathBuf::from(text("target")?),
        })
    }
}

/// Moves a file to a path that must not exist yet: the move never replaces
/// a file. Across volumes it copies, then removes the original.
fn move_new(from: &Path, to: &Path) -> io::Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => return fs::remove_file(from),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(error),
        Err(_) => {}
    }
    let mut reader = fs::File::open(from)?;
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    io::copy(&mut reader, &mut writer)?;
    writer.sync_all()?;
    drop(writer);
    fs::remove_file(from)
}

/// The view's ownership marker, manifest, hard links, and pending files.
#[derive(Default)]
pub struct Links {
    owner: Option<fs::File>,
    manifest: Option<fs::File>,
    pub(super) hard: Vec<HardLink>,
    pending: Vec<Pending>,
}

impl Links {
    /// Marks the view as owned by this live process and starts its manifest,
    /// so a later launch can finish the restore if this one never does.
    pub fn claim(&mut self, root: &Path) -> io::Result<()> {
        self.owner = Some(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .share_mode(FILE_SHARE_READ)
                .open(root.join(OWNER))?,
        );
        self.manifest = Some(
            fs::OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(root.join(MANIFEST))?,
        );
        Ok(())
    }

    pub fn link_file(&mut self, source: &Path, target: &Path) -> io::Result<()> {
        let mut refused = None;
        if !FILE_SYMLINKS_DENIED.load(Ordering::Relaxed) {
            match std::os::windows::fs::symlink_file(source, target) {
                Ok(()) => return Ok(()),
                Err(error) if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
                    FILE_SYMLINKS_DENIED.store(true, Ordering::Relaxed);
                }
                Err(error) => refused = Some(error),
            }
        }
        let link = HardLink::create(source, target).map_err(|error| match &refused {
            Some(symlink_error) => io::Error::new(
                error.kind(),
                format!("symbolic link: {symlink_error}; hard link: {error}"),
            ),
            None => error,
        })?;
        self.record(&link.to_json())?;
        self.hard.push(link);
        Ok(())
    }

    /// Links a Muse file that does not exist yet: a dangling symbolic link
    /// when Windows allows one, otherwise a pending entry.
    pub fn link_missing(&mut self, source: &Path, target: &Path) -> io::Result<()> {
        if !FILE_SYMLINKS_DENIED.load(Ordering::Relaxed) {
            match std::os::windows::fs::symlink_file(source, target) {
                Ok(()) => return Ok(()),
                Err(error) if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
                    FILE_SYMLINKS_DENIED.store(true, Ordering::Relaxed);
                }
                Err(error) => return Err(error),
            }
        }
        let pending = Pending {
            source: source.to_path_buf(),
            target: target.to_path_buf(),
        };
        self.record(&pending.to_json())?;
        self.pending.push(pending);
        Ok(())
    }

    fn record(&mut self, line: &str) -> io::Result<()> {
        match &mut self.manifest {
            Some(manifest) => writeln!(manifest, "{line}"),
            None => Ok(()),
        }
    }

    /// Links pending files that appeared in the real folder, and relinks
    /// hard links whose original was replaced, so the running host sees
    /// changes made outside it, such as a login in a terminal.
    pub fn refresh(&mut self) {
        let mut fresh = Vec::new();
        // Files that appeared in the real folder after launch.
        self.pending.retain(|pending| {
            if fs::symlink_metadata(&pending.target).is_ok()
                || fs::symlink_metadata(&pending.source).is_err()
            {
                return true;
            }
            match HardLink::create(&pending.source, &pending.target) {
                Ok(link) => {
                    fresh.push(link);
                    false
                }
                Err(error) => {
                    log(&format!(
                        "could not link the new {}: {error}",
                        pending.source.display()
                    ));
                    true
                }
            }
        });
        // Originals replaced while the view still links the old file.
        let mut kept = Vec::new();
        for link in std::mem::take(&mut self.hard) {
            if !link.stale() {
                kept.push(link);
                continue;
            }
            match fs::remove_file(&link.target)
                .and_then(|()| HardLink::create(&link.source, &link.target))
            {
                Ok(relinked) => fresh.push(relinked),
                Err(error) => {
                    log(&format!(
                        "could not relink {}: {error}",
                        link.source.display()
                    ));
                    kept.push(link);
                }
            }
        }
        self.hard = kept;
        for link in fresh {
            if let Err(error) = self.record(&link.to_json()) {
                log(&format!("could not record a refreshed link: {error}"));
            }
            self.hard.push(link);
        }
    }

    /// Closes the owner and manifest without restoring, as when the process
    /// is terminated.
    #[cfg(test)]
    pub(super) fn abandon(&mut self) {
        self.owner = None;
        self.manifest = None;
        self.hard.clear();
        self.pending.clear();
    }

    /// Restores every hard-linked file and keeps new files, then releases
    /// the owner file so the view can be removed.
    pub fn restore(&mut self) {
        for link in &self.hard {
            link.restore();
        }
        for pending in &self.pending {
            pending.keep();
        }
        self.manifest = None;
        self.owner = None;
    }
}

/// Finishes the views earlier adapters left behind. A view whose owner file
/// can be deleted has no live owner: restore its hard-linked files, then
/// remove it. Views without a manifest are left alone; an older adapter may
/// still be using one.
pub fn sweep() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(VIEW_PREFIX) {
            continue;
        }
        let root = entry.path();
        if !root.join(MANIFEST).is_file() {
            continue;
        }
        match fs::remove_file(root.join(OWNER)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            // Still open: its adapter is running.
            Err(_) => continue,
        }
        let manifest = fs::read_to_string(root.join(MANIFEST)).unwrap_or_default();
        for line in manifest.lines() {
            if let Some(pending) = Pending::from_json(line) {
                pending.keep();
            } else if let Some(link) = HardLink::from_json(line) {
                link.restore();
            }
        }
        if let Err(error) = fs::remove_dir_all(&root) {
            log(&format!(
                "could not remove a stale settings view at {}: {error}",
                root.display()
            ));
        }
    }
}
