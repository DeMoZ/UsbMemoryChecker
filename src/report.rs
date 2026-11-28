//! The report of one check or repair: a small JSON file in the user's logs folder, so that a failed or strange
//! outcome can be understood afterwards. The window (or the command line) writes it as the user; the elevated helper
//! only hands over its event log and its result.
//!
//! A report holds stages, error codes and offsets, counts, the restore check and the partition layout. It never
//! holds file contents, data of the drive or digests of it, volume labels, mount paths, volume UUIDs, disk ids of
//! the partition table, serial numbers, the user name or the home path: every free text goes through [`redact`].

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

use crate::disks::{Disk, Volume};
use crate::fullcheck;
use crate::helper::RunError;
use crate::probe::MAX_LISTED;

pub const FORMAT: u32 = 1;
/// Most reports kept, and the longest time one is kept.
const KEEP: usize = 50;
const MAX_AGE: Duration = Duration::from_secs(90 * 86_400);
const MAX_BYTES: usize = 256 * 1024;
/// Most stages listed (a run has a dozen of them).
const MAX_STAGES: usize = 200;
/// Most characters of a free text (an error of the system, the output of a tool).
const MAX_TEXT: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Probe,
    Full,
    Fix,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Probe => "probe",
            Kind::Full => "full",
            Kind::Fix => "fix",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub id: String,
    pub bus: String,
    pub kind: String,
    pub model: String,
    pub declared: u64,
    pub block_size: u64,
    pub partitions: Vec<Value>,
    pub table_unreadable: bool,
    pub volumes_mounted: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    pub name: String,
    pub at: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<Value>,
}

/// What a quick check reports beyond its stages.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbePart {
    pub grid: Value,
    pub counts: Value,
    pub failures: Vec<Value>,
    pub failures_not_listed: u64,
    pub restore_check: Value,
    pub flush: Value,
    pub layout_before: Value,
    pub layout_after: Value,
    /// The access error: cause, operation and error code.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub access: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixPart {
    /// The planned partition size.
    pub size: u64,
    pub fs: String,
    pub tool_exit: Option<i64>,
    pub output: Option<String>,
}

/// How the run ended, as the window decided it.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomePart {
    /// The window's error key, or `ok`.
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Quick check: whether the result was recorded, and why a replug is asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub format: u32,
    pub app: String,
    pub os: String,
    pub arch: String,
    pub kind: &'static str,
    pub started_at: String,
    pub ended_at: String,
    pub drive: DriveInfo,
    pub stages: Vec<Stage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<ProbePart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixPart>,
    pub outcome: OutcomePart,
    /// Start time (Unix seconds) and kind, for the file name.
    #[serde(skip)]
    started: u64,
    #[serde(skip)]
    what: Kind,
}

/// The folder of the reports.
pub fn dir() -> PathBuf {
    crate::net::logs_dir().join("checks")
}

/// The partitions of a drive as the report and the helper's log list them: offset, size and file system.
pub fn partitions_of(disk: &Disk) -> Vec<Value> {
    disk.partitions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let fs = disk.partition_fs.get(i).map_or("", String::as_str);
            json!({"offset": p.offset, "size": p.size, "fs": fs})
        })
        .collect()
}

/// A layout in two numbers: how many partitions, and where each one ends.
pub fn layout_summary(disk: &Disk) -> Value {
    let ends: Vec<u64> = disk.partitions.iter().map(|p| p.offset.saturating_add(p.size)).collect();
    json!({"n": ends.len(), "ends": ends})
}

/// The drive with the name of the volume a repair is about to make, so that [`redact`] knows it too.
pub fn with_new_label(disk: &Disk, label: &str) -> Disk {
    let mut disk = disk.clone();
    disk.volumes.push(Volume {
        mount: format!("/Volumes/{label}"),
        label: label.to_string(),
        fs: String::new(),
        total: 0,
        free: 0,
        writable: false,
    });
    disk
}

/// Replaces `word` where it stands on its own (not inside a longer word); case sensitive.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let part = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut done = 0;
    for (at, found) in text.match_indices(word) {
        let before = text[..at].chars().next_back();
        let after = text[at + found.len()..].chars().next();
        if before.is_some_and(part) || after.is_some_and(part) {
            continue;
        }
        out.push_str(&text[done..at]);
        out.push_str(with);
        done = at + found.len();
    }
    out.push_str(&text[done..]);
    out
}

