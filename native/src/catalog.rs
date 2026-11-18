//! Read-only application discovery. No collector invents an uninstall command.
use crate::{
    backend,
    model::{AppRecord, AppResult},
};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::Read,
    os::windows::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY},
};

const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_LIBRARIES: usize = 16;
const MAX_MANIFESTS: usize = 1000;
const REPARSE: u32 = 0x400;

#[derive(Clone, Debug, Default)]
pub struct AppFacts {
    pub source: String,
    pub scope: String,
    pub category: String,
    pub protected: bool,
    pub description: String,
}
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub apps: Vec<AppRecord>,
    pub facts: HashMap<String, AppFacts>,
    pub warnings: Vec<String>,
}

pub fn is_registration(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    (id.starts_with("hkey_local_machine\\") || id.starts_with("hkey_current_user\\"))
        && (id.contains("\\software\\microsoft\\windows\\currentversion\\uninstall\\")
            || id.contains(
                "\\software\\wow6432node\\microsoft\\windows\\currentversion\\uninstall\\",
            ))
}

/// Labels are conservative local heuristics, not a malware verdict or usage measurement.
pub fn classify(app: &AppRecord) -> AppFacts {
    let name = app.name.to_lowercase();
    let publisher = app.publisher.to_lowercase();
    let words: Vec<_> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let contains = |terms: &[&str]| {
        terms.iter().any(|term| {
            if matches!(
                *term,
                "git"
                    | "rust"
                    | "java"
                    | "jdk"
                    | "jre"
                    | "word"
                    | "opera"
                    | "signal"
                    | "anki"
                    | "eset"
                    | "vpn"
            ) {
                words.contains(term)
            } else {
                name.contains(term)
            }
        })
    };
    let (category, protected, description) = if contains(&[
        "driver",
        "chipset",
        "firmware",
        "realtek",
        "nvidia graphics",
        "amd software",
        "intel(r)",
        "intel®",
        "displaylink",
        "vulkan",
        "physx",
        "directx",
    ]) {
        (
            "Drivers & hardware",
            true,
            "Hardware support software. Removing it can affect devices, graphics, audio or connectivity.",
        )
    } else if contains(&[
        "redistributable",
        "visual c++",
        "visual c +",
        ".net",
        "dotnet",
        "runtime",
        "framework",
        "webview",
        "windows sdk",
        "windows desktop targeting",
        "microsoft edge update",
        "microsoft update",
        "windows update",
        "windows app",
        "vclibs",
        "ui.xaml",
        "directml",
        "opengl",
        "opencl",
        "java",
        "jdk",
        "jre",
        "python launcher",
    ]) {
        (
            "Shared components",
            true,
            "A runtime, framework or shared dependency. Other applications may require this exact version.",
        )
    } else if contains(&[
        "antivirus",
        "security",
        "defender",
        "firewall",
        "malwarebytes",
        "bitdefender",
        "kaspersky",
        "eset",
        "avast",
        "avira",
        "crowdstrike",
        "sentinel",
        "sophos",
        "norton",
        "mcafee",
        "vpn",
        "wireguard",
        "openvpn",
    ]) {
        (
            "Security & networking",
            true,
            "Security or network software. Review its purpose and any administrator requirements before changing it.",
        )
    } else if contains(&[
        "windows shell",
        "windows client",
        "windows experience",
        "startmenuexperience",
        "shellexperience",
        "windows search",
        "windows logon",
        "windows print",
        "windows feature",
        "windows servicing",
        "windows language",
        "windows terminal",
        "microsoft edge",
    ]) {
        (
            "Windows components",
            true,
            "A Windows or browser component. Treat it as a dependency rather than a cleanup target.",
        )
    } else if app.id.starts_with("steam:")
        || app.id.to_ascii_lowercase().contains("\\steam app ")
        || contains(&[
            "steam",
            "epic games",
            "gog galaxy",
            "ubisoft connect",
            "battle.net",
            "riot client",
            "xbox",
        ])
    {
        (
            "Games & launchers",
            false,
            "A game or game launcher. Review installed content in its launcher before removing it.",
        )
    } else if contains(&[
        "visual studio",
        "vscode",
        "visual studio code",
        "git",
        "github",
        "gitextensions",
        "node.js",
        "python",
        "rust",
        "docker",
        "android studio",
        "jetbrains",
        "intellij",
        "pycharm",
        "postman",
        "powershell",
    ]) {
        (
            "Developer tools",
            false,
            "Development software. Projects and workflows may depend on it.",
        )
    } else if contains(&[
        "photoshop",
        "illustrator",
        "premiere",
        "davinci",
        "blender",
        "figma",
        "audacity",
        "obs studio",
        "lightroom",
        "after effects",
        "gimp",
    ]) {
        (
            "Creative tools",
            false,
            "Creative software. Check projects, plug-ins and saved work before removing it.",
        )
    } else if contains(&["chrome", "firefox", "brave", "opera", "vivaldi", "browser"]) {
        (
            "Browsers",
            false,
            "A web browser. Check bookmarks, profiles and any default-browser settings before removing it.",
        )
    } else if contains(&[
        "office",
        "word",
        "excel",
        "powerpoint",
        "onenote",
        "outlook",
        "libreoffice",
        "notion",
        "acrobat",
        "anki",
    ]) {
        (
            "Productivity",
            false,
            "Productivity software. Check documents and account data before removing it.",
        )
    } else if contains(&[
        "discord", "slack", "teams", "zoom", "telegram", "whatsapp", "signal", "skype",
    ]) {
        (
            "Communication",
            false,
            "Communication software. It may be required for work or personal contacts.",
        )
    } else {
        (
            "Other apps",
            false,
            "Detected from local installation evidence. Category and usage are not verified.",
        )
    };
    let (source, scope) = if app.id.starts_with("store:") {
        ("Microsoft Store", "Current user")
    } else if app.id.starts_with("steam:")
        || app
            .id
            .to_ascii_lowercase()
            .rsplit('\\')
            .next()
            .is_some_and(|tail| {
                tail.strip_prefix("steam app ")
                    .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            })
    {
        (
            "Steam library",
            if app.id.starts_with("steam:") {
                "Game library"
            } else if app.id.to_ascii_lowercase().starts_with("hkey_current_user") {
                "Current user"
            } else {
                "All users"
            },
        )
    } else if app.id.starts_with("discovered:") {
        ("App folder", "Current user")
    } else if app.id.to_ascii_lowercase().starts_with("hkey_current_user") {
        (
            "Desktop registry",
            if app.id.to_ascii_lowercase().contains("wow6432node") {
                "Current user · 32-bit"
            } else {
                "Current user"
            },
        )
    } else {
        (
            "Desktop registry",
            if app.id.to_ascii_lowercase().contains("wow6432node") {
                "All users · 32-bit"
            } else {
                "All users · 64-bit"
            },
        )
    };
    let mut facts = AppFacts {
        source: source.into(),
        scope: scope.into(),
        category: category.into(),
        protected,
        description: description.into(),
    };
    if category == "Other apps"
        && (publisher.contains("intel")
            || publisher.contains("realtek")
            || publisher.contains("nvidia")
            || publisher.contains("advanced micro devices"))
    {
        facts.category = "Drivers & hardware".into();
        facts.protected = true;
        facts.description="Hardware-vendor software. Keep it unless you have verified the purpose and replacement.".into();
    }
    // Package removability exists only in collected Store facts. Missing facts must fail closed.
    if app.id.starts_with("store:") {
        facts.protected = true;
        facts.description.push_str(&format!(
            " Package identity: {}. Removability requires live Store metadata.",
            app.id.trim_start_matches("store:")
        ));
    }
    facts
}

