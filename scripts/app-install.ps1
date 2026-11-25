# Install Watt for the current Windows user (no admin prompt) from the NSIS installer.
# Run with: npm run app:install:win
# Options:
#   -InstallerPath <exe>  install this installer instead of the built one
#   -Rebuild              rebuild first, even if the installer looks up to date
#   -CloseRunning         stop the installed Watt if it is running (default: refuse and ask you to close it)
param(
  [string]$InstallerPath = '',
  [switch]$Rebuild,
  [switch]$CloseRunning
)

$ErrorActionPreference = 'Stop'

function Info([string]$msg) { Write-Output "==> $msg" }

function Fail([string]$msg) {
  [Console]::Error.WriteLine("error: $msg")
  exit 1
}

if ($env:OS -ne 'Windows_NT') { Fail 'this script installs the Windows app and needs Windows' }

$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

$pkgVersion = (Get-Content -LiteralPath (Join-Path $root 'package.json') -Raw | ConvertFrom-Json).version
$installDir = Join-Path $env:LOCALAPPDATA 'Watt'
$installedExe = Join-Path $installDir 'watt-terminal.exe'
$desktopLnk = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Watt.lnk'
$startLnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Watt.lnk'
$nsisDir = Join-Path $root 'src-tauri\target\release\bundle\nsis'

function Find-Installer {
  Get-ChildItem -LiteralPath $nsisDir -Filter "Watt_${pkgVersion}_*-setup.exe" -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

# Rebuild when the installer is missing or older than any source input.
function Test-InstallerStale([System.IO.FileInfo]$installer) {
  $inputs = @('src', 'src-tauri\src', 'src-tauri\windows', 'src-tauri\Cargo.toml', 'src-tauri\Cargo.lock',
    'src-tauri\tauri.conf.json', 'src-tauri\tauri.windows.conf.json', 'src-tauri\vendor', 'package.json', 'index.html',
    'vite.config.ts') |
    Where-Object { Test-Path -LiteralPath (Join-Path $root $_) }
  $newest = $inputs | ForEach-Object {
    $p = Join-Path $root $_
    if (Test-Path -LiteralPath $p -PathType Container) {
      Get-ChildItem -LiteralPath $p -Recurse -File | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    } else {
      Get-Item -LiteralPath $p
    }
  } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
  return ($newest.LastWriteTime -gt $installer.LastWriteTime)
}

Info 'preflight'
if ($InstallerPath) {
  if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) { Fail "installer not found: $InstallerPath" }
  $installer = Get-Item -LiteralPath $InstallerPath
  Info "using the installer you passed: $($installer.FullName)"
} else {
  $installer = Find-Installer
  $stale = [bool]($installer -and (Test-InstallerStale $installer))
  if ($Rebuild) {
    Info 'rebuilding (-Rebuild)'
  } elseif (-not $installer) {
    Info 'no installer found, building one'
  } elseif ($stale) {
    Info 'the installer is older than the sources, rebuilding'
  } else {
    Info "installer is up to date: $($installer.FullName)"
  }
  if ($Rebuild -or -not $installer -or $stale) {
    & "$PSScriptRoot\app-build.ps1"
    if ($LASTEXITCODE -ne 0) { Fail "build failed (exit code $LASTEXITCODE). Nothing was installed." }
    $installer = Find-Installer
    if (-not $installer) { Fail "build finished but no installer was found in $nsisDir" }
  }
}

$running = @(Get-Process -Name 'watt-terminal' -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -eq $installedExe })
if ($running.Count -gt 0) {
  $pids = ($running | ForEach-Object { $_.Id }) -join ', '
  if (-not $CloseRunning) {
    Fail "Watt is running from $installedExe (pid $pids). Close it and run again, or pass -CloseRunning. Nothing was installed."
  }
  Info "closing the running Watt (pid $pids)"
  foreach ($p in $running) {
    [void]$p.CloseMainWindow()
    if (-not $p.WaitForExit(10000)) {
      Info "pid $($p.Id) did not close in 10 seconds, stopping it"
      Stop-Process -Id $p.Id -Force
    }
  }
}

Info "running the installer silently (/S, per-user, no elevation): $($installer.FullName)"
$proc = Start-Process -FilePath $installer.FullName -ArgumentList '/S' -Wait -PassThru
if ($proc.ExitCode -ne 0) { Fail "installer exited with code $($proc.ExitCode)" }

Info 'verifying the install'
$problems = @()

if (Test-Path -LiteralPath $installedExe -PathType Leaf) {
  Write-Output "  install dir: $installDir (exe present)"
} else {
  $problems += "missing executable: $installedExe"
}

# portable-pty falls back silently to the inbox conhost if these are not next to the exe.
foreach ($host_file in 'conpty.dll', 'OpenConsole.exe') {
  $hostPath = Join-Path $installDir $host_file
  if (Test-Path -LiteralPath $hostPath -PathType Leaf) {
    Write-Output "  conpty host: $hostPath (present)"
  } else {
    $problems += "missing ConPTY host next to the exe: $hostPath"
  }
}

$uninstKeys = @(Get-ChildItem -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
  Where-Object { (Get-ItemProperty -LiteralPath $_.PSPath).DisplayName -eq 'Watt' })
if ($uninstKeys.Count -ne 1) {
  $problems += "expected 1 HKCU Uninstall entry with DisplayName Watt, found $($uninstKeys.Count)"
} else {
  $u = Get-ItemProperty -LiteralPath $uninstKeys[0].PSPath
  Write-Output "  registry:    $($uninstKeys[0].Name)"
  Write-Output "    DisplayName:     $($u.DisplayName)"
  Write-Output "    DisplayVersion:  $($u.DisplayVersion)"
  Write-Output "    Publisher:       $($u.Publisher)"
  Write-Output "    InstallLocation: $($u.InstallLocation)"
  Write-Output "    DisplayIcon:     $($u.DisplayIcon)"
  Write-Output "    UninstallString: $($u.UninstallString)"
  if ($u.DisplayVersion -ne $pkgVersion) { $problems += "DisplayVersion is '$($u.DisplayVersion)', expected $pkgVersion" }
}

$wsh = New-Object -ComObject WScript.Shell
try {
  foreach ($pair in @(@('Desktop shortcut', $desktopLnk), @('Start Menu shortcut', $startLnk))) {
    $label = $pair[0]
    $path = $pair[1]
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
      $problems += "$label missing: $path"
      continue
    }
    $target = $wsh.CreateShortcut($path).TargetPath
    Write-Output "  $label`: $path"
    Write-Output "    target: $target"
    if ($target -ne $installedExe) { $problems += "$label targets '$target', expected $installedExe" }
  }
} finally {
  [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($wsh)
}

if ($problems.Count -gt 0) {
  foreach ($msg in $problems) { [Console]::Error.WriteLine("error: $msg") }
  Fail 'the install did not verify'
}
Write-Output "Watt $pkgVersion installed at $installDir"
