#!/usr/bin/env bash
# Runs the vehicle-signals demo headless and checks that its log shows what the
# demo is meant to prove. Exits non-zero naming the first missing line, and
# prints "demo check passed" on success. The demo is stopped either way.
#
#   tools/demo-check.sh              check only
#   tools/demo-check.sh <file>       also copy the demo's output to <file>, only
#                                    if the check passed
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log="$(mktemp)"
cleanup() {
  # demo.sh stops everything itself; this covers a demo.sh that is missing or
  # was killed half way.
  if [ -f "$root/tools/demo.sh" ]; then
    bash "$root/tools/demo.sh" --stop >/dev/null 2>&1 || true
  fi
  rm -f "$log"
}
trap cleanup EXIT

rc=0
bash "$root/tools/demo.sh" --headless >"$log" 2>&1 || rc=$?
if [ "$rc" -ne 0 ]; then
  cat "$log"
  echo "demo check failed: tools/demo.sh --headless exited $rc" >&2
  exit 1
fi

expect() {
  if ! grep -Eq -- "$1" "$log"; then
    cat "$log"
    echo "demo check failed: missing line: $2" >&2
    exit 1
  fi
}

expect '^vsigd: serving .*\(own priority 20\)$' 'vsigd: serving ... (own priority 20)'
expect 'served safety-client at priority 40$' 'served safety-client at priority 40'
expect 'served infotainment-client at priority 10$' 'served infotainment-client at priority 10'
expect 'restarting infotainment-client$' 'restarting infotainment-client'
expect '^speed_kmh=[0-9]+\.[0-9]$' 'cat /dev/vehicle/speed output (speed_kmh=<n>.<d>)'
expect '^# every request of this run, as vsigd logged it \(distinct lines\):$' \
  '# every request of this run (the closing list of served lines)'

# Every request must have run at its client's priority (40 or 10), never at
# vsigd's own 20 or anything else. demo.sh ends with each distinct served line
# of vsigd's whole log, printed after everything stopped, so this covers every
# request of the run, not only the lines shown in steps 5 and 8.
other="$(grep -E ' served .* at priority ' "$log" | grep -Ev ' served (safety-client at priority 40|infotainment-client at priority 10)$' || true)"
if [ -n "$other" ]; then
  cat "$log"
  echo "demo check failed: unexpected priority line: $(echo "$other" | head -n 1)" >&2
  exit 1
fi

if [ -n "${1:-}" ]; then
  cp "$log" "$1"
fi
echo "demo check passed"
