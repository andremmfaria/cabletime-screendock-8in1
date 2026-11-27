getent group screendock >/dev/null || groupadd --system screendock
if ! getent passwd screendock >/dev/null; then
    if getent group input >/dev/null; then
        useradd --system --gid screendock --groups input --no-create-home \
            --home-dir /nonexistent --shell /usr/sbin/nologin screendock
    else
        useradd --system --gid screendock --no-create-home \
            --home-dir /nonexistent --shell /usr/sbin/nologin screendock
    fi
fi
exit 0
