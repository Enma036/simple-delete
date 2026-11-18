use crate::complete_cleanup::{self, DriveScan, ScanMode};
use crate::{app_icons, backend, catalog, icons, model::*, suggestions};
use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{self, Receiver, Sender};

mod drive_cleanup;
use drive_cleanup::DriveCleanup;

const APP_ROW_HEIGHT: f32 = 76.0;

struct AppTexture {
    handle: egui::TextureHandle,
    dark_backdrop: bool,
}

fn uninstall_then_scan(
    uninstall: impl FnOnce() -> AppResult<String>,
    inventory: impl FnOnce() -> AppResult<Vec<AppRecord>>,
    scan: impl FnOnce() -> AppResult<ScanPlan>,
) -> AppResult<(Vec<AppRecord>, ScanPlan)> {
    uninstall()?;
    let apps = inventory()?;
    Ok((apps, scan()?))
}

const INK: Color32 = Color32::from_rgb(27, 39, 59);
const MUTED: Color32 = Color32::from_rgb(113, 126, 145);
const ACCENT: Color32 = Color32::from_rgb(31, 99, 225);
const LINE: Color32 = Color32::from_rgb(228, 233, 241);
const CANVAS: Color32 = Color32::from_rgb(245, 247, 251);

fn surface() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(egui::Stroke::new(1.0, LINE))
        .corner_radius(12)
        .inner_margin(18)
}

fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE))
        .fill(ACCENT)
        .corner_radius(7)
}

fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    surface().show(ui, |ui| {
        ui.set_min_width((ui.available_width() - 2.0).max(0.0));
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(title).size(19.0).strong().color(INK));
            ui.label(RichText::new(detail).color(MUTED));
        });
        ui.add_space(24.0);
    });
}

fn resource_row(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    tag: &str,
    action: &str,
    enabled: bool,
    menu: bool,
    registration_allowed: bool,
    icon: icons::Icon,
    texture: Option<&AppTexture>,
) -> (bool, bool, bool, bool) {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, APP_ROW_HEIGHT), egui::Sense::hover());
    // Absolute-positioned controls use a child so their scopes cannot move the list cursor.
    let mut row_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let ui = &mut row_ui;
    let fill = if response.hovered() {
        Color32::from_rgb(249, 251, 255)
    } else {
        Color32::WHITE
    };
    ui.painter().rect(
        rect.shrink(0.5),
        10,
        fill,
        egui::Stroke::new(1.0, LINE),
        egui::StrokeKind::Inside,
    );
    let avatar =
        egui::Rect::from_min_size(rect.min + egui::vec2(16.0, 17.0), egui::vec2(42.0, 42.0));
    identity_at(ui, avatar, texture, icon);
    let controls_width = if menu { 146.0 } else { 118.0 };
    let text_right = rect.right() - controls_width - if width > 760.0 { 125.0 } else { 18.0 };
    let text_x = rect.left() + 72.0;
    for (text, y, size, color, strong) in [
        (title, 14.0, 15.0, INK, true),
        (subtitle, 39.0, 12.0, MUTED, false),
    ] {
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(text_x, rect.top() + y),
            egui::pos2(text_right, rect.top() + y + 23.0),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(text_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                let mut rich = RichText::new(text).size(size).color(color);
                if strong {
                    rich = rich.strong();
                }
                ui.add(egui::Label::new(rich).truncate())
                    .on_hover_text(text);
            },
        );
    }
    if width > 760.0 {
        ui.painter().text(
            egui::pos2(rect.right() - controls_width - 32.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            tag,
            egui::FontId::proportional(13.0),
            MUTED,
        );
    }
    let action_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - controls_width, rect.top() + 21.0),
        egui::vec2(96.0, 34.0),
    );
    let clicked = ui
        .scope_builder(egui::UiBuilder::new().max_rect(action_rect), |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                ui.put(
                    action_rect,
                    egui::Button::new(RichText::new(action).color(if enabled {
                        ACCENT
                    } else {
                        MUTED
                    }))
                    .fill(Color32::from_rgb(237, 243, 255))
                    .corner_radius(7),
                )
            })
            .inner
            .clicked()
        })
        .inner;
    let mut scan = false;
    let mut registration = false;
    let mut details = false;
    if menu {
        let menu_rect = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 40.0, rect.top() + 21.0),
            egui::vec2(30.0, 34.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(menu_rect), |ui| {
            ui.spacing_mut().button_padding = egui::vec2(6.0, 7.0);
            ui.menu_button("…", |ui| {
                if ui.button("App details").clicked() {
                    details = true;
                    ui.close();
                }
                if ui.button("Scan leftovers").clicked() {
                    scan = true;
                    ui.close();
                }
                ui.separator();
                if registration_allowed
                    && ui
                        .button(
                            RichText::new("Remove registration")
                                .color(Color32::from_rgb(186, 62, 72)),
                        )
                        .on_hover_text("Back up and remove a stale entry; installed files remain")
                        .clicked()
                {
                    registration = true;
                    ui.close();
                }
            });
        });
    }
    (scan, clicked, registration, details)
}

fn app_row(
    ui: &mut egui::Ui,
    app: &AppRecord,
    facts: Option<&catalog::AppFacts>,
    texture: Option<&AppTexture>,
) -> (bool, bool, bool, bool) {
    let fallback = catalog::classify(app);
    let facts = facts.unwrap_or(&fallback);
    let detail = if app.version.is_empty() {
        app.publisher.clone()
    } else {
        format!("{}  ·  v{}", app.publisher, app.version)
    };
    let detail = format!("{}  ·  {}  ·  {}", facts.source, facts.category, detail);
    let size = if app.size_mb >= 1024.0 {
        format!("{:.1} GB", app.size_mb / 1024.0)
    } else if app.size_mb > 0.0 {
        format!("{:.0} MB", app.size_mb)
    } else {
        "Size unavailable".into()
    };
    resource_row(
        ui,
        &app.name,
        &detail,
        &size,
        if app.id.starts_with("store:") {
            if facts.protected {
                "Protected"
            } else {
                "Remove"
            }
        } else if app.command.is_empty() {
            "Details"
        } else {
            "Uninstall"
        },
        !app.id.starts_with("store:") || !facts.protected,
        true,
        catalog::is_registration(&app.id),
        icons::Icon::category(&facts.category),
        texture,
    )
}

