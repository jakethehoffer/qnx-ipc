# How this was built

`qnx-ipc` was built by an AI coding agent under a person's direction. This page
says who did what, how the work was checked, and where the agent got things
wrong. Every mistake below is in this repository's history; the commit hashes
link to it.

## Who did what

**Jake Hoffman**, a fourth-year Computer Engineering student at Queen's
University, chose to enter the QNX Global Hackathon, set up his QNX account
and license, and approved the design (which picked this project) and the
implementation plan.

**Claude Code**, Anthropic's AI coding agent (Claude Opus 5.5, as the
`Co-Authored-By` lines in the commits say), drafted the design and the plan for
Jake's approval, then wrote the code, the tests, the scripts and these docs.
Jake did not type the code.

**A separate AI reviewer**, another Claude agent that did not write the code,
reviewed each task's diff against the design before the next task started. Its
findings were fixed in follow-up commits or recorded as minor and deferred.

## The workflow

1. **Design and plan.** The design named the problem, the API surface, the demo
   and a quality bar (every public item documented, tests on a real QNX target,
   an honest safety write-up). A nine-task plan split the work: toolchain and
   target, generated bindings, channels and messages, pulses and the serve loop,
   named channels and large messages, C interop, resource managers, the demo,
   and these docs.
2. **One task at a time, test first.** For each task the agent wrote the tests
   first and ran them to see them fail (usually a compile error for the missing
   API), then wrote the code until the tests passed on the QNX target. Where a
   test could pass for the wrong reason, the agent broke the code on purpose to
   see the test fail (a "mutation check"), then reverted. Examples:
   `Request::drop` forced to `ENOSYS` on panic made
   `panicking_handler_replies_eio` fail; a channel created without
   `ChannelFlags::DISCONNECT` made `client_exit_delivers_disconnect` time out;
   the C ping server answering `n + 2` made `rust_client_talks_to_c_server`
   fail.
3. **Review, then fix, then the next task.** The reviewer read the diff. Three
   tasks needed a fix round (below). Minor findings were written down rather
   than fixed on the spot; the ones that matter for users are listed under
   "Known limits" in [architecture.md](architecture.md).

## The on-target test loop

The agent could not claim that something worked without QNX agreeing.
[`tools/env.sh`](../tools/env.sh) sets
[`tools/qnx-run`](../tools/qnx-run) as cargo's runner for the
`x86_64-pc-nto-qnx800` target, so `cargo test` and `cargo run` copy each binary
over SSH to a QNX 8.0 virtual machine under QEMU
([`tools/qemu-up.sh`](../tools/qemu-up.sh)), run it there and return its real
exit code. Every test in [test-log.txt](test-log.txt) ran on QNX, not on Linux.
Blocking calls in the tests run under a kernel timeout (`TimerTimeout`), so a
broken implementation fails a test instead of hanging the run.

[`tools/test-all.sh`](../tools/test-all.sh) runs everything in one command:
format and clippy checks, the docs build with warnings as errors, all tests on
the target (with the C programs for the interop tests), and the demo checked
from its log ([`tools/demo-check.sh`](../tools/demo-check.sh)). It writes
[test-log.txt](test-log.txt) and [demo-log.txt](demo-log.txt).

## Where the agent got it wrong

### Caught by the reviewer

