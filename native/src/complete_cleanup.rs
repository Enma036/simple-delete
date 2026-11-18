use crate::model::{AppRecord, AppResult};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::env;
use std::fs::{self, Metadata};
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, SystemTime};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;

pub const PAGE: &str = "Complete Clean Up";
pub const BACKUP_KIND: &str = "drive file";
pub const TEMP_DAYS: u64 = 30;
pub const REVIEW_DAYS: [u64; 3] = [90, 180, 365];
pub const RESULT_LIMIT: usize = 1000;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ScanMode {
    Review,
    Automatic,
}

const PROTECTED_ATTRIBUTES: u32 = FILE_ATTRIBUTE_HIDDEN
    | FILE_ATTRIBUTE_SYSTEM
    | FILE_ATTRIBUTE_REPARSE_POINT
    | FILE_ATTRIBUTE_OFFLINE;
const PROTECTED_DIRS: &[&str] = &[
    "windows",
    "windows.old",
    "program files",
    "program files (x86)",
    "programdata",
    "windowsapps",
    "wpsystem",
    "winsxs",
    "system volume information",
    "$recycle.bin",
    "recovery",
    "boot",
    "efi",
    "steamapps",
    "node_modules",
    ".git",
    ".svn",
    ".hg",
    ".simple-delete-quarantine",
    "simpledelete",
];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FileKind {
    Temporary,
    Older,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DriveCandidate {
    pub bytes: u64,
    pub path: PathBuf,
    pub kind: FileKind,
    pub days: u64,
    created: u64,
    modified: u64,
}

impl DriveCandidate {
    pub fn still_matches(&self, metadata: &Metadata, older_days: u64) -> bool {
        eligible_file(metadata)
            && metadata.len() == self.bytes
            && metadata.creation_time() == self.created
            && metadata.last_write_time() == self.modified
            && age_days(metadata, SystemTime::now())
                .is_some_and(|days| days >= self.kind.minimum_age(older_days))
    }
}

impl FileKind {
    fn minimum_age(self, older_days: u64) -> u64 {
        match self {
            Self::Temporary => TEMP_DAYS,
            Self::Older => older_days,
        }
    }
}

#[derive(Default)]
pub struct ScanProgress {
    pub inspected: AtomicU64,
    pub cancelled: AtomicBool,
}

pub struct DriveScan {
    pub drive: u8,
    pub older_days: u64,
    pub inspected: u64,
    pub protected: u64,
    pub unreadable: u64,
    pub omitted: u64,
    pub candidates: Vec<DriveCandidate>,
}

pub fn available_drives() -> Vec<u8> {
    let mask = unsafe { GetLogicalDrives() };
    (b'A'..=b'Z')
        .filter(|&drive| mask & (1 << (drive - b'A')) != 0 && is_fixed(drive))
        .collect()
}

fn is_fixed(drive: u8) -> bool {
    let root = [drive as u16, ':' as u16, '\\' as u16, 0];
    if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
        return false;
    }
    // A SUBST letter can look like a fixed drive while exposing a protected subtree.
    let path = PathBuf::from(format!("{}:\\", char::from(drive)));
    fs::canonicalize(&path).is_ok_and(|resolved| {
        let resolved = crate::backend::normalize_windows_path(&resolved);
        resolved.file_name().is_none() && drive_letter(&resolved) == Some(drive)
    })
}

// Only ordinary absolute drive paths can be recorded, reopened, or restored.
pub fn drive_letter(path: &Path) -> Option<u8> {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return None;
    };
    let Prefix::Disk(drive) = prefix.kind() else {
        return None;
    };
    if !matches!(parts.next(), Some(Component::RootDir))
        || !parts.all(|part| matches!(part, Component::Normal(name) if ordinary_name(name)))
    {
        return None;
    }
    Some(drive)
}

pub fn local_file_drive(path: &Path) -> Option<u8> {
    path.file_name()?;
    drive_letter(path).filter(|&drive| is_fixed(drive))
}

fn ordinary_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| !name.ends_with(['.', ' ']) && !name.contains([':', '\\', '/', '\0']))
}

pub(crate) fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

pub struct DrivePolicy {
    profile: Option<PathBuf>,
    profiles: PathBuf,
    temp: Option<String>,
    protected: HashSet<String>,
    ignored_files: HashSet<String>,
}

