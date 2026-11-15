//! Serves `/dev/counter` until killed: `cat /dev/counter` prints the count,
//! `echo 5 > /dev/counter` adds 5 to it, and two `devctl` commands read the
//! count as a number or reset it.
//!
//! Run as root (attaching under `/dev` needs it).

use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};

use qnx_ipc::Error;
use qnx_ipc_sys as sys;
use qnx_resmgr::{Device, ResourceManager};

const EINVAL: i32 = sys::EINVAL as i32;
const ENOTTY: i32 = sys::ENOTTY as i32;

/// `_DCMD_MISC` from `<devctl.h>`: the class for your own commands.
const DCMD_MISC: u32 = 0x05;

/// `__DIOF(_DCMD_MISC, 1, uint64_t)`: the count comes back as a `u64`.
const DCMD_COUNTER_GET: u32 =
    ((size_of::<u64>() as u32) << 16) | (DCMD_MISC << 8) | 1 | sys::_POSIX_DEVDIR_FROM;

/// `__DION(_DCMD_MISC, 2)`: sets the count to 0, no data.
const DCMD_COUNTER_RESET: u32 = (DCMD_MISC << 8) | 2 | sys::_POSIX_DEVDIR_NONE;

struct Counter {
    count: AtomicU64,
}

impl Device for Counter {
    fn read(&self) -> qnx_ipc::Result<Vec<u8>> {
        let count = self.count.load(Ordering::SeqCst);
        Ok(format!("{count}\n").into_bytes())
    }

    fn write(&self, data: &[u8]) -> qnx_ipc::Result<usize> {
        let text = std::str::from_utf8(data).map_err(|_| Error::Os(EINVAL))?;
        let n: u64 = text.trim().parse().map_err(|_| Error::Os(EINVAL))?;
        self.count.fetch_add(n, Ordering::SeqCst);
        Ok(data.len())
    }

    fn devctl(&self, cmd: u32, data: &mut [u8]) -> qnx_ipc::Result<usize> {
        match cmd {
            DCMD_COUNTER_GET => {
                let count = self.count.load(Ordering::SeqCst).to_ne_bytes();
                let out = data.get_mut(..count.len()).ok_or(Error::Os(EINVAL))?;
                out.copy_from_slice(&count);
                Ok(count.len())
            }
            DCMD_COUNTER_RESET => {
                self.count.store(0, Ordering::SeqCst);
                Ok(0)
            }
            _ => Err(Error::Os(ENOTTY)),
        }
    }
}

fn main() -> qnx_ipc::Result<()> {
    eprintln!("serving /dev/counter");
    let counter = Counter {
        count: AtomicU64::new(0),
    };
    ResourceManager::new("/dev/counter")?.run(counter)
}