pub fn load() -> AppResult<Catalog> {
    let mut out = Catalog::default();
    registry_coverage(&mut out.warnings);
    match backend::inventory() {
        Ok(apps) => {
            for app in apps {
                insert(&mut out, app, None);
            }
        }
        Err(error) => out
            .warnings
            .push(format!("Desktop inventory could not be read: {error}")),
    }
    match backend::store_apps() {
        Ok(apps) => {
            for app in apps {
                let record = AppRecord {
                    id: format!("store:{}", app.id),
                    name: store_name(&app.name),
                    publisher: publisher_label(&app.publisher),
                    version: app.version,
                    ..Default::default()
                };
                let mut facts = classify(&record);
                // Replace the conservative fallback evidence with this live Store result.
                facts.description = facts
                    .description
                    .split(" Package identity:")
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if app.removable {
                    facts.protected = matches!(
                        facts.category.as_str(),
                        "Drivers & hardware"
                            | "Shared components"
                            | "Security & networking"
                            | "Windows components"
                    );
                }
                if !app.removable {
                    facts.protected = true;
                    facts.description="Windows marks this package as a non-removable, framework or resource package. It is excluded from removal suggestions.".into();
                    facts.category = "Windows components".into();
                }
                facts.description.push_str(&format!(
                    " Package identity: {}.",
                    record.id.trim_start_matches("store:")
                ));
                insert(&mut out, record, Some(facts));
            }
        }
        Err(error) => out
            .warnings
            .push(format!("Microsoft Store inventory is incomplete: {error}")),
    }
    collect_steam(&mut out);
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        collect_app_folders(&PathBuf::from(local).join("Programs"), &mut out);
    }
    out.apps.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    out.warnings.sort();
    out.warnings.dedup();
    Ok(out)
}

