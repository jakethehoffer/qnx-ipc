//! Pulses: user pulses, pulse code validation, periodic timer pulses and the
//! disconnect pulse when a client process exits. Every blocking receive runs
//! under a kernel timeout, so a broken implementation fails instead of hanging.

use std::process::Command;
use std::time::{Duration, Instant};

use qnx_ipc::{
    Channel, ChannelFlags, Connection, Error, Pulse, PulseCode, PulseKind, PulseTimer, Received,
    RecvBuf,
};
use qnx_ipc_sys as sys;

/// Env var that turns this test binary into the child of
/// `client_exit_delivers_disconnect`.
const CHILD_ENV: &str = "QNX_IPC_CHILD";
const PARENT_PID_ENV: &str = "QNX_IPC_PARENT_PID";
const PARENT_CHID_ENV: &str = "QNX_IPC_PARENT_CHID";

/// Runs `f` with a kernel timeout of `limit` armed on the calling thread for the
/// next receive, send or reply block (`TimerTimeout`). A blocked `MsgReceive`
/// then fails with `ETIMEDOUT` instead of hanging the test run.
///
/// The kernel clears the timeout when the next kernel call returns, so `f` must
/// make its blocking call before any other kernel call.
fn with_timeout<T>(limit: Duration, f: impl FnOnce() -> T) -> T {
    let flags =
        (1u32 << sys::STATE_RECEIVE) | (1u32 << sys::STATE_SEND) | (1u32 << sys::STATE_REPLY);
    let ns: u64 = limit.as_nanos().try_into().unwrap_or(u64::MAX).max(1);
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

/// Receives one pulse within `limit`, failing the test on a message or timeout.
fn receive_pulse(channel: &Channel, buf: &mut RecvBuf, limit: Duration) -> Pulse {
    match with_timeout(limit, || channel.receive(buf)).expect("receive") {
        Received::Pulse(p) => p,
        Received::Message(req) => panic!("expected a pulse, got a message of kind {}", req.kind()),
    }
}

#[test]
fn user_pulse_arrives() {
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    conn.send_pulse(PulseCode::user(7).expect("code"), 1234)
        .expect("send_pulse");
    let mut buf = RecvBuf::with_capacity(64);
    let pulse = receive_pulse(&channel, &mut buf, Duration::from_secs(5));
    assert_eq!(pulse.kind(), PulseKind::User(7));
    assert_eq!(pulse.value, 1234);
}

#[test]
fn bad_pulse_code_rejected() {
    assert_eq!(PulseCode::user(200), Err(Error::InvalidPulseCode(200)));
    assert!(PulseCode::user(127).is_ok());
    assert!(PulseCode::user(0).is_ok());
}

#[test]
fn pulse_kind_maps_kernel_codes() {
    let kind = |code: i32| {
        Pulse {
            code: code as i8,
            value: 0,
        }
        .kind()
    };
    assert_eq!(kind(sys::_PULSE_CODE_DISCONNECT), PulseKind::Disconnect);
    assert_eq!(kind(sys::_PULSE_CODE_UNBLOCK), PulseKind::Unblock);
    assert_eq!(kind(sys::_PULSE_CODE_THREADDEATH), PulseKind::ThreadDeath);
    assert_eq!(kind(sys::_PULSE_CODE_COIDDEATH), PulseKind::CoidDeath);
    assert_eq!(kind(-1), PulseKind::Other(-1));
    assert_eq!(kind(127), PulseKind::User(127));
}

#[test]
fn periodic_timer_delivers_pulses() {
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let code = PulseCode::user(9).expect("code");
    let _timer = PulseTimer::periodic(&conn, code, Duration::from_millis(100)).expect("timer");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = RecvBuf::with_capacity(64);
    let mut count = 0;
    while count < 3 {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "only {count} timer pulses within the deadline"
        );
        let pulse = receive_pulse(&channel, &mut buf, left);
        assert_eq!(pulse.kind(), PulseKind::User(9));
        count += 1;
    }
    assert!(count >= 3);
}

#[test]
fn zero_period_timer_rejected() {
    let channel = Channel::new(ChannelFlags::empty()).expect("channel");
    let conn = Connection::to_local(channel.id()).expect("connection");
    let code = PulseCode::user(9).expect("code");
    let r = PulseTimer::periodic(&conn, code, Duration::ZERO);
    assert_eq!(r.err(), Some(Error::Os(sys::EINVAL as i32)));
}

/// The child side of `client_exit_delivers_disconnect`. Without `QNX_IPC_CHILD` in
/// the environment it does nothing. With it, it connects to the parent's channel
/// and exits the process without detaching.
#[test]
fn child_connect_and_exit() {
    if std::env::var(CHILD_ENV).as_deref() != Ok("connect-and-exit") {
        return;
    }
    let pid: i32 = std::env::var(PARENT_PID_ENV).unwrap().parse().unwrap();
    let chid: i32 = std::env::var(PARENT_CHID_ENV).unwrap().parse().unwrap();
    let conn = Connection::to_process(pid, chid).expect("child connect");
    // Exit with the connection still attached: the kernel must clean it up and
    // tell the parent.
    std::mem::forget(conn);
    std::process::exit(0);
}

#[test]
fn client_exit_delivers_disconnect() {
    let channel = Channel::new(ChannelFlags::DISCONNECT).expect("channel");
    // The child's output is captured (not mixed into this run's output) and shown
    // only if it fails.
    let child = Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", "child_connect_and_exit", "--nocapture"])
        .env(CHILD_ENV, "connect-and-exit")
        .env(PARENT_PID_ENV, std::process::id().to_string())
        .env(PARENT_CHID_ENV, channel.id().to_string())
        .output()
        .expect("spawn child");
    assert!(
        child.status.success(),
        "child failed: {:?}\n{}{}",
        child.status,
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );

    let mut buf = RecvBuf::with_capacity(64);
    let pulse = receive_pulse(&channel, &mut buf, Duration::from_secs(5));
    assert_eq!(pulse.kind(), PulseKind::Disconnect);
}
