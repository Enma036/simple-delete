//! End-to-end verification against only a newly created fixture directory.
//! Reads installed-app identities but never launches uninstallers or edits registry keys.
use simple_delete::backend;
use std::{
    env, fs, io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

fn require(condition: bool, message: &str) -> Result<(), io::Error> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local = fs::canonicalize(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is missing")?)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let name = format!(
        "SimpleDeleteCleanerVerification{}{}",
        std::process::id(),
        stamp
    );
    let fixture = local.join(&name);
    require(
        fixture.parent() == Some(local.as_path()),
        "Fixture escaped test root",
    )?;
    // create_dir deliberately fails if this location already exists.
    fs::create_dir(&fixture)?;
    fs::create_dir(fixture.join("Nested"))?;
    let payload = [0u8, 255, 10, 13, 42, 0, 128];
    fs::write(fixture.join("Nested/payload.bin"), payload)?;
    let settings = local.join(format!("{name}.json"));
    let mut settings_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&settings)?;
    std::io::Write::write_all(&mut settings_file, b"{\"fixture\":true}")?;
    drop(settings_file);

    let plan = backend::scan_leftovers(&name, None)?;
    require(
        !plan.active,
        "Unexpected matching installed app; refusing cleanup",
    )?;
    require(
        plan.candidates.len() == 2,
        "Unexpected scan candidates; refusing cleanup",
    )?;
    let candidate = plan
        .candidates
        .iter()
        .find(|c| c.kind == "directory")
        .ok_or("Missing directory fixture")?;
    let settings_candidate = plan
        .candidates
        .iter()
        .find(|c| c.kind == "file")
        .ok_or("Missing settings fixture")?;
    require(
        fs::canonicalize(&settings_candidate.path)? == settings,
        "File candidate is not the test fixture",
    )?;
    require(
        candidate.kind == "directory" && !candidate.blocked,
        "Fixture not an eligible directory",
    )?;
    require(
        fs::canonicalize(&candidate.path)? == fixture,
        "Candidate is not the test fixture",
    )?;
    let results = backend::cleanup(
        &plan.id,
        &[candidate.id.clone(), settings_candidate.id.clone()],
        false,
    )?;
    require(
        results.len() == 2 && results.iter().all(|r| r.error.is_none()),
        "Fixture cleanup failed",
    )?;
    let id = results[0]
        .backup_id
        .as_deref()
        .ok_or("No backup identifier returned")?;
    require(
        id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'),
        "Invalid backup identifier",
    )?;
    require(!fixture.exists(), "Fixture was not quarantined")?;
    let quarantined = local.join(".simple-delete-quarantine").join(id);
    require(
        fs::read(quarantined.join("Nested/payload.bin"))? == payload,
        "Quarantined data changed",
    )?;
    let backups = backend::list_backups()?;
    require(
        backups
            .iter()
            .any(|b| b.id == id && b.status == "quarantined"),
        "Backup not listed in Recovery",
    )?;
    backend::restore_backup(id)?;
    require(
        fs::read(fixture.join("Nested/payload.bin"))? == payload,
        "Restored data changed",
    )?;
    require(
        !quarantined.exists(),
        "Quarantine still contains fixture after restore",
    )?;
    require(
        backend::restore_backup(id).is_err(),
        "Repeated restore unexpectedly succeeded",
    )?;
    let file_id = results[1]
        .backup_id
        .as_deref()
        .ok_or("Missing file backup identifier")?;
    require(
        file_id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'),
        "Invalid file backup identifier",
    )?;
    require(!settings.exists(), "Settings file was not quarantined")?;
    require(
        fs::read(local.join(".simple-delete-quarantine").join(file_id))? == b"{\"fixture\":true}",
        "Quarantined settings changed",
    )?;
    backend::restore_backup(file_id)?;
    require(
        fs::read(&settings)? == b"{\"fixture\":true}",
        "Restored settings changed",
    )?;

    // Remove only files created above, without recursive deletion or touching other backups.
    fs::remove_file(fixture.join("Nested/payload.bin"))?;
    fs::remove_dir(fixture.join("Nested"))?;
    fs::remove_dir(&fixture)?;
    fs::remove_file(&settings)?;
    let metadata = local
        .join("SimpleDelete/native-backups")
        .join(format!("{id}.json"));
    require(
        metadata.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new("native-backups")),
        "Unexpected metadata parent",
    )?;
    fs::remove_file(metadata)?;
    fs::remove_file(
        local
            .join("SimpleDelete/native-backups")
            .join(format!("{file_id}.json")),
    )?;
    // An existing quarantine with other backups is deliberately left alone.
    let _ = fs::remove_dir(local.join(".simple-delete-quarantine"));
    println!(
        "CLEANER_E2E_OK: real scan, quarantine, Recovery listing, byte-exact restore, repeat-restore refusal; own fixture removed. No installed applications or registry keys changed."
    );
    Ok(())
}