fn identity_at(
    ui: &egui::Ui,
    rect: egui::Rect,
    texture: Option<&AppTexture>,
    fallback: icons::Icon,
) {
    if let Some(texture) = texture {
        ui.painter().rect_filled(
            rect,
            9,
            if texture.dark_backdrop {
                Color32::from_rgb(47, 65, 89)
            } else {
                Color32::WHITE
            },
        );
        let available = rect.shrink(3.0);
        let size = texture.handle.size_vec2();
        let scale = (available.width() / size.x).min(available.height() / size.y);
        let image_rect = egui::Rect::from_center_size(rect.center(), size * scale);
        ui.painter().image(
            texture.handle.id(),
            image_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        icons::badge_at(ui.painter(), rect, fallback);
    }
}

fn identity_badge(
    ui: &mut egui::Ui,
    texture: Option<&AppTexture>,
    fallback: icons::Icon,
    size: f32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    identity_at(ui, rect, texture, fallback);
}

fn nav_item(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    _index: usize,
    count: Option<usize>,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            8,
            if selected {
                Color32::from_rgb(37, 57, 82)
            } else {
                Color32::from_rgb(26, 42, 61)
            },
        );
    }
    let color = if selected {
        Color32::from_rgb(144, 203, 255)
    } else {
        Color32::from_rgb(159, 177, 199)
    };
    let center = egui::pos2(rect.left() + 21.0, rect.center().y);
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(center, egui::vec2(20.0, 20.0)),
        icons::Icon::page(label),
        color,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 42.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if selected {
            Color32::WHITE
        } else {
            Color32::from_rgb(186, 199, 216)
        },
    );
    if let Some(count) = count {
        ui.painter().text(
            egui::pos2(rect.right() - 12.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(12.0),
            color,
        );
    }
    response.clicked()
}

fn stat(ui: &mut egui::Ui, value: String, label: &str) {
    surface().show(ui, |ui| {
        ui.set_min_width((ui.available_width() - 2.0).max(0.0));
        ui.label(RichText::new(value).size(26.0).strong().color(INK));
        ui.label(RichText::new(label).size(12.0).color(MUTED));
    });
}
fn filter_apps(apps: &[AppRecord], search: &str, sort: usize, page: &str) -> Vec<usize> {
    let needle = search.trim().to_lowercase();
    let mut indices: Vec<usize> = apps
        .iter()
        .enumerate()
        .filter(|(_, a)| {
            (needle.is_empty()
                || a.name.to_lowercase().contains(&needle)
                || a.publisher.to_lowercase().contains(&needle))
                && (page != "Large apps" || a.size_mb >= 1024.0)
                && (page != "Recently installed" || !a.installed.is_empty())
        })
        .map(|(i, _)| i)
        .collect();
    indices.sort_by(|&a, &b| {
        match if page == "Recently installed" {
            2
        } else {
            sort
        } {
            1 => apps[b].size_mb.total_cmp(&apps[a].size_mb),
            2 => apps[b].installed.cmp(&apps[a].installed),
            _ => apps[a]
                .name
                .to_lowercase()
                .cmp(&apps[b].name.to_lowercase()),
        }
    });
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_runs_only_after_uninstaller_returns_and_refreshes_inventory() {
        let events = std::cell::RefCell::new(Vec::new());
        let (_, plan) = uninstall_then_scan(
            || {
                events.borrow_mut().push("finished");
                Ok("done".into())
            },
            || {
                events.borrow_mut().push("inventory");
                Ok(vec![])
            },
            || {
                events.borrow_mut().push("scan");
                Ok(ScanPlan::default())
            },
        )
        .unwrap();
        assert_eq!(*events.borrow(), ["finished", "inventory", "scan"]);
        assert!(!plan.active);
    }

    #[test]
    fn cancellation_or_launch_failure_never_starts_cleanup() {
        let result = uninstall_then_scan(
            || Err("cancelled".into()),
            || panic!("inventory must not run"),
            || panic!("scan must not run"),
        );
        assert_eq!(result.unwrap_err(), "cancelled");
    }

    #[test]
    fn virtual_rows_keep_the_same_height_for_long_names_and_narrow_windows() {
        let ctx = egui::Context::default();
        for width in [570.0, 1100.0] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(width);
                let before = ui.next_widget_position().y;
                app_row(
                    ui,
                    &AppRecord {
                        name: "Very long application name ".repeat(20),
                        publisher: "Publisher ".repeat(30),
                        ..Default::default()
                    },
                    None,
                    None,
                );
                let after = ui.next_widget_position().y;
                assert!(
                    (after - before - APP_ROW_HEIGHT - ui.spacing().item_spacing.y).abs() < 0.1,
                    "width={width}, row advance={}, expected={}",
                    after - before,
                    APP_ROW_HEIGHT + ui.spacing().item_spacing.y
                );
            });
            output.textures_delta.clear();
        }
    }
    #[test]
    fn filters_and_sorts_realistic_inventory() {
        let apps = vec![
            AppRecord {
                name: "Zed".into(),
                publisher: "Tools".into(),
                size_mb: 20.0,
                installed: "20261114".into(),
                ..Default::default()
            },
            AppRecord {
                name: "Alpha".into(),
                publisher: "Big Publisher".into(),
                size_mb: 2048.0,
                ..Default::default()
            },
        ];
        assert_eq!(filter_apps(&apps, "", 0, "All apps"), vec![1, 0]);
        assert_eq!(filter_apps(&apps, "publisher", 0, "All apps"), vec![1]);
        assert_eq!(filter_apps(&apps, "", 0, "Large apps"), vec![1]);
        assert_eq!(filter_apps(&apps, "", 0, "Recently installed"), vec![0]);
        assert_eq!(
            filter_apps(&apps, "missing", 0, "All apps"),
            Vec::<usize>::new()
        );
    }
}

