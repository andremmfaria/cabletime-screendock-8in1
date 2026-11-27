if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
    udevadm control --reload || true
    udevadm trigger --action=add --subsystem-match=hidraw || true
fi
exit 0
