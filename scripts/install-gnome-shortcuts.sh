#!/usr/bin/env bash
# Create GNOME custom keyboard shortcuts for desktop-switcher.
#
# Uses GNOME's own custom-shortcut mechanism rather than a global-hotkey
# library, because under Wayland an application cannot reliably grab keys for
# itself and GNOME is the only thing that can.
#
# Two explicit shortcuts, one per destination. `toggle` is deliberately not
# bound: after someone changes an input with the monitor's own buttons, a
# toggle has to guess what "the other one" means, and this tool refuses to
# guess rather than switching to the wrong computer.
#
# Usage:  ./install-gnome-shortcuts.sh [--uninstall]
# Needs no sudo. Removing the shortcuts is --uninstall, or the Settings UI.

set -u

BINDING_SELF='<Control><Alt>1'
BINDING_OTHER='<Control><Alt>2'
BASE=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings
SCHEMA=org.gnome.settings-daemon.plugins.media-keys

# Over SSH there is no session bus in the environment, but the shortcuts
# belong to the logged-in graphical session, so point at its bus explicitly.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
fi

if ! command -v gsettings >/dev/null 2>&1; then
    printf 'gsettings is not available; this script only applies to GNOME.\n' >&2
    exit 1
fi

slot_self="$BASE/desktop-switcher-self/"
slot_other="$BASE/desktop-switcher-other/"

if [ "${1:-}" = "--uninstall" ]; then
    existing=$(gsettings get "$SCHEMA" custom-keybindings 2>/dev/null || echo "@as []")
    remaining=$(printf '%s' "$existing" \
        | sed "s|'$slot_self',\\? *||; s|'$slot_other',\\? *||; s|, *\\]|]|")
    gsettings set "$SCHEMA" custom-keybindings "$remaining"
    printf 'Removed the desktop-switcher shortcuts.\n'
    exit 0
fi

BIN=${BIN:-$HOME/.local/bin/desktop-switcher}
if [ ! -x "$BIN" ]; then
    BIN=$(command -v desktop-switcher 2>/dev/null || true)
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
    printf 'desktop-switcher was not found. Set BIN=/path/to/desktop-switcher and retry.\n' >&2
    exit 1
fi

CONFIG="$HOME/.config/desktop-switcher/config.toml"
if [ ! -f "$CONFIG" ]; then
    printf 'No configuration at %s. Run `desktop-switcher configure` first.\n' "$CONFIG" >&2
    exit 1
fi

# Read the destination names rather than assuming them: they are whatever was
# chosen during `configure`.
self_dest=$(sed -n 's/^self_destination *= *"\(.*\)"$/\1/p' "$CONFIG" | head -1)
all_dests=$(sed -n 's/^\[monitors\.destinations\.\(.*\)\]$/\1/p' "$CONFIG" | sort -u)
other_dest=$(printf '%s\n' "$all_dests" | grep -vx "$self_dest" | head -1)

if [ -z "$self_dest" ] || [ -z "$other_dest" ]; then
    printf 'Could not read two destination names from %s (found: %s).\n' \
        "$CONFIG" "$(printf '%s' "$all_dests" | tr '\n' ' ')" >&2
    exit 1
fi

# Register both slots, preserving any custom shortcuts already configured.
existing=$(gsettings get "$SCHEMA" custom-keybindings 2>/dev/null || echo "@as []")
case "$existing" in
    *desktop-switcher-self*) list="$existing" ;;
    "@as []"|"[]") list="['$slot_self', '$slot_other']" ;;
    *) list=$(printf '%s' "$existing" | sed "s|\\]|, '$slot_self', '$slot_other']|") ;;
esac
gsettings set "$SCHEMA" custom-keybindings "$list"

configure_slot() {
    slot=$1; name=$2; command=$3; binding=$4
    gsettings set "$SCHEMA.custom-keybinding:$slot" name "$name"
    gsettings set "$SCHEMA.custom-keybinding:$slot" command "$command"
    gsettings set "$SCHEMA.custom-keybinding:$slot" binding "$binding"
    printf '%-28s %-18s -> %s\n' "$name" "$binding" "$command"
}

configure_slot "$slot_self"  "Monitors to $self_dest"  "$BIN switch $self_dest"  "$BINDING_SELF"
configure_slot "$slot_other" "Monitors to $other_dest" "$BIN switch $other_dest" "$BINDING_OTHER"

printf '\nVisible in Settings > Keyboard > View and Customize Shortcuts > Custom Shortcuts.\n'
printf 'Remove them again with: %s --uninstall\n' "$0"
