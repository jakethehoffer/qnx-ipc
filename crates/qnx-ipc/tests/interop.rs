//! Interoperability with C. `c/ping_server.c` and `c/ping_client.c` speak the
//! ping protocol of `c/include/qnx_ipc_ping.h` through plain `name_attach`,
//! `name_open`, `MsgReceive` and `MsgSend`.
//!
//! Build the C programs with `tools/build-c.sh` and run with `QNX_RUN_WITH_C=1`,
//! which makes `tools/qnx-run` copy them next to this test binary. Both tests use
//! the fixed name `PING_NAME`, so they run one at a time under `LOCK`. Every wait
//! is bounded, and a spawned program is killed if the test ends early.

#[path = "common/ping.rs"]
mod ping;

use std::io::Read;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use qnx_ipc::{Connection, Handler, NamedChannel, RecvBuf, Request};

use ping::{Ping, Pong, PING_NAME};

/// Serializes the tests: they share one name.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    // A failed test poisons the lock; the next test must still run.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The path of a C program built next to this test binary.
fn c_program(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    exe.parent().expect("exe dir").join(name)
}

/// A spawned child that is killed and reaped if it is still running when dropped.
struct Spawned(Child);

impl Spawned {
    fn spawn(name: &str, args: &[&str]) -> Spawned {
        let path = c_program(name);
        let child = Command::new(&path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", path.display()));
        Spawned(child)
    }

    /// Waits up to `limit` for the child to exit and returns its status.
    fn wait_within(&mut self, limit: Duration) -> ExitStatus {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.0.try_wait().expect("try_wait") {
                return status;
            }
            assert!(Instant::now() < deadline, "child did not exit in time");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The child's stdout, read to the end. Call after it exited.
    fn stdout(&mut self) -> String {
        let mut out = String::new();
        self.0
            .stdout
            .take()
            .expect("stdout piped")
            .read_to_string(&mut out)
            .expect("read stdout");
        out
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Opens `name`, retrying for up to 5 s while the server starts.
fn open_within_5s(name: &str) -> Connection {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match Connection::open(name) {
            Ok(conn) => return conn,
            Err(e) => {
                assert!(Instant::now() < deadline, "server never appeared: {e:?}");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[test]
fn rust_client_talks_to_c_server() {
    let _guard = lock();
    let mut server = Spawned::spawn("ping_server", &[]);

    let conn = open_within_5s(PING_NAME);
    // The server kills itself with SIGALRM after 30 s, which fails this send too.
    let pong: Pong = conn.send(&Ping { n: 1 }).expect("send");
    assert_eq!(pong, Pong { n: 2 });
    drop(conn);

    // The server exits by itself after one ping.
    let status = server.wait_within(Duration::from_secs(5));
    assert!(status.success(), "ping_server exited with {status:?}");
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

#[test]
fn c_client_talks_to_rust_server() {
    let _guard = lock();
    let channel = NamedChannel::attach(PING_NAME).expect("attach");
    let server = thread::spawn(move || {
        let mut buf = RecvBuf::with_capacity(256);
        channel.serve(&mut buf, &mut OnePong)
    });

    let mut client = Spawned::spawn("ping_client", &["5"]);
    // The client kills itself with SIGALRM after 30 s; give it a little longer.
    let status = client.wait_within(Duration::from_secs(35));
    assert!(status.success(), "ping_client exited with {status:?}");
    assert_eq!(client.stdout(), "pong 6\n");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !server.is_finished() {
        assert!(Instant::now() < deadline, "server thread did not finish");
        thread::sleep(Duration::from_millis(10));
    }
    server.join().expect("server panicked").expect("serve");
}
