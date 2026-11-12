//! Serves `/dev/hello` until killed: `cat /dev/hello` prints `hello from Rust`.
//!
//! Run as root (attaching under `/dev` needs it).

use qnx_resmgr::{Device, ResourceManager};

struct Hello;

impl Device for Hello {
    fn read(&self) -> qnx_ipc::Result<Vec<u8>> {
        Ok(b"hello from Rust\n".to_vec())
    }
}

fn main() -> qnx_ipc::Result<()> {
    eprintln!("serving /dev/hello");
    ResourceManager::new("/dev/hello")?.run(Hello)
}
