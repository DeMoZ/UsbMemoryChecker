//! Work that needs administrator rights runs in a second copy of the program started with `--helper <folder>`:
//! the quick probe (raw access to the whole device) and the repair (new partition of the real size).
//!
//! - macOS: `osascript … with administrator privileges` (the system password window).
//! - Windows: `ShellExecuteExW` with the `runas` verb (the UAC window).
//!
//! The window and the helper talk through files in a job folder: `job.json` (what to do), `progress.json`
//! (rewritten while it works), `result.json` (at the end) and `cancel` (created by the window to stop it).
//! Before touching the drive the helper looks it up again among removable drives and checks that its size is
//! still the same, so a stale or forged job cannot reach another disk.
//!
//! The helper also appends one JSON line per stage to `log.jsonl` (the window makes the report from it, as the
//! user). It runs as root in a folder the user owns, so it never writes through a link: it refuses a job folder
//! that is a link, opens the folder once and reaches every file through that handle ([`JobFolder`]), and every file
//! it writes there is a new file that is not a link. After a quick check it closes
//! the device, remounts and compares the partition layout with the one from before ([`LayoutAfter`]).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::capacity::Fingerprint;
use crate::dio;
use crate::disks::{self, Disk};
use crate::probe;
use crate::report;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Probe,
    Fix,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Fs {
    Exfat,
    Fat32,
}

/// FAT32 above this size is not formatted by Windows and is a poor choice anyway.
pub const FAT32_MAX: u64 = 32 * 1000 * 1000 * 1000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub action: Action,
    pub disk: String,
    /// Size the drive reported when the user chose it.
    pub size: u64,
    /// Fix: bytes for the new partition (0 or ≥ size — the whole drive).
    #[serde(default)]
    pub real: u64,
    /// Fix: exact bytes for the new partition (a whole number of MiB). 0 = derive the size from `real`.
    #[serde(default)]
    pub partition: u64,
    /// The partition layout Rust saw when it planned the job (its own list, taken after the window's `layoutKey` was
    /// checked against it by `capacity::check_fix`'s caller). The helper compares it with the drive it finds before it
    /// touches it, so a drive swapped while the password window was open is refused (None: no check, as the command
    /// line).
    #[serde(default)]
    pub layout: Option<crate::capacity::Fingerprint>,
    /// Fix: a format at the full declared size was confirmed (set only by Rust: a drive found genuine by a
    /// whole-device check, or the user's explicit tick; the command line's whole-drive request sets it too). The helper
    /// refuses an `eraseDisk` / full-size `diskpart` without it, whatever else the job says.
    #[serde(default)]
    pub whole_confirmed: bool,
    #[serde(default = "default_fs")]
    pub fs: Fs,
    #[serde(default)]
    pub label: String,
}

fn default_fs() -> Fs {
    Fs::Exfat
}

/// Volume label that every system accepts: A–Z, 0–9, `_`, `-`, up to 11 characters.
pub fn clean_label(label: &str) -> String {
    let l: String =
        label.to_uppercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(11).collect();
    if l.is_empty() { "USB".into() } else { l }
}

/// Partition size for the repair: a little below the real boundary, whole MiB.
pub fn partition_size(real: u64, reported: u64) -> u64 {
    if real == 0 || real >= reported {
        return reported;
    }
    real.saturating_sub(2 << 20) / (1 << 20) * (1 << 20)
}

const MIB: u64 = 1 << 20;

/// Size of the partition the repair creates, or the reason it must not run. Checked before anything is touched.
/// With `partition > 0` it must be a whole number of MiB (or the whole drive), at least 1 MiB and at most the
/// drive's size. Otherwise it is `partition_size(real, size)`, which must not be 0 (a drive that keeps nothing).
pub fn fix_size(job: &Job) -> Result<u64, String> {
    if job.partition > 0 {
        if job.partition > job.size {
            return Err(format!(
                "the partition ({} bytes) is larger than the drive ({} bytes)",
                job.partition, job.size
            ));
        }
        if job.partition < MIB {
            return Err("the partition is smaller than 1 MiB".into());
        }
        if job.partition != job.size && !job.partition.is_multiple_of(MIB) {
            return Err("the partition size must be a whole number of MiB".into());
        }
        return Ok(job.partition);
    }
    match partition_size(job.real, job.size) {
        0 => Err("the drive keeps no usable space; nothing to partition".into()),
        n => Ok(n),
    }
}

// ------------------------------------------------------------------ the helper side (elevated)

/// The job folder as the helper uses it: opened once, and every file is then reached through that one handle. The
/// user owns the folder and the helper runs as root, so the folder's path is never followed again: swapping it for a
/// link later changes nothing.
///
/// Unix: a directory descriptor (`O_DIRECTORY | O_NOFOLLOW`) with `openat` / `renameat` / `unlinkat`.
/// Windows: a handle to the folder without delete sharing (no rename or removal), with `NtCreateFile` relative to
/// it. The event log stays open inside it, also without delete sharing.
pub struct JobFolder {
    #[cfg(unix)]
    dir: File,
    #[cfg(not(any(unix, windows)))]
    path: PathBuf,
    #[cfg(windows)]
    pin: File,
}

#[cfg(unix)]
impl JobFolder {
    /// An error when `path` is not a real folder (a link to one included).
    pub fn open(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path)?;
        Ok(JobFolder { dir })
    }

    fn name(name: &str) -> io::Result<std::ffi::CString> {
        std::ffi::CString::new(name).map_err(io::Error::other)
    }

    /// A file of the folder; a link in its place is an error, never followed.
    fn open_at(&self, name: &str, flags: libc::c_int) -> io::Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = Self::name(name)?;
        let flags = flags | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let mode: libc::c_uint = 0o666;
        // SAFETY: the descriptor of the folder is open and the name is a C string.
        let fd = unsafe { libc::openat(self.dir.as_raw_fd(), name.as_ptr(), flags, mode) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// A new file; an existing file or a link in its place is an error.
    fn create_new(&self, name: &str) -> io::Result<File> {
        self.open_at(name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
    }

    fn create_log(&self) -> io::Result<File> {
        self.create_new("log.jsonl")
    }

    /// The content of a file of the folder: a regular file with this one name, owned by the folder's owner. Anything
    /// else (a hard link to a file only root may read, say) is refused before a byte of it is read, so none of it
    /// can end up in an error text of the result.
    fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        use std::os::unix::fs::MetadataExt;
        let mut file = self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK)?;
        let found = file.metadata()?;
        if !is_own_file(found.is_file(), found.nlink(), found.uid(), self.dir.metadata()?.uid()) {
            return Err(not_a_job_file());
        }
        let mut data = Vec::new();
        io::Read::read_to_end(&mut file, &mut data)?;
        Ok(data)
    }

    fn exists(&self, name: &str) -> bool {
        self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK).is_ok()
    }

    /// Removes the entry (a link itself, never its target).
    fn remove(&self, name: &str) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let name = Self::name(name)?;
        // SAFETY: as in `open_at`.
        match unsafe { libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), 0) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let (from, to) = (Self::name(from)?, Self::name(to)?);
        let fd = self.dir.as_raw_fd();
        // SAFETY: as in `open_at`.
        match unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

/// A file that may be read as a job file.
#[cfg(unix)]
fn is_own_file(regular: bool, links: u64, owner: u32, folder_owner: u32) -> bool {
    regular && links == 1 && owner == folder_owner
}

#[cfg(any(unix, windows))]
fn not_a_job_file() -> io::Error {
    io::Error::other("not a plain file of the job folder")
}

