//! Requests a server holds while it keeps receiving (deferred replies): which
//! held request an unblock pulse is about, byte replies, and a client process
//! that dies while its request is held. Each test holds a `Watchdog`, and every
//! receive has a timeout, so a broken implementation fails instead of hanging.

use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use qnx_ipc::{
    Channel, ChannelFlags, Connection, Error, Message, Pulse, PulseKind, Received, RecvBuf, Request,
};
use qnx_ipc_sys as sys;

const ETIMEDOUT: i32 = sys::ETIMEDOUT as i32;

/// The client timeout the unblock test waits for.
const SHORT: Duration = Duration::from_millis(300);
/// The bound on every wait that should not time out.
const LONG: Duration = Duration::from_secs(10);

/// Env vars that turn this test binary into the client process of
/// `dead_client_request_cannot_answer_the_next_client`.
const CHILD_ENV: &str = "QNX_IPC_HELD_CHILD";
const PARENT_PID_ENV: &str = "QNX_IPC_PARENT_PID";
const PARENT_CHID_ENV: &str = "QNX_IPC_PARENT_CHID";
const PING_ENV: &str = "QNX_IPC_PING";

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

/// Serializes the tests. QNX numbers a server's scoids across all of its
/// channels, so a connection another test made at the wrong moment would take
/// the number the dead-client test watches.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    // A failed test poisons the lock; the next test must still run.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ends the whole test binary if it is still alive after 60 seconds. Dropping
/// it disarms it.
struct Watchdog {
    _stop: mpsc::Sender<()>,
}

fn watchdog(test: &'static str) -> Watchdog {
    let (stop, stopped) = mpsc::channel::<()>();
    thread::spawn(move || {
        if let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(Duration::from_secs(60))
        {
            eprintln!("{test}: still running after 60 s");
            std::process::exit(101);
        }
    });
    Watchdog { _stop: stop }
}

/// Receives one message within `LONG`, failing the test on a pulse or timeout.
fn receive_message<'b>(channel: &'b Channel, buf: &'b mut RecvBuf, what: &str) -> Request<'b> {
    match channel.receive_timeout(buf, LONG) {
        Ok(Received::Message(req)) => req,
        other => panic!("{what}: expected a message, got {other:?}"),
    }
}

/// Receives one pulse within `LONG`, failing the test on a message or timeout.
fn receive_pulse(channel: &Channel, buf: &mut RecvBuf, what: &str) -> Pulse {
    match channel.receive_timeout(buf, LONG) {
        Ok(Received::Pulse(pulse)) => pulse,
        other => panic!("{what}: expected a pulse, got {other:?}"),
    }
}

#[test]
fn unblock_pulse_names_the_request_that_timed_out() {
    // Two clients wait for their replies on an `UNBLOCK` channel. One of them
    // times out. The unblock pulse carries its request's receive id, so the
    // server can answer that one and leave the other waiting.
    let _lock = lock();
    let _dog = watchdog("unblock_pulse_names_the_request_that_timed_out");
    let channel = Channel::new(ChannelFlags::UNBLOCK).expect("channel");
    let chid = channel.id();

    let patient = thread::spawn(move || {
        let conn = Connection::to_local(chid).expect("connection");
        conn.send_timeout::<Ping, Pong>(&Ping { n: 1 }, LONG)
    });
    let mut patient_buf = RecvBuf::with_capacity(64);
    let patient_req = receive_message(&channel, &mut patient_buf, "patient client");
    assert_eq!(patient_req.decode::<Ping>(), Ok(Ping { n: 1 }));

    let hasty = thread::spawn(move || {
        let conn = Connection::to_local(chid).expect("connection");
        conn.send_timeout::<Ping, Pong>(&Ping { n: 2 }, SHORT)
    });
    let mut hasty_buf = RecvBuf::with_capacity(64);
    let hasty_req = receive_message(&channel, &mut hasty_buf, "hasty client");
    assert_eq!(hasty_req.decode::<Ping>(), Ok(Ping { n: 2 }));

    let mut pulse_buf = RecvBuf::with_capacity(64);
    let pulse = receive_pulse(&channel, &mut pulse_buf, "unblock");
    eprintln!(
        "unblock pulse value_long {:#x}, hasty rcvid {:#x}, patient rcvid {:#x}",
        pulse.value_long,
        hasty_req.rcvid().raw(),
        patient_req.rcvid().raw()
    );
    assert_eq!(pulse.kind(), PulseKind::Unblock);
    assert_eq!(pulse.rcvid(), Some(hasty_req.rcvid()));
    assert_ne!(pulse.rcvid(), Some(patient_req.rcvid()));

    hasty_req.reply_error(ETIMEDOUT).expect("answer hasty");
    assert_eq!(hasty.join().unwrap(), Err(Error::Os(ETIMEDOUT)));
    patient_req.reply(&Pong { n: 2 }).expect("answer patient");
    assert_eq!(patient.join().unwrap(), Ok(Pong { n: 2 }));
}

