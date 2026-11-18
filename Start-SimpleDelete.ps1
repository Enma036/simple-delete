$ErrorActionPreference = 'Stop'
$packaged = Join-Path $PSScriptRoot 'dist/Simple-Delete-Rust.exe'
if (-not (Test-Path -LiteralPath $packaged)) { throw 'The Rust app has not been built. Run Build-Rust.ps1 first.' }
Start-Process -FilePath $packaged
