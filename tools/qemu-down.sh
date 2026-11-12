#!/usr/bin/env bash
# Stop the QEMU QNX target started by tools/qemu-up.sh.
set -euo pipefail
: "${QNX_IMAGE_DIR:?run 'source tools/env.sh' first}"
pidfile="$QNX_IMAGE_DIR/output/qemu.pid"
if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
  ssh -i "$QNX_SSH_KEY" -p "$QNX_SSH_PORT" -o BatchMode=yes -o StrictHostKeyChecking=no \
      -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=3 \
      root@127.0.0.1 'shutdown -b' 2>/dev/null || true
  for _ in $(seq 1 10); do kill -0 "$(cat "$pidfile")" 2>/dev/null || break; sleep 1; done
  kill "$(cat "$pidfile")" 2>/dev/null || true
  echo "QNX target stopped"
else
  echo "QNX target not running"
fi
rm -f "$pidfile"
