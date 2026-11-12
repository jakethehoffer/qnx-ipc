//! Typed messages over a local channel: round trip, errors, dropped requests,
//! short messages and sender information. Every blocking call runs under
//! `with_deadline`, so a broken implementation fails the test instead of hanging it.

use std::mem::size_of;
use std::thread;

use bytemuck::{Pod, Zeroable};
use qnx_ipc::{Channel, ChannelFlags, Connection, Error, Message, Received, RecvBuf, Request};
use qnx_ipc_sys as sys;

const ENOSYS: i32 = sys::ENOSYS as i32;
const EINVAL: i32 = sys::EINVAL as i32;

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

/// A kind the test servers have no handler for.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Mystery {
    n: u32,
}

impl Message for Mystery {
    const KIND: u16 = 0x2FF;
}

/// Runs `f` with a 5-second kernel timeout armed on the calling thread for the
/// next receive, send or reply block (`TimerTimeout` with `_NTO_TIMEOUT_RECEIVE`,
/// `_NTO_TIMEOUT_SEND` and `_NTO_TIMEOUT_REPLY`). A blocked `MsgSend` or
/// `MsgReceive` then fails with `ETIMEDOUT` instead of hanging the test run.
///
/// The kernel clears the timeout when the next kernel call returns, so `f` must
/// make its blocking call before any other kernel call.
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

/// Spawns a server thread that receives one message on `channel` and passes it to
/// `handle`, which returns what the test wants to check on the server side.
fn serve_one<T: Send + 'static>(
    channel: Channel,
    handle: impl FnOnce(Request<'_>) -> T + Send + 'static,
) -> thread::JoinHandle<T> {
    thread::spawn(move || {
        let mut buf = RecvBuf::with_capacity(256);
        let received = with_deadline(|| channel.receive(&mut buf)).expect("receive");
        match received {
            Received::Message(req) => handle(req),
            Received::Pulse(p) => panic!("expected a message, got pulse {p:?}"),
        }
    })
}

fn setup() -> (Channel, Connection) {
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    (channel, conn)
}

#[test]
fn ping_pong_round_trip() {
    let (channel, conn) = setup();
    let server = serve_one(channel, |req| {
        assert_eq!(req.kind(), Ping::KIND);
        let ping: Ping = req.decode().expect("decode");
        req.reply(&Pong { n: ping.n + 1 }).expect("reply");
        ping
    });
    let pong: Pong = with_deadline(|| conn.send(&Ping { n: 41 })).expect("send");
    assert_eq!(pong, Pong { n: 42 });
    assert_eq!(server.join().unwrap(), Ping { n: 41 });
}

#[test]
fn unknown_kind_gets_enosys() {
    let (channel, conn) = setup();
    let server = serve_one(channel, |req| {
        let kind = req.kind();
        if kind != Ping::KIND {
            req.reply_error(ENOSYS).expect("reply_error");
        }
        kind
    });
    let r: qnx_ipc::Result<Pong> = with_deadline(|| conn.send(&Mystery { n: 7 }));
    assert_eq!(r, Err(Error::Os(ENOSYS)));
    assert_eq!(server.join().unwrap(), 0x2FF);
}

#[test]
fn reply_error_rejects_non_positive_errno() {
    // `MsgError(rcvid, 0)` would make the client's send succeed with a zeroed
    // reply, so `reply_error` refuses 0 and negative values. The refused request
    // is then dropped, which answers the client ENOSYS.
    for errno in [0, -1] {
        let (channel, conn) = setup();
        let server = serve_one(channel, move |req| req.reply_error(errno));
        let r: qnx_ipc::Result<Pong> = with_deadline(|| conn.send(&Ping { n: 1 }));
        assert_eq!(r, Err(Error::Os(ENOSYS)), "client, reply_error({errno})");
        assert_eq!(
            server.join().unwrap(),
            Err(Error::Os(EINVAL)),
            "server, reply_error({errno})"
        );
    }
}

#[test]
fn dropped_request_unblocks_client() {
    let (channel, conn) = setup();
    let server = serve_one(channel, |req| drop(req));
    let r: qnx_ipc::Result<Pong> = with_deadline(|| conn.send(&Ping { n: 1 }));
    assert_eq!(r, Err(Error::Os(ENOSYS)));
    server.join().unwrap();
}

#[test]
fn short_message_is_rejected() {
    let (channel, conn) = setup();
    let server = serve_one(channel, |req| {
        let decoded = req.decode::<Ping>();
        req.reply_error(EINVAL).expect("reply_error");
        decoded
    });

    // Only the first two bytes of a header: the kind, nothing else.
    let raw: [u8; 2] = Ping::KIND.to_ne_bytes();
    let mut reply = [0u8; 4];
    let siov = sys::iovec {
        __bindgen_anon_1: sys::iovec__bindgen_ty_1 {
            iov_base_const: raw.as_ptr().cast(),
        },
        iov_len: raw.len() as sys::_Sizet,
    };
    let riov = sys::iovec {
        __bindgen_anon_1: sys::iovec__bindgen_ty_1 {
            iov_base: reply.as_mut_ptr().cast(),
        },
        iov_len: reply.len() as sys::_Sizet,
    };
    // SAFETY: both iovecs point at live local buffers of the stated lengths, and
    // the connection is open for the whole call.
    let rc = with_deadline(|| unsafe { sys::MsgSendv(conn.coid(), &siov, 1, &riov, 1) });
    let client_err = Error::last_os();

    assert_eq!(rc, -1, "the client must get an error reply");
    assert_eq!(client_err, Error::Os(EINVAL));
    assert_eq!(
        server.join().unwrap(),
        Err(Error::Truncated {
            expected: 4 + size_of::<Ping>(),
            got: 2,
        })
    );
}

#[test]
fn request_reports_sender() {
    let (channel, conn) = setup();
    let server = serve_one(channel, |req| {
        let sender = req.sender();
        req.reply(&Pong { n: 0 }).expect("reply");
        sender
    });
    let _: Pong = with_deadline(|| conn.send(&Ping { n: 1 })).expect("send");
    let sender = server.join().unwrap();
    assert_eq!(sender.pid, std::process::id() as i32);
    assert!(sender.tid > 0);
    assert!(sender.priority > 0);
    assert_eq!(sender.srcmsglen, 4 + size_of::<Ping>());
}
