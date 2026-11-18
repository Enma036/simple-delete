use crate::complete_cleanup::{
    self, DriveCandidate, DrivePolicy, DriveScan, FileKind, ScanMode, ScanProgress,
};
use crate::model::{
    AppRecord, AppResult, Backup, Candidate, CleanupItem, ScanPlan, StartupEntry, StoreApp,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo,
    GetFileInformationByHandle, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Registry::{
    KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, KEY_SET_VALUE,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, GetExitCodeProcess, INFINITE, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_DWORD,
    REG_EXPAND_SZ, REG_SZ,
};
use winreg::{RegKey, RegValue};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
const REGISTRY_TREE_DELETE_ACCESS: u32 =
    DELETE | KEY_ENUMERATE_SUB_KEYS | KEY_QUERY_VALUE | KEY_SET_VALUE;
const PLAN_TTL: Duration = Duration::from_secs(1800);
const MAX_TRAVERSAL_ITEMS: usize = 20_000;
const MAX_SCAN_ENTRIES: usize = 5_000;
const SCAN_DURATION: Duration = Duration::from_secs(5);
const HKCU_RUN_KEY: &str = r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run";
const HKLM_RUN_KEY: &str = r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Run";
const HKCU_RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const HKLM_RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

const FORBIDDEN_NAMES: &[&str] = &[
    "microsoft",
    "windows",
    "programs",
    "commonfiles",
    "common",
    "software",
    "data",
    "apps",
    "application",
    "app",
    "temp",
    "packages",
    "cache",
    "system",
    "users",
    "programfiles",
    "programdata",
    "classes",
    "wow6432node",
    "policies",
    "registeredapplications",
];

const DISALLOWED_UNINSTALL_EXES: &[&str] = &[
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wscript.exe",
    "cscript.exe",
    "mshta.exe",
];

static ID_SEQ: AtomicU64 = AtomicU64::new(1);
static PLANS: LazyLock<Mutex<HashMap<String, StoredPlan>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Debug)]
struct StoredPlan {
    plan: ScanPlan,
    app_id: Option<String>,
    created_at: Instant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BackupRecord {
    id: String,
    name: String,
    kind: String,
    original: String,
    status: String,
    created_at_ms: u64,
    #[serde(default)]
    restored_at_ms: Option<u64>,
    #[serde(default)]
    backup_path: Option<String>,
    #[serde(default)]
    value_name: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    value_kind: Option<String>,
}

impl BackupRecord {
    fn to_model(&self) -> Backup {
        Backup {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind.clone(),
            original: self.original.clone(),
            status: self.status.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HiveKind {
    Hklm,
    Hkcu,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UninstallTarget {
    hive: HiveKind,
    parent_subkey: &'static str,
    subkey_name: String,
    native: String,
    wow64_64: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StartupValueInfo {
    name: String,
    command: String,
    kind_label: String,
}

trait SystemOps {
    fn app_data_root(&self) -> PathBuf;
    fn backups_dir(&self) -> PathBuf;
    fn approved_roots(&self) -> Vec<PathBuf>;
    fn list_installed(&self) -> AppResult<Vec<AppRecord>>;
    fn uninstall_entry_command(
        &self,
        target: &UninstallTarget,
        name: &str,
    ) -> AppResult<Option<String>>;
    fn list_hkcu_software_keys(&self, parent: &str, limit: usize) -> AppResult<Vec<String>>;
    fn export_registry_key(&self, native_key: &str, dest_file: &Path) -> AppResult<()>;
    fn import_registry_file(&self, src_file: &Path) -> AppResult<()>;
    fn registry_key_exists(&self, native_key: &str) -> AppResult<bool>;
    fn delete_hkcu_software_child(&self, child: &str) -> AppResult<()>;
    fn delete_uninstall_key(&self, target: &UninstallTarget) -> AppResult<()>;
    fn list_startup_entries(&self) -> AppResult<Vec<StartupEntry>>;
    fn read_hkcu_run_value(&self, value_name: &str) -> AppResult<Option<StartupValueInfo>>;
    fn write_hkcu_run_value(
        &self,
        value_name: &str,
        command: &str,
        kind_label: &str,
    ) -> AppResult<()>;
    fn delete_hkcu_run_value(&self, value_name: &str) -> AppResult<()>;
    fn launch_uninstaller(&self, executable: &str, args: &str) -> AppResult<()>;
    fn list_store_apps(&self) -> AppResult<Vec<StoreApp>>;
    fn remove_store_package(&self, package_full_name: &str) -> AppResult<()>;
    fn create_windows_restore_point(&self) -> AppResult<String>;
}

struct RealSystem;

impl SystemOps for RealSystem {
    fn app_data_root(&self) -> PathBuf {
        env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("SimpleDelete")
    }

    fn backups_dir(&self) -> PathBuf {
        self.app_data_root().join("native-backups")
    }

    fn approved_roots(&self) -> Vec<PathBuf> {
        let vars = [
            "LOCALAPPDATA",
            "APPDATA",
            "ProgramData",
            "ProgramFiles",
            "ProgramFiles(x86)",
        ];
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for var in vars {
            if let Some(val) = env::var_os(var)
                && !val.is_empty()
            {
                let norm = normalize_windows_path(Path::new(&val));
                let key = norm.to_string_lossy().to_ascii_lowercase();
                if seen.insert(key) {
                    out.push(norm);
                }
            }
        }
        out
    }

    fn list_installed(&self) -> AppResult<Vec<AppRecord>> {
        let sources = [
            (
                HKEY_LOCAL_MACHINE,
                "HKEY_LOCAL_MACHINE",
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
                KEY_READ | KEY_WOW64_64KEY,
            ),
            (
                HKEY_LOCAL_MACHINE,
                "HKEY_LOCAL_MACHINE",
                r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
                KEY_READ,
            ),
            (
                HKEY_CURRENT_USER,
                "HKEY_CURRENT_USER",
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
                KEY_READ,
            ),
            (
                HKEY_CURRENT_USER,
                "HKEY_CURRENT_USER",
                r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
                KEY_READ,
            ),
        ];

        let mut records = Vec::new();
        for (hive_handle, hive_name, base_path, flags) in sources {
            let root = RegKey::predef(hive_handle);
            let Ok(uninstall_key) = root.open_subkey_with_flags(base_path, flags) else {
                continue;
            };

            for child_res in uninstall_key.enum_keys() {
                let Ok(child_name) = child_res else {
                    continue;
                };
                let Ok(subkey) = uninstall_key.open_subkey_with_flags(&child_name, flags) else {
                    continue;
                };

                let display_name = read_reg_string(&subkey, "DisplayName", true);
                if display_name.trim().is_empty() {
                    continue;
                }
                if is_system_component(&subkey) {
                    continue;
                }

                let publisher = read_reg_string(&subkey, "Publisher", true);
                let version = read_reg_string(&subkey, "DisplayVersion", true);
                let installed = read_reg_string(&subkey, "InstallDate", true);
                let location = read_reg_string(&subkey, "InstallLocation", true);
                let command = read_reg_string(&subkey, "UninstallString", true);
                let size_mb = match subkey.get_value::<u32, _>("EstimatedSize") {
                    Ok(kb) => ((kb as f64 / 1024.0) * 10.0).round() / 10.0,
                    Err(_) => 0.0,
                };

                records.push(AppRecord {
                    id: format!(r"{}\{}\{}", hive_name, base_path, child_name),
                    name: display_name,
                    publisher,
                    version,
                    size_mb,
                    installed,
                    location,
                    command,
                });
            }
        }

        records.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(records)
    }

    fn uninstall_entry_command(
        &self,
        target: &UninstallTarget,
        name: &str,
    ) -> AppResult<Option<String>> {
        let hive = match target.hive {
            HiveKind::Hklm => RegKey::predef(HKEY_LOCAL_MACHINE),
            HiveKind::Hkcu => RegKey::predef(HKEY_CURRENT_USER),
        };
        let flags = if target.wow64_64 {
            KEY_READ | KEY_WOW64_64KEY
        } else {
            KEY_READ
        };
        let parent = match hive.open_subkey_with_flags(target.parent_subkey, flags) {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Unable to open uninstall registry branch: {e}")),
        };
        let subkey = match parent.open_subkey_with_flags(&target.subkey_name, flags) {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Unable to open application registration: {e}")),
        };
        let live_name = read_reg_string(&subkey, "DisplayName", true);
        if is_system_component(&subkey)
            || live_name.trim().is_empty()
            || live_name.trim() != name.trim()
        {
            return Err("The application registration changed. Refresh your library.".into());
        }
        Ok(Some(read_reg_string(&subkey, "UninstallString", true)))
    }

    fn list_hkcu_software_keys(&self, parent: &str, limit: usize) -> AppResult<Vec<String>> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if !parent.is_empty() && !vendor_name(parent) {
            return Err("Protected registry parent.".into());
        }
        let path = if parent.is_empty() {
            "Software".into()
        } else {
            format!(r"Software\{parent}")
        };
        let software = match hkcu.open_subkey_with_flags(&path, KEY_READ) {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("Unable to read HKCU\\{path}: {e}")),
        };
        let mut keys = Vec::new();
        for name in software.enum_keys().take(limit) {
            keys.push(name.map_err(|e| format!("Unable to finish reading HKCU\\Software: {e}"))?);
        }
        Ok(keys)
    }

    fn export_registry_key(&self, native_key: &str, dest_file: &Path) -> AppResult<()> {
        match fs::symlink_metadata(dest_file) {
            Ok(_) => return Err("Registry backup destination already exists.".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Unable to check registry backup destination: {error}"
                ));
            }
        }
        if let Some(parent) = dest_file.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create registry backup directory: {e}"))?;
        }
        let dest_str = dest_file.to_string_lossy().into_owned();
        let mut child = Command::new("reg.exe")
            .args(["export", native_key, &dest_str])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Failed to run reg.exe export: {e}"))?;
        // If a competing file appears, reg.exe may wait for overwrite confirmation
        // even with closed stdin. Abort the export instead of permitting overwrite.
        if unsafe { WaitForSingleObject(child.as_raw_handle(), 15_000) } != WAIT_OBJECT_0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Registry export did not complete within 15 seconds.".into());
        }
        let output = child
            .wait_with_output()
            .map_err(|e| format!("Unable to collect registry export result: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            return Err(if detail.is_empty() {
                "Registry backup failed.".to_string()
            } else {
                format!("Registry backup failed: {detail}")
            });
        }
        let meta = fs::metadata(dest_file)
            .map_err(|e| format!("Registry backup file missing after export: {e}"))?;
        if meta.len() == 0 {
            return Err("Registry backup produced an empty file.".to_string());
        }
        Ok(())
    }

    fn import_registry_file(&self, src_file: &Path) -> AppResult<()> {
        if !src_file.is_file() {
            return Err("Registry backup file is missing.".to_string());
        }
        let src_str = src_file.to_string_lossy().into_owned();
        let output = Command::new("reg.exe")
            .args(["import", &src_str])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("Failed to run reg.exe import: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            return Err(if detail.is_empty() {
                "Registry restore failed.".to_string()
            } else {
                format!("Registry restore failed: {detail}")
            });
        }
        Ok(())
    }

    fn registry_key_exists(&self, native_key: &str) -> AppResult<bool> {
        let (hive, subkey, flags) = split_native_registry_key(native_key)?;
        match hive.open_subkey_with_flags(subkey, flags) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("Unable to check registry key {native_key}: {e}")),
        }
    }

    fn delete_hkcu_software_child(&self, child: &str) -> AppResult<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let software = hkcu
            .open_subkey_with_flags("Software", REGISTRY_TREE_DELETE_ACCESS)
            .map_err(|e| format!("Unable to open HKCU\\Software for removal: {e}"))?;
        software
            .delete_subkey_all(child)
            .map_err(|e| format!("Unable to remove HKCU\\Software\\{child}: {e}"))
    }

    fn delete_uninstall_key(&self, target: &UninstallTarget) -> AppResult<()> {
        let hive = match target.hive {
            HiveKind::Hklm => RegKey::predef(HKEY_LOCAL_MACHINE),
            HiveKind::Hkcu => RegKey::predef(HKEY_CURRENT_USER),
        };
        let flags = if target.wow64_64 {
            REGISTRY_TREE_DELETE_ACCESS | KEY_WOW64_64KEY
        } else {
            REGISTRY_TREE_DELETE_ACCESS
        };
        let parent = hive
            .open_subkey_with_flags(target.parent_subkey, flags)
            .map_err(|e| format!("Unable to open uninstall key for removal: {e}"))?;
        parent
            .delete_subkey_all(&target.subkey_name)
            .map_err(|e| format!("Unable to remove application registration: {e}"))
    }

    fn list_startup_entries(&self) -> AppResult<Vec<StartupEntry>> {
        let mut entries = Vec::new();
        let sources = [
            (
                HKEY_CURRENT_USER,
                HKCU_RUN_KEY,
                HKCU_RUN_SUBKEY,
                KEY_READ,
                "Current user",
                true,
            ),
            (
                HKEY_LOCAL_MACHINE,
                HKLM_RUN_KEY,
                HKLM_RUN_SUBKEY,
                KEY_READ | KEY_WOW64_64KEY,
                "Machine",
                false,
            ),
        ];

        for (hive_handle, full_prefix, subkey_path, flags, scope, editable) in sources {
            let root = RegKey::predef(hive_handle);
            let Ok(run_key) = root.open_subkey_with_flags(subkey_path, flags) else {
                continue;
            };
            for item in run_key.enum_values() {
                let Ok((name, raw_val)) = item else {
                    continue;
                };
                if name.is_empty() {
                    continue;
                }
                let command = decode_reg_value(&raw_val, true);
                entries.push(StartupEntry {
                    id: format!("{full_prefix}|{name}"),
                    name,
                    command,
                    scope: scope.to_string(),
                    editable,
                });
            }
        }

        entries.sort_by(|a, b| {
            b.editable
                .cmp(&a.editable)
                .then_with(|| {
                    a.name
                        .to_ascii_lowercase()
                        .cmp(&b.name.to_ascii_lowercase())
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(entries)
    }

    fn read_hkcu_run_value(&self, value_name: &str) -> AppResult<Option<StartupValueInfo>> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = match hkcu.open_subkey_with_flags(HKCU_RUN_SUBKEY, KEY_READ) {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Unable to open startup key: {e}")),
        };
        let raw = match run_key.get_raw_value(value_name) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Unable to read startup value: {e}")),
        };
        let kind_label = match raw.vtype {
            REG_SZ => "String".to_string(),
            REG_EXPAND_SZ => "ExpandString".to_string(),
            _ => return Err("Unsupported startup value type.".to_string()),
        };
        let command = decode_reg_value(&raw, false);
        Ok(Some(StartupValueInfo {
            name: value_name.to_string(),
            command,
            kind_label,
        }))
    }

    fn write_hkcu_run_value(
        &self,
        value_name: &str,
        command: &str,
        kind_label: &str,
    ) -> AppResult<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = hkcu
            .create_subkey_with_flags(HKCU_RUN_SUBKEY, KEY_WRITE)
            .map_err(|e| format!("Unable to open startup key for restore: {e}"))?;
        let vtype = match kind_label {
            "String" => REG_SZ,
            "ExpandString" => REG_EXPAND_SZ,
            _ => return Err("Unsupported startup value type.".to_string()),
        };
        let raw = RegValue {
            bytes: encode_utf16_nul(command).into(),
            vtype,
        };
        run_key
            .set_raw_value(value_name, &raw)
            .map_err(|e| format!("Unable to restore startup entry: {e}"))
    }

    fn delete_hkcu_run_value(&self, value_name: &str) -> AppResult<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu
            .open_subkey_with_flags(HKCU_RUN_SUBKEY, KEY_WRITE)
            .map_err(|e| format!("Unable to open startup key: {e}"))?;
        run_key
            .delete_value(value_name)
            .map_err(|e| format!("Unable to disable startup entry: {e}"))
    }

    fn launch_uninstaller(&self, executable: &str, args: &str) -> AppResult<()> {
        if !executable.eq_ignore_ascii_case("msiexec.exe") && !Path::new(executable).is_file() {
            return Err(format!("Uninstaller executable not found: {executable}"));
        }
        let op_w = to_wide_nul("open");
        let file_w = to_wide_nul(executable);
        let params_w = (!args.is_empty()).then(|| to_wide_nul(args));
        let dir_w = Path::new(executable)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| to_wide_nul(&p.to_string_lossy()));

        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: op_w.as_ptr(),
            lpFile: file_w.as_ptr(),
            lpParameters: params_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            lpDirectory: dir_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            nShow: SW_SHOWNORMAL,
            ..Default::default()
        };
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            return Err(format!(
                "Failed to launch uninstaller: {}",
                std::io::Error::last_os_error()
            ));
        }
        if info.hProcess.is_null() {
            return Err("Windows did not provide an uninstaller process to track. Finish its prompts, then scan from the library.".into());
        }
        // This runs on a worker: keep the UI responsive while the vendor's wizard is open.
        let wait = unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
        let mut exit_code = 0;
        let got_exit = unsafe { GetExitCodeProcess(info.hProcess, &mut exit_code) };
        unsafe {
            CloseHandle(info.hProcess);
        }
        if wait != WAIT_OBJECT_0 || got_exit == 0 {
            return Err("Unable to verify the uninstaller finished. Scan from the library after completing its prompts.".into());
        }
        if !matches!(exit_code, 0 | 1641 | 3010) {
            return Err(format!(
                "Uninstaller cancelled or failed (exit code {exit_code}). No automatic cleanup was started."
            ));
        }
        Ok(())
    }

    fn list_store_apps(&self) -> AppResult<Vec<StoreApp>> {
        let script = r#"$friendly=@{};try{foreach($entry in @(Get-StartApps -ErrorAction Stop)){if($entry.AppID -like '*!*'){$family=([string]$entry.AppID).Split('!')[0];if(-not $friendly.ContainsKey($family)){$friendly[$family]=[string]$entry.Name}}}}catch{};$apps=@(Get-AppxPackage | Select-Object @{n='id';e={[string]$_.PackageFullName}},@{n='name';e={if($friendly.ContainsKey([string]$_.PackageFamilyName)){$friendly[[string]$_.PackageFamilyName]}else{[string]$_.Name}}},@{n='version';e={[string]$_.Version}},@{n='publisher';e={[string]$_.Publisher}},@{n='removable';e={[bool](-not $_.NonRemovable -and -not $_.IsFramework -and -not $_.IsResourcePackage)}});ConvertTo-Json -InputObject $apps -Compress"#;
        let out = run_powershell(script)?;
        let trimmed = out.trim();
        if trimmed.is_empty() || trimmed == "null" {
            return Ok(Vec::new());
        }
        if trimmed.starts_with('{') {
            let single: StoreApp = serde_json::from_str(trimmed)
                .map_err(|e| format!("Failed to parse Store apps output: {e}"))?;
            return Ok(vec![single]);
        }
        let mut apps: Vec<StoreApp> = serde_json::from_str(trimmed)
            .map_err(|e| format!("Failed to parse Store apps output: {e}"))?;
        apps.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(apps)
    }

    fn remove_store_package(&self, package_full_name: &str) -> AppResult<()> {
        let script = format!(
            "Remove-AppxPackage -Package {}",
            ps_literal(package_full_name)
        );
        let _ = run_powershell(&script)?;
        Ok(())
    }

    fn create_windows_restore_point(&self) -> AppResult<String> {
        let script = "$before=@(Get-ComputerRestorePoint | Select-Object -ExpandProperty SequenceNumber);Checkpoint-Computer -Description 'Simple Delete - before cleanup' -RestorePointType MODIFY_SETTINGS;$created=@(Get-ComputerRestorePoint | Where-Object {$before -notcontains $_.SequenceNumber});if($created.Count -eq 0){throw 'Windows did not create a new restore point. Check elevation, System Protection and restore-point frequency limits.'};[string]($created | Select-Object -Last 1 -ExpandProperty SequenceNumber)";
        let seq = run_powershell(script)?;
        let seq = seq.trim();
        if seq.is_empty() {
            return Err("Windows did not report a new restore point sequence number.".to_string());
        }
        Ok(seq.to_string())
    }
}

