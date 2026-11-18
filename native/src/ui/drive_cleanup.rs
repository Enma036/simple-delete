use super::{Action, MUTED, ResultMessage, SimpleDeleteApp, empty_state, primary, surface};
use crate::complete_cleanup::{self, DriveScan, FileKind, ScanMode, ScanProgress};
use crate::{backend, model::CleanupItem};
use eframe::egui::{self, Color32, RichText};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

pub(super) struct DriveCleanup {
    drives: Vec<u8>,
    drive: Option<u8>,
    older_days: u64,
    pub(super) progress: Option<Arc<ScanProgress>>,
    pub(super) scan: Option<DriveScan>,
    pub(super) selected: Vec<bool>,
    pub(super) results: Vec<CleanupItem>,
    pub(super) cleaning: bool,
    pub(super) automatic_summary: String,
}

impl Default for DriveCleanup {
    fn default() -> Self {
        Self {
            drives: Vec::new(),
            drive: None,
            older_days: 180,
            progress: None,
            scan: None,
            selected: Vec::new(),
            results: Vec::new(),
            cleaning: false,
            automatic_summary: String::new(),
        }
    }
}

impl DriveCleanup {
    pub(super) fn refresh_drives(&mut self) {
        self.drives = complete_cleanup::available_drives();
        if !self.drive.is_some_and(|drive| self.drives.contains(&drive)) {
            let system_drive = std::env::var("SystemDrive")
                .ok()
                .and_then(|drive| drive.bytes().next())
                .map(|drive| drive.to_ascii_uppercase());
            self.drive = system_drive
                .filter(|drive| self.drives.contains(drive))
                .or_else(|| self.drives.first().copied());
        }
    }

    pub(super) fn set_scan(&mut self, scan: DriveScan) {
        self.selected = scan
            .candidates
            .iter()
            .map(|candidate| candidate.kind == FileKind::Temporary)
            .collect();
        self.scan = Some(scan);
    }
}

