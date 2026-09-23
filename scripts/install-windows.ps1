<#
.SYNOPSIS
    Install the CLI, GUI and tray icon, and bind the hotkeys for your actions.

.DESCRIPTION
    Copies the binaries into %LOCALAPPDATA%\Programs\Desktop Switcher, adds
    Start Menu entries, starts the tray icon and has it start at login, and
    runs the hotkey installer.

    It stops a running GUI and tray first. Windows locks a running
    executable, so a copy over it fails -- and a failed copy is easy to miss,
    leaving an old binary in place that reads the configuration with last
    week's rules. That happened once; hence this script.

    The tray starts at login through the per-user Run key. Turn that off in
    Task Manager > Startup apps, like any other.

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
$tray = Join-Path $Source 'desktop-switcher-tray.exe'
foreach ($f in @($cli, $gui, $tray)) {
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

# Ask the tray to close rather than killing it: a killed tray leaves a ghost
# icon in the notification area until someone hovers over it.
$trayRunning = Get-Process desktop-switcher-tray -ErrorAction SilentlyContinue
if ($trayRunning) {
    Write-Host "Closing the running tray icon…"
    Add-Type -Namespace DesktopSwitcher -Name Win32 -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr FindWindow(string className, string windowName);
[DllImport("user32.dll")]
public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
'@
    $window = [DesktopSwitcher.Win32]::FindWindow('DesktopSwitcherTray', $null)
    if ($window -ne [IntPtr]::Zero) {
        # WM_CLOSE
        [void][DesktopSwitcher.Win32]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    }
    $trayRunning | Wait-Process -Timeout 5 -ErrorAction SilentlyContinue
    Get-Process desktop-switcher-tray -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 300
}

$dir = Join-Path $env:LOCALAPPDATA 'Programs\Desktop Switcher'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

foreach ($f in @($cli, $gui, $tray)) {
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
$trayExe = Join-Path $dir 'desktop-switcher-tray.exe'
$lnk = $shell.CreateShortcut((Join-Path $startMenu 'Desktop Switcher Tray.lnk'))
$lnk.TargetPath = $trayExe
$lnk.WorkingDirectory = $dir
$lnk.Description = 'Put the monitor switcher in the notification area'
$lnk.Save()
Write-Host "  Start Menu entries for the GUI and the tray"

# Start the tray at login, and now.
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
Set-ItemProperty -Path $run -Name 'Desktop Switcher Tray' -Value "`"$trayExe`""
Start-Process $trayExe
Write-Host "  Tray icon started, and set to start at login"

if (-not $SkipHotkeys) {
    Write-Host ""
    & (Join-Path $PSScriptRoot 'install-windows-shortcuts.ps1') -Exe (Join-Path $dir 'desktop-switcher.exe')
}

Write-Host ""
Write-Host "Installed in: $dir"
