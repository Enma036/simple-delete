use serde::{Deserialize, Serialize};
pub type AppResult<T> = Result<T, String>;
#[derive(Clone, Debug)]
pub struct CleanupItem {
    pub path: String,
    pub backup_id: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppRecord {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub size_mb: f64,
    pub installed: String,
    pub location: String,
    pub command: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub path: String,
    pub kind: String,
    pub reason: String,
    pub blocked: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanPlan {
    pub id: String,
    pub name: String,
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
    pub active: bool,
    #[serde(default)]
    pub coverage: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub original: String,
    pub status: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StartupEntry {
    pub id: String,
    pub name: String,
    pub command: String,
    pub scope: String,
    pub editable: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoreApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub removable: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MonitorRecord {
    pub id: String,
    pub label: String,
    pub status: String,
    pub started_at: String,
    pub warnings: Vec<String>,
    pub report: String,
}
