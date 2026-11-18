import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import {
  MonitorError,
  compareSnapshots,
  getMonitor,
  getMonitorStorageDir,
  listMonitors,
  normalizeLimits,
  startMonitor,
  stopMonitor
} from '../lib/monitor.mjs';

function makeSnapshot(overrides = {}) {
  return {
    schemaVersion: 1,
    capturedAt: '2026-11-14T20:00:00.000Z',
    durationMs: 250,
    platform: 'win32',
    limits: {
      maxDepth: 2,
      maxEntries: 4000,
      maxRegistryEntries: 4000,
      maxDurationMs: 15000
    },
    completeness: {
      complete: true,
      truncated: false,
      registryTruncated: false,
      filesystemTruncated: false,
      warningCount: 0,
      errorCount: 0,
      warnings: []
    },
    roots: [
      { key: 'programFiles', path: 'C:\\Program Files', exists: true, scannedEntries: 10, truncated: false, error: null },
      { key: 'localAppData', path: 'C:\\Users\\Tester\\AppData\\Local', exists: true, scannedEntries: 15, truncated: false, error: null },
      { key: 'roamingAppData', path: 'C:\\Users\\Tester\\AppData\\Roaming', exists: true, scannedEntries: 8, truncated: false, error: null }
    ],
    registry: [],
    directories: [],
    ...overrides
  };
}

