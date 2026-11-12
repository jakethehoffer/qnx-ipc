//! Client side: a connection to a channel and typed sends over it.

use core::ffi::{c_int, c_long, c_void};
use core::mem::size_of;

use qnx_ipc_sys as sys;

use crate::message::{Header, Message, UserKind, FIRST_USER_KIND};
use crate::name::checked_name;
use crate::pulse::PulseCode;
use crate::{Error, Result};

/// A connection to a channel, owned by this process. Dropping it detaches it
/// (`ConnectDetach`), or closes it (`name_close`) if it came from
/// [`Connection::open`].
#[derive(Debug)]
pub struct Connection {
    coid: i32,
    /// Opened by name (`name_open`), so closed with `name_close`.
    named: bool,
}

impl Connection {
    /// Connects to channel `chid` of this process.
    pub fn to_local(chid: i32) -> Result<Connection> {
        Connection::to_process(std::process::id() as i32, chid)
    }

    /// Connects to channel `chid` of process `pid` on this node.
    ///
    /// The connection id is a side-channel id (it does not use a file
    /// descriptor slot) and is closed on `exec`.
    pub fn to_process(pid: i32, chid: i32) -> Result<Connection> {
        // SAFETY: ConnectAttach takes only integers and has no memory
        // preconditions. Node 0 is the local node.
        let coid = unsafe {
            sys::ConnectAttach(
                0,
                pid,
                chid,
                sys::_NTO_SIDE_CHANNEL,
                sys::_NTO_COF_CLOEXEC as i32,
            )
        };
        if coid == -1 {
            return Err(Error::last_os());
        }
        Ok(Connection { coid, named: false })
    }

    /// Connects to the channel registered as `name` by
    /// [`NamedChannel::attach`](crate::NamedChannel::attach), possibly in
    /// another process (`name_open(name, 0)`).
    ///
    /// `name` follows the same rules as for `attach` (non-empty, ASCII letters,
    /// digits, `-`, `_`, `.`), else [`Error::InvalidName`]. A name nobody has
    /// attached fails with `Error::Os(ENOENT)`.
    ///
    /// On QNX 8.0, opening a local name sends the server nothing, so this does
    /// not wait for the server; should a connect message (`_IO_CONNECT`) arrive,
    /// [`NamedChannel`]'s `receive` and `serve` answer it.
    ///
    /// [`NamedChannel`]: crate::NamedChannel
    pub fn open(name: &str) -> Result<Connection> {
        let c_name = checked_name(name)?;
        // SAFETY: `c_name` is a NUL-terminated string that outlives the call.
        // Flags 0 means a local name.
        let coid = unsafe { sys::name_open(c_name.as_ptr(), 0) };
        if coid == -1 {
            return Err(Error::last_os());
        }
        Ok(Connection { coid, named: true })
    }

    /// The QNX connection id.
    pub fn coid(&self) -> i32 {
        self.coid
    }

    /// Sends `msg` (as a [`Header`] with `M::KIND` followed by `msg`'s bytes) and
    /// blocks until the server replies, returning the reply as an `R`.
    ///
    /// If the server replies with fewer bytes than `size_of::<R>()`, the rest of
    /// the returned value is zero. If the server answers with an error (for
    /// example a dropped, unanswered request gives `ENOSYS`), the result is
    /// `Err(Error::Os(errno))`.
    ///
    /// This makes no kernel call before the send itself, so a kernel timeout armed
    /// with `TimerTimeout` just before it applies to the send.
    pub fn send<M: Message, R: bytemuck::Pod>(&self, msg: &M) -> Result<R> {
        let () = UserKind::<M>::OK;
        let header = Header {
            kind: M::KIND,
            reserved: 0,
        };
        let mut reply = R::zeroed();
        let siov = [
            iov(&header as *const Header as *mut c_void, size_of::<Header>()),
            iov(msg as *const M as *mut c_void, size_of::<M>()),
        ];
        let riov = [iov(&mut reply as *mut R as *mut c_void, size_of::<R>())];
        // SAFETY: each send iovec points at a live, initialized value of the stated
        // size, and the kernel only reads it. The reply iovec points at `reply`,
        // which is live for the call; the kernel writes at most `size_of::<R>()`
        // bytes into it, and any bytes are a valid `R` because `R: Pod`.
        let rc = unsafe {
            sys::MsgSendv(
                self.coid,
                siov.as_ptr(),
                siov.len() as sys::_Sizet,
                riov.as_ptr(),
                riov.len() as sys::_Sizet,
            )
        };
        if rc == -1 {
            return Err(Error::last_os());
        }
        Ok(reply)
    }

