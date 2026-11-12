# Vehicle-signals demo

A small vehicle-signals system on QNX 8.0, built with `qnx-ipc` and `qnx-resmgr`.
It shows three things a QNX engineer cares about, in safe Rust talking to plain C:

1. **Priority inheritance.** `vsigd` is a single-threaded server. A safety client
   at priority 40 and an infotainment client at priority 10 both send it
   requests. For each request `vsigd` asks the kernel (`SchedGet`,
   `sched_curpriority`) what priority its own thread is running at, and logs it:
   40 while it serves the safety client, 10 while it serves the infotainment
   client. Nothing in `vsigd` sets a priority; QNX message passing does it.
2. **Signals as files.** The same values are served as text files at
   `/dev/vehicle/speed` and `/dev/vehicle/brake`, so `cat` and any other POSIX
   program can read them.
3. **Supervision with pulses.** Every client sends the watchdog a heartbeat pulse
   after each answer. When the infotainment client is frozen with `kill -STOP`,
   its heartbeats stop, the watchdog misses three checks in a row, kills it and
   starts it again.

The vehicle signals are **simulated**: `vsigd` moves speed and brake pressure
along a smooth, repeating curve on a 10 Hz pulse timer. There is no vehicle bus.

## The programs

| Program | Language | What it does |
| --- | --- | --- |
| `demo/vsigd` | Rust | Registers the name `vehicle`, answers `HELLO` and `GET_SIGNAL`, serves `/dev/vehicle/speed` and `/dev/vehicle/brake`, updates the signals from a `PulseTimer`. |
| `demo/safety-client` | Rust | `SCHED_FIFO` priority 40. Reads brake pressure every 100 ms, sends heartbeats. |
| `c/infotainment_client.c` | C | `SCHED_FIFO` priority 10. Reads the speed in a loop, sends heartbeats. |
| `demo/watchdog` | Rust | Registers the name `watchdog`, starts both clients, checks their heartbeats every 500 ms, restarts a client after 3 missed checks. |

The protocol is `c/include/vehicle.h`, mirrored in `demo/vsigd/src/proto.rs`.
Both copies assert the struct sizes, so they cannot drift apart silently.

| Item | Value |
| --- | --- |
| Server name | `"vehicle"` |
| `GET_SIGNAL` | kind `0x210`, `u32 signal_id` (1 speed, 2 brake pressure), reply `i64 value_milli` |
| `HELLO` | kind `0x211`, 32-byte NUL-padded client name, empty reply; sent once per connection |
| Heartbeat | pulse code `10` to the name `"watchdog"`, value = client id (1 safety, 2 infotainment) |
| Files | `speed_kmh=87.5` and `brake_kpa=310.2`, one line, one decimal place |

The watchdog tells clients apart by the heartbeat's value: a pulse does not say
which connection sent it.

## Running it

With the QEMU target up (`tools/qemu-up.sh`), from WSL:

```sh
tools/demo.sh              # narrated run with a short pause between steps
tools/demo.sh --pause 4    # longer pauses, for recording
tools/demo.sh --headless   # no pauses
tools/demo.sh --stop       # stop a run that was interrupted
tools/demo-check.sh        # headless run that checks the log, prints "demo check passed"
```

`tools/demo.sh` builds the programs, copies them to `/tmp/vehicle-demo` on the
target, runs the scenario and stops everything at the end: no demo process is
left and `/dev/vehicle` is gone. It first stops anything an earlier run left
behind, so it can be run again at any time. All programs write to
`/tmp/vehicle-demo/demo.log` on the target; the script prints the interesting
lines, with the per-request `served` lines reduced to the first one per client.
