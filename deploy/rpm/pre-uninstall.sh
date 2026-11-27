if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then
    systemctl stop screendockd-system.service screendock-terminal.service screendock-xorg.service || true
fi
exit 0
