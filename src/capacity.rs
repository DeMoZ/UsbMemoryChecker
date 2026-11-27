//! What the application knows about the real capacity of each drive, in one place.
//!
//! - [`Knowledge`]: the declared size, the measured data loss, the last check and the last repair of one drive.
//! - [`view`]: the state shown in the window (not checked, genuine, fake, repaired, ...) and the repair plan,
//!   worked out from the knowledge and the current partition layout. The window only displays it.
//! - [`Store`]: `capacity.json` in the config folder. A record is found again by the declared size plus the
//!   partition layout ([`Fingerprint`]), because the `diskN` number changes when a drive is plugged in again.
//! - [`sync`]: keeps the in-memory entries and the store in step with the list of connected drives.
//!
//! Everything here is plain data and pure functions, so it is tested without a drive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::disks::{Disk, Partition};
use crate::helper;

pub const MIB: u64 = 1 << 20;
/// Most records kept in `capacity.json`; the ones not seen for the longest time are dropped.
const MAX_RECORDS: usize = 100;

/// How much of the drive may stay outside every partition before the layout counts as limited: the larger of
/// 64 MiB and 10% of the declared size (a genuine drive formatted by any system leaves only a few MiB).
pub fn limit_margin(declared: u64) -> u64 {
    (64 * MIB).max(declared / 10)
}

/// Partition size the repair gives a drive that keeps `real` bytes. `helper::partition_size(0, ..)` means "the
/// whole drive", but a drive whose real size is 0 keeps nothing, so 0 stays 0 here.
pub fn loss_repair_size(real: u64, declared: u64) -> u64 {
    if real == 0 { 0 } else { helper::partition_size(real, declared) }
}

/// End of the last partition (0 without partitions).
fn layout_end(disk: &Disk) -> u64 {
    disk.partitions.iter().map(|p| p.offset.saturating_add(p.size)).max().unwrap_or(0)
}

fn largest_partition(disk: &Disk) -> u64 {
    disk.partitions.iter().map(|p| p.size).max().unwrap_or(0)
}

/// Size of the largest partition when the partitions end well before the declared size (a repaired fake drive,
/// or a card with a small partition); None for a normal layout and for a drive without a partition table.
pub fn layout_limit(disk: &Disk) -> Option<u64> {
    if disk.partitions.is_empty() || disk.size.saturating_sub(layout_end(disk)) < limit_margin(disk.size) {
        return None;
    }
    disk.partitions.iter().map(|p| p.size).max()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Quick,
    Full,
}

/// What a check covered: the whole device, or only a partition much smaller than the drive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    #[default]
    Device,
    Partition,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    #[default]
    Genuine,
    Fake,
    Damaged,
    Broken,
}

/// Data the drive loses: `real` bytes from the start keep data (for a full check already 1% below the estimate).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Loss {
    pub real: u64,
    pub verdict: Verdict,
}

/// One finished check with everything the verdict panel shows, so it looks the same after a restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckRecord {
    pub mode: Mode,
    pub scope: Scope,
    pub verdict: Verdict,
    pub real: u64,
    pub declared: u64,
    /// Size of the checked volume (full check only).
    pub volume: u64,
    pub tested: u64,
    pub bad: u64,
    pub stopped_early: bool,
    pub write_error: Option<String>,
    pub points: usize,
    pub seconds: f64,
    pub restore_failures: usize,
    /// The full check ended before it covered the volume (a write error, or clearly less written than the free
    /// space): its pass says nothing about the rest, and it never counts as "verified".
    pub cut_short: bool,
    /// The volume was nearly full before the full check: the test could only write the little free space, which a
    /// wrap-around fake may map onto the cells of the files already there. Such a pass proves nothing about the drive.
    pub nearly_full: bool,
    /// Unix time (seconds) when the check ended.
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RepairRecord {
    pub partition: u64,
    pub at: u64,
}

/// A repair as the knowledge sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repair {
    Partition(u64),
    Whole,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Knowledge {
    pub declared: u64,
    pub loss: Option<Loss>,
    pub last: Option<CheckRecord>,
    pub repair: Option<RepairRecord>,
}

impl Knowledge {
    pub fn new(declared: u64) -> Self {
        Knowledge { declared, loss: None, last: None, repair: None }
    }

    /// A quick check looks at the whole device: its result replaces the loss.
    pub fn apply_quick(&mut self, rec: CheckRecord) {
        self.loss = (rec.verdict != Verdict::Genuine).then_some(Loss { real: rec.real, verdict: rec.verdict });
        self.last = Some(rec);
    }

    /// A full check of the whole device replaces the loss. A full check of a partition only adds to it: a
    /// loss takes the smaller of the old and the new one, a pass leaves it alone.
    pub fn apply_full(&mut self, rec: CheckRecord) {
        let found = (rec.verdict != Verdict::Genuine).then_some(Loss { real: rec.real, verdict: rec.verdict });
        match rec.scope {
            Scope::Device => self.loss = found,
            Scope::Partition => {
                if let Some(new) = found {
                    self.loss = match self.loss.take() {
                        Some(old) if old.real <= new.real => Some(old),
                        _ => Some(new),
                    };
                }
            }
        }
        self.last = Some(rec);
    }

    /// A repair keeps what is known and adds the new partition. A repair at the full declared size drops the
    /// repair entry (the loss stays, so the drive shows as fake again).
    pub fn apply_repair(&mut self, repair: Repair, now: u64) {
        self.repair = match repair {
            Repair::Partition(partition) => Some(RepairRecord { partition, at: now }),
            Repair::Whole => None,
        };
    }
}

/// What the full check measured (the parts of `fullcheck::Outcome` the record needs).
#[derive(Clone, Debug, Default)]
pub struct FullResult {
    /// Size of the checked volume.
    pub total: u64,
    /// Bytes that were in use before the test.
    pub used_before: u64,
    pub tested: u64,
    pub bad: u64,
    pub real_estimate: u64,
    pub stopped_early: bool,
    pub write_error: Option<String>,
    pub seconds: f64,
}

/// The test normally covers the whole free space but its reserve (8 MiB); more missing than this means the test
/// ended early.
const FULL_SLACK: u64 = 16 * MIB;
/// The unused tail a whole-device layout may have: a few MiB (alignment, a backup partition table).
const TAIL_SLACK: u64 = 16 * MIB;

/// The test ended before it covered the volume (a write error, or clearly less than the free space was written):
/// a pass then says nothing about the rest of the drive.
fn is_partial(r: &FullResult) -> bool {
    r.write_error.is_some() || r.total.saturating_sub(r.used_before.saturating_add(r.tested)) > FULL_SLACK
}

/// What a full check of a volume of `volume` bytes covers. Only a volume that fills a layout which itself ends within
/// a few MiB of the declared size covers the whole device: the volume may miss at most max(64 MiB, 2%) of the drive.
/// Anything smaller covers itself only, and so does a volume within a known loss and any volume of a drive our
/// repair made smaller: a check of a partition must never speak for the whole declared size.
pub fn full_scope(k: Option<&Knowledge>, disk: &Disk, volume: u64, used_before: u64) -> Scope {
    let declared = disk.size;
    let known_limit =
        k.is_some_and(|k| k.repair.is_some() || k.loss.as_ref().is_some_and(|l| volume <= l.real.saturating_add(MIB)));
    let fills_volume = declared.saturating_sub(volume) <= (64 * MIB).max(declared / 50);
    let fills_layout = disk.partitions.is_empty() || declared.saturating_sub(layout_end(disk)) <= TAIL_SLACK;
    if known_limit || !fills_volume || !fills_layout || nearly_full(declared, used_before) {
        Scope::Partition
    } else {
        Scope::Device
    }
}

/// More than max(64 MiB, 2%) of the drive was in use before the full check: it only wrote the little free space.
pub fn nearly_full(declared: u64, used_before: u64) -> bool {
    used_before > (64 * MIB).max(declared / 50)
}

/// A test that ended by a write error (or wrote clearly less than the free space) after writing less than a MiB tells
/// nothing: the check is reported as failed and the record stays as it is.
pub fn wrote_too_little(r: &FullResult) -> bool {
    is_partial(r) && r.bad == 0 && r.tested < MIB
}

/// The record of a finished full check. A bad block means the drive loses data: the estimate (capped at the
/// declared size) minus 1% is the real size for the repair, and a size the repair cannot use means "keeps nothing".
/// A test that ended early (write error, less written than the free space) never counts as a check of the whole
/// device: its pass covers what was written (`real` = used before + tested), so it cannot clear or raise a known loss.
pub fn full_record(k: Option<&Knowledge>, disk: &Disk, r: &FullResult, now: u64) -> CheckRecord {
    let declared = disk.size;
    let partial = is_partial(r);
    let scope = if partial { Scope::Partition } else { full_scope(k, disk, r.total, r.used_before) };
    let real = if r.bad > 0 {
        r.real_estimate.min(declared) / 100 * 99
    } else if partial {
        r.used_before.saturating_add(r.tested).min(r.total)
    } else if scope == Scope::Partition {
        r.total
    } else {
        declared
    };
    let verdict = if r.bad == 0 {
        Verdict::Genuine
    } else if loss_repair_size(real, declared) == 0 {
        Verdict::Broken
    } else {
        Verdict::Fake
    };
    CheckRecord {
        mode: Mode::Full,
        scope,
        verdict,
        real,
        declared,
        volume: r.total,
        tested: r.tested,
        bad: r.bad,
        stopped_early: r.stopped_early,
        write_error: r.write_error.clone(),
        seconds: r.seconds,
        cut_short: partial,
        nearly_full: nearly_full(declared, r.used_before),
        at: now,
        ..Default::default()
    }
}

// ------------------------------------------------------------------ the view

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Unknown,
    Genuine,
    Fake,
    Damaged,
    /// The drive keeps no usable data: repair size 0.
    Broken,
    Repaired,
    PartitionOk,
    Limited,
    /// What was known belongs to another layout than the one the drive has now (another drive took this `diskN`,
    /// or the layout cannot be read): shown as unconfirmed, no plan comes from it, a new check is asked for.
    Unconfirmed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Unknown => "unknown",
            State::Genuine => "genuine",
            State::Fake => "fake",
            State::Damaged => "damaged",
            State::Broken => "broken",
            State::Repaired => "repaired",
            State::PartitionOk => "partitionOk",
            State::Limited => "limited",
            State::Unconfirmed => "unconfirmed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanDefault {
    Limit,
    Whole,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairPlan {
    pub default: PlanDefault,
    /// Partition size of the limited plan (a whole number of MiB).
    pub limit: Option<u64>,
    /// The whole declared size.
    pub whole: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub state: State,
    /// Repaired and the partition passed the full check.
    pub verified: bool,
    /// The number for the "Real capacity" box.
    pub value: u64,
    pub declared: u64,
    /// The limit of a limited layout (its largest partition).
    pub partition: Option<u64>,
    /// Key of the drive's layout (see [`Fingerprint::key`]).
    pub layout_key: String,
    /// The real size of the recorded loss.
    pub loss_real: Option<u64>,
    /// Declared size minus the end of the last partition (0 without partitions).
    pub unused: u64,
    pub check: Option<CheckRecord>,
    /// The last check was made before this run of the program.
    pub remembered: bool,
    pub repair: RepairPlan,
}

