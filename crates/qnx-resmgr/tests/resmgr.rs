//! Resource managers end to end: real clients (`cat`, `std::fs`, raw `devctl`)
//! talk to a `Device` attached under `/dev/qnx-ipc-test/<pid>/`, so parallel
//! tests and test runs never share a path.

use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use qnx_ipc::{Error, Result};
use qnx_ipc_sys as sys;
use qnx_resmgr::{Device, ResourceManager, RunningManager};

const EIO: i32 = sys::EIO as i32;
const EINVAL: i32 = sys::EINVAL as i32;
const ENOTTY: i32 = sys::ENOTTY as i32;

/// A path no other test (or test process) uses.
fn test_path(name: &str) -> String {
    format!("/dev/qnx-ipc-test/{}/{name}", std::process::id())
}

fn spawn<D: Device>(name: &str, device: D) -> (String, RunningManager) {
    let path = test_path(name);
    let running = ResourceManager::new(&path)
        .expect("valid path")
        .spawn(device)
        .expect("spawn resource manager");
    (path, running)
}

/// Drops `running` on another thread and fails the test if the drop does not
/// finish within 10 seconds, so a dispatch loop that never stops fails instead
/// of hanging the run.
fn drop_with_deadline(running: RunningManager) {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        drop(running);
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("RunningManager::drop did not finish within 10 s");
}

struct Fixed(&'static [u8]);

impl Device for Fixed {
    fn read(&self) -> Result<Vec<u8>> {
        Ok(self.0.to_vec())
    }
}

#[test]
fn cat_reads_device_content() {
    let (path, running) = spawn("cat", Fixed(b"hello\n"));
    let out = Command::new("cat").arg(&path).output().expect("run cat");
    assert!(out.status.success(), "cat failed: {out:?}");
    assert_eq!(out.stdout, b"hello\n");
    drop_with_deadline(running);
}

/// 64 KiB of content that differs at every position a short read could skip
/// or repeat; counts `read` calls.
struct Patterned {
    content: Vec<u8>,
    reads: Arc<AtomicUsize>,
}

impl Device for Patterned {
    fn read(&self) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.content.clone())
    }
}

#[test]
fn chunked_read_returns_full_content_then_eof() {
    let content: Vec<u8> = (0..64 * 1024u32)
        .map(|i| (i.wrapping_mul(7) % 251) as u8)
        .collect();
    let reads = Arc::new(AtomicUsize::new(0));
    let device = Patterned {
        content: content.clone(),
        reads: Arc::clone(&reads),
    };
    let (path, running) = spawn("chunked", device);

    let mut file = File::open(&path).expect("open");
    let mut got = Vec::new();
    let mut buf = [0u8; 100];
    loop {
        let n = file.read(&mut buf).expect("read");
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
        assert!(
            got.len() <= content.len(),
            "read past the end of the content"
        );
    }
    assert_eq!(got.len(), content.len());
    assert!(got == content, "content differs");
    // At EOF a further read keeps returning 0 bytes.
    assert_eq!(file.read(&mut buf).expect("read at EOF"), 0);
    // One snapshot per open: the device was asked once.
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    drop(file);
    drop_with_deadline(running);
}

/// Empty until `filled` is set, then `"late\n"`; counts `read` calls.
struct FillsLater {
    filled: Arc<AtomicBool>,
    reads: Arc<AtomicUsize>,
}

impl Device for FillsLater {
    fn read(&self) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.filled.load(Ordering::SeqCst) {
            Ok(b"late\n".to_vec())
        } else {
            Ok(Vec::new())
        }
    }
}

#[test]
fn empty_snapshot_stays_at_eof_for_its_open() {
    let filled = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let device = FillsLater {
        filled: Arc::clone(&filled),
        reads: Arc::clone(&reads),
    };
    let (path, running) = spawn("fills-later", device);

    let mut file = File::open(&path).expect("open");
    let mut buf = [0u8; 64];
    assert_eq!(file.read(&mut buf).expect("first read"), 0, "empty device");
    filled.store(true, Ordering::SeqCst);
    // Same open: still end of file, and the device was not asked again.
    assert_eq!(
        file.read(&mut buf).expect("read after EOF"),
        0,
        "a reader that saw EOF must not get data later on the same open"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1, "one snapshot per open");
    drop(file);

    // A new open takes a new snapshot and sees the new content.
    assert_eq!(std::fs::read(&path).expect("new open"), b"late\n");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    drop_with_deadline(running);
}

/// Stores what is written and reads it back.
#[derive(Default)]
struct Store {
    data: Mutex<Vec<u8>>,
}

impl Device for Store {
    fn read(&self) -> Result<Vec<u8>> {
        Ok(self.data.lock().unwrap().clone())
    }

    fn write(&self, data: &[u8]) -> Result<usize> {
        self.data.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
}

#[test]
fn write_then_read_back() {
    let (path, running) = spawn("store", Store::default());
    let payload: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&path, &payload).expect("write");
    assert_eq!(std::fs::read(&path).expect("read back"), payload);
    drop_with_deadline(running);
}

