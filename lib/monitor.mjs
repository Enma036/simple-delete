import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { access, mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);

const MODULE_DIR = path.dirname(fileURLToPath(import.meta.url));
const SNAPSHOT_SCRIPT_PATH = path.resolve(MODULE_DIR, '..', 'scripts', 'snapshot.ps1').replace(/app\.asar([\\/])/,'app.asar.unpacked$1');

const DEFAULT_LIMITS = Object.freeze({
  maxDepth: 2,
  maxEntries: 4000,
  maxRegistryEntries: 4000,
  maxDurationMs: 15000
});

const MONITOR_ID_PATTERN = /^[a-zA-Z0-9_-]{1,128}$/;

const GENERIC_ATTRIBUTION_TOKENS = new Set([
  'app',
  'apps',
  'software',
  'program',
  'programs',
  'files',
  'windows',
  'microsoft',
  'setup',
  'install',
  'installer',
  'update',
  'updater',
  'inc',
  'llc',
  'ltd',
  'corp',
  'corporation',
  'gmbh',
  'the',
  'for',
  'and',
  'with',
  'x64',
  'x86',
  'win32',
  'win64',
  'version',
  'edition',
  'client',
  'desktop',
  'runtime'
]);

const REGISTRY_COMPARE_FIELDS = [
  'name',
  'publisher',
  'version',
  'installDate',
  'installLocation',
  'estimatedSizeKB',
  'canUninstall',
  'windowsInstaller',
  'hive',
  'keyName'
];

const DIRECTORY_COMPARE_FIELDS = [
  'modifiedAt',
  'createdAt',
  'attributes',
  'isReparsePoint',
  'depth',
  'childDirCount',
  'childFileCount'
];

export class MonitorError extends Error {
  constructor(code, message, options = {}) {
    super(message, options.cause ? { cause: options.cause } : undefined);
    this.name = 'MonitorError';
    this.code = code;
    if (options.details !== undefined) {
      this.details = options.details;
    }
  }
}

function asArray(value) {
  if (Array.isArray(value)) return value;
  if (value === null || value === undefined) return [];
  return [value];
}

function clampInt(value, fallback, min, max) {
  if (value === undefined || value === null) return fallback;
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(max, Math.max(min, Math.trunc(parsed)));
}

export function normalizeLimits(options = {}) {
  return {
    maxDepth: clampInt(options.maxDepth, DEFAULT_LIMITS.maxDepth, 1, 4),
    maxEntries: clampInt(options.maxEntries, DEFAULT_LIMITS.maxEntries, 10, 20000),
    maxRegistryEntries: clampInt(options.maxRegistryEntries, DEFAULT_LIMITS.maxRegistryEntries, 10, 10000),
    maxDurationMs: clampInt(options.maxDurationMs, DEFAULT_LIMITS.maxDurationMs, 500, 60000)
  };
}

export function getMonitorStorageDir(options = {}) {
  if (typeof options.storageDir === 'string' && options.storageDir.trim().length > 0) {
    return path.resolve(options.storageDir);
  }
  if (typeof options.dataDir === 'string' && options.dataDir.trim().length > 0) {
    return path.join(path.resolve(options.dataDir), 'monitors');
  }
  const localAppData = process.env.LOCALAPPDATA && process.env.LOCALAPPDATA.trim().length > 0
    ? process.env.LOCALAPPDATA
    : path.join(os.homedir(), 'AppData', 'Local');
  return path.join(localAppData, 'SimpleDelete', 'monitors');
}

function validateMonitorId(id) {
  if (typeof id !== 'string' || !MONITOR_ID_PATTERN.test(id)) {
    throw new MonitorError(
      'INVALID_MONITOR_ID',
      'Monitor id must be a non-empty alphanumeric/hyphen/underscore string (max 128 chars).'
    );
  }
  return id;
}