impl DrivePolicy {
    pub fn new(drive: u8, apps: &[AppRecord]) -> Self {
        let profile = env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .filter(|path| drive_letter(path).is_some() && path.file_name().is_some());
        let profiles = profile
            .as_deref()
            .and_then(Path::parent)
            .filter(|path| path.file_name().is_some())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                let drive = env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
                PathBuf::from(format!(r"{drive}\Users"))
            });
        let temp = profile
            .as_ref()
            .map(|path| path_key(&path.join(r"AppData\Local\Temp")));
        let mut protected = HashSet::new();
        for location in apps
            .iter()
            .map(|app| PathBuf::from(app.location.trim().trim_matches('"')))
            .chain(
                [
                    "SystemRoot",
                    "ProgramFiles",
                    "ProgramFiles(x86)",
                    "ProgramW6432",
                    "ProgramData",
                ]
                .into_iter()
                .filter_map(env::var_os)
                .map(PathBuf::from),
            )
        {
            if !location.is_absolute() {
                continue;
            }
            let normalized = crate::backend::normalize_windows_path(&location);
            if drive_letter(&normalized) != Some(drive) || normalized.file_name().is_none() {
                continue;
            }
            protected.insert(path_key(&normalized));
            if let Ok(resolved) = fs::canonicalize(&normalized) {
                protected.insert(path_key(&crate::backend::normalize_windows_path(&resolved)));
            }
        }
        Self {
            profile,
            profiles,
            temp,
            protected,
            ignored_files: HashSet::new(),
        }
    }

    pub fn ignore_files(&mut self, paths: HashSet<String>) {
        self.ignored_files = paths;
    }

    fn protected_directory(&self, path: &Path) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        let key = path_key(path);
        if PROTECTED_DIRS.contains(&name.as_str()) || self.protected.contains(&key) {
            return true;
        }
        if let Some(parent) = path.parent()
            && (path_key(parent) == path_key(&self.profiles)
                || parent
                    .file_name()
                    .is_some_and(|part| part.eq_ignore_ascii_case("Users"))
                    && parent
                        .parent()
                        .is_some_and(|root| root.file_name().is_none()))
        {
            return !self
                .profile
                .as_deref()
                .is_some_and(|profile| path_key(profile) == key);
        }
        (name == "appdata"
            || path.ancestors().skip(1).any(|part| {
                part.file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case("AppData"))
            }))
            && !self
                .temp
                .as_deref()
                .is_some_and(|temp| paths_overlap(&key, temp))
    }

    pub fn classify(&self, path: &Path) -> Option<FileKind> {
        if drive_letter(path).is_none()
            || path
                .ancestors()
                .skip(1)
                .any(|parent| self.protected_directory(parent))
        {
            return None;
        }
        self.classify_in_directory(path, self.in_temp(path.parent()?))
    }

    fn in_temp(&self, directory: &Path) -> bool {
        self.temp
            .as_deref()
            .is_some_and(|temp| at_or_below(&path_key(directory), temp))
    }

    pub fn directory_attributes_allowed(&self, directory: &Path, attributes: u32) -> bool {
        let mut excluded = PROTECTED_ATTRIBUTES;
        let key = path_key(directory);
        if self
            .temp
            .as_deref()
            .and_then(|temp| temp.strip_prefix(&key))
            .is_some_and(|rest| matches!(rest, r"\local\temp" | r"\temp"))
        {
            // Windows can mark AppData and Local hidden or system. Only the Temp branch is scanned.
            excluded &= !(FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
        }
        attributes & excluded == 0
    }

    fn classify_in_directory(&self, path: &Path, in_temp: bool) -> Option<FileKind> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        if in_temp {
            return matches!(extension.as_str(), "tmp" | "temp").then_some(FileKind::Temporary);
        }
        if path.ancestors().skip(1).any(|parent| {
            parent
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("AppData"))
        }) {
            return None;
        }
        match extension.as_str() {
            "7z" | "zip" | "rar" | "tar" | "gz" | "iso" | "msi" | "msix" | "msixbundle" | "pdf"
            | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "jpg" | "jpeg" | "png" | "heic"
            | "webp" | "mp4" | "mov" | "mkv" | "mp3" | "wav" | "flac" => Some(FileKind::Older),
            _ => None,
        }
    }
}

fn at_or_below(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('\\'))
}

fn paths_overlap(a: &str, b: &str) -> bool {
    at_or_below(a, b) || at_or_below(b, a)
}

