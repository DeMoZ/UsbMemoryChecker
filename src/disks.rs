//! List of removable drives: USB flash drives, memory cards, external disks. Internal and system disks never
//! appear here, so the dangerous actions (raw probe, repartition) cannot reach them.
//!
//! - macOS: `diskutil list -plist` + `diskutil info -plist` for each whole disk.
//! - Windows: PowerShell `Get-Disk` / `Get-Partition` / `Get-Volume` as JSON; only USB / SD / MMC buses,
//!   never the boot or system disk.

use serde::Serialize;

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

    fn diskutil(args: &[&str]) -> Result<Dictionary, String> {
        let out = crate::net::command("/usr/sbin/diskutil")
            .args(args)
            .output()
            .map_err(|e| format!("could not run diskutil: {e}"))?;
        if !out.status.success() {
            return Err(format!("diskutil {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Value::from_reader_xml(&out.stdout[..])
            .map_err(|e| format!("diskutil output: {e}"))?
            .into_dictionary()
            .ok_or_else(|| "diskutil output is not a dictionary".into())
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

    pub fn list() -> Result<Vec<Disk>, String> {
        let all = diskutil(&["list", "-plist"])?;
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
            let Ok(info) = diskutil(&["info", "-plist", &id]) else { continue };
            let virt = s(&info, "VirtualOrPhysical") == "Virtual";
            if virt && !show_virtual() {
                continue;
            }
            let bus = s(&info, "BusProtocol").to_string();
            let removable = b(&info, "RemovableMedia") || b(&info, "Ejectable");
            let external = !b(&info, "Internal");
            if !(external || removable || bus == "Secure Digital" || virt) {
                continue;
            }
            // APFS containers made from another disk are listed as whole disks too: skip them
            if s(&info, "Content").is_empty() && !s(&info, "APFSContainerReference").is_empty() {
                continue;
            }
            let Some(entry) = entries.iter().find(|e| s(e, "DeviceIdentifier") == id) else { continue };
            if !entry.get("APFSPhysicalStores").is_none() {
                continue;
            }
            let mut volumes = Vec::new();
            if let Some(v) = volume(&info) {
                volumes.push(v);
            }
            let mut boot = s(&info, "MountPoint") == "/";
            for part in arr(entry, "Partitions") {
                let pid = s(part, "DeviceIdentifier");
                if !s(part, "MountPoint").is_empty()
                    && let Ok(pinfo) = diskutil(&["info", "-plist", pid])
                {
                    boot |= s(&pinfo, "MountPoint") == "/";
                    volumes.extend(volume(&pinfo));
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
                        if let Ok(vinfo) = diskutil(&["info", "-plist", s(v, "DeviceIdentifier")]) {
                            volumes.extend(volume(&vinfo));
                        }
                    }
                }
            }
            if boot {
                continue;
            }
            let name = match s(&info, "MediaName").trim() {
                "" => s(&info, "IORegistryEntryName").trim().to_string(),
                m => m.to_string(),
            };
            disks.push(Disk {
                device: format!("/dev/r{id}"),
                id,
                name: if name.is_empty() { "Drive".into() } else { name },
                kind: if virt { "other".into() } else { kind_of(&bus).into() },
                bus,
                size: n(&info, "TotalSize"),
                block_size: n(&info, "DeviceBlockSize").max(512),
                volumes,
            });
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

#[cfg(windows)]
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
  foreach ($p in @(Get-Partition -DiskNumber $d.Number)) {
    if (-not $p.DriveLetter -or $p.DriveLetter -eq [char]0) { continue }
    $v = Get-Volume -Partition $p
    $vols += [pscustomobject]@{ letter = [string]$p.DriveLetter; label = [string]$v.FileSystemLabel; fs = [string]$v.FileSystem; size = [uint64]$v.Size; free = [uint64]$v.SizeRemaining }
  }
  $out += [pscustomobject]@{ number = [int]$d.Number; name = [string]$d.FriendlyName; bus = $bus; size = [uint64]$d.Size; sector = [uint64]$d.LogicalSectorSize; readonly = [bool]$d.IsReadOnly; vols = $vols }
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
    }

    /// ConvertTo-Json writes a one-element array as a bare object.
    fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<PsVol>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(PsVol),
            Many(Vec<PsVol>),
            Null(()),
        }
        Ok(match OneOrMany::deserialize(d)? {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
            OneOrMany::Null(()) => Vec::new(),
        })
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

    pub fn list() -> Result<Vec<Disk>, String> {
        let out = crate::net::command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCRIPT])
            .output()
            .map_err(|e| format!("could not run PowerShell: {e}"))?;
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
        };
        assert_eq!(d.main_volume().unwrap().mount, "/c");
    }

    #[cfg(windows)]
    #[test]
    fn powershell_json() {
        let one = r#"[{"number":2,"name":"SanDisk","bus":"USB","size":64000000000,"sector":512,"readonly":false,"vols":{"letter":"E","label":"X","fs":"exFAT","size":63000000000,"free":1000}}]"#;
        let d = win::parse(one).unwrap();
        assert_eq!(d[0].device, r"\\.\PhysicalDrive2");
        assert_eq!(d[0].volumes[0].mount, r"E:\");
        assert!(win::parse("").unwrap().is_empty());
    }
}
