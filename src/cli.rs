//! Command line without a window, for scripts and for checking the engines on a real drive:
//!
//! ```text
//! usbmemorycheker --cli list
//! usbmemorycheker --cli speed <disk> <MB>
//! usbmemorycheker --cli full <disk> [limit MB]
//! usbmemorycheker --cli probe <disk>        (asks for administrator rights like the window does)
//! usbmemorycheker --cli fix <disk> <real bytes|0> [exfat|fat32] [LABEL]
//! ```
//!
//! Prints JSON results to stdout, progress to stderr.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde_json::json;

use crate::{bench, disks, fullcheck, helper, i18n};

fn disk(id: Option<&String>) -> Result<disks::Disk, String> {
    let id = id.ok_or("no drive id")?;
    disks::list()?.into_iter().find(|d| &d.id == id).ok_or_else(|| format!("drive {id} not found"))
}

fn mount(d: &disks::Disk) -> Result<PathBuf, String> {
    d.main_volume().map(|v| PathBuf::from(&v.mount)).ok_or_else(|| "the drive has no mounted volume".into())
}

pub fn main(args: &[String]) -> i32 {
    match run(args) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn run(args: &[String]) -> Result<serde_json::Value, String> {
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
        Some("full") => {
            let d = disk(args.get(1))?;
            let limit = args.get(2).and_then(|s| s.parse::<u64>().ok()).map(|mb| mb << 20);
            let mut last = 0u64;
            let r = fullcheck::run(&mount(&d)?, limit, &cancel, &mut |p| {
                if p.done >= last + (256 << 20) || p.done == p.total {
                    last = p.done;
                    eprintln!("{} {} / {} MB", p.op, p.done >> 20, p.total >> 20);
                }
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(json!(r))
        }
        Some(action @ ("probe" | "fix")) => {
            let d = disk(args.get(1))?;
            let job = helper::Job {
                action: if action == "probe" { helper::Action::Probe } else { helper::Action::Fix },
                disk: d.id.clone(),
                size: d.size,
                real: args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
                fs: if args.get(3).is_some_and(|s| s == "fat32") { helper::Fs::Fat32 } else { helper::Fs::Exfat },
                label: args.get(4).cloned().unwrap_or_else(|| "USB".into()),
            };
            let mut last = String::new();
            helper::run(&job, i18n::admin_prompt("en"), &cancel, &mut |f, stage| {
                let line = format!("{stage} {:.0}%", f * 100.0);
                if line != last {
                    eprintln!("{line}");
                    last = line;
                }
            })
            .map_err(|e| match e {
                helper::RunError::Declined => "administrator access was not given".to_string(),
                helper::RunError::Failed(e) => e,
            })
        }
        _ => Err(
            "usage: --cli list | speed <disk> <MB> | full <disk> [MB] | probe <disk> | fix <disk> <real> [fs] [label]"
                .into(),
        ),
    }
}