/// Windows: every file is opened relative to the held folder (`NtCreateFile` with `RootDirectory`), as `openat`
/// does, so the folder's path is never followed again. `OBJ_DONT_REPARSE` makes an open that would have to go
/// through a reparse point fail (the held folder turned into a junction while it was still empty, say), and
/// `FILE_OPEN_REPARSE_POINT` opens a link in a file's place as the link itself.
#[cfg(windows)]
impl JobFolder {
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE`: others may read and write, nobody may rename or delete.
    const SHARE_NO_DELETE: u32 = 0x1 | 0x2;
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;

    /// An error when `path` is not a real folder (a link or a junction to one included).
    pub fn open(path: &Path) -> io::Result<Self> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        let pin = OpenOptions::new()
            .read(true)
            .share_mode(Self::SHARE_NO_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let attributes = pin.metadata()?.file_attributes();
        if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("not a folder"));
        }
        Ok(JobFolder { pin })
    }

    /// Opens or creates the file `name` (a single name, never a path) inside the held folder.
    fn open_at(&self, name: &str, access: u32, share: u32, disposition: u32) -> io::Result<File> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
        use windows_sys::Wdk::Storage::FileSystem::{
            FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
        };
        use windows_sys::Win32::Foundation::{
            OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, RtlNtStatusToDosError, UNICODE_STRING,
        };
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, SYNCHRONIZE};
        use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        let bytes = (wide.len() * 2) as u16;
        let object_name = UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: wide.as_mut_ptr() };
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: self.pin.as_raw_handle() as _,
            ObjectName: &object_name,
            Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
            SecurityDescriptor: std::ptr::null(),
            SecurityQualityOfService: std::ptr::null(),
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: plain data that the call fills in.
        let mut status_block: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        // SAFETY: the folder's handle is open; the name and the attributes outlive the call.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                access | SYNCHRONIZE,
                &attributes,
                &mut status_block,
                std::ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                share,
                disposition,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                std::ptr::null(),
                0,
            )
        };
        if status < 0 {
            // SAFETY: only translates the code.
            return Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32));
        }
        // SAFETY: a new handle that nothing else owns.
        Ok(unsafe { File::from_raw_handle(handle as _) })
    }

    fn create_shared(&self, name: &str, share: u32) -> io::Result<File> {
        use windows_sys::Wdk::Storage::FileSystem::FILE_CREATE;
        use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;
        self.open_at(name, FILE_GENERIC_WRITE, share, FILE_CREATE)
    }

    /// An existing entry of the folder (a link itself, never its target).
    fn open_existing(&self, name: &str, access: u32) -> io::Result<File> {
        use windows_sys::Wdk::Storage::FileSystem::FILE_OPEN;
        self.open_at(name, access, Self::SHARE_ALL, FILE_OPEN)
    }

    /// A new file; an existing file or a link in its place is an error.
    fn create_new(&self, name: &str) -> io::Result<File> {
        self.create_shared(name, Self::SHARE_ALL)
    }

    /// The event log. Nobody can delete or rename it while it is open.
    fn create_log(&self) -> io::Result<File> {
        self.create_shared("log.jsonl", Self::SHARE_NO_DELETE)
    }

    /// The content of a file of the folder: not a link, and with this one name (no hard link to another file).
    fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_READ,
            GetFileInformationByHandle,
        };
        let mut file = self.open_existing(name, FILE_GENERIC_READ)?;
        // SAFETY: plain data that the call fills in.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is open.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let other = FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT;
        if info.dwFileAttributes & other != 0 || info.nNumberOfLinks != 1 {
            return Err(not_a_job_file());
        }
        let mut data = Vec::new();
        io::Read::read_to_end(&mut file, &mut data)?;
        Ok(data)
    }

    fn exists(&self, name: &str) -> bool {
        use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
        self.open_existing(name, FILE_READ_ATTRIBUTES).is_ok()
    }

    /// Removes the entry (a link itself, never its target).
    fn remove(&self, name: &str) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        };
        let file = self.open_existing(name, DELETE)?;
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: the handle is open and the information has the size given. The file goes when it is closed.
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as _,
                FileDispositionInfo,
                (&raw const info).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// Renames `from` to `to` inside the folder, replacing `to`. A simple rename (no root, a bare name) stays in
    /// the folder the file is in: no path is resolved.
    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Wdk::Storage::FileSystem::{FileRenameInformation, NtSetInformationFile};
        use windows_sys::Win32::Foundation::{HANDLE, RtlNtStatusToDosError};
        use windows_sys::Win32::Storage::FileSystem::DELETE;
        use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
        /// `FILE_RENAME_INFORMATION` with room for the name.
        #[repr(C)]
        struct Rename {
            /// `ReplaceIfExists` is the first byte.
            replace: u32,
            root: HANDLE,
            length: u32,
            name: [u16; 64],
        }
        let mut info = Rename { replace: 1, root: std::ptr::null_mut(), length: 0, name: [0; 64] };
        let wide: Vec<u16> = to.encode_utf16().collect();
        info.name.get_mut(..wide.len()).ok_or_else(|| io::Error::other("name too long"))?.copy_from_slice(&wide);
        info.length = (wide.len() * 2) as u32;
        let file = self.open_existing(from, DELETE)?;
        // SAFETY: plain data that the call fills in.
        let mut status_block: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is open and the information has the size given.
        let status = unsafe {
            NtSetInformationFile(
                file.as_raw_handle() as _,
                &mut status_block,
                (&raw const info).cast(),
                std::mem::size_of::<Rename>() as u32,
                FileRenameInformation,
            )
        };
        if status < 0 {
            // SAFETY: only translates the code.
            return Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32));
        }
        Ok(())
    }
}

/// Other systems have no elevated helper that works on a drive: plain paths.
#[cfg(not(any(unix, windows)))]
impl JobFolder {
    /// An error when `path` is not a real folder.
    pub fn open(path: &Path) -> io::Result<Self> {
        if !std::fs::symlink_metadata(path)?.is_dir() {
            return Err(io::Error::other("not a folder"));
        }
        Ok(JobFolder { path: path.to_path_buf() })
    }

    /// A new file; an existing file or a link in its place is an error.
    fn create_new(&self, name: &str) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).open(self.path.join(name))
    }

    fn create_log(&self) -> io::Result<File> {
        self.create_new("log.jsonl")
    }

    fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.path.join(name))
    }

    fn exists(&self, name: &str) -> bool {
        self.path.join(name).exists()
    }

    /// Removes the entry (a link itself, never its target).
    fn remove(&self, name: &str) -> io::Result<()> {
        std::fs::remove_file(self.path.join(name))
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        std::fs::rename(self.path.join(from), self.path.join(to))
    }
}

impl JobFolder {
    /// Replaces the file `name` with the JSON: a new temporary file (an old one is removed, which removes a link and
    /// not its target), then a rename.
    fn write_json(&self, name: &str, v: &Value) {
        let tmp = Path::new(name).with_extension("tmp").to_string_lossy().into_owned();
        let _ = self.remove(&tmp);
        let written = self.create_new(&tmp).and_then(|mut f| f.write_all(v.to_string().as_bytes()));
        if written.is_ok() {
            let _ = self.rename(&tmp, name);
        }
    }
}

/// `log.jsonl` of the job folder: one JSON line per stage or failure, flushed line by line, so the lines are there
/// even when the helper dies.
pub struct EventLog {
    file: File,
}

impl EventLog {
    pub fn create(folder: &JobFolder) -> io::Result<Self> {
        Ok(EventLog { file: folder.create_log()? })
    }

    pub fn event(&mut self, stage: &str, ok: bool, info: Value) {
        let t_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let line = format!("{}\n", json!({"t_ms": t_ms, "stage": stage, "ok": ok, "info": info}));
        // the log is only for the report: a line that cannot be written changes nothing in the job
        let _ = self.file.write_all(line.as_bytes()).and_then(|()| self.file.flush());
    }
}

/// Why a job failed, as it goes into `result.json`.
#[derive(Debug)]
struct JobError {
    message: String,
    /// `privacy`, `busy`, `readOnly`, `gone`, `denied`, `unresponsive`, `tooSmall`, `cancelled` or `other`.
    cause: String,
    /// Quick check: counts, restore check, cache flush, layout check and the access error.
    details: Value,
}

impl JobError {
    fn new(cause: impl Into<String>, message: impl Into<String>) -> Self {
        JobError { message: message.into(), cause: cause.into(), details: Value::Null }
    }

    fn to_json(&self) -> Value {
        json!({"ok": false, "error": self.message, "cause": self.cause, "details": self.details})
    }
}

impl From<String> for JobError {
    fn from(message: String) -> Self {
        JobError::new("other", message)
    }
}

/// `"privacy"`, `"busy"`, … as the cause goes into the result.
fn cause_name(cause: dio::AccessCause) -> String {
    serde_json::to_value(cause).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "denied".into())
}

/// Entry of `--helper <folder>`; returns the process exit code.
pub fn main(dir: &Path) -> i32 {
    // a job folder that is not a real folder (a link, say) gets nothing written into it
    let Ok(folder) = JobFolder::open(dir) else {
        eprintln!("the job folder is not safe");
        return 1;
    };
    disks::set_elevated();
    let result = match EventLog::create(&folder) {
        Ok(mut log) => folder
            .read("job.json")
            .map_err(|e| format!("could not read the job: {e}"))
            .and_then(|d| serde_json::from_slice::<Job>(&d).map_err(|e| format!("bad job: {e}")))
            .map_err(JobError::from)
            .and_then(|job| run_job(&job, &folder, &mut log)),
        // on Windows a folder the log cannot be created in gets nothing more written into it
        Err(e) if cfg!(windows) => {
            eprintln!("the job folder is not safe: {e}");
            return 1;
        }
        Err(e) => Err(JobError::new("other", format!("the job folder is not safe: {e}"))),
    };
    let ok = result.is_ok();
    let v = match result {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(e) => e.to_json(),
    };
    folder.write_json("result.json", &v);
    if ok { 0 } else { 1 }
}

/// The drive found again must have the partition layout the window saw (a drive swapped while the password window
/// was open has another one).
fn check_layout(job: &Job, disk: &Disk) -> Result<(), String> {
    match &job.layout {
        Some(layout) if &Fingerprint::of(disk) != layout => {
            Err("the drive changed since you chose it (its partitions are not the same); choose it again".into())
        }
        _ => Ok(()),
    }
}

/// What the log says about the drive as it was found: counts, offsets, sizes and file systems only, never a label
/// or a mount path.
fn found_info(disk: &Disk) -> Value {
    json!({"volumesMounted": disk.volumes.len(), "partitions": report::partitions_of(disk)})
}

fn run_job(job: &Job, folder: &JobFolder, log: &mut EventLog) -> Result<Value, JobError> {
    if !disks::valid_id(&job.disk) {
        return Err(format!("bad drive id {:?}", job.disk).into());
    }
    let disk = match disks::find(&job.disk, job.size) {
        Ok(disk) => disk,
        Err(e) => {
            log.event("found", false, json!({}));
            return Err(e.into());
        }
    };
    let checked = check_layout(job, &disk);
    log.event("found", checked.is_ok(), found_info(&disk));
    checked?;
    if job.action == Action::Fix && disk.table_unreadable {
        return Err("the partition table of the drive could not be read; look at the drive again".to_string().into());
    }
    let cancel = AtomicBool::new(false);
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut progress = |fraction: f64, stage: &'static str| {
        if last.elapsed() >= Duration::from_millis(200) || fraction >= 1.0 {
            last = Instant::now();
            if folder.exists("cancel") {
                cancel.store(true, Ordering::Relaxed);
            }
            folder.write_json("progress.json", &json!({"fraction": fraction, "stage": stage}));
        }
    };
    match job.action {
        Action::Probe => {
            let expected = expected_layout(job, &disk);
            run_probe(&disk, expected.as_ref(), &cancel, &mut progress, log)
        }
        Action::Fix => {
            progress(0.05, "prFixing");
            let out = run_fix(&disk, job, log)?;
            progress(1.0, "prFixing");
            Ok(json!({"log": out}))
        }
    }
}

/// How the partition layout looks after the check, compared with the one from before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayoutAfter {
    Same,
    Changed,
    /// There were partitions and now there are none.
    TableGone,
    /// The drive is not connected any more (or another one took its place).
    DriveGone,
    /// The list of drives did not answer, or the partition table could not be read.
    Unreadable,
    /// The drive is there, but its table could not be read before the check: there is nothing to compare with. The
    /// layout neither passes nor fails; the read-back check alone says whether the result is trusted.
    NotCompared,
}

impl LayoutAfter {
    /// Nothing to wait for any more.
    fn settled(self) -> bool {
        matches!(self, LayoutAfter::Same | LayoutAfter::NotCompared)
    }
}

/// The layout the drive should have after the check: the job's, else the one found at the start. None: the table
/// could not be read at the start, so there is nothing to compare with.
fn expected_layout(job: &Job, disk: &Disk) -> Option<Fingerprint> {
    (!disk.table_unreadable).then(|| job.layout.clone().unwrap_or_else(|| Fingerprint::of(disk)))
}

pub fn layout_after(expected: Option<&Fingerprint>, found: Result<Disk, String>) -> LayoutAfter {
    let found = match found {
        Ok(found) => found,
        // the messages of `disks::find`
        Err(e) if e.contains("is not connected") || e.contains("changed (size") => return LayoutAfter::DriveGone,
        Err(_) => return LayoutAfter::Unreadable,
    };
    match expected {
        None => LayoutAfter::NotCompared,
        Some(_) if found.table_unreadable => LayoutAfter::Unreadable,
        Some(expected) if &Fingerprint::of(&found) == expected => LayoutAfter::Same,
        Some(expected) if found.partitions.is_empty() && !expected.partitions.is_empty() => LayoutAfter::TableGone,
        Some(_) => LayoutAfter::Changed,
    }
}

/// Looks the drive up once a second for up to 10 s, until it lists with the expected layout again (or just lists,
/// when there is nothing to compare with).
#[cfg(any(target_os = "macos", windows))]
fn check_layout_after(disk: &Disk, expected: Option<&Fingerprint>, log: &mut EventLog) -> LayoutAfter {
    let start = Instant::now();
    loop {
        let found = disks::find(&disk.id, disk.size);
        let seen = found.as_ref().ok().map(report::layout_summary);
        let after = layout_after(expected, found);
        if after.settled() || start.elapsed() >= Duration::from_secs(10) {
            log.event("layout", after.settled(), json!({"result": after, "layout": seen}));
            return after;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn counts(stats: &probe::Stats) -> Value {
    json!({"mismatches": stats.mismatches, "ioErrors": stats.io_errors, "unsaved": stats.unsaved,
        "restoreFailures": stats.restore_failures})
}

/// The result of a probe run with the layout check that followed it.
fn probe_result(run: probe::ProbeRun, layout: LayoutAfter) -> Result<Value, JobError> {
    let mut details = json!({"counts": counts(&run.stats), "restoreCheck": run.restore_check, "flush": run.flush,
        "layoutAfter": layout, "wrote": run.wrote});
    let (cause, message) = match run.result {
        Ok(o) => {
            let mut v = serde_json::to_value(o).map_err(|e| JobError::from(e.to_string()))?;
            v["layoutAfter"] = json!(layout);
            return Ok(v);
        }
        Err(probe::Abort::Cancelled) => ("cancelled".to_string(), "cancelled".to_string()),
        Err(probe::Abort::TooSmall) => ("tooSmall".into(), "the drive is too small for the quick probe".into()),
        Err(probe::Abort::Unresponsive) => ("unresponsive".into(), "the drive stopped answering".into()),
        Err(probe::Abort::Access(a)) => {
            details["access"] = json!({"cause": a.cause, "op": a.op, "errno": a.errno});
            (cause_name(a.cause), a.text)
        }
    };
    Err(JobError { message, cause, details })
}

/// A failed `open` of the raw device: an access error like any other (so `EPERM` is the privacy protection).
fn open_error(device: &str, e: &io::Error) -> JobError {
    let message = format!("could not open {device}: {e}");
    match probe::AccessFailure::new(e, probe::Op::Open) {
        Some(a) => JobError {
            message,
            cause: cause_name(a.cause),
            details: json!({"access": {"cause": a.cause, "op": a.op, "errno": a.errno}, "wrote": false}),
        },
        None => JobError {
            message,
            cause: "other".into(),
            details: json!({"access": {"op": probe::Op::Open, "errno": e.raw_os_error()}, "wrote": false}),
        },
    }
}

/// Opens the raw device, runs the probe and closes the device again.
#[cfg(any(target_os = "macos", windows))]
fn probe_device(
    disk: &Disk,
    cancel: &AtomicBool,
    progress: &mut probe::Progress,
    log: &mut EventLog,
) -> Result<probe::ProbeRun, JobError> {
    let mut dev = match crate::rawdev::RawDevice::open(Path::new(&disk.device)) {
        Ok(dev) => dev,
        Err(e) => {
            let error = open_error(&disk.device, &e);
            log.event("open", false, error.details["access"].clone());
            return Err(error);
        }
    };
    log.event("open", true, json!({}));
    let run = probe::probe(&mut dev, probe::Params::default(), cancel, progress, &mut |stage, ok, info| {
        log.event(stage, ok, info)
    });
    drop(dev);
    log.event("closed", true, json!({}));
    log.event(
        "summary",
        run.result.is_ok(),
        json!({"counts": counts(&run.stats), "grid": run.stats.grid,
            "failuresNotListed": run.stats.failures_not_listed}),
    );
    Ok(run)
}

/// The layout check after the probe (also when the device did not even open), and the result.
#[cfg(any(target_os = "macos", windows))]
fn after_probe(
    disk: &Disk,
    expected: Option<&Fingerprint>,
    probed: Result<probe::ProbeRun, JobError>,
    log: &mut EventLog,
) -> Result<Value, JobError> {
    let layout = check_layout_after(disk, expected, log);
    match probed {
        Ok(run) => probe_result(run, layout),
        Err(mut e) => {
            e.details["layoutAfter"] = json!(layout);
            Err(e)
        }
    }
}

/// What a system tool did: its exit status (None: it did not run or did not finish) and its output.
struct Tool {
    ok: bool,
    exit: Option<i32>,
    text: String,
}

impl Tool {
    fn from_output(out: io::Result<std::process::Output>) -> Tool {
        match out {
            Ok(out) => Tool {
                ok: out.status.success(),
                exit: out.status.code(),
                text: format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
            },
            Err(e) => Tool { ok: false, exit: None, text: format!("could not run the tool: {e}") },
        }
    }

    /// The event of this tool: the exit status and the output without names and paths, shortened.
    fn info(&self, disk: &Disk) -> Value {
        let home = crate::net::home_dir();
        json!({"exit": self.exit, "output": report::free_text(&self.text, disk, &home)})
    }
}

#[cfg(target_os = "macos")]
fn diskutil_tool(args: &[&str]) -> Tool {
    Tool::from_output(crate::net::command("/usr/sbin/diskutil").args(args).output())
}

#[cfg(target_os = "macos")]
fn diskutil(args: &[&str]) -> Result<String, String> {
    let t = diskutil_tool(args);
    if t.ok { Ok(t.text) } else { Err(format!("diskutil {}: {}", args.join(" "), t.text.trim())) }
}

#[cfg(target_os = "macos")]
fn run_probe(
    disk: &Disk,
    expected: Option<&Fingerprint>,
    cancel: &AtomicBool,
    progress: &mut probe::Progress,
    log: &mut EventLog,
) -> Result<Value, JobError> {
    progress(0.0, "prPrepare");
    let was_mounted = !disk.volumes.is_empty();
    if was_mounted {
        let unmounted = diskutil(&["unmountDisk", &disk.id]);
        log.event("unmounted", unmounted.is_ok(), json!({"volumes": disk.volumes.len()}));
        unmounted.map_err(|e| format!("could not unmount the drive (close its files): {e}"))?;
    }
    let probed = probe_device(disk, cancel, progress, log);
    if was_mounted {
        // a drive that stopped answering must not hang the helper here
        let t = Tool::from_output(crate::net::output_timeout(
            crate::net::command("/usr/sbin/diskutil").args(["mountDisk", &disk.id]),
            Duration::from_secs(60),
        ));
        log.event("remounted", t.ok, t.info(disk));
    }
    after_probe(disk, expected, probed, log)
}

#[cfg(windows)]
fn run_probe(
    disk: &Disk,
    expected: Option<&Fingerprint>,
    cancel: &AtomicBool,
    progress: &mut probe::Progress,
    log: &mut EventLog,
) -> Result<Value, JobError> {
    progress(0.0, "prPrepare");
    let letters: Vec<String> = disk.volumes.iter().map(|v| v.mount.trim_end_matches(['\\', ':']).to_string()).collect();
    let locks = crate::rawdev::VolumeLocks::take(&letters);
    log.event("unmounted", locks.is_ok(), json!({"volumes": letters.len()}));
    let locks = locks?;
    let probed = probe_device(disk, cancel, progress, log);
    drop(locks);
    log.event("remounted", true, json!({"volumes": letters.len()}));
    after_probe(disk, expected, probed, log)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn run_probe(
    _: &Disk,
    _: Option<&Fingerprint>,
    _: &AtomicBool,
    _: &mut probe::Progress,
    _: &mut EventLog,
) -> Result<Value, JobError> {
    Err("not supported on this system".to_string().into())
}

/// diskutil arguments for the repair (tested without running them).
pub fn mac_fix_args(disk: &str, job: &Job, size: u64) -> Vec<String> {
    let fs = match job.fs {
        Fs::Exfat => "ExFAT",
        Fs::Fat32 => "FAT32",
    };
    let label = clean_label(&job.label);
    if size >= job.size {
        ["eraseDisk", fs, &label, "MBR", disk].map(String::from).to_vec()
    } else {
        // an exact size for the partition and the rest left as free space, so nothing is ever written there
        ["partitionDisk", disk, "2", "MBR", fs, &label, &size.to_string(), "FREE", "gap", "R"]
            .map(String::from)
            .to_vec()
    }
}

/// diskpart script for the repair (tested without running it).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn diskpart_script(number: &str, job: &Job, size: u64) -> String {
    let fs = match job.fs {
        Fs::Exfat => "exfat",
        Fs::Fat32 => "fat32",
    };
    let create = if size >= job.size {
        "create partition primary".to_string()
    } else {
        format!("create partition primary size={}", size / (1 << 20))
    };
    format!(
        "select disk {number}\r\nattributes disk clear readonly\r\nclean\r\nconvert mbr\r\n{create}\r\n\
         format fs={fs} quick label=\"{}\"\r\nassign\r\n",
        clean_label(&job.label)
    )
}

/// Everything the repair needs to know before it touches the drive: the partition size and the macOS
/// `diskutil` arguments. Errors here mean nothing has been unmounted, cleaned or partitioned.
pub fn plan_fix(disk_id: &str, job: &Job) -> Result<(u64, Vec<String>), String> {
    let size = fix_size(job)?;
    if size >= job.size && !job.whole_confirmed {
        return Err("a format at the full size of the drive needs an explicit confirmation".into());
    }
    if job.fs == Fs::Fat32 && size > FAT32_MAX {
        return Err("FAT32 is limited to 32 GB here; choose exFAT".into());
    }
    Ok((size, mac_fix_args(disk_id, job, size)))
}

fn run_fix(disk: &Disk, job: &Job, log: &mut EventLog) -> Result<String, String> {
    let (size, args) = plan_fix(&disk.id, job)?;
    // the tool's output names the new volume too: its label and mount path stay out of the log
    let named = report::with_new_label(disk, &clean_label(&job.label));
    #[cfg(target_os = "macos")]
    {
        let _ = size;
        let unmounted = diskutil(&["unmountDisk", "force", &disk.id]);
        log.event("unmounted", unmounted.is_ok(), json!({"volumes": disk.volumes.len()}));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let t = diskutil_tool(&args);
        log.event("tool", t.ok, t.info(&named));
        if t.ok { Ok(t.text) } else { Err(format!("diskutil {}: {}", args.join(" "), t.text.trim())) }
    }
    #[cfg(windows)]
    {
        let _ = args;
        let script = std::env::temp_dir().join(format!("UsbMemoryChecker-fix-{}.txt", std::process::id()));
        std::fs::write(&script, diskpart_script(&disk.id, job, size)).map_err(|e| e.to_string())?;
        let out = crate::net::command("diskpart.exe").arg("/s").arg(&script).output();
        let _ = std::fs::remove_file(&script);
        let t = Tool::from_output(out);
        log.event("tool", t.ok, t.info(&named));
        if t.ok { Ok(t.text) } else { Err(format!("diskpart failed: {}", t.text.trim())) }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (disk, job, size, args, named, log);
        Err("not supported on this system".into())
    }
}

// ------------------------------------------------------------------ the window side

#[derive(Debug)]
pub enum RunError {
    /// The user closed the password / UAC window.
    Declined,
    /// `cause` and `details` come from the helper's `result.json`; both are empty when the helper gave none.
    Failed { message: String, cause: Option<String>, details: Value },
}

impl RunError {
    pub fn failed(message: impl Into<String>) -> Self {
        RunError::Failed { message: message.into(), cause: None, details: Value::Null }
    }
}

/// A finished run: the result and the lines of the helper's event log.
pub struct Ran {
    pub result: Result<Value, RunError>,
    pub log: Vec<String>,
}

fn job_dir() -> PathBuf {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    std::env::temp_dir().join(format!("UsbMemoryChecker-job-{}-{t}", std::process::id()))
}

/// A new folder that only its owner can read and write; an existing folder or a link in its place is an error.
fn create_job_dir(dir: &Path) -> io::Result<()> {
    #[allow(unused_mut)]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// The lines of the helper's event log (empty when it never started).
pub(crate) fn read_log(dir: &Path) -> Vec<String> {
    std::fs::read(dir.join("log.jsonl"))
        .map(|d| String::from_utf8_lossy(&d).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The content of `result.json`.
pub(crate) fn parse_result(data: &[u8]) -> Result<Value, RunError> {
    let v: Value = serde_json::from_slice(data).map_err(|e| RunError::failed(e.to_string()))?;
    if v["ok"].as_bool() == Some(true) {
        Ok(v["result"].clone())
    } else {
        Err(RunError::Failed {
            message: v["error"].as_str().unwrap_or("unknown error").to_string(),
            cause: v["cause"].as_str().map(str::to_string),
            details: v["details"].clone(),
        })
    }
}

/// Runs the job elevated and waits for it. `on_progress` gets (fraction, stage key).
/// `UMC_NO_ELEVATE=1` runs the helper with the current rights (for testing on a RAM disk).
pub fn run(job: &Job, prompt: &str, cancel: &AtomicBool, on_progress: &mut dyn FnMut(f64, String)) -> Ran {
    let dir = job_dir();
    if let Err(e) = create_job_dir(&dir) {
        return Ran { result: Err(RunError::failed(e.to_string())), log: Vec::new() };
    }
    let result = run_in(&dir, job, prompt, cancel, on_progress);
    // the log is read before the folder goes, also when the helper stopped without a result
    let log = read_log(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    Ran { result, log }
}

fn run_in(
    dir: &Path,
    job: &Job,
    prompt: &str,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f64, String),
) -> Result<Value, RunError> {
    let fail = |e: String| RunError::failed(e);
    std::fs::write(dir.join("job.json"), serde_json::to_vec(job).map_err(|e| fail(e.to_string()))?)
        .map_err(|e| fail(e.to_string()))?;
    let exe = std::env::current_exe().map_err(|e| fail(e.to_string()))?;
    let mut process = Elevated::start(&exe, dir, prompt)?;
    let mut cancel_sent = false;
    loop {
        if let Ok(d) = std::fs::read(dir.join("progress.json"))
            && let Ok(v) = serde_json::from_slice::<Value>(&d)
        {
            on_progress(v["fraction"].as_f64().unwrap_or(0.0), v["stage"].as_str().unwrap_or("prPrepare").to_string());
        }
        if cancel.load(Ordering::Relaxed) && !cancel_sent {
            let _ = std::fs::write(dir.join("cancel"), b"");
            cancel_sent = true;
        }
        if let Some(outcome) = process.finished() {
            outcome?;
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let data =
        std::fs::read(dir.join("result.json")).map_err(|_| fail("the helper stopped without a result".into()))?;
    parse_result(&data)
}

/// `'…'` for sh.
#[cfg_attr(windows, allow(dead_code))]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `"…"` for AppleScript.
#[cfg_attr(windows, allow(dead_code))]
fn as_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

#[cfg_attr(windows, allow(dead_code))]
pub fn apple_script(exe: &Path, dir: &Path, prompt: &str) -> String {
    let cmd = format!("{} --helper {}", sh_quote(&exe.to_string_lossy()), sh_quote(&dir.to_string_lossy()));
    format!("do shell script {} with prompt {} with administrator privileges", as_quote(&cmd), as_quote(prompt))
}

enum Elevated {
    Child(std::process::Child),
    #[cfg(windows)]
    Handle(windows_sys::Win32::Foundation::HANDLE),
}

impl Elevated {
    fn start(exe: &Path, dir: &Path, prompt: &str) -> Result<Self, RunError> {
        if std::env::var("UMC_NO_ELEVATE").is_ok_and(|v| v == "1") {
            let child = crate::net::command(&exe.to_string_lossy())
                .arg("--helper")
                .arg(dir)
                .spawn()
                .map_err(|e| RunError::failed(e.to_string()))?;
            return Ok(Elevated::Child(child));
        }
        #[cfg(windows)]
        {
            let _ = prompt;
            start_windows(exe, dir)
        }
        #[cfg(not(windows))]
        {
            let child = crate::net::command("/usr/bin/osascript")
                .arg("-e")
                .arg(apple_script(exe, dir, prompt))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| RunError::failed(format!("could not run osascript: {e}")))?;
            Ok(Elevated::Child(child))
        }
    }

    /// Some(..) once the process has exited.
    fn finished(&mut self) -> Option<Result<(), RunError>> {
        match self {
            Elevated::Child(child) => match child.try_wait() {
                Ok(None) => None,
                Ok(Some(status)) => {
                    let mut err = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut pipe, &mut err);
                    }
                    // osascript: error -128 — the user cancelled the password window
                    if !status.success() && err.contains("-128") {
                        return Some(Err(RunError::Declined));
                    }
                    Some(Ok(()))
                }
                Err(e) => Some(Err(RunError::failed(e.to_string()))),
            },
            #[cfg(windows)]
            Elevated::Handle(h) => {
                use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
                use windows_sys::Win32::System::Threading::WaitForSingleObject;
                // SAFETY: h is the process handle from ShellExecuteExW, closed only here.
                unsafe {
                    if WaitForSingleObject(*h, 0) == WAIT_TIMEOUT {
                        return None;
                    }
                    CloseHandle(*h);
                }
                Some(Ok(()))
            }
        }
    }
}

#[cfg(windows)]
fn start_windows(exe: &Path, dir: &Path) -> Result<Elevated, RunError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    let params = wide(format!("--helper \"{}\"", dir.display()).as_ref());
    // SAFETY: the struct is plain data; all strings outlive the call.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE as _;
    // SAFETY: info is filled in above.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let e = std::io::Error::last_os_error();
        // ERROR_CANCELLED: the user said No in the UAC window
        return Err(if e.raw_os_error() == Some(1223) { RunError::Declined } else { RunError::failed(e.to_string()) });
    }
    if info.hProcess.is_null() {
        return Err(RunError::failed("the elevated helper did not start"));
    }
    Ok(Elevated::Handle(info.hProcess))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(real: u64, fs: Fs) -> Job {
        Job {
            action: Action::Fix,
            disk: "disk4".into(),
            size: 2_000_398_934_016,
            real,
            partition: 0,
            layout: None,
            whole_confirmed: true,
            fs,
            label: "my usb!".into(),
        }
    }

    #[test]
    fn labels() {
        assert_eq!(clean_label("my usb!"), "MYUSB");
        assert_eq!(clean_label("флешка"), "USB");
        assert_eq!(clean_label("a_very-long-label"), "A_VERY-LONG");
    }

    #[test]
    fn partition_sizes() {
        assert_eq!(partition_size(0, 100 << 30), 100 << 30);
        assert_eq!(partition_size(100 << 30, 100 << 30), 100 << 30);
        assert_eq!(partition_size(8 << 30, 100 << 30), (8 << 30) - (2 << 20));
        assert_eq!(partition_size((8 << 30) + 12345, 100 << 30) % (1 << 20), 0);
    }

    #[test]
    fn mac_commands() {
        let j = job(8 << 30, Fs::Exfat);
        let a = mac_fix_args("disk4", &j, fix_size(&j).unwrap());
        assert_eq!(a, ["partitionDisk", "disk4", "2", "MBR", "ExFAT", "MYUSB", "8587837440", "FREE", "gap", "R"]);
        let j = job(0, Fs::Fat32);
        let full = mac_fix_args("disk4", &j, fix_size(&j).unwrap());
        assert_eq!(full, ["eraseDisk", "FAT32", "MYUSB", "MBR", "disk4"]);
    }

    #[test]
    fn diskpart_commands() {
        let j = job(8 << 30, Fs::Exfat);
        let s = diskpart_script("3", &j, fix_size(&j).unwrap());
        assert!(s.starts_with("select disk 3\r\n"));
        assert!(s.contains("create partition primary size=8190\r\n"));
        assert!(s.contains("format fs=exfat quick label=\"MYUSB\""));
        let j = job(0, Fs::Fat32);
        assert!(
            diskpart_script("3", &j, fix_size(&j).unwrap()).contains("create partition primary\r\nformat fs=fat32")
        );
    }

    #[test]
    fn applescript_quoting() {
        let s = apple_script(Path::new("/Applications/My \"Tool\".app/x"), Path::new("/tmp/it's"), "Need rights");
        assert_eq!(
            s,
            r#"do shell script "'/Applications/My \"Tool\".app/x' --helper '/tmp/it'\\''s'" with prompt "Need rights" with administrator privileges"#
        );
    }

    #[test]
    fn job_json_round_trip() {
        let j = job(5, Fs::Fat32);
        let back: Job = serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert_eq!(back, j);
        let minimal: Job = serde_json::from_str(r#"{"action":"probe","disk":"disk2","size":10}"#).unwrap();
        assert_eq!((minimal.fs, minimal.real, minimal.partition), (Fs::Exfat, 0, 0));
    }

    #[test]
    fn fix_sizes() {
        let size = 125_829_120_000;
        let with = |real, partition| Job { size, partition, ..job(real, Fs::Exfat) };
        assert_eq!(fix_size(&with(0, 0)), Ok(size));
        assert_eq!(fix_size(&with(8 << 30, 0)), Ok((8 << 30) - (2 << 20)));
        assert!(fix_size(&with(1 << 20, 0)).is_err());
        assert!(fix_size(&with((2 << 20) + 5, 0)).is_err());
        assert_eq!(fix_size(&with(0, 8_303_673_344)), Ok(8_303_673_344));
        assert!(fix_size(&with(0, 8_303_673_345)).is_err());
        assert!(fix_size(&with(0, size + (1 << 20))).is_err());
        assert!(fix_size(&with(0, 1000)).is_err());
        assert_eq!(fix_size(&with(0, size)), Ok(size));
    }

    #[test]
    fn exact_partition_commands() {
        let j = Job { size: 125_829_120_000, partition: 8_303_673_344, ..job(0, Fs::Exfat) };
        let size = fix_size(&j).unwrap();
        let a = mac_fix_args("disk6", &j, size);
        assert_eq!(a[a.len() - 4..], ["8303673344", "FREE", "gap", "R"]);
        assert!(diskpart_script("3", &j, size).contains("create partition primary size=7919\r\n"));
    }

    #[test]
    fn run_fix_refuses_size_zero_before_touching_the_drive() {
        let j = Job { size: 125_829_120_000, ..job(1 << 20, Fs::Exfat) };
        assert!(plan_fix("disk6", &j).is_err());
        let fat = Job { size: 125_829_120_000, partition: 40_000_000_000 / MIB * MIB, ..job(0, Fs::Fat32) };
        assert!(plan_fix("disk6", &fat).is_err());
    }

    #[test]
    fn a_swapped_drive_is_refused_before_anything_is_touched() {
        let disk = |uuid: &str| Disk {
            id: "disk6".into(),
            device: String::new(),
            name: String::new(),
            kind: "usb".into(),
            bus: "USB".into(),
            size: 2_000_398_934_016,
            block_size: 512,
            volumes: Vec::new(),
            partitions: vec![disks::Partition { offset: 1 << 20, size: 1 << 30, id: uuid.into() }],
            table_unreadable: false,
            table_id: String::new(),
            partition_fs: Vec::new(),
        };
        let mut j = job(0, Fs::Exfat);
        assert!(check_layout(&j, &disk("A")).is_ok(), "no layout: no check (command line)");
        j.layout = Some(crate::capacity::Fingerprint::of(&disk("A")));
        assert!(check_layout(&j, &disk("A")).is_ok());
        assert!(check_layout(&j, &disk("B")).is_err());
        // the job survives the file round trip with its layout
        let back: Job = serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert_eq!(back, j);
    }

    #[test]
    fn a_full_size_format_needs_the_confirmation() {
        let size = 125_829_120_000;
        let whole = |confirmed| Job { size, whole_confirmed: confirmed, ..job(0, Fs::Exfat) };
        assert!(plan_fix("disk6", &whole(false)).is_err(), "eraseDisk / diskpart without the flag is refused");
        assert!(plan_fix("disk6", &whole(true)).is_ok());
        // an exact partition equal to the declared size is a full-size format too
        let exact = Job { partition: size, ..whole(false) };
        assert!(plan_fix("disk6", &exact).is_err());
        // a smaller partition needs no confirmation
        let part = Job { partition: 8_303_673_344, ..whole(false) };
        assert!(plan_fix("disk6", &part).is_ok());
        // old job files (no flag) are refused for a whole-drive format
        let old: Job = serde_json::from_str(r#"{"action":"fix","disk":"disk2","size":10}"#).unwrap();
        assert!(!old.whole_confirmed);
    }

    /// A temporary folder of one test, removed at its end.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("umc_helper_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn write_json_does_not_follow_a_link() {
        let t = Temp::new("link");
        let target = t.0.join("target.txt");
        std::fs::write(&target, "keep me").unwrap();
        std::os::unix::fs::symlink(&target, t.0.join("progress.tmp")).unwrap();
        JobFolder::open(&t.0).unwrap().write_json("progress.json", &json!({"fraction": 0.5}));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
        let written = t.0.join("progress.json");
        assert!(std::fs::symlink_metadata(&written).unwrap().is_file());
        assert_eq!(std::fs::read_to_string(written).unwrap(), json!({"fraction": 0.5}).to_string());
    }

    #[cfg(unix)]
    #[test]
    fn a_job_folder_swapped_for_a_link_later_gets_no_writes() {
        let t = Temp::new("swapped");
        let (job, moved, victim) = (t.0.join("job"), t.0.join("moved"), t.0.join("victim"));
        std::fs::create_dir(&job).unwrap();
        std::fs::write(job.join("job.json"), "mine").unwrap();
        std::fs::create_dir(&victim).unwrap();
        let names = ["job.json", "progress.json", "progress.tmp", "result.json", "result.tmp", "cancel"];
        for name in names {
            std::fs::write(victim.join(name), "keep me").unwrap();
        }
        let folder = JobFolder::open(&job).unwrap();
        // the user's process swaps the folder for a link after the helper has opened it
        std::fs::rename(&job, &moved).unwrap();
        std::os::unix::fs::symlink(&victim, &job).unwrap();
        let mut log = EventLog::create(&folder).unwrap();
        log.event("found", true, json!({}));
        folder.write_json("progress.json", &json!({"fraction": 0.5}));
        folder.write_json("result.json", &json!({"ok": true}));
        assert_eq!(folder.read("job.json").unwrap(), b"mine");
        assert!(!folder.exists("cancel"), "the other folder's file is not seen");
        // nothing in the link's target was created, removed or changed
        assert_eq!(std::fs::read_dir(&victim).unwrap().count(), names.len());
        for name in names {
            assert_eq!(std::fs::read_to_string(victim.join(name)).unwrap(), "keep me", "{name}");
        }
        // everything went into the folder that was opened
        assert_eq!(read_log(&moved).len(), 1);
        for name in ["progress.json", "result.json"] {
            assert!(moved.join(name).is_file(), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn helper_refuses_a_linked_job_folder() {
        let t = Temp::new("linked_dir");
        let real = t.0.join("real");
        std::fs::create_dir(&real).unwrap();
        let link = t.0.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(main(&link), 1);
        assert_eq!(std::fs::read_dir(&real).unwrap().count(), 0, "nothing was written");
        assert_eq!(main(&t.0.join("missing")), 1);
    }

    #[cfg(unix)]
    #[test]
    fn job_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new("private");
        let dir = t.0.join("job");
        create_job_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(create_job_dir(&dir).is_err(), "an existing folder is refused");
        let link = t.0.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(create_job_dir(&link).is_err(), "a link is refused");
    }

    #[test]
    fn job_folder_files_are_replaced_removed_and_found() {
        let t = Temp::new("files");
        let folder = JobFolder::open(&t.0).unwrap();
        let log = EventLog::create(&folder).unwrap();
        assert!(!folder.exists("cancel"));
        std::fs::write(t.0.join("cancel"), b"").unwrap();
        assert!(folder.exists("cancel"));
        for n in [1, 2] {
            folder.write_json("progress.json", &json!({"n": n}));
            assert_eq!(folder.read("progress.json").unwrap(), json!({"n": n}).to_string().as_bytes());
        }
        assert!(!t.0.join("progress.tmp").exists());
        folder.remove("cancel").unwrap();
        assert!(!t.0.join("cancel").exists());
        drop(log);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_hard_linked_job_file_is_not_read() {
        let t = Temp::new("hard_link");
        let job = t.0.join("job");
        std::fs::create_dir(&job).unwrap();
        // stands for a file only root may read, given a second name inside the job folder
        let secret = t.0.join("secret.txt");
        std::fs::write(&secret, "\"top secret\"").unwrap();
        std::fs::hard_link(&secret, job.join("job.json")).unwrap();
        assert!(JobFolder::open(&job).unwrap().read("job.json").is_err());
        assert_eq!(main(&job), 1);
        let result = std::fs::read_to_string(job.join("result.json")).unwrap();
        assert!(result.contains("could not read the job"), "{result}");
        assert!(!result.contains("top secret"), "{result}");
        // with one name again it is an ordinary file
        std::fs::remove_file(&secret).unwrap();
        assert_eq!(JobFolder::open(&job).unwrap().read("job.json").unwrap(), b"\"top secret\"");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_plain_file_of_the_folders_owner_is_a_job_file() {
        assert!(is_own_file(true, 1, 501, 501));
        assert!(!is_own_file(false, 1, 501, 501), "not a regular file");
        assert!(!is_own_file(true, 2, 501, 501), "a second name");
        assert!(!is_own_file(true, 0, 501, 501), "no name left");
        assert!(!is_own_file(true, 1, 0, 501), "another owner");
        // a folder in the file's place
        let t = Temp::new("not_a_file");
        std::fs::create_dir(t.0.join("job.json")).unwrap();
        assert!(JobFolder::open(&t.0).unwrap().read("job.json").is_err());
    }

    #[test]
    fn log_refuses_an_existing_file() {
        let t = Temp::new("log_exists");
        std::fs::write(t.0.join("log.jsonl"), "old").unwrap();
        assert!(EventLog::create(&JobFolder::open(&t.0).unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(t.0.join("log.jsonl")).unwrap(), "old");
        #[cfg(unix)]
        {
            let target = t.0.join("target.txt");
            std::fs::write(&target, "keep me").unwrap();
            std::fs::remove_file(t.0.join("log.jsonl")).unwrap();
            std::os::unix::fs::symlink(&target, t.0.join("log.jsonl")).unwrap();
            assert!(EventLog::create(&JobFolder::open(&t.0).unwrap()).is_err());
            // the whole job fails with an error, and the target of the link stays as it was
            assert_eq!(main(&t.0), 1);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
            let result = std::fs::read(t.0.join("result.json")).unwrap();
            let Err(RunError::Failed { message, .. }) = parse_result(&result) else { panic!("an error") };
            assert!(message.contains("the job folder is not safe"), "{message}");
        }
    }

    #[test]
    fn log_lines_are_json() {
        let t = Temp::new("log_lines");
        let mut log = EventLog::create(&JobFolder::open(&t.0).unwrap()).unwrap();
        log.event("found", true, json!({"volumesMounted": 1}));
        log.event("open", false, json!({"op": "open", "errno": 1}));
        log.event("layout", true, json!({"result": "same"}));
        drop(log);
        let lines = read_log(&t.0);
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let v: Value = serde_json::from_str(line).unwrap();
            assert!(v["t_ms"].as_u64().unwrap() > 0 && v["stage"].is_string() && v["ok"].is_boolean());
        }
    }

    fn run_of(result: Result<probe::Outcome, probe::Abort>) -> probe::ProbeRun {
        probe::ProbeRun {
            result,
            stats: probe::Stats { io_errors: 1882, mismatches: 3, unsaved: 5, ..Default::default() },
            restore_check: Default::default(),
            flush: probe::FlushResult::Unsupported,
            wrote: true,
        }
    }

    #[test]
    fn probe_result_causes() {
        let access = |code| {
            let e = io::Error::from_raw_os_error(code);
            probe::Abort::Access(probe::AccessFailure {
                cause: dio::AccessCause::Busy,
                op: probe::Op::Write,
                errno: Some(code),
                text: e.to_string(),
            })
        };
        let cases = [
            (probe::Abort::Cancelled, "cancelled"),
            (probe::Abort::TooSmall, "tooSmall"),
            (probe::Abort::Unresponsive, "unresponsive"),
            (access(16), "busy"),
        ];
        for (abort, cause) in cases {
            let e = probe_result(run_of(Err(abort)), LayoutAfter::TableGone).unwrap_err();
            let v = e.to_json();
            assert_eq!((v["ok"].as_bool(), v["cause"].as_str()), (Some(false), Some(cause)));
            assert_eq!(v["details"]["counts"]["ioErrors"], 1882);
            assert_eq!(v["details"]["counts"]["mismatches"], 3);
            assert_eq!(v["details"]["layoutAfter"], "tableGone");
            assert_eq!(v["details"]["flush"], "unsupported");
            assert_eq!(v["details"]["wrote"], true);
            assert_eq!(v["details"]["restoreCheck"]["mismatched"], 0);
        }
        for cause in ["privacy", "busy", "readOnly", "gone", "denied"] {
            let cause: dio::AccessCause = serde_json::from_value(json!(cause)).unwrap();
            assert_eq!(cause_name(cause), json!(cause).as_str().unwrap());
        }
        // an open() failure goes through the same classification
        #[cfg(target_os = "macos")]
        {
            let e = open_error("/dev/rdisk9", &io::Error::from_raw_os_error(libc::EPERM));
            assert_eq!(e.cause, "privacy");
            assert_eq!(
                (e.details["access"]["op"].as_str(), e.details["access"]["errno"].as_i64()),
                (Some("open"), Some(1))
            );
            assert_eq!(e.details["wrote"], false);
            assert_eq!(open_error("/dev/rdisk9", &io::Error::from_raw_os_error(libc::EBUSY)).cause, "busy");
            assert_eq!(open_error("/dev/rdisk9", &io::Error::from_raw_os_error(libc::ENOENT)).cause, "other");
        }
        let unwritten = probe::ProbeRun { wrote: false, ..run_of(Err(probe::Abort::Cancelled)) };
        assert_eq!(probe_result(unwritten, LayoutAfter::Changed).unwrap_err().details["wrote"], false);
        // a verdict carries the layout check
        let outcome: probe::Outcome = serde_json::from_value(json!({"verdict": "genuine", "reported": 10, "real": 10,
            "points": 1, "badPoints": 0, "seconds": 1.0, "restoreFailures": 0}))
        .unwrap();
        let v = probe_result(run_of(Ok(outcome)), LayoutAfter::Same).unwrap();
        assert_eq!(v["layoutAfter"], "same");
        assert!(v["restoreCheck"].is_object());
    }

    #[test]
    fn run_in_reads_cause_and_details() {
        let t = Temp::new("result");
        let path = t.0.join("result.json");
        JobFolder::open(&t.0).unwrap().write_json(
            "result.json",
            &json!({"ok": false, "error": "the drive stopped answering", "cause": "unresponsive",
                "details": {"counts": {"ioErrors": 1882}, "layoutAfter": "tableGone"}}),
        );
        let Err(RunError::Failed { message, cause, details }) = parse_result(&std::fs::read(&path).unwrap()) else {
            panic!("an error is expected");
        };
        assert_eq!(message, "the drive stopped answering");
        assert_eq!(cause.as_deref(), Some("unresponsive"));
        assert_eq!(details["counts"]["ioErrors"], 1882);
        assert_eq!(details["layoutAfter"], "tableGone");
        assert_eq!(parse_result(br#"{"ok":true,"result":{"x":1}}"#).unwrap(), json!({"x": 1}));
        assert!(matches!(parse_result(b"{"), Err(RunError::Failed { cause: None, .. })));
    }

    fn listed(partitions: Vec<disks::Partition>) -> Disk {
        Disk {
            id: "disk6".into(),
            device: "/dev/rdisk6".into(),
            name: "Storage Device".into(),
            kind: "usb".into(),
            bus: "USB".into(),
            size: 125_829_120_000,
            block_size: 512,
            volumes: Vec::new(),
            partitions,
            table_unreadable: false,
            table_id: String::new(),
            partition_fs: Vec::new(),
        }
    }

    #[test]
    fn layout_after_cases() {
        let one = disks::Partition { offset: MIB, size: 8_303_673_344, id: "U".into() };
        let expected = Fingerprint::of(&listed(vec![one.clone()]));
        let after = |found| layout_after(Some(&expected), found);
        assert_eq!(after(Ok(listed(vec![one.clone()]))), LayoutAfter::Same);
        let other = disks::Partition { size: 1 << 30, ..one.clone() };
        assert_eq!(after(Ok(listed(vec![other]))), LayoutAfter::Changed);
        // the incident: one partition at 1 MiB before, "Uninitialized" after
        assert_eq!(after(Ok(listed(Vec::new()))), LayoutAfter::TableGone);
        assert_eq!(after(Err("drive disk6 is not connected".into())), LayoutAfter::DriveGone);
        assert_eq!(
            after(Err("drive disk6 changed (size 1 instead of 2); choose it again".into())),
            LayoutAfter::DriveGone
        );
        assert_eq!(after(Err("diskutil list -plist did not answer in 10 s".into())), LayoutAfter::Unreadable);
        let unreadable = Disk { table_unreadable: true, ..listed(Vec::new()) };
        assert_eq!(after(Ok(unreadable)), LayoutAfter::Unreadable);
        // a blank drive that stays blank is the same
        let blank = Fingerprint::of(&listed(Vec::new()));
        assert_eq!(layout_after(Some(&blank), Ok(listed(Vec::new()))), LayoutAfter::Same);
    }

    #[test]
    fn an_unreadable_start_layout_is_not_compared() {
        let one = disks::Partition { offset: MIB, size: 8_303_673_344, id: "U".into() };
        let unreadable = Disk { table_unreadable: true, ..listed(Vec::new()) };
        let job = Job { action: Action::Probe, layout: Some(Fingerprint::of(&unreadable)), ..job(0, Fs::Exfat) };
        assert_eq!(expected_layout(&job, &unreadable), None);
        let readable = listed(vec![one.clone()]);
        assert_eq!(expected_layout(&Job { layout: None, ..job.clone() }, &readable), Some(Fingerprint::of(&readable)));
        // whatever the drive lists afterwards, the layout neither passes nor fails
        for found in [unreadable, listed(Vec::new()), readable] {
            let after = layout_after(None, Ok(found));
            assert_eq!(after, LayoutAfter::NotCompared);
            assert!(after.settled());
        }
        assert_eq!(serde_json::to_value(LayoutAfter::NotCompared).unwrap(), "notCompared");
        // a drive that is gone, or a list that does not answer, is still a failure
        assert_eq!(layout_after(None, Err("drive disk6 is not connected".into())), LayoutAfter::DriveGone);
        assert_eq!(layout_after(None, Err("diskutil did not answer".into())), LayoutAfter::Unreadable);
    }

    #[test]
    fn stage_info_has_no_labels() {
        let disk = Disk {
            volumes: vec![disks::Volume {
                mount: "/Volumes/PHOTOS".into(),
                label: "PHOTOS".into(),
                fs: "ExFAT".into(),
                total: 10,
                free: 5,
                writable: true,
            }],
            partition_fs: vec!["ExFAT".into()],
            ..listed(vec![disks::Partition { offset: MIB, size: 10, id: "SECRET-UUID".into() }])
        };
        let info = found_info(&disk).to_string();
        for secret in ["PHOTOS", "/Volumes", "SECRET-UUID"] {
            assert!(!info.contains(secret), "{secret} in {info}");
        }
        assert_eq!(found_info(&disk)["volumesMounted"], 1);
        assert_eq!(found_info(&disk)["partitions"][0], json!({"offset": MIB, "size": 10, "fs": "ExFAT"}));
    }
}