fn registry_coverage(warnings: &mut Vec<String>) {
    for (handle, path, flags) in [
        (
            HKEY_LOCAL_MACHINE,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_READ | KEY_WOW64_64KEY,
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_READ,
        ),
        (
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_READ,
        ),
        (
            HKEY_CURRENT_USER,
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            KEY_READ,
        ),
    ] {
        if let Err(error) = RegKey::predef(handle).open_subkey_with_flags(path, flags) {
            if error.kind() != std::io::ErrorKind::NotFound {
                warnings.push(format!(
                    "An uninstall-registry branch could not be read ({path}): {error}"
                ));
            }
        }
    }
}

fn path_key(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}
fn name_key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn same_evidence(existing: &AppRecord, new: &AppRecord) -> bool {
    if existing.id.eq_ignore_ascii_case(&new.id) {
        return true;
    }
    // Distinct registrations and Store packages retain their independent identities.
    if is_registration(&new.id) || new.id.starts_with("store:") {
        return false;
    }
    if let Some(appid) = new.id.strip_prefix("steam:") {
        if existing
            .id
            .to_ascii_lowercase()
            .ends_with(&format!("\\steam app {appid}"))
        {
            return true;
        }
    }
    let names_match = name_key(&existing.name) == name_key(&new.name);
    let location_match = !existing.location.is_empty()
        && !new.location.is_empty()
        && path_key(&existing.location) == path_key(&new.location);
    if location_match && names_match {
        return true;
    }
    // An executable directly inside a matching registered install folder proves folder overlap.
    new.id.starts_with("discovered:") && location_match
}
fn insert(out: &mut Catalog, app: AppRecord, facts: Option<AppFacts>) {
    if let Some(index) = out.apps.iter().position(|a| same_evidence(a, &app)) {
        let existing = &mut out.apps[index];
        if existing.size_mb <= 0.0 && app.size_mb > 0.0 {
            existing.size_mb = app.size_mb;
        }
        if existing.location.is_empty() && !app.location.is_empty() {
            existing.location = app.location;
        }
        return;
    }
    out.facts
        .insert(app.id.clone(), facts.unwrap_or_else(|| classify(&app)));
    out.apps.push(app);
}
fn store_name(raw: &str) -> String {
    // Start-menu display names already contain spaces and should be preserved.
    if raw.contains(' ') {
        return raw.to_owned();
    }
    let trimmed = raw
        .strip_prefix("Microsoft.")
        .or_else(|| raw.strip_prefix("MicrosoftCorporationII."))
        .unwrap_or(raw);
    let mut text = String::new();
    let mut prev_lower = false;
    for ch in trimmed.chars() {
        if ch == '.' || ch == '_' {
            text.push(' ');
            prev_lower = false;
        } else {
            if ch.is_uppercase() && prev_lower {
                text.push(' ');
            }
            text.push(ch);
            prev_lower = ch.is_lowercase();
        }
    }
    text
}

