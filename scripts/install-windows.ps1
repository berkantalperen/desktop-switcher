<#
.SYNOPSIS
    Install the CLI and GUI, and bind the hotkeys for your actions.

.DESCRIPTION
    Copies both binaries into %LOCALAPPDATA%\Programs\Desktop Switcher, adds a
    Start Menu entry for the GUI, and runs the hotkey installer.

    It stops a running GUI first. Windows locks a running executable, so a
    copy over it fails -- and a failed copy is easy to miss, leaving an old
    binary in place that reads the configuration with last week's rules. That
    happened once; hence this script.

    The GUI does not go in WindowsApps: an Application Control policy can
    block launching it from there. The CLI is copied there as well, because
    that directory is already on PATH for the current user.

    Nothing here needs elevation.

.EXAMPLE
    .\install-windows.ps1
    .\install-windows.ps1 -SkipHotkeys
#>
[CmdletBinding()]
param(
    [string] $Source = (Join-Path $PSScriptRoot '..\target\release'),
    [switch] $SkipHotkeys
)

$ErrorActionPreference = 'Stop'

$cli = Join-Path $Source 'desktop-switcher.exe'
$gui = Join-Path $Source 'desktop-switcher-gui.exe'
foreach ($f in @($cli, $gui)) {
    if (-not (Test-Path $f)) {
        throw "Not found: $f`nBuild first with: cargo build --release"
    }
}

# A running instance holds its own file open, so stop it before copying.
$running = Get-Process desktop-switcher-gui -ErrorAction SilentlyContinue
if ($running) {
    Write-Host "Stopping the running GUI ($($running.Count) instance(s))…"
    $running | Stop-Process -Force
    Start-Sleep -Milliseconds 800
}

$dir = Join-Path $env:LOCALAPPDATA 'Programs\Desktop Switcher'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

foreach ($f in @($cli, $gui)) {
    $dest = Join-Path $dir (Split-Path $f -Leaf)
    Copy-Item $f $dest -Force
    # Verify rather than trust: a silently skipped copy is the whole reason
    # this script exists.
    $a = (Get-Item $f).LastWriteTimeUtc
    $b = (Get-Item $dest).LastWriteTimeUtc
    if ($a -ne $b) { throw "Copy did not take effect: $dest" }
    Write-Host ("  {0,-30} {1}" -f (Split-Path $dest -Leaf), $b.ToLocalTime())
}

# The CLI also goes somewhere already on PATH, so `desktop-switcher` works
# from any directory and from a shortcut.
$onPath = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps'
if (Test-Path $onPath) {
    try {
        Copy-Item $cli (Join-Path $onPath 'desktop-switcher.exe') -Force
        Write-Host "  desktop-switcher.exe           also on PATH"
    } catch {
        Write-Warning "Could not copy the CLI to $onPath ($($_.Exception.Message))."
        Write-Warning "Add $dir to PATH instead if you want to call it by name."
    }
}

$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Desktop Switcher'
New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut((Join-Path $startMenu 'Desktop Switcher.lnk'))
$lnk.TargetPath = Join-Path $dir 'desktop-switcher-gui.exe'
$lnk.WorkingDirectory = $dir
$lnk.Description = 'Configure and drive the monitor switcher'
$lnk.Save()
Write-Host "  Start Menu entry for the GUI"

if (-not $SkipHotkeys) {
    Write-Host ""
    & (Join-Path $PSScriptRoot 'install-windows-shortcuts.ps1') -Exe (Join-Path $dir 'desktop-switcher.exe')
}

Write-Host ""
Write-Host "Installed in: $dir"
