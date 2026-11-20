//! Timed sends and receives (`send_timeout`, `send_bytes_timeout`,
//! `receive_timeout`): one test per blocked state a timeout can meet. Each test
//! holds a `Watchdog`, so a timeout that never fires ends the test binary with
//! a message instead of hanging the run.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use qnx_ipc::{
    Channel, ChannelFlags, Connection, Error, Message, NamedChannel, PulseCode, PulseKind,
    Received, RecvBuf,
};
use qnx_ipc_sys as sys;

const ETIMEDOUT: i32 = sys::ETIMEDOUT as i32;

/// The timeout the tests wait for.
const SHORT: Duration = Duration::from_millis(300);

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

/// Ends the whole test binary if it is still alive after 20 seconds, so a
/// timeout that does not fire fails the run instead of hanging it. Dropping it
/// disarms it.
struct Watchdog {
    _stop: mpsc::Sender<()>,
}

fn watchdog(test: &'static str) -> Watchdog {
    let (stop, stopped) = mpsc::channel::<()>();
    thread::spawn(move || {
        if let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(Duration::from_secs(20))
        {
            eprintln!("{test}: still blocked after 20 s, the timeout did not fire");
            std::process::exit(101);
        }
    });
    Watchdog { _stop: stop }
}

/// Checks that a timed call took about `timeout`: not less than 90% of it (the
/// kernel may round to its clock tick) and not more than 3 seconds over, a loose
/// bound because the QEMU target can be slow on a busy host.
fn assert_took(start: Instant, timeout: Duration, what: &str) {
    let took = start.elapsed();
    assert!(
        took >= timeout * 9 / 10,
        "{what}: returned after {took:?}, before its timeout {timeout:?}"
    );
    assert!(
        took < timeout + Duration::from_secs(3),
        "{what}: returned after {took:?}, long after its timeout {timeout:?}"
    );
}

fn test_name(test: &str) -> String {
    format!("qnx-ipc-test-{}-{test}", std::process::id())
}

#[test]
fn send_timeout_returns_reply_in_time() {
    let _dog = watchdog("send_timeout_returns_reply_in_time");
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let server = thread::spawn(move || {
        let mut buf = RecvBuf::with_capacity(64);
        match channel.receive_timeout(&mut buf, Duration::from_secs(10)) {
            Ok(Received::Message(req)) => {
                let ping: Ping = req.decode().expect("decode");
                req.reply(&Pong { n: ping.n + 1 }).expect("reply");
            }
            other => panic!("expected a message, got {other:?}"),
        };
    });
    let pong: Pong = conn
        .send_timeout(&Ping { n: 1 }, Duration::from_secs(5))
        .expect("send_timeout");
    assert_eq!(pong, Pong { n: 2 });
    server.join().unwrap();
}

#[test]
fn send_timeout_when_nobody_receives() {
    // The client stays send-blocked: nobody ever receives on the channel.
    let _dog = watchdog("send_timeout_when_nobody_receives");
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");

    let start = Instant::now();
    let r: qnx_ipc::Result<Pong> = conn.send_timeout(&Ping { n: 1 }, SHORT);
    assert_eq!(r, Err(Error::Os(ETIMEDOUT)));
    assert!(r.unwrap_err().is_timeout());
    assert_took(start, SHORT, "send_timeout");

    let mut reply = [0u8; 8];
    let start = Instant::now();
    let r = conn.send_bytes_timeout(0x210, &[1, 2, 3], &mut reply, SHORT);
    assert_eq!(r, Err(Error::Os(ETIMEDOUT)));
    assert_took(start, SHORT, "send_bytes_timeout");

    // A zero timeout fails at once.
    let start = Instant::now();
    let r: qnx_ipc::Result<Pong> = conn.send_timeout(&Ping { n: 1 }, Duration::ZERO);
    assert_eq!(r, Err(Error::Os(ETIMEDOUT)));
    assert!(start.elapsed() < Duration::from_secs(1), "zero send waited");
    drop(channel);
}

#[test]
fn send_timeout_to_named_channel_nobody_receives() {
    // A named channel has `_NTO_CHF_UNBLOCK`, which only changes the reply wait:
    // a message nobody has received yet still times out.
    let _dog = watchdog("send_timeout_to_named_channel_nobody_receives");
    let name = test_name("timeout-send");
    let channel = NamedChannel::attach(&name).expect("attach");
    let conn = Connection::open(&name).expect("open");

    let start = Instant::now();
    let r: qnx_ipc::Result<Pong> = conn.send_timeout(&Ping { n: 1 }, SHORT);
    assert_eq!(r, Err(Error::Os(ETIMEDOUT)));
    assert_took(start, SHORT, "send_timeout");
    drop(conn);
    drop(channel);
}