pub fn inventory() -> AppResult<Vec<AppRecord>> {
    RealSystem.list_installed()
}

pub fn uninstall(app: &AppRecord) -> AppResult<String> {
    uninstall_with(&RealSystem, app)
}

pub fn scan_leftovers(name: &str, app: Option<&AppRecord>) -> AppResult<ScanPlan> {
    scan_leftovers_with(&RealSystem, name, app)
}

pub fn cleanup(plan_id: &str, ids: &[String], force: bool) -> AppResult<Vec<CleanupItem>> {
    cleanup_with(&RealSystem, plan_id, ids, force)
}

pub(crate) fn scan_drive(
    drive: u8,
    older_days: u64,
    mode: ScanMode,
    progress: &ScanProgress,
) -> AppResult<Option<DriveScan>> {
    let mut policy = DrivePolicy::new(drive, &RealSystem.list_installed()?);
    if mode == ScanMode::Automatic {
        policy.ignore_files(restored_drive_paths(&RealSystem)?);
    }
    complete_cleanup::scan(drive, older_days, mode, &policy, progress)
}

pub(crate) fn clean_drive(plan: DriveScan, selected: Vec<bool>) -> AppResult<Vec<CleanupItem>> {
    clean_drive_with(&RealSystem, plan, selected)
}

pub(crate) fn auto_clean_drive(plan: DriveScan) -> AppResult<Vec<CleanupItem>> {
    auto_clean_drive_with(&RealSystem, plan)
}

fn auto_clean_drive_with(sys: &dyn SystemOps, plan: DriveScan) -> AppResult<Vec<CleanupItem>> {
    let restored = restored_drive_paths(sys)?;
    let selected: Vec<_> = plan
        .candidates
        .iter()
        .map(|file| {
            file.kind == FileKind::Temporary
                && !restored.contains(&complete_cleanup::path_key(&file.path))
        })
        .collect();
    if !selected.contains(&true) {
        return Ok(Vec::new());
    }
    clean_drive_with(sys, plan, selected)
}

fn restored_drive_paths(sys: &dyn SystemOps) -> AppResult<HashSet<String>> {
    Ok(load_all_backup_records(&sys.backups_dir())?
        .into_iter()
        .filter(|record| {
            record.kind == complete_cleanup::BACKUP_KIND && record.status == "restored"
        })
        .map(|record| complete_cleanup::path_key(Path::new(&record.original)))
        .collect())
}

fn clean_drive_with(
    sys: &dyn SystemOps,
    plan: DriveScan,
    selected: Vec<bool>,
) -> AppResult<Vec<CleanupItem>> {
    if !complete_cleanup::available_drives().contains(&plan.drive) {
        return Err("The scanned local drive is unavailable.".into());
    }
    if selected.len() != plan.candidates.len() || !selected.contains(&true) {
        return Err("Select files from a completed drive scan.".into());
    }
    let policy = DrivePolicy::new(plan.drive, &sys.list_installed()?);
    let backups_dir = sys.backups_dir();
    let _backup_directories = pin_directories(&backups_dir, true)?;
    Ok(plan
        .candidates
        .into_iter()
        .zip(selected)
        .filter(|(_, selected)| *selected)
        .map(|(candidate, _)| {
            let path = candidate.path.to_string_lossy().into_owned();
            let (backup_id, error) = match quarantine_drive_file(
                &candidate,
                plan.drive,
                plan.older_days,
                &policy,
                &backups_dir,
            ) {
                Ok((id, warning)) => (Some(id), warning),
                Err(error) => (None, Some(error)),
            };
            CleanupItem {
                path,
                backup_id,
                error,
            }
        })
        .collect())
}

fn quarantine_drive_file(
    candidate: &DriveCandidate,
    drive: u8,
    older_days: u64,
    policy: &DrivePolicy,
    backups_dir: &Path,
) -> AppResult<(String, Option<String>)> {
    let source = &candidate.path;
    if complete_cleanup::drive_letter(source) != Some(drive)
        || policy.classify(source) != Some(candidate.kind)
        || is_inside(&RealSystem.app_data_root(), source)
    {
        return Err("This location is now protected. Scan again.".into());
    }
    let parent = source.parent().ok_or("The file has no parent directory.")?;
    let parents = pin_directories(parent, false)?;
    for (handle, path) in parents.iter().rev().zip(parent.ancestors()) {
        let metadata = handle
            .metadata()
            .map_err(|e| format!("Unable to inspect parent directory: {e}"))?;
        if path.file_name().is_some()
            && !policy.directory_attributes_allowed(path, metadata.file_attributes())
        {
            return Err("This location is now protected. Scan again.".into());
        }
    }
    let file = open_drive_file_for_move(source)?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Unable to inspect the file: {e}"))?;
    if !candidate.still_matches(&metadata, older_days) {
        return Err("The file changed after the scan. Scan again before cleaning it.".into());
    }
    store_quarantined_path(
        source,
        source
            .file_name()
            .ok_or("Missing file name.")?
            .to_string_lossy()
            .as_ref(),
        complete_cleanup::BACKUP_KIND,
        backups_dir,
        &drive_quarantine_dir(parent, drive, backups_dir),
        |dest| rename_open_file(&file, dest),
    )
}

fn drive_quarantine_dir(parent: &Path, drive: u8, backups_dir: &Path) -> PathBuf {
    if complete_cleanup::drive_letter(backups_dir) == Some(drive) {
        // Prefer the app's backup directory over a path beside files in Temp.
        backups_dir.join("drive-files")
    } else {
        parent.join(".simple-delete-quarantine")
    }
}

pub fn list_backups() -> AppResult<Vec<Backup>> {
    list_backups_with(&RealSystem)
}

pub fn restore_backup(id: &str) -> AppResult<String> {
    restore_backup_with(&RealSystem, id)
}

pub fn startup_entries() -> AppResult<Vec<StartupEntry>> {
    RealSystem.list_startup_entries()
}

pub fn disable_startup(id: &str) -> AppResult<String> {
    disable_startup_with(&RealSystem, id)
}

pub fn store_apps() -> AppResult<Vec<StoreApp>> {
    RealSystem.list_store_apps()
}

pub fn remove_store(id: &str) -> AppResult<String> {
    remove_store_with(&RealSystem, id)
}

pub fn create_restore_point() -> AppResult<String> {
    create_restore_point_with(&RealSystem)
}

pub fn remove_registration(app: &AppRecord) -> AppResult<String> {
    remove_registration_with(&RealSystem, app)
}

fn uninstall_with(sys: &dyn SystemOps, app: &AppRecord) -> AppResult<String> {
    let target = parse_uninstall_target(&app.id)?;
    let live_command = sys
        .uninstall_entry_command(&target, &app.name)?
        .ok_or_else(|| "Registration no longer exists. Refresh your library.".to_string())?;
    if live_command.trim().is_empty() {
        return Err("No registered uninstaller.".to_string());
    }
    let (executable, args) = parse_uninstall_command(&live_command)?;
    sys.launch_uninstaller(&executable, &args)?;
    Ok("Uninstaller finished.".to_string())
}

fn scan_filesystem(
    roots: &[PathBuf],
    wanted: &str,
    data_root: &Path,
    max_entries: usize,
) -> (Vec<Candidate>, Vec<String>) {
    let deadline = Instant::now() + SCAN_DURATION;
    let mut queue: VecDeque<_> = roots.iter().map(|r| (r.clone(), 0usize)).collect();
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    let mut inspected = 0usize;
    let mut unreadable = 0usize;
    while let Some((directory, depth)) = queue.pop_front() {
        if Instant::now() >= deadline || inspected >= max_entries {
            warnings.push(
                "Filesystem scan reached its entry/time limit; coverage is incomplete.".into(),
            );
            break;
        }
        match fs::symlink_metadata(&directory) {
            Ok(m) if m.is_dir() && !is_reparse_or_symlink(&m) => {}
            _ => {
                unreadable += 1;
                continue;
            }
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        for entry in entries {
            if inspected >= max_entries || Instant::now() >= deadline {
                warnings.push(
                    "Filesystem scan reached its entry/time limit; coverage is incomplete.".into(),
                );
                if unreadable > 0 {
                    warnings.push(format!(
                        "{unreadable} filesystem entries or locations could not be inspected."
                    ));
                }
                return (candidates, warnings);
            }
            inspected += 1;
            let Ok(entry) = entry else {
                unreadable += 1;
                continue;
            };
            let path = normalize_windows_path(&entry.path());
            if same_path(&path, data_root)
                || is_inside(data_root, &path)
                || entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(".simple-delete-quarantine")
            {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                unreadable += 1;
                continue;
            };
            if is_reparse_or_symlink(&meta) {
                continue;
            }
            let directory_match =
                meta.is_dir() && normalize_name(&entry.file_name().to_string_lossy()) == wanted;
            let file_match = meta.is_file()
                && path
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .is_some_and(|s| normalize_name(s) == wanted)
                && path.extension().and_then(OsStr::to_str).is_some_and(|s| {
                    [
                        "log", "ini", "json", "db", "sqlite", "cfg", "dat", "cache", "xml",
                    ]
                    .contains(&s.to_ascii_lowercase().as_str())
                });
            if directory_match || file_match {
                if !candidates
                    .iter()
                    .any(|c: &Candidate| same_path(Path::new(&c.path), &path))
                {
                    candidates.push(Candidate {
                        id: next_id(), path: path.to_string_lossy().into_owned(),
                        kind: if directory_match { "directory" } else { "file" }.into(),
                        reason: format!("Exact application-name {}match at depth {}; ownership is unproven. May include personal settings.", if file_match { "file-stem " } else { "" }, depth + 1),
                        blocked: false,
                    });
                }
            } else if meta.is_dir()
                && depth == 0
                && vendor_name(&entry.file_name().to_string_lossy())
            {
                // Only one vendor/product level; never recurse into matched apps or quarantine.
                queue.push_back((path, depth + 1));
            }
        }
    }
    if unreadable > 0 {
        warnings.push(format!("{unreadable} filesystem entries or locations could not be inspected; coverage is incomplete."));
    }
    (candidates, warnings)
}

fn scan_registry(
    sys: &dyn SystemOps,
    wanted: &str,
    max_entries: usize,
) -> (Vec<Candidate>, Vec<String>) {
    let deadline = Instant::now() + SCAN_DURATION;
    let mut queue = VecDeque::from([String::new()]);
    let mut inspected = 0usize;
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    while let Some(parent) = queue.pop_front() {
        if inspected >= max_entries || Instant::now() >= deadline {
            warnings
                .push("Registry scan reached its entry/time limit; coverage is incomplete.".into());
            break;
        }
        let keys = match sys.list_hkcu_software_keys(&parent, max_entries - inspected) {
            Ok(keys) => keys,
            Err(e) => {
                if warnings.len() < 16 {
                    warnings.push(e);
                }
                continue;
            }
        };
        for child in keys {
            inspected += 1;
            if !vendor_name(&child) {
                continue;
            }
            let relative = if parent.is_empty() {
                child.clone()
            } else {
                format!(r"{parent}\{child}")
            };
            if specific_name(&child) && normalize_name(&child) == wanted {
                candidates.push(Candidate {
                    id: next_id(), path: format!(r"HKEY_CURRENT_USER\Software\{relative}"), kind: "registry".into(),
                    reason: "Exact current-user product-key match; only this key is selected, not its vendor parent. May include preferences or licenses.".into(), blocked: false,
                });
            } else if parent.is_empty() {
                queue.push_back(relative);
            }
        }
        if inspected >= max_entries {
            warnings
                .push("Registry scan reached its entry limit; coverage may be incomplete.".into());
            break;
        }
    }
    (candidates, warnings)
}

fn scan_leftovers_with(
    sys: &dyn SystemOps,
    name: &str,
    app: Option<&AppRecord>,
) -> AppResult<ScanPlan> {
    let effective_name = if name.trim().is_empty() {
        app.map(|a| a.name.as_str()).unwrap_or("")
    } else {
        name.trim()
    };
    if !specific_name(effective_name) {
        return Err("Enter a specific app name of at least four letters or digits.".to_string());
    }

    let wanted = normalize_name(effective_name);
    let roots = sys.approved_roots();
    let installed = sys.list_installed()?;
    let data_root = normalize_windows_path(&sys.app_data_root());
    let backups_root = normalize_windows_path(&sys.backups_dir());

    let (mut candidates, mut warnings) =
        scan_filesystem(&roots, &wanted, &data_root, MAX_SCAN_ENTRIES);

    if let Some(app_rec) = app
        && !app_rec.location.trim().is_empty()
        && normalize_name(&app_rec.name) == wanted
    {
        let raw = Path::new(app_rec.location.trim());
        let target = normalize_windows_path(raw);
        if let Some(base_name) = target.file_name().and_then(|s| s.to_str())
            && raw.is_absolute()
            && roots.iter().any(|r| is_inside(r, &target))
            && specific_name(base_name)
            && !candidates
                .iter()
                .any(|c| c.kind == "directory" && same_path(Path::new(&c.path), &target))
        {
            match fs::symlink_metadata(&target) {
                Ok(meta) if meta.is_dir() && !is_reparse_or_symlink(&meta) => {
                    candidates.push(Candidate {
                        id: next_id(),
                        path: target.to_string_lossy().into_owned(),
                        kind: "directory".to_string(),
                        reason: "Recorded install location; may contain preferences or user-created files. Review ownership before cleanup.".to_string(),
                        blocked: false,
                    });
                }
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    warnings.push(format!(
                        "{}: {e}; install location could not be inspected.",
                        target.display()
                    ));
                }
                _ => {}
            }
        }
    }

    let (registry_candidates, registry_warnings) = scan_registry(sys, &wanted, MAX_SCAN_ENTRIES);
    candidates.extend(registry_candidates);
    warnings.extend(registry_warnings);

    let app_id = app.map(|a| a.id.clone()).filter(|s| !s.is_empty());
    let active = installed.iter().any(|a| {
        normalize_name(&a.name) == wanted
            || app_id
                .as_deref()
                .is_some_and(|pid| a.id.eq_ignore_ascii_case(pid))
    });

    for c in &mut candidates {
        if c.kind == "directory" || c.kind == "file" {
            let c_path = Path::new(&c.path);
            let touches_data = same_path(c_path, &data_root)
                || same_path(c_path, &backups_root)
                || is_inside(c_path, &backups_root)
                || is_inside(&data_root, c_path);
            let used_by_other = installed.iter().any(|a| {
                let same_app = app_id
                    .as_deref()
                    .is_some_and(|pid| a.id.eq_ignore_ascii_case(pid));
                if same_app || a.location.trim().is_empty() {
                    return false;
                }
                let loc = normalize_windows_path(Path::new(a.location.trim()));
                same_path(&loc, c_path) || is_inside(c_path, &loc) || is_inside(&loc, c_path)
            });
            c.blocked = touches_data || used_by_other;
        }
    }

    let plan = ScanPlan {
        id: next_id(),
        name: effective_name.to_string(),
        candidates,
        warnings,
        active,
        coverage: "Files and folders: depth 2, up to 5,000 entries / 5 seconds. Registry: HKCU Software product keys at depth 1–2, up to 5,000 keys / 5 seconds. Recorded install location checked separately. No exhaustive ownership proof.".into(),
    };

    let mut guard = PLANS
        .lock()
        .map_err(|_| "Scan plan lock poisoned.".to_string())?;
    guard.retain(|_, p| p.created_at.elapsed() <= PLAN_TTL);
    guard.insert(
        plan.id.clone(),
        StoredPlan {
            plan: plan.clone(),
            app_id,
            created_at: Instant::now(),
        },
    );

    Ok(plan)
}

