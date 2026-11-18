//! Bounded, read-only installation snapshots. Changes are correlations, not causal proof.
use crate::model::{AppRecord, AppResult, MonitorRecord};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 4_000;
const MAX_REGISTRY: usize = 4_000;
const MAX_WARNINGS: usize = 100;
const BUDGET: Duration = Duration::from_secs(5);
static SERIAL: AtomicU64 = AtomicU64::new(0);
static LIFECYCLE: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub path: String,
    pub modified_ns: Option<String>,
    pub created_ns: Option<String>,
    pub reparse_point: bool,
    pub depth: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub captured_at: String,
    pub registry: Vec<AppRecord>,
    pub directories: Vec<DirectoryEntry>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObservedDiff {
    pub registry_added: Vec<AppRecord>,
    pub registry_removed: Vec<AppRecord>,
    pub registry_changed: Vec<AppRecord>,
    pub directories_added: Vec<DirectoryEntry>,
    pub directories_removed: Vec<DirectoryEntry>,
    pub directories_changed: Vec<DirectoryEntry>,
    pub warnings: Vec<String>,
    pub complete: bool,
    pub proven_causal_attribution: bool,
    pub statement: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredMonitor {
    record: MonitorRecord,
    baseline: Snapshot,
    final_snapshot: Option<Snapshot>,
    comparison: Option<ObservedDiff>,
}

fn now() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(t) => format!("Unix {}.{:03} UTC", t.as_secs(), t.subsec_millis()),
        Err(_) => "Clock before Unix epoch".into(),
    }
}

fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "mon_{nanos:x}_{:x}_{:x}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}

fn validate_id(id: &str) -> AppResult<()> {
    if id.is_empty()
        || id.len() > 120
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("Invalid monitor ID.".into());
    }
    Ok(())
}

fn storage_root() -> AppResult<PathBuf> {
    let root = std::env::var_os("LOCALAPPDATA")
        .ok_or("LOCALAPPDATA is unavailable; monitor storage cannot be selected.")?;
    Ok(PathBuf::from(root)
        .join("SimpleDelete")
        .join("native-monitors"))
}

fn warn(snapshot: &mut Snapshot, message: String) {
    if snapshot.warnings.len() < MAX_WARNINGS {
        snapshot.warnings.push(message);
    } else if snapshot.warnings.len() == MAX_WARNINGS {
        snapshot
            .warnings
            .push("Additional coverage warnings were omitted.".into());
    }
}

#[cfg(windows)]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}
#[cfg(not(windows))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn timestamp(time: std::io::Result<SystemTime>) -> Option<String> {
    time.ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|t| t.as_nanos().to_string())
}

fn snapshot_roots() -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    [
        "LOCALAPPDATA",
        "APPDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .map(PathBuf::from)
    .filter(|p| seen.insert(p.to_string_lossy().to_lowercase()))
    .collect()
}

fn capture_directories(
    snapshot: &mut Snapshot,
    roots: Vec<PathBuf>,
    max_entries: usize,
    budget: Duration,
) {
    let started = Instant::now();
    let mut queue: VecDeque<_> = roots.into_iter().map(|root| (root, 0usize)).collect();
    let mut examined = 0usize;
    while let Some((directory, depth)) = queue.pop_front() {
        if snapshot.directories.len() >= max_entries || started.elapsed() >= budget {
            snapshot.truncated = true;
            warn(
                snapshot,
                "Directory entry/time limit reached; snapshot coverage is partial.".into(),
            );
            break;
        }
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(m) => m,
            Err(e) => {
                warn(
                    snapshot,
                    format!("Cannot read {}: {e}", directory.display()),
                );
                continue;
            }
        };
        if is_reparse(&metadata) {
            warn(
                snapshot,
                format!("Not traversing reparse point {}", directory.display()),
            );
            continue;
        }
        let children = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) => {
                warn(
                    snapshot,
                    format!("Cannot enumerate {}: {e}", directory.display()),
                );
                continue;
            }
        };
        for child in children {
            examined += 1;
            if examined > max_entries.saturating_mul(20)
                || snapshot.directories.len() >= max_entries
                || started.elapsed() >= budget
            {
                snapshot.truncated = true;
                warn(
                    snapshot,
                    "Directory enumeration bound reached; snapshot coverage is partial.".into(),
                );
                return;
            }
            let child = match child {
                Ok(c) => c,
                Err(e) => {
                    warn(
                        snapshot,
                        format!("Cannot inspect an entry in {}: {e}", directory.display()),
                    );
                    continue;
                }
            };
            let metadata = match fs::symlink_metadata(child.path()) {
                Ok(m) => m,
                Err(e) => {
                    warn(
                        snapshot,
                        format!("Cannot read {}: {e}", child.path().display()),
                    );
                    continue;
                }
            };
            let reparse = is_reparse(&metadata);
            if !metadata.is_dir() && !reparse {
                continue;
            }
            let child_depth = depth + 1;
            snapshot.directories.push(DirectoryEntry {
                path: child.path().to_string_lossy().into_owned(),
                modified_ns: timestamp(metadata.modified()),
                created_ns: timestamp(metadata.created()),
                reparse_point: reparse,
                depth: child_depth,
            });
            if reparse {
                warn(
                    snapshot,
                    format!("Not traversing reparse point {}", child.path().display()),
                );
            } else if child_depth < 2 {
                queue.push_back((child.path(), child_depth));
            }
        }
    }
}

