<#
.SYNOPSIS
  Install the amasario binary from this checkout on Windows.

.DESCRIPTION
  Builds from source with the locked dependency set, so an installed binary
  corresponds to the versions this repository pinned and tested rather than to
  whatever the registry resolves to on the day it runs.

  This is the PowerShell counterpart of scripts/install.sh. It performs the same
  steps; the two exist because a shell script is not something cmd.exe or
  PowerShell can run, and asking a Windows contributor to obtain bash is a worse
  answer than shipping the four lines it takes to do the same thing natively.

.PARAMETER Root
  Install into <Root>\bin instead of the cargo default (%USERPROFILE%\.cargo\bin).

.PARAMETER Force
  Reinstall even when the binary is already present.

.PARAMETER CargoArgs
  Additional arguments passed through to `cargo install`.

.EXAMPLE
  scripts/install.ps1
  scripts/install.ps1 -Root C:\tools -Force
#>
[CmdletBinding()]
param(
    [string]$Root,
    [switch]$Force,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$CargoArgs
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo was not found on PATH. Install Rust from https://rustup.rs and try again."
    exit 1
}

# The toolchain is pinned in rust-toolchain.toml. Report the requirement before the
# build starts so an older compiler names itself rather than surfacing as a confusing
# type error.
$toolchainFile = Join-Path $repoRoot 'rust-toolchain.toml'
if (Test-Path $toolchainFile) {
    $match = Select-String -Path $toolchainFile -Pattern '^channel\s*=\s*"(.+)"' | Select-Object -First 1
    if ($match) {
        $required = $match.Matches[0].Groups[1].Value
        $installed = (rustup toolchain list) -join "`n"
        if ($installed -notmatch [regex]::Escape($required)) {
            Write-Host "==> toolchain $required is not installed; fetching it."
            rustup toolchain install $required
        }
    }
}

$installArgs = @(
    'install',
    '--path', (Join-Path $repoRoot 'crates/amasario-cli'),
    '--locked'
)

if ($Root) { $installArgs += @('--root', $Root) }
if ($Force) { $installArgs += '--force' }
if ($CargoArgs) { $installArgs += $CargoArgs }

Write-Host "==> cargo $($installArgs -join ' ')"
& cargo @installArgs
if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo install failed with exit code $LASTEXITCODE."
    exit $LASTEXITCODE
}

$binDir = if ($Root) { Join-Path $Root 'bin' } else { Join-Path $env:USERPROFILE '.cargo\bin' }
Write-Host ""
Write-Host "Installed. If '$binDir' is not on your PATH, add it, then run:"
Write-Host "  amasario --help"