function normalizeWindowsPath(rawPath) {
  if (typeof rawPath !== 'string') return '';
  let normalized = rawPath.trim().replace(/^["']+|["']+$/g, '').replaceAll('/', '\\');
  if (!normalized) return '';
  if (/^[a-zA-Z]:\\$/.test(normalized)) {
    return normalized.toLowerCase();
  }
  normalized = normalized.replace(/\\+$/, '');
  if (/^[a-zA-Z]:$/.test(normalized)) {
    return `${normalized.toLowerCase()}\\`;
  }
  return normalized.toLowerCase();
}

function normalizeRegistryEntry(raw) {
  if (!raw || typeof raw !== 'object') return null;
  const hive = raw.hive ?? null;
  const keyName = raw.keyName ?? null;
  const name = raw.name ?? null;
  const id = raw.id
    ? String(raw.id)
    : `${hive ?? 'UNKNOWN'}::${keyName ?? name ?? ''}`;

  return {
    id,
    hive: hive !== null ? String(hive) : null,
    keyName: keyName !== null ? String(keyName) : null,
    name: name !== null ? String(name) : '',
    publisher: raw.publisher ?? null,
    version: raw.version ?? null,
    installDate: raw.installDate ?? null,
    installLocation: raw.installLocation ?? null,
    estimatedSizeKB: typeof raw.estimatedSizeKB === 'number' && Number.isFinite(raw.estimatedSizeKB)
      ? raw.estimatedSizeKB
      : null,
    canUninstall: Boolean(raw.canUninstall),
    windowsInstaller: Boolean(raw.windowsInstaller)
  };
}

function normalizeDirectoryEntry(raw) {
  if (!raw || typeof raw !== 'object') return null;
  const rawPath = raw.path ? String(raw.path) : '';
  const rootKey = raw.rootKey ? String(raw.rootKey) : null;
  const relativePath = raw.relativePath ? String(raw.relativePath) : null;
  const name = raw.name
    ? String(raw.name)
    : (rawPath ? path.win32.basename(rawPath) : (relativePath ? path.win32.basename(relativePath) : ''));

  const id = raw.id
    ? String(raw.id)
    : (rootKey && relativePath
      ? `${rootKey}:${relativePath.replaceAll('/', '\\').toLowerCase()}`
      : normalizeWindowsPath(rawPath));

  const normalized = {
    id,
    rootKey,
    path: rawPath,
    relativePath,
    name,
    depth: typeof raw.depth === 'number' && Number.isFinite(raw.depth) ? raw.depth : null,
    createdAt: raw.createdAt ?? null,
    modifiedAt: raw.modifiedAt ?? null,
    attributes: raw.attributes ?? null,
    isReparsePoint: Boolean(raw.isReparsePoint)
  };

  if (typeof raw.childDirCount === 'number' && Number.isFinite(raw.childDirCount)) {
    normalized.childDirCount = raw.childDirCount;
  }
  if (typeof raw.childFileCount === 'number' && Number.isFinite(raw.childFileCount)) {
    normalized.childFileCount = raw.childFileCount;
  }

  return normalized;
}

function normalizeWarning(raw, defaultScope = 'snapshot') {
  if (!raw || typeof raw !== 'object') {
    return {
      code: 'UNKNOWN_WARNING',
      scope: defaultScope,
      path: null,
      message: String(raw ?? 'Unknown warning')
    };
  }
  return {
    code: String(raw.code || 'UNKNOWN_WARNING'),
    scope: String(raw.scope || defaultScope),
    path: raw.path ?? null,
    message: String(raw.message || '')
  };
}

function normalizeSnapshot(raw) {
  if (!raw || typeof raw !== 'object') {
    throw new MonitorError('INVALID_SNAPSHOT', 'Snapshot payload must be a JSON object.');
  }

  const registry = asArray(raw.registry).map(normalizeRegistryEntry).filter(Boolean);
  const directories = asArray(raw.directories).map(normalizeDirectoryEntry).filter(Boolean);
  const roots = asArray(raw.roots).map((root) => ({
    key: root?.key ? String(root.key) : 'unknown',
    path: root?.path ? String(root.path) : '',
    exists: Boolean(root?.exists),
    scannedEntries: Number.isFinite(Number(root?.scannedEntries)) ? Number(root.scannedEntries) : 0,
    truncated: Boolean(root?.truncated),
    error: root?.error ?? null
  }));

  const rawCompleteness = raw.completeness && typeof raw.completeness === 'object' ? raw.completeness : {};
  const warnings = asArray(rawCompleteness.warnings).map((w) => normalizeWarning(w));
  const registryTruncated = Boolean(rawCompleteness.registryTruncated);
  const filesystemTruncated = Boolean(rawCompleteness.filesystemTruncated);
  const truncated = Boolean(rawCompleteness.truncated || registryTruncated || filesystemTruncated);
  const errorCount = Number.isFinite(Number(rawCompleteness.errorCount)) ? Number(rawCompleteness.errorCount) : 0;
  const complete = rawCompleteness.complete !== undefined
    ? Boolean(rawCompleteness.complete) && !truncated && errorCount === 0
    : (!truncated && errorCount === 0);

  return {
    schemaVersion: Number.isFinite(Number(raw.schemaVersion)) ? Number(raw.schemaVersion) : 1,
    capturedAt: raw.capturedAt ? String(raw.capturedAt) : new Date().toISOString(),
    durationMs: Number.isFinite(Number(raw.durationMs)) ? Number(raw.durationMs) : 0,
    platform: raw.platform ? String(raw.platform) : process.platform,
    limits: raw.limits && typeof raw.limits === 'object' ? { ...raw.limits } : { ...DEFAULT_LIMITS },
    completeness: {
      complete,
      truncated,
      registryTruncated,
      filesystemTruncated,
      warningCount: warnings.length,
      errorCount,
      warnings
    },
    roots,
    registry,
    directories
  };
}

function summarizeSnapshot(snapshot) {
  return {
    capturedAt: snapshot.capturedAt,
    durationMs: snapshot.durationMs,
    registryCount: snapshot.registry.length,
    directoryCount: snapshot.directories.length,
    rootCount: snapshot.roots.length,
    completeness: snapshot.completeness
  };
}

async function captureWindowsSnapshot(limits) {
  if (process.platform !== 'win32') {
    throw new MonitorError(
      'UNSUPPORTED_PLATFORM',
      `Windows snapshot monitoring requires win32 platform (current: ${process.platform}).`
    );
  }

  const args = [
    '-NoProfile',
    '-NonInteractive',
    '-ExecutionPolicy',
    'Bypass',
    '-File',
    SNAPSHOT_SCRIPT_PATH,
    '-MaxDepth',
    String(limits.maxDepth),
    '-MaxEntries',
    String(limits.maxEntries),
    '-MaxRegistryEntries',
    String(limits.maxRegistryEntries),
    '-MaxDurationMs',
    String(limits.maxDurationMs)
  ];

  let stdout;
  try {
    const result = await execFileAsync('powershell.exe', args, {
      windowsHide: true,
      maxBuffer: 32 * 1024 * 1024,
      timeout: limits.maxDurationMs + 5000,
      encoding: 'utf8'
    });
    stdout = result.stdout;
  } catch (error) {
    if (error?.killed || error?.signal || error?.code === 'ETIMEDOUT') {
      throw new MonitorError('SNAPSHOT_TIMEOUT', 'Windows snapshot script timed out.', {
        cause: error,
        details: { limits, stderr: error.stderr || null }
      });
    }
    if (error?.code === 'ENOENT') {
      throw new MonitorError('POWERSHELL_NOT_FOUND', 'powershell.exe was not found on PATH.', {
        cause: error
      });
    }
    throw new MonitorError('SNAPSHOT_EXECUTION_FAILED', `Snapshot script failed: ${error.message}`, {
      cause: error,
      details: { code: error.code ?? null, stderr: error.stderr || null }
    });
  }

  try {
    const cleaned = String(stdout || '').replace(/^\uFEFF/, '').trim();
    const parsed = JSON.parse(cleaned);
    return normalizeSnapshot(parsed);
  } catch (error) {
    if (error instanceof MonitorError) throw error;
    throw new MonitorError('SNAPSHOT_PARSE_FAILED', `Failed to parse snapshot JSON output: ${error.message}`, {
      cause: error
    });
  }
}

async function runSnapshot(limits, options = {}) {
  if (typeof options.snapshotFn === 'function') {
    const raw = await options.snapshotFn(limits);
    return normalizeSnapshot(raw);
  }
  return captureWindowsSnapshot(limits);
}

function registryComparisonKey(entry) {
  if (entry.id) return String(entry.id).toLowerCase();
  return `${String(entry.hive || '').toLowerCase()}::${String(entry.keyName || entry.name || '').toLowerCase()}`;
}

function directoryComparisonKey(entry) {
  const normPath = normalizeWindowsPath(entry.path);
  if (normPath) return normPath;
  if (entry.id) return String(entry.id).toLowerCase();
  return `${String(entry.rootKey || '').toLowerCase()}:${String(entry.relativePath || '').replaceAll('/', '\\').toLowerCase()}`;
}

function diffFields(beforeObj, afterObj, fields) {
  const changes = [];
  for (const field of fields) {
    const beforeVal = beforeObj[field];
    const afterVal = afterObj[field];
    if (beforeVal === undefined && afterVal === undefined) continue;
    if (beforeVal !== afterVal) {
      changes.push({
        field,
        before: beforeVal ?? null,
        after: afterVal ?? null
      });
    }
  }
  return changes;
}

function isSpecificInstallLocation(normalizedLoc, scannedRootPaths) {
  if (!normalizedLoc) return false;
  if (/^[a-z]:\\?$/.test(normalizedLoc)) return false;
  if (scannedRootPaths.has(normalizedLoc)) return false;
  const parts = normalizedLoc.split('\\').filter(Boolean);
  return parts.length >= 3;
}

function extractAttributionTokens(text) {
  if (typeof text !== 'string' || !text.trim()) {
    return { phrases: [], tokens: [] };
  }
  const normalized = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, ' ')
    .trim();
  if (!normalized) return { phrases: [], tokens: [] };

  const words = normalized.split(/\s+/).filter(Boolean);
  const meaningfulWords = words.filter(
    (word) => word.length >= 3 && !GENERIC_ATTRIBUTION_TOKENS.has(word)
  );
  const phrases = normalized.length >= 4 && !GENERIC_ATTRIBUTION_TOKENS.has(normalized)
    ? [normalized]
    : [];

  return {
    phrases,
    tokens: Array.from(new Set(meaningfulWords))
  };
}

function directoryTextMatchesToken(haystackLower, wordsSet, token) {
  if (wordsSet.has(token)) return true;
  return token.length >= 4 && haystackLower.includes(token);
}

function classifyDirectoryAssociation(dirEntry, changeType, candidates, scannedRootPaths, targetHint) {
  const dirNormPath = normalizeWindowsPath(dirEntry.path || dirEntry.relativePath || '');
  const searchSurface = `${dirEntry.relativePath || ''} ${dirEntry.name || ''} ${dirEntry.path || ''}`.toLowerCase();
  const surfaceWords = new Set(searchSurface.split(/[^a-z0-9]+/).filter(Boolean));

  const strongMatches = [];
  const heuristicMatches = [];

  for (const candidate of candidates) {
    const locNorm = normalizeWindowsPath(candidate.installLocation || '');
    const hasSpecificLoc = isSpecificInstallLocation(locNorm, scannedRootPaths);

    if (hasSpecificLoc && dirNormPath && (dirNormPath === locNorm || dirNormPath.startsWith(`${locNorm}\\`))) {
      strongMatches.push(candidate.registryId);
      continue;
    }

    if (hasSpecificLoc && dirNormPath && locNorm.startsWith(`${dirNormPath}\\`)) {
      heuristicMatches.push(candidate.registryId);
      continue;
    }

    const nameMeta = extractAttributionTokens(candidate.name);
    const pubMeta = extractAttributionTokens(candidate.publisher);
    const allPhrases = [...nameMeta.phrases, ...pubMeta.phrases];
    const allTokens = [...nameMeta.tokens, ...pubMeta.tokens];

    const matchedPhrase = allPhrases.some((phrase) => searchSurface.includes(phrase));
    const matchedToken = allTokens.some((token) => directoryTextMatchesToken(searchSurface, surfaceWords, token));

    if (matchedPhrase || matchedToken) {
      heuristicMatches.push(candidate.registryId);
    }
  }

  let matchedHint = false;
  if (typeof targetHint === 'string' && targetHint.trim().length > 0) {
    const hintMeta = extractAttributionTokens(targetHint);
    matchedHint =
      hintMeta.phrases.some((phrase) => searchSurface.includes(phrase)) ||
      hintMeta.tokens.some((token) => directoryTextMatchesToken(searchSurface, surfaceWords, token));
  }

  if (strongMatches.length > 0) {
    return {
      changeType,
      path: dirEntry.path || dirEntry.relativePath || dirEntry.id,
      rootKey: dirEntry.rootKey ?? null,
      relativePath: dirEntry.relativePath ?? null,
      confidence: 'strong-path-match',
      proven: false,
      correlationOnly: true,
      matchedCandidateIds: strongMatches,
      matchedHint,
      reason: 'Directory path matches or sits inside a candidate registry entry InstallLocation; path correlation is not causal proof.'
    };
  }

  if (heuristicMatches.length > 0 || matchedHint) {
    return {
      changeType,
      path: dirEntry.path || dirEntry.relativePath || dirEntry.id,
      rootKey: dirEntry.rootKey ?? null,
      relativePath: dirEntry.relativePath ?? null,
      confidence: 'name-or-publisher-match',
      proven: false,
      correlationOnly: true,
      matchedCandidateIds: heuristicMatches,
      matchedHint,
      reason: 'Directory name or path heuristically matches a candidate app name, publisher, or target hint; correlation is not causal proof.'
    };
  }

  return {
    changeType,
    path: dirEntry.path || dirEntry.relativePath || dirEntry.id,
    rootKey: dirEntry.rootKey ?? null,
    relativePath: dirEntry.relativePath ?? null,
    confidence: 'unattributed',
    proven: false,
    correlationOnly: true,
    matchedCandidateIds: [],
    matchedHint: false,
    reason: 'Observed during monitoring window without matching candidate registry location, name, publisher, or target hint; may reflect concurrent background activity.'
  };
}

export function compareSnapshots(baselineInput, finalInput, options = {}) {
  const warnings = [];

  if (!baselineInput || typeof baselineInput !== 'object') {
    warnings.push({
      code: 'INVALID_BASELINE_SNAPSHOT',
      scope: 'comparison',
      path: null,
      message: 'Baseline snapshot was missing or invalid.'
    });
  }
  if (!finalInput || typeof finalInput !== 'object') {
    warnings.push({
      code: 'INVALID_FINAL_SNAPSHOT',
      scope: 'comparison',
      path: null,
      message: 'Final snapshot was missing or invalid.'
    });
  }

  const baseline = baselineInput && typeof baselineInput === 'object' ? normalizeSnapshot(baselineInput) : normalizeSnapshot({});
  const final = finalInput && typeof finalInput === 'object' ? normalizeSnapshot(finalInput) : normalizeSnapshot({});

  for (const w of baseline.completeness.warnings) {
    warnings.push({ ...w, scope: `baseline:${w.scope}` });
  }
  for (const w of final.completeness.warnings) {
    warnings.push({ ...w, scope: `final:${w.scope}` });
  }

  if (baseline.completeness.truncated || final.completeness.truncated) {
    warnings.push({
      code: 'SNAPSHOT_TRUNCATED_DIFF_INCOMPLETE',
      scope: 'comparison',
      path: null,
      message: 'One or both snapshots were truncated by entry or runtime bounds; added/removed entries may reflect incomplete coverage rather than actual installation changes.'
    });
  }

  if (baseline.limits?.maxDepth !== final.limits?.maxDepth) {
    warnings.push({
      code: 'SNAPSHOT_DEPTH_MISMATCH',
      scope: 'comparison',
      path: null,
      message: `Baseline maxDepth (${baseline.limits?.maxDepth}) differs from final maxDepth (${final.limits?.maxDepth}).`
    });
  }

  // Registry comparison
  const baseRegMap = new Map();
  for (const entry of baseline.registry) {
    baseRegMap.set(registryComparisonKey(entry), entry);
  }
  const finalRegMap = new Map();
  for (const entry of final.registry) {
    finalRegMap.set(registryComparisonKey(entry), entry);
  }

  const registryAdded = [];
  const registryRemoved = [];
  const registryChanged = [];
  let registryUnchangedCount = 0;

  for (const [key, finalEntry] of finalRegMap.entries()) {
    const baseEntry = baseRegMap.get(key);
    if (!baseEntry) {
      registryAdded.push(finalEntry);
      continue;
    }
    const changes = diffFields(baseEntry, finalEntry, REGISTRY_COMPARE_FIELDS);
    if (changes.length > 0) {
      registryChanged.push({
        id: finalEntry.id,
        name: finalEntry.name || baseEntry.name,
        before: baseEntry,
        after: finalEntry,
        changes
      });
    } else {
      registryUnchangedCount++;
    }
  }

  for (const [key, baseEntry] of baseRegMap.entries()) {
    if (!finalRegMap.has(key)) {
      registryRemoved.push(baseEntry);
    }
  }

  registryAdded.sort((a, b) => a.id.localeCompare(b.id));
  registryRemoved.sort((a, b) => a.id.localeCompare(b.id));
  registryChanged.sort((a, b) => a.id.localeCompare(b.id));

  // Directory comparison
  const baseDirMap = new Map();
  for (const entry of baseline.directories) {
    baseDirMap.set(directoryComparisonKey(entry), entry);
  }
  const finalDirMap = new Map();
  for (const entry of final.directories) {
    finalDirMap.set(directoryComparisonKey(entry), entry);
  }

  const dirAdded = [];
  const dirRemoved = [];
  const dirChanged = [];
  let dirUnchangedCount = 0;

  for (const [key, finalEntry] of finalDirMap.entries()) {
    const baseEntry = baseDirMap.get(key);
    if (!baseEntry) {
      dirAdded.push(finalEntry);
      continue;
    }
    const changes = diffFields(baseEntry, finalEntry, DIRECTORY_COMPARE_FIELDS);
    if (changes.length > 0) {
      dirChanged.push({
        key,
        path: finalEntry.path || baseEntry.path || finalEntry.id,
        before: baseEntry,
        after: finalEntry,
        changes
      });
    } else {
      dirUnchangedCount++;
    }
  }

  for (const [key, baseEntry] of baseDirMap.entries()) {
    if (!finalDirMap.has(key)) {
      dirRemoved.push(baseEntry);
    }
  }

  dirAdded.sort((a, b) => directoryComparisonKey(a).localeCompare(directoryComparisonKey(b)));
  dirRemoved.sort((a, b) => directoryComparisonKey(a).localeCompare(directoryComparisonKey(b)));
  dirChanged.sort((a, b) => a.key.localeCompare(b.key));

  // Attribution analysis (explicitly distinguishing observation from causal proof)
  const scannedRootPaths = new Set();
  for (const root of [...baseline.roots, ...final.roots]) {
    const norm = normalizeWindowsPath(root.path);
    if (norm) scannedRootPaths.add(norm);
  }

  const candidateApps = [
    ...registryAdded.map((entry) => ({
      registryId: entry.id,
      name: entry.name,
      publisher: entry.publisher,
      version: entry.version,
      installLocation: entry.installLocation,
      changeType: 'added',
      evidenceLevel: 'registry-entry-observed'
    })),
    ...registryChanged.map((item) => ({
      registryId: item.after.id,
      name: item.after.name,
      publisher: item.after.publisher,
      version: item.after.version,
      installLocation: item.after.installLocation,
      changeType: 'changed',
      evidenceLevel: 'registry-entry-observed'
    }))
  ].sort((a, b) => a.name.localeCompare(b.name) || a.registryId.localeCompare(b.registryId));

  const targetHint = typeof options.targetHint === 'string' && options.targetHint.trim().length > 0
    ? options.targetHint.trim()
    : null;

  const directoryAssociations = [
    ...dirAdded.map((entry) => classifyDirectoryAssociation(entry, 'added', candidateApps, scannedRootPaths, targetHint)),
    ...dirChanged.map((item) => classifyDirectoryAssociation(item.after, 'changed', candidateApps, scannedRootPaths, targetHint)),
    ...dirRemoved.map((entry) => classifyDirectoryAssociation(entry, 'removed', candidateApps, scannedRootPaths, targetHint))
  ];

  let strongPathMatchCount = 0;
  let heuristicNameMatchCount = 0;
  let unattributedCount = 0;

  for (const assoc of directoryAssociations) {
    if (assoc.confidence === 'strong-path-match') strongPathMatchCount++;
    else if (assoc.confidence === 'name-or-publisher-match') heuristicNameMatchCount++;
    else unattributedCount++;
  }

  const reliable = Boolean(
    baselineInput &&
    finalInput &&
    !baseline.completeness.truncated &&
    !final.completeness.truncated &&
    baseline.limits?.maxDepth === final.limits?.maxDepth
  );
  const complete = reliable && baseline.completeness.complete && final.completeness.complete;

  const totalObservedChanges =
    registryAdded.length +
    registryRemoved.length +
    registryChanged.length +
    dirAdded.length +
    dirRemoved.length +
    dirChanged.length;

  return {
    schemaVersion: 1,
    comparedAt: options.comparedAt ?? final.capturedAt ?? null,
    baselineCapturedAt: baseline.capturedAt,
    finalCapturedAt: final.capturedAt,
    targetHint,
    completeness: {
      complete,
      reliable,
      baselineComplete: baseline.completeness.complete,
      finalComplete: final.completeness.complete,
      warningCount: warnings.length,
      warnings
    },
    observedChanges: {
      registry: {
        added: registryAdded,
        removed: registryRemoved,
        changed: registryChanged,
        unchangedCount: registryUnchangedCount
      },
      directories: {
        added: dirAdded,
        removed: dirRemoved,
        changed: dirChanged,
        unchangedCount: dirUnchangedCount
      },
      summary: {
        registryAdded: registryAdded.length,
        registryRemoved: registryRemoved.length,
        registryChanged: registryChanged.length,
        registryUnchanged: registryUnchangedCount,
        directoriesAdded: dirAdded.length,
        directoriesRemoved: dirRemoved.length,
        directoriesChanged: dirChanged.length,
        directoriesUnchanged: dirUnchangedCount,
        totalObservedChanges
      }
    },
    attributionAnalysis: {
      methodology: 'snapshot-diff',
      provenCausalAttribution: false,
      provenCount: 0,
      statement:
        'Observed changes reflect state differences between baseline and final snapshots. Snapshot monitoring does not trace kernel or process I/O and cannot prove which installer or background process caused a change.',
      candidateApps,
      directoryAssociations,
      summary: {
        candidateAppCount: candidateApps.length,
        strongPathMatchCount,
        heuristicNameMatchCount,
        unattributedCount,
        provenCount: 0
      }
    }
  };
}

async function readJsonFile(filePath) {
  const raw = await readFile(filePath, 'utf8');
  return JSON.parse(raw);
}

async function writeJsonFile(filePath, data) {
  await writeFile(filePath, JSON.stringify(data, null, 2), 'utf8');
}

export async function startMonitor(options = {}) {
  const storageRoot = getMonitorStorageDir(options);
  const limits = normalizeLimits(options);
  const id = options.id
    ? validateMonitorId(options.id)
    : `mon_${Date.now().toString(36)}_${randomBytes(4).toString('hex')}`;

  const monitorDir = path.join(storageRoot, id);
  let alreadyExists = false;
  try {
    await access(monitorDir);
    alreadyExists = true;
  } catch {
    alreadyExists = false;
  }
  if (alreadyExists) {
    throw new MonitorError('MONITOR_ALREADY_EXISTS', `Monitor "${id}" already exists.`);
  }

  const label = typeof options.label === 'string' && options.label.trim().length > 0
    ? options.label.trim()
    : null;
  const targetHint = typeof options.targetHint === 'string' && options.targetHint.trim().length > 0
    ? options.targetHint.trim()
    : null;

  const baseline = await runSnapshot(limits, options);
  const baselineSummary = summarizeSnapshot(baseline);

  const monitorRecord = {
    schemaVersion: 1,
    id,
    label,
    targetHint,
    status: 'active',
    startedAt: baseline.capturedAt,
    stoppedAt: null,
    limits,
    storagePath: monitorDir,
    baselineSummary,
    finalSummary: null,
    comparisonSummary: null,
    attributionSummary: null,
    completeness: {
      complete: baseline.completeness.complete,
      reliable: !baseline.completeness.truncated,
      warningCount: baseline.completeness.warningCount,
      warnings: baseline.completeness.warnings
    }
  };

  await mkdir(storageRoot, { recursive: true });
  try {
    await mkdir(monitorDir, { recursive: false });
  } catch (error) {
    if (error?.code === 'EEXIST') {
      throw new MonitorError('MONITOR_ALREADY_EXISTS', `Monitor "${id}" already exists.`);
    }
    throw error;
  }
  await writeJsonFile(path.join(monitorDir, 'baseline.json'), baseline);
  await writeJsonFile(path.join(monitorDir, 'monitor.json'), monitorRecord);

  return monitorRecord;
}

export async function stopMonitor(id, options = {}) {
  const validId = validateMonitorId(id);
  const storageRoot = getMonitorStorageDir(options);
  const monitorDir = path.join(storageRoot, validId);
  const monitorPath = path.join(monitorDir, 'monitor.json');
  const baselinePath = path.join(monitorDir, 'baseline.json');

  let monitorRecord;
  let baseline;
  try {
    monitorRecord = await readJsonFile(monitorPath);
    baseline = await readJsonFile(baselinePath);
  } catch (error) {
    if (error?.code === 'ENOENT') {
      throw new MonitorError('MONITOR_NOT_FOUND', `Monitor "${validId}" was not found.`);
    }
    throw new MonitorError('MONITOR_READ_FAILED', `Failed to read monitor "${validId}": ${error.message}`, {
      cause: error
    });
  }

  if (monitorRecord.status === 'stopped') {
    throw new MonitorError('MONITOR_ALREADY_STOPPED', `Monitor "${validId}" is already stopped.`);
  }

  const limits = normalizeLimits({
    ...monitorRecord.limits,
    ...(options.maxDepth !== undefined ? { maxDepth: options.maxDepth } : {}),
    ...(options.maxEntries !== undefined ? { maxEntries: options.maxEntries } : {}),
    ...(options.maxRegistryEntries !== undefined ? { maxRegistryEntries: options.maxRegistryEntries } : {}),
    ...(options.maxDurationMs !== undefined ? { maxDurationMs: options.maxDurationMs } : {})
  });

  const finalSnapshot = await runSnapshot(limits, options);
  const finalSummary = summarizeSnapshot(finalSnapshot);
  const comparison = compareSnapshots(baseline, finalSnapshot, {
    targetHint: options.targetHint ?? monitorRecord.targetHint,
    comparedAt: finalSnapshot.capturedAt
  });

  const updatedRecord = {
    ...monitorRecord,
    status: 'stopped',
    stoppedAt: finalSnapshot.capturedAt,
    finalSummary,
    comparisonSummary: comparison.observedChanges.summary,
    attributionSummary: comparison.attributionAnalysis.summary,
    completeness: comparison.completeness
  };

  await writeJsonFile(path.join(monitorDir, 'final.json'), finalSnapshot);
  await writeJsonFile(path.join(monitorDir, 'comparison.json'), comparison);
  await writeJsonFile(monitorPath, updatedRecord);

  return {
    ...updatedRecord,
    comparison
  };
}

export async function listMonitors(options = {}) {
  const storageRoot = getMonitorStorageDir(options);

  let entries;
  try {
    entries = await readdir(storageRoot, { withFileTypes: true });
  } catch (error) {
    if (error?.code === 'ENOENT') {
      return [];
    }
    throw new MonitorError('MONITOR_LIST_FAILED', `Failed to list monitors: ${error.message}`, {
      cause: error
    });
  }

  const monitors = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || !MONITOR_ID_PATTERN.test(entry.name)) {
      continue;
    }
    const monitorPath = path.join(storageRoot, entry.name, 'monitor.json');
    try {
      const record = await readJsonFile(monitorPath);
      if (record && typeof record === 'object' && record.id) {
        monitors.push(record);
      }
    } catch {
      // Skip incomplete or unreadable monitor entries cleanly
    }
  }

  monitors.sort((a, b) => {
    const timeA = String(a.startedAt || '');
    const timeB = String(b.startedAt || '');
    return timeB.localeCompare(timeA) || String(a.id).localeCompare(String(b.id));
  });

  return monitors;
}