    /// Sends a message of `kind` (a [`Header`] with `kind`, then `payload`) and
    /// blocks until the server replies, which it writes into `reply`. Returns
    /// the reply length.
    ///
    /// QNX does not tell the client how many bytes the server replied: the
    /// kernel copies at most `reply.len()` bytes of the reply into `reply`, and
    /// `MsgSend` returns the status the server passed to `MsgReply`. A
    /// [`Request::reply`](crate::Request::reply) sets that status to the
    /// reply's length in bytes (the QNX convention), so the returned length is
    /// `min(status, reply.len())`: the bytes of `reply` that hold the server's
    /// answer. A server that replies with status `EOK` (0), as many C servers
    /// do, yields 0 even if it sent data.
    ///
    /// The payload can be far larger than the server's receive buffer; the
    /// server reads the rest with [`Request::read_into`](crate::Request::read_into).
    /// A `kind` below 0x200 (reserved for QNX) fails with `Error::Os(EINVAL)`
    /// without sending. A server error reply gives `Err(Error::Os(errno))`.
    ///
    /// This makes no kernel call before the send itself, so a kernel timeout armed
    /// with `TimerTimeout` just before it applies to the send.
    pub fn send_bytes(&self, kind: u16, payload: &[u8], reply: &mut [u8]) -> Result<usize> {
        if kind < FIRST_USER_KIND {
            return Err(Error::Os(sys::EINVAL as i32));
        }
        let header = Header { kind, reserved: 0 };
        let siov = [
            iov(&header as *const Header as *mut c_void, size_of::<Header>()),
            iov(payload.as_ptr() as *mut c_void, payload.len()),
        ];
        let riov = [iov(reply.as_mut_ptr().cast(), reply.len())];
        // SAFETY: the send iovecs point at `header` and `payload`, live and
        // initialized for their stated lengths; the kernel only reads them. The
        // reply iovec points at `reply`, exclusively borrowed for the call; the
        // kernel writes at most `reply.len()` bytes into it.
        let rc = unsafe {
            sys::MsgSendv(
                self.coid,
                siov.as_ptr(),
                siov.len() as sys::_Sizet,
                riov.as_ptr(),
                riov.len() as sys::_Sizet,
            )
        };
        if rc == -1 {
            return Err(Error::last_os());
        }
        Ok(usize::try_from(rc).unwrap_or(0).min(reply.len()))
    }

    /// Sends a pulse with `code` and `value` without blocking. The pulse carries
    /// the calling thread's priority.
    pub fn send_pulse(&self, code: PulseCode, value: i32) -> Result<()> {
        let priority = thread_priority()?;
        // SAFETY: MsgSendPulse takes only integers. `coid` is an attached
        // connection owned by `self`.
        let rc = unsafe {
            sys::MsgSendPulse(
                self.coid,
                priority,
                c_int::from(code.get()),
                c_long::from(value),
            )
        };
        if rc == -1 {
            return Err(Error::last_os());
        }
        Ok(())
    }
}

/// The calling thread's (assigned) scheduling priority, from `SchedGet`.
pub(crate) fn thread_priority() -> Result<c_int> {
    let mut param = sys::sched_param::default();
    // SAFETY: pid 0 and tid 0 name the calling thread; the kernel fills `param`, a
    // live local of the type SchedGet takes.
    let rc = unsafe { sys::SchedGet(0, 0, &mut param) };
    if rc == -1 {
        return Err(Error::last_os());
    }
    Ok(param.sched_priority)
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.named {
            // SAFETY: `coid` came from a successful name_open and is closed only
            // here, once. A failure cannot be acted on in drop, so it is ignored.
            unsafe {
                sys::name_close(self.coid);
            }
        } else {
            // SAFETY: `coid` came from a successful ConnectAttach and is detached
            // only here, once. A failure cannot be acted on in drop, so it is
            // ignored.
            unsafe {
                sys::ConnectDetach(self.coid);
            }
        }
    }
}

/// An iovec for `len` bytes at `base`.
fn iov(base: *mut c_void, len: usize) -> sys::iovec {
    sys::iovec {
        __bindgen_anon_1: sys::iovec__bindgen_ty_1 { iov_base: base },
        iov_len: len as sys::_Sizet,
    }
}
