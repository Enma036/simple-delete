# Revo investigation and independent improvements

Target: installed `C:\Program Files\VS Revo Group\Revo Uninstaller\RevoUnin.exe`.

The REA 6.9.0 import inventory, evidence ID `ev_b5e990a5e383487e2b1e0b7cba83632473ebf639f50d984c3a0fa84d9449a16d`, has SHA-256 `e4a69fee0105698736177ee9cc1772699cf92f7650ec1dcb0f78f2baae65b46e`, matching the earlier [resource evidence](investigation.md). It declares registry deletion imports including `SHDeleteKeyW` and `SHDeleteValueW`, process enumeration and MSI imports. Imports are static declarations, not evidence of which workflow calls them.

Ghidra 12.1.4 was used for a CLI string/reference trace and function dossier. The [deep investigation](revo-rea/deep/status.md) summarizes both records. It traces uninstall UI configuration and a worker-thread launch, and identifies scan-mode and selective-recovery descriptions. Matching, deletion and recovery algorithms remain unknown. No runtime equivalence or full feature parity is claimed. No proprietary code or artwork is reused.

## Implemented independently

- Filesystem search now checks two levels under the configured application-data/program roots: direct matches and vendor/product matches. It avoids reparse traversal, protected generic containers, its own data and quarantine. Exact-name matching remains required. A matched app folder is not recursively offered as overlapping child candidates.
- Settings files with exact matching stems and extensions `log`, `ini`, `json`, `db`, `sqlite`, `cfg`, `dat`, `cache` or `xml` can be reviewed separately. Documents and partial name matches are excluded.
- Registry search checks direct keys and vendor/product keys under current-user Software. Nested cleanup exports and removes only the selected product key, retaining vendor siblings. Protected system parents and deeper paths are rejected. Machine hives and individual registry values remain outside leftover discovery.
- Each filesystem/registry traversal checks an entry budget of 5,000 and elapsed-time budget of five seconds. Windows calls are synchronous and an individual call can exceed that budget; limits are cooperative, not hard timeouts. Truncation and access failures remain visible. Recorded install locations are checked separately.
- Review shows scan coverage, type and evidence reasons. Candidates begin unchecked and require review, acknowledgment and confirmation. Backup and restore support individual files as well as directories and registry keys. Moves refuse existing destinations atomically.

## Remaining differences and unknowns

This does not reproduce Revo's algorithms. Publisher identity aliases, machine/32-bit registry value cleanup, services, scheduled tasks, shortcut discovery, permanent backup purge and process-attributed install tracing are not implemented by this change. Monitoring remains a bounded snapshot comparison. Arbitrary drives, deep settings paths and unknown portable installations can be missed. Quarantine preserves recoverability and occupies disk space. Unknown ownership is stated for every heuristic candidate.

Vendor parents with at least two normalized characters (for example `IBM`, `HP` and `EA`) can now be traversed. The product must still have an exact matching name of at least four normalized characters. Short vendor parents are never selected as cleanup candidates on their own, and protected containers remain excluded. This fixes a coverage gap independently of Revo's unknown matching algorithm.

## Verification

Fixture checks cover nested vendor/product folders, untouched similarly named siblings and documents, protected containers, file and registry restoration, entry-limit warnings, changed file-type rejection, no-overwrite moves and prior active/shared-location safeguards. The public-API Windows example verifies only its newly created directory and settings file; it does not uninstall applications or mutate registry keys.

The configured-engine pass adds fixtures for short vendor parents, restoring one item while another remains quarantined, and failed restoration of missing backup data or corrupt metadata. Failed restoration must not mark the item restored or alter the restored sibling. These validate our independent implementation; they do not establish equivalence to Revo.