enum ResultMessage {
    Catalog(AppResult<catalog::Catalog>),
    Scan(AppResult<ScanPlan>),
    Backups(AppResult<Vec<Backup>>),
    Startup(AppResult<Vec<StartupEntry>>),
    Store(AppResult<Vec<StoreApp>>),
    Done(AppResult<String>),
    Clean(AppResult<Vec<CleanupItem>>),
    DriveScan(ScanMode, AppResult<Option<DriveScan>>),
    DriveClean(AppResult<Vec<CleanupItem>>),
    Uninstalled(String, AppResult<(Vec<AppRecord>, ScanPlan)>),
}
#[derive(Clone)]
enum Action {
    Uninstall(AppRecord),
    Cleanup,
    CleanDrive,
    Restore(String),
    Disable(String),
    RemoveStore(String),
    RestorePoint,
    Registration(AppRecord),
}
pub struct SimpleDeleteApp {
    tx: Sender<ResultMessage>,
    rx: Receiver<ResultMessage>,
    busy: usize,
    page: String,
    search: String,
    sort: usize,
    apps: Vec<AppRecord>,
    backups: Vec<Backup>,
    startup: Vec<StartupEntry>,
    store: Vec<StoreApp>,
    scan: Option<ScanPlan>,
    selected: Vec<bool>,
    acknowledge: bool,
    force_cleanup: bool,
    confirmation: Option<Action>,
    typed: String,
    notice: String,
    cleanup_name: String,
    cleanup_app: Option<AppRecord>,
    cleanup_results: Vec<CleanupItem>,
    drive_cleanup: DriveCleanup,
    filter_key: Option<(String, usize, String)>,
    visible: Vec<usize>,
    activity: Vec<String>,
    uninstalling: Option<String>,
    facts: std::collections::HashMap<String, catalog::AppFacts>,
    detection_warnings: Vec<String>,
    recommendations: Vec<suggestions::Suggestion>,
    kept: std::collections::HashSet<String>,
    details: Option<AppRecord>,
    source_filter: usize,
    show_components: bool,
    icon_tx: Sender<Vec<app_icons::LoadedIcon>>,
    icon_rx: Receiver<Vec<app_icons::LoadedIcon>>,
    app_textures: std::collections::HashMap<String, AppTexture>,
    icon_requested: std::collections::HashSet<String>,
    logo: egui::TextureHandle,
}
impl SimpleDeleteApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::ThemePreference::Light);
        let mut fonts = egui::FontDefinitions::default();
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            fonts.font_data.insert(
                "windows-ui".into(),
                egui::FontData::from_owned(bytes).into(),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "windows-ui".into());
        }
        cc.egui_ctx.set_fonts(fonts);
        let mut style = (*cc.egui_ctx.style_of(egui::Theme::Light)).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.panel_fill = CANVAS;
        style.visuals.window_fill = Color32::WHITE;
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(220, 233, 255);
        style.visuals.selection.bg_fill = Color32::from_rgb(210, 228, 255);
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.visuals.override_text_color = Some(INK);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(239, 243, 249);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(239, 243, 249);
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(226, 235, 251);
        style.visuals.extreme_bg_color = Color32::WHITE;
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        cc.egui_ctx.set_style_of(egui::Theme::Light, style);
        cc.egui_ctx
            .options_mut(|options| options.input_options.line_scroll_speed = 28.0);
        let (tx, rx) = mpsc::channel();
        let (icon_tx, icon_rx) = mpsc::channel();
        let logo = image::load_from_memory(include_bytes!("../../assets/app.png"))
            .expect("Bundled application logo")
            .into_rgba8();
        let logo = cc.egui_ctx.load_texture(
            "simple-delete-logo",
            egui::ColorImage::from_rgba_unmultiplied(
                [logo.width() as usize, logo.height() as usize],
                logo.as_raw(),
            ),
            egui::TextureOptions::LINEAR,
        );
        let mut app = Self {
            logo,
            tx,
            rx,
            busy: 0,
            page: "All apps".into(),
            search: String::new(),
            sort: 0,
            apps: vec![],
            backups: vec![],
            startup: vec![],
            store: vec![],
            scan: None,
            selected: vec![],
            acknowledge: false,
            force_cleanup: false,
            confirmation: None,
            typed: String::new(),
            notice: String::new(),
            cleanup_name: String::new(),
            cleanup_app: None,
            cleanup_results: vec![],
            drive_cleanup: DriveCleanup::default(),
            filter_key: None,
            visible: vec![],
            activity: vec![],
            uninstalling: None,
            facts: Default::default(),
            detection_warnings: vec![],
            recommendations: vec![],
            kept: Default::default(),
            details: None,
            source_filter: 0,
            show_components: false,
            icon_tx,
            icon_rx,
            app_textures: Default::default(),
            icon_requested: Default::default(),
        };
        match suggestions::load_kept() {
            Ok(kept) => app.kept = kept,
            Err(e) => app.notice = e,
        }
        app.refresh("All apps", &cc.egui_ctx);
        app
    }
    fn job(&mut self, ctx: &egui::Context, f: impl FnOnce() -> ResultMessage + Send + 'static) {
        self.busy += 1;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
                    ResultMessage::Done(Err("Background operation failed. See native logs.".into()))
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn refresh(&mut self, page: &str, ctx: &egui::Context) {
        if page == complete_cleanup::PAGE {
            self.drive_cleanup.refresh_drives();
            return;
        }
        if self.busy > 0 {
            return;
        }
        match page {
            "Cleanup" if !self.cleanup_name.trim().is_empty() => self.scan_cleanup(ctx),
            "All apps" | "Large apps" | "Recently installed" | "Suggestions" => {
                self.job(ctx, || ResultMessage::Catalog(catalog::load()))
            }
            "Recovery" => self.job(ctx, || ResultMessage::Backups(backend::list_backups())),
            "Startup" => self.job(ctx, || ResultMessage::Startup(backend::startup_entries())),
            "Store apps" => self.job(ctx, || ResultMessage::Store(backend::store_apps())),
            _ => {}
        }
    }
    fn scan_cleanup(&mut self, ctx: &egui::Context) {
        let name = self.cleanup_name.clone();
        let app = self.cleanup_app.clone().filter(|a| a.name == name);
        self.job(ctx, move || {
            ResultMessage::Scan(backend::scan_leftovers(&name, app.as_ref()))
        });
    }
    fn run_action(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Uninstall(a) => {
                self.cleanup_app = Some(a.clone());
                self.cleanup_results.clear();
                self.uninstalling = Some(a.name.clone());
                self.scan = None;
                self.selected.clear();
                self.notice = format!(
                    "Complete the uninstaller for {}. Leftover review will open automatically when it finishes.",
                    a.name
                );
                self.job(ctx, move || {
                    let result = uninstall_then_scan(
                        || backend::uninstall(&a),
                        backend::inventory,
                        || backend::scan_leftovers(&a.name, Some(&a)),
                    );
                    ResultMessage::Uninstalled(a.name, result)
                });
            }
            Action::Cleanup => {
                if let Some(plan) = &self.scan {
                    let id = plan.id.clone();
                    let ids = plan
                        .candidates
                        .iter()
                        .zip(&self.selected)
                        .filter(|(c, s)| **s && !c.blocked)
                        .map(|(c, _)| c.id.clone())
                        .collect::<Vec<_>>();
                    let force = self.force_cleanup;
                    self.job(ctx, move || {
                        ResultMessage::Clean(backend::cleanup(&id, &ids, force))
                    });
                }
            }
            Action::CleanDrive => {
                if let Some(plan) = self.drive_cleanup.scan.take() {
                    let selected = std::mem::take(&mut self.drive_cleanup.selected);
                    self.drive_cleanup.cleaning = true;
                    self.job(ctx, move || {
                        ResultMessage::DriveClean(backend::clean_drive(plan, selected))
                    });
                }
            }
            Action::Restore(id) => self.job(ctx, move || {
                ResultMessage::Done(backend::restore_backup(&id))
            }),
            Action::Disable(id) => self.job(ctx, move || {
                ResultMessage::Done(backend::disable_startup(&id))
            }),
            Action::RemoveStore(id) => {
                self.job(ctx, move || ResultMessage::Done(backend::remove_store(&id)))
            }
            Action::RestorePoint => {
                self.job(ctx, || ResultMessage::Done(backend::create_restore_point()))
            }
            Action::Registration(a) => self.job(ctx, move || {
                ResultMessage::Done(backend::remove_registration(&a))
            }),
        }
    }
    fn review_app(&mut self, app: AppRecord) {
        if let Some(id) = app.id.strip_prefix("store:") {
            if !self.facts.get(&app.id).is_some_and(|facts| facts.protected) {
                self.confirmation = Some(Action::RemoveStore(id.to_owned()));
            }
        } else if catalog::is_registration(&app.id) && !app.command.is_empty() {
            self.confirmation = Some(Action::Uninstall(app));
        } else {
            self.details = Some(app);
        }
    }

    fn update_suggestions(&mut self) {
        self.recommendations = suggestions::rank(
            &catalog::Catalog {
                apps: self.apps.clone(),
                facts: self.facts.clone(),
                warnings: self.detection_warnings.clone(),
            },
            &self.kept,
        );
    }
    fn receive(&mut self, ctx: &egui::Context) {
        for _ in 0..4 {
            let Ok(batch) = self.icon_rx.try_recv() else {
                break;
            };
            for icon in batch {
                if self.apps.iter().any(|app| app.id == icon.id) {
                    let visible = icon
                        .image
                        .pixels
                        .iter()
                        .filter(|pixel| pixel.a() > 32)
                        .count();
                    if visible == 0 {
                        continue;
                    }
                    let white = icon
                        .image
                        .pixels
                        .iter()
                        .filter(|pixel| {
                            let a = pixel.a() as u32;
                            a > 32
                                && [pixel.r(), pixel.g(), pixel.b()]
                                    .iter()
                                    .all(|&channel| channel as u32 * 255 >= 220 * a)
                        })
                        .count();
                    let dark_backdrop = white * 4 > visible * 3;
                    self.app_textures.insert(
                        icon.id.clone(),
                        AppTexture {
                            dark_backdrop,
                            handle: ctx.load_texture(
                                format!("app-icon:{}", icon.id),
                                icon.image,
                                egui::TextureOptions::LINEAR,
                            ),
                        },
                    );
                }
            }
            ctx.request_repaint();
        }
        let mut reload = false;
        while let Ok(msg) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match msg {
                ResultMessage::Uninstalled(name, result) => {
                    self.uninstalling = None;
                    match result {
                        Ok((apps, plan)) => {
                            self.apps = apps;
                            self.filter_key = None;
                            self.cleanup_name = name.clone();
                            self.notice = if plan.active {
                                format!(
                                    "The uninstaller closed, but {name} is still registered. It may have been cancelled or handed off to another process. Finish it before cleaning; review the warnings below."
                                )
                            } else {
                                format!(
                                    "Uninstaller finished for {name}. Review the leftover scan below."
                                )
                            };
                            self.selected = vec![false; plan.candidates.len()];
                            self.acknowledge = false;
                            self.force_cleanup = false;
                            self.scan = Some(plan);
                            self.page = "Cleanup".into();
                        }
                        Err(e) => self.notice = format!("{name}: {e}"),
                    }
                    self.activity.insert(0, self.notice.clone());
                    self.refresh("All apps", ctx);
                }
                ResultMessage::Catalog(r) => match r {
                    Ok(v) => {
                        self.recommendations = suggestions::rank(&v, &self.kept);
                        self.apps = v.apps;
                        self.facts = v.facts;
                        self.detection_warnings = v.warnings;
                        let ids: std::collections::HashSet<_> =
                            self.apps.iter().map(|app| app.id.clone()).collect();
                        self.app_textures.retain(|id, _| ids.contains(id));
                        self.icon_requested.retain(|id| ids.contains(id));
                        let pending = self
                            .apps
                            .iter()
                            .filter(|app| !self.icon_requested.contains(&app.id))
                            .cloned()
                            .collect::<Vec<_>>();
                        self.icon_requested
                            .extend(pending.iter().map(|app| app.id.clone()));
                        if !pending.is_empty() {
                            app_icons::start(pending, self.icon_tx.clone(), ctx.clone());
                        }
                        self.filter_key = None;
                    }
                    Err(e) => self.notice = e,
                },
                ResultMessage::Scan(r) => match r {
                    Ok(v) => {
                        self.notice = if v.active {
                            format!(
                                "{} is still registered. Finish its uninstaller, then Check again before cleanup.",
                                v.name
                            )
                        } else {
                            format!(
                                "Fresh scan for {}. Review each path before selecting it.",
                                v.name
                            )
                        };
                        self.selected = vec![false; v.candidates.len()];
                        self.acknowledge = false;
                        self.force_cleanup = false;
                        self.scan = Some(v);
                        self.page = "Cleanup".into();
                    }
                    Err(e) => self.notice = e,
                },
                ResultMessage::Backups(r) => match r {
                    Ok(v) => self.backups = v,
                    Err(e) => self.notice = e,
                },
                ResultMessage::Startup(r) => match r {
                    Ok(v) => self.startup = v,
                    Err(e) => self.notice = e,
                },
                ResultMessage::Store(r) => match r {
                    Ok(v) => self.store = v,
                    Err(e) => self.notice = e,
                },
                ResultMessage::Done(r) => {
                    self.drive_cleanup.progress = None;
                    self.drive_cleanup.cleaning = false;
                    self.notice = r.unwrap_or_else(|e| format!("Error: {e}"));
                    self.activity.insert(0, self.notice.clone());
                    reload = true;
                }
                ResultMessage::Clean(r) => {
                    match r {
                        Ok(items) => {
                            let succeeded =
                                items.iter().filter(|item| item.backup_id.is_some()).count();
                            let failed = items.len() - succeeded;
                            self.notice = format!(
                                "Cleanup finished: {succeeded} backed up, {failed} failed. Review each result below."
                            );
                            self.cleanup_results = items;
                            self.scan = None;
                            self.selected.clear();
                            self.acknowledge = false;
                            self.force_cleanup = false;
                        }
                        Err(e) => {
                            self.notice = format!(
                                "Cleanup could not start: {e}. Check again for a fresh scan."
                            )
                        }
                    }
                    self.activity.insert(0, self.notice.clone());
                }
                ResultMessage::DriveScan(mode, result) => {
                    let cancelled = self.drive_cleanup.progress.take().is_some_and(|progress| {
                        progress
                            .cancelled
                            .load(std::sync::atomic::Ordering::Relaxed)
                    });
                    if cancelled {
                        self.notice = "Drive scan cancelled.".into();
                        continue;
                    }
                    match result {
                        Ok(Some(scan)) if mode == ScanMode::Automatic => {
                            let mut summary = format!(
                                "{} entries checked · {} protected entries skipped · {} unreadable entries",
                                scan.inspected, scan.protected, scan.unreadable,
                            );
                            if scan.omitted > 0 {
                                summary.push_str(&format!(
                                    " · {} matches beyond this run’s limit",
                                    scan.omitted
                                ));
                            }
                            self.drive_cleanup.automatic_summary = summary;
                            self.drive_cleanup.cleaning = true;
                            self.job(ctx, move || {
                                ResultMessage::DriveClean(backend::auto_clean_drive(scan))
                            });
                        }
                        Ok(Some(scan)) => self.drive_cleanup.set_scan(scan),
                        Ok(None) => self.notice = "Drive scan cancelled.".into(),
                        Err(error) => self.notice = error,
                    }
                }
                ResultMessage::DriveClean(result) => {
                    self.drive_cleanup.cleaning = false;
                    match result {
                        Ok(items) if items.is_empty() => {
                            self.notice =
                                "Auto Clean found no eligible temporary files to move.".into();
                        }
                        Ok(items) => {
                            let moved =
                                items.iter().filter(|item| item.backup_id.is_some()).count();
                            self.notice = format!(
                                "Complete Clean Up moved {moved} files to Recovery. {} could not be moved.",
                                items.len() - moved,
                            );
                            self.drive_cleanup.results = items;
                        }
                        Err(error) => {
                            self.notice = format!("{error} Run cleanup again before retrying.")
                        }
                    }
                    self.activity.insert(0, self.notice.clone());
                    reload = self.page == "Recovery";
                }
            }
        }
        if reload
            && matches!(
                self.page.as_str(),
                "Recovery" | "Startup" | "Store apps" | "Suggestions"
            )
        {
            let page = self.page.clone();
            self.refresh(&page, ctx);
        }
    }
}
impl eframe::App for SimpleDeleteApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx_owned = root_ui.ctx().clone();
        let ctx = &ctx_owned;
        self.receive(ctx);
        if ctx.input(|input| input.is_scrolling()) {
            ctx.request_repaint();
        }
        egui::Panel::left("sidebar")
            .exact_size(224.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(16, 28, 45))
                    .inner_margin(16),
            )
            .show(root_ui, |ui| {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.image((self.logo.id(), egui::vec2(32.0, 32.0)));
                    ui.label(
                        RichText::new("Simple Delete")
                            .size(18.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                });
                ui.add_space(12.0);
                let cleanup_button = primary(complete_cleanup::PAGE)
                    .selected(self.page == complete_cleanup::PAGE);
                if ui.add_sized([ui.available_width(), 36.0], cleanup_button).clicked() {
                    self.page = complete_cleanup::PAGE.into();
                    self.refresh(complete_cleanup::PAGE, ctx);
                }
                ui.add_space(18.0);
                let nav_height = (ui.available_height() - 72.0).max(100.0);
                egui::ScrollArea::vertical()
                    .id_salt("sidebar-navigation")
                    .max_height(nav_height)
                    .show(ui, |ui| {
                        for (section, pages) in [
                            (
                                "LIBRARY",
                                vec![
                                    ("All apps", 0),
                                    ("Large apps", 1),
                                    ("Recently installed", 2),
                                    ("Store apps", 7),
                                ],
                            ),
                            (
                                "TOOLS",
                                vec![
                                    ("Suggestions", 9),
                                    ("Cleanup", 4),
                                    ("Startup", 6),
                                ],
                            ),
                            ("HISTORY", vec![("Recovery", 8), ("Activity", 3)]),
                        ] {
                            ui.label(
                                RichText::new(section)
                                    .size(10.0)
                                    .strong()
                                    .color(Color32::from_rgb(105, 128, 155)),
                            );
                            ui.add_space(4.0);
                            for (page, index) in pages {
                                let count = match page {
                                    "All apps" => Some(self.apps.len()),
                                    "Large apps" => Some(
                                        self.apps.iter().filter(|a| a.size_mb >= 1024.0).count(),
                                    ),
                                    _ => None,
                                };
                                if nav_item(ui, page, self.page == page, index, count) {
                                    self.page = page.into();
                                    self.refresh(page, ctx);
                                }
                            }
                            ui.add_space(16.0);
                        }
                    });
                let remaining = ui.available_height() - 72.0;
                if remaining > 0.0 {
                    ui.add_space(remaining);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if self.busy > 0 {
                        ui.spinner();
                    } else {
                        let (r, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            r.center(),
                            3.0,
                            Color32::from_rgb(93, 207, 167),
                        );
                    }
                    ui.label(
                        RichText::new(if self.busy > 0 {
                            "Working in background"
                        } else {
                            "Ready when you are"
                        })
                        .size(11.0)
                        .color(Color32::from_rgb(150, 172, 195)),
                    );
                });
                ui.label(
                    RichText::new(concat!("SIMPLE DELETE  /  ", env!("CARGO_PKG_VERSION")))
                        .size(9.0)
                        .color(Color32::from_rgb(91, 113, 141)),
                );
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(CANVAS).inner_margin(28)).show(root_ui,|ui| {
            let (title,subtitle) = match self.page.as_str() {
                "All apps" => ("Installed apps","Desktop apps, Store packages, Steam games and discovered app folders."),
                "Suggestions" => ("Smart suggestions","Evidence to help you decide what to keep."),
                "Large apps" => ("Large apps","Find the apps taking up the most space."),
                "Recently installed" => ("Recently installed","Review the latest additions to your PC."),
                "Cleanup" => ("Leftover cleanup","Review what remains. Keep what matters."),
                complete_cleanup::PAGE => (complete_cleanup::PAGE, "Find old temporary files and files you may no longer need."),
                "Startup" => ("Startup apps","Choose what starts with Windows."),
                "Store apps" => ("Microsoft Store apps","Manage packages installed for your account."),
                "Recovery" => ("Recovery","Your safety net for changes made here."),
                _ => ("Activity","A record of the actions in this session.")
            };
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| { icons::badge(ui,icons::Icon::page(&self.page),36.0); ui.label(RichText::new(title).size(29.0).strong().color(INK)); });
                    ui.label(RichText::new(subtitle).size(13.0).color(MUTED));
                });
                if self.page != complete_cleanup::PAGE { ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                    if ui.add(egui::Button::new("Refresh").min_size(egui::vec2(86.0,34.0))).clicked() {let page=self.page.clone();self.refresh(&page,ctx);}
                }); }
            });
            ui.add_space(20.0);
            if !self.notice.is_empty() {
                egui::Frame::new().fill(Color32::from_rgb(231,240,255)).corner_radius(9).inner_margin(14).show(ui,|ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {ui.set_max_width((ui.available_width()-68.0).max(100.0));ui.label(RichText::new(&self.notice).color(Color32::from_rgb(46,79,131)));});
                        if ui.small_button("Dismiss").clicked() {self.notice.clear();}
                    });
                });ui.add_space(12.0);
            }
            match self.page.as_str() {
                "All apps" | "Large apps" | "Recently installed" => {
                    ui.columns(3,|cols| {
                        stat(&mut cols[0],self.apps.len().to_string(),"DETECTED ENTRIES");
                        stat(&mut cols[1],format!("{:.1} GB",self.apps.iter().map(|a|a.size_mb).sum::<f64>()/1024.0),"REPORTED APP SIZE");
                        stat(&mut cols[2],self.apps.iter().filter(|a|a.size_mb>=1024.0).count().to_string(),"APPS OVER 1 GB");
                    });
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let search_width=(ui.available_width()-225.0).clamp(140.0,540.0);
                        ui.add_sized([search_width,36.0],egui::TextEdit::singleline(&mut self.search).hint_text("Search apps or publishers...").margin(egui::vec2(12.0,9.0)));
                        egui::ComboBox::from_id_salt("sort").width(145.0).selected_text(["Name: A to Z","Largest first","Newest first"][self.sort]).show_ui(ui,|ui| {
                            ui.selectable_value(&mut self.sort,0,"Name: A to Z");ui.selectable_value(&mut self.sort,1,"Largest first");ui.selectable_value(&mut self.sort,2,"Newest first");
                        });
                    });
                    ui.add_space(6.0);
                    let previous_filters=(self.source_filter,self.show_components);
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("source-filter").selected_text(["All sources","Desktop registry","Microsoft Store","Steam","App folders"][self.source_filter]).show_ui(ui,|ui|{for (i,name) in ["All sources","Desktop registry","Microsoft Store","Steam","App folders"].iter().enumerate(){ui.selectable_value(&mut self.source_filter,i,*name);}});
                        ui.checkbox(&mut self.show_components,"Show Store system components");
                    });
                    if previous_filters!=(self.source_filter,self.show_components){self.filter_key=None;}
                    if !self.detection_warnings.is_empty(){ui.collapsing(format!("Detection notes ({})",self.detection_warnings.len()),|ui|{for warning in &self.detection_warnings{ui.label(RichText::new(warning).small().color(MUTED));}});}
                    let key=(self.search.clone(),self.sort,self.page.clone());
                    if self.filter_key.as_ref()!=Some(&key) {self.visible=filter_apps(&self.apps,"",self.sort,&self.page).into_iter().filter(|&index| {
    let app=&self.apps[index]; let facts=self.facts.get(&app.id);
    let is_store=app.id.starts_with("store:");
    let is_steam=facts.is_some_and(|f|f.source.to_lowercase().contains("steam"));
    let source_match=match self.source_filter {1=>catalog::is_registration(&app.id)&&!is_steam,2=>is_store,3=>is_steam,4=>!is_store&&!is_steam&&!catalog::is_registration(&app.id),_=>true};
    let needle=self.search.trim().to_lowercase();
    let search_match=needle.is_empty()||app.name.to_lowercase().contains(&needle)||app.publisher.to_lowercase().contains(&needle)||facts.is_some_and(|f|format!("{} {} {}",f.source,f.category,f.description).to_lowercase().contains(&needle));
    source_match&&search_match&&(self.show_components||!is_store||!facts.is_some_and(|f|f.protected))
}).collect();self.filter_key=Some(key);}
                    ui.horizontal(|ui| {ui.label(RichText::new(format!("{} apps",self.visible.len())).size(12.0).color(MUTED));ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{ui.label(RichText::new("Uninstall, then review leftovers automatically").size(11.0).color(MUTED));});});
                    ui.add_space(4.0);
                    if self.visible.is_empty() { empty_state(ui,if self.busy>0 {"Loading your library..."} else {"No apps found"},"Try another app name or publisher."); }
                    let mut scan=None; let mut review=None;
                    egui::ScrollArea::vertical().id_salt("installed-apps").auto_shrink([false,false]).show_rows(ui,APP_ROW_HEIGHT,self.visible.len(),|ui,range| {
                        for row in range {
                            let a=&self.apps[self.visible[row]];
                            let (scan_clicked,uninstall_clicked,registration_clicked,details_clicked)=app_row(ui,a,self.facts.get(&a.id),self.app_textures.get(&a.id));
                            if scan_clicked {scan=Some(a.clone());}
                            if uninstall_clicked && self.uninstalling.is_none() { review=Some(a.clone()); }
                            if registration_clicked {self.confirmation=Some(Action::Registration(a.clone()));}
                            if details_clicked {self.details=Some(a.clone());}
                        }
                    });
                    if let Some(a)=review {self.review_app(a);}
                    if let Some(a)=scan {self.cleanup_name=a.name.clone();self.cleanup_app=Some(a);self.cleanup_results.clear();self.scan_cleanup(ctx);}
                }
                "Suggestions" => {
                    surface().show(ui,|ui| {
                        ui.set_min_width((ui.available_width()-2.0).max(0.0));
                        ui.label(RichText::new("A shortlist to review, with a reason for every app").strong().size(17.0));
                        ui.label(RichText::new("Suggestions run locally. They use reported size and distinct registered versions, not usage history. Large does not mean unnecessary.").color(MUTED));
                        ui.label(RichText::new("Recognized drivers, security tools, shared runtimes, and system packages are excluded. Check your own dependencies before removing software.").small().color(MUTED));
                        if !self.kept.is_empty()&&ui.small_button(format!("Reset {} saved Keep choices",self.kept.len())).clicked(){self.kept.clear();if let Err(e)=suggestions::save_kept(&self.kept){self.notice=e;}self.update_suggestions();}
                    });
                    ui.add_space(14.0);
                    ui.label(RichText::new(format!("{} apps worth reviewing",self.recommendations.len())).size(14.0).strong());
                    if self.recommendations.is_empty(){empty_state(ui,"No strong candidates right now","Refresh detection or browse Large apps. We won't invent a reason to remove something.");}
                    let mut keep=None;let mut review=None;
                    egui::ScrollArea::vertical().id_salt("suggestions").show(ui,|ui| {
                        for suggestion in &self.recommendations {
                            surface().show(ui,|ui| {
                                ui.set_min_width((ui.available_width()-2.0).max(0.0));
                                ui.horizontal(|ui| { let category=self.facts.get(&suggestion.app.id).map(|f|f.category.as_str()).unwrap_or("Other apps"); identity_badge(ui,self.app_textures.get(&suggestion.app.id),icons::Icon::category(category),34.0); ui.label(RichText::new(&suggestion.app.name).size(18.0).strong()); });
                                if let Some(facts)=self.facts.get(&suggestion.app.id){ui.label(RichText::new(format!("{}  ·  {}  ·  {}",facts.source,facts.category,suggestion.app.publisher)).small().color(MUTED));}
                                for reason in &suggestion.reasons {ui.label(reason);}
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if ui.add(primary("Review uninstall")).clicked(){review=Some(suggestion.app.clone());}
                                    if ui.button("App details").clicked(){self.details=Some(suggestion.app.clone());}
                                    if ui.button("Keep this app").clicked(){keep=Some(suggestion.app.id.clone());}
                                });
                            });ui.add_space(6.0);
                        }
                    });
                    if let Some(id)=keep{self.kept.insert(id);if let Err(e)=suggestions::save_kept(&self.kept){self.notice=e;}self.update_suggestions();}
                    if let Some(app)=review{self.review_app(app);}
                }
                "Cleanup" => {
                    surface().show(ui,|ui| {
                        ui.set_min_width((ui.available_width()-2.0).max(0.0));
                        ui.label(RichText::new("Find leftover files and settings").strong().size(16.0));
                        ui.label(RichText::new("This step opens automatically after an uninstall. You can also scan by app name.").size(12.0).color(MUTED));
                        ui.horizontal(|ui| {
                            ui.add_sized([ui.available_width().min(380.0)-135.0,34.0],egui::TextEdit::singleline(&mut self.cleanup_name).hint_text("App name"));
                            if ui.add_enabled(self.busy==0 && !self.cleanup_name.trim().is_empty(),primary(if self.scan.is_some() || !self.cleanup_results.is_empty() {"Check again"} else {"Scan leftovers"})).clicked() {self.scan_cleanup(ctx);}
                        });
                    });
                    ui.add_space(14.0);
                    if !self.cleanup_results.is_empty() {
                        let results = self.cleanup_results.clone();
                        surface().show(ui, |ui| {
                            ui.label(RichText::new("Last cleanup results").strong());
                            egui::ScrollArea::vertical().id_salt("cleanup-results").max_height(180.0).show(ui, |ui| {
                                for item in &results {
                                    ui.label(RichText::new(&item.path).strong());
                                    if let Some(error) = &item.error { ui.colored_label(Color32::from_rgb(177,62,62), format!("Failed: {error}")); }
                                    else if let Some(id) = &item.backup_id { ui.label(RichText::new(format!("Backed up · {id}")).color(MUTED)); }
                                    ui.separator();
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Open Recovery").clicked() { self.page="Recovery".into(); self.refresh("Recovery",ctx); }
                                ui.label(RichText::new("Check again before retrying failed items; paths are revalidated.").small().color(MUTED));
                            });
                        });
                        ui.add_space(10.0);
                    }
                    if let Some(plan)=&self.scan {
                        ui.horizontal(|ui| {ui.label(RichText::new(&plan.name).size(18.0).strong());ui.label(RichText::new(format!("{} candidates",plan.candidates.len())).color(MUTED));});
                        ui.label(RichText::new(&plan.coverage).small().color(MUTED));
                        for warning in &plan.warnings {ui.colored_label(Color32::from_rgb(161,102,34),warning);}
                        if plan.candidates.is_empty() {empty_state(ui,"No matching leftovers found",if plan.warnings.is_empty(){"No candidates in the checked locations. This scan does not cover every possible leftover."}else{"Some locations could not be checked. Review the warnings; an empty result does not mean all leftovers are gone."});}
                        egui::ScrollArea::vertical().id_salt("cleanup-candidates").max_height((ui.available_height()-160.0).max(80.0)).show(ui,|ui| {
                            for (i,c) in plan.candidates.iter().enumerate() {
                                egui::Frame::new().fill(Color32::WHITE).stroke(egui::Stroke::new(1.0,LINE)).corner_radius(8).inner_margin(12).show(ui,|ui| {
                                    ui.set_min_width((ui.available_width()-2.0).max(0.0));
                                    ui.add_enabled_ui(!c.blocked,|ui| {ui.checkbox(&mut self.selected[i],RichText::new(&c.path).strong());});
                                    ui.label(RichText::new(format!("{}  ·  {}{}",c.kind,c.reason,if c.blocked {"  ·  Protected"} else {""})).size(12.0).color(MUTED));
                                });
                            }
                        });
                        ui.add_space(10.0);
                        ui.checkbox(&mut self.acknowledge,"I reviewed the selected paths. Removed items will be backed up.");
                        ui.collapsing("Advanced options",|ui| {ui.checkbox(&mut self.force_cleanup,"Allow cleanup while the app is still installed (can break it).");});
                        let count=self.selected.iter().filter(|v|**v).count();
                        ui.horizontal(|ui| {if ui.add_enabled(self.acknowledge && count>0 && self.busy==0 && (!plan.active || self.force_cleanup),primary(&format!("Clean {count} selected items"))).clicked(){self.confirmation=Some(Action::Cleanup);}ui.label(RichText::new("Review backups in Recovery.").small().color(MUTED));});
                    } else {empty_state(ui,"Ready for a fresh start","Uninstall an app or enter its name above to look for leftovers.");}
                }
                complete_cleanup::PAGE => self.drive_cleanup_ui(ui, ctx),
                "Startup" => {
                    if self.startup.is_empty() {empty_state(ui,"No startup entries to show","Refresh to check your startup registry entries.");}
                    egui::ScrollArea::vertical().id_salt("startup").show_rows(ui,APP_ROW_HEIGHT,self.startup.len(),|ui,range| {
                        for i in range {let a=&self.startup[i];if resource_row(ui,&a.name,&a.command,&a.scope,"Disable",a.editable,false,false,icons::Icon::Startup,None).1 {self.confirmation=Some(Action::Disable(a.id.clone()));}}
                    });
                }
                "Store apps" => {
                    ui.label(RichText::new(format!("{} packages  ·  System frameworks are protected",self.store.len())).small().color(MUTED));ui.add_space(8.0);
                    if self.store.is_empty() {empty_state(ui,"Loading Store packages","Windows may take a moment to return the package list.");}
                    egui::ScrollArea::vertical().id_salt("store").show_rows(ui,APP_ROW_HEIGHT,self.store.len(),|ui,range| {
                        for i in range {let a=&self.store[i];if resource_row(ui,&a.name,&a.publisher,&a.version,if a.removable {"Remove"} else {"Protected"},a.removable,false,false,icons::Icon::Store,self.app_textures.get(&format!("store:{}",a.id))).1 {self.confirmation=Some(Action::RemoveStore(a.id.clone()));}}
                    });
                }
                "Recovery" => {
                    surface().show(ui,|ui| {ui.set_min_width((ui.available_width()-2.0).max(0.0));ui.label(RichText::new("Before making a big change").strong().size(17.0));ui.label(RichText::new("Create a Windows restore point for an additional way back.").color(MUTED));if ui.button("Create restore point").clicked(){self.confirmation=Some(Action::RestorePoint);}});
                    ui.add_space(16.0);ui.label(RichText::new("Your backups").strong().size(18.0));
                    if self.backups.is_empty(){empty_state(ui,"No backups yet","Items cleaned up here will appear in Recovery.");}
                    egui::ScrollArea::vertical().id_salt("recovery").show_rows(ui,APP_ROW_HEIGHT,self.backups.len(),|ui,range|{for i in range{let a=&self.backups[i];if resource_row(ui,&a.name,&format!("{}  ·  {}",a.kind,a.original),&a.status,"Restore",a.status!="restored",false,false,icons::Icon::Recovery,None).1{self.confirmation=Some(Action::Restore(a.id.clone()));}}});
                }
                "Activity" => {
                    if self.activity.is_empty(){empty_state(ui,"All quiet for now","Completed operations will appear here during this session.");}
                    egui::ScrollArea::vertical().id_salt("activity").show(ui,|ui|{for entry in &self.activity{surface().show(ui,|ui|{ui.set_min_width((ui.available_width()-2.0).max(0.0));ui.label(entry);});ui.add_space(4.0);}});
                }
                _=>{}
            }
        });
        if let Some(app) = self.details.clone() {
            let mut open = true;
            let mut review = false;
            let mut keep = false;
            let fallback = catalog::classify(&app);
            let facts = self.facts.get(&app.id).unwrap_or(&fallback);
            egui::Window::new("App details").id(egui::Id::unique("app-details")).open(&mut open).default_width(580.0).resizable(true).show(ctx,|ui| {
                ui.horizontal(|ui| {identity_badge(ui,self.app_textures.get(&app.id),icons::Icon::category(&facts.category),44.0);ui.label(RichText::new(&app.name).size(23.0).strong());});
                ui.label(RichText::new(&facts.description).color(MUTED));ui.add_space(12.0);
                egui::Grid::new("app-facts").num_columns(2).spacing([20.0,12.0]).show(ui,|ui| {
                    for (key,value) in [("Detected through",facts.source.clone()),("Scope",facts.scope.clone()),("Category",facts.category.clone()),("Publisher",app.publisher.clone()),("Version",app.version.clone()),("Reported size",if app.size_mb>0.0{format!("{:.1} MB",app.size_mb)}else{"Not supplied by this source".into()}),("Install location",app.location.clone()),("Installed date",app.installed.clone())] {
                        ui.label(RichText::new(key).small().color(MUTED));ui.label(if value.is_empty(){"Not supplied"}else{&value});ui.end_row();
                    }
                });ui.add_space(14.0);
                if facts.protected{ui.label(RichText::new("System, shared dependency, driver or security component: excluded from suggestions.").color(Color32::from_rgb(159,103,36)));}
                let can_remove=if app.id.starts_with("store:"){!facts.protected}else{catalog::is_registration(&app.id)&&!app.command.is_empty()};
                if !can_remove&&!app.id.starts_with("store:"){ui.label(RichText::new("Detected app folder or manifest. A verified registered uninstaller is unavailable; use its own app manager to remove it.").color(MUTED));}
                ui.horizontal(|ui|{if ui.add_enabled(can_remove&&self.busy==0,primary("Review removal")).clicked(){review=true;}if ui.button("Keep this app").clicked(){keep=true;}});
            });
            if keep {
                self.kept.insert(app.id.clone());
                if let Err(e) = suggestions::save_kept(&self.kept) {
                    self.notice = e;
                }
                self.update_suggestions();
            }
            if !open || review {
                self.details = None;
            }
            if review {
                self.review_app(app);
            }
        }
        if let Some(action) = self.confirmation.clone() {
            let mut cancel = false;
            let mut confirm = false;
            egui::Modal::new(egui::Id::unique("confirmation")).show(ctx,|ui| {
                ui.set_width(430.0);ui.add_space(6.0);
                ui.label(RichText::new("Review this action").size(22.0).strong());ui.add_space(8.0);
                match &action {
                    Action::Uninstall(a)=>{ui.label(format!("Uninstall {}?",a.name));ui.label(RichText::new("Follow the app's uninstaller. We'll open leftover review when it finishes.").color(MUTED));
                        let fallback=catalog::classify(a);let facts=self.facts.get(&a.id).unwrap_or(&fallback);
                        if facts.protected {ui.add_space(8.0);ui.colored_label(Color32::from_rgb(161,102,34),"This may be a driver, security tool or shared component. Removing it can affect Windows or other apps.");ui.label(&facts.description);}
                    }
                    Action::Cleanup=>{ui.label("Clean the selected files and registry settings?");ui.label(RichText::new("Backups will be available in Recovery.").color(MUTED));}
                    Action::CleanDrive=>{
                        let selected = self.drive_cleanup.selected.iter().filter(|&&selected| selected).count();
                        ui.label(format!("Move {selected} selected files to Recovery?"));
                        ui.label(RichText::new("You can restore them to their original locations. They will still use space on this drive.").color(MUTED));
                    }
                    Action::Restore(id)=>{ui.label(format!("Restore backup {id}?"));}
                    Action::Disable(id)=>{ui.label(format!("Disable startup entry {id}?"));}
                    Action::RemoveStore(id)=>{ui.label(format!("Remove Store package {id}?"));}
                    Action::RestorePoint=>{ui.label("Create a Windows system restore point?");}
                    Action::Registration(a)=>{ui.label(format!("Remove the registration for {}? Installed files will remain.",a.name));ui.label(RichText::new("Type REMOVE to continue.").color(MUTED));ui.text_edit_singleline(&mut self.typed);}
                }
                ui.add_space(16.0);
                ui.horizontal(|ui|{if ui.button("Cancel").clicked(){cancel=true;}if ui.add_enabled(self.busy==0&&(!matches!(action,Action::Registration(_))||self.typed=="REMOVE"),primary("Confirm")).clicked(){confirm=true;}});
            });
            if cancel || confirm {
                self.confirmation = None;
                self.typed.clear();
            }
            if confirm {
                self.run_action(action, ctx);
            }
        }
    }
}
