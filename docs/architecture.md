# Architecture and safety model

This page explains how the three crates fit together, who owns each QNX kernel
object, why the public API is sound, and how the library makes sure a blocked
client always gets an answer. Every `unsafe` block in the library crates is
listed below with the invariant it relies on. The same invariants are written
as `// SAFETY:` comments next to the code.

## Crate layering

```
  your program / demo/vsigd, demo/watchdog, demo/safety-client
        |                         |
        v                         v
  qnx-resmgr  ----------->  qnx-ipc          safe API, no QNX types in it
  (Device, ResourceManager)  (Channel, Connection, Request, PulseTimer, ...)
        |                         |
        +-----------+-------------+
                    v
              qnx-ipc-sys                     raw FFI, all calls unsafe
              bindgen over the QNX SDP 8.0 headers in wrapper.h
```

- **[`qnx-ipc-sys`](../crates/qnx-ipc-sys)**: `build.rs` runs bindgen over
  [`wrapper.h`](../crates/qnx-ipc-sys/wrapper.h) (`sys/neutrino.h`,
  `sys/dispatch.h`, `sys/iofunc.h`, `sys/resmgr.h` and a few more) into cargo's
  `OUT_DIR` at build time, with an allowlist of the functions, types and
  constants the other crates use. The generated code is never committed. C
  macros bindgen cannot translate (`SIGEV_PULSE_INIT`, `SETIOV`,
  `_RESMGR_NPARTS`, `_RESMGR_PTR`, `_IO_SET_READ_NBYTES` and others) are written
  by hand in [`src/lib.rs`](../crates/qnx-ipc-sys/src/lib.rs), each with a link
  to its QNX 8.0 reference page. The bindings always come from the QNX headers,
  also when cargo builds for the Linux host, so `clippy` and `rustdoc` can check
  the crates on Linux (QNX's Rust package ships neither).
- **[`qnx-ipc`](../crates/qnx-ipc)**: channels, connections, typed messages,
  requests, pulses, pulse timers, named channels and the serve loop. Its error
  type ([`Error`](../crates/qnx-ipc/src/error.rs)) is shared by all crates.
- **[`qnx-resmgr`](../crates/qnx-resmgr)**: a `Device` trait (`read`, `write`,
  `devctl`) and a `ResourceManager` that attaches it at a path with QNX's
  dispatch and `iofunc` layers. Generic `extern "C"` callbacks in
  [`trampoline.rs`](../crates/qnx-resmgr/src/trampoline.rs) connect the C
  framework to the trait.
- **C side**: [`c/include/qnx_ipc_ping.h`](../c/include/qnx_ipc_ping.h) and
  [`c/include/vehicle.h`](../c/include/vehicle.h) describe the same wire layout
  as the Rust types (a 4-byte `{ uint16_t kind; uint16_t reserved; }` header,
  then the payload). Both sides assert the struct sizes.

### Wire format

A typed message is a [`Header`](../crates/qnx-ipc/src/message.rs) (`kind`,
`reserved`) followed by the bytes of a `Message` type. `Message: bytemuck::Pod`,
so any bytes are a valid value and a value has no padding or pointers. User
kinds are `>= 0x200`: lower kinds are QNX's (`_IO_BASE`..`_IO_MAX`, 0x100 to
0x1FF, used for connect and I/O messages). Sending or decoding a `Message` with
a lower `KIND` is a compile error (a `const` assertion evaluated in `send` and
`decode`). `Request::reply` passes the reply length as the `MsgReply` status,
the QNX convention, so a C client treats only `-1` from `MsgSend` as failure.

## Ownership and `Drop` rules

Every kernel object is owned by exactly one Rust value and released exactly
once, in its `Drop`. Drop never panics; a failed release cannot be acted on and
is ignored.

