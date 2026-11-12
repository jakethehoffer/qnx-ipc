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
//!
//! [`ResourceManager::run`] serves on the calling thread;
//! [`ResourceManager::spawn`] serves on a new thread and returns a
//! [`RunningManager`], whose drop detaches the path and closes the files still
//! open on it. Each open file reads its own snapshot of [`Device::read`], so
//! small or chunked reads return the whole content once and then end of file.
//! [`Device::write`] gets each write's full payload and [`Device::devctl`] the
//! commands the framework does not handle itself. A device method that returns
//! an error or panics gives its client an `errno` (`EIO` for a panic) and the
//! manager keeps serving.
//!
//! Inside, generic `extern "C"` callbacks connect QNX's dispatch and `iofunc`
//! layers to the trait; `docs/architecture.md` in the repository explains why
//! the pointers they use stay valid.
#![deny(missing_docs)]

mod device;
mod manager;
mod trampoline;

pub use device::Device;
pub use manager::{ResourceManager, RunningManager};
#[doc(hidden)]
pub use trampoline::OpenFileStats;
