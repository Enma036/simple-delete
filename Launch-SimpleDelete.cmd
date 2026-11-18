@echo off
setlocal
set ELECTRON_RUN_AS_NODE=
cd /d "%~dp0"
if exist "dist\Simple-Delete-Rust.exe" (
  start "" "dist\Simple-Delete-Rust.exe"
  exit /b 0
)
echo The Rust app has not been built. Run Build-Rust.ps1 or use dist\Simple-Delete-Rust.exe.
pause
