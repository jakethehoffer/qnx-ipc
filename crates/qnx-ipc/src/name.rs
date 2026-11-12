//! Named channels: a server registers a name with `name_attach`, and clients
//! find it with [`Connection::open`](crate::Connection::open).

use core::ptr::NonNull;
use std::ffi::CString;

use qnx_ipc_sys as sys;

use crate::channel::{receive_on, SystemMessages};
use crate::serve::{serve_on, Handler};
use crate::{Error, Received, RecvBuf, Result};

/// A channel registered under a local name (`/dev/name/local/<name>`) with
/// `name_attach`. Clients connect with
/// [`Connection::open`](crate::Connection::open). Dropping it removes the name
/// and destroys the channel (`name_detach`).
///
/// QNX creates the channel with `_NTO_CHF_DISCONNECT`, `_NTO_CHF_UNBLOCK` and
/// `_NTO_CHF_COID_DISCONNECT`, so it gets disconnect and unblock pulses. With
/// `_NTO_CHF_UNBLOCK`, a client blocked waiting for a reply that is signalled or
/// times out stays blocked until the server answers; the server gets a
/// [`PulseKind::Unblock`](crate::PulseKind::Unblock) pulse instead. Unlike
/// a plain [`Channel`](crate::Channel), the channel may also receive QNX system
/// messages (kinds `0..=0x1FF`); [`receive`](NamedChannel::receive) and
/// [`serve`](NamedChannel::serve) answer those themselves.
#[derive(Debug)]
pub struct NamedChannel {
    attach: NonNull<sys::name_attach_t>,
    chid: i32,
}

// SAFETY: `attach` points at the `name_attach_t` that name_attach allocated for
// this value alone. It is never read after construction (the chid is copied out)
// and is passed to name_detach once, in Drop, which may run on any thread.
unsafe impl Send for NamedChannel {}
// SAFETY: shared access only copies out `chid`; receiving on one channel from
// several threads at once is supported by the kernel.
unsafe impl Sync for NamedChannel {}

impl NamedChannel {
    /// Registers `name` (`name_attach(NULL, name, 0)`) on a new channel.
    ///
    /// Names are non-empty and use only ASCII letters, digits, `-`, `_` and `.`;
    /// any other name fails with [`Error::InvalidName`] before any kernel call. A
    /// name that is already attached fails with `Error::Os(EEXIST)`.
    pub fn attach(name: &str) -> Result<NamedChannel> {
        let c_name = checked_name(name)?;
        // SAFETY: `c_name` is a NUL-terminated string that outlives the call. A
        // null dispatch pointer asks name_attach to create its own dispatch
        // structure and channel. Flags 0 means a local name.
        let attach = unsafe { sys::name_attach(core::ptr::null_mut(), c_name.as_ptr(), 0) };
        let attach = NonNull::new(attach).ok_or_else(Error::last_os)?;
        // SAFETY: name_attach succeeded, so `attach` points at a valid, initialized
        // `name_attach_t` that stays allocated until name_detach.
        let chid = unsafe { attach.as_ref().chid };
        Ok(NamedChannel { attach, chid })
    }

    /// The QNX channel id behind the name.
    pub fn id(&self) -> i32 {
        self.chid
    }

    /// Blocks until a message or a pulse arrives, and receives it into `buf`,
    /// like [`Channel::receive`](crate::Channel::receive).
    ///
    /// QNX system messages (kinds `0..=0x1FF`), which any client can send, never
    /// reach the caller: an `_IO_CONNECT` (connect) message is answered `EOK`
    /// with no data, every other system message `ENOSYS`, and the receive
    /// continues. (QNX 8.0's `name_open` of a local name sends no message; its
    /// `name_close` produces a disconnect pulse once the process has no other
    /// connection to the channel.)
    /// Messages shorter than the 2-byte kind are returned as they are. Pulses
    /// are returned, and on a disconnect pulse the departed client's server
    /// connection has already been detached.
    ///
    /// A kernel timeout armed with `TimerTimeout` just before this call applies
    /// to the first receive only: if that one gets a system message, the next
    /// receive is not covered.
    pub fn receive<'b>(&self, buf: &'b mut RecvBuf) -> Result<Received<'b>> {
        receive_on(self.chid, buf, SystemMessages::Answer)
    }

    /// The serve loop of [`Channel::serve`](crate::Channel::serve) on this
    /// channel, with system messages answered as in
    /// [`receive`](NamedChannel::receive).
    pub fn serve<H: Handler>(&self, buf: &mut RecvBuf, handler: &mut H) -> Result<()> {
        serve_on(self.chid, SystemMessages::Answer, buf, handler)
    }
}

impl Drop for NamedChannel {
    fn drop(&mut self) {
        // SAFETY: `attach` came from a successful name_attach and is detached only
        // here, once; flags 0 also destroys the dispatch structure and channel
        // that name_attach created. A failure cannot be acted on in drop.
        unsafe {
            sys::name_detach(self.attach.as_ptr(), 0);
        }
    }
}

/// `name` as a C string if it is a valid channel name: non-empty, ASCII letters,
/// digits, `-`, `_` and `.` only. Otherwise [`Error::InvalidName`].
pub(crate) fn checked_name(name: &str) -> Result<CString> {
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !valid {
        return Err(Error::InvalidName(name.to_string()));
    }
    // The checks above exclude NUL, so this cannot fail.
    CString::new(name).map_err(|_| Error::InvalidName(name.to_string()))
}
