//! Server side: one received message, answered at most once.

use core::ffi::{c_int, c_long};
use core::mem::size_of;

use qnx_ipc_sys as sys;

use crate::message::{wire_len, Header, Message, UserKind};
use crate::{Error, Result};

/// Who sent a request, from the kernel's `_msg_info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderInfo {
    /// Process id of the sender.
    pub pid: i32,
    /// Thread id of the sender.
    pub tid: i32,
    /// The sender's priority (the receiving thread runs at this priority).
    pub priority: i32,
    /// The full length of the message the sender sent, which can be more than
    /// the receive buffer holds.
    pub srcmsglen: usize,
}

impl SenderInfo {
    pub(crate) fn from_info(info: &sys::_msg_info64) -> SenderInfo {
        SenderInfo {
            pid: info.pid,
            tid: info.tid,
            priority: i32::from(info.priority),
            srcmsglen: usize::try_from(info.srcmsglen).unwrap_or(0),
        }
    }
}

/// A received message whose sender is blocked waiting for the answer.
///
/// [`reply`](Request::reply) and [`reply_error`](Request::reply_error) consume
/// the request, so it is answered at most once. A request dropped without an
/// answer replies `ENOSYS`, or `EIO` if it is dropped while its thread panics,
/// so a dropped request still answers its client. (A request leaked with
/// `std::mem::forget` never runs its `Drop` and leaves its client blocked.)
/// A request borrows its channel and receive buffer, so it cannot outlive
/// the channel.
#[derive(Debug)]
pub struct Request<'b> {
    rcvid: sys::rcvid_t,
    data: &'b [u8],
    sender: SenderInfo,
    answered: bool,
}

impl<'b> Request<'b> {
    pub(crate) fn new(rcvid: sys::rcvid_t, data: &'b [u8], sender: SenderInfo) -> Request<'b> {
        Request {
            rcvid,
            data,
            sender,
            answered: false,
        }
    }

    /// The message kind from the header (native byte order, as [`Header`] is
    /// sent), or 0 if the message is shorter than the 2-byte kind field.
    pub fn kind(&self) -> u16 {
        match self.data {
            [a, b, ..] => u16::from_ne_bytes([*a, *b]),
            _ => 0,
        }
    }

    /// Who sent the request.
    pub fn sender(&self) -> SenderInfo {
        self.sender
    }

    /// Decodes the payload as an `M`.
    ///
    /// Fails with [`Error::Truncated`] if fewer than `4 + size_of::<M>()` bytes
    /// arrived, and with [`Error::UnknownKind`] if the header's kind is not
    /// `M::KIND`.
    pub fn decode<M: Message>(&self) -> Result<M> {
        let () = UserKind::<M>::OK;
        let expected = wire_len::<M>();
        if self.data.len() < expected {
            return Err(Error::Truncated {
                expected,
                got: self.data.len(),
            });
        }
        let kind = self.kind();
        if kind != M::KIND {
            return Err(Error::UnknownKind(kind));
        }
        Ok(bytemuck::pod_read_unaligned(
            &self.data[size_of::<Header>()..expected],
        ))
    }

    /// The bytes received, header included. This is at most the receive
    /// buffer's capacity; see [`SenderInfo::srcmsglen`] for the full length.
    pub fn bytes(&self) -> &[u8] {
        self.data
    }

    /// Copies the message's bytes from `offset` (counted from the start of the
    /// message, header included) into `out` with `MsgRead`, and returns how many
    /// were copied: `min(out.len(), srcmsglen - offset)`, so 0 at or past the
    /// end. Use it when [`SenderInfo::srcmsglen`] is larger than the receive
    /// buffer.
    pub fn read_into(&self, offset: usize, out: &mut [u8]) -> Result<usize> {
        // SAFETY: `out` is live and exclusively borrowed for the call; the kernel
        // writes at most `out.len()` bytes into it. `rcvid` is a received message
        // that is not answered yet (answering consumes `self`), so its sender is
        // still blocked and its message readable.
        let n = unsafe {
            sys::MsgRead(
                self.rcvid,
                out.as_mut_ptr().cast(),
                out.len() as sys::_Sizet,
                offset as sys::_Sizet,
            )
        };
        if n < 0 {
            return Err(Error::last_os());
        }
        Ok(n as usize)
    }

    /// Replies with `reply` and unblocks the sender, whose `send` returns it.
    ///
    /// The reply status (what the client's `MsgSend` returns) is the reply's
    /// length in bytes, so [`Connection::send_bytes`](crate::Connection::send_bytes)
    /// can report it.
    ///
    /// If the reply fails, the request is dropped and so answers `ENOSYS`.
    pub fn reply<R: bytemuck::Pod>(mut self, reply: &R) -> Result<()> {
        // SAFETY: `reply` is a live, initialized `R` of `size_of::<R>()` bytes that
        // the kernel only reads. `rcvid` is a received, not yet answered message.
        let rc = unsafe {
            sys::MsgReply(
                self.rcvid,
                size_of::<R>() as c_long,
                (reply as *const R).cast(),
                size_of::<R>() as sys::_Sizet,
            )
        };
        if rc == -1 {
            return Err(Error::last_os());
        }
        self.answered = true;
        Ok(())
    }

    /// Unblocks the sender with error `errno`: its `send` returns
    /// `Err(Error::Os(errno))`.
    ///
    /// If the error reply fails, the request is dropped and so answers `ENOSYS`.
    pub fn reply_error(mut self, errno: i32) -> Result<()> {
        // SAFETY: MsgError takes only integers. `rcvid` is a received, not yet
        // answered message.
        let rc = unsafe { sys::MsgError(self.rcvid, errno as c_int) };
        if rc == -1 {
            return Err(Error::last_os());
        }
        self.answered = true;
        Ok(())
    }
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        if !self.answered {
            // A request dropped by a panicking handler answers EIO (the server
            // failed); one dropped normally answers ENOSYS (not handled).
            let errno = if std::thread::panicking() {
                sys::EIO
            } else {
                sys::ENOSYS
            };
            // SAFETY: MsgError takes only integers. `rcvid` was not answered yet.
            // A failure (for example the client is gone) cannot be acted on here
            // and is ignored; this never panics.
            unsafe {
                sys::MsgError(self.rcvid, errno as c_int);
            }
        }
    }
}
