# Build Watt as a per-user NSIS installer for this Windows PC.
# Run with: npm run app:build:win
# The last line of stdout is INSTALLER=<path to the .exe>. scripts/app-install.ps1 uses that path.
$ErrorActionPreference = 'Stop'

function Info([string]$msg) { Write-Output "==> $msg" }

function Fail([string]$msg) {
  [Console]::Error.WriteLine("error: $msg")
  exit 1
}

if ($env:OS -ne 'Windows_NT') { Fail 'this script builds the Windows installer and needs Windows' }

$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

# rustup installs cargo here; it may not be on PATH in a fresh shell.
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path -LiteralPath $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }

Info 'preflight'
foreach ($tool in @('node', 'npm.cmd', 'npx.cmd', 'cargo')) {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
    Fail "$tool not found on PATH (Rust: https://rustup.rs, Node.js: https://nodejs.org)"
  }
}

if (-not (Test-Path -LiteralPath (Join-Path $root 'node_modules'))) {
  Fail 'node_modules is missing. Run: npm ci'
}

$pkgVersion = (Get-Content -LiteralPath (Join-Path $root 'package.json') -Raw | ConvertFrom-Json).version
$confVersion = (Get-Content -LiteralPath (Join-Path $root 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
$cargoText = Get-Content -LiteralPath (Join-Path $root 'src-tauri\Cargo.toml') -Raw
if ($cargoText -notmatch '(?ms)^\[package\]\s*\r?\n.*?^version\s*=\s*"([^"]+)"') {
  Fail 'could not read the version from src-tauri\Cargo.toml'
}
$cargoVersion = $Matches[1]
if ($pkgVersion -ne $confVersion -or $pkgVersion -ne $cargoVersion) {
  Fail "version mismatch: package.json $pkgVersion, src-tauri\tauri.conf.json $confVersion, src-tauri\Cargo.toml $cargoVersion"
}
Info "version $pkgVersion (package.json, tauri.conf.json and Cargo.toml agree)"
Info "node $(node --version), npm $(npm.cmd --version), $(cargo --version)"

Info "building Watt $pkgVersion (NSIS installer, per-user). The frontend build runs first, through beforeBuildCommand."
$started = Get-Date
& npx.cmd tauri build --bundles nsis
if ($LASTEXITCODE -ne 0) { Fail "tauri build failed (exit code $LASTEXITCODE)" }

$nsisDir = Join-Path $root 'src-tauri\target\release\bundle\nsis'
$installer = Get-ChildItem -LiteralPath $nsisDir -Filter "Watt_${pkgVersion}_*-setup.exe" -File -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { Fail "expected installer not found: $nsisDir\Watt_${pkgVersion}_*-setup.exe" }

$sizeMB = [math]::Round($installer.Length / 1MB, 1)
# .NET directly: Get-FileHash is not found when a PowerShell 7 module path leaks into powershell.exe.
$sha = [System.Security.Cryptography.SHA256]::Create()
$stream = [System.IO.File]::OpenRead($installer.FullName)
try { $hash = [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '') } finally { $stream.Dispose(); $sha.Dispose() }

Write-Output ''
Write-Output "Watt $pkgVersion built in $([math]::Round(((Get-Date) - $started).TotalMinutes, 1)) min"
Write-Output "  installer:  $($installer.FullName)"
Write-Output "  size:       $sizeMB MB ($($installer.Length) bytes)"
Write-Output "  sha256:     $hash"
Write-Output "INSTALLER=$($installer.FullName)"
