//! List of removable drives: USB flash drives, memory cards, external disks. Internal and system disks never
//! appear here, so the dangerous actions (raw probe, repartition) cannot reach them.
//!
//! - macOS: `diskutil list -plist` + `diskutil info -plist` for each whole disk.
//! - Windows: PowerShell `Get-Disk` / `Get-Partition` / `Get-Volume` as JSON; only USB / SD / MMC buses,
//!   never the boot or system disk.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// Mount point (`/Volumes/NAME`) or drive root (`E:\`).
    pub mount: String,
    pub label: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    pub writable: bool,
}

/// Set in the elevated helper process: it never takes test hooks from the environment.
static ELEVATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_elevated() {
    ELEVATED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// One entry of the partition table, mounted or not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Partition {
    pub offset: u64,
    pub size: u64,
    /// Volume UUID on macOS (empty when the partition has none); always empty on Windows.
    #[serde(default)]
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Disk {
    /// `disk4` on macOS, disk number (`3`) on Windows.
    pub id: String,
    /// Whole device for raw access: `/dev/rdisk4`, `\\.\PhysicalDrive3`.
    pub device: String,
    pub name: String,
    /// `usb`, `sd`, `thunderbolt`, `other`.
    pub kind: String,
    /// Bus name as the system reports it.
    pub bus: String,
    /// Size the drive reports.
    pub size: u64,
    pub block_size: u64,
    pub volumes: Vec<Volume>,
    /// The partition table, also the partitions that are not mounted.
    pub partitions: Vec<Partition>,
    /// The partition table could not be read (a `diskutil info` of a partition failed or timed out, or
    /// `Get-Partition` failed): the drive is listed without partitions, and nothing known about it is confirmed.
    #[serde(skip)]
    pub table_unreadable: bool,
    /// Windows: the MBR disk signature (8 hex digits) or the GPT disk GUID. Empty on macOS.
    pub table_id: String,
}

impl Disk {
    /// Writable mounted volume with the most space: where the speed test and the full check write.
    pub fn main_volume(&self) -> Option<&Volume> {
        self.volumes.iter().filter(|v| v.writable).max_by_key(|v| v.total)
    }
}

fn kind_of(bus: &str) -> &'static str {
    let b = bus.to_ascii_lowercase();
    if b.contains("usb") {
        "usb"
    } else if b.contains("secure digital") || b == "sd" || b == "mmc" || b.contains("sdxc") {
        "sd"
    } else if b.contains("thunderbolt") || b.contains("pci") || b.contains("nvme") {
        "thunderbolt"
    } else {
        "other"
    }
}

/// Disks the probe and the repair may touch: same id and the same reported size as when the user chose it.
pub fn find(id: &str, size: u64) -> Result<Disk, String> {
    let disks = list()?;
    let disk = disks.into_iter().find(|d| d.id == id).ok_or_else(|| format!("drive {id} is not connected"))?;
    if disk.size != size {
        return Err(format!("drive {id} changed (size {} instead of {size}); choose it again", disk.size));
    }
    Ok(disk)
}

#[cfg(target_os = "macos")]
pub use mac::list;
#[cfg(windows)]
pub use win::list;

