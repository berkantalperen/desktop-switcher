#!/usr/bin/env bash
# Bind your configured actions to GNOME keyboard shortcuts.
#
# Reads the actions out of your configuration and registers one GNOME custom
# shortcut per action that has a hotkey. Actions are named by you, so this
# script invents no names of its own and assumes nothing about what is plugged
# into which input.
#
# GNOME's own custom-shortcut mechanism is used rather than a global-hotkey
# library, because under Wayland an application cannot reliably grab keys for
# itself and GNOME is the only thing that can.
#
# Usage:  ./install-gnome-shortcuts.sh [--uninstall]
# Needs no sudo. Removing them again is --uninstall, or the Settings UI.

set -u

BASE=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings
SCHEMA=org.gnome.settings-daemon.plugins.media-keys
PREFIX=desktop-switcher-

# Over SSH there is no session bus in the environment, but the shortcuts
# belong to the logged-in graphical session, so point at its bus explicitly.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
fi

if ! command -v gsettings >/dev/null 2>&1; then
    printf 'gsettings is not available; this script only applies to GNOME.\n' >&2
    exit 1
fi

current_list() {
    gsettings get "$SCHEMA" custom-keybindings 2>/dev/null || echo "@as []"
}

# Drop every slot this script owns, leaving anyone else's shortcuts alone.
remove_ours() {
    local list
    list=$(current_list)
    printf '%s' "$list" \
        | tr ',' '\n' \
        | sed "s/[][]//g; s/^ *//; s/ *$//; s/^'//; s/'$//" \
        | grep -v "^$" \
        | grep -v "$PREFIX" \
        | awk 'BEGIN{ORS=""; print "["} {if(NR>1) print ", "; printf "%c%s%c", 39, $0, 39} END{print "]"}'
}

if [ "${1:-}" = "--uninstall" ]; then
    gsettings set "$SCHEMA" custom-keybindings "$(remove_ours)"
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

# Walk the [[actions]] blocks, pairing each name with its hotkey. Only actions
# that have one get a shortcut; the rest are run from the GUI or the command
# line.
names=()
keys=()
name=""
hotkey=""
in_action=0

flush() {
    if [ "$in_action" -eq 1 ] && [ -n "$name" ] && [ -n "$hotkey" ]; then
        names+=("$name")
        keys+=("$hotkey")
    fi
    name=""
    hotkey=""
}

while IFS= read -r line; do
    case "$line" in
        '[[actions]]'*)
            flush
            in_action=1
            continue
            ;;
        '[[actions.steps]]'*)
            # Inside the same action; keep whatever we have so far.
            continue
            ;;
        '['*)
            flush
            in_action=0
            continue
            ;;
    esac
    [ "$in_action" -eq 1 ] || continue
    case "$line" in
        name*=*) name=$(printf '%s' "$line" | sed -n 's/^ *name *= *"\(.*\)" *$/\1/p') ;;
        hotkey*=*) hotkey=$(printf '%s' "$line" | sed -n 's/^ *hotkey *= *"\(.*\)" *$/\1/p') ;;
    esac
done < "$CONFIG"
flush

if [ "${#names[@]}" -eq 0 ]; then
    printf 'No action has a hotkey yet.\n'
    printf 'Add one in the GUI (Actions tab), save, then run this again.\n'
    exit 0
fi

# Translate CTRL+ALT+2 into GNOME's <Control><Alt>2.
to_gnome_binding() {
    printf '%s' "$1" | awk -F'+' '{
        out = ""
        for (i = 1; i < NF; i++) {
            m = toupper($i)
            if (m == "CTRL" || m == "CONTROL") out = out "<Control>"
            else if (m == "ALT") out = out "<Alt>"
            else if (m == "SHIFT") out = out "<Shift>"
            else if (m == "WIN" || m == "SUPER" || m == "META") out = out "<Super>"
        }
        print out $NF
    }'
}

slugify() {
    printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]\+/-/g; s/^-//; s/-$//'
}

list=$(remove_ours)
slots=()
for n in "${names[@]}"; do
    slots+=("$BASE/$PREFIX$(slugify "$n")/")
done

# Append our slots to whatever else is configured.
for slot in "${slots[@]}"; do
    case "$list" in
        "[]"|"@as []") list="['$slot']" ;;
        *) list=$(printf '%s' "$list" | sed "s|\\]|, '$slot']|") ;;
    esac
done
gsettings set "$SCHEMA" custom-keybindings "$list"

i=0
while [ "$i" -lt "${#names[@]}" ]; do
    slot="${slots[$i]}"
    n="${names[$i]}"
    binding=$(to_gnome_binding "${keys[$i]}")
    gsettings set "$SCHEMA.custom-keybinding:$slot" name "$n"
    gsettings set "$SCHEMA.custom-keybinding:$slot" command "$BIN run-action \"$n\""
    gsettings set "$SCHEMA.custom-keybinding:$slot" binding "$binding"
    printf '%-32s %-18s -> run-action\n' "$n" "$binding"
    i=$((i + 1))
done

printf '\nVisible in Settings > Keyboard > View and Customize Shortcuts > Custom Shortcuts.\n'
printf 'Remove them again with: %s --uninstall\n' "$0"
