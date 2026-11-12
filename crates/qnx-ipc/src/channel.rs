//! Server side: channels and receiving messages and pulses on them.

use core::ffi::{c_int, c_long};
use core::mem::size_of;

use qnx_ipc_sys as sys;

use crate::pulse::Pulse;
use crate::request::{Request, SenderInfo};
use crate::{Error, Result};

bitflags::bitflags! {
    /// Flags for [`Channel::new`], mapped to QNX's `_NTO_CHF_*` values.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ChannelFlags: u32 {
        /// Deliver a `_PULSE_CODE_DISCONNECT` pulse when a client's last
        /// connection goes away (`_NTO_CHF_DISCONNECT`).
        const DISCONNECT = sys::_NTO_CHF_DISCONNECT;
        /// Deliver a `_PULSE_CODE_UNBLOCK` pulse when a blocked client is
        /// signalled or times out, instead of unblocking it (`_NTO_CHF_UNBLOCK`).
        const UNBLOCK = sys::_NTO_CHF_UNBLOCK;
        /// Do not inherit the client's priority when receiving
        /// (`_NTO_CHF_FIXED_PRIORITY`).
        const FIXED_PRIORITY = sys::_NTO_CHF_FIXED_PRIORITY;
        /// Only connections from this process are allowed (`_NTO_CHF_PRIVATE`).
        const PRIVATE = sys::_NTO_CHF_PRIVATE;
    }
}

/// A QNX channel that this process receives messages and pulses on. Dropping it
/// destroys it.
#[derive(Debug)]
pub struct Channel {
    chid: i32,
}

impl Channel {
    /// Creates a channel with `flags`.
    ///
    /// The channel always also has `_NTO_CHF_SENDER_LEN`, so
    /// [`SenderInfo::srcmsglen`] holds the full length the client sent.
    pub fn new(flags: ChannelFlags) -> Result<Channel> {
        let raw = flags.bits() | sys::_NTO_CHF_SENDER_LEN;
        // SAFETY: ChannelCreate takes only an integer flag set and has no memory
        // preconditions.
        let chid = unsafe { sys::ChannelCreate(raw) };
        if chid == -1 {
            return Err(Error::last_os());
        }
        Ok(Channel { chid })
    }

    /// The QNX channel id, for [`Connection::to_process`](crate::Connection::to_process).
    pub fn id(&self) -> i32 {
        self.chid
    }

    /// Blocks until a message or a pulse arrives, and receives it into `buf`.
    ///
    /// A message longer than `buf` is cut to its capacity: the request's
    /// [`bytes`](Request::bytes) hold what fit and
    /// [`SenderInfo::srcmsglen`] the full length. Use
    /// [`Request::read_into`] to read the rest.
    ///
    /// On a disconnect pulse ([`PulseKind::Disconnect`](crate::PulseKind::Disconnect))
    /// this detaches the kernel's server connection to the departed client
    /// (`ConnectDetach(scoid)`) before returning the pulse, which frees it.
    ///
    /// This makes no kernel call before the receive itself, so a kernel timeout
    /// armed with `TimerTimeout` just before it applies to the receive.
    pub fn receive<'b>(&self, buf: &'b mut RecvBuf) -> Result<Received<'b>> {
        receive_on(self.chid, buf, SystemMessages::Deliver)
    }
}

/// What [`receive_on`] does with QNX system messages (kinds `0..=_IO_MAX`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemMessages {
    /// Return them to the caller like any other message ([`Channel`]).
    Deliver,
    /// Answer them and keep receiving, as a `name_attach` server must
    /// ([`NamedChannel`](crate::NamedChannel)): `_IO_CONNECT` gets `EOK` with no
    /// data, every other system message `ENOSYS`.
    Answer,
}

