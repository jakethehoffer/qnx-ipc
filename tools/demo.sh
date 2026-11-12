#!/usr/bin/env bash
# The vehicle-signals demo on the QEMU QNX 8.0 target: builds and deploys the
# programs in demo/ and c/infotainment_client.c, starts them, walks through the
# scenario and prints a narrated log. Everything is stopped at the end.
#
#   tools/demo.sh                 run with a short pause between steps (for video)
#   tools/demo.sh --pause <s>     the same with a pause of <s> seconds
#   tools/demo.sh --headless      run without pauses (tools/demo-check.sh)
#   tools/demo.sh --stop          stop a demo left running and exit
#
# Needs the target running (tools/qemu-up.sh). Safe to run again: it first stops
# anything a previous run left behind.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# shellcheck disable=SC1091
source tools/env.sh

pause_s=2
mode=run
while [ $# -gt 0 ]; do
  case "$1" in
    --headless) pause_s=0 ;;
    --pause) pause_s="${2:?--pause needs a number of seconds}"; shift ;;
    --stop) mode=stop ;;
    *) echo "usage: tools/demo.sh [--headless | --pause <seconds> | --stop]" >&2; exit 2 ;;
  esac
  shift
done

remote_dir=/tmp/vehicle-demo
log="$remote_dir/demo.log"
# The timeouts make a hung target fail the demo instead of hanging it.
ssh_opts=(-i "$QNX_SSH_KEY" -o BatchMode=yes -o StrictHostKeyChecking=no
          -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR
          -o ConnectTimeout=5 -o ServerAliveInterval=5 -o ServerAliveCountMax=3)
target() { ssh "${ssh_opts[@]}" -p "$QNX_SSH_PORT" root@127.0.0.1 "$@"; }

# Stops every demo program (the watchdog first, so it cannot restart a client)
# and waits until /dev/vehicle is gone. Fails if something is left.
stop_demo() {
  target 'slay -f -Q -s SIGKILL watchdog 2>/dev/null
          slay -f -Q -s SIGKILL safety-client 2>/dev/null
          slay -f -Q -s SIGKILL infotainment_client 2>/dev/null
          slay -f -Q -s SIGTERM vsigd 2>/dev/null
          for i in 1 2 3 4 5 6 7 8 9 10; do
            [ -e /dev/vehicle ] || [ -n "$(slay -p vsigd 2>/dev/null)" ] || break
            sleep 0.2
          done
          left="$(for p in watchdog safety-client infotainment_client vsigd; do slay -p $p 2>/dev/null; done)"
          if [ -n "$left" ] || [ -e /dev/vehicle ]; then
            echo "demo: could not stop everything" >&2
            exit 1
          fi'
}

if [ "$mode" = stop ]; then
  stop_demo
  exit 0
fi

pause() { [ "$pause_s" = 0 ] || sleep "$pause_s"; }
# Prints a step header, with a blank line before every step but the first.
first_step=1
step() {
  [ "$first_step" = 1 ] || echo
  first_step=0
  echo "== $1"
}

# The target's demo log, as it is now.
fetch_log() { target "cat $log"; }

# Waits until the demo log has a line matching the ERE $1 after line $2
# (default 0). Fails after about 30 s.
wait_for() {
  local pattern="$1" after="${2:-0}"
  for _ in $(seq 150); do
    if fetch_log | tail -n "+$((after + 1))" | grep -Eq -- "$pattern"; then
      return 0
    fi
    sleep 0.2
  done
  echo "demo: timed out waiting for: $pattern" >&2
  echo "--- demo log ---" >&2
  fetch_log >&2 || true
  return 1
}

# Lines in the demo log after line $1, with the per-request "served" lines
# reduced to the first one for each client.
new_lines() {
  fetch_log | tail -n "+$(($1 + 1))" | awk '/ served / { if (seen[$0]++) next } { print }'
}

started=0
finish() {
  rc=$?
  if [ "$started" = 1 ]; then
    stop_demo >/dev/null 2>&1 || true
  fi
  exit "$rc"
}
trap finish EXIT

# --- 1. build --------------------------------------------------------------
step "1. build: vsigd, watchdog, safety-client (Rust, using qnx-ipc and qnx-resmgr) and infotainment_client (C)"
build_log="$(mktemp)"
if ! { cargo build -j 6 --release --target "$QNX_RUST_TARGET" \
         -p vsigd -p safety-client -p watchdog &&
       mkdir -p "$CARGO_TARGET_DIR/c" &&
       tools/build-c.sh c/infotainment_client.c "$CARGO_TARGET_DIR/c/infotainment_client"
     } >"$build_log" 2>&1; then
  cat "$build_log"
  rm -f "$build_log"
  exit 1
