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
    # A missing ddcutil is not a reason to stop: the kernel, permission and
    # connector evidence below is exactly what explains *why* it will or will
    # not work once installed, and it is the same evidence either way.
    HAVE_DDCUTIL=0
    if command -v ddcutil >/dev/null 2>&1; then
        HAVE_DDCUTIL=1
        printf 'ddcutil path: %s\n' "$(command -v ddcutil)"
        capture ddcutil --version
    else
        printf 'ddcutil is NOT installed.\n'
        printf 'To install:  sudo apt install ddcutil\n'
        printf 'Continuing with the checks that do not need it.\n'
    fi

    section "graphics hardware"
    capture lspci -nn -k -d ::0300

    section "i2c kernel support and permissions"
    # i2c-dev may be a module OR built into the kernel. Testing lsmod alone
    # reports a false alarm on a kernel that has it builtin, which is the case
    # on the HP Z4 this was written for. Test what actually matters: whether
    # the device nodes exist.
    printf -- '--- is i2c-dev available? ---\n'
    if lsmod 2>/dev/null | grep -q '^i2c_dev'; then
        printf 'i2c_dev: loaded as a module\n'
    elif modinfo i2c-dev 2>/dev/null | grep -qi 'builtin'; then
        printf 'i2c_dev: built into the kernel, nothing to load\n'
    elif ls /dev/i2c-* >/dev/null 2>&1; then
        printf 'i2c_dev: not listed by lsmod, but /dev/i2c-* exists, so it is present\n'
    else
        printf 'i2c_dev: NOT available, and no /dev/i2c-* device nodes exist\n'
        printf 'To load now:        sudo modprobe i2c-dev\n'
        printf 'To load at boot:    echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf\n'
    fi

    printf '\n--- /dev/i2c-* devices ---\n'
    ls -l /dev/i2c-* 2>&1

    # Which buses are graphics DDC lines rather than chipset SMBus. Only the
    # former can reach a monitor, and on some driver stacks none are exposed.
    printf '\n--- i2c adapter names ---\n'
    for bus in /sys/bus/i2c/devices/i2c-*; do
        [ -e "$bus/name" ] || continue
        printf '%-10s %s\n' "$(basename "$bus")" "$(cat "$bus/name" 2>/dev/null)"
    done

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
    printf '%-24s %-14s %-10s %s\n' "CONNECTOR" "STATUS" "EDID" "ENABLED"
    for card in /sys/class/drm/card*-*; do
        [ -e "$card/status" ] || continue
        printf '%-24s %-14s %-10s %s\n' \
            "$(basename "$card")" \
            "$(cat "$card/status" 2>/dev/null)" \
            "$(stat -c '%s bytes' "$card/edid" 2>/dev/null || echo '-')" \
            "$(cat "$card/enabled" 2>/dev/null || echo '-')"
    done
    printf '\nAn EDID of 0 bytes is normal with the proprietary NVIDIA driver:\n'
    printf 'it does not publish EDID through sysfs, so identity has to come\n'
    printf 'from ddcutil instead.\n'

    printf '\n--- EDID from sysfs, where the driver publishes it ---\n'
    for card in /sys/class/drm/card*-*; do
        [ -s "$card/edid" ] || continue
        printf '%s: ' "$(basename "$card")"
        if command -v xxd >/dev/null 2>&1; then
            xxd -p "$card/edid" | tr -d '\n'
            printf '\n'
        else
            printf '(install xxd to dump it)\n'
        fi
    done

    if [ "$HAVE_DDCUTIL" -eq 0 ]; then
        section "end (ddcutil not installed)"
        printf 'Install ddcutil and re-run this script to capture monitor identity,\n'
        printf 'capabilities and the current input source.\n'
        printf 'No VCP value was written by this script.\n'
        exit 0
    fi

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
