#!/bin/bash
# Checks the built .deb and .rpm: payload files and modes, the PipeWire runtime dependency,
# the unit lines, and that the packaged binary starts from an extracted root.
# usage: deploy/check-packages.sh <dir holding the .deb and .rpm> [expected version]
set -euo pipefail
dir=${1:?usage: check-packages.sh <package dir> [version]}
deb=$(ls "$dir"/*.deb)
rpm=$(ls "$dir"/*.rpm)
fail=0
need() { # need <label> <haystack> <pattern>
    if grep -Eq -- "$3" <<<"$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

contents=$(dpkg-deb -c "$deb")
need "deb binary is 0755" "$contents" '^-rwxr-xr-x .* \./usr/bin/screendockd$'
need "deb udev rule is 0644" "$contents" '^-rw-r--r-- .* \./lib/udev/rules\.d/99-screendock\.rules$'
need "deb user unit is 0644" "$contents" '^-rw-r--r-- .* \./usr/lib/systemd/user/screendockd\.service$'
need "deb config template is 0644" "$contents" '^-rw-r--r-- .* \./etc/screendockd/screendock_cfg\.toml$'
need "deb ships the user docs" "$contents" '\./usr/share/doc/screendockd/README\.md$'
need "deb depends on libpipewire" "$(dpkg-deb -f "$deb" Depends)" 'libpipewire-0\.3-0'
need "deb recommends the portal" "$(dpkg-deb -f "$deb" Recommends)" 'xdg-desktop-portal'

rpm_files=$(rpm -qplv "$rpm")
need "rpm binary is 0755" "$rpm_files" '^-rwxr-xr-x .* /usr/bin/screendockd$'
need "rpm user unit is 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/systemd/user/screendockd\.service$'
need "rpm udev rule is 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/udev/rules\.d/99-screendock\.rules$'
need "rpm config template is 0644" "$rpm_files" '^-rw-r--r-- .* /etc/screendockd/screendock_cfg\.toml$'
need "rpm ships the user docs" "$rpm_files" '/usr/share/doc/screendockd/README\.md$'
need "rpm requires libpipewire" "$(rpm -qpR "$rpm")" 'libpipewire-0\.3\.so\.0'
need "rpm requires pipewire-libs" "$(rpm -qpR "$rpm")" '^pipewire-libs'

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
dpkg-deb -x "$deb" "$root"
unit="$root/usr/lib/systemd/user/screendockd.service"
need "unit has ExecReload" "$(cat "$unit")" '^ExecReload=/usr/bin/screendockd ctl reload$'
need "unit has ExecStopPost" "$(cat "$unit")" '^ExecStopPost=/usr/bin/screendockd vdisplay cleanup$'
version=$("$root/usr/bin/screendockd" --version)
need "extracted binary reports its version" "$version" "^screendockd ${2:-[0-9]+\.[0-9]+\.[0-9]+}\$"
cfg="$root/etc/screendockd/screendock_cfg.toml"
"$root/usr/bin/screendockd" --config "$cfg" ctl --help >/dev/null && echo "ok   ctl --help runs" || { echo "FAIL ctl --help"; fail=1; }
exit $fail
