$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$paths = @('HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*','HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*')
$items = @(foreach ($path in $paths) {
  Get-ItemProperty $path -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -and $_.SystemComponent -ne 1 } | ForEach-Object {
    [pscustomobject]@{ id=$_.PSPath; name=$_.DisplayName; publisher=$_.Publisher; version=$_.DisplayVersion; sizeMB=[math]::Round(($_.EstimatedSize / 1024),1); installed=$_.InstallDate; location=$_.InstallLocation; command=$_.UninstallString; canUninstall=[bool]$_.UninstallString }
  }
})
ConvertTo-Json -InputObject $items -Compress -Depth 3
