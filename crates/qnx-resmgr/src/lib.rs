//! QNX 8.0 resource managers in safe Rust.
//!
//! Implement [`Device`] and attach it at a path with [`ResourceManager`]. Every
//! QNX program can then use the path as a file: `cat` it, `read` and `write` it,
//! or send it `devctl` commands. The framework (QNX's `iofunc` layer) handles
//! open, close, stat and permissions; your device only sees its data.
//!
//! ```no_run
//! use qnx_resmgr::{Device, ResourceManager};
//!
//! struct Hello;
//!
//! impl Device for Hello {
//!     fn read(&self) -> qnx_ipc::Result<Vec<u8>> {
//!         Ok(b"hello from Rust\n".to_vec())
//!     }
//! }
//!
//! ResourceManager::new("/dev/hello")?.run(Hello)?;
//! # Ok::<(), qnx_ipc::Error>(())
//! ```

mod device;
mod manager;
mod trampoline;

pub use device::Device;
pub use manager::{ResourceManager, RunningManager};
