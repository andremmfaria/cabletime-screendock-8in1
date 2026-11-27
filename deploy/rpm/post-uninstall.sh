if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
    udevadm control --reload || true
fi
exit 0
