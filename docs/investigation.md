# REA investigation and implementation boundaries

Target: locally installed `C:\Program Files\VS Revo Group\Revo Uninstaller\RevoUnin.exe`.

REA 6.9.0 ran the `inspect-pe-resources` operation through its CLI. The raw resource inventory was retained locally.

Evidence ID: `ev_6b543b1726f0a814934fc9c80e543871dbf7606ed4b39438eb46487176308e39`.

Observed: a PE32+ x64 executable, 21,853,984 bytes, SHA-256 `e4a69fee0105698736177ee9cc1772699cf92f7650ec1dcb0f78f2baae65b46e`, with native resource directories including AFX dialog layouts. Resource inventory is static evidence only. No code was decompiled, no Revo process was launched, and no proprietary assets were copied into the UI.

Implemented independently: Windows registry inventory, in-memory cache, browser-side search/sort/pagination, registered-uninstaller launch with confirmation, and session launch activity. The interface and caching strategy are design choices, not recovered Revo internals.

Unresolved: Revo's leftover matching, monitored installation algorithms, runtime behavior, and comparative speed. No parity or faster-than-Revo claim is established.

The expanded implementation adds independently designed exact-name leftover review, same-volume directory quarantine, exported-registry backups and restoration, stale uninstall-registration removal, explicit forced cleanup, current-user Store package removal, user Run-entry disable/restore, Windows restore-point requests, and bounded installation snapshot comparison. These are design choices and Windows API integrations, not findings recovered from Revo's engine. Snapshot monitoring and its limits are described in `monitoring.md`.

Read-only measurements on this machine: 126 registry entries, 1,048 ms registry scan, 1,181 ms first HTTP request, 18 ms cached request. These are single-run measurements, not a controlled benchmark. App sizes are installer estimates, not measured disk usage. MSI command conversion, quoted paths, and rejection of shell/relative commands are covered by automated tests. No uninstall was executed during validation.

Expanded validation: 17 automated tests cover monitoring diffs and persistence, quarantine/restore using temporary fixture folders, no-overwrite restoration, junction and path boundaries, protected registration scope, and HTTP session/host checks. Read-only Windows smoke checks returned 12 Run entries, 226 Store packages (94 eligible for removal), and 126 uninstall identities plus 100 directories from a deliberately limited snapshot. Its truncation warning was preserved. No real application, registry key, Store package or startup entry was removed during testing. UI scripts passed syntax checks; visual browser validation and privileged mutation validation were not performed.