- **A timer that could outlive its connection** (`e55116b`, fixed in
  [`1390a00`](https://github.com/jakethehoffer/qnx-ipc/commit/1390a00)). The
  first `PulseTimer` did not borrow its `Connection`, and its doc said that
  dropping the connection just stopped the pulses. The reviewer pointed out
  that QNX reuses connection ids, so a timer that outlived its connection
  would send pulses to whatever channel got that id next. The agent confirmed
  the reuse on the target, made the timer borrow the connection
  (`PulseTimer<'c>`), and added a `compile_fail` example showing the bad order
  no longer compiles.
- **Files still open when a resource manager stops** (`a734205`, fixed in
  [`5c80155`](https://github.com/jakethehoffer/qnx-ipc/commit/5c80155)). The
  reviewer flagged that the teardown detached with `_RESMGR_DETACH_ALL` alone,
  which by a header comment would leave the state of still-open files
  allocated. The agent added counters for allocated and freed open-file blocks
  and a test that keeps a file open across the drop. That test showed QNX 8.0
  already closed those files with `_RESMGR_DETACH_ALL`, as its reference page
  says, so the original code did not leak in practice. The fix kept the
  explicit `_RESMGR_DETACH_ALL | _RESMGR_DETACH_CLOSE` and the test, which
  fails if the detach flag is changed to `_RESMGR_DETACH_PATHNAME`.
- **A demo line that did not prove what it claimed** (`fca5696`, fixed in
  [`74c07fd`](https://github.com/jakethehoffer/qnx-ipc/commit/74c07fd)). The
  demo logged `served infotainment-client at priority 10` as evidence of
  priority inheritance, but the server itself ran at the default priority 10,
  so the line would have read the same without inheritance. The fix starts the
  server at priority 20, has it measure and log its own priority at startup,
  and makes the demo check fail on any served line that is not at the client's
  priority (40 or 10). The log now shows the server thread going up to 40 and
  down to 10.
- **Docs that promised more than the code kept** (review of the docs task,
  fixed in the commit after `28322f0`). The first draft of these docs said a
  client is "never left blocked" while listing a leaked `Request` as a limit;
  the wording now says exactly which paths answer. The same review found that
  an empty device re-read its content on every read, so a reader that had seen
  end of file could later get data. Each open file now takes one snapshot, a
  new test (`empty_snapshot_stays_at_eof_for_its_open`) failed on the old code
  and passes on the new, and a received `Request` now borrows its channel so
  it cannot answer a stale receive id.

### Caught by the target

- **A resource manager could not open its own path.** The first test run of
  `qnx-resmgr` passed 1 of 6 tests: every open from the serving process failed
  with `ENOENT`. QNX needs `_RESMGR_FLAG_SELF` for that; it was added
  (`a734205`).
- **Bindgen could not read the QNX headers** at first: they stop with
  `Endian not defined` unless the macros `qcc` predefines are passed to clang
  (`cba93dc`, documented in [toolchain.md](toolchain.md)).
- **A wrong assumption about `name_open`.** The plan expected a connect
  message (`_IO_CONNECT`) on every `name_open`. A probe on QNX 8.0 showed that
  opening a local name sends the server nothing. The library still answers such
  messages, as the `name_attach` docs require, and a test injects one to pin
  that path (`0ab07f8`).
- **The `libc` crate does not build for QNX 8.0.** The locked version 0.2.191
  only knows QNX 7.x target environments, so the errno constants come from the
  generated bindings instead (`a734205`).
- **Flaky copies to the target.** Back-to-back test runs failed about 3 times
  in 8 with `scp: ... Resource busy`: QNX keeps a program file busy for a
  moment after the process exits. The runner now uses a fresh directory per run
  (`dd17a5b`).
- **Smaller ones:** a C sign-compare warning (`-Werror`) in the ping server;
  `ssh` hanging when the demo started programs with `nohup ... &` (fixed with a
  detaching subshell); `slay -p` exiting non-zero when it finds a match.

### Caught by the agent's own review before committing

- An aliasing bug in the resource-manager callbacks: they read the attribute
  through a raw open-file pointer while a `&mut` to the same open-file block was
  live. The callbacks now read the device and attribute pointers first.

## Setting up QNX

Setup problems, all solved in the first commit
([`09b59a9`](https://github.com/jakethehoffer/qnx-ipc/commit/09b59a9)) and
written up in [toolchain.md](toolchain.md):

- QNX's Rust package for SDP 8.0 ships `rustc` 1.85.1 with the standard library
  prebuilt for QNX, but no `cargo`, `clippy` or `rustfmt`. The build pairs it
  with rustup's cargo 1.85.1, and lint runs clippy for the Linux host, which
  works because the bindings always come from the QNX headers.
- The default `mkqnximage` QEMU image starts `sshd`, which exits at boot
  because its `sshd-auth` helper is missing from the image.
  `tools/qemu-up.sh` adds the helper at `/usr/libexec/sshd-auth`.
- `mkqnximage --run` needs a host network bridge; the scripts use QEMU
  user-mode networking with SSH forwarded to `127.0.0.1:2222` instead.