#[test]
fn send_timeout_while_server_holds_request() {
    // Without `UNBLOCK`, a client waiting for its reply times out at once.
    let _dog = watchdog("send_timeout_while_server_holds_request");
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let (got, got_rx) = mpsc::channel::<()>();
    let (release, release_rx) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut buf = RecvBuf::with_capacity(64);
        let received = channel
            .receive_timeout(&mut buf, Duration::from_secs(10))
            .expect("receive");
        let Received::Message(req) = received else {
            panic!("expected a message");
        };
        got.send(()).unwrap();
        // Hold the request until the client has timed out, then drop it. Its
        // ENOSYS goes nowhere: that client is no longer waiting.
        release_rx.recv_timeout(Duration::from_secs(10)).ok();
        drop(req);
    });

    let start = Instant::now();
    let r: qnx_ipc::Result<Pong> = conn.send_timeout(&Ping { n: 1 }, SHORT);
    assert_eq!(r, Err(Error::Os(ETIMEDOUT)));
    assert_took(start, SHORT, "send_timeout");
    got_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the server received the message, so the client was reply-blocked");
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn send_timeout_with_unblock_waits_for_the_answer() {
    // With `UNBLOCK`, QNX keeps a reply-blocked client blocked when its timeout
    // expires and sends the server an unblock pulse; the client gets the
    // server's answer, after the timeout.
    let _dog = watchdog("send_timeout_with_unblock_waits_for_the_answer");
    let channel = Channel::new(ChannelFlags::UNBLOCK).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let server = thread::spawn(move || {
        let mut req_buf = RecvBuf::with_capacity(64);
        let received = channel
            .receive_timeout(&mut req_buf, Duration::from_secs(10))
            .expect("receive");
        let Received::Message(req) = received else {
            panic!("expected a message");
        };
        let mut pulse_buf = RecvBuf::with_capacity(64);
        let kind = match channel.receive_timeout(&mut pulse_buf, Duration::from_secs(10)) {
            Ok(Received::Pulse(pulse)) => pulse.kind(),
            other => panic!("expected the unblock pulse, got {other:?}"),
        };
        req.reply(&Pong { n: 7 }).expect("reply");
        kind
    });

    let start = Instant::now();
    let r: qnx_ipc::Result<Pong> = conn.send_timeout(&Ping { n: 1 }, SHORT);
    let took = start.elapsed();
    assert_eq!(server.join().unwrap(), PulseKind::Unblock);
    assert_eq!(r, Ok(Pong { n: 7 }));
    assert!(took >= SHORT * 9 / 10, "client returned after {took:?}");
}

#[test]
fn receive_timeout_when_nothing_arrives() {
    let _dog = watchdog("receive_timeout_when_nothing_arrives");
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let mut buf = RecvBuf::with_capacity(64);
    let start = Instant::now();
    let r = channel.receive_timeout(&mut buf, SHORT);
    assert!(matches!(r, Err(Error::Os(ETIMEDOUT))), "got {r:?}");
    assert_took(start, SHORT, "receive_timeout");
}

#[test]
fn receive_timeout_zero_polls() {
    let _dog = watchdog("receive_timeout_zero_polls");
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let mut buf = RecvBuf::with_capacity(64);

    let start = Instant::now();
    let r = channel.receive_timeout(&mut buf, Duration::ZERO);
    assert!(matches!(r, Err(Error::Os(ETIMEDOUT))), "empty: got {r:?}");
    drop(r);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "zero receive waited"
    );

    let code = PulseCode::user(5).unwrap();
    conn.send_pulse(code, 42).expect("send_pulse");
    match channel.receive_timeout(&mut buf, Duration::ZERO) {
        Ok(Received::Pulse(pulse)) => {
            assert_eq!(pulse.kind(), PulseKind::User(5));
            assert_eq!(pulse.value, 42);
        }
        other => panic!("waiting pulse: got {other:?}"),
    };
}

/// Sends `raw` as it is (no header added) and returns the error, if any.
fn send_raw(conn: &Connection, raw: &[u8]) -> Option<Error> {
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
    let rc = unsafe { sys::MsgSendv(conn.coid(), &siov, 1, &riov, 1) };
    (rc == -1).then(Error::last_os)
}

#[test]
fn named_receive_timeout_spans_system_messages() {
    // A system message arrives during the wait. `receive_timeout` answers it
    // and keeps waiting only for what is left, so it still ends on time.
    let _dog = watchdog("named_receive_timeout_spans_system_messages");
    let name = test_name("timeout-recv");
    let channel = NamedChannel::attach(&name).expect("attach");
    let timeout = Duration::from_millis(800);
    let client_name = name.clone();
    let client = thread::spawn(move || {
        let conn = Connection::open(&client_name).expect("open");
        thread::sleep(Duration::from_millis(150));
        let read = (sys::_IO_READ as u16).to_ne_bytes();
        let err = send_raw(&conn, &[read[0], read[1], 0, 0]);
        // Returned, not dropped here: closing it would send the server a
        // disconnect pulse, which `receive_timeout` would rightly return.
        (conn, err)
    });

    let mut buf = RecvBuf::with_capacity(64);
    let start = Instant::now();
    let deadline = start + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match channel.receive_timeout(&mut buf, left) {
            // The other tests in this binary run at the same time and destroy
            // channels that this process still has connections to. QNX then
            // sends a CoidDeath pulse to every channel here that asked for
            // them, and a `name_attach` channel does. Such a pulse is a correct
            // result, so wait on for the time left.
            Ok(Received::Pulse(pulse)) if pulse.kind() == PulseKind::CoidDeath => continue,
            Err(Error::Os(ETIMEDOUT)) => break,
            other => panic!("got {other:?}"),
        };
    }
    assert_took(start, timeout, "named receive_timeout");
    let (conn, err) = client.join().unwrap();
    drop(conn);
    assert_eq!(
        err,
        Some(Error::Os(sys::ENOSYS as i32)),
        "the system message arrived during the wait and was answered"
    );
}
