//! Named channels (`name_attach` / `name_open`) and messages larger than the
//! server's receive buffer. Blocking client calls run under `with_deadline` and
//! server threads are joined with `join_within`, so a broken implementation fails
//! the test instead of hanging it.

use std::mem::size_of;
use std::ops::ControlFlow;
use std::thread;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use qnx_ipc::{Connection, Error, Handler, Message, NamedChannel, RecvBuf, Request};
use qnx_ipc_sys as sys;

const EEXIST: i32 = sys::EEXIST as i32;

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

/// The kind of the large-payload message.
const BULK_KIND: u16 = 0x210;

/// Runs `f` with a 5-second kernel timeout armed on the calling thread for the
/// next receive, send or reply block (`TimerTimeout` with `_NTO_TIMEOUT_RECEIVE`,
/// `_NTO_TIMEOUT_SEND` and `_NTO_TIMEOUT_REPLY`).
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

/// Joins `handle`, failing the test if the thread is still running after 10 s.
fn join_within<T>(handle: thread::JoinHandle<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !handle.is_finished() {
        assert!(Instant::now() < deadline, "server thread did not finish");
        thread::sleep(Duration::from_millis(10));
    }
    handle.join().expect("server thread panicked")
}

/// A name unique to this process and test, so parallel tests do not collide.
fn test_name(test: &str) -> String {
    format!("qnx-ipc-test-{}-{test}", std::process::id())
}

/// Replies `n + 1` to one `Ping`, then stops.
struct OnePong;

impl Handler for OnePong {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        let ping: Ping = req.decode()?;
        req.reply(&Pong { n: ping.n + 1 })?;
        Ok(ControlFlow::Break(()))
    }
}

/// Starts `serve` with `handler` on `channel` in its own thread.
fn spawn_server<H: Handler + Send + 'static>(
    channel: NamedChannel,
    mut handler: H,
    buf_bytes: usize,
) -> thread::JoinHandle<qnx_ipc::Result<()>> {
    thread::spawn(move || {
        let mut buf = RecvBuf::with_capacity(buf_bytes);
        channel.serve(&mut buf, &mut handler)
    })
}

#[test]
fn open_by_name_round_trip() {
    let name = test_name("round-trip");
    let channel = NamedChannel::attach(&name).expect("attach");
    let server = spawn_server(channel, OnePong, 256);

    let conn = Connection::open(&name).expect("open");
    let pong: Pong = with_deadline(|| conn.send(&Ping { n: 41 })).expect("send");
    assert_eq!(pong, Pong { n: 42 });
    join_within(server).expect("serve");
}

#[test]
fn duplicate_name_fails() {
    let name = test_name("duplicate");
    let _first = NamedChannel::attach(&name).expect("first attach");
    // QNX documents EEXIST ("The specified path already exists") for name_attach
    // of a name that is already attached, and the target returns exactly that.
    let second = NamedChannel::attach(&name);
    assert_eq!(second.err(), Some(Error::Os(EEXIST)));
}

#[test]
fn invalid_name_rejected() {
    assert_eq!(
        NamedChannel::attach("a/b").err(),
        Some(Error::InvalidName("a/b".to_string()))
    );
    assert_eq!(
        NamedChannel::attach("").err(),
        Some(Error::InvalidName(String::new()))
    );
}

/// Sends `raw` as one message on `conn` with `MsgSendv` and a 4-byte reply
/// buffer, returning `MsgSendv`'s result and the errno if it failed.
fn send_raw(conn: &Connection, raw: &[u8]) -> (isize, Option<Error>) {
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
    // SAFETY: both iovecs point at live buffers of the stated lengths, and the
    // connection is open for the whole call.
    let rc = with_deadline(|| unsafe { sys::MsgSendv(conn.coid(), &siov, 1, &riov, 1) });
    let err = (rc == -1).then(Error::last_os);
    (rc as isize, err)
}

#[test]
fn system_messages_answered_transparently() {
    // QNX 8.0's name_open of a local name sends the server nothing (observed on
    // the target), but the name_attach documentation requires a server to answer
    // system messages, which any client can send. Inject them by hand: the
    // handler must never see them, and the Ping after them must still work.
    let name = test_name("system");
    let channel = NamedChannel::attach(&name).expect("attach");
    let server = spawn_server(channel, OnePong, 256);
    let conn = Connection::open(&name).expect("open");

    // An `_IO_CONNECT` header (type, subtype): answered EOK with no data.
    let mut connect = [0u8; 8];
    connect[..2].copy_from_slice(&(sys::_IO_CONNECT as u16).to_ne_bytes());
    assert_eq!(send_raw(&conn, &connect), (0, None));

    // `_IO_READ` and a kind below `_IO_BASE`: answered ENOSYS.
    let enosys = Some(Error::Os(sys::ENOSYS as i32));
    let read = (sys::_IO_READ as u16).to_ne_bytes();
    assert_eq!(
        send_raw(&conn, &[read[0], read[1], 0, 0]),
        (-1, enosys.clone())
    );
    assert_eq!(send_raw(&conn, &[0x10, 0, 0, 0]), (-1, enosys));

    let pong: Pong = with_deadline(|| conn.send(&Ping { n: 1 })).expect("send");
    assert_eq!(pong, Pong { n: 2 });
    join_within(server).expect("serve");
}

/// Reads the whole bulk message with `read_into`, replies the sum of its payload
/// bytes as a `u64` and stops.
struct ByteSummer {
    /// The full message length the server saw, header included.
    seen_len: usize,
}

impl Handler for ByteSummer {
    fn on_message(&mut self, req: Request<'_>) -> qnx_ipc::Result<ControlFlow<()>> {
        assert_eq!(req.kind(), BULK_KIND);
        let total = req.sender().srcmsglen;
        self.seen_len = total;
        let mut chunk = vec![0u8; 64 * 1024];
        let mut offset = size_of::<qnx_ipc::Header>();
        let mut sum: u64 = 0;
        while offset < total {
            let n = req.read_into(offset, &mut chunk)?;
            assert!(n > 0, "read_into returned 0 before the end of the message");
            sum += chunk[..n].iter().map(|&b| u64::from(b)).sum::<u64>();
            offset += n;
        }
        req.reply(&sum)?;
        Ok(ControlFlow::Break(()))
    }
}

#[test]
fn one_mebibyte_payload_round_trip() {
    const LEN: usize = 1024 * 1024;
    let payload: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
    let expected: u64 = payload.iter().map(|&b| u64::from(b)).sum();

    let name = test_name("mebibyte");
    let channel = NamedChannel::attach(&name).expect("attach");
    let server = thread::spawn(move || {
        let mut buf = RecvBuf::with_capacity(64);
        let mut handler = ByteSummer { seen_len: 0 };
        let r = channel.serve(&mut buf, &mut handler);
        (r, handler.seen_len)
    });

    let conn = Connection::open(&name).expect("open");
    let mut reply = [0u8; 8];
    let n = with_deadline(|| conn.send_bytes(BULK_KIND, &payload, &mut reply)).expect("send");
    assert_eq!(n, 8);
    assert_eq!(u64::from_ne_bytes(reply), expected);

    let (r, seen_len) = join_within(server);
    r.expect("serve");
    assert_eq!(seen_len, 4 + LEN);
}
