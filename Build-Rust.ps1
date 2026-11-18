$ErrorActionPreference = 'Stop'
$cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe'
$compiler = Join-Path $env:USERPROFILE '.codex/tools/rust-mingw/mingw64/bin'
if (-not (Test-Path -LiteralPath $cargo)) { throw 'Install the Rust GNU toolchain with rustup first.' }
if (-not (Test-Path -LiteralPath (Join-Path $compiler 'gcc.exe'))) { throw 'The MinGW compiler is missing. Add a MinGW-w64 toolchain at .codex/tools/rust-mingw/mingw64.' }
$env:PATH = "$compiler;$(Split-Path $cargo);$env:PATH"
$env:RUSTFLAGS = '-C target-feature=+crt-static'
& $cargo test --release --manifest-path "$PSScriptRoot/native/Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
& $cargo build --release --manifest-path "$PSScriptRoot/native/Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed.' }
New-Item -ItemType Directory -Force -Path "$PSScriptRoot/dist" | Out-Null
Copy-Item -LiteralPath "$PSScriptRoot/native/target/release/simple-delete.exe" -Destination "$PSScriptRoot/dist/Simple-Delete-Rust.exe" -Force
Write-Host "Built $PSScriptRoot/dist/Simple-Delete-Rust.exe"
