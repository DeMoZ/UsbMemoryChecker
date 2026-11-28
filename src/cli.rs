//! Command line without a window, for scripts and for checking the engines on a real drive:
//!
//! ```text
//! usbmemorychecker --cli list
//! usbmemorychecker --cli speed <disk> <MB>
//! usbmemorychecker --cli full <disk> [limit MB]
//! usbmemorychecker --cli probe <disk>        (asks for administrator rights like the window does)
//! usbmemorychecker --cli fix <disk> <real bytes|0> [exfat|fat32] [LABEL]
//! ```
//!
//! Prints JSON results to stdout, progress to stderr.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde_json::json;

use crate::{bench, disks, fullcheck, helper, i18n, net, report, settings};

fn disk(id: Option<&String>) -> Result<disks::Disk, String> {
    let id = id.ok_or("no drive id")?;
    disks::list()?.into_iter().find(|d| &d.id == id).ok_or_else(|| format!("drive {id} not found"))
}

fn mount(d: &disks::Disk) -> Result<PathBuf, String> {
    d.main_volume().map(|v| PathBuf::from(&v.mount)).ok_or_else(|| "the drive has no mounted volume".into())
}

/// What a command ended with: the JSON or the error text, and the report it left.
struct Done {
    result: Result<serde_json::Value, String>,
    report: Option<PathBuf>,
}

impl From<Result<serde_json::Value, String>> for Done {
    fn from(result: Result<serde_json::Value, String>) -> Done {
        Done { result, report: None }
    }
}

pub fn main(args: &[String]) -> i32 {
    let done = run(args);
    let code = match done.result {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    };
    // always the last line on stderr
    if let Some(path) = done.report {
        eprintln!("report: {}", path.display());
    }
    code
}

/// The text of a failed helper run: its cause, when it gave one, and its message.
fn failure_text(cause: Option<&str>, message: &str) -> String {
    match cause {
        Some(cause) => format!("{cause}: {message}"),
        None => message.to_string(),
    }
}

/// Writes the report as the window does; a failure is only printed.
fn write_report(mut rep: report::Report) -> Option<PathBuf> {
    report::write(&report::dir(), &mut rep).map_err(|e| eprintln!("could not write the report: {e}")).ok()
}

fn run(args: &[String]) -> Done {
    match args.first().map(String::as_str) {
        Some("full") => match disk(args.get(1)) {
            Ok(d) => full(&d, args),
            Err(e) => Err(e).into(),
        },
        Some(action @ ("probe" | "fix")) => match disk(args.get(1)).and_then(|d| Ok((job(action, &d, args)?, d))) {
            Ok((job, d)) => elevated(&d, &job),
            Err(e) => Err(e).into(),
        },
        _ => other(args).into(),
    }
}