impl SimpleDeleteApp {
    pub(super) fn drive_cleanup_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let cleanup = &mut self.drive_cleanup;
        let mut scan_mode = None;
        surface().show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 2.0).max(0.0));
            ui.add_enabled_ui(self.busy == 0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("cleanup-drive")
                        .selected_text(cleanup.drive.map_or_else(|| "No local drive".into(), drive_label))
                        .show_ui(ui, |ui| {
                            for &drive in &cleanup.drives {
                                ui.selectable_value(&mut cleanup.drive, Some(drive), drive_label(drive));
                            }
                        });
                    let available = cleanup.drive.is_some();
                    if ui.add_enabled(available, primary("Auto Clean")).clicked() {
                        scan_mode = Some(ScanMode::Automatic);
                    }
                    if ui.add_enabled(available, egui::Button::new("Scan for review")).clicked() {
                        scan_mode = Some(ScanMode::Review);
                    }
                    egui::ComboBox::from_id_salt("cleanup-days")
                        .selected_text(format!("Older files · {}+ days", cleanup.older_days))
                        .show_ui(ui, |ui| {
                            for days in complete_cleanup::REVIEW_DAYS {
                                ui.selectable_value(&mut cleanup.older_days, days, format!("{days}+ days"));
                            }
                        });
                });
            });
            ui.label(RichText::new(
                format!("Auto Clean scans the selected drive and moves up to {} files at least {} days old from your Windows Temp folder to Recovery. It moves only .tmp and .temp files.",
                    complete_cleanup::RESULT_LIMIT,
                    complete_cleanup::TEMP_DAYS,
                ),
            ).small().color(MUTED));
        });
        ui.add_space(10.0);

        if let (Some(mode), Some(drive)) = (scan_mode, cleanup.drive) {
            let older_days = cleanup.older_days;
            let progress = Arc::new(ScanProgress::default());
            cleanup.progress = Some(Arc::clone(&progress));
            cleanup.scan = None;
            cleanup.selected.clear();
            cleanup.results.clear();
            cleanup.automatic_summary.clear();
            self.notice.clear();
            self.job(ctx, move || {
                ResultMessage::DriveScan(
                    mode,
                    backend::scan_drive(drive, older_days, mode, &progress),
                )
            });
            return;
        }

        if let Some(progress) = &cleanup.progress {
            ui.horizontal(|ui| {
                ui.spinner();
                if progress.cancelled.load(Relaxed) {
                    ui.label("Cancelling scan…");
                } else {
                    ui.label(format!(
                        "Scanning… {} entries checked",
                        progress.inspected.load(Relaxed)
                    ));
                    if ui.button("Cancel scan").clicked() {
                        progress.cancelled.store(true, Relaxed);
                    }
                }
            });
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }
        if cleanup.cleaning {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Moving selected files to Recovery…");
            });
            return;
        }

        if !cleanup.automatic_summary.is_empty() {
            ui.label(
                RichText::new(&cleanup.automatic_summary)
                    .small()
                    .color(MUTED),
            );
        }

        if !cleanup.results.is_empty() {
            if ui
                .add_enabled(self.busy == 0, primary("Open Recovery"))
                .clicked()
            {
                self.page = "Recovery".into();
                self.refresh("Recovery", ctx);
                return;
            }
            ui.label(RichText::new("Last cleanup results").strong());
            egui::ScrollArea::vertical()
                .id_salt("drive-clean-results")
                .show(ui, |ui| {
                    for item in &cleanup.results {
                        ui.label(RichText::new(&item.path).strong());
                        match (&item.backup_id, &item.error) {
                            (Some(_), Some(error)) => {
                                ui.colored_label(
                                    Color32::from_rgb(161, 102, 34),
                                    format!("In Recovery with a prepared record. {error}"),
                                );
                            }
                            (Some(_), None) => {
                                ui.label(RichText::new("In Recovery").color(MUTED));
                            }
                            (None, Some(error)) => {
                                ui.colored_label(Color32::from_rgb(177, 62, 62), error);
                            }
                            (None, None) => {}
                        }
                        ui.separator();
                    }
                });
            return;
        }

        let Some(scan) = &cleanup.scan else {
            if !cleanup.automatic_summary.is_empty() {
                return;
            }
            if cleanup.drives.is_empty() {
                empty_state(
                    ui,
                    "No local drives available",
                    "Reopen Complete Clean Up to check for local drives.",
                );
            } else {
                empty_state(
                    ui,
                    "Review older files",
                    "Scan for review to choose older documents, media, archives, or installers individually.",
                );
            }
            return;
        };

        ui.label(
            RichText::new(format!(
                "{} · {} entries checked · {} protected entries skipped · {} unreadable entries",
                drive_label(scan.drive),
                scan.inspected,
                scan.protected,
                scan.unreadable,
            ))
            .small()
            .color(MUTED),
        );
        ui.label(
            RichText::new("Windows access dates can be incomplete.")
                .small()
                .color(MUTED),
        );
        if scan.omitted > 0 {
            ui.label(
                RichText::new(format!(
                    "Showing the largest {} files per group. {} additional matches were left out.",
                    complete_cleanup::RESULT_LIMIT,
                    scan.omitted,
                ))
                .small()
                .color(MUTED),
            );
        }
        if scan.candidates.is_empty() {
            empty_state(
                ui,
                "No matching files found",
                "Try a shorter age or scan another drive.",
            );
            return;
        }
        if cleanup.drive != Some(scan.drive) || cleanup.older_days != scan.older_days {
            ui.label(RichText::new("Scan again to apply the new drive or age.").color(MUTED));
        }
        let (selected, bytes) = scan
            .candidates
            .iter()
            .zip(&cleanup.selected)
            .filter(|(_, picked)| **picked)
            .fold((0, 0u64), |(count, bytes), (file, _)| {
                (count + 1, bytes.saturating_add(file.bytes))
            });
        ui.horizontal_wrapped(|ui| {
            let ready = selected > 0
                && self.busy == 0
                && cleanup.drive == Some(scan.drive)
                && cleanup.older_days == scan.older_days;
            if ui
                .add_enabled(ready, primary(&format!("Move {selected} to Recovery")))
                .clicked()
            {
                self.confirmation = Some(Action::CleanDrive);
            }
            ui.label(RichText::new(format!("{} selected", size_label(bytes))).color(MUTED));
            if ui
                .add_enabled(selected > 0, egui::Button::new("Clear selection"))
                .clicked()
            {
                cleanup.selected.fill(false);
            }
        });
        ui.label(
            RichText::new("Files in Recovery still take up space on this drive.")
                .small()
                .color(MUTED),
        );
        egui::ScrollArea::vertical()
            .id_salt("drive-clean-candidates")
            .show_rows(ui, 40.0, scan.candidates.len(), |ui, range| {
                for index in range {
                    let candidate = &scan.candidates[index];
                    let kind = match candidate.kind {
                        FileKind::Temporary => "Temporary file",
                        FileKind::Older => "Older file",
                    };
                    let path = candidate.path.to_string_lossy();
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut cleanup.selected[index], "")
                            .on_hover_text(path.as_ref());
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(path.as_ref()).strong()).truncate(),
                            )
                            .on_hover_text(path.as_ref());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!(
                                        "{kind} · newest timestamp {} days ago · {}",
                                        candidate.days,
                                        size_label(candidate.bytes),
                                    ))
                                    .small()
                                    .color(MUTED),
                                )
                                .truncate(),
                            );
                        });
                    });
                }
            });
    }
}

fn drive_label(drive: u8) -> String {
    format!("{}:\\", char::from(drive))
}

fn size_label(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}
