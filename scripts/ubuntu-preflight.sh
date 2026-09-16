#!/usr/bin/env bash
# Stage A evidence capture for the Ubuntu (HP Z4) host.
#
# READ ONLY. This script never runs `setvcp`, never changes an input, and
# never changes system configuration. It only reads and reports. If it tells
# you something is missing, it prints the command to fix it rather than
# running it.
#
# Usage:  ./ubuntu-preflight.sh [output-file]
# Default output: ./ubuntu-preflight-<hostname>-<date>.txt
#
# The output contains monitor model and serial numbers, which are the
# identifiers this project binds to. Review before sharing.

set -u

OUT="${1:-./ubuntu-preflight-$(hostname)-$(date +%Y%m%d-%H%M%S).txt}"

section() {
    printf '\n===== %s =====\n' "$1"
}

# Run a command, recording the exit status. Never aborts the script: a missing
# tool is itself evidence worth capturing.
capture() {
    printf '\n$ %s\n' "$*"
    "$@" 2>&1
    printf '[exit status: %d]\n' "$?"
}

{
    section "context"
    printf 'captured    : %s\n' "$(date -Is)"
    printf 'hostname    : %s\n' "$(hostname)"
    printf 'user        : %s\n' "$(id -un)"
    printf 'groups      : %s\n' "$(id -Gn)"
    capture uname -a
    if [ -r /etc/os-release ]; then
        printf '\n--- /etc/os-release ---\n'
        cat /etc/os-release
    fi
    printf '\nsession type: %s\n' "${XDG_SESSION_TYPE:-unset}"
    printf 'desktop     : %s\n' "${XDG_CURRENT_DESKTOP:-unset}"

    section "ddcutil availability"
    if command -v ddcutil >/dev/null 2>&1; then
        printf 'ddcutil path: %s\n' "$(command -v ddcutil)"
        capture ddcutil --version
    else
        printf 'ddcutil is NOT installed.\n'
        printf 'To install:  sudo apt install ddcutil\n'
        printf 'Stopping here: everything below needs ddcutil.\n'
        exit 0
    fi

    section "i2c kernel support and permissions"
    capture lsmod
    printf '\n--- i2c-dev loaded? ---\n'
    if lsmod 2>/dev/null | grep -q '^i2c_dev'; then
        printf 'i2c_dev: loaded\n'
    else
        printf 'i2c_dev: NOT loaded\n'
        printf 'To load now:        sudo modprobe i2c-dev\n'
        printf 'To load at boot:    echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf\n'
    fi

    printf '\n--- /dev/i2c-* devices ---\n'
    ls -l /dev/i2c-* 2>&1

    printf '\n--- i2c group membership ---\n'
    if getent group i2c >/dev/null 2>&1; then
        printf 'i2c group exists: %s\n' "$(getent group i2c)"
        if id -nG | tr ' ' '\n' | grep -qx i2c; then
            printf 'current user IS in the i2c group\n'
        else
            printf 'current user is NOT in the i2c group\n'
            printf 'To fix (then log out and back in):  sudo usermod -aG i2c %s\n' "$(id -un)"
        fi
    else
        printf 'no i2c group on this system\n'
        printf 'ddcutil ships a udev rule for this; see https://www.ddcutil.com/i2c_permissions/\n'
    fi

    printf '\n--- can we open the i2c devices without sudo? ---\n'
    for dev in /dev/i2c-*; do
        [ -e "$dev" ] || continue
        if [ -r "$dev" ] && [ -w "$dev" ]; then
            printf '%s: readable and writable by %s\n' "$dev" "$(id -un)"
        else
            printf '%s: NOT accessible by %s (would need sudo)\n' "$dev" "$(id -un)"
        fi
    done

    section "drm connectors (which physical port each display uses)"
    for card in /sys/class/drm/card*-*; do
        [ -e "$card/status" ] || continue
        printf '%-28s %s\n' "$(basename "$card")" "$(cat "$card/status" 2>/dev/null)"
    done

    section "ddcutil detect (identity)"
    capture ddcutil detect
    printf '\n--- with --verbose, for EDID detail and any ambiguity ---\n'
    capture ddcutil detect --verbose

    section "per-display capabilities and current input"
    # Iterate over the display numbers ddcutil actually reported, rather than
    # assuming there are exactly two.
    DISPLAYS=$(ddcutil detect 2>/dev/null | sed -n 's/^Display \([0-9]\+\).*/\1/p')
    if [ -z "$DISPLAYS" ]; then
        printf 'No displays detected over DDC/CI.\n'
        printf 'Check that DDC/CI is enabled in each monitor on-screen menu.\n'
    fi
    for d in $DISPLAYS; do
        section "display $d"
        capture ddcutil --display "$d" capabilities
        # The current input source. This is a READ: setvcp is never run here.
        capture ddcutil --display "$d" getvcp 60
        capture ddcutil --display "$d" getvcp 60 --terse
    done

    section "what VCP 60 means in the spec (NOT this monitor's real values)"
    printf 'The list below is what MCCS defines, not what these panels accept.\n'
    printf 'Only the capabilities output above describes these monitors, and even\n'
    printf 'that is a claim rather than proof.\n'
    capture ddcutil vcpinfo 60 --verbose

    section "end"
    printf 'Completed: %s\n' "$(date -Is)"
    printf 'No VCP value was written by this script.\n'
} 2>&1 | tee "$OUT"

printf '\nEvidence written to: %s\n' "$OUT"
