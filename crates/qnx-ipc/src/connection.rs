//! Client side: a connection to a channel and typed sends over it.

use core::ffi::{c_int, c_long, c_void};
use core::mem::size_of;

use qnx_ipc_sys as sys;

use crate::message::{Header, Message, UserKind};
use crate::pulse::PulseCode;
use crate::{Error, Result};

/// A connection to a channel, owned by this process. Dropping it detaches it.
#[derive(Debug)]
pub struct Connection {
    coid: i32,
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
        Ok(Connection { coid })
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
        // SAFETY: `coid` came from a successful ConnectAttach and is detached only
        // here, once. A failure cannot be acted on in drop, so it is ignored.
        unsafe {
            sys::ConnectDetach(self.coid);
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