pub fn eligible_file(metadata: &Metadata) -> bool {
    metadata.is_file()
        && metadata.file_attributes() & (PROTECTED_ATTRIBUTES | FILE_ATTRIBUTE_READONLY) == 0
}

fn age_days(metadata: &Metadata, now: SystemTime) -> Option<u64> {
    if metadata.creation_time() == 0
        || metadata.last_write_time() == 0
        || metadata.last_access_time() == 0
    {
        return None;
    }
    let latest = metadata
        .created()
        .ok()?
        .max(metadata.modified().ok()?)
        .max(metadata.accessed().ok()?);
    Some(now.duration_since(latest).ok()?.as_secs() / Duration::from_hours(24).as_secs())
}

pub fn scan(
    drive: u8,
    older_days: u64,
    mode: ScanMode,
    policy: &DrivePolicy,
    progress: &ScanProgress,
) -> AppResult<Option<DriveScan>> {
    if !drive.is_ascii_uppercase() || !is_fixed(drive) {
        return Err("The selected local drive is unavailable.".into());
    }
    if !REVIEW_DAYS.contains(&older_days) {
        return Err("Choose a supported file age.".into());
    }
    let root = PathBuf::from(format!("{}:\\", char::from(drive)));
    scan_directory(&root, drive, older_days, mode, policy, progress)
}

pub(crate) fn scan_directory(
    root: &Path,
    drive: u8,
    older_days: u64,
    mode: ScanMode,
    policy: &DrivePolicy,
    progress: &ScanProgress,
) -> AppResult<Option<DriveScan>> {
    let entries =
        fs::read_dir(root).map_err(|e| format!("Could not scan {}: {e}", root.display()))?;
    let mut stack = vec![(entries, policy.in_temp(root))];
    let now = SystemTime::now();
    let mut temporary = BinaryHeap::<Reverse<DriveCandidate>>::new();
    let mut older = BinaryHeap::<Reverse<DriveCandidate>>::new();
    let mut report = DriveScan {
        drive,
        older_days,
        inspected: 0,
        protected: 0,
        unreadable: 0,
        omitted: 0,
        candidates: Vec::new(),
    };
    while let Some((entries, in_temp)) = stack.last_mut() {
        if progress.cancelled.load(Relaxed) {
            return Ok(None);
        }
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        report.inspected += 1;
        if report.inspected % 128 == 0 {
            progress.inspected.store(report.inspected, Relaxed);
        }
        let Ok((entry, metadata)) = entry.and_then(|entry| {
            let metadata = entry.metadata()?;
            Ok((entry, metadata))
        }) else {
            report.unreadable += 1;
            continue;
        };
        if !ordinary_name(&entry.file_name()) {
            report.protected += 1;
            continue;
        }
        let path = entry.path();
        if metadata.is_dir() {
            if !policy.directory_attributes_allowed(&path, metadata.file_attributes())
                || policy.protected_directory(&path)
            {
                report.protected += 1;
                continue;
            }
            match fs::read_dir(&path) {
                Ok(entries) => stack.push((entries, policy.in_temp(&path))),
                Err(_) => report.unreadable += 1,
            }
        } else if metadata.file_attributes() & PROTECTED_ATTRIBUTES != 0 {
            report.protected += 1;
        } else if eligible_file(&metadata)
            && let Some(kind) = policy.classify_in_directory(&path, *in_temp)
            && (mode == ScanMode::Review || kind == FileKind::Temporary)
            && (policy.ignored_files.is_empty() || !policy.ignored_files.contains(&path_key(&path)))
            && let Some(days) = age_days(&metadata, now)
            && days >= kind.minimum_age(older_days)
        {
            let candidates = match kind {
                FileKind::Temporary => &mut temporary,
                FileKind::Older => &mut older,
            };
            let candidate = DriveCandidate {
                bytes: metadata.len(),
                path,
                kind,
                days,
                created: metadata.creation_time(),
                modified: metadata.last_write_time(),
            };
            candidates.push(Reverse(candidate));
            if candidates.len() > RESULT_LIMIT {
                candidates.pop();
                report.omitted += 1;
            }
        }
    }
    if progress.cancelled.load(Relaxed) {
        return Ok(None);
    }
    report.candidates = temporary
        .into_sorted_vec()
        .into_iter()
        .chain(older.into_sorted_vec())
        .map(|Reverse(candidate)| candidate)
        .collect();
    Ok(Some(report))
}
