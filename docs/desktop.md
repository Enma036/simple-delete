# Windows desktop packaging

The old launcher required a developer environment and a separate browser. Version 0.3 bundles an Electron runtime, app source and PowerShell collectors in one portable Windows x64 executable. Runtime code is packed into an ASAR archive; PowerShell scripts are unpacked separately for Windows execution. The app resolves collector paths appropriately in both development and packaged runs.

`desktop/main.mjs` owns the window and backend lifecycle, uses an automatically assigned loopback port, enforces a single instance, disables Node access in the renderer, denies additional windows/permissions and external navigation, and logs startup errors outside the installation folder. The development launcher clears inherited `ELECTRON_RUN_AS_NODE` to ensure the runtime opens a desktop window. Registration confirmation uses an app dialog because Electron does not support JavaScript `prompt()`.

Build with `npm ci` and `npm run build`. Dependency versions are recorded in `package-lock.json`. Build-only minimatch overrides resolve the advisories reported by npm for the packaging toolchain; npm audit reported zero findings after installation. The portable executable is unsigned.

Validation: all 17 Node tests pass. The real desktop renderer and packaged ASAR app both loaded 126 installed applications and passed checks for local search, cleanup/startup/Store/monitor/recovery panel rendering, and returning to the app library. The test does not remove software or alter real registry/startup entries. Collector execution and privileged removal remain subject to Windows permissions. Use `--smoke-test` to run the read-only renderer checks without displaying the window; results default to the app log directory, or `SIMPLE_DELETE_SMOKE_RESULT` when supplied.