fn capture() -> AppResult<Snapshot> {
    let mut snapshot = Snapshot {
        captured_at: now(),
        ..Snapshot::default()
    };
    snapshot.registry = crate::backend::inventory()?;
    if snapshot.registry.len() > MAX_REGISTRY {
        snapshot.registry.truncate(MAX_REGISTRY);
        snapshot.truncated = true;
        warn(
            &mut snapshot,
            "Uninstall registry entry limit reached; registry coverage is partial.".into(),
        );
    }
    let roots = snapshot_roots();
    if roots.is_empty() {
        warn(
            &mut snapshot,
            "No Windows application-data roots were found.".into(),
        );
    }
    capture_directories(&mut snapshot, roots, MAX_ENTRIES, BUDGET);
    Ok(snapshot)
}

fn app_metadata(app: &AppRecord) -> (&str, &str, &str, u64, &str, &str, &str) {
    (
        &app.name,
        &app.publisher,
        &app.version,
        app.size_mb.to_bits(),
        &app.installed,
        &app.location,
        &app.command,
    )
}

/// Pure metadata comparison. An apparent removal in partial coverage is not proof of deletion.
pub fn compare_snapshots(before: &Snapshot, after: &Snapshot) -> ObservedDiff {
    let mut diff = ObservedDiff {
        complete: !before.truncated && !after.truncated && before.warnings.is_empty() && after.warnings.is_empty(),
        proven_causal_attribution: false,
        statement: "These are observed differences between two bounded metadata snapshots. Concurrent updates can appear here. A matching path or name does not prove that an installer caused a change; file contents, services and drivers are not tracked.".into(),
        ..ObservedDiff::default()
    };
    let before_apps: BTreeMap<_, _> = before
        .registry
        .iter()
        .map(|a| (a.id.to_lowercase(), a))
        .collect();
    let after_apps: BTreeMap<_, _> = after
        .registry
        .iter()
        .map(|a| (a.id.to_lowercase(), a))
        .collect();
    for (id, app) in &after_apps {
        match before_apps.get(id) {
            None => diff.registry_added.push((*app).clone()),
            Some(old) if app_metadata(old) != app_metadata(app) => {
                diff.registry_changed.push((*app).clone())
            }
            _ => {}
        }
    }
    for (id, app) in &before_apps {
        if !after_apps.contains_key(id) {
            diff.registry_removed.push((*app).clone());
        }
    }
    let before_dirs: BTreeMap<_, _> = before
        .directories
        .iter()
        .map(|d| (d.path.to_lowercase(), d))
        .collect();
    let after_dirs: BTreeMap<_, _> = after
        .directories
        .iter()
        .map(|d| (d.path.to_lowercase(), d))
        .collect();
    for (id, dir) in &after_dirs {
        match before_dirs.get(id) {
            None => diff.directories_added.push((*dir).clone()),
            Some(old) if *old != *dir => diff.directories_changed.push((*dir).clone()),
            _ => {}
        }
    }
    for (id, dir) in &before_dirs {
        if !after_dirs.contains_key(id) {
            diff.directories_removed.push((*dir).clone());
        }
    }
    diff.warnings
        .extend(before.warnings.iter().map(|w| format!("Baseline: {w}")));
    diff.warnings
        .extend(after.warnings.iter().map(|w| format!("Final: {w}")));
    if before.truncated || after.truncated {
        diff.warnings.push("At least one snapshot was truncated. Apparent additions/removals can be coverage artifacts, and the diff is incomplete.".into());
    }
    diff
}