pub fn view(k: Option<&Knowledge>, disk: &Disk, session_start: u64) -> View {
    let declared = disk.size;
    let limit = layout_limit(disk);
    let end = layout_end(disk);
    let loss = k.and_then(|k| k.loss.as_ref());
    let last = k.and_then(|k| k.last.as_ref());
    let repair_size = loss.map(|l| loss_repair_size(l.real, declared));
    let partition_pass =
        last.is_some_and(|c| c.scope == Scope::Partition && c.verdict == Verdict::Genuine && !c.nearly_full);
    let device_pass = last.is_some_and(|c| c.scope == Scope::Device && c.verdict == Verdict::Genuine);

    let state = match loss {
        Some(_) if repair_size == Some(0) => State::Broken,
        // our own repair leaves partitions ending at the loss; with a repair record this also holds when the
        // unused tail is smaller than the limit margin (a fake that keeps 95% of its declared size)
        Some(l)
            if (limit.is_some() || k.is_some_and(|k| k.repair.is_some()))
                && !disk.partitions.is_empty()
                && end <= l.real.saturating_add(MIB) =>
        {
            State::Repaired
        }
        Some(l) if l.verdict == Verdict::Damaged => State::Damaged,
        Some(_) => State::Fake,
        None if device_pass => State::Genuine,
        None if partition_pass => State::PartitionOk,
        None if limit.is_some() => State::Limited,
        None => State::Unknown,
    };
    let value = match state {
        State::Unknown | State::Broken | State::Unconfirmed => 0,
        State::Genuine => declared,
        State::Fake | State::Damaged => loss.map_or(0, |l| l.real),
        State::Repaired | State::Limited => limit.unwrap_or_else(|| largest_partition(disk)),
        // a cut-short pass verified only what was written
        State::PartitionOk => {
            let real = last.map_or(0, |c| c.real);
            limit.map_or(real, |l| l.min(real))
        }
    };
    let whole = declared;
    // A computed size of 0 (or one that is not below the declared size) never becomes the full size: that would
    // format a drive nobody verified at its declared size. Without a usable size there is no plan at all, and the
    // window asks for a check; the full size stays available as an explicit choice.
    let no_plan = RepairPlan { default: PlanDefault::None, limit: None, whole };
    let limit_plan = |size: u64| {
        if size > 0 && size < declared {
            RepairPlan { default: PlanDefault::Limit, limit: Some(size), whole }
        } else {
            no_plan.clone()
        }
    };
    let layout_size = limit.unwrap_or(0) / MIB * MIB;
    let repair = match state {
        State::Broken => RepairPlan { default: PlanDefault::None, limit: None, whole },
        State::Fake | State::Damaged | State::Repaired => limit_plan(repair_size.unwrap_or(0)),
        State::Limited => limit_plan(layout_size),
        // a pass that verified less than a MiB falls back to the size of the layout's limit, else no plan
        State::PartitionOk => limit_plan(match value / MIB * MIB {
            0 => layout_size,
            n => n,
        }),
        // the full declared size is a default only for a drive that a whole-device check found genuine (and a full
        // check of a nearly empty volume); everything else needs the explicit choice
        State::Genuine => RepairPlan { default: PlanDefault::Whole, limit: None, whole },
        State::Unknown => no_plan.clone(),
        State::Unconfirmed => no_plan.clone(),
    };
    View {
        state,
        // verified: a passing partition check made after the repair (an older one speaks about the old layout)
        verified: state == State::Repaired
            && partition_pass
            && last.is_some_and(|c| !c.cut_short && k.and_then(|k| k.repair.as_ref()).is_none_or(|r| c.at >= r.at)),
        loss_real: loss.map(|l| l.real),
        layout_key: Fingerprint::of(disk).key(),
        value,
        declared,
        partition: limit,
        unused: if disk.partitions.is_empty() { 0 } else { declared.saturating_sub(end) },
        check: last.cloned(),
        remembered: last.is_some_and(|c| c.at < session_start),
        repair,
    }
}

/// Checks the repair request of the window against the plan Rust works out itself. Ok is the exact partition
/// size for `Job.partition` (0 = the whole drive); Err is the window's error key. A drive that keeps nothing is
/// refused whatever the plan says, and so is one whose knowledge is unconfirmed. `explicit`: the user ticked "use
/// the full declared size" in the state shown.
pub fn check_fix(view: &View, plan: &str, size: u64, explicit: Option<Shown>) -> Result<u64, &'static str> {
    match view.state {
        State::Broken => return Err("errBroken"),
        State::Unconfirmed => return Err("errUnconfirmed"),
        _ => {}
    }
    match plan {
        "limit" => match view.repair.limit {
            Some(limit) if limit == size => Ok(limit),
            _ if view.repair.default == PlanDefault::None => Err("errNoPlan"),
            _ => Err("errRepairChanged"),
        },
        // the full declared size is a default only for a drive a whole-device check found genuine; in every other
        // state it needs the explicit choice, made in the very state the window showed
        "whole" if size == view.declared => match explicit {
            _ if view.repair.default == PlanDefault::Whole => Ok(0),
            Some(shown) if shown.state == view.state.as_str() && shown.limit == view.repair.limit => Ok(0),
            _ if view.repair.default == PlanDefault::None => Err("errNoPlan"),
            _ => Err("errRepairChanged"),
        },
        _ => Err("errRepairChanged"),
    }
}

/// The state and limit the window showed when the user ticked "use the full declared size".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown<'a> {
    pub state: &'a str,
    pub limit: Option<u64>,
}

// ------------------------------------------------------------------ persistence

/// How a record is found again: the declared size plus the partition table (offset, size and the volume UUID on
/// macOS; the disk signature or GUID on Windows). The `diskN` number is not part of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub declared: u64,
    #[serde(default)]
    pub table_id: String,
    pub partitions: Vec<Partition>,
}

impl Fingerprint {
    pub fn of(disk: &Disk) -> Self {
        let mut partitions = disk.partitions.clone();
        partitions.sort_by_key(|p| (p.offset, p.size));
        Fingerprint { declared: disk.size, table_id: disk.table_id.clone(), partitions }
    }

    /// A short key of this layout (a hash): the window sends back the key of the layout it showed, and a repair is
    /// refused when the drive has another layout by now.
    pub fn key(&self) -> String {
        use sha2::{Digest, Sha256};
        let text = serde_json::to_string(self).unwrap_or_default();
        Sha256::digest(text.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    /// Only a layout with an id (volume UUID or table id) identifies a drive; one without never matches.
    pub fn usable(&self) -> bool {
        !self.partitions.is_empty() && (!self.table_id.is_empty() || self.partitions.iter().any(|p| !p.id.is_empty()))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub fingerprint: Fingerprint,
    #[serde(default)]
    pub knowledge: Knowledge,
    /// Unix time the drive was last seen with this layout (changes on every lookup).
    #[serde(default)]
    pub seen: u64,
    /// Unix time the content of the record was last written: the merge of two instances goes by this.
    #[serde(default)]
    pub changed: u64,
    /// The key was made by our own repair: the new volume has a fresh random serial, so no other drive shares it.
    /// Only such records are matched when a drive is connected (a factory image gives same-batch fakes one key).
    #[serde(default)]
    pub from_repair: bool,
}

/// A key that was removed (the record moved to a new layout or was evicted), so a merge does not bring it back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tombstone {
    pub fingerprint: Fingerprint,
    pub at: u64,
}

const MAX_TOMBSTONES: usize = 100;

/// `capacity.json`. Records are parsed one by one: a record this version cannot read (damaged, or written by a
/// newer version) is kept as raw JSON in `foreign` and written back unchanged, never dropped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Store {
    pub drives: Vec<Record>,
    pub foreign: Vec<serde_json::Value>,
    /// Removed keys (merged between instances).
    tombstones: Vec<Tombstone>,
    /// Where the store is saved. None (a store made by a test): never written, so no test touches the real config
    /// folder.
    path: Option<PathBuf>,
}

/// A save of the store, run outside the lock of the program state (writing flushes the file to disk).
pub struct SaveJob {
    order: u64,
    path: PathBuf,
    store: Store,
}

static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_gen() -> u64 {
    GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// The newest save generation that has been written.
static WRITTEN_GEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

impl SaveJob {
    /// Writes the file. Jobs are ordered by generation, so an older one never overwrites a newer one.
    pub fn run(self) {
        let mut written = WRITTEN_GEN.lock().unwrap_or_else(|e| e.into_inner());
        if self.order <= *written {
            return;
        }
        *written = self.order;
        if let Err(e) = self.store.save_to(&self.path) {
            eprintln!("could not save capacity.json: {e}");
        }
    }
}

impl Store {
    /// `capacity.json` next to the settings.
    pub fn default_path() -> PathBuf {
        crate::net::config_dir().join("capacity.json")
    }

    /// Loads the file and remembers the path for later saves.
    pub fn open(path: PathBuf) -> Store {
        let mut store = Store::load_from(&path);
        store.path = Some(path);
        store
    }

    /// A missing, unreadable or invalid file gives an empty store; one bad record does not lose the others.
    pub fn load_from(path: &Path) -> Store {
        let Some(root) = std::fs::read(path).ok().and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok())
        else {
            return Store::default();
        };
        let mut store = Store::default();
        for v in root.get("drives").and_then(|d| d.as_array()).into_iter().flatten() {
            match serde_json::from_value::<Record>(v.clone()) {
                Ok(r) => store.drives.push(r),
                Err(_) => store.foreign.push(v.clone()),
            }
        }
        for v in root.get("removed").and_then(|d| d.as_array()).into_iter().flatten() {
            if let Ok(t) = serde_json::from_value::<Tombstone>(v.clone()) {
                store.tombstones.push(t);
            }
        }
        store
    }

    fn tombstone(&mut self, fingerprint: Fingerprint, at: u64) {
        self.tombstones.retain(|t| t.fingerprint != fingerprint);
        self.tombstones.push(Tombstone { fingerprint, at });
        if self.tombstones.len() > MAX_TOMBSTONES {
            self.tombstones.sort_by_key(|t| std::cmp::Reverse(t.at));
            self.tombstones.truncate(MAX_TOMBSTONES);
        }
    }

    /// Writes a temporary file (flushed to disk) and renames it, so a crash never leaves half a file. Another
    /// instance may have written the file meanwhile: it is read again and merged by key first. For a record both
    /// have, the one written last (`changed`) wins; a record only the file has is kept unless a removal of its key
    /// (in either instance) is newer than the record.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let on_disk = Store::load_from(path);
        let mut tombstones = self.tombstones.clone();
        for t in &on_disk.tombstones {
            match tombstones.iter_mut().find(|m| m.fingerprint == t.fingerprint) {
                Some(m) => m.at = m.at.max(t.at),
                None => tombstones.push(t.clone()),
            }
        }
        let removed_after = |r: &Record| tombstones.iter().any(|t| t.fingerprint == r.fingerprint && t.at >= r.changed);
        let mut merged: Vec<Record> = Vec::with_capacity(self.drives.len());
        for mine in &self.drives {
            match on_disk.drives.iter().find(|r| r.fingerprint == mine.fingerprint) {
                Some(theirs) if theirs.changed > mine.changed => merged.push(theirs.clone()),
                // the other instance removed this key after our last change to it
                None if removed_after(mine) => {}
                _ => merged.push(mine.clone()),
            }
        }
        for theirs in &on_disk.drives {
            if !self.drives.iter().any(|r| r.fingerprint == theirs.fingerprint) && !removed_after(theirs) {
                merged.push(theirs.clone());
            }
        }
        let mut foreign = self.foreign.clone();
        for f in &on_disk.foreign {
            if !foreign.contains(f) {
                foreign.push(f.clone());
            }
        }
        let mut drives: Vec<serde_json::Value> = Vec::with_capacity(merged.len() + foreign.len());
        for r in &merged {
            drives.push(serde_json::to_value(r).map_err(std::io::Error::other)?);
        }
        drives.extend(foreign);
        let doc = serde_json::json!({"version": 1, "drives": drives, "removed": tombstones});
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    }

    /// A copy of the store to write outside the state lock; None for a store without a path.
    pub fn save_job(&self) -> Option<SaveJob> {
        let path = self.path.clone()?;
        let mut store = self.clone();
        store.path = None;
        // the generation is taken under the lock of the program state, so it follows the order of the changes
        Some(SaveJob { order: next_gen(), path, store })
    }

    /// The knowledge of a drive with exactly this layout, if the key was made by our own repair; marks the record
    /// as seen. A record whose knowledge is about another declared size is never used.
    pub fn find(&mut self, fp: &Fingerprint, now: u64) -> Option<Knowledge> {
        if !fp.usable() {
            return None;
        }
        let rec = self.drives.iter_mut().find(|r| {
            r.from_repair
                    && &r.fingerprint == fp
                    && r.knowledge.declared == fp.declared
                    // a loss of the whole declared size or more is not a loss: such a record is damaged
                    && r.knowledge.loss.as_ref().is_none_or(|l| l.real < fp.declared)
        })?;
        rec.seen = now;
        Some(rec.knowledge.clone())
    }

    /// Stores the knowledge of a repaired drive under `new` (a key made by our repair), replacing the record that
    /// had the `old` layout or an earlier record with the `new` one. Keeps the 100 most recently seen records.
    pub fn put(&mut self, old: Option<&Fingerprint>, new: Fingerprint, knowledge: Knowledge, now: u64) {
        // a record stored under this key is newer than any removal of it
        self.tombstones.retain(|t| t.fingerprint != new);
        let rec = Record { fingerprint: new.clone(), knowledge, seen: now, changed: now, from_repair: true };
        let at = old.and_then(|o| self.drives.iter().position(|r| &r.fingerprint == o));
        let same = self.drives.iter().position(|r| r.fingerprint == new);
        match (at, same) {
            (Some(i), Some(j)) if i != j => {
                let gone = self.drives[i].fingerprint.clone();
                self.tombstone(gone, now);
                self.drives[i] = rec;
                self.drives.remove(j);
            }
            (Some(i), _) | (None, Some(i)) => {
                if self.drives[i].fingerprint != new {
                    let gone = self.drives[i].fingerprint.clone();
                    self.tombstone(gone, now);
                }
                self.drives[i] = rec;
            }
            (None, None) => self.drives.push(rec),
        }
        if self.drives.len() > MAX_RECORDS {
            self.drives.sort_by_key(|r| std::cmp::Reverse(r.seen));
            let evicted: Vec<Fingerprint> =
                self.drives.split_off(MAX_RECORDS).into_iter().map(|r| r.fingerprint).collect();
            for f in evicted {
                self.tombstone(f, now);
            }
        }
    }
}

