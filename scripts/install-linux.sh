#!/usr/bin/env bash
# Install desktop-switcher for the current user on Linux: the CLI, the
# settings window and the top-bar icon, an app-menu entry, the icon starting
# with the session, and GNOME shortcuts for your actions' hotkeys.
#
# Run after `cargo build --release`, or from an unpacked release, where the
# programs sit next to this script. Needs no sudo: everything goes under
# ~/.local and ~/.config, and your configuration is never touched.
#
# Usage:  scripts/install-linux.sh [--uninstall]

set -eu

here=$(cd "$(dirname "$0")" && pwd)
if [ -x "$here/desktop-switcher" ]; then
    default_source=$here
else
    default_source=$here/../target/release
fi
source_dir=${SOURCE:-$default_source}
bin=${BIN_DIR:-$HOME/.local/bin}
programs=(desktop-switcher desktop-switcher-gui desktop-switcher-tray)

# Over SSH there is no session bus in the environment, but the icon and the
# shortcuts belong to the logged-in graphical session.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
fi

# Not `pkill -x name`: Linux truncates process names to 15 characters, so a
# name like desktop-switcher-tray never matches. The tray is found exactly,
# through the D-Bus name it holds while running; the settings window by its
# full command line, which is how its app-menu entry starts it.
stop_running() {
    local stopped=0 pid
    pid=$(gdbus call --session --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.GetConnectionUnixProcessID \
        io.github.berkantalperen.DesktopSwitcherTray 2>/dev/null |
        sed -n 's/.*uint32 \([0-9][0-9]*\).*/\1/p' || true)
    if [ -n "$pid" ] && kill "$pid" 2>/dev/null; then stopped=1; fi
    if pkill -xf "$bin/desktop-switcher-gui" 2>/dev/null; then stopped=1; fi
    if [ "$stopped" = 1 ]; then sleep 1; fi
}

if [ "${1:-}" = "--uninstall" ]; then
    stop_running
    if [ -x "$bin/desktop-switcher-tray" ]; then
        "$bin/desktop-switcher-tray" --uninstall || true
    fi
    "$here/install-gnome-shortcuts.sh" --uninstall || true
    for p in "${programs[@]}"; do rm -f "$bin/$p"; done
    echo "Removed. Your configuration in ~/.config/desktop-switcher is left alone."
    exit 0
fi

for p in "${programs[@]}"; do
    if [ ! -x "$source_dir/$p" ]; then
        echo "Not found: $source_dir/$p -- build first with: cargo build --release" >&2
        exit 1
    fi
done

# A running copy is stopped first: it would otherwise keep running the old
# code, and writing over a running executable fails.
stop_running

mkdir -p "$bin"
for p in "${programs[@]}"; do
    install -m755 "$source_dir/$p" "$bin/$p"
    # Verify rather than trust.
    if ! cmp -s "$source_dir/$p" "$bin/$p"; then
        echo "Copy did not take effect: $bin/$p" >&2
        exit 1
    fi
    printf '  %-26s installed\n' "$p"
done

"$bin/desktop-switcher-tray" --install

# Start the icon now. Under the user's service manager when there is one, so
# it outlives the shell (or SSH session) that ran this; detached otherwise.
if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --collect --quiet --unit="desktop-switcher-tray-$$" \
        "$bin/desktop-switcher-tray" 2>/dev/null; then
    :
else
    setsid "$bin/desktop-switcher-tray" </dev/null >/dev/null 2>&1 &
fi
echo "  top-bar icon started, and set to start with your session"

echo
"$here/install-gnome-shortcuts.sh"
