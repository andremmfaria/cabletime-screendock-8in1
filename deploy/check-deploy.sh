#!/bin/bash
# Static checks of the system terminal files; needs no root and no hardware.
# - shell syntax of the launchers and their dry runs
# - udev syntax of the activation rule (udevadm verify)
# - systemd-analyze verify of the three units against a root that holds the launchers
# usage: deploy/check-deploy.sh [root]
#   With a root (for example an extracted package) the units and launchers are taken from it.
#   Without one, a temporary root is built from the files in deploy/.
set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
run() { # run <label> <command...>
    local label=$1
    shift
    if "$@" >/dev/null 2>&1; then ok "$label"; else bad "$label"; fi
}

root=${1:-}
tmp=
if [ -z "$root" ]; then
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    root=$tmp
    mkdir -p "$root/usr/lib/systemd/system" "$root/usr/libexec/screendockd" "$root/usr/bin" "$root/lib/udev/rules.d"
    cp "$here"/screendock-xorg.service "$here"/screendock-terminal.service "$here"/screendockd-system.service \
        "$root/usr/lib/systemd/system/"
    cp "$here"/libexec/* "$root/usr/libexec/screendockd/"
    printf '#!/bin/sh\n' >"$root/usr/bin/screendockd"
    chmod 755 "$root/usr/bin/screendockd"
    cp "$here"/98-screendock-system.rules "$here"/99-screendock.rules "$root/lib/udev/rules.d/"
fi

libexec=$root/usr/libexec/screendockd
for script in xorg-start terminal-start; do
    run "$script passes sh -n" sh -n "$libexec/$script"
done
run "xorg-start dry run" env XORG_CANDIDATES=/bin/sh "$libexec/xorg-start" --dry-run
run "terminal-start dry run" "$libexec/terminal-start" --dry-run
if "$libexec/terminal-start" --dry-run 2>/dev/null | grep -Eq -- '-e [^ ]*setsid -wc [^ ]*login$'; then
    ok "terminal-start dry run runs login under setsid -wc"
else
    bad "terminal-start dry run runs login under setsid -wc"
fi
# Regression guard: login without its own session exits at once under xterm.
if grep -Eq '^ *-e "\$\(bin setsid\)" -wc "\$\(bin login\)"$' "$libexec/terminal-start"; then
    ok "terminal-start exec line has setsid -wc"
else
    bad "terminal-start exec line lacks setsid -wc"
fi
if XORG_CANDIDATES=/nonexistent "$libexec/xorg-start" --dry-run >/dev/null 2>&1; then
    bad "xorg-start fails without an Xorg binary"
else
    ok "xorg-start fails without an Xorg binary"
fi

# 99-screendock.rules names the screendock group, which only exists after the package is installed.
rules=$root/lib/udev/rules.d/98-screendock-system.rules
if [ -f "$rules" ] && command -v udevadm >/dev/null; then
    run "udevadm verify 98-screendock-system.rules" udevadm verify "$rules"
fi

if command -v systemd-analyze >/dev/null; then
    # The units are checked as copies whose executable paths point into the root, so the check
    # does not depend on what is installed on this machine.
    copies=$(mktemp -d)
    for unit in screendock-xorg screendock-terminal screendockd-system; do
        sed "s#=/usr/#=$root/usr/#" "$root/usr/lib/systemd/system/$unit.service" >"$copies/$unit.service"
    done
    out=$(systemd-analyze verify --man=no "$copies"/*.service 2>&1)
    status=$?
    if [ "$status" -eq 0 ] && ! grep -Eq 'not executable|Failed to|Unknown key|Unknown section' <<<"$out"; then
        ok "systemd-analyze verify of the three units"
    else
        bad "systemd-analyze verify of the three units"
        echo "$out"
    fi
    rm -rf "$copies"
else
    echo "skip systemd-analyze verify (systemd-analyze not found)"
fi
exit $fail