/// What the program holds for a connected drive: its knowledge and the layout it is associated with.
#[derive(Clone, Debug)]
pub struct Entry {
    pub knowledge: Knowledge,
    /// The layout the knowledge belongs to. None: the layout could not identify the drive when the entry was made.
    pub saved: Option<Fingerprint>,
    /// The record is kept in `capacity.json`: its key was made by our own repair. Knowledge from checks of a drive
    /// that was not repaired by us stays in memory and is dropped when the drive goes away.
    pub from_repair: bool,
    /// Our repair finished but the new layout could not be used as a key (the listing failed, the layout cannot name
    /// the drive, or it is still the old one): the knowledge stays in memory only, shown as unconfirmed, and nothing
    /// is stored under any key. A new check replaces it.
    pub unconfirmed: bool,
}

impl Entry {
    pub fn new(knowledge: Knowledge, saved: Option<Fingerprint>, from_repair: bool) -> Self {
        Entry { knowledge, saved, from_repair, unconfirmed: false }
    }

    /// The drive in front of us is the one the knowledge is about: its layout is the saved one (an entry without a
    /// key can only be confirmed by a layout that cannot name a drive either).
    pub fn confirmed_by(&self, fp: &Fingerprint) -> bool {
        if self.unconfirmed {
            return false;
        }
        match &self.saved {
            Some(saved) => fp.usable() && saved == fp,
            None => !fp.usable(),
        }
    }
}

/// The view of a drive whose knowledge cannot be confirmed: no value, no plan, a new check is asked for.
fn unconfirmed(disk: &Disk, session_start: u64) -> View {
    let mut v = view(None, disk, session_start);
    v.state = State::Unconfirmed;
    v.value = 0;
    v.verified = false;
    v.repair = RepairPlan { default: PlanDefault::None, limit: None, whole: disk.size };
    v
}

/// The view of a drive from its entry. Knowledge that the drive's current layout does not confirm (another drive
/// took this `diskN`, the layout changed by something other than our repair, or it cannot be read) is shown as
/// unconfirmed and never drives a plan. `busy`: a task of this drive is running (its layout may change under it),
/// so the entry is taken as it is.
pub fn view_entry(entry: Option<&Entry>, disk: &Disk, session_start: u64, busy: bool) -> View {
    // a partition table that could not be read hides what the drive is: nothing known about it can be confirmed
    if disk.table_unreadable {
        return unconfirmed(disk, session_start);
    }
    match entry {
        Some(e)
            if e.knowledge.declared == disk.size
                && !(busy && !e.unconfirmed)
                && !e.confirmed_by(&Fingerprint::of(disk)) =>
        {
            unconfirmed(disk, session_start)
        }
        Some(e) if e.knowledge.declared == disk.size => view(Some(&e.knowledge), disk, session_start),
        _ => view(None, disk, session_start),
    }
}

/// Brings the entries in step with the connected drives. Returns true when an entry was created, dropped or
/// replaced (the window then needs a new list) or the store changed.
/// 1. Entries of drives that are gone (or changed size) are dropped; their saved records stay.
/// 2. A drive whose layout is usable and differs from the entry's is not the drive the entry was about (another
///    drive took the `diskN`, or it was reformatted): the stored record of the old layout is left untouched, and the
///    new layout is looked up afresh among the records made by our repair. A drive found there gets that record.
///    Otherwise the entry stays but is shown as unconfirmed ([`view_entry`]), so no plan comes from it until a new check.
///
/// While a task runs the same is done for every drive except the task's own (`skip`), so a drive that vanished or
/// changed size during a long check does not keep its knowledge.
///
/// A layout that cannot identify the drive right now (during a repair, `diskutil` failed) changes nothing in the
/// entries; the knowledge is then shown as unconfirmed as well.
pub fn sync(
    entries: &mut HashMap<String, Entry>,
    store: &mut Store,
    disks: &[Disk],
    now: u64,
    skip: Option<&str>,
) -> bool {
    let mut changed = false;
    let before = entries.len();
    // `skip`: the drive of the running task, whose layout belongs to the task
    entries.retain(|id, e| {
        Some(id.as_str()) == skip || disks.iter().any(|d| &d.id == id && d.size == e.knowledge.declared)
    });
    changed |= entries.len() != before;
    for disk in disks {
        let fp = Fingerprint::of(disk);
        if Some(disk.id.as_str()) == skip
            || !fp.usable()
            || entries.get(&disk.id).is_some_and(|e| e.saved.as_ref() == Some(&fp))
        {
            continue;
        }
        if let Some(knowledge) = store.find(&fp, now) {
            entries.insert(disk.id.clone(), Entry::new(knowledge, Some(fp), true));
            changed = true;
        }
    }
    changed
}

/// The entry of `disk`: the one in memory, else a record of our repair in the store.
pub fn entry_for(entries: &HashMap<String, Entry>, store: &mut Store, disk: &Disk, now: u64) -> Option<Entry> {
    if let Some(e) = entries.get(&disk.id).filter(|e| e.knowledge.declared == disk.size) {
        return Some(e.clone());
    }
    let fp = Fingerprint::of(disk);
    store.find(&fp, now).map(|k| Entry::new(k, Some(fp), true))
}

/// Changes the knowledge of the drive `disk` (as it is now). A missing entry is first loaded from the store, so an
/// older result is not overwritten by an empty record; an entry the layout of `disk` does not confirm is the
/// knowledge of another drive and is replaced. `change` sees that knowledge, so a result is worked out from
/// everything known. The knowledge is saved in `capacity.json` only for a drive whose key was made by our repair;
/// a check of any other drive stays in memory.
///
/// `repair`: this is our own repair, and the value is the layout the drive had before it. When the new layout can
/// name the drive and differs from the old one, the record moves to it. Otherwise nothing is stored under any key
/// (the old one is shared by the drives of a factory batch): the knowledge stays in memory, unconfirmed, and the
/// stored record (if any) is left as it is. A repair of a drive that knows nothing stores nothing.
/// Returns true when the store changed.
pub fn record_change(
    entries: &mut HashMap<String, Entry>,
    store: &mut Store,
    disk: &Disk,
    now: u64,
    repair: Option<&Fingerprint>,
    change: impl FnOnce(&mut Knowledge),
) -> bool {
    let fp = Fingerprint::of(disk);
    let stale = entries
        .get(&disk.id)
        .is_some_and(|e| e.knowledge.declared != disk.size || (repair.is_none() && !e.confirmed_by(&fp)));
    if stale {
        entries.remove(&disk.id);
    }
    let entry = entries.entry(disk.id.clone()).or_insert_with(|| match store.find(&fp, now) {
        Some(knowledge) => Entry::new(knowledge, Some(fp.clone()), true),
        None => Entry::new(Knowledge::new(disk.size), None, false),
    });
    change(&mut entry.knowledge);
    if let Some(pre) = repair {
        if entry.knowledge == Knowledge::new(disk.size) {
            // nothing is known about this drive: there is nothing to remember
            entries.remove(&disk.id);
            return false;
        }
        if !fp.usable() || &fp == pre {
            entry.unconfirmed = true;
            return false;
        }
        let old = entry.saved.clone();
        entry.saved = Some(fp.clone());
        entry.unconfirmed = false;
        entry.from_repair = true;
        store.put(old.as_ref(), fp, entry.knowledge.clone(), now);
        return true;
    }
    if !fp.usable() {
        // a check while the layout cannot name the drive: the entry has no key and stays in memory (an entry
        // with a key is already replaced above, because an unreadable layout does not confirm it)
        return false;
    }
    entry.saved = Some(fp.clone());
    if entry.from_repair {
        store.put(None, fp, entry.knowledge.clone(), now);
        return true;
    }
    false
}