| Type | Acquires | Drop releases | Borrows |
| --- | --- | --- | --- |
| `Channel` | `ChannelCreate` | `ChannelDestroy` | |
| `NamedChannel` | `name_attach` (its own channel) | `name_detach` | |
| `Connection` | `ConnectAttach` (`to_local`, `to_process`) or `name_open` (`open`) | `ConnectDetach` or `name_close`, whichever matches | |
| `RecvBuf` | a `Vec<u8>`, at least 16 bytes (one pulse) | the buffer | |
| `Request<'b>` | a receive id (`rcvid`) from `MsgReceive` | if not answered: `MsgError(ENOSYS)`, or `EIO` while panicking | the `RecvBuf` (`'b`) |
| `PulseTimer<'c>` | `timer_create` + `timer_settime` (`SIGEV_PULSE`) | `timer_delete` | the `Connection` (`'c`) |
| `RunningManager` | a dispatch thread and a side-channel connection to it | sets the stop flag, sends the stop pulse, joins the thread, `ConnectDetach` | |
| `Attached<D>` (private, owned by the dispatch thread) | `dispatch_create`, a boxed `Resources<D>`, `resmgr_attach`, `pulse_attach`, `dispatch_context_alloc` | in reverse: `resmgr_detach(_RESMGR_DETACH_ALL \| _RESMGR_DETACH_CLOSE)`, `pulse_detach`, `dispatch_context_free`, `dispatch_destroy`, then frees the box and the device | |

Two of these borrows exist because of real problems:

- **`Request` borrows `RecvBuf`.** A request's bytes are a slice of the buffer,
  so the buffer cannot be received into again while a request is alive.
- **`PulseTimer` borrows `Connection`.** The kernel timer holds only the coid
  number, and QNX reuses coids. A timer that outlived its connection would send
  pulses to whatever channel got that coid next. This was a review finding (see
  [how-it-was-built.md](how-it-was-built.md)); the borrow makes it a compile
  error, shown by a `compile_fail` example in
  [`timer.rs`](../crates/qnx-ipc/src/timer.rs).

`Channel::receive` also releases a kernel object the caller never sees: on a
disconnect pulse it calls `ConnectDetach(scoid)` for the departed client's
server connection, which QNX keeps allocated until the server detaches it.

## Why the public API is sound

The public API has no `unsafe fn` and takes no raw pointers. The arguments for
soundness:

1. **The kernel only writes into memory we exclusively own, of the length we
   state.** Receive buffers are `&mut` borrowed for the call; reply buffers are
   `&mut [u8]` or a zeroed `R: Pod`, so any bytes the kernel writes are a valid
   value. Send buffers are only read.
2. **Decoding cannot read out of bounds or build an invalid value.**
   `Request::decode` checks the length (`Error::Truncated`) and the kind
   (`Error::UnknownKind`) first, then uses `bytemuck::pod_read_unaligned`.
3. **Kernel ids are owned.** A chid, coid, timer id or `name_attach_t` lives in
   one value, is never copied into another owner, and is released once.
4. **No unwind crosses FFI.** Every `extern "C"` callback in `qnx-resmgr` runs
   its body in `catch_unwind`. `serve` runs handlers in `catch_unwind` too.
5. **Resource-manager pointers stay valid.** Everything the C framework points
   into lives in one boxed `Resources<D>` whose address is fixed from before
   `resmgr_attach` until after `dispatch_destroy`, and is freed only by the
   thread that ran the dispatch loop, after the loop has stopped.

### Every `unsafe` block

`qnx-ipc` ([`src/`](../crates/qnx-ipc/src)):