/// The custom command: class 0x01, number 0x01, so its command part (the low
/// 16 bits, `class << 8 | number`) is 0x0101.
const DOUBLE_CMD: u32 = 0x0101;

/// The full devctl command word as QNX's `__DIOTF(class, cmd, data)` macro builds
/// it in `<devctl.h>`: `sizeof(data) << 16 | class << 8 | cmd |
/// _POSIX_DEVDIR_TOFROM`. The data is a `u32` that goes to the device and back.
const DCMD_DOUBLE: u32 =
    ((std::mem::size_of::<u32>() as u32) << 16) | DOUBLE_CMD | sys::_POSIX_DEVDIR_TOFROM;

/// Doubles the `u32` it is given.
struct Doubler;

impl Device for Doubler {
    fn read(&self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn devctl(&self, cmd: u32, data: &mut [u8]) -> Result<usize> {
        if cmd != DCMD_DOUBLE {
            return Err(Error::Os(ENOTTY));
        }
        let input: [u8; 4] = data
            .get(..4)
            .and_then(|b| b.try_into().ok())
            .ok_or(Error::Os(EINVAL))?;
        let doubled = u32::from_ne_bytes(input).wrapping_mul(2);
        data[..4].copy_from_slice(&doubled.to_ne_bytes());
        Ok(4)
    }
}

#[test]
fn devctl_round_trip() {
    let (path, running) = spawn("doubler", Doubler);
    let file = File::options()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open");
    let mut value: u32 = 21;
    let mut info: i32 = -1;
    // SAFETY: `value` is a live, writable u32 for the whole call and `nbytes` is
    // its size; `info` is a live i32.
    let status = unsafe {
        sys::devctl(
            file.as_raw_fd(),
            DCMD_DOUBLE as i32,
            (&mut value as *mut u32).cast(),
            std::mem::size_of::<u32>(),
            &mut info,
        )
    };
    assert_eq!(status, sys::EOK as i32, "devctl failed");
    assert_eq!(value, 42);
    assert_eq!(info, 0);

    // A command the device does not know fails with the device's errno.
    let unknown = ((std::mem::size_of::<u32>() as u32) << 16) | 0x0102 | sys::_POSIX_DEVDIR_TOFROM;
    // SAFETY: as above.
    let status = unsafe {
        sys::devctl(
            file.as_raw_fd(),
            unknown as i32,
            (&mut value as *mut u32).cast(),
            std::mem::size_of::<u32>(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, ENOTTY);
    drop(file);
    drop_with_deadline(running);
}

#[test]
fn drop_detaches_path() {
    let (path, running) = spawn("detach", Fixed(b"x"));
    let stats = running.open_file_stats();
    std::fs::metadata(&path).expect("path exists while the manager runs");
    // One file opened, read (so it holds a snapshot) and closed...
    assert_eq!(std::fs::read(&path).expect("read"), b"x");
    // ...and one still open, with a snapshot, when the manager stops.
    let mut open = File::open(&path).expect("open");
    let mut buf = [0u8; 8];
    assert_eq!(open.read(&mut buf).expect("read"), 1);
    let opened = stats.opened.load(Ordering::SeqCst);
    assert!(opened >= 2, "expected at least 2 OCBs, got {opened}");
    assert_eq!(
        stats.closed.load(Ordering::SeqCst),
        opened - 1,
        "only the still-open file should hold an OCB before the drop"
    );

    drop_with_deadline(running);
    assert!(
        std::fs::metadata(&path).is_err(),
        "path still exists after the RunningManager was dropped"
    );
    // Every OCB, including the one of the file still open, was freed.
    assert_eq!(
        stats.closed.load(Ordering::SeqCst),
        stats.opened.load(Ordering::SeqCst),
        "an open file's OCB leaked when the manager stopped"
    );
    assert!(
        open.read(&mut buf).is_err(),
        "a file opened before the drop still reads"
    );
}

/// Panics on the first `read`, then behaves.
struct PanicsOnce {
    panicked: AtomicBool,
}

impl Device for PanicsOnce {
    fn read(&self) -> Result<Vec<u8>> {
        if !self.panicked.swap(true, Ordering::SeqCst) {
            panic!("device read panics (expected by the test)");
        }
        Ok(b"recovered".to_vec())
    }
}

#[test]
fn panicking_device_returns_eio() {
    let device = PanicsOnce {
        panicked: AtomicBool::new(false),
    };
    let (path, running) = spawn("panics", device);
    let err = std::fs::read(&path).expect_err("read of a panicking device must fail");
    assert_eq!(err.raw_os_error(), Some(EIO));
    // The manager keeps serving: a new open works.
    assert_eq!(
        std::fs::read(&path).expect("read after panic"),
        b"recovered"
    );
    drop_with_deadline(running);
}