/// Replaces what looks like a UUID or a GUID (8-4-4-4-12 hexadecimal digits).
fn replace_uuids(text: &str) -> String {
    let bytes = text.as_bytes();
    let is_uuid = |w: &[u8]| {
        w.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
    };
    let mut out = String::with_capacity(text.len());
    let (mut done, mut i) = (0, 0);
    while i + 36 <= bytes.len() {
        // a match is all ASCII, so both of its ends are character boundaries
        if is_uuid(&bytes[i..i + 36]) {
            out.push_str(&text[done..i]);
            out.push_str("<uuid>");
            i += 36;
            done = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[done..]);
    out
}

/// A free text (an error of the system, the output of a tool) without anything that names the user or the drive's
/// content: mount paths and volume labels become `<volume>`, the home folder `<home>`, the user name `<user>`,
/// UUIDs `<uuid>`; a line about a serial number or the computer's name is dropped.
pub fn redact(text: &str, disk: &Disk, home: &Path) -> String {
    let mut out: String = text
        .split('\n')
        .map(|line| {
            let low = line.to_lowercase();
            if low.contains("serial") || low.starts_with("on computer:") { "<removed>" } else { line }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut mounts: Vec<&str> = disk.volumes.iter().map(|v| v.mount.trim_end_matches('\\')).collect();
    mounts.sort_by_key(|m| std::cmp::Reverse(m.len()));
    for mount in mounts.into_iter().filter(|m| m.len() > 1) {
        out = out.replace(mount, "<volume>");
    }
    let home_text = home.to_string_lossy();
    let home_text = home_text.trim_end_matches(['/', '\\']);
    if home_text.len() > 1 {
        out = out.replace(home_text, "<home>");
    }
    // any other volume under /Volumes
    while let Some(at) = out.find("/Volumes/") {
        let rest = &out[at + "/Volumes/".len()..];
        let end = rest.find(['/', ':', '\'', '"', '\n']).unwrap_or(rest.len());
        out.replace_range(at..at + "/Volumes/".len() + end, "<volume>");
    }
    for id in disk.partitions.iter().map(|p| p.id.as_str()).chain([disk.table_id.as_str()]) {
        if !id.is_empty() {
            out = out.replace(id, "<uuid>");
        }
    }
    out = replace_uuids(&out);
    let mut labels: Vec<&str> = disk.volumes.iter().map(|v| v.label.as_str()).collect();
    labels.sort_by_key(|l| std::cmp::Reverse(l.len()));
    for label in labels.into_iter().filter(|l| !l.trim().is_empty()) {
        out = replace_word(&out, label, "<volume>");
    }
    if let Some(user) = home.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|u| u.len() > 1) {
        out = replace_word(&out, &user, "<user>");
    }
    out
}

/// [`redact`], then shortened.
pub fn free_text(text: &str, disk: &Disk, home: &Path) -> String {
    let mut out = redact(text.trim(), disk, home);
    // Windows tools print the computer's name
    if let Ok(computer) = std::env::var("COMPUTERNAME")
        && computer.len() > 1
    {
        out = replace_word(&out, &computer, "<computer>");
    }
    out.chars().take(MAX_TEXT).collect()
}

/// Runs every free text inside a stage's details through [`free_text`].
fn clean_info(v: &mut Value, disk: &Disk, home: &Path) {
    match v {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                match value {
                    Value::String(s) if ["output", "error", "text", "failed"].contains(&key.as_str()) => {
                        *s = free_text(s, disk, home);
                    }
                    _ => clean_info(value, disk, home),
                }
            }
        }
        Value::Array(list) => list.iter_mut().for_each(|v| clean_info(v, disk, home)),
        _ => {}
    }
}

/// Date and time (UTC) of Unix seconds: year, month, day, hour, minute, second.
fn utc(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // civil date from the day number (Howard Hinnant's algorithm), with years that start on 1 March
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day, rest / 3600, rest % 3600 / 60, rest % 60)
}

