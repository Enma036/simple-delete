# Installation Monitoring

Simple Delete provides bounded, read-only installation monitoring through [`lib/monitor.mjs`](../lib/monitor.mjs) and [`scripts/snapshot.ps1`](../scripts/snapshot.ps1).

## Workflow

1. **Start monitoring (`startMonitor`)**: Captures an explicit baseline snapshot of Windows uninstall registry identity and depth-limited directory metadata, persists it outside git under `%LOCALAPPDATA%\SimpleDelete\monitors\<id>\`, and records the monitor as `active`.
2. **User runs installation externally**: Simple Delete does **not** launch installers, delete files, or mutate the Windows registry.
3. **Stop monitoring (`stopMonitor`)**: Captures an explicit final snapshot using the same bounds, computes a pure diff via `compareSnapshots(baseline, final)`, persists `final.json` and `comparison.json`, and updates the monitor status to `stopped`.
4. **Inspect monitors (`listMonitors`, `getMonitor`)**: Lists persisted sessions or loads a monitor record with its comparison report (and optionally raw `baselineSnapshot` / `finalSnapshot`).

## Persistent Storage

Monitor state is stored outside the repository at:

```text
%LOCALAPPDATA%\SimpleDelete\monitors\<monitorId>\
  ├── monitor.json      # Session metadata, bounds, summaries, completeness status
  ├── baseline.json     # Explicit baseline snapshot at startMonitor()
  ├── final.json        # Explicit final snapshot at stopMonitor()
  └── comparison.json   # Observed diff + attribution analysis
```

## Snapshot Scope & Bounds

Each snapshot runs `scripts/snapshot.ps1` read-only with bounded limits (clamped by `normalizeLimits`):

- **Registry Uninstall Identity**: `HKLM` (64-bit and `WOW6432Node`) and `HKCU` `Uninstall` keys (`DisplayName` present, excluding `SystemComponent = 1`).
- **Filesystem Directory Metadata**: Breadth-first traversal across deduplicated Windows roots (`ProgramFiles`, `ProgramFiles(x86)`, `ProgramW6432`, `LOCALAPPDATA`, `APPDATA`, `ProgramData`) up to `maxDepth` (default `2`, max `4`).
- **Safety & Bounds Defaults**:
  - `maxDepth`: `2` (range `1..4`)
  - `maxEntries`: `4000` directory entries (range `10..20000`)
  - `maxRegistryEntries`: `4000` registry entries (range `10..10000`)
  - `maxDurationMs`: `15000` ms (range `500..60000`; PowerShell process timeout is `maxDurationMs + 5000` ms)
- **Reparse Points**: Junctions and symbolic links (e.g., Windows `Application Data` compatibility junctions) are recorded with `isReparsePoint: true` and are **not** traversed recursively.
- **Completeness Warnings**: Windows errors (`ACCESS_DENIED`, `SECURITY_ERROR`, `PATH_TOO_LONG`, `REGISTRY_ACCESS_DENIED`, `ROOT_NOT_FOUND`) and bound stops (`ENTRY_LIMIT_REACHED`, `REGISTRY_ENTRY_LIMIT_REACHED`, `RUNTIME_BUDGET_EXCEEDED`) are captured in `completeness.warnings` instead of crashing or silently hiding partial coverage.

## Observed Changes vs. Proven App Attribution

`compareSnapshots` explicitly separates **observed state changes** from **causal proof**:

- **`observedChanges`**: Exact added, removed, and modified entries between the baseline and final snapshots for both `registry` and `directories`.
- **`attributionAnalysis`**:
  - `provenCausalAttribution` is always `false` and `provenCount` is always `0`.
  - Candidate apps are derived from added or changed uninstall registry entries (`evidenceLevel: 'registry-entry-observed'`).
  - Directory changes are heuristically grouped as:
    - `strong-path-match`: directory path equals or lies inside a candidate app's specific `InstallLocation` (excluding broad root directories such as `C:\Program Files`).
    - `name-or-publisher-match`: directory path/name matches a candidate app's name, publisher, parent of `InstallLocation`, or optional `targetHint`.
    - `unattributed`: no heuristic match to observed candidate apps or `targetHint`.
  - Every association carries `proven: false` and `correlationOnly: true`.

## Snapshot Monitoring Limitations

1. **No Kernel or Process-Level Tracing**: Snapshot diffing compares machine state at two points in time. It does **not** install a filesystem minifilter driver, registry callback, or ETW/process-creation tracer. Any concurrent system activity during the monitoring window—Windows Update, browser background updates, scheduled tasks, antivirus scans, or temporary file unpacking—appears in `observedChanges` and cannot be causally disproven or proven to belong to a specific installer.
2. **Depth-Limited Directory Visibility**: Default traversal scans directory metadata up to depth `2` under AppData and Program Files roots and does not hash or inventory individual files inside deep subdirectories. Modifications deeper than `maxDepth` (without modifying a depth `<= maxDepth` directory's metadata) and changes outside the scanned roots (such as `System32`, driver stores, Start Menu shortcuts, Scheduled Tasks, Services, or arbitrary custom drives) are not captured.
3. **Bounded Coverage & Truncation Risk**: If `maxEntries`, `maxRegistryEntries`, or `maxDurationMs` is exceeded, `completeness.truncated` is `true` and `compareSnapshots` marks `completeness.reliable = false` with `SNAPSHOT_TRUNCATED_DIFF_INCOMPLETE`. An entry skipped in a truncated baseline but reached in a non-truncated final snapshot can look like an addition even when it already existed.
4. **Permissions & Elevation Boundaries**: Running unprivileged prevents reading restricted directories (for example `WindowsApps` or another user's profile) or protected registry keys. Such gaps are surfaced as `ACCESS_DENIED` warnings in `completeness.warnings`.
5. **Installer Metadata Assumptions**:
   - Registry entries with `SystemComponent = 1` or without `DisplayName`, as well as Microsoft Store / MSIX packages outside classic `Uninstall` keys, are not included in registry identity.
   - `InstallLocation` is installer-reported and frequently omitted, inaccurate, or pointed at a shared parent folder.
6. **NTFS Timestamp Granularity**: Directory `modifiedAt` updates when immediate children are added, renamed, or removed, but file content overwrites inside an existing child file do not update parent directory timestamps.
