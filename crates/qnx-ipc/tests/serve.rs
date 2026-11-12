//! The serve loop: stopping on `Break`, and a panicking handler that must answer
//! its client with `EIO` and keep serving. Every client send runs under a kernel
//! timeout and the server thread is joined with a deadline, so a broken
//! implementation fails instead of hanging.

use std::ops::ControlFlow;
use std::thread;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use qnx_ipc::{Channel, ChannelFlags, Connection, Error, Handler, Message, RecvBuf, Request};
use qnx_ipc_sys as sys;

const EIO: i32 = sys::EIO as i32;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
struct Ping {
    n: u32,
}

impl Message for Ping {
    const KIND: u16 = 0x201;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
struct Pong {
    n: u32,
}

/// Runs `f` with a 5-second kernel timeout armed on the calling thread for the
/// next send or reply block (`TimerTimeout`), so a blocked `MsgSend` fails with
/// `ETIMEDOUT` instead of hanging. `f` must block before any other kernel call.
fn with_deadline<T>(f: impl FnOnce() -> T) -> T {
    let flags =
        (1u32 << sys::STATE_RECEIVE) | (1u32 << sys::STATE_SEND) | (1u32 << sys::STATE_REPLY);
    let ns: u64 = 5_000_000_000;
    // SAFETY: a null event means "unblock the thread" (SIGEV_UNBLOCK). `ns` lives
    // across the call and the old-time pointer is null (not wanted).
    let rc = unsafe {
        sys::TimerTimeout(
            sys::CLOCK_MONOTONIC as sys::clockid_t,
            flags,
            std::ptr::null(),
            &ns,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(rc, -1, "TimerTimeout failed: {:?}", Error::last_os());
    f()
}

/// Joins `handle`, failing the test if the thread is still running after 5 s.
fn join_within<T>(handle: thread::JoinHandle<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished() {
        assert!(Instant::now() < deadline, "server thread did not finish");
        thread::sleep(Duration::from_millis(10));
    }
    handle.join().expect("server thread panicked")
}

/// Replies `n + 1` to every `Ping` and stops after `stop_after` messages.
struct Counter {
    seen: u32,
    stop_after: u32,
}

impl Handler for Counter {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        let ping: Ping = req.decode()?;
        req.reply(&Pong { n: ping.n + 1 })?;
        self.seen += 1;
        if self.seen >= self.stop_after {
            Ok(ControlFlow::Break(()))
        } else {
            Ok(ControlFlow::Continue(()))
        }
    }
}

/// Starts `serve` with `handler` on a new channel's own thread and returns the
/// thread and a connection to the channel.
fn spawn_server<H: Handler + Send + 'static>(
    mut handler: H,
) -> (thread::JoinHandle<(qnx_ipc::Result<()>, H)>, Connection) {
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let server = thread::spawn(move || {
        let mut buf = RecvBuf::with_capacity(256);
        let r = channel.serve(&mut buf, &mut handler);
        (r, handler)
    });
    (server, conn)
}

#[test]
fn serve_stops_on_break() {
    let (server, conn) = spawn_server(Counter {
        seen: 0,
        stop_after: 3,
    });
    for n in 0..3 {
        let pong: Pong = with_deadline(|| conn.send(&Ping { n })).expect("send");
        assert_eq!(pong, Pong { n: n + 1 });
    }
    let (r, handler) = join_within(server);
    assert_eq!(r, Ok(()));
    assert_eq!(handler.seen, 3);
}

/// Panics on the first message, then behaves like `Counter` stopping after one
/// answered message.
struct PanicsFirst {
    calls: u32,
}

impl Handler for PanicsFirst {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        self.calls += 1;
        if self.calls == 1 {
            panic!("handler failure on purpose");
        }
        let ping: Ping = req.decode()?;
        req.reply(&Pong { n: ping.n + 1 })?;
        Ok(ControlFlow::Break(()))
    }
}

#[test]
fn panicking_handler_replies_eio() {
    let (server, conn) = spawn_server(PanicsFirst { calls: 0 });
    let first: qnx_ipc::Result<Pong> = with_deadline(|| conn.send(&Ping { n: 1 }));
    assert_eq!(first, Err(Error::Os(EIO)));
    let second: Pong = with_deadline(|| conn.send(&Ping { n: 10 })).expect("second send");
    assert_eq!(second, Pong { n: 11 });
    let (r, handler) = join_within(server);
    assert_eq!(r, Ok(()));
    assert_eq!(handler.calls, 2);
}

/// Counts each pulse and then panics; stops on the first message.
struct PanicsOnPulse {
    /// Incremented before the panic, so it survives the caught unwind.
    pulses: u32,
}

impl Handler for PanicsOnPulse {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        req.reply(&Pong { n: 0 })?;
        Ok(ControlFlow::Break(()))
    }

    fn on_pulse(&mut self, _pulse: qnx_ipc::Pulse) -> qnx_ipc::Result<ControlFlow<()>> {
        self.pulses += 1;
        panic!("pulse handler failure on purpose");
    }
}

#[test]
fn panicking_pulse_handler_keeps_serving() {
    let (server, conn) = spawn_server(PanicsOnPulse { pulses: 0 });
    conn.send_pulse(qnx_ipc::PulseCode::user(1).expect("code"), 0)
        .expect("send_pulse");
    let pong: Pong = with_deadline(|| conn.send(&Ping { n: 1 })).expect("send");
    assert_eq!(pong, Pong { n: 0 });
    let (r, handler) = join_within(server);
    assert_eq!(r, Ok(()));
    // The pulse was queued before the message, so `on_pulse` ran (and panicked)
    // exactly once before the message stopped the loop.
    assert_eq!(handler.pulses, 1);
}