fn other(args: &[String]) -> Result<serde_json::Value, String> {
    let cancel = AtomicBool::new(false);
    match args.first().map(String::as_str) {
        Some("list") => Ok(json!(disks::list()?)),
        Some("speed") => {
            let d = disk(args.get(1))?;
            let mb: u64 = args.get(2).and_then(|s| s.parse().ok()).ok_or("size in MB")?;
            let r = bench::run(&mount(&d)?, mb << 20, &cancel, &mut |p| {
                eprintln!("{} {:>5.1}% {:>8.1} MB/s", p.op, p.done as f64 * 100.0 / p.total as f64, p.mbps)
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(json!(r))
        }
        _ => Err(
            "usage: --cli list | speed <disk> <MB> | full <disk> [MB] | probe <disk> | fix <disk> <real> [fs] [label]"
                .into(),
        ),
    }
}

fn full(d: &disks::Disk, args: &[String]) -> Done {
    let started = settings::now();
    let home = net::home_dir();
    let cancel = AtomicBool::new(false);
    let limit = args.get(2).and_then(|s| s.parse::<u64>().ok()).map(|mb| mb << 20);
    let (measured, result) = match mount(d) {
        Err(e) => (None, Err(("errNoVolume", e))),
        Ok(mount) => {
            let mut last = 0u64;
            let mut told_early = false;
            let r = fullcheck::run(&mount, limit, &cancel, &mut |p| {
                if p.early && !told_early {
                    told_early = true;
                    eprintln!("data loss found after {} MB: writing stopped, checking what was written", p.total >> 20);
                }
                if p.done >= last + (256 << 20) || p.done == p.total {
                    last = p.done;
                    eprintln!("{} {} / {} MB", p.op, p.done >> 20, p.total >> 20);
                }
            });
            if let Ok(r) = &r
                && r.stopped_early
                && !told_early
            {
                eprintln!(
                    "writing stopped early after {} MB: samples could not be read, checked what was written",
                    r.tested >> 20
                );
            }
            let result = match &r {
                Ok(o) => Ok(json!(o)),
                Err(fullcheck::Error::Gone) => Err((
                    "errDriveGone",
                    "the drive is gone: a test file is missing (unplugged or remounted)".to_string(),
                )),
                Err(e) => Err((full_key(e), format!("{e:?}"))),
            };
            (Some(r), result)
        }
    };
    let outcome = match &result {
        Ok(_) => report::OutcomePart { key: "ok".into(), ..Default::default() },
        Err((key, text)) => report::OutcomePart { key: (*key).into(), error: Some(text.clone()), ..Default::default() },
    };
    let rep = report::Report::new(report::Kind::Full, d, started, settings::now())
        .full_data(measured.as_ref(), d, &home)
        .ended(outcome, d, &home);
    Done { result: result.map_err(|(_, text)| text), report: write_report(rep) }
}

fn full_key(e: &fullcheck::Error) -> &'static str {
    match e {
        fullcheck::Error::Cancelled => "errCancelled",
        fullcheck::Error::NoSpace(_) => "errNoSpaceFull",
        fullcheck::Error::Io(_) => "errIo",
        fullcheck::Error::Inconclusive => "errFullInconclusive",
        fullcheck::Error::Gone => "errDriveGone",
    }
}

/// `probe` and `fix`: the run of the elevated helper and its report.
fn elevated(d: &disks::Disk, job: &helper::Job) -> Done {
    let started = settings::now();
    let home = net::home_dir();
    let cancel = AtomicBool::new(false);
    let mut last = String::new();
    let ran = helper::run(job, i18n::admin_prompt("en"), &cancel, &mut |f, stage| {
        let line = format!("{stage} {:.0}%", f * 100.0);
        if line != last {
            eprintln!("{line}");
            last = line;
        }
    });
    let rep = if job.action == helper::Action::Probe {
        let judged = crate::judge_probe(&ran.result, &ran.log);
        if let Some(replug) = &judged.replug {
            eprintln!(
                "warning: the result cannot be trusted ({}): unplug the drive, plug it in again and check it again; \
                 do not format, initialize or repair it first",
                replug.reason
            );
        }
        report::Report::new(report::Kind::Probe, d, started, settings::now())
            .probe_data(&ran.log, &ran.result, d, &home)
            .ended(judged.outcome_part(&ran.result), d, &home)
    } else {
        let named = report::with_new_label(d, &helper::clean_label(&job.label));
        let planned = helper::fix_size(job).unwrap_or(job.real);
        let fs = if job.fs == helper::Fs::Fat32 { "fat32" } else { "exfat" };
        let outcome = match &ran.result {
            Ok(_) => report::OutcomePart { key: "ok".into(), ..Default::default() },
            Err(helper::RunError::Declined) => report::OutcomePart { key: "errDeclined".into(), ..Default::default() },
            Err(helper::RunError::Failed { message, cause, .. }) => report::OutcomePart {
                key: "errHelper".into(),
                cause: cause.clone(),
                error: Some(message.clone()),
                ..Default::default()
            },
        };
        report::Report::new(report::Kind::Fix, d, started, settings::now())
            .fix_data(planned, fs, &ran.log, &named, &home)
            .ended(outcome, &named, &home)
    };
    let result = ran.result.map_err(|e| match e {
        helper::RunError::Declined => "administrator access was not given".to_string(),
        helper::RunError::Failed { message, cause, .. } => failure_text(cause.as_deref(), &message),
    });
    Done { result, report: write_report(rep) }
}

/// The job of `--cli probe|fix <disk> <real bytes|0> [exfat|fat32] [LABEL]`: no exact partition size and no layout
/// check. A repair needs the size: a missing or non-numeric one is an error, and only an explicit `0` (or a size that
/// is not below the drive's) is a request for the whole drive.
fn job(action: &str, d: &disks::Disk, args: &[String]) -> Result<helper::Job, String> {
    let fix = action != "probe";
    // the size belongs to the repair only: a quick check never reads it
    let real: u64 = match (fix, args.get(2)) {
        (false, _) => 0,
        (true, Some(s)) => s
            .parse()
            .map_err(|_| format!("the size must be a number of bytes (or 0 for the whole drive), not {s:?}"))?,
        (true, None) => return Err("fix needs the size in bytes, or 0 for the whole drive".into()),
    };
    Ok(helper::Job {
        action: if fix { helper::Action::Fix } else { helper::Action::Probe },
        disk: d.id.clone(),
        size: d.size,
        real,
        partition: 0,
        layout: None,
        // only an explicit whole-drive request confirms a format at the full size
        whole_confirmed: fix && (real == 0 || real >= d.size),
        fs: if args.get(3).is_some_and(|s| s == "fat32") { helper::Fs::Fat32 } else { helper::Fs::Exfat },
        label: args.get(4).cloned().unwrap_or_else(|| "USB".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_error_names_the_cause() {
        assert_eq!(failure_text(Some("privacy"), "Operation not permitted"), "privacy: Operation not permitted");
        assert_eq!(failure_text(None, "the helper stopped without a result"), "the helper stopped without a result");
    }

    #[test]
    fn cli_fix_builds_a_job_without_a_plan_or_layout() {
        let d = disks::Disk {
            id: "disk4".into(),
            device: String::new(),
            name: String::new(),
            kind: "usb".into(),
            bus: "USB".into(),
            size: 1000,
            block_size: 512,
            volumes: Vec::new(),
            partitions: Vec::new(),
            table_unreadable: false,
            table_id: String::new(),
            partition_fs: Vec::new(),
        };
        let args: Vec<String> = ["fix", "disk4", "0", "fat32", "MY"].map(String::from).to_vec();
        let j = job("fix", &d, &args).unwrap();
        assert_eq!((j.partition, j.layout.is_none(), j.real, j.whole_confirmed), (0, true, 0, true));
        assert_eq!((j.fs, j.label.as_str(), j.action), (helper::Fs::Fat32, "MY", helper::Action::Fix));
        let args: Vec<String> = ["fix", "disk4", "8589934592"].map(String::from).to_vec();
        // 8 GiB on a 1000-byte "drive" is not below its size: the whole drive
        assert!(job("fix", &d, &args).unwrap().whole_confirmed);
        let big = disks::Disk { size: 125_829_120_000, ..d.clone() };
        let j = job("fix", &big, &args).unwrap();
        assert_eq!((j.real, j.whole_confirmed), (8_589_934_592, false));
        assert_eq!(job("probe", &d, &args).unwrap().action, helper::Action::Probe);
        // a forgotten or garbled size never erases the whole drive
        let missing: Vec<String> = ["fix", "disk4"].map(String::from).to_vec();
        assert!(job("fix", &big, &missing).is_err());
        let text: Vec<String> = ["fix", "disk4", "8GB"].map(String::from).to_vec();
        assert!(job("fix", &big, &text).is_err());
        assert!(job("probe", &big, &missing).is_ok());
        let zero: Vec<String> = ["fix", "disk4", "0"].map(String::from).to_vec();
        assert!(job("fix", &big, &zero).unwrap().whole_confirmed);
    }
}
