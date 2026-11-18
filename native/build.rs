use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=../assets/app.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../assets/app.ico")
        .canonicalize()
        .unwrap();
    let rc = out.join("app.rc");
    let obj = out.join("app-resource.o");
    fs::write(
        &rc,
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let status = Command::new("windres")
        .arg("--input")
        .arg(&rc)
        .arg("--output")
        .arg(&obj)
        .args(["--output-format", "coff"])
        .status()
        .expect("Windows resource compiler missing; use Build-Rust.ps1");
    assert!(status.success(), "Failed to compile application icon");
    println!("cargo:rustc-link-arg-bin=simple-delete={}", obj.display());
}