/// The end of our own repair: puts the new layout of the drive into the list (so the next `disks` message shows
/// the result), restores the entry if the drive dropped out of the list meanwhile, and moves the record to the
/// new layout. `pre` is the layout the drive had before the repair. Returns true when the store changed.
// the pieces of State it works on are passed one by one so it stays a pure function that tests call directly
#[allow(clippy::too_many_arguments)]
pub fn finish_repair(
    entries: &mut HashMap<String, Entry>,
    store: &mut Store,
    disks: &mut [Disk],
    before: Option<Entry>,
    layout: Disk,
    pre: &Fingerprint,
    repair: Repair,
    now: u64,
) -> bool {
    if let Some(entry) = before {
        entries.entry(layout.id.clone()).or_insert(entry);
    }
    if let Some(slot) = disks.iter_mut().find(|d| d.id == layout.id && d.size == layout.size) {
        *slot = layout.clone();
    }
    record_change(entries, store, &layout, now, Some(pre), |k| k.apply_repair(repair, now))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;
    const DECLARED: u64 = 125_829_120_000;
    const PART: u64 = 8_303_673_344;
    const UUID: &str = "00000000-0000-4000-8000-000000000001";

    fn part(offset: u64, size: u64, id: &str) -> Partition {
        Partition { offset, size, id: id.into() }
    }

    fn disk(id: &str, size: u64, partitions: Vec<Partition>) -> Disk {
        Disk {
            id: id.into(),
            device: format!("/dev/r{id}"),
            name: "Storage Device".into(),
            kind: "usb".into(),
            bus: "USB".into(),
            size,
            block_size: 512,
            volumes: Vec::new(),
            partitions,
            table_unreadable: false,
            table_id: String::new(),
        }
    }

    /// The repaired fake from the bug report.
    fn repaired_disk() -> Disk {
        disk("disk6", DECLARED, vec![part(MIB, PART, UUID)])
    }

    fn shown(v: &View) -> Shown<'static> {
        Shown { state: v.state.as_str(), limit: v.repair.limit }
    }

    fn rec(mode: Mode, scope: Scope, verdict: Verdict, real: u64) -> CheckRecord {
        CheckRecord { mode, scope, verdict, real, declared: DECLARED, at: 1000, ..Default::default() }
    }

    fn fake_loss(real: u64) -> Knowledge {
        let mut k = Knowledge::new(DECLARED);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Fake, real));
        k
    }

    // ---- layout

    #[test]
    fn margin_values() {
        assert_eq!(limit_margin(125_829_120_000), 12_582_912_000);
        assert_eq!(limit_margin(300 * MIB), 64 * MIB);
    }

    #[test]
    fn layout_limit_of_repaired_disk6() {
        assert_eq!(layout_limit(&repaired_disk()), Some(PART));
    }

    #[test]
    fn layout_not_limited_when_full() {
        let d = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, UUID)]);
        assert_eq!(layout_limit(&d), None);
    }

    #[test]
    fn layout_not_limited_without_partitions() {
        assert_eq!(layout_limit(&disk("disk6", DECLARED, Vec::new())), None);
    }

    #[test]
    fn small_card_limited() {
        let d = disk("disk6", GB, vec![part(MIB, 300 * MIB, UUID)]);
        assert_eq!(layout_limit(&d), Some(300 * MIB));
    }

    #[test]
    fn loss_repair_size_zero_for_zero_and_tiny() {
        let size = 100 << 30;
        assert_eq!(loss_repair_size(0, size), 0);
        assert_eq!(loss_repair_size(MIB, size), 0);
        assert_eq!(loss_repair_size(2 * MIB + 1, size), 0);
        assert_eq!(loss_repair_size(8 << 30, size), (8 << 30) - 2 * MIB);
    }

    // ---- knowledge

    #[test]
    fn quick_fake_records_loss() {
        let k = fake_loss(PART);
        assert_eq!(k.loss, Some(Loss { real: PART, verdict: Verdict::Fake }));
        let last = k.last.unwrap();
        assert_eq!((last.mode, last.scope, last.verdict, last.real), (Mode::Quick, Scope::Device, Verdict::Fake, PART));
    }

    #[test]
    fn device_pass_clears_loss() {
        let mut k = fake_loss(PART);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Genuine, DECLARED));
        assert_eq!(k.loss, None);
        let mut k = fake_loss(PART);
        k.apply_full(rec(Mode::Full, Scope::Device, Verdict::Genuine, DECLARED));
        assert_eq!(k.loss, None);
        assert_eq!(k.last.unwrap().verdict, Verdict::Genuine);
    }

    #[test]
    fn full_loss_uses_99_percent() {
        // the caller works the loss out as min(E, declared) / 100 * 99; the knowledge stores it as given
        let estimate = 10 * GB;
        let real = estimate.min(DECLARED) / 100 * 99;
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(rec(Mode::Full, Scope::Device, Verdict::Fake, real));
        assert_eq!(k.loss, Some(Loss { real: 9_900_000_000, verdict: Verdict::Fake }));
    }

    #[test]
    fn partition_loss_takes_min() {
        let mut k = fake_loss(PART);
        k.apply_full(rec(Mode::Full, Scope::Partition, Verdict::Fake, 5 * GB));
        assert_eq!(k.loss.as_ref().unwrap().real, 5 * GB);
        k.apply_full(rec(Mode::Full, Scope::Partition, Verdict::Fake, 7 * GB));
        assert_eq!(k.loss.as_ref().unwrap().real, 5 * GB);
        assert_eq!(k.last.as_ref().unwrap().real, 7 * GB);
    }

    #[test]
    fn partition_pass_keeps_loss() {
        let mut k = fake_loss(PART);
        k.apply_full(rec(Mode::Full, Scope::Partition, Verdict::Genuine, PART));
        assert_eq!(k.loss, Some(Loss { real: PART, verdict: Verdict::Fake }));
        assert_eq!(k.last.unwrap().scope, Scope::Partition);
    }

    #[test]
    fn repair_keeps_knowledge() {
        let mut k = fake_loss(PART);
        k.apply_repair(Repair::Partition(PART - 2 * MIB), 2000);
        assert!(k.loss.is_some() && k.last.is_some());
        assert_eq!(k.repair, Some(RepairRecord { partition: PART - 2 * MIB, at: 2000 }));
    }

    #[test]
    fn whole_repair_drops_repair_entry() {
        let mut k = fake_loss(PART);
        k.apply_repair(Repair::Partition(PART - 2 * MIB), 2000);
        k.apply_repair(Repair::Whole, 3000);
        assert_eq!(k.repair, None);
        assert!(k.loss.is_some());
    }

    // ---- view

    fn repaired_k() -> Knowledge {
        // the loss of the bug report; our repair leaves a partition of real - 2 MiB at offset 1 MiB
        fake_loss(PART)
    }

    #[test]
    fn state_after_repair_is_repaired() {
        let d = disk("disk6", DECLARED, vec![part(MIB, PART - 2 * MIB, UUID)]);
        let v = view(Some(&repaired_k()), &d, 0);
        assert_eq!(v.state, State::Repaired);
        assert_eq!(v.value, PART - 2 * MIB);
        assert_eq!(v.repair.default, PlanDefault::Limit);
        assert_eq!(v.repair.limit, Some(PART - 2 * MIB));
        // the layout of the bug report itself (partition exactly as big as the loss) is repaired too
        let v = view(Some(&repaired_k()), &repaired_disk(), 0);
        assert_eq!((v.state, v.repair.limit), (State::Repaired, Some(PART - 2 * MIB)));
    }

    #[test]
    fn repaired_then_verified() {
        let mut k = repaired_k();
        k.apply_full(rec(Mode::Full, Scope::Partition, Verdict::Genuine, PART));
        let v = view(Some(&k), &repaired_disk(), 0);
        assert_eq!(v.state, State::Repaired);
        assert!(v.verified);
        assert_ne!(v.state, State::Genuine);
    }

    #[test]
    fn reformatted_full_size_is_fake_again() {
        let d = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, "other")]);
        let v = view(Some(&repaired_k()), &d, 0);
        assert_eq!(v.state, State::Fake);
        assert_eq!(v.repair.default, PlanDefault::Limit);
    }

    #[test]
    fn limited_without_record() {
        let d = disk("disk6", DECLARED, vec![part(MIB, PART + 12345, UUID)]);
        let v = view(None, &d, 0);
        assert_eq!(v.state, State::Limited);
        assert_eq!(v.value, PART + 12345);
        assert_eq!(v.repair.default, PlanDefault::Limit);
        assert_eq!(v.repair.limit, Some((PART + 12345) / MIB * MIB));
    }

    #[test]
    fn broken_by_tiny_loss() {
        for real in [0, MIB, 2 * MIB + 1] {
            let v = view(Some(&fake_loss(real)), &repaired_disk(), 0);
            assert_eq!(v.state, State::Broken, "real {real}");
            assert_eq!(v.value, 0);
        }
    }

    #[test]
    fn broken_by_quick_real_zero() {
        let mut k = Knowledge::new(DECLARED);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Broken, 0));
        let d = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, UUID)]);
        let v = view(Some(&k), &d, 0);
        assert_eq!(v.state, State::Broken);
        assert_eq!(v.repair.default, PlanDefault::None);
        assert_eq!(v.repair.limit, None);
    }

    #[test]
    fn genuine_with_small_partition() {
        let mut k = Knowledge::new(DECLARED);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Genuine, DECLARED));
        let v = view(Some(&k), &repaired_disk(), 0);
        assert_eq!(v.state, State::Genuine);
        assert_eq!(v.value, DECLARED);
        assert_eq!(v.repair.default, PlanDefault::Whole);
    }

    #[test]
    fn unknown_full_partition_has_no_plan() {
        let d = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, UUID)]);
        let v = view(None, &d, 0);
        assert_eq!(v.state, State::Unknown);
        assert_eq!(v.repair.default, PlanDefault::None);
        assert_eq!(v.repair.limit, None);
        assert_eq!(view(None, &disk("disk6", DECLARED, Vec::new()), 0).state, State::Unknown);
    }

    #[test]
    fn partition_pass_shows_partition_verified_also_without_a_limit() {
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(rec(Mode::Full, Scope::Partition, Verdict::Genuine, PART));
        assert_eq!(view(Some(&k), &repaired_disk(), 0).state, State::PartitionOk);
        let d = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, UUID)]);
        assert_eq!(view(Some(&k), &d, 0).state, State::PartitionOk);
    }

    #[test]
    fn repeated_repair_same_size() {
        let size = loss_repair_size(PART, DECLARED);
        let d = disk("disk6", DECLARED, vec![part(MIB, size, UUID)]);
        let v = view(Some(&repaired_k()), &d, 0);
        assert_eq!(v.repair.limit, Some(layout_limit(&d).unwrap()));
    }

    #[test]
    fn remembered_follows_the_session_start() {
        let k = fake_loss(PART);
        assert!(view(Some(&k), &repaired_disk(), 2000).remembered);
        assert!(!view(Some(&k), &repaired_disk(), 500).remembered);
    }

    #[test]
    fn view_json_shape() {
        let v = view(Some(&repaired_k()), &repaired_disk(), 0);
        let j = serde_json::to_value(&v).unwrap();
        for key in ["state", "verified", "value", "declared", "partition", "unused", "check", "remembered", "repair"] {
            assert!(j.get(key).is_some(), "missing {key}");
        }
        assert_eq!(j["state"], "repaired");
        assert_eq!(j["repair"]["default"], "limit");
        assert!(j["repair"].get("limit").is_some() && j["repair"].get("whole").is_some());
        assert_eq!(j["check"]["stoppedEarly"], false);
        let j = serde_json::to_value(view(None, &repaired_disk(), 0)).unwrap();
        assert_eq!(j["state"], "limited");
        assert_eq!(j["check"], serde_json::Value::Null);
    }

    // ---- full check scope and record

    /// What the full check of a volume of `total` bytes found; `tested` is everything that was free minus the reserve.
    fn full_result(total: u64, bad: u64, estimate: u64) -> FullResult {
        let used_before = 8 * MIB;
        FullResult {
            total,
            used_before,
            tested: total - used_before - 8 * MIB,
            bad,
            real_estimate: estimate,
            seconds: 1.0,
            ..Default::default()
        }
    }

    /// A drive with a factory layout: one partition up to the end of the drive.
    fn full_layout() -> Disk {
        disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, UUID)])
    }

    fn repaired_k_with_repair(size: u64) -> Knowledge {
        let mut k = repaired_k();
        k.apply_repair(Repair::Partition(size), 5);
        k
    }

    #[test]
    fn passing_check_of_a_repaired_fake() {
        // the bug report: the 8.3 GB volume of a 125.8 GB drive
        let mut k = repaired_k_with_repair(PART - 2 * MIB);
        let d = disk("disk6", DECLARED, vec![part(MIB, PART - 2 * MIB, UUID)]);
        let r = full_record(Some(&k), &d, &full_result(PART - 40 * MIB, 0, 0), 9);
        assert_eq!((r.scope, r.verdict, r.real), (Scope::Partition, Verdict::Genuine, PART - 40 * MIB));
        k.apply_full(r);
        let v = view(Some(&k), &d, 0);
        assert_eq!((v.state, v.verified), (State::Repaired, true));
        assert_eq!(v.repair.limit, Some(PART - 2 * MIB));
    }

    #[test]
    fn passing_check_of_a_full_size_volume() {
        let r = full_record(None, &full_layout(), &full_result(DECLARED - 40 * MIB, 0, 0), 9);
        assert_eq!((r.scope, r.verdict, r.real), (Scope::Device, Verdict::Genuine, DECLARED));
    }

    #[test]
    fn partition_check_finds_loss() {
        let k = repaired_k();
        let d = repaired_disk();
        let r = full_record(Some(&k), &d, &full_result(PART - 40 * MIB, 1000, 5 * GB), 9);
        assert_eq!((r.scope, r.verdict, r.real), (Scope::Partition, Verdict::Fake, 4_950_000_000));
        let mut k2 = k.clone();
        k2.apply_full(r);
        assert_eq!(k2.loss.unwrap().real, 4_950_000_000);
        // a bigger estimate than the known loss keeps the known loss
        let r = full_record(Some(&k), &d, &full_result(PART - 40 * MIB, 1000, 8 * GB), 9);
        let mut k3 = k.clone();
        k3.apply_full(r);
        assert_eq!(k3.loss.unwrap().real, 7_920_000_000);
    }

    #[test]
    fn estimate_that_is_only_the_used_space_keeps_nothing() {
        // every test block damaged, the estimate is the 32 KiB first cluster that was in use
        let r = full_record(None, &full_layout(), &full_result(DECLARED - 40 * MIB, 999, 32 * 1024), 9);
        assert_eq!((r.verdict, r.real), (Verdict::Broken, 32 * 1024 / 100 * 99));
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(r);
        let v = view(Some(&k), &disk("disk6", DECLARED, Vec::new()), 0);
        assert_eq!(v.state, State::Broken);
        assert_eq!(v.repair.default, PlanDefault::None);
    }

    #[test]
    fn repaired_near_full_fake_stays_repaired() {
        // probe: damaged, real 120 GB of 125.8 GB: the unused tail is below the limit margin
        let real = 120 * GB;
        let mut k = Knowledge::new(DECLARED);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Damaged, real));
        let size = loss_repair_size(real, DECLARED);
        let d = disk("disk6", DECLARED, vec![part(MIB, size, UUID)]);
        assert_eq!(layout_limit(&d), None);
        // before the repair the layout does not tell, after our repair the record does
        k.apply_repair(Repair::Partition(size), 10);
        let v = view(Some(&k), &d, 0);
        assert_eq!((v.state, v.value), (State::Repaired, size));
        assert_eq!(v.repair.limit, Some(size));
        // the offered check of the repaired volume is a partition check, and its pass changes nothing
        let r = full_record(Some(&k), &d, &full_result(size - 40 * MIB, 0, 0), 20);
        assert_eq!(r.scope, Scope::Partition);
        k.apply_full(r);
        let v = view(Some(&k), &d, 0);
        assert_eq!((v.state, v.verified), (State::Repaired, true));
        assert_eq!(v.repair.default, PlanDefault::Limit);
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errRepairChanged"));
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(shown(&v))), Ok(0)); // only by an explicit choice
        assert_eq!(check_fix(&v, "limit", size, None), Ok(size));
    }

    #[test]
    fn volume_within_a_known_loss_is_a_partition_check() {
        // no repair record, but the volume does not exceed the loss
        let k = fake_loss(120 * GB);
        let d = full_layout();
        assert_eq!(full_scope(Some(&k), &d, 119 * GB, 0), Scope::Partition);
        assert_eq!(full_scope(Some(&k), &d, DECLARED - 40 * MIB, 0), Scope::Device);
    }

    #[test]
    fn a_volume_a_bit_smaller_than_the_drive_is_a_partition_check() {
        // a fake of 120 GB repaired elsewhere: no record, the volume misses 6.8 GB (more than 2%)
        let size = 119 * GB;
        let d = disk("disk6", DECLARED, vec![part(MIB, size, UUID)]);
        let r = full_record(None, &d, &full_result(size - 40 * MIB, 0, 0), 9);
        assert_eq!((r.scope, r.verdict, r.real), (Scope::Partition, Verdict::Genuine, size - 40 * MIB));
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(r);
        let v = view(Some(&k), &d, 0);
        assert_eq!(v.state, State::PartitionOk);
        assert_eq!(v.value, size - 40 * MIB);
        assert_eq!(v.repair.default, PlanDefault::Limit);
        // a layout with a big tail is a partition check even when the volume fills the partition
        let d = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 500 * MIB, UUID)]);
        let r = full_record(None, &d, &full_result(DECLARED - 502 * MIB, 0, 0), 9);
        assert_eq!(r.scope, Scope::Partition);
        // a volume that misses 1% of a big drive and a layout ending at the end: the whole device
        let d = full_layout();
        let r = full_record(None, &d, &full_result(DECLARED - DECLARED / 100, 0, 0), 9);
        assert_eq!(r.scope, Scope::Device);
    }

    #[test]
    fn write_error_pass_never_clears_a_known_loss() {
        // a known fake (8.3 GB real) with the factory layout; the full check writes ~8 GB, then the drive refuses
        // writes with an I/O error; what was written reads back fine
        let mut k = fake_loss(PART);
        let d = full_layout();
        let r = FullResult {
            total: DECLARED - 40 * MIB,
            used_before: 8 * MIB,
            tested: 8 * GB,
            write_error: Some("Input/output error".into()),
            seconds: 1.0,
            ..Default::default()
        };
        let rec = full_record(Some(&k), &d, &r, 5);
        assert_eq!(rec.scope, Scope::Partition);
        assert_eq!(rec.real, 8 * MIB + 8 * GB);
        k.apply_full(rec);
        assert_eq!(k.loss, Some(Loss { real: PART, verdict: Verdict::Fake }));
        let v = view(Some(&k), &d, 0);
        assert_eq!(v.state, State::Fake);
        assert_eq!(v.repair.default, PlanDefault::Limit);
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errRepairChanged"));
    }

    #[test]
    fn write_error_pass_without_a_record_is_not_genuine() {
        let d = full_layout();
        let r = FullResult {
            total: DECLARED - 40 * MIB,
            used_before: 8 * MIB,
            tested: 8 * GB,
            write_error: Some("Input/output error".into()),
            seconds: 1.0,
            ..Default::default()
        };
        let rec = full_record(None, &d, &r, 5);
        assert_eq!((rec.scope, rec.verdict), (Scope::Partition, Verdict::Genuine));
        assert!(rec.real < DECLARED);
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(rec);
        let v = view(Some(&k), &d, 0);
        assert_ne!(v.state, State::Genuine);
        assert_eq!(v.state, State::PartitionOk);
        assert_eq!(v.value, 8 * MIB + 8 * GB);
        assert_eq!(v.repair.default, PlanDefault::Limit);
    }

    #[test]
    fn a_test_that_wrote_much_less_than_the_free_space_is_partial() {
        // no error reported, but far less than the free space was written
        let d = full_layout();
        let r = FullResult { total: DECLARED - 40 * MIB, used_before: 8 * MIB, tested: 20 * GB, ..Default::default() };
        let rec = full_record(None, &d, &r, 5);
        assert_eq!(rec.scope, Scope::Partition);
        // the whole free space (minus the reserve) is a full pass
        assert_eq!(full_record(None, &d, &full_result(DECLARED - 40 * MIB, 0, 0), 5).scope, Scope::Device);
    }

    #[test]
    fn verified_needs_a_check_made_after_the_repair() {
        let size = PART - 2 * MIB;
        let d = disk("disk6", DECLARED, vec![part(MIB, size, UUID)]);
        let mut k = repaired_k();
        // a partition pass from before the repair
        k.apply_full(CheckRecord { at: 100, ..rec(Mode::Full, Scope::Partition, Verdict::Genuine, PART) });
        k.apply_repair(Repair::Partition(size), 200);
        assert!(!view(Some(&k), &d, 0).verified);
        k.apply_full(CheckRecord { at: 300, ..rec(Mode::Full, Scope::Partition, Verdict::Genuine, PART) });
        assert!(view(Some(&k), &d, 0).verified);
    }

    #[test]
    fn store_keeps_unreadable_records() {
        let path = temp_path("foreign");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let good = Record {
            fingerprint: fp(DECLARED, MIB, PART, UUID),
            knowledge: fake_loss(PART),
            seen: 1,
            changed: 1,
            from_repair: true,
        };
        let doc = serde_json::json!({
            "version": 1,
            "drives": [serde_json::to_value(&good).unwrap(), {"fingerprint": "nonsense"}, {"future": true}]
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        let mut store = Store::load_from(&path);
        assert_eq!((store.drives.len(), store.foreign.len()), (1, 2));
        store.put(None, fp(GB, MIB, 300 * MIB, "x"), Knowledge::new(GB), 2);
        store.save_to(&path).unwrap();
        let again = Store::load_from(&path);
        assert_eq!((again.drives.len(), again.foreign.len()), (2, 2));
        // a record with missing optional fields still loads
        std::fs::write(
            &path,
            r#"{"drives":[{"fingerprint":{"declared":5,"partitions":[]},"knowledge":{"declared":5}}]}"#,
        )
        .unwrap();
        assert_eq!(Store::load_from(&path).drives.len(), 1);
        cleanup(&path);
    }

    // ---- repair request

    #[test]
    fn fix_limit_ok() {
        let v = view(Some(&repaired_k()), &repaired_disk(), 0);
        assert_eq!(check_fix(&v, "limit", PART - 2 * MIB, None), Ok(PART - 2 * MIB));
    }

    #[test]
    fn fix_limit_mismatch_refused() {
        let v = view(Some(&repaired_k()), &repaired_disk(), 0);
        assert_eq!(check_fix(&v, "limit", PART, None), Err("errRepairChanged"));
    }

    #[test]
    fn fix_limit_without_limit_refused() {
        let unknown = view(None, &disk("disk6", DECLARED, Vec::new()), 0);
        assert_eq!(check_fix(&unknown, "limit", PART, None), Err("errNoPlan"));
        let mut k = Knowledge::new(DECLARED);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Genuine, DECLARED));
        let genuine = view(Some(&k), &repaired_disk(), 0);
        assert_eq!(check_fix(&genuine, "limit", PART, None), Err("errRepairChanged"));
        assert_eq!(check_fix(&genuine, "nonsense", DECLARED, None), Err("errRepairChanged"));
    }

    #[test]
    fn fix_whole_ok() {
        let v = view(Some(&repaired_k()), &repaired_disk(), 0);
        // a drive with a limit gets the full size only by the explicit choice
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errRepairChanged"));
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(shown(&v))), Ok(0));
        assert_eq!(check_fix(&v, "whole", DECLARED - MIB, Some(shown(&v))), Err("errRepairChanged"));
        // a never-checked drive: no plan, the full size only by the tick
        let unknown = view(None, &disk("disk6", DECLARED, Vec::new()), 0);
        assert_eq!(check_fix(&unknown, "whole", DECLARED, None), Err("errNoPlan"));
        assert_eq!(check_fix(&unknown, "whole", DECLARED, Some(shown(&unknown))), Ok(0));
        // a genuine drive: the default
        let mut k = Knowledge::new(DECLARED);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Genuine, DECLARED));
        let genuine = view(Some(&k), &repaired_disk(), 0);
        assert_eq!(check_fix(&genuine, "whole", DECLARED, None), Ok(0));
    }

    #[test]
    fn fix_broken_refused_for_both_plans() {
        let v = view(Some(&fake_loss(MIB)), &repaired_disk(), 0);
        assert_eq!(check_fix(&v, "limit", PART, None), Err("errBroken"));
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errBroken"));
    }

    // ---- store

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("umc-capacity-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("capacity.json")
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn fp(declared: u64, offset: u64, size: u64, id: &str) -> Fingerprint {
        Fingerprint { declared, table_id: String::new(), partitions: vec![part(offset, size, id)] }
    }

    #[test]
    fn store_round_trip() {
        let path = temp_path("round");
        let mut store = Store::default();
        store.put(None, fp(DECLARED, MIB, PART, UUID), fake_loss(PART), 5);
        store.save_to(&path).unwrap();
        let mut back = Store::load_from(&path);
        assert_eq!(back, store);
        assert_eq!(back.find(&fp(DECLARED, MIB, PART, UUID), 9), Some(fake_loss(PART)));
        assert_eq!(back.drives[0].seen, 9);
        cleanup(&path);
    }

    #[test]
    fn broken_file_gives_empty_store() {
        let path = temp_path("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        let store = Store::load_from(&path);
        assert!(store.drives.is_empty());
        assert!(Store::load_from(&path.with_file_name("missing.json")).drives.is_empty());
        store.save_to(&path).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).is_ok());
        cleanup(&path);
    }

    #[test]
    fn fingerprint_without_ids_never_matches() {
        let mut store = Store::default();
        let none = fp(DECLARED, MIB, PART, "");
        store.put(None, none.clone(), fake_loss(PART), 1);
        assert!(!none.usable());
        assert_eq!(store.find(&none, 2), None);
        // a Windows layout is usable through the table id
        let win = Fingerprint { table_id: "1A2B3C4D".into(), ..none };
        assert!(win.usable());
        store.put(None, win.clone(), fake_loss(PART), 1);
        assert!(store.find(&win, 2).is_some());
    }

    #[test]
    fn match_needs_size_layout_and_id() {
        let mut store = Store::default();
        store.put(None, fp(DECLARED, MIB, PART, UUID), fake_loss(PART), 1);
        assert!(store.find(&fp(DECLARED, MIB, PART, UUID), 2).is_some());
        assert!(store.find(&fp(DECLARED + 1, MIB, PART, UUID), 2).is_none());
        assert!(store.find(&fp(DECLARED, 2 * MIB, PART, UUID), 2).is_none());
        assert!(store.find(&fp(DECLARED, MIB, PART - MIB, UUID), 2).is_none());
        assert!(store.find(&fp(DECLARED, MIB, PART, "other"), 2).is_none());
    }

    #[test]
    fn put_rekeys_instead_of_duplicating() {
        let mut store = Store::default();
        let old = fp(DECLARED, MIB, PART, UUID);
        let new = fp(DECLARED, MIB, PART - 2 * MIB, "new-uuid");
        store.put(None, old.clone(), fake_loss(PART), 1);
        store.put(Some(&old), new.clone(), fake_loss(PART), 2);
        assert_eq!(store.drives.len(), 1);
        assert_eq!(store.drives[0].fingerprint, new);
        // an unrelated drive stays
        store.put(None, fp(GB, MIB, 300 * MIB, "x"), Knowledge::new(GB), 3);
        store.put(Some(&new), fp(DECLARED, MIB, PART, "again"), fake_loss(PART), 4);
        assert_eq!(store.drives.len(), 2);
    }

    #[test]
    fn evicts_oldest_over_100() {
        let mut store = Store::default();
        for i in 0..105u64 {
            store.put(None, fp(DECLARED, MIB, PART, &format!("id-{i}")), Knowledge::new(DECLARED), i + 1);
        }
        assert_eq!(store.drives.len(), 100);
        assert!(store.find(&fp(DECLARED, MIB, PART, "id-0"), 999).is_none());
        assert!(store.find(&fp(DECLARED, MIB, PART, "id-104"), 999).is_some());
    }

    // ---- sync

    #[test]
    fn sync_loads_record_on_replug_with_new_id() {
        let mut store = Store::default();
        store.put(None, Fingerprint::of(&repaired_disk()), fake_loss(PART), 1);
        let mut entries = HashMap::new();
        let replugged = disk("disk7", DECLARED, repaired_disk().partitions);
        assert!(sync(&mut entries, &mut store, &[replugged], 10, None));
        assert_eq!(entries["disk7"].knowledge, fake_loss(PART));
        assert!(!entries.contains_key("disk6"));
    }

    #[test]
    fn sync_drops_gone_disk_keeps_record() {
        let mut store = Store::default();
        store.put(None, Fingerprint::of(&repaired_disk()), fake_loss(PART), 1);
        let mut entries = HashMap::new();
        sync(&mut entries, &mut store, &[repaired_disk()], 10, None);
        assert!(entries.contains_key("disk6"));
        sync(&mut entries, &mut store, &[], 11, None);
        assert!(entries.is_empty());
        assert_eq!(store.drives.len(), 1);
        // another declared size under the same id is another drive
        sync(&mut entries, &mut store, &[repaired_disk()], 12, None);
        let other = disk("disk6", GB, vec![part(MIB, 300 * MIB, "x")]);
        sync(&mut entries, &mut store, &[other], 13, None);
        assert!(!entries.contains_key("disk6"));
    }

    #[test]
    fn sync_does_not_move_a_record_on_a_layout_change() {
        // another layout under the same id (reformatted, or the drive was swapped between two scans): the
        // knowledge is shown as unconfirmed (no plan until a new check) and the stored record of the old layout
        // stays untouched
        let mut store = Store::default();
        let old = Fingerprint::of(&repaired_disk());
        store.put(None, old.clone(), fake_loss(PART), 1);
        let mut entries = HashMap::new();
        entries.insert("disk6".to_string(), Entry::new(fake_loss(PART), Some(old.clone()), true));
        let now_layout = disk("disk6", DECLARED, vec![part(MIB, DECLARED - MIB, "new-uuid")]);
        sync(&mut entries, &mut store, std::slice::from_ref(&now_layout), 20, None);
        assert_eq!(store.drives.len(), 1);
        assert_eq!(store.drives[0].fingerprint, old);
        let v = view_entry(entries.get("disk6"), &now_layout, 0, false);
        assert_eq!(v.state, State::Unconfirmed);
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(shown(&v))), Err("errUnconfirmed"));
        // the old drive plugged in again is still recognised
        assert!(store.find(&old, 30).is_some());
    }

    #[test]
    fn swap_between_scans_keeps_both_records() {
        // drive A (repaired fake, real 8.3 GB) is disk6; A is unplugged and fake B of the same batch (same
        // declared size, factory layout, other volume UUID) takes disk6 before the next scan
        let a = repaired_disk();
        let mut store = Store::default();
        let mut entries = HashMap::new();
        store.put(None, Fingerprint::of(&a), fake_loss(PART), 1);
        sync(&mut entries, &mut store, std::slice::from_ref(&a), 2, None);
        assert_eq!(entries["disk6"].knowledge, fake_loss(PART));
        let b = disk("disk6", DECLARED, vec![part(MIB, DECLARED - MIB, "B-UUID")]);
        sync(&mut entries, &mut store, std::slice::from_ref(&b), 3, None);
        assert_eq!(view_entry(entries.get("disk6"), &b, 0, false).state, State::Unconfirmed);
        // B's check stays in memory (a check-only drive is not remembered), and A's record is still there
        record_change(&mut entries, &mut store, &b, 4, None, |k| {
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Fake, 4 * GB))
        });
        assert_eq!(store.drives.len(), 1);
        assert_eq!(entries["disk6"].knowledge.loss.as_ref().unwrap().real, 4 * GB);
        assert_eq!(store.find(&Fingerprint::of(&a), 5), Some(fake_loss(PART)));
    }

    #[test]
    fn foreign_record_does_not_flap() {
        // a record whose knowledge is about another declared size never matches, so scans never change the store
        let mut store = Store::default();
        store.put(None, Fingerprint::of(&repaired_disk()), Knowledge::new(GB), 1);
        let mut entries = HashMap::new();
        let changed = (0..5).filter(|i| sync(&mut entries, &mut store, &[repaired_disk()], 10 + i, None)).count();
        assert_eq!(changed, 0);
        assert!(entries.is_empty());
    }

    #[test]
    fn record_change_loads_the_older_record_first() {
        let mut store = Store::default();
        let d = repaired_disk();
        store.put(None, Fingerprint::of(&d), fake_loss(PART), 1);
        // the entry is gone from memory (the drive dropped out of the list), then a check finishes
        let mut entries = HashMap::new();
        record_change(&mut entries, &mut store, &d, 5, None, |k| {
            k.apply_full(rec(Mode::Full, Scope::Partition, Verdict::Genuine, PART))
        });
        let k = &store.drives[0].knowledge;
        assert_eq!(k.loss, Some(Loss { real: PART, verdict: Verdict::Fake }));
        assert_eq!(k.last.as_ref().unwrap().scope, Scope::Partition);
        assert_eq!(store.drives.len(), 1);
    }

    #[test]
    fn finish_repair_rekeys_and_shows_repaired() {
        // before: a fake drive with a full-size factory layout and a recorded loss
        let old_disk = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, "old-uuid")]);
        let mut store = Store::default();
        let mut entries = HashMap::new();
        store.put(None, Fingerprint::of(&old_disk), fake_loss(PART), 1);
        sync(&mut entries, &mut store, std::slice::from_ref(&old_disk), 2, None);
        let before = entries.get("disk6").cloned();
        // after the repair the list still has the old layout; the watcher may even have dropped the entry
        let list_pre = old_disk.clone();
        let mut list = vec![old_disk];
        entries.clear();
        let size = loss_repair_size(PART, DECLARED);
        let new_layout = disk("disk6", DECLARED, vec![part(MIB, size, "new-uuid")]);
        let saved = finish_repair(
            &mut entries,
            &mut store,
            &mut list,
            before,
            new_layout.clone(),
            &Fingerprint::of(&list_pre),
            Repair::Partition(size),
            9,
        );
        assert!(saved);
        // the record moved to the new layout, no duplicate
        assert_eq!(store.drives.len(), 1);
        assert_eq!(store.drives[0].fingerprint, Fingerprint::of(&new_layout));
        assert_eq!(entries["disk6"].saved, Some(Fingerprint::of(&new_layout)));
        // the list already has the new layout, so the view sent at the end of the task says "repaired"
        assert_eq!(list[0], new_layout);
        let v = view(entries.get("disk6").map(|e| &e.knowledge), &list[0], 0);
        assert_eq!(v.state, State::Repaired);
        assert_eq!(v.repair.limit, Some(size));
        // the watcher finds nothing to move afterwards
        assert!(!sync(&mut entries, &mut store, &list, 10, None));
    }

    #[test]
    fn sync_skips_unusable_fingerprint() {
        let mut store = Store::default();
        let old = Fingerprint::of(&repaired_disk());
        store.put(None, old.clone(), fake_loss(PART), 1);
        let mut entries = HashMap::new();
        entries.insert("disk6".to_string(), Entry::new(fake_loss(PART), Some(old.clone()), true));
        // during a repair the disk has no partitions
        let blank = disk("disk6", DECLARED, Vec::new());
        assert!(!sync(&mut entries, &mut store, &[blank], 20, None));
        assert_eq!(store.drives[0].fingerprint, old);
        assert_eq!(entries["disk6"].saved, Some(old));
    }

    // ---- review round 2

    #[test]
    fn stale_whole_request_for_a_late_loaded_record_is_refused() {
        // B (a repaired near-full fake, record in the store) was plugged in during a task: listed, no entry
        let real = 120 * GB;
        let size = loss_repair_size(real, DECLARED);
        let b = disk("disk7", DECLARED, vec![part(MIB, size, "B")]);
        let mut k = Knowledge::new(DECLARED);
        k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Damaged, real));
        k.apply_repair(Repair::Partition(size), 10);
        let mut store = Store::default();
        store.put(None, Fingerprint::of(&b), k, 1);
        let mut entries = HashMap::new();
        let before = view(entries.get("disk7").map(|e: &Entry| &e.knowledge), &b, 0);
        assert_eq!((before.state, before.repair.default), (State::Unknown, PlanDefault::None));
        // the first scan after the task: the list is the same, but sync changed what the window shows
        assert!(sync(&mut entries, &mut store, std::slice::from_ref(&b), 20, None));
        let after = view(entries.get("disk7").map(|e| &e.knowledge), &b, 0);
        assert_eq!((after.state, after.repair.default), (State::Repaired, PlanDefault::Limit));
        // the window still has the old view and asks for the default "whole" without the explicit tick
        assert_eq!(check_fix(&after, "whole", DECLARED, None), Err("errRepairChanged"));
        assert_eq!(check_fix(&after, "limit", size, None), Ok(size));
    }

    #[test]
    fn only_records_made_by_our_repair_are_matched() {
        let d = repaired_disk();
        let mut store = Store::default();
        let mut check_only = Record {
            fingerprint: Fingerprint::of(&d),
            knowledge: fake_loss(PART),
            seen: 1,
            changed: 1,
            from_repair: false,
        };
        store.drives.push(check_only.clone());
        let mut entries = HashMap::new();
        // a factory image shares its volume UUID between drives of one batch: never matched
        assert!(!sync(&mut entries, &mut store, std::slice::from_ref(&d), 2, None));
        assert!(entries.is_empty());
        check_only.from_repair = true;
        store.drives[0] = check_only;
        assert!(sync(&mut entries, &mut store, &[d], 3, None));
        assert!(entries.contains_key("disk6"));
    }

    #[test]
    fn a_check_without_a_repair_stays_in_memory() {
        let d = repaired_disk();
        let mut store = Store::default();
        let mut entries = HashMap::new();
        assert!(!record_change(&mut entries, &mut store, &d, 5, None, |k| {
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Fake, PART))
        }));
        assert!(store.drives.is_empty());
        // while the drive stays connected the knowledge is there; replugged, it is gone
        assert_eq!(entries["disk6"].knowledge.loss.as_ref().unwrap().real, PART);
        assert!(!sync(&mut entries, &mut store, std::slice::from_ref(&d), 6, None));
        assert!(entries.contains_key("disk6"));
        sync(&mut entries, &mut store, &[], 7, None);
        sync(&mut entries, &mut store, &[disk("disk7", DECLARED, d.partitions.clone())], 8, None);
        assert!(entries.is_empty());
        // our repair makes the record persistent
        record_change(&mut entries, &mut store, &d, 9, Some(&Fingerprint::of(&full_layout())), |k| {
            k.apply_repair(Repair::Partition(PART - 2 * MIB), 9)
        });
        assert_eq!(store.drives.len(), 1);
        assert!(store.drives[0].from_repair);
    }

    #[test]
    fn an_entry_without_a_key_is_unconfirmed_for_a_drive_with_a_layout() {
        // a check on a drive whose layout could not name it (no partitions) leaves an entry without a key. Known
        // limitation: two blank drives of one size swapped between two scans cannot be told apart
        let blank = disk("disk6", DECLARED, Vec::new());
        let mut store = Store::default();
        let mut entries = HashMap::new();
        record_change(&mut entries, &mut store, &blank, 1, None, |k| {
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Fake, PART))
        });
        assert!(entries["disk6"].saved.is_none());
        // another drive of the same size with a layout takes that diskN: the entry is unconfirmed for it
        let other = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, "OTHER")]);
        sync(&mut entries, &mut store, std::slice::from_ref(&other), 2, None);
        assert_eq!(view_entry(entries.get("disk6"), &other, 0, false).state, State::Unconfirmed);
    }

    #[test]
    fn save_merges_what_another_instance_wrote() {
        let path = temp_path("merge");
        let a = fp(DECLARED, MIB, PART, "A");
        let b = fp(DECLARED, MIB, PART - MIB, "B");
        let mut one = Store::default();
        one.put(None, a.clone(), fake_loss(PART), 1);
        one.save_to(&path).unwrap();
        // a second instance, started before the first wrote B, saves its own record
        let mut two = Store::load_from(&path);
        let mut other = Store::default();
        other.put(None, b.clone(), fake_loss(PART - MIB), 5);
        other.save_to(&path).unwrap();
        two.put(None, fp(GB, MIB, 300 * MIB, "C"), Knowledge::new(GB), 6);
        two.save_to(&path).unwrap();
        let mut all = Store::load_from(&path);
        assert_eq!(all.drives.len(), 3);
        assert!(all.find(&b, 9).is_some());
        // a key removed by this instance does not come back
        two.put(Some(&a), fp(DECLARED, MIB, PART, "A2"), fake_loss(PART), 10);
        two.save_to(&path).unwrap();
        let mut all = Store::load_from(&path);
        assert!(all.find(&a, 11).is_none());
        assert!(all.find(&fp(DECLARED, MIB, PART, "A2"), 11).is_some());
        cleanup(&path);
    }

    #[test]
    fn view_carries_the_real_size_of_the_loss() {
        let v = view(Some(&repaired_k()), &repaired_disk(), 0);
        assert_eq!(v.loss_real, Some(PART));
        assert_eq!(view(None, &repaired_disk(), 0).loss_real, None);
    }

    // ---- review round 3

    #[test]
    fn review3_unusable_swap_inherits() {
        // A (repaired, record from our repair) is disk6; it is unplugged and a blank (superfloppy) fake B of the same
        // declared size takes disk6 before the next scan
        let a = repaired_disk();
        let mut store = Store::default();
        let mut entries = HashMap::new();
        store.put(None, Fingerprint::of(&a), fake_loss(PART), 1);
        sync(&mut entries, &mut store, std::slice::from_ref(&a), 2, None);
        assert_eq!(view_entry(entries.get("disk6"), &a, 0, false).state, State::Repaired);
        let b = disk("disk6", DECLARED, Vec::new());
        sync(&mut entries, &mut store, std::slice::from_ref(&b), 3, None);
        // the knowledge is kept (a failed repair can leave a disk without a table) but unconfirmed: no plan from it
        let v = view_entry(entries.get("disk6"), &b, 0, false);
        assert_eq!(v.state, State::Unconfirmed);
        assert_eq!((v.value, v.repair.default, v.repair.limit), (0, PlanDefault::None, None));
        assert_eq!(check_fix(&v, "limit", PART, None), Err("errUnconfirmed"));
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(shown(&v))), Err("errUnconfirmed"));
        // a new check replaces what was known about another drive
        record_change(&mut entries, &mut store, &b, 4, None, |k| {
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Fake, 4 * GB))
        });
        assert_eq!(entries["disk6"].knowledge.loss.as_ref().unwrap().real, 4 * GB);
        // A's record is intact
        assert_eq!(store.find(&Fingerprint::of(&a), 5), Some(fake_loss(PART)));
    }

    #[test]
    fn unconfirmed_also_before_the_first_sync_with_a_usable_other_layout() {
        let a = repaired_disk();
        let e = Entry::new(fake_loss(PART), Some(Fingerprint::of(&a)), true);
        let b = disk("disk6", DECLARED, vec![part(MIB, DECLARED - MIB, "B")]);
        assert_eq!(view_entry(Some(&e), &b, 0, false).state, State::Unconfirmed);
        // while a task of this drive runs its layout may change under it: the entry is taken as it is
        assert_eq!(view_entry(Some(&e), &b, 0, true).state, State::Fake);
        assert_eq!(view_entry(Some(&e), &a, 0, false).state, State::Repaired);
        // an unkeyed entry is confirmed only by a layout that cannot name a drive either
        let unkeyed = Entry::new(fake_loss(PART), None, false);
        assert_eq!(view_entry(Some(&unkeyed), &disk("disk6", DECLARED, Vec::new()), 0, false).state, State::Fake);
        assert_eq!(view_entry(Some(&unkeyed), &b, 0, false).state, State::Unconfirmed);
    }

    #[test]
    fn review3_find_failure_keys_record_under_pre_repair_layout() {
        // pre-repair: factory layout with the factory volume UUID (shared by the batch)
        let factory = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, "FACTORY")]);
        let pre = Fingerprint::of(&factory);
        let mut store = Store::default();
        let mut entries = HashMap::new();
        record_change(&mut entries, &mut store, &factory, 1, None, |k| {
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Fake, PART))
        });
        let before = entries.get("disk6").cloned();
        let size = loss_repair_size(PART, DECLARED);
        // disks::find failed after the repair: the drive is taken as one without a partition table
        let mut list = vec![factory.clone()];
        let nameless = Disk { partitions: Vec::new(), ..factory.clone() };
        finish_repair(&mut entries, &mut store, &mut list, before.clone(), nameless, &pre, Repair::Partition(size), 9);
        assert!(store.drives.is_empty(), "nothing is stored under the pre-repair key");
        // the knowledge stays in memory, unconfirmed: no plan comes from it
        assert_eq!(view_entry(entries.get("disk6"), &factory, 0, false).state, State::Unconfirmed);
        // even a layout that still equals the pre-repair one is never used as the new key
        let mut store2 = Store::default();
        let mut entries2 = HashMap::new();
        finish_repair(
            &mut entries2,
            &mut store2,
            &mut list,
            before.clone(),
            factory.clone(),
            &pre,
            Repair::Partition(size),
            9,
        );
        assert!(store2.drives.is_empty());
        // sibling B of the same batch (factory image), plugged in later as disk7, gets nothing
        let b = disk("disk7", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, "FACTORY")]);
        let mut entries3 = HashMap::new();
        sync(&mut entries3, &mut store, std::slice::from_ref(&b), 20, None);
        assert_eq!(view_entry(entries3.get("disk7"), &b, 0, false).state, State::Unknown);
        // a repair whose new layout is usable and new moves the record
        let mut store4 = Store::default();
        let mut entries4 = HashMap::new();
        entries4.insert("disk6".to_string(), before.clone().unwrap());
        let ok = disk("disk6", DECLARED, vec![part(MIB, size, "NEW")]);
        assert!(finish_repair(
            &mut entries4,
            &mut store4,
            &mut list,
            before,
            ok.clone(),
            &pre,
            Repair::Partition(size),
            9
        ));
        assert_eq!(store4.drives.len(), 1);
        assert_eq!(view_entry(entries4.get("disk6"), &ok, 0, false).state, State::Repaired);
    }

    #[test]
    fn review3_partial_pass_on_limited_layout() {
        // a v0.1.2-repaired drive (limited layout, no record); the full check gets an I/O write error after 2 GB
        let d = repaired_disk();
        let r = FullResult {
            total: PART - 40 * MIB,
            used_before: 8 * MIB,
            tested: 2 * GB,
            write_error: Some("Input/output error".into()),
            seconds: 1.0,
            ..Default::default()
        };
        let rec1 = full_record(None, &d, &r, 5);
        assert!(rec1.cut_short);
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(rec1.clone());
        let v = view(Some(&k), &d, 0);
        // only what was written is verified, and the repair plan follows that
        assert_eq!(v.state, State::PartitionOk);
        assert_eq!(v.value, rec1.real);
        assert_eq!(v.repair.limit, Some(rec1.real / MIB * MIB));
        // the same on a repaired drive with a known loss: never "verified"
        let mut k = repaired_k_with_repair(PART - 2 * MIB);
        let d2 = disk("disk6", DECLARED, vec![part(MIB, PART - 2 * MIB, UUID)]);
        let rec2 = full_record(Some(&k), &d2, &FullResult { total: PART - 40 * MIB, ..r.clone() }, 9);
        k.apply_full(rec2);
        let v = view(Some(&k), &d2, 0);
        assert_eq!((v.state, v.verified), (State::Repaired, false));
        // a complete pass does verify
        let rec3 = full_record(Some(&k), &d2, &full_result(PART - 40 * MIB, 0, 0), 10);
        assert!(!rec3.cut_short);
        k.apply_full(rec3);
        assert!(view(Some(&k), &d2, 0).verified);
    }

    #[test]
    fn merge_goes_by_the_time_of_the_change_and_keeps_removals_removed() {
        let path = temp_path("merge2");
        let a = fp(DECLARED, MIB, PART, "A");
        let a2 = fp(DECLARED, MIB, PART, "A2");
        let mut one = Store::default();
        one.put(None, a.clone(), fake_loss(PART), 10);
        one.save_to(&path).unwrap();
        // instance two read the file, looked the record up much later (only `seen` changes), and has not changed it
        let mut two = Store::load_from(&path);
        two.find(&a, 500).unwrap();
        // instance one re-keys the record: its content changed at 20
        one.put(Some(&a), a2.clone(), fake_loss(PART), 20);
        one.save_to(&path).unwrap();
        // instance two saves for another reason: neither the old key comes back nor the newer content is lost
        two.put(None, fp(GB, MIB, 300 * MIB, "C"), Knowledge::new(GB), 30);
        two.save_to(&path).unwrap();
        let mut all = Store::load_from(&path);
        assert!(all.find(&a, 600).is_none(), "the removed key was resurrected");
        assert!(all.find(&a2, 600).is_some());
        assert!(all.find(&fp(GB, MIB, 300 * MIB, "C"), 600).is_some());
        // a record both have: the one changed last wins, whatever `seen` says
        let mut x = Store::default();
        let b = fp(DECLARED, MIB, PART - MIB, "B");
        x.put(None, b.clone(), fake_loss(PART - MIB), 50);
        x.save_to(&path).unwrap();
        let mut y = Store::load_from(&path);
        y.find(&b, 900);
        let newer = Knowledge { loss: Some(Loss { real: 1, verdict: Verdict::Fake }), ..fake_loss(PART) };
        x.put(None, b.clone(), newer.clone(), 60);
        x.save_to(&path).unwrap();
        y.save_to(&path).unwrap();
        assert_eq!(Store::load_from(&path).drives.iter().find(|r| r.fingerprint == b).unwrap().knowledge, newer);
        cleanup(&path);
    }

    #[test]
    fn whole_needs_the_state_the_window_showed() {
        let v = view(Some(&repaired_k()), &repaired_disk(), 0);
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(Shown { state: "repaired", limit: v.repair.limit })), Ok(0));
        assert_eq!(
            check_fix(&v, "whole", DECLARED, Some(Shown { state: "unknown", limit: None })),
            Err("errRepairChanged")
        );
        assert_eq!(
            check_fix(&v, "whole", DECLARED, Some(Shown { state: "repaired", limit: Some(MIB) })),
            Err("errRepairChanged")
        );
    }

    #[test]
    fn a_full_size_repair_of_an_unknown_drive_stores_nothing() {
        let d = full_layout();
        let mut store = Store::default();
        let mut entries = HashMap::new();
        let after = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, "NEW")]);
        let mut list = vec![d.clone()];
        let saved =
            finish_repair(&mut entries, &mut store, &mut list, None, after, &Fingerprint::of(&d), Repair::Whole, 9);
        assert!(!saved && store.drives.is_empty() && entries.is_empty());
    }

    // ---- review round 5: a computed size of 0 never becomes the whole drive

    fn early_write_error(used_before: u64, tested: u64) -> FullResult {
        FullResult {
            total: PART - 40 * MIB,
            used_before,
            tested,
            write_error: Some("Input/output error".into()),
            seconds: 1.0,
            ..Default::default()
        }
    }

    #[test]
    fn early_write_error_on_a_limited_layout_keeps_the_limit_plan() {
        let d = repaired_disk();
        let before = view(None, &d, 0);
        assert_eq!((before.state, before.repair.limit), (State::Limited, Some(PART)));
        // used 32 KiB, written 512 KiB, then the card refuses writes
        let r = early_write_error(32 * 1024, 512 * 1024);
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(full_record(None, &d, &r, 5));
        let v = view(Some(&k), &d, 0);
        assert_eq!(v.repair.default, PlanDefault::Limit);
        assert_eq!(v.repair.limit, Some(PART));
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errRepairChanged"));
    }

    #[test]
    fn early_write_error_with_a_repair_record_and_no_loss_keeps_the_plan() {
        let size = PART - 2 * MIB;
        let d = disk("disk6", DECLARED, vec![part(MIB, size, UUID)]);
        let mut k = Knowledge::new(DECLARED);
        k.apply_repair(Repair::Partition(size), 5);
        let before = view(Some(&k), &d, 0).repair;
        k.apply_full(full_record(Some(&k), &d, &early_write_error(32 * 1024, 512 * 1024), 9));
        let after = view(Some(&k), &d, 0);
        assert_eq!(after.repair, before);
        assert_ne!(after.repair.default, PlanDefault::Whole);
    }

    #[test]
    fn nothing_written_is_a_failed_check_not_a_record() {
        // used 32 KiB, written 0: reported as an error by the application, never recorded
        assert!(wrote_too_little(&early_write_error(32 * 1024, 0)));
        assert!(wrote_too_little(&early_write_error(32 * 1024, 512 * 1024)));
        assert!(!wrote_too_little(&early_write_error(32 * 1024, 2 * MIB)));
        // a complete pass is never "too little", and a bad block is a loss to record
        assert!(!wrote_too_little(&full_result(PART - 40 * MIB, 0, 0)));
        assert!(!wrote_too_little(&FullResult { bad: 1, ..early_write_error(0, 0) }));
        // and the view of such a pass (should it ever reach it) is never the whole drive
        let d = repaired_disk();
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(full_record(None, &d, &early_write_error(32 * 1024, 0), 5));
        let v = view(Some(&k), &d, 0);
        assert_ne!(v.repair.default, PlanDefault::Whole);
    }

    #[test]
    fn a_limited_drive_with_a_sub_mib_partition_has_no_plan() {
        let d = disk("disk6", DECLARED, vec![part(MIB, 512 * 1024, UUID)]);
        let v = view(None, &d, 0);
        assert_eq!(v.state, State::Limited);
        assert_eq!((v.repair.default, v.repair.limit), (PlanDefault::None, None));
        // no plan: the full size only by the explicit tick
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errNoPlan"));
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(shown(&v))), Ok(0));
        assert_eq!(check_fix(&v, "limit", 0, None), Err("errNoPlan"));
    }

    #[test]
    fn a_partition_pass_that_verified_nothing_falls_back_to_the_layout_limit_or_no_plan() {
        // with a limited layout: the layout's limit; without a limit (near-full layout): no plan
        let mut k = Knowledge::new(DECLARED);
        k.apply_full(CheckRecord { cut_short: true, ..rec(Mode::Full, Scope::Partition, Verdict::Genuine, 0) });
        let v = view(Some(&k), &repaired_disk(), 0);
        assert_eq!((v.state, v.repair.default, v.repair.limit), (State::PartitionOk, PlanDefault::Limit, Some(PART)));
        let near = disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, UUID)]);
        let v = view(Some(&k), &near, 0);
        assert_eq!((v.state, v.repair.default), (State::PartitionOk, PlanDefault::None));
    }

    // ---- review round 6

    /// The state x tick matrix: where a plan and a full-size repair come from.
    #[test]
    fn full_size_is_a_default_only_for_a_genuine_drive() {
        let genuine = {
            let mut k = Knowledge::new(DECLARED);
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Genuine, DECLARED));
            view(Some(&k), &repaired_disk(), 0)
        };
        let full_genuine = {
            let mut k = Knowledge::new(DECLARED);
            let d = full_layout();
            k.apply_full(full_record(None, &d, &full_result(DECLARED - 40 * MIB, 0, 0), 5));
            view(Some(&k), &d, 0)
        };
        let unknown = view(None, &full_layout(), 0);
        let limited = view(None, &repaired_disk(), 0);
        let fake = view(Some(&fake_loss(PART)), &full_layout(), 0);
        let repaired = view(Some(&repaired_k()), &repaired_disk(), 0);
        let partition_ok = {
            let mut k = Knowledge::new(DECLARED);
            k.apply_full(rec(Mode::Full, Scope::Partition, Verdict::Genuine, PART));
            view(Some(&k), &repaired_disk(), 0)
        };
        let broken = view(Some(&fake_loss(MIB)), &repaired_disk(), 0);
        let unconfirmed = {
            let e = Entry::new(fake_loss(PART), Some(Fingerprint::of(&repaired_disk())), true);
            view_entry(Some(&e), &disk("disk6", DECLARED, Vec::new()), 0, false)
        };
        // (view, default, whole without the tick, whole with the tick)
        type Case<'a> = (&'a str, &'a View, PlanDefault, Result<u64, &'a str>, Result<u64, &'a str>);
        let cases: [Case; 9] = [
            ("genuine", &genuine, PlanDefault::Whole, Ok(0), Ok(0)),
            ("genuine by a full check", &full_genuine, PlanDefault::Whole, Ok(0), Ok(0)),
            ("unknown", &unknown, PlanDefault::None, Err("errNoPlan"), Ok(0)),
            ("limited", &limited, PlanDefault::Limit, Err("errRepairChanged"), Ok(0)),
            ("fake", &fake, PlanDefault::Limit, Err("errRepairChanged"), Ok(0)),
            ("repaired", &repaired, PlanDefault::Limit, Err("errRepairChanged"), Ok(0)),
            ("partition verified", &partition_ok, PlanDefault::Limit, Err("errRepairChanged"), Ok(0)),
            ("broken", &broken, PlanDefault::None, Err("errBroken"), Err("errBroken")),
            ("unconfirmed", &unconfirmed, PlanDefault::None, Err("errUnconfirmed"), Err("errUnconfirmed")),
        ];
        for (name, v, default, no_tick, tick) in cases {
            assert_eq!(v.repair.default, default, "{name}: default");
            assert_eq!(check_fix(v, "whole", DECLARED, None), no_tick, "{name}: no tick");
            assert_eq!(check_fix(v, "whole", DECLARED, Some(shown(v))), tick, "{name}: tick");
            // a tick made in another state never counts
            if default != PlanDefault::Whole && tick == Ok(0) {
                let other = Shown { state: "someOtherState", limit: v.repair.limit };
                assert_ne!(check_fix(v, "whole", DECLARED, Some(other)), Ok(0), "{name}: stale tick");
            }
        }
    }

    #[test]
    fn zz_used_volume_pass_clears_known_loss() {
        // 120 GB "in use" (files copied onto the fake), the free 5.8 GB is written and reads back (it wraps onto the
        // real cells of the old files): not a whole-device check
        let size = DECLARED - 2 * MIB;
        let d = disk("disk6", DECLARED, vec![part(MIB, size, UUID)]);
        let mut k = fake_loss(PART);
        let v0 = view(Some(&k), &d, 0);
        assert_eq!((v0.state, v0.repair.default), (State::Fake, PlanDefault::Limit));
        let free = 5_800_000_000u64;
        let r = FullResult {
            total: size,
            used_before: size - free,
            tested: free - 8 * MIB,
            seconds: 1.0,
            ..Default::default()
        };
        let rec = full_record(Some(&k), &d, &r, 5);
        assert_eq!((rec.scope, rec.nearly_full), (Scope::Partition, true));
        k.apply_full(rec);
        let v = view(Some(&k), &d, 0);
        assert_eq!((v.state, v.repair.default), (State::Fake, PlanDefault::Limit));
        assert_eq!(v.loss_real, Some(PART));
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errRepairChanged"));
        // without a known loss the pass is not "verified" either: not checked, no plan
        let mut k2 = Knowledge::new(DECLARED);
        k2.apply_full(full_record(None, &d, &r, 5));
        let v2 = view(Some(&k2), &d, 0);
        assert_eq!((v2.state, v2.repair.default), (State::Unknown, PlanDefault::None));
        // a nearly empty volume is a whole-device check
        let empty = FullResult {
            used_before: 32 * 1024,
            tested: size - 8 * MIB - 32 * 1024,
            total: size,
            ..Default::default()
        };
        assert_eq!(full_record(None, &d, &empty, 5).scope, Scope::Device);
    }

    #[test]
    fn zz_sweep_full_results_never_whole_for_known_or_limited() {
        // sweep used/tested/write_error over a repaired disk and a known-loss full-size disk; a drive with a limit
        // or a record never gets the full size as a default (the nearly empty, complete pass of a full-size volume is
        // a legitimate whole-device check and is left out)
        let layouts = [repaired_disk(), disk("disk6", DECLARED, vec![part(MIB, DECLARED - 2 * MIB, UUID)])];
        let mut hits = Vec::new();
        for (li, d) in layouts.iter().enumerate() {
            let total = d.partitions[0].size;
            for kind in 0..3 {
                let base = match kind {
                    0 => fake_loss(PART),
                    1 => {
                        let mut k = fake_loss(PART);
                        k.apply_repair(Repair::Partition(PART - 2 * MIB), 3);
                        k
                    }
                    _ => Knowledge::new(DECLARED),
                };
                for used in [0u64, 32 * 1024, MIB, GB, total / 2, total - 2 * GB, total - 100 * MIB] {
                    for frac in [0u64, 1, 10, 50, 99, 100] {
                        for err in [false, true] {
                            let tested = (total - used).saturating_sub(8 * MIB) / 100 * frac;
                            let r = FullResult {
                                total,
                                used_before: used,
                                tested,
                                write_error: err.then(|| "e".into()),
                                seconds: 1.0,
                                ..Default::default()
                            };
                            if wrote_too_little(&r) {
                                continue;
                            }
                            let mut k = base.clone();
                            k.apply_full(full_record(Some(&k), d, &r, 5));
                            let v = view(Some(&k), d, 0);
                            let known = base.loss.is_some() || base.repair.is_some();
                            let legit = !nearly_full(DECLARED, used)
                                && !err
                                && frac == 100
                                && layout_limit(d).is_none()
                                && !known;
                            if (layout_limit(d).is_some() || known) && v.repair.default == PlanDefault::Whole && !legit
                            {
                                hits.push((li, kind, used, frac, err, v.state));
                            }
                        }
                    }
                }
            }
        }
        // a fake with a loss whose whole-device check later passes completely is genuine: the loss is cleared
        hits.retain(|h| !(h.1 == 0 && !nearly_full(DECLARED, h.2) && !h.4 && h.3 == 100 && h.0 == 1));
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn zz_table_unreadable_hides_a_stored_repaired_fake() {
        let d = repaired_disk();
        let mut k = fake_loss(PART + 3 * MIB);
        k.apply_repair(Repair::Partition(PART), 5);
        let mut store = Store::default();
        store.put(None, Fingerprint::of(&d), k, 1);
        let mut entries = HashMap::new();
        // the scan lost the partition table (one partition's diskutil info timed out)
        let hiccup = Disk { table_unreadable: true, ..disk("disk6", DECLARED, Vec::new()) };
        sync(&mut entries, &mut store, std::slice::from_ref(&hiccup), 10, None);
        let v = view_entry(entries.get("disk6"), &hiccup, 0, false);
        assert_eq!((v.state, v.repair.default), (State::Unconfirmed, PlanDefault::None));
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errUnconfirmed"));
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(shown(&v))), Err("errUnconfirmed"));
        // with the entry in memory too
        sync(&mut entries, &mut store, std::slice::from_ref(&d), 11, None);
        assert_eq!(view_entry(entries.get("disk6"), &hiccup, 0, false).state, State::Unconfirmed);
        assert_eq!(view_entry(entries.get("disk6"), &d, 0, false).state, State::Repaired);
    }

    #[test]
    fn a_record_with_a_loss_of_the_whole_size_is_ignored() {
        let d = repaired_disk();
        let mut store = Store::default();
        store.put(None, Fingerprint::of(&d), fake_loss(DECLARED), 1);
        assert!(store.find(&Fingerprint::of(&d), 2).is_none());
        store.put(None, Fingerprint::of(&d), fake_loss(PART), 3);
        assert!(store.find(&Fingerprint::of(&d), 4).is_some());
    }

    // ---- review round 7

    #[test]
    fn a_blank_disk_is_not_unreadable_and_offers_the_tick() {
        // a blank disk (no partitions, flag not set): not checked, no plan, the full size only by the tick
        let blank = disk("disk6", DECLARED, Vec::new());
        assert!(!blank.table_unreadable);
        let v = view_entry(None, &blank, 0, false);
        assert_eq!((v.state, v.repair.default), (State::Unknown, PlanDefault::None));
        assert_eq!(check_fix(&v, "whole", DECLARED, None), Err("errNoPlan"));
        assert_eq!(check_fix(&v, "whole", DECLARED, Some(shown(&v))), Ok(0));
        // an unreadable table is Unconfirmed for as long as it stays unreadable; a check made meanwhile does not
        // change that, and the knowledge shows again when the table reads fine
        let bad = Disk { table_unreadable: true, ..blank.clone() };
        let mut store = Store::default();
        let mut entries = HashMap::new();
        record_change(&mut entries, &mut store, &bad, 1, None, |k| {
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Genuine, DECLARED))
        });
        assert_eq!(view_entry(entries.get("disk6"), &bad, 0, false).state, State::Unconfirmed);
        assert_eq!(view_entry(entries.get("disk6"), &blank, 0, false).state, State::Genuine);
    }

    #[test]
    fn a_vanished_drive_loses_its_knowledge_during_a_task_of_another_drive() {
        // drive A (keyless, known genuine) is disk6; disk7 runs a long check. A is unplugged; a scan during the task
        // must drop A's entry, so a keyless fake B that takes disk6 does not show A's "genuine"
        let a = disk("disk6", DECLARED, Vec::new());
        let task_drive = disk("disk7", GB, Vec::new());
        let mut store = Store::default();
        let mut entries = HashMap::new();
        record_change(&mut entries, &mut store, &a, 1, None, |k| {
            k.apply_quick(rec(Mode::Quick, Scope::Device, Verdict::Genuine, DECLARED))
        });
        record_change(&mut entries, &mut store, &task_drive, 1, None, |k| *k = Knowledge::new(GB));
        // the scan during the task: disk6 is gone
        assert!(sync(&mut entries, &mut store, std::slice::from_ref(&task_drive), 5, Some("disk7")));
        assert!(!entries.contains_key("disk6"));
        assert!(entries.contains_key("disk7"), "the task's own drive is left alone");
        // the task's own drive is kept even when the scan does not list it (the task decides)
        sync(&mut entries, &mut store, &[], 6, Some("disk7"));
        assert!(entries.contains_key("disk7"));
        // B takes disk6
        let b = disk("disk6", DECLARED, Vec::new());
        sync(&mut entries, &mut store, &[b.clone(), task_drive], 7, Some("disk7"));
        assert_eq!(view_entry(entries.get("disk6"), &b, 0, false).state, State::Unknown);
    }
}
