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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::disks::{self, Disk};
use crate::probe;

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

fn write_json(path: &Path, v: &Value) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Entry of `--helper <folder>`; returns the process exit code.
pub fn main(dir: &Path) -> i32 {
    disks::set_elevated();
    let result = std::fs::read(dir.join("job.json"))
        .map_err(|e| format!("could not read the job: {e}"))
        .and_then(|d| serde_json::from_slice::<Job>(&d).map_err(|e| format!("bad job: {e}")))
        .and_then(|job| run_job(&job, dir));
    let ok = result.is_ok();
    let v = match result {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(e) => json!({"ok": false, "error": e}),
    };
    write_json(&dir.join("result.json"), &v);
    if ok { 0 } else { 1 }
}

/// The drive found again must have the partition layout the window saw (a drive swapped while the password window
/// was open has another one).
fn check_layout(job: &Job, disk: &Disk) -> Result<(), String> {
    match &job.layout {
        Some(layout) if &crate::capacity::Fingerprint::of(disk) != layout => {
            Err("the drive changed since you chose it (its partitions are not the same); choose it again".into())
        }
        _ => Ok(()),
    }
}

fn run_job(job: &Job, dir: &Path) -> Result<Value, String> {
    if !disks::valid_id(&job.disk) {
        return Err(format!("bad drive id {:?}", job.disk));
    }
    let disk = disks::find(&job.disk, job.size)?;
    check_layout(job, &disk)?;
    if job.action == Action::Fix && disk.table_unreadable {
        return Err("the partition table of the drive could not be read; look at the drive again".into());
    }
    let cancel_file = dir.join("cancel");
    let progress_file = dir.join("progress.json");
    let cancel = AtomicBool::new(false);
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut progress = |fraction: f64, stage: &'static str| {
        if last.elapsed() >= Duration::from_millis(200) || fraction >= 1.0 {
            last = Instant::now();
            if cancel_file.exists() {
                cancel.store(true, Ordering::Relaxed);
            }
            write_json(&progress_file, &json!({"fraction": fraction, "stage": stage}));
        }
    };
    match job.action {
        Action::Probe => run_probe(&disk, &cancel, &mut progress),
        Action::Fix => {
            progress(0.05, "prFixing");
            let out = run_fix(&disk, job)?;
            progress(1.0, "prFixing");
            Ok(json!({"log": out}))
        }
    }
}

fn probe_result(r: Result<probe::Outcome, probe::Abort>) -> Result<Value, String> {
    match r {
        Ok(o) => serde_json::to_value(o).map_err(|e| e.to_string()),
        Err(probe::Abort::Cancelled) => Err("cancelled".into()),
        Err(probe::Abort::TooSmall) => Err("the drive is too small for the quick probe".into()),
        Err(probe::Abort::Access(e)) => Err(format!("the system refused access to the drive: {e}")),
    }
}

#[cfg(target_os = "macos")]
fn diskutil(args: &[&str]) -> Result<String, String> {
    let out = crate::net::command("/usr/sbin/diskutil")
        .args(args)
        .output()
        .map_err(|e| format!("could not run diskutil: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(format!("diskutil {}: {}", args.join(" "), text.trim())) }
}

#[cfg(target_os = "macos")]
fn run_probe(disk: &Disk, cancel: &AtomicBool, progress: &mut probe::Progress) -> Result<Value, String> {
    progress(0.0, "prPrepare");
    let was_mounted = !disk.volumes.is_empty();
    if was_mounted {
        diskutil(&["unmountDisk", &disk.id])
            .map_err(|e| format!("could not unmount the drive (close its files): {e}"))?;
    }
    let result = crate::rawdev::RawDevice::open(Path::new(&disk.device))
        .map_err(|e| format!("could not open {}: {e}", disk.device))
        .and_then(|mut dev| probe_result(probe::probe(&mut dev, probe::Params::default(), cancel, progress)));
    if was_mounted {
        let _ = diskutil(&["mountDisk", &disk.id]);
    }
    result
}

#[cfg(windows)]
fn run_probe(disk: &Disk, cancel: &AtomicBool, progress: &mut probe::Progress) -> Result<Value, String> {
    progress(0.0, "prPrepare");
    let letters: Vec<String> = disk.volumes.iter().map(|v| v.mount.trim_end_matches(['\\', ':']).to_string()).collect();
    let locks = crate::rawdev::VolumeLocks::take(&letters)?;
    let result = crate::rawdev::RawDevice::open(Path::new(&disk.device))
        .map_err(|e| format!("could not open {}: {e}", disk.device))
        .and_then(|mut dev| probe_result(probe::probe(&mut dev, probe::Params::default(), cancel, progress)));
    drop(locks);
    result
}

#[cfg(not(any(target_os = "macos", windows)))]
fn run_probe(_: &Disk, _: &AtomicBool, _: &mut probe::Progress) -> Result<Value, String> {
    Err("not supported on this system".into())
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

fn run_fix(disk: &Disk, job: &Job) -> Result<String, String> {
    let (size, args) = plan_fix(&disk.id, job)?;
    #[cfg(target_os = "macos")]
    {
        let _ = size;
        let _ = diskutil(&["unmountDisk", "force", &disk.id]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        diskutil(&args)
    }
    #[cfg(windows)]
    {
        let _ = args;
        let script = std::env::temp_dir().join(format!("UsbMemoryChecker-fix-{}.txt", std::process::id()));
        std::fs::write(&script, diskpart_script(&disk.id, job, size)).map_err(|e| e.to_string())?;
        let out = crate::net::command("diskpart.exe").arg("/s").arg(&script).output();
        let _ = std::fs::remove_file(&script);
        let out = out.map_err(|e| format!("could not run diskpart: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if out.status.success() { Ok(text) } else { Err(format!("diskpart failed: {}", text.trim())) }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (disk, job, size, args);
        Err("not supported on this system".into())
    }
}

// ------------------------------------------------------------------ the window side

pub enum RunError {
    /// The user closed the password / UAC window.
    Declined,
    Failed(String),
}

fn job_dir() -> PathBuf {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    std::env::temp_dir().join(format!("UsbMemoryChecker-job-{}-{t}", std::process::id()))
}

/// Runs the job elevated and waits for it. `on_progress` gets (fraction, stage key).
/// `UMC_NO_ELEVATE=1` runs the helper with the current rights (for testing on a RAM disk).
pub fn run(
    job: &Job,
    prompt: &str,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f64, String),
) -> Result<Value, RunError> {
    let dir = job_dir();
    std::fs::create_dir_all(&dir).map_err(|e| RunError::Failed(e.to_string()))?;
    let result = run_in(&dir, job, prompt, cancel, on_progress);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn run_in(
    dir: &Path,
    job: &Job,
    prompt: &str,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f64, String),
) -> Result<Value, RunError> {
    let fail = |e: String| RunError::Failed(e);
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
    let v: Value = serde_json::from_slice(&data).map_err(|e| fail(e.to_string()))?;
    if v["ok"].as_bool() == Some(true) {
        Ok(v["result"].clone())
    } else {
        Err(fail(v["error"].as_str().unwrap_or("unknown error").to_string()))
    }
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
                .map_err(|e| RunError::Failed(e.to_string()))?;
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
                .map_err(|e| RunError::Failed(format!("could not run osascript: {e}")))?;
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
                Err(e) => Some(Err(RunError::Failed(e.to_string()))),
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
        return Err(if e.raw_os_error() == Some(1223) { RunError::Declined } else { RunError::Failed(e.to_string()) });
    }
    if info.hProcess.is_null() {
        return Err(RunError::Failed("the elevated helper did not start".into()));
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
}
