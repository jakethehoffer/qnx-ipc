//! The error type shared by every `qnx-ipc` API.

use core::fmt;
use std::io;

/// Everything that can go wrong in `qnx-ipc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A QNX call failed with this `errno` value.
    Os(i32),
    /// A message was shorter than its type needs.
    Truncated {
        /// Bytes the type needs.
        expected: usize,
        /// Bytes that arrived.
        got: usize,
    },
    /// A message carried a kind that no handler is registered for.
    UnknownKind(u16),
    /// A channel or path name was rejected (for example it contained a NUL byte).
    InvalidName(String),
    /// A pulse code was outside the range open to user pulses.
    InvalidPulseCode(i32),
}

/// Result type of every fallible `qnx-ipc` API.
pub type Result<T> = core::result::Result<T, Error>;

impl Error {
    /// The error of the QNX call that just failed, taken from the calling
    /// thread's `errno`.
    pub fn last_os() -> Error {
        Error::Os(io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }

    /// The `errno` value if this is an OS error.
    pub fn errno(&self) -> Option<i32> {
        match *self {
            Error::Os(code) => Some(code),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Os(code) => write!(f, "{}", io::Error::from_raw_os_error(*code)),
            Error::Truncated { expected, got } => {
                write!(f, "message truncated: expected {expected} bytes, got {got}")
            }
            Error::UnknownKind(kind) => write!(f, "unknown message kind {kind:#06x}"),
            Error::InvalidName(name) => write!(f, "invalid name {name:?}"),
            Error::InvalidPulseCode(code) => write!(f, "invalid pulse code {code}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for io::Error {
    fn from(e: Error) -> io::Error {
        match e {
            Error::Os(code) => io::Error::from_raw_os_error(code),
            Error::Truncated { .. } | Error::UnknownKind(_) => {
                io::Error::new(io::ErrorKind::InvalidData, e)
            }
            Error::InvalidName(_) | Error::InvalidPulseCode(_) => {
                io::Error::new(io::ErrorKind::InvalidInput, e)
            }
        }
    }
}
