#!/bin/sh
set -e

# Start socat to forward 0.0.0.0:9222 -> 127.0.0.1:9223
# This works around Chromium ignoring --remote-debugging-address in newer versions.
socat TCP-LISTEN:9222,fork,reuseaddr,bind=0.0.0.0 TCP:127.0.0.1:9223 &

exec chromium \
    --headless=new \
    --disable-gpu \
    --disable-dev-shm-usage \
    --no-sandbox \
    --remote-debugging-port=9223 \
    --remote-allow-origins=* \
    --user-data-dir=/tmp/chromium \
    about:blank
