#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use simple_delete::{backend, catalog, suggestions, ui::SimpleDeleteApp};
use std::{fs, path::PathBuf, time::Instant};

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("SimpleDelete/logs");
        let _ = fs::create_dir_all(&root);
        let _ = fs::write(root.join("native-panic.log"), info.to_string());
    }));
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|a| a == "--diagnostics") {
        let start = Instant::now();
        let apps = backend::inventory();
        let elapsed = start.elapsed().as_millis();
        let catalog_report = catalog::load().map(|catalog| {
            let mut sources = std::collections::BTreeMap::<String, usize>::new();
            for facts in catalog.facts.values() { *sources.entry(facts.source.clone()).or_default() += 1; }
            serde_json::json!({"entries":catalog.apps.len(),"sources":sources,"warnings":catalog.warnings,"suggestions":suggestions::rank(&catalog,&Default::default()).len(),"protected":catalog.facts.values().filter(|f|f.protected).count()})
        });
        let report = serde_json::json!({"implementation":"native-rust","inventory_ms":elapsed,"applications":apps.as_ref().map(|v|v.len()).ok(),"inventory_error":apps.as_ref().err(),"catalog":catalog_report,"startup_entries":backend::startup_entries().map(|v|v.len()),"store_packages":backend::store_apps().map(|v|v.len())});
        let output = args
            .get(index + 1)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("native-diagnostics.json"));
        let _ = fs::write(output, serde_json::to_vec_pretty(&report).unwrap());
        return;
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Simple Delete")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../../assets/app.png"))
                    .expect("Bundled application icon"),
            )
            .with_inner_size([1370.0, 920.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "Simple Delete",
        options,
        Box::new(|cc| Ok(Box::new(SimpleDeleteApp::new(cc)))),
    ) {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("SimpleDelete/logs");
        let _ = fs::create_dir_all(&root);
        let _ = fs::write(root.join("native-startup.log"), error.to_string());
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
            let message: Vec<u16> = format!(
                "Unable to open Simple Delete: {error}\nDetails: {}",
                root.display()
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            let title: Vec<u16> = "Simple Delete".encode_utf16().chain(Some(0)).collect();
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
