<#
.SYNOPSIS
    Bind your configured actions to global hotkeys.

.DESCRIPTION
    Reads the actions out of your configuration and creates one Start Menu
    shortcut per action that has a hotkey. Windows only honours a shortcut's
    hotkey when the shortcut lives in the Start Menu or on the Desktop, which
    is why they go there.

    Actions are named by you, so this script invents no names of its own and
    assumes nothing about what is plugged into which input.

    Nothing here needs elevation, and uninstalling is deleting the .lnk files.

.PARAMETER Exe
    Path to desktop-switcher.exe. Defaults to whatever is on PATH.

.EXAMPLE
    .\install-windows-shortcuts.ps1
    .\install-windows-shortcuts.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    [string] $Exe,
    [switch] $Uninstall
)

$ErrorActionPreference = 'Stop'

$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Desktop Switcher'

if ($Uninstall) {
    if (Test-Path $startMenu) {
        # Leave any GUI shortcut alone; only the hotkey ones are ours to remove.
        Get-ChildItem $startMenu -Filter '*.lnk' |
            Where-Object { $_.BaseName -ne 'Desktop Switcher' } |
            ForEach-Object { Remove-Item $_.FullName -Force; Write-Host "Removed $($_.BaseName)" }
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

$configPath = Join-Path $env:APPDATA 'desktop-switcher\config\config.toml'
if (-not (Test-Path $configPath)) {
    throw "No configuration at $configPath. Run ``desktop-switcher configure`` first."
}

# Walk the [[actions]] blocks, pairing each name with its hotkey. Only actions
# that have a hotkey get a shortcut; the rest are run from the GUI or the
# command line.
$actions = @()
$current = $null
foreach ($line in Get-Content $configPath) {
    if ($line -match '^\s*\[\[actions\]\]\s*$') {
        if ($current -and $current.Name -and $current.Hotkey) { $actions += $current }
        $current = [pscustomobject]@{ Name = $null; Hotkey = $null }
        continue
    }
    if ($line -match '^\s*\[\[?[a-z]' -and $line -notmatch '^\s*\[\[actions\.steps\]\]') {
        # Any other top-level table ends the actions section.
        if ($current -and $current.Name -and $current.Hotkey) { $actions += $current; $current = $null }
    }
    if (-not $current) { continue }
    if ($line -match '^\s*name\s*=\s*"(.*)"\s*$')   { $current.Name   = $Matches[1] }
    if ($line -match '^\s*hotkey\s*=\s*"(.*)"\s*$') { $current.Hotkey = $Matches[1] }
}
if ($current -and $current.Name -and $current.Hotkey) { $actions += $current }

if ($actions.Count -eq 0) {
    Write-Host "No action has a hotkey yet."
    Write-Host "Add one in the GUI (Actions tab), save, then run this again."
    return
}

New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
$shell = New-Object -ComObject WScript.Shell

foreach ($action in $actions) {
    # A shortcut filename cannot contain the characters an action name may.
    $safe = ($action.Name -replace '[\\/:*?"<>|]', '-').Trim()
    $path = Join-Path $startMenu "$safe.lnk"

    $lnk = $shell.CreateShortcut($path)
    $lnk.TargetPath = $Exe
    $lnk.Arguments = "run-action `"$($action.Name)`""
    $lnk.WorkingDirectory = Split-Path $Exe
    $lnk.Description = "Run the `"$($action.Name)`" action"
    $lnk.Hotkey = $action.Hotkey
    # Hotkey-launched shortcuts flash a console window otherwise.
    $lnk.WindowStyle = 7
    $lnk.Save()

    Write-Host ("{0,-32} {1,-16} -> run-action" -f $action.Name, $action.Hotkey)
}

Write-Host ""
Write-Host "Installed in: $startMenu"
Write-Host "Hotkeys take effect immediately and survive a reboot."
Write-Host "Remove them again with: .\install-windows-shortcuts.ps1 -Uninstall"