fi
rm -f "$build_log"
bin="$CARGO_TARGET_DIR/$QNX_RUST_TARGET/release"
echo "built all four programs"

# --- 2. deploy -------------------------------------------------------------
step "2. deploy to the QNX 8.0 target: $remote_dir"
started=1
stop_demo >/dev/null
# A program file can stay busy for a moment after its process ended, so retry.
target "for i in 1 2 3 4 5 6 7 8 9 10; do
          rm -rf $remote_dir 2>/dev/null && break
          sleep 0.5
        done
        mkdir -p $remote_dir"
scp -q -O "${ssh_opts[@]}" -P "$QNX_SSH_PORT" \
  "$bin/vsigd" "$bin/watchdog" "$bin/safety-client" "$CARGO_TARGET_DIR/c/infotainment_client" \
  "root@127.0.0.1:$remote_dir/"
echo "copied vsigd, watchdog, safety-client, infotainment_client"
pause

# --- 3. vsigd --------------------------------------------------------------
step "3. start vsigd: named channel \"vehicle\" plus two /dev files"
# vsigd runs at priority 20, apart from both clients' priorities (40 and 10),
# so the log can show its thread moving to each client's priority and back.
# The subshell detaches the program, so ssh returns at once.
target "cd $remote_dir && : > demo.log && (on -p 20 ./vsigd >>demo.log 2>&1 </dev/null &)"
wait_for '^vsigd: serving'
new_lines 0
# vsigd's own priority as it measured and logged it.
vsigd_priority="$(fetch_log | sed -n 's/^vsigd: serving .*(own priority \([0-9]*\))$/\1/p' | head -n 1)"
pause

# --- 4. watchdog and clients -----------------------------------------------
step "4. start the watchdog; it starts both clients"
mark="$(fetch_log | wc -l)"
target "cd $remote_dir && (./watchdog \
          1 safety-client ./safety-client \
          2 infotainment-client ./infotainment_client \
          >>demo.log 2>&1 </dev/null &)"
wait_for 'served safety-client at' "$mark"
wait_for 'served infotainment-client at' "$mark"
new_lines "$mark" | grep -v ' served '
pause

# --- 5. priority inheritance -----------------------------------------------
step "5. priority inheritance: vsigd (own priority ${vsigd_priority:-unknown}) runs each request at its client's priority"
new_lines "$mark" | grep ' served '
pause

# --- 6. files --------------------------------------------------------------
step "6. any program can read the signals as files"
echo "# cat /dev/vehicle/speed"
target "cat /dev/vehicle/speed"
echo "# cat /dev/vehicle/brake"
target "cat /dev/vehicle/brake"
pause

# --- 7. freeze -------------------------------------------------------------
step "7. freeze the infotainment client"
# slay's exit status is the number of matches, so it is not checked.
pid="$(target "slay -p infotainment_client || true")"
if ! [[ "$pid" =~ ^[0-9]+$ ]]; then
  echo "demo: expected one infotainment_client process, found: $pid" >&2
  exit 1
fi
mark="$(fetch_log | wc -l)"
# The real pid changes from run to run; the narration names it instead.
echo "# kill -STOP <infotainment_client pid>"
target "kill -STOP $pid"
pause

# --- 8. watchdog restart ---------------------------------------------------
step "8. its heartbeats stop; the watchdog restarts it"
wait_for 'restarting infotainment-client' "$mark"
restart_line="$(fetch_log | grep -n 'restarting infotainment-client' | tail -n 1 | cut -d: -f1)"
wait_for 'served infotainment-client at' "$restart_line"
fetch_log | tail -n "+$((mark + 1))" | grep -v ' served '
fetch_log | tail -n "+$((restart_line + 1))" | grep -m 1 'served infotainment-client at'
pause

# --- 9. stop ---------------------------------------------------------------
step "9. stop everything"
stop_demo
started=0
echo "all demo programs stopped, /dev/vehicle is gone"
# Every request of the run, now that no more can come: each distinct served
# line in vsigd's whole log. tools/demo-check.sh checks every one of them.
echo "# every request of this run, as vsigd logged it (distinct lines):"
fetch_log | awk '/ served / && !seen[$0]++'