| Where | What | Invariant |
| --- | --- | --- |
| `channel.rs` `Channel::new` | `ChannelCreate` | Integer flags only; no memory preconditions. |
| `channel.rs` `receive_on` | `MsgReceive` | Kernel writes at most `buf.data.len()` bytes into `buf.data`, which is exclusively borrowed for the call; `info` is a live local of the right type. |
| `channel.rs` `take_pulse` | `read_unaligned` of `_pulse` | On a pulse the kernel wrote a `_pulse` at the start of the buffer; `RecvBuf::with_capacity` guarantees at least `size_of::<_pulse>()` bytes; every bit pattern is a valid `_pulse`. |
| `channel.rs` `take_pulse` | read `sival_int` union field | It overlaps the start of the union; any bits are a valid `i32`. |
| `channel.rs` `take_pulse` | `ConnectDetach(scoid)` | On a disconnect pulse `scoid` is the server connection to the departed client, which must be detached; integer only. |
| `channel.rs` `answer_system_message` | `MsgReply(rcvid, EOK, null, 0)` | `rcvid` is received and not yet answered; null with length 0 sends no data. |
| `channel.rs` `answer_system_message` | `MsgError(rcvid, ENOSYS)` | Integers only; `rcvid` received and not yet answered. |
| `channel.rs` `Drop for Channel` | `ChannelDestroy` | `chid` came from a successful `ChannelCreate` and is destroyed only here. |
| `connection.rs` `to_process` | `ConnectAttach` | Integers only. |
| `connection.rs` `open` | `name_open` | `c_name` is a NUL-terminated `CString` that outlives the call. |
| `connection.rs` `send` | `MsgSendv` | Send iovecs point at live `Header` and `M` values of the stated sizes (kernel reads only); the reply iovec points at `reply: R`, live for the call, at most `size_of::<R>()` bytes written, any bytes valid since `R: Pod`. |
| `connection.rs` `send_bytes` | `MsgSendv` | Same, with `payload: &[u8]` and `reply: &mut [u8]` (exclusively borrowed). |
| `connection.rs` `send_pulse` | `MsgSendPulse` | Integers only; `coid` is owned by `self`. |
| `connection.rs` `thread_priority` | `SchedGet(0, 0, &mut param)` | 0/0 is the calling thread; `param` is a live local. |
| `connection.rs` `Drop for Connection` | `name_close` / `ConnectDetach` | The coid came from the matching open call and is released only here. |
| `name.rs` | `unsafe impl Send for NamedChannel` | The `name_attach_t` pointer is owned by this value, never read after construction, and passed to `name_detach` once, from any thread. |
| `name.rs` | `unsafe impl Sync for NamedChannel` | Shared access only copies out `chid`; the kernel supports receiving on one channel from several threads. |
| `name.rs` `attach` | `name_attach(NULL, name, 0)` | `c_name` is a live `CString`; a null dispatch pointer asks QNX to create its own. |
| `name.rs` `attach` | read `attach.chid` | `name_attach` succeeded, so the pointer is a valid, initialized `name_attach_t`. |
| `name.rs` `Drop for NamedChannel` | `name_detach` | The pointer came from a successful `name_attach` and is detached only here. |
| `request.rs` `read_into` | `MsgRead` | `out` is exclusively borrowed, at most `out.len()` bytes written; `rcvid` is not answered yet (answering consumes `self`), so the sender is still blocked and its message readable. |
| `request.rs` `reply` | `MsgReply` | `reply: &R` is live and initialized for `size_of::<R>()` bytes (kernel reads only); `rcvid` not yet answered. |
| `request.rs` `reply_error` | `MsgError` | Integers only; `rcvid` not yet answered. |
| `request.rs` `Drop for Request` | `MsgError(ENOSYS or EIO)` | Runs only if `answered` is false; integers only; never panics. |
| `timer.rs` `periodic` | `timer_create` | `event` is a fully initialized `sigevent` and `id` a live local; both outlive the call. |
| `timer.rs` `periodic` | `timer_settime` | `value` is live and only read; `timer.id` is a live timer. |
| `timer.rs` `Drop for PulseTimer` | `timer_delete` | `id` came from a successful `timer_create` and is deleted only here. |

`qnx-resmgr` ([`src/`](../crates/qnx-resmgr/src)):