test('compareSnapshots diffs added, removed, changed, and unchanged registry and directory entries', () => {
  const baseline = makeSnapshot({
    capturedAt: '2026-11-14T20:00:00.000Z',
    registry: [
      {
        id: 'HKLM\\Uninstall\\UnchangedApp',
        hive: 'HKLM-64',
        keyName: 'UnchangedApp',
        name: 'Unchanged App',
        publisher: 'Stable Corp',
        version: '1.0.0',
        installDate: '20260101',
        installLocation: 'C:\\Program Files\\Stable Corp\\Unchanged App',
        estimatedSizeKB: 1024,
        canUninstall: true,
        windowsInstaller: false
      },
      {
        id: 'HKLM\\Uninstall\\UpgradedApp',
        hive: 'HKLM-64',
        keyName: 'UpgradedApp',
        name: 'Upgraded App',
        publisher: 'Acme Labs',
        version: '2.0.0',
        installDate: '20260201',
        installLocation: 'C:\\Program Files\\Acme Labs\\Upgraded App',
        estimatedSizeKB: 2048,
        canUninstall: true,
        windowsInstaller: false
      },
      {
        id: 'HKCU\\Uninstall\\RemovedApp',
        hive: 'HKCU',
        keyName: 'RemovedApp',
        name: 'Removed App',
        publisher: 'Old Soft',
        version: '0.9.0',
        installDate: '20250101',
        installLocation: 'C:\\Users\\Tester\\AppData\\Local\\RemovedApp',
        estimatedSizeKB: 512,
        canUninstall: true,
        windowsInstaller: false
      }
    ],
    directories: [
      {
        id: 'programFiles:stable corp\\unchanged app',
        rootKey: 'programFiles',
        path: 'C:\\Program Files\\Stable Corp\\Unchanged App',
        relativePath: 'Stable Corp\\Unchanged App',
        name: 'Unchanged App',
        depth: 2,
        createdAt: '2026-01-01T00:00:00.000Z',
        modifiedAt: '2026-01-01T00:00:00.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      },
      {
        id: 'programFiles:acme labs\\upgraded app',
        rootKey: 'programFiles',
        path: 'C:\\Program Files\\Acme Labs\\Upgraded App',
        relativePath: 'Acme Labs\\Upgraded App',
        name: 'Upgraded App',
        depth: 2,
        createdAt: '2026-02-01T00:00:00.000Z',
        modifiedAt: '2026-02-01T00:00:00.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      },
      {
        id: 'localAppData:removedapp',
        rootKey: 'localAppData',
        path: 'C:\\Users\\Tester\\AppData\\Local\\RemovedApp',
        relativePath: 'RemovedApp',
        name: 'RemovedApp',
        depth: 1,
        createdAt: '2025-01-01T00:00:00.000Z',
        modifiedAt: '2025-01-01T00:00:00.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      }
    ]
  });

  const final = makeSnapshot({
    capturedAt: '2026-11-14T20:05:00.000Z',
    registry: [
      {
        id: 'HKLM\\Uninstall\\UnchangedApp',
        hive: 'HKLM-64',
        keyName: 'UnchangedApp',
        name: 'Unchanged App',
        publisher: 'Stable Corp',
        version: '1.0.0',
        installDate: '20260101',
        installLocation: 'C:\\Program Files\\Stable Corp\\Unchanged App',
        estimatedSizeKB: 1024,
        canUninstall: true,
        windowsInstaller: false
      },
      {
        id: 'HKLM\\Uninstall\\UpgradedApp',
        hive: 'HKLM-64',
        keyName: 'UpgradedApp',
        name: 'Upgraded App',
        publisher: 'Acme Labs',
        version: '2.1.0',
        installDate: '20261114',
        installLocation: 'C:\\Program Files\\Acme Labs\\Upgraded App',
        estimatedSizeKB: 4096,
        canUninstall: true,
        windowsInstaller: false
      },
      {
        id: 'HKLM\\Uninstall\\WidgetStudio',
        hive: 'HKLM-64',
        keyName: 'WidgetStudio',
        name: 'Widget Studio',
        publisher: 'Northwind Tools',
        version: '3.4.0',
        installDate: '20261114',
        installLocation: '"C:\\Program Files\\Northwind Tools\\Widget Studio\\"',
        estimatedSizeKB: 8192,
        canUninstall: true,
        windowsInstaller: true
      }
    ],
    directories: [
      // Different casing + trailing slash should still match baseline as unchanged
      {
        id: 'programFiles:stable corp\\unchanged app',
        rootKey: 'programFiles',
        path: 'c:\\program files\\stable corp\\unchanged app\\',
        relativePath: 'Stable Corp\\Unchanged App',
        name: 'Unchanged App',
        depth: 2,
        createdAt: '2026-01-01T00:00:00.000Z',
        modifiedAt: '2026-01-01T00:00:00.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      },
      {
        id: 'programFiles:acme labs\\upgraded app',
        rootKey: 'programFiles',
        path: 'C:\\Program Files\\Acme Labs\\Upgraded App',
        relativePath: 'Acme Labs\\Upgraded App',
        name: 'Upgraded App',
        depth: 2,
        createdAt: '2026-02-01T00:00:00.000Z',
        modifiedAt: '2026-11-14T20:03:00.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      },
      {
        id: 'programFiles:northwind tools\\widget studio',
        rootKey: 'programFiles',
        path: 'C:\\Program Files\\Northwind Tools\\Widget Studio',
        relativePath: 'Northwind Tools\\Widget Studio',
        name: 'Widget Studio',
        depth: 2,
        createdAt: '2026-11-14T20:02:00.000Z',
        modifiedAt: '2026-11-14T20:02:30.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      },
      {
        id: 'roamingAppData:northwind tools',
        rootKey: 'roamingAppData',
        path: 'C:\\Users\\Tester\\AppData\\Roaming\\Northwind Tools',
        relativePath: 'Northwind Tools',
        name: 'Northwind Tools',
        depth: 1,
        createdAt: '2026-11-14T20:02:40.000Z',
        modifiedAt: '2026-11-14T20:02:40.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      },
      {
        id: 'localAppData:temp\\cab_1234',
        rootKey: 'localAppData',
        path: 'C:\\Users\\Tester\\AppData\\Local\\Temp\\cab_1234',
        relativePath: 'Temp\\cab_1234',
        name: 'cab_1234',
        depth: 2,
        createdAt: '2026-11-14T20:01:00.000Z',
        modifiedAt: '2026-11-14T20:01:00.000Z',
        attributes: 'Directory',
        isReparsePoint: false
      }
    ]
  });

  const baselineClone = structuredClone(baseline);
  const finalClone = structuredClone(final);

  const result = compareSnapshots(baseline, final, { targetHint: 'Widget Studio' });

  // Purity check: inputs must not be mutated
  assert.deepEqual(baseline, baselineClone);
  assert.deepEqual(final, finalClone);

  assert.equal(result.completeness.complete, true);
  assert.equal(result.completeness.reliable, true);

  assert.deepEqual(result.observedChanges.summary, {
    registryAdded: 1,
    registryRemoved: 1,
    registryChanged: 1,
    registryUnchanged: 1,
    directoriesAdded: 3,
    directoriesRemoved: 1,
    directoriesChanged: 1,
    directoriesUnchanged: 1,
    totalObservedChanges: 8
  });

  assert.equal(result.observedChanges.registry.added[0].name, 'Widget Studio');
  assert.equal(result.observedChanges.registry.removed[0].name, 'Removed App');
  assert.equal(result.observedChanges.registry.changed[0].name, 'Upgraded App');
  assert.deepEqual(
    result.observedChanges.registry.changed[0].changes.map((c) => c.field),
    ['version', 'installDate', 'estimatedSizeKB']
  );

  // Attribution must distinguish observed correlation from causal proof
  assert.equal(result.attributionAnalysis.provenCausalAttribution, false);
  assert.equal(result.attributionAnalysis.provenCount, 0);
  assert.equal(result.attributionAnalysis.summary.provenCount, 0);

  const byPath = new Map(
    result.attributionAnalysis.directoryAssociations.map((a) => [a.path, a])
  );

  const widgetDir = byPath.get('C:\\Program Files\\Northwind Tools\\Widget Studio');
  assert.equal(widgetDir.confidence, 'strong-path-match');
  assert.equal(widgetDir.proven, false);
  assert.equal(widgetDir.correlationOnly, true);
  assert.deepEqual(widgetDir.matchedCandidateIds, ['HKLM\\Uninstall\\WidgetStudio']);

  const roamingVendorDir = byPath.get('C:\\Users\\Tester\\AppData\\Roaming\\Northwind Tools');
  assert.equal(roamingVendorDir.confidence, 'name-or-publisher-match');
  assert.equal(roamingVendorDir.proven, false);

  const tempDir = byPath.get('C:\\Users\\Tester\\AppData\\Local\\Temp\\cab_1234');
  assert.equal(tempDir.confidence, 'unattributed');
  assert.equal(tempDir.proven, false);
  assert.deepEqual(tempDir.matchedCandidateIds, []);
});

test('compareSnapshots prevents overly broad root InstallLocation from claiming strong-path-match', () => {
  const baseline = makeSnapshot();
  const final = makeSnapshot({
    registry: [
      {
        id: 'HKLM\\Uninstall\\BroadLocApp',
        hive: 'HKLM-64',
        keyName: 'BroadLocApp',
        name: 'BroadLocApp',
        publisher: 'Generic',
        version: '1.0',
        installLocation: 'C:\\Program Files\\',
        canUninstall: true,
        windowsInstaller: false
      }
    ],
    directories: [
      {
        id: 'programFiles:unrelated vendor\\other app',
        rootKey: 'programFiles',
        path: 'C:\\Program Files\\Unrelated Vendor\\Other App',
        relativePath: 'Unrelated Vendor\\Other App',
        name: 'Other App',
        depth: 2,
        modifiedAt: '2026-11-14T20:01:00.000Z'
      }
    ]
  });

  const result = compareSnapshots(baseline, final);
  const assoc = result.attributionAnalysis.directoryAssociations[0];
  assert.equal(assoc.confidence, 'unattributed');
  assert.equal(assoc.proven, false);
});

test('compareSnapshots propagates completeness warnings and marks truncated or mismatched snapshots unreliable', () => {
  const baseline = makeSnapshot({
    limits: { maxDepth: 2, maxEntries: 100, maxRegistryEntries: 100, maxDurationMs: 5000 },
    completeness: {
      complete: false,
      truncated: true,
      registryTruncated: false,
      filesystemTruncated: true,
      warningCount: 2,
      errorCount: 1,
      warnings: [
        { code: 'ACCESS_DENIED', scope: 'filesystem', path: 'C:\\Program Files\\WindowsApps', message: 'Access denied' },
        { code: 'ENTRY_LIMIT_REACHED', scope: 'filesystem', path: null, message: 'Entry limit reached' }
      ]
    }
  });

  const final = makeSnapshot({
    limits: { maxDepth: 1, maxEntries: 100, maxRegistryEntries: 100, maxDurationMs: 5000 },
    completeness: {
      complete: true,
      truncated: false,
      registryTruncated: false,
      filesystemTruncated: false,
      warningCount: 0,
      errorCount: 0,
      warnings: []
    }
  });

  const result = compareSnapshots(baseline, final);
  assert.equal(result.completeness.complete, false);
  assert.equal(result.completeness.reliable, false);
  assert.equal(result.completeness.baselineComplete, false);
  assert.equal(result.completeness.finalComplete, true);

  const codes = result.completeness.warnings.map((w) => w.code);
  assert.ok(codes.includes('ACCESS_DENIED'));
  assert.ok(codes.includes('ENTRY_LIMIT_REACHED'));
  assert.ok(codes.includes('SNAPSHOT_TRUNCATED_DIFF_INCOMPLETE'));
  assert.ok(codes.includes('SNAPSHOT_DEPTH_MISMATCH'));
});

test('getMonitorStorageDir defaults to LOCALAPPDATA/SimpleDelete/monitors and normalizeLimits clamps bounds', () => {
  const prev = process.env.LOCALAPPDATA;
  try {
    process.env.LOCALAPPDATA = 'C:\\Users\\Example\\AppData\\Local';
    assert.equal(
      getMonitorStorageDir(),
      path.join('C:\\Users\\Example\\AppData\\Local', 'SimpleDelete', 'monitors')
    );
  } finally {
    if (prev === undefined) delete process.env.LOCALAPPDATA;
    else process.env.LOCALAPPDATA = prev;
  }

  assert.deepEqual(
    normalizeLimits({ maxDepth: 99, maxEntries: 1, maxRegistryEntries: 999999, maxDurationMs: 100 }),
    {
      maxDepth: 4,
      maxEntries: 10,
      maxRegistryEntries: 10000,
      maxDurationMs: 500
    }
  );
});

test('startMonitor, listMonitors, getMonitor, and stopMonitor persist baseline/final snapshots and comparison in isolated storage', async (t) => {
  const tempDataDir = await mkdtemp(path.join(os.tmpdir(), 'simple-delete-monitor-'));
  t.after(async () => {
    await rm(tempDataDir, { recursive: true, force: true });
  });

  // Empty storage lists cleanly
  assert.deepEqual(await listMonitors({ dataDir: tempDataDir }), []);
  assert.equal(await getMonitor('missing_id', { dataDir: tempDataDir }), null);

  let callCount = 0;
  const fixtureSnapshots = [
    makeSnapshot({
      capturedAt: '2026-11-14T20:10:00.000Z',
      registry: [
        { id: 'HKLM\\Uninstall\\Base', hive: 'HKLM-64', keyName: 'Base', name: 'Base App', version: '1.0' }
      ],
      directories: [
        { rootKey: 'programFiles', path: 'C:\\Program Files\\Base App', relativePath: 'Base App', name: 'Base App', depth: 1, modifiedAt: '2026-01-01T00:00:00.000Z' }
      ]
    }),
    makeSnapshot({
      capturedAt: '2026-11-14T20:12:00.000Z',
      completeness: {
        complete: false,
        truncated: false,
        registryTruncated: false,
        filesystemTruncated: false,
        warningCount: 1,
        errorCount: 1,
        warnings: [{ code: 'ACCESS_DENIED', scope: 'filesystem', path: 'C:\\Program Files\\Locked', message: 'Denied' }]
      },
      registry: [
        { id: 'HKLM\\Uninstall\\Base', hive: 'HKLM-64', keyName: 'Base', name: 'Base App', version: '1.0' },
        { id: 'HKLM\\Uninstall\\Demo', hive: 'HKLM-64', keyName: 'Demo', name: 'Demo Tool', publisher: 'Demo Inc', version: '1.2.3', installLocation: 'C:\\Program Files\\Demo Tool' }
      ],
      directories: [
        { rootKey: 'programFiles', path: 'C:\\Program Files\\Base App', relativePath: 'Base App', name: 'Base App', depth: 1, modifiedAt: '2026-01-01T00:00:00.000Z' },
        { rootKey: 'programFiles', path: 'C:\\Program Files\\Demo Tool', relativePath: 'Demo Tool', name: 'Demo Tool', depth: 1, modifiedAt: '2026-11-14T20:11:00.000Z' }
      ]
    })
  ];

  const snapshotFn = async () => fixtureSnapshots[callCount++];

  const started = await startMonitor({
    id: 'demo_install_1',
    label: 'Install Demo Tool',
    targetHint: 'Demo Tool',
    dataDir: tempDataDir,
    snapshotFn
  });

  assert.equal(started.id, 'demo_install_1');
  assert.equal(started.status, 'active');
  assert.equal(started.baselineSummary.registryCount, 1);
  assert.equal(started.baselineSummary.directoryCount, 1);

  // Duplicate ID must be rejected
  await assert.rejects(
    () => startMonitor({ id: 'demo_install_1', dataDir: tempDataDir, snapshotFn }),
    (err) => err instanceof MonitorError && err.code === 'MONITOR_ALREADY_EXISTS'
  );

  // Corrupt directory in storage should not break listMonitors
  await mkdir(path.join(tempDataDir, 'monitors', 'corrupt_dir'), { recursive: true });
  await writeFile(path.join(tempDataDir, 'monitors', 'corrupt_dir', 'monitor.json'), '{not-json', 'utf8');

  const activeList = await listMonitors({ dataDir: tempDataDir });
  assert.equal(activeList.length, 1);
  assert.equal(activeList[0].status, 'active');

  const activeLoaded = await getMonitor('demo_install_1', { dataDir: tempDataDir });
  assert.equal(activeLoaded.status, 'active');
  assert.equal(activeLoaded.comparison, undefined);

  const stopped = await stopMonitor('demo_install_1', { dataDir: tempDataDir, snapshotFn });
  assert.equal(stopped.status, 'stopped');
  assert.equal(stopped.stoppedAt, '2026-11-14T20:12:00.000Z');
  assert.equal(stopped.comparisonSummary.registryAdded, 1);
  assert.equal(stopped.comparisonSummary.directoriesAdded, 1);
  assert.equal(stopped.attributionSummary.strongPathMatchCount, 1);
  assert.equal(stopped.attributionSummary.provenCount, 0);
  assert.equal(stopped.completeness.complete, false);
  assert.equal(stopped.completeness.reliable, true);

  // Stopping twice must fail cleanly
  await assert.rejects(
    () => stopMonitor('demo_install_1', { dataDir: tempDataDir, snapshotFn }),
    (err) => err instanceof MonitorError && err.code === 'MONITOR_ALREADY_STOPPED'
  );

  // Unknown stop must fail cleanly
  await assert.rejects(
    () => stopMonitor('nonexistent_monitor', { dataDir: tempDataDir, snapshotFn }),
    (err) => err instanceof MonitorError && err.code === 'MONITOR_NOT_FOUND'
  );

  // Path traversal id must fail cleanly
  await assert.rejects(
    () => getMonitor('../escape', { dataDir: tempDataDir }),
    (err) => err instanceof MonitorError && err.code === 'INVALID_MONITOR_ID'
  );

  const loadedStopped = await getMonitor('demo_install_1', { dataDir: tempDataDir, includeSnapshots: true });
  assert.equal(loadedStopped.status, 'stopped');
  assert.equal(loadedStopped.comparison.observedChanges.registry.added[0].name, 'Demo Tool');
  assert.equal(loadedStopped.baselineSnapshot.registry.length, 1);
  assert.equal(loadedStopped.finalSnapshot.registry.length, 2);
});

test('startMonitor leaves no monitor directory if baseline snapshot fails', async (t) => {
  const tempDataDir = await mkdtemp(path.join(os.tmpdir(), 'simple-delete-monitor-fail-'));
  t.after(async () => {
    await rm(tempDataDir, { recursive: true, force: true });
  });

  await assert.rejects(
    () => startMonitor({
      id: 'failed_baseline',
      dataDir: tempDataDir,
      snapshotFn: async () => {
        throw new MonitorError('SNAPSHOT_EXECUTION_FAILED', 'Simulated failure');
      }
    }),
    (err) => err instanceof MonitorError && err.code === 'SNAPSHOT_EXECUTION_FAILED'
  );

  assert.deepEqual(await listMonitors({ dataDir: tempDataDir }), []);
});