#[test]
fn reply_bytes_reaches_send_bytes() {
    let _lock = lock();
    let _dog = watchdog("reply_bytes_reaches_send_bytes");
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let mut buf = RecvBuf::with_capacity(64);
            let req = receive_message(&channel, &mut buf, "bytes request");
            assert_eq!(req.kind(), 0x210);
            req.reply_bytes(b"hello").expect("reply_bytes");
        }
    });

    let mut reply = [0u8; 16];
    let n = conn
        .send_bytes_timeout(0x210, &[1, 2, 3], &mut reply, LONG)
        .expect("send_bytes");
    assert_eq!(&reply[..n], b"hello");

    // A reply buffer shorter than the reply gets its first bytes.
    let mut short = [0u8; 3];
    let n = conn
        .send_bytes_timeout(0x210, &[1, 2, 3], &mut short, LONG)
        .expect("send_bytes");
    assert_eq!(&short[..n], b"hel");
    server.join().unwrap();
}

/// The client process of `dead_client_request_cannot_answer_the_next_client`,
/// which runs this binary again with `--ignored --exact child_send_and_wait`.
/// It is `#[ignore]`d so a normal test run skips it, and does nothing without
/// `QNX_IPC_HELD_CHILD`. With it, it sends one `Ping` to the parent's channel
/// and exits 0 if the answer is the matching `Pong`, else with another code.
#[test]
#[ignore = "helper process for dead_client_request_cannot_answer_the_next_client"]
fn child_send_and_wait() {
    if std::env::var(CHILD_ENV).as_deref() != Ok("send") {
        return;
    }
    let pid: i32 = std::env::var(PARENT_PID_ENV).unwrap().parse().unwrap();
    let chid: i32 = std::env::var(PARENT_CHID_ENV).unwrap().parse().unwrap();
    let n: u32 = std::env::var(PING_ENV).unwrap().parse().unwrap();
    let conn = Connection::to_process(pid, chid).expect("child connect");
    let code = match conn.send::<Ping, Pong>(&Ping { n }) {
        Ok(pong) if pong.n == n + 1 => 0,
        Ok(pong) => {
            eprintln!("client {n}: wrong answer {pong:?}");
            4
        }
        Err(e) => {
            eprintln!("client {n}: send failed: {e:?}");
            3
        }
    };
    std::process::exit(code);
}

/// A client process that is killed and reaped if it is still running when
/// dropped.
struct Client(Child);

