#!/bin/bash
# Checks the built .deb and .rpm: payload files and modes, the PipeWire and X server runtime
# dependencies, config files, maintainer scripts, the unit lines, the static checks of the system
# terminal files, and that the packaged binary starts from an extracted root.
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
need "deb system units are 0644" "$contents" '^-rw-r--r-- .* \./usr/lib/systemd/system/screendock-xorg\.service$'
need "deb terminal unit is 0644" "$contents" '^-rw-r--r-- .* \./usr/lib/systemd/system/screendock-terminal\.service$'
need "deb system daemon unit is 0644" "$contents" '^-rw-r--r-- .* \./usr/lib/systemd/system/screendockd-system\.service$'
need "deb activation rule is 0644" "$contents" '^-rw-r--r-- .* \./lib/udev/rules\.d/98-screendock-system\.rules$'
need "deb xorg.conf is 0644" "$contents" '^-rw-r--r-- .* \./etc/screendockd/xorg\.conf$'
need "deb xorg-start is 0755" "$contents" '^-rwxr-xr-x .* \./usr/libexec/screendockd/xorg-start$'
need "deb terminal-start is 0755" "$contents" '^-rwxr-xr-x .* \./usr/libexec/screendockd/terminal-start$'
need "deb xorg.conf is a conffile" "$(dpkg-deb -I "$deb" conffiles)" '^/etc/screendockd/xorg\.conf$'
for script in postinst prerm postrm; do
    need "deb has $script" "$(dpkg-deb -I "$deb")" "^ +[0-9]+ bytes, +[0-9]+ lines +\\*  $script "
done
deb_depends=$(dpkg-deb -f "$deb" Depends)
for dep in xserver-xorg-core xserver-xorg-video-dummy xserver-xorg-input-libinput xauth x11-xserver-utils xterm login util-linux; do
    need "deb depends on $dep" "$deb_depends" "(^|, )$dep(,|\$| )"
done

rpm_files=$(rpm -qplv "$rpm")
need "rpm binary is 0755" "$rpm_files" '^-rwxr-xr-x .* /usr/bin/screendockd$'
need "rpm user unit is 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/systemd/user/screendockd\.service$'
need "rpm udev rule is 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/udev/rules\.d/99-screendock\.rules$'
need "rpm config template is 0644" "$rpm_files" '^-rw-r--r-- .* /etc/screendockd/screendock_cfg\.toml$'
need "rpm ships the user docs" "$rpm_files" '/usr/share/doc/screendockd/README\.md$'
need "rpm requires libpipewire" "$(rpm -qpR "$rpm")" 'libpipewire-0\.3\.so\.0'
need "rpm requires pipewire-libs" "$(rpm -qpR "$rpm")" '^pipewire-libs'
need "rpm system units are 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/systemd/system/screendock-xorg\.service$'
need "rpm terminal unit is 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/systemd/system/screendock-terminal\.service$'
need "rpm system daemon unit is 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/systemd/system/screendockd-system\.service$'
need "rpm activation rule is 0644" "$rpm_files" '^-rw-r--r-- .* /usr/lib/udev/rules\.d/98-screendock-system\.rules$'
need "rpm xorg.conf is 0644" "$rpm_files" '^-rw-r--r-- .* /etc/screendockd/xorg\.conf$'
need "rpm xorg-start is 0755" "$rpm_files" '^-rwxr-xr-x .* /usr/libexec/screendockd/xorg-start$'
need "rpm terminal-start is 0755" "$rpm_files" '^-rwxr-xr-x .* /usr/libexec/screendockd/terminal-start$'
need "rpm xorg.conf is a noreplace config file" "$(rpm -qp --queryformat '[%{FILENAMES} %{FILEFLAGS:fflags}\n]' "$rpm")" '^/etc/screendockd/xorg\.conf c?n'
need "rpm pre-install creates the user" "$(rpm -qp --scripts "$rpm")" 'useradd --system --gid screendock --groups input'
need "rpm post-install reloads udev" "$(rpm -qp --scripts "$rpm")" 'udevadm trigger --action=add --subsystem-match=hidraw'
need "rpm pre-uninstall stops the services" "$(rpm -qp --scripts "$rpm")" 'systemctl stop screendockd-system\.service'
rpm_requires=$(rpm -qpR "$rpm")
for dep in xorg-x11-server-Xorg xorg-x11-drv-dummy xorg-x11-drv-libinput /usr/bin/xauth /usr/bin/xset /usr/bin/xsetroot /usr/bin/xterm; do
    need "rpm requires $dep" "$rpm_requires" "^$dep"
done

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
dpkg-deb -x "$deb" "$root"
unit="$root/usr/lib/systemd/user/screendockd.service"
need "unit has ExecReload" "$(cat "$unit")" '^ExecReload=/usr/bin/screendockd ctl reload$'
need "unit has ExecStopPost" "$(cat "$unit")" '^ExecStopPost=/usr/bin/screendockd vdisplay cleanup$'
version=$("$root/usr/bin/screendockd" --version)
need "extracted binary reports its version" "$version" "^screendockd ${2:-[0-9]+\.[0-9]+\.[0-9]+}\$"
cfg="$root/etc/screendockd/screendock_cfg.toml"
echo "--- static checks of the system terminal files from the package"
"$(dirname "$0")/check-deploy.sh" "$root" || fail=1
"$root/usr/bin/screendockd" --config "$cfg" ctl --help >/dev/null && echo "ok   ctl --help runs" || { echo "FAIL ctl --help"; fail=1; }
exit $fail