fn cleanup_with(
    sys: &dyn SystemOps,
    plan_id: &str,
    ids: &[String],
    force: bool,
) -> AppResult<Vec<CleanupItem>> {
    if plan_id.trim().is_empty() {
        return Err("Scan expired. Scan again before cleaning.".to_string());
    }
    if ids.is_empty() {
        return Err("Select valid scan candidates.".to_string());
    }

    let stored = {
        let mut guard = PLANS
            .lock()
            .map_err(|_| "Scan plan lock poisoned.".to_string())?;
        guard.retain(|_, p| p.created_at.elapsed() <= PLAN_TTL);
        let Some(entry) = guard.get(plan_id).cloned() else {
            return Err("Scan expired. Scan again before cleaning.".to_string());
        };
        entry
    };

    let mut selected_indices = Vec::new();
    let mut seen_idx = HashSet::new();
    for sel in ids {
        let found_idx = stored
            .plan
            .candidates
            .iter()
            .position(|c| &c.id == sel || c.path.eq_ignore_ascii_case(sel));
        let Some(idx) = found_idx else {
            return Err("Select valid scan candidates.".to_string());
        };
        if seen_idx.insert(idx) {
            selected_indices.push(idx);
        }
    }

    let installed = sys.list_installed()?;
    let active = installed.iter().any(|a| {
        stored
            .app_id
            .as_deref()
            .is_some_and(|pid| a.id.eq_ignore_ascii_case(pid))
            || normalize_name(&a.name) == normalize_name(&stored.plan.name)
    });
    if active && !force {
        return Err(
            "Application is still installed. Complete uninstall and refresh, or explicitly choose forced cleanup."
                .to_string(),
        );
    }

    {
        let mut guard = PLANS
            .lock()
            .map_err(|_| "Scan plan lock poisoned.".to_string())?;
        if guard.remove(plan_id).is_none() {
            return Err("Scan expired. Scan again before cleaning.".to_string());
        }
    }

    let roots = sys.approved_roots();
    let backups_dir = sys.backups_dir();
    let data_root = normalize_windows_path(&sys.app_data_root());
    let mut results = Vec::new();

    for idx in selected_indices {
        let c = &stored.plan.candidates[idx];
        let outcome = quarantine_candidate(
            sys,
            c,
            &stored.plan.name,
            stored.app_id.as_deref(),
            &installed,
            &roots,
            &backups_dir,
            &data_root,
        );
        match outcome {
            Ok(backup_id) => results.push(CleanupItem {
                path: c.path.clone(),
                backup_id: Some(backup_id),
                error: None,
            }),
            Err(err) => results.push(CleanupItem {
                path: c.path.clone(),
                backup_id: None,
                error: Some(err),
            }),
        }
    }

    Ok(results)
}

#[allow(clippy::too_many_arguments)]
fn quarantine_candidate(
    sys: &dyn SystemOps,
    c: &Candidate,
    app_name: &str,
    plan_app_id: Option<&str>,
    installed: &[AppRecord],
    roots: &[PathBuf],
    backups_dir: &Path,
    data_root: &Path,
) -> AppResult<String> {
    if c.kind == "directory" || c.kind == "file" {
        let target = normalize_windows_path(Path::new(&c.path));
        let touches_data = same_path(&target, data_root)
            || same_path(&target, backups_dir)
            || is_inside(&target, backups_dir)
            || is_inside(data_root, &target);
        let used_by_other = installed.iter().any(|a| {
            let same_app = plan_app_id.is_some_and(|pid| a.id.eq_ignore_ascii_case(pid));
            if same_app || a.location.trim().is_empty() {
                return false;
            }
            let loc = normalize_windows_path(Path::new(a.location.trim()));
            same_path(&loc, &target) || is_inside(&target, &loc) || is_inside(&loc, &target)
        });
        if c.blocked || touches_data || used_by_other {
            return Err("Another registered app uses this location.".to_string());
        }

        let metadata = fs::symlink_metadata(&target)
            .map_err(|e| format!("Unable to inspect candidate: {e}"))?;
        if (c.kind == "directory" && !metadata.is_dir())
            || (c.kind == "file" && !metadata.is_file())
        {
            return Err("Candidate type changed; scan again before cleanup.".into());
        }
        let parent = target
            .parent()
            .ok_or_else(|| "Target has no parent directory.".to_string())?;
        let _parents = pin_directories(parent, false)?;
        validate_tree(&target, roots, MAX_TRAVERSAL_ITEMS)?;
        let (backup_id, warning) = store_quarantined_path(
            &target, app_name, &c.kind, backups_dir,
            &parent.join(".simple-delete-quarantine"),
            |dest| move_without_overwrite(&target, dest),
        )?;
        warning.map_or(Ok(backup_id), Err)
    } else if c.kind == "registry" {
        let backup_id = next_id();
        let created_at_ms = now_epoch_ms();
        let child = parse_hkcu_software_child(&c.path)?;
        if !specific_name(&child) {
            return Err("Protected registry key.".to_string());
        }
        let native_key = format!(r"HKEY_CURRENT_USER\Software\{child}");
        let reg_file = backups_dir
            .join("registry")
            .join(format!("{backup_id}.reg"));
        let mut record = BackupRecord {
            id: backup_id.clone(),
            name: app_name.to_string(),
            kind: "registry".to_string(),
            original: native_key.clone(),
            status: "prepared".to_string(),
            created_at_ms,
            restored_at_ms: None,
            backup_path: Some(reg_file.to_string_lossy().into_owned()),
            value_name: None,
            command: None,
            value_kind: None,
        };
        // Export must succeed before advertising a recoverable registry backup.
        // Persist the prepared record before deleting the original key.
        let (directories, backup_file) =
            export_validated_registry_key(sys, &native_key, &reg_file)?;
        if let Err(error) = save_backup_record(backups_dir, &record) {
            drop(backup_file);
            let _ = fs::remove_file(&reg_file);
            return Err(error);
        }
        if let Err(error) = verify_registry_export_unchanged(sys, &native_key, &reg_file) {
            drop(backup_file);
            return Err(discard_unused_registry_backup(
                backups_dir,
                &backup_id,
                &reg_file,
                error,
            ));
        }
        sys.delete_hkcu_software_child(&child)
            .map_err(|error| registry_removal_error(error, &backup_id, &reg_file))?;
        drop((backup_file, directories));

        record.status = "quarantined".to_string();
        save_backup_record(backups_dir, &record)?;
        Ok(backup_id)
    } else {
        Err("Unsupported candidate type.".to_string())
    }
}

fn store_quarantined_path(
    source: &Path,
    name: &str,
    kind: &str,
    backups_dir: &Path,
    quarantine_dir: &Path,
    move_item: impl FnOnce(&Path) -> std::io::Result<()>,
) -> AppResult<(String, Option<String>)> {
    fs::create_dir_all(quarantine_dir)
        .map_err(|e| format!("Failed to create quarantine directory: {e}"))?;
    let _quarantine = pin_directories(quarantine_dir, false)?;
    let id = next_id();
    let destination = quarantine_dir.join(&id);
    let mut record = BackupRecord {
        id: id.clone(),
        name: name.to_string(),
        kind: kind.to_string(),
        original: source.to_string_lossy().into_owned(),
        status: "prepared".to_string(),
        created_at_ms: now_epoch_ms(),
        restored_at_ms: None,
        backup_path: Some(destination.to_string_lossy().into_owned()),
        value_name: None,
        command: None,
        value_kind: None,
    };
    save_backup_record(backups_dir, &record)?;
    if let Err(error) = move_item(&destination) {
        return Err(discard_prepared_record(
            backups_dir,
            &id,
            format!("Failed to move item to quarantine: {error}"),
        ));
    }
    record.status = "quarantined".to_string();
    // The durable prepared record already contains everything Recovery needs.
    let warning = save_backup_record(backups_dir, &record).err();
    Ok((id, warning))
}

fn open_drive_file_for_move(path: &Path) -> AppResult<fs::File> {
    let file = fs::OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| format!("Could not open the file for a move: {e}"))?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(format!(
            "Could not inspect the file: {}",
            std::io::Error::last_os_error()
        ));
    }
    if info.nNumberOfLinks != 1 {
        return Err("The file must have exactly one filesystem link.".into());
    }
    Ok(file)
}

fn rename_open_file(file: &fs::File, destination: &Path) -> std::io::Result<()> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;

    let parent = destination
        .parent()
        .ok_or(std::io::ErrorKind::InvalidInput)?;
    let name = destination
        .file_name()
        .ok_or(std::io::ErrorKind::InvalidInput)?;
    let destination = fs::canonicalize(parent)?.join(name);
    let wide_name: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let name_bytes = (wide_name.len() - 1) * size_of::<u16>();
    let name_offset = offset_of!(FILE_RENAME_INFO, FileName);
    let buffer_bytes = name_offset + name_bytes + size_of::<u16>();
    // Keep the variable-length Win32 structure aligned and leave ReplaceIfExists false.
    let mut buffer = vec![0usize; buffer_bytes.div_ceil(size_of::<usize>())];
    let raw = buffer.as_mut_ptr().cast::<u8>();
    unsafe {
        let info = raw.cast::<FILE_RENAME_INFO>();
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            wide_name.as_ptr(),
            raw.add(name_offset).cast::<u16>(),
            wide_name.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            raw.cast(),
            buffer_bytes as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn restore_drive_file(record: &BackupRecord, backups_dir: &Path) -> AppResult<()> {
    let original = Path::new(&record.original);
    let drive = complete_cleanup::local_file_drive(original)
        .ok_or("The original local drive or path is unavailable.")?;
    let parent = original
        .parent()
        .ok_or("Missing original parent directory.")?;
    let quarantine = drive_quarantine_dir(parent, drive, backups_dir);
    let expected = quarantine.join(&record.id);
    if record.backup_path.as_deref().map(Path::new) != Some(expected.as_path()) {
        return Err("Backup path does not match its quarantine record.".into());
    }
    let _directories = pin_directories(&quarantine, false)?;
    let file = open_drive_file_for_move(&expected)?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Unable to inspect the backup: {e}"))?;
    if !complete_cleanup::eligible_file(&metadata) {
        return Err("The quarantined item no longer matches its recorded type.".into());
    }
    let _original_directories = pin_directories(parent, true)?;
    match fs::symlink_metadata(original) {
        Ok(_) => return Err("Original location exists. Restore will not overwrite it.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Unable to check original location: {e}")),
    }
    match rename_open_file(&file, original) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("Original location exists. Restore will not overwrite it.".into())
        }
        Err(e) => Err(format!("Failed to restore quarantined file: {e}")),
    }
}

fn list_backups_with(sys: &dyn SystemOps) -> AppResult<Vec<Backup>> {
    let dir = sys.backups_dir();
    let records = load_all_backup_records(&dir)?;
    Ok(records.into_iter().map(|r| r.to_model()).collect())
}