export async function getMonitor(id, options = {}) {
  const validId = validateMonitorId(id);
  const storageRoot = getMonitorStorageDir(options);
  const monitorDir = path.join(storageRoot, validId);
  const monitorPath = path.join(monitorDir, 'monitor.json');

  let record;
  try {
    record = await readJsonFile(monitorPath);
  } catch (error) {
    if (error?.code === 'ENOENT') {
      return null;
    }
    throw new MonitorError('MONITOR_READ_FAILED', `Failed to read monitor "${validId}": ${error.message}`, {
      cause: error
    });
  }

  const result = { ...record };

  if (record.status === 'stopped') {
    try {
      result.comparison = await readJsonFile(path.join(monitorDir, 'comparison.json'));
    } catch (error) {
      if (error?.code !== 'ENOENT') {
        throw new MonitorError('MONITOR_READ_FAILED', `Failed to read comparison for "${validId}": ${error.message}`, {
          cause: error
        });
      }
      result.comparison = null;
    }
  }

  if (options.includeSnapshots === true) {
    try {
      result.baselineSnapshot = await readJsonFile(path.join(monitorDir, 'baseline.json'));
    } catch {
      result.baselineSnapshot = null;
    }
    if (record.status === 'stopped') {
      try {
        result.finalSnapshot = await readJsonFile(path.join(monitorDir, 'final.json'));
      } catch {
        result.finalSnapshot = null;
      }
    }
  }

  return result;
}