impl Client {
    /// Starts a client process that sends `Ping { n }` to `channel`.
    fn spawn(channel: &Channel, n: u32) -> Client {
        let child = Command::new(std::env::current_exe().expect("current_exe"))
            .args(["--ignored", "--exact", "child_send_and_wait", "--nocapture"])
            .env(CHILD_ENV, "send")
            .env(PARENT_PID_ENV, std::process::id().to_string())
            .env(PARENT_CHID_ENV, channel.id().to_string())
            .env(PING_ENV, n.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn client");
        Client(child)
    }

    /// Kills the process and waits until it is gone.
    fn kill(&mut self) {
        self.0.kill().expect("kill client");
        self.0.wait().expect("reap client");
    }

    /// Waits up to `LONG` for the process to exit and returns its status.
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + LONG;
        loop {
            if let Some(status) = self.0.try_wait().expect("try_wait") {
                return status;
            }
            assert!(Instant::now() < deadline, "client did not exit in time");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The message counter inside a receive id, as seen on QNX 8.0 x86_64 (it is
/// not documented): 14 bits from bit 48 that go up by one with every message
/// received on the channel and start again after 16384 messages. The rest of
/// the id is the client's scoid and its thread.
fn counter(req: &Request<'_>) -> i64 {
    (req.rcvid().raw() >> 48) & 0x3fff
}

/// Sends filler messages on `filler` from another thread and receives them
/// until the channel's message counter stands one before `wanted`, so the next
/// message received gets `wanted`.
fn advance_counter_to(channel: &Channel, filler: &Connection, wanted: i64) {
    thread::scope(|scope| {
        scope.spawn(|| {
            let mut stop = [0u8; 1];
            while stop[0] == 0 {
                filler
                    .send_bytes_timeout(0x220, &[0], &mut stop, LONG)
                    .expect("filler send");
            }
        });
        let mut buf = RecvBuf::with_capacity(64);
        // Two rounds of the counter are more than enough to reach any value.
        for _ in 0..2 * 16384 {
            let req = receive_message(channel, &mut buf, "filler");
            let last = (counter(&req) + 1) & 0x3fff == wanted;
            req.reply_bytes(&[u8::from(last)]).expect("answer filler");
            if last {
                return;
            }
        }
        panic!("the message counter never reached {wanted:#x}");
    });
}

#[test]
fn dead_client_request_cannot_answer_the_next_client() {
    // A client process dies while the server holds its request, and a new
    // client arrives before the server lets go of that request. The dead
    // client's answer must not reach the new client.
    //
    // QNX gives the new client the dead one's scoid as soon as the server
    // detaches it, and a receive id is the scoid, the thread and a short
    // message counter. The test advances the counter to where it stood for
    // the held request, the one case in which the two receive ids would be
    // the same. With the detach done at the disconnect pulse, the second
    // client then gets the dead client's ENOSYS.
    let _lock = lock();
    let _dog = watchdog("dead_client_request_cannot_answer_the_next_client");
    let channel = Channel::new(ChannelFlags::DISCONNECT).expect("channel");
    // This process's own connection, for the filler messages below. Made first
    // so that it has its scoid before any client has one, and kept to the end:
    // closing it would send the channel this process's own disconnect pulse.
    let filler = Connection::to_local(channel.id()).expect("filler connection");

    let mut first = Client::spawn(&channel, 1);
    let mut first_buf = RecvBuf::with_capacity(64);
    let held = receive_message(&channel, &mut first_buf, "first client");
    assert_eq!(held.decode::<Ping>(), Ok(Ping { n: 1 }));
    let held_id = held.rcvid();
    let held_scoid = held.sender().scoid;
    let held_counter = counter(&held);

    first.kill();
    let mut pulse_buf = RecvBuf::with_capacity(64);
    let pulse = receive_pulse(&channel, &mut pulse_buf, "first client's disconnect");
    assert_eq!(pulse.kind(), PulseKind::Disconnect);
    assert_eq!(pulse.scoid, held_scoid, "the pulse names the dead client");

    advance_counter_to(&channel, &filler, held_counter);
    let mut second = Client::spawn(&channel, 10);
    let mut second_buf = RecvBuf::with_capacity(64);
    let next = receive_message(&channel, &mut second_buf, "second client");
    assert_eq!(next.decode::<Ping>(), Ok(Ping { n: 10 }));
    let next_id = next.rcvid();
    let next_scoid = next.sender().scoid;
    eprintln!(
        "held: rcvid {:#x} scoid {held_scoid:#x}; next: rcvid {:#x} scoid {next_scoid:#x}",
        held_id.raw(),
        next_id.raw()
    );

    // Dropping the dead client's request answers ENOSYS on its receive id.
    drop(held);
    let replied = next.reply(&Pong { n: 11 });
    let status = second.wait();
    assert!(
        status.success(),
        "the second client did not get its own answer ({status:?}, reply {replied:?})"
    );
    assert_eq!(replied, Ok(()));
    // The server connection of the dead client was kept while its request was
    // held, so QNX could not give its number, or its receive id, to the next
    // client.
    assert_ne!(next_scoid, held_scoid);
    assert_ne!(next_id, held_id);

    let pulse = receive_pulse(&channel, &mut pulse_buf, "second client's disconnect");
    assert_eq!(pulse.kind(), PulseKind::Disconnect);
    assert_eq!(pulse.scoid, next_scoid);

    // Dropping the held request released the dead client's server connection:
    // two more clients, alive together, get the two numbers that are free
    // again. Had the dead client's scoid leaked, one of them would get a new
    // number.
    let mut third = Client::spawn(&channel, 20);
    let mut third_buf = RecvBuf::with_capacity(64);
    let third_req = receive_message(&channel, &mut third_buf, "third client");
    let mut fourth = Client::spawn(&channel, 30);
    let mut fourth_buf = RecvBuf::with_capacity(64);
    let fourth_req = receive_message(&channel, &mut fourth_buf, "fourth client");
    let mut reused = [third_req.sender().scoid, fourth_req.sender().scoid];
    reused.sort_unstable();
    let mut freed = [held_scoid, next_scoid];
    freed.sort_unstable();
    third_req.reply(&Pong { n: 21 }).expect("answer third");
    fourth_req.reply(&Pong { n: 31 }).expect("answer fourth");
    assert!(third.wait().success());
    assert!(fourth.wait().success());
    assert_eq!(reused, freed, "the dead client's scoid was not released");
    drop(filler);
}
