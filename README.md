<img src="assets/icon.svg" width="64" height="64" alt="">

# Simple Delete

An independent Windows application manager inspired by the app-removal workflow in Revo Uninstaller. A native desktop interface, instant local search, application discovery across several sources, and explicit confirmation before removal.

## Run

Double-click **`dist/Simple-Delete-Rust.exe`** on Windows 10/11 x64. Version 0.4.2 is a standalone native Rust desktop app with an egui window. It needs no browser, Chromium, Node.js, Rust installation, or local web server. You can copy this one executable elsewhere. `Launch-SimpleDelete.cmd` and `Start-SimpleDelete.ps1` open the copy in `dist`.

The executable is unsigned. Protected operations require launching it with **Run as administrator**. Startup failures are logged to `%LOCALAPPDATA%\SimpleDelete\logs\native-startup.log` (panics use `native-panic.log`).

To build from source, use the Rust GNU toolchain and MinGW-w64, then run `Build-Rust.ps1`. The script runs optimized tests, builds the executable and copies it to `dist`. Source is in `native/`; dependencies are locked in `native/Cargo.lock`. The previous JS/Electron implementation is retained as reference and is not part of the Rust executable.

The library and startup inventory use native Windows registry reads. Search results are cached, rows are virtualized, and blocking operations run off the UI thread. Store package management and restore-point creation use Windows PowerShell internally, with no terminal window. `Simple-Delete-Rust.exe --diagnostics report.json` runs read-only collector checks and writes results.

## Included

- Per-user and machine registry inventory, including 32-bit entries.
- Unified discovery of desktop registrations, Microsoft Store packages, Steam library manifests and executable-containing folders under the current user's `AppData/Local/Programs`. Coverage is bounded; arbitrary portable folders and other users' profiles are not exhaustively scanned.
- Source filters and app details with publisher, version, installation location, source and inferred category. Store system components are hidden by default in the combined library; enable **Show Store system components** to include them.
- **Suggestions** ranks large apps and distinct registered versions with explicit reasons. It uses local rules, excludes recognized system/shared/driver/security components, and does not measure usage or guarantee that removal is appropriate. **Keep this app** choices persist locally and can be reset.
- Cached inventory; explicit refresh after an uninstall finishes.
- App/publisher search, size and date sorting, large-app filter, virtualized scrolling.
- Confirmed registered-uninstaller launch and session activity.
- **Check again** on Cleanup retains the original app identity and install location after an uninstaller hands off. Cleanup stays disabled while registration remains, unless the advanced override is explicitly enabled.
- Per-item cleanup results distinguish backed-up items from failures, with a fresh-scan retry path and direct access to Recovery. Recovery metadata updates are flushed and atomically replaced.
- Native desktop layout and modal confirmation dialogs.
- Actual application icons from local Windows resources and Store package logos, loaded asynchronously and cached. Custom category icons remain the fallback when artwork is unavailable.
- Leftover review: bounded vendor/product folder and settings-file discovery to depth two in AppData, ProgramData and Program Files, recorded install folders within those roots, and direct/nested current-user product registry keys. Recorded folders can differ from the app display name. Coverage, match reasons and partial scan errors are shown. Select candidates explicitly before cleanup.
- **Complete Clean Up**, at the top of the sidebar, scans a selected fixed drive from its root and skips protected paths, links, and other user profiles. **Auto Clean** moves up to 1,000 eligible `.tmp` and `.temp` files from your Windows user Temp folder to **Recovery** in one run. Files must be at least 30 days old. Auto Clean skips files you previously restored through Recovery. **Scan for review** lets you individually select older documents, media, archives, and installer packages using a 90-, 180-, or 365-day cutoff. Age uses the newest recorded creation, modification, or access timestamp. Recovery restores files to their original paths, and its backups still occupy space on the scanned drive.
- Leftover recovery: selected folders and files move into a same-volume `.simple-delete-quarantine` directory. Selected registry keys are exported before removal. Restore from **Recovery**. File moves refuse existing destinations atomically.
- Forced cleanup for selected candidates of installed apps, with a separate acknowledgment. **Remove entry** backs up and removes a broken app-list registration; this does not uninstall binaries, services or drivers.
- Store / MSIX removal for the current user, with non-removable, framework and resource packages protected.
- Startup management: view user/machine registry Run entries, disable current-user entries with a recoverable backup.
- Windows restore-point creation, verified by checking for a new restore-point sequence after the request.

Installers may request elevation themselves. Protected locations and restore points require running the application as administrator; errors are surfaced without automatic elevation. Windows System Protection must be enabled for restore points, and Windows frequency limits apply. Launch success does not mean removal completed. Sizes and dates are installer-reported.

This is an independently implemented application manager, not full Revo feature parity. Leftover cleanup uses conservative exact names, excludes junctions and shared app locations, and cannot prove ownership. Quarantine retains disk space until backups are removed manually; there is no automatic purge. Startup-folder shortcuts, services, scheduled tasks, recursive registry searches and driver removal are outside this version. It has not been benchmarked against Revo.

Backups are stored under `%LOCALAPPDATA%\SimpleDelete\native-backups`, outside the repository. Complete Clean Up stores files under `native-backups\drive-files` when they originate on the same drive as the backup folder. Files from other drives and app leftovers stay in quarantine beside their original locations. Keep these locations for restoration. Legacy JS backups remain separate. Confirmation is disabled while a background job is active; local search remains available.

REA evidence and the reconstruction boundary are documented in [docs/investigation.md](docs/investigation.md).