| Where | What | Invariant |
| --- | --- | --- |
| `manager.rs` `spawn` | `message_connect(dpp, MSG_FLAG_SIDE_CHANNEL)` | `dpp` is the live dispatch handle. The coid belongs to `RunningManager`, so it cannot be reused for another channel before the stop pulse is sent. |
| `manager.rs` `spawn` | clone the `Arc` in `*res` | `res` is the live box; the loop thread has not started. |
| `manager.rs` `spawn` | `ConnectDetach(coid)` on thread-spawn failure | Our connection, not used again. |
| `manager.rs` `Drop for RunningManager` | `MsgSendPulse(coid, -1, code, 0)` | Our live connection; a pulse carries no pointers. If it cannot be sent and the thread still runs, drop does not join (no hang). |
| `manager.rs` `Drop for RunningManager` | `ConnectDetach(coid)` | Our connection, after the join, not used again. |
| `manager.rs` | `unsafe impl Send for Attached<D>` | Its raw pointers are owned by it and used by one thread at a time; the device is `Send + Sync`. |
| `manager.rs` `Attached::new` | `dispatch_create` | No preconditions. |
| `manager.rs` `Attached::new` | `iofunc_func_init`, `iofunc_mount_init`, `iofunc_attr_init`, `resmgr_attach` and the field set-up | Every pointer handed to the framework is derived from the `Box::into_raw` pointer and points into that box. Only raw place expressions (`addr_of_mut!`) are used, so no Rust reference outlives a statement. |
| `manager.rs` `Attached::new` | `pulse_attach(MSG_FLAG_ALLOC_PULSE)` | `dpp` is live; the handler ignores its arguments. |
| `manager.rs` `Attached::new` | `dispatch_context_alloc` | `dpp` is live and has its attachments. |
| `manager.rs` `Attached::serve` | `dispatch_block` | `ctp` is our live context; a returned (possibly reallocated) context replaces it. |
| `manager.rs` `Attached::serve` | `dispatch_handler` | `ctp` holds the message `dispatch_block` just received. |
| `manager.rs` `Drop for Attached` | detach, `pulse_detach`, context free, `dispatch_destroy`, `Box::from_raw` | Released once, in reverse order, on the thread that ran the loop, so no callback is running. `_RESMGR_DETACH_CLOSE` runs the close callbacks of files still open before the box (attribute, counters, device) is freed. |
| `trampoline.rs` `parts_of` (unsafe fn) | read `ocb->attr`, cast to `DeviceAttr<D>`, create `&D` | The caller passes a live OCB of a path attached with a `DeviceAttr<D>` handle. `DeviceAttr` is `#[repr(C)]` with `iofunc_attr_t` first, so the cast gets the original pointer back. It is called before any `&mut DeviceOcb` exists, so the read does not alias it. |
| `trampoline.rs` `mark_attr` (unsafe fn) | set `attr->flags` | A live attribute, touched only by the dispatch thread; `flags` is disjoint from the device behind a live `&D`. |
| `trampoline.rs` `ocb_calloc` | read `DeviceAttr::stats`, bump `opened` | `attr` is our path's handle; the counters live in the same box and outlive every OCB. |
| `trampoline.rs` `ocb_free` | `Box::from_raw(DeviceOcb)`, bump `closed` | Every OCB of our paths comes from `ocb_calloc` (`Box::into_raw`), and the framework frees each exactly once. |
| `trampoline.rs` `io_read`, `io_write`, `io_devctl` | call the bodies `read`, `write`, `devctl` | The framework passes a live context, the received message and an OCB of our path, valid for the call. |
| `trampoline.rs` `read`, `write` | `iofunc_read_verify` / `iofunc_write_verify` | Arguments as the framework passed them. |
| `trampoline.rs` `read`, `write` | read `msg->i` / `msg->i64` | The received message; both views overlay the same plain-integer header. |
| `trampoline.rs` `read` | `&mut *ocb.cast::<DeviceOcb>()` | The OCB comes from `ocb_calloc`; one open file per OCB and a single dispatch thread, so this is the only reference while the callback runs. |
| `trampoline.rs` `read` | `_IO_SET_READ_NBYTES(&mut *ctp)` | `ctp` is the live context of this message. |
| `trampoline.rs` `read` | `MsgReply(rcvid, count, chunk)` | `chunk` is `count` readable bytes of the snapshot, live across the call. Nothing after it can panic, so the client is answered once. |
| `trampoline.rs` `payload_available` (unsafe fn) | read `ctp->info.srcmsglen`, `ctp->offset` | `ctp` is the live context. Write and devctl buffers are sized from it, so a bogus `nbytes` cannot force a huge allocation. |
| `trampoline.rs` `write`, `devctl` | `resmgr_msgread` | `data` has at least the requested number of writable bytes; the offset is relative to this message. |
| `trampoline.rs` `write` | `_IO_SET_WRITE_NBYTES(&mut *ctp)` | `ctp` is the live context. |
| `trampoline.rs` `devctl` | `iofunc_devctl_default` | Arguments as the framework passed them. |
| `trampoline.rs` `devctl` | copy `msg->i` | The received `io_devctl_t`, copied before the receive buffer can be reused. |
| `trampoline.rs` `devctl` | `MsgReplyv` (2 parts) | Both parts point at live buffers (`reply` header, `data`) of the given lengths for the whole call. |

