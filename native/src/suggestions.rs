use crate::{
    catalog::{AppFacts, Catalog},
    model::{AppRecord, AppResult},
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct Suggestion {
    pub app: AppRecord,
    pub reasons: Vec<String>,
    pub score: u32,
}

// Strip only explicit numeric version suffixes. Edition/channel words stay distinct.
fn product_key(app: &AppRecord) -> Option<String> {
    if app.publisher.trim().is_empty() {
        return None;
    }
    let name = app
        .name
        .split_whitespace()
        .filter(|word| {
            let word = word.trim_matches(['(', ')']);
            let version = word.strip_prefix('v').unwrap_or(word);
            !matches!(
                word.to_ascii_lowercase().as_str(),
                "x64" | "x86" | "64-bit" | "32-bit"
            ) && !(version.chars().next().is_some_and(|c| c.is_ascii_digit())
                && version.chars().all(|c| c.is_ascii_digit() || c == '.'))
        })
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if name.len() < 4 {
        return None;
    }
    Some(format!("{}|{}", name, app.publisher.trim().to_lowercase()))
}

pub fn rank(catalog: &Catalog, kept: &HashSet<String>) -> Vec<Suggestion> {
    let mut products: HashMap<String, Vec<&AppRecord>> = HashMap::new();
    for app in &catalog.apps {
        if let Some(key) = product_key(app) {
            products.entry(key).or_default().push(app);
        }
    }
    let mut suggestions = Vec::new();
    for app in &catalog.apps {
        let fallback = crate::catalog::classify(app);
        let facts: &AppFacts = catalog.facts.get(&app.id).unwrap_or(&fallback);
        // Discovery alone is not permission, nor proof that we can uninstall the app.
        if facts.protected
            || kept.contains(&app.id)
            || (!crate::catalog::is_registration(&app.id) && !app.id.starts_with("store:"))
            || (crate::catalog::is_registration(&app.id) && app.command.is_empty())
        {
            continue;
        }
        let mut reasons = Vec::new();
        let mut score = 0;
        if app.size_mb.is_finite() && app.size_mb >= 2048.0 {
            reasons.push(format!("Reported size: {:.1} GB. Removing it may free space if you no longer need it; this is an estimate, not a measured reclaimable total.", app.size_mb / 1024.0));
            score += 20 + (app.size_mb / 1024.0).min(80.0) as u32;
        }
        if let Some(group) = product_key(app).and_then(|key| products.get(&key)) {
            let versions: HashSet<_> = group
                .iter()
                .filter_map(|a| (!a.version.trim().is_empty()).then_some(a.version.trim()))
                .collect();
            // Identical duplicate registrations are not evidence of two removable installations.
            if versions.len() > 1 && !app.version.trim().is_empty() {
                reasons.push(format!("{} distinct versions are registered for this product and publisher. Check whether your work still requires each version; side-by-side versions may be intentional.", versions.len()));
                score += 35;
            }
        }
        if !reasons.is_empty() {
            suggestions.push(Suggestion {
                app: app.clone(),
                reasons,
                score,
            });
        }
    }
    suggestions.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.app.size_mb.total_cmp(&a.app.size_mb))
            .then_with(|| a.app.id.cmp(&b.app.id))
    });
    suggestions
}

fn preference_path() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("SimpleDelete/kept-apps.json")
}

pub fn load_kept() -> AppResult<HashSet<String>> {
    match std::fs::read(preference_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Could not read saved Keep choices: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(e) => Err(format!("Could not read saved Keep choices: {e}")),
    }
}

pub fn save_kept(kept: &HashSet<String>) -> AppResult<()> {
    let path = preference_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(kept).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("Could not save Keep choices: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(id: &str, name: &str, version: &str, size: f64) -> AppRecord {
        AppRecord {
            id: format!(
                r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\{id}"
            ),
            name: name.into(),
            publisher: "Example Vendor".into(),
            version: version.into(),
            size_mb: size,
            command: r"C:\Apps\uninstall.exe".into(),
            ..Default::default()
        }
    }
    fn catalog(apps: Vec<AppRecord>) -> Catalog {
        let facts = apps
            .iter()
            .map(|a| (a.id.clone(), crate::catalog::classify(a)))
            .collect();
        Catalog {
            apps,
            facts,
            warnings: vec![],
        }
    }
    #[test]
    fn large_apps_are_conditional_and_keep_excludes_them() {
        let app = app("photo", "Photo Editor", "1", 5000.0);
        let data = catalog(vec![app.clone()]);
        let ranked = rank(&data, &HashSet::new());
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].reasons[0].contains("if you no longer need"));
        assert!(rank(&data, &HashSet::from([app.id])).is_empty());
    }
    #[test]
    fn shared_runtimes_drivers_security_and_unverified_folders_are_never_suggested() {
        let mut folder = app("folder", "Photo Editor", "1", 12000.0);
        folder.id = "discovered:folder".into();
        let data = catalog(vec![
            app(
                "runtime",
                "Microsoft Visual C++ 2022 Redistributable",
                "1",
                5000.0,
            ),
            app("driver", "NVIDIA Graphics Driver", "1", 5000.0),
            app("security", "Windows Defender", "1", 5000.0),
            folder,
        ]);
        assert!(rank(&data, &HashSet::new()).is_empty());
    }
    #[test]
    fn only_distinct_versions_of_same_publisher_are_flagged() {
        let mut third = app("third", "Photo Editor 3", "3", 20.0);
        third.publisher = "Other Company".into();
        let data = catalog(vec![
            app("first", "Photo Editor 1", "1", 20.0),
            app("second", "Photo Editor 2", "2", 20.0),
            third,
        ]);
        assert_eq!(rank(&data, &HashSet::new()).len(), 2);
        let data = catalog(vec![
            app("first", "Photo Editor", "1", 20.0),
            app("second", "Photo Editor", "1", 20.0),
        ]);
        assert!(rank(&data, &HashSet::new()).is_empty());
    }
}
