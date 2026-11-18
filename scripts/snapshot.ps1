param(
  [ValidateRange(1, 4)]
  [int]$MaxDepth = 2,

  [ValidateRange(10, 20000)]
  [int]$MaxEntries = 4000,

  [ValidateRange(10, 10000)]
  [int]$MaxRegistryEntries = 4000,

  [ValidateRange(500, 60000)]
  [int]$MaxDurationMs = 15000
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$capturedAt = [DateTime]::UtcNow.ToString('o')

$warnings = [System.Collections.Generic.List[object]]::new()
$registryEntries = [System.Collections.Generic.List[object]]::new()
$directoryEntries = [System.Collections.Generic.List[object]]::new()
$rootReports = [System.Collections.Generic.List[object]]::new()

$registryTruncated = $false
$filesystemTruncated = $false
$errorCount = 0

function Add-WarningItem {
  param(
    [string]$Code,
    [string]$Scope,
    [string]$Path,
    [string]$Message
  )
  $warnings.Add([pscustomobject]@{
    code    = $Code
    scope   = $Scope
    path    = $Path
    message = $Message
  })
}

# 1. Bounded uninstall registry identity scan (read-only)
$registryHives = @(
  @{ key = 'HKLM-64'; path = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' },
  @{ key = 'HKLM-32'; path = 'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*' },
  @{ key = 'HKCU';    path = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' }
)

foreach ($hive in $registryHives) {
  if ($stopwatch.ElapsedMilliseconds -ge $MaxDurationMs) {
    $registryTruncated = $true
    Add-WarningItem -Code 'RUNTIME_BUDGET_EXCEEDED' -Scope 'registry' -Path $hive.path -Message "Runtime budget of ${MaxDurationMs}ms exceeded before completing registry hive $($hive.key)."
    break
  }
  if ($registryEntries.Count -ge $MaxRegistryEntries) {
    $registryTruncated = $true
    Add-WarningItem -Code 'REGISTRY_ENTRY_LIMIT_REACHED' -Scope 'registry' -Path $hive.path -Message "Registry entry limit of $MaxRegistryEntries reached."
    break
  }

  try {
    $items = Get-ItemProperty -Path $hive.path -ErrorAction SilentlyContinue
    foreach ($item in $items) {
      if ($stopwatch.ElapsedMilliseconds -ge $MaxDurationMs) {
        $registryTruncated = $true
        Add-WarningItem -Code 'RUNTIME_BUDGET_EXCEEDED' -Scope 'registry' -Path $hive.path -Message "Runtime budget of ${MaxDurationMs}ms exceeded while enumerating $($hive.key)."
        break
      }
      if ($registryEntries.Count -ge $MaxRegistryEntries) {
        $registryTruncated = $true
        Add-WarningItem -Code 'REGISTRY_ENTRY_LIMIT_REACHED' -Scope 'registry' -Path $hive.path -Message "Registry entry limit of $MaxRegistryEntries reached while enumerating $($hive.key)."
        break
      }
      if (-not $item.DisplayName -or $item.SystemComponent -eq 1) {
        continue
      }

      $estimatedSizeKB = $null
      if ($null -ne $item.EstimatedSize) {
        try {
          $estimatedSizeKB = [int64]$item.EstimatedSize
        } catch {
          $estimatedSizeKB = $null
        }
      }

      $registryEntries.Add([pscustomobject]@{
        id              = [string]$item.PSPath
        hive            = $hive.key
        keyName         = [string]$item.PSChildName
        name            = [string]$item.DisplayName
        publisher       = if ($null -ne $item.Publisher) { [string]$item.Publisher } else { $null }
        version         = if ($null -ne $item.DisplayVersion) { [string]$item.DisplayVersion } else { $null }
        installDate     = if ($null -ne $item.InstallDate) { [string]$item.InstallDate } else { $null }
        installLocation = if ($null -ne $item.InstallLocation) { [string]$item.InstallLocation } else { $null }
        estimatedSizeKB = $estimatedSizeKB
        canUninstall    = [bool]$item.UninstallString
        windowsInstaller = ($item.WindowsInstaller -eq 1)
      })
    }
  } catch [System.Security.SecurityException] {
    $errorCount++
    Add-WarningItem -Code 'REGISTRY_ACCESS_DENIED' -Scope 'registry' -Path $hive.path -Message $_.Exception.Message
  } catch {
    $errorCount++
    Add-WarningItem -Code 'REGISTRY_ERROR' -Scope 'registry' -Path $hive.path -Message $_.Exception.Message
  }
}

# 2. Bounded BFS directory inventory of AppData / Program Files / ProgramData (read-only)
$candidateRoots = @(
  @{ key = 'programFiles';    raw = $env:ProgramFiles },
  @{ key = 'programFilesX86'; raw = ${env:ProgramFiles(x86)} },
  @{ key = 'programW6432';    raw = $env:ProgramW6432 },
  @{ key = 'localAppData';    raw = $env:LOCALAPPDATA },
  @{ key = 'roamingAppData';  raw = $env:APPDATA },
  @{ key = 'programData';     raw = $env:ProgramData }
)

$seenRoots = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
$queue = [System.Collections.Generic.Queue[object]]::new()
$rootStateByKey = @{}

foreach ($candidate in $candidateRoots) {
  if ([string]::IsNullOrWhiteSpace($candidate.raw)) {
    continue
  }

  $fullPath = $null
  try {
    $fullPath = [System.IO.Path]::GetFullPath($candidate.raw).TrimEnd('\')
  } catch {
    $errorCount++
    Add-WarningItem -Code 'INVALID_ROOT_PATH' -Scope 'filesystem' -Path $candidate.raw -Message $_.Exception.Message
    continue
  }

  if (-not $seenRoots.Add($fullPath)) {
    continue
  }

  $exists = Test-Path -LiteralPath $fullPath -PathType Container
  $report = [pscustomobject]@{
    key            = $candidate.key
    path           = $fullPath
    exists         = $exists
    scannedEntries = 0
    truncated      = $false
    error          = $null
  }
  $rootStateByKey[$candidate.key] = $report
  $rootReports.Add($report)

  if (-not $exists) {
    Add-WarningItem -Code 'ROOT_NOT_FOUND' -Scope 'filesystem' -Path $fullPath -Message "Configured root $($candidate.key) does not exist."
    continue
  }

  $queue.Enqueue([pscustomobject]@{
    dirInfo  = [System.IO.DirectoryInfo]::new($fullPath)
    rootKey  = $candidate.key
    rootPath = $fullPath
    depth    = 0
  })
}

while ($queue.Count -gt 0) {
  if ($stopwatch.ElapsedMilliseconds -ge $MaxDurationMs) {
    $filesystemTruncated = $true
    Add-WarningItem -Code 'RUNTIME_BUDGET_EXCEEDED' -Scope 'filesystem' -Path $null -Message "Runtime budget of ${MaxDurationMs}ms exceeded during directory inventory."
    break
  }
  if ($directoryEntries.Count -ge $MaxEntries) {
    $filesystemTruncated = $true
    Add-WarningItem -Code 'ENTRY_LIMIT_REACHED' -Scope 'filesystem' -Path $null -Message "Filesystem directory entry limit of $MaxEntries reached."
    break
  }

  $node = $queue.Dequeue()
  if ($node.depth -ge $MaxDepth) {
    continue
  }

  $subDirs = $null
  try {
    $subDirs = $node.dirInfo.EnumerateDirectories()
    foreach ($subDir in $subDirs) {
      if ($stopwatch.ElapsedMilliseconds -ge $MaxDurationMs) {
        $filesystemTruncated = $true
        $rootStateByKey[$node.rootKey].truncated = $true
        Add-WarningItem -Code 'RUNTIME_BUDGET_EXCEEDED' -Scope 'filesystem' -Path $node.dirInfo.FullName -Message "Runtime budget of ${MaxDurationMs}ms exceeded while enumerating $($node.dirInfo.FullName)."
        break
      }
      if ($directoryEntries.Count -ge $MaxEntries) {
        $filesystemTruncated = $true
        $rootStateByKey[$node.rootKey].truncated = $true
        Add-WarningItem -Code 'ENTRY_LIMIT_REACHED' -Scope 'filesystem' -Path $node.dirInfo.FullName -Message "Filesystem directory entry limit of $MaxEntries reached while enumerating $($node.dirInfo.FullName)."
        break
      }

      $childDepth = $node.depth + 1
      $childPath = $subDir.FullName.TrimEnd('\')
      $relPath = if ($childPath.Length -gt $node.rootPath.Length + 1) {
        $childPath.Substring($node.rootPath.Length + 1)
      } else {
        $subDir.Name
      }

      $isReparse = (($subDir.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
      $createdAt = $null
      $modifiedAt = $null
      try { $createdAt = $subDir.CreationTimeUtc.ToString('o') } catch {}
      try { $modifiedAt = $subDir.LastWriteTimeUtc.ToString('o') } catch {}

      $directoryEntries.Add([pscustomobject]@{
        id             = "$($node.rootKey):$($relPath.ToLowerInvariant())"
        rootKey        = $node.rootKey
        path           = $childPath
        relativePath   = $relPath
        name           = $subDir.Name
        depth          = $childDepth
        createdAt      = $createdAt
        modifiedAt     = $modifiedAt
        attributes     = [string]$subDir.Attributes
        isReparsePoint = $isReparse
      })

      $rootStateByKey[$node.rootKey].scannedEntries++

      if (-not $isReparse -and $childDepth -lt $MaxDepth) {
        $queue.Enqueue([pscustomobject]@{
          dirInfo  = $subDir
          rootKey  = $node.rootKey
          rootPath = $node.rootPath
          depth    = $childDepth
        })
      }
    }
  } catch [System.UnauthorizedAccessException] {
    $errorCount++
    if ($node.depth -eq 0) {
      $rootStateByKey[$node.rootKey].error = $_.Exception.Message
    }
    Add-WarningItem -Code 'ACCESS_DENIED' -Scope 'filesystem' -Path $node.dirInfo.FullName -Message $_.Exception.Message
  } catch [System.Security.SecurityException] {
    $errorCount++
    if ($node.depth -eq 0) {
      $rootStateByKey[$node.rootKey].error = $_.Exception.Message
    }
    Add-WarningItem -Code 'SECURITY_ERROR' -Scope 'filesystem' -Path $node.dirInfo.FullName -Message $_.Exception.Message
  } catch [System.IO.PathTooLongException] {
    $errorCount++
    Add-WarningItem -Code 'PATH_TOO_LONG' -Scope 'filesystem' -Path $node.dirInfo.FullName -Message $_.Exception.Message
  } catch [System.IO.DirectoryNotFoundException] {
    $errorCount++
    Add-WarningItem -Code 'DIRECTORY_NOT_FOUND' -Scope 'filesystem' -Path $node.dirInfo.FullName -Message $_.Exception.Message
  } catch {
    $errorCount++
    if ($node.depth -eq 0) {
      $rootStateByKey[$node.rootKey].error = $_.Exception.Message
    }
    Add-WarningItem -Code 'FS_ERROR' -Scope 'filesystem' -Path $node.dirInfo.FullName -Message $_.Exception.Message
  }

  if ($filesystemTruncated) {
    break
  }
}

$stopwatch.Stop()
$truncated = $registryTruncated -or $filesystemTruncated

$payload = [pscustomobject]@{
  schemaVersion = 1
  capturedAt    = $capturedAt
  durationMs    = [int64]$stopwatch.ElapsedMilliseconds
  platform      = 'win32'
  limits        = [pscustomobject]@{
    maxDepth           = $MaxDepth
    maxEntries         = $MaxEntries
    maxRegistryEntries = $MaxRegistryEntries
    maxDurationMs      = $MaxDurationMs
  }
  completeness  = [pscustomobject]@{
    complete            = (-not $truncated -and $errorCount -eq 0)
    truncated           = $truncated
    registryTruncated   = $registryTruncated
    filesystemTruncated = $filesystemTruncated
    warningCount        = $warnings.Count
    errorCount          = $errorCount
    warnings            = @($warnings.ToArray())
  }
  roots         = @($rootReports.ToArray())
  registry      = @($registryEntries.ToArray())
  directories   = @($directoryEntries.ToArray())
}

ConvertTo-Json -InputObject $payload -Compress -Depth 6