#[cfg(not(any(target_os = "macos", windows)))]
pub fn list() -> Result<Vec<Disk>, String> {
    Ok(Vec::new())
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use plist::{Dictionary, Value};
    use std::time::Duration;

    /// One `diskutil` call may take this long. A stuck list or whole-disk call fails the scan (the previous list stays)
    /// instead of blocking it; a stuck call for one partition only leaves that drive without a partition table.
    const DISKUTIL_LIMIT: Duration = Duration::from_secs(10);

    /// Why a `diskutil` call failed. For the list and a whole disk a timeout fails the scan and any other failure skips
    /// that disk; a failed call for a partition only leaves its drive without a partition table.
    pub struct Failure {
        pub timed_out: bool,
        pub message: String,
    }

    fn diskutil_path() -> String {
        // test hook of debug builds; never in the elevated helper
        #[cfg(debug_assertions)]
        if !super::ELEVATED.load(std::sync::atomic::Ordering::Relaxed)
            && let Ok(p) = std::env::var("UMC_DISKUTIL")
        {
            return p;
        }
        "/usr/sbin/diskutil".into()
    }

    fn diskutil(args: &[&str]) -> Result<Dictionary, Failure> {
        let fail = |timed_out, message| Failure { timed_out, message };
        let out = crate::net::output_timeout(crate::net::command(&diskutil_path()).args(args), DISKUTIL_LIMIT)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    fail(true, format!("diskutil {} did not answer in {} s", args.join(" "), DISKUTIL_LIMIT.as_secs()))
                } else {
                    fail(false, format!("could not run diskutil: {e}"))
                }
            })?;
        if !out.status.success() {
            return Err(fail(
                false,
                format!("diskutil {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
            ));
        }
        Value::from_reader_xml(&out.stdout[..])
            .map_err(|e| fail(false, format!("diskutil output: {e}")))?
            .into_dictionary()
            .ok_or_else(|| fail(false, "diskutil output is not a dictionary".into()))
    }

    fn s<'a>(d: &'a Dictionary, k: &str) -> &'a str {
        d.get(k).and_then(Value::as_string).unwrap_or("")
    }
    fn b(d: &Dictionary, k: &str) -> bool {
        d.get(k).and_then(Value::as_boolean).unwrap_or(false)
    }
    fn n(d: &Dictionary, k: &str) -> u64 {
        d.get(k).and_then(Value::as_unsigned_integer).unwrap_or(0)
    }
    fn arr<'a>(d: &'a Dictionary, k: &str) -> impl Iterator<Item = &'a Dictionary> {
        d.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_dictionary)
    }

    /// Disk images and RAM disks are shown only with `UMC_SHOW_VIRTUAL=1` (for testing without a real drive).
    fn show_virtual() -> bool {
        std::env::var("UMC_SHOW_VIRTUAL").is_ok_and(|v| v == "1")
    }

    fn volume(info: &Dictionary) -> Option<Volume> {
        let mount = s(info, "MountPoint");
        if mount.is_empty() {
            return None;
        }
        let (free, total) = crate::dio::space(std::path::Path::new(mount)).unwrap_or((0, n(info, "TotalSize")));
        let fs = match s(info, "FilesystemUserVisibleName") {
            "" => s(info, "FilesystemName"),
            v => v,
        };
        Some(Volume {
            mount: mount.to_string(),
            label: s(info, "VolumeName").to_string(),
            fs: fs.to_string(),
            total,
            free,
            writable: b(info, "WritableVolume") || b(info, "Writable"),
        })
    }

    /// A partition of the table: size from the `diskutil list` entry, offset from `diskutil info`, id = the
    /// volume UUID (else the disk UUID, else empty).
    pub fn partition_from(entry: &Dictionary, info: Option<&Dictionary>) -> Partition {
        let from_info = |k| info.map(|i| s(i, k)).unwrap_or("");
        let id = [s(entry, "VolumeUUID"), from_info("VolumeUUID"), from_info("DiskUUID"), s(entry, "DiskUUID")]
            .into_iter()
            .find(|v| !v.is_empty())
            .unwrap_or("");
        let size = match n(entry, "Size") {
            0 => info.map(|i| n(i, "Size")).unwrap_or(0),
            v => v,
        };
        Partition { offset: info.map(|i| n(i, "PartitionMapPartitionOffset")).unwrap_or(0), size, id: id.to_string() }
    }

    /// Builds one listed disk from the parsed `diskutil` output; `info_of` runs `diskutil info -plist <id>`.
    /// `Ok(None)`: not a removable drive (or the boot disk). A partition whose `info` fails or times out only leaves the
    /// drive without a partition table; `Err` is kept for the signature the scan uses (nothing returns it now).
    pub fn build_disk(
        id: &str,
        info: &Dictionary,
        entries: &[&Dictionary],
        virtual_shown: bool,
        info_of: &mut dyn FnMut(&str) -> Result<Dictionary, Failure>,
    ) -> Result<Option<Disk>, String> {
        let virt = s(info, "VirtualOrPhysical") == "Virtual";
        if virt && !virtual_shown {
            return Ok(None);
        }
        let bus = s(info, "BusProtocol").to_string();
        let removable = b(info, "RemovableMedia") || b(info, "Ejectable");
        let external = !b(info, "Internal");
        if !(external || removable || bus == "Secure Digital" || virt) {
            return Ok(None);
        }
        // APFS containers made from another disk are listed as whole disks too: skip them
        if s(info, "Content").is_empty() && !s(info, "APFSContainerReference").is_empty() {
            return Ok(None);
        }
        let Some(entry) = entries.iter().find(|e| s(e, "DeviceIdentifier") == id) else { return Ok(None) };
        if entry.get("APFSPhysicalStores").is_some() {
            return Ok(None);
        }
        // a partition whose `diskutil info` fails or keeps timing out leaves this drive without a partition table;
        // it does not fail the scan of the other drives
        let mut info_of = |pid: &str| -> Result<Option<Dictionary>, String> { Ok(info_of(pid).ok()) };
        let mut volumes = Vec::new();
        let mut partitions = Vec::new();
        // a partition whose `diskutil info` failed has no offset: the whole table is then unreliable
        let mut table_complete = true;
        if let Some(v) = volume(info) {
            volumes.push(v);
        }
        let mut boot = s(info, "MountPoint") == "/";
        for part in arr(entry, "Partitions") {
            let pid = s(part, "DeviceIdentifier");
            let pinfo = info_of(pid)?;
            table_complete &= pinfo.is_some();
            partitions.push(partition_from(part, pinfo.as_ref()));
            if let Some(pinfo) = &pinfo {
                boot |= s(pinfo, "MountPoint") == "/";
                volumes.extend(volume(pinfo));
            }
            // APFS: the volumes live in a container disk made from this partition
            for container in
                entries.iter().filter(|e| arr(e, "APFSPhysicalStores").any(|st| s(st, "DeviceIdentifier") == pid))
            {
                for v in arr(container, "APFSVolumes") {
                    let mount = s(v, "MountPoint");
                    boot |= mount == "/" || mount.starts_with("/System/Volumes");
                    if mount.is_empty() || mount.starts_with("/System/Volumes") {
                        continue;
                    }
                    if let Some(vinfo) = info_of(s(v, "DeviceIdentifier"))? {
                        volumes.extend(volume(&vinfo));
                    }
                }
            }
        }
        if boot {
            return Ok(None);
        }
        if !table_complete {
            // no partition table is better than a wrong one: no limited layout, no record is matched or saved
            partitions.clear();
        }
        let name = match s(info, "MediaName").trim() {
            "" => s(info, "IORegistryEntryName").trim().to_string(),
            m => m.to_string(),
        };
        Ok(Some(Disk {
            device: format!("/dev/r{id}"),
            id: id.to_string(),
            name: if name.is_empty() { "Drive".into() } else { name },
            kind: if virt { "other".into() } else { kind_of(&bus).into() },
            bus,
            size: n(info, "TotalSize"),
            block_size: n(info, "DeviceBlockSize").max(512),
            volumes,
            partitions,
            table_unreadable: !table_complete,
            table_id: String::new(),
        }))
    }

    pub fn list() -> Result<Vec<Disk>, String> {
        let all = diskutil(&["list", "-plist"]).map_err(|f| f.message)?;
        let entries: Vec<&Dictionary> = arr(&all, "AllDisksAndPartitions").collect();
        let whole: Vec<String> = all
            .get("WholeDisks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_string().map(str::to_string))
            .collect();
        let mut disks = Vec::new();
        for id in whole {
            let info = match diskutil(&["info", "-plist", &id]) {
                Ok(i) => i,
                Err(f) if f.timed_out => return Err(f.message),
                Err(_) => continue,
            };
            let mut info_of = |pid: &str| diskutil(&["info", "-plist", pid]);
            disks.extend(build_disk(&id, &info, &entries, show_virtual(), &mut info_of)?);
        }
        Ok(disks)
    }

    /// `disk4` and nothing else may go to diskutil as root.
    pub fn valid_id(id: &str) -> bool {
        id.strip_prefix("disk").is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
    }
}

