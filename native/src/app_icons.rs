//! Local icon discovery. Images are read on a worker, never while laying out a row.
use crate::{backend, icon_pixels, model::AppRecord};
use eframe::egui::ColorImage;
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::mpsc::Sender,
};
use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY},
};

pub struct LoadedIcon {
    pub id: String,
    pub image: ColorImage,
}

fn expanded(value: &str) -> String {
    let mut out = String::new();
    let mut remaining = value;
    while let Some(start) = remaining.find('%') {
        out.push_str(&remaining[..start]);
        let rest = &remaining[start + 1..];
        let Some(end) = rest.find('%') else {
            out.push_str(&remaining[start..]);
            return out;
        };
        match std::env::var(&rest[..end]) {
            Ok(value) => out.push_str(&value),
            Err(_) => {
                out.push('%');
                out.push_str(&rest[..end]);
                out.push('%');
            }
        }
        remaining = &rest[end + 1..];
    }
    out.push_str(remaining);
    out
}

fn display_icon(value: &str) -> (PathBuf, i32) {
    let value = value.trim().trim_start_matches('@');
    let (path, index) = value
        .rsplit_once(',')
        .and_then(|(path, index)| index.trim().parse::<i32>().ok().map(|index| (path, index)))
        .unwrap_or((value, 0));
    (
        PathBuf::from(expanded(path.trim().trim_matches('"'))),
        index,
    )
}

fn registered_icon(id: &str) -> Option<(PathBuf, i32)> {
    if !crate::catalog::is_registration(id) {
        return None;
    }
    let (hive, path) = if let Some(path) = id.strip_prefix("HKEY_LOCAL_MACHINE\\") {
        (HKEY_LOCAL_MACHINE, path)
    } else {
        (HKEY_CURRENT_USER, id.strip_prefix("HKEY_CURRENT_USER\\")?)
    };
    let key = RegKey::predef(hive)
        .open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
        .ok()?;
    let raw = key.get_value::<String, _>("DisplayIcon").ok()?;
    (!raw.trim().is_empty()).then(|| display_icon(&raw))
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn executable_in_folder(app: &AppRecord) -> Option<PathBuf> {
    let root = Path::new(&app.location);
    // Never enumerate arbitrary network locations from an uninstall registration.
    if !icon_pixels::is_local_path(root) {
        return None;
    }
    let mut choices = Vec::new();
    for entry in std::fs::read_dir(root).ok()?.take(128).flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            continue;
        }
        let stem = path.file_stem()?.to_string_lossy().to_lowercase();
        if [
            "unins",
            "setup",
            "update",
            "crash",
            "helper",
            "report",
            "maintenancetool",
        ]
        .iter()
        .any(|word| stem.contains(word))
        {
            continue;
        }
        if !icon_pixels::is_local_path(&path) {
            continue;
        }
        choices.push(path);
    }
    choices.sort();
    let name = normalized(&app.name);
    let matches: Vec<_> = choices
        .iter()
        .filter(|path| {
            let stem = normalized(&path.file_stem().unwrap_or_default().to_string_lossy());
            stem.len() >= 4 && (name.contains(&stem) || stem.contains(&name))
        })
        .collect();
    if matches.len() == 1 {
        Some(matches[0].clone())
    } else if choices.len() == 1 {
        choices.pop()
    } else {
        None
    }
}

#[derive(Deserialize)]
struct StoreLogo {
    id: String,
    folder: String,
    logo: String,
}

fn store_logos() -> HashMap<String, PathBuf> {
    let script = r#"$items=@(foreach($pkg in @(Get-AppxPackage)){try{$m=Get-AppxPackageManifest -Package $pkg.PackageFullName;$v=$m.SelectSingleNode(\"//*[local-name()='Application']/*[local-name()='VisualElements']\");$logo='';if($v){$logo=$v.GetAttribute('Square44x44Logo');if(-not $logo){$logo=$v.GetAttribute('Square150x150Logo')}};if(-not $logo){$n=$m.SelectSingleNode(\"//*[local-name()='Properties']/*[local-name()='Logo']\");if($n){$logo=$n.InnerText}};if($logo){[pscustomobject]@{id=[string]$pkg.PackageFullName;folder=[string]$pkg.InstallLocation;logo=[string]$logo}}}catch{}});ConvertTo-Json -InputObject $items -Compress"#;
    // Raw Rust strings require literal quotes in the embedded PowerShell XPath.
    let script = script.replace("\\\"", "\"");
    let Ok(output) = backend::run_powershell(&script) else {
        return HashMap::new();
    };
    let Ok(records) = serde_json::from_str::<Vec<StoreLogo>>(&output) else {
        return HashMap::new();
    };
    records
        .into_iter()
        .take(1000)
        .filter_map(|record| {
            logo_path(Path::new(&record.folder), &record.logo)
                .map(|path| (format!("store:{}", record.id), path))
        })
        .collect()
}