fn iso(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = utc(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn value_or_null(v: Option<&Value>) -> Value {
    v.cloned().unwrap_or(Value::Null)
}

impl Report {
    pub fn new(kind: Kind, disk: &Disk, started: u64, ended: u64) -> Report {
        Report {
            format: FORMAT,
            app: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            kind: kind.name(),
            started_at: iso(started),
            ended_at: iso(ended),
            drive: DriveInfo {
                id: disk.id.clone(),
                bus: disk.bus.clone(),
                kind: disk.kind.clone(),
                model: disk.name.clone(),
                declared: disk.size,
                block_size: disk.block_size,
                partitions: partitions_of(disk),
                table_unreadable: disk.table_unreadable,
                volumes_mounted: disk.volumes.len(),
            },
            stages: Vec::new(),
            probe: None,
            full: None,
            fix: None,
            outcome: OutcomePart::default(),
            started,
            what: kind,
        }
    }

    /// Sets how the run ended; a free text in it is cleaned.
    pub fn ended(mut self, mut outcome: OutcomePart, disk: &Disk, home: &Path) -> Report {
        outcome.error = outcome.error.map(|e| free_text(&e, disk, home));
        self.outcome = outcome;
        self
    }

    /// The stages from the lines of the helper's event log. The single failures and the summary are not stages:
    /// they are returned for the caller.
    fn read_log(&mut self, log: &[String], disk: &Disk, home: &Path) -> (Vec<Value>, Value) {
        let (mut failures, mut summary) = (Vec::new(), Value::Null);
        for line in log {
            let Ok(mut v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(name) = v["stage"].as_str().map(str::to_string) else { continue };
            let mut info = v["info"].take();
            clean_info(&mut info, disk, home);
            match name.as_str() {
                "failure" => failures.push(info),
                "summary" => summary = info,
                _ if self.stages.len() < MAX_STAGES => self.stages.push(Stage {
                    name,
                    at: iso(v["t_ms"].as_u64().unwrap_or(0) / 1000),
                    ok: v["ok"].as_bool().unwrap_or(false),
                    info: (!info.is_null()).then_some(info),
                }),
                _ => {}
            }
        }
        (failures, summary)
    }

    fn stage_info(&self, name: &str) -> Option<&Value> {
        self.stages.iter().rev().find(|s| s.name == name).and_then(|s| s.info.as_ref())
    }

    /// The quick check: from the helper's log (also when it stopped without a result) and its result.
    pub fn probe_data(mut self, log: &[String], result: &Result<Value, RunError>, disk: &Disk, home: &Path) -> Report {
        let (mut failures, summary) = self.read_log(log, disk, home);
        // a verdict carries its numbers itself; an error carries them in its details
        let (numbers, details) = match result {
            Ok(v) => (v.clone(), v.clone()),
            Err(RunError::Failed { details, .. }) => (details["counts"].clone(), details.clone()),
            Err(RunError::Declined) => (Value::Null, Value::Null),
        };
        let listed = failures.len().min(MAX_LISTED);
        let not_listed = summary["failuresNotListed"].as_u64().unwrap_or(0) + (failures.len() - listed) as u64;
        failures.truncate(listed);
        let mut counts = serde_json::Map::new();
        for key in ["mismatches", "ioErrors", "unsaved", "restoreFailures"] {
            // without a result: what the log said last
            let value = [&numbers, &summary["counts"]]
                .into_iter()
                .chain(self.stage_info("restore"))
                .chain(self.stage_info("round"))
                .find_map(|v| v.get(key).and_then(Value::as_u64));
            if let Some(n) = value {
                counts.insert(key.into(), json!(n));
            }
        }
        let grid = match summary.get("grid") {
            Some(g) => g.clone(),
            None => self.stage_info("baseline").map_or(
                Value::Null,
                |b| json!({"step": b["step"], "points": b["points"], "coarseBlocks": b["coarseBlocks"]}),
            ),
        };
        let mut restore_check =
            details.get("restoreCheck").cloned().unwrap_or_else(|| value_or_null(self.stage_info("verify")));
        if let Some(first) = restore_check.get_mut("first").and_then(Value::as_array_mut) {
            first.truncate(MAX_LISTED);
        }
        let mut flush = details
            .get("flush")
            .cloned()
            .unwrap_or_else(|| value_or_null(self.stage_info("flush").and_then(|f| f.get("result"))));
        clean_info(&mut flush, disk, home);
        let layout_before = self.stage_info("found").and_then(|f| f["partitions"].as_array()).map_or_else(
            || layout_summary(disk),
            |list| {
                let ends: Vec<u64> = list
                    .iter()
                    .map(|p| p["offset"].as_u64().unwrap_or(0).saturating_add(p["size"].as_u64().unwrap_or(0)))
                    .collect();
                json!({"n": ends.len(), "ends": ends})
            },
        );
        self.probe = Some(ProbePart {
            grid,
            counts: Value::Object(counts),
            failures,
            failures_not_listed: not_listed,
            restore_check,
            flush,
            layout_before,
            layout_after: value_or_null(details.get("layoutAfter")),
            access: value_or_null(details.get("access")),
        });
        self
    }

    /// The full check: its start and end, and the numbers the window shows. None: it did not run.
    pub fn full_data(
        mut self,
        result: Option<&Result<fullcheck::Outcome, fullcheck::Error>>,
        disk: &Disk,
        home: &Path,
    ) -> Report {
        let (ok, info) = match result {
            Some(Ok(o)) => {
                let write_error = o.write_error.as_deref().map(|e| free_text(e, disk, home));
                self.full = Some(json!({"total": o.total, "usedBefore": o.used_before, "tested": o.tested,
                    "bad": o.bad, "realEstimate": o.real_estimate, "stoppedEarly": o.stopped_early,
                    "writeError": write_error, "seconds": o.write.seconds + o.read.seconds}));
                (true, None)
            }
            Some(Err(e)) => {
                let text = match e {
                    fullcheck::Error::Io(text) => text.clone(),
                    other => format!("{other:?}"),
                };
                (false, Some(json!({"error": free_text(&text, disk, home)})))
            }
            None => (false, None),
        };
        self.stages = vec![
            Stage { name: "start".into(), at: self.started_at.clone(), ok: result.is_some(), info: None },
            Stage { name: "end".into(), at: self.ended_at.clone(), ok, info },
        ];
        self
    }

    /// The repair: the planned size and file system, and what the system tool did.
    pub fn fix_data(mut self, size: u64, fs: &str, log: &[String], disk: &Disk, home: &Path) -> Report {
        self.read_log(log, disk, home);
        let tool = self.stage_info("tool");
        self.fix = Some(FixPart {
            size,
            fs: fs.to_string(),
            tool_exit: tool.and_then(|t| t["exit"].as_i64()),
            output: tool.and_then(|t| t["output"].as_str()).map(str::to_string),
        });
        self
    }

    /// The file's text, below [`MAX_BYTES`]: the details of the stages are dropped if need be.
    fn serialize_small(&mut self) -> io::Result<Vec<u8>> {
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        if text.len() < MAX_BYTES {
            return Ok(text);
        }
        self.stages.iter_mut().for_each(|s| s.info = None);
        serde_json::to_vec_pretty(self).map_err(io::Error::other)
    }
}

/// `20261128-101502-probe-disk6.json` (UTC), with `-2`, `-3`, … when the name is taken.
pub fn file_name(now: u64, kind: Kind, disk_id: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let (y, mo, d, h, mi, s) = utc(now);
    let id: String = disk_id.chars().filter(char::is_ascii_alphanumeric).collect();
    let id = if id.is_empty() { "drive" } else { &id };
    let base = format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{}-{id}", kind.name());
    let mut name = format!("{base}.json");
    let mut n = 1;
    while taken(&name) {
        n += 1;
        name = format!("{base}-{n}.json");
    }
    name
}

/// `^\d{8}-\d{6}-(probe|full|fix)-[A-Za-z0-9]+(-\d+)?\.json$`
fn is_report_name(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    let Some(stem) = name.strip_suffix(".json") else { return false };
    let parts: Vec<&str> = stem.split('-').collect();
    (4..=5).contains(&parts.len())
        && parts[0].len() == 8
        && digits(parts[0])
        && parts[1].len() == 6
        && digits(parts[1])
        && ["probe", "full", "fix"].contains(&parts[2])
        && !parts[3].is_empty()
        && parts[3].bytes().all(|c| c.is_ascii_alphanumeric())
        && parts.get(4).is_none_or(|n| digits(n))
}

/// Writes the report into `dir` (readable by the owner only on Unix) through a temporary file and a rename, then
/// removes old reports.
pub fn write(dir: &Path, report: &mut Report) -> io::Result<PathBuf> {
    #[allow(unused_mut)]
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let name = file_name(report.started, report.what, &report.drive.id, &|n| dir.join(n).exists());
    let path = dir.join(&name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let text = report.serialize_small()?;
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&tmp)?.write_all(&text)?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    prune(dir, SystemTime::now());
    Ok(path)
}

/// Removes reports older than 90 days, then the oldest ones until at most 50 are left. Only files with a report's
/// name are touched.
pub fn prune(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut reports: Vec<(SystemTime, String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let meta = e.metadata().ok()?;
            (meta.is_file() && is_report_name(&name)).then(|| (meta.modified().unwrap_or(UNIX_EPOCH), name, e.path()))
        })
        .collect();
    reports.retain(|(modified, _, path)| {
        let old = now.duration_since(*modified).is_ok_and(|age| age > MAX_AGE);
        if old {
            let _ = std::fs::remove_file(path);
        }
        !old
    });
    reports.sort();
    let extra = reports.len().saturating_sub(KEEP);
    for (_, _, path) in &reports[..extra] {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disks::Partition;

    const DECLARED: u64 = 125_829_120_000;
    const NOW: u64 = 1_795_860_902; // 2026-11-28T10:15:02Z

    /// A temporary folder of one test, removed at its end.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("umc_report_{name}_{}", std::process::id()));
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

    fn private_disk() -> Disk {
        Disk {
            id: "disk6".into(),
            device: "/dev/rdisk6".into(),
            name: "Storage Device".into(),
            kind: "usb".into(),
            bus: "USB".into(),
            size: DECLARED,
            block_size: 512,
            volumes: vec![Volume {
                mount: "/Volumes/PHOTOS".into(),
                label: "PHOTOS".into(),
                fs: "ExFAT".into(),
                total: 10,
                free: 5,
                writable: true,
            }],
            partitions: vec![Partition {
                offset: 1 << 20,
                size: 8_303_673_344,
                id: "11112222-3333-4444-5555-666677778888".into(),
            }],
            table_unreadable: false,
            table_id: "9F8E7D6C".into(),
            partition_fs: vec!["ExFAT".into()],
        }
    }

    fn home() -> PathBuf {
        PathBuf::from("/Users/somebody")
    }

    fn line(stage: &str, ok: bool, info: Value) -> String {
        json!({"t_ms": NOW * 1000, "stage": stage, "ok": ok, "info": info}).to_string()
    }

    fn report() -> Report {
        Report::new(Kind::Probe, &private_disk(), NOW, NOW + 7)
    }

    fn json_of(report: &mut Report) -> String {
        String::from_utf8(report.serialize_small().unwrap()).unwrap()
    }

    #[test]
    fn dates_are_utc() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(951_826_554), "2000-02-29T12:15:54Z");
        assert_eq!(iso(NOW), "2026-11-28T10:15:02Z");
    }

    #[test]
    fn redact_replaces_mount_label_and_home() {
        let d = private_disk();
        let r = |t: &str| redact(t, &d, &home());
        assert_eq!(
            r("/Volumes/PHOTOS/UsbMemoryChecker-test/00001.bin: Input/output error"),
            "<volume>/UsbMemoryChecker-test/00001.bin: Input/output error"
        );
        // the label as a whole word only
        assert_eq!(r("volume PHOTOS, not PHOTOSHOP or OLD_PHOTOS"), "volume <volume>, not PHOTOSHOP or OLD_PHOTOS");
        assert_eq!(r("/Users/somebody/Library/x"), "<home>/Library/x");
        assert_eq!(r("owner somebody"), "owner <user>");
        assert_eq!(r("/Volumes/OTHER/f"), "<volume>/f");
        assert_eq!(
            r("uuid 11112222-3333-4444-5555-666677778888, 9F8E7D6C, ABCDEF01-2345-6789-abcd-ef0123456789 Юникод"),
            "uuid <uuid>, <uuid>, <uuid> Юникод"
        );
        assert_eq!(r("Volume name : X\nVolume Serial # : 693f1a2b\ndone"), "Volume name : X\n<removed>\ndone");
        assert_eq!(r("On computer: DESKTOP-1"), "<removed>");
        // the new volume of a repair is a label too
        let named = with_new_label(&d, "MYSTICK");
        assert_eq!(redact("Mounted MYSTICK at /Volumes/MYSTICK", &named, &home()), "Mounted <volume> at <volume>");
        assert_eq!(free_text(&"x".repeat(1000), &d, &home()).len(), MAX_TEXT);
    }

    #[test]
    fn report_has_no_private_fields() {
        let d = private_disk();
        let secret_text = "could not write /Volumes/PHOTOS/a (PHOTOS, /Users/somebody/x, \
                           11112222-3333-4444-5555-666677778888, 9F8E7D6C)";
        let log = vec![
            line("found", true, json!({"volumesMounted": 1, "partitions": partitions_of(&d)})),
            line("failure", false, json!({"offset": 4096, "op": "write", "errno": 5, "class": "ioError"})),
            line("remounted", false, json!({"exit": 1, "output": secret_text})),
            line("flush", false, json!({"result": {"failed": secret_text}})),
        ];
        let result = Err(RunError::Failed {
            message: secret_text.into(),
            cause: Some("unresponsive".into()),
            details: json!({"counts": {"ioErrors": 1}, "layoutAfter": "tableGone"}),
        });
        let outcome = OutcomePart { key: "errHelper".into(), error: Some(secret_text.into()), ..Default::default() };
        let mut r = report().probe_data(&log, &result, &d, &home()).ended(outcome, &d, &home());
        let text = json_of(&mut r);
        for secret in ["PHOTOS", "/Volumes", "somebody", "11112222-3333", "9F8E7D6C", "/dev/rdisk6"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert!(text.contains("<volume>") && text.contains("Storage Device"));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["format"], 1);
        assert_eq!(v["drive"]["partitions"][0], json!({"offset": 1_048_576, "size": 8_303_673_344u64, "fs": "ExFAT"}));
        assert_eq!(v["drive"]["volumesMounted"], 1);
        assert_eq!(v["probe"]["layoutBefore"], json!({"n": 1, "ends": [8_304_721_920u64]}));
        assert_eq!(v["probe"]["failures"][0]["errno"], 5);
        assert_eq!(v["startedAt"], "2026-11-28T10:15:02Z");
    }

    #[test]
    fn probe_report_from_helper_files() {
        let t = Temp::new("job");
        // the incident: the drive stopped answering, and afterwards it lists with no partitions
        let log = [
            line("found", true, json!({"volumesMounted": 1, "partitions": partitions_of(&private_disk())})),
            line("unmounted", true, json!({"volumes": 1})),
            line("open", true, json!({})),
            line("baseline", true, json!({"step": 67_108_864, "points": 1875, "coarseBlocks": 1882})),
            line(
                "failure",
                false,
                json!({"offset": 125_829_115_904u64, "op": "readOld", "errno": 5, "class": "ioError"}),
            ),
            line("round", false, json!({"n": 1, "ioErrors": 1882, "mismatches": 0, "abort": "unresponsive"})),
            line("restore", true, json!({"writes": 0, "restoreFailures": 0})),
            line("flush", true, json!({"result": "ok"})),
            line("verify", false, json!({"checked": 0, "unreadable": 6000})),
            line("closed", true, json!({})),
            line(
                "summary",
                false,
                json!({"counts": {"ioErrors": 1882, "mismatches": 0, "unsaved": 1882,
                "restoreFailures": 0}, "grid": {"step": 67_108_864, "points": 1875, "coarseBlocks": 1882},
                "failuresNotListed": 1850}),
            ),
            line("remounted", false, json!({"exit": 1, "output": "Volume on disk6 failed to mount"})),
            line("layout", false, json!({"result": "tableGone", "layout": {"n": 0, "ends": []}})),
        ];
        std::fs::write(t.0.join("log.jsonl"), log.join("\n") + "\n").unwrap();
        let result_json = json!({"ok": false, "error": "the drive stopped answering", "cause": "unresponsive",
            "details": {"counts": {"ioErrors": 1882, "mismatches": 0, "unsaved": 1882, "restoreFailures": 0},
                "restoreCheck": {"checked": 0, "unreadable": 6000, "mismatched": 0}, "flush": "ok",
                "layoutAfter": "tableGone"}});
        std::fs::write(t.0.join("result.json"), result_json.to_string()).unwrap();

        let lines = crate::helper::read_log(&t.0);
        let result = crate::helper::parse_result(&std::fs::read(t.0.join("result.json")).unwrap());
        let judged = crate::judge_probe(&result, &lines);
        let d = private_disk();
        let mut r = report().probe_data(&lines, &result, &d, &home()).ended(judged.outcome_part(&result), &d, &home());
        let v: Value = serde_json::from_str(&json_of(&mut r)).unwrap();
        let names: Vec<&str> = v["stages"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "found",
                "unmounted",
                "open",
                "baseline",
                "round",
                "restore",
                "flush",
                "verify",
                "closed",
                "remounted",
                "layout"
            ]
        );
        assert_eq!(v["probe"]["counts"]["ioErrors"], 1882);
        assert_eq!(v["probe"]["counts"]["mismatches"], 0);
        assert_eq!(v["probe"]["counts"]["unsaved"], 1882);
        assert_eq!(v["probe"]["counts"]["restoreFailures"], 0);
        assert_eq!(v["probe"]["grid"]["points"], 1875);
        assert_eq!(v["probe"]["failuresNotListed"], 1850);
        assert_eq!(v["probe"]["layoutAfter"], "tableGone");
        assert_eq!(v["probe"]["restoreCheck"]["unreadable"], 6000);
        assert_eq!(v["outcome"]["key"], "errUnresponsive");
        assert_eq!(v["outcome"]["cause"], "unresponsive");
        assert_eq!(v["outcome"]["trusted"], false);
        assert_eq!(v["outcome"]["notice"], "unresponsive");

        // the helper stopped without a result: the log still gives the stages and the counts
        let died: Result<Value, RunError> = Err(RunError::failed("the helper stopped without a result"));
        let mut r = report().probe_data(&lines[..6], &died, &d, &home());
        let v: Value = serde_json::from_str(&json_of(&mut r)).unwrap();
        assert_eq!(v["stages"].as_array().unwrap().len(), 5);
        assert_eq!(v["probe"]["counts"]["ioErrors"], 1882);
        assert_eq!(v["probe"]["grid"]["coarseBlocks"], 1882);
        assert_eq!(v["probe"]["layoutAfter"], Value::Null);
    }

    #[test]
    fn privacy_report_names_the_operation_and_code() {
        let log = vec![line("open", false, json!({"cause": "privacy", "op": "open", "errno": 1}))];
        let result = Err(RunError::Failed {
            message: "could not open /dev/rdisk6".into(),
            cause: Some("privacy".into()),
            details: json!({"access": {"cause": "privacy", "op": "open", "errno": 1}, "layoutAfter": "same"}),
        });
        let judged = crate::judge_probe(&result, &log);
        let d = private_disk();
        let r = report().probe_data(&log, &result, &d, &home()).ended(judged.outcome_part(&result), &d, &home());
        assert_eq!(r.outcome.cause.as_deref(), Some("privacy"));
        assert_eq!(r.outcome.key, "errFullDiskAccess");
        let probe = r.probe.unwrap();
        assert_eq!(probe.access, json!({"cause": "privacy", "op": "open", "errno": 1}));
        assert_eq!(r.stages[0].name, "open");
    }

    #[test]
    fn full_and_fix_reports() {
        let d = private_disk();
        let io: Result<fullcheck::Outcome, fullcheck::Error> =
            Err(fullcheck::Error::Io("/Volumes/PHOTOS/UsbMemoryChecker-test/00001.bin: I/O error".into()));
        let mut r = Report::new(Kind::Full, &d, NOW, NOW + 1).full_data(Some(&io), &d, &home());
        let text = json_of(&mut r);
        assert!(text.contains("<volume>/UsbMemoryChecker-test/00001.bin"), "{text}");
        assert!(!text.contains("PHOTOS") && !text.contains("11112222"));

        let done: Result<fullcheck::Outcome, fullcheck::Error> = Ok(fullcheck::Outcome {
            tested: 100,
            ok: 100,
            bad: 0,
            first_bad: None,
            total: 200,
            used_before: 10,
            real_estimate: 100,
            write: Default::default(),
            read: Default::default(),
            write_error: None,
            stopped_early: false,
        });
        let r = Report::new(Kind::Full, &d, NOW, NOW + 1).full_data(Some(&done), &d, &home());
        assert_eq!(r.full.as_ref().unwrap()["tested"], 100);
        assert_eq!(r.stages.len(), 2);

        let log = vec![line("tool", false, json!({"exit": 1, "output": "Error: -69877 on PHOTOS"}))];
        let r = Report::new(Kind::Fix, &d, NOW, NOW + 1).fix_data(8_303_673_344, "exfat", &log, &d, &home());
        let fix = r.fix.unwrap();
        assert_eq!((fix.size, fix.fs.as_str(), fix.tool_exit), (8_303_673_344, "exfat", Some(1)));
        assert_eq!(fix.output.as_deref(), Some("Error: -69877 on <volume>"));
    }

    #[test]
    fn file_names_are_unique() {
        assert_eq!(file_name(NOW, Kind::Probe, "disk6", &|_| false), "20261128-101502-probe-disk6.json");
        let mut taken = Vec::new();
        for _ in 0..3 {
            let name = file_name(NOW, Kind::Fix, "3", &|n| taken.iter().any(|t| t == n));
            assert!(is_report_name(&name), "{name}");
            taken.push(name);
        }
        assert_eq!(
            taken,
            ["20261128-101502-fix-3.json", "20261128-101502-fix-3-2.json", "20261128-101502-fix-3-3.json"]
        );
        for other in
            ["notes.txt", "20261128-101502-speed-disk6.json", "x-101502-fix-3.json", ".20261128-101502-fix-3.json.tmp"]
        {
            assert!(!is_report_name(other), "{other}");
        }
    }

    fn put(dir: &Path, name: &str, age: Duration) {
        let f = std::fs::File::create(dir.join(name)).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut n: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        n.sort();
        n
    }

    #[test]
    fn prune_keeps_50_and_unrelated_files() {
        let t = Temp::new("prune_50");
        for i in 0..60u64 {
            put(&t.0, &format!("20261128-0000{i:02}-probe-disk6.json"), Duration::from_secs((60 - i) * 60));
        }
        put(&t.0, "notes.txt", Duration::from_secs(400 * 86_400));
        prune(&t.0, SystemTime::now());
        let left = names(&t.0);
        assert_eq!(left.len(), 51);
        assert!(left.contains(&"notes.txt".to_string()));
        assert!(!left.contains(&"20261128-000009-probe-disk6.json".to_string()));
        assert!(left.contains(&"20261128-000010-probe-disk6.json".to_string()));
    }

    #[test]
    fn prune_drops_older_than_90_days() {
        let t = Temp::new("prune_90");
        put(&t.0, "20260101-000000-full-disk6.json", Duration::from_secs(91 * 86_400));
        put(&t.0, "20261101-000000-full-disk6.json", Duration::from_secs(89 * 86_400));
        put(&t.0, "old.json", Duration::from_secs(200 * 86_400));
        prune(&t.0, SystemTime::now());
        assert_eq!(names(&t.0), ["20261101-000000-full-disk6.json", "old.json"]);
    }

    fn worst_case() -> Report {
        let d = private_disk();
        let mut log: Vec<String> = (0..1875u64)
            .map(|i| {
                line("failure", false, json!({"offset": i << 26, "op": "readBack", "errno": 5, "class": "ioError"}))
            })
            .collect();
        let long = "x".repeat(5000);
        log.extend((0..3000).map(|n| line("round", true, json!({"n": n, "output": long, "points": 1875}))));
        let first: Vec<Value> = (0..5000u64).map(|i| json!({"offset": i * 4096, "kind": "unreadable"})).collect();
        let result = Err(RunError::Failed {
            message: long.clone(),
            cause: Some("unresponsive".into()),
            details: json!({"restoreCheck": {"first": first}}),
        });
        let outcome = OutcomePart { key: "errUnresponsive".into(), error: Some(long.clone()), ..Default::default() };
        report().probe_data(&log, &result, &d, &home()).ended(outcome, &d, &home())
    }

    #[test]
    fn failures_are_capped() {
        let r = worst_case();
        let probe = r.probe.unwrap();
        assert_eq!(probe.failures.len(), 32);
        assert_eq!(probe.failures_not_listed, 1843);
        assert_eq!(probe.restore_check["first"].as_array().unwrap().len(), 32);
    }

    #[test]
    fn report_is_small() {
        let mut r = worst_case();
        let text = r.serialize_small().unwrap();
        assert!(text.len() < MAX_BYTES, "{}", text.len());
        // stages whose details are too big lose them, the rest of the report stays
        assert!(serde_json::from_slice::<Value>(&text).unwrap()["probe"]["failures"].is_array());
    }

    #[cfg(unix)]
    #[test]
    fn report_file_modes() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new("modes");
        let dir = t.0.join("logs/checks");
        let first = write(&dir, &mut report()).unwrap();
        let second = write(&dir, &mut report()).unwrap();
        assert_ne!(first, second);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&first), 0o600);
        assert_eq!(names(&dir).len(), 2, "no temporary file is left");
        // a folder that cannot be made is an error for the caller to print
        std::fs::write(t.0.join("file"), "").unwrap();
        assert!(write(&t.0.join("file/checks"), &mut report()).is_err());
    }
}