#[cfg(target_os = "macos")]
pub use mac::valid_id;

#[cfg(windows)]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() < 4 && id.bytes().all(|c| c.is_ascii_digit())
}

#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod win {
    use super::*;
    use serde::Deserialize;

    const SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$out = @()
foreach ($d in Get-Disk) {
  if ($d.IsBoot -or $d.IsSystem) { continue }
  $bus = [string]$d.BusType
  if (@('USB','SD','MMC','1394') -notcontains $bus) { continue }
  $vols = @()
  $pe = $null
  $all = @(Get-Partition -DiskNumber $d.Number -ErrorAction SilentlyContinue -ErrorVariable pe)
  foreach ($p in $all) {
    if (-not $p.DriveLetter -or $p.DriveLetter -eq [char]0) { continue }
    $v = Get-Volume -Partition $p
    $vols += [pscustomobject]@{ letter = [string]$p.DriveLetter; label = [string]$v.FileSystemLabel; fs = [string]$v.FileSystem; size = [uint64]$v.Size; free = [uint64]$v.SizeRemaining }
  }
  $parts = @()
  foreach ($p in $all) {
    $parts += [pscustomobject]@{ offset = [uint64]$p.Offset; size = [uint64]$p.Size }
  }
  $out += [pscustomobject]@{ number = [int]$d.Number; name = [string]$d.FriendlyName; bus = $bus; size = [uint64]$d.Size; sector = [uint64]$d.LogicalSectorSize; readonly = [bool]$d.IsReadOnly; vols = $vols; parts = $parts; partsfail = [bool]((@($pe | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' }).Count -gt 0) -or ($all.Count -eq 0 -and $d.NumberOfPartitions -gt 0)); sig = [uint32]$d.Signature; guid = [string]$d.Guid }
}
ConvertTo-Json -InputObject @($out) -Depth 4 -Compress
"#;

    #[derive(Deserialize)]
    struct PsVol {
        letter: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        fs: String,
        #[serde(default)]
        size: u64,
        #[serde(default)]
        free: u64,
    }

    #[derive(Deserialize)]
    struct PsPart {
        #[serde(default)]
        offset: u64,
        #[serde(default)]
        size: u64,
    }

    #[derive(Deserialize)]
    struct PsDisk {
        number: u32,
        #[serde(default)]
        name: String,
        #[serde(default)]
        bus: String,
        #[serde(default)]
        size: u64,
        #[serde(default)]
        sector: u64,
        #[serde(default)]
        readonly: bool,
        #[serde(default, deserialize_with = "one_or_many")]
        vols: Vec<PsVol>,
        #[serde(default, deserialize_with = "one_or_many")]
        parts: Vec<PsPart>,
        /// `Get-Partition` reported an error for this disk.
        #[serde(default)]
        partsfail: bool,
        /// MBR disk signature (0 for GPT disks).
        #[serde(default)]
        sig: u32,
        /// GPT disk GUID (empty for MBR disks).
        #[serde(default)]
        guid: String,
    }

    /// ConvertTo-Json writes a one-element array as a bare object.
    fn one_or_many<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Deserialize<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany<T> {
            // an array first: a struct whose fields all have defaults would also accept an empty array as one value
            Many(Vec<T>),
            One(T),
            Null(()),
        }
        Ok(match OneOrMany::<T>::deserialize(d)? {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
            OneOrMany::Null(()) => Vec::new(),
        })
    }

    /// The GPT GUID without braces when the disk has one, else the MBR signature as 8 hex digits, else empty.
    fn table_id(guid: &str, sig: u32) -> String {
        let guid = guid.trim().trim_matches(['{', '}']);
        if !guid.is_empty() {
            guid.to_string()
        } else if sig != 0 {
            format!("{sig:08X}")
        } else {
            String::new()
        }
    }

    pub fn parse(json: &str) -> Result<Vec<Disk>, String> {
        let json = json.trim();
        if json.is_empty() {
            return Ok(Vec::new());
        }
        let list: Vec<PsDisk> = serde_json::from_str(json).map_err(|e| format!("disk list: {e}"))?;
        Ok(list
            .into_iter()
            .map(|d| Disk {
                table_id: table_id(&d.guid, d.sig),
                // no partitions although the disk has volumes, or a failed `Get-Partition`: the table is unreadable
                table_unreadable: d.partsfail || (d.parts.is_empty() && !d.vols.is_empty()),
                partitions: d
                    .parts
                    .iter()
                    .map(|p| Partition { offset: p.offset, size: p.size, id: String::new() })
                    .collect(),
                id: d.number.to_string(),
                device: format!(r"\\.\PhysicalDrive{}", d.number),
                name: if d.name.trim().is_empty() { "Drive".into() } else { d.name.trim().to_string() },
                kind: kind_of(&d.bus).into(),
                bus: d.bus,
                size: d.size,
                block_size: d.sector.max(512),
                volumes: d
                    .vols
                    .into_iter()
                    .map(|v| {
                        let mount = format!(r"{}:\", v.letter);
                        let (free, total) = crate::dio::space(std::path::Path::new(&mount)).unwrap_or((v.free, v.size));
                        Volume { mount, label: v.label, fs: v.fs, total, free, writable: !d.readonly }
                    })
                    .collect(),
            })
            .collect())
    }

    #[cfg(windows)]
    pub fn list() -> Result<Vec<Disk>, String> {
        let mut cmd = crate::net::command("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCRIPT]);
        let out = crate::net::output_timeout(&mut cmd, std::time::Duration::from_secs(30)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                "PowerShell did not answer in 30 s".to_string()
            } else {
                format!("could not run PowerShell: {e}")
            }
        })?;
        parse(&String::from_utf8_lossy(&out.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_kinds() {
        assert_eq!(kind_of("USB"), "usb");
        assert_eq!(kind_of("Secure Digital"), "sd");
        assert_eq!(kind_of("SD"), "sd");
        assert_eq!(kind_of("PCI-Express"), "thunderbolt");
        assert_eq!(kind_of("SATA"), "other");
    }

    #[test]
    fn ids_for_root_commands() {
        assert!(valid_id(if cfg!(windows) { "3" } else { "disk4" }));
        assert!(!valid_id("disk4s1"));
        assert!(!valid_id("disk4; rm -rf /"));
        assert!(!valid_id(""));
    }

    #[test]
    fn main_volume_is_the_biggest_writable() {
        let v = |mount: &str, total, writable| Volume {
            mount: mount.into(),
            label: String::new(),
            fs: String::new(),
            total,
            free: 0,
            writable,
        };
        let d = Disk {
            id: "disk9".into(),
            device: String::new(),
            name: String::new(),
            kind: "usb".into(),
            bus: "USB".into(),
            size: 0,
            block_size: 512,
            volumes: vec![v("/a", 10, true), v("/b", 30, false), v("/c", 20, true)],
            partitions: Vec::new(),
            table_unreadable: false,
            table_id: String::new(),
        };
        assert_eq!(d.main_volume().unwrap().mount, "/c");
    }

    #[test]
    fn powershell_json() {
        let one = r#"[{"number":2,"name":"SanDisk","bus":"USB","size":64000000000,"sector":512,"readonly":false,"vols":{"letter":"E","label":"X","fs":"exFAT","size":63000000000,"free":1000}}]"#;
        let d = win::parse(one).unwrap();
        assert_eq!(d[0].device, r"\\.\PhysicalDrive2");
        assert_eq!(d[0].volumes[0].mount, r"E:\");
        assert!(d[0].partitions.is_empty());
        assert_eq!(d[0].table_id, "");
        assert!(win::parse("").unwrap().is_empty());
    }

    #[test]
    fn powershell_repaired_mbr_disk() {
        let json = r#"[{"number":3,"name":"Storage Device","bus":"USB","size":125829120000,"sector":512,"readonly":false,"vols":null,"parts":{"offset":1048576,"size":8303673344},"sig":439041101,"guid":""}]"#;
        let d = win::parse(json).unwrap();
        assert_eq!(d[0].table_id, "1A2B3C4D");
        assert!(d[0].volumes.is_empty());
        assert_eq!(d[0].partitions, vec![Partition { offset: 1_048_576, size: 8_303_673_344, id: String::new() }]);
    }

    #[test]
    fn powershell_unreadable_table_is_flagged() {
        // Get-Partition failed (partsfail), or the disk has volumes but no partitions were returned
        let failed = r#"[{"number":1,"bus":"USB","size":1000,"vols":[],"parts":[],"partsfail":true}]"#;
        assert!(win::parse(failed).unwrap()[0].table_unreadable);
        let lost = r#"[{"number":1,"bus":"USB","size":1000,"vols":{"letter":"E","size":1,"free":1},"parts":[]}]"#;
        assert!(win::parse(lost).unwrap()[0].table_unreadable);
        // a blank disk (RAW, after `clean`, a new card): Get-Partition says "not found", which is not a failure
        let blank = r#"[{"number":1,"bus":"USB","size":1000,"vols":[],"parts":[],"partsfail":false}]"#;
        assert!(!win::parse(blank).unwrap()[0].table_unreadable);
        let blank_no_field = r#"[{"number":1,"bus":"USB","size":1000,"vols":[],"parts":[]}]"#;
        assert!(!win::parse(blank_no_field).unwrap()[0].table_unreadable);
        let ok = r#"[{"number":1,"bus":"USB","size":1000,"vols":[],"parts":{"offset":1048576,"size":100}}]"#;
        assert!(!win::parse(ok).unwrap()[0].table_unreadable);
    }

    #[test]
    fn powershell_gpt_guid() {
        let json = r#"[{"number":4,"bus":"USB","size":1000,"sector":512,"vols":[],"parts":[{"offset":1,"size":2}],"sig":0,"guid":"{0F1E2D3C-0000-4000-8000-00000000ABCD}"}]"#;
        let d = win::parse(json).unwrap();
        assert_eq!(d[0].table_id, "0F1E2D3C-0000-4000-8000-00000000ABCD");
    }

    #[test]
    fn powershell_parts_one_or_many() {
        let many =
            r#"[{"number":1,"bus":"USB","parts":[{"offset":1048576,"size":100},{"offset":2097152,"size":200}]}]"#;
        let one = r#"[{"number":1,"bus":"USB","parts":{"offset":1048576,"size":100}}]"#;
        assert_eq!(win::parse(many).unwrap()[0].partitions.len(), 2);
        assert_eq!(win::parse(one).unwrap()[0].partitions.len(), 1);
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::super::mac::*;
        use super::super::*;
        use plist::{Dictionary, Value};

        fn load(text: &str) -> Dictionary {
            Value::from_reader_xml(text.as_bytes()).unwrap().into_dictionary().unwrap()
        }

        fn fixtures() -> (Dictionary, Dictionary, Dictionary) {
            (
                load(include_str!("testdata/disk6-list.plist")),
                load(include_str!("testdata/disk6-info.plist")),
                load(include_str!("testdata/disk6s1-info.plist")),
            )
        }

        fn entries(list: &Dictionary) -> Vec<&Dictionary> {
            list.get("AllDisksAndPartitions")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_dictionary)
                .collect()
        }

        #[test]
        fn parses_repaired_disk6_fixture() {
            let (list, info, pinfo) = fixtures();
            let mut info_of = |_: &str| Ok(pinfo.clone());
            let d = build_disk("disk6", &info, &entries(&list), false, &mut info_of).ok().flatten().unwrap();
            assert_eq!(d.size, 125_829_120_000);
            assert_eq!(
                d.partitions,
                vec![Partition {
                    offset: 1_048_576,
                    size: 8_303_673_344,
                    id: "00000000-0000-4000-8000-000000000001".into()
                }]
            );
            assert_eq!(d.table_id, "");
            assert_eq!(d.volumes.len(), 1);
        }

        #[test]
        fn failed_partition_info_leaves_no_table() {
            let (list, info, _) = fixtures();
            let mut info_of =
                |_: &str| Err(Failure { timed_out: false, message: "could not get disk info".to_string() });
            let d = build_disk("disk6", &info, &entries(&list), false, &mut info_of).ok().flatten().unwrap();
            assert!(d.partitions.is_empty());
            assert!(d.table_unreadable, "an unreadable table is flagged, so nothing known is confirmed");
        }

        #[test]
        fn timed_out_partition_info_lists_the_drive_without_a_table() {
            let (list, info, _) = fixtures();
            let mut info_of = |_: &str| Err(Failure { timed_out: true, message: "slow".to_string() });
            let d = build_disk("disk6", &info, &entries(&list), false, &mut info_of).ok().flatten().unwrap();
            assert!(d.partitions.is_empty(), "no table, so no record is matched or saved");
            assert!(d.table_unreadable);
            assert_eq!(d.size, 125_829_120_000);
        }

        #[test]
        fn unmounted_partition_still_listed() {
            let (mut list, info, mut pinfo) = fixtures();
            // the same entry without a mount point, and info without one too
            let all = list.get_mut("AllDisksAndPartitions").unwrap().as_array_mut().unwrap();
            let part = all[0].as_dictionary_mut().unwrap().get_mut("Partitions").unwrap().as_array_mut().unwrap();
            part[0].as_dictionary_mut().unwrap().remove("MountPoint");
            pinfo.insert("MountPoint".into(), Value::String(String::new()));
            let mut info_of = |_: &str| Ok(pinfo.clone());
            let d = build_disk("disk6", &info, &entries(&list), false, &mut info_of).ok().flatten().unwrap();
            assert_eq!(d.partitions.len(), 1);
            assert_eq!(d.partitions[0].offset, 1_048_576);
            assert!(d.volumes.is_empty());
        }
    }
}