fn file_path(root: &Path, id: &str) -> AppResult<PathBuf> {
    validate_id(id)?;
    Ok(root.join(format!("{id}.json")))
}

fn read_stored(root: &Path, id: &str) -> AppResult<StoredMonitor> {
    let file = file_path(root, id)?;
    let bytes = fs::read(&file).map_err(|e| format!("Cannot read monitor {id}: {e}"))?;
    let stored: StoredMonitor =
        serde_json::from_slice(&bytes).map_err(|e| format!("Monitor {id} is invalid: {e}"))?;
    if stored.record.id != id {
        return Err("Monitor identity does not match its storage file.".into());
    }
    Ok(stored)
}

fn write_stored(root: &Path, stored: &StoredMonitor) -> AppResult<()> {
    fs::create_dir_all(root).map_err(|e| format!("Cannot create monitor storage: {e}"))?;
    let file = file_path(root, &stored.record.id)?;
    let temp = root.join(format!(
        "{}.{}.tmp",
        stored.record.id,
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec_pretty(stored).map_err(|e| e.to_string())?;
    fs::write(&temp, bytes).map_err(|e| format!("Cannot save monitor: {e}"))?;
    fs::rename(&temp, &file).map_err(|e| format!("Cannot finish saving monitor: {e}"))?;
    Ok(())
}

fn start_at(root: &Path, label: &str, snapshot: Snapshot) -> AppResult<MonitorRecord> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 120 {
        return Err("Enter a session name between 1 and 120 characters.".into());
    }
    let record = MonitorRecord {
        id: new_id(),
        label: label.into(),
        status: "active".into(),
        started_at: snapshot.captured_at.clone(),
        warnings: snapshot.warnings.clone(),
        report: format!(
            "Baseline saved: {} uninstall identities, {} directory records. Run the installer yourself, then finish this session. Capture covers directory metadata to depth 2, at most {MAX_ENTRIES} directories and {MAX_REGISTRY} registry identities. It is not continuous process monitoring.",
            snapshot.registry.len(),
            snapshot.directories.len()
        ),
    };
    write_stored(
        root,
        &StoredMonitor {
            record: record.clone(),
            baseline: snapshot,
            final_snapshot: None,
            comparison: None,
        },
    )?;
    Ok(record)
}

fn stop_at(root: &Path, id: &str, snapshot: Snapshot) -> AppResult<MonitorRecord> {
    let mut stored = read_stored(root, id)?;
    if stored.record.status != "active" {
        return Err("This monitoring session has already finished.".into());
    }
    let comparison = compare_snapshots(&stored.baseline, &snapshot);
    stored.record.status = "stopped".into();
    stored.record.warnings = comparison.warnings.clone();
    stored.record.report = serde_json::to_string_pretty(&comparison).map_err(|e| e.to_string())?;
    stored.final_snapshot = Some(snapshot);
    stored.comparison = Some(comparison);
    write_stored(root, &stored)?;
    Ok(stored.record)
}

pub fn start_monitor(label: &str) -> AppResult<MonitorRecord> {
    let _guard = LIFECYCLE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Monitor lock was poisoned.")?;
    if label.trim().is_empty() || label.chars().count() > 120 {
        return Err("Enter a session name between 1 and 120 characters.".into());
    }
    start_at(&storage_root()?, label, capture()?)
}

pub fn stop_monitor(id: &str) -> AppResult<MonitorRecord> {
    let _guard = LIFECYCLE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Monitor lock was poisoned.")?;
    let root = storage_root()?;
    if read_stored(&root, id)?.record.status != "active" {
        return Err("This monitoring session has already finished.".into());
    }
    stop_at(&root, id, capture()?)
}

pub fn list_monitors() -> AppResult<Vec<MonitorRecord>> {
    let root = storage_root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| format!("Cannot list monitoring sessions: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let id = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        records.push(read_stored(&root, &id)?.record);
    }
    records.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(records)
}

pub fn get_monitor(id: &str) -> AppResult<MonitorRecord> {
    Ok(read_stored(&storage_root()?, id)?.record)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(id: &str, version: &str) -> AppRecord {
        AppRecord {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            ..Default::default()
        }
    }
    fn dir(path: &str, modified: &str) -> DirectoryEntry {
        DirectoryEntry {
            path: path.into(),
            modified_ns: Some(modified.into()),
            depth: 1,
            ..Default::default()
        }
    }

    #[test]
    fn diffs_metadata_without_claiming_causality() {
        let before = Snapshot {
            registry: vec![app("same", "1"), app("changed", "1"), app("gone", "1")],
            directories: vec![
                dir("C:\\Same", "1"),
                dir("C:\\Changed", "1"),
                dir("C:\\Gone", "1"),
            ],
            ..Default::default()
        };
        let after = Snapshot {
            registry: vec![app("same", "1"), app("changed", "2"), app("new", "1")],
            directories: vec![
                dir("C:\\Same", "1"),
                dir("C:\\Changed", "2"),
                dir("C:\\New", "1"),
            ],
            ..Default::default()
        };
        let diff = compare_snapshots(&before, &after);
        assert_eq!(diff.registry_added.len(), 1);
        assert_eq!(diff.registry_changed.len(), 1);
        assert_eq!(diff.registry_removed.len(), 1);
        assert_eq!(diff.directories_added.len(), 1);
        assert_eq!(diff.directories_changed.len(), 1);
        assert_eq!(diff.directories_removed.len(), 1);
        assert!(diff.complete);
        assert!(!diff.proven_causal_attribution);
    }

    #[test]
    fn partial_snapshots_keep_diff_limits_visible() {
        let before = Snapshot {
            truncated: true,
            warnings: vec!["Access denied".into()],
            ..Default::default()
        };
        let diff = compare_snapshots(&before, &Snapshot::default());
        assert!(!diff.complete);
        assert!(
            diff.warnings
                .iter()
                .any(|w| w.contains("coverage artifacts"))
        );
        assert!(!diff.proven_causal_attribution);
    }

    #[test]
    fn ids_cannot_escape_storage() {
        for id in ["../other", "C:\\outside", "a/b", "", "a.b"] {
            assert!(validate_id(id).is_err());
        }
        assert!(validate_id("mon_123_ab").is_ok());
    }

    #[test]
    fn lifecycle_persists_fixture_snapshots_and_refuses_second_stop() {
        let root = std::env::temp_dir().join(new_id());
        let baseline = Snapshot {
            captured_at: "fixture baseline".into(),
            registry: vec![app("old", "1")],
            ..Default::default()
        };
        let record = start_at(&root, " Example ", baseline).unwrap();
        assert_eq!(record.label, "Example");
        assert_eq!(record.status, "active");
        let final_snapshot = Snapshot {
            captured_at: "fixture final".into(),
            registry: vec![app("new", "1")],
            ..Default::default()
        };
        let stopped = stop_at(&root, &record.id, final_snapshot.clone()).unwrap();
        assert_eq!(stopped.status, "stopped");
        assert!(stopped.report.contains("registry_added"));
        assert!(
            read_stored(&root, &record.id)
                .unwrap()
                .final_snapshot
                .is_some()
        );
        assert!(stop_at(&root, &record.id, final_snapshot).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_capture_respects_entry_bound() {
        let root = std::env::temp_dir().join(new_id());
        fs::create_dir_all(root.join("one").join("nested")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        let mut snapshot = Snapshot::default();
        capture_directories(&mut snapshot, vec![root.clone()], 1, Duration::from_secs(2));
        assert_eq!(snapshot.directories.len(), 1);
        assert!(snapshot.truncated);
        fs::remove_dir_all(root).unwrap();
    }
}