fn publisher_label(raw: &str) -> String {
    let name = raw
        .strip_prefix("CN=")
        .unwrap_or(raw)
        .split(',')
        .next()
        .unwrap_or("")
        .trim();
    // Certificate GUIDs aren't useful publisher names. The package identity stays in details.
    if name.len() == 36 && name.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        String::new()
    } else {
        name.to_owned()
    }
}
fn regular_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        m.is_dir() && !m.file_type().is_symlink() && m.file_attributes() & REPARSE == 0
    })
}
fn bounded_read(path: &Path) -> AppResult<String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.file_attributes() & REPARSE != 0
        || meta.len() > MAX_FILE
    {
        return Err("not a regular bounded metadata file".into());
    }
    let mut text = String::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_FILE + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_FILE {
        return Err("metadata file exceeded the byte limit".into());
    }
    Ok(text)
}

#[derive(Debug, Clone)]
enum Vdf {
    Text(String),
    Object(Vec<(String, Vdf)>),
}
fn tokens(text: &str) -> AppResult<Vec<String>> {
    let mut result = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if result.len() >= 65_536 {
            return Err("metadata token limit reached".into());
        }
        match ch {
            c if c.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '{' | '}' => result.push(ch.to_string()),
            '"' => {
                let mut s = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    if c == '\\' {
                        match chars.peek() {
                            Some('"') | Some('\\') => s.push(chars.next().unwrap()),
                            _ => s.push(c),
                        }
                    } else {
                        s.push(c)
                    }
                }
                if !closed {
                    return Err("unterminated quoted metadata".into());
                }
                result.push(s);
            }
            _ => {
                let mut s = ch.to_string();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '{' || c == '}' {
                        break;
                    }
                    s.push(chars.next().unwrap());
                }
                result.push(s);
            }
        }
    }
    Ok(result)
}
fn parse_vdf(text: &str) -> AppResult<Vec<(String, Vdf)>> {
    fn pairs(
        tokens: &[String],
        at: &mut usize,
        depth: usize,
        nested: bool,
    ) -> AppResult<Vec<(String, Vdf)>> {
        if depth > 16 {
            return Err("metadata nesting limit reached".into());
        }
        let mut result = Vec::new();
        while *at < tokens.len() {
            if tokens[*at] == "}" {
                if !nested {
                    return Err("unexpected closing brace".into());
                }
                *at += 1;
                return Ok(result);
            }
            let key = tokens[*at].clone();
            *at += 1;
            if key == "{" || *at >= tokens.len() {
                return Err("invalid metadata key/value pair".into());
            }
            let value = if tokens[*at] == "{" {
                *at += 1;
                Vdf::Object(pairs(tokens, at, depth + 1, true)?)
            } else if tokens[*at] == "}" {
                return Err("missing metadata value".into());
            } else {
                let value = Vdf::Text(tokens[*at].clone());
                *at += 1;
                value
            };
            result.push((key, value));
        }
        if nested {
            return Err("unclosed metadata object".into());
        }
        Ok(result)
    }
    let tokens = tokens(text)?;
    pairs(&tokens, &mut 0, 0, false)
}
fn value<'a>(pairs: &'a [(String, Vdf)], key: &str) -> Option<&'a str> {
    pairs.iter().find_map(|(k, v)| {
        if k.eq_ignore_ascii_case(key) {
            if let Vdf::Text(s) = v {
                Some(s.as_str())
            } else {
                None
            }
        } else {
            None
        }
    })
}
fn manifest(text: &str, steamapps: &Path) -> AppResult<AppRecord> {
    let root = parse_vdf(text)?;
    let state = root
        .iter()
        .find_map(|(k, v)| {
            if k.eq_ignore_ascii_case("AppState") {
                if let Vdf::Object(o) = v {
                    Some(o)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .ok_or("missing AppState")?;
    let appid = value(state, "appid").ok_or("missing app id")?;
    if appid.is_empty() || !appid.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid Steam app id".into());
    }
    let flags = value(state, "StateFlags")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    if flags & 4 == 0 {
        return Err("game is not fully installed".into());
    }
    let name = value(state, "name")
        .filter(|s| !s.trim().is_empty())
        .ok_or("missing game name")?;
    let folder = value(state, "installdir").ok_or("missing install directory")?;
    let components = Path::new(folder).components().collect::<Vec<_>>();
    if components.len() != 1
        || !matches!(components[0], Component::Normal(_))
        || folder.contains(['/', '\\'])
    {
        return Err("invalid Steam install directory".into());
    }
    let location = steamapps.join("common").join(folder);
    if !regular_directory(&location) {
        return Err("installed game directory is missing or a reparse point".into());
    }
    Ok(AppRecord {
        id: format!("steam:{appid}"),
        name: name.into(),
        publisher: "Steam library".into(),
        version: value(state, "buildid")
            .map(|s| format!("Build {s}"))
            .unwrap_or_default(),
        size_mb: value(state, "SizeOnDisk")
            .and_then(|s| s.parse::<u64>().ok())
            .map(|n| n as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0),
        location: location.to_string_lossy().into_owned(),
        ..Default::default()
    })
}
fn library_paths(text: &str) -> AppResult<Vec<PathBuf>> {
    fn walk(pairs: &[(String, Vdf)], out: &mut Vec<PathBuf>) {
        for (k, v) in pairs {
            match v {
                Vdf::Text(s)
                    if k.eq_ignore_ascii_case("path")
                        || (k.bytes().all(|b| b.is_ascii_digit())
                            && !k.is_empty()
                            && (s.contains('\\') || s.contains('/'))) =>
                {
                    out.push(PathBuf::from(s))
                }
                Vdf::Object(o) => walk(o, out),
                _ => {}
            }
        }
    }
    let mut paths = Vec::new();
    walk(&parse_vdf(text)?, &mut paths);
    Ok(paths)
}
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for (hive, path, flags) in [
        (HKEY_CURRENT_USER, r"Software\Valve\Steam", KEY_READ),
        (
            HKEY_LOCAL_MACHINE,
            r"Software\Valve\Steam",
            KEY_READ | KEY_WOW64_64KEY,
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"Software\WOW6432Node\Valve\Steam",
            KEY_READ,
        ),
    ] {
        if let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(path, flags) {
            for name in ["SteamPath", "InstallPath"] {
                if let Ok(value) = key.get_value::<String, _>(name) {
                    if !value.is_empty() {
                        roots.push(PathBuf::from(value));
                    }
                }
            }
        }
    }
    if let Some(programs) = env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(programs).join("Steam"));
    }
    roots
}
fn collect_steam(out: &mut Catalog) {
    let roots = steam_roots();
    let mut libraries = roots.clone();
    for root in &roots {
        let file = root.join("steamapps/libraryfolders.vdf");
        if file.exists() {
            match bounded_read(&file).and_then(|text| library_paths(&text)) {
                Ok(paths) => libraries.extend(paths),
                Err(error) => out.warnings.push(format!(
                    "Steam library locations could not be read: {error}"
                )),
            }
        }
    }
    collect_steam_libraries(libraries, out);
}
fn collect_steam_libraries(libraries: Vec<PathBuf>, out: &mut Catalog) {
    let start = Instant::now();
    let mut seen = HashSet::new();
    let mut count = 0;
    for library in libraries {
        if !seen.insert(path_key(&library.to_string_lossy())) {
            continue;
        }
        if seen.len() > MAX_LIBRARIES {
            out.warnings
                .push("Steam discovery reached the 16-library limit.".into());
            break;
        }
        let steamapps = library.join("steamapps");
        if !steamapps.exists() {
            continue;
        }
        if !regular_directory(&steamapps) {
            out.warnings.push(format!(
                "Steam library skipped because it is not a regular directory: {}",
                steamapps.display()
            ));
            continue;
        }
        let entries = match fs::read_dir(&steamapps) {
            Ok(v) => v,
            Err(error) => {
                out.warnings
                    .push(format!("Steam library could not be read: {error}"));
                continue;
            }
        };
        for entry in entries {
            if count >= MAX_MANIFESTS || start.elapsed() > Duration::from_secs(3) {
                out.warnings.push(
                    "Steam discovery reached its file/time budget; coverage may be partial.".into(),
                );
                return;
            }
            let Ok(entry) = entry else {
                out.warnings
                    .push("Some Steam library entries could not be read.".into());
                continue;
            };
            let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !filename.starts_with("appmanifest_") || !filename.ends_with(".acf") {
                continue;
            }
            count += 1;
            match bounded_read(&entry.path()).and_then(|text| manifest(&text, &steamapps)) {
                Ok(app) => {
                    let mut facts = classify(&app);
                    facts.description="An installed game found in a Steam manifest. Manage it in Steam; no uninstall command is assumed.".into();
                    insert(out, app, Some(facts));
                }
                Err(error) => {
                    if !error.contains("not fully installed") {
                        out.warnings.push(format!(
                            "A Steam manifest was skipped ({filename}): {error}"
                        ));
                    }
                }
            }
        }
    }
}
fn collect_app_folders(root: &Path, out: &mut Catalog) {
    if !root.exists() {
        return;
    }
    if !regular_directory(root) {
        out.warnings
            .push("Per-user app-folder discovery skipped a reparse or inaccessible root.".into());
        return;
    }
    let start = Instant::now();
    let entries = match fs::read_dir(root) {
        Ok(v) => v,
        Err(error) => {
            out.warnings
                .push(format!("Per-user app folders could not be read: {error}"));
            return;
        }
    };
    for (index, entry) in entries.enumerate() {
        if index >= 128 || start.elapsed() > Duration::from_secs(2) {
            out.warnings
                .push("Per-user app-folder discovery reached its directory/time budget.".into());
            break;
        }
        let Ok(entry) = entry else {
            out.warnings
                .push("A per-user app folder could not be read.".into());
            continue;
        };
        let folder = entry.path();
        if !regular_directory(&folder) {
            continue;
        }
        let mut found = false;
        let mut truncated = false;
        if let Ok(files) = fs::read_dir(&folder) {
            for (count, file) in files.enumerate() {
                if count >= 128 {
                    truncated = true;
                    break;
                }
                let Ok(file) = file else {
                    continue;
                };
                let p = file.path();
                let is_exe = p.extension().is_some_and(|s| s.eq_ignore_ascii_case("exe"));
                let name = file.file_name().to_string_lossy().to_ascii_lowercase();
                if is_exe
                    && !name.contains("unins")
                    && !name.contains("update")
                    && fs::symlink_metadata(&p).is_ok_and(|m| {
                        m.is_file()
                            && !m.file_type().is_symlink()
                            && m.file_attributes() & REPARSE == 0
                    })
                {
                    found = true;
                    break;
                }
            }
        }
        if truncated {
            out.warnings.push(format!(
                "App-folder metadata scan was bounded: {}",
                folder.display()
            ));
        }
        if !found {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let location = folder.to_string_lossy().into_owned();
        let app = AppRecord {
            id: format!("discovered:{}", path_key(&location)),
            name,
            location,
            ..Default::default()
        };
        let mut facts = classify(&app);
        facts.description="An executable was found in this per-user application folder without a matching registered install location. This is a discovery hint, not proof that the app is portable or safe to remove.".into();
        insert(out, app, Some(facts));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn friendly_store_names_and_publishers_stay_readable() {
        assert_eq!(store_name("Windows App SDK"), "Windows App SDK");
        assert_eq!(
            publisher_label("CN=Microsoft Corporation, O=Microsoft Corporation, C=US"),
            "Microsoft Corporation"
        );
        assert!(publisher_label("CN=38903382-DB59-47AA-81EA-F5FE85192885").is_empty());
    }
    #[test]
    fn ambiguous_substrings_do_not_assign_unrelated_product_categories() {
        for name in [
            "Logitech Options",
            "RustDesk",
            "JavaScript Editor",
            "Password Manager",
            "SignalRGB",
        ] {
            assert_eq!(
                classify(&AppRecord {
                    name: name.into(),
                    ..Default::default()
                })
                .category,
                "Other apps",
                "{name}"
            );
        }
        assert_eq!(
            classify(&AppRecord {
                name: "Git version 2".into(),
                ..Default::default()
            })
            .category,
            "Developer tools"
        );
        assert!(
            classify(&AppRecord {
                id: "store:WindowsContactSupport_1".into(),
                name: "Contact Support".into(),
                ..Default::default()
            })
            .protected
        );
        assert_eq!(classify(&AppRecord {id:r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\Steam App 123".into(),..Default::default()}).source,"Steam library");
    }
    #[test]
    fn protects_dependencies_drivers_and_security() {
        for name in [
            "Microsoft Visual C++ 2015-2022 Redistributable",
            "Microsoft .NET Runtime 8",
            "Realtek Audio Driver",
            "NVIDIA Graphics Driver",
            "WireGuard VPN",
            "Microsoft Edge",
            "Windows App Runtime",
        ] {
            assert!(
                classify(&AppRecord {
                    name: name.into(),
                    ..Default::default()
                })
                .protected,
                "{name}"
            );
        }
        assert!(
            !classify(&AppRecord {
                name: "Blender".into(),
                ..Default::default()
            })
            .protected
        );
    }
    #[test]
    fn library_parser_handles_escaped_paths_nested_and_legacy() {
        let paths=library_paths(r#""libraryfolders" { "0" { "path" "C:\\Steam" "apps" { "123" "1" } } "1" "D:\\Games" }"#).unwrap();
        assert_eq!(
            paths,
            vec![PathBuf::from(r"C:\Steam"), PathBuf::from(r"D:\Games")]
        );
        assert!(parse_vdf("\"x\" { \"a\" \"unterminated").is_err());
    }
    #[test]
    fn discovered_evidence_deduplicates_but_registrations_and_store_keep_identity() {
        let mut out = Catalog::default();
        let registry=AppRecord{id:r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\Steam App 123".into(),name:"A Game".into(),location:r"D:\Steam\steamapps\common\Game".into(),..Default::default()};
        insert(&mut out, registry.clone(), None);
        insert(
            &mut out,
            AppRecord {
                id: "steam:123".into(),
                size_mb: 2000.0,
                ..registry.clone()
            },
            None,
        );
        assert_eq!(out.apps.len(), 1);
        assert_eq!(out.apps[0].size_mb, 2000.0);
        insert(
            &mut out,
            AppRecord {
                id: r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\other"
                    .into(),
                ..registry.clone()
            },
            None,
        );
        insert(
            &mut out,
            AppRecord {
                id: "store:another".into(),
                ..registry
            },
            None,
        );
        assert_eq!(out.apps.len(), 3);
    }
    #[test]
    fn source_and_scope_are_explicit() {
        let facts=classify(&AppRecord{id:r"HKEY_CURRENT_USER\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Tool".into(),..Default::default()});
        assert_eq!(facts.scope, "Current user · 32-bit");
        assert!(is_registration(
            r"HKEY_CURRENT_USER\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Tool"
        ));
        assert!(!is_registration("store:thing"));
    }
    #[test]
    fn manifest_requires_installed_directory_and_keeps_no_uninstall_command() {
        let root = env::temp_dir().join(format!(
            "simple-delete-catalog-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("steamapps/common/Game")).unwrap();
        let text = r#""AppState" { "appid" "123" "name" "Example Game" "StateFlags" "4" "installdir" "Game" "SizeOnDisk" "2147483648" "buildid" "12" }"#;
        let app = manifest(text, &root.join("steamapps")).unwrap();
        assert_eq!(app.size_mb, 2048.0);
        assert!(app.command.is_empty());
        assert_eq!(app.id, "steam:123");
        assert!(
            manifest(
                &text.replace("\"Game\"", "\"../outside\""),
                &root.join("steamapps")
            )
            .is_err()
        );
        assert!(manifest(&text.replace("\"4\"", "\"2\""), &root.join("steamapps")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