fn restore_backup_with(sys: &dyn SystemOps, id: &str) -> AppResult<String> {
    if !is_valid_id(id) {
        return Err("Invalid backup identifier.".to_string());
    }
    let dir = sys.backups_dir();
    let mut record =
        load_backup_record(&dir, id)?.ok_or_else(|| "Backup is unavailable.".to_string())?;
    if record.status != "quarantined" && record.status != "prepared" {
        return Err("Backup is unavailable.".to_string());
    }

    match record.kind.as_str() {
        complete_cleanup::BACKUP_KIND => restore_drive_file(&record, &dir)?,
        "directory" | "file" => {
            let original = normalize_windows_path(Path::new(&record.original));
            let backup_path_str = record
                .backup_path
                .as_deref()
                .ok_or_else(|| "Backup path is missing.".to_string())?;
            if !Path::new(&record.original).is_absolute()
                || !Path::new(backup_path_str).is_absolute()
            {
                return Err("Backup paths must be absolute.".into());
            }

            let orig_parent = original
                .parent()
                .ok_or_else(|| "Original location has no parent directory.".to_string())?;
            let expected = orig_parent.join(".simple-delete-quarantine").join(id);
            if expected != normalize_windows_path(Path::new(backup_path_str)) {
                return Err("Backup path does not match its quarantine record.".to_string());
            }

            let roots = sys.approved_roots();
            if !roots.iter().any(|root| is_inside(root, &original)) {
                return Err("Original location is outside the approved roots.".to_string());
            }
            let _directories = pin_directories(
                expected.parent().ok_or("Missing quarantine directory.")?,
                false,
            )?;
            match fs::symlink_metadata(&original) {
                Ok(_) => {
                    return Err("Original location exists. Restore will not overwrite it.".into());
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Unable to check original location: {e}")),
            }
            let metadata = fs::symlink_metadata(&expected)
                .map_err(|e| format!("Unable to read quarantined item: {e}"))?;
            if (record.kind == "directory" && !metadata.is_dir())
                || (record.kind == "file" && !metadata.is_file())
            {
                return Err("Quarantined item no longer matches its recorded type.".to_string());
            }
            validate_tree(&expected, &roots, MAX_TRAVERSAL_ITEMS)?;
            move_without_overwrite(&expected, &original)
                .map_err(|e| format!("Failed to restore quarantined item: {e}"))?;
        }
        "startup" => {
            if record.original != HKCU_RUN_KEY || record.backup_path.is_some() {
                return Err("Invalid startup backup target.".into());
            }
            let value_name = record
                .value_name
                .as_deref()
                .filter(|s| !s.is_empty() && !s.contains('\0'))
                .ok_or_else(|| "Invalid startup backup metadata.".to_string())?;
            let command = record
                .command
                .as_deref()
                .filter(|s| !s.contains('\0'))
                .ok_or_else(|| "Invalid startup backup command.".to_string())?;
            let value_kind = record.value_kind.as_deref().unwrap_or("String");
            if !matches!(value_kind, "String" | "ExpandString") {
                return Err("Unsupported startup value type.".into());
            }
            if sys.read_hkcu_run_value(value_name)?.is_some() {
                return Err("Startup value already exists; restore refused.".to_string());
            }
            sys.write_hkcu_run_value(value_name, command, value_kind)?;
        }
        "registration" => {
            let target = parse_uninstall_target(&record.original)?;
            check_registration_backup_scope(&target)?;
            let backup_path_str = record
                .backup_path
                .as_deref()
                .ok_or_else(|| "Registration backup file is missing.".to_string())?;
            restore_registry_payload(sys, &dir, id, backup_path_str, &target.native)?;
        }
        "registry" => {
            let child = parse_hkcu_software_child(&record.original)?;
            if !specific_name(&child) {
                return Err("Invalid registry backup.".to_string());
            }
            let native = format!(r"HKEY_CURRENT_USER\Software\{child}");
            let backup_path_str = record
                .backup_path
                .as_deref()
                .ok_or_else(|| "Registry backup file is missing.".to_string())?;
            restore_registry_payload(sys, &dir, id, backup_path_str, &native)?;
        }
        _ => return Err("Unsupported backup type.".to_string()),
    }

    record.status = "restored".to_string();
    record.restored_at_ms = Some(now_epoch_ms());
    save_backup_record(&dir, &record)?;
    Ok(format!("Restored {} ({}).", record.name, record.kind))
}

fn restore_registry_payload(
    sys: &dyn SystemOps,
    dir: &Path,
    id: &str,
    recorded_path: &str,
    original_key: &str,
) -> AppResult<()> {
    let registry_dir = dir.join("registry");
    let expected = registry_dir.join(format!("{id}.reg"));
    let recorded_path = Path::new(recorded_path);
    if !recorded_path.is_absolute()
        || normalize_windows_path(recorded_path) != normalize_windows_path(&expected)
    {
        return Err("Registry backup path does not match its record.".into());
    }
    let expected = normalize_windows_path(&expected);
    let parent = expected
        .parent()
        .ok_or("Registry backup has no parent directory.")?;
    let _directories = pin_directories(parent, false)?;
    let _file = open_validated_registry_payload(&expected, original_key, false)?;
    if sys.registry_key_exists(original_key)? {
        return Err(format!(
            "Original registry key exists. Restore would overwrite it. Backup file: {}",
            expected.display()
        ));
    }
    sys.import_registry_file(&expected)
}

fn pin_directories(parent: &Path, create: bool) -> AppResult<Vec<fs::File>> {
    let ancestors: Vec<_> = parent.ancestors().collect();
    let mut directory_handles = Vec::with_capacity(ancestors.len());
    for path in ancestors.into_iter().rev() {
        if create && path.parent().is_some() {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!(
                        "Unable to create directory {}: {error}",
                        path.display()
                    ));
                }
            }
        }
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|e| format!("Unable to open directory {}: {e}", path.display()))?;
        let metadata = handle
            .metadata()
            .map_err(|e| format!("Unable to inspect registry backup: {e}"))?;
        if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
            return Err(format!(
                "Directory cannot contain junctions or links: {}",
                path.display()
            ));
        }
        directory_handles.push(handle);
    }
    Ok(directory_handles)
}

fn open_validated_registry_payload(
    path: &Path,
    original_key: &str,
    flush: bool,
) -> AppResult<fs::File> {
    // Keep the validated file open with read sharing only while reg.exe reads it.
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(flush)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| format!("Unable to open registry backup: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Unable to inspect registry backup: {e}"))?;
    if !metadata.is_file() || is_reparse_or_symlink(&metadata) {
        return Err("Registry backup must be a regular file.".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("Unable to read registry backup: {e}"))?;
    validate_registry_export(&bytes, original_key)?;
    if flush {
        file.sync_all()
            .map_err(|e| format!("Unable to flush registry backup: {e}"))?;
    }
    Ok(file)
}

fn export_validated_registry_key(
    sys: &dyn SystemOps,
    key: &str,
    path: &Path,
) -> AppResult<(Vec<fs::File>, fs::File)> {
    let absolute = normalize_windows_path(path);
    let parent = absolute
        .parent()
        .ok_or("Registry backup has no parent directory.")?;
    let directories = pin_directories(parent, true)?;
    match fs::symlink_metadata(&absolute) {
        Ok(_) => return Err("Registry backup destination already exists.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Unable to check registry backup destination: {error}"
            ));
        }
    }
    // A failed export does not establish ownership of the destination.
    sys.export_registry_key(key, &absolute)?;
    match open_validated_registry_payload(&absolute, key, true) {
        Ok(file) => Ok((directories, file)),
        Err(error) => {
            let _ = fs::remove_file(&absolute);
            Err(error)
        }
    }
}

fn verify_registry_export_unchanged(
    sys: &dyn SystemOps,
    key: &str,
    backup: &Path,
) -> AppResult<()> {
    let current = backup.with_file_name(format!("{}.reg", next_id()));
    let (directories, file) = export_validated_registry_key(sys, key, &current)?;
    let comparison =
        fs::read(backup).and_then(|original| fs::read(&current).map(|fresh| fresh == original));
    drop(file);
    let removal = fs::remove_file(&current);
    drop(directories);
    removal.map_err(|e| format!("Unable to remove registry verification export: {e}"))?;
    if !comparison.map_err(|e| format!("Unable to compare registry exports: {e}"))? {
        return Err(
            "The registry key changed while it was being backed up. Refresh and try again.".into(),
        );
    }
    Ok(())
}

fn registry_removal_error(error: String, id: &str, backup: &Path) -> String {
    format!(
        "{error}. Registry removal may be incomplete. Backup {id} was kept at {}.",
        backup.display()
    )
}

fn discard_prepared_record(dir: &Path, id: &str, error: String) -> String {
    match fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => error,
        Err(cleanup) => format!("{error}. Unable to remove the unused backup record: {cleanup}"),
    }
}

fn discard_unused_registry_backup(dir: &Path, id: &str, payload: &Path, error: String) -> String {
    match fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => match fs::remove_file(payload) {
            Ok(()) => error,
            Err(cleanup) => {
                format!("{error}. Unable to remove the unused registry export: {cleanup}")
            }
        },
        Err(cleanup) => format!("{error}. Unable to remove the unused backup record: {cleanup}"),
    }
}

fn validate_registry_export(bytes: &[u8], original_key: &str) -> AppResult<()> {
    let invalid = || {
        "Registry backup does not contain only the recorded key and its descendants.".to_string()
    };
    let content = if let Some(utf16) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let units = utf16.chunks_exact(2);
        if !units.remainder().is_empty() {
            return Err(invalid());
        }
        String::from_utf16(
            &units
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| invalid())?
    } else {
        std::str::from_utf8(bytes)
            .map_err(|_| invalid())?
            .to_owned()
    };
    let mut lines = content.lines();
    if content.contains(['\0', '\r']) && content.replace("\r\n", "").contains(['\0', '\r']) {
        return Err(invalid());
    }
    if lines.next() != Some("Windows Registry Editor Version 5.00") {
        return Err(invalid());
    }
    let mut saw_root = false;
    for line in lines.map(str::trim) {
        if !line.starts_with('[') {
            continue;
        }
        let key = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
            .ok_or_else(invalid)?;
        if key.eq_ignore_ascii_case(original_key) {
            saw_root = true;
        } else if !saw_root
            || !strip_prefix_ci(key, original_key)
                .is_some_and(|tail| tail.starts_with('\\') && tail.len() > 1)
        {
            return Err(invalid());
        }
    }
    if !saw_root {
        return Err(invalid());
    }
    Ok(())
}

fn disable_startup_with(sys: &dyn SystemOps, id: &str) -> AppResult<String> {
    let entries = sys.list_startup_entries()?;
    let Some(entry) = entries.into_iter().find(|e| e.id == id && e.editable) else {
        return Err("Only current-user Run entries can be disabled here.".to_string());
    };
    let value_name = parse_hkcu_run_entry_id(id)?;
    if value_name != entry.name {
        return Err("Invalid startup entry identifier.".to_string());
    }
    let val_info = sys
        .read_hkcu_run_value(&value_name)?
        .ok_or_else(|| "Startup entry no longer exists.".to_string())?;
    if val_info.command.contains('\0') {
        return Err(
            "Startup command contains an embedded NUL and cannot be backed up safely.".into(),
        );
    }

    let backups_dir = sys.backups_dir();
    let backup_id = next_id();
    let mut record = BackupRecord {
        id: backup_id.clone(),
        name: value_name.clone(),
        kind: "startup".to_string(),
        original: HKCU_RUN_KEY.to_string(),
        status: "prepared".to_string(),
        created_at_ms: now_epoch_ms(),
        restored_at_ms: None,
        backup_path: None,
        value_name: Some(value_name.clone()),
        command: Some(val_info.command),
        value_kind: Some(val_info.kind_label),
    };
    save_backup_record(&backups_dir, &record)?;

    if let Err(error) = sys.delete_hkcu_run_value(&value_name) {
        return Err(discard_prepared_record(&backups_dir, &backup_id, error));
    }

    record.status = "quarantined".to_string();
    save_backup_record(&backups_dir, &record)?;
    Ok(format!(
        "Startup entry '{value_name}' disabled (backup {backup_id})."
    ))
}

fn remove_store_with(sys: &dyn SystemOps, id: &str) -> AppResult<String> {
    let trimmed = id.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
        || trimmed.contains(['\'', '"', ';', '`', '$'])
    {
        return Err("Invalid Store package identifier.".to_string());
    }
    let apps = sys.list_store_apps()?;
    if !apps.iter().any(|a| a.id == trimmed && a.removable) {
        return Err("Package cannot be removed for this user.".to_string());
    }
    sys.remove_store_package(trimmed)?;
    Ok("Package removed for the current user.".to_string())
}

fn create_restore_point_with(sys: &dyn SystemOps) -> AppResult<String> {
    let seq = sys.create_windows_restore_point()?;
    Ok(format!(
        "Restore point created (sequence {seq}). Check Windows System Protection for availability."
    ))
}

fn remove_registration_with(sys: &dyn SystemOps, app: &AppRecord) -> AppResult<String> {
    let target = parse_uninstall_target(&app.id)?;
    check_registration_backup_scope(&target)?;
    let check_live = || {
        sys.uninstall_entry_command(&target, &app.name)?
            .ok_or_else(|| "Registration no longer exists. Refresh your library.".to_string())
    };
    let original_command = check_live()?;

    let backups_dir = sys.backups_dir();
    let backup_id = next_id();
    let reg_file = backups_dir
        .join("registry")
        .join(format!("{backup_id}.reg"));
    let mut record = BackupRecord {
        id: backup_id.clone(),
        name: app.name.clone(),
        kind: "registration".to_string(),
        original: target.native.clone(),
        status: "prepared".to_string(),
        created_at_ms: now_epoch_ms(),
        restored_at_ms: None,
        backup_path: Some(reg_file.to_string_lossy().into_owned()),
        value_name: None,
        command: None,
        value_kind: None,
    };
    let (directories, backup_file) = export_validated_registry_key(sys, &target.native, &reg_file)?;
    if let Err(error) = save_backup_record(&backups_dir, &record) {
        drop(backup_file);
        let _ = fs::remove_file(&reg_file);
        return Err(error);
    }
    let unchanged = verify_registry_export_unchanged(sys, &target.native, &reg_file)
        .and_then(|()| check_live())
        .and_then(|command| {
            if command == original_command {
                Ok(())
            } else {
                Err("The application registration changed. Refresh your library.".into())
            }
        });
    if let Err(error) = unchanged {
        drop(backup_file);
        return Err(discard_unused_registry_backup(
            &backups_dir,
            &backup_id,
            &reg_file,
            error,
        ));
    }
    sys.delete_uninstall_key(&target)
        .map_err(|error| registry_removal_error(error, &backup_id, &reg_file))?;
    drop((backup_file, directories));

    record.status = "quarantined".to_string();
    save_backup_record(&backups_dir, &record)?;
    Ok(format!(
        "Registration backed up and removed (backup {backup_id}). Refresh your library."
    ))
}

fn check_registration_backup_scope(target: &UninstallTarget) -> AppResult<()> {
    if matches!(target.hive, HiveKind::Hklm) {
        return Err("Machine-wide registrations require administrator-managed backups. Use the app's registered uninstaller.".into());
    }
    Ok(())
}

fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn specific_name(s: &str) -> bool {
    let n = normalize_name(s);
    n.len() >= 4 && !FORBIDDEN_NAMES.contains(&n.as_str())
}

fn vendor_name(s: &str) -> bool {
    let n = normalize_name(s);
    n.len() >= 2 && !FORBIDDEN_NAMES.contains(&n.as_str()) && !s.contains(['\\', '/', '\0'])
}

