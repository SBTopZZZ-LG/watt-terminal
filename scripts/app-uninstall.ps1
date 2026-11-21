# Uninstall Watt for the current Windows user, silently, through the installer's own uninstaller.
# Run with: npm run app:uninstall:win
# Removes the install folder, the shortcuts and the Installed apps entry. Your data
# (%APPDATA%\dev.watt.terminal and %LOCALAPPDATA%\dev.watt.terminal) is kept.
$ErrorActionPreference = 'Stop'

function Info([string]$msg) { Write-Output "==> $msg" }

function Fail([string]$msg) {
  [Console]::Error.WriteLine("error: $msg")
  exit 1
}

if ($env:OS -ne 'Windows_NT') { Fail 'this script uninstalls the Windows app and needs Windows' }

$installDir = Join-Path $env:LOCALAPPDATA 'Watt'
$installedExe = Join-Path $installDir 'watt-terminal.exe'
$desktopLnk = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Watt.lnk'
$startLnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Watt.lnk'

# The Installed apps entry of this install: DisplayName Watt and InstallLocation is $installDir. Another product can be named
# Watt, so the name alone is not enough.
function Get-WattEntries {
  @(Get-ChildItem -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue | Where-Object {
    $p = Get-ItemProperty -LiteralPath $_.PSPath
    $loc = ([string]$p.InstallLocation).Trim().Trim('"').TrimEnd('\')
    $p.DisplayName -eq 'Watt' -and $loc -ieq $installDir
  })
}

$uninstKeys = Get-WattEntries
if ($uninstKeys.Count -gt 1) { Fail "found $($uninstKeys.Count) HKCU Uninstall entries named Watt. Remove the extra ones by hand first." }

$running = @(Get-Process -Name 'watt-terminal' -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -eq $installedExe })
if ($running.Count -gt 0) {
  Fail "Watt is running from $installedExe. Close it and run again."
}

if ($uninstKeys.Count -eq 0) {
  Info 'no Installed apps entry for Watt (HKCU); nothing to run'
} else {
  $u = Get-ItemProperty -LiteralPath $uninstKeys[0].PSPath
  $cmd = [string]$u.UninstallString
  if ($cmd -match '^\s*"([^"]+)"\s*(.*)$') { $exe = $Matches[1]; $extra = $Matches[2] }
  elseif ($cmd -match '^\s*(\S+)\s*(.*)$') { $exe = $Matches[1]; $extra = $Matches[2] }
  else { Fail "cannot parse UninstallString: $cmd" }
  if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { Fail "uninstaller not found: $exe" }
  $uArgs = (@($extra.Trim(), '/S') | Where-Object { $_ }) -join ' '
  Info "running the uninstaller silently: $exe $uArgs"
  $proc = Start-Process -FilePath $exe -ArgumentList $uArgs -Wait -PassThru
  if ($proc.ExitCode -ne 0) { Fail "uninstaller exited with code $($proc.ExitCode)" }
}

# The NSIS uninstaller can finish its own cleanup a moment after it exits, so poll briefly.
$leftovers = @()
for ($i = 0; $i -lt 20; $i++) {
  $leftovers = @()
  if (Test-Path -LiteralPath $installDir) { $leftovers += "install dir: $installDir" }
  if (Test-Path -LiteralPath $desktopLnk) { $leftovers += "Desktop shortcut: $desktopLnk" }
  if (Test-Path -LiteralPath $startLnk) { $leftovers += "Start Menu shortcut: $startLnk" }
  $left = Get-WattEntries
  if ($left.Count -gt 0) { $leftovers += "Installed apps entry: $($left[0].Name)" }
  if ($leftovers.Count -eq 0) { break }
  [System.Threading.Thread]::Sleep(500)
}

if ($leftovers.Count -gt 0) {
  foreach ($msg in $leftovers) { [Console]::Error.WriteLine("error: still present: $msg") }
  Fail 'the uninstall did not remove everything'
}
Write-Output 'Watt uninstalled: install dir, shortcuts and Installed apps entry are gone.'
Write-Output "Kept (your data, if present): $env:APPDATA\dev.watt.terminal and $env:LOCALAPPDATA\dev.watt.terminal"
