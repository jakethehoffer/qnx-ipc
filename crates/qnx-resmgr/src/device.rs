//! The [`Device`] trait: what a resource manager serves.

use qnx_ipc::{Error, Result};
use qnx_ipc_sys as sys;

/// A device served at a path by a [`ResourceManager`](crate::ResourceManager).
///
/// The methods see no QNX types. Errors go back to the client: an
/// [`Error::Os`] with a positive `errno` value is returned as that `errno`,
/// any other error as `EIO`. A method that panics is caught; its client gets
/// `EIO` and the manager keeps serving.
pub trait Device: Send + Sync + 'static {
    /// Returns the device's content.
    ///
    /// Called once per open file, on its first read (and again whenever a read
    /// starts from offset 0). The result is kept as that open file's snapshot:
    /// later reads are served from it at the file offset, and a read at or past
    /// its end returns 0 bytes (end of file). Each open gets its own snapshot.
    fn read(&self) -> Result<Vec<u8>>;

    /// Handles one `write` with its full payload and returns how many bytes it
    /// took (at most `data.len()`; a larger value is reported as `data.len()`).
    ///
    /// The default rejects writes with `ENOSYS`.
    fn write(&self, data: &[u8]) -> Result<usize> {
        let _ = data;
        Err(Error::Os(sys::ENOSYS as i32))
    }

    /// Handles one `devctl` command the framework does not handle itself.
    ///
    /// `cmd` is the client's full command word as `<devctl.h>` builds it: the
    /// command (`class << 8 | number`) in the low 16 bits, the data size in
    /// bits 16 to 29, and the direction (`_POSIX_DEVDIR_TO`/`_FROM`) in the top
    /// two bits. `data` holds the client's input and has room for the client's
    /// `nbytes`; write the output into it and return the number of output bytes
    /// (at most `data.len()`). The client's `devctl` returns `EOK` and its
    /// `dev_info` is 0.
    ///
    /// The default rejects every command with `ENOTTY`.
    fn devctl(&self, cmd: u32, data: &mut [u8]) -> Result<usize> {
        let _ = (cmd, data);
        Err(Error::Os(sys::ENOTTY as i32))
    }
}
