# Leftover cleaner validation

Validated on Windows, 2026-11-15. No installed applications were removed, and no real registry keys were changed.

## Fixes

- A recorded install directory was previously missed when its basename differed from the display name, such as `AcmeTool 2026 (64-bit)` installed in `Vendor/AcmeTool`. Matching app context now includes that recorded directory as an unchecked review candidate, while preserving root, generic-name, shared-location and reparse safeguards.
- Directory-entry inspection failures and registry enumeration errors are reported. Empty-result text now describes scan coverage and warns when locations could not be checked.
- Registry backup export must succeed before a prepared Recovery record is saved. The original key is deleted only after export and metadata persistence succeed.

## Verification

The optimized Rust suite includes filesystem and mocked-registry checks for:

- Versioned display names and nested recorded install folders; root, generic and outside-root rejection.
- Actual quarantine and byte-for-byte restoration of nested binary file data.
- Refusal to overwrite an existing restore destination or replay a completed operation.
- Active-app protection, shared-location protection, and a different app claiming the location after scanning.
- Invalid/empty selections, expired plans, and fresh-scan retry after partial cleanup.
- A failed registry export preserving the original key, reporting failure separately from successful filesystem cleanup, and not advertising a nonexistent backup.
- Directory traversal bounds and reparse/link exclusions. Symlink creation coverage depends on Windows privileges.

`native/examples/verify_cleanup.rs` also exercises the public production APIs on Windows. It creates a unique fixture folder under LocalAppData, scans it, quarantines it, verifies its Recovery listing and bytes, restores it, rejects repeated restore, and removes only its own test files and backup metadata. This run returned `CLEANER_E2E_OK`. Installed-app registry inventory is read, but registry export/deletion is not invoked by this example.

To repeat that isolated check with the configured Rust toolchain:

```powershell
cargo run --release --manifest-path native/Cargo.toml --example verify_cleanup
```

## Coverage limits

This is a candidate scanner, not exhaustive installation tracing. It checks exact normalized names at the configured roots, matching direct current-user software keys, and the original app's recorded install location inside approved roots. Publisher-nested settings folders without recorded location evidence, arbitrary drives, services, tasks and deep registry remnants can remain. Candidates may contain personal preferences or user files and require review. Quarantine preserves recoverability and continues to occupy disk space.