fn logo_path(folder: &Path, logo: &str) -> Option<PathBuf> {
    if !icon_pixels::is_local_path(folder) {
        return None;
    }
    let logo = PathBuf::from(logo.replace('/', "\\"));
    if logo.as_os_str().is_empty()
        || logo
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let base = folder.join(logo);
    if !base
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return None;
    }
    let stem = base.file_stem()?.to_string_lossy().to_lowercase();
    let mut choices = Vec::new();
    if base.is_file() {
        choices.push((50, base.clone()));
    }
    for entry in std::fs::read_dir(base.parent()?).ok()?.take(256).flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            continue;
        }
        let name = path.file_stem()?.to_string_lossy().to_lowercase();
        if !name.starts_with(&format!("{stem}.")) || name.contains("contrast-") {
            continue;
        }
        let score = if name.contains("targetsize-64") {
            100
        } else if name.contains("targetsize-48") {
            90
        } else if name.contains("targetsize-96") {
            85
        } else if name.contains("targetsize-256") {
            75
        } else if name.contains("scale-200") {
            65
        } else {
            20
        };
        choices.push((score, path));
    }
    choices.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    choices
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| icon_pixels::is_local_path(path))
}

pub fn start(apps: Vec<AppRecord>, tx: Sender<Vec<LoadedIcon>>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let mut cache: HashMap<(PathBuf, i32), Option<ColorImage>> = HashMap::new();
        let mut batch = Vec::new();
        let mut emit = |id: String, path: PathBuf, index: i32| {
            let image = cache
                .entry((path.clone(), index))
                .or_insert_with(|| icon_pixels::load(&path, index))
                .clone();
            let found = image.is_some();
            if let Some(image) = image {
                batch.push(LoadedIcon { id, image });
            }
            if batch.len() >= 8 {
                let _ = tx.send(std::mem::take(&mut batch));
                ctx.request_repaint();
            }
            found
        };
        for app in apps
            .iter()
            .filter(|a| !a.id.starts_with("store:"))
            .take(1500)
        {
            if let Some((path, index)) = registered_icon(&app.id) {
                if emit(app.id.clone(), path, index) {
                    continue;
                }
            }
            if let Some(path) = executable_in_folder(app) {
                emit(app.id.clone(), path, 0);
            }
        }
        if apps.iter().any(|a| a.id.starts_with("store:")) {
            let logos = store_logos();
            for app in apps
                .iter()
                .filter(|a| a.id.starts_with("store:"))
                .take(1000)
            {
                if let Some(path) = logos.get(&app.id) {
                    emit(app.id.clone(), path.clone(), 0);
                }
            }
        }
        if !batch.is_empty() {
            let _ = tx.send(batch);
            ctx.request_repaint();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_icon_parses_quotes_commas_and_resource_ids_without_execution() {
        assert_eq!(
            display_icon(r#""C:\Program Files\Example\app.exe",-42"#),
            (PathBuf::from(r"C:\Program Files\Example\app.exe"), -42)
        );
        assert_eq!(
            display_icon(r"C:\Apps\comma, name.exe"),
            (PathBuf::from(r"C:\Apps\comma, name.exe"), 0)
        );
        assert_eq!(
            display_icon(r"C:\Apps\app.ico,0"),
            (PathBuf::from(r"C:\Apps\app.ico"), 0)
        );
    }
    #[test]
    fn logo_paths_cannot_escape_package_or_request_network_files() {
        assert!(logo_path(Path::new(r"C:\Windows"), r"..\outside.png").is_none());
        assert!(logo_path(Path::new(r"C:\Windows"), r"\\server\share\logo.png").is_none());
        assert!(logo_path(Path::new(r"\\server\share"), "logo.png").is_none());
    }
}