/// Receives the next message or pulse on channel `chid` into `buf`; the shared
/// body of [`Channel::receive`] and
/// [`NamedChannel::receive`](crate::NamedChannel::receive).
///
/// The first kernel call is the receive, so a `TimerTimeout` armed just before
/// applies to it. With [`SystemMessages::Answer`], a system message answered
/// here is followed by another receive that the timeout no longer covers.
pub(crate) fn receive_on(
    chid: i32,
    buf: &mut RecvBuf,
    system: SystemMessages,
) -> Result<Received<'_>> {
    loop {
        let mut info = sys::_msg_info64::default();
        // SAFETY: the kernel writes at most `buf.data.len()` bytes into `buf.data`,
        // which is live and exclusively borrowed for the call, and fills `info`, a
        // live local of the type MsgReceive takes.
        let rcvid = unsafe {
            sys::MsgReceive(
                chid,
                buf.data.as_mut_ptr().cast(),
                buf.data.len() as sys::_Sizet,
                &mut info,
            )
        };
        if rcvid < 0 {
            return Err(Error::last_os());
        }
        if rcvid == 0 {
            return Ok(Received::Pulse(take_pulse(buf)));
        }
        let len = usize::try_from(info.msglen)
            .unwrap_or(0)
            .min(buf.data.len());
        if system == SystemMessages::Answer && len >= 2 {
            let kind = u32::from(u16::from_ne_bytes([buf.data[0], buf.data[1]]));
            if kind <= sys::_IO_MAX {
                answer_system_message(rcvid, kind);
                continue;
            }
        }
        return Ok(Received::Message(Request::new(
            rcvid,
            &buf.data[..len],
            SenderInfo::from_info(&info),
        )));
    }
}

/// Decodes the pulse the kernel just wrote at the start of `buf`, detaching the
/// departed client's server connection if it is a disconnect pulse.
fn take_pulse(buf: &RecvBuf) -> Pulse {
    // SAFETY: on a pulse the kernel wrote a `_pulse` at the start of `buf.data`,
    // which is at least `size_of::<_pulse>()` bytes long (RecvBuf::with_capacity
    // guarantees it). `read_unaligned` copes with the byte buffer's alignment, and
    // every bit pattern is a valid `_pulse` (integers and a union of integers and
    // a pointer).
    let pulse: sys::_pulse = unsafe { buf.data.as_ptr().cast::<sys::_pulse>().read_unaligned() };
    // SAFETY: `sival_int` overlaps the start of the union; any bits are a valid i32.
    let value = unsafe { pulse.value.sival_int };
    if i32::from(pulse.code) == sys::_PULSE_CODE_DISCONNECT {
        // SAFETY: on a disconnect pulse `scoid` is the kernel's server connection
        // to the departed client, which stays allocated until the server detaches
        // it. ConnectDetach takes only an integer; a failure leaves nothing to
        // clean up and is ignored.
        unsafe {
            sys::ConnectDetach(pulse.scoid);
        }
    }
    Pulse {
        code: pulse.code,
        value,
    }
}

/// Answers system message `rcvid` of `kind` (`<= _IO_MAX`) on a named channel:
/// `_IO_CONNECT` (a connect message, as for an open) gets `EOK` with no data, so
/// the open succeeds; anything else gets `ENOSYS`, as the `name_attach`
/// documentation prescribes.
fn answer_system_message(rcvid: sys::rcvid_t, kind: u32) {
    if kind == sys::_IO_CONNECT {
        // SAFETY: `rcvid` is a received, not yet answered message; a null buffer
        // with length 0 sends no data. A failure (the client is gone) leaves
        // nothing to answer and is ignored.
        unsafe {
            sys::MsgReply(rcvid, sys::EOK as c_long, core::ptr::null(), 0);
        }
    } else {
        // SAFETY: MsgError takes only integers; `rcvid` is a received, not yet
        // answered message. A failure (the client is gone) is ignored.
        unsafe {
            sys::MsgError(rcvid, sys::ENOSYS as c_int);
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // SAFETY: `chid` came from a successful ChannelCreate and is destroyed only
        // here, once. A failure cannot be acted on in drop, so it is ignored.
        unsafe {
            sys::ChannelDestroy(self.chid);
        }
    }
}

/// A buffer that messages and pulses are received into.
#[derive(Debug, Clone)]
pub struct RecvBuf {
    data: Vec<u8>,
}

impl RecvBuf {
    /// A buffer that receives up to `bytes` bytes of each message. It is never
    /// smaller than a pulse (16 bytes), so pulses always fit.
    pub fn with_capacity(bytes: usize) -> RecvBuf {
        RecvBuf {
            data: vec![0; bytes.max(size_of::<sys::_pulse>())],
        }
    }
}

/// What [`Channel::receive`] got: a message waiting for an answer, or a pulse.
#[derive(Debug)]
pub enum Received<'b> {
    /// A message. Its sender stays blocked until the request is answered or
    /// dropped.
    Message(Request<'b>),
    /// A pulse. Nobody waits for an answer.
    Pulse(Pulse),
}
