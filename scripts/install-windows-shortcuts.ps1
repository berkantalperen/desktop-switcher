<#
.SYNOPSIS
    Create Start Menu shortcuts with global hotkeys for desktop-switcher.

.DESCRIPTION
    Windows only honours a shortcut's hotkey when the shortcut lives in the
    Start Menu or on the Desktop, so they go in the Start Menu.

    Two explicit shortcuts are created, one per destination. `toggle` is
    deliberately not bound: after someone changes an input with the monitor's
    own buttons, a toggle has to guess what "the other one" means, and this
    tool refuses to guess rather than switching to the wrong computer.

    Nothing here needs elevation, and uninstalling is deleting the .lnk files.

.PARAMETER Exe
    Path to desktop-switcher.exe. Defaults to whatever is on PATH.

.EXAMPLE
    .\install-windows-shortcuts.ps1
    .\install-windows-shortcuts.ps1 -Exe C:\tools\desktop-switcher.exe
#>
[CmdletBinding()]
param(
    [string] $Exe,
    [string] $ToWindowsHotkey = 'CTRL+ALT+1',
    [string] $ToUbuntuHotkey  = 'CTRL+ALT+2',
    [switch] $Uninstall
)

$ErrorActionPreference = 'Stop'

$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Desktop Switcher'

if ($Uninstall) {
    if (Test-Path $startMenu) {
        Remove-Item $startMenu -Recurse -Force
        Write-Host "Removed $startMenu"
    } else {
        Write-Host "Nothing to remove."
    }
    return
}

if (-not $Exe) {
    $found = Get-Command desktop-switcher -ErrorAction SilentlyContinue
    if (-not $found) {
        throw "desktop-switcher is not on PATH. Pass -Exe with the full path to the binary."
    }
    $Exe = $found.Source
}
if (-not (Test-Path $Exe)) { throw "No such file: $Exe" }

# Resolve the destination names from the configuration rather than assuming
# them, since they are whatever was chosen during `configure`.
$configPath = Join-Path $env:APPDATA 'desktop-switcher\config\config.toml'
if (-not (Test-Path $configPath)) {
    throw "No configuration at $configPath. Run `desktop-switcher configure` first."
}
$destinations = Select-String -Path $configPath -Pattern '^\[monitors\.destinations\.(.+)\]$' |
    ForEach-Object { $_.Matches[0].Groups[1].Value } |
    Sort-Object -Unique
if ($destinations.Count -lt 2) {
    throw "Expected two destinations in the configuration, found: $($destinations -join ', ')"
}

$self = (Select-String -Path $configPath -Pattern '^self_destination\s*=\s*"(.+)"$').Matches[0].Groups[1].Value
$other = $destinations | Where-Object { $_ -ne $self } | Select-Object -First 1

New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
$shell = New-Object -ComObject WScript.Shell

function New-SwitcherShortcut {
    param([string] $Name, [string] $Destination, [string] $Hotkey)

    $path = Join-Path $startMenu "$Name.lnk"
    $lnk = $shell.CreateShortcut($path)
    $lnk.TargetPath = $Exe
    $lnk.Arguments = "switch $Destination"
    $lnk.WorkingDirectory = Split-Path $Exe
    $lnk.Description = "Point the monitors at $Destination"
    $lnk.Hotkey = $Hotkey
    # Hotkey-launched shortcuts flash a console window otherwise.
    $lnk.WindowStyle = 7
    $lnk.Save()
    Write-Host ("{0,-28} {1,-14} -> switch {2}" -f $Name, $Hotkey, $Destination)
}

New-SwitcherShortcut -Name "Monitors to $self"  -Destination $self  -Hotkey $ToWindowsHotkey
New-SwitcherShortcut -Name "Monitors to $other" -Destination $other -Hotkey $ToUbuntuHotkey

Write-Host ""
Write-Host "Installed in: $startMenu"
Write-Host "Hotkeys take effect immediately and survive a reboot."
Write-Host "Remove them again with: .\install-windows-shortcuts.ps1 -Uninstall"