Test code (`tests/`) has its own `unsafe` blocks for raw sends and
`TimerTimeout` deadlines, each with a `SAFETY` comment. The demo programs use
`unsafe` only for `SchedGet` and `SchedSet`.

## Reply guarantee: a blocked client always gets an answer

A QNX client that sends a message is blocked until the server replies. A server
bug that forgets to reply leaves the client blocked forever. The library closes
every path to that, and each rule has a test that runs on QNX:

| Situation | What the client gets | Test |
| --- | --- | --- |
| Handler replies | the reply (`send` returns it) | `messages::ping_pong_round_trip` |
| Handler drops the `Request` without answering | `ENOSYS` | `messages::dropped_request_unblocks_client` |
| Message of a kind the server does not handle | `ENOSYS` (the server checks `kind()` and calls `reply_error`; dropping the request gives the same) | `messages::unknown_kind_gets_enosys` |
| Message shorter than its type | the server's `decode` returns `Error::Truncated`, never reads past the data; the client gets the error the server chooses (`ENOSYS` if it just drops the request) | `messages::short_message_is_rejected` |
| `on_message` panics inside `serve` | `EIO`; the server keeps serving | `serve::panicking_handler_replies_eio` |
| `on_pulse` panics inside `serve` | (no client waits) the loop keeps serving | `serve::panicking_pulse_handler_keeps_serving` |
| `MsgReply` or `MsgError` itself fails | `answered` stays false, so `Drop` still sends `MsgError(ENOSYS)` | (by construction) |
| QNX system message (kinds up to 0x1FF) on a `NamedChannel` | `_IO_CONNECT` gets `EOK`, others `ENOSYS`; the handler never sees them | `names::system_messages_answered_transparently` |
| Client process dies | the server gets a `PulseKind::Disconnect` pulse and the server connection is released | `pulses::client_exit_delivers_disconnect` |
| `Device` method panics | `EIO`; the manager keeps serving (the next open works) | `resmgr::panicking_device_returns_eio` |
| `Device` method returns an error | its `errno` (for example `ENOTTY` for an unknown `devctl` command) | `resmgr::devctl_round_trip` |
| Resource manager stops with files open | those files are closed (their OCBs freed); later reads fail | `resmgr::drop_detaches_path` |

How the panic path works: `serve` moves the `Request` into `on_message`. If the
handler panics, the request is dropped during the unwind, before
`catch_unwind` returns, and its `Drop` sees `std::thread::panicking()` and
replies `EIO`. The `answered` flag makes sure a handler that replied before it
panicked is not answered twice. In `qnx-resmgr`, the read and devctl callbacks
reply with `MsgReply`/`MsgReplyv` and return `_RESMGR_NOREPLY`; the only code
after the reply is that `return`, so a panic cannot cause a second reply.

## Known limits

These are honest edges of the current design, not hidden bugs:

- **A leaked `Request` leaves its client blocked.** Rust allows
  `std::mem::forget`, and a forgotten request never runs its `Drop`. The
  library cannot answer what it is not told about.
- **A `Request` can outlive its `Channel`.** The request borrows the receive
  buffer, not the channel. If the channel is destroyed first, the request's
  `Drop` replies to a receive id that no longer exists (the call fails and is
  ignored). Tying the request to the channel's lifetime would close this.
- **`panic = "abort"`** turns the panic paths above into process aborts; the
  client then sees the server's death instead of `EIO`.
- **Handler state after a caught panic** may be half-updated; `serve` uses
  `AssertUnwindSafe` and keeps going.
- **One dispatch thread per resource manager.** A `Device` method that does I/O
  on its own path would wait for itself; this is documented on
  `ResourceManager`.
- **A `TimerTimeout` armed before `NamedChannel::receive`** covers only its
  first internal receive, since an answered system message is followed by
  another receive.
- **`Pulse` does not expose the `scoid`**, so a server cannot map a disconnect
  pulse to a per-client entry (the demo's `vsigd` keys clients by pid instead).
- **Only x86_64 is tested.** The bindgen set-up has an aarch64 path that has not
  been run.