pub(crate) fn normalize_windows_path(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy();
    let stripped = strip_verbatim_prefix(&s);
    let mut out = PathBuf::new();
    for comp in Path::new(&stripped).components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = out
                    .components()
                    .next_back()
                    .is_some_and(|c| matches!(c, Component::Normal(_)));
                if can_pop {
                    out.pop();
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn strip_verbatim_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

fn norm_components(p: &Path) -> Vec<String> {
    normalize_windows_path(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect()
}

fn same_path(a: &Path, b: &Path) -> bool {
    norm_components(a) == norm_components(b)
}

fn is_inside(root: &Path, target: &Path) -> bool {
    let rc = norm_components(root);
    let tc = norm_components(target);
    !rc.is_empty() && tc.len() > rc.len() && tc[..rc.len()] == rc[..]
}

fn is_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink() || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

fn validate_tree(target: &Path, allowed_roots: &[PathBuf], limit: usize) -> AppResult<usize> {
    let norm_target = normalize_windows_path(target);
    let norm_roots: Vec<PathBuf> = allowed_roots
        .iter()
        .map(|r| normalize_windows_path(r))
        .collect();
    let matched_root = norm_roots
        .iter()
        .find(|r| is_inside(r, &norm_target))
        .ok_or_else(|| "Target is outside an application-data root.".to_string())?;

    let mut cursor = norm_target.clone();
    loop {
        let meta = fs::symlink_metadata(&cursor)
            .map_err(|e| format!("Unable to inspect path {}: {e}", cursor.display()))?;
        if is_reparse_or_symlink(&meta) {
            return Err("Junctions and symbolic links are excluded.".to_string());
        }
        if same_path(&cursor, matched_root) {
            break;
        }
        let Some(parent) = cursor.parent() else {
            return Err("Invalid path boundary.".to_string());
        };
        if same_path(parent, &cursor) {
            return Err("Invalid path boundary.".to_string());
        }
        cursor = parent.to_path_buf();
    }

    let real_root = fs::canonicalize(matched_root)
        .map_err(|e| format!("Unable to resolve root {}: {e}", matched_root.display()))?;
    let real_target = fs::canonicalize(&norm_target)
        .map_err(|e| format!("Unable to resolve target {}: {e}", norm_target.display()))?;
    if !real_target
        .strip_prefix(&real_root)
        .is_ok_and(|relative| !relative.as_os_str().is_empty())
    {
        return Err("Resolved path escaped its root.".to_string());
    }

    let mut count = 0usize;
    let mut queue = VecDeque::new();
    queue.push_back(norm_target);

    while let Some(path) = queue.pop_front() {
        count += 1;
        if count > limit {
            return Err("Directory exceeds the review limit; manage it manually.".to_string());
        }
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("Unable to inspect {}: {e}", path.display()))?;
        if is_reparse_or_symlink(&meta) {
            return Err("Directory contains a junction or link.".to_string());
        }
        if meta.is_dir() {
            let entries = fs::read_dir(&path)
                .map_err(|e| format!("Unable to read directory {}: {e}", path.display()))?;
            for entry_res in entries {
                let entry = entry_res
                    .map_err(|e| format!("Unable to iterate directory {}: {e}", path.display()))?;
                queue.push_back(entry.path());
            }
        }
    }

    Ok(count)
}

fn parse_uninstall_target(raw_id: &str) -> AppResult<UninstallTarget> {
    let mut key = raw_id.trim();
    for prefix in ["Microsoft.PowerShell.Core\\Registry::", "Registry::"] {
        if key.len() >= prefix.len() && key[..prefix.len()].eq_ignore_ascii_case(prefix) {
            key = &key[prefix.len()..];
            break;
        }
    }

    let (hive, hive_name, rest) = if let Some(r) = strip_prefix_ci(key, r"HKEY_LOCAL_MACHINE\") {
        (HiveKind::Hklm, "HKEY_LOCAL_MACHINE", r)
    } else if let Some(r) = strip_prefix_ci(key, r"HKLM:\") {
        (HiveKind::Hklm, "HKEY_LOCAL_MACHINE", r)
    } else if let Some(r) = strip_prefix_ci(key, r"HKEY_CURRENT_USER\") {
        (HiveKind::Hkcu, "HKEY_CURRENT_USER", r)
    } else if let Some(r) = strip_prefix_ci(key, r"HKCU:\") {
        (HiveKind::Hkcu, "HKEY_CURRENT_USER", r)
    } else {
        return Err("Not a recognized application registration.".to_string());
    };

    let (parent_subkey, wow64_64, child) = if let Some(c) = strip_prefix_ci(
        rest,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\",
    ) {
        (
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
            hive == HiveKind::Hklm,
            c,
        )
    } else if let Some(c) = strip_prefix_ci(
        rest,
        r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\",
    ) {
        (
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            false,
            c,
        )
    } else {
        return Err("Not a recognized application registration.".to_string());
    };

    if child.is_empty()
        || child.contains('\\')
        || child.contains('/')
        || child.contains('\0')
        || child == "."
        || child == ".."
    {
        return Err("Not a recognized application registration.".to_string());
    }

    Ok(UninstallTarget {
        hive,
        parent_subkey,
        subkey_name: child.to_string(),
        native: format!(r"{hive_name}\{parent_subkey}\{child}"),
        wow64_64,
    })
}

fn parse_hkcu_software_child(raw_path: &str) -> AppResult<String> {
    let key = raw_path.trim();
    let child = strip_prefix_ci(key, r"HKEY_CURRENT_USER\Software\")
        .or_else(|| strip_prefix_ci(key, r"HKCU:\Software\"))
        .or_else(|| strip_prefix_ci(key, r"HKCU\Software\"))
        .ok_or_else(|| "Protected registry key.".to_string())?;
    let parts: Vec<_> = child.split('\\').collect();
    if parts.is_empty()
        || parts.len() > 2
        || !specific_name(parts.last().copied().unwrap_or_default())
        || (parts.len() == 2 && !vendor_name(parts[0]))
        || child.contains('/')
        || child.contains('\0')
        || child == "."
        || child == ".."
    {
        return Err("Protected registry key.".to_string());
    }
    Ok(child.to_string())
}

fn parse_hkcu_run_entry_id(id: &str) -> AppResult<String> {
    let rest = strip_prefix_ci(
        id,
        r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run|",
    )
    .or_else(|| strip_prefix_ci(id, r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Run|"))
    .ok_or_else(|| "Only current-user Run entries can be disabled here.".to_string())?;
    if rest.is_empty() || rest.contains('\0') {
        return Err("Invalid startup entry identifier.".to_string());
    }
    Ok(rest.to_string())
}

fn split_native_registry_key(native_key: &str) -> AppResult<(RegKey, &str, u32)> {
    if let Some(sub) = strip_prefix_ci(native_key, r"HKEY_LOCAL_MACHINE\") {
        let flags = if strip_prefix_ci(sub, r"Software\WOW6432Node\").is_some() {
            KEY_READ
        } else {
            KEY_READ | KEY_WOW64_64KEY
        };
        Ok((RegKey::predef(HKEY_LOCAL_MACHINE), sub, flags))
    } else if let Some(sub) = strip_prefix_ci(native_key, r"HKEY_CURRENT_USER\") {
        Ok((RegKey::predef(HKEY_CURRENT_USER), sub, KEY_READ))
    } else {
        Err("Unsupported registry hive.".to_string())
    }
}

fn parse_uninstall_command(command: &str) -> AppResult<(String, String)> {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(
            "Unsupported uninstall command. Use Windows Settings for this app.".to_string(),
        );
    }

    let (raw_exe, raw_args) = if let Some(after_open_quote) = trimmed.strip_prefix('"') {
        let Some(end_quote) = after_open_quote.find('"') else {
            return Err(
                "Unsupported uninstall command. Use Windows Settings for this app.".to_string(),
            );
        };
        let exe = after_open_quote[..end_quote].trim();
        let rest = after_open_quote[end_quote + 1..].trim();
        if !ends_with_exe(exe) {
            return Err(
                "Unsupported uninstall command. Use Windows Settings for this app.".to_string(),
            );
        }
        (exe, rest)
    } else {
        find_unquoted_exe_split(trimmed).ok_or_else(|| {
            "Unsupported uninstall command. Use Windows Settings for this app.".to_string()
        })?
    };

    let exe_file_name = Path::new(raw_exe)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if DISALLOWED_UNINSTALL_EXES.contains(&exe_file_name.as_str()) {
        return Err(
            "Unsupported uninstall command. Use Windows Settings for this app.".to_string(),
        );
    }

    if exe_file_name == "msiexec.exe" || exe_file_name == "msiexec" {
        if !raw_exe.eq_ignore_ascii_case("msiexec.exe")
            && !raw_exe.eq_ignore_ascii_case("msiexec")
            && !is_absolute_drive_exe(raw_exe)
        {
            return Err("Uninstaller must have an absolute executable path.".to_string());
        }
        return Ok((
            "msiexec.exe".to_string(),
            replace_msi_install_flag(raw_args),
        ));
    }

    if !is_absolute_drive_exe(raw_exe) {
        return Err("Uninstaller must have an absolute executable path.".to_string());
    }

    Ok((raw_exe.to_string(), raw_args.to_string()))
}

fn find_unquoted_exe_split(s: &str) -> Option<(&str, &str)> {
    let token_end = s.find(char::is_whitespace).unwrap_or(s.len());
    if s[..token_end].eq_ignore_ascii_case("msiexec") {
        return Some((&s[..token_end], s[token_end..].trim()));
    }
    let bytes = s.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    for i in 0..=bytes.len() - 4 {
        if bytes[i..i + 4].eq_ignore_ascii_case(b".exe") {
            let end = i + 4;
            let at_boundary =
                end == bytes.len() || s[end..].chars().next().is_some_and(char::is_whitespace);
            if at_boundary {
                let exe = s[..end].trim();
                let args = s[end..].trim();
                if !exe.is_empty() {
                    return Some((exe, args));
                }
            }
        }
    }
    None
}

fn ends_with_exe(s: &str) -> bool {
    s.len() > 4 && s.as_bytes()[s.len() - 4..].eq_ignore_ascii_case(b".exe")
}

fn is_absolute_drive_exe(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() > 7
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
        && ends_with_exe(s)
}

fn replace_msi_install_flag(args: &str) -> String {
    let bytes = args.as_bytes();
    if bytes.len() < 3 {
        return args.to_string();
    }
    for i in 0..=bytes.len() - 2 {
        if bytes[i] == b'/' && (bytes[i + 1] == b'I' || bytes[i + 1] == b'i') {
            let prev_ok = i == 0
                || args[..i]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);
            let next_ok = args[i + 2..]
                .chars()
                .next()
                .is_some_and(|c| c == '{' || c.is_whitespace());
            if prev_ok && next_ok {
                let mut out = String::with_capacity(args.len());
                out.push_str(&args[..i]);
                out.push_str("/X");
                out.push_str(&args[i + 2..]);
                return out;
            }
        }
    }
    args.to_string()
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
    {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn is_system_component(subkey: &RegKey) -> bool {
    if let Ok(v) = subkey.get_value::<u32, _>("SystemComponent")
        && v == 1
    {
        return true;
    }
    if let Ok(v) = subkey.get_value::<String, _>("SystemComponent")
        && v.trim() == "1"
    {
        return true;
    }
    false
}

fn read_reg_string(subkey: &RegKey, name: &str, expand: bool) -> String {
    let Ok(raw) = subkey.get_raw_value(name) else {
        return String::new();
    };
    decode_reg_value(&raw, expand)
}

fn decode_reg_value(raw: &RegValue<'_>, expand: bool) -> String {
    match raw.vtype {
        REG_SZ | REG_EXPAND_SZ => {
            let mut units = Vec::with_capacity(raw.bytes.len() / 2);
            for chunk in raw.bytes.chunks_exact(2) {
                units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
            }
            while units.last() == Some(&0) {
                units.pop();
            }
            let s = String::from_utf16_lossy(&units);
            if expand && raw.vtype == REG_EXPAND_SZ {
                expand_env_vars(&s)
            } else {
                s
            }
        }
        REG_DWORD if raw.bytes.len() >= 4 => {
            let v = u32::from_le_bytes([raw.bytes[0], raw.bytes[1], raw.bytes[2], raw.bytes[3]]);
            v.to_string()
        }
        _ => String::new(),
    }
}

fn expand_env_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        if let Some(end) = after_start.find('%') {
            let var_name = &after_start[..end];
            if !var_name.is_empty()
                && let Ok(val) = env::var(var_name)
            {
                out.push_str(&val);
                rest = &after_start[end + 1..];
                continue;
            }
            out.push('%');
            rest = after_start;
        } else {
            out.push('%');
            rest = after_start;
            break;
        }
    }
    out.push_str(rest);
    out
}

fn encode_utf16_nul(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0u16))
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

fn to_wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0u16)).collect()
}

fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub(crate) fn run_powershell(script: &str) -> AppResult<String> {
    let full = format!(
        "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);{script}"
    );
    let utf16_bytes: Vec<u8> = full.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let encoded = base64_encode(&utf16_bytes);
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to invoke powershell.exe: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if detail.is_empty() {
            "PowerShell command failed.".to_string()
        } else {
            detail
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_start_matches('\u{feff}')
        .trim()
        .to_string();
    Ok(stdout)
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

fn next_id() -> String {
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let a = splitmix64(nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let b = splitmix64(pid ^ (seq << 17) ^ a);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        ((a >> 16) & 0xFFFF) as u16,
        (a & 0x0FFF) as u16,
        (((b >> 48) as u16) & 0x3FFF) | 0x8000,
        b & 0xFFFF_FFFF_FFFF
    )
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn move_without_overwrite(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // No REPLACE_EXISTING: refuse an existing destination atomically, including files.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn save_backup_record(backups_dir: &Path, record: &BackupRecord) -> AppResult<()> {
    fs::create_dir_all(backups_dir)
        .map_err(|e| format!("Failed to create backups directory: {e}"))?;
    let json_path = backups_dir.join(format!("{}.json", record.id));
    let tmp_path = backups_dir.join(format!("{}.json.tmp", record.id));
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| format!("Failed to serialize backup metadata: {e}"))?;
    use std::io::Write;
    let mut file =
        fs::File::create(&tmp_path).map_err(|e| format!("Failed to write backup metadata: {e}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Failed to flush backup metadata: {e}"))?;
    drop(file);
    // Windows rename cannot replace an existing file. Replace atomically rather than
    // truncating the previous recovery record if an update is interrupted.
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = tmp_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = json_path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("Failed to persist backup metadata: {error}"));
    }
    Ok(())
}

fn load_backup_record(backups_dir: &Path, id: &str) -> AppResult<Option<BackupRecord>> {
    if !is_valid_id(id) {
        return Ok(None);
    }
    let path = backups_dir.join(format!("{id}.json"));
    match fs::read(&path) {
        Ok(bytes) => {
            let rec: BackupRecord = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid backup metadata for {id}: {e}"))?;
            if rec.id != id {
                return Err(format!("Backup metadata does not match identifier {id}."));
            }
            Ok(Some(rec))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Unable to read backup {id}: {e}")),
    }
}

fn load_all_backup_records(backups_dir: &Path) -> AppResult<Vec<BackupRecord>> {
    fs::create_dir_all(backups_dir)
        .map_err(|e| format!("Failed to initialize backups directory: {e}"))?;
    let mut records = Vec::new();
    let entries =
        fs::read_dir(backups_dir).map_err(|e| format!("Unable to read backups directory: {e}"))?;
    for entry_res in entries {
        let Ok(entry) = entry_res else {
            continue;
        };
        let path = entry.path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_id(stem) {
            continue;
        }
        if let Ok(bytes) = fs::read(&path)
            && let Ok(rec) = serde_json::from_slice::<BackupRecord>(&bytes)
            && rec.id == stem
        {
            records.push(rec);
        }
    }
    records.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::os::windows::fs::symlink_dir;

    struct FixtureEnv {
        root: PathBuf,
        installed: RefCell<Vec<AppRecord>>,
        hkcu_software: RefCell<HashSet<String>>,
        reg_keys: RefCell<HashSet<String>>,
        startup_run: RefCell<HashMap<String, StartupValueInfo>>,
        startup_readonly: RefCell<Vec<StartupEntry>>,
        store: RefCell<Vec<StoreApp>>,
        launched_uninstalls: RefCell<Vec<(String, String)>>,
        removed_store_ids: RefCell<Vec<String>>,
        restore_point_seq: RefCell<Option<String>>,
        fail_export: Cell<bool>,
        wrong_export_key: Cell<bool>,
        change_during_export: Cell<bool>,
        export_count: Cell<u32>,
    }

    impl FixtureEnv {
        fn new(root: PathBuf) -> Self {
            Self {
                root,
                installed: RefCell::new(Vec::new()),
                hkcu_software: RefCell::new(HashSet::new()),
                reg_keys: RefCell::new(HashSet::new()),
                startup_run: RefCell::new(HashMap::new()),
                startup_readonly: RefCell::new(Vec::new()),
                store: RefCell::new(Vec::new()),
                launched_uninstalls: RefCell::new(Vec::new()),
                removed_store_ids: RefCell::new(Vec::new()),
                restore_point_seq: RefCell::new(Some("42".to_string())),
                fail_export: Cell::new(false),
                wrong_export_key: Cell::new(false),
                change_during_export: Cell::new(false),
                export_count: Cell::new(0),
            }
        }
    }

    impl SystemOps for FixtureEnv {
        fn app_data_root(&self) -> PathBuf {
            self.root.join("SimpleDelete")
        }

        fn backups_dir(&self) -> PathBuf {
            self.app_data_root().join("native-backups")
        }

        fn approved_roots(&self) -> Vec<PathBuf> {
            vec![normalize_windows_path(&self.root.join("ApprovedRoot"))]
        }

        fn list_installed(&self) -> AppResult<Vec<AppRecord>> {
            Ok(self.installed.borrow().clone())
        }

        fn uninstall_entry_command(
            &self,
            target: &UninstallTarget,
            name: &str,
        ) -> AppResult<Option<String>> {
            self.installed
                .borrow()
                .iter()
                .find(|a| a.id.eq_ignore_ascii_case(&target.native))
                .map(|a| {
                    if a.name.trim() != name.trim() {
                        return Err(
                            "The application registration changed. Refresh your library.".into(),
                        );
                    }
                    Ok(a.command.clone())
                })
                .transpose()
        }

        fn list_hkcu_software_keys(&self, parent: &str, limit: usize) -> AppResult<Vec<String>> {
            let prefix = format!(r"{parent}\");
            let mut keys = HashSet::new();
            for key in self.hkcu_software.borrow().iter() {
                let relative = if parent.is_empty() {
                    Some(key.as_str())
                } else {
                    strip_prefix_ci(key, &prefix)
                };
                if let Some(first) = relative.and_then(|r| r.split('\\').next()) {
                    keys.insert(first.to_string());
                }
            }
            let mut keys: Vec<_> = keys.into_iter().collect();
            keys.sort();
            keys.truncate(limit);
            Ok(keys)
        }

        fn export_registry_key(&self, native_key: &str, dest_file: &Path) -> AppResult<()> {
            if self.fail_export.get() {
                return Err("Injected registry export failure".into());
            }
            let native_key = if self.wrong_export_key.get() {
                r"HKEY_CURRENT_USER\Software\WrongProduct"
            } else {
                native_key
            };
            if let Some(parent) = dest_file.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let count = self.export_count.get() + 1;
            self.export_count.set(count);
            let change = if self.change_during_export.get() && count == 2 {
                "\"UpdatedSetting\"=dword:00000001\r\n"
            } else {
                ""
            };
            fs::write(
                dest_file,
                format!("Windows Registry Editor Version 5.00\r\n[{native_key}]\r\n{change}"),
            )
            .map_err(|e| e.to_string())
        }

        fn import_registry_file(&self, src_file: &Path) -> AppResult<()> {
            let content = fs::read_to_string(src_file).map_err(|e| e.to_string())?;
            for line in content.lines() {
                if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    self.reg_keys
                        .borrow_mut()
                        .insert(inner.to_ascii_lowercase());
                    if let Some(child) = strip_prefix_ci(inner, r"HKEY_CURRENT_USER\Software\") {
                        self.hkcu_software.borrow_mut().insert(child.to_string());
                    }
                }
            }
            Ok(())
        }

        fn registry_key_exists(&self, native_key: &str) -> AppResult<bool> {
            Ok(self
                .reg_keys
                .borrow()
                .contains(&native_key.to_ascii_lowercase()))
        }

        fn delete_hkcu_software_child(&self, child: &str) -> AppResult<()> {
            let key = format!(r"hkey_current_user\software\{}", child.to_ascii_lowercase());
            self.reg_keys.borrow_mut().remove(&key);
            self.hkcu_software.borrow_mut().retain(|k| {
                !k.eq_ignore_ascii_case(child)
                    && strip_prefix_ci(k, &format!(r"{child}\")).is_none()
            });
            Ok(())
        }

        fn delete_uninstall_key(&self, target: &UninstallTarget) -> AppResult<()> {
            self.reg_keys
                .borrow_mut()
                .remove(&target.native.to_ascii_lowercase());
            self.installed
                .borrow_mut()
                .retain(|a| !a.id.eq_ignore_ascii_case(&target.native));
            Ok(())
        }

        fn list_startup_entries(&self) -> AppResult<Vec<StartupEntry>> {
            let mut out = Vec::new();
            for (name, info) in self.startup_run.borrow().iter() {
                out.push(StartupEntry {
                    id: format!("{HKCU_RUN_KEY}|{name}"),
                    name: name.clone(),
                    command: info.command.clone(),
                    scope: "Current user".to_string(),
                    editable: true,
                });
            }
            out.extend(self.startup_readonly.borrow().iter().cloned());
            Ok(out)
        }

        fn read_hkcu_run_value(&self, value_name: &str) -> AppResult<Option<StartupValueInfo>> {
            Ok(self.startup_run.borrow().get(value_name).cloned())
        }

        fn write_hkcu_run_value(
            &self,
            value_name: &str,
            command: &str,
            kind_label: &str,
        ) -> AppResult<()> {
            self.startup_run.borrow_mut().insert(
                value_name.to_string(),
                StartupValueInfo {
                    name: value_name.to_string(),
                    command: command.to_string(),
                    kind_label: kind_label.to_string(),
                },
            );
            Ok(())
        }

        fn delete_hkcu_run_value(&self, value_name: &str) -> AppResult<()> {
            self.startup_run
                .borrow_mut()
                .remove(value_name)
                .ok_or_else(|| "Missing startup value.".to_string())?;
            Ok(())
        }

        fn launch_uninstaller(&self, executable: &str, args: &str) -> AppResult<()> {
            self.launched_uninstalls
                .borrow_mut()
                .push((executable.to_string(), args.to_string()));
            Ok(())
        }

        fn list_store_apps(&self) -> AppResult<Vec<StoreApp>> {
            Ok(self.store.borrow().clone())
        }

        fn remove_store_package(&self, package_full_name: &str) -> AppResult<()> {
            self.removed_store_ids
                .borrow_mut()
                .push(package_full_name.to_string());
            Ok(())
        }

        fn create_windows_restore_point(&self) -> AppResult<String> {
            self.restore_point_seq
                .borrow()
                .clone()
                .ok_or_else(|| "Windows did not create a new restore point.".to_string())
        }
    }

    struct TempDirGuard(PathBuf);
    impl TempDirGuard {
        fn new(label: &str) -> Self {
            let dir = env::temp_dir().join(format!("sd-backend-test-{label}-{}", next_id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn complete_cleanup_scans_expected_files_and_restores_without_overwriting() {
        use crate::complete_cleanup::FileKind;
        use std::os::windows::fs::FileTimesExt;

        let profile = PathBuf::from(env::var_os("USERPROFILE").unwrap());
        let fixture_name = format!("sd-drive-cleanup-test-{}", next_id());
        let temp = TempDirGuard(profile.join(r"AppData\Local\Temp").join(&fixture_name));
        let user_files = TempDirGuard(profile.join(&fixture_name));
        let windows = user_files.path().join("Windows");
        let install = user_files.path().join("RegisteredProduct");
        let documents = user_files.path().join("Documents");
        for dir in [temp.path(), &windows, &install, &documents] {
            fs::create_dir_all(dir).unwrap();
        }
        let old_file = |path: &Path| {
            fs::write(path, b"fixture data").unwrap();
            let old = SystemTime::now() - Duration::from_hours(24 * 400);
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_created(old)
                        .set_modified(old)
                        .set_accessed(old),
                )
                .unwrap();
        };
        let cleanable = temp.path().join("removable.tmp");
        let changed = temp.path().join("changed.tmp");
        let linked = temp.path().join("linked.tmp");
        let personal = documents.join("archived report.pdf");
        for path in [
            cleanable.clone(),
            changed.clone(),
            linked.clone(),
            personal.clone(),
            windows.join("system.zip"),
            install.join("assets.zip"),
            temp.path().join("keep.pdf"),
        ] {
            old_file(&path);
        }
        fs::hard_link(&linked, temp.path().join("keep.link")).unwrap();
        fs::write(temp.path().join("recent.tmp"), b"recent").unwrap();
        let drive = complete_cleanup::drive_letter(&profile).unwrap();
        let policy = DrivePolicy::new(
            drive,
            &[AppRecord {
                location: install.display().to_string(),
                ..AppRecord::default()
            }],
        );
        assert!(complete_cleanup::available_drives().contains(&drive));
        let progress = ScanProgress::default();
        let older = complete_cleanup::scan_directory(
            user_files.path(),
            drive,
            180,
            ScanMode::Review,
            &policy,
            &progress,
        )
        .unwrap()
        .unwrap();
        assert_eq!(older.candidates.len(), 1);
        assert_eq!(older.candidates[0].kind, FileKind::Older);
        assert_eq!(older.candidates[0].path, personal);
        let scan = complete_cleanup::scan_directory(
            temp.path(),
            drive,
            180,
            ScanMode::Review,
            &policy,
            &progress,
        )
        .unwrap()
        .unwrap();
        assert_eq!(scan.candidates.len(), 3);
        assert!(
            scan.candidates
                .iter()
                .any(|c| c.kind == FileKind::Temporary && c.path == cleanable)
        );

        let sys = FixtureEnv::new(temp.path().to_path_buf());
        let backups = sys.backups_dir();
        let move_candidate = |path: &Path| {
            let candidate = scan.candidates.iter().find(|c| c.path == path).unwrap();
            quarantine_drive_file(candidate, drive, 180, &policy, &backups)
        };
        fs::write(&changed, b"replacement").unwrap();
        let error = move_candidate(&changed).unwrap_err();
        assert!(error.contains("changed"), "{error}");
        let error = move_candidate(&linked).unwrap_err();
        assert!(error.contains("filesystem link"), "{error}");
        assert!(changed.exists() && linked.exists());
        let (backup, warning) = move_candidate(&cleanable).unwrap();
        assert!(warning.is_none());
        assert!(!cleanable.exists());
        let records = list_backups_with(&sys).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, backup);
        assert!(backups.join("drive-files").join(&backup).is_file());
        let mut prepared = load_backup_record(&backups, &backup).unwrap().unwrap();
        prepared.status = "prepared".into();
        save_backup_record(&backups, &prepared).unwrap();

        fs::write(&cleanable, b"new file").unwrap();
        assert!(
            restore_backup_with(&sys, &backup)
                .unwrap_err()
                .contains("overwrite")
        );
        assert_eq!(fs::read(&cleanable).unwrap(), b"new file");
        fs::remove_file(&cleanable).unwrap();
        restore_backup_with(&sys, &backup).unwrap();
        assert_eq!(fs::read(&cleanable).unwrap(), b"fixture data");
        assert_eq!(list_backups_with(&sys).unwrap()[0].status, "restored");
        assert!(personal.exists());
        let (backup, warning) =
            quarantine_drive_file(&older.candidates[0], drive, 180, &policy, &backups).unwrap();
        assert!(warning.is_none());
        assert!(!personal.exists());
        fs::remove_dir(&documents).unwrap();
        restore_backup_with(&sys, &backup).unwrap();
        assert_eq!(fs::read(&personal).unwrap(), b"fixture data");

        old_file(&personal);
        old_file(&cleanable);
        let automatic_file = temp.path().join("automatic.temp");
        old_file(&automatic_file);
        let mut automatic_policy = DrivePolicy::new(drive, &[]);
        automatic_policy.ignore_files(restored_drive_paths(&sys).unwrap());
        let mut automatic_scan = complete_cleanup::scan_directory(
            temp.path(),
            drive,
            180,
            ScanMode::Automatic,
            &automatic_policy,
            &progress,
        )
        .unwrap()
        .unwrap();
        assert!(
            automatic_scan
                .candidates
                .iter()
                .all(|candidate| candidate.path != cleanable)
        );
        assert!(
            complete_cleanup::scan_directory(
                user_files.path(),
                drive,
                180,
                ScanMode::Automatic,
                &automatic_policy,
                &progress,
            )
            .unwrap()
            .unwrap()
            .candidates
            .is_empty()
        );

        // The cleanup stage also checks these rules if given candidates from a review scan.
        automatic_scan.candidates.extend(older.candidates);
        automatic_scan.candidates.extend(
            scan.candidates
                .into_iter()
                .filter(|candidate| candidate.path == cleanable),
        );
        let moved = auto_clean_drive_with(&sys, automatic_scan).unwrap();
        assert_eq!(
            moved.iter().filter(|item| item.backup_id.is_some()).count(),
            1
        );
        assert!(!automatic_file.exists());
        assert!(cleanable.exists() && personal.exists() && linked.exists());

        progress.cancelled.store(true, Ordering::Relaxed);
        assert!(
            complete_cleanup::scan_directory(
                temp.path(),
                drive,
                180,
                ScanMode::Automatic,
                &policy,
                &progress
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn generic_names_and_containment_are_rejected() {
        for bad in [
            "Microsoft",
            "Windows",
            "Program Files",
            "Common Files",
            "Packages",
            "Apps",
            "Temp",
            "a",
            "abc",
        ] {
            assert!(!specific_name(bad), "expected {bad} to be rejected");
        }
        assert!(specific_name("Discord"));

        let root = Path::new(r"C:\cleanup-test");
        assert!(!is_inside(root, root));
        assert!(!is_inside(root, Path::new(r"C:\cleanup-test-sibling")));
        assert!(!is_inside(root, Path::new(r"C:\")));
        assert!(is_inside(root, Path::new(r"C:\cleanup-test\app")));
    }

    #[test]
    fn validate_tree_enforces_root_limits_and_symlinks() {
        let tmp = TempDirGuard::new("tree");
        let root = tmp.path().join("ApprovedRoot");
        let app_dir = root.join("SampleApp");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(app_dir.join("settings.json"), "{}").unwrap();

        assert_eq!(validate_tree(&app_dir, &[root.clone()], 100).unwrap(), 2);
        assert!(validate_tree(&root, &[root.clone()], 100).is_err());
        assert!(validate_tree(&app_dir, &[root.clone()], 1).is_err());

        let outside = tmp.path().join("OutsideApp");
        fs::create_dir_all(&outside).unwrap();
        assert!(validate_tree(&outside, &[root.clone()], 100).is_err());

        // Symlink/junction check if privilege allows creating directory symlinks;
        // if symlink creation is disallowed on non-admin Windows, test remains valid.
        let link_path = root.join("LinkedApp");
        if symlink_dir(&app_dir, &link_path).is_ok() {
            assert!(validate_tree(&link_path, &[root.clone()], 100).is_err());
            let nested = app_dir.join("nested-link");
            if symlink_dir(&outside, &nested).is_ok() {
                assert!(validate_tree(&app_dir, &[root], 100).is_err());
            }
        }
    }

    #[test]
    fn uninstall_command_parser_accepts_safe_commands_and_rejects_shells() {
        assert_eq!(
            parse_uninstall_command("MsIeXeC /I{123-ABC}").unwrap(),
            ("msiexec.exe".into(), "/X{123-ABC}".into())
        );
        assert!(parse_uninstall_command("msiexec-malicious /X{123-ABC}").is_err());
        assert_eq!(
            parse_uninstall_command(r#""C:\Program Files\App\uninstall.exe" /remove"#).unwrap(),
            (
                r"C:\Program Files\App\uninstall.exe".to_string(),
                "/remove".to_string()
            )
        );
        assert_eq!(
            parse_uninstall_command("MsiExec.exe /I{123-ABC}").unwrap(),
            ("msiexec.exe".to_string(), "/X{123-ABC}".to_string())
        );
        assert_eq!(
            parse_uninstall_command(r"C:\Apps\uninstall.exe /S").unwrap(),
            (r"C:\Apps\uninstall.exe".to_string(), "/S".to_string())
        );
        for bad in [
            "cmd /c del x",
            r"C:\Windows\System32\cmd.exe /c del x",
            "uninstall.exe /remove",
            "powershell.exe -Command x",
            "https://example.com",
            "",
        ] {
            assert!(
                parse_uninstall_command(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn registration_target_restricts_to_uninstall_subkeys() {
        let good = r"Microsoft.PowerShell.Core\Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\ExampleApp";
        let parsed = parse_uninstall_target(good).unwrap();
        assert_eq!(
            parsed.native,
            r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\ExampleApp"
        );
        let user32 = parse_uninstall_target(r"HKEY_CURRENT_USER\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\UserTool").unwrap();
        assert_eq!(user32.hive, HiveKind::Hkcu);
        assert!(!user32.wow64_64);
        for bad in [
            r"HKEY_LOCAL_MACHINE\Software",
            r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall",
            r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\App\Subkey",
        ] {
            assert!(
                parse_uninstall_target(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn scan_cleanup_quarantine_and_restore_respect_active_and_no_overwrite() {
        let tmp = TempDirGuard::new("cleanup");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let approved = tmp.path().join("ApprovedRoot");
        let app_dir = approved.join("AcmeTool");
        let shared_dir = approved.join("SharedSuite");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&shared_dir).unwrap();
        fs::write(app_dir.join("prefs.ini"), "keep=1").unwrap();

        sys.hkcu_software
            .borrow_mut()
            .insert("AcmeTool".to_string());
        sys.reg_keys
            .borrow_mut()
            .insert(r"hkey_current_user\software\acmetool".to_string());

        let active_app = AppRecord {
            id: r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\AcmeTool"
                .to_string(),
            name: "AcmeTool".to_string(),
            location: app_dir.to_string_lossy().into_owned(),
            command: r"C:\Apps\AcmeTool\uninstall.exe /S".to_string(),
            ..Default::default()
        };
        let other_app = AppRecord {
            id: r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\OtherApp"
                .to_string(),
            name: "OtherApp".to_string(),
            location: shared_dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        sys.installed
            .borrow_mut()
            .extend([active_app.clone(), other_app]);

        let plan = scan_leftovers_with(&sys, "AcmeTool", Some(&active_app)).unwrap();
        assert!(plan.active);
        assert_eq!(plan.candidates.len(), 2);
        assert!(plan.candidates.iter().all(|c| !c.blocked));

        let cand_ids: Vec<String> = plan.candidates.iter().map(|c| c.id.clone()).collect();
        // Normal cleanup refuses when app is still installed.
        let err = cleanup_with(&sys, &plan.id, &cand_ids, false).unwrap_err();
        assert!(err.contains("still installed"));

        // Forced cleanup succeeds and consumes the plan.
        let res = cleanup_with(&sys, &plan.id, &cand_ids, true).unwrap();
        assert_eq!(res.len(), 2);
        assert!(
            res.iter()
                .all(|item| item.backup_id.is_some() && item.error.is_none())
        );
        assert!(!app_dir.exists());
        assert!(sys.hkcu_software.borrow().is_empty());

        // Plan cannot be replayed.
        assert!(cleanup_with(&sys, &plan.id, &cand_ids, true).is_err());

        let backups = list_backups_with(&sys).unwrap();
        assert_eq!(backups.len(), 2);
        let dir_backup = backups.iter().find(|b| b.kind == "directory").unwrap();
        let reg_backup = backups.iter().find(|b| b.kind == "registry").unwrap();

        // Simulate interruption after the directory moved but before metadata
        // advanced: the durable prepared record must still restore the files.
        let dir = sys.backups_dir();
        let mut interrupted = load_backup_record(&dir, &dir_backup.id).unwrap().unwrap();
        interrupted.status = "prepared".into();
        save_backup_record(&dir, &interrupted).unwrap();
        assert_eq!(
            load_backup_record(&dir, &dir_backup.id)
                .unwrap()
                .unwrap()
                .status,
            "prepared"
        );
        assert!(!dir.join(format!("{}.json.tmp", dir_backup.id)).exists());

        // No-overwrite restoration check.
        fs::create_dir_all(&app_dir).unwrap();
        assert!(restore_backup_with(&sys, &dir_backup.id).is_err());
        fs::remove_dir_all(&app_dir).unwrap();

        restore_backup_with(&sys, &dir_backup.id).unwrap();
        assert_eq!(
            fs::read_to_string(app_dir.join("prefs.ini")).unwrap(),
            "keep=1"
        );

        restore_backup_with(&sys, &reg_backup.id).unwrap();
        assert!(sys.hkcu_software.borrow().contains("AcmeTool"));
        // Restoring an already-restored backup fails.
        assert!(restore_backup_with(&sys, &reg_backup.id).is_err());
    }

    #[test]
    fn startup_disable_and_restore_are_current_user_only_and_no_overwrite() {
        let tmp = TempDirGuard::new("startup");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        sys.startup_run.borrow_mut().insert(
            "MyHelper".to_string(),
            StartupValueInfo {
                name: "MyHelper".to_string(),
                command: r"%LOCALAPPDATA%\MyHelper\helper.exe --tray".to_string(),
                kind_label: "ExpandString".to_string(),
            },
        );
        let machine_id = format!("{HKLM_RUN_KEY}|SysHelper");
        sys.startup_readonly.borrow_mut().push(StartupEntry {
            id: machine_id.clone(),
            name: "SysHelper".to_string(),
            command: r"C:\Program Files\Sys\helper.exe".to_string(),
            scope: "Machine".to_string(),
            editable: false,
        });

        assert!(disable_startup_with(&sys, &machine_id).is_err());
        let user_id = format!("{HKCU_RUN_KEY}|MyHelper");
        disable_startup_with(&sys, &user_id).unwrap();
        assert!(!sys.startup_run.borrow().contains_key("MyHelper"));

        let backups = list_backups_with(&sys).unwrap();
        assert_eq!(backups.len(), 1);
        let bid = backups[0].id.clone();

        // Refuse overwrite if value exists again.
        sys.startup_run.borrow_mut().insert(
            "MyHelper".to_string(),
            StartupValueInfo {
                name: "MyHelper".to_string(),
                command: "other.exe".to_string(),
                kind_label: "String".to_string(),
            },
        );
        assert!(restore_backup_with(&sys, &bid).is_err());

        sys.startup_run.borrow_mut().remove("MyHelper");
        restore_backup_with(&sys, &bid).unwrap();
        let restored = sys.startup_run.borrow().get("MyHelper").unwrap().clone();
        assert_eq!(restored.kind_label, "ExpandString");
        assert_eq!(
            restored.command,
            r"%LOCALAPPDATA%\MyHelper\helper.exe --tray"
        );
    }

    #[test]
    fn registration_removal_store_recheck_and_restore_point_validation() {
        let tmp = TempDirGuard::new("regstore");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let reg_id =
            r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\GhostApp";
        let app = AppRecord {
            id: reg_id.to_string(),
            name: "GhostApp".to_string(),
            command: "MsiExec.exe /I{1111-2222}".to_string(),
            ..Default::default()
        };

        // Fails when not live in inventory.
        assert!(remove_registration_with(&sys, &app).is_err());
        assert!(uninstall_with(&sys, &app).is_err());

        sys.installed.borrow_mut().push(app.clone());
        sys.reg_keys
            .borrow_mut()
            .insert(reg_id.to_ascii_lowercase());

        uninstall_with(&sys, &app).unwrap();
        assert_eq!(
            sys.launched_uninstalls.borrow().as_slice(),
            &[("msiexec.exe".to_string(), "/X{1111-2222}".to_string())]
        );
        sys.installed.borrow_mut()[0].command.clear();
        assert!(
            uninstall_with(&sys, &app)
                .unwrap_err()
                .contains("No registered uninstaller")
        );
        assert_eq!(sys.launched_uninstalls.borrow().len(), 1);
        sys.installed.borrow_mut()[0].command = "MsiExec.exe /I{3333-4444}".into();
        uninstall_with(&sys, &app).unwrap();
        assert_eq!(sys.launched_uninstalls.borrow()[1].1, "/X{3333-4444}");
        sys.installed.borrow_mut()[0].name = "Different product".into();
        assert!(
            uninstall_with(&sys, &app)
                .unwrap_err()
                .contains("registration changed")
        );
        assert_eq!(sys.launched_uninstalls.borrow().len(), 2);
        assert!(remove_registration_with(&sys, &app).is_err());
        assert!(sys.registry_key_exists(reg_id).unwrap());
        sys.installed.borrow_mut()[0].name = app.name.clone();

        sys.fail_export.set(true);
        assert!(remove_registration_with(&sys, &app).is_err());
        assert!(list_backups_with(&sys).unwrap().is_empty());
        assert!(sys.registry_key_exists(reg_id).unwrap());
        sys.fail_export.set(false);
        sys.wrong_export_key.set(true);
        assert!(remove_registration_with(&sys, &app).is_err());
        assert!(list_backups_with(&sys).unwrap().is_empty());
        assert!(sys.registry_key_exists(reg_id).unwrap());
        sys.wrong_export_key.set(false);
        remove_registration_with(&sys, &app).unwrap();
        assert!(!sys.registry_key_exists(reg_id).unwrap());

        let backups = list_backups_with(&sys).unwrap();
        assert_eq!(backups.len(), 1);
        let payload = sys
            .backups_dir()
            .join("registry")
            .join(format!("{}.reg", backups[0].id));
        let good = fs::read(&payload).unwrap();
        let other_key = r"HKEY_CURRENT_USER\Software\OtherProduct";
        fs::write(
            &payload,
            format!("Windows Registry Editor Version 5.00\r\n[{reg_id}]\r\n[{other_key}]\r\n"),
        )
        .unwrap();
        assert!(restore_backup_with(&sys, &backups[0].id).is_err());
        assert!(!sys.registry_key_exists(other_key).unwrap());
        assert!(!sys.registry_key_exists(reg_id).unwrap());
        fs::write(&payload, good).unwrap();
        restore_backup_with(&sys, &backups[0].id).unwrap();
        assert!(sys.registry_key_exists(reg_id).unwrap());

        sys.store.borrow_mut().extend([
            StoreApp {
                id: "Pkg.Protected_1.0_x64__abc".to_string(),
                name: "Protected".to_string(),
                version: "1.0".to_string(),
                publisher: "Microsoft".to_string(),
                removable: false,
            },
            StoreApp {
                id: "Pkg.Removable_1.0_x64__abc".to_string(),
                name: "Removable".to_string(),
                version: "1.0".to_string(),
                publisher: "Vendor".to_string(),
                removable: true,
            },
        ]);
        assert!(remove_store_with(&sys, "Pkg.Protected_1.0_x64__abc").is_err());
        assert!(remove_store_with(&sys, "Pkg.Removable;calc.exe").is_err());
        remove_store_with(&sys, "Pkg.Removable_1.0_x64__abc").unwrap();
        assert_eq!(
            sys.removed_store_ids.borrow().as_slice(),
            &["Pkg.Removable_1.0_x64__abc".to_string()]
        );

        assert!(create_restore_point_with(&sys).unwrap().contains("42"));
        *sys.restore_point_seq.borrow_mut() = None;
        assert!(create_restore_point_with(&sys).is_err());
    }

    #[test]
    fn base64_and_ps_literal_encode_correctly() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(ps_literal("App's; $(test)"), "'App''s; $(test)'");
    }

    #[test]
    fn cleanup_accepts_candidate_paths_and_blocks_shared_locations() {
        let tmp = TempDirGuard::new("shared");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let approved = tmp.path().join("ApprovedRoot");
        let shared_dir = approved.join("SharedApp");
        let inside_shared = shared_dir.join("NestedApp");
        fs::create_dir_all(&inside_shared).unwrap();

        sys.installed.borrow_mut().push(AppRecord {
            id: r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\Nested"
                .to_string(),
            name: "Nested".to_string(),
            location: inside_shared.to_string_lossy().into_owned(),
            ..Default::default()
        });

        let plan = scan_leftovers_with(&sys, "SharedApp", None).unwrap();
        assert_eq!(plan.candidates.len(), 1);
        assert!(plan.candidates[0].blocked);

        // Selecting a blocked candidate by path (as UI does) fails safely without moving it.
        let out = cleanup_with(&sys, &plan.id, &[plan.candidates[0].path.clone()], true).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].error.is_some());
        assert!(shared_dir.exists());

        // Non-blocked directory selected by candidate.path succeeds.
        let clean_dir = approved.join("CleanApp");
        fs::create_dir_all(&clean_dir).unwrap();
        let plan2 = scan_leftovers_with(&sys, "CleanApp", None).unwrap();
        assert!(!plan2.candidates[0].blocked);
        let out2 =
            cleanup_with(&sys, &plan2.id, &[plan2.candidates[0].path.clone()], false).unwrap();
        assert!(out2[0].backup_id.is_some());
        assert!(!clean_dir.exists());
    }

    #[test]
    fn recorded_location_is_found_even_when_display_name_contains_version() {
        let tmp = TempDirGuard::new("recorded-location");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let location = tmp.path().join("ApprovedRoot/Vendor/AcmeTool");
        fs::create_dir_all(location.join("Profiles/nested")).unwrap();
        let bytes = vec![0u8, 255, 23, 10, 13, 0, 42];
        fs::write(location.join("Profiles/nested/settings.bin"), &bytes).unwrap();
        let app = AppRecord {
            id: "test-registration".into(),
            name: "AcmeTool 2026 (64-bit)".into(),
            location: location.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let plan = scan_leftovers_with(&sys, &app.name, Some(&app)).unwrap();
        assert_eq!(plan.candidates.len(), 1);
        assert!(
            plan.candidates[0]
                .reason
                .contains("Recorded install location")
        );
        assert!(!plan.candidates[0].blocked);
        let results =
            cleanup_with(&sys, &plan.id, &[plan.candidates[0].id.clone()], false).unwrap();
        assert!(results[0].error.is_none());
        assert!(!location.exists());
        let id = results[0].backup_id.as_ref().unwrap();
        let record = load_backup_record(&sys.backups_dir(), id).unwrap().unwrap();
        assert_eq!(
            fs::read(
                Path::new(record.backup_path.as_ref().unwrap())
                    .join("Profiles/nested/settings.bin")
            )
            .unwrap(),
            bytes
        );
        restore_backup_with(&sys, id).unwrap();
        assert_eq!(
            fs::read(location.join("Profiles/nested/settings.bin")).unwrap(),
            bytes
        );
        // A different manual name must not inherit another app's location.
        assert!(
            scan_leftovers_with(&sys, "DifferentApp", Some(&app))
                .unwrap()
                .candidates
                .is_empty()
        );
        for invalid in [
            tmp.path().join("Outside"),
            tmp.path().join("ApprovedRoot"),
            tmp.path().join("ApprovedRoot/Common Files"),
        ] {
            fs::create_dir_all(&invalid).unwrap();
            let bad = AppRecord {
                location: invalid.to_string_lossy().into_owned(),
                ..app.clone()
            };
            assert!(
                scan_leftovers_with(&sys, &app.name, Some(&bad))
                    .unwrap()
                    .candidates
                    .is_empty()
            );
        }
    }

    #[test]
    fn selections_are_validated_and_new_shared_owners_are_rechecked() {
        let tmp = TempDirGuard::new("selection-and-owner");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let location = tmp.path().join("ApprovedRoot/AcmeTool");
        fs::create_dir_all(&location).unwrap();
        let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        assert!(cleanup_with(&sys, &plan.id, &["invented-id".into()], false).is_err());
        assert!(cleanup_with(&sys, &plan.id, &[], false).is_err());
        assert!(location.exists());
        sys.installed.borrow_mut().push(AppRecord {
            id: "new-owner".into(),
            name: "Other product".into(),
            location: location.to_string_lossy().into_owned(),
            ..Default::default()
        });
        let results = cleanup_with(&sys, &plan.id, &[plan.candidates[0].id.clone()], true).unwrap();
        assert!(
            results[0]
                .error
                .as_ref()
                .unwrap()
                .contains("Another registered app")
        );
        assert!(location.exists());
        assert!(list_backups_with(&sys).unwrap().is_empty());
    }

    #[test]
    fn registry_cleanup_preserves_a_key_that_changes_after_backup() {
        let tmp = TempDirGuard::new("registry-cleanup-changed");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let key = r"HKEY_CURRENT_USER\Software\AcmeTool";
        sys.hkcu_software.borrow_mut().insert("AcmeTool".into());
        sys.reg_keys.borrow_mut().insert(key.to_ascii_lowercase());
        sys.change_during_export.set(true);

        let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        let candidate = plan
            .candidates
            .iter()
            .find(|entry| entry.kind == "registry")
            .unwrap();
        let results =
            cleanup_with(&sys, &plan.id, std::slice::from_ref(&candidate.id), false).unwrap();
        assert!(
            results[0]
                .error
                .as_ref()
                .unwrap()
                .contains("registry key changed")
        );
        assert!(sys.registry_key_exists(key).unwrap());
        assert!(list_backups_with(&sys).unwrap().is_empty());
        assert_eq!(
            fs::read_dir(sys.backups_dir().join("registry"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn mixed_cleanup_reports_failures_and_never_deletes_without_registry_backup() {
        let tmp = TempDirGuard::new("partial-cleanup");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let location = tmp.path().join("ApprovedRoot/AcmeTool");
        fs::create_dir_all(&location).unwrap();
        fs::write(location.join("prefs.txt"), "preserved").unwrap();
        sys.hkcu_software.borrow_mut().insert("AcmeTool".into());
        sys.reg_keys
            .borrow_mut()
            .insert(r"hkey_current_user\software\acmetool".into());
        let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        assert_eq!(plan.candidates.len(), 2);
        sys.fail_export.set(true);
        let ids: Vec<_> = plan.candidates.iter().map(|c| c.id.clone()).collect();
        let results = cleanup_with(&sys, &plan.id, &ids, false).unwrap();
        assert_eq!(results.iter().filter(|r| r.backup_id.is_some()).count(), 1);
        assert_eq!(results.iter().filter(|r| r.error.is_some()).count(), 1);
        assert!(sys.hkcu_software.borrow().contains("AcmeTool"));
        assert!(
            sys.reg_keys
                .borrow()
                .contains(r"hkey_current_user\software\acmetool")
        );
        assert!(!location.exists());
        assert_eq!(
            list_backups_with(&sys).unwrap().len(),
            1,
            "Failed export must not advertise a nonexistent backup"
        );
        restore_backup_with(
            &sys,
            results.iter().find_map(|r| r.backup_id.as_ref()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(location.join("prefs.txt")).unwrap(),
            "preserved"
        );
        // Retrying requires a new scan; an old plan cannot replay a partial operation.
        assert!(cleanup_with(&sys, &plan.id, &ids, false).is_err());
        let fresh = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        let registry = fresh
            .candidates
            .iter()
            .find(|c| c.kind == "registry")
            .unwrap();
        sys.fail_export.set(false);
        let retry = cleanup_with(&sys, &fresh.id, &[registry.id.clone()], false).unwrap();
        assert!(retry[0].backup_id.is_some());
        assert!(!sys.hkcu_software.borrow().contains("AcmeTool"));
    }

    #[test]
    fn expired_plans_and_incomplete_scans_are_reported() {
        let tmp = TempDirGuard::new("expired-scan");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let incomplete = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        assert!(!incomplete.warnings.is_empty());
        let location = tmp.path().join("ApprovedRoot/AcmeTool");
        fs::create_dir_all(&location).unwrap();
        let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        PLANS.lock().unwrap().get_mut(&plan.id).unwrap().created_at =
            Instant::now() - PLAN_TTL - Duration::from_secs(1);
        assert!(
            cleanup_with(&sys, &plan.id, &[plan.candidates[0].id.clone()], false)
                .unwrap_err()
                .contains("expired")
        );
        assert!(location.exists());
    }

    #[test]
    fn nested_product_candidates_preserve_vendor_siblings_and_roundtrip_files() {
        let tmp = TempDirGuard::new("nested-products");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let root = tmp.path().join("ApprovedRoot");
        let product = root.join("Vendor/AcmeTool");
        let sibling = root.join("Vendor/AcmeToolUpdater");
        fs::create_dir_all(&product).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::create_dir_all(root.join("Microsoft/AcmeTool")).unwrap();
        fs::write(product.join("settings.bin"), [0, 255, 42]).unwrap();
        let settings = root.join("Vendor/AcmeTool.json");
        fs::write(&settings, b"{\"preserve\":true}").unwrap();
        fs::write(root.join("Vendor/AcmeTool.pdf"), "unrelated document").unwrap();
        sys.hkcu_software
            .borrow_mut()
            .extend([r"Vendor\AcmeTool".into(), r"Vendor\OtherProduct".into()]);
        sys.reg_keys.borrow_mut().extend([
            r"hkey_current_user\software\vendor\acmetool".into(),
            r"hkey_current_user\software\vendor\otherproduct".into(),
        ]);
        let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        assert_eq!(plan.candidates.len(), 3);
        assert!(plan.candidates.iter().any(|c| c.kind == "file"));
        let ids = plan
            .candidates
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>();
        let results = cleanup_with(&sys, &plan.id, &ids, false).unwrap();
        assert!(results.iter().all(|r| r.error.is_none()), "{results:#?}");
        assert!(!product.exists());
        assert!(!settings.exists());
        assert!(sibling.exists());
        assert!(root.join("Vendor/AcmeTool.pdf").exists());
        assert!(root.join("Microsoft/AcmeTool").exists());
        assert!(sys.hkcu_software.borrow().contains(r"Vendor\OtherProduct"));
        assert!(!sys.hkcu_software.borrow().contains(r"Vendor\AcmeTool"));
        for result in &results {
            if result.path.ends_with("AcmeTool.json") {
                fs::write(&settings, "new settings").unwrap();
                assert!(restore_backup_with(&sys, result.backup_id.as_ref().unwrap()).is_err());
                assert_eq!(fs::read_to_string(&settings).unwrap(), "new settings");
                fs::remove_file(&settings).unwrap();
            }
            restore_backup_with(&sys, result.backup_id.as_ref().unwrap()).unwrap();
        }
        assert_eq!(
            fs::read(product.join("settings.bin")).unwrap(),
            [0, 255, 42]
        );
        assert_eq!(fs::read(&settings).unwrap(), b"{\"preserve\":true}");
        assert!(sys.hkcu_software.borrow().contains(r"Vendor\AcmeTool"));
    }

    #[test]
    #[ignore = "creates and removes a unique temporary HKCU fixture"]
    fn windows_registry_backup_roundtrip() {
        use winreg::enums::RegDisposition::REG_CREATED_NEW_KEY;

        let dir = TempDirGuard::new("registry-roundtrip");
        let id = next_id();
        let child = format!("SimpleDeleteVerification-{id}");
        let subkey = format!(r"Software\{child}");
        let native = format!(r"HKEY_CURRENT_USER\{subkey}");
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, disposition) = hkcu.create_subkey(&subkey).unwrap();
        assert!(matches!(disposition, REG_CREATED_NEW_KEY));
        struct RemoveFixture(String);
        impl Drop for RemoveFixture {
            fn drop(&mut self) {
                let _ = RealSystem.delete_hkcu_software_child(&self.0);
            }
        }
        let _cleanup = RemoveFixture(child.clone());
        key.set_value("Marker", &"round trip").unwrap();
        let (nested, _) = key.create_subkey("Nested").unwrap();
        nested.set_value("Count", &42u32).unwrap();
        drop((nested, key));

        let payload = dir.path().join("registry").join(format!("{id}.reg"));
        let backup = export_validated_registry_key(&RealSystem, &native, &payload).unwrap();
        let occupied = payload.with_file_name("occupied.reg");
        fs::write(&occupied, b"existing data").unwrap();
        assert!(RealSystem.export_registry_key(&native, &occupied).is_err());
        assert_eq!(fs::read(&occupied).unwrap(), b"existing data");
        verify_registry_export_unchanged(&RealSystem, &native, &payload).unwrap();
        let key = hkcu.open_subkey_with_flags(&subkey, KEY_WRITE).unwrap();
        let nested = key.open_subkey_with_flags("Nested", KEY_WRITE).unwrap();
        nested.set_value("Count", &43u32).unwrap();
        assert!(
            verify_registry_export_unchanged(&RealSystem, &native, &payload)
                .unwrap_err()
                .contains("changed")
        );
        nested.set_value("Count", &42u32).unwrap();
        drop((nested, key));
        RealSystem.delete_hkcu_software_child(&child).unwrap();
        assert!(!RealSystem.registry_key_exists(&native).unwrap());
        drop(backup);
        restore_registry_payload(
            &RealSystem,
            dir.path(),
            &id,
            payload.to_str().unwrap(),
            &native,
        )
        .unwrap();
        let restored = hkcu.open_subkey(&subkey).unwrap();
        assert_eq!(
            restored.get_value::<String, _>("Marker").unwrap(),
            "round trip"
        );
        assert_eq!(
            restored
                .open_subkey("Nested")
                .unwrap()
                .get_value::<u32, _>("Count")
                .unwrap(),
            42
        );
        assert!(
            restore_registry_payload(
                &RealSystem,
                dir.path(),
                &id,
                payload.to_str().unwrap(),
                &native
            )
            .is_err()
        );
        drop(restored);
        RealSystem.delete_hkcu_software_child(&child).unwrap();
        assert!(!RealSystem.registry_key_exists(&native).unwrap());
    }

    #[test]
    fn registry_export_scope_accepts_utf16_and_rejects_deletion_sections() {
        let root = r"HKEY_CURRENT_USER\Software\AcmeTool";
        let encode = |body: String| {
            let text = format!("\u{feff}Windows Registry Editor Version 5.00\r\n\r\n{body}\r\n");
            text.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        assert!(
            validate_registry_export(&encode(format!("[{root}]\r\n[{root}\\Preferences]")), root)
                .is_ok()
        );
        for body in [
            format!("[{root}\\Preferences]"),
            format!("[{root}]\r\n[-{root}\\Preferences]"),
            format!("[{root}]\r\n[{root}Sibling]"),
        ] {
            assert!(validate_registry_export(&encode(body), root).is_err());
        }
    }

    #[test]
    fn selective_recovery_preserves_other_backups_and_reports_missing_data() {
        let tmp = TempDirGuard::new("selective-recovery");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let root = tmp.path().join("ApprovedRoot");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("AcmeTool.ini");
        let second = root.join("AcmeTool.json");
        fs::write(&first, b"first settings").unwrap();
        fs::write(&second, b"second settings").unwrap();
        let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        let ids: Vec<_> = plan.candidates.iter().map(|c| c.id.clone()).collect();
        let results = cleanup_with(&sys, &plan.id, &ids, false).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.error.is_none()));
        let first_id = results
            .iter()
            .find(|r| same_path(Path::new(&r.path), &first))
            .unwrap()
            .backup_id
            .as_ref()
            .unwrap();
        let second_id = results
            .iter()
            .find(|r| same_path(Path::new(&r.path), &second))
            .unwrap()
            .backup_id
            .as_ref()
            .unwrap();
        restore_backup_with(&sys, first_id).unwrap();
        assert_eq!(fs::read(&first).unwrap(), b"first settings");
        assert!(!second.exists());
        let pending = load_backup_record(&sys.backups_dir(), second_id)
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, "quarantined");
        let backup_path = Path::new(pending.backup_path.as_ref().unwrap());
        assert_eq!(fs::read(backup_path).unwrap(), b"second settings");
        for (change, message) in [
            ("path", "quarantine record"),
            ("root", "outside the approved roots"),
            ("type", "recorded type"),
            ("id", "does not match identifier"),
        ] {
            let mut altered = pending.clone();
            match change {
                "path" => altered.backup_path = Some(first.to_string_lossy().into_owned()),
                "root" => {
                    altered.original = tmp
                        .path()
                        .join("Outside.ini")
                        .to_string_lossy()
                        .into_owned();
                    altered.backup_path = Some(
                        tmp.path()
                            .join(".simple-delete-quarantine")
                            .join(second_id)
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                "type" => altered.kind = "directory".into(),
                "id" => altered.id = first_id.clone(),
                _ => unreachable!(),
            }
            fs::write(
                sys.backups_dir().join(format!("{second_id}.json")),
                serde_json::to_vec(&altered).unwrap(),
            )
            .unwrap();
            assert!(
                restore_backup_with(&sys, second_id)
                    .unwrap_err()
                    .contains(message)
            );
            if change == "id" {
                assert_eq!(list_backups_with(&sys).unwrap().len(), 1);
            }
            assert!(!second.exists());
            assert_eq!(fs::read(backup_path).unwrap(), b"second settings");
        }
        save_backup_record(&sys.backups_dir(), &pending).unwrap();
        // Simulate loss of this fixture's backup: restoration must fail without
        // marking it restored or affecting the already restored sibling.
        fs::remove_file(backup_path).unwrap();
        assert!(restore_backup_with(&sys, second_id).is_err());
        let failed = load_backup_record(&sys.backups_dir(), second_id)
            .unwrap()
            .unwrap();
        assert_eq!(failed.status, "quarantined");
        assert!(failed.restored_at_ms.is_none());
        assert!(!second.exists());
        assert_eq!(fs::read(&first).unwrap(), b"first settings");
        fs::write(
            sys.backups_dir().join(format!("{second_id}.json")),
            b"invalid JSON",
        )
        .unwrap();
        assert!(restore_backup_with(&sys, second_id).is_err());
        assert!(!second.exists());
    }

    #[test]
    fn short_vendor_parents_allow_only_exact_product_cleanup() {
        let tmp = TempDirGuard::new("short-vendors");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let root = tmp.path().join("ApprovedRoot");
        for vendor in ["IBM", "HP", "EA"] {
            let product = root.join(vendor).join("AcmeTool");
            let sibling = root.join(vendor).join("OtherProduct");
            fs::create_dir_all(&product).unwrap();
            fs::create_dir_all(&sibling).unwrap();
            fs::write(product.join("settings.bin"), b"preserve").unwrap();
            let key = format!(r"{vendor}\AcmeTool");
            let sibling_key = format!(r"{vendor}\OtherProduct");
            sys.hkcu_software
                .borrow_mut()
                .extend([key.clone(), sibling_key.clone()]);
            sys.reg_keys
                .borrow_mut()
                .insert(format!(r"hkey_current_user\software\{key}").to_ascii_lowercase());
            let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
            let ids: Vec<_> = plan
                .candidates
                .iter()
                .filter(|c| {
                    same_path(Path::new(&c.path), &product)
                        || c.path == format!(r"HKEY_CURRENT_USER\Software\{key}")
                })
                .map(|c| c.id.clone())
                .collect();
            assert_eq!(ids.len(), 2, "Missing product under {vendor}");
            let results = cleanup_with(&sys, &plan.id, &ids, false).unwrap();
            assert!(results.iter().all(|r| r.error.is_none()), "{results:#?}");
            assert!(!product.exists());
            assert!(sibling.exists());
            assert!(sys.hkcu_software.borrow().contains(&sibling_key));
            assert!(!sys.hkcu_software.borrow().contains(&key));
            for result in results {
                restore_backup_with(&sys, result.backup_id.as_ref().unwrap()).unwrap();
            }
            assert_eq!(fs::read(product.join("settings.bin")).unwrap(), b"preserve");
            assert!(sys.hkcu_software.borrow().contains(&key));
            assert!(scan_leftovers_with(&sys, vendor, None).is_err());
        }
        for invalid in [
            r"HKCU\Software\HP",
            r"HKCU\Software\IBM\HP",
            r"HKCU\Software\..\AcmeTool",
            r"HKCU\Software\Microsoft\AcmeTool",
        ] {
            assert!(parse_hkcu_software_child(invalid).is_err());
        }
    }

    #[test]
    fn discovery_limits_warn_and_changed_file_types_are_refused() {
        let tmp = TempDirGuard::new("scan-bounds");
        let sys = FixtureEnv::new(tmp.path().to_path_buf());
        let root = tmp.path().join("ApprovedRoot");
        fs::create_dir_all(root.join("Vendor")).unwrap();
        let settings = root.join("AcmeTool.ini");
        fs::write(&settings, "keep=1").unwrap();
        let (_, warnings) = scan_filesystem(&[root.clone()], "acmetool", &sys.app_data_root(), 1);
        assert!(warnings.iter().any(|w| w.contains("limit")));
        sys.hkcu_software
            .borrow_mut()
            .extend([r"Vendor\AcmeTool".into(), r"OtherVendor\AcmeTool".into()]);
        let (_, warnings) = scan_registry(&sys, "acmetool", 1);
        assert!(warnings.iter().any(|w| w.contains("limit")));
        let plan = scan_leftovers_with(&sys, "AcmeTool", None).unwrap();
        let file = plan.candidates.iter().find(|c| c.kind == "file").unwrap();
        fs::remove_file(&settings).unwrap();
        fs::create_dir(&settings).unwrap();
        let results = cleanup_with(&sys, &plan.id, &[file.id.clone()], false).unwrap();
        assert!(results[0].error.as_ref().unwrap().contains("type changed"));
        assert!(settings.is_dir());
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();
        assert!(move_without_overwrite(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "source");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
        for invalid in [
            r"HKCU\Software\Microsoft\AcmeTool",
            r"HKCU\Software\Vendor\AcmeTool\Child",
            r"HKCU\Software\Vendor\..",
            r"HKCU\Software\Classes\AcmeTool",
        ] {
            assert!(parse_hkcu_software_child(invalid).is_err());
        }
    }

    #[test]
    fn expand_env_vars_handles_known_unknown_and_unclosed_percent() {
        let expanded = expand_env_vars(r"%SystemRoot%\System32\msiexec.exe");
        assert!(!expanded.is_empty());
        assert_eq!(
            expand_env_vars(r"%SIMPLE_DELETE_NONEXISTENT_VAR_999%\app.exe"),
            r"%SIMPLE_DELETE_NONEXISTENT_VAR_999%\app.exe"
        );
        assert_eq!(expand_env_vars(r"C:\App\foo%bar"), r"C:\App\foo%bar");
    }
}
